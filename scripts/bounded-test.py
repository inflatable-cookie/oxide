"""Supervise an Effigy-owned test process group; never leave a hung test alive.

`oxide` runs Cargo as leader of its own process group and forwards TERM to
it, so a KILL of this supervisor's group alone would miss Cargo's group. At
the limit the supervisor identifies every group led by a descendant of its
child, TERMs its own group, then KILLs those groups and its own, and reports
any member it could not see exit.
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
    """{pid: (parent pid, process group)} for every visible process."""
    table = {}
    listing = subprocess.run(
        ["ps", "-A", "-o", "pid=,ppid=,pgid="], check=True, capture_output=True, text=True
    ).stdout
    for line in listing.splitlines():
        pid, parent, group = map(int, line.split())
        table[pid] = (parent, group)
    return table


def descendant_groups(root):
    """Process groups whose leader descends from `root`, root's own group excluded."""
    table = processes()
    descendants, frontier = set(), [root]
    while frontier:
        parent = frontier.pop()
        for pid, (ppid, _) in table.items():
            if ppid == parent and pid not in descendants:
                descendants.add(pid)
                frontier.append(pid)
    return {group for pid, (_, group) in table.items() if group in descendants and group != root}


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
    try:
        return process.wait(timeout=limit)
    except subprocess.TimeoutExpired:
        pass
    # Groups are exactly those led by this child's descendants, never a name match.
    groups = descendant_groups(process.pid)
    signal_group(process.pid, signal.SIGTERM)
    wait(process, grace)
    if process.poll() is None:
        groups |= descendant_groups(process.pid)
    # Cargo's groups first, so a waiting `oxide` can see them close and release.
    for group in groups:
        signal_group(group, signal.SIGKILL)
    wait(process, closure)
    signal_group(process.pid, signal.SIGKILL)
    process.wait()
    owned = groups | {process.pid}
    deadline = time.monotonic() + closure
    while True:
        alive = sorted(pid for pid, (_, group) in processes().items() if group in owned)
        if not alive or time.monotonic() > deadline:
            break
        time.sleep(0.1)
    print(f"bounded repository test exceeded {limit} seconds", file=sys.stderr)
    if alive:
        print(f"processes still alive in owned groups: {alive}", file=sys.stderr)
    return 124


if __name__ == "__main__":
    sys.exit(supervise(sys.argv[1:]))
