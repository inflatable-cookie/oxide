# Open questions

## Q-001 — What implementation shape best supports durable exclusive ownership?

A per-user local daemon remains the recommended production hub for one durable lease ledger, shared consumer integration and bounded observations. The minimal per-invocation lock/journal alternative is compared in [architecture](architecture.md#minimum-safe-cross-worktree-reuse-design); it still needs process guardianship and conservative reconciliation. Queue authorized a bounded local prototype of the daemon candidate, described in [architecture](architecture.md#local-storage-prototype); that prototype does not select a production implementation. The selected host's proof that task descendants cannot outlive slot ownership, plus daemon failure/restart mechanics, remain open for qualification.

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Cargo 1.91 stabilizes `build.build-dir` and `CARGO_BUILD_BUILD_DIR`; this moves intermediates but does not prove cross-path reuse. The exact pinned Cargo/rustc and host are still pilot prerequisites. Qualify generated code, proc macros, embedded paths, test executables and separate final artifact capture before choosing shared versus path-isolated partitions. No assumed MSRV. See [Cargo's current capabilities](rust-dx-landscape.md) and [the protocol](architecture.md#minimum-safe-cross-worktree-reuse-design).

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

The design recommends distinct compatibility-partition and per-run identities; durable idempotent acquire/reconcile/finalize; refusal for concurrent or uncertain claims; and proven-idle-only eviction. The pilot proposal uses a 10 GiB disposable cap and 20 GiB host free-space reserve, but read-only feasibility and enforceability remain prerequisites. Exact host locking/process-containment behavior and any later product budgets are unsettled. See [the ownership protocol](architecture.md#semantic-operations) and [budget proposal](architecture.md#budget-and-observations).

## Q-004 — What narrowly scoped pilot may be authorized?

Candidate: one native Nucleus core-test Effigy selector at one fixed exact head in two disposable checkout paths, with per-run artifacts/logs and one bounded exclusive intermediate slot. The design task does not authorize execution. The exact selector ID, commit/tree, installed pinned toolchain, paths, measurement method and enforceable cap/reserve need separate scoped approval. Resolve the RustQuality profile/deviations before Rust authoring, coordinate with the Nucleus owner to avoid compiler-wrapper duplication, then require independent exact-head review. See [pilot gates and protocol](architecture.md#proposed-bounded-qualification) and [authority](vision.md).

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
