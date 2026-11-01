#!/usr/bin/env python3
"""Stage a merge-base oxide and run cold_seed_default_off's ignored identity tests.

A short fixed script under the shared proof supervisor. It reuses the reviewed
oxide#104 `stage_at` helper and guarded child runner; it adds no supervision
of its own. Default baseline is `git merge-base HEAD origin/main`; override
with `OXIDE_SEED_BASELINE_COMMIT` or one commit argument. Stages that source
offline, `--locked`, with 1.97.1 into owned scratch, exports
`OXIDE_SEED_BASELINE_BINARY`, and runs `tests/cold_seed_default_off.rs` with
`--ignored`. Not a native proof and not an alias for full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util
import json
import subprocess
import os
import re
import signal
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "pool_layout", os.path.join(HERE, "test-oxide-pool-layout.py")
)
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)
Proof = layout.Proof
expect, fail, MIB = layout.expect, layout.fail, layout.MIB
stage_at, sha256 = layout.stage_at, layout.sha256

# Premise dispositions: (1) rustup is captured before fixture HOME replacement;
# the Rust cases create private HOME/CARGO_HOME and the checked-in lock is used.
# (2) Fixture children honour private HOME via OXIDE_TEST_HOME_FROM_ENV.
# (3) Host OXIDE, XDG, Cargo build, Git and compiler controls are scrubbed.
# (4) The identity cases do not assert an unconverted checkout.
# (5) No Cargo external subcommand is used.
# (6) No image is attached or detached.
# (7) Both stage and fixture use empty encoded and ordinary rust flags.
# (8) TMPDIR and fixture pools are under the supervisor's selected-volume root.
# (9) Python mkdtemp and the Rust mkdtemp both name their owned parent explicitly.
BUILDER = "1.97.1"
S3 = "79e55f4"
POOL_CAP = "128MiB"
SHUTDOWN_MARGIN_S = 30
TESTS = (
    "status_off_matches_ignored_opt_in_and_held_adds_only_its_notice",
    "tiny_build_matches_staged_base_when_seed_is_off",
)


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the runner's shutdown margin")
    return left


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")


def overdue(signum, frame) -> None:
    raise RuntimeError("work deadline reached (SIGALRM fallback)")


def record(name: str, contents: str) -> None:
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        handle.write(contents)


def given_commit() -> str:
    args = [arg for arg in sys.argv[1:] if arg != "--"]
    if args and args[0].startswith("-"):
        fail(f"unknown argument {args[0]!r}")
    if len(args) > 1:
        fail("expected at most one commit")
    env_commit = (os.environ.get("OXIDE_SEED_BASELINE_COMMIT") or "").strip()
    arg_commit = args[0].strip() if args else ""
    return env_commit or arg_commit


def resolve_baseline(proof, repo: str) -> str:
    given = given_commit()
    if given:
        parsed = proof.run(
            ["git", "rev-parse", "--verify", "--end-of-options", f"{given}^{{commit}}"],
            cwd=repo,
            timeout=30,
        )
        expect(parsed.returncode == 0, f"OXIDE_SEED_BASELINE_COMMIT {given!r} is not a commit: {parsed.stderr!r}")
        commit = parsed.stdout.decode().strip()
    else:
        parsed = proof.run(["git", "merge-base", "HEAD", "origin/main"], cwd=repo, timeout=30)
        expect(parsed.returncode == 0, f"default merge-base with origin/main failed: {parsed.stderr!r}")
        commit = parsed.stdout.decode().strip()
    expect(len(commit) == 40 and set(commit) <= set("0123456789abcdef"), f"baseline commit is not a SHA: {commit!r}")
    ancestry = proof.run(["git", "merge-base", "--is-ancestor", S3, commit], cwd=repo, timeout=30)
    expect(ancestry.returncode == 0, f"baseline {commit} must contain scratch-pools S3 ({S3})")
    return commit


def archive_baseline(proof: Proof, repo: str, commit: str, dest: str) -> None:
    os.makedirs(dest)
    archive = proof.run(
        ["/bin/sh", "-c", 'git -C "$0" archive "$2" | tar -x -C "$1"', repo, dest, commit],
        timeout=60,
    )
    expect(archive.returncode == 0, f"git archive {commit} failed: {archive.stderr!r}")
    expect(os.path.isfile(os.path.join(dest, "Cargo.lock")), f"archived {commit} has no Cargo.lock")


def cargo_test_env(root: str, baseline: str) -> dict:
    env = dict(os.environ)
    for name in list(env):
        if name.startswith(("SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_")) or name in (
            "CARGO",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
        ):
            env.pop(name)
    rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    internal = os.path.join(root, "internal")
    os.makedirs(internal, mode=0o700)
    env.update(
        {
            "OXIDE_SEED_BASELINE_BINARY": baseline,
            "OXIDE_SCRATCH_ROOT": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "OXIDE_POOL_CAP": POOL_CAP,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "OXIDE_TEST_INTERNAL_ROOT": internal,
            "RUSTUP_HOME": rustup,
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(root, "test-target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(root, "test-build"),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "CARGO_INCREMENTAL": "0",
        }
    )
    return env


def expect_both_passed(stdout: str, stderr: str) -> None:
    combined = stdout + stderr
    missing = [
        name
        for name in TESTS
        if not re.search(rf"^test {re.escape(name)} \.\.\. ok$", combined, re.M)
    ]
    expect(not missing, f"ignored tests did not pass: {missing}\n{combined[-4000:]}")
    expect("test result: ok." in combined, f"cargo test did not report ok:\n{combined[-4000:]}")


def run(proof: Proof, repo: str, commit: str, diagnostic: bool) -> str:
    print(f"seed-default-off baseline {commit} builder {BUILDER}")
    proof.phase("start")
    base_src = os.path.join(proof.root, "base-src")
    archive_baseline(proof, repo, commit, base_src)
    print(f"archived {commit} ({layout.source_facts(base_src)})")
    baseline = stage_at(proof, base_src, "oxide-seed-baseline")
    expect(os.path.realpath(baseline).startswith(proof.root + "/"), "staged baseline is outside owned scratch")
    expect(os.path.isfile(baseline), f"staged baseline is not a file: {baseline}")
    print(f"staged baseline sha256 {sha256(baseline)}")
    proof.phase("after staging")
    env = cargo_test_env(proof.root, baseline)
    outcomes = []
    # Each ignored test has its own scratch fixture. In diagnostic mode a
    # completed failing case does not prevent the other independent case.
    for name in TESTS if diagnostic else (None,):
        command = ["cargo", "test", "--locked", "--offline", "--test", "cold_seed_default_off", "--"]
        if name:
            command.append(name)
        command += ["--ignored", "--nocapture"]
        result = proof.run(command, cwd=repo, env=env, timeout=work_left())
        stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
        print(f"----- cargo test {name or 'all'}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
        passed = result.returncode == 0 and all(
            re.search(rf"^test {re.escape(test)} \.\.\. ok$", stdout + stderr, re.M)
            for test in ((name,) if name else TESTS)
        ) and "test result: ok." in stdout + stderr
        outcomes.append({"case": name or "all", "passed": bool(passed), "exit": result.returncode})
        if not diagnostic:
            expect(passed, "ignored identity tests failed")
    if diagnostic:
        record("diagnostic.json", json.dumps({"qualified": False, "cases": outcomes}) + "\n")
        fail("diagnostic is uncounted; see diagnostic.json")
    proof.phase("after tests")
    return commit


def self_check() -> None:
    from unittest.mock import patch
    class Git:
        def __init__(self, accepted):
            self.accepted = accepted
            self.commands = []
        def run(self, command, *, cwd, timeout):
            self.commands.append(command)
            if "--is-ancestor" in command:
                return subprocess.CompletedProcess(command, 0 if self.accepted else 1, b"", b"")
            return subprocess.CompletedProcess(command, 0, b"a" * 40 + b"\n", b"")
    with patch.object(sys, "argv", ["proof", "baseline"]), patch.dict(os.environ, {}, clear=True):
        old = Git(False)
        try:
            resolve_baseline(old, "/repo")
        except RuntimeError as error:
            assert "must contain scratch-pools S3" in str(error)
        else:
            raise AssertionError("pre-S3 baseline was accepted")
        assert len(old.commands) == 2 and old.commands[-1][-2:] == [S3, "a" * 40]
        current = Git(True)
        assert resolve_baseline(current, "/repo") == "a" * 40
    with patch.dict(os.environ, {"OXIDE_TASK_SCRATCH_ROOT": "/Volumes/Builds/owned"}, clear=True), \
         patch.object(os, "makedirs") as mkdir:
        env = cargo_test_env("/Volumes/Builds/owned/case", "/baseline")
        assert env["OXIDE_SCRATCH_ROOT"] == "/Volumes/Builds/owned"
        assert env["OXIDE_POOL_CAP"] == POOL_CAP and env["OXIDE_TEST_HOME_FROM_ENV"] == "1"
        assert env["OXIDE_TEST_INTERNAL_ROOT"].startswith(env["OXIDE_SCRATCH_ROOT"] + "/")
        mkdir.assert_called_once()
    print("seed-default-off effect-free contract checks passed")


def main() -> int:
    if sys.argv[1:] == ["--plan"]:
        print("seed-default-off: S3 ancestry gate; selected-volume scratch pool; 300 s / 512 MiB; "
              "diagnostic: two independent ignored cases, uncounted")
        return 0
    if sys.argv[1:] == ["--self-check"]:
        self_check()
        return 0
    if sys.platform != "darwin":
        fail("the seed-default-off selector requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the runner's scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the shared proof supervisor",
    )
    diagnostic = sys.argv[1:2] == ["--diagnostic"]
    if diagnostic:
        del sys.argv[1]
    repo = os.path.dirname(HERE)
    # Resolve and reject incompatible baselines before creating a fixture root
    # or launching any build. Git reads only repository metadata.
    class GitReader:
        def run(self, command, *, cwd, timeout):
            return subprocess.run(command, cwd=cwd, timeout=timeout, capture_output=True)
    commit = resolve_baseline(GitReader(), repo)
    rustup = os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for name in list(os.environ):
        if name.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "GIT_")) and name not in (
            "OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS",
        ):
            os.environ.pop(name)
    os.environ.update({"RUSTUP_HOME": rustup, "RUSTFLAGS": "", "CARGO_ENCODED_RUSTFLAGS": ""})
    root = tempfile.mkdtemp(prefix="seed-default-off.", dir=scratch)
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    try:
        commit = run(proof, repo, commit, diagnostic)
    except BaseException as failure:
        if diagnostic:
            receipt = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "diagnostic.json")
            if not os.path.exists(receipt):
                record("diagnostic.json", json.dumps({"qualified": False, "cases": [],
                    "skipped": list(TESTS), "failure": str(failure)[:512]}) + "\n")
        record(
            "result.txt",
            f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\nroot: {root}\n",
        )
        print(f"seed-default-off failed; scratch {root} left to the shared supervisor", file=sys.stderr)
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    proof.phase("end")
    elapsed = time.monotonic() - layout.runner.STARTED
    summary_line = (
        f"seed-default-off: baseline {commit}, both ignored tests passed in {elapsed:.0f} s, "
        f"peak owned {proof.peak // MIB} MiB; scratch closure and cleanup are the shared runner's"
    )
    record("result.txt", f"passed\n{summary_line}\n")
    print(summary_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
