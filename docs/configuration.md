# Configuration

Oxide works with no configuration. This page covers the settings you can change: where the pool lives, how large it may grow, how many builds run at once, and how to use a removable drive or keep final outputs off the internal disk.

Precedence is **environment variables, then the config file, then built-in defaults**.

## Config file

| Platform | Location |
| --- | --- |
| macOS | `~/Library/Application Support/oxide/config/config.toml` |
| Elsewhere | `$XDG_CONFIG_HOME/oxide/config.toml`, or `~/.config/oxide/config.toml` when `XDG_CONFIG_HOME` is unset or relative |

Set `OXIDE_CONFIG` to an absolute path to use a different file. On macOS, `~/.config/oxide/config.toml` is never read, and `oxide status` says so.

The file holds machine settings only. It may be up to 64 KiB, and unknown keys are refused, so a typo fails loudly instead of being ignored:

```toml
[pool]
path = "~/oxide-pool"   # absolute after ~ expansion; no "." or ".." components
budget = "100GB"        # or a whole number of bytes
max_active = 3          # integer from 1 to 32
```

All three keys are optional.

## Pool location

The pool holds build slots, its ledger and its control records. Its default location is:

- macOS: `~/Library/Caches/oxide/build-pool`
- Elsewhere: `$XDG_CACHE_HOME/oxide/build-pool`, or `~/.cache/oxide/build-pool` when `XDG_CACHE_HOME` is unset

Set a different location with `[pool] path` or `OXIDE_POOL`. Oxide refuses a pool path that:

- contains a `.` or `..` component
- lies inside a Git work tree
- lies under `/Volumes` without being a registered removable volume

An explicit pool path that does not exist is refused rather than created silently. Set `OXIDE_POOL_CREATE=1` to allow creation, or place the path inside `OXIDE_TASK_SCRATCH_ROOT`. The default pool is created on first use.

## Size budget

The pool has a soft budget, 100 GB by default. Set it with `[pool] budget`, or for one session with `OXIDE_POOL_CAP`. Sizes are whole bytes, or whole decimal or binary units (`KB`, `MB`, `GB`, `TB`, `KiB`, `MiB`, `GiB`, `TiB`) from 1 MiB to 16 TiB.

The budget is not a quota. When a build needs room, Oxide evicts whole idle slots until the pool is under budget. If only slots that are in use remain, the build waits. A running build may go over the budget.

Oxide also protects the host's disk. It creates no pool when less than 20 GiB is free, and a running build is stopped if free space falls to 22 GiB.

## Concurrent builds

`max_active` limits how many leased builds run at once on the host.

- Default on macOS: `min(4, max(2, floor(physical cores / 4)))`.
- Default elsewhere, or when the core count is unavailable: 2.
- Valid range: 1 to 32.
- Set it with `[pool] max_active` or, for one session, `OXIDE_MAX_ACTIVE`.

Commands beyond the limit wait in first-in, first-out order. `oxide status` lists them.

## Environment variables

| Variable | Purpose |
| --- | --- |
| `OXIDE_CONFIG` | Absolute path of an alternative config file. |
| `OXIDE_POOL` | Pool path. Overrides `[pool] path`. |
| `OXIDE_POOL_CREATE=1` | Allow creating an explicit pool path that does not exist yet. |
| `OXIDE_POOL_CAP` | Budget for a new pool. An existing pool keeps the budget recorded when it was created. |
| `OXIDE_MAX_ACTIVE` | Host-wide limit on active builds, from 1 to 32. |
| `OXIDE_POOL_BACKEND=directory-budget` | The only supported backend. It is a directory pool with a size budget, on every platform. |
| `OXIDE_ALLOW_RUSTC_WRAPPER=1` | Keep an inherited `rustc` wrapper such as sccache. Leased builds bypass it by default. When you keep it, Oxide's interruption and toolchain-identity guarantees no longer cover the build. |
| `OXIDE_COLD_SEED=0` | Turn off cold-slot seeding. It is on by default. |

Pools written by the retired sparse-image backend are refused with a message, and `oxide status` lists them as legacy. Nothing in them is changed or deleted.

## macOS and Time Machine

On macOS, Oxide checks whether the pool is excluded from Time Machine before a build writes to it, and excludes it if needed. A build refuses to start if Oxide cannot confirm the exclusion. Spotlight is not changed.

## Removable drives

On macOS, an APFS volume on a removable drive can hold the pool, so the internal disk does less writing.

```sh
oxide pool register-removable <mount-point> --reserve <size> --cap <size> [--mode ephemeral|permanent]
oxide pool set-mode permanent
oxide pool describe --json
```

- **Registration** records the volume's mount point and UUID, the mode, the free-space reserve and the budget. A different UUID or path is refused. Only one drive can be registered.
- **Modes.** `ephemeral` (the default) keeps final outputs in the project's `target/` on the internal disk. `permanent` moves them to the drive (see [owned outputs](#owned-outputs)). Both modes use the same checks.
- **Selection.** A build uses the drive only when it is mounted, its UUID matches and it is writable. When the drive is absent, Oxide falls back to the internal pool and prints a warning. A different drive under the same mount point is refused.
- **Free space.** Space below the reserve starts pruning. It does not block a build unless pruning cannot recover enough.
- **Removal.** Unplugging the drive during a build stops that build and reports the lost volume. A slot whose writers cannot be confirmed as stopped stays held. `oxide pool recover-interrupted <slot>` releases it once you are sure nothing is writing to it.
- `oxide pool describe --json` reports the registration without changing anything. Tools that need the drive should read it rather than guess at mount points.

## Owned outputs

In `permanent` mode, Oxide places a checkout's final binaries on the registered drive and links them into the project. Oxide owns those links and only follows links that point at its own marked directories.

- **Roots** live at `<mount-point>/oxide-outputs/<project>-<checkout id>/`. If the drive is unavailable, outputs stay in `<checkout>/.oxide-outputs/local/`.
- **Git rules.** Before the first link is published, Git must ignore the member paths (`target/debug`, `target/release`, and their triple variants). A rule such as `/target` covers them. A tracked or negated path does not. Oxide never rewrites `.gitignore`. If the rules are missing, an explicit adoption refuses and names the fix. Automatic conversion leaves the checkout unconverted and prints one notice.
- **Automatic conversion.** A fresh checkout with no existing `target/` converts itself on its first build, when the permanent drive is usable. Existing output is never moved.
- **Commands.** `oxide outputs status` and `oxide outputs plan` are read-only. `oxide outputs adopt` converts a checkout explicitly. `oxide outputs detach` restores a real, empty `target/` and keeps every output that is already on the drive. `oxide outputs prune` removes old output roots. Use `--dry-run` first, then apply the exact plan with `--plan <file> --plan-sha256 <hex>`.
- **Limits.** A single command handles at most eight member directories, and a checkout's output receipt records at most 32.

For a checkout that should keep its output on the internal disk, run `oxide outputs detach`.

## Cargo home cleanup

`oxide clean --cargo-cache` runs one pass of Cargo's own cleanup of the Cargo home (`CARGO_HOME`, or `~/.cargo`). It refuses when the environment or Cargo configuration could disable the cleanup, redirect or fetch from the network, or change sources, registries or credentials. It also refuses when `Cargo.lock` is missing, oversized, or pulls from anything other than crates.io, and when the workspace is not covered by Oxide's guidance block. See [commands](commands.md#oxide-clean) for the outcomes.
