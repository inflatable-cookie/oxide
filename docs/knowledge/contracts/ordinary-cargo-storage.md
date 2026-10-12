# Ordinary-Cargo same-root storage route

Status: the versioned Oxide storage API and one private qualification-only
ordinary-Cargo attempt route are implemented. The route is inactive unless a
trusted owner installs the protected authority and launches the private
process. Synthetic route fixtures do not qualify real Cargo behavior, install
policy, or authorize consumer activation. This contract defines one graph-free
route for a single consumer-owned source workspace and sequential attempts at
that same physical root.

## Decision and boundary

Use ordinary Cargo as the freshness authority for one explicitly accepted,
cooperative workflow. Keep the existing durable Oxide storage client and the
consumer's current StorageAttempt, Effigy 010 submission, settlement, and
retention owners. The qualification-only route acquires storage before it asks
the consumer to submit a host run. It returns the exact approved selector,
argv, source root, acquired slot as `CARGO_BUILD_BUILD_DIR`, and private
per-attempt target/temp/capture paths to that consumer. It never starts Cargo;
the consumer's existing Effigy 010 path remains the only host-submit path and
must run the declared tests.

The consumer gives this route one fresh physical root and owns its allocation
for the whole task lifecycle. It may make sequential attempts there, with a new
run and attempt identity for each invocation. This design does not recycle a
root between tasks or promise cross-root reuse. The source tree and content
digest identify each attempt; they are not a per-revision slot key. Root ID and
allocation epoch remain authenticated consumer identities. The compatibility
partition binds the physical root and accepted Cargo/tool/configuration
environment. Task, workspace, branch, run, attempt, source, root allocation,
storage instance, slot, generation, and lease identities remain distinct.

This is a cooperative policy departure from the registered
`oxide_storage::registered::Runner` route. It assigns ordinary Cargo freshness
and declared project behavior to a bounded consumer-approved workflow. It does
not weaken, bypass, or relabel the registered route's audited graph, immutable
input receipts, tool manifests, or exact environment evidence. The registered
Runner remains unchanged and is still no-go for real Nucleus without its
existing full input boundary. This proposal creates no graph resolver, parser,
compiler wrapper, scheduler, trusted issuer, or generic provenance wrapper
chain. Task 043's parser schema is not a dependency.

## What exists and what remains unqualified

Shipped Oxide storage APIs include generic `storage::Client::acquire`,
`lookup_acquire`, `finalize`, and `lookup_finalize`, plus the ordinary-Cargo
methods documented below. Both routes use the durable
`(storage_instance_id, slot_id, generation)` identity and opaque lease token.
Lookup methods are read-only: `Absent` permits only an exact replay of the
original operation; `Present` and `Uncertain` grant neither a new attempt nor
access to slot bytes. The lease token remains in Rust host memory and is never
persisted in the ledger.

Generic `Client::acquire` still returns a matching idle generic slot as
`reused=true` and may evict an eligible generic idle slot for capacity. It does
not implement ordinary-Cargo selection. Ordinary generations carry separate
durable eligibility and are explicitly excluded from generic acquire,
finalize/reconcile, and eviction. The current `same_root_fixture_v1` path
identity remains fixture-only and retains its own one-partition rule.

`Client::acquire_ordinary_cargo` supplies the required state-aware seam. It
supports exactly the `ordinary-test` and `full-debug` labels, binds the
canonical physical source root, consumer root ID, allocation epoch, policy and
installation epochs, accepted input-boundary digest, and complete existing
`PartitionKey`. Source commit/tree and task/workspace/run/attempt identities
remain per-attempt fields and do not create more partitions. A compatible
generation is warm only when its state is `reusable`; `empty` admits cold work;
`pending`, `Uncertain`, unknown, incompatible, and `cold-only` states refuse.
Admission never calls generic `make_room` or LRU eviction. Each accepted root
allocation has at most one generation for each of the two labels.

The durable barrier belongs in the versioned storage-ledger record, bound to
`(storage_instance_id, slot_id, generation, lease_id)` and the exact ordinary
request. Acquire selects the generation and commits its `pending` state, lease,
and saved prior state (`empty` or `reusable`) together before returning its
path or token. Authenticated finalize makes a generation `reusable` only after
successful tests and complete capture, or `cold-only` after an authenticated
settled failure. Authenticated `NotSpawned` restores the saved state and
releases occupancy without allocating another generation. An uncertain writer
stays `pending`; lookup cannot clear it.

## Shipped ordinary storage API

The private local transport is `OXSTOR01` protocol version 2. The durable
ledger keeps the `OXIDEST1` magic and uses version 4. Version-3 and other
unsupported ledgers are refused without migration or mutation. Protocol
version 1, unversioned frames, and unsupported peers are rejected before
dispatch. Changing these request or response encodings requires another
protocol version.

`OrdinaryCargoRequest` carries an existing `AcquireRequest`, a
`StrictPhysicalCargoRoot` (`source_root: PathBuf`, `consumer_root_id`, and
`allocation_epoch`), one `OrdinaryCargoPartition` (`OrdinaryTest` or
`FullDebug`), and `OrdinaryCargoAcceptance` (`policy_version == 1` or `2`,
`installation_epoch`, `acceptance_reference`, SHA-256 `input_boundary_digest`,
and an opaque credential). Version 1 keeps exact input-boundary digest reuse
semantics. Version 2 uses the typed stable eligibility reference and separate
attempt authentication described in [ordinary Cargo reuse identity](ordinary-cargo-reuse-identity.md).
The source root must be an existing canonical absolute physical directory.
The complete existing `PartitionKey` supplies the compatibility partition;
the task/run/source identity remains bound in the request but is not a
partition key.

The client API is:

```rust
pub fn acquire_ordinary_cargo(
    &self,
    request: OrdinaryCargoRequest,
) -> Result<OrdinaryCargoAcquireResult, StorageError>
pub fn lookup_ordinary_cargo(
    &self,
    request: OrdinaryCargoRequest,
) -> Result<LookupResult<OrdinaryCargoLeaseIdentity>, StorageError>
pub fn finalize_ordinary_cargo(
    &self,
    request: OrdinaryCargoFinalizeRequest,
) -> Result<OrdinaryCargoFinalizationReceipt, StorageError>
pub fn lookup_finalize_ordinary_cargo(
    &self,
    request: OrdinaryCargoFinalizeRequest,
) -> Result<LookupResult<OrdinaryCargoFinalizationReceipt>, StorageError>
```

Acquire responses include the policy version, root, partition, durable reuse
state, and existing `AcquireResult`. Lookup returns only an access-free lease
identity. Finalize repeats the exact ordinary request, lease ID, operation ID,
and authenticated settlement evidence. The token is verified against the
lease but excluded from the operation-identity digest; the digest binds every
ordinary policy/root/epoch field and the evidence. Finalize lookup sends no
token and returns no path or access. Exact replay returns the stored result; a
changed request or evidence conflicts, and invalid credentials are
unauthorized.

`EvidenceAuthority::verify_ordinary_cargo_admission` and
`verify_ordinary_cargo_settlement` authenticate each exact request and its
accepted input boundary. Both have safe defaults that return `false`. Policy
version 2 also requires
`verify_ordinary_cargo_stable_reuse_policy`; its safe default is `false`, and
it authenticates the installed stable eligibility independently. A
caller-provided flag or digest cannot install this trust. Missing or unknown
input-boundary acceptance refuses before a lease can become warm. Versioned
generation comparison and per-attempt evidence are owned by the linked
identity contract.

`CargoReuseState::{Empty, Pending, Reusable, ColdOnly}` is separate from
`SlotState::{Idle, Leased, Uncertain}`. The ledger stores root identity,
partition label and compatibility key, policy/installation epochs,
acceptance reference and input-boundary digest, saved pre-acquire state, and
finalized transition. Version 1 compares the generation digest with each
accepted attempt; version 2 preserves the generation's first-attempt digest
while comparing the stable eligibility reference and checking each historical
attempt against its own exact request. Credentials are redacted and lease
tokens are stored only as hashes. Restart changes an active lease to
`Uncertain` while retaining `Pending`.

Admission uses checked arithmetic for measured and reserved bytes against the
configured aggregate cap and free-space reserve. Unknown accounting or free
space refuses. Explicit capacity results include
`OrdinaryCargoPending`, `OrdinaryCargoColdOnly`,
`OrdinaryCargoPartitionLimit`, and `OrdinaryCargoIncompatible`. There is one
generation per root allocation and label, with no successor or implicit pool
growth. Generic acquire rejects the reserved ordinary path identity; generic
finalize/reconcile and lookups cannot operate on ordinary leases; generic
eviction and `make_room` skip ordinary records.

Synthetic socket fixtures cover cold and reusable no-spawn restoration,
successful capture before reuse, settled failure and cold-only refusal,
concurrent pending refusal, two-partition and checked budget bounds, exact
lookup/replay and credential redaction, generic bypass/eviction refusal,
uninstalled policy rejection, unsupported ledger/protocol versions, and
restart uncertainty. They do not run Cargo or establish a real input
boundary.

The shipped private Rust routes are available, in Unix builds with the
`private-qualification` feature enabled, as
`oxide_storage::ordinary_cargo::{OrdinaryCargoAttemptRunner,
OrdinaryCargoAttemptBudget, OrdinaryCargoCaptureBudget, OrdinaryCargoInvocation,
run_attempt, serve_bootstrap}` and as the feature-gated unpublished
`oxide-ordinary-cargo-attempt` binary. The binary accepts exactly one argument:
the path to a protected, one-use bootstrap. Both entry points use the actual
ordinary storage methods, validate the acquire reply against access-free lookup
and durable inventory, then issue at most one submit callback. They do not
execute Cargo or construct registered `Registration`/`UnitEvidence`.

Oxide now defines the private callback schema below, but there is still no
Nucleus `StorageAttempt` installation of it. Installation policy and accepted
real input classes, real Effigy callback receipts, full-debug/source retention
lookup, and useful warm benefit remain unqualified. This task does not activate
or contact that consumer.

The registered `RunnerHost` calls the graph-backed `Runner::run`. Its
`ExecutionOwner` callbacks are behind registration, input audit, reuse checks,
and the pending-map barrier. The project `ProjectAdapter` and
`CargoWorkflowRunner` are fixtures; the latter executes a bounded local
qualification command and is not the genuine Effigy host path. The private
qualified runner is not consumer or production authority. None of these
surfaces is a ready-made ordinary-Cargo Nucleus route.

Nucleus already has a durable `StorageAttempt` ledger and the reviewed
`validation-client.ts` submission path. Its opt-in transport exercises a
synthetic child channel; production registration remains disabled. There is no
current Nucleus lookup wire operation for Oxide acquire/finalize recovery and
the callback schema below is not installed in Nucleus. Do not have TypeScript
implement the Oxide storage codec or settlement evidence, call `Runner`
without its required evidence, or submit directly through a local native Cargo
runner.

## Cargo freshness responsibility

For a narrowly versioned workflow, ordinary pinned Cargo decides whether its
units are fresh. The consumer and project owner must accept the following
cooperative conditions before warm reuse is eligible:

- One clean, committed source tree per attempt, with exact commit, tree,
  content digest, submodule/object availability, and a consumer-owned physical
  root ID and allocation epoch. Dirty, ignored, generated, submodule, or
  otherwise unretained inputs are ineligible unless their exact bytes and
  behavior role are covered by the same reviewed source contract.
- An accepted, pinned Cargo/rustc/sysroot/host-target and native tool/SDK
  installation epoch. Its owner guarantees no replacement throughout each
  lease and attempt. A tool, SDK, linker, sysroot, or accepted installation
  change invalidates the entire compatibility partition. A version string,
  path, executable hash, or caller assertion alone does not establish the
  epoch or no-replacement guarantee.
- Exact selector and argv arrays, working directory, Cargo configuration,
  lockfile and manifest identity, profile, feature selection, target, and
  environment **including absent variables**. Any change to a
  behavior-affecting value must either select a different cold partition or
  make warm reuse refuse. Never silently inherit an ambient environment.
- A project behavior declaration that its build scripts, proc macros, native
  helpers, dependency code, and generated outputs express their inputs through
  Cargo's supported freshness contract. Relevant file reads, environment
  reads, feature and profile choices, tool inputs, and generated-file
  dependencies must be declared so Cargo invalidates affected units.

The final item is a bounded cooperative assumption, not an Oxide proof about an
arbitrary Cargo graph. Registry archive checksums do not establish macro,
build-script, native-helper, C/header, SDK, or generated-output behavior.
Dep-info, a local clean status, mtime checks, successful tests, a package
archive hash, an executable version, or consumer-supplied booleans cannot fill
those gaps. These unknown inputs include dependency and tool behavior, not
only edited local files.

Warm reuse therefore requires a qualified **complete supported invalidation
boundary for this workflow and its pinned installation epoch**. Qualification
must demonstrate the accepted project declarations and Cargo behavior for
source, configuration, feature, toolchain/install-epoch, and environment
changes. It must also exercise the project's relevant build-script,
proc-macro, native-helper, and generated-output classes under those
declarations. This is not a general threat-model claim or a new authenticity
receipt. A missing class or unqualified input makes the whole affected
compatibility partition cold-only; if Oxide cannot direct Cargo to an empty
generation already within the two-partition/two-generation allowance without
destroying retained or uncertain bytes, cold execution is refused until an
owner-authorized recovery path exists. Warm use is refused in either case. Do
not call cold-only storage a reuse benefit.

Cargo's ordinary freshness is separate from run validity. Every submitted
attempt invokes the exact Effigy selector that includes the required tests;
reuse never skips tests or changes their result. A compile failure before the
test phase is a failed invocation, not a successful run with passing tests. A
passing test is not evidence that unknown inputs were fresh. Exact
task/run/source provenance and the test result are retained independently
from cache eligibility.

## Implemented API and attempt lifecycle

The private Rust API is feature-gated by `private-qualification` and consists
of `OrdinaryCargoAttemptRunner::new(Client, storage_root, parent_endpoint,
CallbackAuthenticator, budget)`, `OrdinaryCargoAttemptRunner::new_with_capture_budget`,
`run_attempt(runner, invocation)`, and `serve_bootstrap(path)`. The bounded
`OrdinaryCargoInvocation` contains the
ordinary `AcquireRequest`; physical root and allocation epoch; project, task,
workspace, branch, run and attempt IDs; commit/tree/content digests; accepted
policy version, reference and installation epoch; selector and argv arrays;
cwd; the complete environment map with explicit presence/absence; private
target/temp/capture paths; expected output roles; and reserved submit, capture,
not-submitted and finalize IDs. The embedder installs the authenticator and
process endpoints; task JSON and environment cannot install trusted policy or
issuer material. The API does not accept a `Registration` or fabricate
`UnitEvidence`.

The original runner constructor remains version 1 and preserves its 16 MiB
capture cap. Version 2 uses the storage API's stable eligibility reference
while every exact source/run/command/output path and capture budget remains in
that attempt's separately authenticated input boundary. A v2 request needs
both the existing exact-request admission and the stable-policy verification;
the protected owner's ordinary request digest allowlist contains the complete
request digest, including its stable reference and exact boundary. See the
[ordinary-Cargo reuse identity contract](ordinary-cargo-reuse-identity.md).

The route uses the existing identity, lease, lookup, and finalize semantics
with the shipped ordinary-Cargo storage API above (not generic
`Client::acquire`):

1. Check the supported invocation shape and exact source/root bindings before
   acquire or any writer starts. No generation-scoped barrier can be written
   yet because no generation is known.
2. Call `Client::acquire_ordinary_cargo` with the authenticated original
   request, `StrictPhysicalCargoRoot` identity, and validated partition. It
   applies the per-generation state policy above and never evicts. On an
   ambiguous transport outcome, use `Client::lookup_ordinary_cargo` with the
   exact original authenticated request. `Absent` permits only exact replay.
   Busy/capacity returns pending without a host run. `Uncertain`, unknown
   accounting, lost token, changed body, or inconclusive lookup holds storage
   and the consumer root.
3. The acquire transaction has committed the pending barrier against the
   returned lease/generation before returning a leased result with path, token,
   storage instance, slot, generation, and reuse result. Verify that ledger
   record before forming the final launch digest from project, task,
   workspace, branch, run, attempt, root ID and allocation epoch; source
   commit/tree/content; policy and installation epoch; selector, argv, cwd,
   canonical source/target/temp paths, complete environment-presence map; and
   the acquired storage instance, slot, generation, lease, canonical slot
   path, leased state and daemon-returned `reused` value. Do not include the
   lease token; keep it in host memory and bind its digest only where the
   existing protocol requires it. If the barrier is absent or does not match
   the acquired tuple, do not submit. A `NotSpawned` settlement is permitted
   only if the authenticated acquire record still supplies the exact saved
   pre-acquire state and the consumer proves no writer began; otherwise hold
   the lease and root for recovery.
4. Send that immutable digest and command to exactly one
   `ordinary_cargo.submit` callback. The callback must echo the digest, persist
   the complete acquired tuple in the existing `StorageAttempt`, then call the
   existing Effigy 010 submission path and return its genuine scheduler run
   identity. No host token or Cargo process exists for a busy attempt, and the
   callback cannot change the command after acquisition.
5. Hold the lease over the complete Cargo process, all selected tests, and
   capture. The consumer returns its authenticated terminal and capture
   evidence, including the complete retained source/output/debug closure and
   full-debug lookup reference when required. Rust binds it to the acquired
   tuple and exact run/source attempt; Rust does not manufacture a successful
   test or capture record.
6. For a started run, only after cooperative no-writer completion and complete
   capture may the host call `Client::finalize_ordinary_cargo` with authenticated
   terminal/capture evidence. A proven no-submit result from
   `ordinary_cargo.not_submitted` uses
   the same operation with an authenticated `NotSpawned` settlement: it is
   accepted only when Effigy proves submission never began, and atomically
   restores the exact pre-acquire `empty` or `reusable` state saved in the
   pending marker before releasing the lease. No capture is fabricated and no
   bytes are cleared. On a lost finalize reply, call
   `Client::lookup_finalize_ordinary_cargo` with the original ordinary request,
   same lease, operation ID, and exact
   evidence before any identical retry. `Present` observes the stored
   transition, `Absent` permits exact replay, and `Uncertain` keeps the lease
   held. A lost submit reply, cancellation, or ambiguous writer state is not
   `NotSpawned` and leaves the marker pending until positive no-writer closure.
   Releasing the storage lease does not recycle the consumer's physical root.

The implemented private process schema is one versioned
`ordinary-cargo-attempt-v1` operation on a private attempt channel, with three
callback messages that have no `Registration` argument:

- `ordinary_cargo.submit(launch_digest, acquired_tuple, command)` returns the
  genuine Effigy scheduler run identity and the echoed launch digest;
- `ordinary_cargo.capture(run_identity, launch_digest, expected_outputs)`
  returns the authenticated terminal, completion and complete capture claims
  for that exact run and source; Rust verifies them against the acquired lease
  before building existing settlement evidence;
- `ordinary_cargo.not_submitted(launch_digest)` returns authenticated
  no-writer evidence only when the consumer proves the Effigy submission never
  began.

These messages are deliberately separate from `ExecutionOwner`'s
`execution.execute`, `execution.verify_success`, `execution.finalize_idle`,
and `execution.finalize_not_spawned` methods: those callbacks are invoked by
the registered Runner and their success-verification signature requires a
`Registration`. The ordinary-Cargo route has no `Registration` and must not
pretend it does. Nucleus must install this exact schema in
`oxide-runner-transport.ts` and its existing `StorageAttempt`; current fixture
hooks are not that installation. The Nucleus adapter must preserve original
request digests and reserved operation IDs before delivery, persist acquired
ownership before acknowledging it, and keep credentials/token material out of
ordinary logs and ledgers. Effigy 010 remains the only host submission and
settlement route.

The callback contract distinguishes a normally completed test failure from
lost or incomplete execution state. A completed failure may be settled only
with complete authenticated terminal/capture evidence; it makes the generation
cold-only. A compile failure before tests is a failed invocation, never a
successful validation. Cancellation, callback loss, output overflow, timeout,
or unknown writer state holds the lease unless the existing process owner
provides positive authenticated no-writer closure. PID disappearance, age,
reply loss, or a file marker is not closure. Do not change the running clock,
detach a writer, delete the generation, or silently retry with a new attempt.
The compiled fixtures use synthetic receipts and do not prove genuine scheduler
or retention provenance.

The durable pending barrier cannot be a permanent boolean that blocks every
future cold attempt. Its states distinguish `pending`, `reusable` after
successful tests and complete capture, `cold-only` after an authenticated
settled failure or invalidation, and the saved pre-acquire `empty` or
`reusable` state restored by authenticated `NotSpawned` finalization. An
uncertain writer remains `pending` and unavailable. The no-spawn transition
releases the barrier without spending another generation, so the exact same
partition can admit its next cold attempt from `empty` or reuse attempt from
`reusable`. A cold-only generation is never served as warm. A separately
approved consumer recovery disposition is required before it can be retired
or reset; neither ordinary acquire nor this task's real proof may delete,
rewrite, or evict failed, retained, shared, or uncertain generations. Until
that disposition exists, the state-aware acquire returns capacity
pending/refusal when the two-generation owner cap provides no eligible
generation. This is an explicit capacity stop with no active lease barrier,
not silent pool growth or a claim that failed bytes became reusable.

The existing registered host has a 30-second callback timeout and a
five-minute session timeout; its 45-minute qualification ceiling is feature
scoped. Do not assume those bounds fit the real selector or widen them
globally. Before a real invocation, the consumer owner must show that the
existing Effigy submit path can return/retain the exact run identity, hold the
lease through process completion and capture, and settle within the one
remaining monotonic deadline. Keep individual callback replies within their
reviewed bound. If the real `validation-client` call blocks past the callback
limit, if the session cannot remain live through capture, or if cancellation
cannot positively fence and join the writer, the route is a finite NO-GO until
the lifecycle mapping is reviewed. It does not justify a second scheduler or
resetting a clock.

Before a later attempt changes the physical source root, the consumer first
archives that run's exact source and complete outputs. Capture must include
Cargo/test logs and results, source/config/tool identities, executables,
full-debug companions and UUID/build-ID, and source-path lookup against the
retained old source bytes. The consumer acknowledges that the old run can be
looked up independently of the current contents at the same physical path.
Failure to retain or retrieve any required output leaves root recycling
pending. This design itself never recycles the task root to another task.

## Private wire packet and consumer handoff

The state-aware acquisition, per-generation reuse metadata, strict physical
root identity, two-partition bound, authenticated transitions, lookup surfaces,
and generic-operation protections described above are the shipped Oxide seam.
The callable private route and this packet are pinned by the reviewed Oxide
commit. Callable means the private qualification binary compiles and passes
synthetic fixtures. It is not qualified: no real Cargo run, real input
freshness, full-debug lookup, accepted installation policy, consumer adoption,
storage relief, or SSD-write evidence exists yet. Use this section with the
Rust definitions and cross-language fixtures
`fixtures/ordinary-cargo-auth-v1.json` and
`fixtures/ordinary-cargo-auth-v2.json`; each route version has its own wire
shape and authentication domain.

### Process bootstrap and version pins

The unpublished binary `oxide-ordinary-cargo-attempt` is compiled only with
`private-qualification`. Its sole argument is the protected bootstrap path. The
process exits 0 for authenticated finalization, 1 for a storage refusal or for
bootstrap/input validation failure returned before any acquire call, and 2 for
`Held`. `Held` is conservative uncertainty, not proof of ownership. It covers
an acquired or uncertain lease, any post-acquire local, protocol, or callback
error, and a deadline that expires before the acquire result is known. Exit 2
can therefore occur with no lease. It is never reported as an unleased refusal.
Process exit, a ready or stopped state, or PID exit alone implies neither
ownership nor release; only authenticated storage or settlement evidence may
change ownership. It writes no credentials to stdout or stderr. The bootstrap is a duplicate-key-rejecting JSON object with
exactly these fields in version 1: `version` (1), `operation`
(`ordinary-cargo-attempt-v1`), `scope`, `remaining_ms`,
`callback_timeout_ms`, `storage`, `parent`, `callback_key_hex`,
`callback_context`, and `invocation`. Version 2 uses operation
`ordinary-cargo-attempt-v2` and adds the owner-issued `capture_budget` field at
the bootstrap top level. A legacy bootstrap omits that field; explicit null is
not treated as absence. Mixed or unsupported versions refuse. Endpoint
objects contain `path`, `device`, and `inode`; device/inode and byte/duration
quantities use lossless `WireU64` encoding (safe JSON integers as numbers and
larger values as canonical decimal strings). The file is owner-only, regular,
single-link and at most 64 KiB; the canonical input-boundary JSON is at most
48 KiB. The file is consumed once; endpoint path/device/inode identities are
rechecked. A durable private marker is synced before the bootstrap is removed.

The `invocation` object contains exactly `acquire` (`envelope`, `acquire_id`,
`required_bytes`), `root` (`source_root`, `consumer_root_id`, `allocation_epoch`),
`partition` (`ordinary-test` or `full-debug`), `acceptance` (`policy_version`,
`installation_epoch`, `acceptance_reference`, `input_boundary_digest`,
`credential`), `branch_id`, `source_content_digest`, `selector`, `argv`,
`cwd`, `environment`, `target_dir`, `temp_dir`, `capture_dir`,
`expected_outputs`, and `operation_ids` (`submit`, `capture`, `not_submitted`,
`finalize`). Environment values are strings or `null` for explicit absence.
`expected_outputs` is a unique nonempty list selected from `source-archive`,
`cargo-log`, `test-result`, `binary` and `full-debug`. Version 1 captured role
entries must match that list exactly and total no more than 16 MiB; its
`MAX_CAPTURE_BYTES` cap and authentication semantics remain unchanged.

Version 2 installs `OrdinaryCargoCaptureBudget { version, total_bytes,
role_bytes }` in the protected bootstrap or trusted runner constructor. Its
version is 1; `total_bytes` is positive and no greater than the 2 GiB
per-attempt engineering maximum. `role_bytes` contains one positive finite
ceiling for every predeclared expected role, with no extra role, wildcard, or
unlimited entry. Every role ceiling is at most the total ceiling. Checked
u64 accumulation rejects per-role or aggregate overflow. Captured entries must
have unique roles matching the complete expected set and satisfy both their
role ceilings and the aggregate ceiling before publication. A missing, extra,
renamed, duplicate, mixed-version, changed, or over-budget receipt holds the
acquired generation; it cannot make that generation reusable.

The v2 policy is per-attempt input, not stable compatibility identity. Its
version, aggregate ceiling and role ceilings are included in the exact
input-boundary digest, exact owner-allowlisted request, acquired identity
tuple, launch digest, callback request and capture-budget digest echoed in the
authenticated capture reply. Finalize repeats the original request and its
credentials. A changed budget requires a new signed boundary and owner
allowlist entry. Stable reuse still uses the separate storage policy-v2
reference, which excludes per-run capture paths and the budget.

The 2 GiB value is only an engineering maximum, not a selected real-trial
ceiling or evidence that artifacts fit. The observed planning binary size of
88,595,944 bytes exceeds the unchanged v1 cap; the return did not include its
source/path/hash, and full-debug, other binaries, source, copy overhead and
aggregate retention remain unmeasured. The synthetic v2 fixture represents
that size only as authenticated receipt metadata: no large or sparse file is
created, read, hashed, copied or retained by the child. The proposed 2 GiB
per-attempt-record retained-evidence figure in the Nucleus packet is not an
installed allowance and does not establish fit. The child validates receipt
fields only; Nucleus remains responsible for complete file enumeration,
hashing, copy, custody and genuine evidence. The 1 MiB frame, 16 MiB aggregate
callback traffic, three-exchange, 64 KiB bootstrap and 48 KiB boundary limits
are unchanged.
The owner computes the source-content digest from its accepted source
inventory and binds it alongside the exact source commit/tree and command
boundary; this route does not transfer or independently hash source bytes. Its
framing is the same byte encoding as
`oxide_storage::registered::cargo_unit_graph::framed_source_content_digest`, without invoking
the registered Runner or its graph audit: the consumer's accepted source
inventory maps each relative file path to `sha256:` plus the lowercase hex
SHA-256 of that file's exact bytes. Path admissibility is enforced by the
`framed_source_content_digest` helper before the route receives the digest, with
the same rules as its `validate_relative_path`. A path is admissible only if it
is a nonempty UTF-8 string with no NUL byte, does not start with `/`, and has no
empty, `.`, or `..` component. Every component must be a normal name. Its UTF-8
byte length must not exceed `path_bytes`, its component count must not exceed
`depth`, and the owner-supplied entry count must not exceed `associations`.
Each entry's framed bytes must fit `bytes`. Violations refuse as
`InvalidSourcePath` or `BudgetExceeded`. Each digest must be `sha256:` followed
by exactly 64 lowercase hex characters. The current defaults are 2048 path
bytes, depth 32, 32768 entries, and 8 MiB framed bytes. The path rules are the
input-boundary owner's precondition; the attempt route never sees paths and
cannot check them. Sort entries by the UTF-8 path bytes, which is the order of
Rust's `String` map keys; duplicate names are impossible in that map. For each
entry, append to one SHA-256 input stream the path byte length as an
unsigned 64-bit little-endian integer, the path bytes, the digest-string byte
length in the same format, then the ASCII digest string. The result is
`sha256:` plus lowercase hex of that stream's SHA-256. There is no entry count,
delimiter, terminator, absolute root path, or directory record in the stream;
Git commit and tree IDs remain separate fields. The accepted input boundary
owns the exact source inventory and completeness claim. The route receives
this digest, validates its syntax, and binds it to the attempt; it does not
read or hash source files.

Attempt callback framing is a 4-byte big-endian length followed by UTF-8 JSON.
The frame cap is 1 MiB, aggregate cap 16 MiB, and the route allows at most
three exchanges. Unknown fields, duplicate JSON keys, malformed/trailing JSON,
extra operations, and invalid version/method/type refuse. `MAX_CAPTURE_BYTES`
remains 16 MiB for route version 1; route version 2 uses its owner-installed
budget only for artifact receipt accounting. These private attempt limits are
independent of the storage transport: `OXSTOR01` storage wire version is 2,
and durable `OXIDEST1` ledger version is 4. Private attempt operations are
explicitly versioned 1 and 2. The capture-budget schema, storage-runtime
bootstrap, readiness schema, and runtime control protocol are version 1; the
storage runtime bootstrap and readiness carry independent
`storage_protocol: 2` and `control_protocol: 1` fields. The runtime policy
digest still binds the wire versions and exact ordinary-request digest
allowlist; the exact request digest includes the v2 stable reference and
capture-budget input boundary. Old storage peers/ledgers refuse without
reinterpretation or migration.

### Canonical receipts and installed policy

Canonical subjects use the existing Rust canonical JSON encoding (UTF-8,
recursively sorted object keys, no lossy number conversion) and SHA-256 with a
`sha256:` prefix. The ordinary request digest covers the existing admission
subject and acquire ID, decimal-text `required_bytes`, source/root ID/allocation
epoch, ordinary partition label, and the policy version, installation epoch,
acceptance reference and input-boundary digest. Admission signs the fields
`[scope, storage_instance_id, request_digest]` under
`oxide-ordinary-cargo-admission-v1`; the existing generic admission credential
is also required.

The version-1 input-boundary subject has exactly `binding`, `root` (`source_root`,
`root_id`, `allocation_epoch`), `ordinary_partition`, `cargo_partition`,
decimal-text `required_bytes`, `branch_id`, `source_content_digest`,
`policy_version`, `installation_epoch`, `acceptance_reference`, `selector`,
`argv`, `cwd`, `environment`, `target_dir`, `temp_dir`, `capture_dir` and
`expected_outputs`; its digest is canonical SHA-256. The ordinary request
digest is the canonical digest of an object with
`acquire` (`envelope` equal to the existing admission subject, `acquire_id`,
decimal-text `required_bytes`), `root` (`source_root`, `consumer_root_id`,
`allocation_epoch`), ordinary partition label, and `acceptance` (policy version,
installation epoch, acceptance reference and input-boundary digest). The
version-1 fixture pins both byte-level digests for representative values.
Version 2 adds one `capture_budget` object to that boundary; its exact schema
is `{version, total_bytes, role_bytes}`. The v2 fixture pins the changed
boundary/request digests and does not reinterpret the v1 vector.

The trusted runtime installs at most four exact ordinary request digests from
the protected `RuntimeBootstrap.ordinary_cargo_request_digests` list. Missing
or empty means ordinary issuance is disabled. The list participates in the
runtime policy digest alongside scope, storage/control protocol versions,
budget, stop grace and existing generic admission subjects. Generic
`QualificationAuthority` continues to reject ordinary issuance by default.
Only the protected owner may issue ordinary admission and settlement receipts;
invocation JSON, argv, environment and parent task data cannot add accepted
digests or keys.

For ordinary settlement, canonicalize
`{ordinary_request_digest, settlement: <existing settlement subject>}` and
sign under `oxide-ordinary-cargo-settlement-v1` with fields
`[scope, storage_instance_id, subject_digest]`. Each stored credential bundles
the existing generic HMAC tag and ordinary HMAC tag as
`hmac-bundle-v1:<generic-hex>:<ordinary-hex>`; the existing verifier checks
both, while the ledger retains only redacted credentials. The lease token stays
in Rust memory and is not included in callbacks, stdout, diagnostics, or the
ledger. Credential-bearing bootstrap/storage/callback traffic is confined to
the protected local process and owner-only sockets; credentials are not safe to
copy to ordinary logs or ledgers.

The version-1 callback context is the canonical digest of
`{scope, operation: "ordinary-cargo-attempt-v1", request_digest,
storage_protocol: 2}`. Version 2 uses the same fields with operation
`ordinary-cargo-attempt-v2`. Each callback reply signs, under
`oxide-owner-callback-v1`, the UTF-8 length-prefixed fields
`[context, method, operation_id, launch_digest, route_operation,
result_digest]`. `result_digest` is the canonical
digest of `{method, id, launch_digest, acquired_digest, command_digest,
payload}`. `MacKey` frames the domain and every UTF-8 field as a 4-byte
big-endian byte length followed by the field bytes; tags are
`hmac-sha256-v1:<lowercase-hex>`. Both fixtures contain the applicable
request-digest, callback-context, ordinary-admission, ordinary-settlement and
callback HMAC vectors checked by Rust and Node. The v1 vector remains a legacy
control; the v2 vector also pins canonical decimal-string encoding for large
`WireU64` values.

### Callback body and durable ordering

Every callback request carries `version`, `operation`, `method`, reserved `id`,
`launch_digest`, `acquired_digest`, `command_digest`, the complete acquired
identity tuple, command, source/run identity, and expected output roles.
The acquired tuple fields are `policy_version`, `installation_epoch`,
`acceptance_reference`, `input_boundary_digest`, `source_root`, `root_id`,
`allocation_epoch`, `partition`, `reuse_state`, `state`, `storage_instance_id`,
`slot_id`, `generation`, `lease_id`, `slot_path` and `reused`; it also repeats
consumer, project, component, workflow, `task_id`, `workspace_id`, `branch_id`,
`run_id`, `attempt_id`, commit, source-tree/content digests and
`cargo_partition`. Version 2 additionally carries the exact `capture_budget`
and its `capture_budget_digest`; version 1 omits both fields. Its digest is the
canonical digest of that object. The
command has exactly `selector`, `argv`,
`cwd`, `environment`, `target_dir`, `temp_dir` and `capture_dir`; its digest is
the canonical digest of that object. The launch digest is the canonical digest
of `operation`, request digest, branch, source-content digest, those command
fields, expected outputs, all reserved operation IDs and the acquired tuple.
Version 2 also includes `capture_budget` and `capture_budget_digest` in that
launch subject.
`environment` preserves all accepted entries and explicitly sets
`CARGO_BUILD_BUILD_DIR` to the acquired slot, `CARGO_TARGET_DIR` to
`target_dir`, and `TMPDIR`, `TMP` and `TEMP` to `temp_dir`.
These digests are stable for one invocation and are not regenerated on replay.

The callback `source` object contains `consumer`, `project`, `component`,
`workflow`, `task_id`, `workspace_id`, `branch_id`, `run_id`, `attempt_id`,
`commit`, `source_tree_digest` and `source_content_digest`. A full
`run_identity` repeats those fields and adds `host_run_id`. All are exact
strings; no identity is truncated or converted through a JavaScript number.
`submit` replies with `status` (`started` or `not_started`) and an optional full
run tuple. `capture` additionally carries `run_identity` and its reply payload
contains `run`, `outcome` (`passed`, `failed`, `cancelled`),
`terminal_credential`, `capture` (`reference`, `digest`, `host_run_id`, `run_id`,
`attempt_id`, `credential`, `outputs` of `{role,digest,bytes}`; version 2 also
echoes `capture_budget_digest`), `completion`
(`lease_id`, `slot_id`, `slot_path`, `temp_dir`, `capture_dir`, `workflow`,
workflow-definition digest, repository head, fixture-source digest,
`host_run_id`, `run_id`, `attempt_id`, process identities, free-space samples,
exit code, stdout/stderr digests, capture reference/digest), and
`settlement_credential`.
`not_submitted` carries an explicit `no_submit_claim` and must return
`status: not_submitted`, `submission_state: never_began`, `writer_state: none`,
`closure_reference`, exact full `run`/source tuple and
`settlement_credential`. Since no host run was submitted, `run.host_run_id`
must be the empty string; the other run/source identity fields exactly repeat
the invocation's consumer-owned tuple.
The process checks callback id/method, all three digests, full source/run tuple,
acquired lease/slot/path, terminal/exit code, capture completeness, output
limits (aggregate in v1; per-role and aggregate in v2), and no-writer fields.
HMAC validity alone is not proof of real Effigy completion or retention.

The Oxide process performs acquire, access-free exact lookup, and inventory
cross-check before it exposes the slot or emits `submit`. Busy/refusal creates
no callback. Before acknowledging `submit`, the Nucleus adapter must persist
the original credential-bearing acquire/finalize request identities, reserved
operation IDs and full acquired tuple in its existing `StorageAttempt`; then it
uses the existing `validation-client.ts` Effigy 010 host-submit path exactly
once. After the genuine host run terminates, the existing retention owner must
capture and durably retain exact source, Cargo/test output and required
full-debug evidence before `capture` is acknowledged. Oxide validates and
finalizes only after that callback. Nucleus must verify its genuine Effigy and
retention evidence before issuing accepted receipts; fixtures cannot supply
those receipts for actual host work.

The route has no parent lookup or cancel callback. A lost submit/capture reply,
process/session loss, timeout, cancellation, invalid callback, or unknown
writer remains pending; it never infers no-spawn from age, PID exit or reply
absence. An owner may use the storage API's access-free exact lookup during a
separately implemented recovery flow, but this process does not provide that
flow. Lost-session recovery is therefore hold-only at this seam.

The process captures its monotonic start before protected bootstrap read and
uses one nonrenewable `remaining_ms` window, bounded to 45 minutes. The
owner-issued `callback_timeout_ms` is positive and no larger than the
remaining window; each callback deadline is the earlier of that limit and the
attempt deadline. No route setting comes from task arguments. Startup, storage
operations, callback traffic and finalization share that deadline; storage
transport retains its existing per-call timeout. Expiry returns held/unknown
ownership and cannot synthesize no-spawn or finalize success. Expiry before
the acquire result is known also returns exit 2 with no lease implied. A
consumer must prove that its real submit/process/capture work fits the
owner-issued window before any real Cargo attempt is separately authorized.

Historical timing caveat: the reviewed fixtures moved from a 5-second callback
window and 6-second withheld reply (aeb41c1) to a 15-second window and
18-second withheld reply (665bbd4). The 25-second attempt window is unchanged.
These are fixture settings, not a production ceiling or reliability fix. An
independent aeb41c1 run held the pass case at Capture. Its cause is unresolved:
no retained trace exists, and the historical load condition is unknown. Do
not infer load reliability from later passing runs.

Synthetic compiled-process fixtures exercise pass/capture, settled failure,
positive no-submit, submit timeout, submit lost reply, full-tuple mismatch,
and the three capture-path controls below, plus command/root/epoch/environment
mutation, acquired-path substitution and secret redaction. The legacy suite
runs nine native child scenarios; the v2 control adds two child attempts
against one storage generation. Each child has a 30-second ceiling. The
parent-side trace for each child records phase labels, monotonic offsets, and
bounded reasons, capped at 64 KiB per scenario. The trace never records frame
bodies, keys, or environment values, and failures print it. The three legacy
capture-path cases are:

- expiry: `CaptureDelayed` withholds a valid reply past the callback window.
  The child must stay held at Capture, pending, with no release;
- receipt/body correspondence: `CaptureMismatch` returns a validly signed reply
  for a foreign operation ID. The child must refuse correspondence and hold;
- channel loss: `CaptureLost` writes a partial frame and closes. The child must
  hold at Capture.

The v2 control changes run/source identity and output paths between attempts,
and signs a changed exact boundary and capture-budget digest while retaining
the same stable eligibility reference. The second attempt reuses the same slot
and generation. A binary receipt represents 88,595,944 bytes without creating
or reading that file. Unit controls cover exact ceilings, one byte over,
aggregate and per-role overflow, missing and extra roles, and legacy 16 MiB
refusal; they do not establish actual artifact fit.

Output refusal is covered by in-process v1 and v2 controls, not by an extra
native refusal child. The child's exit line names only
the phase. Distinguishing among the several `Held(Capture)` returns inside a
single child needs a private diagnostic hook, which this fixture does not add.
The parent trace separates timing expiry from delivered-but-refused replies.
Capture-path timing is recorded in the trace. The on-time valid capture still
finalizes.

These prove the Rust process and synthetic storage/callback protocol only; they
do not establish genuine scheduler evidence, accepted installation policy, real
input freshness, full-debug lookup, consumer adoption, storage relief or SSD
writes.

The Nucleus owner must accept this exact handover after the Oxide change has
independent review, merge and Queue closeout, before a separately scoped
Nucleus-owned client task is dispatched. The existing `Nucleus#175` pin stays
on the older 046 route; this change does not amend or activate it. Later client follow-through
uses the version-2 bootstrap, callback and exact input-boundary packet through
one existing `StorageAttempt`/Effigy Contract 010 path and the reviewed root,
partition, and storage limits. It does not install a new scheduler or storage
codec, and it cannot activate a real Cargo workflow without the later joint
proof packet below.

That future Nucleus task is bounded by its own approved brief, one
`ordinary-cargo-attempt-v2` adapter with an owner-issued capture budget, one
fresh consumer root, and the four-attempt/45-minute ceiling in the proof
packet below. It may install the accepted callback schema only in the existing
`StorageAttempt`, `oxide-runner-transport.ts`, and `validation-client.ts`
path, route submission and settlement through Contract 010 and the existing
Effigy scheduler, and use the current `oxide-retention.ts` owner for source,
output, and debug capture. It must demonstrate bounded synthetic cases for
busy-before-host-run, persistence before submit, cancellation/unknown writer
hold, capture-before-finalize, source/debug lookup, and credential redaction.
The client cannot implement the Oxide codec or settlement rules, add a second
scheduler, exceed the proposed root/partition/generation and evidence ceilings
below, or activate a real workflow in this task. Nucleus, Queue/Effigy, and
retention owners must approve its fields, root policy, lifecycle, and evidence
handling in that task's brief. This design does not contact, prompt, or mutate
those owners. The existing registered Runner remains unchanged.

## Finite later proof packet

Real qualification is a separate gate and must not start from this design.
Before a new proof brief is dispatched, Oxide and Nucleus owners must jointly
check one packet containing the exact clean source commits and Git trees for
Oxide, Nucleus, Queue/Effigy integration, and the one source workspace; lockfile
digest; Cargo, rustc, sysroot, host/target and relevant native/SDK executable
identities; accepted no-replacement installation epoch; Effigy selector and
forwarded argv arrays; Cargo profile, features, configuration and complete
environment; and the distinct per-invocation task/run/attempt identities.
The Nucleus/Queue pins recorded by prior read-only inspection are only
candidates: Nucleus `c0c9617533a675bad035b50aefe2fc6b6043f323` with tree
`1940a897676a82cd0dd5a7557d916391ada07eba`, and Queue
`7b9b840354a638a529ab023542903949b5b562ac` with tree
`244eec3284d272e6db8b056264b31cd25b459f77`. Re-pin clean final heads and
obtain owner approval in the checked packet; those inspection snapshots are
not execution authority. Oxide's exact implementation and API head must also
be reviewed and pinned there. Name the canonical clean source-root path, root
ID and allocation epoch; name the unique fresh APFS image path, volume/image
identity, ownership, available capacity, empty-state proof and cleanup owner.
The paths must be under that session's approved unique resource root; do not
reuse another task's worktree, image, or temporary directory.

The candidate workflow and toolchain from prior read-only inspection are
Cargo `1.97.1 (c980f4866 2026-06-30)` / rustc
`1.97.1 (8bab26f4f 2026-07-14)`, host = target
`aarch64-apple-darwin`, no target override, wrapper, remapping, or alternate
backend. The proof selector adds Cargo's structured output mode to the
existing test invocation so reuse can be observed without resolving a graph.
The candidate Effigy argv is:

```text
["effigy", "check:core:test", "--", "--message-format=json-render-diagnostics", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"]
```

At the inspected source, Effigy forwards this as
`cargo test --offline --message-format=json-render-diagnostics -p nucleus-core
--test planning --test build_diagnostics`; the ordinary profile inherits
dev's line-tables debug setting. The full-debug control appends
`--profile full-debug` after `build_diagnostics`, creating a separate
compatibility partition. This proof-only output argument must be accepted by
the Effigy and Nucleus owners and pinned with the selector; if it cannot be
passed through the reviewed 010 path, no warm-reuse proof runs.

The bounded input-class scope is only this one locked `nucleus-core` workflow
and its actual target/feature closure. Before launch, the Nucleus package and
dependency owners must provide a finite table of every package, version,
feature set, build script, proc macro, native helper, generated input/output,
compiler/linker/SDK, and environment dependency that can affect this
selector. Prior read-only inspection already identified candidate classes
that the table must address: bundled native SQLite via
`libsqlite3-sys 0.38.2` and `cc 1.4.7`; derive/proc-macro execution including
`serde_derive 1.0.229`, `schemars_derive 0.8.22`, `proc-macro2 1.0.107`,
`quote 1.0.47`, and locked `syn` versions; and any build scripts or generated
files in the local `nucleus-core`/`nucleus-observer` packages. These are
known candidates, not a complete active graph. Owners must classify the
actual pinned workflow, state the Cargo freshness declaration for each
behavioral input, and identify unsupported classes. A class without that
owner-reviewed contract makes the partition cold-only; Cargo's artifact
messages cannot qualify the input behavior themselves. As of this design,
these Nucleus classes have no accepted owner qualification, so a real Nucleus
warm-reuse proof is currently NO-GO. The bounded route may proceed to synthetic
implementation; it may not enter the conditional warm sequence below until
the class table is complete and accepted.

For output observation, the consumer captures the bounded JSON event stream
from each genuine Effigy run and parses only Cargo `compiler-artifact`
records (`package_id`, target, profile and `fresh`) plus the matching command
and run identity. This is observation of Cargo's reuse decision, not a graph
resolver, input receipt, or acceptance proof. The preapproved class table
names the expected package/target records. A cold-A event outside that table
stops the sequence before any warm attempt. For B, the changed local target
must report `fresh: false` and a predeclared unaffected target must report
`fresh: true`; passing tests alone proves correctness, not reuse. The full-
debug partition miss must report `fresh: false` for the comparable targets.
Preserve the raw bounded event stream with the exact run and source record.

These candidate pins, class table, output mode, and selectors must be
reverified against the exact clean source in the joint packet. No
`cargo metadata`, Cargo version command, graph probe, or unaccounted Cargo
invocation is allowed in that proof.

Owner commitments in that packet must include: Nucleus managed-Git/root
allocation and source/archive/reconstruction; StorageAttempt and task
admission; Queue host authority under frozen Contract 010; the existing Effigy
submit, cancellation, settlement and process-completion path; capture and
retention owners; the accepted cooperative Cargo/project behavior declaration
and installation no-replacement owner; the Nucleus package/dependency owners
for the complete class table above; and the Oxide storage owner. They must
confirm one physical root, sequential leases, no cross-root reuse, busy before
host submission, full capture before finalize, and old full-debug source
lookup before changing the physical root's source bytes. They must also
confirm the real selector fits the existing callback/session lifecycle and
one absolute 45-minute end-to-end proof deadline; the registered host's
feature-scoped qualification timeout is not an automatic grant for this route.

Proposed finite ceilings for that packet, not measured fit or an assumption
about current Nucleus size:

- one consumer-owned physical source root (R=1), at most two compatibility
  partitions (P=2: ordinary test and full-debug), and at most two generations
  total (R×P=2), one per partition, at 8 GiB each and 16 GiB maximum
  intermediate storage. This follows the Nucleus owner recommendation and the
  lane's one store per compatible partition; there is no third successor or
  implicit growth. If a settled failure/invalidation leaves a partition
  cold-only, stop that partition at capacity until a separately approved
  consumer recovery disposition exists. A proven no-spawn attempt restores
  its prior state and consumes no additional capacity. No eviction or recovery
  deletion occurs in this proof;
- one fresh task-owned disposable APFS image capped at 24 GiB total allocated
  footprint, including source, both private target/temp locations, all
  intermediates, live output/capture, and logs; all such writes stay inside
  that image;
- independently retained source and output/debug evidence capped at 8 GiB
  total, with at most 2 GiB per each of the four attempt records. Keep this
  outside the mutable root and image. No data is trimmed to make a later run
  fit;
- 32 GiB maximum combined footprint: the 24-GiB image cap plus the separate
  8-GiB retained-evidence cap above. The 16-GiB Oxide intermediate limit is
  inside the image and does not increase the owner's two-generation pool.
  Preserve the brief's 20-GiB host free-space reserve continuously, as a
  start/abort threshold, not a guarantee against other host writers;
- no fallback root or alternate cache (0 bytes), and recovery/deletion of an
  existing generation disabled for this proof. No unallocated successor is
  assumed or used;
- at most four Cargo invocations and 45 minutes total from preparation through
  source/debug lookup. Every invocation uses the exact selected test workflow
  and JSON output mode; there are no standalone Cargo metadata/version/probe
  calls, graph or nightly gates, or extra retry. Stop before a new invocation
  at either cap.

The current candidate Nucleus classes are not yet owner-qualified; therefore
the warm-reuse sequence below is a conditional plan, not a launchable proof.
If the class table is incomplete or lacks owner commitments, stop before
Cargo. A separately approved cold-only storage proof may show bounded storage
but cannot establish reuse or write reduction.

After the class table and output mode are accepted, candidate sequence:
(1) cold source A under the ordinary test profile; capture the event stream
and require every observed target to match the owner-approved class table
before proceeding. (2) Changed source B under the same profile; tests must
run, the changed local target must be `fresh: false`, and at least one
predeclared unaffected dependency target from the qualified class table must
be `fresh: true`. (3) Source B under full-debug as the whole-partition miss;
tests run and comparable targets must be `fresh: false`, with complete
full-debug capture. (4) Changed source C under that full-debug partition;
tests run, Cargo event records show changed-source invalidation plus
predeclared unaffected reuse, then a lookup proves source B's old full-debug
artifacts and paths still resolve to B's retained bytes. Each invocation gets
a new authenticated run/attempt and one genuine Effigy submission. Cargo
event evidence distinguishes reuse or a cold partition miss; exit status and
passing tests alone do not. The exact A/B/C source pins and bounded changes
must be named in the checked packet. If Cargo fails before executing tests,
any test fails, an observed class is outside the approved table, required
freshness events are missing, evidence is incomplete, or reuse/miss evidence
is ambiguous, stop the proof; preserve the image and records, mark the
generation cold-only or held as uncertainty requires, and make no benefit
claim.

Do not start if any owner has not committed to its boundary, if the complete
source/tool/config/input boundary cannot be qualified, if exact pins or clean
root/image resources are absent, or if old full-debug lookup cannot fit inside
the retention cap. At the ceilings, stop and preserve evidence. Do not delete
shared or uncertain generations, expand the deadline, reset an ended clock,
or silently increase a cap. Reaching the bounds without warm reuse can still
establish bounded cold storage, but it establishes no repeated-work, latency,
or SSD-write benefit. The existing pilot clock is unstarted and unchanged.
