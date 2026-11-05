# Normal warm-reuse admission

**Implemented source; Elevated runtime correctness proof pending.** Tom's
`e3dccedb-0046-4003-a9a0-f4d44517cc44` v2 approved the design (2026-11-05),
independently reviewed at `cbc57b55`. Decision
`23f2fa22-2a1c-4834-b74f-88ce008b4328` v2 then authorized this Normal-only
implementation and targeted covering. Root identity, ownership, leases,
writer custody and backing safeguards remain authoritative. Native execution,
merge, installation and consumer QA remain separately gated.

## Implemented policy boundary

A compatible **Normal** idle slot is admitted from its verified root and
exclusive lease state, without recursively measuring its contents. The same
rule applies to `directory-budget-v1` internal and `apfs-volume-v1` removable
pools. The result reports
**slot size unknown** for that acquisition.

This deliberately removes a descendant-completeness precondition. It is a
policy change, not a demonstrated redundant-syscall optimization. The historical
Effigy refusal visited 61,624 entries in 60,006 ms during matching reuse; its
cause remains unknown. A scan's size does not decide capacity on this branch,
but the scan also detects some descendant faults. Those facts need separate
treatment below.

The contract is a cooperative, root-bound lease, **not descendant
filesystem containment for Cargo**. A build may encounter an existing symlink
or nested mount without Oxide detecting it first. Root validation does not
prove otherwise. The current inventory is also not a write fence: its
pathname samples cannot exclude transient replacement, and a completed scan
does not establish lasting immutability. This behavior grants no ownership of
another tree, mount or writer. It must not be described as a sandbox.

## Source baseline and consumers

The design was traced against `e8c08f6d`; the implementation separates the
Normal branch as follows. Symbols identify the owning current source.

| Source location | Current behavior |
| --- | --- |
| [`Pool::lease_inner`, `reuse_normal_idle`, `normal_reuse_scope`](../../../src/lease/pool.rs) | Normal candidate returns before new-slot `allocate`/pressure, with two root/parent samples and no inventory. OneUse still calls measured `reuse_idle` after restore. |
| [`validate_idle_slot_scope`, `validate_slot_tree`, `require_slot_parent`](../../../src/lease/pool.rs) | Check generation, idle state, project/profile, canonical owned root/parent, allowed layout and device. |
| [`ReuseMeasurement` / `Leased`](../../../src/lease/mod.rs) | Complete OneUse target observation. Normal acquisition returns `None`; release accounting is independent. |
| [`Store::attempt`, `Lease.measured`](../../../src/bin/oxide/store.rs) | Only a present measurement writes a diagnostic `LeaseReuse` sidecar. Failure to publish does not decide admission. |
| [`run_invocation_inner`, `print_summary`, `leased_size`](../../../src/bin/oxide/main.rs) | Carry the acquisition observation to the result. Warm plus `None` renders “slot size unknown”; no reporting walk. |
| [`take_idle`, `release_or_recover`, `measure_for_release`](../../../src/lease/pool.rs) | Taking clears `SlotRecord.recorded`; release/recovery conditionally publishes a complete closed observation. |
| [`recorded_total`, `slot_figure`, `largest_recorded_slot`, `conservative_charge`, `hold_slot_tree_with_phase`](../../../src/lease/pool.rs) | Recorded idle observations feed pressure, incomplete charges and holding charges with existing safeguards. No new capacity calculation on Normal reuse. |
| [`snapshot_of`, `ledger_slot`](../../../src/lease/pool.rs); [`Store::report`, `recorded_size`](../../../src/bin/oxide/store.rs) | Passive/status consumers show current ledger records or separately sourced historical sidecar observations with age and generation matching. Explicit measured status remains a bounded audit. |

## Required before Cargo writes

Keep the existing control/admission locks, freshness and compatible-input gates,
and all [writer-closure rules](local-lease.md). An idle ledger row must name the
requested project/profile and exact slot/incarnation, with no active lease.
Held, uncertain, incompatible, quarantined and unresolved generations gain no
new reuse route; same-holder replay keeps its existing semantics.

`Pool::open_inner`, `open_budget_payload`, `open_volume_payload`,
`require_slot_parent` (pool.rs) and `canonical_existing_dir` (mod.rs) retain
canonical real directories, caller UID,
owner-only mode, allowed recorded path/layout, valid ledger/config/fences and
the expected backing device. Internal payload stays on its pool device;
removable payload keeps its registered mount/UUID binding and descriptor/path
agreement through `volume::Binding::{open, verify, dir_at}`.

The removed walk's **root** boundary samples are replaced by bounded metadata
rechecks. `normal_reuse_scope` samples slot and parent device/inode, canonical
paths, kind, UID, mode and backing device; `reuse_normal_idle` repeats it and
requires the same identities immediately before `take_idle` under the same
pool lock. Removable binding verification and payload descriptor/path identity
are rechecked as well. A missing, unreadable, replaced or mismatched sample refuses
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

| Fact previously required recursively | Normal grant policy |
| --- | --- |
| Descendant device/mount boundary | No recursive pregrant assertion. An unobserved nested mount can be reached by Cargo. Retain inventory's refusal to descend across devices and deletion's independent device checks; no new authority over that mount's target. |
| Symlink/no-follow semantics | Root and parent symlinks still refuse. Descendant symlinks are not inspected for grant; Cargo retains its ordinary path behavior. Inventory counts links without following them; deletion keeps its own no-follow rules. |
| Missing/unreadable descendant metadata | Does not refuse this grant. Cargo may succeed without accessing it or fail when it does; propagate its actual outcome. Root/parent unreadability still refuses. |
| Descendant directory identity change or concurrent mutation | No pregrant claim about descendant stability. Cargo handles its own reads/invalidation; independent writers still require the existing custody/refusal/recovery rules. An inventory scan cannot establish writer closure. |
| Entry/depth/deadline limits | No walk and no completeness claim on this branch. All measured routes retain their cumulative bounds and partial/refusal semantics. |

The reviewed policy accepts Normal reuse without
certifying descendant accessibility or device uniformity, within the existing
cooperative Cargo boundary. The [architecture](../architecture.md#storage-invariants)
requires compatible inputs, path identity, exclusive ownership and conservative
recovery; it does not establish a Cargo descendant sandbox. A future requirement
for descendant device uniformity **before any Cargo write** would need a
different contract: finite root checks cannot prove it, and an earlier complete
walk cannot keep it true. It would need a separately designed enforcement
boundary.

## Class separation and reporting

The two call sites are separate: OneUse keeps its restore, membership,
slice union/budgets and complete target walk; only the Normal candidate
branch uses `reuse_normal_idle`. The common final `take_idle` transition is
unchanged; measured `reuse_idle` is not globally weakened. Normal new-slot
restore, pressure, reserve/hard floor, victim selection and deletion remain
unchanged. Release measurement still runs and publishes only complete results;
an incomplete release leaves the slot unrecorded as today.

Normal returns `Leased.measured = None` on this Normal path. Do not write a new
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

The implementation is **HIGH complexity, Elevated verification** because
changed admission permits subsequent writes on removable media. Deletion and
writer-closure decisions are unchanged. Covering selectors are
`effigy rust:compile`, `rust:compile-release`, `rust:format`, `rust:warm-reuse`
and `docs:check`. The focused selector runs library/front-door regressions
through ordinary Oxide. Existing unchanged accounting, volume and watchdog
rules remain owned by their contracts; the new tests do not qualify a mount.

`proof:warm-reuse:diagnostic` and `proof:warm-reuse` prepare the exact commands:

```text
python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-warm-reuse.py --diagnostic
python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-warm-reuse.py
```

Both are heavy selectors under the unchanged qualified guard
`fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b`.
The fixed driver runs the ignored library correctness case and a front-door
case through ordinary offline/locked Oxide; it uses fresh evidence fixtures
and scratch temp, with compiler output in ordinary managed storage. Thirty-one
independent library case rows and the front-door receipt cover actual lease
engine/filesystem effects; removable verification and device/UID/kind fault
projections are explicitly synthetic. No image, real account, private HOME,
metadata-actor probe or live pool is used. Guard closure remains a separate
receipt; child success cannot establish it. The driver's nine premises are
listed in its module docstring. Diagnostic mode retains independent failures
and always returns nonzero with `qualified: false`; infrastructure failure
stops dependent work. There is no automatic retry or new supervisor.

Source/readiness review precedes planner activation of one immutable uncounted
diagnostic, then (only when adequate) one author and one distinct independent
reviewer correctness occurrence. None has run at this implementation head.
Historical cost diagnostics/counts remain failed or missing, with no waiver or
PASS, and the last privileged trace's descendant closure remains unproven.
Superseded investigation tooling is removed from the landing diff; its
published commits and evidence remain history. Source implementation alone
permits neither merge, installation nor Effigy QA.
