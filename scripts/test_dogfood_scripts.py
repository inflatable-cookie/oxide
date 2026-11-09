#!/usr/bin/env python3
"""Synthetic fixtures for the dogfood installer and the account-home proof's
image custody (oxide#143).

The shared session supervisor (`bounded-test.py`) has its own suite,
`test_supervisor.py`, run by `proof:supervisor:qualify`.

Never builds, never touches the real ~/.local/bin or pool. Fixture roots stay
inside one fresh mktemp directory. Aggregate budget is 60 seconds.
"""
from __future__ import annotations

import datetime
import hashlib
import importlib.util
import json
import os
import shutil
import signal
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

signal.alarm(60)

SCRIPTS = Path(__file__).resolve().parent
_SCRATCH_DIR = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
SCRATCH = Path(tempfile.mkdtemp(prefix="oxide-dogfood-", dir=_SCRATCH_DIR or None)).resolve()


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


installer = load("oxide-install")
account_home = load("test-account-home")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.home = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        (self.home / ".local/bin").mkdir(parents=True)
        os.chmod(self.home / ".local", 0o755)
        installer.HOME = self.home
        installer.DESTINATION = self.home / ".local/bin/oxide"
        installer.RECEIPTS = self.home / ".local/state/oxide/install"
        self.path = os.environ["PATH"]
        os.environ["PATH"] = str(self.home / "empty")
        self.built = self.home / "built"
        self.built.write_bytes(b"new oxide")

    def tearDown(self):
        os.environ["PATH"] = self.path

    def receipt(self, data):
        installer.RECEIPTS.mkdir(parents=True)
        (installer.RECEIPTS / "1.json").write_text(json.dumps({"installed": {"sha256": sha256(data)}}))

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
        self.assertEqual(path.name, "20260101T000000Z-aaaaaaaaaaaa.json")
        # An existing receipt is never overwritten.
        with self.assertRaises(FileExistsError):
            installer.write_receipt(receipt, started, "a" * 40)

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
        (self.home / ".local/bin").mkdir(parents=True)
        os.chmod(self.home / ".local", 0o755)
        installer.HOME = self.home
        installer.DESTINATION = self.home / ".local/bin/oxide"
        installer.RECEIPTS = self.home / ".local/state/oxide/install"

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
                # A concurrent fetch of another ref rewrites shared FETCH_HEAD
                # between this run's fetch and its decision: the interleaving
                # that made the old shared-ref check falsely refuse.
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


class AccountHomeCustodyTests(unittest.TestCase):
    """The account-home proof's image custody and cleanup decisions with a
    fake hdiutil, clock and mount table: nothing is created or mounted."""

    MOUNT = "/Volumes/OxAhsynthetic"
    PLIST = (b'<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>system-entities</key><array>'
             b'<dict><key>dev-entry</key><string>/dev/disk9s1</string><key>mount-point</key>'
             b'<string>/Volumes/OxAhsynthetic</string></dict></array></dict></plist>')

    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(dir=SCRATCH)) / "owned-image"
        self.custody = account_home.Custody(str(self.directory), self.MOUNT)
        Path(self.custody.image).write_bytes(b"backing")
        self.custody.record("image-created")
        self.calls = []

    def runner(self, answers):
        def run(command, timeout):
            self.calls.append((command[1], timeout))
            answer = answers.pop(0)
            if isinstance(answer, BaseException):
                raise answer
            return answer
        return run

    def persisted(self):
        return json.loads((self.directory / "custody.json").read_text())

    def attached(self):
        mount = account_home.attach_image(self.custody, self.runner([(0, self.PLIST, "")]), 30)
        self.assertEqual(mount, self.MOUNT)
        self.assertEqual(self.persisted()["state"], "attached")

    def test_custody_is_persisted_before_the_attach_runs(self):
        def run(command, timeout):
            self.assertEqual(self.persisted()["state"], "attach-requested")
            return 0, self.PLIST, ""
        account_home.attach_image(self.custody, run, 30)

    def test_attach_failures_keep_custody_and_the_image(self):
        for answer, state in ((account_home.subprocess.TimeoutExpired("hdiutil", 30), "attach-unknown"),
                              ((1, b"not a plist", "boom"), "attach-unknown"),
                              ((1, b'<?xml version="1.0"?><plist version="1.0"><dict><key>system-entities</key><array/></dict></plist>', "x"),
                               "attach-mismatch")):
            self.custody.record("image-created", mounts=[])
            with self.assertRaises(RuntimeError):
                account_home.attach_image(self.custody, self.runner([answer]), 30)
            self.assertEqual(self.persisted()["state"], state)
            self.assertFalse(account_home.detach_image(self.custody, self.runner([]), 10**12, False,
                                                       clock=lambda: 0, mounted=lambda path: False))
            self.assertEqual(self.persisted()["state"], "retained")
            self.assertTrue(Path(self.custody.image).exists())
        self.assertEqual([call[0] for call in self.calls], ["attach", "attach", "attach"])

    def test_a_mismatched_mount_is_recorded_and_detached_ordinarily(self):
        other = self.PLIST.replace(b"OxAhsynthetic", b"OxAhsynthetic 1")
        with self.assertRaises(RuntimeError):
            account_home.attach_image(self.custody, self.runner([(0, other, "")]), 30)
        self.assertEqual(self.persisted()["mounts"], [self.MOUNT + " 1"])
        self.assertTrue(account_home.detach_image(self.custody, self.runner([(0, b"", "")]), 10**12, False,
                                                  clock=lambda: 0, mounted=lambda path: False))
        self.assertEqual(self.calls[-1][0], "detach")
        self.assertEqual(self.persisted()["state"], "image-removed")
        self.assertFalse(Path(self.custody.image).exists())

    def test_unknown_writers_or_too_little_time_keep_the_image_without_detach(self):
        self.attached()
        self.assertFalse(account_home.detach_image(self.custody, self.runner([]), 10**12, True,
                                                   clock=lambda: 0, mounted=lambda path: True))
        self.assertIn("writer custody unknown", self.persisted()["reason"])
        self.custody.record("attached")
        deadline = 2 * account_home.NS
        self.assertFalse(account_home.detach_image(self.custody, self.runner([]), deadline, False,
                                                   clock=lambda: 0, mounted=lambda path: True))
        self.assertIn("before the guard's stop", self.persisted()["reason"])
        self.assertEqual([call[0] for call in self.calls], ["attach"])
        self.assertTrue(Path(self.custody.image).exists())

    def test_a_failed_timed_out_or_lingering_detach_keeps_the_image_and_never_retries(self):
        for answer, present in (((1, b"", "busy"), True), (account_home.subprocess.TimeoutExpired("hdiutil", 5), True),
                                ((0, b"", ""), True)):
            self.custody.record("attached", mounts=[self.MOUNT])
            self.assertFalse(account_home.detach_image(self.custody, self.runner([answer]), 10**12, False,
                                                       clock=lambda: 0, mounted=lambda path: present))
            self.assertEqual(self.persisted()["state"], "retained")
            self.assertFalse(account_home.detach_image(self.custody, self.runner([]), 10**12, False,
                                                       clock=lambda: 0, mounted=lambda path: False))
            self.assertTrue(Path(self.custody.image).exists())

    def test_detach_is_bounded_by_the_guard_stop_not_the_work_margin(self):
        self.attached()
        deadline = 9 * account_home.NS
        self.assertTrue(account_home.detach_image(self.custody, self.runner([(0, b"", "")]), deadline, False,
                                                  clock=lambda: 0, mounted=lambda path: False))
        self.assertEqual(self.calls[-1], ("detach", 8.0))
        os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(100 * account_home.NS)
        try:
            self.assertEqual(account_home.cleanup_deadline_ns(), 79 * account_home.NS)
        finally:
            del os.environ["OXIDE_PROOF_DEADLINE_NS"]

    def test_an_interrupted_or_failing_detach_latches_retained_with_one_attempt(self):
        for raised in (KeyboardInterrupt(), OSError("runner failed")):
            self.custody.record("attached", mounts=[self.MOUNT])
            self.calls.clear()
            with self.assertRaises(type(raised)):
                account_home.detach_image(self.custody, self.runner([raised]), 10**12, False,
                                          clock=lambda: 0, mounted=lambda path: False)
            self.assertEqual(self.custody.facts["state"], "retained")
            self.assertFalse(account_home.detach_image(self.custody, self.runner([]), 10**12, False,
                                                       clock=lambda: 0, mounted=lambda path: False))
            self.assertEqual([call[0] for call in self.calls], ["detach"])
            self.assertTrue(Path(self.custody.image).exists())

    def test_a_failing_confirmation_or_evidence_write_latches_retained(self):
        self.custody.record("attached", mounts=[self.MOUNT])

        def confirm(path):
            raise OSError("statfs failed")
        with self.assertRaises(OSError):
            account_home.detach_image(self.custody, self.runner([(0, b"", "")]), 10**12, False,
                                      clock=lambda: 0, mounted=confirm)
        self.assertEqual(self.custody.facts["state"], "retained")
        self.assertTrue(Path(self.custody.image).exists())

        self.custody.record("attached", mounts=[self.MOUNT])
        self.calls.clear()
        original = self.custody.record

        def broken(state, **facts):
            raise OSError("evidence write failed")
        self.custody.record = broken
        try:
            with self.assertRaises(OSError):
                account_home.detach_image(self.custody, self.runner([(0, b"", "")]), 10**12, False,
                                          clock=lambda: 0, mounted=lambda path: False)
        finally:
            self.custody.record = original
        self.assertEqual(self.custody.facts["state"], "retained")
        self.assertEqual(self.custody.facts["write_error"], "OSError")
        self.assertEqual(self.calls, [], "no device action after the intention could not be recorded")
        self.assertTrue(Path(self.custody.image).exists())

    def test_the_first_fault_survives_cleanup_and_record_failures(self):
        evidence = Path(tempfile.mkdtemp(dir=SCRATCH))
        primary = RuntimeError("case 07 failed")
        self.custody.record("attached", mounts=[self.MOUNT])

        def cleanup():
            self.custody.record = lambda state, **facts: (_ for _ in ()).throw(OSError("evidence write failed"))
            raise KeyboardInterrupt()
        body = {}
        written = account_home.closeout_after_failure(primary, self.custody, False, body, str(evidence), detach=cleanup)
        receipt = json.loads(Path(written).read_text())
        self.assertIn("case 07 failed", receipt["result"])
        self.assertIn("KeyboardInterrupt", receipt["cleanup_error"])
        self.assertTrue(Path(self.custody.image).exists())

        def no_receipt(directory, body):
            raise OSError("evidence full")
        self.assertTrue(account_home.closeout_after_failure(primary, None, False, {}, str(evidence),
                                                            write_receipt=no_receipt).startswith("not written (OSError"))

    def test_the_custody_record_stays_bounded(self):
        self.custody.record("attached", stderr="x" * 200_000)
        self.assertLessEqual((self.directory / "custody.json").stat().st_size, account_home.CUSTODY_MAX_BYTES)
        self.assertTrue(self.persisted()["truncated"])

class HandoffSelectorTests(unittest.TestCase):
    """Pin both selected-volume commands and their unchanged bounds."""

    def selector_line(self, manifest, selector):
        header = f'[tasks."{selector}"]'
        self.assertIn(header, manifest)
        section = manifest.split(header, 1)[1]
        for line in section.splitlines()[1:]:
            if line.startswith("[tasks."):
                break
            if line.startswith("run = "):
                return line[len("run = "):].strip().strip('"')
        self.fail(f"{selector} has no run line")

    def test_exact_commands(self):
        manifest = (SCRIPTS.parent / "effigy.toml").read_text(encoding="utf-8")
        base = ("python3 scripts/bounded-test.py --limit 600 --scratch-budget-mib 128 "
                "-- python3 scripts/test-installed-handoff.py")
        self.assertEqual(self.selector_line(manifest, "rust:installed-handoff"), base)
        self.assertEqual(self.selector_line(manifest, "rust:installed-handoff:diagnostic"),
                         base + " --diagnostic")
        self.assertEqual(self.selector_line(manifest, "proof:installed-handoff:test"),
                         "python3 scripts/test-installed-handoff.py --self-check")
        self.assertEqual(self.selector_line(manifest, "proof:installed-handoff:plan"),
                         "python3 scripts/test-installed-handoff.py --plan")


if __name__ == "__main__":
    unittest.main(verbosity=2)
