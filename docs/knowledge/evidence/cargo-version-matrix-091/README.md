# Cargo version matrix, stable 1.95–1.99 (oxide#091, C1)

Scope: the shipped freshness gate, stray ordinary-Cargo output detection and
dep-info decoder. No skip path, build-script reader or donor code was used.
There is no claim about consumer benefit, real projects or Linux runtime.

## Result

| Release | Cargo | Source check | Runtime proof |
| --- | --- | --- | --- |
| 1.95.0 | `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` | unchanged | run failed in the shared 1.94.1 fallback phase on a wrong script expectation. Every 1.95.0 phase before it passed. |
| 1.96.1 | `cargo 1.96.1 (356927216 2026-06-26)` | unchanged | pass, 46 s |
| 1.97.1 | `cargo 1.97.1 (c980f4866 2026-06-30)` | baseline | pass, 67 s |
| 1.98.1 | `cargo 1.98.1 (797e8a9bc 2026-08-05)` | unchanged | pass, 35 s |
| 1.99.1 | `cargo 1.99.1 (5f94df478 2026-08-27)` | unchanged (import paths moved) | pass, 33 s |

Each release is the latest stable patch of its minor in the official channel
manifests fetched 2026-10-19 (`raw/channel-manifests.txt`; the sha256 of each
manifest matched its published checksum). `channel-rust-1.100.toml` returned 404.

Each proof (`effigy rust:cargo-matrix:<release>`) staged `oxide` and the
decoder driver offline with builder 1.97.1. It then ran the tested Cargo over
one fresh 128 MiB APFS pool through these phases:

- the full two-worktree freshness gate scenario: path change, stale mtime,
  unchanged repeat, refused clone, crash inside the gate;
- effective `build.build-dir`: no intermediates in the checkout target, and
  `deps/` plus `.fingerprint/` in the slots;
- `oxide status` stray output: `none` after Oxide alone, and reported after a
  plain `cargo check`;
- the shipped decoder: it decoded every `.fingerprint/*/dep-*` file, 24 from
  the slots and 8 from the plain target, with 0 refused;
- the fallback phase on installed 1.94.1.

Run 1 (1.95.0) expected worktree B to be refused A's affine 1.94.1 slot. The
store never offers an affine slot to another checkout (`store.rs`, shareable
origins are `Built::Content` only), so B built cold on its own affine slot.
The behaviour was right and the expectation was wrong. The script was fixed
and runs 2–5 passed that phase. Run 1 remains a failed run; its 1.95.0
feature phases are in `raw/proof-1.95.0.log`. This is the author-time ledger
(5 of 6 submissions); the independent review consumed the sixth below. The
`remaining_shared_extra` field in `summary.json` is preserved as an author-time
observation, not a current allowance.

## Independent review and final coverage boundary

The independent reviewer used the sixth and final shared submission for
1.95.0 at `936dee41f206202c930253723a3014efb0d6fa49`: request
`dfbf16ed-77bc-480e-a75f-5c187682f6ac`, native exit 0, 24,599 ms. It passed
the version-specific phases and the corrected 1.94.1 fallback control.
All six original submissions were authenticated from their original
scheduler settlements under caller `interactive:501`, epoch 1. The earlier
wrong-caller `unknown_run` observations are not missing native outcomes.
**Six of six submissions are spent.** The original failed run stays failed.

Review [6270571882](https://github.com/inflatable-cookie/oxide/pull/88#issuecomment-6270571882)
then found that the version query ran in the workspace directory while the
guarded command inherited its caller's directory. The final repair makes the
query inherit the same directory and adds a focused regression.

Independent Sol review
[6270741311](https://github.com/inflatable-cookie/oxide/pull/88#issuecomment-6270741311)
accepted all four subjects at exact final head
`a16dd7c807e0769e16f931d8a051fdd00f11b2c9`. Compile, format, Rust tests and
docs checks passed there. It authenticated the complete retained matrix and
fresh official source/checksum pins without launching a seventh proof.

The final directory-context repair has source and unit coverage, not a new
native member-toolchain experiment. Every retained matrix phase explicitly
selected `RUSTUP_TOOLCHAIN`, so its Cargo selection is unchanged by that
repair. Earlier native runs are historical evidence; they are not described
as executions of the final head. The exact-pair support boundary and the
absence of skip-path, reader, consumer or Linux-runtime qualification remain.

## Source boundary

`raw/cargo-source-files.sha256` pins each file read at the full Cargo commit.
In all five pins:

- the encoded dep-info format is the same: marker `01 00 00 00 ff`, version 1;
- `find_stale_file` treats `path_mtime <= reference_mtime` as fresh;
- local fingerprints hash workspace-relative `path_args`;
- `cargo metadata` uses format 1;
- the lock refusal and the unknown `-Z` refusal keep their wording;
- `build.build-dir` has no unstable gate;
- the stable layout keeps `<profile>/deps`, `incremental` and `.fingerprint`,
  while the unit-grouped layout stays behind `-Zbuild-dir-new-layout`.

1.95.0 names its build lock `.cargo-lock`; later releases use
`.cargo-build-lock` and `.cargo-artifact-lock`. None of the three features
reads lock files.

## Fallback

`VERIFIED_CARGO` in `src/bin/oxide/freshness.rs` holds exactly these five
`cargo -V` prefixes, commit hashes included. Any other Cargo is unverified,
including 1.96.0 and 1.97.0:

- a new slot stays affine to its checkout;
- a content slot that needs refreshing is refused and the command builds cold;
- stray detection that finds nothing reports `unknown`.

The decoder still refuses any other marker or version.

## Host effects

- Installed with `--profile minimal`:
  - 1.98.1: 87,612,644 bytes downloaded, 446,500 KiB installed;
  - 1.99.1: 90,388,396 bytes downloaded, 462,532 KiB installed, later 455,320 KiB.
- Reused: 1.95.0 and 1.96.1.
- Unchanged: the default (`stable`, which still resolves to 1.97.1), the
  overrides (none) and Oxide's `rust-version`.
- The matrix toolchains stay installed. Removing them is Tom's choice.

See `summary.json` for the ledger, request ids, timings and bounds.
