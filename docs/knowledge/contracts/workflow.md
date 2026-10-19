# Oxide workflow

Status: product design under Tom's
[2026-10-18 workflow direction and bounded implementation authority](../vision.md#product-workflow-direction).
The foundation core and `init` / `new` adoption are implemented as the
unpublished `oxide` binary, installed for Tom only to
[dogfood](#dogfooding-in-oxides-repository) in this repository; the agent
skill exists as repository source only. [Implemented foundation core](#implemented-foundation-core)
states exactly what ships, including the cross-checkout freshness gate, the
[`run` no-op fast path](#run-no-op-fast-path), the budget-only directory
backend and [Cargo home hygiene](#cargo-home-hygiene). Non-build run strategies and the
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
| Extended built-ins | `init`, `new`, `clean` | Cargo's command plus Oxide's step (see [adoption](#adoption-oxide-init)). `clean` removes the checkout's private outputs and slot affinity and never deletes a shared slot: a 2026-10-18 probe showed plain `cargo clean` also deletes the injected build directory. |
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
whatever rustup resolves for the project directory; Oxide does not select or
pin it.

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

## What one command does (local executor)

0. `run` only: if the [no-op fast path](#run-no-op-fast-path) key matches,
   execute the recorded binary and stop: no child process, lease, slot or
   pool access.
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
   - the profile definitions in the environment.
5. `run`: keep the lease for the build, release it, then execute the binary
   from the private target directory. `test`: keep the lease through test
   execution, because test binaries live in the build directory. Reuse never
   skips tests.
6. When the process group exits, record the compile origin, release the lease
   and print one line: profile, strategy, warm/cold, elapsed time, pool use.

The [qualified cross-worktree result](project-adoption.md#focused-cross-worktree-result)
supports this shape for one workflow: a different-path checkout on a shared
slot rebuilt only its changed unit, and tests ran every time. It did not isolate
the mtime hazard above.

## `run` no-op fast path

Implemented and fixture-proven, not real-project qualified. `oxide run`
executes the binary from its last successful build directly when an
in-process file-metadata check shows nothing the key observes has changed.
Any doubt takes the normal leased path, which records again on success.
Cargo's ordinary freshness semantics are the reference, not absolute input
immutability ([vision](../vision.md#product-workflow-direction)).

- **Key.** One record per canonical working directory and modelled build
  arguments (`--bin`, `--example`, `-p` / `--package`, `--features` / `-F`,
  `--all-features`, `--no-default-features`, `--profile`, `--release` / `-r`,
  `--target`, in the given order). The working directory is part of the key,
  so a member directory never runs another member's binary. Program arguments
  after `--` are not part of the key and pass unchanged. Each input is a
  stat tuple following symbolic links: device, inode, size, mtime and ctime
  in nanoseconds, regular-file flag, or explicit absence. The record holds
  tuples for:
  - every path in the final binary's documented dep-info (`<binary>.d`);
  - every participating manifest: each path package's `manifest_path` from
    the build's `compiler-artifact` messages (fresh units included),
    cross-checked with every `Cargo.toml` between a listed source and the
    workspace root, so a `[lib] path` outside its package or a Git-ignored
    manifest is still covered;
  - Cargo's automatic discovery, present or absent, for every workspace
    member (from the `cargo metadata --no-deps` the normal `run` path reads)
    and every participating path package: the package directory (an added
    `build.rs`), `src`, `src/bin`, `examples` and each subdirectory of the
    last two (added targets that change `run`'s selection), and the
    package's parent inside the workspace (glob members);
  - every workspace member's manifest that metadata read for selection, built
    or not (an unbuilt sibling's `default-run` changes `run`'s pick); each
    must be strictly older than a time taken before that query;
  - when the workspace root manifest mentions `members`, every directory
    under the workspace root except the root `target` and `.git`: a member
    glob such as `crates/*/pkg` can match a package added at any depth, so an
    added member changes a recorded directory. A root manifest containing a
    backslash (an escaped key or pattern), or whose text could let a pattern
    reach the skipped `target` or `.git` (a quoted string whose first path
    component, after any `./`, is `target` or `.git` or contains `*`, `?` or
    `[`; a leading `..` followed by either name or a metacharacter; a `/../`
    climb; or any multi-line string), a directory
    symlink or
    more than 4096 directories records nothing;
  - `Cargo.lock`;
  - `.cargo/config`, `.cargo/config.toml`, `Cargo.toml`, `rust-toolchain`
    and `rust-toolchain.toml` in the working directory and every ancestor,
    and `$CARGO_HOME/config(.toml)`, absent ones included. `CARGO_HOME` and
    `RUSTUP_HOME` resolve as Cargo and rustup do: empty means unset, relative
    joins the working directory, unset falls back to the home directory;
  - toolchain resolution: rustup's `settings.toml`, every `PATH` candidate
    for `cargo` and `rustc` up to the one that resolves, the `rustup` a proxy
    is, and the resolved sysroot's `bin/rustc`, `bin/cargo`, `lib`, channel
    manifest and component list;
  - the built binary's post-build tuple, recorded separately.
  It also holds the arguments, working directory, workspace root and a
  SHA-256 over every environment variable's name and its value's SHA-256
  (raw values are never stored), plus Oxide's profile definitions.
  `OXIDE_NO_SKIP` is an ordinary variable there, so a forced build records
  a key the next plain run does not match.
- **Capture.** Before any Cargo or rustc query or use by the command
  (metadata, `locate-project`, `rustc -vV`, the build), the normal path binds
  the actual toolchain. A first `rustc --print sysroot` only locates it.
  The `PATH` `cargo` and `rustc` must each be the sysroot's own binary, or a
  rustup proxy (the same file as the `rustup` beside it) whose sysroot lies
  under rustup's `toolchains`; any other wrapper records nothing. Oxide
  stats the resolution inputs, the proxies and the sysroot's binaries, then a
  second query must name the same sysroot with every tuple unchanged, so the
  actual binaries are observed before a query that uses them. Every tuple is
  verified unchanged again at publish, after Cargo's queries and build. After workspace
  resolution it drops the key's record and stats `Cargo.lock`; that instant
  is the build start. Every leased command holds
  `<workspace>/target/oxide-run-skip/active.lock` shared until its lease is
  released; Cargo's process group inherits the descriptor, so the hold
  survives a killed Oxide while any group member runs. A successful `run` converts it to exclusive without waiting
  (another active Oxide command records nothing), then, still under its
  lease, re-checks every pre-build tuple and the environment, reads the
  dep-info and manifests and publishes atomically. Every input's mtime and
  ctime must be strictly older than the build start (a whole-second time
  older than its whole second, for truncating filesystems); the binary is
  exempt. A failed or interrupted build, a reserve stop, a missing or
  partial build report, or any read error publishes nothing.
- **Skip.** Only on an exact match of version, working directory,
  arguments, workspace root, environment digest and every tuple, with the
  activity lock free and the binary a regular file of the recorded tuple. The
  check is in-process: no Cargo, metadata, rustc, rustup, Git or shell child.
  It prints `oxide: <profile> profile, build+execute strategy, unchanged —
  skipped Cargo`, then replaces Oxide with the program, as the normal path
  does. A program failure is the program's exit code, never a rebuild.
- **Concurrency.** As Cargo's: the skip sweeps the tuples, then executes. A
  write landing after the sweep is caught by the next `oxide run`.
- **Eligibility.** Pinned Cargo 1.97.1 follows registry and Git package
  sources through their source identity, not file metadata: the
  [compiler call site](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/mod.rs#L454-L465)
  passes the unit's local-source classification, and
  [dep-info translation](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/dep_info.rs#L262-L267)
  skips package-relative files for non-local units. `Cargo.lock` in the key
  covers them as Cargo does. A registry or Git package with a build script
  is refused: Cargo follows its `rerun-if-changed` paths wherever they point
  and no documented output reports them. A path package's build script must
  declare `rerun-if-changed` or `rerun-if-env-changed` in its saved output;
  otherwise Cargo fingerprints the whole package listing, which the key does
  not observe. Path inputs outside the checkout are stat-recorded, not
  refused. Every dep-info path must be absolute and a regular file (a
  directory input refuses). Any other source kind refuses. A refusal prints
  `oxide: run skip not recorded (<reason>)` on the normal path; it is
  explicit, not an implied all-project speedup.
- **Ineligible arguments and settings.** Any other argument before `--`
  (including `-v`, `--manifest-path`, `--message-format`, `--config`,
  `--locked`); a working directory outside the workspace; `RUSTC`,
  `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER` or their `CARGO_BUILD_*` forms
  set; a Cargo config containing `include`, `replace-with`, `paths` or
  `rustc`, or any backslash (a TOML quoted key can spell those names with
  escapes, so the screen never decodes TOML); a config over 1 MiB or not
  UTF-8. A configured compiler or wrapper therefore never records: the
  compiler the key binds is always the `PATH` one.
- **Record.** Versioned JSON (version 4) in
  `<workspace>/target/oxide-run-skip/`, at most 4 MiB each and 64 per
  checkout (cleared past that), so `oxide clean` drops records with the
  binaries. A missing, other-version, partial or corrupt record never skips.
- **Overrides.** `oxide run --rebuild` (before `--`) or `OXIDE_NO_SKIP=1`
  (any non-empty value) runs Cargo and records again. `--rebuild` after `--`
  is a program argument. `test`, `check`, `build` and `clippy` always run
  Cargo; they only hold the activity lock.

**Limits.** A build script's undeclared reads (an outside file without a
`rerun-if-changed` line, a tool on the system) are not observed; nor is a
linker or system library changing in place. Stat tuples are cooperative
same-user evidence, not content attestation. Exact source and lock coverage
is relative to pinned Cargo, not downloaded-source immutability.
Generated sources a dep-info lists in a slot are stat-recorded like any
other input. Qualification beyond the pinned toolchain is separate work.

**Proof and gate.** `effigy rust:oxide-run-skip` runs one fresh 128 MiB APFS
pool within 300 s from admission, including offline product staging. Staging
output and temporary files, fixtures, copied inputs, the pool backing and
fixture temporary directories share one owned parent, whose allocation is
sampled after every phase against 512 MiB together with the 20 GiB host
reserve; fixture inputs are held under 8 MiB. Skips run under
`RLIMIT_NPROC` 0, so a successful skip started no child; a control shows the
same limit refuses the leased path. It covers repeat skips, each key class
falling through (source edit, restored-old-mtime edit caught by ctime, the
ignored custom-lib-path manifest, root manifest, lock, config added above the
workspace, toolchain file, environment variable, deleted binary, outside path
dependency, an added `build.rs`, an added binary reaching Cargo's ambiguity
refusal), member selection, a failed build leaving no record, a
`cfg-if` registry dependency eligible and a `libc` build-script dependency
refused, from named cached inputs in a private `CARGO_HOME`. The fixed #079
two-part gate (architect ruling, 2026-10-19) takes three samples of each
side and prints them raw in nanoseconds: the slowest skip at most half the
fastest leased `oxide run --rebuild` (both execute the program), and the
fastest skip at most the fastest bare `cargo build --profile oxide-quick`
no-op (build only) plus noise, the larger max-minus-min spread of the skip
and bare samples. A failure ends the proof as a no-go. The author's run before
the discovery, toolchain-binding and accounting revision held in 22 s: skip
10.277–12.861 ms, leased 444.143–712.410 ms, bare 28.463–30.556 ms, noise
2.583 ms; both parts passed. The independent review's run of that head also
passed both parts. Fixture timings are not a real-world
speed claim, and the skip is not claimed to beat bare Cargo on larger
repositories.

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
  `oxide-storage-lease recover` is the explicit release. A record written by
  an earlier front-door build has no holder, so it is kept the same way.
  `oxide status` names which of these applies to each interrupted slot and
  promises automatic recovery only where the rule allows it.
- Each automatic recovery is logged in the control state (slot, lease,
  checkout, holder PID and start time, group, time, recovering PID). The
  event is written to the record before the ledger recovery and moved into
  the log after it, so a crash between the two still logs it on the next
  sweep. `oxide status` shows the newest 16.
- Leases taken through the standalone `oxide-storage-lease` CLI have no
  control record and no lock holder, so they are never auto-recovered.

**Residual risk.** A descendant that leaves the group with `setsid` or a new
spawn group is invisible to `killpg`. If it still writes in the slot when the
group is otherwise empty, the slot can be recovered and reused under it.
This is the accepted limit; the fixture proof shows such an escapee does not
keep the slot held.

This replaces the local lease's "never infer abandonment" rule for Oxide's
own commands only. Consumers that lease per task, such as Nucleus, keep their
explicit `recover` unless they adopt it.

## Storage and space management

- One per-user host pool: the existing [bounded APFS backend](bounded-storage.md)
  on macOS, where the volume size is the hard aggregate ceiling; the
  budget-only [directory backend](#platform-backends) elsewhere or by explicit
  setting. Only whole idle slots are evicted.
- The first command creates the pool automatically ([W-1](#open-decisions)).
- Per-project slot cap, recommended 2 to start, plus a host-wide limit on
  concurrent Cargo invocations ([W-2](#open-decisions)). Queue/Nucleus still
  decide which tasks run. Oxide only orders the Rust work they launch.
- `oxide status` reuses the read-only [storage inventory](storage-inventory.md).
  It reports pool and slot use (each slot's file names, distinct inodes and
  allocation counted once per inode), the Cargo home's allocation, and stray
  output: a checkout `target/` holding `deps/` or `incremental/` was built by
  plain Cargo. Reclaiming that space is an explicit command; live cleanup
  still needs scoped approval. Nothing inside a slot is ever deleted.
- `OXIDE_POOL_CAP` sets a new pool's cap; `oxide clean --cargo-cache` asks
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
  `~/Library/Caches/oxide/pool` under the shipped rules: a 24 GiB
  `apfs-sparse-image-v1` image, refused below the 20 GiB host reserve.
- **Selectors.** `rust:compile`, `rust:compile-linux`, `rust:format` and
  `test rust` call `oxide`. With the default `quick` profile, check and test
  use line tables rather than `dev`'s full debug info. Focused proof, profile
  and install selectors call Cargo, because they test or build Oxide itself.
- **Escapes.** `rust:compile:cargo`, `rust:compile-linux:cargo`,
  `rust:format:cargo` and `test rust-cargo` keep the plain-Cargo commands.
  Use them when `oxide` itself is broken. They build into the checkout's
  `target/` unless `CARGO_TARGET_DIR` points elsewhere, and `oxide init`
  readiness lists them as plain-Cargo lines by design.
- **Test supervisor.** `oxide` runs Cargo in its own process group and
  forwards TERM to it, so killing the supervisor's group alone would miss
  Cargo. `bounded-test.py` starts the command in a new session; `oxide`'s
  `setpgid` keeps Cargo, its tests and any child orphaned by Cargo's exit in
  that session. At the 300-second limit, before reaping its child, it
  records every process group in the session, TERMs its own group, and
  after ten seconds KILLs the recorded groups and then its own. It reports
  any session or group member still alive five seconds later. A process
  that called `setsid` is outside the session and is not seen.
- **Fixtures.** `effigy dogfood:scripts:test` proves that escalation,
  including the orphan whose group leader exited, and
  the installer's refusals (linked or foreign directories, an existing
  destination, a destination swapped during the build) on synthetic
  fixtures. It never builds or installs.

### Rollback

Do not rehearse these steps; they remove the dogfood artifacts.

1. Stop starting Oxide work: switch selectors to the `:cargo` escapes or
   revert the adoption commit, which restores the plain-Cargo selectors and
   removes the marked `AGENTS.md` block.
2. Unlink the binary only if it still matches the newest receipt:
   `shasum -a 256 ~/.local/bin/oxide` equals its `installed.sha256`. Keep the
   receipts. A mismatch means an unknown file: leave it and report.
3. To retire the pool as well, run `oxide status` with Oxide still installed,
   or `oxide-storage-lease status` on the pool. Every slot must be idle, no
   command waiting and no recovery pending. A held or unknown holder, group
   or waiter means stop: preserve and report. Never reset a lease by hand.
   That CLI has `init`, `lease`, `release`, `recover` and `status`; it has no
   detach verb.
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
| [Local lease](local-lease.md), pool, APFS backing and [bounded storage](bounded-storage.md) | The storage layer of every `oxide` command; live with Nucleus. |
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
inputs outside Git that the gate does not cover.

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
- **Pool.** `OXIDE_POOL`, else `~/Library/Caches/oxide/pool` on macOS and
  `$XDG_CACHE_HOME/oxide/pool` or `~/.cache/oxide/pool` elsewhere, created on
  first building command. Creation refuses below 20 GiB host free space. On
  macOS the default is `apfs-sparse-image-v1` with a 24 GiB cap equal to its
  sparse image. `OXIDE_POOL_BACKEND=directory-budget` selects
  `directory-budget-v1` instead, with a recorded 24 GiB budget; it is the
  default on other platforms, where `apfs-sparse-image` refuses. Any other
  value refuses. The variable applies only at creation: naming a backend
  other than an existing pool's refuses, so an existing pool from
  `oxide-storage-lease init` opens unchanged. See
  [Platform backends](#platform-backends). `OXIDE_POOL_CAP` sets the cap
  (or budget) a new pool records: a byte count or a whole number of `KiB`,
  `MiB`, `GiB` or `TiB`, a whole MiB, from 128 MiB (APFS) or 1 MiB
  (directory) to 16 TiB. Any other value refuses before any effect. An
  existing pool keeps its recorded cap: a different value prints
  `OXIDE_POOL_CAP … not applied` and is ignored; nothing is resized or
  migrated. Creation still refuses below the 20 GiB host reserve, and an
  image cap bounds only the pool, not other host writes.
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
  Oxide with the program; an unchanged key skips straight to the program
  ([fast path](#run-no-op-fast-path)). It sets `CARGO_MANIFEST_DIR` only; other `cargo run`
  environment (`CARGO_PKG_*`, dylib paths) is not set. Exit codes are Cargo's,
  or the program's for `run`.
- **Summary.** One stderr line per leased command: profile, strategy (`build`
  or `build+execute`), slot, warm/cold, elapsed time, pool use and backend:
  `pool 3 of 128 MiB used (apfs-sparse-image-v1)` (payload volume use), or
  `pool 12 MiB allocated of 24576 MiB budget (directory-budget-v1, budget-only)`
  (slot allocation against the budget).
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
  - any Cargo config file Cargo would read (`.cargo/config.toml` or
    `.cargo/config` in the current directory or an ancestor, or in the Cargo
    home). Oxide does not parse them: a file can set source replacement,
    registries, `git-fetch-with-cli`, credential providers or includes;
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
  source-replacement config file, a custom registry and a stale lock whose
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

## Platform backends

`directory-budget-v1` is implemented (see [below](#directory-budget-v1-as-shipped));
`linux-image-v1`, CI cache save/restore and container execution are proposed
only. The pool needs a backend on every host where builds run. On macOS, the APFS
sparse image gives an unprivileged hard ceiling. Linux has no unprivileged
equivalent: loop mounts, XFS/ext4 project quotas and btrfs qgroup limits all
need root, and the filesystems an unprivileged user namespace may mount
(`tmpfs`, overlay, FUSE) are RAM-backed or slow. The default must still need
no setup, per Tom's ease-of-use ruling.

| Backend | Where | Bound | Setup |
| --- | --- | --- | --- |
| `apfs-sparse-image-v1` (shipped) | macOS | OS-enforced aggregate ceiling | None; created automatically |
| `directory-budget-v1` | Any host, the Linux default | Budget only: admission plus whole-idle-slot eviction against a recorded budget, with no OS ceiling. A supervised build samples caller-available space through `statfs` and attempts to stop its own process group below the 22 GiB trigger (20 GiB protected reserve plus 2 GiB margin). Best effort, with no guarantee against faster writers or other processes; no directory walk during the build. | None |
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
  (`cap_bytes`, default 24 GiB; `image_bytes` 0). An image or receipt file,
  any payload entry but `slots`, or a payload on another device refuses.
  The library never mints a `BoundedBacking` for it.
- **Admission.** The ledger's existing rules with the budget as the cap:
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
  sets a new pool's budget. Release builds ignore both. The reserve and
  margin have no override.
- **Standalone CLI.** `oxide-storage-lease` refuses a directory pool
  (`backing_mismatch`, exit 4); see
  [local lease](local-lease.md#oxide-front-door-control-directory).
- **Proof.** `effigy rust:oxide-cli` adds a macOS case on two scratch
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

## Open decisions

- **W-1 Default pool.** Foundation choice above: per-user cache directory,
  24 GiB sparse image, refuse below the 20 GiB host reserve. Revisit with real
  use.
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
