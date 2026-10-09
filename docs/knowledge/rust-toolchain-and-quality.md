# Rust toolchain and quality

The repository pins the already-installed Rust 1.97.1 toolchain in
[`rust-toolchain.toml`](../../rust-toolchain.toml). The selected compiler is
`rustc 1.97.1` and its matching Cargo is `cargo 1.97.1`; this is the local
development and validation toolchain, not a minimum supported Rust version.
The package does not declare `rust-version`, so no MSRV is claimed. The
RustQuality profile stops version-sensitive work while that policy is
unresolved.

The unpublished root package uses Rust edition 2021 and has no dependencies or
product behavior. Its internal name is `oxide-storage`; publication names remain
subject to [Q-005](questions.md#q-005--what-names-are-safe-to-publish).

The reviewed design selects Cargo build-dir redirection as the smallest
mechanism to qualify later. That setting is stable from Cargo 1.91; the pinned
Cargo 1.97.1 supports it. This foundation does not configure shared storage or
prove cross-checkout correctness. See [storage boundaries](architecture.md).

Everyday Rust authoring follows the strict Northstar RustQuality profile at
[`rust-quality-profile.json`](contracts/rust-quality-profile.json), with
deviations recorded in
[`rust-quality-deviations.json`](contracts/rust-quality-deviations.json).
The installed package route supplies the authoring rules; these repository
files activate the route and define its local profile.

Repository validation selectors are listed in [`AGENTS.md`](../../AGENTS.md).
