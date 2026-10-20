"""Guardian for one bounded focused proof session (oxide#097).

Used only by scripts/test-oxide-removable-pool.py. One object owns:
- a sticky first fault (reserve, storage, output, cancellation, observation,
  deadline); nothing clears it and no experimental child starts after it;
- a 250 ms host-reserve sampler (20 GiB raw threshold; a failed sample, a
  dead sampler or a successful sample older than 1 s faults);
- an owned-byte observer that starts a complete walk at least every 2 s,
  finishes within 1 s and refuses a successful sample older than 4 s. On
  ENOENT the whole incomplete total is discarded and the walk restarts once
  from the owned roots inside the same 1 s deadline; a second disappearance,
  any other error or the deadline faults. A positively identified mount is
  counted by its used blocks; an unresolved attachment is never zero;
- continuous bounded draining of both raw streams against the aggregate
  output allowance;
- absolute deadlines from admission (work, then total through teardown);
- the single supervised spawn path, with unconditional bounded closure
  (TERM, then KILL, then a fresh session/group observation) on every
  post-launch path. A session not positively closed stays UNCLOSED and
  blocks every detach.

This is bounded periodic detection, not an OS-enforced ceiling: overshoot
between samples is possible and any sample over a bound fails the proof.
"""

from __future__ import annotations

import errno
import os
import signal
import stat as stat_mode
import subprocess
import threading
import time

MIB = 1024 * 1024
RESERVE_INTERVAL = 0.25
RESERVE_STALE = 1.0
WALK_INTERVAL = 2.0
WALK_DEADLINE = 1.0
WALK_STALE = 4.0
TERM_GRACE = 5.0
KILL_GRACE = 5.0
PS_TIMEOUT = 5.0
DRAIN_GRACE = 2.0
POLL = 0.05


class Fault(RuntimeError):
    """The guardian's first fault, raised to stop experimental work."""


class Cancelled(BaseException):
    """SIGTERM or SIGINT reached the runner."""


def install_cancellation() -> None:
    def cancel(number, _frame):
        raise Cancelled(f"signal {number}")

    signal.signal(signal.SIGTERM, cancel)
    signal.signal(signal.SIGINT, cancel)


def ps_table(timeout: float = PS_TIMEOUT) -> list:
    """[(pid, pgid, sid or None)]; raises on any observation failure."""
    listing = subprocess.run(
        ["/bin/ps", "-A", "-o", "pid=,pgid="], capture_output=True, text=True, timeout=timeout, check=True
    )
    table = []
    for line in listing.stdout.splitlines():
        pid, group = map(int, line.split())
        try:
            session = os.getsid(pid)
        except (ProcessLookupError, PermissionError):
            session = None
        table.append((pid, group, session))
    return table


def host_free(path: str) -> int:
    stat = os.statvfs(path)
    return stat.f_bavail * stat.f_frsize


class Walker:
    """Whole-walk owned-byte total over `roots` plus counted mounts."""

    def __init__(
        self,
        roots: list,
        clock=time.monotonic,
        lstat=os.lstat,
        scandir=os.scandir,
        stat=os.stat,
        statvfs=os.statvfs,
        ismount=os.path.ismount,
    ):
        self.roots = roots
        self.clock = clock
        self.lstat = lstat
        self.scandir = scandir
        self.stat = stat
        self.statvfs = statvfs
        self.ismount = ismount
        # Set before guarded removal of the owned roots: the last complete
        # total, reported as a conservative upper bound while removal only
        # shrinks it.
        self.frozen = None
        # Mounts: path -> st_dev of the positively identified mount root.
        self.mounts = {}
        # Attach intent recorded before mutation and not yet resolved:
        # (image, bound). Counted at its whole capacity bound, never zero.
        self.unresolved_attach = None
        # Mounts being detached: counted at their last used bytes until the
        # detach is confirmed.
        self.held = {}

    def _late(self, deadline: float) -> None:
        if self.clock() > deadline:
            raise TimeoutError("owned-byte observation finished after its 1 s deadline")

    def _walk_once(self, deadline: float) -> int:
        total = 0
        stack = [root for root in self.roots if os.path.lexists(root)]
        self._late(deadline)
        mounts = dict(self.mounts)
        held = dict(self.held)
        pending = self.unresolved_attach
        while stack:
            self._late(deadline)
            current = stack.pop()
            if current in mounts:
                continue
            metadata = self.lstat(current)
            self._late(deadline)
            total += max(metadata.st_size, metadata.st_blocks * 512)
            if stat_mode.S_ISDIR(metadata.st_mode):
                with self.scandir(current) as entries:
                    for entry in entries:
                        stack.append(entry.path)
                self._late(deadline)
        total += sum(held.values())
        if pending is not None:
            total += pending[1]
        for mount, device in mounts.items():
            metadata = self.stat(mount)
            self._late(deadline)
            if metadata.st_dev != device or not self.ismount(mount):
                raise OSError(errno.ENODEV, f"{mount} is no longer the identified mount")
            self._late(deadline)
            sample = self.statvfs(mount)
            self._late(deadline)
            total += (sample.f_blocks - sample.f_bfree) * sample.f_frsize
        self._late(deadline)
        return total

    def mount_used(self, mount: str) -> int:
        stat = os.statvfs(mount)
        return (stat.f_blocks - stat.f_bfree) * stat.f_frsize

    def hold(self, mount: str) -> None:
        """Before an ordinary detach: keep counting the mount's last used
        bytes until `release` confirms it is gone."""
        self.held[mount] = self.mount_used(mount)
        self.mounts.pop(mount, None)

    def release(self, mount: str) -> None:
        if os.path.ismount(mount) or os.path.lexists(mount):
            raise OSError(errno.EBUSY, f"{mount} is still present")
        self.held.pop(mount, None)

    def sample(self) -> int:
        """At most two complete attempts within one 1 s deadline."""
        if self.frozen is not None:
            return self.frozen
        deadline = self.clock() + WALK_DEADLINE
        for attempt in (1, 2):
            try:
                total = self._walk_once(deadline)
                # One deadline across both attempts, checked before publishing.
                self._late(deadline)
                return total
            except FileNotFoundError:
                if attempt == 2:
                    raise
                # Discard the partial total and walk again from the roots.
        raise AssertionError("unreachable")


class Guardian:
    def __init__(
        self,
        started: float,
        reserve_path: str,
        walker: Walker,
        *,
        reserve_bytes: int = 20 * 1024 * MIB,
        cap_bytes: int = 512 * MIB,
        output_bytes: int = 32 * MIB,
        work_seconds: float = 240,
        total_seconds: float = 300,
        clock=time.monotonic,
        free=host_free,
        table=ps_table,
        outer: Guardian | None = None,
        threads: bool = True,
    ):
        self.started = started
        self.reserve_path = reserve_path
        self.walker = walker
        self.reserve_bytes = reserve_bytes
        self.cap_bytes = cap_bytes
        self.output_cap = output_bytes
        self.work_deadline = started + work_seconds
        self.total_deadline = started + total_seconds
        self.clock = clock
        self.free = free
        self.table = table
        self.outer = outer
        self.lock = threading.Lock()
        self.first = None
        self.output = 0
        self.unclosed = set()
        self.children = {}
        self.cleanup_failures = []
        self.reserve_at = None
        self.reserve_min = None
        self.walk_at = None
        self.walk_max = 0
        self.walk_count = 0
        self.stop = threading.Event()
        self.threads = []
        if threads:
            for target in (self._reserve_loop, self._walk_loop):
                thread = threading.Thread(target=target, daemon=True)
                thread.start()
                self.threads.append(thread)

    # Faults ---------------------------------------------------------------
    def fault(self, kind: str, message: str) -> None:
        with self.lock:
            if self.first is None:
                self.first = f"{kind}: {message}"

    def check(self, experimental: bool = True) -> None:
        """Raise the sticky fault; refresh liveness/staleness/deadline faults."""
        now = self.clock()
        if self.threads and not all(thread.is_alive() for thread in self.threads):
            self.fault("observation", "a monitor thread stopped")
        if self.reserve_at is None or now - self.reserve_at > RESERVE_STALE:
            if now - self.started > RESERVE_STALE:
                self.fault("reserve", "no successful reserve sample within 1 s")
        if self.walk_at is None or now - self.walk_at > WALK_STALE:
            if now - self.started > WALK_STALE:
                self.fault("storage", "no successful owned-byte sample within 4 s")
        if experimental and now > self.work_deadline:
            self.fault("deadline", "work deadline passed")
        if now > self.total_deadline:
            self.fault("deadline", "total deadline passed")
        if self.first is not None:
            raise Fault(self.first)

    # Monitors -------------------------------------------------------------
    def sample_reserve(self) -> None:
        try:
            free = self.free(self.reserve_path)
        except Exception as error:  # any failed observation faults
            self.fault("reserve", f"sample failed: {error}")
            return
        self.reserve_min = free if self.reserve_min is None else min(self.reserve_min, free)
        if free < self.reserve_bytes:
            self.fault("reserve", f"host free {free} below {self.reserve_bytes}")
            return
        self.reserve_at = self.clock()

    def sample_storage(self) -> None:
        try:
            total = self.walker.sample()
        except Exception as error:
            self.fault("storage", f"owned-byte walk failed: {error!r}")
            return
        self.walk_count += 1
        self.walk_max = max(self.walk_max, total)
        if total > self.cap_bytes:
            self.fault("storage", f"owned bytes {total} exceed {self.cap_bytes}")
            return
        self.walk_at = self.clock()

    def _reserve_loop(self) -> None:
        while not self.stop.wait(0 if self.reserve_at is None else RESERVE_INTERVAL):
            self.sample_reserve()

    def _walk_loop(self) -> None:
        while not self.stop.wait(0 if self.walk_at is None else WALK_INTERVAL - WALK_DEADLINE):
            self.sample_storage()

    def wait_healthy(self, seconds: float = 2.0) -> None:
        end = time.monotonic() + seconds
        while (self.reserve_at is None or self.walk_at is None) and time.monotonic() < end and self.first is None:
            time.sleep(POLL)
        self.check()

    def count_output(self, size: int) -> bool:
        with self.lock:
            self.output += size
            over = self.output > self.output_cap
        if over:
            self.fault("output", f"raw output exceeds {self.output_cap} bytes")
        if self.outer is not None:
            return self.outer.count_output(size) and not over
        return not over

    def remaining(self) -> float:
        """What is left of the ORIGINAL total budget; never extended."""
        return self.total_deadline - self.clock()

    # Processes ------------------------------------------------------------
    def _members(self, session: int, groups: set) -> list:
        left = self.remaining()
        if left <= 0:
            raise TimeoutError("no total budget left to observe processes")
        table = self.table(min(PS_TIMEOUT, left))
        return [pid for pid, group, member in table if member == session or group in groups]

    def close(self, session: int, groups: set, child: subprocess.Popen) -> bool:
        """TERM then KILL every group of the session, reaping the leader,
        then a fresh observation. False when closure is not positive."""
        try:
            for number, grace in ((signal.SIGTERM, TERM_GRACE), (signal.SIGKILL, KILL_GRACE)):
                if self.remaining() <= 0:
                    self.fault("deadline", f"no total budget left to close session {session}")
                    return False
                groups |= {group for _, group, member in self.table(min(PS_TIMEOUT, self.remaining())) if member == session}
                for group in groups:
                    try:
                        os.killpg(group, number)
                    except (ProcessLookupError, PermissionError):
                        pass
                # Grace windows never extend the original total deadline.
                end = time.monotonic() + min(grace, max(0.0, self.remaining()))
                while time.monotonic() < end:
                    child.poll()
                    if child.returncode is not None and not self._members(session, groups):
                        return True
                    time.sleep(0.1)
            child.poll()
            return child.returncode is not None and not self._members(session, groups)
        except Exception as error:
            self.fault("observation", f"process observation failed: {error!r}")
            return False

    def run(
        self, command: list, *, env: dict, cwd: str | None = None, timeout: float = 120, cleanup: bool = False
    ) -> subprocess.CompletedProcess:
        """The single supervised spawn path. `cleanup` admits only an
        ordinary teardown step: it may run after a fault, never clears it,
        is refused while any owned session is unclosed and stops at the
        total deadline."""
        if cleanup:
            if self.unclosed or self.clock() > self.total_deadline:
                raise Fault(f"cleanup refused: unclosed {sorted(self.unclosed)} or total deadline passed")
            limit = min(self.clock() + timeout, self.total_deadline)
        else:
            self.check()
            if self.outer is not None:
                self.outer.check()
            limit = min(self.clock() + timeout, self.work_deadline)
        child = subprocess.Popen(
            command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True
        )
        session = child.pid
        groups = {session}
        chunks = {"stdout": [], "stderr": []}
        readers = []
        closed = False
        timed_out = False
        try:
            for owner in (self, self.outer):
                if owner is not None:
                    owner.unclosed.add(session)
                    owner.children[session] = child

            def drain(stream, name):
                try:
                    while True:
                        block = os.read(stream.fileno(), 65536)
                        if not block:
                            return
                        if self.count_output(len(block)):
                            chunks[name].append(block)
                except Exception as error:
                    self.fault("output", f"{name} drain failed: {error!r}")

            for stream, name in ((child.stdout, "stdout"), (child.stderr, "stderr")):
                reader = threading.Thread(target=drain, args=(stream, name), daemon=True)
                reader.start()
                readers.append(reader)
            while child.poll() is None:
                if self.clock() > limit:
                    if cleanup:
                        timed_out = True
                        self.cleanup_failures.append(f"{command[0]} exceeded its cleanup time limit")
                        break
                    self.fault("deadline", f"{command[0]} exceeded its time limit")
                if not cleanup:
                    self.check(experimental=False)
                if self.outer is not None:
                    self.outer.check(experimental=False)
                time.sleep(POLL)
            if child.poll() is not None:
                end = time.monotonic() + min(DRAIN_GRACE, max(0.0, self.remaining()))
                for reader in readers:
                    reader.join(max(0.0, end - time.monotonic()))
                if any(reader.is_alive() for reader in readers):
                    self.fault("output", f"{command[0]} left a pipe open after exit")
                # Closure needs the reaped leader AND a fresh empty observation
                # of the whole session and its groups, leader included.
                try:
                    closed = child.returncode is not None and not self._members(session, groups)
                except Exception as error:
                    self.fault("observation", f"process observation failed: {error!r}")
            if not closed:
                closed = self.close(session, groups, child)
        finally:
            if not closed:
                closed = self.close(session, groups, child)
            end = time.monotonic() + min(DRAIN_GRACE, max(0.0, self.remaining()))
            for reader in readers:
                reader.join(max(0.0, end - time.monotonic()))
            pipes_open = any(reader.is_alive() for reader in readers)
            if closed and not pipes_open:
                for owner in (self, self.outer):
                    if owner is not None:
                        owner.unclosed.discard(session)
                for stream in (child.stdout, child.stderr):
                    stream.close()
            else:
                self.fault("observation", f"session {session} not positively closed; resources preserved")
        if not cleanup:
            self.check(experimental=False)
        elif self.unclosed:
            raise Fault(f"cleanup step left sessions {sorted(self.unclosed)} unclosed")
        if timed_out:
            raise Fault(f"cleanup step {command[0]} timed out; its session was closed, the step failed")
        if child.returncode is None:
            raise Fault(f"{command[0]} has no exit status; treated as failure")
        return subprocess.CompletedProcess(
            command, child.returncode, b"".join(chunks["stdout"]), b"".join(chunks["stderr"])
        )

    def shutdown(self) -> None:
        self.stop.set()
        for thread in self.threads:
            thread.join(2)

    def settle(self) -> list:
        """Positively close every session still recorded as unclosed (for
        example one an injected inner observer failed to close). Returns the
        sessions that remain unclosed."""
        for session in sorted(self.unclosed):
            child = self.children.get(session)
            if self.remaining() <= 0:
                # Settlement never mints time: what is left stays unclosed.
                break
            if child is not None and self.close(session, {session}, child):
                self.unclosed.discard(session)
                for stream in (child.stdout, child.stderr):
                    if stream is not None:
                        stream.close()
        return sorted(self.unclosed)

    def cleanup(self, label: str, action) -> bool:
        """Cleanup-only step (ordinary detach/removal). Never clears the
        fault; refused while any owned session is unclosed or after the
        total deadline. A failure is recorded and resources are preserved."""
        if self.unclosed:
            self.cleanup_failures.append(f"{label}: sessions {sorted(self.unclosed)} unclosed; preserved")
            return False
        if self.clock() > self.total_deadline:
            self.cleanup_failures.append(f"{label}: total deadline passed; preserved")
            return False
        try:
            done = bool(action())
        except Exception as error:
            done = False
            self.cleanup_failures.append(f"{label}: {error!r}; preserved")
            return False
        if not done:
            self.cleanup_failures.append(f"{label}: failed; preserved")
        return done
