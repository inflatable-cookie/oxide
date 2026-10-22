#!/usr/bin/env python3
"""Focused removable drive-mode proof for oxide#116.

The qualified shared supervisor owns the 300 s deadline, 512 MiB monitored
scratch, process closure and cleanup. This script reuses the reviewed oxide#104
staging/fixture helper and writes only inside the supervisor-owned scratch. It
uses private registration metadata plus a unique absent /Volumes path; it
mounts no drive and never changes checkout-output placement.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util
import json
import os
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

APP = {
    "Cargo.toml": '[package]\nname = "drive-mode-app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
}

ACTIVE_APP = {
    "Cargo.toml": '[package]\nname = "drive-mode-active"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n',
    "build.rs": r'''use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DRIVE_MODE_GATE");
    let gate = PathBuf::from(std::env::var_os("DRIVE_MODE_GATE").unwrap());
    fs::create_dir_all(&gate).unwrap();
    fs::write(gate.join("started"), b"started").unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while !gate.join("release").exists() {
        assert!(Instant::now() < deadline, "proof did not release the build");
        std::thread::sleep(Duration::from_millis(25));
    }
}
''',
    "src/lib.rs": "pub fn value() -> u32 {\n    11\n}\n",
}


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


def v1_registration(mount: str) -> bytes:
    return (
        json.dumps(
            {
                "version": 1,
                "mount_point": mount,
                "volume_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
                "container_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
                "reserve_bytes": MIB,
                "cap_bytes": MIB,
                "quota_bytes": None,
                "capacity_bytes": None,
                "registered_ms": 17,
            },
            indent=2,
        )
        + "\n"
    ).encode()


def write_registration(path: str, contents: bytes) -> None:
    with open(path, "wb") as handle:
        handle.write(contents)
    os.chmod(path, 0o600)


def selected_line(stdout: str) -> str:
    lines = [
        line
        for line in stdout.splitlines()
        if line.startswith("selected for the next build: ")
    ]
    expect(len(lines) == 1, f"status selection lines: {lines}")
    return lines[0]


def removable_line(stdout: str) -> str:
    lines = [line for line in stdout.splitlines() if line.startswith("removable pool: ")]
    expect(len(lines) == 1, f"status removable lines: {lines}")
    return lines[0]


def start_active(proof: Proof, cwd: str, env: dict) -> subprocess.Popen:
    child = subprocess.Popen(
        [proof.binary, "check"],
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    proof.unclosed.add(child.pid)
    return child


def finish_active(proof: Proof, child: subprocess.Popen, label: str) -> tuple[int, str, str]:
    try:
        out, err = child.communicate(timeout=min(45, work_left()))
    except BaseException:
        child.kill()
        try:
            child.communicate(timeout=5)
            proof.unclosed.discard(child.pid)
        except BaseException as error:
            print(f"active child {child.pid} not reaped: {error!r}", file=sys.stderr)
        raise
    proof.unclosed.discard(child.pid)
    stdout, stderr = out.decode(errors="replace"), err.decode(errors="replace")
    print(f"----- {label}: exit {child.returncode} -----\n{stdout}{stderr}", flush=True)
    return child.returncode, stdout, stderr


def wait_for(path: str, label: str) -> None:
    deadline = time.monotonic() + min(45, work_left())
    while not os.path.exists(path):
        if time.monotonic() >= deadline:
            fail(f"{label}: {path} did not appear")
        time.sleep(0.05)


def run(proof: Proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    mount = f"/Volumes/OxDm{secrets.token_hex(5)}"
    expect(not os.path.lexists(mount), f"{mount} exists before proof; never touched")
    print(f"drive-mode proof {os.getpid()} source {source} ({layout.source_facts(repo)}) mount {mount}")
    proof.phase("start")

    candidate = layout.stage_at(proof, repo, "oxide-drive-mode")
    proof.binary = candidate
    proof.phase("after staging")

    pool = os.path.join(proof.root, "pool")
    home = os.path.join(proof.root, "home")
    cargo_home = os.path.join(proof.root, "cargo-home")
    gate = os.path.join(proof.root, "active-gate")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for key in list(env):
        if key in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS"):
            continue
        if key.startswith(("SCCACHE_", "OXIDE_", "XDG_", "CARGO_BUILD_", "CARGO_TARGET_")) or key in (
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
            "DRIVE_MODE_GATE": gate,
        }
    )
    proof.env = env

    app = write_tree(os.path.join(proof.root, "app"), APP)
    active_app = write_tree(os.path.join(proof.root, "active-app"), ACTIVE_APP)
    code, _, stderr = proof.oxide(["pool", "set-mode", "permanent"], "set-mode with no pool", app)
    expect(code != 0 and "no pool exists" in stderr, f"missing pool was not refused: {stderr}")
    expect(not os.path.exists(pool), "set-mode created a missing pool")

    code, _, stderr = proof.oxide(["check"], "create private internal pool", app)
    expect(code == 0 and "slot 1 cold" in summary(stderr), f"initial build failed: {stderr}")
    control = os.path.join(pool, "oxide-managed")
    registration_path = os.path.join(control, "removable.json")
    code, _, stderr = proof.oxide(
        ["pool", "set-mode", "permanent"], "set-mode without registration", app
    )
    expect(code != 0 and "no removable pool is registered" in stderr, stderr)
    expect(not os.path.exists(registration_path), "set-mode created a missing registration")
    legacy = v1_registration(mount)
    write_registration(registration_path, legacy)
    proof.phase("after pool and v1 registration")

    code, before_stdout, before_stderr = proof.oxide(["status"], "legacy status and fallback", app)
    expect(code == 0, f"legacy status failed: {before_stderr}")
    expect("(mode: ephemeral)" in removable_line(before_stdout), "v1 mode did not default to ephemeral")
    baseline_selection = selected_line(before_stdout)
    expect(baseline_selection.startswith("selected for the next build: internal pool (removable volume not used:"), baseline_selection)
    expect(fs_read(registration_path) == legacy, "status migrated the v1 registration")
    expect(not os.path.lexists(mount), "status created the absent mount point")

    code, stdout, stderr = proof.oxide(["pool", "set-mode", "permanent"], "upgrade absent-drive mode", app)
    expect(code == 0 and "mode set to permanent" in stdout, f"permanent update failed: {stdout}{stderr}")
    current = json.loads(fs_read(registration_path))
    expect(current["version"] == 2 and current["mode"] == "permanent", f"not a v2 permanent registration: {current}")
    legacy_fields = json.loads(legacy)
    legacy_fields.pop("version")
    for key, value in legacy_fields.items():
        expect(
            current[key] == value,
            f"mode update changed registration field {key}: {current[key]} != {value}",
        )
    expect(not os.path.lexists(mount), "set-mode touched the absent mount point")

    code, permanent_stdout, permanent_stderr = proof.oxide(["status"], "permanent status", app)
    expect(code == 0, f"permanent status failed: {permanent_stderr}")
    expect("(mode: permanent)" in removable_line(permanent_stdout), "status omitted permanent mode")
    expect(selected_line(permanent_stdout) == baseline_selection, "mode changed the absent-volume selection or reason")

    code, _, stderr = proof.oxide(
        ["exec", "--", os.path.join(proof.root, "does-not-exist")],
        "permanent warning before no-summary failure",
        app,
    )
    expect(code == 127, f"missing exec code was {code}: {stderr}")
    expect("WARNING: permanent removable mode fell back to the internal pool" in stderr, stderr)
    expect(baseline_selection.split("removable volume not used: ", 1)[1].rstrip(")") in stderr, stderr)
    expect(" profile, " not in stderr, "the no-summary path fabricated a summary")

    code, _, stderr = proof.oxide(["check"], "permanent fallback success", app)
    expect(code == 0, f"permanent fallback build failed: {stderr}")
    reason = baseline_selection.split("removable volume not used: ", 1)[1].rstrip(")")
    expect(f"WARNING: permanent removable mode fell back to the internal pool ({reason})" in stderr, stderr)
    expect(f"permanent removable fallback to internal pool: {reason}" in summary(stderr), stderr)
    expect(not os.path.lexists(mount), "permanent fallback created the absent mount point")

    code, stdout, stderr = proof.oxide(["pool", "set-mode", "ephemeral"], "restore ephemeral mode", app)
    expect(code == 0 and "mode set to ephemeral" in stdout, f"ephemeral update failed: {stdout}{stderr}")
    code, _, stderr = proof.oxide(["check"], "ephemeral fallback unchanged", app)
    expect(code == 0, f"ephemeral fallback build failed: {stderr}")
    expect("oxide: internal pool: removable volume not used" in stderr, stderr)
    expect("WARNING: permanent removable mode" not in stderr, stderr)
    expect("permanent removable fallback" not in summary(stderr), stderr)

    active_env = dict(env)
    active_env["DRIVE_MODE_GATE"] = gate
    child = start_active(proof, active_app, active_env)
    try:
        wait_for(os.path.join(gate, "started"), "active build script")
        expect(child.poll() is None, "active build exited before mode update")
        code, status_before, stderr = proof.oxide(["status"], "status during active command", active_app)
        expect(code == 0, f"active status failed: {stderr}")
        expect(any("drive-mode-active" in line and "held by" in line for line in status_before.splitlines()), status_before)

        code, stdout, stderr = proof.oxide(
            ["pool", "set-mode", "permanent"], "change mode during active command", active_app
        )
        expect(code == 0 and "mode set to permanent" in stdout, f"active mode update failed: {stdout}{stderr}")
        expect(child.poll() is None, "mode update ended the active command")
        code, status_after, stderr = proof.oxide(["status"], "status after active mode update", active_app)
        expect(code == 0, f"post-update active status failed: {stderr}")
        expect(any("drive-mode-active" in line and "held by" in line for line in status_after.splitlines()), status_after)
    finally:
        os.makedirs(gate, exist_ok=True)
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        active_code, _, active_stderr = finish_active(proof, child, "active build after mode update")
    expect(active_code == 0, f"active build failed after metadata update: {active_stderr}")
    expect("oxide: internal pool: removable volume not used" in active_stderr, active_stderr)
    expect("WARNING: permanent removable mode" not in active_stderr, active_stderr)

    code, stdout, stderr = proof.oxide(["pool", "set-mode", "permanent"], "permanent for later commands", app)
    expect(code == 0, f"later permanent update failed: {stdout}{stderr}")
    code, _, stderr = proof.oxide(
        ["exec", "--", os.path.join(proof.root, "still-missing")],
        "later command observes permanent mode",
        app,
    )
    expect(code == 127 and "WARNING: permanent removable mode" in stderr, stderr)
    expect(not os.path.lexists(mount), "proof created its absent mount point")
    proof.phase("after diagnostics and active lease")


def fs_read(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")


def overdue(signum, frame) -> None:
    raise RuntimeError("work deadline reached (SIGALRM fallback)")


def record(name: str, contents: str) -> None:
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        handle.write(contents)


def main() -> int:
    if sys.platform != "darwin":
        fail("the drive-mode proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
        and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified shared proof supervisor",
    )
    root = tempfile.mkdtemp(prefix="drive-mode.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    try:
        run(proof, repo)
    except BaseException as failure:
        record("result.txt", f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\nroot: {root}\n")
        print(f"drive-mode proof failed; scratch {root} left to the shared supervisor", file=sys.stderr)
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    proof.phase("end")
    elapsed = time.monotonic() - layout.runner.STARTED
    summary_line = (
        f"drive-mode proof held: {elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
        f"phase-sampled host free minimum {proof.low // MIB} MiB; no mounted volume, shared supervisor owns cleanup"
    )
    record("result.txt", f"passed\n{summary_line}\n")
    print(summary_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
