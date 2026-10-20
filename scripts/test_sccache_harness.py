"""Synthetic regressions for the oxide#096 sccache proof harness.

Deterministic and effect-free beyond `sleep` / `sh` children inside one fresh
`/private/tmp` directory: no Cargo, sccache, pool or image. Each case checks
the actual signal, reap and closure outcome, not only exception text.
"""
from __future__ import annotations

import importlib.util
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location("sccache_harness", os.path.join(HERE, "test-sccache-compatibility.py"))
h = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(h)
cli = h.cli


def gone(call, target: int, seconds: float = 5) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if cli.probe(call, target) == "ESRCH":
            return True
        time.sleep(0.05)
    return cli.probe(call, target) == "ESRCH"


def wait_file(path: str, seconds: float = 5) -> int:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if os.path.exists(path):
            with open(path, encoding="utf-8") as handle:
                text = handle.read().strip()
            if text:
                return int(text)
        time.sleep(0.02)
    raise AssertionError(f"{path} never written")


class Harness(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = os.path.realpath(tempfile.mkdtemp(prefix="oxide-096-synth.", dir="/private/tmp"))
        self.spawned = []
        h.DEADLINE[0] = time.monotonic() + 30

    def tearDown(self) -> None:
        # Only this test's own children.
        for child in self.spawned:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=5)
        for root, dirs, _ in os.walk(self.dir):
            for name in dirs:
                os.chmod(os.path.join(root, name), 0o700)
        for root, _, files in os.walk(self.dir):
            for name in files:
                os.chmod(os.path.join(root, name), 0o600)
        shutil.rmtree(self.dir)

    def sleeper(self, leader: bool) -> subprocess.Popen:
        child = subprocess.Popen(["/bin/sleep", "30"], start_new_session=leader)
        self.spawned.append(child)
        return child

    def test_clock_expiry_after_popen_settles_staging_group(self) -> None:
        monitor = h.Monitor(self.dir)
        pidfile = os.path.join(self.dir, "grandchild.pid")
        argv = ["/bin/sh", "-c", f"/bin/sleep 30 >/dev/null 2>&1 & echo $! > {pidfile}; wait"]
        calls = {"n": 0}

        def remaining() -> float:
            calls["n"] += 1
            if calls["n"] == 1:
                return 100.0
            wait_file(pidfile)
            raise RuntimeError("proof phase exceeded 255 s")

        with mock.patch.object(cli, "remaining", remaining):
            with self.assertRaisesRegex(RuntimeError, "proof phase exceeded"):
                h.stage_build(monitor, argv, self.dir, dict(os.environ))
        build, leader = monitor.children[0]
        grandchild = wait_file(pidfile)
        self.assertTrue(leader)
        self.assertIsNotNone(build.returncode, "staging leader was not reaped")
        self.assertTrue(gone(os.killpg, build.pid), "staging group survived")
        self.assertTrue(gone(os.kill, grandchild), "staging grandchild survived")
        self.assertIn(("group", build.pid), monitor.settled)

    def test_parse_failure_with_live_group_member_settles(self) -> None:
        monitor = h.Monitor(self.dir)
        pidfile = os.path.join(self.dir, "grandchild.pid")
        argv = ["/bin/sh", "-c", f"/bin/sleep 30 >/dev/null 2>&1 & echo $! > {pidfile}; echo '{{bad json'"]
        with self.assertRaises(ValueError):
            h.stage_build(monitor, argv, self.dir, dict(os.environ))
        build, _ = monitor.children[0]
        grandchild = wait_file(pidfile)
        self.assertTrue(gone(os.kill, grandchild), "grandchild outlived the parse failure")
        self.assertTrue(gone(os.killpg, build.pid))

    def test_interrupt_during_staging_settles(self) -> None:
        monitor = h.Monitor(self.dir)
        pidfile = os.path.join(self.dir, "leader.pid")
        argv = ["/bin/sh", "-c", f"echo $$ > {pidfile}; exec /bin/sleep 30"]
        original = subprocess.Popen.communicate

        def interrupted(self_, *args, **kwargs):
            wait_file(pidfile)
            raise KeyboardInterrupt

        with mock.patch.object(subprocess.Popen, "communicate", interrupted):
            with self.assertRaises(KeyboardInterrupt):
                h.stage_build(monitor, argv, self.dir, dict(os.environ))
        build, _ = monitor.children[0]
        self.assertIsNotNone(build.returncode)
        self.assertEqual(build.returncode, -signal.SIGKILL)
        self.assertTrue(gone(os.killpg, build.pid))
        subprocess.Popen.communicate = original

    def test_unreadable_sources_and_failed_signal_do_not_stop_other_targets(self) -> None:
        monitor = h.Monitor(self.dir)
        state = os.path.join(self.dir, "pool", "oxide-managed", "state.json")
        os.makedirs(os.path.dirname(state))
        with open(state, "w", encoding="utf-8") as handle:
            handle.write("{bad json")
        monitor.cargo_log = os.path.join(self.dir, "cargo.log")
        with open(monitor.cargo_log, "w", encoding="utf-8") as handle:
            handle.write("1|/|check\n")
        os.chmod(monitor.cargo_log, 0)
        stubborn = self.sleeper(True)
        child = self.sleeper(True)
        server = self.sleeper(False)
        monitor.children += [(stubborn, True), (child, True)]
        monitor.servers[server.pid] = "synthetic server"

        def killpg(group: int, sig: int) -> None:
            if group == stubborn.pid and sig:
                raise PermissionError("injected")
            os.killpg(group, sig)

        monitor.killpg = killpg
        pending = monitor.settle(time.monotonic() + 3)
        self.assertEqual(child.wait(timeout=5), -signal.SIGKILL)
        self.assertEqual(server.wait(timeout=5), -signal.SIGKILL)
        self.assertIsNone(stubborn.poll(), "the injected failure should leave this target alive")
        self.assertIn(f"group {stubborn.pid}", pending)
        joined = "\n".join(pending)
        self.assertIn("UNKNOWN Oxide state unreadable", joined)
        self.assertIn("UNKNOWN launcher log unreadable", joined)
        self.assertIn(f"UNKNOWN SIGKILL group {stubborn.pid} failed", joined)
        self.assertNotIn(("group", 1), monitor.settled)

    def test_monitor_failure_interrupts_main_and_kills_writers(self) -> None:
        monitor = h.Monitor(self.dir)
        child = self.sleeper(True)
        monitor.children.append((child, True))

        def broken(_path: str) -> int:
            raise OSError("df unavailable")

        with mock.patch.object(cli, "host_free_bytes", broken):
            with self.assertRaises(KeyboardInterrupt):
                monitor.thread.start()
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    time.sleep(0.02)
        monitor.thread.join(timeout=5)
        self.assertIn("monitor failed", monitor.breach)
        self.assertEqual(child.wait(timeout=5), -signal.SIGKILL)

    def test_temp_binding_reaches_every_child_environment(self) -> None:
        saved = dict(os.environ)
        sentinel = "/nonexistent/oxide-096-sentinel"
        try:
            for name in ("TMPDIR", "TMP", "TEMP", "OXIDE_TASK_SCRATCH_ROOT"):
                os.environ[name] = sentinel
            tmp = h.bind_temp(self.dir)
            self.assertEqual(tmp, os.path.join(self.dir, "tmp"))
            home = os.path.join(self.dir, "home")
            base, _ = h.clean_env(home, "/nonexistent/rustup")
            stage = dict(base)
            stage.update({"CARGO_HOME": os.path.join(self.dir, "stage-home")})
            runner = cli.Runner("oxide", "lease", self.dir, extra={"HOME": home})
            for env in (base, stage, runner.env):
                shown = subprocess.run(["/usr/bin/env"], env=env, capture_output=True, text=True, timeout=10).stdout
                self.assertNotIn(sentinel, shown)
                for name in ("TMPDIR", "TMP", "TEMP"):
                    self.assertEqual(os.path.realpath(env[name]), tmp)
            self.assertEqual(base["OXIDE_TASK_SCRATCH_ROOT"], tmp)
            self.assertEqual(stage["OXIDE_TASK_SCRATCH_ROOT"], tmp)
        finally:
            os.environ.clear()
            os.environ.update(saved)

    def test_launcher_render_keeps_private_names(self) -> None:
        text = h.LAUNCHER.replace("@REAL_CARGO@", "/x/cargo").replace("@SCCACHE@", h.SCCACHE).replace("@LOG@", "/x/log")
        self.assertIsNone(re.search(r"@[A-Z_]+@", text))
        self.assertNotIn("/sccache_", text)
        self.assertIn("SCCACHE_NO_DAEMON=1", text)
        self.assertIn('[ -n "$FIXTURE_SCCACHE" ]', text)
        path = os.path.join(self.dir, "launcher.sh")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
        self.assertEqual(subprocess.run(["/bin/sh", "-n", path], timeout=10).returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
