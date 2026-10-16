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
oxide-storage-lease --pool <path> status [--json] [--max-output-bytes <n>]
```

Covering CLI proof is `effigy rust:storage-lease-cli`: it builds the binary,
exercises usage refusals, then on macOS with a 20 GiB host reserve provisions
one fresh 128 MiB APFS image and runs lease/release/recover/status, concurrent
acquire, duplicate-holder replay, busy, symlink/foreign refusal, idle reuse,
delayed APFS metadata arrival and mismatched-backing refusal. Rust lease tests
cover direct/dangling/nested symlink reclamation, interrupted-delete recovery,
unsafe-root status/release and invalid metadata entries. It is not a
filesystem-fill benchmark and not full QA.

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
treats as abandoned or uncertain. This version never infers abandoned from
TTL, PID, age or holder loss; status reports such occupancy as `held` until
release or recover. Automatic TTL reuse is not implemented.

**status** lists finite slots with holder, observed workspace, occupancy,
logical and unique allocated bytes, pool cap, and full/unknown. Coverage is
`complete`, `partial` or `unknown`. Unknown accounting is honest. No payload
or secret content is printed.

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
Writes that fill the volume return ENOSPC from the OS and remain leased; this
CLI does not repeat the filesystem-full proof (see bounded storage).

Host reserve, APFS metadata and private workspace `target`/temp bytes are
distinct from managed payload bytes. Status does not add them together.

## Exit codes

| Code | When |
| --- | --- |
| 0 | init/lease/release/recover success; complete status |
| 1 | usage, invalid input, I/O, unsupported platform |
| 2 | busy or holder conflict; also partial/truncated status |
| 3 | full, unknown accounting, or ledger/slot exhaustion |
| 4 | unsafe path, symlink, foreign uid, or backing mismatch |
| 5 | unknown lease identity |

Lease/release/recover/init emit one versioned JSON object on stdout. Errors
go to stderr as `oxide-storage-lease: <reason>: <message>`.

## Reviewed local handover artifact

A packaging-only rebuild from merged reviewed source
`0496f2fbad544d59f515d7dda3051e19a8c3524c` (runtime source identical to reviewed
`ecc185bc77ce382d5a832656166a144822a1bc6a`) is available at:

```text
/Users/tom/Dev/projects/oxide/target/oxide-handover/ecc185bc77ce382d5a832656166a144822a1bc6a/oxide-storage-lease
```

SHA-256: `9d468378af13b1438227d93c5b166ab638ce5a19adfe2ead3857ee4f6085d1a8`.
This canonical-checkout rebuild has its own hash; the experimental binary
hash remains unchanged in the evidence. No native fixture, consumer Cargo
experiment, pool initialization or installation was performed by packaging.

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

- No Nucleus, Queue, Effigy or editor integration
- No general cross-worktree/source-invalidation guarantee beyond the recorded
  pinned dev workflow; timestamp-restored input-only changes were not isolated
- No debug retention after release; released bytes are disposable
- No publication, install, service, global settings or frozen Contract 010 change
- No automatic process-group kill, TTL release or foreign cleanup
