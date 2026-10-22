#!/usr/bin/env python3
"""Final outputs on the permanent drive: the owned checkout target link
(oxide#117).

The qualified shared supervisor owns the 300 s deadline, 512 MiB monitored
scratch, process closure and cleanup. This script reuses the reviewed
oxide#104 staging/child helper and writes only inside the supervisor-owned
scratch. It stages this clean HEAD's `oxide` offline with 1.97.1, models the
permanent drive as one owned 128 MiB APFS sparse image attached at a unique
/Volumes/<name>, and proves:

- an effect-free JSON plan, and refusals that never move, adopt or replace a
  nonempty real target or a foreign symlink;
- explicit conversion of a missing/empty target into an owned link, builds,
  `run`, `test`, profiles and `target/debug` binaries through it;
- the checkout-local fallback root while the drive is absent, and redocking
  with both roots retained and nothing copied or deleted;
- a wrong recorded volume UUID selects the local root without writing to the
  external one, and no absent mount point is recreated;
- snapshot/barrier behaviour with an active build script in the checkout;
- crash replay after a prepared receipt, a live sibling link and a completed
  rename; and a contradictory state that stops and keeps both roots;
- `oxide clean` resolving the receipt to the real root and preserving the
  link, the marker and inactive roots;
- `oxide outputs detach` restoring a real directory and refusing while the
  drive mode is permanent.

Not a benchmark, not real-drive qualification and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import glob
import hashlib
import importlib.util
import json
import os
import plistlib
import re
import secrets
import shutil
import signal
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "pool_layout", os.path.join(HERE, "test-oxide-pool-layout.py")
)
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)
Proof = layout.Proof
expect, fail, summary, MIB = layout.expect, layout.fail, layout.summary, layout.MIB

BUILDER = "1.97.1"
SHUTDOWN_MARGIN_S = 30
MARKER = ".oxide-output.json"

APP = {
    "Cargo.toml": (
        '[package]\nname = "poutputs"\nversion = "0.1.0"\nedition = "2021"\n'
        'build = "build.rs"\n'
    ),
    "build.rs": r'''use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OXIDE_OUTPUT_GATE");
    let Ok(gate) = std::env::var("OXIDE_OUTPUT_GATE") else {
        return;
    };
    let gate = PathBuf::from(gate);
    fs::create_dir_all(&gate).unwrap();
    fs::write(gate.join("started"), b"started").unwrap();
    fs::write(
        gate.join(format!("started-{}", std::process::id())),
        b"started",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while !gate.join("release").exists() {
        assert!(Instant::now() < deadline, "proof did not release the build");
        std::thread::sleep(Duration::from_millis(25));
    }
}
''',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    "src/main.rs": (
        "fn main() {\n"
        '    println!("permanent-outputs-ok {}", poutputs::value());\n'
        "}\n"
    ),
    "tests/smoke.rs": (
        "#[test]\nfn smoke() {\n    assert_eq!(poutputs::value(), 7);\n}\n"
    ),
}

PLAIN_PACKAGE = '[package]\nname = "plain"\nversion = "0.1.0"\nedition = "2021"\n'


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the runner's shutdown margin")
    return left


def write_tree(root: str, files: dict[str, str]) -> str:
    for relative, contents in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(contents)
    return os.path.realpath(root)


def write_plain(root: str) -> str:
    os.makedirs(root, exist_ok=True)
    write_tree(root, {"Cargo.toml": PLAIN_PACKAGE, "src/lib.rs": "pub fn v() -> u32 { 1 }\n"})
    return os.path.realpath(root)


def oxide_env(proof: Proof, args: list, label: str, cwd: str, env: dict) -> tuple:
    """`Proof.oxide` under a command-specific environment."""
    result = proof.run([proof.binary, *args], cwd=cwd, env=env)
    stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
    print(f"----- {label}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
    return result.returncode, stdout, stderr


def read_bytes(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def slot_state(stderr: str) -> tuple[str, str]:
    """The leased slot id and whether it was warm, from the summary line."""
    line = summary(stderr)
    match = re.search(r"slot (\S+) (cold|warm)", line)
    expect(match is not None, f"no slot state in {line}")
    return match.group(1), match.group(2)


def started_markers(gate: str) -> list[str]:
    return sorted(glob.glob(os.path.join(gate, "started-*")))


def stop_gate(gate: str, release: str) -> None:
    with open(release, "wb") as handle:
        handle.write(b"release")


def read_json(path: str) -> dict:
    return json.loads(read_bytes(path).decode())


def handover_absent(root: str) -> bool:
    return not os.path.exists(os.path.join(root, "handover"))


def receipt_path(pool: str, checkout: str) -> str:
    """The receipt is keyed by the canonical checkout's SHA-256 digest."""
    key = hashlib.sha256(checkout.encode()).hexdigest()
    path = os.path.join(pool, "oxide-managed", "outputs", key + ".json")
    expect(os.path.isfile(path), f"no output receipt for {checkout} under {pool}")
    return path


def registration_path(pool: str) -> str:
    return os.path.join(pool, "oxide-managed", "removable.json")


def set_recorded_uuid(pool: str, uuid: str) -> None:
    path = registration_path(pool)
    current = read_json(path)
    current["volume_uuid"] = uuid
    with open(path, "wb") as handle:
        handle.write(json.dumps(current, indent=2).encode() + b"\n")
    os.chmod(path, 0o600)


def real_uuid(pool: str) -> str:
    return read_json(registration_path(pool))["volume_uuid"]


def link_target(path: str) -> str:
    return os.path.realpath(os.readlink(path))


def start_child(proof: Proof, command: list, cwd: str, env: dict) -> subprocess.Popen:
    child = subprocess.Popen(
        command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    proof.unclosed.add(child.pid)
    return child


def finish_child(proof: Proof, child: subprocess.Popen, label: str) -> tuple[int, str, str]:
    try:
        out, err = child.communicate(timeout=min(60, work_left()))
    except BaseException:
        child.kill()
        try:
            child.communicate(timeout=5)
            proof.unclosed.discard(child.pid)
        except BaseException as error:
            print(f"child {child.pid} not reaped: {error!r}", file=sys.stderr)
        raise
    proof.unclosed.discard(child.pid)
    stdout, stderr = out.decode(errors="replace"), err.decode(errors="replace")
    print(f"----- {label}: exit {child.returncode} -----\n{stdout}{stderr}", flush=True)
    return child.returncode, stdout, stderr


def reset_gate(gate: str) -> None:
    """Start each gated build from a clean gate, so a previous case's
    `started`/`release` file cannot make the next one run ungated."""
    shutil.rmtree(gate, ignore_errors=True)


def wait_for(path: str, label: str) -> None:
    deadline = time.monotonic() + min(60, work_left())
    while not os.path.exists(path):
        if time.monotonic() >= deadline:
            fail(f"{label}: {path} did not appear")
        time.sleep(0.05)


def plan(proof: Proof, cwd: str, label: str) -> dict:
    code, stdout, stderr = proof.oxide(["outputs", "plan"], label, cwd)
    expect(code == 0, f"{label}: plan exited {code}: {stderr}")
    report = json.loads(stdout)
    expect(report["version"] == "oxide.outputs.plan/1", f"{label}: {report}")
    expect(report["data_move"] is False, f"{label}: plan claims a data move: {report}")
    expect(len(stdout) < 64 * 1024, f"{label}: plan exceeds its byte bound")
    return report


def attach_image(proof: Proof, root: str, name: str) -> str:
    image = os.path.join(root, "volume.sparseimage")
    if not os.path.exists(image):
        created = proof.run(
            ["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name]
            + ["-type", "SPARSE", image],
            timeout=60,
        )
        expect(created.returncode == 0, f"image creation failed: {created.stderr!r}")
    attached = proof.run(["/usr/bin/hdiutil", "attach", "-nobrowse", "-plist", image], timeout=60)
    expect(
        attached.returncode == 0,
        f"attach failed; attachment unresolved, evidence kept: {attached.stderr!r}",
    )
    mounts = [
        entity["mount-point"]
        for entity in plistlib.loads(attached.stdout)["system-entities"]
        if entity.get("mount-point")
    ]
    expect(len(mounts) == 1, f"expected one mount, got {mounts}")
    proof.mount = mounts[0]
    return mounts[0]


def run(proof: Proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = (
        proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30)
        .stdout.decode()
        .strip()
    )
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxPo" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"permanent-outputs proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")

    candidate = layout.stage_at(proof, repo, "oxide-permanent-outputs")
    proof.binary = candidate
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    gate = os.path.join(root, "gate")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for key in list(env):
        if key in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS"):
            continue
        if key.startswith(
            (
                "SCCACHE_",
                "OXIDE_",
                "XDG_",
                "CARGO_BUILD_",
                "CARGO_TARGET_",
                "GIT_",
            )
        ) or key in (
            "CARGO",
            "RUSTC",
            "RUSTUP_TOOLCHAIN",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
        ):
            env.pop(key, None)
    scratch = os.environ["OXIDE_TASK_SCRATCH_ROOT"]
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": "64MiB",
            "HOME": home,
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": real_rustup,
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "OXIDE_TASK_SCRATCH_ROOT": scratch,
            "TMPDIR": scratch,
            "TMP": scratch,
            "TEMP": scratch,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "oxide proof",
            "GIT_AUTHOR_EMAIL": "proof@example.invalid",
            "GIT_COMMITTER_NAME": "oxide proof",
            "GIT_COMMITTER_EMAIL": "proof@example.invalid",
        }
    )
    proof.env = env

    app = write_tree(os.path.join(root, "app"), APP)
    keep = write_plain(os.path.join(root, "keep"))
    foreign = write_plain(os.path.join(root, "foreign"))
    untracked = write_plain(os.path.join(root, "untracked"))
    for fixture in (app, keep, foreign, untracked):
        proof.run(["git", "init", "-q"], cwd=fixture, timeout=30)
    expect(
        proof.run(["git", "check-ignore", "-q", "--", ".oxide-outputs"], cwd=app).returncode != 0,
        "the fixture already ignored .oxide-outputs before oxide init",
    )
    code, stdout, stderr = proof.oxide(["init"], "oxide init adds the ignore entry", app)
    expect(
        code == 0 and ("created" in stdout.lower() or "added" in stdout.lower()),
        stdout + stderr,
    )
    ignore_text = read_bytes(os.path.join(app, ".gitignore")).decode()
    expect(
        "/.oxide-outputs/" in ignore_text.splitlines(),
        f"oxide init did not add the local-output ignore entry: {ignore_text!r}",
    )
    code, stdout, stderr = proof.oxide(["init"], "oxide init is idempotent", app)
    expect(code == 0 and "already present" in stdout, stdout + stderr)
    expect(
        read_bytes(os.path.join(app, ".gitignore")).decode().count("/.oxide-outputs/") == 1,
        "oxide init duplicated the ignore entry",
    )
    proof.phase("after fixtures and adoption")

    # A first build creates the internal pool; the checkout stays ephemeral
    # and its real target directory is untouched by output policy.
    code, _, stderr = proof.oxide(["check"], "ephemeral first build", app)
    expect(code == 0, f"first build failed: {stderr}")
    expect(os.path.isdir(os.path.join(app, "target")), "ephemeral target missing")
    expect(
        not os.path.lexists(os.path.join(app, "target", MARKER)),
        "ephemeral build wrote an output marker",
    )
    expect(
        not os.path.exists(os.path.join(app, ".oxide-outputs")),
        "an ephemeral build created the local output root",
    )

    # Registration: permission for permanent output placement.
    code, stdout, stderr = proof.oxide(["outputs", "plan"], "plan before any drive", app)
    expect(code == 0, stderr)
    report = json.loads(stdout)
    expect(report["mode"] == "unregistered" and report["action"] == "refuse", report)
    code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt without permanent mode", app)
    expect(code != 0 and "permanent drive mode" in stderr, stderr)
    expect(
        not os.path.exists(os.path.join(app, ".oxide-outputs")),
        "a refused adopt created the local root",
    )

    mounted = attach_image(proof, root, name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        app,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    wrong = "00000000-0000-0000-0000-000000000000"
    good = real_uuid(pool)
    proof.phase("after drive registration")

    # 1. A nonempty real target refuses, with no mutation.
    keep_target = os.path.join(keep, "target")
    os.makedirs(keep_target, exist_ok=True)
    with open(os.path.join(keep_target, "keep.txt"), "wb") as handle:
        handle.write(b"warm build")
    report = plan(proof, keep, "plan with a nonempty target")
    expect(report["current_target"] == "real-nonempty" and report["action"] == "refuse", report)
    expect("never moves" in report["reason"], report)
    code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt a nonempty target", keep)
    expect(code != 0 and "never moves" in stderr, stderr)
    expect(read_bytes(os.path.join(keep_target, "keep.txt")) == b"warm build", "target mutated")
    expect(os.path.isdir(keep_target), "the real target directory was replaced")
    expect(not os.path.exists(os.path.join(keep, ".oxide-outputs")), "refused adopt wrote a root")

    # 2. A foreign symlink refuses, with no mutation.
    outside = os.path.join(root, "outside")
    os.makedirs(outside)
    with open(os.path.join(outside, "sentinel"), "wb") as handle:
        handle.write(b"foreign")
    os.symlink(outside, os.path.join(foreign, "target"))
    report = plan(proof, foreign, "plan with a foreign symlink")
    expect(report["current_target"] == "foreign" and report["action"] == "refuse", report)
    code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt a foreign symlink", foreign)
    expect(code != 0 and "never moves" in stderr, stderr)
    expect(os.readlink(os.path.join(foreign, "target")) == outside, "foreign link was replaced")
    expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "foreign target mutated")

    # 3. Explicit conversion of an empty real target.
    shutil.rmtree(os.path.join(app, "target"))
    os.makedirs(os.path.join(app, "target"))
    report = plan(proof, app, "plan with an empty real target")
    expect(report["action"] == "create-owned-link", report)
    expect(
        report["proposed_root"]["kind"] == "external"
        and report["proposed_root"]["root"].startswith(mount + "/oxide-outputs/"),
        report,
    )
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "adopt an empty target", app)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    expect(
        "no existing output was moved" in stdout,
        f"the conversion did not state the no-move rule: {stdout}",
    )
    link = os.path.join(app, "target")
    expect(os.path.islink(link), "target is not a symlink after adopt")
    external_root = link_target(link)
    expect(external_root.startswith(mount + "/oxide-outputs/"), external_root)
    expect(os.path.isfile(os.path.join(external_root, MARKER)), "the root has no marker")
    marker = read_json(os.path.join(external_root, MARKER))
    expect(marker["version"] == "oxide.outputs.root/1", marker)
    expect(marker["checkout"] == app and marker["kind"] == "external", marker)
    receipt = read_json(receipt_path(pool, app))
    expect(
        receipt["version"] == "oxide.outputs.transition/1"
        and receipt["state"] == "committed"
        and receipt["checkout"] == app,
        receipt,
    )
    expect(not os.path.exists(os.path.join(app, ".oxide-outputs")), "adopt created a local root")
    proof.phase("after first conversion")

    # 4. Builds, run, test and profiles through the owned link.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build through the link", app)
    expect(code == 0, f"build through the link failed: {stderr}")
    binary = os.path.join(app, "target", "debug", "poutputs")
    expect(os.path.isfile(binary), f"no target/debug binary at {binary}")
    expect(os.path.realpath(binary).startswith(external_root + "/"), "binary is not in the root")
    code, stdout, _ = proof.oxide(["run", "--bin", "poutputs"], "run through the link", app)
    expect(code == 0 and "permanent-outputs-ok 7" in stdout, stdout)
    code, _, stderr = proof.oxide(["test"], "test through the link", app)
    expect(code == 0, f"test through the link failed: {stderr}")
    code, _, stderr = proof.oxide(["check", "--profile", "debug"], "debug profile through the link", app)
    expect(code == 0, f"debug profile failed: {stderr}")
    expect(
        os.path.isdir(os.path.join(app, "target", "debug")),
        "the debug profile did not write target/debug",
    )
    code, stdout, _ = proof.oxide(["outputs", "status"], "status of an owned checkout", app)
    expect("outputs: mode permanent" in stdout and "generation" in stdout, stdout)
    expect(
        "local fallback root" in stdout and "not selected" in stdout,
        f"status did not name the unused local root: {stdout}",
    )

    # 5. Detaching the drive: the checkout-local fallback root.
    external_before = sorted(os.listdir(external_root))
    with open(os.path.join(external_root, "external-sentinel"), "wb") as handle:
        handle.write(b"external kept")
    proof.detach()
    expect(not os.path.lexists(mount), "the detached mount point still exists")
    code, stdout, stderr = proof.oxide(["build", "--bin", "poutputs"], "build with the drive absent", app)
    expect(code == 0, f"fallback build failed: {stderr}")
    expect("WARNING: permanent drive mode" in stderr, stderr)
    expect("checkout-local" in stderr, stderr)
    local_root = os.path.join(app, ".oxide-outputs", "local")
    expect(link_target(link) == local_root, f"the link did not fall back: {os.readlink(link)}")
    expect(os.path.isfile(os.path.join(local_root, MARKER)), "the fallback root has no marker")
    fallback_marker = read_json(os.path.join(local_root, MARKER))
    expect(fallback_marker["kind"] == "local" and "volume_uuid" not in fallback_marker, fallback_marker)
    expect(
        os.path.realpath(binary).startswith(local_root + "/"),
        "the fallback binary is not in the local root",
    )
    # The first fallback leased the generation the local root was then created
    # at, so an identical command reuses the same slot warm. A lease keyed from
    # a marker-less g0 would take a second cold slot here.
    fallback_slot, fallback_warmth = slot_state(stderr)
    expect(fallback_warmth == "cold", f"the first fallback was not cold: {fallback_slot} {fallback_warmth}")
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "second fallback build", app)
    expect(code == 0, f"second fallback build failed: {stderr}")
    expect(
        slot_state(stderr) == (fallback_slot, "warm"),
        f"the fallback destination did not keep its slot: {stderr}",
    )
    expect(
        os.path.realpath(binary).startswith(local_root + "/"),
        "the reused fallback binary left the local root",
    )
    with open(os.path.join(local_root, "local-sentinel"), "wb") as handle:
        handle.write(b"local kept")
    proof.phase("after fallback")

    # 6. Redocking relinks without copying or deleting either root.
    mounted = attach_image(proof, root, name)
    expect(mounted == mount, f"re-attached at {mounted}, not {mount}")
    before_redock = read_json(receipt_path(pool, app))
    report = plan(proof, app, "plan after redocking")
    expect(report["action"] in ("relink", "noop"), report)
    expect(
        report["proposed_root"]["generation"]
        == read_json(os.path.join(external_root, MARKER))["generation"],
        report,
    )
    print(
        "redock before: receipt "
        f"gen {before_redock['generation']} {before_redock['state']}, "
        f"external marker {read_json(os.path.join(external_root, MARKER))['generation']}, "
        f"local marker {read_json(os.path.join(local_root, MARKER))['generation']}, "
        f"plan {report['proposed_root']}"
    )
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build after redocking", app)
    expect(code == 0, f"redocked build failed: {stderr}")
    expect(link_target(link) == external_root, f"redock did not relink: {os.readlink(link)}")
    expect(read_bytes(os.path.join(external_root, "external-sentinel")) == b"external kept", "root lost")
    expect(
        sorted(os.listdir(external_root)) == sorted(external_before + ["external-sentinel"]),
        "the absent drive's root changed",
    )
    expect(read_bytes(os.path.join(local_root, "local-sentinel")) == b"local kept", "fallback lost")
    expect(
        os.path.realpath(binary).startswith(external_root + "/"),
        "the redocked binary did not return to the external root",
    )
    code, stdout, _ = proof.oxide(["outputs", "status"], "status with both roots", app)
    expect("preserved local fallback root" in stdout, stdout)
    proof.phase("after redock")

    # 6b. The local fallback root refuses to appear where Git would track it.
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "adopt the untracked fixture", untracked)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    untracked_link = os.path.join(untracked, "target")
    untracked_root = link_target(untracked_link)
    set_recorded_uuid(pool, "00000000-0000-0000-0000-000000000000")
    code, _, stderr = proof.oxide(["check"], "local root where Git would track it", untracked)
    expect(code != 0, "a Git-tracked local fallback root was created")
    expect("Git would track it" in stderr, stderr)
    expect(
        not os.path.exists(os.path.join(untracked, ".oxide-outputs")),
        "the refused fallback created a local root",
    )
    expect(link_target(untracked_link) == untracked_root, "the refused fallback changed the link")
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "roll the refused fallback back", untracked)
    expect(code == 0 and link_target(untracked_link) == untracked_root, stderr)
    # A local root that already exists (empty) is still refused and left
    # untouched when Git would track it.
    empty_local = os.path.join(untracked, ".oxide-outputs", "local")
    os.makedirs(empty_local)
    set_recorded_uuid(pool, "00000000-0000-0000-0000-000000000000")
    code, _, stderr = proof.oxide(["check"], "pre-created empty local root", untracked)
    expect(code != 0 and "Git would track it" in stderr, stderr)
    expect(os.path.isdir(empty_local) and os.listdir(empty_local) == [], "the empty local root changed")
    expect(link_target(untracked_link) == untracked_root, "the refused fallback changed the link")
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "roll the empty-local-root refusal back", untracked)
    expect(code == 0 and link_target(untracked_link) == untracked_root, stderr)
    proof.phase("after the tracked-fallback refusal")

    # 7. A wrong recorded volume UUID selects the local root and never writes
    #    to the external one, and no absent mount point is recreated.
    marker_before = read_bytes(os.path.join(external_root, MARKER))
    entries_before = sorted(os.listdir(external_root))
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(["check"], "build with a wrong recorded UUID", app)
    expect(code == 0 and "WARNING: permanent drive mode" in stderr, stderr)
    expect(link_target(link) == local_root, "a wrong UUID did not fall back to the local root")
    expect(read_bytes(os.path.join(external_root, MARKER)) == marker_before, "the wrong UUID wrote a marker")
    expect(sorted(os.listdir(external_root)) == entries_before, "the wrong UUID wrote into the root")
    set_recorded_uuid(pool, good)
    proof.phase("after wrong UUID")

    # 8a. A second command in the same checkout waits for the running build,
    #     then binds its own destination: the first build keeps the root it
    #     selected and no swap reaches it.
    gate_env = dict(env)
    gate_env["OXIDE_OUTPUT_GATE"] = gate
    expect(link_target(link) == local_root, f"the pre-writer link is {os.readlink(link)}")
    reset_gate(gate)
    first = start_child(proof, [proof.binary, "check"], app, gate_env)
    try:
        wait_for(os.path.join(gate, "started"), "first gated build script")
        expect(link_target(link) == external_root, "the first command did not bind external outputs")
        set_recorded_uuid(pool, wrong)
        second = start_child(proof, [proof.binary, "check"], app, env)
        time.sleep(2.0)
        if second.poll() is not None:
            early_out, early_err = second.communicate(timeout=5)
            proof.unclosed.discard(second.pid)
            fail(
                "a second command did not wait for the running build in its checkout: "
                f"exit {second.returncode}\n{early_out.decode(errors='replace')}"
                f"{early_err.decode(errors='replace')}"
            )
        expect(link_target(link) == external_root, "a swap reached the running command")
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        first_code, _, first_stderr = finish_child(proof, first, "first gated build")
        expect(first_code == 0, f"the first gated build failed: {first_stderr}")
        expect(
            os.path.isdir(os.path.join(external_root, "debug")),
            "the first build did not write to the root it selected",
        )
        second_code, second_stdout, second_stderr = finish_child(
            proof, second, "second command after the writer closed"
        )
        expect(
            second_code == 0,
            f"the second command failed: {second_stdout}{second_stderr}",
        )
        expect(
            link_target(link) == local_root,
            "the second command did not transition to its own destination",
        )
    finally:
        os.makedirs(gate, exist_ok=True)
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        if first.poll() is None:
            first.kill()
    set_recorded_uuid(pool, good)
    proof.phase("after the second-command transition")

    # 8b. An active writer blocks the output barrier until it closes.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "relink to external before the adopt", app)
    expect(code == 0 and link_target(link) == external_root, f"pre-adopt relink failed: {stderr}")
    gate_env = dict(env)
    gate_env["OXIDE_OUTPUT_GATE"] = gate
    reset_gate(gate)
    writer = start_child(proof, [proof.binary, "check"], app, gate_env)
    try:
        wait_for(os.path.join(gate, "started"), "gated build script")
        expect(writer.poll() is None, "the gated build exited early")
        set_recorded_uuid(pool, wrong)
        waiting = start_child(
            proof, [proof.binary, "outputs", "adopt"], app, env
        )
        time.sleep(2.0)
        if waiting.poll() is not None:
            early_out, early_err = waiting.communicate(timeout=5)
            proof.unclosed.discard(waiting.pid)
            fail(
                "outputs adopt did not wait for the active writer in the checkout: "
                f"exit {waiting.returncode}\n{early_out.decode(errors='replace')}"
                f"{early_err.decode(errors='replace')}"
            )
        expect(link_target(link) == external_root, "a swap redirected the running build")
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        writer_code, _, writer_stderr = finish_child(proof, writer, "gated build")
        expect(writer_code == 0, f"the gated build failed: {writer_stderr}")
        adopt_code, adopt_stdout, adopt_stderr = finish_child(proof, waiting, "adopt after the writer")
        expect(adopt_code == 0, f"adopt after the writer failed: {adopt_stdout}{adopt_stderr}")
        expect(link_target(link) == local_root, "adopt did not relink after the writer closed")
        expect(
            os.path.isdir(os.path.join(external_root, "debug")),
            "the running build did not write to the external root",
        )
    finally:
        os.makedirs(gate, exist_ok=True)
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        if writer.poll() is None:
            writer.kill()
    set_recorded_uuid(pool, good)
    proof.phase("after the writer barrier")

    # 8c. A long-lived exec in the checkout does not block a second command
    #     whose destination is unchanged (the second-slot rule), and the exec
    #     is not redirected.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "relink before the exec case", app)
    expect(code == 0 and link_target(link) == external_root, f"pre-exec relink failed: {stderr}")
    exec_release = os.path.join(root, "exec-release")
    exec_child = start_child(
        proof,
        [
            proof.binary,
            "exec",
            "--",
            "/bin/sh",
            "-c",
            f"while [ ! -f {exec_release} ]; do sleep 0.05; done",
        ],
        app,
        env,
    )
    try:
        time.sleep(1.0)
        expect(exec_child.poll() is None, "the long-lived exec exited early")
        code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "command beside a long-lived exec", app)
        expect(
            code == 0 and link_target(link) == external_root,
            f"a command was blocked by the checkout's exec: {stderr}",
        )
        expect(exec_child.poll() is None, "the exec ended before the second command")
    finally:
        with open(exec_release, "wb") as handle:
            handle.write(b"go")
    exec_code, _, exec_stderr = finish_child(proof, exec_child, "long-lived exec")
    expect(exec_code == 0, f"the long-lived exec failed: {exec_stderr}")

    # 8d. Two commands started together with the same destination both reach
    #     Cargo: no post-lease deadlock between starters.
    reset_gate(gate)
    starter_one = start_child(proof, [proof.binary, "check"], app, gate_env)
    starter_two = start_child(proof, [proof.binary, "check"], app, gate_env)
    try:
        deadline = time.monotonic() + min(60, work_left())
        while len(started_markers(gate)) < 2:
            if time.monotonic() >= deadline:
                fail(
                    "only "
                    f"{len(started_markers(gate))} of two simultaneous commands reached Cargo"
                )
            time.sleep(0.05)
        expect(
            starter_one.poll() is None and starter_two.poll() is None,
            "a simultaneous command finished before the gate",
        )
        stop_gate(gate, os.path.join(gate, "release"))
        first_code, _, first_stderr = finish_child(proof, starter_one, "simultaneous command one")
        second_code, _, second_stderr = finish_child(proof, starter_two, "simultaneous command two")
        expect(
            first_code == 0 and second_code == 0,
            f"a simultaneous command failed: {first_stderr}{second_stderr}",
        )
    finally:
        os.makedirs(gate, exist_ok=True)
        stop_gate(gate, os.path.join(gate, "release"))
        for child in (starter_one, starter_two):
            if child.poll() is None:
                child.kill()
    expect(
        os.path.isdir(os.path.join(external_root, "debug")),
        "the simultaneous commands did not use the external root",
    )

    # 8e. A replaced output-directory ancestor refuses an unchanged
    #     destination, so Cargo never writes onto internal disk.
    volume_outputs = os.path.join(mount, "oxide-outputs")
    saved_outputs = os.path.join(mount, "saved-oxide-outputs")
    internal_outputs = os.path.join(root, "internal-outputs")
    os.rename(volume_outputs, saved_outputs)
    shutil.copytree(saved_outputs, internal_outputs)
    os.symlink(internal_outputs, volume_outputs)
    leaf = os.path.basename(external_root)
    before = sorted(os.listdir(os.path.join(internal_outputs, leaf)))
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "replaced ancestor refuses", app)
    expect(code != 0, "a replaced output ancestor was written through")
    expect(
        "not on the registered volume" in stderr
        or "does not resolve to itself" in stderr
        or "is a mount point" in stderr,
        stderr,
    )
    expect(
        sorted(os.listdir(os.path.join(internal_outputs, leaf))) == before,
        "the refused build wrote into the internal copy",
    )
    os.remove(volume_outputs)
    os.rename(saved_outputs, volume_outputs)
    shutil.rmtree(internal_outputs)
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build after restoring the ancestor", app)
    expect(code == 0 and link_target(link) == external_root, f"ancestor restore failed: {stderr}")
    proof.phase("after the barrier and ancestor controls")

    # 9. Crash replay at each write-ahead point.
    expect(receipt_path(pool, app) and read_json(receipt_path(pool, app))["state"] == "committed", read_json(receipt_path(pool, app)))
    # Start from the checkout-local root so the first injection has a
    # transition to stop in.
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(["check"], "local root before the first crash", app)
    expect(code == 0 and link_target(link) == local_root, stderr)
    set_recorded_uuid(pool, good)
    crash_env = dict(env)
    crash_env["OXIDE_TEST_OUTPUTS_CRASH"] = "prepared"
    code, _, stderr = oxide_env(
        proof, ["outputs", "adopt"], "injected stop after the prepared receipt", app, crash_env
    )
    expect(code != 0 and "injected transition stop" in stderr, stderr)
    prepared = read_json(receipt_path(pool, app))
    expect(prepared["state"] == "prepared" and prepared["prepared"], prepared)
    expect(link_target(link) == local_root, "the prepared receipt changed the live link")
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "replay a prepared receipt", app)
    expect(code == 0 and link_target(link) == external_root, f"replay failed: {stdout}{stderr}")
    replay = read_json(receipt_path(pool, app))
    expect(replay["state"] == "committed" and replay["generation"] == prepared["prepared"]["generation"], replay)

    for point in ("linked", "renamed"):
        set_recorded_uuid(pool, wrong)
        code, _, stderr = proof.oxide(["check"], f"local root before the {point} crash", app)
        expect(code == 0 and link_target(link) == local_root, stderr)
        set_recorded_uuid(pool, good)
        crash_env["OXIDE_TEST_OUTPUTS_CRASH"] = point
        code, _, stderr = oxide_env(
            proof, ["outputs", "adopt"], f"injected stop after {point}", app, crash_env
        )
        expect(code != 0 and "injected transition stop" in stderr, stderr)
        prepared = read_json(receipt_path(pool, app))
        expect(prepared["state"] == "prepared", prepared)
        if point == "linked":
            expect(link_target(link) == local_root, "the linked crash moved the live link")
            sibling = os.path.join(app, ".target.oxide-new-%d" % prepared["prepared"]["generation"])
            expect(os.path.islink(sibling), f"no prepared sibling at {sibling}")
            # A registration that no longer names the prepared root's volume
            # stops the replay before its rename: link, sibling and receipt kept.
            prepared_bytes = read_bytes(receipt_path(pool, app))
            set_recorded_uuid(pool, wrong)
            code, _, stderr = proof.oxide(["outputs", "adopt"], "replay with a wrong UUID refuses", app)
            expect(code != 0 and "cannot be replayed" in stderr, stderr)
            expect(link_target(link) == local_root, "the refused replay moved the live link")
            expect(os.path.islink(sibling), "the refused replay removed the sibling")
            expect(read_bytes(receipt_path(pool, app)) == prepared_bytes, "the refused replay rewrote the receipt")
            set_recorded_uuid(pool, good)
        else:
            expect(link_target(link) == external_root, "the renamed crash did not publish the link")
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], f"replay after {point}", app)
        expect(code == 0 and link_target(link) == external_root, f"{point} replay failed: {stdout}{stderr}")
        expect(
            read_json(receipt_path(pool, app))["state"] == "committed",
            f"{point} replay did not commit",
        )
        expect(
            not glob.glob(os.path.join(app, ".target.oxide-new-*")),
            "a prepared sibling was left behind",
        )
    code, stdout, _ = proof.oxide(["outputs", "adopt"], "idempotent adopt", app)
    expect(code == 0 and "already selects" in stdout, stdout)
    proof.phase("after crash replay")

    # 9b. A same-device symlink in place of `.oxide-outputs` is not the real
    #     checkout-local root: build and clean refuse and the tree it points
    #     at is unchanged.
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(["check"], "local root before the ancestor swap", app)
    expect(code == 0 and link_target(link) == local_root, stderr)
    local_outer = os.path.join(app, ".oxide-outputs")
    moved_outer = os.path.join(root, "moved-local-outputs")
    os.rename(local_outer, moved_outer)
    os.symlink(moved_outer, local_outer)
    moved_local = os.path.join(moved_outer, "local")
    before = sorted(os.listdir(moved_local))
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "symlinked local ancestor refuses", app)
    expect(code != 0 and "not a real directory" in stderr, stderr)
    code, _, stderr = proof.oxide(["clean"], "clean through a symlinked local ancestor refuses", app)
    expect(code != 0 and "clean refused" in stderr, stderr)
    expect(sorted(os.listdir(moved_local)) == before, "the refused commands changed the moved local root")
    os.remove(local_outer)
    os.rename(moved_outer, local_outer)
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "redock after the ancestor control", app)
    expect(code == 0 and link_target(link) == external_root, stderr)
    proof.phase("after the local ancestor control")

    # 10. A foreign live link refuses and is left untouched.
    os.remove(os.path.join(app, "target"))
    os.symlink(os.path.join(root, "outside"), os.path.join(app, "target"))
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "foreign live link refuses", app)
    expect(code != 0 and "not this checkout's owned link" in stderr, stderr)
    expect(os.path.islink(os.path.join(app, "target")), "the foreign link was replaced")
    expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "the foreign target mutated")
    os.remove(os.path.join(app, "target"))
    os.symlink(external_root, os.path.join(app, "target"))

    # 11. Clean resolves the receipt to the real root and preserves it.
    committed_bytes = read_bytes(receipt_path(pool, app))
    # 11a. A selected clean revalidates ownership after its barrier: a link
    #      that turns foreign while it waits is never cleaned through.
    clean_release = os.path.join(root, "clean-release")
    holder = start_child(
        proof,
        [
            proof.binary,
            "exec",
            "--",
            "/bin/sh",
            "-c",
            f"while [ ! -f {clean_release} ]; do sleep 0.05; done",
        ],
        app,
        env,
    )
    try:
        time.sleep(1.0)
        expect(holder.poll() is None, "the clean-wait holder exited early")
        code, status_out, status_err = proof.oxide(["status"], "status during the clean holder", app)
        expect(code == 0, status_err)
        expect(
            any("held by exec /bin/sh" in line for line in status_out.splitlines()),
            f"the exec holder is not recorded as active: {status_out}",
        )
        waiting = start_child(proof, [proof.binary, "clean", "-p", "poutputs"], app, env)
        time.sleep(1.5)
        if waiting.poll() is not None:
            early_out, early_err = waiting.communicate(timeout=5)
            proof.unclosed.discard(waiting.pid)
            fail(
                "selected clean did not wait for the checkout's active command: "
                f"exit {waiting.returncode}\n{early_out.decode(errors='replace')}"
                f"{early_err.decode(errors='replace')}"
            )
        os.remove(link)
        os.symlink(os.path.join(root, "outside"), link)
        with open(clean_release, "wb") as handle:
            handle.write(b"go")
        holder_code, _, holder_stderr = finish_child(proof, holder, "clean-wait holder")
        expect(holder_code == 0, f"the clean-wait holder failed: {holder_stderr}")
        clean_code, clean_out, clean_err = finish_child(
            proof, waiting, "selected clean after the link changed"
        )
        expect(
            clean_code != 0 and "not the owned link" in clean_err,
            f"selected clean ran after its ownership changed: {clean_out}{clean_err}",
        )
        expect(
            read_bytes(os.path.join(outside, "sentinel")) == b"foreign",
            "the refused selected clean removed bytes through the foreign link",
        )
    finally:
        with open(clean_release, "wb") as handle:
            handle.write(b"go")
        if holder.poll() is None:
            holder.kill()
    os.remove(link)
    os.symlink(external_root, link)
    code, _, stderr = proof.oxide(
        ["clean", "-p", "poutputs"], "selected clean through the owned link", app
    )
    expect(code == 0, f"selected clean failed: {stderr}")
    expect(os.path.islink(link) and link_target(link) == external_root, "selected clean lost the link")
    expect(os.path.isfile(os.path.join(external_root, MARKER)), "selected clean lost the marker")
    # A selected clean never runs against a replaced or unusable volume.
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(
        ["clean", "-p", "poutputs"], "selected clean with an unusable volume", app
    )
    expect(code != 0 and "registered volume" in stderr, stderr)
    code, stdout, stderr = proof.oxide(
        ["clean", "--dry-run"], "whole clean with an unusable volume", app
    )
    expect(code != 0 and "registered volume" in stderr, stderr)
    expect(stdout.strip() == "" and handover_absent(external_root), "a refused clean printed or removed paths")
    set_recorded_uuid(pool, good)
    # A foreign live link is never cleaned through, selected or whole.
    os.remove(link)
    os.symlink(os.path.join(root, "outside"), link)
    code, _, stderr = proof.oxide(
        ["clean", "-p", "poutputs"], "selected clean through a foreign link", app
    )
    expect(code != 0 and "not the owned link" in stderr, stderr)
    expect(os.readlink(link) == os.path.join(root, "outside"), "a refused selected clean replaced the link")
    expect(os.path.isfile(os.path.join(outside, "sentinel")), "a refused selected clean removed bytes")
    os.remove(link)
    os.symlink(external_root, link)
    os.remove(os.path.join(external_root, "external-sentinel"))
    with open(os.path.join(external_root, "handover"), "wb") as handle:
        handle.write(b"foreign")
    code, _, stderr = proof.oxide(["clean"], "clean refuses a foreign root entry", app)
    expect(code != 0 and "handover" in stderr, stderr)
    expect(os.path.isfile(os.path.join(external_root, "handover")), "the refused clean removed bytes")
    os.remove(os.path.join(external_root, "handover"))
    code, stdout, stderr = proof.oxide(["clean", "--dry-run"], "clean dry run through the receipt", app)
    expect(code == 0, stderr)
    listed = stdout.splitlines()
    expect(
        all(path != external_root and not path.endswith(MARKER) for path in listed),
        f"the dry run would remove the root or its marker: {listed[:5]}",
    )
    expect(any(path.startswith(external_root + "/") for path in listed), listed[:5])
    expect("are kept" in stderr and "owned link" in stderr, stderr)
    code, _, stderr = proof.oxide(["clean"], "clean through the receipt", app)
    expect(code == 0, f"clean failed: {stderr}")
    expect(os.path.islink(link) and link_target(link) == external_root, "clean removed the owned link")
    expect(os.path.isfile(os.path.join(external_root, MARKER)), "clean removed the root marker")
    expect(not os.path.exists(os.path.join(external_root, "debug")), "clean left Cargo output")
    expect(read_bytes(os.path.join(local_root, "local-sentinel")) == b"local kept", "clean deleted a root")
    expect("are kept" in stderr, stderr)
    code, stdout, _ = proof.oxide(["outputs", "status"], "status after clean", app)
    expect("preserved local fallback root" in stdout, stdout)
    proof.phase("after clean")

    # 12. Detach: refuse while permanent, then restore a real directory.
    code, _, stderr = proof.oxide(["outputs", "detach"], "detach while permanent", app)
    expect(code != 0 and "still permanent" in stderr, stderr)
    expect(os.path.islink(link), "a refused detach removed the link")
    code, _, stderr = proof.oxide(["pool", "set-mode", "ephemeral"], "set ephemeral mode", app)
    expect(code == 0, stderr)
    code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach in ephemeral mode", app)
    expect(code == 0 and "real empty directory again" in stdout, stdout + stderr)
    expect(os.path.isdir(link) and not os.path.islink(link), "detach did not restore a real directory")
    expect(os.listdir(link) == [], "the detached target is not empty")
    expect(os.path.isdir(external_root) and os.path.isdir(local_root), "detach deleted a root")
    expect(read_bytes(os.path.join(local_root, "local-sentinel")) == b"local kept", "detach lost a root")
    code, stdout, _ = proof.oxide(["outputs", "status"], "status after detach", app)
    expect("detached" in stdout, stdout)

    # 13. A detached checkout builds ephemerally; the drive stays untouched.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build after detach", app)
    expect(code == 0, f"detached build failed: {stderr}")
    expect(os.path.isdir(os.path.join(app, "target", "debug")), "detached build wrote no target/debug")
    expect(not os.path.islink(link), "a detached build re-created the owned link")
    # 14. Unknown writer custody (a lost slot never positively closed) blocks
    #     relinking and cleaning for this checkout, in either pool.
    state_paths = [
        os.path.join(pool, "oxide-managed", "state.json"),
        os.path.join(pool, "oxide-managed", "removable", "oxide-managed", "state.json"),
    ]
    saved_states = {}
    crafted = False
    for path in state_paths:
        if not os.path.exists(path) or crafted:
            continue
        saved_states[path] = read_bytes(path)
        data = json.loads(saved_states[path])
        record = next(
            (entry for entry in data.get("records", []) if entry.get("checkout") == app), None
        )
        if record is None:
            continue
        record["phase"] = "retired"
        record["retired"] = "quarantined: crafted proof writer"
        record["lost"] = {"slot_id": "77", "at_ms": 1, "unclosed": "crafted unknown writer"}
        data["next_id"] = max(int(data.get("next_id", 1)), 1000)
        with open(path, "wb") as handle:
            handle.write(json.dumps(data).encode())
        os.chmod(path, 0o600)
        crafted = True
    expect(crafted, "no checkout record to quarantine in the proof state")
    try:
        code, _, stderr = proof.oxide(
            ["pool", "set-mode", "permanent"], "permanent mode for the custody case", app
        )
        expect(code == 0, stderr)
        code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt refused on quarantined custody", app)
        expect(code != 0 and "never confirmed closed" in stderr, stderr)
        code, _, stderr = proof.oxide(["clean"], "clean refused on quarantined custody", app)
        expect(code != 0 and "unknown writer custody" in stderr, stderr)
        expect(os.path.isdir(local_root) and os.path.isdir(external_root), "a refused clean removed a root")
    finally:
        for path, contents in saved_states.items():
            with open(path, "wb") as handle:
                handle.write(contents)
            os.chmod(path, 0o600)
    proof.phase("after the quarantined-custody refusal")

    # 15. A prepared first conversion never renames over a link that appeared
    #     after the crash, even when a matching sibling is live.
    code, _, stderr = proof.oxide(
        ["pool", "set-mode", "permanent"], "permanent mode for the crafted replay", app
    )
    expect(code == 0, stderr)
    crafted = json.loads(committed_bytes.decode())
    crafted["state"] = "prepared"
    generation = crafted["generation"] + 1
    crafted["prepared"] = {
        "generation": generation,
        "to": {
            "kind": "local",
            "root": local_root,
            "generation": generation,
        },
        "link": link,
    }
    with open(receipt_path(pool, app), "wb") as handle:
        handle.write(json.dumps(crafted).encode())
    os.chmod(receipt_path(pool, app), 0o600)
    sibling = os.path.join(app, f".target.oxide-new-{generation}")
    os.symlink(local_root, sibling)
    # After the detached build the target is a real directory again.
    shutil.rmtree(link)
    os.symlink(os.path.join(app, "absent-foreign"), link)
    code, stdout, stderr = proof.oxide(
        ["outputs", "adopt"], "replay never renames over a foreign dangling link", app
    )
    expect(code != 0 and "cannot be replayed" in stderr, stdout + stderr)
    expect(
        os.readlink(link) == os.path.join(app, "absent-foreign"),
        "replay replaced a link that appeared after the crash",
    )
    expect(os.path.islink(sibling), "replay removed the prepared sibling")
    expect(os.path.isdir(local_root), "replay removed an output root")
    proof.phase("after the crafted replay refusal")
    proof.phase("before the ordinary detach")
    proof.detach()
    proof.phase("end")
    expect(not os.path.lexists(mount), "the scratch mount point is still present")


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")


def overdue(signum, frame) -> None:
    raise RuntimeError("work deadline reached (SIGALRM fallback)")


def record(name: str, contents: str) -> None:
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        handle.write(contents)


def main() -> int:
    if sys.platform != "darwin":
        fail("the permanent-outputs proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
        and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified shared proof supervisor",
    )
    root = tempfile.mkdtemp(prefix="permanent-outputs.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    try:
        run(proof, repo)
    except BaseException as failure:
        record(
            "result.txt",
            f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\nroot: {root}\n",
        )
        print(
            f"permanent-outputs proof failed; scratch {root} left to the shared supervisor",
            file=sys.stderr,
        )
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    elapsed = time.monotonic() - layout.runner.STARTED
    summary_line = (
        f"permanent-outputs proof held: {elapsed:.0f} s script time, peak owned "
        f"{proof.peak // MIB} MiB at phase checks, phase-sampled host free minimum "
        f"{proof.low // MIB} MiB; one owned 128 MiB scratch image, ordinary detach, "
        f"shared supervisor owns cleanup"
    )
    record("result.txt", f"passed\n{summary_line}\n")
    print(summary_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
