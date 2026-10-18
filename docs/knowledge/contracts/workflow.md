# Oxide workflow (proposed)

Status: proposed product design under Tom's
[2026-10-18 workflow direction](../vision.md#product-workflow-direction). Nothing
here is implemented, installed or authorized for implementation. It sets the
target shape that future Queue briefs build toward and names the decisions it
still needs.

## Shape

One explicit command family replaces direct Cargo use in adopted projects.
Oxide resolves the project, picks storage, runs the real Cargo with injected
settings and reports what it did. Setup is one binary on `PATH`. No toolchain
shim, `PATH` shim, shell alias or project file is required.
[Interception](../rust-dx-landscape.md#cargo-command-interception) is deferred,
not designed in.

```text
oxide <check|test|clippy|build|run> [--profile <name>] [cargo args] [-- program args]
oxide cargo <subcommand> [args]     # any other Cargo command under the same storage
oxide status                        # pool, slots, waits, stray Cargo output
oxide init                          # write the agent guidance block (only project change)
```

- Cargo-shaped arguments (`-p`, `--features`, `--bin`, `--test`, filters,
  `--message-format`) pass through unchanged. Agents reuse what they know.
- `--release` means `--profile release`.
- Oxide owns storage placement. `--target-dir`, and `--config` keys that set
  `build.build-dir` or `build.target-dir`, are refused with an explanation.
- `oxide cargo …` covers the long tail (`doc`, `bench`, `nextest`, `expand`)
  without Oxide modelling each one. It is the documented escape hatch. Agents
  use it instead of plain `cargo`.

## Profiles

Named Oxide profiles are the user's intent. Their Cargo mechanism can improve
without changing the name. Oxide injects each definition through Cargo
`--config` (`profile.<name>.inherits`, `debug`, `package."*".debug`). A 2026-10-18
probe on 1.97.1 confirmed that a custom profile defined only by `--config`
builds. It also inherits the project's own `[profile.dev]` settings. So no
`Cargo.toml` edit or project file is needed.

| Oxide profile | Cargo mechanism today | Trade |
| --- | --- | --- |
| `quick` (default) | `oxide-light` recipe: inherits `dev`, workspace line tables, dependency debug off | Fastest stable feedback with usable backtraces; no dependency-level debugging. Future Cranelift/JIT candidates land here only after qualification ([Q-009](../questions.md#q-009--which-alternative-execution-modes-improve-the-daily-loop)). |
| `debug` | `oxide-full-debug` recipe: full debug for workspace, dependency and build-time units | Full debugger experience; more storage and link time. |
| `release` | Project's own `release` profile, untouched | Production-shaped build. |

Recipes are owned by [stable Cargo profiles](stable-cargo-profiles.md). That
contract's no-go comparison stands: `quick` is the intended default, not a
measured speed or storage claim.

Projects customise through their existing `[profile.dev]` / `[profile.release]`.
A per-project override of an Oxide profile is the only case found so far that
might need a project file ([W-3](#open-decisions)).

## Project identity without configuration

Correctness never depends on project identity. Cargo's fingerprints stay the
freshness authority and rebuild whatever differs. Identity only chooses which
warm slot to try, so it can be inferred:

1. Repository identity: the root commit(s) of the Git history plus the
   workspace's path within the repository. Worktrees and separate clones of
   the same repository therefore match. A fork matches too, which is harmless.
2. Shallow clones without the root commit fall back to the normalised `origin`
   URL.
3. Outside Git: the canonical workspace root, with no cross-checkout reuse.

The slot compatibility key is repository identity + Oxide profile + resolved
toolchain (`rustc -vV`) + target triple. The toolchain is whatever rustup
resolves for the project directory; Oxide does not select or pin it.
Mismatched toolchains get separate slots and never thrash one slot.

## What one command does (local executor)

1. Resolve the workspace (`cargo locate-project --workspace`), identity, profile
   and toolchain.
2. Lease a slot from the host pool. Prefer this checkout's last slot, then any
   idle compatible slot, then a new slot if the per-project cap and budget
   allow. Otherwise wait in order, with a visible "waiting for …" line.
3. Run the real Cargo as a child in its own process group, with:
   - `CARGO_BUILD_BUILD_DIR` set to the slot. Shared intermediates live on the
     bounded pool.
   - `CARGO_TARGET_DIR` set to `<checkout>/target`. Final outputs stay private,
     as in the [local lease](local-lease.md).
   - The profile injected through `--config`.
4. `run`: keep the lease for the build, release it, then execute the binary
   from the private target directory. `test`: keep the lease through test
   execution, because test binaries live in the build directory. Tests always
   run; reuse never skips them.
5. When the process group exits, release the lease and print a one-line
   summary: profile, slot reuse (warm/cold), elapsed time, pool usage.

The [qualified cross-worktree result](project-adoption.md#current-nucleus-integration-boundary)
is the evidence this shape relies on. A different-path checkout on a shared
`dev` slot rebuilt only its changed unit, and selected tests ran every time.
That covers one workflow; it is not a general invalidation guarantee.

## Storage and space management

- One per-user host pool: the existing [bounded APFS backend](bounded-storage.md)
  on macOS. The volume size is the hard aggregate ceiling. Only whole idle slots
  are evicted; held or uncertain slots stay protected.
- The first command creates the pool automatically at a default location and
  cap. No separate setup step ([W-1](#open-decisions)).
- Per-project slot cap, recommended 2 to start. The pool can then hold several
  projects warm instead of one project many times.
- A host-wide limit on concurrent Cargo invocations, since fewer simultaneous
  builds is the stated priority. Queue/Nucleus still decide which tasks run.
  Oxide only orders the Rust work they launch ([W-2](#open-decisions)).
- `oxide status` reuses the read-only [storage inventory](storage-inventory.md).
  It reports pool and slot use plus stray Cargo output. A checkout whose
  `target/` contains `deps/` or `incremental/` was built by plain Cargo. It
  shows the byte cost per checkout. Reclaiming that space is an explicit
  command; live cleanup still needs scoped approval.

## Execution location

Every command becomes a request handed to an executor, so where it runs is a
setting, not a different workflow:

- Request: repository identity, source snapshot (commit plus uncommitted
  diff, or the local path for the local executor), command, profile, Cargo
  arguments, target triple, environment allowlist.
- Result: streamed Cargo diagnostics in the requested message format, exit
  status, test results and, for `build`/`run`, an artifact manifest.

The foundation ships only the local executor. Remote kinds (SSH host on the
LAN, CI runner, cloud) select through per-user settings or `--on <name>`, never
project files. For `run`, build location and run location are independent: a
remote build for the local target triple fetches the binary and runs it
locally. Remote execution adds transfer, trust, credential and latency costs
([landscape](../rust-dx-landscape.md#experimental-frontier)). It must not become
required infrastructure for the core experience.

## Agent integration

Agents fall back to plain Cargo unless the Oxide path is the documented default
and the cheapest one. Three levers, none of them interception:

1. **Oxide agent skill.** It teaches the command map, choosing a profile,
   `oxide cargo …` as the escape hatch, how to read waits and refusals, and to
   run `oxide status` when storage looks wrong. It never says "use plain cargo".
2. **Project guidance block.** `oxide init` writes or updates one marked section
   in the project's agent instructions (`AGENTS.md`). It is about five lines: use
   `oxide check|test|clippy|build|run`, the default profile, the escape hatch.
   This block is the opt-in marker; there is no separate config.
3. **Effigy selectors.** In Effigy projects the existing Rust selectors call
   `oxide …`. Agents already follow selector maps. Effigy stays language-neutral
   because the selector only names a command.

Plain Cargo stays harmless: it builds privately as today. `oxide status` makes
the cost visible, so drift is measurable and the guidance can be corrected.
rust-analyzer keeps its own private check target unless the user opts its
`check.overrideCommand` into `oxide check --message-format=json`. Editor routing
is not in the foundation.

## Relationship to existing work

| Existing piece | Role in this design |
| --- | --- |
| [Local lease](local-lease.md) and [bounded storage](bounded-storage.md) | Become the in-process storage layer of `oxide <cmd>`. The standalone lease CLI remains for consumers that lease per task, such as Nucleus's current integration. |
| [Stable profiles](stable-cargo-profiles.md) | Become the `quick` / `debug` recipes, injected by `--config`. |
| [Storage inventory](storage-inventory.md) | Backs `oxide status` and stray detection. |
| [Build-context policy](build-context-policy.md) | Candidate input for per-command incremental/debug defaults; stays proposed. |
| Protected ordinary-Cargo route, declared-workflow front end, signed attempts, source-map qualifier | Not on the foundation path; parked pending a retirement ruling ([W-5](#open-decisions)). |

## Foundation slice

The foundation is the local executor on macOS with: the five commands plus
`oxide cargo`; the three profiles injected by `--config`; inferred identity;
per-command leasing with waits; automatic pool creation; and `oxide status`
with stray detection. Agent adoption comes from the skill plus `oxide init`.
It is working consistently when a second worktree's first `oxide test` on a
real project reuses dependency units. Tests must run, the pool must stay under
its ceiling, and normal use must leave no plain-Cargo intermediates in
checkouts. Re-evaluate interception only after that holds in everyday use.

## Open decisions

- **W-1 Default pool.** Location, default cap and the host-reserve rule for
  automatic creation. Recommendation: per-user cache directory, sparse image,
  modest default cap, refuse below the monitored host reserve.
- **W-2 Waiting and concurrency.** Per-project slot cap, host-wide concurrent
  Cargo limit, and wait versus cold-slot preference. Coordinates with
  [Q-011](../questions.md#q-011--how-should-slot-waiting-coordinate-with-host-admission).
- **W-3 Project file.** Needed only if projects must override Oxide profiles
  or pin Oxide behavior. Recommendation: none until a real project needs it.
- **W-4 Crash recovery.** Current leases never infer abandonment. Product ease
  of use needs automatic recovery when the holder and its whole process group
  are gone. This relaxes the cooperative-writer stance, so it needs a ruling
  ([Q-013](../questions.md#q-013--how-can-a-healthy-macos-workflow-safely-return-its-slot-for-reuse)).
- **W-5 Parking versus retiring.** Whether the protected route, declared
  workflow and source-map machinery are formally retired.
- **W-6 Linux backend.** CI, LAN boxes and Colima need a pool backend without
  APFS. A directory budget-only mode must say so honestly.
- **W-7 Command name.** `oxide` collides with Oxide Computer Company's CLI
  ([Q-005](../questions.md#q-005--what-names-are-safe-to-publish)). The design is
  name-independent.
