# Optimization adoption policy

Adopt each optimization at the project boundary where its prerequisites can
be enforced. Projects can use a qualified preparation improvement without
adopting every Oxide facility. Keep familiar commands, document the supported
routes, and make the benefit and its evidence limits clear.

| Optimization | Project commitment | Current boundary |
| --- | --- | --- |
| Component-lazy preparation | Declare the components each command needs and prepare only those components before that command. | Project-owned routing pattern demonstrated in Acow; no Oxide installation or central registration is required. |
| Reusable intermediate slots | Authenticate attempts, acquire exclusive storage, reconcile uncertain effects and preserve independent run evidence. | Separate consumer integration governed by [project adoption and slot waiting](project-adoption.md); component selection does not activate it. |

There is no universal Oxide opt-in flag, installer or configuration file for
component preparation. Acow's selectors are project-specific. Effigy remains
language-neutral: projects own their Rust preparation policy and compose it
through their existing task surface.

## Component-lazy preparation

### When it helps

Use this pattern when worktree setup or a broad command eagerly prepares Rust
targets that the requested work does not use. Preparation can include an
existing seed or clone operation; avoiding an unrelated target avoids that
operation entirely. The mechanism and bounded evidence for Acow live in
[storage-efficiency findings](../storage-efficiency.md#component-preparation).

A project without eager target preparation need not introduce a seeder to
adopt this pattern. Keep Cargo's ordinary private outputs and remove only
identified unnecessary work. Selecting components does not itself improve
Cargo's cache algorithm or prove cross-worktree cache reuse.

### Project changes

1. **Define preparation units.** Map each component to its Cargo workspace,
   existing target location and host or container runtime. A component is a
   project-defined preparation unit, not necessarily one Cargo package.
2. **Map commands to required sets.** Inventory direct and composed build,
   check, test, development, debug and QA routes, plus any other route that
   produces Rust outputs. Include generated-code prerequisites. Composition
   prepares the explicit union; a command building the whole workspace still
   prepares everything it actually needs.
3. **Move preparation to use.** Remove unconditional Rust preparation from
   generic worktree setup. Run the selected preparation before Cargo through
   familiar project entry points. JS/docs-only work prepares no Rust targets;
   formatting or inspection commands need preparation only if they actually
   produce those outputs. Agents should not need an extra manual ritual.
4. **Preserve protections and verify routing.** Apply the acceptance rules
   below through the project's normal reviewed implementation process.
   Record the component map and supported command inventory in its owning
   documentation.

For example, a hypothetical project can map documentation checks to no
components, backend tests to `backend`, and combined desktop/backend tests
to `{desktop, backend}`. These are example policy values, not Oxide CLI keys.

Acow's existing entry point is `effigy worktree:rust:seed --component farmyard`;
repeated `--component` flags request a union. Other projects adapt their own
preparation surface rather than assuming that selector exists everywhere.

### Acceptance rules

- Outputs remain private to their owning worktree or run. Do not point
  multiple writers at a shared mutable target or symlink private targets into
  shared storage. Keep host and container preparation separate.
- Unknown components refuse before preparation. Repeated preparation preserves
  existing destinations. An explicit operator command may prepare all
  components; missing or unsupported routing must not silently select all.
- Existing seeding protections remain: no overwrite of live destinations or
  primary targets, compiler compatibility, dirty-source restrictions,
  exclusive cold-preparation ownership and readiness only after successful
  preparation. Component selection grants no new copying fallback, cold-build,
  cleanup or cache-sharing authority.
- Supported routes must still compile and run the requested tests and retain
  their existing debug behavior. Direct Cargo use keeps ordinary private
  outputs; preparation benefits apply only to mapped entry points.
- Use fresh bounded fixtures to check single selection, unions, no preparation,
  unknown-component refusal, repeat behavior and the existing applicable
  protection rules. Exercise the changed routing and run the project's targeted
  compile, format and documentation checks through its supported selectors.
  Independent review checks the exact implementation head. Synthetic fixtures
  establish selection behavior, not real compilation or full milestone QA.

The adopting project owns implementation, admission and validation authority.
Oxide guidance does not authorize another project's builds, settings, installs
or cleanup. Reverting the routing change must preserve existing targets and
run evidence; adoption does not require deleting caches.

## Evidence and future facilities

Record what an optimization avoids, its supported routes and the validation
limits alongside the project's policy. For component selection, the initial
claim is **unrequested preparation is avoided**. Allocated storage, elapsed
time and write reductions need separately bounded ordinary-use observations;
apparent target size does not establish SSD writes or lifetime savings.

Future facilities should document their actual callable surface, owner,
prerequisites, failure behavior and evidence before offering an adoption path.
Link to the owning contract rather than copying its rules here. Experimental
or disabled integrations must remain labelled as such; one qualified
optimization does not qualify the rest of Oxide.
