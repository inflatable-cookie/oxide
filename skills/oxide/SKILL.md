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
oxide exec -- npm run tauri build   # a tool that runs Cargo itself
```

`oxide exec [--manifest-path <Cargo.toml>] [--profile quick|debug|release] -- <command…>`
runs one tool that calls Cargo for one workspace with Oxide's storage, recipe
and wrapper policy. Oxide's options go before `--`; the command after it is
kept exactly and keeps your working directory. `--profile` picks the recipe
only: the tool still chooses dev or release itself. Outputs stay in
`<workspace>/target`. The slot stays held until every process in the tool's
group exits. A terminal stdin is not passed (the tool reads end-of-file).
Cooperative only: a tool that sets its own `CARGO_TARGET_DIR`, switches
workspace or toolchain, or daemonizes escapes Oxide.

Plain `cargo` still works but builds privately and wastes space;
`oxide status` reports that output.

## Profiles

- `quick` (default): line tables, fastest feedback.
- `--profile debug`: full debug info, for a debugger session.
- `--release` or `--profile release`: the project's own release profile.

`quick` and `debug` tune Cargo's `dev` profile, so binaries land in
`target/debug/` as with plain Cargo; the latest build wins.
External tools get the quick recipe with no flag: `oxide nextest run`.
`--cargo-profile oxide-quick` is deprecated and builds into
`target/oxide-quick/`. A tool's `--cargo-profile debug` is Cargo's own
profile, not Oxide's `--profile debug`.

## What Oxide prints

After each building command, one stderr line:
`oxide: quick profile, build strategy, slot 3 warm, 4.2s, pool 210 MiB allocated of 95367 MiB budget (directory-budget-v1, budget-only)`.
The pool's soft budget (100 GB by default) steers admission and eviction but
is not a ceiling: a running build may pass it.
The strategy is how Oxide met the profile (`build`, or `build+execute` for
`run` and `test`). Cold means the slot was rebuilt; that is normal after
`oxide clean`. A new worktree of the same repository usually starts warm on
another checkout's idle slot: the line then adds what the freshness gate
refreshed and that inputs outside Git or ignored by it are not checked. If
Oxide says the gate refused a slot, it builds cold instead; that is safe.

If admission waited, Oxide prints `oxide: waited …; slot ready` after the
final freshness gate and cancellation check, before spawning Cargo or a tool.
The summary adds the cumulative monotonic wait duration across reacquisitions.
The existing elapsed value keeps its whole-invocation meaning, including
freshness-gate and execution work. The measured wait is attributed to the last
observed exclusive reason (queue, host limit, checkout, clean, project cap,
uncertain slot or capacity); polling and admission work after that observation
stays with the category. These reasons are observations, not root causes, and
do not include Cargo-lock, Cargo or execution time. If checked monotonic timing
fails, Oxide reports that the wait duration is unavailable.

## Waits and refusals

- `oxide: waiting for …` reports the current observed admission reason. A
  command prints a changed reason up to eight times, then one notice that
  further changes are omitted; queue-position changes alone do not print
  another reason line. The reason categories and measured durations are
  approximate observations of waiting state, not simultaneous root causes.
  A second command in a busy
  checkout normally takes another slot instead of waiting. Let it wait; do not kill it or fall back to `cargo`. Ctrl-C cancels a wait
  (exit 130).
- Oxide places build output, so it refuses `--target-dir`, a leading
  `+toolchain`, `--config` keys for `build.build-dir` / `build.target-dir`, and
  `--config` file paths. Drop the flag; do not work around it.
- Exit codes are Cargo's, or the program's for `oxide run` and `oxide exec`
  (127: program not found).
- `oxide: reserve stop: …` on a directory pool means host free space fell
  below about 22 GiB, so Oxide stopped the build to protect the 20 GiB
  reserve. Free disk space before retrying; do not loop. If it says closure
  is unconfirmed, the slot stays held until those processes exit.

## Status and cleaning

- `oxide status`: pool use, slots (size and file counts), waiting commands,
  and each recorded lease-ticket PID with an approximate age from the
  owner-private ticket file's creation time (or modification time if creation
  time is unavailable). Missing, symlinked, foreign, unreadable or future
  timestamps remain unknown. The lookup reads metadata without opening or
  mutating the ticket; wall-clock jumps cannot all be detected, and age never
  affects admission or recovery. A ticket can be stale, so its PID and age do
  not prove that a command is still alive. Clean commands waiting for the
  checkout barrier have no ticket yet and are not listed. Ticket age is a
  wall-clock file age, not the measured monotonic wait.
  Also reports plain-Cargo output in this checkout's `target/` and the Cargo
  home's size.
  It never creates the pool.
- `oxide pool describe --json` (macOS): the registered drive as stable JSON
  (schema `oxide.pool.describe/1`). Branch on `available`, not on reason
  strings. It changes nothing. The answer is a snapshot: before writing into
  `suggested_scratch_root`, recheck the volume UUID, the mount root and free
  space above `reserve_bytes`. The
  suggestion is not write permission, and nothing writes under `payload`.
- `oxide status --launch-check` (macOS) times a new executable's first launch
  against a re-run, in about a second. Above 30 ms it names the app to add
  under Privacy & Security → Developer Tools. It changes no setting; leave
  that change to the user, and never disable Gatekeeper.
- `oxide clean --cargo-cache` asks Cargo to clean its own download cache once,
  with network refused. Use it when the Cargo home is large and builds always
  run offline. It reads Cargo config and refuses (naming the key or file) for
  source replacement, registries, credential providers, `git-fetch-with-cli`,
  includes, unknown keys or unparseable files, and for git or custom sources,
  a missing `Cargo.lock` and similar routes; it reports `ran`,
  `skipped` or `failed`. Do not delete files in the Cargo home or a slot by
  hand.
- `OXIDE_POOL_CAP` sets the cap only when a pool is first created.
- Use `oxide clean`, never plain `cargo clean`. `oxide clean` waits for active
  commands in this checkout, removes its private output and slot affinity, and
  leaves shared slots intact. If `target/` holds anything Cargo did not
  create, it refuses and names it: move it out, or pass `--include-foreign`.
  `oxide clean --dry-run` prints every path a clean would remove. Plain `cargo clean` does not know the slot; with
  Oxide's storage environment it would delete the shared build directory.

## Adoption

`oxide init` (or `oxide new <path>`) makes a Cargo project if needed and
writes a short guidance block into the workspace-root `AGENTS.md`. Re-running is
safe. It prints readiness, including Effigy selectors that still call plain
`cargo`. It never edits Effigy files; switching a selector to `oxide` is the
project's choice.

When switching selectors:
- **Escapes:** keep a `:cargo` escape beside each main one.
- **CI:** if CI calls the same selectors, run `cargo` when `GITHUB_ACTIONS=true` and `oxide` otherwise, failing loudly when `oxide` is missing.
- **Stay on `cargo`:**
  - scripts that set their own `CARGO_TARGET_DIR`, because Oxide replaces it;
  - tools that run Cargo themselves, such as the Tauri CLI, unless run through `oxide exec` and checked in that project;
  - container builds;
  - tests that find sibling binaries from `current_exe()`, unless repaired. Test executables run from the slot, not `target/`. Use `env!("CARGO_BIN_EXE_<name>")` or `$CARGO_TARGET_DIR`.

  Full rules: [workflow](../../docs/knowledge/contracts/workflow.md#switching-an-existing-project).
