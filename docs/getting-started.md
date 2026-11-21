# Getting started

This guide takes you from a fresh checkout of Oxide to a Cargo project that builds through the shared pool. For every option, see [commands](commands.md). For machine settings, see [configuration](configuration.md).

## 1. Install Oxide

You can install a prebuilt release tarball (`oxide-<tag>-<target>.tar.gz` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, or `aarch64-unknown-linux-gnu`) from [GitHub Releases](https://github.com/inflatable-cookie/oxide/releases): verify its SHA-256 checksum against the accompanying `SHA256SUMS.txt`, extract `oxide` into a directory on your `PATH`, and run `oxide --version`.

To build from source instead, you need Rust 1.100.0. The repository pins it in `rust-toolchain.toml`, so `rustup` selects it for you inside the checkout.

```sh
git clone https://github.com/inflatable-cookie/oxide
cd oxide
cargo build --release --locked
cp target/release/oxide /usr/local/bin/oxide   # or any directory on your PATH
oxide --version
```

`oxide --version` prints Oxide's identity and then the version of the Cargo it finds on your `PATH`.

## 2. Adopt a project

Run this once per project:

```sh
oxide init        # an existing Cargo project, or a directory with no Cargo.toml yet
oxide new my-app  # a new project: cargo new, then the same adoption step
```

Adoption does two things:

- It writes one marked block into the project's `AGENTS.md`. Inside a Git repository the block sits in the repository root and lists every Cargo workspace it covers. Outside Git it sits in the project's own `AGENTS.md`. Re-running is safe. Text outside the markers is never touched.
- It adds `/.oxide-outputs/` to the workspace `.gitignore` if that rule is missing.

It also prints a readiness report: the covered paths, the toolchain Cargo resolves, the pool a build would use and any build scripts that still call plain `cargo`. Adoption does not create a configuration file.

Oxide refuses to write when it cannot read the Git state confidently, or when the `AGENTS.md` markers are malformed or duplicated. Nothing is written in those cases.

## 3. Build as usual

Use `oxide` where you would have used `cargo`. Arguments are Cargo's.

```sh
oxide check
oxide test -p core -- --nocapture
oxide build --release
oxide run -- --port 8080
```

The default profile is `quick`, which gives the fastest feedback. Use `--profile debug` when you need full debug information for a debugger. Use `--release` for production shapes. Commands that do not compile, such as `oxide add serde`, run plain Cargo with no lease.

Some tools call Cargo themselves. Run them under `oxide exec`:

```sh
oxide exec -- npm run tauri build
```

`oxide exec` keeps the tool's working directory and exit code. It does not pass a terminal's input to the tool, so interactive prompts are not supported.

Each leased command waits for a free slot if the host is busy. Use `--admission-budget` or `--execution-budget` when a command must finish within a time limit. See [commands](commands.md#time-budgets).

## 4. Check the pool

```sh
oxide status
```

`status` is read-only. It reports the selected pool, its slots, which command holds each one, waiting commands, and the size each slot had when last measured. It does not walk the disk. Add `--measure` for a bounded walk of the pool, the slots, the checkout's stray output and the Cargo home. The walk shares a 60-second budget.

On macOS, `oxide status --launch-check` measures how much slower a newly written executable launches the first time. It changes nothing.

## 5. Clean up

```sh
oxide clean             # remove this checkout's target/ and its slot affinity
oxide clean --dry-run   # list every path that would be removed, changing nothing
oxide clean --cargo-cache
```

Use `oxide clean` rather than `cargo clean` inside an adopted project. Under Oxide's environment, `cargo clean` would delete the shared build directory. `oxide clean` removes only this checkout's private output. Shared slots stay until the pool evicts them.

## Opting out

You can stop using Oxide in a project at any time:

- **Plain Cargo still works.** Running `cargo build` builds privately into the project's `target/` and does not use the pool. `oxide status` reports that output.
- **Restore a real `target/`.** If the project's final outputs were moved to a removable drive, run `oxide outputs detach`. It restores a real, empty `target/` and keeps every output that was already on the drive.
- **Remove the guidance.** Delete the marked block from `AGENTS.md` by hand. Oxide does not remove it for you.

Oxide never rewrites your `.gitignore` rules, so a `target/` rule that you added yourself stays as it is.
