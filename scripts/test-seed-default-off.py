#!/usr/bin/env python3
"""Stage a merge-base oxide and run cold_seed_default_off's ignored identity tests.

A short fixed script under the shared proof supervisor. It reuses the reviewed
oxide#104 `stage_at` helper and guarded child runner; it adds no supervision
of its own. Default baseline is `git merge-base HEAD origin/main`; override
with `OXIDE_SEED_BASELINE_COMMIT` or one commit argument. Stages that source
offline, `--locked`, with 1.97.1 into owned scratch, exports
`OXIDE_SEED_BASELINE_BINARY`, and runs `tests/cold_seed_default_off.rs` with
`--ignored`. The baseline must contain S3 (79e55f4). `--plan` and
`--self-check` are effect-free; `--diagnostic` runs independent cases in an
uncounted collection. A /private/tmp fallback exercises the inert internal
path and is not scratch-pool coverage. Not an alias for full QA.
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

# HIGH premises, with executable locations:
# 1. main captures invoking RUSTUP_HOME and CARGO_HOME before scrubbing;
#    stage_at uses the checked-in archived lock; the Rust cases create private
#    homes and use offline fixtures (tests/cold_seed_default_off.rs:48,106).
# 2. cargo_test_env sets OXIDE_TEST_HOME_FROM_ENV for those fixture children.
# 3. main scrubs host OXIDE/XDG/CARGO/GIT; cargo_test_env clears wrappers.
# 4. The two tests in tests/cold_seed_default_off.rs assert identity, not an
#    unconverted checkout or adoption receipt.
# 5. run invokes Cargo test, no external Cargo subcommand.
# 6. Neither run nor the two Rust cases attach images.
# 7. main freezes both RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS for all children.
# 8. main creates the private scratch-pools child; cargo_test_env directs
#    fixture TMPDIR there, under the supervisor's monitored root.
# 9. main uses tempfile.mkdtemp(dir=scratch); Scratch::new in
#    tests/cold_seed_default_off.rs uses mkdtemp with an explicit template.
BUILDER = "1.97.1"
S3 = "79e55f4"
POOL_CAP = "128MiB"
SHUTDOWN_MARGIN_S = 30
TESTS = (
    "status_qualified_modes_match_baseline_without_side_effects",
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


def cargo_test_env(root: str, baseline: str, pool_root: str) -> dict:
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
    env.update(
        {
            "OXIDE_SEED_BASELINE_BINARY": baseline,
            "OXIDE_SCRATCH_ROOT": pool_root,
            "TMPDIR": pool_root,
            "TMP": pool_root,
            "TEMP": pool_root,
            "OXIDE_POOL_CAP": POOL_CAP,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
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


def run(proof: Proof, repo: str, commit: str, diagnostic: bool, pool_root: str) -> str:
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
    env = cargo_test_env(proof.root, baseline, pool_root)
    execute_tests(proof, repo, env, diagnostic)
    proof.phase("after tests")
    return commit


def execute_tests(proof, repo: str, env: dict, diagnostic: bool) -> None:
    outcomes = []
    def save(failure=None):
        if diagnostic:
            receipt = {"qualified": False, "cases": outcomes,
                       "skipped": list(TESTS[len(outcomes):])}
            if failure is not None:
                receipt["failure"] = str(failure)[:512]
            record("diagnostic.json", json.dumps(receipt) + "\n")
    save()
    # Each ignored test has its own scratch fixture. In diagnostic mode a
    # completed failing case does not prevent the other independent case.
    for name in TESTS if diagnostic else (None,):
        command = ["cargo", "test", "--locked", "--offline", "--test", "cold_seed_default_off", "--"]
        if name:
            command.append(name)
        command += ["--ignored", "--nocapture"]
        try:
            result = proof.run(command, cwd=repo, env=env, timeout=work_left())
        except BaseException as error:
            save(error)
            raise
        stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
        print(f"----- cargo test {name or 'all'}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
        passed = result.returncode == 0 and all(
            re.search(rf"^test {re.escape(test)} \.\.\. ok$", stdout + stderr, re.M)
            for test in ((name,) if name else TESTS)
        ) and "test result: ok." in stdout + stderr
        outcomes.append({"case": name or "all", "passed": bool(passed), "exit": result.returncode})
        save()
        if result.returncode < 0:
            fail(f"test child terminated by signal {-result.returncode}; remaining cases skipped")
        if not diagnostic:
            expect(passed, "ignored identity tests failed")
    if diagnostic:
        fail("diagnostic is uncounted; see diagnostic.json")


def self_check() -> None:
    from unittest.mock import patch
    with open(os.path.join(os.path.dirname(HERE), "effigy.toml"), encoding="utf-8") as handle:
        manifest = handle.read()
    command = "python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-seed-default-off.py"
    for name, expected in (("rust:seed-default-off", command),
                           ("rust:seed-default-off:diagnostic", command + " --diagnostic")):
        match = re.search(r'^\[tasks\."' + re.escape(name) + r'"\]\n([^\[]+)', manifest, re.M)
        assert match and 'admission = "heavy"' in match.group(1)
        assert f'run = "{expected}"' in match.group(1)
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
    # Exercise the actual entry point, with git and every effectful boundary
    # mocked. Both override and default paths must refuse before fixture or
    # build child creation; the accepted path must reach fixture creation.
    guard = {"OXIDE_TASK_SCRATCH_ROOT": "/Volumes/Builds/owned",
             "OXIDE_PROOF_EVIDENCE": "/evidence", "OXIDE_PROOF_DEADLINE_NS": "1",
             "TMPDIR": "/Volumes/Builds/owned", "TMP": "/Volumes/Builds/owned",
             "TEMP": "/Volumes/Builds/owned", "CARGO_HOME": "/invoking/cargo"}
    for argv, accepted in ((["proof", "baseline"], False), (["proof"], False), (["proof"], True)):
        git = Git(accepted)
        def git_run(command, *, cwd, timeout, capture_output):
            return git.run(command, cwd=cwd, timeout=timeout)
        with patch.object(sys, "argv", argv), patch.object(sys, "platform", "darwin"), \
             patch.dict(os.environ, guard, clear=True), \
             patch.object(os.path, "realpath", side_effect=lambda path: path), \
             patch.object(os.path, "isdir", return_value=True), \
             patch.object(subprocess, "run", side_effect=git_run), \
             patch.object(subprocess, "Popen") as popen, \
             patch.object(tempfile, "mkdtemp", side_effect=RuntimeError("fixture boundary")) as mkdtemp, \
             patch.object(os, "mkdir") as mkdir, patch.object(globals()["Proof"], "run") as child, \
             patch.dict(globals(), {"stage_at": lambda *args: (_ for _ in ()).throw(AssertionError("stage"))}):
            try:
                main()
            except RuntimeError as error:
                assert ("fixture boundary" if accepted else "must contain scratch-pools S3") in str(error)
            else:
                raise AssertionError("entry point did not stop")
            assert mkdtemp.call_count == (1 if accepted else 0)
            mkdir.assert_not_called()
            popen.assert_not_called()
            child.assert_not_called()
            assert git.commands[0][1:3] == (["rev-parse", "--verify"] if len(argv) > 1 else ["merge-base", "HEAD"])
            if accepted:
                assert os.environ["CARGO_HOME"] == "/invoking/cargo"
                assert "OXIDE_TEST_INTERNAL_ROOT" not in os.environ
    from types import SimpleNamespace
    git = Git(True)
    with patch.object(sys, "argv", ["proof"]), patch.object(sys, "platform", "darwin"), \
         patch.dict(os.environ, guard, clear=True), \
         patch.object(os.path, "realpath", side_effect=lambda path: path), \
         patch.object(os.path, "isdir", return_value=True), \
         patch.object(os, "stat", return_value=SimpleNamespace(st_dev=7)), \
         patch.object(subprocess, "run", side_effect=lambda command, cwd, timeout, capture_output:
                      git.run(command, cwd=cwd, timeout=timeout)), \
         patch.object(tempfile, "mkdtemp", return_value="/Volumes/Builds/owned/case") as mkdtemp, \
         patch.object(os, "mkdir") as mkdir, \
         patch.object(signal, "signal", side_effect=RuntimeError("setup boundary")):
        try:
            main()
        except RuntimeError as error:
            assert str(error) == "setup boundary"
        else:
            raise AssertionError("setup boundary not reached")
        mkdtemp.assert_called_once_with(prefix="seed-default-off.", dir=guard["TMPDIR"])
        mkdir.assert_called_once_with("/Volumes/Builds/owned/case/scratch-pools", 0o700)
    with patch.dict(os.environ, {"CARGO_HOME": "/invoking/cargo"}, clear=True):
        pool = "/Volumes/Builds/owned/case/scratch-pools"
        env = cargo_test_env("/Volumes/Builds/owned/case", "/baseline", pool)
        assert env["OXIDE_SCRATCH_ROOT"] == pool and env["TMPDIR"] == pool
        assert env["CARGO_HOME"] == "/invoking/cargo"
        assert env["OXIDE_POOL_CAP"] == POOL_CAP and env["OXIDE_TEST_HOME_FROM_ENV"] == "1"
        assert "OXIDE_TEST_INTERNAL_ROOT" not in env
    # Preserve the first case if a later child raises; a signal exit aborts
    # without starting the second case. No subprocess or fixture is created.
    class Cases:
        def __init__(self, second):
            self.calls = 0
            self.second = second
        def run(self, command, **kwargs):
            self.calls += 1
            if self.calls == 2 and isinstance(self.second, BaseException):
                raise self.second
            code = self.second if self.calls == 1 and isinstance(self.second, int) else 0
            out = (f"test {TESTS[self.calls - 1]} ... ok\ntest result: ok.\n").encode()
            return subprocess.CompletedProcess(command, code, out, b"")
    for second in (RuntimeError("child failed"), -9):
        cases = Cases(second)
        receipts = []
        with patch.dict(globals(), {"record": lambda name, data: receipts.append(json.loads(data)),
                                    "work_left": lambda: 10}):
            try:
                execute_tests(cases, "/repo", {}, True)
            except RuntimeError:
                pass
            else:
                raise AssertionError("diagnostic did not abort")
        assert cases.calls == (1 if isinstance(second, int) else 2)
        assert len(receipts[-1]["cases"]) == 1 and receipts[-1]["skipped"] == [TESTS[1]]
        assert receipts[-1]["qualified"] is False
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
    cargo_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    for name in list(os.environ):
        if name.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "GIT_")) and name not in (
            "OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS",
        ):
            os.environ.pop(name)
    os.environ.update({"RUSTUP_HOME": rustup, "CARGO_HOME": cargo_home, "RUSTFLAGS": "", "CARGO_ENCODED_RUSTFLAGS": ""})
    root = tempfile.mkdtemp(prefix="seed-default-off.", dir=scratch)
    pool_root = os.path.join(root, "scratch-pools")
    os.mkdir(pool_root, 0o700)
    expect(os.path.realpath(pool_root) == pool_root and os.stat(pool_root).st_dev == os.stat(scratch).st_dev,
           "scratch-pools must be canonical and on the selected device")
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    try:
        commit = run(proof, repo, commit, diagnostic, pool_root)
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
