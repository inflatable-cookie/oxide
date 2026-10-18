# Knowledge map

[Vision](vision.md) owns scope, Tom's ruling and authorization. [Architecture](architecture.md) owns storage invariants and integration boundaries. [Rust DX landscape](rust-dx-landscape.md) owns sourced research findings, candidate remedies and evidence limits. [Questions](questions.md) owns unresolved design decisions. [Release](contracts/release.md) owns publication restrictions. [Retirements](retired.toml) records removed concepts.

[Workflow](contracts/workflow.md) owns the `oxide` command classes, profiles and run strategies, inferred project identity, cross-checkout freshness gate, crash recovery, local executor, execution-location seam, agent integration, the keep/retire assessment, the foundation slice's open decisions and what the unpublished `oxide` binary implements today, including `init` / `new` adoption and the fixture-proven freshness gate (slot origins live in `oxide-managed/origins/`). It also points to the repository-owned agent skill source at `skills/oxide/SKILL.md`, which is not installed anywhere.

[Storage-efficiency findings](storage-efficiency.md) records current component preparation, profile and disposable-output behavior across the inspected projects, with limits on what the source evidence establishes.

[Build-context policy](contracts/build-context-policy.md) owns the proposed lifecycle-aware incremental/debug context semantics, the frozen one-shot comparison recipe, and the private opt-in launcher that enforces deadline and owned-footprint limits. Production policy stays inert. The [direct incremental comparison](storage-efficiency.md#direct-incremental-comparison-2026-10-15) supplies real timing, storage and selected backtrace evidence with raw output attached; production adoption and shared-slot reuse remain unqualified.

[Optimization adoption](contracts/optimization-adoption.md) owns the project adoption policy for component-lazy preparation and the distinction between lightweight improvements and reusable-slot integration.

[Storage inventory](contracts/storage-inventory.md) owns the shipped bounded read-only inventory of one explicitly selected target directory: categories, logical-versus-allocated and hardlink-union accounting, coverage gaps, exit semantics, explicit unknowns and its fixture limits. It is a diagnostic, not storage ownership or cleanup authority.

[Local lease](contracts/local-lease.md) owns the unpublished same-user local lease CLI: init/lease/release/recover/status, JSON and exits, finite pool/ledger limits, idle-only eviction, the `oxide` control-directory tolerance and the explicit not-Nucleus-ready / cross-worktree-unqualified boundary.

[Project adoption](contracts/project-adoption.md) owns the consumer integration boundary, the reviewed cross-worktree result and the owner-reported Nucleus lease use. [Rust toolchain and quality](rust-toolchain-and-quality.md) owns the development compiler, initial minimum Rust version and quality-policy routing. [Opt-in stable Cargo profiles](contracts/stable-cargo-profiles.md) owns Oxide's own `oxide-light`/`oxide-full-debug` recipes, their debugging tradeoff, the focused profile-proof selectors and the no-go disposition of the comparison observations.

[Qualification findings](qualification-readiness.md) preserves the Cargo freshness, compiler path, cross-path, containment and capacity findings the workflow builds on. They grant no execution authority.
