# Open questions

## Q-001 — What implementation shape best supports durable exclusive ownership?

A per-user local daemon is the recommended hub for one durable lease ledger, shared consumer integration and bounded observations. The minimal per-invocation lock/journal alternative is compared in [architecture](architecture.md#minimum-safe-cross-worktree-reuse-design); it still needs process guardianship and conservative reconciliation. This remains a design recommendation, not a selected implementation. The selected host's proof that task descendants cannot outlive slot ownership, plus daemon failure/restart mechanics, remain open for qualification.

The reviewed [macOS readiness findings](qualification-readiness.md#process-containment) refuse process scans as descendant-complete proof. A usable normal-completion mechanism is Q-013; no permanent-quarantine fallback has been accepted as the product design.

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Cargo 1.91 stabilizes `build.build-dir` and `CARGO_BUILD_BUILD_DIR`; this moves intermediates but does not prove cross-path reuse. The exact pinned Cargo/rustc and host are still pilot prerequisites. Qualify generated code, proc macros, embedded paths, test executables and separate final artifact capture before choosing shared versus path-isolated partitions. No assumed MSRV. See [Cargo's current capabilities](rust-dx-landscape.md) and [the protocol](architecture.md#minimum-safe-cross-worktree-reuse-design).

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

The design recommends distinct compatibility-partition and per-run identities; durable idempotent acquire/reconcile/finalize; refusal for concurrent or uncertain claims; and proven-idle-only eviction. The pilot proposal uses a 10 GiB disposable cap and 20 GiB host free-space reserve, but read-only feasibility and enforceability remain prerequisites. Exact host locking/process-containment behavior and any later product budgets are unsettled. See [the ownership protocol](architecture.md#semantic-operations) and [budget proposal](architecture.md#budget-and-observations).

Reviewed [capacity findings](qualification-readiness.md#temporary-output-must-be-in-the-10-gib-footprint) include temporary output in the disposable footprint and distinguish sampled aborts from kernel enforcement. The host reserve policy needs Tom's ruling in Q-014. Existing hard-bound prerequisites remain in force until explicitly changed.

## Q-004 — What narrowly scoped pilot may be authorized?

Candidate: one native Nucleus core-test Effigy selector at one fixed exact head in two disposable checkout paths, with per-run artifacts/logs and one bounded exclusive intermediate slot. The design task does not authorize execution. The exact selector ID, commit/tree, installed pinned toolchain, paths, measurement method and enforceable cap/reserve need separate scoped approval. Resolve the RustQuality profile/deviations before Rust authoring, coordinate with the Nucleus owner to avoid compiler-wrapper duplication, then require independent exact-head review. See [pilot gates and protocol](architecture.md#proposed-bounded-qualification) and [authority](vision.md).

[Qualification readiness](qualification-readiness.md#candidate-selector-head-and-toolchain) refines the candidate and records remaining gates. Its single-held-lease measurement proposal is not an approved replacement for normal release/reuse or idle-eviction proof. Merging design documentation does not authorize any build or pilot.

## Q-005 — What names are safe to publish?

Review executable/crate/distribution names against the existing Oxide Computer Company SDK/CLI before publication. Owner: [release](contracts/release.md).

## Q-006 — What experience and budgets define light Rust development?

For the first foothold, use one narrow native workflow and bounded qualification; broad baselines are not a prerequisite. Choose longer-term representative workflows and acceptable disk footprint, write volume, feedback latency, memory and operator-effort targets after first storage relief. Tom's direction is confirmed; numeric targets are not. Owner: [vision](vision.md).

## Q-007 — How should Oxide consume upstream caching and existing tools?

The strategy is settled: use stable Cargo 1.91 build-dir redirection as the smallest mechanism to qualify while the accepted upstream cross-workspace cache goal is still experimental direction, not a stable capability guarantee. Keep policy independent of the mechanism. Replace it only after supported Cargo coverage and exact-workflow tests prove ownership, path correctness, lifecycle bounds and evidence preservation. See the [sourced capability and replacement criteria](rust-dx-landscape.md) and [architecture](architecture.md#exact-run-evidence-and-cargo-boundary).

## Q-008 — Which host/container division preserves required production parity?

Coordinate Rust-specific recommendations with Effigy's language-neutral execution routes. Native feedback, Linux builds/tests and Colima storage need separate measurements and ownership. Owner: [vision](vision.md).

## Q-009 — Which alternative execution modes improve the daily loop?

JIT, Cranelift and other code generation/execution solutions are confirmed research scope. Compare fast ahead-of-time compilation, JIT and hot reload by complete edit-to-feedback cost and supported semantics. Mechanisms, compatibility and validation mapping remain open. Owner: [Rust DX landscape](rust-dx-landscape.md); priority ruling: [vision](vision.md).

## Q-010 — Which integration surface best preserves familiar development loops?

Design answer: [project adoption](contracts/project-adoption.md#recommendation) recommends explicit project/workflow opt-in plus an adapter around familiar commands, with per-invocation Cargo settings and private final output. This is a proposed integration contract, not an implemented consumer registration or rollout. The concrete consumer binding remains Q-012.

## Q-011 — How should slot waiting coordinate with host admission?

Design answer: [project adoption](contracts/project-adoption.md#readiness-admission-and-acquire-ordering) defines non-reserving cancellable waiting, advisory readiness, authoritative consumer admission, exclusive acquire and consumer-owned recovery from a busy race. Consumer lifecycle mapping remains Q-012; numeric waiting/attention targets remain part of Q-006.

## Q-012 — How does the consumer bind task admission, storage acquire and host-run scheduling?

The proposed [adoption contract](contracts/project-adoption.md#readiness-admission-and-acquire-ordering) requires a consumer-approved mapping before real Queue/Nucleus adapter implementation. Frozen Contract010 admits and launches host runs together; it has no not-started storage-busy outcome. Establish authenticated task/attempt authorization for acquire, then the existing heavy-host-run submission, with exact identity, cancellation and capture binding. No invented host settlement, forged run token or execution slot held while waiting. Concrete consumer opt-in fields and compatibility-key availability after checkout preparation also need owner binding. Oxide's task-owned fixture prototype can exercise the semantic boundary without claiming this consumer mapping exists; consumer integration and real execution remain gated.

## Q-013 — How can a healthy macOS workflow safely return its slot for reuse?

The reviewed [containment disposition](qualification-readiness.md#process-containment) supplies no in-scope descendant-complete release proof after a native launch. Permanent uncertainty after every build would prevent the intended everyday reuse. Define the supported writer/threat model and establish a concrete completion or fencing mechanism, its crash/cancel/restart behavior and a bounded qualification plan. Neither a clean process scan nor a consumer completion claim alone is accepted proof. Unknown outcomes remain unavailable. A design follow-up must identify any required contract change or host authority for explicit approval; no relaxation is assumed.

## Q-014 — Is 20 GiB host free space a guarantee or a start/abort threshold?

The reviewed [reserve findings](qualification-readiness.md#20-gib-host-free-space-reserve--unresolved) show that the proposed isolated-volume quota would not guarantee free space on the shared host. Tom must decide whether to retain the guarantee (execution remains blocked) or explicitly adopt a monitored start/abort threshold with its stated races. This decision would not authorize volume creation, weaken the separate disposable-footprint cap or release the pilot hold.
