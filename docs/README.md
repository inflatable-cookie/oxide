# Oxide documentation

Oxide aims to make Rust development easy and light: predictable storage, less build churn and a fast, understandable feedback loop. Bounded reusable build storage is its first foothold. Queue and Nucleus should consume the same reusable facility; a daemon is one candidate architecture.

- [Vision and operator ruling](knowledge/vision.md)
- [Oxide workflow (foundation core, adoption, freshness gate, budget-only directory backend, Cargo home hygiene and removable-volume part 1 implemented; dogfooded in this repository)](knowledge/contracts/workflow.md)
- [Oxide agent skill source (not installed)](../skills/oxide/SKILL.md)
- [Storage invariants and authority boundaries](knowledge/architecture.md)
- [Optimization adoption policy](knowledge/contracts/optimization-adoption.md)
- [Project adoption boundary](knowledge/contracts/project-adoption.md)
- [Rust DX research landscape](knowledge/rust-dx-landscape.md)
- [Storage-efficiency findings](knowledge/storage-efficiency.md)
- [Build-context policy (proposed)](knowledge/contracts/build-context-policy.md)
- [Storage inventory (read-only)](knowledge/contracts/storage-inventory.md)
- [Same-user local lease CLI](knowledge/contracts/local-lease.md)
- [Rust toolchain and quality](knowledge/rust-toolchain-and-quality.md)
- [Opt-in stable Cargo profiles](knowledge/contracts/stable-cargo-profiles.md)
- [Open decisions](knowledge/questions.md)
- [Knowledge map](knowledge/README.md)
- [Release contract](knowledge/contracts/release.md)

The project plan and proposed briefs live in Queue project `oxide`. Repository knowledge records current truth, not work status.
