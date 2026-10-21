# Proposed correction: shared proof supervisor (oxide#109)

Proposed, not implemented; it awaits independent design review. It corrects
`scripts/bounded-test.py` at `f5a998c` (branch
`ns-7413c9ec-bcca-4c84-9808-3079ac34764a`) after its independent review returned
changes required. That review's findings, the extra author submission, the
fixed `/tmp` logs and the failed native evidence all stand as recorded. This
document doesn't re-score them, and it qualifies nothing.

## Contract: pinned-leader cooperative closure

Tom's ruling, 2026-10-21, "Yes, (2)", Queue decision `99ed1d7f`
([vision](../vision.md#authority-boundaries)).

**Why.** macOS has no positive-identity signal for a process group that the
supervisor doesn't pin: no pidfd and no subreaper. A recheck followed by
`killpg` is still check-then-act, so the group can dissolve and its number can
be reissued in between. Rechecking narrows that window but doesn't close it,
and this design never presents it as closing it.

**Leader.** The supervised command starts in a new session. Until finalization
the supervisor holds it unreaped: exit is detected without reaping, through
kqueue `NOTE_EXIT`, or `waitid(WEXITED | WNOHANG | WNOWAIT)` if kqueue setup
fails. Its PID is therefore pinned, and so are its group ID and session ID. It
is the only number the supervisor ever signals.

**Membership.** Each observation classifies processes under the observing user
ID:

- **Member:** `getsid == SID`.
- **Uncertain:** the session can't be read for a process that started at or
  after the leader. An exiting process, or one whose `getsid` fails, is an
  example.

Uncertain processes are never treated as absent. They block `closed` and
appear in the receipt. Processes of other users, and processes that called
`setsid` (unless they still hold an output pipe), are outside the observable
scope. That scope boundary is stated in the receipt.

**Closure sequence.** It runs on every path: exit, timeout, fault, a signal to
the supervisor, or an exception after spawn.

1. TERM the pinned group (`killpg(SID)`). This is safe while the leader is
   unreaped, including after its exit.
2. A cooperative window of up to `term_grace`. Descendants close through their
   own parents' cascade: `oxide` forwards TERM to Cargo's group, which it holds
   unreaped, and nextest closes its test groups. The supervisor observes until
   no member or uncertain process remains.
3. KILL the pinned group.
4. A settle window of up to `kill_grace`.
5. Any member left in another group, and any uncertain process, is reported.
   The state is `UNCLOSED`. No numeric signal is ever sent to an unpinned
   group or PID.
6. Reap the leader and check both output pipes for EOF.

The state is `closed` only when the final observation has no members and no
uncertain processes, the leader is reaped, both pipes reached EOF and no
error was recorded. Otherwise it is `UNCLOSED`, the exit is non-zero, and owned
scratch is kept.

## Construction

Assign the `Popen` handle to the supervisor before any `Drain` thread or kqueue
setup. A later setup failure, `Drain` construction included, doesn't skip
closure. It runs the closure sequence with whichever exit detection is
available. While closure is unknown, ownership and scratch are kept.

## Bounded finalization

Timeout parameters can't interrupt blocking filesystem or stdio calls in
Python. The bound therefore comes from process structure:

- **Bootstrap.** A minimal outer process owns the absolute deadline. It starts
  the supervisor as its own child and pins it unreaped. Before the start it
  opens the incomplete-receipt path.
- **Supervisor.** The supervisor writes its receipt atomically, through a
  temporary file and rename, and only then removes scratch.
- **Deadline.** If the supervisor hasn't exited by the deadline, the bootstrap:
  1. writes a small, fixed incomplete receipt through its pre-opened
     descriptor;
  2. SIGKILLs its pinned child;
  3. exits non-zero.

  It never deletes scratch, and scratch whose removal was interrupted is
  reported as kept.
- **Output.** Forwarding uses non-blocking writes with a bounded buffer.
  Output past the buffer is discarded and counted, so a stalled consumer can't
  block finalization.

## Fixtures

Every real fixture child has its own finite monotonic self-exit. For example,
the endless writer loops only while `monotonic() < start + fixture_max`, where
`fixture_max` is longer than the supervisor's deadline plus its finalization
budget.

The proof asserts:

- the supervisor reported its verdict (`UNCLOSED` where expected) before that
  fallback fired;
- a fixture exiting by its own fallback is never counted as closure.

## Timestamps

- One monotonic clock for intervals.
- One wall-clock source for stamps.
- Integer milliseconds throughout the receipt.

## Proof (elevated, after independent review of this design)

A hook records every `killpg` and `kill` target. It proves each of:

- A child group that cascades on TERM ends `closed`.
- A separate group that ignores TERM and outlives the leader's KILL ends
  `UNCLOSED`, with scratch kept. No signal reaches that group. The fixture's
  own fallback ends it later.
- A kqueue or `Drain` setup failure after `Popen` still runs closure.
- An uncertain process blocks `closed`.
- A finalization that overruns produces the bootstrap's incomplete receipt
  inside the bound.
- A stalled output consumer can't block finalization.
- Receipt timestamps are consistent.
