# Oxide documentation

Oxide aims to make Rust development easy and light: predictable storage, less build churn and a fast, understandable feedback loop. Bounded reusable build storage is its first foothold.

## Start here

- [Vision and operator rulings](knowledge/vision.md): scope, authority and validation policy.
- [Architecture](knowledge/architecture.md): storage invariants and authority boundaries.
- [Workflow contract](knowledge/contracts/workflow.md): the `oxide` command classes, profiles, lease and freshness behaviour as implemented.
- [Agent skill source](../skills/oxide/SKILL.md): repository-owned agent guidance, not installed.
- [Knowledge map](knowledge/README.md): what each knowledge file owns.
- [Open questions](knowledge/questions.md): unresolved design decisions.

## Contracts

- [Barge adoption](knowledge/contracts/barge-adoption.md): storage authority, areas, holds and the transition for existing roots.
- [Scratch pools](knowledge/contracts/scratch-pools.md): the external validation-volume pool and its refusal rules.
- [Project adoption](knowledge/contracts/project-adoption.md): the consumer integration boundary.
- [Optimization adoption](knowledge/contracts/optimization-adoption.md): policy for component-lazy preparation.
- [Local lease](knowledge/contracts/local-lease.md): the retained lease library semantics.
- [Nested lease](knowledge/contracts/nested-lease.md): proposed design for nested lease joining; not implemented.
- [Warm-reuse admission](knowledge/contracts/warm-reuse-admission.md): admission without a recursive size scan.
- [Storage inventory](knowledge/contracts/storage-inventory.md): the read-only inventory diagnostic.
- [Stable Cargo profiles](knowledge/contracts/stable-cargo-profiles.md): Oxide's own profile recipes.
- [Build-context policy](knowledge/contracts/build-context-policy.md): proposed context semantics; production policy inert.
- [Cold-slot seeding](knowledge/contracts/cold-slot-seeding.md): seeding a new slot from an idle donor.
- [Idle exec sessions](knowledge/contracts/idle-exec-sessions.md): proposed design for idle sessions and run-slot custody; not implemented.
- [Exec output inference](knowledge/contracts/exec-output-inference.md): `oxide exec` derives a tool's output members from the receipt; derived link creation is not implemented.
- [One-use workspaces](knowledge/contracts/one-use-workspaces.md): disposable checkouts beside normal slots; the lease-library states are implemented but dormant, and admission is not active.
- [Nucleus core reuse](knowledge/contracts/nucleus-core-reuse.md) and [Nucleus input roots](knowledge/contracts/nucleus-input-roots.md): condensed historical records.
- [Release contract](knowledge/contracts/release.md): publication restrictions.

## Background

- [Rust DX landscape](knowledge/rust-dx-landscape.md): sourced research and candidate remedies.
- [Storage-efficiency findings](knowledge/storage-efficiency.md): measured preparation and output behaviour.
- [Qualification readiness](knowledge/qualification-readiness.md): condensed containment, freshness and capacity findings.
- [Rust toolchain and quality](knowledge/rust-toolchain-and-quality.md): compiler and quality-policy routing.

The project plan and proposed briefs live in Nucleus project `oxide`. Repository knowledge records current truth, not work status.
