# Vision and scope

## Purpose

Tom's restated vision, 2026-10-09: make working in Rust as easy and light as working in TypeScript or Go. Oxide should aggressively investigate common Rust development problems, qualify solutions and bring them together into a comprehensive, easy-to-use development experience. Ease of use is the primary product axis. Shared caching is the first step, not the product's ceiling.

The immediate problem is recurring drive exhaustion and excessive build churn: every fresh project/worktree, particularly in Paseo with Queue or Nucleus, creates new Rust target directories and repeats expensive work. Tom is concerned about sustained writes to his soldered, non-replaceable laptop SSD. Large targets, slow builds, inconsistent incremental benefits, manual clean/rebuild cycles and disappointing sccache experiences make this costly to manage. These are operator observations; their causes and write volume need measurement rather than assumptions about SSD lifetime.

Containerized Rust development inside Colima compounds storage and build-management friction for projects seeking Linux parity and runtime isolation. It is a related problem to solve in coordination with Effigy, not a reason to assume native builds can replace required Linux checks.

## Intended experience

Tom's Atlas clarification, 2026-10-09: the first adoption audience is his everyday Queue-driven Rust development across projects, with worker agents contending for resources and duplicating builds and storage. This agent-oriented, automatically orchestrated workflow is his main development loop. Oxide's eventual audience is anyone developing Rust, including others with similar agent workflows; the first foothold should serve the actual orchestrated loop.

Developers should be able to open a project or a fresh worktree, obtain useful feedback quickly and keep working without routinely tuning caches, clearing targets, resolving mysterious locks or assembling a collection of performance tools. Agents and orchestrators should consume the same facility with explicit ownership and reliable run evidence. Nucleus will supersede Queue; Oxide's reusable integration must serve that transition.

The desired outcomes are bounded and predictable storage, less repeated compilation and disk writing, fast and dependable edit/check/test loops, intelligible explanations when reuse fails, simple setup and recovery, and deliberate host/container/CI behavior. Breadth includes toolchain/configuration friction, IDE contention, dependencies/features, native dependencies, testing, diagnostics and debugging. Discovery should stay open to other recurring Rust DX problems; this is not a claim that every problem is known or solved.

Ease of use means a small coherent entry point, useful defaults, visible costs, actionable explanations and a way back from changes. Integrate effective existing tools and upstream improvements where possible. A new layer must reduce total management effort. Correctness, useful diagnostics, required checks and repository-owned policy remain constraints on convenience.

Adoption should have minimal impact per project. Tom accepts necessary project changes and potentially tight integration, assessed case by case. Prefer integration into familiar development loops so agents and operators have less to learn, while preserving established development tools. The Cargo integration surface, separate Oxide surface or hybrid remains an open design choice; no transparent Cargo interception is selected.

Some waiting and queue reordering under load are acceptable: agents can wait cheaply for slots or responses. Prioritize fewer simultaneous builds to avoid overloading the system rather than starting every requested build immediately. Queue/Nucleus retain scheduler/admission authority; the slot-waiting mechanism, admission coordination and cancellation policy remain to be designed. Tom has not supplied numeric waiting limits or a new crash-release policy.

Tom's additional ruling, 2026-10-09: the day-to-day development loop prioritizes fast feedback over optimized builds. JIT execution, Cranelift and other alternative code generation/execution solutions are explicitly in scope. Investigate their compromises rather than dismissing them because they differ from production builds. Distinguish fast ahead-of-time compilation, JIT execution and hot reload; none is selected yet. Assess the whole edit-to-feedback cycle, including execution time, memory, writes, compatibility and ease of use. Required validation and release behavior remain governed by their own contracts.

## First foothold and research discipline

Bounded reusable Rust build storage with correct concurrent ownership remains the first foothold. A daemon remains a candidate architecture to qualify. Investigate aggressively but adopt based on measured benefit and supported behavior. Compare elapsed feedback time, occupied space, actual disk writes, peak memory, contention and operator effort independently; reducing stored bytes can increase rebuild writes. Track benefits over sustained edit/worktree cycles, not just one warm build.

Tom's priority ruling, 2026-10-09: independent worktrees duplicating compilation and storage are the dominant current problem. Treat that as settled direction; do not make relief depend on a broad measurement programme or re-proving the operator's diagnosis. Bound initial qualification to the minimum needed for safe reuse and a useful benefit decision. Other Rust DX work can take a more measured approach after this first problem is addressed.

Initial qualification should compare one isolated reference with cross-worktree reuse for one narrow native workflow. Correct input invalidation, exclusive ownership, uncertain release and independent run artifacts remain mandatory. General diagnostics, sustained incremental studies, multiple project baselines, containers and JIT research are later work; none is a prerequisite for first storage relief. Execution bounds and experiment details belong in the Queue brief.

Tom's parallel-priority ruling, 2026-10-10: "I'm happy with the work tree reuse work, I don't want to stop it, but we should definitely be looking at this more holistically and working the cheaper storage angle in parallel." Continue reusable worktree/build-storage delivery while pursuing lower-cost reductions independently: component-lazy preparation, appropriate debug/incremental storage and bounded disposable-output retention. Do not make those improvements depend on completion of the reusable-root facility. Begin with a short, read-only assessment of current project behavior and existing measurements, producing one concrete owner-ready change rather than another broad measurement programme. Distinguish apparent target size from allocated disk space, and reduced retention from reduced writes. Shared compiler caching remains a candidate where supported evidence shows useful coverage; the rejected Acow trial does not establish universal failure. Project changes still use scoped owning-project briefs and independent review; this priority ruling does not authorize builds, installations, live cleanup, settings changes or interference with active workers.

The target experience is aspirational, not a promise of compiler-speed parity with TypeScript or Go. Compiler/backend limitations may require upstream contributions. Prefer adopting upstream cache mechanisms over reproducing Cargo internals when they satisfy the need. Research evidence and limits live in [the Rust DX landscape](rust-dx-landscape.md); ordered outcomes and proposed work live in Queue.

Tom's architecture direction, 2026-10-09: qualify a central daemon as the preferred hub for reusable storage and normalized measurement of real project workflows. Projects should opt into a defined workflow so observations can come from ordinary use, with future experiments integrated deliberately. This is design direction, not permission to install a daemon, change projects or run experiments. Compare the simplest viable daemon design with alternatives within the existing design time box. Storage correctness and immediate duplication relief remain first.

## Relationship with Cargo

Tom's ruling, 2026-10-09: reuse work within Cargo wherever it is available and meets the need; that is the ideal long-term mechanism. Oxide must also fill practical gaps until Cargo catches up. An upstream initiative or roadmap is not a reason to defer relief for today's development problems indefinitely.

Assess shipped behavior and supported pinned toolchains, not promised completion dates. Build the smallest qualified gap-filling mechanism that delivers a useful experience now. Keep that mechanism replaceable so Cargo improvements can take over without making developers or Queue/Nucleus consumers reconstruct their workflows. Temporary mechanisms need explicit coverage limits and criteria for replacement or retirement; durable Oxide policy and experience may remain useful after a mechanism moves upstream. Upstream contributions are welcome when they help close the gap, but their acceptance or timing must not be assumed.

## Authority boundaries

Tom's retained ruling, 2026-10-09: Effigy must remain language agnostic. Cargo/compiler-cache policy must not be added to Effigy. Oxide is a separate Rust-oriented development efficiency/DX project. Queue and Nucleus should consume one reusable Oxide facility rather than two implementations.

Tom's Atlas confirmation, 2026-10-09: Queue/Nucleus manage tasks, agents and milestone QA run slots — the what and when. Oxide manages how Rust work executes efficiently within consumer-authorized workflows and resource bounds, including compatible build storage and Rust-specific execution strategy. Effigy owns language-neutral selector/process/env/lifecycle orchestration. Queue and Nucleus retain scheduler/admission authority, managed Git/source/task/workspace/run identity, authenticated settlement, source/evidence retention and artifact capture policy. Frozen Contract010 must not change.

Current authority covers project registration/claim, repository knowledge, read-only research, planning and proposed briefs. Tom approved the bounded design-only Queue brief on 2026-10-09, including independent review and normal Queue delivery. Tom authorized planning and queueing the following bounded Oxide tranche on 2026-10-09. Each approved Queue brief defines implementation and targeted validation authority after its prerequisites. Real-project experimental execution remains separately gated until exact pilot scope, resource bounds and source-owner coordination are settled. Consumer adoption and host/service installation are not authorized by tranche queueing.

The approved storage-core tranche may prototype the preferred daemon candidate in task-owned scratch state under independent review. Its ordinary local transport and ledger implementation choices can be resolved within the reviewed ownership design. This stages qualification rather than selecting a production deployment: real command process containment, cross-path correctness and adoption still require their own evidence and scoped authority. Unknown containment/capture state remains unavailable for reuse or eviction.

Tom also approved bounded parallel documentation-only design of project adoption/coordinated waiting and pilot containment/capacity readiness on 2026-10-09. These briefs permit read-only source and primary-documentation inspection, owning knowledge edits and independent review. They grant no builds, live consumer changes, experiments, host installations or release changes.

Tom approved the prepared macOS healthy-completion/fencing design follow-up on 2026-10-09: 60 minutes of active investigation plus 20 minutes of independent review, documentation and read-only research only. The task must define a usable normal-release mechanism and its supported writer model, or give a precise missing-proof/authority disposition. Unknown crash/cancel/reply outcomes remain unavailable. Any proposed containment contract or writer-model change still requires a separate ruling before implementation/adoption. The pilot dispatch hold remains in force. Tom also approved the monitored host-free-space policy recorded in [architecture](architecture.md#budget-and-observations); neither approval grants experimental execution or host changes.

Hybrid dev/container/test profile design remains a separate Effigy plus Underlay bundle lane. Longhorn QA pilot phase 3 remains Longhorn-owned. Existing Nucleus workers are outside Oxide bootstrap scope; do not mutate or prompt them. The Nucleus compiler-wrapper lead `lead-1965` potentially duplicates this facility and must not run in parallel with Oxide; its disposition belongs to its owner.

Tom approved the [cooperative supported writer model](architecture.md#supported-writer-model) and the prepared native-fixture implementation/qualification brief on 2026-10-09 after clarification of the macOS design outcome. Authority covers product changes through that Queue PR, targeted repository validation under the injected policy, and at most six small task-owned native-process scenarios: 90 minutes worker execution plus 30 minutes independent replay, with construction-enforced fixture bounds and the monitored host-free-space threshold. No actual consumer/Cargo benefit pilot, host installation, quota volume, deployment or adoption is authorized. The existing real-build pilot hold remains in force.

Tom separately approved the prepared standalone Cargo pilot (oxide#005) on 2026-10-09: implement its minimal experimental Cargo/local-run boundary, then qualify the pinned Nucleus planning/diagnostics workflow in two independent disposable clones. Source approval covers immutable commit `bca75ed0143f96cd20b88ff72a421f1fb5f4ef19`, tree `67ecef91c907ca4085d94ea242b62f678c92993f`, and the brief's single disposable source-miss marker patch; the operator checkout stays read-only. Task-owned setup may create and mount one fixed 8 GiB APFS sparse disk image under `/private/var/tmp/oxide-cargo-8e87a74d`, with ordinary detach only after settled execution/leases. The worker must establish capacity and complete output coverage before Cargo; uncertainty preserves storage and evidence. The unchanged 10 GiB disposable ceiling and monitored 20 GiB host threshold apply. At most six scenarios, 90 minutes worker experiments including setup, and 30 minutes independent replay; no scope or budget expansion.

Tom's subsequent runtime ruling, 2026-10-09: "You don't need to keep asking for approval, blanket approval granted." Within this already-approved pilot, corrected launch arguments and routine bounded retries/recovery have standing authorization; they no longer require a separate operator approval for each attempt. This supersedes the earlier one-shot launch restriction and prohibition on routine retries, without resetting the original experiment budget or expanding scenarios, storage, source or integration scope. A fresh host-tool denial must be reported rather than evaded through another execution route or permission/settings change. Resource, ownership and completion uncertainty still stop execution and preserve evidence. Queue decision `56cc1c2b-f625-4672-b80d-3f3d2f773286` records the exact ruling.

This approval explicitly sequences standalone local qualification before actual consumer integration. Local run/capture identities are qualification evidence, not Queue/Nucleus authenticated settlement or production QA admission. Q-012 consumer lifecycle mapping remains a rollout gate. No active Nucleus prompts, duplicate Nucleus compiler-wrapper implementation, live source/cache/settings changes, installation, existing APFS volume modification, image resize, forced detach, uncertain cleanup, deployment or adoption is authorized. The exact approved protocol and release instructions live in the Queue-held brief; results still need independent exact-head review.

The org repository name was chosen by Tom. Future executable, crate and distribution names need review before publication because of the Oxide Computer Company naming collision; see [release](contracts/release.md).

Tom approved the prepared portability follow-up on 2026-10-09 with "Continue": 45 minutes of documentation/design investigation plus 20 minutes of independent review. It may inspect preserved pilot evidence read-only, compare at most two source/path mechanisms and recommend one concrete implementation/qualification follow-up. It grants no new Cargo scenarios, Rust implementation, source/slot mutations, cleanup/detach, installs or consumer changes. Any proposed replacement of the strict checkout-path proof must be identified explicitly rather than silently weakening its requirements.

Tom accepted the logical-source contract on 2026-10-10 through Queue decision `b5ea13cd-4c03-4a18-8cc3-93baab1f2039` version 2: "Accept logical paths and implement the experimental qualifier." Experimental logical mode may use stable compiler debug/diagnostic, `file!()` and panic-location names instead of physical checkout names, provided each run maps them to its exact requested source tree. Full-debug/source lookup, behavior-bearing path correctness, source/configuration/toolchain invalidation, lease and capture binding remain mandatory. This authorizes the prepared Oxide-only source-map/fail-closed qualifier implementation with small task-owned fixtures and targeted repository validation through normal Queue PR and independent exact-head review. It does not authorize a new real-project pilot, consumer root pool/source-view change, host installation or production reuse; checkout-isolated production keys remain until separate qualification.

Tom approved two prepared parallel follow-ups on 2026-10-10 with "Go for it": Oxide-only registered Cargo source-map evidence collection, with small task-owned fixtures and targeted repository validation; and documentation-only consumer stable-source-path/lifecycle design, bounded to 45 minutes of investigation plus 20 minutes of independent review. Both use normal Queue delivery and independent exact-head review. The design may inspect Queue/Nucleus sources and contracts read-only, but grants no consumer implementation or active-worker prompts. Neither brief authorizes a new real-project Cargo scenario, image/slot mutation or cleanup, installation, source-view rollout, scheduler/wire change or production reuse. The existing pilot evidence remains read-only; fixture evidence cannot be relabelled as real compiler qualification.

Tom approved the prepared owned-Rust reuse probe on 2026-10-10 with "Ok great, carry on", after clarification that it tests actual cross-worktree reuse, changed-source invalidation and behavior-bearing paths. Authority covers Oxide-only producer/probe implementation through Queue and independent exact-head review, plus one tiny std/local-package Rust workspace at two disposable paths. The experiment allows at most four compiler/test invocations, 60 minutes including setup and 20 minutes of independent review, one fresh fixed 8 GiB APFS sparse image under `/private/var/tmp/oxide-rust-probe-20261010`, a 10 GiB disposable ceiling and the monitored 20 GiB host threshold. Routine argument/setup corrections remain within the original caps; tool denials must be reported rather than evaded. Preserve raw observations and uncertain resources. A negative mechanism result is acceptable; more evidence infrastructure alone does not qualify reuse. This grants no new Nucleus build, consumer integration or owner prompts, installation, scheduler/wire change, live cache cleanup or mutation of the previous pilot's resources. The Queue brief owns the exact session and acceptance checks; the delivery goal remains meaningful reduction of recurring worktree build duplication and storage.

Tom's standing-authority ruling, 2026-10-10: "Go for it. You have blanket approval, you don't need to keep asking me" (Queue decision `e75d12a9-5ebd-473e-ad9b-8d70b91f64a7`). This approves the prepared probe-launch correction and fresh bounded retry: the same tiny Rust source/path mechanism test, four experimental compiler/test invocations maximum, 20 minutes including setup plus 20 minutes independent review, a fresh fixed 8 GiB APFS sparse image at `/private/var/tmp/oxide-rust-probe-retry-20261010`, the unchanged 10 GiB disposable ceiling and monitored 20 GiB host threshold. Routine Oxide-only corrective work and further equivalent bounded attempts toward this agreed reuse proof have standing authorization through scoped Queue briefs and independent review; do not ask again solely for a launch correction, ordinary retry or a fresh equivalent session after a window expires. Record each session's scope, deadline, invocation count and resources before dispatch, and never reset a running session's clock or silently increase its caps. Preserve old evidence and unknown leases; blanket approval does not turn uncertainty into release or permit evading host-tool denials. Materially different mechanisms, larger experiments, real consumer/source/scheduler changes, installation, cleanup or production adoption remain separate scope decisions. The correction must include a launch-contract regression before lease acquisition and require genuine Cargo execution before measurement-task completion.

Tom approved the prepared Nucleus-first follow-up on 2026-10-10 with "Go for it": documentation-only design of one existing Nucleus core-test workflow's Oxide input eligibility and consumer-owned persistent source-path/lifecycle contract, bounded to 30 minutes of author investigation and 20 minutes of independent exact-head review. Read-only Nucleus/Queue source and contract inspection may identify the minimum Oxide extension and concrete consumer-owner decisions. This grants no builds or new experiments, consumer implementation or active-owner prompts, source-pool activation, installation, cleanup or scheduler/wire changes. The reviewed tiny same-root fixture result is the starting evidence; real-project coverage and everyday storage/write relief still require separate qualification. Proposed implementation scopes belong in Queue, not repository planning mirrors.

Tom's subsequent "Continue" on 2026-10-10 authorizes the prepared bounded coordination packet to the existing Nucleus and Queue planners (Queue decision `0df0f3b5-b80e-4166-aeb5-57f6a3db093d`). Oxide may send one planning packet and consolidate one response per owner concerning the first workflow's input evidence, finite persistent physical paths, task concurrency, source/debug retention and authenticated acquire/wait/settlement contract. This supersedes the no-owner-contact boundary for that discussion only. It grants no implementation-worker prompts or interruption, consumer task assignment or code/policy/settings changes, builds or experiments, installation or activation. Consumer implementation and representative real-workflow execution still require concrete scoped briefs and their owner/evidence prerequisites.

Tom approved the prepared isolated implementation tranche on 2026-10-10 with "Approve" (Queue decision `0d65d2d2-7720-4b69-b78e-ac6475052496`). Authority covers Oxide's registered-input verification and explicit-argv runner, plus Nucleus-owned task-scoped source/debug retention and authenticated storage-attempt handling through its existing managed-Git, retention and admission owners. Oxide may relay the concrete consumer scope to the existing Nucleus planner, which owns its Queue briefs and dispatch; do not create a competing compiler or Git manager. Each separately reviewed scope has 90 minutes of author work and 30 minutes of independent exact-head review, with targeted ordinary repository validation and newly owned bounded fixtures. Code paths remain inactive by default. This supersedes the prior coordination-only limit for these implementation scopes; it does not authorize active-checkout conversion, live root allocation/recycling, representative Nucleus workflow experiments, installation, cleanup, settings/scheduler/wire changes or Contract 010 edits. Real use still requires the audited native-input/accepted-issuer contract, old-run source/debug retrieval proof, explicit Contract 002 pilot departure and finite storage/retention/waiting policy, followed by a separately scoped representative proof.

Tom approved the isolated binding tranche on 2026-10-11 through Queue decision
`6c303464-2250-4a3e-873e-80acb121d3f4` version 2: "Approve isolated binding
tranche." It covers three sequential, independently reviewed scopes: Oxide's
durable slot-generation API and authenticated lookup-only acquire/finalize
recovery; its private framed Rust registered-runner bridge with an injected
synthetic parent; then Nucleus's disabled synthetic `StorageTransport` client
after the reviewed Oxide API and bridge merge and closeout. Each scope has
90 minutes of author work and 30 minutes of independent exact-head review,
targeted repository checks and fresh bounded fixtures. This narrowly permits
the prototype storage-protocol and private bridge changes excluded previously.
Generation remains bound to the authenticated storage instance and slot;
lookup must not allocate, release, evict or grant writer access. Lost tokens and
uncertain ownership hold. Nucleus owns its brief, dispatch and client code;
Oxide may relay the approved packet and actual reviewed API. The Nucleus owner
may temporarily set native admission to one solely for that client task after
Oxide prerequisites, restoring zero after final closeout.

The binding approval grants no production issuer or signing-key deployment,
real Cargo/SDK/native-input scan or representative experiment, live root
recycling, cache cleanup, installation, activation, release or Contract 010
change. Production registration remains disabled. Genuine host submission
uses the existing reviewed Effigy path; no second scheduler or manager is
authorized. Real consumer eligibility, Contract 002 departure, budgets and
source/debug evidence remain separate gates. Acow's cheaper-storage work
continues independently.

Tom approved the next representative-workflow step on 2026-10-11 with
"Go for it" (Queue decision `81c05164-5914-4c10-b85a-e80084bb46f0` version 2):
complete the actual private Rust host/Nucleus client path and trusted input,
execution and capture evidence for the selected core-test workflow, then run
one bounded cold, warm and changed-source qualification. Oxide may dispatch
its implementation through checked Queue briefs and relay the consumer scope
to Nucleus's existing owner, which owns its separate briefs and task controls.
This permits a private, unpublished host entry point and owner-installed
qualification authentication; it does not deploy a general production issuer
or install a service. Implementation scopes retain 90-minute author and
30-minute independent exact-head review budgets. Independent seams may proceed
in parallel; no active consumer worker is prompted or interrupted.

The representative proof requires its own checked execution brief after the
real input/graph/tool/SDK, source identity, cooperative completion and full-debug
retention gates pass. Record exact source/tool/selector pins and fresh resources
before launch. Bound the proposed proof to at most four Cargo invocations and
45 minutes of live work, with separately bounded read-only review. The existing
registered read ceilings and conservative uncertainty rules remain in force;
finite total-footprint enforcement, host reserve and consumer waiting/retention
policy must be settled before that brief is admitted. No broad benchmark or
host scan, old-resource reuse, active-checkout conversion, production root-pool
activation, cleanup, release or Contract 010 change follows from this ruling.
It grants no general admission/settings change; any consumer dispatch capacity
must follow its owning recorded authority. Implementation or synthetic success
cannot satisfy the real proof's gates, and trial success is not production rollout.

Tom's further ruling, relayed by the Nucleus owner on 2026-10-11:
"Sounds good - get this going, and the work you suggested in your previous
response. You have blanket authority - keep all of this moving, make progress
on your own." Progress the smaller ordinary-Cargo storage-first route alongside
the bounded graph parser. Oxide owns the separately reviewed storage/execution
route; Nucleus owns source/attempt binding, genuine existing Effigy/Contract010
submission and settlement, retention and its private client. Start with one
fresh qualification workspace and sequential same-root attempts, exclusive
ownership, always-run selected tests, finite budgets and an accepted pinned
installation epoch/no-replacement commitment. This is an explicit ordinary
Cargo freshness policy, not a relaxation of the registered Runner or a claim
of complete graph/tool-content proof. Unknown dependency macro, build-script
or native inputs require a qualified invalidation boundary, whole uncertain
partition rebuild, or refusal; cleaning local source outputs is insufficient.
Checked Queue scopes and independent review precede implementation and use.
The original total four-Cargo/45-minute proof remains unstarted until the joint
checked packet fixes pins, numeric footprint, deadlines, capture/debug evidence
and owner conditions. No production activation, general installation/config
change, new scheduler or consumer admission change is implied. Do not ask again
for routine bounded progress already covered by this ruling.

Tom approved the lifecycle-aware Rust build-policy proposal on 2026-10-15
with "Approved". Authority covers an independently reviewed design and one
small comparison of incremental-on/unpacked and incremental-off/unpacked
on the same pinned workflow and edit sequence. The comparison is bounded by
ten Cargo invocations in total, twenty minutes including preparation and
capture, a fresh owned footprint of at most 16 GiB, and a monitored 20 GiB
raw host free-space reserve. Cargo probes count toward the invocation cap.
Exact source, tool, command, resource and evidence pins must be recorded in
the checked execution brief before launch; there is no implicit retry or
clock reset. Selected tests must execute, and retained bytes, file counts,
hardlink-aware allocation, latency and attributable writes must be reported
separately. Unmeasured writes and debugger capabilities remain unknown.

The design distinguishes expected build/cache lifetime and debugging needs
from human or agent identity. One-shot incremental-off and iterative
incremental-on are candidates to qualify, not changes to consumer defaults.
At that approval, Nucleus's managed-agent stopgap remained in force pending
consumer adoption; Tom's later Nucleus ruling below supersedes it. This
approval does not enable a production policy resolver,
shared warm slots, automatic generation retirement, selective object deletion,
global configuration changes or scans/deletion of existing target trees.
Object age, filename, compiler session locks or process disappearance cannot
authorize Oxide lease release or evidence removal. The original four-Cargo,
45-minute reuse proof remains separate and is neither consumed nor reset by
this comparison. Queue owns the scoped briefs, dispatch and independent review.


Tom's direct-execution correction, relayed by the Nucleus Supervisor on
2026-10-15: stop the incremental-comparison task-fixes-task chain. Once the
existing bootstrap correction finishes, the Oxide planner should run the
approved comparison directly in its own thread rather than dispatch another
brief. This supersedes the checked-execution-brief and worker-dispatch
requirement for this comparison only. A small disposable shell measurement
script is permitted; no product implementation or general adoption follows.
Keep the same ten-Cargo-call, twenty-minute total, 16 GiB fresh-owned limit,
20 GiB raw host reserve, pinned Nucleus source, incremental on/off with
unpacked line tables, cold/edit/unchanged sequence, executed tests and raw
backtrace/output evidence. Preserve genuine Effigy host admission and all
existing evidence; do not reset a live clock, change consumer defaults or
inspect/delete old targets. Record actual limitations rather than building
another measurement framework. Product changes still require ordinary
independent exact-head review.


Tom's Nucleus adoption, relayed by its Supervisor on 2026-10-15 and recorded
in Nucleus `ff43e69701f08e5a9ddf84d424a381dea8d033de` Contract 002:
managed agents build with incremental on again; one-shot validation keeps
`CARGO_INCREMENTAL=0`, citing Oxide's direct comparison `943c6f9`. Nucleus
owns task-workspace deletion at closeout. This limits retention by task
lifetime, not by an enforced per-task byte budget. It supersedes the prior
Nucleus agent stopgap without activating Oxide shared storage or changing
Queue policy. [Storage efficiency](storage-efficiency.md#nucleus-consumer-adoption-2026-10-15)
owns the adoption receipt and its limits.


Tom requested the next roadmap and board work on 2026-10-15: “Update the
planner, get the tasks on the board. What's next?” Together with the standing
Oxide development authority, this authorizes the next bounded Oxide-only
product tranche through checked Queue briefs and independent exact-head
review: enforced owned build-storage bounds, a bounded read-only storage
inventory, and explicit retirement of proven-idle generations. Preserve the
usual 90-minute author / 30-minute review scope per task and name finite
fresh fixture limits before dispatch. This is product delivery, rather than
another comparison-launcher or generic provenance-wrapper chain. The direct
measurement exception stays specific to that experiment. Real consumer
integration, the original representative trial, installation, default changes
and cleanup of existing storage retain their separate boundaries.
