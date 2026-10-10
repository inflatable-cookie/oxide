# Project adoption and coordinated slot waiting

Status: proposed consumer design. Oxide now contains an unpublished Rust
library adapter for task-owned fixtures. It defines no production Queue/Nucleus
binding, consumer configuration key, daemon deployment or consumer adoption.
The storage rules remain in [architecture](../architecture.md),
product authority remains in [vision](../vision.md), and Q-010/Q-011 hold the
integration design answers. Q-012 remains open for the consumer-owned source
path and task-attempt lifecycle gates described below.

## Recommendation

Use a hybrid boundary: one explicit project-and-workflow opt-in, plus a
consumer-owned adapter that invokes the project's existing Cargo command or
Effigy selector. Queue or Nucleus supplies the admitted run identity and decides
when the work may run. The adapter resolves the Rust inputs, acquires the
compatible Oxide slot, and sets Cargo's per-invocation intermediate build
directory. Cargo's normal `target` output and the consumer's captured run
artifacts remain private to that checkout or run.

Keep the opt-in in the consumer's existing project/workflow registration. It
names the Rust workspace and the exact existing selectors allowed to use the
facility. It is off by default. This is one control-plane registration per
adopted project/workflow; it does not require changes to `Cargo.toml`,
`.cargo/config.toml`, or an Effigy manifest. The owning consumer must choose the
concrete field in its own integration brief; this document does not invent an
Oxide config file or claim that such a field already exists.

| Surface | Benefit | Limit | Recommendation |
| --- | --- | --- | --- |
| Cargo configuration alone | Familiar to every Cargo caller, including editors and test tools. | A static build-directory path cannot authenticate the run, coordinate admission, take an exclusive lease, or preserve per-run capture. A project or global setting that points at shared mutable storage would let direct callers bypass ownership. | Do not use as the integration by itself. Never point shared Cargo configuration at an Oxide slot. |
| Explicit adapter alone | Carries consumer admission, run identity, cancellation and capture context. It can use Cargo's standard build-directory setting for one invocation. | Every adopted invocation must pass through the adapter; direct commands and editor actions otherwise bypass it. | Keep as the safety boundary, but pair it with a simple opt-in. |
| Hybrid opt-in plus explicit adapter | Leaves familiar commands and selectors in place while making only declared workflows eligible for reuse. | Editor actions reuse storage only when the consumer can route those actions through the same adapter. | Recommended. |

For a managed invocation, use Cargo's documented per-invocation build-directory
mechanism where the pinned Cargo supports it. This relocates intermediates; it
does not itself provide slot ownership or cross-path correctness. The candidate
Cargo mechanism and its limits are described in the [Rust DX landscape](../rust-dx-landscape.md)
and [storage architecture](../architecture.md#exact-run-evidence-and-cargo-boundary).
Do not share `CARGO_TARGET_DIR`: it contains final outputs and cleanup scope.

### Familiar commands and safe bypass

- A worker still runs its usual `cargo check`, `cargo test`, or existing project
  command. An Effigy-backed workflow keeps the same selector and lets Effigy
  retain its language-neutral process and lifecycle role. Oxide adds no global
  shell, `PATH`, Cargo, editor, or Effigy interception.
- An editor or test-tool action is eligible only if Queue/Nucleus explicitly
  routes it through the same adapter with a valid run envelope. An editor's
  independent Cargo process remains an ordinary private build until it is
  deliberately integrated. No IDE-specific sharing is implied.
- An unopted project, an unlisted selector, or a direct command that cannot
  present the required identity uses its existing private build directory.
  If an opted-in invocation cannot contact Oxide or resolve a relevant input,
  it may fall back to a private build only when consumer policy allows it;
  otherwise it refuses before starting Cargo. Neither path claims a cache hit
  or emits managed-reuse evidence.
- Build reuse never skips the requested tests. Tests execute for the admitted
  run, and logs, binaries, test output and receipts stay in the consumer-owned
  capture area. The reusable slot is not the run's artifact store.

## Identity and compatibility

Keep the two identities from the storage architecture:

- **Compatibility key:** stable project and component/workflow identity; exact
  Cargo and rustc provenance; host and target; profile, features, effective
  Cargo configuration and relevant compiler, linker, SDK and environment
  inputs. Bind resolved values or stable digests, never secrets. Include the
  checkout path until cross-path portability is proved. Unknown relevant inputs
  mean private build or refusal, not a broader match.
- **Run binding:** consumer, task, workspace, admitted run and attempt IDs when
  present; exact source tree and submodules; compatibility-key digest; and the
  admission reference. Source revision belongs to this run binding and Cargo's
  own fingerprints still decide which build units need rebuilding.

A pending request exists before an admitted run does. Its stable identity is
`(consumer, project, workflow/selector, task, request_id)`. The consumer keeps
that identity while the task waits. Once it admits an attempt, it adds the
authenticated run and attempt IDs. Reconnecting or repeating the same operation
reuses its operation ID and identical payload. A new attempt gets a new attempt
ID and operation IDs; it never inherits an earlier lease. These identity tuples
describe semantics, not existing wire fields.

## Stable source-path lifecycle

This is a proposed Queue/Nucleus integration contract, not a shipped API or an
approval to change either consumer. Read-only inspection found that Queue
currently creates task-specific checkout roots, shares Cargo downloads through
`CARGO_HOME`, keeps build outputs private to each task, and cleans them only
after closeout proofs (Queue Spec 028). Nucleus Contract 002 preserves one
logical workspace and branch per task and retains failed or unpublished
workspaces; successful archival removes the checkout after artifact
preservation. Its managed-Git direction can share Git objects without sharing
mutable worktrees. Contract 010 is frozen: Queue admits and launches host runs
through one scheduler path; Nucleus shadows it and its current host service
does not implement host-run scheduling. None of these contracts currently
provides a reusable finite task-root pool, a task-attempt credential for Oxide
acquisition, or a dirty-source snapshot receipt.

Compare these two consumer-owned arrangements:

| Arrangement | Recurrence and ownership | Cost or blocker | Decision |
| --- | --- | --- | --- |
| Finite pool of task-worktree paths | A Queue/Nucleus task workspace occupies one registered canonical path exclusively. After ordinary archive proves the workspace safe to remove, a later, distinct workspace may use that same path. Every root is part of one fixed complete map, so tasks assigned to different roots keep the same compiler flags. | Requires a bounded path allocator integrated with managed Git/worktree creation, workspace identity and archive. Failed, unmerged or uncertain work keeps its path and can exhaust the pool. Exact dirty/untracked source must remain reproducible before a path is recycled. | **Recommend for the next consumer design brief.** It reuses the actual task worktree and adds no second source copy. It remains unimplemented and unqualified. |
| Explicit stable compilation-source view | The consumer materializes each authorized task source into a separate stable view and runs Cargo there while retaining the ordinary task checkout elsewhere. | No current Queue/Nucleus view or snapshot contract was found. Copying files increases source storage and requires dirty-source sync, generated-input handling, sibling-relative-path preservation and exact source return. A filesystem snapshot/reflink would need its own supported consumer contract. It is not a safe assumed fallback. | Defer unless the managed-Git/worktree owners identify an existing supported copy-on-write view that avoids full copies and proves exact source behavior. |

The pool's stable identity is its fixed, ordered set of root IDs, canonical
paths and logical compiler prefixes plus the remap policy. Register the whole
set before mapped use. It is independent of each task's mutable source
revision. Queue/Nucleus binds every build receipt to the task/workspace/run/
attempt, assigned root, exact commit/tree and submodules, any permitted dirty
source delta, root-set/policy digest, effective configuration/toolchain,
Oxide lease, and independent capture receipt. A source revision change under
the same roots does not change compiler flags; Cargo fingerprints and explicit
source/configuration/toolchain miss controls must still work. Any changed root
set or prefix changes the map and produces a cold partition. Never add a root
to chase a hit or evict an old partition as if that were a recurrence.

The consumer's root reservation lasts for the task workspace's normal lifetime;
the Oxide lease covers one compile/test/capture run only. Reacquire for each
managed selector. Do not hold an Oxide cache lease while a task edits or waits.

For each opted-in selector, the map must cover every relevant Rust source root,
including any sibling checkout or generated source tree that affects the
build. If the consumer cannot register and retain a stable path for a relevant
root, that selector remains on its private path; the map must not omit the root
to manufacture a hit.

### Preparation through retained source

The proposed consumer implementation is small in responsibility, though it
touches lifecycle seams:

1. **Prepare:** the opted-in Queue/Nucleus repository/workflow registration
   names a finite set of stable task-worktree paths and the exact selectors
   allowed to use mapped reuse. This is a proposal for a consumer-owned field,
   not an existing setting. Resolve/canonicalize the complete map before the
   first managed build. Include all relevant source roots for the selector.
   Create a checkout only when a task actually takes that slot; do not
   pre-clone or seed idle roots.
2. **Authorize the request:** the consumer checks task dependencies, project
   opt-in and exact selector eligibility under its normal rules, then keeps a
   stable request identity while the work is pending. This is task-level
   authorization, not worker/attempt admission or a host-run token.
3. **Wait:** if no root is free or Oxide reports no compatible storage ready,
   Queue/Nucleus keeps the request in its own pending lifecycle. Register a
   cancellable wait by stable request ID and resume on a state-change hint; do
   not poll, prepare a task checkout, start a worker/build, consume a host-run
   execution slot or take an Oxide lease while waiting. A full/uncertain pool
   holds the request visibly; it does not trigger automatic cloning, cleanup,
   map growth or a private claim of reuse.
4. **Admit attempt and acquire:** after a wake, normal consumer admission
   authorizes the task attempt and assigns an available path to its own
   workspace. The consumer records that source revision and sends Oxide a
   task/attempt authorization that authenticates `(consumer, project,
   workflow, task, workspace, run, attempt, root_id, root_set_digest)` plus
   exact source evidence. This is a required proposed capability, not a
   current Queue or Nucleus API, and is separate from `HOST_RUN_TOKEN`. The
   adapter then sends one authenticated, idempotent acquire. If it races and
   returns `busy`, submit no host run and start no command; the consumer must
   release/park the authorized attempt and return the request to pending with
   no execution slot held. This needed return transition is not established by
   the inspected contracts. It uses no new `host.run` outcome or fabricated
   `launched: false` settlement. If acquire succeeds, persist the durable
   lease before continuing. An acquire timeout/lost reply reconciles the same
   acquire ID; it never creates a second lease.
5. **Existing host-run admission/launch:** only after successful acquire does
   the adapter submit the already-supported heavy Effigy selector to the
   current host-run scheduler. Contract 010 remains the only host admission,
   launch, cancellation and process-settlement authority. It creates the real
   host-run ID and supplies its genuine token to the launched process; Oxide
   must not mint, forge or substitute `HOST_RUN_TOKEN`. If the existing
   scheduler settles a queued pre-launch cancellation or capacity timeout,
   use that authentic envelope to reconcile the exact lease. Do not add a
   storage-busy settlement. While the scheduler queues after acquire, Oxide's
   storage lease remains reserved, but no host CPU/memory execution slot has
   been admitted.
6. **Test, capture and settle:** run the requested selector and tests normally.
   Queue/Nucleus owns the host-run terminal receipt, exact source identity,
   captured logs/binaries/test evidence and retention. The adapter binds the
   real host-run ID/epoch, lease, source/root-set digests and independent
   capture-receipt digest into the Oxide run receipt. Finalize only after the
   consumer's ordinary host-run/process proof and capture receipt agree.
   Missing, mismatched or lost evidence leaves the Oxide slot `uncertain` and
   unavailable; a cache hit never changes test settlement.
7. **Retain and recycle source:** the consumer keeps the task's root assigned
   through its normal workspace lifetime. Before archive/reuse it preserves
   the exact run source and receipt outside that mutable checkout. A clean,
   published head can use the consumer's normal closeout and artifact
   preservation gates. Dirty/untracked source needs an exact retained delta
   or remains private/held. Failed, unmerged, cancelled or uncertain
   workspaces follow current retention rules and keep their path occupied.
   Once ordinary archive is proved, Queue/Nucleus may recreate a new task
   worktree at that path; it does not reset a live checkout or ask Oxide to
   retain source.

Under [useful-reuse admission](../qualification-readiness.md#task-019-useful-bounded-reuse-across-changing-tasks-design-only-2026-10-10),
strict-physical partitions already make a cross-root pair a miss: task A at
`r0` and task B at `r1` never share a slot, even with identical trees. B must
not execute A's intermediates and must not be counted as a warm hit. Useful
recurrence is a later, distinct workspace that reoccupies the same physical
root after ordinary archive, with a new task/run/attempt identity and
possibly a different source tree. Oxide reuses that root's exclusive slot;
Cargo keeps unmodified dependency work and rebuilds changed workspace units.
Same-root plus same-tree retry is a control, not satisfaction of useful
reuse. A sequential allocator check that assigns a freed path is the
consumer path-pool prerequisite for everyday relief; the Oxide same-root
proof can run on a fixture path without that pool. Follow with the
mtime-stale content refusal and separate configuration/toolchain miss
controls; tests still run. If any root, flag or policy changes, record a
cold partition instead of counting it as reuse.

The finite path count bounds concurrent workspaces eligible for this map,
not the full-debug cache bytes; that footprint remains unmeasured. Count every
registered checkout, slot, redirected temporary file and retained run capture
inside the applicable bounded-storage policy. The map does not make source
roots free, and retained failures can reduce usable pool capacity until normal
consumer disposition. Never evict leased/uncertain cache state or delete
task-owned source/evidence to make room. A task that cannot obtain a pool root
uses its existing private path only if the consumer explicitly allows that
fallback; it makes no remapping/reuse claim.

Before implementation, Tom must approve a new consumer integration brief
that binds the finite root registration and workspace-path reuse to the
Queue/Nucleus managed-Git/worktree owners, and identifies who implements the
task-attempt authorization, acquire/cancel/reconcile receipt, exact dirty-source
retention and archive gate. That brief must treat useful-reuse admission as current Oxide policy: a
cross-root pair at `r0`/`r1` is refused as a hit; everyday relief needs the
same physical `root_id` after ordinary archive, or Tom chooses a different
mechanism. Do not ship Task 018's origin+tree match as that policy: it only
gives same-tree retry. Queue host-run owners must confirm this ordering
against frozen Contract 010; Nucleus owners must confirm the workspace and
retention mapping against Contract 002 and the same frozen 010. The consumers
must not be treated as interchangeable until Nucleus actually supports each
required capability. Oxide implementation, real-project qualification and
rollout each need their own approved scope. The next qualification must still
measure full-debug bytes and preserve source/configuration/toolchain
invalidation, behavior-bearing paths, independent artifacts and conservative
uncertainty.

## Readiness, admission and acquire ordering

Readiness is advisory and never reserves a slot. A compatible slot may become
busy after a ready response. The smallest safe waiting contract is:

1. Keep an ineligible task in Queue/Nucleus's own pending queue. Register one
   cancellable storage wait using its stable request identity and compatibility
   key. Registration does not create a run, start an agent, take an Oxide lease,
   or consume a milestone QA slot.
2. Oxide notifies compatible pending requests when the relevant slot state
   changes. It does not order requests, admit work, choose an agent, reserve a
   host resource, or decide retry priority. Queue/Nucleus applies its own
   dependency, priority, fairness and resource rules when reconsidering the
   task. The consumer's existing admission is authoritative.
3. After normal Queue/Nucleus task-attempt authorization and source-root
   assignment, but before submitting the heavy host run, the adapter sends one
   authenticated acquire bound to that task attempt. Oxide durably records
   exclusive ownership before returning a lease token. Only then may the
   adapter submit the existing host-run request and start Cargo against the
   selected intermediate directory. This separates task-attempt authorization
   from Host Run admission; the consumer credential is a required proposal,
   not a current API or a substitute for `HOST_RUN_TOKEN`.
4. Hold the lease through the command's descendants, required test execution,
   and independent capture receipt. Finalize only after the consumer and process
   guardian prove the run has stopped and its evidence is captured, following
   the [storage recovery contract](../architecture.md#durable-ownership-and-recovery).

The consumer must reconsider pending work on a readiness event, not poll or
retry in a loop. Wakeups are hints and can be coalesced; consumers recheck the
current state before admission. Oxide must not implement a second host scheduler
or claim Queue/Nucleus fairness. Exact waiter fairness and operator-attention
thresholds remain provisional; no numeric timeout or retry count is selected.

If readiness races with another valid claimant, acquire returns `busy` before
any host-run submission and starts no build. Queue/Nucleus must return that
authorized request to its own pending lifecycle and reconsider it after a
newer readiness event; it must not create a `host.run` record or translate
`busy` into `cancelled`, `failed` or `capacity_timeout`. Read-only inspection
did not find a current task-attempt-to-acquire/return transition for this case.
Its exact consumer-owned representation is therefore a prerequisite, not an
existing API. Do not wait while holding a host execution slot, and do not
automatically replay a lost or uncertain acquire. Reconcile the same acquire
ID first. If Queue/Nucleus cannot represent this pre-host-run wait/refusal
without falsifying a run outcome or changing frozen Contract 010, stop before
adapter implementation and obtain the narrow lifecycle ruling from its owner.

This boundary preserves the existing ownership split: Queue/Nucleus control
tasks, agents, QA slots and settlement; Oxide controls compatible Rust storage
and execution behavior; Effigy remains language neutral. It aligns with Queue's
private Cargo-output and worktree rules (Spec 028), Nucleus Core Contract 002,
and frozen Host Run Scheduling Contract 010. None is amended here.

## Refusal, cancellation and retry ownership

- **Busy:** a compatible slot is currently leased. A registered request may
  wait without acquiring it. A post-wakeup acquire refusal happens before
  `host.run` submission and starts no command. The consumer must return the
  task request to its own pending lifecycle and wait for a state change; that
  transition still needs owner approval and implementation.
- **Full:** the configured cap/reserve is reached, unknown, or unenforceable,
  or no compatible owned capacity can be made available safely. Refuse with
  the measured/reserved state and owner-visible next step. Do not wait for a
  configuration change, delete active state, or evict uncertain state.
- **Uncertain or unreachable daemon:** report unavailable. Unknown lease state
  stays unavailable and remains accounted. Do not start in the shared
  directory, guess from age or PID absence, or retry a new acquire ID. A lost
  reply is reconciled using the same operation ID.
- **Cancellation while waiting:** Queue/Nucleus cancels its task/request and
  removes the corresponding wait registration. Cancellation before acquire
  releases no lease. Replayed cancellation is idempotent.
- **Cancellation after acquire:** Queue/Nucleus owns the cancellation decision.
  Before host-run submission, release is allowed only with authenticated
  task-attempt cancellation evidence proving that no host run was submitted;
  this consumer capability remains a prerequisite. After host-run submission,
  use its genuine cancellation and settlement path. The adapter forwards
  cancellation to the process guardian, captures available run evidence, and
  finalizes only after descendants are proven stopped. A cancel request, test
  result, daemon restart or lost response alone does not release the slot. If
  acquire or host-run reply state is unknown at cancellation time, reconcile
  the same operation ID; keep the slot unavailable until proof resolves it.

The consumer owns retry and admission. The adapter may retry an identical
operation only with the same ID and payload to resolve a lost response. It may
not create a second build, invent a new attempt, or perform timer-based retries.
After a settled refusal, a later consumer attempt uses a new attempt ID and a
new acquire ID. Pending waits may remain visible without holding execution
capacity until a compatible state change, explicit cancellation, or an
owner-visible hold such as unresolved daemon recovery.

## Example flows

**Normal flow.** A project opts in one existing Effigy test selector. The
consumer leaves its task pending while no compatible root or Oxide slot is
ready. Oxide signals a readiness change; Queue/Nucleus applies its normal
scheduling rules, prepares the workspace and supplies the authenticated task
attempt envelope proposed above. The adapter acquires the lease before
submitting the existing heavy Effigy selector to the host-run scheduler. The
scheduler issues its genuine host-run ID/token and launches the selector, which
runs tests with Cargo's intermediate directory set for that lease. Queue/Nucleus
captures its normal output and receipt, descendants stop, and the adapter
finalizes. Per-run output is retained independently of the reusable
intermediates.

**Busy after wakeup.** Two pending tasks receive a readiness event. The
consumer authorizes one attempt and its acquire succeeds. If another
authorized attempt reaches acquire and gets `busy`, it starts no Cargo process
and submits no host run. Queue/Nucleus must return that request to pending
without holding an execution slot, then reconsider it after a new state event.
That return transition is a consumer integration prerequisite; it is not an
existing host-run settlement. No retry loop is allowed.

**Full capacity.** Preflight reports that the enforceable cap/reserve cannot
admit the requested compatible storage. The adapter returns a full refusal
with measured/reserved capacity and any blocking owner. The task remains under
consumer control; no automatic cleanup, new budget or shared build is
attempted.

**Uncertain daemon.** The daemon restarts with a lease lacking complete
process/capture proof. Readiness reports unavailable. Queue/Nucleus keeps the
task pending with the reason visible. Oxide preserves the uncertain bytes and
waits for authenticated reconciliation. Age, a missing PID or a new worker does
not make the slot ready.

**Cancellation.** A task cancelled before readiness removes its wait and never
acquires. If cancellation arrives after acquire but before host-run submission,
the consumer must authenticate that no host run was submitted before the lease
can be reconciled. After host-run submission, the consumer stops it through its
normal process guardian; the adapter captures evidence and releases the lease
only after shutdown and capture are proved. A lost acquire or host-run reply is
reconciled before either path is chosen.

## Minimum adapter vocabulary

These are semantic capabilities and result categories, not command names or
wire states. The Rust fixture prototype covers the bounded subset described
below; it does not provide a consumer integration.

| Capability or result | Meaning |
| --- | --- |
| `opted_in` / `not_opted_in` | The exact project/workflow is declared eligible, or must use its private path. |
| `ready` | A compatible slot was observed; this is advisory and grants no ownership. |
| `wait_registered` / `wake` / `cancelled` | A cancellable request is waiting for a state change, was notified, or was withdrawn. |
| `admitted` / `not_admitted` | The authenticated consumer has or has not authorized this run. Acquire requires `admitted`. |
| `acquired` / `busy` | Durable exclusive lease granted, or no lease granted because another owner is active. |
| `full` / `incompatible` / `input_unknown` | Safe bounded capacity, key compatibility, or a required input cannot be established. Refuse shared reuse. |
| `daemon_unavailable` / `lease_uncertain` | Readiness or ownership cannot be proved. Fail closed; do not infer availability. |
| `reconcile_required` / `finalized` | Resolve a prior operation by its stable ID, or confirm safe release after capture and process-stop proof. |
| `private_build_allowed` / `private_build_refused` | Apply consumer policy when shared service or identity is unavailable; private execution makes no reuse claim. |

At minimum the adapter must be able to pass an authenticated consumer envelope,
resolve or refuse an effective compatibility key, register and cancel a
state-change wait, acquire exclusively with idempotent operation identity,
retain independent run capture, and reconcile uncertainty. These remain the
consumer-facing design requirements. The fixture prototype resolves explicitly
provided toolchain, platform, profile/features, allowlisted configuration and
environment values; it refuses incomplete inputs and Cargo older than 1.91,
partitions by canonical checkout path, and passes per-invocation
`CARGO_BUILD_BUILD_DIR` and `CARGO_TARGET_DIR` overrides to an injected host
boundary. It does not scan or launch a real project.

The current opt-in is an in-process `ProjectDescriptor` with one exact
workflow/selector entry, passed with a `ProjectInvocation` to `ProjectAdapter`;
there is no project-file format, CLI, or Cargo interception. Calls made outside
that adapter, including direct Cargo invocations and undeclared selectors, are
bypasses and carry no Oxide reuse or observation claim.

The fixture flow waits without a lease, performs at most one readiness wake and
recheck, accepts an injected consumer authorization, acquires exclusively, then
submits a distinct host-run identity. Busy/full is returned to the fixture
consumer before a command runner is invoked. The fixed native fixture runner
waits on its owned child handle and can mint the private completion certificate
only for its exact lease, slot, host-run/run/attempt, process marker and output
bundle. The consumer receipt must match that bundle before the storage core
accepts the lease-token-bound evidence for idle finalization. A caller-supplied
`FixtureCompletionProof`, leader exit or capture receipt alone is insufficient.
The retained-writer, cancellation and mismatched-capture cases stay unavailable
even if a known child is later reaped.

This native boundary qualifies only the registered cooperative fixture
contract: a trusted foreground workflow waits for writing children and closes
its write handles before completion. Hidden process departures and unrelated
same-user writers are unsupported bypasses and cannot be detected or fenced by
this prototype. Its fixed fixture bounds and host free-space samples do not
qualify arbitrary Cargo output or a production cap.
Observations remain in an in-process 16 MiB ring with at most 12 events per run
and 1 KiB per event. Records are keyed by consumer, run and attempt identity and retain
workflow, selector, commit, source-tree digest and partition/configuration
digests; unavailable write attribution is recorded explicitly.

The injected authorization/host-run split is fixture scaffolding, not an
existing authenticated Queue/Nucleus capability. Q-012 remains unresolved for
real bindings. The prototype does not solve arbitrary process containment,
cross-path correctness, a persistent observation service, a real Cargo pilot,
consumer adoption, host/service installation, or numeric fairness/timeout
policy. Each remains separately gated and requires its own approved scope and
review.
