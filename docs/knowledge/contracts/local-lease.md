# Local lease library (standalone CLI retired)

**Retired, oxide#102 (Tom, 2026-10-20).** The standalone
`oxide-storage-lease` binary, its `rust:storage-lease-cli` selector and the
`apfs-sparse-image-v1` backend are gone from this repository. The library
(`oxide_storage::lease::managed`) keeps the pool, ledger, leases, release,
recover, status, idle eviction and the `oxide` control directory below, and
opens only `directory-budget-v1` and `apfs-volume-v1` pools; a pool recording
`apfs-sparse-image-v1`, or holding an image or receipt file, refuses with
`backing_mismatch` before anything is written. `LeaseRequest.budget_bytes`
carries the caller's soft budget for one admission. Current pool, budget and
recovery behaviour is owned by
[workflow](workflow.md#budget-only-default-pool-and-user-configuration);
`oxide pool recover-interrupted <slot>` replaced the CLI's explicit `recover`.
The sections on the CLI, image backing, capacity status, exit codes and the
reviewed handover below are history: Nucleus's installed binary and the
reviewed handover copies outside this repository are retained, untouched.

### Retired lease-CLI fixture audit (papercut 17674724, closed deprecated)

Papercut `17674724-b4d7-414d-916f-e53d277f5ee8` asked for the retired
standalone lease CLI fixture's scratch cleanup. Tom approved conditional
deprecation (Supervisor `083047a8`). The original subject is bound to oxide#064
/ PR #64 review `6213212075` at source `ecc185bc`:
`scripts/test-storage-lease-cli.py`'s `scratch_root` (line 32) called an
external `mktemp`, and `main` (line 439) and `prove_usage` (line 135) each
called it. Current main `d246d40` has no such script, no
`rust:storage-lease-cli` selector and no `oxide-storage-lease` binary;
[retirements](../retired.toml) records exactly that script, selector and
binary as retired on 2026-10-20. Both adviser inventories found no execution
route. The retained `src/lease/tests.rs` `Fixture::new` does execute through
`test rust`, but it is a distinct Rust fixture using `std::env::temp_dir`, not
the original repeated-`mktemp` path, and it is not declared retired or fixed.
The suspended `oxide-cli` modes fail before effects; `clean-safety` and
`launch-check` each allocate one fallback root and preserve failure evidence.
Disposition: closed as deprecated with this bounded inventory evidence. No
historical temporary-directory cleanup and no fix claim follows, and no broad
temporary-file refactor or Rust-fixture cleanup is implied.

Former status: unpublished Oxide-only product command. Same-UID callers take exclusive
owned slots on a verified APFS payload volume through a short-lived OS lock and
a durable JSON ledger. There is no resident daemon, HMAC credential, issuer,
callback service, Cargo runner, scheduler or Git manager. A focused pinned
Nucleus `dev` workflow demonstrated cross-worktree registry reuse under
ordinary Cargo and cooperative writer-closed release. The [consumer boundary](project-adoption.md#current-nucleus-integration-boundary)
and [raw-backed evidence](../evidence/oxide-064-cross-path-qualification.json)
record the qualified scope and limits. Consumer integration remains owner-run;
this is not an installed service or a general source-invalidation guarantee.

Publication naming remains [Q-005](../questions.md#q-005--what-names-are-safe-to-publish).
The package stays `publish = false`. The binary name `oxide-storage-lease` is
internal. Capacity semantics were owned by the bounded-storage contract,
retired with the image backend. Consumer adoption remains
[project adoption](project-adoption.md).

## Callable

Library (`oxide_storage::lease`) and one unpublished binary built by the
ordinary workspace:

```sh
oxide-storage-lease --pool <path> init --cap-bytes <n> --image-bytes <n>
oxide-storage-lease --pool <path> lease --project <canonical-root> \
    --workspace <checkout> --profile <dev|full-debug> --holder <id>
oxide-storage-lease --pool <path> release --lease <id>
oxide-storage-lease --pool <path> recover --lease <id>
oxide-storage-lease --pool <path> status [--capacity] [--json] [--max-output-bytes <n>]
```

Covering CLI proof is `effigy rust:storage-lease-cli`: it builds the binary,
exercises usage refusals, then on macOS with a 20 GiB host reserve provisions
one fresh 128 MiB APFS image and runs lease/release/recover/status, capacity
sampling, concurrent acquire, duplicate-holder replay, busy, symlink/foreign
refusal, idle reuse, delayed APFS metadata arrival and mismatched-backing
refusal. Rust lease tests cover injected zero/unknown samples, hardlinks,
metadata separation and byte-for-byte observational behavior. Synthetic
fixtures do not claim native quota behavior. It is not a filesystem-fill
benchmark and not full QA.

## Pool

`--pool` is required. It is the owner-only control directory (mode `0700`,
caller uid). Init creates it empty, or tightens an empty caller-owned directory
to `0700`. A directory whose only occupant is the CLI's leftover `lock` file
from a failed init is still init-able. An existing pool cannot have its cap
raised and cannot be reinitialized. Arbitrary directories are not reinterpreted
as bounded.

Pinned at init and recorded in `config.json`:

| Field | Meaning |
| --- | --- |
| `version` | `1` |
| `backend` | exactly `apfs-sparse-image-v1` |
| `store_id` | 32 lowercase hex chars, never reused |
| `cap_bytes` | declared payload size; must equal `image_bytes` at init; callers cannot raise it |
| `image_bytes` | sparse image size; must equal the declared cap so the volume size is the enforced ceiling |

Control files stay on the host: `config.json`, `ledger.json`, `lock`,
`image.sparseimage`, `receipt.json`. The payload mount is `<pool>/payload`.
Slots live only under `<pool>/payload/slots/<id>/`. Ledger and receipt are
atomic replace plus `sync`. The exclusive `flock` is held only while the CLI
runs. Cargo and tests run after the process exits; a finished CLI is not writer
completion.

Directory-only roots are not a product mode. Tests may inject a private
directory solely to exercise the ledger; that path is not compiled into the
CLI and does not claim an OS hard bound.

The lean pool accepts these exact APFS metadata directories alongside
`slots`: `.fseventsd`, `.Spotlight-V100`, `.Trashes` and `.TemporaryItems`.
Each must be a real directory on the verified payload volume; the operating
system may own it. The pool does not recurse into or inventory these entries,
and every other payload-root entry refuses. Their writes still consume the
APFS volume's enforced capacity.

## Oxide front door control directory

The unpublished [`oxide`](workflow.md#implemented-foundation-core) command
leases through this library. It keeps its own state in `<pool>/oxide-managed/`,
an owner-only real directory. Pool opens accept that one extra entry; a
symlink there refuses like any other. The CLI never reads or writes it, and a
pool without it opens as before. Oxide passes one private directory under it
as each lease's `--project`, so library reuse follows Oxide's records: a
checkout's own slot, or another checkout's idle slot only through the
[freshness gate](workflow.md#cross-checkout-freshness). Its
automatic crash recovery touches only leases whose ID or holder string its
own control record names.

Oxide calls the library's `managed` entry points (`lease`, `release`,
`recover`, `status`, `snapshot`, `measure`, `evict_idle_slot`,
`init_budget_pool`, `init_volume_pool`, `flush_volume`). They behave as
the functions below on an APFS pool and also open the budget-only
`directory-budget-v1` pool ([workflow](workflow.md#directory-budget-v1-as-shipped))
and the removable `apfs-volume-v1` pool ([workflow](workflow.md#removable-build-volume)),
whose config adds a `volume` object (mount point, volume UUID, payload
directory); other configs serialize unchanged.
The CLI and the plain library functions keep refusing both as
`backing_mismatch` (exit 4), so Nucleus and other CLI consumers see no
change. A directory pool's config records `backend: directory-budget-v1`,
the budget as `cap_bytes` and `image_bytes: 0`; an older build refuses it.
`OXIDE_POOL_CAP` chooses the cap of a pool the front door creates
([workflow](workflow.md#implemented-foundation-core)); this CLI's
`init --cap-bytes` and every existing pool's recorded cap are unchanged.

Oxide resolves its host-wide active-build limit once per command from
`OXIDE_MAX_ACTIVE`, `[pool] max_active`, or the approved default. The limit
continues to count active commands across internal and removable pools using
the existing state and admission rules. `oxide status` reports the effective
value and whether it came from the environment, the selected user config, the
physical-core formula or its fallback; see the workflow's
[user configuration](workflow.md#budget-only-default-pool-and-user-configuration).

The library also exports `evict_idle_slot` for Oxide's per-project slot cap
(eight slots per project identity; [workflow](workflow.md#storage-and-space-management)).
It retires one recorded idle slot whole through the same durable eviction path
as capacity eviction, and returns busy for a held slot. The CLI does not
expose it. Ledger v1, receipt v2, JSON and exits are unchanged. An earlier
`oxide-storage-lease` build refuses a pool that has `oxide-managed` as an
unknown entry; use the current build against pools the front door touched.

## Commands

**init** checks cap and image size, and refuses unsupported platforms, before
any host effect. A declared cap that differs from the image size refuses:
a smaller cap on a larger volume is admission only and is not OS-enforced.
It then provisions one sparse APFS image, attaches it at
`payload`, writes the capacity receipt, pins the cap, and returns JSON
`PoolInfo`. The receipt's measured `verified_capacity_bytes` is the effective
OS hard bound; it sits slightly below the declared size by APFS metadata
overhead, and writers cannot exceed the declared ceiling through it. A volume
that measures larger than declared refuses without migration. Receipts that
predate this rule (version 1, or cap not equal to image size) refuse as
mismatched backing. A leftover empty `lock` from a failed init does not prevent retry.

**lease** requires canonical existing project and workspace directories,
`dev` or `full-debug`, and an opaque holder. Project is stable identity;
workspace is the actual checkout and remains observed. A compatible idle slot
(same project and profile) is the reuse candidate, including a different
workspace. Exact duplicate holder+project+profile+workspace while held
reconciles to the same lease id instead of allocating a second lease.
Conflicting reuse of a held holder refuses. A different holder against a held
compatible slot is busy. Identity is `v1.<store>.<slot>.<incarnation>.<seq>`
and is never reused. The lease is committed before paths are returned.

Returned environment:

- `CARGO_BUILD_BUILD_DIR` — slot `build` directory on the payload volume
- `CARGO_TARGET_DIR` — `<workspace>/target` (private final output)

The CLI does not invoke Cargo, force incremental/debug/feature flags, remap
rustc paths, or copy source into the slot.

**release** is the same-UID caller declaring every writer using this lease has
ended. It is not QA success. Replay against an already-idle lease is
access-free and idempotent.

**recover** is the same writer-closed declaration for held occupancy the owner
treats as abandoned or uncertain. The CLI and library never infer abandoned
from TTL, PID, age or holder loss; status reports such occupancy as `held`
until release or recover. Automatic TTL reuse is not implemented. The one
exception is outside this CLI: the `oxide` front door calls `recover` for a
lease it took itself once that command's record lock is free and its Cargo
process group is empty ([workflow crash recovery](workflow.md#crash-recovery)).
A lease taken through this CLI is never recovered that way.

**status** lists finite slots with holder, observed workspace, occupancy,
logical and unique allocated bytes, pool cap, and full/unknown. Coverage is
`complete`, `partial` or `unknown`. Unknown accounting is honest. Each walk
also reports its gap kinds, entries visited, elapsed time and bounds
(`payload_accounting`, and `accounting` per slot; see
[accounting walks](#capacity-and-eviction)). No payload
or secret content is printed. The legacy `full` field compares owned-slot
unique allocated bytes with the declared cap. It retains that documented
meaning and does not establish physical filesystem headroom.

**status --capacity** reports the same ordinary status inside a separate
version-1 JSON envelope (`version`, `status`, `capacity`, `notes`) or concise
human output. The nested legacy `PoolStatus` fields do not change. The capacity
section reports the declared cap and receipt-verified payload ceiling, raw
payload filesystem free and unprivileged available bytes, the pinned sparse
image's logical and allocated file bytes, and host filesystem available bytes
at the pool directory. Byte fields are exact integer byte counts or `null`
with a bounded `unknown_reason`; zero is a measured zero. The source fields
are `statfs f_bfree * f_bsize` and `statfs f_bavail * f_bsize` on the exact
verified payload mount, image `st_size`, image `st_blocks * 512`, and host
`statfs f_bavail * f_bsize`. Unsupported or failed non-payload samples remain
unknown. `coverage` is complete only when each capacity observation is known.

Capacity mode takes the existing pool lock without creating it, verifies the
already-pinned receipt, image identity and mounted APFS root, then samples
metadata only. A missing, replaced, unmounted, symlinked or unsafe backing
refuses; it does not attach an image or sample a replacement filesystem. It
does not reconcile or remove an `Evicting` slot, or write the config, ledger,
receipt or payload. Existing `Evicting` state remains visible and makes the
nested accounting coverage unknown. Held slot accounting remains observational.
Writers may still mutate held payload files while the command samples them, so
the report is not an atomic snapshot.

The payload filesystem free/available sample is not a reservation, build-fit
prediction, attributable write total or guaranteed reclaimable space. Do not
add slot accounting to image allocation or subtract slots from the nominal cap
to infer physical free space. Image `st_blocks` is the host-reported allocation
of that file, not APFS unique extents or snapshot accounting. Private target and
temporary files, control files, OS metadata and snapshots are not combined
into a whole-host cost; total host cost and Finder purgeable space remain
unknown. Capacity reporting grants no storage ownership or cleanup authority.
The envelope notes explicitly retain the legacy meaning of `status.full` and
the limits of sampled capacity and image allocation.
Either capacity observation coverage or nested slot coverage that is partial
or unknown, and any output truncation, returns exit 2.

## Capacity and eviction

Current pools are budget-only (`directory-budget-v1`, `apfs-volume-v1`):
the budget is a soft admission figure with no OS ceiling, and current
budget, watchdog and removable behaviour is owned by
[workflow](workflow.md#budget-only-default-pool-and-user-configuration).
Admission uses this admission's budget (`LeaseRequest.budget_bytes`, else
the recorded one) and the [storage inventory](storage-inventory.md)
unique-allocated figure for owned slots, a `(device, inode)` union so a hard
link counts once; filesystem metadata directories remain outside that slot
accounting. Control/ledger headroom stays outside the payload and is bounded
(`MAX_LEDGER_BYTES` 2 MiB, `MAX_SLOTS` 128). Eviction happens only at lease
time and only for known owned idle whole slots, in the pool's recorded
least-recently-used order (oxide#118, [below](#slot-headroom-and-ranked-idle-replacement-oxide118)). Held occupancy, foreign
entries, symlinked pool/payload/slots/slot roots, unknown accounting and
mismatched backing refuse. During a durably recorded whole-slot deletion, an
owned nested symlink entry may be unlinked as a leaf after its parent and
entry ownership are verified. The target is never followed or removed.
Deletion never touches project or workspace paths. A lost delete is
reconciled only for the recorded evicting slot path.

**Accounting walks (oxide#108).** Every pool accounting walk uses the same
pool-specific bounds: 2,000,000 entries, depth 64 below the walked root and a
60 s cooperative deadline. The whole-payload walk starts at the payload root,
two levels above the slot content directories (`slots` and a project folder,
oxide#104), so it uses depth 66 and slot contents keep 64
([Finder metadata](workflow.md#finder-metadata-in-pools), oxide#105). They are separate from the inventory tool's
defaults, which are unchanged. The callers:

| Walk | Gate |
| --- | --- |
| The matching idle slot's own walk before reuse | Must be complete, or the lease refuses |
| The whole-payload walk before a new slot, and again after eviction | Must be complete, or the lease refuses |
| The whole-payload walk before each idle eviction in the loop | Must be complete, or the lease refuses |
| `status --measure`: the whole payload and each slot | Observational; reports coverage, causes and time. Default `oxide status` walks nothing (oxide#128) |

**How one walk reads (oxide#141).** The walk itself is unchanged in scope,
bounds, policy and refusal classes. Each enumerated regular file or directory
is read immediately from the live directory entry the walk holds, when its
absolute path name is shorter than the platform `PATH_MAX` including its
terminating NUL, and with the former full-path no-follow read when it is not;
directories are still read by path name, so no long-path completeness is
claimed. Every directory that is read is also sampled by path name before its
entries are read and again after they were enumerated, and must still be the
directory observed when it was classified (kind, root device, observed inode).
A mismatch is `unsupported_metadata` (another kind or inode), `mount_boundary`
(another device) or, for a non-root `NotFound`, one `vanished` observation; a
directory that fails its sample is not read, and after enumeration only its
newly queued children are discarded while observed entries and bytes are kept.
`ENOTDIR` and `ELOOP` are never `vanished`. The samples do not bind the
directory `read_dir` opened and are not an atomic snapshot, and the extra two
path-name reads per directory cost real time. This removes some redundant
ancestor path resolution; it is not a measured speed claim and not a guarantee
that a large or cold pool meets the 60 s deadline, so a walk that reaches the
deadline still refuses exactly as before. The inventory's own
[per-entry and boundary-sample rules](storage-inventory.md#per-entry-reads-and-directory-boundary-samples-oxide141)
own the details. Admission walk counts are unchanged.

A walk that reaches a bound, meets an unreadable entry, missing or unusable
metadata, a directory whose path-name identity changed at either of its two
boundary samples, or a mount boundary, or cannot open its root is incomplete.
It refuses with reason `unknown_accounting` (exit 3, unchanged), and the error
names the gap kinds (`entry_limit`, `depth_limit`, `elapsed_deadline`,
`mount_boundary`, `unreadable_entry`, `missing_metadata`,
`unsupported_metadata`, `root_missing`, `root_not_directory`,
`root_unreadable`, `root_symlink`) with counts, entries visited, elapsed
milliseconds and the bounds used. No path or name is printed. An incomplete
walk's bytes are never used for admission and never make a slot an eviction
candidate; `status` shows them as a lower bound. A measured zero stays
distinct from unknown.

**Enumerated paths that vanish (oxide#113).** If a non-root path enumerated by
the same walk returns `NotFound` at deferred `read_dir`, `file_type` or
no-follow metadata, the walk increments a separate `vanished` count rather
than a gap. A walk with only these observations is complete for the names it
could still read, and its bytes may be used by the existing admission and
whole-idle-slot eviction rules. The scan and its result remain a best-effort,
non-atomic path observation; `vanished` does not prove that inode allocation
was physically freed or is reclaimable. Bytes for other paths already
observed, including a link to the same inode, remain counted. A root
`NotFound` is still `root_missing` and refuses even if that root was opened
earlier in the walk. Iterator errors without a path identity, `EACCES`, other
metadata errors, bounds, mount boundaries and all ownership, foreign-entry,
symlink and identity refusals retain their existing categories. `status` and
`unknown_accounting` refusal text show `vanished xN` even alongside a real
gap, and state that allocation reclamation is not inferred. The count is not
part of ledger/config persistence; new readers accept
older scan JSON with the field absent, while older strict readers may reject a
new scan that includes a nonzero count.

Only a slot whose own walk completes can be reused; other
`unknown_accounting` refusals (invalid identities, ledger, ownership or
backing failures, foreign entries) are unchanged.

The bounds are finite, not an arbitrary-size guarantee. The entry bound
covers 128 slots of about 15,600 entries each, not the 60,657-entry
single-project slot reported for Farmyard (2026-10-20); a pool beyond it
refuses until idle slots are evicted or cleaned. A walk over a slow or cold volume can reach the deadline;
`status` reports each walk's elapsed time. One admission may walk the payload
more than once (the front door's `status`, then the library's admission
walk), so its cost grows with pool size. The 50,000-entry, depth-24, 5 s
bounds before oxide#108 refused real build caches (Praxis's 76,669-entry and
Farmyard's 60,657-entry trees, Supervisor report), including reuse of an
existing slot above 50,000 entries.

Before leasing a compatible idle slot, the library inventories that exact
slot's complete unique allocated bytes and keeps the warm generation; the
half-budget recycling rule is retired (oxide#118). Incomplete or unknown
accounting refuses the lease. A held same-holder replay keeps its exact lease
and path, and another holder remains busy without touching the held slot.
Byte pressure is handled by admitting a new slot, not by discarding a
compatible warm one: with the whole-payload union at or above the budget, the
oldest eligible idle whole slot is removed until the union is under it, and
`Full` refuses when only held slots remain.


### Ledger-only view for front-door membership (oxide#110, lead `e4c74399`)

Implemented as a routine correction with no admission, closure, recovery or deletion change. The design passed independent prospective review at `edc4f54`. Its focused native proof `rust:front-door-accounting` passed once at source `38f79ac` (host run `9d11cd31`, exit 0, 169 s wall; script 168 s including cleanup, sampled peak scratch 125 MiB) under the old `scripts/bounded-test.py` exception route. The Effigy client lost the scheduler (`scheduler_unreachable`, exit 75) and truncated the stream; the full output was recovered from the scheduler afterwards. An earlier submission was cancelled before launch.

**Problem it removed.** The library's `status` runs one whole-payload walk plus one walk per slot. Until oxide#110 the front door called it on ordinary commands where it needed only ledger membership. From source at `3842239`:

| Call site | What it used from status | Now |
| --- | --- | --- |
| `Store::open_at` (internal pool, every command) | `backend` | ledger view |
| `Store::open_removable` (selection through `usable`, every command while registered) | `backend` | ledger view |
| `Store::attempt` (acquisition affinity) | `slots[].project` | ledger view |
| `Store::recover_settled` | held `slots[]` by project | ledger view |
| `Store::recover_interrupted` | held `slots[]`: `slot_id`, `lease_id`, project | ledger view |
| `removable::internal_holds` | held `slots[]` by ID | ledger view |
| `Store::payload_use` (the per-command result line) | `payload_unique_allocated_bytes`, `cap_bytes` | removed; the lease's own reuse walk |

On a removable pool, one ordinary command paid several full walks, and a waiting command paid them on every 200 ms poll. At 201,948 entries one walk took 3.4 s.

**Ledger view.** `managed::ledger_view(pool) -> LedgerView`:
- **Opening:** the unchanged `Pool::open`: canonical root, foreign-entry and retired-image refusals, pool lock, config and backing verification, ledger identity checks, the slot limit and `reconcile_evicting`. The view is therefore **not side-effect-free**: recorded-deletion reconciliation runs exactly as for `status`.
- **Per-record checks:** under the same pool lock and in ledger order, it keeps status's cheap, non-recursive checks with the same errors, through one shared helper: `slot_path` refuses a zero slot ID (`unknown_accounting`), and a slot-root `symlink_metadata` error other than `NotFound` propagates (`io`). A record whose root is missing, held or idle, stays a member: no closure is inferred and no refusal is added. A symlinked root or partial accounting stays non-gating, as in status.
- **Returns:** `store_id`, `backend`, `cap_bytes`, `reconciliation_blocked` and each slot's `slot_id`, `incarnation`, occupancy, `project`, `workspace`, `profile`, `holder` and `lease_id`, the same values status shows. The type has **no byte or coverage fields**, so a skipped walk cannot read as zero or complete. Selection, affinity, wait and recovery decisions, record-lock probes, process-group checks, retirement checks and admission-lock order are unchanged.

**Full accounting stays** for `oxide status --measure` (oxide#128: the
library's passive `measure`, the same payload and per-slot walks made without
the pool lock or reconciliation), `oxide init`/`new` readiness, which reads its
figure and effective budget from its own completed full status, and every
library admission, reuse, recycling and eviction walk, with unchanged bounds.
Default `oxide status` no longer walks, opens the pool lock or reconciles:
it reads the [passive snapshot](#passive-snapshot-and-measurement-oxide128).

**Result line.** It walks nothing. `managed::lease_measured` returns the grant and, only when the library's reuse walk took an idle slot, that walk's unique allocated bytes with the exact slot ID and incarnation. The line shows that figure with its provenance, `slot 5 was 7900 MiB when leased`; `new slot` after allocation or recycling; `slot size unknown` for a held same-holder replay, where `reused_slot` is true but nothing was measured. It then shows this command's soft budget and `pool total: see oxide status`. Nothing walks a slot after release, when another command may hold it.

**Effect.** An ordinary reuse command walks only its target slot, the library's existing reuse walk. New-slot allocation keeps exactly its whole-payload walks before and after eviction. Waits and recovery walk nothing.

**Observation hook.** Debug builds only, absent from release builds such as the installed `oxide`: `inventory::walk_observer` records the root of every `inventory_directory` walk, the one entry point of library accounting, `status --measure`, Cargo-home measurement and the stray check, on a per-thread recorder for tests and, when `OXIDE_TEST_WALK_LOG` names an absolute file, one line per walk. It changes no walk. Walk counts show routing only; they are not a resource gate. The same debug-only hook can inject a scoped fixture fault for a proof: a one-shot `delay_before_read_dir` of 1..=65,000 ms (aggregate-bounded, marked fired before it sleeps and reported by a debug marker so a proof can prove activation). Injections are gated on the guard's own canonical `/private/tmp` scratch and evidence variables plus `OXIDE_PROOF_DEADLINE_NS`; a malformed, misapplied or out-of-scope injection installs nothing, and release builds have no hook.

**Proof.** Standard tier. Library tests show that the view equals status's membership field for field with no walk while `managed::status` still walks the payload and every slot; that reuse walks only its slot and returns that measurement, a held replay walks and measures nothing, and a new slot keeps its single admission walk; that a zero slot ID, a slot-root lookup error and a foreign pool entry refuse the view and status alike; and that a missing held root stays a member while partial accounting and a symlinked root stay non-gating and reuse still refuses incomplete accounting. The unreadable-walk and slot-root refusal cases inject `io::ErrorKind::PermissionDenied` at exact canonical-path `read_dir` and `slot_root` `symlink_metadata` calls with a current-test-thread `cfg(test)` guard and hit assertions. These cases preserve the library's gap and refusal mapping, but do not prove kernel permission enforcement or an operating-system `EACCES` response. Front-door recovery rules take the view's slot type unchanged. The native proof `effigy rust:front-door-accounting` covers, on one private pool with an idle slot of more than 200,000 entries: new-slot and reuse walks, a default `status` that walks nothing, `status --measure` and readiness walks; a live-holder and killed-holder `recover-interrupted` refusal with no walk; a waiting build that walks nothing, then recovers, evicts the untrusted slot and builds cold; and new-slot admission with four whole-payload walks around eviction, hard links counted once. Removable selection is covered by the shared refusal tests only: no native session attaches a volume.

### Passive snapshot and measurement (oxide#128)

Default `oxide status` must stay fast and read-only on a large pool, so it
does not call `status`, `ledger_view` or `Store::open`, all of which take the
pool lock and run recorded-deletion reconciliation. `managed::snapshot(pool)`
is a separate read:

- **Opening:** canonical owner-only pool root, the unchanged foreign-entry and
  retired-image refusals, then config and ledger reads. It creates no lock,
  takes no lock, waits for no writer and writes nothing, so a snapshot
  returns while another command holds the pool lock.
- **Validation shared with `Pool::open`:** backend shape, config/ledger
  identity, the slot ceiling, and every record's ID, incarnation and recorded
  directory. A directory pool's payload root must still be the real private
  payload directory on the pool's device; a removable pool's recorded volume
  is reported as recorded and never probed, so an absent volume still yields
  its ledger.
- **Torn reads:** each file is replaced atomically, so one snapshot may
  straddle a concurrent replacement. A mismatched config/ledger pair is read
  again once; a second mismatch refuses as `backing_mismatch`. It is never
  repaired and never waits.
- **Returns:** `store_id`, backend, recorded cap, the recorded payload root,
  the recorded volume binding, ledger membership in ledger order, and the
  slot IDs recorded `Evicting`. A pending deletion is reported as pending
  reconciliation, distinct from idle and held; nothing infers writer closure
  or current eligibility.

`managed::measure(pool)` is the diagnostic measurement: the same recorded
snapshot plus one whole-payload walk and one walk per slot, using the
existing entry and depth bounds. The caller's remaining share of one
cumulative command budget becomes a single deadline (clamped to the
accounting bound) that every walk recomputes against, so the payload walk and
all slot walks stay inside it rather than each starting a fresh bound; past
the deadline a walk keeps a 1 ms bound and reports an elapsed-deadline gap. It
takes no pool lock, creates none, reconciles nothing and writes nothing, so a
recorded `Evicting` slot is walked as it stands and makes coverage unknown.
After walking it re-reads the pair once; if any slot's ID, incarnation,
directory or occupancy changed, coverage is unknown rather than pairing a
walk with a different generation. Neither entry point persists anything.

The front door records one diagnostic fact from the reuse walk an acquiring
command already makes, in
`<pool>/oxide-managed/status-observations.json` (schema
`oxide.status-observations/1`, ≤256 KiB, 128 retained recent facts, each
bound to store ID, slot ID, incarnation, exact recorded directory, wall-clock
time, allocated bytes, source and measuring lease). The sidecar is an
auxiliary diagnostic inside an already-tolerated directory: it bumps no core
format, an older binary may ignore it, and a write failure changes no
admission, ownership or exit status. A missing, unreadable, foreign, corrupt,
unknown-version or mismatched sidecar reads as not measured, never as a
fallback scan. Default status never creates or refreshes it and `--measure`
never persists.

### Slot headroom and ranked idle replacement (oxide#118)

Implemented from the approved Queue brief (Tom, 2026-10-26: "Oh yeah, raise
the slots!", then "Ah yeah, switch to your solution"). Slot headroom only:
`MAX_SLOTS` is 128 and Oxide's project cap is eight; neither build concurrency
nor `max_active` changed, and there is no new configuration.

**Representation.** `MAX_LEDGER_BYTES` is 2 MiB. Every bounded field keeps its
cap: paths 1024 bytes, holder 128, lease ID 160, slot directory component 80.
A compact JSON ledger with every field at its ceiling, escaped sixfold and
with 20-digit integers, is proven by a unit test to fit the cap and round-trip.
Control state is bounded separately: 256 records, 128 wait tickets, 128 clean
marks, 16 recoveries, 16 flushes, per-object compact ceilings (record 40 KiB,
recovery 9 KiB, holder 5 KiB, lost 8 KiB, flush 8 KiB, clean 7 KiB, ticket
128 B, state header 4 KiB) and a 16 MiB file, whose gross object sum is
11,702,272 bytes. The two representations are envelopes for valid state, not
a total control-directory bound: preexisting orphan origins and crash-left
`ledger.tmp-*` files stay outside them, and nothing here authorizes cleanup.

**Permanent fences.** A legacy ledger (no `capacity_policy`/`next_rank`, rank
0) and a version-1 control state stay readable and are never migrated by
`status`. The first revised grant, release or recovery, or an explicit
eviction, installs the library ledger's `capacity_policy: 1` fence and rank
sequence atomically with that mutation; every Store-controlled mutation writes
control `version: 2`. A completed legacy `Evicting` reconciliation may still
finish in the legacy representation. Old binaries refuse the fenced
representations. Before a revised write, every stored field is validated
against the raised caps; an oversized legacy field refuses with the original
file, ownership and quarantine bytes untouched, never truncated or dropped.

**Hand-off trigger.** The numeric `version` of a control `state.json` (the
internal pool's and the removable pool's) and of `removable.json` is also the
front-door hand-off trigger (oxide#122). A newer binary that writes version 3,
for example, lets an older binary hand off once to a receipt-matching
installed successor whose recorded capabilities cover that version, instead of
only refusing. Only a numeric `u32` above the running binary's version
participates; malformed, foreign, oversized, unreadable and differently-fenced
documents keep the existing refusal, and a future incompatible format must
bump its numeric version to participate. The hand-off itself is pre-effect
only: see [installed-binary hand-off](workflow.md#installed-binary-hand-off-oxide122).

**Ranked LRU.** Each pool keeps one monotonic idle-rank sequence. A new actual
grant and a first held-to-idle release or recovery stamp a fresh rank in the
same atomic ledger write; holder/grant replay and release/recovery replay do
not advance it. Legacy ranks are 0 and sort oldest. Eviction order is
ascending `(rank, numeric slot id)`. Counter exhaustion refuses before any
deletion or transition, including the explicit `evict_idle_slot` path used for
an untrusted or lost slot; there is no wrap, rebase or cross-pool comparison.
Store spawn touches stay separate freshness-origin marks and the wall-clock
`last_used_ms` remains an observation.

**Admission.** Every allocated ledger entry counts: held, idle,
retired-backed and unresolved `Evicting`. Compatible idle reuse stays first,
with its complete target walk. Below both count ceilings and without byte
pressure nothing is evicted, so the old 32/2 counts no longer churn slots.
At the pool ceiling, the least-recently-used *eligible* idle whole slot is
replaced and the new slot is built cold; a byte-pressure admission does the
same using the global idle order and remeasures the inode union after each
deletion. Each deletion must reduce membership and an admission is bounded by
the entries it saw initially. Held, retired, uncertain (live or unattributable
lock), foreign and unmeasurable slots are never eligible, and a safety failure
stops admission instead of skipping to another slot. With no safely eligible
idle slot, the lease refuses honestly; it does not promise a deletion through
a failed safety gate. At the eight-slot project ceiling Oxide replaces that
project's oldest eligible idle whole slot, which also frees pool capacity.
The front door chooses that victim by recorded rank but asks the library to
retire it as part of the same admission: complete whole-payload accounting and
rank headroom are checked before any deletion, and a refusal leaves the victim
and every metadata record in place.
A command waits only when a specific held compatible slot of its own checkout
exists under the existing identity and freshness rules; an unrelated build's
completion is not assumed to free a count or byte ceiling. The freshness loop
is bounded at nine grants: the eight project slots a refusal can try plus one
cold fallback, with sharing off after the first refusal and the existing
discard/unshare before reacquisition.

**Proof and evidence.** `effigy rust:slot-headroom` under the qualified shared
supervisor covers the fences, no-migration reads, rank exhaustion, a 128-slot
pool's 129th and 130th replacements, the eight-slot project cap with a specific
held-slot wait and one LRU idle replacement, compatible reuse, no eviction
below both ceilings, byte-pressure LRU with a held slot excluded, and a
foreign payload-entry refusal. It also drives a project replacement whose
payload walk is incomplete (a depth-bound gap) and checks that the victim
survives and membership is unchanged; every fixture child stays in the
qualified supervisor's session and the script starts no session or group of
its own, so the runner owns closure on any fault. The worst-valid
ledger/control serialization, escaped round-trip and oversized-field refusals
are unit tests in `effigy test rust`. Not covered natively: the nine-grant
freshness bound is a bounded-loop inspection, and no large live inventory or
physical multi-million-entry performance is claimed.

### Parked: cached closed-slot accounting (lead `cbb2824d`)

Parked on 2026-10-21, not implemented. Sol's re-review of revision 2 (`d578447`) was again changes required. Transient hard-link mutation means an idle slot's contents and link topology cannot be shown to be unchanged cheaply, and out-of-lock walks need snapshot and conditional-commit rules. An exact cache therefore needs an immutability mechanism or an explicit policy relaxation. The per-command cost is addressed first by removing the front door's non-authoritative whole-payload walks, a separate correction. Both review verdicts are kept on the lead. Revision 2 text follows for reference. Revision 2 answers Sol's prospective review of `5852670` (changes required: shared-inode mutation, metadata outside slots, transition rules). Revision 2 drops revision 1's held baselines and overcounting sums.

**Goal.** Stop walking every idle slot on every command, while keeping today's admission figures. Today each command walks the whole payload at least twice: once in the front door's selection status and once at library admission. Measured on the Builds pool: 95,020 entries in 1.2 s, then 201,948 entries in 3.4 s an hour later.

**What is cached.** A closed observation per idle slot: `{incarnation, closing lease ID, unique bytes, logical bytes, entries, shared inodes, measured-at}`.
- *Shared inodes* counts regular inodes whose `nlink` exceeds the links found inside that slot.
- An incomplete walk stores no observation. It stores a diagnostic instead: gap causes, an observed lower bound and the time.
- Ledger records stay within `MAX_LEDGER_BYTES`. The worst-case serialized size is proven in tests.

**Admission figure.** The fast path is the sum of three parts:
1. fresh shallow accounting of the payload root and `slots/` entries: the tolerated macOS metadata from oxide#105, never control-root bytes;
2. a fresh walk of each **held** slot, as today, so held growth still counts;
3. the cached observation of each **idle** slot.

The fast path applies only when every idle slot has a valid observation with zero shared inodes, every held-slot walk shows zero shared inodes, and every shallow metadata entry has `nlink == 1`. Under those conditions the slots are pairwise disjoint: an inode in two slots has links outside each, so it shows as shared in both. The sum then equals today's `(device, inode)` union.

Otherwise admission uses the **union fallback**: fresh bounded walks of the root entries and every slot, unioned as today. The fallback also refreshes the observations of idle slots whose walks complete. Shared-inode mutation therefore never reaches the sums or subtractions. In Sol's B/A case, B closes with shared inodes greater than zero, and every later admission and eviction takes the union path. There is no persistent inode census.

**Validity.** An observation is valid only while all of these hold:
- the slot is idle and unleased;
- its incarnation and last lease ID equal the observation's;
- its root `(device, inode)` and the removable volume binding are unchanged;
- it is younger than the maximum age (proposed 24 h).

Anything else invalidates it.

**Transitions.** Every row is one atomic, synced ledger replacement unless stated otherwise.

| Event | Ledger result | Crash before / after the write |
| --- | --- | --- |
| Allocate a new slot | Held; no observation | Unchanged / held new slot (recovery as today) |
| Acquire or replay an idle slot | Held; observation **cleared in the same write that changes `last_lease_id`** | Idle with observation / held without one |
| Close or recover (after positive writer closure and, on removable pools, a successful flush; the walk runs through the bound slot fd before the write) | Idle, plus the new observation if the walk completed, or the diagnostic if not | Still held, so closure or recovery repeats the walk / idle, settled |
| Removable flush fails | Retired or held, as today; no observation | Unchanged |
| Lazy refresh (idle, observation missing or expired): walk outside the pool lock, then recheck under it | Observation or diagnostic, **written before and independently of the admission decision**, so progress survives a refusal | No change / refreshed |
| Recycle, eviction or reconciliation | Evicting, observation cleared, then the record is removed after deletion as today | As today |
| Upgrade a v1 ledger | v2 with no observations; held slots are unaffected because they are always walked fresh | The first v2 write is the durable upgrade; before it, the ledger is still v1 |
| `status --verify` | May publish observations for **idle** slots that still match under the lock; never for held slots | No change / refreshed |

**Rules.**
- **Refresh per admission:** each admission refreshes at most two missing or expired observations, oldest first. If any stay missing, that admission takes the union fallback, so behaviour matches today.
- **Reuse:** reusing a compatible idle slot still walks only that slot, as today. It is never blocked by other slots' missing observations.
- **Locking:** the internal admission barrier comes before the removable admission, and pool locking stays inside admission. Nothing blocks on a record-owner lock while holding admission, or takes admission from inside the pool lock. Walks run outside the pool lock and publish only after a recheck.
- **Missing observations:** these are reported as "not cached". They are never a selection failure, and they never cause a new fallback to the internal pool. The existing explicit selection fallbacks are unchanged.
- **Front-door paths:** one library accounting result serves every path, with no private walks: selection, `status`, the per-command result line, `slot_size` and the recursive reporting at `main.rs:664`, project-cap eviction, shared-slot handback, untrusted-slot discard, normal and reserve-stop release, automatic and explicit recovery, and removable flush and retirement.
- **`status --verify`:** each walk uses the existing bounds, with an overall deadline (proposed 300 s). A partial result is reported as partial, and only completed idle walks are published.
- **Ledger version:** the ledger version is independent of the lease and config wire identities, and existing held IDs are preserved. Old binaries refuse a v2 ledger without changing it.

**Policy change for Tom.** One remains: **delayed detection inside idle slots.** Between measurements (at most the maximum age, and also at every reuse, recycle or `status --verify`), two things go unseen:
- a newly unreadable entry or mount boundary inside an idle slot;
- Finder metadata bytes added inside an idle slot.

Today a new allocation would detect the first, and count the second, immediately. Revision 2 has no other admission change: held slots still count fresh, unions stay exact, and duplicate accounting adds no evictions.

**Proof (after #109 qualification, elevated, `rust:pool-accounting-cache`).**
- A pool of at least 200,000 entries, including a slot of at least 60,000, where fast-path admission walks only the held slots and the root.
- Each transition row and both crash sides.
- Lazy refresh persisting across a refusal.
- A migrated held slot.
- Cross-slot links, including mutation followed by eviction, taking the union path.
- Root metadata, including a hard-linked entry.
- `status --verify` running alongside a held build.
- Removable flush failure and reattachment.
- Measured close-time and acquire-time cost.

History, retired with the image backend (oxide#102): the verified APFS
sparse-image volume was the hard aggregate bound, the declared cap had to
equal the image size, and `verified_capacity_bytes` was the effective OS
bound. Current pools have no such ceiling.

Nothing in the library stops a leased build from filling its filesystem:
writes then fail with the OS's own error and the lease stays held. The front
door's best-effort host-reserve watchdog is owned by
[workflow](workflow.md#directory-budget-v1-as-shipped). Host reserve, OS
metadata and private workspace `target`/temp bytes are distinct from managed
payload bytes; status does not add them together. Deleting an idle slot frees
its space on the payload filesystem. No build-size distribution or failure
percentage is established. Selective `cargo clean -p` is not part of this
interface.

## Exit codes

| Code | When |
| --- | --- |
| 0 | init/lease/release/recover success; complete status and capacity observations |
| 1 | usage, invalid input, I/O, unsupported platform |
| 2 | busy or holder conflict; also partial/truncated status |
| 3 | full, unknown accounting, or ledger/slot exhaustion |
| 4 | unsafe path, symlink, foreign uid, or backing mismatch |
| 5 | unknown lease identity |

Lease/release/recover/init emit one versioned JSON object on stdout. Errors
go to stderr as `oxide-storage-lease: <reason>: <message>`.

## Reviewed local handover artifact

A packaging-only rebuild from merged reviewed source
`556171b63b69c32a89ec927ca2763b78f18efbc1` (reviewed head
`f75cbb4d0efd11034adb3d1dd826df9bb993ecf0`, PR #67) is available at:

```text
/Users/tom/.local/share/northstar/release-audits/oxide/2026-10-19/lease-handover/f75cbb4d0efd11034adb3d1dd826df9bb993ecf0/oxide-storage-lease
```

SHA-256: `ddceaf502589ea760a86edec8f47e3563675727f754c00888127eb4074535045`.

On 2026-10-19, the four handover binaries were copied, without rebuilding or
moving their originals, out of disposable `target/` into that durable audit
root. `manifest.json` beside the source-ref directories records source and
copy paths, sizes, modes and matching SHA-256 hashes. The historical copies are:

| Source ref directory | Binary SHA-256 |
| --- | --- |
| `376844daedd363ff5c2441e3ba12d1dfbe4e54b0` | `637a68b740b0ad424a085bd42b5b30feee129962334831c3cb4ee8f60e087785` |
| `8941ae075356b456957d5678b77dd919f34eb65f` | `9b0b0fc853c8607a94ccec765b84323d25ab42e0dcb95a40e1446b5a6cf12fbd` |
| `ecc185bc77ce382d5a832656166a144822a1bc6a` | `9d468378af13b1438227d93c5b166ab638ce5a19adfe2ead3857ee4f6085d1a8` |
| `f75cbb4d0efd11034adb3d1dd826df9bb993ecf0` | `ddceaf502589ea760a86edec8f47e3563675727f754c00888127eb4074535045` |

This preserves existing artifacts; it grants no new validation or installation
credit. Nucleus's installed copy and live use were not read or changed.

This canonical-checkout rebuild has its own hash; the experimental binary
hash remains unchanged in the evidence. No native fixture, consumer Cargo
experiment, pool initialization or installation was performed by packaging.

This optional diagnostic upgrade adds `status --capacity` to the earlier
`637a68b7…` recycling artifact. The APFS metadata correction and half-cap idle
recycling remain present. Existing receipt-v2/ledger-v1 pools reopen without
re-init or migration; legacy lease/status JSON and exits remain compatible.
The independent reviewer ran all covering selectors successfully, including
235 Rust tests (three environment-gated probes ignored) and a fresh native
CLI proof with capacity sampling and byte-for-byte non-mutation checks.
The Nucleus owner reports this diagnostic artifact installed and hash-checked,
with no adapter restart or consumer code change. The [consumer boundary](project-adoption.md#first-consumer-use-and-apfs-metadata-correction)
owns its live-pool observation and remaining first-task gate. The diagnostic
does not expand the pool, clean packages, compact the image or change
admission/ownership policy.

Lease JSON v1 includes `lease_id`, `store_id`, `slot_id`, `incarnation`,
`project`, `workspace`, `profile`, `holder`, `state`, `reused_slot` and the
`env` object with the two exact variable names above. Release/recover returns
`{version:1, lease_id, state:"idle", replay}`. Status uses `slots`,
`payload_logical_bytes`, `payload_unique_allocated_bytes`, `coverage`, `full`
and pool identity/cap fields. Actual serialization is JSON with quoted keys.
The profile label selects storage; callers must use the corresponding Cargo
profile themselves. Apply the returned environment through actual Effigy
projection; an outer assignment alone is insufficient for an admitted run.

Replay of the latest released lease returns success; older/evicted lease IDs
may return exit 5 without touching any new holder. Do not treat an unknown
or mistyped ID as evidence that the intended writer ended. A held compatible
project/profile returns busy rather than allowing a concurrent writer.

If an interrupted `Evicting` deletion cannot safely remove its recorded slot
root or an owned entry, the generation remains `Evicting` and unavailable.
Status stays bounded and reports unknown accounting; an unrelated held lease
can still be released. New leases remain refused until the recorded deletion
can safely finish. A later open resumes only that recorded slot path. Do not
edit a live ledger, follow/delete a foreign target, or use TTL to clear a
refusal. Failed provisioning with files beyond the lock can also need explicit
owner recovery. The focused real run did not exercise eviction or census slot
symlinks. These limits are separate from its observed reuse result.

## Limits

- Holder ≤ 128 printable ASCII bytes without `/`
- Paths ≤ 1024 bytes, absolute, canonical, no symlinks
- Status output default 65536 bytes (minimum 64)
- Finite slots 128 and a 2 MiB ledger; new slots refuse at exhaustion rather than dropping active state
- Init refuses a declared cap that differs from the image size; pre-rule pools
  refuse as mismatched backing without migration

## Not claimed

- No general Queue, Effigy or editor integration; the consumer-owned Nucleus
  integration and its first observed task are recorded in the [consumer boundary](project-adoption.md#observed-ordinary-task-lifecycle)
- No general cross-worktree/source-invalidation guarantee beyond the recorded
  pinned dev workflow; timestamp-restored input-only changes were not isolated
- No debug retention after release; released bytes are disposable
- No publication, install, service, global settings or frozen Contract 010 change
- No automatic process-group kill, TTL release or foreign cleanup
