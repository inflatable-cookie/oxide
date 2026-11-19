# Optimization adoption policy

This contract owns the policy for adopting an optimization at the project
boundary where its prerequisites can be enforced. A project can use one qualified
preparation improvement without adopting every Oxide facility. Keep familiar
commands, document the supported routes, and state the benefit and its limits.

| Optimization | Project commitment | Current boundary |
| --- | --- | --- |
| Component-lazy preparation | Declare the components each command needs and prepare only those before that command. | Project-owned routing pattern, demonstrated in Acow. No Oxide installer or central registration. |
| Reusable intermediate slots | Authenticate attempts, acquire exclusive storage, reconcile uncertain effects and keep independent run evidence. | Consumer integration governed by [project adoption](project-adoption.md). Component selection does not activate it. |

There is no universal Oxide flag, installer or config file for component
preparation. Effigy stays language-neutral: projects own their Rust preparation
policy and compose it through their own task surface.

## Component-lazy preparation

Use this pattern when worktree setup or a broad command eagerly prepares Rust
targets the requested work does not use. Avoiding an unrelated target also avoids
its seed or clone operation. A project without eager preparation needs no seeder
to benefit from the pattern. It should keep Cargo's ordinary private outputs and
remove only unnecessary work. Selecting components does not improve Cargo's cache
algorithm or prove reuse across worktrees.

### Project changes

1. **Define preparation units.** Map each component to its Cargo workspace, target
   location and host or container runtime. A component is a project-defined unit,
   not necessarily one Cargo package.
2. **Map commands to required sets.** Inventory every build, check, test,
   development, debug and QA route that produces Rust outputs, including generated
   prerequisites. A command prepares the explicit union of what it needs.
3. **Move preparation to use.** Remove unconditional Rust preparation from generic
   worktree setup. Run the selected preparation before Cargo, through the project's
   existing entry points. Docs-only work prepares no Rust targets.
4. **Preserve protections and verify routing.** Record the component map and the
   supported command inventory in the project's owning documentation.

Acow's entry point is `effigy worktree:rust:seed --component <name>`; repeated
flags request a union. Other projects adapt their own surface and do not assume
that selector exists.

### Acceptance rules

- Outputs stay private to their owning worktree or run. Do not point several
  writers at one mutable target, or symlink private targets into shared storage.
  Keep host and container preparation separate.
- Unknown components refuse before preparation. Repeated preparation preserves
  existing destinations. Preparing every component must be an explicit operator
  command; missing or unsupported routing must not silently select all.
- Existing seeding protections stay in force: no overwrite of live destinations
  or primary targets, compiler compatibility, dirty-source restrictions, exclusive
  cold-preparation ownership, and readiness only after successful preparation.
  Component selection grants no copying fallback, cold-build, cleanup or
  cache-sharing authority.
- Supported routes must still compile and run the requested tests and keep their
  debug behaviour. Direct Cargo use keeps ordinary private outputs.
- Validate with fresh bounded fixtures covering single selection, unions, no
  preparation, unknown-component refusal and repeat behaviour, plus the project's
  targeted compile, format and documentation checks. Independent review checks the
  exact head. Synthetic fixtures establish selection behaviour only, not real
  compilation.

The adopting project owns its implementation, admission and validation. Oxide
guidance does not authorise another project's builds, settings, installs or
cleanup. Reverting the routing change keeps existing targets and run evidence.

## Evidence

Record what an optimization avoids, its supported routes and its validation
limits beside the project's policy. For component selection the claim is limited
to **unrequested preparation is avoided**. Allocated storage, elapsed time and
write reductions need separate ordinary-use observations. Apparent target size
establishes neither SSD writes nor lifetime savings.

Future facilities document their callable surface, owner, prerequisites, failure
behaviour and evidence before offering an adoption path. Experimental or disabled
integrations stay labelled. One qualified optimization does not qualify the rest
of Oxide.
