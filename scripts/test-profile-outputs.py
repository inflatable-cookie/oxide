#!/usr/bin/env python3
"""Owned output links at profile directories (oxide#133): the fixed native
matrix.

The qualified shared supervisor (`scripts/bounded-test.py`, SHA-256
`961164e797c01432fa8234497627f75e56489ef6395ef66e48824cdb0e48c052`, the
merged oxide#142 monitor qualified at main `791f552`) owns the deadline,
512 MiB monitored scratch, continuous host reserve sampling, process
census/closure and cleanup. The
brief's scoped amendment (root owner, 2026-10-23T16:10Z, under the temporary
Supervisor delegation) makes this selector/script/plan 600 s TOTAL instead of
420; the old 420 publication at 030836f6 stays historical and unactivated. The
helper's work cutoff is about 565 s (its 30 s reserve plus a 5 s refusal
margin), inside which cleanup and image detach must finish. The published
360-540 s work estimate is UNMEASURED, not an established fit:

    python3 scripts/bounded-test.py --limit 600 --scratch-budget-mib 512 \\
        -- python3 scripts/test-profile-outputs.py

This script is a whole fixed script with no supervision, clock, signal or
scratch of its own. Every write stays inside the supervisor-owned scratch
(`OXIDE_TASK_SCRATCH_ROOT`, also `TMPDIR`/`TMP`/`TEMP`); evidence goes to
`OXIDE_PROOF_EVIDENCE`. It models the removable drive as ONE owned 128 MiB
APFS sparse image at a unique `/Volumes/<name>`, attached with
`hdiutil attach -owners on` on EVERY attach and reattach, and uses one private
`HOME`, `CARGO_HOME`, directory pool and fixture tree per session. The fixture
prerequisite is published and asserted twice: a bounded read-only `/sbin/mount`
observation that the mounted root does not carry `noowners`, and the product's
own `pool register-removable` refusal for an ignore-ownership mount. This is a
private-image fixture option only; no real drive, persistent host setting,
mount permission or product safety check is changed.

The image is 128 MiB and every normal registration reserves 16 MiB below that
geometry with a 64 MiB admission cap, so an external root is really selected; a
positive control asserts the selected root is under
`<mount>/oxide-outputs/` with no fallback warning before any external-root
oracle. The fixture `.gitignore` uses `/target` (valid for a whole-target link
and as a member parent rule) and a separate `/target/` parent-ignore control
proves a directory-only rule refuses the legacy link but accepts profile member
links. The top-level preservation control lives on its own checkout so the
cleaned checkout carries no foreign entry. The selected-clean reserve stop uses
a positively created directory-budget pool and a gated delegating fake cargo;
the removable watchdog is removal-only and is never claimed as a reserve stop.
A SEPARATE deliberate low-reserve control temporarily raises this
private pool's recorded registration reserve to 200 MiB (above the image's free
space), asserts the honest checkout-local fallback and its warning, then
restores the exact recorded bytes; no second image or pool is created, so
fallback can never masquerade as external placement. Top-level preservation is nonvacuous: a benign ordinary
file created at the real target root before adoption must still exist, be a
real regular file and be unchanged after adoption and builds, while Cargo's own
`CACHEDIR.TAG`/`.rustc_info.json` are asserted real only when present.

CPU controls for a 6 CPU / 8 GiB planning request: the two independent
stagings are SEQUENTIAL and each is explicitly bounded to 6 Cargo jobs; fixture
Cargo children are bounded to 2 jobs each and at most two gated writers overlap
(2 x 2 = 4), so the whole proof peaks at 6 concurrent Cargo jobs. No inherited
CARGO_BUILD_JOBS is trusted: the shared staging helper sanitizes it first and
takes an explicit optional job bound (its default stays unchanged for other
selectors).

Staging (an unmeasured estimate, not a gate): two independent configs from
authenticated pinned source, this HEAD's candidate (~45-80 s at 6 jobs) and
base `4a70b83` (~45-80 s at 6 jobs); each target is copied out and discarded
before the next staging starts, so only one staging target exists at a time.
The script discards each staging target after copying
its binary into the owned `bin/` directory so both never exceed the allocation:
288 MiB staging peak + 128 MiB image backing + 48 MiB fixtures + 32 MiB
evidence + 16 MiB margin = 512 MiB monitored.

The 20 cases (see `--plan`):

 1 real target with a debug member link; Cargo top-level files stay real
 2 a member mutation waits unchanged for a gated Running writer
 3 test / bench / bench --profile debug / doc actual member mapping
 4 a host --target member (`<triple>/debug`)
 5 unknown-target linked refusal versus an unlinked checkout that still runs
 6 opaque custom-profile linked refusal and the unchanged modelled custom refusal
 7 complete exec declaration, undeclared refusal and custom declaration refusal
 8 real nonempty member kept and an empty one converted by explicit adopt
 9 container-style preflight sees a real `target`
10 legacy whole-target receipt, link and `outputs-2` key unchanged and warm under state 4
11 explicit base-created migration, source-shaped root/1 fixture (no nonce in receipt/registry), shared nonce upgrade, unchanged root inode/payload
12 P0-P4 replay, creator/parent identity races, exact pre-P4 rollback at both marker boundaries
13 an active writer blocks migration without effects
14 per-member fallback/return with custody
15 detach and re-adopt
16 whole and selected clean with the existing watchdog semantics
17 malformed member, unknown custody, internal fallback versus external link refusals
18 linked-versus-real placement changes yield distinct cold slots
19 an independently staged state-3 binary refuses state 4 without effects
20 root-group prune with another checkout reference KEEP, exact guarded rollback and terminal crash image oracles

`--diagnostic` is the ONE opt-in diagnostic mode (decision 1e4feb57): the same
fixed 20 cases in one session, where an oracle failure is recorded and the run
continues to the next case whose prerequisites still hold, and a step whose
dependency failed or whose shared fixture is no longer usable is skipped with
its reason. It never steps over a safety failure — the runner's deadline,
child closure, image/device/writer custody or an unrecordable receipt still
stop it at once — and it writes ONE uniquely named
`profile-outputs.diagnostic-*.json` that is distinct from the guard receipt and
from the counted run's own assertion receipt, explicitly `diagnostic` and never
a PASS, closure, census or qualified native claim. Without the argument the
counted fail-fast behaviour is unchanged: the first failure raises, and the
default assertion receipt is the only one written.

Not a benchmark, not real-project qualification and not full QA. The script
publishes assertions only; it does not run a native session by itself, and the
merge hold requires separately counted author and reviewer activations.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util
import json
import os
import re
import secrets
import shutil
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "profile_perm", os.path.join(HERE, "test-permanent-outputs.py")
)
perm = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(perm)
_spec = importlib.util.spec_from_file_location(
    "profile_layout", os.path.join(HERE, "test-oxide-pool-layout.py")
)
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)

Proof = perm.Proof
summary, MIB = perm.summary, perm.MIB


class OracleFailure(RuntimeError):
    """This script's own assertion or oracle failed.

    It is deliberately distinguishable from the infrastructure failures that a
    diagnostic run must never step over: interruption, an OS/subprocess error,
    an unknown child or attachment, the shared runner's deadline or an
    unrecordable receipt all stay plain RuntimeError/OSError/etc."""


def fail(message: str) -> None:
    raise OracleFailure(message)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)

BUILDER = "1.97.1"
BASE = "4a70b83"
# Explicit CPU controls (planning request 6 CPU / 8 GiB): each independent
# staging is sequential with at most 6 Cargo jobs; fixture children are
# bounded to 2 jobs each and at most two gated writers overlap, so no moment
# exceeds 6 concurrent Cargo jobs.
STAGE_JOBS = 6
FIXTURE_JOBS = 2
# The private image is 128 MiB. Every normal registration reserves well below
# that geometry (16 MiB) with a 64 MiB admission cap, so the volume is really
# selected; one SEPARATE deliberate low-reserve control temporarily raises THIS
# registration's recorded reserve above the image's free space and restores the
# exact recorded bytes afterwards, proving the honest internal fallback.
IMAGE_RESERVE = "16MiB"
LOW_RESERVE_MIB = 200
SHUTDOWN_MARGIN_S = 30
MARKER = ".oxide-output.json"
CAPABILITIES = "__oxide-build-capabilities"
STATE3_FENCE = "oxide.outputs/2"
STATE4_FENCE = "oxide.outputs/3"

CASES = [
    "01-real-target-debug-link",
    "02-member-mutation-waits",
    "03-test-bench-doc-mapping",
    "04-host-target-member",
    "05-unknown-target",
    "06-custom-profile-refusal",
    "07-exec-declarations",
    "08-real-nonempty-kept",
    "09-container-preflight",
    "10-legacy-key-warm",
    "11-migration-root1",
    "12-migration-replay-rollback",
    "13-writer-blocks-migration",
    "14-per-member-fallback",
    "15-detach-readopt",
    "16-clean",
    "17-refusals",
    "18-cold-slot-placement",
    "19-state3-refusal",
    "20-prune-group",
]

APP = {
    "Cargo.toml": (
        '[package]\nname = "profileapp"\nversion = "0.1.0"\nedition = "2021"\n'
        'build = "build.rs"\n'
    ),
    "build.rs": r'''use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OXIDE_PROFILE_GATE");
    let Ok(gate) = std::env::var("OXIDE_PROFILE_GATE") else {
        return;
    };
    let gate = PathBuf::from(gate);
    fs::create_dir_all(&gate).unwrap();
    fs::write(gate.join(format!("started-{}", std::process::id())), b"started").unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !gate.join("release").exists() {
        assert!(Instant::now() < deadline, "proof did not release the build");
        std::thread::sleep(Duration::from_millis(25));
    }
}
''',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    "src/main.rs": 'fn main() { println!("profile-app {} {}", profileapp::value(), std::env::current_exe().unwrap().display()); }\n',
    # The whole-target link conversion needs the exact non-directory path;
    # `/target` also ignores profile members under the real target directory.
    ".gitignore": "/target\n/.oxide-outputs/\n",
}
# A separate fixture whose parent rule is directory-only: it must refuse a
# whole-target link but accept a profile member link.
PARENT_IGNORE_APP = {**APP, ".gitignore": "/target/\n/.oxide-outputs/\n"}


def plan_only() -> int:
    print("profile-outputs proof allocation (unmeasured staging estimate)")
    print(f"  script          {os.path.abspath(__file__)}")
    print("  supervisor      scripts/bounded-test.py --limit 600 --scratch-budget-mib 512")
    print(f"  deadline        600 s total (scoped brief amendment); helper work cutoff ~565 s")
    print("                  = 30 s helper reserve + 5 s refusal margin; cleanup/detach inside it")
    print("  fit             UNMEASURED: 360-540 s estimated work inside the ~565 s cutoff;")
    print("                  no native, trial or setup-only run has been activated, so the")
    print("                  estimate is not an established fit")
    print("  cpu             planning request 6 CPU / 8 GiB: two SEQUENTIAL stagings at")
    print("                  STAGE_JOBS=6 each, fixture children at FIXTURE_JOBS=2 with at")
    print("                  most two gated writers (2 x 2 = 4); peak 6 Cargo jobs")
    print("  time estimate   two sequential 6-job stagings ~45-80 s each, 25 s setup,")
    print("                  150-240 s cases (base control, p0/parent races, unknown")
    print("                  custody, prune oracles), plus the controls added in this phase:")
    print("                  a seeded directory-budget adopt+build for the watched clean,")
    print("                  three extra fixture builds/adopts (mixed run, second eligible")
    print("                  gone root, cleared-target parent-ignore), settled-background")
    print("                  waits and additional dry runs (~60-90 s), 30 s cleanup")
    print("                  -> 360-540 s estimated work; UNMEASURED, not a fit claim")
    print("  history         the 420 s publication at 030836f6 is historical and unactivated")
    print("  amendment       GRANTED scoped bound: 600 s total (root owner 2026-10-23T16:10Z),")
    print("                  512 MiB unchanged; no disk increase, runner change or native start")
    print("  disk            288 MiB staging peak, counted as ONE staging target at a time")
    print("                  (each is copied out and discarded before the next starts; two")
    print("                  live targets are NOT assumed), + 128 MiB image backing +")
    print("                  48 MiB fixtures (three added tiny checkouts) + 32 MiB")
    print("                  evidence + 16 MiB margin = 512 MiB")
    print("                  real target occupancy: one tiny locked checkout per case,")
    print("                  a few MiB each, inside the fixtures allowance")
    print(f"  staging         candidate {os.environ.get('OXIDE_PROFILE_HEAD', 'this HEAD')} and base {BASE}")
    print("  seed            none: both stagings are plain Cargo offline builds from authenticated pinned source")
    print("  base order      both base-created legacy states (case 10 warm slot and case 11")
    print("                  migration) are prepared under ONE base-compatible registration")
    print("                  before ANY candidate command runs in that pool")
    print("  monitoring      supervisor reserve sampling, census/closure and cleanup; this script runs no clock, timer or harness")
    print("  image           one owned 128 MiB APFS sparse image at a unique /Volumes/<name>,")
    print("                  attached with hdiutil -owners on for every attach/reattach,")
    print("                  asserted by a bounded read-only mount observation and the")
    print("                  product registration control; ordinary detach")
    print("  reserves        normal registration 16 MiB reserve / 64 MiB cap below the")
    print("                  128 MiB geometry, with a positive selected-removable control")
    print("                  before external oracles and a separate deliberate 200 MiB")
    print("                  low-reserve control (recorded reserve raised to 200 MiB,")
    print("                  then restored byte-exactly) proving the honest local fallback")
    print("  top level       a benign ordinary target-root file on its OWN checkout")
    print("                  survives adopt and builds, then a clean of THAT checkout must")
    print("                  refuse it as foreign and succeed after removal;")
    print("                  CACHEDIR.TAG/.rustc_info.json asserted real only when present")
    print("  ignore          the fixture uses /target (valid for whole-target links and")
    print("                  member parents) plus a separate /target/ parent-ignore control")
    print("  peer keep       an injected committed peer receipt plus matching registry")
    print("                  rows (SYNTHETIC INJECTED AUTHORITY, not a bind-produced")
    print("                  receipt) keeps that root; a second, distinct eligible gone")
    print("                  root is a candidate in the SAME preview")
    print("  clean           the reserve stop uses a positively created directory-budget")
    print("                  pool with a gated delegating fake cargo; the removable")
    print("                  watchdog is removal-only and is not claimed as a reserve stop")
    print("  environment     one private HOME, CARGO_HOME and directory pool per session")
    print("  diagnostic      ONE opt-in --diagnostic run records every oracle failure,")
    print("                  skips only unmet-dependency steps, still stops on any")
    print("                  safety failure, writes ONE distinct diagnostic receipt,")
    print("                  and is never counted or accepted as a native PASS")
    print("  cases")
    for case in CASES:
        print(f"    {case}")
    return 0


def work_left() -> float:
    return perm.work_left()


def source_at(proof: Proof, repo: str, commit: str, label: str) -> str:
    """Extract one authenticated pinned commit into owned scratch."""
    dest = os.path.join(proof.root, f"src-{label}")
    os.makedirs(dest)
    result = proof.run(
        ["/bin/sh", "-c", f"git -C '{repo}' archive {commit} | tar -x -C '{dest}'"],
        timeout=60,
    )
    expect(result.returncode == 0, f"{label}: cannot extract {commit}: {result.stderr!r}")
    expect(os.path.isfile(os.path.join(dest, "Cargo.toml")), f"{label}: no Cargo.toml")
    return dest


def capabilities(binary: str) -> int:
    result = perm.subprocess.run(
        [binary, CAPABILITIES], capture_output=True, timeout=30
    )
    expect(result.returncode == 0, f"{binary}: capability probe exit {result.returncode}")
    return int(json.loads(result.stdout)["control_state_version"])


def fence_in(pool: str) -> str:
    state = perm.read_json(os.path.join(pool, "oxide-managed", "state.json"))
    fence = state.get("owned_outputs") or ""
    return fence


def link_at(path: str) -> str:
    expect(os.path.islink(path), f"{path} is not a link")
    return os.readlink(path)


def is_real_dir(path: str) -> bool:
    return os.path.isdir(path) and not os.path.islink(path)


def receipt_file(pool: str, checkout: str) -> str:
    """The receipt path keyed by the canonical checkout digest, WITHOUT
    asserting that it exists (used for positive absence checks)."""
    key = perm.hashlib.sha256(checkout.encode()).hexdigest()
    return os.path.join(pool, "oxide-managed", "outputs", key + ".json")


def receipt(pool: str, checkout: str) -> dict:
    return perm.read_json(receipt_file(pool, checkout))


def item(pool: str, checkout: str, key: str):
    return receipt(pool, checkout)[key]


def member_link(checkout: str, member: str) -> str:
    return os.path.join(checkout, "target", member)


def member_destination(root: str, member: str) -> str:
    """The approved distinct destination one linked member selects beneath the
    shared root; only the legacy whole-target placement selects the root."""
    return root if member == "target" else os.path.join(root, member)


def expect_layout(checkout: str, members: dict) -> None:
    """`target` is a real directory and each named member has the expected
    placement ("link:<root>" or "real"). A linked member must select its own
    destination `root/<member>`, never the shared root itself."""
    target = os.path.join(checkout, "target")
    expect(is_real_dir(target), f"{target} is not a real directory")
    for member, expected in members.items():
        path = member_link(checkout, member)
        if expected == "real":
            expect(is_real_dir(path), f"{path} is not a real directory")
        else:
            root = member_destination(expected[len("link:"):], member)
            expect(
                os.path.islink(path) and os.readlink(path) == root,
                f"{path} -> {os.readlink(path) if os.path.islink(path) else 'not a link'}, not {root}",
            )


def root_of(pool: str, checkout: str) -> str:
    links = [
        placement["selected"]["root"]
        for placement in receipt(pool, checkout)["links"]
        if placement.get("selected")
    ]
    expect(links, f"{checkout}: the receipt selects no root")
    expect(len(set(links)) == 1, f"{checkout}: members select different roots: {links}")
    return links[0]


def make_root_one_fixture(pool: str, checkout: str) -> str:
    """Turn a base-style converted checkout into the source shape of a
    checkout created before oxide#129: a root/1 marker with no nonce and no
    nonce anywhere in the receipt or registry. Returns the root path."""
    root = root_of(pool, checkout)
    marker_path = os.path.join(root, MARKER)
    marker = perm.read_json(marker_path)
    marker.pop("nonce", None)
    marker.pop("checkout_volume_uuid", None)
    marker["version"] = "oxide.outputs.root/1"
    os.chmod(marker_path, 0o600)
    with open(marker_path, "w", encoding="utf-8") as handle:
        json.dump(marker, handle, indent=2)
        handle.write("\n")
    path = perm.receipt_path(pool, checkout)
    doc = perm.read_json(path)
    for placement in doc.get("links", []):
        selected = placement.get("selected")
        if selected:
            selected.pop("nonce", None)
    for known in doc.get("roots", []):
        known.pop("nonce", None)
    os.chmod(path, 0o600)
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(doc, handle, indent=2)
        handle.write("\n")
    registry_path = os.path.join(pool, "oxide-managed", "outputs", "registry.json")
    registry = perm.read_json(registry_path)
    checkout_id = perm.hashlib.sha256(checkout.encode()).hexdigest()
    for row in registry["rows"]:
        if row["checkout_id"] == checkout_id:
            row.pop("nonce", None)
    os.chmod(registry_path, 0o600)
    with open(registry_path, "w", encoding="utf-8") as handle:
        json.dump(registry, handle, indent=2)
        handle.write("\n")
    return root


def wait_started(gate: str, label: str) -> None:
    deadline = time.monotonic() + min(90, work_left())
    while not perm.started_markers(gate):
        if time.monotonic() >= deadline:
            fail(f"{label}: no build script started under {gate}")
        time.sleep(0.05)


def make_checkout(proof: Proof, name: str, files: dict | None = None) -> str:
    workspace = os.path.join(proof.root, "checkouts", name)
    os.makedirs(workspace)
    perm.write_tree(workspace, files or APP)
    result = proof.run(
        ["git", "init", "-q"],
        cwd=workspace,
        env={**proof.env, "GIT_CONFIG_NOSYSTEM": "1"},
        timeout=30,
    )
    expect(result.returncode == 0, f"{name}: git init failed")
    proof.run(["git", "add", "-A"], cwd=workspace, env=proof.env, timeout=30)
    proof.run(
        ["git", "-c", "user.name=proof", "-c", "user.email=proof@example.invalid",
         "commit", "-q", "-m", "fixture"],
        cwd=workspace,
        env=proof.env,
        timeout=30,
    )
    return os.path.realpath(workspace)


def owners_honored(proof: Proof, mount: str, label: str) -> None:
    """Bounded read-only observation that the private image's mounted root does
    not carry the ignore-ownership flag. The product registration below is the
    second control: it refuses an ignore-ownership mount."""
    result = proof.run(["/sbin/mount"], timeout=30)
    expect(result.returncode == 0, f"{label}: mount listing failed: {result.stderr!r}")
    line = next(
        (
            entry
            for entry in result.stdout.decode(errors="replace").splitlines()
            if f" on {mount} " in entry
        ),
        None,
    )
    expect(line is not None, f"{label}: {mount} is not in the mount listing")
    expect(
        "noowners" not in line,
        f"{label}: {mount} was mounted with owners ignored (MNT_IGNORE_OWNERSHIP): {line}",
    )


def owned_image_dir() -> str:
    """The private image backing lives in the qualified guard's OWN evidence
    directory, which it keeps, beside (not inside) its monitored scratch. The
    same guard root therefore counts the 128 MiB backing inside the published
    512 MiB allocation, and a retained backing stays monitored for the
    custodian."""
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(evidence) and os.path.isdir(evidence), f"OXIDE_PROOF_EVIDENCE {evidence!r} is not a directory")
    expect(bool(scratch) and os.path.isdir(scratch), f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not a directory")
    common = os.path.commonpath([os.path.realpath(evidence), os.path.dirname(os.path.realpath(scratch))])
    expect(common == os.path.dirname(os.path.realpath(scratch)), "the evidence directory is not beside the guard scratch")
    expect(
        not os.path.realpath(evidence).startswith(os.path.realpath(scratch) + os.sep),
        "the evidence directory is inside the guard scratch",
    )
    directory = os.path.join(evidence, "owned-image")
    os.makedirs(directory, mode=0o700, exist_ok=True)
    os.chmod(directory, 0o700)
    return directory


def evidence_root() -> str:
    """The runner's evidence root, or "" when this script is not under it."""
    return os.environ.get("OXIDE_PROOF_EVIDENCE", "")


def write_attachment_facts(name: str, record: dict, evidence: str | None = None) -> str | None:
    """Persist bounded attachment custody in the guard's OWN evidence root,
    before or after a risky operation, so an interrupted run still leaves the
    last known mount/device/UUID/image facts for the custodian."""
    root = evidence_root() if evidence is None else evidence
    if not root:
        return None
    path = os.path.join(root, name)
    with open(path + ".new", "w", encoding="utf-8") as handle:
        json.dump(record, handle, indent=2, sort_keys=True)
        handle.write("\n")
    os.replace(path + ".new", path)
    return path


class ChildCustody:
    """Script-local ownership of the gated children and gates THIS proof
    created, so a failure path can settle exactly them through the existing
    perm.stop_gate/perm.finish_child helpers within the supervisor's existing
    deadline. It is not a clock, signal controller, census or second harness:
    children it does not own are never touched, and anything it cannot settle
    stays in proof.unclosed and keeps blocking detach."""

    def __init__(self) -> None:
        self._children = {}
        self._finished = set()

    def track(self, child, gate: str | None, label: str) -> None:
        self._children[child.pid] = (child, gate, label)

    def finished(self, child) -> None:
        self._finished.add(child.pid)
        self._children.pop(child.pid, None)

    def pending(self, proof: Proof) -> list:
        unclosed = set(getattr(proof, "unclosed", set()))
        return [pid for pid in sorted(unclosed) if pid in self._children and pid not in self._finished]

    def not_owned(self, proof: Proof) -> list:
        return [
            pid
            for pid in sorted(getattr(proof, "unclosed", set()))
            if pid not in self._children
        ]

    def settle(self, proof: Proof, label: str, finish=None, remaining=None) -> dict:
        """Release this script's gates and finish its own children, bounded by
        the existing deadline. A child that ALREADY finished keeps its own
        recorded status and is never signalled or re-run as new work."""
        finish = finish or perm.finish_child
        remaining = remaining or perm.work_left
        record = {"settled": [], "unresolved": [], "not_owned": self.not_owned(proof)}
        for pid in self.pending(proof):
            child, gate, child_label = self._children[pid]
            if child.poll() is not None:
                # It exited without a recorded helper outcome. Nothing is
                # signalled or re-run as new work, and its status is kept in the
                # record, but closure is NOT claimed from a poll: it stays
                # unclosed and keeps blocking detach.
                record["unresolved"].append(
                    {
                        "pid": pid,
                        "label": child_label,
                        "reason": "exited without a recorded helper outcome; left unclosed",
                        "observed_exit": child.returncode,
                    }
                )
                continue
            try:
                left = remaining()
            except Exception as error:
                record["unresolved"].append({"pid": pid, "label": child_label, "reason": f"no bounded time left ({error})"})
                break
            if left <= 0:
                record["unresolved"].append({"pid": pid, "label": child_label, "reason": "no bounded time left"})
                break
            try:
                if gate:
                    perm.stop_gate(gate, os.path.join(gate, "release"))
                code, _, _ = finish(proof, child, f"{label}: {child_label}")
            except Exception as error:
                record["unresolved"].append({"pid": pid, "label": child_label, "reason": repr(error)})
                continue
            self.finished(child)
            record["settled"].append({"pid": pid, "exit": code, "label": child_label})
        record["still_unclosed"] = sorted(getattr(proof, "unclosed", set()))
        return record


CUSTODY = ChildCustody()


def known_attachment_facts(proof: Proof, label: str, state: str) -> dict:
    """Source-KNOWN custody facts with NO subprocess.

    Everything here is already in hand (the proof's mount, the owned backing
    path, the unclosed-child set and the last cached query). Nothing is
    queried, so a failure handler can always persist this record even when
    diskutil would hang, time out or fail."""
    mount = proof.mount
    # A record must be writable even when the owned paths cannot be derived:
    # unknown stays honestly unknown instead of raising inside a failure path.
    try:
        directory = owned_image_dir()
        backing = os.path.join(directory, "volume.sparseimage")
        backing_present = os.path.exists(backing)
    except Exception:
        directory = ""
        backing = ""
        backing_present = False
    cached = getattr(proof, "_attachment_details", None)
    return {
        "label": label,
        "state": state,
        "mount": mount,
        "image": backing or None,
        "image_dir": directory or None,
        "backing_present": backing_present,
        "owned": True,
        "unclosed_children": sorted(getattr(proof, "unclosed", set())),
        "details": dict(cached) if cached else {
            "device": None,
            "volume_uuid": None,
            "owners": None,
            "mount_point": mount,
        },
        "details_known": bool(cached) or mount is None,
    }


def refresh_attachment_details(proof: Proof) -> dict:
    """Bounded read-only diskutil query, cached as the last-known facts.

    Never raises and never blocks a record: a refusing, failing, timing-out or
    unparseable query is itself recorded as unavailable."""
    mount = proof.mount
    details = {}
    if mount:
        try:
            info = proof.run(["/usr/sbin/diskutil", "info", "-plist", mount], timeout=30)
            if info.returncode == 0:
                plist = perm.plistlib.loads(info.stdout)
                details = {
                    "device": plist.get("DeviceIdentifier"),
                    "volume_uuid": plist.get("VolumeUUID"),
                    "owners": plist.get("Owners"),
                    "mount_point": plist.get("MountPoint"),
                }
            else:
                details = {"unavailable": f"diskutil exited {info.returncode}"}
        except Exception as error:
            details = {"unavailable": repr(error)}
    try:
        proof._attachment_details = details
    except Exception:
        pass
    return details


def attach_owned_image(
    proof: Proof, label: str, image_name: str = "volume.sparseimage"
) -> str:
    """Attach this proof's private image with filesystem owners ON, then
    positively observe the mount state. Every attach and reattach in this
    proof goes through here. A distinct `image_name` gives an isolated owned
    volume for a control that must not share another registration's payload."""
    mount = perm.attach_image(
        proof,
        owned_image_dir(),
        "ProfileOutputs-" + secrets.token_hex(3),
        owners=True,
        image_name=image_name,
    )
    owners_honored(proof, mount, label)
    refresh_attachment_details(proof)
    write_attachment_facts("attachment-facts.json", known_attachment_facts(proof, label, "attached"))
    return mount


def detach_failure_record(label: str, mount: str, detach_exit: int, stderr: str, image: str, backing_present: bool, details: dict) -> dict:
    """The bounded record a failed ordinary detach retains: attachment custody
    must be reported with the mount, device, volume UUID, image path, backing
    presence and ownership state."""
    return {
        "label": label,
        "mount": mount,
        "detach_exit": detach_exit,
        "stderr": stderr[:4000],
        "image": image,
        "backing_present": backing_present,
        "details": {
            "device": details.get("device"),
            "volume_uuid": details.get("volume_uuid"),
            "owners": details.get("owners"),
            "mount_point": details.get("mount_point"),
        },
    }


def pure_detach_record_control() -> None:
    """No-subprocess control for the detach-failure custody record."""
    record = detach_failure_record(
        "failure",
        "/Volumes/ProfileOutputs-proof",
        -1,
        "detach refused",
        "/evidence/owned-image/volume.sparseimage",
        True,
        {"device": "/dev/disk99s1", "volume_uuid": "11111111-2222-3333-4444-555555555555", "owners": True, "mount_point": "/Volumes/ProfileOutputs-proof"},
    )
    for key in ("mount", "device", "volume_uuid", "image", "backing_present"):
        reference = record if key in record else record["details"]
        expect(reference.get(key), f"the detach-failure record does not carry {key}")
    expect(record["details"].get("owners") is True, "the detach-failure record does not carry ownership")
    expect(record["backing_present"] is True, "the detach-failure record does not report the retained backing")
    print("pure detach-failure record control: ok", flush=True)


def reserve_stop_facts(stderr: str, trigger: int) -> dict:
    """Parse and validate ONE injected reserve-stop report (A11).

    Source: watchdog.rs::report emits
      "oxide: reserve stop (injected test readings): pool filesystem available
       <reading> bytes fell below the <trigger> byte trigger (20 GiB reserve +
       2 GiB margin); Cargo group <id>: <TERM | TERM, then KILL after N ms>;
       sampled minimum <bytes> bytes, overshoot past trigger <bytes> bytes,
       stop latency <ms>; budget-only pool: best-effort stop, ..."
    Every fact is required: the injected reason, a real reading and trigger, a
    concrete group id, a numeric sampled minimum, an overshoot that follows from
    trigger minus minimum, and a CONFIRMED closure latency.
    """
    line = next((entry for entry in stderr.splitlines() if "oxide: reserve stop" in entry), "")
    expect(line != "", f"no reserve-stop line in the clean summary: {stderr}")
    expect("(injected test readings)" in line, f"the stop did not name the injected readings: {line}")
    numbers = re.search(r"pool filesystem available (\d+) bytes fell below the (\d+) byte trigger", line)
    expect(numbers is not None, f"no reading/trigger in the stop: {line}")
    reading, seen_trigger = int(numbers.group(1)), int(numbers.group(2))
    expect(seen_trigger == trigger, f"the trigger is {seen_trigger}, not {trigger}: {line}")
    group = re.search(r"Cargo group (-?\d+): (TERM(?:, then KILL after \d+ ms)?)", line)
    expect(group is not None, f"no concrete Cargo group and signal in the stop: {line}")
    minimum = re.search(r"sampled minimum (\d+) bytes", line)
    expect(minimum is not None, f"the sampled minimum is not a number: {line}")
    overshoot = re.search(r"overshoot past trigger (\d+) bytes", line)
    expect(overshoot is not None, f"no overshoot in the stop: {line}")
    expect(
        int(overshoot.group(1)) == seen_trigger - int(minimum.group(1)),
        f"the overshoot does not follow from the trigger and minimum: {line}",
    )
    latency = re.search(r"stop latency (\d+) ms", line)
    expect(latency is not None, f"the stop closure was not confirmed: {line}")
    expect("20 GiB reserve + 2 GiB margin" in line, f"the reserve/margin numbers changed: {line}")
    return {
        "line": line,
        "reading": reading,
        "trigger": seen_trigger,
        "group": int(group.group(1)),
        "escalated": "then KILL" in line,
        "minimum": int(minimum.group(1)),
    }


def reserve_stop_status(facts: dict) -> set:
    """The exit statuses a watchdog-terminated Cargo child can really return.

    Source chain: the clean waits with wait_leader_watched (reaping the leader)
    and then wait_group_empty_watched, which escalates only REMAINING
    descendants. So a report of "TERM, then KILL after N ms" can still leave the
    leader reaped by TERM -> 143; 137 requires the leader itself to be killed.
    Therefore: TERM-only permits exactly 143; an escalated report permits 143 or
    137. A blanket 1 (or any other code) is never acceptable."""
    return {143, 137} if facts["escalated"] else {143}


def pure_reserve_stop_control() -> None:
    """No-subprocess matched/mismatched controls for the reserve-stop parser."""
    trigger = 22 * 1024 * 1024 * 1024
    reading = trigger - 4096
    head = (
        f"oxide: reserve stop (injected test readings): pool filesystem available {reading} bytes "
        f"fell below the {trigger} byte trigger (20 GiB reserve + 2 GiB margin); Cargo group 4242: "
    )
    tail = f"; sampled minimum {reading} bytes, overshoot past trigger 4096 bytes, stop latency 12 ms; budget-only pool: best-effort stop, no guarantee against faster writers or other processes"
    plain = head + "TERM" + tail
    escalated = head + "TERM, then KILL after 250 ms" + tail
    facts = reserve_stop_facts(plain, trigger)
    expect(facts["reading"] == reading and facts["group"] == 4242, f"the plain stop parsed wrong: {facts}")
    expect(
        reserve_stop_status(facts) == {143},
        f"a TERM stop must permit exactly 143, not {reserve_stop_status(facts)}",
    )
    facts = reserve_stop_facts(escalated, trigger)
    expect(facts["escalated"] is True, f"the escalation was not parsed: {facts}")
    # ROOT nuance: the leader may be reaped by TERM before descendants escalate,
    # so an escalated report permits 143 as well as 137 - but never anything
    # else, and a TERM-only report permits only 143.
    expect(
        reserve_stop_status(facts) == {143, 137},
        "an escalated stop must permit exactly the leader's TERM or KILL status",
    )
    expect(
        reserve_stop_status(reserve_stop_facts(plain, trigger)) == {143},
        "a TERM-only stop must permit only 143",
    )
    for wrong in (0, 1, 2, 130, 137):
        expect(
            wrong not in reserve_stop_status(reserve_stop_facts(plain, trigger)),
            f"a TERM-only stop must not accept status {wrong}",
        )
    for bad, why in (
        (plain.replace("(injected test readings)", "(real volume)"), "unnamed injected source"),
        (plain.replace(f"available {reading} bytes", "available plenty bytes", 1), "a non-numeric reading"),
        (plain.replace(f"fell below the {trigger} byte trigger", "fell below the 1024 byte trigger"), "wrong trigger"),
        (plain.replace("Cargo group 4242", "Cargo group unknown"), "no concrete group id"),
        (plain.replace(f"sampled minimum {reading} bytes", "sampled minimum unknown bytes"), "unknown minimum"),
        (plain.replace("overshoot past trigger 4096 bytes", "overshoot past trigger 7 bytes"), "overshoot that does not follow"),
        (plain.replace("stop latency 12 ms", "closure not confirmed"), "unconfirmed closure"),
        ("oxide: quick profile, build strategy, slot abc123 cold, 0.6s, new slot", "a normal successful build"),
        ("oxide: reserve stop (injected test readings): pool filesystem available 1 bytes fell below the 2 byte trigger", "a truncated stop"),
    ):
        try:
            reserve_stop_facts(bad, trigger)
        except Exception:
            continue
        raise AssertionError(f"the reserve-stop parser accepted {why}")
    print("pure reserve-stop control: ok", flush=True)


class _FakeCustodyChild:
    """A child stand-in: no process, no subprocess, no signal."""

    def __init__(self, pid: int, returncode=None) -> None:
        self.pid = pid
        self.returncode = returncode

    def poll(self):
        return self.returncode


class _FakeUnclosedProof:
    def __init__(self, pids) -> None:
        self.unclosed = set(pids)
        self.mount = None


def pure_child_custody_control() -> None:
    """Synthetic, no-subprocess controls for the A10 child custody bookkeeping.

    They prove: an owned gated child is released and finished through the
    injected existing-helper seam; a child that exited without a helper outcome
    is never signalled or re-run and stays unclosed; a failing finish keeps it
    unclosed; exhausted remaining time settles nothing; a child this proof does
    not own is never touched; and an unwritable evidence root is an explicit
    failure rather than a silent success."""
    # 1. An owned gated child is released and finished.
    with tempfile.TemporaryDirectory() as root:
        gate = os.path.join(root, "gate")
        os.makedirs(gate)
        proof = _FakeUnclosedProof({101})
        custody = ChildCustody()
        child = _FakeCustodyChild(101)
        custody.track(child, gate, "gated case")
        calls = []

        def finish(fake_proof, fake_child, label):
            calls.append((fake_child.pid, label))
            fake_proof.unclosed.discard(fake_child.pid)
            return 0, "ok", ""

        record = custody.settle(proof, "mock failure", finish=finish, remaining=lambda: 60.0)
        expect(calls == [(101, "mock failure: gated case")], f"the child was not finished through the helper seam: {calls}")
        expect(os.path.isfile(os.path.join(gate, "release")), "the owned gate was not released")
        expect(record["settled"] and record["settled"][0]["exit"] == 0, f"the settled record is wrong: {record}")
        expect(record["still_unclosed"] == [], f"the settled child is still unclosed: {record}")
        expect(record["not_owned"] == [], f"an owned child was reported as foreign: {record}")

    # 2. A child that exited without a helper outcome is not touched.
    proof = _FakeUnclosedProof({202})
    custody = ChildCustody()
    custody.track(_FakeCustodyChild(202, returncode=7), None, "exited early")
    calls = []
    record = custody.settle(
        proof, "mock", finish=lambda *a: calls.append(a) or (0, "", ""), remaining=lambda: 60.0
    )
    expect(calls == [], "an already-exited child was finished as new work")
    expect(record["settled"] == [], f"an unestablished exit was reported settled: {record}")
    expect(202 in record["still_unclosed"], f"poll was treated as closure: {record}")
    expect(record["unresolved"] and record["unresolved"][0]["observed_exit"] == 7, f"the observed exit was lost: {record}")

    # 3. A failing finish keeps the child unclosed and does not claim closure.
    proof = _FakeUnclosedProof({303})
    custody = ChildCustody()
    custody.track(_FakeCustodyChild(303), None, "finish fails")
    record = custody.settle(
        proof,
        "mock",
        finish=lambda *a: (_ for _ in ()).throw(RuntimeError("finish refused")),
        remaining=lambda: 60.0,
    )
    expect(record["settled"] == [], f"a failed finish was reported settled: {record}")
    expect(303 in record["still_unclosed"], f"a failed finish dropped the child: {record}")

    # 4. Exhausted remaining time settles nothing and does not signal.
    proof = _FakeUnclosedProof({404})
    custody = ChildCustody()
    custody.track(_FakeCustodyChild(404), None, "no time")
    calls = []
    record = custody.settle(
        proof, "mock", finish=lambda *a: calls.append(a) or (0, "", ""), remaining=lambda: 0
    )
    expect(calls == [], "a child was finished after the bounded time ran out")
    expect(
        record["unresolved"] and "no bounded time left" in record["unresolved"][0]["reason"],
        f"the exhausted-time record is wrong: {record}",
    )
    # A raising remaining-time probe is the same outcome, never a hang.
    record = custody.settle(
        proof,
        "mock",
        finish=lambda *a: calls.append(a) or (0, "", ""),
        remaining=lambda: (_ for _ in ()).throw(RuntimeError("deadline reached")),
    )
    expect(not record["settled"] and calls == [], f"a raising time probe settled work: {record}")

    # 5. A child this proof does not own is never touched.
    proof = _FakeUnclosedProof({505})
    custody = ChildCustody()
    calls = []
    record = custody.settle(
        proof, "mock", finish=lambda *a: calls.append(a) or (0, "", ""), remaining=lambda: 60.0
    )
    expect(calls == [], "an unowned child was finished")
    expect(record["not_owned"] == [505], f"an unowned child was not reported: {record}")
    expect(record["still_unclosed"] == [505], f"an unowned child was dropped: {record}")

    # 6. An unwritable evidence root is an explicit failure, not a success.
    with tempfile.TemporaryDirectory() as root:
        blocked = os.path.join(root, "not-a-directory")
        with open(blocked, "w", encoding="utf-8") as handle:
            handle.write("x")
        try:
            write_attachment_facts("attachment-facts.json", {"state": "x"}, evidence=blocked)
        except Exception:
            pass
        else:
            raise AssertionError("a custody record was reported written into an unwritable root")
    print("pure child-custody control: ok (synthetic, no subprocess)", flush=True)


def pure_slot_state_control() -> None:
    """No-subprocess control for the parsed slot summary (A11). The slot id and
    the cold/warm state come from the summary LINE, so a path or directory name
    containing "cold", "warm", "fallback" or "foreign" can never stand in for
    the state, and a line without a slot is a failure rather than a default."""
    slot, state = perm.slot_state(
        "oxide: quick profile, build strategy, slot abc123 cold, 0.6s, new slot, pool total: see oxide status"
    )
    expect((slot, state) == ("abc123", "cold"), f"the parsed slot/state changed: {(slot, state)}")
    slot, state = perm.slot_state(
        "oxide: quick profile, build strategy, slot zz9 warm, 0.2s, slot zz9 was 1 MiB when leased"
    )
    expect((slot, state) == ("zz9", "warm"), f"the parsed warm slot changed: {(slot, state)}")
    for bad, why in (
        ("oxide: quick profile built into /tmp/cold-warm/fallback/foreign, 0.6s", "path words only"),
        ("oxide: quick profile, build strategy, slot abc123, 0.6s", "no state"),
    ):
        try:
            perm.slot_state(bad)
        except Exception:
            continue
        raise AssertionError(f"the slot summary parsed a {why} line")
    print("pure slot-state control: ok", flush=True)


class _FakeCustodyProof:
    """Synthetic stand-in for the custody paths: it NEVER spawns a subprocess.

    `run` raises whatever failure the case wants to model (query refused,
    deadline hit), which is exactly the situation in which a failure record
    must still be written from already-known facts."""

    def __init__(
        self, mount: str, failure: str = "query refused", detach_returncode=None
    ) -> None:
        self.mount = mount
        self.unclosed = set()
        self.failure = failure
        self.detach_returncode = detach_returncode
        self.calls = []

    def run(self, args, **_kwargs):
        self.calls.append(list(args))
        if self.detach_returncode is not None and "detach" in args:
            return _FakeDetachResult(self.detach_returncode)
        raise RuntimeError(self.failure)


class _FakeDetachResult:
    """A refused ordinary detach: no subprocess, a real return code."""

    def __init__(self, returncode: int) -> None:
        self.returncode = returncode
        self.stdout = b""
        self.stderr = b"detach refused by the mock"


def pure_attachment_facts_control() -> None:
    """Synthetic-filesystem, no-subprocess control for the A10 custody facts.

    It writes only into its OWN temporary directories, never into the runner's
    evidence root: the writer takes an explicit evidence seam, and where the
    control does exercise the environment it SAVES and RESTORES whatever the
    runner set (a sentinel is checked byte-for-byte afterwards)."""
    real_evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    with tempfile.TemporaryDirectory() as root:
        # 1. Explicit seam: no environment needed at all.
        path = write_attachment_facts("attachment-facts.json", {"state": "attached"}, evidence=root)
        expect(path == os.path.join(root, "attachment-facts.json"), f"the explicit seam wrote {path}")
        write_attachment_facts("attachment-facts.json", {"state": "detached"}, evidence=root)
        with open(os.path.join(root, "attachment-facts.json"), encoding="utf-8") as handle:
            record = json.load(handle)
        expect(record.get("state") == "detached", f"the facts were not replaced atomically: {record}")
        expect(
            not os.path.exists(os.path.join(root, "attachment-facts.json.new")),
            "a temporary attachment-facts file was left behind",
        )
        # 2. Absent evidence root: skipped, and no file appears anywhere.
        expect(
            write_attachment_facts("attachment-unset.json", {"state": "never"}, evidence="") is None,
            "the writer wrote without an evidence root",
        )
        expect(
            not os.path.exists(os.path.join(root, "attachment-unset.json")),
            "the writer wrote into a directory it was not given",
        )
        # 3. Environment seam with a SENTINEL: the runner's value is preserved
        #    exactly and nothing is written into the runner's real root.
        os.environ["OXIDE_PROOF_EVIDENCE"] = root
        try:
            write_attachment_facts("attachment-env.json", {"state": "attached"})
            expect(
                os.path.isfile(os.path.join(root, "attachment-env.json")),
                "the environment seam did not write into the temporary root",
            )
        finally:
            if real_evidence is None:
                os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
            else:
                os.environ["OXIDE_PROOF_EVIDENCE"] = real_evidence
        expect(
            os.environ.get("OXIDE_PROOF_EVIDENCE") == real_evidence,
            "the control did not restore the runner's evidence value exactly",
        )
        if real_evidence:
            expect(
                not os.path.exists(os.path.join(real_evidence, "attachment-unset.json")),
                "the control wrote a probe file into the runner's real evidence root",
            )
        # 4. Failure paths use KNOWN facts only: no subprocess, nothing lost.
        mount = os.path.join(root, "mock-mount")
        os.makedirs(mount)
        os.makedirs(os.path.join(root, "owned-image"), exist_ok=True)
        backing = os.path.join(root, "owned-image", "volume.sparseimage")
        with open(backing, "wb") as handle:
            handle.write(b"owned backing")
        # owned_image_dir() reads the evidence root, so the synthetic custody
        # section runs with the TEMPORARY root (restored in the finally below).
        real_scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
        os.environ["OXIDE_PROOF_EVIDENCE"] = root
        os.environ["OXIDE_TASK_SCRATCH_ROOT"] = root
        fake = _FakeCustodyProof(mount)
        facts = known_attachment_facts(fake, "mock", "detaching")
        expect(fake.calls == [], f"the known-facts record spawned a query: {fake.calls}")
        expect(facts["mount"] == mount and facts["owned"] is True, f"the known facts lost the mount: {facts}")
        expect(
            facts["details"]["device"] is None and facts["details_known"] is False,
            f"unknown device facts were invented: {facts['details']}",
        )
        # A failing observation is recorded as unavailable, never raised.
        os.environ["OXIDE_PROOF_EVIDENCE"] = root
        try:
            refreshed = refresh_attachment_details(fake)
            expect("unavailable" in refreshed, f"a failed query was not recorded as unavailable: {refreshed}")
            expect(
                known_attachment_facts(fake, "mock", "attached")["details_known"] is True,
                "the cached last-known facts were not retained",
            )
            expect(os.path.isfile(backing), "the backing disappeared before the detach attempt")
            try:
                detach_owned_volume(fake, "mock detach timeout")
            except RuntimeError:
                pass
            else:
                raise AssertionError("a timed-out detach reported success")
            unresolved = os.path.join(root, "detach-unresolved.json")
            expect(os.path.isfile(unresolved), "a timed-out detach left no unresolved-custody record")
            with open(unresolved, encoding="utf-8") as handle:
                record = json.load(handle)
            expect(
                "unresolved" in record.get("state", "") and record.get("mount") == mount,
                f"the unresolved record is not honest about the mount: {record}",
            )
            expect(os.path.isfile(backing), "the backing was deleted after a failed detach")
            # A REFUSED ordinary detach: the record must be a FILE under the
            # evidence root (never the directory, never os.devnull), written from
            # known facts before any further query, with the backing retained.
            refused_detach = _FakeCustodyProof(mount, detach_returncode=1)
            try:
                detach_owned_volume(refused_detach, "mock detach refused")
            except RuntimeError:
                pass
            else:
                raise AssertionError("a refused detach reported success")
            failure_record = os.path.join(root, "detach-failure.json")
            expect(
                os.path.isfile(failure_record),
                "a refused detach wrote no failure record file",
            )
            with open(failure_record, encoding="utf-8") as handle:
                record = json.load(handle)
            expect(
                record.get("mount") == mount and record.get("detach_exit") == 1,
                f"the failure record is not honest about the mount or exit: {record}",
            )
            expect(
                os.path.isfile(os.path.join(root, "detach-details-unavailable.json")),
                "a refused post-failure detail query was not recorded",
            )
            expect(os.path.isfile(backing), "the backing was deleted after a refused detach")
        finally:
            if real_evidence is None:
                os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
            else:
                os.environ["OXIDE_PROOF_EVIDENCE"] = real_evidence
            if real_scratch is None:
                os.environ.pop("OXIDE_TASK_SCRATCH_ROOT", None)
            else:
                os.environ["OXIDE_TASK_SCRATCH_ROOT"] = real_scratch
    print("pure attachment-facts control: ok (synthetic filesystem, no subprocess)", flush=True)


def detach_owned_volume(proof: Proof, label: str) -> None:
    """Ordinary detach of the proof-owned mount, used on success AND after a
    failure once direct child outcomes are known. A failed or unresolved
    detach fails the proof and records the mount/device/UUID/image/ownership;
    the backing stays in the guard's OWN evidence for the custodian. Backing is
    removed only after a positive ordinary detach confirmation, never from a
    process census."""
    mount = proof.mount
    if not mount:
        return
    directory = owned_image_dir()
    backing = os.path.join(directory, "volume.sparseimage")
    # A10: unknown custody blocks detach. Gated children are settled through
    # the existing helpers before this point; an unclosed child means their
    # outcome is not known, so the mount is kept and reported instead.
    unclosed = sorted(getattr(proof, "unclosed", set()))
    if unclosed:
        # A10: first settle only THIS script's own gated children through the
        # existing helpers, within the existing deadline, and persist that.
        custody = CUSTODY.settle(proof, label)
        write_attachment_facts(
            "detach-custody.json",
            {
                **known_attachment_facts(proof, label, "settling owned children"),
                "custody": custody,
            },
        )
        unclosed = sorted(getattr(proof, "unclosed", set()))
    if unclosed:
        record = known_attachment_facts(proof, label, "unresolved: unclosed children")
        record["reason"] = "refused to detach while child custody is unknown"
        record["not_owned"] = CUSTODY.not_owned(proof)
        write_attachment_facts("detach-unresolved.json", record)
        fail(f"{label}: refusing ordinary detach while {len(unclosed)} child(ren) are unclosed; the mount and backing are retained")
        return
    expect(os.path.lexists(mount), f"{label}: the owned mount {mount} vanished before ordinary detach")
    # Persist the last-known facts BEFORE the risky calls, so an interruption
    # (including a timeout) still leaves custody facts in evidence. A write that
    # itself fails is an explicit failure and the mount is never touched.
    try:
        write_attachment_facts(
            "attachment-facts.json", known_attachment_facts(proof, label, "detaching")
        )
    except Exception as error:
        fail(
            f"{label}: cannot record the attachment facts before detaching ({error!r}); "
            f"the mount {mount} and any backing are retained"
        )
        return
    try:
        result = proof.run(["/usr/bin/hdiutil", "detach", mount], timeout=60)
    except Exception as error:  # a timeout or a harness fault is unresolved custody
        record = known_attachment_facts(proof, label, "unresolved: detach did not complete")
        record["reason"] = f"detach raised {error!r}"
        try:
            write_attachment_facts("detach-unresolved.json", record)
        except Exception as write_error:
            fail(
                f"{label}: the unresolved-custody record could not be written ({write_error!r}); "
                f"the mount {mount} and any backing are retained"
            )
            return
        fail(f"{label}: ordinary detach of {mount} did not complete ({error!r}); the backing is retained in the guard evidence")
        return
    if result.returncode != 0:
        # A10: the failure record is written FIRST, from last-known facts only.
        # Nothing here needs a subprocess, and the record is a FILE under the
        # evidence root (never the directory itself, never os.devnull).
        record = dict(
            detach_failure_record(
                label,
                mount,
                result.returncode,
                result.stderr.decode(errors="replace"),
                backing,
                os.path.exists(backing),
                dict(getattr(proof, "_attachment_details", {}) or {}),
            )
        )
        record["known"] = known_attachment_facts(proof, label, "unresolved: detach failed")
        try:
            write_attachment_facts("detach-failure.json", record)
            write_attachment_facts(
                "detach-unresolved.json",
                known_attachment_facts(proof, label, "unresolved: detach failed"),
            )
        except Exception as error:
            fail(
                f"{label}: ordinary detach of {mount} failed and the custody record could not be "
                f"written ({error!r}); the backing is retained in the guard evidence"
            )
            return
        # A refused detail query is recorded separately and can never be the
        # reason the failure above went unrecorded.
        details = refresh_attachment_details(proof)
        if "unavailable" in details:
            write_attachment_facts(
                "detach-details-unavailable.json",
                {"label": label, "error": details["unavailable"]},
            )
        fail(f"{label}: ordinary detach of {mount} failed; the backing is retained in the guard evidence")
        return
    proof.mount = None
    write_attachment_facts("attachment-facts.json", known_attachment_facts(proof, label, "detached"))
    if os.path.exists(backing):
        os.remove(backing)
    try:
        os.rmdir(directory)
    except OSError:
        pass


def register(proof: Proof, pool: str, mode: str = "permanent") -> None:
    mount = attach_owned_image(proof, "initial attach", "volume.sparseimage")
    code, _, stderr = proof.oxide(
        ["pool", "register-removable", mount, "--reserve", IMAGE_RESERVE, "--cap", "64MiB", "--mode", mode],
        "register removable",
        proof.root,
    )
    expect(code == 0, f"registration failed: {stderr}")
    expect(os.path.isfile(perm.registration_path(pool)), "no removable registration")
    registration = perm.read_json(perm.registration_path(pool))
    expect(registration["reserve_bytes"] == 16 * 1024 * 1024, f"reserve {registration.get('reserve_bytes')}")
    expect(registration["cap_bytes"] == 64 * 1024 * 1024, f"cap {registration.get('cap_bytes')}")
    volume = perm.plistlib.loads(
        proof.run(["/usr/sbin/diskutil", "info", "-plist", mount], timeout=30).stdout
    )
    expect(volume.get("VolumeUUID") == registration["volume_uuid"], "registration UUID does not match the mounted volume")


def oxide(proof: Proof, args: list, label: str, cwd: str) -> tuple:
    return proof.oxide(args, label, cwd)


def auto_legacy(proof: Proof, checkout: str) -> None:
    """Create the published whole-target /2 state the way a fresh checkout on
    the permanent drive gets it: the automatic first conversion."""
    code, _, stderr = oxide(proof, ["build"], "auto-adopt", checkout)
    expect(code == 0, f"auto-adopt build failed: {stderr}")
    expect(os.path.islink(os.path.join(checkout, "target")), "automatic conversion did not publish a whole-target link")


def base_legacy(proof: Proof, base: str, pool: str, checkout: str) -> None:
    """Create a whole-target /2 state with the STAGED BASE binary (never the
    candidate) in the one private pool BEFORE any candidate publishes the
    successor fence there."""
    env = {**proof.env, "OXIDE_POOL": pool}
    result = proof.run([base, "outputs", "adopt"], cwd=checkout, env=env, timeout=90)
    stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
    print(f"----- base legacy adopt: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
    expect(result.returncode == 0, f"base whole-target adopt failed: {stderr}")
    expect(
        os.path.islink(os.path.join(checkout, "target")),
        "base adopt did not publish a whole-target link",
    )
    doc = perm.read_json(perm.receipt_path(pool, checkout))
    expect(doc["version"] == "oxide.outputs.transition/2", f"base receipt {doc['version']}")
    # The base binary wrote a /2 whole-target placement, not a profile one.
    expect(doc["links"][0]["member"] == "target", f"base placement {doc['links'][0]}")
    # The second pool is still on the published fence, so the candidate's
    # adopt must take the explicit migration path.
    state = perm.read_json(os.path.join(pool, "oxide-managed", "state.json"))
    expect(
        state.get("owned_outputs") == "oxide.outputs/2",
        f"the base pool fence is {state.get('owned_outputs')}, not the published /2",
    )


def validate_migrated_registry(rows: list, root: str, nonce: str, volume_uuid, generation: int) -> None:
    """Pure registry predicate for a migrated profile group.

    Exactly the debug+release profile rows are live and share the upgraded root
    nonce and identity; any retained whole-target history row is non-selected,
    non-pending and never a live reference. A missing or malformed selected
    nonce, an extra live member or a live legacy row is an explicit failure.
    """
    selected = [row for row in rows if row.get("selected")]
    pending = [row for row in rows if row.get("pending")]
    live_other = [
        row
        for row in rows
        if row.get("pruning") or row.get("reservation") or row.get("cancelling")
    ]
    expect({row.get("member") for row in selected} == {"debug", "release"}, f"11: selected profile rows are {selected}")
    # A7: exactly ONE selected row per expected member. A set comparison above
    # cannot see a duplicated row, so the sorted member list is compared too.
    expect(
        sorted(row.get("member") for row in selected) == ["debug", "release"],
        f"11: selected profile rows are not exactly one debug and one release: {selected}",
    )
    expect({row.get("member") for row in pending} == set(), f"11: pending registry references remain: {pending}")
    expect(
        [row.get("member") for row in pending] == [],
        f"11: duplicate pending registry references remain: {pending}",
    )
    expect({row.get("member") for row in live_other} == set(), f"11: live pruning/reservation rows remain: {live_other}")
    for row in selected:
        expect(row.get("nonce") == nonce, f"11: selected {row.get('member')} row does not carry the upgraded nonce: {row}")
        expect(row.get("root") == root, f"11: selected {row.get('member')} row names another root: {row}")
        expect(row.get("volume_uuid") == volume_uuid, f"11: selected {row.get('member')} row names another volume: {row}")
        expect(row.get("generation") == generation, f"11: selected {row.get('member')} row names another generation: {row}")
    for row in rows:
        if row.get("member") == "target":
            expect(
                not row.get("selected") and not row.get("pending"),
                f"11: retained legacy target history is live: {row}",
            )
            expect(row.get("root") == root, f"11: legacy history names another root: {row}")


def pure_registry_controls() -> None:
    """No-subprocess self-controls for the case-11 registry predicate."""
    good = [
        {"member": "debug", "selected": True, "pending": False, "root": "/r", "nonce": "a" * 32, "volume_uuid": "11111111-2222-3333-4444-555555555555", "generation": 1},
        {"member": "release", "selected": True, "pending": False, "root": "/r", "nonce": "a" * 32, "volume_uuid": "11111111-2222-3333-4444-555555555555", "generation": 1},
        {"member": "target", "selected": False, "pending": False, "root": "/r", "generation": 1},
    ]
    validate_migrated_registry(good, "/r", "a" * 32, "11111111-2222-3333-4444-555555555555", 1)
    for broken, why in (
        ([{**good[0], "nonce": None}, good[1], good[2]], "missing selected nonce"),
        ([{**good[0], "nonce": "b" * 32}, good[1], good[2]], "mismatched selected nonce"),
        ([good[0], good[1], {**good[2], "selected": True}], "extra live legacy member"),
        ([good[0], good[1], {**good[2], "pending": True}], "pending legacy member"),
        ([good[0], good[1], good[2], {**good[0], "member": "x86_64-unknown-linux-gnu/debug"}], "extra live profile member"),
        # A7: a duplicated selected row keeps the member SET identical, so only
        # an explicit per-row count rejects it.
        ([good[0], good[0], good[1], good[2]], "duplicated selected profile row"),
        ([good[0], good[1], {**good[2], "selected": False, "pending": True}, {**good[2], "pending": True}], "duplicated pending history row"),
    ):
        try:
            validate_migrated_registry(broken, "/r", "a" * 32, "11111111-2222-3333-4444-555555555555", 1)
        except Exception:
            continue
        raise AssertionError(f"the registry predicate accepted a {why}")
    print("pure registry controls: ok", flush=True)


def summary_members(stderr: str) -> set:
    """The owned member links the PRODUCT reports for this command.

    `oxide` prints one summary line ending in
    "... through profile member links (<m1>, <m2>)", which names exactly the
    members this command placed outputs for. This is the member-identifying
    product fact; a leased slot id is a key of (recipe, layout, member) and is
    never member identity on its own."""
    line = summary(stderr)
    match = re.search(r"profile member links \(([^)]*)\)", line)
    if match is None:
        return set()
    return {part.strip() for part in match.group(1).split(",") if part.strip()}


def summary_recipe(stderr: str) -> str:
    """The recipe label the PRODUCT reports: quick, debug or release."""
    line = summary(stderr)
    match = re.search(r"oxide: (quick|debug|release) profile,", line)
    expect(match is not None, f"no recipe label in the summary: {line}")
    return match.group(1)


def member_payload_state(root: str, member: str) -> tuple:
    """A bounded identity of ONE member payload, to prove a command wrote (or
    did not write) that member: absent, or the directory identity plus a capped
    listing and its newest entry mtime. Never walks the payload."""
    path = os.path.join(root, member)
    if not os.path.isdir(path):
        return ("absent",)
    info = os.stat(path)
    entries = sorted(os.listdir(path))[:64]
    newest = 0
    for name in entries:
        try:
            newest = max(newest, os.stat(os.path.join(path, name)).st_mtime_ns)
        except OSError:
            continue
    return ("dir", info.st_dev, info.st_ino, tuple(entries), newest)


ASSERTION_RECEIPT_BYTES = 64 * 1024
CASE_IDS = [f"{index:02d}" for index in range(1, 21)]
_CASE_PROGRESS = {"reached": [], "started_ms": None}


def case_reached(case_id: str) -> None:
    """Record that one fixed case block has started, in execution order."""
    reached = _CASE_PROGRESS["reached"]
    if case_id not in reached:
        reached.append(case_id)
    if _CASE_PROGRESS["started_ms"] is None:
        _CASE_PROGRESS["started_ms"] = time.time_ns() // 1_000_000


def write_assertion_receipt(outcome: str, failure) -> str:
    """Write ONE uniquely named bounded assertion/source receipt beside the
    guard receipt in the runner's kept evidence root.

    It records only what this script itself knows: the source/script/helper
    hashes it ran, the cases it actually reached, and the primary failure. It
    makes NO closure, census, gate or native-PASS claim, and it never
    reconstructs an earlier run. Exclusive creation, at most 64 KiB."""
    evidence = evidence_root()
    expect(
        bool(evidence) and os.path.isdir(evidence),
        f"no kept evidence root for the assertion receipt: {evidence!r}",
    )
    reached = list(_CASE_PROGRESS["reached"])
    if outcome == "pass":
        # The case blocks run in the script's own order, which is not the plan
        # order (the base-created migration controls come first), so the fixed
        # set is compared as a set and the execution order is kept in the
        # record exactly as it happened.
        expect(
            sorted(reached) == CASE_IDS,
            f"the proof finished without running every fixed case: reached {reached}",
        )
        completed, failed_in, unexecuted = reached, None, []
    else:
        # Conservative on failure: the last reached case is reported as
        # started-but-unconfirmed, never as completed.
        completed = reached[:-1]
        failed_in = reached[-1] if reached else None
        unexecuted = [case for case in CASE_IDS if case not in reached]
    record = {
        "version": "oxide.outputs.proof-assertions/1",
        "outcome": outcome,
        "primary_failure": None if failure is None else str(failure)[-2000:],
        "started_ms": _CASE_PROGRESS["started_ms"],
        "finished_ms": time.time_ns() // 1_000_000,
        "source": os.environ.get("OXIDE_PROFILE_HEAD", "unbound"),
        "script": {
            "path": "scripts/test-profile-outputs.py",
            "sha256": perm.sha256_file(os.path.abspath(__file__)),
        },
        "helpers": {
            "scripts/test-oxide-pool-layout.py": perm.sha256_file(
                os.path.join(HERE, "test-oxide-pool-layout.py")
            ),
            "scripts/test-permanent-outputs.py": perm.sha256_file(
                os.path.join(HERE, "test-permanent-outputs.py")
            ),
        },
        "cases": {
            "total": len(CASE_IDS),
            "reached": reached,
            "completed": completed,
            "failed_in": failed_in,
            "unexecuted": unexecuted,
        },
        "claims": "this script's own case progress and source hashes only; no closure, census, gate or native PASS claim",
    }
    payload = (json.dumps(record, indent=2, sort_keys=True) + "\n").encode()
    if len(payload) > ASSERTION_RECEIPT_BYTES:
        raise RuntimeError(
            f"the assertion receipt would exceed {ASSERTION_RECEIPT_BYTES} bytes"
        )
    name = f"profile-outputs.assertions-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{os.getpid()}.json"
    path = os.path.join(evidence, name)
    with open(path, "x", encoding="utf-8") as handle:
        handle.write(payload.decode())
        handle.flush()
        os.fsync(handle.fileno())
    return path


def selected_backend_slots(proof: Proof, pool: str, label: str) -> str:
    """The ACTUAL selected removable backend's slot root, derived from the
    fixture registration rather than an assumed path: the registered volume's
    payload directory (`removable::PAYLOAD_DIR` = "oxide-pool") holds the pool's
    `SLOTS_NAME` ("slots") directory. A8: slot no-effect controls must watch
    this tree, not the unused internal payload tree."""
    registration = perm.read_json(perm.registration_path(pool))
    mount = registration.get("mount_point")
    expect(bool(mount), f"{label}: the registration names no mount point")
    expect(
        proof.mount is not None and os.path.realpath(mount) == os.path.realpath(proof.mount),
        f"{label}: the registered mount {mount} is not the attached volume {proof.mount}",
    )
    slots = os.path.join(mount, "oxide-pool", "slots")
    expect(os.path.isdir(slots), f"{label}: the selected backend slot root {slots} is missing")
    return slots


def custody_snapshot(pool: str, checkout: str) -> dict:
    """Bounded observable state of one checkout's output authority: receipt
    bytes, member link texts, the root marker's bytes, the checkout's own
    target link (text and inode, so a rewritten legacy link is visible) and the
    bounded registry authority digest. A8: a wait/no-effect control cannot rest
    on links and a marker alone."""
    receipt_path = receipt_file(pool, checkout)
    snapshot = {"receipt": perm.sha256_file(receipt_path) if os.path.exists(receipt_path) else "absent"}
    links = {}
    for member in ("debug", "release"):
        link = member_link(checkout, member)
        if os.path.islink(link):
            links[member] = os.readlink(link)
    snapshot["links"] = links
    marker = os.path.join(root_of(pool, checkout), MARKER) if os.path.exists(receipt_path) else ""
    snapshot["marker"] = perm.sha256_file(marker) if marker and os.path.exists(marker) else "absent"
    target = os.path.join(checkout, "target")
    if os.path.lexists(target):
        info = os.lstat(target)
        snapshot["target"] = {
            "is_link": os.path.islink(target),
            "text": os.readlink(target) if os.path.islink(target) else "",
            "dev": info.st_dev,
            "ino": info.st_ino,
        }
    else:
        snapshot["target"] = "absent"
    registry = os.path.join(pool, "oxide-managed", "outputs", "registry.json")
    snapshot["registry"] = perm.sha256_file(registry) if os.path.exists(registry) else "absent"
    return snapshot


def observed_wait(child, before: dict, pool: str, checkout: str, label: str) -> None:
    """The waiter must still be running while the writer holds the gate, and
    it must not change the checkout's output authority while it waits."""
    deadline = time.monotonic() + min(10, work_left())
    while time.monotonic() < deadline:
        expect(child.poll() is None, f"{label}: the waiter exited instead of waiting")
        expect(
            custody_snapshot(pool, checkout) == before,
            f"{label}: the waiter changed the output authority while the writer was gated",
        )
        time.sleep(0.1)


def adopt(proof: Proof, checkout: str, label: str) -> None:
    code, _, stderr = proof.oxide(["outputs", "adopt"], label, checkout)
    expect(code == 0, f"{label}: adopt exited {code}: {stderr}")


DIAGNOSTIC_ARGUMENT = "--diagnostic"
DIAGNOSTIC_RECEIPT_BYTES = 64 * 1024
# The ONLY messages that turn an oracle failure into a safety abort. A
# diagnostic may step over a wrong expectation; it may never step over the
# deadline, an unrecordable receipt, unknown custody or an unsettled child.
DIAGNOSTIC_SAFETY_MARKERS = (
    "work deadline reached",
    "no kept evidence root",
    "unclosed",
    "custody",
)

# Every case block of the fixed matrix, its own id and the steps it needs to
# be complete before it can run. Case 11 is two blocks (its base-created
# fixture must exist before case 10's warm check and its migration runs after),
# so the unit is the step and the numbered case is the reported subject.
CASE_STEPS = [
    ("11.1", "11", ()),
    ("11.2", "11", ("11.1",)),
    ("01.1", "01", ()),
    ("02.1", "02", ("01.1",)),
    ("03.1", "03", ("01.1",)),
    ("04.1", "04", ("01.1",)),
    ("05.1", "05", ("01.1",)),
    ("06.1", "06", ("05.1",)),
    ("07.1", "07", ("01.1",)),
    ("08.1", "08", ("01.1",)),
    ("09.1", "09", ("01.1",)),
    ("10.1", "10", ()),
    ("12.1", "12", ("01.1",)),
    ("13.1", "13", ("01.1",)),
    ("14.1", "14", ("01.1",)),
    ("15.1", "15", ("01.1",)),
    ("16.1", "16", ("01.1", "11.2")),
    ("17.1", "17", ("07.1",)),
    ("18.1", "18", ("01.1",)),
    ("19.1", "19", ("11.2",)),
    ("20.1", "20", ("01.1",)),
]
CASE_STEP_DEPS = {step: deps for step, _, deps in CASE_STEPS}
CASE_STEP_CASE = {step: case for step, case, _ in CASE_STEPS}

SHARED: dict = {}


def first_usable() -> tuple:
    """Bounded, effect-free check that the shared `first` fixture is still the
    linked profile checkout its dependent cases assume. A tainted or missing
    fixture skips those cases; it is never faked or reported as complete."""
    first = SHARED.get("first")
    if not first or not os.path.isdir(first):
        return False, "the shared first checkout was never created"
    target = os.path.join(first, "target")
    if not os.path.isdir(target) or os.path.islink(target):
        return False, "the shared first checkout's target is no longer a real directory"
    for member in ("debug", "release"):
        if not os.path.islink(os.path.join(target, member)):
            return False, f"the shared first checkout lost its {member} member link"
    return True, ""


class Diagnostic:
    """Diagnostic-mode bookkeeping only. Default (counted) mode never uses it:
    the first failure raises immediately and keeps fail-fast semantics."""

    def __init__(self) -> None:
        self.reset()

    def reset(self) -> None:
        self.mode = False
        self.done: list = []
        self.failures: list = []
        self.skips: list = []
        self.aborted = None

    def enter(self, step: str, fixtures: tuple = ()) -> bool:
        if not self.mode:
            return True
        reasons = []
        for need in CASE_STEP_DEPS[step]:
            if need in self.done:
                continue
            failed = next((item for item in self.failures if item["step"] == need), None)
            skipped = next((item for item in self.skips if item["step"] == need), None)
            if failed is not None:
                reasons.append(f"{need} failed at its {failed['kind']} ({failed['observed'][:200]})")
            elif skipped is not None:
                reasons.append(f"{need} was skipped")
            else:
                reasons.append(f"{need} did not complete")
        for guard in fixtures:
            ok, reason = guard()
            if not ok:
                reasons.append(reason)
        if reasons:
            self.skips.append({"step": step, "case": CASE_STEP_CASE[step], "reasons": reasons})
            print(f"DIAGNOSTIC skip {step}: {'; '.join(reasons)}", flush=True)
            return False
        return True

    def finished(self, step: str) -> None:
        if self.mode:
            self.done.append(step)

    @staticmethod
    def classify(error: BaseException) -> str:
        if isinstance(error, (KeyboardInterrupt, SystemExit)):
            return "abort"
        if isinstance(error, NameError):
            return "dependency"
        if isinstance(error, OracleFailure):
            text = str(error)
            if any(marker in text for marker in DIAGNOSTIC_SAFETY_MARKERS):
                return "abort"
            return "oracle"
        return "abort"

    def failed_step(self, proof, step: str, error: BaseException) -> None:
        if not self.mode:
            raise error
        kind = self.classify(error)
        if kind == "dependency":
            self.skips.append(
                {
                    "step": step,
                    "case": CASE_STEP_CASE[step],
                    "reasons": [f"a prerequisite fixture is missing: {error}"],
                }
            )
            print(f"DIAGNOSTIC skip {step}: prerequisite fixture missing ({error})", flush=True)
            return
        unsettled = CUSTODY.pending(proof) if proof is not None else []
        unclosed = sorted(getattr(proof, "unclosed", set())) if proof is not None else []
        record = {
            "step": step,
            "case": CASE_STEP_CASE[step],
            "kind": kind,
            "observed": str(error)[-2000:],
            "unsettled_children": unsettled,
            "unclosed": unclosed,
        }
        if kind == "abort" or unsettled or unclosed:
            # Safety failures stop the whole diagnostic: the guard/deadline,
            # child closure, image/device/writer custody or the evidence path is
            # no longer trustworthy, so no later case may run.
            self.aborted = record
            print(f"DIAGNOSTIC abort in {step}: {error}", flush=True)
            raise error
        self.failures.append(record)
        print(f"DIAGNOSTIC failure in {step}: {error}", flush=True)


DIAG = Diagnostic()


def diagnostic_record(proof, started_ms, primary, secondary) -> dict:
    """The diagnostic receipt body. It records only what this script itself
    knows and never claims a counted PASS, closure or resource gate."""
    completed = [
        case
        for case in CASE_IDS
        if all(step in DIAG.done for step, known, _ in CASE_STEPS if known == case)
    ]
    failed = sorted({item["case"] for item in DIAG.failures})
    skipped = sorted({item["case"] for item in DIAG.skips})
    unexecuted = [
        case for case in CASE_IDS if case not in completed and case not in failed and case not in skipped
    ]
    outcome = (
        "no_oracle_failures"
        if not DIAG.failures and not DIAG.skips and DIAG.aborted is None and not unexecuted
        else "failures_recorded"
    )
    return {
        "version": "oxide.outputs.proof-diagnostic/1",
        "mode": "diagnostic",
        "qualified": False,
        "outcome": outcome,
        "claims": "diagnostic case coverage only; never a counted native PASS, closure, census or gate claim",
        "source": os.environ.get("OXIDE_PROFILE_HEAD", "unbound"),
        "script": {
            "path": "scripts/test-profile-outputs.py",
            "sha256": perm.sha256_file(os.path.abspath(__file__)),
        },
        "helpers": {
            "scripts/test-oxide-pool-layout.py": perm.sha256_file(
                os.path.join(HERE, "test-oxide-pool-layout.py")
            ),
            "scripts/test-permanent-outputs.py": perm.sha256_file(
                os.path.join(HERE, "test-permanent-outputs.py")
            ),
        },
        "started_ms": started_ms,
        "finished_ms": time.time_ns() // 1_000_000,
        "cases": {
            "total": len(CASE_IDS),
            "reached": list(_CASE_PROGRESS["reached"]),
            "completed": completed,
            "failed": failed,
            "skipped": skipped,
            "unexecuted": unexecuted,
            "aborted": DIAG.aborted,
        },
        "failures": [dict(item) for item in DIAG.failures],
        "skips": [dict(item) for item in DIAG.skips],
        "primary_failure": None if primary is None else str(primary)[-2000:],
        "secondary_errors": list(secondary),
        "custody": {
            "unclosed": sorted(getattr(proof, "unclosed", set())) if proof is not None else [],
            "owned_children_pending": CUSTODY.pending(proof) if proof is not None else [],
            "mount": getattr(proof, "mount", None) if proof is not None else None,
        },
        "limits": "600 s / 512 MiB are owned by the qualified runner; this receipt makes no closure, census or resource claim",
    }


def diagnostic_payload(record: dict) -> tuple:
    """The exact receipt bytes and whether the failure list had to be marked
    INCOMPLETE to stay inside the bound. Nothing is dropped silently."""
    failures = list(record["failures"])
    omitted = 0
    while True:
        body = dict(record)
        body["failures"] = failures
        body["incomplete"] = omitted > 0
        body["failures_omitted"] = omitted
        payload = (json.dumps(body, indent=2, sort_keys=True) + "\n").encode()
        if len(payload) <= DIAGNOSTIC_RECEIPT_BYTES or not failures:
            return payload, omitted > 0
        failures.pop()
        omitted += 1


def write_diagnostic_receipt(record: dict) -> str:
    """Write the ONE uniquely named diagnostic receipt beside the guard receipt.
    It is distinct from the guard receipt and from the counted run's own
    assertion receipt, and it never claims PASS or closure."""
    evidence = evidence_root()
    expect(
        bool(evidence) and os.path.isdir(evidence),
        f"no kept evidence root for the diagnostic receipt: {evidence!r}",
    )
    payload, incomplete = diagnostic_payload(record)
    if len(payload) > DIAGNOSTIC_RECEIPT_BYTES:
        raise OracleFailure(
            f"the diagnostic receipt exceeds {DIAGNOSTIC_RECEIPT_BYTES} bytes even without failure entries; no complete-list claim is made"
        )
    name = f"profile-outputs.diagnostic-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{os.getpid()}.json"
    path = os.path.join(evidence, name)
    with open(path, "x", encoding="utf-8") as handle:
        handle.write(payload.decode())
        handle.flush()
        os.fsync(handle.fileno())
    if incomplete:
        raise OracleFailure(
            f"the diagnostic failure list exceeded {DIAGNOSTIC_RECEIPT_BYTES} bytes; {path} is explicitly INCOMPLETE and the run stops"
        )
    return path


def pure_diagnostic_controls() -> None:
    """Pure/mock controls for the diagnostic state machine. No child, image,
    pool or fixture is touched; nothing is written."""
    saved = (DIAG.mode, list(DIAG.done), list(DIAG.failures), list(DIAG.skips), DIAG.aborted)
    try:
        # 1. Default mode stays fail-fast: the first oracle failure raises.
        DIAG.reset()
        try:
            DIAG.failed_step(None, "02.1", OracleFailure("02: default mode must not step over this"))
            expect(False, "default mode did not re-raise an oracle failure")
        except OracleFailure as error:
            expect("must not step over" in str(error), f"default fail-fast message changed: {error}")
        expect(DIAG.failures == [], f"default mode recorded a diagnostic failure: {DIAG.failures}")
        # 2. Diagnostic mode records the failure and steps over it.
        DIAG.reset()
        DIAG.mode = True
        DIAG.failed_step(None, "02.1", OracleFailure("02: observed A, expected B"))
        expect(len(DIAG.failures) == 1, f"diagnostic did not record the failure: {DIAG.failures}")
        expect(DIAG.failures[0]["kind"] == "oracle", f"oracle failure classified {DIAG.failures[0]}")
        expect(DIAG.failures[0]["case"] == "02", f"failure case {DIAG.failures[0]}")
        # 3. The first failure stays the primary/first recorded fault.
        DIAG.failed_step(None, "03.1", OracleFailure("03: a later, unrelated failure"))
        expect(
            [item["step"] for item in DIAG.failures] == ["02.1", "03.1"],
            f"the recorded fault order changed: {DIAG.failures}",
        )
        expect(DIAG.aborted is None, "a before-first diagnostic abort was recorded")
        # 4. A step whose prerequisite failed is skipped, with the reason.
        expect(not DIAG.enter("16.1"), "a step entered although its prerequisite had not completed")
        expect(
            DIAG.skips and "11.2 did not complete" in "; ".join(DIAG.skips[-1]["reasons"]),
            f"skip reasons {DIAG.skips}",
        )
        expect(not DIAG.enter("06.1"), "a step entered after its prerequisite failed")
        expect(
            "05.1 did not complete" in "; ".join(DIAG.skips[-1]["reasons"]),
            f"skip reasons {DIAG.skips[-1]}",
        )
        # 5. A tainted shared fixture skips dependent cases instead of faking it.
        SHARED.pop("first", None)
        ok, reason = first_usable()
        expect(not ok and "never created" in reason, f"the missing fixture guard said {reason}")
        # 6. Safety failures stop the whole diagnostic.
        for error, kind in (
            (OracleFailure("work deadline reached before the runner's shutdown margin"), "abort"),
            (OracleFailure("custody is unknown for pid 42"), "abort"),
            (RuntimeError("an infrastructure failure"), "abort"),
            (OSError("a disk error"), "abort"),
            (KeyboardInterrupt(), "abort"),
            (NameError("first"), "dependency"),
        ):
            expect(DIAG.classify(error) == kind, f"{error!r} classified {DIAG.classify(error)}, not {kind}")
        DIAG.reset()
        DIAG.mode = True
        try:
            DIAG.failed_step(None, "04.1", OracleFailure("work deadline reached before the runner's shutdown margin"))
            expect(False, "a safety failure did not stop the diagnostic")
        except OracleFailure as error:
            expect("work deadline" in str(error), f"safety abort message changed: {error}")
        expect(DIAG.aborted and DIAG.aborted["kind"] == "abort", f"abort record {DIAG.aborted}")
        expect(DIAG.failures == [], f"a safety abort was recorded as an oracle failure: {DIAG.failures}")
        # 7. The diagnostic receipt is distinct, bounded and never a PASS claim.
        DIAG.reset()
        DIAG.mode = True
        DIAG.failed_step(None, "02.1", OracleFailure("02: first recorded fault"))
        record = diagnostic_record(None, 1234, None, ["a secondary error"])
        expect(record["version"] == "oxide.outputs.proof-diagnostic/1", record["version"])
        expect(record["mode"] == "diagnostic" and record["qualified"] is False, f"{record['mode']}/{record['qualified']}")
        expect(record["outcome"] == "failures_recorded", record["outcome"])
        expect("never a counted native PASS" in record["claims"], record["claims"])
        payload, incomplete = diagnostic_payload(record)
        expect(not incomplete and len(payload) <= DIAGNOSTIC_RECEIPT_BYTES, f"diagnostic payload {len(payload)} / {incomplete}")
        body = json.loads(payload)
        expect(
            "passed" not in body
            and body.get("outcome") != "pass"
            and body.get("qualified") is False
            and body.get("mode") == "diagnostic",
            "the diagnostic receipt claims a PASS",
        )
        # 8. An oversized failure list is marked INCOMPLETE inside the bound,
        #    never silently truncated and never claimed complete.
        oversized = dict(record)
        oversized["failures"] = [{"observed": "x" * 2000} for _ in range(60)]
        payload, incomplete = diagnostic_payload(oversized)
        expect(incomplete and len(payload) <= DIAGNOSTIC_RECEIPT_BYTES, f"oversized payload {len(payload)} / {incomplete}")
        body = json.loads(payload)
        expect(body["incomplete"] is True and body["failures_omitted"] > 0, f"overflow marking {body.get('failures_omitted')}")
        expect(len(body["failures"]) + body["failures_omitted"] == 60, "the overflow record lost its count")
    finally:
        DIAG.reset()
        DIAG.mode, done, failures, skips, aborted = saved
        DIAG.done, DIAG.failures, DIAG.skips, DIAG.aborted = done, failures, skips, aborted


def run(proof: Proof, repo: str) -> None:
    pure_diagnostic_controls()
    pure_registry_controls()
    pure_detach_record_control()
    pure_attachment_facts_control()
    pure_slot_state_control()
    pure_reserve_stop_control()
    pure_child_custody_control()
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    print(f"profile-outputs proof {os.getpid()} source {source}", flush=True)
    # Independent sequential staging from authenticated pinned source, each in
    # its own target, copied out and then discarded.
    # Explicit CPU controls for a 6 CPU / 8 GiB planning request. The two
    # stages are sequential and each is bounded to 6 Cargo jobs (passed
    # explicitly to the shared helper, which sanitizes CARGO_BUILD_* first).
    base_src = source_at(proof, repo, BASE, "base")
    base = layout.stage_at(proof, base_src, "base", jobs=STAGE_JOBS)
    shutil.rmtree(base_src)
    candidate = layout.stage_at(proof, repo, "candidate", jobs=STAGE_JOBS)
    expect(perm.sha256_file(base) != perm.sha256_file(candidate), "base and candidate binaries are identical")
    expect(capabilities(base) == 3, f"base binary is not a state-3 binary")
    expect(capabilities(candidate) == 4, "candidate binary is not a state-4 binary")
    print(f"base {perm.sha256_file(base)} candidate {perm.sha256_file(candidate)}", flush=True)
    proof.binary = candidate

    pool = os.path.join(proof.root, "pool")
    home = os.path.join(proof.root, "home")
    cargo_home = os.path.join(proof.root, "cargo-home")
    for directory in (home, cargo_home):
        os.makedirs(directory)
    proof.env = perm.proof_env(pool, home, cargo_home)
    # Every fixture Cargo child (oxide build/test/bench/doc/check/metadata)
    # inherits this bound. At most two gated fixture writers run at once, so
    # the fixture peak is 2 x FIXTURE_JOBS = 4 jobs; staging is sequential and
    # peaked earlier at STAGE_JOBS = 6. The fake clean `cargo` compiles
    # nothing, and one `cargo metadata` process does not compile, so the whole
    # proof peaks at 6 concurrent Cargo jobs.
    proof.env["CARGO_BUILD_JOBS"] = str(FIXTURE_JOBS)
    register(proof, pool)

    # Case 10 (base half): a BASE-produced whole-target /2 state and warm slot
    # key prepared under the base-compatible registration before ANY candidate
    # command runs in this pool. The candidate must later reuse that exact
    # slot, receipt and link unchanged.
    legacy = make_checkout(proof, "legacy")
    result = proof.run([base, "outputs", "adopt"], cwd=legacy, env=proof.env, timeout=120)
    expect(result.returncode == 0, f"10: base whole-target adopt failed: {result.stderr.decode(errors='replace')}")
    legacy_root = root_of(pool, legacy)
    expect(
        legacy_root.startswith(os.path.join(proof.mount, "oxide-outputs") + os.sep),
        f"10: the base legacy root {legacy_root} is not on the registered volume",
    )
    expect(os.path.islink(os.path.join(legacy, "target")), "10: base did not publish a whole-target link")
    expect(item(pool, legacy, "version") == "oxide.outputs.transition/2", "10: base receipt is not /2")
    result = proof.run([base, "build"], cwd=legacy, env=proof.env, timeout=180)
    expect(result.returncode == 0, f"10: base build failed: {result.stderr.decode(errors='replace')[-2000:]}")
    base_slot, _ = perm.slot_state(result.stderr.decode(errors="replace"))
    # A7: keep the EXACT base receipt bytes and link facts, so the candidate's
    # no-mutation claim is a byte comparison rather than a version/member spot
    # check.
    base_receipt_bytes = perm.read_bytes(perm.receipt_path(pool, legacy))
    base_link_text = link_at(os.path.join(legacy, "target"))
    if DIAG.enter("11.1", fixtures=()):
        try:
            # ----- case 11: explicit migration of a base-created checkout ---------
            case_reached("11")
            # The base binary can create its committed whole-target /2 state only
            # before any candidate command runs in this pool, so this base state is
            # prepared under the SAME base-compatible registration as case 10's, and
            # the candidate migrates it only after case 10's base warm check.
            migrated = make_checkout(proof, "migrate")
            base_legacy(proof, base, pool, migrated)
            # The base binary's committed whole-target /2 state is made source-shaped
            # BEFORE any migration: a root/1 marker and no nonce in the receipt or
            # registry, so the only path that can reach the profile layout is the
            # candidate's explicit migrate_layout.
            migrated_root = make_root_one_fixture(pool, migrated)
            expect(
                migrated_root.startswith(os.path.join(proof.mount, "oxide-outputs") + os.sep),
                f"11: the base legacy root {migrated_root} is not on the registered volume",
            )
            expect(
                perm.read_json(perm.receipt_path(pool, migrated))["version"] == "oxide.outputs.transition/2",
                "11: the source-shaped fixture is not a /2 receipt",
            )
            marker_path = os.path.join(migrated_root, MARKER)
            root_ino = os.stat(migrated_root).st_ino
            # A7: a tiny NONEMPTY sentinel in the root payload, plus the immutable
            # marker facts, so the migration is shown to leave real bytes and inodes
            # alone rather than only the entry names.
            marker_before = perm.read_json(marker_path)
            sentinel = os.path.join(migrated_root, "migration-sentinel")
            with open(sentinel, "wb") as handle:
                handle.write(b"migration sentinel bytes")
            sentinel_ino = os.stat(sentinel).st_ino
            expect(perm.read_bytes(sentinel) == b"migration sentinel bytes", "11: the sentinel was not written")
            payload = sorted(os.listdir(migrated_root))
            # Persist the exact bounded PRE-migration registry/receipt/marker bytes in
            # OWN evidence before the migration and its oracle.
            evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
            if evidence:
                for name, path in (
                    ("registry", os.path.join(pool, "oxide-managed", "outputs", "registry.json")),
                    ("receipt", perm.receipt_path(pool, migrated)),
                    ("marker", marker_path),
                ):
                    with open(os.path.join(evidence, f"case11-{name}-pre.json"), "wb") as handle:
                        handle.write(perm.read_bytes(path))

            # Case 10 (candidate half): only after BOTH base-created legacy states
            # (this warm-slot checkout and case 11's migration checkout) exist does the
            # first candidate command run in this pool.
            code, _, stderr = oxide(proof, ["build"], "10 candidate warm on base slot", legacy)
            expect(code == 0, f"10: candidate build on the base legacy slot failed: {stderr}")
            candidate_slot, candidate_state = perm.slot_state(stderr)
            expect(candidate_state == "warm", f"10: candidate did not reuse the base key: {summary(stderr)}")
            expect(candidate_slot == base_slot, f"10: candidate changed the base slot {base_slot} -> {candidate_slot}")
            expect(os.path.islink(os.path.join(legacy, "target")), "10: the base legacy link changed")
            expect(link_at(os.path.join(legacy, "target")) == legacy_root, "10: the base legacy link text changed")
            expect(item(pool, legacy, "version") == "oxide.outputs.transition/2", "10: the base receipt was rewritten")
            base_links = item(pool, legacy, "links")
            expect(base_links[0]["member"] == "target", "10: the base placement changed")
            expect(
                perm.read_bytes(perm.receipt_path(pool, legacy)) == base_receipt_bytes,
                "10: the candidate rewrote the base receipt bytes",
            )
            expect(
                link_at(os.path.join(legacy, "target")) == base_link_text,
                "10: the candidate changed the base link text",
            )
            expect(
                "uses the whole-target layout" in stderr and "oxide outputs adopt" in stderr,
                f"10: the candidate did not print the one adopt notice: {stderr}",
            )
            DIAG.finished("11.1")
        except BaseException as error:
            DIAG.failed_step(proof, "11.1", error)

    if DIAG.enter("11.2", fixtures=()):
        try:
            # ----- case 11 (continued): the candidate migration runs only now -----
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "11 migrate", migrated)
            expect(code == 0, f"11: candidate migration failed: {stderr}")
            expect_layout(migrated, {"debug": f"link:{migrated_root}", "release": f"link:{migrated_root}"})
            upgraded = perm.read_json(marker_path)
            expect(upgraded["version"] == "oxide.outputs.root/2", "11: marker not upgraded to root/2")
            nonce = upgraded["nonce"]
            expect(len(nonce) == 32, "11: migrated nonce is not 128-bit hex")
            # A7: only version and nonce may change; owner, recipe, kind, volume and
            # the ORIGINAL root-creation generation and timestamp are immutable.
            for field in ("checkout", "checkout_id", "label", "recipe", "kind", "volume_uuid", "generation", "created_ms"):
                expect(
                    upgraded.get(field) == marker_before.get(field),
                    f"11: the root/1 upgrade changed the immutable marker field {field!r}: "
                    f"{marker_before.get(field)!r} -> {upgraded.get(field)!r}",
                )
            expect(
                os.stat(sentinel).st_ino == sentinel_ino
                and perm.read_bytes(sentinel) == b"migration sentinel bytes",
                "11: the migration changed the root payload sentinel",
            )
            migrated_receipt = receipt(pool, migrated)
            expect({p["selected"]["nonce"] for p in migrated_receipt["links"] if p.get("selected")} == {nonce}, "11: members do not share the upgraded nonce")
            expect(os.stat(migrated_root).st_ino == root_ino, "11: the root inode changed")
            present = {name for name in os.listdir(migrated_root) if name in payload}
            expect(present == set(payload), f"11: root payload changed: {present} vs {payload}")
            expect(not any(name.endswith(".oxide-old-2") for name in os.listdir(migrated_root)), "11: the marker backup was kept")
            registry_path = os.path.join(pool, "oxide-managed", "outputs", "registry.json")
            registry = perm.read_json(registry_path)
            checkout_id = perm.hashlib.sha256(migrated.encode()).hexdigest()
            rows = [row for row in registry["rows"] if row["checkout_id"] == checkout_id]
            # Persist the exact bounded registry/receipt/marker bytes into OWN evidence
            # BEFORE the oracle so a failure leaves the real state for diagnosis.
            evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
            if evidence:
                for name, path in (
                    ("registry", registry_path),
                    ("receipt", perm.receipt_path(pool, migrated)),
                    ("marker", marker_path),
                ):
                    with open(os.path.join(evidence, f"case11-{name}-post.json"), "wb") as handle:
                        handle.write(perm.read_bytes(path))
            validate_migrated_registry(rows, migrated_root, nonce, upgraded.get("volume_uuid"), upgraded["generation"])
            expect(migrated_receipt["version"] == "oxide.outputs.transition/3", "11: migrated receipt version")

            # Explicit positive control BEFORE any external-root oracle: the
            # registered removable volume is really selected, so an internal-pool
            # fallback can never masquerade as external-placement evidence.
            expect(proof.mount is not None, "the private image is not mounted")
            expect(os.path.isfile(perm.registration_path(pool)), "no removable registration")
            selected = make_checkout(proof, "selected-removable")
            os.makedirs(os.path.join(selected, "target"))
            adopt(proof, selected, "selected-removable adopt")
            selected_root = root_of(pool, selected)
            expect(
                selected_root.startswith(os.path.join(proof.mount, "oxide-outputs") + os.sep),
                f"the selected root {selected_root} is not under {proof.mount}/oxide-outputs",
            )
            code, _, stderr = oxide(proof, ["build"], "selected-removable build", selected)
            expect(code == 0, f"selected-removable build failed: {stderr}")
            expect(
                "fallback" not in summary(stderr),
                f"the registered removable pool was not selected: {summary(stderr)}",
            )
            expect(
                os.readlink(member_link(selected, "debug")) == member_destination(selected_root, "debug"),
                "the member link does not select the registered external root",
            )
            # A SEPARATE deliberate low-reserve control: a reserve above the image's
            # free space must fall back to the internal pool with an honest warning and
            # must not claim an external placement.
            # A SEPARATE deliberate low-reserve control: the private pool's recorded
            # registration reserve is temporarily raised above the image's free space,
            # then its exact bytes are restored. No second image or pool is created and
            # no real drive or host setting is touched.
            registration_file = perm.registration_path(pool)
            original_registration = perm.read_bytes(registration_file)
            registration = json.loads(original_registration)
            registration["reserve_bytes"] = LOW_RESERVE_MIB * 1024 * 1024
            with open(registration_file, "w", encoding="utf-8") as handle:
                json.dump(registration, handle, indent=2)
                handle.write("\n")
            os.chmod(registration_file, 0o600)
            low = make_checkout(proof, "low-reserve")
            os.makedirs(os.path.join(low, "target"))
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "low-reserve adopt", low)
            expect(code == 0, f"low-reserve adopt failed: {stderr}")
            low_root = root_of(pool, low)
            expect(
                low_root.startswith(low + os.sep + ".oxide-outputs" + os.sep),
                f"the low-reserve control selected {low_root} instead of the checkout-local fallback",
            )
            code, _, stderr = oxide(proof, ["build"], "low-reserve build", low)
            expect(code == 0, f"low-reserve build failed: {stderr}")
            expect(
                "not above the" in stderr and "reserve" in stderr,
                f"the low-reserve control did not report its fallback honestly: {stderr}",
            )
            # Restore the exact recorded registration bytes this fixture changed.
            with open(registration_file, "wb") as handle:
                handle.write(original_registration)
            os.chmod(registration_file, 0o600)
            expect(
                perm.read_bytes(registration_file) == original_registration,
                "the low-reserve control did not restore the recorded registration",
            )

            # A separate parent-ignore control: a directory-only `/target/` rule
            # cannot cover the published non-directory link, so the automatic
            # whole-target conversion refuses it. Git decides that question itself and
            # answers "does not ignore `target`" while the path is still absent (a
            # trailing-slash rule only matches a directory it can see), so the proof
            # asserts THIS reachable refusal verbatim rather than a branch that the
            # absent-target geometry never reaches. The same rule stays a valid parent
            # ignore for profile members under the real target directory, which is why
            # the real-empty explicit profile adopt that follows must succeed.
            parent = make_checkout(proof, "parent-ignore", PARENT_IGNORE_APP)
            parent_target = os.path.join(parent, "target")
            code, _, stderr = oxide(proof, ["build"], "parent-ignore auto-adopt", parent)
            expect(code == 1, f"the uncovariable directory-only rule did not refuse with exit 1: {code}: {stderr}")
            expect(
                f"refusing to convert {parent_target} to an owned link" in stderr,
                f"the refusal did not name the whole-target link it declined: {stderr}",
            )
            expect(
                "Git does not ignore `target`" in stderr,
                f"the refusal did not report Git's own non-directory answer: {stderr}",
            )
            expect(
                "add `/target` (or `target`), not `target/`" in stderr,
                f"the refusal did not name the non-directory remedy: {stderr}",
            )
            expect("nothing was changed" in stderr, f"the refusal did not report that nothing changed: {stderr}")
            # No authority change: no receipt, no link, no owned root, no target.
            expect(not os.path.exists(receipt_file(pool, parent)), "the refused whole-target conversion wrote a receipt")
            expect(not os.path.lexists(parent_target), f"the refused conversion left {parent_target} in place")
            expect(
                not os.path.lexists(os.path.join(parent, ".oxide-outputs")),
                "the refused conversion created a checkout-local owned root",
            )
            # The same checkout then succeeds through the explicitly requested profile
            # conversion over a REAL empty target: the directory-only rule is a valid
            # parent ignore where the member link really sits under a directory.
            os.makedirs(parent_target)
            adopt(proof, parent, "parent-ignore adopt")
            parent_root = root_of(pool, parent)
            expect_layout(parent, {"debug": f"link:{parent_root}"})
            code, _, stderr = oxide(proof, ["build"], "parent-ignore build", parent)
            expect(code == 0, f"the adopted parent-ignore checkout did not build: {stderr}")
            expect("fallback" not in summary(stderr), f"parent-ignore: external placement unexpectedly fell back: {summary(stderr)}")
            expect_layout(parent, {"debug": f"link:{parent_root}"})

            # Nonvacuous top-level preservation on its own checkout, so the cleaned
            # `first` checkout never carries a foreign entry.
            top = make_checkout(proof, "top-level")
            os.makedirs(os.path.join(top, "target"))
            control_file = os.path.join(top, "target", "oxide-proof-top-level-control")
            with open(control_file, "wb") as handle:
                handle.write(b"control")
            adopt(proof, top, "top-level adopt")
            code, _, stderr = oxide(proof, ["build"], "top-level build", top)
            expect(code == 0, f"top-level build failed: {stderr}")
            expect(os.path.isfile(control_file) and not os.path.islink(control_file), "the ordinary top-level control file was removed or replaced")
            expect(perm.read_bytes(control_file) == b"control", "the top-level control file changed")
            expect_layout(top, {"debug": f"link:{root_of(pool, top)}"})
            DIAG.finished("11.2")
        except BaseException as error:
            DIAG.failed_step(proof, "11.2", error)

    if DIAG.enter("01.1", fixtures=()):
        try:
            # ----- case 01: a real target with a debug member link -----------------
            case_reached("01")
            first = make_checkout(proof, "first")
            SHARED["first"] = first
            # A real EMPTY target selects the explicit profile adopt: an absent target
            # would let the automatic whole-target first conversion run, and building
            # first would fill target/debug so adoption would keep it real.
            os.makedirs(os.path.join(first, "target"))
            adopt(proof, first, "01 adopt")
            root = root_of(pool, first)
            expect(root.startswith(os.path.join(proof.mount, "oxide-outputs") + os.sep), f"01: external root {root} is not on the registered volume")
            expect_layout(first, {"debug": f"link:{root}", "release": f"link:{root}"})
            code, _, stderr = oxide(proof, ["build"], "01 build", first)
            expect(code == 0, f"01: build through the profile layout failed: {stderr}")
            expect("fallback" not in summary(stderr), f"01: external placement unexpectedly fell back: {summary(stderr)}")
            expect_layout(first, {"debug": f"link:{root}", "release": f"link:{root}"})
            # A real executable control: `run` executes the artifact of its own linked
            # member, not a global root assumption.
            code, stdout, stderr = oxide(proof, ["run"], "01 run", first)
            expect(code == 0, f"01: run failed: {stderr}")
            expected_exe = member_destination(root, "debug")
            expect(
                "profile-app 7 " in stdout and expected_exe in stdout,
                f"01: run did not execute the debug member artifact {expected_exe}: {stdout}",
            )
            # Cargo's own top-level metadata may live in the redirected build directory;
            # it is asserted real only when it is present at the target root.
            for name in ("CACHEDIR.TAG", ".rustc_info.json"):
                path = os.path.join(first, "target", name)
                if os.path.exists(path):
                    expect(
                        os.path.isfile(path) and not os.path.islink(path),
                        f"01: {path} is not a real regular file",
                    )
            report = perm.plan(proof, first, "01 plan")
            expect(report["layout"] == "profile", f"01: plan layout {report['layout']}")
            members = {entry["member"]: entry["state"] for entry in report["members"]}
            expect(members.get("debug") == "owned-link", f"01: plan members {members}")
            expect(fence_in(pool) == STATE4_FENCE, f"01: fence {fence_in(pool)}")
            expect(item(pool, first, "version") == "oxide.outputs.transition/3", "01: receipt version")
            registry = perm.read_json(os.path.join(pool, "oxide-managed", "outputs", "registry.json"))
            expect(registry["schema"] == 2, f"01: registry schema {registry['schema']}")
            first_members = sorted(
                {
                    row["member"]
                    for row in registry["rows"]
                    if row["checkout_id"] == perm.hashlib.sha256(first.encode()).hexdigest()
                }
            )
            expect(
                first_members == ["debug", "release"],
                f"01: registry members are {first_members}, not the debug/release pair",
            )
            DIAG.finished("01.1")
        except BaseException as error:
            DIAG.failed_step(proof, "01.1", error)

    if DIAG.enter("02.1", fixtures=(first_usable,)):
        try:
            # ----- case 02: a member mutation waits for a gated writer ------------
            case_reached("02")
            gate = os.path.join(proof.root, "gate2")
            os.makedirs(gate)
            env = {**proof.env, "OXIDE_PROFILE_GATE": gate}
            child = perm.start_child(proof, [proof.binary, "build"], first, env)
            CUSTODY.track(child, gate, "02 gated build")
            wait_started(gate, "02 gated build start")
            before = custody_snapshot(pool, first)
            second = perm.start_child(proof, [proof.binary, "outputs", "adopt"], first, proof.env)
            CUSTODY.track(second, None, "02 waited adopt")
            observed_wait(second, before, pool, first, "02")
            perm.stop_gate(gate, os.path.join(gate, "release"))
            code, _, _ = perm.finish_child(proof, child, "02 gated build")
            CUSTODY.finished(child)
            expect(code == 0, f"02: gated build exited {code}")
            code, _, _ = perm.finish_child(proof, second, "02 waited adopt")
            CUSTODY.finished(second)
            expect(code == 0, "02: the waiting adopt failed")
            expect(custody_snapshot(pool, first) == before, "02: the waited adopt changed authority")
            expect_layout(first, {"debug": f"link:{root}", "release": f"link:{root}"})
            DIAG.finished("02.1")
        except BaseException as error:
            DIAG.failed_step(proof, "02.1", error)

    if DIAG.enter("03.1", fixtures=(first_usable,)):
        try:
            # ----- case 03: test / bench / bench debug / doc mapping --------------
            case_reached("03")
            # A3: the mapping is decided by MEMBER-identifying product facts -- the
            # member set the product reports and the member payload it actually writes
            # -- never by an exit code, and never by cross-recipe slot equality. A
            # leased slot id keys (recipe, layout, member), so two commands can map to
            # the SAME member and legitimately lease DIFFERENT slots when their recipe
            # differs (oxide#068/#106). Recipe equality is asserted separately as its
            # own fact; it is not used as member evidence.
            code, _, stderr = oxide(proof, ["build"], "03 debug build", first)
            expect(code == 0, f"03: debug build failed: {stderr}")
            expect(
                summary_members(stderr) == {"debug"},
                f"03: build did not report the debug member: {summary(stderr)}",
            )
            expect(
                summary_recipe(stderr) == "quick",
                f"03: build did not use the quick recipe: {summary(stderr)}",
            )
            debug_slot, debug_state = perm.slot_state(stderr)
            expect("fallback" not in summary(stderr), f"03: external placement unexpectedly fell back: {summary(stderr)}")
            # test and doc share the build's recipe AND member, so the exact keyed slot
            # must be reused warm: a genuinely valid same-recipe identity check.
            code, _, stderr = oxide(proof, ["test"], "03 test", first)
            expect(code == 0, f"03: test failed: {stderr}")
            expect(
                summary_members(stderr) == {"debug"},
                f"03: test did not report the debug member: {summary(stderr)}",
            )
            expect(
                summary_recipe(stderr) == "quick",
                f"03: test did not use the quick recipe: {summary(stderr)}",
            )
            test_slot, test_state = perm.slot_state(stderr)
            expect(
                test_slot == debug_slot and test_state == "warm",
                f"03: test did not reuse the debug member's quick slot: {debug_slot} vs {test_slot} ({test_state})",
            )
            code, _, stderr = oxide(proof, ["doc"], "03 doc", first)
            expect(code == 0, f"03: doc failed: {stderr}")
            expect(
                summary_members(stderr) == {"debug"},
                f"03: doc did not report the debug member: {summary(stderr)}",
            )
            expect(
                summary_recipe(stderr) == "quick",
                f"03: doc did not use the quick recipe: {summary(stderr)}",
            )
            doc_slot, _ = perm.slot_state(stderr)
            expect(
                doc_slot == debug_slot,
                f"03: doc did not reuse the debug member's quick slot ({doc_slot} vs {debug_slot})",
            )
            # Plain bench maps to the RELEASE member: it must report release, key its
            # own slot, write the release payload, and leave the debug payload alone.
            debug_payload_before = member_payload_state(root, "debug")
            code, _, stderr = oxide(proof, ["bench"], "03 bench", first)
            expect(code == 0, f"03: bench failed: {stderr}")
            expect(
                summary_members(stderr) == {"release"},
                f"03: plain bench did not report the release member: {summary(stderr)}",
            )
            release_slot, release_state = perm.slot_state(stderr)
            expect(
                release_slot != debug_slot and release_state == "cold",
                f"03: plain bench did not key its own release slot: {release_slot} ({release_state}) vs {debug_slot}",
            )
            expect(
                member_payload_state(root, "release") != ("absent",),
                "03: plain bench wrote no release member payload",
            )
            expect(
                member_payload_state(root, "debug") == debug_payload_before,
                "03: plain bench touched the debug member payload",
            )
            # `bench --profile debug` maps to the DEBUG member with the DEBUG recipe.
            # Its recipe differs from the plain build's, so a different slot id is
            # legitimate and is NOT evidence of a wrong mapping; the member set is, and
            # the release member must stay untouched.
            release_payload_before = member_payload_state(root, "release")
            code, _, stderr = oxide(proof, ["bench", "--profile", "debug"], "03 bench debug", first)
            expect(code == 0, f"03: bench debug failed: {stderr}")
            expect(
                summary_members(stderr) == {"debug"},
                f"03: bench --profile debug did not map to the debug member: {summary(stderr)}",
            )
            expect(
                summary_recipe(stderr) == "debug",
                f"03: bench --profile debug did not use the debug recipe: {summary(stderr)}",
            )
            bench_debug_slot, bench_debug_state = perm.slot_state(stderr)
            expect(
                bench_debug_slot != release_slot,
                f"03: bench --profile debug reused the release member's slot {release_slot} ({bench_debug_state})",
            )
            expect(
                member_payload_state(root, "release") == release_payload_before,
                "03: bench --profile debug touched the release member payload",
            )
            expect(os.path.isdir(os.path.join(first, "target", "doc")), "03: target/doc is missing")
            expect(not os.path.islink(os.path.join(first, "target", "doc")), "03: target/doc is a link")
            for member in ("debug", "release"):
                expect_layout(first, {member: f"link:{root}"})
            # Cargo's intermediates live in the leased slot's profile build directory
            # (for example <slot>/quick-6/build/release/deps), never in
            # oxide-outputs/<root>/<member>, so no deps path is asserted here. The
            # member mapping is proved above by the product's member report, the payload
            # facts and the untouched-opposite-member controls; all this adds is that
            # the release member payload root is a real directory once plain bench has
            # written it.
            expect(
                os.path.isdir(os.path.join(root, "release")),
                "03: the release member payload root is not a real directory",
            )
            DIAG.finished("03.1")
        except BaseException as error:
            DIAG.failed_step(proof, "03.1", error)

    if DIAG.enter("04.1", fixtures=(first_usable,)):
        try:
            # ----- case 04: a host --target member --------------------------------
            case_reached("04")
            host = proof.run(["rustc", "-vV"], cwd=first, env=proof.env, timeout=30).stdout.decode()
            triple = next(line.split(": ", 1)[1] for line in host.splitlines() if line.startswith("host: "))
            code, _, stderr = oxide(proof, ["build", "--target", triple], "04 host target", first)
            expect(code == 0, f"04: host-target build failed: {stderr}")
            expect_layout(first, {f"{triple}/debug": f"link:{root}"})
            report = perm.plan(proof, first, "04 plan")
            expect(f"{triple}/debug" in {entry["member"] for entry in report["members"]}, "04: plan misses the triple member")
            DIAG.finished("04.1")
        except BaseException as error:
            DIAG.failed_step(proof, "04.1", error)

    if DIAG.enter("05.1", fixtures=(first_usable,)):
        try:
            # ----- case 05: an unknown target refuses only when linked ------------
            case_reached("05")
            unknown = make_checkout(proof, "unknown-target")
            # A11 / case05 cause: the "unlinked" premise must be POSITIVE. Automatic
            # first conversion (oxide#121, outputs::auto_adopts) adopts a checkout
            # whose target is positively ABSENT under a usable permanent registration,
            # and it keeps that conversion even when Cargo then rejects the JSON spec
            # below. A real target directory is not eligible, so this checkout stays
            # unconverted and the absent-receipt oracle actually says something about
            # the unlinked path instead of about a conversion the product was right to
            # make.
            os.makedirs(os.path.join(unknown, "target"))
            expect(
                not os.path.exists(receipt_file(pool, unknown)),
                "05: the unlinked unknown-target fixture already has an output receipt",
            )
            code, _, stderr = oxide(proof, ["build", "--target", "targets/spec.json"], "05 unlinked", unknown)
            expect("not a plain triple" not in stderr, f"05: unlinked checkout refused a JSON target: {stderr}")
            expect(
                not os.path.exists(receipt_file(pool, unknown)),
                "05: an unlinked unknown-target build wrote an output receipt",
            )
            # A failed unknown-target build may leave a partial target; start the
            # adopt from a positively empty real target so only the member links are
            # what the assertions see.
            shutil.rmtree(os.path.join(unknown, "target"), ignore_errors=True)
            os.makedirs(os.path.join(unknown, "target"))
            adopt(proof, unknown, "05 adopt")
            code, _, stderr = oxide(proof, ["build", "--target", "targets/spec.json"], "05 linked", unknown)
            expect(code != 0 and "not a plain triple" in stderr, f"05: linked unknown target was not refused: {stderr}")
            expect_layout(unknown, {"debug": f"link:{root_of(pool, unknown)}"})
            # An external subcommand has opaque output directories. A2: the positive
            # control comes FIRST and is established on a checkout that is positively
            # unconverted (a real empty target prevents automatic whole-target
            # adoption), so the later absence really is the linked refusal. The
            # recording tool is installed under BOTH names a Cargo external subcommand
            # can be looked up as, and it records its exact argv.
            fake_bin = os.path.join(proof.root, "fake-bin")
            os.makedirs(fake_bin, exist_ok=True)
            external_marker = os.path.join(proof.root, "external-tool-ran")
            fake_body = "#!/bin/sh\nprintf '%s' \"$*\" > \"$OXIDE_FAKE_EXTERNAL_MARKER\"\nexit 0\n"
            for name in ("nextest", "cargo-nextest"):
                fake = os.path.join(fake_bin, name)
                with open(fake, "w", encoding="utf-8") as handle:
                    handle.write(fake_body)
                os.chmod(fake, 0o755)
            fake_env = {
                **proof.env,
                "PATH": fake_bin + os.pathsep + proof.env.get("PATH", ""),
                "OXIDE_FAKE_EXTERNAL_MARKER": external_marker,
            }
            plain = make_checkout(proof, "unlinked-external")
            os.makedirs(os.path.join(plain, "target"))
            code, _, stderr = perm.oxide_env(proof, ["nextest", "run", "--help"], "05 unlinked external", plain, fake_env)
            expect(code == 0, f"05: the positive fake external tool did not run in an unlinked checkout: {stderr}")
            expect(
                # Cargo's own external-subcommand convention: the tool is invoked
            # as `<tool> nextest run --help`, so the recorded argv carries the
            # subcommand token as well (pinned native8 observed it directly).
            perm.read_bytes(external_marker) == b"nextest run --help",
                f"05: the unlinked external tool's argv changed: {perm.read_bytes(external_marker)!r}",
            )
            expect(
                not os.path.exists(receipt_file(pool, plain)),
                "05: the unconverted external control wrote an output receipt",
            )
            os.remove(external_marker)
            # Now the linked checkout: the same installed tool must not run at all.
            code, _, stderr = perm.oxide_env(proof, ["nextest", "run"], "05 linked external", unknown, fake_env)
            expect(code != 0 and "not a command whose profile outputs Oxide models" in stderr, f"05: linked external command was not refused: {stderr}")
            expect(not os.path.exists(external_marker), "05: the external tool ran in a linked checkout")
            DIAG.finished("05.1")
        except BaseException as error:
            DIAG.failed_step(proof, "05.1", error)

    if DIAG.enter("06.1", fixtures=(first_usable,)):
        try:
            # ----- case 06: opaque custom-profile refusal -------------------------
            case_reached("06")
            code, _, stderr = oxide(proof, ["build", "--profile", "custom"], "06 modelled custom", first)
            expect(code != 0 and "not an Oxide profile" in stderr, f"06: modelled custom profile was not refused: {stderr}")
            # Case 05's unlinked run wrote the marker; clear it first so this absence
            # check is about case 06's refusal, not leftover state.
            if os.path.exists(external_marker):
                os.remove(external_marker)
            authority_before = {
                "receipt": perm.sha256_file(perm.receipt_path(pool, first)),
                "links": {member: os.readlink(member_link(first, member)) for member in ("debug", "release")},
                "root_entries": sorted(os.listdir(root)),
            }
            code, _, stderr = perm.oxide_env(proof, ["nextest", "--profile", "custom"], "06 opaque custom", first, fake_env)
            # A11: main.rs treats an opaque command as unmodelled and calls
            # outputs::refuse_unknown_layout, whose Err maps to exit 1. Require that
            # exact status and the two source-defined semantic parts (the unmodelled
            # reason naming the command, and the declare-or-detach remedy), never an
            # arbitrary nonzero.
            expect(code == 1, f"06: an opaque tool's custom profile exited {code}, not 1: {stderr}")
            expect(
                "`nextest` is not a command whose profile outputs Oxide models" in stderr,
                f"06: the unmodelled-command reason changed: {stderr}",
            )
            expect(
                "will not guess where this command writes" in stderr
                and "delete its outputs with `oxide exec --output <member>`" not in stderr,
                f"06: the refusal remedy changed: {stderr}",
            )
            expect(
                "declare its outputs with `oxide exec --output <member>`" in stderr
                and "oxide outputs detach" in stderr,
                f"06: the declare-or-detach remedy is missing: {stderr}",
            )
            expect(not os.path.exists(external_marker), "06: the opaque tool ran in a linked checkout")
            expect(
                perm.sha256_file(perm.receipt_path(pool, first)) == authority_before["receipt"]
                and {member: os.readlink(member_link(first, member)) for member in ("debug", "release")}
                == authority_before["links"]
                and sorted(os.listdir(root)) == authority_before["root_entries"],
                "06: the opaque refusal changed the checkout authority",
            )
            DIAG.finished("06.1")
        except BaseException as error:
            DIAG.failed_step(proof, "06.1", error)

    if DIAG.enter("07.1", fixtures=(first_usable,)):
        try:
            # ----- case 07: exec declarations -------------------------------------
            case_reached("07")
            script = os.path.join(first, "declared.sh")
            exec_argv = os.path.join(proof.root, "exec-argv")
            with open(script, "w", encoding="utf-8") as handle:
                handle.write("#!/bin/sh\nprintf '%s' \"$*\" > \"$OXIDE_EXEC_ARGV\"\nexit 0\n")
            os.chmod(script, 0o755)
            exec_env = {**proof.env, "OXIDE_EXEC_ARGV": exec_argv}
            code, _, stderr = oxide(proof, ["exec", "--", script], "07 undeclared exec", first)
            expect(code != 0 and "declare" in stderr, f"07: undeclared exec was not refused: {stderr}")
            code, _, stderr = perm.oxide_env(
                proof,
                ["exec", "--output", "debug", "--", script, "alpha", "beta"],
                "07 declared exec",
                first,
                exec_env,
            )
            expect(code == 0, f"07: declared exec failed: {stderr}")
            expect(
                perm.read_bytes(exec_argv) == b"alpha beta",
                f"07: the declared tool's argv changed: {perm.read_bytes(exec_argv)!r}",
            )
            code, _, stderr = oxide(proof, ["exec", "--output", "custom", "--", script], "07 custom exec", first)
            expect(code != 0 and "not an output member" in stderr, f"07: custom declaration was accepted: {stderr}")
            # A3: the complete declaration must run with the tool's environment and
            # produce FRESH exact argv. The recording tool ends with `exit 0`, so an
            # exit code alone could be an unconditional success after a failed
            # redirection; the recorded argv file is therefore removed first and then
            # required to exist with exactly the expected bytes.
            if os.path.exists(exec_argv):
                os.remove(exec_argv)
            code, _, stderr = perm.oxide_env(
                proof,
                ["exec", "--output", "debug", "--output", "release", "--", script, "gamma"],
                "07 complete exec",
                first,
                exec_env,
            )
            expect(code == 0, f"07: complete declaration failed: {stderr}")
            expect(
                os.path.isfile(exec_argv) and perm.read_bytes(exec_argv) == b"gamma",
                f"07: the complete declaration did not run the tool with its own argv: "
                f"{perm.read_bytes(exec_argv) if os.path.isfile(exec_argv) else 'no recorded argv'}",
            )
            DIAG.finished("07.1")
        except BaseException as error:
            DIAG.failed_step(proof, "07.1", error)

    if DIAG.enter("08.1", fixtures=()):
        try:
            # ----- case 08: real nonempty kept, empty converted -------------------
            case_reached("08")
            mixed = make_checkout(proof, "mixed")
            # A real NONEMPTY release member must be kept; debug is absent, so the
            # explicit adopt links it. No build runs before adoption: a pre-build would
            # also fill target/debug and both members would stay real.
            os.makedirs(os.path.join(mixed, "target", "release"), exist_ok=True)
            with open(os.path.join(mixed, "target", "release", "keep"), "wb") as handle:
                handle.write(b"keep")
            adopt(proof, mixed, "08 adopt")
            expect_layout(mixed, {"debug": f"link:{root_of(pool, mixed)}", "release": "real"})
            expect(perm.read_bytes(os.path.join(mixed, "target", "release", "keep")) == b"keep", "08: real member content changed")
            code, _, stderr = oxide(proof, ["build"], "08 build", mixed)
            expect(code == 0, f"08: build with a kept real member failed: {stderr}")
            expect_layout(mixed, {"debug": f"link:{root_of(pool, mixed)}", "release": "real"})
            # A3 positive mixture control: `release` is REAL here and `debug` is
            # LINKED, so `run --release` must execute the real member's own Cargo path
            # while a plain `run` executes the linked member's destination. Neither may
            # fall back to one global root assumption.
            mixed_root = root_of(pool, mixed)
            code, _, stderr = oxide(proof, ["build", "--release"], "08 release build", mixed)
            expect(code == 0, f"08: release build with a real member failed: {stderr}")
            expect_layout(mixed, {"debug": f"link:{mixed_root}", "release": "real"})
            real_exe = os.path.join(mixed, "target", "release", "profileapp")
            code, stdout, stderr = oxide(proof, ["run", "--release"], "08 release run", mixed)
            expect(code == 0, f"08: mixed run --release failed: {stderr}")
            expect(
                "profile-app 7 " in stdout and real_exe in stdout,
                f"08: run --release did not execute the real member artifact {real_exe}: {stdout}",
            )
            linked_exe = member_destination(mixed_root, "debug")
            code, stdout, stderr = oxide(proof, ["run"], "08 linked run", mixed)
            expect(code == 0, f"08: mixed plain run failed: {stderr}")
            expect(
                "profile-app 7 " in stdout and linked_exe in stdout,
                f"08: plain run did not execute the linked member artifact {linked_exe}: {stdout}",
            )
            other = make_checkout(proof, "empty-member")
            # An empty release directory is converted only by this explicit adopt.
            os.makedirs(os.path.join(other, "target", "release"), exist_ok=True)
            adopt(proof, other, "08 empty adopt")
            expect_layout(other, {"debug": f"link:{root_of(pool, other)}", "release": f"link:{root_of(pool, other)}"})
            DIAG.finished("08.1")
        except BaseException as error:
            DIAG.failed_step(proof, "08.1", error)

    if DIAG.enter("09.1", fixtures=(first_usable,)):
        try:
            # ----- case 09: container-style preflight sees a real target ----------
            case_reached("09")
            expect(is_real_dir(os.path.join(first, "target")), "09: target is not a real directory for a container preflight")
            env = {**proof.env, "CARGO_TARGET_DIR": os.path.join(first, "target")}
            result = proof.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=first, env=env, timeout=60)
            expect(result.returncode == 0, "09: metadata through the real target failed")
            DIAG.finished("09.1")
        except BaseException as error:
            DIAG.failed_step(proof, "09.1", error)

    if DIAG.enter("10.1", fixtures=()):
        try:
            # ----- case 10: legacy whole-target state stays unchanged under state 4 ---
            case_reached("10")
            # The pool now carries the successor fence (case 11); the base-produced /2
            # checkout must still build warm with the same slot, key, receipt and link.
            expect(fence_in(pool) == STATE4_FENCE, f"10: the pool is not state 4: {fence_in(pool)}")
            code, _, stderr = oxide(proof, ["build"], "10 warm under state4", legacy)
            expect(code == 0, f"10: legacy build under state4 failed: {stderr}")
            warm_slot, warm_state = perm.slot_state(stderr)
            expect(warm_state == "warm", f"10: legacy build was not warm under state4: {summary(stderr)}")
            expect(warm_slot == base_slot, f"10: legacy slot changed under state4 {base_slot} -> {warm_slot}")
            expect(os.path.islink(os.path.join(legacy, "target")), "10: the legacy link changed")
            expect(link_at(os.path.join(legacy, "target")) == legacy_root, "10: the legacy link text changed")
            expect(item(pool, legacy, "version") == "oxide.outputs.transition/2", "10: the legacy receipt was rewritten")
            DIAG.finished("10.1")
        except BaseException as error:
            DIAG.failed_step(proof, "10.1", error)

    if DIAG.enter("12.1", fixtures=()):
        try:
            # ----- case 12: P0-P4 replay, races and exact rollback ----------------
            case_reached("12")
            # A crash at every pre-commit phase replays to the committed profile
            # layout; a rollback at the staged and marker boundaries restores the
            # exact published /2 state.
            for boundary in ("p0", "staged", "marker", "swapped"):
                fixture = make_checkout(proof, f"replay-{boundary}")
                auto_legacy(proof, fixture)
                root = root_of(pool, fixture)
                marker_before = perm.read_bytes(os.path.join(root, MARKER))
                if boundary == "marker":
                    make_root_one_fixture(pool, fixture)
                    marker_before = perm.read_bytes(os.path.join(root, MARKER))
                env = {**proof.env, "OXIDE_TEST_OUTPUTS_CRASH": f"migration-{boundary}"}
                if boundary == "p0":
                    # The P0 crash point is named "prepared" by the journal phase.
                    env["OXIDE_TEST_OUTPUTS_CRASH"] = "migration-p0"
                    code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], "12 crash p0", fixture, env)
                    expect(code != 0, "12: the injected P0 stop did not stop adopt")
                    stored = perm.read_json(perm.receipt_path(pool, fixture))
                    expect(stored["state"] == "prepared", f"12: P0 left state {stored['state']}")
                    expect(
                        stored.get("transition", {}).get("phase") == "prepared",
                        f"12: P0 journal phase is {stored.get('transition')}",
                    )
                    expect(
                        os.path.islink(os.path.join(fixture, "target")),
                        "12: P0 changed the legacy target link",
                    )
                    # A P0 replay starts from the journal and commits the profile layout.
                    code, _, stderr = oxide(proof, ["outputs", "adopt"], "12 replay p0", fixture)
                    expect(code == 0, f"12: replay from p0 failed: {stderr}")
                    expect_layout(fixture, {"debug": f"link:{root_of(pool, fixture)}"})
                    continue
                code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], f"12 crash {boundary}", fixture, env)
                expect(code != 0, f"12: the injected {boundary} stop did not stop adopt")
                stored = perm.read_json(perm.receipt_path(pool, fixture))
                expect(stored["state"] == "prepared", f"12: {boundary} left state {stored['state']}")
                # Ordinary commands refuse while the journal is prepared.
                code, _, stderr = oxide(proof, ["build"], f"12 pending {boundary}", fixture)
                expect(code != 0, f"12: a build was allowed while the migration was pending")
                del stderr
                # A second adopt replays the exact phases and commits.
                code, _, stderr = oxide(proof, ["outputs", "adopt"], f"12 replay {boundary}", fixture)
                expect(code == 0, f"12: replay from {boundary} failed: {stderr}")
                expect_layout(fixture, {"debug": f"link:{root_of(pool, fixture)}"})
                expect(receipt(pool, fixture)["version"] == "oxide.outputs.transition/3", f"12: {boundary} replay did not commit /3")
            for boundary in ("staged", "marker"):
                # A source-shaped root/1 marker makes both boundaries the two sides of
                # the marker upgrade: staged is before it, marker is after it.
                fixture = make_checkout(proof, f"rollback-{boundary}")
                auto_legacy(proof, fixture)
                root = make_root_one_fixture(pool, fixture)
                marker_before = perm.read_bytes(os.path.join(root, MARKER))
                env = {**proof.env, "OXIDE_TEST_MIGRATION_ROLLBACK": boundary}
                code, _, stderr = perm.oxide_env(proof, ["outputs", "adopt"], f"12 rollback {boundary}", fixture, env)
                expect(code == 0, f"12: rollback at {boundary} failed: {stderr}")
                expect(os.path.islink(os.path.join(fixture, "target")), f"12: rollback at {boundary} did not restore the old link")
                expect(perm.read_bytes(os.path.join(root, MARKER)) == marker_before, f"12: rollback at {boundary} did not restore the marker")
                restored = receipt(pool, fixture)
                expect(restored["version"] == "oxide.outputs.transition/2", f"12: rollback at {boundary} did not restore /2")
                expect(
                    restored["links"][0]["selected"].get("nonce") is None,
                    f"12: rollback at {boundary} restored a nonce into the source-shaped /2 receipt",
                )
                registry = perm.read_json(os.path.join(pool, "oxide-managed", "outputs", "registry.json"))
                fixture_id = perm.hashlib.sha256(fixture.encode()).hexdigest()
                fixture_rows = [row for row in registry["rows"] if row["checkout_id"] == fixture_id]
                expect(
                    not any(row["member"] in ("debug", "release") and row.get("selected") for row in fixture_rows),
                    f"12: rollback at {boundary} left profile rows selected: {fixture_rows}",
                )
                expect(
                    not any(row.get("pruning") or row.get("pending") for row in fixture_rows),
                    f"12: rollback at {boundary} left a live profile reference: {fixture_rows}",
                )
                # The rollback retires the journal and reserves nothing.
                expect("transition" not in receipt(pool, fixture), f"12: rollback at {boundary} left a journal")
                # It can be migrated for real afterwards.
                adopt(proof, fixture, f"12 migrate {boundary}")
                expect_layout(fixture, {"debug": f"link:{root_of(pool, fixture)}"})
            # A creator of the target between the backup and the swap is preserved and
            # leaves the migration pending: no payload is deleted and no creator is
            # replaced.
            creator_fixture = make_checkout(proof, "creator-race")
            auto_legacy(proof, creator_fixture)
            creator_root = root_of(pool, creator_fixture)
            env = {**proof.env, "OXIDE_TEST_OUTPUTS_CRASH": "migration-backed-up"}
            code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], "12 backup crash", creator_fixture, env)
            expect(code != 0, "12: the injected backup stop did not stop adopt")
            creator = os.path.join(creator_fixture, "target")
            expect(not os.path.lexists(creator), "12: the old link was not moved to its backup")
            os.makedirs(creator)
            with open(os.path.join(creator, "creator"), "wb") as handle:
                handle.write(b"creator")
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "12 creator race", creator_fixture)
            expect(code != 0, f"12: the creator race committed: {stderr}")
            expect(perm.read_bytes(os.path.join(creator, "creator")) == b"creator", "12: the creator was deleted")
            stored = perm.read_json(perm.receipt_path(pool, creator_fixture))
            expect(stored["state"] == "prepared", "12: the creator race did not stay pending")
            os.remove(os.path.join(creator, "creator"))
            os.rmdir(creator)
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "12 creator cleared", creator_fixture)
            expect(code == 0, f"12: adopt after clearing the creator failed: {stderr}")
            expect_layout(creator_fixture, {"debug": f"link:{creator_root}"})
            # A replaced checkout directory (same path, new inode) is an identity
            # change: the journal stays pending with no effects until the exact pinned
            # identity returns.
            parent_race = make_checkout(proof, "parent-race")
            auto_legacy(proof, parent_race)
            env = {**proof.env, "OXIDE_TEST_OUTPUTS_CRASH": "migration-staged"}
            code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], "12 parent crash", parent_race, env)
            expect(code != 0, "12: the parent-race crash did not stop adopt")
            os.rename(parent_race, parent_race + "-old")
            shutil.copytree(parent_race + "-old", parent_race, symlinks=True)
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "12 parent race", parent_race)
            expect(code != 0 and "identity" in stderr, f"12: a replaced checkout identity was replayed: {stderr}")
            expect(
                perm.read_json(perm.receipt_path(pool, parent_race))["state"] == "prepared",
                "12: the parent race did not stay pending",
            )
            shutil.rmtree(parent_race)
            os.rename(parent_race + "-old", parent_race)
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "12 parent restored", parent_race)
            expect(code == 0, f"12: adopt after restoring the checkout identity failed: {stderr}")
            expect_layout(parent_race, {"debug": f"link:{root_of(pool, parent_race)}"})

            # A crash after the P4 commit keeps the committed real target and the
            # cleanup journal; a replay finishes cleanup and never rolls back.
            p4 = make_checkout(proof, "p4-cleanup")
            auto_legacy(proof, p4)
            env = {**proof.env, "OXIDE_TEST_OUTPUTS_CRASH": "migration-p4"}
            code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], "12 p4 crash", p4, env)
            expect(code != 0, "12: the injected P4 stop did not stop adopt")
            p4_receipt = perm.read_json(perm.receipt_path(pool, p4))
            expect(p4_receipt["state"] == "committed", "12: P4 did not commit before cleanup")
            expect(p4_receipt["layout"] == "profile", "12: P4 did not select the profile layout")
            expect("transition" in p4_receipt, "12: the P4 cleanup journal was not retained")
            expect(
                not os.path.islink(os.path.join(p4, "target"))
                and os.path.isdir(os.path.join(p4, "target")),
                "12: P4 target is not a real directory",
            )
            expect_layout(p4, {"debug": f"link:{root_of(pool, p4)}"})
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "12 p4 replay", p4)
            expect(code == 0, f"12: replaying the P4 cleanup failed: {stderr}")
            finished = perm.read_json(perm.receipt_path(pool, p4))
            expect("transition" not in finished, "12: the replay did not finish the cleanup journal")
            expect_layout(p4, {"debug": f"link:{root_of(pool, p4)}"})
            # The staging and backup leaves are derived from the journal's own
            # generation (the committed receipt's), never a hardcoded one, and lexists
            # is used so a dangling leftover link still counts as risen.
            p4_generation = finished["generation"]
            expect(
                isinstance(p4_generation, int) and p4_generation > 0,
                f"12: the finished receipt carries no usable generation: {p4_generation!r}",
            )
            for leaf in (
                f".target.oxide-new-{p4_generation}",
                f".target.oxide-old-{p4_generation}",
            ):
                expect(
                    not os.path.lexists(os.path.join(p4, leaf)),
                    f"12: {leaf} survived the finished cleanup",
                )

            # A committed migration is never rolled back.
            rollback = make_checkout(proof, "committed")
            auto_legacy(proof, rollback)
            adopt(proof, rollback, "12 committed migrate")
            env = {**proof.env, "OXIDE_TEST_MIGRATION_ROLLBACK": "1"}
            code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], "12 committed", rollback, env)
            expect(code == 0, "12: adopt on a committed profile checkout failed")
            expect_layout(rollback, {"debug": f"link:{root_of(pool, rollback)}"})
            expect(receipt(pool, rollback)["version"] == "oxide.outputs.transition/3", "12: committed migration changed format")
            DIAG.finished("12.1")
        except BaseException as error:
            DIAG.failed_step(proof, "12.1", error)

    if DIAG.enter("13.1", fixtures=()):
        try:
            # ----- case 13: an active writer blocks migration ---------------------
            case_reached("13")
            blocked = make_checkout(proof, "blocked")
            auto_legacy(proof, blocked)
            gate = os.path.join(proof.root, "gate13")
            os.makedirs(gate)
            child = perm.start_child(proof, [proof.binary, "build"], blocked, {**proof.env, "OXIDE_PROFILE_GATE": gate})
            CUSTODY.track(child, gate, "13 gated writer")
            wait_started(gate, "13 writer start")
            before = custody_snapshot(pool, blocked)
            waiter = perm.start_child(proof, [proof.binary, "outputs", "adopt"], blocked, proof.env)
            CUSTODY.track(waiter, None, "13 waiting adopt")
            observed_wait(waiter, before, pool, blocked, "13")
            expect(
                os.path.islink(os.path.join(blocked, "target")),
                "13: the migration changed the target while a writer ran",
            )
            perm.stop_gate(gate, os.path.join(gate, "release"))
            code, _, _ = perm.finish_child(proof, child, "13 writer")
            CUSTODY.finished(child)
            expect(code == 0, f"13: the gated writer exited {code}")
            code, _, _ = perm.finish_child(proof, waiter, "13 waiting adopt")
            CUSTODY.finished(waiter)
            expect(code == 0, "13: the waiting adopt failed")
            expect_layout(blocked, {"debug": f"link:{root_of(pool, blocked)}"})
            DIAG.finished("13.1")
        except BaseException as error:
            DIAG.failed_step(proof, "13.1", error)

    if DIAG.enter("14.1", fixtures=(first_usable,)):
        try:
            # ----- case 14: per-member fallback and return ------------------------
            case_reached("14")
            fallback = make_checkout(proof, "fallback")
            code, _, _ = oxide(proof, ["build"], "14 prebuild", fallback)
            expect(code == 0, "14: prebuild failed")
            adopt(proof, fallback, "14 adopt")
            external = root_of(pool, fallback)
            expect_layout(fallback, {"debug": f"link:{external}"})
            # The volume goes away: the next command's intermediate slot falls back to
            # the internal pool while its member link selects the external root, so it
            # refuses instead of writing through an unsupervised drive.
            # A10: the custody facts are persisted BEFORE this risky ordinary detach,
            # and the detached state is recorded after it.
            write_attachment_facts("attachment-facts.json", known_attachment_facts(proof, "14 volume away", "detaching"))
            detached = proof.run(["/usr/bin/hdiutil", "detach", proof.mount], timeout=60)
            expect(detached.returncode == 0, "14: ordinary detach failed")
            proof.mount = None
            write_attachment_facts("attachment-facts.json", known_attachment_facts(proof, "14 volume away", "detached"))
            # The internal fallback slot meets an external owned link: the command
            # refuses rather than writing to an unsupervised drive. The oracle is exact
            # (exit 1, both the slot reason and the refusal reason, the affected member
            # and its paths) and the recorded authority is compared byte for byte: the
            # fixture directory is named "fallback", so a bare substring would prove
            # nothing.
            before_fallback = perm.read_bytes(receipt_file(pool, fallback))
            code, _, stderr = oxide(proof, ["build"], "14 fallback refusal", fallback)
            expect(code == 1, f"14: the fallback build did not refuse with exit 1: {code}: {stderr}")
            expect(
                "fell back to the internal pool" in stderr,
                f"14: the refusal did not report the internal-pool fallback: {stderr}",
            )
            expect("nothing was built" in stderr, f"14: the refusal did not report that nothing was built: {stderr}")
            expect(
                member_destination(external, "debug") in stderr,
                f"14: the refusal did not name the external member it keeps: {stderr}",
            )
            expect(
                member_link(fallback, "debug") in stderr,
                f"14: the refusal did not name the affected member link: {stderr}",
            )
            expect(
                "never moves an owned external output to another pool automatically" in stderr,
                f"14: the refusal did not state that it will not move the root automatically: {stderr}",
            )
            expect(
                perm.read_bytes(receipt_file(pool, fallback)) == before_fallback,
                "14: the fallback refusal changed the recorded authority",
            )
            expect(
                os.readlink(member_link(fallback, "debug")) == member_destination(external, "debug"),
                "14: the refusal changed the member link",
            )
            # An explicit adopt moves every owned member to the checkout-local root
            # under custody, keeping the external root; re-attaching returns them.
            adopt(proof, fallback, "14 fallback adopt")
            local = root_of(pool, fallback)
            expect(local != external, "14: fallback kept the external root")
            expect_layout(fallback, {"debug": f"link:{local}", "release": f"link:{local}"})
            expect(
                not os.path.exists(external),
                "14: the external root is reachable while its volume is unmounted",
            )
            remount = attach_owned_image(proof, "fallback return attach")
            expect(remount is not None, "14: cannot re-attach the image")
            adopt(proof, fallback, "14 return adopt")
            expect_layout(fallback, {"debug": f"link:{external}", "release": f"link:{external}"})
            expect(os.path.isdir(external), "14: the external root did not return with its volume")
            expect(os.path.isdir(local), "14: the local root was deleted on return")
            # Positive backend checks only once the volume is mounted again.
            expect(os.stat(external).st_dev == os.stat(proof.mount).st_dev, "14: the returned root is not on the registered volume")
            registration = perm.read_json(perm.registration_path(pool))
            volume = perm.plistlib.loads(proof.run(["/usr/sbin/diskutil", "info", "-plist", proof.mount], timeout=30).stdout)
            expect(volume.get("VolumeUUID") == registration["volume_uuid"], "14: the mounted volume UUID changed")
            DIAG.finished("14.1")
        except BaseException as error:
            DIAG.failed_step(proof, "14.1", error)

    if DIAG.enter("15.1", fixtures=(first_usable,)):
        try:
            # ----- case 15: detach and re-adopt ----------------------------------
            case_reached("15")
            code, _, stderr = oxide(proof, ["outputs", "detach"], "15 detach", first)
            expect(code == 0, f"15: detach failed: {stderr}")
            expect_layout(first, {"debug": "real", "release": "real"})
            expect(receipt(pool, first)["state"] == "detached", "15: receipt not detached")
            expect(os.path.isdir(root), "15: a root was deleted by detach")
            adopt(proof, first, "15 readopt")
            expect_layout(first, {"debug": f"link:{root_of(pool, first)}"})
            host = proof.run(["rustc", "-vV"], cwd=first, env=proof.env, timeout=30).stdout.decode()
            triple = next(line.split(": ", 1)[1] for line in host.splitlines() if line.startswith("host: "))
            code, _, stderr = oxide(proof, ["build", "--target", triple], "15 triple build", first)
            expect(code == 0, f"15: host-triple build failed: {stderr}")
            expect_layout(first, {f"{triple}/debug": f"link:{root_of(pool, first)}"})
            code, _, stderr = oxide(proof, ["outputs", "detach"], "15 triple detach", first)
            expect(code == 0, f"15: triple detach failed: {stderr}")
            expect_layout(first, {"debug": "real", "release": "real", f"{triple}/debug": "real"})
            adopt(proof, first, "15 triple readopt")
            expect_layout(
                first,
                {"debug": f"link:{root_of(pool, first)}", f"{triple}/debug": f"link:{root_of(pool, first)}"},
            )
            DIAG.finished("15.1")
        except BaseException as error:
            DIAG.failed_step(proof, "15.1", error)

    if DIAG.enter("16.1", fixtures=(first_usable,)):
        try:
            # ----- case 16: whole and selected clean keep links and markers -------
            case_reached("16")
            # The known fixture top-level file is a foreign entry for a whole clean:
            # prove the refusal names it, then remove it and prove clean succeeds.
            top_control = os.path.join(top, "target", "oxide-proof-top-level-control")
            code, _, stderr = oxide(proof, ["clean", "--dry-run"], "16 top foreign dry", top)
            expect(code != 0 and "oxide-proof-top-level-control" in stderr, f"16: the known top file did not refuse a clean: {stderr}")
            expect(os.path.isfile(top_control), "16: the refused clean removed the known top file")
            os.remove(top_control)
            code, _, stderr = oxide(proof, ["clean", "--dry-run"], "16 top clean dry", top)
            expect(code == 0, f"16: clean still refused after the known file was removed: {stderr}")
            code, _, stderr = oxide(proof, ["clean"], "16 top clean", top)
            expect(code == 0, f"16: clean failed after the known file was removed: {stderr}")
            expect_layout(top, {"debug": f"link:{root_of(pool, top)}"})
            code, _, stderr = oxide(proof, ["clean", "--dry-run"], "16 clean dry", first)
            expect(code == 0, f"16: clean dry run failed: {stderr}")
            code, _, stderr = oxide(proof, ["clean"], "16 clean", first)
            expect(code == 0, f"16: clean failed: {stderr}")
            expect_layout(first, {"debug": f"link:{root_of(pool, first)}", "release": f"link:{root_of(pool, first)}"})
            expect(os.path.isfile(os.path.join(root, MARKER)), "16: clean removed the root marker")
            code, _, stderr = oxide(proof, ["clean", "-p", "profileapp"], "16 selected clean", first)
            expect(code == 0, f"16: selected clean failed: {stderr}")
            expect_layout(first, {"debug": f"link:{root_of(pool, first)}"})
            # The selected clean runs the existing pool watchdog. The REMOVABLE
            # watchdog is removal-only (its reserve trigger is disabled), so the
            # reserve stop is exercised through a positively created directory-budget
            # pool: a gated fake cargo that delegates `locate-project` and every other
            # non-clean command to the real cargo starts the recorded group FIRST, then
            # a below-reserve reading stops it and the stop is reported.
            dirpool = os.path.join(proof.root, "clean-pool")
            real_cargo = shutil.which("cargo")
            expect(real_cargo is not None, "16: no real cargo on PATH for the fake to delegate to")
            binaries = os.path.join(proof.root, "fake-bin")
            os.makedirs(binaries, exist_ok=True)
            gate = os.path.join(proof.root, "clean-gate")
            os.makedirs(gate, exist_ok=True)
            fake = os.path.join(binaries, "cargo")
            with open(fake, "w", encoding="utf-8") as handle:
                handle.write(
                    "#!/bin/sh\n"
                    "case \"$1\" in\n"
                    "  clean)\n"
                    "    touch \"$OXIDE_CLEAN_GATE/started\"\n"
                    "    n=0\n"
                    "    while [ ! -f \"$OXIDE_CLEAN_GATE/release\" ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done\n"
                    "    exit 0 ;;\n"
                    "  *)\n"
                    "    exec \"" + real_cargo + "\" \"$@\" ;;\n"
                    "esac\n"
                )
            os.chmod(fake, 0o755)
            readings = os.path.join(proof.root, "clean-readings")
            with open(readings, "w", encoding="utf-8") as handle:
                handle.write(str(23 * 1024 * 1024 * 1024))
            env = {
                **proof.env,
                "OXIDE_POOL": dirpool,
                "OXIDE_POOL_BACKEND": "directory-budget",
                "PATH": binaries + os.pathsep + proof.env.get("PATH", ""),
                "OXIDE_CLEAN_GATE": gate,
                "OXIDE_TEST_WATCHDOG_READINGS": readings,
            }
            # A5: the watched clean runs in the checkout whose OWN receipt and custody
            # live in this directory-budget pool. It is adopted and built here, so no
            # receipt authority is copied across pools and `first` (a primary-pool
            # checkout) is never cleaned under this pool's context.
            seeded = make_checkout(proof, "clean-pool-fixture")
            os.makedirs(os.path.join(seeded, "target"))
            code, _, stderr = perm.oxide_env(proof, ["outputs", "adopt"], "16 clean pool adopt", seeded, env)
            expect(code == 0, f"16: creating the directory-budget pool failed: {stderr}")
            code, _, stderr = perm.oxide_env(proof, ["build"], "16 clean pool seed", seeded, env)
            expect(code == 0, f"16: seeding the directory-budget pool failed: {stderr}")
            seeded_root = root_of(dirpool, seeded)
            expect_layout(seeded, {"debug": f"link:{seeded_root}", "release": f"link:{seeded_root}"})
            seeded_receipt = perm.read_bytes(receipt_file(dirpool, seeded))
            child = perm.start_child(proof, [proof.binary, "clean", "-p", "profileapp"], seeded, env)
            CUSTODY.track(child, gate, "16 watched clean")
            perm.wait_for(os.path.join(gate, "started"), "16 watched clean writer start")
            with open(readings, "w", encoding="utf-8") as handle:
                handle.write(str(22 * 1024 * 1024 * 1024 - 4096))
            code, _, stderr = perm.finish_child(proof, child, "16 watched clean")
            CUSTODY.finished(child)
            # A11: the stop is judged by the source-defined report, not by a word
            # match. watchdog.rs::report emits the injected reason, the reading and
            # trigger, a concrete Cargo group, the sampled minimum, the overshoot and
            # a confirmed closure latency; the clean command then returns the child's
            # SIGNAL-derived status (process::exit_code), TERM -> 143 and 137 only if
            # the stop escalated to KILL. An unrelated nonzero or a bare 1 fails here.
            stop_trigger = 22 * 1024 * 1024 * 1024
            facts = reserve_stop_facts(stderr, stop_trigger)
            expect(
                facts["reading"] == stop_trigger - 4096,
                f"16: the reported reading is {facts['reading']}, not the injected trigger-4096: {facts['line']}",
            )
            expect(
                code in reserve_stop_status(facts),
                f"16: the clean exited {code}, which the reported stop does not permit "
                f"({'TERM or KILL' if facts['escalated'] else 'TERM only'}): {facts['line']}",
            )
            # The stopped clean keeps the seeded checkout's own authority: same links,
            # same receipt bytes, root still present.
            expect_layout(seeded, {"debug": f"link:{seeded_root}", "release": f"link:{seeded_root}"})
            expect(
                perm.read_bytes(receipt_file(dirpool, seeded)) == seeded_receipt,
                "16: the stopped clean changed the seeded receipt bytes",
            )
            expect(os.path.isdir(seeded_root), "16: the stopped clean removed the seeded root")
            with open(os.path.join(gate, "release"), "wb") as handle:
                handle.write(b"release")
            DIAG.finished("16.1")
        except BaseException as error:
            DIAG.failed_step(proof, "16.1", error)

    if DIAG.enter("17.1", fixtures=(first_usable,)):
        try:
            # ----- case 17: refusals ---------------------------------------------
            case_reached("17")
            malformed = make_checkout(proof, "malformed")
            code, _, _ = oxide(proof, ["build"], "17 prebuild", malformed)
            adopt(proof, malformed, "17 adopt")
            code, _, stderr = oxide(proof, ["exec", "--output", "../escape", "--", script], "17 malformed member", malformed)
            expect(code != 0 and "not an output member" in stderr, f"17: a traversing member was accepted: {stderr}")
            # A foreign member link refuses before Cargo. The oracle is exact: exit 1,
            # the affected member path, Oxide's own label for an entry it does not own
            # and its never-follow policy. The former oracle was `A and B or C`, so a
            # bare "foreign" substring passed even when nothing was named. No authority
            # changes: the receipt bytes and the link itself are compared.
            referent = os.path.join(malformed, "foreign-referent")
            os.makedirs(referent)
            with open(os.path.join(referent, "keep"), "wb") as handle:
                handle.write(b"referent")
            foreign = os.path.join(malformed, "target", "release")
            os.remove(foreign) if os.path.islink(foreign) else None
            os.symlink(referent, foreign)
            before_receipt = perm.read_bytes(receipt_file(pool, malformed))
            # The command must declare the affected member: a debug build never touches
            # release, so `build --release` is the refusal that proves it.
            code, _, stderr = oxide(proof, ["build", "--release"], "17 foreign release link", malformed)
            expect(code == 1, f"17: a foreign release link did not refuse with exit 1: {code}: {stderr}")
            expect(foreign in stderr, f"17: the refusal did not name the affected release member: {stderr}")
            expect(
                "a symlink or file Oxide does not own" in stderr,
                f"17: the refusal did not label the entry as unowned: {stderr}",
            )
            expect(
                "never follows or replaces it" in stderr
                or "never followed, adopted or replaced" in stderr,
                f"17: the refusal did not state the never-follow policy: {stderr}",
            )
            expect(
                perm.read_bytes(receipt_file(pool, malformed)) == before_receipt,
                "17: the foreign-link refusal changed the recorded authority",
            )
            expect(
                os.readlink(foreign) == referent
                and perm.read_bytes(os.path.join(referent, "keep")) == b"referent",
                "17: the refused command followed or replaced the foreign link",
            )
            code, _, stderr = oxide(proof, ["build"], "17 debug still usable", malformed)
            expect(code == 0, f"17: the unaffected debug member stopped building: {stderr}")
            os.remove(foreign)
            os.remove(os.path.join(referent, "keep"))
            os.rmdir(referent)
            # Actual unknown custody: a retired record whose writers were never
            # positively closed must make adopt refuse with every byte unchanged.
            custody = make_checkout(proof, "unknown-custody")
            os.makedirs(os.path.join(custody, "target"))
            adopt(proof, custody, "17 custody adopt")
            state_path = os.path.join(pool, "oxide-managed", "state.json")
            original_state = perm.read_bytes(state_path)
            state = json.loads(original_state)
            template = next(record for record in state["records"] if record.get("checkout"))
            stolen = dict(template)
            stolen["id"] = max(record["id"] for record in state["records"]) + 1
            stolen["checkout"] = custody
            stolen["phase"] = "retired"
            stolen["retired"] = "proof fixture"
            stolen["lost"] = {"slot_id": "proof-lost", "at_ms": 1, "unclosed": "proof fixture: writers never confirmed closed"}
            state["records"].append(stolen)
            with open(state_path, "w", encoding="utf-8") as handle:
                json.dump(state, handle, indent=2)
                handle.write("\n")
            os.chmod(state_path, 0o600)
            before = custody_snapshot(pool, custody)
            code, _, stderr = oxide(proof, ["outputs", "adopt"], "17 unknown custody", custody)
            expect(code != 0 and "lost with writers" in stderr, f"17: unknown custody was not refused: {stderr}")
            expect(custody_snapshot(pool, custody) == before, "17: the unknown-custody refusal changed authority")
            with open(state_path, "wb") as handle:
                handle.write(original_state)
            os.chmod(state_path, 0o600)
            DIAG.finished("17.1")
        except BaseException as error:
            DIAG.failed_step(proof, "17.1", error)

    if DIAG.enter("18.1", fixtures=(first_usable,)):
        try:
            # ----- case 18: real and linked placements key different cold slots ----
            case_reached("18")
            # A real NONEMPTY debug directory is kept real, so the first keyed build
            # uses real:debug. The operator then removes that real directory (Oxide
            # never moves it) and adopts again: the same member becomes a link, so the
            # next build's key changes and it cannot reuse the real placement's slot.
            cold = make_checkout(proof, "cold")
            os.makedirs(os.path.join(cold, "target", "debug"))
            with open(os.path.join(cold, "target", "debug", "keep"), "wb") as handle:
                handle.write(b"keep")
            code, _, stderr = oxide(proof, ["build"], "18 real debug", cold)
            expect(code == 0, f"18: real-member build failed: {stderr}")
            adopt(proof, cold, "18 adopt")
            cold_root = root_of(pool, cold)
            expect_layout(cold, {"debug": "real", "release": f"link:{cold_root}"})
            facts_real = perm.plan(proof, cold, "18 real plan")["members"]
            code, _, stderr = oxide(proof, ["build"], "18 real build again", cold)
            expect(code == 0, f"18: second real-member build failed: {stderr}")
            expect_layout(cold, {"debug": "real", "release": f"link:{cold_root}"})
            # A8: the real placement's slot is read from the parsed summary, and the
            # repeat is required to be WARM in that exact slot - not a substring test.
            real_slot, real_state = perm.slot_state(stderr)
            expect(real_state == "warm", f"18: the real placement's repeat was not warm: {summary(stderr)}")
            # A8: the selected removable backend's own slot root, plus the internal
            # pool's authority for a no-effect comparison.
            slots_dir = selected_backend_slots(proof, pool, "18")
            slots_before = sorted(os.listdir(slots_dir))
            internal_slots = os.path.join(pool, "payload", "slots")
            internal_before = sorted(os.listdir(internal_slots)) if os.path.isdir(internal_slots) else []
            expect(
                real_slot in slots_before,
                f"18: the real placement's slot {real_slot} is not in the selected backend tree {slots_dir}",
            )
            shutil.rmtree(os.path.join(cold, "target", "debug"))
            adopt(proof, cold, "18 relink")
            expect_layout(cold, {"debug": f"link:{cold_root}", "release": f"link:{cold_root}"})
            code, _, stderr = oxide(proof, ["build"], "18 linked debug", cold)
            expect(code == 0, f"18: linked build failed: {stderr}")
            linked_slot, linked_state = perm.slot_state(stderr)
            expect(
                linked_state == "cold",
                f"18: the linked placement reused the real placement's slot: {summary(stderr)}",
            )
            expect(
                linked_slot != real_slot,
                f"18: the placement change reused slot {real_slot} ({real_state}); a distinct key needs its own slot",
            )
            slots_after = sorted(os.listdir(slots_dir))
            expect(
                set(slots_before).issubset(set(slots_after)),
                f"18: the real placement's slot was evicted instead of remaining available: {slots_before} -> {slots_after}",
            )
            expect(
                real_slot in slots_after,
                f"18: the real placement's slot {real_slot} left the selected backend tree",
            )
            if os.path.isdir(internal_slots):
                expect(
                    sorted(os.listdir(internal_slots)) == internal_before,
                    "18: the internal pool's slot authority changed during external builds",
                )
            facts_linked = perm.plan(proof, cold, "18 linked plan")["members"]
            expect(
                facts_real != facts_linked,
                f"18: placement change did not change the planned members: {facts_real} vs {facts_linked}",
            )
            DIAG.finished("18.1")
        except BaseException as error:
            DIAG.failed_step(proof, "18.1", error)

    if DIAG.enter("19.1", fixtures=()):
        try:
            # ----- case 19: the staged state-3 binary refuses state 4 -------------
            case_reached("19")
            snapshot = {member: os.readlink(member_link(migrated, member)) for member in ("debug", "release")}
            receipt_before = perm.sha256_file(perm.receipt_path(pool, migrated))
            slots_dir = selected_backend_slots(proof, pool, "19")
            slots_before = sorted(os.listdir(slots_dir))
            internal_slots = os.path.join(pool, "payload", "slots")
            internal_before = sorted(os.listdir(internal_slots)) if os.path.isdir(internal_slots) else []
            # Working base control immediately before the refusal: the same base
            # binary serves a fresh private pool normally.
            control = proof.run([base, "--version"], cwd=migrated, env=proof.env, timeout=60)
            expect(control.returncode == 0, f"19: the working base control failed: {control.stderr.decode(errors='replace')}")
            # Case 10 already records the base binary producing and warming a /2 pool
            # before the candidate fence; this control proves the same staged binary
            # still runs immediately before its forward-fence refusal.
            # Zero Cargo calls: a recording fake cargo refuses and records any call.
            recorder_bin = os.path.join(proof.root, "record-bin")
            os.makedirs(recorder_bin, exist_ok=True)
            cargo_marker = os.path.join(proof.root, "cargo-was-called")
            recorder = os.path.join(recorder_bin, "cargo")
            with open(recorder, "w", encoding="utf-8") as handle:
                handle.write("#!/bin/sh\nprintf '%s' \"$*\" >> \"" + cargo_marker + "\"\nexit 99\n")
            os.chmod(recorder, 0o755)
            refusal_env = {**proof.env, "PATH": recorder_bin + os.pathsep + proof.env.get("PATH", "")}
            result = proof.run([base, "build"], cwd=migrated, env=refusal_env, timeout=90)
            expect(result.returncode == 75, f"19: the state-3 refusal exit is {result.returncode}, not 75")
            refusal_text = result.stderr.decode(errors="replace")
            expect(
                "oxide was upgraded during this command; re-run." in refusal_text,
                f"19: the forward-fence refusal text changed: {refusal_text}",
            )
            expect(not os.path.exists(cargo_marker), "19: the refused state-3 binary called Cargo")
            print(refusal_text[-2000:])
            for member, target in snapshot.items():
                expect(os.readlink(member_link(migrated, member)) == target, "19: the refusal changed a member link")
            expect(perm.sha256_file(perm.receipt_path(pool, migrated)) == receipt_before, "19: the refusal rewrote the receipt")
            expect(
                sorted(os.listdir(slots_dir)) == slots_before,
                "19: the refused state-3 run created or changed a slot in the selected backend",
            )
            if os.path.isdir(internal_slots):
                expect(
                    sorted(os.listdir(internal_slots)) == internal_before,
                    "19: the refused state-3 run changed the internal pool authority",
                )
            DIAG.finished("19.1")
        except BaseException as error:
            DIAG.failed_step(proof, "19.1", error)

    if DIAG.enter("20.1", fixtures=()):
        try:
            # ----- case 20: root-group prune keeps another checkout's reference ---
            case_reached("20")
            group = make_checkout(proof, "group")
            code, _, _ = oxide(proof, ["build"], "20 prebuild", group)
            adopt(proof, group, "20 adopt")
            group_root = root_of(pool, group)
            report = perm.plan(proof, group, "20 plan")
            expect(report["layout"] == "profile", "20: not a profile checkout")
            code, stdout, stderr = oxide(proof, ["outputs", "prune", "--dry-run"], "20 prune dry", group)
            expect(code == 0, f"20: prune dry run failed: {stderr}")
            preview = json.loads(stdout)
            expect(preview["version"] == "oxide.outputs.prune-preview/1", f"20: preview version {preview.get('version')}")
            # A live checkout's root is KEEP: it is neither a candidate nor removed.
            expect(preview["candidates"] == [], f"20: a live checkout's root is a prune candidate: {preview['candidates']}")
            expect(os.path.isdir(group_root), "20: the live root disappeared")
            expect(os.path.isdir(os.path.join(group, "target")), "20: prune removed the target")
            expect(receipt(pool, group)["links"], "20: the live receipt lost its references")
            # A positively gone checkout: the checkout directory is moved away, the
            # receipt and root stay. The dry run then lists its root as a candidate.
            gone = make_checkout(proof, "gone")
            code, _, _ = oxide(proof, ["build"], "20 gone build", gone)
            expect(code == 0, "20: gone build failed")
            adopt(proof, gone, "20 gone adopt")
            gone_root = root_of(pool, gone)
            os.rename(gone, gone + "-moved")
            code, stdout, stderr = oxide(proof, ["outputs", "prune", "--dry-run"], "20 gone dry", group)
            expect(code == 0, f"20: gone prune dry run failed: {stderr}")
            preview = json.loads(stdout)
            expect(
                any(candidate["root"] == gone_root for candidate in preview["candidates"]),
                f"20: the gone checkout's root is not a candidate: {preview['candidates']}",
            )

            # Another checkout's reference keeps the root: a peer reference for the
            # same root turns the candidate set empty (KEEP) while it is live.
            registry_path = os.path.join(pool, "oxide-managed", "outputs", "registry.json")
            registry_original = perm.read_bytes(registry_path)
            registry = json.loads(registry_original)
            gone_id = perm.hashlib.sha256(gone.encode()).hexdigest()
            peer = make_checkout(proof, "group-peer")
            peer_id = perm.hashlib.sha256(peer.encode()).hexdigest()
            peer_receipt_path = receipt_file(pool, peer)
            rows = [row for row in registry["rows"] if row["checkout_id"] == gone_id]
            expect(bool(rows), "20: the gone checkout has no registry reference")
            # SYNTHETIC INJECTED AUTHORITY: a crafted committed peer receipt plus
            # matching registry rows (not a bind-produced receipt and not a missing
            # receipt) selects the gone root with the identical recorded identity.
            peer_receipt = {
                "version": "oxide.outputs.transition/2",
                "checkout": peer,
                "checkout_id": peer_id,
                "generation": rows[0]["generation"],
                "state": "committed",
                "links": [
                    {
                        "member": "target",
                        "selected": {
                            "kind": rows[0]["root_kind"],
                            "root": rows[0]["root"],
                            "volume_uuid": rows[0]["volume_uuid"],
                            "nonce": rows[0]["nonce"],
                            "generation": rows[0]["generation"],
                        },
                    }
                ],
                "updated_ms": 1,
            }
            with open(peer_receipt_path, "w", encoding="utf-8") as handle:
                json.dump(peer_receipt, handle, indent=2)
                handle.write("\n")
            os.chmod(peer_receipt_path, 0o600)
            for row in rows:
                reference = dict(row)
                reference["checkout_id"] = peer_id
                reference["checkout"] = peer
                reference["receipt"] = peer_receipt_path
                reference["selected"] = True
                reference["pending"] = False
                reference["pruning"] = None
                reference["pruned_ms"] = None
                reference["reservation"] = None
                reference["cancelling"] = None
                registry["rows"].append(reference)
            registry["revision"] = registry["revision"] + 1
            with open(registry_path, "w", encoding="utf-8") as handle:
                json.dump(registry, handle, indent=2)
                handle.write("\n")
            os.chmod(registry_path, 0o600)
            code, stdout, stderr = oxide(proof, ["outputs", "prune", "--dry-run"], "20 peer keep", group)
            expect(code == 0, f"20: the peer-reference dry run failed: {stderr}")
            kept_preview = json.loads(stdout)
            expect(
                not any(candidate["root"] == gone_root for candidate in kept_preview["candidates"]),
                f"20: another checkout's reference did not keep the root: {kept_preview['candidates']}",
            )

            expect(os.path.isdir(gone_root), "20: the peer-referenced root was removed")
            kept_reason = next(
                (
                    entry.get("reason", "")
                    for entry in kept_preview.get("kept", [])
                    if entry.get("root") == gone_root
                ),
                "",
            )
            expect(
                "another checkout's live reference" in kept_reason,
                f"20: the peer KEEP did not carry the other-owner reason: {kept_preview.get('kept')}",
            )
            # A9: ONE preview must show the peer-referenced root KEPT, a SECOND
            # DISTINCT eligible gone root as a candidate, and the live checkout's root
            # kept. The peer reference is synthetic injected authority (a crafted
            # committed receipt plus matching registry rows), not a bind-produced
            # receipt; the second eligible root is real fixture state with its own
            # identity, so neither control can stand in for the other.
            extra = make_checkout(proof, "root-group-extra-gone")
            code, _, stderr = oxide(proof, ["build"], "20 extra build", extra)
            expect(code == 0, f"20: the second eligible gone checkout failed to build: {stderr}")
            adopt(proof, extra, "20 extra adopt")
            extra_root = root_of(pool, extra)
            expect(
                len({extra_root, gone_root, group_root}) == 3,
                f"20: the simultaneous control reuses a root: {extra_root}, {gone_root}, {group_root}",
            )
            os.rename(extra, extra + "-moved")
            peer_receipt_bytes = perm.read_bytes(peer_receipt_path)
            code, stdout, stderr = oxide(
                proof, ["outputs", "prune", "--dry-run"], "20 peer keep and candidate", group
            )
            expect(code == 0, f"20: the simultaneous dry run failed: {stderr}")
            simultaneous = json.loads(stdout)
            candidate_roots = [candidate["root"] for candidate in simultaneous["candidates"]]
            kept_roots = [entry.get("root") for entry in simultaneous.get("kept", [])]
            expect(
                gone_root in kept_roots,
                f"20: the peer-referenced root is not KEEP in the simultaneous preview: {kept_roots}",
            )
            expect(
                extra_root in candidate_roots,
                f"20: the second eligible gone root is not a candidate: {candidate_roots}",
            )
            expect(
                group_root not in candidate_roots,
                f"20: the live checkout's root became a candidate: {candidate_roots}",
            )
            expect(os.path.isdir(gone_root), "20: the simultaneous preview removed the peer-referenced root")
            expect(os.path.isdir(extra_root), "20: the simultaneous preview removed the eligible root")
            expect(
                perm.read_bytes(peer_receipt_path) == peer_receipt_bytes,
                "20: the simultaneous preview rewrote the peer receipt",
            )
            with open(registry_path, "wb") as handle:
                handle.write(registry_original)
            os.chmod(registry_path, 0o600)
            os.remove(peer_receipt_path)
            # Guarded rollback: an intent with no filesystem effect restores the
            # row's selection only while the checkout receipt is still the recorded
            # preimage. A changed receipt keeps the root (pending/KEEP).
            code, stdout, stderr = oxide(proof, ["outputs", "prune", "--dry-run"], "20 rollback plan", group)
            expect(code == 0, f"20: the rollback dry run failed: {stderr}")
            rollback_plan = os.path.join(proof.root, "prune-rollback.json")
            with open(rollback_plan, "wb") as handle:
                handle.write(stdout.encode())
            rollback_sha = perm.sha256_file(rollback_plan)
            # A9: the source seam is prune.rs CRASH_ENVIRONMENT=OXIDE_TEST_PRUNE_CRASH,
            # fired AFTER the durable journal publication and BEFORE quarantine, and a
            # Pending stop exits 3. Require the exact exit, the named diagnostic and a
            # real durable intent that pins the complete recorded group and the actual
            # receipt preimage: a generic nonzero (for example a pre-intent refusal)
            # must not pass.
            intent_receipt_path = perm.receipt_path(pool, gone)
            intent_preimage = perm.sha256_file(intent_receipt_path)
            registry_path = os.path.join(pool, "oxide-managed", "outputs", "registry.json")
            gone_id = perm.hashlib.sha256(gone.encode()).hexdigest()
            group_rows_before = sorted(
                row.get("member")
                for row in perm.read_json(registry_path).get("rows", [])
                if row.get("checkout_id") == gone_id and row.get("root") == gone_root
            )
            expect(bool(group_rows_before), "20: the gone checkout has no recorded group rows to inject against")
            env = {**proof.env, "OXIDE_TEST_PRUNE_CRASH": "intent"}
            code, _, stderr = perm.oxide_env(
                proof,
                ["outputs", "prune", "--plan", rollback_plan, "--plan-sha256", rollback_sha],
                "20 intent crash",
                group,
                env,
            )
            expect(code == 3, f"20: the injected intent stop exited {code}, not 3: {stderr}")
            expect(
                "prune: injected stop at intent" in stderr
                and "OXIDE_TEST_PRUNE_CRASH=intent" in stderr
                and "the intent stays pending" in stderr,
                f"20: the injected intent diagnostic changed: {stderr}",
            )
            expect(os.path.isdir(gone_root), "20: the intent crash removed the root")
            expect(
                perm.sha256_file(intent_receipt_path) == intent_preimage,
                "20: the intent stop changed the receipt preimage",
            )
            intent = next(
                (entry for entry in perm.read_json(registry_path).get("intents", []) if entry["identity"]["root"] == gone_root),
                None,
            )
            expect(intent is not None, "20: the injected intent stop published no durable intent")
            evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
            if evidence:
                with open(os.path.join(evidence, "case20-intent-registry.json"), "wb") as handle:
                    handle.write(perm.read_bytes(registry_path))
            expect(
                intent.get("prior_receipt_sha") == intent_preimage,
                f"20: the intent does not pin the receipt preimage: {intent.get('prior_receipt_sha')} vs {intent_preimage}",
            )
            injected_group = [row.get("member") for row in (intent.get("group") or [])]
            expect(
                sorted(injected_group) == sorted(set(injected_group)),
                f"20: the injected intent group holds duplicate members: {injected_group}",
            )
            expect(
                sorted(injected_group) == group_rows_before,
                f"20: the injected intent group is not the COMPLETE recorded group: {sorted(injected_group)} vs {group_rows_before}",
            )
            expect(
                intent.get("receipt_settled") is False,
                "20: the injected intent was already settled at the stop",
            )
            receipt_path = perm.receipt_path(pool, gone)
            original_receipt = perm.read_bytes(receipt_path)
            with open(receipt_path, "wb") as handle:
                handle.write(original_receipt + b"\n")
            os.chmod(receipt_path, 0o600)
            code, _, _ = oxide(
                proof,
                ["outputs", "prune", "--plan", rollback_plan, "--plan-sha256", rollback_sha],
                "20 guarded rollback",
                group,
            )
            expect(code != 0, "20: a rollback with a changed receipt reported success")
            expect(os.path.isdir(gone_root), "20: the changed-receipt rollback removed the root")
            with open(receipt_path, "wb") as handle:
                handle.write(original_receipt)
            os.chmod(receipt_path, 0o600)
            code, _, stderr = oxide(
                proof,
                ["outputs", "prune", "--plan", rollback_plan, "--plan-sha256", rollback_sha],
                "20 rollback replay",
                group,
            )
            if code != 0:
                # A settled rollback may stop after restoring the row; one identical
                # replay then performs the planned deletion. A kept root is not a
                # settled rollback.
                expect(os.path.isdir(gone_root), f"20: the restored rollback failed and removed the root: {stderr}")
                code, _, stderr = oxide(
                    proof,
                    ["outputs", "prune", "--plan", rollback_plan, "--plan-sha256", rollback_sha],
                    "20 rollback replay 2",
                    group,
                )
            expect(code == 0, f"20: the restored rollback did not settle: {stderr}")
            expect(not os.path.exists(gone_root), "20: the rolled-back root survived its deletion")
            # A second gone checkout carries the terminal crash image oracle.
            terminal = make_checkout(proof, "terminal")
            code, _, _ = oxide(proof, ["build"], "20 terminal build", terminal)
            expect(code == 0, "20: terminal build failed")
            adopt(proof, terminal, "20 terminal adopt")
            terminal_root = root_of(pool, terminal)
            os.rename(terminal, terminal + "-moved")
            code, stdout, stderr = oxide(proof, ["outputs", "prune", "--dry-run"], "20 terminal dry", group)
            expect(code == 0, f"20: terminal dry run failed: {stderr}")
            expect(
                any(candidate["root"] == terminal_root for candidate in json.loads(stdout)["candidates"]),
                "20: the terminal root is not a candidate",
            )
            plan_path = os.path.join(proof.root, "prune-plan.json")
            with open(plan_path, "wb") as handle:
                handle.write(stdout.encode())
            plan_sha = perm.sha256_file(plan_path)
            gone = terminal
            gone_root = terminal_root
            # Terminal crash image oracle: the intent's expected terminal SHA and
            # FIXED timestamp are durable before publication and the published
            # receipt is exactly that image.
            env = {**proof.env, "OXIDE_TEST_PRUNE_CRASH": "terminal-image"}
            code, _, stderr = perm.oxide_env(
                proof,
                ["outputs", "prune", "--plan", plan_path, "--plan-sha256", plan_sha],
                "20 terminal image crash",
                group,
                env,
            )
            expect(code != 0, "20: the injected terminal-image stop did not stop the prune")
            expect(
                "prune: injected stop at terminal-image" in stderr
                and "OXIDE_TEST_PRUNE_CRASH=terminal-image" in stderr,
                f"20: the terminal-image diagnostic changed: {stderr}",
            )
            registry = perm.read_json(os.path.join(pool, "oxide-managed", "outputs", "registry.json"))
            intent = next((entry for entry in registry.get("intents", []) if entry["identity"]["root"] == gone_root), None)
            expect(intent is not None, "20: the terminal crash lost its intent")
            evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
            if evidence:
                with open(os.path.join(evidence, "case20-prune-registry.json"), "wb") as handle:
                    handle.write(perm.read_bytes(os.path.join(pool, "oxide-managed", "outputs", "registry.json")))
            expected = intent.get("expected_terminal_sha")
            fixed_ms = intent.get("terminal_ms")
            expect(intent.get("phase") == "deleted", f"20: terminal phase is {intent.get('phase')}, not deleted")
            expect(
                intent.get("receipt_settled") is False,
                "20: the terminal-image stop fired after intent settlement",
            )
            expect(expected is not None and fixed_ms is not None, f"20: no fixed terminal image: {intent}")
            published = perm.sha256_file(perm.receipt_path(pool, gone))
            expect(published == expected, "20: the published receipt is not the recorded terminal image")
            expect(not os.path.exists(gone_root), "20: the root survived its terminal deletion")
            # A replay accepts the exact image and settles the intent without a
            # rewrite; the fixed timestamp survives.
            code, _, stderr = oxide(proof, ["outputs", "prune", "--plan", plan_path, "--plan-sha256", plan_sha], "20 settle", group)
            expect(code == 0, f"20: settling the terminal image failed: {stderr}")
            settled = receipt(pool, gone)
            expect(settled["pruned"]["at_ms"] == fixed_ms, f"20: the terminal timestamp changed: {settled.get('pruned')}")
            expect(perm.sha256_file(perm.receipt_path(pool, gone)) == expected, "20: the replay rewrote the terminal image")
            expect(not os.path.exists(gone_root), "20: the root reappeared")
            # An unrelated gone checkout is still an eligible candidate and is pruned
            # independently, so the peer KEEP above is specific to the referenced root.
            unrelated = make_checkout(proof, "gone-unrelated")
            code, _, stderr = oxide(proof, ["build"], "20 unrelated build", unrelated)
            expect(code == 0, f"20: unrelated build failed: {stderr}")
            adopt(proof, unrelated, "20 unrelated adopt")
            unrelated_root = root_of(pool, unrelated)
            os.rename(unrelated, unrelated + "-moved")
            code, stdout, stderr = oxide(proof, ["outputs", "prune", "--dry-run"], "20 unrelated dry", group)
            expect(code == 0, f"20: unrelated dry run failed: {stderr}")
            unrelated_preview = json.loads(stdout)
            candidate_roots = [candidate["root"] for candidate in unrelated_preview["candidates"]]
            expect(
                unrelated_root in candidate_roots,
                f"20: the unrelated gone root is not an eligible candidate: {candidate_roots}",
            )
            # Specificity in the SAME preview: the eligible deletion names the gone
            # root and the live checkout's root is never a candidate.
            expect(
                group_root not in candidate_roots,
                f"20: the live checkout's root is a candidate beside the eligible one: {candidate_roots}",
            )
            # A9 honest limitation, not silently dropped: a peer receipt that keeps its
            # root is asserted KEEP in its own preview earlier, but a single preview
            # showing peer KEEP and this eligible deletion simultaneously is still
            # missing; the peer receipt is removed before this run.
            expect(
                not os.path.exists(peer_receipt_path),
                "20: the peer receipt is supposed to be gone before the unrelated preview",
            )
            unrelated_plan = os.path.join(proof.root, "prune-unrelated.json")
            with open(unrelated_plan, "wb") as handle:
                handle.write(stdout.encode())
            code, _, stderr = oxide(
                proof,
                ["outputs", "prune", "--plan", unrelated_plan, "--plan-sha256", perm.sha256_file(unrelated_plan)],
                "20 unrelated prune",
                group,
            )
            expect(code == 0, f"20: the unrelated prune failed: {stderr}")
            expect(not os.path.exists(unrelated_root), "20: the unrelated root was not pruned")
            DIAG.finished("20.1")
        except BaseException as error:
            DIAG.failed_step(proof, "20.1", error)

USAGE = (
    "usage: test-profile-outputs.py [--plan|--self-check|--diagnostic]\n"
    "--plan prints the fixed allocation and cases; --self-check runs the pure\n"
    "predicates only (registry, detach record, attachment facts, slot state,\n"
    "reserve-stop report, diagnostic state machine). --plan has no filesystem\n"
    "effects; --self-check only uses its own temporary directory and never the\n"
    "runner's evidence root. --diagnostic runs the same fixed 20 cases in ONE\n"
    "session, records every oracle failure instead of stopping at the first,\n"
    "skips only steps whose prerequisites are unmet, still stops on any safety\n"
    "failure (deadline, custody, unsettled child, unrecordable evidence), and\n"
    "writes ONE diagnostic receipt that never claims PASS or closure. It is not\n"
    "a counted native attempt and no other argument is accepted."
)


def diagnostic_main(proof: Proof, repo: str) -> int:
    """Run the whole fixed matrix once in diagnostic mode and write the ONE
    diagnostic receipt. Coverage aid only: it never claims a counted PASS,
    closure or qualified native evidence, even with zero oracle failures."""
    DIAG.reset()
    DIAG.mode = True
    primary = None
    secondary: list = []
    try:
        run(proof, repo)
    except BaseException as error:
        primary = error
        if DIAG.aborted is None:
            DIAG.aborted = {
                "step": None,
                "case": None,
                "kind": DIAG.classify(error),
                "observed": str(error)[-2000:],
                "unclosed": sorted(getattr(proof, "unclosed", set())),
            }
    try:
        detach_owned_volume(proof, "failure" if primary is not None else "diagnostic")
    except BaseException as error:
        secondary.append(f"ordinary detach failed: {error!r}")
        if primary is None:
            primary = error
    record = diagnostic_record(proof, _CASE_PROGRESS["started_ms"], primary, secondary)
    try:
        path = write_diagnostic_receipt(record)
    except BaseException as error:
        # A secondary failure never hides or replaces the primary one.
        print(
            f"the diagnostic receipt could not be written ({error!r}); "
            "no complete-list claim is made and the primary outcome stands",
            file=sys.stderr,
            flush=True,
        )
        if primary is not None:
            raise primary
        raise
    print(
        f"profile-outputs diagnostic: {record['outcome']}; "
        f"{len(record['failures'])} recorded failure(s), {len(record['skips'])} skip(s), "
        f"receipt {path}",
        flush=True,
    )
    if primary is not None:
        raise primary
    return 0 if record["outcome"] == "no_oracle_failures" else 1


def main() -> int:
    arguments = sys.argv[1:]
    if arguments == ["--plan"]:
        return plan_only()
    if arguments == ["--self-check"]:
        pure_diagnostic_controls()
        pure_registry_controls()
        pure_detach_record_control()
        pure_attachment_facts_control()
        pure_slot_state_control()
        pure_reserve_stop_control()
        pure_child_custody_control()
        return 0
    if arguments == [DIAGNOSTIC_ARGUMENT]:
        DIAG.mode = True
    elif arguments:
        print(USAGE, file=sys.stderr)
        return 2
    if sys.platform != "darwin":
        fail("the profile-outputs proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the runner's scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
        and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified runner",
    )
    # No signal handler, timer or harness of this script's own: the qualified
    # supervisor owns the deadline, reserve sampling, census/closure and
    # cleanup. Child timeouts only read the deadline it exports.
    root = perm.tempfile.mkdtemp(prefix="profile-outputs.", dir=scratch)
    repo = os.path.dirname(HERE)
    proof = Proof(root)
    if DIAG.mode:
        return diagnostic_main(proof, repo)
    try:
        run(proof, repo)
    except BaseException as error:
        try:
            detach_owned_volume(proof, "failure")
        except BaseException as detach_error:
            print(
                f"ordinary detach after failure also failed: {detach_error!r}",
                file=sys.stderr,
            )
        try:
            write_assertion_receipt("fail", error)
        except BaseException as receipt_error:
            # A secondary failure never hides or replaces the primary one.
            print(
                f"the assertion receipt could not be written ({receipt_error!r}); "
                "the primary failure above stands",
                file=sys.stderr,
                flush=True,
            )
        raise
    detach_owned_volume(proof, "success")
    # On success a missing or failed assertion receipt FAILS the proof.
    receipt = write_assertion_receipt("pass", None)
    print(f"profile-outputs proof: 20 cases held; assertion receipt {receipt}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
