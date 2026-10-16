# Same-user local lease CLI

Status: unpublished Oxide-only product command. Same-UID callers take exclusive
owned slots on a verified APFS payload volume through a short-lived OS lock and
a durable JSON ledger. There is no resident daemon, HMAC credential, issuer,
callback service, Cargo runner, scheduler or Git manager. Cross-worktree
usefulness is a mandatory dependent qualification and is **not** established by
this command. Do not treat this CLI as Nucleus-ready.

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
acquire, duplicate-holder replay, busy, symlink/foreign refusal, idle reuse and
mismatched-backing refusal. It is not a filesystem-fill benchmark and not full
QA.

## Pool

`--pool` is required. It is the owner-only control directory (mode `0700`,
caller uid). Init creates it empty, or tightens an empty caller-owned directory
to `0700`. An existing pool cannot have its cap raised and cannot be
reinitialized. Arbitrary directories are not reinterpreted as bounded.

Pinned at init and recorded in `config.json`:

| Field | Meaning |
| --- | --- |
| `version` | `1` |
| `backend` | exactly `apfs-sparse-image-v1` |
| `store_id` | 32 lowercase hex chars, never reused |
| `cap_bytes` | finite payload ceiling; callers cannot raise it |
| `image_bytes` | sparse image size; must cover the cap plus APFS metadata |

Control files stay on the host: `config.json`, `ledger.json`, `lock`,
`image.sparseimage`, `receipt.json`. The payload mount is `<pool>/payload`.
Slots live only under `<pool>/payload/slots/<id>/`. Ledger and receipt are
atomic replace plus `sync`. The exclusive `flock` is held only while the CLI
runs. Cargo and tests run after the process exits; a finished CLI is not writer
completion.

Directory-only roots are not a product mode. Tests may inject a private
directory solely to exercise the ledger; that path is not compiled into the
CLI and does not claim an OS hard bound.

## Commands

**init** provisions one sparse APFS image, attaches it at `payload`, writes the
capacity receipt, pins the cap, and returns JSON `PoolInfo`. Unsupported
platforms refuse before any host effect.

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

The verified APFS volume is the hard aggregate bound. Admission uses the
pinned cap and the [storage inventory](storage-inventory.md) unique-allocated
figure for owned slots. Control/ledger headroom stays outside the payload and
is bounded (`MAX_LEDGER_BYTES` 256 KiB, `MAX_SLOTS` 32). Eviction happens only
at lease time and only for known owned idle whole slots. Held occupancy,
foreign entries, symlinks, unknown accounting and mismatched backing refuse.
Deletion never follows a symlink and never touches project or workspace
paths. A lost delete is reconciled only for the recorded evicting slot path.
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

## Limits

- Holder ≤ 128 printable ASCII bytes without `/`
- Paths ≤ 1024 bytes, absolute, canonical, no symlinks
- Status output default 65536 bytes (minimum 64)
- Finite slots 32; new slots refuse at exhaustion rather than dropping active state

## Not claimed

- No Nucleus, Queue, Effigy or editor integration
- No cross-worktree correctness, cache-hit proof or source invalidation
- No debug retention after release; released bytes are disposable
- No publication, install, service, global settings or frozen Contract 010 change
- No automatic process-group kill, TTL release or foreign cleanup
