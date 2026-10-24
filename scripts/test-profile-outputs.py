#!/usr/bin/env python3
"""Owned output links at profile directories (oxide#133): the fixed native
matrix.

The qualified shared supervisor owns the deadline, 512 MiB monitored scratch,
continuous host reserve sampling, process census/closure and cleanup. The
brief's scoped amendment (root owner, 2026-10-23T16:10Z, under the temporary
Supervisor delegation) makes this selector/script/plan 600 s TOTAL instead of
420; the old 420 publication at 030836f6 stays historical and unactivated. The
helper's work cutoff is about 565 s (its 30 s reserve plus a 5 s refusal
margin), inside which cleanup and image detach must finish. The published
300-450 s work estimate is UNMEASURED, not an established fit:

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
288 MiB staging peak + 128 MiB image backing + 32 MiB fixtures + 32 MiB
evidence + 32 MiB margin = 512 MiB monitored.

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
expect, fail, summary, MIB = perm.expect, perm.fail, perm.summary, perm.MIB

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
# selected; one SEPARATE deliberate low-reserve control registers another pool
# above the image's free space to prove the honest internal fallback.
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
    print("  fit             UNMEASURED: 300-450 s estimated work inside the ~565 s cutoff;")
    print("                  no native, trial or setup-only run has been activated, so the")
    print("                  estimate is not an established fit")
    print("  cpu             planning request 6 CPU / 8 GiB: two SEQUENTIAL stagings at")
    print("                  STAGE_JOBS=6 each, fixture children at FIXTURE_JOBS=2 with at")
    print("                  most two gated writers (2 x 2 = 4); peak 6 Cargo jobs")
    print("  time estimate   two sequential 6-job stagings ~45-80 s each, 25 s setup,")
    print("                  150-240 s cases (base control, p0/parent races, unknown")
    print("                  custody, prune oracles), 30 s cleanup -> 300-450 s estimated work")
    print("  history         the 420 s publication at 030836f6 is historical and unactivated")
    print("  amendment       GRANTED scoped bound: 600 s total (root owner 2026-10-23T16:10Z),")
    print("                  512 MiB unchanged; no disk increase, runner change or native start")
    print("  disk            288 MiB staging peak, counted as ONE staging target at a time")
    print("                  (each is copied out and discarded before the next starts; two")
    print("                  live targets are NOT assumed), + 128 MiB image backing +")
    print("                  32 MiB fixtures + 32 MiB evidence + 32 MiB margin = 512 MiB")
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
    print("  peer keep       a REAL peer checkout receipt for the same root (not a")
    print("                  synthetic row or a missing receipt) keeps that root")
    print("  clean           the reserve stop uses a positively created directory-budget")
    print("                  pool with a gated delegating fake cargo; the removable")
    print("                  watchdog is removal-only and is not claimed as a reserve stop")
    print("  environment     one private HOME, CARGO_HOME and directory pool per session")
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


def write_attachment_facts(name: str, record: dict) -> None:
    """Persist bounded attachment custody in the guard's OWN evidence root,
    before or after a risky operation, so an interrupted run still leaves the
    last known mount/device/UUID/image facts for the custodian."""
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    if not evidence:
        return None
    path = os.path.join(evidence, name)
    with open(path + ".new", "w", encoding="utf-8") as handle:
        json.dump(record, handle, indent=2, sort_keys=True)
        handle.write("\n")
    os.replace(path + ".new", path)
    return path


def attachment_facts(proof: Proof, label: str, state: str) -> dict:
    """Bounded last-known attachment facts for one owned mount."""
    mount = proof.mount
    directory = owned_image_dir()
    backing = os.path.join(directory, "volume.sparseimage")
    details = {}
    if mount:
        info = proof.run(["/usr/sbin/diskutil", "info", "-plist", mount], timeout=30)
        if info.returncode == 0:
            try:
                plist = perm.plistlib.loads(info.stdout)
                details = {
                    "device": plist.get("DeviceIdentifier"),
                    "volume_uuid": plist.get("VolumeUUID"),
                    "owners": plist.get("Owners"),
                    "mount_point": plist.get("MountPoint"),
                }
            except Exception:
                details = {"plist": "unreadable"}
    return {
        "label": label,
        "state": state,
        "mount": mount,
        "image": backing,
        "image_dir": directory,
        "backing_present": os.path.exists(backing),
        "owned": True,
        "unclosed_children": sorted(getattr(proof, "unclosed", set())),
        "details": details,
    }


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
    write_attachment_facts("attachment-facts.json", attachment_facts(proof, label, "attached"))
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


def pure_attachment_facts_control() -> None:
    """No-subprocess control for A10 custody facts: they are written into the
    evidence root, replaced atomically, leave no temporary file behind, and
    are skipped entirely when no evidence root is configured."""
    written = write_attachment_facts("attachment-unset.json", {"state": "never"})
    expect(written is None, "the attachment facts were written without an evidence root")
    with tempfile.TemporaryDirectory() as root:
        os.environ["OXIDE_PROOF_EVIDENCE"] = root
        try:
            write_attachment_facts("attachment-facts.json", {"state": "attached", "mount": "/Volumes/x", "unclosed_children": []})
            write_attachment_facts("attachment-facts.json", {"state": "detached", "mount": "/Volumes/x", "unclosed_children": []})
            with open(os.path.join(root, "attachment-facts.json"), encoding="utf-8") as handle:
                record = json.load(handle)
            expect(record.get("state") == "detached", f"the attachment facts were not replaced: {record}")
            expect(record.get("mount") == "/Volumes/x", f"the attachment facts lost the mount: {record}")
            expect(
                not os.path.exists(os.path.join(root, "attachment-facts.json.new")),
                "a temporary attachment-facts file was left behind",
            )
        finally:
            os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
    print("pure attachment-facts control: ok", flush=True)


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
        record = attachment_facts(proof, label, "unresolved: unclosed children")
        record["reason"] = "refused to detach while child custody is unknown"
        write_attachment_facts("detach-unresolved.json", record)
        fail(f"{label}: refusing ordinary detach while {len(unclosed)} child(ren) are unclosed; the mount and backing are retained")
        return
    expect(os.path.lexists(mount), f"{label}: the owned mount {mount} vanished before ordinary detach")
    # Persist the last-known facts BEFORE the risky calls, so an interruption
    # (including a timeout) still leaves custody facts in evidence.
    write_attachment_facts("attachment-facts.json", attachment_facts(proof, label, "detaching"))
    try:
        result = proof.run(["/usr/bin/hdiutil", "detach", mount], timeout=60)
    except Exception as error:  # a timeout or a harness fault is unresolved custody
        record = attachment_facts(proof, label, "unresolved: detach did not complete")
        record["reason"] = f"detach raised {error!r}"
        write_attachment_facts("detach-unresolved.json", record)
        fail(f"{label}: ordinary detach of {mount} did not complete ({error!r}); the backing is retained in the guard evidence")
        return
    if result.returncode != 0:
        details = {}
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
        except Exception as error:
            details = {"unavailable": repr(error)}
        record = detach_failure_record(
            label,
            mount,
            result.returncode,
            result.stderr.decode(errors="replace"),
            backing,
            os.path.exists(backing),
            details,
        )
        evidence = os.environ["OXIDE_PROOF_EVIDENCE"]
        with open(os.path.join(evidence, "detach-failure.json"), "w", encoding="utf-8") as handle:
            json.dump(record, handle, indent=2)
            handle.write("\n")
        write_attachment_facts("detach-unresolved.json", attachment_facts(proof, label, "unresolved: detach failed"))
        fail(f"{label}: ordinary detach of {mount} failed; the backing is retained in the guard evidence")
        return
    proof.mount = None
    write_attachment_facts("attachment-facts.json", attachment_facts(proof, label, "detached"))
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
    candidate), in a pool that has not published the successor fence."""
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


def run(proof: Proof, repo: str) -> None:
    pure_registry_controls()
    pure_detach_record_control()
    pure_attachment_facts_control()
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
    # ----- case 11: explicit migration of a base-created checkout ---------
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

    # ----- case 01: a real target with a debug member link -----------------
    first = make_checkout(proof, "first")
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

    # ----- case 02: a member mutation waits for a gated writer ------------
    gate = os.path.join(proof.root, "gate2")
    os.makedirs(gate)
    env = {**proof.env, "OXIDE_PROFILE_GATE": gate}
    child = perm.start_child(proof, [proof.binary, "build"], first, env)
    wait_started(gate, "02 gated build start")
    before = custody_snapshot(pool, first)
    second = perm.start_child(proof, [proof.binary, "outputs", "adopt"], first, proof.env)
    observed_wait(second, before, pool, first, "02")
    perm.stop_gate(gate, os.path.join(gate, "release"))
    code, _, _ = perm.finish_child(proof, child, "02 gated build")
    expect(code == 0, f"02: gated build exited {code}")
    code, _, _ = perm.finish_child(proof, second, "02 waited adopt")
    expect(code == 0, "02: the waiting adopt failed")
    expect(custody_snapshot(pool, first) == before, "02: the waited adopt changed authority")
    expect_layout(first, {"debug": f"link:{root}", "release": f"link:{root}"})

    # ----- case 03: test / bench / bench debug / doc mapping --------------
    code, _, stderr = oxide(proof, ["test"], "03 test", first)
    expect(code == 0, f"03: test failed: {stderr}")
    expect("fallback" not in summary(stderr), f"03: external placement unexpectedly fell back: {summary(stderr)}")
    code, _, stderr = oxide(proof, ["bench"], "03 bench", first)
    expect(code == 0, f"03: bench failed: {stderr}")
    code, _, stderr = oxide(proof, ["bench", "--profile", "debug"], "03 bench debug", first)
    expect(code == 0, f"03: bench debug failed: {stderr}")
    code, _, stderr = oxide(proof, ["doc"], "03 doc", first)
    expect(code == 0, f"03: doc failed: {stderr}")
    expect(os.path.isdir(os.path.join(first, "target", "doc")), "03: target/doc is missing")
    expect(not os.path.islink(os.path.join(first, "target", "doc")), "03: target/doc is a link")
    for member in ("debug", "release"):
        expect_layout(first, {member: f"link:{root}"})
    # test and doc ran through the debug member; bench through release.
    expect(os.path.exists(os.path.join(root, "release", "deps")), "03: no release member payload")

    # ----- case 04: a host --target member --------------------------------
    host = proof.run(["rustc", "-vV"], cwd=first, env=proof.env, timeout=30).stdout.decode()
    triple = next(line.split(": ", 1)[1] for line in host.splitlines() if line.startswith("host: "))
    code, _, stderr = oxide(proof, ["build", "--target", triple], "04 host target", first)
    expect(code == 0, f"04: host-target build failed: {stderr}")
    expect_layout(first, {f"{triple}/debug": f"link:{root}"})
    report = perm.plan(proof, first, "04 plan")
    expect(f"{triple}/debug" in {entry["member"] for entry in report["members"]}, "04: plan misses the triple member")

    # ----- case 05: an unknown target refuses only when linked ------------
    unknown = make_checkout(proof, "unknown-target")
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
        perm.read_bytes(external_marker) == b"run --help",
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
    # ----- case 06: opaque custom-profile refusal -------------------------
    code, _, stderr = oxide(proof, ["build", "--profile", "custom"], "06 modelled custom", first)
    expect(code != 0 and "not an Oxide profile" in stderr, f"06: modelled custom profile was not refused: {stderr}")
    # Case 05's unlinked run wrote the marker; clear it first so this absence
    # check is about case 06's refusal, not leftover state.
    if os.path.exists(external_marker):
        os.remove(external_marker)
    code, _, stderr = perm.oxide_env(proof, ["nextest", "--profile", "custom"], "06 opaque custom", first, fake_env)
    expect(code != 0, f"06: an opaque tool's custom profile was accepted: {stderr}")
    expect(not os.path.exists(external_marker), "06: the opaque tool ran in a linked checkout")

    # ----- case 07: exec declarations -------------------------------------
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

    # ----- case 08: real nonempty kept, empty converted -------------------
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
    other = make_checkout(proof, "empty-member")
    # An empty release directory is converted only by this explicit adopt.
    os.makedirs(os.path.join(other, "target", "release"), exist_ok=True)
    adopt(proof, other, "08 empty adopt")
    expect_layout(other, {"debug": f"link:{root_of(pool, other)}", "release": f"link:{root_of(pool, other)}"})

    # ----- case 09: container-style preflight sees a real target ----------
    expect(is_real_dir(os.path.join(first, "target")), "09: target is not a real directory for a container preflight")
    env = {**proof.env, "CARGO_TARGET_DIR": os.path.join(first, "target")}
    result = proof.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=first, env=env, timeout=60)
    expect(result.returncode == 0, "09: metadata through the real target failed")

    # ----- case 10: legacy whole-target state stays unchanged under state 4 ---
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

    # ----- case 12: P0-P4 replay, races and exact rollback ----------------
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

    # ----- case 13: an active writer blocks migration ---------------------
    blocked = make_checkout(proof, "blocked")
    auto_legacy(proof, blocked)
    gate = os.path.join(proof.root, "gate13")
    os.makedirs(gate)
    child = perm.start_child(proof, [proof.binary, "build"], blocked, {**proof.env, "OXIDE_PROFILE_GATE": gate})
    wait_started(gate, "13 writer start")
    before = custody_snapshot(pool, blocked)
    waiter = perm.start_child(proof, [proof.binary, "outputs", "adopt"], blocked, proof.env)
    observed_wait(waiter, before, pool, blocked, "13")
    expect(
        os.path.islink(os.path.join(blocked, "target")),
        "13: the migration changed the target while a writer ran",
    )
    perm.stop_gate(gate, os.path.join(gate, "release"))
    code, _, _ = perm.finish_child(proof, child, "13 writer")
    expect(code == 0, f"13: the gated writer exited {code}")
    code, _, _ = perm.finish_child(proof, waiter, "13 waiting adopt")
    expect(code == 0, "13: the waiting adopt failed")
    expect_layout(blocked, {"debug": f"link:{root_of(pool, blocked)}"})

    # ----- case 14: per-member fallback and return ------------------------
    fallback = make_checkout(proof, "fallback")
    code, _, _ = oxide(proof, ["build"], "14 prebuild", fallback)
    expect(code == 0, "14: prebuild failed")
    adopt(proof, fallback, "14 adopt")
    external = root_of(pool, fallback)
    expect_layout(fallback, {"debug": f"link:{external}"})
    # The volume goes away: the next command's intermediate slot falls back to
    # the internal pool while its member link selects the external root, so it
    # refuses instead of writing through an unsupervised drive.
    detached = proof.run(["/usr/bin/hdiutil", "detach", proof.mount], timeout=60)
    expect(detached.returncode == 0, "14: ordinary detach failed")
    proof.mount = None
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

    # ----- case 15: detach and re-adopt ----------------------------------
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

    # ----- case 16: whole and selected clean keep links and markers -------
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
    perm.wait_for(os.path.join(gate, "started"), "16 watched clean writer start")
    with open(readings, "w", encoding="utf-8") as handle:
        handle.write(str(22 * 1024 * 1024 * 1024 - 4096))
    code, _, stderr = perm.finish_child(proof, child, "16 watched clean")
    expect(code != 0, f"16: a stopped clean claimed success: {stderr}")
    expect(
        any(word in stderr.lower() for word in ("reserve", "watchdog", "stop")),
        f"16: the stop was not reported honestly: {stderr}",
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

    # ----- case 17: refusals ---------------------------------------------
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

    # ----- case 18: real and linked placements key different cold slots ----
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
    slots_dir = os.path.join(pool, "payload", "slots")
    slots_before = sorted(os.listdir(slots_dir))
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
    facts_linked = perm.plan(proof, cold, "18 linked plan")["members"]
    expect(
        facts_real != facts_linked,
        f"18: placement change did not change the planned members: {facts_real} vs {facts_linked}",
    )

    # ----- case 19: the staged state-3 binary refuses state 4 -------------
    snapshot = {member: os.readlink(member_link(migrated, member)) for member in ("debug", "release")}
    receipt_before = perm.sha256_file(perm.receipt_path(pool, migrated))
    slots_dir = os.path.join(pool, "payload", "slots")
    slots_before = sorted(os.listdir(slots_dir))
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
        "19: the refused state-3 run created or changed a slot",
    )

    # ----- case 20: root-group prune keeps another checkout's reference ---
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
    # A real, valid peer receipt for the same root (not a synthetic row and
    # not a missing receipt): the peer checkout exists, its committed receipt
    # selects the gone root with the identical recorded identity.
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
    env = {**proof.env, "OXIDE_TEST_PRUNE_CRASH": "intent"}
    code, _, stderr = perm.oxide_env(
        proof,
        ["outputs", "prune", "--plan", rollback_plan, "--plan-sha256", rollback_sha],
        "20 intent crash",
        group,
        env,
    )
    expect(code != 0, "20: the injected intent stop did not stop the prune")
    expect(os.path.isdir(gone_root), "20: the intent crash removed the root")
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
    expect(
        any(candidate["root"] == unrelated_root for candidate in unrelated_preview["candidates"]),
        f"20: the unrelated gone root is not an eligible candidate: {unrelated_preview['candidates']}",
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



USAGE = "usage: test-profile-outputs.py [--plan|--self-check]\n--plan prints the fixed allocation and cases; --self-check runs the pure\nregistry predicates only. Neither has filesystem effects."


def main() -> int:
    arguments = sys.argv[1:]
    if arguments == ["--plan"]:
        return plan_only()
    if arguments == ["--self-check"]:
        pure_registry_controls()
        pure_detach_record_control()
        pure_attachment_facts_control()
        return 0
    if arguments:
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
    try:
        run(proof, repo)
    except BaseException:
        try:
            detach_owned_volume(proof, "failure")
        except BaseException as error:
            print(f"ordinary detach after failure also failed: {error!r}", file=sys.stderr)
        raise
    detach_owned_volume(proof, "success")
    print("profile-outputs proof: 20 cases held", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
