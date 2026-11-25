# Opt-in stable Cargo development profiles

This contract covers the `quick` dependency debug opt-in and Oxide's named
Cargo profiles for everyday Rust feedback. `oxide-light` and its explicit
control, `oxide-full-debug`, are selected per invocation on the pinned stable
toolchain (`rust-toolchain.toml`, channel 1.100.0).

## Quick without dependency debug information

`oxide build --no-dep-debug` adds `--config 'profile.dev.package."*".debug=0'`
for that `quick` invocation. The wildcard applies to packages outside the
workspace. Cargo's normal profile precedence still applies, including named
package overrides. Workspace units keep the quick `line-tables-only` recipe.
Dependency backtraces may lose file and line information, and dependency
types and variables may be unavailable in a debugger. Use `oxide build
--profile debug` for full debug information.

Cargo 1.100.0 has no environment variable for the wildcard package override.
The option is for leased Cargo commands under `quick`, including Clippy and
external subcommands that accept and apply `--config`. `oxide exec`, `debug`
and `release` refuse it. Cargo 1.100.0 fixtures exercise `check`, `build`,
`run` and `test`, including the workspace backtrace, unwinding, debug
assertions and overflow checks described below.

## Declared profiles

Both profiles inherit `dev`, so incremental compilation, unwinding, debug
assertions, overflow checks and opt-level 0 apply to workspace units.

- `oxide-light` keeps line tables for workspace units only. Dependency and
  build-time units carry no debug information.
- `oxide-full-debug` sets full debug information for workspace, dependency and
  build-time units.

The definitions live in `Cargo.toml` under `[profile.oxide-light]` and
`[profile.oxide-full-debug]`, with `package."*"` and `build-override` tables.

## Adoption

Adoption is per invocation. There is no default, environment variable or Cargo
configuration change:

```sh
cargo test --profile oxide-light --test profile_backtrace
cargo check --profile oxide-light --all-targets --locked
cargo test --profile oxide-full-debug --test profile_backtrace
```

Each profile writes to its own artifact directory under the target directory
(`target/oxide-light`, `target/oxide-full-debug`), separate from `target/debug`.
A light build never reuses or overwrites full-debug artifacts. The first use of
a profile is a cold build in its own directory.

## Debugging tradeoff

In `oxide-light`, dependency and build-time units carry no debug information.
Dependency source, types and variables are unavailable, and dependency frames
resolved from debug information are missing. Workspace frames are the only ones
guaranteed to keep `file:line`. Rich debugging needs an explicit
`oxide-full-debug` build. No debugger-variable or type-inspection guarantee is
made beyond what has been exercised.

`tests/profile_backtrace.rs` is the workspace probe for behaviour the light
profile keeps: a panic inside a workspace helper unwinds to `catch_unwind` with
its payload, the panic's backtrace resolves more than one workspace frame to
`file:line`, and debug assertions and overflow checks stay active. The panic's
initial location alone is not accepted as that proof.

## Verification

The focused profile selectors and their resolved-flag checks are retired. The
probe is an ordinary integration test. Covering checks are `rust:compile`,
`rust:compile-release` and `rust:format`, plus the ordinary suite
(`rust:barge-adoption`).

## Cargo 1.100.0 native `debug` profile

Verified in scratch fixtures on `cargo 1.100.0 (3acc79c0c 2026-11-05)` vs
`cargo 1.97.1 (c980f4866 2026-06-30)` (and `1.99.1`):

- Cargo 1.97.1 and 1.98.1 reject `--profile debug` (`profile name 'debug' is
  reserved`).
- Cargo 1.99.1 and 1.100.0 accept `--profile debug` and place outputs in
  `target/debug/`, but Cargo's native `debug` profile inherits `dev`
  (`dir-name = "debug"`):
  - Build-script (`build-override`) units still compile without `-C debuginfo`
    unless `CARGO_PROFILE_*_BUILD_OVERRIDE_DEBUG=full` is set.
  - Because `debug` inherits `dev`, `[profile.dev]` or `CARGO_PROFILE_DEV_*`
    settings apply to `--profile debug` unless overridden on `debug`.
  - `cargo test --profile debug` bypasses `[profile.test]`, whereas tuning
    `dev` preserves `[profile.test]` inheritance.
  - Child Cargo invocations inside `oxide exec --profile debug` run `dev`
    unless the tool explicitly passes `--profile debug`.
- Both Oxide's `--profile debug` `dev` environment recipe
  (`CARGO_PROFILE_DEV_DEBUG=full`, `CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG=full`
  in [workflow](workflow.md#profiles-and-strategies)) and `oxide-full-debug`'s
  explicit `build-override` table therefore remain necessary on Cargo 1.100.0 as
  well as older Cargo releases.

## Limits

- No benefit, speed, write reduction or universal result is claimed. The
  2026-10-16 bounded comparison was recorded as a no-go and its figures are not
  acceptance evidence.
- The profiles do not change the default cache, select a shared cache or affect
  any consumer. Consumer adoption, default changes and host installation remain
  separate owner-approved scopes.
