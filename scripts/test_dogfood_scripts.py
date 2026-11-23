#!/usr/bin/env python3
"""Synthetic fixtures for the dogfood installer (scripts/oxide-install.py),
its pin authentication and the persisted wire-format guard.

Never builds, never touches the real ~/.local/bin or pool. Fixture roots stay
inside one fresh mktemp directory. Aggregate budget is 60 seconds.
"""
from __future__ import annotations

import atexit
import datetime
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
_SCRATCH_DIR = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
SCRATCH = Path(tempfile.mkdtemp(prefix="oxide-dogfood-", dir=_SCRATCH_DIR or None)).resolve()


def _remove_scratch():
    # Only this run's own fixture root; never raises, so it is safe in a handler.
    shutil.rmtree(SCRATCH, ignore_errors=True)


def _expire(signum, frame):
    # The alarm's default action would end the process without removing SCRATCH.
    _remove_scratch()
    print("dogfood scripts: the 60 s budget expired", file=sys.stderr)
    os._exit(128 + signum)


atexit.register(_remove_scratch)
signal.signal(signal.SIGALRM, _expire)
signal.alarm(60)


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


installer = load("oxide-install")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.home = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        destination = self.home / ".local/bin/oxide"
        receipts = self.home / ".local/state/oxide/install"
        (self.home / ".local/bin").mkdir(parents=True)
        os.chmod(self.home / ".local", 0o755)
        # Every patched global and the shortened PATH is restored by unittest,
        # even when a case fails, so one case cannot leak into the next.
        for patcher in (
            mock.patch.multiple(installer, HOME=self.home, DESTINATION=destination, RECEIPTS=receipts),
            mock.patch.dict(os.environ, {"PATH": str(self.home / "empty")}),
        ):
            patcher.start()
            self.addCleanup(patcher.stop)
        self.built = self.home / "built"
        self.built.write_bytes(b"new oxide")

    def receipt(self, data):
        # The first replacement may be of a binary from the old installer.
        installer.RECEIPTS.mkdir(parents=True)
        path = installer.RECEIPTS / "20260101T000000Z-aaaaaaaaaaaa.json"
        path.write_text(json.dumps({
            "pin": "a" * 40, "build_instance": "b" * 32,
            "control_state_version": 9, "registration_version": 2,
            "installed": {"path": str(installer.DESTINATION), "sha256": sha256(data)},
        }))
        path.chmod(0o600)
        return path

    def test_initial_install_is_a_regular_staged_file(self):
        previous, expected = installer.preflight(False)
        built_hash, installed = installer.install(self.built, expected)
        self.assertIsNone(previous)
        self.assertEqual(installed["sha256"], built_hash)
        self.assertFalse(installer.DESTINATION.is_symlink())
        self.assertEqual(installer.DESTINATION.read_bytes(), b"new oxide")
        self.assertEqual([p.name for p in installer.DESTINATION.parent.iterdir()], ["oxide"])

    def test_symlinked_bin_directory_refuses(self):
        elsewhere = self.home / "elsewhere"
        elsewhere.mkdir()
        (self.home / ".local/bin").rmdir()
        (self.home / ".local/bin").symlink_to(elsewhere)
        with self.assertRaises(installer.Refusal):
            installer.preflight(False)
        with self.assertRaises(installer.Refusal):
            installer.install(self.built, None)
        self.assertEqual(list(elsewhere.iterdir()), [])

    def test_initial_install_never_clobbers(self):
        _, expected = installer.preflight(False)
        installer.DESTINATION.write_bytes(b"unknown")
        with self.assertRaises(installer.Refusal):
            installer.install(self.built, expected)
        self.assertEqual(installer.DESTINATION.read_bytes(), b"unknown")

    def test_replace_refuses_a_same_content_symlink_swap(self):
        installer.DESTINATION.write_bytes(b"old oxide")
        self.receipt(b"old oxide")
        _, expected = installer.preflight(True)
        decoy = self.home / "decoy"
        decoy.write_bytes(b"old oxide")
        installer.DESTINATION.unlink()
        installer.DESTINATION.symlink_to(decoy)
        with self.assertRaises(installer.Refusal):
            installer.install(self.built, expected)
        self.assertTrue(installer.DESTINATION.is_symlink())
        self.assertEqual(decoy.read_bytes(), b"old oxide")

    def test_replace_refuses_a_same_content_file_swap(self):
        installer.DESTINATION.write_bytes(b"old oxide")
        self.receipt(b"old oxide")
        _, expected = installer.preflight(True)
        replacement = self.home / "replacement"
        replacement.write_bytes(b"old oxide")
        os.replace(replacement, installer.DESTINATION)
        with self.assertRaises(installer.Refusal):
            installer.install(self.built, expected)

    def test_replace_swaps_only_the_receipt_matching_file(self):
        installer.DESTINATION.write_bytes(b"old oxide")
        self.receipt(b"old oxide")
        _, expected = installer.preflight(True)
        _, installed = installer.install(self.built, expected)
        self.assertEqual(installed["sha256"], sha256(b"new oxide"))
        self.assertEqual(installer.DESTINATION.read_bytes(), b"new oxide")

    def capability_binary(self, output, code=0):
        stub = self.home / "capability-stub"
        stub.write_text("#!/bin/sh\nprintf '%s\\n' '" + output + "'\nexit " + str(code) + "\n")
        stub.chmod(0o755)
        return stub

    def test_capability_probe_records_the_built_binary_numbers(self):
        built = self.capability_binary('{"control_state_version": 2, "registration_version": 3, "test_controls": false}')
        self.assertEqual(
            installer.state_capabilities(built),
            {"control_state_version": 2, "registration_version": 3},
        )

    def test_capability_probe_refuses_missing_or_invalid_numbers(self):
        for output in (
            '{"control_state_version": 2}',
            '{"control_state_version": "2", "registration_version": 2}',
            '{"control_state_version": true, "registration_version": 2}',
            '{"control_state_version": 4294967296, "registration_version": 2}',
            '{"control_state_version": 2, "registration_version": -1}',
            "not json",
            "[]",
        ):
            with self.assertRaises(installer.Refusal):
                installer.state_capabilities(self.capability_binary(output))
        with self.assertRaises(installer.Refusal):
            installer.state_capabilities(self.capability_binary('{"control_state_version": 2, "registration_version": 2}', code=1))

    def test_capability_probe_refuses_an_artifact_with_proof_controls(self):
        for output in (
            '{"control_state_version": 2, "registration_version": 2, "test_controls": true}',
            '{"control_state_version": 2, "registration_version": 2}',
            '{"control_state_version": 2, "registration_version": 2, "test_controls": "false"}',
        ):
            with self.assertRaises(installer.Refusal):
                installer.state_capabilities(self.capability_binary(output))

    def test_a_build_is_stamped_with_the_pin_and_a_fresh_instance(self):
        pin, instance = "a" * 40, installer.new_instance()
        self.assertRegex(instance, r"^[0-9a-f]{32}$")
        self.assertEqual(instance, instance.lower())
        self.assertEqual(len({installer.new_instance() for _ in range(8)}), 8)
        os.environ["CARGO_TARGET_DIR"] = "/inherited"
        try:
            environment = installer.build_environment(self.home / "staging", pin, instance)
        finally:
            del os.environ["CARGO_TARGET_DIR"]
        self.assertEqual(environment["OXIDE_BUILD_PIN"], pin)
        self.assertEqual(environment["OXIDE_BUILD_INSTANCE"], instance)
        self.assertEqual(environment["CARGO_TARGET_DIR"], str(self.home / "staging/target"))
        self.assertEqual(
            [name for name in environment if name.startswith("CARGO_TARGET_")],
            ["CARGO_TARGET_DIR"],
        )

    def test_build_identity_must_match_the_supplied_stamp(self):
        pin, instance = "a" * 40, "b" * 32
        matching = json.dumps({"build_pin": pin, "build_instance": instance})
        self.assertEqual(
            installer.build_identity(self.capability_binary(matching), pin, instance)["build_pin"],
            pin,
        )
        for output, code in (
            (json.dumps({"build_pin": pin}), 0),
            (json.dumps({"build_instance": instance}), 0),
            (json.dumps({"build_pin": "c" * 40, "build_instance": instance}), 0),
            (json.dumps({"build_pin": pin, "build_instance": "c" * 32}), 0),
            (json.dumps({"build_pin": None, "build_instance": None}), 0),
            (json.dumps([pin, instance]), 0),
            ("not json", 0),
            (matching, 1),
        ):
            with self.assertRaises(installer.Refusal):
                installer.build_identity(self.capability_binary(output, code=code), pin, instance)

    def test_a_new_receipt_is_owner_only_and_names_its_instance(self):
        started = datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)
        receipt = {"pin": "a" * 40, "build_instance": "b" * 32, "installed": {"path": "x"}}
        path = installer.write_receipt(receipt, started, "a" * 40)
        self.assertEqual(stat.S_IMODE(os.stat(path).st_mode), 0o600)
        self.assertEqual(json.loads(path.read_text())["build_instance"], "b" * 32)
        self.assertEqual(path.name, "20260101T000000Z_000000-aaaaaaaaaaaa.json")
        # An existing receipt is never overwritten.
        with self.assertRaises(FileExistsError):
            installer.write_receipt(receipt, started, "a" * 40)
        # A clock rollback must not publish a receipt readers will overlook.
        with self.assertRaisesRegex(installer.Refusal, "timestamp"):
            installer.write_receipt(receipt, started - datetime.timedelta(seconds=1), "a" * 40)

    def upgrade(self):
        installer.DESTINATION.write_bytes(b"old oxide")
        prior = self.receipt(b"old oxide")
        with (
            contextlib.redirect_stdout(io.StringIO()),
            mock.patch.object(sys, "argv", ["oxide-install.py", "--replace"]),
            mock.patch.object(installer, "authenticate_pin", return_value=("c" * 40, "d" * 40, "e" * 40)),
            mock.patch.object(installer, "build", return_value=(self.built, {"cargo": "fixture"})),
            mock.patch.object(installer, "build_identity"),
            mock.patch.object(installer, "state_capabilities", return_value={
                "control_state_version": 10, "registration_version": 2,
                "ledger_version": 2, "writer_generation": 1,
            }),
            mock.patch.object(installer, "report_old_processes") as report,
        ):
            self.assertEqual(installer.main(), 0)
        report.assert_called_once()
        return prior, Path(str(prior) + ".binary")

    def invoke_rollback(self):
        output, errors = io.StringIO(), io.StringIO()
        with (
            contextlib.redirect_stdout(output),
            contextlib.redirect_stderr(errors),
            mock.patch.object(sys, "argv", ["oxide-install.py", "--rollback"]),
            mock.patch.object(installer, "authenticate_pin", side_effect=AssertionError("git access")),
            mock.patch.object(installer, "build", side_effect=AssertionError("build")),
            mock.patch.object(installer, "report_old_processes"),
        ):
            code = installer.main()
        return code, output.getvalue(), errors.getvalue()

    def test_rollback_restores_a_private_hash_matching_copy_and_records_its_identity(self):
        prior, backup = self.upgrade()
        self.assertEqual(backup.read_bytes(), b"old oxide")
        self.assertEqual(stat.S_IMODE(backup.stat().st_mode), 0o700)
        self.assertNotEqual(backup.stat().st_ino, installer.DESTINATION.stat().st_ino)
        before_rollback = installer.newest_receipt()
        self.assertEqual(before_rollback[1]["version"], 2)
        self.assertEqual(set(before_rollback[1]), {
            "version", "mode", "started", "finished", "pin", "tree", "origin_main", "build",
            "toolchain", "built_sha256", "build_instance", "control_state_version",
            "registration_version", "ledger_version", "writer_generation", "installed", "prior",
        }, "install receipt layout changed. Bump its version and document downgrade compatibility.")
        self.assertEqual(set(before_rollback[1]["installed"]),
                         {"path", "owner_uid", "mode", "size", "mtime", "sha256"})
        self.assertEqual(set(before_rollback[1]["prior"]), {"receipt", "receipt_sha256", "sha256"})
        code, output, errors = self.invoke_rollback()
        self.assertEqual(code, 0, errors)
        self.assertIn("Downgrading persisted state", output)
        self.assertEqual(installer.DESTINATION.read_bytes(), b"old oxide")
        newest = installer.newest_receipt()[1]
        self.assertEqual(newest["version"], installer.RECEIPT_VERSION)
        self.assertEqual(newest["mode"], "rollback")
        self.assertEqual(newest["pin"], "a" * 40)
        self.assertEqual(newest["build_instance"], "b" * 32)
        self.assertEqual(newest["installed"]["sha256"], sha256(b"old oxide"))
        self.assertEqual(newest["prior"]["receipt"], before_rollback[0].name)
        self.assertTrue(prior.is_file())
        self.assertEqual(self.invoke_rollback()[0], 0)
        self.assertEqual(installer.DESTINATION.read_bytes(), b"new oxide")

    def test_rollback_refuses_mismatches_without_changing_the_install(self):
        prior, backup = self.upgrade()
        receipt_bytes = prior.read_bytes()
        for problem in ("missing", "hash", "mode", "link", "receipt", "installed"):
            with self.subTest(problem=problem):
                if problem == "missing":
                    backup.unlink()
                elif problem == "hash":
                    backup.write_bytes(b"changed")
                elif problem == "mode":
                    backup.chmod(0o755)
                elif problem == "link":
                    backup.unlink()
                    backup.symlink_to(self.built)
                elif problem == "receipt":
                    prior.write_bytes(receipt_bytes + b"\n")
                else:
                    installer.DESTINATION.write_bytes(b"changed")
                receipts = sorted(installer.RECEIPTS.glob("*.json"))
                installed = installer.DESTINATION.read_bytes()
                code, _, errors = self.invoke_rollback()
                self.assertEqual(code, 1, errors)
                self.assertIn("oxide:rollback refused", errors)
                self.assertEqual(installer.DESTINATION.read_bytes(), installed)
                self.assertEqual(sorted(installer.RECEIPTS.glob("*.json")), receipts)
                if backup.is_symlink():
                    backup.unlink()
                backup.write_bytes(b"old oxide")
                backup.chmod(0o700)
                prior.write_bytes(receipt_bytes)
                installer.DESTINATION.write_bytes(b"new oxide")

    def test_receipt_write_failure_restores_the_previous_inode(self):
        installer.DESTINATION.write_bytes(b"old oxide")
        prior = self.receipt(b"old oxide")
        old_inode = installer.DESTINATION.stat().st_ino
        with installer.receipt_lock() as directory:
            previous, expected = installer.preflight(True)
            with mock.patch.object(installer, "write_receipt", side_effect=OSError("write failed")):
                with self.assertRaisesRegex(OSError, "write failed"):
                    installer.commit_install(self.built, sha256(b"new oxide"), {
                        "pin": "c" * 40,
                    }, previous, expected, directory)
        self.assertEqual(installer.DESTINATION.read_bytes(), b"old oxide")
        self.assertEqual(installer.DESTINATION.stat().st_ino, old_inode)
        self.assertEqual(installer.newest_receipt()[0], prior)

    def test_old_process_report_only_reads_ps_and_keeps_pid_start_and_argv(self):
        uid = os.getuid()
        old = f"  71 {uid} Thu Jan  1 00:00:08 2026 "
        recent = f"  72 {uid} Thu Jan  1 00:00:10 2026 "
        other = f"  73 {uid} Thu Jan  1 00:00:07 2026 "
        exited = f"  74 {uid} Thu Jan  1 00:00:06 2026 "
        tables = [
            old + "/dir with spaces/oxide\n" + recent + "oxide\n" + other + "cargo\n" + exited + "oxide\n",
            old + "oxide check --profile 'my profile'\n" + recent + "oxide test\n" + other + "cargo oxide\n",
        ]
        output = io.StringIO()
        cutoff = datetime.datetime(2026, 1, 1, 0, 0, 9, tzinfo=datetime.timezone.utc)
        with (
            contextlib.redirect_stdout(output),
            mock.patch.object(os, "kill", side_effect=AssertionError("signalled a process")),
            mock.patch.object(subprocess, "run", side_effect=[
                subprocess.CompletedProcess(["ps"], 0, table) for table in tables
            ]) as read,
        ):
            installer.report_old_processes(cutoff)
        self.assertEqual([(call.args[0][0], call.args[0][-1]) for call in read.call_args_list],
                         [("ps", "comm="), ("ps", "args=")])
        self.assertIn("PID 71  2026-01-01T00:00:08+00:00", output.getvalue())
        self.assertIn("oxide check --profile", output.getvalue())
        for pid in (72, 73, 74):
            self.assertNotIn(f"PID {pid}", output.getvalue())

    def test_replace_refuses_an_unmatched_or_linked_destination(self):
        self.receipt(b"old oxide")
        installer.DESTINATION.write_bytes(b"something else")
        with self.assertRaises(installer.Refusal):
            installer.preflight(True)
        installer.DESTINATION.unlink()
        decoy = self.home / "decoy"
        decoy.write_bytes(b"old oxide")
        installer.DESTINATION.symlink_to(decoy)
        with self.assertRaises(installer.Refusal):
            installer.preflight(True)


FAKE_OXIDE = r'''#!__PYTHON__
import json, os, sys
from pathlib import Path
CONFIG = json.loads(__CONFIG__)
LIVE = CONFIG["live"]
args = sys.argv[1:]
if args == ["__oxide-build-capabilities"]:
    print(json.dumps(CONFIG["probe"]))
elif args == ["__oxide-build-identity"]:
    print(json.dumps({"build_pin": CONFIG["pin"], "build_instance": CONFIG["instance"]}))
else:
    assert os.environ.get("OXIDE_INTERNAL_HANDED_OFF") == "1"
    assert "OXIDE_SCRATCH_ROOT" not in os.environ
    pool = os.environ.get("OXIDE_POOL", "")
    mode = CONFIG["mode"]
    # The first describe names the pool itself, so only commands that run
    # against a pool (every check, and a format-change status) are pinned.
    if args[0] == "check" or (args[0] == "status" and mode == "format"):
        if mode == "live":
            assert "OXIDE_POOL_CREATE" not in os.environ and pool == LIVE, pool
        elif mode == "scratch":
            assert os.environ.get("OXIDE_POOL_CREATE") == "1", "a fresh pool needs the explicit opt-in"
            assert pool != LIVE and pool.startswith(CONFIG["scratch"] + "/"), pool
        else:
            assert "OXIDE_POOL_CREATE" not in os.environ and pool != LIVE, "format-change never names the live pool"
            assert pool.startswith(CONFIG["scratch"] + "/") and Path(pool, "config.json").is_file(), pool
            # The real open refuses a payload with no slots/ as unknown
            # accounting before the copied ledger is read (oxide#367).
            if not Path(pool, "payload", "slots").is_dir():
                print("cannot open: cannot open the pool: unknown_accounting "
                      "(owned accounting is unknown or a foreign entry is present)", file=sys.stderr)
                sys.exit(1)
    with open(CONFIG["log"], "a") as log:
        print(json.dumps([args[0], os.getcwd(), pool]), file=log)
    if args == ["pool", "describe", "--json"]:
        print(json.dumps(CONFIG.get("describe") or {"schema": "oxide.pool.describe/1", "selected": "internal",
                          "internal": {"pool": LIVE}, "removable": None}))
    elif args[0] == "check":
        assert args[-1] == "--offline" and Path("target").is_dir()
        assert Path("Cargo.toml").is_file() and Path("src/lib.rs").is_file()
        assert Path.cwd().parent == Path(CONFIG["scratch"])
        if mode == "scratch":
            Path(pool).mkdir(parents=True, exist_ok=True)
        if CONFIG["mutate_live"] and mode == "format":
            # A writer that ignores the copy would change the live ledger.
            Path(LIVE, "ledger.json").write_text('{"ledger": "changed"}')
        slot = Path(pool) / "slot-9"
        warm = slot.exists()
        if not warm and mode == "format":
            # The first mutating admission upgrades the copy's control state.
            state = Path(pool) / "oxide-managed" / "state.json"
            if state.exists():
                document = json.loads(state.read_text())
                document["version"] = CONFIG["probe"]["control_state_version"]
                state.write_text(json.dumps(document))
        slot.touch()
        elapsed, wait, warmth = (CONFIG["warm_ms"], 0, "warm") if warm else (CONFIG["cold_ms"], CONFIG["queue_ns"], "cold")
        print(f"oxide: admission queue wait {wait} ns", file=sys.stderr)
        print(f"oxide: quick profile, build strategy, slot 9 {warmth}, 0.1s, "
              "(directory-budget-v1, budget-only), pool lock 2.1 ms", file=sys.stderr)
        writer = int(args[args.index("--budget-events") + 1])
        for event in (
            dict(seq=1, event="admitted_child_started", phase="primary_cargo",
                 spawn_late=False, admission_elapsed_ms=elapsed),
            dict(seq=2, event="finished", outcome="child_exit", exit_code=0,
                 drain="complete", lease="released"),
        ):
            os.write(writer, (json.dumps(dict(v=1, invocation="example", **event)) + "\n").encode())
    else:
        assert args == ["status"], args
'''


class CanaryTests(unittest.TestCase):
    PIN = "a" * 40
    CAPABILITIES = {"control_state_version": 10, "registration_version": 2,
                    "ledger_version": 3, "writer_generation": 1}

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(dir=SCRATCH))
        self.destination = self.root / ".local/bin/oxide"
        self.receipts = self.root / ".local/state/oxide/install"
        self.pool = self.root / "live-pool"
        self.scratch = self.root / "scratch"
        self.log = self.root / "calls.jsonl"
        self.destination.parent.mkdir(parents=True)
        self.receipts.mkdir(parents=True)
        self.pool.mkdir()
        self.scratch.mkdir()
        (self.pool / "config.json").write_text("{}")
        (self.pool / "ledger.json").write_text('{"ledger": "live"}')
        (self.scratch / "unrelated").mkdir()
        self.destination.write_text("#!/bin/sh\nexit 99\n")
        self.destination.chmod(0o700)
        (self.receipts / "previous.json").write_text(json.dumps({
            "installed": {"path": str(self.destination), "sha256": installer.sha256(self.destination)},
            **self.CAPABILITIES,
        }))
        for patcher in (
            mock.patch.multiple(installer, HOME=self.root, DESTINATION=self.destination, RECEIPTS=self.receipts),
            mock.patch.object(tempfile, "tempdir", str(self.root)),
            mock.patch.dict(os.environ, {"PATH": str(self.destination.parent), "OXIDE_POOL_CREATE": "1",
                                         "OXIDE_SCRATCH_ROOT": "must not reach the candidate"}),
        ):
            patcher.start()
            self.addCleanup(patcher.stop)

    def fake_build(self, cold_ms=12_200, warm_ms=200, queue_ns=12_000_000_000, mode="live",
                   mutate_live=False, describe=None, **capabilities):
        probe = {**self.CAPABILITIES, **capabilities, "test_controls": False}

        def build(pin, instance, staging):
            built = staging / "oxide"
            config = dict(probe=probe, pin=pin, instance=instance, mode=mode, live=str(self.pool),
                          scratch=str(self.scratch), log=str(self.log), cold_ms=cold_ms,
                          warm_ms=warm_ms, queue_ns=queue_ns, mutate_live=mutate_live,
                          describe=describe)
            built.write_text(FAKE_OXIDE.replace("__PYTHON__", sys.executable)
                             .replace("__CONFIG__", repr(json.dumps(config))))
            built.chmod(0o700)
            return built, {}
        return build

    def invoke(self, flags, build):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.ExitStack() as stack:
            stack.enter_context(mock.patch.object(sys, "argv", ["oxide-install.py", *flags]))
            stack.enter_context(mock.patch.object(
                installer, "authenticate_pin", return_value=(self.PIN, "tree", self.PIN)
            ))
            builder = stack.enter_context(mock.patch.object(installer, "build", side_effect=build))
            stack.enter_context(contextlib.redirect_stdout(stdout))
            stack.enter_context(contextlib.redirect_stderr(stderr))
            status = installer.main()
        return status, stdout.getvalue(), stderr.getvalue(), builder

    def assert_only_crate_removed(self):
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual([call[0] for call in calls], ["status", "pool", "check", "check"])
        self.assertEqual(calls[2][1], calls[3][1])
        self.assertFalse(Path(calls[2][1]).exists())
        self.assertEqual(list(self.scratch.iterdir()), [self.scratch / "unrelated"])
        self.assertTrue((self.pool / "slot-9").is_file())
        self.assertEqual(self.destination.read_text(), "#!/bin/sh\nexit 99\n")

    def test_pass_uses_same_slot_and_excludes_queue_time(self):
        status, stdout, stderr, builder = self.invoke(["--canary"], self.fake_build())
        self.assertEqual(status, 0, stderr)
        builder.assert_called_once()
        self.assertIn("oxide:canary PASS", stdout)
        self.assertIn("new slot: queue wait 12.000s", stdout)
        self.assertIn("new slot: admission 0.200s", stdout)
        self.assertIn("pool lock 2.1 ms", stderr)
        self.assert_only_crate_removed()
        installer.require_canary(self.PIN)
        status, _, stderr, builder = self.invoke(["--replace", "--check", "--require-canary"], None)
        self.assertEqual(status, 0, stderr)
        builder.assert_not_called()

    def test_threshold_fail_invalidates_pass_and_cleans_only_the_crate(self):
        installer.write_canary_receipt(self.PIN, "PASS")
        flags = ["--canary", "--new-seconds", "0.1", "--warm-seconds", "0.1"]
        status, stdout, stderr, _ = self.invoke(flags, self.fake_build())
        self.assertEqual(status, 1)
        self.assertIn("oxide:canary FAIL", stderr)
        self.assertIn("new slot admission exceeded 0.1s", stderr)
        self.assertIn("warm reuse admission exceeded 0.1s", stderr)
        self.assertNotIn("oxide:canary PASS", stdout)
        self.assert_only_crate_removed()
        with self.assertRaises(installer.Refusal):
            installer.require_canary(self.PIN)

    def test_install_and_reinstall_refuse_missing_pass_before_building(self):
        for flags in ([], ["--replace"]):
            status, stdout, stderr, builder = self.invoke([*flags, "--require-canary"], None)
            self.assertEqual(status, 1)
            self.assertIn("Run effigy oxide:canary", stdout)
            self.assertIn("no fresh canary PASS", stderr)
            builder.assert_not_called()

    def test_pins_and_receipt_age_are_checked(self):
        installer.write_canary_receipt(self.PIN, "PASS")
        with self.assertRaises(installer.Refusal):
            installer.require_canary("b" * 40)
        path = self.receipts / "canary" / f"{self.PIN}.json"
        receipt = json.loads(path.read_text())
        now = datetime.datetime.now(datetime.timezone.utc)
        for delta in (datetime.timedelta(hours=-1), datetime.timedelta(seconds=30)):
            with self.subTest(delta=delta):
                path.write_text(json.dumps({**receipt, "finished": (now + delta).isoformat()}))
                with self.assertRaises(installer.Refusal):
                    installer.require_canary(self.PIN)

    def test_forward_only_candidate_refuses_before_live_commands(self):
        status, _, stderr, _ = self.invoke(["--canary"], self.fake_build(writer_generation=2))
        self.assertEqual(status, 1)
        self.assertIn("live canary needs matching writer_generation", stderr)
        self.assertFalse(self.log.exists())

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def live_bytes(self):
        return {name: (self.pool / name).read_bytes() for name in ("config.json", "ledger.json")}

    def write_live_state(self, version):
        (self.pool / "oxide-managed").mkdir()
        (self.pool / "oxide-managed" / "state.json").write_text(json.dumps({"version": version}))

    def test_live_canary_still_refuses_a_control_state_version_change(self):
        status, _, stderr, _ = self.invoke(["--canary"], self.fake_build(control_state_version=11))
        self.assertEqual(status, 1)
        self.assertIn("live canary needs matching control_state_version", stderr)
        self.assertFalse(self.log.exists())

    def test_scratch_root_runs_a_fresh_pool_with_no_live_contact(self):
        flags = ["--canary", "--scratch-root", str(self.scratch)]
        status, stdout, stderr, _ = self.invoke(flags, self.fake_build(mode="scratch", control_state_version=11))
        self.assertEqual(status, 0, stderr)
        self.assertIn("oxide:canary PASS for " + self.PIN + " (scratch-root)", stdout)
        self.assertIn("new slot: admission", stdout)
        calls = self.calls()
        self.assertEqual([call[0] for call in calls], ["check", "check"])
        self.assertTrue(all(call[2] != str(self.pool) for call in calls))
        self.assertFalse((self.pool / "slot-9").exists())
        self.assertEqual(list(self.scratch.iterdir()), [self.scratch / "unrelated"])
        self.assertEqual(json.loads((self.receipts / "canary" / f"{self.PIN}.json").read_text())["mode"],
                         "scratch-root")
        installer.require_canary(self.PIN)

    def test_format_change_upgrades_a_read_only_copy_and_never_writes_live(self):
        self.write_live_state(10)
        before = self.live_bytes()
        before_state = (self.pool / "oxide-managed" / "state.json").read_bytes()
        status, stdout, stderr, _ = self.invoke(
            ["--canary", "--format-change", "--scratch-root", str(self.scratch)],
            self.fake_build(mode="format", control_state_version=11),
        )
        self.assertEqual(status, 0, stderr)
        self.assertIn("oxide:canary PASS for " + self.PIN + " (format-change)", stdout)
        self.assertIn("format-change: control state 10 -> 11 (staged 11)", stdout)
        self.assertIn("format-change: live documents unchanged", stdout)
        self.assertEqual(self.live_bytes(), before)
        self.assertEqual((self.pool / "oxide-managed" / "state.json").read_bytes(), before_state)
        self.assertFalse((self.pool / "slot-9").exists())
        self.assertNotIn(str(self.pool), [call[2] for call in self.calls() if call[0] != "pool"])
        self.assertEqual(list(self.scratch.iterdir()), [self.scratch / "unrelated"])
        # The PASS satisfies --require-canary for the same pin.
        status, _, stderr, builder = self.invoke(["--replace", "--check", "--require-canary"], None)
        self.assertEqual(status, 0, stderr)
        builder.assert_not_called()

    def test_format_change_supports_a_barge_backed_pool_with_owned_slots(self):
        # A Barge-backed internal pool names Barge-owned slots in ledger.json
        # whose roots live in Barge state, not in payload/. The copy carries
        # an empty payload/slots/ so the staged binary still opens the copied
        # documents and upgrades the copied control state (oxide#367).
        (self.pool / "payload").mkdir()
        (self.pool / "payload" / "slots").mkdir()
        (self.pool / "config.json").write_text(json.dumps({
            "version": 1, "backend": "directory-budget-v1", "store_id": "live",
            "cap_bytes": 100000000000, "image_bytes": 0}))
        owned = [
            {"id": 1, "incarnation": 2, "state": "idle", "project": "p",
             "workspace": "w", "profile": "quick", "holder": "oxide-11-3",
             "lease_id": None, "last_lease_id": "lease-1", "dir": None,
             "root": "6b86b273ff34fce1", "owned": True, "rank": 1},
            {"id": 2, "incarnation": 1, "state": "idle", "project": "p",
             "workspace": "w", "profile": "quick", "holder": "oxide-11-4",
             "lease_id": None, "last_lease_id": "lease-2",
             "dir": "effigy-abc/quick-2", "root": "d4735e3a265e16ee",
             "owned": True, "rank": 2},
        ]
        (self.pool / "ledger.json").write_text(json.dumps({
            "version": 3, "store_id": "live", "cap_bytes": 100000000000,
            "next_slot": 3, "next_lease": 5, "holding": [], "slots": owned}))
        self.write_live_state(10)
        before = self.live_bytes()
        before_state = (self.pool / "oxide-managed" / "state.json").read_bytes()
        status, stdout, stderr, _ = self.invoke(
            ["--canary", "--format-change", "--scratch-root", str(self.scratch)],
            self.fake_build(mode="format", control_state_version=11),
        )
        self.assertEqual(status, 0, stderr)
        self.assertIn("oxide:canary PASS for " + self.PIN + " (format-change)", stdout)
        self.assertIn("format-change: control state 10 -> 11 (staged 11)", stdout)
        self.assertIn("format-change: live documents unchanged", stdout)
        self.assertEqual(self.live_bytes(), before)
        self.assertEqual((self.pool / "oxide-managed" / "state.json").read_bytes(), before_state)
        self.assertFalse((self.pool / "slot-9").exists())
        self.assertEqual(list(self.scratch.iterdir()), [self.scratch / "unrelated"])

    def test_format_change_uses_internal_scratch_when_a_removable_pool_is_registered(self):
        self.write_live_state(10)
        removable_scratch = self.root / "removable-scratch"
        removable_scratch.mkdir()
        describe = {
            "schema": "oxide.pool.describe/1",
            "selected": "removable",
            "internal": {"pool": str(self.pool)},
            "removable": {
                "available": True,
                "mount_point": str(self.root / "removable-mount"),
                "payload": str(self.root / "removable-mount" / "payload"),
                "suggested_scratch_root": str(removable_scratch),
                "unavailable_reason": None,
            },
        }
        build = lambda: self.fake_build(mode="format", control_state_version=11, describe=describe)
        for flags in (["--canary", "--format-change"],
                       ["--canary", "--format-change", "--scratch-root", str(self.scratch)]):
            with self.subTest(flags=flags):
                status, stdout, stderr, _ = self.invoke(flags, build())
                self.assertEqual(status, 0, stderr)
                self.assertIn("oxide:canary PASS for " + self.PIN + " (format-change)", stdout)
                self.assertIn("format-change: control state 10 -> 11 (staged 11)", stdout)
                self.assertEqual(list(self.scratch.iterdir()), [self.scratch / "unrelated"])
                self.assertEqual(list(removable_scratch.iterdir()), [])
        self.assertFalse((self.pool / "slot-9").exists())

    def test_format_change_without_a_control_state_still_passes(self):
        status, stdout, stderr, _ = self.invoke(
            ["--canary", "--format-change"], self.fake_build(mode="format", control_state_version=11),
        )
        self.assertEqual(status, 0, stderr)
        self.assertIn("control state absent -> absent (staged 11)", stdout)

    def test_format_change_refuses_when_the_live_documents_change(self):
        self.write_live_state(10)
        status, _, stderr, _ = self.invoke(
            ["--canary", "--format-change"],
            self.fake_build(mode="format", control_state_version=11, mutate_live=True),
        )
        self.assertEqual(status, 1)
        self.assertIn("live pool documents changed during the format-change canary", stderr)
        self.assertEqual((self.pool / "ledger.json").read_text(), '{"ledger": "changed"}')
        with self.assertRaises(installer.Refusal):
            installer.require_canary(self.PIN)

    def test_format_change_refuses_a_live_state_newer_than_the_staged_build(self):
        self.write_live_state(12)
        status, _, stderr, _ = self.invoke(
            ["--canary", "--format-change"], self.fake_build(mode="format", control_state_version=11),
        )
        self.assertEqual(status, 1)
        self.assertIn("is newer than the staged build", stderr)
        self.assertEqual([call[0] for call in self.calls()], ["pool"])

    def test_format_flags_require_canary(self):
        for flags in (["--format-change"], ["--scratch-root", str(self.scratch)]):
            with self.subTest(flags=flags), self.assertRaises(SystemExit):
                self.invoke(flags, None)


class AuthenticatePinTests(unittest.TestCase):
    """The installer authenticates its own fetched source (oxide#302).

    Tiny owned local Git repositories model origin, a publisher and the
    installer's checkout; `installer.REPOSITORY` is repointed at the fixture
    checkout and restored in tearDown, so the real repository is never
    touched. No network, no build, no install, no receipt.
    """

    def setUp(self):
        self.original_repository = installer.REPOSITORY
        self.root = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        self.remote = self.root / "origin.git"
        self.git_raw(None, "init", "-q", "-b", "main", "--bare", str(self.remote))
        self.author = self.root / "author"
        self.git_raw(None, "init", "-q", "-b", "main", str(self.author))
        self.git(self.author, "config", "user.email", "fixture@example.invalid")
        self.git(self.author, "config", "user.name", "fixture")
        (self.author / "one.txt").write_text("one\n")
        self.git(self.author, "add", ".")
        self.git(self.author, "commit", "-qm", "one")
        self.git(self.author, "remote", "add", "origin", str(self.remote))
        self.git(self.author, "push", "-q", "origin", "main")
        (self.author / "two.txt").write_text("two\n")
        self.git(self.author, "add", ".")
        self.git(self.author, "commit", "-qm", "two")
        self.git(self.author, "push", "-q", "origin", "main")
        # An ancestor branch for the requested-pin and interleaving cases.
        self.git(self.author, "branch", "feature", "main~1")
        self.git(self.author, "push", "-q", "origin", "feature")
        self.work = self.root / "work"
        self.git_raw(None, "clone", "-q", str(self.remote), str(self.work))
        installer.REPOSITORY = self.work

    def tearDown(self):
        installer.REPOSITORY = self.original_repository

    def git_raw(self, cwd, *args):
        result = subprocess.run(
            ["git", *([] if cwd is None else ["-C", str(cwd)]), *args],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def git(self, cwd, *args):
        return self.git_raw(cwd, *args).strip()

    def push(self, message, filename):
        (self.author / filename).write_text(message + "\n")
        self.git(self.author, "add", ".")
        self.git(self.author, "commit", "-qm", message)
        self.git(self.author, "push", "-q", "origin", "main")
        return self.git(self.author, "rev-parse", "main")

    def push_side_branch(self):
        """A tip on the remote that is a commit but not an ancestor of main."""
        self.git(self.author, "switch", "-q", "-c", "side", "main~1")
        (self.author / "side.txt").write_text("side\n")
        self.git(self.author, "add", ".")
        self.git(self.author, "commit", "-qm", "side")
        self.git(self.author, "push", "-q", "origin", "side")
        return self.git(self.author, "rev-parse", "side")

    def assert_no_private_refs(self):
        self.assertEqual(self.git(self.work, "for-each-ref", installer.PRIVATE_REF_PREFIX), "")

    def private_fixture_home(self):
        self.home = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        destination = self.home / ".local/bin/oxide"
        receipts = self.home / ".local/state/oxide/install"
        (self.home / ".local/bin").mkdir(parents=True)
        os.chmod(self.home / ".local", 0o755)
        # The patched originals come back with the test that borrowed them.
        patcher = mock.patch.multiple(
            installer, HOME=self.home, DESTINATION=destination, RECEIPTS=receipts
        )
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_exact_advertised_tip_and_requested_ancestors_bind(self):
        tip = self.git(self.author, "rev-parse", "main")
        ancestor = self.git(self.author, "rev-parse", "main~1")
        pin, tree, observed = installer.authenticate_pin(None)
        self.assertEqual((pin, observed), (tip, tip))
        self.assertEqual(tree, self.git(self.work, "rev-parse", f"{tip}^{{tree}}"))
        pin, tree, observed = installer.authenticate_pin(ancestor)
        self.assertEqual((pin, observed), (ancestor, tip))
        self.assertEqual(tree, self.git(self.work, "rev-parse", f"{ancestor}^{{tree}}"))
        self.assert_no_private_refs()

    def test_an_interleaved_fetch_cannot_move_the_deciding_source(self):
        feature = self.git(self.author, "rev-parse", "feature")
        moved = {}

        def interlude(stage):
            if stage == "fetched":
                # Fires after this run's fetch (its private ref already
                # exists) and before the decision: a concurrent fetch of
                # another ref rewrites shared FETCH_HEAD inside the exact
                # race window where an old shared-ref check would read it.
                self.assertEqual(
                    len(self.git(self.work, "for-each-ref", installer.PRIVATE_REF_PREFIX).splitlines()), 1
                )
                self.git(self.work, "fetch", "-q", str(self.remote), "refs/heads/feature")
                moved["head"] = (self.work / ".git/FETCH_HEAD").read_text().split()[0]

        pin, tree, tip = installer.authenticate_pin(None, interlude=interlude)
        self.assertEqual(moved["head"], feature)
        self.assertNotEqual(moved["head"], tip)
        self.assertEqual(pin, tip)
        self.assertEqual(tree, self.git(self.work, "rev-parse", f"{tip}^{{tree}}"))
        self.assert_no_private_refs()

    def test_a_deleted_fetch_head_is_not_needed(self):
        def interlude(stage):
            if stage == "fetched":
                # Deleted inside the race window, before this run decides.
                (self.work / ".git/FETCH_HEAD").unlink()

        pin, _, tip = installer.authenticate_pin(None, interlude=interlude)
        self.assertEqual(pin, tip)
        self.assert_no_private_refs()

    def test_remote_main_advancing_between_advertisement_and_fetch_refuses(self):
        self.private_fixture_home()

        def interlude(stage):
            if stage == "advertised":
                self.push("advance", "advance.txt")

        with self.assertRaises(installer.Refusal) as caught:
            installer.authenticate_pin(None, interlude=interlude)
        self.assertIn("does not match ls-remote", str(caught.exception))
        self.assertFalse(installer.RECEIPTS.exists())
        self.assertEqual(os.listdir(installer.DESTINATION.parent), [])
        self.assertEqual(os.listdir(self.home / ".local"), ["bin"])
        self.assert_no_private_refs()

    def test_a_pin_off_observed_main_refuses(self):
        side = self.push_side_branch()
        # The requested pin must resolve locally; fetching it also leaves
        # shared FETCH_HEAD naming exactly this unauthenticatable commit.
        self.git(self.work, "fetch", "-q", str(self.remote), "refs/heads/side")
        with self.assertRaises(installer.Refusal) as caught:
            installer.authenticate_pin(side)
        self.assertIn("is not on pushed origin main", str(caught.exception))
        self.assert_no_private_refs()

    def test_missing_or_non_commit_requested_objects_refuse(self):
        blob = self.git(self.work, "hash-object", "-w", str(self.author / "one.txt"))
        for bad in ("1" * 40, blob):
            with self.assertRaises(installer.Refusal):
                installer.authenticate_pin(bad)
        self.assert_no_private_refs()

    def test_missing_advertisement_and_unreachable_origin_refuse(self):
        empty = self.root / "empty.git"
        self.git_raw(None, "init", "-q", "--bare", str(empty))
        bare_checkout = self.root / "empty-work"
        self.git_raw(None, "clone", "-q", str(empty), str(bare_checkout))
        installer.REPOSITORY = bare_checkout
        with self.assertRaises(installer.Refusal) as caught:
            installer.authenticate_pin(None)
        self.assertIn("not found", str(caught.exception))
        # The advertisement answers, then the origin vanishes before the fetch.
        installer.REPOSITORY = self.work

        def interlude(stage):
            if stage == "advertised":
                shutil.rmtree(self.remote)

        with self.assertRaises(installer.Refusal) as caught:
            installer.authenticate_pin(None, interlude=interlude)
        self.assertIn("fetching", str(caught.exception))
        self.assert_no_private_refs()

    def test_a_malformed_advertisement_refuses(self):
        original_git = installer.git
        for listing in ("", "9bba566 refs/heads/other", "one two three"):
            installer.git = lambda *args, listing=listing: listing
            try:
                with self.assertRaises(installer.Refusal):
                    installer.authenticate_pin(None)
            finally:
                installer.git = original_git
        self.assert_no_private_refs()


class PersistedFormatTests(unittest.TestCase):
    # Version-specific wire declarations, including nested records and serde
    # attributes. This catches even optional fields omitted by empty fixtures.
    # Add a case for a new version after providing its compatibility path.
    def test_versioned_wire_shapes(self):
        cases = (
            ("ledger", "src/lease/mod.rs", "LEDGER_VERSION", "2", "src/lease/pool.rs",
             ("Ledger", "SlotRecord", "SlotPersistState"),
             "45f102db3b3bbc47ac7425695cd077a9d1f132f24ba8dd4bea9ab80c3d2245b0"),
            ("held-eviction intent", "src/lease/pool/held_eviction.rs", "INTENT_VERSION", "1",
             "src/lease/pool/held_eviction.rs", ("HeldEvictionIntent",),
             "2f1a7984381a9046ccf375e24aa25313aa9e0efab8891aa32020a6f5fccb5e74"),
            ("control", "src/lease/mod.rs", "CONTROL_FENCE_VERSION", "10", "src/lease/control.rs",
             ("State", "Record", "Phase", "Holder", "Lost", "Recovery",
              "Ticket", "Clean", "Flush", "OutputsInit"),
             "99bb43f34292750d3b49f30eaa3007020fe46644c69911e4611fd570f40ada2e"),
            ("registry", "src/bin/oxide/prune.rs", "REGISTRY_VERSION", '"oxide.outputs.registry/2"',
             "src/bin/oxide/prune.rs", ("Registry", "RegistryDoc", "Row", "Incomplete"),
             "a4d2e9d58c996c060573ac543a7e84693ae230b1f2972d4569e33b0bffee90c2"),
            ("output receipt", "src/bin/oxide/outputs/receipt.rs", "PROFILE_RECEIPT_VERSION",
             '"oxide.outputs.transition/3"', "src/bin/oxide/outputs/receipt.rs",
             ("Doc", "PlacementDoc", "Selected", "Prepared", "Reservation", "RootKind",
              "LayoutKind", "ReceiptState"),
             "4d261a0e20ce2fd734c1ec56d5b1433a3fd0fd0a7f2bf66546a5fac0b0b8e5e9"),
            ("removable registration", "src/bin/oxide/removable.rs", "REGISTRATION_VERSION", "3",
             "src/bin/oxide/removable.rs", ("Registration",),
             "5419b1f81c9a1d875921543dce341a10fe2d90b39912ff6e988c9e91f0b691c0"),
            ("unit activity", "src/bin/oxide/cargo_prune.rs", "UNIT_RECORD_VERSION", "1",
             "src/bin/oxide/cargo_prune.rs", ("UnitActivityFile", "UnitActivityEntry"),
             "39736a20697e8b897d73f1fdf65871490de1df3fa4644b2db5e2a534d5d7d2db"),
        )
        for label, version_path, constant, version, path, names, digest in cases:
            with self.subTest(format=label):
                version_source = (SCRIPTS.parent / version_path).read_text()
                declared = re.search(rf"\bconst {constant}: [^=]+? = ([^;]+);", version_source)
                self.assertIsNotNone(declared)
                self.assertEqual(declared[1], version, "Record the new format's compatibility path.")
                source = (SCRIPTS.parent / path).read_text()
                declarations = []
                for name in names:
                    matches = list(re.finditer(
                        rf"^( *)(?:#\[[^\n]+\]\n\1)*(?:pub(?:\((?:crate|in crate::[a-zA-Z0-9_:]+)\))? )?"
                        rf"(?:struct|enum) {name}\b[^{{]*\{{.*?^\1\}}",
                        source, re.S | re.M,
                    ))
                    self.assertEqual(len(matches), 2 if name in ("Doc", "PlacementDoc") else 1, name)
                    declarations.extend(match[0] for match in matches)
                normalized = re.sub(r"\s+", "", re.sub(r"(?m)//.*$", "", "\n".join(declarations)))
                self.assertEqual(sha256(normalized.encode()), digest,
                                 "Persisted fields changed. Bump the format and cover the previous reader or downgrade.")


class StagingCleanupTests(unittest.TestCase):
    def test_a_failed_build_removes_its_staging_directory(self):
        seen = []

        def failing_build(pin, instance, staging):
            seen.append(staging)
            raise installer.Refusal("synthetic build failure")

        with mock.patch.object(sys, "argv", ["oxide-install"]), \
                mock.patch.object(installer, "authenticate_pin", return_value=("a" * 40, "tree", "tip")), \
                mock.patch.object(installer, "preflight", return_value=(None, None)), \
                mock.patch.object(installer, "build", side_effect=failing_build), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            status = installer.main()
        self.assertEqual(status, 1)
        self.assertEqual(len(seen), 1, "the build ran once")
        self.assertFalse(seen[0].exists(), f"staging left behind: {seen[0]}")


class CanaryCleanupTests(unittest.TestCase):
    def test_a_refused_cleanup_does_not_mask_the_original_error(self):
        scratch = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        scratch_id = installer.directory_identity(scratch)

        def replace_crate(built, crate, environment):
            # A new directory at the same path makes the cleanup identity check refuse.
            crate.rename(crate.with_name(crate.name + ".moved"))
            crate.mkdir()
            return None, 0.0, [{"v": 1, "event": "finished", "lease": "released", "drain": "complete"}]

        def failing_assessment(*args, **kwargs):
            raise RuntimeError("original failure")

        arguments = types.SimpleNamespace(new_seconds=10, warm_seconds=5)
        with mock.patch.object(installer, "canary_check", side_effect=replace_crate), \
                mock.patch.object(installer, "assess_canary", side_effect=failing_assessment), \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "original failure"):
                installer.exercise(Path("/unused"), {}, scratch, scratch_id, None, arguments,
                                   {"safe": True})


class DocumentAndScratchTests(unittest.TestCase):
    """The format-change canary's read-only contact and scratch validation.

    These exercise the descriptors the live contact is built on: one no-follow
    open of the scratch directory, one no-follow open of the pool, and every
    live document read relative to it.
    """

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        self.pool = self.root / "pool"
        (self.pool / "payload").mkdir(parents=True)

    def write_live_pool(self, control=True):
        (self.pool / "config.json").write_text("{}")
        (self.pool / "ledger.json").write_text('{"ledger": "live"}')
        if control:
            (self.pool / "oxide-managed").mkdir()
            (self.pool / "oxide-managed/state.json").write_text('{"version": 10}')

    def scratch(self):
        scratch = self.root / "scratch"
        scratch.mkdir(mode=0o700)
        return scratch

    def test_live_documents_are_read_relative_to_the_opened_pool(self):
        self.write_live_pool()
        documents = installer.read_live_documents(self.pool)
        self.assertEqual(sorted(documents), sorted(installer.LIVE_DOCUMENTS))
        self.assertEqual(documents[installer.LIVE_CONFIG][0], b"{}")
        self.assertEqual(documents[installer.LIVE_CONTROL][0], b'{"version": 10}')

    def test_a_pool_without_control_state_reads_two_documents(self):
        self.write_live_pool(control=False)
        documents = installer.read_live_documents(self.pool)
        self.assertEqual(sorted(documents), [installer.LIVE_CONFIG, installer.LIVE_LEDGER])

    def test_a_symlinked_live_document_is_refused(self):
        self.write_live_pool()
        decoy = self.root / "decoy.json"
        decoy.write_text('{"ledger": "elsewhere"}')
        (self.pool / "ledger.json").unlink()
        (self.pool / "ledger.json").symlink_to(decoy)
        with self.assertRaises(installer.Refusal):
            installer.read_live_documents(self.pool)

    def test_a_symlinked_control_directory_is_refused(self):
        self.write_live_pool(control=False)
        elsewhere = self.root / "control"
        elsewhere.mkdir()
        (elsewhere / "state.json").write_text('{"version": 10}')
        (self.pool / "oxide-managed").symlink_to(elsewhere)
        with self.assertRaises(installer.Refusal):
            installer.read_live_documents(self.pool)

    def test_write_document_copy_carries_an_empty_owner_only_slots_directory(self):
        # A Barge-backed pool's copy must open: an empty payload/ with no
        # slots/ refuses as unknown accounting before the ledger is read.
        self.write_live_pool()
        documents = installer.read_live_documents(self.pool)
        root = self.root / "copy-root"
        root.mkdir(mode=0o700)
        copy = installer.write_document_copy(root, documents)
        slots = copy / "payload" / "slots"
        self.assertTrue(slots.is_dir())
        self.assertEqual(list(slots.iterdir()), [])
        self.assertEqual(stat.S_IMODE(slots.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((copy / "payload").stat().st_mode), 0o700)
        for name in ("config.json", "ledger.json", "oxide-managed/state.json"):
            self.assertEqual((copy / name).read_bytes(), documents[name][0])

    def test_verify_live_documents_detects_a_changed_document(self):
        self.write_live_pool()
        before = installer.read_live_documents(self.pool)
        self.assertIsNone(installer.verify_live_documents(self.pool, before))
        (self.pool / "ledger.json").write_text('{"ledger": "changed"}')
        with self.assertRaises(installer.Refusal):
            installer.verify_live_documents(self.pool, before)

    def test_verify_scratch_dir_returns_the_opened_identity(self):
        scratch = self.scratch()
        self.assertEqual(installer.verify_scratch_dir(scratch), installer.directory_identity(scratch))

    def test_verify_scratch_dir_refuses_a_symlink(self):
        scratch = self.scratch()
        alias = self.root / "alias"
        alias.symlink_to(scratch)
        with self.assertRaisesRegex(installer.Refusal, "without following links"):
            installer.verify_scratch_dir(alias)

    def test_verify_scratch_dir_refuses_a_regular_file(self):
        path = self.root / "file"
        path.write_text("x")
        with self.assertRaises(installer.Refusal):
            installer.verify_scratch_dir(path)

    def test_verify_scratch_dir_refuses_a_group_writable_directory(self):
        scratch = self.scratch()
        scratch.chmod(0o770)
        with self.assertRaisesRegex(installer.Refusal, "privately writable"):
            installer.verify_scratch_dir(scratch)

    def test_verify_scratch_dir_refuses_another_volume(self):
        scratch = self.scratch()
        other = scratch.stat().st_dev + 1
        with self.assertRaisesRegex(installer.Refusal, "volume"):
            installer.verify_scratch_dir(scratch, other)

    def describe_removable(self, mount, payload):
        return {
            "schema": "oxide.pool.describe/1",
            "selected": "removable",
            "internal": {"pool": str(self.pool)},
            "removable": {
                "available": True,
                "mount_point": str(mount),
                "payload": str(payload),
                "suggested_scratch_root": str(mount / "scratch"),
                "unavailable_reason": None,
            },
        }

    def run_removable_canary(self, description):
        """Run `canary_scratch` with the fixture mount presented as its own volume.

        The fixture lives on one filesystem, so the mount's `stat` reports a
        distinct device while every other path keeps its real identity.
        """
        real_stat = Path.stat
        mount = Path(description["removable"]["mount_point"])
        device = real_stat(mount).st_dev + 1

        class _MountedStat:
            def __init__(self, real):
                self._real = real
                self.st_dev = device

            def __getattr__(self, name):
                return getattr(self._real, name)

        def fake_stat(self):
            real = real_stat(self)
            if self == mount or mount in self.parents:
                return _MountedStat(real)
            return real

        with mock.patch.object(Path, "stat", fake_stat), \
                mock.patch.object(installer, "verify_scratch_dir",
                                  return_value="identity") as check:
            result = installer.canary_scratch(description, None)
        return result, check, device

    def make_removable_mount(self, payload_parts):
        self.write_live_pool()
        mount = self.root / "mnt"
        payload = mount.joinpath(*payload_parts)
        payload.mkdir(parents=True)
        (mount / "scratch").mkdir(mode=0o700)
        return mount, payload

    def test_canary_scratch_accepts_a_nested_removable_payload(self):
        mount, payload = self.make_removable_mount(("builds", "oxide", "pool"))
        (result, check, device) = self.run_removable_canary(
            self.describe_removable(mount, payload))
        pool, selected_payload, scratch, identity = result
        self.assertEqual(pool, self.pool)
        self.assertEqual(selected_payload, payload)
        self.assertEqual(scratch, mount / "scratch")
        self.assertEqual(identity, "identity")
        check.assert_called_once_with(scratch, device)

    def test_canary_scratch_still_accepts_a_flat_removable_payload(self):
        mount, payload = self.make_removable_mount(("oxide-pool",))
        (pool, selected_payload, scratch, identity), _, _ = self.run_removable_canary(
            self.describe_removable(mount, payload))
        self.assertEqual(pool, self.pool)
        self.assertEqual(selected_payload, payload)
        self.assertEqual(scratch, mount / "scratch")
        self.assertEqual(identity, "identity")

    def test_canary_scratch_refuses_a_payload_equal_to_the_mount(self):
        self.write_live_pool()
        mount = self.root / "mnt"
        mount.mkdir()
        (mount / "scratch").mkdir(mode=0o700)
        with self.assertRaisesRegex(installer.Refusal, "registered mounted volume"):
            self.run_removable_canary(self.describe_removable(mount, mount))

    def test_canary_scratch_refuses_a_payload_outside_the_mount(self):
        self.write_live_pool()
        mount = self.root / "mnt"
        mount.mkdir()
        (mount / "scratch").mkdir(mode=0o700)
        elsewhere = self.root / "elsewhere" / "pool"
        elsewhere.mkdir(parents=True)
        dotted = mount / "builds" / ".." / "escape"
        for payload in (elsewhere, dotted):
            with self.subTest(payload=payload):
                with self.assertRaisesRegex(installer.Refusal, "registered mounted volume"):
                    self.run_removable_canary(self.describe_removable(mount, payload))

    def test_canary_scratch_refuses_a_payload_escaping_via_a_symlink(self):
        self.write_live_pool()
        mount = self.root / "mnt"
        mount.mkdir()
        (mount / "scratch").mkdir(mode=0o700)
        target = self.root / "elsewhere" / "pool"
        target.mkdir(parents=True)
        payload = mount / "pool"
        payload.symlink_to(target)
        with self.assertRaisesRegex(installer.Refusal, "registered mounted volume"):
            self.run_removable_canary(self.describe_removable(mount, payload))

    def test_canary_scratch_refuses_a_dangling_payload_symlink(self):
        # A dangling link is present at the payload path, so it must resolve
        # strictly and fail closed — never fall back to the parent chain.
        self.write_live_pool()
        mount = self.root / "mnt"
        mount.mkdir()
        (mount / "scratch").mkdir(mode=0o700)
        outside = mount / "pool"
        outside.symlink_to(self.root / "elsewhere" / "missing")
        inside = mount / "builds" / "pool"
        inside.parent.mkdir(parents=True)
        inside.symlink_to(mount / "builds" / "also-missing")
        for payload in (outside, inside):
            with self.subTest(payload=payload):
                with self.assertRaisesRegex(installer.Refusal, "registered mounted volume"):
                    self.run_removable_canary(self.describe_removable(mount, payload))

    def test_format_canary_scratch_ignores_a_registered_removable_volume(self):
        self.write_live_pool()
        scratch = self.scratch()
        description = {
            "schema": "oxide.pool.describe/1",
            "selected": "removable",
            "internal": {"pool": str(self.pool)},
            "removable": {
                "available": True,
                "mount_point": str(self.root / "mount"),
                "payload": str(self.root / "mount" / "payload"),
                "suggested_scratch_root": str(self.root / "mount" / "scratch"),
                "unavailable_reason": None,
            },
        }
        with mock.patch.object(installer, "verify_scratch_dir", return_value="identity") as check:
            pool, selected, identity = installer.format_canary_scratch(description, None)
            self.assertEqual(pool, self.root / "pool")
            self.assertEqual(selected, scratch)
            check.assert_called_once_with(scratch, os.stat(self.pool).st_dev)
            self.assertEqual(identity, "identity")

    def test_format_canary_scratch_honors_an_explicit_scratch_root(self):
        self.write_live_pool()
        scratch = self.scratch()
        description = {
            "schema": "oxide.pool.describe/1",
            "selected": "removable",
            "internal": {"pool": str(self.pool)},
            "removable": {
                "available": True,
                "mount_point": str(self.root / "mount"),
                "payload": str(self.root / "mount" / "payload"),
                "suggested_scratch_root": str(self.root / "mount" / "scratch"),
                "unavailable_reason": None,
            },
        }
        with mock.patch.object(installer, "verify_scratch_dir", return_value="identity") as check:
            _, selected, _ = installer.format_canary_scratch(description, scratch)
            self.assertEqual(selected, scratch)
            check.assert_called_once_with(scratch, os.stat(self.pool).st_dev)

    def test_git_is_bounded_in_time(self):
        expired = subprocess.TimeoutExpired(["git"], installer.GIT_SECONDS)
        with mock.patch.object(installer.subprocess, "run", side_effect=expired):
            with self.assertRaisesRegex(installer.Refusal, "exceeded"):
                installer.git("ls-remote", "origin", "main")

    def test_a_refused_archive_extraction_reaps_the_writer(self):
        class Archive:
            def __init__(self):
                self.stdout = io.BytesIO()
                self.terminated = False
                self.waited = False

            def __enter__(self):
                return self

            def __exit__(self, *exception):
                self.waited = True
                return False

            def terminate(self):
                self.terminated = True

            def wait(self):
                self.waited = True
                return 0

        archive = Archive()
        staging = self.root / "staging"
        staging.mkdir()

        def refuse(*args, **kwargs):
            raise subprocess.CalledProcessError(2, "tar")

        with mock.patch.object(installer.subprocess, "Popen", return_value=archive), \
                mock.patch.object(installer.subprocess, "run", side_effect=refuse):
            with self.assertRaises(subprocess.CalledProcessError):
                installer.build("a" * 40, "b" * 32, staging)
        self.assertTrue(archive.terminated, "the archive writer must be terminated on failure")
        self.assertTrue(archive.stdout.closed, "the parent's pipe copy must be closed")
        self.assertTrue(archive.waited)


if __name__ == "__main__":
    unittest.main(verbosity=2)
