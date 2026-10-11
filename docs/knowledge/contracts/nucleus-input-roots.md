# Nucleus pinned-workflow input-root acquisition

This snapshot records bounded, read-only discovery for the pinned Nucleus
planning/diagnostics workflow. It distinguishes source-authenticated Git objects
from installed path candidates. It does not establish acquisition readiness,
input closure, an accepted production issuer, or a successful Cargo/test/profile
result.

## Pinned source and route

The candidate path supplied for this workflow is
`/Users/tom/Dev/projects/nucleus`. Its configured `origin` is
`git@github.com:inflatable-cookie/nucleus.git`. The requested committed object
exists at `576ba7904d58e4bf829ddddda89a06833b8c5087`, whose tree is
`0aa379b840c2f8990adeecc1fc49d9e57ab5869b`. At inspection, the checkout at
that path instead reported HEAD `f056f458ccab6e389a74861f1bfa76f05153bf68`
and tree `d3465c41321d91eb47da164b380c71c3a18b05c9`. Source leaves below were
read from the requested commit object with `git show <commit>:<path>`; the
checkout was not changed, fetched, checked out, built, or status-scanned. The
Git commit/tree are authenticated source identities; the physical workspace
root for an admitted run is not established by this snapshot.

The six committed source leaves whose contents were read total 57,801 bytes:

| Pinned leaf | Bytes | Relevant fact |
| --- | ---: | --- |
| `Cargo.toml` | 499 | Workspace members include `crates/nucleus-core` and `crates/nucleus-observer`; resolver 3. |
| `effigy.toml` | 5,408 | The owning selector is defined inline; no external Effigy manifest is referenced by it. |
| `crates/nucleus-core/Cargo.toml` | 450 | `nucleus-core`, edition 2024; local `nucleus-observer`; `rusqlite = 0.40.2` with `bundled`; other direct dependencies are pinned. No `rust-version`. |
| `crates/nucleus-observer/Cargo.toml` | 180 | Local dependency is edition 2024 with `serde` derive and `schemars`; no `rust-version`. |
| `crates/README.md` | 33,197 | Documents ordinary line-table debug info, the separate full-debug requirement, and the diagnostics test. |
| `docs/knowledge/contracts/001-working-rules.md` | 18,067 | Read as the pinned repository contract leaf; it supplies no additional installed-root receipt. |

The pinned `Cargo.lock` object is 14,021 bytes, below the per-leaf limit, but
its contents and lock-selected package blocks were not read. The available
`python3` lacks `tomllib`; no alternate parser or Cargo metadata command was
used. Thus the exact lock-resolved native/archive producer set and its
checksums remain unresolved. The following exact root leaves were absent:
`rust-toolchain`, `rust-toolchain.toml`, `.cargo/config`, and
`.cargo/config.toml`. No other source leaves, tree inventory, package
inventory, submodule/LFS survey, or external include were read.

The ordinary workflow is invoked from the canonical Nucleus workspace root as:

```text
["effigy", "check:core:test", "--", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

The pinned `effigy.toml` expands this to:

```text
["cargo", "test", "--offline", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

There is no `--locked`, `--target`, extra feature, or test-runner argument in
that ordinary vector. The root Cargo manifest sets `[profile.dev] debug =
"line-tables-only"`; tests inherit it. `[profile.full-debug]` inherits dev and
sets `debug = "full"`. Full-debug source/artifact lookup remains a separate
requirement in the owning contract. This packet assumes no additional Cargo
invocation and asserts no successful test or profile fit. Nucleus declares no
toolchain pin or MSRV in the inspected workspace/package manifests. Oxide's
repo-owned Rust authoring profile is `strict` with no deviations and an
unresolved MSRV is a stop; this change authors documentation only.

## Installed path observations

The allowlisted path configuration observed was:

```text
CARGO_HOME=/Users/tom/.cargo
RUSTUP_HOME=<unset>
DEVELOPER_DIR=<unset>
SDKROOT=/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk
PATH=/Users/tom/.codex/packages/standalone/releases/0.159.0-aarch64-apple-darwin/codex-path:/Users/tom/.local/bin:/Users/tom/.local/bin:/Users/tom/.grok/bin:/Users/tom/.bun/bin:/Users/tom/.kimi-code/bin:/Users/tom/.opencode/bin:/Users/tom/.local/bin:/Users/tom/.local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/System/Cryptexes/App/usr/bin:/usr/bin:/bin:/usr/sbin:/sbin:/var/run/com.apple.security.cryptexd/codex.system/bootstrap/usr/local/bin:/var/run/com.apple.security.cryptexd/codex.system/bootstrap/usr/bin:/var/run/com.apple.security.cryptexd/codex.system/bootstrap/usr/appleinternal/bin:/pkg/env/global/bin:/Library/Apple/usr/bin:/opt/homebrew/bin:/Users/tom/.codex/tmp/arg0/codex-arg0XCo8bl:/Users/tom/.codex/packages/standalone/releases/0.159.0-aarch64-apple-darwin/codex-path:/Users/tom/.local/bin:/Users/tom/.grok/bin:/Users/tom/.bun/bin:/Users/tom/.kimi-code/bin:/Users/tom/.opencode/bin:/opt/homebrew/sbin:/Users/tom/.codex/tmp/arg0/codex-arg0Jb1PaY:/Users/tom/.cargo/bin:/Users/tom/.docker/bin:/Users/tom/.docker/bin
```

There were 12 bounded informational queries in total: one command-resolution
batch for `rustup`, `rustc`, `cargo`, `xcrun`, `clang`, `cc`, `ar`, and `ld`;
one allowlisted path-configuration query; two `rustup which --toolchain
1.97.1` queries; four `xcrun --find` queries; two xcrun SDK queries; and two
direct-rustc queries. All returned exit 0 within 15 seconds, each output was
under 4 KiB, and aggregate query output was under 64 KiB. No Cargo command was
run.

| Query | Result |
| --- | --- |
| `command -v` | `rustup=/Users/tom/.cargo/bin/rustup`; `rustc=/Users/tom/.cargo/bin/rustc`; `cargo=/Users/tom/.cargo/bin/cargo`; `xcrun=/usr/bin/xcrun`; `clang=/usr/bin/clang`; `cc=/usr/bin/cc`; `ar=/usr/bin/ar`; `ld=/usr/bin/ld`. |
| `rustup which --toolchain 1.97.1 rustc` | `/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/rustc` |
| `rustup which --toolchain 1.97.1 cargo` | `/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/cargo` |
| `xcrun --find clang` | `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang` |
| `xcrun --find cc` | `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/cc` |
| `xcrun --find ar` | `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ar` |
| `xcrun --find ld` | `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld` |
| `xcrun --show-sdk-path`; `--show-sdk-version` | SDK candidate matches `SDKROOT`; version `27.0`. |
| Direct `rustc -vV` | `rustc 1.97.1 (8bab26f4f 2026-07-14)`; commit `8bab26f4f68e0e26f0bb7960be334d5b520ea452`; host `aarch64-apple-darwin`; LLVM `22.1.6`. |
| Direct `rustc --print=sysroot` | `/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin` |

Metadata-only `lstat`/`realpath`/uid/mode/device/inode observations were made
for exactly 16 canonical candidates. Each tuple is `uid/mode/dev/inode`; mode
is octal. Unless a `->` target is shown, `realpath` equals the listed path and
target metadata equals lstat metadata. The `/usr/bin/ld` result was not included
in the metadata set because of the 16-path cap.

| Candidate path | Metadata |
| --- | --- |
| `/Users/tom/.cargo` | `501/0755/16777234/1445456` |
| `/Users/tom/.cargo/bin/rustup` | `501/0755/16777234/1238326084` |
| `/Users/tom/.cargo/bin/rustc` | lstat `501/0755/16777234/1445462` -> `/Users/tom/.cargo/bin/rustup`, target `501/0755/16777234/1238326084` |
| `/Users/tom/.cargo/bin/cargo` | lstat `501/0755/16777234/1445464` -> `/Users/tom/.cargo/bin/rustup`, target `501/0755/16777234/1238326084` |
| `/usr/bin/xcrun` | `0/0755/16777234/1152921500312573114` |
| `/usr/bin/clang` | `0/0755/16777234/1152921500312571562` |
| `/usr/bin/cc` | `0/0755/16777234/1152921500312571562` |
| `/usr/bin/ar` | `0/0755/16777234/1152921500312571562` |
| `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang` | `0/0755/16777234/875763218` |
| `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/cc` | lstat `0/0755/16777234/875763216` -> `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang`, target `0/0755/16777234/875763218` |
| `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ar` | `0/0755/16777234/875737742` |
| `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld` | `0/0755/16777234/875763265` |
| `/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk` | `0/0755/16777234/875700264` |
| `/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/rustc` | `501/0755/16777234/205615529` |
| `/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin/bin/cargo` | `501/0755/16777234/205615332` |
| `/Users/tom/.rustup/toolchains/1.97.1-aarch64-apple-darwin` | `501/0755/16777234/205615341` |

No directory was listed or traversed; no file contents, hashes, credentials,
Cargo user configuration, registry entries, SDK contents, or Rustlib contents
were read. In particular, no `$HOME/.cargo` path was guessed: the observed
`CARGO_HOME` is explicitly set, but it is only a candidate root. No query
verified Cargo's version/source, selected compiler wrappers, effective native
flags, or the contents and integrity of any installed root. Version/path and
uid ownership do not prove closure, immutability, or no replacement.

## Acquisition handover

The Git commit/tree are the only source-authenticated identities established
here. The path currently has a different checked-out commit, so the Nucleus
managed-Git/workspace and root-allocation owner must provide an exact clean root
at the pinned commit/tree, with canonical root mapping, allowed-read evidence,
and an explicit submodule/LFS disposition.
The Git tree OID is not an Oxide framed input digest; no source inventory or
input digest was computed ([digest definitions](nucleus-core-reuse.md#digest-identities-all-distinct)).

The next acquisition packet for this exact selector needs owner receipts for:

1. **Cargo registry and Rust archive installation owners:** resolve effective
   Cargo configuration without exposing secrets; identify the lock-selected
   package/archive set, checksums, extracted file manifests, canonical roots,
   integrity checks, and no-replacement guarantee. Account for the bundled
   `rusqlite 0.40.2` native producer and the actual Rust archive/build-script/
   compiler helper chain; provide per-producer and aggregate read counters and
   footprint rather than inferring them from the lock or source tree.
2. **Rust toolchain installation owner:** verify the installed Cargo `1.97.1` source
   `c980f4866141969fab6254a680546a277789d6f0` required by the collector, the
   rustc version/commit and host, absolute binary integrity, sysroot/Rustlib
   closure, and no replacement through the lease. The rustc self-report and
   Cargo path observed above are candidates only.
3. **Xcode/native toolchain installation owner:** choose the actual
   clang/cc/ar/ld paths from the observed PATH and xcrun candidates; provide
   immutable identity/read receipts for compiler, archiver, linker, SDK
   headers/frameworks and reachable native inputs, including environment
   presence/absence and no replacement. SDK version `27.0` and path alone are
   insufficient.
4. **Nucleus graph/audited-input owner, task-admission owner, and Queue host
   authority:** supply the complete enabled semantic unit graph, selected
   macro/build-script/native/helper closure and explicit tool/SDK/native
   associations, with bounded read counters and aggregate footprint. Provide an
   accepted real graph/audit issuer and policy through those owners; oxide#035
   still accepts only the synthetic qualification issuer
   ([audit boundary](nucleus-core-reuse.md#callable-accepted-audit-record-oxide035)).

The owner packet must report actual counters and aggregate footprint against
the proposed ceilings in the [registered-input contract](nucleus-core-reuse.md#finite-readevidence-budgets-and-guard-publication):
16 MiB/file, 256 MiB/source inventory operation, 256 MiB/collection, and up to
520 MiB of accepted reads for admission and independently for finalize. These
are proposed limits, not measured closure sizes or an authorization to scan.

The existing source/FINAL acquired request, StorageAttempt/Effigy 010 binding,
cooperative capture, old-debug lookup, and scoped admission gates remain in
force. This packet performs no real audit, Cargo invocation, installation,
consumer activation, or issuer change; it is not a run receipt or proof of
reuse.
