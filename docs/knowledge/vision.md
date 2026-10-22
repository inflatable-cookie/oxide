# Vision and scope

Current truth only: Oxide's purpose, direction, ownership and standing authority.
Shipped behaviour lives in the [contracts](contracts/workflow.md). Research lives in
the [Rust DX landscape](rust-dx-landscape.md). Open questions live in
[questions](questions.md). Plans, briefs and one-off operational rulings live in
Queue, whose decisions record Tom's exact words. This file was condensed on
2026-10-21. The dated ruling log it held from 2026-10-09 to 2026-10-21 is
preserved in git at commit `ecf379e`.

## Purpose

Tom, 2026-10-09: make working in Rust as easy and light as working in
TypeScript or Go. Oxide investigates common Rust development problems,
qualifies solutions and combines them into one easy-to-use development
experience. Ease of use is the primary product axis. Shared caching is the
first step, not the ceiling.

The immediate problem is build churn and disk use. Every fresh project or
worktree, especially under Paseo with Queue, recreates Rust target directories
and repeats expensive work. Tom's laptop SSD is soldered and non-replaceable,
so sustained writes to it are the cost to cut. Causes and write volumes are
measured rather than assumed.

## Intended experience

- **Who it's for:** Tom's everyday Queue-driven development first, where worker
  agents contend for resources and duplicate builds. Eventually anyone who
  develops in Rust.
- **What it should feel like:** open a project or a fresh worktree, get fast
  feedback, and keep working. No routine cache tuning, target clearing, lock
  hunting or tool assembly.
  - Agents and orchestrators use the same facility, with explicit ownership and
    reliable run evidence.
- **What it should deliver:**
  - bounded, predictable storage;
  - less repeated compilation and disk writing;
  - fast, dependable edit, check and test loops;
  - an understandable explanation when reuse fails;
  - simple setup and recovery.
- **Ease of use means:**
  - a small, coherent entry point;
  - good defaults;
  - visible costs;
  - actionable messages;
  - a way back from every change.

  Correctness, required checks and repository-owned policy still limit
  convenience.
- **The dev loop favours fast feedback over optimised builds.** JIT,
  Cranelift and other alternative code-generation strategies are in scope.
  They are judged on the whole edit-to-feedback cycle. Required validation and
  release builds keep their own contracts.
- **Adoption should cost little per project.** Necessary project changes are
  acceptable, case by case. Integrate into familiar development loops and keep
  established tools.
- **Waiting under load is acceptable.** Fewer simultaneous builds beat an
  overloaded machine.

## Relationship with Cargo

Tom, 2026-10-09: reuse Cargo's own mechanisms wherever they meet the need;
that is the long-term ideal. Oxide fills practical gaps until Cargo catches up.
An upstream roadmap is never a reason to defer today's relief.

- **What counts as evidence:** shipped behaviour on pinned, supported
  toolchains, not promised dates.
- **What to build:** the smallest qualified gap-filler. Keep it replaceable,
  so a Cargo improvement can take over without making developers or consumers
  rebuild their workflows. Each temporary mechanism states its coverage and
  when it retires.
- **Upstream contributions** are welcome, but neither their acceptance nor
  their timing is assumed.
- **Cargo's planned cross-workspace cache** is the preferred long-term
  complement to Oxide's slots. The new build-dir layout in Rust 1.100
  (2026-11-12) is the next upgrade point.

## Ownership

- **Oxide** owns how Rust work runs efficiently: build storage, slot reuse,
  profiles and execution strategy, within the workflows and resource bounds
  its consumers authorise.
- **Effigy** stays language-agnostic, and Cargo or compiler policy never
  enters its core (Tom, 2026-10-09). The approved
  [program fallback](#adoption-pattern) is a generic Effigy mechanism, not
  Oxide logic in Effigy.
- **Queue, and later Nucleus,** own tasks, agents, scheduling and admission,
  plus workspace, run and source identity. Oxide never changes Contract 010.
- **Product architecture** belongs to the appointed Oxide Supervisor; Queue
  operations belong to the operations planner.

## Product workflow direction

### Front door

Tom, 2026-10-18. Oxide is an explicit build service, not an interceptor.
- **Commands:** `oxide check|test|clippy|build|run|fmt|nextest|…` take Cargo's
  arguments. Commands Oxide doesn't model pass straight through to Cargo.
  `oxide init` and `oxide new` include Oxide adoption.
- **Profiles:** `quick` (the default), `debug` and `release`. `quick` and
  `debug` tune Cargo's `dev` profile, so outputs land in `target/debug`
  (#106).
- **No interception:** `cargo` is not transparently redirected. Rustup or
  PATH shims and shell aliases are deferred until the foundation is
  consistently reliable.
- **Adoption** happens through the `oxide init` agent-docs block, the Oxide
  skill and each project's Effigy selectors.
- **Crash recovery:** Oxide recovers its own interrupted commands. A
  recovery requires a free record lock and matching lease ownership, plus
  either an empty Cargo process group (`ESRCH`) or a recorded pre-launch
  state. An unknown or `EPERM` holder stays held.
- **`oxide exec -- <tool>`** runs Cargo-wrapping tools such as the Tauri CLI
  with Oxide storage (#107). When a checkout's slot is busy, a further command
  in that checkout takes a compatible spare slot. Whether Tauri honours
  build-dir is still unverified.
- **Paused:** the unchanged-source `oxide run` fast path. Its plan is preserved
  in #089's closed task and its committee contract.
- **Deferred:** remote builds. The executor seam stays local-only.

### Storage

- **Pools are budget-only:** a soft admission budget, whole-idle-slot eviction
  and the host-reserve watchdog, with no OS-enforced cap.
- **Internal pool:** `~/Dev/oxide`, with a 100 GB budget. Config follows
  Longhorn's storage-location policy, through Longhorn's own resolver.
- **The Builds drive** is the preferred pool:
  - a registered removable APFS volume, permanently attached and treated as
    consumable;
  - other tools may keep scratch on it outside `oxide-pool`.
- **Drive mode:** the Builds drive is registered as permanent or ephemeral,
  with a loud warning when a build falls back to the internal pool.
  `oxide pool describe --json` reports it to other tools.
- **Final outputs** stay checkout-local by default. Permanent drive mode adds
  one explicit, per-checkout owned `target` link; it never moves existing
  output, and ephemeral mode is unchanged
  ([workflow](contracts/workflow.md#final-outputs-on-the-permanent-drive-oxide117)).
- **Sharing builds:** slots share warm builds across worktrees, sized by the
  byte budget rather than small fixed counts.
- **sccache:** Oxide bypasses inherited compiler wrappers and doesn't manage
  sccache. Integration is revisited once the current work is done. Cargo's
  own cross-workspace cache is the preferred long-term complement.
- **Approved, not built:**
  - per-slot test scratch;
  - higher slot limits with budget-driven eviction;
  - submodule-aware cross-worktree reuse.

  Details live in their Queue leads and, once built, in the
  [workflow contract](contracts/workflow.md).
- **Parked:** cached slot accounting.

### Adoption pattern

- **How a project switches:** its Effigy selectors call `oxide`. Anything that
  owns its own target directory, or runs in a container, stays on Cargo. The
  full rules are in the
  [workflow contract](contracts/workflow.md#switching-an-existing-project).
- **The fallback to Cargo is Effigy's,** through a generic per-program
  fallback that Effigy owns and documents. It replaces per-selector `:cargo`
  escapes and project-side CI switches. Oxide ships no CI shim.
- **Projects on Oxide** are tracked in Queue. Nucleus adoption is off.

### Toolchain support

- **Supported range:** the target is stable Rust 1.95 and later.
  Version-sensitive behaviour is qualified per exact pinned toolchain.
  - The qualified pins are listed in the workflow contract's Cargo version
    matrix.
  - Newer or patch versions are not assumed to qualify.
  - Any unqualified version turns those features off rather than guessing.
- **Next upgrade point:** Rust 1.100. Qualifying on it after release is
  approved.

## Validation policy

Tom, 2026-10-20: risk-proportionate verification. AGENTS.md "Validate" holds
the operating rules.
- **Standard tier:** author checks and native proof, plus an independent source
  review.
- **Elevated tier,** for deletion or recycling, crash recovery or writer
  closure, and removable media: adds an independent reviewer native session.
- **Bounds:** wall-clock time and scratch budget. Resource figures are
  reported, not gated.
- **Operator acceptance fast path:** Tom can accept a task as-is. It is
  recorded as operator acceptance, never as a review pass. A direct GitHub
  merge needs explicit scoped authorisation.
- **Native-proof harness:** native proofs run under the shared proof
  supervisor, `scripts/bounded-test.py`. It qualified as #109 (PR #99) on
  2026-10-21, under the pinned-leader cooperative-closure contract (Tom,
  "Yes, (2)"), with independent author and reviewer native passes at
  `018f6e1`.
  - New tasks use it. They don't need per-task guard exceptions.
  - The earlier task-specific old-guard exceptions (route C for #110 and #106,
    route B for #107, and #104's one run) are history, and their disclosed
    gaps stay with those tasks.

## Authority boundaries

- **Standing correction authority** (Tom, 2026-10-10, "Go for it. You have
  blanket approval, you don't need to keep asking me"):
  - covers routine Oxide-only corrections and equivalent bounded retries,
    through scoped Queue briefs and independent review;
  - never resets a running clock or enlarges caps silently;
  - doesn't turn uncertainty into release, or allow evading a host-tool
    denial.
- **Model routing:** briefs declare task complexity; Queue selects the author
  and reviewer models.
- **Still need Tom's scoped approval:**
  - switching a new consumer project;
  - host installs beyond the planner's receipt-matched `oxide` reinstall;
  - releases, tags and workflow changes;
  - settings or scheduler changes;
  - live cache or storage cleanup;
  - material scope changes.
- **Nucleus:** never prompt or change active Nucleus work.
- **Recording rulings:**
  - Queue decisions record Tom's exact words.
  - A ruling that changes current truth updates its owning knowledge file.
  - A purely operational ruling stays in Queue.
