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

Hybrid dev/container/test profile design remains a separate Effigy plus Underlay bundle lane. Longhorn QA pilot phase 3 remains Longhorn-owned. Existing Nucleus workers are outside Oxide bootstrap scope; do not mutate or prompt them. The Nucleus compiler-wrapper lead `lead-1965` potentially duplicates this facility and must not run in parallel with Oxide; its disposition belongs to its owner.

The org repository name was chosen by Tom. Future executable, crate and distribution names need review before publication because of the Oxide Computer Company naming collision; see [release](contracts/release.md).
