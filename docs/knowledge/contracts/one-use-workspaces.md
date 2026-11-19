# One-use workspaces

This contract owns disposable temporary checkouts: how a workspace is recognised
as one-use, how its final outputs stay local, how its slots stay apart from normal
slots, and what each part does today. Source owners are
`src/bin/oxide/one_use.rs` (classification), `src/bin/oxide/outputs.rs` (the
local-output guard), `src/bin/oxide/store.rs` (slot class and front-door
membership) and `src/lease/` (library admission types).

**Status: dormant.** Part of the feature is implemented, but no shipped binary
activates it. Section 15 gives the status of each slice. Nothing here authorizes
activation, installation or a consumer change.

## 1. Outcome and non-goals

A one-use workspace is a canonical Cargo workspace inside a temporary root, or one
that opts in. It gets a real warm slot in its own bounded slice and never evicts a
normal slot. Its final outputs stay real local directories, so it leaves no owned
root, receipt or link.

Non-goals: no daemon, scheduler or cache cleanup; no change to the slot key,
checkout identity or ledger format; no byte-saving claim; no containment of
arbitrary tool writes; no relabel, relink or deletion of existing receipts, links,
roots or slots.

## 2. Classification and the two predicates

`one_use::classify` runs once per command, after pool selection and before any
output effect. It is effect-free and never reads a receipt, slot, payload, child
temporary directory or the current directory.

- `one_use` (membership or opt-in) selects the admission class and its slice.
- `guarded` (membership or `one_use`) selects the local-output guard.

Membership alone implies the guard, so a forced `normal` class cannot disable it.

## 3. Workspace classification

### 3.1 What is classified

Only the canonical workspace root from `locate_workspace`. A workspace that does
not resolve stays normal, and its existing error is kept.

### 3.2 Candidate roots

Four raw-byte sources, in order: literal `/private/tmp`, literal
`/private/var/folders` (the canonical form of the macOS default `TMPDIR`), the
inherited `TMPDIR`, and the inherited `OXIDE_TASK_SCRATCH_ROOT`. `TMP` and `TEMP`
are not candidates.

### 3.3 Canonicalization and dropping

A candidate must be an absolute path that resolves to an existing directory.
Accepted roots are deduplicated. Each dropped candidate gets one bounded escaped
note. A candidate is dropped when it is relative, empty, unresolvable or not a
directory; the filesystem root or a volume root (`/Volumes/<name>`); the account
home; or a registered pool payload, inside one, or an ancestor of one.

The removable mount point is not a payload, so a scratch root on the same volume
but outside the payload stays accepted. If a registration cannot be read, every
candidate is dropped. Unknown facts never justify a guessed safe root.

### 3.4 Membership

A workspace is a member only as a strict component descendant of an accepted root.
String prefixes do not count (`/private/tmpfoo` is not under `/private/tmp`), and a
root is never a member of itself.

### 3.5 Opt-in

Only the exact value `OXIDE_ONE_USE_WORKSPACE=1` opts in. Any other value is
ignored with one note. It is never an opt-out.

### 3.6 Class and guard

See section 2. The forced-class control that separates the two predicates belongs
to the unshipped proof slice (section 11).

### 3.7 Order and scope

`classify` runs before `auto_adopt`, reservation, binding and admission. It is not
recomputed for child temporary directories, so a one-use workspace stays one-use
with a pool-resident `TMPDIR`. Classification does not choose the pool or profile,
decide whether a build may run, or require a receipt.

## 4. Guarded local outputs

### 4.1 Precheck, then authoritative revalidation

The guard has two parts, and only the second is authoritative. A **fast precheck**
with no effect may refuse early but never authorizes a command. An
**authoritative revalidation**, under the verb's own custody and before any effect,
re-reads the receipt, the pending migration journal and the relevant components.

| Verb | Custody | Revalidation point |
| --- | --- | --- |
| `outputs adopt` | none | at the precheck; it always refuses, and receipt state picks only the text |
| `outputs detach`, `cancel-reservation` | checkout binding mutex, then a `Barrier`-mode clean barrier | under the binding mutex, before any clean mark, id, `waiters/` file or state save |
| leased command binding | checkout binding mutex | inside the mutex, before replay |
| automatic first conversion | none | skipped entirely for a guarded workspace |

The barrier is tried with bounded non-blocking attempts, releasing the binding mutex
between them, because a blocking acquisition would invert the shipped lock order.
On budget expiry the verb refuses with `BARRIER_BUSY`. In `Barrier` mode a live busy
fact waits for a holder that can progress, a dead fact (a free lock with no live
group) refuses with a bounded diagnostic that an ordinary command clears, and an
unknown fact refuses.

Verb-level refusals allocate no id, create no mark or waiter lock, and save no
state. A whole command that reaches binding can still advance `next_id` through its
own ticket and lease, so no whole-command no-change claim is made. The guard does
not contain writes made after the check or outside the declared components, and it
does not verify a real local directory.

### 4.2 Receipt and path states

| Receipt | Live components | Result |
| --- | --- | --- |
| absent | absent or real directories | runs locally |
| absent, or `Detached` with an inserted link | any link or non-directory | refuses, naming the path |
| `Detached` | absent or real directories | runs locally; historical roots are not re-selected |
| `Committed`, any member shape | any | refuses; names `oxide outputs detach` |
| `Prepared`, or a pending migration journal | any | refuses with the 4.4 diagnostic |
| malformed, unsupported, mismatched, `Pruning`, unsettled prune intent | any | refuses through the shipped validation |
| `Pruned` | any | treated as unconverted |

No receipt or link is deleted to make one-use work. Only the existing
ownership-checked detach may turn an owned link back into a real directory.

### 4.3 Components inspected

The guard uses `symlink_metadata`, reaching each path through its parent chain so an
intermediate link is never followed. It inspects `target`, the triple directory when
modelled, and each profile member the command's layout declares. Each must be absent
or a real directory.

### 4.4 Prepared diagnostic

Every site that would replay a `Prepared` transition refuses with this line, before
any sibling, marker or root read:

```text
prepared output transition unresolved; this guarded workspace cannot replay it; nothing changed
```

No adopt or detach advice is attached. `oxide clean` keeps its `Prepared` refusal
with the same text.

### 4.5 Adopt, detach, clean, plan and status

These rules are dormant: none of them print in a shipped binary. `adopt` refuses on a
guarded workspace, with the 4.4 text for `Prepared` and a general refusal otherwise.
`detach` stays safe: for a `Committed` placement it turns owned links back into real
directories, moves no root and deletes nothing. `plan` and `status` print one bounded
line that outputs are local, and never recommend adopt.

### 4.6 What the guard does not claim

Concurrent commands in one checkout stay under the shipped binding and clean rules.
The guard does not make `target` exclusive.

## 5. Slot class in Oxide state

### 5.1 Record field and fence

`Record` gains `class` (`normal` or `one_use`) and `class_reason`. Both are omitted at
their defaults, so a normal record is byte-identical to a legacy one. A class-bearing
record is written only under the one-use fence, `one_use_workspaces:
"oxide.one-use/1"`, which is written durably before the first class-bearing record.
Version 6 requires the fence. Versions 7 and later may omit it unless class-bearing
records exist. A document below version 6 with a `class` or `class_reason` key of any
value is refused before the typed parse, so an older binary never meets an unknown
field.

### 5.2 Class is fixed and class-matched

Class is written at creation and never relabelled. Record matching requires the same
class, so a class change is a distinct record with its own slot. Class is never
inferred from a name, a folder, a profile label or a payload shape. The
freshness gate's cross-checkout share takes only an idle slot of the same class.

### 5.3 Project cap counts normal slots only

The project cap (`PROJECT_SLOT_CAP`, 12) counts and selects normal records only. A
one-use slot never uses a normal slot of the cap, and deleting one never relieves it.

### 5.4 Sharing

Covered in 5.2.

### 5.5 Front-door membership and evictable split

Under the pool's admission lock, `class_membership` derives `members` (ledger slots
whose project is a one-use record's affinity directory) and `unknown` (ledger slots
no record owns). `evictable_members` then derives the subset that passes the
checklist.

### 5.5.1 The evictable checklist

Every item is required, and any doubt means protected.

1. the owning record is `Idle`, with no holder, lease, pending recovery, retired or
   lost mark, and no recorded process group;
2. the record lock is free (a non-blocking probe; `Missing` is not free);
3. no other active record in this pool for the checkout, and no open `lost.unclosed`
   entry for this boot;
4. the ledger row is idle, unleased, and matches the member pair;
5. no donor pin covers the slot and no `oxide clean` is recorded for the checkout.

A ledger-idle row is not writer closure by itself, because the ledger's idle write
and the record's idle write are separate atomic steps.

### 5.5.2 Custody direction

Only the slot's own pool's facts are used. The removable sibling's state is read
without its lock, so a cross-pool fact is never a premise for eviction.

### 5.6 Class-unknown rows

An ownerless ledger slot is never a one-use victim and is not counted as slice bytes.
A pool with one reports a lower bound. A one-use admission in that state refuses with
`unknown_accounting`; a normal admission continues.

### 5.7 Legacy records

A record written before this feature has no class and reads as normal. A later
temporary-workspace command takes a distinct one-use record. Nothing is relabelled or
deleted.

## 6. Library admission

### 6.1 Request types and bounds

`LeaseRequest` carries `class`, `victims: VictimPolicy` and `slice:
Option<OneUseSlice>` (`src/lease/slice.rs`). `VictimPolicy::Any` is the default and
today's behaviour. `Prefer` is for a normal admission: evictable members first, then
the ordinary order with every one-use pair removed. `Only` is for a one-use
admission: evictable members only. `OneUseSlice` carries the member set,
`max_slots` (`ONE_USE_MAX_SLOTS`, 16), `max_bytes` (`slice_byte_limit`:
`min(admission budget / 10, 32 GiB)`, at least 1) and the count of unknown rows.

### 6.2 Membership binding

Specified: the library verifies each `(slot_id, incarnation)` pair against the ledger
under the pool lock, drops an unverified pair without replacing it, and treats only
the evictable subset as a victim source. **Not implemented.**

### 6.3 Restore before admission

Specified: for a slice-bearing admission, measure the member union, then reclaim the
oldest idle evictable members while a count or byte bound is exceeded. A new one-use
slot needs one free place; a warm reuse of a member is admissible at 16. **Not
implemented.** `restore` in `src/lease/pool.rs` is a stub that returns `NotNeeded`
and reads neither the members nor the bounds.

### 6.4 Member union measurement

Specified: the unique allocated bytes of the member slots, counting a cross-member
hard link once. **Partly present:** `union_directory` in `src/inventory.rs` does this
with a shared `(device, inode)` set, and admission does not call it.

### 6.5 Bounds

The slice is an admission bound, not an OS ceiling or a runtime limit. The numeric
bounds are in 6.1.

### 6.6 Warm reuse and new slots

Normal warm reuse does no restore and no measurement. A normal new slot uses `Prefer`,
and an overage is reported while the admission continues. A one-use warm reuse needs
a verified member as its candidate. A one-use new slot uses `Only` and refuses or
waits per 6.9.

### 6.7 Pressure paths

The victim sets are disjoint. `Prefer` never selects a one-use pair, and `Only` never
selects a normal slot, an orphan or a protected member. A safety failure on a selected
victim stops the decision, and the next candidate is never tried. **Not implemented:**
no pressure path in `pool.rs` reads `VictimPolicy` or `request.class`, so the library
applies ordinary eviction whatever the front door passes.

### 6.8 Normal admission continues

A normal new-slot admission that cannot bring the slice within bound reports an
overage and continues. It never fails only because the slice is over.

### 6.9 One-use full

A one-use admission refuses rather than waiting indefinitely. A lower bound with
unknown rows refuses with `unknown_accounting`. A full slice with a progress-capable
member holder takes the bounded wait. Any other full or unmeasured slice refuses with
`LeaseError::OneUseBudget`, which the store maps but the library does not yet produce.

## 7. Crash, closure and recovery

Recovery of `Acquiring` or `Spawning` records keeps the shipped rules and never
changes `class`. Release does not write `class`, and no code path converts a one-use
slot to normal or the reverse. A crash between the ledger's idle write and the
record's idle write is handled by the shipped settle rules, and the evictable checklist
keeps a disagreeing slot protected until the record settles.

## 8. Cold-slot seeding (oxide#137)

Cold-slot seeding is shipped separately (see [cold-slot seeding](cold-slot-seeding.md)).
Its one-use integration is not implemented.

### 8.2 Requirements, not yet met

Guarded outputs and the one-use class do not disable seeding, and seeding never
disables one-use protection. A donor pinned for a seed must stay visible to the slice.

### 8.3 Slice accounting with seed bytes, not yet met

Seed headroom is measured fresh inside the seed transaction. A known shortfall skips
seeding and builds cold in the same slot. An unknown measurement refuses before the
first clone and never becomes a cold build.

## 9. Child scratch

### 9.1 Classification precedes the child environment

Child temporary directories are chosen after the lease and are never a classification
input. A one-use slot's own `scratch` is an ordinary owned entry of the slot, and a
held slot is never a deletion candidate.

## 10. Persistence and visibility

### 10.1 Control state

The one-use fence is `oxide.one-use/1`, at state version 6. That number is shared
with later fences (see 5.1). Unknown class values, unknown fences, and a version-6
document without the fence are refused before use.

### 10.2 Unchanged formats

The ledger, capacity-policy, receipt, marker, plan and registration formats are
unchanged, and no ledger field carries a class.

Open item: a removable admission reads its registration from the removable store's own
control directory, where the registration does not live. Its reserve branch is
therefore not wired and the reserve-prune path does not run. Wiring it would add new
deletion behaviour, so it is returned to the owner. Taken from source comments and not
re-verified here.

### 10.3 Capability probe, installer and removable legacy fence

Not implemented. `__oxide-build-capabilities` reports no `proof_controls` field, no
installer refusal exists, and no legacy fence document is written at the removable
pool root.

### 10.5 Visibility

Implemented, dormant: the plan and status lines in 4.5, the `one_use` status label on a
one-use slot (`main.rs`), and the one-use admission note. Nothing claims a saving or
free space.

## 11. Proof controls

### 11.1 Feature and readers

Not implemented. There is no `proof-controls` Cargo feature, and neither
`OXIDE_TEST_WORKSPACE_CLASS` nor `OXIDE_TEST_TEMP_ROOTS` has a reader. Production
classification is gated only by the thread-local `one_use::is_active()` switch, which
is `false` in every binary and is set only by unit tests. It has no production
activation path.

## 12. Limits

- Dormant. Activation needs the library work in 6.2, 6.3 and 6.7 plus an approved brief.
- A committed or prepared receipt refuses, and there is no automatic repair.
- An unreadable registration drops the candidate root. The default is always normal.
- A full slice of protected members waits only for a progress-capable holder and
  otherwise refuses. A normal slot is never taken.
- A class-unknown orphan blocks one-use admission until it is removed.
- Byte figures are unmeasured, and APFS clone sharing is not measured, so no saving is
  claimed.

## 13. Refusal summary

| Situation | Result |
| --- | --- |
| guarded: link or non-directory on a declared component, or `Committed` receipt | refuses; nothing changed; `Committed` names detach |
| guarded: `Prepared` at adopt, detach, cancel-reservation, binding or clean | refuses with the 4.4 text before any effect |
| guarded: dead busy fact in the barrier | bounded refusal; the dead lease or mark is left as found |
| version below 6 carrying `class`, or version 6 without the fence | refuses before use |
| one-use admission with a class-unknown row | refuses with `unknown_accounting` |

## 15. Slices and status

| Slice | Content | Status |
| --- | --- | --- |
| S1 classification and guard | `one_use.rs`, the guard in `outputs.rs`, the `Barrier` mode and the `Prepared` diagnostic | implemented, dormant |
| S2 record class and front-door scoping | record fields, the fence and presence decode, class-matched lookup, project-cap and sharing filters, membership and the evictable checklist | implemented, dormant |
| S3 library admission | request fields, `VictimPolicy`, `OneUseSlice`, `MemberSet`, slice bounds | types present; restore, victim application and member verification not implemented |
| S4 proof controls | `proof-controls` feature, the two readers, capability field, installer refusal | not implemented |
| S5 seeding interaction | seed headroom and donor-pin visibility | not implemented; depends on the seeding interface work |

Each remaining slice needs its own approved brief and validation. Nothing here grants
activation or installation.
