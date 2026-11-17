# Oxide

Oxide aims to make Rust development easy and light, with ease of use as its primary product axis. Read [docs/README.md](docs/README.md), then the owning knowledge files before changing anything.

## Ownership and work

Queue project: `oxide`; repository: `inflatable-cookie/oxide`. Queue owns the plan, lane documents, leads, briefs, tasks, reviews and outcomes. Do not create repository planning/status mirrors. Product implementation requires an explicitly approved Queue brief and independent exact-head review. Small planner knowledge changes may land on main after docs checks.

Tom's scope and authority ruling lives in [vision](docs/knowledge/vision.md); storage invariants live in [architecture](docs/knowledge/architecture.md). Read [questions](docs/knowledge/questions.md) and [retirements](docs/knowledge/retired.toml) before planning.

For each pull request, add one short unreleased entry in `changelog.d/<task-or-PR>-<slug>.<type>.md` (`added`, `changed`, `fixed`, `removed` or `security`). Keep `CHANGELOG.md`'s `[Unreleased]` section empty; release preparation folds fragments there. See the [release contract](docs/knowledge/contracts/release.md).

## Guardrails

Tom authorized planning and queueing the next bounded tranche on 2026-10-09. Approved Queue briefs define each task’s authority, prerequisites and validation scope; dependency completion is not permission to bypass an explicit dispatch hold. Oxide-only implementation and its targeted repository validation must follow the independently reviewed design. Real-project qualification, consumer adoption and host installation remain separately gated. No live cache cleanup, settings/scheduler or consumer wire mutations, releases, tags or workflow changes without scoped approval. Do not prompt or mutate active Nucleus work. Resolve the repo-owned RustQuality profile/deviations before future Rust authoring; use each package's declared `rust-version` and do not infer one where absent.

When a change alters current truth, update its owning knowledge file in the same PR. Record operator rulings before yielding.

Tom's 2026-10-10 blanket approval in [vision](docs/knowledge/vision.md#authority-boundaries) gives standing authority for routine Oxide-only corrections and equivalent bounded retries toward the agreed Rust reuse proof. Use scoped Queue briefs and independent review; do not re-request permission solely for a correction or fresh equivalent session. Preserve historical session limits and evidence; new validation follows the policy below. Material scope changes still need an operator ruling.

## Validate

Oxide#314's lean reset removes the capture-only Cargo selectors with the
capture programme. Their older rulings below remain historical; this adoption
uses the ordinary Oxide selectors and `rust:barge-adoption`.

Tom approved risk-proportionate verification on 2026-10-20 ([vision](docs/knowledge/vision.md#validation-policy)). Every product brief names its tier, covering Effigy selectors, wall-clock limit and scratch-disk budget.
Tom decision `c6e3e9c8` v2 (2026-11-04) permits the named capture-only Cargo selectors `rust:compile:cargo`, `rust:compile-release:cargo`, `rust:format:cargo` and `rust:test-600:cargo` for direct capture covering. They establish build and embedded-harness behavior, not private launcher isolation or current export production. Ordinary Rust selectors remain through Oxide. The direct checks require separate planner activation and the existing guard, root and host safeguards.
Every native-proof authoring or correction brief declares HIGH complexity, applies the fixed [premise checklist](docs/knowledge/contracts/workflow.md#proof-authoring-premise-checklist), and requires an uncounted keep-going diagnostic before its counted native ([policy](docs/knowledge/vision.md#validation-policy)). A source sweep alone does not satisfy that gate.

- **Standard, the default:** the author runs covering compile, format when applicable, unit/regression tests, docs checks when docs change, and the focused native proof. An independent reviewer reviews the exact published source, regressions and the author's evidence. The reviewer does not repeat the native proof or covering selectors; missing or inadequate evidence is reported honestly.
- **Elevated:** add an independent reviewer native session for changes that delete or recycle storage, decide crash recovery or writer closure, or write to removable media. The brief states the trigger and focused selector. Fixture isolation does not by itself remove a product's elevated trigger.
- Native proofs use a separately qualified shared runner or a short fixed script under existing host safeguards: Effigy heavy admission, the host-reserve watchdog, a wall-clock limit and scratch cleanup. Do not build a bespoke in-process supervision or accounting harness for each task. The shared runner is `scripts/bounded-test.py` ([shared proof supervisor](docs/knowledge/contracts/workflow.md#shared-proof-supervisor), oxide#109 pinned-leader design; qualified only once that task's independent exact-head native passes; macOS, cooperative session scope): a new proof runs as one short fixed script that is its whole command, as an Effigy `heavy` task `python3 scripts/bounded-test.py python3 scripts/<proof>.py`, writing only inside `OXIDE_TASK_SCRATCH_ROOT` and leaving no process outside its leader's group (such a process is never signalled and ends the run `UNCLOSED`). The removable-pool runner (`bounded_runner.py`) and selector remain unqualified tooling.
- Gate experiments on wall-clock time and a scratch-disk budget. Measure and report resource figures; Cargo-call, version-probe and monitor-timing counters are observations, not acceptance gates. Preserve product correctness, isolation, ownership and conservative handling of unknown writers. Do not reset active clocks or erase earlier failures. Explicit historical limits stay historical; a scoped amendment names any in-flight change.
- Run targeted checks through Effigy, not raw tools. Do not run full `effigy qa` or repeat completed validation without a changed input or unresolved finding that warrants it. Milestone QA remains separate.
- Tom may accept a task as-is. Record this as operator acceptance, preserve independent findings and missing evidence, and merge/close out without another publication-only review loop. If Queue cannot represent that disposition, raise the capability gap with its owner; do not fabricate a verdict or silently bypass a gate. A direct GitHub merge requires explicit scoped authorization and is reconciled through Queue's external-merge closeout.

The oxide#233 supervisor candidate implements bounded same-round replacement
accounting ([workflow](docs/knowledge/contracts/workflow.md#foreign-churn-in-the-supervisor-census)); it is unqualified. Tom's task-specific ruling `c8991f99` requires one retained Queue reviewer and one separately activated, exact-head uncounted diagnostic after source approval; the reviewer judges that evidence and the planner owns further qualification and active-consumer disposition as release gates. This task does not authorize counted author or reviewer native sessions. `effigy proof:supervisor:test` currently loads 406 pure cases; the qualification suite additionally loads 59 real cases, including two explicitly synthetic replacement observations. The one Queue diagnostic at `e6ed727` failed four fixture-attribution cases; its packet remains historical and gives no native credit. The correction adds bounded test-only exact-identity ledger observations; it changes no census decision or diagnostic cap. The corrected-head `535ebbcb` uncounted diagnostic passed all 465 cases but failed `nested-cases` on two unmodelled fixture fields; its packet remains historical with no qualification credit. The candidate models those fields only for their two named cases and preserves containment and receipt checks. Previous 382-pure / 57-real receipts remain bound to their historical bytes. The later Queue diagnostic `0e148446` at `8b4c2e0` completed 465/465 cases and 12/12 checks with no problems, closed status 0 under its fixed bounds; its intentional exit 1 and `qualified: false` remain uncounted, not deployment credit. The safe-landing route keeps `scripts/bounded-test.py` exactly `fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b` for ordinary consumers and stores unchanged candidate `c578243126a2470cde0dfd19530254b3ec61b4ea5bb1fa43c6bbd2d41853c2b7` at `scripts/bounded-test-candidate.py` for qualification only. Bootstrap and suite refuse missing, changed or swapped sources before candidate use; no ordinary selector or consumer pin is repointed. Routing review and any owner-activated changed-head diagnostic remain separate; promotion is not authorized.

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
- `docs:check`: links, retired concepts, changelog and whitespace.
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
