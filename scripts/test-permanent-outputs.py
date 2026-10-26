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
- `oxide outputs detach` restoring a real directory in ephemeral mode.

The `detach-permanent` mode (oxide#124) proves the per-checkout opt-out under
permanent mode: detach keeps the roots and records a detached receipt, the next
command builds into the real target without relinking or a notice, automatic
adoption stays off for it, and an explicit later adopt re-converts an empty
target through the existing preconditions.

The focused modes reuse this fixture and staging helpers:

- `auto-adopt`: automatic first conversion (oxide#121).
- `ignore-safety`: the first conversion's Git ignore prerequisite
  (oxide#125): a target link is published only when Git ignores its exact
  non-directory repository-relative path, proven for explicit adopt, automatic
  adoption and prepared first-conversion replay, with trailing-slash,
  negated, unignored, tracked, missing-Git, timeout and malformed-answer
  refusals that change nothing.
- `exec-inference`: `oxide exec` output-member inference (oxide#160, S1) and
  the profile-layout automatic first conversion (S4b): a derived `exec` set
  that places a tool's build in the checkout's committed members, a warm
  repeat, the declared `--output` parity, an unrecorded member left local,
  a detached and an unconverted checkout running locally, a spaced checkout
  path, an existing whole-target checkout that stays whole-target, a foreign
  member link, root drift, a receipt changed while the command waits and an
  interrupted tool whose group closes before the lease releases. It runs
  under `--root-parent /private/tmp`, the owner-accepted internal-placement
  exception (decision bd357a4c): its control pool is a genuine internal
  `directory-budget` pool, which the product refuses on an external volume.

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
# The last main commit without owned outputs: its state parser has no fence.
PRE_FEATURE = "8c9b5aeab57b8d0ca285b5358e07c89606724c8d"

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
    "src/main.rs": r'''use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    // A gated run records the executable it was started as, then waits, or
    // leaves an ordinary child (same process group) waiting and exits first.
    if let Ok(gate) = std::env::var("OXIDE_RUN_GATE") {
        let gate = PathBuf::from(gate);
        let exe = std::env::current_exe().unwrap();
        std::fs::write(gate.join("exe"), exe.to_string_lossy().as_bytes()).unwrap();
        if std::env::var_os("OXIDE_RUN_CHILD").is_some() {
            let release = gate.join("release");
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("n=0; while [ ! -f \"$1\" ] && [ $n -lt 1800 ]; do sleep 0.05; n=$((n+1)); done")
                .arg("sh")
                .arg(&release)
                .spawn()
                .unwrap();
            std::fs::write(gate.join("leader-exited"), b"exited").unwrap();
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(90);
        while !gate.join("release").exists() {
            assert!(Instant::now() < deadline, "proof did not release the run");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    println!("permanent-outputs-ok {}", poutputs::value());
}
''',
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


def set_container_uuid(pool: str, uuid: str) -> None:
    path = registration_path(pool)
    current = read_json(path)
    current["container_uuid"] = uuid
    with open(path, "wb") as handle:
        handle.write(json.dumps(current, indent=2).encode() + b"\n")
    os.chmod(path, 0o600)


def force_detach(proof: Proof, image: str, mount: str, uuid: str) -> None:
    """Surprise removal of the owned image only: positive identity of this
    image path's own whole device, its mount and its volume UUID immediately
    before `hdiutil detach -force` of that device."""
    info = plistlib.loads(proof.run(["/usr/bin/hdiutil", "info", "-plist"], timeout=30).stdout)
    own = [
        entry
        for entry in info.get("images", [])
        if os.path.realpath(entry.get("image-path", "")) == os.path.realpath(image)
    ]
    expect(len(own) == 1, f"image {image} attached {len(own)} times")
    entities = own[0]["system-entities"]
    mounts = {entity.get("mount-point") for entity in entities} - {None}
    expect(mounts == {mount}, f"image {image} is mounted at {mounts}, not {mount}")
    wholes = sorted(
        (entity["dev-entry"] for entity in entities if re.fullmatch(r"/dev/disk\d+", entity.get("dev-entry", ""))),
        key=lambda dev: int(dev[len("/dev/disk"):]),
    )
    expect(bool(wholes), f"no whole-disk device in {entities}")
    volume = plistlib.loads(proof.run(["/usr/sbin/diskutil", "info", "-plist", mount], timeout=30).stdout)
    expect(volume.get("VolumeUUID") == uuid, f"{mount} is not volume {uuid}")
    result = proof.run(["/usr/bin/hdiutil", "detach", "-force", wholes[0]], timeout=60)
    expect(result.returncode == 0, f"forced detach of {wholes[0]} failed; evidence kept: {result.stderr!r}")
    proof.mount = None
    expect(not os.path.lexists(mount), f"{mount} still exists after the forced detach")
    print(f"forced detach of {wholes[0]} ({mount})", flush=True)


def sha256_file(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


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
    expect(report["version"] == "oxide.outputs.plan/2", f"{label}: {report}")
    expect(report["layout"] in ("whole-target", "profile", "none"), f"{label}: {report}")
    expect(report["data_move"] is False, f"{label}: plan claims a data move: {report}")
    expect(len(stdout) < 64 * 1024, f"{label}: plan exceeds its byte bound")
    return report


def attach_image(
    proof: Proof, root: str, name: str, owners: bool = False, image_name: str = "volume.sparseimage"
) -> str:
    """Attach this proof's OWNED sparse image. `owners` is an optional
    `hdiutil attach -owners on` request used only by a proof that needs
    filesystem ownership honored on its private image; `image_name` selects a
    distinct owned image file when a proof needs a second isolated volume. Both
    defaults keep every existing selector's command and behaviour unchanged."""
    image = os.path.join(root, image_name)
    if not os.path.exists(image):
        created = proof.run(
            ["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name]
            + ["-type", "SPARSE", image],
            timeout=60,
        )
        expect(created.returncode == 0, f"image creation failed: {created.stderr!r}")
    attach_command = ["/usr/bin/hdiutil", "attach", "-nobrowse", "-plist"]
    if owners:
        attach_command.extend(["-owners", "on"])
    attach_command.append(image)
    attached = proof.run(attach_command, timeout=60)
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


def proof_env(pool: str, home: str, cargo_home: str) -> dict:
    """The host-isolated environment every fixture command runs under."""
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
            "OXIDE_TEST_HOME_FROM_ENV": "1",
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
    return env


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
    env = proof_env(pool, home, cargo_home)
    proof.env = env

    app = write_tree(os.path.join(root, "app"), APP)
    keep = write_plain(os.path.join(root, "keep"))
    foreign = write_plain(os.path.join(root, "foreign"))
    untracked = write_plain(os.path.join(root, "untracked"))
    for fixture in (app, keep, foreign, untracked):
        proof.run(["git", "init", "-q"], cwd=fixture, timeout=30)
        # A first conversion needs Git to ignore the exact non-directory link
        # path (oxide#125); `/.oxide-outputs/` is still left to `oxide init`.
        with open(os.path.join(fixture, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/target\n")
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
    # 2b. Without a receipt, permanent mode never hands Cargo the foreign link:
    #     a build and a selected clean refuse with nothing written or removed
    #     through it, including a Cargo-shaped artifact planted behind it.
    os.makedirs(os.path.join(outside, "debug"))
    with open(os.path.join(outside, "debug", "plain"), "wb") as handle:
        handle.write(b"planted")
    outside_before = sorted(
        os.path.relpath(os.path.join(base, name), outside)
        for base, dirs, files in os.walk(outside)
        for name in dirs + files
    )
    for args, label in (
        (["build"], "build through a receiptless foreign link"),
        (["clean", "-p", "plain"], "selected clean through a receiptless foreign link"),
    ):
        code, _, stderr = proof.oxide(args, label, foreign)
        expect(code != 0 and "never follows" in stderr, stderr)
        after = sorted(
            os.path.relpath(os.path.join(base, name), outside)
            for base, dirs, files in os.walk(outside)
            for name in dirs + files
        )
        expect(after == outside_before, f"{label} changed the link's referent: {after}")
        expect(os.readlink(os.path.join(foreign, "target")) == outside, f"{label} replaced the link")
    expect(read_bytes(os.path.join(outside, "debug", "plain")) == b"planted", "the planted artifact changed")

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
    expect(marker["version"] == "oxide.outputs.root/2", marker)
    expect(marker["checkout"] == app and marker["kind"] == "external", marker)
    receipt = read_json(receipt_path(pool, app))
    expect(
        receipt["version"] == "oxide.outputs.transition/2"
        and receipt["state"] == "committed"
        and receipt["checkout"] == app,
        receipt,
    )
    expect(not os.path.exists(os.path.join(app, ".oxide-outputs")), "adopt created a local root")
    proof.phase("after first conversion")

    # 4b. Downgrade fence: the last pre-feature Oxide refuses both pools'
    #     fenced state, so it never builds or cleans through the owned link.
    pre_src = os.path.join(root, "pre-feature-src")
    os.makedirs(pre_src)
    archive = proof.run(
        ["/bin/sh", "-c", f'git -C "$0" archive {PRE_FEATURE} | tar -x -C "$1"', repo, pre_src],
        timeout=60,
    )
    expect(archive.returncode == 0, f"pre-feature archive failed: {archive.stderr!r}")
    pre_feature = layout.stage_at(proof, pre_src, "oxide-pre-feature")
    shutil.rmtree(pre_src)
    expect(sha256_file(pre_feature) != sha256_file(proof.binary), "the pre-feature binary is the candidate")
    for state_dir in (os.path.join(pool, "oxide-managed"), os.path.join(pool, "oxide-managed", "removable", "oxide-managed")):
        fenced = read_json(os.path.join(state_dir, "state.json"))
        expect(fenced.get("owned_outputs") == "oxide.outputs/2", f"{state_dir} is not fenced: {fenced}")
    external_before = sorted(os.listdir(external_root))
    for args, label in (
        (["check"], "pre-feature check refuses the fenced pool"),
        (["clean", "-p", "poutputs"], "pre-feature selected clean refuses the fenced pool"),
    ):
        child = start_child(proof, [pre_feature] + args, app, env)
        code, _, stderr = finish_child(proof, child, label)
        # With current main's pool layout (oxide#118) the pre-feature binary
        # refuses the pool at accounting before it parses state; either
        # refusal keeps it off the owned link. The state-parser fence itself
        # is covered by store::tests::the_owned_outputs_fence_refuses_a_pre_feature_parser.
        expect(code != 0, f"{label}: the pre-feature binary did not refuse: {stderr}")
        expect(
            ("Oxide state is invalid" in stderr and "owned_outputs" in stderr)
            or "unknown_accounting" in stderr,
            f"{label}: unexpected refusal: {stderr}",
        )
        print(f"{label}: refused by {'the state fence' if 'owned_outputs' in stderr else 'pool accounting'}", flush=True)
        expect(link_target(link) == external_root, f"{label} moved the link")
        expect(sorted(os.listdir(external_root)) == external_before, f"{label} changed the owned root")
    proof.phase("after the downgrade fence")

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

    # 9c. Split selection: the drive is present and its output root valid, but
    #     the intermediate pool falls back (container mismatch). Outputs follow
    #     the pool to the local root, so nothing writes to the drive without
    #     its removal watchdog; restoring the container redocks.
    good_container = read_json(registration_path(pool))["container_uuid"]
    set_container_uuid(pool, "00000000-0000-0000-0000-00000000C0DE")
    code, _, stderr = proof.oxide(["check"], "pool fallback with the drive present", app)
    expect(code == 0 and link_target(link) == local_root, f"split selection kept the drive: {stderr}")
    expect("fell back to the internal pool" in stderr, stderr)
    set_container_uuid(pool, good_container)
    code, _, stderr = proof.oxide(["check"], "redock after the split selection", app)
    expect(code == 0 and link_target(link) == external_root, stderr)

    # 9d. A program `oxide run` started keeps the checkout's output custody
    #     until its whole process group closes: it runs from the real external
    #     root, its leader exits first leaving an ordinary child, and `run`
    #     stays held (a relink waits, plan names the holder) until the child
    #     ends too.
    run_gate = os.path.join(root, "run-gate")
    os.makedirs(run_gate)
    run_env = dict(env)
    run_env["OXIDE_RUN_GATE"] = run_gate
    run_env["OXIDE_RUN_CHILD"] = "1"
    runner = start_child(proof, [proof.binary, "run", "--bin", "poutputs"], app, run_env)
    relinker = None
    try:
        wait_for(os.path.join(run_gate, "leader-exited"), "run leader exit")
        exe = read_bytes(os.path.join(run_gate, "exe")).decode()
        expect(
            os.path.realpath(exe).startswith(external_root + os.sep) and not exe.startswith(link),
            f"the run did not execute from the bound real root: {exe}",
        )
        time.sleep(0.5)
        expect(runner.poll() is None, "oxide run returned while its program's child still runs")
        held = plan(proof, app, "plan while a run child lives")
        expect(held["cooperative_holders"], f"plan names no holder for the run: {held}")
        set_recorded_uuid(pool, wrong)
        relinker = start_child(proof, [proof.binary, "check"], app, env)
        time.sleep(1.5)
        expect(relinker.poll() is None, "a relink did not wait for the run program's child")
        expect(link_target(link) == external_root, "the link moved under a running program")
    finally:
        open(os.path.join(run_gate, "release"), "wb").close()
    code, stdout, stderr = finish_child(proof, runner, "run with an outliving child")
    expect(code == 0 and "permanent-outputs-ok" not in stdout, stdout + stderr)
    code, _, stderr = finish_child(proof, relinker, "relink after the run group closes")
    expect(code == 0 and link_target(link) == local_root, stderr)
    expect("a program `oxide run` started" in stderr, stderr)
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "redock after the run custody case", app)
    expect(code == 0 and link_target(link) == external_root, stderr)

    # 9e. Removal while the program runs from the external root: the removal
    #     watchdog stops the program's group, `run` fails, and after
    #     re-attaching the next command redocks.
    removal_gate = os.path.join(root, "removal-gate")
    os.makedirs(removal_gate)
    removal_env = dict(env)
    removal_env["OXIDE_RUN_GATE"] = removal_gate
    runner = start_child(proof, [proof.binary, "run", "--bin", "poutputs"], app, removal_env)
    try:
        wait_for(os.path.join(removal_gate, "exe"), "gated run before removal")
        force_detach(proof, os.path.join(root, "volume.sparseimage"), mount, good)
        code, stdout, stderr = finish_child(proof, runner, "run program stopped by removal")
    finally:
        open(os.path.join(removal_gate, "release"), "wb").close()
    expect(code != 0, f"a run whose drive vanished succeeded: {stdout}{stderr}")
    expect(
        re.search(rf"removable volume {re.escape(mount)} is gone; stopped Cargo group \d+: TERM", stderr)
        is not None,
        f"no removal stop during the program phase: {stderr}",
    )
    expect(attach_image(proof, root, name) == mount, "the drive re-attached elsewhere")
    code, _, stderr = proof.oxide(["check"], "redock after the program-phase removal", app)
    expect(code == 0 and link_target(link) == external_root, stderr)
    proof.phase("after the split selection, run custody and removal cases")

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

    # 12. Detach in ephemeral mode, then restore a real directory. Permanent
    #     mode detaches too (oxide#124): `detach-permanent` proves that.
    code, _, stderr = proof.oxide(["pool", "set-mode", "ephemeral"], "set ephemeral mode", app)
    expect(code == 0, stderr)
    # A converted checkout keeps building through its owned link in
    # ephemeral mode until it is detached; nothing is relinked.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "ephemeral build before detach", app)
    expect(code == 0 and link_target(link) == external_root, f"ephemeral build refused: {stderr}")
    expect(os.path.isfile(os.path.join(external_root, "debug", "poutputs")), "no binary in the bound root")
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


AUTO_NOTICE = "oxide: auto-adopted "


def tree_digest(root: str) -> dict:
    """Every entry under `root`: file hashes, link texts and directories, so
    a root's bytes compare exactly before and after a command."""
    digest = {}
    for current, dirs, files in os.walk(root):
        for name in dirs + files:
            full = os.path.join(current, name)
            relative = os.path.relpath(full, root)
            if os.path.islink(full):
                digest[relative] = "link:" + os.readlink(full)
            elif os.path.isfile(full):
                digest[relative] = sha256_file(full)
            else:
                digest[relative] = "dir"
    return digest


def receipt_state(pool: str, checkout: str) -> str:
    with open(receipt_file(pool, checkout), "rb") as handle:
        return json.load(handle)["state"]


def detach_run(proof: Proof, repo: str) -> None:
    """Focused per-checkout detach proof under permanent mode (oxide#124): the
    #121 fixture and staging, one owned 128 MiB scratch image, ordinary detach.
    Only the fixture commands' stderr, exit codes, link, receipt, plan and
    root bytes are checked."""
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxDp" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"detach-permanent proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-detach-permanent")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    image = os.path.join(root, "volume.sparseimage")

    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "unregistered build keeps a real target", maker)
    expect(code == 0 and not auto_notices(stderr), stderr)

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
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    proof.phase("after drive registration")

    # 1. The precondition: a fresh checkout adopts on its first build.
    app = fresh_checkout(proof, root, "detach-adopted")
    link = os.path.join(app, "target")
    code, _, stderr = proof.oxide(["build"], "fresh checkout adopts on its first build", app)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    # The automatic first conversion publishes the profile layout (oxide#160,
    # S4b): a real `target` holding an owned `debug`/`release` pair into one
    # root on the drive.
    adopted = expect_profile_layout(pool, mount, app, "the detach fixture's first conversion")
    adopted_debug = member_link_target(app, "debug")
    expect(os.path.isfile(os.path.join(link, "debug", "poutputs")), "binary not reachable through the link")
    proof.phase("after adoption")

    # 2. Detach waits for an active build in the checkout, then restores the
    #    real directory once that build has closed. The root keeps the build.
    gate = os.path.join(root, "gate")
    gate_env = dict(proof.env, OXIDE_OUTPUT_GATE=gate)
    reset_gate(gate)
    writer = start_child(proof, [proof.binary, "check"], app, gate_env)
    waiting = None
    try:
        wait_for(os.path.join(gate, "started"), "gated build script")
        expect(writer.poll() is None, "the gated build exited early")
        waiting = start_child(proof, [proof.binary, "outputs", "detach"], app, proof.env)
        time.sleep(2.0)
        if waiting.poll() is not None:
            early_out, early_err = waiting.communicate(timeout=5)
            proof.unclosed.discard(waiting.pid)
            fail(
                "outputs detach did not wait for the active build in the checkout: "
                f"exit {waiting.returncode}\n{early_out.decode(errors='replace')}"
                f"{early_err.decode(errors='replace')}"
            )
        expect(
            member_link_target(app, "debug") == adopted_debug,
            "the owned member link moved while a build was running",
        )
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        writer_code, _, writer_stderr = finish_child(proof, writer, "gated build")
        expect(writer_code == 0, f"the gated build failed: {writer_stderr}")
        expect(os.path.isdir(adopted), "the build wrote nothing to the root")
        before = tree_digest(adopted)
        detach_code, detach_stdout, detach_stderr = finish_child(proof, waiting, "detach after the build")
        waiting = None
        expect(detach_code == 0, f"detach after the build failed: {detach_stdout}{detach_stderr}")
    finally:
        os.makedirs(gate, exist_ok=True)
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        if writer.poll() is None:
            writer.kill()
        if waiting is not None and waiting.poll() is None:
            waiting.kill()
    proof.phase("after the gated detach")

    # 3. The detach restored a real empty directory, kept every root byte and
    #    recorded a detached receipt. Plan and status agree.
    expect("real empty directory again" in detach_stdout, detach_stdout)
    expect(os.path.isdir(link) and not os.path.islink(link), "detach did not restore a real directory")
    # The profile layout's detach restores one real EMPTY directory per owned
    # member and keeps the real target directory (oxide#133/#144).
    for member in ("debug", "release"):
        path = os.path.join(link, member)
        expect(
            os.path.isdir(path) and not os.path.islink(path),
            f"detach did not restore a real {member} directory",
        )
        expect(os.listdir(path) == [], f"the detached {member} is not empty")
    expect(tree_digest(adopted) == before, "detach changed the owned root")
    expect(receipt_state(pool, app) == "detached", "the receipt is not detached")
    code, stdout, stderr = proof.oxide(["outputs", "status"], "status after detach", app)
    expect(code == 0 and "detached" in stdout and "permanent" in stdout, stdout + stderr)
    report = plan(proof, app, "plan after detach")
    expect(report["mode"] == "permanent" and report["state"] == "detached", report)
    # The real target now holds the (empty) member directories, so the target
    # itself classifies as populated even though every member is empty.
    expect(report["current_target"] == "real-nonempty", report)
    expect(report["layout"] == "profile", report)
    expect("selected" not in report and "current_root" not in report, report)
    expect("detached" in (report.get("reason") or ""), report)
    code, stdout, stderr = proof.oxide(["outputs", "detach"], "a second detach changes nothing", app)
    expect(code == 0 and "already detached" in stdout, stdout + stderr)
    proof.phase("after the detach")

    # 4. The next leased builds use the real target: no relink, no notice, and
    #    the detached root is never written.
    code, _, stderr = proof.oxide(["build"], "the next build uses the real target", app)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(os.path.isdir(link) and not os.path.islink(link), "a build relinked the target")
    expect(os.path.isfile(os.path.join(link, "debug", "poutputs")), "no binary in the real target")
    expect(tree_digest(adopted) == before, "a build wrote into the detached root")
    code, _, stderr = proof.oxide(["build"], "a repeated build stays real, no notice", app)
    expect(code == 0 and not auto_notices(stderr) and not os.path.islink(link), stderr)
    report = plan(proof, app, "plan after real-target builds")
    expect(report["state"] == "detached" and report["current_target"] == "real-nonempty", report)
    proof.phase("after the real-target builds")

    # 5. Controls. Explicit adopt never converts a NONEMPTY real member (the
    #    profile contract keeps it local) and writes nothing into the root; a
    #    missing target beside a detached receipt is never adopted.
    before = tree_digest(adopted)
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "adopt keeps the nonempty real members", app)
    expect(code == 0, f"adopt refused the nonempty members: {stdout}{stderr}")
    expect("stay real directories" in stderr, stderr)
    expect(
        os.path.isfile(os.path.join(link, "debug", "poutputs")),
        "adopt moved or converted the real member",
    )
    expect(
        os.path.isdir(os.path.join(link, "debug"))
        and not os.path.islink(os.path.join(link, "debug")),
        "adopt linked a nonempty real member",
    )
    expect(tree_digest(adopted) == before, "adopt wrote into the root")
    shutil.rmtree(link)
    code, _, stderr = proof.oxide(["build"], "a detached receipt with an absent target is never adopted", app)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(os.path.isdir(link) and not os.path.islink(link), "a detached checkout was adopted")
    expect(receipt_state(pool, app) == "detached", "the receipt left detached state")
    proof.phase("after the absent-target control")

    # 6. Explicit adopt re-converts an empty detached target, to the same root
    #    the checkout already had, through the existing preconditions.
    again = fresh_checkout(proof, root, "detach-readopt")
    code, _, stderr = proof.oxide(["build"], "re-adopt fixture adopts on its first build", again)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    first = os.path.dirname(member_link_target(again, "debug"))
    code, _, stderr = proof.oxide(["outputs", "detach"], "detach the re-adopt fixture", again)
    expect(code == 0, stderr)
    for member in ("debug", "release"):
        expect(
            os.listdir(os.path.join(again, "target", member)) == [],
            f"the re-adopt {member} is not empty",
        )
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "explicit adopt converts the empty members", again)
    expect(code == 0 and "now selects the owned external root" in stdout, stdout + stderr)
    expect(
        os.path.dirname(member_link_target(again, "debug")) == first,
        "re-adopt chose another root",
    )
    expect(receipt_state(pool, again) == "committed", "re-adopt did not commit")
    first_debug = member_link_target(again, "debug")
    code, _, stderr = proof.oxide(["build"], "the re-adopted checkout builds through its links", again)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(member_link_target(again, "debug") == first_debug, "a build relinked the re-adopted member")
    proof.phase("after explicit re-adoption")

    # 7. Errors stay errors: a foreign link refuses the detach with no change
    #    and the receipt stays committed; a checkout never converted has no
    #    receipt to detach.
    foreign = fresh_checkout(proof, root, "detach-foreign")
    code, _, stderr = proof.oxide(["build"], "foreign fixture adopts on its first build", foreign)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    outside = os.path.join(root, "outside-detach")
    os.makedirs(outside)
    with open(os.path.join(outside, "sentinel"), "wb") as handle:
        handle.write(b"foreign")
    os.remove(os.path.join(foreign, "target"))
    os.symlink(outside, os.path.join(foreign, "target"))
    code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach refuses a foreign link", foreign)
    expect(code != 0 and "left untouched" in stderr, stdout + stderr)
    expect(os.readlink(os.path.join(foreign, "target")) == outside, "the foreign link was replaced")
    expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "the foreign referent changed")
    expect(receipt_state(pool, foreign) == "committed", "a refused detach changed the receipt")

    never = fresh_checkout(proof, root, "detach-never-converted")
    code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach without a receipt refuses", never)
    expect(code != 0 and "nothing to detach" in stderr, stdout + stderr)
    expect(not os.path.lexists(os.path.join(never, "target")), "a refused detach created a target")
    proof.phase("after the error controls")

    proof.detach()


def auto_notices(stderr: str) -> list[str]:
    return [line for line in stderr.splitlines() if line.startswith(AUTO_NOTICE)]


def receipt_file(pool: str, checkout: str) -> str:
    """Where a receipt is, present or not (`receipt_path` requires it)."""
    key = hashlib.sha256(checkout.encode()).hexdigest()
    return os.path.join(pool, "oxide-managed", "outputs", key + ".json")


def fresh_checkout(proof: Proof, root: str, name: str, ignore: str | None = "/target\n") -> str:
    """A fresh checkout in its own Git work tree. `ignore` is its whole
    `.gitignore`; a first conversion needs Git to ignore the exact
    non-directory link path (oxide#125)."""
    path = write_tree(os.path.join(root, "checkouts", name), APP)
    proof.run(["git", "init", "-q"], cwd=path, timeout=30)
    if ignore is not None:
        with open(os.path.join(path, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write(ignore)
    expect(not os.path.lexists(os.path.join(path, "target")), f"{name} already has a target")
    return path


def set_mode(proof: Proof, mode: str, cwd: str) -> None:
    code, _, stderr = proof.oxide(["pool", "set-mode", mode], f"drive mode {mode}", cwd)
    expect(code == 0, stderr)


def auto_run(proof: Proof, repo: str) -> None:
    """Focused auto-adopt proof (oxide#121): one fixture, one volume, the
    #117 scenario's staging and helpers. Only the fixture commands' stderr,
    exit codes and link, root, receipt and source bytes are checked."""
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxAd" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"auto-adopt proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-auto-adopt")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    image = os.path.join(root, "volume.sparseimage")

    # The internal pool exists before any drive is registered: a build then
    # keeps its real target, as it always has.
    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "unregistered build keeps a real target", maker)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(not os.path.islink(os.path.join(maker, "target")), "unregistered build linked target")

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
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    uuid = real_uuid(pool)
    wrong = "00000000-0000-0000-0000-000000000000"
    proof.phase("after drive registration")

    # 1. Eligible: a fresh checkout adopts on its first leased build. One
    #    notice, the profile layout the explicit adoption publishes (oxide#160,
    #    S4b: a real `target` holding an owned `debug`/`release` pair into one
    #    root on the drive), output through the pair, a receipt, then a warm
    #    rebuild with no notice and the same links.
    fresh = fresh_checkout(proof, root, "fresh")
    link = os.path.join(fresh, "target")
    code, _, stderr = proof.oxide(["build"], "fresh checkout adopts on its first build", fresh)
    expect(code == 0, stderr)
    notes = auto_notices(stderr)
    expect(len(notes) == 1, f"expected exactly one auto notice: {stderr}")
    expect(link in notes[0] and "no existing outputs moved" in notes[0], notes[0])
    expect(
        os.path.isdir(link) and not os.path.islink(link),
        "the fresh target is not a real directory: the first conversion did not "
        "create profile members (oxide#160, S4b)",
    )
    fresh_root = expect_profile_layout(pool, mount, fresh, "the auto-adopted fresh checkout")
    binary = os.path.join(link, "debug", "poutputs")
    expect(os.path.isfile(binary), "the binary is not reachable through the owned member link")
    expect(os.path.realpath(binary).startswith(mount + os.sep), "the binary is not on the drive")
    expect(slot_state(stderr)[1] == "cold", stderr)
    code, stdout, stderr = proof.oxide(["status"], "status shows the adopted root", fresh)
    expect(code == 0 and fresh_root in stdout, stdout + stderr)
    code, _, stderr = proof.oxide(["build"], "adopted checkout rebuilds warm, no notice", fresh)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(slot_state(stderr)[1] == "warm", f"adopted destination key did not reuse: {stderr}")
    expect(
        member_link_target(fresh, "debug") == os.path.join(fresh_root, "debug"),
        "a warm build relinked the debug member",
    )
    expect(
        member_link_target(fresh, "release") == os.path.join(fresh_root, "release"),
        "a warm build relinked the release member",
    )
    proof.phase("after eligible adoption")

    # A second fresh checkout gets its own root, leaf and receipt.
    second = fresh_checkout(proof, root, "second")
    code, _, stderr = proof.oxide(["build"], "second fresh checkout adopts its own root", second)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    expect(
        os.path.dirname(member_link_target(second, "debug")) != fresh_root,
        "two checkouts share a root",
    )

    # 2. Controls: every present target is kept, and no notice is printed.
    keep = fresh_checkout(proof, root, "real-target")
    os.makedirs(os.path.join(keep, "target"))
    with open(os.path.join(keep, "target", "keep.txt"), "wb") as handle:
        handle.write(b"warm build")
    code, _, stderr = proof.oxide(["build"], "a non-empty real target is never adopted", keep)
    expect(not auto_notices(stderr), stderr)
    expect(
        os.path.isdir(os.path.join(keep, "target"))
        and not os.path.islink(os.path.join(keep, "target")),
        "the real target was replaced",
    )
    expect(read_bytes(os.path.join(keep, "target", "keep.txt")) == b"warm build", "target mutated")
    expect(not os.path.exists(receipt_file(pool, keep)), "a control wrote a receipt")

    empty = fresh_checkout(proof, root, "empty-target")
    os.makedirs(os.path.join(empty, "target"))
    code, _, stderr = proof.oxide(["build"], "an empty real target is never adopted", empty)
    expect(not auto_notices(stderr) and not os.path.islink(os.path.join(empty, "target")), stderr)
    expect(not os.path.exists(receipt_file(pool, empty)), "an empty target got a receipt")

    outside = os.path.join(root, "outside")
    os.makedirs(outside)
    with open(os.path.join(outside, "sentinel"), "wb") as handle:
        handle.write(b"foreign")
    foreign = fresh_checkout(proof, root, "foreign-link")
    os.symlink(outside, os.path.join(foreign, "target"))
    code, _, stderr = proof.oxide(["build"], "a foreign link is refused, not adopted", foreign)
    expect(code != 0 and not auto_notices(stderr), stderr)
    expect(os.readlink(os.path.join(foreign, "target")) == outside, "foreign link was replaced")
    expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "foreign referent mutated")

    absent = os.path.join(root, "absent-root")
    dangling = fresh_checkout(proof, root, "dangling-link")
    os.symlink(absent, os.path.join(dangling, "target"))
    code, _, stderr = proof.oxide(["build"], "a dangling link is refused, not adopted", dangling)
    expect(code != 0 and not auto_notices(stderr), stderr)
    expect(os.readlink(os.path.join(dangling, "target")) == absent, "dangling link was replaced")
    expect(not os.path.lexists(absent), "the dangling link's root was created")

    filed = fresh_checkout(proof, root, "file-target")
    with open(os.path.join(filed, "target"), "wb") as handle:
        handle.write(b"not a directory")
    code, _, stderr = proof.oxide(["build"], "a file at target is never adopted", filed)
    expect(not auto_notices(stderr), stderr)
    expect(read_bytes(os.path.join(filed, "target")) == b"not a directory", "file target mutated")
    proof.phase("after controls")

    # 3. A corrupt receipt is an error, never an absent receipt: the command
    #    refuses before any slot is taken, and the missing target stays missing.
    bad = fresh_checkout(proof, root, "bad-receipt")
    os.makedirs(os.path.dirname(receipt_file(pool, bad)), exist_ok=True)
    corrupt = receipt_file(pool, bad)
    with open(corrupt, "wb") as handle:
        handle.write(b"{not a receipt")
    # Owner-only, so the product reaches its parse refusal, not the mode check.
    os.chmod(corrupt, 0o600)
    code, _, stderr = proof.oxide(["build"], "a corrupt receipt refuses, never adopts", bad)
    expect(code != 0 and "is invalid" in stderr and not auto_notices(stderr), stderr)
    expect(not os.path.lexists(os.path.join(bad, "target")), "a corrupt receipt let a target appear")
    os.remove(receipt_file(pool, bad))

    # 4. A detached checkout with its target missing is excluded: the receipt
    #    of any state refuses automatic adoption.
    det = fresh_checkout(proof, root, "detached")
    code, _, stderr = proof.oxide(["build"], "detached checkout starts adopted", det)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    set_mode(proof, "ephemeral", det)
    code, _, stderr = proof.oxide(["outputs", "detach"], "detach the checkout", det)
    expect(code == 0, stderr)
    shutil.rmtree(os.path.join(det, "target"))
    set_mode(proof, "permanent", det)
    code, _, stderr = proof.oxide(["build"], "missing target with a detached receipt is excluded", det)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(
        os.path.isdir(os.path.join(det, "target")) and not os.path.islink(os.path.join(det, "target")),
        "a detached checkout was adopted",
    )
    with open(receipt_file(pool, det), "rb") as handle:
        expect(json.load(handle)["state"] == "detached", "the receipt left detached state")
    proof.phase("after detached exclusion")

    # 5. Ephemeral registration: no adoption, a real target, then permanent.
    set_mode(proof, "ephemeral", fresh)
    eph = fresh_checkout(proof, root, "ephemeral-drive")
    code, _, stderr = proof.oxide(["build"], "ephemeral registration never adopts", eph)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(not os.path.islink(os.path.join(eph, "target")), "ephemeral build linked its target")
    set_mode(proof, "permanent", fresh)

    # 6. Absent drive: no adoption, the internal real target. The drive is
    #    then re-attached at the same mount point.
    force_detach(proof, image, mount, uuid)
    gone = fresh_checkout(proof, root, "absent-drive")
    code, _, stderr = proof.oxide(["build"], "an absent drive never adopts", gone)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(not os.path.islink(os.path.join(gone, "target")), "absent drive linked its target")
    expect(not os.path.exists(receipt_file(pool, gone)), "absent drive wrote a receipt")
    mounted = attach_image(proof, root, name)
    expect(mounted == mount, f"re-attached at {mounted}, not {mount}")
    proof.phase("after absent drive")

    # 7. Internal fallback (recorded volume UUID mismatch): no adoption.
    set_recorded_uuid(pool, wrong)
    mismatch = fresh_checkout(proof, root, "uuid-mismatch")
    code, _, stderr = proof.oxide(["build"], "a fallback to the internal pool never adopts", mismatch)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(not os.path.islink(os.path.join(mismatch, "target")), "fallback linked its target")
    set_recorded_uuid(pool, uuid)
    proof.phase("after fallback control")

    # 8. Crash replay. A stop after the sibling link leaves a prepared receipt;
    #    a creator that appears at the target afterwards is never replaced.
    crashed = fresh_checkout(proof, root, "crash-linked")
    crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="linked")
    code, _, stderr = oxide_env(proof, ["build"], "crash after the sibling link", crashed, crash_env)
    expect(code != 0 and "injected transition stop at linked" in stderr, stderr)
    expect(not auto_notices(stderr), "a crashed adoption printed its notice")
    # The first conversion's FIRST member transition is what crashes, so the
    # sibling link belongs to `target/debug` and the real `target` directory
    # already exists (oxide#160, S4b).
    expect(
        os.path.isdir(os.path.join(crashed, "target")),
        "the crash did not leave the profile target directory",
    )
    sibling = os.path.join(crashed, "target", ".debug.oxide-new-1")
    expect(os.path.islink(sibling), "the crash left no sibling link")
    with open(os.path.join(crashed, "target", "debug"), "wb") as handle:
        handle.write(b"creator")
    code, _, stderr = proof.oxide(["build"], "replay refuses a creator at the member link", crashed)
    expect(code != 0 and "cannot be replayed" in stderr, stderr)
    expect(
        read_bytes(os.path.join(crashed, "target", "debug")) == b"creator",
        "the creator was replaced",
    )
    expect(os.path.islink(sibling), "the prepared sibling was removed")

    # A stop before any link is published: the next command's replay rolls the
    # preparation back to detached and refuses once (its slot was keyed for the
    # root). The command after it builds as an unconverted checkout.
    prepared = fresh_checkout(proof, root, "crash-prepared")
    crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="prepared")
    code, _, stderr = oxide_env(proof, ["build"], "crash before publication", prepared, crash_env)
    expect(code != 0 and "injected transition stop at prepared" in stderr, stderr)
    expect(
        not os.path.lexists(os.path.join(prepared, "target", "debug")),
        "a stop published a member link",
    )
    code, _, stderr = proof.oxide(["build"], "replay rolls back an unpublished adoption", prepared)
    expect(code != 0 and "run the command again" in stderr and not auto_notices(stderr), stderr)
    code, _, stderr = proof.oxide(["build"], "the command after the rollback is unconverted", prepared)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(not os.path.islink(os.path.join(prepared, "target")), "rollback left a link")
    with open(receipt_file(pool, prepared), "rb") as handle:
        expect(json.load(handle)["state"] == "detached", "rollback did not detach")
    proof.phase("after crash replay")

    # 9. Committed adoption survives a failed build: one notice, the link and
    #    receipt stay, and a repaired source builds through them.
    failing = fresh_checkout(proof, root, "build-fails")
    with open(os.path.join(failing, "src", "main.rs"), "w", encoding="utf-8") as handle:
        handle.write('fn main() { let value: u32 = "not a number"; }\n')
    code, _, stderr = proof.oxide(["build"], "adoption survives a failed build", failing)
    expect(code != 0 and len(auto_notices(stderr)) == 1, stderr)
    expect(
        os.path.isdir(os.path.join(failing, "target"))
        and os.path.islink(os.path.join(failing, "target", "debug")),
        "the committed member links were lost",
    )
    with open(os.path.join(failing, "src", "main.rs"), "w", encoding="utf-8") as handle:
        handle.write(APP["src/main.rs"])
    code, _, stderr = proof.oxide(["build"], "the repaired source builds through the link", failing)
    expect(code == 0 and not auto_notices(stderr), stderr)
    proof.phase("after failed build")

    proof.detach()


def roots_on(mount: str) -> list[str]:
    """The output roots currently on the drive's oxide-outputs directory."""
    root = os.path.join(mount, "oxide-outputs")
    return sorted(os.listdir(root)) if os.path.isdir(root) else []


def repo_relative_target(proof: Proof, checkout: str) -> str:
    """This checkout's `target` relative to its repository root."""
    result = proof.run(["git", "rev-parse", "--show-prefix"], cwd=checkout, timeout=30)
    expect(result.returncode == 0, f"git rev-parse --show-prefix failed: {result.stderr!r}")
    return result.stdout.decode(errors="replace").strip() + "target"


def git_status_names_target(proof: Proof, checkout: str) -> list[str]:
    """Exact `git status` entries naming this checkout's `target`.

    NUL delimited, with every untracked file listed: a wholly untracked
    workspace would otherwise collapse to one entry such as `rust/` and hide an
    exposed `rust/target`. The path is compared exactly (repository-relative,
    or the checkout-relative form some Git versions print), never by suffix."""
    expected = repo_relative_target(proof, checkout)
    result = proof.run(
        ["git", "status", "--porcelain", "-z", "--untracked-files=all", "--no-renames"],
        cwd=checkout,
        timeout=30,
    )
    expect(result.returncode == 0, f"git status failed in {checkout}: {result.stderr!r}")
    named = []
    for part in result.stdout.split(b"\0"):
        if not part:
            continue
        entry = part.decode(errors="replace")
        expect(
            len(entry) > 3 and entry[2] == " ",
            f"unexpected git status entry {entry!r}",
        )
        if entry[3:] in (expected, "target"):
            named.append(entry)
    return named


def git_index_names_target(proof: Proof, checkout: str) -> bytes:
    result = proof.run(["git", "ls-files", "-z", "--", "target"], cwd=checkout, timeout=30)
    expect(result.returncode == 0, f"git ls-files failed in {checkout}: {result.stderr!r}")
    return result.stdout


def ignore_fixture(
    proof: Proof,
    root: str,
    name: str,
    ignore: str | None,
    *,
    empty: bool = False,
    relative: str = "",
    git: bool = True,
) -> tuple[str, str]:
    """A tiny Cargo checkout in its own Git work tree. `ignore` is the whole
    repository `.gitignore`; `relative` nests the checkout under that root so
    its owned link has a nested repository-relative path."""
    repo = os.path.join(root, "ignore", name)
    os.makedirs(repo)
    checkout = write_tree(os.path.join(repo, relative) if relative else repo, APP)
    if git:
        proof.run(["git", "init", "-q"], cwd=repo, timeout=30)
    if ignore is not None:
        with open(os.path.join(repo, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write(ignore)
    if empty:
        os.makedirs(os.path.join(checkout, "target"))
    expect(not os.path.islink(os.path.join(checkout, "target")), f"{name}: target is already a link")
    return repo, checkout


def refused_untouched(
    proof: Proof,
    pool: str,
    mount: str,
    checkout: str,
    label: str,
    args: list,
    reasons: tuple,
    *,
    empty: bool = False,
    env: dict | None = None,
    keeps_target: bool = False,
) -> str:
    """One refused conversion: the reason names the fix and the target, the
    receipt and the drive's roots are exactly as they were."""
    before = roots_on(mount)
    if env is None:
        code, stdout, stderr = proof.oxide(args, label, checkout)
    else:
        code, stdout, stderr = oxide_env(proof, args, label, checkout, env)
    expect(code != 0, f"{label}: nothing was refused (exit {code})\n{stdout}{stderr}")
    expect(
        any(reason in stderr for reason in reasons),
        f"{label}: unexpected refusal for {reasons}: {stderr}",
    )
    # Presentation-insensitive: the product closes some refusals with
    # "Nothing was changed" and others with "; nothing was changed".
    expect(
        "nothing was changed" in stderr.casefold(),
        f"{label}: the refusal omits the no-change promise: {stderr}",
    )
    link = os.path.join(checkout, "target")
    if empty:
        expect(
            os.path.isdir(link) and not os.path.islink(link) and os.listdir(link) == [],
            f"{label}: the real empty target changed",
        )
    elif not keeps_target:
        expect(not os.path.lexists(link), f"{label}: a target appeared")
    expect(not os.path.exists(receipt_file(pool, checkout)), f"{label}: a receipt was written")
    expect(roots_on(mount) == before, f"{label}: the drive's roots changed")
    return stderr


def expect_published(proof: Proof, pool: str, mount: str, checkout: str, label: str) -> str:
    """One accepted conversion's profile layout, receipt and Git invisibility.

    Since oxide#160 (S4b) a first conversion publishes profile members: a real
    `<checkout>/target` holding an owned `<root>/<member>` pair."""
    link = os.path.join(checkout, "target")
    expect(
        os.path.isdir(link) and not os.path.islink(link),
        f"{label}: target is not a real directory",
    )
    root = expect_profile_layout(pool, mount, checkout, label)
    expect(os.path.isdir(link), f"{label}: the converted target is missing")
    receipt = read_json(receipt_file(pool, checkout))
    expect(
        receipt["state"] == "committed" and receipt["generation"] >= 1,
        f"{label}: the receipt is not a committed first conversion: {receipt}",
    )
    expect(
        git_status_names_target(proof, checkout) == [],
        f"{label}: git status lists the published target",
    )
    expect(
        git_index_names_target(proof, checkout) == b"",
        f"{label}: git ls-files shows target tracked",
    )
    return root


def fake_git(root: str, kind: str, script: str) -> str:
    """A private `git` first on PATH for one oxide invocation."""
    directory = os.path.join(root, "fake-git", kind)
    os.makedirs(directory)
    path = os.path.join(directory, "git")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(script)
    os.chmod(path, 0o755)
    return directory


def private_path(root: str) -> str:
    """A PATH with the tools oxide needs before its first Git call, and no
    git at all."""
    directory = os.path.join(root, "no-git-bin")
    os.makedirs(directory)
    for tool in ("cargo", "rustc", "rustup"):
        found = shutil.which(tool)
        expect(found is not None, f"{tool} is not on the host PATH")
        os.symlink(found, os.path.join(directory, tool))
    return directory


def ignore_run(proof: Proof, repo: str) -> None:
    """Focused ignore-safety proof (oxide#125, updated for oxide#160/S4b): a
    first conversion publishes owned `<checkout>/target/<member>` links only
    when Git ignores the exact repository-relative path of the link it is about
    to publish, and refuses — changing nothing at all, not even creating the
    real target directory — for a rule that re-includes the member, an
    unignored path, a tracked member, no Git, no repository, a timeout, a
    command error and a malformed answer. A directory-only rule such as
    `/target/` genuinely covers a member path inside the real target directory,
    so it is accepted; the whole-target link's stricter non-directory rule no
    longer applies to a first conversion. Explicit adopt, automatic adoption
    and the replay of a prepared first conversion are all covered, and detach
    still works."""
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxIg" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"ignore-safety proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-ignore-safety")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)

    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "internal pool before the drive", maker)
    expect(code == 0, stderr)

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
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    proof.phase("after drive registration")

    # 1. A directory-only rule genuinely covers a profile MEMBER path: the
    #    published link is `target/<member>` inside a real target directory, so
    #    `/target/`, `target/` and `**/target/` are accepted (oxide#160, S4b).
    #    The single non-directory-path rule is accepted the same way.
    for index, rule in enumerate(("/target/", "target/", "**/target/", "/target", "target")):
        _, checkout = ignore_fixture(proof, root, f"rule-{index}", rule + "\n/.oxide-outputs/\n")
        code, _, stderr = proof.oxide(
            ["outputs", "adopt"], f"explicit adopt with {rule!r}", checkout
        )
        expect(code == 0, f"{rule!r}: an explicit adopt refused the rule: {stderr}")
        expect_published(proof, pool, mount, checkout, f"explicit adopt with {rule!r}")
    for index, rule in enumerate(("/target/", "target")):
        _, checkout = ignore_fixture(proof, root, f"auto-{index}", rule + "\n/.oxide-outputs/\n")
        code, _, stderr = proof.oxide(["build"], f"automatic adoption with {rule!r}", checkout)
        expect(code == 0 and len(auto_notices(stderr)) == 1, f"{rule!r}: {stderr}")
        expect_published(proof, pool, mount, checkout, f"automatic adoption with {rule!r}")
    #    A real empty target takes the same accepted path.
    _, empty = ignore_fixture(
        proof, root, "empty-accepted", "/target/\n/.oxide-outputs/\n", empty=True
    )
    code, _, stderr = proof.oxide(["outputs", "adopt"], "explicit adopt of an empty target", empty)
    expect(code == 0, f"an empty target was refused: {stderr}")
    expect_published(proof, pool, mount, empty, "explicit adopt of an empty target")
    proof.phase("after the directory-rule matrix")

    # 2. A nested workspace names its exact repository-relative path, and a
    #    directory-only nested rule covers the nested member too.
    _, nested = ignore_fixture(
        proof,
        root,
        "nested-empty",
        "/rust/target/\n.oxide-outputs/\n",
        relative="rust",
        empty=True,
    )
    code, _, stderr = proof.oxide(["outputs", "adopt"], "nested /rust/target/", nested)
    expect(code == 0, f"the nested directory rule was refused: {stderr}")
    expect_published(proof, pool, mount, nested, "the nested directory rule")
    _, nested_missing = ignore_fixture(
        proof, root, "nested-unignored", ".oxide-outputs/\n", relative="rust"
    )
    refused_untouched(
        proof,
        pool,
        mount,
        nested_missing,
        "a nested checkout with no target rule",
        ["outputs", "adopt"],
        ("does not ignore",),
    )
    expect(
        not os.path.lexists(os.path.join(nested_missing, "target")),
        "a refused nested conversion created a target",
    )

    # 3. Negated and unignored paths refuse; nothing links.
    _, negated = ignore_fixture(
        proof, root, "negated", "/target/\n!/target/debug\n/.oxide-outputs/\n"
    )
    stderr = refused_untouched(
        proof,
        pool,
        mount,
        negated,
        "a rule that re-includes the debug member",
        ["outputs", "adopt"],
        ("re-includes",),
    )
    expect("target/debug" in stderr, stderr)
    _, unignored = ignore_fixture(proof, root, "unignored", "/.oxide-outputs/\n")
    refused_untouched(
        proof,
        pool,
        mount,
        unignored,
        "an unignored target",
        ["outputs", "adopt"],
        ("does not ignore",),
    )

    # 4. A tracked path refuses even when the working tree has no target.
    _, tracked = ignore_fixture(proof, root, "tracked", "/target\n/.oxide-outputs/\n")
    os.makedirs(os.path.join(tracked, "target"))
    with open(os.path.join(tracked, "target", "debug"), "wb") as handle:
        handle.write(b"tracked")
    added = proof.run(["git", "add", "-f", "target/debug"], cwd=tracked, timeout=30)
    expect(added.returncode == 0, f"git add failed: {added.stderr!r}")
    os.remove(os.path.join(tracked, "target", "debug"))
    stderr = refused_untouched(
        proof,
        pool,
        mount,
        tracked,
        "a tracked member path",
        ["outputs", "adopt"],
        ("already tracks",),
        empty=True,
    )
    expect("target/debug" in stderr, stderr)

    # 5. Unknown Git is never permission: no repository, no Git binary, a
    #    timeout, a command error and a malformed answer all refuse.
    _, no_repo = ignore_fixture(proof, root, "no-repository", "/target\n", git=False)
    refused_untouched(
        proof,
        pool,
        mount,
        no_repo,
        "no Git repository",
        ["outputs", "adopt"],
        ("could not confirm",),
    )
    no_git_env = dict(proof.env, PATH=private_path(root))
    _, no_git = ignore_fixture(proof, root, "no-git", "/target\n/.oxide-outputs/\n")
    refused_untouched(
        proof,
        pool,
        mount,
        no_git,
        "no Git binary on PATH",
        ["build"],
        ("could not confirm", "cannot run git"),
        env=no_git_env,
    )
    _, slow = ignore_fixture(proof, root, "slow-git", "/target\n/.oxide-outputs/\n")
    slow_dir = fake_git(root, "timeout", "#!/bin/sh\nexec sleep 30\n")
    refused_untouched(
        proof,
        pool,
        mount,
        slow,
        "a Git that does not answer in time",
        ["outputs", "adopt"],
        ("did not finish in time",),
        env=dict(proof.env, PATH=slow_dir + ":" + proof.env["PATH"]),
    )
    real_git = shutil.which("git")
    expect(real_git is not None, "git is not on the host PATH")
    _, invalid = ignore_fixture(proof, root, "invalid-git", "/target\n/.oxide-outputs/\n")
    invalid_dir = fake_git(
        root,
        "invalid",
        "#!/bin/sh\ncase \"$*\" in\n  *check-ignore*) printf 'not-a-rule'; exit 0 ;;\nesac\n"
        f'exec "{real_git}" "$@"\n',
    )
    refused_untouched(
        proof,
        pool,
        mount,
        invalid,
        "a malformed Git ignore answer",
        ["outputs", "adopt"],
        ("not one complete record",),
        env=dict(proof.env, PATH=invalid_dir + ":" + proof.env["PATH"]),
    )
    _, failing = ignore_fixture(proof, root, "failing-git", "/target\n/.oxide-outputs/\n")
    failing_dir = fake_git(
        root,
        "error",
        "#!/bin/sh\ncase \"$*\" in\n  *check-ignore*) exit 128 ;;\nesac\n"
        f'exec "{real_git}" "$@"\n',
    )
    refused_untouched(
        proof,
        pool,
        mount,
        failing,
        "a failing Git ignore call",
        ["outputs", "adopt"],
        ("did not answer",),
        env=dict(proof.env, PATH=failing_dir + ":" + proof.env["PATH"]),
    )
    proof.phase("after the unknown-Git controls")

    # 6. Present but unadoptable targets keep their content and are not
    #    covered by this guard.
    _, nonempty = ignore_fixture(proof, root, "nonempty", "/target\n/.oxide-outputs/\n")
    os.makedirs(os.path.join(nonempty, "target"))
    with open(os.path.join(nonempty, "target", "keep.txt"), "wb") as handle:
        handle.write(b"warm build")
    code, _, stderr = proof.oxide(["outputs", "adopt"], "a nonempty target keeps its file", nonempty)
    expect(code == 0, f"adopt refused a nonempty target: {stderr}")
    expect(
        read_bytes(os.path.join(nonempty, "target", "keep.txt")) == b"warm build",
        "the top-level target file was moved",
    )
    expect(os.path.isdir(os.path.join(nonempty, "target")), "the real target was replaced")
    expect_published(proof, pool, mount, nonempty, "a nonempty target's members")

    outside = os.path.join(root, "outside")
    os.makedirs(outside)
    with open(os.path.join(outside, "sentinel"), "wb") as handle:
        handle.write(b"foreign")
    _, foreign = ignore_fixture(proof, root, "foreign", "/target\n/.oxide-outputs/\n")
    os.symlink(outside, os.path.join(foreign, "target"))
    code, _, stderr = proof.oxide(["outputs", "adopt"], "a foreign link is kept", foreign)
    expect(code != 0 and "never moves" in stderr, stderr)
    expect(os.readlink(os.path.join(foreign, "target")) == outside, "the foreign link was replaced")
    expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "the referent mutated")

    absent = os.path.join(root, "absent-root")
    _, dangling = ignore_fixture(proof, root, "dangling", "/target\n/.oxide-outputs/\n")
    os.symlink(absent, os.path.join(dangling, "target"))
    code, _, stderr = proof.oxide(["outputs", "adopt"], "a dangling link is kept", dangling)
    expect(code != 0 and "never moves" in stderr, stderr)
    expect(os.readlink(os.path.join(dangling, "target")) == absent, "the dangling link was replaced")
    expect(not os.path.lexists(absent), "the dangling link's root was created")
    proof.phase("after present-target controls")

    # 7. Prepared first-conversion replay rechecks the prerequisite: a rule
    #    removed after the crash refuses to replay and keeps the prepared
    #    receipt; restoring the rule replays and commits.
    _, replay = ignore_fixture(proof, root, "replay", "/target\n/.oxide-outputs/\n")
    crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="prepared")
    code, _, stderr = oxide_env(proof, ["outputs", "adopt"], "crash after the prepared receipt", replay, crash_env)
    expect(code != 0 and "injected transition stop at prepared" in stderr, stderr)
    link = os.path.join(replay, "target", "debug")
    expect(not os.path.lexists(link), "the crash published a member link")
    prepared = read_json(receipt_file(pool, replay))
    expect(prepared["state"] == "prepared", prepared)
    expect(prepared["layout"] == "profile", prepared)
    before_roots = roots_on(mount)
    with open(os.path.join(replay, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write("/.oxide-outputs/\n")
    code, _, stderr = proof.oxide(["outputs", "adopt"], "replay refuses an unignored target", replay)
    expect(code != 0 and "does not ignore" in stderr, stderr)
    expect(not os.path.lexists(link), "the refused replay published a link")
    expect(
        read_json(receipt_file(pool, replay))["state"] == "prepared",
        "the refused replay rewrote the prepared receipt",
    )
    expect(roots_on(mount) == before_roots, "the refused replay created a root")
    with open(os.path.join(replay, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write("/target\n/.oxide-outputs/\n")
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "replay after restoring the rule", replay)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    replay_root = expect_published(proof, pool, mount, replay, "the replayed first conversion")
    expect(os.path.isdir(replay_root), "the replayed root is missing")
    proof.phase("after the replay case")

    # 8. Accepted publications: an automatic first conversion, an explicit one
    #    and a nested workspace all hide the link from Git.
    _, automatic = ignore_fixture(proof, root, "auto-accepted", "/target\n/.oxide-outputs/\n")
    code, _, stderr = proof.oxide(["build"], "automatic adoption publishes an ignored link", automatic)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    expect_published(proof, pool, mount, automatic, "the automatic first conversion")
    expect(
        os.path.isfile(os.path.join(automatic, "target", "debug", "poutputs")),
        "the automatic conversion built nothing through the link",
    )

    _, explicit = ignore_fixture(proof, root, "explicit-accepted", "target\n/.oxide-outputs/\n")
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "explicit adoption with `target`", explicit)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    explicit_root = expect_published(proof, pool, mount, explicit, "the explicit first conversion")

    nested_repo, nested_ok = ignore_fixture(
        proof, root, "nested-accepted", "/rust/target\n.oxide-outputs/\n", relative="rust"
    )
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "nested /rust/target", nested_ok)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    expect_published(proof, pool, mount, nested_ok, "the nested first conversion")
    # The nested check is non-vacuous: a wholly untracked workspace collapses to
    # `rust/` in plain `git status`, but with every untracked file listed and
    # exact NUL-delimited paths, removing the rule exposes `rust/target`.
    nested_ignore = os.path.join(nested_repo, ".gitignore")
    nested_saved = read_bytes(nested_ignore)
    with open(nested_ignore, "wb") as handle:
        handle.write(b"")
    try:
        expected = repo_relative_target(proof, nested_ok)
        exposed = git_status_names_target(proof, nested_ok)
        expect(
            any(entry[3:] == expected for entry in exposed),
            f"the nested detector missed an exposed target: {exposed}",
        )
    finally:
        with open(nested_ignore, "wb") as handle:
            handle.write(nested_saved)
    expect_published(proof, pool, mount, nested_ok, "the nested first conversion restored")
    proof.phase("after accepted publications")

    # 9. Detach still works for an existing owned link.
    set_mode(proof, "ephemeral", explicit)
    code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach an owned ignored link", explicit)
    expect(code == 0 and "real empty directory again" in stdout, stdout + stderr)
    detached = os.path.join(explicit, "target")
    expect(os.path.isdir(detached) and not os.path.islink(detached), "detach left a link")
    for member in ("debug", "release"):
        path = os.path.join(detached, member)
        expect(
            os.path.isdir(path) and not os.path.islink(path),
            f"detach did not restore a real {member} directory",
        )
        expect(os.listdir(path) == [], f"the detached {member} is not empty")
    expect(os.path.isdir(explicit_root), "detach deleted the output root")
    set_mode(proof, "permanent", explicit)
    proof.phase("after detach")
    proof.detach()


# Premise 7 (HIGH checklist): the staged binary and every fixture child build
# under ONE frozen compiler-flag environment. `layout.stage_at` copies
# `os.environ`, so the freeze is applied to the process environment before
# staging and then to the fixture environment, which is built from the same
# values. `CARGO_ENCODED_RUSTFLAGS` is removed everywhere: it takes precedence
# over `RUSTFLAGS`, so an inherited value would unfreeze the "empty" flag set.
FROZEN_RUSTFLAGS = ""


def freeze_compiler_flags(env) -> None:
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["RUSTFLAGS"] = FROZEN_RUSTFLAGS


# Premise 6 (HIGH checklist): every attach and reattach of this proof's own
# image requests filesystem owners. The product registration that follows is
# the second control: it refuses an ignore-ownership mount.
def owners_honored(proof: Proof, mount: str, label: str) -> None:
    """Bounded read-only observation that the private image's mounted root does
    not carry the ignore-ownership flag."""
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


# The fixed exec-inference case table. `--plan` prints exactly this and touches
# nothing; `exec_run` executes the same labels in this order.
EXEC_CASES = [
    ("s4b", "a fresh eligible checkout's first leased build creates the profile layout, an exec with no --output places its output in the root, and an existing whole-target fixture (case 09) stays whole-target"),
    ("01", "explicit adoption publishes the same profile layout"),
    ("02", "exec without --output runs a tool that builds through the owned pair"),
    ("03", "a repeat reuses the same warm slot and relinks nothing"),
    ("04", "the tool's release build is placed under the same root"),
    ("05", "the declared --output override reaches the same keyed slot and placement"),
    ("06", "a member the receipt does not place stays real and local"),
    ("07+14", "exec on a detached checkout runs locally; detach restored real empty members and kept the root"),
    ("08", "an ineligible (empty-target) checkout is never converted"),
    ("09", "an existing whole-target checkout stays whole-target through a build and an exec"),
    ("10", "a foreign member link refuses before Cargo and changes nothing"),
    ("11", "a receipt changed while the command waits refuses"),
    ("12", "root drift refuses and names the explicit adoption"),
    ("13", "a checkout path containing a space converts and runs end to end"),
    ("15", "Ctrl-C reaches the tool group, the lease holds until the group closes, then releases"),
]


def plan_exec_inference() -> None:
    """The effect-free table and allocation, printed by `--plan`."""
    print("exec-inference (oxide#160): S1 derived exec member sets + S4b profile first conversion")
    print(
        "supervisor: python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 "
        "--scratch-budget-mib 512 -- python3 scripts/test-permanent-outputs.py exec-inference"
    )
    print("limits: 300 s wall clock, 512 MiB monitored scratch, one owned 128 MiB APFS image")
    print("        attached with hdiutil -owners on, ordinary detach")
    print("root:   --root-parent /private/tmp, the owner-accepted internal-placement exception")
    print("        (decision bd357a4c): the fixture's internal directory-budget pool, the")
    print("        supervisor scratch, the image backing and fixture temp paths stay inside")
    print("        one monitored 512 MiB root; the image-backed removable pool is registered")
    print("        as the operating pool afterwards")
    print("fixture work, sequential, one Cargo child at a time: 4 Cargo jobs for the single")
    print("        staging and 1 per fixture child; planning demand 4 CPU / 6 GiB")
    print("cases (fail-fast, in this order):")
    for label, text in EXEC_CASES:
        print(f"  {label}: {text}")
    print("nothing was created, mounted, spawned or written")


# Phase labels this script reached, for the uncounted diagnostic's bounded
# receipt. The counted run uses the proof's own phase checks.
PHASES: list[str] = []


def phase(proof: Proof, label: str) -> None:
    PHASES.append(label)
    proof.phase(label)


def write_diagnostic(proof: Proof, label: str, fault: str | None) -> str:
    """The uncounted diagnostic's bounded receipt: `qualified: false`, the
    phases reached and the first fault. It claims no closure and no PASS."""
    receipt = {
        "qualified": False,
        "mode": label,
        "kind": "uncounted diagnostic: first-fault abort, no keep-going",
        "phases": PHASES,
        "first_fault": fault,
        "reached_every_case": fault is None,
        "limits": {
            "scratch_root": os.environ.get("OXIDE_TASK_SCRATCH_ROOT"),
            "work_deadline_ns": os.environ.get("OXIDE_PROOF_DEADLINE_NS"),
            "supervisor_scratch_budget_mib": 512,
        },
        "limitation": (
            "This diagnostic aborts at the first fault. It does not keep going "
            "into dependency-independent cases, so its unchecked cases are "
            "reported as unexecuted, never as passing, and it is never a "
            "counted native PASS."
        ),
    }
    name = f"{label}-diagnostic.json"
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        json.dump(receipt, handle, indent=2)
    return name


EXEC_TOOLS = {
    # A tiny tool that calls the real Cargo. `oxide exec` sets CARGO_TARGET_DIR
    # and the build directory, so the tool's own build is what writes the
    # profile members; Oxide reads no argv from it.
    "tool-build.sh": '#!/bin/sh\nset -eu\nexec cargo build "$@"\n',
    # A tool that writes a member-shaped directory the receipt does not place.
    "tool-staging.sh": (
        '#!/bin/sh\nset -eu\n'
        'mkdir -p target/staging\n'
        'printf local > target/staging/out\n'
    ),
    # A gated tool whose LEADER exits on the forwarded interrupt while a group
    # member that ignores it keeps waiting for the release file, so the lease
    # must stay held until that member closes (case 15).
    "tool-gated.sh": (
        '#!/bin/sh\nset -eu\n'
        'gate="$1"\n'
        'printf started > "$gate.started"\n'
        '(\n'
        "  trap '' INT TERM\n"
        '  while [ ! -f "$gate.release" ]; do sleep 0.05; done\n'
        ') &\n'
        "trap 'exit 0' INT TERM\n"
        'while [ ! -f "$gate.release" ]; do sleep 0.05; done\n'
        'wait\n'
    ),
}


def write_tools(root: str) -> str:
    """The fixture's tool directory, outside every checkout so a tool's own
    file never appears in `git status`."""
    directory = os.path.join(root, "exec-tools")
    os.makedirs(directory, exist_ok=True)
    for name, body in EXEC_TOOLS.items():
        path = os.path.join(directory, name)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(body)
        os.chmod(path, 0o755)
    return directory


def member_link_target(checkout: str, member: str) -> str:
    """The destination one profile member link selects."""
    path = os.path.join(checkout, "target", member)
    expect(os.path.islink(path), f"{path} is not an owned member link")
    return os.readlink(path)


def expect_profile_layout(pool: str, mount: str, checkout: str, label: str) -> str:
    """A committed first conversion in the profile layout: a real `target`, an
    owned `debug`/`release` pair into ONE root on the drive, that root's marker
    and its committed profile receipt."""
    target = os.path.join(checkout, "target")
    expect(
        os.path.isdir(target) and not os.path.islink(target),
        f"{label}: target is not a real directory",
    )
    debug = member_link_target(checkout, "debug")
    release = member_link_target(checkout, "release")
    for member, destination in (("debug", debug), ("release", release)):
        expect(
            destination.startswith(os.path.join(mount, "oxide-outputs") + os.sep),
            f"{label}: {member} selects {destination}, not a root on the drive",
        )
    root = os.path.dirname(debug)
    expect(
        release == os.path.join(root, "release"),
        f"{label}: the pair does not share one root: {debug} {release}",
    )
    expect(os.path.isfile(os.path.join(root, MARKER)), f"{label}: the root has no marker")
    receipt = read_json(receipt_file(pool, checkout))
    expect(
        receipt["layout"] == "profile" and receipt["state"] == "committed",
        f"{label}: the receipt is not a committed profile receipt: {receipt}",
    )
    expect(
        sorted(placement["member"] for placement in receipt["links"]) == ["debug", "release"],
        f"{label}: the receipt places {receipt['links']}",
    )
    return root


def legacy_whole_target(proof: Proof, pool: str, mount: str, checkout: str, label: str) -> str:
    """An EXISTING whole-target checkout, built without hand-computing any
    marker fact: the product's own explicit conversion creates the root and its
    `oxide.outputs.root/2` marker for this checkout, and the fixture then
    records the legacy version-2 whole-target receipt that selects that same
    root and replaces the real `target` with the legacy owned link. The root
    keeps the payload and the marker; only the layout the receipt records is
    the pre-oxide#133 one."""
    code, _, stderr = proof.oxide(
        ["outputs", "adopt"], f"{label}: convert to obtain a real root and marker", checkout
    )
    expect(code == 0, stderr)
    root = expect_profile_layout(pool, mount, checkout, label)
    receipt = read_json(receipt_file(pool, checkout))
    selected = next(
        (placement["selected"] for placement in receipt["links"] if placement.get("selected")),
        None,
    )
    expect(selected is not None, f"{label}: the conversion selected no root")
    target = os.path.join(checkout, "target")
    for member in ("debug", "release"):
        path = os.path.join(target, member)
        if os.path.islink(path):
            os.remove(path)
    os.rmdir(target)
    os.symlink(root, target)
    # The document copies the product's own `checkout` and `checkout_id` bytes,
    # so no fixture-side path or digest derivation can disagree with it.
    legacy = {
        "version": "oxide.outputs.transition/2",
        "checkout": receipt["checkout"],
        "checkout_id": receipt["checkout_id"],
        "generation": receipt["generation"],
        "state": "committed",
        "links": [{"member": "target", "selected": selected}],
        "updated_ms": int(time.time() * 1000),
    }
    with open(receipt_file(pool, checkout), "w", encoding="utf-8") as handle:
        json.dump(legacy, handle, indent=2)
    return root


def exec_run(proof: Proof, repo: str) -> None:
    """Focused exec-output-inference proof (oxide#160): S1 derives an `exec`
    member set from the checkout's committed placement, and S4b makes the
    automatic first conversion create that profile layout.

    Every case is a positive oracle plus its own control: a real derived run
    and the foreign member link that differs only in destination, an
    unrecorded real member and the root bytes it must not reach, a detached
    checkout that runs locally and the root it must not write, an existing
    whole-target checkout (case 09) that must stay whole-target beside the
    profile conversion, and an interrupted tool (case 15) whose surviving group
    member must keep the lease held. `--plan` prints the same table and
    allocation with no effect.

    The HIGH premise checklist is traced through the executable bodies:

    1. private HOME and rustup: `proof_env` captures the invoking `RUSTUP_HOME`
       before replacing `HOME`, pins `RUSTUP_TOOLCHAIN` 1.97.1 and sets
       `RUSTUP_AUTO_INSTALL=0`; every fixture crate is std-only, so the only
       `--locked` call is this repository's staging.
    2. account-home control: `OXIDE_TEST_HOME_FROM_ENV=1` in `proof_env`.
    3. environment isolation: `proof_env` scrubs `OXIDE_*`, `XDG_*`,
       `CARGO_BUILD_*`, `CARGO_TARGET_*`, `GIT_*`, the wrapper variables and
       `CARGO`, then sets only this fixture's values; the pause and crash hooks
       are added per case.
    4. automatic adoption: every case states its receipt premise, and case 08
       positively creates an ineligible (real empty target) checkout and
       asserts the receipt is absent.
    5. Cargo external argv: not applicable. The fixture tools are invoked
       directly (`oxide exec -- <tool>`); none is a Cargo subcommand, so no
       external argv is modelled.
    6. owned images: every attach goes through `attach_image(..., owners=True)`
       and `owners_honored` observes the mount read-only; the product's own
       `pool register-removable` is the second control, since it refuses an
       ignore-ownership mount.
    7. compiler flags: `freeze_compiler_flags` sets one `RUSTFLAGS` (empty) and
       removes `CARGO_ENCODED_RUSTFLAGS` on `os.environ` BEFORE staging and on
       the fixture environment afterwards, so the staged binary and every
       fixture child build under the same flag set.
    8. temporary storage: this fixture's control pool is a genuine internal
       `directory-budget` pool, which the product correctly refuses on an
       external volume, so the selector runs under the owner-accepted internal
       monitored root (`--root-parent /private/tmp`, decision `bd357a4c`,
       Tom-approved 2026-10-26): the internal pool, the supervisor scratch, the
       image backing and the fixture `TMPDIR` all stay inside one monitored
       512 MiB root (asserted as `TMPDIR == OXIDE_TASK_SCRATCH_ROOT`), the
       owned image is attached with `-owners on` and registered as the
       operating removable pool afterwards, and the guard is the qualified
       shared supervisor with its unchanged 300 s limit. The uncounted
       diagnostic at `171ae6fb` failed exactly on the missing exception, which
       is apparatus evidence, not a product defect.
    9. macOS `mktemp`: every fixture temp root is `tempfile.mkdtemp` under
       `OXIDE_TASK_SCRATCH_ROOT` and the gate/release paths are fixed names
       inside it; no fixture or tool calls `mktemp`.
    """
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxEx" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"exec-output-inference proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    phase(proof, "start")
    # Premise 7: staging copies os.environ, so the freeze must precede it.
    freeze_compiler_flags(os.environ)
    proof.binary = layout.stage_at(proof, repo, "oxide-exec-inference")
    phase(proof, "after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    # The same frozen flag set for every fixture child (premise 7).
    freeze_compiler_flags(proof.env)
    expect(proof.env["RUSTFLAGS"] == FROZEN_RUSTFLAGS, "the fixture environment is not frozen")
    expect("CARGO_ENCODED_RUSTFLAGS" not in proof.env, "an encoded rustflags value survives")
    # Premises 8 and 9: the fixture's own temp paths are templates under the
    # supervisor's selected-volume scratch, and no fixture calls `mktemp`
    # outside it.
    expect(proof.env["TMPDIR"] == os.environ["OXIDE_TASK_SCRATCH_ROOT"], "the fixture TMPDIR is not the supervisor scratch")
    tools = write_tools(root)
    build_tool = os.path.join(tools, "tool-build.sh")
    staging_tool = os.path.join(tools, "tool-staging.sh")
    gated_tool = os.path.join(tools, "tool-gated.sh")

    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "internal pool before the drive", maker)
    expect(code == 0, stderr)
    # Premise 6: this proof's own image is attached with filesystem owners on
    # and observed read-only; the product registration that follows is the
    # second control (it refuses an ignore-ownership mount).
    mounted = attach_image(proof, root, name, owners=True)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    owners_honored(proof, mount, "exec-inference attach")
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
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)

    # S4b: a fresh eligible checkout's first leased build creates the profile
    # layout, and an `exec` with no `--output` places its output in that root.
    first = fresh_checkout(proof, root, "first-conversion")
    code, _, stderr = proof.oxide(["build"], "S4b first leased build converts", first)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    first_root = expect_profile_layout(pool, mount, first, "S4b conversion")
    expect(
        os.path.isfile(os.path.join(first, "target", "debug", "poutputs")),
        "S4b: the build wrote no binary through the owned pair",
    )
    expect(
        os.path.isdir(os.path.join(first_root, "release")),
        "S4b: the conversion left the release member directory out",
    )
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "S4b exec after the conversion", first)
    expect(code == 0, stderr)
    expect("through profile member links" in stderr, stderr)
    expect(
        member_link_target(first, "debug") == os.path.join(first_root, "debug"),
        "S4b: the exec relinked debug",
    )
    expect(
        os.path.isfile(os.path.join(first_root, "debug", "poutputs")),
        "S4b: the exec's output is not in the owned root",
    )
    phase(proof, "after the first conversion")

    # 01: an explicit adoption of a fresh checkout publishes the same layout.
    app = fresh_checkout(proof, root, "exec-adopted")
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "01 explicit adoption", app)
    expect(code == 0 and "now selects the owned external root" in stdout, stdout + stderr)
    app_root = expect_profile_layout(pool, mount, app, "01 adoption")
    debug_link = member_link_target(app, "debug")
    release_link = member_link_target(app, "release")
    receipt_before = read_bytes(receipt_file(pool, app))

    # 02: exec with no --output runs a tool that builds through the pair.
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "02 exec without --output", app)
    expect(code == 0, stderr)
    expect(member_link_target(app, "debug") == debug_link, "02: the exec relinked debug")
    expect(member_link_target(app, "release") == release_link, "02: the exec relinked release")
    expect(
        os.path.isfile(os.path.join(app_root, "debug", "poutputs")),
        "02: the tool's output is not in the owned debug member",
    )
    expect("through profile member links (debug, release)" in stderr, stderr)
    slot, _ = slot_state(stderr)

    # 03: a repeat reuses the same warm slot and relinks nothing.
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "03 repeat exec", app)
    expect(code == 0, stderr)
    again, state = slot_state(stderr)
    expect(again == slot and state == "warm", f"03: slot {again} {state}\n{stderr}")
    expect(member_link_target(app, "debug") == debug_link, "03: the repeat relinked debug")

    # 04: the tool's release build is placed under the same root.
    code, _, stderr = proof.oxide(
        ["exec", "--", build_tool, "--release"], "04 exec release build", app
    )
    expect(code == 0, stderr)
    expect(
        os.path.isfile(os.path.join(app_root, "release", "poutputs")),
        "04: the release output is not in the owned release member",
    )

    # 05: the declared override reaches the same keyed slot and placement.
    code, _, stderr = proof.oxide(
        ["exec", "--output", "debug", "--output", "release", "--", build_tool],
        "05 declared parity",
        app,
    )
    expect(code == 0, stderr)
    declared_slot, declared_state = slot_state(stderr)
    expect(
        declared_slot == slot and declared_state == "warm",
        f"05: the declared set did not share the derived key: {declared_slot} "
        f"{declared_state}\n{stderr}",
    )
    expect(member_link_target(app, "debug") == debug_link, "05: the declared override relinked debug")
    expect(read_bytes(receipt_file(pool, app)) == receipt_before, "05: an exec changed the receipt")
    phase(proof, "after the derived and declared runs")

    # 06: a member the receipt does not place stays real and local.
    root_before = tree_digest(app_root)
    receipt_before = read_bytes(receipt_file(pool, app))
    code, _, stderr = proof.oxide(["exec", "--", staging_tool], "06 unrecorded member", app)
    expect(code == 0, stderr)
    staging = os.path.join(app, "target", "staging")
    expect(
        os.path.isdir(staging) and not os.path.islink(staging),
        "06: the unrecorded member is not a real directory",
    )
    expect(read_bytes(os.path.join(staging, "out")) == b"local", "06: the tool's write was lost")
    expect(tree_digest(app_root) == root_before, "06: an unrecorded member reached the owned root")
    expect(
        read_bytes(receipt_file(pool, app)) == receipt_before,
        "06: an unrecorded member changed the receipt",
    )
    phase(proof, "after the unrecorded member")

    # 07 + 14: detach restores real empty members and keeps the root (the
    # shipped detach, reused); an exec on the detached checkout then runs
    # locally and writes nothing into the root.
    code, _, stderr = proof.oxide(["outputs", "detach"], "07 detach", app)
    expect(code == 0, stderr)
    target = os.path.join(app, "target")
    expect(os.path.isdir(target) and not os.path.islink(target), "07: detach left a link")
    for member in ("debug", "release"):
        path = os.path.join(target, member)
        expect(
            os.path.isdir(path) and not os.path.islink(path),
            f"07: {member} is not a real directory after detach",
        )
        expect(os.listdir(path) == [], f"07: {member} is not empty after detach")
    expect(receipt_state(pool, app) == "detached", "07: the receipt is not detached")
    expect(os.path.isdir(app_root), "07: detach deleted the owned root")
    root_before = tree_digest(app_root)
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "07 exec on a detached checkout", app)
    expect(code == 0, stderr)
    expect(
        os.path.isdir(os.path.join(target, "debug"))
        and not os.path.islink(os.path.join(target, "debug")),
        "07: the detached exec linked a member",
    )
    expect(
        os.path.isfile(os.path.join(target, "debug", "poutputs")),
        "07: the detached exec built nothing locally",
    )
    expect(tree_digest(app_root) == root_before, "07: a detached exec wrote into the root")
    phase(proof, "after the detached run")

    # 08: a checkout whose first conversion is ineligible is never converted.
    plain = fresh_checkout(proof, root, "exec-unconverted")
    os.makedirs(os.path.join(plain, "target"))
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "08 unconverted exec", plain)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(
        not os.path.islink(os.path.join(plain, "target")),
        "08: an unconverted target became a link",
    )
    expect(not os.path.exists(receipt_file(pool, plain)), "08: an unconverted checkout got a receipt")

    # 09: an EXISTING whole-target checkout stays whole-target through a build
    # and through an exec. This is also the S4b "existing whole-target fixture
    # is untouched" half of the brief's native case.
    legacy = fresh_checkout(proof, root, "legacy-whole-target")
    legacy_root = legacy_whole_target(proof, pool, mount, legacy, "09 legacy fixture")
    legacy_target = os.path.join(legacy, "target")
    expect(os.path.islink(legacy_target), "09: the fixture's target is not the legacy link")
    legacy_receipt = read_bytes(receipt_file(pool, legacy))
    legacy_report = plan(proof, legacy, "09 the legacy fixture plans the whole target")
    expect(
        legacy_report["layout"] == "whole-target",
        f"09: the product does not read the fixture as a whole-target checkout: {legacy_report}",
    )
    code, _, stderr = proof.oxide(["build"], "09 a build keeps the whole-target link", legacy)
    expect(code == 0, stderr)
    expect(os.path.islink(legacy_target), "09: a build replaced the whole-target link")
    expect(os.readlink(legacy_target) == legacy_root, "09: a build relinked the whole target")
    expect(
        os.path.isfile(os.path.join(legacy_target, "debug", "poutputs")),
        "09: the build wrote no binary through the whole-target link",
    )
    expect(
        read_bytes(receipt_file(pool, legacy)) == legacy_receipt,
        "09: a build rewrote the whole-target receipt",
    )
    code, _, stderr = proof.oxide(
        ["exec", "--", build_tool], "09 an exec keeps the whole-target link", legacy
    )
    expect(code == 0, stderr)
    expect(os.path.islink(legacy_target), "09: an exec replaced the whole-target link")
    expect(
        os.readlink(legacy_target) == legacy_root,
        "09: an exec relinked the whole target",
    )
    expect(
        read_bytes(receipt_file(pool, legacy)) == legacy_receipt,
        "09: an exec rewrote the whole-target receipt",
    )
    phase(proof, "after the legacy whole-target fixture")

    # 13: a checkout path containing a space converts and runs end to end.
    spaced = fresh_checkout(proof, root, "exec spaced")
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "13 spaced path exec", spaced)
    expect(code == 0, stderr)
    spaced_root = expect_profile_layout(pool, mount, spaced, "13 spaced path")
    expect(
        os.path.isfile(os.path.join(spaced_root, "debug", "poutputs")),
        "13: the tool's output is not in the spaced checkout's root",
    )

    # 10: a foreign member link refuses before Cargo and changes nothing.
    foreign = fresh_checkout(proof, root, "exec-foreign")
    code, _, stderr = proof.oxide(["build"], "10 foreign fixture adopts", foreign)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    expect_profile_layout(pool, mount, foreign, "10 adoption")
    outside = os.path.join(root, "outside-exec")
    os.makedirs(outside)
    with open(os.path.join(outside, "sentinel"), "wb") as handle:
        handle.write(b"foreign")
    foreign_debug = member_link_target(foreign, "debug")
    foreign_receipt = read_bytes(receipt_file(pool, foreign))
    os.remove(os.path.join(foreign, "target", "release"))
    os.symlink(outside, os.path.join(foreign, "target", "release"))
    code, _, stderr = proof.oxide(
        ["exec", "--", build_tool], "10 a foreign member link refuses", foreign
    )
    expect(code != 0, f"10: a foreign member link was followed\n{stderr}")
    expect("target/release" in stderr, stderr)
    expect("oxide outputs adopt" in stderr, stderr)
    expect(
        read_bytes(os.path.join(outside, "sentinel")) == b"foreign",
        "10: the foreign referent mutated",
    )
    expect(member_link_target(foreign, "debug") == foreign_debug, "10: a refused exec relinked debug")
    expect(
        read_bytes(receipt_file(pool, foreign)) == foreign_receipt,
        "10: a refused exec changed the receipt",
    )
    phase(proof, "after the foreign member link")

    # 12: root drift (this checkout's own root marker removed) refuses and
    # names the explicit adoption.
    drift = fresh_checkout(proof, root, "exec-drift")
    code, _, stderr = proof.oxide(["build"], "12 drift fixture adopts", drift)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    drift_root = expect_profile_layout(pool, mount, drift, "12 adoption")
    os.remove(os.path.join(drift_root, MARKER))
    drift_debug = member_link_target(drift, "debug")
    code, _, stderr = proof.oxide(["exec", "--", build_tool], "12 root drift refuses", drift)
    expect(code != 0 and "oxide outputs adopt" in stderr, stderr)
    expect(member_link_target(drift, "debug") == drift_debug, "12: a refused exec relinked debug")
    phase(proof, "after root drift")

    # 11: a receipt changed while the command waits refuses rather than
    # building into a placement the receipt no longer selects.
    waiting = fresh_checkout(proof, root, "exec-changed")
    code, _, stderr = proof.oxide(["build"], "11 wait fixture adopts", waiting)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    expect_profile_layout(pool, mount, waiting, "11 adoption")
    gate = os.path.join(root, "exec-pause")
    pause_env = dict(proof.env, OXIDE_TEST_PAUSE="keyed:" + gate)
    child = start_child(proof, [proof.binary, "exec", "--", build_tool], waiting, pause_env)
    try:
        wait_for(f"{gate}.keyed-{child.pid}", "11 the exec command's key pause")
        receipt = read_json(receipt_file(pool, waiting))
        receipt["state"] = "detached"
        for placement in receipt["links"]:
            placement["selected"] = None
            placement.pop("prepared", None)
        with open(receipt_file(pool, waiting), "w", encoding="utf-8") as handle:
            json.dump(receipt, handle, indent=2)
        with open(gate, "wb") as handle:
            handle.write(b"release")
        code, _, stderr = finish_child(proof, child, "11 the waiting exec")
        child = None
    finally:
        if child is not None and child.poll() is None:
            child.kill()
        if not os.path.exists(gate):
            with open(gate, "wb") as handle:
                handle.write(b"release")
    expect(code != 0 and "Run the command again" in stderr, stderr)
    phase(proof, "after the changed receipt")

    # 15: Ctrl-C reaches the tool's group; the lease stays held while a group
    # member outlives the leader, and the slot is released once the group is
    # empty and a later build proceeds.
    interrupted = fresh_checkout(proof, root, "exec-interrupt")
    code, _, stderr = proof.oxide(["build"], "15 interrupt fixture adopts", interrupted)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    expect_profile_layout(pool, mount, interrupted, "15 adoption")
    gate = os.path.join(root, "interrupt-gate")
    exec_child = start_child(
        proof,
        [proof.binary, "exec", "--", gated_tool, gate],
        interrupted,
        proof.env,
    )
    try:
        wait_for(f"{gate}.started", "15 the gated tool started")
        exec_child.send_signal(signal.SIGINT)
        # The leader exits on the forwarded signal; the surviving group member
        # ignores it, so the exec process must still be running and `status`
        # must still name the holder.
        deadline = time.monotonic() + min(40, work_left())
        held = False
        while time.monotonic() < deadline:
            code, stdout, stderr = proof.oxide(
                ["status"], "15 status while the group member lives", interrupted
            )
            expect(code == 0, stderr)
            if "exec " in stdout:
                held = True
                break
            time.sleep(0.2)
        expect(
            held,
            "15: the interrupted exec's lease was not held while its group member lived",
        )
        expect(exec_child.poll() is None, "15: the exec returned before its group closed")
        with open(f"{gate}.release", "wb") as handle:
            handle.write(b"release")
        code, stdout, stderr = finish_child(proof, exec_child, "15 the interrupted exec")
        exec_child = None
    finally:
        if not os.path.exists(f"{gate}.release"):
            with open(f"{gate}.release", "wb") as handle:
                handle.write(b"release")
        if exec_child is not None and exec_child.poll() is None:
            exec_child.kill()
    expect(code != 0, f"15: an interrupted exec reported success: {stdout}")
    code, _, stderr = proof.oxide(["build"], "15 the slot is released after the group closes", interrupted)
    expect(code == 0, stderr)
    phase(proof, "after the interrupt control")

    proof.detach()


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")

def overdue(signum, frame) -> None:
    raise RuntimeError("work deadline reached (SIGALRM fallback)")


def record(name: str, contents: str) -> None:
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        handle.write(contents)


def cleanup_after_failure(proof: Proof) -> str:
    """Ordinary detach of this run's own mount after a failure, through
    Proof.detach's checks: refused while any child is unclosed, never forced,
    and bounded by the proof's own clock. A closed process group is not taken
    as a detached device; the mount path is checked afterwards."""
    # The alarm would cut the cleanup short; the proof clock still bounds it.
    signal.setitimer(signal.ITIMER_REAL, 0)
    image = os.path.join(proof.root, "volume.sparseimage")
    mount = proof.mount
    if not mount:
        return f"no mount attached by this run (image {image})"
    try:
        proof.detach()
    except BaseException as error:
        return f"detach of {mount} (image {image}) not completed: {error!r}"
    if os.path.ismount(mount):
        return f"detach of {mount} (image {image}) returned but the mount is still present"
    return f"ordinary detach of {mount} (image {image}) confirmed; mount no longer present"


def main() -> int:
    # `--plan` is effect-free and needs no supervisor: it prints the fixed
    # exec-inference table and allocation, then returns.
    if "--plan" in sys.argv[1:]:
        expect(
            [argument for argument in sys.argv[1:] if argument != "--plan"] == ["exec-inference"],
            "only the exec-inference mode has a --plan table",
        )
        plan_exec_inference()
        return 0
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
    # Each focused mode is one scenario on the same fixture: `auto-adopt`
    # (oxide#121), `detach-permanent` (oxide#124), `ignore-safety` (oxide#125)
    # and `exec-inference` (oxide#160). No argument runs the full #117
    # scenario. `--diagnostic` appends the uncounted, first-fault collection to
    # `exec-inference`; it is never a counted native PASS.
    arguments = list(sys.argv[1:])
    diagnostic = "--diagnostic" in arguments
    if diagnostic:
        arguments.remove("--diagnostic")
        expect(
            tuple(arguments) == ("exec-inference",),
            "only the exec-inference mode has an uncounted diagnostic",
        )
    modes = {
        (): (run, "permanent-outputs"),
        ("auto-adopt",): (auto_run, "auto-adopt"),
        ("detach-permanent",): (detach_run, "detach-permanent"),
        ("ignore-safety",): (ignore_run, "ignore-safety"),
        ("exec-inference",): (exec_run, "exec-inference"),
    }
    expect(tuple(arguments) in modes, f"unknown mode {arguments!r}")
    scenario, label = modes[tuple(arguments)]
    root = tempfile.mkdtemp(prefix="permanent-outputs.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    if diagnostic:
        try:
            scenario(proof, repo)
        except BaseException as failure:
            receipt = write_diagnostic(proof, label, repr(failure))
            cleanup = cleanup_after_failure(proof)
            record(
                "result.txt",
                f"diagnostic failed (uncounted): {failure!r}\n"
                f"bounded receipt: {receipt}\nunclosed children: {sorted(proof.unclosed)}\n"
                f"root: {root}\ncleanup: {cleanup}\n",
            )
            print(
                f"{label} uncounted diagnostic stopped at the first fault; "
                f"bounded receipt {receipt} in the evidence directory; "
                f"scratch {root} left to the shared supervisor",
                file=sys.stderr,
            )
            raise
        receipt = write_diagnostic(proof, label, None)
        elapsed = time.monotonic() - layout.runner.STARTED
        line = (
            f"{label} uncounted diagnostic reached every case ({elapsed:.0f} s); "
            f"bounded receipt {receipt}; qualified: false - never a counted PASS"
        )
        record("result.txt", f"{line}\n")
        print(line)
        return 0
    try:
        scenario(proof, repo)
    except BaseException as failure:
        cleanup = cleanup_after_failure(proof)
        record(
            "result.txt",
            f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\n"
            f"root: {root}\ncleanup: {cleanup}\n",
        )
        print(
            f"{label} proof failed; scratch {root} left to the shared supervisor",
            file=sys.stderr,
        )
        # The first fault is re-raised; a cleanup failure is only recorded.
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    elapsed = time.monotonic() - layout.runner.STARTED
    summary_line = (
        f"{label} proof held: {elapsed:.0f} s script time, peak owned "
        f"{proof.peak // MIB} MiB at phase checks, phase-sampled host free minimum "
        f"{proof.low // MIB} MiB; one owned 128 MiB scratch image, ordinary detach, "
        f"shared supervisor owns cleanup"
    )
    record("result.txt", f"passed\n{summary_line}\n")
    print(summary_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
