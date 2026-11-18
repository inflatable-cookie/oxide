# Local lease library (standalone CLI retired)

Oxide keeps Rust workflow state in `oxide_storage::lease::managed`: slots,
compatibility keys, generations, affinity, ranks, lease tokens and the front-door
control records. Barge owns their storage beneath it: locations, marked roots,
shared/exclusive holds, pins, pressure pruning and marker-checked trash. The
[adoption contract](barge-adoption.md) owns that boundary and transition rules.

The standalone `oxide-storage-lease` CLI and APFS image backend were retired in
oxide#102. Nucleus's installed copy and historical evidence remain untouched;
this source change neither installs a binary nor migrates live data.

## Callable

The library supports `directory-budget-v1` and `apfs-volume-v1` pools. The
unpublished `oxide` front door supplies Cargo identity, profile, checkout,
budget and registered-volume policy. Older image formats refuse before writes.
The library returns the Barge root hold with each successful lease; callers
must retain it for the entire use. Independent replay of the same holder is
refused rather than returning a grant without a hold.

## Pool

A pool has owner-only control state, its Rust lease ledger and a private Barge
account. The `oxide/pool` area preserves the existing payload/slots placement.
Compatible idle reuse retains both root path and bytes. A new generation after
retirement or eviction uses `create_root`, with a fresh ID and path. An
unrecorded stray at an old path never becomes a successor.

Pool preparation adopts eligible legacy rows before admission. An idle row
whose previous holder is absent or positively dead can be adopted and
evicted without a warm grant. A live older holder keeps the row unavailable.
Held legacy rows still need the existing explicit writer-closure check.
A free Barge lock gives no closure evidence for a writer that never held it.

## Oxide front door control directory

`oxide-managed` holds the front door's state, record locks, clean barriers,
affinity and seed sidecars. These describe Rust command workflow, not generic
storage ownership. Output receipts describe selected member links; Barge pins
protect their selected roots independently of command lifetime. Newer control
formats retain the existing forward refusal and installed-binary handoff.

## Commands

Admission takes an exclusive Barge hold before returning a slot. Release
records a bounded measurement of the closing slot and updates Rust lease
state before the caller drops its hold. Recovery uses cooperative
hold availability; normal process-group stop/exit handling remains product
handling and is not an arbitrary-writer custody certificate. Children inherit
the actual root hold on Oxide's launch route. A child closing inherited
descriptors or an unrelated unheld writer is outside this guarantee.

Seeding retains a shared donor hold beside the recipient's exclusive hold.
The generation-bound catalog, compatibility, source verification and clone
failure policy remain Oxide's responsibility. A pre-write failure can skip;
after recipient writes, failure cannot publish a trusted origin.

## Capacity and eviction

Budgets remain soft admission figures. New-slot admission uses recorded
logical bytes against the pool budget. It checks removable free space against
Barge's reserve and floor and rereads free space after each reserve eviction.
Warm reuse checks the floor without evicting another slot. Low space never
silently selects another volume.

Eviction takes an exclusive nonblocking hold and rechecks the durable pin
under the area lock before renaming into Barge trash. Deletion requires the
matching marker, follows no symlink and crosses no mount. Oxide has no
per-entry deletion journal, holding-area deleter or descriptor census.

### Normal warm reuse (oxide#280)

The normal compatible idle path keeps its generation and does not recursively
scan before every grant. Barge validates and holds the root; Cargo and Oxide's
freshness checks still decide reuse correctness. A cache hit is never test
success.

### Ledger-only view for front-door membership (oxide#110, lead `e4c74399`)

Membership and workflow rank views read the bounded ledger without walking
payloads. They grant no independent deletion authority.

### Passive snapshot and measurement (oxide#128)

Status is read-only. Explicit measurement starts its caller deadline before
passive opening, checks it through setup and uses it for accounting and the
stability reread; zero supplies no grace period.
partial or unreadable measurements report unknown capacity. Logical and
allocated bytes remain distinct, and a sample is not a physical peak.

### Slot headroom and ranked idle replacement (oxide#118)

Oxide keeps Rust slot/project headroom and ranks idle ledger rows by least
recent use. It sends one chosen root ID to Barge at a time. A hold or pin
refusal lets it consider the next candidate. If no eligible root can make
room, admission reports a capacity refusal.

### Recorded per-slot accounting (oxide#146: implemented, native held)

Oxide#315 restores this admission strategy over Barge. A size belongs to the lease generation that was observed. A new grant clears
the old observation. Release samples only the closing root, and new-slot
admission samples only rows without a current record. A complete observation
is persisted once per generation and refreshed at release. An incomplete
sample carries a positive estimate for that admission and stays out of
byte-pressure eviction. Status reads ledger observations without walking or
reconciling. Explicit measurement makes a fresh bounded observation.

## Reviewed local handover artifact

The historical oxide#064 handover and installed Nucleus lease CLI are retained
outside this repository. Their original scope is recorded in
[project adoption](project-adoption.md). This adoption changes neither those
copies nor their evidence. Consumer switching remains an owner action.

## Limits

This is a cooperative same-user storage library, with no daemon or process
scan. Ordinary tests on private temporary fixtures and independent exact-head
review establish the source change. Installation, real-data migration and
live-drive qualification remain separate actions.
