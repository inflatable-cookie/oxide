#!/usr/bin/env python3
"""Owned output links at profile directories (oxide#133): the fixed native
matrix.

The qualified shared supervisor owns the 420 s deadline (roughly 385 s of work
after its 30 s shutdown margin), 512 MiB monitored scratch, continuous host
reserve sampling, process census/closure and cleanup:

    python3 scripts/bounded-test.py --limit 420 --scratch-budget-mib 512 \\
        -- python3 scripts/test-profile-outputs.py

This script is a whole fixed script with no supervision, clock, signal or
scratch of its own. Every write stays inside the supervisor-owned scratch
(`OXIDE_TASK_SCRATCH_ROOT`, also `TMPDIR`/`TMP`/`TEMP`); evidence goes to
`OXIDE_PROOF_EVIDENCE`. It models the removable drive as ONE owned 128 MiB
APFS sparse image at a unique `/Volumes/<name>` and uses one private `HOME`,
`CARGO_HOME`, directory pool and fixture tree per session.

Staging (measured at author time, not a gate): two independent configs from
authenticated pinned source, this HEAD's candidate (~37-60 s) and base
`4a70b83` (~37-60 s); the script discards each staging target after copying
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
    "src/main.rs": 'fn main() { println!("profile-app {}", profileapp::value()); }\n',
    ".gitignore": "/target/\n/.oxide-outputs/\n",
}


def plan_only() -> int:
    print("profile-outputs proof allocation (unmeasured staging estimate)")
    print(f"  script          {os.path.abspath(__file__)}")
    print(f"  supervisor      scripts/bounded-test.py --limit 420 --scratch-budget-mib 512")
    print(f"  deadline        420 s total; helper work window ~385 s ({SHUTDOWN_MARGIN_S} s margin)")
    print("  disk            288 MiB staging peak + 128 MiB image + 32 MiB fixtures + 32 MiB evidence + 32 MiB margin")
    print(f"  staging         candidate {os.environ.get('OXIDE_PROFILE_HEAD', 'this HEAD')} and base {BASE}")
    print("  seed            none: both stagings are plain Cargo offline builds from authenticated pinned source")
    print("  monitoring      supervisor reserve sampling, census/closure and cleanup; this script runs no clock or harness")
    print(f"  image           one owned 128 MiB APFS sparse image at a unique /Volumes/<name>, ordinary detach")
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


def receipt(pool: str, checkout: str) -> dict:
    return perm.read_json(perm.receipt_path(pool, checkout))


def item(pool: str, checkout: str, key: str):
    return receipt(pool, checkout)[key]


def member_link(checkout: str, member: str) -> str:
    return os.path.join(checkout, "target", member)


def expect_layout(checkout: str, members: dict) -> None:
    """`target` is a real directory and each named member has the expected
    placement ("link:<root>" or "real")."""
    target = os.path.join(checkout, "target")
    expect(is_real_dir(target), f"{target} is not a real directory")
    for member, expected in members.items():
        path = member_link(checkout, member)
        if expected == "real":
            expect(is_real_dir(path), f"{path} is not a real directory")
        else:
            root = expected[len("link:"):]
            expect(os.path.islink(path) and os.readlink(path) == root, f"{path} -> {os.readlink(path) if os.path.islink(path) else 'not a link'}, not {root}")


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


def register(proof: Proof, pool: str, mode: str = "permanent") -> None:
    mount = perm.attach_image(proof, proof.root, "ProfileOutputs-" + secrets.token_hex(3))
    code, _, stderr = proof.oxide(
        ["pool", "register-removable", mount, "--reserve", "2GiB", "--cap", "96MiB", "--mode", mode],
        "register removable",
        proof.root,
    )
    expect(code == 0, f"registration failed: {stderr}")
    expect(os.path.isfile(perm.registration_path(pool)), "no removable registration")


def oxide(proof: Proof, args: list, label: str, cwd: str) -> tuple:
    return proof.oxide(args, label, cwd)


def adopt(proof: Proof, checkout: str, label: str) -> None:
    code, _, stderr = proof.oxide(["outputs", "adopt"], label, checkout)
    expect(code == 0, f"{label}: adopt exited {code}: {stderr}")


def run(proof: Proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    print(f"profile-outputs proof {os.getpid()} source {source}", flush=True)
    # Independent sequential staging from authenticated pinned source, each in
    # its own target, copied out and then discarded.
    base_src = source_at(proof, repo, BASE, "base")
    base = layout.stage_at(proof, base_src, "base")
    shutil.rmtree(base_src)
    candidate = layout.stage_at(proof, repo, "candidate")
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
    register(proof, pool, "unused", "unused")

    # ----- case 01: a real target with a debug member link -----------------
    first = make_checkout(proof, "first")
    code, _, stderr = oxide(proof, ["build"], "01 build", first)
    expect(code == 0, f"01: build failed: {stderr}")
    adopt(proof, first, "01 adopt")
    root = root_of(pool, first)
    expect_layout(first, {"debug": f"link:{root}", "release": f"link:{root}"})
    for name in ("CACHEDIR.TAG", ".rustc_info.json"):
        path = os.path.join(first, "target", name)
        expect(os.path.exists(path) and not os.path.islink(path), f"01: {path} is not real")
    report = perm.plan(proof, first, "01 plan")
    expect(report["layout"] == "profile", f"01: plan layout {report['layout']}")
    members = {entry["member"]: entry["state"] for entry in report["members"]}
    expect(members.get("debug") == "owned-link", f"01: plan members {members}")
    expect(fence_in(pool) == STATE4_FENCE, f"01: fence {fence_in(pool)}")
    expect(item(pool, first, "version") == "oxide.outputs.transition/3", "01: receipt version")
    registry = perm.read_json(os.path.join(pool, "oxide-managed", "outputs", "registry.json"))
    expect(registry["schema"] == 2, f"01: registry schema {registry['schema']}")
    expect(
        sorted({row["member"] for row in registry["rows"] if row["checkout_id"] == perm.hashlib.sha256(first.encode()).hexdigest()}),
        ["debug", "release"],
    )

    # ----- case 02: a member mutation waits for a gated writer ------------
    gate = os.path.join(proof.root, "gate2")
    os.makedirs(gate)
    env = {**proof.env, "OXIDE_PROFILE_GATE": gate}
    child = perm.start_child(proof, [proof.binary, "build"], first, env)
    wait_started(gate, "02 gated build start")
    second = perm.start_child(proof, [proof.binary, "outputs", "adopt"], first, proof.env)
    time.sleep(0.4)
    expect(second.poll() is None, "02: a mutation did not wait for the running writer")
    perm.stop_gate(gate, os.path.join(gate, "release"))
    code, _, _ = perm.finish_child(proof, child, "02 gated build")
    expect(code == 0, f"02: gated build exited {code}")
    perm.finish_child(proof, second, "02 waited adopt")
    expect_layout(first, {"debug": f"link:{root}", "release": f"link:{root}"})

    # ----- case 03: test / bench / bench debug / doc mapping --------------
    code, _, stderr = oxide(proof, ["test"], "03 test", first)
    expect(code == 0, f"03: test failed: {stderr}")
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
    adopt(proof, unknown, "05 adopt")
    code, _, stderr = oxide(proof, ["build", "--target", "targets/spec.json"], "05 linked", unknown)
    expect(code != 0 and "not a plain triple" in stderr, f"05: linked unknown target was not refused: {stderr}")
    expect_layout(unknown, {"debug": f"link:{root_of(pool, unknown)}"})

    # ----- case 06: opaque custom-profile refusal -------------------------
    code, _, stderr = oxide(proof, ["build", "--profile", "custom"], "06 modelled custom", first)
    expect(code != 0 and "not an Oxide profile" in stderr, f"06: modelled custom profile was not refused: {stderr}")
    code, _, stderr = oxide(proof, ["nextest", "--profile", "custom"], "06 opaque custom", first)
    expect(code != 0, f"06: an opaque tool's custom profile was accepted: {stderr}")

    # ----- case 07: exec declarations -------------------------------------
    script = os.path.join(first, "declared.sh")
    with open(script, "w", encoding="utf-8") as handle:
        handle.write("#!/bin/sh\nexit 0\n")
    os.chmod(script, 0o755)
    code, _, stderr = oxide(proof, ["exec", "--", script], "07 undeclared exec", first)
    expect(code != 0 and "declare" in stderr, f"07: undeclared exec was not refused: {stderr}")
    code, _, stderr = oxide(proof, ["exec", "--output", "debug", "--", script], "07 declared exec", first)
    expect(code == 0, f"07: declared exec failed: {stderr}")
    code, _, stderr = oxide(proof, ["exec", "--output", "custom", "--", script], "07 custom exec", first)
    expect(code != 0 and "not an output member" in stderr, f"07: custom declaration was accepted: {stderr}")
    code, _, stderr = oxide(proof, ["exec", "--output", "debug", "--output", "release", "--", script], "07 complete exec", first)
    expect(code == 0, f"07: complete declaration failed: {stderr}")

    # ----- case 08: real nonempty kept, empty converted -------------------
    mixed = make_checkout(proof, "mixed")
    code, _, _ = oxide(proof, ["build"], "08 prebuild", mixed)
    expect(code == 0, "08: prebuild failed")
    os.makedirs(os.path.join(mixed, "target", "release"), exist_ok=True)
    with open(os.path.join(mixed, "target", "release", "keep"), "wb") as handle:
        handle.write(b"keep")
    adopt(proof, mixed, "08 adopt")
    expect_layout(mixed, {"debug": f"link:{root_of(pool, mixed)}", "release": "real"})
    expect(perm.read_bytes(os.path.join(mixed, "target", "release", "keep")) == b"keep", "08: real member content changed")
    other = make_checkout(proof, "empty-member")
    code, _, _ = oxide(proof, ["build"], "08 empty prebuild", other)
    expect(code == 0, "08: empty prebuild failed")
    os.makedirs(os.path.join(other, "target", "release"), exist_ok=True)
    adopt(proof, other, "08 empty adopt")
    expect_layout(other, {"release": f"link:{root_of(pool, other)}"})

    # ----- case 09: container-style preflight sees a real target ----------
    expect(is_real_dir(os.path.join(first, "target")), "09: target is not a real directory for a container preflight")
    env = {**proof.env, "CARGO_TARGET_DIR": os.path.join(first, "target")}
    result = proof.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=first, env=env, timeout=60)
    expect(result.returncode == 0, "09: metadata through the real target failed")

    # ----- case 10: a legacy whole-target checkout is unchanged and warm ---
    legacy = make_checkout(proof, "legacy")
    code, _, _ = oxide(proof, ["build"], "10 prebuild", legacy)
    expect(code == 0, "10: prebuild failed")
    adopt(proof, legacy, "10 base-style adopt")
    legacy_root = root_of(pool, legacy)
    # A whole-target /2 layout, as the base binary creates it.
    expect(os.path.islink(os.path.join(legacy, "target")), "10: legacy target is not a link")
    expect(item(pool, legacy, "version") == "oxide.outputs.transition/2", "10: legacy receipt version changed")
    facts = item(pool, legacy, "links")
    expect(facts[0]["member"] == "target", "10: legacy member changed")
    code, _, stderr = oxide(proof, ["build"], "10 second", legacy)
    expect(code == 0, f"10: second legacy build failed: {stderr}")
    code, _, stderr = oxide(proof, ["build"], "10 warm", legacy)
    expect(code == 0, f"10: warm legacy build failed: {stderr}")
    expect("warm" in summary(stderr), f"10: legacy build was not warm: {summary(stderr)}")
    expect(os.path.islink(os.path.join(legacy, "target")), "10: the legacy link changed")
    expect(link_at(os.path.join(legacy, "target")) == legacy_root, "10: the legacy link text changed")
    expect(item(pool, legacy, "version") == "oxide.outputs.transition/2", "10: the legacy receipt was rewritten")

    # ----- case 11: explicit migration of a base-created checkout ---------
    migrated = make_checkout(proof, "migrate")
    code, _, _ = oxide(proof, ["build"], "11 prebuild", migrated)
    expect(code == 0, "11: prebuild failed")
    adopt(proof, migrated, "11 legacy adopt")
    # A source-shaped pre-oxide#129 checkout: a root/1 marker and no nonce in
    # the receipt or registry.
    migrated_root = make_root_one_fixture(pool, migrated)
    marker_path = os.path.join(migrated_root, MARKER)
    root_ino = os.stat(migrated_root).st_ino
    payload = sorted(os.listdir(migrated_root))
    adopt(proof, migrated, "11 migrate")
    expect_layout(migrated, {"debug": f"link:{migrated_root}", "release": f"link:{migrated_root}"})
    upgraded = perm.read_json(marker_path)
    expect(upgraded["version"] == "oxide.outputs.root/2", "11: marker not upgraded to root/2")
    nonce = upgraded["nonce"]
    expect(len(nonce) == 32, "11: migrated nonce is not 128-bit hex")
    migrated_receipt = receipt(pool, migrated)
    expect({p["selected"]["nonce"] for p in migrated_receipt["links"] if p.get("selected")} == {nonce}, "11: members do not share the upgraded nonce")
    expect(os.stat(migrated_root).st_ino == root_ino, "11: the root inode changed")
    present = {name for name in os.listdir(migrated_root) if name in payload}
    expect(present == set(payload), f"11: root payload changed: {present} vs {payload}")
    expect(not any(name.endswith(".oxide-old-2") for name in os.listdir(migrated_root)), "11: the marker backup was kept")
    registry = perm.read_json(os.path.join(pool, "oxide-managed", "outputs", "registry.json"))
    rows = [row for row in registry["rows"] if row["checkout_id"] == perm.hashlib.sha256(migrated.encode()).hexdigest()]
    expect({row["member"] for row in rows if row["selected"]} == {"debug", "release"}, f"11: registry members {rows}")
    expect({row["nonce"] for row in rows} == {nonce}, "11: registry nonces do not share the upgraded nonce")
    expect(migrated_receipt["version"] == "oxide.outputs.transition/3", "11: migrated receipt version")

    # ----- case 12: P0-P4 replay, races and exact rollback ----------------
    rollback = make_checkout(proof, "rollback")
    code, _, _ = oxide(proof, ["build"], "12 prebuild", rollback)
    expect(code == 0, "12: prebuild failed")
    adopt(proof, rollback, "12 legacy adopt")
    rollback_root = make_root_one_fixture(pool, rollback)
    old_bytes = perm.read_bytes(os.path.join(rollback_root, MARKER))
    env = {**proof.env, "OXIDE_TEST_MIGRATION_ROLLBACK": "marker"}
    code, _, stderr = perm.oxide_env(proof, ["outputs", "adopt"], "12 rollback", rollback, env)
    expect(code == 0, f"12: rollback adopt failed: {stderr}")
    expect(os.path.islink(os.path.join(rollback, "target")), "12: rollback did not restore the old link")
    expect(perm.read_bytes(os.path.join(rollback_root, MARKER)) == old_bytes, "12: rollback did not restore the old marker")
    expect(receipt(pool, rollback)["version"] == "oxide.outputs.transition/2", "12: rollback did not restore the /2 receipt")
    # A committed migration is never rolled back: the debug-only rollback
    # request after the commit leaves the profile layout and member links.
    adopt(proof, rollback, "12 migrate")
    env = {**proof.env, "OXIDE_TEST_MIGRATION_ROLLBACK": "1"}
    code, _, _ = perm.oxide_env(proof, ["outputs", "adopt"], "12 committed", rollback, env)
    expect(code == 0, "12: adopt on a committed profile checkout failed")
    expect_layout(rollback, {"debug": f"link:{root_of(pool, rollback)}"})
    expect(receipt(pool, rollback)["version"] == "oxide.outputs.transition/3", "12: committed migration changed format")

    # ----- case 13: an active writer blocks migration ---------------------
    blocked = make_checkout(proof, "blocked")
    code, _, _ = oxide(proof, ["build"], "13 prebuild", blocked)
    expect(code == 0, "13: prebuild failed")
    adopt(proof, blocked, "13 legacy adopt")
    gate = os.path.join(proof.root, "gate13")
    os.makedirs(gate)
    child = perm.start_child(proof, [proof.binary, "build"], blocked, {**proof.env, "OXIDE_PROFILE_GATE": gate})
    wait_started(gate, "13 writer start")
    waiter = perm.start_child(proof, [proof.binary, "outputs", "adopt"], blocked, proof.env)
    time.sleep(0.4)
    expect(waiter.poll() is None, "13: migration did not wait for the active writer")
    expect(os.path.islink(os.path.join(blocked, "target")) and not os.path.isdir(os.path.join(blocked, "target")), "13: migration changed the target while a writer ran")
    perm.stop_gate(gate, os.path.join(gate, "release"))
    perm.finish_child(proof, child, "13 writer")
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
    code, _, stderr = oxide(proof, ["build"], "14 fallback", fallback)
    expect(code != 0 and "fallback" in stderr.lower(), f"14: fallback build was not refused: {stderr}")
    expect(os.readlink(member_link(fallback, "debug")) == external, "14: the refusal changed the member link")
    remount = perm.attach_image(proof, proof.root, "ProfileOutputs-" + secrets.token_hex(3))
    expect(remount is not None, "14: cannot re-attach the image")

    # ----- case 15: detach and re-adopt ----------------------------------
    code, _, stderr = oxide(proof, ["outputs", "detach"], "15 detach", first)
    expect(code == 0, f"15: detach failed: {stderr}")
    expect_layout(first, {"debug": "real", "release": "real"})
    expect(receipt(pool, first)["state"] == "detached", "15: receipt not detached")
    expect(os.path.isdir(root), "15: a root was deleted by detach")
    adopt(proof, first, "15 readopt")
    expect_layout(first, {"debug": f"link:{root_of(pool, first)}"})

    # ----- case 16: whole and selected clean keep links and markers -------
    code, _, stderr = oxide(proof, ["clean", "--dry-run"], "16 clean dry", first)
    expect(code == 0, f"16: clean dry run failed: {stderr}")
    code, _, stderr = oxide(proof, ["clean"], "16 clean", first)
    expect(code == 0, f"16: clean failed: {stderr}")
    expect_layout(first, {"debug": f"link:{root_of(pool, first)}", "release": f"link:{root_of(pool, first)}"})
    expect(os.path.isfile(os.path.join(root, MARKER)), "16: clean removed the root marker")
    code, _, stderr = oxide(proof, ["clean", "-p", "profileapp"], "16 selected clean", first)
    expect(code == 0, f"16: selected clean failed: {stderr}")
    expect_layout(first, {"debug": f"link:{root_of(pool, first)}"})

    # ----- case 17: refusals ---------------------------------------------
    malformed = make_checkout(proof, "malformed")
    code, _, _ = oxide(proof, ["build"], "17 prebuild", malformed)
    adopt(proof, malformed, "17 adopt")
    code, _, stderr = oxide(proof, ["exec", "--output", "../escape", "--", script], "17 malformed member", malformed)
    expect(code != 0 and "not an output member" in stderr, f"17: a traversing member was accepted: {stderr}")
    foreign = os.path.join(malformed, "target", "release")
    os.remove(foreign) if os.path.islink(foreign) else None
    os.symlink("/tmp", foreign)
    code, _, stderr = oxide(proof, ["build"], "17 foreign link", malformed)
    expect(code != 0, f"17: a foreign member link was followed: {stderr}")
    os.remove(foreign)

    # ----- case 18: real and linked placements key different cold slots ----
    cold = make_checkout(proof, "cold")
    os.makedirs(os.path.join(cold, "target", "debug"))
    code, _, stderr = oxide(proof, ["build"], "18 real debug", cold)
    expect(code == 0, f"18: real-member build failed: {stderr}")
    facts_real = perm.plan(proof, cold, "18 real plan")["members"]
    adopt(proof, cold, "18 adopt")
    code, _, stderr = oxide(proof, ["build"], "18 linked debug", cold)
    expect(code == 0, f"18: linked build failed: {stderr}")
    expect("cold" in summary(stderr), f"18: linked placement reused the real neighbour's slot: {summary(stderr)}")
    facts_linked = perm.plan(proof, cold, "18 linked plan")["members"]
    expect(facts_real != facts_linked, "18: placement change did not change the planned members")

    # ----- case 19: the staged state-3 binary refuses state 4 -------------
    snapshot = {member: os.readlink(member_link(migrated, member)) for member in ("debug", "release")}
    receipt_before = perm.sha256_file(perm.receipt_path(pool, migrated))
    result = proof.run([base, "build"], cwd=migrated, env=proof.env, timeout=90)
    expect(result.returncode != 0, "19: the state-3 binary did not refuse a state-4 pool")
    print(result.stderr.decode(errors="replace")[-2000:])
    for member, target in snapshot.items():
        expect(os.readlink(member_link(migrated, member)) == target, "19: the refusal changed a member link")
    expect(perm.sha256_file(perm.receipt_path(pool, migrated)) == receipt_before, "19: the refusal rewrote the receipt")

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
    expect(preview["candidates"] == [], f"20: a live checkout's root is a prune candidate: {preview['candidates']}")
    # Another checkout's reference keeps the root too: a second checkout links
    # the same image root but is not pruned while it is live.
    peer = make_checkout(proof, "group-peer")
    code, _, _ = oxide(proof, ["build"], "20 peer", peer)
    expect(code == 0, "20: peer build failed")
    expect(os.path.isdir(group_root), "20: the live root disappeared")
    expect(os.path.isdir(os.path.join(group, "target")), "20: prune removed the target")

    print("profile-outputs proof: all 20 cases passed", flush=True)


def main() -> int:
    if "--plan" in sys.argv[1:]:
        return plan_only()
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
    root = perm.tempfile.mkdtemp(prefix="profile-outputs.", dir=scratch)
    repo = os.path.dirname(HERE)
    perm.signal.signal(perm.signal.SIGTERM, layout.terminated)
    perm.signal.signal(perm.signal.SIGALRM, layout.overdue)
    perm.signal.setitimer(perm.signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    try:
        run(proof, repo)
    finally:
        perm.signal.setitimer(perm.signal.ITIMER_REAL, 0)
        try:
            proof.detach()
        except BaseException as error:
            print(f"cleanup after failure: {error!r}", file=sys.stderr)
    print("profile-outputs proof: 20 cases held", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
