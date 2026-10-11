# First Nucleus core-test reuse contract

Status: proposed consumer contract, with inactive library seams and a private
framed Rust runner host implemented. No accepted consumer owners or genuine
consumer integration are installed.
This contract defines eligibility and owner gates; it does not enable Nucleus, approve its source for execution, or
claim real-project reuse. [Vision](../vision.md#authority-boundaries) owns
authority; [architecture](../architecture.md) owns storage/lease invariants;
[project adoption](project-adoption.md) owns the general adapter boundary.

The independently reviewed [Task 021 fixture result](../qualification-readiness.md#task-021-v2-same-root-execution-result-2026-10-10)
is the starting evidence: changed task/source at one physical root retained
dependency work, ran tests and preserved full-debug/source evidence. Review was
at `faf1490484af174dcb5ec4e98b116c408d2680ca`
([verdict](https://github.com/inflatable-cookie/oxide/pull/21#issuecomment-6101426638));
merge was `df7f084f6e89481b4b0a1b414abc32f23728d922`. That evidence does not
cover this workflow's third-party/native inputs, consumer authentication,
root recycling, successful live cold recovery or everyday storage/write relief.
Task 017's negative mapped-root evidence and unknown lease remain preserved.

## Implemented registered library boundary

`oxide_storage::registered` is a separate version-1 library route. It does not
change the frozen nine-file/seven-unit fixture, the production adapter default,
Cargo fingerprints, storage transport or Contract010. It has no registered
Nucleus workflow, built-in trusted issuer, scheduler or consumer authentication.
Installing a trusted owner is an embedding decision, never task configuration.
There is no hardcoded project identity allowlist in this route.

`RegistrationOwner` authenticates the exact attempt, current exclusive lease and
physical allocation, clean committed source receipt, and separately audits the
complete registration's supported behavior closure. Versioned source, dependency
and effective-configuration contracts bind literal executable/argv/cwd/env,
registered graph/units/packages/features (through the audited graph digest),
toolchain/configuration/debug identities, root/partition/generation and declared
source inventory. Effective Cargo-generated and external unit environment values
have a separately audited per-unit map with explicit presence/absence; they are
not injected into or inferred from the launcher environment. Each attempt retains distinct task/workspace/run/commit/source
and allocation-epoch identity. The source content digest frames sorted UTF-8
relative filenames and their SHA-256 digest strings with little-endian u64 byte
lengths; it is not the Git tree object ID. The consumer owns Git cleanliness,
complete behavior-file inventory and durable committed objects. Dirty source,
submodules, LFS, new graphs, wrappers and opaque reads require refusal under this
initial policy, not a caller assertion that they are safe.

`InputEvidenceOwner` resolves only accepted issuer references. Registry receipts
must bind the locked archive checksum, exact extracted file/directory inventory/content,
canonical source root, provenance and immutable epoch. Tool/SDK receipts bind
bounded reachable files and their provenance/no-replacement agreement. Oxide
checks every listed digest, exact file membership, stable reads and physical
file identity at admission/finalize. The launcher must itself occur in an
accepted tool manifest. The accepted owner still preserves the no-replacement
agreement through the lease. Verified archive/extraction bytes are not a trusted
issuer, do not prove native/proc-macro/C/header closure, and do not qualify a
Nucleus workflow. The separate audited closure remains essential. There is no
archive extractor, whole-host scanner or shipped native/SDK issuer here.

A bounded verifier in `src/registered/archive.rs` proves an explicitly supplied
locked registry archive corresponds to an explicitly owned extracted package
root. The supported form is a single-member gzip (magic `1f 8b`) of a
ustar/PAX or legacy GNU tar whose members share one canonical package
top-level prefix, matching a Cargo `.crate`. The extracted directory's last
component must equal that prefix. The verifier enforces the locked SHA-256 of the compressed archive
bytes (`sha256:` plus 64 lowercase hex), exact file content digests, and exact
file/directory membership, including empty directories and parent directories
implied by files or by nested empty directories. Implied parents are inventory
members and charge the same 32,768-entry cap as tar headers and extracted
listings. Duplicate tar members refuse,
including a repeated prefix-only package-root directory header. Extracted
directory membership is snapshotted before and after each directory walk;
identity or membership change refuses as unstable. Each listing charges a child
against the inventory cap before retaining its path; a directory that would
exceed the cap refuses without materializing the remainder. The after-walk
snapshot is capped at the before set plus one extra member. `VerifiedCorrespondence` is
constructed only by `verify`; its fields are private. Conversion to
`ImmutableManifest` is `VerifiedCorrespondence::into_registered` and requires
`TrustedContext::new` with an already-accepted `InputEvidenceOwner`, that
owner's issuer, source identity and epoch, plus the receipt and `Attempt` the
owner authenticates. Conversion calls `InputEvidenceOwner::immutable` and
accepts the issued manifest only when it is `ImmutableClass::Registry` with
`ArchiveEvidence` matching the verified checksum, paths and inventory. It does
not mint an issuer from task strings and never infers generated, native or
proc-macro provenance. An owner that returns `MissingAuthority` refuses
conversion.

Local Cargo sidecars `.cargo-ok` and `.cargo-checksum.json` may exist at the
extracted package root. They are excluded from archive evidence and never
attest archive contents. Those exact root-relative names as archive members
refuse. Nested paths that use the same file names (`src/.cargo-ok`,
`src/.cargo-checksum.json`) are ordinary members: archive and extraction bytes
and membership must match exactly, and matching files enter the verified
inventory. Unmatched extra extracted files, including unmatched nested
sidecar-named files, refuse; extra files are not skipped. Symlink, hardlink and special tar entries, extracted symlinks,
extracted regular files with `nlink ≠ 1`, and extracted special files refuse.
Absolute/parent/escaping paths, `.`/`..` components, backslashes,
non-UTF-8 names, duplicate or file/directory collisions, unknown formats
(including V7/old headers, bare tar, extra gzip members and trailing compressed bytes),
corrupt or truncated streams, and unstable reads refuse.

Reads charge one shared preparation budget that is not reset per member, phase
or package: compressed archive bytes, decompressed tar bytes, and extracted-file
bytes. Compressed archives, each decompressed package tar, and each extracted
file have a 16 MiB cap; the verifier refuses at the cap if it cannot establish
EOF without another read. Archive and extracted-file reads also stay within the
remaining collection budget. Counters stay at or below the recorded ceiling.
`verify(request, limits, counters)` continues from the caller-supplied
`Counters` and returns `Outcome` with the aggregate `Counters`
(`archive_bytes`, `decompressed_bytes`, `extracted_bytes`, `entries`) and a
`Refusal` category on every attempt, including success. Nucleus engineering
maxima are 256 MiB collection, 16 MiB per decoded or extracted file, 32,768
inventory entries, 2,048-byte paths and depth 32; callers may pass lower
bounds. The collection cap applies to the accumulated counters, so successive
package verifies in one preparation share it. There is no unpack/write into the
package root, host registry scan, per-call counter reset or automatic growth.

A distinct private streaming route, `archive::stream_verify`, exists for a
locked container whose total decoded tar size genuinely exceeds 16 MiB while
every individual archive member and extracted file stays at or below 16 MiB. It
streams the single gzip member through a metered reader and raw tar entries, so
the complete decoded tar is never materialized and no per-container cap applies;
only the unchanged shared collection budget (256 MiB across compressed, decoded
and extracted bytes) bounds the aggregate. The compressed archive, every archive
member payload and every extracted file remain capped at 16 MiB, and each
declared member size is checked before its payload is consumed. GNU long-name,
long-link and PAX extension entries are refused before the tar reader can buffer
an unbounded metadata payload. The gzip trailer, single-member and trailing-byte
checks still run, and decoded bytes after the tar terminator are charged and must
be NUL padding, so a smuggled trailing member refuses. Entry, path, depth,
duplicate, collision, sidecar, link, hardlink, nlink, stability, membership,
content and identity rules are shared with `verify`. Both routes consume one
caller-supplied `Counters` and return every consumed counter on success and
refusal. The schema-v2 `verify` container cap is unchanged.

Task 037 added a private, opt-in acquisition caller at
`src/registered/archive_acquisition.rs` and
`src/bin/registry_acquisition.rs`, selected by
`effigy registry:acquire:nucleus`. The Python caller hashes the five
allowlisted source leaves read from the pinned Git blobs. The Rust entry
requires the pinned commit/tree, the exact five-leaf name set with
well-formed digests, the pinned `Cargo.lock` digest, and the pinned 58-package
tuple inventory digest; it does not pin the other four leaf digests itself.
It requires explicit `CARGO_HOME=/Users/tom/.cargo`, examines at most eight
immediate namespace names per cache/source root, and tests only lock-derived
archive and extracted-directory names. Canonical containment,
unique candidates, metadata stability and one shared `archive::verify`
counter set are enforced; the JSON output is capped at 8 MiB. It does not read
registry index data, fall back to arbitrary paths, modify installed roots, call
`into_registered`, or issue registry authority. The Python entry reads only
the pinned Nucleus Git blobs, the two allowed Cargo config files, and
verified-package behavior leaves under the task's read limits; it does not
invoke Cargo. Task 037 observed one supported real correspondence and refused
57 of 58 locked registry packages. See the
[`nucleus-input-roots.md`](nucleus-input-roots.md#task-037-lock-and-registry-correspondence-update)
evidence update for exact outcomes and limits. This is not a complete registry
receipt, behavior graph, immutability agreement, or Nucleus build qualification.

## Task 038 archive-format and registry-pairing correction

Task 038 retained the Task 037 report unchanged and corrected the two observed
refusal classes. Bounded diagnostics found that `bitflags 2.13.2` and
`cc 1.4.7` use legacy GNU tar headers (`ustar ` magic, ` \0` version); the old
verifier rejected them at its `as_ustar()` gate before checking member types or
paths. The verifier now accepts those headers while retaining checksum,
single-gzip, entry type, path, duplicate, inventory and stability checks. The
same-namespace pairing selects exactly one complete archive/extraction pair;
an unmatched spare archive no longer creates ambiguity. Multiple or absent
complete pairs still refuse. The serde observation selected the sole complete
pair in `index.crates.io-1949cf8c6b5b557f`, while retaining the unmatched spare
archive candidate from `index.crates.io-6f17d22bba15001f`.

The initial 58-package attempt at `ee9f3319095632f4ed42d2f11608eb3cf18aecc1`
verified 56 packages but read one overflow byte past the decompressed 16 MiB
limit for each of `libsqlite3-sys 0.38.2` and `sqlite-wasm-rs 0.5.5`. Its exact
receipt is preserved as the
[initial attempt](../evidence/nucleus-registry-acquisition-038-initial-attempt.json).
Follow-up commit `8bd9949623a06df63cadd9cb7ea3bdbcebe8a70a` fixed the cap check.
A corrected 58-package attempt at
`98629cfc05ece0cdc0fde3b562e3be48b9e8d3a0` carried forward the first attempt's
counters and verified 56 packages. The same two packages refused at exactly
16 MiB, before any tar entry or extracted-file read. The
[final acquisition evidence](../evidence/nucleus-registry-acquisition-038.json)
keeps its per-package deltas separate from cumulative counters and links the
prior receipt by digest.

Across diagnosis and both attempts, the phase ledger records 207,717,057
compressed/decompressed/extracted archive bytes, 89,716 pinned-source bytes,
60 allowlisted-config bytes, 600,545 behavior-triage bytes, 54,218 diagnostic
receipt bytes, 662,827 bytes to carry the prior report, 466 candidate-path
observations, and 209,124,423 aggregate bytes. Triage reached the shared cap of
128 leaves in the first attempt and was not repeated. This remains filesystem
correspondence only: it establishes neither effective Cargo source selection
nor a registry issuer, complete behavior graph, native closure, immutability
agreement, or Nucleus build qualification. Acquisition request/report schema
v2 carries counters across the diagnostic and refresh phases; the corrective
handover also preserves them across the bounded retry.

The unit seam consumes owner-verified semantic evidence for pinned Cargo
`c980f4866` [`LocalFingerprint`](https://github.com/rust-lang/cargo/blob/c980f4866/src/cargo/core/compiler/fingerprint/mod.rs).
It supports multiple dep-info/script file and environment references, explicit
presence/absence, finite directory membership including empty directories,
audited immutable-registry `Precalculated`, and explicitly associated generated
producer/native-tool inputs. Every declared source file needs a consuming
anchor, and every consuming anchor is retained; changed content with old/equal
mtime relative to any anchor refuses. Immutable changes always refuse regardless
of mtime. `Precalculated` on mutable source, unknown units, unassociated generated
inputs, unsupported semantic variants, symlink/escape/special-file paths and
budget overflow refuse. Immutable directory rerun inputs are conservatively
unsupported; their whole-package immutable inventory is verified independently.
Rust dep-info supplies no inferred C/header coverage.

These are typed library semantics, **not** a newly invented Cargo capture format.
A bounded decoder for the exact Cargo
[`EncodedDepInfo`](https://github.com/rust-lang/cargo/blob/c980f4866/src/cargo/core/compiler/fingerprint/dep_info.rs)
v1 byte format now lives in `src/registered/dep_info.rs`. It accepts an explicit
byte slice up to 16 MiB; file and environment counts are each capped at 32,768,
and UTF-8 strings at 2,048 bytes, matching the registered file, association and
path/value ceilings. It requires marker `01 00 00 00 ff`, version `01`,
little-endian u32 counts/lengths, path tags 0 (package-root-relative) and 1
(build-root-relative), and strict 0/1 presence markers. It retains path tags,
file size/checksum text, and environment `None` versus `Some("")` versus a
nonempty value. It refuses non-UTF-8 strings, invalid tags/markers/bools,
truncation, trailing bytes, duplicate file references or environment keys,
oversized counts/strings, and unsupported versions. The literal hand-annotated
byte vector is a pinned-format test fixture, not compiler-run evidence.

The conversion seam requires trusted owner context containing the authenticated
unit and consuming anchor plus an exact mapping for every `(path tag, path)` to
one registered `InputPath` and its association. Missing, duplicate or extra
mappings, unsafe paths, and generated inputs without a registered producer
refuse. The runner checks the converted unit against `UnitEvidence.id`; it keeps
its existing source inventory, anchor, mtime, integrity and identity checks.
Checksum-bearing files decode without loss but conversion refuses them because
`LocalInput::DepInfo` does not represent checksum freshness; they are never
downgraded to mtime evidence. The decoder does not resolve paths or inspect the
filesystem, build a graph, infer native/generated producers, or authenticate an
issuer. A real owner must still audit complete input behavior and supply actual
associations. No raw Nucleus fingerprint/unit/native observation was made. The
remaining evidence includes the complete real input audit, authentic immutable
registry/native SDK manifests, and genuine consumer/scheduler/test/debug
capture receipts. No mock owner or fixture completion can activate the default
adapter.

`ExecutionOwner` alone submits the approved foreground launch through the genuine
scheduler and validates independently owned completion/test/artifact/debug capture
and durable exact-lease Idle receipts. `CaptureInputs` binds that verification
to the collected source/dependency/generated digests and map identity. The command builder clears inherited env,
uses literal argv, and adds only the authenticated slot, private target and temp
paths as per-run relocation fields. The behavior audit must approve those path
relocations; behavior-bearing paths cannot silently become partition-independent.
No host token or settlement is manufactured. Cooperative owners must wait for
writing children and close handles; unrelated same-user writers remain an
unsupported bypass. In-process owner implementations are trusted integration
code, not a security sandbox for hostile implementations of these traits.

The registered map has its own bounded versioned encoding and owner-authenticated
seal. A durable pending barrier precedes scheduler submission. Complete successful
capture and unchanged admitted source/tools produce a candidate; only confirmed
authenticated Idle removes the barrier. Failed/partial success, capture failure,
mutation, cancellation or lost replies leave it ineligible, including after
reconciliation to Idle. An authenticated proof that submission/writing never
started preserves the prior map. No public recovery/removal or ordinary-acquire
eviction operation is added.

The engineering ceilings below are enforced independently from fixture limits.
Each admission/finalize has one source byte budget, one collection byte budget
(shared by immutable manifests and unit inputs, with deduplicated reads) and at
most one bounded map read. Metadata membership/stability checks are separately
finite and may conservatively refuse below a byte ceiling. An unstable read or
oversized file refuses rather than truncating. Evidence owners must use bounded
pre-issued receipts or charge their own separately approved preparation; they
must not hide host scans behind these seams. Ordinary synthetic regressions
exercise these semantics in fresh owned scratch and claim no actual Cargo reuse,
Nucleus eligibility, representative fit or independent qualification.

## Task 039 private streaming correspondence

Task 039 added the distinct private schema-v3 streaming route
`archive::stream_verify` and its opt-in caller
`archive_acquisition::run_streaming` / `oxide-registry-acquisition --stream`,
selected by `effigy registry:stream:nucleus`. It binds the same pinned source
commit/tree/lock and the five recorded per-leaf digests as constants, accepts
only the exact two SQLite registry tuples of Task 038, and reuses the shared
candidate discovery, path safety, membership, integrity and counter logic. The
schema-v2 legacy `verify` route, its 16 MiB decoded-container cap and the
default callers are unchanged; caller or task JSON cannot raise the member,
collection or entry ceilings. Existing map constants and the old receipts are
not reinterpreted.

The one bounded intended observation at implementation
`d9983e750a04354d14744a18a9bbfda43da92b2e` (tree
`2ab95ce285fe212eec18828bfd97c0f3f928ce8e`) applied the streaming route to both
previously refused pairs. Both reached `supported`:

| Package | Compressed | Decoded tar | Extracted | Entries | Files | Directories |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `libsqlite3-sys 0.38.2` | 5,361,739 | 21,646,848 | 21,624,199 | 61 | 26 | 4 |
| `sqlite-wasm-rs 0.5.5` | 5,954,429 | 24,785,920 | 24,724,725 | 189 | 77 | 17 |

Each decoded container exceeds the 16 MiB legacy cap while every member is well
below the 16 MiB member cap. The phase used one shared counter set starting at
zero: 11,316,168 compressed, 46,432,768 decoded, 46,348,924 extracted bytes and
250 entries; 104,097,860 archive bytes total, 8 candidate-path observations and
a 20,670-byte report. Adding the 611,520-byte Task 038 receipt read for
continuity gives 104,730,050 aggregate bytes, within the 256 MiB owner-phase and
260 MiB total-read ceilings, and the 20 GiB host reserve held. The full receipt
is
[`nucleus-registry-acquisition-039.json`](../evidence/nucleus-registry-acquisition-039.json).
No legacy `verify` call ran and no 58-package replay, package script, macro,
helper, Cargo, compiler, SDK or native content was read or executed.

This is filesystem correspondence only. It does not establish effective Cargo
registry selection, a registry issuer, enabled units, native/header/proc-macro
or SDK closure, an immutability/no-replacement agreement, or a successful
Nucleus workflow. The Task 037 and Task 038 reports remain unchanged.

## Task 040 pre-execution behavior audit (read-only)

Task 040 produced the first source-backed, pre-execution behavior audit of the
pinned `check:core:test` workflow and one finite next acquisition package. It
ran no Cargo, rustc, selector, script, macro or helper, read no tool/SDK/sysroot
content and replayed no archive. The full record, per-leaf digests and ledgers
are in
[`nucleus-core-test-behavior-audit-040.json`](../evidence/nucleus-core-test-behavior-audit-040.json).

Read boundary: fourteen pinned Nucleus Git-object leaves (365,037 bytes),
seventy-five declared leaves from the six permitted extracted registry roots
(1,789,111 bytes), and the existing contracts and prior receipts. Every root
matched its committed Task038/Task039 name, version, checksum, file/directory
count and pinned lock tuple. All 75 read leaves bind exactly to a retained
per-file SHA-256: 65 to Task038 (`cc`, `pkg-config`, `vcpkg`, `serde_derive`,
`schemars_derive`) and all ten `libsqlite3-sys 0.38.2` leaves to Task039, which
retains a 26-file/4-directory manifest. Registry content correspondence is
observed, not a gap.

Behavior findings. The selected `build_diagnostics` test is
`#[cfg(debug_assertions)]` and reads its own line-table/backtrace data.
`tests/planning.rs` has 50 tests and its compile-time inputs are
`include_str!("../examples/portable-records.json")` and
`include_bytes!/include_str!("fixtures/decision-self-relay-v1.json")`; at
runtime it resolves and spawns `mktemp`, writes SQLite WAL stores under
`std::env::temp_dir()`, and cleans them up. It does not exercise the
`nucleus-observer` `/bin/ps`, `/usr/sbin/lsof` (with a `lsof` PATH fallback),
`/bin/kill -0` or `/usr/bin/id` reads, which are compiled but unused here.
`rusqlite` `bundled` reaches `libsqlite3-sys 0.38.2` `build_bundled::main`,
which copies `sqlite3/bindgen_bundled_version.rs` to `$OUT_DIR/bindgen.rs`,
includes the `sqlite3` directory, declares `sqlite3/sqlite3.c` and
`sqlite3/wasm32-wasi-vfs.c` as changed-file inputs, and compiles `sqlite3.c`
through `cc::Build` with fixed flags. It reads `TARGET`, `HOST`, `OUT_DIR`,
`CARGO_CFG_*`, `SQLITE_MAX_*`, `LIBSQLITE3_FLAGS` and
`LIBSQLITE3_SYS_USE_PKG_CONFIG`. `cc 1.4.7` resolves target-qualified
`CC`/`CFLAGS`/`AR`/`ARFLAGS`/`CXX` variables, `SDKROOT`, `RUSTC_WRAPPER` and
`RUSTC_LINKER`, executes compiler/archiver/linker/`rustc` probes, consults
`xcrun` on Apple and writes temporary flag-check units. `pkg-config` and `vcpkg`
are in the lock's `libsqlite3-sys` dependency edges (its default
`min_sqlite_version_3_34_1` feature). `build_bundled::main` does not call them,
but `LIBSQLITE3_SYS_USE_PKG_CONFIG` present and not `0` (or feature
`loadable_extension`) routes build.rs `main` to `build_linked` before the
`bundled` branch (build.rs:79-83), which does call pkg-config and vcpkg. The
owning contract requires that variable absent or `0`; its effective presence is
a decisive unresolved input here, not an unreachable branch.
`serde_derive 1.0.229` and `schemars_derive 0.8.22` are token-only within their
own crates; the only compile-time environment use is `serde_derive`'s
`env!("CARGO_PKG_VERSION_PATCH")`. `proc-macro2`, `quote`, `syn`, `serde_core`,
`serde_derive_internals` and `thiserror-impl` are outside the permitted roots,
so the reported rustc probes/private-module behavior is not re-observed.

Decision. The registered boundary covers the six permitted roots' per-file
registry receipts (Task038 for `cc`, `pkg-config`, `vcpkg`, `serde_derive` and
`schemars_derive`; Task039 for `libsqlite3-sys`), the `$OUT_DIR/bindgen.rs`
generated-association, and the pinned dep-info mechanism. It cannot cover, and
no accepted receipt exists for: the native C/header closure; `cc`/tool/SDK
integrity; effective Cargo configuration and environment; the enabled semantic
unit graph; registry installation provenance; a continuous no-replacement
agreement; the native consuming associations; genuine run/debug evidence; or a
real accepted graph/audit issuer, which remains synthetic-only under oxide#035.
The one finite next acquisition package has two visibly separate parts: a
Cargo-free accepted native/tool/SDK/config receipt (native C inputs,
toolchain/SDK content identities, effective configuration inventory,
no-replacement and installation-provenance evidence, and the issuer ruling),
plus a separately scoped Cargo-side enabled-unit-graph observation. The
retained Task038/Task039 registry manifests are reused and are not re-acquired.
Three owner rulings are still unrecorded: a qualification-only real verifier/issuer
to replace the synthetic-only oxide#035 policy; bounded tool/SDK read and
content-integrity acquisition (this brief permits zero tool/SDK reads); and a
Cargo-side or accepted-equivalent enabled-unit-graph observation. The actual
enabled graph size is unavailable, so no fit or infeasibility assertion is made
for the remaining <=4 Cargo/45-minute trial.

Disposition: **BLOCKED for eligibility**, with the precise map above and the
one acquisition package named; ordinary fresh private validation is retained.
This audit grants no real issuer, graph, native/tool receipt or consumer
adoption.

## Task 041 unit-graph and tool-size feasibility (read-only)

Task 041 resolved the two feasibility questions Task 040 exposed: whether
Cargo's own pinned planner can yield actual command-specific units without
nightly or executing a build, and whether the explicitly named installed tools
fit the unchanged 16 MiB/file evidence contract. It ran no Cargo, rustc,
selector, script or tool, read no tool/SDK/rustlib content and installed
nothing. It acquired sixteen pinned Cargo `c980f4866141969fab6254a680546a277789d6f0`
source files (672,998 bytes) through `raw.githubusercontent.com` and made
metadata-only `lstat`/`readlink` observations for the eight named paths. The
full record, per-file source ledger and refusal notes are in
[`cargo-unit-graph-and-tool-fit-041.json`](../evidence/cargo-unit-graph-and-tool-fit-041.json).

Graph feasibility. The pinned planner does construct command-specific units:
`compile_ws` -> `create_bcx` runs `RustcTargetData::new`, `ops::resolve_ws_with_opts`,
`Profiles::new`, `UnitGenerator::generate_root_units`, `build_unit_dependencies`
and `rebuild_unit_graph_shared` before `unit_graph::emit_serialized_unit_graph`
writes units with package, target, profile, platform, mode, features and
dependencies (`src/cargo/ops/cargo_compile/mod.rs:157-540`,
`src/cargo/core/compiler/unit_graph.rs`). The emit returns before
`BuildRunner::new` and the compiler queue, so nothing is built. But there is no
supported stable CLI: `--unit-graph` is unstable
(`src/cargo/util/command_prelude.rs:818-827`) and is gated by
`fail_if_stable_opt("--unit-graph", 8002)`, whose non-nightly branch reports
that the flag is only available on the nightly channel
(`src/cargo/core/features.rs:1476-1505`; `nightly_features_allowed` comes from
the channel at `src/cargo/util/context/mod.rs:397`, and `rust-toolchain.toml`
pins stable `1.97.1`). The pinned reference documents the nightly invocation and
states that `cargo metadata` fundamentally cannot represent command-specific or
cross-dependency-kind features (`src/doc/src/reference/unstable.md:733-757`).
The planner is also not non-executing: `TargetInfo::new` invokes
`rustc --print=file-names/sysroot/split-debuginfo/crate-name/cfg`
(`src/cargo/core/compiler/build_context/target_info.rs:168-221`), and
`resolve_with_registry` can write `Cargo.lock` and `resolve_ws_with_opts` calls
`pkg_set.download_accessible` (`src/cargo/ops/resolve.rs:274,370`). There is
no public stable graph API; the only non-CLI route is a vendored/forked pinned
Cargo library calling `create_bcx` + `emit_serialized_unit_graph`, a new
mechanism against unstable internal APIs. A `UnitGraph` alone still supplies no
audited behavior, consuming anchors, native associations, issuer or
installation provenance. Verdict: **NO-GO** for a supported stable or truly
nonexecuting producer under the unchanged constraints.

Tool metadata fit. The required Rust toolchain `cargo` is 31,867,672 bytes
(30.39 MiB) and the effective C/C++ driver `clang` is 124,676,976 bytes
(118.90 MiB); `cc` is a single symlink to `clang`. Both exceed the unchanged
16 MiB (`16,777,216`-byte) per-file bound before any content read. The Runner's
`ImmutableClass::ToolSdk` path requires complete-root membership plus a
whole-file digest under `Limits::file_bytes` and a shared 256 MiB collection
(`src/registered.rs:62-74,554-573,638-683,990-1090`), so even a narrow-root
receipt must still enumerate the whole toolchain root and stay within the
entry and collection caps. `rustc` (412,504 B), `ar` (73,520 B), `ld`
(2,749,664 B), `xcrun` (117,104 B) and `mktemp` (101,536 B) fit individually;
no missing or unstable required file was observed. Verdict: **NO-GO** under the
unchanged per-file bound; no stream, new class or cap raise is authorized here.

Disposition. The two questions both resolve NO-GO under unchanged constraints,
so the material mechanism-policy choice must be recorded rather than another
wrapper: an explicitly different workflow/unstable route (consumer-side
`--unit-graph` captured before the build), a pinned Cargo embedding/fork, or a
new bounded ToolSdk policy for the oversized named binaries. The one concrete
next mechanism proposal, for a separately scoped operator ruling only, is the
explicitly different workflow/unstable route; it is viable only alongside a
separate bounded ToolSdk policy ruling for `cargo`/`clang`, which is recorded
as a required material choice rather than a second mechanism. None of these
choices is applied or authorized by this feasibility PR. Eligibility stays
**BLOCKED** as in Task 040; ordinary fresh private validation is retained and
no reuse, benefit or storage claim is made.

## Task 042 graph-route and installation-evidence design (design-only)

Status: **proposed route, no producer or invocation approved.** Task 042 is the
DOCS-ONLY follow-up to the Task 041 NO-GO. Its final source ledger has 15 pinned
Cargo files (568,816 bytes; the 041 digests match for every overlapping file).
Task 042 ran no Cargo, rustc, build script or tool, read no tool/SDK content and
installed nothing. The record is
[`cargo-graph-route-and-installation-evidence-042.json`](../evidence/cargo-graph-route-and-installation-evidence-042.json).

### Graph route: same pinned Cargo behind a development-channel gate

Recommended candidate **A**: the same Cargo 1.97.1 (`c980f48`) planner, run as
`cargo build|test --unit-graph -Z unstable-options --frozen` with
`__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS=nightly` in the graph process
only. It is an unstable bypass, never a stable or supported feature. Its
planner runs the same code path as the ordinary build: `create_bcx` runs in
both, and the emit at `cargo_compile/mod.rs:188-190` returns before compilation.
Task 042 initially summarized two differences: the gate accepts unstable
manifest and `-Z`/`[unstable]` input, and it serializes three unit-graph
dependency fields that stable never emits. A later planner whole-tree text
search found additional channel-dependent sites, so this is not a complete
equivalence inventory.

The source gate, at this pin:

- `features.rs:1563-1566` honours the override for any value. `RUSTC_BOOTSTRAP=1`
  (`features.rs:1570-1573`) also maps to `dev`, but rustc reads it too, so it is
  refused as the gate.
- `context/mod.rs:397` sets `nightly_features_allowed` once. It also gates
  `-Z` parsing (`:1155`, `features.rs:1244-1246`), the `[unstable]` table
  (`context/mod.rs:1226`), unstable manifest `cargo-features` (`features.rs:681-684`),
  and whether the unit-graph `public/noprelude/nounused` fields are serialized
  (`unit_graph.rs:115`). Those fields are computed in `unit_dependencies.rs`
  regardless of channel. Normal edges take `is_public_dep`, with `noprelude` and
  `nounused` false (`:942-944`). Only the std-root artificial edge sets all three
  true (`:207-210`), and it is attached only when `std_roots` is non-empty, which
  is the build-std path. Other `UnitDep` constructors were not traced. So graph
  success does not imply ordinary stable success, because of the manifest and
  `-Z` input gates.
- `--unit-graph` exists on `build` (`commands/build.rs:38`) and `test`
  (`commands/test.rs:60`). It is gated by `fail_if_stable_opt`
  (`command_prelude.rs:823-825`). Removing the override makes the graph run fail
  closed, with Cargo's own nightly-only refusal.

The planner completed the whole-tree text-search prerequisite against the
pinned public Cargo source archive
`https://codeload.github.com/rust-lang/cargo/tar.gz/c980f4866141969fab6254a680546a277789d6f0`
(SHA-256 `74431490023e3fbcd6bb72abf09cf54b0339b850bc49e1b2446c88a1be54ea9b`,
4,439,958 compressed bytes, 2,918 regular files, 17,626,902 expanded bytes).
Every regular member was searched for `nightly_features_allowed` and
`channel()`. Besides the sites traced above, the results and targeted source
review found `-C` changing Cargo's working directory behind unstable options
in `src/bin/cargo/cli.rs`, nightly `CARGO_COMPLETE` handling in
`src/bin/cargo/main.rs`, version-5 lockfile acceptance behind
`next-lockfile-bump` in `src/cargo/core/resolver/encode.rs`, and a changed
`cargo-lint` warning in `src/cargo/util/toml/mod.rs`. Other hits include
`src/cargo/core/compiler/custom_build.rs`, `src/cargo/ops/fix/mod.rs`, feature
diagnostics and documentation links; unrelated asynchronous `channel()` hits
were not gate evidence. The source search is textual provenance, not a proof
that the graph route is behaviorally equivalent to an ordinary stable build,
and it is not consent to invoke the unstable route.

Alternatives, all rejected for now:

- **B, separately pinned nightly capture.** Documented, but it is a different
  Cargo and rustc from the ordinary 1.97.1 path, and planner equivalence is not
  assumed. It needs a toolchain install that is not approved. Fallback only.
- **C, vendored or forked library** (`create_bcx` + `emit_serialized_unit_graph`).
  Internal unstable APIs, a larger maintenance surface, and it still runs probes.
- **D, `cargo metadata`.** Cannot express command-specific features (041).
- **E, no graph.** The current NO-GO disposition stays.

Reach and side effects that the graph run cannot avoid:

- **rustc probes.** `target_info.rs:168-221` runs `rustc --print=...` per kind,
  and `util/rustc.rs:65-68` runs `rustc -vV`. The rustc-info cache goes to
  `ws.build_dir()/.rustc_info.json` (`context/mod.rs:510-511`). Probes use the
  consumer's rustc, so that rustc must be receipted and the build dir private.
- **`[env]` config.** `core/compiler/mod.rs:2643-2655` applies `[env]` values to
  rustc and probe processes, unless Cargo already set that key on the builder.
  A config `RUSTC_BOOTSTRAP` therefore reaches rustc in ordinary runs. The
  channel is read from the process env only, so `[env]` cannot flip the gate.
- **Lock and network.** `ops/lockfile.rs:55-66` refuses lock writes under
  `--locked`/`--frozen`. `--frozen` also sets `offline_flag`
  (`context/mod.rs:1254-1262`). The enforcement point in the source layer was not
  in the fetched set; prefer a private directory-vendored source so no network
  source exists.
- **No build.** `compile_ws` returns before `BuildRunner`. Build scripts, proc
  macros, test binaries and crate compiles do not run. The graph JSON omits the
  rustc version, cfg, lock checksums, config, env, build-script output, native
  closure and proc-macro helpers.

Binding and refusal, proposed:

- Graph argv = ordinary argv + exactly `{--unit-graph, -Z, unstable-options, --frozen}`.
  Graph env = ordinary env + exactly the override. Any other difference refuses.
- A provenance record binds the Cargo and rustc receipts, `rustc -vV`, host
  triple, Git tree, lock digest before and after (equal), manifest and config
  file-set digests, env digest with secrets removed, private CARGO_HOME and
  target/build dirs, argv, the selector/profile/feature inputs, the graph bytes
  digest, and the clock/invocation accounting.
- The decoder accepts only `version: 1` (`unit_graph.rs:53`). Unknown fields
  refuse. Under the gate, `public`, `noprelude` and `nounused` are known v1
  fields. Value rule: `noprelude` and `nounused` must be `false` or absent on
  every edge, else refuse. A `true` value comes only from the std artificial
  edge, which needs `-Z build-std` and is refused by the argv rule. `public` may
  be either value and is part of the edge identity, so a change in it changes
  the reuse key. Stable never emits these fields, and the ordinary path does not
  produce a graph. Each target `src_path` must lie under a registered root. The
  graph root set must equal the ordinary root selection.
- Consumer rules, both runs: refuse any `[env]`, `--config env.*`, `RUSTC*`/
  `build.rustc*` wrapper, `[unstable]` table, `-Z` outside the gate, or manifest
  `cargo-features`.
- The record is produced by the consumer's admitted host run and settled by the
  consumer owner. Oxide verifies bytes and binding. No issuer attestation exists.

The whole-tree search is complete as a text inventory, but it is not a complete
route audit or equivalence proof. Any future route brief still needs to account
for the additional sites above, the exact invocation and consumer inputs, and
the distinction between graph output and stable execution. It grants no route
activation authority.

### Task 043 pinned graph decoder and provenance verifier (parser only)

The Oxide-only Rust API is
`registered::cargo_unit_graph::{decode_unit_graph, verify_provenance,
framed_source_content_digest}`. It is parsing groundwork; the proposed Cargo
producer and any unstable graph invocation remain unimplemented and
unauthorized.

Its public signatures are:

```rust
pub fn decode_unit_graph(
    bytes: &[u8],
    registered_roots: &[&std::path::Path],
    limits: CargoUnitGraphLimits,
) -> Result<CargoUnitGraph, CargoUnitGraphRefusal>;

pub fn verify_provenance(
    graph_bytes: &[u8],
    provenance_bytes: &[u8],
    expected: &CargoUnitGraphBindings,
    limits: CargoUnitGraphLimits,
) -> Result<(), CargoUnitGraphRefusal>;

pub fn framed_source_content_digest(
    files: &std::collections::BTreeMap<String, String>,
    limits: CargoUnitGraphLimits,
) -> Result<String, CargoUnitGraphRefusal>;
```

`CargoUnitGraphBindings` and `CargoUnitGraphRunAccounting` are caller-built
expected identities and are serializable for record construction; they do not
implement `Deserialize`, so the untrusted record cannot supply the expected
side through deserialization.

`decode_unit_graph(bytes, registered_roots, limits)` accepts only the pinned
Cargo 1.97.1/c980f48 unit-graph v1 shape. `registered_roots` are explicit
lexical absolute roots supplied by the caller; no filesystem is inspected.
The returned graph retains package IDs, target fields and source paths, the
complete validated profile value, platform, mode, features, roots, and each
edge's index, extern name and optional `public`/`noprelude`/`nounused` values.
Duplicate JSON keys are rejected before map construction, as are unknown
fields, malformed or trailing bytes, unsupported values and invalid indices.
Omitted `is_std` is false; a true value refuses build-std. `noprelude` and
`nounused` may be omitted or false; true refuses. `public` may be either bool
or omitted and its presence/value remains in the edge. Every target `src_path`
must be under one supplied root. The default and maximum limits are 8 MiB,
256 units or roots, 32,768 dependency edges or provenance identity-map entries,
2,048 path bytes and depth 32; callers may choose lower ceilings, including
zero, never higher ones. Registered roots are also bounded by the unit limit.

`verify_provenance(graph_bytes, provenance_bytes, expected, limits)` accepts
the version-1 `oxide-cargo-unit-graph-provenance-v1` record only when its
digest matches the exact graph bytes and its bindings equal the separate
constructor-supplied `CargoUnitGraphBindings`. The record carries the pinned
Cargo version/commit, rustc version, `rustc -vV` digest, host triple and
tool-receipt identity digests; source Git commit/tree and separate framed
source-content digest; unchanged lock digests; manifest/config-set,
argv/selector/profile/features/target/environment digests; private path
identity digests; and supplied task/workspace/run/attempt and clock/invocation
accounting. `framed_source_content_digest(files, limits)` frames each sorted
relative path and supplied `sha256:` file digest with an unsigned 64-bit
little-endian byte length. It reads no source files and keeps this content
identity distinct from Git commit/tree IDs. Identity fields contain digests or
IDs, not environment values, credentials or private paths. The embedding must
construct expected digests from its canonical inputs; this verifier compares
them but does not compute argv, environment, configuration or path identities.
The caller separately decodes graph bytes with `decode_unit_graph`; the
provenance verifier itself checks exact-byte digest correspondence and does not
decode the graph. It refuses missing or unknown fields, duplicate keys,
pin/digest/lock/binding mismatch and invalid accounting.

This verifies internal correspondence only. It does not authenticate genuine
settlement, establish verified tool bytes, prove source completeness, audit
native or proc-macro behavior, construct `TrustedCargoGraphContext`, or make a
graph owner/issuer trusted. Nothing in this parser enables a producer, consumer
execution, installation, trial, or reuse eligibility.

### Installation evidence for oversized tools

Recommended: **owner-installed immutable receipts (C)** plus **bounded streaming
complete-content verification (B)**. Not a per-file cap raise (A). The budgets
below are proposals. None is measured or ruled.

- **B, streaming verification.** Hash each file in 1 MiB chunks. Resident memory
  is the chunk plus the manifest. Refuse if dev/inode/size/mtime/ctime change
  around the read, the same pattern as `src/registered.rs:559-573`. No sampled
  digest, no split-file exemption, no size-only check.
- **C, receipts.** The installation owner commits the root identity, an epoch
  id, per-entry path/kind/size/mode (metadata only)/sha256 or link target/inode,
  the declared required closure and an installer statement of the no-replacement
  promise. The Nucleus owner accepts it under its acceptance conditions. Any
  mismatch refuses and marks the epoch invalid; there is no auto re-accept. A
  new toolchain or Xcode update starts a new epoch.
- **Trust boundary.** Detection, not prevention. rustup and Xcode roots are
  user-writable, so no OS immutability is assumed. Residual risk: same-UID
  replacement between the last identity check and exec, the cooperative model
  already recorded here. The receipt is an accepted commitment, not a signature.
- **D, narrowed root.** Allowed only as a receipt over a named root with complete
  membership and a declared closure. The sysroot/rustlib and SDK closure must be
  included or justified. Partial closure with no declared root boundary is refused.
- **Symlinks.** Record the link with its exact target string. Refuse absolute,
  escaping, dangling and hard-linked entries, and special files. `cc -> clang`
  is one clang read, not two.

Proposed budgets, unruled and unmeasured:

| Item | Proposal | Note |
| --- | --- | --- |
| Class | `InstalledToolReceipt`, constructor-installed and versioned | Not an existing class |
| Per-file | 256 MiB, this class only | Covers cargo (30.39 MiB) and clang (118.90 MiB). The 16 MiB policy and `Limits::NUCLEUS` stay unchanged |
| Chunk / manifest | 1 MiB / 8 MiB | |
| Closure entries | 4096 per receipt | |
| Root membership | 32,768 entries | Existing association cap |
| Per phase | 1 GiB, after a measured inventory | |
| Admission reads | 520 MiB existing, plus a separate tool term | The existing 520 MiB already fills source inventory, map and collection. Owner ruling, applied at finalize too |
| Writes | 0 (verify in place) | A private copy needs its own write budget |

Cost: admission and finalize each re-hash the closure. cargo + clang alone is
156,544,648 bytes (149.28 MiB), so 298.56 MiB of reads per run before any SDK.
Hash throughput is unmeasured. The 041 statement that the rustup and Xcode roots
hold "far more than 32768 entries" was not enumerated in 041 or 042 and is still
unverified.

Owner commitments needed: the Rust toolchain owner supplies root identity,
`rustc -vV`, cargo/rustc entries, sysroot closure, epoch and promise. The Xcode
owner supplies the chosen clang/cc/ar/ld paths, the root membership, the SDK
subset actually used, and the epoch and promise. The Nucleus owner supplies the
class acceptance conditions and the tool-read admission and finalize term.

Refusal: overflow, unreadable entry, identity change, unknown epoch, unsupported
link or mismatch refuses before spawn. Unknown outcome retains bytes and holds
ownership. Ordinary fresh private validation remains the fallback.

### Finite delivery and cost

Sequence, each step a separate Queue brief unless noted:

1. **Oxide-only, owned fixtures, no host reads.** Unit-graph v1 decoder and
   provenance binding verifier.
2. **Oxide-only, owned fixtures.** `InstalledToolReceipt` schema, streaming
   verifier, symlink and membership rules, identity checks, constructor budgets.
   Synthetic files only.
3. **Nucleus owner, not Oxide.** Accept or decline the receipt class, the tool
   admission/finalize term, and epoch acceptance. Contract011 and NucleusContract002
   departures need the owner's later agreement.
4. **Material ruling plus a scoped host-read brief.** One bounded inventory of the
   declared roots. This is the first measurement that can settle fit.
5. **Material ruling.** Approve or refuse candidate A: env, flags, refusal set,
   nightly-branch inventory, and a changed trial cap if needed.
6. **Separate execution brief after 1-5.** One real graph plus receipt trial with
   fresh caps. Not authorized here.

Trial accounting: the graph call is one of the original 4 Cargo invocations and
its full wall time counts against the 45-minute clock, including probes, resolution
and any download. Probes are children of that invocation and must be reported
under it. Receipt hashing is inside the attempt and must be inside the trial
clock. A trial with a graph call leaves 3 Cargo invocations and the rest of the
time for build and test. No extra graph, probe or metadata call is allowed beyond
what the executing brief names.

Burden and benefit: route A is moderate: version refusal, an unstable env that
fails closed if removed, a nightly-branch re-audit on each Cargo upgrade, and a
decoder. Receipts are moderate to high: re-acceptance on every Rust or Xcode
update, and a full re-hash on each admission and finalize. The graph and
receipts serve the stricter cross-root and unit-binding claim. They do not supply
first storage relief. Same-root dependency reuse was shown by Task021 without a
graph. Recommendation: do not start the trial until the owner path exists and
Tom rules that the stricter claim is needed now.

Design success establishes no eligibility, reuse, storage relief or permission
to execute. Remaining gates: the real issuer, native/C and proc-macro closure,
debug/capture, and resources.

## Inspected workflow and identity

The bounded installed-root discovery for the exact candidate selector and
pinned source object is recorded in [Nucleus input-root acquisition](nucleus-input-roots.md).
It separates the authenticated Git object from the unaccepted Cargo, compiler,
native-tool and SDK candidates; it does not establish acquisition readiness or
an accepted production audit issuer.

Read-only inspection on 2026-10-10 used these clean working trees (no porcelain
status entries), without checkout, fetch, Cargo, selector execution or owner
contact:

| Repository | HEAD | Git tree |
| --- | --- | --- |
| Nucleus | `c0c9617533a675bad035b50aefe2fc6b6043f323` | `1940a897676a82cd0dd5a7557d916391ada07eba` |
| Queue | `7b9b840354a638a529ab023542903949b5b562ac` | `244eec3284d272e6db8b056264b31cd25b459f77` |

These are inspection snapshots, not execution-approved commits or frozen active
checkouts. Re-inspect and obtain exact-source approval in a later qualification
brief. Consumer policy context is Nucleus Contracts
[002](https://github.com/inflatable-cookie/nucleus/blob/c0c9617533a675bad035b50aefe2fc6b6043f323/docs/knowledge/contracts/002-core-requirements.md)
and [010](https://github.com/inflatable-cookie/nucleus/blob/c0c9617533a675bad035b50aefe2fc6b6043f323/docs/knowledge/contracts/010-host-run-scheduling.md),
Queue
[Spec 028](https://github.com/inflatable-cookie/paseo-northstar-queue/blob/7b9b840354a638a529ab023542903949b5b562ac/docs/knowledge/specs/028-fresh-sibling-checkouts.md)
and
[cleanup implementation](https://github.com/inflatable-cookie/paseo-northstar-queue/blob/7b9b840354a638a529ab023542903949b5b562ac/server/worktree-cleanup.ts).
This adds no parallel managed-Git or compiler-wrapper manager; `lead-1965`
remains an owner coordination lead, not a dispatched dependency here.

Select just the existing `check:core:test` workflow, with this exact ordinary
argv, run from the canonical Nucleus workspace root:

```text
["effigy", "check:core:test", "--", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

Inspected `effigy.toml` defines `cargo test --offline {args}`. Forwarding is
therefore exactly:

```text
["cargo", "test", "--offline", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

There is no `--locked`, test-name filter, extra feature argument or test-runner
argument in this ordinary vector. Preserve it. If Cargo updates the admitted
lockfile, source identity changes and finalize must invalidate rather than
publish reuse evidence. `planning.rs` contains 50 test functions and exercises planning,
lead/version/dispatch/dependency/admission/history/receipt behavior, uses temporary
SQLite stores and embedded portable-record/decision fixtures, and invokes
`mktemp` in several cases. `build_diagnostics.rs` contains one test and checks debug assertions,
overflow checks and a source-line backtrace. It does not prove complete DWARF,
type/variable inspection or old-run source lookup after recycling.

Ordinary Cargo test profile inherits `[profile.dev] debug = "line-tables-only"`;
other settings are Cargo defaults at this snapshot. Separately propose the same
vector with `"--profile", "full-debug"` appended after `"build_diagnostics"`.
The existing `full-debug` profile inherits dev with `debug = "full"`. This is a
separate partition and a qualification invocation, not replacement ordinary QA.
Capture whatever split-debug companions the resolved profile actually produces;
packed `.dSYM` cannot be inferred from Task 021's fixture profile. An additional
split-debug override needs explicit qualification identity/approval.

Pin the already-installed qualification candidate Cargo
`1.97.1 (c980f4866 2026-06-30)` / rustc
`1.97.1 (8bab26f4f 2026-07-14)`, native `aarch64-apple-darwin` host = target,
no `--target`, remapping, backend or wrapper. Nucleus declares no `rust-version`
or toolchain pin: this is neither an inferred MSRV nor a consumer manifest edit.
A future receipt must verify absolute executable identity, digest and full
version/host/sysroot provenance; this task did not invoke those executables.

Bind selector and forwarded argv as arrays, cwd/root ID, profile and effective
settings, default features plus resolved per-package feature sets, lockfile and
manifest identity, host/target, compiler/linker/SDK identities, effective Cargo
configuration and environment. Direct declarations include path
`nucleus-observer`, bundled `rusqlite = 0.40.2`, `schemars = 0.8.22`,
`serde = 1.0.229` with derive/rc, `serde_json = 1.0.151`,
`serde_yaml_ng = 0.10.0`, and dev `quote = 1.0.47` / `syn = 2.0.119`
with full. Lock entries include `libsqlite3-sys = 0.38.2`, `cc = 1.4.7`,
`proc-macro2 = 1.0.107`, `serde_derive = 1.0.229` (with `syn = 3.0.6`),
`schemars_derive = 0.8.22` and `serde_derive_internals = 0.29.1`.
These are declarations/locked candidates, not an observed Cargo unit graph.

A read-only traversal of lock dependency names from nucleus-core yields 60
candidate packages, including target-specific wasm candidates. It does not
resolve target cfgs, optional dependencies, build/dev feature unification or
which proc macros actually execute. Locally present candidate sources include
scripts in generic-array, libc, libsqlite3-sys, proc-macro2, quote, schemars,
serde, serde_core, serde_json and zmij; target-specific candidates include other
scripts/macros. Rust dep-info alone cannot audit their behavior. The observed
path-package census (core + observer) is 109 files / 3,075,303 bytes, largest
295,682 bytes; it includes tests/schema data and is not the compilation closure.

## Minimum eligibility extension

A safe useful slice is possible on the reviewed strict-physical mechanism,
conditional on a closed, independently audited workflow input contract. It is
**not currently eligible**. The separately versioned registered library route above supplies the
verification/runner seams; a real Nucleus registration and evidence owners remain
required. Retain the proposed workflow scope; preserve the fixed fixture route and its controls.
Do not substitute Nucleus constants into its nine-file/seven-unit allowlists.
Cargo remains graph/freshness owner; Oxide reads evidence and refuses unsafe
reuse, never computes or edits Cargo fingerprints.

| Class | Supported now | Minimum Nucleus extension | Excluded/unknown; evidence gate |
| --- | --- | --- | --- |
| Workflow/source identity | Frozen nine files, seven unit shapes, exact fixture selector/test profile, empty effective config | Exact argv arrays; approved path-package/identity/behavior-file inventory; fresh per-run content digest, submodule and dirty-state receipts; registered dependency identities | No observed Nucleus compiler graph. New target/member/package/input class requires new audited contract or refusal |
| Workspace/path Rust inputs | Version-1 encoded `CheckDepInfo`, unit references, conditional old/equal-mtime hashes; no external paths | Bind each canonical core/observer source path to all real consuming references; inventory manifests/lock/config presence and embedded test data | Sibling paths, symlink escapes, rustdoc, arbitrary path dependencies excluded. No general workspace coverage claim |
| Registry dependencies | Fixed local fixture only | Locked archive checksum plus verified extracted file inventory/content, stable canonical source roots; immutable package receipt reverified at acquire/finalize; register exact package/unit/features | Shared Cargo downloads are not immutability proof. Source replacement, patch/git source, registry mutation or unverified contents refuse; preserve dependency sources for old debug lookup |
| Third-party proc macros | One known fixture macro and declared generated read | Audit exact macro code and reachable helpers for file/env/tool reads; bind immutable macro source/binary and declared behavior inputs; include hidden reads conservatively | serde/schemars derives are present, but token-only behavior is not proved by searching for fs calls. Opaque/external/undeclared reads refuse unless explicitly audited and covered |
| Generated inputs | Three named fixture outputs in owned slot | Declare producer/OUT_DIR and consumers; hash relevant generated Rust/bindings/behavior inputs against their actual producer/consumer references; retain exact bytes in run evidence | Existing OUT_DIR may persist. No assumption of regeneration, complete dep-info or arbitrary native/generated output coverage; missing producer/read boundary invalidates map |
| Modern script rerun rules | One exact file-only output; directory inputs refuse | Parse pinned Cargo evidence for multiple `RerunIfChanged` and `RerunIfEnvChanged` locals; resolve package-relative files; finite recursive directory inventory including absent/present membership; associate each member with script output reference | Undeclared reads, unsupported formats, escapes and over-limit recursion refuse. Added/removed/missing members refuse conservatively; no inference from just a directory mtime |
| Scripts without rerun directives | `Precalculated` / old-style scripts excluded | Only audited immutable registry packages may use pinned `Precalculated` evidence plus unconditional whole-package integrity and tool/env binding | Inspected schemars and generic-array scripts have no rerun directives. Keeping the fixture exclusion would reject this candidate. Mutable path-package old-style scripts remain excluded |
| Native C/tools/headers | None | Audited bundled SQLite path only, immutable package/native source identity, resolved cc/ar/linker/SDK content/provenance and effective arguments/env; declared external read set or authoritative immutable input receipt | No C/header dependency observation. Rust dep-info does not cover C or system headers. Missing SDK integrity/immutability proof, custom wrapper, linked SQLite, pkg-config/vcpkg, bindgen, SQLCipher or cross compile refuses |
| Config/environment | Fixture allowlist, ancestor/user config refused, six fixed env keys | Resolve exact Cargo hierarchy/env/CLI precedence; capture complete non-secret effective identity and deliberate absence; restrictive launch environment; partition on relevant tool/config/env changes | No Nucleus effective user/ancestor config was resolved here. Overrides/replacement sources/new flags/wrappers/backend remain excluded until audited |
| Runner/capture | Disposable trial runner, whitespace-split selector, local fixture identity/producer | Explicit argv and digest; authenticated consumer envelope; supported foreground command through genuine scheduler; private target/temp/capture paths, real unit/test evidence and full-debug source mapping | Current `cargo.rs` is experimental, not production consumer authentication. Fixture hook/source/scanner constants cannot serve as Nucleus evidence |

### Audited third-party/native boundary

Read-only local registry inspection used the exact versions above. SQLite's
`build.rs` SHA-256 was
`269acf1864de5046ac5b77814926d41fe24108b0d027f635812a1db3c965f81f`.
It copies bundled bindings into `OUT_DIR/bindgen.rs`, declares SQLite C and WASI
VFS file reruns, and uses cc. Package files are `sqlite3/sqlite3.c`
9,507,037 bytes, `sqlite3.h` 690,725 bytes and bundled bindings 126,484 bytes.
Copying bindings and including headers are not fully described by its two
file-rerun lines. Whole immutable package coverage is required, alongside native
external inputs. The
[versioned script source](https://docs.rs/crate/libsqlite3-sys/0.38.2/source/build.rs)
and [cc source](https://docs.rs/crate/cc/1.4.7/source/src/lib.rs)
are primary references; local files have not been authenticated against the
locked archive checksums in this task.

Bind presence/value of `LIBSQLITE3_SYS_USE_PKG_CONFIG` (require absent or `0`),
`LIBSQLITE3_FLAGS`, `SQLITE_MAX_VARIABLE_NUMBER`, `SQLITE_MAX_EXPR_DEPTH` and
`SQLITE_MAX_COLUMN`; initial supported policy requires custom flags/limits
absent. Resolve all cc target-qualified/unqualified `CC`, `CFLAGS`, `AR`,
`ARFLAGS`, HOST/TARGET variants and precedence; initial policy rejects custom
flags/wrappers. Include `PATH`, `SDKROOT`, `DEVELOPER_DIR`, deployment target,
compiler builtin resource/header roots, SDK/system header/framework/linker
roots and actual compiler/archiver/linker identities. A version string or SDK
path alone is not content integrity. A consumer-owned immutable-tool/SDK receipt
must cover all reachable external files, with integrity verification and no
replacement during the lease, or reuse refuses. Which installed SDK and which
bounded integrity procedure can supply that receipt remains an owner/qualification
gate; do not hash an arbitrary whole host or silently accept opaque native reads.

serde/serde_core write versioned private modules and query rustc; proc-macro2
performs rustc compile probes under OUT_DIR and reads wrapper/bootstrap/flags
inputs. Bind Cargo-generated host/target/cfg/profile/package env values to the
resolved unit contract and external env presence explicitly. Require wrappers,
`RUSTC_BOOTSTRAP` and `RUSTC_STAGE` absent. Include Rust/linker flags, rustup
selection, CARGO_HOME/HOME/PATH, jobs, temp paths, backtrace/test env and the
selector launcher identity. Keep `CARGO_BUILD_JOBS=1` for the proposed first
proof. Fixed launch/capture paths are receipt fields; approved per-run target,
temp and evidence relocation must be distinguished from behavior-bearing compile
inputs so run IDs do not themselves generate partitions. Any path-sensitive
compiled use must be retained/mapped or cause refusal.

Cargo's [script change-detection contract](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection)
uses declared files/directories and environment; its
[configuration hierarchy](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure)
must be resolved rather than inferred empty from the checkout. These declarations
are not a sandbox or a complete read census. A cold discovery run also requires
the pre-audited boundary: discovery cannot retroactively authorize unknown reads.
Compare real artifact/unit and script evidence with the approved candidate
inventory at finalize; extra/unsupported units prevent publishing eligibility.
No network/fetch, undeclared generator, arbitrary script or macro extension is
part of the first slice. Missing audit evidence is a refusal, not coverage.

### Finite read/evidence budgets and guard publication

Retain current fixture limits unchanged: 4 MiB/file, 32 MiB/tree or collection
operation, 1 MiB/map, 4096 input/reference entries, 16 fingerprint directories,
eight entries/unit and 1024-byte paths. SQLite C already exceeds the file cap;
tracked-file counts cannot justify unit/reference or native/SDK coverage.

Propose independent Nucleus limits to qualify: 16 MiB/file, 256 MiB per source
inventory operation and 256 MiB per collection, 8 MiB serialized map, 32,768
input/reference entries, 256 units, 64 fingerprint-directory entries/unit,
2048-byte paths, recursive depth 32. Admission performs at most one source
inventory, one map read and one collection (520 MiB accepted reads); finalize
has the same independent ceiling. Deduplicate immutable file reads within an
operation without losing per-unit associations. These are proposed ceilings,
not measured closure sizes or proof that the candidate fits. 16 MiB covers the
inspected C file; the other increases are finite engineering proposals needing
real closure/counter evidence. External SDK/tool receipt preparation is separately
budgeted by its owner; it must not escape these invocation read bounds through
unaccounted scans. Overflow, inaccessible input, unstable read or absent proof
refuses, without auto-growth.

At every reuse check source/identity presence and every supported consuming
reference. For changed content with mtime <= any consuming anchor, refuse before
Cargo; newer changes rely only on Cargo's **audited** coverage. Unconditionally
verify immutable registry/tool/SDK identity; changes cannot be accepted merely
because their mtimes are newer. Directory membership/absence and untracked
behavior reads need explicit inventory. No unit reference is invented from a
project-wide timestamp. Maintain the durable pending barrier before spawn;
only successful compile, both test targets, complete capture and unchanged
admitted source/input identities publish the new map after authenticated Idle
finalization. Partial success, compile/test/capture failure, cancellation, lost
reply or mutation invalidates it. Reconciliation to Idle never resurrects the
old map. A proven pre-spawn refusal preserves the previous valid map.

## Consumer-owned physical source pool

Recommend one canonical physical task-root initially, with at most two compatible
partitions at that root (ordinary test and full-debug). This first pilot serializes
opted-in tasks for their whole root-allocation lifetime, including editing,
review and closeout, not just their builds. It is not a production concurrency
choice; the managed-Git/workspace owner must accept or change that tradeoff before
activation. Root count R=1 and
partition allowance P=2 bound retained intermediate generations to R×P=2,
independent of tasks and source revisions. Exact paths are assigned by the
consumer's managed-Git owner later; none is created/registered here. Recommend
an enforced 8 GiB/generation and 16 GiB total intermediate ceiling as proposed
first limits, subject to qualification fit. These are not Task 021 budgets or
operator-approved allocations. Source, private target/temp, independent evidence
and any fallback need additional finite count/byte/retention bounds, all recorded
before execution. Reject a third toolchain/config partition when both allowances
are occupied; no implicit eviction or growth. Root sets contain canonical physical
IDs and ownership policy, no logical remap or cross-root hit promise.

Every task still has its own logical workspace and branch. A root allocation
receipt binds consumer/project, task/workspace/branch, task run/attempt, root ID,
allocation epoch, source manifest and policy digest. A new task at the same path
has a new allocation epoch and distinct identities. Physical path equality is
not task identity. Authenticate the receipt through the consumer, not fixture
credentials. Replay old allocation/lease receipts against a new epoch refuses.

Source preparation and archive are serialized operations in the existing
managed-Git/worktree owner. Contract 002 already authorizes qualification of
linked worktrees from an adapter-owned repository store, separate from the
operator checkout, with private per-checkout metadata. Use that owner and
retain task-specific bundles/candidate object pins without capturing other
tasks' unpublished refs; it does not yet provide this path-pool policy. Only one task owns mutable source at a root. No
second compilation-source copy, Oxide Git operations or task-root reset is
introduced. Preserve the root allocation through editing, workers/replacements,
exact-head review and closeout. An Oxide lease spans one compile→tests→capture;
release it between invocations without releasing the workspace's root.

Before source removal/reallocation, ordinary archive must prove clean/published
head, merge/closeout disposition, released writers/reviewer, all authenticated
execution settled, cleanup ownership and independent evidence retention. Failed,
cancelled/unmerged, dirty/unpublished, retained, leased or uncertain work occupies
the root. Missing artifact or source retention leaves archive pending. Adopted
work requires explicit cleanup-ownership transfer. PID disappearance, age,
pressure or a reply timeout never frees a root. A root cannot be recycled while
an Oxide writer remains uncertain, even if the consumer UI says completed.

Before recycling, retain outside mutable source and Oxide generations an exact
source manifest: origin/commit/tree, submodule identities and content availability,
tracked modifications, deletions/modes, untracked/ignored behavior files and their
bytes where permitted, dependency source checksums/content, relevant generated
inputs, effective config/env/tool/SDK identity, source digest and retention-policy
receipt. Ordinary committed source needs pinned durable Git objects, including
submodule objects, not just a remote SHA that might later disappear. Dirty inputs
need a content-addressed snapshot/delta with base and reconstruction verification,
or are ineligible and held. Initial proof should use clean approved committed
revisions; that does not remove the ordinary-use dirty-source owner gate.

Independent per-run capture retains logs, always-executed test results, exact
binaries and complete debug companions/UUID or build-ID and digest bindings,
source-path→retained-source mappings, private artifacts, QA and authenticated
host settlement, source/partition/lease/generation/epoch receipts. The archive
receipt states that source and artifacts are retrievable after R is occupied
by the next task. Old full-debug source lookup must resolve old bytes, not the
new contents at the same physical path. Snapshot evidence is an immutable
archive, not another mutable compilation checkout. Retention lasts through the
consumer's agreed review/debug/history period; its finite quotas and eventual
evidence deletion policy need owner approval. Never use current root content as
old-run evidence or silently delete evidence for cache capacity.

Pool exhaustion waits in authoritative consumer pending without another checkout,
Oxide lease or host execution slot. Propose fallback disabled for the first slice
(fallback count/bytes = 0). A later owner-approved private fallback needs a
separate count/byte budget and ordinary retention; no hit is claimed. Keep root
allocation waits separate from build waits for a task already occupying R.

### Required owner decisions and semantic receipts

These field groups describe proposals, not existing settings or wire APIs:

| Owner | Proposed fields/receipt and approval subject |
| --- | --- |
| Nucleus managed-Git/workspace + Contract 002 owner / Tom | Opt-in workflow ID/version, canonical root pool and allocation epoch, R/P/count/byte caps; explicit acceptance or revision of R=1 task serialization; distinct workspace/branch at each reuse; source-preparation/archive/recycle intent and outcome; exact-source retention manifest and reconstruction/debug lookup receipt |
| Consumer task/admission owner | Pending request ID; attempt authorization bound to task/workspace/branch/run/attempt/root epoch/source/partition; idempotent authorize/acquire/return-to-pending/cancel/reconcile operations and terminal no-spawn receipt |
| Host-run owner under unchanged 010 | Genuine submit request ID/run ID/epoch/token and settlement envelope; adapter command/capture binding; confirmation that acquisition-before-submit works with real scheduler authority and nested Effigy execution; explicit acceptance or rejection of holding an Oxide lease while a submitted host run waits for capacity, including cancellation/deadline and reconciliation bounds |
| Queue Spec 028 runtime maintainers / Tom | Optional shared intermediate directory outside task checkout, bounded retention and cleanup handoff; task-private target/final outputs unchanged. Existing outside-root cleanup refusal is preserved; never teach task cleanup to delete an Oxide slot |
| Consumer evidence/retention owner | Private artifact/source/generated/dependency/debug capture, independently retained digest manifests, retention quotas/duration and archive acknowledgment; failure preserves occupancy |
| Oxide registration/storage owner | Audited input-boundary version, compatibility digest, lease/generation and pending-map state; capacities, refusal evidence and optional separately granted whole-generation recovery |

Contract 002's initial task-specific workspace model does not authorize a pooled
physical path allocation. The narrow proposed departure reuses a **path only
after archive**, never logical workspace/branch/history or mutable source across
live tasks. Its failed-work retention and shared exact-head review rules remain.
Spec 028 currently keeps Cargo build outputs local, shares downloads/Git objects,
and excludes shared build outputs; retaining Oxide intermediates outside the
checkout needs its owner/Tom agreement. Object sharing alone grants neither
root recycling nor compiler reuse. Current managed-Git/retention owners must
implement these seams; this task does not amend their contracts or contact them.
Consumer integration requires its own scoped operator-approved brief and owner
agreement under [vision](../vision.md#authority-boundaries).

## Lifecycle with frozen Contract 010

| Transition | Authority and retained evidence |
| --- | --- |
| Pending/readiness/wait | Consumer owns dependency/priority/wait/cancel state. Oxide readiness is advisory and non-reserving; no agent/host slot/cache lease is created by waiting |
| Authorize task attempt and prepare source | Consumer assigns root exclusively, records exact source and authenticates attempt envelope. Root allocation and Oxide lease are separate lifetimes |
| Exclusive acquire | Durable idempotent operation binds admitted attempt/root epoch/source/partition. Busy returns to consumer pending before host submission; no fake token, host run or settlement |
| Submit actual host run | After lease receipt is durably retained, submit exact argv/cwd/env through the current authority under 010; persist clientRequestId→hostRunId/epoch. Scheduler alone supplies HOST_RUN_TOKEN. Queued host capacity may hold the Oxide lease; cancellation/deadline bounds this queue wait |
| Compile/test/capture | Genuine admitted foreground run uses exclusive intermediate generation and private target/temp. Consumer capture completes while the lease remains exclusive; Cargo freshness never skips requested tests |
| Settle/finalize | Authenticated scheduler envelope + supported cooperative writer completion + independent capture/source proof bind the exact lease/generation/run. Healthy successful finalize may publish map and return Idle; other spawned outcomes invalidate, and unknown completion stays Uncertain |

The inspected 010 lifecycle is `queued→admitted→running→settled`; its nonlaunched
settlements apply only to actual submitted runs cancelled or capacity-timed-out.
Pre-acquire readiness and pool waiting reserve no Oxide lease or host-run slot.
The proposed post-acquire host-capacity wait is a separate, explicitly gated
owner decision in the table above; it is not permission to hold leases while
waiting indefinitely, and no timeout by itself proves release.
Storage busy is **not** a host outcome. Current consumers do not expose the
required authenticated task-attempt→Oxide acquire→return-to-pending transition.
Their task/lifecycle owner must approve that narrow durable representation outside
010. Inspection of `crates/nucleus-core/src/ingress/planning.rs` found
`PendingDispatch` (pre-dispatch, without workflow run/reserved operation/resource)
and prerequisite release; `hosts/paseo/src/admission.ts` is worker admission,
while `shadow-scheduler.ts` checks scheduler conformance. These existing
capacity/planning/shadow surfaces are not an authenticated storage-attempt
credential or its return transition. Extend the owning admission/lifecycle
surface rather than overloading prerequisite release or introducing another
manager. Nucleus currently shadows Queue's host authority; do not assume Nucleus
already implements a scheduler or switch authority to make the adapter work.

Cancellation while pending removes the wait. After authorization but before
acquire, cancel/park the attempt without a host token. After successful acquire
but before submission, token-bound proof that no runner/submission could start
may settle the no-spawn lease; an uncertain submit requires reconciliation first.
Persist acquire and submit intents before delivery. Lost acquire reply replays
identical acquire ID/payload; lost submit reply uses 010 status by
`(caller, clientRequestId)` and follows its unknown_run rule, never a blind new
submission. A crash between acquire and submit holds ownership until the durable
no-spawn/submission evidence is reconciled. There is no release from timeout.

The registered test/runtime boundary must also satisfy the cooperative writer
model: tests/scripts wait for writing children and close handles; mktemp/SQLite
outputs stay in the bounded private temp footprint. External servers, detached
writers and same-user bypasses are unsupported. Source/config/tool/dependency
mutation during execution fails the unchanged-input finalize check. No process
census or arbitrary host fence is claimed.

After submit, cancellation is sent to the real scheduler; authentic nonlaunched
cancel/capacity_timeout may reconcile a no-spawn lease only with adapter proof
that no Cargo writer began. Launched failures/cancel/lost response need supported
completion/capture proofs; group absence or an expired token alone is insufficient.
Unknown escaped writers keep the generation and source root unavailable. A retry
gets a new attempt and operation IDs only after the previous attempt is authoritatively
settled; it cannot inherit its lease or resurrect invalid guard evidence.

Optional cold recovery is **disabled** initially. Its minimum later grant names
a proven-Idle generation, owned canonical deletion scope, authenticated consumer
policy/operation ID, byte/count reservation and source/evidence exclusions.
Serialize admission/eviction and persist recovery intent; delete only that whole
generation, verify completion and empty successor before retiring/reassigning
capacity, then cold compile/test/capture with full input checks and unchanged
source. Interrupted deletion retains reservation/unavailability. Leased,
Uncertain, corrupt/unknown owner or unproved emptiness refuses. Ordinary acquire
never deletes an invalid generation. Successful live recovery/rebind needs its
own bounded proof; Task 021 did not execute it.

## Eligibility and qualification gates

### Consumer-owner assessment

The existing Queue and Nucleus planners assessed this contract on 2026-10-10
under the bounded coordination authority in
[vision](../vision.md#authority-boundaries). Both recommend the narrow
Nucleus-first pilot in principle, with Queue retaining host-run authority.
Their recommendations grant no implementation, activation, storage allocation
or policy-departure approval.

Both recommend acquire before genuine host submission, with an explicit finite
host-capacity wait while holding the acquired Oxide lease. Readiness and storage
busy remain non-reserving. A submitted run's authenticated nonlaunched
cancel/capacity-timeout settlement needs adapter no-writer proof before release;
deadline expiry alone never releases ownership. The consumer-owned
storage-attempt ledger must persist authorization, acquire/submit intents and
identities, busy/uncertain outcomes, cancellation and reconciliation. Existing
operation journals and worker admission are patterns, not this API.

Nucleus's [consumer boundary, Contract 011](https://github.com/inflatable-cookie/nucleus/blob/9a562d1492ddb1474ebb2ea503889ecc6b0845b1/docs/knowledge/contracts/011-oxide-consumer-boundary.md)
defines proposed version-1 consumer-local receipts for allocation, storage
authorization, acquire, submission, capture, archive/recycle and no-spawn closure.
They bind the task/run/root allocation epoch, requested source tree, workflow and
partition, with operation/body identity and a verified issuer boundary. These are
consumer-owned fields, not shipped Oxide RPC names. Fixture verifiers establish
adapter behavior only; unconfigured production issuers refuse. Live binding
requires an independently reviewed mapping to the actual Oxide API. Per-run head
and tree binding must not make every source revision a new compatibility partition.
Contract 011 describes the consumer-local mapping to the Rust library above;
its reviewed dep-info mapping requires `LocalInput::DepInfo.unit` to match
`UnitEvidence.id`. The unit is included in the semantic publication-map framing,
while the version constants remain 1. A future binding must pin the reviewed
implementation and refuse unqualified compatibility with maps published before
that framing change. The existing TypeScript seams neither encode dep-info nor
consume Oxide publication maps, so this change introduces no immediate TypeScript
code incompatibility.
The TypeScript consumer now supplies the disabled fixture transport described
under [inactive Nucleus client](#inactive-nucleus-client). Production embedding,
accepted owners and genuine input/completion evidence remain prerequisites.

Nucleus now has disabled consumer-local managed-Git/root-allocation,
storage-attempt and bounded retention seams. Managed Git supplies task-scoped
object pins and bundles. The retention fixture implements independently owned
capture, archive/recycle acknowledgments and retained-byte lookup through its
existing owners. Its local retained-source manifest digest differs from Oxide's
framed source digest. Debug identities use `fixture-v1`; this does not establish
real native debug-companion binding or old-run debugger lookup. Production
placement remains fresh private clones, and production retention registration
refuses. Accepted production issuers, authenticated Rust embedding and genuine
execution/source/debug evidence remain prerequisites for a qualified physical
root pool. Oxide must not become the consumer's Git or retention manager.

The retention code received independent static exact-head review; missing
dependencies prevented the reviewer from executing tests/typecheck. Covering
checks on merged Nucleus main `9a562d1` subsequently passed: 234 tests, one
existing Effigy-pin-dependent skip, typecheck and docs. Those are merged-head
reconciliation results, not pre-merge executed reviewer evidence or live reuse
qualification.

Nucleus recommends starting the pilot with clean committed source only;
unsupported submodules/LFS refuse, and dirty/unmerged/failed/uncertain tasks
retain root occupancy. Both owners treat R=1/P=2 as a conditional pilot proposal
with whole-task serialization, not a production concurrency choice. More
parallel tasks require an explicitly bounded R>1, one live task per root, and
no cross-root hit assumption.

Neither consumer has a general immutable registry/tool/SDK receipt issuer.
They recommend that the registered-workflow qualification owner supply the
audited input manifest and accepted issuer policy, with the installation owner
supplying bounded provenance/content integrity and a cooperative no-replacement
agreement for declared reachable inputs during the lease. Oxide verifies that
contract at acquire/finalize. Missing native closure stays ineligible; this does
not authorize broad host hashing or turn paths/versions into integrity proof.

Both recommend private fallback and automatic recovery remain disabled for the
first pilot. Intermediate limits remain Oxide-owned; source/private-output/temp/
evidence budgets and retention remain consumer-owned. Proposed 8/16 GiB limits,
aggregate capture-copy peaks, retention periods and queue deadlines still need
their explicit scoped decisions before real execution. The owner responses and
proposed delivery dependencies are recorded in Queue's coordination lead;
they are not new shipped consumer capabilities.

The smallest Oxide boundary is the versioned registered Nucleus collector/argv
runner extension above, retaining strict physical partitions, the stale-content
refusal and durable failure barrier. The consumer boundary is a narrow opt-in
through existing managed-Git/admission/capture/retention owners with the proposed
receipts and pending transition. Neither boundary requires a new fingerprint
engine or Contract 010 change. If owners cannot supply those receipts or the
native/opaque input audit, the workflow remains private/ineligible; stop rather
than substitute a new mechanism.

Before real execution, independently review the audited target/feature/unit and
script/macro/native input contract, exact source/tool/SDK/config identity,
finite resource/time/invocation limits, consumer path/archive/evidence approval
and authentic lifecycle mapping. Read-only source inspection cannot supply
runtime graph, actual C header closure, complete macro behavior, successful
capture or owner consent. This contract leaves those evidence gates open.

A later representative proof must use two distinct approved tasks/workspaces/
branches/runs and committed source revisions at the **same recycled physical
root** after ordinary archive. Show current changed-source behavior, affected-unit
rebuild and retained third-party dependency units, both tests executing every
run, independent full-debug source/artifact lookup for the old run after the
new source occupies R, stale-input and concurrent refusal before Cargo, and
failed-run invalidation/post-restore refusal. Separate config/toolchain changes
must miss/refuse within the finite partition budget; successful recovery is not
implied. Exercise busy/cancellation/lost-response reconciliation without fictitious
host settlement. Use actual unit/artifact evidence, not elapsed time as a hit.

Measure allocated/logical intermediate, source, private output/temp and evidence
bytes, retained generation counts and peak bounded footprint, plus actual cold /
changed-task latency, wait time and operator actions. Require useful recurring
storage/latency results for an ordinary-use decision, not another tiny fixture
replay or a broad baseline programme. SSD writes remain a separate gate needing
an attributable real write observable and separately approved measurement scope;
allocation deltas/global free space do not measure writes. Qualification and
consumer delivery scopes belong in Queue, not this knowledge contract.

## Proposed minimal Rust binding (inactive design)

Status: proposal from read-only source inspection on 2026-10-11 (Task 026). It is
not a production RPC, accepted production issuer, live binding or real-workflow
evidence. The private process/authentication seam is now supplied only by the
[qualification surface](#private-authenticated-qualification-process-oxide029);
consumer and production owner gates remain. Inspection pins: Oxide main
`3c957a9`, Rust registered library at PR25 `57ee0cf` / merged `d9ff179`; Nucleus
`9a562d1` ([Contract 011](https://github.com/inflatable-cookie/nucleus/blob/9a562d1492ddb1474ebb2ea503889ecc6b0845b1/docs/knowledge/contracts/011-oxide-consumer-boundary.md),
`root-allocation.ts`, `managed-git.ts`, `storage-attempt.ts`, `oxide-retention.ts`,
`validation-client.ts`). No build, test, scan or owner contact was made.

### One attempt, traced through actual code

| Step | Actual surface | Owner today |
| --- | --- | --- |
| 1. Root and source | `ManagedGit` owned worktree and head pin; `RootAllocation` durable allocation via injected `RootVerifier` | Nucleus |
| 2. Authorize, acquire intent | `StorageAuthority.authorize`, `StorageTransport.acquire` (TS semantic seam); ledger in `storage-attempt.ts` | Nucleus, seam pending |
| 3. Acquire | Rust `storage::Client::acquire(AcquireRequest)` returns `AcquireResult { lease_id, slot_id, storage_instance_id, generation, state, slot_path, token, reused }` or a `StorageError` (classified below) | Oxide daemon, Rust-only |
| 4. Host submission | `validation-client.ts` `runValidation` (reviewed, unchanged), then scheduler under 010 | Queue host authority |
| 5. Registered admission and execution | `Runner::run(&Registration, &Attempt)`: `inputs` (authenticate, audit, source inventory), reuse check, pending barrier, `ExecutionOwner::execute` | Oxide library; no installed owners |
| 6. Capture and finalize | `verify_success`, `seal_map`, `finalize_idle` or `finalize_not_spawned`; daemon `Client::finalize(lease_id, token, operation_id, SettlementEvidence)` | Oxide plus Nucleus capture owner |
| 7. Archive and recycle | `OxideRetention.capture/archive/recycle`; `RootAllocation` seal. No Oxide call | Nucleus |

Facts that drive the shape:

- The daemon is reachable only through Rust `storage::Client` (binary codec over a
  Unix socket). TS has no codec and cannot construct daemon frames.
- Acquire outcomes, from `store.rs` `acquire` and `acquire_result`:
  - `Ok` with `SlotState::Leased` and a `slot_path`: acquired. Attempt receives it.
  - `Ok` with `SlotState::Uncertain` and no `slot_path`: uncertain hold. The lease,
    reservation and root stay occupied. No host run.
  - `Ok` with `SlotState::Idle` (finalized lease replay): no access. Not busy; the
    parent reconciles before any new attempt.
  - `Refused(LeaseUnavailable)`: occupancy wait. The slot or lease is held. Storage
    busy, pending, no host run.
  - `Refused(BudgetExceeded | FreeSpaceReserve | FreeSpaceUnavailable)`: capacity
    wait. Pending and non-reserving. The parent decides wait or refuse by policy.
  - `Refused(UnknownAccounting)`: uncertain accounting. Hold, no automatic retry.
  - `Unauthorized`: admission verifier refused. No lease, no spawn.
  - `Conflict`: same `acquire_id` with a changed body. Refuse.
  - `AlreadyRunning` comes only from `Daemon::bind` and stale-socket checks, never
    from acquire. `NoIdleStorage` comes only from `evict_slot`; neither is an
    acquire-busy outcome. Same-root fixture acquire never evicts. Other acquire
    paths may evict eligible idle slots through `make_room` or
    `require_free_space` to satisfy the existing budget and reserve policy.
- `NoSpawnEvidence` and `CooperativeCompletionEvidence` are signed with the
  daemon-issued `LeaseToken` through `pub(crate)` methods. Settlement can therefore
  only be built by the Rust side that holds the token from acquire.
- `Runner` has no daemon RPC. `Attempt` is caller data with no authority, and its
  `reused` flag must come from the daemon, not from Nucleus.
- `AcquireResult.generation` and `storage_instance_id` identify the durable slot
  incarnation. It still has no allocation epoch. `RunEnvelope` has no epoch field;
  the admission verifier must bind that consumer-owned identity.
- `EvidenceAuthority::verify_admission` and `verify_settlement` return `bool`. An
  implementation that does not authenticate through the accepted consumer verifier
  must refuse. A boolean is not authority.

### Chosen embedding

Use a Rust **registered runner host** (proposed module `src/registered/runner_host.rs`),
spawned per attempt by the Nucleus adapter and driven over a private framed channel
(newline-delimited JSON, versioned, explicit frame cap, unknown op or version refuses):

- The host owns every `storage::Client` call for acquire, finalize and reconcile.
  It holds `LeaseToken` in memory for the attempt only. Nucleus stores only token digests.
- `RegistrationOwner`, `InputEvidenceOwner` and `ExecutionOwner` are implemented as
  parent callbacks over the same channel. Each returns an owner receipt that the Rust
  side verifies against subject and policy digests before use. Verifier material is
  installed at embedder construction, never from task JSON or environment.
- `ExecutionOwner::execute` is the nested host submission. The parent persists its
  submission intent, calls reviewed `runValidation`, then returns `Completed` or
  `NotSpawned`. `Runner::run` itself does not change.
- Ordering: parent persists acquire intent → host `acquire` → `LeaseUnavailable` or a
  capacity refusal returns `storage-pending` to the parent without calling `Runner::run`
  (uncertain or unknown-accounting results hold instead) → acquired lease
  retained before host submission. A submitted run may hold the lease for the finite
  capacity wait that fits its queue deadline. Cancellation sends the fence first,
  then the host requests reconcile or finalize only with authentic proof.

Alternative, rejected: TS-only storage client plus direct Effigy invocation without
`Runner`. That needs a TypeScript reimplementation of the binary daemon codec and of
the map, input and seal checks, which duplicates the Oxide manager.

The framed library host and private qualification process below supply this
Oxide-owned boundary. It stays inactive until a trusted embedder launches it.

### Inactive framed host (oxide#028)

The callable library entry points are
`registered::runner_host::RunnerHost::new` and
`registered::runner_host::RunnerHost::serve`. Construction installs a Rust
storage::Client and an embedder-provided OwnerReceiptVerifier; it does not
spawn a process, register a workflow, install verifier material, or enable a
default host. The embedder supplies one private Unix stream per attempt. No
default executable or distribution entry point is shipped. The opt-in private
qualification process is specified [below](#private-authenticated-qualification-process-oxide029).

The private protocol is version-1 newline-delimited JSON. It refuses unknown
operations or versions, unknown top-level fields, malformed/truncated frames,
and callback replies that do not match the outstanding callback ID, owner,
subject, policy, and result digest. Its fixed bounds are 8 MiB per frame,
64 MiB total input/output, 256 owner callbacks, 30 seconds per stream read or
write, and five minutes per session. JSON u64 values use safe integers through
2^53-1 and canonical decimal strings above that. Callback subject and result
digests use compact JSON with object keys sorted by UTF-8 byte order; they are
separate from source, Git, and consumer-retention digests.

The callable start schema is a single `run` frame with `version`, request `id`,
and a payload containing `registration`, `attempt`, `acquire`, and `finalize_id`.
`acquire` is the serialized Rust `AcquireRequest`: its envelope retains the exact
`RunBinding`, complete partition, admission reference and credential bytes, plus
`acquire_id` and `required_bytes`. The credential-bearing request is supplied at
the private embedder boundary and stays in this host attempt's memory for lookup
and any permitted retry. It is not copied to ordinary consumer or storage
ledgers, logs, or error frames, and the host has no credential issuer or remint
path. If that original credential payload is unavailable, recovery holds rather
than manufacturing a replacement. A host restart does not preserve that secret
in this library; an embedder without its original accepted secret must keep the
attempt held.

The callback request schema is `callback` with a monotone string `id`, owner,
`subject_digest`, `policy_digest`, complete `storage` identity
(`slot_id`, `storage_instance_id`, `generation`, and `reused`), full partition,
and owner payload. The response must echo version, operation, callback ID, owner,
subject and policy digests, then supply the result digest, receipt, and result.
The exact subject digest covers the whole storage tuple and callback payload;
receipt verification is against that digest and the installed policy. A legacy
consumer `Lease{id,generation}` or a tuple with a different instance or slot
cannot be interpreted as this binding and is refused. In particular, generation
alone never identifies a storage instance.

The delivered callback names are `registration.authenticate`,
`registration.audit`, `input.immutable`, `input.units` (in-memory fixture mode)
or `input.graph_owner` (collector mode; never both), `input.seal_map`,
`input.verify_map`, `execution.execute`, `execution.verify_success`, and one of
`execution.finalize_idle` or `execution.finalize_not_spawned` when the existing
Runner path reaches those operations. Replies are authenticated before their
values reach Runner. The exact path-dependent order remains `Runner::run`'s
existing order; this bridge does not change its map, input, or ownership rules.

The storage ordering maps onto the existing `StorageAttempt` responsibility as
follows, without changing its TypeScript interface: the consumer records acquire
intent and passes the accepted original request to the host; Rust calls
`Client::acquire`. Busy/capacity replies return pending before Runner, while
unknown accounting holds. After an ambiguous acquire error Rust calls
`Client::lookup_acquire` with the exact credential-bearing request. `Present`
and `Uncertain` return a held observation. The wire `StorageIdentity` contains
slot, storage-instance, generation and optional reuse information; it omits
lease ID and state, so the consumer cannot distinguish Leased, Idle or Uncertain
from that frame. The internal `LeaseIdentity` has no path or token and never
authorizes Runner execution. Only `Absent` permits an exact
replay of the original request. After Runner's authenticated parent callbacks,
Rust finalizes with its in-memory token and exact settlement evidence. An
ambiguous finalize uses `Client::lookup_finalize` with that same lease ID,
finalize ID, and evidence before any identical finalize replay. `Present Idle`
is an observation of completion, `Absent` permits the exact retry, and
uncertainty holds. The host never uses `Client::reconcile` as lookup.

The Oxide host alone does not upgrade Nucleus's old lease or publication-map
formats. The [inactive Nucleus client](#inactive-nucleus-client) binds the complete
storage tuple and preserves original secret material only within its private
session. Session or secret loss holds; it provides no durable secret recovery
or production adapter.

Each callback request binds the exact payload to the acquired slot ID,
storage-instance ID, generation, reuse result, and complete partition key. Its
policy digest binds the full registration digest, graph/config/toolchain/debug
identities, read limits, callback owner, and reviewed registered-map
implementation pin d9ff17988c7eaa5cf57747bc55bfe4f138692f15. An installed
verifier must authenticate the returned owner receipt over those digests and
the result digest. A missing or false verifier result refuses the callback.
Old map receipts without that pin do not verify. The host asks Runner::run
only after acquire returns a leased result with a canonical generation, slot
path, and token. Busy or capacity results return pending before Runner;
uncertain or unknown accounting remains held. The host holds the lease token
only in attempt memory and emits its SHA-256 digest, never token material.
Ambiguous acquire and finalize errors use their exact lookup-only APIs before
any identical replay; lookup results are never treated as settlement or as
writer access. A lost token or uncertain result remains held; timeout, PID, or
label evidence never releases it, and this host does not perform reconciliation.

The test parent uses synthetic receipts and the actual oxide#027 storage
generation and lookup APIs. It proves the private callback and ownership
boundary only. No production issuer, real graph/registry/native closure,
representative test/debug capture, workflow eligibility, or everyday storage
savings claim is established. A real embedding still needs owner-selected
issuers and verifier keys, complete input auditing, consumer capture and
settlement evidence, and its own transport/process policy.

### Private authenticated qualification process (oxide#029)

Feature `private-qualification` adds the unpublished Unix executable
`oxide-qualified-runner`; `publish = false` remains in force. Build/check with
`effigy rust:private-compile`. The covering `effigy rust:private-process` builds
the actual binary offline with the committed lockfile, resolves Cargo's emitted
executable artifact path, checks independent Node authentication vectors, and
runs only its named private fixtures. Its fixture build has a 300-second
supervisor limit and test runner a 300-second process-group forced-exit limit; each of the
three actual child sessions has a 30-second deadline and uses construction-bound
small synthetic data below 16 MiB. Repository compilation is separate from
those sessions. Standard compile, format, Rust suite and docs selectors still
cover the default route. Live Cargo probes remain ignored.

The exact callable surface is `<compiled-artifact> <canonical-bootstrap-file>`:
one non-secret pathname argument; all attempt traffic uses the Unix stream,
with no additional arguments, environment discovery, installation or service. A trusted qualification
embedder creates a private mode-0700 bootstrap directory, the four canonical
owned attempt directories, an owner-only mode-0600 parent Unix listener and a
real `Daemon::bind` under a separate canonical mode-0700 storage root. The
embedder installs `QualificationAuthority` as the daemon's `EvidenceAuthority`
and its explicitly selected `FreeSpaceProbe`. Before serving, it calls
`Daemon::storage_instance_id()` and `QualificationAuthority::install_instance`.
The authority accepts at most four exact approved `RunEnvelope` subjects;
changing the installed instance refuses. This non-secret getter does not mint
admission, a lease, a signer or a new protocol operation. A trusted production
issuer, persistent secret store and host free-space probe are not supplied.

`registered::qualification::Bootstrap` is the version-1 JSON schema; unknown
fields refuse. Its required fields are:

| Field | Meaning |
| --- | --- |
| `version`, `host_protocol`, `storage_protocol` | All exactly `1`; storage stays `OXSTOR01`, ledger version 3. |
| `map_pin` | Exactly `d9ff17988c7eaa5cf57747bc55bfe4f138692f15`. |
| `expires_unix_seconds` | Current UTC seconds through at most 30 seconds ahead; checked at load and before acquire. |
| `scope` | Owner-approved qualification scope, bounded to 4096 UTF-8 bytes. |
| `storage_instance_id` | The actual daemon's 64 lowercase hex identity. |
| `start_digest` | Canonical SHA-256 digest of the complete approved version-1 `run` frame, including binding, registration, partition, policies, original credentials and operation IDs. |
| `callback_key_hex` | Owner-created cryptographically random 256-bit short-lived key, exactly 64 lowercase hex characters; private file only. |
| `parent`, `storage` | Each `{path, device, inode}`, captured after binding via `Endpoint::capture`; canonical socket path, original exact unsigned-integer device/inode and mode 0600. A non-Rust bootstrap producer must preserve all integer digits without JavaScript Number rounding. |
| `roots` | Four approved canonical owned directories containing `source_root`, `target`, `temp`, `capture`; group/other write permissions refuse. |

The bootstrap must be a canonical, same-effective-user regular file, mode
0600, one hard link, 1–65536 bytes. Open uses no-follow/nonblocking flags; the
named and opened device/inode, size, mode, UID, link count and nanosecond
mtime/ctime must agree before/after the bounded read. Endpoints and private
parents are rechecked before effects; replaced, missing, unsafe or unapproved
paths refuse. Supported registration/source/dependency/configuration versions
are checked before acquire. Valid bootstrap is unlinked before connecting; raw
read bytes are cleared and keys live only for this process. Rejected bootstrap
is retained for its trusted owner to dispose of. Protected material never goes
in command-line values, ambient environment, errors, logs or ordinary ledgers.
The generic process error discloses no subject or secret. This supplies no
secure memory erasure or fencing against a noncooperating same-user process.

The child connects to that pinned parent listener and calls the actual
`RunnerHost::serve` once. The parent sends the exact pinned `run` frame and
answers the existing callbacks. Rust alone connects to the pinned storage
endpoint, acquires/finalizes with the real client and retains the lease token.
The acquired instance and canonical `root/slots/<slot_id>` path must match the
bootstrap. The child issues no command, Cargo spawn, host submission, consumer
lookup/cancel operation, reconciliation or invalid-generation deletion.
Existing host limits, callback/result version 1, OXSTOR01 version 1 and the
registered-map pin remain unchanged.

Before acquire, the child durably creates an owner-only
`attempt-<start_digest without sha256 prefix>` marker beside the parent endpoint,
using exclusive create and file/directory sync. Existing, partial or uncertain
markers refuse even when the bootstrap has been recreated. They contain only
`oxide-qualified-attempt-v1` and no credentials. The owner must retain the
marker directory throughout qualification and recovery; an ended/interrupted
attempt cannot be launched afresh as lookup. Exact storage lookup/retry within
the original host session remains unchanged. A changed original request
refuses the pinned digest or daemon's exact-body identity checks. Markers are
not removed automatically; owner disposal is outside an unknown lease's
recovery. Channel loss leaves durable ownership held, and daemon restart makes
an unsettled lease Uncertain. Missing original secrets, changed key/instance,
reply loss, PID disappearance or elapsed time never release ownership.

Authentication uses pinned RustCrypto `hmac` 0.12.1 with SHA-256 0.10.9,
constant-time `verify_slice`, and 32-byte owner-supplied keys. Each receipt is
`hmac-sha256-v1:<64 lowercase hex tag>`. The MAC input is a sequence of fields:
first the domain, then the listed fields; each is UTF-8 prefixed by its **u32
big-endian byte length**, with no delimiter or terminator. At most 16 subject
fields and 1 MiB framed input are accepted. Canonical JSON and safe-u64/decimal
text rules are those of the existing host, not Git or retention identity.

| Domain | Fields after the domain |
| --- | --- |
| `oxide-owner-callback-v1` | callback context, owner, callback ID, exact subject digest, policy digest, result digest |
| `oxide-admission-v1` | qualification scope, storage instance, canonical admission-subject digest |
| `oxide-settlement-v1`, `oxide-terminal-v1`, `oxide-capture-v1` | qualification scope, storage instance, complete settlement-subject digest |

Callback context is the canonical digest of
`{scope, storage_instance_id, start_digest}`. Each callback already binds the
complete slot/instance/generation tuple, partition and exact payload; its
policy includes the registration and reviewed map pin. `CallbackAuthenticator`
implements only `OwnerReceiptVerifier`. `QualificationAuthority` implements
only daemon `EvidenceAuthority`, using a separately installed key. Admission
subject is `{binding, partition, admission_reference}`. Settlement subject is
`{binding, terminal, capture, containment}`, where terminal and capture contain
all their non-credential fields and containment contains its kind plus every
non-signature completion field (including lease, slot/path, process claims,
free-space samples, output and capture digests). `settlement_subject` defines
those exact JSON field names; unknown/running/process-absence containment has
no authenticated subject. The consumer, terminal and capture credentials use
separate domains over this same complete subject. Rust separately signs and
checks the private lease-token completion MAC; a callback receipt cannot replace
daemon settlement authentication. Parent settlement claims remain owner
assertions under the cooperative writer model.

The qualification input owner must keep a **separate map key and verification
identity** across sequential attempts at the same storage instance. The tested
map-seal domain is `oxide-map-v1`, with fields: scope, storage instance, slot ID,
canonical generation, canonical partition digest, canonical registration digest,
map-body digest. Attempt ID is deliberately outside this persistent seal;
Runner independently verifies the original map and exact current input state.
A per-attempt callback key/context may change without invalidating older map
seals. Recreated storage, changed tuple/policy/body, reminted/missing map key or
lost original verification identity refuses. No caller configuration installs a
trusted owner; the embedder must install these boundaries explicitly.

[Authentication vectors](../../../fixtures/qualification-auth-v1.json) and the
[independent Node verifier](../../../scripts/qualification-vectors.mjs) pin
UTF-8 length framing, canonical object-key ordering and receipt verification.
The covering fixtures use actual compiled Rust children and real durable lease
state for cold publication and sequential reuse, and interrupted-channel hold
across daemon restart. A pure pre-acquire marker test rejects fresh-bootstrap
replay and changed original requests without another host session. All registration, immutable,
execution, process-completion, capture and test results are **synthetic**.
There is no real Cargo execution, Nucleus benefit, production issuer, installed
service or production-readiness claim. Nucleus's existing client below is still
inactive and unchanged; its owner receives this callable surface only through
reviewed merge/closeout and separately scoped integration.

#### Private storage-runtime process (oxide#033)

The callable runtime is unpublished (`publish = false`) and behind feature
`private-qualification`. Artifact `oxide-storage-runtime`, source
`src/bin/storage_runtime.rs`, library `registered::storage_runtime`. Build with
`effigy rust:private-compile`. The covering `effigy rust:private-process` builds it
offline with the committed lockfile and passes its executable path as
`OXIDE_STORAGE_RUNTIME`. The proof launches actual children, uses the real storage
`Client` for cold publication, sequential same-slot reuse, unapproved admission and
forged settlement refusal, control loss and lifetime expiry with restart, then
authenticated shutdown. Its bootstrap, readiness and shutdown are constructed by the
trusted parent in test code. The parent never constructs `Daemon` for these sessions.

Callable surface: `oxide-storage-runtime <canonical-bootstrap-file>`. It takes one
non-secret path. Keys, budgets and roots come only from the bootstrap. No environment
discovery, command-line secrets, install, service or default executable is supplied.

Exit codes: `0` authenticated shutdown. `2` lifetime, control EOF or control refusal
after readiness, with durable state left as it was. `3` abandoned blocked storage
operation after readiness. `4` owner window ended before readiness; no ready frame was
published. `1` refusal before effects, or any other failure. Exits `2` and `3` write
`private storage runtime stopped: <Stop>` or an abandonment line to stderr. Exit `4`
writes a startup-expired line. No exit proves writer completion or Idle.

**Bootstrap** (version 1 JSON; unknown fields refuse; read with the oxide#029 protected-file
rules: canonical, owner-only mode 0600, one link, no-follow, stable identity, at most
64 KiB):

| Field | Meaning |
| --- | --- |
| `version`, `storage_protocol`, `control_protocol` | All `1`. Storage remains `OXSTOR01`, ledger version 3. |
| `scope` | Owner-approved qualification scope, 1–4096 bytes. |
| `expires_unix_seconds` | Owner's remaining window end, as WireU64. The window is counted from the entry sample, taken when `serve_bootstrap` is called and before the bootstrap is read. The entry monotonic instant and the full-precision wall clock are sampled as a pair, with the instant first. The lifetime is `min(expires - entry_wall, 2700 s - stop_grace - 2 s)`, in exact arithmetic. No fractional second is dropped. The monotonic deadline is `entry_instant + lifetime`, so it never passes the owner's end. The published expiry is the whole second at or before `entry_wall + lifetime`. It is never later than the deadline, so the parent may refuse up to one second early and never late. Startup time is inside the window. |
| `stop_grace_seconds` | WireU64, 1–10. Bounded wait after a stop before an in-flight storage operation is abandoned. Bound into `policy_digest`. |
| `storage_root` | Absolute canonical owner-only (0700) directory. The daemon endpoint is `<root>/daemon.sock`. |
| `budget` | `{cap_bytes > 0, free_reserve_bytes}` as WireU64. Explicit finite policy, not kernel enforcement, not aggregate footprint, not a host reserve. |
| `admissions` | 1–4 approved `{binding, partition, admission_reference}` subjects with no credentials. The owner mints credentials after readiness reports the instance. |
| `authority_key_hex` | 64 lowercase hex. Admission and settlement authenticator for `QualificationAuthority`. |
| `control_key_hex` | 64 lowercase hex. Readiness and shutdown authenticator, distinct from the authority key. |
| `control` | `{path, device, inode}` for the pinned parent Unix listener: socket mode 0600, owner uid, private 0700 parent, parent is not the storage root. |

WireU64 means a safe JSON integer through 2^53-1 or canonical decimal text above
that. Non-Rust producers must write exact integer digits or decimal text, never a
JavaScript `Number`.

**Process bound and startup supervision.** Before the bootstrap is read, the only bound is
the process ceiling of 45 minutes from entry. That is a compile-time constant, not
something read from the bootstrap. Reading the bootstrap is a bounded local file read that
does not wait on a peer. Validation then shrinks the bound to the owner's window, with the
startup-time rule above. A supervisor thread owns the bound. The startup worker sends it the
window end once validated, and the serve-phase bound once ready is written. Every blocking
step runs on the worker: the protected read, validation, marker write and sync, bootstrap
unlink, control connect, `Daemon::bind` and the ready write. The worker checks the window
before each step. If the bound passes while the worker is still blocked, the supervisor
returns `StartupExpired` (exit 4) before readiness, or `Abandoned` (exit 3) after it. The
binary returns from `main` and the process ends. Nothing is released, finalized or deleted.
Readiness is written only while at least 1 second of owner window remains. A worker
result that arrives after the bound is never observed. Process exit is not a writer
completion or Idle proof.

**Supervisor rule.** Each event, such as a queued window, ready or finished notice, is
applied only after the supervisor reads the current monotonic time. Before readiness, a
late event expires the startup. A late ready never changes readiness or extends the bound.
A worker that finishes after the bound before readiness is never a success. After
readiness, a second ready is a protocol failure. The decision is a pure step function
with unit tests for each ordering.

**Deadline checks around setup.** The worker checks the window after the control connect
and endpoint capture, before `Daemon::bind`, and again after bind. It checks before the
ready write. The ready write is bounded by the owner window, not a fresh tolerance. Partial
writes are re-armed with the remaining window, rounded to whole milliseconds with a
one-millisecond minimum. The write must complete inside the window, and the window is
checked again before the serve-phase bound is posted. A write that is still in progress
when the window ends is not a readiness event.

**What is guaranteed.** The runtime never posts readiness or starts serving after its
window has ended. The transport cannot be unsent, so a frame whose write completes at the
boundary may reach the parent. The parent therefore refuses a ready frame received at or
after its `expires_unix_seconds`, which `verify_ready` enforces. A truncated or incomplete
frame refuses. Not even a late authenticated frame can start serving, because the
serving step is never entered after expiry. Blocking connect and `Daemon::bind` have no
timeout of their own. Those steps are bounded by the supervisor, which exits the process at
the window, not by the step returning.

**Controlled pre-ready stall (unpublished test surface).** Feature
`private-qualification-fixture` is unpublished and off by default. It compiles two
blocking points. Each blocks on an owned FIFO that the trusted test creates in the owner's
control directory. The points are `before-bind`, after connect and endpoint capture, and
`before-ready`, after bind and frame construction. The bootstrap, environment and argv
cannot select or disable them. Actual-child proof: a window of three seconds reaches the
blocked step, the child exits 4 with `fixture stall entered: <point>` in stderr, and the
connection receives no bytes. For `before-bind`, bind never ran and no endpoint exists. For
`before-ready`, bind ran and the stale endpoint remains, because exit does not run the
daemon's drop. The replay marker stays, and the bootstrap is consumed. Replay of the
identical bytes is refused and the file is retained. The step expires by the window, not by
the test's own timing.

**Startup, before effects.** Read and validate the bootstrap. Validate the owned
storage root, the pinned control identity, the budget and the window. Create the
exclusive replay marker `runtime-<sha256 hex of the exact bootstrap bytes>`
(mode 0600, synced) in the control parent. Existing markers refuse, and the file
stays for the owner. Unlink the consumed bootstrap, then connect to the pinned control
listener and recheck its identity. Call `Daemon::bind` with `QualificationAuthority` and
`OwnedRootFreeSpace`. That probe calls statvfs on the one owned canonical root only.
It uses checked `f_bavail * f_frsize` and refuses unknown capacity or any other path.
Install the instance, capture the storage endpoint and write ready. A failure after
the control connection writes an authenticated `failed` frame when it can, with code
`refused`, `storage`, `io`, `protocol` or `expired`. Its write is bounded by the 2-second
exit tolerance. An expired window stops before the next effect. A marker that exists
without a finished startup stays in place, so a later replay is refused.

**Control frames.** Newline-delimited JSON. Each frame is at most 64 KiB. Reads total
at most 256 KiB across the session. Each blocked read resumes after 30 seconds. Every
runtime write, including ready, the stopped ack and failure frames, times out after
the 2-second exit tolerance. Unknown operations, unknown fields, bad MACs, oversize and
truncated frames refuse. Control traffic never renews the lifetime.

**Ready** (runtime to parent, one frame): `version`, `operation: "ready"`, `scope`,
`bootstrap_digest` (`sha256:` of the exact bootstrap bytes), `policy_digest` (canonical
digest of `{scope, storage_protocol, control_protocol, budget, stop_grace_seconds, admissions}`
with credentials excluded), `storage_instance_id` (64 lowercase hex), `storage`
(`{path, device, inode}`), `storage_protocol`, `control_protocol`, `budget`,
`expires_unix_seconds` (the whole second at or before the effective deadline), then `mac`. Parent verification
(`registered::storage_runtime::verify_ready`) checks the MAC, scope, digests, instance
text, the exact endpoint path and device/inode captured on the parent side. Ready is
metadata only. It grants no lease, token, admission or submission. The parent refuses a ready
frame received at or after its `expires_unix_seconds`, and it still verifies the MAC over
every field before it uses the identity.

**Shutdown and ack.** Parent to runtime: `{version: 1, operation: "shutdown", id, mac}`.
`id` is 1–128 bytes of `[A-Za-z0-9_-]`. The runtime stops its daemon, then sends
`{version: 1, operation: "stopped", id, mac}` only after an authenticated shutdown.
The parent verifier (`verify_stopped`) requires exactly these four fields. An extra
field refuses even when the MAC is valid. Frames the runtime reads also refuse
unknown fields.

**MAC domains.** Each uses HMAC-SHA256 through the 029 length-prefixed framing, with
the domain first. Keys are not interchangeable.

| Domain | Key | Fields after the domain |
| --- | --- | --- |
| `oxide-runtime-ready-v1` | control | scope, bootstrap digest, policy digest, instance, canonical digest of the ready body without `mac` |
| `oxide-runtime-shutdown-v1` | control | scope, instance, bootstrap digest, id |
| `oxide-runtime-stopped-v1` | control | scope, instance, bootstrap digest, id |
| `oxide-runtime-failed-v1` | control | scope, bootstrap digest, code |
| `oxide-admission-v1`, `oxide-settlement-v1`, `oxide-terminal-v1`, `oxide-capture-v1` | authority | unchanged 029 domains |

**Lifetime and stop path.** The deadline is monotonic. It is set once at entry, from the same
instant as the Unix timestamp used for the window. After readiness the storage daemon runs
on its own thread. The first stop reason, from the control reader or the deadline
supervisor, is forwarded to it. The worker then waits at most `stop_grace_seconds` for
storage to return. Then it runs the stop action for that reason:

| Stop | Trigger | Exit | Ack | Durable state |
| --- | --- | --- | --- | --- |
| `Shutdown(id)` | authenticated shutdown | 0 | `stopped` after storage returns | unchanged |
| `Lifetime` | effective window elapsed | 2 | none | leases stay held |
| `ControlClosed` | parent EOF | 2 | none | leases stay held |
| `ControlRefused` | refused control frame | 2 | none | leases stay held |
| `Abandoned` | storage still busy after the grace | 3 | none | last committed ledger; leases stay held |
| `StartupExpired` | window ended before readiness | 4 | none | no ready; replay marker and partial state kept |

`Abandoned` treats a blocked storage operation like a crash. The ledger commits by
temp file, sync, rename and directory sync, so a restart sees the last committed state.
The process exit ends the blocked thread, and the stale endpoint file stays until the
next bind removes it after a refused connect. The runtime does not wait for a stalled
client's own 5-second read timeout.

A held lease restarts as `Uncertain` through the existing daemon recovery. A runtime
that ends by exit, PID loss, deadline, EOF or abandonment never releases a lease or
makes a slot Idle. The runtime never deletes the bootstrap, replay marker, storage root, slots or
ledger. It stops only its own daemon and never a scheduler's external workload.

**Pins.** Map implementation `d9ff17988c7eaa5cf57747bc55bfe4f138692f15`. Rust
`1.97.1`. Dependencies `hmac 0.12.1`, `sha2 0.10.9`, `libc 0.2.190`, `serde 1.0.229`,
`serde_json 1.0.151`. Independent vector `fixtures/storage-runtime-ready-v1.json`.
The Node check in `scripts/qualification-vectors.mjs` recomputes the MAC and refuses
substitutions and forgeries. The Rust test runs the production verifier on the same
bytes.

**Timing.** This runtime does not use the framed host's version1 limits or the
oxide#032 runner timing window below. Real Cargo capacity wait, compilation, tests
and capture need the runner timing mechanism, and its adoption is a separate owner
decision.

**TypeScript owner sketch.** Illustrative only. The owner must verify every field
before it uses the instance.

```ts
const child = spawn(artifactPath, [bootstrapPath], { env: {}, stdio: ['ignore', 'ignore', 'pipe'] });
const ready = await readControlFrame(parentListener);       // newline JSON, <= 64 KiB
const identity = verifyReady(ready, expectation, controlKey); // HMAC + exact endpoint; no lease implied
// Mint admission credentials for identity.storage_instance_id with the authority key,
// then connect the existing client to identity.storage.path. Keep device/inode as exact text.
writeControlFrame(parentListener, shutdownFrame(id, identity, controlKey)); // expect stopped ack
```

**Known gap and not proven.** The storage client decodes `InvalidInput` and `Protocol`
error payloads without consuming their strings, so a daemon error of those kinds reaches
the caller as `trailing response data`. Both paths still fail closed. This is an existing
codec defect, not fixed here. Also not supplied: a production issuer or installed
authority, a persistent secret store, real Cargo input or debug capture, real footprint
evidence, host installation or service, consumer adoption and any real storage saving.
The runner timing work is a separate seam and is not changed here.
#### Remaining runtime and timing seams

Independent review of the actual process confirms that the unchanged
30-second I/O and five-minute session limits do not establish a fit for genuine
capacity wait plus compilation, tests and capture. oxide#032 supplies the
explicit finite mechanism below. It binds limits to the trusted bootstrap,
keeps a nonrenewable deadline and preserves ownership on timeout or channel
loss. It does not enlarge version1 defaults and is not authority to run the
real workflow or change consumer admission.

#### Finite qualification timing (oxide#032)

The bootstrap has an optional `timing` object. Absent means the unchanged
version1 host defaults. Present means an explicit finite window, and every
field is required and unknown fields refuse:

| Field | Type and unit | Rule |
| --- | --- | --- |
| `policy` | string | Only `oxide-qualification-timing-v1` is supported. |
| `callback_ms` | decimal u64 milliseconds | Greater than 0 and no greater than `remaining_ms`. Ceiling for one owner callback exchange, request write plus reply read. |
| `remaining_ms` | decimal u64 milliseconds | Greater than 0 and no greater than 2,700,000 (45 minutes). The owner's remaining session window at issuance. |
| `receipt` | `hmac-sha256-v1:<hex>` | HMAC-SHA256 with the bootstrap callback key over domain `oxide-qualification-timing-receipt-v1` and fields `scope`, `storage_instance_id`, `start_digest`, `policy`, `callback_ms`, `remaining_ms`. |

Issuance is constructor-owned: `TimingAuthority::issue(&bootstrap, callback,
remaining)` returns a signed policy only when its bounds pass. Verification refuses before the
bootstrap file is consumed, before connecting to the parent and before any
acquire. Unsupported policy and contradictory or out-of-range bounds return
`HostError::Protocol`. A receipt that does not verify for this exact bootstrap,
including a forged, reminted or moved window, returns `HostError::OwnerRejected`.

The session deadline is frozen once. `serve_bootstrap` takes a monotonic entry
instant before it reads the file, and `entry + remaining_ms` is the deadline.
Bootstrap reading, validation, connect and handshake all draw from that window.
Wall-clock changes, stream traffic, partial frames, retries and reconnects
cannot extend it. There is no keepalive or lifetime-extension path.

Each callback exchange has one ceiling: the smaller of `now + callback_ms` at
exchange start and the session deadline. Each frame read or write re-arms the
socket with the remaining part of that ceiling. Drip-fed bytes cannot renew it.
A complete frame that finishes after its ceiling is refused, not promoted. Expiry
is checked before each write or read, and again after a blocked completion. The
start-frame read is bounded the same way, so startup is charged to the session.

On timeout or channel loss the host writes an `error` frame with `code:
"timeout"` when the stream still accepts it. It never sends a result frame. It
does not cancel the scheduler, finalize, publish a map, release a lease, remove
markers or retry. Durable lease, root and map state stays as recorded, so the
slot remains Leased or Uncertain. A replay from a fresh bootstrap for the same
approved start refuses at the durable attempt marker before acquire.

Version1 keeps its 30-second callback ceiling and five-minute session measured
from channel creation. Its frames are now bounded in total as well, so the
change tightens behavior and does not relax it. Bootstrap `expires_unix_seconds`
remains a separate load-freshness check of at most 30 seconds. The execution
window applies only after that check passes.

Engineering ceilings are not measured workload timing. The 45-minute cap and
any chosen `callback_ms` are bounds for the mechanism, not evidence that a Cargo
build, test, capture or Nucleus workload fits. Qualification tests use short
synthetic windows. An owner's `execution.execute` callback is the whole
workflow receipt, so its `callback_ms` must cover that call. A smaller value
times out and leaves uncertainty, which is the correct failure.

Consumer handover after merge and closeout: the trusted launcher writes
`timing` into the one-use bootstrap, issued for the exact start digest. An
example owner-bound policy for a 45-minute remaining window with a 30-minute
callback ceiling is:

```json
"timing": {"policy": "oxide-qualification-timing-v1", "callback_ms": 1800000, "remaining_ms": 2700000, "receipt": "hmac-sha256-v1:<issued-by-trusted-launcher>"}
```

Pins for the reviewed surface are `TIMING_POLICY_V1`, `TimingAuthority::issue`
and `TimingAuthority` under the `private-qualification` feature. Nucleus must not
prompt workers or change admission from this contract alone.

### Inactive Nucleus client

Nucleus source `03b8eb95dbaad63ca154e81049280069f66697ea` contains
[`oxide-runner-transport.ts`](https://github.com/inflatable-cookie/nucleus/blob/03b8eb95dbaad63ca154e81049280069f66697ea/hosts/paseo/src/oxide-runner-transport.ts)
and opt-in fixture hooks in its existing `StorageAttempt`. The client uses a
synthetic child channel against the actual reviewed host schema; it does not
launch the Rust host or supply an executable. It pins the Oxide bridge
`24ad2848b1fb91760919b1693a143b5a35fec23c`, storage API
`ae56f4ebcfae65730a9de3ce26fd5d21386cc3f2` and registered-map implementation
`d9ff17988c7eaa5cf57747bc55bfe4f138692f15`.

Before delivery, the attempt retains the redacted start intent, exact original
body digest, acquire identity and reserved finalize ID. Trusted
`registration.authenticate` binds the acquired attempt, lease and full
instance/slot/generation tuple. The existing ledger persists acquired ownership
before acknowledging that callback or permitting `execution.execute`. Execution
uses the same durable submission identity and reviewed Effigy client; it adds
no second host submission path. Capture and finalization barriers precede the
corresponding replies. The client checks framed source-content correspondence
separately from Git head/tree and the consumer's retained-source digest.

Generation conversion refuses values outside the positive safe JavaScript
integer range. Framing, callback authentication, bounds and redaction are checked.
Original credentials and token material do not enter ordinary ledgers or logs.
There is no standalone parent lookup or cancellation wire operation. Session
loss or consumer reconciliation holds; the client never repeats `run` to imitate
lookup or recover missing secret material. Production registration refuses.

Independent exact-head review at
`8d118b2a6fc2d4baf066ea8c7d779c2d36171960` executed the covering storage-attempt,
runner-transport, validation-client and retention tests: 124 passed, zero failed,
eight existing private-qualification skips. Documentation checks passed. The
reviewer's piped typecheck lacked a captured exit status; owner reconciliation
on source `03b8eb9` separately established typecheck exit 0. These are fixture
and merged-head reconciliation results, not an actual cross-language host
session, trusted production issuers, real Cargo execution or reuse qualification.

### Real-workflow binding requirements

The fixture client is not the real-workflow owner adapter. Its source verifier
allows 128 files, 64 KiB per file and 128 KiB total; those bounds do not cover
the inspected Nucleus inputs. Qualification needs a finite, audited behavior
inventory with actual counters. The complete clean Git head/tree identity stays
distinct from the declared compiler-input digest. Omitting unrelated tracked
files from that input inventory requires the accepted behavior audit, not a
shortcut to make fixture limits pass. Every declared compiler input still needs
its consuming anchors and retained bytes.

The fixture's validation request fixes environment paths before acquisition,
whereas the actual intermediate path arrives with the acquired attempt.
The owner adapter must authenticate the acquired source/intermediate/target/temp
binding and persist the final request before the existing submission path can
start a writer. It must not guess a slot, allocate through a lookup, or mutate
an already submitted request. The host's `registration.authenticate` callback
provides the acquired attempt; a reviewed consumer binding must establish how
that callback closes this pre-submission seam.

The host's 30-second stream timeout and five-minute session bound are not
qualified for genuine capacity waiting, compilation, tests and capture.
oxide#032 supplies the reviewed finite timing mechanism (see Finite qualification
timing above). Evidence that the actual workflow fits is still required.
Synthetic timing is insufficient; a timeout retains uncertainty rather than
releasing ownership. No silent deadline increase or
unsupervised keepalive follows from the integration approval.

The pinned Cargo producer is now `registered::cargo_freshness::collect`, called
by an installed `InputEvidenceOwner::units_with_budget` with the runner's shared
`EvidenceReadBudget`. It returns `Vec<UnitEvidence>` to the existing registered
freshness checks; it adds no second guard or map version. The
callable signature is
`collect(&Registration, &Attempt, &BTreeMap<String, ImmutableManifest>,
&mut EvidenceReadBudget, &TrustedCargoGraphContext) -> Result<Vec<UnitEvidence>>`.
The `manifests` argument is the exact accepted receipt set already verified by
`Runner.inputs`; the owner override receives that same set and budget from
`Runner.collect`.

The private compiled child installs that reading owner when bootstrap
`graph_owner` is present. Absent `graph_owner` keeps the previous in-memory
fixture mode: `ParentCallbacks::units` is the authenticated `input.units`
callback and `units_with_budget` inherits that default. Collector mode never
falls back to a parent-produced `UnitEvidence` array; `units` itself refuses
`MissingAuthority`, and the child calls `input.graph_owner` then
`cargo_freshness::collect`. Precomputed `input.units` success arrays are not
collector proof. This is the synthetic qualification owner, not an accepted
Nucleus graph, native closure, issuer, completion or debug proof.

`TrustedCargoGraphContext` must be built by trusted embedding code after graph
provenance and complete behavior audit. It binds the registered graph digest,
exact semantic unit set, Cargo version/source revision, `Attempt.slot` build
root, named package roots and per-unit environment. Each `CargoUnitContext`
names one fingerprint directory/file and its complete directory-entry set,
exact binary dep-info mappings, package-relative rerun mappings, graph edges,
explicit generated/native/immutable associations and any audited registry-only
`Precalculated` policy. Do not construct it from task labels or caller JSON.
The type is not `Deserialize`; task JSON cannot assert trust.

### Callable synthetic graph-owner provisioning

This is the process API for an external parent that must not author a Rust
harness. Schema `oxide-qualification-cargo-graph-v1`, version `1`, issuer
`synthetic-qualification`. An incompatible schema, version or issuer refuses;
unchanged `MAP_VERSION` / map pin `d9ff17988c7eaa5cf57747bc55bfe4f138692f15`
do not imply provisioning compatibility.

Bootstrap field `graph_owner` is optional. Issue it with
`GraphOwnerAuthority::issue(&bootstrap)` using the bootstrap callback key over
domain `oxide-qualification-graph-owner-install-v1` and fields
`scope`, `storage_instance_id`, `start_digest`, schema, issuer, `"1"`.
Unsupported schema/issuer/version is `HostError::Protocol`. A forged, moved or
reminted installation receipt is `OwnerRejected`. Verification happens before
the one-use bootstrap is consumed, so a refused owner never connects.

After acquire, collector mode requests `input.graph_owner` with the Runner
`registration` and acquired `attempt` (slot included). The parent returns
untrusted `GraphProvision`: version, schema, issuer, Cargo `1.97.1` /
`c980f4866141969fab6254a680546a277789d6f0`, graph digest, a unit vector, and
`receipt`. Unknown fields refuse. Duplicate JSON object keys in that callback
result refuse at frame parse, before `serde_json::Value` canonicalization or
`GraphProvision` conversion; a collapsed Value must not be accepted as unique
context fields. Duplicate unit IDs refuse. The provision
must not name `build_root`; the Rust owner binds `TrustedCargoGraphContext.build_root`
to `Attempt.slot` after acquire. Package roots must sit under the authenticated
source root, acquired slot, or a Runner-verified immutable manifest root.

Issue the provision with `issue_graph_provision` (same callback key) over
domain `oxide-qualification-graph-owner-v1` and fields `scope`,
`storage_instance_id`, `start_digest`, canonical digest of the subject
`{body, source_root, root_id, run_id, attempt_id, build_root, storage_instance_id,
slot_id, generation, scope, start_digest}` where `body` is the provision with
an empty receipt. The cross-language vector is
`fixtures/qualification-graph-owner-v1.json`. Forged, moved, replayed or
slot/instance/generation-mismatched receipts refuse `Authentication` before
collector file reads.

Decode and copies charge the same `EvidenceReadBudget` as collector and Runner
input reads (256 MiB aggregate, 16 MiB/file, 256 units, 32,768 associations,
64 entries/unit, path 2048, depth 32, map 8 MiB). Oversize refuses without
resetting counters. The collector then runs under that budget and the existing
freshness/map/seal path. Post-execution fingerprint and anchor files are
observations named by the preapproved semantic graph; they are not graph
authority. On a cold empty slot, `Runner::run` still executes before the first
collect; reuse collect (and therefore graph verification) happens before
execute. Refusal uses existing host paths: no fabricated completion, release,
or new replay token. Same-UID cooperative ownership and no-replacement remain
the writer model.

Pins: `GRAPH_OWNER_SCHEMA`, `GRAPH_OWNER_ISSUER`, `GRAPH_OWNER_VERSION`,
`GRAPH_OWNER_PROVISION_DOMAIN`, `GraphOwnerAuthority::issue`,
`issue_graph_provision`, `accept_graph_provision`. Nucleus must not prompt
workers or change admission from this contract alone.

The accepted persisted formats are Cargo `Fingerprint` JSON and binary
`EncodedDepInfo` v1 at commit `c980f4866141969fab6254a680546a277789d6f0`
(Cargo version `1.97.1`). JSON is parsed with strict fields and bounded
sequences. The collector reads explicitly named Cargo evidence files beneath
the canonical build root; package and immutable inputs are associated here
and read by the existing runner under the same operation budget. The collector
supports `CheckDepInfo`, `RerunIfChanged`, `RerunIfEnvChanged` and
`Precalculated`. Every path/tag is resolved through the exact owner mapping;
package-root and build-root references remain distinct. It preserves absent
environment values separately from empty strings and records the actual
fingerprint, dep-info and rerun-output anchors. Duplicate or missing units,
edges, references, consuming mappings and directory entries refuse, as do
unknown fields/variants, duplicate JSON keys, unsafe or unstable files,
checksum freshness, mutable `Precalculated` and unsupported policies.

The collector shares the runner's 256 MiB operation budget, 16 MiB per-file
limit, 256-unit and 32,768-association ceilings, 64-entry per-unit fingerprint
inventory, 2,048-byte path and depth-32 limits. It charges evidence reads,
decoded and copied allocations, and repeated directory membership reads. The
existing map encoder enforces the 8 MiB persisted-map limit. The source-linked
JSON records and binary vector are hand-annotated pinned-format fixtures, not
output from an actual Cargo graph. They establish neither graph issuance nor an
audited native closure, installed issuer, real registry/SDK scan, Nucleus reuse
or consumer capture. The trusted owner must still authenticate the real graph,
resolve every consuming reference and prove all native, proc-macro and SDK
reads; archive agreement alone cannot prove that closure. Real full-debug capture
and retrieval remain the consumer's separate obligation.

Consumer implementation admission remains under the Nucleus owner. The earlier
single-client capacity exception ended at closeout; the currently prepared
real-workflow client has no new scoped capacity grant. This is separate from
the approved integration implementation and its API/qualification gates.

### Callable accepted audit record (oxide#035)

Private qualification only. The bootstrap field `audit_owner` (absent keeps the
previous synthetic-only behaviour) installs an accepted audit owner through
`AuditOwnerAuthority::issue(&bootstrap, owner, policy_digest)`. Schema
`oxide-qualification-audit-record-v1`, version `1`, issuer
`synthetic-qualification`. Install receipt domain
`oxide-qualification-audit-owner-install-v1` over `scope`,
`storage_instance_id`, `start_digest`, schema, issuer, owner, policy digest and
version. Unsupported schema, issuer or version is `HostError::Protocol`; a
forged or reminted install is `OwnerRejected`. A `graph_owner` without
`audit_owner` refuses before the one-use bootstrap is consumed. The owner
identity and policy are embedder-installed; task JSON, labels and environment
cannot name them.

Order per acquired attempt, cold or reused: `registration.authenticate` →
`registration.audit` (the record, verified by `verify_audit_record`) →
`input.graph_owner` (`authorize`, a provision accepted before any file read) →
[reused only: map verify and collect] → `execution.execute`. Post-execution
`inputs` repeats the audit and requests the provision again; a changed digest
refuses, and an identical provision is not charged again. Collect never requests
a second provision; it uses the context accepted before execution, so
post-execution observations cannot redefine the graph. A cold empty slot is
authorized without requiring fingerprint, dep-info or rerun files to exist.

The parent returns an untrusted `AuditRecord` (unknown fields refuse): owner,
policy digest, registration ID, graph digest, `registration_digest`
(canonical digest of the complete `Registration`), `attempt_digest` (canonical
digest of the acquired `Attempt`, including source tree binding, slot,
generation, lease and partition), `closure_receipts` and `receipt`. The receipt
is HMAC over domain `oxide-qualification-audit-record-v1` and fields `scope`,
`storage_instance_id`, `start_digest` and the canonical digest of the record
body. `closure_receipts` must equal `Registration.immutable_receipts` exactly;
there is no boolean closure-complete flag. Verification refuses owner or policy
mismatch and forged receipts (`Authentication`), changed registration, graph or
attempt identity (`Identity`), and closure drift (`Membership`). Moving to
another storage instance refuses by receipt.

Synthetic signing (`audit_record_receipt`, `describe_audit_record`) is test
evidence under the fixture key. It is not the Nucleus closure audit, a native or
SDK receipt, or an installed production issuer. The accepted owner, its policy
and its independently reviewed audit record remain absent. Real behaviour
audit evidence is still required before any real workflow may execute.

### Field correspondence

| Nucleus subject or receipt | Oxide field | Rule |
| --- | --- | --- |
| `consumer` | `RunBinding.consumer` | exact string |
| `project` | `RunBinding.project` | exact string |
| `component` (explicit admitted registration) | `RunBinding.component`, `PartitionKey.project_component` | must match; registration name is proposed |
| `workflowVersion` | `RunBinding.workflow`, `PartitionKey.workflow` | proposed canonical `<workflowId>@<version>`; not a current Nucleus string |
| `task` | `RunBinding.task_id` | exact |
| `workspace` | `RunBinding.workspace_id` | canonical workspace ID, not the branch name |
| `branch` | `Attempt.branch`, `Launch.branch` | not in `RunBinding`; must not enter partition identity |
| `run`, `attempt` | `RunBinding.run_id`, `RunBinding.attempt_id` | exact |
| `head` | `RunBinding.commit`, `SourceEvidence.commit` | Git commit OID; must match exactly |
| `tree` (Git tree OID) | none | Nucleus identity only; not Oxide's source digest |
| source inventory | `RunBinding.source_tree_digest`, `SourceEvidence.tree_digest` | Oxide framed digest, defined below |
| `rootId`, `rootPath` | `Attempt.root_id`, `Attempt.source_root` | canonical; `Attempt.slot` is the daemon `slot_path`, never `rootPath` |
| `allocationEpoch` | `Attempt.allocation_epoch` | canonical decimal string; no daemon field, so the admission verifier must check it |
| `partitionId` | `Attempt.partition` | digest of canonical `PartitionKey` (11 fields); no task, head, run or epoch inside |
| lease ID | `AcquireResult.lease_id`, `Attempt.lease_id` | daemon string; Nucleus `Lease.id` must equal it |
| `Lease.generation` | `AcquireResult.generation` → `Attempt.generation` | canonical positive u64 text; convert to the TypeScript safe number only after a checked range test |
| storage instance | `AcquireResult.storage_instance_id` | bind with `slot_id` and generation so ledger recreation cannot validate old maps or receipts |
| acquire `token` | `AcquireResult.token` (`LeaseToken`) | host memory only; Nucleus stores a digest |
| authorization | `Attempt.authorization`, `RunEnvelope.admission_reference` | Nucleus authorization receipt operation and payload digests |
| admission credential | `RunEnvelope.admission_credential` | owner-installed private qualification MAC exists; no production consumer issuer (see owner table) |
| `reused` | `AcquireResult.reused` → `Attempt.reused` | daemon value only |
| required bytes | `AcquireRequest.required_bytes` | Nucleus policy byte bound |
| `target`, `temp`, `capture` | `Attempt.target`, `temp`, `capture` | canonical private paths; `Runner` adds `CARGO_*`/`TMP*` env, which the audit must approve |

### Digest identities (all distinct)

| Digest | Producer and encoding | Use |
| --- | --- | --- |
| `sourceManifestDigest` | Nucleus local manifest in `root-allocation.ts` and `storage-attempt.ts` | consumer identity only |
| `retainedSourceDigest` | `oxide-retention.ts`: SHA-256 over JSON `{path, sha256, bytes, mode}` sorted by **`localeCompare`**, bare hex file digests, source and behavior classes only | consumer retention only; never forwarded as Oxide |
| Git head / tree | 40 or 64 hex object IDs | Nucleus identity |
| Oxide `tree_digest` | `sha256:` + hex of SHA-256 over, for each `(name, file)` in Rust `BTreeMap<String,String>` order (UTF-8 byte order), `frame(name)` then `frame(file)`. `frame(x)` = u64 little-endian byte length + bytes. `file` = `sha256:` + 64 lowercase hex of the file bytes | `RunBinding.source_tree_digest` |

The file digest is the `sha256:`-prefixed string. Nucleus's bare-hex `sha256` must be
prefixed before framing. Both `retainedSourceDigest` and JS default `.sort()` order differ
from UTF-8 byte order, so neither may feed Oxide.

### One bounded correspondence proof

1. Clean committed tree only. `git status --porcelain=v1 -z --ignored --untracked-files=all`
   is empty, with bounded output and no host scan. Submodules (mode 160000), LFS
   attributes, symlinks (120000) and non-blob entries refuse.
2. `git ls-tree -r -z --full-tree <head>` lists the set of regular blobs with modes
   100644 or 100755 only. Each path must be valid UTF-8, contain no NUL, and have only
   `Normal` components within the 2048-byte and depth-32 limits (`relative()` enforces these).
3. Set equality: ls-tree paths = `Registration.source_files` = `SourceEvidence.files` keys.
4. For each path, read the owned bytes (per-file 16 MiB, inventory 256 MiB). Recompute
   the Git blob OID (`blob <len>\0` + bytes, SHA-1 or SHA-256 per repo) and require it to
   equal the ls-tree OID. This is the **added** check, binding disk bytes to the committed
   object. Oxide's existing check (`state.digest != expected`) only binds the SHA-256.
5. Sort by UTF-8 bytes, build the framed digest, and compare with `RunBinding.source_tree_digest`.
6. Modes are not in Oxide's digest. Nucleus records modes in its manifest and refuses any
   mode other than 100644 or 100755.
7. Integers. Byte counts and epochs cross JSON as safe integers only (≤ 2^53−1). Larger
   values must be canonical decimal strings (no leading zero, `0` alone). Parse as `u64`
   in Rust. Floats, negatives and non-canonical forms refuse.

### Previous map provenance and PR25 framing

- Reuse path: `verify_map(attempt, digest(encode(previous, "")), seal)`, then
  `collect(current)`, then `admit(previous, current)`, which compares publication,
  identity, tree, semantics, source and anchors. A passing previous map is provenance
  only; `reused` is not authority.
- PR25 (`57ee0cf`) changed the semantic digest. `frame(unit.id)` and, for
  `LocalInput::DepInfo`, `frame("dep-info")`, `frame(evidence_unit)`, `frame(anchor)`
  are now in the semantic framing, and `LocalInput::DepInfo` gained `unit` (checked
  against `UnitEvidence.id`). `MAP_VERSION` and the map and pending/candidate prefixes
  stayed `1`, so bytes alone do not distinguish old maps.
- Binding rule: the Oxide implementation pin (`d9ff179`, or a later reviewed head) is
  bound into the **owner seal**, which `seal_map` and `verify_map` already authenticate.
  Verification refuses a seal that lacks the pin. Pre-PR25 maps are therefore refused
  rather than converted. Admission's semantic mismatch is not relied on alone, because it
  only catches units with dep-info.

### Replay, lost reply and no-spawn

- Acquire replays the same `acquire_id` with an identical body (Nucleus `bodyDigest`).
  A changed body refuses. After a lost acquire reply or cancellation, the caller
  uses `Client::lookup_acquire` with the exact authenticated request before any
  retry decision. `Present` or `Uncertain` never starts another acquire; `Absent`
  is only a lookup result and does not reserve a slot.
- Finalize uses the existing `operation_id`. After a lost finalize reply, the
  caller uses `Client::lookup_finalize` with the original evidence and ID.
  `Present` reflects the accepted operation without repeating settlement;
  `Uncertain` keeps the lease held; `Absent` does not itself finalize.
- These Oxide lookup APIs are implemented, while the proposed bridge and Nucleus
  call flow remain absent. `Client::acquire` still allocates when an attempt is
  absent, and `Client::reconcile` may change lease state; neither substitutes for
  lookup. Lost tokens or uncertain ownership hold; a finalized replay grants no access.
- Time, PID, filenames (`PENDING`, `CANDIDATE`), labels, SHA hashes and fixture booleans
  carry no authority. The durable pending barrier is ineligibility, not permission.
- `NotSpawned` finalizes only with authentic no-writer evidence. Timeout, PID absence and
  archive state never release a lease. Uncertain ownership holds the lease, reservation and
  root.
- Lease token material never appears in serialized frames, logs or the ledger (digest only).

### Refusal rules

- Any mismatch across `Attempt`, `RunBinding`, daemon results and Nucleus receipts refuses.
- `acquired` without `slot_path`, `token` or `generation` refuses.
- A map without the bound implementation pin refuses.
- Non-UTF-8 paths, symlinks, submodules, LFS, modes other than 100644/100755, and unsafe
  integers refuse.
- A `reused` flag not returned by the daemon refuses.
- Per-revision, task, branch and epoch identities never enter `PartitionKey` or `partitionId`.
- Unknown op, version or frame size refuses. Contract 010 is unchanged.

### Owner table and missing seams

| Missing seam | Owner | Status |
| --- | --- | --- |
| Admission credential and `EvidenceAuthority::verify_admission` | Nucleus task admission and Queue host authority; Tom decides which signs | private qualification authenticator only; no production consumer issuer |
| Settlement and capture credentials (`SettlementEvidence.consumer_credential`, `TerminalRecord`, `CaptureReceipt`) | Queue scheduler envelope and Nucleus capture owner | no source |
| `RegistrationOwner.authenticate` and `audit` | Nucleus managed-Git and root owner; audited-input owner | private qualification verifies an installed synthetic audit record before graph acceptance and execution (oxide#035); no accepted production owner or real closure audit |
| `InputEvidenceOwner.immutable`, `units`, `units_with_budget`, `seal_map`, `verify_map` | installation registry and SDK owner; Cargo graph audit | private synthetic reading owner installs `units_with_budget` → `collect`; no accepted production owner or issuer |
| `ExecutionOwner.execute`, `verify_success`, `finalize_*` | Queue host authority (submission); Nucleus capture owner | TS fixture only |
| Daemon `generation` and storage-instance identity in `AcquireResult`; epoch in admission | Oxide storage owner | Oxide API implemented; Nucleus fixture client binds the complete tuple and checks safe conversion; production binding disabled |
| Authenticated lookup-only acquire/finalize recovery | Oxide storage owner; Nucleus attempt owner | Oxide APIs implemented; private host uses them internally; Nucleus session-loss reconciliation holds and adds no lookup wire verb |
| Oxide `EvidenceAuthority` implementation | Nucleus embedding, in process | missing |

Smallest proposed issuer API, needing owner decision: Nucleus task admission signs a
`nucleus.storage.authorization` v1 receipt bound to the full subject. The Rust host
verifies it against a verifier key installed at embedder construction. `verify_admission`
returns true only on that verification. Credential material is never logged or persisted.

Concrete cross-language embedding is buildable and testable with a synthetic parent.
Installed real authorities (issuers, registry/SDK receipts, complete Cargo graph and
native closure, cooperative completion, full-debug retention) remain separate gates. No
bridge or synthetic test supplies them.

## Durable generation and lookup recovery (oxide#027)

The approved Oxide API now adds `AcquireResult.storage_instance_id: String`
and `AcquireResult.generation: String`. The generation is the durable ordinal
already encoded by `slot-<16 lowercase hex digits>`, rendered as canonical
positive u64 decimal text (`1` through `18446744073709551615`, with no sign,
whitespace, or leading zero). The ledger persists a random 256-bit
`storage_instance_id`; the returned identity is the tuple
`(storage_instance_id, slot_id, generation)`. Generation stays stable over
sequential leases and source revisions on one physical slot. A replacement
slot gets the next durable ordinal. Ledger recreation creates a new instance
identity, fencing previous maps and receipts even if the new ordinal repeats.
The slot counter is not reset by eviction.

Nucleus's fixture client implements its reviewed mapping: place this decimal
text in `Attempt.generation` unchanged, and
convert to the existing TypeScript numeric generation only after checking
`1 <= value <= Number.MAX_SAFE_INTEGER`. Oxide rejects malformed, noncanonical
and overflowing u64 values. The client checks conversion and binds the full
tuple; production registration remains disabled.

`Client::lookup_acquire` takes the original authenticated `AcquireRequest` and
returns `LookupResult<LeaseIdentity>`. `Client::lookup_finalize` takes the
lease ID, original finalize ID and authenticated settlement evidence and
returns `LookupResult<FinalizationReceipt>`. Exact request digests include
credentials but only their digests are retained. Changed bodies refuse;
invalid credentials fail authentication; a changed binding for a known lease
conflicts. Results distinguish `Absent`, `Present`, and `Uncertain`. An
uncertain finalize with no durable operation returns `Uncertain(None)`; an
accepted uncertain operation includes its receipt. These read-only methods do
not allocate, evict, settle, mint a token, or return a slot path. Normal
`Client::acquire` still allocates on a miss and cannot replace lookup recovery.
Lost tokens remain held, and a finalized replay grants no access.

The local ledger format is version 3; version-2 data is refused without
migration or reinterpretation. The framed local protocol is `OXSTOR01` plus
version 1 on every request and response. Unversioned or unsupported peers are
refused before dispatch, so they cannot receive a misleading acquire result
or cause an allocation they cannot decode. No old retained root or uncertain
resource is opened through a compatibility shim.

| Source pin | Evidence used |
| --- | --- |
| Oxide base `c50af3874fdb90edaceecaaeb53fc256ff73afa7` | Approved implementation base; `AcquireResult` had no generation |
| Oxide implementation `bf27ff748dc641c18126d2e698e37800d147856a` | Durable slot identity, authenticated lookup APIs, versioned protocol, and targeted regressions |
| [Nucleus review `9a562d1492ddb1474ebb2ea503889ecc6b0845b1`](https://github.com/inflatable-cookie/nucleus/commit/9a562d1492ddb1474ebb2ea503889ecc6b0845b1) | Nucleus `Lease.generation` uses a positive safe JS number; registered `Attempt.generation` is text; owner-selected mapping |

Oxide implementation sources are `src/storage/types.rs`,
`src/storage/store.rs`, and `src/storage/transport.rs`; targeted regressions
are in `src/storage/tests.rs` and the transport codec tests. No Nucleus source,
SDK, issuer, service endpoint, or consumer configuration is changed by this
PR.

### Gates still open

Admission setting stays at 0. Real credentials and issuers need owner decisions. Real
evidence needs a scoped audit: Cargo unit graph and proc-macro execution, bundled
SQLite native inputs, SDK membership, and closure bytes against the 520 MiB accepted-read
ceiling. No scan or Cargo invocation is authorized here. Numeric production budgets
(8/16 GiB), Contract 002 departure and SSD-write measurement remain separate decisions.
