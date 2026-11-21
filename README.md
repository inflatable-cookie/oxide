# Oxide

Oxide is Cargo with managed build storage, fast build profiles and bounded disk use. You keep typing `oxide check`, `oxide test` and `oxide run` where you would have typed `cargo`, and Oxide decides where the build output lives.

Every new checkout and worktree normally starts with an empty `target/` and recompiles the same dependencies. Oxide keeps compiled output in one shared, size-limited pool instead, so a new checkout can reuse work that a sibling already did, and the pool stays within a budget you set.

## What Oxide does

- **Shared, bounded storage.** Build directories ("slots") live in one pool with a soft budget (100 GB by default). Idle slots are evicted when the pool is full. Storage bookkeeping (roots, holds, pins, pruning and trash) is delegated to Barge, a storage library Oxide builds on.
- **Profiles.** `quick` (the default) gives fastest feedback with line-table debug info. `debug` keeps full debug info for a debugger. `release` uses your project's own release profile.
- **Cross-checkout reuse.** A checkout can reuse a compatible idle slot after a freshness check. On Cargo 1.100.0, Oxide seeds a cold slot from an idle donor slot and prunes stale units from warm slots, so reuse does not grow without limit.
- **Removable drives.** A removable APFS volume can hold the pool, and in permanent mode it can hold final binaries too, so the internal disk does less writing.
- **Cooperative wrapper for other tools.** `oxide exec -- <command>` runs a tool that calls Cargo itself, such as a Tauri build, under the same storage and profile rules.
- **Cleanup you control.** `oxide clean` removes a checkout's own build output. `oxide clean --cargo-cache` runs one pass of Cargo's own cache cleanup with the network switched off.
- **Read-only status.** `oxide status` reports the pool, slots and waiting commands without changing anything.

Oxide never falls back to plain Cargo for a build it has leased, and it never deletes a shared slot it does not own. When it cannot confirm what is writing to a slot, it holds the slot and refuses, instead of guessing.

## Name

Oxide is not affiliated with Oxide Computer Company or its [`oxidecomputer/oxide.rs`](https://github.com/oxidecomputer/oxide.rs) project. This project is not published on crates.io (`publish = false`). It is distributed from its git repository through tagged releases with prebuilt binary tarballs on [GitHub Releases](https://github.com/inflatable-cookie/oxide/releases) and source builds.

## Requirements

- **To build Oxide from source:** Rust 1.100.0. The toolchain is pinned in `rust-toolchain.toml`, so `rustup` selects it automatically inside this repository.
- **Projects Oxide manages:** Cargo 1.95.0 through 1.100.0 are supported. On other Cargo releases Oxide still runs, but cross-checkout reuse is turned off.
- **Platforms:** macOS is the primary platform. Linux is supported with a directory pool, which needs no removable-drive features.
- **Network access (building from source):** Oxide depends on two git dependencies pinned by revision or tag in `Cargo.toml`: Barge and the Longhorn storage layout. Building from source needs access to both repositories.

## Install

### Prebuilt binaries

Prebuilt release tarballs (`oxide-<tag>-<target>.tar.gz`, containing `oxide`, `LICENSE` and `README.md`) and `SHA256SUMS.txt` are published on [GitHub Releases](https://github.com/inflatable-cookie/oxide/releases) for:

- `aarch64-apple-darwin` (Apple Silicon macOS)
- `x86_64-apple-darwin` (Intel macOS)
- `x86_64-unknown-linux-gnu` (x86_64 Linux)
- `aarch64-unknown-linux-gnu` (arm64 Linux)

Download the archive for your target along with `SHA256SUMS.txt`, verify the checksum (`shasum -a 256 -c SHA256SUMS.txt --ignore-missing` on macOS or `sha256sum -c --ignore-missing SHA256SUMS.txt` on Linux), and place `oxide` on your `PATH`.

### Build from source

```sh
git clone https://github.com/inflatable-cookie/oxide
cd oxide
cargo build --release --locked
cp target/release/oxide /usr/local/bin/oxide   # or any directory on your PATH
```

## Quick start

```sh
cd path/to/your/project
oxide init                 # adopt an existing Cargo project (or: oxide new <path>)
oxide check
oxide test
oxide build
oxide run
oxide exec -- <command>    # for a tool that runs Cargo itself
oxide status               # pool, slots and waits; read-only
oxide status --measure     # also walks the pool and Cargo home; slower
oxide clean                # remove this checkout's target/ output
oxide clean --cargo-cache  # one pass of Cargo's own cache cleanup
```

`oxide init` writes one marked guidance block into the project's `AGENTS.md`, so coding agents know to call `oxide`. It does not create a configuration file.

## Documentation

- [User guide](docs/README.md): [getting started](docs/getting-started.md), [commands](docs/commands.md) and [configuration](docs/configuration.md).
- [Architecture and contracts](docs/README.md#architecture--contracts) for the storage invariants and the implemented workflow.

## Licence

Oxide is released under the [MIT licence](LICENSE).
