#!/usr/bin/env python3
"""Synthetic fixtures for the dogfood installer.

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
import signal
import stat
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
