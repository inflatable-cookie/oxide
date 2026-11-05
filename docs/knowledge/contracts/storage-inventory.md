# Storage inventory (read-only)

Status: shipped Oxide-only diagnostic. The bounded read-only inventory of one
explicitly selected target directory exists as a library function and one
private unpublished binary. It is a diagnostic, not a manager: it does not
invoke Cargo or the compiler, does not own or evict storage, and does not
recommend deleting anything. Product distribution naming remains unchosen under
[Q-005](../questions.md#q-005--what-names-are-safe-to-publish), and the package
stays `publish = false`.

This is distinct from the [local lease](local-lease.md) `status` command, which
reads managed slot membership (up to 256 slots in an 8 MiB ledger), state and
lease identity. The target-directory inventory has no lease, slot key or
ownership semantics and is not a claim on Oxide-managed storage.

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

The `oxide` front door calls the library read-only for `status`: stray
`target/` output, each slot's size and file counts, and the Cargo home
([workflow](workflow.md#cargo-home-hygiene)). It reports `total.files`,
`total.distinct_files` and `total.allocated_bytes`, and `partial` when
coverage is incomplete.

The inventory stays a runnable product command. It is not a language-neutral
QA alias and does not scan a real project target. Covering exact-head CLI
proof of the unpublished binary is `effigy rust:storage-inventory-cli`: it
builds `oxide-storage-inventory` through Effigy and checks the output-byte
ceiling on tiny synthetic fixtures (the 64-byte minimum and a normal cap,
long or multibyte root fields, valid UTF-8 and JSON, explicit truncation,
and exit 2 when the cap is reached).

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

The walk is a best-effort, non-atomic observation. When a non-root path that
this same walk enumerated returns `NotFound` at a deferred `read_dir`,
`file_type` or no-follow metadata call, the report increments the separate
`vanished` diagnostic. That path contributes no bytes; other inode bytes
already observed remain in the union. This scoped exception does not create a
coverage gap, so coverage may remain complete. It does not establish that the
path's inode allocation was freed or is reclaimable: rename, open-unlinked
files and hardlinks may retain it. A missing root remains a refusal, including
when the root disappears after it was opened. Iterator errors without an
associated enumerated path and all other errors keep their existing
conservative gap categories. `ENOTDIR` and `ELOOP` are never turned into a
`vanished` observation, and a pathname sample that finds a different kind or
inode is `unsupported_metadata`, not `vanished`.

### Per-entry reads and directory boundary samples (oxide#141)

An enumerated regular file or directory is read immediately from the live
directory entry the walk holds, and that entry is never retained. When the
absolute path name is shorter than the platform `PATH_MAX` including its
terminating NUL (`len + 1 <= PATH_MAX`) the read is relative to the already
open directory; otherwise the walk performs exactly the former full-path
no-follow operation with its unchanged error handling. `PATH_MAX` is a lookup
limit here, never a walk bound: the entry, depth, output and elapsed bounds are
unchanged, a path the platform cannot name still fails the same way
(`missing_metadata`, hence partial coverage), and no long-path completeness is
invented. Directories are still read by path name, so this is not general
long-path support. The selection is a source-level fact; the standard library
on musl and other targets may still resolve the entry by path internally, so
no syscall count or wall-clock speed is claimed.

Every directory that is read is sampled twice by path name with no-follow
metadata: once before its entries are read and once after they were
enumerated. Each sample must still be the directory observed when it was
classified, with the root's device and the observed inode. A sample that finds
another kind or inode is `unsupported_metadata`; another device is
`mount_boundary`; a root sample keeps the root refusal classes (`root_missing`
for `NotFound`, `root_unreadable` for other errors, kind or identity change,
`root_not_directory` for another kind). A non-root `NotFound` is one `vanished`
observation for that directory. A directory whose sample does not match is
never read: before the read it is skipped, and after enumeration only the
children newly queued by that directory are discarded, while older pending
directories, already observed entries and their bytes are kept and no
`vanished` event is invented for the discarded children. A slow second sample
is charged to the same cooperative deadline, so it can only make coverage
partial. The two samples detect a persistent observed replacement of the
path name; they do not bind the directory `read_dir` opened, cannot exclude a
transient swap or inode reuse, and are not an atomic snapshot or containment
proof.

These samples add two pathname metadata reads per read directory, and the
pending stack holds one observed inode beside each pending path and depth. The
memory stays bounded by the depth and entry limits. The extra work is real and
its net effect against the ancestor path resolution the entry-relative read
avoids is unmeasured; this is not a performance guarantee.

Unreadable entries, unsupported metadata and finite-bound stops produce a
partial report, not a panic and not a false exact total. Because unvisited
entries cannot be counted, a partial report is a lower bound and never claims
exactness or nonzero savings. Human and JSON reports include the separate
`vanished` count when it is nonzero; zero is omitted on the wire. New readers
default an absent count to zero. Older strict readers may reject a report that
contains a nonzero `vanished` field. This diagnostic is not persisted in the
pool ledger or configuration.

### Accounting report cost diagnosis (oxide#280)

`inventory_sizes_with` consumes only coverage/gaps, logical bytes by name,
unique allocated bytes, visited entries, elapsed milliseconds and vanished
count. It discards category breakdowns, hardlink-overlap detail, dominant
category, filesystem free-space results, notes and policy hints. Setting
`max_output_bytes` to 64 does not avoid constructing those fields: it limits
rendering, not traversal or report construction.

There is demonstrable unused report work, but no demonstrated link to the
[matching-reuse refusal](local-lease.md#matching-reuse-deadline-diagnosis-oxide280).
The filesystem-capacity probe runs after `Walker::run` and `verify_root`;
`finish` samples elapsed time before aggregating inode/category totals and
building notes/policy. Removing that final work alone cannot prevent the
walker's earlier deadline gap. Classification of each relative path and
category-bit maintenance do occur during traversal; their cost remains
unmeasured. Global `(device, inode)` allocation union work is required even
when category detail is unused.

At the inspected 1.97.1 toolchain, the locally installed Rust standard-library
source (`library/std/src/sys/fs/unix.rs`, `DirEntry::metadata`) selects
`dirfd` plus `fstatat(AT_SYMLINK_NOFOLLOW)` on Apple. Thus replacing the existing
entry-relative read with another descriptor-relative implementation is not
an established remedy. This source fact is not a syscall timing measurement.

The private investigation at `1dac1848` used a fresh, generated Cargo-shaped
tree: 60,002 declared file names (one cross-category hardlink), 818 directories,
depth up to 14, hashed object-style names and a deeper long-name branch. This
was not copied from the unavailable historical tree and did not preserve that
tree's unknown depth/name distribution or cold-volume state. An ignored
library test uses the existing walker and metadata-source observer, splits
traversal/capacity/report timings, then compares its totals with a second,
explicitly warm public-entry walk. No product code or acceptance path changes.

The replacement uncounted diagnostic **failed**: four exact-tree oracles
disagreed with the generated manifest. Both walks observed 60,822 entries and
60,004 file names, versus 60,820 entries and 60,002 names declared; allocation
was 245,780,480 versus 245,764,096 bytes. The retained tree subsequently named
two additional `.DS_Store` files, at its root and `debug/`. Those names explain
the discrepancy but their origin is **unknown**; their payloads were not read.
The ten other checks passed, including one entry-metadata source selection per
observed file/directory, complete matching public totals, hardlink union, and
partial entry/depth/deadline counterfactuals. These are not substitute PASS
evidence for the failed exact-tree checks.

Observed timings were 5,976.8 ms traversal, 0.0085 ms capacity probe, 4.37 ms
report construction and 6,204.6 ms warm public walk. These failed-packet
observations do not isolate in-walk classification from required metadata,
enumeration or pathname samples, and do not establish a remedy or historical
cause. Source-selection counts are not syscall counts. The qualified guard
sampled a 245,829,632-byte peak against 512 MiB and minimum free space
3,242,000,936,960 bytes. Its receipt records closed child status 1 without a
deadline/fault, but ends `cleanup_partial_or_unknown`; Effigy exited 125.
Closure is not successful cleanup or qualification.

The packet remains at
`/Volumes/Stash/scratch/oxide/oxide-proof.if_xhj5b/evidence/` (`receipt.json`,
`matching-cost.json`, `matching-cost-child.log`). The earlier `98b4bbb6`
diagnostic failed on Python 3.9.6's unsupported
`Path.stat(follow_symlinks=False)` before inventory ran; its receipt remains
at `/Volumes/Stash/scratch/oxide/oxide-proof.mg2hh512/evidence/receipt.json`.
The corrected driver checks its `Path.lstat()` API before effects. Both spent
occurrences remain failed and charged; no counted occurrence followed.
Planner disposition `8f6ab30f` initially permitted review of that precise
impediment only. The later owner reopening authorized the bounded fixture
correction below, without converting either earlier failure into a pass.

**Evidence-retained fixture correction.** The corrected private driver places
its fresh 0700 fixture directly inside the current guard's evidence directory.
The unchanged `fcfda2b0` guard samples `tree_bytes(ctx["root"])`, including
evidence, and its ordinary shutdown removes only `ctx["scratch"]`. The driver
and ignored test require canonical sibling scratch/evidence paths and a
private owned fixture directly under this evidence path; no outside-root or
arbitrary fixture-path authority is added. Retention is deliberate and remains
within the 512 MiB monitored total, not a resolution of old cleanup uncertainty.

Expected names, directory identities and file identity/mode/size/allocation/
mtime/ctime observations come only from deliberate creation. The two metadata
controls are exclusively created with known bytes, and ordinary inventory
counts them as regular files. The fixed main layout is 60,822 entries, including
60,004 file names and 818 directories. The oracle compares the intended name
set and metadata before and after measured calls and after bound controls;
unexpected names or changes fail rather than being adopted. A separate tiny
negative control freezes one intended name, adds an unplanned entry, and must
be rejected by the same oracle. Its focused unit regression passed.

The owner-activated diagnostic at `46dacd3e` **failed before the inventory
child**: exclusive creation found `tree/.DS_Store` already present after bulk
population and raised `FileExistsError`; nothing overwrote or adopted it.
Its origin remains **UNKNOWN**. Independently, the guard latched
`scratch readings stale: none in 10 s`, recorded `UNCLOSED`, child status 1,
no deadline, and returned 125. Its receipt is
`/Volumes/Stash/scratch/oxide/oxide-proof.q436kq0i/evidence/receipt.json`,
SHA-256 `eb9ee35eb3bee17b88185adc864f3cbe181eb04bf3b1350daa4794d07a6184b3`.
Sampled peak was 152,424,448 bytes against 512 MiB; minimum sampled free space
was 3,241,596,981,248 bytes. The partial population and all evidence remain
kept. This packet measures neither inventory performance nor the cause of
the stale reading; it supplies no new complete-tree or native credit.

The driver now creates the two exclusive controls before bulk population,
reducing that creation window while still refusing an existing name. This
ordering correction has only an interpreter/API check, not a fresh native
occurrence. It does not fence unknown writers or repair the guard fault.
The required passing diagnostic and counted fixture occurrence remain
**missing**. No automatic replacement or cleanup follows a failed occurrence.
Admission is **unrestored**, with no installation or Effigy QA readiness.
No accounting-only fast path is implemented, and historical causation remains
**UNKNOWN**. Earlier roots and conclusions remain unchanged.

**Smaller-fixture continuation.** Planner direction `49467784` subsequently
authorized one replacement diagnostic using at most 8,192 file names under
the same guard and bounds. The current driver generates 7,172 file names and
7,350 total entries (178 directories), retaining the depth-14 branch, all Cargo
categories, the hardlink, explicit metadata controls and fixed-manifest
negative control. This is a smaller-fixture investigation, not full-scale
evidence, a guard change or a resolution of historical/live admission behavior.

At source `513c6350`, the attempted `proof:matching-slot-cost:diagnostic`
did not launch: Effigy admission run `agq2zrwlkixtivaagxdjjzhr3u` failed closed
because its hook exited 127 before admission. The hook's Nucleus version
`0.0.0-f97727980c2d2e12193d503d1818ff7db834fd82-c96693691f67b1c7aea6a26b498dfc06b5810e30e28084bfcad5fe0017144246`
had no `runtime/node` at its configured path under
`/Users/tom/.local/share/nucleus/versions/`. No guard, fixture or inventory child
ran. The driver interpreter/API check passed, but there is no smaller-fixture
measurement or diagnostic credit. Retained reviewer disposition `d35c74b5`
requires planner-owned environment repair/disposition; the author does not
unset or replace the hook. A repaired route still needs planner confirmation
before execution. The passing diagnostic and original counted occurrence
remain missing; admission, full-scale behavior and historical cause remain
unresolved. All prior failures and kept resources remain unchanged.

The Nucleus owner subsequently confirmed its supported compatibility-link
repair and authorized one diagnostic through the repaired normal heavy route,
without a hook change by this author. At `4576f67e` (executable source
`513c6350`) admission succeeded, but the test failed
`fixture identity/name uncertainty before measurement` and exited 101.
The guard closed with command status 1, no fault or deadline, and Effigy
returned 1. Positive closure did not make the failed fixture a passing
diagnostic; no counted occurrence followed. No inventory timing was produced.

This occurrence's packet is at
`/Volumes/Stash/scratch/oxide/oxide-proof.7t3oonbe/evidence/`
(`receipt.json`, `matching-cost.json`, `matching-cost-child.log`). Guard receipt
SHA-256 is `8a59589b141e900f60db5a784eea75b5ab33bd646eaf4a07f8706a440c3fc950`;
sampled peak was 30,683,136 bytes of 512 MiB and minimum sampled free space was
3,236,558,389,248 bytes. The generated manifest declared 7,350 entries and
7,172 file names, but the fixed-tree precondition refused before measurement.

A post-failure, metadata-only observation of this occurrence found two changed
files: `.DS_Store` inode 52177443 became 52185485, and `debug/.DS_Store` inode
52177445 became 52181764. Both deliberately created 40-byte files were then
6,148 bytes with changed timestamps and allocation. No names were missing,
the root identity matched, and no other manifest mismatch was observed.
The observation is not atomic, does not read payloads or update the oracle,
and does not establish who or what replaced the files. Its recorded result is
`post-failure-metadata-observation.json` in that evidence directory, SHA-256
`64edef91054bafdb6ccd7575c26c9c6e44cbcdc057164f832814d85c86d64d51`.
Retained reviewer disposition `e7b2dd0b` requires returning this unexplained
replacement for planner disposition: no additional attempt or weaker identity
check is authorized. The required counted evidence remains missing. All earlier
failures, UNCLOSED resources and charges remain history; admission and live
readiness remain unrestored, with historical causation **UNKNOWN**.

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
  the fingerprint hint points at the retained
  [Cargo freshness findings](../qualification-readiness.md#cargo-freshness);
- always: this diagnostic is read-only and makes no cleanup recommendation. The
  general policy for adopting a project optimization is in
  [optimization adoption](optimization-adoption.md).

## Validation and evidence limits

Covering repository checks are the ordinary selectors: `effigy rust:compile`,
`effigy rust:format`, `effigy test rust` and `effigy docs:check`. When the
inventory binary or rendered output path changes, also run
`effigy rust:storage-inventory-cli`. The inventory command itself never
invokes Cargo or the compiler, so its fixtures are synthetic and small.

The library unit tests build fresh fixtures under the task-owned scratch root
and cover: known category grouping, a real hardlink across two categories with
union and overlap accounting, a sparse file with apparent greater than
allocated bytes, dangling and directory symlinks that are counted but never
followed, partial coverage when `read_dir` returns an injected
`PermissionDenied`, entry and depth truncation, the cooperative deadline,
bounded rendering (the 64-byte minimum including truncation
markers and the JSON fallback), long or multibyte root/note fields remaining
inside the cap with valid UTF-8/JSON, refused roots, invalid limits, concurrent
mutation without panic, the measured filesystem value and the absence of
stale or savings claims.

The unreadable-directory case injects `io::ErrorKind::PermissionDenied` at the
walker's `read_dir` call for one canonical fixture path. It proves partial
coverage handling; it does not prove kernel permission enforcement or an
actual operating-system `EACCES` response.

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
