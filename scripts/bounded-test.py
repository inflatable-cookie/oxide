"""Supervise an Effigy-owned test process group; never leave a hung test alive.

The command starts in a new session whose ID is the child's PID. `oxide`
runs Cargo as leader of its own process group with `setpgid`, which keeps
the session, so Cargo, its tests and any child orphaned by Cargo's exit stay
identifiable by session ID until one calls `setsid`. At the limit, while the
child is still unreaped and its PID cannot be reused, the supervisor records
every process group in that session, TERMs its own group (`oxide` forwards
TERM to Cargo's group), then KILLs the recorded groups and its own, and
reports any session or group member it could not see exit.
"""
import os
import signal
import subprocess
import sys
import time

LIMIT_SECONDS = 300
GRACE_SECONDS = 10
CLOSURE_SECONDS = 5


def processes():
    """[(pid, process group, session or None)] for every visible process."""
    table = []
    listing = subprocess.run(
        ["ps", "-A", "-o", "pid=,pgid="], check=True, capture_output=True, text=True
    ).stdout
    for line in listing.splitlines():
        pid, group = map(int, line.split())
        try:
            session = os.getsid(pid)
        except (ProcessLookupError, PermissionError):
            session = None
        table.append((pid, group, session))
    return table


def session_groups(session):
    return {group for _, group, member in processes() if member == session}


def signal_group(group, number):
    try:
        os.killpg(group, number)
    except ProcessLookupError:
        pass


def wait(process, seconds):
    try:
        process.wait(timeout=seconds)
    except subprocess.TimeoutExpired:
        pass


def supervise(command, limit=LIMIT_SECONDS, grace=GRACE_SECONDS, closure=CLOSURE_SECONDS):
    process = subprocess.Popen(command, start_new_session=True)
    session = process.pid
    try:
        return process.wait(timeout=limit)
    except subprocess.TimeoutExpired:
        pass
    # The child is unreaped, so its PID still names this session alone.
    groups = session_groups(session)
    signal_group(session, signal.SIGTERM)
    wait(process, grace)
    if process.returncode is None:
        groups |= session_groups(session)
    # Cargo's groups first, so a waiting `oxide` can see them close and release.
    for group in groups - {session}:
        signal_group(group, signal.SIGKILL)
    wait(process, closure)
    signal_group(session, signal.SIGKILL)
    process.wait()
    groups.add(session)
    deadline = time.monotonic() + closure
    while True:
        alive = sorted(
            pid for pid, group, member in processes() if member == session or group in groups
        )
        if not alive or time.monotonic() > deadline:
            break
        time.sleep(0.1)
    print(f"bounded repository test exceeded {limit} seconds", file=sys.stderr)
    if alive:
        print(f"processes still alive in the test session or its groups: {alive}", file=sys.stderr)
    return 124


if __name__ == "__main__":
    sys.exit(supervise(sys.argv[1:]))
