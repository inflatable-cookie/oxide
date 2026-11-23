# Knowledge map

Each fact has one owning file.

- [Vision](vision.md) owns scope, Tom's rulings, authority and the validation policy.
- [Architecture](architecture.md) owns storage invariants and integration boundaries.
- [Questions](questions.md) owns unresolved design decisions.
- [Workflow](contracts/workflow.md) owns the `oxide` command classes, profiles, run strategies, lease and freshness behaviour, crash recovery and agent integration as implemented.
- [Release](contracts/release.md) owns publication restrictions.
- [Rust toolchain and quality](rust-toolchain-and-quality.md) owns the development compiler, minimum Rust version and quality routing.
- [Rust DX landscape](rust-dx-landscape.md) owns sourced research findings, candidate remedies and their evidence limits.
- [Storage-efficiency findings](storage-efficiency.md) owns measured preparation, profile and disposable-output observations.
- [Qualification readiness](qualification-readiness.md) owns the condensed containment, freshness, compiler-path and capacity findings. They grant no execution authority.

## Contracts

- [Barge adoption](contracts/barge-adoption.md) owns Oxide's storage authority after the lean reset: what Barge owns, what Oxide keeps, area declarations and the transition for existing roots.
- [Scratch pools](contracts/scratch-pools.md) owns the explicit `directory-budget-v1` pool on an external validation volume.
- [Project adoption](contracts/project-adoption.md) owns the consumer integration boundary.
- [Optimization adoption](contracts/optimization-adoption.md) owns the policy for component-lazy preparation.
- [Local lease](contracts/local-lease.md) owns the retained lease library semantics.
- [Session broker](contracts/session-broker.md) owns the nested membership and permit-borrowing transport.
- [Warm-reuse admission](contracts/warm-reuse-admission.md) owns admission of a normal warm reuse without a recursive size scan.
- [Stable Cargo profiles](contracts/stable-cargo-profiles.md) owns Oxide's own profile recipes and their trade-offs.
- [Exec output inference](contracts/exec-output-inference.md) owns how `oxide exec` derives a tool's output members without a declared output. The receipt-derived set is implemented; derived link creation stays open in [Q-019](questions.md#q-019--how-should-oxide-exec-know-a-tools-profile-output-members).
- [Cold-slot seeding](contracts/cold-slot-seeding.md) owns the seeding of a new leased slot from an idle same-volume donor.
