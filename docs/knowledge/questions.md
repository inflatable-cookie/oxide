# Open questions

## Q-001 — What implementation shape best supports durable exclusive ownership?

Pointer, 2026-10-18: the daemon prototype and its protocol were retired ([retirements](retired.toml)). The kept same-user ledger is the [local lease CLI](contracts/local-lease.md); containment findings are in [qualification findings](qualification-readiness.md#process-containment). The text below is the original answer.

A per-user local daemon remains the recommended production hub for one durable lease ledger, shared consumer integration and bounded observations. The minimal per-invocation lock/journal alternative is compared in [architecture](architecture.md#storage-invariants); it still needs process guardianship and conservative reconciliation. Queue authorized a bounded local prototype of the daemon candidate, described in [architecture](architecture.md#current-facilities); that prototype does not select a production implementation. The selected host's proof that all slot writers and outstanding write capabilities end or are fenced, plus daemon failure/restart mechanics, remain open for qualification.

The reviewed [macOS readiness findings](qualification-readiness.md#process-containment) refuse process scans as descendant-complete proof. The follow-up compared inherited App Sandbox rights and Endpoint Security; neither supplies a reviewed fence for arbitrary descendants or unrelated same-user writers. Tom approved a narrower cooperative foreground-workflow contract; the fixed native fixtures and S1–S3 of the standalone Cargo workflow have qualified completion within that contract under Q-013. No permanent-quarantine fallback has been accepted as the product design.

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Pointer, 2026-10-18: the mapped-root, source-map and registered collection machinery named below was retired ([retirements](retired.toml)). The condensed findings are in [qualification findings](qualification-readiness.md#cargo-freshness); the proposed cross-checkout gate is W-8 in [workflow](contracts/workflow.md#cross-checkout-freshness). The text below is the original answer.

Current lean-route answer: one pinned Nucleus `dev` workflow demonstrated
registry reuse across fresh checkout paths with current marker/path tests.
The [consumer boundary](contracts/project-adoption.md#current-nucleus-integration-boundary)
owns that narrower ordinary-Cargo result and its unisolated old-mtime control.
The historical strict mapped-path results below remain limitations of that
separate route, not prerequisites to restore signed/callback integration.

Cargo 1.91 stabilizes `build.build-dir` and `CARGO_BUILD_BUILD_DIR`; this moves intermediates but does not prove cross-path reuse. Run 5 on pinned 1.97.1 reused the released slot at B with zero rebuilt units and tests executed, but its binaries retained A compiler source paths. The strict proof failed, and source/configuration/toolchain miss controls did not run. The logical-path candidate uses one identical complete map of consumer-owned roots registered before the first mapped build. Its permanent root-set identity is separate from each task's mutable source revision and exact-run receipt. The consumer proposal is a finite pool of physical task-worktree paths, allocated exclusively and returned only after normal workspace archive and exact source/evidence retention; [project adoption](contracts/project-adoption.md#stable-source-path-lifecycle) summarizes this strict-physical lifecycle and links to the detailed first Nucleus core-test contract. Current Queue task roots are unique and its Spec 028 build outputs stay private; Nucleus Contract 002 keeps task workspaces and branch identities separate. Neither consumer currently supplies a root pool, task-attempt authorization for Oxide acquisition or dirty-source snapshot receipt. These are proposed owner integrations, not shipped APIs. If the consumer cannot support that path lifecycle, stop finite-root remapping for ad hoc worktrees and leave the duplication problem unsolved. Task 019 selects bounded physical-root reuse (strict-physical slot per recycled compile root, Cargo unit invalidation, Oxide mtime-stale content gate) as the useful-reuse architecture; a supported compiled-dependency-only boundary does not exist in pinned Cargo 1.97.1. Exact-tree retry and Task 018's origin+tree acquire match remain safety fallbacks and do not satisfy useful reuse. See the [Task 019 design](qualification-readiness.md#cross-path-results). Cargo 1.97.1 fingerprints Rust-flag changes, so changing the map safely causes a cold partition; do not add roots opportunistically or discard such a partition and call it reuse. Old partitions remain subject to bounded, safe eviction. The only cited size, about 2.3 GiB of planning+diagnostics artifacts, used the line-tables profile; full-debug size is unmeasured. The map only remaps compiler source names, not behavior-bearing manifest/generated/runtime paths. Tom accepted logical compiler names with exact-run source mapping and the experimental qualifier implementation on 2026-10-10; [vision](vision.md#authority-boundaries) owns that ruling. Fixtures exercise resolver, flags and receipt rules only. The registered Cargo runner now has a test-only producer. Task 015 launched one owned Cargo invocation after the 014 TMPDIR rejection; the frozen fixture build script failed (`PathBuf` formatted with `Display` in `probe/build.rs:21`), tests did not run, and mapped reuse remains unqualified. Production keys stay checkout-isolated and no portability or daily-duplication benefit is established. Reassess at each supported Cargo/rustc upgrade; retire remapping only after shipped Cargo support passes the full qualification gate without the per-root map. See the [portability assessment](qualification-readiness.md#compiler-and-behavior-paths), [Cargo capability findings](rust-dx-landscape.md) and [storage architecture](qualification-readiness.md#cross-path-results).

The current Oxide evidence path parses bounded hook rows, recomputes private binary/log digests, and binds receipts to policy/map/build identity, requested tree, lease and exact run/capture. Source, scanner, origin and behavior rows remain producer declarations. Task 017 added context-bound relative prefix mapping and an explicit full-debug `packed` fixture profile. The original cold-A run passed and emitted accepted logical `file!()` and panic paths; its scanner initially refused ambiguous sibling `.dSYM` bundles. The corrected producer selects Cargo's exact `<name>.dSYM`, parses real dashed UUID lines, and extracts the fixture's compile-unit and line-table path forms. The probe now derives `CARGO_ENCODED_RUSTFLAGS` from the resolved map's ordered effective flags. In the v2 session, cold A passed its fixture test and the exact companion UUIDs and required sections matched, but `dwarfdump --debug-info` exceeded the 1 MiB scanner cap, so full-debug logical source lookup remained incomplete. The run stopped before source invalidation and cross-path reuse. The recorded UTC session deadline was preserved without reset, but malformed-scale monotonic markers and gaps in 30-second host sampling leave `session_clock` and `resource_bounds` insufficient. Any further probe requires a new scoped Queue record and fresh resources. The exact result, preserved evidence and resource bounds are in the [Task 017 execution record](qualification-readiness.md#compiler-and-behavior-paths) and [registered Cargo source-map schema](qualification-readiness.md#compiler-and-behavior-paths).

The v3 continuation now observed the required full-debug project source lookup,
complete packed-companion capture and UUID/hash binding after a passing cold-A
test. Changed B rebuilt affected units and executed the expected new values,
but the driver rejected a fresh unchanged build-script compiler artifact. This
was a source-control harness mismatch; its correction has ordinary coverage
but no new live receipt. The driver ended after two invocations without shared
cold A or warm B. V3 is preserved and cannot be resumed with a reconstructed
clock. Cross-path reuse remains unqualified; see the
[v3 observations](qualification-readiness.md#compiler-and-behavior-paths).

The fresh v4 session passed baseline/full-debug lookup and the corrected
changed-source control. Its third invocation requested A but reacquired the
control slot and reused changed-B artifacts with zero rebuilds. The test
executed and failed: runtime baseline bytes disagreed with the compiled changed
marker. The lease became Uncertain and remains retained; warm B was not run.
Task 018 pinned the cause: logical mapped partitions omit requested root ID, so
the allocator reused the idle control slot, and Cargo 1.97.1 treated relative
`rerun-if-changed` / dep-info inputs as fresh because A's files were older than
B's fingerprint anchors and encoded dep-info stores no `CARGO_MANIFEST_DIR`.
The smallest fail-closed correction for that mapped-logical defect is
pre-execution compile-origin admission: acquire matches
`partition + requested_root_id + source_tree_digest`, and a mismatch creates
a new slot or falls back rather than executing those intermediates.
Same-origin warm retry can remain; mapped A/B hits cannot. That correction
does not satisfy useful reuse. [Task 019](qualification-readiness.md#cross-path-results)
selects same-root slot reuse under strict-physical identity instead. There
is no qualified cross-path reuse or storage benefit. See
[v4 observations](qualification-readiness.md#cargo-freshness)
and [task 018](qualification-readiness.md#cargo-freshness).

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

The design recommends distinct compatibility-partition and per-run identities; durable idempotent acquire/reconcile/finalize; refusal for concurrent or uncertain claims; and proven-idle-only eviction. A later ruling settled the 20 GiB host free-space rule as a monitored start/abort threshold, not a guarantee. Fixed native fixtures enforce a small task-owned bound; the standalone Cargo pilot's approved 8 GiB image and host-threshold controls passed independent review for S1–S3 under its 10 GiB disposable ceiling. This does not qualify production capacity policy, arbitrary output coverage or write volume. Production host locking/process containment and later product budgets remain open. See [the ownership protocol](architecture.md#storage-invariants) and [budget policy](architecture.md#storage-invariants).

Reviewed [capacity findings](qualification-readiness.md#capacity) include temporary output in the disposable footprint and distinguish sampled aborts from kernel enforcement. The host reserve policy is settled in Q-014. The separate enforced disposable-footprint cap remains a prerequisite.

## Q-004 — What narrowly scoped pilot may be authorized?

Pointer, 2026-10-18: the experimental qualifier and probes named below were retired ([retirements](retired.toml)); their results are condensed in [qualification findings](qualification-readiness.md). The text below is the original answer.

Tom approved the standalone oxide#005 trial on 2026-10-09 after independent native-fixture review. [Vision](vision.md#authority-boundaries) owns its exact source/patch, temporary disk-image authority, local-binding sequence and time limits; the Queue brief owns the executable protocol. Its reviewed result is partial negative qualification: cooperative completion, pilot resource bounds and local evidence binding passed for S1–S3, while strict portability failed and invalidation was incomplete. The original experiment window is closed. A design-only follow-up recommends explicit rustc path remapping for a finite pre-registered root set and proposes a fresh five-scenario pilot: source, configuration and toolchain miss controls first, then one cold-A/different-path-warm-B pair. Tom subsequently accepted the logical-path contract. The Oxide-only qualifier is now implemented with small fixture validation, not a new real-project experiment. Any future pilot needs a separately approved Queue brief, new resources and time scope; do not reuse 005 authority or resources. Actual consumer integration and compiler-wrapper rollout coordination remain separate adoption gates. Resolve the RustQuality profile/deviations before Rust authoring and require independent exact-head review. See [reviewed evidence](qualification-readiness.md#compiler-and-behavior-paths) and the [portability proposal](qualification-readiness.md#compiler-and-behavior-paths).

[Qualification readiness](qualification-readiness.md#cross-path-results) records a candidate selector and gates. Its earlier single-held-lease measurement proposal is superseded: it is neither an authorized experiment nor a substitute for normal release/reacquire. Merging design documentation does not authorize any build or pilot.

Tom's standing authority in [vision](vision.md#authority-boundaries) and the scoped Queue briefs authorized the tiny Oxide-owned Rust probe. Task 014 stopped before Cargo; Task 015 hit the fixture compile defect; Task 016 fixed the fixture but failed the accepted logical-path predicate. Task 017's v1 and v2 cold-A tests passed and emitted accepted logical `file!()` and panic paths. Exact `.dSYM` selection and UUID binding worked in v2, but the full DWARF source scan exceeded its 1 MiB output limit. In that v2 session, baseline source lookup remained unproven; source invalidation, cross-path behavior and reuse were not reached. The v2 root/image and raw capture are retained read-only. Its original UTC deadline was kept, although the raw monotonic marker scale and 30-second host sampling cadence make those two review gates insufficient. A new scoped Queue record and fresh resources are required for any further attempt. V4 later established baseline lookup and source invalidation, then stopped on actual stale-source behavior in its shared slot; its Uncertain lease/image remain preserved. Task 018 records the compile-origin admission contract and a four-invocation proof proposal; it authorizes no implementation or live probe. Task 019 records the useful-reuse architecture and a separate same-root different-tree proof proposal; it also authorizes no implementation or live probe. Its corrected cold recovery design distinguishes invalid reuse evidence from uncertain ownership: only proven-Idle, authorized, budgeted whole-generation replacement can initialize an empty successor and rebind after successful unchanged-tree compile/test/capture. The four-invocation proof checks post-failure refusal and recovery eligibility only; live successful recovery/rebind requires a separately bounded future proof, not a fifth compiler invocation. Do not dispatch the 018 origin-gate proof as a substitute for 019. No ended clock can be resumed, no v5 of old resources is authorized, and consumer adoption and a new Nucleus qualification remain separate scopes.

## Q-005 — What names are safe to publish?

Review executable/crate/distribution names against the existing Oxide Computer Company SDK/CLI before publication. Owner: [release](contracts/release.md).

## Q-006 — What experience and budgets define light Rust development?

For the first foothold, use one narrow native workflow and bounded qualification; broad baselines are not a prerequisite. Choose longer-term representative workflows and acceptable disk footprint, write volume, feedback latency, memory and operator-effort targets after first storage relief. Tom's direction is confirmed; numeric targets are not. Owner: [vision](vision.md).

## Q-007 — How should Oxide consume upstream caching and existing tools?

The strategy is settled: use stable Cargo 1.91 build-dir redirection as the smallest mechanism to qualify while the accepted upstream cross-workspace cache goal is still experimental direction, not a stable capability guarantee. Keep policy independent of the mechanism. Replace it only after supported Cargo coverage and exact-workflow tests prove ownership, path correctness, lifecycle bounds and evidence preservation. See the [sourced capability and replacement criteria](rust-dx-landscape.md) and [architecture](architecture.md#storage-invariants).

## Q-008 — Which host/container division preserves required production parity?

Coordinate Rust-specific recommendations with Effigy's language-neutral execution routes. Native feedback, Linux builds/tests and Colima storage need separate measurements and ownership. Owner: [vision](vision.md).

## Q-009 — Which alternative execution modes improve the daily loop?

JIT, Cranelift and other code generation/execution solutions are confirmed research scope. Compare fast ahead-of-time compilation, JIT and hot reload by complete edit-to-feedback cost and supported semantics. Mechanisms, compatibility and validation mapping remain open. Owner: [Rust DX landscape](rust-dx-landscape.md); priority ruling: [vision](vision.md).

## Q-010 — Which integration surface best preserves familiar development loops?

Pointer, 2026-10-18: the hybrid adapter design below was retired with the protected route; [project adoption](contracts/project-adoption.md#recommendation) keeps its lasting rules.

Tom's [2026-10-18 direction](vision.md#product-workflow-direction) selects explicit `oxide check|test|clippy|build|run` commands with agent guidance as the product surface; interception is deferred. The proposed [workflow](contracts/workflow.md) owns the shape. The text below records the earlier consumer-route answers.

Tom's 2026-10-16 Nucleus ruling replaces its planned protected-route adoption
with a same-user local lease CLI. The unpublished command is owned by
[local lease](contracts/local-lease.md). The [current consumer boundary](contracts/project-adoption.md#current-nucleus-integration-boundary)
owns the request and Oxide's source-path, release/expiry and capacity
clarifications. Cross-worktree qualification remains a dependent gate. The
earlier hybrid design below remains a separate upstream route, not a
prerequisite for the lean CLI.

Design answer: [project adoption](contracts/project-adoption.md#recommendation) recommends explicit project/workflow opt-in plus an adapter around familiar commands, with per-invocation Cargo settings and private final output. This is a proposed integration contract, not an implemented consumer registration or rollout. The concrete consumer binding remains Q-012.

## Q-011 — How should slot waiting coordinate with host admission?

Design answer: [project adoption](contracts/project-adoption.md#readiness-admission-and-acquire-ordering) defines non-reserving cancellable waiting, advisory readiness, authoritative consumer admission, exclusive acquire and consumer-owned recovery from a busy race. Consumer lifecycle mapping remains Q-012; numeric waiting/attention targets remain part of Q-006.

## Q-012 — How does the consumer bind task admission, storage acquire and host-run scheduling?

Pointer, 2026-10-18: the registered library route and fixture bridge named below were retired ([retirements](retired.toml)); the [Nucleus core reuse record](contracts/nucleus-core-reuse.md) is condensed historical evidence. The text below is the original answer.

Design answer and exact open owner gates live in the
[first Nucleus core-test contract](contracts/nucleus-core-reuse.md), with general
ordering in [project adoption](contracts/project-adoption.md#readiness-admission-and-acquire-ordering).
Select the existing core planning/diagnostics test workflow and sequential
strict-physical root reuse after ordinary archive. The reviewed positive Task 021
fixture is the mechanism starting point, not Nucleus input eligibility or
adoption evidence. Production consumers still lack an installed authenticated
Oxide binding and qualified source pool; disabled consumer-local ownership,
storage-attempt and retention fixtures are described in the owning contract. Managed-Git,
Contract 002/Spec 028, admission/host-run and evidence/retention owners must agree
the proposed opt-in, allocation epochs, bounded retention, exact dirty/generated/
dependency source and old-run full-debug lookup before root recycling. Frozen
010 remains unchanged: storage busy creates no host run or settlement, and the
current genuine scheduler alone issues host tokens. Third-party/native input
audit, SDK/tool integrity, runtime unit coverage, representative useful relief
and successful live recovery remain separate gates. No owner consent, execution,
consumer implementation or everyday benefit is established by this design.

## Q-013 — How can a healthy macOS workflow safely return its slot for reuse?

Pointer, 2026-10-18: the native fixture runner was retired; its result is condensed in [qualification findings](qualification-readiness.md#process-containment). The text below is the original answer.

The reviewed [macOS completion/fencing disposition](qualification-readiness.md#process-containment) found no supported public mechanism proving healthy release under its broad writer model. Tom subsequently approved the cooperative registered foreground-workflow contract on 2026-10-09; [architecture](qualification-readiness.md#process-containment) owns its promises, excluded bypasses and uncertainty rules. The native fixture boundary passed independent exact-head review for `writer_model`, `healthy_completion`, `uncertainty`, `capture_binding` and `fixture_bounds`. The standalone Cargo runner then qualified cooperative completion and local capture binding for S1–S3, including release/reacquire; [qualification readiness](qualification-readiness.md) owns both receipts and coverage limits. This is not a descendant census or arbitrary same-user fence. Cargo-specific interruption controls, broader workflows, cross-path reuse and consumer adoption remain unqualified or gated.

## Q-014 — Is 20 GiB host free space a guarantee or a start/abort threshold?

Settled by Tom on 2026-10-09: use 20 GiB as a monitored start/abort threshold, not a guarantee against unrelated host writers. [Architecture](architecture.md#storage-invariants) owns the policy and required execution-protocol details. Tom separately approved one fixed task-owned image for the standalone pilot; its capacity and monitored host-threshold controls passed review for S1–S3. This grants no general volume creation or production budget authority; [vision](vision.md#authority-boundaries) owns that exact scope.

## Q-015 — What does the foundation workflow need settled before implementation?

Pointer, 2026-10-18: W-5 retirement execution is done ([retirements](retired.toml)).

The proposed [workflow](contracts/workflow.md#open-decisions) lists W-1 to W-8: default pool, waiting and concurrency, project file, crash-recovery qualification, retirement execution, Linux backend, command naming and the cross-checkout freshness gate. Crash-recovery and retirement direction are ruled; the rest have recommendations. Owner: [workflow](contracts/workflow.md).
