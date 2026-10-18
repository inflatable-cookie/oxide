---
name: oxide
description: Build, test and run Rust through Oxide. Use in any Rust project whose AGENTS.md carries the Oxide block, or when the user mentions oxide. Covers the command rule, profiles, the reported strategy, waits and refusals, oxide status, and oxide clean versus cargo clean.
---

# Oxide

Oxide is Cargo with managed build storage, profiles and execution.

## The rule

Write `oxide` wherever you would write `cargo`. Arguments are Cargo's.

```sh
oxide check
oxide test -p core -- --nocapture
oxide run -- --port 8080
oxide add serde        # non-building commands are plain Cargo
oxide nextest run      # external subcommands work too
```

Plain `cargo` still works but builds privately and wastes space;
`oxide status` reports that output.

## Profiles

- `quick` (default): line tables, fastest feedback.
- `--profile debug`: full debug info, for a debugger session.
- `--release` or `--profile release`: the project's own release profile.

External tools name the Cargo profile themselves, for example
`oxide nextest run --cargo-profile oxide-quick`.

## What Oxide prints

After each building command, one stderr line:
`oxide: quick profile, build strategy, slot 3 warm, 4.2s, pool 210 of 24576 MiB used`.
The strategy is how Oxide met the profile (`build`, or `build+execute` for
`run` and `test`). Cold means the slot was rebuilt; that is normal after
`oxide clean`. A new worktree of the same repository usually starts warm on
another checkout's idle slot: the line then adds what the freshness gate
refreshed and that inputs outside Git or ignored by it are not checked. If
Oxide says the gate refused a slot, it builds cold instead; that is safe.

## Waits and refusals

- `oxide: waiting for …` means another build holds the slot or the host limit.
  Let it wait; do not kill it or fall back to `cargo`. Ctrl-C cancels a wait
  (exit 130).
- Oxide places build output, so it refuses `--target-dir`, a leading
  `+toolchain`, `--config` keys for `build.build-dir` / `build.target-dir`, and
  `--config` file paths. Drop the flag; do not work around it.
- Exit codes are Cargo's, or the program's for `oxide run`.

## Status and cleaning

- `oxide status`: pool use, slots, waiting commands, and plain-Cargo output in
  this checkout's `target/`. It never creates the pool.
- Use `oxide clean`, never plain `cargo clean`. `oxide clean` waits for active
  commands in this checkout, removes its private output and slot affinity, and
  leaves shared slots intact. Plain `cargo clean` does not know the slot; with
  Oxide's storage environment it would delete the shared build directory.

## Adoption

`oxide init` (or `oxide new <path>`) makes a Cargo project if needed and
writes a short guidance block into the workspace-root `AGENTS.md`. Re-running is
safe. It prints readiness, including Effigy selectors that still call plain
`cargo`. It never edits Effigy files; switching a selector to `oxide` is the
project's choice.
