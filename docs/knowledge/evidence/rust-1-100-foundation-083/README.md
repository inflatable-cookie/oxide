# Rust/Cargo 1.100 foundation qualification: readiness (oxide#083)

Status: **the one proof ran and FAILED; no 1.100 verdict.** The one
approved `rustup toolchain install 1.100.0 --profile minimal` ran at
`b9138a6` (exit 0, 14.0 s); as a side effect rustup self-updated from 1.29.1
to 1.29.3, which the owner recorded. The single Queue diagnostic then ran
once at `364d605` and failed (see below). That occurrence is spent: no retry,
second occurrence or native follows from this document.

## Spent proof result (`364d605`, Queue run `d9c1616b`)

- **Delivered evidence:** one complete raw log (4,686 B, EOF, no
  truncation), both copied phase receipts and the final capture receipt;
  4 files, 11,643 B, retained by Nucleus with original locators and
  SHA-256. This was the first end-to-end use of the capture and delivery
  route, and it worked.
- **Composition:** phase 1.97.1 closed with status 3 (no fault, no
  deadline); phase 2 1.100.0 then ran and also closed with 3. Capture was
  complete, `qualified: false`. Two of three invocations were used.
- **Observations (sampled, not peaks):** about 37 s in total (phases about
  15.3 s and 14.5 s). Owned peak 170.8 and 171.8 MiB of 402. Host free
  minimum about 2314 GiB.
- **Identities:** `cargo 1.100.0 (3acc79c0c 2026-11-05)`,
  `rustc 1.100.0 (0ef1bc386 2026-11-09)`; `verified: false`, as the
  unchanged table requires.
- **Cases: every focused product case failed or was skipped.**
  - On 1.97.1, `profiles` and `freshness-gate` failed.
  - On 1.100.0, `profiles` and `fallback` failed.
  - Eight dependent cases were skipped in each phase.
  - Cause: the recipe put the fixture pool on the selected external volume
    but did not declare `OXIDE_SCRATCH_ROOT`, so the shipped product's
    external-volume refusal (`config::judge_pool_path`) fired on the first
    leased command. This is a fixture premise defect, not evidence of a 1.100
    product regression.
- **The only case that completed** was the 1.100.0 tiny probe: 7 Cargo
  calls, 0.8 s, 1,015,808 bytes, `rcgu_counts [1,1,1,1,1]`. Stale `*.rcgu.o`
  did not accumulate across four rebuilds of that crate. This is a single
  observation and does not qualify anything.

**No verdict exists** for routing, profiles, checkout-affine and
cross-checkout freshness, crash recovery, group-gone, status,
waiting/interrupt, layout and stray detection, the dep-info decoder (v1 or
reshaped v2) or the unverified-Cargo fallback. Per-phase 300 s fit is still
unmeasured: cold staging took almost all of each phase while the substantive
cases were skipped. The required dual-toolchain focused proof stays failed.

**Source correction after the run:** the runner now declares a private
owner-only `scratch-pools` directory inside the phase parent as
`OXIDE_SCRATCH_ROOT`, with the fixture pool strictly inside it, following
`rust:front-door-accounting`. Self-check controls pin it. It has **not been
exercised natively**: correcting the source does not reopen the spent
occurrence, and any further run needs a separate owner decision.

## Release and source facts

- Release post: <https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/>.
- Channel manifest (owner-verified 2026-11-13):
  <https://static.rust-lang.org/dist/channel-rust-1.100.0.toml>, SHA-256
  `3051e859570f64a8c5acb9d09e87425db52c7d97287c67e8f6d57d7d34ef9258`,
  `rustc 1.100.0 (0ef1bc386 2026-11-09)`, rust commit
  `0ef1bc386059596dada8fb65b50425aab2ea637a`.
- Cargo pin (read 2026-11-14): the `src/tools/cargo` gitlink at that rust
  commit is `3acc79c0c841844675bc53a605922dbfc441e750` (2026-11-05,
  <https://api.github.com/repos/rust-lang/rust/contents/src/tools/cargo?ref=0ef1bc386059596dada8fb65b50425aab2ea637a>).
  The manifest's `pkg.cargo.version = "0.101.0 (3acc79c0c 2026-11-05)"`
  agrees. Expected `cargo -V`: `cargo 1.100.0 (3acc79c0c 2026-11-05)`; the
  `1.100.0` part is convention until the installed binary shows it.

Facts at Cargo `3acc79c0c` (Cargo moved `src/cargo/core/compiler/` to
`src/compiler/` before this pin):

| Fact (workflow, Cargo version matrix) | 1.100.0 | Source |
| --- | --- | --- |
| `find_stale_file`: `path_mtime <= reference_mtime` is fresh | same | `src/compiler/fingerprint/mod.rs` |
| Encoded dep-info marker `01 00 00 00 ff`, version 1 | same | `src/compiler/fingerprint/dep_info.rs` (`CURRENT_ENCODED_DEP_INFO_VERSION: u8 = 1`) |
| Local units hash workspace-relative paths | same (docs and `strip_prefix` calls; hash code not traced) | `fingerprint/mod.rs` |
| `cargo metadata` format 1 | same | `doc/book/src/commands/cargo-metadata.md` |
| `build.build-dir` stable | same | `src/workspace/features.rs` |
| Default layout keeps `<profile>/deps`, `incremental`, `.fingerprint` | **changed**: layout v2 is the default; only `incremental` and `build/` remain; per-unit `build/<pkg>/<meta>/{out,fingerprint,run}`; `-Zbuild-dir-new-layout` stabilized with a warning; opt-out only via the private `__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT` | `src/workspace/features.rs`, `src/compiler/layout.rs` (`BuildDirLayout::prepare`) |

The Cargo 1.100 changelog lists the layout change (#17354) and no entry on
checksum freshness, cleanup/gc or dep-info format (first 100k characters read).

### Consequence: no `VERIFIED_CARGO` entry proposed

One source fact changed, so 1.100.0 cannot join the table as is. Stray
detection keys on `deps/` and `incremental/`; a release build or any unit
without incremental leaves neither under v2, so a verified 1.100 would print
`none` over real intermediates. Slot dep-info for the gate also moves to
`build/<pkg>/<meta>/fingerprint/`. Making 1.100 verified therefore needs a
layout-v2-aware detection change and a gate path review — a freshness-adjacent
change outside this task's tiny-correction scope. Until then the shipped
behaviour on 1.100 is the fallback: affine slots, and stray output reads `holds` (incremental files) or `unknown`, never `none`. That is
what the recipe tests.

## Prior input

oxide#081 (`apfs-clone-relocation-20261018/summary.json`) observed layout v2
only on nightly 2026-08-05 and gives no 1.100 credit.

## Recipe

`scripts/foundation-rust-1-100.py`:

- `effigy proof:foundation:rust-1-100:plan`: the effect-free plan.
- `effigy proof:foundation:rust-1-100:test`: 191 pure controls.
- `effigy proof:foundation:rust-1-100:diagnostic` (heavy):
  `python3 scripts/foundation-rust-1-100.py --compose`, one fixed
  task-specific capture producer.

`--compose` does, in order:

1. Creates one fresh capture directory under `TMPDIR` and records the source
   (HEAD, tree, tracked-dirty state, guard digest).
2. Runs the gate in-process. It refuses unless the planner issued
   `FOUNDATION_083_BINDING` (JSON naming `task`, `selector`, exact `head`,
   `decision`) with a matching `FOUNDATION_083_BINDING_SHA256` and both
   toolchains are installed. A refusal runs no phase.
3. Runs phase 1:
   `python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 402 -- python3 scripts/foundation-rust-1-100.py --diagnostic --toolchain 1.97.1`.
   Its merged output is copied to `raw.log` only (no external mirror), drained
   to EOF. Each phase re-checks the binding inside the guard before any
   effect and prints its evidence directory.
4. Measures phase 1's retained evidence directory. Phase 2 (`1.100.0`) runs
   only when phase 1 returned 3 and capture is complete: EOF seen, raw log
   not truncated, evidence measured at no more than 32 MiB.
5. Writes `capture.json` once, atomically (at most 256 KiB), carrying the final status; a failed or oversized write returns 6. The final exit is
   phase 2's own status, the stopping phase's status, or 6 when capture is
   incomplete (truncated, no EOF, lost stream, oversized or unmeasured
   evidence, or receipt not written). The third invocation is never spent.

**Clock.** The guard exports `OXIDE_PROOF_DEADLINE_NS` as its absolute
deadline (start + 300 s) in `CLOCK_UPTIME_RAW` nanoseconds. Each phase reads
the same clock: every legacy `remaining()` is replaced by
deadline − now − 30 s stop reserve, read fresh. Setup time is charged and an
already-spent window refuses before any effect.

**Phase statuses.** The guard passes the command's own status through only
for a closed run within its bounds; otherwise it returns 124 (deadline), 125
(fault, refusal, UNCLOSED or missing status) or 128+N (signal). The
diagnostic returns:

- 3: receipt written, collection not aborted; ordinary case failures stay
  keep-going results inside it;
- 4: setup fault or aborted collection;
- 5: no receipt written;
- 2: refused.

### Whole-capture accounting (original 512 MiB)

**Parent liveness.** The producer mirrors nothing to an external stream.
Each phase's output is read with `select` plus non-blocking reads that never
wait past that phase's original 300 s window (`bounded_chunks`). The child
wait is capped at what is left of the window, and no signal is sent. A
window reached before EOF, or a guard still running at its end, is recorded
as incomplete or unknown custody (status 6, or 125 when the status is
missing), and no later phase runs. Status lines to stdout are best effort and
non-blocking, so a full pipe drops them. The evidence walk stops at 4096
entries and reports unmeasured. Residual calls outside a phase window: the
gate's and source record's `git` reads (10 s timeout each), the evidence
walk, the chunk copies and the one `capture.json` write with fsync. All are
small local operations, disclosed rather than clocked.

**Delivery.** The raw log is capped at 24 MiB, within the original 32 MiB
maximum. At finalization it is copied into at most three `raw.log.part-NN`
chunks of 8 MiB or less: copies of the one log, not a second log. Each phase
receipt that ran is copied into the capture leaf as `receipt-<toolchain>.json`,
with its original path, size and SHA-256 recorded. A missing, symlinked,
non-regular, unreadable or oversized (over 1 MiB) receipt, or a failed copy,
makes the capture incomplete (6); it is never silently dropped.

The recorded delivery set gives absolute paths under the recorded
`capture_dir`: the chunks, the two receipt copies and a reserved 256 KiB
`capture.json` entry. That is at most 6 files and about 26.3 MiB, within
Nucleus proofFiles limits of 8 MiB per file, 32 MiB total and 12 files.
`delivery_plan` refuses an oversized file, an extra copy, more than 32 MiB or
more than 12 files, and that refusal also gives status 6.

**Read completeness.** A non-blocking read with no data (spurious readiness)
is retried inside the same window and is never treated as EOF; only a real
empty read is EOF. The synthetic phase status line goes through the same
capped writer, and if it does not fit, the phase capture is marked truncated
(status 6).

| Item | MiB | Bound |
| --- | --- | --- |
| Raw log | 24 | enforced by the capture writer; overflow is marked, status 6 |
| Capture dir: chunk copies + receipt copies + capture.json slack | 27 | 24 + 2 + 1 |
| Retained phase-1 evidence (while phase 2 runs) | 32 | measured before phase 2; over → status 6, no phase 2 |
| Nucleus delivery copies (after exit) | 27 | chunks 24 + capture 1 + receipts 2 |
| **Guard budget per phase** | **402** | 512 − 24 − 27 − 32 − 27; pool cap 128 fits inside |

| Branch | All owned (MiB) |
| --- | --- |
| Phase 1 live | 402 + 51 = 453 |
| Phase 1 UNCLOSED (scratch kept) + delivery | 453 + 27 = 480 |
| Phase 2 live | 32 + 402 + 51 = 485 |
| Phase 2 UNCLOSED (scratch kept) + delivery | 32 + 402 + 51 + 27 = 512 |
| Both closed + delivery | 32 + 32 + 51 + 27 = 142 |

Remaining premises, not derived maxima:

- the guard's owned-bytes monitor is sampled, so it bounds the live root only
  at its sampling cadence;
- a closed guard removes its scratch and keeps its evidence (guard source);
- the guard persists at most 8 MiB per output stream into evidence;
- delivery copies are the raw log and receipts only.

The diagnostic's after-case `owned_peak` is a sample, not a physical peak.
Staging bytes and per-phase 300 s runtime fit remain **unmeasured**.
Historical `rust:oxide-cli` (65 s) and `cargo-matrix` (33–67 s) ran on the
retired image pool and give no current figure.

### Cases

| Case | Toolchains | Body | Origin |
| --- | --- | --- | --- |
| profiles | both | `case_profiles` | rewritten from `prove_native`: no APFS init/cap conflict, binary at `target/debug` (oxide#160), budget-only summary |
| routing | both | `case_routing` | rewritten from `prove_routing`: external `$CARGO` build dir from the ledger's recorded slot dir (oxide#104), test/run exits, virtual-workspace and member `default-run`, passthrough and refusals leave the ledger unchanged |
| status | both | `case_status` | rewritten: current `slot_line` format, default status writes nothing |
| clean-cross-checkout | both | `case_clean_cross` | rewritten from `prove_native` tail: clean detaches affinity; second checkout shares through the gate (verified) or stays affine (unverified); old project-cap-of-2 oracle dropped (cap is 12 since oxide#118) |
| waiting | both | `prove_waiting`, `prove_clean_waits` | reused |
| interrupt | both | `prove_interrupt` | reused |
| crash-recovery | both | `prove_recovery` | reused |
| group-gone | both | `prove_group_gone` | reused |
| freshness-gate | 1.97.1 | `prove_freshness` (+`prove_gate_crash`) | reused, verified Cargo only |
| layout | 1.97.1 | `case_layout` | `prove_layout` with the stray line read from `oxide status --measure` |
| fallback | 1.100.0 | `case_fallback` | rewritten from `prove_fallback` for the tested Cargo itself (no third toolchain) |
| layout-observation | 1.100.0 | `case_layout_observation` | new: measured stray `holds` or `unknown` (never `none`), observed plain-target dirs, shipped decoder over v1 and reshaped v2 dep-info |
| probes | 1.100.0 | `case_probes` | new: rcgu baseline + four rebuilds |

Reused helpers run through a `legacy.Runner` subclass whose `lease_status()`
reads `ledger.json` (each row also carries the retired CLI's `slot_id` name,
which `prove_recovery` reads) and whose `state()` reads
`oxide-managed/state.json`; the
self-check proves none of them calls the retired lease CLI, a standalone
lease, `hdiutil` or APFS text. Dropped: the standalone-lease-never-recovered
sub-case and the APFS backend/cap paragraphs, whose product surfaces were
retired by oxide#102.

The v2 dep-info files are copied byte for byte into
`.fingerprint/<n>/dep-*` so the shipped walker (`examples/dep_info_matrix.rs`)
finds them; no Rust or decoder change. Zero files found is insufficient,
never a pass.

Stray lines are read only from `oxide status --measure`: recorded status
prints `not measured by status` (`main.rs` `print_checkout`). On an
unverified Cargo `report_stray` prints `holds` when `deps/` or `incremental/`
holds files, else `unknown`. Layout v2 still creates `<profile>/incremental`,
so a dev-profile plain build may read `holds`; the oracle accepts either,
refuses `none`, and records which.

### Allocation (ceilings, not measurements)

Per phase: staging target and copied `oxide`, private `CARGO_HOME`,
directory pool (`OXIDE_POOL_CAP=128MiB`), fixtures ≤ 8 MiB, probe ≤ 64 MiB,
receipt ≤ 1 MiB. Owned bytes are checked after every case against 512 MiB and
the 20 GiB reserve is sampled before and after every case; the guard's own
monitor is the continuous bound. Probe Cargo calls are counted against 8
(2 version + baseline + 4 rebuilds = 7; the version pair is the tested
toolchain identity check). The 90 s probe window starts when the probe case
starts, after staging and the other cases, and each probe call's timeout is
clipped to what is left of it. Plain Cargo calls inside the focused invocations
(staging and builder identity, the 1.97.1 fixture identity pair,
`generate-lockfile` per fixture, routing's plain `cargo test` and one plain
`cargo check` per layout case) are bounded by the phase guard like the original proofs and
listed by `:plan`. Two receipts stay inside the 32 MiB
raw-log budget. Nothing is deleted in-script: pool, slots and staging stay in
the guard root for its post-leader census. The toolchain install footprint
(473,039,609 B logical, 480,686,080 B allocated, internal Data volume; APFS
point observations, not a peak) belongs to the separate install authority and
is not proof scratch.

### Premise checklist

Recorded per item in `PREMISES` and printed by `:plan`: 1–3 through
`legacy.scrubbed`, `legacy.Runner` and `fixture_environment`; 4 no drive
registered; 5 the probe subcommand ignores argv; 6 no image; 7 `RUSTFLAGS=''`
and `CARGO_ENCODED_RUSTFLAGS` removed in staging and fixtures; 8–9 private
`tempfile.mkdtemp` under the guard's `OXIDE_TASK_SCRATCH_ROOT`, never the
legacy `/private/tmp` `mktemp`. Fixtures pin 1.97.1 in `rust-toolchain.toml`;
`RUSTUP_TOOLCHAIN` overrides it.

## Not run

- The single diagnostic **did run**, once at `364d605`, and **failed** (see
  "Spent proof result"). That occurrence is spent.
- **No native has run since the fixture correction**, so the declared
  `OXIDE_SCRATCH_ROOT` route is unexercised. Every per-area 1.100 verdict
  above stays absent, and per-phase fit stays unmeasured.
- `rust:compile`, `rust:compile-release`, `rust:format` and `test rust` were
  not run because no Rust changed.
- The one approved install ran at `b9138a6` and is not repeated.

Run at this head: `:test` (191/191) and `docs:check`.
