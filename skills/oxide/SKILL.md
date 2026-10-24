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
oxide deny check       # deny/audit/outdated/machete forward unchanged, no lease
oxide nextest run      # other external subcommands are leased
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

## Version

`oxide --version` (or `-V`) prints this build's package identity, its source
pin — ``unknown`` for a build the installer did not stamp — and whether it is
the installer's verified binary (with the receipt's digest kept separate from
the binary's), then Cargo's own version. `oxide --list` passes straight to
Cargo.

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
  (127: program not found). Exit 75 with
  `oxide: oxide was upgraded during this command; re-run.` means this `oxide`
  is older than the pool's state and could not prove the installed binary
  understands it. Run the command again from a shell that resolves the new
  `oxide`; do not retry in a loop, delete state or fall back to plain `cargo`
  for a slot build.
- `oxide: handing off to the installed binary (state is newer than this one).`
  means this `oxide` was older than the pool's state, so the command was
  re-run once as the installed binary with the same arguments. That is normal
  after an upgrade; the next command starts on the new binary.
- `oxide: reserve stop: …` on a directory pool means host free space fell
  below about 22 GiB, so Oxide stopped the build to protect the 20 GiB
  reserve. Free disk space before retrying; do not loop. If it says closure
  is unconfirmed, the slot stays held until those processes exit.

## Status and cleaning

- `oxide status` is fast and read-only by default. It reports recorded facts
  only: pool and slot membership, each slot's recorded directory, holders,
  waiting commands, clean marks, flushes, the registered drive mode and latest
  use. A slot size is a recorded observation from an earlier acquiring
  command, shown with its age, source and lease, or `not measured`; it may have
  changed since, and recorded sizes are never a current pool total. It takes
  no pool lock, creates no lock, reconciles and deletes nothing, walks no
  payload, checkout or Cargo home, and runs no Cargo, `tmutil` or `diskutil`.
  A slot recorded as being deleted shows as a pending reconciliation, not as
  idle. Cargo-home and stray-output lines say not measured.
- `oxide status --measure` runs the bounded whole-payload walk, each slot's own
  walk, the checkout stray-output scan and the Cargo home once, under one
  cumulative 60 s budget, and reports current bytes and coverage. It still takes
  no pool lock, reconciles and writes nothing, and does not refresh the
  recorded sizes; a recorded deletion stays pending and makes coverage unknown.
- Each recorded lease-ticket PID comes with an approximate age from the
  owner-private ticket file's creation time (or modification time if creation
  time is unavailable). Missing, symlinked, foreign, unreadable or future
  timestamps remain unknown. The lookup reads metadata without opening or
  mutating the ticket; wall-clock jumps cannot all be detected, and age never
  affects admission or recovery. A ticket can be stale, so its PID and age do
  not prove that a command is still alive. Clean commands waiting for the
  checkout barrier have no ticket yet and are not listed. Ticket age is a
  wall-clock file age, not the measured monotonic wait.
  `oxide status` never creates the pool.
- `oxide pool describe --json` (macOS): the registered drive as stable JSON
  (schema `oxide.pool.describe/1`). Branch on `available`, not on reason
  strings. It reports the registered mode even when the volume is unavailable.
  It changes nothing. The answer is a snapshot: before writing into
  `suggested_scratch_root`, recheck the volume UUID, the mount root, that the
  device differs from both `/` and `/System/Volumes/Data`, and free space above
  `reserve_bytes`. The suggestion is not write permission, and nothing writes
  under `payload`. The full contract is in `docs/knowledge/contracts/workflow.md`,
  section `oxide pool describe --json`.
- A registered removable drive has `ephemeral` or `permanent` mode. Register
  one with `oxide pool register-removable <mount> --reserve <size> --cap
  <size> [--mode ephemeral|permanent]`; the default is `ephemeral`. Change an
  existing registration with `oxide pool set-mode <ephemeral|permanent>`.
  The setting can be changed while the drive is absent. Both modes use the
  same availability checks and pool preference, and final outputs stay in
  `<workspace>/target`.
- If a permanent drive cannot be used, Oxide warns before admission with the
  actual fallback reason, then includes that reason in the command summary
  when one is produced. Ephemeral mode keeps the existing fallback line.
  `oxide status` shows configured mode separately from the pool selected for
  the next build, including its fallback reason.
- In permanent mode a checkout's `<workspace>/target` becomes an owned link
  into the drive, after its first build (one notice) or an explicit `oxide
  outputs adopt`. To keep one checkout's outputs in place, run `oxide outputs
  detach` in that checkout: it restores a real empty `target`, keeps the
  drive's output root and its bytes, and records the checkout as detached.
  Detach works in either mode. A detached checkout stays unconverted
  automatically; the next builds use its real `target`. Before its first
  build, create a real empty `<workspace>/target` directory to opt that
  checkout out. Oxide never converts an existing real `target` automatically.
  `oxide outputs adopt` converts a detached checkout again, but only when its
  `target` is missing or empty; a nonempty real target is refused and kept.
- A first conversion (`oxide outputs adopt`, or the automatic one on a fresh
  checkout's first build in permanent mode) publishes the owned
  `<workspace>/target` link only when Git ignores that exact non-directory
  path. A `target/`, `/target/` or `**/target/` rule ignores directories only
  and refuses; add `/target` (or `target`), not `target/`, to the repository
  `.gitignore`. A nested workspace needs its exact repository-relative rule,
  for example `/rust/target`. A tracked `target`, an unignored or negated
  path, or a Git decision Oxide cannot confirm (no repository, no `git`, a
  timeout, an error or a bad answer) also refuse, and nothing changes.
  Already-created links, `detach`, `status`, `plan` and ordinary builds are
  unaffected.
- A root whose checkout is gone is never cleaned automatically. Inspect it
  with `oxide outputs prune --dry-run`, which prints a bounded preview and its
  SHA-256 and changes nothing; a real run needs those exact bytes
  (`--plan FILE --plan-sha256 HEX`) and revalidates every predicate. Roots
  whose checkout is present, whose selection is not the last live one, whose
  writer custody is unknown, whose marker or volume does not verify, or which
  predate the version-2 registry identity are kept. A root refused by a
  fresh check before its intent is kept and the run continues: exit 0 lists
  what was deleted and kept, exit 2 means nothing changed. An unfinished intent
  reports exit 3 and is settled only with `oxide outputs resolve-intent
  <intent-id>` when both the original and quarantine leaves are positively
  absent. Never prune by hand or delete an `oxide-outputs` directory.
- A build that needs a new owned root reserves its nonce first; a
  `cancelled or replaced` refusal means another command or
  `oxide outputs cancel-reservation` changed that reservation: run the command
  again.
- On a pool converted before the version-2 registry, a build's relink refuses
  until `oxide outputs adopt` or `oxide outputs detach` upgrades the format;
  that upgrade waits for no other Oxide command to run in either pool.
- Old version 1 registrations without a mode read as ephemeral and remain
  unchanged until an explicit mode update. An update writes version 2 with a
  required mode. Older Oxide binaries refuse version 2; reinstalling a binary
  does not restore registration metadata.
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
- Oxide finds its config, pool and Builds drive from your account's home
  directory, not `HOME`, so a temporary `HOME` still uses the shared pool
  (with a one-line notice). `OXIDE_POOL` selects another pool and
  `OXIDE_CONFIG` another config file; `status` still reads the account's own
  records.
- `OXIDE_POOL_CAP` sets the cap only when a pool is first created.
- Use `oxide clean`, never plain `cargo clean`. `oxide clean` waits for active
  commands in this checkout, removes its private output and slot affinity, and
  leaves shared slots intact. If `target/` holds anything Cargo did not
  create, it refuses and names it: move it out, or pass `--include-foreign`.
  `oxide clean --dry-run` prints every path a clean would remove. Plain `cargo clean` does not know the slot; with
  Oxide's storage environment it would delete the shared build directory.

## Adoption

`oxide init` (or `oxide new <path>`) makes a Cargo project if needed and
writes a short guidance block into `AGENTS.md`: the Git work-tree root's when
the project is inside Git, listing the Cargo workspaces it covers, else the
project's own. Re-running is safe. It prints readiness, including Effigy selectors that still call plain
`cargo`. A `:cargo` task or a `-cargo` test suite that calls plain `cargo` is
acknowledged once per run as an intentional escape; any other selector that
calls plain `cargo` is an actionable warning. It never edits Effigy files and
remembers nothing; switching a selector to `oxide` is the project's choice.

When switching selectors:
- **Escapes:** keep a `:cargo` escape beside each main one.
- **CI:** if CI calls the same selectors, run `cargo` when `GITHUB_ACTIONS=true` and `oxide` otherwise, failing loudly when `oxide` is missing.
- **Stay on `cargo`:**
  - scripts that set their own `CARGO_TARGET_DIR`, because Oxide replaces it;
  - tools that run Cargo themselves, such as the Tauri CLI, unless run through `oxide exec` and checked in that project;
  - container builds;
  - tests that find sibling binaries from `current_exe()`, unless repaired. Test executables run from the slot, not `target/`. Use `env!("CARGO_BIN_EXE_<name>")` or `$CARGO_TARGET_DIR`.

  Full rules: [workflow](../../docs/knowledge/contracts/workflow.md#switching-an-existing-project).
