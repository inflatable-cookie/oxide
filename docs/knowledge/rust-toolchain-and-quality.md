# Rust toolchain and quality

The repository pins the already-installed Rust 1.97.1 toolchain in
[`rust-toolchain.toml`](../../rust-toolchain.toml). The selected compiler is
`rustc 1.97.1` and its matching Cargo is `cargo 1.97.1`; this is the local
development and validation toolchain.

Tom's ruling, 2026-10-09: Oxide's initial minimum supported Rust version is
**1.97.1**. Package manifests must declare `rust-version = "1.97.1"`; the strict
RustQuality policy remains in force with no unresolved-MSRV exception. The
prototype's recorded checks exercised this compiler. Supporting an older
compiler requires deliberate compatibility validation and a revised policy.
This minimum governs building Oxide itself; consumer project compilers remain
independently resolved inputs to their build-storage identity.

The unpublished root package uses Rust edition 2021 and declares
`rust-version = "1.97.1"`. Its internal name is `oxide-storage`; publication
names remain subject to [Q-005](questions.md#q-005--what-names-are-safe-to-publish).
It contains a Unix-only local storage prototype, the unpublished
`oxide-storage-lease` and `oxide` binaries, and pins `sha2` 0.10.9 for
persisted lease-token hashes. This does not establish production host support.

The reviewed design selects Cargo build-dir redirection as the smallest
mechanism to qualify later. That setting is stable from Cargo 1.91; the pinned
Cargo 1.97.1 supports it. The prototype does not configure Cargo storage or
prove cross-checkout correctness. See [storage boundaries](architecture.md).

Everyday Rust authoring follows the strict Northstar RustQuality profile at
[`rust-quality-profile.json`](contracts/rust-quality-profile.json), with
deviations recorded in
[`rust-quality-deviations.json`](contracts/rust-quality-deviations.json).
The installed package route supplies the authoring rules; these repository
files activate the route and define its local profile.

Repository validation selectors are listed in [`AGENTS.md`](../../AGENTS.md).
The opt-in `oxide-light`/`oxide-full-debug` development profiles, their
selectors and the no-go disposition of their unqualified comparison
observations are owned by the
[stable Cargo profiles contract](contracts/stable-cargo-profiles.md); they do
not change the default profiles or the pinned toolchain.
