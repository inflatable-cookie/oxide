# Commands

`oxide` takes Cargo's commands and arguments unchanged, and adds its own storage, profiles and execution around them. Run `oxide --help` for the short usage summary. This page explains what each class of command does.

## Command classes

| Class | Commands | What happens |
| --- | --- | --- |
| Modelled | `check`, `test`, `clippy`, `build`, `run` | Profile-aware and leased. Oxide picks a compatible slot and sets the build directory for you. |
| Building passthrough | `doc`, `bench`, `rustc`, `rustdoc`, `fix`, and any unknown subcommand such as `nextest` or `llvm-cov` | Runs Cargo under a lease with Oxide's profile recipe. |
| Plain passthrough | `fmt`, `tree`, `metadata`, `add`, `remove`, `update`, `fetch`, `package`, `publish`, `install` and similar | Runs plain Cargo with no lease. |
| Exact passthrough | `deny`, `audit`, `outdated`, `machete` | Replaces itself with your Cargo, with the same arguments, directory, environment and exit status. |
| Extended | `init`, `new`, `clean` | Cargo's behaviour plus Oxide's step. See [getting started](getting-started.md) and [clean](#oxide-clean). |
| Oxide-owned | `exec`, `status`, `pool`, `outputs` | Oxide's own commands, described below. |

Two rules apply everywhere:

- `--release` means `--profile release`. `--profile quick|debug|release` is Oxide's and is consumed by Oxide. Cargo's own `--cargo-profile` passes through to Cargo.
- `oxide <command> --help` runs `cargo <command> --help` and touches no storage.

Oxide refuses these before doing anything, because it places build output itself:

- `--target-dir`, and `--config` keys `build.build-dir` or `build.target-dir`
- `--config` file paths
- a leading `+toolchain`. Use `RUSTUP_TOOLCHAIN=<toolchain> oxide ...` or a `rust-toolchain` file instead.

## Profiles

| Profile | Use it for | Recipe |
| --- | --- | --- |
| `quick` (default) | Fastest feedback | Line-table debug info for every unit. |
| `debug` | Stepping through code in a debugger | Full debug info for every unit, including build scripts. |
| `release` | Production builds | Your project's own `release` profile. |

`quick` and `debug` set Cargo's `dev` profile through environment variables only. Your `Cargo.toml` is not changed. Output lands in `target/debug/`, as with plain Cargo. `test` uses the same recipe. `bench` keeps Cargo's release-based default unless you pass a quick or debug profile.

The older `--cargo-profile oxide-quick` and `--cargo-profile oxide-debug` still build but are deprecated. Drop the flag and use `--profile` instead.

## Time budgets

Leased commands accept two optional limits:

```sh
oxide test --execution-budget 90s
oxide check --admission-budget 45s --execution-budget 300s
oxide run --execution-budget 120s
```

- `--admission-budget` covers everything before the build starts: setup, waiting for a slot and the freshness check. If it expires, nothing starts and Oxide exits with **124**.
- `--execution-budget` starts when the build process is seen to start. It covers compilation, tests and cleanup. If it expires, Oxide stops the process group (TERM, a short grace period, then KILL) and exits with **125**.
- A `run` keeps one window across the build and the program.
- Durations are whole seconds or milliseconds, such as `30s` or `1500ms`. Minutes and hours are refused.
- `--budget-events <fd>` writes one JSON record per line to an already-open, non-blocking pipe. If the pipe is lost, the run is reported as unknown rather than successful.

An omitted budget is unlimited. A nested `oxide` call uses the outer window rather than starting a deadline of its own.

## `oxide exec`

`exec` runs a tool that calls Cargo itself under Oxide's storage and profile rules:

```sh
oxide exec --profile release -- npm run tauri build
oxide exec --manifest-path crates/app/Cargo.toml -- ./scripts/package.sh
```

- Oxide's options come before `--`. Everything after `--` is run exactly as given, without a shell.
- `--profile` picks the slot and recipe only. The tool still chooses its own dev or release build.
- Final outputs go to `<workspace>/target`, or to the profile links in a converted checkout (see [configuration](configuration.md#owned-outputs)). `--output <member>` declares the complete set of output directories. If you leave it out, Oxide derives the set from the checkout's recorded output receipt.
- A tool that passes its own `--target` needs a matching `--output <triple>/debug` or `--output <triple>/release`.
- The tool leads its own process group. Oxide holds the slot until that group is empty. Ctrl-C and termination signals reach the whole group.
- Exit codes: the tool's own code, or 128 plus the signal number. **127** means the command was not found and **126** means it could not start.

`exec` is cooperative. A tool that sets its own build directory, changes its workspace or toolchain, or detaches with `setsid` escapes Oxide's custody, and Oxide makes no guarantee about it.

## `oxide run`

`run` builds with the recipe and then runs the program. It chooses the binary the same way `cargo run` does: `--bin`, then `--example`, then `default-run`, then the only binary. If several binaries qualify, it refuses before taking a lease. The program receives the environment `cargo run` gives it, including `CARGO_MANIFEST_DIR` and the package's `CARGO_PKG_*` values.

## `oxide status`

- `oxide status` is read-only and fast. It reports the selected pool, slots and holders, waiting commands, the last recorded size of each slot and its age, the config file in use and the effective host limit.
- `oxide status --measure` adds bounded walks of the pool, the slots, the checkout's stray output and the Cargo home, under one 60-second budget. It still writes nothing.
- `oxide status --launch-check` (macOS) times the first launch of a new executable against a re-run. It runs under a 3-second limit and changes no setting.

## `oxide pool`

`pool` manages removable drives. See [configuration](configuration.md#removable-drives) for the full workflow.

```sh
oxide pool register-removable <mount-point> --reserve <size> --cap <size> [--mode ephemeral|permanent]
oxide pool set-mode <ephemeral|permanent>
oxide pool describe --json
oxide pool recover-interrupted <slot>
```

`describe --json` prints a stable, read-only description of the registered drive (schema `oxide.pool.describe/1`). It changes nothing.

## `oxide outputs`

`outputs` manages where final binaries are stored when the mode is `permanent`.

```sh
oxide outputs plan              # read-only: what would change, as JSON
oxide outputs status            # read-only
oxide outputs adopt             # move this checkout's outputs to the drive
oxide outputs detach            # restore a real, empty target/
oxide outputs cancel-reservation
oxide outputs prune --dry-run   # preview removal of old output roots
oxide outputs prune --plan <file> --plan-sha256 <hex>
```

`prune` removes only roots that no pin or hold protects. Unknown legacy roots are kept.

## `oxide clean`

```sh
oxide clean                        # remove this checkout's target/ output
oxide clean --dry-run              # list every path, change nothing
oxide clean --include-foreign      # also remove entries Oxide did not create
oxide clean -p core                # Cargo's own selected clean, on Oxide's private target
oxide clean --cargo-cache          # one pass of Cargo's cache cleanup, no network
```

A whole-target clean waits for running commands in the checkout and blocks new ones until it finishes. It never removes a shared slot. Entries that Cargo does not create in `target/` make the clean refuse, and are listed, unless you pass `--include-foreign`. `--dry-run` and `--include-foreign` cannot be combined with a selection such as `-p`.

`--cargo-cache` takes no other arguments. It runs Cargo's own cache cleanup once, with network access disabled, and it refuses when the environment or Cargo configuration could redirect or fetch. It reports one of three outcomes: `ran`, `skipped` (Cargo's own reason, such as a held lock) or `failed`. It does not report how much space was freed.

## `oxide init` and `oxide new`

- `oxide init [cargo init args] [path]` runs `cargo init` when there is no `Cargo.toml`, then adopts the project.
- `oxide new <path> [cargo new args]` runs `cargo new` and then adopts the project.

Adoption is described in [getting started](getting-started.md#2-adopt-a-project).

## Cargo versions

Oxide's build-directory layout, freshness check and seeding depend on Cargo internals, so it verifies each Cargo release before it trusts that release.

| Cargo | Shared reuse | Cold-slot seeding | Stale-unit pruning |
| --- | --- | --- | --- |
| 1.95.0, 1.96.1, 1.97.1, 1.98.1, 1.99.1 | Yes | Only on 1.97.1 | No (layout v1) |
| 1.100.0 | Yes | Yes (layout v2) | Yes (layout v2) |
| Any other release | No | No | No |

On any other Cargo release, Oxide still builds, but each slot is used only by its own checkout. Oxide reads the layout from the directories Cargo wrote, not from the version number alone.
