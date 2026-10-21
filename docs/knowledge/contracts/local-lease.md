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
`recover`, `status`, `evict_idle_slot`, `init_budget_pool`,
`init_volume_pool`, `flush_volume`). They behave as
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

The library also exports `evict_idle_slot` for Oxide's per-project slot cap.
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
(`MAX_LEDGER_BYTES` 256 KiB, `MAX_SLOTS` 32). Eviction happens only at lease
time and only for known owned idle whole slots. Held occupancy, foreign
entries, symlinked pool/payload/slots/slot roots, unknown accounting and
mismatched backing refuse. During a durably recorded whole-slot deletion, an
owned nested symlink entry may be unlinked as a leaf after its parent and
entry ownership are verified. The target is never followed or removed.
Deletion never touches project or workspace paths. A lost delete is
reconciled only for the recorded evicting slot path.

**Accounting walks (oxide#108).** Every pool accounting walk uses the same
pool-specific bounds: 2,000,000 entries, depth 64 below the walked root and a
60 s cooperative deadline. The whole-payload walk starts at the payload root,
one level above `slots`, so it uses depth 65 and slot contents keep 64
([Finder metadata](workflow.md#finder-metadata-in-pools), oxide#105). They are separate from the inventory tool's
defaults, which are unchanged. The callers:

| Walk | Gate |
| --- | --- |
| The matching idle slot's own walk before reuse or recycling | Must be complete, or the lease refuses |
| The whole-payload walk before a new slot, and again after eviction | Must be complete, or the lease refuses |
| The whole-payload walk before each idle eviction in the loop | Must be complete, or the lease refuses |
| `status`: the whole payload and each slot | Observational; reports coverage, causes and time |

A walk that reaches a bound, meets an unreadable entry, missing or unusable
metadata or a mount boundary, or cannot open its root is incomplete. It
refuses with reason `unknown_accounting` (exit 3, unchanged), and the error
names the gap kinds (`entry_limit`, `depth_limit`, `elapsed_deadline`,
`mount_boundary`, `unreadable_entry`, `missing_metadata`,
`unsupported_metadata`, `root_missing`, `root_not_directory`,
`root_unreadable`, `root_symlink`) with counts, entries visited, elapsed
milliseconds and the bounds used. No path or name is printed. An incomplete
walk's bytes are never used for admission and never make a slot an eviction
candidate; `status` shows them as a lower bound. A measured zero stays
distinct from unknown. Only a slot whose own walk completes can be reused;
other `unknown_accounting` refusals (invalid identities, ledger, ownership or
backing failures, foreign entries) are unchanged.

The bounds are finite, not an arbitrary-size guarantee. The entry bound
covers 32 slots of 60,657 entries, the largest single-project slot reported
(Farmyard, 2026-10-20); a pool beyond it refuses until idle slots are
evicted or cleaned. A walk over a slow or cold volume can reach the deadline;
`status` reports each walk's elapsed time. One admission may walk the payload
more than once (the front door's `status`, then the library's admission
walk), so its cost grows with pool size. The 50,000-entry, depth-24, 5 s
bounds before oxide#108 refused real build caches (Praxis's 76,669-entry and
Farmyard's 60,657-entry trees, Supervisor report), including reuse of an
existing slot above 50,000 entries.

Before leasing a compatible idle slot, the library inventories that exact
slot's complete unique allocated bytes. At `ceil(budget / 2)` or above, it
durably retires that idle generation and allocates a fresh monotonic slot and
lease; the grant reports `reused_slot: false`. Below the threshold, it keeps
the warm slot. Incomplete or unknown accounting refuses the lease. A held
same-holder replay keeps its exact lease and path, and another holder remains
busy without recycling the held slot. This policy can make an acquisition pay
a cold-build cost to discard a large idle cache. The threshold supplies
headroom at acquisition; it does not guarantee that a particular build fits,
reserve space, constrain writes during a lease, or prevent `ENOSPC`. Other
slots still count against the budget, so ordinary admission and refusal rules
apply after the selected generation is removed.

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

**Full accounting stays** for explicit `oxide status` (`Store::report`, `slot_size` and the recursive reporting), `oxide init`/`new` readiness, which reads its figure and effective budget from its own completed full status, and every library admission, reuse, recycling and eviction walk, with unchanged bounds.

**Result line.** It walks nothing. `managed::lease_measured` returns the grant and, only when the library's reuse walk took an idle slot, that walk's unique allocated bytes with the exact slot ID and incarnation. The line shows that figure with its provenance, `slot 5 was 7900 MiB when leased`; `new slot` after allocation or recycling; `slot size unknown` for a held same-holder replay, where `reused_slot` is true but nothing was measured. It then shows this command's soft budget and `pool total: see oxide status`. Nothing walks a slot after release, when another command may hold it.

**Effect.** An ordinary reuse command walks only its target slot, the library's existing reuse walk. New-slot allocation keeps exactly its whole-payload walks before and after eviction. Waits and recovery walk nothing.

**Observation hook.** Debug builds only, absent from release builds such as the installed `oxide`: `inventory::walk_observer` records the root of every `inventory_directory` walk, the one entry point of library accounting, `slot_size`, Cargo-home measurement and the stray check, on a per-thread recorder for tests and, when `OXIDE_TEST_WALK_LOG` names an absolute file, one line per walk. It changes no walk. Walk counts show routing only; they are not a resource gate.

**Proof.** Standard tier. Library tests show that the view equals status's membership field for field with no walk while status still walks the payload and every slot; that reuse walks only its slot and returns that measurement, a held replay walks and measures nothing, and a new slot keeps its single admission walk; that a zero slot ID, a slot-root lookup error and a foreign pool entry refuse the view and status alike; and that a missing held root stays a member while partial accounting and a symlinked root stay non-gating and reuse still refuses incomplete accounting. Front-door recovery rules take the view's slot type unchanged. The native proof `effigy rust:front-door-accounting` covers, on one private pool with an idle slot of more than 200,000 entries: new-slot, reuse, status and readiness walks; a live-holder and killed-holder `recover-interrupted` refusal with no walk; a waiting build that walks nothing, then recovers, evicts the untrusted slot and builds cold; and new-slot admission with four whole-payload walks around eviction, hard links counted once. Removable selection is covered by the shared refusal tests only: no native session attaches a volume.

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
- Finite slots 32; new slots refuse at exhaustion rather than dropping active state
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
