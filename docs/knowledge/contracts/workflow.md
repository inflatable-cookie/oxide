# Oxide workflow

Status: product design under Tom's
[2026-10-18 workflow direction and bounded implementation authority](../vision.md#product-workflow-direction).
The foundation core is implemented as the unpublished, uninstalled `oxide`
binary; [Implemented foundation core](#implemented-foundation-core) states
exactly what ships. `init` / `new` adoption, the freshness gate, the agent
skill, non-build run strategies and a Linux backend are not implemented.
Fixture proofs are not real-project qualification, default-pool creation
outside tests, installation or consumer-adoption authority. Other sections
state the target design.

## Shape

`oxide` is Cargo with managed storage, profiles and execution. In an adopted
project, the agent guidance is one sentence: write `oxide` where you would
write `cargo`. Setup is one binary on `PATH`. There is no toolchain shim, `PATH`
shim, shell alias or project file.
[Interception](../rust-dx-landscape.md#cargo-command-interception) is deferred
until this foundation works consistently.

```text
oxide <check|test|clippy|build|run> [--profile <name>] [cargo args] [-- program args]
oxide <any other cargo command> [args]   # passed to Cargo, with storage where it builds
oxide status                             # pool, slots, waits, stray Cargo output
oxide init [cargo init args]             # make this a Cargo project if needed, then adopt Oxide
oxide new <path> [cargo new args]        # cargo new, then adopt Oxide
```

Commands fall into four classes:

| Class | Commands | Behavior |
| --- | --- | --- |
| Modelled | `check`, `test`, `clippy`, `build`, `run` | Profile-aware, leased, strategy-selected (see [profiles](#profiles-and-strategies)). |
| Building passthrough | Other built-ins that compile (`doc`, `bench`, `rustc`, `rustdoc`, `fix`) and every unknown or external subcommand (`nextest`, `llvm-cov`, `miri`) | Passed to Cargo unchanged, under a lease with storage and profile definitions injected. Unknown commands are treated as building, which is the conservative choice. |
| Plain passthrough | Non-building built-ins (`fmt`, `tree`, `metadata`, `add`, `remove`, `update`, `info`, `search`, `install`, `publish`, `login`, `vendor`, `fetch`, `generate-lockfile`, `locate-project`, `pkgid`, `version`, `help`) | Run as plain Cargo; no lease. |
| Extended built-ins | `init`, `new`, `clean` | Cargo's command plus Oxide's step (see [adoption](#adoption-oxide-init)). `clean` removes the checkout's private outputs and slot affinity and never deletes a shared slot: a 2026-10-18 probe showed plain `cargo clean` also deletes the injected build directory. |
| Oxide-owned | `status` | Oxide semantics. |

- A built-in Oxide extends keeps Cargo's meaning and adds to it. Purely
  Oxide verbs never reuse a Cargo built-in name.
- Arguments pass through unchanged. `--release` means `--profile release`.
  Oxide owns storage placement, so `--target-dir`, and `--config` keys that set
  `build.build-dir` or `build.target-dir`, are refused with an explanation.
- All injection uses environment variables (`CARGO_BUILD_BUILD_DIR`,
  `CARGO_TARGET_DIR`, `CARGO_PROFILE_OXIDE_*`), because child Cargo calls inherit
  them. A 2026-10-18 probe on 1.97.1 showed that a `--config` profile
  definition is invisible to an external subcommand's `$CARGO` call, while an
  environment-only definition works there.
- Built-in commands get `--profile <oxide name>` translated to the Cargo profile
  name. External tools receive arguments untouched. They name the Cargo profile
  themselves (for example `oxide nextest run --cargo-profile oxide-quick`), or
  they get Cargo's default profile in a separate unit set.

## Profiles and strategies

An Oxide profile is the user's intent. A strategy is how Oxide meets that
intent for one command. Names stay stable while strategies improve.

| Profile | Intent | `check` / `clippy` / `build` | `run` | `test` |
| --- | --- | --- | --- | --- |
| `quick` (default) | Fastest route to feedback on the current source | Quick build recipe | Fastest qualified execution strategy, falling back to quick build plus execute | Quick build plus execute, unless a strategy preserves test semantics |
| `debug` | Full debugger experience | Full-debug recipe | Full-debug build plus execute | Full-debug build plus execute |
| `release` | Production-shaped output | Project's own `release` profile | Release build plus execute | Release build plus execute |

Cargo recipes are defined through environment variables only, so no
`Cargo.toml` edit is needed. A 2026-10-18 probe confirmed that a custom profile
defined that way builds, works from child Cargo calls and inherits the
project's own `[profile.dev]` settings:

- `oxide-quick`: inherits `dev`, `debug = "line-tables-only"` for every unit.
  Per-package overrides (`package."*"`) cannot be expressed through the
  environment; the probe showed the dependency stayed at line tables. So this
  recipe keeps dependency line tables, unlike the `oxide-light` recipe in
  [stable Cargo profiles](stable-cargo-profiles.md). Whether dependency debug-off
  is worth a config file is part of [W-3](#open-decisions).
- `oxide-debug`: inherits `dev`, full debug for every unit, including build-time
  units through `CARGO_PROFILE_OXIDE_DEBUG_BUILD_OVERRIDE_DEBUG`.
- `release`: untouched.

`debug` cannot be a Cargo profile name because Cargo reserves it; the `oxide-`
prefix also avoids clashing with project profiles. The recipes are the
intended defaults, not measured speed or storage claims. That contract's
comparison stands as a no-go.

**Run strategies.** A quick run means "execute the current source as soon as
possible". Candidates are a Cranelift ahead-of-time backend, Cranelift JIT and
hot patching of a running process. Each is qualified per platform and project
shape before Oxide may select it; current state is in the
[landscape](../rust-dx-landscape.md#quick-run-strategies)
([Q-009](../questions.md#q-009--which-alternative-execution-modes-improve-the-daily-loop)).
Rules:

- Oxide prints the strategy it used and any semantic compromise, for example
  panic-abort under Cranelift on macOS.
- If a strategy cannot build or start the program, Oxide falls back to quick
  build plus execute. A program failure is reported, never retried under
  another strategy.
- `test` uses only strategies that preserve test semantics (unwinding,
  `should_panic`, harness behavior). Required validation keeps its own
  contract and authoritative backend.
- The codegen backend is part of the slot key, so strategies never share
  intermediates.

## Project identity without configuration

Identity chooses which warm slot to try:

1. Repository identity: the Git root commit(s) plus the workspace path within
   the repository. Worktrees and separate clones match; a fork also matches.
2. Shallow clones without the root commit fall back to the normalised `origin`
   URL.
3. Outside Git: the canonical workspace root, with no cross-checkout reuse.

The slot compatibility key is repository identity + Cargo profile + resolved
toolchain (`rustc -vV`) + target triple + codegen backend. The toolchain is
whatever rustup resolves for the project directory; Oxide does not select or
pin it.

## Cross-checkout freshness

Reusing one slot across checkouts is not automatically safe. Cargo 1.97.1
fingerprints workspace units by relative path and file mtime, and treats an
input whose `path_mtime <= reference_mtime` as fresh. Reviewed evidence
([qualification readiness](../qualification-readiness.md)) proved that a second
checkout reused the slot's units as fresh when its differing files were older
than the slot's last build. Cargo also omits `CARGO_MANIFEST_DIR` from encoded
dep-info, so units that embed the checkout path keep the previous checkout's
path.

Per-command leasing makes this ordinary. Example: worktrees A and B both exist
from time T0. A edits and builds at T1. B then builds on the same slot, and
B's differing files still carry mtime T0, which is earlier than T1. Nucleus's
current lease-at-placement model mostly avoids this by construction: a
worktree is created after the slot's last release, so its files are newer.
That is an inference from its lifecycle, not a qualification.

Proposed freshness gate (W-8), run before Cargo whenever the slot's last
compile origin differs from this checkout:

1. The slot records its last compile origin: checkout path, Git tree, dirty and
   untracked file digests, and the build anchor time.
2. Oxide diffs the two content sets with Git. Each differing file whose mtime is
   not newer than the anchor gets its mtime set to now, so Cargo's own
   invalidation rebuilds it. Content stays untouched; only the checkout's own
   files change.
3. When the checkout path changes, workspace packages that can embed it get the
   same treatment: those with a build script, or whose source mentions
   `CARGO_MANIFEST_DIR`. Registry and Git dependencies live at shared paths and
   stay warm. They are the bulk of the reuse benefit.
4. Inputs outside the repository or ignored by Git are not covered; their
   limits stay explicit.

This revives the qualification work's rejected "source touching" fix as a
deliberate product mechanism. It needs a fresh qualification before
cross-checkout reuse is a default. Without the gate, Oxide must keep slots
checkout-affine, which keeps correctness but loses most of the worktree
benefit.

Debug paths are a separate, known limit. Reused workspace units carry the
compiling checkout's absolute paths in debug info. That is tolerable for
`quick` backtraces, but wrong for `debug` sessions after that checkout is gone.
Stable path remapping needs identical flags across checkouts to avoid
fingerprint churn. The retired source-map work found no cheap general fix.

## What one command does (local executor)

1. Resolve the workspace (`cargo locate-project --workspace`), identity,
   profile, strategy and toolchain.
2. Lease a slot from the host pool. Prefer this checkout's last slot, then any
   idle compatible slot, then a new slot if the per-project cap and budget
   allow. Otherwise wait in order, with a visible "waiting for …" line.
3. Run the freshness gate when the slot's last origin differs.
4. Run the real Cargo as the leader of a new process group, with:
   - the build directory pointed at the slot;
   - the target directory set to `<checkout>/target`, so final outputs stay
     private, as in the [local lease](local-lease.md);
   - the profile definitions in the environment.
5. `run`: keep the lease for the build, release it, then execute the binary
   from the private target directory. `test`: keep the lease through test
   execution, because test binaries live in the build directory. Reuse never
   skips tests.
6. When the process group exits, record the compile origin, release the lease
   and print one line: profile, strategy, warm/cold, elapsed time, pool use.

The [qualified cross-worktree result](project-adoption.md#focused-cross-worktree-result)
supports this shape for one workflow: a different-path checkout on a shared
slot rebuilt only its changed unit, and tests ran every time. It did not isolate
the mtime hazard above.

## Crash recovery

Tom ruled on 2026-10-18 that Oxide must manage crash recovery itself
([vision](../vision.md#product-workflow-direction)). Proposed mechanism:

- While a command holds a lease, it also holds an exclusive `flock` on a
  per-slot lock file. The kernel drops that lock when the process dies for any
  reason. Oxide's control state records the command phase and the Cargo
  process-group ID; the lease ledger is unchanged.
- Ctrl-C and termination signals are forwarded to the group. Oxide waits for
  the group to exit, then releases normally.
- A later command that finds the lease held checks two things: whether the lock
  can be taken, and whether the process group is empty (`killpg(pgid, 0)`
  returns `ESRCH`). If both hold, it recovers the slot automatically, records
  the recovery and continues. Cargo already tolerates interrupted builds: a
  unit whose fingerprint was not written is rebuilt.
- A surviving group member, or a reused group ID, keeps the slot held. Those
  failures are conservative. A descendant that escaped the group with `setsid`
  is the accepted residual risk.
- A command that dies after recording "Cargo starting" but before recording
  its group cannot prove the group empty. That slot stays held, `oxide status`
  marks it interrupted, and `oxide-storage-lease recover` is the explicit
  release.

This replaces the local lease's "never infer abandonment" rule for Oxide's
own commands. Consumers that lease per task, such as Nucleus, keep their
explicit `recover` unless they adopt it.

## Storage and space management

- One per-user host pool: the existing [bounded APFS backend](bounded-storage.md)
  on macOS. The volume size is the hard aggregate ceiling. Only whole idle slots
  are evicted.
- The first command creates the pool automatically ([W-1](#open-decisions)).
- Per-project slot cap, recommended 2 to start, plus a host-wide limit on
  concurrent Cargo invocations ([W-2](#open-decisions)). Queue/Nucleus still
  decide which tasks run. Oxide only orders the Rust work they launch.
- `oxide status` reuses the read-only [storage inventory](storage-inventory.md).
  It reports pool and slot use plus stray output: a checkout `target/` holding
  `deps/` or `incremental/` was built by plain Cargo. Reclaiming that space is
  an explicit command; live cleanup still needs scoped approval.

## Execution location

Every command becomes a request handed to an executor:

- Request: repository identity, source snapshot (commit plus uncommitted diff,
  or the local path), command, profile, strategy, Cargo arguments, target
  triple, environment allowlist.
- Result: streamed Cargo diagnostics in the requested format, exit status, test
  results and, for `build`/`run`, an artifact manifest.

The foundation ships only the local executor. Remote kinds (LAN host over SSH,
CI, cloud) select through per-user settings or `--on <name>`, never project
files. For `run`, build and run locations are independent. Remote execution
must not become required infrastructure for the core experience
([landscape](../rust-dx-landscape.md#experimental-frontier)).

## Agent integration

1. **Oxide agent skill.** The rule is "`oxide` wherever you would write
   `cargo`". The skill also covers profiles, the reported strategy, waits and
   refusals, and `oxide status`.
2. **Project guidance block.** `oxide init` writes one marked section in the
   project's `AGENTS.md`, a few lines long. The block is the opt-in marker; no
   config file exists.
3. **Effigy selectors** in Effigy projects call `oxide …`. Effigy stays
   language-neutral.

Plain Cargo stays harmless: it builds privately, as today. `oxide status` makes
drift visible. rust-analyzer keeps its own private check target unless the user
routes `check.overrideCommand` to `oxide check --message-format=json`; editor
routing is outside the foundation.

## Adoption: `oxide init`

`oxide init` is a superset of `cargo init`, so one command takes a directory
to an Oxide-adopted Rust project:

1. If the target directory has no `Cargo.toml`, run `cargo init` with the
   supplied arguments (`--lib`, `--name`, `--edition`, `--vcs`). Inside an
   existing workspace, Cargo adds the new member as it normally does.
2. If a `Cargo.toml` already exists, skip Cargo. Cargo-only arguments then
   refuse with an explanation instead of being silently ignored.
3. Resolve the workspace root and write or refresh the marked Oxide block in
   the root `AGENTS.md`, creating the file if absent. Re-running is
   idempotent and never touches text outside the markers.
4. Print readiness: resolved toolchain, inferred project identity, pool state,
   and any Effigy Rust selectors that still call plain `cargo`. Oxide reports
   those; it does not edit them.

`oxide new <path>` is `cargo new` followed by steps 3 and 4. Neither command
creates an Oxide config file or changes `Cargo.toml` beyond what Cargo itself
writes.

## Relationship to existing work

Tom approved retiring the earlier machinery on 2026-10-18, on the condition
that nothing of distinct value is lost. oxide#070 executed it;
[retirements](../retired.toml) lists the removed terms, paths and features.
Assessment:

| Keep | Why |
| --- | --- |
| [Local lease](local-lease.md), pool, APFS backing and [bounded storage](bounded-storage.md) | The storage layer of every `oxide` command; live with Nucleus. |
| [Storage inventory](storage-inventory.md) | Backs `oxide status` and stray detection. |
| [Stable profiles](stable-cargo-profiles.md) knowledge | Source of the `quick` / `debug` recipes. |
| Cargo freshness, path-embedding, containment and capacity findings in [qualification readiness](../qualification-readiness.md) | Ground the freshness gate, debug-path limit and crash-recovery design. Kept as evidence; the narrative is condensed. |
| [Build-context policy](build-context-policy.md) | Candidate input for per-command incremental/debug defaults; stays proposed. |
| Bounded dep-info decoder (`src/dep_info.rs`) | Per-unit Cargo input lists for the W-8 gate; separable from the retired conversion. |

| Retired | Why it has no distinct value now |
| --- | --- |
| Protected ordinary-Cargo route, storage daemon, transport/codec, HMAC attempts and handoff, `oxide-storage-runtime`, `oxide-qualified-runner`, `oxide-ordinary-cargo-attempt` | Nucleus rejected the signed callback shape; the lean lease replaced it. |
| Declared-workflow front end (`oxide-declared-workflow`) | Superseded by zero-config identity and the `oxide` commands. |
| Registered runner/host bridge, unit-graph and fingerprint collectors | Evidence infrastructure for a route no longer pursued. |
| Registry archive verifier/acquisition (`oxide-registry-acquisition`) | Registry deps sit at shared paths; Cargo already verifies archives. |
| Source-map qualifier and probes | Their finding (no cheap stable remap) is recorded above; the code serves no remaining route. |

The mtime-stale content-gate implementation (`src/project/freshness.rs`) did
not separate from the retired adapter: it was bound to the fixed source-map
fixture, the resolver and the daemon codec. It was removed; W-8 reimplements
the gate from the retained [Cargo freshness findings](../qualification-readiness.md#cargo-freshness).

The bounded Cargo dep-info decoder did separate. Its `decode` and plain decoded
types never depended on the registered route; only the `into_registered`
conversion did. The decoder now lives at `src/dep_info.rs` with its
hand-annotated upstream-bytes and refusal tests, unchanged limits (16 MiB
input, 32768 references, 2048-byte strings) and its own two-variant refusal.
The conversion was retired. It is not called by any command yet; W-8 may use
its per-unit file and environment lists to find inputs outside Git.

## Foundation slice

The local executor on macOS, with: the command classes above; the three
profiles and the build fallback for every strategy; inferred identity;
per-command leasing with waits and crash recovery; the freshness gate (or
checkout affinity until it qualifies); automatic pool creation; `oxide status`
with stray detection; the skill and `oxide init` / `oxide new`. It is working consistently
when a second worktree's first `oxide test` on a real project reuses dependency
units. Tests must run, the stale-mtime scenario must rebuild, the pool must
stay under its ceiling, and normal use must leave no plain-Cargo intermediates.
Re-evaluate interception and run strategies after that.

## Foundation qualification (proposed)

Proposed and not authorized. This is the gated real-project step after the
foundation tasks merge. Its job is to prove correctness and lifecycle on a real
workspace. A benefit claim is not part of it.

**Subject.** Oxide's own repository at a pinned post-foundation commit, so no
consumer coordination is needed. It is a real Cargo workspace with registry
dependencies but a small graph (about 39 lock entries), which makes it good for
correctness and weak for benefit. A benefit reading on a heavy project (for
example Nucleus) is a separate, owner-coordinated second stage.

**Setup.**
- Two disposable Git worktrees, A and B, of the pinned commit under one scratch
  root.
- One fresh dedicated pool, not the default path, small enough to keep the host
  above its 20 GiB reserve.
- Cargo runs `--offline` against the already-populated host registry.
- One disposable patch adds a marker test that reads a checkout-local fixture
  and `CARGO_MANIFEST_DIR`, so a stale result is observable.

**Scenarios, in order, each through `oxide` only:**

| # | Action | Must hold |
| --- | --- | --- |
| Q1 | `oxide test` in A | Cold; tests run and pass. |
| Q2 | `oxide test` in B (worktree created before Q1) | Registry dependency units fresh; the gate rebuilds the path-embedding units; B's marker test sees B's paths and content. |
| Q3 | Edit the marker fixture in A, `oxide test` in A, then `oxide test` in B without touching B | The stale-mtime scenario: B rebuilds the affected units and its marker test sees B's unchanged content, not A's edit. |
| Q4 | Repeat `oxide test` in B | Nothing rebuilt; tests run. |
| Q5 | SIGKILL `oxide` during a build in A, then `oxide test` in A | The next command auto-recovers the slot; the build completes; tests pass. |
| Q6 | Plain `cargo check` in B, then `oxide status` | The stray `target/` intermediates are reported with their bytes. |
| Q7 | `oxide status` at the end | Pool under its ceiling, every lease released, no held or uncertain slot. |

**Evidence.**
- Cargo JSON fresh/rebuilt rows per scenario.
- Elapsed time.
- Slot and pool bytes, checkout `target/` bytes and host free space at each
  boundary.
- The `oxide` summary lines.

Raw output is preserved. Readings are observations, not speed or SSD claims.

**Bounds to propose with the brief.** About nine `oxide` invocations, 30
minutes including setup, and 20 minutes of independent review that checks the
raw evidence instead of repeating the run. No consumer, default pool,
installation or host configuration change.

**Pass.** Q1–Q7 all hold. Any stale success in Q2 or Q3 is a hard fail and
keeps slots checkout-affine. A failure in another scenario is reported with its
evidence and does not reset bounds.

## Implemented foundation core

The unpublished `oxide` binary (`src/bin/oxide/`, `publish = false`) runs the
local executor on macOS. Its covering proof is `effigy rust:oxide-cli`: tiny
offline fixture crates on one fresh 128 MiB APFS pool. It is not installed and
has run against no real project.

- **Classes.** As in [Shape](#shape), except `init` / `new` stay plain
  passthrough until adoption lands. Plain passthrough replaces Oxide with
  Cargo, so output and exit codes are Cargo's. Unknown commands are building
  passthrough. A leading `+toolchain` or Cargo flag before the command
  refuses; `--version`, `-V` and `--list` pass through.
- **Profiles.** `quick` (default) and `debug` translate to `oxide-quick` and
  `oxide-debug`; `release` and `--release` use the project's `release`. Both
  recipes are always defined in the environment, so external tools can name
  either. Building built-ins translate `--profile` only when given; external
  subcommands receive arguments untouched.
- **Refusals.** `--target-dir`, `--config` keys `build.build-dir` /
  `build.target-dir` (any quoting or `build = {…}` form) and every `--config`
  file path refuse, because a file could set either key.
- **Identity and key.** Git root commits plus the workspace prefix; shallow
  clones use the normalised `origin`; otherwise the canonical path. The slot
  key adds the Oxide profile, the `rustc -vV` digest, the target (`--target`,
  then `CARGO_BUILD_TARGET`, then host) and the default backend. Building
  passthrough uses the quick key unless a built-in names another profile;
  Cargo's per-profile subdirectories keep profiles apart inside one slot.
- **Checkout affinity.** A slot is reused only by the same canonical checkout
  with the same key. Cross-checkout reuse waits for the freshness gate (W-8).
- **Pool.** `OXIDE_POOL`, else `~/Library/Caches/oxide/pool`, created on first
  building command with a 24 GiB cap equal to its sparse image. Creation
  refuses below 20 GiB host free space. An existing pool from
  `oxide-storage-lease init` opens unchanged.
- **Slots and waiting.** Per-project cap 2: a new checkout or key at the cap
  evicts the project's least recently used idle slot whole and builds cold.
  Host-wide limit 2 commands holding slots (`OXIDE_MAX_ACTIVE`, 1–32). Excess
  commands wait in FIFO order, print one `oxide: waiting for …` line on stderr
  and cancel on Ctrl-C with exit 130. Another command in the same checkout
  and key waits too.
- **Execution.** Cargo runs from the caller's directory as leader of a new
  process group, with `CARGO_BUILD_BUILD_DIR` on the slot and
  `CARGO_TARGET_DIR=<workspace>/target`. SIGINT, SIGTERM and SIGHUP are
  forwarded to the group; Oxide releases only after the group is empty. `test`
  holds the slot through execution. `run` builds, releases, then replaces
  Oxide with the program, setting `CARGO_MANIFEST_DIR` only; other `cargo run`
  environment (`CARGO_PKG_*`, dylib paths) is not set, and several built
  binaries need `--bin`. Exit codes are Cargo's, or the program's for `run`.
- **Summary.** One stderr line per leased command: profile, strategy (`build`
  or `build+execute`), slot, warm/cold, elapsed time and payload volume use.
- **`clean`.** Runs `cargo clean` against the private target directory and an
  empty private build directory, then drops this checkout's affinity. Slots
  stay intact until evicted, so the next build is cold.
- **Crash recovery.** Per [Crash recovery](#crash-recovery), for Oxide's own
  commands only. A follower in the same checkout waits while the crashed
  command's group survives, then recovers and continues.
- **`status`.** Pool path or "not created", cap, payload use, slot accounting
  coverage, each slot's state, profile and checkout, waiting commands, and
  whether the current workspace's `target/` holds plain-Cargo `deps/` or
  `incremental/`. It never creates the pool.

Control state lives in `<pool>/oxide-managed/` (owner-only): records mapping
each checkout and key to a private lease project directory, FIFO wait
tickets, an admission lock and one lock per record. The lease ledger,
receipt and standalone CLI contract are unchanged; see
[local lease](local-lease.md#oxide-front-door-control-directory).

## Open decisions

- **W-1 Default pool.** Foundation choice above: per-user cache directory,
  24 GiB sparse image, refuse below the 20 GiB host reserve. Revisit with real
  use.
- **W-2 Waiting and concurrency.** Foundation choice above: per-project cap
  2, host-wide limit 2, strict FIFO waiting, idle-slot eviction before
  waiting. Coordinates with
  [Q-011](../questions.md#q-011--how-should-slot-waiting-coordinate-with-host-admission).
- **W-3 Project file.** Needed only for per-project profile overrides or
  dependency debug-off. Recommendation: none until a real project needs it.
- **W-4 Crash recovery.** Direction ruled and implemented for Oxide's own
  commands; the fixture proof covers a killed command whose group survives.
  Qualification, including a group-escape test, remains.
- **W-5 Retirement execution.** Done in oxide#070; see
  [retirements](../retired.toml).
- **W-6 Linux backend.** CI, LAN boxes and Colima need a pool backend without
  APFS. A directory budget-only mode must say so honestly.
- **W-7 Command name.** `oxide` collides with Oxide Computer Company's CLI
  ([Q-005](../questions.md#q-005--what-names-are-safe-to-publish)). The design
  is name-independent.
- **W-8 Freshness gate.** The foundation core ships checkout-affine slots.
  Recommendation: build and qualify the gate next. Without it the main
  worktree benefit is unavailable.
