# Nested lease joining and permit borrowing (proposed, not implemented)

Status: proposed design for oxide#157. It is not implemented and not frozen,
and it changes no product behaviour. Current behaviour: a nested `oxide` takes
its own independent lease through the normal admission path
([workflow](workflow.md#what-one-command-does-local-executor),
[local lease](local-lease.md)). Nothing in `src/` issues or validates a nested
lease token or a borrowed permit.

This file owns the nested-join contract, written as constraints that any future
implementation must meet. The front door keeps command classes, admission,
waiting and closure; the [local lease](local-lease.md) keeps the pool, ledger,
slots and accounting; [architecture](../architecture.md#storage-invariants)
keeps the storage invariants. The direction is approved in
[vision](../vision.md#product-workflow-direction). The open questions are in
[Q-020](../questions.md#q-020--how-should-nested-oxide-lease-joining-and-permit-borrowing-work).
An implementation needs its own reviewed brief.

## Intended behaviour

- **Join.** A nested `oxide` whose slot key equals the outer command's key
  joins the outer lease. It takes no ledger lease, slot, host permit or
  eviction. It builds in a private, cold build directory inside the outer slot,
  registered as a writer ticket, so the outer cannot publish closure,
  accounting or a flush while the writer is live. A joined command never
  releases, deletes, recovers, evicts, flushes or rebinds anything.
- **Borrow.** A nested `oxide` with a different key takes its own slot through
  normal admission and borrows the outer host permit, so nesting cannot
  deadlock `max_active`. Borrowing changes one input, the host active-build
  count. It bypasses no reserve, budget, accounting, ownership, freshness,
  output or flush rule.
- **Refuse past a bound.** A token or command beyond a bound exits 1. It never
  silently becomes an independent lease.

## Constraints for any implementation

### Lease token

A token grants nothing until three independent factors hold:

1. **Possession** of an unlinked, owner-only descriptor inherited from the
   outer command's subtree. An environment string alone is not authority. It
   can be copied, outlive its generation or be forged.
2. **A live generation.** The outer record is active with the token's nonce,
   its record lock is still held, and the ledger shows the same lease, slot and
   incarnation. A free lock, a released or recovered generation, or a
   mismatched nonce fails. There is no wall-clock TTL: the token lasts exactly
   as long as its generation, because a clock cannot tell whether the outer is
   alive.
3. **Subtree relation.** A bounded walk of the caller's ancestors, comparing
   PIDs with start times, reaches the issuer, and the caller's session matches
   the issuer's session. A platform that cannot answer refuses the join.

Stale, copied, tampered, foreign-pool or out-of-subtree tokens take the
independent path. They never widen custody or grant deletion.

### No permit double-counting

- A borrowed command counts through its root permit only while that permit is
  held. A released or dead parent covers nothing. The borrower then counts
  itself at the next admission, so a released permit is never silently lent.
- A permit is never copied into a child and never released by a child. The
  count is derived from live recorded state under the admission lock, so a
  child that dies cannot leak a permit.
- Every borrow in one tree references the root permit, not an intermediate
  command, so an intermediate command cannot hold an allowance open.

### Nested deadlock rule

A command that positively detects it runs inside a live Oxide generation must
not wait on something only its ancestor can release. It refuses with exit 1 and
names the ancestor. The cases are a busy slot in the same checkout, a clean that
needs the ancestor's checkout lease, and the host limit when every counted
active command is an ancestor. Waits not implicated by an ancestor keep normal
FIFO behaviour. A joined compile in an idle exec session must also acquire that
session's permit through its gate (see Open).

### Bounds

These are product constants, not settings:

- 4 joined levels below one outer command;
- 8 live joined writers per generation;
- 8 live borrows per root permit;
- 64 ancestor hops in the walk.

### Other rules

- The outer closes a generation's gate before deciding closure from its
  writer tickets. A ticket that is not positively closed keeps the slot held
  and blocks recovery. The outer never waits on a live ticket.
- A joined command runs no output adoption, reservation or binding. It does not
  inherit or reset the outer's budget window.
- A nested command never takes the terminal foreground.
- A joined build directory is cold and is not reused across generations. A
  reused one would carry one checkout's absolute-path units into a later
  generation.
- Ownership, conservative recovery, idle-only eviction, recorded per-slot
  accounting, flush order and output custody all stand unchanged.

## Open

- Deadline inheritance for nested commands:
  [Q-018](../questions.md#q-018--how-do-per-command-admission-and-execution-budgets-behave-after-freeze).
- Composition with [idle exec sessions](idle-exec-sessions.md): a joined compile
  in an idle session must go through that session's permit gate and must not
  bypass it. Which design lands first is undecided.
- Whether a joined compile should ever be warm.
- Hostile same-UID code, containers, Windows and sandboxed macOS are outside the
  cooperative model. This design claims no containment.

## Not claimed

No implementation, native evidence, install or performance benefit. Every bound
above is a proposal for review, not a validated number.
