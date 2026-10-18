# Oxide

Oxide aims to make Rust development easy and light, with ease of use as its primary product axis. Read [docs/README.md](docs/README.md), then the owning knowledge files before changing anything.

## Ownership and work

Queue project: `oxide`; repository: `inflatable-cookie/oxide`. Queue owns the plan, lane documents, leads, briefs, tasks, reviews and outcomes. Do not create repository planning/status mirrors. Product implementation requires an explicitly approved Queue brief and independent exact-head review. Small planner knowledge changes may land on main after docs checks.

Tom's scope and authority ruling lives in [vision](docs/knowledge/vision.md); storage invariants live in [architecture](docs/knowledge/architecture.md). Read [questions](docs/knowledge/questions.md) and [retirements](docs/knowledge/retired.toml) before planning.

## Guardrails

Tom authorized planning and queueing the next bounded tranche on 2026-10-09. Approved Queue briefs define each task’s authority, prerequisites and validation scope; dependency completion is not permission to bypass an explicit dispatch hold. Oxide-only implementation and its targeted repository validation must follow the independently reviewed design. Real-project qualification, consumer adoption and host installation remain separately gated. No live cache cleanup, settings/scheduler or consumer wire mutations, releases, tags or workflow changes without scoped approval. Do not prompt or mutate active Nucleus work. Resolve the repo-owned RustQuality profile/deviations before future Rust authoring; use each package's declared `rust-version` and do not infer one where absent.

When a change alters current truth, update its owning knowledge file in the same PR. Record operator rulings before yielding.

Tom's 2026-10-10 blanket approval in [vision](docs/knowledge/vision.md#authority-boundaries) gives standing authority for routine Oxide-only corrections and equivalent bounded retries toward the agreed Rust reuse proof. Use scoped Queue briefs and independent review; do not re-request permission solely for a correction or fresh equivalent session. Each session retains its recorded time/storage/invocation caps and conservative uncertainty rules. Material scope changes still need an operator ruling.

## Effigy command map

- `effigy rust:compile` checks the workspace and all targets with the committed lockfile.
- `effigy rust:format` checks Rust formatting.
- `effigy rust:profile-light` and `effigy rust:profile-full-debug` are opt-in profile proofs: each verifies the resolved rustc debug settings Cargo passed for workspace, dependency and build-time units and runs the focused backtrace/unwind probe under that profile. They are focused proofs, not benchmarks or an alias for full QA.
- `effigy test rust` is the named Rust test suite. Do not add a `tasks.test` alias.
- `effigy docs:check` runs the Northstar link and retired-concept checks plus `git diff --check HEAD`. The link check excludes `docs/knowledge/architecture.md`, whose references to optional sibling repositories are unavailable in a standalone checkout.
- `effigy build-context:comparison:test` runs synthetic fixtures for the incremental comparison launcher. It never enters the live image or Cargo path.
- `effigy rust:storage-inventory-cli` builds the unpublished `oxide-storage-inventory` binary and proves the rendered output-byte ceiling on tiny synthetic fixtures. It does not inventory a real project target and is not an alias for full QA.
- `effigy rust:oxide-cli` builds the unpublished `oxide` front door, proves usage refusals without a pool, and on macOS with a 20 GiB host reserve runs tiny offline fixture crates over one fresh 128 MiB APFS pool: command routing, quick/debug flags (including an external subcommand's `$CARGO`), storage-override refusals, `clean` (including waiting for an active command), `run` `default-run` selection from package and virtual workspace roots, cross-checkout reuse through the freshness gate (two Git worktrees of a scratch repository: stale-mtime rebuild, every in-checkout package from `cargo metadata` rebuilds on a path change (escaped-key custom build script, default `build.rs`, proc-macro caller), outside-repository path dependency fresh, unchanged repeat rebuilds nothing, refused gate builds cold, crash inside the gate leaves the retry gated; the gate's effect-free metadata call carries the command's lock, network and config arguments: a fresh worktree with an ignored lock gets it from `generate-lockfile` and warms its outside path dependency, certain `--locked`/`--frozen` refusals end with Cargo's error, `--offline` is mirrored, a stale lock or help request falls back cold without the gate writing a lock), waiting and the host limit, Ctrl-C forwarding, own-command crash recovery (surviving group held, dead group recovered and logged, standalone lease never recovered, `setsid` escape shown), `status` and Cargo-equal exit codes. Before the pool exists it also proves `init` / `new` adoption on tiny scratch directories (empty dir, existing package and Cargo-only refusals, byte-identical re-run, text outside the markers, workspace member, `new`) and that adoption never creates the pool. It is not a benchmark, real-project qualification or an alias for full QA.
- `effigy rust:storage-lease-cli` builds the unpublished `oxide-storage-lease` binary, proves usage refusals, and on macOS with a 20 GiB host reserve runs one fresh 128 MiB APFS pool through lease/release/recover/status. It is not a filesystem-fill benchmark and is not an alias for full QA.

There is no product QA selector yet. Use the focused selectors above; they do not replace milestone QA.

For Rust tests on macOS, set `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP` to the same fresh canonical directory under `/private/tmp` before running the selector. The suite checks canonical paths and uses short socket paths; the default `/var/folders` temp directory does not satisfy that contract. For example:

```sh
oxide_validation_scratch=$(mktemp -d /private/tmp/oxide-validation.XXXXXX)
OXIDE_TASK_SCRATCH_ROOT="$oxide_validation_scratch" TMPDIR="$oxide_validation_scratch" TMP="$oxide_validation_scratch" TEMP="$oxide_validation_scratch" effigy test rust
```


## Documentation validation

Use `effigy docs:check` for repository documentation changes. Check its exit code. These checks do not establish product validation success. Future implementation briefs must name targeted repository selectors and respect Queue's validation policy. Release rules live in [release](docs/knowledge/contracts/release.md).

<!-- northstar:rust-quality:start -->
## Northstar Rust Quality

Scope: Rust source, Cargo manifests, build files, tests, and directly related
documentation under this directory.

Use Northstar's strict everyday-authoring route for ordinary Rust work. Resolve
the repository-owned profile and deviations under `docs/knowledge/contracts/`;
use each package's declared `rust-version` and do not infer one where absent.
Re-enter at task start and coherent batch closeout.
Preserve unrelated work. A quality audit, no-slop pass, or audit-and-fix request
is explicit audit intent; never route it through everyday authoring.
<!-- northstar:rust-quality:end -->
