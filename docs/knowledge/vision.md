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

- **Pools are budget-only.** Admission is against a soft budget, with
  whole-idle-slot eviction and the host-reserve watchdog (trigger 22 GiB free,
  floor 20 GiB). There is no OS-enforced cap. Disk-image pools and the
  standalone lease tool are retired.
- **The internal pool** is `~/Dev/oxide`, with a 100 GB budget.
- **Config** lives at `~/Library/Application Support/oxide/config/config.toml`
  (XDG on Linux). That follows Longhorn's storage-location policy, mirrored
  until Longhorn's CLI crate ships in 0.4.0.
- **The Builds drive**, a registered removable APFS volume, is the preferred
  pool:
  - admission cap 750 GB, reserve 100 GB;
  - since 2026-10-21 it is permanently attached;
  - its root is `755`;
  - it is treated as consumable, so Oxide should report its writes;
  - non-Oxide scratch may live on it **outside** `oxide-pool`, provided it
    falls back to internal when the drive is absent and never creates
    `/Volumes/...` on the internal disk.
- **Final outputs** stay checkout-local unless the drive is in permanent mode.
- **Approved, not built: the drive mode** (`--mode permanent|ephemeral`,
  default ephemeral).
  - In permanent mode, a missing drive gives a **loud** fallback, final outputs
    go to the drive behind a `target` link that Oxide maintains, and per-slot
    test scratch goes to the drive.
  - `oxide pool describe --json` publishes the drive details for other tools.
- **Slots and accounting:**
  - Finder metadata in the pool is tolerated (#105);
  - project-named paths for new slots are approved and being built (#104),
    not shipped;
  - pool accounting has pool-specific bounds (#108);
  - front-door membership checks read only the ledger (#110). New-slot
    admission and eviction still walk the whole payload, and reuse walks the
    target slot.
- **Approved, not built:**
  - per-slot test scratch for processes Oxide launches. An explicit `TMPDIR`
    still wins;
  - a configurable host build limit (`[pool] max_active`):
    - precedence is environment, then config, then the default;
    - the default is `min(4, max(2, floor(physical cores / 4)))`;
    - if physical cores can't be read, it falls back to 2 and says so, and
      never silently uses logical cores;
    - `oxide status` reports the value and where it came from.
- **Parked:** cached slot accounting. Exact caching needs proof that an idle
  slot is immutable, which doesn't exist yet. Explicit `oxide status` still
  walks the whole pool and is slow.
- **sccache:** Oxide bypasses any inherited `rustc-wrapper` (#099). Combining
  sccache with Oxide is deferred, and revisited only if cold slots prove
  costly.

### Adoption pattern

- **How a project switches:** its Effigy selectors call `oxide`. Scripts that
  choose their own `CARGO_TARGET_DIR`, container builds, Queue hooks and tools
  that run Cargo themselves stay on Cargo (until `oxide exec` covers them).
- **Approved, Effigy to build: a program fallback.** It replaces per-selector
  `:cargo` escapes and project-side CI switches.
  - **Config:** `[programs.oxide] fallback = "cargo"`, with an optional `allow`
    list and a `force` switch.
  - **When `oxide` doesn't resolve, or `force` is set:** Effigy runs `cargo`
    with exact arguments through a run-scoped PATH alias, with an honest notice
    each run.
  - **Without `allow`,** Oxide-only commands take Cargo's meaning. For example,
    `oxide clean` becomes `cargo clean`.
  - **Documentation condition:** `effigy help` and the Effigy skill must
    document these conditions.
- **Parked: the Oxide CI shim** (`setup-oxide`), for Effigy-run CI.
- **Projects on Oxide** are tracked in Queue lead `f4813b68`. Nucleus adoption
  is off.

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
- **Native-proof harness:** the shared proof supervisor is #109, under the
  pinned-leader cooperative-closure contract (Tom, "Yes, (2)").
  - Its design passed review (revision 4, `29f0950e`).
  - The implementation is in focused completion under Tom's 2026-10-21
    standing authority: "Keep going - at this point we may as well keep
    persevering until its done?"
- **Until #109 qualifies,** native proofs use only the task-specific exceptions
  Tom granted: main's old `scripts/bounded-test.py` at `3d947a9`, with its gaps
  disclosed per task.
  - **Standard tasks:** #110 and #106 (route C). #104 has one corrected
    author native session ("#104 only for now"), not a task-wide exception;
    Queue holds its later counts.
  - **Elevated tasks:** route B covers #107, #098 and #100.
  - **Not covered:** new tasks don't inherit these exceptions.

## Authority boundaries

- **Standing correction authority** (Tom, 2026-10-10, "Go for it. You have
  blanket approval, you don't need to keep asking me"):
  - covers routine Oxide-only corrections and equivalent bounded retries,
    through scoped Queue briefs and independent review;
  - never resets a running clock or enlarges caps silently;
  - doesn't turn uncertainty into release, or allow evading a host-tool
    denial.
- **#109 completion authority** (Tom, 2026-10-21): localised, in-scope
  corrections continue until it qualifies, with no per-defect ask.
  - Each failed session still stops for owner inspection.
  - Redesign, new infrastructure or privilege, and any weakening of the safety
    contract remain Tom's call.
- **Model routing:** Opus High authors and Sol High reviews. Astra is not used
  for review (Tom, 2026-10-18, "Use Sol").
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
