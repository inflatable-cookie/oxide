# Oxide

Oxide aims to make Rust development easy and light, with ease of use as its primary product axis. Read [docs/README.md](docs/README.md), then the owning knowledge files before changing anything.

## Ownership and work

Nucleus project: `oxide`; repository: `inflatable-cookie/oxide`. Nucleus owns the plan, lane documents, leads, briefs, tasks, reviews and outcomes. Do not create repository planning/status mirrors. Product implementation requires an explicitly approved Nucleus brief and independent exact-head review. Small planner knowledge changes may land on main after docs checks.

Tom's scope and authority ruling lives in [vision](docs/knowledge/vision.md); storage invariants live in [architecture](docs/knowledge/architecture.md). Read [questions](docs/knowledge/questions.md) before planning.

For each pull request, add one short unreleased entry in `changelog.d/<task-or-PR>-<slug>.<type>.md` (`added`, `changed`, `fixed`, `removed` or `security`). Keep `CHANGELOG.md`'s `[Unreleased]` section empty; release preparation folds fragments there. See the [release contract](docs/knowledge/contracts/release.md).

## Guardrails

Tom authorized planning and queueing the next bounded tranche on 2026-10-09. Approved Nucleus briefs define each task’s authority, prerequisites and validation scope; dependency completion is not permission to bypass an explicit dispatch hold. Oxide-only implementation and its targeted repository validation must follow the independently reviewed design. Real-project qualification, consumer adoption and host installation remain separately gated. No live cache cleanup, settings/scheduler or consumer wire mutations, releases, tags or workflow changes without scoped approval. Do not prompt or mutate active Nucleus work. Use each package's declared `rust-version` and do not infer one where absent.

When a change alters current truth, update its owning knowledge file in the same PR. Record operator rulings before yielding.

Tom's 2026-10-10 blanket approval in [vision](docs/knowledge/vision.md#authority-boundaries) gives standing authority for routine Oxide-only corrections and equivalent bounded retries toward the agreed Rust reuse proof. Use scoped Nucleus briefs and independent review; do not re-request permission solely for a correction or fresh equivalent session. Preserve historical session limits and evidence; new validation follows the policy below. Material scope changes still need an operator ruling.

## Validate

Tom's 2026-11-17 lean reset ([vision](docs/knowledge/vision.md#validation-policy)) applies to all Oxide work: ordinary tests and one independent exact-head review, with no native-proof ladders.

- Every product brief names its covering Effigy selectors, wall-clock limit and scratch-disk budget.
- The author runs `rust:compile`, `rust:compile-release`, `rust:format`, `docs:check` when docs change, and the ordinary unit/integration suite (`rust:barge-adoption`) on fresh fixtures below the task's TMPDIR. Crash recovery, deletion and removable-media behaviour are covered by ordinary regression tests there.
- One independent reviewer judges the exact published head, its regressions and the author's evidence. It does not repeat the covering selectors and reports missing or inadequate evidence honestly.
- No native proof, reviewer-native session, uncounted diagnostic or qualification ladder is required. The older tier rules, native selectors and the unqualified oxide#233 supervisor candidate remain in the repository and the [workflow contract](docs/knowledge/contracts/workflow.md) as history. They run only on an explicit owner request, and their evidence keeps no transferred credit.
- Gate experiments on wall-clock time and scratch disk. Report resource figures as observations. Preserve product correctness, isolation, ownership and conservative handling of unknown writers.
- Run targeted checks through Effigy. Don't run full `effigy qa` or repeat completed validation without a changed input or an unresolved finding. Milestone QA stays separate.
- Tom may accept a task as-is. Record that as operator acceptance, keep the independent findings, and close out without another publication-only review loop. A direct GitHub merge needs explicit scoped authorization and is reconciled through Nucleus's external-merge closeout.
- Installing a storage-affecting binary is a separate planner step after merge. Time one real new-slot admission on the live pool before replacing the installed binary, and check for old-binary processes afterwards (2026-11-18 incident, decision `d9f8839e`).

## Effigy command map

Ordinary Rust selectors run through the installed `oxide`. If it is broken,
use Effigy's existing machine-wide per-program `oxide` fallback and report the
dogfood finding. Preserve the ordinary routing and existing compiler profiles.

- `rust:compile`: `oxide check --workspace --all-targets --locked`.
- `rust:compile-release`: release binary check with `RUSTFLAGS='-D warnings'`.
  Run both compile selectors for unconditional Rust changes.
- `rust:format`: check formatting; `rust:format-write`: apply it during authoring.
- `rust:compile-linux`: offline compile for the already-installed target only.
- `rust:barge-adoption-linux`: compile all targets, including the Linux pool
  regression, for that installed target; no linking or runtime claim.
- `docs:check`: links, changelog and whitespace.
- `rust:barge-adoption`: ordinary heavy covering workspace unit/integration
  suite for oxide#314, with no named skips and fresh fixtures below task TMPDIR.
- `rust:test-600`: the separate historical dual-root wrapper; not authorized
  for this task's scratch boundary.
- `proof:rust-test-600:test` / `:plan`: wrapper self-check / effect-free plan.
- `proof:supervisor:test` / `:preflight`: pure cases / effect-free host readiness.
  Native `proof:supervisor:qualify` and its diagnostic still require their own
  owner activation; adoption does not promote its candidate.
- `oxide:install` / `oxide:reinstall`: planner-only installation after reviewed
  merge, with receipt matching; source adoption authorizes neither.

`rust:barge-adoption` creates one private per-run directory below the invoking
`TMPDIR`, passes it as TMPDIR/TMP/TEMP and the test fixture roots, and removes
only that directory after a successful test command. A failed command retains
it. Effigy owns heavy admission and the 600-second run deadline. Tests run serially
because child launches inherit process-wide Barge holds. Unit tests
inject only the platform/classifier inputs that formerly required boot-volume
temporary paths; their file operations remain in the private fixture. The
shared supervisor and the separate dual-root recipe are unchanged and provide
no authority for this task to create internal-drive fixtures.

The lean Barge adoption uses ordinary tests and independent exact-head review,
with no adoption/capture/supplier native ladder. Barge owns volumes, locations,
areas, roots, holds, pins, pressure pruning and marker-checked trash. Oxide keeps
Cargo and lease/recipe/freshness/seeding/output-link workflow. Children closing
inherited holds and unrelated unheld writers are outside this cooperative
model. Unknown legacy roots are retained. See the
[adoption contract](docs/knowledge/contracts/barge-adoption.md).

Use `effigy tasks` for the remaining selectors and their descriptions, and
[workflow](docs/knowledge/contracts/workflow.md) for their owning contracts.
Do not run a native selector solely because it exists.
