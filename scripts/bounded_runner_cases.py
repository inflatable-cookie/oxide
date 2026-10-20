"""Deterministic fault cases for scripts/bounded_runner.py (oxide#097).

Run only inside the focused rust:oxide-removable-pool session, before
staging, under the real outer guardian. Each case builds its own injected
inner guardian; its expected faults never touch the outer fault. Real
children are tiny, register with the outer guardian too, and must be
positively settled by it. Attachment and detach cases use stand-ins.
"""

from __future__ import annotations

import os
import signal
import threading
import time

import bounded_runner as br


class FakeClock:
    def __init__(self, now: float = 1000.0):
        self.now = now

    def __call__(self) -> float:
        return self.now


def expect_fault(guardian: br.Guardian, kind: str) -> str:
    try:
        guardian.check(experimental=False)
    except br.Fault as fault:
        if not str(fault).startswith(kind + ":"):
            raise AssertionError(f"expected a {kind} fault, got {fault}")
        return str(fault)
    raise AssertionError(f"expected a {kind} fault, got none")


def inner(outer, scratch, *, clock=None, free=None, walker=None, table=None, **bounds):
    clock = clock or FakeClock()
    guardian = br.Guardian(
        clock(),
        scratch,
        walker or br.Walker([scratch], clock=clock),
        clock=clock,
        free=free or (lambda _path: 64 * 1024 * br.MIB),
        table=table or br.ps_table,
        outer=outer,
        threads=False,
        **bounds,
    )
    guardian.reserve_at = clock()
    guardian.walk_at = clock()
    return guardian


def real_inner(outer, scratch, **bounds):
    """Real clock, injected healthy monitors, for real-child cases."""
    guardian = inner(outer, scratch, clock=time.monotonic, **bounds)

    def keep_fresh():
        while not guardian.stop.wait(0.1):
            guardian.reserve_at = time.monotonic()
            guardian.walk_at = time.monotonic()

    threading.Thread(target=keep_fresh, daemon=True).start()
    return guardian


def no_members(outer: br.Guardian, session_count_before: int) -> None:
    if outer.settle():
        raise AssertionError(f"outer guardian could not settle {sorted(outer.unclosed)}")


def run_cases(outer: br.Guardian, scratch: str, env: dict, log) -> int:
    cases = 0

    def case(name):
        nonlocal cases
        cases += 1
        log(f"synthetic case {cases}: {name}")

    # Reserve -------------------------------------------------------------
    case("reserve breach")
    guardian = inner(outer, scratch, free=lambda _p: 1)
    guardian.sample_reserve()
    expect_fault(guardian, "reserve")
    guardian.first = None  # a fresh check below must find the sticky record
    guardian.fault("reserve", "first")
    guardian.fault("storage", "second")
    if not expect_fault(guardian, "reserve").endswith("first"):
        raise AssertionError("first fault was not sticky")

    case("reserve sample error")
    def broken(_p):
        raise OSError("statvfs failed")
    guardian = inner(outer, scratch, free=broken)
    guardian.sample_reserve()
    expect_fault(guardian, "reserve")

    case("dead reserve sampler")
    clock = FakeClock()
    guardian = inner(outer, scratch, clock=clock)
    dead = threading.Thread(target=lambda: None)
    dead.start()
    dead.join()
    guardian.threads = [dead]
    expect_fault(guardian, "observation")

    case("stale reserve sample")
    clock = FakeClock()
    guardian = inner(outer, scratch, clock=clock)
    clock.now += 1.5
    guardian.walk_at = clock.now
    expect_fault(guardian, "reserve")

    # Storage -------------------------------------------------------------
    root = os.path.join(scratch, "walk")
    os.makedirs(os.path.join(root, "a"))
    with open(os.path.join(root, "a", "f"), "wb") as handle:
        handle.write(b"x" * 8192)

    case("storage excess")
    guardian = inner(outer, scratch, walker=br.Walker([root]), cap_bytes=4096)
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    case("storage walk error")
    def denied(_path):
        raise PermissionError("denied")
    guardian = inner(outer, scratch, walker=br.Walker([root], lstat=denied))
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    case("stale storage sample")
    clock = FakeClock()
    guardian = inner(outer, scratch, clock=clock)
    clock.now += 4.5
    guardian.reserve_at = clock.now
    expect_fault(guardian, "storage")

    case("ENOENT restarts the whole walk and discards the partial total")
    full = br.Walker([root]).sample()
    calls = {"n": 0}
    def flaky(path):
        calls["n"] += 1
        if calls["n"] == 3:
            raise FileNotFoundError(path)
        return os.lstat(path)
    walker = br.Walker([root], lstat=flaky)
    if walker.sample() != full:
        raise AssertionError("a retried walk did not equal one complete walk")
    def vanishing(path):
        if path.endswith("/f"):
            raise FileNotFoundError(path)
        return os.lstat(path)
    guardian = inner(outer, scratch, walker=br.Walker([root], lstat=vanishing))
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    case("walk deadline")
    clock = FakeClock()
    def slow(path):
        clock.now += 0.6
        return os.lstat(path)
    guardian = inner(outer, scratch, clock=clock, walker=br.Walker([root], clock=clock, lstat=slow))
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    case("unresolved attachment is counted at its bound, never zero")
    walker = br.Walker([root])
    walker.unresolved_attach = ("/private/tmp/x.sparseimage", 128 * br.MIB)
    if walker.sample() < full + 128 * br.MIB:
        raise AssertionError("an unresolved attachment was not counted at its bound")
    guardian = inner(outer, scratch, walker=walker, cap_bytes=64 * br.MIB)
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    # Output --------------------------------------------------------------
    case("simultaneous pipe pressure drains both streams")
    guardian = real_inner(outer, scratch, output_bytes=16 * br.MIB)
    result = guardian.run(
        ["/bin/sh", "-c", "head -c 2000000 /dev/zero & head -c 2000000 /dev/zero >&2; wait"], env=env, timeout=30
    )
    guardian.stop.set()
    if len(result.stdout) != 2000000 or len(result.stderr) != 2000000:
        raise AssertionError("pipe pressure lost output")

    case("output excess")
    guardian = real_inner(outer, scratch, output_bytes=1 * br.MIB)
    try:
        guardian.run(["/bin/sh", "-c", "head -c 4000000 /dev/zero; sleep 30"], env=env, timeout=30)
        raise AssertionError("output excess did not fault")
    except br.Fault as fault:
        if not str(fault).startswith("output:"):
            raise
    guardian.stop.set()
    no_members(outer, 0)

    case("open pipe after leader exit")
    guardian = real_inner(outer, scratch)
    try:
        guardian.run(["/bin/sh", "-c", "sleep 30 &"], env=env, timeout=30)
        raise AssertionError("an open pipe did not fault")
    except br.Fault as fault:
        if not str(fault).startswith("output:"):
            raise
    guardian.stop.set()
    no_members(outer, 0)

    # Post-launch faults and closure --------------------------------------
    case("fault during a running child closes its session")
    guardian = real_inner(outer, scratch)
    threading.Timer(0.3, lambda: guardian.fault("reserve", "injected")).start()
    try:
        guardian.run(["/bin/sleep", "30"], env=env, timeout=30)
        raise AssertionError("injected fault did not stop the child")
    except br.Fault:
        pass
    guardian.stop.set()
    if outer.unclosed:
        raise AssertionError("closed session still recorded")

    case("cancellation during a running child")
    guardian = real_inner(outer, scratch)
    threading.Timer(0.3, lambda: os.kill(os.getpid(), signal.SIGINT)).start()
    try:
        guardian.run(["/bin/sleep", "30"], env=env, timeout=30)
        raise AssertionError("cancellation did not stop the child")
    except br.Cancelled:
        pass
    guardian.stop.set()
    if outer.unclosed:
        raise AssertionError("cancelled session still recorded")

    case("TERM-ignoring group needs KILL")
    guardian = real_inner(outer, scratch)
    started = time.monotonic()
    try:
        guardian.run(["/bin/sh", "-c", "trap '' TERM; sleep 30"], env=env, timeout=0.5)
        raise AssertionError("time limit did not fault")
    except br.Fault as fault:
        if not str(fault).startswith("deadline:"):
            raise
    guardian.stop.set()
    if time.monotonic() - started < br.TERM_GRACE or outer.unclosed:
        raise AssertionError("TERM-ignoring group was not escalated to KILL and closed")

    case("failed process observation keeps the session unclosed for the outer guardian")
    def blind(_timeout=None):
        raise OSError("ps failed")
    guardian = real_inner(outer, scratch, table=blind)
    threading.Timer(0.3, lambda: guardian.fault("reserve", "injected")).start()
    try:
        guardian.run(["/bin/sleep", "30"], env=env, timeout=30)
        raise AssertionError("injected fault did not stop the child")
    except br.Fault:
        pass
    guardian.stop.set()
    if not outer.unclosed:
        raise AssertionError("an unobserved session was treated as closed")
    no_members(outer, 0)

    # Cleanup -------------------------------------------------------------
    case("ordinary detach failure preserves resources")
    guardian = inner(outer, scratch)
    if guardian.cleanup("detach stand-in", lambda: False) or not guardian.cleanup_failures:
        raise AssertionError("a failed detach was not preserved")

    case("cleanup refused while a session is unclosed")
    guardian = inner(outer, scratch)
    guardian.unclosed.add(999999)
    if guardian.cleanup("detach stand-in", lambda: True):
        raise AssertionError("cleanup ran with an unclosed session")

    case("cleanup-deadline expiry")
    clock = FakeClock()
    guardian = inner(outer, scratch, clock=clock)
    clock.now += 301
    if guardian.cleanup("detach stand-in", lambda: True):
        raise AssertionError("cleanup ran after the total deadline")

    # Review 6295233361 regressions ----------------------------------------
    case("late final lstat is not published")
    single = os.path.join(scratch, "single")
    os.makedirs(single)
    clock = FakeClock()
    def late_final(path):
        result = os.lstat(path)
        clock.now += 1.5
        return result
    guardian = inner(outer, scratch, clock=clock, walker=br.Walker([single], clock=clock, lstat=late_final))
    before = guardian.walk_at
    guardian.sample_storage()
    if guardian.walk_at != before:
        raise AssertionError("a late walk refreshed the sample time")
    expect_fault(guardian, "storage")

    case("late scandir is not published")
    clock = FakeClock()
    def late_listing(path):
        clock.now += 1.5
        return os.scandir(path)
    guardian = inner(outer, scratch, clock=clock, walker=br.Walker([single], clock=clock, scandir=late_listing))
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    for label, slow in (("stat", "stat"), ("ismount", "ismount"), ("statvfs", "statvfs")):
        case(f"late mount {label} is not published")
        clock = FakeClock()
        def advance(function):
            def wrapped(path):
                result = function(path)
                clock.now += 1.5
                return result
            return wrapped
        observers = {"stat": os.stat, "ismount": lambda _p: True, "statvfs": os.statvfs}
        observers[slow] = advance(observers[slow])
        walker = br.Walker([], clock=clock, **observers)
        walker.mounts[scratch] = os.stat(scratch).st_dev
        guardian = inner(outer, scratch, clock=clock, walker=walker)
        guardian.sample_storage()
        expect_fault(guardian, "storage")

    case("second ENOENT attempt that runs past the original deadline")
    clock = FakeClock()
    calls = {"n": 0}
    def first_vanishes(path):
        calls["n"] += 1
        clock.now += 0.6
        if calls["n"] == 1:
            raise FileNotFoundError(path)
        return os.lstat(path)
    guardian = inner(outer, scratch, clock=clock, walker=br.Walker([single], clock=clock, lstat=first_vanishes))
    guardian.sample_storage()
    expect_fault(guardian, "storage")

    case("cleanup leader that closes its streams but stays alive fails and is closed")
    guardian = real_inner(outer, scratch)
    removed = {"done": False}
    try:
        guardian.run(["/bin/sh", "-c", "exec >&- 2>&-; sleep 30"], env=env, timeout=0.5, cleanup=True)
        removed["done"] = True  # a false success would authorize removal
    except br.Fault:
        pass
    guardian.stop.set()
    if removed["done"] or not guardian.cleanup_failures:
        raise AssertionError("a live cleanup leader was reported as a successful step")
    if outer.unclosed:
        raise AssertionError("the timed-out cleanup leader was not positively closed")

    case("deadline expiry during close does not mint grace time")
    guardian = real_inner(outer, scratch, total_seconds=1.5)
    started = time.monotonic()
    try:
        guardian.run(["/bin/sh", "-c", "trap '' TERM; sleep 30"], env=env, timeout=0.3)
        raise AssertionError("time limit did not fault")
    except br.Fault:
        pass
    guardian.stop.set()
    # Closing within what is left is fine; minting new grace windows is not.
    if time.monotonic() - started > 3.0:
        raise AssertionError("closure ran past the original total deadline")
    no_members(outer, 0)

    case("deadline expiry during drain")
    guardian = real_inner(outer, scratch, total_seconds=1.0)
    started = time.monotonic()
    try:
        guardian.run(["/bin/sh", "-c", "sleep 30 &"], env=env, timeout=30)
        raise AssertionError("an open pipe near the deadline did not fault")
    except br.Fault:
        pass
    guardian.stop.set()
    if time.monotonic() - started > 2.5:
        raise AssertionError("drain ran past the original total deadline")
    no_members(outer, 0)

    case("settle after the deadline leaves sessions unclosed")
    guardian = real_inner(outer, scratch, table=blind)
    threading.Timer(0.3, lambda: guardian.fault("reserve", "injected")).start()
    try:
        guardian.run(["/bin/sleep", "30"], env=env, timeout=30)
    except br.Fault:
        pass
    guardian.stop.set()
    guardian.total_deadline = time.monotonic() - 1
    started = time.monotonic()
    if not guardian.settle() or time.monotonic() - started > 0.5:
        raise AssertionError("settle after the deadline claimed closure or took time")
    no_members(outer, 0)

    case("deadline expiry during a cleanup step")
    guardian = real_inner(outer, scratch, total_seconds=1.0)
    try:
        guardian.run(["/bin/sleep", "30"], env=env, timeout=30, cleanup=True)
        raise AssertionError("a cleanup step past the deadline succeeded")
    except br.Fault:
        pass
    guardian.stop.set()
    if not guardian.cleanup_failures:
        raise AssertionError("cleanup expiry was not recorded")
    no_members(outer, 0)

    case("frozen accounting during guarded removal")
    walker = br.Walker([single])
    last = walker.sample()
    walker.frozen = last
    os.rmdir(single)
    if walker.sample() != last:
        raise AssertionError("removal accounting was not the last complete total")

    if outer.unclosed:
        raise AssertionError(f"outer guardian still holds {sorted(outer.unclosed)}")
    return cases
