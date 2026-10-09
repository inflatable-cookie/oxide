# Open questions

## Q-001 — What implementation shape best supports durable exclusive ownership?

A per-user local daemon remains the recommended production hub for one durable lease ledger, shared consumer integration and bounded observations. The minimal per-invocation lock/journal alternative is compared in [architecture](architecture.md#minimum-safe-cross-worktree-reuse-design); it still needs process guardianship and conservative reconciliation. Queue authorized a bounded local prototype of the daemon candidate, described in [architecture](architecture.md#local-storage-prototype); that prototype does not select a production implementation. The selected host's proof that all slot writers and outstanding write capabilities end or are fenced, plus daemon failure/restart mechanics, remain open for qualification.

The reviewed [macOS readiness findings](qualification-readiness.md#process-containment) refuse process scans as descendant-complete proof. The follow-up compared inherited App Sandbox rights and Endpoint Security; neither supplies a reviewed fence for arbitrary descendants or unrelated same-user writers. Tom approved a narrower cooperative foreground-workflow contract; the fixed native fixtures and S1–S3 of the standalone Cargo workflow have qualified completion within that contract under Q-013. No permanent-quarantine fallback has been accepted as the product design.

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Cargo 1.91 stabilizes `build.build-dir` and `CARGO_BUILD_BUILD_DIR`; this moves intermediates but does not prove cross-path reuse. Run 5 on pinned 1.97.1 reused the released slot at B with zero rebuilt units and tests executed, but its binaries retained A compiler source paths. The strict proof failed, and source/configuration/toolchain miss controls did not run. The design recommendation is rustc path remapping to a logical prefix using one identical complete map for all authorized roots registered before the first build. Cargo 1.97.1 fingerprints Rust-flag changes, so changing the map safely causes misses; new roots are not shown to reuse arbitrary prior work. Since Queue worktrees are ad hoc and normally unique, every root-set change can cold-start a new partition; benefit requires a stable checkout pool reused across tasks, which is not established. This remaps compiler source names, not behavior-bearing manifest/generated/runtime paths. An operator decision is required before logical source names replace the B-absolute-path requirement. Until then production keys stay checkout-isolated and the strict proof remains binding. Reassess remapping at each supported Cargo/rustc upgrade; retire it only after shipped Cargo support passes the full qualification gate without the per-root map. If Queue cannot maintain a stable pool, stop this candidate and record that the duplication problem remains unsolved. See [the portability assessment, exact pass predicates and bounded follow-up proposal](qualification-readiness.md#portable-source-identity-proposal-design-only-2026-10-09), [Cargo's capabilities](rust-dx-landscape.md) and [the protocol](architecture.md#minimum-safe-cross-worktree-reuse-design).

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

The design recommends distinct compatibility-partition and per-run identities; durable idempotent acquire/reconcile/finalize; refusal for concurrent or uncertain claims; and proven-idle-only eviction. A later ruling settled the 20 GiB host free-space rule as a monitored start/abort threshold, not a guarantee. Fixed native fixtures enforce a small task-owned bound; the standalone Cargo pilot's approved 8 GiB image and host-threshold controls passed independent review for S1–S3 under its 10 GiB disposable ceiling. This does not qualify production capacity policy, arbitrary output coverage or write volume. Production host locking/process containment and later product budgets remain open. See [the ownership protocol](architecture.md#semantic-operations) and [budget policy](architecture.md#budget-and-observations).

Reviewed [capacity findings](qualification-readiness.md#temporary-output-must-be-in-the-10-gib-footprint) include temporary output in the disposable footprint and distinguish sampled aborts from kernel enforcement. The host reserve policy is settled in Q-014. The separate enforced disposable-footprint cap remains a prerequisite.

## Q-004 — What narrowly scoped pilot may be authorized?

Tom approved the standalone oxide#005 trial on 2026-10-09 after independent native-fixture review. [Vision](vision.md#authority-boundaries) owns its exact source/patch, temporary disk-image authority, local-binding sequence and time limits; the Queue brief owns the executable protocol. Its reviewed result is partial negative qualification: cooperative completion, pilot resource bounds and local evidence binding passed for S1–S3, while strict portability failed and invalidation was incomplete. The original experiment window is closed. A design-only follow-up recommends explicit rustc path remapping for a finite pre-registered root set and proposes a fresh five-scenario pilot: source, configuration and toolchain miss controls first, then one cold-A/different-path-warm-B pair. Neither implementation nor experiment is authorized by this record. Any future run needs a separate Queue brief, new resources and time scope, and an operator decision on logical source paths; do not reuse 005 authority or resources. Actual consumer integration and compiler-wrapper rollout coordination remain separate adoption gates. Resolve the RustQuality profile/deviations before Rust authoring and require independent exact-head review. See [reviewed evidence](qualification-readiness.md#pilot-execution-record-run-5-2026-10-09-partial) and the [portability proposal](qualification-readiness.md#portable-source-identity-proposal-design-only-2026-10-09).

[Qualification readiness](qualification-readiness.md#candidate-selector-head-and-toolchain) records a candidate selector and gates. Its earlier single-held-lease measurement proposal is superseded: it is neither an authorized experiment nor a substitute for normal release/reacquire. Merging design documentation does not authorize any build or pilot.

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

The proposed [adoption contract](contracts/project-adoption.md#readiness-admission-and-acquire-ordering) requires a consumer-approved mapping before real Queue/Nucleus adapter implementation. Frozen Contract010 admits and launches host runs together; it has no not-started storage-busy outcome. Establish authenticated task/attempt authorization for acquire, then the existing heavy-host-run submission, with exact identity, cancellation and capture binding. No invented host settlement, forged run token or execution slot held while waiting. Concrete consumer opt-in fields and compatibility-key availability after checkout preparation also need owner binding. The fixture prototype and Tom-approved standalone local pilot may exercise facility semantics without claiming this consumer mapping exists. Actual consumer integration, authenticated settlement and rollout remain gated; honest standalone qualification uses separate local run/capture identities under its approved brief.

## Q-013 — How can a healthy macOS workflow safely return its slot for reuse?

The reviewed [macOS completion/fencing disposition](qualification-readiness.md#writer-model-and-completion-proof) found no supported public mechanism proving healthy release under its broad writer model. Tom subsequently approved the cooperative registered foreground-workflow contract on 2026-10-09; [architecture](architecture.md#supported-writer-model) owns its promises, excluded bypasses and uncertainty rules. The native fixture boundary passed independent exact-head review for `writer_model`, `healthy_completion`, `uncertainty`, `capture_binding` and `fixture_bounds`. The standalone Cargo runner then qualified cooperative completion and local capture binding for S1–S3, including release/reacquire; [qualification readiness](qualification-readiness.md) owns both receipts and coverage limits. This is not a descendant census or arbitrary same-user fence. Cargo-specific interruption controls, broader workflows, cross-path reuse and consumer adoption remain unqualified or gated.

## Q-014 — Is 20 GiB host free space a guarantee or a start/abort threshold?

Settled by Tom on 2026-10-09: use 20 GiB as a monitored start/abort threshold, not a guarantee against unrelated host writers. [Architecture](architecture.md#budget-and-observations) owns the policy and required execution-protocol details. Tom separately approved one fixed task-owned image for the standalone pilot; its capacity and monitored host-threshold controls passed review for S1–S3. This grants no general volume creation or production budget authority; [vision](vision.md#authority-boundaries) owns that exact scope.
