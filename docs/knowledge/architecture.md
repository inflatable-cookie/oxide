# Build storage boundaries

These are design-only constraints agreed by Tom on 2026-10-09, not an implemented API.

## Storage and ownership

Persistent bounded cache slots partition by project/component identity, exact toolchain, host/target platform, and resolved configuration/profile/features/relevant environment. One exclusive durable lease covers compile through test and artifact capture. Unknown crash outcomes, lost replies, age and PID disappearance do not imply release. Reconciliation must prove safe ownership settlement before reuse or eviction.

Exact-run final binaries, logs, QA receipts and evidence remain independently owned and preserved. Cargo intermediates are internal and may contain test executables; separating output directories alone cannot prove isolation. Cache reuse never counts as validation success.

Prepare only requested components lazily. No eager copying of all targets and no full-copy fallback. Evict idle slots only, within explicit budgets; occupied space and actual disk writes must be measured separately. Inventory/acquire/reconcile/finalize/budget describe interface semantics, not an existing CLI.

## Qualification boundaries

Modern Cargo `build.build-dir` / `CARGO_BUILD_BUILD_DIR`, separate from target-dir, is worth qualifying on an actually supported pinned toolchain at different checkout paths. Support and portability are unproven here. If portability fails, use path-isolated partitions and report the reduced benefit. Do not blindly adopt sccache: Acow's measured trial rejected it.

Nucleus Contract002 prohibits mutable target sharing. An exclusive-intermediate design requires narrow qualification before adoption and cannot weaken that contract. Frozen Contract010 remains unchanged. Queue/Nucleus own admission, source and run identities, authenticated settlement, retention and artifact capture; Oxide owns only its bounded storage semantics. See [scope](vision.md).

## Existing facilities and source context

These sources inform qualification; they are not Oxide implementation or portability proof:

- Acow [QA speed](../../../acowtancy/docs/knowledge/operations/267-qa-speed.md) and [Rust target seeder](../../../acowtancy/scripts/tasks/rust-target-seed.ts): eager Farmyard/Desktop/Silo clone-copy preparation with full-copy fallback. Desktop storage is unnecessary for root QA. This approach is not the proposed Oxide policy.
- Queue [fresh sibling checkouts](../../../paseo-northstar-queue/docs/knowledge/specs/028-fresh-sibling-checkouts.md) defers sibling build-cache sharing. Shared CARGO_HOME download sources and [guarded output cleanup](../../../paseo-northstar-queue/server/worktree-cleanup.ts) are potential integration context, not slot managers.
- Nucleus [core requirements](../../../nucleus/docs/knowledge/contracts/002-core-requirements.md) owns task isolation and build-write requirements.
- Effigy task-result fingerprint caches and container cache lifecycle are not a central Rust slot facility.

Existing Nucleus integration preparation, backup root locking/crash reconciliation, retention cleanup, managed Git worktrees and smaller debug artifacts have their own owners. Consult their Queue records when authorized coordination is needed; do not duplicate their authority or rewrite their work from Oxide.
