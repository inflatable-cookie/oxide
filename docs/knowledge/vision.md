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
- **Diagnostic and adoption direction** (Tom, 2026-10-23): default
  `oxide status` should be fast and read-only, showing recorded sizes with
  their age and source, or "not measured". Explicit `status --measure`
  requests bounded measurement; observation must not reconcile deletions.
  These diagnostic facts do not decide admission or reopen cached accounting.
  In a Git repository, `oxide init` should put one managed block in the
  work-tree root's `AGENTS.md`, listing the Cargo workspaces it covers.
  Existing nested blocks are reported and retained; outside Git the current
  workspace-local behavior stays. These are approved correction targets,
  not claims about the installed binary.
- **Crash recovery:** Oxide recovers its own interrupted commands. A
  recovery requires a free record lock and matching lease ownership, plus
  either an empty Cargo process group (`ESRCH`) or a recorded pre-launch
  state. An unknown or `EPERM` holder stays held.
- **`oxide exec -- <tool>`** runs Cargo-wrapping tools such as the Tauri CLI
  with Oxide storage (#107). When a checkout's slot is busy, a further command
  in that checkout takes a compatible spare slot. Whether Tauri honours
  build-dir is still unverified.
- **Command time budgets** (Tom, 2026-10-23): approved direction is separate
  finite admission and child-execution budgets, with a typed child-start
  boundary. Independent design review precedes implementation. Compilation,
  Cargo locks and nested admissions consume the execution budget once it
  starts; nested calls never pause or reset an outer budget. Preserve signal
  forwarding and conservative group closure. Consumer adoption and installation
  remain separate gates; this is not an installed capability.
- **Nested `oxide` joins the outer lease** (approved, not built; design first):
  a leased command passes its children a lease token that cannot be forged and
  is scoped to its own process subtree. A nested `oxide` for the same
  workspace, recipe and toolchain joins that lease: no new slot or permit, and
  the outer closure and custody apply. One for another workspace gets its own
  slot but borrows the outer host permit, so nesting cannot deadlock the host
  limit. A stale or foreign token is ignored and never grants deletion.
  Today a nested call takes an independent lease. The
  [proposed design](contracts/nested-lease.md) records the token, ticket,
  permit, deadlock and idle-exec composition rules; it is design-reviewed but
  not frozen and changes nothing until a briefed implementation lands.
- **Idle exec sessions release their host permit** (approved, not built;
  design first): `oxide exec` keeps its slot for the whole session but holds
  the host active-build permit only while a compile runs in its group, taking
  it back before the next compile, so open dev servers such as `tauri dev` do
  not starve builds. Closure, crash custody and the reserve watchdog are
  unchanged. Whether a compile can be detected before it starts is a design
  question. Today an exec session holds its permit throughout. The
  [proposed design](contracts/idle-exec-sessions.md) records the interception
  analysis, the supported boundary and the open operator dispositions.
- **Paused:** the unchanged-source `oxide run` fast path. Its plan is preserved
  in #089's closed task and its committee contract.
- **Deferred:** remote builds. The executor seam stays local-only.

### Storage

- **Barge owns drive management** (approved, staged): volume registration and selection, placement and fallback, owned roots, pruning and scratch move to the separate Barge product ([inflatable-cookie/barge](https://github.com/inflatable-cookie/barge)), which Oxide will use as a library. Oxide keeps Cargo, slots, leases, recipes, freshness and `exec`. Until each part moves, Oxide's current implementation stays authoritative.
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
  targeted verification proceed through the scoped Queue brief and independent
  review; the ruling itself establishes no runtime or QA readiness.
- **Internal pool:** `~/Dev/oxide`, with a 100 GB budget. Config follows
  Longhorn's storage-location policy, through Longhorn's own resolver.
- **The Builds drive** is the preferred pool:
  - a registered removable APFS volume, permanently attached and treated as
    consumable;
  - other tools may keep scratch on it outside `oxide-pool`.
- **Drive mode:** the Builds drive is registered as permanent or ephemeral,
  with a loud warning when a build falls back to the internal pool.
  `oxide pool describe --json` reports it to other tools.
- **Low space on the Builds drive** (approved, not built; delivered through Barge stage 3, not in Oxide): the pool cap counts
  pool bytes only; caches, scratch and other data on the drive never count
  against it. The drive's registered reserve is a prune trigger, not a stop:
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
  ([workflow](contracts/workflow.md#final-outputs-on-the-permanent-drive-oxide117)).
- **Profile layout becomes the default** (built by oxide#160 for S1 and S4b; migration of existing checkouts still gated): automatic first conversion creates profile member links, keeping `target` a real directory so container tools that mount it (Effigy's isolated `target` mounts) keep working, instead of the legacy whole-target link. `oxide exec` now derives a tool's output members from the checkout's committed placement, so no route needs `--output` ([design](contracts/exec-output-inference.md), which records that a member the checkout's committed placement does not cover stays a real local directory and is reported, never guessed); `--output` remains an optional complete override. Existing whole-target checkouts are untouched and migrate only by explicit `oxide outputs adopt`, Acowtancy's primary checkout first — that migration, the design's S2/S3/S5 follow-ons and a real Tauri dogfood are not part of oxide#160.
- **Owned links at profile directories** (v1, operator accepted): a converted
  checkout keeps `target` itself a real directory and owns only the positively
  identified members `debug`, `release`, `<triple>/debug` and
  `<triple>/release`, so tools that mount `target` keep working. An existing
  whole-target link moves only through an explicit `oxide outputs adopt` under
  a durable journal, a real nonempty profile directory is never moved or
  converted, and an undeclared opaque tool refuses instead of guessing. The
  profile layout is implemented; `oxide outputs detach` recognises the host
  triple and every other supported member link by its approved destination
  and this checkout's root marker, so a `<triple>/debug` member no longer
  refuses a safe detach, while an unowned, wrong-destination, dangling or
  malformed link still refuses with nothing changed. Corrected native
  coverage of detach, clean and prune is still unactivated. The as-is
  acceptance, contract, formats and evidence limits are owned by the
  [workflow](contracts/workflow.md#owned-output-members-at-profile-directories-oxide133).
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
  ([workflow](contracts/workflow.md#child-temporary-files-oxide136)). The
  [proposed idle-session design](contracts/idle-exec-sessions.md) weighs the
  successor custody alternatives and keeps this guarantee; it selects none of
  them. It is
  implemented on the oxide#136 branch but is not installed or qualified: its
  counted native has not passed and both uncounted diagnostics were attempted
  and failed. The separate proof and validation tooling
  default is implemented and qualified by oxide#145 (merged at `0520fc04`,
  guard `a6d73326`), which places and monitors proof scratch on the selected
  pool's own volume with a warned `/private/tmp` fallback
  ([placement](contracts/workflow.md#temporary-scratch-placement)). Internal-
  placement proofs may declare an explicit internal root as the owner-accepted
  narrow exception, with their fixtures beside the pool, never inside it. Both
  cover Oxide-owned scratch only: CI, Queue worktrees, containers and host
  settings stay as they are.
- **Leftover output roots** of checkouts that no longer exist are removed
  only by an explicit `oxide outputs prune`, never automatically.
- Tom's 2026-11-05 selected-root protection ruling and its conservative
  alignment boundary are owned by the
  [controlled decision comparison](contracts/barge-adoption.md#controlled-decision-comparison).
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

  Details live in their Queue leads and, once built, in the
  [workflow contract](contracts/workflow.md).
- **Implemented, native held: cold-slot seeding** (oxide#169): a fresh slot
  is warmed once from matching plain registry library compile units of an idle
  donor on the same volume, removable pools included, with Cargo still judging
  freshness; default-on, with natives held. The
  [design](contracts/cold-slot-seeding.md) owns the protocol and
  `effigy rust:cold-slot-seeding` its 24-case proof.
- **Implemented, native held** (Tom, 2026-10-25): recorded per-slot accounting
  replaces whole-payload admission walks. Closed-slot records are bound to
  generation/incarnation; missing or invalid records require bounded slot
  walks, and cross-slot hard-link overcount is accepted. A removable pool's
  cap counts only its own bytes — another tool's data on the same volume never
  pushes it over the cap — while the volume's free space is a separate reserve
  signal: above the reserve it admits without eviction, at or below it prunes
  eligible idle whole slots, and a fixed 10 GiB hard floor decides once nothing
  is left to prune. Source and unit regressions are in;
  the focused proof and its elevated natives are held for separate owner
  activations. The [accounting contract](contracts/local-lease.md#recorded-per-slot-accounting-oxide146-implemented-native-held)
  owns the design, the implementation record and its trust limits. This supersedes the earlier cached
  accounting decline for this scope; its negative reviews remain history.

### Adoption pattern

- **How a project switches:** its Effigy selectors call `oxide`. Anything that
  owns its own target directory, or runs in a container, stays on Cargo. The
  full rules are in the
  [workflow contract](contracts/workflow.md#switching-an-existing-project).
- **The fallback to Cargo is Effigy's,** through a generic per-program
  fallback that Effigy owns and documents. It replaces per-selector `:cargo`
  escapes and project-side CI switches. Oxide ships no CI shim.
- **Projects switch by standing rule:** a Rust project switches when Queue
  next starts a task in it, and every project with a Paseo workspace is
  switching now. Each project's planner owns its change. Projects on Oxide
  are tracked in Queue. Nucleus adoption is off.
- **Only genuine Cargo uses stay on Cargo:** container or VM guest builds,
  CI, tools that insist on their own target dir, and tests of Cargo itself.

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

Tom decision `ea8ee47f-adc0-4cd3-b420-aaa30642d623` v2 (2026-11-16)
adopts the reviewed [cooperative qualification contract](contracts/cooperative-qualification.md)
as the acceptance target for future 309 work and scopes one independently
reviewed Nucleus-owned route proposal, without implementation or proof runs.
The earlier `57374e05` v2 decision authorized drafting and review only. Those
drafting and adoption facts remain accurate at their own time. Tom's later
decision `5639bd34-635f-49b4-8261-92903d1013b4` v2 (2026-11-16, "Retain and
count image evidence") records the future image-backing policy in that same
contract: retain and count image files as evidence, and require positive
ordinary provider evidence that every mount and attached entity of each exact
owned image is absent. The earlier literal entire-backing-object absence gate
is superseded prospectively only. The adoption and the 5639v2 ruling
establish no route, readiness, qualified provider or proof activation and
change no existing execution instruction. Existing 309/061/304 briefs and
dispatch and merge holds remain binding; later activation requires
prospective amendment.

Tom decision `c6e3e9c8` v2 (2026-11-04) separates capture delivery from
private launcher qualification for the bounded O2 capture tasks. Their named
capture-only Cargo selectors cover build and embedded harness behavior; they
do not qualify launcher isolation, global live-state invariance or current
export production. Ordinary Rust selectors continue to use Oxide. Direct
covering requires the separately activated, qualified-guard route in the
[barge adoption contract](contracts/barge-adoption.md#oxide-test-only-capture-apparatus).

Tom, 2026-10-20: risk-proportionate verification. AGENTS.md "Validate" holds
the operating rules.

- **Standard tier:** author checks and native proof, plus an independent source
  review.
- **Elevated tier,** for deletion or recycling, crash recovery or writer
  closure, and removable media: adds an independent reviewer native session.
- **Bounds:** wall-clock time and scratch budget. Resource figures are
  reported, not gated.
- **Proof authoring guidance** (product architect, 2026-10-24; diagnostic
  requirement strengthened 2026-10-25): briefs that author or correct a native
  proof script, its fixtures or its oracles declare **HIGH** complexity. Queue
  still selects the models. Before the first counted native, the author must
  execute a separately activated, **uncounted diagnostic** of the fixed proof.
  Use its existing safe keep-going mode; if none exists, add a minimal opt-in
  `--diagnostic` that records oracle failures and continues through
  dependency-independent safe cases. A source sweep alone no longer satisfies
  this gate. Apply this before the next counted runs of #136 and #140 and to
  every future proof.
  - Keep the counted default fail-fast and preserve the qualified guard,
    existing wall-clock and scratch bounds, custody and cleanup. Record
    dependency or taint skips honestly; infrastructure faults, unknown device
    or writer effects, uncertain custody and an unrecordable receipt abort.
    Do not invent fixture repair or presume a reaped child proves closure.
    If no safe diagnostic can fit these constraints, return the concrete
    design issue to the owner before a counted run.
  - Every brief also applies the fixed [proof-authoring premise checklist](contracts/workflow.md#proof-authoring-premise-checklist)
    to the executable case bodies and helpers, recording each disposition or
    reason it does not apply. This source sweep complements the execution;
    it does not replace it.
  - The diagnostic receipt is explicitly uncounted and never a native PASS,
    closure claim or substitute for the counted proof. It records all reached,
    completed, failed, skipped and unexecuted cases, reasons and first fault.
    Its execution needs its own exact source/input pins, supported route,
    resource basis and fixed activation window. This guidance grants no
    automatic run, cap increase, clock reset or reuse of a spent allowance.
    Earlier counted failures remain counted failures, never retrospectively
    relabelled as diagnostics. Changed proof inputs need the applicable
    diagnostic and review before their counted activation.
- **Mechanical native-evidence gates** (Supervisor, 2026-10-23): every Oxide
  brief requiring counted native evidence encodes it in Queue before
  promotion. Use `reviewVerdicts: true` with a `reviewPassGate` subject such
  as `exact_head_native_evidence`, and/or a `mergeHold` pending that evidence.
  An owner releases the hold only after verifying the required receipts and
  their exact source heads. Prose alone is insufficient; a source-only review
  does not satisfy a required native proof. The brief still names the separate
  activation, count, time and scratch limits.
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
- **Shared recovery simplicity** (Tom, 2026-11-10, decision
  `1d4a7aee-a8e6-4a75-96f0-b142c4176db2` v2): Oxide and Barge must both
  become simpler and dependable as integrations grow. Investigate one
  Barge-owned operation journal with durable caller hooks before extending
  the two-journal integration. The source-only design and independent review
  must identify machinery removed from both products and preserve admission,
  crash recovery and safe repeated finalization. The architecture remains a
  proposal until the implementation ruling below; existing work and evidence
  stay preserved.
- **Single-authority recovery implementation** (Tom, 2026-11-11, decision
  `1735df31-7cc1-4748-9f8a-0767ee6259ff` v2): implement the independently
  reviewed Barge stage 4 operation-journal design on disposable fixtures.
  Barge owns operation progress and durable hook obligations; Oxide keeps
  eligibility and repeatable domain settlement. Scope the journal integrity,
  coordinator and caller removals separately, with independent review and
  bounded Elevated qualification. The selected design includes retained
  cancellation, atomic settlement entries and fail-fast journal consults
  under admission, with their refusal and retention costs. Native sessions
  remain separately owner-activated. This grants no installation, live
  migration or adoption, retained-resource cleanup or supervisor promotion.
- **Barge descriptor-listing design** (Tom, 2026-11-10, decision
  `9e68c8ce-ac61-4190-a31c-b45f99fd6ecd` v2): investigate a bounded shared
  listing within the existing total descriptor, process and time budgets,
  through source-only design and independent review. The implementation ruling
  below is separate.
- **Barge descriptor-listing implementation** (Tom, 2026-11-10, decision
  `13834566-5b7f-44be-a655-6444a4eeaa32` v2): implement the reviewed bounded
  listing design and qualify it on disposable fixtures, preserving the existing
  total scan ceilings. Scoped implementation, independent review and separately
  activated Elevated proofs precede the Oxide reconnection. This grants no
  installation, live adoption or retained-image cleanup.
- **Relax quick-run trial** (Tom, 2026-11-11, decision
  `d6a33913-cc2d-460f-bce0-25c424c176d1` v2, "Agreed. OK, shall we dispatch
  the trial? See if it offers more than the cranelift spike?"): a bounded
  compatibility/performance trial and recommendation, not product
  integration or adoption. Oxide owns execution, admission, storage and
  diagnostics; the trial embeds no compiler, changes no
  quick/test/debug/release default, installs nothing and keeps the Barge
  integration independent. Owning research record:
  [landscape](rust-dx-landscape.md#relax-quick-run-trial-2026-11-11-prepared).
- **Relax compatibility recheck** (Tom, 2026-11-12, decision
  `7b5a88ce-96cb-4fab-a584-2e5deffa28ce` v2, "Go for it"): a focused,
  independently reviewed recheck of a NEWER pinned Relax engine on actual
  float formatting and the unchanged prior Oxide workload, to determine whether
  the observed ARM64 intrinsic blocker is cleared. The 307 no-go for its own
  pin stays exactly as recorded. One prepared apparatus PR may land with
  effect-free readiness; an uncounted diagnostic and then one counted author
  compatibility qualification each need separate planner activation at an
  accepted published head, and a source or fit correction needs its own
  review and activation. Any performance comparison stays conditional on
  compatibility passing and needs a new measurement activation. The three
  spent uncounted diagnostics produced a scoped no-go at `6ea9d063`: the
  float-formatting and CRC32 probes match the Cargo baseline while the
  unchanged workload panics in Relax's front end, so the counted
  qualification was not activated and no performance comparison is
  justified. No product
  integration, engine fork, installation, default change or consumer adoption
  is authorized by a pass. Owning research record:
  [landscape](rust-dx-landscape.md#relax-compatibility-recheck-2026-11-12).
- **Still need Tom's scoped approval:**
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
