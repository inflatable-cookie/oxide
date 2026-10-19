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

- The ordinary Rust selectors run through the installed `oxide` ([dogfooding](docs/knowledge/contracts/workflow.md#dogfooding-in-oxides-repository)). If `oxide` itself is broken, use the plain-Cargo escapes `rust:compile:cargo`, `rust:compile-linux:cargo`, `rust:format:cargo` and `test rust-cargo`, and report the failure as a dogfood finding.
- `effigy rust:compile` runs `oxide check --workspace --all-targets --locked`.
- `effigy rust:format` runs `oxide fmt --all -- --check`, a Cargo passthrough.
- `effigy rust:compile-linux` runs `oxide check --locked --offline --target x86_64-unknown-linux-gnu` against the already-installed target. It checks compilation only: no target installation, linking or Linux runtime validation.
- `effigy oxide:install` builds `oxide` from a commit on pushed main and installs it at `~/.local/bin/oxide` with a hash receipt; it refuses any existing `oxide`. `effigy oxide:reinstall` replaces only the receipt-matching binary. Only the planner runs them, after a reviewed merge. `effigy dogfood:scripts:test` proves the installer's refusals and the test supervisor's timeout escalation on synthetic fixtures; it never builds or installs.
- `effigy rust:profile-light` and `effigy rust:profile-full-debug` are opt-in profile proofs: each verifies the resolved rustc debug settings Cargo passed for workspace, dependency and build-time units and runs the focused backtrace/unwind probe under that profile. They are focused proofs, not benchmarks or an alias for full QA.
- `effigy test rust` is the named Rust test suite, run as `oxide test`; `effigy test rust-cargo` is its plain-Cargo escape. Do not add a `tasks.test` alias.
- `effigy docs:check` runs the Northstar link and retired-concept checks plus `git diff --check HEAD`. The link check excludes `docs/knowledge/architecture.md`, whose references to optional sibling repositories are unavailable in a standalone checkout.
- `effigy build-context:comparison:test` runs synthetic fixtures for the incremental comparison launcher. It never enters the live image or Cargo path.
- `effigy rust:storage-inventory-cli` builds the unpublished `oxide-storage-inventory` binary and proves the rendered output-byte ceiling on tiny synthetic fixtures. It does not inventory a real project target and is not an alias for full QA.
- `effigy rust:oxide-cli` builds the unpublished `oxide` front door, proves usage refusals without a pool, and on macOS with a 20 GiB host reserve runs tiny offline fixture crates over one fresh 128 MiB APFS pool: command routing, quick/debug flags (including an external subcommand's `$CARGO`), storage-override refusals, `clean` (including waiting for an active command), `run` `default-run` selection from package and virtual workspace roots, cross-checkout reuse through the freshness gate (two Git worktrees of a scratch repository: stale-mtime rebuild, every in-checkout package from `cargo metadata` rebuilds on a path change (escaped-key custom build script, default `build.rs`, proc-macro caller), outside-repository path dependency fresh, unchanged repeat rebuilds nothing, refused gate builds cold, crash inside the gate leaves the retry gated; the gate's effect-free metadata call carries the command's lock, network and config arguments: a fresh worktree with an ignored lock gets it from `generate-lockfile` and warms its outside path dependency, certain `--locked`/`--frozen` refusals end with Cargo's error, `--offline` is mirrored, a stale lock or help request falls back cold without the gate writing a lock), waiting and the host limit, Ctrl-C forwarding, own-command crash recovery (surviving group held, dead group recovered and logged, standalone lease never recovered, `setsid` escape shown), `status` and Cargo-equal exit codes. After detaching the image it runs the `directory-budget` case on two scratch directory pools: budget-only labelling in creation, summaries and `status`, standalone-CLI refusal, an injected-reading reserve stop of a bounded writer that ignores TERM (KILL, positive closure, normal release; sampled minimum, overshoot and latency printed), an unreaped group member keeping the slot held until the next command recovers it, and admission evicting a whole idle slot on a 1 MiB budget. Before the pool exists it also runs one macOS `status --launch-check` under a `cargo` that records and refuses any call (bounded time, temporary copies removed, well-formed output, no Cargo; the hidden probe mode gets the same control) and proves `init` / `new` adoption on tiny scratch directories (empty dir, existing package and Cargo-only refusals, byte-identical re-run, text outside the markers, workspace member, `new`) and that adoption never creates the pool. On scratch directory pools it also proves slot file counts with a hard-linked pair counted once and `OXIDE_POOL_CAP` (refused when invalid, used on create, reported and ignored on an existing pool); then, on one fresh private `CARGO_HOME` whose setup downloads only `cfg-if` 1.0.0, the `status` Cargo home line, `clean --cargo-cache` refusals (missing lock, SSH git in either quote style, `git-fetch-with-cli`, config source replacement, custom registry, stale lock adding an SSH dependency), a cleanup that ran with no proxy connection, Cargo's lock skip, and a closed-proxy denial an owned control listener never sees. It is not a benchmark, real-project qualification or an alias for full QA.
- `effigy rust:oxide-cli:launch-check` runs only the `status --launch-check` case: offline staging with 1.97.1 into its own counted scratch, the 20 GiB reserve sampled before writing and after each phase, no pool, image or download, 300 s including cleanup. It is not a benchmark or an alias for full QA.
- `effigy rust:cargo-matrix:<release>` (`1.95.0`, `1.96.1`, `1.97.1`, `1.98.1`, `1.99.1`) is one logical [Cargo version matrix](docs/knowledge/contracts/workflow.md#cargo-version-matrix) proof. It stages `oxide` offline with 1.97.1 and runs the freshness gate scenario, effective build-dir, stray detection and the dep-info decoder under that already-installed Cargo, over one fresh 128 MiB APFS pool, plus an installed-1.94.1 fallback phase. It never installs a toolchain. It is not a benchmark or an alias for full QA.
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

<!-- oxide:begin -->
## Rust commands: use `oxide`

- Write `oxide` wherever you would write `cargo`: `oxide check`, `oxide test`, `oxide run -- <args>`, `oxide add <crate>`.
- Profiles: `quick` (default), `--profile debug` for a debugger, `--release`. Oxide places build output, so it refuses `--target-dir`.
- A `waiting for …` line means another build holds a slot; let it wait. `oxide status` shows the pool, slots and waits.
- Use `oxide clean`, not `cargo clean`.

Managed by `oxide init`; edits between these markers are replaced.
<!-- oxide:end -->
