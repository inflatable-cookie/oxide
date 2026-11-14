# Rust/Cargo 1.100 foundation qualification: readiness (oxide#083)

Status: **source-complete for review; not run.** No toolchain was installed
and no compiler, Cargo, staging, fixture, pool, image or proof ran. No 1.100
behaviour verdict exists yet.

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
behaviour on 1.100 is the fallback: affine slots, stray `unknown`. That is
what the recipe tests.

## Prior input

oxide#081 (`apfs-clone-relocation-20261018/summary.json`) observed layout v2
only on nightly 2026-08-05 and gives no 1.100 credit.

## Recipe

`scripts/foundation-rust-1-100.py`:

- `effigy proof:foundation:rust-1-100:plan`: the effect-free plan.
- `effigy proof:foundation:rust-1-100:test`: 78 pure controls.
- `effigy proof:foundation:rust-1-100:diagnostic` (heavy): a pre-guard gate,
  then two separately guarded sequential phases, then `exit 1`.

Command (also pinned by the self-check against `effigy.toml`):

```sh
python3 scripts/foundation-rust-1-100.py --gate --binding "$FOUNDATION_083_BINDING" --sha256 "$FOUNDATION_083_BINDING_SHA256" && { python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/foundation-rust-1-100.py --diagnostic --toolchain 1.97.1; python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/foundation-rust-1-100.py --diagnostic --toolchain 1.100.0; exit 1; }
```

The gate refuses before the guard creates anything unless the planner issued
a binding JSON (`task`, `selector`, exact `head`, `decision`) with a matching
SHA-256 and both toolchains are installed. Each phase re-checks the binding
inside the guard before any effect.

The whole command cannot be shown to fit one 300 s clock: each phase stages
`oxide` and runs every retained case. It therefore uses the smallest existing
composition: two separately guarded phases of the ordinary qualified guard
(`fcfda2b0…`), each with the original 300 s / 512 MiB, two of the three
allowed invocations, in ONE Queue occurrence. The third invocation is never
spent by the selector. **Fit is unmeasured**; historical `rust:oxide-cli` (65 s)
and `cargo-matrix` (33–67 s) ran on the retired image pool and give no current
figure.

### Cases

| Case | Toolchains | Body | Origin |
| --- | --- | --- | --- |
| profiles | both | `case_profiles` | rewritten from `prove_native`: no APFS init/cap conflict, binary at `target/debug` (oxide#160), budget-only summary |
| routing | both | `case_routing` | rewritten from `prove_routing`: external `$CARGO` build dir from the ledger's recorded slot dir (oxide#104), test/run exits, passthrough and refusals leave the ledger unchanged |
| status | both | `case_status` | rewritten: current `slot_line` format, default status writes nothing |
| clean-cross-checkout | both | `case_clean_cross` | rewritten from `prove_native` tail: clean detaches affinity; second checkout shares through the gate (verified) or stays affine (unverified); old project-cap-of-2 oracle dropped (cap is 12 since oxide#118) |
| waiting | both | `prove_waiting`, `prove_clean_waits` | reused |
| interrupt | both | `prove_interrupt` | reused |
| crash-recovery | both | `prove_recovery` | reused |
| group-gone | both | `prove_group_gone` | reused |
| freshness-gate | 1.97.1 | `prove_freshness` (+`prove_gate_crash`) | reused, verified Cargo only |
| layout | 1.97.1 | `prove_layout` | reused |
| fallback | 1.100.0 | `case_fallback` | rewritten from `prove_fallback` for the tested Cargo itself (no third toolchain) |
| layout-observation | 1.100.0 | `case_layout_observation` | new: stray `unknown`, observed plain-target dirs, shipped decoder over v1 and reshaped v2 dep-info |
| probes | 1.100.0 | `case_probes` | new: rcgu baseline + four rebuilds |

Reused helpers run through a `legacy.Runner` subclass whose `lease_status()`
reads `ledger.json` and whose `state()` reads `oxide-managed/state.json`; the
self-check proves none of them calls the retired lease CLI, a standalone
lease, `hdiutil` or APFS text. Dropped: the standalone-lease-never-recovered
sub-case and the APFS backend/cap paragraphs, whose product surfaces were
retired by oxide#102.

The v2 dep-info files are copied byte for byte into
`.fingerprint/<n>/dep-*` so the shipped walker (`examples/dep_info_matrix.rs`)
finds them; no Rust or decoder change. Zero files found is insufficient,
never a pass.

### Allocation (ceilings, not measurements)

Per phase: staging target and copied `oxide`, private `CARGO_HOME`,
directory pool (`OXIDE_POOL_CAP=128MiB`), fixtures ≤ 8 MiB, probe ≤ 64 MiB,
receipt ≤ 1 MiB. Owned bytes are checked after every case against 512 MiB and
the 20 GiB reserve is sampled before and after every case; the guard's own
monitor is the continuous bound. Probe Cargo calls are counted against 8
(2 version + baseline + 4 rebuilds = 7). Two receipts stay inside the 32 MiB
raw-log budget. Nothing is deleted in-script: pool, slots and staging stay in
the guard root for its post-leader census. The toolchain install footprint is
reported separately when the install runs.

### Premise checklist

Recorded per item in `PREMISES` and printed by `:plan`: 1–3 through
`legacy.scrubbed`, `legacy.Runner` and `fixture_environment`; 4 no drive
registered; 5 the probe subcommand ignores argv; 6 no image; 7 `RUSTFLAGS=''`
and `CARGO_ENCODED_RUSTFLAGS` removed in staging and fixtures; 8–9 private
`tempfile.mkdtemp` under the guard's `OXIDE_TASK_SCRATCH_ROOT`, never the
legacy `/private/tmp` `mktemp`. Fixtures pin 1.97.1 in `rust-toolchain.toml`;
`RUSTUP_TOOLCHAIN` overrides it.

## Not run

Every native, the toolchain install, `rust:compile`, `rust:compile-release`,
`rust:format` and `test rust` (no Rust changed). Run: `:test` (78/78),
`:plan`, `docs:check`.
