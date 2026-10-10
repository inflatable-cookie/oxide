# First Nucleus core-test reuse contract

Status: proposed consumer contract, with inactive library seams implemented.
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
accepted tool manifest. The issuer, rather than Oxide's byte checker, must prove
archive-to-extraction correspondence and preserve the no-replacement agreement
through the lease. A correct byte hash does not prove complete native/proc-macro
behavior. The separate audited closure remains essential. There is no archive
extractor, whole-host scanner or shipped native/SDK issuer here.

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
deadline expiry alone never releases ownership. The missing consumer-owned
storage-attempt ledger must persist authorization, acquire/submit intents and
identities, busy/uncertain outcomes, cancellation and reconciliation. Existing
operation journals and worker admission are patterns, not this API.

Nucleus's [consumer boundary, Contract 011](https://github.com/inflatable-cookie/nucleus/blob/b4321e2fd729cd3329e7111a2eacbe1ec27d934e/docs/knowledge/contracts/011-oxide-consumer-boundary.md)
defines proposed version-1 consumer-local receipts for allocation, storage
authorization, acquire, submission, capture, archive/recycle and no-spawn closure.
They bind the task/run/root allocation epoch, requested source tree, workflow and
partition, with operation/body identity and a verified issuer boundary. These are
consumer-owned fields, not shipped Oxide RPC names. Fixture verifiers establish
adapter behavior only; unconfigured production issuers refuse. Live binding
requires an independently reviewed mapping to the actual Oxide API. Per-run head
and tree binding must not make every source revision a new compatibility partition.
Contract 011 describes the consumer-local mapping to the Rust library above;
the TypeScript consumer seams do not supply an authenticated concrete Rust
embedding or transport. That binding is a separate implementation prerequisite,
along with accepted owners and genuine input/completion evidence.

Nucleus's managed-Git direction is not yet a qualified physical-root pool.
Placement still creates clones. Its current preservation hashes timelines and a
Git bundle, and archive observes host workspace state; neither proves complete
old-run source/generated/dependency/debug retrieval. Shared repository storage
also requires replacing `bundle --all` with task-scoped refs/object pins so
preservation cannot include unrelated tasks' unpublished refs. Extend those
existing placement, integration-preparation, preservation and archive owners;
Oxide must not become their Git manager. A durable capture/archive/recycle
acknowledgment and verified old-run lookup are missing prerequisites.

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
