# Ordinary-Cargo same-root storage route

Status: proposed qualification-only design, read-only source inspection on
2026-10-11. This defines one graph-free route for a single fresh consumer-owned
source workspace and sequential attempts at that same physical root. It is not
an installed API, accepted Nucleus workflow, production input policy, or
authorization to execute Cargo. The task-specific implementation and any later
real proof need their own checked scopes.

## Decision and boundary

Use ordinary Cargo as the freshness authority for one explicitly accepted,
cooperative workflow. Keep the existing durable Oxide storage client and the
consumer's current StorageAttempt, Effigy 010 submission, settlement, and
retention owners. Add one qualification-only route that acquires storage before
it asks the consumer to submit a host run. The route invokes the exact approved
Cargo selector on the consumer's source root, with the acquired slot as Cargo's
build directory, and always asks that selector to run its declared tests.

The consumer gives this route one fresh physical root and owns its allocation
for the whole task lifecycle. It may make sequential attempts there, with a new
run and attempt identity for each invocation. This design does not recycle a
root between tasks or promise cross-root reuse. The source tree and content
digest identify each attempt; they are not a per-revision slot key. Root ID and
allocation epoch remain authenticated consumer identities. The compatibility
partition binds the physical root and accepted Cargo/tool/configuration
environment. Task, workspace, branch, run, attempt, source, root allocation,
storage instance, slot, generation, and lease identities remain distinct.

This is a cooperative policy departure from the registered
`oxide_storage::registered::Runner` route. It assigns ordinary Cargo freshness
and declared project behavior to a bounded consumer-approved workflow. It does
not weaken, bypass, or relabel the registered route's audited graph, immutable
input receipts, tool manifests, or exact environment evidence. The registered
Runner remains unchanged and is still no-go for real Nucleus without its
existing full input boundary. This proposal creates no graph resolver, parser,
compiler wrapper, scheduler, trusted issuer, or generic provenance wrapper
chain. Task 043's parser schema is not a dependency.

## What exists and what is missing

Shipped Oxide storage APIs include `storage::Client::acquire`,
`lookup_acquire`, `finalize`, and `lookup_finalize`, plus the durable
`(storage_instance_id, slot_id, generation)` identity and lease token. Lookup
methods are read-only: `Absent` permits only an exact replay of the original
operation; `Present` and `Uncertain` grant neither a new attempt nor access to
slot bytes. Acquire can allocate on a miss, so it never substitutes for lookup.
The lease token remains in the Rust host process and does not enter Nucleus
logs, frames, or ordinary ledger records.

Those generic APIs do not implement this route's selection policy. Generic
`Client::acquire` can return an idle slot as `reused=true`, and its capacity
path can evict the oldest measured idle slot. The durable slot state records
occupancy, not Cargo reuse eligibility, so it cannot distinguish a reusable
generation from a failed/cold-only one or an in-flight pending generation.
The current `same_root_fixture_v1` path identity is fixture-only and refuses a
second partition under that root. None of those behaviors can be treated as
ordinary-Cargo route support.

The proposed route therefore needs a narrow storage seam, not just a caller
around generic acquire: a state-aware `Client::acquire_ordinary_cargo` (or an
equivalent explicit policy input to acquire), and versioned reuse metadata in
the existing durable ledger. The proposed `CargoReuseState` (`empty`,
`pending`, `reusable`, `cold-only`) is per generation and remains separate from
the existing slot occupancy states (`Idle`, `Leased`, `Uncertain`). Its
selection policy must lease a compatible reusable generation only when its
state is `reusable`; choose a clean, already-budgeted empty generation for
cold work; return pending/refusal for `pending`, `Uncertain`, unknown, or
incompatible state; and never return `cold-only` as warm. Capacity exhaustion
returns refusal without calling the generic LRU eviction path. The route's
proposed path identity is `StrictPhysicalCargoRoot`: canonical physical
source-root path plus authenticated consumer root ID and allocation epoch,
with a bounded partition key of `ordinary-test` or `full-debug`. This route
may use only the owner's R×P=2 allowance: one generation per partition, with
no spare successor and no implicit growth. It does not broaden
`same_root_fixture_v1` or generic storage behavior.

The durable barrier belongs in that versioned storage-ledger record, bound to
`(storage_instance_id, slot_id, generation, lease_id)` and the original
operation ID. The state-aware acquire transaction first selects and leases the
exact generation, then commits its `pending` reuse state and lease together
before returning the acquired tuple to the host. This ordering makes the
barrier follow generation selection without exposing a callback window before
the barrier exists. It is not a Nucleus-only flag or sidecar file. The shipped
ledger has no such field today: the implementation must add an explicit
compatible ledger version and refuse unknown versions rather than reinterpret
old records. The pending marker also preserves the exact pre-acquire reuse
state (`empty` or `reusable`). Authenticated finalize transitions the record
to `reusable` only after successful tests and complete capture, or
`cold-only` after an authenticated settled failure/invalidation. A positive
authenticated no-spawn settlement restores that exact saved state and releases
the lease without consuming another generation. An uncertain writer stays
`pending`; lookup cannot clear it.

The registered `RunnerHost` calls the graph-backed `Runner::run`. Its
`ExecutionOwner` callbacks are behind registration, input audit, reuse checks,
and the pending-map barrier. The project `ProjectAdapter` and
`CargoWorkflowRunner` are fixtures; the latter executes a bounded local
qualification command and is not the genuine Effigy host path. The private
qualified runner is not consumer or production authority. None of these
surfaces is a ready-made ordinary-Cargo Nucleus route.

Nucleus already has a durable `StorageAttempt` ledger and the reviewed
`validation-client.ts` submission path. Its opt-in transport exercises a
synthetic child channel; production registration remains disabled. There is no
current Nucleus lookup wire operation for Oxide acquire/finalize recovery and
no installed ordinary-Cargo callback schema. Do not have TypeScript implement
the Oxide storage codec or settlement evidence, call `Runner` without its
required evidence, or submit directly through a local native Cargo runner.

## Cargo freshness responsibility

For a narrowly versioned workflow, ordinary pinned Cargo decides whether its
units are fresh. The consumer and project owner must accept the following
cooperative conditions before warm reuse is eligible:

- One clean, committed source tree per attempt, with exact commit, tree,
  content digest, submodule/object availability, and a consumer-owned physical
  root ID and allocation epoch. Dirty, ignored, generated, submodule, or
  otherwise unretained inputs are ineligible unless their exact bytes and
  behavior role are covered by the same reviewed source contract.
- An accepted, pinned Cargo/rustc/sysroot/host-target and native tool/SDK
  installation epoch. Its owner guarantees no replacement throughout each
  lease and attempt. A tool, SDK, linker, sysroot, or accepted installation
  change invalidates the entire compatibility partition. A version string,
  path, executable hash, or caller assertion alone does not establish the
  epoch or no-replacement guarantee.
- Exact selector and argv arrays, working directory, Cargo configuration,
  lockfile and manifest identity, profile, feature selection, target, and
  environment **including absent variables**. Any change to a
  behavior-affecting value must either select a different cold partition or
  make warm reuse refuse. Never silently inherit an ambient environment.
- A project behavior declaration that its build scripts, proc macros, native
  helpers, dependency code, and generated outputs express their inputs through
  Cargo's supported freshness contract. Relevant file reads, environment
  reads, feature and profile choices, tool inputs, and generated-file
  dependencies must be declared so Cargo invalidates affected units.

The final item is a bounded cooperative assumption, not an Oxide proof about an
arbitrary Cargo graph. Registry archive checksums do not establish macro,
build-script, native-helper, C/header, SDK, or generated-output behavior.
Dep-info, a local clean status, mtime checks, successful tests, a package
archive hash, an executable version, or consumer-supplied booleans cannot fill
those gaps. These unknown inputs include dependency and tool behavior, not
only edited local files.

Warm reuse therefore requires a qualified **complete supported invalidation
boundary for this workflow and its pinned installation epoch**. Qualification
must demonstrate the accepted project declarations and Cargo behavior for
source, configuration, feature, toolchain/install-epoch, and environment
changes. It must also exercise the project's relevant build-script,
proc-macro, native-helper, and generated-output classes under those
declarations. This is not a general threat-model claim or a new authenticity
receipt. A missing class or unqualified input makes the whole affected
compatibility partition cold-only; if Oxide cannot direct Cargo to an empty
generation already within the two-partition/two-generation allowance without
destroying retained or uncertain bytes, cold execution is refused until an
owner-authorized recovery path exists. Warm use is refused in either case. Do
not call cold-only storage a reuse benefit.

Cargo's ordinary freshness is separate from run validity. Every submitted
attempt invokes the exact Effigy selector that includes the required tests;
reuse never skips tests or changes their result. A compile failure before the
test phase is a failed invocation, not a successful run with passing tests. A
passing test is not evidence that unknown inputs were fresh. Exact
task/run/source provenance and the test result are retained independently
from cache eligibility.

## Minimal API and attempt lifecycle

The next Oxide implementation should add one inactive, qualification-only
entry point, provisionally `project::ordinary_cargo::run_attempt`, behind the
private qualification feature. It takes one existing `AcquireRequest` and one
strictly bounded `OrdinaryCargoInvocation` containing the consumer-bound root
and allocation epoch; project, task, workspace, branch, run and attempt IDs;
source commit/tree/content digest; accepted workflow/policy version and
installation epoch; selector and argv arrays; cwd; complete env map with
presence/absence; private target/temp/capture paths; and a finalize operation
ID. The embedder installs the verifier and callback dispatcher at construction;
task JSON and environment cannot install trusted policy or issuer material.
The entry point does not accept a `Registration` or fabricate `UnitEvidence`.

The host uses the existing identity, lease, lookup, and finalize semantics,
with the proposed state-aware acquire seam above (not generic
`Client::acquire`):

1. Check the supported invocation shape and exact source/root bindings before
   acquire or any writer starts. No generation-scoped barrier can be written
   yet because no generation is known.
2. Call `Client::acquire_ordinary_cargo` with the authenticated original
   request, `StrictPhysicalCargoRoot` identity, and validated partition. It
   applies the per-generation state policy above and never evicts. On an
   ambiguous transport outcome, use `Client::lookup_acquire` with the exact
   original authenticated request. `Absent` permits only exact replay.
   Busy/capacity returns pending without a host run. `Uncertain`, unknown
   accounting, lost token, changed body, or inconclusive lookup holds storage
   and the consumer root.
3. The acquire transaction has committed the pending barrier against the
   returned lease/generation before returning a leased result with path, token,
   storage instance, slot, generation, and reuse result. Verify that ledger
   record before forming the final launch digest from project, task,
   workspace, branch, run, attempt, root ID and allocation epoch; source
   commit/tree/content; policy and installation epoch; selector, argv, cwd,
   canonical source/target/temp paths, complete environment-presence map; and
   the acquired storage instance, slot, generation, lease, canonical slot
   path, leased state and daemon-returned `reused` value. Do not include the
   lease token; keep it in host memory and bind its digest only where the
   existing protocol requires it. If the barrier is absent or does not match
   the acquired tuple, do not submit. A `NotSpawned` settlement is permitted
   only if the authenticated acquire record still supplies the exact saved
   pre-acquire state and the consumer proves no writer began; otherwise hold
   the lease and root for recovery.
4. Send that immutable digest and command to exactly one
   `ordinary_cargo.submit` callback. The callback must echo the digest, persist
   the complete acquired tuple in the existing `StorageAttempt`, then call the
   existing Effigy 010 submission path and return its genuine scheduler run
   identity. No host token or Cargo process exists for a busy attempt, and the
   callback cannot change the command after acquisition.
5. Hold the lease over the complete Cargo process, all selected tests, and
   capture. The consumer returns its authenticated terminal and capture
   evidence, including the complete retained source/output/debug closure and
   full-debug lookup reference when required. Rust binds it to the acquired
   tuple and exact run/source attempt; Rust does not manufacture a successful
   test or capture record.
6. For a started run, only after cooperative no-writer completion and complete
   capture may the host call the proposed
   `Client::finalize_ordinary_cargo` with authenticated terminal/capture
   evidence. A proven no-submit result from `ordinary_cargo.not_submitted` uses
   the same operation with an authenticated `NotSpawned` settlement: it is
   accepted only when Effigy proves submission never began, and atomically
   restores the exact pre-acquire `empty` or `reusable` state saved in the
   pending marker before releasing the lease. No capture is fabricated and no
   bytes are cleared. On a lost finalize reply, call
   `Client::lookup_finalize` with the same lease, operation ID, and exact
   evidence before any identical retry. `Present` observes the stored
   transition, `Absent` permits exact replay, and `Uncertain` keeps the lease
   held. A lost submit reply, cancellation, or ambiguous writer state is not
   `NotSpawned` and leaves the marker pending until positive no-writer closure.
   Releasing the storage lease does not recycle the consumer's physical root.

The matching process schema should be one versioned
`ordinary-cargo-attempt-v1` operation on a private attempt channel, with three
new callback messages that have no `Registration` argument:

- `ordinary_cargo.submit(launch_digest, acquired_tuple, command)` returns the
  genuine Effigy scheduler run identity and the echoed launch digest;
- `ordinary_cargo.capture(run_identity, launch_digest, expected_outputs)`
  returns the authenticated terminal, completion and complete capture claims
  for that exact run and source; Rust verifies them against the acquired lease
  before building existing settlement evidence;
- `ordinary_cargo.not_submitted(launch_digest)` returns authenticated
  no-writer evidence only when the consumer proves the Effigy submission never
  began.

These messages are deliberately separate from `ExecutionOwner`'s
`execution.execute`, `execution.verify_success`, `execution.finalize_idle`,
and `execution.finalize_not_spawned` methods: those callbacks are invoked by
the registered Runner and their success-verification signature requires a
`Registration`. The ordinary-Cargo route has no `Registration` and must not
pretend it does. Nucleus must install this exact schema in
`oxide-runner-transport.ts` and its existing `StorageAttempt`; current fixture
hooks are not that installation. The Nucleus adapter must preserve original
request digests and reserved operation IDs before delivery, persist acquired
ownership before acknowledging it, and keep credentials/token material out of
ordinary logs and ledgers. Effigy 010 remains the only host submission and
settlement route.

The callback contract must distinguish a normally completed test failure from
lost or incomplete execution state. A completed failure may be settled only
with complete authenticated terminal/capture evidence; it makes the generation
cold-only. A compile failure before tests is a failed invocation, never a
successful validation. Cancellation, callback loss, output overflow, timeout,
or unknown writer state holds the lease unless the existing process owner
provides positive authenticated no-writer closure. PID disappearance, age,
reply loss, or a file marker is not closure. Do not change the running clock,
detach a writer, delete the generation, or silently retry with a new attempt.

The durable pending barrier cannot be a permanent boolean that blocks every
future cold attempt. Its states distinguish `pending`, `reusable` after
successful tests and complete capture, `cold-only` after an authenticated
settled failure or invalidation, and the saved pre-acquire `empty` or
`reusable` state restored by authenticated `NotSpawned` finalization. An
uncertain writer remains `pending` and unavailable. The no-spawn transition
releases the barrier without spending another generation, so the exact same
partition can admit its next cold attempt from `empty` or reuse attempt from
`reusable`. A cold-only generation is never served as warm. A separately
approved consumer recovery disposition is required before it can be retired
or reset; neither ordinary acquire nor this task's real proof may delete,
rewrite, or evict failed, retained, shared, or uncertain generations. Until
that disposition exists, the state-aware acquire returns capacity
pending/refusal when the two-generation owner cap provides no eligible
generation. This is an explicit capacity stop with no active lease barrier,
not silent pool growth or a claim that failed bytes became reusable.

The existing registered host has a 30-second callback timeout and a
five-minute session timeout; its 45-minute qualification ceiling is feature
scoped. Do not assume those bounds fit the real selector or widen them
globally. Before a real invocation, the consumer owner must show that the
existing Effigy submit path can return/retain the exact run identity, hold the
lease through process completion and capture, and settle within the one
remaining monotonic deadline. Keep individual callback replies within their
reviewed bound. If the real `validation-client` call blocks past the callback
limit, if the session cannot remain live through capture, or if cancellation
cannot positively fence and join the writer, the route is a finite NO-GO until
the lifecycle mapping is reviewed. It does not justify a second scheduler or
resetting a clock.

Before a later attempt changes the physical source root, the consumer first
archives that run's exact source and complete outputs. Capture must include
Cargo/test logs and results, source/config/tool identities, executables,
full-debug companions and UUID/build-ID, and source-path lookup against the
retained old source bytes. The consumer acknowledges that the old run can be
looked up independently of the current contents at the same physical path.
Failure to retain or retrieve any required output leaves root recycling
pending. This design itself never recycles the task root to another task.

## Next checked implementation and owner handoff

Define one Oxide-only implementation scope of at most 90 minutes authoring and
30 minutes independent exact-head review. It must add the state-aware
`acquire_ordinary_cargo` seam and compatible versioned per-generation reuse
metadata in the existing storage ledger; it must not route this policy through
generic `Client::acquire` or `make_room`. The scope includes the proposed
`StrictPhysicalCargoRoot` path identity, support for exactly the bounded
`ordinary-test` and `full-debug` partitions, atomic acquire-plus-pending-marker
ordering, authenticated finalize state transitions, and refusal on unknown
ledger versions. Keep all fixtures in fresh task-owned scratch. Synthetic
fixtures must cover busy/no-spawn, exact lookup/replay, source/config/feature/
tool/epoch/env invalidation, failed/partial builds, and capture-before-finalize;
prove that a reusable idle generation is selected only when its reuse state
permits it, `cold-only` is never returned as warm, and an idle generation is
never evicted to admit another partition or generation. Exercise the one and
two partition boundary, capacity refusal, pending/uncertain holds, and
authenticated no-spawn finalize restoring a prior `empty` state followed by
cold admission in the same partition without allocating another generation.
Also prove `reusable` restoration, exact finalize lookup/replay, and that a
settled failure stays cold-only and capacity-refuses when no owner-approved
recovery exists. Unknown input must refuse warm use. This does not run or
qualify a real Nucleus Cargo workflow, install an authority, or activate a
consumer.

After that API receives independent exact-head review, the handover packet to
Nucleus must name the Oxide commit SHA, reviewed method/request/response
signatures, callback schema digest, storage-ledger version and reuse-state
definitions, the owner's R×P=2 / 8-GiB-per-generation / 16-GiB-total limits,
no-successor/no-eviction/capacity-refusal semantics, redaction rules, and
synthetic evidence. Nucleus owners
must accept that exact packet before a separately scoped Nucleus-owned client
task is dispatched; consumer work cannot start before this handover.

That Nucleus task is bounded to at most 90 minutes authoring and 30 minutes
independent exact-head review, one `ordinary-cargo-attempt-v1` adapter, one
fresh consumer root, and the four-attempt/45-minute ceiling in the proof
packet below. It may install the accepted callback schema only in the existing
`StorageAttempt`, `oxide-runner-transport.ts`, and `validation-client.ts`
path, route submission and settlement through Contract 010 and the existing
Effigy scheduler, and use the current `oxide-retention.ts` owner for source,
output, and debug capture. It must demonstrate bounded synthetic cases for
busy-before-host-run, persistence before submit, cancellation/unknown writer
hold, capture-before-finalize, source/debug lookup, and credential redaction.
The client cannot implement the Oxide codec or settlement rules, add a second
scheduler, exceed the proposed root/partition/generation and evidence ceilings
below, or activate a real workflow in this task. Nucleus, Queue/Effigy, and
retention owners must approve its fields, root policy, lifecycle, and evidence
handling in that task's brief. This design does not contact, prompt, or mutate
those owners. The existing registered Runner remains unchanged.

## Finite later proof packet

Real qualification is a separate gate and must not start from this design.
Before a new proof brief is dispatched, Oxide and Nucleus owners must jointly
check one packet containing the exact clean source commits and Git trees for
Oxide, Nucleus, Queue/Effigy integration, and the one source workspace; lockfile
digest; Cargo, rustc, sysroot, host/target and relevant native/SDK executable
identities; accepted no-replacement installation epoch; Effigy selector and
forwarded argv arrays; Cargo profile, features, configuration and complete
environment; and the distinct per-invocation task/run/attempt identities.
The Nucleus/Queue pins recorded by prior read-only inspection are only
candidates: Nucleus `c0c9617533a675bad035b50aefe2fc6b6043f323` with tree
`1940a897676a82cd0dd5a7557d916391ada07eba`, and Queue
`7b9b840354a638a529ab023542903949b5b562ac` with tree
`244eec3284d272e6db8b056264b31cd25b459f77`. Re-pin clean final heads and
obtain owner approval in the checked packet; those inspection snapshots are
not execution authority. Oxide's exact implementation and API head must also
be reviewed and pinned there. Name the canonical clean source-root path, root
ID and allocation epoch; name the unique fresh APFS image path, volume/image
identity, ownership, available capacity, empty-state proof and cleanup owner.
The paths must be under that session's approved unique resource root; do not
reuse another task's worktree, image, or temporary directory.

The candidate workflow and toolchain from prior read-only inspection are
Cargo `1.97.1 (c980f4866 2026-06-30)` / rustc
`1.97.1 (8bab26f4f 2026-07-14)`, host = target
`aarch64-apple-darwin`, no target override, wrapper, remapping, or alternate
backend. The proof selector adds Cargo's structured output mode to the
existing test invocation so reuse can be observed without resolving a graph.
The candidate Effigy argv is:

```text
["effigy", "check:core:test", "--", "--message-format=json-render-diagnostics", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

At the inspected source, Effigy forwards this as
`cargo test --offline --message-format=json-render-diagnostics -p nucleus-core
--test planning --test build_diagnostics`; the ordinary profile inherits
dev's line-tables debug setting. The full-debug control appends
`--profile full-debug` after `build_diagnostics`, creating a separate
compatibility partition. This proof-only output argument must be accepted by
the Effigy and Nucleus owners and pinned with the selector; if it cannot be
passed through the reviewed 010 path, no warm-reuse proof runs.

The bounded input-class scope is only this one locked `nucleus-core` workflow
and its actual target/feature closure. Before launch, the Nucleus package and
dependency owners must provide a finite table of every package, version,
feature set, build script, proc macro, native helper, generated input/output,
compiler/linker/SDK, and environment dependency that can affect this
selector. Prior read-only inspection already identified candidate classes
that the table must address: bundled native SQLite via
`libsqlite3-sys 0.38.2` and `cc 1.4.7`; derive/proc-macro execution including
`serde_derive 1.0.229`, `schemars_derive 0.8.22`, `proc-macro2 1.0.107`,
`quote 1.0.47`, and locked `syn` versions; and any build scripts or generated
files in the local `nucleus-core`/`nucleus-observer` packages. These are
known candidates, not a complete active graph. Owners must classify the
actual pinned workflow, state the Cargo freshness declaration for each
behavioral input, and identify unsupported classes. A class without that
owner-reviewed contract makes the partition cold-only; Cargo's artifact
messages cannot qualify the input behavior themselves. As of this design,
these Nucleus classes have no accepted owner qualification, so a real Nucleus
warm-reuse proof is currently NO-GO. The bounded route may proceed to synthetic
implementation; it may not enter the conditional warm sequence below until
the class table is complete and accepted.

For output observation, the consumer captures the bounded JSON event stream
from each genuine Effigy run and parses only Cargo `compiler-artifact`
records (`package_id`, target, profile and `fresh`) plus the matching command
and run identity. This is observation of Cargo's reuse decision, not a graph
resolver, input receipt, or acceptance proof. The preapproved class table
names the expected package/target records. A cold-A event outside that table
stops the sequence before any warm attempt. For B, the changed local target
must report `fresh: false` and a predeclared unaffected target must report
`fresh: true`; passing tests alone proves correctness, not reuse. The full-
debug partition miss must report `fresh: false` for the comparable targets.
Preserve the raw bounded event stream with the exact run and source record.

These candidate pins, class table, output mode, and selectors must be
reverified against the exact clean source in the joint packet. No
`cargo metadata`, Cargo version command, graph probe, or unaccounted Cargo
invocation is allowed in that proof.

Owner commitments in that packet must include: Nucleus managed-Git/root
allocation and source/archive/reconstruction; StorageAttempt and task
admission; Queue host authority under frozen Contract 010; the existing Effigy
submit, cancellation, settlement and process-completion path; capture and
retention owners; the accepted cooperative Cargo/project behavior declaration
and installation no-replacement owner; the Nucleus package/dependency owners
for the complete class table above; and the Oxide storage owner. They must
confirm one physical root, sequential leases, no cross-root reuse, busy before
host submission, full capture before finalize, and old full-debug source
lookup before changing the physical root's source bytes. They must also
confirm the real selector fits the existing callback/session lifecycle and
one absolute 45-minute end-to-end proof deadline; the registered host's
feature-scoped qualification timeout is not an automatic grant for this route.

Proposed finite ceilings for that packet, not measured fit or an assumption
about current Nucleus size:

- one consumer-owned physical source root (R=1), at most two compatibility
  partitions (P=2: ordinary test and full-debug), and at most two generations
  total (R×P=2), one per partition, at 8 GiB each and 16 GiB maximum
  intermediate storage. This follows the Nucleus owner recommendation and the
  lane's one store per compatible partition; there is no third successor or
  implicit growth. If a settled failure/invalidation leaves a partition
  cold-only, stop that partition at capacity until a separately approved
  consumer recovery disposition exists. A proven no-spawn attempt restores
  its prior state and consumes no additional capacity. No eviction or recovery
  deletion occurs in this proof;
- one fresh task-owned disposable APFS image capped at 24 GiB total allocated
  footprint, including source, both private target/temp locations, all
  intermediates, live output/capture, and logs; all such writes stay inside
  that image;
- independently retained source and output/debug evidence capped at 8 GiB
  total, with at most 2 GiB per each of the four attempt records. Keep this
  outside the mutable root and image. No data is trimmed to make a later run
  fit;
- 32 GiB maximum combined footprint: the 24-GiB image cap plus the separate
  8-GiB retained-evidence cap above. The 16-GiB Oxide intermediate limit is
  inside the image and does not increase the owner's two-generation pool.
  Preserve the brief's 20-GiB host free-space reserve continuously, as a
  start/abort threshold, not a guarantee against other host writers;
- no fallback root or alternate cache (0 bytes), and recovery/deletion of an
  existing generation disabled for this proof. No unallocated successor is
  assumed or used;
- at most four Cargo invocations and 45 minutes total from preparation through
  source/debug lookup. Every invocation uses the exact selected test workflow
  and JSON output mode; there are no standalone Cargo metadata/version/probe
  calls, graph or nightly gates, or extra retry. Stop before a new invocation
  at either cap.

The current candidate Nucleus classes are not yet owner-qualified; therefore
the warm-reuse sequence below is a conditional plan, not a launchable proof.
If the class table is incomplete or lacks owner commitments, stop before
Cargo. A separately approved cold-only storage proof may show bounded storage
but cannot establish reuse or write reduction.

After the class table and output mode are accepted, candidate sequence:
(1) cold source A under the ordinary test profile; capture the event stream
and require every observed target to match the owner-approved class table
before proceeding. (2) Changed source B under the same profile; tests must
run, the changed local target must be `fresh: false`, and at least one
predeclared unaffected dependency target from the qualified class table must
be `fresh: true`. (3) Source B under full-debug as the whole-partition miss;
tests run and comparable targets must be `fresh: false`, with complete
full-debug capture. (4) Changed source C under that full-debug partition;
tests run, Cargo event records show changed-source invalidation plus
predeclared unaffected reuse, then a lookup proves source B's old full-debug
artifacts and paths still resolve to B's retained bytes. Each invocation gets
a new authenticated run/attempt and one genuine Effigy submission. Cargo
event evidence distinguishes reuse or a cold partition miss; exit status and
passing tests alone do not. The exact A/B/C source pins and bounded changes
must be named in the checked packet. If Cargo fails before executing tests,
any test fails, an observed class is outside the approved table, required
freshness events are missing, evidence is incomplete, or reuse/miss evidence
is ambiguous, stop the proof; preserve the image and records, mark the
generation cold-only or held as uncertainty requires, and make no benefit
claim.

Do not start if any owner has not committed to its boundary, if the complete
source/tool/config/input boundary cannot be qualified, if exact pins or clean
root/image resources are absent, or if old full-debug lookup cannot fit inside
the retention cap. At the ceilings, stop and preserve evidence. Do not delete
shared or uncertain generations, expand the deadline, reset an ended clock,
or silently increase a cap. Reaching the bounds without warm reuse can still
establish bounded cold storage, but it establishes no repeated-work, latency,
or SSD-write benefit. The existing pilot clock is unstarted and unchanged.
