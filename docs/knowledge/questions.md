# Open questions

## Q-001 — What implementation shape best supports durable exclusive ownership?

Compare daemon and non-daemon options, failure/restart semantics and consumer authority boundaries. A daemon is not selected. Owner: [architecture](architecture.md).

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Qualify build-dir support, generated code, proc macros, embedded paths, test executables and final artifact capture before choosing shared versus path-isolated partitions. No assumed MSRV. Owner: [architecture](architecture.md).

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

Resolve key derivation, capacity refusal, idle-only eviction, uncertain ownership and idempotent lost-reply handling. Owner: [architecture](architecture.md).

## Q-004 — What narrowly scoped pilot may be authorized?

Candidate: Nucleus core-test Effigy selector at one fixed exact head and two disposable checkout paths, per-run artifacts/logs and one bounded exclusive intermediate slot. Execution is not authorized. Resolve selector, source pin, storage/write/time bounds, RustQuality profile/deviations and consumer coordination separately before execution. Owner: [vision](vision.md).

## Q-005 — What names are safe to publish?

Review executable/crate/distribution names against the existing Oxide Computer Company SDK/CLI before publication. Owner: [release](contracts/release.md).
