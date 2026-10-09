# Build storage boundaries

These are design-only constraints agreed by Tom on 2026-10-09, not an implemented API.

## Storage and ownership

Persistent bounded cache slots partition by project/component identity, exact toolchain, host/target platform, and resolved configuration/profile/features/relevant environment. One exclusive durable lease covers compile through test and artifact capture. Unknown crash outcomes, lost replies, age and PID disappearance do not imply release. Reconciliation must prove safe ownership settlement before reuse or eviction.

Exact-run final binaries, logs, QA receipts and evidence remain independently owned and preserved. Cargo intermediates are internal and may contain test executables; separating output directories alone cannot prove isolation. Cache reuse never counts as validation success.

Prepare only requested components lazily. No eager copying of all targets and no full-copy fallback. Evict idle slots only, within explicit budgets; occupied space and actual disk writes must be measured separately. Inventory/acquire/reconcile/finalize/budget describe interface semantics, not an existing CLI.

## Qualification boundaries

Modern Cargo `build.build-dir` / `CARGO_BUILD_BUILD_DIR`, separate from target-dir, is worth qualifying on an actually supported pinned toolchain at different checkout paths. Support and portability are unproven here. If portability fails, use path-isolated partitions and report the reduced benefit. Do not blindly adopt sccache: Acow's measured trial rejected it.

Nucleus Contract002 prohibits mutable target sharing. An exclusive-intermediate design requires narrow qualification before adoption and cannot weaken that contract. Frozen Contract010 remains unchanged. Queue/Nucleus own admission, source and run identities, authenticated settlement, retention and artifact capture; Oxide owns only its bounded storage semantics. See [scope](vision.md).

## Preferred daemon hub to qualify

Tom's direction, 2026-10-09: a central daemon could normalize project workflows, own reusable storage and provide a consistent place to measure real-world use. It is the preferred candidate for the bounded design task, not an implemented system or a final implementation choice.

The design should define a minimal opt-in project/consumer adapter: resolve build identity, obtain scheduler admission where required, acquire a slot, execute the declared workflow, preserve/capture run outputs under consumer policy and finalize ownership with authenticated evidence. Every actor touching an adopted slot must follow the lease contract; direct Cargo access cannot silently bypass it. Commands outside the adapter have no claim to Oxide-managed coverage.

The daemon owns the storage ledger and records observations keyed by project/component, source/run identity, resolved toolchain/configuration and backend. Observation is not authenticated run settlement or a validation verdict. Queue/Nucleus remain authoritative for admission, managed source/run identities, settlement and evidence/retention policy. Daemon failure/restart must reconcile durable leases conservatively; loss of connection cannot grant reuse. Consumer cancellation and process lifetime must have explicit owners.

Start with bounded local events for acquisition/refusal, timing, rebuild/reuse observations, storage growth and finalization. Account for measurement overhead and unavailable write attribution. General tracing, dashboards and experiment scheduling are not requirements for first relief. Ordinary-use telemetry can reveal contention and trends after adoption; controlled qualification is still needed for cache correctness and failure cases. Future experiments are explicit opt-ins, use isolated declared resources and do not silently alter everyday flags or source.

Keep storage/execution adapters replaceable and consumer-facing semantics stable as Cargo catches up. The design task must propose concrete daemon availability, project adoption and recovery behavior within its two-hour investigation bound; it does not install, execute or wire this design.

## Existing facilities and source context

These sources inform qualification; they are not Oxide implementation or portability proof:

- Acow [QA speed](../../../acowtancy/docs/knowledge/operations/267-qa-speed.md) and [Rust target seeder](../../../acowtancy/scripts/tasks/rust-target-seed.ts): eager Farmyard/Desktop/Silo clone-copy preparation with full-copy fallback. Desktop storage is unnecessary for root QA. This approach is not the proposed Oxide policy.
- Queue [fresh sibling checkouts](../../../paseo-northstar-queue/docs/knowledge/specs/028-fresh-sibling-checkouts.md) defers sibling build-cache sharing. Shared CARGO_HOME download sources and [guarded output cleanup](../../../paseo-northstar-queue/server/worktree-cleanup.ts) are potential integration context, not slot managers.
- Nucleus [core requirements](../../../nucleus/docs/knowledge/contracts/002-core-requirements.md) owns task isolation and build-write requirements.
- Effigy task-result fingerprint caches and container cache lifecycle are not a central Rust slot facility.

Existing Nucleus integration preparation, backup root locking/crash reconciliation, retention cleanup, managed Git worktrees and smaller debug artifacts have their own owners. Consult their Queue records when authorized coordination is needed; do not duplicate their authority or rewrite their work from Oxide.
