# Same-user local lease CLI

Status: unpublished Oxide-only product command. Same-UID callers take exclusive
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
internal. Capacity semantics are owned by
[bounded storage](bounded-storage.md). Consumer adoption remains
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
as each lease's `--project`, so library reuse stays checkout-affine. Its
automatic crash recovery touches only leases whose ID or holder string its
own control record names.

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
`complete`, `partial` or `unknown`. Unknown accounting is honest. No payload
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

The verified APFS volume is the hard aggregate bound, and the declared cap
must equal the image size so that volume size is the enforced ceiling. A
smaller declared cap on a larger volume is refused at init and at open:
admission accounting alone is not a quota. Admission uses the
pinned cap and the [storage inventory](storage-inventory.md) unique-allocated
figure for owned slots; filesystem metadata directories remain outside that
slot accounting. The measured `verified_capacity_bytes` (APFS metadata
overhead below the declared size) is the effective OS bound, and writers
cannot exceed the declared ceiling through it. Control/ledger headroom stays outside the payload and
is bounded (`MAX_LEDGER_BYTES` 256 KiB, `MAX_SLOTS` 32). Eviction happens only
at lease time and only for known owned idle whole slots. Held occupancy,
foreign entries, symlinked pool/payload/slots/slot roots, unknown accounting
and mismatched backing refuse. During a durably recorded whole-slot deletion,
an owned nested symlink entry may be unlinked as a leaf after its parent and
entry ownership are verified. The target is never followed or removed.
Deletion never touches project or workspace paths. A lost delete is
reconciled only for the recorded evicting slot path.

Before leasing a compatible idle slot, the CLI inventories that exact slot's
complete unique allocated bytes. At `ceil(cap_bytes / 2)` or above, it durably
retires that idle generation and allocates a fresh monotonic slot and lease;
the grant reports `reused_slot: false`. Below the threshold, it keeps the warm
slot. Incomplete or unknown accounting refuses the lease. A held same-holder
replay keeps its exact lease and path, and another holder remains busy without
recycling the held slot. This policy can make an acquisition pay a cold-build
cost to discard a large idle cache. The threshold supplies headroom at
acquisition; it does not guarantee that a particular build fits, reserve half
the volume, constrain writes during a lease, or prevent `ENOSPC`. Other slots
and OS metadata still consume the aggregate cap, so ordinary admission and
refusal rules apply after the selected generation is removed.

Writes that fill the volume return ENOSPC from the OS and remain leased; this
CLI does not repeat the filesystem-full proof (see bounded storage).

Host reserve, APFS metadata and private workspace `target`/temp bytes are
distinct from managed payload bytes. Status does not add them together.

Deletion of an idle slot frees space inside the payload filesystem for later
builds. Returning host-allocated sparse-image space is a different operation:
this CLI does not compact its backing image. A retained host allocation does
not itself mean the deleted slot still consumes live payload space. Apple's
installed `hdiutil(1)` manual documents conditional APFS sparse-image compaction;
that is not a shipped Oxide operation or authority to compact a mounted or
uncertain pool. The payload ceiling also does not claim an exact ceiling on
all host-side image/control metadata, snapshots or private target/temp output.

A new empty slot still cannot hold a build larger than the remaining payload
capacity. Increasing the pool changes that finite limit; it does not remove
it. No build-size distribution or failure percentage is established. Selective
`cargo clean -p` before release is not part of this interface and has not been
qualified as removing all historical path-specific workspace artifacts while
preserving the desired shared intermediates. Any such cleanup would remain
inside exclusive ownership with admitted execution and positive completion;
it cannot be added after an assertion that all writers have ended.

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
/Users/tom/Dev/projects/oxide/target/oxide-handover/f75cbb4d0efd11034adb3d1dd826df9bb993ecf0/oxide-storage-lease
```

SHA-256: `ddceaf502589ea760a86edec8f47e3563675727f754c00888127eb4074535045`.
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
