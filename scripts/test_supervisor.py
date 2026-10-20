#!/usr/bin/env python3
"""Synthetic qualification suite for the shared session supervisor (oxide#109).

Runs under `proof:supervisor:qualify`, inside the candidate supervisor's own
session and scratch. Pure cases drive `bounded-test.py` with a fake clock,
process table, signals and readings: nothing is spawned and no survivor can
outlive them. Real cases run tiny Python children in owned scratch and check
that every fixture PID is gone as soon as the supervisor returns. Fixture
sleepers carry a finite self-exit fallback (90 s) as secondary protection; a
case passes only when the supervisor closed them well before it.
Low-reserve cases inject readings and never fill the disk.
"""
from __future__ import annotations

import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
SUPERVISOR = SCRIPTS / "bounded-test.py"
_scratch_root = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
SCRATCH = Path(tempfile.mkdtemp(prefix="suite.", dir=_scratch_root or None)).resolve()
GIB = 1024 ** 3
FALLBACK = 90  # fixture self-exit, seconds; never reached by a passing case

spec = importlib.util.spec_from_file_location("bounded_test", SUPERVISOR)
bounded = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bounded)


# Pure fakes ---------------------------------------------------------------


class Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class FakeLeader:
    """A leader in the fake process table. It exits naturally at `exit_at`
    (becoming a zombie with an unreadable session) or when signalled."""

    def __init__(self, world, exit_at=None, status=0, pipes=True, ignores_term=False):
        self.world, self.pid, self.exit_at, self.status, self.pipes = world, world.session, exit_at, status, pipes
        self.reaped = False
        world.procs[self.pid] = [self.pid, self.pid, "S", ignores_term]

    def exited(self, timeout=0.0):
        self.world.clock.sleep(timeout)
        if self.exit_at is not None and self.world.clock() >= self.exit_at and self.pid in self.world.procs:
            self.world.procs[self.pid] = [self.pid, None, "Z", False]
        entry = self.world.procs.get(self.pid)
        return entry is None or entry[2] == "Z"

    def reap(self, timeout):
        self.world.events.append("reap")
        self.reaped = True
        return self.status if self.pid not in self.world.procs or self.world.procs[self.pid][2] == "Z" else -9

    def output_closed(self, timeout):
        return self.pipes


class World:
    def __init__(self, session=500):
        self.clock = Clock()
        self.session = session
        self.procs = {}  # pid -> [pgid, sid, state, ignores TERM ("all" ignores KILL too)]
        self.signals = []
        self.events = []
        self.observe_error = None
        self.identities = {}

    def observe(self, timeout):
        assert timeout > 0, "observation called with no budget"
        self.events.append("observe")
        if self.observe_error is not None:
            raise self.observe_error
        return [(pid, group, sid, state) for pid, (group, sid, state, _) in self.procs.items()]

    def identity(self, pid):
        if pid in self.identities:
            return self.identities[pid]
        if pid not in self.procs:
            raise ProcessLookupError(pid)
        return self.procs[pid][0], self.procs[pid][1]

    def killpg(self, group, number):
        self.signals.append((group, signal.Signals(number).name))
        members = [pid for pid, entry in self.procs.items() if entry[0] == group]
        if not members:
            raise ProcessLookupError(group)
        for pid in members:
            ignores = self.procs[pid][3]
            if ignores == "all" or (ignores and number == signal.SIGTERM):
                continue
            if pid == self.session:
                self.procs[pid] = [pid, None, "Z", False]
            else:
                del self.procs[pid]


def fake_supervisor(world, *, free=lambda: 100 * GIB, size=lambda: 0, leader=None, **options):
    directory = tempfile.mkdtemp(dir=SCRATCH)
    options.setdefault("limit", 60)
    options.setdefault("cleanup", 20)

    def sleep(seconds):
        # The real samplers keep running on threads while closure waits.
        world.clock.sleep(seconds)
        for sampler in (getattr(supervisor, "reserve", None), getattr(supervisor, "scratch_sampler", None)):
            if sampler is not None and not sampler.stop.is_set():
                sampler.sample_once()

    supervisor = bounded.Supervisor(
        ["fake"], term_grace=2, kill_grace=2, scratch=directory, owned=options.pop("owned", False),
        clock=world.clock, sleep=sleep, free_space=lambda _path: free(),
        scratch_size=lambda _path: size(), observe=world.observe, identity=world.identity,
        killpg=world.killpg, threaded=False,
        spawn=lambda command, env: leader if leader is not None else FakeLeader(world, exit_at=world.clock() + 1),
        **options,
    )
    return supervisor, directory


class PureMonitorCases(unittest.TestCase):
    def test_low_initial_reserve_refuses_before_spawn(self):
        world = World()
        spawned = []
        supervisor, _ = fake_supervisor(world, free=lambda: 21 * GIB)
        supervisor.spawn = lambda *args: spawned.append(args)
        self.assertEqual(supervisor.run(), 125)
        self.assertEqual(spawned, [])
        self.assertEqual(supervisor.receipt["primary"]["kind"], "refused")
        self.assertIn("abort threshold", supervisor.receipt["primary"]["detail"])

    def test_threshold_is_reserve_plus_margin(self):
        world = World()
        supervisor, _ = fake_supervisor(world, free=lambda: 22 * GIB - 1)
        self.assertEqual(supervisor.run(), 125)
        world = World()
        supervisor, _ = fake_supervisor(world, free=lambda: 22 * GIB)
        self.assertEqual(supervisor.run(), 0)

    def test_initial_measurement_error_refuses(self):
        def broken():
            raise OSError(5, "injected statvfs failure")

        world = World()
        supervisor, _ = fake_supervisor(world, free=broken)
        self.assertEqual(supervisor.run(), 125)
        self.assertIn("measurement failed", supervisor.receipt["primary"]["detail"])
        self.assertIsNone(supervisor.receipt["closure"])

    def test_reserve_drop_mid_run_cancels_and_closes(self):
        world = World()
        readings = iter([100 * GIB] * 5)
        leader = FakeLeader(world)
        supervisor, _ = fake_supervisor(world, free=lambda: next(readings, 21 * GIB), leader=leader)
        self.assertEqual(supervisor.run(), 125)
        self.assertEqual(supervisor.receipt["primary"]["kind"], "fault")
        self.assertEqual(supervisor.receipt["closure"]["state"], "closed")
        self.assertIn((500, "SIGTERM"), world.signals)
        self.assertEqual(supervisor.receipt["reserve"]["minimum"], 21 * GIB)

    def test_measurement_error_mid_run_faults(self):
        world = World()
        calls = []

        def flaky():
            calls.append(1)
            if len(calls) > 3:
                raise OSError(5, "injected")
            return 100 * GIB

        supervisor, _ = fake_supervisor(world, free=flaky, leader=FakeLeader(world))
        self.assertEqual(supervisor.run(), 125)
        self.assertIn("host free-space measurement failed", supervisor.receipt["primary"]["detail"])
        self.assertEqual(supervisor.receipt["closure"]["state"], "closed")

    def test_stale_sampler_faults(self):
        world = World()
        supervisor, _ = fake_supervisor(world)
        supervisor.threaded = True  # samples come only from the (absent) thread
        supervisor.started = world.clock()
        supervisor.reserve = bounded.Sampler(lambda: 100 * GIB, 0.25, world.clock)
        supervisor.scratch_sampler = bounded.Sampler(lambda: 0, 2.0, world.clock)
        supervisor.reserve.sample_once()
        supervisor.scratch_sampler.sample_once()
        world.clock.sleep(0.9)
        self.assertIsNone(supervisor.monitor_fault())
        world.clock.sleep(0.2)
        self.assertIn("host free-space sampler stale", supervisor.monitor_fault())
        world.clock.sleep(0.0)
        supervisor.reserve.sample_once()
        self.assertIn("stale", supervisor.monitor_fault(), "a fault never clears")

    def test_dip_between_checks_is_latched(self):
        world = World()
        supervisor, _ = fake_supervisor(world)
        supervisor.threaded = True
        supervisor.started = world.clock()
        readings = iter([100 * GIB, 10 * GIB, 100 * GIB])
        supervisor.reserve = bounded.Sampler(lambda: next(readings), 0.25, world.clock)
        supervisor.scratch_sampler = bounded.Sampler(lambda: 0, 2.0, world.clock)
        for _ in range(3):
            supervisor.reserve.sample_once()
        supervisor.scratch_sampler.sample_once()
        self.assertIn("below", supervisor.monitor_fault())

    def test_scratch_over_budget_faults(self):
        world = World()
        sizes = iter([0, 0, 0])
        supervisor, _ = fake_supervisor(
            world, size=lambda: next(sizes, 600 * 1024 * 1024), leader=FakeLeader(world)
        )
        self.assertEqual(supervisor.run(), 125)
        self.assertIn("over the", supervisor.receipt["primary"]["detail"])


class PureClosureCases(unittest.TestCase):
    def test_healthy_status_preserved_and_reaped_after_observation(self):
        world = World()
        supervisor, _ = fake_supervisor(world, leader=FakeLeader(world, exit_at=world.clock() + 1, status=7))
        self.assertEqual(supervisor.run(), 7)
        self.assertEqual(world.events[-2:], ["observe", "reap"])
        self.assertEqual(supervisor.receipt["closure"]["state"], "closed")

    def test_signal_status_maps_to_128_plus(self):
        world = World()
        supervisor, _ = fake_supervisor(world, leader=FakeLeader(world, exit_at=world.clock() + 1, status=-9))
        self.assertEqual(supervisor.run(), 137)

    def test_term_then_kill_separate_group_first(self):
        world = World()
        leader = FakeLeader(world)
        world.procs[600] = [600, 500, "S", True]  # Cargo-like group ignoring TERM
        supervisor, _ = fake_supervisor(world, leader=leader, limit=30, cleanup=20)
        self.assertEqual(supervisor.run(), 124)
        self.assertEqual(world.signals, [(600, "SIGTERM"), (500, "SIGTERM"), (600, "SIGKILL")])
        self.assertEqual(supervisor.receipt["closure"]["state"], "closed")
        self.assertLessEqual(supervisor.receipt["wall_s"], 30)

    def test_leftover_group_after_normal_exit_is_settled(self):
        world = World()
        world.procs[600] = [600, 500, "S", True]
        supervisor, _ = fake_supervisor(world)
        self.assertEqual(supervisor.run(), 0)
        self.assertEqual(world.signals, [(600, "SIGTERM"), (600, "SIGKILL")])
        self.assertEqual(supervisor.receipt["closure"]["settled"], 1)

    def test_survivor_after_kill_is_unclosed_and_scratch_kept(self):
        world = World()
        world.procs[600] = [600, 500, "D", "all"]
        supervisor, directory = fake_supervisor(world, owned=True)
        self.assertEqual(supervisor.run(), 125)
        closure = supervisor.receipt["closure"]
        self.assertEqual(closure["state"], "UNCLOSED")
        self.assertEqual(closure["survivors"], [[600, 600, 500, "D"]])
        self.assertTrue(os.path.isdir(directory))
        self.assertEqual(supervisor.receipt["primary"]["kind"], "exit")

    def test_nonzero_status_stays_nonzero_when_unclosed(self):
        world = World()
        world.procs[600] = [600, 500, "S", "all"]
        supervisor, _ = fake_supervisor(world, leader=FakeLeader(world, exit_at=world.clock() + 1, status=4))
        self.assertEqual(supervisor.run(), 4)
        self.assertEqual(supervisor.receipt["closure"]["state"], "UNCLOSED")

    def test_failed_process_listing_is_unclosed_with_primary_preserved(self):
        world = World()
        leader = FakeLeader(world)
        world.observe_error = subprocess.TimeoutExpired("ps", 5)
        supervisor, directory = fake_supervisor(world, leader=leader, limit=30, cleanup=20, owned=True)
        self.assertEqual(supervisor.run(), 124)
        receipt = supervisor.receipt
        self.assertEqual(receipt["primary"]["kind"], "deadline")
        self.assertEqual(receipt["closure"]["state"], "UNCLOSED")
        self.assertIn("observation", receipt["closure"]["errors"][0])
        # The leader's own group is still positively owned and is stopped.
        self.assertEqual(world.signals, [(500, "SIGKILL")])
        self.assertTrue(os.path.isdir(directory))
        self.assertTrue(any("retained" in error for error in receipt["cleanup_errors"]))

    def test_expired_deadline_makes_no_observation(self):
        world = World()

        class Stalled(FakeLeader):
            def exited(self, timeout=0.0):
                self.world.clock.sleep(1000)  # the main loop stalled past the deadline
                return super().exited(timeout)

        supervisor, _ = fake_supervisor(world, leader=Stalled(world, exit_at=world.clock() + 1))
        self.assertEqual(supervisor.run(), 125)
        self.assertNotIn("observe", world.events)
        self.assertIn("budget exhausted", supervisor.receipt["closure"]["errors"][0])

    def test_closure_budget_exhausted_mid_escalation(self):
        world = World()
        world.procs[600] = [600, 500, "S", "all"]
        supervisor, _ = fake_supervisor(world, limit=30, cleanup=10, leader=FakeLeader(world, exit_at=world.clock() + 19))
        supervisor.term_grace = supervisor.kill_grace = 100  # graces cannot exceed the deadline
        self.assertEqual(supervisor.run(), 125)
        self.assertEqual(supervisor.receipt["closure"]["state"], "UNCLOSED")
        self.assertLessEqual(world.clock() - supervisor.started, 30 + 0.2)

    def test_unknown_session_member_is_never_signalled(self):
        world = World()
        world.procs[601] = [500, None, "S", "all"]  # session unreadable (permission)
        supervisor, _ = fake_supervisor(world)
        self.assertEqual(supervisor.run(), 125)
        self.assertEqual(world.signals, [])
        self.assertEqual(supervisor.receipt["closure"]["survivors"], [[601, 500, None, "S"]])

    def test_recycled_group_is_never_signalled(self):
        world = World()
        world.procs[700] = [700, 500, "S", False]
        world.identities[700] = (700, 999)  # pid now belongs to another session
        supervisor, _ = fake_supervisor(world)
        self.assertEqual(supervisor.run(), 125)
        self.assertNotIn(700, [group for group, _ in world.signals])

    def test_pipe_held_outside_session_is_unclosed(self):
        world = World()
        supervisor, directory = fake_supervisor(world, leader=FakeLeader(world, exit_at=world.clock() + 1, pipes=False), owned=True)
        self.assertEqual(supervisor.run(), 125)
        self.assertIn("pipe", supervisor.receipt["closure"]["errors"][0])
        self.assertTrue(os.path.isdir(directory))

    def test_post_spawn_exception_still_closes(self):
        world = World()

        class Broken(bounded.Supervisor):
            def watch(self, leader):
                raise RuntimeError("injected")

        supervisor, _ = fake_supervisor(world, leader=FakeLeader(world))
        supervisor.__class__ = Broken
        self.assertEqual(supervisor.run(), 125)
        self.assertEqual(supervisor.receipt["primary"]["kind"], "exception")
        self.assertEqual(supervisor.receipt["closure"]["state"], "closed")
        self.assertIn((500, "SIGTERM"), world.signals)

    def test_spawn_failure_reports_without_closure(self):
        world = World()
        supervisor, _ = fake_supervisor(world)

        def fail(*_):
            raise FileNotFoundError("no such command")

        supervisor.spawn = fail
        self.assertEqual(supervisor.run(), 125)
        self.assertEqual(supervisor.receipt["primary"]["kind"], "spawn_failed")

    def test_cancellation_before_exit(self):
        world = World()
        supervisor, _ = fake_supervisor(world, leader=FakeLeader(world))
        supervisor.cancel(signal.SIGINT)
        self.assertEqual(supervisor.run(), 130)
        self.assertEqual(supervisor.receipt["closure"]["state"], "closed")


# Real tiny children ---------------------------------------------------------

SLEEPER = f"import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep({FALLBACK})"

# Leader starting a Cargo-like TERM-ignoring child in its own group, then
# exiting with the status in argv[2]. The child inherits both output pipes.
LEAVER = """
import os, subprocess, sys
child = subprocess.Popen([sys.executable, "-c", %r], preexec_fn=lambda: os.setpgid(0, 0))
open(sys.argv[1], "w").write(str(child.pid))
sys.exit(int(sys.argv[2]))
""" % SLEEPER

# `oxide`-shaped: forwards TERM to the child's group and waits for it.
FORWARDER = """
import os, signal, subprocess, sys
child = subprocess.Popen([sys.executable, "-c", %r], preexec_fn=lambda: os.setpgid(0, 0))
open(sys.argv[1], "w").write(str(child.pid))
signal.signal(signal.SIGTERM, lambda *_: os.killpg(child.pid, signal.SIGTERM))
child.wait()
""" % SLEEPER

# The group leader exits at once leaving a TERM-ignoring member; the parent
# waits for the group to empty, as `oxide` does.
ORPHAN = """
import os, signal, subprocess, sys, time
leader = subprocess.Popen([sys.executable, "-c",
    "import subprocess, sys; child = subprocess.Popen([sys.executable, '-c', %r]); "
    "open(sys.argv[1], 'w').write(str(child.pid))", sys.argv[1]],
    preexec_fn=lambda: os.setpgid(0, 0))
group = leader.pid
signal.signal(signal.SIGTERM, lambda *_: os.killpg(group, signal.SIGTERM))
leader.wait()
deadline = time.monotonic() + %d
while time.monotonic() < deadline:
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        break
    time.sleep(0.1)
""" % (SLEEPER, FALLBACK)


class Null:
    def __init__(self):
        self.bytes = 0

    def write(self, data):
        self.bytes += len(data)

    def flush(self):
        pass


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


class RealChildCases(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(dir=SCRATCH)).resolve()
        self.pid_file = self.dir / "child.pid"

    def supervise(self, script, *args, **options):
        options.setdefault("limit", 30)
        options.setdefault("cleanup", 10)
        options.setdefault("term_grace", 1)
        options.setdefault("kill_grace", 2)
        self.sinks = (Null(), Null())
        supervisor = bounded.Supervisor(
            [sys.executable, "-c", script, str(self.pid_file), *args],
            scratch=str(self.dir), owned=False, sinks=self.sinks, **options,
        )
        started = time.monotonic()
        code = supervisor.run()
        self.elapsed = time.monotonic() - started
        self.receipt = supervisor.receipt
        return code

    def assert_settled(self):
        """Closed by the supervisor, not by the fixture's fallback expiry."""
        self.assertEqual(self.receipt["closure"]["state"], "closed", self.receipt["closure"])
        self.assertLess(self.elapsed, FALLBACK / 3)
        if self.pid_file.exists():
            self.assertFalse(alive(int(self.pid_file.read_text())), "fixture child survived")

    def test_normal_exit_with_live_separate_group(self):
        self.assertEqual(self.supervise(LEAVER, "0"), 0)
        self.assert_settled()
        self.assertIn("SIGKILL", [name for _, name in self.receipt["closure"]["signals"]])
        self.assertEqual(self.receipt["closure"]["settled"], 1)

    def test_nonzero_exit_with_live_separate_group(self):
        self.assertEqual(self.supervise(LEAVER, "7"), 7)
        self.assert_settled()

    def test_timeout_reaches_term_ignoring_cargo_group(self):
        self.assertEqual(self.supervise(FORWARDER, limit=12, cleanup=10), 124)
        self.assert_settled()
        self.assertLessEqual(self.elapsed, 12 + 0.5)

    def test_timeout_reaches_group_after_its_leader_exits(self):
        self.assertEqual(self.supervise(ORPHAN, limit=12, cleanup=10), 124)
        self.assert_settled()

    def test_pipe_pressure_is_drained(self):
        script = (
            "import os, sys\n"
            "block = b'x' * 65536\n"
            "for _ in range(48 * 16): os.write(1, block)\n"
            "for _ in range(8 * 16): os.write(2, block)\n"
        )
        self.assertEqual(self.supervise(script), 0)
        self.assert_settled()
        self.assertEqual(self.receipt["output"]["bytes"], 56 * 1024 * 1024)
        self.assertGreater(self.receipt["output"]["discarded"], 0)

    def test_endless_writer_hits_the_deadline(self):
        script = "import os\nwhile True: os.write(1, b'y' * 65536)\n"
        self.assertEqual(self.supervise(script, limit=13, cleanup=10), 124)
        self.assert_settled()

    def test_injected_low_reserve_stops_a_real_child(self):
        readings = iter([100 * GIB] * 8)
        code = self.supervise(FORWARDER, free_space=lambda _path: next(readings, 1 * GIB))
        self.assertEqual(code, 125)
        self.assertEqual(self.receipt["primary"]["kind"], "fault")
        self.assert_settled()

    def test_post_spawn_exception_closes_a_real_child(self):
        class Broken(bounded.Supervisor):
            def watch(self, leader):
                deadline = time.monotonic() + 5
                while not Path(self.command[3]).exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                raise RuntimeError("injected after spawn")

        original = bounded.Supervisor
        bounded.Supervisor = Broken
        try:
            code = self.supervise(FORWARDER)
        finally:
            bounded.Supervisor = original
        self.assertEqual(code, 125)
        self.assertEqual(self.receipt["primary"]["kind"], "exception")
        self.assert_settled()

    def guardian_signal(self, number, script):
        receipt = self.dir / "receipt.json"
        with open(self.dir / "guardian.log", "wb") as log:
            guardian = subprocess.Popen(
                [sys.executable, str(SUPERVISOR), "--limit", "60", "--receipt", str(receipt), "--",
                 sys.executable, "-c", script, str(self.pid_file)],
                stdout=log, stderr=log, env=dict(os.environ, OXIDE_TASK_SCRATCH_ROOT=str(self.dir)),
            )
            try:
                deadline = time.monotonic() + 10
                while not self.pid_file.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                started = time.monotonic()
                guardian.send_signal(number)
                code = guardian.wait(timeout=45)
            finally:
                if guardian.poll() is None:
                    guardian.kill()
                    guardian.wait(5)
        self.elapsed = time.monotonic() - started
        self.receipt = json.loads(receipt.read_text())
        return code

    def test_sigterm_to_guardian_closes_term_ignoring_group(self):
        self.assertEqual(self.guardian_signal(signal.SIGTERM, FORWARDER), 143)
        self.assertEqual(self.receipt["primary"]["kind"], "signal")
        self.assert_settled()

    def test_sigint_to_guardian_closes_the_session(self):
        self.assertEqual(self.guardian_signal(signal.SIGINT, LEAVER.replace("sys.exit(int(sys.argv[2]))", "child.wait()")), 130)
        self.assert_settled()


def write(name, size):
    with open(SCRATCH / name, "wb") as handle:
        handle.write(os.urandom(size))


class Results(unittest.TextTestResult):
    cases = []

    def addSuccess(self, test):
        super().addSuccess(test)
        Results.cases.append([test.id(), "pass"])

    def addFailure(self, test, err):
        super().addFailure(test, err)
        Results.cases.append([test.id(), "fail"])

    def addError(self, test, err):
        super().addError(test, err)
        Results.cases.append([test.id(), "error"])


def main():
    signal.alarm(240)  # secondary bound; the supervisor's deadline is primary
    loader = unittest.defaultTestLoader
    runner = unittest.TextTestRunner(verbosity=2, resultclass=Results)
    # Fault injection first: fake clock, table, signals and readings only.
    pure = unittest.TestSuite(loader.loadTestsFromTestCase(case) for case in (PureMonitorCases, PureClosureCases))
    ok = runner.run(pure).wasSuccessful()
    if sys.argv[1:] != ["--pure"]:
        # Writers before, during and after a simulated staging step, all in
        # the outer supervisor's scratch so its monitors see them.
        write("before.bin", 1024 * 1024)
        staged = str(SCRATCH / "staged.bin")
        subprocess.run([sys.executable, "-c", f"open({staged!r}, 'wb').write(bytes(4 * 1024 * 1024))"],
                       check=True, timeout=30)
        time.sleep(2.5)  # at least one complete scratch sample sees the staging
        ok = runner.run(loader.loadTestsFromTestCase(RealChildCases)).wasSuccessful() and ok
        write("after.bin", 1024 * 1024)
        cases_path = os.environ.get("OXIDE_SUPERVISOR_CASES")
        if cases_path:
            with open(cases_path, "w") as handle:
                json.dump(Results.cases, handle, indent=1)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
