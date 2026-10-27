#!/usr/bin/env python3
"""Focused checked-out-absence proof (oxide#135), under bounded-test.py.

One staged candidate, one private 128 MiB APFS image, no installed binary and no
live pool. The qualified shared supervisor owns the deadline, continuous host
reserve sampling, monitored scratch accounting, session closure and cleanup; this
script starts no session, signals no group, keeps no timer and owns no scratch of
its own. The private Cargo home is seeded from this HEAD's lock by the shared
bounded seeder, and the existing supervised staging helper builds the candidate.

Premise checklist (mandatory for a proof-authoring brief):
(1) seed_and_stage uses the invoking Cargo home, a private Cargo home and 1.97.1
    with no install. Staging is the only Cargo call THIS SCRIPT makes. The
    private pool bootstrap runs one product `oxide check`, which invokes Cargo in
    its own process group (two jobs); it is a bounded child in the supervisor's
    session, the product's own command reports its positive closure before it
    returns, the proof requires exit 0 and an empty session before proceeding,
    and its time and scratch are declared below.
(2) proof_env sets OXIDE_TEST_HOME_FROM_ENV=1 for every fixture command.
(3) proof_env scrubs inherited Oxide, XDG, CARGO_* and GIT_* controls before its
    private settings.
(4) every fixture checkout's `target` is asserted absent before its adopt.
(5) no external Cargo subcommand is exercised, so no argv convention is assumed.
(6) ImageCustody attaches with `-owners on` and records exactly one ordinary
    detach in a finally path, before which the backing is never removed. The
    detach runs only when THIS RUN's own bookkeeping is positively settled: any
    killed, signalled or unreaped child, an unconfirmed-closure report or any
    recorded fault latches an unknown reason BEFORE the fallible cleanup, stops
    the run before its next fixture effect, and refuses the detach while keeping
    the backing. That is direct-child bookkeeping and explicitly NOT a process
    census, and the ORDER is stated honestly: this script detaches from its own
    children's outcomes and exits, and the qualified supervisor censuses the
    session only AFTER its leader has ended, so that census is authoritative for
    the run's reported closure but cannot retroactively retain an image this
    script has already detached. The detach itself now follows the Oxide
    Supervisor's pre-detach protocol: the image is positively rebound (backing,
    mounted entity, bound device, volume UUID and whole-disk device), this
    proof's own session is censused with a bounded readable reader before the
    detach, ONE ordinary `hdiutil detach` of the bound device is made (never
    `-force` and never retried), the mount and device must be positively gone, the
    session is censused again, and only then is the evidence backing removed. Any
    unknown, unreadable row, row-bound hit or extra session member retains the
    image, its backing and its record for the owner. The residual limitation is
    named: a process that calls `setsid` leaves this session and is outside the
    census — exactly the coordinator's own scope statement — so no claim is made
    that such a writer is observed, and the backing never leaves the guard's
    never-auto-cleaned evidence directory.
(7) staging scrubs the encoded and plain Rust-wrapper flags; fixtures set
    RUSTFLAGS empty and drop CARGO_ENCODED_RUSTFLAGS.
(8) the selector fixes --root-parent /private/tmp: the missing-ancestor premise
    is the boot Data volume, which /private/tmp supplies, and the fixture's
    directory-budget pool is a genuine internal pool the product correctly
    refuses on an external volume. Both are the owner-accepted
    internal-placement exception (decision bd357a4c), inside the same monitored
    owned root; require_data_volume() refuses before any effect otherwise.
(9) no mktemp is invoked: the supervisor supplies the canonical scratch root and
    the image backing lives in its evidence directory. Every direct child is one
    short-lived `oxide` call in the supervisor's session under one private env.

What this proof is about (oxide#135): a checkout path outside /Volumes may be
proved absent when its ancestors are missing, but only from a no-follow
descriptor walk whose held nearest existing ancestor is a readable directory on
the same device and APFS volume UUID as /System/Volumes/Data. A missing chain
never waives a mount boundary, a recorded checkout volume, or any other
authority predicate, and the real run's own ancestry witness is compared again
under the checkout's barrier and binding and, as the deciding comparison, under
publication custody immediately before the journal.

Ordinary path ineligibility is a PER-ROOT keep: an unprovable path keeps its own
root and leaves the snapshot complete, so a sibling root can still be pruned.
Incomplete required authority (a missing, malformed or contradictory receipt or
registry, or unknown writer custody) still refuses the whole batch. Case 1 proves
both sides of that line in one mixed batch.

Cases, in execution order (counted fail-fast; the diagnostic records each one and
continues into cases that do not depend on a failure):
  1 data-missing-chains   a MIXED batch: a registered profile group and an
                          operator-listed receiptless legacy root, each with
                          MULTIPLE missing ancestors on the boot Data volume, are
                          candidates while a marker-recorded conflicting checkout
                          volume and a symlinked ancestor are ordinary per-root
                          keeps in the SAME batch; one real private run completes
                          both candidates through the journal, terminal receipts
                          and tombstones with no intent left, leaves both kept
                          roots byte-identical, and a live checkout stays kept.
  2 mount-boundaries      a registered checkout inside the attached image whose
                          ancestor below the mount is missing stays KEEP naming
                          the leaf-only rule, and a missing chain under an absent
                          /Volumes/<name> stays KEEP in BOTH spellings, each as a
                          complete-snapshot per-root keep.
  3 symlink-and-nondir    a symlinked ancestor and a non-directory component each
                          stay KEEP with unchanged root bytes.
  4 eacces                the debug-only scoped EACCES injection fires and keeps
                          the root, with the uninjected counterpart proving the
                          same chain absent.
  5 custody-and-group     the unknown-custody fault keeps the whole batch with no
                          stale intent, and the kept root's group rows, receipt
                          bytes and registry bytes are unchanged.
  6 witness-races         after the real run's own snapshot: recreating the
                          checkout refuses, and replacing an existing ancestor by
                          a DIFFERENT empty directory while the checkout stays
                          absent refuses with the witness comparison, which a
                          fresh absence proof alone would not notice. The seam's
                          exclusively created marker is reset and bound to each
                          race's own root, so neither race can be satisfied by a
                          stale firing.
  7 competing-owner-and-  on a positively absent missing-chain candidate: another
    mismatch              checkout's live reference keeps the root as ONE per-root
                          keep in a complete snapshot, while a mismatched group
                          identity and a mismatched receipt each refuse the whole
                          batch. No case leaves an intent, quarantine, tombstone
                          or changed root byte behind.

Prospective, UNMEASURED fit (one staged candidate, one product Cargo child for the
private pool bootstrap; planning demand 2 CPU / 4 GiB, every Cargo call at two
jobs):
  launch and source binding                                    6 s
  offline seed + constrained debug staging (2 Cargo jobs)     85 s
  image allocate/attach/register + all fixture checkouts      25 s
  private pool bootstrap (one Cargo child, own group, 2 jobs) 10 s
  case 1 (mixed batch + positive real run)                    30 s
  case 2 (one registered control + two listed controls)       10 s
  case 3 (two listed controls)                                 8 s
  case 4 (unarmed control + armed control)                     8 s
  case 5 (armed control + unchanged-authority checks)          8 s
  case 6 (two 11-second armed races + fixtures)               40 s
  case 7 (three registry/receipt controls, exact restores)    20 s
  ordinary detach, custody record and helper cleanup          25 s
  supervisor closure, reserve and contingency                 25 s
  total                                                      300 s
Scratch: 280 MiB staging + 128 MiB image + 32 MiB fixtures + 32 MiB evidence +
8 MiB bootstrap Cargo output + 32 MiB margin = 512 MiB. Every figure includes
cleanup, is prospective and is NOT a measured fit: a native run must establish it
at the published source.

Each native invocation needs independent published source-fit, fresh finite
headroom and a SEPARATE explicit ROOT activation. Diagnostic mode is uncounted
and never a PASS.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
from types import SimpleNamespace
import stat
import subprocess
import sys
import time
import traceback

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


wrapper = load("proof_wrapper135", "test-oxide-wrapper-bypass.py")
prior = load("proof_prune135", "test-outputs-prune.py")
permanent = prior._load("permanent135", "test-permanent-outputs.py")
prior.permanent = permanent
prior.expect = permanent.expect
prior.fail = permanent.fail
prior.DIAG = prior.Diagnostics(enabled=False)

PLAIN = '[package]\nname = "plain"\nversion = "0.1.0"\nedition = "2021"\n'
TOTAL_SECONDS = 300
TOTAL_MIB = 512
ALLOCATION_SECONDS = [
    ("launch and source binding", 6),
    ("offline seed and staging", 85),
    ("image, registration and fixtures", 25),
    ("private pool bootstrap", 10),
    ("data-missing-chains", 30),
    ("mount-boundaries", 10),
    ("symlink-and-nondir", 8),
    ("eacces", 8),
    ("custody-and-group", 8),
    ("witness-races", 40),
    ("competing-owner-and-mismatch", 20),
    ("ordinary detach and cleanup", 25),
    ("supervisor closure and contingency", 25),
]
ALLOCATION_MIB = [
    ("staging", 280),
    ("APFS sparse image", 128),
    ("fixtures", 32),
    ("evidence", 32),
    ("bootstrap Cargo output", 8),
    ("margin", 32),
]
CASE_NAMES = (
    "data-missing-chains",
    "mount-boundaries",
    "symlink-and-nondir",
    "eacces",
    "custody-and-group",
    "witness-races",
    "competing-owner-and-mismatch",
)
# Which earlier cases a case depends on. A recoverable failure records itself and
# blocks only its dependents.
CASE_NEEDS: dict[str, tuple] = {name: () for name in CASE_NAMES}
# A case that MUTATES the fixture, the registry or a receipt is TAINTING: after
# its failure this run no longer knows the state it left behind, so the
# uncounted diagnostic stops instead of continuing into that state. Only the two
# read-only cases (a listed dry run over a static fixture) are genuinely
# recoverable; every other case vanishes a chain, renames a mount directory,
# edits the registry or a receipt, or races a child.
CASE_TAINTS: dict[str, bool] = {
    "data-missing-chains": True,
    "mount-boundaries": True,
    "symlink-and-nondir": False,
    "eacces": False,
    "custody-and-group": True,
    "witness-races": True,
    "competing-owner-and-mismatch": True,
}
# The product's debug-only fixture seams used here, with the marker file each
# writes. A firing is an observation of the fixture, never a gate.
EACCES_VAR, EACCES_MARKER = "OXIDE_TEST_PRUNE_EACCES", "prune-eacces-fired"
DELAY_VAR, DELAY_MARKER = "OXIDE_TEST_PRUNE_DIAGNOSTIC_DELAY", "prune-diagnostic-delay-fired"
CUSTODY_VAR, CUSTODY_MARKER = (
    "OXIDE_TEST_PRUNE_UNKNOWN_CUSTODY",
    "prune-unknown-custody-fired",
)
# The product's own words for each refusal this proof expects: exact source
# sentences, never loose keywords.
PRESENT_KEEP = "the checkout is present, so its root is kept"
LEAF_ONLY_REFUSAL = "an ancestor of the checkout below its mount is missing"
NO_RECORDED_VOLUME = "a /Volumes checkout has no recorded volume UUID"
RECORDED_VOLUME_CONFLICT = "is not on the checkout's recorded volume"
NOT_A_DIRECTORY = "an ancestor of the checkout is not a directory"
UNREADABLE = "Permission denied"
ABSENCE_UNKNOWN = "checkout absence is unknown ("
WITNESS_REFUSAL = "the checkout's ancestry changed after the real run's snapshot"
COMPETING_OWNER = "another checkout's live reference also names this root"
GROUP_DISAGREEMENT = "selected group references disagree about root identity"
RECEIPT_MISMATCH = "the registry row does not match the checkout's committed receipt"
UNKNOWN_CUSTODY = "writer custody is unknown (injected fixture)"


def require(value, message):
    if not value:
        raise RuntimeError(message)


def require_batch_refusal(doc, expected):
    """The whole batch was kept and the named INCOMPLETE-AUTHORITY fault did it."""
    require(
        doc["coverage"]["complete"] is False and not doc["candidates"],
        f"the batch was not kept as incomplete authority: {doc['coverage']} {doc['candidates']}",
    )
    reasons = [item["reason"] for item in doc["kept"]]
    require(
        any(expected in reason for reason in reasons),
        f"the batch refusal did not name {expected!r}: {reasons}",
    )


def require_root_kept(doc, root, expected):
    """One root was kept as ORDINARY path ineligibility in a COMPLETE snapshot.

    That distinction is the point: an unprovable path is not an untrustworthy
    snapshot, so a sibling root can still be a candidate in the same batch.
    """
    require(
        doc["coverage"]["complete"] is True,
        f"an ordinary keep made the snapshot incomplete: {doc['coverage']}",
    )
    require(
        root not in {candidate["root"] for candidate in doc["candidates"]},
        f"{root} became a candidate",
    )
    reasons = [item["reason"] for item in doc["kept"] if item.get("root") == root]
    # One root may carry several kept references (one per checkout/root group),
    # so the oracle is that THIS root's keep names the expected fault.
    require(
        any(expected in reason for reason in reasons),
        f"no keep reason named {expected!r} for {root}: {reasons}",
    )


def mount_branch(path: str) -> str:
    """Which absence branch an exact absolute checkout path takes.

    A mirror of the product's component classifier, used only so a case can
    prove it exercises the branch it intends: the same missing chain under
    /Volumes and under the boot Data alias of /Volumes is a MOUNT spelling, so
    an absent mount name is a mount refusal rather than a plain missing chain.
    """
    if not path.startswith("/") or path.endswith("/") or "//" in path:
        return "invalid"
    parts = path.split("/")[1:]
    if any(part in ("", ".", "..") for part in parts):
        return "invalid"
    if parts[:1] == ["Volumes"]:
        return "mount" if len(parts) > 2 else "invalid"
    if parts[:4] == ["System", "Volumes", "Data", "Volumes"]:
        return "mount" if len(parts) > 5 else "invalid"
    return "root"


def require_data_volume(root: str, mount: str) -> None:
    """The fixture premise: fixture checkouts are on the boot Data volume.

    The product proves the same fact from held descriptors and volume UUIDs; the
    fixture only compares devices so a wrong `--root-parent` refuses before any
    effect instead of silently proving nothing.
    """
    data = os.stat("/System/Volumes/Data")
    here = os.stat(root)
    require(
        here.st_dev == data.st_dev,
        f"{root} is not on the boot Data volume; run this proof with "
        "--root-parent /private/tmp (the owner-accepted internal-placement exception)",
    )
    if mount:
        require(
            os.stat(mount).st_dev != data.st_dev,
            f"the attached image {mount} is not a separate device, so it cannot be "
            "the non-Data control",
        )


def closure_unknown(code, killed: bool, reaped: bool, unclosed) -> str | None:
    """Why one finished child's writer closure is UNKNOWN, or `None`.

    A direct child that ended on a signal, a child that was never reaped, a
    leader this run had to kill, or any pid still in the session means the group
    behind it is not positively closed. The decision is pure so the uncounted
    diagnostic and the pure check share it.
    """
    if killed:
        return "a proof child was killed, so its group closure is unknown"
    if not reaped:
        return "a proof child was never reaped, so its group closure is unknown"
    if code is None:
        return "a proof child left no exit status, so its closure is unknown"
    if code < 0:
        return f"a proof child ended on signal {-code}, so its group closure is unknown"
    if unclosed:
        return f"{len(unclosed)} proof child(ren) were never reaped, so closure is unknown"
    return None


def abnormal_child(code, stderr: str) -> str | None:
    """Why one child's writer closure is NOT positively known, or `None`.

    A signalled exit and any of the product's own unconfirmed-closure sentences
    both mean the child's group may still be live, so neither may be read as a
    closed writer. Pure: the adapter hook, `settle_child` and the pure check
    share one decision.
    """
    if code is not None and code < 0:
        return f"the child ended on signal {-code}, so its group closure is unknown"
    for marker in permanent.UNCONFIRMED_CUSTODY_MARKERS:
        if marker in stderr:
            return f"the child reported an unconfirmed group closure ({marker!r})"
    return None


def custody_abort_reason(proof, unclosed) -> str | None:
    """Why THIS RUN's own bookkeeping forbids continuing or detaching, or `None`.

    It joins the proof's own latch, the shared child helper's unconfirmed closure
    and the direct children it started that were never reaped. It is explicitly
    NOT a process census: this script sees only the children it started, never a
    grandchild that left their group. The qualified supervisor's session census
    runs AFTER this leader exits, so it is authoritative for the run's reported
    closure but cannot prevent a detach this script already completed; a writer
    that escaped its parent's group is outside the cooperative custody boundary
    both this proof and the product document. The proof therefore refuses to
    continue or detach on any fault it CAN see, and never claims more.
    """
    latched = getattr(proof, "custody_unknown", None)
    if latched is not None:
        return latched
    if not proof.closure_known:
        return "the shared child helper reported unconfirmed closure"
    if unclosed:
        return f"{len(unclosed)} proof child(ren) were never reaped"
    return None


def latch_unknown(proof, reason: str) -> None:
    """Record the FIRST custody reason, BEFORE any fallible cleanup."""
    fresh = getattr(proof, "custody_unknown", None) is None
    if fresh:
        proof.custody_unknown = reason
    proof.closure_known = False
    if fresh:
        print(f"custody latched unknown: {reason}", file=sys.stderr, flush=True)


def settle_child(world, child, proof, label: str, unclosed, wait: float = 120.0):
    """Wait for one armed child, then decide its custody BEFORE cleaning up.

    A child that is still RUNNING is expected: the product is inside its own
    11-second fixture delay while the proof mutates the fixture. Only a child
    that outlives this bounded wait is killed, and the latch is recorded BEFORE
    the kill, so a killed leader's surviving group can never be read as closed.
    """
    killed = False
    deadline = time.monotonic() + min(wait, permanent.work_left())
    while child.poll() is None:
        if time.monotonic() >= deadline:
            killed = True
            latch_unknown(
                proof, f"{label}: the child outlived its {wait:.0f}-second wait and was killed"
            )
            child.kill()
            break
        time.sleep(0.02)
    try:
        code, out, err = world.finish(child, label)
    except BaseException as error:
        latch_unknown(proof, f"{label}: the child could not be reaped ({error})")
        raise
    reason = closure_unknown(code, killed, True, unclosed) or abnormal_child(code, err)
    if reason is not None:
        latch_unknown(proof, f"{label}: {reason}")
    return code, out, err, reason


class Adapter:
    """The existing qualified-session child helper behind World's own API."""

    def __init__(self, proof, binary, unclosed=None):
        self.inner = proof
        self.root = str(proof.root)
        self.binary = str(binary)
        self.env = {}
        self.serial = 0
        self.unclosed = unclosed if unclosed is not None else set()

    def run(self, argv, cwd=None, env=None, timeout=120):
        self.serial += 1
        label = f"prune135-{self.serial:03d}"
        code, out, err = self.inner.run(argv, cwd=cwd, env=env, label=label, limit=timeout)
        # Every ordinary fixture child is classified too: a signalled exit or the
        # product's own unconfirmed-closure sentence is never read as closed.
        reason = abnormal_child(code, err)
        if reason is not None:
            latch_unknown(self.inner, f"{label}: {reason}")
            # Stop HERE: the next fixture effect must not follow a child whose
            # group is not positively closed.
            require_settled(self.inner, f"{label}")
        return subprocess.CompletedProcess(argv, code, out.encode(), err.encode())


def chain_checkout(world, path: Path) -> str:
    """One tiny Git checkout at an ARBITRARY path, so its ancestors can vanish
    together later. Its `target` is asserted absent before its adopt."""
    path.mkdir(parents=True)
    (path / "src").mkdir()
    (path / "Cargo.toml").write_text(PLAIN)
    (path / "src" / "lib.rs").write_text("pub fn v() -> u32 { 1 }\n")
    world.proof.run(["git", "init", "-q"], cwd=str(path), timeout=30)
    (path / ".gitignore").write_text("/target\n/.oxide-outputs/\n")
    require(not os.path.lexists(path / "target"), f"{path} already has a target")
    return os.path.realpath(path)


def listed_legacy_root(
    world,
    marker_template: dict,
    checkout: str,
    label: str,
    recorded_checkout_volume: str | None = None,
) -> tuple:
    """One operator-listed receiptless root whose marker names `checkout`.

    Without `recorded_checkout_volume` it is the genuine legacy shape (root/1,
    no nonce, no recorded checkout volume). With one, it keeps the current
    marker shape and nonce and records that volume, which the product must
    honour: the mixed batch's conflicting control is exactly this case. Only one
    leaf is created: the container is the product's own owner-only
    oxide-outputs directory."""
    checkout_id = hashlib.sha256(checkout.encode()).hexdigest()
    root = os.path.join(world.mount, "oxide-outputs", f"{label}-{checkout_id[:8]}")
    os.mkdir(root, 0o700)
    document = dict(marker_template)
    if recorded_checkout_volume is None:
        document.pop("nonce", None)
        document.pop("checkout_volume_uuid", None)
        document["version"] = "oxide.outputs.root/1"
    else:
        document["checkout_volume_uuid"] = recorded_checkout_volume
    document.update(
        checkout=checkout,
        checkout_id=checkout_id,
        generation=1,
        label=label,
    )
    Path(root, ".oxide-output.json").write_text(json.dumps(document))
    Path(root, "payload").write_bytes(b"legacy-payload" * 512)
    return root, checkout_id


def operator_list(world, roots: list, name: str) -> str:
    path = os.path.join(world.root, f"operator-list-{name}.json")
    Path(path).write_text(json.dumps(roots))
    return path


def vanished(world, path: str) -> str:
    """Move a whole ancestor subtree away, so the recorded checkout path and
    every component below it are missing while its parent stays on Data."""
    away = os.path.join(world.gone, os.path.basename(path) + "-" + secrets.token_hex(3))
    os.rename(path, away)
    require(not os.path.lexists(path), f"{path} still exists after vanishing")
    return away


def fired(path: Path, expected: str) -> None:
    require(path.exists(), f"{path.name} never fired")
    recorded = path.read_text(errors="replace").strip().splitlines()
    require(
        recorded and recorded[-1] == expected,
        f"{path.name} did not record {expected}: {recorded}",
    )


def snapshot_file(path: Path) -> tuple:
    """Exact bytes and mode of one control document this proof restores."""
    facts = os.lstat(path)
    require(stat.S_ISREG(facts.st_mode) and facts.st_mode & 0o777 == 0o600, f"{path} is not 0600")
    return path.read_bytes(), stat.S_IMODE(facts.st_mode)


def restore_file(path: Path, saved: tuple) -> None:
    """Restore exact bytes and mode, then verify both."""
    original, mode = saved
    path.write_bytes(original)
    os.chmod(path, mode)
    require(path.read_bytes() == original, f"{path} was not restored byte-exactly")
    require(stat.S_IMODE(os.lstat(path).st_mode) == mode, f"{path} lost its mode")


def sweep_unconfirmed(proof) -> None:
    """Latch if ANY child this run recorded an unconfirmed-closure sentence.

    Every wrapper child writes its stderr into the evidence directory, so this is
    a complete sweep over the children this proof ran, not a per-label guess. It
    applies the same pure `abnormal_child` decision and only observes: the latch
    then gates diagnostic continuation and the ordinary detach.
    """
    for path in sorted(proof.evidence.glob("*.stderr")):
        try:
            text = path.read_text(errors="replace")
        except OSError as error:
            latch_unknown(proof, f"cannot classify {path.name} ({error})")
            continue
        for marker in permanent.UNCONFIRMED_CUSTODY_MARKERS:
            if marker in text:
                latch_unknown(
                    proof,
                    f"{path.name}: the child reported an unconfirmed group closure ({marker!r})",
                )
                break


def require_settled(proof, stage: str) -> None:
    """Stop BEFORE the next effect as soon as this script's own custody faults.

    Setup must not continue after a latched fault: no image attach, no fixture
    and no further child may follow a child whose group is not positively closed.
    """
    reason = custody_abort_reason(proof, proof.unclosed)
    if reason is not None:
        raise prior.Aborted(f"{stage}: custody is not positively closed ({reason})")


def restore_fixture(proof, label: str, steps) -> None:
    """Run one fixture restore, or refuse to touch it while custody is unknown.

    A killed or unreaped child may still be walking the fixture, so restoring it
    would mutate state whose writer is unknown. The state is then RETAINED (the
    evidence is the next operator's) and the collection aborts. A restore that
    itself fails latches the reason before it propagates.
    """
    reason = custody_abort_reason(proof, proof.unclosed)
    if reason is not None:
        print(f"{label}: fixture state retained, custody unknown ({reason})", file=sys.stderr)
        raise prior.Aborted(f"{label}: custody is not positively closed ({reason})")
    try:
        steps()
    except BaseException as error:
        latch_unknown(
            proof, f"{label}: the fixture restore failed ({type(error).__name__}: {error})"
        )
        raise


def armed_real_run(world, plan: str, sha: str, extra: dict):
    """Start one real run in the supervisor's session, so the proof can mutate
    the fixture while the product holds its own snapshot."""
    env = dict(world.proof.env)
    env.update(extra)
    argv = [world.proof.binary, "outputs", "prune", "--plan", plan, "--plan-sha256", sha]
    return permanent.start_child(world.proof, argv, world.root, env)


def await_firing(marker: Path, expected: str, child, label: str) -> None:
    """Wait, bounded by the supervisor's own deadline, for THIS run's firing.

    The seam's marker is created exclusively, so a marker that already exists is
    a previous case's firing and can never satisfy this wait: the loop requires
    the recorded path to be `expected`. The child is never finished here: its
    custody decision belongs to `settle_child`, which latches before cleanup."""
    deadline = time.monotonic() + min(30.0, permanent.work_left())
    while True:
        if marker.exists():
            recorded = marker.read_text(errors="replace").strip().splitlines()
            if recorded and recorded[-1] == expected:
                return
        code = child.poll()
        if code is not None:
            raise RuntimeError(
                f"{label} ended (exit {code}) before {marker.name} fired for {expected}"
            )
        if time.monotonic() >= deadline:
            raise RuntimeError(f"{label} never fired {marker.name} for {expected}")
        time.sleep(0.05)


def write_competitor(world, control: str, template_row: dict, root: str) -> tuple:
    """A hand-written live registry row for ANOTHER checkout naming `root`.

    The product's own rows cannot produce this state (two checkouts never select
    one root), so the fixture writes the row and the committed whole-target
    receipt its readers require, and the proof restores both exactly. The
    fabricated checkout path is under `/`, where no missing-chain proof applies,
    so the competitor can never become a candidate itself.
    """
    checkout = f"/oxide135-competitor/{secrets.token_hex(8)}"
    checkout_id = hashlib.sha256(checkout.encode()).hexdigest()
    receipt_path = os.path.join(control, "outputs", checkout_id + ".json")
    row = dict(template_row)
    for key in ("pruning", "pruned_ms", "reservation", "cancelling"):
        row.pop(key, None)
    row.update(
        checkout=checkout,
        checkout_id=checkout_id,
        receipt=receipt_path,
        member="target",
        generation=1,
        selected=True,
        pending=False,
    )
    selection = {
        "kind": "external",
        "root": root,
        "volume_uuid": row["volume_uuid"],
        "nonce": row["nonce"],
        "generation": 1,
    }
    document = {
        "version": "oxide.outputs.transition/2",
        "checkout": checkout,
        "checkout_id": checkout_id,
        "generation": 1,
        "state": "committed",
        "roots": [selection],
        "links": [{"member": "target", "selected": selection}],
        "updated_ms": 1,
    }
    Path(receipt_path).write_text(json.dumps(document))
    os.chmod(receipt_path, 0o600)
    return row, receipt_path


def verified_seed_source(home: Path) -> dict:
    """Resolve the two account-home links, then positively bind one real root.

    The strict seeder still opens that root and every descendant no-follow on
    the same device. This function never writes to the host cache.
    """
    if not home.is_absolute():
        raise RuntimeError("the host Cargo home is not absolute")
    registry = (home / "registry").resolve(strict=True)
    git = (home / "git").resolve(strict=True)
    require(
        registry.name == "registry" and git.name == "git",
        "the host Cargo cache has an unexpected layout",
    )
    require(
        registry.parent == git.parent,
        "the registry and Git caches do not share one canonical root",
    )
    root = registry.parent
    require(
        root.is_absolute() and root.resolve(strict=True) == root,
        "the Cargo cache root is not canonical",
    )
    identity = {}
    with wrapper.offline_seed.Anchor(root, "canonical host Cargo cache") as anchor:
        root_metadata = os.lstat(root)
        require(
            stat.S_ISDIR(root_metadata.st_mode)
            and (root_metadata.st_dev, root_metadata.st_ino) == anchor.identity,
            "the Cargo cache root changed while binding",
        )
        identity["root"] = list(anchor.identity)
        for name, path in (("registry", registry), ("git", git)):
            metadata = os.lstat(path)
            require(
                stat.S_ISDIR(metadata.st_mode) and metadata.st_dev == anchor.dev,
                f"{name} is not a real directory on the cache device",
            )
            descriptor = anchor.open_dir((name,))
            try:
                opened = os.fstat(descriptor)
                require(
                    (opened.st_dev, opened.st_ino) == (metadata.st_dev, metadata.st_ino),
                    f"{name} changed while binding the cache",
                )
                followed = os.stat(home / name)
                require(
                    (followed.st_dev, followed.st_ino) == (opened.st_dev, opened.st_ino),
                    f"{name} account-home link changed",
                )
                identity[name] = [opened.st_dev, opened.st_ino]
            finally:
                os.close(descriptor)
    return {"root": str(root), "home": str(home), "identity": identity}


def run_cases(proof, custody, diagnostic, cases):
    host_home = Path(os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo"))
    source = verified_seed_source(host_home)
    oxide, provenance = wrapper.seed_and_stage(proof, jobs=2, host_cargo_home=source["root"])
    if verified_seed_source(host_home) != source:
        raise RuntimeError("the canonical Cargo cache identity changed during staging")
    provenance["seed_source"] = source
    # A staging child is a child too: classify what it printed before any fixture,
    # and stop here rather than attach an image beside an unclassified group.
    sweep_unconfirmed(proof)
    require_settled(proof, "after staging")
    (proof.evidence / "prune135-provenance.json").write_text(json.dumps(provenance, indent=2))
    adapter = Adapter(proof, oxide, proof.unclosed)
    world = prior.World(adapter, adapter.root)
    home, cargo_home = proof.root / "home", proof.root / "fixture-cargo-home"
    home.mkdir(mode=0o700)
    cargo_home.mkdir(mode=0o700)
    adapter.env = permanent.proof_env(world.pool, str(home), str(cargo_home))
    adapter.env["CARGO_BUILD_JOBS"] = "2"
    adapter.env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    adapter.env["RUSTFLAGS"] = ""
    # The premise refuses BEFORE any pool, image or child.
    require_data_volume(adapter.root, "")
    boot = world.checkout("bootstrap")
    code, _, err = world.oxide(["check"], "private pool bootstrap", boot)
    if code != 0:
        latch_unknown(proof, "the private pool bootstrap failed, so its Cargo group is unclassified")
        raise RuntimeError(f"the private pool bootstrap failed (exit {code}): {err}")
    require(
        not adapter.unclosed,
        f"the bootstrap left an unreaped child: {sorted(adapter.unclosed)}",
    )
    require_settled(proof, "after the private pool bootstrap")
    custody.allocate(str(proof.evidence), str(proof.root))
    world.mount = custody.attach()
    require_data_volume(adapter.root, world.mount)
    code, out, err = world.oxide(
        [
            "pool",
            "register-removable",
            world.mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "private register",
        boot,
    )
    require(code == 0 and "registered removable pool" in out, err)
    require_settled(proof, "after the drive registration")
    # The image's own volume UUID, as the product recorded it for this pool. The
    # pre-detach rebind requires exactly this value.
    proof.expected_volume_uuid = permanent.real_uuid(world.pool)

    chains = Path(adapter.root) / "chains"
    chains.mkdir(mode=0o700)

    def adopt_chain(tail: tuple) -> dict:
        """One tiny Git checkout at `chains/<token>/<tail...>`, adopted while
        its ancestors are present. `token` is the chain root a case may move
        away, so the recorded checkout path keeps its exact bytes while every
        component below the token becomes missing."""
        token = chains / secrets.token_hex(4)
        path = token
        for component in tail:
            path = path / component
        checkout = chain_checkout(world, path)
        world.adopt(checkout)
        require_settled(proof, "during fixture adoption")
        return {"checkout": checkout, "path": path, "token": token}

    group = adopt_chain(("a", "b", "checkout"))
    live = adopt_chain(("checkout",))
    kept_group = adopt_chain(("checkout",))
    owned_chain = adopt_chain(("checkout",))
    recreate_race = adopt_chain(("checkout",))
    replace_race = adopt_chain(("anc", "child", "checkout"))
    custody_case = adopt_chain(("checkout",))
    image = {"checkout": world.checkout("image-chain", base=world.mount)}
    world.adopt(image["checkout"])
    require_settled(proof, "during the image fixture adoption")
    marker_template = prior.marker(world.selected(group["checkout"])["root"])
    # A receiptless root whose marker RECORDS the image's checkout volume while
    # its surviving ancestors are on the boot Data volume: the record must be
    # honoured, so this chain is KEPT even though the Data proof holds.
    conflicting_checkout = os.path.join(
        adapter.root, "chains", secrets.token_hex(4), "x", "checkout"
    )
    conflicting_root, _ = listed_legacy_root(
        world,
        marker_template,
        conflicting_checkout,
        "conflict",
        recorded_checkout_volume=permanent.real_uuid(world.pool),
    )
    # A receiptless legacy root whose checkout has TWO missing ancestors plus
    # the missing leaf, and which never existed as a checkout at all.
    legacy_checkout = os.path.join(
        adapter.root, "chains", secrets.token_hex(4), "x", "y", "checkout"
    )
    legacy_root, _ = listed_legacy_root(world, marker_template, legacy_checkout, "legacy")
    # The debug-only EACCES control: an existing directory the walk must read,
    # with the same shape of missing chain below it.
    listed_dir = Path(adapter.root) / "listed"
    listed_dir.mkdir(mode=0o700)
    eacces_target = str(listed_dir)
    eacces_checkout = str(listed_dir / "eacces" / "chain" / "checkout")
    eacces_root, _ = listed_legacy_root(world, marker_template, eacces_checkout, "eacces")
    # A symlinked ancestor and a non-directory component, each its own fixture.
    links = Path(adapter.root) / "links"
    (links / "real").mkdir(parents=True, mode=0o700)
    os.symlink(links / "real", links / "parent")
    symlink_checkout = str(links / "parent" / "checkout")
    symlink_root, _ = listed_legacy_root(world, marker_template, symlink_checkout, "symlink")
    files = Path(adapter.root) / "files"
    files.mkdir(mode=0o700)
    (files / "plain").write_bytes(b"not a directory")
    file_checkout = str(files / "plain" / "checkout")
    file_root, _ = listed_legacy_root(world, marker_template, file_checkout, "file")
    # Two absent-mount controls, one per /Volumes spelling, under a mount name
    # unique to this run that is never mounted.
    absent_mount = "Ox135Absent" + secrets.token_hex(3)
    plain_mount_checkout = f"/Volumes/{absent_mount}/deep/checkout"
    plain_mount_root, _ = listed_legacy_root(world, marker_template, plain_mount_checkout, "mount")
    alias_mount_checkout = f"/System/Volumes/Data/Volumes/{absent_mount}/deep/checkout"
    alias_mount_root, _ = listed_legacy_root(world, marker_template, alias_mount_checkout, "aliased")

    roots = {
        key: world.selected(value["checkout"])["root"]
        for key, value in (
            ("group", group),
            ("live", live),
            ("kept_group", kept_group),
            ("owned_chain", owned_chain),
            ("recreate_race", recreate_race),
            ("replace_race", replace_race),
            ("custody", custody_case),
            ("image", image),
        )
    }
    members = sorted(
        row["member"]
        for row in world.registry_doc()["rows"]
        if row["root"] == roots["group"] and row["selected"]
    )
    require(members == ["debug", "release"], f"the profile group premise is {members}")
    for key in ("group", "live", "custody", "owned_chain"):
        Path(roots[key], "payload").write_bytes(b"x" * 4096)
    live_before = prior.tree_snapshot(roots["live"])
    live_identity = (os.lstat(roots["live"]).st_dev, os.lstat(roots["live"]).st_ino)
    failed = set()

    def case(name, body):
        needs = CASE_NEEDS[name]
        missing = [dependency for dependency in needs if dependency in failed]
        if missing:
            cases.append(
                {
                    "case": name,
                    "state": "skipped",
                    "reason": f"depends on {', '.join(missing)}",
                }
            )
            return
        cases.append({"case": name, "state": "reached"})
        try:
            body()
            cases[-1]["state"] = "completed"
        except (prior.Aborted, KeyboardInterrupt, SystemExit):
            cases[-1].update(state="failed", reason="infrastructure or custody abort")
            raise
        except BaseException as error:
            cases[-1].update(state="failed", reason=f"{type(error).__name__}: {error}")
            failed.add(name)
            if not diagnostic:
                raise
            custody_reason = custody_abort_reason(proof, proof.unclosed)
            if custody_reason is not None or CASE_TAINTS[name]:
                raise prior.Aborted(
                    f"{name} failed and this run cannot prove the state it left "
                    f"({custody_reason or 'the case mutates shared fixture or authority state'}); "
                    "collection aborted"
                )
            print(f"---- {name} failed (read-only case, diagnostic continues): {error!r}", flush=True)
        # An unclassifiable custody or deadline outcome aborts the collection:
        # no later case may run beside an unknown writer group. The SAME
        # decision gates the ordinary detach in `main`.
        custody_reason = custody_abort_reason(proof, proof.unclosed)
        if custody_reason is not None:
            raise prior.Aborted(
                f"custody is not positively closed ({custody_reason}); "
                f"collection aborted after {name}"
            )
        if permanent.work_left() < 10:
            raise prior.Aborted(f"the work window is exhausted after {name}")

    def data_missing_chains():
        vanished(world, str(group["token"]))
        # The MIXED batch: two positively absent Data chains are candidates while
        # two ordinary per-root keeps (a recorded-volume conflict and a symlinked
        # ancestor) sit in the SAME complete snapshot. A listless batch holds only
        # the registered candidate, so the receiptless roots need the exact list.
        _, out, _ = world.dry_run("no list", boot)
        without_list = json.loads(out)
        require(
            without_list["coverage"]["complete"]
            and world.candidates(without_list) == {roots["group"]},
            f"the listless batch is not exactly the registered candidate: {without_list}",
        )
        conflict_before = prior.tree_snapshot(conflicting_root)
        symlink_before = prior.tree_snapshot(symlink_root)
        listed = operator_list(
            world, [legacy_root, conflicting_root, symlink_root], "data"
        )
        plan, sha, doc = world.plan_file("mixed batch", boot, listed)
        require(
            doc["coverage"]["complete"]
            and world.candidates(doc) == {roots["group"], legacy_root},
            f"the mixed batch is not two candidates plus two keeps: "
            f"{doc['candidates']} {doc['kept']}",
        )
        require_root_kept(doc, conflicting_root, RECORDED_VOLUME_CONFLICT)
        require_root_kept(doc, symlink_root, NOT_A_DIRECTORY)
        require(
            world.kept(doc, roots["live"]) == PRESENT_KEEP,
            f"the live checkout was not kept: {doc['kept']}",
        )
        code, out, err = world.real_run("positive prune", boot, plan, sha, listed)
        require(code == 0, err)
        for root in (roots["group"], legacy_root):
            require(not os.path.lexists(root), f"{root} survived the positive prune")
        for checkout in (group["checkout"], legacy_checkout):
            receipt = permanent.read_json(permanent.receipt_file(world.pool, checkout))
            require(
                receipt["state"] == "pruned" and receipt["pruned"]["outcome"] == "deleted",
                str(receipt),
            )
        entries = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))["entries"]
        require(
            {roots["group"], legacy_root}.issubset({entry["root"] for entry in entries}),
            f"a pruned root has no tombstone: {entries}",
        )
        require(not world.intents() and not world.quarantines(), "an intent or quarantine remains")
        require(
            prior.tree_snapshot(roots["live"]) == live_before
            and (os.lstat(roots["live"]).st_dev, os.lstat(roots["live"]).st_ino) == live_identity,
            "the kept live root changed",
        )
        # The ordinary keeps in that same batch are untouched by the effect.
        require(prior.tree_snapshot(conflicting_root) == conflict_before, "the conflict root changed")
        require(prior.tree_snapshot(symlink_root) == symlink_before, "the symlink root changed")
        require("positively absent" in out, out)

    case("data-missing-chains", data_missing_chains)

    def mount_boundaries():
        payload = prior.tree_snapshot(roots["image"])
        checkouts = os.path.join(world.mount, "checkouts")
        away = os.path.join(world.mount, "gone-checkouts")
        os.rename(checkouts, away)
        try:
            _, out, _ = world.dry_run("image missing ancestor", boot)
            require_root_kept(json.loads(out), roots["image"], LEAF_ONLY_REFUSAL)
            require(prior.tree_snapshot(roots["image"]) == payload, "the kept image root changed")
        finally:
            # Restored: the row is an ordinary present-checkout keep again, so
            # no later batch inherits an unprovable absence. Like every other
            # fixture restore it is refused while custody is unknown, because a
            # surviving writer group may still be reading that image.
            restore_fixture(proof, "mount-boundaries", lambda: os.rename(away, checkouts))
        for label, spelling, root in (
            ("absent mount", plain_mount_checkout, plain_mount_root),
            ("aliased absent mount", alias_mount_checkout, alias_mount_root),
        ):
            require(mount_branch(spelling) == "mount", f"{spelling} is not a mount spelling")
            untouched = prior.tree_snapshot(root)
            listed = operator_list(world, [root], "mount")
            _, out, _ = world.dry_run(label, boot, listed)
            doc = json.loads(out)
            require_root_kept(doc, root, NO_RECORDED_VOLUME)
            require(prior.tree_snapshot(root) == untouched, f"the {label} root changed")

    case("mount-boundaries", mount_boundaries)

    def symlink_and_nondir():
        for label, spelling, root in (
            ("symlinked ancestor", symlink_checkout, symlink_root),
            ("non-directory component", file_checkout, file_root),
        ):
            untouched = prior.tree_snapshot(root)
            listed = operator_list(world, [root], label)
            _, out, _ = world.dry_run(label, boot, listed)
            require_root_kept(json.loads(out), root, NOT_A_DIRECTORY)
            require(prior.tree_snapshot(root) == untouched, f"the {label} root changed")
            require(not world.intents() and not world.quarantines(), f"{label} left an intent")

    case("symlink-and-nondir", symlink_and_nondir)

    def eacces():
        listed = operator_list(world, [eacces_root], "eacces")
        _, out, _ = world.dry_run("uninjected control", boot, listed)
        control = json.loads(out)
        require(
            control["coverage"]["complete"] and eacces_root in world.candidates(control),
            f"the uninjected chain was not proven absent: {control['kept']}",
        )
        untouched = prior.tree_snapshot(eacces_root)
        _, out, _ = world.dry_run("eacces", boot, listed, {EACCES_VAR: eacces_target})
        require_root_kept(json.loads(out), eacces_root, UNREADABLE)
        fired(proof.evidence / EACCES_MARKER, eacces_target)
        require(prior.tree_snapshot(eacces_root) == untouched, "the unreadable-ancestor root changed")
        require(not world.intents() and not world.quarantines(), "the EACCES refusal left an intent")

    case("eacces", eacces)

    def custody_and_group():
        away = vanished(world, str(custody_case["token"]))
        try:
            authority = world.authority(kept_group["checkout"])
            payload = prior.tree_snapshot(roots["kept_group"])
            _, out, _ = world.dry_run(
                "unknown custody", boot, None, {CUSTODY_VAR: custody_case["checkout"]}
            )
            # Unknown writer custody is incomplete AUTHORITY, not path
            # ineligibility: the whole batch refuses.
            require_batch_refusal(json.loads(out), UNKNOWN_CUSTODY)
            fired(proof.evidence / CUSTODY_MARKER, custody_case["checkout"])
            require(
                not world.intents() and not world.quarantines(),
                "a custody refusal left an intent",
            )
            require(
                prior.tree_snapshot(roots["kept_group"]) == payload,
                "the kept group root changed",
            )
            require(
                world.authority(kept_group["checkout"]) == authority,
                "the kept group's receipt or registry changed",
            )
            require(os.path.isdir(roots["custody"]), "the custody fixture root vanished")
        finally:
            # Put this case's own chain back: a later batch must see exactly its
            # own candidate, never a leftover from this one. It is only touched
            # when this run's writer closure is positively known.
            restore_fixture(
                proof,
                "custody-and-group",
                lambda: os.rename(away, str(custody_case["token"])),
            )
        group = [
            row
            for row in world.registry_doc()["rows"]
            if row["root"] == roots["kept_group"] and row["selected"]
        ]
        require(
            sorted(row["member"] for row in group) == ["debug", "release"],
            f"the kept root's group rows changed: {group}",
        )
        require(
            prior.tree_snapshot(roots["live"]) == live_before
            and (os.lstat(roots["live"]).st_dev, os.lstat(roots["live"]).st_ino) == live_identity,
            "the live checkout root changed",
        )

    case("custody-and-group", custody_and_group)

    def race_plan(label: str, expected_root: str):
        """One race plan whose ONLY candidate is the race's own root.

        A leftover absent chain from another case would make the product prune a
        second root after the delay, so an uncontrolled candidate set aborts the
        collection instead of being read as this case's failure.
        """
        plan, sha, doc = world.plan_file(label, boot)
        if world.candidates(doc) != {expected_root}:
            raise prior.Aborted(
                f"{label}: the fixture does not control the candidate set "
                f"({sorted(world.candidates(doc))}); no race may run"
            )
        return plan, sha

    def witness_races():
        # (a) The checkout recreated after the real run's own snapshot. A fresh
        # absence proof already sees it present, so this proves the lock-time
        # revalidation rather than the witness comparison. The product is inside
        # its own 11-second fixture delay while the fixture mutates, so the child
        # is expected to be RUNNING here: settle_child waits for it.
        away = vanished(world, str(recreate_race["token"]))
        try:
            plan, sha = race_plan("recreate plan", roots["recreate_race"])
            before = prior.tree_snapshot(roots["recreate_race"])
            child = armed_real_run(world, plan, sha, {DELAY_VAR: roots["recreate_race"]})
            try:
                await_firing(
                    proof.evidence / DELAY_MARKER,
                    roots["recreate_race"],
                    child,
                    "recreate race",
                )
                os.makedirs(recreate_race["checkout"], exist_ok=True)
            finally:
                code, out, err, reason = settle_child(
                    world, child, proof, "recreate race", adapter.unclosed
                )
            if reason is not None:
                raise prior.Aborted(f"the recreate race left custody unknown: {reason}")
            require(code != 0, f"a recreated checkout was pruned: {out}{err}")
            require(
                PRESENT_KEEP in err, f"the recreated checkout was not refused as present: {err}"
            )
            require(prior.tree_snapshot(roots["recreate_race"]) == before, "the refused root changed")
            require(
                not world.intents() and not world.quarantines(),
                "the recreate race left an intent",
            )
            # The product creates this seam's marker exclusively, so the second
            # race needs its own firing: removing this case's marker is what
            # makes the next wait prove the SECOND root's delay.
            marker = proof.evidence / DELAY_MARKER
            fired(marker, roots["recreate_race"])
            marker.unlink()
        finally:
            # The race mutation made an empty checkout directory where the chain
            # was; remove exactly that and put the original chain back, but only
            # while custody is positively known.
            def restore_recreate():
                if os.path.lexists(recreate_race["checkout"]):
                    require(
                        not os.listdir(recreate_race["checkout"]),
                        "the recreated checkout is not empty; the fixture state is unknown",
                    )
                    os.rmdir(recreate_race["checkout"])
                os.rename(away, str(recreate_race["token"]))

            restore_fixture(proof, "recreate race", restore_recreate)
        # (b) An EXISTING ancestor replaced by a DIFFERENT empty directory while
        # the checkout stays absent. A fresh absence proof still succeeds, so
        # only the real run's own witness comparison can refuse this.
        child_away = vanished(world, str(replace_race["path"].parent))
        ancestor = str(replace_race["path"].parent.parent)
        try:
            plan, sha = race_plan("replace plan", roots["replace_race"])
            before = prior.tree_snapshot(roots["replace_race"])
            child = armed_real_run(world, plan, sha, {DELAY_VAR: roots["replace_race"]})
            try:
                await_firing(
                    proof.evidence / DELAY_MARKER,
                    roots["replace_race"],
                    child,
                    "replace race",
                )
                os.rename(ancestor, ancestor + "-away")
                os.mkdir(ancestor, 0o700)
            finally:
                code, out, err, reason = settle_child(
                    world, child, proof, "replace race", adapter.unclosed
                )
            if reason is not None:
                raise prior.Aborted(f"the replace race left custody unknown: {reason}")
            require(code != 0, f"a replaced ancestor was pruned: {out}{err}")
            require(WITNESS_REFUSAL in err, f"the witness comparison did not refuse: {err}")
            require(prior.tree_snapshot(roots["replace_race"]) == before, "the refused root changed")
            require(
                not world.intents() and not world.quarantines(),
                "the replace race left an intent",
            )
        finally:
            # Undo the replacement and the vanished child exactly, so no later
            # batch inherits an unprovable path from this case. The replacement
            # only exists when the mutation ran, and nothing is touched while
            # custody is unknown.
            def restore_replace():
                if os.path.lexists(ancestor + "-away"):
                    os.rmdir(ancestor)
                    os.rename(ancestor + "-away", ancestor)
                require(os.path.isdir(ancestor), "the replaced ancestor was not restored")
                os.rename(child_away, str(replace_race["path"].parent))

            restore_fixture(proof, "replace race", restore_replace)

    case("witness-races", witness_races)

    def competing_owner_and_mismatch():
        # A positively absent missing-chain candidate that must survive every
        # control below unchanged.
        vanished(world, str(owned_chain["token"]))
        plan, sha, doc = world.plan_file("candidate plan", boot)
        require(
            world.candidates(doc) == {roots["owned_chain"]},
            f"the control fixture is not one candidate: {doc['candidates']}",
        )
        root_before = prior.tree_snapshot(roots["owned_chain"])
        registry = Path(world.registry)
        registry_saved = snapshot_file(registry)

        def no_effect(label):
            require(
                os.path.isdir(roots["owned_chain"]),
                f"{label}: the candidate root was deleted",
            )
            require(
                prior.tree_snapshot(roots["owned_chain"]) == root_before,
                f"{label}: the candidate root changed",
            )
            require(
                not world.intents() and not world.quarantines(),
                f"{label}: a stale intent or quarantine appeared",
            )
            tombstones = os.path.join(world.outputs, "tombstones.json")
            if os.path.exists(tombstones):
                entries = permanent.read_json(tombstones)["entries"]
                require(
                    roots["owned_chain"] not in {entry["root"] for entry in entries},
                    f"{label}: a tombstone names the candidate root",
                )

        # (a) Another checkout's live reference names the same root: ONE per-root
        # keep in a COMPLETE snapshot, never a batch-wide verdict.
        candidate_row = next(
            row
            for row in world.registry_doc()["rows"]
            if row["root"] == roots["owned_chain"] and row["member"] == "debug"
        )
        competitor, competitor_receipt = write_competitor(
            world, world.control, candidate_row, roots["owned_chain"]
        )
        try:
            document = json.loads(registry_saved[0])
            document["rows"].append(competitor)
            document["revision"] = int(document["revision"]) + 1
            registry.write_text(json.dumps(document))
            os.chmod(registry, registry_saved[1])
            _, out, _ = world.dry_run("competing owner", boot)
            doc = json.loads(out)
            require_root_kept(doc, roots["owned_chain"], COMPETING_OWNER)
            require(not doc["candidates"], f"a competing owner left a candidate: {doc['candidates']}")
            no_effect("competing owner")
        finally:
            # Registry and receipt restores are fixture restores too: no write is
            # made while this run's writer closure is unknown.
            def restore_competitor():
                restore_file(registry, registry_saved)
                os.unlink(competitor_receipt)

            restore_fixture(proof, "competing owner", restore_competitor)

        # (b) A mismatched GROUP identity is incomplete authority: the whole
        # batch refuses and nothing is written.
        try:
            document = json.loads(registry_saved[0])
            release = next(
                row
                for row in document["rows"]
                if row["root"] == roots["kept_group"] and row["member"] == "release"
            )
            release["nonce"] = "0" * 32
            document["revision"] = int(document["revision"]) + 1
            registry.write_text(json.dumps(document))
            os.chmod(registry, registry_saved[1])
            _, out, _ = world.dry_run("group mismatch", boot)
            require_batch_refusal(json.loads(out), GROUP_DISAGREEMENT)
            no_effect("group mismatch")
        finally:
            restore_fixture(
                proof, "group mismatch", lambda: restore_file(registry, registry_saved)
            )

        # (c) A mismatched RECEIPT is incomplete authority too.
        receipt = Path(permanent.receipt_file(world.pool, owned_chain["checkout"]))
        receipt_saved = snapshot_file(receipt)
        try:
            document = json.loads(receipt_saved[0])
            document["links"][0]["selected"]["generation"] += 1
            receipt.write_text(json.dumps(document))
            os.chmod(receipt, receipt_saved[1])
            _, out, _ = world.dry_run("receipt mismatch", boot)
            require_batch_refusal(json.loads(out), RECEIPT_MISMATCH)
            no_effect("receipt mismatch")
        finally:
            restore_fixture(
                proof, "receipt mismatch", lambda: restore_file(receipt, receipt_saved)
            )

        # The controls leave the candidate exactly as they found it.
        _, out, _ = world.dry_run("after the controls", boot)
        require(
            world.candidates(json.loads(out)) == {roots["owned_chain"]},
            "the candidate did not survive its own controls",
        )
        no_effect("after the controls")

    case("competing-owner-and-mismatch", competing_owner_and_mismatch)
    require(
        not adapter.unclosed,
        f"a proof child was never reaped: {sorted(adapter.unclosed)}",
    )
    sweep_unconfirmed(proof)
    return cases


class FakeCustodyEffects:
    """In-memory custody effects for the pre-detach counterfactuals.

    It records the ORDER of the censuses, the detach and the backing removal, and
    it can inject an EBUSY detach, a lingering device or a failing removal. No
    file, image or process is touched.
    """

    def __init__(self, entities, volume, detach_code=0, lingering=(), remove_error=None):
        self.entities = entities
        self.volume = volume
        self.detach_code = detach_code
        self.lingering = set(lingering)
        self.remove_error = remove_error
        self.records = {}
        self.events = []
        self.detached = []
        self.removed = []

    def stat(self, path):
        return SimpleNamespace(
            st_mode=stat.S_IFREG | 0o600,
            st_uid=os.geteuid(),
            st_dev=7,
            st_ino=1,
            st_size=128 * 1024 * 1024,
            st_nlink=1,
        )

    def realpath(self, path):
        return path

    def euid(self):
        return os.geteuid()

    def exists(self, path):
        return path in self.lingering

    def write_record(self, path, text):
        self.records[path] = text

    def image_entities(self, backing):
        return self.entities

    def volume_facts(self, device):
        return self.volume

    def detach_device(self, device):
        self.events.append("detach")
        self.detached.append(device)
        return self.detach_code, ""

    def detach(self, mount):
        raise AssertionError("the legacy mount detach is not used by the pre-detach protocol")

    def remove(self, path):
        self.events.append("remove")
        if self.remove_error is not None:
            raise OSError(self.remove_error)
        self.removed.append(path)


def fake_custody(effects, volume_uuid="UUID-135", whole_disk="/dev/disk9"):
    """One attached ImageCustody over in-memory effects (no allocation, no image)."""
    custody = prior.ImageCustody("Ox135Fake", effects=effects)
    custody.evidence = "/evidence"
    custody.backing = "/evidence/152-image-Ox135Fake.sparseimage"
    custody.record = "/evidence/152-image-Ox135Fake-custody.json"
    custody.attach_document = "/evidence/152-image-Ox135Fake-attach.plist"
    custody.expected_mount = "/Volumes/Ox135Fake"
    custody.mount = "/Volumes/Ox135Fake"
    custody.device = "/dev/disk9s1"
    custody.volume_uuid = volume_uuid
    custody.whole_disk = whole_disk
    custody.state = "attached"
    custody.disposition = None
    return custody


def fake_census(events, extra_after=None):
    """A census that records its order and reports an extra member after a count.

    `extra_after=None` never reports one; `0` reports one on every call; `1`
    reports one only from the second call on (a clean pre-census, a dirty post)."""
    calls = {"n": 0}

    def census(owner, allowed):
        calls["n"] += 1
        events.append("census")
        extra = [4242] if extra_after is not None and calls["n"] > extra_after else []
        return {
            "certain": not extra,
            "members": [owner],
            "extra": extra,
            "reason": None if not extra else "synthetic extra member",
        }

    return census


def detach_counterfactuals() -> None:
    """The four pre-detach counterfactuals, plus `-force` being unreachable."""
    # No `-force` is reachable: the program tuple is detach-only and a device
    # argument is never a flag.
    require("force" not in prior.detach_argv("/dev/disk9s1"), "a force flag is reachable")
    try:
        prior.detach_argv("-force")
    except ValueError:
        pass
    else:
        raise AssertionError("a flag passed as the device was accepted")
    entities = [
        {"mount-point": "/Volumes/Ox135Fake", "dev-entry": "/dev/disk9s1"},
        {"dev-entry": "/dev/disk9"},
    ]
    volume = {"uuid": "UUID-135", "whole_disk": "/dev/disk9"}
    # Extra PRE-detach member: nothing is detached and the backing is kept.
    effects = FakeCustodyEffects(entities, volume)
    custody = fake_custody(effects)
    disposition = custody.detach_once(
        True,
        prior.DetachCustody(
            os.getpid(), (os.getpid(),), "UUID-135", fake_census(effects.events, extra_after=0)
        ),
    )
    require(disposition == "refused:census", f"an extra pre member was not refused: {disposition}")
    require(not effects.detached and not effects.removed, "an extra pre member still detached")
    require(effects.events == ["census"], f"the pre-census was not the only effect: {effects.events}")
    # Extra POST-detach member: detached once, backing kept, no retry.
    effects = FakeCustodyEffects(entities, volume)
    custody = fake_custody(effects)
    disposition = custody.detach_once(
        True,
        prior.DetachCustody(
            os.getpid(), (os.getpid(),), "UUID-135", fake_census(effects.events, extra_after=1)
        ),
    )
    require(
        disposition == "retained:post-census",
        f"an extra post member did not retain the backing: {disposition}",
    )
    require(len(effects.detached) == 1, "the detach was replayed")
    require(not effects.removed, "the backing was removed despite an extra post member")
    require(effects.events == ["census", "detach", "census"], f"unexpected order: {effects.events}")
    # Injected EBUSY: retained, never retried, never forced.
    effects = FakeCustodyEffects(entities, volume, detach_code=16)
    custody = fake_custody(effects)
    disposition = custody.detach_once(
        True,
        prior.DetachCustody(os.getpid(), (os.getpid(),), "UUID-135", fake_census(effects.events)),
    )
    require(
        disposition == "retained:detach-failed",
        f"an EBUSY detach was not retained: {disposition}",
    )
    require(len(effects.detached) == 1 and not effects.removed, "an EBUSY detach retried or removed")
    # Success removes the backing only AFTER the pre-census, the detach, the
    # absence check and the post-census.
    effects = FakeCustodyEffects(entities, volume)
    custody = fake_custody(effects)
    disposition = custody.detach_once(
        True,
        prior.DetachCustody(os.getpid(), (os.getpid(),), "UUID-135", fake_census(effects.events)),
    )
    require(disposition == "completed", f"a clean detach did not complete: {disposition}")
    require(
        effects.events == ["census", "detach", "census", "remove"],
        f"the backing was not removed after both censuses: {effects.events}",
    )
    require(effects.removed == [custody.backing], "the backing was not the removed object")
    require(
        '"removed-after-clean-pre/post-census-and-ordinary-detach"'
        in effects.records[custody.record],
        "the success record does not carry the two-census removal marker",
    )
    # A lingering device after a successful detach retains the backing.
    effects = FakeCustodyEffects(entities, volume, lingering={"/dev/disk9s1"})
    custody = fake_custody(effects)
    disposition = custody.detach_once(
        True,
        prior.DetachCustody(os.getpid(), (os.getpid(),), "UUID-135", fake_census(effects.events)),
    )
    require(
        disposition == "retained:lingering",
        f"a lingering device did not retain the backing: {disposition}",
    )
    require(not effects.removed, "the backing was removed with a lingering device")
    # A wrong volume UUID refuses before any detach.
    effects = FakeCustodyEffects(entities, volume)
    custody = fake_custody(effects)
    disposition = custody.detach_once(
        True,
        prior.DetachCustody(os.getpid(), (os.getpid(),), "OTHER", fake_census(effects.events)),
    )
    require(disposition == "refused:unbound", f"a wrong volume UUID was not refused: {disposition}")
    require(not effects.detached and effects.events == [], "a wrong volume UUID still detached")


def source_structure_problems(source: str) -> list:
    """Static problems in THIS script's own source, checked without an effect.

    The reviewed defects must not be able to come back silently: every
    `case(...)` dispatch must match the published case table exactly and in
    order, and every helper whose value is unpacked must be unpacked into the
    number of values it returns. It parses its own source and writes nothing.
    """
    import ast as _ast

    problems = []
    tree = _ast.parse(source)
    dispatched = []
    for node in _ast.walk(tree):
        if (
            isinstance(node, _ast.Call)
            and isinstance(node.func, _ast.Name)
            and node.func.id == "case"
            and node.args
            and isinstance(node.args[0], _ast.Constant)
            and isinstance(node.args[0].value, str)
        ):
            dispatched.append(node.args[0].value)
        if (
            isinstance(node, _ast.Assign)
            and isinstance(node.value, _ast.Call)
            and isinstance(node.value.func, _ast.Name)
        ):
            arity = {"write_competitor": 2, "settle_child": 4}.get(node.value.func.id)
            if arity is None:
                continue
            targets = node.targets
            if (
                len(targets) != 1
                or not isinstance(targets[0], _ast.Tuple)
                or len(targets[0].elts) != arity
            ):
                problems.append(
                    f"{node.value.func.id} is not unpacked into its {arity} values"
                )
    if dispatched != list(CASE_NAMES):
        problems.append(f"the dispatcher does not match the case table: {dispatched}")
    # A classified child fault must stop the run before its next fixture effect:
    # the adapter that runs every ordinary child has to call the settled check.
    adapter_run = [
        node
        for node in _ast.walk(tree)
        if isinstance(node, _ast.ClassDef)
        and node.name == "Adapter"
        for node in node.body
        if isinstance(node, _ast.FunctionDef) and node.name == "run"
    ]
    if not adapter_run:
        problems.append("the adapter's child runner is missing")
    elif not any(
        isinstance(call, _ast.Call)
        and isinstance(call.func, _ast.Name)
        and call.func.id == "require_settled"
        for call in _ast.walk(adapter_run[0])
    ):
        problems.append("a classified child fault could return to a later fixture effect")
    return problems


def self_check() -> int:
    """Pure apparatus check: no child, no scratch, no pool, no image.

    It drives the real oracles, the branch classifier, the per-root keep versus
    batch-refusal distinction, the closure decision and the case/allocation table
    with in-memory counterfactuals, so a fixture that names the wrong refusal,
    the wrong branch or the wrong custody outcome cannot pass.
    """
    require(
        sum(item[1] for item in ALLOCATION_SECONDS) == TOTAL_SECONDS,
        "the prospective time allocation does not sum to the published limit",
    )
    require(
        sum(item[1] for item in ALLOCATION_MIB) == TOTAL_MIB,
        "the prospective scratch allocation does not sum to the published budget",
    )
    allocated = {name for name, _ in ALLOCATION_SECONDS}
    require(all(name in allocated for name in CASE_NAMES), "a case has no time allocation")
    require(all(seconds > 0 for _, seconds in ALLOCATION_SECONDS), "a phase has no time")
    require(all(mib > 0 for _, mib in ALLOCATION_MIB), "a phase has no scratch")
    require(set(CASE_NEEDS) == set(CASE_NAMES), "a case has no dependency entry")
    require(set(CASE_TAINTS) == set(CASE_NAMES), "a case has no taint entry")
    require(
        {name for name, taints in CASE_TAINTS.items() if not taints}
        == {"symlink-and-nondir", "eacces"},
        "only the two read-only cases may be treated as recoverable",
    )
    # This file's own structure: the case table and every unpacked helper.
    structure = source_structure_problems(Path(__file__).read_text())
    require(not structure, f"the proof's own structure is inconsistent: {structure}")
    # The pre-detach protocol's counterfactuals (injectable effects only).
    detach_counterfactuals()
    # A different incomplete-authority reason never satisfies an oracle, even
    # though it also keeps the whole batch.
    for expected in (
        LEAF_ONLY_REFUSAL,
        NO_RECORDED_VOLUME,
        NOT_A_DIRECTORY,
        UNKNOWN_CUSTODY,
        GROUP_DISAGREEMENT,
        RECEIPT_MISMATCH,
    ):
        doc = {
            "coverage": {"complete": False},
            "candidates": [],
            "kept": [
                {"reason": "the registry snapshot is incomplete (unrelated); the whole batch is kept"}
            ],
        }
        try:
            require_batch_refusal(doc, expected)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"an unrelated fault satisfied {expected}")
        doc["kept"][0][
            "reason"
        ] = f"the registry snapshot is incomplete ({expected}); the whole batch is kept"
        require_batch_refusal(doc, expected)
    for doc in (
        {"coverage": {"complete": True}, "candidates": [], "kept": [{"reason": LEAF_ONLY_REFUSAL}]},
        {
            "coverage": {"complete": False},
            "candidates": [{"root": "/x"}],
            "kept": [{"reason": LEAF_ONLY_REFUSAL}],
        },
        {"coverage": {"complete": False}, "candidates": [], "kept": [{"reason": "kept"}]},
    ):
        try:
            require_batch_refusal(doc, LEAF_ONLY_REFUSAL)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"a non-refusing batch passed: {doc}")
    # Per-root keep versus incomplete authority: the SAME document must satisfy
    # exactly one of the two oracles.
    root = "/vol/oxide-outputs/app"
    per_root = {
        "coverage": {"complete": True},
        "candidates": [{"root": "/vol/oxide-outputs/other"}],
        "kept": [{"root": root, "reason": f"checkout absence is unknown ({UNREADABLE}), so its root is kept"}],
    }
    require_root_kept(per_root, root, UNREADABLE)
    require_root_kept(per_root, root, ABSENCE_UNKNOWN)
    try:
        require_batch_refusal(per_root, UNREADABLE)
    except RuntimeError:
        pass
    else:
        raise AssertionError("a complete per-root keep satisfied the whole-batch oracle")
    try:
        require_root_kept(
            {
                "coverage": {"complete": False},
                "candidates": [],
                "kept": [{"root": root, "reason": UNREADABLE}],
            },
            root,
            UNREADABLE,
        )
    except RuntimeError:
        pass
    else:
        raise AssertionError("an incomplete snapshot satisfied the per-root oracle")
    try:
        require_root_kept(
            {
                "coverage": {"complete": True},
                "candidates": [{"root": root}],
                "kept": [{"root": root, "reason": UNREADABLE}],
            },
            root,
            UNREADABLE,
        )
    except RuntimeError:
        pass
    else:
        raise AssertionError("a candidate satisfied the per-root keep oracle")
    try:
        require_root_kept(
            {
                "coverage": {"complete": True},
                "candidates": [],
                "kept": [{"root": "/other", "reason": UNREADABLE}],
            },
            root,
            UNREADABLE,
        )
    except RuntimeError:
        pass
    else:
        raise AssertionError("another root's keep satisfied this root's oracle")
    # The custody decision: only a reaped, normal-exit child with an empty
    # session is positively closed.
    require(closure_unknown(0, False, True, set()) is None, "a clean exit was not closed")
    require(closure_unknown(2, False, True, set()) is None, "a normal refusal was not closed")
    require(closure_unknown(-9, False, True, set()) is not None, "a signalled child was closed")
    require(closure_unknown(None, False, True, set()) is not None, "a missing status was closed")
    require(closure_unknown(0, True, True, set()) is not None, "a killed child was closed")
    require(closure_unknown(0, False, False, set()) is not None, "an unreaped child was closed")
    require(closure_unknown(0, False, True, {7}) is not None, "an unclosed session was closed")
    # A latch is idempotent and keeps the FIRST reason.
    class FakeProof:
        closure_known = True
        custody_unknown = None
        unclosed: set = set()

    fake = FakeProof()
    require(custody_abort_reason(fake, set()) is None, "a clean proof could not continue")
    latch_unknown(fake, "first")
    latch_unknown(fake, "second")
    require(fake.custody_unknown == "first" and fake.closure_known is False, "the latch moved")
    require(custody_abort_reason(fake, set()) == "first", "the latch did not gate continuation")
    # The shared child helper's own unconfirmed closure is classified the same
    # way, with no latch of its own.
    unconfirmed = FakeProof()
    unconfirmed.closure_known = False
    require(
        custody_abort_reason(unconfirmed, set()) is not None,
        "unconfirmed helper closure could neither continue nor refuse the detach",
    )
    # A normal, reaped child whose session still holds a pid is not closed.
    require(
        custody_abort_reason(FakeProof(), {11}) is not None,
        "an un-reaped session was read as closed",
    )
    # EVERY child's abnormal outcome is classified: a signalled exit and each of
    # the product's own unconfirmed-closure sentences are not closed writers.
    require(abnormal_child(0, "") is None, "a clean child was classified abnormal")
    require(abnormal_child(2, "kept\n") is None, "a normal refusal was classified abnormal")
    require(abnormal_child(-15, "") is not None, "a signalled child was read as closed")
    for marker in permanent.UNCONFIRMED_CUSTODY_MARKERS:
        require(
            abnormal_child(0, f"oxide: {marker} 77\n") is not None,
            f"an unconfirmed closure was read as closed: {marker!r}",
        )
    # A fixture restore is REFUSED while custody is unknown, runs once when it is
    # known, and latches when it fails.
    ran = []
    held = FakeProof()
    latch_unknown(held, "held")
    try:
        restore_fixture(held, "synthetic", lambda: ran.append(1))
    except prior.Aborted:
        pass
    else:
        raise AssertionError("a fixture restore ran under unknown custody")
    require(not ran, "the fixture was mutated under unknown custody")
    restore_fixture(FakeProof(), "synthetic", lambda: ran.append(1))
    require(ran == [1], "a clean restore did not run exactly once")
    broken = FakeProof()

    def failing():
        raise RuntimeError("synthetic restore failure")

    try:
        restore_fixture(broken, "synthetic", failing)
    except RuntimeError:
        pass
    else:
        raise AssertionError("a failing restore did not propagate")
    require(broken.custody_unknown is not None, "a failing restore did not latch")
    # Setup stops at the first latched fault instead of attaching an image or
    # writing a fixture beside an unclassified writer group.
    require_settled(FakeProof(), "synthetic setup")
    try:
        require_settled(held, "synthetic setup")
    except prior.Aborted:
        pass
    else:
        raise AssertionError("setup continued after a latched custody fault")
    try:
        require_settled(broken, "synthetic setup")
    except prior.Aborted:
        pass
    else:
        raise AssertionError("setup continued after a failed restore latched")
    # Both /Volumes spellings are mounts, including an absent mount name; the
    # mount root itself and every non-normal spelling are invalid, and a plain
    # path is not a mount.
    for spelling, branch in (
        ("/Volumes/Ox135Absent/deep/checkout", "mount"),
        ("/System/Volumes/Data/Volumes/Ox135Absent/deep/checkout", "mount"),
        ("/Volumes/Ox135Absent", "invalid"),
        ("/System/Volumes/Data/Volumes/Ox135Absent", "invalid"),
        ("/Volumes", "invalid"),
        ("/private/tmp/chains/token/a/checkout", "root"),
        ("/System/Volumes/Data/x", "root"),
        ("relative/checkout", "invalid"),
        ("/private//tmp/x", "invalid"),
        ("/private/tmp/x/", "invalid"),
        ("/private/./tmp/x", "invalid"),
        ("/private/tmp/../tmp/x", "invalid"),
    ):
        require(mount_branch(spelling) == branch, f"{spelling} was not classified {branch}")
    # The two races must be distinguishable, and the fixture seams must not alias.
    require(
        WITNESS_REFUSAL != PRESENT_KEEP and WITNESS_REFUSAL not in PRESENT_KEEP,
        "the two race refusals must be distinguishable sentences",
    )
    require(EACCES_VAR != DELAY_VAR and EACCES_MARKER != DELAY_MARKER, "the seams alias")
    require(CUSTODY_VAR not in (EACCES_VAR, DELAY_VAR), "the custody seam aliases")
    require(
        GROUP_DISAGREEMENT != RECEIPT_MISMATCH and COMPETING_OWNER != GROUP_DISAGREEMENT,
        "the mismatch oracles must be distinguishable",
    )
    print("prune absence self-check: OK")
    return 0


def main():
    if sys.argv[1:] == ["--self-check"]:
        return self_check()
    diagnostic = sys.argv[1:] == ["--diagnostic"]
    if sys.argv[1:] not in ([], ["--diagnostic"]):
        raise SystemExit("expected no argument or --diagnostic")
    proof = wrapper.Proof()
    proof.custody_unknown = None
    proof.unclosed = set()
    require(wrapper.git(proof, "status", "--porcelain") == "", "source is not clean")
    custody = prior.ImageCustody("Ox135" + secrets.token_hex(4))
    cases, error = [], None
    try:
        run_cases(proof, custody, diagnostic, cases)
    except BaseException as exc:
        error = exc
        traceback.print_exc()
    finally:
        # The detach needs this run's own bookkeeping to be positively settled:
        # one decision shared with the diagnostic's continuation check, so a
        # killed, signalled or unreaped child, an unconfirmed closure report or
        # any recorded fault is classified the same way before both. This is
        # never a process census, and it is decided before the supervisor's
        # post-leader census can run: that census is authoritative for what the
        # run reports, not a retention this script can rely on. Nothing here
        # claims otherwise.
        own_children = custody_abort_reason(proof, proof.unclosed)
        # The pre-detach protocol: rebind the image, census THIS proof's own
        # session, detach the bound device ordinarily (never -force), verify the
        # mount/device are gone, census again, and only then remove the backing.
        detach_custody = prior.DetachCustody(
            owner_pid=os.getpid(),
            allowed_pids=(os.getpid(),),
            volume_uuid=getattr(proof, "expected_volume_uuid", "") or "",
        )
        disposition = custody.detach_once(
            sessions_closed=own_children is None, custody=detach_custody
        )
    recorded = {item["case"] for item in cases}
    cases.extend(
        {"case": name, "state": "unexecuted"} for name in CASE_NAMES if name not in recorded
    )
    report = {
        "qualified": False if diagnostic else None,
        "cases": cases,
        "failed": sorted(item["case"] for item in cases if item["state"] == "failed"),
        "own_children": own_children
        or (
            "settled from this script's own direct children before the supervisor's "
            "post-leader census; that census is authoritative for the reported closure"
        ),
        "error": None if error is None else f"{type(error).__name__}: {error}",
        "custody": custody.state,
        "detach": disposition,
    }
    if diagnostic:
        (proof.evidence / "prune135-diagnostic.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2), flush=True)
    if error is not None:
        raise error
    require(disposition == "completed", f"the ordinary detach was {disposition}")
    recorded = permanent.read_json(custody.record)
    require(
        recorded.get("census") == "pre-clean,post-clean"
        and str(recorded.get("backing", "")).startswith(
            "removed-after-clean-pre/post-census-and-ordinary-detach"
        ),
        f"the recorded detach disposition is not the two-census one: {recorded}",
    )
    require(own_children is None, f"this run's own child closure is unknown: {own_children}")
    require(all(case["state"] == "completed" for case in cases), "proof incomplete")


if __name__ == "__main__":
    main()
