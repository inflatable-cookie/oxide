#!/usr/bin/env python3
"""Shared session supervisor for repository tests and native proofs (oxide#109).

    bounded-test.py [--limit S] [--scratch-budget-mib N] [--receipt PATH] [--] command ...

One supervisor owns the whole proof command, from before its first writer
starts through child closure and owned scratch cleanup:

- One absolute deadline (default 300 s) covers the command, closure and
  cleanup. Work stops `CLEANUP_SECONDS` before it; every closure step uses
  only what is left. Nothing runs after it unbounded.
- Raw host free space (`statvfs` `f_bavail`) is sampled every 0.25 s on a
  thread. A reading below the 22 GiB abort threshold (20 GiB reserve plus
  2 GiB margin), a failed sample or no sample for 1 s latches a fault and
  cancels the command; a low or missing first reading refuses before spawn.
  This is a sampled abort threshold, not an instantaneous guarantee against
  unrelated writers.
- The scratch directory (owned: created here and passed as
  `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP`; or adopted from a
  caller's `OXIDE_TASK_SCRATCH_ROOT`) is walked every 2 s. A complete sample
  over the budget (default 512 MiB), a failed walk or none for 10 s faults.
  The budget is monitored and reported, not a hard ceiling.
- The command starts in a new session whose ID is the leader's PID. Its exit
  is detected without reaping (kqueue `NOTE_EXIT`), so that ID cannot be
  reused while closure runs. On every path (exit, timeout, fault, SIGINT or
  SIGTERM to the supervisor, exception after spawn) the session is observed
  afresh, every group in it is TERMed, then KILLed, Cargo-style separate
  groups before the leader's, then observed again. Closure is positive only
  when no session member remains, the leader is reaped and both output pipes
  reached EOF. Observation, signalling or reaping errors, survivors, an
  exhausted budget or a pipe still held elsewhere leave it UNCLOSED: the exit
  is nonzero and owned scratch is kept.
- Scope is the cooperative session: a process that calls `setsid` leaves it
  and is not seen unless it still holds an output pipe. Unrelated same-user
  writers are outside the promise.

Exit: the command's own status when healthy and closed; 124 deadline; 125
guard fault, refusal, exception or UNCLOSED with an otherwise zero status;
128+N on signal N. Output is forwarded on threads; past 32 MiB per stream it
is drained and discarded.
"""
from __future__ import annotations

import errno
import json
import os
import platform
import select
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time

GIB = 1024 ** 3
MIB = 1024 ** 2
LIMIT_SECONDS = 300
CLEANUP_SECONDS = 30
TERM_GRACE = 10
KILL_GRACE = 5
RESERVE_BYTES = 20 * GIB
MARGIN_BYTES = 2 * GIB
RESERVE_INTERVAL = 0.25
RESERVE_STALE = 1.0
SCRATCH_BUDGET = 512 * MIB
SCRATCH_INTERVAL = 2.0
SCRATCH_STALE = 10.0
OUTPUT_FORWARD = 32 * MIB
PS_TIMEOUT = 5.0
POLL = 0.05


def free_bytes(path):
    sample = os.statvfs(path)
    return sample.f_bavail * sample.f_frsize


def tree_bytes(root):
    """Allocated bytes under `root`, hard links once; vanished entries skipped."""
    total, seen, stack = 0, set(), [root]
    while stack:
        path = stack.pop()
        try:
            meta = os.lstat(path)
        except FileNotFoundError:
            continue
        if (meta.st_dev, meta.st_ino) in seen:
            continue
        seen.add((meta.st_dev, meta.st_ino))
        total += meta.st_blocks * 512
        if stat.S_ISDIR(meta.st_mode):
            try:
                with os.scandir(path) as entries:
                    stack.extend(entry.path for entry in entries)
            except FileNotFoundError:
                continue
    return total


def ps_table(timeout):
    """[(pid, pgid, sid or None, state)] for every visible process. A getsid
    failure on a live process leaves its session unknown (None)."""
    listing = subprocess.run(
        ["/bin/ps", "-A", "-o", "pid=,pgid=,stat="],
        capture_output=True, text=True, timeout=timeout, check=True,
    ).stdout
    table = []
    for line in listing.splitlines():
        pid, group, state = line.split(None, 2)
        pid, group = int(pid), int(group)
        try:
            session = os.getsid(pid)
        except ProcessLookupError:
            if not state.startswith("Z"):
                continue  # exited between ps and getsid
            session = None  # zombies have no readable session
        except PermissionError:
            session = None
        table.append((pid, group, session, state.strip()))
    return table


def identity(pid):
    return os.getpgid(pid), os.getsid(pid)


class Sampler:
    """Runs `measure` every `interval` on a daemon thread; the supervisor
    reads the latest state. Stopped threads are abandoned, never joined."""

    def __init__(self, measure, interval, clock):
        self.measure, self.interval, self.clock = measure, interval, clock
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.value = self.minimum = self.maximum = self.first = self.last = None
        self.error = None
        self.count = 0
        self.max_gap = 0.0

    def sample_once(self):
        try:
            value = self.measure()
        except Exception as error:  # any failed measurement is a fault
            with self.lock:
                if self.error is None:
                    self.error = f"{type(error).__name__}: {error}"
            return
        now = self.clock()
        with self.lock:
            if self.last is not None:
                self.max_gap = max(self.max_gap, now - self.last)
            if self.first is None:
                self.first = now
            self.value, self.last = value, now
            self.count += 1
            self.minimum = value if self.minimum is None else min(self.minimum, value)
            self.maximum = value if self.maximum is None else max(self.maximum, value)

    def start(self):
        def loop():
            while not self.stop.is_set():
                self.sample_once()
                self.stop.wait(self.interval)

        threading.Thread(target=loop, daemon=True).start()


class Drain:
    """Reads one pipe to EOF on a daemon thread, forwarding a bounded prefix."""

    def __init__(self, pipe, sink):
        self.pipe, self.sink = pipe, sink
        self.eof = threading.Event()
        self.bytes = self.discarded = 0
        threading.Thread(target=self.run, daemon=True).start()

    def run(self):
        fd = self.pipe.fileno()
        try:
            while True:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                self.bytes += len(chunk)
                if self.sink is None or self.bytes > OUTPUT_FORWARD:
                    self.discarded += len(chunk)
                    continue
                try:
                    self.sink.write(chunk)
                    self.sink.flush()
                except (BrokenPipeError, ValueError, OSError):
                    self.sink = None
        finally:
            self.eof.set()


class Leader:
    """The supervised command: own session, piped output, exit seen without
    reaping so its PID (the session ID) stays reserved until `reap`."""

    def __init__(self, command, env, sinks):
        self.process = subprocess.Popen(
            command, start_new_session=True, env=env,
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.pid = self.process.pid
        self.drains = [Drain(self.process.stdout, sinks[0]), Drain(self.process.stderr, sinks[1])]
        self.done = False
        try:
            self.queue = select.kqueue()
            event = select.kevent(
                self.pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_ONESHOT, select.KQ_NOTE_EXIT
            )
            self._take(self.queue.control([event], 1, 0))
        except BaseException:
            # Without exit notification the session cannot be closed safely
            # later; stop the leader's own group now, while it is unreaped.
            os.killpg(self.pid, signal.SIGKILL)
            self.process.wait(KILL_GRACE)
            raise

    def _take(self, events):
        for event in events:
            if event.flags & select.KQ_EV_ERROR:
                if event.data != errno.ESRCH:
                    raise OSError(event.data, "kqueue registration failed")
            self.done = True  # NOTE_EXIT, or ESRCH: already a zombie

    def exited(self, timeout=0.0):
        if not self.done:
            self._take(self.queue.control(None, 1, max(timeout, 0.0)))
        return self.done

    def reap(self, timeout):
        try:
            status = self.process.wait(timeout=max(timeout, 0.0))
        except subprocess.TimeoutExpired:
            return None
        self.queue.close()
        return status

    def output_closed(self, timeout):
        end = time.monotonic() + max(timeout, 0.0)
        for drain in self.drains:
            if not drain.eof.wait(max(end - time.monotonic(), 0.0)):
                return False
        return True

    def output(self):
        return {"bytes": sum(d.bytes for d in self.drains), "discarded": sum(d.discarded for d in self.drains)}


class Supervisor:
    def __init__(
        self, command, *, limit=LIMIT_SECONDS, cleanup=CLEANUP_SECONDS, term_grace=TERM_GRACE,
        kill_grace=KILL_GRACE, scratch=None, owned=None, scratch_budget=SCRATCH_BUDGET,
        threshold=RESERVE_BYTES + MARGIN_BYTES, clock=time.monotonic, sleep=time.sleep,
        free_space=free_bytes, scratch_size=tree_bytes, observe=ps_table, identity=identity,
        killpg=os.killpg, spawn=None, sinks=None, threaded=True,
    ):
        if not limit > cleanup >= term_grace + kill_grace + 3:
            raise ValueError("need limit > cleanup >= term grace + kill grace + 3 s")
        self.command, self.limit, self.cleanup = command, limit, cleanup
        self.term_grace, self.kill_grace = term_grace, kill_grace
        self.scratch, self.owned, self.scratch_budget = scratch, owned, scratch_budget
        self.threshold, self.clock, self.sleep = threshold, clock, sleep
        self.free_space, self.scratch_size = free_space, scratch_size
        self.observe, self.identity, self.killpg = observe, identity, killpg
        self.spawn = spawn or (lambda command, env: Leader(command, env, self.sinks))
        self.sinks = sinks or (sys.stdout.buffer, sys.stderr.buffer)
        self.threaded = threaded
        self.cancelled = None
        self.fault = None
        self.receipt = {}

    def cancel(self, number):
        if self.cancelled is None:
            self.cancelled = number

    # Monitors -----------------------------------------------------------

    def monitor_fault(self, scratch=True):
        """The first monitor fault, or None. Faults latch: they are judged on
        each sampler's whole history (first error, minimum, maximum, largest
        gap), so a reading between checks is not lost and nothing clears."""
        if self.fault is not None:
            return self.fault
        checks = [(self.reserve, "host free-space", RESERVE_STALE)]
        if scratch:
            checks.append((self.scratch_sampler, "scratch", SCRATCH_STALE))
        if not self.threaded:
            for sampler, _, _ in checks:
                sampler.sample_once()
        now = self.clock()
        for sampler, name, stale in checks:
            with sampler.lock:
                error, last, gap = sampler.error, sampler.last, sampler.max_gap
                minimum, maximum = sampler.minimum, sampler.maximum
            if error is not None:
                self.fault = f"{name} measurement failed: {error}"
            elif max(gap, now - (self.started if last is None else last)) > stale:
                self.fault = f"{name} sampler stale: no sample for {stale:g} s"
            elif sampler is self.reserve and minimum is not None and minimum < self.threshold:
                self.fault = f"host free {minimum} bytes is below the {self.threshold} byte abort threshold"
            elif sampler is self.scratch_sampler and maximum is not None and maximum > self.scratch_budget:
                self.fault = f"scratch {maximum} bytes is over the {self.scratch_budget} byte budget"
            if self.fault is not None:
                return self.fault
        return None

    # Closure ------------------------------------------------------------

    def remaining(self):
        return self.deadline - self.clock()

    def members(self, session, groups, leader_done):
        """Fresh session members, excluding the exited (unreaped) leader. A
        member whose session is unreadable counts when its group is known."""
        if self.remaining() <= 0:
            raise TimeoutError("closure budget exhausted before observation")
        found = []
        for pid, group, sid, state in self.observe(min(PS_TIMEOUT, self.remaining())):
            if pid == session and leader_done:
                continue
            if sid == session:
                groups.add(group)
                found.append((pid, group, sid, state))
            elif sid is None and group in groups:
                found.append((pid, group, sid, state))
        return found

    def signal_groups(self, session, found, number, closure):
        """Signal each group with a member positively in this session, verified
        again just before the signal. Separate groups go before the leader's."""
        targets = {}
        for pid, group, sid, _ in found:
            if sid == session:
                targets.setdefault(group, pid)
        for group in sorted(targets, key=lambda g: g == session):
            if group != session:
                # The leader is unreaped, so its group cannot be recycled;
                # any other group is checked again through a live member.
                try:
                    if self.identity(targets[group]) != (group, session):
                        continue  # moved or gone; the next observation decides
                except ProcessLookupError:
                    continue
                except OSError as error:
                    closure["errors"].append(f"identity of group {group}: {error}")
                    continue
            try:
                self.killpg(group, number)
                closure["signals"].append([group, signal.Signals(number).name])
            except ProcessLookupError:
                pass
            except OSError as error:
                closure["errors"].append(f"signal {number} to group {group}: {error}")

    def close(self, leader):
        session = leader.pid
        closure = {"state": "UNCLOSED", "signals": [], "errors": [], "survivors": [], "settled": 0}
        groups = {session}
        found = None
        try:
            done = leader.exited(0)
            found = self.members(session, groups, done)
            first = len(found)
            for number, grace in ((signal.SIGTERM, self.term_grace), (signal.SIGKILL, self.kill_grace)):
                if not found:
                    break
                self.signal_groups(session, found, number, closure)
                end = min(self.clock() + grace, self.deadline)
                while True:
                    done = leader.exited(0)
                    found = self.members(session, groups, done)
                    if not found or self.clock() >= end:
                        break
                    self.sleep(0.1)
            closure["settled"] = first - len(found)
        except Exception as error:
            closure["errors"].append(f"observation: {type(error).__name__}: {error}")
            found = None
            # The leader's own group stays positively owned while it is
            # unreaped; stop it even though the session cannot be seen.
            if not leader.exited(0):
                try:
                    self.killpg(session, signal.SIGKILL)
                    closure["signals"].append([session, "SIGKILL"])
                except OSError as kill_error:
                    closure["errors"].append(f"signal 9 to group {session}: {kill_error}")
                leader.exited(min(self.kill_grace, max(self.remaining(), 0.0)))
        if found:
            closure["survivors"] = [list(member) for member in found]
        status = None
        if leader.exited(0):
            status = leader.reap(min(1.0, max(self.remaining(), 0.0)))
            if status is None:
                closure["errors"].append("leader exit status not collected")
        else:
            closure["errors"].append("leader still running after KILL")
        pipes = leader.output_closed(min(2.0, max(self.remaining(), 0.0)))
        if not pipes:
            closure["errors"].append("an output pipe is still held open (possibly outside the session)")
        if found == [] and status is not None and pipes and not closure["errors"]:
            closure["state"] = "closed"
        return status, closure

    # Run ----------------------------------------------------------------

    def prepare_scratch(self):
        if self.scratch is None:
            adopted = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
            if adopted:
                self.scratch, self.owned = os.path.realpath(adopted), False
                if not os.path.isdir(self.scratch):
                    raise FileNotFoundError(f"OXIDE_TASK_SCRATCH_ROOT {self.scratch} is not a directory")
            else:
                base = "/private/tmp" if sys.platform == "darwin" else tempfile.gettempdir()
                self.scratch = os.path.realpath(tempfile.mkdtemp(prefix="oxide-proof.", dir=base))
                self.owned = True
        env = dict(os.environ)
        if self.owned:
            for name in ("OXIDE_TASK_SCRATCH_ROOT", "TMPDIR", "TMP", "TEMP"):
                env[name] = self.scratch
        return env

    def remove_scratch(self, receipt):
        if not self.owned:
            return
        thread = threading.Thread(target=shutil.rmtree, args=(self.scratch,), daemon=True)
        thread.start()
        thread.join(max(self.remaining(), 0.0))
        if os.path.lexists(self.scratch):
            receipt["cleanup_errors"].append(f"owned scratch {self.scratch} not fully removed")
        else:
            receipt["scratch_removed"] = True

    def run(self):
        self.started = self.clock()
        self.deadline = self.started + self.limit
        receipt = self.receipt
        receipt.update({
            "command": self.command, "limit_s": self.limit, "cleanup_reserved_s": self.cleanup,
            "primary": None, "closure": None, "cleanup_errors": [], "scratch_removed": False,
        })
        env = self.prepare_scratch()
        # Sample the scratch's file system at a path that outlives its removal.
        reserve_path = os.path.dirname(self.scratch) if self.owned else self.scratch
        receipt.update({"scratch": self.scratch, "scratch_owned": self.owned, "reserve_path": reserve_path})
        self.reserve = Sampler(lambda: self.free_space(reserve_path), RESERVE_INTERVAL, self.clock)
        self.scratch_sampler = Sampler(lambda: self.scratch_size(self.scratch), SCRATCH_INTERVAL, self.clock)
        if self.threaded:
            self.reserve.start()
            self.scratch_sampler.start()
            while self.reserve.count == 0 and self.reserve.error is None:
                if self.clock() - self.started > RESERVE_STALE:
                    break
                self.sleep(POLL)
        leader = status = None
        fault = self.monitor_fault()
        if self.reserve.count == 0 and fault is None:
            fault = self.fault = "no initial host free-space reading"
        if fault is not None:
            receipt["primary"] = {"kind": "refused", "detail": fault}
        else:
            try:
                receipt["spawned_at_s"] = self.clock() - self.started
                leader = self.spawn(self.command, env)
            except Exception as error:
                receipt["primary"] = {"kind": "spawn_failed", "detail": f"{type(error).__name__}: {error}"}
        if leader is not None:
            try:
                receipt["primary"] = self.watch(leader)
            except BaseException as error:  # post-spawn exception, closure still runs
                receipt["primary"] = {"kind": "exception", "detail": f"{type(error).__name__}: {error}"}
            status, receipt["closure"] = self.close(leader)
            receipt["closed_at_s"] = self.clock() - self.started
            receipt["output"] = getattr(leader, "output", lambda: None)()
        closed = leader is None or receipt["closure"]["state"] == "closed"
        # A fault first seen during closure fails an otherwise healthy run.
        late = self.monitor_fault() if self.fault is None else None
        self.scratch_sampler.stop.set()
        if closed:
            self.remove_scratch(receipt)
        else:
            receipt["cleanup_errors"].append(f"closure not positive; scratch {self.scratch} retained")
        if late is None and self.fault is None:
            late = self.monitor_fault(scratch=False)
        self.reserve.stop.set()
        if late is not None:
            receipt["late_fault"] = late
        receipt["status"] = status
        receipt["exit_code"] = self.exit_code(status, closed, late)
        receipt["wall_s"] = round(self.clock() - self.started, 3)
        receipt["reserve"] = self.sampler_report(self.reserve, RESERVE_INTERVAL)
        receipt["reserve"]["abort_threshold_bytes"] = self.threshold
        receipt["scratch_usage"] = self.sampler_report(self.scratch_sampler, SCRATCH_INTERVAL)
        receipt["scratch_usage"]["budget_bytes"] = self.scratch_budget
        return receipt["exit_code"]

    def watch(self, leader):
        work_end = self.deadline - self.cleanup
        while True:
            if leader.exited(POLL):
                return {"kind": "exit"}
            if self.cancelled is not None:
                return {"kind": "signal", "detail": signal.Signals(self.cancelled).name, "number": self.cancelled}
            fault = self.monitor_fault()
            if fault is not None:
                return {"kind": "fault", "detail": fault}
            if self.clock() >= work_end:
                return {"kind": "deadline", "detail": f"work exceeded {self.limit - self.cleanup:g} s"}

    def exit_code(self, status, closed, late):
        kind = self.receipt["primary"]["kind"]
        if kind == "deadline":
            code = 124
        elif kind == "signal":
            code = 128 + self.receipt["primary"]["number"]
        elif kind == "exit" and status is not None and late is None:
            code = status if status >= 0 else 128 - status
        else:
            code = 125
        return 125 if code == 0 and not closed else code

    def sampler_report(self, sampler, interval):
        start = self.started
        return {
            "interval_s": interval, "samples": sampler.count, "minimum": sampler.minimum,
            "maximum": sampler.maximum, "max_gap_s": round(sampler.max_gap, 3), "error": sampler.error,
            "first_at_s": None if sampler.first is None else round(sampler.first - start, 3),
            "last_at_s": None if sampler.last is None else round(sampler.last - start, 3),
        }


def describe(receipt):
    primary, closure = receipt["primary"], receipt["closure"]
    lines = [f"supervisor: exit {receipt['exit_code']}; {primary['kind']}"
             + (f": {primary['detail']}" if primary.get("detail") else "")
             + (f" (command status {receipt['status']})" if receipt["status"] is not None else "")
             + f"; wall {receipt['wall_s']} s of {receipt['limit_s']} s"]
    if closure is not None:
        lines.append(f"supervisor: closure {closure['state']}; settled {closure['settled']} remaining"
                     f" process(es); signals {closure['signals']}")
        for error in closure["errors"]:
            lines.append(f"supervisor: closure error: {error}")
        if closure["survivors"]:
            lines.append(f"supervisor: survivors [pid, pgid, sid, state]: {closure['survivors']}")
    reserve = receipt["reserve"]
    minimum = "none" if reserve["minimum"] is None else f"{reserve['minimum'] / GIB:.1f} GiB"
    lines.append(f"supervisor: host free sampled {reserve['samples']} times every {reserve['interval_s']} s"
                 f" (largest gap {reserve['max_gap_s']} s), minimum {minimum}, abort threshold"
                 f" {reserve['abort_threshold_bytes'] / GIB:g} GiB; sampled, not an instantaneous guarantee")
    usage = receipt["scratch_usage"]
    peak = "none" if usage["maximum"] is None else f"{usage['maximum'] / MIB:.1f} MiB"
    lines.append(f"supervisor: scratch peak {peak} of {usage['budget_bytes'] // MIB} MiB monitored budget"
                 f" ({usage['samples']} complete samples); {receipt['scratch']}"
                 + (" removed" if receipt["scratch_removed"] else ""))
    if receipt.get("late_fault"):
        lines.append(f"supervisor: fault during closure: {receipt['late_fault']}")
    for error in receipt["cleanup_errors"]:
        lines.append(f"supervisor: cleanup: {error}")
    return lines


def parse(argv):
    options = {"limit": LIMIT_SECONDS, "scratch_budget": SCRATCH_BUDGET, "receipt": None}
    while argv and argv[0].startswith("--"):
        flag = argv.pop(0)
        if flag == "--":
            break
        if flag not in ("--limit", "--scratch-budget-mib", "--receipt") or not argv:
            raise SystemExit(f"bounded-test.py: unknown or incomplete option {flag}")
        value = argv.pop(0)
        if flag == "--limit":
            options["limit"] = float(value)
        elif flag == "--scratch-budget-mib":
            options["scratch_budget"] = int(value) * MIB
        else:
            options["receipt"] = value
    if not argv:
        raise SystemExit("bounded-test.py: no command")
    return argv, options


def main(argv):
    command, options = parse(list(argv))
    if not hasattr(select, "kqueue"):
        raise SystemExit("bounded-test.py: needs kqueue process-exit notification (macOS)")
    if options["limit"] <= CLEANUP_SECONDS:
        raise SystemExit(f"bounded-test.py: --limit must exceed the {CLEANUP_SECONDS} s cleanup reserve")
    supervisor = Supervisor(command, limit=options["limit"], scratch_budget=options["scratch_budget"])
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, lambda n, _frame: supervisor.cancel(n))
    code = supervisor.run()
    supervisor.receipt["platform"] = {"system": platform.platform(), "python": platform.python_version()}
    for line in describe(supervisor.receipt):
        print(line, file=sys.stderr, flush=True)
    if options["receipt"]:
        with open(options["receipt"], "w") as handle:
            json.dump(supervisor.receipt, handle, indent=2)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
