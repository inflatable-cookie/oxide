# Normal warm-reuse admission (proposed)

**Design only; unimplemented.** Tom's decision
`e3dccedb-0046-4003-a9a0-f4d44517cc44` v2 (2026-11-05), “Design warm reuse
without a full scan,” authorizes this direction and independent design review.
The [ruling recorded in vision at `c4def456`](https://github.com/inflatable-cookie/oxide/blob/c4def456/docs/knowledge/vision.md)
preserves root identity, ownership, leases, writer custody and backing safeguards;
allocation, eviction and OneUse slice rules stay outside this change.
Implementation, validation, installation and QA require later scoped authority.

## Recommendation and policy boundary

Admit a compatible **Normal** idle slot from its verified root and exclusive
lease state, without recursively measuring its contents. Apply the same rule
to `directory-budget-v1` internal and `apfs-volume-v1` removable pools. Report
**slot size unknown** for that acquisition.

This deliberately removes a descendant-completeness precondition. It is a
policy change, not a demonstrated redundant-syscall optimization. The historical
Effigy refusal visited 61,624 entries in 60,006 ms during matching reuse; its
cause remains unknown. A scan's size does not decide capacity on this branch,
but the scan also detects some descendant faults. Those facts need separate
treatment below.

The proposed contract is a cooperative, root-bound lease, **not descendant
filesystem containment for Cargo**. A build may encounter an existing symlink
or nested mount without Oxide detecting it first. Root validation does not
prove otherwise. The current inventory is also not a write fence: its
pathname samples cannot exclude transient replacement, and a completed scan
does not establish lasting immutability. This proposal grants no ownership of
another tree, mount or writer. It must not be described as a sandbox.

## Source baseline and consumers

Locations below refer to executable baseline
`e8c08f6d9e223d94d3c44486907aceb0226ba715`; no executable behavior changes here.

| Source location | Current behavior and proposed boundary |
| --- | --- |
| [`Pool::lease_inner`, pool.rs:1834–1990](../../../src/lease/pool.rs) | Checks holder exclusion and resolves membership/victims. OneUse restores its slice before reuse. Normal candidate reuse returns before new-slot `allocate` and pressure decisions. |
| [`Pool::reuse_idle`, pool.rs:2005](../../../src/lease/pool.rs) | Validates scope, requires the target inventory complete, then checks policy/rank headroom and takes the slot. Only Normal should bypass its walk. |
| [`Pool::validate_idle_slot_scope` / `validate_slot_tree`, pool.rs:3579–3640](../../../src/lease/pool.rs) | Checks generation, idle state, project/profile, canonical owned root and parent, and device. Returns root device/inode; reuse currently discards that identity. |
| [`ReuseMeasurement` / `Leased`, mod.rs:696–709](../../../src/lease/mod.rs) | Complete acquisition observation: slot, incarnation, directory and unique allocated bytes. `Option::None` already represents no measurement. Keep the type for measured routes. |
| [`Store::attempt`, store.rs:4644–4767](../../../src/bin/oxide/store.rs) | Passes the measurement into `Lease.measured` and, only when present, writes the diagnostic `LeaseReuse` sidecar. Neither publication nor its failure decides admission. |
| [`run_invocation_inner` / `print_summary`, main.rs:1033 / 1723](../../../src/bin/oxide/main.rs) | Carries that measurement to the result line; warm plus `None` already renders “slot size unknown.” No summary walk. |
| [`take_idle`, pool.rs:2457; `measure_for_release`, :3424](../../../src/lease/pool.rs) | Taking clears `SlotRecord.recorded`; `release_or_recover` (:2636) conditionally publishes a complete closed observation. Preserve both transitions. |
| [`recorded_total` / `slot_figure`, pool.rs:3057 / 3103](../../../src/lease/pool.rs) | Ledger idle observations feed pool pressure, with existing generation checks and fallback walks. `largest_recorded_slot` (:3214) / `conservative_charge` (:3230) also use them for incomplete charges; `hold_slot_tree_with_phase` (:5129) carries their charge into holding until deletion settles. These remain separate from Normal reuse. |
| [`snapshot_of`, pool.rs:4094; `ledger_slot`, :5597](../../../src/lease/pool.rs) | Export recorded totals, coverage and per-generation record facts for passive library/status consumers; they do not measure or grant. |
| [`Store::report`, store.rs:3857–3968; `recorded_size`, :6788](../../../src/bin/oxide/store.rs) | Default status prefers a current ledger record, otherwise the diagnostic sidecar matched by store/slot/incarnation/directory, labelled with source and age. Explicit measured status remains a bounded audit. |

## Required before Cargo writes

Keep the existing control/admission locks, freshness and compatible-input gates,
and all [writer-closure rules](local-lease.md). An idle ledger row must name the
requested project/profile and exact slot/incarnation, with no active lease.
Held, uncertain, incompatible, quarantined and unresolved generations gain no
new reuse route; same-holder replay keeps its existing semantics.

Retain `Pool::open_inner` (pool.rs:1571), `open_budget_payload` (:4203),
`open_volume_payload` (:4245), `require_slot_parent` (:4670) and
`canonical_existing_dir` (mod.rs:895): canonical real directories, caller UID,
owner-only mode, allowed recorded path/layout, valid ledger/config/fences and
the expected backing device. Internal payload stays on its pool device;
removable payload keeps its registered mount/UUID binding and descriptor/path
agreement through `volume::Binding` (volume.rs:281, :391, :463).

Replace the removed walk's **root** boundary samples with bounded metadata
rechecks, not a replacement traversal. Sample the slot and its validated parent
device/inode at scope validation; revalidate their path, kind, UID, mode,
device and identity immediately before the durable `Idle → Held` commit under
the same pool lock. A missing, unreadable, replaced or mismatched sample refuses
before grant. Keep policy/rank exhaustion refusals and persist the exclusive
lease before returning it. These are point-in-time checks, not a fence against
a same-UID actor changing paths after the last sample. There is no persistent
historical root-inode field today; do not claim to detect a valid replacement
that predates all admission samples, or introduce a ledger schema for it here.

Retain front-door backing/reserve checks before child launch, the host-reserve
and removable-removal watchdogs, owned Cargo-group closure, flush and release
ordering. A skipped size walk must skip none of these gates. The watchdog
observes the selected backing; it does not protect a different nested mount.

## Descendant facts and when they matter

The [inventory contract](storage-inventory.md)
owns the current no-follow walk and its limitations.

| Fact currently observed recursively | Proposed Normal grant policy |
| --- | --- |
| Descendant device/mount boundary | No recursive pregrant assertion. An unobserved nested mount can be reached by Cargo. Retain inventory's refusal to descend across devices and deletion's independent device checks; no new authority over that mount's target. |
| Symlink/no-follow semantics | Root and parent symlinks still refuse. Descendant symlinks are not inspected for grant; Cargo retains its ordinary path behavior. Inventory counts links without following them; deletion keeps its own no-follow rules. |
| Missing/unreadable descendant metadata | Does not refuse this grant. Cargo may succeed without accessing it or fail when it does; propagate its actual outcome. Root/parent unreadability still refuses. |
| Descendant directory identity change or concurrent mutation | No pregrant claim about descendant stability. Cargo handles its own reads/invalidation; independent writers still require the existing custody/refusal/recovery rules. An inventory scan cannot establish writer closure. |
| Entry/depth/deadline limits | No walk and no completeness claim on this branch. All measured routes retain their cumulative bounds and partial/refusal semantics. |

This is the safety choice for independent review: accept Normal reuse without
certifying descendant accessibility or device uniformity, within the existing
cooperative Cargo boundary. The [architecture](../architecture.md#storage-invariants)
requires compatible inputs, path identity, exclusive ownership and conservative
recovery; it does not establish a Cargo descendant sandbox. If descendant
device uniformity **before any Cargo write** is required, this recommendation
conflicts with that requirement: finite root checks cannot prove it, and an
earlier complete walk cannot keep it true. That would need a separately
designed enforcement boundary, not a stale size record or an invented fence.

## Narrow implementation and reporting

Separate the two call sites explicitly: keep OneUse's restore, membership,
slice union/budgets and complete target walk; give only the Normal candidate
branch a scope-checked, unmeasured transition. Share the final checked ledger
transition if useful, not a globally weakened `reuse_idle`. Normal new-slot
restore, pressure, reserve/hard floor, victim selection and deletion remain
unchanged. Release measurement still runs and publishes only complete results;
an incomplete release leaves the slot unrecorded as today.

Return `Leased.measured = None` on this Normal path. Do not write a new
`LeaseReuse` observation, perform a reporting walk, substitute a stale ledger
figure or introduce a conservative charge where no capacity decision occurs.
Existing status records retain their observation source/age and generation
matching; they are neither this acquisition's measurement nor new deletion
authority. Partial bytes never become a complete record. Preserve current
accounting/deletion safeguards and unknown-writer handling independently.

## Acceptance and future validation

Use direct correctness cases, not another timing benchmark or actor search:

- On both backends, a compatible Normal idle slot grants without any target
  inventory call, including a large/deep tree and injected unreadable, missing,
  symlinked, cross-device or changing descendants, under the policy above.
- Missing/wrong root, parent, device, UID, mode, registration/UUID, ledger or
  incarnation refuses; replacements between root/parent samples refuse without
  a grant. Held/incompatible generations, recovery uncertainty and rank/fence
  refusals remain unchanged. Test the stated sampling limits honestly.
- OneUse still restores/measures and refuses the same incomplete or over-budget
  cases. Normal new-slot pressure, eviction, conservative charges and release
  publication keep their existing assertions. No new deletion eligibility.
- Result says “slot size unknown,” with no new sidecar measurement or fallback
  scan. Recorded status remains explicitly historical. Reserve refusal,
  watchdog stop/removal, group closure and release ordering remain effective.

Future implementation should be **HIGH complexity, Elevated verification**:
the changed admission permits subsequent writes on removable media. No change
to deletion or writer-closure decisions is proposed. Cover with
`effigy rust:compile`, `rust:compile-release`, `rust:format`, `docs:check` and
a focused Effigy library/front-door regression selector using the ordinary
Oxide-backed test route. Propose a small fixed warm-reuse correctness selector
and its uncounted diagnostic under the unchanged qualified shared guard,
normal heavy admission, 300 s and 512 MiB, followed only when adequate by the
counted author and distinct independent reviewer occurrences. A future brief
must name those selectors and safe backend fixtures; existing removable proof
tooling is not presumed qualified. This design activates none of them.

The published investigation apparatus and failed/inconclusive evidence remain
unchanged. The original adequate cost diagnostic and counted proof are still
missing, with no waiver or PASS. Design acceptance establishes neither product
readiness nor permission to merge that apparatus, install or run Effigy QA.
