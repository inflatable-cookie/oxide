# Rust/Cargo 1.100 foundation qualification: readiness (oxide#083)

Status: **source/control phase only. No 1.100 verdict yet.** No toolchain was
installed and no compiler, Cargo, staging, fixture, pool, image or proof ran.

## Release facts (owner-verified 2026-11-13)

- Release post: <https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/>.
- Channel manifest: <https://static.rust-lang.org/dist/channel-rust-1.100.0.toml>,
  date 2026-11-12, `rustc 1.100.0 (0ef1bc386 2026-11-09)`, SHA-256
  `3051e859570f64a8c5acb9d09e87425db52c7d97287c67e8f6d57d7d34ef9258`.
- The Cargo `-V` prefix and Cargo source pin are not recorded here. They are
  read from the installed toolchain, not guessed.

## Prior input

oxide#081 (`apfs-clone-relocation-20261018/summary.json`) observed layout v2
only on nightly 2026-08-05 and gives no 1.100 credit.

## Recipe

`scripts/foundation-rust-1-100.py`, selectors:

- `effigy proof:foundation:rust-1-100:plan`: effect-free plan (command, cases,
  allocation, premise dispositions, readiness gap).
- `effigy proof:foundation:rust-1-100:test`: pure self-check, 27 controls
  (guard digest `fcfda2b0…`, suspended-selector refusals, `VERIFIED_CARGO`
  contents, dep-info version gate, helper inventory, allocation arithmetic).
- `effigy proof:foundation:rust-1-100:diagnostic` (heavy): the planned single
  Queue proof, `bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
  scripts/foundation-rust-1-100.py --diagnostic`. At this head the script
  refuses before any effect. Not executed.

Planned allocation: at most three focused invocations total (pinned 1.97.1,
1.100.0, one spare); probes 8 Cargo calls per participant (2 version, 1 plain
build reused for stray and dep-info, 5 rcgu). Fit is unmeasured.

## Readiness gap

The brief retains routing, profiles, checkout-affine and cross-checkout
freshness, gate crash, crash recovery, group-gone and `status` oracles. Their
executable bodies (`scripts/test-oxide-cli.py`: `prove_routing`,
`prove_native`, `prove_freshness`, `prove_gate_crash`, `prove_recovery`,
`prove_group_gone`) read the retired pool's `oxide-managed/state.json` records
or the standalone lease CLI's `status --json`, and staging builds the retired lease
binary (oxide#102). The shipped directory-budget backend has neither surface.
Only `prove_layout` (effective build-dir, stray, dep-info) and `prove_fallback`
use shipped surfaces alone.

An equivalent migration rewrites each oracle against `oxide status` and the
current ledger and closes premise items 7 (frozen `RUSTFLAGS` /
`CARGO_ENCODED_RUSTFLAGS`) and 8 (qualified selected-volume scratch). That is
larger than this phase. No oracle was weakened and no retired route restored.

## Expected 1.100 behaviour from shipped source (not evidence)

`1.100.0` is not in `VERIFIED_CARGO`, so the shipped gate runs its unverified
fallback: slot affine to one checkout, stray detection `unknown`. The dep-info
decoder accepts only marker `01 00 00 00 ff` version 1 and refuses anything
else. A shared-slot verdict on 1.100 needs the Cargo source-pin check
(workflow, Cargo version matrix) before the table can grow; that is a product
change outside this phase.
