# Storage-efficiency findings

Source findings and bounded preparation evidence. This note records current
behavior and evidence limits. It does not establish measured storage savings
or authorize changes in the inspected projects.

## Component preparation

The reusable project routing requirements live in the
[optimization adoption policy](contracts/optimization-adoption.md#component-lazy-preparation).

At Acowtancy commit `70b6d7c8c83a48a85abbbd24717ed8064898cf5a`,
worktree setup no longer seeds host Rust targets. Supported host Rust selectors
prepare only their declared Farmyard, Desktop or Silo component; composed
selectors request an explicit union. JS/docs-only setup prepares none.
`effigy worktree:rust:seed --component <name>` selects a component, repeated
flags select a union, and unknown components refuse. Explicit `--path`
selection remains available. An unfiltered direct seed command remains an
operator opt-in for all three targets. Acow owns the
[route inventory and preparation policy](https://github.com/acowtancy/acowtancy-v2/blob/70b6d7c8c83a48a85abbbd24717ed8064898cf5a/docs/knowledge/operations/267-qa-speed.md#shared-compile-cache).

The copy mechanism first tries `cp -c -R` and falls back to
`cp -R`; a Rust compiler mismatch uses a separate toolchain-keyed source
target, built only when the primary checkout is clean. Cache readiness is
published after the matching compiler and Cargo build are confirmed.

The preceding source assessment at `5db16a550fc8bf572465be28d1dac11117fa7538`
identified unconditional setup of all three targets. Component selection now
avoids that unrelated preparation; it does not add a shared mutable compiler
manager. Existing no-overwrite, primary-target, compiler-matching,
dirty-primary, locking and readiness protections remain. Container target
ownership is separate.

Independent review at `91d6e4b2af18e977c8a323351004d67b799ec749`
passed all five preparation, routing, protection, isolation and scope gates.
The author and reviewer each ran 18 passing synthetic fixture tests in
separate fresh roots. The reviewer also ran the focused seed compile/format
and documentation checks; no real Cargo or rustc was invoked. Component-only
and union fixtures leave unrelated destinations absent. These fixtures prove
avoided unrelated preparation, not real component compilation or a complete
milestone QA result.

On APFS a clone can share file data until it diverges; the plain-copy fallback
can retain a full copy. Allocated bytes, ordinary-use latency, write volume and
SSD lifetime savings remain unmeasured. The evidence does not quantify them.

Acowtancy's existing Rust profiles already distinguish ordinary and test
work: Farmyard `dev` keeps incremental compilation and debug level 1, while
its `test` profile keeps workspace line tables, removes dependency debug info
and disables incremental output. Desktop also has separate dev and test
profiles. These settings are existing project policy, not evidence for a new
profile change.

Acowtancy's recorded sccache 0.18.0 trial at the pinned Farmyard toolchain
reported zero hits for fresh targets and for a warm cache at a different
target path; forced recompilation at the same path reported three hits. The
trial is configuration-specific. It does not reject shared compiler caching
for every project or configuration.

## Profiles and debug access

At Nucleus commit `92dfb60cbae3b2eece70f03aa448e7b0cebc61d7`, the root Cargo
profile sets `dev` debug info to line tables and defines `full-debug` with full
debug info. Its validation guidance keeps those routes distinct. The workspace
does not declare a `rust-version`; no MSRV is inferred here. Acowtancy's
existing test-profile reductions and Nucleus's explicit full-debug route leave
no source-backed case for another profile edit in this assessment. Disabling
incremental output can increase rebuild work, and final target size alone
would not establish reduced disk writes.

## Incremental and debug storage measurements

Supervisor-reported, single busy-machine observations on Darwin 25.6 with APFS,
rustc 1.97.1 (`8bab26f4f`) and matching Cargo. Nucleus's dev profile is
`line-tables-only` with the macOS default split-debuginfo mode (unpacked). The
raw logs were not preserved, so these figures are attributed to the supervisor
report and not re-observed here.

| Mode (cold + two changed no-run builds) | Build time | Loose `deps` objects | Incremental state | Notes |
| --- | --- | --- | --- | --- |
| Incremental on, unpacked | 60 s + 38 s = 98 s | 12,303 | about 3 GB | Object set grows across builds. |
| Incremental off, unpacked | 66 s + 107 s = 173 s | 408, unchanged after warm | none | Object set flat. |
| Packed, incremental setting not reported | 82 s + 59 s = 141 s | zero loose | about 3 GB incremental state and about 2x deps, as reported | The source report does not state the incremental setting for this row. Do not read it as incremental off. |

Method and caveats:

- Compile-only no-run builds. No test execution is in these figures.
- Packed-mode file:line and LLDB behavior were not tested.
- The packed row's incremental setting is not in the source report. It is not a
  clean comparison with the off/unpacked row, and it does not establish a packed
  effect on incremental state.
- The 37.4 GB `deps` object figure in the source report is apparent size across
  139,421 names. It is not proven stale or reclaimable. It may overlap the 38.4 GB
  incremental figure through hardlinks and multiple variants.
- Incremental off does not show lower cumulative writes. It removes retained
  incremental state; changed-crate recompilation still writes objects.
- Cache sharing does not prevent per-slot growth. Shared warm reuse is a separate
  quantity from bounded growth and actual writes.

The proposed policy and a frozen one-shot comparison are in the
[build-context policy](contracts/build-context-policy.md). The comparison is
not yet run.

## Disposable output and retention

At Queue commit `7b9b840354a638a529ab023542903949b5b562ac`, Spec 028 keeps
Cargo build outputs task-local while sharing downloaded Cargo sources. Its
close path discovers package and workspace target directories and removes
only task-local Cargo targets and `node_modules` after the clean, published
head, idle-thread and no-live-run proofs. Cleanup checks configured Cargo
target paths, tracked content and symlink ownership; unproved roots are
retained and reported. Failed or cancelled unmerged worktrees are retained
under the owning workspace policy. Run evidence and preserved artifacts have
separate owners and are not disposable build output.

Nucleus Contract 002 likewise separates build-output retention from source,
task history and evidence retention. It retains failed or cancelled unmerged
worktrees for inspection and does not allow age alone to authorize removal.
The reviewed controls already bound cleanup to ownership and settlement
proofs; this review found no safe basis for shortening retention or deleting
older or uncertain resources.

## Evidence limits

The initial assessment inspected Acowtancy `5db16a550fc8bf572465be28d1dac11117fa7538`,
Paseo Northstar Queue `7b9b840354a638a529ab023542903949b5b562ac`, and Nucleus
`92dfb60cbae3b2eece70f03aa448e7b0cebc61d7`. Oxide's documentation base was
`91343a280ba275079fc5f329bb3b07d419d9cdb9`.

That read-only assessment did not run builds, tests or Cargo metadata, inspect
target trees, measure allocated storage or disk writes, or change consumer
state. The separately owned component-preparation evidence above uses small
synthetic targets and applies to Acowtancy's host routes only.
Apparent target size is not allocated storage. A smaller retained target is
not proof of fewer SSD writes, and no SSD-life estimate follows from these
findings.
