# Vision and scope

Current truth only: Oxide's purpose, direction, ownership and standing authority.
Shipped behaviour lives in the [contracts](contracts/workflow.md). Research lives in
the [Rust DX landscape](rust-dx-landscape.md). Open questions live in
[questions](questions.md). Plans, briefs and one-off operational rulings live in
Nucleus, whose decisions record Tom's exact words. This file was condensed on
2026-10-21; its dated ruling log is preserved in git history at `ecf379e`.

## Purpose

Tom, 2026-10-09: make working in Rust as easy and light as working in
TypeScript or Go. Oxide investigates common Rust development problems,
qualifies solutions and combines them into one easy-to-use development
experience. Ease of use is the primary product axis. Shared caching is the
first step, not the ceiling.

The immediate problem is build churn and disk use. Every fresh project or
worktree, especially under Paseo with Nucleus, recreates Rust target directories
and repeats expensive work. Tom's laptop SSD is soldered and non-replaceable,
so sustained writes to it are the cost to cut. Causes and write volumes are
measured rather than assumed.

## Intended experience

- **Who it's for:** Tom's everyday Nucleus-driven development first, where worker
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
  complement to Oxide's slots. Oxide is pinned to Rust 1.100 and reads the
  build-directory layout Cargo actually wrote: v2 by default, and v1 under
  Cargo's own opt-out.

## Ownership

- **Oxide** owns how Rust work runs efficiently: build storage, slot reuse,
  profiles and execution strategy, within the workflows and resource bounds
  its consumers authorise.
- **Effigy** stays language-agnostic, and Cargo or compiler policy never
  enters its core (Tom, 2026-10-09). The approved
  [program fallback](#adoption-pattern) is a generic Effigy mechanism, not
  Oxide logic in Effigy.
- **Nucleus** owns tasks, agents, scheduling and admission,
  plus workspace, run and source identity. Oxide never changes Contract 010.
- **Product architecture** belongs to the appointed Oxide Supervisor; Nucleus
  operations belong to the operations planner.

## Product workflow direction

### Planning discipline

Tom, 2026-10-23: keep concrete work we intend to deliver, rather than an
indefinite parked backlog. Give each item a delivery path or explicitly decline
it, preserving the reason and history. Fold duplicate tracking into its owning
brief or project. Closing an unimplemented item records that disposition; it
does not claim a fix. New evidence can justify a new decision.

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
- **Diagnostics and adoption direction** (Tom, 2026-10-23; implemented):
  default `oxide status` is fast and read-only, showing recorded sizes with
  their age and source, or "not measured". Explicit `status --measure`
  requests bounded measurement; observation never reconciles deletions, and
  these diagnostic facts do not decide admission or reopen cached accounting.
  In a Git repository, `oxide init` writes one managed block in the
  work-tree root's `AGENTS.md`, listing the Cargo workspaces it covers.
  Existing nested blocks are reported and retained; outside Git the
  workspace-local block stands.
- **Crash recovery:** Oxide recovers its own interrupted commands. A
  recovery requires a free record lock and matching lease ownership, plus
  either an empty Cargo process group (`ESRCH`) or a recorded pre-launch
  state. An unknown or `EPERM` holder stays held.
- **`oxide exec -- <tool>`** runs Cargo-wrapping tools such as the Tauri CLI
  with Oxide storage (#107). When a checkout's slot is busy, a further command
  in that checkout takes a compatible spare slot. Whether Tauri honours
  build-dir is still unverified.
- **Command time budgets** (Tom, 2026-10-23; implemented): leased commands
  accept separate finite admission and child-execution budgets, with a typed
  child-start boundary reported as an event. Compilation, Cargo locks and
  nested admissions consume the execution budget once it starts; nested calls
  never pause or reset an outer budget. Signal forwarding and conservative
  group closure are preserved. Consumer adoption and installation remain
  separate gates.
Nested commands currently take an independent lease, and idle `oxide exec`
sessions hold their host permit for the full session. Tom's 2026-11-23 ruling
settled the Q-020 / Q-018 direction: build both nested lease joining and idle
`oxide exec` permit borrowing over a parent-owned local Unix domain socket
broker with a per-session random token (`0700` socket dir, same-UID peer
credential verification), clamp nested admission/execution budgets to
`min(inner, outer_remaining_execution_ms)` via the broker handshake (never
extending or resetting the outer budget; outer process-group kill remains the
hard backstop), and deliver macOS first followed by Linux parity. Implementation
is tracked in Nucleus (`oxide-nested-lease-joining`).
- **Paused:** the unchanged-source `oxide run` fast path. Its plan is preserved
  in #089's closed task and its committee contract.
- **Deferred:** remote builds. The executor seam stays local-only.

### Current storage direction (2026-11-17)

Tom's later ruling is to remove the old qualification machinery and "just get
it working without all the ceremony". Barge now has its own project and
planner. Oxide uses Barge as a linked Rust library: installing Oxide needs no
Barge daemon or separate Barge installation.

The selected replacement uses generic areas, owned roots, cooperative file-lock
holds, durable pins and rename-to-trash deletion. Oxide retains Cargo,
compatibility and freshness, slots, recipes, seeding, output member links and
`exec`. Preserve warm root reuse, warned absent-volume fallback, soft budgets,
idle eviction, selected-output protection, `init`/`new` and older-binary handling.
An evicted root's successor gets a fresh path; importing a legacy directory
must not silently reclaim a stray at a retired path. Barge's
[architecture](https://github.com/inflatable-cookie/barge/blob/main/docs/knowledge/architecture.md)
owns the generic library boundary.

One Oxide adoption PR replaces the duplicated storage internals and removes the
capture, comparison, descriptor-census and qualification apparatus. Ordinary
temporary-directory unit and integration tests, including Oxide's real child
launch inheriting the hold, plus independent exact-head source/evidence review
replace the old native proof ladders for this work. Earlier journal, supplier,
common-clock and supervisor-ceiling scopes recorded below are history, not
current adoption prerequisites. Their failures and evidence are not rewritten.

The existing installed Oxide remains authoritative until the replacement is
reviewed and installed separately. Source work authorizes no live data
migration, registration or cleanup. Retained fixture cleanup needs its own
operator instruction; the general lean reset is not deletion authority.

### Storage

- **Barge owns drive management** (implemented in oxide#314): volume registration and selection, placement and fallback, owned roots, pruning and scratch use the linked lean Barge library ([inflatable-cookie/barge](https://github.com/inflatable-cookie/barge)), which Oxide uses as a pinned library. Oxide keeps Cargo, slots, leases, recipes, freshness and `exec`. Barge holds and selected-output pins replace Oxide's deletion/census/prune engines; Cargo and output-link workflow stay in Oxide.
- **Pools are budget-only:** a soft admission budget, whole-idle-slot eviction
  and the host-reserve watchdog, with no OS-enforced cap.
- **Warm reuse redesign** (Tom, 2026-11-05, decision
  `e3dccedb-0046-4003-a9a0-f4d44517cc44`, "Design warm reuse without a full
  scan"): design normal matching-slot reuse without a mandatory recursive size
  scan. Preserve root identity, ownership, leases, writer custody and backing
  safeguards; allocation, eviction and OneUse slice rules stay outside this
  change. Independent design review must resolve the descendant checks the
  scan currently supplies, including mount boundaries and identity changes,
  before implementation is briefed. Tom's subsequent decision
  `23f2fa22-2a1c-4834-b74f-88ce008b4328` v2, "Implement reviewed warm reuse,"
  authorizes the independently reviewed Normal-only policy: Cargo may encounter
  descendant symlinks or nested mounts without a pregrant scan. Oxide remains a
  cooperative root-bound lease, not a descendant sandbox. Implementation and
  targeted verification proceed through the scoped Nucleus brief and independent
  review; the ruling itself establishes no runtime or QA readiness.
- **Internal pool:** the account cache directory's `build-pool`
  (`~/Library/Caches/oxide/build-pool` on macOS,
  `$XDG_CACHE_HOME/oxide/build-pool` elsewhere), with a 100 GB budget. An
  explicit `[pool] path` overrides it. Config follows Longhorn's
  storage-location policy, through Longhorn's own resolver.
- **The Builds drive** is the preferred pool:
  - a registered removable APFS volume, permanently attached and treated as
    consumable;
  - other tools may keep scratch on it outside `oxide-pool`.
- **Drive mode:** the Builds drive is registered as permanent or ephemeral,
  with a loud warning when a build falls back to the internal pool.
  `oxide pool describe --json` reports it to other tools.
- **Low space on the Builds drive** (built through Barge, not a second Oxide
  engine): the pool cap counts pool bytes only; caches, scratch and other data
  on the drive never count against it. The drive's registered reserve is a
  prune trigger, not a stop:
  below it, at admission and while builds run, Oxide evicts the
  least-recently-used idle whole slots on the drive until it is back above the
  reserve. Only a 10 GiB hard floor stops work: below it with no idle slot left,
  a new build refuses and a running build stops, with a clear message. Low
  space never sends a build to the internal pool.
- **Final outputs** stay checkout-local by default. Permanent drive mode adds
  owned output links: a fresh checkout with no `target` gets its first
  conversion on build, others by explicit adoption. Existing whole-target
  links migrate only by explicit adoption. It never moves existing output,
  and ephemeral mode is unchanged
  ([workflow](contracts/workflow.md)).
- **Profile layout becomes the default** (built by oxide#160 for S1 and S4b; migration of existing checkouts still gated): automatic first conversion creates profile member links, keeping `target` a real directory so container tools that mount it (Effigy's isolated `target` mounts) keep working, instead of the legacy whole-target link. `oxide exec` now derives a tool's output members from the checkout's committed placement, so no route needs `--output` ([design](contracts/exec-output-inference.md), which records that a member the checkout's committed placement does not cover stays a real local directory and is reported, never guessed); `--output` remains an optional complete override. Existing whole-target checkouts are untouched and migrate only by explicit `oxide outputs detach`
  followed by `oxide outputs adopt`, Acowtancy's primary checkout first — that migration, the design's S2/S3/S5 follow-ons and a real Tauri dogfood are not part of oxide#160.
- **Owned links at profile directories** (v1, operator accepted): a converted
  checkout keeps `target` itself a real directory and owns only the positively
  identified members `debug`, `release`, `<triple>/debug` and
  `<triple>/release`, so tools that mount `target` keep working. A legacy
  whole-target link switches through an explicit `oxide outputs detach`
  followed by `oxide outputs adopt`; the switch is a write-ahead receipt
  transition (`prepared`, sibling link, rename, `committed`, replayed after a
  crash), with no separate migration journal. A real nonempty profile
  directory is never moved or converted, and an undeclared opaque tool refuses
  instead of guessing. `oxide outputs detach` recognises the host triple and
  every other supported member link by its approved destination and this
  checkout's root marker, so a `<triple>/debug` member no longer refuses a
  safe detach, while an unowned, wrong-destination, dangling or malformed link
  still refuses with nothing changed. Detach, clean and prune are covered by
  ordinary regression tests. The contract, formats and evidence limits are
  owned by the [workflow](contracts/workflow.md).
- **Machine pool from the account:** Oxide resolves its config, pool and
  drive from the account home, not `$HOME`, so a harness that overrides
  HOME still uses them; private pools need an explicit setting. Its proof
  controls exist only in debug builds; any other build refuses them.
- **Temporary files off the internal drive:** a leased command's children
  (tests, run programs, build scripts, `oxide exec` tools) put temporary files
  on the selected pool. An inherited `TMPDIR` is kept, as its canonical
  directory, only when it is a writable directory on the pool's own device;
  external owners retain cleanup of their inherited paths and Oxide never
  writes to or removes one. Otherwise Oxide owns one short per-command
  directory inside the slot, removed at release after writer closure; a `run`
  program that uses one keeps the lease until its own group closes
  ([workflow](contracts/workflow.md)). Idle exec-session custody design is tracked
  in Nucleus and is not implemented. Proof and validation scratch placement is
  retired history: oxide#145 (merged at `0520fc04`, guard `a6d73326`) placed and
  monitored proof scratch on the selected pool's own volume with a warned
  `/private/tmp` fallback, and oxide#322 removed that proof ladder, its selectors
  and its shared supervisor, so nothing here now configures, selects or monitors
  proof scratch. Tests may still declare an explicit internal root as the
  owner-accepted narrow exception, with their fixtures beside the pool, never
  inside it. That exception and this temporary-file rule cover Oxide-owned
  scratch only: CI, Nucleus worktrees, containers and host settings stay as they
  are.
- **Leftover output roots** of checkouts that no longer exist are removed
  only by an explicit `oxide outputs prune`, never automatically.
- Tom's 2026-11-05 selected-root protection ruling and its conservative
  alignment boundary are recorded in the superseded controlled decision
  comparison and the [lean adoption contract](contracts/barge-adoption.md).
- **Sharing builds:** slots share warm builds across worktrees, up to 12 per
  project, with least-recently-used idle slots evicted under the byte budget.
- **Cold slots:** the chosen direction instead of sccache is seeding a cold
  slot by cloning matching dependency units from other slots on the same
  volume, with Cargo still judging freshness. It starts conservative:
  build scripts whose outputs record absolute paths rerun.
  Oxide bypasses inherited compiler wrappers and doesn't manage sccache.
  Cargo's own cross-workspace cache is the preferred long-term complement.
  The concrete protocol, its partial-association limit and its implementation
  record are in the [cold-slot seeding design](contracts/cold-slot-seeding.md).
- **Approved, not built:**
  - broader submodule-aware cross-worktree reuse beyond the narrow
    two-state coverage in the workflow contract.

  Details live in their Nucleus leads and, once built, in the
  [workflow contract](contracts/workflow.md).
- **Cold-slot seeding** is implemented and default-on; its scope and limits are in
  the [cold-slot seeding contract](contracts/cold-slot-seeding.md). Per-slot
  accounting and its trust limits are owned by [local lease](contracts/local-lease.md).

### Adoption pattern

- **How a project switches:** its Effigy selectors call `oxide`. Anything that
  owns its own target directory, or runs in a container, stays on Cargo. The
  full rules are in the
  [workflow contract](contracts/workflow.md#switching-an-existing-project).
- **The fallback to Cargo is Effigy's,** through a generic per-program
  fallback that Effigy owns and documents. It replaces per-selector `:cargo`
  escapes and project-side CI switches. Oxide ships no CI shim.
- **Projects switch by standing rule:** a Rust project switches when Nucleus
  next starts a task in it, and every project with a Paseo workspace is
  switching now. Each project's planner owns its change. Projects on Oxide
  are tracked in Nucleus. Nucleus adoption is off.
- **Only genuine Cargo uses stay on Cargo:** container or VM guest builds,
  CI, tools that insist on their own target dir, and tests of Cargo itself.

### Toolchain support

- **Supported range:** the target is stable Rust 1.95 and later.
  Version-sensitive behaviour is qualified per exact pinned toolchain.
  - The qualified pins are listed in the workflow contract's Cargo version
    matrix.
  - Newer or patch versions are not assumed to qualify.
  - Any unqualified version turns those features off rather than guessing.
- **Next upgrade point:** a Rust release newer than the pinned 1.100 is
  qualified separately before its layout or internals are assumed.

## Validation policy

Tom's 2026-11-17 lean reset ("simplify this across all three projects… without all the ceremony") is the current validation policy for all Oxide work: ordinary bounded unit/integration tests on private fixtures and one independent exact-head review. No native-proof, reviewer-native, diagnostic or qualification ladder is required. Installation and live migration remain separate owner/planner actions. Before installing a storage-affecting binary, time one real admission on the live pool (2026-11-18 incident, decision `d9f8839e`). The rulings below are history; their evidence keeps no transferred credit.

Earlier qualification, common-clock, supplier, capture, Relax-trial and
proof-ladder rulings are preserved in git history; their evidence carries no
transferred credit. Current validation and installation policy is stated above.

## Authority boundaries

- **Standing correction authority** (Tom, 2026-10-10, "Go for it. You have
  blanket approval, you don't need to keep asking me"):
  - covers routine Oxide-only corrections and equivalent bounded retries,
    through scoped Nucleus briefs and independent review;
  - never resets a running clock or enlarges caps silently;
  - doesn't turn uncertainty into release, or allow evading a host-tool
    denial.
- **Model routing:** briefs declare task complexity; Nucleus selects the author
  and reviewer models.
- **Still need Tom's scoped approval:**
  - host installs beyond the planner's receipt-matched `oxide` reinstall;
  - releases, tags and workflow changes;
  - settings or scheduler changes;
  - live cache or storage cleanup;
  - material scope changes.
- **Nucleus:** never prompt or change active Nucleus work.
- **Recording rulings:**
  - Nucleus decisions record Tom's exact words.
  - A ruling that changes current truth updates its owning knowledge file.
  - A purely operational ruling stays in Nucleus.
