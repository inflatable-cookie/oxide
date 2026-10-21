# Proposed correction: shared proof supervisor (oxide#109)

Proposed, not implemented; it awaits independent design review. This is
revision 2. It answers Sol's prospective review of `b9efc9aa` (changes
required: the stop path could block, custody of the proof leader, partial
construction, census rules and receipt phases). It corrects
`scripts/bounded-test.py` at `f5a998c`. The earlier review findings, the extra
author submission, the fixed `/tmp` logs and the failed native evidence all
stand as recorded. This document doesn't re-score them, and it qualifies
nothing.

## Contract: pinned-leader cooperative closure

Tom's ruling, 2026-10-21, "Yes, (2)", Queue decision `99ed1d7f`.

macOS has no positive-identity signal for a group the supervisor doesn't pin,
and a recheck doesn't close the reuse race. So the only number the supervisor
ever signals is the proof leader's pinned group, and survivors leave the run
`UNCLOSED`.

## Actors

There are two processes, with no general framework.

- **Custodian.** The process Effigy starts. It owns the absolute deadline and
  custody of the actual proof leader. It spawns the leader directly with
  `os.posix_spawn(..., setsid=True)`, which is available in the host's Python
  3.9.6. That call returns the PID, and there's no `Popen` constructor that can
  hang after the child exists. The leader stays the custodian's unreaped
  child until the custodian's last signal, so its PID, group ID and session ID
  stay pinned however the monitor fares. The custodian's stop path makes only
  non-blocking system calls: `killpg` on the pinned SID, `kill` on the monitor,
  `kevent`, and `waitpid` after the final signal. It never writes files or
  stdio on the stop path.
- **Monitor.** The custodian's second child. It does all the I/O: reserve and
  scratch sampling, the census, output forwarding, receipt phases and cleanup.
  It reports to the custodian over a pipe. If the monitor dies, the custodian
  still holds the pin. It never signals any PID the monitor reported. It stops
  through the pinned group alone and ends `UNCLOSED`.

## Start order

1. The custodian creates the evidence directory, which sits outside scratch,
   and writes the receipt as phase `started: incomplete` with `fsync`. That
   happens before any writer exists. Every later failure leaves at least that
   artifact.
2. It starts the monitor and waits for its first reserve and scratch readings,
   within a bounded wait. A missing or low first reading refuses before spawn.
3. It spawns the leader. stdout and stderr go to files in the evidence
   directory, not pipes, so the custodian never drains output. The monitor
   tails those files with bounded, non-blocking reads.
4. It registers kqueue `NOTE_EXIT` for the leader.
   - `ESRCH` means the leader is already a zombie.
   - Any other registration failure means **no detector**. The custodian
     immediately `killpg`s the pinned group with SIGTERM, then SIGKILL, while
     the leader is still unreaped. The outcome is `UNCLOSED`, and scratch is
     kept. It is never reported as "nothing launched".

## Deadlines

`absolute = start + limit`, and `work_end = absolute - stop_reserve`. The stop
reserve is proposed at 15 s: TERM grace, KILL grace and the final non-blocking
steps.

| Moment | Custodian (non-blocking only) | Monitor |
| --- | --- | --- |
| Leader exits, a fault latches, `work_end` passes, or the custodian is signalled | Starts closure: TERM the pinned group | Census continues |
| + `term_grace` | KILL the pinned group | Census continues |
| + `kill_grace` | `waitpid` the leader; tell the monitor "stop" | Final census, receipt phase, cleanup |
| `absolute` | If the monitor hasn't confirmed, KILL the monitor (its own child, also pinned), reap it, exit non-zero | Killed |

At `absolute` the custodian writes nothing. The last fsynced receipt phase
is the truth, and it is never final success unless the monitor reached that
phase.

## Census (monitor)

- **Enumeration:** one complete process-table snapshot per round through
  `sysctl` `KERN_PROC_ALL`, giving PID, PPID, PGID, UID and start time, plus
  `getsid` for each PID. A failed, truncated or timed-out snapshot, or missing
  metadata for any process, makes the round uncertain. An uncertain round
  blocks `closed`; nothing is discarded.
- **Identity:** `(PID, start time)`. A process seen once in the session stays a
  tracked member, whatever its UID or later parent, until a later snapshot
  positively shows that identity gone. Reparenting doesn't erase membership.
- **The leader** is excluded only once it is positively exited, unreaped, by
  `NOTE_EXIT` or `ESRCH`.
- **Uncertain:** a process that started at or after the leader whose session
  can't be read.
- **Out of scope:** processes never seen in the session. That includes
  `setsid` escapes and unrelated processes of any UID. The receipt says so.
- **`closed`:** requires the final round to be certain, with no tracked members
  and no uncertain processes.

## Receipt phases

Each phase is a whole-file atomic replacement (temporary file, `fsync`,
`rename`) in the evidence directory, outside scratch:

1. `started: incomplete`
2. `closure_established: cleanup_pending`, or `unclosed: scratch_kept`
3. `cleanup_complete`, or `cleanup_partial_or_unknown`

   An interrupted removal is reported as partial or unknown, not as kept.
4. `final_success`

   Only after `cleanup_complete`, with the command's status and evidence
   recorded.

A timeout, the monitor's death or a custodian exit leaves the last phase in
place, and a phase earlier than `final_success` is never success.

## Fixtures

Every real fixture child, including nested guardians and pressure consumers,
calls `signal.alarm(fixture_max)` first and leaves SIGALRM's default action in
place. The kernel then ends it after `fixture_max` even while it is blocked in
`write`. A fixture that can't do that may use only bounded, non-blocking
operations.

`fixture_max` is greater than `absolute` plus a margin, so the expected
`UNCLOSED` verdict is recorded before any fallback fires. The proof checks the
eventual exit inside the overall qualification bound. Fallback expiry never
counts as closure.

## Time

One origin is captured at start: `(monotonic, wall)`. Every stamp is
`wall_origin + (monotonic - mono_origin)`, in integer milliseconds. Intervals
use monotonic time only.

## Proof (elevated, after independent review of this design)

A hook records every `kill` and `killpg` target, which must only ever be the
pinned SID or the monitor. The proof covers:

- a group that cascades on TERM: `closed` and `final_success`;
- a separate group that outlives the leader's KILL: `UNCLOSED`, scratch kept,
  never signalled;
- a kqueue registration failure: immediate pinned stop and `UNCLOSED`;
- the monitor killed mid-run: the custodian stops the pinned group, the last
  phase stays non-success, and no reported PID is signalled;
- a blocked monitor write at the deadline: the custodian exits inside the
  bound;
- a truncated or failed census round: uncertain;
- a reparented member still tracked;
- an interrupted cleanup reported as `cleanup_partial_or_unknown`;
- a fixture blocked in `write` that `alarm` still ends after the verdict;
- consistent timestamps.

If any of this can't be met inside the approved scope, the brief stops and
names the missing mechanism.
