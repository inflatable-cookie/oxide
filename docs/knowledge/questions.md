# Open questions

## Q-001 — What implementation shape best supports durable exclusive ownership?

A central daemon hub is Tom’s preferred candidate to qualify. Compare it with the simplest alternative, resolve failure/restart semantics and consumer authority boundaries; final implementation selection is still open. Owner: [architecture](architecture.md).

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Qualify build-dir support, generated code, proc macros, embedded paths, test executables and final artifact capture before choosing shared versus path-isolated partitions. No assumed MSRV. Owner: [architecture](architecture.md).

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

Resolve key derivation, capacity refusal, idle-only eviction, uncertain ownership and idempotent lost-reply handling. Owner: [architecture](architecture.md).

## Q-004 — What narrowly scoped pilot may be authorized?

Candidate: Nucleus core-test Effigy selector at one fixed exact head and two disposable checkout paths, per-run artifacts/logs and one bounded exclusive intermediate slot. The bounded design-only task is approved; experimental execution is not authorized. Resolve selector, source pin, storage/write/time bounds, RustQuality profile/deviations and consumer coordination separately before execution. Owner: [vision](vision.md).

## Q-005 — What names are safe to publish?

Review executable/crate/distribution names against the existing Oxide Computer Company SDK/CLI before publication. Owner: [release](contracts/release.md).

## Q-006 — What experience and budgets define light Rust development?

For the first foothold, use one narrow native workflow and bounded qualification; broad baselines are not a prerequisite. Choose longer-term representative workflows and acceptable disk footprint, write volume, feedback latency, memory and operator-effort targets after first storage relief. Tom's direction is confirmed; numeric targets are not. Owner: [vision](vision.md).

## Q-007 — How should Oxide consume upstream caching and existing tools?

The strategy is settled: reuse usable Cargo capabilities and fill current gaps without waiting indefinitely; see [vision](vision.md#relationship-with-cargo). Identify supported capability/version boundaries, keep policy independent of replaceable mechanisms, and define replacement/retirement criteria for each temporary mechanism. The accepted Cargo cross-workspace-cache goal is direction, not installed capability proof. Owner: [Rust DX landscape](rust-dx-landscape.md).

## Q-008 — Which host/container division preserves required production parity?

Coordinate Rust-specific recommendations with Effigy's language-neutral execution routes. Native feedback, Linux builds/tests and Colima storage need separate measurements and ownership. Owner: [vision](vision.md).

## Q-009 — Which alternative execution modes improve the daily loop?

JIT, Cranelift and other code generation/execution solutions are confirmed research scope. Compare fast ahead-of-time compilation, JIT and hot reload by complete edit-to-feedback cost and supported semantics. Mechanisms, compatibility and validation mapping remain open. Owner: [Rust DX landscape](rust-dx-landscape.md); priority ruling: [vision](vision.md).
