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
bounded successor without destroying retained or uncertain bytes, warm use is
refused until an owner-authorized recovery path exists. Do not call cold-only
storage a reuse benefit.

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

The host uses the existing client and lease semantics:

1. Check the supported invocation shape and exact source/root bindings before
   any writer starts. Persist a pending-use barrier for the acquired
   generation before submission.
2. Call `Client::acquire`. On ambiguous transport outcome, use
   `Client::lookup_acquire` with the exact original authenticated request.
   `Absent` permits only exact replay. Busy/capacity returns pending without a
   host run. `Uncertain`, unknown accounting, lost token, changed body, or
   inconclusive lookup holds storage and the consumer root.
3. After a leased result with path, token, storage instance, slot and
   generation, issue exactly one consumer `execution.execute` callback. No
   host token or Cargo process exists for a busy attempt. Before callback,
   bind the final acquired storage tuple, source/root/run identities, policy,
   exact argv, cwd and full env-presence map into one launch digest; the
   callback echoes that binding and cannot alter the command. It must record
   the acquired tuple in the current `StorageAttempt`, then use the existing
   Effigy 010 submission path and return the genuine scheduler run identity.
4. Hold the lease over the complete Cargo process, all selected tests, and
   capture. The consumer returns its authenticated terminal and capture
   evidence, including the complete retained source/output/debug closure and
   full-debug lookup reference when required. Rust binds it to the acquired
   tuple and exact run/source attempt; Rust does not manufacture a successful
   test or capture record.
5. Only after cooperative no-writer completion and complete capture may the
   host call `Client::finalize` with the existing authenticated settlement
   evidence. On a lost finalize reply, call `Client::lookup_finalize` with the
   same lease, operation ID, and evidence before any identical retry.
   `Present` observes settlement, `Absent` permits exact replay, and
   `Uncertain` keeps the lease held. Releasing the storage lease does not
   recycle the consumer's physical root.

The matching process schema should be one versioned
`ordinary-cargo-attempt-v1` operation on a private attempt channel. It reuses
the existing consumer callback meanings for `execution.execute`,
`execution.verify_success`, `execution.finalize_idle`, and
`execution.finalize_not_spawned`, with an ordinary-Cargo launch payload and
the same installed verifier boundary. It does not call the registered Runner
or its registration/input callbacks. Nucleus must install this exact operation
in `oxide-runner-transport.ts` and `StorageAttempt`; current fixture hooks are
not that installation. The Nucleus adapter must preserve original request
digests and reserved operation IDs before delivery, persist acquired ownership
before acknowledging it, and keep credentials/token material out of ordinary
logs and ledgers. Effigy 010 remains the only host submission and settlement
route.

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
future cold attempt. Its states must distinguish `pending`, `reusable` after
successful tests and complete capture, and `cold-only` after an authenticated
settled failure or invalidation. An uncertain writer remains `pending` and
unavailable. A cold-only generation is never served as warm: a separately
authorized attempt may use an already-budgeted empty successor generation
under the same compatibility partition, preserving the old generation. If no
empty bounded successor exists, acquire stays pending/refuses until the
consumer grants a specific recovery disposition. Ordinary acquire does not
delete or rewrite shared, failed, retained, or uncertain generations. This
terminal cold-only state releases the logical barrier without making stale
bytes eligible, so it does not deadlock admission merely because a compile or
test failed.

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
30 minutes independent exact-head review. It may add the one entry point,
private schema, barrier transitions, and bounded synthetic-owned fixtures for
busy/no-spawn, exact lookup/replay, source/config/feature/tool/epoch/env
invalidation, failed/partial builds, cold-only admission, and capture-before-
finalize. Keep all fixtures in fresh task-owned scratch. It must prove that
unknown input refuses warm use and that a bounded empty successor permits
cold admission without deleting the previous generation. It does not run or
qualify a real Nucleus Cargo workflow, install an authority, or activate a
consumer.

After that API receives independent exact-head review, hand the exact API and
schema to a separately scoped Nucleus-owned client task. That client scope
uses the existing `StorageAttempt`, `oxide-runner-transport.ts`,
`validation-client.ts`, Contract 010 scheduler, and current
`oxide-retention.ts` owner. It binds source/root allocation, exact attempt and
acquired storage tuple, uses the genuine Effigy submission and settlement,
archives old source/debug/output before the next source change, and refuses on
missing capture or authority. Queue/Nucleus owners must agree to its fields,
root policy, lifecycle, and retention before consumer dispatch. This design
does not contact, prompt, or mutate those owners. The existing registered
Runner remains unchanged.

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
backend. The existing Effigy selector is the exact argv array:

```text
["effigy", "check:core:test", "--", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

At the inspected source, Effigy forwards this as
`cargo test --offline -p nucleus-core --test planning --test
build_diagnostics`; the ordinary profile inherits dev's line-tables debug
setting. The full-debug control appends `--profile full-debug` after
`build_diagnostics`, creating a separate compatibility partition. These
candidate pins and selector must be reverified against the exact clean source
in the joint packet. No `cargo metadata`, Cargo version command, graph probe,
or unaccounted Cargo invocation is allowed in that proof.

Owner commitments in that packet must include: Nucleus managed-Git/root
allocation and source/archive/reconstruction; StorageAttempt and task
admission; Queue host authority under frozen Contract 010; the existing Effigy
submit, cancellation, settlement and process-completion path; capture and
retention owners; the accepted cooperative Cargo/project behavior declaration
and installation no-replacement owner; and the Oxide storage owner. They must
confirm one physical root, sequential leases, no cross-root reuse, busy before
host submission, full capture before finalize, and old full-debug source
lookup before changing the physical root's source bytes. They must also
confirm the real selector fits the existing callback/session lifecycle and
one absolute 45-minute end-to-end proof deadline; the registered host's
feature-scoped qualification timeout is not an automatic grant for this route.

Proposed finite ceilings for that packet, not measured fit or an assumption
about current Nucleus size:

- one consumer-owned physical source root, at most two compatibility
  partitions (ordinary test and full-debug), and at most three 8-GiB Oxide
  generations total: one per partition plus one empty cold successor reserve;
  24 GiB maximum intermediate storage, with no eviction or recovery deletion;
- one fresh task-owned disposable APFS image capped at 32 GiB total allocated
  footprint, including source, both private target/temp locations, all
  intermediates, live output/capture, and logs; all such writes stay inside
  that image;
- independently retained source and output/debug evidence capped at 8 GiB
  total, with at most 2 GiB per each of the four attempt records. Keep this
  outside the mutable root and image. No data is trimmed to make a later run
  fit;
- 40 GiB maximum combined image plus retained evidence. Preserve the brief's
  20-GiB host free-space reserve continuously, as a start/abort threshold, not
  a guarantee against other host writers;
- no fallback root or alternate cache (0 bytes), and recovery/deletion of an
  existing generation disabled. The one empty successor is only a clean new
  slot and never authorizes overwriting an old one;
- at most four Cargo invocations and 45 minutes total from preparation through
  source/debug lookup. Every invocation uses the exact selected test workflow;
  there are no standalone Cargo metadata/version/probe calls, graph or nightly
  gates, or extra retry. Stop before a new invocation at either cap.

Candidate four-invocation sequence: (1) cold source A under the ordinary test
profile; (2) changed source B under the same profile, with tests and Cargo
freshness output demonstrating changed source invalidation and observed reuse
of unaffected dependencies; (3) source B under full-debug as the whole
partition miss, with tests and full-debug capture; (4) changed source C under
that full-debug partition, with tests and a lookup proving source B's old
full-debug artifacts and source paths still resolve to B's retained bytes.
Each invocation gets a new authenticated run/attempt and one genuine Effigy
submission. Cargo event evidence distinguishes reuse or a cold partition
miss; exit status and passing tests alone do not. The exact A/B/C source pins
and any bounded source changes must be named in the checked packet. If Cargo
fails before executing tests, any test fails, evidence is incomplete, or
reuse/miss evidence is ambiguous, stop the proof; preserve the image and
records, mark the generation cold-only or held as uncertainty requires, and
make no benefit claim.

Do not start if any owner has not committed to its boundary, if the complete
source/tool/config/input boundary cannot be qualified, if exact pins or clean
root/image resources are absent, or if old full-debug lookup cannot fit inside
the retention cap. At the ceilings, stop and preserve evidence. Do not delete
shared or uncertain generations, expand the deadline, reset an ended clock,
or silently increase a cap. Reaching the bounds without warm reuse can still
establish bounded cold storage, but it establishes no repeated-work, latency,
or SSD-write benefit. The existing pilot clock is unstarted and unchanged.
