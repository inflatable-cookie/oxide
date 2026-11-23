# Oxide workflow

This contract states how `oxide` behaves today. The implementation is
`src/bin/oxide/`, built on the lease library in `src/lease/`. Storage ownership
(volumes, roots, holds, pins, eviction and trash) belongs to Barge; see the
[Barge adoption contract](barge-adoption.md). The lease, ledger and eviction
rules, and per-slot accounting, are in [local lease](local-lease.md). Agent guidance lives in
[the oxide skill](../../../skills/oxide/SKILL.md). Release and local install
policy is in [release](release.md).

## Invariants

- Oxide never deletes, moves or rewrites a shared slot, a foreign or unknown
  directory, or a Barge root except through the owning Barge operation, with its
  marker, pin and hold checks.
- Unknown is never success. Unknown writer closure, unknown accounting,
  unreadable state and unconfirmed Git answers hold the lease, refuse the
  operation or leave a checkout unconverted. They never fall through to reuse or
  deletion.
- A running command keeps the pool, root and slot it selected. Nothing switches
  mid-run.
- Oxide never falls back to plain Cargo for a leased build. Escapes are explicit
  and operator-chosen (see [switching](#switching-an-existing-project)).
- Read-only commands (`status`, `pool describe --json`, `outputs plan`) change
  nothing. Live pools, registrations, installed binaries and host settings change
  only through their explicit commands, with owner approval where required.

## Command surface

`oxide` is Cargo with managed storage, profiles and execution. Commands fall
into six classes; the exact lists are in [`cli.rs`](../../../src/bin/oxide/cli.rs).

| Class | Commands | Behaviour |
| --- | --- | --- |
| Modelled | `check`, `test`, `clippy`, `build`, `run` | Profile-aware and leased. Slot chosen by identity and freshness. |
| Building passthrough | `doc`, `bench`, `rustc`, `rustdoc`, `fix`, and every unknown or external subcommand (for example `nextest`, `llvm-cov`) | Cargo's command under a lease, with storage and profile definitions injected. Unknown commands count as building. |
| Plain passthrough | Non-building built-ins such as `fmt`, `tree`, `metadata`, `add`, `update`, `fetch`, `locate-project`, `help`, `package`, `publish`, `install` | Plain Cargo, no lease. |
| Exact passthrough | `deny`, `audit`, `outdated`, `machete` | Oxide replaces itself with the caller's Cargo: same arguments, directory, environment, streams and exit status. No slot, pool, adoption or recipe. |
| Extended built-ins | `init`, `new`, `clean` | Cargo's command plus Oxide's step. See [adoption](#adoption-oxide-init-and-oxide-new) and [clean](#clean). |
| Oxide-owned | `status`, `pool`, `outputs`, `exec` | Oxide semantics. `exec` runs a tool that calls Cargo under a lease. |

Rules:

- Arguments pass through unchanged. `--release` means `--profile release`.
  `--profile quick|debug|release` is Oxide's and is consumed. `--cargo-profile`
  is Cargo's.
- Refused before any effect: `--target-dir`; `--config` keys `build.build-dir`
  or `build.target-dir`; `--config` file paths; a leading `+toolchain`. Use
  `RUSTUP_TOOLCHAIN=<toolchain> oxide` or a `rust-toolchain` file instead.
- Oxide injects storage only through environment variables, because child Cargo
  calls inherit them: `CARGO_BUILD_BUILD_DIR` points at the slot,
  `CARGO_TARGET_DIR` at `<workspace>/target`, and `CARGO_PROFILE_DEV_*` carry the
  recipe.
- `oxide <cmd> --help` forwards the caller's whole request to Cargo unchanged —
  `oxide nextest run --help` runs `cargo nextest run --help` — with no storage,
  pool or lease touched. Oxide's own leading budget options are dropped, because
  they are not Cargo's. After `--`, `--help` belongs to the program.
- `oxide --version` (or `-V`) prints Oxide's identity, then Cargo's version. See
  [version identity](#version-identity). `oxide --list` passes straight to Cargo.
- Plain `cargo` still works and builds privately. `oxide status` reports that
  output.

### `oxide exec`: Cargo-wrapping tools

```text
oxide exec [--manifest-path <Cargo.toml>] [--profile quick|debug|release] [--output <member>]… -- <command…>
```

- Oxide's options come before `--`. The command after it is kept exactly,
  spawned without a shell, and keeps the working directory.
- `--profile` chooses the recipe and slot only. The tool still chooses its own
  dev or release build, and Oxide does not rewrite its flags.
- Final outputs go to `<workspace>/target`, or to the profile member links in a
  converted checkout. `--output` declares the tool's complete member set. Without
  it, the set derives from the checkout's committed output receipt. A tool that
  passes its own `--target` needs the matching `<triple>/debug` or
  `<triple>/release` declaration.
- The tool leads a recorded process group. The lease is held until the leader
  exits and the group is empty. Ctrl-C and termination signals (INT, TERM, HUP,
  QUIT) are forwarded to the group, and the group is continued after them so a
  stopped member acts on the signal.
- Exit code: the tool's, or 128 plus the signal. `127` means not found and `126`
  means it could not start.
- A terminal standard input is replaced by `/dev/null`. Interactive prompts are
  not supported.
- `oxide status` shows the holder as `exec <program>`.
- The contract is cooperative. A tool that sets `CARGO_TARGET_DIR` or
  `--target-dir` moves only its final outputs. A tool that sets its own build
  directory, changes workspace or toolchain, or daemonizes with `setsid` escapes
  storage custody, and Oxide makes no crash-closure claim for it.

### Version identity

`oxide --version` prints the package identity, a build line and a provenance
line. An installer-stamped build carries the authenticated source pin and a
per-build instance. Provenance is `verified` only when the newest receipt under
this account's `~/.local/state/oxide/install/` names the running executable,
with the same pin and instance, and matching size and SHA-256. Otherwise the line
says `unverified` with the reason. An unstamped build says `unknown`. The binary
digest and the receipt digest are reported under separate labels.

## Profiles and strategies

| Profile | Intent | Recipe |
| --- | --- | --- |
| `quick` (default) | Fastest feedback | Line-table debug info for every unit (`CARGO_PROFILE_DEV_DEBUG=line-tables-only`). |
| `debug` | Debugger session | Full debug info for every unit, including build scripts. |
| `release` (or `--release`) | Production shape | The project's own `release` profile. |

- `quick` and `debug` tune Cargo's `dev` profile through environment variables
  on every leased Cargo process. No `Cargo.toml` or machine configuration
  changes. Final binaries land in `target/debug/` as with plain Cargo, and the
  latest build wins. `quick` clears the build-override debug setting, so an
  outer `debug` setting never reaches a nested quick build. Dependencies keep
  line tables under `quick`.
- Oxide keeps this `dev` environment recipe for `--profile debug` on Cargo
  1.100.0 (`3acc79c0c`) as well as on older verified Cargo releases (such as
  1.97.1). Verified on `cargo 1.100.0` vs `cargo 1.97.1`:
  - Cargo 1.97.1 and 1.98.1 reject `--profile debug` as a reserved name.
  - Cargo 1.99.1 and 1.100.0 accept `--profile debug` and write to
    `target/debug/`, but Cargo's native `debug` profile inherits `dev`
    (`dir-name = "debug"`): it still omits debuginfo on build scripts
    (`build-override`) unless `CARGO_PROFILE_*_BUILD_OVERRIDE_DEBUG=full` is
    set, inherits `[profile.dev]` or `CARGO_PROFILE_DEV_*` overrides unless
    `CARGO_PROFILE_DEBUG_*` overrides them, bypasses `[profile.test]` on
    `cargo test --profile debug`, and is not selected by `oxide exec --profile
    debug` tools that invoke Cargo without `--profile debug`.
  - Keeping the `dev` environment recipe (`CARGO_PROFILE_DEV_DEBUG=full` and
    `CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG=full`) preserves `target/debug/`
    placement, full debuginfo on workspace, dependency and build-script units,
    `[profile.test]` inheritance, `oxide exec --profile debug`, and uniform
    compatibility across Cargo 1.95.0–1.100.0.
- `test` inherits the recipe. `bench` keeps its release-based default. An
  explicit quick or debug `bench` passes `--profile dev`.
- External tools receive the quick recipe through the inherited `dev` settings,
  with no flag. Use `oxide nextest run` for that.
- Cargo's own `--profile debug` is not Oxide's `debug`. A tool argument such as
  `--cargo-profile debug` passes through and selects Cargo's profile. On Cargo
  1.97.1 and 1.98.1 Cargo itself rejects `--profile debug`; on Cargo 1.99.1 and
  1.100.0 Cargo's `debug` profile inherits `dev`, so under Oxide's default
  `quick` recipe it receives `CARGO_PROFILE_DEV_DEBUG=line-tables-only` unless
  `CARGO_PROFILE_DEBUG_*` or `[profile.debug]` sets otherwise.
- The deprecated `oxide-quick` and `oxide-debug` profiles remain defined in the
  environment. `--cargo-profile oxide-quick` still builds, into
  `target/oxide-quick/`. Drop the flag.
- No alternative run strategy is selected. `run` builds with the recipe and then
  executes the program. `test` keeps normal test semantics. Oxide never retries
  a failed program under another strategy.
- `run` resolves its binary as `cargo run` does: `--bin`, `--example`, then
  `default-run` or the only binary. Several candidates refuse with Cargo's error,
  before any lease.
- `run` gives the program the environment `cargo run` gives it: `CARGO` (the
  Cargo Oxide itself runs, resolved on `PATH`), `CARGO_MANIFEST_DIR`,
  `CARGO_MANIFEST_PATH`, every `CARGO_PKG_*` fact of its package (read from
  Cargo's own metadata, so workspace inheritance is resolved), and the loader's
  search path — the program's directory, its `deps`, the toolchain's
  `target-libdir` and `sysroot/lib`, then the caller's own
  `DYLD_FALLBACK_LIBRARY_PATH` or `LD_LIBRARY_PATH`, or the loader's default.
  Each post-build metadata and toolchain query has a separate ten-second
  limit with bounded stop and cleanup. If package facts cannot be read, Oxide
  warns and runs the program with the other environment fields it could set.
  If the toolchain query fails, the search path omits its library directories.
- `run` needs Cargo's JSON to find the program, so `--message-format` takes a
  json variant (`json`, `json-render-diagnostics`, `json-diagnostic-short`,
  `json-diagnostic-rendered-ansi`): the caller's variant is forwarded as given
  and its messages are printed, and a human format refuses by name. With no
  caller format Oxide asks for `json-render-diagnostics` and prints nothing.

## Slots and admission

### Identity and compatibility key

- **Repository identity:** the Git root commit(s) plus the workspace path inside
  the repository. Worktrees, clones and forks of one repository match. A shallow
  clone without its root commit falls back to the normalised `origin` URL.
  Outside Git, the canonical workspace root is used, with no cross-checkout reuse.
- **Slot key:** repository identity, the recipe key (`quick:dev-recipe-2`,
  `debug:dev-recipe-2` or `release`), the resolved toolchain (`rustc -vV` of
  `RUSTC` or `rustc`, run in the command's working directory), the target triple,
  the codegen backend, and the effective incremental setting (`CARGO_INCREMENTAL`,
  then `build.incremental`, then the profile). A checkout that writes outputs into
  a root also keys its destination. A different key never shares intermediates.
  Oxide does not select or pin a toolchain. `rustup` does.

### Slots and layout

- New slots are single-level Barge roots at
  `slots/<label>-<profile>-<16-hex>`. The label is the sanitised last segment
  of `origin`, else the repository directory name; the profile is the
  command's profile; the 16-hex id is Barge's root id. Legacy `slots/<n>` and
  nested `slots/<folder>/<profile>-<n>` directories are never renamed or moved.
  Cargo records absolute paths in them. A ledger without a directory field
  works unchanged.
- Eviction removes only the slot directory. Project folders are left in place.
- A slot holds Cargo's build directory, the child temporary directory (see
  [temporary files](#child-temporary-files)) and the slot's
  compile origin record.

### Choosing a slot and waiting

1. This checkout's last slot, if idle and compatible.
2. Another idle compatible slot, after the [freshness gate](#cross-checkout-freshness)
   reconciles it.
3. A new slot, within the caps and the soft budget.
4. Otherwise wait in FIFO order among unblocked commands (commands blocked on a
   per-checkout or per-slot wait step aside so unrelated checkouts do not
   head-of-line block), or evict an idle slot of the project.

- **Second-slot rule:** a busy slot of the same key in the same checkout — or a
  ledger-idle slot whose Barge root hold is still held by another process (for
  example, a lingering child that kept the inherited descriptor) — is never
  reused, released, evicted or relabelled. The command takes another idle
  compatible slot or a new one. Two slots of one checkout share
  `<workspace>/target`, and the latest final artifact wins.
- **Caps:** a project holds at most 12 slots (`PROJECT_SLOT_CAP`). At the cap, an
  idle unheld slot of that project is evicted whole. The pool holds at most 256
  slots (`MAX_SLOTS`).
- **Host-wide active builds:** `OXIDE_MAX_ACTIVE`, or `[pool] max_active` in the
  config file, sets an integer from 1 to 32. Without either, macOS uses
  `min(4, max(2, floor(physical_cores / 4)))` from `hw.physicalcpu`. Other
  platforms, and any failed or implausible query, use 2. The count covers
  commands in both the internal and the removable pool. Lowering the limit leaves
  running commands alone.
- **Waiting:** unblocked waiters competing for the host-wide active-build limit
  wait in strict FIFO order. Waiters blocked only on a per-checkout or per-slot
  condition (a checkout clean barrier, project cap, uncertain interrupted slot,
  Barge-held slot, or capacity with a busy checkout slot) keep their ticket
  without head-of-line blocking unrelated checkouts behind them when active
  builds are below the host limit. A waiting command prints `oxide: waiting for …`
  (naming the Barge-held slot when waiting on another process's Barge hold). A
  changed reason is printed up to eight times, then one notice says later changes
  are omitted. Ctrl-C cancels with exit 130. Do not kill a waiting command or
  fall back to `cargo`. Wait totals use a monotonic clock and are attributed to
  the last exclusive reason. They are approximate observations.
- A command that waited prints `oxide: waited …; slot ready` after its final
  freshness check, before Cargo or the tool starts.

### Time budgets

Leased commands (`check`, `build`, `test`, `clippy`, `run`, `doc`, `bench`, the
other building built-ins, `exec` and external subcommands) accept opt-in budgets:

```bash
oxide test --execution-budget 90s
oxide check --admission-budget 45s --execution-budget 300s
oxide run --execution-budget 120s
oxide exec --execution-budget 300s -- npm run tauri build
```

- `--admission-budget` covers everything before the child starts: preparation,
  slot waits and the freshness gate. Expiry starts nothing and exits 124.
- `--execution-budget` starts when the child is observed to start. It covers
  compilation, Cargo locks, tests, output drain and closeout. Expiry stops the
  owned process group (TERM, a grace period, then KILL) and exits 125. A child's
  own 124 or 125 is still that child's exit.
- A child that starts after the admission deadline is reported as late, stopped
  at once, and exits 124.
- A bounded `run` keeps its lease through the program, under one cumulative
  window.
- `--budget-events <fd>` writes newline JSON records to a non-blocking pipe. A
  lost stream makes the run unknown, never a typed success.
- A nested `oxide` consumes the outer window and inherits no deadline of its own.
  Budgets do not cover kernel-blocking operations, a `setsid` escapee, or sleep
  time on macOS.
- Accepted durations are whole milliseconds or seconds (`30s`, `1500ms`), at
  most seven days. Invalid or duplicate options refuse before any effect.

## What one command does (local executor)

1. Resolve the workspace, identity, profile, recipe and toolchain. Help requests,
   `clean`, `init`, `new` and passthrough classes take no lease.
2. Lease a slot as above, printing wait reasons as they change.
3. Run the freshness gate when the slot's compile origin differs from this
   checkout.
4. Run Cargo as the leader of a new process group, with the build directory on
   the slot, the target directory at `<checkout>/target`, the recipe variables,
   `RUSTC_WRAPPER=""` (see [inherited wrapper](#inherited-rustc-wrapper)),
   and the temporary-directory choice.
5. `run` releases the lease after the build and executes the binary from the
   checkout's target directory, unless a budget or converted output custody keeps
   the lease through the program. `test` holds the lease through execution. Reuse
   never skips tests.
6. When the group is empty, record the compile origin, run bounded stale-unit
   pruning on warm layout-v2 slots after a clean build while still holding the
   slot's exclusive Barge hold, release the lease and print
   one summary line: profile, strategy, slot, warm or cold, elapsed time, the
   pool's soft budget and backend. `OXIDE_TRACE_RELEASE=1` adds phase lines on
   stderr for closeout diagnosis.

## Cross-checkout freshness

Cargo 1.97.1 and 1.100.0 fingerprint local units by relative path and file mtime. It treats
an input whose `path_mtime <= reference_mtime` as fresh, and it embeds the
checkout path in some dep-info. Without a gate, a slot reused from another
checkout can keep stale units. The gate runs before Cargo whenever a slot's
recorded origin differs from the current checkout:

1. After each leased command whose Cargo ran, the slot records its compile
   origin: the canonical checkout path, the `HEAD` tree, the Git blob digest of
   each dirty or untracked non-ignored file, the state of each declared submodule,
   and a build anchor (the time after Cargo's group is empty).
2. Oxide diffs the two states with Git. Each differing file whose mtime is not
   newer than the anchor gets its mtime set to now, so Cargo rebuilds it. The
   gate never changes file content, Cargo's fingerprints or the slot itself.
3. If the checkout path changed, every local package under the checkout is
   rebuilt. Oxide asks `cargo metadata` for the local packages, in the command's
   own context, with `--offline --locked` added so metadata never downloads or
   writes the lock. Registry, Git and outside-checkout path dependencies stay warm.
   Any metadata failure, oversize output, a local manifest Git cannot see, or two
   packages in one directory refuses sharing, and the command builds cold.
4. A built-in build command may get a missing `Cargo.lock` generated offline by
   `cargo generate-lockfile`, with its own arguments. A stale lock, an uncertain
   offline answer, or a help request refuses the slot. Cargo's own `--locked` and
   `--frozen` refusals stand, with Cargo's message and exit code.
5. The slot keeps an uncertain set of paths that differed or changed during a run,
   so the next checkout refreshes them too. Past 4,096 paths the slot stops being
   shared.
6. Declared submodules (at most 32, one level deep) are checked in both the index
   and `HEAD`. Two worktree states are recognised: uninitialised, and exact (the
   child's `HEAD` is the declared commit, its tracked files are clean, and its git
   directory lies in this repository's module store). Anything else refuses. The
   gate never runs `git submodule`, never fetches, and never writes a submodule's
   config, index or content.
7. Bounds per snapshot: 64 Git invocations, 60 s of waiting, 16 MiB of stdout and
   200,000 listed files. Each Git call runs with `submodule.recurse=0` and
   `core.fsmonitor=false`. Any bound refuses the slot without a partial fingerprint.

Trust: an origin is trusted only while the slot's use counter still equals the
value stamped when it was written. A command that dies after Cargo starts, a
failed snapshot, a crash between re-stamp and state change, or a legacy slot makes
the slot untrusted. The next lease then releases and evicts it whole and builds
cold. A checkout without Git content records an affine build, which only that
checkout reuses.

Limits: inputs outside Git or ignored by it are not checked. The summary says so
when another checkout's build was reconciled. Reused workspace units carry the
compiling checkout's absolute paths in debug info. That is tolerable for `quick`,
but wrong for `debug` sessions after that checkout is gone. Local workspace units
do not reuse across checkout paths. The cross-checkout benefit is dependency
reuse.

### Cargo version matrix

The gate, stray detection and dep-info decoder depend on Cargo internals. They run
only on Cargo releases listed in `VERIFIED_CARGO` (`src/bin/oxide/freshness.rs`):
1.95.0, 1.96.1, 1.97.1, 1.98.1, 1.99.1 and 1.100.0, matched by their `cargo -V`
prefix. Oxide itself is pinned to 1.100.0 in `rust-toolchain.toml`.

| Cargo | Default build-dir layout | Stray detection reads | Cold-slot seeding |
| --- | --- | --- | --- |
| 1.95.0, 1.96.1, 1.97.1, 1.98.1, 1.99.1 | v1: `<profile>/deps`, `.fingerprint`, `incremental` | `deps/`, `incremental/` | adapter `cargo-1.97.1/c980f4866/build-dir-v1` (1.97.1 only) |
| 1.100.0 (`3acc79c0c`) | v2: `<profile>/build/<pkg>/<meta>/{out,fingerprint,run}`, `incremental/`, `build/` | `deps/`, `incremental/` and, under a profile that holds v2 units, `build/` | adapter `cargo-1.100.0/3acc79c0c/build-dir-v2` (1.100.0 only; the private opt-out is refused) |

- Layout is read from the directories Cargo wrote, not from the version alone.
  Cargo 1.100.0's private opt-out (`__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT`)
  writes v1 there, and a v1 profile's `build/` keeps its old meaning: build-script
  output, not counted.
- The gate reads no slot dep-info or fingerprint. It relies on Cargo's
  `path_mtime <= reference_mtime` rule, and on 1.100.0 the reference is the v2
  `build/<pkg>/<meta>/fingerprint/dep-*` file. Fixture runs showed a copied file
  with an old mtime stays fresh and a refreshed one rebuilds.
- Cargo 1.100.0 facts rechecked against `3acc79c0c` source and real output: the stale-file
  rule, the encoded dep-info marker `01 00 00 00 ff` with version 1, workspace-relative
  local paths, `cargo metadata --format-version 1`, and the `--locked` refusal wording.
  Its `build.build-dir` setting is stable.

- On any other Cargo, shared reuse is off. A new slot is an affine build reused
  only by its own checkout, and a content slot needing refresh builds cold.
- `cargo -V` and `rustc -vV` run in the command's working directory, so a
  member's `rust-toolchain.toml` counts.
- Not in the slot key, and so unsupported for compiler separation: `build.rustc`
  from config or `--config`, `CARGO_BUILD_RUSTC`, `RUSTC_WORKSPACE_WRAPPER`, a kept
  inherited rustc wrapper, and a `PATH` whose `rustc` belongs to another install
  than its `cargo`.
- A new Cargo release joins the table only after its source facts are rechecked.
- `oxide clean --cargo-cache` (`QUALIFIED_CARGO` in `src/bin/oxide/cargo_home.rs`)
  is qualified on Cargo 1.97.1 (`c980f4866`) and Cargo 1.100.0 (`3acc79c0c`); see
  [Cargo home hygiene](#cargo-home-hygiene).

### Stale-unit pruning (layout v2)

In Cargo layout v2, each compile or build-script unit lives in its own
`<profile>/build/<pkg>/<16-hex>/` directory (`fingerprint/`, `out/`, optional
`run/`). Before releasing a warm slot after a clean build (exit 0, no signal,
watchdog or budget stop, and positive group closure), Oxide prunes stale unused
unit directories while still holding the slot's exclusive Barge hold and before
taking the pool or admission lock for release:

- **Filesystem observation:** before Cargo starts on a warm slot, Oxide checks
  that the slot's mount does not set `noatime` and runs a single-file
  `futimens` + `pread` probe to confirm that reading a file with `atime < mtime`
  updates `atime`. Where confirmed, primary `fingerprint/` files with
  `atime >= mtime` are armed with `atime = mtime - 1 s` (`UTIME_OMIT` for
  `mtime`, never altering `mtime`). If `atime` is unavailable or unconfirmed,
  pruning is skipped.
- **Keep rule:** any unit whose `fingerprint/` or subtree timestamps show it
  was written or read during the build (`mtime` or `atime` at or after the
  pre-Cargo start minus 1 s) is never deleted, and its recorded activity in
  `<pool>/oxide-managed/units/<slot_id>.json` advances to now. Units used within
  the 7-day staleness window (`STALE_UNIT_MAX_AGE`) are also kept, and cold
  slots never delete anything on their initial build.
- **Conservative shape & boundary checks:** layout v1 members (holding `deps/`
  or `.fingerprint/`, or lacking v2 units), `<16-hex>` directories missing
  `fingerprint/` or holding entries outside `{fingerprint, out, run}`, symlinks,
  foreign-owned entries and cross-mount entries are left completely untouched.
  A stale unit is unlinked bottom-up descriptor-relative (`openat` with
  `O_NOFOLLOW`, `fstatat` with `AT_SYMLINK_NOFOLLOW`, `unlinkat`) only after its
  entire subtree passes verification; an empty `<pkg>` parent directory is
  removed only if `unlinkat(..., AT_REMOVEDIR)` succeeds.
- **Bounds:** pre-Cargo arming is capped at 200 ms; post-build pruning is capped
  at 500 ms (both clamped to any remaining command budget), 16 profile members,
  4,096 packages per profile, 16,384 units, 50,000 walked entries, depth 32, and
  at most 256 deleted unit directories per release. Any bound reached stops the
  pass cleanly without stalling slot release.

## Crash recovery

Oxide manages crash recovery for its own commands. Other lease users keep their
explicit recovery.

- While a command holds a lease, it holds an exclusive `flock` on its control
  record's lock file. The kernel drops that lock when the process dies. The record
  names the holder (PID, process start time and holder string), the lease ID, the
  phase and Cargo's process-group ID.
- Every wait on the pool lock or an admission-class lock is bounded (oxide#325).
  It ends at the command's admission window when one is set, otherwise after
  120 s, and refuses with a message naming what it waited on: the slot recorded
  as being deleted, or an unknown holder. It prints one progress line after
  5 s. No such wait is unbounded.
- Ctrl-C and termination signals (INT, TERM, HUP, QUIT) are forwarded to the
  group, and the group is continued after them so a stopped member acts on the
  signal. Oxide releases only after the group is empty. A clean exit or Ctrl-C is
  never logged as a recovery.
- A group that does not own the terminal's foreground reads `/dev/null` instead
  of a terminal standard input, so a background read cannot stop the job. A
  program Oxide runs in the foreground owns it, and a job-control stop (Ctrl-Z)
  hands the terminal back, stops Oxide with the job, and on resume gives the
  terminal and SIGCONT back to the group.
- A signal received while Cargo runs ends the command with 128 plus the signal:
  a `run` never launches its program after it.
- A later command recovers a dead lease automatically only when all of these hold:
  the record lock is free; the lease is the record's own; and either Cargo never
  started or `killpg(pgid, 0)` returns `ESRCH`. On macOS, `EPERM` means a group
  of only unreaped zombies, so the slot stays held. No PID age, TTL or start-time
  comparison is used.
- A surviving group member, a reused group ID or any other error keeps the slot
  held. A follower takes another compatible slot or waits. It never touches the
  held one.
- If Cargo had started, the recovered slot has no trusted origin. The next lease
  evicts it and builds cold.
- A command that dies after "Cargo starting" but before recording its group cannot
  prove the group empty. The slot stays held and `oxide status` marks it
  interrupted. `oxide pool recover-interrupted <slot>` releases it explicitly. The
  command refuses while a live command holds the record.
- Each automatic recovery is logged in the control state. `oxide status` shows the
  newest 16.
- Accepted residual risk: a descendant that leaves the group with `setsid` is
  invisible to `killpg`. If it still writes in the slot, the slot can be reused
  under it.

## Cold-slot seeding

Seeding is on by default. Only the exact value `OXIDE_COLD_SEED=0` opts out.

- A fresh slot of a supported command is warmed once, before any child starts.
  Matching plain registry library compile units are cloned from an idle donor
  slot on the same pool. The clone runs under a shared donor hold, the recipient's
  exclusive hold and the donor's record lock.
- Each cloned artifact, `.d` file and fingerprint is re-hashed against a
  generation-bound catalog. The recipient publishes only after a complete,
  ownership-clean post-clone walk. The donor is never mutated.
- Only plain registry library units are offered. Not offered: non-leaf packages,
  proc macros, build-script units, dependencies that read `OUT_DIR` or lie outside
  the root, Git and path sources, unprovable dep-info, a differing producer
  context and run-only fingerprints.
- A failure before the first recipient write falls back to a normal cold build. A
  later clone, digest or custody fault refuses. It releases the slot with no
  trusted origin and leaves the untrusted generation to whole-root eviction.
- The catalog lives in `oxide-managed/seeds/`. Both internal and removable pools
  seed, and the reserve gate applies to the start.

## Inherited rustc wrapper

**Rustc wrapper.** Leased builds bypass an inherited `RUSTC_WRAPPER`, such as a
machine-wide `sccache`. Every Cargo call Oxide makes for a leased command gets
`RUSTC_WRAPPER=""`. That override beats config files and `--config`, and child
Cargo calls inherit it. Plain `cargo` and other tools keep the wrapper. Oxide
never changes Cargo config, the sccache server or its cache. `RUSTC_WORKSPACE_WRAPPER`
and the clippy driver are untouched. `OXIDE_ALLOW_RUSTC_WRAPPER=1` keeps the
inherited wrapper, but that path is unsupported for interruption semantics. The
summary names the bypassed wrapper once. `oxide status` reports the wrapper it
found and its effective state.

## Child temporary files

Before any writer starts, a leased command decides
once for its whole child tree:

- An inherited `TMPDIR` is kept, as its canonical path, when it is an existing
  directory this user can write on the same device as the selected pool's payload.
  Oxide creates nothing in it and never cleans it.
- Otherwise the command gets one Oxide-owned `0700` directory, `<slot>/scratch`,
  removed at release. Its marker is bound to the slot's incarnation. A later lease
  of the same slot reconciles it. Any other leftover is kept and reported.

`TMPDIR`, `TMP` and `TEMP` are set to the chosen value for children only. A path
over 91 bytes is refused before spawning, because Unix-domain sockets need short
paths. The summary says `temp: inherited <path>` or `temp: oxide <path>`.

## Storage and space management

- **Backend:** `directory-budget-v1` is the only internal backend. Its default path
  is `~/Library/Caches/oxide/build-pool` on macOS and
  `$XDG_CACHE_HOME/oxide/build-pool` (or `~/.cache/oxide/build-pool`) elsewhere.
  The control root (ledger, leases, state, registrations) lives inside the pool.
- **Soft budget:** 100 GB (decimal) by default. The effective value comes from
  `OXIDE_POOL_CAP`, then `[pool] budget`, then the default. A removable pool uses
  the budget recorded at its registration. Admission evicts whole idle slots until
  the pool is under budget. If only held slots remain, admission reports `Full`.
  The budget is not a quota. A running build may pass it.
- **Accounting:** each walk is bounded. An incomplete walk refuses with
  `unknown_accounting` and names its cause. Its bytes never make a slot evictable,
  and reuse needs the slot's own walk to be complete.
- **Host reserve:** no pool is created below 20 GiB of free space. A supervised
  build samples the pool filesystem about once a second. At 22 GiB free (the 20 GiB
  reserve plus a 2 GiB margin) it stops the owned process group. The stop is an
  ordinary failed command: TERM, a grace period, then KILL, applied only to the
  owned group. The lease is released only after positive closure. The margin is a
  best-effort bound, not a guarantee against faster writers.
- **Barge:** Barge owns roots, bytes, holds, pins, pressure pruning and
  marker-checked trash. Eviction is whole-root, under Barge's exclusive lock with a
  pin recheck before rename. Trash never follows symlinks or crosses filesystems.
  There are no per-entry journals. Unknown legacy roots are retained.
- **Retired:** `apfs-sparse-image-v1` pools refuse with a retirement message and are
  never locked, attached, written or removed. `status` reports them as legacy.
  `OXIDE_POOL_BACKEND` accepts only unset or `directory-budget`. `linux-image-v1`
  is not implemented.
- **Config file:** `config.toml` in the platform config root
  (`~/Library/Application Support/oxide/config/config.toml` on macOS,
  `$XDG_CONFIG_HOME/oxide/config.toml` elsewhere). `OXIDE_CONFIG` selects another
  absolute path. It holds machine settings only, at most 64 KiB, and unknown keys
  refuse:

  ```toml
  [pool]
  path = "~/Dev/oxide"   # absolute, no . or .. components
  budget = "100GB"       # or a byte count
  max_active = 3         # integer from 1 to 32
  ```

- **Environment:** `OXIDE_POOL` selects the pool path. `OXIDE_POOL_CREATE=1`
  allows creation at an explicit path (or one inside `OXIDE_TASK_SCRATCH_ROOT`).
  `OXIDE_POOL_CAP` sets the budget. `OXIDE_MAX_ACTIVE` sets the host limit. The
  precedence is environment, then config file, then default. Sizes are whole bytes
  or whole decimal or binary units from 1 MiB to 16 TiB.
- **Path refusals:** a `.` or `..` component; a path inside a Git work tree; a
  path under `/Volumes` unless it is a registered removable volume; an unknown
  identity. An absent pool named by `OXIDE_POOL` refuses rather than being created
  silently.
- **Account home:** home-derived paths use the account's passwd home, not `$HOME`.
  A test harness that overrides `HOME` still reaches the machine's pool. When the
  two differ and the default pool is used, a notice says so. `status` prints a
  `home:` line.
- **Time Machine:** the cheap `tmutil isexcluded` check runs first. A pool inside
  an already excluded parent (a scratch volume, for example) is excluded through
  inheritance, so `tmutil addexclusion` — about eleven seconds on this host — is
  skipped. Only a genuinely included root pays for it. `tmutil isexcluded` must
  then confirm the result before a build writes; failure or an unknown result
  refuses before Cargo runs. Spotlight is not changed, and `status` says so.
- **Recorded pools:** the state directory records the newest 16 pools. `status`
  lists pools at other known paths as orphaned. Nothing reclaims them.
- **Explicit release:** `oxide pool recover-interrupted <slot>` releases an
  interrupted slot. `oxide pool release-quarantined <slot> --confirm-writers-stopped`
  refuses legacy quarantined generations because they lack cooperative Barge
  hold history; confirmation and `--dry-run` do not change that refusal.

## Removable build volume

- **Registration:** `oxide pool register-removable <mount> --reserve <size> --cap <size> [--mode ephemeral|permanent] [--payload-dir <rel>] [--outputs-dir <rel>]`.
  The default mode is `ephemeral`. `oxide pool set-mode <ephemeral|permanent>` may
  run while the drive is absent. The registration file is Oxide's user policy
  (mount, UUID, mode, reserve, budget, payload and outputs directories). It is
  imported into the pool's Barge account. A different UUID or path refuses.
- **Layout:** the payload directory (`payload_dir`, default `oxide-pool`) and the
  outputs directory (`outputs_dir`, default `oxide-outputs`) are clean relative
  paths under the volume root, up to 8 names deep, which may be nested such as
  `builds/oxide/pool` and `builds/oxide/outputs`. They must differ and neither may
  contain the other. Each component is walked through the verified volume
  descriptor with no-follow and same-device checks. Registration version 3 records
  both fields. Versions 1 and 2 read with the defaults and keep working unchanged;
  the next write publishes version 3. Existing layouts can move with
  `oxide pool relocate --payload-dir <rel> --outputs-dir <rel>`.
- **Relocation:** Oxide takes internal then removable admission and holds the
  removable ledger lock through publication. Held builds, waiter tickets,
  cleans, Barge holds, unknown writer custody and unfinished holding entries
  refuse. Both destinations must be absent and disjoint from the source paths.
  Oxide moves the two areas through Barge, updates the path-keyed outputs
  account, registry, receipts and still-owned member links, clears removable
  compile origins and publishes the new config and registration. A failed move
  rolls back completed steps. If interrupted or rollback fails,
  `pool-relocation.json` records both layouts and fences registration readers
  until the paths and records have been reconciled.
- **Legacy upgrade:** `oxide pool upgrade-legacy [--dry-run] [--json]` prepares
  a previously used registered volume for relocation. It verifies the bound
  volume and takes both existing admission locks and the removable ledger lock.
  It plans all changes before writing and refuses unknown paths, live nested
  slots, held roots or other active writers. The output registry must account
  for each unmarked output. A live checkout keeps its output at the same path
  with a Barge identity and pin protection for any selection. Only roots whose
  recorded checkouts are all missing are removed, with an atomic registry
  update. A dead nested slot is removed before its store and ledger records
  are retired. Empty legacy project declarations and recognized markerless
  slot trash are removed under the same quiet window. The preview reports the
  same counts and paths without modifying files.
- **Selection:** when a build starts, the drive is selected only if it is mounted,
  its registered UUID matches and it is writable. Selection does not repeat the
  boot-container or APFS-role checks; registration and `describe` run those, and
  the registered UUID pins the volume. Positive absence falls back to the internal
  pool with a warning. An unknown or different identity refuses. Free space below
  the reserve starts Barge's reserve pruning and never refuses selection; the floor
  refuses admission only when that space cannot be recovered. A running command
  never switches volumes.
- **Registration failure:** paths whose creation by the failed attempt cannot be
  established are retained and reported for inspection before another attempt.
- **Modes:** both modes use the same checks. `ephemeral` keeps final outputs local.
  `permanent` places them on the drive (see [final outputs](#final-outputs)). A
  permanent drive that cannot be used produces a warning before admission, with the
  reason.
- **Surprise removal:** the watchdog stops the running command on removal, and the
  command reports the lost volume. A slot whose writer closure is unknown stays
  held or quarantined. `recover-interrupted` can release an interrupted slot;
  legacy quarantined generations remain retained. A removable slot is flushed before it goes idle. A failed flush retires the
  slot.
- **Previous registrations:** a registration under another pool's control root is
  never moved, adopted or removed. `register-removable` refuses while one exists
  or while the payload directory is not empty.
- **Describe:** `oxide pool describe --json` (see [status](#status-and-inspection))
  reports the registration without writing anything, including the `payload` and
  `outputs` paths on the volume.

## Final outputs

Final outputs are the binaries and artifacts Cargo writes under `target/`. In
`ephemeral` mode they stay in `<workspace>/target`. In `permanent` mode they move
to the registered drive, and Oxide owns the links that point there.

- **Layouts.** The current (profile) layout keeps `<checkout>/target` as a real
  directory. Oxide-owned links sit at its profile members (`debug`, `release`,
  `<triple>/debug`, `<triple>/release`), and each member points at its own subpath
  of one owned root. Cargo's top-level files, `target/tmp`, `target/doc` and
  other tool directories stay real. The older whole-target layout makes
  `<checkout>/target` itself a link; checkouts switch to the profile layout via
  `oxide outputs detach` followed by `oxide outputs adopt`. A member link points
  at its approved subpath only.
- **Roots and marker.** A root is `<mount>/<outputs_dir>/<project>-<checkout id>/`
  on the registered volume (`outputs_dir` is `oxide-outputs` unless the
  registration names a nested directory such as `builds/oxide/outputs`), or the local fallback
  `<checkout>/.oxide-outputs/local/`. Each root carries `.oxide-output.json` (the
  checkout, its id, the project label, the kind, the volume UUID and the creation
  generation). A link is owned only when it points at a real directory with this
  checkout's marker. A foreign link is never followed, adopted, swapped or cleaned
  through.
- **Receipt.** `<internal pool>/oxide-managed/outputs/<checkout id>.json` is a
  strict, bounded document. It holds the committed selection and generation, a
  bounded history of at most eight roots, or one prepared transition. An unknown
  version or field refuses.
- **Git prerequisite.** Before the first link is published, Git must positively
  ignore each member path (`target/debug`, `target/release`, and so on). A rule
  such as `/target`, `target` or `target/` covers them. A tracked or negated path
  is not covered. Explicit adoption refuses, naming the fix. Automatic conversion
  instead leaves the checkout unconverted, with one notice. Oxide never rewrites
  `.gitignore`.
- **Automatic first conversion.** A leased build converts a fresh checkout only
  when all of these hold: the selected pool is the permanent drive, the drive is
  usable now, the checkout has no receipt of any state, and `<checkout>/target` is
  absent (a dangling link counts as present). The conversion creates the profile
  layout under the barrier and the admission lock, moves no existing output, and
  prints `oxide: auto-adopted <target> -> <root> …` once.
- **Explicit commands.** `oxide outputs status` and `oxide outputs plan` are
  read-only. `plan` prints bounded JSON (`oxide.outputs.plan/2`) and writes nothing.
  `oxide outputs adopt` converts a missing or empty real target (or a detached
  checkout). `oxide outputs detach` restores a real empty
  `target` and keeps every root and its bytes, and handles legacy whole-target
  checkouts so they can switch via `detach` then `adopt`. It is the per-checkout opt-out in
  either mode. Creating a real empty `target` before the first build does the same
  thing. `oxide outputs cancel-reservation` drops this checkout's unused reserved
  nonce under the same barrier and binding lock, so the next build reserves afresh.
- **Transitions.** A relink is write-ahead. It records `prepared`, creates the
  destination root and marker, creates a sibling link, renames it onto `target`,
  and then records `committed`. It waits for other commands in the checkout to stop
  running Cargo. After a crash, the next permanent-mode leased command replays the
  prepared transition under the checkout's binding mutex and the cross-pool custody
  rule. If nothing was published, it rolls back to the committed state. If the
  sibling is live, or the rename finished, it commits after the full root checks.
  A foreign or changed path, or unknown custody, stops with everything kept.
  `oxide outputs status` shows a pending transition as state.
- **Downgrade fence.** Before the first link, `adopt` writes the forward-only
  `owned_outputs` fence into both pools' state. Older Oxide builds refuse the pool.
  The fence is never cleared.
- **Deleted target.** A committed profile checkout whose `target` was deleted gets
  that real directory recreated, and its member links republished into the same
  root. Only the receipt's generation advances. A file or link named `target`
  refuses.
- **Slot keys.** The destination is part of the slot key. A linked member and a
  real member never share a slot. Relinking to the same root keeps the key.
- **Limits per command.** At most eight members per command and 32 per receipt. A
  non-empty real member directory is used locally, with a notice. An empty real
  member converts only under an explicit `adopt`.
- **Custody.** Create, relink, detach and clean each wait for all
  checkout writers, with positive closure across pools. Unknown writer custody
  refuses, and nothing is downgraded to "clear". A converted checkout's `run` keeps
  its lease while the program runs from its path in the real root. The removal
  watchdog covers that phase too.
- **Removal.** If the volume vanishes during a build, the build fails through the
  removal path. Links are never swapped mid-command, and neither root is copied or
  deleted. The next command transitions once the checkout is quiet. An unknown or
  contradictory state refuses.
- **Pruning.** Output roots are Barge areas. Selected roots hold durable pins before
  their links are published. An old pin is released only after the new receipt is
  durable. `oxide outputs prune --dry-run` previews the candidates and prints a
  SHA-256. `oxide outputs prune --plan <file> --plan-sha256 <hex>` applies that
  exact plan, and Barge rechecks holds and pins before moving a root to
  marker-checked trash. Unknown legacy roots are retained. A historical registry
  with non-empty `intents` or `cancelled_overflow` refuses safely instead of
  mutating roots. A root whose trash copy could not be
  deleted is settled in the registry, frees no space, and makes `--plan` exit 1
  with a count of roots not freed. So does a root Barge keeps.

## Clean

`oxide clean` removes the checkout's private outputs and slot affinity. It never
deletes a shared slot. Plain `cargo clean` must not be used: with Oxide's storage
environment it would delete the shared build directory.

- **Whole-target clean** (no `-p`, `--workspace`, `--doc`, `--profile` or
  `--release`) is planned and performed by Oxide, not by Cargo. It waits for active
  commands in the checkout and blocks new ones while it runs. Afterwards it drops
  this checkout's slot affinity. Shared slots stay intact until eviction.
- **Plan:** one enumeration, parents first. Symlinks are listed, never followed.
  Another filesystem under the target, an unreadable directory, more than
  1,000,000 entries or depth 256 refuse rather than truncate. A symlinked target
  refuses. So does a pool located inside the target, checked by path, real path and
  device/inode.
- **Ownership:** each top-level entry must match Cargo's layout: the cache tag,
  `.rustc_info.json`, `doc`, `cargo-timings`, `tmp`, `package`, `.metabuild`, and
  the rustdoc and future-incompat report files. A profile directory qualifies only
  when it holds `.cargo-lock` or `.cargo-build-lock`. A target-triple directory
  qualifies only when it holds such profile directories. Anything else is foreign.
  Foreign entries refuse, naming the first 20, unless `--include-foreign` is given.
  That flag authorises only enumerated entries, never a symlink's referent.
- `--dry-run` prints every planned path and changes nothing. A real clean plans
  again under the barrier and removes exactly the planned paths, children first. A
  path that changed type or gained unplanned content is left in place. The command
  then fails and keeps the affinity.
- Finder metadata is not tolerated. Content that appears after the plan is left and
  counted.
- **Selected clean** (`-p`, `--workspace`, `--doc`, `--profile`, `--release`) uses
  Cargo's own clean on the private target. `--dry-run` and `--include-foreign` refuse
  with a selection, because Cargo's selected dry run does not list every path.
- **Profile layout:** a whole clean keeps `target` itself, every owned member link
  and each real ancestor that holds one. It removes the payload around them. A
  retained triple directory is held to the same foreign-entry policy, and each
  foreign name is reported relative to `target`.
- A clean refuses while a relink is pending or writer custody is unknown.
  `--include-foreign` cannot override that.

## Cargo home hygiene

The Cargo home is `CARGO_HOME`, or `$HOME/.cargo` when that is unset or empty.
Oxide reports it and offers one explicit cleanup request. It never deletes there
itself.

- `oxide status` prints the path and says it is not measured. `oxide status
  --measure` adds an allocation figure from one bounded walk that counts each inode
  once. Symlinks are not followed and mounts are not crossed. The walk shares the
  60-second measurement budget.
- `oxide clean --cargo-cache` takes no other argument. It runs one pass of Cargo's
  own automatic cleanup, with `cache.auto-clean-frequency="always"`, and with
  network refused. Oxide's loopback listener closes every connection, and the
  libcurl proxy variables are removed. It runs under Cargo 1.97.1 (`c980f4866`)
  and Cargo 1.100.0 (`3acc79c0c`), so it now runs inside Oxide's own pinned
  1.100.0 checkout as well, and continues to refuse unqualified Cargo releases.
  It refuses before Cargo runs when the environment or
  config could disable, redirect or fetch:
  - environment that disables cleanup, uses the Git CLI, or redirects registries,
    sources or unstable features;
  - config keys outside the contained set: sources, registries, credential
    providers, `include`, `patch`, `paths`, unstable settings, program wrappers and
    unknown keys. Unreadable or unparseable config refuses too;
  - a missing or oversized `Cargo.lock`, or a lock with any source other than
    crates.io;
  - an offline `cargo metadata --locked` that fails, or a workspace not covered by
    the Oxide guidance block.
- Outcome: `ran`, `skipped` (Cargo's own reason, such as the frequency gate or a
  held cache lock) or `failed`. No freed-space claim is made.

## Launch check

`oxide status --launch-check` (macOS) measures how much slower a newly written
executable launches than a re-run of the same file. It copies the running `oxide`
into fresh owner-only files, times their first launches and compares the medians.
It runs under a 3-second monotonic limit and kills its own helper group on
expiry. It changes no setting, runs no Cargo, creates no pool and never disables
Gatekeeper. Above 30 ms it advises adding the launching app under System Settings,
Privacy & Security, Developer Tools. It states that the cause is inferred.

## Status and inspection

- `oxide status` is read-only. It takes no pool lock, reconciles nothing and walks
  no payload. It reports recorded facts: pool and slot membership, each slot's
  recorded directory and holder, waiting commands (noting waiters blocked only on
  a checkout or slot), clean marks, the selected pool and fallback reason, the
  configured mode, the latest use and each slot's last recorded size with its
  age. A slot marked for deletion shows as pending reconciliation, not as idle,
  and a ledger-idle slot whose Barge hold is still held by another process is
  reported as Barge-held (not reusable or evictable) when observable.
- `oxide status --measure` runs the bounded walks (the whole payload, each slot,
  the checkout's stray-output scan and the Cargo home) under one 60-second budget.
  It still writes nothing. A pending deletion makes coverage unknown.
- Status also reports the config file and its source, the effective host limit,
  Time Machine and Spotlight state, legacy and orphaned pools, the newest 16
  automatic recoveries, and the temporary-directory choice the next command would
  make.
- `oxide pool describe --json` prints schema `oxide.pool.describe/1`. It is the
  stable, read-only view other tools use to find the registered drive. It opens no
  pool and takes no lock. Consumers branch on `available` and not on reason strings.
  Fields:
  - `removable` is `null` when no drive is registered. `mount_point`, `volume_uuid`,
    `mode`, `reserve_bytes`, `pending_reconciliation_slots` and
    `pending_reconciliation_slot_ids` describe a registered one. `payload` and
    `outputs` are the registration's payload and outputs directories under the
    mount point, `null` when no registration can be read.
  - `available` is passive eligibility. It needs the mount to hold the registered
    volume's root, the UUID to match, the volume to be writable and outside the
    boot container, the payload to exist, and the config and ledger to read
    cleanly. Free space is not part of `available`; the reserve is a prune trigger. Each `diskutil` or `plutil` call is bounded
    by 5 seconds.
  - `unavailable_reason` is `absent`, `identity_mismatch`, `volume_unusable`,
    `metadata_unknown`, `control_missing` or `payload_missing`, or `null` when
    available.
  - `suggested_scratch_root` is always outside `payload`. It is a suggestion, not a
    write grant.
  - Exit code is 0 whenever a description is produced. Before writing, a consumer
    must recheck the UUID, the mount root and that the device differs from both `/`
    and `/System/Volumes/Data`, and read free space against `reserve_bytes` only as
    a prune trigger, never as a refusal.

## Adoption: `oxide init` and `oxide new`

- `oxide init [cargo init args]` makes the directory a Cargo project if no manifest
  exists, running `cargo init` with the supplied arguments. With a manifest present,
  Cargo-only arguments refuse. `oxide new <path>` is `cargo new` followed by the same
  adoption step.
- Adoption writes or refreshes one marked block in the guidance root's `AGENTS.md`.
  The guidance root is the Git work-tree root when the project is inside Git, else
  the project itself. The block sits between the whole-line markers
  `<!-- oxide:begin -->` and `<!-- oxide:end -->`. It lists the covered Cargo
  workspaces in `<!-- oxide:workspaces: . crates/app -->`, sorted and unique.
  Re-running is idempotent. Text outside the markers is never touched. Malformed or
  duplicate markers, and uncertain Git answers, refuse with nothing written. A nested
  workspace's own `AGENTS.md` is never given a second block.
- `oxide init` adds `/.oxide-outputs/` to the workspace `.gitignore` if it is not
  already there. It does not rewrite other rules. It warns when a member link path
  is not covered by Git.
- Readiness output covers the workspace and guidance root, covered paths, the
  resolved toolchain, the pool a build would select and the configured pool (both
  not measured), and any Effigy routes that still call plain `cargo`. Effigy files
  are never edited.

## Switching an existing project

Standing rules for consumer projects, applied to every Rust project under Tom's
Dev directory:

- **Selectors.** Ordinary build, check, clippy, test, nextest and format routes call
  `oxide`. There are no per-selector `:cargo` siblings. The only escapes are
  machine-wide and explicit: Effigy's per-program fallback (`[programs.oxide]
  fallback = "cargo"` in `~/.effigy/config.toml`, installed with `force = false`, so
  it acts only when `oxide` is missing; set `force = true` while `oxide` is broken),
  `EFFIGY_OXIDE_ESCAPE=cargo`, or running `cargo` by hand. The fallback runs
  `cargo` with the same arguments. It does not cover `oxide exec` or Oxide-only flags.
- **CI.** Where CI calls the same selectors and its runners lack `oxide`, the
  selector runs `cargo` when `GITHUB_ACTIONS=true` and `oxide` otherwise. Locally, a
  missing `oxide` fails loudly.
- **Stays on Cargo:** CI and release workflows, container and VM builds without
  Oxide, product code that runs Cargo as part of its own behaviour, test fixtures,
  and scripts that set their own `CARGO_TARGET_DIR`. Oxide replaces that variable.
- **Tools that run Cargo themselves** (the Tauri CLI, packaging scripts) run under
  `oxide exec --`. A `CARGO_TARGET_DIR` set on the `oxide exec` call itself does not
  survive. Move it into the tool's own command, with `--target-dir`, or into its
  child environment.
- **Sibling binaries.** Test executables run from the slot, and final binaries are
  uplifted to `target/<profile>/`. A test that finds a sibling from `current_exe()`
  in `deps/` breaks. Use `env!("CARGO_BIN_EXE_<name>")`, or read
  `$CARGO_TARGET_DIR/<profile>`.
- **Throwaway stages.** A stage outside Git, or one whose `.gitignore` cannot
  cover the member links, is an unconverted checkout. It builds into a real local
  `target/`, prints one notice, and needs no preparation. Keep stages under the
  drive's `scratch/` or `OXIDE_TASK_SCRATCH_ROOT`.
- **Toolchain pins.** `cargo +<toolchain>` becomes `RUSTUP_TOOLCHAIN=<toolchain>
  oxide`, or a `rust-toolchain` file.

## Dogfooding in Oxide's repository

Oxide's own repository is the only adopter that installs the binary.

- **Installed binary.** `~/.local/bin/oxide` is a regular file built by
  `scripts/oxide-install.py` from a pinned commit on pushed `origin/main`. The pin
  is `--pin <commit>`, or `OXIDE_INSTALL_PIN`, or the tip of `origin/main`. The
  installer authenticates the pin with `git ls-remote` and a private ref. It builds
  from `git archive` in a private directory with
  `cargo build --release --locked --offline --bin oxide`, stamped with the pin and
  a fresh instance. Installs use a sibling temporary file and a no-clobber `link`.
  Every directory on the path must be owned by this user, and none may be a link or
  writable by others below `HOME`.
- **Receipts.** Each install writes an owner-only JSON receipt (version 2) under
  `~/.local/state/oxide/install/`. It records the pin, tree, toolchain, build
  command, path, size, time, SHA-256, build instance, and the build's
  `control_state_version` and `registration_version`, read by an effect-free probe.
  A build that cannot report them is refused before anything is installed.
- **Initial install** (`effigy oxide:install`) refuses if any `oxide` is on `PATH`
  or the destination exists.
- **Reinstall** (`effigy oxide:reinstall`, the `--replace` mode) replaces only a
  regular, user-owned binary whose hash matches the newest receipt. It saves that
  binary beside its receipt as `<receipt>.binary`, mode `0700`, verified by hash.
  Swaps are serialised under the install lock. The destination is rechecked
  immediately before the atomic rename, and the old inode is kept until the new
  receipt is published.
- **Canary** (`effigy oxide:canary -- --pin <commit>`) runs before a storage-affecting
  pin is installed, after reviewed merge. It stages the same build, runs `status` and
  `pool describe --json`, then in a fresh dependency-free crate on the selected
  pool's scratch directory runs two offline `oxide check`s. The first must take a
  new slot within 10 seconds. The second must reuse that slot within 5 seconds.
  Override the limits with `--new-seconds` and `--warm-seconds`. The reported
  admission ends when the child starts: a ceiling eviction's Barge trash
  deletion runs beside the writer, with the pool and admission locks released,
  so it is not counted in this figure (oxide#324). Each check has a 300-second
  admission budget and a 60-second execution budget. The receipt records
  FAIL before the attempt, and PASS only after both checks and cleanup. The crate is
  removed only after writer closure is confirmed.
- **Canary modes** (oxide#323). All modes share one receipt per pin:
  - *Live* (default): as above. It refuses when any staged capability differs
    from the installed receipt's: `control_state_version`,
    `registration_version`, `ledger_version` and `writer_generation`.
  - *`--scratch-root DIR`*: a fresh pool under `DIR`, created at an explicit path
    with `OXIDE_POOL_CREATE=1`. No live pool is read or written and no live-version
    check applies.
  - *`--format-change`* (optionally with `--scratch-root DIR`): for a build that
    changes the persisted format. Oxide reads the live `config.json`, `ledger.json`
    and `oxide-managed/state.json` read-only and copies them into a scratch pool
    layout. The copy always lives on the internal pool's volume (`DIR`, or the
    pool's sibling `scratch/` by default), never on a registered removable
    volume's scratch directory, because an explicit internal pool on an external
    volume is refused. The copy carries an empty owner-only `payload/slots/`
    directory (a payload with no `slots/` refuses as unknown accounting before
    the copied ledger is read); the staged binary initializes the copy's own
    Barge state under the copied `oxide-managed/` directory on first open and
    reconciles the copied ledger there, so the live pool is never written. The
    staged binary then runs status, a new-slot check (which
    performs the upgrade) and a warm rerun, all against the copy. It prints the
    control-state version before and after the upgrade and refuses if the upgraded
    version is not the staged one, if the live state is newer than the staged build,
    or if any live document's bytes or file identity changed during the run. Its PASS
    satisfies `--require-canary` for the pin.
- **Required canary.** `--require-canary` on install or reinstall requires a PASS
  receipt for the same pin, less than one hour old. It is checked before the build
  and again under the install lock before the swap.
- **Rollback** (`effigy oxide:rollback`) restores the prior binary named by the
  newest receipt. It verifies every hash, builds nothing, and writes an owner-only
  rollback receipt. Format differences then follow
  [persisted state](#persisted-state-versions-and-downgrade).
- **Process report.** After the swap, the installer lists same-user `oxide` processes
  it observed before the swap, with PID, start time and command line. It sends no
  signals. It is a point-in-time report, not proof of writer closure.
- **Preflight.** `--check` stops after the preflight and builds nothing.
- An update that adds a forward-only state fence needs no older-generation writer
  running. The planner confirms that no milestone or project QA run is active before
  installing. Atomic replacement leaves an already-running process on its old inode.
  Old commands can still refuse newer state.

## Installed-binary hand-off

A command whose `oxide` is older than the control state it finds hands off once to
the installed binary, instead of refusing:

- **Trigger:** before any store is opened, the front door reads at most three control
  documents, read-only: the internal `state.json`, and the removable pool's
  `removable/state.json` and `removable.json`. A numeric `version` above this
  binary's own triggers the hand-off. Missing, unreadable, oversized or malformed
  documents trigger nothing. Plain passthrough, usage-only `exec` and the launch
  probe skip the check.
- **Candidate:** exactly one executable, the newest install receipt's path, opened
  without following links, verified against the receipt's size and SHA-256, owned
  by this user, executable, not group- or world-writable, and reached through
  `PATH`. No other executable is searched or installed.
- **Hand-off:** prints one notice, `oxide: handing off to the installed binary (state is newer than this one).`,
  then executes the candidate with the caller's arguments and environment, adding
  `OXIDE_INTERNAL_HANDED_OFF=1`. That marker prevents a second hand-off. Signals and
  exit codes are the executed binary's.
- **Refusals:** any mismatch, or a missing candidate, prints
  `oxide: oxide was upgraded during this command; re-run.` and exits 75. Cargo's own
  exit 75 is passed through untouched.
- **Residual:** the executed file is checked, not bound. A same-user rename between
  the check and `exec` can substitute the inode.

## Persisted state versions and downgrade

- **Writer generation.** The control document carries `minimum_writer_generation`
  (control schema 10). The first mutating open publishes the binary's generation.
  Admission, registry publication and removable registration recheck it after
  taking their lock. A stale writer exits 76 with
  `oxide was updated while this command waited; rerun it`. Advance the generation
  only when an update must exclude older writers.
- **Format versions.** Bump the owning version for any change to persisted ledger,
  control, registry or receipt layouts, including optional fields. Either show that
  the previous reader can read the new bytes, or document the downgrade. The
  ledger writes version 2 and still accepts version 1. Install receipts are version 2.
- **Downgrade.** Before resuming an older writer or format, the planner positively
  closes Oxide writers and pauses launches. It covers the internal pool, any
  registered removable pool's control files, and any other pool the newer binary
  touched. Keep owner-only copies of the control and ledger documents before editing.
  Preserve every lease, slot, output link, Barge root and pin. Settle any
  `held-eviction.json` with the current writer first. Convert only a documented pair,
  one file at a time, through an owner-only sibling and an atomic rename. For any
  other pair, stop.

## Validation

Oxide follows the lean validation policy ([vision](../vision.md#validation-policy)):
ordinary tests, plus one independent review of the exact published head. No native
proof ladders, reviewer-native sessions or qualification selectors exist.

- Each product brief names its covering selectors, a wall-clock limit and a
  scratch-disk budget.
- The author runs `rust:compile`, `rust:compile-release` (with `-D warnings`),
  `rust:format` (`rust:format-write` while authoring), `docs:check` when docs
  change, and the covering unit and integration suite, `rust:barge-adoption`.
- `rust:barge-adoption` runs serially in one private per-run directory under the
  invoking `TMPDIR`. That directory is passed as `TMPDIR`, `TMP`, `TEMP` and the
  fixture roots. It is removed only after success. A failed run keeps it. Effigy
  owns the heavy-admission and 600-second deadline. Unit tests inject only the
  platform inputs that otherwise need boot-volume paths. Their file operations stay
  in the private fixture. Crash recovery, deletion and removable-media behaviour are
  covered by ordinary regression tests.
- Linux: `rust:barge-adoption-linux` compiles all targets offline for the installed
  target, including the Linux pool regression. It makes no link or runtime claim.
- Other surviving selectors: `rust:tree-duplicates`, `dogfood:scripts:test` (the
  installer's synthetic install, rollback and canary cases, including the three canary
  modes with a fake staged binary and a check that the format-change run never names
  or writes the live documents).
- The independent reviewer judges the exact head, its regressions and the author's
  evidence. The reviewer does not repeat the covering selectors and reports missing
  evidence.
- Resource figures are observations, not acceptance thresholds. Do not run full
  `effigy qa`, or repeat completed validation, without a changed input or an open
  finding.
- Installing a storage-affecting binary is a separate planner step after merge. See
  [dogfooding](#dogfooding-in-oxides-repository).

## Changelog and release

- Each pull request adds one fragment, `changelog.d/<task-or-PR>-<slug>.<type>.md`,
  where the type is `added`, `changed`, `fixed`, `removed` or `security`. The
  `[Unreleased]` section of `CHANGELOG.md` stays empty.
- `effigy changelog:check` runs inside `docs:check`. `effigy changelog:test` covers
  the checker. `effigy changelog:fold --version <x.y.z> [--date YYYY-MM-DD]` folds
  fragments only during release preparation, under explicit release authority.
- Release, tags and publication are governed by the [release contract](release.md).

## Not implemented

These are not current behaviour. Their designs are proposed only.

- `linux-image-v1` storage, CI cache save and restore, and container or remote
  executors. Linux has no runtime proof of storage behaviour yet.
- Nested commands joining their outer lease. Today each nested command takes its
  own independent lease; the future design is tracked in Nucleus.
- Separating an idle exec session's host permit from its slot. Today an exec session holds its host permit throughout; the future design is tracked in Nucleus.
- Alternative run strategies, such as ahead-of-time or JIT codegen or hot patching.
- Per-project profile overrides or project files. Oxide reads none.
- Interception of plain `cargo`, and an adaptive `rustc` wrapper for builds that
  bypass Oxide.
