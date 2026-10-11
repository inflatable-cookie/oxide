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
ustar/pax tar whose members share one canonical package top-level prefix,
matching a Cargo `.crate`. The extracted directory's last component must equal
that prefix. The verifier enforces the locked SHA-256 of the compressed archive
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
extracted package root. They are not archive members and are never read as
attestations of archive contents. Those names inside the archive refuse.
Nested copies, and any other extra extracted file, refuse; extra files are not
skipped. Symlink, hardlink and special tar entries, extracted symlinks,
extracted regular files with `nlink ≠ 1`, and extracted special files refuse.
Absolute/parent/escaping paths, `.`/`..` components, backslashes,
non-UTF-8 names, duplicate or file/directory collisions, unknown formats
(including V7/old and GNU tar headers, bare tar, extra gzip members and trailing compressed bytes),
corrupt or truncated streams, and unstable reads refuse.

Reads charge one shared preparation budget that is not reset per member, phase
or package: compressed archive bytes, decompressed tar bytes, and extracted-file
bytes. `verify(request, limits, counters)` continues from the caller-supplied
`Counters` and returns `Outcome` with the aggregate `Counters`
(`archive_bytes`, `decompressed_bytes`, `extracted_bytes`, `entries`) and a
`Refusal` category on every attempt, including success. Nucleus engineering
maxima are 256 MiB collection, 16 MiB per decoded or extracted file, 32,768
inventory entries, 2,048-byte paths and depth 32; callers may pass lower
bounds. The collection cap applies to the accumulated counters, so successive
package verifies in one preparation share it. There is no unpack/write into the
package root, host registry scan, per-call counter reset or automatic growth.

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

## Inspected workflow and identity

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
not a shipped RPC, accepted issuer, live binding or evidence. Pins: Oxide main
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

Where a new local boundary is required: nothing in Oxide exposes `Runner` out of
process today. The framed host above is that boundary. It is new, Oxide-owned and
inactive until an embedder spawns it.

### Inactive framed host (oxide#028)

The callable library entry points are
`registered::runner_host::RunnerHost::new` and
`registered::runner_host::RunnerHost::serve`. Construction installs a Rust
storage::Client and an embedder-provided OwnerReceiptVerifier; it does not
spawn a process, register a workflow, install verifier material, or enable a
default host. The embedder supplies one private Unix stream per attempt. No
executable or distribution entry point is shipped.

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
`registration.audit`, `input.immutable`, `input.units`, `input.seal_map`,
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
| admission credential | `RunEnvelope.admission_credential` | no accepted issuer exists (see owner table) |
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
| Admission credential and `EvidenceAuthority::verify_admission` | Nucleus task admission and Queue host authority; Tom decides which signs | no issuer in source; the host calls the accepted verifier |
| Settlement and capture credentials (`SettlementEvidence.consumer_credential`, `TerminalRecord`, `CaptureReceipt`) | Queue scheduler envelope and Nucleus capture owner | no source |
| `RegistrationOwner.authenticate` and `audit` | Nucleus managed-Git and root owner; audited-input owner | missing |
| `InputEvidenceOwner.immutable`, `units`, `seal_map`, `verify_map` | installation registry and SDK owner; Cargo graph audit | decoder only; no issuer |
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
