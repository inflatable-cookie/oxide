# Storage inventory (read-only)

This contract owns the read-only inventory of one explicitly named directory. It
is a diagnostic, not a manager. It does not invoke Cargo or the compiler, own or
evict storage, or recommend deleting anything. It exists as a library function
only: the unpublished `oxide-storage-inventory` binary was retired (red-pen 1,
oxide#340). The package stays `publish = false`; product naming is open under
[Q-005](../questions.md#q-005--what-names-are-safe-to-publish).

This is distinct from the [local lease](local-lease.md) `status`, which reads
managed slot membership. The inventory has no lease, slot key or ownership
semantics and makes no claim on Oxide-managed storage.

## Callable

```rust
oxide_storage::inventory::inventory_directory(root: &Path, limits: InventoryLimits)
    -> Result<InventoryReport, InventoryError>
```

The `oxide` front door calls the library read-only for `status`. It reports
`total.files`, `total.distinct_files` and `total.allocated_bytes`, and `partial`
when coverage is incomplete.

## Root and finite limits

`--root` is required. There is no default walk of `HOME`, a workspace,
`CARGO_HOME`, `/` or the host. The root is canonicalised once. Traversal never
follows symlinks and never crosses a device boundary. A symlink entry is counted
and not descended.

Limits are always in force and can be lowered per invocation:

| Limit | Default | Meaning |
| --- | --- | --- |
| `max_entries` | 200000 | Directory entries visited before stopping partial |
| `max_depth` | 64 | Directory depth below the root |
| `max_output_bytes` | 65536 | Cap on one rendered report; minimum 64 |
| `max_elapsed_ms` | 30000 | Cooperative deadline, checked between filesystem calls |

A blocked filesystem call cannot be preempted, so the deadline is a cooperative
bound, not a hard wall-clock ceiling. The report says so in its notes.

## Categories

Entries group by Cargo target subdirectory name found in the path relative to
the root, in fixed priority: `deps`, `incremental`, `build`, `.fingerprint`, then
`other`. A directory is classified with its own name; a file by its parent path.
Unknown layouts stay in `other`.

## Accounting

Three quantities are kept separate and never added:

- **Logical bytes by name:** apparent size summed over every name, so a hardlink
  counts once per name.
- **Allocated bytes:** `st_blocks * 512` summed over distinct device and inode
  pairs. Within a category each inode counts once. The global union counts each
  inode once across the root, and is the only non-overlapping allocation figure.
- **Filesystem capacity:** total, free and available bytes from `statvfs` for the
  root's device. This is a separate measured value, not reclaimable space and not
  an inventory total.

Category totals overlap when one inode appears in several categories. The report
gives `hardlink_overlap`. Never sum category totals to estimate disk use. Apparent
size is not allocated size: sparse files, compression and clones can make logical
bytes exceed allocated bytes, and block rounding does the reverse for small files.

## Coverage and gaps

Coverage is `complete` only with no gaps. Otherwise the report lists gaps with
counts: `entry_limit`, `depth_limit`, `elapsed_deadline`, `mount_boundary`,
`unreadable_entry`, `missing_metadata`, `unsupported_metadata` and `output_limit`.

The walk is a best-effort, non-atomic observation. A path it enumerated that
vanishes during the walk increments a separate `vanished` count. It contributes
no bytes and creates no gap. A vanished path does not prove its space was freed,
because renames, open-unlinked files and hardlinks can retain it. A missing root
remains a refusal. `ENOTDIR`, `ELOOP` and a changed kind or inode are never
`vanished`.

Each directory read is sampled by path before and after enumeration. A sample
that finds another kind, inode or device is reported as a gap and the directory
is not read. These samples detect a persistent replacement of the path name. They
do not bind the open directory, exclude a transient swap, or make the walk an
atomic snapshot or containment proof.

Unreadable entries, unsupported metadata and bound stops produce a partial
report, never a panic and never a false exact total. A partial report is a lower
bound.

## Output and exit codes

The human report shows root, coverage, visited counts, a per-category table
(files, directories, symlinks, other, logical and allocated bytes), the by-name
total, the global union, hardlink overlap, the filesystem value, the dominant
category and factual notes. `--json` emits the same report as JSON.

Rendered output, including truncation markers, stays at or below
`max_output_bytes`. Truncation keeps valid UTF-8 and valid JSON, and it is always
explicit: a truncated report never implies complete accounting. A very small cap
returns a fixed truncated object.

- `0`: complete inventory.
- `1`: refused. Usage error, or a missing or unusable root. No inventory.
- `2`: partial inventory. A bound, an unreadable entry or the output cap stopped
  it. The report is still produced.

## Explicit unknowns

- APFS clone and snapshot sharing are unknown. `st_blocks` shows the file's own
  allocation only.
- Finder reclaimable space is unknown. Raw filesystem free space is a different
  measured value and does not settle the reporting question.
- Attributable disk writes are unknown. Occupancy is not write volume and never
  becomes an SSD-lifetime estimate.
- File names, sizes and age never imply an entry is stale or safe to delete. The
  report says so and recommends no deletion.
- No cache hit or miss rate is inferred.

When one category dominates, the report points at the relevant policy area
without predicting benefit. Incremental state points to
[build-context policy](build-context-policy.md), which is proposed and inert.
Disabling incremental does not by itself reduce cumulative writes.

## Validation

The library unit tests build fresh fixtures below the task scratch root. They
cover category grouping, hardlinks across categories, sparse files, symlinks that
are counted but not followed, injected unreadable directories, entry and depth
truncation, the deadline, bounded rendering, invalid limits, concurrent mutation
and the absence of savings claims. The ordinary workspace suite covers them.
Covering checks are `rust:compile`, `rust:format` and `docs:check` when docs
change. The unreadable-directory case injects `PermissionDenied` in the walker; it
does not prove kernel permission enforcement.

These fixtures qualify no real project, savings or performance result. The
inventory never scans a real target, Nucleus, Queue or other tree.

## Not claimed

- No daemon, scheduler, plugin or manager.
- No Cargo or compiler invocation, no file mutation, no cleanup and no eviction.
- No physical SSD usage, reclaimable space, write attribution or cache hit rate.
