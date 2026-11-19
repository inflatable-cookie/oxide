# Idle exec sessions and run-slot custody (proposed, not implemented)

Status: proposed design for oxide#158. It is not implemented or frozen, and it
grants no implementation, native-run, install, host-setting or consumer
authority. Current behaviour: an `oxide exec` session holds its slot and its
host active-build permit for its whole life, and a long-lived `oxide run` keeps
its owned temp directory and slot until the program's group closes
([workflow](workflow.md#oxide-exec-cargo-wrapping-tools),
[child temporary files](workflow.md),
[local lease](local-lease.md)). Nothing in `src/` gates compiles or separates
the permit from slot custody.

This file owns the proposal for that split. Storage invariants stay in
[architecture](../architecture.md#storage-invariants). The direction is approved
but not built ([vision](../vision.md#product-workflow-direction)).

## Problem

Two resources are coupled because one control record answers both questions:

- **Slot custody**: the exclusive right to write a slot's build directory and
  owned scratch. An unknown writer must keep it.
- **Host active-build permit**: one unit of the host-wide `max_active` limit. It
  bounds concurrent build load, not bytes.

A dev server such as `tauri dev` therefore uses a host permit while it is not
compiling. Releasing the permit alone does not stop a warm slot from being
pinned. Releasing slot custody does not help the host limit, and it can corrupt
a running program. The two must be handled separately.

## Rulings this design keeps

- Exec keeps its slot and owned temp for the whole session. Only the permit is
  separated.
- Exclusive use, conservative recovery and finite capacity stand. Unknown
  custody stays held.
- No PATH shims, no machine-wide wrapper and no new daemon. The broker is part
  of the running command.
- Unknown state never becomes idle or closed. A failed gate never starts a
  compiler.
- The three existing watchdog paths are unchanged. The split adds no host-space
  stop and removes none.
- No config key, flag, host setting, scheduler or consumer change.
- `run` slot custody is unchanged. Warm-slot pinning by a long-lived program
  needs a separate successor design.

## Proposed mechanism

Only a seam that acts before work is acceptable. Process scans, quiet-tree
heuristics, CPU samples and mtime inference all observe after a compiler has
started, so they are rejected.

The proposed seam is a scoped `RUSTC_WRAPPER` gate in the session's child
environment. Cargo runs it before each rustc, so a gated compile must obtain the
session's permit before the compiler starts. Its properties:

- **Before work.** The gate requests a permit and starts the compiler only after
  a grant. A refused, closed or timed-out gate starts nothing.
- **One permit per session.** The session broker counts live gated writers and
  grants against one permit.
- **Transparent.** The compiler gets the same argv, descriptors, jobserver,
  stdio and exit status.
- **Stable path.** One per-user gate copy, not one per session, so no session
  nonce enters a path Cargo or a build script may record.
- **Fail closed.** Any gate apparatus error ends with no compiler started.
- **Socket in owned space.** The socket sits inside the slot and is unlinked at
  release. The gate token is an accidental-request guard, not a security
  boundary.

The control record would gain one fact, `permit`, that is either `held` or
`released`. Only a custody-holding session may release it. Admission counts held
permits only. Slot rules (second slot, per-project cap, clean barrier, recovery,
eviction) keep reading slot activity, so an idle session still occupies its slot
and still blocks `oxide clean`.

- **Release** happens after the last gated writer closes and no request is
  pending. The write is durable. A failed write keeps the permit held.
- **Reacquire** happens on the next gated compile. The grant follows a durable
  write, and the session prints the ordinary wait line if the permit is busy.

## Coverage limits

Gated: every rustc that Cargo runs through the wrapper, including build scripts,
proc macros and test targets, and child Cargo processes that inherit the
variable.

Not gated (documented gaps, not claims):

- tools that compile through an absolute path, their own toolchain, a container
  or a rewritten environment;
- `RUSTC_WORKSPACE_WRAPPER` and Clippy's driver, which are unverified here;
- Cargo's non-compiler children: build script and test execution, and `run`
  programs;
- `RUSTDOC`-driven doctest compilation;
- processes that daemonise or call `setsid` and then rewrite their environment.

The gate guarantees that gated compiles acquire the permit before work. It does
not guarantee that every compiler inside an arbitrary tool is gated. An escaped
compile is uncounted load, not a storage-safety violation, but the operator must
accept that boundary.

## Interactions

- **Nested `oxide`.** A nested command that joins the outer lease uses the outer
  permit through the gate. A nested command that does not join takes its own
  permit and must strip the outer gate variables, so its compile is counted once.
  Otherwise a limit of 1 deadlocks. See [nested lease](nested-lease.md).
- **Stale tokens.** A stale or foreign gate token grants nothing and widens no
  custody.
- **Removable volumes.** A permit transition never flushes. If the volume
  vanishes, the existing removal watchdog stops the group. The gate cannot reach
  a socket on a vanished volume, so a compile fails closed.
- **Budgets.** The session's execution window covers permit waits and never
  resets.

## Open operator dispositions

None of these is recorded as settled in [questions](../questions.md). Until
they are, this file is only a proposal.

- **D-1 Permit meaning.** For a custody-holding exec or run session, the host
  permit is held only while a gated compiler is live. Rejecting it keeps the
  permit for the whole session and stops this design.
- **D-2 Scoped gate.** A scoped compiler gate in a session's child environment is
  acceptable under the deferred-interception ruling. Rejecting it stops this
  design.
- **D-3 Boundary.** Ungated compiles and Cargo's non-compiler children are not
  counted while a session is idle. Rejecting it stops this design.
- **D-4 Run custody.** No slot or temp custody change for `run`.
- **D-5 Schema.** The control-state version bump and installed-binary hand-off
  follow the existing pattern and need a receipt-matching install gate.

Also open: whether the permit must count Cargo's non-compiler children, or
whether gated compilation is an acceptable unit for long-lived sessions.

## Implementation order, if accepted

1. Permit field and admission counting, with no behaviour change.
2. Gate apparatus with the permit still held, to prove the wrapper is
   transparent: no unit rebuilds, no change in recorded tool identity.
3. Idle release and reacquire for `exec`, bounded `run` and `run` with an owned
   temp.

Run custody is not part of this design. Each slice needs its own approved brief
and the Oxide Supervisor's design freeze.

## Not claimed

No implementation, no measured benefit and no completeness of descendant gating.
Warm-rebuild figures in the source proposal are producer observations, not
measurements. Nothing here shows that ungated compiles are impossible.
