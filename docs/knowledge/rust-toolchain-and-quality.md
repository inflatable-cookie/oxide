# Rust toolchain and quality

The repository pins the already-installed Rust 1.97.1 toolchain in
[`rust-toolchain.toml`](../../rust-toolchain.toml). The selected compiler is
`rustc 1.97.1` and its matching Cargo is `cargo 1.97.1`; this is the local
development and validation toolchain.

Tom's ruling, 2026-10-09: Oxide's initial minimum supported Rust version is
**1.97.1**. Package manifests must declare `rust-version = "1.97.1"`. The
prototype's recorded checks exercised this compiler. Supporting an older
compiler requires deliberate compatibility validation and a revised policy.
This minimum governs building Oxide itself; consumer project compilers remain
independently resolved inputs to their build-storage identity. The Cargo
releases the freshness gate, stray detection and dep-info decoder support are
listed in the [Cargo version matrix](contracts/workflow.md#cargo-version-matrix).

The unpublished root package uses Rust edition 2021 and declares
`rust-version = "1.97.1"`. Its internal name is `oxide-storage`; publication
names remain subject to [Q-005](questions.md#q-005--what-names-are-safe-to-publish).
It contains a Unix-only local storage library, the unpublished `oxide` and
`oxide-storage-inventory` binaries (the standalone lease binary was retired in
oxide#102), and pins `sha2` 0.11.0 for
persisted lease-token hashes. It pins `basic-toml` `=0.1.10` (oxide#095), the
strict-profile exception in Tom's
[2026-10-19 ruling](vision.md#product-workflow-direction), only to parse Cargo
config for [`clean --cargo-cache`](contracts/workflow.md#cargo-home-hygiene)
and, since oxide#102, the [user config file](contracts/workflow.md):
a TOML 1.0 parser whose only dependency is the already-pinned `serde`, so
`Cargo.lock` gains this one package; it was resolved offline from the local
cache (checksum `ba62675e8242a4c4e806d12f11d136e626e6c8361d6b829310732241652a178a`, matching the cached archive). This does not establish production host support.

Since oxide#314 the package depends on Barge's lean storage core at one full
40-hex commit, the `rev` pinned in [`Cargo.toml`](../../Cargo.toml). It includes the
lean core, adopted-root seam, under-lock eviction identity checks and read-only
account opener (Barge PRs 65, 66, 68 and 67). `Cargo.lock` gains only the `barge`
package; its `serde`, `serde_json` and `libc` are already pinned. Offline builds
take the locked Barge source from the bounded seeder, as they do for Longhorn.

Oxide's Cargo invocations set `CARGO_BUILD_BUILD_DIR` to the storage-owned build
directory and keep `CARGO_TARGET_DIR` in the checkout. That setting is stable
from Cargo 1.91, and the pinned Cargo 1.97.1 supports it. Cross-checkout
correctness is qualified only to the limits in
[qualification readiness](qualification-readiness.md). See
[storage boundaries](architecture.md).

Repository validation selectors are listed in [`AGENTS.md`](../../AGENTS.md).
The release profile carries no diagnostic allowance: `effigy
rust:compile-release` compiles the binaries with `RUSTFLAGS='-D warnings'`, so
an unused import, an unneeded `mut` or an item only a debug build reaches fails
the check instead of becoming noise in the profile Oxide is installed from
(oxide#223). Cargo still caps dependency lints, so that gate covers this
workspace package only. An item kept for a debug or test caller is gated with
the same `cfg` that caller compiles under (`#[cfg(debug_assertions)]` for proof
hooks and controls, `#[cfg(test)]` for test seams), never with a broad `allow`,
so no gate can hide an item from the build that uses it.
The opt-in `oxide-light`/`oxide-full-debug` development profiles, their
selectors and the no-go disposition of their unqualified comparison
observations are owned by the
[stable Cargo profiles contract](contracts/stable-cargo-profiles.md); they do
not change the default profiles or the pinned toolchain.
