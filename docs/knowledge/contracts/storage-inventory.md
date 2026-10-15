# Storage inventory (read-only)

Status: shipped Oxide-only diagnostic. The bounded read-only inventory of one
explicitly selected target directory exists as a library function and one
private unpublished binary. It is a diagnostic, not a manager: it does not
invoke Cargo or the compiler, does not own or evict storage, and does not
recommend deleting anything. Product distribution naming remains unchosen under
[Q-005](../questions.md#q-005--what-names-are-safe-to-publish), and the package
stays `publish = false`.

This is distinct from the daemon `inventory` operation described in
[architecture](../architecture.md#semantic-operations), which reads managed
slot cap, reserve, state and lease identity. The target-directory inventory has
no lease, slot key or ownership semantics and is not a claim on Oxide-managed
storage.

## Purpose

Developers and agents should be able to understand storage cost without manual
compiler archaeology. The inventory answers what occupies space in one
operator-named directory, which category dominates, and whether the accounting
is exact or partial. It runs before any shared-cache adoption decision and is
independent of the bounded backend and reusable-slot work.

## Callable

Library:

```rust
oxide_storage::inventory::inventory_directory(root: &Path, limits: InventoryLimits)
    -> Result<InventoryReport, InventoryError>
```

Binary (unpublished, built by the ordinary workspace):

```sh
cargo run --bin oxide-storage-inventory -- --root /path/to/target
cargo run --bin oxide-storage-inventory -- --root /path/to/target --json
cargo run --bin oxide-storage-inventory -- --root /path/to/target \
    --max-entries 50000 --max-depth 24 --max-output-bytes 32768 --max-elapsed-ms 10000
```

There is intentionally no Effigy alias: the inventory is a runnable product
command, not a language-neutral selector or a task-runner alias for QA.

## Required root and finite limits

`--root <path>` is required. There is no default walk of `HOME`, a workspace,
`CARGO_HOME`, `/`, or the host. The root is canonicalized once; traversal below
it never follows symlinks and never crosses a device boundary. A symlink entry
is counted and never descended.

Finite limits are always in force and can be lowered per invocation:

| Limit | Default | Meaning |
| --- | --- | --- |
| `max_entries` | 200000 | Directory entries visited before stopping partial. |
| `max_depth` | 64 | Directory depth read below the root. |
| `max_output_bytes` | 65536 | Cap on one rendered report. |
| `max_elapsed_ms` | 30000 | Cooperative traversal deadline. |

`max_entries` must be at least 1 and `max_output_bytes` at least 64. A
`max_elapsed_ms` of `0` stops on the first cooperative check.

The deadline is checked between filesystem calls. A filesystem syscall that
blocks cannot be preempted by this process, so the elapsed limit is a
cooperative bound and not a hard wall-clock ceiling. This limitation is
reported in the report notes and is not claimed as an enforced timeout.

## Categories

Entries are grouped by recognized Cargo target subdirectory names found in the
path relative to the root, with fixed priority `deps`, `incremental`, `build`,
`.fingerprint`, then `other`. A directory is classified including its own name;
a file is classified by its parent path. Unknown layouts stay in `other`.

## Accounting

Three quantities are kept separate and are never added together:

- **Logical bytes by name** are apparent (`st_size`) bytes summed over every
  name, so a hardlinked file contributes once per name.
- **Allocated bytes** are `st_blocks * 512` summed over distinct
  `(device, inode)` values. Within a category each inode counts once; the
  global union counts each inode once across the whole root.
- **Raw filesystem capacity** for the root's device is read with `statvfs` and
  reported as its own `filesystem_free` value: total, free and unprivileged
  available bytes. It is not reclaimable space and is not an inventory total.

Per-category allocated bytes deliberately overlap: one inode can appear in
several categories. The report exposes `hardlink_overlap` with the extra name
count, the number of inodes shared across categories, and shared logical and
allocated bytes. Only the global union is a non-overlapping allocation figure.
Category totals must never be summed to estimate physical disk usage.

Apparent size is not allocated size. Sparse files, compression and clone
sharing can make logical bytes exceed allocated bytes, and block rounding can
make allocated bytes exceed logical bytes for many small files.

## Coverage and partial results

Coverage is `complete` only when no gaps occurred. Every other outcome lists
explicit gaps with occurrence counts: `entry_limit`, `depth_limit`,
`elapsed_deadline`, `mount_boundary`, `unreadable_entry`, `missing_metadata`,
`unsupported_metadata` and `output_limit`.

Concurrent mutation, unreadable entries and unsupported metadata produce a
partial report, not a panic and not a false exact total. Because unvisited
entries cannot be counted, a partial report is a lower bound and never claims
exactness or nonzero savings.

## Output and exit semantics

Human output is concise: root, coverage, visited counts, a per-category table
(files, directories, symlinks, other, logical bytes, allocated bytes), the
by-name total, the global unique-inode union, hardlink overlap, the raw
filesystem value, the dominant category, factual notes and the relevant policy
pointers. `--json` emits the same report as bounded machine-readable JSON.

Rendered human and JSON bytes, including truncation markers, JSON fallback,
escaping and path fields, stay at or below `max_output_bytes`. The 64-byte
minimum is an exact ceiling: a tiny cap must not grow past 64 because a marker
was appended after the cut. Truncation is character-boundary safe, always
valid UTF-8, and JSON output is always syntactically valid. A deliberately
tiny cap returns a small fixed `{"truncated":true,"partial":true}` object so a
long root or note cannot blow the cap. Truncation is explicit; a truncated
report never implies complete accounting. Exit 2 still applies when the
output cap is reached.

Exit codes:

- `0` — complete inventory.
- `1` — refused: usage error, missing root or unusable root; no inventory.
- `2` — partial inventory: a finite bound, an unreadable entry or the output
  cap was reached. The report is still produced and honest.

## Explicit unknowns and non-claims

- APFS clone and snapshot sharing are **unknown** here. `st_blocks` is the
  file's own allocation and does not reveal shared extents.
- Finder reclaimable space is **unknown** here. The open capacity papercut
  `d4757020` (Finder reporting versus raw APFS space) is **not** solved or
  silently declared resolved by this inventory. Raw filesystem free and
  available bytes are merely a distinct measured value.
- Attributable disk writes are **unknown**. Occupancy is not write volume and
  never becomes an SSD-lifetime estimate.
- File names, sizes and age never imply an entry is stale or safe to delete.
  The report explicitly states this and never recommends deletion.
- No cache hit or miss rate is inferred, and no automatic recommendation to
  remove debug objects is produced.

An inventory result is not storage ownership or a cleanup capability.
Root paths remain explicit operator input.

## Supported policy references

When a category dominates, the report points at the supported policy area
without predicting benefit:

- incremental state dominates: [build-context policy](build-context-policy.md)
  lifecycle-aware incremental-on/off is proposed, not activated, and disabling
  incremental does not by itself reduce cumulative writes;
- compiled dependencies dominate: [project adoption](project-adoption.md)
  reusable intermediate slots require authenticated consumer integration;
- build-script output or fingerprints dominate: these are Cargo-managed inputs;
- always: this diagnostic is read-only and makes no cleanup recommendation. The
  general policy for adopting a project optimization is in
  [optimization adoption](optimization-adoption.md).

## Validation and evidence limits

Covering repository checks are the ordinary selectors: `effigy rust:compile`,
`effigy rust:format`, `effigy test rust` and `effigy docs:check`. The inventory
itself never invokes Cargo or the compiler, so its fixtures are synthetic and
small.

The library unit tests build fresh fixtures under the task-owned scratch root
and cover: known category grouping, a real hardlink across two categories with
union and overlap accounting, a sparse file with apparent greater than
allocated bytes, dangling and directory symlinks that are counted but never
followed, an unreadable directory, entry and depth truncation, the cooperative
deadline, bounded rendering (the 64-byte minimum including truncation
markers and the JSON fallback), long or multibyte root/note fields remaining
inside the cap with valid UTF-8/JSON, refused roots, invalid limits, concurrent
mutation without panic, the measured filesystem value and the absence of
stale or savings claims.

These fixtures do not qualify real project behavior, real savings or a
performance result. No existing target, Nucleus, Queue or Acow tree is scanned,
and no cache is cleaned. See [storage-efficiency findings](../storage-efficiency.md)
for what the separate comparison evidence does and does not establish.

## Not claimed

- No daemon, scheduler, dependency scanner, parser graph, plugin or new
  manager.
- No Cargo or compiler invocation, no payload/config/secret read, no file
  mutation, no cleanup and no eviction.
- No shared-cache adoption, consumer activation, installation or release.
- No physical SSD usage, reclaimable space, write attribution or cache
  hit/miss rate.
