# Project adoption and coordinated slot waiting

For project-owned component-lazy preparation, use the
[optimization adoption policy](optimization-adoption.md). That improvement
requires no shared slot or consumer lifecycle integration. This document owns
the separate reusable-slot adoption design.

Status: proposed consumer design. Oxide now contains an unpublished Rust
library adapter for task-owned fixtures. It defines no production Queue/Nucleus
binding, consumer configuration key, daemon deployment or consumer adoption.
The [declared workflow entry point](declared-workflow-adoption.md) is available
for private qualification. Nucleus removed its former synthetic client; the
[current consumer boundary](#current-nucleus-integration-boundary) owns its
integration status and authority.
The storage rules remain in [architecture](../architecture.md),
product authority remains in [vision](../vision.md), and Q-010/Q-011 hold the
integration design answers. Q-012 remains open for the consumer-owned source
path and task-attempt lifecycle gates described below.

## Recommendation

The following describes the earlier protected route; it is not a requirement
for the lean Nucleus interface now requested in the current boundary below.

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

Recommend a finite consumer-owned pool of physical task-worktree paths, each
with a bounded strict-physical compatibility allowance. Distinct tasks reuse a
path sequentially only after ordinary closeout/archive and independently retained
exact source/artifact/debug evidence. Source tree is per-run identity, not a
per-revision slot key. Cross-root reuse remains unqualified; no logical root
remap is selected for this consumer route.

The detailed first-workflow contract is owned by
[first Nucleus core-test reuse](nucleus-core-reuse.md). It pins the inspected
selector/source, identifies the fixture collector's missing third-party/native
input coverage, and specifies proposed root allocation, source retention,
authenticated attempt/host-run binding and recovery receipts. It is a design,
not shipped fields, consumer consent or execution approval.

Queue Spec 028 currently keeps build outputs local to each task and only shares
Cargo downloads/Git objects. Nucleus Contract 002 keeps distinct task workspaces
and branches, archives successful work after preservation, and retains failed /
unmerged work. The narrow owner decisions are sequential physical path reuse
after archive, bounded shared intermediates outside task checkout, independently
retained exact dirty/generated/dependency source and debug evidence, and a durable
task-attempt acquire/return-to-pending transition. Existing managed-Git and
retention owners must supply those seams; Oxide does not manage Git, copy a
second mutable compilation tree, or recycle a live checkout.

Root allocation lasts through the task's normal workspace lifetime; an Oxide
lease lasts through one compile/test/capture only. Dirty, unpublished, retained,
leased or uncertain tasks occupy their root. Pool exhaustion waits without an
Oxide lease or host-run execution slot, or uses separately bounded consumer-
authorized private fallback. No age/PID/reply-loss cleanup, implicit map growth
or silent eviction is permitted. Invalid reuse evidence is distinct from unknown
ownership: only an explicit proven-Idle whole-generation recovery grant may
replace invalid owned storage. Ordinary acquire grants no deletion authority.

Frozen Contract 010 stays unchanged. Readiness is advisory, consumer task-attempt
authorization precedes acquire, and successful acquire precedes genuine host-run
submission. Busy returns to consumer pending without a host run/token/settlement;
this transition and credential are not current APIs. Nucleus shadows Queue's
host authority, so consumer capability cannot be inferred from a proposed
integration. Real-workflow eligibility, source-pool/lifecycle implementation,
qualification and adoption each retain their scoped owner gates.
Before consumer implementation, Tom must approve its scoped integration brief
and the affected owners must agree the narrow policy/lifecycle changes; the
design and fixture result do not supply that authority.

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

The fixture opt-in described above is an in-process `ProjectDescriptor` with one
exact workflow/selector entry, passed with a `ProjectInvocation` to
`ProjectAdapter`; it has no project-file format, CLI or Cargo interception. The
separate [declared workflow opt-in](declared-workflow-adoption.md) now adds one
versioned declaration file, a read-only plan and an unpublished private
`oxide-declared-workflow` CLI over the ordinary-Cargo route; it is a different,
still unadopted surface that also scans nothing and launches no Cargo. Calls
made outside an adapter, including direct Cargo invocations and undeclared
selectors, are bypasses and carry no Oxide reuse or observation claim.

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


## Current Nucleus integration boundary

Nucleus development and planning belong to Supervisor
`6585d6bf-a1aa-4877-91d6-a675b7ca6282`, following Tom's 2026-10-15 ownership
ruling in Nucleus Contract 001 (recorded at `5b34f80`). The former Sol planner
`cddd99af` has handed over and Tom has archived it; its assessments do not
commit Nucleus. Send future substantive coordination only to the Supervisor,
without prompting active workers.

On 2026-10-16 the Supervisor relayed Tom's replacement ruling: Nucleus
will **not** integrate #060 as reviewed. The signed policy-3 attempt and
consumer callback-service shape conflicts with the simplification reset.
The earlier plan to take that API after the chart port is superseded.
Nucleus requests a same-user local CLI and will own its dispatch/validation
calls, environment forwarding, closeout/cancellation release and private cold
fallback when storage is busy. No current accepted CLI binary or consumer
binding exists; the proposed replacement is described below. Do not restore
the historical adapters or make the protected route a prerequisite.

### Lean local lease interface agreed for delivery

The requested public shape is `lease --project <canonical-root> --profile
<dev|full-debug> --holder <opaque-id>`, idempotent `release --lease <id>`, and
`status`. Lease success returns bounded versioned JSON containing a lease ID
and exact environment; refusals distinguish busy from invalid input or
storage failure. Same-UID cooperative callers and private owner-only local
state replace signed attempts and callback authority. A local OS lock and
durable lease state can serialize CLI mutations without a resident service;
this is a candidate implementation, not a shipped command or lock-only proof
of writer closure. Caller-provided paths must never authorize deletion outside
Oxide-owned storage. Exact JSON, exit codes and binary path belong in the
reviewed implementation handover, not an invented current API.

On 2026-10-16 the Supervisor accepted these boundary clarifications and
required cross-worktree correctness qualification in this delivery:

- `release` is the same-user caller's assertion that every writer using this
  lease has ended, including cancellation descendants, and any outputs it
  needs beyond cache lifetime have been preserved. No signature, callback or
  validation-success receipt is required for this lean ownership assertion.
  Release never asserts tests passed. A repeated release is access-free and
  idempotent. TTL expiry or holder loss marks an abandoned lease; neither
  proves compiler descendants stopped. The Supervisor accepted holding such
  leases until an explicit owner `recover` operation asserts writer closure;
  automatic TTL-based reuse is not required. Nucleus owns its closeout/cancel
  retirement path and validation-process-group termination before release.
  It identified its direct-child-only validation timeout handling as a
  consumer fix to make before adopting the release contract.
- Separate canonical project identity from the actual source checkout path
  through an explicit `--workspace <checkout>` argument. The Supervisor
  accepted it and requires useful registry-dependency reuse across different
  task/validation paths, including bundled SQLite. Path-isolated warm reuse
  provides insufficient benefit and is not an acceptable Nucleus handover.
  Include a focused real cross-worktree correctness qualification in this
  delivery: observe actual dependency reuse, run selected tests every time,
  and verify changed/older-or-equal-mtime local source cannot produce stale
  success when alternating checkout paths. Cargo's source-path/package-key
  behavior is a hypothesis to test, not an already qualified fact. Do not
  restore the old signed input-audit route to pursue this gate.
- Lease-time idle eviction is admission management; it cannot constrain
  writes after return. Reuse the verified bounded payload filesystem for an
  actual aggregate byte ceiling, with durable control outside it. Full writes
  fail normally without releasing a lease. Evict whole idle slots only; active
  or abandoned/uncertain slots remain protected. Directory-only mode, if
  offered, must say budget-only rather than claim a hard cap.

Nucleus explicitly needs no debug lookup after release. Released bytes are
therefore disposable under its chosen lean policy; no native debug retention
receipt is a prerequisite for that consumer handover.

Prefer shared intermediates with private final target output where the pinned
Cargo behavior supports that separation; returned variables must specify the
actual supported choice. On macOS unpacked debug information may reference
intermediate objects. Cache bytes are disposable after a valid release, so
callers needing later source/debug lookup must preserve the needed bytes first
or retain the lease. Do not claim independent debug retention from a copied
binary alone. Do not impose the old signed capture protocol on this lean
route. Existing private cold fallback must omit all acquired shared paths.

The delivery is scoped as two focused PRs: the usable local lease CLI with
bounded backing/admission/idle eviction, then the focused real cross-worktree
qualification and only necessary corrections plus the short consumer handover.
Each needs a checked Queue brief and independent exact-head review. Cross-path
reuse must pass before Nucleus is told the CLI is ready; a precise negative
result is reported instead of handing over a path-isolated substitute. This
is an estimate, not a budget reset or a claim that the gate already passes. Reuse the existing bounded filesystem and inventory implementation;
old authentication/callback plumbing is not required. Exact implementation
briefs must name finite fixture limits and independent review. This
assessment creates no live consumer, host installation or real experiment.

Nucleus intentionally removed its never-live disabled Oxide consumer code
in the 2026-10-15 simplification reset. At reviewed owner inspection of
Nucleus `15de6ac6b4f10bf4e17335fede01670ea2c5fb48`, there is **no current
accepted Nucleus Oxide embedding or installed artifact/bootstrap pin**.
Commit `443e56fe485ef132d67aa82097a624195adfea4f` removed
`storage-attempt.ts`, `oxide-runner-transport.ts`, `oxide-retention.ts` and
supporting tests; `189373b0c296bfc3a4c6d5e8629c6caaa4e55de8` later removed
`validation-client.ts`. Nucleus Contract 011 marks its later implementation
sections as historical. Earlier delivery receipts are not current APIs.

Oxide's ordinary-Cargo library/process and storage client remain upstream
implementation references, including the private
[declared policy-3 route](declared-workflow-adoption.md). They acquire storage and request consumer
submit/capture/not-submitted callbacks; they do not execute Cargo or supply
Nucleus authority. A future consumer integration must start from Oxide's
actual reviewed handover and Nucleus's current owners, not restore the old
fixture bridge as an activation shortcut.

The retained ordinary authentication/readiness vectors and historical
streaming/readback failure cases are reusable design/test material after
remapping and review. The orphaned registered-runner child fixture is not an
ordinary-Cargo v2 transport. Existing root-allocation/managed-Git fixtures
have no production issuer binding; current source-commit/bundle preservation
primitives do not provide native artifact capture. Genuine source/attempt,
submission/settlement, independent native debug/source custody, installation
acceptance and invalidation ownership therefore remain explicit integration
work, separate from the size-bound storage core and from Nucleus's
incremental-policy adoption.

### Environment projection prerequisite

The read-only inspection of Effigy
`b8a6409a2d762c691fd3805f08244a1c9a245ece`,
`src/runner/host_scheduler/submit.rs`, found that its default forwarded runtime
environment includes `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL`, but excludes
`CARGO_BUILD_BUILD_DIR`, `TMPDIR`, `TMP` and `TEMP`. Explicit selected-task
environment references can participate in its existing language-neutral
projection. The inspected Nucleus `check:core:test` selector declares none of
those additional references. Setting only the outer process environment is
therefore insufficient to put Cargo intermediates and temporary output in the
admitted paths.

The consumer scope must establish the exact admitted projection through the
existing execution surface and bind its final values before submission.
Do not add Rust cache policy to Effigy or assume global forwarding. Final
values must reach the actual worker and validation commands; synthetic
callback tests do not establish that projection. Signed-request rules apply
only to the earlier protected route. This is a retained integration prerequisite, not authority
to change Effigy or Nucleus configuration now.
