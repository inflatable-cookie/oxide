# Oxide

Oxide aims to make Rust development easy and light, with ease of use as its primary product axis. Read [docs/README.md](docs/README.md), then the owning knowledge files before changing anything.

## Ownership and work

Queue project: `oxide`; repository: `inflatable-cookie/oxide`. Queue owns the plan, lane documents, leads, briefs, tasks, reviews and outcomes. Do not create repository planning/status mirrors. Product implementation requires an explicitly approved Queue brief and independent exact-head review. Small planner knowledge changes may land on main after docs checks.

Tom's scope and authority ruling lives in [vision](docs/knowledge/vision.md); storage invariants live in [architecture](docs/knowledge/architecture.md). Read [questions](docs/knowledge/questions.md) and [retirements](docs/knowledge/retired.toml) before planning.

## Guardrails

Tom authorized planning and queueing the next bounded tranche on 2026-10-09. Approved Queue briefs define each task’s authority, prerequisites and validation scope; dependency completion is not permission to bypass an explicit dispatch hold. Oxide-only implementation and its targeted repository validation must follow the independently reviewed design. Real-project qualification, consumer adoption and host installation remain separately gated. No live cache cleanup, settings/scheduler or consumer wire mutations, releases, tags or workflow changes without scoped approval. Do not prompt or mutate active Nucleus work. Resolve the repo-owned RustQuality profile/deviations before future Rust authoring; no assumed MSRV.

When a change alters current truth, update its owning knowledge file in the same PR. Record operator rulings before yielding.

## Documentation validation

There is no product build or repository QA selector yet. Documentation-only bootstrap uses installed Northstar checks from this checkout:

- `effigy skill run northstar/check-links`
- `effigy skill run northstar/retired-concepts`
- `git diff --check`

Check exit codes. These checks do not establish product validation success. Future implementation briefs must name targeted repository selectors and respect Queue's validation policy. Release rules live in [release](docs/knowledge/contracts/release.md).
