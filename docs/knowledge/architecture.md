# Build storage boundaries

These are design-only constraints agreed by Tom on 2026-10-09, not an implemented API. The design below recommends a per-user local daemon as the storage hub; it does not select an implementation, establish portability, or authorize a pilot.

## Storage and ownership

Persistent bounded cache slots partition by project/component identity, exact toolchain, host/target platform, and resolved configuration/profile/features/relevant environment. One exclusive durable lease covers compile through test and artifact capture. Unknown crash outcomes, lost replies, age and PID disappearance do not imply release. Reconciliation must prove safe ownership settlement before reuse or eviction.

Exact-run final binaries, logs, QA receipts and evidence remain independently owned and preserved. Cargo intermediates are internal and may contain test executables; separating output directories alone cannot prove isolation. Cache reuse never counts as validation success.

Prepare only requested components lazily. No eager copying of all targets and no full-copy fallback. Evict idle slots only, within explicit budgets; occupied space and actual disk writes must be measured separately. Inventory/acquire/reconcile/finalize/budget describe interface semantics, not an existing CLI.

## Qualification boundaries

Cargo's `build.build-dir` / `CARGO_BUILD_BUILD_DIR` is stable starting in Rust 1.91 and is separate from `target-dir`. It is the smallest useful mechanism to qualify: direct Cargo to an Oxide-owned intermediate directory and let Cargo retain its own unit invalidation. It supplies no Oxide ownership, whole-workflow lease, budget, or run-evidence semantics. The exact pinned toolchain and different-path portability still need a controlled qualification. If portability fails, use path-isolated partitions and report the reduced benefit. Do not blindly adopt sccache: Acow's measured trial rejected it.

Nucleus Contract002 prohibits mutable target sharing. An exclusive-intermediate design requires narrow qualification before adoption and cannot weaken that contract. Frozen Contract010 remains unchanged. Queue/Nucleus own admission, source and run identities, authenticated settlement, retention and artifact capture; Oxide owns only its bounded storage semantics. See [scope](vision.md).

## Preferred daemon hub to qualify

Tom's direction, 2026-10-09: a central daemon could normalize project workflows, own reusable storage and provide a consistent place to measure real-world use. It is the preferred candidate for the bounded design task, not an implemented system or a final implementation choice.

The design should define a minimal opt-in project/consumer adapter: resolve build identity, obtain scheduler admission where required, acquire a slot, execute the declared workflow, preserve/capture run outputs under consumer policy and finalize ownership with authenticated evidence. Every actor touching an adopted slot must follow the lease contract; direct Cargo access cannot silently bypass it. Commands outside the adapter have no claim to Oxide-managed coverage.

The daemon owns the storage ledger and records observations keyed by project/component, source/run identity, resolved toolchain/configuration and backend. Observation is not authenticated run settlement or a validation verdict. Queue/Nucleus remain authoritative for admission, managed source/run identities, settlement and evidence/retention policy. Daemon failure/restart must reconcile durable leases conservatively; loss of connection cannot grant reuse. Consumer cancellation and process lifetime must have explicit owners.

Start with bounded local events for acquisition/refusal, timing, rebuild/reuse observations, storage growth and finalization. Account for measurement overhead and unavailable write attribution. General tracing, dashboards and experiment scheduling are not requirements for first relief. Ordinary-use telemetry can reveal contention and trends after adoption; controlled qualification is still needed for cache correctness and failure cases. Future experiments are explicit opt-ins, use isolated declared resources and do not silently alter everyday flags or source.

Keep storage/execution adapters replaceable and consumer-facing semantics stable as Cargo catches up. The design task must propose concrete daemon availability, project adoption and recovery behavior within its two-hour investigation bound; it does not install, execute or wire this design.

## Minimum safe cross-worktree reuse design

### Recommendation and alternatives

Prefer one opt-in, per-user local daemon as the storage hub for Queue, Nucleus and later developer workflows. It owns the slot directory, durable lease ledger, budget accounting and bounded observations. Each adapter supplies the effective build identity and the consumer-authenticated run envelope. The daemon has no selector scheduler, task admission, Git source management, run settlement, validation verdict, artifact-retention or capture-policy authority.

The minimal alternative is a per-invocation wrapper using a shared OS lock and durable journal. It can serialize one command and avoid a background service, but every wrapper must implement crash reconciliation and communicate through the same journal. A dead wrapper can release an OS lock while a compiler or test descendant is still alive; a journal alone cannot prove the slot is idle. Making this alternative safe therefore needs a durable process guardian and the same ledger/reconcile protocol. It remains viable if daemon lifecycle cost outweighs the value of a common hub, but it is not simpler for multiple consumers or normalized observations. Neither model is proven until the pilot's failure cases pass.

The daemon is a single local-user service, not a remote cache or a scheduler. Its data root is private to that user and on a filesystem whose locking, atomic replacement and durability behavior is qualified. A daemon-wide OS singleton lock prevents two service instances from owning the ledger. The slot ledger is durable and transitions are committed before replies. That singleton lock is not a slot lease: an in-flight slot stays unavailable after a daemon restart until reconciliation proves it safe. If the selected host cannot keep or establish proof of process containment after daemon failure, ambiguous slots remain quarantined and cannot be reused or evicted.

### Adoption and execution boundary

Adoption is explicit per project and per declared workflow/selector. The adapter resolves the project and component IDs, exact source identity, exact Cargo/rustc toolchain, host and target, profile, features, effective Cargo configuration and relevant environment before requesting storage. Secret values are never recorded; only an allowlisted effective input set or its digest is used. If a relevant input cannot be resolved, the adapter uses a private isolated build directory or refuses managed reuse.

Queue/Nucleus admission happens before `acquire`. An admitted consumer passes an authenticated envelope containing its project, workflow, task/workspace/run/attempt identity, source tree and admission reference. For developer workflows without a scheduler, the adapter creates a local run identity but does not claim Queue/Nucleus authority. The daemon validates the envelope or a consumer-issued capability, binds it to the lease and returns an opaque lease token. It never creates or changes the consumer's identity.

Under a lease, the adapter sets `CARGO_BUILD_BUILD_DIR` to the selected Oxide intermediate slot. It keeps `CARGO_TARGET_DIR` and captured outputs private to the checkout/run. The lease stays held from compile start through test completion and independent artifact/log/receipt capture. Direct Cargo access to the shared build directory is a bypass and is unsupported. An invocation that cannot reach the daemon may run only with a consumer-approved private isolated directory; otherwise it refuses. Private fallback and unmanaged commands make no Oxide reuse or observation claim.

Effigy continues to launch the declared language-neutral selector and own selector/process/env lifecycle. The project adapter carries the Rust-specific identity and lease context. Queue/Nucleus (or the local developer) owns the cancellation decision; the adapter passes it to the process guardian, which must establish that every task descendant has stopped before the lease can be released. The daemon's service lifetime does not define the workflow's lifetime.

The first-use path should be one explicit project/workflow opt-in, with the adapter resolving Cargo settings and toolchain identity instead of asking users to edit global Cargo configuration. Explain a valid miss by category (source, configuration, toolchain, path partition or unknown) and show the effective non-secret identity. Under pressure, explain measured/reserved bytes, the cap/reserve, the active or uncertain owner and the safe next action; never suggest deleting evidence or forcibly clearing an uncertain slot. Reconnect should retry the same operation identity and reconcile when proof is available. If proof is unavailable, report that the slot is being held and why, then fail closed. These are ease-of-use acceptance criteria, not new user-facing command names.

### Identity and slot key

Use two identities for different purposes:

- **Compatibility partition key:** stable project ID and component/workflow ID; exact Cargo and rustc provenance; host and target triples; resolved profile, feature set and Cargo/rustc configuration; and relevant compiler, linker, SDK and environment inputs. Include an input in full or by a stable digest. Include checkout-path identity until different-path portability has passed. A failed path test requires path-isolated partitions.
- **Run binding:** consumer, task/workspace/run/attempt IDs when present, exact Git commit and source-tree state (including submodules and any permitted disposable source patch), resolved partition-key digest, and operation ID. A source revision is recorded per lease rather than automatically creating an unbounded partition; Cargo's own fingerprints must show that source changes invalidate affected units.

An unknown, changing or path-sensitive input is not silently omitted to improve hit rate. It forces a separate partition or a refusal. A change to toolchain or effective build configuration cannot use the old partition. The partition key is not a substitute for Cargo's dependency/build-unit invalidation.

### Semantic operations

These operation names describe a future protocol, not an existing command or API.

| Operation | Required behavior and retry identity | Refusal or result |
| --- | --- | --- |
| `inventory` | Read-only snapshot of configured cap/reserve, measured bytes, slot key, and `idle`, `leased`, `uncertain` or `evictable` state. A request ID correlates the read; safe retries may return a newer snapshot. | Report unknown size/state as unknown; never infer idle from missing metadata. |
| `acquire` | Require admission/authenticated run envelope first. Persist the lease and owner binding before returning its opaque token. Deduplicate on `(consumer, run_id, attempt_id, acquire_id)`; identical retry returns the same result, changed payload with the same ID is rejected. | Refuse a different claimant while leased/uncertain, an incompatible key, unresolved input, insufficient bounded capacity or failed admission. No automatic wait/retry that expands the operation. |
| `reconcile` | Reconcile one durable lease using a stable `(lease_id, reconcile_id)` and authenticated consumer evidence. Confirm process containment and capture state, not just a reported PID or terminal task state. Repeating identical evidence returns the same result. | If ownership, descendants, capture or reply outcome is uncertain, keep the lease `uncertain`, reserve its bytes, and refuse reuse and eviction. Conflicting evidence is rejected. |
| `finalize` | Accept an authenticated terminal run record plus capture receipt/digests after the process guardian proves all task descendants have ended. Deduplicate on `(lease_id, finalize_id)`; a repeated identical finalize returns the prior receipt. | Do not release on age, vanished PID, daemon crash, lost reply, cancellation request, or test outcome alone. Missing proof leaves the slot uncertain. A failed test may release only after safe process end and evidence capture; it never becomes a validation success. |
| `budget` | Read configured cap/reserve and current measured/reserved bytes using a caller request ID; safe retries may return a newer snapshot. Limit policy is supplied by explicit owner-approved configuration, not changed by a consumer. If a future authorized policy update exists, use stable `(policy_revision, update_id)` and reject changed payload under the same ID. | Refuse acquisition when a cap/reserve cannot be enforced or would be exceeded. Evict only proven-idle slots and never captured run evidence. |

Each attempt has a new attempt ID; retries of the same logical operation reuse its operation ID and byte-identical payload. A new attempt cannot inherit an earlier lease. An identical acquire retry returns the original lease identity and current durable state; it never revives a finalized lease. The consumer adapter must single-flight execution for one attempt so a retry cannot launch a second workflow. A lease token is scoped to one run binding and one partition. Successful finalization is the only ordinary transition from `leased` to `idle`; restart recovery enters `uncertain` until reconciliation succeeds.

### Durable ownership and recovery

The ledger records the slot key, lease token hash, owner/run binding, acquire/finalize operation IDs, state, reserved/measured bytes and capture receipt reference. Durable transitions must be flushed before acknowledging them. The daemon serializes ledger mutation and atomically publishes completed metadata. A durable record is necessary but not sufficient: the selected process-control mechanism must ensure the slot remains unavailable while any launched task descendant may still write to it, even if the daemon or client disconnects.

On daemon restart, every persisted `leased` record without a matching durable completion receipt becomes `uncertain`. The daemon queries the owning consumer for authenticated task/source/terminal/capture evidence and checks the guardian's process-containment record. PID plus start-time or a lock-file state may help locate a job, but absence alone proves nothing. If either authority is unavailable or a descendant may still exist, preserve the slot and refuse acquisition/eviction. Recovery may be retried with the same reconciliation ID; lost replies do not create new leases. Operator cleanup is outside this first protocol and must not be inferred from age.

### Budget and observations

Capacity covers the dedicated disposable footprint, including checkout roots, intermediates and pilot run captures. The proposed pilot ceiling is 10 GiB total and a 20 GiB host free-space reserve. Before execution, read-only preflight must establish the measurement method and demonstrate an enforceable cap/reserve for the selected host and filesystem. A preflight measurement without an enforcement mechanism is insufficient; if either bound is unknown or unenforceable, do not start a build. Capacity pressure produces an actionable refusal and inventory. Do not delete an active or uncertain slot or consumer-retained evidence to make room.

Emit only bounded lifecycle observations: one normalized run record and at most 12 fixed-schema events per run, each capped at 1 KiB. Events cover request/result, compile completion, test completion, capture completion, finalization and any uncertainty/reconciliation transitions. Repeated identical RPCs update a retry counter rather than append an event. Record durations, reuse/rebuild summaries, occupied/reserved bytes and available write counters; omit raw environment values, secrets, compiler command lines and per-file traces. Cap the observation ring at 16 MiB and drop oldest settled observations first. Lease ledger and consumer run evidence are outside this ring and cannot be evicted by it. The daemon is not a general tracing service.

Observations correlate through project/component ID, consumer run/attempt ID, source-tree digest, partition digest and event sequence. They explain storage behavior but are not authenticated settlement, a test verdict or a replacement for controlled correctness/failure qualification. If actual write attribution is unavailable, record it as unavailable; do not treat occupied-space deltas as disk-write counts or infer SSD lifetime.

### Exact-run evidence and Cargo boundary

Queue/Nucleus continue to preserve run outputs under their own policy. Each run's binaries, logs, test output, receipts and evidence live in its independently owned capture area, not in the reusable slot. The consumer receipt binds their digests to its authenticated run ID, exact source/tree, selector, toolchain/configuration and test result. The daemon may retain a receipt reference and digest but cannot mint the consumer's authenticated receipt. Cargo's intermediate directory is disposable internal state and may contain test-related executables; none of its contents alone establishes that a run's tests executed or passed.

Run `cargo test` and the selected Effigy tests on every build scenario. A cache hit skips compilation only; it cannot skip tests, create a successful receipt or satisfy validation. Target-directory separation alone does not reuse compiler intermediates across worktrees. Sharing `CARGO_TARGET_DIR` would also mix final outputs and cleanup scope, violating the mutable-target boundary. The replaceable initial mechanism is stable Cargo build-dir redirection plus Oxide's exclusive lease, project adapter, ledger and budget—not a second Cargo fingerprint engine, eager directory copier, full-copy fallback or `sccache` assumption.

Retire or replace the redirection mechanism only when an actually supported Cargo release supplies the needed cross-workspace unit reuse and lifecycle behavior for the selected workflows, including build scripts/proc macros as required, and controlled tests show the same ownership, path correctness, capacity and evidence guarantees. Keep the adapter, authority boundaries, run binding, budgets and consumer API semantics independent of that mechanism. Reassess at each supported Cargo upgrade; upstream roadmap acceptance alone is not a replacement trigger.

### Proposed bounded qualification

The sole proposed first pilot is one native Nucleus core-test Effigy selector, one exact pinned head in two disposable checkout paths, and one exclusive intermediate slot. The selector ID, exact commit/tree, installed Cargo/rustc toolchain, host filesystem and measurement/enforcement mechanism remain preconditions to a separate execution approval; no selector, revision or MSRV is inferred here. Both paths start clean at the same commit with matching lockfile/submodules. The task owner must approve any temporary source patch used for invalidation control.

Before execution, require separate scoped approval; exact selector/source/toolchain/paths and storage/write/time limits; validated 10 GiB disposable cap and 20 GiB free reserve; a resolved RustQuality profile/deviations before any Rust authoring; Nucleus owner coordination that avoids duplicating its compiler-wrapper lead; and independent exact-head review. Consumer adoption is another decision after the evidence review.

Worker scenarios are capped at six, with 90 minutes for the worker and 30 minutes for independent reviewer replay, two hours total. No automatic retries or expansion. Hard stops are the worker/reviewer time limits, the 10 GiB total disposable cap and the 20 GiB host free-space reserve; any separate cumulative-write limit must be named in the execution approval. Suggested scenarios:

1. isolated cold reference in a private build directory;
2. shared-slot cold run at checkout A;
3. same-head warm run at checkout B;
4. controlled tracked-source change in one disposable checkout, with a recorded new tree digest and evidence that affected units rebuild;
5. effective configuration change, which must change the partition or be refused while incompatible state is present;
6. toolchain control using an already-installed alternative, or a key-separation/refusal proof without another compiler build.

Every scenario that actually builds runs the tests in that same lease. A configuration/toolchain scenario that only proves key separation/refusal does not claim a build result. Do not install a toolchain to fill the matrix. Measure elapsed time, independently sampled actual write bytes, and occupied/reserved space separately. Count the checkouts and captures within the cap. State unavailable write attribution plainly; never infer SSD lifetime. At each predeclared time, space or write threshold, stop and report rather than retry or expand.

Correctness/failure proofs within the same bounded session: generated-code and proc-macro behavior; path-sensitive build output at both roots; exact-head/source receipt and artifact binding; tests run on every real build; a simultaneous second claimant is refused without touching the slot; idempotent acquire/finalize retries after deliberately lost replies; daemon crash during an in-flight build with restart still refusing while any descendant/capture outcome is uncertain; and budget pressure evicts only a reconciled idle slot while active/uncertain state remains unavailable. Age, vanished PID or restart alone must not make a slot available. Independent reviewer replays at most one warm and one meaningful miss, within the 30-minute reviewer cap.

Disposition is one of: proceed to an adoption decision if all ownership/correctness gates pass and measured benefit is useful; path-isolated partitions with explicitly reduced benefit if cross-path correctness fails but isolated use is safe; or a named blocking failure and smallest next action. Path isolation does not meet the cross-worktree objective. Failed ownership or correctness blocks adoption. No outcome authorizes implementation or consumer rollout.

## Existing facilities and source context

These sources inform qualification; they are not Oxide implementation or portability proof:

- Acow [QA speed](../../../acowtancy/docs/knowledge/operations/267-qa-speed.md) and [Rust target seeder](../../../acowtancy/scripts/tasks/rust-target-seed.ts): eager Farmyard/Desktop/Silo clone-copy preparation with full-copy fallback. Desktop storage is unnecessary for root QA. This approach is not the proposed Oxide policy.
- Queue [fresh sibling checkouts](../../../paseo-northstar-queue/docs/knowledge/specs/028-fresh-sibling-checkouts.md) defers sibling build-cache sharing. Shared CARGO_HOME download sources and [guarded output cleanup](../../../paseo-northstar-queue/server/worktree-cleanup.ts) are potential integration context, not slot managers.
- Nucleus [core requirements](../../../nucleus/docs/knowledge/contracts/002-core-requirements.md) owns task isolation and build-write requirements.
- Effigy task-result fingerprint caches and container cache lifecycle are not a central Rust slot facility.

Existing Nucleus integration preparation, backup root locking/crash reconciliation, retention cleanup, managed Git worktrees and smaller debug artifacts have their own owners. Consult their Queue records when authorized coordination is needed; do not duplicate their authority or rewrite their work from Oxide.
