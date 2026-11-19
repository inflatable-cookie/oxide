# Barge adoption: Oxide's storage authority

**Lean storage boundary (oxide#314).** `src/storage.rs` provides the pinned
Barge declarations. Pool slots, removable-volume selection, output pins and
pruning all use Barge. Oxide's owned-delete, descriptor census and output-prune
custody/eligibility engines are deleted with their mechanism tests. The
selection/reservation index remains Rust workflow state for receipts and links;
it grants no deletion authority. Nothing here authorizes installation or live
data migration. Tom's 2026-11-17 lean reset replaces the old custody scans,
operation journals, capture/equivalence apparatus and native proof ladders.

The pre-reset design, its capture contract, the shared-file comparison harness
and the qualification slices are deleted with their code. They remain in Git
and Queue history; no evidence, credit or fixture from them is migrated.

## The pinned library

`barge` is a Git dependency pinned by full 40-hex revision in `Cargo.toml` and
`Cargo.lock`; never a branch or tag. The current pin is merged Barge main
`9336617a87ef182460bdccc4a419f3f8d497b245`. The library uses caret dependency
requirements. Only test builds request Barge's `testing` feature.
`scripts/offline_seed.py` derives the exact Git revision from `Cargo.lock`;
a missing revision is an offline failure.

Barge reads no Oxide format and knows nothing about Oxide. Oxide forges no
Barge marker and writes no Barge document except through the library.

## Responsibility map

| Mechanic | Owner |
| --- | --- |
| Volume registration, UUID/mount identity, absence vs a different volume | Barge |
| Declared areas, their folder, absent-volume policy and soft cap | Barge |
| Owned roots, their marker and explicit inventory | Barge |
| Cooperative holds (exclusive/shared, inheritable) | Barge |
| Durable pins | Barge |
| Registered volume free space, reserve and floor | Barge |
| Recorded admission bytes and idle candidate rank | Oxide |
| Eligibility and execution of each eviction | Barge |
| Marker-checked trash, deletion bounds (no symlink, no mount crossing) | Barge |
| Scratch areas and expiry | Barge |
| Cargo front door, command/exit routing | Oxide |
| Slots, lease workflow state, compatibility and freshness | Oxide |
| Recipes and profiles, seeding policy, output member links, `exec` | Oxide |
| Platform attachment facts (a path is a mounted APFS root, writable) | Oxide, through Barge's `Os` seam |

Oxide supplies the platform facts Barge's current seam cannot see. Linux
budget-directory storage observes the kernel filesystem ID through an opened
directory descriptor (`fstatvfs`) and encodes it as a namespaced UUID for
Barge's identity comparison. A missing directory is absent; unreadable or zero
filesystem IDs refuse. The ID names the containing filesystem, independent of
the pool folder. Tests exercise this real directory seam and compile the Linux
production pool-opening regression with `rust:barge-adoption-linux`; cross
compilation is not Linux runtime evidence.

On macOS, a
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

Each output location has a private account under
`<control>/output-storage/<parent-path-sha256>/barge`, with a literal
`oxide/outputs` declaration. This preserves both permanent-drive and
checkout-local root paths, member links and a real `target` directory.
Selections are pinned before link publication; replaced selections are
unpinned only after the receipt's durable rename. Existing receipts and markers
are checked before generic Barge import. A root's stable Barge identity is
resolved independently of receipt selection generation and recorded with the
selection. A recorded missing Barge ID is never re-adopted, even if a stray
contains a copied old Oxide marker.

Registration imports the existing Oxide policy into Barge when the registered
volume is positively present; absence defers first import. An existing Barge
registration must agree with Oxide's UUID/path. For a nonzero reserve below the
default floor, the floor is lowered to that reserve, so it never exceeds the
selected prune target. A zero reserve retains Barge's default floor.

## Holds, pins and prune

- A slot is held with an exclusive `flock` while a command uses it, and a
  seeding donor is held shared beside the recipient's exclusive hold. Output
  roots use shared holds to preserve concurrent command behavior; their durable
  selection pins remain independent of running commands. The hold's descriptor is made inheritable before the real child
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
- Barge's rename into trash is the held eviction's commit point. A reported
  post-rename deletion failure retires workflow membership and surfaces the
  trash failure. A workflow publication failure after the rename reports
  that the root moved. Neither failure gives freed-space credit.
- Deletion never follows a symlink and never descends into another filesystem;
  a trash entry is removed only when its marker still names the area.
  Eviction renames first, so a crash leaves owned trash, which the next prune
  or sweep removes, and a later root never reuses the evicted path.

## Transition from Oxide's existing roots

Oxide's existing roots are not Barge roots: they predate the marker, and Barge
records any folder without a matching marker as a straggler it will not reuse,
evict or delete.

Barge's `Area::adopt_root(folder, label, id)` validates a caller-owned
directory inside a declared area, records its folder in a `barge.root/2`
marker and refuses a symlink, an unsafe directory or a conflicting marker or
ID. Adoption and resolution inspect markers without walking payloads. The
duplicate-ID check still enumerates the area. Preparation processes at most
eight eligible rows per command. It resolves interrupted adoption by
deterministic ID and recorded path, and publishes each completed or refused
row before visiting the next one. Later commands continue from that ledger.
Lease admission, release and status never call adoption.

Preparation adopts idle legacy rows in place when the kernel reports their
recorded Oxide holder PID absent. It creates no durable pin. Opaque or
malformed holder tokens and still-held rows remain unavailable. Retried
adoption preserves any existing Barge pin, including pins published before
the root ID was recorded. The pinned Barge API cannot verify pin provenance
and remove the pin in one operation, so existing pins need explicit
maintenance. A refused row stays unowned, and unrecorded strays are retained.
Every transition test uses fresh temporary fixtures.

New-slot admission reads the Oxide ledger's logical-byte observations and
walks only rows missing a current observation. Count, byte and reserve
pressure can choose a ranked idle slot and call Barge eviction for that
root alone. Barge rechecks the actual hold, pin and marker and handles
trash. Removable admission checks free space through Barge. It only starts reserve
eviction below the reserve and rereads free space after each completed
eviction. An eviction left in trash surfaces an error and earns no freed
space. Warm reuse above the reserve walks no other payload. Barge's own
soft caps use its release size records, with measurements of vanished roots
reported as approximate. Identity and protection views use bytes-free Barge
listings. See the [admission observations](local-lease.md#admission-observations-oxide317)
for the difference between payload walks and Barge's internal marker enumeration.

Selecting a volume, refusing a mismatch, falling back while the drive is away
and reporting the fallback in status are ordinary behaviour and do not wait on
the transition.

## Removed apparatus

Deleted with the reset: the library and binary observation tapes, the capture
wire and limited verdict policy, the prune capture harness, the shared-file
comparison harness and its selectors, the capture transcripts, vectors and
manifest schemas, and the qualification/cooperative-supplier contracts that
only served them. The ordinary shared process supervisor
(`scripts/bounded-test.py`) and ordinary Effigy admission stay; a removed
selector leaves no dangling reference.

The recorder layer itself is gone from the source, not only from the selector
list: `src/tape.rs`, `src/bin/oxide/tape.rs` and
`src/bin/oxide/capture_wire.rs` are removed, and with them every `tape::*` call
site, the test-only errno save/restore pair, the declared path-only authority
observation seam and its forced-failure switch, and the captured-outcome
guards. Production reads, refusals and writes are the ones the product already
ran; only the recording statements and their test doubles were dropped.

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

The required covering route is `effigy rust:barge-adoption`: an ordinary heavy
`oxide test --workspace --all-targets --locked` command, with one fresh private
fixture directory strictly below the assigned task TMPDIR. It uses ordinary
Effigy admission and deadline handling, with no staging/proof supervisor and no
named skips. The Rust harness runs serially so an unrelated parallel test
cannot inherit another fixture's cooperative holds. The old dual-root `rust:test-600` route is not this task's proof.
Prior use of internal and sibling validation roots remains disclosed history,
not authorization or current proof. The shared supervisor is unchanged.

Prune previews name the Barge area, stable root ID and path, and match the
recorded ID in the selection registry. Execution resolves and evicts only that
ID. Matching paths do not substitute for identity. The pinned Barge revalidates
the expected root marker's area, ID, label and folder under its root and area
locks immediately before rename, and checks the expected ID again before
post-eviction deletion. An earlier unlocked resolve does not establish that
authority.

Preview uses the passive Oxide store and looks up existing output declarations;
it never imports roots, reconciles the ledger, sweeps trash or declares an
area. A missing declaration yields no eligible roots. The account is opened
with `Barge::open_read_only_with`, which never creates directories or lock files,
even when the account is absent or concurrently removed. Its areas reject
mutations before filesystem access. No existing-directory precheck substitutes
for that read-only capability.
