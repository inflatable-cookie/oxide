# Qualification findings: freshness, paths, containment and capacity

Historical findings from the 2026-10-09 to 2026-10-16 qualification work that
the storage design rests on. They are not a current qualification or an
execution authority. The machinery that produced them was retired on 2026-10-18.
Raw receipts are kept under [evidence](evidence/), and they authorise nothing.
[Architecture](architecture.md) owns the storage invariants, [vision](vision.md)
owns authority, [questions](questions.md) owns open decisions, and
[workflow](contracts/workflow.md) owns current behaviour.

## Cargo freshness

Pinned Cargo 1.97.1 decides freshness of workspace units from relative paths and
file mtimes.

- An input is fresh when its mtime is at or before the reference mtime, so equal
  timestamps count as fresh.
- Library and test units read package-relative dep-info. Rustc's environment
  dependencies, such as `CARGO_MANIFEST_DIR`, are dropped on the assumption that
  they are tracked elsewhere. They are not.
- Fingerprint paths are workspace-relative, and Cargo does not hash the absolute
  checkout path.
- The unstable checksum-freshness option needs nightly and is not a stable fix.

Observed failure (2026-10-10): a slot built for checkout B was reused for
checkout A. A's unchanged files were older than B's anchors, so Cargo reported
zero rebuilt units. The test ran and failed. The binary held B's compiled value,
while the runtime file was A's baseline. The lease was retained as uncertain.

Proven: Cargo treated older-or-equal relative inputs as fresh across checkouts.
Not proven: that every cross-worktree reuse is impossible, that Cargo
invalidation is generally broken, or that touching sources is a supported fix.
Within one root, Cargo handled a newer input correctly.

Rule: before reusing a slot across checkouts, Oxide must account for source
content, mtimes against the slot's build anchor, Cargo's named inputs, the
build-script environment and the physical root. Incomplete evidence refuses, or
falls back to a checkout-affine slot. See [cross-checkout
freshness](contracts/workflow.md#cross-checkout-freshness) for the current
rules.

## Compiler and behavior paths

Keep three path classes apart:

- **Compiler source identity:** debug information, diagnostics, `file!()` and
  panic locations. The last two are visible to the program.
- **Behaviour-bearing paths:** `CARGO_MANIFEST_DIR`, `OUT_DIR`, `include!`
  targets, build-script and proc-macro inputs and outputs, generated code and
  runtime resource lookup. Path remapping rewrites none of these.
- **Debugger lookup:** a logical path in debug information helps only if it maps
  back to the exact source tree of that run.

Findings on pinned rustc 1.97.1:

- Stable `--remap-path-prefix` is best effort. Its default scope also rewrites
  `file!()` and panic strings. It does not rewrite Apple linker OSO entries.
- Cargo includes rust flags in its freshness fingerprint. A remap whose source
  roots differ between checkouts therefore misses, and adding a root forces a
  cold rebuild.
- Cargo can pass relative source paths, so relative prefixes need remapping as
  well as absolute ones.
- On macOS, the default unpacked split debug info leaves debug data in objects
  referenced from the build directory. An empty executable debug section does
  not mean debug info is off. A full-debug lookup must inspect the companion
  objects or use packed debug info.
- No stable Cargo option presents arbitrary worktrees as one source root.

Consequence: reused workspace units carry the compiling checkout's paths. That
is tolerable for `quick` backtraces and wrong for `debug` sessions after that
checkout is gone. No cheap general remap exists on 1.97.1.

## Cross-path results

- **Same-root fixture (positive, narrow).** At one physical root, a changed task
  reused its slot, kept unchanged dependency and proc-macro work, and executed
  current generated, proc-macro and runtime values. Stale-source and
  post-failure requests were refused before Cargo ran. This was a tiny fixture.
- **Mapped logical roots (negative).** Stale changed-source reuse, described
  above under freshness.
- **Ordinary Cargo across fresh checkout paths (positive, narrow, 2026-10-16).**
  One shared `dev` slot and Nucleus's planning and diagnostics tests. A
  different-path checkout rebuilt only its changed test unit, with the SQLite
  system crate reused, and the tests ran every time. The equal-mtime and
  input-only timestamp cases were not isolated. The [project adoption
  boundary](contracts/project-adoption.md#focused-cross-worktree-result) owns
  the summary.

None of these qualifies general source-path correctness, cross-checkout
freshness or a storage benefit.

## Process containment

Read-only review on native arm64 macOS 26.6.2 (2026-10-09), with a probe on
2026-10-18.

- A dedicated process group (`setpgid` at spawn, `killpg` to signal) is
  supported. A descendant can leave the group with `setsid` or a new spawn
  group.
- `kill(-pgid, 0)` returning `ESRCH` means no member is visible. In the probe,
  `EPERM` meant every remaining member was an unreaped zombie. That is neither
  proof of a live writer nor proof of absence, so it is treated as held and
  retried later. A test harness must reap its own children before probing.
- A start-identity marker plus PID and start time detects PID reuse. A process
  can drop the marker, and whole-second start times accept a same-second reuse.
- `lsof` or cwd scans see only processes that still hold files. `Unknown` is not
  quiet. A writer that closed its descriptors and changed directory disappears
  from the scan.
- `kqueue` `NOTE_TRACK` is unsupported since macOS 10.5. `proc_listchildpids` is
  a racy helper, not a tree proof. `sandbox-exec` is deprecated.
- Inherited sandbox rights are static: a parent's exit does not revoke a
  surviving child's access. Endpoint Security write events are notifications and
  do not revoke an open descriptor. `RLIMIT_FSIZE` caps one file, not a tree. An
  APFS unmount that fails on open files is an observation, not a fence.

No reviewed public mechanism proves that arbitrary descendants, transferred
descriptors, writable mappings and unrelated same-user writers have lost write
access. Group emptiness, PID exit, age and quiet scans are observations only.

What qualified, inside a stated cooperative promise, was the native fixture
boundary and standalone pilot runs with real Cargo, including release and
reacquire. Cancellation, a retained writer and capture mismatch stayed
uncertain.

Consequence for crash recovery: an empty group plus a dropped lock is the
cooperative closure signal that Oxide's own commands may use. It needs a
group-escape test before anything relies on it, and it never extends to
standalone consumer leases.

## Capacity

- **Host reserve.** The 20 GiB free-space figure from Tom's 2026-10-09 ruling is
  a monitored floor, not a guarantee against concurrent writers. Current reserve
  semantics are in [workflow](contracts/workflow.md).
- **Temporary output.** Cargo, rustc, `cc` and tests also write through
  `TMPDIR`, `TMP` and `TEMP`, and integration tests write through
  `CARGO_TARGET_TMPDIR`. A footprint claim must force those into an accounted
  directory. Tools that hard-code `/tmp` remain a residual bypass.
- **Accounting.** Occupied bytes are unique `(device, inode)` `st_blocks × 512`,
  so hard links count once. APFS compression, clones and snapshots make any walk
  approximate. Unknown size is unknown, not zero. Occupied-space deltas are not
  write volume.
- **Hard bound.** A sampled walk is accounting plus abort. The kernel bound used
  until oxide#102 retired it was one fixed-size APFS volume in a sparse image.
  Its proof observed genuine ENOSPC under an open lease, and its control ledger
  stayed durable outside the payload. Sparse image logical size is not host
  allocation, so do not add image and slot figures.
- **Sizes.** Nucleus's planning and diagnostics pair measured about 2.3 GiB of
  unique artifacts under line-table debug info. Full-debug size was not
  measured. Neither figure is representative or a production budget.

The retired 128 MiB lease selector proved the lease lifecycle on a fresh owned
payload. It did not fill the filesystem or qualify a real Cargo footprint.
