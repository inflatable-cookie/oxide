"""Supervise an Effigy-owned test process group; never leave a hung test alive.

`oxide` runs Cargo as leader of its own process group and forwards TERM to
it, so a KILL of this supervisor's group alone would miss Cargo's group.
While the command runs, the supervisor samples the process table and records
every group led by a descendant of its child, with the (PID, start time) of
each member it sees. A recorded group stays owned after its leader exits for
as long as a recorded member is alive in it. At the limit the supervisor
TERMs its own group, then KILLs the owned groups and its own, and reports any
member it could not see exit.
"""
import os
import signal
import subprocess
import sys
import time

LIMIT_SECONDS = 300
GRACE_SECONDS = 10
CLOSURE_SECONDS = 5
SAMPLE_SECONDS = 0.5


def processes():
    """{pid: (parent pid, process group, start time)} for every visible process."""
    table = {}
    listing = subprocess.run(
        ["ps", "-A", "-o", "pid=,ppid=,pgid=,lstart="], check=True, capture_output=True, text=True
    ).stdout
    for line in listing.splitlines():
        pid, parent, group, started = line.split(None, 3)
        table[int(pid)] = (int(parent), int(group), started.strip())
    return table


class Groups:
    """Process groups positively owned by the supervised command."""

    def __init__(self, root):
        self.root = root
        self.members = {}

    def live(self, table):
        """Recorded groups that still hold a recorded member."""
        return {
            group
            for group, members in self.members.items()
            if any(table.get(pid, (0, 0, ""))[1:] == (group, started) for pid, started in members)
        }

    def sample(self):
        table = processes()
        descendants, frontier = set(), [self.root]
        while frontier:
            parent = frontier.pop()
            for pid, (ppid, _, _) in table.items():
                if ppid == parent and pid not in descendants:
                    descendants.add(pid)
                    frontier.append(pid)
        for pid in descendants:
            group = table[pid][1]
            if group in descendants and group != self.root:
                self.members.setdefault(group, set())
        # A group stays owned through a recorded member, so members that
        # joined it after its leader exited are recorded too.
        live = self.live(table)
        for pid, (_, group, started) in table.items():
            if group in live or (group in self.members and pid in descendants):
                self.members[group].add((pid, started))
        return table


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


def supervise(
    command,
    limit=LIMIT_SECONDS,
    grace=GRACE_SECONDS,
    closure=CLOSURE_SECONDS,
    sample=SAMPLE_SECONDS,
):
    process = subprocess.Popen(command, start_new_session=True)
    groups = Groups(process.pid)
    deadline = time.monotonic() + limit
    while time.monotonic() < deadline:
        try:
            return process.wait(timeout=min(sample, max(deadline - time.monotonic(), 0)))
        except subprocess.TimeoutExpired:
            groups.sample()
    signal_group(process.pid, signal.SIGTERM)
    wait(process, grace)
    # Cargo's groups first, so a waiting `oxide` can see them close and release.
    killed = groups.live(groups.sample())
    for group in killed:
        signal_group(group, signal.SIGKILL)
    wait(process, closure)
    signal_group(process.pid, signal.SIGKILL)
    process.wait()
    owned = killed | {process.pid}
    closure_deadline = time.monotonic() + closure
    while True:
        alive = sorted(pid for pid, (_, group, _) in processes().items() if group in owned)
        if not alive or time.monotonic() > closure_deadline:
            break
        time.sleep(0.1)
    print(f"bounded repository test exceeded {limit} seconds", file=sys.stderr)
    if alive:
        print(f"processes still alive in owned groups: {alive}", file=sys.stderr)
    return 124


if __name__ == "__main__":
    sys.exit(supervise(sys.argv[1:]))
