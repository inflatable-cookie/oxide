# Oxide workflow

Status: product design under Tom's
[2026-10-18 workflow direction and bounded implementation authority](../vision.md#product-workflow-direction).
The foundation core and `init` / `new` adoption are implemented as the
unpublished `oxide` binary, installed for Tom only to
[dogfood](#dogfooding-in-oxides-repository) in this repository; the agent
skill exists as repository source only. [Implemented foundation core](#implemented-foundation-core)
states exactly what ships, including the cross-checkout freshness gate, the
budget-only directory backend, [Cargo home hygiene](#cargo-home-hygiene) and
part 1 of the [removable build volume](#removable-build-volume). Non-build run strategies and the
privileged `linux-image-v1` backend are not implemented, and nothing has run
on a real Linux host. Fixture proofs are not real-project
qualification, default-pool creation outside tests, installation (binary or
skill) or consumer-adoption authority. Other sections
state the target design.

## Shape

`oxide` is Cargo with managed storage, profiles and execution. In an adopted
project, the agent guidance is one sentence: write `oxide` where you would
write `cargo`. Setup is one binary on `PATH`. There is no toolchain shim, `PATH`
shim, shell alias or project file.
[Interception](../rust-dx-landscape.md#cargo-command-interception) is deferred
until this foundation works consistently.

```text
oxide <check|test|clippy|build|run> [--profile <name>] [cargo args] [-- program args]
oxide <any other cargo command> [args]   # passed to Cargo, with storage where it builds
oxide status                             # pool, slots, waits, stray Cargo output, Cargo home
oxide status --launch-check              # macOS: a new executable's first-launch penalty, advisory
oxide clean [--dry-run] [--include-foreign]  # remove <workspace>/target and slot affinity
oxide clean --cargo-cache                # one pass of Cargo's own home cleanup, network refused
oxide init [cargo init args]             # make this a Cargo project if needed, then adopt Oxide
oxide new <path> [cargo new args]        # cargo new, then adopt Oxide
```

Commands fall into four classes:

| Class | Commands | Behavior |
| --- | --- | --- |
| Modelled | `check`, `test`, `clippy`, `build`, `run` | Profile-aware, leased, strategy-selected (see [profiles](#profiles-and-strategies)). |
| Building passthrough | Other built-ins that compile (`doc`, `bench`, `rustc`, `rustdoc`, `fix`) and every unknown or external subcommand (`nextest`, `llvm-cov`, `miri`) | Passed to Cargo unchanged, under a lease with storage and profile definitions injected. Unknown commands are treated as building, which is the conservative choice. |
| Plain passthrough | Non-building built-ins (`fmt`, `tree`, `metadata`, `add`, `remove`, `update`, `info`, `search`, `install`, `publish`, `login`, `vendor`, `fetch`, `generate-lockfile`, `locate-project`, `pkgid`, `version`, `help`) | Run as plain Cargo; no lease. |
| Extended built-ins | `init`, `new`, `clean` | Cargo's command plus Oxide's step (see [adoption](#adoption-oxide-init)). `clean` removes the checkout's private outputs and slot affinity and never deletes a shared slot: a 2026-10-18 probe showed plain `cargo clean` also deletes the injected build directory. A whole-target clean is Oxide's own plan ([clean](#clean)). |
| Oxide-owned | `status` | Oxide semantics. |

- A built-in Oxide extends keeps Cargo's meaning and adds to it. Purely
  Oxide verbs never reuse a Cargo built-in name.
- Arguments pass through unchanged. `--release` means `--profile release`.
  Oxide owns storage placement, so `--target-dir`, and `--config` keys that set
  `build.build-dir` or `build.target-dir`, are refused with an explanation.
- All injection uses environment variables (`CARGO_BUILD_BUILD_DIR`,
  `CARGO_TARGET_DIR`, `CARGO_PROFILE_OXIDE_*`), because child Cargo calls inherit
  them. A 2026-10-18 probe on 1.97.1 showed that a `--config` profile
  definition is invisible to an external subcommand's `$CARGO` call, while an
  environment-only definition works there.
- Built-in commands get `--profile <oxide name>` translated to the Cargo profile
  name. External tools receive arguments untouched. They name the Cargo profile
  themselves (for example `oxide nextest run --cargo-profile oxide-quick`), or
  they get Cargo's default profile in a separate unit set.

## Profiles and strategies

An Oxide profile is the user's intent. A strategy is how Oxide meets that
intent for one command. Names stay stable while strategies improve.

| Profile | Intent | `check` / `clippy` / `build` | `run` | `test` |
| --- | --- | --- | --- | --- |
| `quick` (default) | Fastest route to feedback on the current source | Quick build recipe | Fastest qualified execution strategy, falling back to quick build plus execute | Quick build plus execute, unless a strategy preserves test semantics |
| `debug` | Full debugger experience | Full-debug recipe | Full-debug build plus execute | Full-debug build plus execute |
| `release` | Production-shaped output | Project's own `release` profile | Release build plus execute | Release build plus execute |

Cargo recipes are defined through environment variables only, so no
`Cargo.toml` edit is needed. A 2026-10-18 probe confirmed that a custom profile
defined that way builds, works from child Cargo calls and inherits the
project's own `[profile.dev]` settings:

- `oxide-quick`: inherits `dev`, `debug = "line-tables-only"` for every unit.
  Per-package overrides (`package."*"`) cannot be expressed through the
  environment; the probe showed the dependency stayed at line tables. So this
  recipe keeps dependency line tables, unlike the `oxide-light` recipe in
  [stable Cargo profiles](stable-cargo-profiles.md). Whether dependency debug-off
  is worth a config file is part of [W-3](#open-decisions).
  On macOS its default unpacked split debug info leaves per-rebuild
  `*.rcgu.o` names in `deps/`. A 2026-10-18 measurement recommends
  `split-debuginfo = "packed"` if a fix is adopted and rejects workspace
  debug-off for `quick` ([landscape](../rust-dx-landscape.md#stale-debug-object-measurement-2026-10-18)).
  The recipe is unchanged pending that decision.
- `oxide-debug`: inherits `dev`, full debug for every unit, including build-time
  units through `CARGO_PROFILE_OXIDE_DEBUG_BUILD_OVERRIDE_DEBUG`.
- `release`: untouched.

`debug` cannot be a Cargo profile name because Cargo reserves it; the `oxide-`
prefix also avoids clashing with project profiles. The recipes are the
intended defaults, not measured speed or storage claims. That contract's
comparison stands as a no-go.

**Run strategies.** A quick run means "execute the current source as soon as
possible". Candidates are a Cranelift ahead-of-time backend, Cranelift JIT and
hot patching of a running process. Each is qualified per platform and project
shape before Oxide may select it; current state is in the
[landscape](../rust-dx-landscape.md#quick-run-strategies)
([Q-009](../questions.md#q-009--which-alternative-execution-modes-improve-the-daily-loop)).
The 2026-10-18 [Cranelift measurement](../rust-dx-landscape.md#cranelift-measurement-2026-10-18)
found no edit-loop gain on Oxide's own workspace, so no strategy is selected.
Rules:

- Oxide prints the strategy it used and any semantic compromise, for example
  `catch_unwind` not catching inside Cranelift-compiled code on macOS.
- If a strategy cannot build or start the program, Oxide falls back to quick
  build plus execute. A program failure is reported, never retried under
  another strategy.
- `test` uses only strategies that preserve test semantics (unwinding,
  `should_panic`, harness behavior). Required validation keeps its own
  contract and authoritative backend.
- The codegen backend is part of the slot key, so strategies never share
  intermediates.

## Project identity without configuration

Identity chooses which warm slot to try:

1. Repository identity: the Git root commit(s) plus the workspace path within
   the repository. Worktrees and separate clones match; a fork also matches.
2. Shallow clones without the root commit fall back to the normalised `origin`
   URL.
3. Outside Git: the canonical workspace root, with no cross-checkout reuse.

The slot compatibility key is repository identity + Cargo profile + resolved
toolchain (`rustc -vV`) + target triple + codegen backend. The toolchain is
whatever rustup resolves where the command runs (see
[caller context](#cargo-version-matrix)); Oxide does not select or pin it.

## Cross-checkout freshness

Reusing one slot across checkouts is not automatically safe. Cargo 1.97.1
fingerprints workspace units by relative path and file mtime, and treats an
input whose `path_mtime <= reference_mtime` as fresh. Reviewed evidence
([qualification readiness](../qualification-readiness.md)) proved that a second
checkout reused the slot's units as fresh when its differing files were older
than the slot's last build. Cargo also omits `CARGO_MANIFEST_DIR` from encoded
dep-info, so units that embed the checkout path keep the previous checkout's
path.

Per-command leasing makes this ordinary. Example: worktrees A and B both exist
from time T0. A edits and builds at T1. B then builds on the same slot, and
B's differing files still carry mtime T0, which is earlier than T1. Nucleus's
current lease-at-placement model mostly avoids this by construction: a
worktree is created after the slot's last release, so its files are newer.
That is an inference from its lifecycle, not a qualification.

Freshness gate (W-8), implemented and fixture-proven, not yet
real-project qualified. It runs before Cargo on every leased command whose
slot has a recorded origin:

1. After each leased command whose Cargo ran, the slot records its last
   compile origin: canonical checkout path, Git tree of `HEAD`, the Git blob
   digest of every dirty or untracked non-ignored file, and the build anchor
   (the time after Cargo's process group is empty).
2. Oxide diffs the two content sets with Git (`git diff` of the trees plus the
   dirty digests). Each differing checkout file whose mtime is not newer than
   the anchor gets its mtime set to now, so Cargo's own invalidation rebuilds
   it. Content stays untouched; only the checkout's own files change; Cargo
   fingerprints and the slot are never edited.
3. On a checkout-path change, every local package under the checkout is
   path-sensitive, with no detection. `cargo metadata --format-version 1`
   names the packages: each one with no registry or Git
   source whose canonical manifest lies under the canonical checkout root
   (component-aware), workspace members and in-repo path dependencies alike.
   Metadata runs in the guarded command's context: the same Cargo, working
   directory (so the same config files, member-local ones included) and
   inherited environment, plus the command's own `--locked`, `--frozen`,
   `--offline`, `--config`, `-Z` and `--lockfile-path` arguments before any
   `--`, unchanged. Cargo merges them with `net.offline` and
   `CARGO_NET_OFFLINE` under its own precedence; `--frozen` stays locked and
   offline. The gate adds `--offline` and `--locked` where the command lacks
   them, so metadata itself never downloads a package or writes the lock.
   When Cargo then refuses because the lock is missing or stale:
   - A built-in Cargo build command (`build`, `check`, `test`, `bench`,
     `doc`, `rustc`, `rustdoc`; `run` builds through `build`) allowed to
     write a missing `Cargo.lock` gets it from `cargo generate-lockfile`
     with the command's own arguments: Cargo's resolution with no previous
     lock, as its build would do, with no package download. Metadata is
     then read again. A fresh worktree of a repository with an ignored lock
     gets warm dependency reuse this way. First Cargo parses the command's
     own arguments with an extra unknown `-Z` before any `--`; it refuses
     that flag before loading the workspace. Unless that refusal is Cargo's
     first error, Cargo would reject the command without resolving, so no
     lock is written and the slot is refused. External and unknown
     commands (including `clippy`), and commands passing their own `-Z`,
     never get a lock from the gate.
   - `--locked` or `--frozen` with a missing lock, or with `--frozen` or
     `--offline`, is the command's own refusal under plain Cargo: the
     command ends with Cargo's message and exit code, the slot goes back
     unchanged and nothing builds cold. This needs the same built-in
     command and parse check as lock generation below; otherwise the slot
     is refused and Cargo runs the command as usual.
   - Anything else refuses the slot: a stale lock the command may update, a
     `--locked` online command whose offline answer is not certain, or a
     help request (`-h`, `--help`), which resolves nothing. The cold build's
     own Cargo then updates, refuses or prints help as usual, and a later
     command from another checkout warms.
   Oxide edits nothing else.
   Every tracked and untracked non-ignored file in the checkout gets the
   same mtime treatment unless its deepest package directory belongs to a
   registry or Git package (vendored sources), so every local unit rebuilds,
   including build scripts that read `current_dir()` and proc macros that
   expand to the caller's manifest dir. Manifests and sources are never read
   to decide this. Any other metadata failure, output over 64 MiB
   or 60 s, two packages in one directory, a local manifest or target
   source Git cannot see, or a local target or symlink inside a vendored
   package refuses sharing (cold, checkout-affine). Registry, Git and
   outside-checkout path dependencies are not refreshed by this rule; they
   keep their paths and stay warm.

   Accepted trade-off: local workspace and in-checkout path units never
   warm-reuse across checkout paths. The cross-checkout benefit is dependency
   reuse. Same-path origins with different content use step 2 only.
4. A command need not rebuild every unit, so the slot also carries an
   uncertain set: every path that differed at an earlier command or changed
   while Cargo ran. Another checkout taking the slot refreshes those too. The
   compiling checkout's own copies are already newer than any unit built
   from other content, so a repeat in the same checkout refreshes nothing. The
   set only resets when the slot is evicted; past 4096 paths the slot stops
   being shared.
5. Inputs outside Git or ignored by it, and submodule contents, are not
   covered. The summary line says so whenever the gate reconciled another
   checkout's build. A differing submodule refuses the gate.

The origin is trusted only when the record's use counter still equals the
value stamped when it was written. Leasing re-stamps a trusted origin before
the slot moves to the new checkout. Cargo's start advances the counter, so the
origin stays trusted until Cargo starts. A command that dies before Cargo,
even mid-gate after taking another checkout's slot, therefore leaves the
retry to run the gate again. The slot becomes untrusted if a command dies
after Cargo started, a snapshot fails, a crash splits the re-stamp from the
state change, it is a legacy slot from an earlier Oxide build, or an older
Oxide build used it. Cargo never runs on an untrusted slot: the next command
to lease it releases and evicts it whole, then builds cold. If the gate fails
or cannot read the history, for example in a clone lacking the slot's tree
object, Cargo does not run on that slot either. A slot holding another
checkout's content goes back to that checkout unchanged; this checkout's own
slot is evicted. The command then builds cold, never with unchecked reuse.
A checkout without Git content (no repository or no commit) records an
affine build instead: only that checkout reuses it, under Cargo's own
checks, and it is never shared.

This revives the qualification work's rejected "source touching" fix as a
deliberate product mechanism. Q2–Q4 of the
[foundation qualification](#qualification-result) held on Oxide's own
repository. That record is partial, not a bounded pass, and a benefit claim
still needs a heavy project.

Debug paths are a separate, known limit. Reused workspace units carry the
compiling checkout's absolute paths in debug info. That is tolerable for
`quick` backtraces, but wrong for `debug` sessions after that checkout is gone.
Stable path remapping needs identical flags across checkouts to avoid
fingerprint churn. The retired source-map work found no cheap general fix.

## Cargo version matrix

The freshness gate, stray detection and dep-info decoder depend on Cargo
behaviour, so each is supported only on exact Cargo releases checked against
pinned source and a fixture proof
([evidence](../evidence/cargo-version-matrix-091/README.md), oxide#091).
Oxide itself still builds with 1.97.1. Each pin is the latest stable patch of
its minor in the official channel manifests on 2026-10-19.

| Release | `cargo -V` prefix | Cargo source pin | Gate | Stray | Dep-info v1 | Proof |
| --- | --- | --- | --- | --- | --- | --- |
| 1.95.0 | `cargo 1.95.0 (f2d3ce0bd ` | `f2d3ce0bd7f24a49f8f72d9000448f8838c4e850` | ✓ | ✓ | ✓ | run failed in the shared fallback phase (wrong expectation); every 1.95.0 phase passed first |
| 1.96.1 | `cargo 1.96.1 (356927216 ` | `356927216a2d746168cf76e5e88cc3f4b58e029d` | ✓ | ✓ | ✓ | pass |
| 1.97.1 | `cargo 1.97.1 (c980f4866 ` | `c980f4866141969fab6254a680546a277789d6f0` | ✓ | ✓ | ✓ | pass |
| 1.98.1 | `cargo 1.98.1 (797e8a9bc ` | `797e8a9bca276c1c9f9f738d2a20f484fa4eea9d` | ✓ | ✓ | ✓ | pass |
| 1.99.1 | `cargo 1.99.1 (5f94df478 ` | `5f94df4789f005f9a352888e8355ffc645b7ed0e` | ✓ | ✓ | ✓ | pass |

Source facts are the same in all five pins:

- `find_stale_file` treats `path_mtime <= reference_mtime` as fresh;
- local units hash workspace-relative paths;
- `cargo metadata` uses format 1;
- the lock refusal and the unknown `-Z` refusal keep their wording;
- `build.build-dir` is stable;
- the default layout keeps `<profile>/deps`, `incremental` and
  `.fingerprint`, and the unit-grouped layout still needs
  `-Zbuild-dir-new-layout`;
- encoded dep-info keeps marker `01 00 00 00 ff`, version 1.

Each `effigy rust:cargo-matrix:<release>` proof ran these phases under the
tested Cargo:

- the two-worktree gate scenario;
- effective build-dir: no intermediates in the checkout `target/`;
- `none` from `oxide status` after Oxide alone, and stray output reported
  after a plain `cargo check`;
- the shipped decoder over every slot and plain-target `dep-*` file, with
  none refused;
- a fallback phase on installed 1.94.1.

Fallback for any other Cargo, older, newer, another patch (1.96.0, 1.97.0)
or a rebuilt one. The table is `VERIFIED_CARGO` in `freshness.rs`. Oxide
runs `cargo -V` in the command's own working directory, not the workspace
root, because that is where the guarded Cargo and the gate's metadata call
resolve the toolchain. A member's `rust-toolchain.toml` therefore counts.

- **Freshness gate.** A new slot records an affine build: only that checkout
  reuses it, under Cargo's own checks, and the summary says `freshness gate
  off`. A content slot that needs refreshing is refused, and the command
  builds cold. An unchanged repeat in the same checkout runs no
  version-sensitive step, so it skips the `cargo -V` check.
- **Stray detection.** Intermediates it finds are reported. Finding none
  reads `unknown`, because another layout may keep them elsewhere.
- **Dep-info decoder.** Any other marker or version is refused, as before.

Slot compatibility includes `rustc -vV` of `RUSTC`, else of `rustc`, run
like `cargo -V` in the command's own working directory and environment, not
the workspace root (oxide#094). The guarded Cargo inherits both, and
`--manifest-path` moves neither: rustup selects the toolchain from the working
directory, so `--manifest-path` names only the package and the workspace that
owns the slot. Repository identity and the slot's checkout still come from
the canonical workspace root. `effigy rust:oxide-toolchain-context`
([evidence](../evidence/toolchain-context-094/README.md)) proves on
installed 1.97.1 (root) and 1.96.1 (member `rust-toolchain.toml`) that each
key digests the `rustc -vV` the build script saw Cargo use: root and member
contexts get separate slots of one owner, same-context repeats reuse warm,
`--manifest-path` either way follows the working directory, and `RUSTC` or
`RUSTUP_TOOLCHAIN` from the root select the member compiler's slot.

Not represented in the key, and so unsupported for compiler separation:
`build.rustc` from Cargo config files or `--config`, `CARGO_BUILD_RUSTC`,
`RUSTC_WORKSPACE_WRAPPER`, an inherited rustc wrapper kept by the
`OXIDE_ALLOW_RUSTC_WRAPPER=1` opt-in, and a `PATH` whose `rustc` belongs to a
different installation than its `cargo`. By default leased builds bypass the
inherited rustc wrapper ([below](#inherited-rustc-wrapper)). `oxide
init` readiness still reports the toolchain rustup resolves at the workspace
root. A new Cargo release needs this check before it joins the table.

## What one command does (local executor)

1. Resolve the workspace (`cargo locate-project --workspace`), identity,
   profile, strategy and toolchain.
2. Lease a slot from the host pool. Prefer this checkout's last slot, then any
   idle compatible slot, then a new slot if the per-project cap and budget
   allow. Otherwise wait in order, with a visible "waiting for …" line.
3. Run the freshness gate when the slot's last origin differs.
4. Run the real Cargo as the leader of a new process group, with:
   - the build directory pointed at the slot;
   - the target directory set to `<checkout>/target`, so final outputs stay
     private, as in the [local lease](local-lease.md);
   - the profile definitions in the environment;
   - `RUSTC_WRAPPER` set to the empty string, unless the opt-in keeps an
     inherited wrapper ([below](#inherited-rustc-wrapper)).
5. `run`: keep the lease for the build, release it, then execute the binary
   from the private target directory. `test`: keep the lease through test
   execution, because test binaries live in the build directory. Reuse never
   skips tests.
6. When the process group exits, record the compile origin, release the lease
   and print one line: profile, strategy, warm/cold, elapsed time, pool use.

### Inherited rustc wrapper

Leased builds bypass any inherited rustc wrapper, today the machine-wide
`build.rustc-wrapper = sccache` (Tom, 2026-10-20, oxide#099). Plain `cargo`,
plain passthrough such as `oxide fmt`, and other tools keep it; Oxide never
changes Cargo config, the sccache server or its cache.

- **Mechanism.** Every Cargo call Oxide makes for a leased command (the
  build itself, the freshness gate's metadata and lock calls, `run`'s target
  pick, a selected `clean`) gets `RUSTC_WRAPPER=""`. Cargo 1.97.1 reads that
  variable before any config layer ([`maybe_get_tool`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/util/context/mod.rs#L1805-L1825)),
  so it beats config files, `CARGO_BUILD_RUSTC_WRAPPER` and `--config`, and an
  empty wrapper is [skipped](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/crates/cargo-util/src/process_builder.rs#L549-L557).
  Cargo's [environment reference](https://doc.rust-lang.org/cargo/reference/environment-variables.html)
  documents the empty-string override. Child Cargo calls inherit it, so an
  external subcommand's `$CARGO` is covered. A child that rewrites its own
  environment is not contained.
- **Untouched:** `RUSTC_WORKSPACE_WRAPPER` and Clippy's driver, so `oxide
  clippy` runs clippy-driver with no outer wrapper; `RUSTC`,
  `RUSTUP_TOOLCHAIN`, the slot key and profiles.
- **Opt-in.** Only `OXIDE_ALLOW_RUSTC_WRAPPER=1` keeps the inherited wrapper.
  It is unqualified for Oxide's interruption semantics and toolchain identity:
  a wrapper's server can outlive the Cargo group Oxide closes. Any other value
  is ignored and reported; the wrapper is still bypassed.
- **Visibility.** The summary line names the bypassed wrapper once, for
  example `rustc wrapper bypassed: /opt/homebrew/bin/sccache`, or says the
  opt-in kept it, active and unqualified. `oxide status` prints the inherited
  wrapper, where it came from and its effective state. Oxide reads the name in
  Cargo's order (`RUSTC_WRAPPER`, the command's `--config`, then
  `CARGO_BUILD_RUSTC_WRAPPER`, then config files from the current directory
  up and the Cargo home, one per directory as Cargo picks it: `config` over
  `config.toml` when both exist). An unreadable file or an `include` reports the name
  as unknown, never as none; the bypass does not depend on the name.
- **Proof:** `effigy rust:oxide-wrapper-bypass`, on fixtures with a recording
  fake wrapper only. Evidence is in the oxide#099 PR. The selector is unqualified
  test tooling: timeout cleanup can leave Cargo's separate process group running.
  Its plain-Cargo staging has no host-reserve watchdog; the reserve is checked
  before and after. The final dual-config and corrected-cleanup cases have no
  native run. Tom's acceptance of the product source with these disclosed gaps
  does not qualify the apparatus or supply that missing evidence.

The [qualified cross-worktree result](project-adoption.md#focused-cross-worktree-result)
supports this shape for one workflow: a different-path checkout on a shared
slot rebuilt only its changed unit, and tests ran every time. It did not isolate
the mtime hazard above.

## Crash recovery

Tom ruled on 2026-10-18 that Oxide must manage crash recovery itself
([vision](../vision.md#product-workflow-direction)). Shipped rule, for
leases taken by `oxide` commands only:

- While a command holds a lease, it holds an exclusive `flock` on the lock
  file of its control record. The kernel drops that lock when the process
  dies for any reason; the descriptor is close-on-exec, so Cargo does not
  inherit it. The record names the holder (PID, process start time and the
  unique lease holder string), the lease ID once granted, the command phase
  and Cargo's process-group ID. The lease ledger is unchanged.
- Ctrl-C and termination signals are forwarded to the group. Oxide waits for
  the group to empty, then releases normally. A clean exit or Ctrl-C is
  never logged as a recovery.
- A later command recovers a non-idle record's held lease only when the
  record lock can be taken **and** the held lease is the record's own (its
  lease ID, or its holder string if it died before saving the ID) **and**
  either Cargo never started (phase acquiring, so no group exists) or
  `killpg(pgid, 0)` returns `ESRCH`. Cargo already tolerates interrupted
  builds: a unit whose fingerprint was not written is rebuilt.
- A surviving group member, a reused group ID or any error other than
  `ESRCH` keeps the slot held. On macOS `EPERM` also means a group of only
  unreaped zombies ([probe](../qualification-readiness.md#process-containment));
  it stays held until they are reaped and `ESRCH` follows. The fixture proof
  waits for that reap within its existing bound instead of reading `EPERM`
  as gone. No PID age, TTL or start-time comparison is
  used to decide. A follower in the same checkout waits; other commands go
  on with other slots.
- A command that dies after recording "Cargo starting" but before recording
  its group cannot prove the group empty. That slot stays held, `oxide
  status` marks it interrupted with no recorded group, and
  `oxide pool recover-interrupted <slot>` is the explicit release (it
  replaced the retired standalone CLI's `recover`, oxide#102). It still
  refuses while a live command holds the record, for a retired slot, or when
  the next command would recover the slot automatically. A record written by
  an earlier front-door build has no holder, so it is kept the same way.
  `oxide status` names which of these applies to each interrupted slot and
  promises automatic recovery only where the rule allows it.
- Each automatic recovery is logged in the control state (slot, lease,
  checkout, holder PID and start time, group, time, recovering PID). The
  event is written to the record before the ledger recovery and moved into
  the log after it, so a crash between the two still logs it on the next
  sweep. `oxide status` shows the newest 16.
- A held lease with no Oxide control record (for example one an older
  standalone-CLI build took) has no lock holder, so it is never
  auto-recovered.

**Residual risk.** A descendant that leaves the group with `setsid` or a new
spawn group is invisible to `killpg`. If it still writes in the slot when the
group is otherwise empty, the slot can be recovered and reused under it.
This is the accepted limit; the fixture proof shows such an escapee does not
keep the slot held.

This replaces the local lease's "never infer abandonment" rule for Oxide's
own commands only. Consumers that lease per task, such as Nucleus, keep their
explicit `recover` unless they adopt it.

## Storage and space management

- One per-user host pool: the budget-only
  [directory backend](#budget-only-default-pool-and-user-configuration) on
  every platform. Its soft budget is checked at admission; a running build
  may grow past it while real free space holds. Only whole idle slots are
  evicted. The APFS sparse-image backend is retired (oxide#102).
- The first command creates the pool automatically ([W-1](#open-decisions)).
- Per-project slot cap, recommended 2 to start, plus a host-wide limit on
  concurrent Cargo invocations ([W-2](#open-decisions)). Queue/Nucleus still
  decide which tasks run. Oxide only orders the Rust work they launch.
- `oxide status` reuses the read-only [storage inventory](storage-inventory.md).
  It reports pool and slot use (each slot's file names, distinct inodes and
  allocation counted once per inode), the Cargo home's allocation, and stray
  output: a checkout `target/` holding `deps/` or `incremental/` was built by
  plain Cargo. Finding none reads `none` only under a
  [verified Cargo release](#cargo-version-matrix); otherwise it reads
  `unknown`. Reclaiming that space is an explicit command; live cleanup
  still needs scoped approval. Nothing inside a slot is ever deleted.
- `OXIDE_POOL_CAP` or the config file sets the soft budget for the next
  admission; `oxide clean --cargo-cache` asks
  Cargo for its own home cleanup ([Cargo home hygiene](#cargo-home-hygiene)).

## Execution location

Every command becomes a request handed to an executor:

- Request: repository identity, source snapshot (commit plus uncommitted diff,
  or the local path), command, profile, strategy, Cargo arguments, target
  triple, environment allowlist.
- Result: streamed Cargo diagnostics in the requested format, exit status, test
  results and, for `build`/`run`, an artifact manifest.

The foundation ships only the local executor. Remote kinds (LAN host over SSH,
CI, cloud) select through per-user settings or `--on <name>`, never project
files. For `run`, build and run locations are independent. Remote execution
must not become required infrastructure for the core experience
([landscape](../rust-dx-landscape.md#experimental-frontier)).

## Agent integration

1. **Oxide agent skill.** Source at
   [`skills/oxide/SKILL.md`](../../../skills/oxide/SKILL.md). The rule is
   "`oxide` wherever you would write `cargo`". The skill also covers profiles,
   the reported strategy, waits and refusals, `oxide status`, and `oxide clean`
   versus plain `cargo clean`. Installing it into an agent skill root is not
   authorized.
2. **Project guidance block.** `oxide init` writes one marked section in the
   project's `AGENTS.md`, a few lines long. The block is the opt-in marker; no
   config file exists.
3. **Effigy selectors** in Effigy projects call `oxide …`. Effigy stays
   language-neutral.

Plain Cargo stays harmless: it builds privately, as today. `oxide status` makes
drift visible. rust-analyzer keeps its own private check target unless the user
routes `check.overrideCommand` to `oxide check --message-format=json`; editor
routing is outside the foundation.

## Adoption: `oxide init`

`oxide init` is a superset of `cargo init`, so one command takes a directory
to an Oxide-adopted Rust project:

1. If the target directory has no `Cargo.toml`, run `cargo init` with the
   supplied arguments (`--lib`, `--name`, `--edition`, `--vcs`). Inside an
   existing workspace, Cargo adds the new member as it normally does.
2. If a `Cargo.toml` already exists, skip Cargo. Cargo-only arguments then
   refuse with an explanation instead of being silently ignored.
3. Resolve the workspace root and write or refresh the marked Oxide block in
   the root `AGENTS.md`, creating the file if absent. Re-running is
   idempotent and never touches text outside the markers.
4. Print readiness: resolved toolchain, inferred project identity, pool state,
   and any Effigy Rust selectors that still call plain `cargo`. Oxide reports
   those; it does not edit them.

`oxide new <path>` is `cargo new` followed by steps 3 and 4. Neither command
creates an Oxide config file or changes `Cargo.toml` beyond what Cargo itself
writes.

Block rules, as implemented:

- The block sits between whole lines `<!-- oxide:begin -->` and
  `<!-- oxide:end -->` and is replaced whole on refresh. With no markers it is
  appended after one blank line. A marker inside a line is prose. Any other
  marker count or order refuses with nothing written.
- An unchanged block leaves the file untouched. A change is written through a
  sibling temporary file and a rename that keeps the file's permissions. An
  `AGENTS.md` that is a link or directory refuses and prints the block.
- The location is the workspace root `cargo locate-project --workspace`
  reports for the target's manifest, so a new member lands the block at its
  workspace root. When Cargo cannot resolve a workspace, for example a package
  inside a workspace that does not list it, Oxide refuses and writes nothing;
  Cargo's own init result stands.
- With a manifest present, every argument other than the directory refuses as
  Cargo-only. `--help` passes to Cargo.
- Readiness never creates the pool or edits a file. The Effigy report scans
  each `effigy.toml` from the workspace root up to the Git top level, line by
  line, and names lines that run `cargo` as a command word; comments,
  `cargo-*` binaries and `$CARGO` do not count.

### Switching an existing project

The standard pattern for consumer projects (Tom, 2026-10-20, [vision](../vision.md#product-workflow-direction)). It was first applied to Underlay, Praxis, Longhorn, Figmatic and Acowtancy; Queue lead `f4813b68` tracks the rollout.

- **Selectors.** Ordinary build, check, clippy, test and nextest selectors call `oxide`. Each main selector keeps a `:cargo` sibling with the plain-Cargo command, for when `oxide` is broken.
- **CI-aware rule.** When CI or release workflows call the same Effigy selectors and the runners lack `oxide`, the selector runs `cargo` under `GITHUB_ACTIONS=true` and `oxide` otherwise. Locally, a missing `oxide` fails loudly and names the `:cargo` escape; it never falls back silently. Workflow files do not change. Installing `oxide` on runners waits on publication (Q-005).
- **Stays on Cargo:**
  - scripts that choose their own `CARGO_TARGET_DIR`, because Oxide replaces an inherited value with `<workspace>/target`;
  - tools that run Cargo themselves (the Tauri CLI, packaging scripts), until `oxide exec` lands;
  - Queue warm-merge hooks;
  - container builds;
  - selectors whose scripts run a binary from `target/debug/` after a quick build. Under the current `oxide-quick` output directory they would miss the fresh binary or run a stale one. #106 moves `quick` and `debug` output to `target/debug/`.
  - tests that find a sibling binary by going from `current_exe()` in `deps/` to its profile directory. With Cargo's build-dir, test executables run from the slot (`<slot>/build/<profile>/deps`) while final binaries are uplifted to `<workspace>/target/<profile>/`. This holds for any build-dir user, and #106 does not change it. Repair: integration tests use `env!("CARGO_BIN_EXE_<name>")`; other code also tries `$CARGO_TARGET_DIR/<profile dir name>/`, which Oxide exports. Found by Figmatic's post-merge QA on 2026-10-20: 7 studio tests failed with "no capture renderer beside …/slots/4/build/oxide-quick".
- **nextest.** Until #106, pass `--cargo-profile oxide-quick` so nextest shares the quick slot.
- **Worktree seeding.** Selectors that move to `oxide` drop any copy of a primary checkout's `target/` into worktrees. The shared slot is the warm state.
- **Proof per project:**
  - the moved selectors pass under `oxide` and, where the CI-aware rule applies, under `cargo` with `GITHUB_ACTIONS=true`;
  - `oxide status` shows the project's slot and which pool served it;
  - one cold run and one warm repeat are timed, as observations.
  - Anything that changes a test result stays on `cargo` and is reported as a dogfood finding.

## Dogfooding in Oxide's repository

Approved by Tom on 2026-10-19 ([vision](../vision.md#product-workflow-direction)).
Oxide's own repository is the only adopter. No skill, other project, consumer
or global Cargo configuration is involved.

- **Installed binary.** `~/.local/bin/oxide` is a regular file built with
  `cargo build --release --locked --offline --bin oxide` from a commit on
  pushed `origin/main`, never from a branch. `effigy oxide:install`
  authenticates the pin with `git ls-remote`, builds it from `git archive` in
  a private `/private/tmp/oxide-install.*` staging directory, installs through
  a sibling temporary file and `link` (no clobber), and writes a JSON receipt
  under `~/.local/state/oxide/install/`. The receipt records pin, tree,
  toolchain, build command, path, owner, mode, size, time and SHA-256. It
  refuses when any `oxide` is on `PATH` or the destination exists, and when
  any directory from `HOME` to `~/.local/bin` is a link, is owned by another
  user or (below `HOME`) is group or world writable. It writes only through
  the opened directory, with no-follow opens.
- **Updating.** After a reviewed merge, the planner runs
  `effigy oxide:reinstall`. It replaces only a regular, user-owned binary
  whose hash matches the newest receipt. Directly before the rename it
  rechecks that the destination is still that file (type, owner, inode and
  hash) and refuses otherwise. Earlier receipts are kept. `OXIDE_INSTALL_PIN=<commit>` selects an older commit on main, for
  example to roll back. Effigy does not forward selector arguments, so the
  pin travels in the environment.
- **Default pool.** The first leased command created
  `~/Library/Caches/oxide/pool` under the then-shipped rules: a 24 GiB
  `apfs-sparse-image-v1` image, refused below the 20 GiB host reserve. That
  backend is retired (oxide#102): current `oxide` never attaches, writes or
  removes that pool and `status` lists it as `legacy pool`. Its ordinary
  detach and deletion are a separate Tom-approved planner step after fresh
  positive idleness.
- **Selectors.** `rust:compile`, `rust:compile-linux`, `rust:format` and
  `test rust` call `oxide`. With the default `quick` profile, check and test
  use line tables rather than `dev`'s full debug info. Focused proof, profile
  and install selectors call Cargo, because they test or build Oxide itself.
- **Escapes.** `rust:compile:cargo`, `rust:compile-linux:cargo`,
  `rust:format:cargo` and `test rust-cargo` keep the plain-Cargo commands.
  Use them when `oxide` itself is broken. They build into the checkout's
  `target/` unless `CARGO_TARGET_DIR` points elsewhere, and `oxide init`
  readiness lists them as plain-Cargo lines by design.
- **Test supervisor.** `test rust`, `test rust-cargo` and `rust:pool-scale`
  run under the [shared proof supervisor](#shared-proof-supervisor).
- **Fixtures.** `effigy dogfood:scripts:test` proves the installer's refusals
  (linked or foreign directories, an existing destination, a destination
  swapped during the build) on synthetic fixtures. It never builds or
  installs.

### Shared proof supervisor

`scripts/bounded-test.py` is the one shared session supervisor for
repository tests and native proofs, qualified by oxide#109 on macOS at the
source its receipt names. It owns the whole proof command, not just Cargo:

```sh
python3 scripts/bounded-test.py [--limit S] [--scratch-budget-mib N] [--receipt PATH] [--] command ...
```

- **Lifecycle.** It starts before the command's first writer and stays
  active through child closure and owned scratch cleanup. A future short
  fixed proof script runs entirely as its command (an Effigy `heavy` task
  whose `run` is `python3 scripts/bounded-test.py python3 scripts/<proof>.py`)
  and writes only inside `OXIDE_TASK_SCRATCH_ROOT`. Staging, fixtures and
  Cargo then share one guard; the script adds no supervision or accounting
  of its own.
- **Deadline.** One absolute limit (default 300 s) covers the command,
  closure and cleanup. Work stops 30 s before it; every closure step uses
  only what remains. `ps` is called with the remaining time as its timeout;
  drains, reaping and scratch removal are joined with bounded waits. Nothing
  starts a fresh timeout window past the limit.
- **Host reserve.** Raw `statvfs` `f_bavail` of the scratch's file system is
  sampled every 0.25 s on a thread. The abort threshold is 22 GiB: the
  20 GiB reserve plus the 2 GiB margin. A low or missing first reading
  refuses before spawn. Any sample below it, a failed sample or a gap over
  1 s latches a fault and cancels the command; the check uses each
  sampler's whole history, so a dip between checks is not lost. This is a
  sampled abort threshold: unrelated writers can overshoot between samples.
  The receipt reports the sample count, largest gap and observed minimum.
- **Scratch.** Without a caller's `OXIDE_TASK_SCRATCH_ROOT` it creates and
  owns `/private/tmp/oxide-proof.*` and passes it as
  `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP`. A caller's root is
  adopted, monitored and never removed. It is walked every 2 s, hard links
  once. A complete sample over the budget (default 512 MiB), a failed walk or
  none for 10 s faults. The budget is monitored, not a hard ceiling. Owned
  scratch is removed only after positive closure.
- **Closure.** The command leads a new session. Its exit is seen through
  kqueue `NOTE_EXIT` without reaping, so the session ID stays reserved
  while closure observes it. Every path (exit at any status, deadline,
  monitor fault, SIGINT or SIGTERM to the supervisor, exception after spawn)
  runs one closure: a fresh `ps` observation of the session, TERM to every
  group with a member positively in it (Cargo's separate `setpgid` groups
  before the leader's), a bounded wait, KILL to the survivors' groups, a
  bounded wait, then a fresh observation. A group other than the leader's is
  re-verified through a live member just before each signal; a member whose
  session cannot be read is never signalled. Closure is positive only when
  no member remains, the leader is reaped and both output pipes reached EOF.
  An observation, signalling or reaping error, a survivor, an exhausted
  budget or a pipe still held open is `UNCLOSED`: the exit is nonzero and
  owned scratch is kept. If observation fails, the leader's own group (still
  positively owned) is KILLed, and closure stays `UNCLOSED`.
- **Exit and output.** The command's status when healthy and closed; 124
  deadline; 125 refusal, guard fault, exception, or `UNCLOSED` with an
  otherwise zero status; 128+N on signal N. The primary cause and any
  cleanup errors are reported separately. Leftover groups settled after a
  healthy exit are reported, and the status is kept. Output is forwarded by
  threads; past 32 MiB per stream it is drained and discarded.
- **Scope.** The cooperative session only. A process that calls `setsid`
  leaves it and is seen only while it still holds an output pipe;
  arbitrary same-user writers are outside the promise. Neither is proven
  contained. A PID recycled between a group's re-verification and `killpg`
  is a residual race. macOS only.

**Qualification.** `effigy proof:supervisor:qualify` (heavy) is a short
bootstrap around the candidate, not a qualified guard itself: a healthy
reserve reading before and after, its own fixed 300 s wall limit, and tiny
fixtures. It runs `bounded-test.py --limit 280 --scratch-budget-mib 512`
around `scripts/test_supervisor.py`, which runs, in order:

- pure cases with a fake clock, process table, signals and readings: low
  initial reserve, the exact 22 GiB threshold, measurement errors, stale
  sampler, a latched dip, scratch over budget, failed process listing,
  expired deadline, budget exhausted mid-escalation, an unreadable-session
  member and a recycled group (never signalled), a pipe held outside the
  session, exception after spawn, spawn failure, cancellation, status
  mapping;
- writers before and after a simulated staging step in the candidate's
  scratch;
- real tiny Python children: normal and nonzero exit with a live
  TERM-ignoring child in its own group, timeouts against an `oxide`-shaped
  forwarder and a group whose leader already exited, pipe pressure, an
  endless writer, an injected low reserve, an exception after spawn, and
  SIGTERM and SIGINT to the supervisor process.

Each real case requires positive closure and every fixture PID gone when the
supervisor returns, well before the fixtures' 90 s self-exit fallback. The
bootstrap prints a receipt binding the commit, the SHA-256 of supervisor,
suite and bootstrap, the platform, every case and the limitations. Effigy
admission waits are outside its 300 s. `effigy proof:supervisor:test` runs
the pure cases alone. Qualification receipts are recorded on the
oxide#109 PR.

Earlier evidence stays historical. `test rust` and `rust:pool-scale` keep
their invocation. Their runs from oxide#109 on get continuous reserve
sampling and the 270 s work window. Pool-scale's own `mktemp` scratch lies
outside the supervisor's monitored scratch, so its 512 MiB figure remains
its own phase measurement. `scripts/bounded_runner.py` (oxide#097) is
separate and stays unqualified.

### Rollback

Do not rehearse these steps; they remove the dogfood artifacts.

1. Stop starting Oxide work: switch selectors to the `:cargo` escapes or
   revert the adoption commit, which restores the plain-Cargo selectors and
   removes the marked `AGENTS.md` block.
2. Unlink the binary only if it still matches the newest receipt:
   `shasum -a 256 ~/.local/bin/oxide` equals its `installed.sha256`. Keep the
   receipts. A mismatch means an unknown file: leave it and report.
3. To retire a legacy image pool: current `oxide` never opens it, so prove
   idleness from fresh evidence (no process holds its lock, image or payload;
   every ledger slot idle, no waiter and no pending recovery in its
   `oxide-managed/state.json`). A held or unknown holder, group or waiter
   means stop: preserve and report. Never reset a lease by hand.
4. In `hdiutil info`, find the entry whose `image-path` is
   `~/Library/Caches/oxide/pool/image.sparseimage` and whose payload mounts
   at `~/Library/Caches/oxide/pool/payload`. Run an ordinary
   `hdiutil detach <its /dev/diskN>` without `-force`. If it reports busy,
   stop and report.
5. Once detached, the owner may remove `~/Library/Caches/oxide/pool` after
   checking that its `receipt.json` names that same image and mount point.
   Removal is explicit owner action, never automatic.

## Relationship to existing work

Tom approved retiring the earlier machinery on 2026-10-18, on the condition
that nothing of distinct value is lost. oxide#070 executed it;
[retirements](../retired.toml) lists the removed terms, paths and features.
Assessment:

| Keep | Why |
| --- | --- |
| [Local lease](local-lease.md) library: pool, ledger, leases, recovery, eviction | The storage layer of every `oxide` command. Its standalone CLI and the APFS image backing are retired (oxide#102); Nucleus keeps its installed copy until its owner switches. |
| [Storage inventory](storage-inventory.md) | Backs `oxide status` and stray detection. |
| [Stable profiles](stable-cargo-profiles.md) knowledge | Source of the `quick` / `debug` recipes. |
| Cargo freshness, path-embedding, containment and capacity findings in [qualification readiness](../qualification-readiness.md) | Ground the freshness gate, debug-path limit and crash-recovery design. Kept as evidence; the narrative is condensed. |
| [Build-context policy](build-context-policy.md) | Candidate input for per-command incremental/debug defaults; stays proposed. |
| Bounded dep-info decoder (`src/dep_info.rs`) | Per-unit Cargo input lists, a candidate for reporting inputs outside Git; separable from the retired conversion. |

| Retired | Why it has no distinct value now |
| --- | --- |
| Protected ordinary-Cargo route, storage daemon, transport/codec, HMAC attempts and handoff, `oxide-storage-runtime`, `oxide-qualified-runner`, `oxide-ordinary-cargo-attempt` | Nucleus rejected the signed callback shape; the lean lease replaced it. |
| Declared-workflow front end (`oxide-declared-workflow`) | Superseded by zero-config identity and the `oxide` commands. |
| Registered runner/host bridge, unit-graph and fingerprint collectors | Evidence infrastructure for a route no longer pursued. |
| Registry archive verifier/acquisition (`oxide-registry-acquisition`) | Registry deps sit at shared paths; Cargo already verifies archives. |
| Source-map qualifier and probes | Their finding (no cheap stable remap) is recorded above; the code serves no remaining route. |

The mtime-stale content-gate implementation (`src/project/freshness.rs`) did
not separate from the retired adapter: it was bound to the fixed source-map
fixture, the resolver and the daemon codec. It was removed; W-8 reimplements
the gate from the retained [Cargo freshness findings](../qualification-readiness.md#cargo-freshness).

The bounded Cargo dep-info decoder did separate. Its `decode` and plain decoded
types never depended on the registered route; only the `into_registered`
conversion did. The decoder now lives at `src/dep_info.rs` with its
hand-annotated upstream-bytes and refusal tests, unchanged limits (16 MiB
input, 32768 references, 2048-byte strings) and its own two-variant refusal.
The conversion was retired. No command calls it: the W-8 gate works from Git
content alone. Its per-unit file and environment lists could later name the
inputs outside Git that the gate does not cover. Its format is
[verified](#cargo-version-matrix) unchanged for Cargo 1.95.0 to 1.99.1.

## Foundation slice

The local executor on macOS, with: the command classes above; the three
profiles and the build fallback for every strategy; inferred identity;
per-command leasing with waits and crash recovery; the freshness gate (or
checkout affinity until it qualifies); automatic pool creation; `oxide status`
with stray detection; the skill and `oxide init` / `oxide new`. It is working consistently
when a second worktree's first `oxide test` on a real project reuses dependency
units. Tests must run, the stale-mtime scenario must rebuild, the pool must
stay under its ceiling, and normal use must leave no plain-Cargo intermediates.
Re-evaluate interception and run strategies after that.

## Foundation qualification

Authorized as a bounded follow-up under Tom's
[2026-10-18 ruling](../vision.md#product-workflow-direction) and run once on
2026-10-18 (oxide#074). **Verdict: partial.** All seven scenarios held, but
the session's bounds evidence falls short; see [Result](#qualification-result).
It was the gated real-project step after the foundation tasks merged and closed
out. The checked Queue brief owned the fixed resource, invocation and deadline
limits. Its job was to prove correctness and lifecycle on Oxide's own
workspace. A benefit claim is not part of it.

**Subject.** Oxide's own repository at a pinned post-foundation commit, so no
consumer coordination is needed. It is a real Cargo workspace with registry
dependencies but a small graph (about 39 lock entries), which makes it good for
correctness and weak for benefit. A benefit reading on a heavy project (for
example Nucleus) is a separate, owner-coordinated second stage.

**Setup.**
- Two disposable Git worktrees, A and B, of the pinned commit under one scratch
  root.
- One fresh dedicated pool, not the default path, small enough to keep the host
  above its 20 GiB reserve.
- Cargo runs `--offline` against the already-populated host registry.
- One disposable patch adds a marker test that reads a checkout-local fixture
  and `CARGO_MANIFEST_DIR`, so a stale result is observable.

**Scenarios, in order, each through `oxide` only:**

| # | Action | Must hold |
| --- | --- | --- |
| Q1 | `oxide test` in A | Cold; tests run and pass. |
| Q2 | `oxide test` in B (worktree created before Q1) | Registry dependency units fresh; the gate rebuilds every in-checkout package; B's marker test sees B's paths and content. |
| Q3 | Edit the marker fixture in A, `oxide test` in A, then `oxide test` in B without touching B | The stale-mtime scenario: B rebuilds the affected units and its marker test sees B's unchanged content, not A's edit. |
| Q4 | Repeat `oxide test` in B | Nothing rebuilt; tests run. |
| Q5 | SIGKILL `oxide` during a build in A, then `oxide test` in A | The next command auto-recovers the slot; the build completes; tests pass. |
| Q6 | Plain `cargo check` in B, then `oxide status` | The stray `target/` intermediates are reported with their bytes. |
| Q7 | `oxide status` at the end | Pool under its ceiling, every lease released, no held or uncertain slot. |

**Evidence.**
- Cargo JSON fresh/rebuilt rows per scenario.
- Elapsed time.
- Slot and pool bytes, checkout `target/` bytes and host free space at each
  boundary.
- The `oxide` summary lines.

Raw output is preserved. Readings are observations, not speed or SSD claims.

**Authorized bounds.** At most nine `oxide` invocations plus the single
plain `cargo check` in Q6, and one counted staging build:
`cargo build --release --locked --offline --bin oxide --bin oxide-storage-lease`.
Stage at the pinned commit in a separate pristine scratch checkout, with target
output outside the dedicated pool and A/B; record the commit, binary paths and
SHA-256s. One counted `oxide-storage-lease --pool <pool> init --cap-bytes
4294967296 --image-bytes 4294967296` before Q1 creates the pool, because the
front door always creates 24 GiB. A failed setup build or init is a no-go.
The one fresh 4 GiB APFS pool, original 30-minute clock including all setup,
raw 20 GiB host reserve, and 20-minute evidence-only review stay unchanged.
No second build/session, consumer, default pool, installation or host
configuration change.

**Pass.** Q1–Q7 all hold. Any stale success in Q2 or Q3 is a hard fail and
keeps slots checkout-affine. A failure in another scenario is reported with its
evidence and does not reset bounds.

### Qualification result

Run 2026-10-18 at main `dc84d039` under Effigy heavy admission. Raw evidence:
[foundation-qualification-20261018](../evidence/foundation-qualification-20261018/README.md).
- Session calls: one setup build, one lease init, nine `oxide` calls, one
  plain check. None failed and none was retried.
- Time: 356.8 s of the 30-minute window by wall clock, 297.3 s of it waiting
  for admission.
- Pool: 4 GiB. Peak payload 354 MiB; backing file 501 MiB at the end.
- Host free space stayed about 716 GiB.

| # | Scenario | Observed |
| --- | --- | --- |
| Q1 | Held | Cold: 10 local and 29 registry units built; 81 tests passed; marker saw A. |
| Q2 | Held | Slot 1 warm. All 29 registry units fresh; the gate refreshed 149 files and all 10 local units rebuilt. B's marker matched compiled and runtime: B's path and content. |
| Q3 | Held | B's unchanged marker was older than A's edit and the slot's build. B rebuilt all 10 local units, and its marker saw `original marker`, not `edited in A`. No stale success. |
| Q4 | Held | All 39 units fresh; 81 tests passed. |
| Q5 | Held | `oxide` was SIGKILLed after Cargo started; Cargo's group survived and finished. The retry waited, logged the recovery once the group was empty, evicted the untrusted slot, built cold in slot 2 and passed. |
| Q6 | Held | Plain `cargo check` wrote `B/target`. `oxide status` reported `deps/` or `incremental/` with 72 files and 41 MiB. |
| Q7 | Held | 197 of 3895 MiB used. One idle slot, 0 waiting, accounting complete, recovery logged. Afterwards the pool was detached without force and removed. |

**Why partial, not pass.** The pass rule requires bounded evidence as well
as the scenarios, and the independent review found that evidence insufficient
on two points:
- **No continuous monotonic clock.** The session clock was a wall-clock
  start taken before the scratch root existed. Python 3.9's monotonic clock
  restarts in each process on macOS, so it could not be carried across. A
  monotonic clock covers only the recipe process: per-call monotonic and wall
  durations agree within 1 ms. The 297 s before admission has wall-clock
  evidence only.
- **Cumulative Cargo count exceeded.** During the earlier blocked preflight,
  `cargo -V` ran twice. The brief counts version probes inside the caps, so
  cumulatively there were 4 Cargo calls against a cap of 2 (one setup build,
  one Q6 check). Only the author's transcript records those probes.

The scenario results stand as observations. A clean pass needs an operator
ruling: either accept these two deviations, or authorize a new session. Before
any new session, take the clock from one system-wide monotonic source (for
example `CLOCK_UPTIME_RAW`) and run no probes outside the caps.

Limits:
- Every slot move in this session was also a checkout-path change, so the
  gate refreshed whole packages. The content-only refresh without a path
  change is covered only by the fixture proof.
- The subject has a small graph, so the run says nothing about benefit on a
  heavy project.
- The front door could then only create 24 GiB pools, so the small pool
  needed `oxide-storage-lease init`. `OXIDE_POOL_CAP` (oxide#085) now sets a
  new pool's cap.

## Implemented foundation core

The unpublished `oxide` binary (`src/bin/oxide/`, `publish = false`) runs the
local executor on macOS. Its covering proof is `effigy rust:oxide-cli`: tiny
offline fixture crates on one fresh 128 MiB APFS pool. Its only installation
is the [dogfood binary](#dogfooding-in-oxides-repository). Its first
real-project run is the [foundation qualification](#qualification-result)
on Oxide's own repository. Its result is partial, not a bounded pass.

- **Classes.** As in [Shape](#shape). Plain passthrough replaces Oxide with
  Cargo, so output and exit codes are Cargo's. Unknown commands are building
  passthrough. A leading `+toolchain` or Cargo flag before the command
  refuses; `--version`, `-V` and `--list` pass through.
- **Profiles.** `quick` (default) and `debug` translate to `oxide-quick` and
  `oxide-debug`; `release` and `--release` use the project's `release`. Both
  recipes are always defined in the environment, so external tools can name
  either. Building built-ins translate `--profile` only when given; external
  subcommands receive arguments untouched.
- **Refusals.** `--target-dir`, `--config` keys `build.build-dir` /
  `build.target-dir` (any quoting or `build = {…}` form) and every `--config`
  file path refuse, because a file could set either key.
- **Identity and key.** Git root commits plus the workspace prefix; shallow
  clones use the normalised `origin`; otherwise the canonical path. The slot
  key adds the Oxide profile, the `rustc -vV` digest, the target (`--target`,
  then `CARGO_BUILD_TARGET`, then host) and the default backend. Building
  passthrough uses the quick key unless a built-in names another profile;
  Cargo's per-profile subdirectories keep profiles apart inside one slot.
- **Slot choice.** This checkout's own slot with the same key first. Without
  one, the most recently used idle slot with the same key whose last checkout
  is another live checkout of the repository, if its origin is trusted; the
  slot moves to this checkout and the [freshness gate](#cross-checkout-freshness)
  runs before Cargo. A slot detached by `oxide clean` is never shared. If the
  gate refuses, the slot goes back unchanged and the command takes a new or
  evicted slot cold.
- **Pool.** A budget-only `directory-budget-v1` pool at the path and soft
  budget of [user configuration](#budget-only-default-pool-and-user-configuration),
  created on the first building command. Creation refuses below 20 GiB host
  free space.
- **Slots and waiting.** Per-project cap 2: a new checkout or key at the cap
  evicts the project's least recently used idle slot whole and builds cold.
  Host-wide limit 2 commands holding slots (`OXIDE_MAX_ACTIVE`, 1–32). Excess
  commands wait in FIFO order, print one `oxide: waiting for …` line on stderr
  and cancel on Ctrl-C with exit 130. Another command in the same checkout
  and key waits too.
- **Execution.** Cargo runs from the caller's directory as leader of a new
  process group, with `CARGO_BUILD_BUILD_DIR` on the slot and
  `CARGO_TARGET_DIR=<workspace>/target`. SIGINT, SIGTERM and SIGHUP are
  forwarded to the group; Oxide releases only after the group is empty. `test`
  holds the slot through execution. `run` resolves the binary as `cargo run`
  does: `--bin` / `--example`; else, over the `-p` package or the workspace
  default members for the current directory (the current package, a virtual
  root's `default-members`, or every member), the single `default-run`, else
  the only binary. Several binaries refuse with Cargo's error and exit 101
  before any lease. `run` builds only that target, releases, then replaces
  Oxide with the program. It sets `CARGO_MANIFEST_DIR` only; other `cargo run`
  environment (`CARGO_PKG_*`, dylib paths) is not set. Exit codes are Cargo's,
  or the program's for `run`.
- **Summary.** One stderr line per leased command: profile, strategy (`build`
  or `build+execute`), slot, warm/cold, elapsed time, pool use and backend:
  `pool 12 MiB allocated of 95367 MiB budget (directory-budget-v1, budget-only)`
  (slot allocation against this command's soft budget).
  When the gate refreshed files, or reconciled another checkout's build, it
  appends the count and the outside-Git limit. A refused gate prints its
  reason on its own line before the cold build.
- **`clean`.** Waits until no live or interrupted-but-running command holds a
  slot for this checkout, then blocks new commands in the checkout while it
  runs `cargo clean` against the private target directory and an empty
  private build directory. It then drops the checkout's idle affinity. Slots
  stay intact until evicted, so the next build is cold. Ctrl-C cancels the
  wait. `cargo clean` runs as a recorded process group and inherits the clean
  lock, so if `oxide clean` dies the barrier stays until that lock is free and
  the group is empty.
- **Crash recovery.** Per [Crash recovery](#crash-recovery), for Oxide's own
  commands only. A follower in the same checkout waits while the crashed
  command's group survives, then recovers. If the crashed command's Cargo had
  started, the recovered slot has no trusted build record, so the follower
  evicts it and builds cold; if Cargo never started, the follower reuses it
  through the freshness gate. The fixture proof
  covers: a killed holder whose group survives stays held; a killed holder
  whose whole group is gone is recovered by the next command without waiting
  and logged; clean exit and Ctrl-C release without recovery; a standalone
  CLI lease stays held through every sweep; a `setsid` escapee does not keep
  the slot held.
- **`init` / `new`.** As in [Adoption](#adoption-oxide-init). Cargo runs as
  an ordinary child with inherited output; a Cargo failure returns Cargo's
  exit and adopts nothing. A refusal exits 1. The covering proof runs before
  any pool exists and checks that adoption never creates one.
- **`status`.** Pool path or "not created" with the backend and cap a
  building command would create; the backend and what bounds it; for APFS
  the cap and payload use, for a directory pool the budget, slot allocation
  and the watchdog trigger with the sampled pool-filesystem available bytes;
  slot accounting coverage, each slot's state, profile, checkout (an
  interrupted slot names the dead holder's PID and Cargo group) and size:
  allocation counted once per inode, file names and distinct inodes, or
  `partial` past a walk limit; waiting commands, checkouts being cleaned, the
  newest automatic recoveries, whether the current workspace's `target/`
  holds plain-Cargo `deps/` or `incremental/`, and the Cargo home (see
  [Cargo home hygiene](#cargo-home-hygiene)). It never creates the pool.
  `status --launch-check` prints only the [launch check](#launch-check);
  plain `status` does not run it. Status has no JSON output.

Control state lives in `<pool>/oxide-managed/` (owner-only): records mapping
each checkout and key to a private lease project directory, FIFO wait
tickets, an admission lock, one lock per record, each record's holder and
lease ID, the recovery log, and `origins/<record>.json` with each slot's
compile origin (at most 8 MiB, stamped with the record's use counter).
`state.json` gains no field for the gate. New fields default when absent, so control
state from the previous build opens without migration; an older build
refuses state that has them. The lease ledger, receipt and standalone CLI
contract are unchanged; see
[local lease](local-lease.md#oxide-front-door-control-directory).

## Clean

Implemented in oxide#095 under Tom's
[2026-10-19 ruling](../vision.md#product-workflow-direction), after a live
`oxide clean` removed reviewed binaries under `target/oxide-handover/` that
its dry run had not listed: Cargo 1.97.1's `clean --dry-run` prints paths
only under `--verbose`, and a whole-target clean removes every entry.

- **Whole-target clean** (no `-p`, `--workspace`, `--doc`, `--profile` or
  `--release`). Oxide plans and removes `<workspace>/target` itself; Cargo's
  `clean` is not run. It accepts `--dry-run`, `--include-foreign`,
  `--manifest-path`, `-q`, `-v`, `--color`, `--locked`, `--frozen` and
  `--offline`; any other argument refuses.
- **Plan.** One enumeration of the target and everything under it, parents
  first. Symlinks are listed as entries and never followed; another
  filesystem under the target, an unreadable directory, more than 1,000,000
  entries or depth 256 refuse rather than truncate. A symlinked target
  refuses. So does the Oxide pool inside the target, checked by path, by
  real path (an outside `OXIDE_POOL` symlink can name a pool inside) and by
  the pool directory's device and inode during enumeration; selected cleans
  get the same path checks. oxide#095 review found an outside symlink
  passing the path-only check, so `--include-foreign` deleted the pool.
- **Ownership.** Each top-level entry is classified against Cargo's target
  layout at the pinned commit `c980f4866141969fab6254a680546a277789d6f0`:
  - files [`CACHEDIR.TAG`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/crates/cargo-util/src/paths.rs#L799-L802),
    [`.rustc_info.json`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/util/context/mod.rs#L511),
    [`.future-incompat-report.json`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/future_incompat.rs#L98)
    and [`.rustdoc_fingerprint.json`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/rustdoc.rs#L192-L199);
  - directories [`doc`, `cargo-timings`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/layout.rs#L312-L313),
    [`tmp`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/layout.rs#L329),
    [`package`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_package/mod.rs#L220)
    and [`.metabuild`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/manifest.rs#L740);
  - a profile directory, whatever its name, only when it holds a regular-file
    [`.cargo-lock` or `.cargo-build-lock`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/layout.rs#L262-L291),
    which Cargo creates in every profile directory it uses (not on NFS,
    where Oxide therefore refuses);
  - a target-triple directory ([pushed onto both roots](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/layout.rs#L243-L246))
    only when it holds at least one such profile directory and otherwise
    only `CACHEDIR.TAG`, `.rustdoc_fingerprint.json`, `doc`,
    `cargo-timings` or `tmp`.
  Anything else, including a symlink or a plausible name without Cargo's
  lock, is foreign. Contents below the top level are not classified.
- **Foreign entries refuse** by default, naming them (first 20), in the dry
  run and the real clean alike, before any wait, deletion or affinity change.
  `--include-foreign` adds them to the plan; it authorizes only the
  enumerated tree, never a symlink's referent.
- **`--dry-run`** prints every planned path on stdout, the target included,
  and a summary on stderr. It changes nothing, takes no clean barrier and
  leaves affinity alone.
- **Real clean.** Waits for active commands in the checkout as before, plans
  again under the clean barrier (a refusal there ends the clean without
  touching affinity), then removes exactly the planned paths, children
  first: files and symlinks are unlinked, directories removed only when
  empty. A path that changed type or gained unplanned content is left,
  listed, and the command fails with affinity kept. On success it drops
  this checkout's slot affinity and prints the counts and any included
  foreign entries.
- **Selected clean** (`-p`, `--workspace`, `--doc`, `--profile`,
  `--release`) is Cargo's, as before: private target, empty private build
  directory, affinity dropped. Cargo removes only its own named artifacts
  there. `--dry-run` with a selection refuses, since Cargo's selected dry run
  does not print every path; so does `--include-foreign`.
- **Proof.** `effigy rust:oxide-cli:clean-safety` (below).

## Cargo home hygiene

Implemented in oxide#085 under Tom's
[2026-10-18 approval](../vision.md#product-workflow-direction), from the
[cleanup-trigger findings](../rust-dx-landscape.md#cargo-home-cleanup-triggers-2026-10-18-probe).
Agents and proofs that always run `--offline` or `--frozen` keep Cargo from
cleaning its home. Oxide reports the home and offers one explicit request;
it never deletes there itself.

- **Resolution.** As Cargo: `CARGO_HOME` when set and non-empty (relative
  to the current directory), else `$HOME/.cargo`. Both unset refuses.
- **`status`.** One `cargo home:` line: allocation counted once per inode,
  file names and distinct inodes, from one bounded walk (200,000 entries,
  depth 64, 10 s; `partial` past a limit). The root must be a real directory;
  symlinks are not followed and mounts are not crossed. A second line states
  Cargo's skip conditions as facts. No last-cleanup time is shown: Cargo's
  only record is also set when its database is created, and reading it
  means opening Cargo's internal database. No usage history is inferred.
- **`oxide clean --cargo-cache`.** Takes no other arguments. Runs
  `cargo fetch --locked --config 'cache.auto-clean-frequency="always"'
  --config 'http.proxy="http://127.0.0.1:<port>"'` from the current
  directory, with `CARGO_LOG=gc=trace,cargo::core::gc=trace` and the libcurl
  proxy variables (`http_proxy`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and
  their other spellings) removed, since `no_proxy` would bypass a forced
  proxy. `--config` is Cargo's highest-precedence form. The port is Oxide's
  own loopback listener: it accepts and closes every connection and
  forwards nothing. A merely bound port is not enough: on macOS a connection
  to it hangs until timeout. Oxide prints how many connections it closed.
  Cargo's log lines are read as evidence and not echoed; other output passes
  through.
- **Refusals before Cargo runs**, each naming its reason:
  - environment: `CARGO_NET_OFFLINE` (disables cleanup),
    `CARGO_NET_GIT_FETCH_WITH_CLI`, any other `CARGO_NET_*` but
    `CARGO_NET_RETRY`, `CARGO_REGISTRIES_*`, `CARGO_REGISTRY_*` but
    `CARGO_REGISTRY_TOKEN`, `CARGO_SOURCE_*`, `CARGO_UNSTABLE_*`. The
    refusal names the variable, never its value, since a refused variable
    can hold a registry token;
  - a Cargo config file Cargo would read (`.cargo/config.toml` or
    `.cargo/config` in the current directory or an ancestor, or
    `config.toml` or `config` in the Cargo home) that is unreadable, larger
    than 1 MiB, not UTF-8, or not parseable as TOML 1.0 by the exact-pinned
    `basic-toml` 0.1.10 (oxide#095), which also refuses TOML 1.1-only syntax
    and datetimes. Each file is parsed, so quoted, dotted and nested
    spellings of a key are one key. The refusal names the file and, for a
    parse failure, the line; parser text, which can quote a value, is never
    shown;
  - any parsed key outside a fixed contained set, named by key, never by
    value: `source`, `registries`, `registry.credential-provider`,
    `registry.global-credential-providers`, `credential-alias`,
    `registry.default`, `net.git-fetch-with-cli`, `net.offline`, `net.ssh`,
    `include`, `patch`, `paths`, `unstable`, the program settings
    `build.rustc`, `build.rustc-wrapper`, `build.rustc-workspace-wrapper`
    and `build.rustdoc`, and every unknown key. Contained, so a config of
    only these (or comments) proceeds: the tables `alias`, `term`,
    `cargo-new`, `doc`, `install`, `future-incompat-report`, `profile`,
    `target`, `env` and `resolver`; `net.retry`; `registry.token`;
    `cache.auto-clean-frequency` and `http.proxy`, which Oxide's own
    `--config` overrides; the `http` keys `timeout`, `low-speed-limit`,
    `multiplexing`, `debug`, `user-agent`, `check-revoke`, `ssl-version`,
    `cainfo` and `proxy-cainfo`; and the `build` keys `jobs`, `target`,
    `target-dir`, `build-dir`, `rustflags`, `rustdocflags`, `incremental`,
    `dep-info-basedir`, `pipelining` and `warnings`. The environment screen
    above does not cover `RUSTC_WRAPPER` or `RUSTC`;
  - a Cargo other than 1.97.1 (`cargo -V`), whose trigger and trace text
    were qualified;
  - a workspace without the Oxide `AGENTS.md` block;
  - a missing `Cargo.lock`, or one over 16 MiB;
  - any lock source other than crates.io (`registry+https://github.com/rust-lang/crates.io-index`
    or `sparse+https://index.crates.io/`) or a path: SSH and other non-HTTP
    git transports, HTTPS git (not qualified), custom registries and unknown
    kinds. The scan reads `source` values in either quote style and refuses
    any other spelling (escapes, multi-line strings). It is a screen only;
  - Cargo's own resolution failing offline: `cargo metadata --format-version 1
    --offline --locked` in the fetch's directory and environment must
    succeed, and every package it resolves must come from crates.io or a
    path. That fails when the lock is stale, when a manifest names a
    dependency the lock lacks (a stale lock could otherwise make the online
    resolve fetch an SSH git source directly, outside the proxy), or when any
    source, index entry or package is not already local. The online fetch
    then has nothing to resolve or download that this resolve did not name.
    Debug builds accept `OXIDE_TEST_SKIP_OFFLINE_RESOLUTION` so the fixture
    proof can send one uncached fetch to the closed proxy; release builds
    always run the gate.
  The proxy is one routing control, not universal containment; these
  refusals cover the other routes. Oxide never falls back to `--offline`,
  which would disable the requested cleanup.
- **One outcome**, from Cargo's own evidence:
  - *ran* (exit 0): the trace shows `last auto gc was …, running`, with no
    `failed to auto-clean cache data` and fetch exit 0;
  - *skipped by Cargo* (exit 75): exactly one skip decision, naming it: the
    cache lock held by another Cargo (`unable to acquire mutate lock`), the
    frequency gate (with `always`, the last pass or database creation was
    this same second), offline, or cleanup disabled;
  - *failed* (Cargo's code, else 1): Cargo's cleanup error, a non-zero fetch
    (for example an uncached crate the closed proxy denied), or exit 0 with
    zero or several decisions in the trace, which is unknown, not success.
  Before and after, it prints the home's allocation from the same bounded
  walk. A difference is not proof of what the cleanup deleted: a completed
  pass can delete nothing, and other writers may change the home. No freed
  space or SSD claim is made.
- **Proof.** `effigy rust:oxide-cli` runs these on one fresh private
  `CARGO_HOME` (0700, under the proof scratch), never the live home. Setup
  downloads one archive, `cfg-if` 1.0.0 (7,934 bytes, sha256
  `baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd`, the
  lockfile checksum), plus its sparse-index metadata. Then: the `status`
  line equals the proof's own walk; refusals for a missing lock, an SSH git
  source (double- and single-quoted), `CARGO_NET_GIT_FETCH_WITH_CLI`, a
  config file setting source replacement, a custom registry and a stale lock whose
  manifest adds an SSH git dependency; a cleanup on the complete cached lock that ran with
  0 proxy connections; Cargo's lock skip under a shared hold on
  `.package-cache-mutate`; and an uncached crate whose fetch failed through
  the closed proxy with the offline gate skipped through the debug hook (at
  least one connection closed) while an owned control
  listener named by `CARGO_HTTP_PROXY` and the libcurl proxy variables saw
  none. oxide#085 review found the stale-lock and single-quoted routes
  reaching an SSH listener directly; the offline gate and quote handling
  close them, covered by Rust tests. The focused proof's new cases were not
  rerun: the task's two proof attempts and two archive downloads were
  spent. Nothing was age-eligible, so no deletion was observed; deletion rests
  on Cargo's source. The same proof covers `OXIDE_POOL_CAP` (an invalid
  value refused before creation, 64 MiB used on a new directory pool, a
  conflicting value reported and ignored there and on the APFS pool) and a
  slot whose build script hard-links a 64 KiB file: `status` matched the
  proof's walk, with the pair counted once.

- **Clean-safety proof.** `effigy rust:oxide-cli:clean-safety` (oxide#095)
  stages `oxide` offline with 1.97.1 and needs no pool, image or download.
  `cargo` on its `PATH` is a recording wrapper that refuses `cargo clean`.
  A tiny std-only crate gets a real Cargo target (`cargo build --offline
  --target <host>`, private empty `CARGO_HOME`). On it: the Cargo-only dry
  run lists exactly the tree and changes nothing, and the clean removes it;
  then, with a nested foreign directory, a foreign file and a symlink to an
  outside sentinel added, the dry run and the clean both refuse naming them
  with nothing changed; the `--include-foreign` dry run lists exactly the
  tree and the clean removes it, sentinel intact; selected-clean
  `--dry-run`/`--include-foreign` and `--target` refuse; with `OXIDE_POOL`
  an outside symlink to a pool directory inside the target, the
  `--include-foreign` clean, its dry run and a `--release` clean all refuse
  with the slot sentinel and tree unchanged. A fresh private
  `CARGO_HOME` is seeded by copying `cfg-if` 1.0.0 and its sparse-index
  entry from the host cache (checksum checked, nothing fetched); there each
  risky key refuses by name (`source`, `registries`, both credential-provider
  keys, quoted-dotted `net.git-fetch-with-cli`, `include`; one in the Cargo
  home, a `build.rustc-wrapper` in an ancestor's `.cargo/config`), with no
  config value in any diagnostic; a malformed file and a TOML 1.1 inline
  table refuse as unparseable; missing lock, SSH lock source, stale lock and
  `CARGO_NET_GIT_FETCH_WITH_CLI` still refuse; and a comments-only project
  config with a `[build]`/`[term]` home config proceeds to Cargo with zero
  proxy connections and a labelled outcome. Its first run (2026-10-19, 25 s
  after admission including cleanup, 72 MiB owned, host free above
  754,000 MiB at every sample) passed; Cargo skipped that pass on its
  frequency gate (exit 75) because the fresh home's database was created the
  same second, so no deletion was observed. The wrapper saw 24 Cargo calls:
  17 `locate-project`, 4 `-V`, 2 `metadata`, 1 `fetch`, no `clean`. The
  second and last run, after the pool fix and with the aliased-pool case
  added, passed the same way (20 s, 73 MiB owned, host free above
  752,000 MiB, the same frequency-gate skip; 27 calls: 20 `locate-project`,
  4 `-V`, 2 `metadata`, 1 `fetch`, no `clean`).

## Launch check

Implemented in oxide#093 under Tom's
[2026-10-19 ruling](../vision.md#product-workflow-direction), from the
[launch-cost finding](../rust-dx-landscape.md#macos-new-executable-launch-cost-2026-10-19).
`oxide status --launch-check` measures how much slower a newly written
executable launches than a re-run of the same file. It is opt-in and
advisory, exits 0 whenever it prints a result (an oversized binary refuses
with exit 1), and reads or changes no setting.

- **Off macOS.** One line: not applicable.
- **Probe and helper.** Two hidden modes run as the first thing `main`
  does, before argument parsing, toolchain, pool or lease work, and are
  absent from usage. `oxide __oxide-launch-probe`, exactly that one
  argument, exits 0 and prints and starts nothing.
  `oxide __oxide-launch-measure` is the measuring helper the check starts;
  it takes its directory, deadlines, delays and source as arguments.
- **Measurement.** The check refuses a binary over 64 MiB, creates a fresh
  owner-only directory under the temporary directory, and starts the helper
  (the running `oxide`, with an empty environment) as the leader of a new
  process group. The helper copies `oxide` in 1 MiB chunks into new
  owner-only files (`create_new`). It runs one copy once as warm-up and five
  more times; then five distinct fresh copies are each written, run once and
  unlinked. At most one copy exists at a time. Each timing spans spawn to
  reap of a probe with an empty environment and null standard streams,
  polled every 250 µs; copy time is not timed. The helper reports samples
  on a pipe, removes its own copies and directory, and stops at its
  deadline.
- **Bounds.** One 3 s monotonic limit covers copies, starts, waits and
  cleanup:
  - A sample counts only if its probe exited successfully by 2.75 s. At
    2.75 s the check stops collecting.
  - The check then kills the whole helper group with `SIGKILL`: the helper
    and any probe, including one still starting. The group ID is signalled
    only before the helper is reaped, so it names only this check's
    processes. The check reaps the helper and waits until the group is
    empty.
  - With every writer gone, the check removes the helper's known copy names
    and the directory itself.
  - Nothing it started outlives it. The one exception is the kernel holding
    a killed process past 3 s; the report then names the process group as
    not confirmed closed. A removal failure is named too.
  - Elapsed time over 3 s, or an unconfirmed group, makes the result
    inconclusive.
  - If the check itself is killed, the helper still stops at its own
    deadline and removes its copies; only the empty directory can remain.
- **Result.** Raw samples always print (warm-up, five same-file runs, five
  fresh-copy runs), then the elapsed time and cleanup. With every sample, it
  prints both medians and their signed difference (negative and small
  differences as measured). A timeout, failed probe, copy failure or
  incomplete set is `inconclusive` with the reason and no median; at the
  limit it also says to run it again, since a newly built `oxide`'s first
  launches can take longer. Copies share content and code signature, so the
  result is likely a lower bound for a freshly linked binary
  ([finding](../rust-dx-landscape.md#oxide-launch-check-runs-2026-10-19)).
- **Advice.** Only when the difference exceeds 30 ms. It states the cause is
  inferred, not measured, and names a best-effort launching app: the
  outermost `.app` bundle on the executable paths of this process's
  ancestors (`proc_pidpath`, same-user readable only, at most 64 hops). No
  bundle, or several distinct ones, is `unknown` with manual guidance. It
  points to System Settings → Privacy & Security → Developer Tools, says the
  app may need a restart, says Oxide has not checked or changed any setting
  and listing an app may not remove the penalty, and warns never to disable
  Gatekeeper globally. The ancestry is not proof of which process macOS
  holds responsible.
- **Never.** No Cargo, pool, network, `spctl`, privileged call, TCC read,
  settings change or restart.
- **Proof.** `effigy rust:oxide-cli:launch-check` is the focused route. It
  stages `oxide` offline with 1.97.1 into its own scratch (counted in the
  512 MiB), from the host's cached registry, with no pool, image, Cargo home
  setup or download. It logs its setup (two version calls, at most 30 s),
  samples the 20 GiB reserve before writing anything and after staging and
  the check, and stays within 300 s including cleanup. `effigy
  rust:oxide-cli` runs the same case before any pool exists. The case puts
  a `cargo` that records any call and exits 97 first on `PATH`. A
  positive control shows that shim records `oxide version`. The hidden
  probe exits 0 with no output and no Cargo call. `status --launch-check`
  exits 0 within its bound (reported at most 3,000 ms, wall at most
  4,000 ms), leaves nothing new in the temporary directory, starts no Cargo,
  creates no pool and prints well-formed output: medians recomputed from the
  raw samples, the advice lines when advised. No penalty size is asserted.
  Unit tests cover the medians, the strict threshold, the bundle choice and
  the report and advice text.
- **Lifecycle tests.** `tests/launch_check.rs` runs the built `oxide` to
  exit. Its probes are shell scripts named by `OXIDE_TEST_LAUNCH_SOURCE`
  (test-only, like `OXIDE_TEST_LAUNCH_LIMIT_MS` and
  `OXIDE_TEST_LAUNCH_DELAY=copy|start|remove:<ms>`, which pauses inside the
  helper). In debug builds `OXIDE_TEST_LAUNCH_GROUP_LOG` names a fresh file
  that receives the measuring helper's process-group ID once it is spawned;
  it changes nothing else. With the CLI gone, each case checks at once that
  the temporary directory is empty, that recorded group is gone
  (`kill(-group, 0)` fails with exactly `ESRCH`; a missing record fails) and
  no logged probe is alive:
  - an ordinary run (20 s limit) reports either complete medians with a result, or exactly that deadline as inconclusive with no median, within the limit and cleaned up; any other outcome fails;
  - under a 5 s limit, a slow probe's group is closed by the deadline
    (whether the script reached its PID marker under host load is separate;
    every PID it logged must be dead);
  - under a 600 ms limit, a copy, a probe start and a removal each paused
    2 s return within 1.6 s, inconclusive.

## Budget-only default pool and user configuration

Tom's 2026-10-20 rulings ([vision](../vision.md#product-workflow-direction)):
a hard cap on drive space "would just break large builds and cause MORE
churn"; retire the disk image and the standalone lease tool; a 100 GB
internal budget; make the pool's location and size configurable. Shipped in
oxide#102.

- **Backend.** Every internal pool is `directory-budget-v1`
  ([as shipped](#directory-budget-v1-as-shipped)). Oxide no longer creates,
  attaches, verifies or detaches APFS sparse images, and the standalone
  `oxide-storage-lease` binary and its selector are gone. The library's
  ledger, leases, crash recovery, eviction and inventory remain; `oxide`
  is their only caller.
- **Soft budget.** Default 100 GB, decimal (100000000000 bytes). It is
  checked at admission: owned slots' unique allocation against the budget,
  whole-idle-slot eviction until under it, `Full` when only held slots
  remain. It is not a quota, a sparse-file size, a mid-build threshold or a
  promise to reserve that much disk. A running build may grow past it; the
  next admission evicts whole idle slots. Oxide never deletes active or
  uncertain slots or files to meet it. A changed budget takes effect at the
  next admission; the pool's recorded creation budget is unchanged and
  `status` shows the effective one. Builds can still run out of real space:
  the 20 GiB host-reserve watchdog stops the owned group below its 22 GiB
  trigger, and ordinary filesystem errors still fail a build.
- **Accounting walks (oxide#108).** Admission, reuse, eviction and `status`
  walk the slots with one set of pool bounds: 2,000,000 entries, depth 64,
  60 s (previously 50,000, 24 and 5 s, which refused Praxis's 76,669-entry
  and Farmyard's 60,657-entry pools). Reusing an idle slot needs that slot's
  own walk complete; a new slot needs the whole-payload walk complete before
  and after eviction. An incomplete walk refuses with `unknown_accounting`
  and names its cause (`entry_limit`, `depth_limit`, `elapsed_deadline`,
  `unreadable_entry`, `missing_metadata`, a mount boundary or a root
  failure), entries visited, elapsed time and bounds; its bytes are never
  used and never make a slot evictable. The bounds are finite: a larger pool
  refuses until idle slots are evicted or cleaned. Owning detail:
  [local lease](local-lease.md#capacity-and-eviction). Rust tests cover
  each old bound, an unreadable entry, a symlinked slot root and hard links
  with injected bounds. The native selector `effigy rust:pool-scale` has no
  passing run: its first session staged the reference binary twice and is
  apparatus evidence only, and the corrected selector is held because no
  existing guard samples the host reserve during plain-Cargo staging.
- **Precedence.** Environment, then the user config file, then defaults.
  `OXIDE_POOL` sets the path (relative resolves against the current
  directory); `OXIDE_POOL_CAP` sets the budget. Sizes are whole bytes or a
  whole number of `KB`, `MB`, `GB`, `TB` (decimal) or `KiB`, `MiB`, `GiB`,
  `TiB`, from 1 MiB to 16 TiB; anything else refuses before any effect.
  `OXIDE_POOL_BACKEND` is retired: unset or `directory-budget` only.
- **Config file.** Machine configuration, never a project file. Platform
  location (Longhorn platform-native-v1 policy, mirrored, no dependency):
  macOS `~/Library/Application Support/oxide/config/config.toml`; elsewhere
  `$XDG_CONFIG_HOME/oxide/config.toml`, with XDG's `~/.config` when the
  variable is unset; a relative `XDG_CONFIG_HOME` or a missing `HOME`
  refuses. `OXIDE_CONFIG` (absolute) selects another file; only the selected
  file is parsed. On macOS `~/.config/oxide/config.toml` is never read;
  `status` says it is ignored, without printing its contents. Parsed with
  `basic-toml` 0.1.10; at most 64 KiB:

  ```toml
  [pool]
  path = "~/Dev/oxide"   # ~ or ~/ expands to HOME; absolute, no . or ..
  budget = "100GB"       # or a byte count
  ```

  An unknown key, a wrong type or a parse failure refuses with the file
  named.
- **Default path.** `~/Library/Caches/oxide/build-pool` on macOS,
  `$XDG_CACHE_HOME/oxide/build-pool` or `~/.cache/oxide/build-pool`
  elsewhere: distinct from the retired image pool's `…/oxide/pool`, so that
  pool is retired separately. The control root (ledger, leases, Oxide state,
  removable registration) lives inside the pool, so a configured pool keeps
  its control root with it.
- **Path refusals.** A pool path (from the environment or the file) with a
  `.` or `..` component refuses outright, so a missing component followed
  by `..` cannot reach a work tree once its parents exist. Then, after
  resolving the nearest existing ancestor's symlinks: a path inside a Git
  work tree (any ancestor holding `.git`), or
  under `/Volumes` or on a volume mounted there (where a removable volume is
  registered, aliases included), refuses. Unknown identity refuses.
- **Legacy image pool.** A pool path holding `image.sparseimage` or
  `receipt.json`, or whose config records `apfs-sparse-image-v1`, refuses
  with a retirement message and is never locked, attached, mounted,
  recovered, cleaned or written. A symlink alias is judged by the directory
  it names, and the library repeats the check read-only before it creates
  or takes the pool lock. `status` reads only file names and the
  config's backend at the legacy default path and reports `legacy pool: …
  pending planner retirement`. No conversion, copying or deletion.
- **Path changes.** Nothing moves or deletes an old pool. `status` lists
  pools at other known paths as `orphaned pool`: the built-in default and
  pools Oxide created from the config file or default, recorded (newest 16)
  in the platform state directory (`~/Library/Application Support/oxide/state/pools.json`
  on macOS, `$XDG_STATE_HOME/oxide/pools.json` elsewhere). Pools named by
  `OXIDE_POOL` are not recorded. The record is informational, never
  ownership or reclaim proof; there is no home or drive scan.
- **Previous removable registration.** A registration or removable pool
  state under another known pool's control root (the legacy default, the
  built-in default or a recorded pool) is never moved, adopted, forked or
  removed. `oxide pool register-removable` refuses while it exists, naming
  the safe prerequisites: confirm with the build that used that pool that
  its removable pool has no held slot or waiting command, then retire the
  old registration as a separate step. A build prints that the previous
  registration is not used and continues on the internal pool.
  `register-removable` also refuses while the volume's `oxide-pool`
  directory is not empty, so an old registration's slots are never adopted.
  The real `Builds` registration ([below](#removable-build-volume)) lives
  under the legacy default pool's control root, so after this change it is
  not used, and re-registering refuses until the planner retires it and its
  `oxide-pool` payload directory after fresh positive idleness, then
  registers again through the shipped command.
- **Time Machine.** On macOS a pool Oxide creates is excluded with the
  sticky, unprivileged `tmutil addexclusion <pool>` and verified with
  `tmutil isexcluded` before any build writes; failure or an unknown result
  refuses before Cargo. Each later command checks the exclusion attribute
  (`com.apple.metadata:com_apple_backup_excludeItem`) on the pool root and
  re-runs that pair when it is missing. Only the owned pool root is
  excluded, never a parent such as `~/Dev`. `status` reports
  `tmutil isexcluded`. Other platforms: not applicable.
- **Spotlight.** Apple documents only the Spotlight Privacy list, which needs
  the user, so Oxide applies no exclusion. `status` says `not excluded by
  Oxide` with that advice. Other platforms: not applicable.
- **`status`.** The config file and whether it exists or was selected by
  `OXIDE_CONFIG`, the pool path and budget with their sources, then per
  pool: backend, soft budget, slot allocation, the pool filesystem's
  available and total space (shared, no Oxide quota or ceiling), Time Machine
  and Spotlight state, legacy and orphaned pools. `slot accounting walk:`
  reports the payload walk's coverage, causes, entries and milliseconds; a
  slot whose own walk is incomplete gets its own line saying it is not
  reused. An incomplete payload walk shows allocation as `at least`.
- **Explicit slot release.** `oxide pool recover-interrupted <slot>`
  replaces the retired CLI's `recover` for an interrupted slot automatic
  [crash recovery](#crash-recovery) keeps held. Source- and unit-reviewed;
  no native proof drives it.
- **Proof.** `effigy rust:oxide-budget-default` runs the
  [removable proof](#removable-build-volume)'s guarded session with these
  cases first, over a scratch `HOME` (toolchains from the real
  `RUSTUP_HOME`): status before any pool; the legacy image pool shape
  detected, refused by path and byte-identical; `OXIDE_POOL_BACKEND`
  refusal; default creation at the new path as a 100 GB directory pool with
  `tmutil isexcluded` confirmed; the ignored `~/.config` file, Time Machine,
  Spotlight and filesystem lines; config-file, `OXIDE_POOL_CAP` and
  `OXIDE_CONFIG` precedence; the orphan report; Git work tree, symlink
  alias, `/Volumes`, unknown key, malformed and relative-path refusals that
  create nothing; a build script writing 4 MiB past a 2 MB budget
  completing, then the next admission evicting that whole idle slot; an
  injected low reading stopping a sleeping build. Before registering, the
  removable phase proves the previous-registration refusal with idle and
  held fixtures (bytes unchanged, no forked ledger, nothing on the volume)
  and the build note. The selector shares `rust:oxide-removable-pool`'s
  unqualified runner.
- **Suspended proofs.** `rust:oxide-cli`, `rust:cargo-matrix:<release>` and
  `rust:oxide-toolchain-context` provisioned their pool with the retired CLI
  and image backend. They refuse before any effect until migrated to a
  directory pool; their earlier results remain historical evidence.
  `rust:oxide-cli:launch-check` and `rust:oxide-cli:clean-safety` use no
  pool and still run.

## Platform backends

`directory-budget-v1` is implemented (see [below](#directory-budget-v1-as-shipped))
and is the only internal backend; `linux-image-v1`, CI cache save/restore and
container execution are proposed only. The APFS sparse image that gave macOS
an unprivileged hard ceiling is retired (oxide#102): a hard cap broke large
builds. Linux has no unprivileged
equivalent: loop mounts, XFS/ext4 project quotas and btrfs qgroup limits all
need root, and the filesystems an unprivileged user namespace may mount
(`tmpfs`, overlay, FUSE) are RAM-backed or slow. The default must still need
no setup, per Tom's ease-of-use ruling.

| Backend | Where | Bound | Setup |
| --- | --- | --- | --- |
| `apfs-sparse-image-v1` (retired, oxide#102) | macOS | Was an OS-enforced aggregate ceiling | Never created or opened now |
| `directory-budget-v1` | Every host, the default | Budget only: admission plus whole-idle-slot eviction against a soft budget, with no OS ceiling. A supervised build samples caller-available space through `statfs` and attempts to stop its own process group below the 22 GiB trigger (20 GiB protected reserve plus 2 GiB margin). Best effort, with no guarantee against faster writers or other processes; no directory walk during the build. | None |
| `linux-image-v1` | Long-lived Linux hosts (LAN build box) | OS-enforced ceiling on an ext4/XFS image or project-quota directory | One optional privileged provisioning step, then unprivileged use |

Rules:

- `status` and every summary line name the backend. A budget-only pool never
  claims a hard ceiling.
- The watchdog samples the pool filesystem about once per second. The 20 GiB
  reserve is the protected floor; the 2 GiB safety margin moves the trigger to
  22 GiB. The margin is intended to cover normal write rates at that interval,
  with no guarantee against faster writers or other processes. Reserve/margin
  overrides are test-only. The bounded proof records sampled minimum, stop
  latency and overshoot past the injected trigger; it never fills the host to
  its real reserve.
- A watchdog stop is an ordinary failed command. TERM and bounded-grace KILL
  apply only to the owned process group. Release follows positive normal group
  closure; unknown closure or surviving writers leave the lease held. Cargo can
  reuse an interrupted slot after that positive closure.
- CI runners are ephemeral: `directory-budget-v1` on the runner disk. A
  persistent cache across runs means saving and restoring whole idle slots
  through the CI cache, keyed like slots. That is a later executor feature.
- Colima and other containers: run Oxide inside the Linux VM or container with
  its own pool on the VM disk. Never share the host APFS pool over virtiofs
  or sshfs: `flock`, mtime and capacity behaviour across that boundary are
  unqualified.
- A remote executor uses the remote host's own pool. Pools are never shared
  across hosts.

### `directory-budget-v1` as shipped

- **Shape.** `<pool>/payload/slots/` is a plain owner-only directory on the
  pool's own device; config, ledger and lock sit beside it as for APFS, with
  no image or receipt. The config records the backend, store ID and budget
  recorded at creation (`cap_bytes`, default 100 GB; `image_bytes` 0). An
  image or receipt file, any payload entry but `slots`, or a payload on
  another device refuses.
- **Admission.** The ledger's existing rules with this command's soft budget
  (`LeaseRequest.budget_bytes`, else the recorded one) as the cap:
  owned slots' unique allocated bytes at lease time, whole-idle-slot
  eviction until under budget, `Full` when only held slots remain, and
  half-budget idle recycling. Nothing enforces the budget while a build
  writes; a build can pass it, and the next new slot evicts.
- **Watchdog.** Only on a directory pool, only while Oxide supervises a
  leased Cargo group (build and `run` build, `test` execution, building
  passthrough). It samples `statfs` `f_bavail` of the payload directory
  (`f_bsize` on macOS, `f_frsize` elsewhere) at the first poll and then
  once a second, with no slot walk. Below 22 GiB, or when a sample fails, it
  sends TERM to the group, then KILL after 2 s, and keeps sampling on every
  20 ms poll so the minimum is recorded. It prints one `oxide: reserve stop`
  line: the triggering sample, trigger, signals and KILL delay, sampled
  minimum, overshoot past the trigger and stop latency (triggering sample to
  observed group closure). The command fails with Cargo's code, never 0. It
  samples only the pool filesystem; `<checkout>/target` elsewhere is not
  watched.
- **Closure.** After a stop the slot is released through the normal path
  only once `killpg(pgid, 0)` reports `ESRCH`. If the leader or the group
  outlives KILL by 10 s, Oxide prints that closure is unconfirmed and exits
  1 without releasing: the ledger lease stays held, the record keeps
  Cargo's group, and [crash recovery](#crash-recovery) releases it once the
  group is empty.
- **Test hooks.** Debug builds only: `OXIDE_TEST_WATCHDOG_READINGS` names a
  file holding the available-bytes reading, and `OXIDE_TEST_BUDGET_BYTES`
  overrides the soft budget above every other source. Release builds ignore
  both. The reserve and
  margin have no override.
- **Proof.** Current: `effigy rust:oxide-budget-default`
  ([above](#budget-only-default-pool-and-user-configuration)). Historical,
  before the CLI's retirement: `effigy rust:oxide-cli` added a macOS case on two scratch
  directory pools after the APFS image is detached: creation, summary and
  `status` labelling; standalone refusal; a reserve stop on injected
  readings where a bounded writer that ignores TERM lowers the reading by
  its own fsynced writes (1 MiB per ~50 ms, at most 48 MiB, starting 8 MiB
  above the trigger), with KILL, positive closure and normal release; an
  unreaped group member that keeps closure unconfirmed, so the slot stays
  held until it is reaped and the next command recovers it; and a 1 MiB
  budget whose idle slot is evicted whole for a new key. The proof prints the
  sampled minimum, overshoot and latency it observed. Readings are
  injected: this shows the stop and closure semantics and the overshoot of
  that one writer at the sampling interval, not margin adequacy for real
  write rates. The authoring run at the oxide#077 head observed: triggering
  sample 10 MiB below the trigger, sampled minimum 40 MiB below it (the
  writer reached its 48 MiB bound during the 2 s grace), KILL after 2017 ms,
  closure 2042 ms after the triggering sample; the held case confirmed no
  closure after KILL plus 10 s and was recovered once its member was
  reaped. Rust tests cover the real `statfs` sample, TERM-then-KILL on
  a group that ignores TERM, a failed sample, and budget admission and
  refusals in the library. `effigy rust:compile-linux` checks that the crate
  compiles for the installed `x86_64-unknown-linux-gnu` target; no Linux
  runtime behaviour is validated.

## Removable build volume

Approved design direction (Tom, 2026-10-20). The goal: while the laptop is docked, Oxide's build-data writes go to a dedicated external SSD, and undocking at any moment is safe, with work falling back to the internal SSD. Measured motivation: [internal SSD write baseline](../storage-efficiency.md#internal-ssd-write-baseline-2026-10-20).

Part 1 (registration, selection and flush on release) is implemented, with author fixture evidence and Tom's operator acceptance; the final independent native proof remains missing. Part 2 (surprise removal, lost-volume leases, discard after unclean disappearance and opt-in final outputs on the volume) is not implemented.

Tom approved an early host trial on 2026-10-20. His `Builds` volume (`A6AC3ED7-33EA-46B4-8082-7254CF291ED2`, `/Volumes/Builds`) is registered and selected for the next build when present. The whole-MiB parser records an admission cap of 715255 MiB (749999226880 bytes, rounded down from 750 GB) and a reserve of 95368 MiB (100000595968 bytes, rounded up to retain at least 100 GB free). These are a budget and start threshold, not an APFS quota. **Do not undock during builds until surprise-removal handling is implemented.** The current registration and control state live under the existing internal pool; a later internal-pool move must preserve them safely or require explicit re-registration. Installation identity and before/after observations are retained in the user's install receipts and release audit.

### Implemented: part 1

- **Backend `apfs-volume-v1`.** Config, ledger, lock and Oxide's own state live in `<internal pool>/oxide-managed/removable/`; the registration in `<internal pool>/oxide-managed/removable.json`. Only slot payload is on the volume, in `<mount>/oxide-pool/slots/`. No disk image on the volume. Budget-only: Oxide's admission cap is an estimate, not an APFS quota, and the volume's capacity is shared with every other directory on it. Only the front door's `managed` library entry points open it; the standalone lease CLI refuses it like a directory pool. No reserve watchdog runs on the volume: its reserve is checked when a build starts.
- **Registration.** `oxide pool register-removable <mount-point> --reserve <size> --cap <size>` (macOS). `pool` is not a Cargo built-in. Both sizes are required, from 1 MiB to 16 TiB in the [size units](#budget-only-default-pool-and-user-configuration) (oxide#102 dropped the whole-MiB rule and added decimal units, so `750000000000` is accepted); zero, overflow and malformed values refuse. Oxide never picks a reserve, cap or quota, and changes no APFS setting. Before any write it requires:
  - a path of exactly `/Volumes/<name>`: nested paths, `..`, hidden names, prefix look-alikes and custom mount parents refuse;
  - `/Volumes` passing the structural mount-parent rule, at registration and at every selection: the caller is not root; `/Volumes` is a real directory (not a symlink) whose path and opened descriptor agree; it is owned by root with no group or other write and no setuid, setgid or sticky bit; `acl_get_fd_np(ACL_TYPE_EXTENDED)` positively reports no extended ACL entry (`ENOENT`, or an ACL with no entry), any other result refuses; and it has no file flag except `UF_HIDDEN` (`sys/stat.h`, a GUI display hint the standard `/Volumes` carries). Every violation is reported. A denied combined `faccessat(W_OK)` is not used, since it does not prove that a specific right such as `add_subdirectory` is denied;
  - the name opened with `O_NOFOLLOW | O_DIRECTORY` (a symlink alias refuses), whose descriptor's `fstatfs` `f_mntonname` names exactly that path, whose `st_dev` differs from `/Volumes`'s, `f_fstypename` `apfs`, not `MNT_RDONLY`, not `MNT_ROOTFS`, and writable by the user;
  - `diskutil info -plist` agreeing on the volume UUID (the kernel's `ATTR_VOL_UUID`), mount point and APFS, with a container other than the one holding `/System/Volumes/Data`;
  - `diskutil apfs list -plist <container>` listing the volume with no APFS role: a Backup (Time Machine) role, and every other role, refuses.
  It records mount point, volume UUID, container UUID, reserve, cap, the volume's capacity and APFS quota (`CapacityQuota`, none when 0) at registration. A second registration refuses, and so does a registration left under a previous pool's control root ([oxide#102](#budget-only-default-pool-and-user-configuration)). The payload directory is created empty through the volume's descriptor.
- **Selection, when a leased command starts.** With nothing registered, the internal pool is used exactly as before. Otherwise the removable pool is used only if, at that moment, the mount point opens as above with the registered UUID, `/Volumes` still passes the mount-parent rule, the volume is writable, `diskutil` again reports it in the registered container, outside the boot/data container and with no APFS role (unknown metadata refuses), and its available space is above the reserve; the library then re-verifies the volume and the payload directory (same device and inode by path and by descriptor) on every open. Anything else falls back to the internal pool with one `oxide: internal pool: removable volume not used (<reason>)` line. Selection creates nothing, so a stale `/Volumes/<name>` is never written to.
- **No stale-mount-point writes by Oxide.** Oxide creates slot directories with `mkdirat` relative to descriptors opened from the verified mount root, rechecking the binding (descriptor still the registered root, path still that root's device) before each creation and before removing a slot. Cargo itself writes through the slot path; while the volume is mounted that path is the volume. A mid-build removal is part 2.
- **Flush on release.** Before a removable slot becomes idle (normal release, a refused shared slot handed back, an untrusted slot released for eviction, and crash recovery), Oxide calls `fsync_volume_np(fd, SYNC_VOLUME_FULLSYNC | SYNC_VOLUME_WAIT)` on the verified mount-root descriptor, verifying the binding before and after. The summary line prints its cost; `status` lists recent flushes, including those made during crash recovery. The flush runs after positive writer closure and is a durability step, not a closure proof. A failed or unknown flush retires the slot: the record becomes `retired`, the lease stays held, the slot is never reused, recovered or deleted, and the build's own exit code is unchanged.
- **Pools and limits.** Each pool keeps its own slots, records, origins and warmth; a lease belongs to the pool whose ledger granted it. The host-wide build limit counts active commands in both pools, reading the other pool's atomically replaced state without its lock, so two commands starting in different pools at the same instant may briefly exceed it by one. A checkout's clean barrier spans both pools: a removable slot attempt holds the internal pool's admission lock (internal first, then removable) and waits for a live internal clean of its checkout, and `oxide clean` (which marks under that lock) waits for the checkout's active removable commands. `oxide clean` drops slot affinity in the internal pool only.
- **Status.** `status` prints the internal pool, the registration (volume and container UUIDs, reserve, admission cap, capacity and quota at registration), the volume's current available/total space, which pool the next build would use and why, and the removable pool's slots, flushes and retired slots.
- **Selection cost.** With a registration, each leased command's selection runs three `diskutil` and three `plutil` calls; without one, none.
- **Platforms.** Off macOS, registration refuses and selection always falls back.

Sources: `fstatfs(2)` / `sys/mount.h`; `getattrlist(2)` / `sys/attr.h` (`ATTR_VOL_UUID`); `faccessat(2)` with `AT_EACCESS` (volume writability only); `acl_get_fd_np(3)`, `acl_get_entry(3)`, `acl_free(3)` / `sys/acl.h`; `UF_HIDDEN` / `sys/stat.h`; `fsync_volume_np(3)` / `unistd.h` (macOS 10.8+: `SYNC_VOLUME_FULLSYNC` writes data and metadata to the disk hardware, `SYNC_VOLUME_WAIT` waits; returns a non-zero error code on failure); `diskutil(8)`. Observed on macOS 26.6: `fgetattrlist(ATTR_VOL_UUID)` on a non-root directory's descriptor answers the containing volume's UUID despite the man page's `EINVAL` rule, so the UUID is never treated as proof of a mount root.

**Proof.** `effigy rust:oxide-removable-pool` stages `oxide` offline with 1.97.1 and models the drive as one separately attached 128 MiB APFS sparse image in owned scratch, mounted by `hdiutil` at a unique `/Volumes/<name>`, over a budget-only internal directory pool. One fresh canonical directory inside the counted scratch is `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP` for the runner and every child from before staging. A session guardian (`scripts/bounded_runner.py`) owns the bounds from admission: a sticky first fault; host free space sampled every 250 ms against the 20 GiB reserve (a failed sample, dead sampler or sample older than 1 s faults); owned bytes walked at least every 2 s within 1 s (scratch, staging, targets, temp, logs, control and image backing, plus each positively identified mount by its used blocks; an attach in progress counts at the image's whole 128 MiB, a mount being detached at its last used bytes; on `ENOENT` the partial total is discarded and the walk restarts once inside the same second, and a second disappearance, any other error or a sample older than 4 s faults); both output streams drained and counted against 32 MiB as they arrive; work stops at 240 s and teardown must finish by 300 s. Every child goes through its one spawn path in its own session; a fault, deadline, exception, SIGTERM or SIGINT closes that session (TERM, then KILL, then a fresh observation), and a session not positively closed stays unclosed and blocks every detach. A child counts as closed only when its leader has exited and been reaped and a fresh observation finds its session and groups empty; a missing exit status, or a cleanup step that times out, is a failure that goes through closure. Every walk observation, including the final one and each mount observation, is checked against the same 1 s deadline before a total is published; a late walk faults and never refreshes the sample time. Every closure wait, process observation, drain and settlement uses only what is left of the original 300 s; when positive settlement cannot fit, the session stays unclosed and resources are kept. After a fault only cleanup runs: positive closure, then ordinary detach, never clearing the fault or starting experimental work. On success the scratch is removed by a bounded cleanup step while the reserve and deadline monitors stay live, with owned bytes explicitly held at the last complete total during removal. The 512 MiB figure is a monitored cap checked by complete samples, not an instantaneous ceiling: overshoot between samples is possible, and any sample over it fails the proof. Before staging, the same session runs a deterministic synthetic suite (`scripts/bounded_runner_cases.py`) under the real guardian: reserve breach, error, dead and stale sampler; storage excess, error, staleness, deadline and the `ENOENT` whole-walk retry; pipe pressure, output excess and an open pipe after exit; a fault and a cancellation during a running child; TERM-to-KILL escalation; a failed process observation settled by the outer guardian; stand-ins for an unresolved attachment, a failed ordinary detach and cleanup after the deadline; a late final `lstat`, `scandir`, mount `stat`, `ismount` and `statvfs`, and a second walk attempt past the deadline; a cleanup leader that closes its streams but stays alive; and deadline expiry during closure, drain, settlement and a cleanup step. Synthetic results are apparatus checks, not product acceptance. It covers: unregistered behaviour unchanged; the mount-parent rule accepting the real `/Volumes` (read only) and refusing an owned scratch directory first for its owner alone, then by name for one `chmod +a "<account name> allow add_subdirectory"` entry (debug builds expose `oxide pool debug-check-parent <dir>` for this); refusals (missing, zero and overflowing reserve, custom parent, nested path, absent volume, the `Macintosh HD` symlink alias, read-only attach, prefix look-alike, second registration); registration matching `diskutil`'s UUID with only `slots` on the volume; selection present (cold then warm, flush cost recorded), below its reserve and absent after an ordinary detach (no mount point recreated); an injected flush failure (debug builds only) retiring a held slot that the next build does not reuse; warmth kept across detach and re-attach; status. Unit tests cover the stale-directory and descriptor-swap cases with real kernel metadata, injected device change, Backup and other roles at registration and selection (container mismatch and the boot/data container too), and size parsing. A Backup-role volume, a mismatched device under the registered name and a stale `/Volumes/<name>` cannot be created unprivileged, so they are unit-tested only. 300 s, 512 MiB owned bytes including the mounted payload, ordinary detach only. Not a benchmark or real-drive qualification. The removable-pool runner and rust:oxide-removable-pool selector are unqualified test tooling; defects are fixed as found. Observed in the author's passing run at an earlier head (2026-10-20, before the review corrections): flushes of 8.9 ms and 8.8 ms after cold checks and 0.0 ms (display precision) after a no-op warm repeat, on a scratch sparse image backed by the internal SSD. These are not the real drive's flush cost.

### Design

**Volume and backend.**
- A dedicated APFS volume (for example `OxideBuild`) on the external drive.
- A new `apfs-volume-v1` backend uses a dedicated directory on the volume as the pool payload root. Report physical volume capacity and any APFS quota separately from Oxide's configured admission cap. Capacity shared by directories is not an exclusive quota for Oxide.
- No disk image on removable media: a long-lived image attachment turns a surprise removal into stuck I/O.

**Control plane internal, data plane external.**
- Registration, ledger and leases stay in the internal pool directory. These are small writes.
- Slots, and optionally final outputs, live on the external volume.
- Registration records the volume and container UUIDs and an explicit reserve value; it does not choose the real drive's reserve or quota policy.
- Accept only an OS-verified mount root directly under `/Volumes`, whose parent is effectively non-writable by the user (including ACL/access semantics). Refuse custom mount points under user-writable paths, the internal boot/data volume and Backup-role volumes.

**Selection, only when a build starts.** The external pool is used only when all of these hold; otherwise the internal default pool is used:
- the registered volume UUID is mounted, at a mount point whose device matches it. A stale `/Volumes/<name>` directory on the internal disk must never be mistaken for the drive. Re-check the device before creation and bind Oxide's mutations to an opened directory descriptor for the verified device, or an equivalent that closes the check/use race;
- the `/Volumes` parent remains effectively non-writable. This prevents an unprivileged child from recreating a removed mount point, but does not establish that the OS always removes it: stale underlying directory ownership and writability require source and fixture verification. If child writes could land on the internal parent after removal, refuse rather than claim no spill;
- it is writable;
- its free space is above the reserve.

A lease records which pool it uses. Each pool stays warm on its own.

**Surprise removal (part 2, not implemented).**
- A build whose volume vanishes fails like any I/O error, and its lease is marked lost-volume.
- **On releasing an external slot, Oxide flushes the volume** (`sync_volume_np` with full-sync-and-wait, or an equivalent the implementation cites). So a released slot's writes are on the device before it can be reused.
- A flush failure makes the slot held and non-reusable. It does not authorize deletion while writers are unknown.
- After an unclean disappearance, slots held at that moment are non-reusable. Discard them only after positive writer closure; otherwise quarantine and retain accounting. A vanished device, or Oxide's Cargo group ending while an inherited sccache server may still write, is not sufficient closure. Idle, flushed slots are kept.
- The ledger never lives on the removable volume.

**Final outputs stay checkout-local by default.** `CARGO_TARGET_DIR` remains `<checkout>/target` regardless of where slot intermediates live, keeping built binaries at the expected checkout path when undocked (Tom, 2026-10-20).
- **Following the pool is an explicit opt-in (part 2, not implemented):** a documented per-invocation flag and/or user config key enables a per-checkout output directory on the selected removable pool. Without the opt-in, docking alone never moves final outputs.
- With the opt-in, an unavailable removable pool falls back to checkout-local outputs. `oxide run` and `oxide test` use the effective output location; opting in or changing pool selection does not itself move or delete existing artifacts.
- External outputs are unavailable undocked. A later command may rebuild into the checkout; an interrupted command is never automatically replayed.

**What moves and what doesn't:**
- **Moves by default:** slot intermediates. Final outputs stay checkout-local; following the pool is the proposed opt-in above.
- **The sccache cache follows the pool only if #096's serialized per-build server model is adopted** (cache inside the pool). The current machine-wide server fixes its cache path at start-up. If that path is on a removable drive, a disappearance can silently recreate it on the internal disk, so it is not moved.
- **Queue worktrees and Colima's VM disk stay internal:** they hold live state that an unplug would break.
- Spotlight indexing of build locations is a host setting, recommended separately.

**Drive:** the dedicated spare 1 TB SSD (Tom, 2026-10-20), formatted by Tom in Disk Utility as plain APFS volume `Builds` (UUID `A6AC3ED7-33EA-46B4-8082-7254CF291ED2`). Vault and its Time Machine backups stay untouched. **Decided (Tom, 2026-10-20):** a 750 GB Oxide admission cap and a 100 GB free-space reserve, both set at registration. The flush primitive is implemented (`fsync_volume_np` with full sync and wait; see part 1 above); **not yet measured:** its cost on the real drive.

## Adaptive sccache location (proposed)

Approved design direction (Tom, 2026-10-20). Not implemented. It applies to builds that do **not** go through Oxide; Oxide-managed builds bypass inherited wrappers.

- **Constraint from sccache:** the cache location (`SCCACHE_DIR`) is read when its server starts, and "the local storage only supports a single sccache server at a time" ([Local.md](https://github.com/mozilla/sccache/blob/main/docs/Local.md)). Multi-level caches write to every level, so they don't keep writes off the internal SSD.
- **Design: one server and cache per location, chosen per compile.** A small Oxide-shipped wrapper binary becomes the machine `rustc-wrapper`. On each compile it runs the removable-volume identity check from #097, with no child process.
  - **Volume present:** `SCCACHE_DIR` on the volume, plus a dedicated `SCCACHE_SERVER_UDS` socket in a private internal runtime directory.
  - **Volume absent:** a separate internal cache directory and socket.
  - Then it execs sccache with the original arguments.
  - Each location keeps its own server and cache, so two servers never share one. Nothing restarts on a switch; an idle server exits on sccache's own timeout.
- **No spill:** the `/Volumes` permission rule from the removable design applies. A vanished volume cannot be recreated on the internal disk.
- **Known costs, to be measured:**
  - a compile in flight at the moment of undocking may fail;
  - each cache warms independently;
  - undocked compiles still write the internal cache, so its cap should be small.
- **Rollout:** after review, a staged host step switches `build.rustc-wrapper` in `~/.cargo/config.toml` to the wrapper. It is reversible by restoring the line, and the Effigy owner is informed first. The previous default server and cache are retired by stopping that server before the switch; they are never shared.

**Not yet decided:** cache caps (proposed 100 GB on `Builds` and 15 GB internal, within the drive's space beside the 750 GB pool cap and 100 GB reserve).

## Open decisions

- **W-1 Default pool.** Ruled 2026-10-20 and shipped in oxide#102: a
  budget-only directory pool, 100 GB soft budget, configurable path and
  budget, refuse below the 20 GiB host reserve
  ([above](#budget-only-default-pool-and-user-configuration)).
- **W-2 Waiting and concurrency.** Foundation choice above: per-project cap
  2, host-wide limit 2, strict FIFO waiting, idle-slot eviction before
  waiting. Coordinates with
  [Q-011](../questions.md#q-011--how-should-slot-waiting-coordinate-with-host-admission).
- **W-3 Project file.** Needed only for per-project profile overrides or
  dependency debug-off. Recommendation: none until a real project needs it.
- **W-4 Crash recovery.** Ruled and implemented for Oxide's own commands
  ([Crash recovery](#crash-recovery)); fixture-proven, including the
  `setsid` escape limit. Held on Oxide's own repository in the partial
  [foundation qualification](#qualification-result) record (Q5).
- **W-5 Retirement execution.** Done in oxide#070; see
  [retirements](../retired.toml).
- **W-6 Linux backend.** [Platform backends](#platform-backends):
  budget-only directory pools with a host-reserve watchdog are implemented,
  the Linux default and opt-in on macOS, fixture-proven on macOS and
  compile-checked for Linux. A Linux runtime proof, real write-rate margin
  evidence and the optional privileged image backend remain.
- **W-7 Command name.** `oxide` collides with Oxide Computer Company's CLI
  ([Q-005](../questions.md#q-005--what-names-are-safe-to-publish)). The design
  is name-independent.
- **W-8 Freshness gate.** Implemented (see
  [Cross-checkout freshness](#cross-checkout-freshness)). The fixture proof
  covers two Git worktrees: the stale-mtime scenario rebuilds and the test sees
  its own checkout's content; on a path change every in-checkout package
  rebuilds and observes the new path, covering a `CARGO_MANIFEST_DIR`
  member, a build script declared through an escaped key (`"b\u0075ild"`, no
  `build.rs`, embedding `current_dir()`), and in-repo non-member path
  dependencies with a default `build.rs` and a proc macro reading its
  caller's manifest dir; a path dependency outside the repository stays fresh; an
  unchanged repeat rebuilds nothing; a clone lacking the slot's tree is
  refused and builds cold; an `oxide` killed inside the gate after taking
  the slot leaves the retry to recover the slot and run the gate again. Warm-path cost is one `git status` and
  `rev-parse` before and after Cargo, about 25 ms on this repository; a
  path change adds one `cargo metadata` call. The metadata-membership rule
  passed the independent native fixture run at `c0867bb`; the later
  file-ownership corrections (unowned files and target sources outside
  package directories refresh; vendored registry/Git sources do not, and a
  local target or symlink into them refuses) passed the reviewer's native
  run at `69db6fe` or are unit-tested after it. The run at `d0fafd3` failed
  before the freshness scenarios: a generic `waiting for` check matched
  Cargo's "Blocking waiting for file lock on package cache". The fixture now
  matches only Oxide's exact wait lines (self-checked before the pool exists)
  and runs every fixture Cargo call with a private scratch `CARGO_HOME`;
  product staging alone uses the host Cargo home, inside the 300 s phase.
  Q2–Q4 held on Oxide's own repository in the partial
  [foundation qualification](#qualification-result) record; a heavy-project benefit
  reading remains a separate stage.
