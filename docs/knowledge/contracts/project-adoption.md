# Project adoption boundary

For project-owned component-lazy preparation, use the
[optimization adoption policy](optimization-adoption.md). That improvement
requires no shared slot or consumer lifecycle integration.

This contract records the boundary between Oxide's storage facility and
consumer-owned integration. The proposed explicit command surface lives in
[workflow](workflow.md); the unpublished callable interface is the
[local lease CLI](local-lease.md). No production Queue/Nucleus binding,
consumer configuration key, service deployment or installation follows from
this document. Q-010/Q-011 hold the integration answers; Q-012 holds the
consumer-owned source-path and task-attempt gates.

## Recommendation

Tom's 2026-10-18 direction selects explicit `oxide` commands as the developer
front door ([vision](../vision.md#product-workflow-direction)). Consumer
adoption remains separate work with its own owner brief and exact workflow
qualification.

The earlier recommendation, a consumer-owned adapter around familiar commands
plus an explicit project/workflow opt-in, was retired with the protected route
on 2026-10-18 ([retirements](../retired.toml)). Its lasting rules carry over:

- Never point shared Cargo configuration or `CARGO_TARGET_DIR` at an Oxide
  slot. Relocate intermediates per invocation through Cargo's build-directory
  setting; keep final outputs, test output and cleanup scope private.
- Oxide adds no global shell, `PATH`, Cargo, editor or Effigy interception.
- Build reuse never skips the requested tests, and a cache hit is not
  validation success.
- An invocation that cannot establish its storage or inputs falls back to a
  private build or refuses before Cargo; neither claims reuse.

## Stable source-path lifecycle

The retired design proposed a finite consumer-owned pool of physical
task-worktree paths, reused sequentially after archive, with strict-physical
slot identity. It was never implemented as a consumer API. Queue Spec 028
keeps build outputs local to each task and shares only Cargo downloads and Git
objects; Nucleus Contract 002 keeps distinct task workspaces and branches.
Cross-path freshness and path-embedding limits are in
[qualification findings](../qualification-readiness.md#cargo-freshness).

## Readiness, admission and acquire ordering

Queue and Nucleus keep task admission, waiting, scheduling, host execution and
settlement authority. Any future consumer adapter must preserve those owners'
existing lifecycle and acquire a lease only under a separately approved
integration. Waiting must not reserve storage, and a busy race is the
consumer's to recover, normally by private cold fallback. No Oxide readiness
service, host scheduler, project registration or consumer credential is
provided.

## Current Nucleus integration boundary

Nucleus development and planning belong to Supervisor
`6585d6bf-a1aa-4877-91d6-a675b7ca6282`, following Tom's 2026-10-15 ownership
ruling in Nucleus Contract 001 (recorded at `5b34f80`). The former Sol planner
`cddd99af` has handed over and Tom has archived it; its assessments do not
commit Nucleus. Send future substantive coordination only to the Supervisor,
without prompting active workers.

On 2026-10-16 the Supervisor relayed Tom's replacement ruling: Nucleus
will **not** integrate #060 as reviewed. The signed policy-3 attempt and
consumer callback-service shape conflicts with the simplification reset.
The earlier plan to take that API after the chart port is superseded.
Nucleus requests a same-user local CLI and will own its dispatch/validation
calls, environment forwarding, closeout/cancellation release and private cold
fallback when storage is busy. The unpublished [local lease CLI](local-lease.md) is the Oxide-only
replacement described below. No consumer binding exists. Do not restore
the historical adapters or make the protected route a prerequisite. Cross-worktree
correctness remains a dependent qualification; this CLI is not Nucleus-ready.

### Lean local lease interface agreed for delivery

The implemented unpublished surface is `oxide-storage-lease` with `init`,
`lease --project <canonical-root> --workspace <checkout> --profile
<dev|full-debug> --holder <opaque-id>`, idempotent `release --lease <id>`,
`recover --lease <id>`, and `status`. Lease success returns bounded versioned
JSON containing a lease ID and exact environment; refusals distinguish busy
from invalid input or storage failure. Same-UID cooperative callers and
private owner-only local state replace signed attempts and callback authority.
A local OS lock and durable lease state serialize CLI mutations without a
resident service. A finished CLI process is not writer closure. Caller-provided
paths never authorize deletion outside Oxide-owned storage. Exact JSON, exit
codes, binary path and limits are owned by [local lease](local-lease.md).

On 2026-10-16 the Supervisor accepted these boundary clarifications and
required cross-worktree correctness qualification in this delivery:

- `release` is the same-user caller's assertion that every writer using this
  lease has ended, including cancellation descendants, and any outputs it
  needs beyond cache lifetime have been preserved. No signature, callback or
  validation-success receipt is required for this lean ownership assertion.
  Release never asserts tests passed. A repeated release is access-free and
  idempotent. TTL expiry or holder loss marks an abandoned lease; neither
  proves compiler descendants stopped. The Supervisor accepted holding such
  leases until an explicit owner `recover` operation asserts writer closure;
  automatic TTL-based reuse is not required. Nucleus owns its closeout/cancel
  retirement path and validation-process-group termination before release.
  On 2026-10-16 its owner reported the validation process-group correction
  `49e3272` live: normal exit and timeout terminate the owned group, and a
  regression covers a descendant retaining the output pipe. This resolves
  that identified consumer prerequisite by owner report; Oxide has not
  independently exercised the Nucleus deployment. It does not prove
  arbitrary detached-writer closure or qualify the pending Oxide CLI/reuse.
- Separate canonical project identity from the actual source checkout path
  through an explicit `--workspace <checkout>` argument. The Supervisor
  accepted it and requires useful registry-dependency reuse across different
  task/validation paths, including bundled SQLite. Path-isolated warm reuse
  provides insufficient benefit and is not an acceptable Nucleus handover.
  Include a focused real cross-worktree correctness qualification in this
  delivery: observe actual dependency reuse, run selected tests every time,
  and verify changed/older-or-equal-mtime local source cannot produce stale
  success when alternating checkout paths. Cargo's source-path/package-key
  behavior is a hypothesis to test, not an already qualified fact. Do not
  restore the old signed input-audit route to pursue this gate.
- Lease-time idle eviction is admission management; it cannot constrain
  writes after return. Reuse the verified bounded payload filesystem for an
  actual aggregate byte ceiling, with durable control outside it. Full writes
  fail normally without releasing a lease. Evict whole idle slots only; active
  or abandoned/uncertain slots remain protected. Directory-only mode, if
  offered, must say budget-only rather than claim a hard cap.

Nucleus explicitly needs no debug lookup after release. Released bytes are
therefore disposable under its chosen lean policy; no native debug retention
receipt is a prerequisite for that consumer handover.

Prefer shared intermediates with private final target output where the pinned
Cargo behavior supports that separation; returned variables must specify the
actual supported choice. On macOS unpacked debug information may reference
intermediate objects. Cache bytes are disposable after a valid release, so
callers needing later source/debug lookup must preserve the needed bytes first
or retain the lease. Do not claim independent debug retention from a copied
binary alone. Do not impose the old signed capture protocol on this lean
route. Existing private cold fallback must omit all acquired shared paths.

The first of those two deliveries is the unpublished local lease CLI with
bounded backing, admission and idle-only eviction, owned by
[local lease](local-lease.md). The focused qualification below establishes the narrow real cross-worktree
result; the [local CLI contract](local-lease.md) supplies its reviewed interface
and handover artifact. Nucleus owns integration of that interface. Broader
source/configuration/workflow guarantees are not implied. Reuse the existing bounded
filesystem and inventory implementation; old authentication/callback plumbing
is not required. This assessment creates no live consumer, host installation
or real experiment.

### Focused cross-worktree result

The focused 2026-10-16 cross-worktree result uses one shared `dev` slot
and pinned Nucleus planning/diagnostics workflow. Cold A rebuilt 73 units in
48.16s; different-path B rebuilt only its changed planning test (72 fresh,
including `libsqlite3-sys`) in 18.40s; changed A repeated that shape; the
unchanged rerun rebuilt nothing. Selected tests executed every time, including
the current marker/manifest-path test where introduced. Same-holder replay,
busy refusal and writer-closed releases were observed. Four Cargo calls were
used; no further experimental call is implied by the positive review.

[Evidence](../evidence/oxide-064-cross-path-qualification.json) preserves the
frozen pins/raw paths and the original driver's `go=false`. The driver's
judge misclassified recurring build-script JSON rows as process executions;
independent review accepted raw Cargo fresh/test receipts with that limitation
explicit. The reported 5.70 GB footprint conservatively counted image apparent
and allocated bytes twice; the union estimate is about 3.46 GB. Footprint and
host reserve were sampled at run boundaries, not continuously. The old/equal
mtime include files accompanied fresh Rust test edits, so input-only timestamp
invalidation was not isolated. No native `.a` hashes, slot symlink census,
full-debug live run or live eviction result was captured. This supports
consumer integration of the observed ordinary-Cargo workflow; it is not a
general source-path/invalidation, native-write, SSD-lifetime or production
activation claim. The [local CLI contract](local-lease.md) owns actual command,
JSON, exits, artifact path and recovery limits.

### First consumer use and APFS metadata correction

On 2026-10-16 Nucleus Supervisor reported consumer implementation `56a4c20`
pushed but not deployed. It leases at task placement and validation, uses a
Nucleus-owned parent `.cargo/config.toml` `[build] build-dir` to project the
returned intermediate directory into actual Cargo calls, and keeps final
outputs private. Sibling-repository tasks and busy/full/refused leases use
private cold fallback. The owner reports four client fixtures, 124 affected
file tests and an Effigy redirect smoke; these are consumer reports, not an
Oxide observation of deployed cache use.

The first real pool exposed a delayed-init defect: macOS added
`payload/.fseventsd/fseventsd-uuid`, and the original CLI refused every lease
as unknown accounting. Independently reviewed PR #65 (head `8941ae0`, merged
`34e6909`) fixes that lean-only payload check. Its native proof covers metadata
arriving after init; unknown entries, symlinked metadata and foreign-volume
entries still refuse. It also fixes safe idle-slot symlink reclamation without
following the target. The [local lease contract](local-lease.md#reviewed-local-handover-artifact)
owns the replacement binary/hash. Receipt v2, ledger v1, lease IDs, JSON and
exits are unchanged; the existing pool can reopen without re-init or migration.
Do not delete its metadata. The Supervisor subsequently reports the fixed
`9b0b0fc8…` binary installed, the same store `e449de33…` reopened with complete
coverage, and consumer deployment live. Its two-worktree live-pool smoke ran
`cargo check --offline -p nucleus-core --tests` through the parent Cargo config:
A leased slot 1 cold in 14.4 seconds; different-path B reused slot 1 in 9.6
seconds. Both leases released and both worktrees were removed. Slot usage grew
from 445 MB to 792 MB; the final idle unique-allocation report was 780 MB.
These are owner-reported smoke observations, not additional Oxide qualification
calls or attributable SSD-write measurements. No production worker had used it
when reported because the task board was empty.

Nucleus's temporary guard `75675d2` used private cold fallback once the compatible
slot exceeded half the cap. The owner requested Oxide-owned lease-time recycling
of oversized compatible idle slots, accepting a cold build. The replacement
[local CLI](local-lease.md#reviewed-local-handover-artifact) now supplies that
policy: at `ceil(cap_bytes / 2)` or above, complete unique-allocated accounting
triggers durable whole-idle-slot deletion and a fresh monotonic slot/lease;
below it warm reuse continues. No new flag or pool migration is required.
A threshold is headroom policy, not a bound on a single leased build: ENOSPC
remains possible and never releases ownership. Held,
uncertain or unknown-accounting occupancy must not be deleted to satisfy it.
The Supervisor reports the recycling binary installed, the guard removed in
`a6baf85`, and the adapter redeployed on the same 8 GiB pool. Ordinary
busy/full/refusal fallback remains. The first real task's lease, slot growth
and writer-closed release remain unobserved by this report; installation and
smoke do not establish a production-task lifecycle or build-size distribution.

Pool sizing remains a consumer/operator budget choice. A build can exceed the
remaining payload capacity during a lease, including on a fresh slot; there is
no automatic mid-build private fallback. The nominal cap is not an exact
whole-host disk-cost ceiling: image/control metadata and private target/temp
output are separate, as detailed in [local capacity](local-lease.md#capacity-and-eviction).
No larger pool, compaction, selective package cleaning or cache removal was
authorized or performed by Oxide in response to that sizing discussion.

The Supervisor reports consumer correction `a4e3ca0` deployed: release now
requires kernel `CloseoutStatus.retire_participants` to be `applied`. Its host
applies that obligation only after observing each participant absent through
`port.agent(id)`; validation's process group has exited before closeout. The
prior idle-agent release was a real defect, not an external guarantee. The
owner names a regression in `hosts/paseo/tests/build-cache.test.ts`: a lease is
released only on the retirement receipt, never on idle agents. This is an
owner deployment report; Oxide has not independently rerun consumer tests.

Pre-merge cancellation currently stops agents without deleting them and has
no closeout receipt. That occupancy stays held; subsequent same-project
acquisitions are busy and use private cold fallback. Nucleus logs the exact
`recover --lease <id>` once for its owner to run only after confirming all
writers ended and participants are gone. Idle/archived agents, PID, elapsed
time and the logged command itself do not establish closure. No automatic
recovery or Oxide-side cancellation cleanup is selected. Nucleus owns any
later cancellation-retirement correction.

The owner subsequently reports the optional capacity-diagnostic binary
installed with its hash verified, no adapter restart and unchanged consumer
code/lease/release behavior. On the existing live 8 GiB pool it reports
7.58 GB free, an image file on disk of 824 MB, and slot 1 idle at 780 MB from
the earlier smoke. These rounded units are preserved as reported, not an
independent raw-byte receipt or synchronized snapshot. Do not add image and
mounted-slot figures. That diagnostic check preceded the actual-task receipt
below; it did not by itself establish a production-task lifecycle.

### Observed ordinary task lifecycle

Nucleus Supervisor reports the first actual task on the lean facility:
Nucleus #252, kernel task 3763, PR #318, merged `daced47`, using the installed
`ddceaf50…` diagnostic binary. At workspace placement, before the worker
started, the task acquired a new lease over idle slot 1 (779 MB from the earlier
smoke); its build directory reached Cargo through the parent `.cargo/config.toml`.
The reported lease identity is abbreviated as `v1.e449de33….1.1.3`, not a
complete ID usable for recovery.

The reported held-slot samples, in MB, were
`779 → 887 → 1685 → 2097 → 2636 → 2353 → 2988` through worker and reviewer
builds, selected `check:core:history-bound`/state-checkpoint tests and clippy.
The largest observed held sample was 2988 MB; continuous peak measurement and
sampling intervals were not supplied. No ENOSPC or cold-fallback log lines
were reported. After closeout recorded `retire_participants: applied`, the
next reconcile tick released the lease. No active consumer lease remained;
Oxide retained slot 1 idle at 2988 MB unique allocated. The final capacity
report gave payload free 5371 MB and image logical size 3181 MB. Image logical
size is not host-allocated bytes; do not add these figures to slot allocation.

This owner receipt establishes actual placement-time acquisition, reuse,
held growth within the finite payload and retirement-receipt-gated release
for one task. Normal source, tests, review, final outputs and closeout remain
Nucleus-owned under the selected disposable-cache policy. It does not prove
per-build speedup, a workload size distribution, continuous resource maxima,
SSD-write savings, live eviction/cancellation recovery or post-release debug
lookup. Per-build warm/cold timings were not measured. The earlier independently
reviewed cross-path experiment remains the separate narrow reuse evidence.
Oxide did not rerun the consumer workload or inspect the live pool to record
this report.

### Historical protected-route references

Nucleus removed its never-live disabled Oxide consumer code in the 2026-10-15
simplification reset; Nucleus Contract 011 marks its later implementation
sections historical. Oxide retired its own protected route, declared workflow
entry point and registered runner bridge on 2026-10-18. Do not restore them as
an activation shortcut. A future consumer integration starts from the actual
reviewed handover and the current owners.

### Environment projection prerequisite

The read-only inspection of Effigy
`b8a6409a2d762c691fd3805f08244a1c9a245ece`,
`src/runner/host_scheduler/submit.rs`, found that its default forwarded runtime
environment includes `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL`, but excludes
`CARGO_BUILD_BUILD_DIR`, `TMPDIR`, `TMP` and `TEMP`. Explicit selected-task
environment references can participate in its existing language-neutral
projection. The inspected Nucleus `check:core:test` selector declares none of
those additional references. Setting only the outer process environment is
therefore insufficient to put Cargo intermediates and temporary output in the
admitted paths.

The consumer scope must establish the exact admitted projection through the
existing execution surface and bind its final values before submission.
Do not add Rust cache policy to Effigy or assume global forwarding. Final
values must reach the actual worker and validation commands; synthetic
callback tests do not establish that projection. This is a retained integration prerequisite, not authority
to change Effigy or Nucleus configuration now.
