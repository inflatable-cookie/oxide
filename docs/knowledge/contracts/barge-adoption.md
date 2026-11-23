# Barge adoption: Oxide's storage authority

This contract owns the split between Barge and Oxide for storage: which library
owns volumes, areas, roots, holds, pins and pruning, and which Oxide keeps for
Cargo, leases and output links. Barge is the storage authority for pool slots,
removable-volume selection, output pins and pruning. Oxide's own owned-delete,
descriptor-census and output-prune engines are removed. The selection and
reservation index remains Rust workflow state for receipts and links, and grants
no deletion authority. The [architecture](../architecture.md#storage-invariants)
storage invariants still apply.

## The pinned library

`barge` is a Git dependency pinned by a full 40-hex revision in `Cargo.toml` and
`Cargo.lock`, never a branch or tag. The current pin is Barge main
`04e52d8742e8f56c8a0e634834700e18500d9d8d`. Only test builds request Barge's
`testing` feature.

Barge reads no Oxide format and knows nothing about Oxide. Oxide writes no Barge
marker and no Barge document except through the library.

## Responsibility map

| Mechanic | Owner |
| --- | --- |
| Volume registration, UUID and mount identity, absence versus a different volume | Barge |
| Declared areas, their folder, absent-volume policy and soft cap | Barge |
| Owned roots, their marker and explicit inventory | Barge |
| Cooperative holds (exclusive and shared, inheritable) | Barge |
| Durable pins | Barge |
| Registered volume free space, reserve and floor | Barge |
| Eligibility and execution of each eviction | Barge |
| Marker-checked trash; no symlink following; no mount crossing | Barge |
| Recorded admission bytes and idle candidate rank | Oxide |
| Cargo front door and command and exit routing | Oxide |
| Slots, lease workflow state, compatibility and freshness | Oxide |
| Recipes, profiles, seeding policy, output member links, `exec` | Oxide |
| Platform attachment facts (mounted APFS root, writable), through Barge's `Os` seam | Oxide |

## Platform facts Oxide supplies

- On macOS, a `/Volumes/<name>` path that is an ordinary directory left by an
  unclean detach is **absent**, not a different volume. Oxide answers for drive
  paths and reports the containing APFS volume's UUID for other paths, so an
  internal or budget pool keeps its recorded identity. A drive that returns under
  a different identity is a mismatch: it is refused, never used and never
  silently re-registered.
- On Linux, budget-directory storage reads the kernel filesystem ID through an
  opened directory descriptor and encodes it as a namespaced UUID. A missing
  directory is absent; unreadable or zero IDs refuse. Cross-compilation is not
  Linux runtime evidence.

## Declarations

Each Oxide pool has one private Barge account under `<pool>/oxide-managed/barge/`.
Its area `oxide/pool` keeps the registered slot parent (`<payload_dir>/slots` on a
drive, where `payload_dir` defaults to `oxide-pool` and may be nested, for example
`builds/oxide/pool/slots`; `payload/slots` in a pool directory), so adoption moves
no path. The soft
cap is the pool budget. A removable pool uses the drive's registered reserve and
the product floor; a budget pool has no reserve trigger, and its soft cap is the
admission bound.

Each output location has its own account under
`<control>/output-storage/<parent-path-sha256>/barge` with a literal
`oxide/outputs` declaration. Selections are pinned before link publication and
unpinned only after the receipt's durable rename. A root's Barge identity is
recorded with its selection; a recorded missing identity is never re-adopted,
even if a stray carries a copied old marker.

Registration imports Oxide's policy into Barge when the volume is positively
present. A nonzero reserve below the default floor lowers the floor to that
reserve, so the floor never exceeds the prune target.

Quiet-window relocation uses `Barge::relocate_area` in each private account.
Barge moves the area folder and records its new location while holding its
area and root locks. Root IDs, markers and pins travel with the folder. Oxide
moves the outputs account to the hash of its new parent. The host account's
class placements and shared volume layout play no part in the move.

## Holds, pins and prune

- A slot is held with an exclusive `flock` while a command uses it. A seeding
  donor is held shared beside the recipient's exclusive hold. Output roots use
  shared holds. The hold descriptor is made inheritable before the real child
  launch, so the root stays held until the child that kept the descriptor exits.
- Holds are cooperative. A child that closes the descriptor, or a process that
  writes into a root without holding it, is outside the guarantee.
- A selected output root carries a durable pin independent of any running hold.
  Pruning evicts only an idle, unpinned root: it takes the root's exclusive lock
  without waiting, rechecks the pin under the area lock, then renames the root
  into `.trash/`.
- The rename into trash is the commit point. A post-rename deletion failure
  retires workflow membership and surfaces the trash failure. A workflow failure
  after the rename reports that the root moved. Neither gives freed-space credit.
  Output prune settles such a root in its registry, reports it as moved to trash
  with deletion failed, and counts it as not freed.
- Deletion never follows a symlink, never descends into another filesystem, and
  removes a trash entry only when its marker still names the area. A crash after
  rename leaves owned trash, which the next prune or sweep removes. A later root
  never reuses an evicted path.

## Transition from existing roots

Oxide's pre-existing roots are not Barge roots. Barge records a folder without a
matching marker as a straggler and never reuses, evicts or deletes it.

- `Area::adopt_root` validates a caller-owned directory inside a declared area,
  writes a `barge.root/2` marker, and refuses a symlink, an unsafe directory, or
  a conflicting marker or ID. Preparation handles at most eight eligible rows per
  command, resolves interrupted adoption by deterministic ID and recorded path,
  and records each row before the next. Lease admission, release and status never
  call adoption.
- Preparation adopts an idle legacy row in place only when the kernel reports its
  recorded holder PID absent. Opaque or malformed holder tokens, and still-held
  rows, stay unavailable. Adoption creates no pin. A refused row stays unowned,
  and unrecorded strays are retained.
- Existing pins are not removed by the pinned API. They need explicit maintenance.

## Capacity and eviction

- New-slot admission reads the Oxide ledger's logical-byte observations and walks
  only rows that lack a current observation. Count, byte and reserve pressure can
  choose a ranked idle slot, and Barge evicts that root alone. Barge rechecks the
  hold, the pin and the marker before trash handling.
- Removable admission checks free space through Barge. It starts reserve eviction
  only below the reserve and rereads free space after each completed eviction.
- An eviction left in trash surfaces an error and earns no freed space.
- Oxide's slot ceiling and byte budget commit the eviction decision under the pool
  lock and let Barge evict afterwards, with the pool and admission locks released
  (oxide#324). Barge still rechecks the hold, the pin and the marker, and the same
  run-then-sweep rules apply. Reserve pressure keeps its inline eviction because it
  needs the bytes a completed deletion returns; see the
  [local lease contract](local-lease.md#deferred-eviction-oxide324).
- Warm reuse above the reserve walks no other payload.
- Oxide's byte walks are counted separately from Barge's internal marker
  enumeration, which is reported as a listing with an unavailable count. Output
  payloads are never measured under the admission lock. The
  [admission observations](local-lease.md#admission-observations-oxide317)
  section owns the measurement rules.

Selecting a volume, refusing a mismatch, falling back while the drive is away
and reporting the fallback in status are ordinary behaviour.

## Removed apparatus

The reset deleted the library and binary observation tapes, the capture wire and
verdict policy, the prune capture harness, the shared-file comparison harness,
its selectors and fixtures, and the qualification and supplier contracts that
served them. The tape modules are gone from source, along with their call sites
and test doubles. Production reads, refusals and writes are unchanged.

## Validation

Ordinary bounded unit and integration tests run on private temporary directories
with a fake volume identity. Validation follows the lean policy in [vision](../vision.md#validation-policy): ordinary tests on private fixtures and an independent exact-head review. There is no native or isolated diagnostic ladder, and no real drive, image, VM, install or live data migration.

- `effigy rust:barge-adoption` runs the workspace suite, serially, with one fresh
  fixture directory strictly below the task TMPDIR. It runs under ordinary Effigy
  admission and has no named skips. The run's directory is removed only after a
  successful run.
- `effigy rust:barge-adoption-linux` compiles all targets for the installed Linux
  target, including the Linux pool-opening regression. It claims no runtime result.

Required coverage: a compatible idle root is reused with its path and bytes, and
a competing exclusive hold refuses the second claim; a shared donor hold is
compatible with the recipient's exclusive hold; a pinned root survives prune, an
idle unpinned root is evicted by reserve, and the soft cap evicts only its own
area's roots; an absent volume uses the declared fallback and reports it, and a
different volume at the mount is refused; a fallback root remains resolvable,
holdable and pinnable after the volume returns; deletion does not follow a
symlink or cross a mount, and a trash entry with a foreign marker is kept; the
real child-launch path keeps the root's hold across `fork` and `exec`.

Prune previews name the area, the stable root ID and the path, and match the
recorded ID. Execution resolves and evicts only that ID; matching paths do not
substitute for identity. Preview uses the passive store and never imports roots,
reconciles the ledger, sweeps trash or declares an area. Its account opens with
`Barge::open_read_only_with`, which never creates directories or lock files.
