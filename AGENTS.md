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
- The ordinary selectors run through the installed `oxide` on the live pool (dogfooding). Their normal pool effects, such as leasing a slot or evicting an idle one at the ceiling, are expected. "No live pool access" in a brief means: don't hand-edit pool files or state, don't point fixtures at the live pool, and don't install.
- Oxide has no `test:affected` selector, and its Nucleus repository record sets no affected-test command. The whole-suite `rust:barge-adoption` run at the head covers every affected test and satisfies a generic "run the affected tests" instruction. Don't block on the missing selector.
- One independent reviewer judges the exact published head, its regressions and the author's evidence. It does not repeat the covering selectors and reports missing or inadequate evidence honestly.
- No native proof, reviewer-native session, uncounted diagnostic or qualification ladder is required. Those selectors and the shared supervisor were removed in oxide#322; their history stays in git and in the historical evidence under `docs/knowledge/evidence/`, which carries no transferred credit.
- Gate experiments on wall-clock time and scratch disk. Report resource figures as observations. Preserve product correctness, isolation, ownership and conservative handling of unknown writers.
- Run targeted checks through Effigy. Don't run full `effigy qa` or repeat completed validation without a changed input or an unresolved finding. Milestone QA stays separate.
- Tom may accept a task as-is. Record that as operator acceptance, keep the independent findings, and close out without another publication-only review loop. A direct GitHub merge needs explicit scoped authorization and is reconciled through Nucleus's external-merge closeout.
- Installing a storage-affecting binary is a separate planner step after merge. Run the staged live-pool canary on the intended pin before replacing the installed binary, and check for old-binary processes afterwards (2026-11-18 incident, decision `d9f8839e`).

## Effigy command map

Ordinary selectors run through the installed `oxide`. If it is broken, use
Effigy's per-program `oxide` fallback and report the dogfood finding. `effigy
tasks` lists the live set.

- `rust:compile`: `oxide check --workspace --all-targets --locked`.
- `rust:compile-release`: release compile of the binaries with `RUSTFLAGS='-D warnings'`.
- `rust:format` / `rust:format-write`: check / apply formatting.
- `rust:compile-linux`: offline compile for the already-installed Linux target; no link or run.
- `rust:barge-adoption-linux`: compile all targets for that target; no link or run.
- `rust:tree-duplicates`: duplicate packages in the normal dependency closure.
- `rust:barge-adoption`: the covering ordinary workspace unit and integration suite.
- `docs:check`: links, changelog fragments and whitespace.
- `changelog:check` / `changelog:fold` / `changelog:test`: changelog fragments; `fold` is release-only.
- `dogfood:scripts:test`: synthetic installer, pin, canary and wire-format cases.
- `offline-seed:test`: synthetic offline-seeder cases.
- `oxide:canary` / `oxide:install` / `oxide:reinstall` / `oxide:rollback`: planner-only, after reviewed merge.

Workflow and install contracts: [workflow](docs/knowledge/contracts/workflow.md).
