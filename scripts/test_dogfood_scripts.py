#!/usr/bin/env python3
"""Synthetic fixtures for the dogfood test supervisor and installer.

Never builds, never touches the real ~/.local/bin or pool. Fixture roots stay
inside one fresh mktemp directory. Aggregate budget is 60 seconds.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import signal
import sys
import tempfile
import time
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


bounded = load("bounded-test")
installer = load("oxide-install")

# An `oxide`-shaped parent: Cargo-like child in its own group (`setpgid`, as
# `oxide` does) that ignores TERM, a parent that forwards TERM to that group
# and waits for it.
FORWARDER = """
import os, signal, subprocess, sys
child = subprocess.Popen([sys.executable, "-c",
    "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)"],
    preexec_fn=lambda: os.setpgid(0, 0))
open(sys.argv[1], "w").write(str(child.pid))
signal.signal(signal.SIGTERM, lambda *_: os.killpg(child.pid, signal.SIGTERM))
child.wait()
"""

# The same parent after Cargo itself exits: the group leader spawns a
# TERM-ignoring child in its group, then exits at once and is reaped; the
# parent waits for the group to empty, as `oxide` does.
ORPHAN = """
import os, signal, subprocess, sys, time
leader = subprocess.Popen([sys.executable, "-c",
    "import subprocess, sys, time; child = subprocess.Popen([sys.executable, '-c', "
    "'import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)']); "
    "open(sys.argv[1], 'w').write(str(child.pid))", sys.argv[1]],
    preexec_fn=lambda: os.setpgid(0, 0))
group = leader.pid
signal.signal(signal.SIGTERM, lambda *_: os.killpg(group, signal.SIGTERM))
leader.wait()
while True:
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        break
    time.sleep(0.1)
"""


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


class SupervisorTests(unittest.TestCase):
    def test_normal_exit_passes_through(self):
        self.assertEqual(bounded.supervise([sys.executable, "-c", "raise SystemExit(3)"]), 3)

    def test_timeout_reaches_the_separate_group(self):
        pid_file = SCRATCH / "child.pid"
        code = bounded.supervise(
            [sys.executable, "-c", FORWARDER, str(pid_file)], limit=1, grace=1, closure=3
        )
        self.assertEqual(code, 124)
        child = int(pid_file.read_text())
        deadline = time.monotonic() + 3
        while alive(child) and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(alive(child), "TERM-ignoring child in Cargo's group survived")

    def test_timeout_reaches_the_group_after_its_leader_exits(self):
        pid_file = SCRATCH / "orphan.pid"
        code = bounded.supervise(
            [sys.executable, "-c", ORPHAN, str(pid_file)], limit=2, grace=1, closure=3
        )
        self.assertEqual(code, 124)
        orphan = int(pid_file.read_text())
        deadline = time.monotonic() + 3
        while alive(orphan) and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(alive(orphan), "orphan in Cargo's group survived its leader's exit")


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
