# Opt-in stable Cargo development profiles

This contract owns Oxide's own opt-in Cargo profiles for everyday Rust
feedback: `oxide-light` and its explicit control, `oxide-full-debug`. They are
named per invocation on the pinned stable toolchain (`rust-toolchain.toml`,
channel 1.97.1). They do not change the default `dev`, `test` or `release`
profiles, any consumer, global Cargo configuration or the toolchain, and nothing
selects them automatically.

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

## Limits

- No benefit, speed, write reduction or universal result is claimed. The
  2026-10-16 bounded comparison was recorded as a no-go and its figures are not
  acceptance evidence.
- The profiles do not change the default cache, select a shared cache or affect
  any consumer. Consumer adoption, default changes and host installation remain
  separate owner-approved scopes.
