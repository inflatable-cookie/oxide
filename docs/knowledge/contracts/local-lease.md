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
unavailable. Adoption creates no pin.

## Front-door control directory

`oxide-managed` holds the front door's state: record locks, clean barriers,
affinity and seed sidecars. These describe Rust command workflow, not generic
storage ownership. Output receipts describe selected member links. Barge pins
protect the selected roots independently of command lifetime. Newer control
formats keep the existing forward refusal and installed-binary handoff.

## Commands

- **Admission** takes an exclusive Barge hold before returning a slot.
- **Release** records a bounded measurement of the closing slot and updates the
  Rust lease state before the caller drops its hold.
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

A held eviction checks the writer generation and exact lease under the pool lock.
It first writes a private, versioned intent bound to the pool, lease, Barge root
and parent identity. A normal refusal clears the intent and leaves the control
files and ledger unchanged. When the fence must advance, the fence is published
after the rename and before the revised ledger is saved. A settlement failure
keeps the intent for recovery. If Barge moves the root but cannot delete the trash
entry, Oxide settles the intent and reports the deletion failure as post-commit.

On the next locked open or exact retry, Oxide clears an intent whose original root
is still present. It retires the matching ledger row only when the original parent
is verified and both the recorded root ID and its path are absent. It leaves
unrelated missing roots, substituted paths and uncertain custody alone, and never
repeats the Barge eviction.

### Admission observations (oxide#317)

Oxide times each admission lock hold, including front-door preparation and refused
attempts. A private sidecar keeps the last 32 observations. The writer publishes
after the pool lock is dropped. The build summary names the phase for holds over
two seconds, holds with more than one byte walk, and holds with an unavailable
Barge listing count. Default status reads recent per-pool maxima without taking
the pool lock.

Count one byte walk per root submitted to a sizing traversal, including incomplete
or later-failed walks. Barge's eviction sizes the root once before rename. A trash
sweep reports reclaimed bytes during deletion, with no separate sizing traversal.
Above the reserve, the reserve check enumerates no roots.

At the slot ceiling, Oxide tries the oldest candidate through Barge first. If that
is protected, one bytes-free inventory identifies other held or pinned candidates
before the rest are tried. A refusal reports held and pinned counts, other
unavailable rows, pending legacy adoption and pin reasons. Default status reads
bounded pin records at the ledger's known paths without enumerating directories.

## Warm reuse

A compatible idle slot keeps its generation and is not scanned before each grant.
Barge validates and holds the root. Cargo's and Oxide's freshness checks decide
reuse correctness, and a cache hit is never test success. The admission rule is
owned by [warm-reuse admission](warm-reuse-admission.md).

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
