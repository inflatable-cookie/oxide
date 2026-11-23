# Oxide documentation

Oxide aims to make Rust development easy and light: predictable storage, less build churn and a fast, understandable feedback loop. Bounded reusable build storage is its first foothold.

## User guide

Start here if you use Oxide.

- [Getting started](getting-started.md): build Oxide, adopt a project, run everyday commands and opt out again.
- [Commands](commands.md): command classes, profiles, time budgets, `exec`, `status`, `pool`, `outputs`, `clean` and the Cargo version matrix.
- [Configuration](configuration.md): config file, pool location and budget, concurrent builds, removable drives, owned outputs and Cargo home cleanup.

## Architecture & contracts

For maintainers and agents. These files record how Oxide is built and what it guarantees.

- [Vision and operator rulings](knowledge/vision.md): scope, authority and validation policy.
- [Architecture](knowledge/architecture.md): storage invariants and authority boundaries.
- [Workflow contract](knowledge/contracts/workflow.md): the `oxide` command classes, profiles, lease and freshness behaviour as implemented.
- [Agent skill source](../skills/oxide/SKILL.md): repository-owned agent guidance, not installed.
- [Knowledge map](knowledge/README.md): what each knowledge file owns.
- [Open questions](knowledge/questions.md): unresolved design decisions.

Contracts:

- [Barge adoption](knowledge/contracts/barge-adoption.md): storage authority, areas, holds and the transition for existing roots.
- [Scratch pools](knowledge/contracts/scratch-pools.md): the external validation-volume pool and its refusal rules.
- [Project adoption](knowledge/contracts/project-adoption.md): the consumer integration boundary.
- [Optimization adoption](knowledge/contracts/optimization-adoption.md): policy for component-lazy preparation.
- [Local lease](knowledge/contracts/local-lease.md): the retained lease library semantics.
- [Session broker](knowledge/contracts/session-broker.md): the macOS transport for nested session membership and host-permit borrowing.
- [Warm-reuse admission](knowledge/contracts/warm-reuse-admission.md): admission without a recursive size scan.
- [Stable Cargo profiles](knowledge/contracts/stable-cargo-profiles.md): Oxide's own profile recipes.
- [Cold-slot seeding](knowledge/contracts/cold-slot-seeding.md): seeding a new slot from an idle donor.
- [Exec output inference](knowledge/contracts/exec-output-inference.md): `oxide exec` derives a tool's output members from the receipt; derived link creation is not implemented.
- [Release contract](knowledge/contracts/release.md): licence, distribution and publication restrictions.

## Background

- [Rust DX landscape](knowledge/rust-dx-landscape.md): sourced research and candidate remedies.
- [Storage-efficiency findings](knowledge/storage-efficiency.md): measured preparation and output behaviour.
- [Qualification readiness](knowledge/qualification-readiness.md): condensed containment, freshness and capacity findings.
- [Rust toolchain and quality](knowledge/rust-toolchain-and-quality.md): compiler and quality-policy routing.

The project plan and proposed briefs live in Nucleus project `oxide`. Repository knowledge records current truth, not work status.
