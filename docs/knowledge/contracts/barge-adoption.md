# Barge adoption: Oxide's storage authority

**Status (2026-11-18): lean reset; adoption in progress under oxide#314.** Tom's
2026-11-17 lean reset replaces the earlier custody scans, operation journal,
capture/equivalence apparatus and native qualification ladders. The Barge
library was reset to one lean core (barge#064, merged as barge#65) and Oxide
links it at a reviewed merged commit. This file is the owning contract for what
Oxide delegates to Barge, what Oxide keeps, and how an existing Oxide
installation moves onto it.

The pre-reset design, its capture contract, the shared-file comparison harness
and the qualification slices are deleted with their code. They remain in Git
and Queue history; no evidence, credit or fixture from them is migrated.

## The pinned library

`barge` is a Git dependency pinned by full 40-hex revision in `Cargo.toml` and
`Cargo.lock`; never a branch or tag. The current pin is the merged lean core
(`a942a1d`). Offline builds seed that locked Git source through
`scripts/offline_seed.py`; a missing candidate is an offline failure, not
permission to resolve a moving ref.

Barge reads no Oxide format and knows nothing about Oxide. Oxide forges no
Barge marker and writes no Barge document except through the library.

## Responsibility map

| Mechanic | Owner |
| --- | --- |
| Volume registration, UUID/mount identity, absence vs a different volume | Barge |
| Declared areas, their folder, absent-volume policy and soft cap | Barge |
| Owned roots, their marker, listing, accounting | Barge |
| Cooperative holds (exclusive/shared, inheritable) | Barge |
| Durable pins | Barge |
| Reserve and floor, least-recently-used idle eviction, soft-cap pressure | Barge |
| Marker-checked trash, deletion bounds (no symlink, no mount crossing) | Barge |
| Scratch areas and expiry | Barge |
| Cargo front door, command/exit routing | Oxide |
| Slots, lease workflow state, compatibility and freshness | Oxide |
| Recipes and profiles, seeding policy, output member links, `exec` | Oxide |
| Platform attachment facts (a path is a mounted APFS root, writable) | Oxide, through Barge's `Os` seam |

Oxide supplies exactly one platform fact Barge's own seam cannot see: a
`/Volumes/<name>` path that is a directory left on the boot disk by an unclean
detach is *absent*, not a different volume. `src/storage.rs` implements
`barge::Os` with that rule; every other path answers its containing
filesystem's identity, which is the fact Barge wants. A drive that comes back
under a different identity is a `Mismatch` and is refused, never used and never
silently re-registered.

## Declarations

One Barge account per Oxide pool keeps the declarations literal and the
account state private to the pool:

- account state: `<pool>/oxide-managed/barge/`;
- volume: the removable drive at its registered mount point with the recorded
  UUID, or for a budget pool the pool directory itself (whose containing
  filesystem identity is read at the mount);
- area `oxide/pool` with its folder overridden to the existing slot parent
  (`oxide-pool/slots` on a drive, `payload/slots` in a pool directory), so
  adoption moves no path;
- soft cap: the pool's budget; reserve and floor: the drive's registered
  reserve and the product floor for a removable pool, and no reserve trigger
  for a budget pool (its soft cap is the admission bound).

The outputs area is `oxide/outputs` with its folder overridden to the existing
`oxide-outputs` directory on the same volume; a checkout-local root stays
checkout-local and is never presented as a Barge root.

## Holds, pins and prune

- A slot or output root is held with an exclusive `flock` while a command uses
  it, and a seeding donor is held shared beside the recipient's exclusive
  hold. The hold's descriptor is made inheritable before the real child
  launch, so the root stays held until the child that keeps the descriptor
  exits.
- Holds are cooperative: a child that closes the descriptor, and a process
  that writes into a root without holding it, are outside this guarantee. The
  docs say so; no process or descriptor census is restored to widen it.
- A selected output root carries a durable pin, independent of any running
  hold. Pruning evicts only an idle, unpinned root: it takes the root's
  exclusive lock without waiting, then rechecks the pin under the area lock
  before renaming the root into `.trash/`. An earlier "idle and unpinned"
  reading never authorises an eviction by itself.
- Deletion never follows a symlink and never descends into another filesystem;
  a trash entry is removed only when its marker still names the area.
  Eviction renames first, so a crash leaves owned trash, which the next prune
  or sweep removes, and a later root never reuses the evicted path.

## Transition from Oxide's existing roots

Oxide's existing roots are not Barge roots: they predate the marker, and Barge
records any folder without a matching marker as a straggler it will not reuse,
evict or delete.

The lean core has no "adopt an existing root" call. Oxide has therefore raised
one concrete, generic API gap with the Barge owner: an `adopt_root` that
validates a caller-owned directory inside a declared area, writes the library's
own marker, and is idempotent for the same identity. Oxide supplies the layout
and identity facts; Barge must not learn any Oxide format. Until that reviewed
API is merged and pinned:

- an existing root is left in place, unowned: nothing is moved, copied,
  relabelled, retired or deleted;
- no Oxide fallback deleter is added, and no marker is forged;
- the transition is exercised only on fresh temporary legacy fixtures,
  including a refused foreign/mismatched root and an idempotent repeat, never
  on a live installation.

Selecting a volume, refusing a mismatch, falling back while the drive is away
and reporting the fallback in status are ordinary behaviour and do not wait on
the transition.

## Removed apparatus

Deleted with the reset: the library and binary observation tapes, the capture
wire and limited verdict policy, the prune capture harness, the shared-file
comparison harness and its selectors, the capture transcripts and vectors, and
the qualification/cooperative-supplier contracts that only served them. The
ordinary shared process supervisor (`scripts/bounded-test.py`) and ordinary
Effigy admission stay; a removed selector leaves no dangling reference.

## Validation

Ordinary bounded unit and integration tests on private temporary directories
with a fake volume identity establish the source change, together with one
independent exact-head review. There is no native or isolated diagnostic
ladder for this adoption, and no real drive, image, VM, install or live data
migration.

Required coverage:

- a compatible idle root is reused with its path and bytes, and a competing
  exclusive hold refuses the second claim;
- a shared donor hold is compatible with the recipient's exclusive hold;
- a selected, pinned root survives prune; an idle unpinned root is evicted by
  the reserve, the floor refuses only when nothing idle is left, and the soft
  cap evicts only its own area's roots;
- an absent volume uses the declared fallback and reports it; a different
  volume at the mount is refused;
- a fallback root stays resolvable, holdable and pinnable after the volume
  returns;
- deletion does not follow a symlink or cross a mount, and a trash entry with
  a foreign marker is kept;
- the real `oxide` child-launch path keeps the root's hold descriptor across
  `fork`/`exec`: while the child holds it a competing eviction refuses even
  after the launcher releases its own copy, and the root becomes eligible
  after the child exits. A fixture root pinned independently of that test
  proves the pin, not the hold.
