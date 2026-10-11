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

## Task 037 lock and registry correspondence update

The preceding lock paragraph records the earlier Task 036 observation. For
Task 037, the exact pinned `Cargo.lock` contents were read from the same
committed Git object, parsed with the already-installed `pip._vendor.tomli`,
and independently bound into the private Rust acquisition request. Its size
is 14,021 bytes and SHA-256 is
`ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d`.
The pinned source identity remains commit
`576ba7904d58e4bf829ddddda89a06833b8c5087`, tree
`0aa379b840c2f8990adeecc1fc49d9e57ab5869b`. Five allowlisted leaves were
read; their individual sizes and digests are in the retained evidence file.
The sorted locked registry tuple inventory (name, version, source, checksum)
has digest
`d34be11369c2dc97d43afca0a4211a287f5e841048187c4a386d0f19f2a29cc6`.
There are 58 crates.io registry tuples and four local workspace tuples. The
inventory is not an enabled-unit or complete feature graph.

The only permitted Cargo configuration files were checked:
`/Users/tom/.cargo/config` and
`/Users/tom/.cargo/config.toml`. The former was absent; the latter contained
20 bytes. No literal registry replacement or custom source mapping appeared
there. `CARGO_HOME` was explicitly `/Users/tom/.cargo`, matching the pinned
candidate root. Other configuration locations and environment overrides were
not read, so effective Cargo configuration and registry identity remain
unresolved.

Discovery enumerated two immediate namespaces under each of
`/Users/tom/.cargo/registry/cache` and
`/Users/tom/.cargo/registry/src`; both roots were stable across the
observation. All 58 locked packages received outcomes after 232 bounded
candidate-path metadata observations: one supported correspondence, 24
`ambiguous_candidates` refusals, and 33 exact
`verifier_Unsupported` refusals. The verifier refusal class is retained as
returned; no more specific unsupported-format cause is inferred. The one
supported package was `fallible-streaming-iterator 0.1.9`, checksum
`7360491ce676a36bf9bb3c56c1aa791658183a54d2744120f27285738d90465a`.
Its archive and extracted root had unique canonical paths in namespace
`index.crates.io-1949cf8c6b5b557f`. The existing
`archive::verify(&Request, Limits, Counters)` implementation checked the
archive checksum and exact extracted file/directory correspondence. The
retained manifest lists eight files and two directories; before/after metadata
and the full package result are preserved in
[`nucleus-registry-acquisition-037.json`](../evidence/nucleus-registry-acquisition-037.json).

All package attempts shared one verifier counter set. It recorded 13,435,995
compressed archive bytes, 60,891,136 decompressed bytes, 36,437 extracted-file
bytes, and 54 entries (74,363,568 charged bytes total). The supported package
attempt's cumulative counters were 304,106 archive bytes, 1,439,232
decompressed bytes, 36,437 extracted-file bytes, and 27 entries; these include
earlier refused attempts. The supported package's own delta from the preceding
cumulative snapshot was 9,249 archive bytes, 44,032 decompressed bytes, 36,437
extracted-file bytes, and 21 entries. Behavior triage read 840 bytes from that
package's verified `Cargo.toml`; it found no build script, proc-macro entry,
or native link declaration. Aggregate acquisition reads were 74,433,606 bytes,
including 69,158 pinned source bytes: 48,600 bounded source bytes were read
during preflight before the acquisition entry existed, and 20,558 were read by
the entry itself. The total also includes 40 configuration bytes observed
across preflight/acquisition and triage. Retained evidence is 171,140 bytes. The
sampled free space stayed above the 20 GiB reserve; its delta is not an SSD
write measurement.

Pinned manifests name `rusqlite = 0.40.2` with `bundled`, serde with
`derive` (and `rc` in nucleus-core), and schemars; the lock inventory also
contains candidate packages including `libsqlite3-sys 0.38.2`, `cc 1.4.7`,
`pkg-config 0.3.34`, `vcpkg 0.2.15`, `serde_derive 1.0.229`,
`schemars_derive 0.8.22`, `syn 2.0.119`, and `quote 1.0.47`. These
manifest and lock references identify the next bounded behavior-audit
candidates; they do not prove enabled units, feature selection, or that the
refused packages are available for source inspection. No refused package
sources were read, and no script, macro, helper, Cargo command, compiler, or
SDK was executed or inspected.

The opt-in private entry is
`effigy registry:acquire:nucleus`. It reads only the pinned source leaves and
allowlisted Cargo configuration, then invokes the feature-gated
`oxide-registry-acquisition` binary, which delegates correspondence checks to
the existing Rust archive verifier. This observation grants no installed
registry issuer, no lease-time no-replacement guarantee, and no full package,
graph, macro/build-script, native-tool, SDK, or hidden-read closure. It is
partial correspondence evidence, not a build, test, reuse, or rollout result.

## Task 038 archive-format and registry-pairing correction

Task 038 retained the Task 037 evidence unchanged and added
[`nucleus-registry-acquisition-038.json`](../evidence/nucleus-registry-acquisition-038.json).
It also preserves the first, over-limit attempt at
[`nucleus-registry-acquisition-038-initial-attempt.json`](../evidence/nucleus-registry-acquisition-038-initial-attempt.json).
The bounded diagnostics tied `bitflags 2.13.2` and `cc 1.4.7` to their exact
lock tuples, checksums and recorded archive paths. Both archives used legacy
GNU tar headers (`ustar ` magic and ` \0` version); the previous verifier
refused at `entry.header().as_ustar().is_none()` before checking member type or
path. The diagnostics described at most 64 headers per package; bitflags
reached that description cap, and cc had 28. Both selected archive candidates
had stable metadata and matched their lock checksum. The diagnosis read
153,835 compressed bytes, 740,864 decompressed bytes and 92 header descriptions.

The verifier now admits those GNU headers through the existing tar reader and
keeps the existing file, directory, path, duplicate, link, checksum, gzip,
inventory, stability and budget checks. The refresh verified bitflags, cc and
serde. For serde, two archive candidates and one extraction candidate produced
one complete same-namespace pair in `index.crates.io-1949cf8c6b5b557f`; the
spare archive in `index.crates.io-6f17d22bba15001f` had no extraction candidate
there. The report preserves every candidate. This is filesystem correspondence
only and does not establish effective Cargo registry selection or an accepted
issuer.

The initial full refresh covered all 58 lock tuples and verified 56. It refused
`libsqlite3-sys 0.38.2` and `sqlite-wasm-rs 0.5.5` with `verifier_Budget`, but
read 16,777,217 decompressed bytes for each due to a one-byte overflow probe.
This attempt and its counters are preserved unchanged. Follow-up commit
`8bd9949623a06df63cadd9cb7ea3bdbcebe8a70a` corrected the cap check. One
corrected full refresh then ran at Oxide commit
`98629cfc05ece0cdc0fde3b562e3be48b9e8d3a0` and again covered the same 58 lock
tuples. It verified 56, including bitflags, cc and serde; the two budget
refusals stopped at exactly 16,777,216 decompressed bytes, before any tar entry
or extracted-file read. The corrected attempt carried forward the first
attempt's archive counters and 234 candidate observations rather than resetting
the phase.

Across diagnosis and both full attempts, the ledger records 207,717,057
compressed/decompressed/extracted archive bytes, 89,716 pinned-source bytes,
60 allowlisted config bytes, 600,545 behavior-triage bytes, 54,218 diagnostic
receipt bytes, and 662,827 bytes reading the prior acquisition receipt. The
aggregate is 209,124,423 bytes with 466 candidate-path observations. The
corrected full refresh added 103,411,178 archive bytes and 232 candidate
observations. Its output is 611,520 bytes. Triage reached the 128-leaf cap in
the initial attempt; it was not repeated, and the initial triage receipt remains
preserved. All cumulative phase totals stayed within their aggregate ceilings.
The evidence does not establish enabled units, native/SDK closure, an issuer,
or a successful Nucleus workflow. No Nucleus Cargo command, package code,
script, macro, helper, SDK content or toolchain content was read or run.

## Task 039 streaming two-package update

Task 039 left the Task 037 and Task 038 records unchanged and added
[`nucleus-registry-acquisition-039.json`](../evidence/nucleus-registry-acquisition-039.json).
The private schema-v3 streaming caller re-observed only the two pairs Task 038
had refused at the 16 MiB decoded-container cap. Both `libsqlite3-sys 0.38.2`
and `sqlite-wasm-rs 0.5.5` reached `supported` with exact archive/extraction
correspondence: 26 files and 4 directories, and 77 files and 17 directories
respectively, selected as the unique complete same-namespace pair. Their decoded
containers are 21,646,848 and 24,785,920 bytes, above the legacy cap, while every
member and extracted file stays below the 16 MiB member cap. The observation
read 104,097,860 archive bytes (compressed, decoded and extracted) plus the
611,520-byte Task 038 receipt, 104,730,050 aggregate, with 8 candidate-path
observations and a 20,670-byte report. It called no legacy `verify`, replayed
none of the other 56 packages, and read no package source, script, macro, native,
SDK or toolchain content.
Still not established: effective Cargo registry selection or configuration,
enabled units, an accepted registry issuer or lease-time no-replacement
agreement, and any Nucleus build, test or reuse result.

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
