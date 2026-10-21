# Proposed correction: shared proof supervisor (oxide#109)

Proposed, not implemented; it awaits independent design review. This is
revision 3, and the last revision under Tom's 2026-10-21 route-C ruling
([vision](../vision.md#authority-boundaries)): if this is again changes
required, Tom chooses the mechanism for elevated tasks. It answers Sol's
review of revision 2 (`ccf9d98f`), and corrects `scripts/bounded-test.py` at
`f5a998c`, keeping that baseline's guards:
- the reserve fault on staleness after 1 s;
- the scratch fault on staleness after 10 s, with its monitored budget;
- bounded output draining, with discard past the forward limit;
- a pipe still held open by a writer counts as evidence against closure.

All earlier findings, deviations and failed evidence stand as recorded.
Nothing here qualifies anything.

## Contract

Pinned-leader cooperative closure (Tom, "Yes, (2)", Queue decision
`99ed1d7f`). The only numbers ever signalled are children that the custodian
holds unreaped: the proof leader's group and the monitor. Survivors and
uncertainty end the run `UNCLOSED`, with scratch kept. The cooperative
observation boundary stands, so no claim is made about completeness for
arbitrary descendants.

## Actors

- **Custodian.** Single-threaded. It owns the absolute deadline, both pins and
  every signal. After setup it makes only bounded system calls:
  - `fork`, and `kevent` with a zero or remaining-budget timeout;
  - `waitpid` with `WNOHANG` only;
  - `killpg` or `kill` on a pinned child only;
  - `select` with a remaining-budget timeout;
  - non-blocking reads and writes on its control pipes.

  It never touches the filesystem or stdio after setup.
- **Monitor.** The custodian's first child. It owns all I/O: the evidence
  directory, receipt phases, output draining, reserve and scratch sampling,
  the census and cleanup. It sends timestamped messages to the custodian over
  a non-blocking control pipe.
- **Leader.** The custodian's second child. It becomes the proof command only
  after the launch gate opens.

## Launch gate

1. The custodian creates the control pipe and both output pipes (stdout and
   stderr), then forks the **monitor**. The monitor keeps the output read ends
   and the custodian closes its copies.
2. Setup runs in the **monitor**, inside the absolute deadline and against a
   bounded setup budget:
   - create the evidence directory;
   - publish phase `started: incomplete`;
   - take the first completed reserve and scratch readings;
   - send `ready`.

   The custodian waits with `select` until the setup deadline. If `ready`
   doesn't arrive by then, or arrives with low or failed readings, the
   custodian kills its pinned monitor, reaps it with `WNOHANG`, exits non-zero
   and launches nothing.
3. The custodian forks the **leader**. The child:
   - calls `setsid`;
   - `dup2`s the output write ends;
   - closes everything else;
   - blocks reading a gate pipe.

   The custodian registers kqueue `NOTE_EXIT` for it. `ESRCH` means the child
   is already a zombie, and any other error means there is no detector. Only
   then does it write one byte to the gate pipe, which can't block because the
   pipe is empty. The child then `exec`s the command, or `_exit(127)`s on
   failure. Custody, meaning the pin and the detector, therefore exists before
   the proof can run, and any blocking exec work happens in the child.
4. With no detector, the custodian never opens the gate. It sends SIGKILL to
   the pinned child, and the outcome is `UNCLOSED`.

## Control protocol

Every message carries `run_id` (128 random bits), the leader's PID, its
kernel start time (`kinfo_proc` `p_starttime`), a sequence number and a
monotonic stamp. `time.monotonic` is `mach_absolute_time`, which is comparable
across processes. A message with a wrong `run_id` or leader identity is a
fault.

| Message | Custodian action |
| --- | --- |
| `reading reserve=<bytes> at=<mono>` / `reading scratch=<bytes> complete=<bool> at=<mono>` | Track the newest **completed** reading. Latch a fault if no reserve reading is newer than 1 s, no scratch reading is newer than 10 s, reserve is below the threshold, or scratch is over budget. Freshness is enforced until the custodian exits, through closure and cleanup too. A heartbeat without a measurement doesn't count. |
| `output eof stdout` / `output eof stderr` | Record the pipe-EOF witness. Both are required for `closed`. |
| `census seq=<n> after=<stop-seq> members=<k> uncertain=<u> certain=<bool>` | Accepted as final only if it started after the custodian's `stopped` message. |
| `cleanup_started`, `cleanup_complete`, `monitor_complete` | Ordering checks; see the receipt phases below. |

Faults latch: detector, signal, reap, guard freshness, guard threshold,
protocol, monitor death, an uncertain census, a missing EOF witness. **No
later message can clear a latched fault.**

## Deadlines and state table

`absolute = start + limit`, `stop_at = absolute - stop_reserve` and
`exit_at = absolute - exit_margin`. The proposed values are a 20 s stop
reserve and a 2 s exit margin.

| Trigger | Custodian |
| --- | --- |
| Leader exit (`NOTE_EXIT`), latched fault, `stop_at`, or a signal to the custodian | Send `stopping`. TERM the pinned leader group. |
| + `term_grace` (bounded by `exit_at`) | KILL the pinned leader group. Send `stopped(seq)`. |
| Next | `waitpid(leader, WNOHANG)` in a `kevent`-timed loop until `exit_at`. Still unreaped means latch `reap` and keep the pin; no further signal is sent anyway. |
| Final census with `certain`, 0 members and 0 uncertain, plus both EOF witnesses | Closure established. Tell the monitor `cleanup`. Otherwise `UNCLOSED`: tell the monitor `keep`. |
| `monitor_complete`, or `exit_at` | Reap the monitor with `WNOHANG`. If it's still running at `exit_at`, KILL the pinned monitor and do one `WNOHANG` reap. Exit. |

**Custodian exit code:** 0 only if there are no latched faults, closure was
established, `monitor_complete` arrived before `exit_at`, and both children
were reaped. Otherwise it is non-zero (124 for the deadline, 125 for faults or
`UNCLOSED`).

## Receipt phases (monitor)

Each phase is published by atomic whole-file replacement in the evidence
directory, which counts against the owned-byte budget:

1. `started: incomplete`
2. `closure_established: cleanup_pending`, or `unclosed: scratch_kept`
3. `cleanup_partial_or_unknown`, published **before the first deletion**
4. `cleanup_complete`, published only after the deletion is confirmed
5. `monitor_complete: pending_custodian`

**There is no success phase in the file.** Success is defined as phase 5
**and** custodian exit 0, as recorded by the caller. A receipt can never claim
success by itself, so the custodian timing out after the monitor publishes
can't leave a false success behind.

## Output

The monitor drains both pipes on threads. It persists up to 8 MiB per stream
to the evidence directory, counted in the owned budget, forwards up to
32 MiB, and discards and counts the rest. The pipe-EOF witness is required for
closure. If something outside the session still holds the pipe open, EOF
never arrives, and the result is `UNCLOSED`.

## Census (monitor)

Each round:
1. take a `KERN_PROC_ALL` snapshot, giving PID, PPID, PGID, UID and
   `p_starttime`;
2. call `getsid` for each candidate;
3. re-read the candidate with `KERN_PROC_PID`.

The session binding counts only if the PID and `p_starttime` are unchanged
across the `getsid` call. Each process is then classified:
- **Member:** validated session equals SID. Once a member, it stays tracked
  by (PID, `p_starttime`) until a snapshot shows that identity gone, whatever
  its UID or parent.
- **Positively foreign:** a validated session that isn't SID, with no earlier
  membership. Also any process whose `p_starttime` precedes the leader's:
  it can't descend from the leader.
- **Uncertain:**
  - started at or after the leader and not validated (unreadable session,
    identity changed, or missing metadata);
  - any process at all in an incomplete, failed or timed-out snapshot.

  Uncertain blocks `closed`. It is never treated as foreign, and that holds
  for first-seen processes too.

Start-time comparisons use the kernel's `p_starttime` on both sides, never
receipt wall time. A false `UNCLOSED` caused by an unrelated process started
during the run with an unreadable session is accepted: the run fails closed.

## Fixture fallbacks

Every fixture process, including nested guardians and pressure consumers,
does three things first:
1. `signal.signal(SIGALRM, SIG_DFL)`;
2. `signal.pthread_sigmask(SIG_UNBLOCK, {SIGALRM})`;
3. `signal.setitimer(ITIMER_REAL, remaining)`, where `remaining` is
   `FIXTURE_DEADLINE_MONO - time.monotonic()`.

`FIXTURE_DEADLINE_MONO` is an absolute monotonic deadline passed in the
environment. A late grandchild therefore can't extend it, and gets a
non-positive remaining time that exits it immediately.

Required ordering: **case verdict deadline < fixture fallback deadline <
overall qualification deadline**. Proposed: case limit 60 s, fallback at case
start + 90 s, qualification 300 s. A fallback firing is observed after the
verdict and is never counted as closure.

## Failure table

| Failure | Outcome |
| --- | --- |
| Monitor setup slow or failed | Nothing launched. Monitor killed, the pin was held. Non-zero exit. |
| No `NOTE_EXIT` detector | Gate never opened. Pinned child KILLed. `UNCLOSED`. |
| Monitor blocked (census, filesystem or receipt write) | Readings go stale, which is a fault. Pinned stop. The monitor is KILLed at `exit_at` if it is still running. The receipt's last phase is not success. |
| Monitor dies | Fault. Pinned stop. No reported PID is signalled. |
| A survivor in another group, or uncertain | `UNCLOSED`, scratch kept, never signalled. |
| Escaped pipe holder | No EOF witness. `UNCLOSED`. |
| Output flood | Bounded persist, forward and discard, counted in the budget. |
| Cleanup interrupted | The phase is `cleanup_partial_or_unknown`. |
| Monitor completes, then the custodian times out | The custodian exits non-zero, so it isn't success. |
| A child is unreaped after KILL | `reap` fault. The pin is kept until exit. No further signals. |

## Proof (elevated, after independent review of this design)

**Signal evidence:** at each signal, record the target and show that it was
the pinned child at that moment.

**Cases:**
- launch-setup stall;
- stale monitoring caused by a monitor blocked in the filesystem;
- delayed reaping;
- output saturation;
- an escaped pipe holder;
- a census identity change across `getsid`;
- interrupted cleanup;
- monitor success followed by a custodian timeout;
- an inherited SIGALRM that is ignored or blocked;
- a group that cascades (`closed`);
- a separate-group survivor (`UNCLOSED`, never signalled).

If any of this can't be met within the approved scope, the brief stops and
names the missing mechanism.
