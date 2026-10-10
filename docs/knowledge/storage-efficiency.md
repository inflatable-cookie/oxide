# Storage-efficiency findings

Read-only source review, 2026-10-10. This note records current behavior and
evidence limits. It does not establish measured storage savings or authorize
changes in the inspected projects.

## Component preparation

At Acowtancy commit `5db16a550fc8bf572465be28d1dac11117fa7538`,
`paseo.json` runs `effigy worktree:rust:seed` during every worktree setup,
without a target filter. The seeder's defaults include
`apps/farmyard/target`, `apps/bovine-desktop/src-tauri/target`, and
`platform/silo/target`. Its `--path` argument already allows callers to choose
specific target directories. It first tries `cp -c -R` and falls back to
`cp -R`; a Rust compiler mismatch uses a separate toolchain-keyed source
target, built only when the primary checkout is clean. Cache readiness is
published after the matching compiler and Cargo build are confirmed.

The setup therefore prepares all three component targets before it knows which
component a worktree will use. The path-selective mechanism exists, but setup
does not use it. On APFS a clone can share file data until it diverges; the
plain-copy fallback can retain a full copy. No allocated-byte comparison or
per-component setup-latency measurement was made here, so this review cannot
quantify the storage or time avoided by selecting fewer targets.

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

The inspected source revisions are Acowtancy `5db16a550fc8bf572465be28d1dac11117fa7538`,
Paseo Northstar Queue `7b9b840354a638a529ab023542903949b5b562ac`, and Nucleus
`92dfb60cbae3b2eece70f03aa448e7b0cebc61d7`. Oxide's documentation base was
`91343a280ba275079fc5f329bb3b07d419d9cdb9`.

This assessment did not run builds, tests or Cargo metadata, inspect target
trees, measure allocated storage or disk writes, or change consumer state.
Apparent target size is not allocated storage. A smaller retained target is
not proof of fewer SSD writes, and no SSD-life estimate follows from these
findings.
