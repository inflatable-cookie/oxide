# Local lease library

This contract owns the semantics of the retained local lease library. Oxide
keeps Rust workflow state in `oxide_storage::lease::managed`: slots, compatibility
keys, generations, affinity, ranks, lease tokens and front-door control records.
Barge stores the physical layer beneath it: locations, marked roots, holds, pins,
pressure pruning and marker-checked trash. The [barge adoption contract](barge-adoption.md)
owns that boundary and the transition rules.

The standalone `oxide-storage-lease` CLI and its APFS image backend were retired
in oxide#102. Its installed copy and history are outside this repository.

## Pools and callers

The library supports `directory-budget-v1` and `apfs-volume-v1` pools. Older image
formats refuse before any write. The `oxide` front door supplies Cargo identity,
profile, checkout, budget and registered-volume policy.

Each successful lease returns the Barge root hold. The caller must keep it for the
whole use. An independent replay by the same holder is refused rather than
granted without a hold.

A pool has owner-only control state, its Rust lease ledger and a private Barge
account. Compatible idle reuse keeps both root path and bytes. A new generation,
after retirement or eviction, gets a fresh ID and path. An unrecorded stray at an
old path never becomes its successor.

Pool preparation adopts an idle legacy row only when the kernel confirms its
recorded Oxide holder PID is absent. A live or unverifiable holder keeps the row
unavailable. Adoption creates no pin. A Barge error while resolving or adopting
the row keeps that row out of service permanently and reports the open as
reconciliation-blocked; it no longer refuses every later command.

## Front-door control directory

`oxide-managed` holds the front door's state: record locks, clean barriers,
affinity and seed sidecars. These describe Rust command workflow, not generic
storage ownership. Output receipts describe selected member links. Barge pins
protect the selected roots independently of command lifetime. Newer control
formats keep the existing forward refusal and installed-binary handoff.

## Commands

- **Admission** takes an exclusive Barge hold before returning a slot.
- **Release** may run bounded stale-unit pruning on a warm Cargo layout-v2 slot
  while the caller still holds that exclusive Barge hold and after positive
  writer closure, before taking the pool lock to run the closing measurement
  walk and mark the slot idle.
- **Release measurement** records a bounded measurement of the closing slot and updates the
  Rust lease state before the caller drops its hold. That one walk is bounded well
  below the pool-lock wait, so no other command waits behind it.
- **Recovery** uses cooperative hold availability. Normal process-group exit
  handling is product handling, not a certificate that no arbitrary writer
  remains.
- **Children** inherit the actual root hold on Oxide's launch route. A child that
  closes inherited descriptors, or an unrelated unheld writer, is outside the
  guarantee.
- **Seeding** keeps a shared donor hold beside the recipient's exclusive hold.
  Catalog, compatibility, source verification and clone-failure policy stay
  Oxide's. A failure before recipient writes can skip; a failure after them cannot
  publish a trusted origin.

## Capacity and eviction

Budgets are soft admission figures. New-slot admission uses recorded logical bytes
against the pool budget. Both new-slot and warm admission read removable free space
against Barge's reserve and floor. Below the reserve they evict idle ledger
candidates by ID and reread free space after each eviction. The warm root being
granted is excluded from that candidate set. Low space never silently selects
another volume.

Eviction takes an exclusive non-blocking hold, rechecks the durable pin under the
area lock, and renames into Barge trash. Deletion needs the matching marker,
follows no symlink and crosses no mount. Oxide has no per-entry deletion journal
and no descriptor census.

### Deferred eviction (oxide#324)

Barge's public eviction is one call: it checks the marker, the hold and the pin,
renames into `.trash/` and deletes the tree there. Deleting a large slot tree took
seconds while the pool lock was held, so every other admission waited behind it.
The decision is committed under the lock instead: a bytes-free inventory skips a
held or pinned candidate, one bounded `oxide-managed/idle-evictions.json` record
names the pool, slot generation, Barge root, area folder, root path and the
recorded size (marked approximate when no observation was current), and the ledger
row is dropped and saved. The record is the commit point: the root is out of the
ledger, so no admission can grant or reuse it.

Barge's eviction then runs with the pool and front-door admission locks released,
from the command that decided it or from a later one. It reports a trash copy that
stayed, and the record is kept until Barge's rename commits, so a later pass asks
again. Free-space pressure is the exception: it must observe the bytes a deletion
returns before deciding again, so a reserve eviction stays one inline call. A full
record list also falls back to the inline call rather than grow the file.

A mutating open drops a record whose recorded root path is absent, because Barge's
rename already committed and its trash is swept by the unlocked settlement pass,
even when no pending records remain. It also rolls a committed decision forward
when its `(slot, incarnation, root)` row is still recorded — the state a crash
between the record and its ledger save leaves behind: the row is dropped and
saved, and the record is removed instead when that save fails, so no record ever
names a live row. A ledger save that fails while the decision is being made is
rolled back in place: the row stays recorded and the durable record is removed,
so the settle pass that runs beside the command never evicts a root an admission
can still grant. An unreadable `idle-evictions.json` is quarantined beside itself
and the open continues: no deletion is invented from it, its roots stay as
recorded, and the library's ledger view reports the open as
reconciliation-blocked.

A sweep failure is reported and does not prevent settlement of other recorded
roots. A record whose root is still present stays for a later pass; recovery
never deletes it directly.

The deciding command settles before it prints its summary, with the pool and
admission locks released, inside a bounded exit wait that reports progress as it
runs. A worker still deleting when that bound is reached, or when a signal has
already ended the command, is detached: the durable record and the trash stay,
and the next command resumes them. A signal never waits out the bound.

A held eviction checks the writer generation and exact lease under the pool lock.
It first writes a private, versioned intent bound to the pool, lease, Barge root
and parent identity. A normal refusal clears the intent and leaves the control
files and ledger unchanged; a root another writer still holds or a pin still
protects is reported busy, and a clear that fails is a pre-commit error rather
than a settlement. When the fence must advance, the fence is published
after the rename and before the revised ledger is saved. A settlement failure
keeps the intent for recovery. If Barge moves the root but cannot delete the trash
entry, Oxide settles the intent and reports the deletion failure as post-commit.

On the next locked open or exact retry, Oxide clears an intent whose original root
is still present. It retires the matching ledger row only when the original parent
is verified and both the recorded root ID and its path are absent. It leaves
unrelated missing roots, substituted paths and uncertain custody alone, and never
repeats the Barge eviction. An intent this open cannot prove — a substituted
parent, another volume, an unreadable root — is quarantined rather than allowed to
refuse every lease, release and prepare: its row keeps exactly the recorded
generation and stays unavailable, the intent stays for a later pass, and the open
reports reconciliation-blocked.

### Admission observations (oxide#317)

Oxide times each admission lock hold, including front-door preparation and refused
attempts. A private sidecar keeps the last 32 observations. The writer publishes
after the pool lock is dropped. The build summary names the phase for holds over
two seconds, holds with more than one byte walk, and holds with an unavailable
Barge listing count. Default status reads recent per-pool maxima without taking
the pool lock.

Count one byte walk per root submitted to a sizing traversal, including incomplete
or later-failed walks. Barge's eviction counts reclaimed bytes during deletion,
with no separate sizing walk before rename, so a refusal before the rename (a
hold, a pin, an unreadable marker, a trash that cannot be created) sizes nothing.
A trash sweep likewise reports bytes during deletion.
Above the reserve, the reserve check enumerates no roots. A committed rename whose
trash copy stayed is progress, not a refusal: the caller re-reads free space and
reports the kept tree. Held rows are never walked under the pool lock — their tree
is still being written — so admission charges them the largest current observation
instead of publishing a mid-build figure.

At the slot ceiling, one bytes-free inventory identifies held or pinned candidates
before any candidate is committed, and the oldest eligible row is retired; the
other candidates are tried in rank order without a second inventory. A refusal
reports held and pinned counts, other unavailable rows, pending legacy adoption and
pin reasons. Default status reads bounded pin records at the ledger's known paths
without enumerating directories.

## Warm reuse

A compatible idle slot keeps its generation and is not scanned before each grant.
Barge validates and holds the root. Cargo's and Oxide's freshness checks decide
reuse correctness, and a cache hit is never test success. The admission rule is
owned by [warm-reuse admission](warm-reuse-admission.md).

Under Cargo layout v2, long-lived warm slots prune stale unused unit
directories (`<profile>/build/<pkg>/<16-hex>/`) once per clean release while
holding the exclusive Barge hold (before the pool lock is taken for release).
Pruning never runs without the exclusive hold, never alters `mtime`, never
follows symlinks or crosses mount boundaries, skips `noatime` filesystems, leaves
layout v1 slots and unrecognized entries untouched, and caps each pass at 500 ms,
16,384 units, 50,000 entries, depth 32 and 256 deletions per release (7-day
staleness threshold, tracked per slot generation in
`oxide-managed/units/<slot_id>.json`). See [workflow](workflow.md#stale-unit-pruning-layout-v2).

## Ledger-only views and passive reads

Membership and workflow rank views read the bounded ledger without walking
payloads. They grant no deletion authority.

Status is read-only. Explicit measurement starts its caller deadline before the
passive open and uses it through setup, accounting and the stability reread. A
partial or unreadable measurement reports unknown capacity. Logical and allocated
bytes stay distinct, and a sample is not a physical peak.

## Slot headroom and ranked replacement

Oxide keeps Rust slot and project headroom, and ranks idle ledger rows by least
recent use. It sends one chosen root ID to Barge at a time. A hold or pin refusal
moves on to the next candidate. If no eligible root can make room, admission
reports a capacity refusal.

### Recorded per-slot accounting

A size belongs to the lease generation that observed it. A new grant clears the
old observation. Release samples only the closing root, and new-slot admission
samples only rows without a current record. A complete observation is persisted
once per generation and refreshed at release. An incomplete sample carries a
positive estimate for that admission and stays out of byte-pressure eviction.
Status reads ledger observations without walking. Explicit measurement makes a
fresh bounded observation.

## Limits

This is a cooperative same-user storage library with no daemon and no process
scan. Installation, real-data migration and live-drive qualification are separate
owner actions.
