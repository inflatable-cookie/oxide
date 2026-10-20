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
    def blind():
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

    if outer.unclosed:
        raise AssertionError(f"outer guardian still holds {sorted(outer.unclosed)}")
    return cases
