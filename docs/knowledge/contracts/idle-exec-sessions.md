# Idle exec sessions and run-slot custody (proposed design)

Status: proposal, design only (oxide#158). It grants no implementation,
native run, install, host-setting or consumer authority. Current behaviour
stays owned by [workflow](workflow.md#oxide-exec-cargo-wrapping-tools),
[child temporary files](workflow.md#child-temporary-files-oxide136) and
[local lease](local-lease.md); nothing here is current truth until a
separately briefed implementation changes it. Implementation waits for the
Oxide Supervisor's design freeze (Tom, decision `95b3bc63`) after independent
review of this exact head, and each slice needs its own approved brief.

This design answers the two approved-but-not-built directions in
[vision](../vision.md#product-workflow-direction): *idle exec sessions release
their host permit* and the long-lived `oxide run` server custody question.
It reads [shape and storage invariants](../architecture.md#storage-invariants),
the [validation policy](../vision.md#validation-policy) and the
[shared proof supervisor](workflow.md#shared-proof-supervisor).

## 1. What this design settles, and what it refuses to settle

It specifies:

1. the only enforceable *before-work* interception point this repository can
   support, its exact coverage and its fail-closed behaviour (§5–§7);
2. a split between a session's **slot custody** and its **host active-build
   permit**, with the permit released while no gated compiler runs and
   reacquired before the next compile (§8–§10);
3. the custody, crash, unknown, fairness and nested-lease rules that must hold
   while a permit is idle (§11–§13);
4. why long-lived `oxide run` servers keep today's `#136` guarantee, and what
   a successor would have to prove before either custody alternative in the
   brief could be adopted (§14);
5. the operator dispositions, slices, dependencies and a bounded test plan
   (§15–§17).

It does **not** decide any of those operator questions, delete or recycle
storage, change host settings, scheduler or capacity policy, install anything,
touch a consumer, or measure a live project. It selects no implementation.

## 2. Problem statement

Two different resources are coupled today because one control record answers
both questions:

- **Slot custody** — the exclusive right to write a slot's build directory and
  its owned `<slot>/scratch`. It must last while any writer may write, and an
  unknown writer must keep it.
- **Host active-build permit** — one unit of the host-wide
  `max_active` limit. It exists to bound concurrent build load, not to protect
  bytes.

Today an `oxide exec` session holds both for its whole life: the command's
control record is *active* from admission until the tool's group closes
(`src/bin/oxide/store.rs`, `Record::active`; `Store::attempt` counts active
records across the internal and removable pools). A `tauri dev` or similar
open session therefore consumes a host permit while it is not compiling, and
`oxide run` of a long-lived API additionally pins a warm slot because its
child temporary directory lives in that slot (`#136`).

Producer observation (Effigy owner, 2026-10-26; attributed measurement, not a
controlled benchmark, not a general claim): on an Acowtancy scratch checkout a
long-running API under `oxide run` retained its lease and warm slot for the
server lifetime; a respawn took another compatible slot and rebuilt cold —
`slot 2216` at 667 MiB: 83.7 s plus a 189.2 s cold rebuild; a later respawn on
warm `slot 2178` at 15 GiB: 15.6 s plus 48.1 s. These figures are producer
observations; they establish no guaranteed saving, host inventory or causal
proof of every timing.

The two problems are independent, and conflating them is the main trap:

- Releasing an **idle permit** alone does not stop a warm slot from being
  pinned, so it does not fix the measured cold rebuild.
- Releasing **slot write custody** alone does not help the host limit, and it
  is the change that can corrupt a running program (see §14).

Motivations in scope are Tauri-style dev servers (`tauri dev`) in Loophole,
Finch, kimi-shell, Soundcheck and Acowtancy Desktop. No consumer is edited or
probed by this design.

## 3. Current truth this design must build on (source map)

| Fact | Source |
| --- | --- |
| One control record per slot carries `phase`, holder, lease ID and process group; `active()` means "not idle and not retired". | `src/bin/oxide/store.rs` (`Record`, `Record::active`) |
| Host admission counts `state.records.iter().filter(active).count()` plus the sibling pool's active records, and refuses with a host-limit wait when it reaches `max_active`. Slot selection, FIFO ticket, clean barrier and recovery run in the same attempt. | `Store::attempt`, `Store::acquire` |
| A leased command's record is `Acquiring`/`Spawning`/`Running` until it releases after positive group closure; a crash leaves it held for the existing recovery rule. | `Store::acquire`, `mark_spawning`, `mark_running`, `release`; [crash recovery](workflow.md#crash-recovery) |
| `oxide exec` spawns the tool as a group leader, keeps the lease until the leader and group are closed, and reports the holder as `exec <program>`. | [exec](workflow.md#oxide-exec-cargo-wrapping-tools), `src/bin/oxide/main.rs` |
| Every leased command sets `RUSTC_WRAPPER=""` to bypass an inherited wrapper; `OXIDE_ALLOW_RUSTC_WRAPPER=1` keeps it. `RUSTC_WORKSPACE_WRAPPER` and Clippy's driver are never touched. | `src/bin/oxide/wrapper.rs`, [inherited wrapper](workflow.md#inherited-rustc-wrapper) |
| A command's child temp is one decision: a same-pool-volume inherited `TMPDIR` is kept, otherwise one owned `<slot>/scratch` directory with an ownership marker; over-long paths refuse before spawn. | `src/bin/oxide/child_temp.rs`, [#136](workflow.md#child-temporary-files-oxide136) |
| An unbounded `run` without an owned temp directory releases the lease and `exec`s the program; with an owned directory it keeps the lease, the record, the group, the removal watchdog and the slot until the program group is empty. | `main.rs` (`run_owned_program`, release-then-`exec` path), [#136](workflow.md#child-temporary-files-oxide136) |
| A bounded `run` keeps the lease through the program under one cumulative execution window. | [command time budgets](workflow.md#command-time-budgets-oxide140) |
| The directory-pool watchdog samples caller-available bytes about once a second and stops the group below a 20 GiB reserve plus a 2 GiB margin; a debug-only injected reading exists. | `src/bin/oxide/watchdog.rs` |
| Admission/execution budgets, a typed start, a bounded stop escalation and a fail-closed event channel already exist. | `src/bin/oxide/budget.rs`, [budgets](workflow.md#command-time-budgets-oxide140) |
| The control state is versioned (`STATE_VERSION = 5`) with a forward-only fence and installed-binary hand-off for older builds. | `store.rs`, [installed hand-off](workflow.md#installed-binary-hand-off-oxide122) |
| `oxide-managed` is the private control root; `oxide status` by default reports recorded facts only. | [local lease](local-lease.md#oxide-front-door-control-directory), [status](workflow.md#storage-and-space-management) |

## 4. Rulings this design preserves or must ask about

- **R1 — exec reserves its slot for the whole session.** Tom's recorded
  direction keeps the slot (and its owned temp) reserved for the entire
  potentially rebuilding session
  ([vision](../vision.md#product-workflow-direction)). This design never
  releases exec slot custody; it only separates the host permit. No proposal
  here contradicts R1.
- **R2 — `#136` stands until equivalence is proved.** An existing child's temp
  must never be recycled, a slot must not be reused under a running program
  without an explicit successor design, and unknown custody stays held. §14
  therefore keeps `#136` unchanged.
- **R3 — exclusive use, conservative recovery, finite capacity.** One lease
  names one slot generation; unknown writers keep it; pressure never deletes
  active or uncertain storage ([storage invariants](../architecture.md#storage-invariants)).
  Any split must keep these.
- **R4 — no interception of `cargo`.** Vision defers rustup/PATH shims and
  shell aliases for the front door. The seam in §5 is a scoped compiler
  wrapper inside one session's child environment, not a PATH shim, but it is
  still a new mechanism and needs the operator disposition D-2.
- **R5 — no new daemon or background service.** The broker in §8 is part of
  the running command and dies with it.
- **R6 — unknown cannot fabricate idle or closed.** A state that is not
  positively observed is reported as held/unknown; a failed gate never starts
  a compiler ([validation policy](../vision.md#validation-policy)).
- **R7 — the reserve watchdog and removability rules are unchanged.** A
  session is still stopped by the host-reserve watchdog and by a removable
  volume's disappearance at the existing cadence, idle or not.
- **R8 — no consumer, install, host-setting, scheduler or capacity change.**
  Config keys and flags are not added by this design.

## 5. The interception question

The lead's target is: release the permit while no compiler (or Cargo child)
runs, reacquire **before the next compile**, and show ordinary waiting.
Polling a process list or watching files after a compiler has started is
**not** permit acquisition before work and is rejected as the mechanism. So is
any design that infers "probably not compiling" from a quiet process tree, an
idle CPU sample, a build-directory mtime or a filesystem event.

The requirement, stated as a property:

> **P1 — before work.** For a supported compile, the permit is durably held
> before the compiler process performs any work, and the acquisition is
> observed by the session, not inferred after the fact.

Candidate seams, with their coverage and disposition:

| Seam | Pre-work? | Coverage | Disposition |
| --- | --- | --- | --- |
| **S1** `RUSTC_WRAPPER` set to an Oxide gate that hands the real compiler the unchanged argv after a permit grant | yes — Cargo invokes the wrapper before the compiler runs | every rustc invocation Cargo makes through the wrapper, including build scripts, proc-macros, test/bench bins, and child Cargo that inherits the variable | **chosen** (§6–§9) |
| **S2** `RUSTC` pointed at a gate | yes | Cargo's compiler and `$RUSTC` in build scripts | rejected: changes the compiler path Cargo and build scripts observe, risking fingerprints and recorded tool identity, and interacts with wrapper precedence |
| **S3** PATH `cargo`/`rustc` shims, or `$CARGO` | at Cargo start | processes that resolve the toolchain through the inherited environment | rejected for this design: vision's deferred interception (R4); `cargo run`'s program phase would still hold a Cargo-granularity permit; absolute paths and cleaned environments bypass it |
| **S4** per-tool integration (Tauri hooks, wrapper flags) | yes, per tool | one tool | rejected: unbounded per-tool work, consumer edits prohibited, and a tool can still bypass it |
| **S5** process observation (kqueue `EVFILT_PROC`, `proc_listchildpids`, `pgrep`, spawn polling) | no | whatever the sample caught | rejected: post-start observation is not P1; [process scans are refused as descendant-complete proof](../qualification-readiness.md#process-containment) |
| **S6** `ptrace`, Endpoint Security, DYLD insertion, sandbox profile | not supported here | would be near-complete | rejected: unsupported/unauthorised host mechanism, no public contract, and outside the cooperative foreground contract |
| **S7** keep the permit for the whole session | trivially yes | everything | the fallback whenever S1 cannot be installed or the operator rejects D-1/D-2 (no change to today) |

`S1` is the only seam that satisfies P1 with a supported Cargo mechanism
inside the cooperative contract, so it carries the design.

## 6. The chosen gate: properties

- **P1 before work** — the gate is the process Cargo invokes as the wrapper.
  It connects to the session, requests a permit, and only after a grant does
  it start the compiler. A refused, closed or timed-out gate exits without
  starting anything.
- **P2 one permit per session** — the session holds at most one host permit no
  matter how many compiler descendants run concurrently. The session's broker
  refcounts live gated writers and grants against one permit (§8).
- **P3 transparent compiler** — the compiler receives the same argv it would
  have received, the same inherited descriptors including Cargo's jobserver,
  and the same stdio. The gate adds one process between Cargo and rustc and
  propagates signals and the exit status. It does not rewrite arguments,
  environment or toolchain selection.
- **P4 stable path** — the wrapper path is stable across sessions and builds
  (one per-user copy, refreshed by content hash), so no session nonce enters a
  path Cargo or a build script may record. A per-session path would risk
  fingerprint churn and is rejected.
- **P5 fail closed** — any error in the gate apparatus (missing socket, bad
  token, protocol mismatch, broker death, unreadable/writable control state)
  ends with "no compiler started", never with an ungated compile.
- **P6 no daemon, no host change** — the broker is the running `oxide`
  process; the gate is a short-lived child; no service, port, login item or
  machine-wide wrapper is installed. The socket is a path inside the owned
  slot.

## 7. Coverage, precisely

Gated by S1:

- every `rustc` invocation Cargo makes with the wrapper: workspace and
  dependency units, build-script compilation, proc-macro compilation, test,
  bench and example targets.
- child Cargo processes that inherit `RUSTC_WRAPPER`, including another
  checkout's Cargo and a tool's own Cargo call, as long as the environment is
  inherited.
- a `setsid` escapee that still inherits the environment can connect while the
  session lives and is gated; once the session is gone its socket is gone and
  the attempt fails closed. One that rewrites its environment is in the
  ungated list below.

**Not gated** (documented gaps, not claims):

- `RUSTC_WORKSPACE_WRAPPER` and Clippy's driver: Cargo's workspace-wrapper
  lookup is separate (`wrapper.rs` says Oxide never touches it). Whether
  `cargo clippy` inside a session bypasses the gate is **unverified in this
  repository** and is treated as a gap until a source-qualified case exists.
- a build script or tool that invokes a compiler or Cargo through an absolute
  path, its own toolchain, a container, a cleaned/rewritten environment, or
  after `setsid`/daemonising;
- Cargo's non-compiler children: build-script *execution*, test/bench
  execution, `run` programs, linkers rustc drives through its own children
  (the linker is counted only while rustc holds the gate connection; a linker
  that outlives the compiler extends the connection if it inherited it);
- `RUSTDOC`-driven doctest compilation.

The consequence must be stated plainly: **the gate guarantees that *gated*
compiles acquire the permit before work; it cannot guarantee that every
compiler inside an arbitrary tool is gated.** That is the operator question
D-1/D-3, not something this design may paper over by redefining "idle" once.

## 8. Actors, layout and protocol

Actors:

- **Session** — the `oxide exec` process (or a bounded `run`, or an unbounded
  `run` that keeps an owned temp directory). It holds the record lock, the
  slot and the tool's process group.
- **Broker** — a component of the session's existing wait loop (`SessionGate`
  in the design vocabulary). It owns the permit decision, the writer count and
  the durable permit field. It is polled from the existing waits; it never
  blocks them.
- **Gate** — the wrapper process. It is the current `oxide` binary's minimal
  early path (detected by a role variable before any pool or identity work),
  invoked by Cargo as `RUSTC_WRAPPER`.
- **Store** — the existing control state (`state.json`, admission lock,
  tickets, record flock).

Identity: the session is bound to its record's slot generation and lease; the
socket path and token are per-session, and the socket is unlinked at release.
The gate's pid and start time are diagnostic only and never a permit, custody
or recovery decision — no PID, TTL or age inference is introduced (R3). The
token is an accidental-request guard, not a security boundary: a same-user
tool that holds it can request a permit, which is equivalent to that tool
running a build. Every state transition (grant, EOF, release, acquire) is
processed in one broker poll, so a release and a new grant cannot interleave.

Layout, all inside owned space:

- gate binary: `<state-dir>/gate/oxide` (the platform state directory that
already holds `pools.json`), one per-user copy, mode 0700, atomically
replaced when the running binary's content hash differs, and
protocol-versioned (`OXIDE-GATE/1`). The copy is not pool payload and not
part of a slot.
- session socket: `<slot>/g` (a Unix domain socket), created after the lease
  and before the tool starts. The existing 91-byte path rule (`#136`) is
  applied to the longer of `<slot>/scratch` and `<slot>/g`, so a session whose
  socket could not fit refuses before spawning. The directory carries the same
  ownership marker shape as `#136` (store, slot, incarnation, lease);
  same-incarnation leftovers are reconciled by that slot's next lease,
  anything else is kept and reported.
- environment for the tool and its descendants, set only by the session:
  `RUSTC_WRAPPER=<gate path>`, `OXIDE_EXEC_GATE_SOCKET=<socket>`,
  `OXIDE_EXEC_GATE_TOKEN=<32 random bytes hex>`,
  `OXIDE_EXEC_GATE_ROLE=rustc-wrapper`; when the wrapper opt-in is kept, the
  inherited wrapper path is passed to the gate as an inner wrapper (the gate
  runs the inherited wrapper with the real compiler, preserving today's
  opt-in). The gate strips the token and socket from the compiler's
  environment after connecting, so a compiler's own helpers cannot request
  permits, while Cargo's other children keep the token.

Protocol (line-oriented, bounded, fail closed):

1. gate connects, sends `hello <protocol> <token> <gate-pid> <start-ms>`.
2. broker validates the protocol and token; a bad hello closes the
   connection with no grant and records one bounded diagnostic.
3. broker either grants immediately (permit held, or the writer count is
   already above zero) or queues the request and prints the ordinary waiting
   line; while waiting it polls the store's nonblocking admission.
4. on a grant the broker writes `grant <nonce>` and increments writers. The
   gate starts the compiler with the gate's socket descriptor duplicated into
   the compiler with close-on-exec **cleared**, so the permit's writer
   lifetime is the compiler's process subtree; the gate itself also keeps its
   descriptor until it reaps the compiler. The broker sees EOF only when every
   holder closed it, which may be after the compiler exits if a helper
   inherited it (conservative).
5. on EOF the broker decrements; at zero with no pending request it durably
   releases the permit.
6. a request that is cancelled (gate killed, session signal, deadline) is
   closed without a grant and never starts a compiler.

The broker's poll is inserted into the existing wait loops *after* harvest
and signal forwarding, preserving the documented order (harvest → forward →
deadline → watchdog) and the 20 ms child-wait cadence; the permit store poll
uses the existing admission poll cadence. A gate therefore waits at most one
poll interval longer than the store's own decision.

## 9. Splitting the permit from custody in the control store

The record gains one fact: `permit`, either *held* or *released*; a record
with `permit` absent keeps today's meaning (active ⇒ held). Only a
custody-holding session may release it. Nothing else about the record changes.

- **Admission count** — host admission counts records whose slot is active
  **and** whose permit is held, plus the sibling pool's, so an idle session
  does not consume a host unit.
- **Slot rules** — second-slot, per-project cap, compatible-busy waits, clean
  barrier, recovery and eviction keep reading slot activity, never the permit.
  An idle session still occupies its slot and still blocks `oxide clean`.
- **Acquire** — a permit-only acquisition adds no slot work: it registers a
  FIFO ticket, counts held permits across both pools, and grants by writing
  `permit = held` durably *before* the broker tells the gate to start. A
  permit-only request never takes another record's lock and never evicts.
- **Release** — `permit = released` is written durably after the last gated
  writer closed and no request is pending; a failed write keeps the permit
  held, reports the error, and disables further release for the session.
- **Ordering and deadlock** — the existing order is sibling barrier → pool
  admission lock → own record; record-lock probes by other commands are
  nonblocking already. A permit-only request is issued by the record's own
  holder and takes the sibling and admission locks with the nonblocking
  primitive (one attempt per broker poll, retried next poll, never
  `flock_blocking`), so it never blocks the wait loop and never introduces a
  blocking reverse order. A command that is *waiting for a slot* never holds a permit
  (permit and slot are still granted together for non-session commands), so
  the only possible cycle — a session holding a slot and waiting for a
  permit, while another command holds a permit and waits for a slot — cannot
  form. The session's permit wait can still be long when the limit is
  saturated; that is ordinary waiting, reported once, not a deadlock.
- **Fairness and cancellation** — a permit request joins the same FIFO ticket
  queue, so it cannot starve a waiting command and cannot jump ahead of one.
  Cancellation (Ctrl-C forwarded to the group, execution deadline, tool
  death) removes the pending request and its ticket; no permit is left
  attached to a dead asker.
- **Schema** — the control state advances to `STATE_VERSION = 6` with the same
  forward-only fence and hand-off semantics as `#146`: a new field an older
  binary would reject must not appear before the fence is durable, and an
  older binary hands off to a receipt-matching successor rather than
  misreading a session. No new config key, no new flag.
- **Status** — recorded `status` names the holder and `permit: held`,
  `permit: released` or `permit: unknown`, with the same rule that it walks
  nothing, writes nothing and creates no lock. `unknown` is used when the
  durable field cannot be read or validated (for example a legacy record whose
  command predates the field) and is counted as held. The session's summary
  names permit waits in its cumulative wait figure, and one bounded line per
  wait reason (the existing eight-line limit).

## 10. Session lifecycle

1. **Admission** — unchanged: the session leases a slot and, as today, holds a
   permit through its own preparation (freshness gate, output binding, temp
   choice). Nothing compiles before this point.
2. **Gate install** — after the temp choice and before `mark_spawning`, the
   session creates the socket directory, the token and the environment. A
   socket path that cannot fit the platform bound is a pre-spawn refusal, like
   `#136`'s own path rule. Any other installation failure (an unreadable or
   unwritable state directory, a gate copy that cannot be written, an unusable
   socket) makes the session **keep its permit**: it runs as today, reports
   `permit held (gate unavailable: <reason>)`, and the feature degrades to
   current behaviour. It never degrades to an ungated release.
3. **Start and handover** — `mark_spawning`/`mark_running` are unchanged. As
   soon as the broker observes writers = 0 and no pending request, it releases
   the preparation permit. If a gate request arrived first, the broker grants
   from the held permit and no release happens.
4. **Idle** — writers = 0, permit released, slot and temp held, the record
   still active for slot purposes, the tool's group unchanged, the watchdog
   still sampling (R7).
5. **Reacquire** — the next gate request duplicates the permit with the
   store's durable write before a grant. The session prints the ordinary wait
   line if the permit is not immediately available.
6. **Several compilers** — P2: one permit, N writers, no extra permits.
7. **End** — when the tool group is observed empty the session (i) unlinks the
   socket and removes the gate directory, (ii) removes the owned temp under
   the `#136` rule, (iii) releases the permit if still held, then (iv) releases
   the lease. A failed unlink/removal keeps the slot held and reported, as
   `#136` already does for its own directory.
8. **Signals** — forwarded to the group unchanged; a waiting gate dies with
   the group and its pending request is cancelled. Ctrl-C before a spawn keeps
   128+signal.
9. **Budgets** — a session's execution window covers permit waits and never
   resets; a deadline stop kills the group through the existing escalation,
   and a gate blocked on a permit never starts after the stop latches.
10. **Removable volume** — a permit transition never flushes; the flush stays
    where it is (before a slot goes idle). If the volume disappears while the
    session is idle, the existing removal watchdog stops the group and
    custody/quarantine rules apply; the gate cannot connect to a socket on a
    vanished volume, so a compile fails closed instead of running elsewhere.

## 11. Failure, crash and race matrix

| Event | Permit | Slot / temp | Behaviour |
| --- | --- | --- | --- |
| Tool idle, no compiler | released | held | another command may admit; the session's slot is not reusable |
| Two gates request together | one held | held | broker serialises; both counted as writers; no second permit |
| Gate killed while waiting | unchanged | held | pending request cancelled; no compiler started |
| Gate dies after grant, before starting the compiler | held until EOF, then released | held | no compiler ran; the tool sees a failed compiler |
| Compiler killed | held until EOF | held | permit released at closure; the group rule is unchanged |
| Broker (session) killed while idle | released, never fabricated as held | held | socket gone; later gated compiles fail closed; group rule keeps the slot |
| Session killed while a gated compiler runs | held | held | record stays active with `permit: held`; recovery waits for the group, as today |
| Session killed mid-release (write before/after) | held or released, never both | held | the durable write is the decision; a failed write keeps it held |
| Control-state write failure | held | held | no further releases; error reported; compiles that cannot be granted fail closed |
| Malformed/foreign/stale token or protocol | unchanged | held | connection closed, no grant, one bounded diagnostic |
| A stale socket from a previous incarnation | n/a | held | marker mismatch is reported and kept, never removed or used |
| Tool bypasses the gate (absolute toolchain, cleaned env, clippy) | released | held | compile is **not** counted (documented gap, §7); the slot is still protected |
| Host reserve breach while idle | n/a | held | watchdog stops the group and reports at the existing cadence (R7) |
| Removable volume removed while idle | n/a | held | removal watchdog + existing quarantine rules; a compile cannot start |
| Nested `oxide` inside the session | see §13 | held | joining nested work uses the outer permit; an independent nested command takes its own and does not double-count |
| Budget expiry while a gate waits | released/held per durable state | held | the stop latches; the gate never starts; closure rules unchanged |

No row lets an unobserved state become *idle* or *closed*: the permit is only
released after a positively observed writer closure and a durable write, and
the slot is only released through the existing positive-closure path.

## 12. What the supported boundary can actually guarantee

Guaranteed inside the boundary:

- a gated compile never begins work without the session's durably held permit;
- at most one permit per session no matter how many gated compilers run;
- an idle session releases its permit and reacquires it with ordinary waiting;
- slot custody, temp custody, crash recovery, unknown-writer handling and the
  reserve watchdog are exactly as before.

Not guaranteed (and not claimed): that every compiler an arbitrary tool can
reach is gated, that Cargo's non-compiler children are counted, or that a tool
which deliberately leaves the inherited environment still consumes a permit.
Those are the same class of cooperative-contract escape as the documented
`setsid` writer residual, but with a **different consequence**: an escaped
compile is uncounted load, not a storage-safety violation.

**Stop condition.** If the operator will not accept that consequence, or will
not accept the scoped wrapper (D-1/D-2/D-3 below), this design stops and the
permit stays held for the session (S7). It must not be "fixed" by declaring
ungated work out of scope in the code while claiming the target is met.

## 13. Nested lease and borrowed permit

The [nested-lease direction](../vision.md#product-workflow-direction) is a
separate design (lead `fff25973`). This design consumes, and does not define,
its token and join rules:

- a nested `oxide` that **joins** the outer lease takes no new slot and no new
  permit; its compiler descendants are gated by the outer session, so they use
  the outer permit. The idle outer session must reacquire for them exactly like
  any other compile.
- a nested `oxide` that does **not** join takes its own lease and its own
  permit; it must remove the outer gate variables from its children's
  environment before applying its own storage, so its compile is counted once
  (by its own command), not once more by the outer broker. Otherwise a limit
  of 1 deadlocks: the nested command holds the only permit while the outer
  broker waits for one before the same compiler may start.
- a stale or foreign gate token is ignored, never grants a permit, and never
  widens custody; the gate token is not the lease token and grants nothing
  beyond a permit request.
- if a later borrowed-permit design lets an idle session lend a permit, the
  broker must remain the single holder for the session's compilers. This
  design requires only that no path counts two permits for one compile and
  that an idle session has nothing to lend.

## 14. Long-lived `oxide run` servers and slot custody

Three axes must not be conflated:

| Axis | Today | This design |
| --- | --- | --- |
| Host permit | held for the whole session | released in the program phase (unbounded `run` with an owned temp, bounded `run`, `exec`); reacquired by the gate |
| Slot write custody | held while an owned temp exists; released before `exec` when the temp is inherited | **unchanged** |
| Program temp custody | owned `<slot>/scratch`, removed at release after group closure | **unchanged** |
| Runtime references to slot bytes | `env!("OUT_DIR")`-derived paths (and any path the program discovers in the build dir) can point into the slot; the executable and target-directory dylibs live in the checkout's private `target`, which a second slot of the same checkout can already replace | **unchanged**, and the reason §14 rejects the alternatives |

Runtime references that decide whether a slot may be reused under a live
program:

- the program's executable and target-directory dylibs are in the checkout's
  private `target`, not the slot; a second slot of the same checkout can
  already replace them today, which is documented behaviour, not a new risk;
- `env!("OUT_DIR")` and any build-directory path the program discovers or
  keeps open point into the slot, and a compatible reuse rewrites those unit
  directories (an already-open descriptor keeps the inode but sees rewritten
  content where the build truncates and rewrites in place);
- test binaries, build-script outputs and proc-macro dylibs are compile-time
  or execution-time artefacts of a build, not something a `run` program holds
  unless it opened them itself.

Two states exist today and must stay distinct:

- **A — owned temp directory.** `run_owned_program` keeps the lease, the
  record, the group, the removal watchdog and the slot until the program group
  closes. This is the Effigy case: warm slot pinned while the server lives.
- **B — inherited temp directory.** The lease is released and the program is
  `exec`ed in place. The slot is reusable **today**, with no gate and no
  broker afterwards; any runtime reference into the slot is already exposed.
  `#136` chose this deliberately ("a released-lease `exec` is used only when
  no owned directory exists").

Evaluating the two brief alternatives for state A:

- **Option (a): release slot write custody once the build finishes, keep the
  owned temp positively protected.** Rejected. It breaks exclusive use (R3):
  a second Cargo invocation would share one build directory, which Cargo does
  not document or support and which the current exclusive-lease rule forbids;
  a compatible reuse rewrites the very unit directories
  (`build/<unit>/out/...`) whose paths are embedded in the running binary;
  the owned temp needs a new per-incarnation registry, protection from
  reconciliation and eviction, and an eviction refusal while referenced — at
  which point the slot is not released, only renamed. It also introduces,
  into the configuration that promises protection, the runtime-reference
  exposure state B already has.
- **Option (b): place run-child temp outside the reusable slot and release the
  slot when the program starts.** Rejected for this design. It requires a new
  owned temp root outside slots (marker, accounting, crash reconciliation,
  eviction, reserve treatment), changes `#136`'s "inside the slot, counted
  once, deleted with its slot" contract, and still leaves the program's
  runtime references into the *build* directory — now exposed to a concurrent
  reuse, which is exactly the corruption `#136` prevents. A successor would
  have to prove equivalence for both the temp root and those references,
  which cannot be established by inspection today.
- **Option (c): preserve `#136`.** The measured cold rebuild is a *different
  slot* problem; it is the approved cold-slot-seeding direction's job
  ([vision](../vision.md#product-workflow-direction)), not a custody release.
  A respawn that must reuse the warm slot needs the old server stopped first.

Recommendation: implement the permit axis in state A (host permit released
during the program, slot and temp held) and change no slot or temp custody.
This keeps R1, R2 and R3, and it removes the host-permit pinning the lead
asked about without touching the bytes the server may still read.

## 15. Operator dispositions required before implementation

- **D-1 — permit meaning.** For a custody-holding `exec`/`run` session, the
  host active-build permit is held while a gated compiler subtree is live,
  instead of for the whole session. This narrows the current "active command
  counts" meaning for these sessions. Accept / reject. *Reject ⇒ keep today's
  permit and stop this design at S7.*
- **D-2 — scoped compiler gate.** Sessions install Oxide's own
  `RUSTC_WRAPPER` gate and a per-session broker in the child environment; no
  PATH shim, no daemon, no machine-wide wrapper. Confirm this does not
  conflict with the deferred-interception ruling (R4). Accept / reject.
  *Reject ⇒ S7.*
- **D-3 — supported boundary.** Accept that ungated compiles (§7) and Cargo's
  non-compiler children are not counted while a session is idle. Accept /
  reject. *Reject ⇒ S7.*
- **D-4 — run custody.** Confirm R2 is unchanged: no slot/temp custody release
  for `run`; the measured warm-slot pinning is not addressed by this design
  and needs a separate successor design plus its own disposition before any
  implementation. Accept / reject.
- **D-5 — schema and install.** `STATE_VERSION = 6` and the installed-binary
  hand-off are routine follow-through of the existing pattern, but they need
  the same receipt-matching install gate; they grant no extra authority.

Nothing above is assumed granted. Until they are recorded, this file is only a
proposal.

## 16. Implementation slices and dependencies

Sequencing: design review and the owner's design freeze first; then one
approved brief per slice; no slice before D-1..D-4 are recorded.

1. **Slice 1 — permit split, no behaviour change.** `permit` field,
   `STATE_VERSION = 6`, fence/hand-off, admission counts permits, status
   wording. Sessions keep holding the permit, so no user-visible change.
   Depends on `#146` and correction `755757db` (the accounting/eviction
   sections this design references but does not modify); the design does not
   assume that correction's outcome.
2. **Slice 2 — gate apparatus with the permit held.** Gate binary copy,
   socket/marker layout, protocol, broker in the wait loops, transparent
   exec, wrapper chaining, nested stripping, fail-closed. Grants are immediate
   because the permit is held; this slice's purpose is to prove the seam is
   transparent (P3/P4) before any release exists. **Feasibility gate:** if
   gated and ungated builds in one slot rebuild units (fingerprint or
   identity sensitivity), or the wrapper changes recorded tool identity, stop
   — the mechanism would cause the churn it exists to avoid.
3. **Slice 3 — idle release and reacquire** for `exec`, bounded `run`, and
   `run` state A; wait accounting; status; cancellation; budget interaction.
   Depends on Slice 2's feasibility gate passing.
4. **Slice 4 — run slot custody (not part of this design).** Only after a
   successor equivalence design and D-4; option (a) or (b) is not selected.
5. **Cross-cutting dependencies.** The nested-lease design owns the token/join
   rules consumed in §13; the released shared supervisor
   (`scripts/bounded-test.py`, current qualified bytes) owns every native run;
   the installed-binary hand-off owns the version fence; no slice adds a
   config key, flag or host setting.

## 17. Proposed test plan

Tier: **HIGH** authoring complexity and **ELEVATED** (permit release decides
crash recovery and writer closure). Author and independent reviewer natives
are separately activated; before the first counted native the author runs one
separately activated, uncounted keep-going diagnostic of the fixed script. A
source sweep never replaces it. Proposed selector names are not created by
this design.

**Pure regressions (no fixture, no child):**

- store: permit accounting across both pools, legacy record reading,
  version-6 fence and hand-off refusal, ticket fairness, cancellation,
  no permit while waiting for a slot.
- broker state machine: pending → grant → writers N → EOF → release;
  concurrent grants; release/acquire serialisation; malformed hello; stale
  token; write failure keeps held; broker death.
- environment construction: wrapper path stability, token not in `status` or
  compiler env, nested stripping, opt-in chaining, gate-unavailable fallback.
- `effigy proof:exec-session-permits:test` (proposed) runs only these, plus a
  `--self-check` of the proof script's constructor and oracles.

**Native proof** (proposed `effigy rust:exec-session-permits`): one fixed
script that is the whole command of the qualified shared supervisor —
`python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 --
python3 scripts/test-exec-session-permits.py` — heavy admission, 300 s wall
clock, 512 MiB monitored scratch including cleanup, a fresh private
`HOME`/`CARGO_HOME`/directory pool and one tiny std-only fixture workspace per
case, selected-volume scratch, no image, no host state. Staging is this HEAD's
debug `oxide`, offline, pinned 1.97.1, into scratch-owned target/build
directories; fixtures run sequentially at one Cargo job.

Cases:

1. **Idle release** — max-active 1; an exec session runs a tool that waits on
   a release file; a competing `oxide check` in another checkout is admitted,
   builds and exits; `status` names the session with `permit: released` and
   the session's slot stays held.
2. **Before-work acquisition** — max-active 1; the competing build holds the
   permit; the session's tool starts a build; the fixture's build-script
   marker and any `target` output for that crate are absent while the
   competitor holds the permit, and the build completes only after release;
   the session prints one ordinary wait line.
3. **One permit, concurrent compilers** — the tool starts two Cargo builds at
   once; the host count stays 1; both complete.
4. **Repeated idle/compile cycles** — several build→idle→build rounds; each
   idle interval admits another command; every compile is gated.
5. **Transparent exec / fingerprint stability** (feasibility gate) — build in
   a slot with `oxide check` (no gate), then `oxide exec -- cargo check` in
   the same slot and the reverse: no unit rebuilds, and the fixture's
   recorded tool identity is unchanged.
6. **Long-lived program phase** — a `run` with an owned temp: the program
   stays alive; `status` shows the slot held and `permit: released`; a
   competing command builds; the program's temp and bytes are untouched; the
   slot is not reusable.
7. **Tool runs a program via Cargo** — `exec -- <tool>` that runs
   `cargo run` of a waiting program: the permit is released while the program
   runs (this is the `tauri dev` shape) and reacquired for a rebuild.
8. **Crash while idle** — kill the session with the program group still
   alive; a later command does not recover or reuse the slot; `status` shows
   interrupted and no fabricated permit; then kill the group and let the
   existing recovery release it.
9. **Crash while compiling** — kill the session during a gated compile; the
   record keeps the permit until the group is empty; recovery follows the
   existing rule.
10. **Gate EOF / cancellation** — close the broker (kill the session) while a
    gate waits; the compile never starts and the tool reports a failed
    compiler; the slot stays held.
11. **Reserve pressure while idle** — with the debug injected watchdog
    reading, trip the reserve while the session is idle: the group stops, the
    slot stays held, and the report is unchanged in shape.
12. **Nested `oxide`** — a session tool launches a nested leased command
    (independent path): it takes its own slot and permit, the host count is
    correct, no double count, and the outer idle session's permit is
    unaffected.
13. **Gate-unavailable fallback** — force a non-path installation failure
    (for example an unwritable state directory for the gate copy): the session
    runs as today with the permit held and a named reason, and no compile runs
    ungated. A socket path beyond the platform bound is asserted separately as
    a pre-spawn refusal.
14. **Bypass is reported, not counted** (documented-gap case) — a tool that
    compiles through an absolute-path toolchain while the session is idle:
    the session does not claim to have counted it; this case asserts the
    documented limitation, never a permit acquisition.

Every case uses the [proof-authoring premise checklist](workflow.md#proof-authoring-premise-checklist).
Dispositions:

1. **Private HOME and rustup** — capture `RUSTUP_HOME` (or `HOME/.rustup`)
   before replacing `HOME`; keep the pinned 1.97.1 and
   `RUSTUP_AUTO_INSTALL=0`; generate each fixture lock offline inside scratch
   before any `--locked` call.
2. **Account-home control** — `OXIDE_TEST_HOME_FROM_ENV=1` for every fixture
   child of the debug candidate.
3. **Environment isolation** — scrub inherited `OXIDE_POOL`, `OXIDE_CONFIG`,
   `OXIDE_TEST_*`, `XDG_*` **and `OXIDE_EXEC_GATE_*`/`RUSTC_WRAPPER`** before
   setting only the fixture's own values, so a parent proof cannot smuggle a
   gate.
4. **Automatic adoption** — fixtures are unconverted and each case asserts
   that no owned output link was created; no permanent-drive mode is used.
5. **Cargo external argv** — not used (no fake external subcommand); if a
   case adds one, it asserts Cargo's leading subcommand name.
6. **Owned images** — not applicable: no image is attached and no removable
   volume is touched.
7. **Compiler flags** — freeze `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`
   identically for staging and every fixture command; the gate must not alter
   them (case 5).
8. **Temporary storage** — scratch and `TMPDIR` on the selected volume under
   the qualified defaults; no internal-root exception is needed because no
   case requires a boot-volume pool.
9. **macOS mktemp** — every `mktemp` uses a template under the owned scratch
   parent and the fixture verifies the resolved path.

Diagnostic and activation: the script carries a minimal opt-in
`--diagnostic` keep-going mode (`effigy rust:exec-session-permits:diagnostic`,
proposed) that records recoverable oracle failures, skips only
dependency-tainted dependents, aborts on an unknown child/writer/custody or
deadline outcome, and writes a bounded `qualified: false` receipt that is
never a PASS. The counted run is fail-fast. Each side (author, independent
reviewer) needs its own ROOT activation; no clock is reused or reset. If the
fit is unmeasured, say so; the 300 s/512 MiB figure is a planning bound, not a
measurement.

## 18. Not claimed

- No implementation, native PASS, install, release or consumer adoption.
- No measurement of Tauri, Vite, dev-server latency, cold-rebuild savings or
  host throughput; the Effigy figures are producer observations (§2).
- No completeness of descendant gating, no process-census containment, no
  claim that ungated compiles are impossible.
- No slot or temp custody change for `run`; no equivalence claim for option
  (a) or (b).
- No change to `max_active`, the reserve, the watchdog, removable-volume
  policy, budgets, profiles or toolchain support.
- No claim about Linux-native behaviour of the gate; its path-length and
  socket rules are stated for both platforms but only macOS is qualified by
  existing proofs.

## 19. Open questions for the operator

- **Q-A** — Is the permit's purpose "bound concurrent Cargo invocations" in a
  way that requires counting Cargo's non-compiler children (tests, build
  scripts, `run` programs), or is "gated compilation" an acceptable unit for
  long-lived sessions? (§7, D-1)
- **Q-B** — If D-1/D-3 are rejected, should the measured warm-slot pinning be
  pursued through the approved cold-slot-seeding direction instead of any
  custody release? (§14, D-4)
- **Q-C** — Does a scoped `RUSTC_WRAPPER` gate inside a session's child
  environment count as the deferred interception R4 rules out, or is it
  acceptable because it is never machine-wide and never redirects `cargo`?
  (D-2)
