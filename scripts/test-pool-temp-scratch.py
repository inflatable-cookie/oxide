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

1. premise and oracle sweep: the pool directory is absent *before* any leased
   command, the observation method is proved against plain `cargo` under this
   invocation's identifier, then `oxide --version` and an effect-free
   pass-through are shown to allocate nothing and to create no pool; the
   script's own `TMPDIR` is not inside the fixture pool (a void premise fails).
2. same-volume inherited `TMPDIR` kept, and the FIRST leased command creates the
   pool: the child reports the canonical directory under this invocation's
   identifier, no Oxide directory is created, a planted file inside it is
   untouched, and the product's own creation path is verified afterwards
   (creation line, `config.json` backend, payload, ledger, device, slot).
3. absent, file, relative, unwritable and symlink-escaping inherited
   directories are replaced by one owned directory; an other-device value is
   replaced too when this host has a writable directory on another device, and
   the case says so instead of asserting a control the fixture cannot build.
   The pool's device is read from the monitored root, not from a pool path that
   may not exist yet.
4. an existing **relative** `TMPDIR` resolves against the parent's working
   directory and reaches the child as the canonical absolute path, so a child
   that changes directory still uses the qualified directory; a relative
   value that does not resolve is replaced.
5. aliases: `TMPDIR`, `TMP` and `TEMP` all follow the chosen value, never the
   caller's conflicting values.
6. the owned directory is observed on the selected pool while a gated writer
   holds it. The proof reads the writer's direct readiness record (identifier,
   directory, process group) rather than a `cargo:warning` line, because Cargo
   does not stream build-script warnings while the script runs. `status
   --measure` runs with the same unset aliases as the held child, and a
   baseline/delta measurement around the planted hard-linked pair accounts the
   pair once within rendered truncation while a by-name double count would be
   rejected.
7. normal release removes it; the released slot keeps none.
8. a crash with a surviving writer group keeps the directory and the slot; the
   proof observes that group's closure the way the product does (`kill(-pgid,
   0)` on the pgid the readiness record reports), then a later command must
   neither adopt nor keep the dead command's leftover: this slot's
   same-incarnation directory is reconciled by its next lease, or the
   untrusted slot is evicted whole — both existing rules, and the case asserts
   the outcome without assuming which one runs. The fixture stays inside the
   supervisor's cooperative session (no new session) and only the proof-owned
   `oxide` PID is killed; reaping that PID is not writer closure.
9. `oxide test`, `oxide exec -- <tool>` and `oxide run` children each observe
   the chosen directory: a test binary asserts the prefix it was given, the
   fake exec tool records `$TMPDIR`, and the run program reports and writes
   into it. A run program using an owned directory runs under the existing
   held-lease custody, so its directory is removed only after its group has
   closed; a **live** program plus a competing lease in the same checkout is
   the counterexample case: the competing command takes another slot and the
   live directory and its bytes stay intact until the program ends.
10. a foreign entry and a link in the scratch position refuse the command
    without following, deleting or spawning anything, and the entry is kept.
    That position is the slot of the immediately preceding fresh owned
    observation, never a slot picked from directory enumeration order, and the
    refusal must name exactly the planted path. An over-bound derived path with
    an absent pool leaf refuses the command after reaching the product's own
    creation path, rather than leaving the child on the machine default.
11. one owned 128 MiB scratch APFS image registered as the removable pool: the
    child's temporary files land on the image, the inherited fixture scratch
    (another device) is replaced, release removes the directory, and after an
    ordinary detach the same registration's selection falls back to the
    original directory pool, whose own scratch is used with no stale `/Volumes/<name>`
    directory created. With an inherited same-device `TMPDIR`, the fallback
    keeps it and creates nothing. The custody directory is derived from the
    supervisor's own published `OXIDE_TASK_SCRATCH_ROOT`/`OXIDE_PROOF_EVIDENCE`
    pair: both must be the `scratch` and `evidence` siblings of one non-shared,
    non-group-writable owned root, this proof's workspace must be inside the
    published scratch, and the custody target must stay inside that evidence
    directory — verified before the directory, the image or the intent record
    exists, with no fallback to the workspace root. The intent, attachment and
    detach-attempt records live in that custody directory. The ONE planned
    ordinary detach records its attempt before the call and is never replayed;
    the backing is deleted only after a positively successful detach and its
    matching post-state; any failed, timed-out or unclassifiable attach, detach
    or cleanup keeps the backing and records and aborts as an unknown outcome,
    with the case's own fault kept as the primary cause.
12. the longest injected path is reported in bytes and characters against the
    91-byte socket bound.




Diagnostic mode (`--diagnostic`). The default proof still fails fast: the first
failure stops it exactly as before. `--diagnostic` records a recoverable oracle
failure and continues only with cases whose declared dependencies all PASSED (a
skipped predecessor is not a passed one); a failure in a case that mutates
shared state skips every later case, because that shared state is no longer
positively known; and an unknown child, device, writer or infrastructure
outcome aborts in both modes rather than being recorded as an oracle mismatch,
appending explicit not-run outcomes for every remaining planned case. It runs under the same qualified supervisor, the same 300 s
and 512 MiB bounds and the same custody and cleanup rules, and it writes a
bounded, distinct receipt labelled `qualified: false` with the per-case
outcome, first fault, at most eight secondary faults and every truthful skip
reason. A diagnostic receipt is never counted native evidence and never claims
a PASS. It is dispatched only through `effigy rust:pool-temp-scratch:diagnostic`
(Effigy selectors do not forward passthrough arguments), it is required before
the next counted native run, and a source sweep complements it but never
replaces it. Three diagnostics were executed at earlier heads and all failed on
fixture defects (external-volume internal pool, inherited-temp fixtures inside
the pool, a pre-created empty pool directory); each is corrected here, and the
next diagnostic still needs its own separate activation.

Command/state premise walkthrough for all twelve executable cases. Each item
names the executable commands, their working directory, the state the case
needs, where that state is established in this file, and the output grammar the
oracle reads. This complements the required diagnostic execution; it never
replaces it.

1. premise and oracle sweep (case 1): `oxide --version`, plain
   `cargo check --offline --locked`, `oxide metadata`, `oxide fmt -- --check` in
   the fixture crate. State: crate files and the manifest-consistent lock
   (`new_fixture` -> `make_fixture` + `ensure_fixture_lock`), private
   HOME/CARGO_HOME, resolved absolute toolchain home, `world.scratch` outside
   the pool, and the pool directory ABSENT (`World.__init__`). Grammar: identity
   text; `cargo:warning=OXIDE-TEMP nonce=<current>` then `dir=` from plain Cargo;
   the pool still absent and no slot after the effect-free commands.
2. same-volume inherited TMPDIR and pool creation (case 2): `oxide check` with
   `TMPDIR=<root>/fx/inherited-tmp`. State: the pool is absent, the fixture is a
   same-device sibling, no owned scratch. Grammar: current identifier, `dir=`
   canonical inherited, `temp: inherited <canonical>`, the product's
   ordinary or scratch creation line, and post-conditions read from the
   created pool (`config.json` backend, payload, ledger, device, slot). A
   successful create passed the applicable product admission gates.
3. unusable inherited TMPDIR (case 3): `oxide check` over absent/file/relative/
   unwritable/symlink values, plus a device-gated other-device value. State:
   each bad value under `root/fx`, the device from the monitored root. Grammar:
   current identifier, `temp: oxide <path>`, scratch under the pool payload,
   gone after release.
4. existing relative TMPDIR (case 4): `oxide check` with `TMPDIR=rel-tmp` and
   `FIXTURE_CHDIR`. State: `crate/rel-tmp`, `crate/chdir-target`, a fresh
   identifier per launch (the build script declares both variables to Cargo).
   Grammar: `dir=` and `dir-after-chdir=` canonical, the `after-chdir` marker in
   the qualified directory, and the unresolved relative value replaced.
5. aliases (case 5): `oxide check` twice. State: `fx/alias-tmp` on the pool
   device, conflicting TMP/TEMP, a fresh identifier per run. Grammar: the
   `OXIDE-TEMP TMPDIR=/TMP=/TEMP=` lines all equal the chosen value.
6. owned scratch observed while held (case 6): gated `oxide check`, then
   `oxide status --measure`. State: the writer publishes
   `ready/accounting.txt` (identifier, directory, pgid) after its markers exist
   and before it waits; the proof plants 16 MiB plus one hard link. Grammar:
   parsed readiness record, `temp: oxide` in the measured status queried with the
   same unset aliases, and a baseline/delta that accounts the pair once.
7. normal release (case 7): no command. State: the case 6 command completed with
   exit 0. Grammar: that observed scratch path and every slot scratch position
   absent (`lexists`).
8. crash with a surviving group (case 8): gated `oxide check` killed by PID,
   `kill(-pgid, 0)` polled from the readiness record, then `oxide check`. State:
   the pool and crate from earlier cases; the gate file exists only when the
   proof writes it. Grammar: the readiness record, then the recovery command's
   current `dir=`, the leftover gone, `ESRCH` before recovery, and no session
   escape (the fixture carries no `start_new_session`).
9. test, exec and run children (case 9): `oxide test`, `oxide exec -- /bin/sh`,
   `oxide run`, then a live `oxide run` with a competing `oxide check`. State:
   manifest/lock from case 1, `OXIDE_MAX_ACTIVE=2` set explicitly by the proof,
   a live gate file. Grammar: Cargo's `1 passed` and the named integration test
   on stdout, the test marker in the chosen directory, the exec tool's alias
   lines, the run program's `nonce=`/`run=` lines and its `run-marker` bytes, and
   the competing command's own `dir=` on another slot with the live marker bytes
   unchanged.
10. refusals (case 10): `oxide check`, then `oxide check` against a link and a
    foreign directory at the position of the preceding fresh observation, then
    against an absent over-bound pool leaf. State: the warm check's own
    observation supplies the slot; the planted entries are the proof's; the long
    pool's parent exists but the leaf does not. Grammar: non-zero exit naming the
    exact planted path with `real directory` or `marker`, the entry preserved, no
    child observation, and the `Unix-socket bound` refusal after the long pool
    was created.
11. removable image and fallback (case 11): `hdiutil create|attach -owners on`,
    `diskutil info -plist`, `oxide pool register-removable`, `oxide check`,
    bounded `hdiutil detach`, then two more `oxide check` runs. State: the image
    and its intent/attachment records under the supervisor evidence directory,
    device/UUID/owners-on bound before registration, the original directory pool present
    for the fallback. Grammar: the mount point, the build's `temp: oxide` path
    under `<mount>/oxide-pool`, positive detach (not mounted, mount point not
    recreated), the original directory pool's fallback path, and the same-device inherited
    directory kept with no owned directory created.
12. socket bound (case 12): `oxide check` with no inherited TMPDIR. State: the
    pool and crate from earlier cases, a fresh identifier. Grammar: the current
    `dir=` path ending in `scratch`, within 91 bytes, and the longest accepted
    path across the run reported in bytes and characters.

Two analogous state/grammar errors found by this walkthrough and corrected in
the same batch as the lock fix: (a) the `--locked` oracle ran without the lock
precondition (the native-2 failure), and (b) case 9 read `test result: ok` from
stderr, where Cargo never writes it.

Proof-authoring premise checklist (workflow.md):
1. `resolve_toolchain_home` and `toolchain_environment` preserve the invoking
   rustup home, pin 1.97.1, disable installation and stay offline. `World` owns
   private HOME/CARGO_HOME; `new_fixture` establishes a manifest-consistent
   lock before the fixture's `--locked` oracle. Staging uses the repo lock.
2. `fixture_environment` sets `OXIDE_TEST_HOME_FROM_ENV=1` for the debug build.
3. `fixture_environment` is built fresh; `World` adds only fixture controls.
   `staging_environment` scrubs ambient OXIDE_, XDG_, GIT_, CARGO_, SCCACHE_
   and compiler overrides, then restores only the invoking Cargo cache and the
   explicit offline settings. `--self-check` exercises hostile controls.
4. `make_fixture` asserts no target, and this proof registers no permanent
   mode; the first build cannot start from an adopted checkout.
5. `CARGO_BUILD_ARGV`, `CARGO_ORACLE_ARGV` and `LOCK_ARGV` include their Cargo
   subcommands; there is no fake external Cargo subcommand in this proof.
6. `removable_flow` uses `-owners on`, verifies device, UUID and ownership,
   records intent and one ordinary detach, and keeps uncertain custody.
7. `toolchain_environment` freezes `RUSTFLAGS` for both environments and
   neither imports `CARGO_ENCODED_RUSTFLAGS`.
8. `scratch_root` canonicalizes the supervisor root; `run_session` creates the
   private `pt` workspace beneath it. `World` declares it as
   `OXIDE_SCRATCH_ROOT`, with both directory pools strictly beneath it and an
   explicit 64 MiB cap. Staging and fixture temp directories are within the
   workspace. A /private/tmp fallback exercises the internal path and is not
   scratch-pool coverage. The independently registered image is its own owned
   removable fixture, with backing and custody in supervisor evidence.
9. No `mktemp` is called. `private_dir` creates named directories under the
   supervisor's fresh owned run root.

Current-main oracle sweep (15ce0e3415f6c01a3aad7e9125336dc498e2efd3):
`--version` checks only the oxide identity, not a product version equality;
0.1.0 is the generated fixture's own package and lock, not a product pin.
The staged binary's SHA256 is measured at runtime; no runner or source digest
is pinned. `src/bin/oxide/scratch_io.rs` `create` prints the scratch creation
notice; `store.rs` `create_pool` prints the ordinary fallback notice. Both
initializers use the directory-budget-v1 backend (`src/lease/pool.rs` and
`src/lease/mod.rs`). `child_temp.rs` supplies the temp summaries, real-directory,
marker and Unix-socket refusal wording and the 91-byte bound. `main.rs`
`slot_line` and `store.rs` `SlotSize::describe` supply the measured status
format; the proof parses the fixture's own slot and allocated figure. The
image-selection and fallback are derived from `removable.rs` `select`, and
its image UUID/device come from this run, not historical identity constants.
The builder 1.97.1 is the explicit proof toolchain, not a product version.
No case requires boot Data placement; the other-device probe only reads device
metadata and the image case uses a separately owned mount.

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
# The fixture's checkout/package label. It becomes part of the product's slot
# folder, so it is deliberately one character: the injected product scratch
# path must still fit `MAX_SCRATCH_PATH_BYTES` under either root this proof
# uses.
FIXTURE_NAME = "t"
# The product's slot folder for this fixture is `<label>-<8 hex>`, so the
# folder component is the label plus nine characters, not the label alone.
PRODUCT_FOLDER_CHARS = len(FIXTURE_NAME) + 1 + 8
# Fixed parts of the longest injected product scratch path this proof builds:
#   <mount>/oxide-proof.<8>/scratch/pt/p/payload/slots/<folder>/quick-1/scratch
# `/oxide-proof.` plus mkdtemp's eight random characters, then the proof's own
# leaves (`/scratch`, `/pt`, `/p`), the product's (`/payload`, `/slots`,
# `/quick-1`, `/scratch`) and one `/` before the folder label.
GUARD_LEAF_CHARS = len("/oxide-proof.") + 8
PROOF_LEAVES_CHARS = len("/scratch") + len("/pt") + len("/p")
PRODUCT_LEAVES_CHARS = len("/payload") + len("/slots") + len("/quick-1") + len("/scratch")


def is_inside(path: str, root: str) -> bool:
    """Component-aware containment, independent of trailing separators."""
    path = os.path.normpath(path)
    root = os.path.normpath(root)
    return path == root or path.startswith(root + os.sep)


def fixture_dir(root: str) -> str:
    """This proof's fixture directory: a SIBLING of the pool directory.

    Inherited-`TMPDIR` fixtures must be on the same device as the pool, but the
    product refuses a directory-budget pool that holds unexpected entries, so
    they can never live inside the pool directory itself.
    """
    return os.path.join(root, "fx")


def pool_path(root: str) -> str:
    """The product's pool directory. This proof never creates it.

    An empty pre-created directory is classed by the product as an unknown
    existing pool (`Found::Unknown`), so creation is skipped and opening
    `config.json` fails. Only the product may create the pool, on its own
    creation path, with its applicable admission and format checks.
    """
    return os.path.join(root, "p")


def constructor_paths(root: str) -> list[str]:
    """The directories this proof's constructor creates, never the pool."""
    return [
        os.path.join(root, "home"),
        os.path.join(root, "cargo-home"),
        fixture_dir(root),
        os.path.join(root, "fixture-scratch"),
    ]


def layout_pre_creates_pool(root: str, paths=None) -> bool:
    """Whether a layout would pre-create the pool (the diagnostic3 defect)."""
    paths = constructor_paths(root) if paths is None else paths
    pool = os.path.normpath(pool_path(root))
    return any(
        os.path.normpath(path) == pool or is_inside(path, pool) for path in paths
    )


SCRATCH_CREATION_NOTICE = "oxide: creating a scratch build pool on volume"
ORDINARY_CREATION_NOTICE = "oxide: creating the build pool at"


def creation_notice_matches(stderr: str, scratch_binding: bool) -> bool:
    """The product uses distinct creation paths on scratch and boot storage."""
    notice = SCRATCH_CREATION_NOTICE if scratch_binding else ORDINARY_CREATION_NOTICE
    return notice in stderr


OBSERVATION_MARKER = "OXIDE-TEMP nonce="


def observed_nonce(text: str) -> Optional[str]:
    """The last invocation identifier an observation stream carries."""
    seen = None
    for line in text.splitlines():
        if OBSERVATION_MARKER in line:
            seen = line.split(OBSERVATION_MARKER, 1)[1].strip()
    return seen


def observed_value(text: str, kind: str, nonce: str) -> str:
    """One observation from a stream that observed THIS invocation.

    A stream whose identifier is missing or belongs to an earlier invocation is
    refused before any path is extracted: Cargo may replay a warm affine slot's
    previous build-script warnings, and a replayed path would make a
    kept/replaced oracle vacuous.
    """
    seen = observed_nonce(text)
    expect(
        seen is not None,
        f"the child output carries no observation identifier: {text[-1000:]}",
    )
    expect(
        seen == nonce,
        f"stale observation identifier {seen!r} (expected {nonce!r}): the build "
        "script did not observe this invocation",
    )
    marker = f"OXIDE-TEMP {kind}="
    for line in text.splitlines():
        if marker in line:
            return line.split(marker, 1)[1].strip()
    fail(f"no {kind} observation in the child output: {text[-2000:]}")


def parse_readiness(text: str, nonce: str) -> dict:
    """Strictly parse the gated writer's direct readiness record.

    The build script publishes `nonce`, `dir` and `pgid` atomically after its
    held markers exist and before it waits on the gate. A missing, stale,
    truncated or malformed record is refused rather than guessed at.
    """
    fields = {}
    for line in text.splitlines():
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        fields[key.strip()] = value.strip()
    expect(
        set(fields) == {"nonce", "dir", "pgid"},
        f"malformed readiness record: {text[-500:]}",
    )
    expect(
        fields["nonce"] == nonce,
        f"stale readiness record {fields['nonce']!r} (expected {nonce!r})",
    )
    expect(
        os.path.isabs(fields["dir"]),
        f"the readiness record names no absolute directory: {fields}",
    )
    expect(
        fields["pgid"].isdigit() and int(fields["pgid"]) > 0,
        f"the readiness record names no process group: {fields}",
    )
    return {"nonce": fields["nonce"], "dir": fields["dir"], "pgid": int(fields["pgid"])}


def script_env_reads(script: str) -> set[str]:
    """Every fixture variable the build script reads by literal name."""
    return set(re.findall(r'std::env::var\("([A-Z0-9_]+)"\)', script))


def rerun_env_names(script: str) -> set[str]:
    """Every variable the build script declares to Cargo as rerun-worthy.

    The script emits the directives from one literal name list, so the list is
    read as well as any literal directive line.
    """
    names = set(re.findall(r"cargo:rerun-if-env-changed=([A-Z0-9_]+)", script))
    block = re.search(r"for name in \[(.*?)\]", script, re.S)
    if block is not None and "cargo:rerun-if-env-changed={name}" in script:
        names |= set(re.findall(r'"([A-Z0-9_]+)"', block.group(1)))
    return names


def report_before_wait(script: str, wait: str = "while !std::path::Path::new(&gate).exists()") -> bool:
    """Whether the gated writer publishes readiness before it waits."""
    publish = script.find("FIXTURE_REPORT")
    blocking = script.find(wait)
    return publish != -1 and blocking != -1 and publish < blocking


def slot_of_observed_scratch(observed: str) -> str:
    """The slot directory an observed owned scratch path belongs to."""
    scratch = observed.rstrip("/")
    expect(
        os.path.basename(scratch) == "scratch",
        f"not an owned scratch observation: {observed}",
    )
    return os.path.dirname(scratch)


def refusal_target(observed: str, enumeration=()) -> str:
    """The exact position to plant a refusal fixture in.

    Derived from the immediately preceding successful owned observation, never
    from directory enumeration order: the product selects compatible idle slots
    by its own rule, so a slot picked from `os.walk` order could be one the next
    command never takes.
    """
    del enumeration
    return os.path.join(slot_of_observed_scratch(observed), "scratch")


def refusal_names(stderr: str, path: str) -> bool:
    """Whether the refusal names exactly the planted position."""
    return bool(path) and path.rstrip("/") in stderr


def same_alias_selection(held_env: dict, query_env: dict) -> bool:
    """Whether a measured-status query uses the same temp aliases as the child.

    The product's `temp:` line describes the *querying* command's prospective
    choice, so a status query with a writable same-device `TMPDIR` would
    correctly print `temp: inherited` for itself and prove nothing about the
    held command's owned directory.
    """
    keys = ("TMPDIR", "TMP", "TEMP")
    return all(held_env.get(key) == query_env.get(key) for key in keys)


def delta_verdict(baseline: int, after: int, unique: int) -> tuple:
    """Whether a measured slot delta accounts for one unique allocation.

    The rendered figure is truncated to KiB/MiB, so one MiB of tolerance in
    either direction is honest. The window is centred on ONE allocation: a
    delta near twice the unique bytes is the by-name count of the hard-linked
    pair and must fail, so the upper bound is `unique + 1 MiB`, not
    `2·unique`.
    """
    delta = after - baseline
    tolerance = MIB
    return unique - tolerance <= delta <= unique + tolerance, delta


def image_custody_decision(mounted_after: bool, detach_ok: bool, mount: str) -> str:
    """Keep the backing image unless a positive ordinary detach is established."""
    if detach_ok and mount and not mounted_after:
        return "delete"
    return "keep"


# The qualified supervisor's published layout: one unique owned root holding
# its `scratch` and its retained `evidence` directory as siblings. These names
# are that supervisor's contract, not a guess about part of a path.
GUARD_SCRATCH_MEMBER = "scratch"
GUARD_EVIDENCE_MEMBER = "evidence"
CUSTODY_MEMBER = "removable-image"
# Shared parents are never "the monitored owned root": accepting one would put
# the fixture image somewhere this proof does not own.
SHARED_PARENTS = frozenset(
    {
        "/",
        "/private",
        "/private/tmp",
        "/private/var",
        "/tmp",
        "/var",
        "/Volumes",
        "/Users",
        os.path.expanduser("~"),
    }
)


def custody_layout(
    published_scratch: Optional[str],
    published_evidence: Optional[str],
    world_root: str,
    *,
    realpath=os.path.realpath,
    is_dir=os.path.isdir,
    mode_of=lambda path: os.stat(path).st_mode,
) -> dict:
    """The monitored owned root, its published members and the custody target.

    The qualified supervisor publishes `OXIDE_TASK_SCRATCH_ROOT` and
    `OXIDE_PROOF_EVIDENCE` as two existing siblings under the one unique owned
    root it created (`<owned>/scratch` and `<owned>/evidence`). The image
    custody directory belongs inside that evidence directory, so the
    containment question is about that owned root, not about this proof's
    private workspace (`<owned>/scratch/pt`) or its parent. Every requirement
    is checked before any effect: a missing or unusable published pair, a
    non-sibling, a broad shared parent, a symlinked escape or a workspace
    outside the published scratch refuses here. There is deliberately no
    fallback to the workspace root.
    """
    require(
        bool(published_scratch) and bool(published_evidence),
        "the supervisor published no OXIDE_TASK_SCRATCH_ROOT/OXIDE_PROOF_EVIDENCE "
        "pair, so the image custody topology cannot be established",
    )
    scratch = realpath(published_scratch)
    evidence = realpath(published_evidence)
    workspace = realpath(world_root)
    require(
        is_dir(scratch) and is_dir(evidence),
        f"the published scratch {scratch} and evidence {evidence} must both exist",
    )
    require(
        os.path.basename(scratch) == GUARD_SCRATCH_MEMBER
        and os.path.basename(evidence) == GUARD_EVIDENCE_MEMBER,
        f"the published members are not the guard's "
        f"{GUARD_SCRATCH_MEMBER!r}/{GUARD_EVIDENCE_MEMBER!r} siblings: "
        f"{scratch}, {evidence}",
    )
    owned = os.path.dirname(scratch)
    require(
        os.path.dirname(evidence) == owned,
        f"the evidence directory {evidence} is not a sibling of the published "
        f"scratch directory {scratch} under one owned root",
    )
    require(
        owned not in SHARED_PARENTS and owned != os.path.dirname(owned),
        f"the owned root {owned} is a shared parent, not the guard's unique root",
    )
    require(
        mode_of(owned) & 0o022 == 0,
        f"the owned root {owned} is group- or world-writable",
    )
    require(
        is_inside(workspace, scratch) and workspace != scratch,
        f"this proof's workspace {workspace} is not inside the published "
        f"scratch directory {scratch}",
    )
    custody = os.path.join(evidence, CUSTODY_MEMBER)
    require(
        is_inside(custody, evidence) and is_inside(custody, owned),
        f"the image custody directory {custody} is outside the monitored owned root",
    )
    return {
        "owned_root": owned,
        "scratch": scratch,
        "evidence": evidence,
        "workspace": workspace,
        "custody": custody,
    }


def longest_injected_path(mount_chars: int, folder_chars: int) -> int:
    """The longest product scratch path this proof's layout can construct.

    `folder_chars` is the product's real folder component (`<label>-<8 hex>`),
    not the bare label; the earlier helper under-modelled that width.
    """
    return (
        mount_chars
        + GUARD_LEAF_CHARS
        + PROOF_LEAVES_CHARS
        + PRODUCT_LEAVES_CHARS
        + 1
        + folder_chars
    )
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
  fixtures: private HOME, CARGO_HOME, OXIDE_SCRATCH_ROOT and OXIDE_POOL with
            a 64 MiB cap; fallback to /private/tmp is not scratch-pool coverage
  removable: one owned 128 MiB APFS scratch image at a unique /Volumes/<name>,
             ordinary detach only, never a real drive
  cases:"""
    )
    for index, name in enumerate(CASE_NAMES, start=1):
        print(f"    {index}. {name}")
    print(
        "  writes: ordinary fixtures in OXIDE_TASK_SCRATCH_ROOT; the owned "
        "image's intent, attachment and backing in OXIDE_PROOF_EVIDENCE, both "
        "members of the same monitored owned root; no live pool is touched"
    )


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
    "removable image and absent-volume directory-pool fallback",
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


def _excerpt(text: str, limit: int) -> tuple:
    """A bounded excerpt and its explicit truncation metadata."""
    raw = text.encode()
    if len(raw) <= limit:
        return text, False, len(raw)
    return raw[:limit].decode(errors="ignore"), True, len(raw)


def diagnostic_record(outcomes, faults, skips, aborted) -> str:
    """Bounded, valid-JSON, explicitly unqualified diagnostic record.

    The record is never byte-sliced: an oversized record is rebuilt in a
    leaner shape and declares `collection: "incomplete"` with explicit
    omitted counts and the reasons, so a reader can tell a complete record from
    a bounded one. Every excerpt carries truncation metadata. The first fault
    and the abort flag are preserved in every shape, and the record never
    claims qualification, a PASS or complete collection when it is not.
    """

    def build(case_limit, detail_limit, fault_limit, skip_limit):
        reasons = []
        cases = []
        for case, outcome, detail in outcomes[:case_limit]:
            entry = {"case": case.name, "outcome": outcome}
            if detail:
                text, truncated, size = _excerpt(detail, detail_limit)
                entry.update(
                    {
                        "detail": text,
                        "detail_truncated": truncated,
                        "detail_bytes": size,
                    }
                )
            cases.append(entry)
        if len(outcomes) > case_limit:
            reasons.append(f"cases: kept {case_limit} of {len(outcomes)}")
        first = None
        if faults:
            text, truncated, size = _excerpt(faults[0][1], max(detail_limit, 600))
            first = {
                "case": faults[0][0],
                "detail": text,
                "detail_truncated": truncated,
                "detail_bytes": size,
            }
        secondary = []
        for name, detail in faults[1:fault_limit]:
            text, truncated, size = _excerpt(detail, detail_limit)
            secondary.append(
                {
                    "case": name,
                    "detail": text,
                    "detail_truncated": truncated,
                    "detail_bytes": size,
                }
            )
        recorded = (1 if first else 0) + len(secondary)
        if recorded < len(faults):
            reasons.append(f"faults: kept {recorded} of {len(faults)}")
        skip_entries = []
        for name, reason in skips[:skip_limit]:
            text, truncated, size = _excerpt(reason, detail_limit)
            skip_entries.append(
                {
                    "case": name,
                    "reason": text,
                    "reason_truncated": truncated,
                    "reason_bytes": size,
                }
            )
        if len(skip_entries) < len(skips):
            reasons.append(f"skips: kept {len(skip_entries)} of {len(skips)}")
        return {
            "mode": "diagnostic",
            "qualified": False,
            "evidence_kind": "diagnostic-only, not counted native evidence",
            "collection": "incomplete" if reasons else "complete",
            "incomplete_reasons": reasons,
            "cases_total": len(outcomes),
            "cases_included": len(cases),
            "cases_omitted": len(outcomes) - len(cases),
            "cases": cases,
            "first_fault": first,
            "secondary_faults": secondary,
            "faults_seen": len(faults),
            "faults_recorded": recorded,
            "faults_omitted": len(faults) - recorded,
            "skips_seen": len(skips),
            "skips_recorded": len(skip_entries),
            "skips_omitted": len(skips) - len(skip_entries),
            "skips": skip_entries,
            "aborted_on_unknown_outcome": aborted,
            "receipt_budget_bytes": MAX_DIAGNOSTIC_RECEIPT_BYTES,
        }

    shapes = [
        (len(outcomes), 600, MAX_DIAGNOSTIC_FAULTS, len(skips)),
        (64, 200, MAX_DIAGNOSTIC_FAULTS, 16),
        (32, 80, 4, 8),
        (8, 0, 2, 4),
    ]
    for case_limit, detail_limit, fault_limit, skip_limit in shapes:
        record = build(case_limit, detail_limit, fault_limit, skip_limit)
        for _ in range(2):
            text = json.dumps(record, indent=1, sort_keys=True)
            record["receipt_bytes"] = len(text.encode())
        if len(text.encode()) <= MAX_DIAGNOSTIC_RECEIPT_BYTES:
            return text
    minimal = {
        "mode": "diagnostic",
        "qualified": False,
        "evidence_kind": "diagnostic-only, not counted native evidence",
        "collection": "incomplete",
        "incomplete_reasons": [
            "receipt could not be recorded within its byte budget",
            f"cases: kept 0 of {len(outcomes)}",
            f"faults: kept 0 of {len(faults)}",
            f"skips: kept 0 of {len(skips)}",
        ],
        "cases_total": len(outcomes),
        "cases_included": 0,
        "cases_omitted": len(outcomes),
        "cases": [],
        "first_fault": None,
        "secondary_faults": [],
        "faults_seen": len(faults),
        "faults_recorded": 0,
        "faults_omitted": len(faults),
        "skips_seen": len(skips),
        "skips_recorded": 0,
        "skips_omitted": len(skips),
        "skips": [],
        "aborted_on_unknown_outcome": aborted,
        "receipt_budget_bytes": MAX_DIAGNOSTIC_RECEIPT_BYTES,
    }
    text = json.dumps(minimal, indent=1, sort_keys=True)
    return text


def write_diagnostic_receipt(path: str, outcomes, faults, skips, aborted) -> str:
    with open(path, "wb") as handle:
        handle.write(diagnostic_record(outcomes, faults, skips, aborted).encode())
    return path


def drive(cases, root, receipt=True) -> int:
    """Run the cases, failing fast by default, or diagnosing when asked.

    Default: the first failure stops the proof exactly as before. Diagnostic:
    a declared oracle failure is recorded and the run continues only with cases
    whose declared dependencies all PASSED (a skipped predecessor is not a
    passed one); a failure in a case that mutates shared state skips every
    later case, because that shared state is not positively known; an unknown
    child/device/writer/infrastructure outcome aborts in both modes and the
    remaining planned cases are recorded as not run, so a reached prefix can
    never be reported as a complete collection. Returns 0 only when every case
    passed.
    """
    outcomes = []
    faults = []
    skips = []
    passed = set()
    shared_state_unknown = None
    aborted = False
    for case in cases:
        if shared_state_unknown is not None:
            outcomes.append((case, "skipped", shared_state_unknown))
            skips.append((case.name, shared_state_unknown))
            continue
        blocked = [dep for dep in case.depends if dep not in passed]
        if blocked:
            reason = "dependency did not pass: case(s) " + ", ".join(
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
            for remaining in cases[case.index + 1 :]:
                outcomes.append(
                    (remaining, "not_run", "the run aborted before this case")
                )
                skips.append((remaining.name, "aborted before this case"))
            break
        except Recoverable as error:
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
            for remaining in cases[case.index + 1 :]:
                outcomes.append(
                    (remaining, "not_run", "the run aborted before this case")
                )
                skips.append((remaining.name, "aborted before this case"))
            break
        passed.add(case.index)
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


def work_left(now_ns=None, deadline_ns=None, margin_s=None) -> float:
    """Seconds left before the supervisor's shutdown margin.

    `now_ns`/`deadline_ns`/`margin_s` are injectable so the pure self-check can
    exercise the real deadline decision with no clock or environment. An
    exhausted deadline is an infrastructure condition, not an oracle mismatch:
    it aborts through `UnknownOutcome`, so the default run stops and the
    diagnostic records the abort instead of continuing into another case.
    """
    if deadline_ns is None:
        deadline_ns = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    if now_ns is None:
        now_ns = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    if margin_s is None:
        margin_s = SHUTDOWN_MARGIN_S
    left = (deadline_ns - now_ns) / 1e9 - margin_s
    if left <= 5:
        abort("work deadline reached before the supervisor's shutdown margin")
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


def staging_environment(environ, toolchain_home: str, scratch: str) -> dict:
    """The staging child's environment: the caller's own, minus Cargo/target
    and wrapper overrides, plus the shared toolchain facts."""
    env = {
        key: value
        for key, value in environ.items()
        if not key.startswith(
            ("CARGO_", "SCCACHE_", "OXIDE_", "XDG_", "GIT_", "RUSTC_")
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
            "CARGO_HOME": environ.get("CARGO_HOME") or os.path.join(environ.get("HOME") or os.path.expanduser("~"), ".cargo"),
            "TMPDIR": scratch,
            "TMP": scratch,
            "TEMP": scratch,
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
    env = staging_environment(os.environ, toolchain_home, root)
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
    // Every variable this script reads dynamically is declared here, so Cargo
    // reruns it when one changes. Without these directives a warm affine slot
    // replays the previous invocation's warnings, and a replayed path would
    // make every kept/replaced oracle vacuous.
    for name in [
        "OXIDE_PROOF_NONCE",
        "TMPDIR",
        "TMP",
        "TEMP",
        "FIXTURE_CHDIR",
        "FIXTURE_HOLD",
        "FIXTURE_REPORT",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let nonce = std::env::var("OXIDE_PROOF_NONCE").unwrap_or_default();
    println!("cargo:warning=OXIDE-TEMP nonce={nonce}");
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
        // A gated writer: it writes into the temporary directory, then
        // publishes a direct readiness record and waits for the release file
        // the proof creates. Cargo does not stream `cargo:warning` lines while
        // the script runs, so the record is the only readiness channel this
        // proof may block on.
        let marker = dir.join("held-marker");
        let mut file = std::fs::File::create(&marker).expect("marker");
        file.write_all(&vec![b'x'; 1 << 20]).expect("write");
        drop(file);
        std::os::unix::fs::symlink("held-marker", dir.join("held-link")).expect("link");
        let clobber = dir.join("clobber");
        let _ = std::fs::hard_link(dir.join("held-marker"), &clobber);
        if let Ok(report) = std::env::var("FIXTURE_REPORT") {
            // Atomic publish: the proof never reads a partial record.
            let body = format!(
                "nonce={nonce}\ndir={}\npgid={}\n",
                dir.display(),
                unsafe { getpgrp() }
            );
            let staged = report.clone() + ".tmp";
            std::fs::write(&staged, body).expect("readiness");
            std::fs::rename(&staged, &report).expect("readiness publish");
        }
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
    println!(
        "OXIDE-TEMP nonce={}",
        std::env::var("OXIDE_PROOF_NONCE").unwrap_or_default()
    );
    println!("OXIDE-TEMP run={}", dir.display());
    // Visible on purpose: a temporary directory that cannot take this write is
    // a real failure of the routing under test, never silently ignored.
    std::fs::write(dir.join("run-marker"), b"run").expect("run-marker");
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
name = "t"
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
    """The private account, pool, Cargo home and checkout of one proof run.

    The pool directory is deliberately NOT created here: the first leased
    command must create it through the product's own creation path (exclusion
    gate, format, ledger, payload, device), which is what case 2 asserts. The
    directory/device seams are injectable so the pure self-check exercises this
    real constructor with no filesystem.
    """

    def __init__(
        self,
        root: str,
        subject: str,
        toolchain_home: str,
        *,
        make_dir=private_dir,
        device_of=None,
        exists=os.path.exists,
    ) -> None:
        self.root = root
        self.subject = subject
        self.toolchain_home = toolchain_home
        self.device_of = device_of or (lambda path: os.stat(path).st_dev)
        self.exists = exists
        for path in constructor_paths(root):
            make_dir(path)
        self.home = os.path.join(root, "home")
        self.cargo_home = os.path.join(root, "cargo-home")
        self.pool = pool_path(root)
        # Sibling of the pool directory, same device: inherited-TMPDIR fixtures
        # belong here, never inside the pool.
        self.fixtures = fixture_dir(root)
        self.scratch = os.path.join(root, "fixture-scratch")
        evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
        self.evidence = evidence if evidence and os.path.isdir(evidence) else root
        expect(
            not self.exists(self.pool),
            f"the pool directory {self.pool} exists before the product created it",
        )
        expect(
            self.device_of(self.fixtures) == self.device_of(root),
            "the fixture directory must be on the monitored root's device",
        )
        self.base = fixture_environment(
            os.environ, toolchain_home, self.home, self.cargo_home
        )
        self.base.update(
            {
                "TMPDIR": self.scratch,
                "TMP": self.scratch,
                "TEMP": self.scratch,
                "OXIDE_SCRATCH_ROOT": self.root,
                "OXIDE_POOL": self.pool,
                "OXIDE_POOL_CREATE": "1",
                "OXIDE_POOL_CAP": "64MiB",
                # A public product control, set explicitly: the live-program
                # case needs two concurrent slots, and the default already
                # clamps to at least two rather than proving a limit-one defect.
                "OXIDE_MAX_ACTIVE": "2",
            }
        )
        self.nonce_seed = f"{os.getpid()}-{int(STARTED)}"
        self.nonces = 0
        self.accepted: list[str] = []
        self.last_owned = ""

    def launch(
        self, base: Optional[dict] = None, **overrides: Optional[str]
    ) -> dict:
        """A fresh launch environment, cloned from `base` when one is given.

        Cloning must NOT reuse the base's observation identifier: several
        launches derived from one dictionary would share an identifier, so a
        later command could pass by replaying an earlier run's warnings. The
        identifier is always minted here, on the launch itself.
        """
        self.nonces += 1
        env = dict(base if base is not None else self.base)
        env["OXIDE_PROOF_NONCE"] = f"{self.nonce_seed}-{self.nonces}"
        for key, value in overrides.items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        return env

    def env(self, **overrides: Optional[str]) -> dict:
        # A fresh identifier on every launch: Cargo reruns the build script
        # when it changes, and every consumed observation must carry it.
        return self.launch(None, **overrides)

    def observe(self, text: str, kind: str, env: dict) -> str:
        nonce = env.get("OXIDE_PROOF_NONCE")
        expect(
            nonce is not None,
            "the command environment carries no observation identifier",
        )
        value = observed_value(text, kind, nonce)
        if kind == "dir":
            self.accepted.append(value.rstrip("/"))
        return value

    def capture(self, name: str) -> tuple:
        """Owned files for one asynchronous child, never an undrained pipe."""
        directory = private_dir(os.path.join(self.root, "log"))
        return (
            os.path.join(directory, name + ".out"),
            os.path.join(directory, name + ".err"),
        )

    def readiness_path(self, name: str) -> str:
        path = os.path.join(private_dir(os.path.join(self.root, "ready")), name + ".txt")
        if os.path.lexists(path):
            os.unlink(path)
        if os.path.lexists(path + ".tmp"):
            os.unlink(path + ".tmp")
        return path

    def readiness(self, path: str, env: dict, child, timeout: float = 60) -> dict:
        """Poll the writer's direct readiness record within the real deadline."""
        require(
            is_inside(path, self.root),
            f"the readiness record {path} is outside the owned root",
        )
        nonce = env.get("OXIDE_PROOF_NONCE", "")
        deadline = time.monotonic() + min(work_left(), timeout)
        text = ""
        while time.monotonic() < deadline:
            if os.path.isfile(path):
                text = read_text(path)
                if text.strip():
                    break
            if child.poll() is not None:
                break
            time.sleep(0.05)
        expect(
            bool(text.strip()),
            f"the gated writer published no readiness record at {path} "
            f"(exit {child.poll()})",
        )
        ready = parse_readiness(text, nonce)
        expect(
            ready["dir"].rstrip("/") == canonical(ready["dir"]),
            f"the readiness record's directory is not canonical: {ready['dir']}",
        )
        return ready

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

    def fixture(self, name: str) -> str:
        """One fixture path in the sibling directory, never inside the pool."""
        path = os.path.join(self.fixtures, name)
        expect(
            not is_inside(canonical(path), canonical(self.pool)),
            f"fixture {path} would be inside the pool directory",
        )
        return path

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


# ---------------------------------------------------------------------------


def held_by_pool(world: World, path: str) -> None:
    real = os.path.realpath(path)
    pool = os.path.realpath(world.pool)
    expect(real.startswith(pool + os.sep), f"{real} is not inside the pool {pool}")


def premise_sweep(world: World, crate: str) -> None:
    case(CASE_NAMES[0])
    # The pool must not exist yet: only the first leased command may create it.
    expect(
        not os.path.lexists(world.pool),
        f"the pool directory {world.pool} exists before any leased command",
    )
    expect(
        world.device_of(world.fixtures) == world.device_of(world.root),
        "the fixture directory is not on the monitored root's device",
    )
    # The script's own TMPDIR is the fixture scratch, never inside the pool:
    # otherwise every later "kept" assertion would be a void premise.
    expect(
        not is_inside(canonical(world.scratch), canonical(world.pool)),
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
    # Oracle: plain Cargo's build script reports the inherited directory under
    # THIS invocation's identifier, so the observation method really sees the
    # child environment.
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
    plain_dir = world.observe(plain.stderr, "dir", plain_env).rstrip("/")
    expect(
        plain_dir == canonical(world.scratch),
        f"plain cargo did not report the inherited TMPDIR: {plain.stderr[-1000:]}",
    )
    # Effect-free commands allocate nothing and create no pool.
    world.run(["metadata", "--format-version", "1"], cwd=crate, env=world.env())
    world.run(["fmt", "--", "--check"], cwd=crate, env=world.env(), check=False)
    expect(
        not os.path.lexists(world.pool),
        "an effect-free command created the pool",
    )
    expect(not world.slot_roots(), "an effect-free command created a slot")


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


def measure_slot(world: World, crate: str, slot: str, env: dict) -> int:
    """One measured-status reading for the held slot, in the same aliases."""
    measured = world.run(["status", "--measure"], cwd=crate, env=env, check=False)
    expect(
        measured.returncode == 0,
        f"status --measure failed: {measured.stderr[-1500:]}",
    )
    expect(
        "temp: oxide" in measured.stdout,
        f"measured status did not name the pool scratch: {measured.stdout[-1500:]}",
    )
    return measured_slot_bytes(measured.stdout, slot_id_of(slot))


def inherited_kept(world: World, crate: str) -> None:
    case(CASE_NAMES[1])
    inherited = private_dir(world.fixture("inherited-tmp"))
    keep = os.path.join(inherited, "effigy-evidence")
    write_private(keep, "retained")
    env = world.env(TMPDIR=inherited, TMP=None, TEMP=None)
    # This is the FIRST production `oxide` command that needs the pool, so it
    # is the one that must create it. Nothing may pre-create it.
    expect(not os.path.lexists(world.pool), "the pool exists before its creating command")
    result = world.run(["check"], cwd=crate, env=env)
    observed = world.observe(result.stderr, "dir", env)
    expect(
        observed.rstrip("/") == canonical(inherited),
        f"the child did not keep the canonical inherited directory: {observed}",
    )
    expect(
        not any(os.path.lexists(path) for path in world.scratch_dirs()),
        "Oxide created its own directory although the inherited one was kept",
    )
    expect(
        open(keep, encoding="utf-8").read() == "retained",
        "the inherited directory's content changed",
    )
    # The selected-volume path has its own creation notice; the internal
    # fallback retains the ordinary notice. Both are product creation paths.
    expect(
        creation_notice_matches(
            result.stderr, os.path.isfile(os.path.join(world.pool, "oxide-managed", "scratch.json"))
        ),
        f"the first leased command did not create the pool: {result.stderr[-1500:]}",
    )
    expect(os.path.isdir(world.pool), "the first leased command did not create the pool")
    config_text = read_text(os.path.join(world.pool, "config.json"))
    try:
        config = json.loads(config_text)
    except ValueError as error:
        abort(f"the created pool's config.json is unreadable: {error}")
    expect(
        config.get("backend") == "directory-budget-v1",
        f"the created pool records backend {config.get('backend')!r}",
    )
    expect(
        os.path.isdir(os.path.join(world.pool, "payload")),
        "the created pool has no payload directory",
    )
    expect(
        bool(world.slot_roots()),
        "the first leased command created no slot in the new pool",
    )
    ledger = os.path.join(world.pool, "ledger.json")
    expect(
        os.path.isfile(ledger) and bool(read_text(ledger).strip()),
        "the created pool has no ledger",
    )
    expect(
        world.device_of(world.pool) == world.device_of(world.fixtures),
        "the created pool is not on the fixture directory's device",
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
    missing = world.fixture("missing-tmp")
    closed = private_dir(world.fixture("closed-tmp"))
    os.chmod(closed, 0o500)
    file_path = world.fixture("not-a-dir")
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
    pool_device = world.device_of(world.root)
    expect(
        world.device_of(world.pool) == pool_device,
        "the pool is not on the monitored root's device",
    )
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
            env = world.env(TMPDIR=value)
            result = world.run(["check"], cwd=crate, env=env)
            observed = world.observe(result.stderr, "dir", env)
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
                not os.path.lexists(observed.rstrip("/")),
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
    env = world.env(TMPDIR="rel-tmp", TMP=None, TEMP=None)
    result = world.run(["check"], cwd=crate, env=env)
    observed = world.observe(result.stderr, "dir", env)
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
        not any(os.path.lexists(path) for path in world.scratch_dirs()),
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
    chdir_env = world.env(
        TMPDIR="rel-tmp",
        TMP=None,
        TEMP=None,
        FIXTURE_CHDIR=chdir_target,
    )
    chdir_run = world.run(["check"], cwd=crate, env=chdir_env)
    expect(
        world.observe(chdir_run.stderr, "dir-after-chdir", chdir_env).rstrip("/")
        == canonical(relative),
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
    missing_env = world.env(TMPDIR="missing-rel", TMP=None, TEMP=None)
    replaced_run = world.run(["check"], cwd=crate, env=missing_env)
    observed = world.observe(replaced_run.stderr, "dir", missing_env)
    held_by_pool(world, observed)
    expect(
        os.path.basename(observed.rstrip("/")) == "scratch",
        f"an unresolved relative value was not replaced: {observed}",
    )


def aliases(world: World, crate: str) -> None:
    case(CASE_NAMES[4])
    inherited = private_dir(world.fixture("alias-tmp"))
    env = world.env(
        TMPDIR=inherited,
        TMP=os.path.join(world.root, "elsewhere-tmp"),
        TEMP=os.path.join(world.root, "third-tmp"),
    )
    result = world.run(["check"], cwd=crate, env=env)
    names = {
        name: world.observe(result.stderr, name, env) for name in ("TMPDIR", "TMP", "TEMP")
    }
    expect(
        names["TMPDIR"] == names["TMP"] == names["TEMP"],
        f"the aliases did not follow one chosen value: {names}",
    )
    expect(
        names["TMPDIR"].rstrip("/") == canonical(inherited),
        f"the aliases did not follow the inherited TMPDIR: {names}",
    )
    # The same, in the owned branch.
    owned_env = world.env(
        TMPDIR=None,
        TMP=os.path.join(world.root, "elsewhere-tmp"),
        TEMP=os.path.join(world.root, "third-tmp"),
    )
    owned = world.run(["check"], cwd=crate, env=owned_env)
    names = {
        name: world.observe(owned.stderr, name, owned_env)
        for name in ("TMPDIR", "TMP", "TEMP")
    }
    expect(
        names["TMPDIR"] == names["TMP"] == names["TEMP"],
        f"the aliases did not follow the owned directory: {names}",
    )
    held_by_pool(world, names["TMPDIR"])


def accounting(world: World, crate: str) -> None:
    case(CASE_NAMES[5])
    gate = os.path.join(world.root, "release")
    if os.path.lexists(gate):
        os.unlink(gate)
    report = world.readiness_path("accounting")
    env = world.env(
        TMPDIR=None, TMP=None, TEMP=None, FIXTURE_HOLD=gate, FIXTURE_REPORT=report
    )
    out_path, err_path = world.capture("accounting")
    scratch = ""
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        child = subprocess.Popen(
            [world.subject, "check"],
            cwd=crate,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=err,
        )
    try:
        ready = world.readiness(report, env, child)
        observed = ready["dir"]
        held_by_pool(world, observed)
        scratch = observed.rstrip("/")
        slot = slot_of_observed_scratch(observed)
        world.last_owned = scratch
        expect(
            os.path.basename(scratch) == "scratch",
            f"the unset TMPDIR did not become the owned scratch: {observed}",
        )
        expect(
            os.path.isdir(slot),
            f"the observed scratch does not belong to an existing slot: {slot}",
        )
        marker = os.path.join(scratch, "held-marker")
        expect(
            os.path.isfile(marker),
            "the writer's marker is not inside the owned directory",
        )
        expect(
            os.path.getsize(marker) == MIB,
            f"the writer's held marker changed size: {os.path.getsize(marker)}",
        )
        # The measured status must run with the SAME unset aliases as the held
        # child, or it correctly describes its own prospective inherited value
        # instead of the held command's owned directory.
        baseline_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
        expect(
            same_alias_selection(env, baseline_env),
            "the measured status query does not use the held child's aliases",
        )
        baseline = measure_slot(world, crate, slot, baseline_env)
        # 16 MiB of unique data plus a hard link to one 8 MiB file: a walk that
        # counted the link by name would measure about 24 MiB. The baseline is
        # measured first, so build intermediates and the held marker cannot
        # hide behind an unmeasured absolute total.
        first = os.path.join(scratch, "first")
        second = os.path.join(scratch, "second")
        link = os.path.join(scratch, "first-link")
        for name, path in (("first", first), ("second", second)):
            with open(path, "wb") as handle:
                handle.write(b"x" * (8 * MIB))
        os.link(first, link)
        expect(
            os.stat(first).st_ino == os.stat(link).st_ino
            and os.stat(first).st_dev == os.stat(link).st_dev,
            "the planted pair is not one inode",
        )
        after_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
        after = measure_slot(world, crate, slot, after_env)
        unique = 16 * MIB
        ok, delta = delta_verdict(baseline, after, unique)
        say(
            f"measured slot {slot_id_of(slot)}: baseline {baseline}, after {after}, "
            f"delta {delta} for {unique} unique bytes"
        )
        expect(
            ok,
            f"the measured delta {delta} does not account the hard-linked pair "
            f"once (~{unique} bytes; a by-name count would be ~{2 * unique})",
        )
        expect(
            os.path.getsize(first) == 8 * MIB and os.path.getsize(link) == 8 * MIB,
            "the planted payload changed size",
        )
        expect(
            os.path.getsize(marker) == MIB,
            "status --measure changed the held marker's bytes",
        )
    finally:
        write_private(gate, "")
        try:
            child.wait(timeout=min(work_left(), 60))
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)
            abort("the released gated writer did not exit after its release")
    expect(
        child.returncode == 0,
        f"the released command failed with {child.returncode}: {read_text(err_path)[-1500:]}",
    )
    # Post-completion warning oracles, read from the owned file.
    released = read_text(err_path)
    expect(
        "OXIDE-TEMP released" in released,
        f"the released writer reported no completion: {released[-1000:]}",
    )
    expect(
        world.observe(released, "dir", env).rstrip("/") == scratch,
        "the released command's observation changed after completion",
    )
    expect(
        not os.path.lexists(scratch),
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
    if os.path.lexists(gate):
        os.unlink(gate)
    report = world.readiness_path("crash")
    env = world.env(
        TMPDIR=None, TMP=None, TEMP=None, FIXTURE_HOLD=gate, FIXTURE_REPORT=report
    )
    out_path, err_path = world.capture("crash")
    scratch = ""
    slot = ""
    pgid = 0
    # No new session: this fixture stays inside the qualified supervisor's
    # cooperative session, and the product already owns Cargo's group.
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        child = subprocess.Popen(
            [world.subject, "check"],
            cwd=crate,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=err,
        )
    try:
        ready = world.readiness(report, env, child)
        scratch = ready["dir"].rstrip("/")
        held_by_pool(world, scratch)
        expect(os.path.isdir(scratch), "no owned directory while holding")
        slot = slot_of_observed_scratch(scratch)
        pgid = ready["pgid"]
        # Kill only Oxide itself: its writer group survives, so closure is
        # unknown and the directory must stay with the held slot. Reaping this
        # PID is not writer closure.
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
        after_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
        after = world.run(["check"], cwd=crate, env=after_env)
        observed = world.observe(after.stderr, "dir", after_env).rstrip("/")
        held_by_pool(world, observed)
        expect(
            not os.path.lexists(leftover),
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
            not os.path.lexists(observed),
            "the next command kept an owned directory after its own release",
        )
    finally:
        write_private(gate, "")
        if child.poll() is None:
            child.kill()
            try:
                child.wait(timeout=min(work_left(), 10))
            except subprocess.TimeoutExpired:
                pass
            # Killing the Oxide PID says nothing about its writer group, so the
            # custody of this fixture is unknown and must abort as such — it is
            # never downgraded to an oracle failure.
            abort(
                "the gated crash child had to be forced closed, so its writers "
                "are unknown and its scratch custody is uncertain"
            )


def spawns(world: World, crate: str) -> None:
    case(CASE_NAMES[8])
    # An inherited directory reaches every child and is never cleaned.
    inherited = private_dir(world.fixture("spawn-tmp"))
    # One fresh environment per launch. `world.launch(<base>, ...)` clones and
    # re-mints the identifier, so no two launches share one.
    tested_env = world.env(
        TMPDIR=inherited, TMP=None, TEMP=None, TEMP_EXPECT_PREFIX=inherited
    )
    tested = world.run(["test"], cwd=crate, env=tested_env)
    results = tested.stdout + tested.stderr
    expect(
        "test result: ok" in results and "0 failed" in results,
        f"oxide test did not pass: {results[-1500:]}",
    )
    # The intended integration test really ran, in the chosen directory.
    expect(
        "reports_the_child_temporary_directory" in results and "1 passed" in results,
        f"the intended test did not run: {results[-1500:]}",
    )
    expect(
        open(os.path.join(inherited, "test-marker"), "rb").read() == b"test",
        "the test binary did not write its marker into the chosen directory",
    )
    exec_env = world.env(TMPDIR=inherited, TMP=None, TEMP=None)
    tool = world.run(
        ["exec", "--", "/bin/sh", "-c", 'printf "%s\\n%s\\n%s" "$TMPDIR" "$TMP" "$TEMP"'],
        cwd=crate,
        env=exec_env,
    )
    lines = [line.strip() for line in tool.stdout.splitlines() if line.strip()]
    expect(
        {line.rstrip("/") for line in lines} == {canonical(inherited)},
        f"the exec tool did not inherit the chosen directory: {lines}",
    )
    run_env = world.env(TMPDIR=inherited, TMP=None, TEMP=None)
    ran = world.run(["run"], cwd=crate, env=run_env)
    expect(
        world.observe(ran.stdout, "run", run_env).rstrip("/") == canonical(inherited),
        f"the run program did not inherit the chosen directory: {ran.stdout[-1000:]}",
    )
    expect(
        open(os.path.join(inherited, "run-marker"), "rb").read() == b"run",
        "the run program did not write into the inherited directory",
    )
    expect(
        os.path.isdir(inherited),
        "the inherited directory was removed after a run program",
    )
    expect(
        not any(os.path.lexists(path) for path in world.scratch_dirs()),
        "an inherited-only child tree still created an owned directory",
    )

    # The owned branch reaches the same children. The program is still the
    # directory's writer, so it runs under the held lease and the directory is
    # removed only after its group has closed.
    prefix = os.path.join(os.path.realpath(world.pool), "payload")
    owned_test_env = world.env(
        TMPDIR=None, TMP=None, TEMP=None, TEMP_EXPECT_PREFIX=prefix
    )
    tested = world.run(["test"], cwd=crate, env=owned_test_env)
    results = tested.stdout + tested.stderr
    expect(
        "1 passed" in results and "0 failed" in results,
        f"the owned-branch test did not run: {results[-1500:]}",
    )
    owned_exec_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
    tool = world.run(
        ["exec", "--", "/bin/sh", "-c", 'printf "%s" "$TMPDIR"'],
        cwd=crate,
        env=owned_exec_env,
    )
    held_by_pool(world, tool.stdout.strip())
    owned_run_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
    ran = world.run(["run"], cwd=crate, env=owned_run_env)
    observed = world.observe(ran.stdout, "run", owned_run_env).rstrip("/")
    held_by_pool(world, observed)
    expect(
        not os.path.lexists(observed),
        "the run program's owned directory survived its own release",
    )

    # A live program plus a competing lease: the lease fences the owned
    # directory from reconciliation and eviction, so a further command in the
    # same checkout must take another slot and leave the live directory and
    # its bytes intact. Same-incarnation ownership alone is not closure.
    gate = os.path.join(world.root, "run-release")
    if os.path.lexists(gate):
        os.unlink(gate)
    out_path, err_path = world.capture("live")
    live_env = world.launch(owned_run_env, RUN_HOLD=gate)
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        live = subprocess.Popen(
            [world.subject, "run"],
            cwd=crate,
            env=live_env,
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=err,
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
        expect(
            open(holding, "rb").read() == b"live",
            "the live program's marker does not carry its exact bytes",
        )
        competing_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
        competing = world.run(["check"], cwd=crate, env=competing_env)
        competing_dir = world.observe(
            competing.stderr, "dir", competing_env
        ).rstrip("/")
        expect(
            competing_dir != live_dir,
            "the competing command reused the live program's slot",
        )
        expect(
            os.path.isdir(live_dir),
            "a competing lease removed the live program's owned directory",
        )
        expect(
            open(holding, "rb").read() == b"live",
            "a competing lease changed the live program's bytes",
        )
        # The program's own announcement is still in the observation stream.
        expect(
            world.observe(read_text(out_path), "run", live_env).rstrip("/") == live_dir,
            "the live program did not report its own chosen directory",
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
            not os.path.lexists(live_dir),
            "the owned directory survived the program's positive closure",
        )
    finally:
        write_private(gate, "")
        if live.poll() is None:
            live.kill()
            live.wait(timeout=10)
            abort("the live run program had to be forced closed, so its writers are unknown")


def refusals(world: World, crate: str) -> None:
    case(CASE_NAMES[9])
    outside = private_dir(os.path.join(world.root, "outside"))
    keep = os.path.join(outside, "keep")
    write_private(keep, "foreign")
    # The target slot comes from this command's OWN fresh observation, never
    # from directory enumeration order: the product selects compatible idle
    # slots by its own rule, so a slot picked from a walk could be one the next
    # command never takes.
    warm_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
    warm = world.run(["check"], cwd=crate, env=warm_env)
    observed = world.observe(warm.stderr, "dir", warm_env).rstrip("/")
    held_by_pool(world, observed)
    expect(
        not os.path.lexists(observed),
        "the warm command kept its own owned directory",
    )
    scratch = refusal_target(observed, world.slot_roots())
    slot = slot_of_observed_scratch(observed)
    expect(
        os.path.isdir(slot),
        f"the observation does not name an existing slot: {slot}",
    )
    say(f"refusal target: slot {slot_id_of(slot)} from its own fresh observation")
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
            refusal_names(refused.stderr, scratch),
            f"the refusal did not name the planted position {scratch}: "
            f"{refused.stderr[-1500:]}",
        )
        expect(
            "OXIDE-TEMP dir=" not in refused.stderr
            and "OXIDE-TEMP nonce=" not in refused.stderr,
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
        refusal_names(refused.stderr, scratch),
        f"the refusal did not name the planted position {scratch}: "
        f"{refused.stderr[-1500:]}",
    )
    expect(
        "OXIDE-TEMP dir=" not in refused.stderr
        and "OXIDE-TEMP nonce=" not in refused.stderr,
        "a child ran with an unqualified temporary directory",
    )
    expect(
        open(os.path.join(scratch, "not-ours"), encoding="utf-8").read() == "keep",
        "the foreign entry was changed",
    )
    shutil.rmtree(scratch)
    # An over-bound derived path refuses instead of leaving the child on the
    # machine default. The pool leaf stays absent, so the command must reach
    # the product's own creation path before the length refusal.
    long_parent = private_dir(os.path.join(world.base["OXIDE_SCRATCH_ROOT"], "nested" * 8))
    long_pool = os.path.join(long_parent, "pool")
    expect(
        not os.path.lexists(long_pool),
        "the over-bound pool directory must stay absent before its command",
    )
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
        os.path.isdir(long_pool),
        "the over-bound pool was never created, so the refusal was not the "
        "socket bound",
    )
    expect(
        "OXIDE-TEMP dir=" not in refused.stderr
        and "OXIDE-TEMP nonce=" not in refused.stderr,
        "a child ran with an unqualified temporary directory",
    )


class SystemEffects:
    """The removable case's OS effects, in one seam the pure tests replace."""

    def canonical(self, path: str) -> str:
        return canonical(path)

    def is_dir(self, path: str) -> bool:
        return os.path.isdir(path)

    def mode_of(self, path: str) -> int:
        return os.stat(path).st_mode

    def ismount(self, path: str) -> bool:
        return os.path.ismount(path)

    def exists(self, path: str) -> bool:
        return os.path.exists(path)

    def lexists(self, path: str) -> bool:
        return os.path.lexists(path)

    def unlink(self, path: str) -> None:
        os.unlink(path)

    def make_private(self, path: str) -> str:
        return private_dir(path)

    def write_private(self, path: str, data: str) -> None:
        write_private(path, data)

    def slot_tree(self, root: str) -> list:
        return [path for path, _, _ in os.walk(root)]

    def work_left(self) -> float:
        return work_left()

    def run(self, argv: list, *, timeout: float, text: bool = True):
        return subprocess.run(argv, capture_output=True, text=text, timeout=timeout)


def custody_cleanup(effects, state: dict, image: str, custody: str) -> dict:
    """One custody decision for one exit of the removable case.

    Nothing is deleted unless the ONE planned ordinary detach was positively
    established: exit 0, not mounted and the mount point not recreated. After
    any other exit the backing image and its intent, attachment and attempt
    records are kept. A bounded cleanup detach runs only when the planned
    detach was never attempted and the attachment is positively bound to this
    fixture; its own post-state is verified, and a failure, timeout or
    unclassifiable result leaves custody unknown rather than being reported as
    recovered. The unknown is never downgraded to the case's own fault.
    """
    if not state["intent"]:
        return {"uncertain": False, "detail": "", "action": "none"}
    if state["detached_ok"]:
        if effects.exists(image):
            effects.unlink(image)
        return {"uncertain": False, "detail": "", "action": "deleted"}
    if state["detach_attempted"]:
        return {
            "uncertain": True,
            "detail": "the planned ordinary detach was already attempted and is "
            f"not positively established; backing and records kept at {custody}",
            "action": "kept",
        }
    mount = state["mount"]
    if not (state["attached"] and state["bound"] and mount and effects.ismount(mount)):
        return {
            "uncertain": bool(state["attached"]),
            "detail": "the attachment is not positively bound to this fixture; "
            f"backing and records kept at {custody}",
            "action": "kept",
        }
    try:
        cleanup = effects.run(
            ["hdiutil", "detach", mount], timeout=min(effects.work_left(), 60)
        )
    except Exception as error:  # noqa: BLE001 - an unclassified cleanup stop
        return {
            "uncertain": True,
            "detail": f"the bounded cleanup detach failed: {error}",
            "action": "kept",
        }
    if cleanup.returncode == 0 and not effects.ismount(mount):
        say(
            f"cleanup ordinary detach of {mount}: exit 0; backing and records kept "
            f"at {custody}"
        )
        return {"uncertain": False, "detail": "", "action": "kept"}
    return {
        "uncertain": True,
        "detail": "the bounded cleanup detach is not positively established "
        f"(exit {cleanup.returncode}); backing and records kept at {custody}",
        "action": "kept",
    }


def removable(
    world: World,
    crate: str,
    *,
    effects=None,
    published_scratch=None,
    published_evidence=None,
) -> None:
    """Case 11: one owned removable fixture, from topology to custody.

    The topology is derived from the supervisor's own published members, so
    the custody directory lives in the evidence directory of the ONE monitored
    owned root rather than in this proof's private workspace. The whole flow is
    one seam-injectable function so the pure self-check exercises the real
    custody decisions.
    """
    case(CASE_NAMES[10])
    if sys.platform != "darwin":
        say("not macOS: no removable fixture, the original directory pool stands")
        return
    removable_flow(
        world,
        crate,
        effects if effects is not None else SystemEffects(),
        published_scratch=(
            published_scratch
            if published_scratch is not None
            else os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
        ),
        published_evidence=(
            published_evidence
            if published_evidence is not None
            else os.environ.get("OXIDE_PROOF_EVIDENCE")
        ),
    )


def removable_flow(
    world, crate: str, effects, *, published_scratch: Optional[str],
    published_evidence: Optional[str],
) -> None:
    """The twenty-step removable case, with every effect behind `effects`.

    Order matters and is asserted by the pure self-check: the topology and its
    containment are verified BEFORE the custody directory, the image, the
    intent record or any attach exists; the intent is recorded before the
    create/attach effects; the returned device, mount, UUID and owners-on fact
    are bound before registration; the ONE planned ordinary detach records its
    attempt before the call and is accepted only with its matching post-state;
    and every exit runs the custody decision, where an unknown detach keeps the
    backing and aborts instead of being downgraded to an oracle failure.
    """
    layout = custody_layout(
        published_scratch,
        published_evidence,
        world.root,
        realpath=effects.canonical,
        is_dir=effects.is_dir,
        mode_of=effects.mode_of,
    )
    custody = layout["custody"]
    image = os.path.join(custody, "dragontail.sparseimage")
    intent_path = os.path.join(custody, "intent.json")
    attach_path = os.path.join(custody, "attachment.json")
    attempt_path = os.path.join(custody, "detach-attempt.json")
    require(
        not effects.lexists(image),
        f"an image already exists in this run's custody directory: {image}",
    )
    state = {
        "intent": True,
        "attached": False,
        "bound": False,
        "mount": "",
        "detach_attempted": False,
        "detached_ok": False,
    }
    primary = None
    try:
        # Every topology check passed, so this is the first effect. The intent
        # is durable before the image exists.
        effects.make_private(custody)
        effects.write_private(
            intent_path,
            json.dumps(
                {
                    "image": image,
                    "size_mib": IMAGE_MIB,
                    "volname": "oxide-temp-proof",
                    "owned_root": layout["owned_root"],
                    "evidence": layout["evidence"],
                    "intent": "one owned APFS fixture image, ordinary detach only",
                },
                indent=1,
                sort_keys=True,
            ),
        )
        created = effects.run(
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
            timeout=effects.work_left(),
        )
        require(created.returncode == 0, f"image creation failed: {created.stderr}")
        # The mount-root rule makes the drive root owner-only, so the owned
        # image is attached with ownership enabled, exactly as the accepted
        # account-home proof does, and that fact is confirmed before
        # registration.
        attached = effects.run(
            [
                "/usr/bin/hdiutil",
                "attach",
                "-nobrowse",
                "-owners",
                "on",
                "-plist",
                image,
            ],
            timeout=effects.work_left(),
            text=False,
        )
        require(attached.returncode == 0, f"image attach failed: {attached.stderr!r}")
        state["attached"] = True
        try:
            entities = plistlib.loads(attached.stdout)["system-entities"]
            mounts = [
                entity["mount-point"] for entity in entities if entity.get("mount-point")
            ]
            devices = [
                entity["dev-entry"] for entity in entities if entity.get("dev-entry")
            ]
        except (ValueError, KeyError, TypeError) as error:
            abort(f"the attach output is unclassifiable: {error}")
        require(
            len(mounts) == 1,
            f"expected one mounted volume for the owned image, got {mounts}",
        )
        mount = mounts[0]
        state["mount"] = mount
        require(
            mount.startswith("/Volumes/"),
            f"the owned image did not mount under /Volumes: {mount}",
        )
        ownership = effects.run(
            ["/usr/sbin/diskutil", "info", "-plist", mount],
            timeout=effects.work_left(),
            text=False,
        )
        require(ownership.returncode == 0, "diskutil info failed for the mount")
        try:
            info = plistlib.loads(ownership.stdout)
        except ValueError as error:
            abort(f"the diskutil output is unclassifiable: {error}")
        require(
            info.get("GlobalPermissionsEnabled") is True,
            "the owned image is mounted without ownership enabled",
        )
        require(
            bool(devices) and devices[0],
            "the attach output bound no device entry for the owned image",
        )
        require(
            bool(info.get("VolumeUUID")),
            "the mounted volume has no volume UUID to bind the attachment to",
        )
        # The mount's own device node must be one of the device entries the
        # attach actually returned: a missing or swapped binding is refused
        # before the attachment record claims one.
        node = info.get("DeviceNode")
        require(
            bool(node) and node in devices,
            "the mounted volume's device node "
            f"{node!r} is not one of the attached device entries {devices}",
        )
        # Bind the returned identity before any further effect, so a detach can
        # be checked against the attachment this run actually made.
        effects.write_private(
            attach_path,
            json.dumps(
                {
                    "image": image,
                    "mount": mount,
                    "device": node,
                    "volume_uuid": info.get("VolumeUUID", ""),
                    "owners_on": True,
                },
                indent=1,
                sort_keys=True,
            ),
        )
        state["bound"] = True
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
        # The inherited fixture scratch is on another device, so the removable
        # build's children must get the owned directory on the image instead.
        removable_env = world.env()
        result = world.run(["check"], cwd=crate, env=removable_env)
        observed = world.observe(result.stderr, "dir", removable_env)
        expect(
            effects.canonical(observed).startswith(
                effects.canonical(os.path.join(mount, "oxide-pool")) + os.sep
            ),
            f"the removable build's temporary files are not on the image: {observed}",
        )
        expect(
            effects.canonical(observed).startswith(effects.canonical(mount) + os.sep),
            f"the inherited other-device directory was not replaced: {observed}",
        )
        expect(
            "temp: oxide" in result.stderr,
            "the removable build did not report the owned scratch",
        )
        expect(
            not effects.lexists(observed.rstrip("/")),
            "the removable build's owned directory survived release",
        )
        # The attempt is recorded BEFORE the ONE planned ordinary detach, so a
        # second attempt can never be made blindly against a possibly reused
        # mount point.
        effects.write_private(
            attempt_path,
            json.dumps(
                {"mount": mount, "device": node, "planned": "one ordinary detach"},
                indent=1,
                sort_keys=True,
            ),
        )
        state["detach_attempted"] = True
        detached = effects.run(
            ["hdiutil", "detach", mount], timeout=min(effects.work_left(), 60)
        )
        require(
            detached.returncode == 0, f"ordinary detach failed: {detached.stderr}"
        )
        require(
            not effects.ismount(mount), f"{mount} is still mounted after detach"
        )
        require(not effects.exists(mount), f"the mount point {mount} was recreated")
        state["detached_ok"] = True
        # The registration still exists and its volume is gone, so this build
        # must go through the product's own fallback selection on the original
        # pool; it is not pointed at a fresh pool.
        fallback_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
        fallback = world.run(["check"], cwd=crate, env=fallback_env)
        observed = world.observe(fallback.stderr, "dir", fallback_env).rstrip("/")
        expect(
            effects.canonical(observed).startswith(
                effects.canonical(os.path.join(world.pool, "payload")) + os.sep
            ),
            f"the fallback did not use the original directory pool's scratch: {observed}",
        )
        say(
            "post-detach selection: "
            + " | ".join(
                line
                for line in fallback.stderr.splitlines()
                if "select" in line or "removable" in line
            )[:400]
        )
        expect(not effects.exists(mount), f"the fallback recreated {mount}")
        # With a same-device inherited TMPDIR the fallback keeps it and
        # creates nothing.
        inherited = effects.make_private(world.fixture("keep-tmp"))
        kept_env = world.env(TMPDIR=inherited, TMP=None, TEMP=None)
        kept = world.run(["check"], cwd=crate, env=kept_env)
        expect(
            world.observe(kept.stderr, "dir", kept_env).rstrip("/")
            == effects.canonical(inherited),
            "the fallback did not keep the same-device inherited directory: "
            f"{kept.stderr[-1500:]}",
        )
        for root in effects.slot_tree(os.path.join(world.pool, "payload", "slots")):
            expect(
                not effects.lexists(os.path.join(root, "scratch")),
                "the fallback created an owned directory it did not need",
            )
    except BaseException as error:  # noqa: BLE001 - custody is decided on every exit
        # The case's own outcome is kept and re-raised after the custody
        # decision; nothing here turns an unknown into an oracle failure.
        primary = error
    cleanup = custody_cleanup(effects, state, image, custody)
    if cleanup["uncertain"]:
        beyond = (
            f"; the earlier {'unknown outcome' if isinstance(primary, UnknownOutcome) else 'oracle failure'} "
            f"stays the primary fault: {primary}"
            if primary is not None
            else ""
        )
        abort(f"image custody is unknown: {cleanup['detail']}{beyond}")
    if primary is not None:
        raise primary
    if cleanup["action"] == "kept":
        say(f"the owned image backing is kept at {image} with its records")


def path_bound(world: World, crate: str) -> None:
    case(CASE_NAMES[11])
    env = world.env(TMPDIR=None, TMP=None, TEMP=None)
    result = world.run(["check"], cwd=crate, env=env)
    observed = world.observe(result.stderr, "dir", env).rstrip("/")
    held_by_pool(world, observed)
    leaf = os.path.basename(observed)
    raw = observed.encode()
    accepted = [
        path
        for path in world.accepted
        if path.endswith("scratch") and is_inside(path, world.pool)
    ]
    longest = max(accepted, key=lambda path: len(path.encode())) if accepted else observed
    say(
        f"longest accepted injected path: {len(longest.encode())} bytes, "
        f"{len(longest)} characters, {longest!r}"
    )
    say(
        f"this observation: {raw!r} = {len(raw)} bytes, "
        f"{len(observed)} characters, leaf {leaf!r}"
    )
    expect(
        len(raw) <= SOCKET_PATH_BOUND,
        f"the injected path is {len(raw)} bytes, over the {SOCKET_PATH_BOUND}-byte bound",
    )
    expect(
        len(longest.encode()) <= SOCKET_PATH_BOUND,
        f"an accepted path is {len(longest.encode())} bytes, over the "
        f"{SOCKET_PATH_BOUND}-byte bound",
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
        "/scratch/fixture",
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
        "GIT_CONFIG_GLOBAL": "/host/git",
        "CARGO_PROFILE_DEV_DEBUG": "2",
        "TMPDIR": "/host/tmp",
    }
    scrubbed_fixture = fixture_environment(
        hostile, "/home/tom/.rustup", "/scratch/fixture/home", "/scratch/fixture/cargo-home"
    )
    scrubbed_staging = staging_environment(hostile, "/home/tom/.rustup", "/scratch/fixture")
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
        "staging keeps only explicit Cargo controls and owned temp",
        "GIT_CONFIG_GLOBAL" not in scrubbed_staging
        and "CARGO_PROFILE_DEV_DEBUG" not in scrubbed_staging
        and all(scrubbed_staging[key] == "/scratch/fixture" for key in ("TMPDIR", "TMP", "TEMP")),
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
        '[[package]]\nname = "t"\nversion = "0.1.0"\n'
    )
    ok, _reason = lock_precondition(MANIFEST, valid_lock)
    property_true("manifest-consistent lock is a valid precondition", ok)
    for label, text in (
        ("missing lock", ""),
        ("no lock version header", '[[package]]\nname = "t"\nversion = "0.1.0"\n'),
        ("mismatched package name", valid_lock.replace('"t"', '"other"')),
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
            with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(
                io.StringIO()
            ):
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
    property_true(
        "a complete diagnostic record declares complete collection",
        json.loads(record)["collection"] == "complete",
    )
    # Forced overflow: a genuinely oversized outcome/fault/skip set must yield
    # valid JSON within the budget that declares itself incomplete with
    # explicit omitted counts, and must keep the first fault and abort flag.
    many = [
        (ProofCase(i, body(str(i)), label=f"case-{i}"), "fail", "d" * 4000)
        for i in range(300)
    ]
    many_faults = [(f"case-{i}", "e" * 4000) for i in range(300)]
    many_skips = [(f"case-{i}", "s" * 2000) for i in range(300)]
    bounded = diagnostic_record(many, many_faults, many_skips, True)
    parsed = json.loads(bounded)
    property_true(
        "forced overflow yields valid JSON within the budget",
        isinstance(parsed, dict) and len(bounded.encode()) <= MAX_DIAGNOSTIC_RECEIPT_BYTES,
    )
    property_true(
        "forced overflow declares incomplete collection with omitted counts",
        parsed["collection"] == "incomplete"
        and parsed["cases_omitted"] > 0
        and bool(parsed["incomplete_reasons"]),
    )
    property_true(
        "forced overflow keeps truthful counts",
        parsed["cases_total"] == 300
        and parsed["cases_included"] + parsed["cases_omitted"] == 300
        and parsed["faults_recorded"] + parsed["faults_omitted"] == 300
        and parsed["skips_recorded"] + parsed["skips_omitted"] == 300,
    )
    property_true(
        "forced overflow keeps the first fault and the abort",
        parsed["first_fault"] is not None
        and parsed["first_fault"]["case"] == "case-0"
        and parsed["aborted_on_unknown_outcome"] is True
        and parsed["qualified"] is False,
    )
    property_true(
        "forced overflow carries truncation metadata",
        any(
            entry.get("detail_truncated") is True
            for entry in [parsed["first_fault"]] + parsed["secondary_faults"]
            if entry
        ),
    )

    # 3f. The guard deadline is an infrastructure abort, exercised through the
    #     real driver and the real work_left with fake clock/environment seams.
    def deadline_body(tag):
        def run():
            ran.append(tag)
            work_left(now_ns=2_000_000_000, deadline_ns=1_000_000_000, margin_s=30)
        return run

    for diagnostic in (False, True):
        deadline_cases = [
            ProofCase(0, body("a"), label="a"),
            ProofCase(1, deadline_body("b"), label="b"),
            ProofCase(2, body("c"), label="c"),
        ]
        status, ran_cases = scenarios(diagnostic, deadline_cases)
        property_true(
            "deadline exhaustion aborts before another case in "
            + ("diagnostic" if diagnostic else "default")
            + " mode",
            status == 1 and ran_cases == ["a", "b"],
        )
        # Positive control on the same seams: with time to spare the real
        # work_left returns a positive budget instead of aborting.
        property_true(
            "the same seams return a positive budget when time remains",
            work_left(now_ns=0, deadline_ns=100_000_000_000, margin_s=30) > 5,
        )

    # 3g. Negative control: if the deadline used the oracle path instead of the
    #     abort path, diagnostic mode would continue into the next independent
    #     case. The property above therefore is not vacuous.
    def recoverable_deadline_body(tag):
        def run():
            ran.append(tag)
            fail("work deadline reached (recoverable variant)")
        return run

    negative_cases = [
        ProofCase(0, body("a"), label="a"),
        ProofCase(1, recoverable_deadline_body("b"), label="b"),
        ProofCase(2, body("c"), label="c"),
    ]
    status, ran_cases = scenarios(True, negative_cases)
    property_true(
        "negative control: a recoverable deadline failure would continue",
        status == 1 and ran_cases == ["a", "b", "c"],
    )

    # 3h. First-fault ordering: an oracle failure before the deadline abort
    #     stays the first fault, and the abort remains visible in the record.
    ordering_cases = [
        ProofCase(0, fail_body("a", "earlier oracle mismatch"), label="a"),
        ProofCase(1, deadline_body("b"), label="b"),
    ]
    captured = {}

    def capture(path, outcomes, faults, skips, aborted):
        captured["text"] = diagnostic_record(outcomes, faults, skips, aborted)
        return path

    saved_writer = globals()["write_diagnostic_receipt"]
    globals()["write_diagnostic_receipt"] = capture
    saved_mode = MODE["diagnostic"]
    MODE["diagnostic"] = True
    try:
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(
            io.StringIO()
        ):
            ordering_status = drive(ordering_cases, "/nonexistent")
    finally:
        MODE["diagnostic"] = saved_mode
        globals()["write_diagnostic_receipt"] = saved_writer
    ordering = json.loads(captured["text"])
    property_true(
        "first fault precedes a later deadline abort",
        ordering_status == 1
        and ordering["first_fault"]["case"] == "a"
        and "earlier oracle mismatch" in ordering["first_fault"]["detail"]
        and ordering["aborted_on_unknown_outcome"] is True
        and any(
            entry["case"] == "b" for entry in ordering["secondary_faults"]
        ),
    )

    # 3h. Fixture placement: inherited-TMPDIR fixtures must be siblings of the
    #     directory-budget pool (same device, outside the pool directory), never
    #     children of it. The negative control is the exact defect the last
    #     diagnostic receipt exposed.
    property_true(
        "the fixture directory is a sibling of the pool directory",
        fixture_dir("/x/pt") == "/x/pt/fx"
        and not is_inside(fixture_dir("/x/pt"), "/x/pt/p"),
    )
    property_true(
        "the fixture directory stays inside the monitored root",
        is_inside(fixture_dir("/x/pt"), "/x/pt"),
    )
    property_true(
        "negative control: a fixture planted in the pool is detected",
        is_inside("/x/pt/p/inherited-tmp", "/x/pt/p")
        and is_inside("/x/pt/p/not-a-dir", "/x/pt/p"),
    )
    property_true(
        "containment is component-aware, not a string prefix",
        not is_inside("/x/pt/fx", "/x/pt/f")
        and not is_inside("/x/pt/p2", "/x/pt/p"),
    )

    # 3i. Source fit for the two roots this proof actually uses, with the
    #     product's real folder width (`t-<8 hex>`, ten characters). The
    #     selected-volume root is the ordinary #145 default; the internal root
    #     is this proof's declared `--root-parent /private/tmp` recipe. No
    #     claim is made for arbitrary mounts: a mount two characters longer
    #     than the observed selected-volume one would exceed the bound, which
    #     is why both figures are asserted exactly.
    selected_mount = len("/Volumes/Builds")
    internal_mount = len("/private/tmp")
    property_true(
        "the selected-volume layout measures 90 bytes",
        longest_injected_path(selected_mount, PRODUCT_FOLDER_CHARS) == 90,
    )
    property_true(
        "the internal-root layout measures 87 bytes",
        longest_injected_path(internal_mount, PRODUCT_FOLDER_CHARS) == 87,
    )
    property_true(
        "both layouts fit the 91-byte bound",
        longest_injected_path(selected_mount, PRODUCT_FOLDER_CHARS) <= 91
        and longest_injected_path(internal_mount, PRODUCT_FOLDER_CHARS) <= 91,
    )
    property_true(
        "a mount two characters longer would exceed the bound",
        longest_injected_path(selected_mount + 2, PRODUCT_FOLDER_CHARS) > 91,
    )
    property_true(
        "the earlier long labels would not have fitted",
        longest_injected_path(
            selected_mount, len("tempcheck") + 1 + 8
        )
        + (len("/pool-temp") - len("/pt") + len("/pool") - len("/p"))
        > 91,
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

    property_true(
        "scratch creation notice cannot pass as ordinary creation",
        creation_notice_matches(SCRATCH_CREATION_NOTICE, True)
        and not creation_notice_matches(ORDINARY_CREATION_NOTICE, True),
    )
    property_true(
        "ordinary creation notice cannot pass as scratch creation",
        creation_notice_matches(ORDINARY_CREATION_NOTICE, False)
        and not creation_notice_matches(SCRATCH_CREATION_NOTICE, False),
    )

    # 6. The real constructor, through narrow directory/device/exists seams:
    #    it creates the account homes, the sibling fixture directory and the
    #    fixture scratch, and NEVER the pool directory (the diagnostic3
    #    defect). The old layout is the negative control.
    created: list[str] = []

    def fake_make(path: str) -> str:
        created.append(path)
        return path

    probe = World(
        "/x/pt",
        "/x/subject",
        "/x/rustup",
        make_dir=fake_make,
        device_of=lambda path: 7,
        exists=lambda path: False,
    )
    property_true(
        "the constructor creates the account homes and fixture scratch",
        {"/x/pt/home", "/x/pt/cargo-home", "/x/pt/fx", "/x/pt/fixture-scratch"}
        <= set(created),
    )
    property_true(
        "the constructor never pre-creates the pool directory",
        "/x/pt/p" not in created and not layout_pre_creates_pool("/x/pt"),
    )
    property_true(
        "negative control: the pre-fix layout pre-created the pool",
        layout_pre_creates_pool("/x/pt", constructor_paths("/x/pt") + ["/x/pt/p"]),
    )
    property_true(
        "negative control: a fixture inside the pool is detected",
        layout_pre_creates_pool("/x/pt", constructor_paths("/x/pt") + ["/x/pt/p/fx"]),
    )
    property_true("the pool path is the product's own directory", probe.pool == "/x/pt/p")
    property_true("scratch declaration and cap bind the pool",
                  probe.base.get("OXIDE_SCRATCH_ROOT") == "/x/pt"
                  and is_inside(probe.pool, probe.base["OXIDE_SCRATCH_ROOT"])
                  and probe.base.get("OXIDE_POOL_CAP") == "64MiB")
    property_true(
        "the fixture directory is never inside the pool",
        not is_inside("/x/pt/fx", probe.pool),
    )
    property_true(
        "each launch mints a fresh observation identifier",
        probe.env()["OXIDE_PROOF_NONCE"] != probe.env()["OXIDE_PROOF_NONCE"]
        and probe.env()["OXIDE_PROOF_NONCE"].startswith(probe.nonce_seed),
    )
    property_true(
        "the fixture environment sets the public two-slot control explicitly",
        probe.base.get("OXIDE_MAX_ACTIVE") == "2",
    )

    # 7. Fresh observations: every variable the build script reads is declared
    #    to Cargo, the identifier is read and declared, and a stale or
    #    identifier-less stream is refused before any path is extracted.
    reads = script_env_reads(BUILD_SCRIPT)
    declared = rerun_env_names(BUILD_SCRIPT)
    property_true(
        "every fixture variable the build script reads is declared to Cargo",
        reads <= declared,
    )
    property_true(
        "the alias names and the observation identifier are declared",
        {"TMPDIR", "TMP", "TEMP", "OXIDE_PROOF_NONCE"} <= declared,
    )
    property_true(
        "negative control: an undeclared variable is detected",
        "FIXTURE_HOLD"
        not in rerun_env_names(BUILD_SCRIPT.replace('"FIXTURE_HOLD",', "")),
    )
    current_stream = (
        "cargo:warning=OXIDE-TEMP nonce=n1\n"
        "cargo:warning=OXIDE-TEMP dir=/x/pt/p/payload/slots/t-12345678/quick-1/scratch\n"
    )
    property_true(
        "a current observation is accepted",
        observed_value(current_stream, "dir", "n1").endswith("/scratch"),
    )
    for label, text, nonce in (
        ("a replayed observation", current_stream, "n2"),
        ("an observation without an identifier", "cargo:warning=OXIDE-TEMP dir=/x\n", "n1"),
    ):
        try:
            observed_value(text, "dir", nonce)
        except Recoverable:
            checks.append(f"refuses {label}")
        else:
            fail(f"self-check property failed: {label} must be refused")

    # 8. The gated writer's readiness record: published before it waits, and
    #    strictly parsed (stale, truncated, missing or relative records fail).
    property_true(
        "the writer publishes readiness before it waits", report_before_wait(BUILD_SCRIPT)
    )
    property_true(
        "negative control: waiting before publishing is detected",
        not report_before_wait(
            "while !std::path::Path::new(&gate).exists() {}\nFIXTURE_REPORT\n"
        ),
    )
    ready = "nonce=n1\ndir=/x/pt/p/payload/slots/t-12345678/quick-1/scratch\npgid=4242\n"
    parsed = parse_readiness(ready, "n1")
    property_true(
        "a current readiness record is accepted",
        parsed["pgid"] == 4242 and parsed["dir"].endswith("scratch"),
    )
    for label, text, nonce in (
        ("a stale readiness record", ready, "n2"),
        ("a truncated record", "nonce=n1\ndir=/x/pt/scr\npgid=", "n1"),
        ("a record with a missing field", "nonce=n1\ndir=/x/pt/fx\n", "n1"),
        ("a relative readiness directory", "nonce=n1\ndir=rel\npgid=1\n", "n1"),
    ):
        try:
            parse_readiness(text, nonce)
        except Recoverable:
            checks.append(f"refuses {label}")
        else:
            fail(f"self-check property failed: {label} must be refused")

    # 9. The refusal position is derived from the last fresh observation, never
    #    from enumeration order, and the refusal must name that exact path.
    observed_slot = (
        "/x/pt/p/payload/slots/t-12345678/quick-2/scratch"
    )
    property_true(
        "the refusal target comes from the fresh observation",
        refusal_target(observed_slot, ["/other/slot", "/elsewhere"])
        == observed_slot
        and refusal_target(observed_slot) == observed_slot,
    )
    property_true(
        "the refusal must name exactly the planted position",
        refusal_names(
            f"oxide: {observed_slot} is not a real directory, so it is kept",
            observed_slot,
        )
        and not refusal_names(
            "oxide: /x/pt/p/payload/slots/t-12345678/quick-1/scratch is not a real "
            "directory",
            observed_slot,
        ),
    )

    # 10. Accounting: a unique allocation is accepted within rendered
    #     truncation, a by-name double count is rejected.
    unique = 16 * MIB
    ok_single, _ = delta_verdict(0, unique, unique)
    ok_truncated, _ = delta_verdict(0, unique - MIB // 2, unique)
    ok_vacuous, _ = delta_verdict(0, 0, unique)
    property_true(
        "one unique allocation is accepted within rendered truncation",
        ok_single and ok_truncated,
    )
    property_true("a zero delta is rejected", not ok_vacuous)
    # The exact defect this window exists for: the case plants 8 MiB + 8 MiB
    # with one hard link, so a walk that counts the link by name reports about
    # 24 MiB. That figure must be rejected, not just an absurd 32 MiB.
    for label, planted in (
        ("the planted by-name 24 MiB count", 3 * 8 * MIB),
        ("twice the unique allocation", 2 * unique),
    ):
        ok, delta = delta_verdict(0, planted, unique)
        property_true(
            f"{label} is rejected as a double count",
            not ok and delta == planted,
        )
    property_true(
        "a delta far above one allocation is rejected",
        not delta_verdict(0, unique + 3 * MIB, unique)[0],
    )

    # A launch cloned from an existing dictionary must mint its own identifier;
    # several launches sharing one dictionary would otherwise share it.
    launch_base = probe.env(TMPDIR="/x/pt/fx/alias-tmp", TMP=None, TEMP=None)
    first_launch = probe.launch(launch_base, FIXTURE_HOLD="/x/pt/release")
    second_launch = probe.launch(launch_base, FIXTURE_HOLD="/x/pt/release")
    property_true(
        "a cloned launch mints its own observation identifier",
        first_launch["OXIDE_PROOF_NONCE"]
        != second_launch["OXIDE_PROOF_NONCE"]
        != launch_base["OXIDE_PROOF_NONCE"],
    )
    property_true(
        "a cloned launch keeps the base's other keys",
        first_launch["TMPDIR"] == launch_base["TMPDIR"]
        and first_launch["OXIDE_MAX_ACTIVE"] == launch_base["OXIDE_MAX_ACTIVE"]
        and second_launch["FIXTURE_HOLD"] == "/x/pt/release",
    )
    reused_one = dict(launch_base, FIXTURE_HOLD="/x/pt/release")
    reused_two = dict(launch_base, FIXTURE_HOLD="/x/pt/release")
    property_true(
        "negative control: reusing one dictionary reuses its identifier",
        reused_one["OXIDE_PROOF_NONCE"] == reused_two["OXIDE_PROOF_NONCE"],
    )
    # The measured status must use the held child's aliases: a query that keeps
    # its own writable same-device TMPDIR would legitimately describe an
    # inherited directory and prove nothing about the held owned directory.
    property_true(
        "a status query with the held child's aliases matches",
        same_alias_selection(
            {"TMPDIR": None, "TMP": None, "TEMP": None},
            {"TMPDIR": None, "TMP": None, "TEMP": None},
        ),
    )
    property_true(
        "negative control: a wrong status environment is detected",
        not same_alias_selection(
            {"TMPDIR": None, "TMP": None, "TEMP": None},
            {"TMPDIR": "/x/pt/fixture-scratch", "TMP": None, "TEMP": None},
        ),
    )

    # 11. Image custody: the backing is deleted only after a positive ordinary
    #     detach, and every ambiguous outcome keeps it.
    property_true(
        "a positive ordinary detach allows deleting the backing",
        image_custody_decision(False, True, "/Volumes/oxide-temp-proof") == "delete",
    )
    for label, args in (
        ("a failed detach", (False, False, "/Volumes/oxide-temp-proof")),
        ("a still-mounted volume", (True, True, "/Volumes/oxide-temp-proof")),
        ("an unclassifiable attachment", (False, True, "")),
    ):
        property_true(
            f"{label} keeps the backing image", image_custody_decision(*args) == "keep"
        )

    # 12. Dependency propagation and abort completeness: a skipped predecessor
    #     is not a passed one, case 2's failure skips 3-12, and an abort still
    #     records every planned case as not run.
    chain = [
        ProofCase(0, fail_body("a"), label="a"),
        ProofCase(1, body("b"), depends=(0,), label="b"),
        ProofCase(2, body("c"), depends=(1,), label="c"),
    ]
    status, ran_cases = scenarios(True, chain)
    property_true(
        "a skipped predecessor is not a passed predecessor",
        status == 1 and ran_cases == ["a"],
    )
    pool_cases = [
        ProofCase(0, body("a"), label="a"),
        ProofCase(1, fail_body("b"), depends=(0,), mutates=True, label="b"),
    ] + [ProofCase(i, body(str(i)), depends=(1,), label=f"p{i}") for i in range(2, 12)]
    status, ran_cases = scenarios(True, pool_cases)
    property_true(
        "the pool-creating case's failure skips cases 3-12",
        status == 1 and ran_cases == ["a", "b"],
    )
    abort_cases = [
        ProofCase(0, body("a"), label="a"),
        ProofCase(1, unknown_body("b"), depends=(0,), label="b"),
    ] + [ProofCase(i, body(str(i)), depends=(1,), label=f"q{i}") for i in range(2, 12)]
    captured = {}

    def capture_abort(path, outcomes, faults, skips, aborted):
        captured["text"] = diagnostic_record(outcomes, faults, skips, aborted)
        return path

    saved_writer = globals()["write_diagnostic_receipt"]
    globals()["write_diagnostic_receipt"] = capture_abort
    saved_mode = MODE["diagnostic"]
    MODE["diagnostic"] = True
    try:
        ran.clear()
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(
            io.StringIO()
        ):
            abort_status = drive(abort_cases, "/nonexistent")
    finally:
        MODE["diagnostic"] = saved_mode
        globals()["write_diagnostic_receipt"] = saved_writer
    abort_record = json.loads(captured["text"])
    property_true(
        "an abort still records every planned case as not run",
        abort_status == 1
        and abort_record["cases_total"] == 12
        and sum(1 for entry in abort_record["cases"] if entry["outcome"] == "not_run") == 10
        and abort_record["aborted_on_unknown_outcome"] is True,
    )

    # 13. Case 11's monitored-root topology and image custody, exercised
    #     through the real custody_layout, custody_cleanup and removable_flow
    #     with fake canonical/filesystem/process/product seams. No image, child
    #     or device is created here.
    norm = os.path.normpath
    owned_scratch = "/owned/root/scratch"
    owned_evidence = "/owned/root/evidence"
    custody_dir = "/owned/root/evidence/removable-image"
    layout = custody_layout(
        owned_scratch,
        owned_evidence,
        "/owned/root/scratch/pt",
        realpath=norm,
        is_dir=lambda path: True,
        mode_of=lambda path: 0o700,
    )
    property_true(
        "the published scratch and evidence resolve to one owned root",
        layout["owned_root"] == "/owned/root" and layout["custody"] == custody_dir,
    )
    property_true(
        "negative control: the old workspace/parent pair rejects that custody",
        not any(
            is_inside(norm(custody_dir), root)
            for root in (norm("/owned/root/scratch/pt"), norm("/owned/root/scratch"))
        ),
    )

    class Proc:
        def __init__(self, returncode: int = 0, stdout="", stderr=""):
            self.returncode = returncode
            self.stdout = stdout
            self.stderr = stderr

    class FakeEffects:
        """The removable flow's effects, in memory, recording every call."""

        def __init__(self, **options):
            self.calls = []
            self.dirs = set()
            self.files = {}
            self.mounted = set()
            self.mount = options.get("mount", "/Volumes/oxide-temp-proof")
            self.attach_exit = options.get("attach_exit", 0)
            self.attach_plist = options.get("attach_plist")
            self.owners = options.get("owners", True)
            self.volume_uuid = options.get("volume_uuid", "UUID-1")
            self.detach_exit = options.get("detach_exit", 0)
            self.detach_raise = options.get("detach_raise")
            self.cleanup_exit = options.get("cleanup_exit", 0)
            self.cleanup_raise = options.get("cleanup_raise")
            self.mode = options.get("mode", 0o700)
            self.dirs_ok = options.get("dirs_ok", True)
            self.canonical_map = options.get("canonical_map", {})
            self.entity_devices = options.get(
                "entity_devices", ["/dev/disk9s1", "/dev/disk9"]
            )
            self.device_node = options.get("device_node")
            self.detach_keeps_mounted = options.get("detach_keeps_mounted", False)
            self.recreate_mount = options.get("recreate_mount", False)
            self.cleanup_keeps_mounted = options.get("cleanup_keeps_mounted", False)
            self.work_left_fn = options.get("work_left_fn")
            self.work_left_calls = 0
            self.detach_calls = 0

        def canonical(self, path):
            return self.canonical_map.get(norm(path), norm(path))

        def is_dir(self, path):
            return self.dirs_ok

        def mode_of(self, path):
            return self.mode

        def ismount(self, path):
            return path in self.mounted

        def exists(self, path):
            return path in self.files or path in self.dirs

        def lexists(self, path):
            return self.exists(path)

        def unlink(self, path):
            self.calls.append(("unlink", path))
            self.files.pop(path, None)

        def make_private(self, path):
            self.calls.append(("mkdir", path))
            self.dirs.add(path)
            return path

        def write_private(self, path, data):
            self.calls.append(("write", path))
            self.files[path] = data

        def slot_tree(self, root):
            return []

        def work_left(self):
            self.work_left_calls += 1
            if self.work_left_fn is not None:
                return self.work_left_fn(self.work_left_calls)
            return 60.0

        def run(self, argv, *, timeout, text=True):
            self.calls.append(("run", tuple(argv)))
            if "create" in argv:
                self.files[argv[-1]] = "backing"
                return Proc()
            if "attach" in argv:
                if self.attach_plist is not None:
                    return Proc(self.attach_exit, self.attach_plist)
                entities = [{"mount-point": self.mount}]
                if self.entity_devices and self.entity_devices[0]:
                    entities[0]["dev-entry"] = self.entity_devices[0]
                if len(self.entity_devices) > 1 and self.entity_devices[1]:
                    entities.append({"dev-entry": self.entity_devices[1]})
                payload = plistlib.dumps({"system-entities": entities})
                if self.attach_exit == 0:
                    self.mounted.add(self.mount)
                return Proc(self.attach_exit, payload)
            if "info" in argv:
                return Proc(
                    0,
                    plistlib.dumps(
                        {
                            "GlobalPermissionsEnabled": self.owners,
                            "VolumeUUID": self.volume_uuid,
                            "DeviceNode": (
                                self.device_node
                                if self.device_node is not None
                                else self.entity_devices[0]
                            ),
                        }
                    ),
                )
            if "detach" in argv:
                self.detach_calls += 1
                # The planned detach is the one whose attempt record the flow
                # wrote first; anything else is the bounded cleanup detach.
                planned = any(
                    path.endswith("detach-attempt.json") for path in self.files
                )
                failure = self.detach_raise if planned else self.cleanup_raise
                exit_code = self.detach_exit if planned else self.cleanup_exit
                if failure is not None:
                    raise failure
                if exit_code == 0:
                    keeps = (
                        self.detach_keeps_mounted
                        if planned
                        else self.cleanup_keeps_mounted
                    )
                    if not keeps:
                        self.mounted.discard(argv[-1])
                    if planned and self.recreate_mount:
                        self.dirs.add(argv[-1])
                return Proc(exit_code, "", "" if exit_code == 0 else "detach refused")
            return Proc(1, "", f"unexpected command {argv}")

    class FakeWorld:
        """The product side of the removable case, with fake observations.

        `observe` is the real `World.observe`, so the live identifier check and
        the accepted-path bookkeeping are exercised rather than re-stated.
        """

        observe = World.observe

        def __init__(
            self, mount="/Volumes/oxide-temp-proof", observed=None,
            root="/owned/root/scratch/pt", stale_nonce=False,
        ):
            self.root = root
            self.pool = os.path.join(root, "p")
            self.mount = mount
            self.runs = []
            self.accepted = []
            self.stale_nonce = stale_nonce
            self.pending = list(
                observed
                if observed is not None
                else [
                    f"{mount}/oxide-pool/payload/slots/t-1/quick-1/scratch",
                    "/owned/root/scratch/pt/p/payload/slots/t-1/quick-1/scratch",
                    "/owned/root/scratch/pt/fx/keep-tmp",
                ]
            )
            self.stderr = "temp: oxide /owned/scratch\n"

        def fixture(self, name):
            return os.path.join(self.root, "fx", name)

        def env(self, **overrides):
            env = {"OXIDE_PROOF_NONCE": "n"}
            for key, value in overrides.items():
                if value is None:
                    env.pop(key, None)
                else:
                    env[key] = value
            return env

        def run(self, args, *, cwd, env, check=True):
            self.runs.append(tuple(args))
            nonce = "n-stale" if self.stale_nonce else env.get("OXIDE_PROOF_NONCE")
            # Only a check consumes the next observation; the registration call
            # before it carries no observation of its own.
            target = self.pending[0] if self.pending else ""
            if self.pending and args and args[0] == "check":
                target = self.pending.pop(0)
            return Proc(
                0,
                "",
                f"cargo:warning=OXIDE-TEMP nonce={nonce}\n"
                f"cargo:warning=OXIDE-TEMP dir={target}\n"
                f"temp: oxide {target}\n",
            )

    def run_flow(**options):
        effects = FakeEffects(**options)
        world = FakeWorld(
            effects.mount,
            options.get("observed"),
            options.get("world_root", "/owned/root/scratch/pt"),
            options.get("stale_nonce", False),
        )
        caught = None
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
                io.StringIO()
            ):
                removable_flow(
                    world,
                    "/owned/root/scratch/pt/crate",
                    effects,
                    published_scratch=options.get("published_scratch", owned_scratch),
                    published_evidence=options.get("published_evidence", owned_evidence),
                )
        except BaseException as error:  # noqa: BLE001 - the oracle asserts on it
            caught = error
        return caught, effects, world

    image_path = os.path.join(custody_dir, "dragontail.sparseimage")
    caught, effects, _world = run_flow()
    property_true(
        "the owned image success flow completes with one planned ordinary detach",
        caught is None and effects.detach_calls == 1 and not effects.mounted,
    )
    property_true(
        "a positive ordinary detach alone deletes the owned backing",
        image_path not in effects.files
        and ("unlink", image_path) in effects.calls,
    )
    property_true(
        "the custody records are written inside the published evidence directory",
        all(
            os.path.dirname(path) == custody_dir
            for path in effects.files
        )
        and os.path.join(custody_dir, "attachment.json") in effects.files
        and os.path.join(custody_dir, "intent.json") in effects.files
        and os.path.join(custody_dir, "detach-attempt.json") in effects.files,
    )
    property_true(
        "no effect precedes the verified topology",
        effects.calls[0][0] == "mkdir" and effects.calls[0][1] == custody_dir,
    )

    for label, options, fragment in (
        ("no published scratch", {"published_scratch": ""}, "published no"),
        ("no published evidence", {"published_evidence": None}, "published no"),
        (
            "evidence on another owned root",
            {"published_evidence": "/elsewhere/evidence"},
            "not a sibling",
        ),
        (
            "members that are not the guard's names",
            {"published_scratch": "/owned/root/tmp-a", "published_evidence": "/owned/root/tmp-b"},
            "not the guard's",
        ),
        (
            "a broad shared parent",
            {"published_scratch": "/private/tmp/scratch", "published_evidence": "/private/tmp/evidence"},
            "shared parent",
        ),
        (
            "a workspace outside the published scratch",
            {"world_root": "/owned/root/scratch2/pt"},
            "inside the published scratch",
        ),
        (
            "a missing published member",
            {"dirs_ok": False},
            "must both exist",
        ),
        (
            "a symlink escaping the owned root",
            {"canonical_map": {owned_evidence: "/elsewhere/evidence"}},
            "not a sibling",
        ),
        (
            "a group-writable owned root",
            {"mode": 0o770},
            "group- or world-writable",
        ),
    ):
        caught, effects, _world = run_flow(**options)
        property_true(
            f"refuses {label} before any effect",
            isinstance(caught, UnknownOutcome)
            and fragment in str(caught)
            and effects.calls == [],
        )

    caught, effects, _world = run_flow(
        observed=[
            "/owned/root/scratch/pt/fixture-scratch/inherited-kept",
            "/owned/root/scratch/pt/p/payload/slots/t-1/quick-1/scratch",
            "/owned/root/scratch/pt/fx/keep-tmp",
        ]
    )
    property_true(
        "a build that kept the inherited directory fails the image oracle",
        isinstance(caught, Recoverable) and "not on the image" in str(caught),
    )
    property_true(
        "a failed case after attach still runs one bounded cleanup detach and keeps the backing",
        effects.detach_calls == 1
        and image_path in effects.files
        and ("unlink", image_path) not in effects.calls,
    )
    property_true(
        "the failed case's cleanup keeps intent and attachment records",
        os.path.join(custody_dir, "attachment.json") in effects.files
        and os.path.join(custody_dir, "detach-attempt.json") not in effects.files,
    )

    timeout = subprocess.TimeoutExpired("hdiutil", 60)
    wrong_scratch = [
        "/owned/root/scratch/pt/fixture-scratch/inherited-kept",
        "/owned/root/scratch/pt/p/payload/slots/t-1/quick-1/scratch",
        "/owned/root/scratch/pt/fx/keep-tmp",
    ]
    for label, options in (
        ("a cleanup detach that exits nonzero", {"cleanup_exit": 5}),
        ("a cleanup detach that times out", {"cleanup_raise": timeout}),
    ):
        caught, effects, _world = run_flow(observed=wrong_scratch, **options)
        property_true(
            f"{label} keeps custody unknown and the backing",
            isinstance(caught, UnknownOutcome)
            and "image custody is unknown" in str(caught)
            and "not on the image" in str(caught)
            and image_path in effects.files,
        )

    # Mutants of the fixture's own binding premises must fail, and a mutant
    # with an unknown cleanup keeps the first cause visible while aborting.
    for label, options, _expected in (
        ("a mount outside /Volumes", {"mount": "/tmp/not-volumes"}, (UnknownOutcome,)),
        ("ownership disabled", {"owners": False}, (UnknownOutcome,)),
        ("no volume UUID", {"volume_uuid": ""}, (UnknownOutcome,)),
        (
            "an unclassifiable attach record",
            {"attach_plist": b"not a plist"},
            (UnknownOutcome,),
        ),
        ("a failed attach", {"attach_exit": 1}, (UnknownOutcome,)),
        ("a failed planned detach", {"detach_exit": 1}, (UnknownOutcome,)),
        (
            "a failed planned detach with a replayed cleanup",
            {"detach_exit": 1, "cleanup_exit": 0},
            (UnknownOutcome,),
        ),
    ):
        caught, effects, _world = run_flow(**options)
        property_true(
            f"{label} aborts with unknown custody and keeps the backing",
            isinstance(caught, UnknownOutcome)
            and ("unlink", image_path) not in effects.calls
            and effects.detach_calls <= 1,
        )
    caught, effects, _world = run_flow(detach_exit=1, cleanup_exit=0)
    property_true(
        "a planned detach that was already attempted is never replayed",
        effects.detach_calls == 1 and "already attempted" in str(caught),
    )
    caught, effects, _world = run_flow(
        cleanup_exit=5, observed=wrong_scratch
    )
    property_true(
        "an unknown cleanup keeps the original case fault visible as primary",
        isinstance(caught, UnknownOutcome)
        and "custody is unknown" in str(caught)
        and "not on the image" in str(caught)
        and "primary fault" in str(caught),
    )

    # Live-seam counterfactuals: a wrong or missing device binding, an
    # identifier the real observe rejects, both ambiguous detach post-states,
    # a cleanup that leaves the volume mounted, and a deadline abort raised
    # inside the cleanup decision. Each drives the real custody functions.
    caught, effects, _world = run_flow(entity_devices=[""])
    property_true(
        "a missing device binding aborts before the attachment record exists",
        isinstance(caught, UnknownOutcome)
        and "bound no device entry" in str(caught)
        and os.path.join(custody_dir, "attachment.json") not in effects.files
        and os.path.join(custody_dir, "detach-attempt.json") not in effects.files,
    )
    caught, effects, _world = run_flow(entity_devices=["", "/dev/disk9"])
    property_true(
        "an attach record with no mount device node is refused",
        isinstance(caught, UnknownOutcome)
        and "not one of the attached device entries" in str(caught)
        and image_path in effects.files,
    )
    caught, effects, _world = run_flow(device_node="/dev/disk99s1")
    property_true(
        "a swapped device binding is refused before the attachment record",
        isinstance(caught, UnknownOutcome)
        and "/dev/disk99s1" in str(caught)
        and os.path.join(custody_dir, "attachment.json") not in effects.files
        and image_path in effects.files,
    )
    caught, effects, world = run_flow(observed=wrong_scratch, stale_nonce=True)
    property_true(
        "a replayed observation identifier fails through the real observe",
        isinstance(caught, Recoverable)
        and "stale observation identifier" in str(caught)
        and world.accepted == [],
    )
    caught, effects, _world = run_flow(detach_keeps_mounted=True)
    property_true(
        "a detach that reports success but leaves the volume mounted aborts",
        isinstance(caught, UnknownOutcome)
        and "still mounted after detach" in str(caught)
        and effects.detach_calls == 1
        and image_path in effects.files,
    )
    caught, effects, _world = run_flow(recreate_mount=True)
    property_true(
        "a detach whose mount point is recreated aborts and keeps the backing",
        isinstance(caught, UnknownOutcome)
        and "was recreated" in str(caught)
        and effects.detach_calls == 1
        and image_path in effects.files,
    )
    caught, effects, _world = run_flow(
        observed=wrong_scratch, cleanup_keeps_mounted=True
    )
    property_true(
        "a cleanup detach that leaves the volume mounted keeps custody unknown",
        isinstance(caught, UnknownOutcome)
        and "image custody is unknown" in str(caught)
        and "not positively established" in str(caught)
        and image_path in effects.files,
    )

    def armed_deadline(call: int) -> float:
        # The wrong-observation failure reaches the custody decision on the
        # fourth work_left call (create, attach, diskutil, cleanup), so the
        # real deadline abort is raised inside the cleanup decision itself.
        if call >= 4:
            work_left(now_ns=2_000_000_000, deadline_ns=1_000_000_000, margin_s=30)
        return 60.0

    caught, effects, _world = run_flow(
        observed=wrong_scratch, work_left_fn=armed_deadline
    )
    property_true(
        "a deadline abort inside the cleanup decision keeps custody unknown",
        isinstance(caught, UnknownOutcome)
        and "image custody is unknown" in str(caught)
        and "work deadline reached" in str(caught)
        and "not on the image" in str(caught)
        and image_path in effects.files,
    )

    # Case 12's 91-byte oracles with fake product states, through the real
    # path_bound body: a socket-safe observation passes, and an over-bound
    # observation, a non-scratch leaf and an over-bound accepted path each
    # fail rather than being accepted by a vacuous check.
    class FakePathWorld:
        observe = World.observe

        def __init__(self, observed, accepted=()):
            self.pool = "/private/tmp/oxide-proof.abcd1234/scratch/pt/p"
            self.observed = observed
            self.accepted = list(accepted)
            self.calls = 0

        def env(self, **overrides):
            env = {"OXIDE_PROOF_NONCE": "n"}
            for key, value in overrides.items():
                if value is None:
                    env.pop(key, None)
                else:
                    env[key] = value
            return env

        def run(self, args, *, cwd, env, check=True):
            self.calls += 1
            return Proc(
                0,
                "",
                f"cargo:warning=OXIDE-TEMP nonce={env.get('OXIDE_PROOF_NONCE')}\n"
                f"cargo:warning=OXIDE-TEMP dir={self.observed}\n",
            )

    def run_path_bound(observed, accepted=()):
        world = FakePathWorld(observed, accepted)
        saved_cases = list(CASES)
        caught = None
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
                io.StringIO()
            ):
                path_bound(world, "/crate")
        except BaseException as error:  # noqa: BLE001 - the oracle asserts on it
            caught = error
        finally:
            CASES[:] = saved_cases
        return caught, world

    slot = (
        "/private/tmp/oxide-proof.abcd1234/scratch/pt/p/payload/slots/"
        "t-1abcdef/quick-1/scratch"
    )
    caught, world = run_path_bound(slot)
    property_true(
        "case 12 accepts a real socket-safe observation and records it",
        caught is None
        and world.calls == 1
        and world.accepted == [slot]
        and len(slot.encode()) <= SOCKET_PATH_BOUND,
    )
    long_leaf = (
        "/private/tmp/oxide-proof.abcd1234/scratch/pt/p/payload/slots/"
        "t-1abcdef/quick-1/" + "s" * 30 + "/scratch"
    )
    caught, _world = run_path_bound(long_leaf)
    property_true(
        "case 12 refuses an over-bound observation",
        isinstance(caught, Recoverable) and "over the 91-byte bound" in str(caught),
    )
    caught, _world = run_path_bound(slot[: -len("/scratch")] + "/tmpdir")
    property_true(
        "case 12 refuses an observation that does not end in scratch",
        isinstance(caught, Recoverable) and "does not end in scratch" in str(caught),
    )
    caught, _world = run_path_bound(slot, accepted=[long_leaf])
    property_true(
        "case 12 refuses an over-bound previously accepted path",
        isinstance(caught, Recoverable) and "an accepted path is" in str(caught),
    )

    # An abort inside case 11 still records case 12 as not run, through the
    # real dispatcher and the real receipt writer.
    class AbortRemovable:
        root = "/owned/root/scratch/pt"

        def __init__(self):
            self.calls = []

        def canonical(self, path):
            return norm(path)

        def is_dir(self, path):
            return True

    def topology_abort_case():
        removable_flow(
            AbortRemovable(),
            "/crate",
            FakeEffects(),
            published_scratch=None,
            published_evidence=None,
        )

    abort_chain = [
        ProofCase(i, body(f"pre{i}"), label=f"pre{i}") for i in range(10)
    ] + [
        ProofCase(10, topology_abort_case, mutates=True, label="removable"),
        ProofCase(11, body("after"), depends=(10,), label="bound"),
    ]
    captured_chain = {}

    def capture_chain(path, outcomes, faults, skips, aborted):
        captured_chain["text"] = diagnostic_record(outcomes, faults, skips, aborted)
        return path

    saved_writer = globals()["write_diagnostic_receipt"]
    globals()["write_diagnostic_receipt"] = capture_chain
    saved_mode = MODE["diagnostic"]
    MODE["diagnostic"] = True
    try:
        ran.clear()
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(
            io.StringIO()
        ):
            chain_status = drive(abort_chain, "/nonexistent")
    finally:
        MODE["diagnostic"] = saved_mode
        globals()["write_diagnostic_receipt"] = saved_writer
    chain_record = json.loads(captured_chain["text"])
    property_true(
        "a case 11 custody abort records case 12 as not run",
        chain_status == 1
        and "after" not in ran
        and chain_record["cases_total"] == 12
        and chain_record["cases"][11]["outcome"] == "not_run"
        and chain_record["aborted_on_unknown_outcome"] is True,
    )

    print(f"self-check ok: {len(checks)} properties, pure (no scratch, child or toolchain)")
    for name in checks:
        print(f"  ok {name}")
    return 0


def release_removed(world: "World") -> None:
    """Case 7: a released slot keeps no owned directory."""
    case(CASE_NAMES[6])
    expect(
        not any(os.path.lexists(path) for path in world.scratch_dirs()),
        "a released slot kept an owned directory",
    )
    expect(
        not world.last_owned or not os.path.lexists(world.last_owned),
        f"the held case's own scratch {world.last_owned} survived its release",
    )


def proof_cases(world: "World", crate: str) -> list:
    """The twelve default cases with their declared dependencies.

    Case 2 is the first production command that needs the pool, so it creates
    it and mutates shared state; every later case therefore depends on case 2
    PASSING (a skipped predecessor is not a passed one). `mutates` marks a case
    that changes shared state (the pool's slots, the removable registration or
    the fixture checkout): a diagnostic failure in such a case skips every
    later case, because that shared state is no longer positively known.
    """
    return [
        ProofCase(0, lambda: premise_sweep(world, crate)),
        ProofCase(1, lambda: inherited_kept(world, crate), depends=(0,), mutates=True),
        ProofCase(2, lambda: replaced(world, crate), depends=(0, 1), mutates=True),
        ProofCase(3, lambda: relative_inherited(world, crate), depends=(1,), mutates=True),
        ProofCase(4, lambda: aliases(world, crate), depends=(1,), mutates=True),
        ProofCase(5, lambda: accounting(world, crate), depends=(1,), mutates=True),
        ProofCase(6, lambda: release_removed(world), depends=(1, 5)),
        ProofCase(7, lambda: crash_and_recovery(world, crate), depends=(1,), mutates=True),
        ProofCase(8, lambda: spawns(world, crate), depends=(1,), mutates=True),
        ProofCase(9, lambda: refusals(world, crate), depends=(1, 8), mutates=True),
        ProofCase(10, lambda: removable(world, crate), depends=(1,), mutates=True),
        ProofCase(11, lambda: path_bound(world, crate), depends=(1, 10), mutates=True),
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
    world_root = private_dir(os.path.join(root, "pt"))
    say(f"scratch root {root}")
    say(f"toolchain home {toolchain_home} (pinned {BUILDER}, no auto-install)")
    subject = stage_subject(world_root, toolchain_home)
    say(f"staged {subject} sha256={sha256_file(subject)}")
    world = World(world_root, subject, toolchain_home)
    crate = new_fixture(world, FIXTURE_NAME)
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
