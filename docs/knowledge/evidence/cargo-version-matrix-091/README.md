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
feature phases are in `raw/proof-1.95.0.log`. One shared native invocation is
left (5 of 6 used).

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
