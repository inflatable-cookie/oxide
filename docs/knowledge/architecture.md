# Build storage boundaries

These are the production ownership constraints agreed by Tom on 2026-10-09,
applied to the facilities that remain after the 2026-10-18 retirement
([retirements](retired.toml)). The proposed user workflow lives in
[workflow](contracts/workflow.md); the retained lease library semantics live in
[local lease](contracts/local-lease.md) and the inventory CLI in
[storage inventory](contracts/storage-inventory.md).

## Storage and ownership

Persistent bounded cache slots partition by project/component identity, exact
toolchain, host/target platform, and resolved configuration/profile/features/relevant
environment. One exclusive durable lease covers compile through test and
artifact capture. Unknown crash outcomes, lost replies, age and PID
disappearance do not imply release. Reconciliation must prove safe ownership
settlement before reuse or eviction. The one exception is Tom's 2026-10-18
ruling that lets Oxide's own commands recover a lease whose holder lock is
free and whose Cargo process group is empty
([vision](vision.md#product-workflow-direction),
[workflow](contracts/workflow.md#crash-recovery)); leases without an Oxide
control record never are.

Exact-run final binaries, logs, QA receipts and evidence remain independently
owned and preserved. Cargo intermediates are internal and may contain test
executables; separating output directories alone cannot prove isolation. Cache
reuse never counts as validation success.

Proposed, not implemented and not current truth: a nested `oxide` inside a
leased command may join that outer lease instead of taking a second one, and
a nested command for another workspace may borrow the outer host permit.
Neither changes exclusive slot ownership, closure, accounting or deletion
rules; the closure set only grows, and the borrow substitutes only the host
active-build count. The [nested lease design](contracts/nested-lease.md) owns
the token, validation, ticket, permit and failure rules and is awaiting
independent design review and the operator's design freeze.

Proposed: effective build context (incremental, debug, split-debuginfo) would
be a partition input. Its lifetime and diagnostics rules are in the
[build-context policy](contracts/build-context-policy.md); it is not active.

Prepare only requested components lazily. No eager copying of all targets and
no full-copy fallback. Evict idle slots only, within explicit budgets; occupied
space and actual disk writes must be measured separately.

The read-only [storage inventory](contracts/storage-inventory.md) reports what
occupies space in one explicitly selected target directory. It grants no
ownership or eviction authority and never recommends deletion.

## Current facilities

- The root library keeps the same-user pool, ledger, lease, recovery and
  eviction code `oxide` uses, over budget-only directory and removable-volume
  pools. There is no background storage service or project adapter. The
  standalone lease CLI and the APFS sparse-image backing are retired
  (oxide#102); Nucleus keeps its installed copy until its owner switches.
  UID trust permits a cooperative caller's explicit writer-closed release;
  TTL or holder loss alone does not establish closure.
- The inventory CLI reads one explicitly selected target directory.
- A bounded decoder for Cargo's encoded dep-info (`oxide_storage::dep_info`)
  is kept. The cross-checkout freshness gate works from Git content and does
  not call it; decoding alone establishes no freshness.
- The stable Cargo profile proofs remain opt-in and do not change default
  development or consumer profiles.

## Storage invariants

1. **Soft budgets, no artificial ceiling.** Tom's 2026-10-20 ruling replaced
   the earlier hard-ceiling invariant: a hard cap broke large builds and
   caused churn ([vision](vision.md#product-workflow-direction)). A pool's
   budget is a soft admission figure with whole-idle-slot eviction; nothing
   stops a running build for passing it. Real free-space safety comes from
   the host-reserve watchdog and the filesystem itself; a registered
   removable volume's own capacity is its only hard limit. No pool claims a
   hard bound.
2. **Exclusive use.** A lease names one owned slot generation. A second claim
   cannot use it while ownership is active or uncertain.
3. **Conservative recovery.** Unknown or uncertain writer state preserves bytes
   and accounting. Age, a missing reply, or a PID observation is not release
   evidence. Reclaim requires the proof defined by the lease contract.
4. **Finite capacity.** Admission respects the soft budget and refuses when
   only held slots remain or capacity or ownership cannot be established. Capacity pressure
   produces an actionable refusal and inventory; never delete active or
   uncertain storage or consumer-retained evidence to make room.
5. **Private final outputs.** Reusing Cargo intermediates must not share
   `CARGO_TARGET_DIR`, test outputs or cleanup scope between runs. Requested
   tests still execute and their results remain bound to their run.
6. **Freshness before reuse.** Cargo remains responsible for unit invalidation.
   Oxide must establish compatible inputs and path identity before reuse; it
   must not rewrite Cargo fingerprints or infer a hit from file counts.

Tom's ruling, 2026-10-09: treat 20 GiB host free space as a monitored
start/abort threshold, not a guarantee against other host writers. Refuse a
start when measured free space is below the threshold or unknown; monitor
during execution and abort/report on a detected breach or unavailable
measurement. This ruling does not authorize volume creation or change
uncertain lease release. If write attribution is unavailable, record it as
unavailable; occupied-space deltas are not disk-write counts or SSD lifetime.

## Cargo boundary

Cargo's `build.build-dir` / `CARGO_BUILD_BUILD_DIR` is stable starting in Rust
1.91 and is separate from `target-dir`. It is the smallest useful mechanism:
direct Cargo to an Oxide-owned intermediate directory and let Cargo retain its
own unit invalidation. It supplies no ownership, lease, budget or run-evidence
semantics, and test binaries land in the build directory. Do not blindly adopt
sccache: Acow's measured trial rejected it. Oxide is not a second Cargo
fingerprint engine, eager directory copier or full-copy fallback.

Retire or replace the redirection only when a supported Cargo release supplies
the needed cross-workspace unit reuse and lifecycle behavior for the selected
workflows, and controlled tests show the same ownership, path correctness,
capacity and evidence guarantees. Reassess at each supported Cargo upgrade;
roadmap acceptance alone is not a trigger.

Nucleus Contract 002 prohibits mutable target sharing; Frozen Contract 010
remains unchanged. Queue and Nucleus own task admission, scheduling, host
execution, source and run identities, settlement, retention and artifact
capture. Effigy remains language-neutral. Oxide's command front door is an
explicit opt-in design; local lease use does not install a service, create a
default pool, modify a consumer or inspect live project state.

Cross-worktree source freshness, compiler path embedding, process containment
and capacity remain proof obligations. The retained findings, including the
narrow reviewed #064 ordinary-Cargo cross-path result, are in
[qualification findings](qualification-readiness.md). No general cross-path
correctness, real-project benefit or production capacity policy is
established.

## Existing facilities and source context

These sources inform qualification; they are not Oxide implementation or portability proof:

- Acow [QA speed](../../../acowtancy/docs/knowledge/operations/267-qa-speed.md) and [Rust target seeder](../../../acowtancy/scripts/tasks/rust-target-seed.ts): eager Farmyard/Desktop/Silo clone-copy preparation with full-copy fallback. Desktop storage is unnecessary for root QA. This approach is not the proposed Oxide policy.
- Queue [fresh sibling checkouts](../../../paseo-northstar-queue/docs/knowledge/specs/028-fresh-sibling-checkouts.md) defers sibling build-cache sharing. Shared CARGO_HOME download sources and [guarded output cleanup](../../../paseo-northstar-queue/server/worktree-cleanup.ts) are potential integration context, not slot managers.
- Nucleus [core requirements](../../../nucleus/docs/knowledge/contracts/002-core-requirements.md) owns task isolation and build-write requirements.
- Effigy task-result fingerprint caches and container cache lifecycle are not a central Rust slot facility.

Existing Nucleus integration preparation, backup root locking/crash reconciliation, retention cleanup, managed Git worktrees and smaller debug artifacts have their own owners. Consult their Queue records when authorized coordination is needed; do not duplicate their authority or rewrite their work from Oxide.
