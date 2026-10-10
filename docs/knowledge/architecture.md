# Build storage boundaries

These are the production ownership constraints agreed by Tom on 2026-10-09. The local daemon candidate described below and in [the prototype boundary](#local-storage-prototype) implements only a bounded core; it does not establish production deployment, portability, process containment or pilot authority.

## Storage and ownership

Persistent bounded cache slots partition by project/component identity, exact toolchain, host/target platform, and resolved configuration/profile/features/relevant environment. One exclusive durable lease covers compile through test and artifact capture. Unknown crash outcomes, lost replies, age and PID disappearance do not imply release. Reconciliation must prove safe ownership settlement before reuse or eviction.

Exact-run final binaries, logs, QA receipts and evidence remain independently owned and preserved. Cargo intermediates are internal and may contain test executables; separating output directories alone cannot prove isolation. Cache reuse never counts as validation success.

Prepare only requested components lazily. No eager copying of all targets and no full-copy fallback. Evict idle slots only, within explicit budgets; occupied space and actual disk writes must be measured separately. Inventory/acquire/reconcile/finalize/budget describe interface semantics, not an existing CLI.

## Qualification boundaries

Cargo's `build.build-dir` / `CARGO_BUILD_BUILD_DIR` is stable starting in Rust 1.91 and is separate from `target-dir`. It is the smallest useful mechanism to qualify: direct Cargo to an Oxide-owned intermediate directory and let Cargo retain its own unit invalidation. It supplies no Oxide ownership, whole-workflow lease, budget, or run-evidence semantics. The exact pinned toolchain and different-path portability still need a controlled qualification. If portability fails, use path-isolated partitions and report the reduced benefit. Do not blindly adopt sccache: Acow's measured trial rejected it.

Nucleus Contract002 prohibits mutable target sharing. An exclusive-intermediate design requires narrow qualification before adoption and cannot weaken that contract. Frozen Contract010 remains unchanged. Queue/Nucleus own admission, source and run identities, authenticated settlement, retention and artifact capture; Oxide owns only its bounded storage semantics. See [scope](vision.md).

## Preferred daemon hub to qualify

Tom's direction, 2026-10-09: a central daemon could normalize project workflows, own reusable storage and provide a consistent place to measure real-world use. It remains the preferred candidate, not a production deployment choice.

The consumer contract defines a minimal opt-in project adapter: resolve build identity, obtain consumer authorization, acquire a slot, execute the declared workflow, preserve/capture run outputs under consumer policy and finalize ownership with authenticated evidence. The Oxide crate now implements a fixture-only slice of that flow. Every actor touching an adopted slot must follow the lease contract; direct Cargo access cannot silently bypass it. Commands outside the adapter have no claim to Oxide-managed coverage.

The production candidate would own the storage ledger and observations keyed by project/component, source/run identity, resolved toolchain/configuration and backend. The current fixture adapter keeps a bounded in-process observation ring; it is not durable daemon telemetry. Observation is not authenticated run settlement or a validation verdict. Queue/Nucleus remain authoritative for admission, managed source/run identities, settlement and evidence/retention policy. Daemon failure/restart must reconcile durable leases conservatively; loss of connection cannot grant reuse. Consumer cancellation and process lifetime must have explicit owners.

Start with bounded local events for acquisition/refusal, timing, rebuild/reuse observations, storage growth and finalization. Account for measurement overhead and unavailable write attribution. General tracing, dashboards and experiment scheduling are not requirements for first relief. Ordinary-use telemetry can reveal contention and trends after adoption; controlled qualification is still needed for cache correctness and failure cases. Future experiments are explicit opt-ins, use isolated declared resources and do not silently alter everyday flags or source.

Keep storage/execution adapters replaceable and consumer-facing semantics stable as Cargo catches up. The design task specified daemon availability, project adoption and recovery semantics; it did not install or wire a service.

## Minimum safe cross-worktree reuse design

### Recommendation and alternatives

Prefer one opt-in, per-user local daemon as the storage hub for Queue, Nucleus and later developer workflows. It owns the slot directory, durable lease ledger, budget accounting and bounded observations. Each adapter supplies the effective build identity and the consumer-authenticated run envelope. The daemon has no selector scheduler, task admission, Git source management, run settlement, validation verdict, artifact-retention or capture-policy authority.

The minimal alternative is a per-invocation wrapper using a shared OS lock and durable journal. It can serialize one command and avoid a background service, but every wrapper must implement crash reconciliation and communicate through the same journal. A dead wrapper can release an OS lock while a compiler or test descendant is still alive; a journal alone cannot prove the slot is idle. Making this alternative safe therefore needs a durable process guardian and the same ledger/reconcile protocol. It remains viable if daemon lifecycle cost outweighs the value of a common hub, but it is not simpler for multiple consumers or normalized observations. Neither model is proven until the pilot's failure cases pass.

The daemon is a single local-user service, not a remote cache or a scheduler. Its data root is private to that user and on a filesystem whose locking, atomic replacement and durability behavior is qualified. A daemon-wide OS singleton lock prevents two service instances from owning the ledger. The slot ledger is durable and transitions are committed before replies. That singleton lock is not a slot lease: an in-flight slot stays unavailable after a daemon restart until reconciliation proves it safe. If the selected host cannot keep or establish proof of process containment after daemon failure, ambiguous slots remain quarantined and cannot be reused or evicted.

### Adoption and execution boundary

Adoption is explicit per project and per declared workflow/selector. The adapter resolves the project and component IDs, exact source identity, exact Cargo/rustc toolchain, host and target, profile, features, effective Cargo configuration and relevant environment before requesting storage. Secret values are never recorded; only an allowlisted effective input set or its digest is used. If a relevant input cannot be resolved, the adapter uses a private isolated build directory or refuses managed reuse.

Queue/Nucleus admission happens before `acquire`. An admitted consumer passes an authenticated envelope containing its project, workflow, task/workspace/run/attempt identity, source tree and admission reference. For developer workflows without a scheduler, the adapter creates a local run identity but does not claim Queue/Nucleus authority. The daemon validates the envelope or a consumer-issued capability, binds it to the lease and returns an opaque lease token. It never creates or changes the consumer's identity.

Under a lease, the adapter sets `CARGO_BUILD_BUILD_DIR` to the selected Oxide intermediate slot. It keeps `CARGO_TARGET_DIR` and captured outputs private to the checkout/run. The fixture prototype passes both Cargo directory overrides to an injected boundary but does not spawn Cargo. Direct Cargo access to the shared build directory is a bypass and is unsupported. An invocation that cannot reach the daemon may run only with a consumer-approved private isolated directory; otherwise it refuses. Private fallback and unmanaged commands make no Oxide reuse or observation claim.

Effigy continues to launch the declared language-neutral selector and own selector/process/env lifecycle. The project adapter carries the Rust-specific identity and lease context. Queue/Nucleus (or the local developer) owns the cancellation decision; the adapter passes it to the process guardian. Before release, the completion boundary must establish writing completion within the supported workflow contract below, together with exact-run terminal and capture evidence. The daemon's service lifetime does not define the workflow's lifetime.

**macOS completion gate:** the read-only [macOS readiness review](qualification-readiness.md#public-macos-fencing-candidates-reviewed) found no public mechanism proving release against arbitrary descendants, transferred handles and unrelated same-user writers. Tom subsequently approved the narrower supported workflow contract below. Oxide's unpublished native fixture boundary and six task-owned scenarios passed independent exact-head review within that fixed scope. The standalone Cargo pilot also qualified cooperative completion and release/reacquire for S1–S3 of its exact registered workflow; [qualification readiness](qualification-readiness.md) owns both receipts and limits. Broader Cargo workflows and actual consumer integration remain unqualified. Interrupted, cancelled, crashed, restarted or lost-reply runs remain `uncertain`. No permanent-held-session design is accepted.

### Supported writer model

Tom's ruling, 2026-10-09: initially support explicitly registered, trusted foreground workflows. Every participating command must wait for its writing children and close writing handles/mappings before declaring completion; it must not daemonize, transfer a slot write handle outside the owned workflow, or leave background slot writers. Workflows unable to satisfy that promise must use private storage or refuse managed execution.

Unrelated same-user processes and commands accessing managed storage outside the adapter are unsupported bypasses. Hidden violations of the workflow promise are not guaranteed detectable or fenced; this contract coordinates participating writers and supplies no OS security boundary against arbitrary same-user programs. Known leftover writers, unexpected process behavior, missing completion/capture evidence and ambiguous crash/cancel/restart/reply outcomes keep storage unavailable. Leader exit, PID absence, age and quiet scans alone do not establish completion.

Healthy release requires completion evidence bound to the registered supported workflow and exact attempt, an authenticated terminal record, independently bound capture evidence and the durable idle transition. Fixed fixtures and S1–S3 of the standalone Cargo pilot qualify only their limited cooperative boundaries. Broader Cargo/consumer workflow eligibility, cross-path reuse, production resource enforcement and adoption remain separately gated.

The native fixture boundary runs only a fixed test-harness entry point with an
environment cleared and `TMPDIR`, `TMP` and `TEMP` forced under the fresh task
scratch root. It holds the direct child handle, checks the child's PID and
per-spawn marker, waits for normal exit, and bounds fixture children, files,
file size and captured output before accepting completion. It records the
repository head, fixture source/definition digests, lease and slot paths,
host-run/run/attempt identities, process marker, exit result, free-space
samples, and stdout/stderr/capture digests. The consumer capture receipt must
match that exact run and local capture bundle; storage verifies a
lease-token-bound completion MAC before making the durable transition to idle.
The fixture protocol refuses an unknown free-space reading or less than 20 GiB,
samples before and after work and during waits at 30-second intervals, and
keeps its task-owned disposable footprint below 10 GiB. Those bounds cover the
fixed fixture construction only; they do not impose a filesystem quota or cap
arbitrary Cargo output.

This evidence means the registered, trusted foreground fixture honored its
promise to wait for its writing child and close its write handle before
completion. It does not discover or fence an unregistered same-user writer,
an undisclosed child, or a workflow that breaks the promise while hiding that
departure. Such behavior is an unsupported bypass, not a passed negative
control or an OS security boundary. A caller-supplied `FixtureCompletionProof`,
leader exit, quiet scan or capture receipt alone cannot mint native completion.
Cancellation, incomplete execution, missing/mismatched capture and ambiguous
daemon/client outcomes remain unavailable even if a known fixture child is
later reaped.

The first-use path should be one explicit project/workflow opt-in, with the adapter resolving Cargo settings and toolchain identity instead of asking users to edit global Cargo configuration. Explain a valid miss by category (source, configuration, toolchain, path partition or unknown) and show the effective non-secret identity. Under pressure, explain measured/reserved bytes, the cap/reserve, the active or uncertain owner and the safe next action; never suggest deleting evidence or forcibly clearing an uncertain slot. Reconnect should retry the same operation identity and reconcile when proof is available. If proof is unavailable, report that the slot is being held and why, then fail closed. These are ease-of-use acceptance criteria, not new user-facing command names.

### Identity and slot key

Use two identities for different purposes:

- **Compatibility partition key:** stable project ID and component/workflow ID; exact Cargo and rustc provenance; host and target triples; resolved profile, feature set and Cargo/rustc configuration; and relevant compiler, linker, SDK and environment inputs. Include an input in full or by a stable digest. Include checkout-path identity until different-path portability has passed. A failed path test requires path-isolated partitions.
- **Run binding:** consumer, task/workspace/run/attempt IDs when present, exact Git commit and source-tree state (including submodules and any permitted disposable source patch), resolved partition-key digest, and operation ID. A source revision is recorded per lease rather than automatically creating an unbounded partition; Cargo's own fingerprints must show that source changes invalidate affected units.

An unknown, changing or path-sensitive input is not silently omitted to improve hit rate. It forces a separate partition or a refusal. A change to toolchain or effective build configuration cannot use the old partition. The partition key is not a substitute for Cargo's dependency/build-unit invalidation.

Production keys remain checkout-isolated until different-path portability passes an operator-approved source-path contract. The experimental local adapter now resolves an explicit ordered map of authorized roots, binds each root ID to its canonical path, and includes the complete map and effective Rust-flag vector in its compatibility identity. A changed root set or flag vector creates a different partition. A disposable storage root also has its own identity bound to its declared canonical path. Mapped reuse refuses before slot resolution unless source, configuration and toolchain miss controls are complete. Logical mode requires Tom's accepted policy ID and full-debug intent; strict physical mode keeps each requested root in its own partition. These are adapter semantics, not Cargo portability evidence. The map does not normalize manifest, generated or runtime paths. The [design and pass predicates](qualification-readiness.md#portable-source-identity-proposal-design-only-2026-10-09) remain the qualification authority; [vision](vision.md#authority-boundaries) owns the ruling and experimental implementation scope.

The local run receipt keeps compile-origin root/tree identity separate from the requested tree and binds the ordered map, source-map policy digest, slot lease, private binary digests and capture digest. Qualification checks full-debug sections, compiler-debug, `file!()` and panic paths separately, then requires evidence for every in-scope manifest, include, build-script, `OUT_DIR`, generated, proc-macro and runtime-resource path. Missing or unresolved proof is insufficient; a mismatch fails. A map alone is not path-behavior evidence or debugger integration. Current fixture evidence exercises this logic only. The registered Cargo runner can pass the resolved flags and record map digests, but it does not yet provide the binary and path evidence required to qualify mapped reuse, so that run remains uncertain. No Cargo portability, adoption, performance or storage-saving result is established.

Queue's ad hoc worktrees normally have unique roots, so adding each new root changes that map and starts a cold partition. A stable checkout-path pool is a Queue-owned prerequisite: Queue would need to retain a finite set of worktree paths and assign later runs back to them under a separate Queue brief. No such pool is established. The only cited size, about 2.3 GiB for the planning+diagnostics artifacts, used the line-tables profile; the proposed full-debug profile's partition size is unmeasured. Old partitions occupy storage until safe eviction, and eviction discards their reuse value. Root churn therefore spends capacity on superseded and newly cold partitions without demonstrated reduction in duplicate work; measure the full-debug partition in an authorized pilot before claiming its size or savings. If Queue cannot maintain a stable pool, stop remapping for this workflow and state that worktree duplication remains unsolved. Keep consumer-facing source-identity, miss and refusal semantics independent of rustc flags and map encoding. Reassess at each supported Cargo/rustc upgrade. Retire remapping only when shipped Cargo support and a fresh exact-path qualification pass the same full-debug/source-lookup, behavior-path, source/config/toolchain miss, exact-run receipt, ownership and bounded-capacity gates without a per-root map. A roadmap is not enough. The portability proposal records the strict/logical pass predicates and limits; the accepted logical-path contract and fixture-tested qualifier do not establish portability or solve daily duplication.

### Semantic operations

These operation names describe a future protocol, not an existing command or API.

| Operation | Required behavior and retry identity | Refusal or result |
| --- | --- | --- |
| `inventory` | Read-only snapshot of configured cap/reserve, measured bytes, slot key, and `idle`, `leased`, `uncertain` or `evictable` state. A request ID correlates the read; safe retries may return a newer snapshot. | Report unknown size/state as unknown; never infer idle from missing metadata. |
| `acquire` | Require admission/authenticated run envelope first. Persist the lease and owner binding before returning its opaque token. Deduplicate on `(consumer, run_id, attempt_id, acquire_id)`; identical retry returns the same result, changed payload with the same ID is rejected. | Refuse a different claimant while leased/uncertain, an incompatible key, unresolved input, insufficient bounded capacity or failed admission. No automatic wait/retry that expands the operation. |
| `reconcile` | Reconcile one durable lease using a stable `(lease_id, reconcile_id)` and authenticated consumer evidence. Confirm completion within the supported writer model and capture state, not just a reported PID or terminal task state. Repeating identical evidence returns the same result. | If ownership, a known/unexplained writer, supported-workflow completion, capture or reply outcome is uncertain, keep the lease `uncertain`, reserve its bytes, and refuse reuse and eviction. Conflicting evidence is rejected. |
| `finalize` | Accept an authenticated terminal run record plus capture receipt/digests only after attempt-bound completion evidence establishes writing completion within the registered supported workflow. Deduplicate on `(lease_id, finalize_id)`; a repeated identical finalize returns the prior receipt. | Do not release on age, vanished PID, daemon crash, lost reply, cancellation request, or test outcome alone. Missing proof leaves the slot uncertain. A failed test may release only after safe writer completion and evidence capture; it never becomes a validation success. |
| `budget` | Read configured cap/reserve and current measured/reserved bytes using a caller request ID; safe retries may return a newer snapshot. Limit policy is supplied by explicit owner-approved configuration, not changed by a consumer. If a future authorized policy update exists, use stable `(policy_revision, update_id)` and reject changed payload under the same ID. | Refuse acquisition when a hard cap or declared guaranteed reserve cannot be enforced or would be exceeded. The owner-approved pilot's monitored host threshold follows the policy below; unknown/breached readings refuse. Evict only proven-idle slots and never captured run evidence. |

Each attempt has a new attempt ID; retries of the same logical operation reuse its operation ID and byte-identical payload. A new attempt cannot inherit an earlier lease. An identical acquire retry returns the original lease identity and current durable state; it never revives a finalized lease. The consumer adapter must single-flight execution for one attempt so a retry cannot launch a second workflow. A lease token is scoped to one run binding and one partition. Successful finalization is the only ordinary transition from `leased` to `idle`; restart recovery enters `uncertain` until reconciliation succeeds.

### Durable ownership and recovery

The ledger records the slot key, lease token hash, owner/run binding, acquire/finalize operation IDs, state, reserved/measured bytes and capture receipt reference. Durable transitions must be flushed before acknowledging them. The daemon serializes ledger mutation and atomically publishes completed metadata. A durable record is necessary but not sufficient: storage stays unavailable while a participating writer may still write or the supported workflow's completion remains unknown, even if the daemon or client disconnects. This is subject to the supported writer model, not a universal same-user write fence.

On daemon restart, every persisted `leased` record without a matching durable completion receipt becomes `uncertain`. The daemon queries the owning consumer for authenticated task/source/terminal/capture evidence and checks the workflow completion record. PID plus start-time or a lock-file state may help locate a job, but absence alone proves nothing. If completion/capture authority is unavailable or a known/unexplained writer persists, preserve the slot and refuse acquisition/eviction. Recovery may be retried with the same reconciliation ID; lost replies do not create new leases. Operator cleanup is outside this first protocol and must not be inferred from age.

### Local storage prototype

The unpublished `oxide-storage` crate contains a Unix-only daemon candidate with a versioned local request protocol and one serialized ledger writer. The endpoint is a Unix-domain socket beneath an existing canonical directory with private permissions; the socket is owner-only. A daemon-wide bound socket prevents a second live instance. Startup removes a stale socket only after a connection fails and the socket inode still matches. No service, launch agent or user installation is provided.

The ledger is atomically replaced through a private temporary file, file sync and root-directory sync before an operation reply. A 256-bit random lease token is SHA-256 hashed in the ledger. Consumer credentials are verified at the adapter boundary and stripped before persistence. If an acquire reply is lost while the daemon remains alive, the same run-attempt request returns its original lease. After daemon restart, every active lease becomes `uncertain`; retry returns no slot path. A client that lost the token can settle an uncertain lease only with authenticated reconciliation evidence.

Slots partition by project/component, workflow, Cargo and rustc provenance, host/target, profile, features, configuration and environment digests, and checkout-path identity. Source and run identity are recorded on the lease. An exclusive lease remains held until authenticated terminal, capture and completion evidence under the registered supported workflow is accepted. The current native fixture evidence applies only to its fixed cooperative child; it does not prove that arbitrary descendants or same-user writers ended. PID absence, age, socket ownership and daemon restart never release a slot. Eviction first persists an `evicting` state and only removes a known owned slot whose ledger state is idle; interrupted eviction resumes only that recorded deletion. Unknown root entries, corrupt records, unsafe paths or unknown accounting fail closed.

The prototype refuses a new attempt while any slot for its partition is leased or uncertain, even when the configured budget could hold another slot. A compatible partition permits sequential reuse only after authenticated settlement; it never permits parallel mutable sharing.

This remains a scratch-state prototype. It requires injected consumer evidence verification and a free-space probe; no Queue/Nucleus authenticator or host probe is included. Its cap accounts for measured slot bytes and requested reservations, but there is no filesystem quota to constrain growth while a client writes. Consumer-owned checkout and capture roots are outside its accounting. The storage core does not launch or contain build/test descendants, and the project adapter below does not spawn Cargo or prove path portability. No host filesystem is qualified. Real managed builds and adoption remain blocked.

### Project adapter fixture prototype

The Unix-only `ProjectAdapter` accepts an exact workflow/selector opt-in and
explicit resolved inputs. It validates Cargo 1.91 or newer, tool executable
digests, host/target, profile/features, complete allowlisted Cargo configuration
and environment entries, source identity and canonical paths. It hashes config
and environment values without logging them, and includes checkout-path identity
in the partition. Unknown or ambiguous inputs fail closed. This is a validation
boundary for injected fixture data, not an automatic Cargo/toolchain/config
scanner.

The adapter gives an injected host boundary `CARGO_BUILD_BUILD_DIR` for the
exclusive slot and a separate `CARGO_TARGET_DIR` for private final outputs. A
non-reserving inventory check may register one cancellable wait and consume one
wake; it never polls or loops. Consumer authorization supplies the run envelope
and acquire ID. The adapter acquires before host-run submission and reports a
busy/full refusal to the fixture consumer before any command runner is called.
Task/attempt identity and host-run identity are separate types. This split is a
fixture model only; it does not solve Q-012 or implement a Queue/Nucleus binding.

The unpublished implementation includes injected fake/lightweight runners and
a fixed native test-fixture runner; it does not launch Cargo or establish a
production workflow. An injected fixture completion proof cannot establish
native completion. The native runner may mint its private completion
certificate only for its owned child, exact attempt, slot and output bundle.
The adapter still requires a matching consumer capture receipt before
finalizing to idle. Cancellation, failed capture, mismatched run identity,
daemon uncertainty or a lost finalization reply without a confirmed retry leave
storage unavailable. It never starts a direct-Cargo fallback in managed state.

The adapter holds at most 12 fixed-schema events per run, each at most 1 KiB,
inside a 16 MiB in-process ring. It drops the oldest settled records first.
Records are keyed by consumer, run and attempt identity and retain workflow, selector,
commit, source-tree digest and partition/configuration digests. Events include
bounded timing, reuse/rebuild summaries when supplied, storage bytes, capture
references and finalization state. Write attribution is explicitly unavailable.
The ring is not persisted, is not broad tracing, and does not issue QA verdicts
or control consumer retention.

### Budget and observations

Capacity covers the dedicated disposable footprint, including checkout roots, intermediates, redirected temporary output and pilot run captures. The proposed pilot ceiling remains 10 GiB total. Before execution, read-only preflight must establish the measurement method and an enforceable disposable-footprint cap for the selected host and filesystem. Sampling alone does not establish that hard cap; an unknown or unenforceable cap blocks a build.

Tom's ruling, 2026-10-09: treat 20 GiB host free space as a monitored start/abort threshold, not a guarantee against other host writers. Refuse a start when measured free space is below the threshold or unknown; monitor during execution and abort/report on a detected breach or unavailable measurement. The execution brief must name the sampling interval, measurement and abort behavior, including overshoot and unrelated-writer races. This ruling does not authorize volume creation, weaken the separate disposable-footprint cap, change uncertain lease release, or release the pilot hold. Capacity pressure produces an actionable refusal and inventory. Do not delete active or uncertain storage or consumer-retained evidence to make room.

The semantic ceiling is one normalized run record and at most 12 fixed-schema events per run, each capped at 1 KiB. Events cover request/result, compile completion, test completion, capture completion, finalization and any uncertainty/reconciliation transitions. Repeated identical RPCs update a retry counter rather than append an event. Record durations, reuse/rebuild summaries, occupied/reserved bytes and available write counters; omit raw environment values, secrets, compiler command lines and per-file traces. Cap the observation ring at 16 MiB and drop oldest settled observations first. Lease ledger and consumer run evidence are outside this ring and cannot be evicted by it. The daemon is not a general tracing service.

Observations correlate through project/component ID, consumer task/workspace/run/attempt ID, host-run ID when submitted, workflow/selector, exact commit/source-tree digest, partition/configuration digest and event sequence. They explain storage behavior but are not authenticated settlement, a test verdict or a replacement for controlled correctness/failure qualification. If actual write attribution is unavailable, record it as unavailable; do not treat occupied-space deltas as disk-write counts or infer SSD lifetime.

### Exact-run evidence and Cargo boundary

Queue/Nucleus continue to preserve run outputs under their own policy. Each run's binaries, logs, test output, receipts and evidence live in its independently owned capture area, not in the reusable slot. The consumer receipt binds their digests to its authenticated run ID, exact source/tree, selector, toolchain/configuration and test result. The daemon may retain a receipt reference and digest but cannot mint the consumer's authenticated receipt. Cargo's intermediate directory is disposable internal state and may contain test-related executables; none of its contents alone establishes that a run's tests executed or passed.

Run `cargo test` and the selected Effigy tests on every build scenario. A cache hit skips compilation only; it cannot skip tests, create a successful receipt or satisfy validation. Target-directory separation alone does not reuse compiler intermediates across worktrees. Sharing `CARGO_TARGET_DIR` would also mix final outputs and cleanup scope, violating the mutable-target boundary. The replaceable initial mechanism is stable Cargo build-dir redirection plus Oxide's exclusive lease, project adapter, ledger and budget—not a second Cargo fingerprint engine, eager directory copier, full-copy fallback or `sccache` assumption.

Retire or replace the redirection mechanism only when an actually supported Cargo release supplies the needed cross-workspace unit reuse and lifecycle behavior for the selected workflows, including build scripts/proc macros as required, and controlled tests show the same ownership, path correctness, capacity and evidence guarantees. Keep the adapter, authority boundaries, run binding, budgets and consumer API semantics independent of that mechanism. Reassess at each supported Cargo upgrade; upstream roadmap acceptance alone is not a replacement trigger.

Experimental state, 2026-10-09: the standalone pilot's registered Cargo runner keeps the Cargo build directory in the slot and binds per-run capture under the exact run. Test binaries land under the slot's build directory, not in the final target directory. A warm checkout-B run on the same slot executed tests with zero rebuilt units, yet its binaries still embedded checkout-A absolute paths. Partition identity is checkout-isolated in production. The trial-only key may omit checkout identity for declared disposable roots, but that is a qualification-only key. Portability is not qualified; see [qualification readiness](qualification-readiness.md#pilot-execution-record-run-5-2026-10-09-partial).

### Original standalone Cargo pilot protocol (oxide#005, closed)

The original oxide#005 brief selected one native Nucleus core-test Effigy selector, one exact pinned head in two disposable checkout paths, and one exclusive intermediate slot. Its partial execution and reviewed evidence are recorded in [qualification readiness](qualification-readiness.md#pilot-execution-record-run-5-2026-10-09-partial). This historical protocol is closed. The separate portability proposal is documented there; it has no implementation or experiment authority.

Tom separately approved the exact standalone oxide#005 protocol recorded in [vision](vision.md#authority-boundaries) and its Queue brief. It used a local qualification binding and preceded consumer integration; it did not solve Q-012 or authorize Queue/Nucleus authenticated settlement, live adoption or a deployed service. That approval ended with the partial run and does not carry forward.

The macOS completion gate was later qualified for S1–S3 under the cooperative workflow. Run 5 completed healthy release and reacquire for its slot runs, but this does not qualify broader Cargo process trees. Cross-path correctness and source/configuration/toolchain invalidation remain unqualified; see [the reviewed result](qualification-readiness.md#pilot-execution-record-run-5-2026-10-09-partial).

Any new experiment requires a separate Queue brief, fresh source-owner approval, exact selector/source/toolchain/paths, new task-owned resources, and a renewed time and storage scope. Consumer adoption remains a separate decision after independent evidence review.

Worker scenarios are capped at six, with 90 minutes for the worker and 30 minutes for independent reviewer replay, two hours total. No automatic retries or expansion. Stops are the worker/reviewer time limits, the 10 GiB total disposable cap and a detected breach of the monitored 20 GiB host-free-space threshold. The host threshold is not a continuous guarantee; any separate cumulative-write limit must be named in the execution approval. Suggested scenarios:

1. isolated cold reference in a private build directory;
2. shared-slot cold run at checkout A;
3. same-head warm run at checkout B;
4. controlled tracked-source change in one disposable checkout, with a recorded new tree digest and evidence that affected units rebuild;
5. effective configuration change, which must change the partition or be refused while incompatible state is present;
6. toolchain control using an already-installed alternative, or a key-separation/refusal proof without another compiler build.

Every scenario that actually builds runs the tests in that same lease. A configuration/toolchain scenario that only proves key separation/refusal does not claim a build result. Do not install a toolchain to fill the matrix. Measure elapsed time, independently sampled actual write bytes, and occupied/reserved space separately. Count the checkouts and captures within the cap. State unavailable write attribution plainly; never infer SSD lifetime. At each predeclared time, space or write threshold, stop and report rather than retry or expand.

Correctness/failure proofs within the same bounded session: generated-code and proc-macro behavior; path-sensitive build output at both roots; exact-head/source receipt and artifact binding; tests run on every real build; a simultaneous second claimant is refused without touching the slot; idempotent acquire/finalize retries after deliberately lost replies; daemon crash during an in-flight build with restart still refusing while any descendant/capture outcome is uncertain; and budget pressure evicts only a reconciled idle slot while active/uncertain state remains unavailable. Age, vanished PID or restart alone must not make a slot available. Independent reviewer replays at most one warm and one meaningful miss, within the 30-minute reviewer cap.

Disposition is one of: proceed to an adoption decision if all ownership/correctness gates pass and measured benefit is useful; path-isolated partitions with explicitly reduced benefit if cross-path correctness fails but isolated use is safe; or a named blocking failure and smallest next action. Path isolation does not meet the cross-worktree objective. Failed ownership or correctness blocks adoption. No outcome authorizes implementation or consumer rollout.

## Existing facilities and source context

These sources inform qualification; they are not Oxide implementation or portability proof:

- Acow [QA speed](../../../acowtancy/docs/knowledge/operations/267-qa-speed.md) and [Rust target seeder](../../../acowtancy/scripts/tasks/rust-target-seed.ts): eager Farmyard/Desktop/Silo clone-copy preparation with full-copy fallback. Desktop storage is unnecessary for root QA. This approach is not the proposed Oxide policy.
- Queue [fresh sibling checkouts](../../../paseo-northstar-queue/docs/knowledge/specs/028-fresh-sibling-checkouts.md) defers sibling build-cache sharing. Shared CARGO_HOME download sources and [guarded output cleanup](../../../paseo-northstar-queue/server/worktree-cleanup.ts) are potential integration context, not slot managers.
- Nucleus [core requirements](../../../nucleus/docs/knowledge/contracts/002-core-requirements.md) owns task isolation and build-write requirements.
- Effigy task-result fingerprint caches and container cache lifecycle are not a central Rust slot facility.

Existing Nucleus integration preparation, backup root locking/crash reconciliation, retention cleanup, managed Git worktrees and smaller debug artifacts have their own owners. Consult their Queue records when authorized coordination is needed; do not duplicate their authority or rewrite their work from Oxide.
