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


def read_json(path: str) -> dict:
    return json.loads(read_bytes(path).decode())


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
    with open(os.path.join(local_root, "local-sentinel"), "wb") as handle:
        handle.write(b"local kept")
    proof.phase("after fallback")

    # 6. Redocking relinks without copying or deleting either root.
    mounted = attach_image(proof, root, name)
    expect(mounted == mount, f"re-attached at {mounted}, not {mount}")
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

    # 8. An active writer blocks the swap until it closes; the running build
    #    keeps the root it selected.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "relink to external before the writer", app)
    expect(code == 0 and link_target(link) == external_root, f"pre-writer relink failed: {stderr}")
    gate_env = dict(env)
    gate_env["OXIDE_OUTPUT_GATE"] = gate
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

    # 9. Crash replay at each write-ahead point.
    expect(receipt_path(pool, app) and read_json(receipt_path(pool, app))["state"] == "committed", read_json(receipt_path(pool, app)))
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
