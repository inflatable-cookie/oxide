# Vision and scope

## Purpose

Tom's restated vision, 2026-10-09: make working in Rust as easy and light as working in TypeScript or Go. Oxide should aggressively investigate common Rust development problems, qualify solutions and bring them together into a comprehensive, easy-to-use development experience. Ease of use is the primary product axis. Shared caching is the first step, not the product's ceiling.

The immediate problem is recurring drive exhaustion and excessive build churn: every fresh project/worktree, particularly in Paseo with Queue or Nucleus, creates new Rust target directories and repeats expensive work. Tom is concerned about sustained writes to his soldered, non-replaceable laptop SSD. Large targets, slow builds, inconsistent incremental benefits, manual clean/rebuild cycles and disappointing sccache experiences make this costly to manage. These are operator observations; their causes and write volume need measurement rather than assumptions about SSD lifetime.

Containerized Rust development inside Colima compounds storage and build-management friction for projects seeking Linux parity and runtime isolation. It is a related problem to solve in coordination with Effigy, not a reason to assume native builds can replace required Linux checks.

## Intended experience

Developers should be able to open a project or a fresh worktree, obtain useful feedback quickly and keep working without routinely tuning caches, clearing targets, resolving mysterious locks or assembling a collection of performance tools. Agents and orchestrators should consume the same facility with explicit ownership and reliable run evidence. Nucleus will supersede Queue; Oxide's reusable integration must serve that transition.

The desired outcomes are bounded and predictable storage, less repeated compilation and disk writing, fast and dependable edit/check/test loops, intelligible explanations when reuse fails, simple setup and recovery, and deliberate host/container/CI behavior. Breadth includes toolchain/configuration friction, IDE contention, dependencies/features, native dependencies, testing, diagnostics and debugging. Discovery should stay open to other recurring Rust DX problems; this is not a claim that every problem is known or solved.

Ease of use means a small coherent entry point, useful defaults, visible costs, actionable explanations and a way back from changes. Integrate effective existing tools and upstream improvements where possible. A new layer must reduce total management effort. Correctness, useful diagnostics, required checks and repository-owned policy remain constraints on convenience.

Tom's additional ruling, 2026-10-09: the day-to-day development loop prioritizes fast feedback over optimized builds. JIT execution, Cranelift and other alternative code generation/execution solutions are explicitly in scope. Investigate their compromises rather than dismissing them because they differ from production builds. Distinguish fast ahead-of-time compilation, JIT execution and hot reload; none is selected yet. Assess the whole edit-to-feedback cycle, including execution time, memory, writes, compatibility and ease of use. Required validation and release behavior remain governed by their own contracts.

## First foothold and research discipline

Bounded reusable Rust build storage with correct concurrent ownership remains the first foothold. A daemon remains a candidate architecture to qualify. Investigate aggressively but adopt based on measured benefit and supported behavior. Compare elapsed feedback time, occupied space, actual disk writes, peak memory, contention and operator effort independently; reducing stored bytes can increase rebuild writes. Track benefits over sustained edit/worktree cycles, not just one warm build.

The target experience is aspirational, not a promise of compiler-speed parity with TypeScript or Go. Compiler/backend limitations may require upstream contributions. Prefer adopting upstream cache mechanisms over reproducing Cargo internals when they satisfy the need. Research evidence and limits live in [the Rust DX landscape](rust-dx-landscape.md); ordered outcomes and proposed work live in Queue.

## Relationship with Cargo

Tom's ruling, 2026-10-09: reuse work within Cargo wherever it is available and meets the need; that is the ideal long-term mechanism. Oxide must also fill practical gaps until Cargo catches up. An upstream initiative or roadmap is not a reason to defer relief for today's development problems indefinitely.

Assess shipped behavior and supported pinned toolchains, not promised completion dates. Build the smallest qualified gap-filling mechanism that delivers a useful experience now. Keep that mechanism replaceable so Cargo improvements can take over without making developers or Queue/Nucleus consumers reconstruct their workflows. Temporary mechanisms need explicit coverage limits and criteria for replacement or retirement; durable Oxide policy and experience may remain useful after a mechanism moves upstream. Upstream contributions are welcome when they help close the gap, but their acceptance or timing must not be assumed.

## Authority boundaries

Tom's retained ruling, 2026-10-09: Effigy must remain language agnostic. Cargo/compiler-cache policy must not be added to Effigy. Oxide is a separate Rust-oriented development efficiency/DX project. Queue and Nucleus should consume one reusable Oxide facility rather than two implementations.

Effigy owns language-neutral selector/process/env/lifecycle orchestration. Queue and Nucleus retain scheduler/admission authority, managed Git/source/task/workspace/run identity, authenticated settlement, source/evidence retention and artifact capture policy. Frozen Contract010 must not change.

Current authority covers project registration/claim, repository knowledge, read-only research, planning and proposed briefs. Tom authorized the broader vision and research/plan on 2026-10-09. There is no experimental execution authority. Product implementation proceeds through explicitly approved Queue briefs and independent review.

Hybrid dev/container/test profile design remains a separate Effigy plus Underlay bundle lane. Longhorn QA pilot phase 3 remains Longhorn-owned. Existing Nucleus workers are outside Oxide bootstrap scope; do not mutate or prompt them. The Nucleus compiler-wrapper lead `lead-1965` potentially duplicates this facility and must not run in parallel with Oxide; its disposition belongs to its owner.

The org repository name was chosen by Tom. Future executable, crate and distribution names need review before publication because of the Oxide Computer Company naming collision; see [release](contracts/release.md).
