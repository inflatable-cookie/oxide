# Qualification findings: freshness, paths, containment and capacity

This file keeps the findings from the 2026-10-09 to 2026-10-16 qualification
work that the proposed [workflow](contracts/workflow.md) builds on. They are
historical evidence, not a current qualification or execution authority. The
machinery that produced them was retired on 2026-10-18
([retirements](retired.toml)); the full narrative stays in Git history and the
raw receipts stay under [`evidence/`](evidence/). [Architecture](architecture.md)
owns the storage invariants, [vision](vision.md) owns authority and
[questions](questions.md) owns open decisions.

## Cargo freshness

Pinned Cargo 1.97.1 (`c980f4866`) fingerprints workspace units by relative path
and file mtime. Evidence: the preserved v4 fixture root
`/private/var/tmp/oxide-rust-probe-proof-613d9b91-v4` and the Task 018
diagnosis of the Cargo fingerprint source.

- `find_stale_file` treats an input as fresh when `path_mtime <=
  reference_mtime`, so equal timestamps are also a hit.
- A build script's `rerun-if-changed` inputs are recorded relative to the
  package (`RerunIfChanged {output, paths: ["marker.txt"]}`) and compared
  against the mtime of the script's `output` file.
- Library and test units use `CheckDepInfo` with `checksum: false`. Encoded
  dep-info lists package-relative sources and slot-relative generated files.
  `translate_dep_info` drops rustc-synthesised environment dependencies such
  as `CARGO_MANIFEST_DIR`, on the assumption they are tracked elsewhere. They
  are not.
- Fingerprint `path` is workspace-relative. Cargo never hashes the absolute
  checkout path, so that a renamed project directory can reuse artifacts
  ([fingerprint module](https://github.com/rust-lang/cargo/blob/c980f4866/src/cargo/core/compiler/fingerprint/mod.rs)).
- Unstable [checksum-freshness](https://doc.rust-lang.org/cargo/reference/unstable.html#checksum-freshness)
  needs nightly, replaces rustc dep-info mtimes only, and leaves build-script
  `RerunIfChanged` on mtimes. It is not a stable 1.97.1 fix.

Observed failure (v4, 2026-10-10): checkout B built a changed marker on a slot;
a later request at checkout A reused that slot. A's unchanged files were older
than B's anchors, so Cargo reported zero rebuilt units. The test executed and
failed: the compiled value was B's `changed-marker`, the runtime file A's
baseline. Generated `OUT_DIR` code and dep-info still named B's paths. The
lease became uncertain and was retained.

Proven: Oxide admitted an idle slot across compile-origin trees and roots, and
Cargo treated older-or-equal relative inputs as fresh. Not proven: that every
cross-worktree reuse is impossible, that Cargo invalidation is generally
broken, or that touching sources or editing fingerprints is a supported fix.
Within one root, Cargo did the right forward miss: a newer relative input reran
the build script and rebuilt the affected units.

Consequence for W-8: before reusing a slot across checkouts, Oxide must
account for source content, mtimes against the slot's build anchor, Cargo's
named inputs, build-script environment and the physical root. Incomplete
evidence refuses or falls back to a checkout-affine slot.

## Compiler and behavior paths

Standalone pilot run 5 (2026-10-09, [review](https://github.com/inflatable-cookie/oxide/pull/9#issuecomment-6088502764))
reused a released slot at checkout B with zero rebuilt units; tests ran and
passed. The reused `planning` test binary held 484 occurrences of the
checkout-A path and none of B; `build_diagnostics` held 56 and none. Review
classified them as rustc source names for debug and panic metadata, with no
`CARGO_MANIFEST_DIR` string or B load path. That failed the strict path proof
without showing wrong-source execution on identical trees.

Keep three path classes apart:

- **Compiler source identity:** debug information, diagnostics, `file!()` and
  panic locations. The last two are program-visible values.
- **Behavior-bearing paths:** `CARGO_MANIFEST_DIR`, `OUT_DIR`, `include!`
  targets, build-script and proc-macro inputs and outputs, generated code and
  runtime resource lookup. Path remapping rewrites none of these.
- **Debugger lookup:** a logical path in debug information is useful only if
  it maps back to the exact source tree of that run.

Remapping findings (Tasks 014–017):

- Pinned rustc 1.97.1 accepts stable `--remap-path-prefix` and
  `--remap-path-scope`. The default scope also rewrites `file!()` and panic
  strings. Remapping is best effort and does not rewrite Apple linker OSO
  entries.
- Cargo 1.97.1 includes Rust flags in its freshness fingerprint. A map whose
  source roots differ between checkouts therefore misses; only an identical,
  complete, ordered flag vector at every checkout can hit, and adding a root
  is a cold rebuild.
- Cargo can pass relative source paths, so relative prefixes need remapping as
  well as absolute ones.
- On macOS the default `split-debuginfo = "unpacked"` leaves debug data in
  objects referenced from the build directory; an empty executable debug
  section does not mean debug information is off. A full-debug lookup must
  inspect the exact companion or use an explicit `packed` profile.
- No stable Cargo option presents arbitrary worktrees as one source root.

Consequence for the workflow: reused workspace units carry the compiling
checkout's paths. That is tolerable for `quick` backtraces and wrong for
`debug` sessions after that checkout is gone. No cheap general remap exists on
1.97.1.

## Cross-path results

- **Same-root fixture (Task 021, positive).** At one physical root, a changed
  task reused its slot, kept unchanged dependency and proc-macro work, and
  executed current generated, proc-macro and runtime values; stale-source and
  post-failure requests refused before Cargo. Review at
  `faf1490484af174dcb5ec4e98b116c408d2680ca`
  ([verdict](https://github.com/inflatable-cookie/oxide/pull/21#issuecomment-6101426638)).
  Tiny-fixture evidence only; the [Nucleus core reuse record](contracts/nucleus-core-reuse.md)
  keeps its limits.
- **Mapped logical roots (v4, negative).** Stale changed-source reuse, above.
- **Ordinary Cargo across fresh checkout paths (#064, positive, narrow).** One
  shared `dev` slot and the pinned Nucleus planning/diagnostics workflow: a
  different-path checkout rebuilt only its changed test unit, with
  `libsqlite3-sys` fresh, and tests ran every time. The old/equal-mtime case
  was not isolated. The [project adoption boundary](contracts/project-adoption.md#focused-cross-worktree-result)
  owns its summary; the raw receipt is
  [`oxide-064-cross-path-qualification.json`](evidence/oxide-064-cross-path-qualification.json).

None of these qualifies general source-path correctness, cross-checkout
freshness or a storage benefit claim.

## Process containment

Read-only readiness review, native arm64 macOS 26.6.2, 2026-10-09:

- A dedicated process group (`POSIX_SPAWN_SETPGROUP` / `setpgid`, then
  `killpg`) is supported. `ESRCH` from `kill(-pgid, 0)` means no member is
  currently visible. A descendant can leave with `setsid` or a new spawn group.
- A start-identity environment marker plus PID and start time detects PID
  reuse. A process can drop the marker; whole-second start times accept a
  same-second reuse.
- `lsof`/cwd scans of the slot see only processes still holding files there.
  `Unknown` is not quiet. A writer that closed its descriptors and changed
  directory disappears.
- `kqueue` `NOTE_TRACK` is unsupported since 10.5. `NOTE_EXIT`/`NOTE_FORK` and
  `proc_listchildpids` are racy helpers, not a tree proof. `sandbox-exec` is
  deprecated.
- Inherited App Sandbox rights are static; parent exit does not revoke a
  surviving child's access or an open descriptor.
- Endpoint Security's write event is notification-only, `AUTH_OPEN` does not
  revoke an existing descriptor, the descendant client is preliminary and
  blind to unrelated processes, and the entitlement and system extension are a
  large host boundary.
- An APFS unmount failing on open files is an observation, not a fence.
- `RLIMIT_FSIZE` caps one file, not a tree.

No reviewed public mechanism proves that arbitrary descendants, transferred
descriptors, writable mappings and unrelated same-user writers have lost write
access. Group emptiness, PID exit, age and quiet scans are observations.

What did qualify, inside a stated cooperative workflow promise: the native
fixture boundary (six scenarios, review at
`1f76bc1f5d3ee8cdbfc552abecb3c1c30d82eda1`,
[PR8](https://github.com/inflatable-cookie/oxide/pull/8#issuecomment-6083800151))
and standalone pilot S1–S3 with real Cargo, including release and reacquire.
Cancellation, a retained writer and capture mismatch stayed uncertain.

Consequence for crash recovery: an empty group plus a dropped lock is the
cooperative signal Oxide's own commands may use under Tom's 2026-10-18 ruling.
It needs the group-escape test named in W-4 and never extends to standalone
consumer leases.

## Capacity

- **Host reserve.** Tom set 20 GiB free host space as a monitored start/abort
  threshold on 2026-10-09, not a guarantee against concurrent writers.
- **Temporary output.** Cargo, rustc, `cc` and tests also write through
  `TMPDIR` (default per-user `/var/folders/…/T/`), `TMP` and `TEMP`, and
  integration tests through `CARGO_TARGET_TMPDIR`. A footprint claim must force
  those into an accounted directory. Tools that hard-code `/tmp` remain a
  residual bypass.
- **Accounting.** Occupied bytes are unique `(device, inode)` `st_blocks *
  512`; file lengths over-count hard links. APFS compression, clones and
  snapshots make any walk approximate. Unknown size is unknown, not zero.
  Occupied-space deltas are not write volume.
- **Hard bound.** A sampled walk is accounting plus abort. The kernel bound
  Oxide uses is one fixed-size APFS volume in a sparse image
  ([bounded storage](contracts/bounded-storage.md)); its oxide#055 proof
  observed genuine ENOSPC under an open lease with the control ledger outside
  the payload staying durable. Sparse image logical size is not host
  allocation; do not add image and slot figures.
- **Sizes.** The Nucleus planning/diagnostics pair measured about 2.3 GiB of
  unique artifacts under line-table debug information; full-debug size was
  not measured. The #064 run's union footprint was about 3.46 GB. Neither is a
  representative workload or a production budget.

The 128 MiB lease selector proves the current lease lifecycle on a fresh owned
payload. It does not fill the filesystem or qualify a real Cargo footprint.
[Local lease](contracts/local-lease.md) owns current capacity semantics.

## Preserved receipts

- [`oxide-064-cross-path-qualification.json`](evidence/oxide-064-cross-path-qualification.json):
  reviewed ordinary-Cargo cross-path session and limits.
- [`cargo-unit-graph-and-tool-fit-041.json`](evidence/cargo-unit-graph-and-tool-fit-041.json):
  Cargo unit-graph and tool-fit findings.
- [`cargo-graph-route-and-installation-evidence-042.json`](evidence/cargo-graph-route-and-installation-evidence-042.json):
  route and installation evidence boundary.
- [`nucleus-core-test-behavior-audit-040.json`](evidence/nucleus-core-test-behavior-audit-040.json):
  read-only Nucleus core-test behavior audit.
- Registry archive receipts 037–039: see
  [Nucleus input roots](contracts/nucleus-input-roots.md).

These receipts do not authorize a live project probe, installation, consumer
change or host mutation.
