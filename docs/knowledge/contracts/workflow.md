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

## Cold-slot seeding

Cold-slot seeding is implemented in the unpublished `oxide` (`oxide#167`,
`oxide#168`, `oxide#169`) and **enabled by default**: this slice's one-line
qualification flip is in the merge head. `OXIDE_COLD_SEED=0` is the opt-out;
the variable was never a permanent switch. A fresh slot of a supported command is warmed once,
before any child exists, by cloning matching dependency units from an idle
donor slot on the same pool under the existing journaled custody: the donor
record lock and pool lock are held, every artifact, `.d` and fingerprint file
is re-hashed against the generation-bound catalog, and the recipient publishes
only after a complete ownership-clean post-clone walk. v1 offers **only plain
registry library compile units**: nothing is offered for a non-leaf package, a
proc macro, a build-script compile/run unit, an outside-root/`OUT_DIR` `.d`
input, a Git/Git-revision or path source, an unprovable `.d` join, a differing
producer context or a run-only fingerprint. A failure before the first
recipient write skips to the normal cold build; every later clone, digest or
custody fault refuses, releases the slot with no trusted origin and leaves the
untrusted generation to the ordinary journaled whole-slot eviction. A failed
post-clone walk publishes nothing and cold-builds in a newly admitted slot.
This adds no grant, no second lease, no deletion path and no new sidecar
format beyond the bounded `oxide-managed/seeds/` catalog/attempt/last
documents, and it never mutates the donor.

The bounded skip message names an ambiguous dependency before a generic
non-leaf root-package rejection. A command with no seed plan bypasses only the
seed reserve gate: the ordinary Cargo reserve watchdog still runs and can stop
its child. Catalog rejection counts group a normalized-name collision into one
row; an omitted `units` means unknown, while an explicit zero means measured
with no candidates.

Both provided backends seed: an internal `directory-budget-v1` pool's start
gate is the host reserve (20 GiB + 2 GiB margin, sampled before the durable
marks and monitored during the clone), and an `apfs-volume-v1` removable pool
re-verifies its registration's mount/volume-UID binding and reserve before the
marks and monitors that binding at the watchdog cadence. A missing
`clonefile`, an unknown source identity or a lost volume follows the existing
refusal, removal and quarantine rules.

The qualification apparatus is one fixed 24-case script,
`scripts/test-cold-slot-seeding.py`:
`effigy rust:cold-slot-seeding` and `:diagnostic` run it under the qualified
shared supervisor with explicitly capped scratch pools on the selected volume
(a `/private/tmp` fallback is not scratch-pool coverage), **900 s** (Tom's decision `09c6cd03`, this selector and its
diagnostic twin only), 512 MiB monitored scratch and one owned 128 MiB image;
`effigy proof:cold-slot-seeding:test` runs only its pure apparatus. Case 21
compares canonical dep-info and workspace source paths, with relative paths
resolved from the workspace and an owned-alias pure regression. The earlier
480 s diagnostic at `86208d1` failed case 21, aborted case 22 at its work
cutoff and left cases 23–24 unexecuted; it remains historical with no counted
credit or complete fit. The diagnostic at `98d665b` completed 23/24 cases with zero failures and case 24 aborted near 600 s; it is historical with no qualification credit. The 900 s bound retains the 30 s margin and all
other safeguards. A fresh exact-head source review must precede the next
separately owner-activated diagnostic. The
uncounted diagnostic, one counted author native and one separately activated
independent reviewer native are owner-activated at the exact head; cases 01
(non-leaf SQLite), 03/06 (Git/build-script/`OUT_DIR` seedctl) and 05 (proc
macro) are negative controls, and positives are plain registry libraries. The
[tier is ELEVATED](../vision.md#validation-policy) because a post-clone
abandon retains an untrusted generation that the ordinary journaled eviction
later deletes and one case writes removable media. The merge stays held until
those natives pass at the exact merge head, because the default-on front door
is product behaviour and not only proof input. The
[cold-slot seeding contract](cold-slot-seeding.md) owns the transaction, its
partial-association limit and the proof plan.

The explicit seed opt-out merge-base identity checks remain in
`tests/cold_seed_default_off.rs`, behind `#[ignore]`; the separate
`effigy rust:seed-default-off` selector stages the S3-or-later merge-base
`oxide` and exports `OXIDE_SEED_BASELINE_BINARY`. They are not part of
`effigy test rust` and do not qualify the default-on candidate.

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
oxide exec [--manifest-path <Cargo.toml>] [--profile <name>] -- <command…>  # a tool that calls Cargo itself
oxide status                             # recorded pool, slots, waits; no walk, lock or reconcile
oxide status --measure                   # bounded payload/slot/checkout/Cargo-home walks, still no reconcile
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
| Building passthrough | Other built-ins that compile (`doc`, `bench`, `rustc`, `rustdoc`, `fix`) and every unknown or external subcommand (`nextest`, `llvm-cov`, `miri`) | Passed to Cargo unchanged, under a lease with storage and profile definitions injected. Unknown commands are treated as building, which is the conservative choice. The compiling built-ins' profile outputs are classified from the command name; an external subcommand derives its member set from the checkout's committed receipt exactly like `oxide exec` (oxide#164). |
| Plain passthrough | Non-building built-ins (`fmt`, `tree`, `metadata`, `add`, `remove`, `update`, `info`, `search`, `install`, `publish`, `login`, `vendor`, `fetch`, `generate-lockfile`, `locate-project`, `pkgid`, `version`, `help`) and the four scanner tools `deny`, `audit`, `outdated`, `machete` | Run as plain Cargo; no lease. |
| Extended built-ins | `init`, `new`, `clean` | Cargo's command plus Oxide's step (see [adoption](#adoption-oxide-init)). `clean` removes the checkout's private outputs and slot affinity and never deletes a shared slot: a 2026-10-18 probe showed plain `cargo clean` also deletes the injected build directory. A whole-target clean is Oxide's own plan ([clean](#clean)). |
| Oxide-owned | `status`, `pool`, `exec` | Oxide semantics. `exec` runs a tool that calls Cargo itself under a lease ([below](#oxide-exec-cargo-wrapping-tools)); it shadows an external `cargo-exec` subcommand, which stays reachable as plain `cargo exec`. |

- A built-in Oxide extends keeps Cargo's meaning and adds to it. Purely
  Oxide verbs never reuse a Cargo built-in name.
- Arguments pass through unchanged. `--release` means `--profile release`.
  Oxide owns storage placement, so `--target-dir`, and `--config` keys that set
  `build.build-dir` or `build.target-dir`, are refused with an explanation.
- All injection uses environment variables (`CARGO_BUILD_BUILD_DIR`,
  `CARGO_TARGET_DIR`, `CARGO_PROFILE_DEV_*`, the deprecated
  `CARGO_PROFILE_OXIDE_*`), because child Cargo calls inherit them. A
  2026-10-18 probe on 1.97.1 showed that a `--config` profile definition is
  invisible to an external subcommand's `$CARGO` call, while an
  environment-only definition works there.
- Built-in commands consume Oxide's `--profile quick|debug` and keep Cargo's
  default profile; `release` becomes `--release` (see [profiles](#profiles-and-strategies)).
  External tools receive arguments untouched and build with the quick recipe
  through the inherited `dev` settings, with no profile flag.
- `deny`, `audit`, `outdated` and `machete` are exact passthrough (oxide#130):
  Oxide replaces itself with the caller's Cargo, which receives the original
  arguments unchanged, in the caller's working directory, with the caller's
  environment, standard streams, signals and exit status. No slot is leased,
  no pool is created or measured, no adoption, guidance or build-recipe
  injection runs, and the front-door hand-off check is skipped. Every other
  unknown or external subcommand stays leased, as above.

## Profiles and strategies

An Oxide profile is the user's intent. A strategy is how Oxide meets that
intent for one command. Names stay stable while strategies improve.

| Profile | Intent | `check` / `clippy` / `build` | `run` | `test` |
| --- | --- | --- | --- | --- |
| `quick` (default) | Fastest route to feedback on the current source | Quick build recipe | Fastest qualified execution strategy, falling back to quick build plus execute | Quick build plus execute, unless a strategy preserves test semantics |
| `debug` | Full debugger experience | Full-debug recipe | Full-debug build plus execute | Full-debug build plus execute |
| `release` | Production-shaped output | Project's own `release` profile | Release build plus execute | Release build plus execute |

`quick` and `debug` tune Cargo's built-in `dev` profile through
`CARGO_PROFILE_DEV_*` environment variables on every leased Cargo process
(Tom, 2026-10-20, [vision](../vision.md#product-workflow-direction); oxide#106).
No `Cargo.toml` or machine configuration changes. Every other `dev` setting
stays the project's own, `test` inherits the recipe, and child Cargo calls,
including an external subcommand's, inherit it with no profile flag. Final
outputs therefore land where plain Cargo puts them, `target/debug/`:

- `quick`: `CARGO_PROFILE_DEV_DEBUG=line-tables-only` for every unit; it
  clears `CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG`, so build-time units keep
  Cargo's `dev` default (no debug info) and an outer `debug` command's
  setting never reaches a nested quick one.
  Per-package overrides (`package."*"`) cannot be expressed through the
  environment; a 2026-10-18 probe showed the dependency stayed at line tables. So this
  recipe keeps dependency line tables, unlike the `oxide-light` recipe in
  [stable Cargo profiles](stable-cargo-profiles.md). Whether dependency debug-off
  is worth a config file is part of [W-3](#open-decisions).
  On macOS its default unpacked split debug info leaves per-rebuild
  `*.rcgu.o` names in `deps/`. A 2026-10-18 measurement recommends
  `split-debuginfo = "packed"` if a fix is adopted and rejects workspace
  debug-off for `quick` ([landscape](../rust-dx-landscape.md#stale-debug-object-measurement-2026-10-18)).
  The recipe is unchanged pending that decision.
- `debug`: `CARGO_PROFILE_DEV_DEBUG=full` and
  `CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG=full`: full debug for every unit,
  including build-time units.
- `release`: untouched; `dev` stays as the caller has it.

Commands keep Cargo's default profile: `dev` for `check`, `clippy`, `build`,
`run`, `doc`, `rustc`, `rustdoc` and `fix`, `test` (inheriting `dev`) for
`test`. `bench` keeps its release-based default; an explicit `--profile quick`
or `debug` bench passes `--profile dev`. Both recipes share
`target/debug/<bin>` and the latest build wins, as with plain Cargo. Their
intermediates stay in separate slots ([key](#project-identity-without-configuration)).

Oxide always consumes its own `--profile debug`. Cargo 1.99 adds a built-in
`debug` profile, currently like `dev`; a 2026-10-20 probe built
`--profile debug` into `target/debug/` on 1.99.1 and 1.97.1 refused it
([landscape](../rust-dx-landscape.md#rust-1981100-relevance-2026-10-20)).
A tool argument such as `oxide nextest run --cargo-profile debug` passes
through unchanged and selects Cargo's own version-dependent profile, which is
not Oxide's full-debug recipe: that profile's inheritance and build override
are Cargo's. Use `oxide nextest run` for the quick recipe.

**Deprecated custom profiles.** Before oxide#106 the recipes were the custom
profiles `oxide-quick` and `oxide-debug`, built into `target/oxide-quick/` and
`target/oxide-debug/`. Their `CARGO_PROFILE_OXIDE_*` definitions stay in every
leased environment, so an existing `--cargo-profile oxide-quick` or
`oxide-debug` still builds, into those directories, not `target/debug/`. Drop
the flag. The recipes are the intended defaults, not measured speed or
storage claims. The [stable profiles](stable-cargo-profiles.md) comparison
stands as a no-go.

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

The slot compatibility key is repository identity + Oxide recipe key +
resolved toolchain (`rustc -vV`) + target triple + codegen backend, plus an
owned checkout's final-output destination (its selected root's kind, real
path, bound volume and creation generation). An ephemeral or detached
checkout adds no destination component. The
recipe key is `quick:dev-recipe-2`, `debug:dev-recipe-2` or `release`.
The effective incremental setting also partitions slots: `CARGO_INCREMENTAL`
beats `build.incremental` (including `--config`), which beats the profile's
setting. An older slot has no recorded setting and is treated as the recipe
default (incremental on for quick/debug, off for release). An unknown setting
uses a stable unknown-setting key, distinct from either known setting; repeated
unknown commands can reuse it. External tools without a global
`CARGO_INCREMENTAL` override choose their own Cargo configuration and profile,
so their setting is unknown to Oxide. Inline `--config` values are considered;
unreadable or unclassifiable config layers stay unknown.
Version 1 keys were the bare `quick` and `debug` of the custom-profile
recipes, so a slot built by them is never reused for the `dev` recipes: the
first quick or debug command after the change takes a new slot cold. Older
slots are not renamed, copied or deleted; they leave through normal eviction,
which at the per-project cap of 2 evicts the least recently used idle slot.
Status still labels them `quick` or `debug`. The toolchain is
whatever rustup resolves where the command runs (see
[caller context](#cargo-version-matrix)); Oxide does not select or pin it.

### Project-named slots and pool status

oxide#104 (Tom, 2026-10-20: "It would be pretty handy to group them by
project name so it's obvious what's what"). Presentation only: the slot key,
identity, ledger semantics, admission, eviction and recovery are unchanged.

- **Layout.** A new slot is created at
  `slots/<label>-<short-id>/<profile>-<n>`, for example
  `slots/oxide-3fa2c1d4/quick-7`. `<n>` is the slot ID; `<profile>` is the
  Oxide profile label (`quick`, `debug`, `release`). The ledger records the
  path below `slots` (`dir`) at creation, and it never changes.
- **Label.** The last segment of the normalised `origin` URL, else the
  repository directory name (`git rev-parse --show-toplevel`), else the
  workspace directory name. It is sanitised to lowercase ASCII letters,
  digits, `_` and single `-`, at most 40 bytes; `project` when nothing
  remains. Origin comes first so worktrees with task-named directories share
  a readable label.
- **Short ID and collisions.** The short ID is the first 8 hex digits of the
  repository identity digest. It is display, not a uniqueness proof. Before
  creating a slot, Oxide checks the folder: one that already holds a slot of
  another repository, or of a project not known to be this repository, is
  skipped, then the 12-, 16- and 64-digit forms are tried. An existing folder
  with no recorded slot is used only when it is empty apart from
  [Finder metadata](#finder-metadata-in-pools). When no form qualifies the
  slot gets the legacy path. A repository's later slots join the folder it
  already uses, so worktrees with different labels still share one folder.
- **Existing slots** keep `slots/<id>`. They are never renamed, moved or
  adopted into a folder: Cargo records absolute paths inside a slot. A
  ledger without `dir` reads and writes unchanged. A binary older than #104
  refuses a ledger holding a labelled slot (its ledger records refuse
  unknown fields) rather than guessing the path.
- **Eviction** removes only the slot directory. Its project folder stays,
  possibly empty; nothing removes folders. Deletion stays limited to a
  recorded slot directly under `slots` or under one owner-only project
  folder on the same device. Payload accounting allows two levels above the
  slots, so slot contents keep their depth allowance.
- **Status.** Default `oxide status` reports the recorded selection and the
  most recently used slot: its pool, slot, project label, checkout and age.
  The selection is what the registration and a cheap mount-point presence
  check record (oxide#128), not the verified selection check a build makes;
  `status --measure` runs that check and reports its reason. Each pool then
  has one header, with the recorded selection first and its own slots beneath
  it; equal slot numbers in two pools never mix. Every slot line shows its
  project label (the folder for a labelled slot, else the checkout directory
  name) and its place, `slots/<dir>` or `slots/<id> (legacy path)`. A
  registered removable pool whose volume is absent or unreadable keeps its
  registration lines, shows its recorded slots from the internal ledger
  (unknown, not empty) and never probes the volume; `status --measure` adds
  the verified reason.
- **Most recent use.** Within one pool Oxide's use counter orders uses.
  Across pools it compares the wall-clock time recorded at each use since
  #104 (`last_used_ms` in the control state). A use recorded before then has
  no time, so the cross-pool answer is `unknown`, with each pool's latest
  listed. Equal times name the first listed (selected) pool and say they are
  tied. For a removable pool that is not usable, status reads its internal
  control state only: the slot is `unknown` and the label is the checkout
  directory name. An unreadable state reports `unknown`, never "none".
  Default status opens no pool lock, acquires nothing and changes no ledger;
  it reads the recorded state without reconciling it.

**Proof.** `effigy rust:oxide-pool-layout`
(`scripts/test-oxide-pool-layout.py`) runs under the qualified shared proof
supervisor: `bounded-test.py --limit 300 --scratch-budget-mib 512 --`
(oxide#109). The script writes only under the supervisor's scratch
(`OXIDE_TASK_SCRATCH_ROOT`, equal to the other three temp variables) and
its evidence directory (`OXIDE_PROOF_EVIDENCE`), keeps every child in the
supervisor's session, stops work 30 s before the exported absolute
`OXIDE_PROOF_DEADLINE_NS` (with a `SIGALRM` fallback) and leaves closure and
scratch cleanup to the supervisor. It reuses the #105 script's phase checks
and staging helpers; on a child timeout it kills only that child, and an
unreaped child blocks detach and keeps evidence.
It stages the base commit's and this HEAD's `oxide` offline with 1.97.1,
each into its own fresh `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR`
with inherited build routing scrubbed. It logs both commits, trees,
manifest and lock digests and binary SHA-256s, and refuses before any pool
or image exists unless the binaries differ and only the candidate carries a
#104 status string. Over a private `HOME`, `CARGO_HOME` and pool: the base
binary makes a legacy `slots/1`, which the candidate reuses warm in place;
same-named checkouts get separate labelled folders; a debug slot joins its
project's folder; one 128 MiB scratch APFS image registered as the
removable pool gets labelled slots, including a slot 1 beside the internal
slot 1; status order, grouping and most-recent attribution are checked with
the volume selected and after an ordinary detach. Not a benchmark or
real-drive qualification.

**Earlier submissions (history).** The first author session at `de7bb8b`
(2026-10-21) staged both sources into one shared target directory; Cargo
reused the base build for the candidate (both SHA-256 `46db66ce…`), so the
first candidate `status` printed the old format and the script failed after
16 s, before any image or product exercise. Two later submissions under a
#104-only exception for the old `3d947a9` supervisor never launched: run
`a5b87b4a…` was withdrawn and settled cancelled, and run `bde2d70a…` hit
Effigy's capacity timeout. All three are consumed.

**Compatibility of the new records.** Legacy slots and a ledger without
`dir` serialize byte-for-byte as before; a labelled slot's path is fixed at
creation and its identity, key and compatibility are unchanged. A binary
older than #104 refuses a ledger holding `dir` instead of using a wrong
path. `last_used_ms` is written only with the use counter when a command
takes a slot, never by `status`; admission, eviction and slot choice still
order by the counter, never by wall time. Missing times, an unreadable
state and ties are reported as such.

The other proof scripts that located a slot at `slots/<id>`
(`test-front-door-accounting.py`, `test-cargo-profiles.py`,
`test-oxide-exec.py`, `test-oxide-removable-pool.py`, `test-pool-scale.py`,
`test-oxide-finder-metadata.py`) now read the ledger's `dir` or find
`slots/*/<profile>-<id>`. These script changes were not rerun natively under
#104.

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
   digest of every dirty or untracked non-ignored file, the state of every
   declared Git submodule (initialized-clean at its gitlink, or positively
   absent or empty uninitialized), and the build anchor (the time after
   Cargo's process group is empty).
2. Oxide diffs the two content sets with Git (`git diff` of the trees, the
   dirty digests and the submodule states). Each differing checkout file whose
   mtime is not newer than the anchor gets its mtime set to now, so Cargo's
   own invalidation rebuilds it. Content stays untouched; only the checkout's
   own files change; Cargo fingerprints and the slot are never edited.
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
     lock is written and the slot is refused. The parse check sets
     `CARGO_TERM_COLOR=never` and removes `CLICOLOR_FORCE`, so a forced-colour
     environment or `term.color = "always"` cannot colour that `error:` prefix
     and make the gate refuse a command Cargo accepts. External and unknown
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
5. Inputs outside Git or ignored by it are not covered. The summary line says
   so whenever the gate reconciled another checkout's build.

Declared Git submodules (gitlinks) come from the index **and** `HEAD`: both
must name the same commit at stage 0, with no assume-unchanged or
skip-worktree bit and no sparse exclusion, at most 32 of them, one level deep.
Two worktree states are covered and persisted as distinct content:

- **Uninitialized**: the gitlink path is absent or an empty real directory
  with no `.git` entry.
- **Exact**: a directory whose own `HEAD` is the declared commit, whose
  tracked files are clean and whose `.git` file or directory resolves inside
  this repository's module store, `<common Git directory>/modules`, which
  must be a real directory rather than a symlink to elsewhere. It also
  records a framed digest of the child's ordered cached paths and raw blob
  IDs, so the same commit reached through different bytes still differs.

Same tree, same dirty digests and same submodule states is a warm no-op. An
uninitialized worktree and an initialized one at the same gitlink never match.
Everything else refuses the slot and the command builds cold: dirty or
untracked child content, a different child `HEAD`, an unmerged or
flag-marked gitlink, a non-empty directory without a `.git` pointer into the
module store, a nested gitlink, an unreadable path, a symlink, or an unknown
state. A linked worktree supports only the uninitialized state; an initialized
child there is not vouched for, because it never owns the module store. The
gate never runs a `submodule` command, never initializes, updates or fetches,
and never writes a submodule's config, index or content. A differing submodule
refreshes the gitlink path without setting a directory mtime and, when the
current state is Exact, the child's own tracked files; a checkout-path change
refreshes those files too, so a local package inside a submodule rebuilds like
any other local input. Submodule ignored files stay outside the limit.

A record without submodule state never vouches for a checkout that declares
gitlinks: version 0 predates the field, and a version-1 record that omits it is
refused the same way rather than read as an empty, equivalent map. Records are
written with content version 1 and always carry the field; the added fields
make an older Oxide discard the slot rather than guess. One snapshot or
prepare is bound to 64 `git` invocations, 60 s of cumulative wait and 16 MiB
of cumulative stdout; each call is a 30 s subcap. Every invocation runs with
`submodule.recurse=0` and `core.fsmonitor=false`, so no configured fsmonitor
hook or daemon inherits its pipes, and the reads and input writer share one
short settle window that never extends past the call's own deadline (the
30 s subcap intersected with the cumulative remainder). A call that ends while
a process Git started still holds its pipes is waited for only within that one
window and then refused, so no reader can hold the gate past its clock.
Exceeding any bound,
more than 32 gitlinks, 200,000 listed files or 4,096 dirty-and-uncertain paths
refuses the slot without a partial fingerprint.

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
- `none` from `oxide status --measure` after Oxide alone, and stray output
  reported after a plain `cargo check`;
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

Help is Cargo's and needs no storage (oxide#182): `oxide <cmd> --help` or
`-h` replaces Oxide with exactly `cargo <cmd> --help` for every leased
command, the building built-ins, `clean`, `init` and `new`, and `oxide help
<cmd>` is the existing plain passthrough to `cargo help <cmd>`. None opens a
pool, takes a lease, reserves a slot, waits for admission, runs the
pre-effect hand-off check (which reads pool control state), clean's own
workspace discovery and planning, or adoption's guidance/readiness work.
Help after `--` belongs to the program (`oxide run -- --help`), not to
Cargo.

1. Resolve the workspace (`cargo locate-project --workspace`), identity,
   profile, strategy and toolchain.
2. Lease a slot from the host pool. Prefer this checkout's last slot, then any
   idle compatible slot, then a new slot if the per-project cap and budget
   allow. Otherwise wait in order, with a visible "waiting for …" line.
   Oxide measures monotonic time from the first observed wait reason through
   each grant, refusal, cancellation or error, and keeps those intervals
   across freshness retries. Registration and the initial attempt are outside
   the measurement; work between later observations is attributed to the
   previous reason. It attributes each interval to the last observed
   exclusive reason (queue, host limit, checkout, clean, project cap,
   uncertain slot or capacity); this is an approximate observation, not a
   root-cause diagnosis. Polling and admission work after an observed wait
   stays with that reason. Cargo, execution and freshness-gate work are not
   measured as waits. A changed reason is printed, up to eight reason lines
   per command followed by one suppression notice.

   **Supervised admission marker** (oxide#192). When the shared proof
   supervisor armed this process — `OXIDE_PROOF_DEADLINE_NS` parses as an
   absolute `CLOCK_UPTIME_RAW` value and the canonical `OXIDE_PROOF_EVIDENCE`
   is nested in, or the `scratch`/`evidence` sibling of, the canonical
   `OXIDE_TASK_SCRATCH_ROOT` — a granted lease publishes one bounded marker,
   `admission.json`, carrying this command's own measured admission interval
   (`{"schema":2,"source":"oxide","run":R,"command":C,"waited_ns":N}`, at most
   512 bytes, written to a private temporary file and renamed). `R` is the
   supervisor's exported run identity and `C` one bounded token for this
   admission attempt, so a report can be paired with its own command and a file
   left by an earlier run is recognisable. It exists so the supervisor can
   report queue time separately instead of spending its work window on it
   ([shared proof supervisor](#shared-proof-supervisor)). When the command
   first observes that it has to wait it also publishes one
   `admission-waiting.json`
   (`{"schema":2,"source":"oxide","run":R,"command":C,"reason":"<token>"}`,
   the same bounds, the reason being its own wait label such as `queue` or
   `host limit`), so a wait that never ends before the supervisor stops is still
   reported; that report grants nothing. Both writes are best effort and
   observation only: an unarmed run writes nothing, a failed write changes
   nothing, Oxide never reads either report back, and no slot, ledger, output or
   exit-code state depends on them. They are compiled in every profile, because
   an installed `oxide` runs the reported case too; they are the proof-facing
   side effects that are not debug-only hooks.
   **Second-slot rule** (Tom, 2026-10-20, oxide#107): when this checkout's
   slot for the same key is busy (a live command such as a long `oxide exec
   -- tauri dev`, or an interrupted one whose group may still run), a further
   leased command in the checkout takes another idle compatible slot (its own
   other slot, else another checkout's through the freshness gate) or a new
   slot through the normal cap, budget and host admission. The busy slot is
   never reused, released, evicted or relabelled. At the per-project cap of 2
   an idle slot of the project may be evicted whole, as for any new slot; with
   no idle candidate and no room the command waits ("another oxide command in
   this checkout"). The host limit, FIFO order, clean barrier and
   unknown-accounting refusal are unchanged. Two slots of one checkout share
   `<workspace>/target`: Cargo serializes the brief final-output uplift on
   `target/<profile>` and the latest final artifact wins.

   A nested `oxide` today takes an independent lease under that second-slot
   rule. Tom approved a design-first change in which a nested command with
   the same workspace, recipe and toolchain joins the outer lease (no new
   slot or host permit) and one with a different key takes its own slot but
   borrows the outer host permit; the
   [nested lease design](nested-lease.md) owns the proposed token,
   validation, closure, accounting, deadlock and idle-exec composition rules;
   its independent design review passed (comment `6412288358`) and the
   operator's design freeze is pending. Nothing above changes until that
   design is frozen and implemented.
3. Run the freshness gate when the slot's last origin differs.
4. Run the real Cargo as the leader of a new process group, with:
   - the build directory pointed at the slot;
   - the target directory set to `<checkout>/target`, so final outputs stay
     private, as in the [local lease](local-lease.md);
   - the profile definitions in the environment;
   - `RUSTC_WRAPPER` set to the empty string, unless the opt-in keeps an
     inherited wrapper ([below](#inherited-rustc-wrapper)).
5. `run`: keep the lease for the build, release it, then execute the binary
   from the private target directory. A bounded `run` (see
   [command time budgets](#command-time-budgets-oxide140)) keeps the lease
   through the program instead. `test`: keep the lease through test
   execution, because test binaries live in the build directory. Reuse never
   skips tests.
6. When the process group exits, record the compile origin, release the lease
   and print one line: profile, strategy, warm/cold, elapsed time, pool use.
   A command that waited first prints `oxide: waited …; slot ready` after its
   final freshness gate and cancellation check, before spawning Cargo or the
   tool. Its summary adds the cumulative measured wait. The existing elapsed
   time still covers its original interval, including gate and execution
   work; it is not a compilation-only duration.

For closeout diagnosis, `OXIDE_TRACE_RELEASE=1` prints a start and elapsed
completion line for Cargo and owned-run group closure, tool-directory notice,
program target resolution, freshness finish, optional seed
publication, admission lock and fence, owned temporary removal, removable
flush, pool open (including its lock and eviction reconciliation), release
walk, ledger write, and origin/control write. A started phase without a finish
identifies the operation in progress, not why it is blocked. The trace is
opt-in, writes to stderr, and does not change custody or deadlines. Group
closure can wait indefinitely without a stop and already reports lingering
members after two seconds. The release walk has a cooperative 60-second,
2-million-entry bound; individual filesystem calls can exceed it. Admission
and pool locks, temporary removal, eviction reconciliation, Git freshness
work, flush and durable writes have no hard elapsed bound. Removable flush
also opens the pool and can wait on its lock or reconcile eviction before the
volume flush itself. Stream writes and scheduling may block too. Release emits
one notice per admission-lock or pool-lock wait after about five seconds even
without tracing, including the removable flush's pool open. An evicting slot in the
unlocked ledger snapshot is identified as recorded, not asserted to be the
current lock holder; unreadable or absent evidence yields `holder unknown`.

### Command time budgets (oxide#140)

Opt-in, leased commands only (`check`, `build`, `test`, `clippy`, `run`,
`doc`, `bench`, the other building built-ins, `exec` and external
subcommands). The three reserved names are Oxide's when they open the
argument list; a tool that owns one of them is reachable through
`oxide exec -- <tool> ...` (or `oxide exec -- cargo <external> ...`), because
nothing after `--` is Oxide's. Other command classes refuse a reserved name
by name.

```text
oxide check --admission-budget 45s --execution-budget 300s
oxide test --execution-budget 90s --budget-events 3
oxide run --execution-budget 120s
```

- **Grammar.** `--admission-budget <dur>`, `--execution-budget <dur>`,
  `--budget-events <fd>`, in `--name value` or `--name=value` form, each at
  most once, no short forms. `dur` is a positive whole number of milliseconds
  or seconds (`30s`, `1500ms`), at most seven days. The two budgets are
  independently optional: an omitted axis is unbounded exactly as before, and
  `--budget-events` needs at least one budget. Invalid, duplicate or
  unsupported-class options refuse before any pool or store effect.
- **Admission** starts once the options are parsed, before the hand-off
  check, `run` selection and every preparation step, and covers preparation
  children, identity and workspace lookup, output custody, lock waits, every
  slot wait or reacquisition and the freshness gate up to a successful
  primary child start. It is one deadline: a grant, retry, eviction or gate
  refusal never grants a new interval. An observed expiry before the launch
  attempt starts no child, and the command exits 124.
- **Execution** arms once, at the observed start of the child, not when the
  caller receives an event. It covers compilation, Cargo's own locks, tests,
  output drain and group closeout. A bounded `run` keeps its lease through the
  program and uses one cumulative window through build, drain, post-build work
  and the program: releasing the lease or launching the program never starts
  a fresh clock. An unbudgeted `run` keeps today's release-then-`exec`
  handoff.
- **Stopping.** An expired execution window is a stop trigger, not an
  immediate kernel guarantee. It latches its origin and reuses the owned-group
  escalation the space watchdog uses (TERM, a two-second grace, KILL, then a
  finite observation bound). The induced child status (143, 137 or even 0)
  never replaces the deadline origin: a controlled stop exits 125, and a
  child that ends 125 on its own is reported as a child exit with no budget
  line. An unconfirmed group or drain is failure 1 with the lease held; a
  successful `release` is not proof that the slot became idle, and a failed
  flush retires it. Before a spawn, Ctrl-C keeps its existing 128+signal
  status.
- **Late starts.** A cooperative launch cannot promise that an in-flight
  `spawn` fails, so a child that starts after the admission deadline is
  reported truthfully as `spawn_late`, latches the admission origin and is
  stopped on the first wait iteration. It never receives a usable execution
  window. Each wait iteration harvests the child, forwards a caller signal,
  then evaluates the deadline, so a child that exited naturally in the same
  poll is reported as that exit rather than as a controlled stop; for `run`,
  the bounded line-read slice is followed by a fresh harvest before the
  deadline is evaluated, so a build whose output settles inside that slice
  also keeps its natural status. The deadline still applies to whatever the
  exit leaves in the group.
- **Events.** `--budget-events <fd>` takes an existing, already-nonblocking,
  writable pipe write end (descriptor 3–1023). Oxide validates the type,
  access mode and flag without changing shared descriptor flags, takes a
  private close-on-exec duplicate and marks the inherited descriptor
  close-on-exec in its own process only. Records are newline JSON, one
  `write` of at most 512 bytes, never retried or waited on. A short or failed
  write disables the stream: a run is unknown unless a complete terminal
  record with contiguous sequence numbers arrived, and a colliding process
  exit code never implies a typed completion. The channel needs the pinned
  Rust runtime's SIGPIPE disposition (parent ignores, children default); when
  the installed runtime differs, a budgeted `--budget-events` command refuses
  before any effect. Event delivery never controls a timer.
- **Nesting and limits.** A nested `oxide` is an ordinary child of the outer
  group: the outer wall-clock deadline keeps running while the outer command
  waits, so a nested call consumes the outer window and never pauses or
  resets it. A nested command has its own group and lease, so the outer
  group's closure does not prove the inner one's; a surviving inner group
  keeps its slot held under the crash-recovery rule. There is no deadline
  inheritance yet. In-kernel filesystem, flush and lock sections, eviction
  walks, standard-stream I/O, scheduling and a `setsid` escapee remain
  cooperative limits; budgets do not make them preemptible.
- **Independently optional axes.** An admission-only command still emits the
  start record (with a null execution axis) and, if its child starts after the
  admission deadline, stops at once and exits 124; a preparation step that the
  window stops is the admission deadline, never an internal error.
- **Debug proof controls.** Two debug-only variables exist for the fixed
  proof and are absent from release builds, which refuse either variable when
  present: `OXIDE_TEST_CLOSURE_STUCK` reports the owned group as unconfirmed
  so the held-custody path is exercisable, and `OXIDE_TEST_SPAWN_DELAY_MS`
  holds the window between the last admission check and the launch so a late
  start is observable. Neither changes a product decision outside a debug
  build.
- **Proof apparatus.** `scripts/test-command-budgets.py` is the fixed proof
  and the whole command of the qualified shared supervisor under heavy
  admission (`bounded-test.py --limit 300 --scratch-budget-mib 512`). Its
  fixtures' pools are scratch pools (scratch-pools.md S4): the script declares
  one private `scratch-pools` directory under the selected-volume scratch root
  as `OXIDE_SCRATCH_ROOT` with an explicit `OXIDE_POOL_CAP`, and a
  `/private/tmp` fallback refuses before any effect. It is
  held for owner activation: the declaration activates nothing. `--plan` is
  effect-free and prints the cases, phases, disk and child-lifetime
  allocation; an unrecognized argument refuses before any subprocess;
  `--self-check` covers the pure boundaries; `--self-test` executes the
  corrected case bodies and the whole-lifecycle fault paths against faked
  process and filesystem seams (no child, image or pool), so a body that
  stops asserting cannot pass it. Every child timeout is clipped to the
  guard's exported deadline minus the reserved cleanup window; fixtures run
  from private checkouts with private `HOME`/`CARGO_HOME`/`OXIDE_CONFIG`/
  `OXIDE_POOL`/temp. Staging is this clean HEAD's debug `oxide`, built
  offline with the pinned toolchain into scratch-owned target and build
  directories at 4 Cargo jobs with 16 codegen units (opt-level `s`, debug
  info off, debug assertions on, incremental off, symbol-stripped); fixtures
  keep one Cargo job, run sequentially and never overlap staging. Those jobs
  and codegen units are configured build parallelism, not a CPU or thread
  hard cap, and they declare no admission demand: the guard's own admission
  and monitoring stay authoritative. `effigy proof:command-budgets:test` runs
  only the script's pure `--self-check`/`--self-test` modes, so it is not
  native and the fit of this recipe stays unmeasured until a separately
  activated diagnostic; the copied candidate must hash-equal the staged binary and
  then answer the built-in `__oxide-build-capabilities` probe (which reports
  `test_controls`) before any private-home fixture exists. The cumulative
  window's oracle is the launched program's own heartbeat counter after a
  ~3 s build under one 5 s window (about 10 beats; a reset window would give
  it about 25), never the command's exit or closeout latency; the nested case has the outer command take the only slot first,
  the inner call wait and end on its own 1 s admission budget with no build,
  and the outer window stop the still-running tool. The late-start case first
  runs one delay-free seed launch on the same private pool, checkout and tool
  argv, and checks the prepared state positively (exit 0, the pool-creation
  notice, recorded control state); the timed launch must reuse that state — a
  second pool-creation notice, a different control root or any pre-spawn expiry
  is an unexercised-fixture failure, never an alternate pass — and a
  delay-free follow-up reuses the released slot. A run writes a uniquely
  named bounded helper receipt beside the guard's own on every outcome,
  including a staging, capability, assertion, interrupt or receipt-write
  failure, with the primary fault, candidate identity when known, reached/
  completed/failed/unexecuted cases and custody uncertainty; that receipt
  never claims guard closure, which the supervisor alone decides. Human budget
  results are recognised only by the product's exact `report_stop`/
  `LOSS_NOTE` grammar: innocent Cargo, project or path output that contains
  the word `budget` (the fixture crates are named `oxide-budget-<label>`) is
  not a stop or a channel loss, and each failing oracle keeps a bounded
  escaped stderr excerpt plus its typed fields. `--diagnostic` is a declared
  but unactivated keep-going collection under the same guard: it records
  recoverable oracle failures and continues only with dependency-independent
  cases (each labels its own private fixtures), while an unknown child or
  writer, custody uncertainty, a prerequisite failure or an exhausted
  deadline stops it; its distinct receipt is `qualified=false` and is never a
  counted PASS. `effigy rust:command-budgets:diagnostic` runs that mode as
  the whole command of the same qualified guard, bounds and custody; like
  the counted selector it is held for owner activation. The only `--locked` invocation is the staging build of this
  repository, whose committed `Cargo.lock` is present; fixture crates are
  std-only and are never invoked with `--locked`. The 300 s/512 MiB fit is
  unmeasured.
- **Locking.** A budgeted command waits for the admission, record and
  checkout-binding locks with the existing nonblocking primitive, polling
  against the remaining window in the unchanged order. Abandoning a wait
  holds no lease and removes the wait ticket. Unbudgeted commands keep the
  blocking calls and their FIFO behaviour; budgeted pollers forgo that
  fairness.

### `oxide exec`: Cargo-wrapping tools

Tom approved `oxide exec` on 2026-10-20 ([vision](../vision.md#product-workflow-direction),
oxide#107). Motivating case: Acowtancy's Tauri app.

```text
oxide exec [--manifest-path <Cargo.toml>] [--profile quick|debug|release] -- <command> [args]
```

- **Parsing.** Oxide's options come before the mandatory `--`; everything
  after it is the command, kept exactly (non-UTF-8 included) and spawned
  directly, never through a shell. A missing `--` or command, a duplicate or
  unknown option, `--release` (use `--profile release`), `--target-dir` and
  `--config` refuse before any lease. `oxide exec --help` prints usage; after
  `--`, `--help` belongs to the tool.
- **Workspace.** From the working directory or `--manifest-path`, through the
  same caller-Cargo and toolchain identity rules as `build`. Neither moves the
  tool's working directory or changes its arguments. One Cargo workspace per
  invocation.
- **Lease.** The same slot-key components (recipe, compiler, target and
  effective incremental setting), freshness gate, admission, host limit,
  clean barrier and reserve watchdog as a leased Cargo command. The gate runs with Cargo's defaults: the tool's own lock or network
  flags are not known to Oxide. A tool that will compile never skips it.
- **Environment.** The same storage, recipe and rustc-wrapper function as
  every leased command (`apply_storage`): intermediates go to the held slot,
  final outputs to `<workspace>/target`, and the [wrapper bypass](#inherited-rustc-wrapper)
  reaches the tool's nested Cargo calls. `--profile` selects the recipe and
  slot only. The tool still chooses its own dev or release build; Oxide
  neither rewrites its flags nor inspects what it built. The release recipe
  is the project's own.
- **Process.** The tool is the leader of a recorded process group. Ctrl-C and
  termination signals are forwarded to the group. The lease stays held until
  the leader has exited **and** the group is empty, so a member that outlives
  the leader keeps the slot held. Crash recovery is [the same](#crash-recovery):
  a killed `oxide` whose tool group lives leaves the slot held and unused,
  recovered by a later command only once that group is gone.
- **Exit code.** The tool's, or 128 + signal when a signal ended it; 127 when
  the program is not found, 126 when it cannot start. A reserve stop still
  fails the command.
- **Input.** The tool's group is not the terminal's foreground group, so a
  terminal read would stop it. A terminal standard input is therefore
  replaced by `/dev/null` (the tool reads end-of-file); a pipe or file passes
  through. Interactive prompts are not supported.
- **Status.** `oxide status` shows the holder as `exec <program>` (control
  characters escaped, at most 160 characters), live or interrupted. The
  summary line names the `build+execute` strategy.

**Limits.** This is a cooperative contract, not containment. A tool that
sets its own `CARGO_TARGET_DIR` (inside the tool, not inherited by
`oxide exec`, which replaces it) or passes `--target-dir` moves only its final
outputs;
its intermediates still go to the held slot. A tool escapes Oxide's storage if
it sets a build directory itself (`CARGO_BUILD_BUILD_DIR` or `build.build-dir`),
changes workspace or toolchain, or builds an unrelated project. A descendant
that calls `setsid` or daemonizes leaves the group: it is not a qualified
writer, and Oxide makes no crash-closure claim for it. The explicit wrapper
opt-in stays unqualified ([#099](#inherited-rustc-wrapper)).

**Evidence.** `effigy rust:oxide-exec` uses a short fixed shell tool that
calls the inherited `cargo build` and `cargo metadata`; `cargo metadata`
reporting `target_directory` as `<workspace>/target` is that fixture's
observation. A real `tauri build` or `tauri dev`, including Vite's
`beforeDevCommand`, is Acowtancy-owned dogfood after merge: expected to work,
unverified here. No Tauri build-directory or output-discovery compatibility,
performance or saving is claimed. The proof runs under the old
`scripts/bounded-test.py` on route B (Tom, 2026-10-21) with its documented
gaps, which do not qualify that script or any shared runner.

**Host-permit lifetime.** An exec session's slot reservation and its host
active-build permit are one record today, so an open session that is not
compiling still consumes a host unit. The interception analysis, the supported
boundary and the open operator dispositions for separating them are in the
[proposed idle-session design](idle-exec-sessions.md); the current session
still holds its permit throughout.

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
  fake wrapper only. The oxide#099 evidence is in its PR and stays historical.
  oxide#131 moved the selector onto the qualified shared supervisor as one
  fixed script: `bounded-test.py --limit 300 --scratch-budget-mib 512` owns
  the absolute deadline, the continuous 20 GiB reserve watchdog through
  plain-Cargo staging and every fixture child, the 512 MiB monitored scratch
  and cleanup. The script keeps no clock, scratch or sampler of its own,
  starts no session and sends no group signal; a child deadline kill touches
  only that direct child PID and reports closure unknown. Closure is the
  supervisor's post-leader session census. Cargo may start its own process
  groups, so no separate-Cargo-group closure is claimed and a `setsid` escape
  is outside the census. Staging seeds HEAD's `Cargo.lock` into a private
  offline `CARGO_HOME` with the [shared seeder](#bounded-offline-lock-seeder)
  and then verifies the private Longhorn checkout at the locked commit; the
  same fixed script first runs that seeder's synthetic cases. The final
  dual-config case is now exercised in source, but no native run of this
  script under the supervisor exists: native activation is separately gated
  (#131), and Tom's acceptance of the product source does not qualify the
  apparatus or substitute for that run.

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
  used to decide. A follower in the same checkout takes another compatible
  slot or a new one under the [second-slot rule](#what-one-command-does-local-executor),
  and waits only when no safe slot exists; the held slot is never touched.
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
- The first command creates the pool automatically ([W-1](#open-decisions))
  for the built-in default and a config-file path. An absent pool named by
  `OXIDE_POOL` is an explicit path and refuses instead of being created
  silently, naming the path (oxide#182); `OXIDE_POOL_CREATE=1` or a path
  strictly inside the caller's canonical `OXIDE_TASK_SCRATCH_ROOT` opts in
  (proof fixtures), and a value ending in a `status` display annotation
  (`<path> (from …)`) refuses with a hint that the annotation is not part of
  the path, for an absent path and for an existing directory that holds no
  Oxide pool alike. An existing valid pool at a normal path and the
  config-file path are unchanged.
- Slot headroom (oxide#118, oxide#177): up to 256 slots per pool and twelve
  per project identity, with compatible reuse first and the oldest eligible
  idle whole slot replaced under count or byte pressure
  ([local lease](local-lease.md#slot-headroom-and-ranked-idle-replacement-oxide118)).
  A host-wide limit still bounds concurrent Cargo invocations
  ([W-2](#open-decisions)). Queue/Nucleus still decide which tasks run. Oxide
  only orders the Rust work they launch.
- `oxide status` is diagnostic and read-only (oxide#128). By default it
  reports recorded facts only: the pool config and ledger, the ledger's
  recorded slot directories, the control state's holders, waiting tickets,
  clean marks, latest use and flushes, and the registration's drive mode. It
  takes no pool lock, creates no lock, reconciles no recorded deletion, runs
  no accounting walk, invokes no Cargo, `tmutil` or `diskutil`, and writes
  nothing. It reports two **separately labelled** recorded sources that are
  never added together: the ledger's closed per-slot observations
  (oxide#146), shown as one `recorded slot sizes (ledger)` total over
  `recorded n of m idle slots` with its unrecorded count as a stated lower
  bound — this one *is* the admission basis — and, per slot, an oxide#128
  sidecar fact from a previous acquiring command's reuse walk, labelled with
  its age, source and lease, or `not measured`. Neither is free space, and a
  recorded figure never itself establishes admission. A recorded `Evicting` slot is reported
  as a pending reconciliation, distinct from idle and held, and is left in
  place. Cargo-home and stray-output lines say not measured.
- `oxide status --measure` runs the existing bounded walks once: the
  whole-payload union and each slot's own walk (the same entry, depth and
  elapsed bounds admission uses), then the checkout stray-output scan and
  the Cargo home, all under one cumulative 60 s command budget. It shows
  current bytes and coverage, still without the pool lock, reconciliation or
  any write, so a recorded deletion stays pending and lowers coverage to
  unknown. It does not refresh the recorded observations. Both modes reuse
  the read-only [storage inventory](storage-inventory.md) bounds; each slot's
  file names, distinct inodes and allocation are counted once per inode. Stray
  output means a checkout `target/` holding `deps/` or `incremental/` was
  built by plain Cargo; finding none reads `none` only under a
  [verified Cargo release](#cargo-version-matrix), otherwise `unknown`.
  Reclaiming space is an explicit command; live cleanup still needs scoped
  approval. Nothing inside a slot is ever deleted.
- Recorded observations live in the auxiliary
  `<pool>/oxide-managed/status-observations.json` sidecar (schema
  `oxide.status-observations/1`, at most 512 KiB and 256 retained recent
  facts). It is diagnostic, not pool, ledger, registration or receipt format:
  an older binary may ignore it. Each fact binds `store_id`, slot ID,
  incarnation, the exact recorded directory, wall-clock time, allocated
  bytes, source and the measuring lease. Only walks an acquiring command
  already made are recorded, atomically and with no extra walk: the reuse walk
  with its figure, and an admission walk that stopped partial with no figure at
  all (source `admission-walk-incomplete`, whose `allocated_bytes` is written
  as 0 and read by nothing; oxide#186), which status reports as `not measured
  (walk incomplete)`. A release's ledger record is reported before any of these
  facts, so a later complete walk supersedes them. Persistence failure changes
  no admission, ownership or exit status. A
  missing, unreadable, foreign, corrupt, unknown-version or
  generation-mismatched sidecar reads as not measured and never falls back
  to a scan. A same-generation fact stays a historical observation that may
  have changed since measurement; a future timestamp is an unknown age. The
  retention cap is at least the slot ceiling and never lowers it.
- Capacity refusals may add bounded, read-only macOS figures (oxide#130).
  After a pool-creation reserve refusal, and after a host-reserve stop once
  every writer in Cargo's group has positively closed, Oxide runs its own
  executing artifact as the hidden `oxide __oxide-capacity-probe
  <absolute-path>` child: a separate process group, one 2 s budget taken
  before the spawn and shared by the answer, a bounded KILL, its positive reap
  and the pipe drain (the reap reserve is carved out of that budget, never
  added to it), at most 8 KiB of combined stdout and stderr, and one typed
  JSON answer. The child reads CoreFoundation URL resource values
  (`CFURLCopyResourcePropertyForKey` with the public
  `NSURLVolumeAvailableCapacityForImportantUsageKey` and
  `NSURLVolumeAvailableCapacityForOpportunisticUsageKey` keys) and reports
  each as optional bytes or `unknown`. An explicit error class, a timeout, an
  oversized or malformed answer, and an absent, negative or wrongly typed
  value are all `unknown`. The figures overlap and are not guaranteed free
  space: the line is labelled informational, nothing is purged, no user-data
  deletion is suggested, and no decision reads it. The raw 20 GiB start,
  abort and reserve decisions are unchanged; no probe runs on plain `oxide
  status`, on a watchdog sample, during polling, or without confirmed writer
  closure. Linux keeps its raw refusal text.
- `OXIDE_POOL_CAP` or the config file sets the soft budget for the next
  admission; `oxide clean --cargo-cache` asks
  Cargo for its own home cleanup ([Cargo home hygiene](#cargo-home-hygiene)).

### Child temporary files (oxide#136)

Implemented. It owns where a leased command's children put temporary files;
it is not final output, final-output placement or a claim of saved writes.

- **One rule, two answers, no third.** Before any writer starts, a leased
  command decides once for its whole child tree:
  - an inherited `TMPDIR` is **kept** when it canonically resolves to an
    existing directory this user can write and that directory is on the
    **same device** as the selected pool's payload root (the actual
    selection, so the internal pool's device when the drive is absent);
  - otherwise the command gets one Oxide-owned `0700` directory,
    `<slot>/scratch`, inside the held slot.

  `TMPDIR`, `TMP` and `TEMP` are all set to the chosen value on every child
  Oxide launches for the command, so a conflicting alias never survives and
  a value this decision rejected never reaches a child. The choice is made
  after the lease and before the spawn, so the child environment never
  influences workspace classification.
- **The kept directory is the canonical one.** Children receive the resolved
  absolute directory, never the raw text: a relative or symlinked `TMPDIR`
  would otherwise be re-resolved against each child's own working directory
  after a `chdir`. Directory identity is the resolved device and an existing
  writable directory, never a path prefix.
- **An inherited directory is never Oxide's.** Oxide creates nothing inside
  it, writes no management data into it and never removes or cleans it; the
  owning tool keeps its own retention rules (an Effigy per-run scratch root
  stays Effigy's). The caller's own environment is unchanged.
- **Where an owned directory is prepared, and what it covers.** Cargo (every
  leased command, `run` and `test` included, and modelled external
  subcommands through it), `oxide exec`'s tool, and a converted checkout's
  owned `run` program under its held lease all receive it; descendants — test
  binaries, build scripts, `run` programs, tools' own Cargo calls — inherit
  it. An unconverted `run` whose children use an **owned** directory keeps
  the `oxide` lease for the program, because the program is still that
  directory's writer: it runs under the existing owned-program custody — its
  own process group named in the record, signals forwarded, the removable
  pool's removal watchdog, and release only after the group is observed
  empty, which is also the one point where the directory is removed. A
  released-lease `exec` is used only when no owned directory exists (the
  kept inherited one, which outlives the lease and is nobody's to clean).
  Ownership markers and the slot incarnation prove *whose* directory it is,
  never that a writer has closed, so nothing is reconciled merely because a
  lease is available.
- **Not covered, deliberately.** Effect-free work keeps the inherited
  environment and creates nothing: plain passthrough (no lease), `--version`,
  probes, `status`, `oxide clean`/`init`/`new`'s own Cargo calls, and the
  freshness gate's `cargo metadata` and lockfile calls, which run before the
  choice. `OXIDE_TASK_SCRATCH_ROOT` and the four-variable macOS validation
  contract keep their meaning; when the selected pool is on another volume,
  the child's three variables follow the pool (the adopted scratch root
  itself is never deleted or redirected).
- **The owned directory is bound and short.** Its ownership marker records
  the pool's store identity, the slot and its incarnation (the lease that
  created it is recorded too), is written before any writer starts, and
  decides every later deletion: a leftover bound to the same incarnation is
  reconciled by the next lease of that slot, and anything else — another
  incarnation, a missing or unreadable marker, a link, another owner or
  device — is reported and kept, never deleted. The directory is created
  `0700`, refuses to follow a link, and lives inside the slot, so the
  slot's own recorded figure counts it exactly once (hard links included),
  whole-slot eviction and crash recovery delete it with the slot they already
  own, and a held, lost or quarantined slot keeps it.
- **Lifecycle.** It is removed at release, after the command has seen its own
  process group empty and before the slot can go idle; a removable slot is
  flushed after that removal. A removal that cannot be positively completed
  keeps the slot held and reported, exactly like an unconfirmed closure, and
  a later command recovers it. A `run` program with an owned directory is
  inside that same lease (previous bullet), so a competing command cannot
  take the slot, reconcile the directory or have it evicted while the
  program lives. An `oxide exec` tool, a kept rustc wrapper or an interrupted
  command obeys the existing custody rules unchanged; a crash leaves the
  directory with the held slot.
- **Length is a limit, not a preference.** Unix-domain sockets need a short
  path. The injected path is the held slot plus `scratch`; a path over 91
  bytes (macOS `sun_path` is 104 bytes including the NUL, less `/` and an
  eleven-byte socket name; Linux allows 108) is reported with its measured
  byte count and the command **refuses before spawning** rather than running
  its children on the machine default this decision rejected. No alias,
  symlink or settings change shortens a configured pool path.
- **Visibility.** A leased command's summary carries the bounded, escaped
  fragment `temp: inherited <path>` or `temp: oxide <path>`. `status
  --measure` names the choice the next leased command would make on the pool
  it verifies; recorded `status` states the rule and never opens the volume
  to answer it (oxide#128).
- **Scope.** This is not a general temporary-file redirection: it applies only
  to children Oxide launches for leased commands. CI, Effigy's own run
  scratch, Queue worktrees, container scratch, `$HOME` and host or global
  settings are untouched, and no other-platform default is guessed.

The host permit a long-lived `run` program holds is a separate axis, and the
successor alternatives for its slot and temp custody are analysed in the
[proposed idle-session design](idle-exec-sessions.md). Nothing there is
implemented: this guarantee stays current until an independently reviewed
successor design establishes equivalence.

**Proof.** `effigy rust:pool-temp-scratch` is one fixed script,
`scripts/test-pool-temp-scratch.py`, and the whole command of the qualified
shared proof supervisor (`bounded-test.py --root-parent /private/tmp --limit 300
--scratch-budget-mib 512`; an ELEVATED trigger: it deletes owned scratch, decides
writer closure and writes to an owned removable fixture volume). It stages
this clean HEAD's debug `oxide` offline with 1.97.1 and covers, on tiny
fixtures with a private `HOME`, `CARGO_HOME` and directory pool. Decision af7df7d6 keeps the explicit internal root because this fixture's shortest admitted slot scratch (`test-1`) under the selected Builds prefix is 103 bytes, above the 91-byte socket bound. The source-derived internal layout is at most 88 bytes for the two-digit slot IDs the proof admits, a 3-byte margin; case 12 verifies the actual names and reports the run figures. No child receives `OXIDE_SCRATCH_ROOT`. Staging and
every fixture child share one resolved absolute toolchain home — an inherited
`RUSTUP_HOME` when it is absolute and usable, else the account default
`$HOME/.rustup` resolved from the original HOME *before* the private fixture
HOME exists — with the pinned 1.97.1, `RUSTUP_AUTO_INSTALL=0` and offline
Cargo; an unusable toolchain home refuses before scratch or staging rather than
hiding the toolchain behind a fake HOME (the first authorised author native
failed exactly there). Fixture children carry the debug build's
`OXIDE_TEST_HOME_FROM_ENV=1` account-home control, a freshly built environment
that imports no ambient `OXIDE_*`/`XDG_*`/`CARGO_ENCODED_RUSTFLAGS`, and one
frozen `RUSTFLAGS` shared with staging; the script's `--self-check` is a pure,
pre-scratch pure regression of that construction (`--self-check`). Before any
`--locked` invocation, the fixture constructor establishes the crate's lock
precondition with one bounded offline `cargo generate-lockfile` setup child in
the fixture's private Cargo home (the package has no dependencies, so nothing
is resolved and no network is used), and a pure predicate requires the lock to
be manifest-consistent — a missing, header-less, mismatched or extra-package
lock is refused rather than treated as valid. The uncounted
`--diagnostic` mode records a recoverable oracle failure and continues only
with cases whose declared dependencies are intact, skips every later case
after a shared-state failure, aborts on any unknown child/device/writer
outcome, and writes a bounded receipt labelled `qualified: false`; the default
run still fails fast. It is dispatched only through
`effigy rust:pool-temp-scratch:diagnostic` (selectors do not forward
passthrough arguments), is required before the next counted native run, and a
source sweep complements it but never replaces it. Inherited-`TMPDIR` fixtures are planted in a sibling fixture directory inside
the same monitored root, never inside the directory-budget pool, because the
product correctly refuses a pool directory that holds unexpected entries. The
proof covers: a
same-volume inherited `TMPDIR` kept canonically with no Oxide directory
created and the inherited content untouched; absent, file, relative,
unwritable, other-device and symlink-escaping values replaced by one owned
directory; an **existing relative** value resolved against the parent's
working directory and delivered canonically; aliases following the chosen
value in both branches; the owned directory observed on the selected pool
while a gated writer holds it, with a hard-linked pair counted once against
the measured slot figure (16 MiB unique must stay below 24 MiB) and the held
scratch untouched by `status --measure`; normal release removing it; an
injected crash whose writer group is observed closed from outside the way the
product observes closure (`kill(-pgid, 0)` on the group the fixture reports),
after which a later command neither adopts nor keeps the dead command's
leftover — this slot's same-incarnation directory is reconciled by its next
lease, or the untrusted slot is evicted whole, and both existing rules are
accepted outcomes rather than one assumed mechanism; `oxide
test`, `oxide exec -- <tool>` and `oxide run` each observing the chosen
directory, with a **live** `run` program plus a competing `oxide check` in the
same checkout proving the competing command takes another slot and the live
directory and its bytes stay intact until that program's own release after its
group closes; a foreign entry, a link and an over-bound
derived path each refusing the command without spawning, deleting or
following anything; and one owned 128 MiB scratch APFS image used as the
removable pool, where a build's temporary files land on the image, then an
ordinary detach selects the original directory pool's scratch with no stale
`/Volumes/<name>` directory written. It reports the longest injected path in
bytes and characters against the bound. It never touches a live pool or the
registered drive, and it is not a benchmark or an alias for full QA.

Fixture lifecycle (corrected after diagnostic3): the constructor creates only
the account homes, the crate, its own scratch and the sibling fixture
directory, and never the pool directory. The first production `oxide` command
that needs the pool creates it, and the proof then verifies the created
`config.json` backend, payload, ledger, device and the inherited-directory
observation, so the product's own applicable admission gates are the oracle rather than a
separate probe. Every fixture launch mints a fresh observation identifier that
the build script declares to Cargo and echoes, and every consumed observation
is refused unless it carries the current identifier, so a replayed warning from
a warm affine slot can never satisfy a kept/replaced oracle. Gated writers
publish a small atomic readiness record (identifier, chosen directory, process
group) before waiting, because Cargo does not stream build-script warnings while
the script runs; the proof polls that record inside the exported deadline and
writes asynchronous child output to owned files instead of undrained pipes. The
held-slot accounting oracle measures a baseline and a delta around the planted
hard-linked pair, the measured status query uses the same unset aliases as the
held child, the refusal target slot comes from the preceding fresh observation
rather than directory order, the crash fixture stays in the supervisor session
and kills only `oxide`, and the one owned image's custody is derived from the
supervisor's own published pair: the canonical `OXIDE_TASK_SCRATCH_ROOT` and
`OXIDE_PROOF_EVIDENCE` must be the `scratch` and `evidence` siblings of one
non-shared, non-group-writable owned root, this proof's scratch workspace must
be inside the published scratch, and the custody directory must stay inside
that evidence directory — all verified before the custody directory, the image
or an intent record exists, with no fallback to the workspace root. The backing
is deleted only after a positively successful ordinary detach and its matching
post-state; the planned detach records its attempt before the call and is never
replayed, and any failed, timed-out or unclassifiable detach or cleanup keeps
the backing and records and aborts as an unknown outcome instead of being
downgraded to the case's own oracle failure. Diagnostic dependency
handling requires a *passed* predecessor (a skipped one is not passed), and an
unknown-outcome abort records every remaining planned case as not run, so a
reached prefix is never reported as a complete collection. At native head `0eeb7404020e86d3b040578d731c232f229733a1`, the
Supervisor-activated uncounted diagnostic completed 12/12 and the counted
author and distinct independent reviewer natives each passed all 12 cases,
closed under `/private/tmp` at the 300 s / 512 MiB bounds. The three runs
used direct commands under decision `d3d61247`, in order, and are recorded
with executor, command, load and log hashes in
`/Volumes/Builds/scratch/oxide/evidence-s4-pool-temp-scratch-0eeb740-20261101T123940Z`
(`NOTE.md`, `sessions.jsonl`, `SHA256SUMS`): 68 s diagnostic, 67 s author,
68 s reviewer, with reviewer peak 71.6 MiB. Carry to the integrated head
requires independent exact-diff confirmation under decision `0475a633`.
Author natives 1 and 2 and diagnostics 1, 2 and 3 at earlier heads are spent
failures: the first two hid the toolchain behind the fixture HOME and used
`--locked` without a lock, and the diagnostics were refused in turn by an
external-volume internal pool, inherited-temp fixtures placed inside the pool
directory, and a pre-created empty pool directory. Diagnostic 4 then reached
cases 1-10 and aborted at the removable case, whose containment check compared
the custody directory against the workspace and its parent instead of the
supervisor's published owned root; that topology and its custody decision are
corrected here, and the run stays a spent failure. The pure self-check
(`effigy proof:pool-temp:test`, 153 properties) covers the constructor layout,
rerun completeness, observation freshness, readiness parsing, refusal
targeting, accounting deltas, the published-root containment refusals, the
removable flow's custody decisions (including a missing or swapped device
binding, a replayed observation identifier, both ambiguous detach post-states
and a deadline abort raised inside the cleanup decision) and first-fault
precedence, case 12's socket-bound oracles with fake product states, and the
diagnostic dependency and abort rules with no scratch, child or toolchain.

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
   guidance root's `AGENTS.md`, a few lines long: the Git work-tree root inside
   Git (listing the Cargo workspaces it covers), else the project's own. The
   block is the opt-in marker; no config file exists.
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
   the guidance root's `AGENTS.md`, creating the file if absent. The guidance
   root is the Git work-tree root when the workspace is inside Git, else the
   workspace itself (see [placement](#placement-of-the-guidance-block)).
   Re-running is idempotent and never touches text outside the markers.
4. Print readiness: the workspace and guidance root, covered paths, any
   retained older block, resolved toolchain, inferred project identity, the
   pool a build would select and the configured internal pool (both not
   measured), and any Effigy Rust selectors that still call plain `cargo`.
   The selected line agrees with `status`'s recorded selection without
   opening the pool, taking its lock or verifying the volume (oxide#182):
   `selected pool: removable <mount> (registered, mode: …; the next build
   verifies the volume)` when the registration's mount point is present, or
   `selected pool: internal <path> (the registered removable pool <mount> is
   absent; …)` / `(no removable volume registered)`, followed by
   `configured internal pool: <path> …`. Oxide reports those; it does not
   edit them.

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
- The workspace is the root `cargo locate-project --workspace` reports for the
  target's manifest. When Cargo cannot resolve one, for example a package
  inside a workspace that does not list it, Oxide refuses and writes nothing;
  Cargo's own init result stands.
- Readiness never creates the pool, opens its ledger or edits a file. The
  pool line names the path and says `use not measured`; `oxide status`
  measures.
- With a manifest present, every argument other than the directory refuses as
  Cargo-only. `--help` passes to Cargo.
- The Effigy report parses each `effigy.toml` from the workspace root up to
  the Git top level (bounded by the same 1 MiB file cap) and classifies its
  executable plain-Cargo routes (oxide#130). A selector counts only through
  the command it runs: a `[tasks]` string value, a task's or suite's `run`
  string or list. A `[tasks]` key ending `:cargo`, or a `[test.suites]` name
  ending `-cargo` or `:cargo`, that runs plain `cargo` is an intentional
  escape and is acknowledged once per invocation, grouped by name and
  separately from the warnings. Every other executable selector that runs
  plain `cargo` stays an actionable warning with the selector name and
  command. Comments, `depends`-style references, `cargo-*` binaries and
  `$CARGO` never count, nothing is edited, and no acknowledgement is
  persisted. A file that is not parseable TOML, or whose `tasks` or suites
  shape cannot be read, is reported honestly as unclassified with its
  calling lines instead of being placed in either group.

### Placement of the guidance block

Placement is classified after Cargo's `init`/`new` has run, so a repository
Cargo just created counts as Git. Oxide asks `git rev-parse --show-toplevel`
in the workspace, with a 5 s limit, 64 KiB of stdout, inherited `GIT_*`
variables cleared and `LC_ALL=C`.

- **Outside Git.** Only a missing `git` or Git's own
  `fatal: not a git repository` answer (exit 128) counts. The block goes in the
  workspace's `AGENTS.md`, with no list, as before.
- **Inside Git.** The work-tree root `T` (canonical) holds the one block in
  `T/AGENTS.md`. Oxide never creates a block in a nested workspace's
  `AGENTS.md` when `T` differs from it. The block lists the covered Cargo
  workspaces, relative to `T`, in one marker line:
  `<!-- oxide:workspaces: . crates/app -->`. `.` appears only for the root
  workspace itself. The list is sorted and unique, at most 64 paths of 1024
  bytes each. Each path is slash-separated components of ASCII letters, digits
  and `- _ . + @`, with no empty component and no component starting with `.`.
  Whitespace, controls, backticks and other characters are refused.
- **Merge.** Init adds the current workspace to an existing valid list. A legacy
  block with no list covers only `.`, and only when `T/Cargo.toml` is a
  positively inspected regular file. A duplicate or malformed list, a duplicate
  block, or unbalanced markers refuses with nothing written.
- **Lock and writes.** Read, merge and write happen under an advisory `flock`
  on the opened read-only `T` directory, waited for 10 s without blocking. The
  file is then replaced atomically and keeps its permissions. The lock
  serialises cooperating `oxide init`/`new` runs. It does not stop unrelated
  edits, and no lock file is created or recovered.
- **Probe failure.** A timeout, a malformed answer, any other Git exit, or an
  unreadable result refuses before any write. Oxide never writes nested
  guidance on an uncertain answer.
- **Older nested blocks.** A workspace's own `AGENTS.md` holding an Oxide block,
  or malformed markers, is reported with its exact path as retained. Oxide
  never deletes or rewrites it.
- **Adoption.** A workspace is adopted when the Git root's list covers it. A
  legacy root block without a list covers only `T` with a regular `T/Cargo.toml`.
  A nested legacy block alone is not adoption. `oxide clean --cargo-cache`
  refuses it, names the path and says to run `oxide init` at the Git root.
  Outside Git, the workspace's own block still decides adoption. An uncertain
  probe or an unreadable file is not adoption.
- **Proof.** `effigy rust:init-guidance` (oxide#127) covers the matrix above on
  tiny owned repositories under the qualified supervisor. Its native run is held
  for owner activation, so no native result is claimed here.
- **Local outputs.** `/.oxide-outputs/` still goes in the workspace's own
  `.gitignore`, which keeps its Git meaning there.

### Switching an existing project

The standard pattern for consumer projects (Tom, 2026-10-20, [vision](../vision.md#product-workflow-direction)). It was first applied to Underlay, Praxis, Longhorn, Figmatic and Acowtancy; Queue lead `f4813b68` tracks the rollout.

Tom ruled on 2026-10-25 that every Rust project in `~/Dev/projects` moves fully to Oxide, and that per-selector `:cargo` escapes are removed. This supersedes the earlier sibling rule and exception list.

- **Selectors.** Every ordinary build, check, clippy, test, nextest and format route calls `oxide`. There are no `:cargo` sibling selectors and no per-project escape variables. The one machine-wide escape is Effigy's per-program fallback (effigy#250) in the user config `~/.effigy/config.toml`: `[programs.oxide] fallback = "cargo"`. It is installed with `force = false`, so it acts only when `oxide` is missing from `PATH`; while `oxide` is broken, set `force = true` and set it back afterwards. It runs `cargo` with the exact arguments, so it covers Cargo-shaped commands only, not `oxide exec` or Oxide-only flags. Effigy's built-in test route also honours `EFFIGY_OXIDE_ESCAPE=cargo`. Otherwise, run `cargo` by hand.
- **CI-aware rule.** When CI or release workflows call the same Effigy selectors and the runners lack `oxide`, the selector runs `cargo` under `GITHUB_ACTIONS=true` and `oxide` otherwise. Locally, a missing `oxide` fails loudly and names the escape; it never falls back silently. Workflow files do not change. Installing `oxide` on runners waits on publication (Q-005).
- **Tools and private target directories.** A tool that runs Cargo itself (the Tauri CLI, packaging scripts, Python or TypeScript drivers), or a script that chooses its own `--target-dir` or `CARGO_TARGET_DIR`, runs under [`oxide exec`](#oxide-exec-cargo-wrapping-tools). Its inner Cargo keeps its own target directory for final outputs when the tool passes `--target-dir` or sets `CARGO_TARGET_DIR` itself, and its intermediates go to the held slot through the inherited build directory. A `CARGO_TARGET_DIR` set in the environment of the `oxide exec` call itself does not survive: exec exports `<workspace>/target` in its place. Move such a value into the tool's own command (`--target-dir`) or its child environment.
- **Disposable staged workspaces.** A stage outside Git, or one whose `.gitignore` Git cannot confirm for the member links a first conversion publishes (`target/debug`, `target/release`), is simply an unconverted checkout: the leased command builds into the stage's real local `target/`, intermediates go to the slot, no output root is created and exactly one unconverted notice is printed. No prepared target is needed (oxide#154 removed the interim recipe that pre-created an empty `target/`). An existing real target is still the per-checkout opt-out: it is never adopted, with or without Git. Keep stages under the drive's `scratch/<project>/` or `OXIDE_TASK_SCRATCH_ROOT`.
- **Named proof exceptions.** Only a proof whose subject is Cargo's own target layout (for example a scanner that reads `target/<profile>/deps`) or that must build without any shared intermediate state may set its own build directory. It runs under `oxide exec`, places that private directory under the drive's `scratch/<project>/` or the proof's `OXIDE_TASK_SCRATCH_ROOT` (never `/var/folders`, `~` or a drive root), removes it afterwards, and is listed by name in the project as an intentional proof exception.
- **Stays on Cargo:** CI and release workflows (above), container builds inside Linux images without `oxide`, product code that runs Cargo as part of its own behaviour, and test fixtures.
  - tests that find a sibling binary by going from `current_exe()` in `deps/` to its profile directory. With Cargo's build-dir, test executables run from the slot (`<slot>/build/<profile>/deps`) while final binaries are uplifted to `<workspace>/target/<profile>/`. This holds for any build-dir user; #106 moved only the final outputs to `target/debug/`. Repair: integration tests use `env!("CARGO_BIN_EXE_<name>")`; other code also tries `$CARGO_TARGET_DIR/<profile dir name>/`, which Oxide exports. Found by Figmatic's post-merge QA on 2026-10-20: 7 studio tests failed with "no capture renderer beside …/slots/4/build/oxide-quick".
- **nextest.** `oxide nextest run` with no profile flag builds with the quick recipe in the quick slot. Without `CARGO_INCREMENTAL`, its
  incremental setting is unknown and it shares the unknown-setting slot with
  compatible `oxide exec` commands. In a profile-layout checkout the external subcommand derives its member set from the committed receipt like `oxide exec` (oxide#164), so it runs instead of refusing, and any directory the tool owns that is not a member (`target/nextest`) stays a real local directory reported once. Drop an existing `--cargo-profile oxide-quick`: it still builds, but into `target/oxide-quick/`.
- **Final outputs.** Since #106, `quick` and `debug` binaries are in `target/debug/`, so scripts that read them there can move to `oxide`. Earlier adopters' `target/oxide-quick/` directories are left alone; `oxide clean` removes them with the rest of `target/`.
- **Worktree seeding.** Selectors that move to `oxide` drop any copy of a primary checkout's `target/` into worktrees. The shared slot is the warm state.
- **Proof per project:**
  - the moved selectors pass under `oxide` and, where the CI-aware rule applies, under `cargo` with `GITHUB_ACTIONS=true`;
  - `oxide status` shows the project's slot and which pool served it;
  - one cold run and one warm repeat are timed, as observations.
  - Anything that changes a test result stays on `cargo` and is reported as a dogfood finding.

### Writing briefs in adopting repositories

Planners writing briefs in an adopted (or adopting) repository follow this
standing guidance (Tom, 2026-10-22, decisions 3b041cf4 and 89797220;
Tom, 2026-10-25: every Rust project moves fully to Oxide). It is carried
here from the Northstar brief skill so this contract owns it; the mechanics
above stay the reference, and this list links to them rather than
restating them.

- Move fully to Oxide: ordinary build, check, clippy, test, nextest and
  format routes call `oxide` with the same arguments, as do metadata,
  locate-project, fetch, update, install, package, publish, tree, fmt, deny
  and audit. There are no per-selector `:cargo` escapes. Changing an entry
  point authorizes no actual publish, install, release or secret use.
- A brief in a repository still on plain Cargo also runs `oxide init` at its
  existing package or workspace, adopting that workspace rather than adding a
  root crate. This is standing approval: never ask the operator per
  repository. The task still gets its normal checks and review.
- Keep toolchain pins: `cargo +<toolchain>` becomes
  `RUSTUP_TOOLCHAIN=<toolchain> oxide` or a `rust-toolchain` file. Tests
  that find sibling binaries through `current_exe` use
  `env!("CARGO_BIN_EXE_<name>")` first, or read
  `$CARGO_TARGET_DIR/<profile>`, because Oxide runs test executables from
  the slot and uplifts final binaries to `target/`.
- A tool or driver that runs Cargo itself runs under `oxide exec --` with
  its command intact (see [`oxide exec`](#oxide-exec-cargo-wrapping-tools)).
  Change the route, never the product or fixture semantics it drives. A
  driver or fixture whose target is throwaway keeps it inside the temp root
  it removes.
- A proof whose subject is Cargo's own target layout, or that must build
  with no shared intermediate state, is a named proof exception: it still
  runs under `oxide exec`, sets its own build directory under the drive's
  `scratch/<project>/` or the proof's `OXIDE_TASK_SCRATCH_ROOT` (never
  `/var/folders`, `~` or a drive root), removes only that directory
  afterwards, and is listed by proof name with its preserved intent in the
  project's guidance.
- Stays on Cargo: CI and release workflows, container and VM guest builds,
  product code that runs Cargo as part of its own behaviour, and test
  fixtures.
- The only escape is explicit: `EFFIGY_OXIDE_ESCAPE=cargo`, or running
  Cargo by hand while Oxide is broken (see **Selectors** above for the
  machine-wide fallback). Nothing falls back silently.

## Dogfooding in Oxide's repository

Approved by Tom on 2026-10-19 ([vision](../vision.md#product-workflow-direction)).
Oxide's own repository is the only adopter. No skill, other project, consumer
or global Cargo configuration is involved.

- **Installed binary.** `~/.local/bin/oxide` is a regular file built with
  `cargo build --release --locked --offline --bin oxide` from a commit on
  pushed `origin/main`, never from a branch. `effigy oxide:install`
  authenticates the pin against `git ls-remote`'s advertised main tip, which
  it fetches into a private per-invocation ref under
  `refs/oxide-install-auth/` deleted again in the same call; the deciding
  identity never comes from shared `FETCH_HEAD` or a remote-tracking ref, so
  concurrent fetch activity cannot change it, and remote main advancing
  between advertisement and fetch — or a tip that is not a commit — is a
  clean refusal the install can simply retry (oxide#302). It builds the pin
  from `git archive` in
  a private `/private/tmp/oxide-install.*` staging directory, installs through
  a sibling temporary file and `link` (no clobber), and writes a JSON receipt
  under `~/.local/state/oxide/install/`. The receipt records pin, tree,
  toolchain, build command, path, owner, mode, size, time and SHA-256, plus
  that build's own `control_state_version` and `registration_version` (read by
  the effect-free hidden probe described under
  [installed-binary hand-off](#installed-binary-hand-off-oxide122); a build
  that cannot report them is refused before installation). It refuses when any
  `oxide` is on `PATH` or the destination exists, and when
  any directory from `HOME` to `~/.local/bin` is a link, is owned by another
  user or (below `HOME`) is group or world writable. It writes only through
  the opened directory, with no-follow opens.
- **Updating.** After a reviewed or explicitly operator-accepted merge, the
  planner runs `effigy oxide:reinstall`. It replaces only a regular,
  user-owned binary whose hash matches the newest receipt. Directly before
  the atomic rename it rechecks that the destination is still that file
  (type, owner, inode and hash) and refuses otherwise. Earlier receipts are
  kept. `OXIDE_INSTALL_PIN=<commit>` selects a commit on pushed main; it does
  not roll back ledger or control state, and an older binary may refuse
  forward-only fields. Effigy does not forward selector arguments, so the
  pin travels in the environment.
- **Install compatibility window.** Before an update, classify whether its
  ledger or control-state format adds a forward-only fence that older
  binaries refuse. For such updates, the planner obtains a fresh supported
  Queue observation showing no milestone or project QA running before
  installing. Inspect Queue's QA index together with host-run records and
  exact QA details, including queued, admitted, running and capacity-retaining
  runs; an empty latest-run list alone is insufficient. Queue currently has
  no bounded QA-launch deferral: these observations cannot atomically exclude
  preparing requests or a new launch between the check and swap. Coordinate
  that short window with Queue's owner and retain the capability gap. If a
  supported bounded deferral becomes available, use it and release it when
  the install settles. Do not substitute a global pause, cancellation,
  preemption or an unapproved scheduler setting. Preserve the observation
  and install receipt. This is a state-compatibility condition, not a
  quiet-machine or CPU-load gate. Updates with no state-format change keep
  the atomic install with no idle wait. Atomic replacement preserves an
  already-running binary's inode, but cannot make it understand newly
  written state: old commands can refuse an early state read as well as a
  final release. Keep those failures and any held records for the existing
  positive-closure recovery path; do not overwrite the new format or infer
  writer closure from the refusal. The
  [installed-binary hand-off](#installed-binary-hand-off-oxide122) adds a
  bounded recovery for commands that do run under an older binary, but it
  cannot repair a binary built without it, so this window policy stands.
- **Default pool.** The first leased command created
  `~/Library/Caches/oxide/pool` under the then-shipped rules: a 24 GiB
  `apfs-sparse-image-v1` image, refused below the 20 GiB host reserve. That
  backend is retired (oxide#102): current `oxide` never attaches, writes or
  removes that pool and `status` lists it as `legacy pool`. Its ordinary
  detach and deletion are a separate Tom-approved planner step after fresh
  positive idleness.
- **Selectors.** `rust:compile`, `rust:compile-release` (oxide#187),
  `rust:compile-linux`, `rust:format`, `test rust` and `rust:test-600` call
  `oxide`. With the default `quick` profile, check and test
  use line tables rather than `dev`'s full debug info. Focused proof, profile
  and install selectors call Cargo, because they test or build Oxide itself.
  `rust:test-600` (oxide#224) is the same covering suite at its measured
  600 s / 512 MiB bounds. `scripts/rust-test-600.py` creates exactly the two
  per-run roots the suite needs — an internal root on the boot volume exported
  as `OXIDE_TASK_SCRATCH_ROOT` and `OXIDE_TEST_INTERNAL_ROOT`, and ordinary
  test temp under the selected volume's `<mount>/scratch/oxide-validation` —
  and runs the unchanged shared supervisor once. It refuses inherited control
  roots and any home, mount-root or group/other-writable ancestor, and removes
  only its own two run roots, only after a closed run; any other outcome keeps
  them with their evidence and prints their paths. Its pure self-check and plan
  are effect-free, and a covering run uses this or `test rust`, not both.
- **Escapes.** There are no `:cargo` sibling selectors (removed oxide#153
  with Tom's explicit approval; the prior sibling rule is superseded). The
  outage escape is Effigy's machine-wide per-program `oxide` fallback
  (see [Switching an existing project](#switching-an-existing-project)):
  `force = true` while `oxide` is broken, or `cargo` by hand.
- **Test supervisor.** `test rust`, `rust:test-600` and `rust:pool-scale`
  run under the [shared proof supervisor](#shared-proof-supervisor).
- **Fixtures.** `effigy dogfood:scripts:test` proves the installer's refusals
  (linked or foreign directories, an existing destination, a destination
  swapped during the build) on synthetic fixtures. It never builds or
  installs.

### Installed-binary hand-off (oxide#122)

A command whose binary is older than the control state it finds can hand off
once to the installed binary instead of only refusing. It is a front-door
compatibility step, not a repair for binaries that lack it: an older `oxide`
still refuses newer state, so the
[install compatibility window](#dogfooding-in-oxides-repository) remains
necessary.

- **Trigger.** Before any classified arm opens a store, the front door reads at
  most three control documents and creates nothing: the internal `state.json`,
  the removable pool's `removable/state.json` and `removable.json`. Each is
  parsed as a bounded `serde_json::Value`, and only a numeric `u32` `version`
  above this binary's `STATE_VERSION` or `REGISTRATION_VERSION` triggers,
  whatever other fields the document carries. (The admission gate writes
  control version 8 with the raised-ceiling fence as every mutating command's
  first durable effect, so an older binary hands off or refuses before it can
  meet a `capacity_policy: 4` ledger; a pool this binary never mutates stays
  at version 5.) A missing pool or file, an IO or
  permission error, an oversized document, a non-object, and a missing, non-numeric,
  negative or wider-than-`u32` version are not proof of a newer format: those
  keep the existing errors and never execute anything. Plain passthrough
  commands, usage-only `exec` and the `status --launch-check` probe skip the
  check.
- **Pre-effect boundary.** The check runs after empty-argument, help,
  `--version`/`--list` and leading-`+toolchain` handling and before any arm may
  open a store, create a pool, exclude it from Time Machine, remember a pool,
  install a signal handler, take the admission lock or start Cargo or a tool.
  Nothing before the decision has an effect. `Store::load` errors are never
  caught and no later phase replays the command: an incompatibility found at a
  final release keeps the existing refusal without a hand-off.
- **Candidate.** Exactly one: the newest timestamp-sort receipt under
  `~/.local/state/oxide/install/*.json` (at most 4096 directory entries, 64 KiB
  per receipt) must name `installed.path` = `~/.local/bin/oxide`, carry
  `installed.sha256`, `installed.size`, `control_state_version` and
  `registration_version`, and cover every newer version observed. Its
  directories from `HOME` down must be real, owned by this user and, below
  `HOME`, not group or world writable. Every `oxide` on `PATH` must be the same
  device and inode, so the candidate is reached through `PATH` and no other
  entry shadows it. The file is opened `O_RDONLY|O_NOFOLLOW`, must be a
  regular, user-owned, executable file, is hashed from that descriptor (bounded
  to 64 MiB) and must match the receipt's SHA-256 and size. The running binary
  itself, another `oxide` on `PATH`, a receipt for another path and an older
  receipt are refused. No arbitrary, older or unknown executable is searched,
  fetched or installed, no flags are translated, and no environment value
  forces a hand-off.
- **Exactly one.** An approved hand-off prints one stderr notice,
  `oxide: handing off to the installed binary (state is newer than this one).`,
  then `exec`s the candidate with the caller's byte-exact arguments (including
  `argv[0]`) and environment, adding only `OXIDE_INTERNAL_HANDED_OFF=1`. That
  name at any value prohibits a second hand-off and is inert without newer
  state; it never authorizes one or bypasses a check. A successor preserves it,
  so one original invocation executes at most once even if a third binary or a
  later state version appears. Signals and exit codes are the executed
  binary's.
- **Refusals.** With newer state but no candidate, a marker already present, or
  any candidate, ownership, path, hash, size or capability mismatch, the
  command prints `oxide: oxide was upgraded during this command; re-run.` and
  exits 75. A failed `exec` prints
  `oxide: could not start the installed oxide: <error>` and exits 1; it never
  retries. Without newer state the previous behaviour and exit codes are
  unchanged, and Cargo's own exit 75 is propagated untouched and is not a
  trigger.
- **Capabilities.** A receipt is capability proof only for the numbers it
  records, read from the authenticated pinned build by the hidden
  `oxide __oxide-build-capabilities` probe before the installer writes
  anything. A pin that cannot report them is refused before installation; a
  receipt without them, or lower than an observed file version, cannot prove
  the installed binary understands the state. Build hash, pin, tree, mtime and
  display version alone prove nothing. A future incompatible control or
  registration format must bump its numeric version to participate; this does
  not catch origin-only or ledger-only fences or every future field.
- **Exec residual.** The last check re-`fstat`s the opened descriptor and
  `lstat`s the destination path, requiring the same device, inode, size and
  mtime immediately before `exec`. The `exec` itself is by path, so a same-user
  rename in the remaining interval can still substitute the executed inode:
  the check is a check, not an atomic bind to the executed bytes. Darwin
  fd-execution is not established and is not used. This weakens neither the
  receipt, `PATH` nor ownership checks, and it is not adversarial same-user
  exclusion.
- **Proof.** `effigy rust:installed-handoff` (heavy) is one fixed script and
  whole command: `bounded-test.py` around `scripts/test-installed-handoff.py`
  with a 600 s wall limit and 128 MiB monitored scratch (decisions 5bf3c438
  and 49f52581). The earlier 120 s diagnostic reached its work deadline at
  100 s and the earlier 64 MiB diagnostic faulted at 64.4 MiB; neither has
  qualification credit. The supervisor's
  selected-volume root on Builds holds all fixtures; each product child declares
  that scratch root and an explicit pool cap. A `/private/tmp` fallback uses
  the internal path where the declaration is inert, so it is not scratch-pool
  coverage. The script stages this HEAD's
  debug `oxide` offline with 1.97.1, then exercises private HOME, receipt and
  scratch-pool fixtures with a recording candidate. It covers newer state and
  registration versions, refusals, skipped commands, and a final-release error
  after effects. The current candidate reports a version newer than the staged
  subject's control version 9; this is not a real installed-version transition.
  `effigy proof:installed-handoff:test` checks the apparatus and fixture
  environment without staging a binary; `proof:installed-handoff:plan` prints
  the case plan. The uncounted `rust:installed-handoff:diagnostic` uses the
  same bounds, publishes an atomic bounded progress receipt at phase, case and
  command boundaries, records monotonic timings, observed exits and bounded
  output, and aborts on unknown writer closure. Its retained merged Cargo
  staging log exposes package-cache waits even if the supervisor stops it.
  A started command without a terminal record has unknown exit.
  Its historical diagnostic at `1852c2f` stopped at the work deadline after
  seven completed cases; its receipt lacks the timing needed to apportion the
  278 s and carries no native credit (decision 88c71155). The later `e64850e`
  diagnostic hit the supervisor deadline during staging, before the old
  in-memory receipt was published; it also has no native credit. The `490e935`
  diagnostic stopped at 282 s after eight reached cases; its recorded staging
  took 55 s, with no native credit (decision 5bf3c438). Its receipt times each
  child process (`commands[].start_ns`/`end_ns`), not each product step. Source
  and retained-evidence diagnosis (oxide#225, no run):
  - **Measured.** All 19 exit-1 `check` processes in the case set ran at
    11.27–13.09 s wall. Each printed `creating a scratch build pool`, the
    pool's first command, and a build summary of 0.1–1.2 s. The 22 `status`
    processes took at most 17 ms; the newer-state hand-off check, which returns
    before pool open, took 18 ms.
  - **Recovered packet.** `/Volumes/Builds/scratch/oxide/evidence-225-recovered-490e935-20261101T225233Z` contains `SHA256SUMS` and `recovery-provenance.json` binding the copied session and log to `490e935`. This is a newly hashed copy, not a historical manifest or native pass. The original `oxide-proof.u7ktgeuh/evidence` recorded 19 exit-1 checks at 11.272969375–13.084510875 s, summaries of 0.1–1.2 s, 22 status calls at most 16.900167 ms, one exit-42 handoff at 18.4845 ms, and one check without a terminal record at the deadline. Its exit remains unknown; the failed diagnostic has no native credit.
  - **Attribution boundary.** The summary's clock starts at `started` in
    `run_invocation`, after pool open, workspace location, removable selection and
    key resolution, and stops at the print. The gap of about 11.2–12.5 s
    therefore lies before `started` or after the print. It is nearly constant
    across 0.1–1.2 s builds, so it is not compile time.
  - **Not shown.** Source shows no fixed 10 s wait on this path: the pool lock
    is an uncontended blocking `flock`, and `seed_lock::read` opens
    non-blockingly, so `seeding skipped: lock unavailable` names an absent
    input rather than a wait. The creation sub-steps (`scratch_io::evaluate`,
    `create`, `repair`, `init_scratch_pool`, `Store::open_at`) are unmeasured.
    The exit-1 status is not explained by the captured stderr.
  - **Limit.** This fixture's first-command pool creation is not normal
    daily-command latency.
  - **Validation clarification (Tom, 2026-11-01).** The no-live-cleanup
    restriction forbids explicit maintenance; ordinary admission eviction by
    the required installed-Oxide covering build is accepted build residue. It
    does not authorize a prune, clean, registry repair or restoration of an
    evicted slot. Subsequent covering builds for this task use a private scratch
    pool and a commit-bound disposable source snapshot.
  - **Trace scope.** The diagnostic's private fixture setup may request a
    debug-only trace to a bounded per-command evidence file, after matching
    the supervisor's running owner marker, scratch/evidence layout and private
    HOME/pool. Each named start and end uses a single monotonic clock; an absent
    end is unknown. Release builds do not emit this trace. Trace records are
    observations, not timing or closure gates; a missing trace cannot establish
    a duration. No new diagnostic has yet attributed the historical gap.
  - **Instrument finding (oxide#239).** The first diagnostic of that
    instrument (`/Volumes/Builds/scratch/oxide/oxide-proof.tj837y2a/evidence`,
    diagnostic receipt SHA-256 `a44391d1540063f72da10a0d6c5723076cc3407007ef0b9d1c5bc1d6ccd37939`,
    guard receipt SHA-256 `c47eca0216d73291d935c4af55c1017ab7ed3cf2dba77f460c9e328c8d723161`)
    completed 17/17 cases with the guard closed, no fault and no deadline
    (417 s of 600 s, 71.6 MiB of 128 MiB), but every one of the 31 instrumented
    commands recorded `layout refused` and no trace file existed: both
    validators required `marker.run_id == OXIDE_PROOF_RUN_ID`, while the guard
    receipt's `run_id` is the supervisor session token and the scratch
    marker's `run_id` is the monitor's independent marker token. The
    historical 11–13 s outside-summary gap therefore stays unattributed; that
    packet keeps its failed uncounted outcome with no evidence rewrite.
  - **Two-token binding (oxide#239).** The trace layout now binds each token
    to its own source and never equates them: the exported `OXIDE_PROOF_RUN_ID`
    (session token) must equal the guard's published `receipt.json` `run_id`,
    read as one bounded no-follow regular-file snapshot with a 64 KiB cap
    that refuses missing, symlinked, foreign-owned, oversized, malformed or
    cross-run documents and accounts for the monitor's atomic replace by
    refusing a snapshot whose file identity changed between `lstat` and
    `open`; the marker's `run_id` must equal that receipt's
    `scratch_marker.run_id` and the receipt must name the verified root,
    scratch and evidence paths and a published phase, while the marker's
    canonical root, UID, device and inode still bind to the already verified
    private descriptor topology. The supervisor's token generation is
    unchanged. Activation stays debug-only, opt-in, bounded at 64 events and
    silent in ordinary and release code.
  - **Trace attribution (oxide#239).** The fixed diagnostic now records a
    separate bounded trace-attribution result — expected instrumented
    commands, per-command allocation and raw phase evidence with starts, ends
    and unpaired starts as unknown ends, and the guard receipt's two
    identities — and requires actual nonempty raw phase evidence from every
    reached creation-path command: layout refusals or absent traces are an
    insufficient-instrumentation outcome (diagnostic exit 1), never a
    successful attribution beside passing cases. Missing ends stay unknown;
    no timing is invented and no requirement reproduces the historical gap;
    trace totals and time comparisons are observations, not performance
    acceptance thresholds. Native and kernel custody remain real and are
    never inferred from trace or command success.
  - **Verified 239 observation boundary.** The passing 239 diagnostic packet
    (`/Volumes/Builds/scratch/oxide/oxide-proof.direkfwy/evidence`, receipt
    SHA-256 `27744de3ecbb52c34abdab361887d18972f1ea83caf9b3419d234a4e332a8347`,
    guard receipt SHA-256
    `45250b772d2466c9864613d6fa9aaef492d4ccdfd05daa592867d733db17f2bf`,
    17/17 cases, 31/31 commands with nonempty paired traces, worker 447 s of
    600 s and 106.8 MiB of 128 MiB) measured each instrumented command at
    11.44–14.75 s wall with only 4–15 ms outside the trace window and
    `scratch-create` at 53–677 ms; the `store` phase closed about 11.07–11.14 s
    after `store-open` in every one of the 31 traces. That fixes only the
    region: the uninstrumented tail of `Store::open_or_create` after
    `Self::open_at` returned carried the remaining time. It is not proof of
    which step costs it, and it is not a latency fix; `tmutil` is not
    concluded as the cause from that packet. The prior 231 uninstrumented
    `tj837y2a` failure stays historical with no trace credit.
  - **Post-open tail partition (oxide#243, observation only).** The same
    guarded `handoff_trace::phase` instrument now partitions that measured
    tail, wrapping exactly one existing step each and changing no statement,
    order, result or branch: `sticky-exclusion` (`config::has_sticky_exclusion`),
    `backup-exclude` (the conditional `config::exclude_from_backup` call)
    with its nested real `tmutil` children `backup-addexclusion` and
    `backup-isexcluded` (the `budget::output` calls, so every caller of
    `backup_state` measures its own child), `remember-pool` (the conditional
    `config::remember_pool` call) and `ensure-control` (`Store::ensure_control`).
    Activation, the two-token binding, the no-follow receipt snapshot and the
    trace destination are unchanged; the 64-event cap does not grow: one
    creation-path command reaches at most 38 events (19 paired phases;
    `remember-pool` is unreachable for the proof's `OXIDE_POOL` commands).
    The parser refuses a file with more than 64 event lines as not product
    output. The attributable per-command summary gains each paired phase's
    actual duration, the measured `store-open`-end-to-`store`-end span with
    its explicit residual (window time no paired phase covers, union-counted
    so nested phases never count twice), and honest per-phase states
    (measured / unknown / not-applicable / absent) for the plan. A trace that
    itself shows `store-open` and `workspace` paired completed the reached
    creation path, and every applicable planned phase must then be paired
    there: a clean 17/17 collection whose reached-path instrumentation is
    absent is insufficient and exits 1, and the pure self-check discriminates
    the pre-243 unpartitioned output, a missing applicable phase, an unpaired
    nested phase and an all-old-events collection. This is a partition of
    observations, not a product verdict: no cause is concluded, no step is
    optimized or skipped, no timing is an acceptance gate, and papercut
    18fc63cf stays open. The one declared
    `effigy rust:installed-handoff:diagnostic` runs through Queue at this
    head after retained exact-head source approval, under the qualified
    `fcfda2b0…` supervisor bytes unchanged; author and reviewer run no
    native themselves and the same retained reviewer judges its one receipt
    (Tom `c8991f99`). Its native attribution can establish a measured
    responsible phase or an honest remaining unknown with the precise
    coverage reason; until then no result is claimed here.
  - **Discriminator.** A debug-only, opt-in phase trace on the creation path
    (the `release_trace::phase` pattern, not that trace, which covers only the
    release closeout) in one uncounted diagnostic would split the gap. That
    trace is a standard-tier change. Any change to creation or its timing
    writes the Builds volume and is Elevated.
  - Activation. The diagnostic and each native session require owner activation at the migrated head; no previous
  receipt carries over. The proof touches no live pool, installed binary or
  real install receipt, and is not a benchmark or full QA.

### Version identity (oxide#130)

`oxide --version` and `oxide -V` first print this build's identity on stdout and
flush it, then hand the unchanged arguments to the caller's Cargo, exactly as
the old direct pass did. Identity write and flush errors (a closed pipe above
all) are ignored and Cargo still runs: Cargo keeps stdout, stderr, signals and
the exit status, and an `exec` failure keeps the existing `oxide: could not
start Cargo:` message and exit 1. `oxide --list` stays a direct pass with no
identity lines.

- **Identity.** One line names the package (`oxide 0.1.0 (package
  oxide-storage)`), then a build line and a provenance line. Where Oxide
  cannot authenticate, it says `unknown` rather than naming the current
  checkout's HEAD: no `git`, pool, workspace or network discovery runs here.
- **Stamp.** The installer authenticates the source pin as it does now, then
  generates a fresh 128-bit lowercase-hex instance before the build and
  supplies `OXIDE_BUILD_PIN` and `OXIDE_BUILD_INSTANCE` to it. `option_env!`
  embeds them, and Cargo's dep-info tracks both names, so a changed stamp
  recompiles. A build without a well-formed 40-hex pin and 32-hex instance
  says `source pin unknown (this build carries no installer stamp)` and
  `provenance: unknown`.
- **Verified provenance.** With a well-formed stamp, the newest receipt under
  this `HOME` (`~/.local/state/oxide/install/*.json`, the hand-off's bounded
  no-follow reader: at most 4096 directory entries, 64 KiB per receipt) must
  be owner-only, carry the same `pin` and `build_instance`, and its
  `installed` facts must name the canonical running executable: path equal
  after canonicalising both, a stable private owned regular file, and a size
  and SHA-256 (bounded to 64 MiB) equal to the receipt's. The binary digest is
  reported as `installed binary sha256`, and the receipt digest — SHA-256 over
  exactly the receipt bytes that were parsed — as `receipt sha256`, always as
  separate labels. The matching instance closes the same-pin replacement gap:
  a runtime path and hash alone cannot authenticate an older executing image
  after replacement.
- **Unverified.** A missing, stale, foreign, malformed, oversized,
  non-owner-only or mismatched receipt, another instance, a symlinked
  destination, a copy elsewhere, or a hash or size mismatch is
  `provenance: unverified (<reason>)`. Historical receipts keep their bytes
  and permissions; an older receipt without an instance simply never
  verifies. The installer writes new receipts with `0600`.
- **Proof.** `effigy rust:everyday-cli` (heavy) is one fixed script and whole
  command under the qualified shared proof supervisor: 300 s including
  staging and cleanup, 512 MiB monitored scratch, 30 s shutdown margin, no
  image, mount, real registration, install, network or host state. Inside
  its scratch it stages this HEAD's debug `oxide` twice from one target
  (unstamped, then stamped with this HEAD's pin and a synthetic instance;
  measured ~10 s of staging at ~82 MiB peak owned) and runs one CLI matrix
  with a recording, refusing fake `cargo`: `--version`/`-V` identity order,
  honest unknown pin, exit code, arguments, working directory, environment and
  stream pass-through; `--list`; verified provenance with separate digests;
  same-pin replacement, unstamped replacement, another instance, non-private
  receipt and missing receipt as `unverified`/`unknown`; readiness escape
  acknowledgement once per run against actionable warnings, a comment that is
  not a route, per-invocation (not persisted) acknowledgement, an unedited
  manifest and a malformed manifest reported unclassified; exact
  `deny`/`audit`/`outdated`/`machete` forwarding with no pool and the caller's
  sentinel `CARGO_TARGET_DIR` intact; and creation-refusal and reserve-stop
  capacity diagnostics in typed, unknown-value, error-class, timeout,
  oversized and real positive-control forms, with the raw refusal and the
  20 GiB decision unchanged and no pool created. The recorded author runs
  passed 50/50 cases (24 s at an 81.9 MiB owned peak, then 22 s at 89.9 MiB
  after the shared-budget correction). It is not a
  benchmark or an alias for full QA.

### Temporary scratch placement

**Status.** The proof and validation tooling uses the selected-volume default
described below, under the qualified supervisor bytes
`5e5b66174fd9a92567436cfb6eff610063f316c4de4fadf93c1b995e05c971f2`; the owner
accepted the narrow internal-root exception for genuine internal-placement
proofs. Oxide#151 moves that default from a leaf at the drive root into the
owned `<mount>/scratch/oxide` subtree and adds the owner marker and bounded
seven-day self-expiry described below; that source is **unqualified** until its
own separate author and independent reviewer `proof:supervisor:qualify`
receipts exist, and the `a6d73326` receipts stay bound to their own bytes. The Oxide children rule is implemented on the oxide#136 branch but is
not installed or qualified: its counted native has not passed, its required
uncounted diagnostic has been attempted and failed, and the independent
reviewer native is missing. Neither facility establishes measured SSD-write
savings.

Tom, 2026-10-25: "Test and proof temp folders definitely should move to the
drive." The two Oxide facilities have separate qualification boundaries.
Effigy owns its own test/run temp
placement and retention; Oxide changes no consumer or daemon configuration.

- **Oxide children: implemented** ([contract](#child-temporary-files-oxide136)).
  The selected pool's actual volume decides temp placement
  for Cargo and its tests/build scripts, run programs, modelled external
  subcommands and `oxide exec` tools. The product architect's same-day interface
  ruling keeps an inherited `TMPDIR` only if it resolves canonically to an
  existing user-writable directory on that same device. Oxide then creates no
  temp directory of its own and never owns or cleans the inherited directory;
  its children use it normally, while the external owner retains cleanup and
  retention. Otherwise Oxide supplies owned per-command temp on the selected
  pool under its existing lease, accounting and positive writer-closure rules.
  `TMP` and `TEMP` follow the chosen `TMPDIR`. A different-volume explicit path
  does not override this rule. Missing/unwritable paths and a symlink escaping
  the selected volume use owned temp too. Absent drive selection falls back
  internally without recreating a mount point. Bounded escaped summary/status
  labels distinguish `temp: inherited <path>` and `temp: oxide <path>`.
- **Proof and validation tooling (oxide#145).** With no caller
  scratch or root parent, `scripts/bounded-test.py` creates its owned root in a
  private area on the volume the effect-free pool selection names:
  - the recorded removable selection when the pool's `removable.json` names a
    mount point that is a canonical real directory directly under a
    root-owned, non-writable `/Volumes`, on a device that is not the boot
    volume, whose kernel `ATTR_VOL_UUID` equals the registered volume UUID;
  - otherwise the configured pool's own volume (mount root of `OXIDE_POOL`,
    the user config file's `[pool] path` with `~` as the account home, or the
    built-in pool), again only when it is not the boot volume.
  The default parent is the owned subtree `<mount>/scratch/oxide` of that
  volume, not the drive root (oxide#151); `mount` stays the actual mount and
  the volume UUID and device facts are unchanged. Selection is effect-free: it
  only reports the parent, and creating it belongs to the supervisor's own
  blocking setup, so plans, pure probes and refusal paths create nothing.
  - **Preparation (shared ancestor, private leaf).** The two components have
    different contracts (Supervisor `2160fee3`, 2026-10-25). `<mount>/scratch`
    is a **shared** folder that other projects also use: it must already exist
    and must be a real canonical directory of this user on the selected device
    with no group or other **write** bit (`0755` is the normal, accepted case;
    a sticky bit does not make a writable ancestor safe). Oxide never creates,
    chmods, adopts, renames or removes it, so a missing, symlinked,
    foreign-owned, group/other-writable, unreadable, non-canonical or
    other-device ancestor refuses before any leaf or run effect. `<mount>/scratch/oxide`
    is Oxide's **private leaf**: exactly `0700`, this user, real, canonical and
    on the selected device. Only that leaf is ever created (with `0700`) when
    absent, and it is re-validated after creation, including its device and
    inode identity and a concurrent creator's result; an existing leaf with a
    different mode, owner, target or device refuses with no chmod or adoption.
    A refusal keeps the established single-line warned fallback to
    `/private/tmp`, and the qualification bootstrap still refuses to qualify on
    a fallback. On the Builds drive macOS ignores ownership (Owners Disabled),
    so these UID/mode checks are consistency and tamper guards for a
    cooperative same-user host, not a multi-user privacy guarantee; no
    behaviour, diskutil probe or permission change is built on that fact.
  - **Effect-free readiness.** `scripts/qualify-supervisor.py --preflight`
    (`effigy proof:supervisor:preflight`, non-heavy) applies exactly this
    preparation policy to the actual selected host using metadata only. It
    creates nothing — an absent private leaf is reported as `absent; would
    create 0700`, never as existing — and it never sweeps, marks, forks the
    deadline actor, launches the candidate or reads reserves. It answers
    ready / ready-to-create / refused for that point in time; the invoked
    qualification re-runs the same checks in its own setup, so readiness is
    never a reservation or a substitute for the setup rechecks.
  - **Run roots.** Each run is one `oxide-proof.*` directory (and each
    qualification bootstrap base one `oxide-supervisor-qualify.*` directory)
    created privately (0700, this user, canonical, same device as the parent)
    directly in that subtree, outside `oxide-pool` and `oxide-outputs` payload.
    Absent, unreadable, malformed, symlinked, wrong-device or otherwise unsafe
    selection falls back to `/private/tmp` with one bounded warning and never
    manufactures a mount path or an internal shadow.
  - **Owner marker and self-expiry.** Immediately after its root is
    exclusively created and before any child or run resource, a run writes one
    private `.oxide-scratch.json` marker: strict fixed schema, unique run ID,
    canonical root, owner UID, device and inode, creation time, initially null
    conclusion, outcome, closure, kept-reason and `nested_scope`.
    `nested_scope` decides who may conclude the root: `exclusive` means the
    run's own established closure is a whole-owned-resource conclusion;
    `hosting` means the root may contain resources created by another
    authority, so the run's own closure is never enough and only the authority
    that owns that nested window can record the positive conclusion. The
    qualification suite protects the candidate root as `hosting` before any
    nested case can exist, an unmarked directory is never adopted, an existing
    conclusion is never downgraded, and every transition re-verifies the marker
    identity it read, so protection and conclusion cannot race.
    The marker is written
    atomically (private temporary file, then one replace) and is updated only
    at a genuine conclusion: a closed session records `outcome: closed`,
    `closure: positive` and kept-reason `evidence`; an unclosed or failed run
    records an honest non-positive state with a null conclusion, or—on an
    abrupt exit—keeps its initial marker. It disambiguates outcome from positive
    closure, is never a process-liveness probe, and an absent, foreign,
    symlinked, oversized, corrupt, unknown-version, future-dated or
    schema-divergent marker fails closed. Before its own root exists, one
    bounded startup sweep confined to the canonical owned subtree may remove
    only real owner-marked roots whose marker records a positive conclusion of
    every owned resource strictly older than seven days. The fixed budgets are
    1 s and 4096 examined entries plus recursive steps, checked between
    filesystem steps; exhaustion leaves the unreached work for a later run and
    there is no background reaper. Nothing else is ever removed: unmarked,
    foreign-owned, non-directory, symlinked, other-device, non-canonical,
    non-expirable, unconcluded or nested-UNCLOSED entries are skipped and
    reported within a bounded row count, symlinks are never followed at any
    level, and the root's marker is removed only after its own contents are
    gone so a partial bounded deletion never turns a damaged root into an
    unmarked or eligible one. A peer's own filesystem fault (list, open,
    unlink, rmdir, a vanishing child or a permission error) is a bounded skip
    with its reason recorded: it never becomes this run's fallback and never
    aborts it, and the untouched peer keeps its marker until a later bounded
    run finishes it. A retained nested UNCLOSED fixture keeps its candidate
    root unconcluded even when the outer session closed and the suite passed,
    and a failed qualification keeps the bootstrap base unconcluded; neither
    can expire while that holds. A passing qualification that intentionally
    retains nested UNKNOWN resources is still a PASS, but its base records
    `outcome: retained` with kept-reason `nested-unclosed` and a null
    conclusion, never `qualify-failed` and never a positive conclusion. A safe
    existing nonempty parent is accepted: an unmarked `legacy-retained/`
    directory (the operator's relocated drive-root roots) is skipped as one
    entry with no descendant enumeration, no marker added and nothing deleted,
    while a marked eligible peer is still swept within budget. Historical
    unmarked roots from earlier runs stay unmarked and ineligible, and
    relocation never makes one concluded. Reserve sampling reads the filesystem
    actually holding the scratch, with the existing thresholds, cadence and
    staleness; the receipt records the selection source, the prepared parent,
    the actual root, the reserve path and device, the bounded sweep summary and
    the marker state. Explicit `--root-parent` and adopted
    `OXIDE_TASK_SCRATCH_ROOT` keep their ownership, retention and refusal
    contracts and create no marker: this changes the default, not caller
    ownership. This is not the product's verified build-time selection (no
    container/role check), and the narrow config reader understands only a
    simple `[pool] path` entry.
- **Unit-validation temp (oxide#151, after main `debffbb`).** A private
  validation temp root derives from the selection's *mount*, not from the
  supervisor's nested `root_parent`: `<mount>/scratch/oxide-validation` on a
  selected volume, or the fallback parent itself. It is a sibling of the
  marked scratch subtree, so it is outside the seven-day self-expiry, and the
  helper that names it creates nothing; the recipe makes that parent and one
  fresh `run.*` directory inside it. The AGENTS.md unit-suite recipe follows
  exactly this shape, with no hardcoded mount and no second `scratch`
  component.

Supervisor SHA-256
`5e5b66174fd9a92567436cfb6eff610063f316c4de4fadf93c1b995e05c971f2`
is the current qualified source: oxide#147 source head
`0adb0c4e9d24f104996013fe735030fb8f5e7e42`, merged at
`93ce14cc4233cfa1d27a63bfa7c1f5135a4b545c`, has independent exact-head source
review and separate author and reviewer `proof:supervisor:qualify` receipts with
the selected-volume default and positive outer closure (oxide#147's own records
hold the case counts, runs and events). Qualification is bound to those bytes
and evidence; an older hash's receipts do not transfer to new bytes.

**Internal-root keep for child-temp proof (oxide#136).** Decision af7df7d6
retains `--root-parent /private/tmp` in both selectors: the guard's selected
Builds prefix cannot fit even the shortest fixture slot scratch beneath the
91-byte socket bound. The short internal fixture path has a source-derived
88-byte maximum with a positive 3-byte margin for the script's admitted slot
names. The guard still monitors one owned root, 300 s, 512 MiB, reserve,
custody, closure and cleanup. This selector exports no `OXIDE_SCRATCH_ROOT`;
the separate owned image stays under its existing custody rules. Inherited
`TMPDIR` fixtures stay outside the directory-budget pool on the same device;
values under the pool are not valid, and the product's pool-membership refusal
is never weakened. This ruling
has separate native evidence at `0eeb740` as described above; carrying it to
the integrated head requires independent exact-diff confirmation. The blocked
review at `8d030b2` and earlier proof packets stay historical. The shorter supervisor prefix is tracked by papercut
`503e484d`. The earlier decision `bd357a4c` accepted the internal-root
exception, and an earlier oxide#136 diagnostic observed about 58 MiB peak;
that figure remains historical and is not a budget reduction or new evidence.

**Genuine internal-placement proofs (Tom, 2026-10-25).** Tom answered decision
`bd357a4c-2e1d-4190-bafb-290d7c2ae244`: "Accept the internal-root exception for
internal-placement proofs (#136 now, similar proofs later)". A proof that must
exercise a genuine internal directory pool may fix the qualified supervisor's
existing `--root-parent /private/tmp` option in its selector. Its selector and
owning proof description must explain that requirement. One monitored owned
root contains staging, temporary files, the internal fixture pool and any
removable fixture image backing. The existing 512 MiB scratch budget,
wall-clock bounds, custody, closure and retention rules remain unchanged;
reserve sampling follows the filesystem actually holding that root. This
exception grants no product pool-path bypass, extra unmonitored scratch root,
new mount topology or host cleanup authority. Ordinary builds, tests and proofs
continue to use the selected volume by default.

Existing proof receipts remain bound to their activated recipe. Moving an
internal-placement selector to this explicit root requires a short independent
recipe review; already-passed evidence is not recounted solely for that selector
migration unless its source changes. Separate ROOT activation still precedes
each diagnostic or native submission. Briefs, activations and outcomes live in
Queue; this knowledge record changes no runtime scratch or host setting.

**Scratch pools on an external validation volume (oxide#173).**
Tom, 2026-10-28: "this needs fixing so we can keep it on Builds next time".
[Scratch pools](scratch-pools.md) is the design for an explicit, UUID-bound
`directory-budget-v1` pool on a verified external volume, so validation and
proof fixtures need no internal root: an environment-only
`OXIDE_SCRATCH_ROOT` declaration consulted only at the external-volume
refusal, the mandatory Git-work-tree recheck for an admitted path, a pool-local
binding document plus a host-side identity record that survives detach, the
absent-volume refusal, the refusal matrix, and a migration list that would drop
the exception for ten proofs while `rust:outputs-prune`, `rust:prune-absence`
and the internal unit fixtures keep their boot Data volume root. The opt-in and S1–S3 implementation exist; `bd357a4c` remains current for each proof until its S4 migration lands. No existing recipe or real pool is changed by this opt-in, and each migration needs its own brief and independent exact-head review.

**Internal unit fixtures (oxide#149).** The selected-volume default leaves two
macOS unit tests without their premise: the product routes a leased child's
`TMPDIR` to the selected pool's volume (`MNT_IGNORE_OWNERSHIP` there), so
`prune::tests::legacy_eligibility_is_proved_by_the_boot_data_volume` cannot
prove a boot Data volume ancestry and
`volume::tests::a_caller_owned_private_root_passes_and_a_writable_one_refuses`
cannot observe real ownership facts. Both take an explicit owned internal root
instead of the test scratch: the caller names one fresh private directory on
the internal volume in `OXIDE_TEST_INTERNAL_ROOT`, each test checks that root is
a canonical real directory this user owns, owner-only, APFS, with the boot Data
volume's device and volume UUID, then creates one unique owner-only fixture
inside it and removes only that fixture. The prune test still proves internal
placement through the product's own `on_boot_data_volume`, so a root on another
volume fails rather than passing or skipping. The Rust test recipe in `AGENTS.md`
provides that root as the qualified supervisor's adopted
`OXIDE_TASK_SCRATCH_ROOT` as well, so the internal fixtures stay inside the one
monitored owned root (the same shape oxide#136 uses), and keeps ordinary test
temp on the selected volume through a separate `TMPDIR`, `TMP` and `TEMP`
directory there, which the product keeps as inherited child temp. A third suite
fixture, `config::tests::git_work_trees_and_aliases_refuse`, takes its root from
`OXIDE_TASK_SCRATCH_ROOT` and needs it off `/Volumes`, which that monitored
root supplies. This is a test-fixture and recipe placement only: no product
pool, refusal, ownership, prune, child-temp or eviction behaviour changes, the
supervisor's 300 s, 512 MiB, reserve, custody, closure and retention rules are
unchanged, no new selector or script exists, and ordinary test and proof
scratch outside this suite keeps selected-volume placement. The same narrow
exception shape as `bd357a4c` applies; it grants no blanket internal scratch
fallback for other work.

## Proof-authoring premise checklist

Every brief that authors or corrects a native proof script, fixture or oracle
uses this fixed checklist before a counted native (product architect,
2026-10-25). Record the executable source location and disposition of every
item, or a concrete reason it does not apply. Check the actual case bodies
and every child environment; plan text alone is not evidence. This supplements
the required uncounted diagnostic rule in the [validation policy](../vision.md#validation-policy)
and grants no native start, installation, clock extension or cleanup authority.

1. **Private HOME and rustup.** Capture the invoking `RUSTUP_HOME`, or the
   invoking `HOME/.rustup` when it is unset, before replacing `HOME`. Preserve
   that toolchain home in fixture children, keep the pinned toolchain and set
   `RUSTUP_AUTO_INSTALL=0`. Private `HOME` and `CARGO_HOME` stay private; never
   repair a fixture by downloading or installing a toolchain. Before any
   `--locked` Cargo call, establish a manifest-consistent `Cargo.lock` in that
   fixture; a fresh crate does not have one automatically. Trace the actual
   setup state as well as argv, and keep any offline lock generation inside
   the activated proof's owned scratch and bounds.
2. **Account-home control.** Set `OXIDE_TEST_HOME_FROM_ENV=1` wherever a private
   `HOME` is meant to be honoured. Without that explicit test control, Oxide
   deliberately uses the account home rather than a caller's `HOME`.
3. **Environment isolation.** Scrub inherited `OXIDE_POOL`, `OXIDE_CONFIG`,
   `OXIDE_TEST_*` and `XDG_*` before setting only the fixture's explicit
   configuration and controls. Do not copy host pool or account provenance
   into a private fixture by accident.
4. **Automatic adoption.** In permanent mode, a fresh checkout with no target
   can be automatically adopted on its first build, including a build that
   fails. An oracle requiring an unconverted checkout must positively create
   that premise and check its receipt state.
5. **Cargo external argv.** Cargo passes the external subcommand name as the
   tool's first argument: a fake `cargo-nextest` invoked by
   `cargo nextest run --help` sees `nextest run --help`.
6. **Owned images.** Use `hdiutil attach -owners on` for an owned image whose
   proof depends on ownership being enabled. Preserve the existing attachment
   identity, ordinary-detach and uncertain-custody rules.
7. **Compiler flags.** Freeze `CARGO_ENCODED_RUSTFLAGS` and `RUSTFLAGS`
   consistently for staging and fixture commands. An inherited encoded value
   must not silently override the proof's declared flags or change the staged
   candidate's capabilities.
8. **Temporary storage.** Use scratch and `TMPDIR` on the selected volume
   under the qualified defaults. A proof requiring a genuine internal pool
   uses the documented internal-placement exception above, fixed in its
   reviewed selector. Follow the exact qualified guard and activated recipe;
   this checklist does not transfer qualification between supervisor bytes.
9. **macOS mktemp.** `mktemp` without a template ignores `TMPDIR` on macOS.
   Supply a template under the intended owned scratch parent and verify that
   the executable fixture uses it.

A source sweep must trace these premises through each case and its helpers.
It complements the required uncounted diagnostic execution; it cannot replace
it. If a script lacks a safe keep-going mode, its author adds a minimal opt-in
`--diagnostic` before the counted run. Record failures and dependent skips
honestly: an unexercised case is not PASS evidence. Any fixture correction
needs the brief's covering checks, independent exact-head review and the
applicable separately activated diagnostic before a counted run.

## Shared proof supervisor

**Interim admission allowance (Tom, 2026-10-22).** Until Queue #381's
admission-hook adapter lands, Oxide native proofs may use Effigy `heavy`
inline, with a fresh owner scheduler headroom receipt immediately before
activation and the qualified supervisor's wall-clock limit, scratch budget,
continuous 20 GiB reserve watchdog and cleanup. This is not host admission or
a reservation. Every receipt bundle and evidence record must state:
"Effigy heavy admission unavailable (f6b3b50 retired host-run client; Queue
#381 pending); not host-admitted". Keep raw qualified-supervisor receipts
unchanged and link that disclosure in their evidence envelope. The allowance
ends when #381 lands; prior non-admitted runs are not retrospectively waived.
Operator source: Supervisor `7cc9444d-a355-463d-a2ca-3c135645a042`, Tom's
verbatim "1. Yes 2. Yes 3. Yes", item 3. Existing proof bounds, unknown-writer
handling, failure history and separate adoption/install gates remain.

**Previously qualified supervisor (oxide#147, merged `93ce14cc`).** The shared
supervisor bytes `5e5b66174fd9a92567436cfb6eff610063f316c4de4fadf93c1b995e05c971f2`
are qualified: oxide#147 source head
`0adb0c4e9d24f104996013fe735030fb8f5e7e42` carries independent exact-head source
review and separate author and reviewer `proof:supervisor:qualify` receipts, each
exercising every case with the selected-volume default and positive outer
closure (oxide#147's own records hold the counts, runs and review event ids).
Its `test_supervisor.py` is
`25b706cc8fb9c7b30fe89005670edb20f0c2946c5f6228e73a3c96607ac8dbc3` and
`qualify-supervisor.py` is
`0789e18372b7d6f787dc8d9ba3357885f0facacee860c476ca5a012d4891a194`. It selects
and monitors the volume the effect-free pool selection names, with the
`/private/tmp` warning fallback described under
[scratch placement](#temporary-scratch-placement), and its census settles a stop
only on a census started after the leader was reaped with no member left. Earlier
proofs keep the guard bytes they ran under as history: the previous qualified
`a6d73326e897ef93f8b9a448b24889cc0a73bda894ea699de44858b1c6d2756b` (oxide#145,
`test_supervisor.py` `79031c19…`, `qualify-supervisor.py` `736f0cf0…`) and the
older `961164e797c01432fa8234497627f75e56489ef6395ef66e48824cdb0e48c052` are
qualified only at their own bytes, and no old run is retroactively migrated.

**Current qualified supervisor (oxide#151, merged `af08749`).** The shared
supervisor bytes are
`bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924`
(`qualify-supervisor.py`
`585e33956b0e9b0cce2d45bc85726e1202952c1b500d64da6fa630ade78e4a95`,
`test_supervisor.py`
`37ea34f049341b28d7ca2ba4e87dd666527a0146d249e307f5c78b3461ccc9c7`). They are
qualified at reviewed source `d9d998853a1ad292a4001801a41a59f947095d0a` by an
independent exact-head source review, one uncounted keep-going diagnostic and
separate author and independent reviewer `proof:supervisor:qualify` receipts of
249 pure plus 40 real cases each with positive outer closure (oxide#151's own
records hold the runs, counts and review event ids; its first reviewer failure
and its retained nested unknowns stay history, not conclusion). The current
`bounded-test.py` and `qualify-supervisor.py` route the default scratch parent
to `<mount>/scratch/oxide`, prepare it in blocking setup, mark every run root
there, run one bounded startup sweep for positively concluded roots older than
seven days, and have the bootstrap verify the routing, sweep summary, marker
conclusion and its own kept base. This qualifies these runner bytes only: it is
no product, task or native evidence for any proof that uses them, and the
`5e5b6617` (#147) and `a6d73326` (#145) receipts stay bound to their own bytes
with no earlier run migrated.

**Post-census image detach request (oxide#191, decision `ee263cea`, unqualified
source).** The oxide#191 runner bytes
(`0848974d79d84617070bd611da9503b48a0127e68c2965cebcd36e8d5fc13414`, as merged on
main) add one opt-in control for a proof
whose own test image must be detached, and it is the only route by which such a
proof may detach after a launch; the qualified shared runner is otherwise
unchanged. A proof publishes ONE bounded atomic request (`detach-request.json`,
at most 4096 bytes, fixed keys `version`, `backing`, `mount`, `device`,
`volume_uuid`, optional `note`) directly inside its `OXIDE_PROOF_EVIDENCE`
after the image exists, because a static command-line mount path cannot
identify a volume minted by the child. The monitor reads it only on its
`cleanup` decision — the qualified guard's certain empty census with the leader
reaped and no fault — and re-derives every binding itself before any effect: a
regular non-symlink request inside that evidence directory, an absolute
canonical backing that is a regular file inside this run's owned root or
evidence directory, a canonical `/Volumes/<one component>` mount point, one
`/dev/diskN` node (whole disk or one mounted slice), a UUID, the same image
attached from that backing with that node mounted at that mount holding that
volume UUID, the mount bound to that node's device, and a caller-owned run root
detaching nothing. The WHOLE sequence — the request read, the identity re-derivation, the
authorization round trip, the child's output drain, the reap, the post-state read
and the final cleanup gate — is bounded by ONE absolute deadline taken from the
custodian's own wait window; every cap in it is a MINIMUM with what is left, a
spent window spawns and waits for nothing, and no phase has a fixed floor that
could exceed the remainder (the reap included, so a child that ignores KILL
cannot extend the sequence). The test-only `custodian-timeout-after-monitor` injection stands in for
the deadline actor firing after the decision and keeps deadline semantics (exit 124); a real monitor
that has not completed at the exit margin remains a fault (125), so the injection cannot weaken child
custody. It then asks the custodian — which alone holds the
latched fault, the deadline and the closure verdict — to authorize the single
attempt over the control pipe; the custodian answers from that state only (no
filesystem call) with its verdict and the wait budget that remains before it would
declare the monitor incomplete, and it is asked again every 0.2 s while the child
runs, so authority withdrawn mid-attempt (a newly latched fault, the deadline, a
lost answer) KILLs the child instead of letting it finish. An allowed attempt is
ONE ORDINARY `hdiutil detach <mount>` of that exact image, clamped to that budget
and to its own ceiling, with a bounded child whose output descriptor has one owner (detached from its buffer and
closed before any wait, so a reused descriptor number is never closed by a stale
finalizer) and an output tail capped at 4096 bytes however much the child writes; the request names the MOUNTED entity's own device, because an APFS
image attaches as a container whole disk plus a synthesized whole disk and the
mount can live on either, so a container name is not the mount's device. The
attachment facts fail closed: a malformed or incomplete relevant structure — a
missing or non-list images list, an image without a path, a non-list or empty
entity list, an entity without a device, a non-string mount or UUID, or a declared
backing attached with no entities — is unreadable, never an empty positively clear
set, so the post-state is unknown and the custody check keeps the root; success
is every fact at once — exit zero, no spawn error, no timeout, no withdrawal, the
child positively reaped, the mount gone *and* no entity of that backing still
attached (an unmounted volume, a leftover container or a synthesized disk is not a
detach) — never a zero or missing exit on its own. A child that is not positively reaped is never reported
as reaped, and its live handle survives every exception path (authorization, the
attempt and the post-state read): the monitor publishes the child's PID **and
start time**, stops touching the handle and waits for the custodian's
acknowledgement before it may reap again, because a reaped PID can be recycled and
a bare PID is never signalled. Every custody lookup goes through one safe reader: a
failed `sysctl` size/read or a malformed row is an unreadable observation — never
`gone`, never an identity — and an unreadable observation is never read as closed.
The child is still the monitor's own direct child, so while the monitor holds it
unreaped its PID cannot be recycled: it stops and reaps it itself inside the
custodian's remaining window and records that closure
(`detach_child_closed_by_monitor`); if that cannot close it, the monitor hands it
over by PARENTAGE — no start time — while still holding it. The custodian's post-decision records (a detach child's pin, signal and observation, a
cleanup keep) are drained into the receipt once, immediately before the final
publication, only for a run that took part in the image protocol and only within what
is left of the custodian's own wait window (one short bounded drain); a request-free
run performs no drain and gains no wait, a keep/UNCLOSED decision never drains either,
and a record met while awaiting a custodian answer is kept rather than discarded. The custodian accepts
that parentage-only message, signals and positively observes it, refuses with an
acknowledgement but no signal when parentage no longer matches, and acknowledges a
positively absent child as gone without signalling or indexing any row. The
test-only unreadable injection is scoped to one actor, so the monitor-side and
custodian-side outcomes are each exercised end to end. When the facts are readable the custodian
re-reads the real `Kinfo.one()` row (liveness field is the boolean `zombie`; there is
no `stat` key) and requires the same PID, the same start time, still this monitor as
parent and a non-zombie before it signals; then it signals, observes it gone under that same identity and
acknowledges (a post-timeout close is the backstop); only a declined handoff (the
child is already gone) lets the monitor finish the reap itself. The same rule covers the attachment read: an `hdiutil info` child that cannot be reaped is kept as
custody, closed bounded by the remaining window before completion, and an orphan that cannot be closed
makes the monitor hold rather than claim completion. The live handle also survives an exception raised
inside the child executor after spawn: the
failure record carries it, so custody is never dropped on an error path. Pin, signal and observation are recorded, an unobserved child is a latched
fault, and the root, backing and evidence stay kept either way. A request that is
present but not an object (including JSON `null`) is a refusal, never an
absent request. A refusal, failure, timeout, identity change or
ambiguous/unreadable post-state is never retried, never removes a backing,
keeps the mount, its backing and the evidence, records the attempt in the
receipt's `detach` object, ends the run unqualified (exit 125) and leaves the
owner marker unconcluded with the `detach` reason. Only a positive detach is
followed by the ordinary owned-scratch cleanup; an UNCLOSED, uncertain or
faulted run never enters that branch. Separately from the detach
protocol, cleanup is fail-closed but **opt-in**: a proof that created an image
publishes a bounded `image-custody.json` naming its backings before attaching,
and only then does the monitor check those backings once before cleaning, rechecking the absolute deadline before EVERY backing read and giving each read only what is left of the custodian's own wait window, shared with the backings still to read and with no floor above that remainder,
keeping the whole root with a `cleanup` object (`state: kept`) and exit 125 when
one is mounted, attached without a mount point (its `/dev` node still exists) or
uncertain (unreadable facts, an unreadable device state, a malformed indication,
or a UUID that does not match the request); a stale `hdiutil` entry whose device
node is positively gone keeps nothing, and a positively clear backing set records
`state: ok`. The custody indication is consulted for every cleanup run AFTER the detach receipt and its
unreaped-child custody are settled, so a custody keep can never hide a detach object; it covers
runs whose requested image detached too, because a proof can declare more than one backing and a
second live one must never be cleaned just because the requested image detached. A run that publishes **neither** an indication nor a request is
never scanned and never gated, so the default cleanup path is byte-for-byte
unchanged and gains no new failure mode. An opted-in run additionally requires
one final custodian authorization, taken after the detach from its current
fault/deadline state, plus a non-zero remainder of its wait window; a late
fault, a reached deadline or a spent window keeps the root. A run whose
proof published no request therefore gains no `detach` object and makes no
detach attempt. A qualification diagnostic receipt is always `qualified:
false`, even when every named oracle passed, and its `result` is never
`pass`. The
custodian keeps its filesystem-free decision path and the existing
cooperative-session limits. These bytes are unqualified until one uncounted
`effigy proof:supervisor:qualify:diagnostic` records a clean picture and then
ONE counted author and ONE separate independent reviewer
`effigy proof:supervisor:qualify` pass at the same head (287 pure plus 48 real
cases at that head, including the seventy-six pure detach cases and the eight real ones: a
bound success, a refused identity, a failed attempt, an unrequested attachment
whose opt-in custody keep is recorded separately, and a real stuck child the
custodian pins, signals and observes closed), with independent exact-head source
review afterwards.

**Ancestor and launcher-sibling exclusions (oxide#197, brief v2, unqualified
source).** Run under Effigy, the supervisor's own ancestor chain — the Effigy
runner, then the payload leader and sentinel, then this supervisor — is
reported by `kinfo_proc` as stable and not zombie while `getsid` fails with
`ESRCH`, so every nested case under the installed Effigy could end `UNCLOSED`
with `census uncertain: N unvalidated rows` while the same bytes passed when run
directly. A direct-run diagnostic then saw one unvalidated row from a
short-lived unrelated Node CLI call that shared the launcher shell's session.
Brief v2's start-time ordering rule was then withdrawn by design ruling
`ea5a3dc9`: a wall/boot/uptime comparison cannot prove the absence of a masked
backward, back-and-forth or forward wall step, so no clock fact may exclude a
row. Only two structural facts remain. In its bounded setup, before the leader
exists, the monitor walks its own `ppid` chain from its owner parent through PID
1, bounded to `ANCESTOR_MAX_LEVELS` (64) levels, and records each level's
positive identity: the exact `(PID, start)` pair the snapshot names, the same
start token the census already uses. The post-leader census then skips and
nothing else:

- **`ancestor`:** the row's own identity is exactly one recorded pair AND that
  identity is re-verified through the safe Kinfo reader after the census
  snapshot. A row whose PID is a recorded ancestor's PID is decided by this rule
  alone and never falls through to its parent's rule. A different identity at
  that PID is decided by ONE bound session observation: if the identity held on
  both sides of that read and the row is in the leader's session it is **carried
  into the census as a member of exactly this identity and tracked** (an ancestor
  predates the leader, so a descendant can never be one), so a later
  contradictory read cannot turn it foreign; every other reading — including a
  stable session that is not the leader's — is **undecided**, excludes no row and
  leaves the row to the ordinary getsid/reread classification.
- **`parent` (the launcher sibling):** the row's observation-time parent row, in
  the same snapshot, has a PID above 1 and an identity that is exactly one
  recorded pair, and ONE sibling observation re-verifies the row's own
  `(PID, start)` identity AND that parent identity on both sides of the
  session/group reads, so a PID reused between the snapshot and those reads can
  never lend another process's session or group to this row; the row must still
  name that parent in both re-reads. Only a positively **outside** observation
  excludes: the session and the group must both be readable and both different
  from the leader's PID (its session and group id). A row positively **inside**
  the leader's session is **carried into the census as a member of exactly this
  identity and tracked**, so a later contradictory session read cannot make it a
  foreign row. Anything else — a recycled or missing parent row, a replaced
  identity, a transiently failed re-verification, an unreadable session or
  group, or a leader group reading with a foreign session — is **undecided**: it
  excludes nothing and the row keeps the ordinary getsid/reread classification, so
  a held-suite zombie, a recycled identity or a foreign row classifies exactly as
  it does without the launcher rules. A matching parent PID alone is not enough,
  and every PPID 1 orphan and unknown parent is never excluded by this rule. The macOS rule that a supervised descendant can
  only be reparented to launchd — never to a supervisor ancestor — makes such
  parentage incompatible with supervised descent.

A **tracked** identity — one this census already accepted as a member — is
never excluded by either rule: both veto it before anything else, and no
exclusion ever removes tracked attribution. A tracked row is therefore never
reclassified, whatever its parent or its session/group read says. A `member`
launcher observation is carried as a tracked member row of exactly the observed
identity, so a positively observed member is never re-read into a foreign row.
Only a positively decided identity may be excluded; an undecided observation
adds no record of its own and never changes main's classification.

No wall, boot or uptime value participates anywhere in the census, so a row
whose start token merely looks earlier is never excluded and a tracked
descendant keeps its existing classification however its token compares.
Descendants, same-session strangers, first-seen unreadable rows and every
identity that was never read are classified exactly as before, an unreadable or
empty ancestor record excludes nothing, and the walk stops at the first level
the snapshot does not name instead of guessing a parent. The receipt records the
recorded chain in one bounded `ancestors` object with fixed keys — `state`
(`recorded`/`partial`/`unreadable`), `count`, the `identities` rows (PID, start,
1-based level) and `stop` (`pid-1`, `depth-bound`, `not-in-snapshot`,
`malformed-row`, `cycle`, `read-error` or null) — and the positive evidence for
every exclusion in one bounded cumulative `ancestor_exclusions` object: at most
`EXCLUSION_MAX_ROWS` (20) distinct `pid`/`start`/`rule` rows, the total decision
count, the omitted count (`count == rows + omitted`) and the per-rule counts. The
integrated `bounded-test.py`
(`94b1aba60a7475a31b5620039986336f13e1c88d58dd3732b5736290dc63685b`) adds this over the oxide#191 detach
control and the oxide#192 admission reports, so those controls are unqualified
until this head's own uncounted `effigy proof:supervisor:qualify:diagnostic` and
separate author and independent reviewer `effigy proof:supervisor:qualify` pass;
the suite gains thirty-one pure structural launcher-rule cases and one real
launcher-sibling case (361 pure plus 55 real = 416 at this head, and the
bootstrap's expected case total follows the suite). The end-to-end slow-setup
launch-and-close regression for #192's sampling-based freshness clock is
inherited here by decision `7bb1089d`: it injects a cold setup longer than the
1 s reserve window but inside the 10 s setup deadline, requires a launch through
the gate, a child execution and a closed outcome, and asserts from the receipt
that the first completed reserve reading arrived after the superseded
custodian-start window, so the case would fault under that basis; the separate
no-reading and setup-deadline controls stay in `PureFreshnessClock`.
**Qualification chain for the integrated #197 head (recorded, decision
`b688eac2`).** The controlling chain is the Validation section's: one uncounted
DIRECT `proof:supervisor:qualify:diagnostic`, one counted DIRECT author
qualification, one counted DIRECT independent reviewer qualification at the exact
source head, and ONE uncounted diagnostic through the installed Effigy with 11/11
checks and zero problems. The merge-hold phrase that also asked for both counted
qualifications "once through the installed Effigy" was an overstatement and is
superseded; no counted installed-Effigy qualification is required.

The supervisor archive
`/Volumes/Builds/scratch/oxide/evidence-197-c1c4446-20261101T010045Z` binds all
of it. `SHA256SUMS` covers `NOTE.md` (`8871c21a…`), `REVIEWER-PROVENANCE.md`
(`d5c68065…`), every run log, `run197.sh` and `sessions.jsonl` (`c84701ac…`).
The native head is `c1c4446…`, every receipt names supervisor `94b1aba6…`, suite
`ed560949…` and bootstrap `88a073e2…`, and each run recorded 54 case rows:

- uncounted DIRECT diagnostic, contaminated: `diag-c1c4446.log` (`b934ba04…`),
  9/11 with two timing-only Real failures while an unauthorized overlapping
  `effigy rust:test` (Barge T5 reviewer `ba3e7b93`) ran at load 34; the census
  cases were clean. Preserved with no credit.
- uncounted DIRECT diagnostic, clean: `diag2-c1c4446.log` (`8109a8cb…`), 11/11
  checks and 0 problems, `result: diagnostic`, **exit 1** (a diagnostic selector
  always exits non-zero even on a clean 11/11), evidence
  `oxide-supervisor-qualify.d2vd5nma`.
- counted DIRECT author qualification: `author-c1c4446.log` (`14dd71a9…`),
  `result: pass`, 0 problems, exit 0, candidate exit 0, evidence
  `oxide-supervisor-qualify.sy0gp881`, executor "Oxide Supervisor e96f186a,
  direct" from a new session leader.
- counted DIRECT independent reviewer qualification: `reviewer-c1c4446.log`
  (`1d116b75…`), `result: pass`, 0 problems, exit 0, candidate exit 0, evidence
  `oxide-supervisor-qualify.zez7b17y`. Its distinct executor is recorded in
  `REVIEWER-PROVENANCE.md`, which `SHA256SUMS` binds: a fresh general-purpose
  subagent (harness id `a9fd3220c7593d6c9`, its own worktree
  `w197-reviewer-c1c4446`, its own process session and conversation, spawned by
  the Supervisor session `e96f186a` under authority `fb3cef4d`) — separate
  context, not a different model — and it wrote its own `sessions.jsonl` line
  ("independent reviewer subagent of Oxide Supervisor e96f186a"). That is the
  provenance to verify; the earlier PR table's attribution to `e96f186a` alone
  was imprecise.
- uncounted installed-Effigy diagnostic, clean: `effigy-diag2-c1c4446.log`
  (`7012d78c…`), 11/11 checks and 0 problems, `result: diagnostic`, **exit 1**
  by that same rule, evidence `oxide-supervisor-qualify._enpfxdr`. The earlier
  `effigy-diag-c1c4446.log` (`2ab57d93…`) is an uncounted 8/11 whose three
  uncertain outer rows were exiting processes of an unrelated Effigy test tree
  (foreign churn, not Effigy launch ancestry), with two consequent DetachReal
  errors and exit 125 — preserved, no credit.

The controlling native head is `c1c4446…`; every later commit on this branch
is text-only against it (`cc579f6` and `fd9c74d` are those text-only heads:
`AGENTS.md` suite counts — 361 pure + 55 real = 416, thirty-two launcher cases —,
the `scripts/test-recorded-slot-accounting.py` docstring guard hash `94b1aba6`,
and this record), so the final head is the branch head and the reviewer verifies
it by the diff stat against `c1c4446` plus an AST comparison with docstrings
stripped that is identical, with `GUARD_SHA256` still `94b1aba6…`. The receipts
therefore carry to the final head without repeating the chain (brief v6 and v7
text-only rulings, parallel to #192's v8). No historical packet is rewritten and
no native credit transfers between tasks.

**Cooperative-session survivors (oxide#195, brief v3, unqualified at this
integrated head).** The integrated `bounded-test.py`
(`fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b`),
`test_supervisor.py`
(`c95d46aa0e590285c3c412294ffb215c0d45b19ba804f2243f0d2ebd62158d7b`) and
`qualify-supervisor.py`
(`c4b7a6a7983dfe4e097c3949e35f603aed8aefdee95ad41722986130cf01922c`) add the
oxide#195 custody hardening over the oxide#197 ancestor rules and the oxide#192
admission reports, so all three controls remain unqualified until this head's own
uncounted `effigy proof:supervisor:qualify:diagnostic` and separate author and
independent reviewer `effigy proof:supervisor:qualify` pass. It removes every path
that signals a merely observed non-child PID (the earlier checked-then-signalled
design is deleted, not narrowed), records the same-session survivors a census
round still sees in the receipt's bounded `survivors` list, and binds the deciding
census to the census message the custodian's own decision named. Closure now also
requires a finished census thread, a certainly empty final round and a certainly
empty named message; an expired join freezes the survivor list at its published
prefix, records `census_join.joined: false`/`frozen`/`refused`, refuses the
cleanup decision, has the custodian latch the reason, and keeps the root, backing
and evidence. Nothing in that path signals anything. The suite gains twenty-one
pure cases and one real recorded-and-never-signalled Cargo-shaped survivor case
in place of the former separate-group survivor case (382 pure plus 57 real = 439
at this head, and the bootstrap's expected case total follows the suite; the
historical #197 record above keeps its own 416). The keeper of the historical
survivor is Queue papercut `9ff7386d`; this head keeps that process's identity on
record and claims no closure of it.

**Admission reports on the integrated source (oxide#192, qualified at the native head).** The
integrated head adds the admission reports described in the supervisor rule
below: the completed-wait marker and its bounded grant, the correlated waiting
report, and the stop-boundary drain with late-marker classification. Together
with oxide#191's merged 287 pure and 48 real cases the suite was 329 pure + 53
real = 382 cases at that native head (39 pure and 5 real of them oxide#192's: the completed wait and its
grant, the rejection control, the late grant, a wait that never ends before the
stop, and the stale-run plus zero-duration-grant correlation), which the
bootstrap's expected count and limitation list follow; the integrated oxide#197
head then adds its structural launcher cases and the inherited slow-setup
regression, for 361 pure + 55 real = 416). The exported deadline,
closure, census, reserve, scratch, cleanup, custody and signal rules are
unchanged, a run with no admission report behaves exactly as before, and the
detach protocol's own rules are oxide#191's. The native chain ran at native head
`a19fb6c`: the owner-activated uncounted diagnostic passed 11/11 checks with zero
problems, and the counted author qualification and the distinct independent
reviewer qualification both passed, recorded in
`/Volumes/Builds/scratch/oxide/evidence-192-a19fb6c-final-20261031T232038Z`
(SHA256SUMS; the reviewer's own receipt is
`oxide-supervisor-qualify.i729jh46`). The final head `71b7963` differs from that
native head only in `AGENTS.md` and `workflow.md` Markdown; the independent
review confirmed by `git diff a19fb6c..71b7963` that it changes no executable
statement, non-docstring constant, test, fixture or proof-script behaviour, so
those receipts bind to the final head under the brief-author source-immutable
rule (decisions `7bb1089d` and `36ab646d`). The remaining end-to-end slow-setup
launch-and-closure regression is assigned to oxide#197, not dropped. Earlier
receipts stay historical, the `bc24b238…` (#151) bytes remain the qualified ones
for their own runs, and no earlier evidence is migrated.

**Historical receipts (superseded source).** At `93bb6db` the author side ran
`effigy proof:supervisor:test` (227 pure), an uncounted diagnostic (270 reached
of 270 expected, 270 completed, none failed or unexecuted, evidence kept at
`/Volumes/Builds/scratch/oxide/oxide-supervisor-qualify._xqoizwl`) and then the
counted `effigy proof:supervisor:qualify` (`result: pass`, no problems, 270/270
cases, candidate `closed` status 0, no fault, no deadline, positive outer
closure, 198.2 s, the 24 expected retained nested `UNCLOSED` case directories and
the base kept at
`/Volumes/Builds/scratch/oxide/oxide-supervisor-qualify.4sjmprvx`; scripts
`supervisor` `a0d7fa83…`, `suite` `59916bd6…`, `bootstrap` `9ae2b84d…`). A
reviewer qualification also passed at that source's documentation head
`68faa7b`, reported by the review rather than by this file. Those receipts, and
oxide#191's own receipts on its reviewed bytes, bind the source they ran under
and are **history** only: the correlated waiting report (schema 2 with the run
and command tokens) changed `bounded-test.py`, `test_supervisor.py` and
`qualify-supervisor.py` again before integration, and a first diagnostic at
`96f0767` recorded a fixture `NameError` as one failed case and continued
(corrected, preserved as history).

**Measured combined-suite basis (oxide#192, 2026-10-31).** After merging
oxide#191's reviewed runner (which already raised the qualification's fixed
deadline to 420 s and its candidate limit to 380 s, and the suite's own alarm to
350 s), the combined 314 pure + 53 real suite was measured once, privately, as a
development measurement (not a qualification): a clean, closed run of
`bounded-test.py --root-parent <owned mktemp> --limit 380 --scratch-budget-mib 512
-- python3 scripts/test_supervisor.py` finished the 53 real cases in **289.6 s**
(guard wall 297 s; owned-byte peak 95.9 MiB of 512 MiB; reserve minimum 2207.7
GiB; closed, status 0, no deadline) — 76.2 % of the 380 s candidate limit,
80.4 % of the 360 s stop point and 82.7 % of the 350 s suite alarm. The earlier
provisional estimate of about 241 s was under-measured because oxide#191's detach
real cases were failing early there. Those figures describe that private run
only: they are superseded by the direct diagnostic below, which ran longer inside
the real qualification, and the brief-author correction then raised the bounds
again.

**Superseded the same day (oxide#192 amendment, Oxide Supervisor, 2026-10-31).**
The first uncounted diagnostic at `5623edd` (direct, under Tom `d3d61247`)
exceeded the 350 s suite alarm (candidate exit 142 = SIGALRM, 357 s wall, load
about 9.5), so inside the real qualification the combined suite runs longer than
the private 297 s measurement. That packet is **failed**: it is preserved as
history with no native credit, and only its pure suite passed. The bounds are
therefore raised to a 600 s fixed deadline (kill at 598 s), a 560 s candidate
limit and a 530 s suite alarm; scratch stays 512 MiB and the margin semantics
are unchanged. Earlier qualifications stay bound to the limits they ran under.
Under Tom decision `d3d61247`, while Effigy's I2/I3 helper-process regression
makes its session census uncertain, the owner may run the exact `effigy.toml`
proof commands directly with the same bounds, environment, reserve and custody
checks and a per-session executor/command/head/exit/load/log-hash envelope; that
is no general waiver, and one clean uncounted diagnostic, one counted author
qualification and one distinct independent reviewer qualification follow in that
order at the final published head.

**Integrated head: qualified, pending the final-head verdict.** The oxide#192
bytes are qualified at native head `a19fb6c` by the owner-activated uncounted
diagnostic (11/11 checks, zero problems) and by the counted author and distinct
independent reviewer qualifications (receipts in
`evidence-192-a19fb6c-final-20261031T232038Z`). The final head `71b7963` changes
only `AGENTS.md` and `workflow.md` Markdown, and the independent review confirmed
with `git diff a19fb6c..71b7963` that it carries no executable statement,
non-docstring constant, test, fixture or proof-script behaviour change, so those
receipts bind to it under the brief-author source-immutable rule. The reviewer's
slow-setup regression finding is resolved by decision `7bb1089d` (the
end-to-end case is assigned to oxide#197, not dropped). What remains is an
independent final-head verdict before hold release; the covering checks at this
that head were `effigy proof:supervisor:test` (329 pure cases) and
`effigy docs:check`, with the earlier `effigy rust:compile`,
`effigy rust:format` and `effigy test rust` runs unchanged-input history.

**Settlement observation (oxide#147, unqualified source).** The current
`bounded-test.py` also carries observation-only settlement diagnostics, so a
run that ends with no completed post-reap census round can be told apart from
one whose round was certain and empty (oxide#140 diagnostic5 reported
`census_rounds` 0 with every other case passing). These are observations, not
acceptance gates and not a closure authority: the head that carries them is
**unqualified** until its own measured-cause correction and its separate
author and independent reviewer `proof:supervisor:qualify` receipts exist, and
the `a6d73326` bytes above stay the qualified ones for their own runs. The
bootstrap file changes with it for two concrete reasons and no others: its
expected case count follows the suite's new total (160 pure + 40 real = 200)
and it gains one limitation line describing this observation, so it adds no
bound and no effect. Oxide#151 later raises the expected total to its own 209
pure + 40 real = 249 with the routing, marker, nested-window protection,
bounded-sweep, qualify-entry and shared-ancestor/readiness cases below. Every quality fact here binds to the digests its own
receipt records, never to the `#145` digests above.

- The monitor's receipt gains a `settlement` object with a fixed schema:
  every stamp is an int `CLOCK_UPTIME_RAW` value or `None` when that step was
  never reached. It records the `stopped` send it was told about, when the
  monitor processed it, the census thread start, the first round start, the
  stop request and the loop exit, plus a bounded list of per-round records
  (index, start, scan end, snapshot and scan durations, rows, `ESRCH` rows,
  zombie rows, confirmation reads, the round's shared confirmation pauses, lock-wait entry, lock acquisition,
  lock release, send return, round end, and the round's published
  `certain`/`members`/`uncertain`). At most 20 round records are kept; further
  rounds only move `rounds_started`, `rounds_completed` and `omitted`. The
  whole object is structurally bounded to at most 16384 compact UTF-8 bytes,
  verified by a pure case rather than by serializing every round.
- The `stopped` command carries one informational field, the custodian's own
  `CLOCK_UPTIME_RAW` send stamp. A missing, ill-typed, negative or
  receive-later value is unknown (`None`), never a fault.
- The census round's counters are observation only: they read the same
  snapshot and the same confirmation reads and pauses the round already
  makes, and they never enter the round's returned object, the control
  message or a classification.
- The custodian writes one best-effort `settlement` record into
  `receipt.custodian` at its closure decision: the census messages it received
  and how many were post-reap eligible, the first and last eligible stamp,
  its `census_ok`/`census_uncertain`, the EOF set it had, and the fault. A
  failed write is dropped and never latched, so this cannot turn a closed run
  into a fault or replace the first fault. It is the decision-time view; the
  receipt's `settlement` object keeps accumulating from later monitor activity,
  so the two are never merged into one "measured" claim.
- Diagnostics can only be missing: loss, truncation or a dropped observation
  never changes an authoritative count, the fault, EOF, census certainty, the
  deadline or cleanup. Neither object is read back by the supervisor.
- **Measured correction (same task).** The uncounted diagnostic at `8849f99`
  measured the settlement window on a real 188-case run: command-pipeline
  receive, thread start, first round start and the receipt lock were
  micro-seconds to a few milli-seconds, while a strict confirmation paid
  `CONFIRM_PAUSE` once per identity per read and so cost about `0.95 s` per
  persistent zombie row (worst observed round 4.25 s of scan, 89 % of it
  pause). ESRCH confirmation is therefore interleaved: the first read for an
  identity still follows its own `getsid` immediately, and every identity
  still pending then takes one read per shared pass, each pass preceded by
  one `CONFIRM_PAUSE`, for at most `CONFIRM_TRIES - 1` passes. An identity
  keeps exactly its existing `1..CONFIRM_TRIES` reads with at least one full
  pause between successive reads, its first-read pairing, its contradiction
  tracking, its absent/replaced priority and its separate final read; a match
  never short-circuits; the per-identity terminal/taint/count rules and all
  bounds are unchanged. Published members, uncertain rows, diagnostics and
  terminal witnesses are assembled in snapshot order from per-row records.
  The serial algorithm paid its prescribed pause once per identity per read,
  so its derived wait grew as about `0.95 s x N` for N persistent zombie rows;
  the interleaved round shares at most `CONFIRM_TRIES - 1` = 19 pauses
  whatever N is, while the same reads and CPU work remain. Those figures are
  pause calls times the declared `CONFIRM_PAUSE`, not independently measured
  sleep duration, and the historical #140 cause stays inferred rather than
  reproduced. A tracked identity that fails its final read keeps its retained
  member attribution, exactly as the pre-correction tail did. This source
  remains **unqualified** until its own uncounted keep-going diagnostic and
  separate author and reviewer `proof:supervisor:qualify` receipts exist.

`scripts/bounded-test.py` is the shared proof supervisor. It implements the
reviewed [pinned-leader correction design](proof-supervisor-correction.md)
(revision 4, `29f0950e`) for oxide#109. It counts as qualified only once
that task's independent exact-head native passes; until then it is the
candidate under test. Earlier #109 evidence stays historical: at `f5a998c`,
the author's two native submissions, the reviewer's failed native and that
review's closure and deadline findings; at `22d1280`, author and reviewer
natives that passed 43/43 cases and review `6312725112`, which found five
defects. Tom's focused completion phase fixes exactly those five, each with
a regression, and routes nothing else. At `52e4a89` the author native passed
50/50, the reviewer native timed out in admission without launching, and
review `6318598212` found one more P1: a post-gate custodian exception
escaped shutdown. Tom's post-gate exception repair phase fixes only that.
Its second author native at `2abe9c6` passed 56/56. Review `6319610400`
then found the exception handler calling `traceback.extract_tb` (source
file lookup) before latching; the diagnostic repair below fixes only that.
Its author native at `07ce385` failed 59/60: the exception-during-stop case
still expected the exception's message text in the fault, which the repair
deliberately no longer includes. That case now expects the class name and
the recorded `stop_leader:` location. Author and reviewer natives at
`3d8e4f2` passed 60/60. Review `6320463419` then found two defects:
- readings stamped at send time, not completion, so a delayed publication
  could refresh an old measurement;
- the custodian's own setup doing blocking filesystem work before any
  deadline wait.

The owner admitted both fixes under Tom's continue ruling.

```sh
python3 scripts/bounded-test.py [--limit S] [--scratch-budget-mib N] [--evidence DIR] [--root-parent DIR] [--] command ...
```

- **Future proofs.** A proof is one short fixed script that is the whole
  supervised command, run as an Effigy `heavy` task:
  `python3 scripts/bounded-test.py python3 scripts/<proof>.py`.
  - It writes only inside `OXIDE_TASK_SCRATCH_ROOT` for its ordinary fixtures;
    a retained fixture image's intent, attachment and backing live in
    `OXIDE_PROOF_EVIDENCE`, the sibling evidence directory of the same
    monitored owned root, and an owned mounted image writes bind that backing.
  - It may read `OXIDE_PROOF_DEADLINE_NS`, the absolute `CLOCK_UPTIME_RAW`
    deadline, and write evidence to keep under `OXIDE_PROOF_EVIDENCE`, the
    evidence directory: counted in owned bytes, never removed by cleanup.
  - It may publish one bounded `admission.json` marker in
    `OXIDE_PROOF_EVIDENCE` when its own admission wait ends (below); the
    supervisor reports that wait and grants bounded time for it.
  - It must leave no process outside its leader's group when it exits: such
    a process is never signalled and ends the run `UNCLOSED`.
  - It adds no supervision or accounting of its own.
- **Processes.**
  - The single-threaded custodian owns the absolute deadline, the two pins
    and every signal. The pins are the monitor and the leader, both held
    unreaped.
  - The monitor owns all I/O: evidence and receipt phases, output draining,
    reserve and owned-byte sampling, the census and cleanup.
  - The leader calls `setsid` and execs the command only after the gate
    opens.
  - After setup, the custodian makes only bounded calls and touches no files
    or stdio.
- **Setup.** The custodian takes its clock and deadlines first, and its own
  setup is filesystem-free: attribute capability checks, signal handlers,
  run ID and environment copy. All blocking setup runs in the monitor,
  inside the 10 s setup budget:
  - the `kinfo_proc` layout check;
  - path resolution;
  - scratch and evidence creation;
  - the first receipt.

  The paths arrive in the monitor's `setup` message. The leader opens
  `/dev/null` itself, before its ack. A stall anywhere in setup, including
  a blocked filesystem call, leaves `ready` unsent, and the direct
  invocation is bounded the same way as under the bootstrap. A setup
  refusal, such as `OXIDE_TASK_SCRATCH_ROOT` not being a directory, is one
  non-blocking stderr line and exit 125. Python start-up and imports before
  the entry point are outside the bound.
- **Launch gate.** Nothing launches unless the custodian has received the
  monitor's `setup`, `ready` and completed reserve and owned-byte readings
  within the 10 s setup budget. If that fails, the pinned monitor is KILLed
  and the exit is 125.
  - The leader child acknowledges with exactly one byte after `setsid` and
    `dup2`.
  - The custodian then registers kqueue `NOTE_EXIT` and writes the gate
    byte.
  - A wrong byte, EOF, a missing detector, a fault, or a signal before the
    gate opens KILLs the pinned PID (never `killpg`). The command never runs.
  - SIGINT and SIGTERM are held blocked from the final check of recorded
    cancellation and faults through the gate write. A cancellation recorded
    or pending at that check wins, including one during or after detector
    registration.
  - SIGCHLD is set to default before any fork, so no pin is auto-reaped.
  - The child resets inherited signal dispositions and the signal mask
    before `exec`. An exec failure exits 127.
- **Signals.** Only an identity the runner holds unreaped across the signal is
  ever signalled: the pinned leader's group (TERM, then KILL after at most 5 s),
  the pinned monitor, and a direct child still held unreaped (the detach
  handoff; the #197 launcher-rule exclusions add no signal). Each signal is
  recorded with its target, pin and `held_unreaped` custody at that moment. On
  macOS a group whose only member is the unreaped leader answers `EPERM`, which
  is recorded and not a fault. No other group and no observed non-child PID is
  ever signalled (oxide#195, Tom's 2026-10-31 ruling `c096c8fc`).
- **Cooperative-session survivors (oxide#195).** The leader's group is not the
  whole session: Oxide starts Cargo as the leader of its own process group in the
  same session, so a leader-group KILL leaves Cargo's group running. Oxide#182's
  archived packet ended `UNCLOSED` with exactly one such survivor (a unit-test
  binary in Cargo's group). Such a process is not this run's child, so it is not
  held unreaped and its PID could be recycled between an identity read and a
  signal: it is **never** signalled, and the supervisor has no path that converts
  an observation into a signal. After the leader is reaped, each census round
  records, once per identity and up to sixteen rows, the exact member rows it
  still classifies in the leader's session (`pid`, `start`, `ppid`, `pgid`,
  `uid`, `zombie`) in the receipt's `survivors` list, and the summary line states
  how many were not signalled. Each row is recorded before the round that reports
  it is sent, and the census thread is joined (bounded, never past the deadline)
  before the receipt is published, so the evidence a verdict rests on is the
  evidence on disk. Closing such a survivor needs a boundary that holds the
  identity (Queue papercut `9ff7386d`); this change does not close the historical
  survivor and does not claim to.
- **Closure.** The leader is reaped first. Only after that does the custodian
  send `stopped`, so the final census never sees the leader. Closure is
  established when all of the following hold:
  - no fault is latched;
  - the census thread finished inside its bounded join, the last completed round
    is certainly empty, and the census MESSAGE the custodian's decision named is
    itself certainly empty (`census_settled`; the custodian echoes that message's
    sequence and facts, and the monitor resolves it against its own record of the
    messages it sent, so a late round or a decision without its own census
    evidence refuses closure);
  - a census started after `stopped` is certain, with zero members and zero
    uncertain rows, within 10 s (bounded by `exit_at`);
  - both output pipes have reached EOF.

  The census classifies every host process:
  - **Member:** `getsid` is the leader's PID, re-validated by a
    `KERN_PROC_PID` re-read of PID and start token. A member stays tracked
    until its identity is gone.
  - **Foreign:** a validated other session that was never a member.
  - **Gone:** `getsid` failed with `ESRCH` and a later successful
    `KERN_PROC_PID` read lacks the identity. A zombie still present is
    re-read within the round, every 50 ms for at most 1 s; when that
    confirmation does not establish absence, the round's existing final
    `KERN_PROC_PID` read for the same PID is still checked under the same
    rule and drops only the old tracked (PID, start) identity. A failed
    read confirms nothing. A replacement identity is never declared
    foreign or member by inference.
  - **Uncertain:** everything else, including first-seen unreadable rows,
    failed re-reads and unconfirmed zombies.
  - **Launcher (oxide#197):** a row a positive structural launcher rule
    excludes, in this order: its own `(PID, start)` identity is exactly one the
    monitor recorded for its own `ppid` chain in setup and is re-verified
    through the safe reader after the snapshot (`ancestor`); or its
    observation-time parent row has a PID above 1, exactly one of those recorded
    identities, re-verified the same way, and the row is positively outside the
    leader's session AND process group (`parent`, the launcher sibling). Each is
    neither a member nor uncertain. Only those positive facts qualify: a
    recycled PID or parent PID with a different start token, a changed identity,
    a missing parent row, an unreadable session or group, a PPID 1 orphan, a
    descendant, a same-session stranger and any row not excluded above keep the
    classification above, an unreadable or empty record excludes nothing, and no
    wall, boot or uptime value ever takes part. A `member` launcher observation
    is carried as a tracked member row; an undecided one excludes nothing and
    leaves the row to the ordinary classification.
  - **Terminal nonwriter** (allowlisted kernels only): an actual `getsid`
    `ESRCH` (errno-checked, not any `ProcessLookupError`) whose snapshot
    row and FIRST existing confirmation read show the same complete
    `SZOMB` identity (exact non-bool bounded ints, positive PID, exact
    `SZOMB` state in both) is accepted as terminal, provided every
    actually performed read in the round (all confirmation reads plus the
    existing final read, same 20+final bounds, no early short-circuit)
    shows the same complete `SZOMB` identity with no failure. Gone keeps
    priority. A terminal row is neither Gone nor Foreign nor a blocking
    member; tracked attribution is kept and eligibility is recomputed
    every round with no exemption cache. A nonqualifying first read is
    never salvaged by later reads; any failed, missing, malformed,
    identity/state or non-zombie observation disqualifies terminal and,
    in allowlisted mode, taints the round's terminal count as
    unaccountable, while rows that were never terminal candidates leave an
    accountable count alone. A successful `getsid` keeps the old
    member/foreign/tracked paths and other session errors (including a
    non-`ESRCH` `errno`) can never qualify. In strict mode terminal
    acceptance never fires and every `ESRCH` row keeps the old uncertain
    handling, while the terminal count stays the accountably known valid
    0. Apart from the two structural launcher rules above (a re-verified
    recorded ancestor identity, decided only by that rule; and a launcher
    sibling whose parent identity and own identity are re-verified around the
    session/group reads and which is positively outside the leader's session and
    process group), no live process is excluded by parent, a tracked member is
    never excluded, and a replacement identity is never declared foreign or
    member by inference.

  The launcher rules are the only parent-based exclusions: the monitor records
  its own chain once in bounded setup (at most 64 levels, from its owner parent
  through PID 1) and the census skips only a re-verified recorded identity, or a
  non-tracked row whose own identity and recorded parent identity are re-verified
  around the session/group reads and which is positively outside the leader's
  session and process group. The receipt carries that record as the bounded
  `ancestors` object and the positive exclusion evidence as the bounded
  `ancestor_exclusions` object, both described above.

  An uncertain or failed census after `stopped` latches a fault at once. A
  later certain empty census never clears it or authorizes cleanup.

  Start time is an identity token, never a liveness or ordering signal: the
  only exclusions it takes part in are the whole `(PID, start)` identity of a
  recorded ancestor or of a recorded parent of a launcher sibling. No wall,
  boot or uptime comparison excludes anything, because a masked backward,
  back-and-forth or forward wall step cannot be ruled out. Owned scratch is
  removed only after established closure.

  Kernel allowlist: once in monitor setup, three fixed-buffer `sysctlbyname`
  reads (`kern.uuid` 64 B, `kern.osversion` 64 B, `kern.version` 1024 B)
  decide the frozen mode. Only the exact reviewed build (UUID
  `8D3E13A0-01FC-381D-9DA3-7F0AF537CB32`, build `25G83`, version token
  `root:xnu-12377.161.14~5/RELEASE_ARM64_T6050`) enables terminal
  acceptance; any read failure, oversize, malformed value or mismatch
  freezes to strict mode. The receipt records the frozen mode, the reason
  (`allowlisted`/`identity_read_error`/`identity_malformed`/`identity_mismatch`)
  and the evidence ID. No setting can force terminal acceptance on an
  unverified kernel, and an OS update returns to strict until reviewed.

  Receipt-only diagnostics summarize `ESRCH` rows (confirm attempts and
  outcome with the final read result and an enumerated error category).
  They live only in the monitor's receipt census object, never in the
  census IPC message, and never change classification, counts, faults,
  closure, cleanup, reads, time or retry budget. The optional object is
  compact UTF-8 JSON (`ensure_ascii`, no whitespace) of at most 16384
  bytes with at most 20 rows and fixed enums only; overflow drops whole
  rows first with an explicit omitted count, or omits the object.

  Every receipt census object also carries mandatory `terminal_count_status`
  (`valid`/`invalid`/`overflow`) and `terminal_nonwriters` (the exact
  non-bool count 0..2^64-1 of this round's terminal-accepted still-present
  `SZOMB` identities, never Gone rows, when valid, else JSON null; a
  strictly completed round with none is `valid` 0). Unaccountable
  terminal candidacy (an actual-`ESRCH` row that could not be fully
  adjudicated), a snapshot or census failure without a countable set, or
  a malformed count is `invalid` with null; genuine count overflow is
  `overflow` with null. Uncertainty of unrelated rows (never terminal
  candidates, e.g. a live row with an injected session failure) does not
  itself invalidate an accountable count. An `invalid`/`overflow` count
  always coincides with `uncertain`>=1 and `certain` false in the
  unchanged IPC fields (forced when necessary); optional diagnostics
  saturation never drives authoritative counts or classification.
  Output-witness loss never touches these fields and they never change
  classification.
  Optional terminal witness rows (PID, start, both qualifying `SZOMB`
  states, prior tracking) share one combined cap with the diagnostics of
  at most 20 rows and 16384 compact bytes; witness loss only ever means
  missing qualification evidence.
- **Faults (latched; none clears).**
  - Monitor: reserve below 22 GiB (20 GiB reserve + 2 GiB margin); no
    reserve reading for 1 s; no completed owned-byte reading for 10 s; owned
    bytes over budget (default 512 MiB); a measurement error; monitor death.
    Freshness counts from each reading's `completed` stamp, taken when the
    measurement finished, never from when its message was sent. A
    completion after the send stamp is a fault.
  - Protocol: wrong run ID, sequence gap, a leader identity that is not the
    pinned leader or is dropped after binding, `ready` before readings, line
    overflow.
  - Process: signal or reap errors; SIGINT or SIGTERM to the custodian.
  - Exception: any exception in the custodian after setup, and a monitor
    message with a missing or ill-typed field (`protocol: malformed`).
    The fault is latched first, as `custodian exception: <class name>`,
    with the name read from the C-level type slot. The custodian never
    converts an exception to text (no `str`, `repr` or format) and never
    uses `traceback` or `linecache`. Only then is the innermost function
    and line recorded, read from the traceback object's own fields; a
    failing record never undoes the latch. OS errors are reported by
    `errno` name.
    Each phase after setup (launch, watch, stop, reap, census, finish) runs
    under its own guard, so the bounded pinned stop (TERM, grace, KILL; the
    KILL is sent even if the stop itself raises), WNOHANG reap, census and
    `keep` still run. A kqueue failure degrades the wait to `select` on the
    control pipe, so monitor messages and sampling continue through
    shutdown. Exception records are capped at 8. An uncaught top-level
    error writes one line of at most `PIPE_BUF` bytes only if stderr is
    writable at once, with no traceback, and exits 125.
  - Closure: an uncertain census after `stopped`; no certain empty census
    or a missing EOF witness within the census window.
  - Clock: a reading stamped later than the custodian's own clock reading
    when it arrives. Both sides use the same system-wide clock, so such a
    reading is never fresh.

  Freshness measures the steady-state sampling window, and it begins when
  sampling actually begins — the monitor's `ready`, after its first completed
  readings — never at custodian start (decision `50acd33b`, papercut `90e93f06`).
  A cold setup, whose bounded startup sweep of a shared parent can take longer
  than 1 s, is owned by its own 10 s setup deadline and can never trip the 1 s
  reserve window; the steady-state windows (1 s reserve, 10 s owned bytes) and
  the setup deadline are unchanged. It also ends with the decision: once the
  custodian has sent its `cleanup`/`keep` decision, the monitor's samplers stop as
  it publishes `monitor_complete`, so the monitor's shutdown is not a freshness
  window (oxide#192; a stalled sample during shutdown cannot turn a completed,
  closed and cleaned run into exit 125). Threshold faults from readings that do
  arrive, every closure rule and the whole settle phase are unchanged, and a
  stale reading while the run is executing still latches.
- **Clock and deadlines.** One clock everywhere: `CLOCK_UPTIME_RAW` integer
  nanoseconds. The supervisor refuses before any proof work if that clock,
  kqueue, macOS or the self-checked `kinfo_proc` layout is missing.
  - `stop_at` is the limit minus 20 s.
  - `exit_at` is the limit minus 2 s.
  - Every wait is a kevent or select timeout within the remaining budget.
  - An admission marker can add at most its own reported wait, this run's own
    elapsed wall clock and 600 s in total to the deadline, never shortening
    it (below).
- **Admission marker (oxide#192).** A supervised child that waited for
  admission may publish one bounded marker, `admission.json`, in the evidence
  directory: exactly the keys `schema`, `source`, `run`, `command` and
  `waited_ns`, schema 2, where `source` and `command` are printable tokens of at
  most 32 characters, `run` is this supervisor's own exported
  `OXIDE_PROOF_RUN_ID` and `waited_ns` a non-negative wait inside `u64`, at most
  512 bytes, published by rename. A document whose `run` is not this run is
  rejected as `stale run` and never paired with this one (the runner rejects the
  retired schema-1 three-key shape too). `oxide` writes it when a leased
  command's own slot admission grant ends (its measured interval from the first
  attempt to the grant), armed only by this supervisor's exported layout.
  Admission sampling starts together with the reserve and owned-byte samplers,
  before the monitor's `ready`: a pre-existing, malformed, stale or foreign
  document is rejected and counted there (a rejection is also a control message
  the custodian counts as an observation) and grants nothing, because only a
  document carrying this run's identity can be paired and the child receives
  that identity only after the gate opens. No marker path can fault the run,
  refuse it or abandon its launch. The monitor polls the path every 0.25 s, opens only a regular file
  without following a link, reports one marker per file identity and stamps
  the moment it read it. Deduplication binds to the identity of the file it
  actually opened (`fstat` on the open handle), never to an identity sampled
  before the open, so an atomic replacement that lands between the sampler's
  `lstat` and the read is reported once and never again on the next poll; and
  the custodian grants one command's wait at most once, counting and ignoring a
  duplicate completion. The custodian adds at most that reported wait, never
  more than this run's own elapsed wall clock and never more than 600 s in
  total, to `absolute`, and recomputes `stop_at` and `exit_at`. Nothing is
  ever shortened and `OXIDE_PROOF_DEADLINE_NS` is not rewritten, so a proof
  script that bounds its own work from it keeps its unextended cutoff and gets
  no benefit from a grant.

  **The stop boundary.** When the watch loop reaches its stop it makes one
  bounded drain of the control pipe before it declares the deadline, so a
  marker that was already published and is waiting to be read can still grant
  and move the stop; the loop continues only when the grant moves the stop past
  the current time, so a marker that grants nothing cannot spin it. A marker
  that reaches the custodian after the stop decision or after a deadline was
  declared cannot move anything: it is recorded as `admission_late` with its
  reason (`deadline` or `stopped`), its observed stamp and its reported wait,
  and the deadline classification stands. The summary says so in its own line.
  A run that stopped as admission completed is therefore reported as a deadline
  with a late admission, never as an unexplained timeout, and no clock is
  moved after the decision. A marker the child could not publish before the
  stop is simply absent: the supervisor never waits past its deadline for one,
  and the product cancels its waiting lease on a signal rather than reporting a
  grant it did not get.

  **A wait that never ends, correlated per command.** Both schema-2 documents
  name the run and one bounded command token. The monitor rejects any document
  whose `run` is not this run (reason `stale run`) and keeps the rejection in
  the report's own bounded list, so a file left by an earlier run is never
  paired with this one. `admission-waiting.json` has exactly the keys
  `schema`, `source`, `run`, `command` and `reason` (two printable tokens of at
  most 32 characters), at most 512 bytes, and is published by the child when a
  wait starts; it never moves `absolute`, `stop_at` or `exit_at`. The custodian
  keeps a bounded pairing table (64 commands) and pairs a command's waiting
  report only with that command's own completed report. Its wait **ended** when
  its own completion arrived, at any duration: a zero-duration grant, a grant
  clamped by the cap and a late grant all end it. An unpaired wait is **never**
  described as a proven unfinished wait: missing terminal evidence is unknown.
  The completion file has one fixed name, so a later admission-file observation —
  another command's grant or waiting report, or a **rejected read** — may have
  replaced a completion before the next 0.25 s poll, and a completion published
  between the last poll and the sampler's stop can be missed entirely. The
  decision record therefore carries only `unpaired` rows with a
  `later_observation` fact, and the summary says either that a later admission
  report or a rejected read may have overwritten the completion, or that no
  admission-file observation followed the wait while its terminal evidence is
  still missing; both are unclassified, and only the observed terminal reports
  (a grant, a zero-duration or capped grant, a late grant) end a wait. A rejected
  or overflowed report is unclassified in the same way. A terminal for a command
  the bounded pairing table cannot retain (more than 64 commands) grants
  **nothing**: without a record the custodian cannot tell a first terminal from a
  duplicate, and a missed extension only makes the run stricter. A duplicate
  terminal for a retained command is likewise counted and ignored, so one command
  can never move the deadline twice for the same wait. Together with the grant and late
  records this makes the states distinguishable in one receipt: granted before
  the stop, granted after it (late, no clock moved), unpaired (nothing granted,
  unclassified because its terminal may have been overwritten or missed), stale
  (rejected by run), and rejected or overflowed (unclassified). A missing, malformed, oversized, linked or unreadable
  marker is recorded in `receipt.admission` with a bounded rejection list and
  nothing else happens: it never latches a fault and never changes closure,
  EOF, cleanup or classification. The grant is cooperative evidence about the
  child's own admission, not a claim about any other writer. `receipt.admission`
  records the observations and the reported waits; each accepted grant and each
  late marker is also a `receipt.custodian` event (at most 32 kept), and one
  best-effort `admission_decision` record carries the decision-time totals, the
  late reason and stamp, the cap and the effective deadline. The summary prints
  the reported wait, the grant and the effective deadline separately, so a
  queue wait is reported honestly rather than consumed from the work window.
  The cap bounds the run: markers can never move the deadline by more than
  600 s.
- **Exit and receipt.**
  - Exit codes: 124 at a deadline; 125 for a fault, refusal or `UNCLOSED`;
    otherwise the command's status (128+N for signal N). Every 125 is explainable
    from one bounded stderr line (papercut `bc0d56cd`): a launch refusal prints
    `not launched: <reason>`, a failed finish prints its deadline and latched
    fault, and an unexpected monitor failure prints its exception class name —
    never a traceback and never silence.
  - The monitor publishes `receipt.json` phases by atomic replacement:
    `started: incomplete`, then `closure_established: cleanup_pending` or
    `unclosed: scratch_kept`, `cleanup_partial_or_unknown` before deleting,
    `cleanup_complete`, and `monitor_complete: pending_custodian`.
  - There is no success phase. Success is `monitor_complete` plus custodian
    exit 0, recorded by the caller.
  - Output persists up to 8 MiB per stream in the evidence directory, is
    forwarded up to 32 MiB, and the rest is counted and discarded.
- **Scratch.** Without a caller's `OXIDE_TASK_SCRATCH_ROOT`, it owns
  `<root-parent>/oxide-proof.*/{scratch,evidence}`. The default root parent is
  the owned `<mount>/scratch/oxide` subtree of the selected volume described
  under [temporary scratch placement](#temporary-scratch-placement): the
  volume the effect-free pool selection names, never the boot volume and never
  the drive root, with the warned `/private/tmp` fallback; `--root-parent`
  overrides it with unchanged caller semantics and creates no marker. It
  passes `scratch` as `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP`,
  and counts the whole root, evidence included. A caller's root is adopted and
  never removed; evidence goes in a `supervisor-evidence.*` directory inside
  it. Evidence directories are kept for review. Reserve sampling reads the
  filesystem holding the scratch in every case, and the receipt records
  `scratch_default` (source, prepared parent, pool, registration, mount,
  volume UUID, device) with the reserve path and device, plus the bounded
  startup sweep summary (`scratch_sweep`) and the owner-marker state
  (`scratch_marker`).
- **Owned bytes.** The monitored count is `st_blocks * 512` for every
  distinct (device, inode) under the root, with no symlink followed, taken
  every 2 s. `PC_PATH_MAX` is only the choice between two lookup forms: a
  path name at or past the platform limit, which no absolute-path call can
  open, is reached from its nearest nameable directory one short
  `O_RDONLY|O_DIRECTORY|O_NOFOLLOW` component at a time, so an owned fixture
  at the limit is counted instead of faulting the reading (oxide#142; without
  it ordinary full-suite sampling faulted with `ENAMETOOLONG` while the #141
  `PATH_MAX` fixture existed, which is the fault the #133 fault run records;
  no #133 proof credit or acceptance follows). No frame keeps a descriptor
  between iterations, so descriptor use stays constant whatever the depth or
  path length. The root itself must still be nameable, a per-filesystem limit
  below the platform's still faults, and a substituted symlink at an anchor is
  an error rather than a walk into another tree: only `FileNotFoundError` is
  ever a vanished entry, so a kind or identity contradiction (`ENOTDIR`,
  `ELOOP`, `EACCES`) still faults. A vanished entry — at its stat, at a
  relative directory's final no-follow open, or at that directory's read — is
  still skipped, and every other error is a measurement fault, never a smaller,
  partial or zero total. The cadence, staleness bound, monitored budget and
  no-follow rule are unchanged; this is a non-atomic observation and claims no
  completeness for a name the platform cannot form.
- **Scope.** Cooperative session only. A `setsid` escape is seen only if
  already tracked as a member or while it holds an output pipe. Arbitrary
  same-user writers are outside the promise. The reserve check is a sampled
  abort threshold, not an instantaneous guarantee. The budget is monitored,
  not a hard ceiling. No hard real-time scheduling claim. macOS only.
- **Test hook.** `OXIDE_SUPERVISOR_INJECT` (test-only) injects launch, monitor,
  census, cleanup and reading faults for the qualification suite.

**Qualification.** `effigy proof:supervisor:qualify` (heavy) is a bootstrap
around the candidate; the bootstrap is not itself a qualified guard. Its own
bounds:
- a clean checkout;
- reserve readings before and after, on the volume that actually holds the
  qualification's scratch, with the boot volume read and labelled separately
  (both keep the 22 GiB threshold);
- a fixed 600 s `CLOCK_UPTIME_RAW` deadline held by a minimal outer actor
  (oxide#192 amendment, 2026-10-31; history: 300 s originally, 420 s under
  oxide#191 when the suite needed 251-258 s against the old 250 s alarm). The
  suite's own alarm is now 530 s, the 560 s candidate limit less its 30 s
  margin; the first direct diagnostic then exhausted oxide#191's 350 s alarm at
  357 s wall (candidate exit 142 = SIGALRM, its pure suite OK).
  The actor forks the whole bootstrap as a pinned worker and makes only
  WNOHANG reaps, bounded sleeps and signals to that worker. All file,
  hashing and stdout work runs in the worker. At 598 s the actor KILLs the
  worker and exits 124, so blocked output or a slow read cannot extend the
  deadline. A candidate still running then is bounded only by its own
  560 s limit (380 s under oxide#191, 280 s before);
- on candidate overrun, TERM then KILL of its own unreaped candidate child
  only, from its own reap loop, so no signal can follow the reap.

The bootstrap deletes nothing. It prepares the owned
`<mount>/scratch/oxide` parent through the candidate's own seam, runs one
bounded startup sweep there before creating its base, and marks that base
before the candidate starts; the candidate's own setup then runs its one
bounded sweep, which must have seen and kept that unconcluded base. Real case
directories live under the candidate's `OXIDE_PROOF_EVIDENCE`, outside the
candidate's cleanup. `UNCLOSED` cases leave live fixture processes whose only
bound is their finite fallback, and fallback expiry is not closure. The
scratch base is therefore kept, and its path is printed for the owner. A base
or candidate root that carries a retained nested `UNCLOSED` fixture stays
unconcluded with an honest reason even when the outer session closed and the
suite passed, so it can never expire; only a fully concluded base or candidate
root older than seven days is eligible for a later run's bounded sweep.

It runs `bounded-test.py --limit 560 --scratch-budget-mib 512` around
`scripts/test_supervisor.py` with **no** `--root-parent` **and no
`--evidence`**: the candidate's own default scratch routing runs, and its
evidence directory is its own `<root>/evidence`, so the suite's nested
real-case directories (created in `OXIDE_PROOF_EVIDENCE`) stay inside the
candidate's monitored owned root on the selected volume. The candidate's
owned-byte walk and 512 MiB budget therefore cover every nested case
resource, including a kept `UNCLOSED` case's scratch, while the evidence
directory is never removed by cleanup. The suite records that layout in
`topology.json` under the bootstrap's base (one new directory on the same
selected volume, kept by the bootstrap, which deletes nothing). The bootstrap
resolves the same passive selection first and, if it is the `/private/tmp`
fallback, refuses before creating anything (exit 2) instead of spending a
native run that could not exercise the default. Its pass check then
re-derives, independently of the candidate's claim: the receipt's
`scratch_default` is the selected volume, the suite's recorded root, scratch
and evidence are one owned root directly on that volume with the receipt
agreeing, every nested case directory lies inside it with its receipt still
present after cleanup, that volume is not the boot volume, its device and
`ATTR_VOL_UUID` are the scratch's own and the registration's, the root is
outside the pool payload, and the reserve was sampled on the scratch's device
with a recorded minimum. A fallback run, or a nested case outside the
monitored root, fails the qualification. Its reserve readings identify the
qualification's scratch volume and the boot volume separately. The suite
runs in order:
- 361 pure cases: guard thresholds, staleness and errors, a future-stamped
  reading, delayed reserve and scratch publications that do not refresh,
  and a completion after its send stamp; the custodian's setup with every
  filesystem call forbidden; `ready` before `setup`; exit precedence;
  census member, foreign, identity change, first-seen unreadable, zombie,
  gone, `ESRCH` with a failing re-read (uncertain), a zombie confirmed gone
  within the round, escaped and reparented member, backwards start time,
  recycled PID, failed snapshot; protocol run, sequence and leader-binding
  faults; ready before readings; census before reap; an uncertain census
  latched over a later empty one; malformed and ill-typed monitor
  messages latched without raising; the top-level report not blocking on a
  full pipe; the exception handler latching with `linecache`, `traceback`,
  `open` and `stat` all forbidden, latching when its record raises, and
  never converting hostile exception text (hostile `__str__`, `__repr__`,
  `__format__`, metaclass `__name__` and `str`-subclass name); refusal to
  signal an unpinned target; the final-read absence rule (matching
  exhaustion then a successful final `None`/replacement is gone for
  tracked and untracked rows, persistent matches and failed or non-`ESRCH`
  reads still block, an earlier confirmation read failure with a positive
  final absence is gone, a replacement is never foreign by inference) and
  bounded receipt diagnostics (fixed enums, 20 maximum-width rows, row
  overflow with an exact omitted count, byte-ceiling trimming and
  serialization failure leaving classifications, counts and faults
  unchanged); mandatory receipt terminal counts (`valid` 0 for a strictly
  completed round or unrelated uncertainty, exact terminal-`SZOMB` count
  when valid, `invalid`/`overflow` with null and forced `uncertain`>=1 /
  `certain` false otherwise, IPC schema unchanged, Gone rows never
  counted, diagnostics saturation never driving the count); the
  exact-kernel allowlist (match, ASCII form, build mismatch, malformed
  UUID length, wrong UUID value, read error, oversize, interior-NUL and
  missing-NUL versions, contradiction, missing token, deterministic
  fake-libc fixed-reader success/oversize/error) and terminal eligibility
  (allowlisted zombie
  accepted with exact witness and kept attribution, strict/live/
  first-read-failure/intermediate-failure/intermediate-live/final-failure/
  final-live/non-`ESRCH`/non-`ESRCH`-errno/replacement never terminal,
  all-actually-performed-read tracking without short-circuit, tracked
  member forcing uncertainty on unaccountable counts, tracked recompute,
  combined witness budget keeping the count); owned-byte traversal of owned
  fixtures that live at and past the platform limit (a regular file one byte
  past it counted, the limit shown to be a byte rather than a character
  rule, a directory past it read, a hard-link pair counted once with the
  union's saving measured against a no-union total, a symlink that names a
  directory never followed, an unreadable such directory raising instead of
  reporting less, a missing root counting nothing, and an ordinary tree
  matching an independent descriptor-relative oracle); three synthetic-FS
  controls over test-only seams that wrap `os.open`/`os.stat`/`os.close`
  (never a monitor hook) and count their own hits, so a seam that does not
  fire fails: a directory that vanishes at its final no-follow open is
  skipped with the rest of the walk counted, no descriptor left open and
  nothing opened outside the fixture; a directory reported where a symlink
  sits faults (`ELOOP`/`ENOTDIR`, never `ENOENT`) with the referent never
  opened; and seams whose target never appears change nothing;
- the qualification topology with injected device/free-space facts and real
  owned-root fixtures: a recorded owned root whose scratch and evidence are
  its members with a nested case inside it and its receipt retained (the pass
  case), a nested case outside the root, an empty case list, a receipt whose
  evidence is not the recorded one, a root not directly on the selected
  volume, a non-canonical record, an unreadable root, a wrong-device root,
  an unreadable boot volume, a boot-volume scratch, a mismatched reserve
  device, a root inside the pool payload and a fallback source, plus reserve
  readings that read and label the scratch volume and the boot volume
  separately;
- the default scratch parent with injected facts only: a registered drive, a
  configured pool on the drive, `OXIDE_POOL` precedence, an internal pool
  without a registration, an absent mount point, a stale mount point on the
  boot volume, a different volume under the registered name, a symlinked and
  an uncanonical mount, an unreadable volume UUID, a writable `/Volumes`, an
  unreadable or malformed registration, the narrow config path forms and the
  built-in pool, the bounded single-line fallback warning, `--root-parent`
  and adopted scratch keeping their own contracts, that the qualification
  bootstrap still passes no `--root-parent` (and does prepare the owned parent
  and sweep before its own base), that the unit-suite recipe in `AGENTS.md`
  derives `<mount>/scratch/oxide-validation` once from the mount (never from the
  nested `root_parent`, never a hardcoded mount) and creates nothing itself,
  that a selected volume reports `<mount>/scratch/oxide` without creating it, and the
  owned leaf's privacy, ownership, device and canonical refusals with the
  refused leaf removed;
- the preparation seam's other invariants on real owned fixtures: an existing
  safe private leaf kept byte- and inode-identical, a concurrent creator's leaf
  validated rather than adopted (including its 0755 refusal without a chmod),
  and symlink, non-directory, non-private, foreign-owner, other-device,
  unreadable, non-canonical, wrong-parent, wrong-mount and non-selected-volume
  refusals that leave the component in place and create no sibling. The shared
  `<mount>/scratch` ancestor is never created: a missing one refuses with no
  mkdir, and only the private `oxide` leaf is ever created (0700);
- the owner marker through its real write and read seams: an initial marker
  round-trips privately (0600, no temporary file, null conclusion, running
  outcome), and every schema divergence fails closed — extra or missing field,
  unknown version, malformed run ID, mismatched root/device/inode, wrong owner,
  negative/NaN/infinite creation time, ill-typed conclusion, unknown outcome,
  closure or kept-reason, a conclusion without positive closure and a closure
  claimed while unconcluded — plus absent, symlinked, non-private,
  foreign-owned, oversized and non-JSON markers, refused writes that leave no
  file, and conclusions that only a terminal closed/positive pair can record;
  no marker field is a PID or liveness fact;
- the bounded startup sweep against real owned fixtures and an injected wall
  clock: a positively concluded root older than seven days removed whole, the
  exact seven-day boundary and young conclusions kept, eligibility decided by
  the kept-reason rather than age (both expirable reasons removed, every
  non-expirable one kept), old unconcluded/unclosed/nested-UNCLOSED/failed
  roots kept, unmarked, foreign-owned, symlinked, plain-file, other-device and
  unrelated entries untouched with their referents intact, corrupt, oversized,
  non-private, symlinked or identity-mismatched markers and future-dated
  conclusions kept, a symlink or foreign entry inside an eligible root
  stopping its deletion with the marker and root kept, entry-budget and
  time-budget stops leaving a partial root whose marker survives for a later
  retry that finishes it, no cross-root deletion, a marker or root identity
  that changes during the sweep refused with its payload intact, a foreign or
  symlinked parent refused and an absent one reported, and fixed bounded
  summary rows with an exact omitted count;
- the pre-existing private parent and the operator's relocation: a real safe
  0700 current-user `<mount>/scratch/oxide` that already holds a nonempty
  unmarked `legacy-retained/` tree (three unmarked historical roots with nested
  UNKNOWN sentinels) is accepted by the real preparation seam, its components
  keep their inodes, and one bounded sweep over it removes only the marked,
  positively concluded peer: the legacy entry is skipped as one entry with no
  descendant enumeration (recorded `listdir`/`open` calls never name it), gains
  no marker, loses no sentinel, and a new separately marked run root still lands
  safely in that parent;
- the nested-window protection and the bootstrap's result-versus-custody
  transitions: the suite protects its outer root as `hosting` through the real
  seam before any case (and never adopts an unmarked root, refuses a corrupt
  marker and stops before the cases when protection fails); an outer closure
  can never conclude a hosting root; a passing qualification that retains nested
  UNKNOWN resources records `retained`/`nested-unclosed` with a null conclusion
  for both the candidate root and the base and is still a PASS; a fully
  concluded qualification records the positive `evidence`/`qualify-base`
  conclusions that the sweep may later expire; a failed qualification keeps the
  base `qualify-failed`; a fabricated hosting conclusion, a mismatched
  protection record, an unmarked root and every marker read/write/re-mark
  failure are reported problems, never silent passes; and the pre-correction
  marker shape (closed/positive/evidence without `nested_scope`) is refused by
  the sweep at any age;
- the peer-fault boundary: `unlink`, `rmdir`, `open`, `listdir` and a
  disappearing child inside one eligible peer each become a bounded skip row
  with the peer's marker retained, a healthy peer beside it is still swept, and
  this run's own new root is still created on the selected parent;
- the bootstrap's `qualify()` entry seam through the real function body with
  in-memory mocks only: platform, git, selection, the module candidate loader
  and the guard's probe are faked, every host, filesystem, directory, sweep,
  marker, probe-read and child seam raises if reached, and the body must resolve
  the loader and stop at the preparation boundary with no forbidden effect; a
  static companion asserts no local binding in the entry shadows a module helper
  it calls, and an in-memory negative counterfactual re-names the subprocess
  handle back to the shadowing name and must fail with `UnboundLocalError`
  without running the loader (the diagnostic2 bootstrap defect, caught without
  any native run);
- the shared-ancestor / private-leaf preparation on real owned fixtures: a
  `0755` shared `<mount>/scratch` accepted with only the private leaf created
  (ancestor inode and mode unchanged), a `0700` ancestor also accepted, sticky
  refused when it is also group/other writable and accepted when it is not,
  symlinked, non-directory, foreign-owned, other-device, non-canonical,
  unreadable and group/other-writable ancestors refused before any leaf effect
  with no mutation and no leaf created, a missing ancestor refused with no
  mkdir at all, and a private leaf that is `0755`, `0750`, `01700`,
  foreign-owned, symlinked or on another device refused with zero chmod or
  adoption (the leaf's mode and inode survive); an absent leaf created exactly
  `0700` and re-verified, a changed post-create identity refused, a concurrent
  creator's `0700` leaf validated and its `0755` leaf refused without a chmod,
  and three contract counterfactuals: the superseded both-private-parent rule
  would reject the now-accepted `0755` ancestor, a writable ancestor is
  refused, and a leaf below exact `0700` is refused even with no group/other
  write bit;
- the effect-free readiness seam through the production validation code: a
  ready world reports `absent; would create 0700` and creates nothing, an
  existing leaf reports `exists` with its exact mode, and group-writable,
  missing-ancestor and sticky-writable worlds are refused exactly as the real
  preparation refuses them, with no leaf or ancestor created; a fallback
  selection reports its own reason;
- the qualification checks for this routing: a real owned tree whose parent is
  `<mount>/scratch/oxide` passes end to end, while the bare mount as parent, a
  root created directly at the drive root, a missing sweep summary, a sweep on
  another parent, an unrecognised sweep stop, invalid counts, a closed
  candidate without a marker conclusion and a missing bootstrap base are each
  refused;
- the closure decision path with fake events only: both DISTINCT stream EOF
  witnesses may close without a fault, one or none refuses, a duplicate stdout
  witness never counts as two, an uncertain census refuses before any EOF
  question and its fault is preserved, a pre-stop census does not qualify, and
  the descriptor-transport API the native fixtures need is present;
- the three new case oracles and their negative mutations (escaped writer:
  absent child, no escape, stale nonce, no response, deadline, unrelated fault,
  not retained, suite or escaped-child signalled, leader failure; held stream:
  wrong pipe, no hold, uncertain or nonempty or missing census, census fault,
  deadline, not retained, suite signalled; released pair: an always-UNCLOSED
  oracle, a fault, a wrong exit);
- the settlement observation through the real monitor and custodian paths,
  judged by an oracle that the cases also falsify: a driven census loop writes
  the round's stamps and counts and keeps the control message's exact existing
  fields, a loop that exits before a round is told apart from a certain empty
  round, a frozen clock is rejected as no observation, a missing stamp, a
  reordered or duplicated field, an unknown phase, a wrong index, an invalid
  integer, a completed round with no record and a round without its kept or
  omitted count are each rejected, a round beyond the cap takes nothing while
  `omitted` counts it, a maximum-width record stays under the byte ceiling, a
  missing or ill-typed `stopped` stamp is unknown rather than a latency, the
  census counters follow the round's real reads and pauses and reset per
  round, and the custodian's decision-time snapshot names its message counts,
  eligible stamps, EOF set and fault while a failed observation write never
  latches or reopens a closure;
- writers before and after a simulated staging step;
- the escaped writer: it leaves the nested session, keeps the leader's
  stdout/stderr descriptors open, signals readiness and answers a fresh
  post-verdict nonce (so liveness is shown, not inferred), and exactly one of
  the two settle refusals must latch with no signal to the escaped writer or
  the suite, the leader at status 0 and the case resources retained;
- EOF-veto fixtures: the nested command sends duplicates of its ACTUAL stdout
  or stderr write descriptor over a short AF_UNIX socket inside the case's
  owned directory to this suite process (never a member of the nested session),
  the suite takes custody, acknowledges and holds them; the held-stream cases
  must latch exactly the missing-EOF refusal after a certainly empty census,
  and the paired control that releases both closes with status 0 and no fault;
- 39 real cases with tiny Python fixtures and injected faults:
  - exit 0, owned cleanup, nonzero, exec 127, output saturation;
  - a case directory that already holds the owned long-path fixture: the
    first scratch reading counts it, closure holds and the command runs (before
    oxide#142 that reading faulted `ENAMETOOLONG` and nothing launched);
  - a held direct-fork zombie child witnessed as the exact (PID, start)
    terminal nonwriter with `CLOSED` status 0 and no fault, reaped only
    in `finally`; a live owned child with an explicitly injected session
    error blocking with terminal `invalid`/null;
  - a group cascading to `closed` at the deadline;
  - a separate-group survivor `UNCLOSED`, never signalled and still alive at
    the verdict, its case directory and receipt outside the outer cleanup;
  - an escaped pipe holder, kept the same way;
  - a latched fault with an empty census (kept, not cleaned);
  - SIGTERM and SIGINT to the custodian;
  - gate EOF, wrong byte, cancel before the ack, no detector, a real
    SIGTERM after detector registration, and a SIGTERM pending at the gate
    decision;
  - setup stall, monitor death, monitor stall, uncertain census, a first
    uncertain census followed by empty ones (owned scratch kept),
    interrupted cleanup, monitor completion followed by a custodian timeout;
  - a reserve reading delayed 0.5 s under the monitor's I/O lock, and a
    scratch reading delayed 4 s, each followed by a stalled sampler. The
    stop must come between the 1 s or 10 s bound and that bound plus
    0.3 s, measured from completion;
  - the direct runner, with no bootstrap, whose first receipt open blocks
    on a FIFO. It exits 125 within 13 s, launches nothing and claims no
    receipt. No reading arrives while setup is blocked, so the 1 s reserve
    staleness guard usually stops it first; the 10 s setup deadline is the
    backstop. The author native at `f3bde11` failed 67/68 only because this
    case expected at least 10 s; it stopped at 1.13 s;
  - the bootstrap actor KILLing a worker blocked on an unread stdout pipe
    at its deadline (exit 124);
  - post-gate exceptions: an event-loop exception, a failing kqueue, a
    malformed monitor message, and an exception during the stop. Each has a
    TERM-ignoring leader and group child: pinned TERM then KILL, the child
    dead, the leader reaped, the last reserve sample no later than and
    within 1 s of the monitor's final publication, owned scratch kept,
    exit 125. The author
    native at `74f1b65` failed only the earlier, stricter form of that
    sampling assertion (a sample after the reap) in two cases;
  - a fixture deadline holding across `exec` and for a late grandchild, with
    SIGALRM inherited as ignored and blocked.

Every fixture arms SIGALRM to `FIXTURE_DEADLINE_NS`. The order is case
verdict 45 s (the detach class's own outer window is 75 s, because its nested
guard's stop is at 40 s and its exit at 58 s; a timeout there retains the mount,
backing and evidence and infers no detach outcome) < fixture fallback 60 s <
qualification 600 s, and fallback
expiry never counts as closure. The bootstrap requires all of:
- candidate exit 0;
- phase `monitor_complete`;
- established closure with status 0;
- the default scratch on the selected volume inside the owned
  `<mount>/scratch/oxide` subtree, as above, with the bootstrap's own base still
  present after the candidate's bounded startup sweep;
- the candidate's bounded startup sweep summary and its deferral of its own
  marker conclusion to the nested window, with the suite's protection record
  and the marker agreeing on one run identity;
- reserve sampling that began before the gate, with no gap over 1 s;
- the fixture writers counted;
- custody at every signal;
- all 416 cases (361 pure + 55 real, including oxide#191's post-census detach
  cases, oxide#192's five admission-report cases and its sampling-freshness
  cases, and oxide#197's structural launcher-rule cases with the launcher-sibling
  and slow-setup real cases);
- every real case directory and receipt still present after the
  candidate's cleanup.

It prints a receipt binding the commit, SHA-256 of all three sources, the
platform, each case's exit, phase, outcome and signal custody, the suite's
recorded layout, the prepared parent, both bounded sweep summaries, both
marker states, the scratch-volume and boot-volume reserve readings, the kept
base and `UNCLOSED` case directories, and the limitations. Admission waits
fall outside its 600 s: that is the amended actor cap (oxide#192 raised it to
600 s from #191's 420 s, itself from 300 s) with the 560 s candidate limit and the
530 s suite alarm, and a candidate marker can only move the candidate's deadline (by at most
600 s) inside it.
`effigy proof:supervisor:test` runs the pure cases alone.

**Uncounted keep-going diagnostic.** `effigy proof:supervisor:qualify:diagnostic`
(heavy) runs the same bootstrap (`qualify-supervisor.py --diagnostic`), guard,
fixed 600 s absolute deadline (oxide#192 amendment; 420 s under #191, 300 s
before), the 560 s candidate limit, the 530 s suite alarm, selected-volume routing and the unchanged
512 MiB monitored scratch with its 30 s shutdown margin, and is required before
the counted author qualification at a changed head. It runs each named qualification oracle on its own — receipt phase,
candidate outcome, reserve sampling, default scratch, sweep, protected root,
nested cases, owned bytes, signal custody, the detach cases and the case total —
so one failing check never hides the rest, records a bounded
`qualified: false` receipt with its per-check rows (`checks_passed`,
`checks_failed`, `claims`) and the case-level `collection` object (reached,
completed, failed and unexecuted against the bootstrap's own count, with the
first fault), and always exits non-zero. A precondition refusal in this mode
also prints one bounded `qualified: false` receipt before exiting. The receipt
is never a counted PASS, never a qualification and never evidence for another
proof, and a skipped or unexecuted check is never a pass. Earlier history is
preserved: the `96f0767` collection recorded a fixture `NameError` as one failed
case and kept going (corrected, not excused), and the `93bb6db` diagnostic and
counted receipts stay bound to their own bytes.

The earlier `test rust`, profile, `rust:pool-scale`, `rust:oxide-exec` and
`rust:front-door-accounting` evidence under the old guard at `3d947a9` stays
historical. Front-door accounting now uses the shared supervisor's default
selected-volume scratch and a private `OXIDE_SCRATCH_ROOT`; its migrated-head
native chain remains pending. Pool-scale's own `mktemp` scratch lies outside
the supervisor's owned bytes.
`scripts/bounded_runner.py` (oxide#097) stays separate and unqualified.

### Bounded offline lock seeder

`scripts/offline_seed.py` (oxide#131) is the one seeder for proof fixtures that
need a private, offline `CARGO_HOME` without a download. It takes one
`Cargo.lock` and the host Cargo home and copies only the lock's inputs, so a
proof never copies the host registry whole.

- **Selection.** One supported crates.io sparse namespace
  (`index.crates.io-1949cf8c6b5b557f`), matched on index and cache: the
  namespace `config.json`, one sparse-index `.cache` entry per locked registry
  package and one `.crate` archive per package whose bytes must equal the lock
  checksum. Workspace and path packages are skipped. Pinned Git sources (exact
  URL, name, version and full 40-hex commit) seed their bare database
  candidates, one database per locked repository: the locked Longhorn source
  and, since oxide#219, the Barge library. Packages from one repository must
  name one revision, and more than four repositories refuse before any copy.
  Another registry source, a duplicate or malformed lock entry, or an unsafe
  name, version, checksum or revision refuses.
- **Anchored reads.** One opened host root; every source component is opened
  descriptor-relative with `O_NOFOLLOW` on the root's device, so a symlink,
  special file, foreign device or missing component refuses. Each copied file
  is checked before and after its bytes are read (identity, size, mtime and
  ctime) and its name is reopened afterwards, so a changed source refuses.
- **Git without host Git.** No Git ever runs on a host repository, so its
  local config, includes, hooks and alternates are never read. Candidate
  selection only measures regular-file bytes through the same anchored walk;
  it establishes no commit. Every candidate that fits the remaining aggregate
  is copied first (each candidate root's host `config` and `hooks` skipped even
  when they are symlinks, a known private config generated,
  `objects/info/alternates` refused), and only then is the locked
  commit read inside the sanitized private copy with system and global config
  disabled, hooks and transports disabled and lazy fetch off. A copied
  candidate that does not carry the commit is rejected and stays charged; an
  oversized candidate is not attempted, its entries stay charged, and its
  record says so. The seed refuses when no attempted candidate resolves the
  commit.
- **Bounds, charged across the whole seed.** 256 MiB of copied bytes checked
  before each write; at most 8 Git candidates per locked repository and at most four
  repositories; at most 100,000 enumerated
  entries and depth 64; at most 1 MiB for one manifest read and 8 MiB of
  manifest reads; at most 64 KiB of Git output and 10 s for one Git command;
  at most 45 s of seed work and never past the caller's exported absolute
  deadline. No byte, entry or metadata charge is reset per attempt, copied
  bytes are never removed, there is no network, authentication, fetch, host
  cache write or unbounded directory copy.
- **Observed fit (2026-10-23, development check, not a native proof).** This
  repository's lock seeds 22 registry packages and three Longhorn bare-database
  candidates in 0.6 s: the required candidate copies 12.2 MB and resolves the
  locked commit; a second 14.4 MB candidate is also copied and resolved it on
  that run; a 2.0 GiB candidate is measured and not attempted, remaining
  inside the entry and byte bounds. Total copied bytes are 30.6 MB of the
  256 MiB seed bound, with 9,627 entries of 100,000; a candidate's host hooks
  and config are skipped and the private config is regenerated. The shared host
  cache is mutable, so candidate contents and outcomes can differ between
  runs; the receipt records each run's own per-candidate records.
- **Provenance (oxide#294).** `seed_cargo_home(..., provenance=dict)` fills
  an additive `oxide.seed-provenance/1` record in place: the explicit requested
  host CARGO_HOME and the private destination, the identity of each descriptor
  the seeder actually opened (source root, the two namespace directories,
  `config.json`, each database root and candidate), and on refusal the first
  failed component and its index, the attempted operation, the errno name when
  the syscall gave one, and a `kind` that is `unknown` unless the same
  operation's own stat obtained it. An errno is never read as a kind: a symlink
  under `O_NOFOLLOW|O_DIRECTORY` reports `ENOTDIR` on macOS, so the record says
  `kind: unknown`. Facts gathered before a refusal stay in the record, and a Git
  child that did not answer marks closure `unknown`. Candidate file lists stay
  in the manifest. Callers that pass no record see no change, and every
  no-follow, same-device, checksum, bound and host-read rule is unchanged. The
  module's `--self-test` now runs 21 synthetic cases, seven of them provenance
  controls.
- **Receipts.** A consumer proof records the seeder's SHA-256 in its own
  exact-head receipt together with its own script. Importing the seeder
  transfers no qualification: each proof's evidence stays bound to the exact
  sources it ran.
- **Synthetic cases.** The module's `--self-test ROOT` (also run first by the
  wrapper-bypass native) creates tiny fixtures only under ROOT and covers a
  registry+Git positive, a copied-but-rejected candidate, a measured and
  unattempted oversized candidate, symlinked host config and hooks skipped at a
  candidate's root, a wrong commit, a missing candidate, a malformed lock,
  symlinked inputs and the byte, candidate and entry cap negatives.
- **Consumers.** `test-accounting-vanished.py` (registry), `test-max-active.py`
  (registry and pinned Git), `test-storage-layout.py` (registry and pinned Git),
  `test-oxide-cli.py`'s `--clean-safety` (a one-package cfg-if 1.0.0 lock with
  its known checksum) and `test-pool-describe.py` (the bounded reference) seed
  through it. Each consumer keeps its own lower caller budgets; the seeder
  respects them and never raises another selector's bound, and their recorded
  native evidence predates the migration and stays historical.
- **Inventory of the other copied crates.io paths.** `scripts/build_context_comparison.py`
  is a configured live copier, not a seeder consumer and not migrated here: its
  `--live` path derives a registry input plan from committed Nucleus #038
  metadata (not this repository's `Cargo.lock`), copies the plan's declared
  archives and index files from the hardcoded `/Users/tom/.cargo` roots into an
  attached scratch image mount with a plain read/write file copy, and is bounded
  by its own 4,096-file and 512 MiB caps and its absolute deadline. No selector
  or test executes that path: `effigy build-context:comparison:test` runs
  synthetic fixtures only, and `--live` needs an admitted resource plan. It
  verifies digests through its staged-plan checks rather than at the copy, does
  not use anchored no-follow reads, and is retired-image containment tooling.
  A future live run must move onto this seeder, or a new scoped record must
  supersede it; oxide#131 makes no live-execution or source-trust claim for it.
  Other proofs that stage with the host Cargo home offline and copy nothing are
  not seeders either.

### Rollback

Do not rehearse these steps; they remove the dogfood artifacts.

1. Stop starting Oxide work: revert the adoption commit, which restores the
   plain-Cargo selectors and removes the marked `AGENTS.md` block, or run
   `cargo` by hand.
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
| [Storage inventory](storage-inventory.md) | Backs `oxide status --measure`, stray detection and admission accounting. |
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
- **Profiles.** `quick` (default) and `debug` set their `dev` recipe in the
  environment and remove Oxide's `--profile` from Cargo's arguments; `release`
  and `--release` pass `--release`. An explicit quick or debug `bench` passes
  `--profile dev`. The deprecated `oxide-quick` / `oxide-debug` definitions
  are always in the environment too. External subcommands receive arguments
  untouched and the quick recipe.
- **Refusals.** `--target-dir`, `--config` keys `build.build-dir` /
  `build.target-dir` (any quoting or `build = {…}` form) and every `--config`
  file path refuse, because a file could set either key.
- **Identity and key.** Git root commits plus the workspace prefix; shallow
  clones use the normalised `origin`; otherwise the canonical path. The slot
  key adds the Oxide recipe key, the `rustc -vV` digest, the target (`--target`,
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
  The host-wide active-build limit counts commands holding slots across both
  pools. `OXIDE_MAX_ACTIVE` overrides `[pool] max_active`; both accept integers
  from 1 to 32. With neither set, macOS uses
  `min(4, max(2, floor(physical_cores / 4)))` from the native
  `hw.physicalcpu` query. A missing, failed or implausible result, and every
  other supported platform, uses 2. The automatic formula is approved policy,
  not measured optimal capacity. Excess commands wait in FIFO order, print one
  `oxide: waiting for …` line on stderr and cancel on Ctrl-C with exit 130.
  Another command in the same checkout
  and key takes a compatible spare slot or creates one within budget; it waits
  when no safe slot or host capacity is available.
  Monotonic wait totals continue across freshness retries. Reason changes are
  reported up to eight times, with one later-change notice; queue-position
  changes alone do not print another line. The totals use the last observed
  exclusive reason as an approximate attribution and exclude freshness-gate,
  Cargo-lock, Cargo and execution time.
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
  or `build+execute`), slot, warm/cold, elapsed time, the slot's size when
  leased, this command's soft budget and the backend:
  `slot 5 was 12 MiB when leased, 95367 MiB budget, pool total: see oxide status (directory-budget-v1, budget-only)`.
  The size is the library's reuse walk of that exact slot generation; a new
  slot says `new slot` and a held replay `slot size unknown`. The line walks
  nothing, so the pool total is in `oxide status`
  ([ledger-only view](local-lease.md#ledger-only-view-for-front-door-membership-oxide110-lead-e4c74399)).
  When the gate refreshed files, or reconciled another checkout's build, it
  appends the count and the outside-Git limit. A refused gate prints its
  reason on its own line before the cold build. A command that observed a wait
  also prints a completion line before spawn and appends `waited N.Ns` to the
  summary; the wait is measured monotonically across reacquisitions, while the
  existing elapsed value retains its current whole-invocation meaning. If the
  monotonic measurement becomes unavailable, the diagnostic says so rather
  than inventing a duration.
- **`clean`.** Waits until no live or interrupted-but-running command holds a
  slot for this checkout, then blocks new commands in the checkout while it
  runs `cargo clean` against the private target directory and an empty
  private build directory. It then drops the checkout's idle affinity. Slots
  stay intact until evicted, so the next build is cold. Ctrl-C cancels the
  wait. A blocked clean receives a monotonic wait-completion line when its
  barrier is acquired; whole-target clean summaries include that measured wait
  when they print a summary. Selected clean has no general summary line.
  `cargo clean` runs as a recorded process group and inherits the clean
  lock, so if `oxide clean` dies the barrier stays until that lock is free and
  the group is empty.
- **Crash recovery.** Per [Crash recovery](#crash-recovery), for Oxide's own
  commands only. A follower in the same checkout waits while the crashed
  command's group survives, then recovers. Since oxide#107 the follower first
  takes another compatible slot or a new one when capacity allows, and the
  held slot is recovered by a later command once its group is empty. If the crashed command's Cargo had
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
  recorded lease-ticket waiters (up to the existing 128-ticket bound) with
  their recorded PID and approximate age from owner-private lockfile metadata
  (creation time when available, modification time otherwise); unknown ages
  stay unknown. The lookup reads metadata without opening or mutating the
  ticket; a missing, symlinked, foreign, unreadable or future timestamp stays
  unknown. Wall-clock jumps cannot all be detected, and age never affects
  admission or recovery. These records may be stale and do not establish that
  a PID is alive. Only lease-ticket waiters are listed: commands waiting at
  `oxide clean` have no ticket before
  acquiring the barrier, so they are not listed. The age is approximate
  wall-clock file age, not the command's measured monotonic wait. Also shows
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
- **Finder metadata is not tolerated here (oxide#155).** Clean removes exactly its enumerated plan, so content that appears after the plan is left, listed and counted, and the command fails. That is a materially different authority boundary from the descriptor-relative whole-tree deletion the bounded race applies to, and the tolerance is deliberately not extended to it: a clean never deletes an entry it did not plan.
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
- **`status`.** Default `oxide status` prints the `cargo home:` path and
  says it is not measured; it walks nothing (oxide#128). `oxide status
  --measure` adds the allocation, counted once per inode, with file names and
  distinct inodes, from one bounded walk (200,000 entries, depth 64, 10 s,
  cut to what is left of the one shared measurement budget; `partial` past a
  limit). The root must be a real directory; symlinks are not followed and
  mounts are not crossed. A second line states Cargo's skip conditions as
  facts. No last-cleanup time is shown: Cargo's
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
  - a workspace not listed by the Oxide block that guides it (the Git root's
    covered list inside Git, else its own `AGENTS.md` outside Git), or a
    nested legacy block alone;
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
  `CARGO_HOME` is seeded by the [shared seeder](#bounded-offline-lock-seeder)
  from a one-package lock for `cfg-if` 1.0.0 (checksum checked, nothing
  fetched); there each
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
- **Lifecycle observations (oxide#304).** The bounded test packet retains
  CLI stdout/stderr, outer uptime timestamps and temporary-directory entries
  before assertions. Under the existing proof guard, a debug-only timing log
  records CLI entry, measurement/spawn/cleanup and ancestry/report phases.
  Helper/probe environments, product deadlines and cleanup assertions stay
  unchanged. `rust:test-launch-check` is the separately authorized single-test
  investigation under the qualified 120 s / 512 MiB guard, not an integrated
  deletion diagnostic or permission to retry a failed full suite.
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
- **Accounting walks (oxide#108).** Admission, reuse, eviction and
  `status --measure` walk the slots with one set of pool bounds; default
  `oxide status` walks nothing (oxide#128): 2,000,000 entries, depth 64
  (66 for the whole-payload walk, which starts at the payload root and stays
  two levels above the slot content directories; see
  [Finder metadata](#finder-metadata-in-pools)),
  60 s (previously 50,000, 24 and 5 s, which refused Praxis's 76,669-entry
  and Farmyard's 60,657-entry pools). Reusing an idle slot needs that slot's
  own walk complete; a new slot needs the whole-payload walk complete before
  and after eviction. A walk reads each enumerated file or directory from its
  live directory entry when the path name is shorter than the platform
  `PATH_MAX` (and by path name when it is not), and samples each read
  directory's path-name identity before and after its entries
  ([per-entry reads and boundary samples](storage-inventory.md#per-entry-reads-and-directory-boundary-samples-oxide141),
  oxide#141). That correction changes no bound, walk count, policy or refusal
  class, and it is not a measured speed claim or a guarantee that a large or
  cold pool meets the deadline. An incomplete walk refuses with
  `unknown_accounting`
  and names its cause (`entry_limit`, `depth_limit`, `elapsed_deadline`,
  `unreadable_entry`, `missing_metadata`, a mount boundary or a root
  failure), entries visited, elapsed time and bounds; its bytes are never
  used and never make a slot evictable. The one exception is an admission's
  walk of an idle slot with no current record on the internal directory pool:
  that slot is charged conservatively (the maximum of the bytes observed so
  far, its last record and the largest recorded slot, never zero and never the
  whole budget) and kept — never evicted (a named count replacement and the
  pool-wide slot ceiling included: each refuses, naming the slot, rather than
  delete unmeasured storage or exceed the caller's slot-count decision), reused
  or adopted on that walk, and no record — while only an admission whose pool
  capacity genuinely cannot fit refuses, with a bounded reason naming it.
  Status reports it as `not measured (walk incomplete)` from the recorded
  incomplete-walk fact, never as a figure (oxide#186). Every other
  incomplete walk, including a held slot's, the payload scope pass and a
  removable pool's, still refuses. The bounds are finite: a larger pool
  refuses until idle slots are evicted or cleaned. A `NotFound` bound to a
  non-root path the same walk enumerated at a deferred `read_dir`, `file_type`
  or no-follow metadata call is reported separately as `vanished xN`, not as
  a gap. Its complete result can still authorize the existing whole-idle-slot
  policy. This is a best-effort, non-atomic path observation and does not
  establish that inode allocation was freed or reclaimable. Root `NotFound`
  remains `root_missing`, including after the root was opened; iterator errors
  without a path identity and other errors keep their conservative categories.
  Status and refusal diagnostics include the separate count. Owning detail:
  [local lease](local-lease.md#capacity-and-eviction). Rust tests cover
  each old bound, injected `PermissionDenied` at inventory `read_dir` and
  slot-root `symlink_metadata` boundaries (not kernel permission enforcement),
  a symlinked slot root and hard links with injected bounds. The native
  selector `effigy rust:pool-scale` has no
  passing run: its first session staged the reference binary twice and is
  apparatus evidence only. The selector now uses the qualified shared
  supervisor; no later passing run is recorded.
- **Precedence.** Environment, then the user config file, then defaults.
  `OXIDE_POOL` sets the path (relative resolves against the current
  directory); `OXIDE_POOL_CAP` sets the budget; `OXIDE_MAX_ACTIVE` sets the
  host-wide active-build limit. Sizes are whole bytes or a
  whole number of `KB`, `MB`, `GB`, `TB` (decimal) or `KiB`, `MiB`, `GiB`,
  `TiB`, from 1 MiB to 16 TiB; anything else refuses before any effect.
  `OXIDE_POOL_BACKEND` is retired: unset or `directory-budget` only.
  An absent pool named by `OXIDE_POOL` refuses instead of being created
  silently (oxide#182). Creation at that explicit path is opt-in through
  `OXIDE_POOL_CREATE=1` or a path strictly inside the caller's canonical
  `OXIDE_TASK_SCRATCH_ROOT`; a value pasted from `oxide status` with its
  trailing ` (from …)` annotation refuses with a hint for every pool state
  (an absent path and an existing directory that holds no Oxide pool alike),
  before any generic open error. The config-file path and the built-in
  default keep creating the pool on first use.
- **Storage roots.** The config, cache and state roots come from
  `longhorn-storage-layout` (git tag `v0.4.1`, commit `5caf1e2c`,
  `platform-native-v1`, leaf `oxide`; oxide#103). Each root is resolved
  only when needed and reads only its own platform fact, so an explicit
  `OXIDE_CONFIG` or `OXIDE_POOL` reads no unused directory variable; no
  directory is created by resolution. Paths are unchanged: macOS
  `~/Library/Application Support/oxide/config`, `~/Library/Caches/oxide`
  and `~/Library/Application Support/oxide/state`, with `XDG_*` ignored;
  Linux `$XDG_CONFIG_HOME/oxide`, `$XDG_CACHE_HOME/oxide` and
  `$XDG_STATE_HOME/oxide`, defaulting to `~/.config`, `~/.cache` and
  `~/.local/state`. A relative or empty `XDG_*` value for a demanded root is
  ignored and the default used, per the XDG spec (Tom, 2026-10-21; it
  formerly refused). An absolute `XDG_*` value supplies its root without
  `HOME`; `XDG_RUNTIME_DIR` is never needed. A demanded root with no usable
  fact (no absolute `HOME` and no absolute `XDG_*`) refuses with `HOME is
  unset or not an absolute path`. Any host other than macOS or Linux now
  refuses a demanded root (formerly XDG); Longhorn's Windows policy is not
  adopted.
  Oxide keeps its own file names beneath the roots and its own TOML parsing
  and overrides; it uses no Longhorn config store or layout digest.
- **The home is the account's, not `$HOME`.** Every home-derived input
  above (the storage roots, `~` in config paths and the ignored macOS legacy
  config) uses the account's passwd home directory
  (`getpwuid_r(geteuid())`), not the `HOME` variable. A test harness or
  wrapper that overrides `HOME` therefore still gets the machine's config,
  pool and Builds-drive registration; it cannot fall into a fresh private pool
  by accident. Explicit settings keep their precedence: `OXIDE_POOL` selects
  the pool, `OXIDE_CONFIG` the config file and absolute `XDG_*` values (Linux)
  their roots; each replaces only what it names, so none alone is complete
  isolation. A failed lookup, or an empty
  or relative passwd home, refuses as a missing home did; Oxide never falls
  back to `$HOME`; the lookup runs once per invocation and its failure
  surfaces only where a home is needed. `status` always prints a `home:` line
  naming the home and its source (passwd entry, proof control or
  unavailable) and, when it differs, `HOME`. When `HOME` differs from the
  account home and the pool is the built-in default under that home, a leased
  command or `status` prints one stderr notice, `HOME is …; using the account
  pool at … (account home …); set OXIDE_POOL to isolate`. An explicit
  `OXIDE_POOL`, a config-file pool path or an honoured XDG cache root chose
  the pool, so none prints it. Paths in both lines are escaped and bounded.
  Cargo's own `CARGO_HOME` default still follows `$HOME`. Explicit pool and
  config settings choose the pool and config file; `status` still reads the
  account's state root (known pools, the legacy default pool and previous
  registrations), read-only.
- **Proof controls for the home.** Debug builds alone compile two controls
  (Tom's ruling, not a Cargo feature): `OXIDE_TEST_HOME_FROM_ENV`, which must
  be exactly `1`, restores `$HOME` as the home input (every proof script that
  sets a private `HOME` sets it), and `OXIDE_TEST_ACCOUNT_HOME` stands in a
  controlled account home or, as `!unavailable`, an injected lookup failure.
  The account control works only inside the qualified supervisor's own
  layout: a canonical `OXIDE_TASK_SCRATCH_ROOT` at `<oxide-proof.*>/scratch`,
  `OXIDE_PROOF_EVIDENCE` at its sibling `evidence` and an
  `OXIDE_PROOF_DEADLINE_NS` that parses as a `u64`; the stand-in must be a
  canonical, real directory strictly inside that scratch. An adopted scratch
  root does not qualify. A malformed value, both controls together, or either
  control present with any value in a build without them (release and
  installed builds; Tom's ruling) refuses before any configuration is read,
  explicit `OXIDE_POOL` and `OXIDE_CONFIG` included. The build-capabilities
  probe reports `test_controls`; the installer refuses `true` or a missing
  answer. Each proof stager that relies on the controls checks
  `test_controls: true` before any case runs (`test-account-home.py`,
  `test-everyday-cli.py`, `test-oxide-cli.py`'s debug stager and
  `test-installed-handoff.py`'s, which `test-outputs-prune.py` reuses); an
  artifact without them from any other stager refuses its private-`HOME`
  control rather than reach the account. `scripts/test-oxide-cli.py` gives its
  runner and `clean-safety` a private `HOME` with the first control, removes
  inherited `OXIDE_CONFIG`, `OXIDE_POOL`, `OXIDE_TEST_*` and `XDG_*`, keeps
  the invoking rustup home (`RUSTUP_HOME`, else the invoking `HOME`'s
  `.rustup`) and sets `RUSTUP_AUTO_INSTALL=0`.
  **Account-home proof.** `effigy rust:account-home` (oxide#143) is one fixed
  script under the qualified shared supervisor (300 s, 512 MiB monitored
  scratch; `--plan` prints phases, budgets and limits with no effect). It
  stages a controlled debug artifact and a normal release artifact
  (opt-level 1) in sequence with a fixed eight Cargo jobs, one Cargo call at
  a time with fixtures included (planning demand 8 CPUs and 6 GiB,
  unmeasured; `RUSTFLAGS` fixed and `CARGO_ENCODED_RUSTFLAGS` removed), each
  target deleted after its copy; the
  installer's capability check refuses the first and accepts the second, and
  the normal artifact refuses each control before configuration. Through the
  account control it proves the account default under a differing `HOME`
  (config, default pool, `home:` line, one notice, a tiny build, `pools.json`
  in the account state root, the harness `HOME` left empty), one owned
  128 MiB scratch image with owners enabled registered under the
  account-selected pool and selected by builds under a differing and a
  matching `HOME`, `OXIDE_POOL` isolation, ordinary detach, a planted account
  config choosing pool and budget without a notice, lazy needed facts with an
  unavailable account home, and the malformed, mixed, unsupervised,
  symlinked, outside and relative refusals. The image backing and its
  custody record live under the never-cleaned evidence directory, written
  before each effect; the backing is removed only after a positive ordinary
  detach, and unknown attach answers or writers, a failed detach or too
  little time keep it. Cleanup after a fault is bounded by the supervisor's
  stop point, not the work margin. It writes a uniquely named helper receipt
  with artifacts recorded as staged and resources known at failure; it never
  claims closure. `--plan` allocations are planning figures, unmeasured.
  `dogfood:scripts:test` covers the custody decisions with a fake `hdiutil`.
  One native run passed at `05fd297` (2026-10-24: 41/41 cases in 56 s,
  66.9 MiB peak owned, supervisor closed with no fault), accepted by Tom
  in place of the separate author and reviewer runs; a further run needs
  its own owner activation.
  **Proof.** Unit tests pin literal former paths for both platforms
  (Unicode and spaces), demanded and unused relative XDG, overrides without
  `HOME` and missing facts with injected facts; they do not prove native
  Linux behaviour. `effigy rust:storage-layout` runs one fixed script under
  the qualified shared supervisor: it records the normal dependency tree
  (exactly one `sha2`), stages `oxide` offline with 1.97.1 and runs `oxide
  status` on a private macOS `HOME` for the default paths, ignored XDG,
  overrides without `HOME`, refusal without `HOME`, and legacy-pool
  registration and known-pools routing through the cache and state roots,
  with nothing created. 300 s, 512 MiB monitored scratch.
- **Digests.** `sha2` is pinned `=0.11.0`, the one version in the
  dependency closure (Longhorn requires `^0.11.0`); `effigy
  rust:tree-duplicates` lists duplicate packages. Slot identity and
  compatibility digests are unchanged: unit tests check FIPS 180-2 vectors
  (one-shot, bytes, streaming), literal stored slot keys and a frozen
  ledger that reads back and serializes to the same bytes.
- **Config file.** Machine configuration, never a project file, at
  `config.toml` in the config root: macOS
  `~/Library/Application Support/oxide/config/config.toml`; elsewhere
  `$XDG_CONFIG_HOME/oxide/config.toml` (`~/.config` by default).
  `OXIDE_CONFIG` (absolute) selects another file; only the selected
  file is parsed. On macOS `~/.config/oxide/config.toml` is never read;
  `status` says it is ignored, without printing its contents. Parsed with
  `basic-toml` 0.1.10; at most 64 KiB:

  ```toml
  [pool]
  path = "~/Dev/oxide"   # ~ or ~/ expands to HOME; absolute, no . or ..
  budget = "100GB"       # or a byte count
  max_active = 3         # optional integer from 1 to 32
  ```

  An unknown key, a wrong type or a parse failure refuses with the file
  named. `max_active` is validated only when the config value is selected;
  therefore a valid `OXIDE_MAX_ACTIVE` overrides a range-invalid config value.
  TOML parse errors, wrong types and unknown fields still refuse even when an
  environment override is present, matching the existing config parser.
- **Automatic active-build limit.** With no environment or config value,
  macOS queries the native `hw.physicalcpu` sysctl once for the command and
  applies `min(4, max(2, floor(physical_cores / 4)))`. Counts outside 1–1024,
  a failed or malformed query, and non-macOS platforms fall back to 2; `status`
  reports the formula input or fallback reason. This is the approved default,
  not a throughput measurement. Existing active commands are left running
  when a lower limit is selected; later admissions wait under the same
  host-wide accounting and ownership rules.
  **Proof.** `effigy rust:max-active` runs one fixed script under the
  qualified shared supervisor. It stages `oxide` offline with 1.97.1, checks
  status provenance and precedence, then uses tiny `oxide exec` hold tools to
  show limit 2 waiting for a third command, config limit 3 admitting a third
  and waiting for a fourth, and a release admitting each waiter. The focused
  wait diagnostics extension checks a recorded ticket PID and approximate
  metadata age, the completion line before the tool marker, the summary wait
  suffix, absence of wait output for commands that did not wait, and a
  whole-target clean barrier completion and suffix. A synthetic unavailable
  removable registration pairs one active removable record with an internal
  command; the unit tests exercise both directions of the internal/removable
  count. No real volume or fixture compilation is used.
  The supervisor owns its 300 s deadline, 512 MiB monitored scratch and
  closure; the fixture stops work 30 s before its exported deadline.
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
  `OXIDE_CONFIG`, the pool path and budget with their sources, and the
  effective host-wide active-build limit with its source, formula input or
  fallback reason, then per
  pool: backend, soft budget, slot allocation, the pool filesystem's
  available and total space (shared, no Oxide quota or ceiling), Time Machine
  and Spotlight state, legacy and orphaned pools. `slot accounting walk:`
  reports the payload walk's coverage, causes, entries and milliseconds; a
  slot whose own walk is incomplete gets its own line saying it is not
  reused. An incomplete payload walk shows allocation as `at least`.
- **Explicit slot release.** `oxide pool recover-interrupted <slot>`
  replaces the retired CLI's `recover` for an interrupted slot automatic
  [crash recovery](#crash-recovery) keeps held. A slot quarantined in the
  current boot session refuses that command and is released separately with
  `oxide pool release-quarantined <slot> --confirm-writers-stopped`
  (oxide#196, [surprise removal](#implemented-part-2-surprise-removal-oxide098)).
  Source- and unit-reviewed; no native proof drives `recover-interrupted`.
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
  pool's own device, holding legacy `<id>` slots and
  [project-named](#project-named-slots-and-pool-status) `<folder>/<profile>-<id>` slots; config, ledger and lock sit beside it as for APFS, with
  no image or receipt. The config records the backend, store ID and budget
  recorded at creation (`cap_bytes`, default 100 GB; `image_bytes` 0). An
  image or receipt file, any payload entry but `slots` and
  [Finder metadata](#finder-metadata-in-pools), or a payload on another
  device refuses.
- **Admission.** The ledger's rules with this command's soft budget
  (`LeaseRequest.budget_bytes`, else the recorded one) as the cap, and the
  raised `MAX_SLOTS` 256 / 8 MiB ledger representation (oxide#177;
  [local lease](local-lease.md#slot-headroom-and-ranked-idle-replacement-oxide118)).
  A compatible idle slot is reused first and kept warm; the half-budget
  recycling rule is retired. A new slot replaces the least-recently-used
  eligible idle whole slot in two distinct cases: **count** pressure at the
  256-slot pool ceiling or the twelve-slot project ceiling, authorised on a
  complete pool figure or a removable pool's bound-only unknown; and
  **recorded byte** pressure, when the attributable pool sum — never the
  shared volume's total — is at or above the budget. A ledger whose slot
  table exceeds 128 carries the forward-only `capacity_policy: 4` fence; every
  mutating command's admission gate makes the version-8 `oxide.slots/256`
  control fence durable as its first write, before any locked pool open or
  ledger write, and a locked `Pool::open` raises the same fence itself under
  the pool lock before reconciliation, so pre-Store entries and direct library
  opens cannot journal first. An older binary meets version 8 and hands off or
  refuses before reading a policy-4 ledger or a multi-journal recovery ledger. It remeasures
  after each deletion and refuses `Full` when only held slots remain. A
  removable pool whose own size is unknown takes the reserve/floor route
  instead of a byte-cap comparison. Held, uncertain, foreign and unmeasurable
  slots are never eligible, a safety failure stops admission instead of
  skipping, and the audit walks behind `status --measure` are not admission
  input. Nothing enforces the budget while a build writes; a build can pass
  it, and the next new slot evicts.
  A deletion that cannot reconcile stays `Evicting` and is excluded from
  reuse and victim choice. Admission charges the maximum of its recorded size
  and the largest recorded slot: these pools have no per-slot byte ceiling and
  no descriptor-relative observed-byte inventory is available. A journaled
  root's recorded modes are restored through verified descriptors before its
  completeness preflight. If identity or completeness cannot be established,
  the root remains isolated and no deletion occurs. The soft-budget and replacement limitation is described in
  [local lease](local-lease.md#capacity-and-eviction). Independent incomplete
  scope remains an accounting error.
  Status names pending slot IDs, and the read-only drive description names them
  in its pending-reconciliation detail. Other independently owned bytes still
  count, and count ceilings still include the pending slot. The admission
  lock and the lease library's pool lock retain their existing blocking behavior
  for unbudgeted commands. Reconciliation runs under the pool lock, so a very
  large eviction can delay another admission until the lock is released.
  Before writing this new ledger shape, the mutating front door publishes the
  version-8 control fence (recorded accounting, eviction accounting and the
  raised ceiling) as its first write; the library uses capacity-policy
  version 3 or, above 128 slots, version 4, so older readers refuse or hand
  off before interpreting an `Evicting` size or a raised slot table.
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
- **Slot headroom proof (oxide#118, oxide#177).** `effigy rust:slot-headroom`
  under the shared proof supervisor, with fixture pools under a private
  `OXIDE_SCRATCH_ROOT` inside the selected volume's monitored run root, stages
  `oxide` offline with 1.97.1
  and proves the permanent ledger/control fences, a legacy read
  without migration, rank exhaustion before any transition, a 256-slot pool
  whose 257th and 258th admissions replace the rank-LRU idle whole slot
  without churn, the twelve-slot project cap with a specific held-slot wait
  and one LRU idle replacement, compatible reuse first, no eviction below
  both ceilings, byte-pressure LRU with a held slot excluded, and a foreign
  payload-entry refusal. With every slot's record current it also checks
  oxide#177's accounting-cost gate: a new-slot admission at 256 walks no
  inventory, the evicted slot is never walked, a release walks only its own
  slot, and default `status` at 256 walks nothing; it also crosses the legacy
  128-slot ceiling natively and requires `capacity_policy: 4` plus the
  version-9 control fence carrying `oxide.slots/256` and `oxide.holding/1`, which the admission gate writes
  as the command's first durable effect and a locked `Pool::open` raises
  itself before reconciliation, before any journal start or ledger write, so
  an older binary meets version 9 and hands off or refuses before reading the
  ledger.
  Worst-valid ledger/control serialization — including the reachable
  one-journal `Evicting` state — round-trip and oversized-field refusals are
  unit tests. `effigy rust:slot-headroom:diagnostic` is the same fixed guard
  and limits with `--diagnostic` (uncounted, keep-going, a bounded
  `qualified: false` receipt, aborting on an unknown outcome or unclosed
  proof child), and `effigy proof:slot-headroom:test` is its pure
  `--self-check`. The thirteen-grant freshness bound is a bounded-loop
  inspection, not a separately induced native case. The earlier internal-root
  recipe passed 50/50 cases at `4b0a485` (#177); that evidence does not carry
  forward to the migrated recipe.
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

### Finder metadata in pools

oxide#105. Finder writes `.DS_Store` files, and AppleDouble `._<name>` sidecars where a filesystem lacks native extended attributes. Both can appear in a pool directory someone has browsed. Both backends tolerate exactly these two forms and nothing broader. The name is `.DS_Store`, or `._` followed by a nonempty name; the entry is a regular file (no-follow), owned by the caller and on the directory's device. The tolerated places are:

- **Pool root and the removable control root.** Allowed beside config, ledger, lock and `oxide-managed`. These bytes are control-directory bytes, like the ledger, and are not payload. A listed name whose lookup finds it already gone (a `ledger.tmp-*` renamed onto the ledger during open, or Finder metadata removed) is skipped, not refused (oxide#179); any other lookup error and any symlink still refuse.
- **Payload root** (`<pool>/payload`, or `<mount>/oxide-pool`). Allowed beside `slots`. Payload accounting now walks the payload root, not `slots` alone. The bytes therefore count in the payload figure and the soft-budget admission check, and the `(device, inode)` union counts a hard link once. The depth bound is one level deeper (two since #104 added project folders), so slot contents keep their previous allowance.
- **`slots`, a project folder, a slot and its build tree.** This is unchanged: these were never name-checked. A [project folder](#project-named-slots-and-pool-status) with no recorded slot is reused only when it holds nothing but Finder metadata. Every regular file there, metadata included, counts in the slot's own figure.

A metadata-named symlink, directory, FIFO or other special file is still refused. So are a file owned by another user, a file on another device, and a near-miss name (`._`, `.ds_store`, `.DS_Store.bak`). Every other unknown entry also refuses wherever it did before. A name never establishes a lease, never makes a slot reusable and never permits a cleanup. Oxide never deletes, moves or rewrites these files. A whole-slot eviction still removes everything inside the slot, as before. Pool creation still requires an empty directory.

**Bounded removal race (oxide#155).** A list-then-remove of a whole owned tree races Finder, which can recreate `.DS_Store`/`._*` in the window and make the removal fail `ENOTEMPTY`. Whole-slot eviction and `Evicting` reconciliation (`remove_tree_owned`) and the prune quarantine pass remove every directory through held, positively verified descriptors: the parent, the child's name in it and the verified child, with `fstatat` no-follow checks and `unlinkat`, and no path reopened or rebound for an effect. The name is revalidated against the held descriptor — same kind, device and inode — before the removal, again before the drain and again immediately before **every** entry's own unlink, and each entry's `(device, inode, type)` is re-read in the same step, so a replaced or rebound name is reported and is never removed or drained. **An enumeration that fails is never an empty directory.** `readdir` reports end-of-directory and failure with the same null return, so the seam clears `errno` before each call and reads it after a null one: a listing that stopped with an error refuses the round as an enumeration failure before any verification, capture or unlink, so no partial list is ever acted on, and a platform whose `errno` the crate cannot read is an unproven enumeration, never an empty directory. **Capture-then-verify (Oxide Supervisor mechanism, superseding the former one-entry residual).** Deletion never removes a name it did not capture and verify itself. For each entry the seam first observes it by no-follow `fstatat`, then captures it with one exclusive same-directory rename (`renameatx_np` + `RENAME_EXCL`, or `RENAME_NOREPLACE` where that is the platform's primitive) to an unguessable private name carrying 128 random bits under `.oxide-capture-`; any rename error stops the caller pending. It then verifies the **captured** object's device, inode and type by no-follow `fstatat` against that observation and rechecks the parent/name binding to the held directory, and only then removes — `unlinkat` on the private name for a file or link, and for a captured directory a verified inode followed by a descriptor-relative recursion and `AT_REMOVEDIR` on the private name. Tolerated Finder metadata additionally has to satisfy the existing #105 predicate — a regular file, on the directory's own device, owned by the caller — with no new size or name rule; a symlink or directory swapped in is never followed, never recursed into and never removed. On a mismatch nothing is unlinked: the seam tries an exclusive private→original put-back, and when a racer has already taken the original name it leaves the private name in place, reports both names and stops the caller pending. The guarantee is therefore: deletion removes only captured, verified objects, never follows a link and never recurses into an unverified directory; the residual is a same-user process discovering the private 128-bit name inside Oxide's own quarantine directory during the capture window. A refused directory is put back under its recorded name, so a pending slot or root is never left under a capture name the ledger cannot name.
**Journaled captures (oxide#155).** Every capture in the lease's whole-tree deletion is journaled, and a deletion is complete only when its journal says so. The root is planned first: the planned private name, its `(device, inode)`, the verified parent's `(device, inode)` and its full `st_mode` are durable before the exclusive rename, the captured phase after the rename and its parent-directory sync, and a `removed` phase only after the root's own removal completed. Each **nested** entry — including those the Finder-metadata drain recovers — gets a durable `captured` record with the private and observed names, the verified `(device, inode)` and full mode, and the chain of captured private names that locates its parent. Names are journaled **exactly**: the walk and the drain require a strict UTF-8 name and never convert lossily, and an entry whose name cannot round-trip is refused **before** it is captured — nothing is renamed or deleted, the escaped bytes are reported, and the operation stays pending (exit 3). A recorded name that cannot round-trip exactly at replay is likewise unknown and pending, never a rename to a different name; a `removed` record follows its own successful unlink. Live deletion requires that record; replay has the narrow regular-file absence rule described below. That record is durable **before any permission change**, including the owner-access repair widening of an unreadable owned directory, and every exit path — an error, a relayed `?` or a panic — restores the recorded mode through a descriptor whose identity was verified for that step, so a kept tree never stays widened. A name that no longer holds the captured object — a failed reopen, an unreadable identity or an identity mismatch, at the repair boundary or after it — is never chmodded **and never renamed**: the verified repair descriptor (when the repair already happened) restores the owner's mode, the unverified object stays exactly where the racer put it, the observed name stays free, and the entry's capture record stays outstanding so the deletion remains pending. Every earlier put-back happens only while the private name still holds exactly the captured object (its verified device, inode and type); anything else is left in place rather than renamed. A capture that does not verify against its own observation — the racer took the observed name before the exclusive rename, at the tree root or below it — is recorded as a foreign capture **before anything is renamed**, both names are kept, and the deletion stays pending: replay never retries past a foreign record and never clears the one that carries it. The repair guard stays armed until the reopened child's identity is established. The narrow residual is the window between an exclusive rename and its `captured` record: replay keeps such an unrecorded private name, reports it and refuses, and never removes it; a captured private name that vanishes during the live run is never success either. A capture that is not the observed object is recorded as a **foreign capture** before any put-back attempt; those records are never removed, their private names are never unlinked, and their presence keeps the deletion pending. A captured parent that was already put back is resolved through its **verified observed name** — when it has recorded child captures, replay gives that verified directory the same identity-bound access repair the still-private branch uses: **one descriptor**, opened without following a link and verified against the record, carries both the descent and the mode restoration — the recorded-mode restoration is **armed on that descriptor immediately after the widening chmod** — before the reopen, so a failed reopen still restores — and runs through that same descriptor on every exit path including a panic, never by reopening the name, and a name rebound after the widening stops the step pending with the original's mode restored and the replacement untouched and descends into its recorded children there, so a put-back parent never strands them; a restored parent with no recorded child is never opened — and every recorded capture must be counted exactly: the deletion completes only when all of them resolve, and otherwise stays pending until the obstruction is removed. Replay never unlinks anything; only the narrow regular-file rule below concludes two absent names. The **root is recorded by its plan, never as a nested capture**: a nested record's chain names only captured parents *below* the root, because the root's own private name is the base replay opens, not a name inside itself. Replay gives the captured root read access through its own verified search-only descriptor when the owner's recorded mode has none, puts every recorded capture back under its observed name deepest-first (so each put-back sees a live parent, and each captured directory's recorded mode is restored before its own put-back, and again through the same verified descriptor on **every** replay error path and panic, so a retained tree never keeps a widened mode), puts the root back with its recorded mode, and only then clears the journal and retries the deletion. It fails closed: a recorded capture the rollback cannot locate — including an inconsistent chain — keeps every record and the pending refusal instead of completing past it. A recorded root `removed` phase is the **only** completion proof — two absent names without it may be a move and stay pending — and no new settlement verb is added; a slot that cannot be resolved stays `Evicting` and is excluded and conservatively charged during admission. The prune caller keeps its own narrower journal (its intent's `intent` then `renamed` phases, reconciling the recorded root and quarantine leaf by identity), and its delete pass refuses a private capture name it holds no record for instead of unlinking it ([Pruning owned output roots](#pruning-owned-output-roots-oxide129)). The same-device requirement is the directory's own device, so a recreated entry on another filesystem is refused. An owned directory the owner left without read permission but with its own search bit is still deleted whole: the seam opens a search-only descriptor of that very entry (`O_SEARCH`, no-follow), revalidates its identity, restores only that entry's own `0o700` bits on the descriptor, then reopens and requires the same identity. Nothing repairs permissions through a path or a link. A directory with no search bit at all cannot be reached by any descriptor as this user, so it is **kept and the deletion refuses** rather than chmodding through a name that could be a link. A vanished name is the caller's decision, and each caller decides explicitly: a nested directory already gone inside a bounded pass is finished work, the ledger's slot removal is complete, while a journaled prune quarantine leaf that is missing without positive deletion proof keeps its intent pending. Release builds carry no injection hook at all. The macOS behaviour that shapes this: `rmdir` of a directory holding only AppleDouble `._*` sidecars succeeds (they are transparent), so the race that needs recovery is a `.DS_Store` (alone or beside `._*`), and that is what the proof races. `oxide clean` is deliberately unchanged: it removes exactly its enumerated plan, so a path that gained unplanned content is left and counted ([Clean](#clean)).

**Named refusals.** A refused pool-root or payload-root entry is reported with its place and an escaped name. The category stays `unknown_accounting`, with exit 3. For example: `cannot open the removable pool: unknown_accounting (the payload directory holds an unexpected entry "stray\x07entry")`. Printable ASCII appears as is. Each other byte, including `"`, `\`, controls, non-ASCII and non-UTF-8, appears as `\xNN`. At most 64 name bytes are printed, then `...`. Accounting that is incomplete for another reason, such as a depth, entry or time bound or an unreadable entry, reports the walk's cause and counts ([accounting walks](#budget-only-default-pool-and-user-configuration), oxide#108) but no entry path, because the walk does not record which entry caused it. Status remains read-only. Removable selection falls back with the named reason.

Sources: `.DS_Store` is the Finder store that Apple's `com.apple.desktopservices` `DSDontWriteNetworkStores` and `DSDontWriteUSBStores` preferences control (Apple Support, "Adjust SMB browsing behavior in macOS"). The AppleDouble `._*` form is from `dot_clean(1)`: "Merge ._* files with corresponding native files", and AppleDouble is documented in `copyfile(3)`.

**Proof.** `effigy rust:oxide-finder-metadata` (`scripts/test-oxide-finder-metadata.py`) is a short fixed script under Effigy heavy admission. It does not use the unqualified #097 runner. Before every phase it checks owned scratch bytes from `du`, which include the sparse image's allocation. The 512 MiB figure is a budget checked between phases, not a ceiling. Every child, staging included, starts only after its deadline (cut from one 300 s budget) and the 20 GiB reserve on `/private/tmp` have been checked. Each child runs in its own session. While it runs, host free space is sampled every 250 ms, and a breach, a failed sample or the deadline kills the whole session. Any exception after a child starts, including Ctrl-C and SIGTERM (which unwinds the same way), kills that child's session and reaps its leader before unwinding. It waits at most 10 s for this, and never past the original 300 s. A session not positively closed blocks detach and keeps the scratch. The reserve watchdog during children and the deadline-before-spawn order were added after independent review 6301259831 at `1f02fdf` found them missing. The exception-path closure was added after review 6301598979 at `fd1ad57`. The deadline bound on that wait, and the scratch removal running as a deadline-checked child, were added after review 6301733025 at `7114bb7`. The single author submission at `02a83a8` ran the earlier script, which checked the reserve only between phases; the corrected script has not run natively. It stages `oxide` offline with 1.97.1 and runs with wrappers disabled under a private `HOME`, `CARGO_HOME` and `OXIDE_POOL`. It never touches the installed binary, the default pool or a real drive. It checks two pools:

- **Internal budget pool.** Metadata at the pool root, payload root, `slots`, slot and build tree keeps status `Complete`, the payload figure rises by the payload-side bytes, and the next check is warm.
- **Removable pool on one owned 128 MiB scratch APFS image** at a unique `/Volumes/<name>`. The same files in the control root and on the volume keep it selected and warm, with the bytes counted. Then an unknown payload-root entry makes `status` and the next build fall back, naming the entry. Every metadata file and the unknown entry are still present afterwards.

Cleanup is an ordinary detach and removal of the scratch. Not a benchmark or real-drive qualification. The author's single run at `02a83a8` (2026-10-20) passed in 38 s of script time, after about 8 minutes waiting for Effigy admission. Staging took 15 s. The internal and removable payload figures each rose from 0 to 12 MiB with the metadata and stayed `Complete`, and both repeats were warm. The fallback named `"stray\x07entry"`. All 20 metadata files and the unknown entry were kept. Peak owned scratch at the phase checks was 105 MiB, and the lowest host free space was 586318 MiB.

**Nested file replay (oxide#175).** A recorded regular file whose private and
original names are both absent may be concluded only beneath the verified,
owner-only private captured root and a verified captured-directory chain. A
same-user process discovering that private name in the quarantine remains the
residual. Replay restores the recorded modes and durably drops the concluded
file record before restoring the root. Root and directory absence, symlink
absence, unreadable names and foreign captures retain the strict refusal. The
normal deletion records each unlink immediately, using its existing durable
ledger replacement; no additional per-entry sync is introduced.

**Bounded-removal race (oxide#155).** Unit coverage is deterministic and in-process: the deletion seam (`src/owned_delete.rs`) is tested with a debug-only arm that creates the entry at exactly the boundary — after the caller's listing, before the removal — for tolerated `.DS_Store`/`._*` (completes), a foreign file, a metadata-named symlink (refused, its target untouched), a metadata-named directory and more names than one pass (refused, nothing deleted), and recreation on every attempt (exhausts the bound). The pre-#155 parent is a control in the same test: a plain `rmdir` of exactly the raced state still fails `ENOTEMPTY`, and the seam completes it. Since oxide#190 the lease eviction no longer uses this seam at all: a slot tree is renamed whole into `<payload>/.oxide-holding` and deleted outside the pool lock, so the only caller left is the outputs-prune drain (and its per-entry journal replay for a legacy `Evicting` slot, which is covered where that replay lives). The prune caller's disposition is covered by `race_pending`, which maps every terminal race outcome to a pending intent and never to success, and its own delete pass is covered through the production traversal over a real fixture tree: an ordinary tree is deleted through verified captures; a regular entry replaced after its observation, a directory replaced by a link, and a directory replaced before the pass descends each keep the racer's own object — the surviving inode is the falsification against the former list-then-unlink pass — and stop the pass pending; tolerated metadata recreated inside a directory after its content was deleted and before its own removal is drained so the directory completes, while a foreign entry at that same boundary is refused with the directory put back under its name and the foreign file untouched; and an observed link, an unrecorded private capture name and an entry whose reading is another device each stop the pass with nothing deleted. A `readdir` failure injected after a tolerated metadata entry was collected and before a foreign entry is refused as an incomplete enumeration, with the metadata entry, the foreign entry and the directory untouched. The journaled root capture is covered deterministically through the real deletion: the plan is asserted to exist while the slot is still at its recorded name and the captured phase only after the exclusive rename, with the root's device, inode and full mode in the record; and the real eviction is ended at each stage a crash can leave — planned before the rename, renamed before its phase, mid-delete, and after the product widened the captured root's mode — after which the next pool open reconciles both names, restores the owner's exact recorded mode and finishes the deletion. A racer that takes the recorded name after the capture keeps both names, the capture record and the pending deletion (and the affected slot stays excluded); a recorded name holding a different object blocks the same way; and a capture record on a slot that is not `Evicting`, or in a shape no writer produces, is refused at read.

The #175 proof source also staged the immutable pre-change commit
`d5f5e4d5f3793d770a4241405dc3f33f200df7cc` and compared its actual binary with the
candidate on the same blocked fixture. Oxide Supervisor decision `6818efde`
retired that case (`lease_old_counterfactual`) with `lease_unlinked_file`: oxide#190
removed the per-entry capture journal and its `OXIDE_TEST_CAPTURE_UNLINK_STOP`
seam, so the pinned pre-#190 binary and the legacy journal replay are no longer
required here and are covered by the #190 units and `rust:holding-delete`. The
proof stages only this HEAD's binary now.

`effigy rust:finder-metadata-race` is one fixed script and the whole command of the qualified shared supervisor (`bounded-test.py --limit 420 --scratch-budget-mib 512 -- python3 scripts/test-finder-metadata-race.py`; heavy admission, 420 s, 512 MiB monitored scratch, the unchanged 30 s shutdown margin and the unchanged custody and ordinary-detach rules — Tom's decision `138a8155` raised only this proof's counted and diagnostic wall-clock limit from 300 s, and the earlier 300 s runs stay history). Every fixture pool is a genuine `directory-budget-v1` pool, so each case declares one private `OXIDE_SCRATCH_ROOT` inside its own run root and keeps its `OXIDE_POOL` strictly inside that root with an explicit `OXIDE_POOL_CAP` (oxide#173 scratch pools, S4 migration oxide#213): on the selected volume that is a scratch pool on removable media, and a `/private/tmp` fallback exercises the internal path where the declaration is inert, which is not scratch-pool coverage and is reported as such. The migration changed the script's source, so no earlier native receipt is carried forward: the migrated head needs its own uncounted diagnostic, counted author native and independent reviewer native. It stages this HEAD's debug `oxide` offline with plain Cargo and 1.97.1 (`RUSTFLAGS` frozen, `CARGO_ENCODED_RUSTFLAGS` removed, the invoking `RUSTUP_HOME` captured before the private `HOME`) and covers both callers, one case per run, each in its own private `HOME`, `CARGO_HOME`, `TMPDIR` and pool:

- **Lease (the post-oxide#190 holding model):** an eviction renames the idle slot tree whole into `<pool>/payload/.oxide-holding/<store>-<slot>-<incarnation>-<nonce>` and records one `holding` row, then deletes the tree outside the pool lock. `lease_control` is the premise control: the second admission evicts the first project's idle whole slot, the slot *leaf* `payload/slots/<folder>/<profile>-<n>` is gone, no holding row or tree survives, and `status` reports neither a held nor a pending deletion. `lease_metadata` plants real `.DS_Store`/`._*` files at the tree root and in a nested directory: they go with the held tree, never refuse the deletion, and leave no row, tree or held line. `lease_foreign` plants one ordinary foreign file, which the drain unlinks by name with the tree (`src/lease/pool.rs:5552`), so the deletion completes and nothing stays charged. `lease_link` plants a `.DS_Store`-named symbolic link to an owned sentinel directory *outside* the pool: the drain unlinks the link itself and never follows it (`unlinkat(..., 0)`, `src/lease/pool.rs:5557`), so the deletion completes and the sentinel and its content are byte- and inode-identical afterwards. `lease_exhaust` plants an entry the drain cannot classify (a FIFO): the drain refuses it by name (`src/lease/pool.rs:5566`), the tree stays under `.oxide-holding` with its row and recorded charge, the command reports that it stays recorded and charged (`src/bin/oxide/main.rs:1931`) and the admission that needed that space is refused with `pool cap cannot admit a new slot` rather than deciding the bytes were free, recorded `status` lists `slot N held for deletion` with its charge and `presence not probed` (`src/bin/oxide/main.rs:2454`) — its lock-free snapshot never probes presence (`src/lease/mod.rs`, `HoldingEntry::present`), so the fixture proves the tree and its unemptiable entry with its own `lstat` — and after the fixture removes its own FIFO the next command finishes the held deletion and drops both the tree and the row. Every lease assertion is read from the slot leaf, the payload's `.oxide-holding` area, the ledger's `holding` rows or recorded `status`; no lease case arms the Finder-race injection, because that seam is reached only from the outputs-prune drain (`src/owned_delete.rs:247`).
- **Prune:** `prune_metadata`, `prune_foreign`, `prune_exhaust`, `prune_missing_quarantine` and `prune_replaced_quarantine` each take one owned 128 MiB sparse image whose backing and custody record live inside the guard's evidence directory (written before the first effect; the image is created with `-volname <this run's unique name>`, because hdiutil's default volume name would mount at `/Volumes/untitled`, and an explicit `-mountpoint` is passed when that path already exists; the attach is `hdiutil -owners on`, its raw plist digest is recorded, exactly one mounted entity must carry this run's expected `/Volumes/<name>` — any other mount point is unknown custody — and its device entry must be a block device whose `st_rdev` equals the mount's `st_dev`). That name is the short `o155-<slug>-<4 hex>`, and the fixture asserts its worst-case per-command temporary path (`<mount>/oxide-pool/slots/<checkout label>/<profile>-<slot>/scratch`) against the product's 91-byte Unix-socket bound **before any effect**, refusing an overlong layout rather than mistaking the product's own bound refusal for a finding: a prune checkout on the image cannot keep an inherited same-volume `TMPDIR`, so it gets an owned per-command directory whose path the bound limits. `effigy rust:finder-metadata-race:test` is the pure check for that layout and for the migrated fixture environment — the declared scratch root, the pool strictly inside it, the explicit cap and the selectors' own recipe, all with no scratch, child, image or attachment — and it keeps the first owner-activated diagnostic's refused 92/102/103-byte layouts as its counterfactual. It also pins, effect-free, the current product values this proof's remaining oracles read (the 91-byte temporary-path bound, `slots`, the `.oxide-holding` area and the drain's by-name unlink and unknown-type refusal, the reported refusal/finish lines and the `held for deletion` status line with its `tree still present` presence, the capacity refusal, the pool-owned `outputs/registry.json` and `.prune-` leaf, the pre-completion crash point, the low-reserve firing line, the preview digest line, the `oxide-outputs` tree and the `oxide-managed` control directory), so a moved constant or reworded line fails a pure check instead of a native run. It then registers the drive as the permanent outputs drive, copies in a real private Git checkout whose ignore rule covers `target`, and **asserts the conversion** before any race: one owned root, a real `target` directory, and every member link the pool-owned registry (`<pool>/oxide-managed/outputs/registry.json`) names as selected for that root resolving into it. The oracles read that per-pool control document, not a guessed path under the fixture's `HOME`. `oxide outputs prune --dry-run` then produces the exact plan the real run uses, and each case asserts the journal: metadata completes with no quarantined leaf, no live intent and no row selecting the root; a foreign entry and recreation on every attempt exit 3 with the `.prune-<intent>` leaf kept, its registry intent in a non-terminal phase, `receipt_settled: false` and no terminal state; a pending intent whose leaf is then removed or replaced by an unknown writer exits 3 on replay and keeps the intent unfinished, with the replacement and its content untouched. Every recovered metadata unlink is followed by the caller's own bounded check — the delete pass's deadline, the registered volume's identity, and the recorded and host reserves — so a low reading or an expired deadline stops the drain at that entry instead of after a batch. The same check is the one the quarantine leaf's own shared drain runs between its unlinks, and it runs again immediately before the leaf is removed and immediately before the terminal journal write; a failure there keeps the intent pending with the caller's reason. Before detaching, that attachment is revalidated (the mount is a real directory on the recorded device; the device entry is that same block device). It is detached ordinarily (never forced), absence is accepted only from `ENOENT` — a failed metadata read is unknown, our device still mounted is unknown, and only a *different* volume at the same path is recorded as reuse — and only after the device entry is positively gone is the backing removed; any unknown custody, device, writer or deadline outcome retains the backing and its custody record and aborts.

The debug-only `OXIDE_TEST_FINDER_RACE` injection is absent from release builds, armed only under the supervisor's exported layout, and appends every firing to `finder-race-fired` in its evidence directory; each case reads only what its own run appended. `--plan` prints the case table, allocation, fixture path budgets and premise dispositions without any effect, `--self-check` is the pure kit check (fixture layout, the declared scratch pool, the retired-case guard and the current-main oracle pins), and an unknown argument refuses before any subprocess. Two further cases drive product faults through the same fixtures: `prune_drain_fault` (the product's low-reserve reading is keyed on **recovered Finder metadata** through the debug-only `OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_RECOVERED`, fires on the first check after one recovered unlink, and the oracle reads only the firing line that case appended — it must name the recovered mode and one recovered entry — so ordinary deletions cannot satisfy it; expect exit 3, leaf kept with its remaining metadata, intent non-terminal, `receipt_settled: false`) and `prune_completion_fault` (the terminal crash point fires after the leaf is removed and the registry terminal write lands but before the checkout receipt settles: exit 3 with `deleted` yet `receipt_settled: false` and no leaf, and the receipt-step replay then settles it exactly). `effigy rust:finder-metadata-race:diagnostic` is the same guard, root and limits with `--diagnostic`: an uncounted collection whose bounded `qualified: false` receipt also covers setup and staging failures and records the first fault plus a per-case reason beside reached, completed, failed, skipped and unexecuted cases. Only a *recoverable oracle* failure is followed by the next independent case; an unknown writer, device, custody, detach or deadline outcome (including any child timeout, any setup or staging `OSError`, and any other non-classified setup exception) aborts the collection immediately and retains every image backing with its custody record, and an unwritable receipt aborts explicitly instead of reporting a diagnostic. Unknown journal or receipt state — an absent, unreadable, incomplete or malformed pool-owned registry, or an intent count other than exactly one for the recorded quarantine — aborts rather than counting as an oracle failure. It never continues over uncertainty. `prune_drain_fault` arms only through the shared debug gate `proof_evidence` (`src/bin/oxide/prune.rs`), which since oxide#211 (decision `60d4914f`, `declaration_allowed`) accepts this fixture's declared `OXIDE_SCRATCH_ROOT` when it is a canonical strict child of the supervisor's run root; the earlier gate required `/private/tmp`, so the `24bd3e3` diagnostic's `prune_drain_fault` never armed, exited 0 with an empty stderr and has no credit. The nine HIGH premises are traced to executable source in the script docstring (private HOME and rustup, account-home control, environment isolation, automatic adoption, Cargo external argv n/a, owned images, compiler flags, temporary storage, macOS `mktemp`). **Run history, preserved.** The first owner-activated uncounted diagnostic (decision `8ddaa98d`, head `7572541`, evidence `155-diagnostic1-7572541-owner`) ran once under the qualified guard: guard closed, status 1, no deadline, no fault; reached 10, completed 7, failed 3, skipped 0, unexecuted 0; host reserve minimum 2208.2 GiB over 753 samples, owned bytes peak 67.2 MiB of 512 MiB, 191 s of the 300 s limit; all five owned images attached at their expected mounts and were ordinarily detached with backings removed. All five lease cases completed, and `prune_foreign` and `prune_exhaust` **completed with the first native per-caller prune journal evidence** (exit 3, `.prune-<intent>` kept, intent in `deleting`, `receipt_settled: false`, no terminal state). `prune_metadata`, `prune_missing_quarantine` and `prune_replaced_quarantine` failed on the fixture's own premise, not the product: their checkout's per-command temporary path was 92/102/103 bytes, over the product's 91-byte Unix-socket bound, so the fixture build refused after adoption. That is why the volume names are now the short `o155-<slug>-<4 hex>` (worst case 76 bytes) with a pre-effect budget refusal and the pure `rust:finder-metadata-race:test` check. The three prune cases were therefore not exercised at that head. At `f8325e6` the short volume names and the pre-effect budget assertion were added, and the owner then activated the chain again: a clean uncounted diagnostic (10/10 completed, no fault, receipt `89b4d30d…`, guard receipt `1901adbe…`, closed/status 0/deadline false/census certain 0, five images ordinarily detached, audit copy `155-diagnostic-f8325e6-owner`) followed by a **counted author native** (`effigy rust:finder-metadata-race`, receipt `ed99fa7a…`, `proof: 10 cases passed`, guard closed/status 0/no deadline, 167.3 s of 300 s, reserve minimum 2210.6 GiB, owned peak 64.3 MiB of 512 MiB, five images detached with backings removed). That is the source-fit and journal evidence at `f8325e6`. **This head (`f8325e6` plus the bounded correction below) has no native**: the owner activates a fresh diagnostic, counted author and independent reviewer native here, and the correction adds two fault cases (`prune_drain_fault`, `prune_completion_fault`) whose evidence therefore comes from that fresh chain. No independent reviewer native has run at any head yet. A later counted author native of the then-14-case script passed **14/14 cases at `95d4105`** (#175); that run's guard receipt holds its time, scratch and reserve figures, and it stays history at that script. The case set is now twelve: `lease_unlinked_file` and `lease_old_counterfactual` are retired (decision `6818efde`), and the four remaining lease outcomes were re-authored against the holding model, so the migrated head's fresh chain is what counts. `oxide clean` is unchanged by design and has no race case.

## Removable build volume

Approved design direction (Tom, 2026-10-20). The goal: while the laptop is docked, Oxide's build-data writes go to a dedicated external SSD, and undocking at any moment is safe, with work falling back to the internal SSD. Measured motivation: [internal SSD write baseline](../storage-efficiency.md#internal-ssd-write-baseline-2026-10-20).

Part 1 (registration, selection and flush on release) is implemented, with author fixture evidence and Tom's operator acceptance; the final independent native proof remains missing. Part 2 (surprise removal: lost-volume slots, whole-slot deletion after positive writer closure or quarantine, and next-command fallback) is implemented by oxide#098, below. Final outputs stay in `<checkout>/target`; output following the pool is not implemented and belongs to the separate output-link lead (`fe95800b`).

Tom approved an early host trial on 2026-10-20. His `Builds` volume (`A6AC3ED7-33EA-46B4-8082-7254CF291ED2`, `/Volumes/Builds`) was the original registered build volume. The whole-MiB parser recorded an admission cap of 715255 MiB (749999226880 bytes, rounded down from 750 GB) and a reserve of 95368 MiB (100000595968 bytes, rounded up to retain at least 100 GB free). These are a budget and start threshold, not an APFS quota. **Do not undock during builds until an installed `oxide` contains part 2** (a reviewed merge and the planner's reinstall). The current registration and control state live under the existing internal pool; a later internal-pool move must preserve them safely or require explicit re-registration. Installation identity and before/after observations are retained in the user's install receipts and release audit.

Tom approved a staged move to `Stash` on 2026-11-04, keeping both volume names. The current permanent registration uses `/Volumes/Stash` (UUID `8A660440-C919-45C6-A3F1-3365B9BD2006`) with the same cap and reserve. Its pool was created fresh; the idle Builds control and payload are retained, and live checkout output links were detached and adopted through the shipped commands. Copied ledgers and historical evidence do not establish identities on Stash. Existing recorded workspaces keep their original paths until their owners drain them.

### Implemented: part 1

- **Backend `apfs-volume-v1`.** Config, ledger, lock and Oxide's own state live in `<internal pool>/oxide-managed/removable/`; the registration in `<internal pool>/oxide-managed/removable.json`. Only slot payload is on the volume, in `<mount>/oxide-pool/slots/`. No disk image on the volume. Budget-only: Oxide's admission cap is an estimate, not an APFS quota, and the volume's capacity is shared with every other directory on it. Only the front door's `managed` library entry points open it; the standalone lease CLI refuses it like a directory pool. No reserve watchdog runs on the volume: its reserve is checked when a build starts. A removal stop runs instead (part 2).
- **Registration.** `oxide pool register-removable <mount-point> --reserve <size> --cap <size> [--mode ephemeral|permanent]` (macOS). `pool` is not a Cargo built-in. Both sizes are required, from 1 MiB to 16 TiB in the [size units](#budget-only-default-pool-and-user-configuration) (oxide#102 dropped the whole-MiB rule and added decimal units, so `750000000000` is accepted); zero, overflow and malformed values refuse. The mode defaults to `ephemeral`. Oxide never picks a reserve, cap or quota, and changes no APFS setting. Before any write it requires:
  - a path of exactly `/Volumes/<name>`: nested paths, `..`, hidden names, prefix look-alikes and custom mount parents refuse;
  - `/Volumes` passing the structural mount-parent rule, at registration and at every selection: the caller is not root; `/Volumes` is a real directory (not a symlink) whose path and opened descriptor agree; it is owned by root with no group or other write and no setuid, setgid or sticky bit; `acl_get_fd_np(ACL_TYPE_EXTENDED)` positively reports no extended ACL entry (`ENOENT`, or an ACL with no entry), any other result refuses; and it has no file flag except `UF_HIDDEN` (`sys/stat.h`, a GUI display hint the standard `/Volumes` carries). Every violation is reported. A denied combined `faccessat(W_OK)` is not used, since it does not prove that a specific right such as `add_subdirectory` is denied;
  - the name opened with `O_NOFOLLOW | O_DIRECTORY` (a symlink alias refuses), whose descriptor's `fstatfs` `f_mntonname` names exactly that path, whose `st_dev` differs from `/Volumes`'s, `f_fstypename` `apfs`, not `MNT_RDONLY`, not `MNT_ROOTFS`, and writable by the user;
  - the mounted root itself, through that volume descriptor, before any media write (oxide#130): a real directory owned by the calling user, with no group or other write, no setuid, setgid or sticky bit, positively no extended ACL entry, no file flag except `UF_HIDDEN`, and mount flags without `MNT_IGNORE_OWNERSHIP`. The check runs after the binding opens and again immediately before the payload directory is created. A violation refuses before registering and names the manual fix on the drive itself (`chmod go-w` as the volume's owner, or erase and reformat); Oxide never changes the drive's ownership or mode. A volume registered before this rule is not re-judged by it;
  - `diskutil info -plist` agreeing on the volume UUID (the kernel's `ATTR_VOL_UUID`), mount point and APFS, with a container other than the one holding `/System/Volumes/Data`;
  - `diskutil apfs list -plist <container>` listing the volume with no APFS role: a Backup (Time Machine) role, and every other role, refuses.
  It records mount point, volume UUID, container UUID, reserve, cap, the volume's capacity and APFS quota (`CapacityQuota`, none when 0) at registration. A second registration refuses, and so does a registration left under a previous pool's control root ([oxide#102](#budget-only-default-pool-and-user-configuration)). The payload directory is created empty through the volume's descriptor.
- **Drive mode (oxide#116).** `oxide pool set-mode <ephemeral|permanent>` changes an existing registration, even while its volume is absent. It reads and writes only the internal control directory and never creates a pool or registration. Register and mode changes share the internal admission lock and reread the validated registration while holding it. The update writes a private, exclusive no-follow temporary file, syncs it, atomically renames it, then syncs the control directory. A failure before rename leaves the prior registration bytes intact; a directory-sync failure after rename reports uncertain durability. Reads never migrate the file. Legacy version 1 registrations without a mode mean `ephemeral`; mode-bearing version 1, version 2 without a known mode, unknown fields and unsupported versions refuse. An explicit mode write publishes version 2 with a required mode. Older Oxide binaries refuse version 2 rather than dropping a permanent setting. Installation receipts do not restore a registration.
- **Selection, when a leased command starts.** With nothing registered, the internal pool is used exactly as before. Otherwise the removable pool is used only if, at that moment, the mount point opens as above with the registered UUID, `/Volumes` still passes the mount-parent rule, the volume is writable, `diskutil` again reports it in the registered container, outside the boot/data container and with no APFS role (unknown metadata refuses), and its available space is above the reserve; the library then re-verifies the volume and the payload directory (same device and inode by path and by descriptor) on every open. Either mode uses these same checks and prefers the removable pool only when they pass. Ephemeral mode keeps the existing fallback diagnostic. Permanent mode emits an early warning with the command's actual fallback reason and carries that same reason into its existing summary when one is produced. Selection creates nothing, so a stale `/Volumes/<name>` is never written to. A command already in progress keeps its original selection snapshot.
- **No stale-mount-point writes by Oxide.** Oxide creates slot directories with `mkdirat` relative to descriptors opened from the verified mount root, rechecking the binding (descriptor still the registered root, path still that root's device) before each creation and before removing a slot. Cargo itself writes through the slot path; while the volume is mounted that path is the volume. A mid-build removal is part 2.
- **Flush on release.** Before a removable slot becomes idle (normal release, a refused shared slot handed back, an untrusted slot released for eviction, and crash recovery), Oxide calls `fsync_volume_np(fd, SYNC_VOLUME_FULLSYNC | SYNC_VOLUME_WAIT)` on the verified mount-root descriptor, verifying the binding before and after. The summary line prints its cost; `status` lists recent flushes, including those made during crash recovery. The flush runs after positive writer closure and is a durability step, not a closure proof. A failed or unknown flush retires the slot: the record becomes `retired`, the lease stays held, the slot is never reused, recovered or deleted, and the build's own exit code is unchanged.
- **Pools and limits.** Each pool keeps its own slots, records, origins and warmth; a lease belongs to the pool whose ledger granted it. The host-wide build limit counts active commands in both pools, reading the other pool's atomically replaced state without its lock, so two commands starting in different pools at the same instant may briefly exceed it by one. A checkout's clean barrier spans both pools: a removable slot attempt holds the internal pool's admission lock (internal first, then removable) and waits for a live internal clean of its checkout, and `oxide clean` (which marks under that lock) waits for the checkout's active removable commands. `oxide clean` drops slot affinity in the internal pool only.
- **Status.** Default `status` prints the internal pool, the registration (volume and container UUIDs, configured mode, reserve, admission cap, capacity and quota at registration), the recorded selection with the mount point's presence, and the removable pool's recorded slots, flushes and retired slots from its internal ledger. It does not open the volume, sample its space or run `diskutil` (oxide#128). `oxide status --measure` verifies the volume exactly as a build would, prints the current available/total space and names the reason when the removable pool is not used. The configured mode and the selected pool are separate facts.
- **Selection cost.** With a registration, each leased command's selection runs three `diskutil` and three `plutil` calls; without one, none. `oxide status --measure` runs the same check once; default `status` never runs it.
- **Platforms.** Off macOS, registration refuses and selection always falls back.

Sources: `fstatfs(2)` / `sys/mount.h`; `getattrlist(2)` / `sys/attr.h` (`ATTR_VOL_UUID`); `faccessat(2)` with `AT_EACCESS` (volume writability only); `acl_get_fd_np(3)`, `acl_get_entry(3)`, `acl_free(3)` / `sys/acl.h`; `UF_HIDDEN` / `sys/stat.h`; `fsync_volume_np(3)` / `unistd.h` (macOS 10.8+: `SYNC_VOLUME_FULLSYNC` writes data and metadata to the disk hardware, `SYNC_VOLUME_WAIT` waits; returns a non-zero error code on failure); `diskutil(8)`. Observed on macOS 26.6: `fgetattrlist(ATTR_VOL_UUID)` on a non-root directory's descriptor answers the containing volume's UUID despite the man page's `EINVAL` rule, so the UUID is never treated as proof of a mount root.

**Proof.** `effigy rust:oxide-removable-pool` stages `oxide` offline with 1.97.1 and models the drive as one separately attached 128 MiB APFS sparse image in owned scratch, mounted by `hdiutil` at a unique `/Volumes/<name>`, over a budget-only internal directory pool. One fresh canonical directory inside the counted scratch is `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP` for the runner and every child from before staging. A session guardian (`scripts/bounded_runner.py`) owns the bounds from admission: a sticky first fault; host free space sampled every 250 ms against the 20 GiB reserve (a failed sample, dead sampler or sample older than 1 s faults); owned bytes walked at least every 2 s within 1 s (scratch, staging, targets, temp, logs, control and image backing, plus each positively identified mount by its used blocks; an attach in progress counts at the image's whole 128 MiB, a mount being detached at its last used bytes; on `ENOENT` the partial total is discarded and the walk restarts once inside the same second, and a second disappearance, any other error or a sample older than 4 s faults); both output streams drained and counted against 32 MiB as they arrive; work stops at 240 s and teardown must finish by 300 s. Every child goes through its one spawn path in its own session; a fault, deadline, exception, SIGTERM or SIGINT closes that session (TERM, then KILL, then a fresh observation), and a session not positively closed stays unclosed and blocks every detach. A child counts as closed only when its leader has exited and been reaped and a fresh observation finds its session and groups empty; a missing exit status, or a cleanup step that times out, is a failure that goes through closure. Every walk observation, including the final one and each mount observation, is checked against the same 1 s deadline before a total is published; a late walk faults and never refreshes the sample time. Every closure wait, process observation, drain and settlement uses only what is left of the original 300 s; when positive settlement cannot fit, the session stays unclosed and resources are kept. After a fault only cleanup runs: positive closure, then ordinary detach, never clearing the fault or starting experimental work. On success the scratch is removed by a bounded cleanup step while the reserve and deadline monitors stay live, with owned bytes explicitly held at the last complete total during removal. The 512 MiB figure is a monitored cap checked by complete samples, not an instantaneous ceiling: overshoot between samples is possible, and any sample over it fails the proof. Before staging, the same session runs a deterministic synthetic suite (`scripts/bounded_runner_cases.py`) under the real guardian: reserve breach, error, dead and stale sampler; storage excess, error, staleness, deadline and the `ENOENT` whole-walk retry; pipe pressure, output excess and an open pipe after exit; a fault and a cancellation during a running child; TERM-to-KILL escalation; a failed process observation settled by the outer guardian; stand-ins for an unresolved attachment, a failed ordinary detach and cleanup after the deadline; a late final `lstat`, `scandir`, mount `stat`, `ismount` and `statvfs`, and a second walk attempt past the deadline; a cleanup leader that closes its streams but stays alive; and deadline expiry during closure, drain, settlement and a cleanup step. Synthetic results are apparatus checks, not product acceptance. It covers: unregistered behaviour unchanged; the mount-parent rule accepting the real `/Volumes` (read only) and refusing an owned scratch directory first for its owner alone, then by name for one `chmod +a "<account name> allow add_subdirectory"` entry (debug builds expose `oxide pool debug-check-parent <dir>` for this); refusals (missing, zero and overflowing reserve, custom parent, nested path, absent volume, the `Macintosh HD` symlink alias, read-only attach, prefix look-alike, second registration); registration matching `diskutil`'s UUID with only `slots` on the volume; selection present (cold then warm, flush cost recorded), below its reserve and absent after an ordinary detach (no mount point recreated); an injected flush failure (debug builds only) retiring a held slot that the next build does not reuse; warmth kept across detach and re-attach; status. Unit tests cover the stale-directory and descriptor-swap cases with real kernel metadata, injected device change, Backup and other roles at registration and selection (container mismatch and the boot/data container too), and size parsing. The mount-root rule is unit-tested with real descriptor facts for an owned scratch directory (accepted at `0700`, refused after `0770`) and injected facts for every refusal reason, since a group-writable drive root or `MNT_IGNORE_OWNERSHIP` mount cannot be created unprivileged; `rust:everyday-cli` proves no mount case and touches no volume. A Backup-role volume, a mismatched device under the registered name and a stale `/Volumes/<name>` cannot be created unprivileged, so they are unit-tested only. 300 s, 512 MiB owned bytes including the mounted payload, ordinary detach only. Not a benchmark or real-drive qualification. The removable-pool runner and rust:oxide-removable-pool selector are unqualified test tooling; defects are fixed as found. Observed in the author's passing run at an earlier head (2026-10-20, before the review corrections): flushes of 8.9 ms and 8.8 ms after cold checks and 0.0 ms (display precision) after a no-op warm repeat, on a scratch sparse image backed by the internal SSD. These are not the real drive's flush cost.

### Implemented: part 2, surprise removal (oxide#098)

- **A build whose volume vanishes fails; Oxide stops its own group.** Cargo's and rustc's later writes into the slot fail (the mount point is gone and `/Volumes` is not writable by the user). A program whose executable lived on the volume may not fail: in the author's first native run a build script running from the slot spun at full CPU after a forced detach and never exited, so Cargo and Oxide waited indefinitely. While a removable command runs, Oxide therefore checks about once a second that the mount root still opens as the device it started on (the reserve watchdog's loop, with no space trigger). When it does not, Oxide sends TERM to its own Cargo (or exec tool) group, KILL after 2 s, and waits up to 10 s more for the group to be empty. The command ends with the build's failure (at least 1); nothing is replayed. If the group is not observed empty in time, the slot is quarantined and the command exits 1.
- **Lost-volume record.** Releasing the slot flushes the volume first (part 1). When that flush fails and the mount root no longer opens as a mount root, the slot is recorded lost: the record is retired with a `lost` entry (slot ID, time, closure), the lease stays held, and the slot is never reused. The command prints `oxide: removable volume gone; slot <n> is …` and, after its summary, that the next command uses the internal pool. A flush failure with the volume still present stays the part-1 retirement, never deleted.
- **Positive writer closure decides deletion.** The holding command has seen its own process group empty before releasing. Closure is still not positive, and the slot is quarantined, when the record belongs to another `oxide` process, the command was `oxide exec` (a tool may start processes outside its group), `OXIDE_ALLOW_RUSTC_WRAPPER=1` let Cargo run an inherited rustc wrapper (a wrapper server may outlive the build), or `RUSTC_WORKSPACE_WRAPPER` / `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER` was set. A workspace wrapper set only in a Cargo config file is not read here; a process that left Cargo's group (for example through `setsid`) is outside this check, as in crash recovery. Such an escaped daemon can still be writing when the lost slot is deleted; group closure is the cooperative boundary, not proof that every external helper has stopped. Device disappearance, elapsed time and PID absence are never taken as closure. **A restart is one more positive-closure fact (oxide#161):** every process ends, so a quarantined record whose holder started before the current boot session is closed. The boot session's start is read through an OS-layer function (`sysctl kern.boottime` on macOS; unknown, never inferred, off macOS) and compared with the record's holder start. An unreadable or zero boot time, a missing or zero holder start, or a start inside a fixed 60 s safety margin stays unknown, so the slot stays quarantined. The margin covers the early-boot clock set and millisecond truncation, and the kernel's boot time follows later clock adjustments with the clock, so a holder of the current session never falls behind it. This fact decides lost-slot handling only, and a record from the current boot keeps exactly its previous behaviour.
- **Re-attach.** The next leased command that selects the removable pool, under its admission lock and before admission: each lost slot whose closure was positive — the holding command saw its own group empty, or the holder predates this boot session — whose record lock is free and whose recorded group (for a current-session holder) is still empty is released and deleted whole through the volume-bound library, with the existing failure handling. It reports once: `oxide: deleted slot <n> held when the removable volume was lost (…)`, or for the restart fact `oxide: deleted slot <n> lost before this boot (holder started <s> s before boot; a restart ends every process)`. A group number recorded in an earlier boot session is not checked, because it may belong to an unrelated process now. A reused group or a live holder leaves it for a later command. A failed lost-slot deletion returns an error before admission. Its persisted retired record remains until whole-slot eviction succeeds: crash recovery skips that record, reuse excludes it, and the next admission retries its deletion before taking a lease. This prevents the temporary idle ledger state between recovery and eviction from becoming reusable through the Oxide command front door. A quarantined slot from the current boot session stays held with its bytes counted: never reused, recovered or deleted; `recover-interrupted` refuses it, and it is released explicitly with `oxide pool release-quarantined <slot> --confirm-writers-stopped` once its writers are stopped (below). Once a later boot session has begun it is closed and deleted as above. Idle slots, flushed at release, stay warm across the removal.
- **Explicit quarantined release (oxide#196).** `oxide pool release-quarantined <slot> [--dry-run] --confirm-writers-stopped` acts only on a slot whose record is retired, held and quarantined in the current boot session, in the selected removable pool. Anything else refuses and names its state: a live command's slot, a retired record that is not a lost slot, a lost slot whose writers are positively closed, a record a later boot session has closed (both deleted by the next selecting command), a ledger entry that is not held, a record that is not provably this slot's lease, and an absent tree (the volume is not attached, so there is nothing to check or delete). `recover-interrupted` keeps its own scope. The operator's flag is required: without it the command refuses before reading any pool and prints the checks it would make. Before any effect it makes one positive, bounded same-user descriptor census of the slot tree: no process of this account may hold a file open inside the tree, no process's working directory may be inside it, and the recorded holder pid must be positively gone (or name another account's process, which the recorded holder cannot be); a same-account live pid is unidentifiable from the record's userspace wall-clock start and refuses. The census follows the writer-custody rules: a descriptor listing the kernel answers `EPERM` for is re-observed by kernel identity with a short bounded backoff and settles only on a positive fact (a later listing, a gone pid, a changed start identity or a changed effective uid); a vnode descriptor whose path and vnode facts are both hidden (`EPERM`) is a counted and reported OS blind spot only when the process's own `csops(CS_OPS_STATUS)` reports `CS_PLATFORM_BINARY` and its kernel start identity is positively the same before and after that status read (Tom decision e528a712, the same rule Barge uses); every other hidden descriptor, and a hidden working directory, is UNKNOWN and refuses, and every readable descriptor of a blind-spot process is still checked (an in-slot one refuses); both-flavour `ENOENT` is a reported dead handle (a revoked vnode cannot do I/O) settled only by a second observation; a descriptor the platform reports closed is never a hold; and a process of another account is outside the cooperative check because the slot tree is this account's own `0700` directory. An unreadable process or descriptor, a reached process, descriptor, per-process descriptor or wall-clock bound, and a live or unidentifiable recorded holder are UNKNOWN and refuse. `--dry-run` prints the census verdict and changes nothing. The effect moves the **held** ledger row and its tree into the [holding area](local-lease.md#whole-tree-holding-deletion-oxide190) in one library transaction: one identity-verified rename under the pool lock plus one durable row write, then deletion outside it by the claiming command, so the space stays charged until the tree is gone. There is no release-to-idle step; a failed eviction leaves the slot held and its record quarantined (a recorded hold intent is resumed by the next open), so an error never changes custody to idle. It logs one line with the slot, its recorded bytes (or `size unknown`), the census summary and the operator flag, then the usual `oxide: deleted slot …` line. The command-level refusals — selection, census and a stale preflight token — change nothing: the record selection and census run read-only before the effect store is opened, and the effect takes the admission lock without fencing, re-reads the record under it, and only a matching token reaches the commit; the admission fence is written after it. The durable hold intent (the derived holding name and the verified root identity) is the commit point. Every refusal and every I/O failure before that intent — an invalid or changed slot, the census, a stale token, a refused held-row/tree validation, a pending reconciliation, a capacity/rank check — leaves the pool bytes and identity unchanged; the admission fence is written only after the commit, never before it. Writing the intent itself replaces the ledger before it syncs the parent directory, so a failure during that write is classified by re-reading the ledger: a visible intent, or an unreadable ledger, is conservatively post-commit (the intent may be durable) with the tree still at its path; only a write that left the old ledger in place is a byte-identical pre-commit refusal. The held eviction itself is a two-durable-step library transaction: the intent is durable before the rename, and the slot row moves to the holding row in one durable write after it. A post-commit I/O failure is not a no-change refusal: before the rename the slot stays held with its tree at its path (the next locked open clears the stale intent); between the rename and the row write the tree sits in the holding area under the recorded intent while the row stays held (the next locked open adopts it by that identity and writes the holding row); a rename whose identity could not be observed is reported as unverified, never as this slot's tree; a swapped source is contained under `.unverified-<name>` with an atomic no-replace rename (`RENAME_EXCL`) when it succeeds, or reported as left at the derived name when it fails, with a pre-existing containment entry preserved byte for byte, the object never reported as the slot tree, adopted or deleted as one and the verified tree never claimed moved; a pre-existing derived or containment name found before the rename is a pre-rename failure reported as "not moved" with the entry never attributed to this slot by name; the record stays quarantined until a retry or recovery concludes it. The command then exits nonzero with a truthful committed-but-unfinished state name; the slot is never idle, adopted or deleted outside the holding protocol, and its bytes stay conservatively charged. Whether post-commit failures fall under the no-change refusal gate is settled for this brief by `precommit_refusals_and_postcommit_recovery`: pre-commit is byte-identical, post-commit is crash-safe partial state. Injected intent-directory-sync, post-intent rename, post-rename observation, swapped-source containment, failed-containment, pre-existing containment-name and derived-name-collision failures exercise the class, the charge, the truthful failure state, the never-adopt/never-delete ownership rule and the recovery.
- **Fallback.** While the volume is absent, selection falls back to the internal pool exactly as in part 1; a lost slot is not an active command, so it does not count toward the host-wide build limit. Interrupted `run` and `test` commands are not replayed; the next `run` or `test` builds through the internal slot and executes the checkout's `target` output.
- **Final outputs.** `CARGO_TARGET_DIR` stays `<checkout>/target` whatever pool holds the intermediates, so binaries built while docked remain at the checkout after the drive is removed. No opt-in to follow the pool exists; selecting another pool never moves or deletes outputs.
- **Status.** With the volume present, a lost slot reads `retired and held, never reused (lost with the removable volume; deleted whole when the volume returns)` or `(quarantined: lost with the removable volume and …; kept with its bytes, never reused or deleted; release with `oxide pool release-quarantined <slot>` once its writers are stopped)`; a quarantined record whose holder predates the current boot session reads `(lost with the removable volume and …; its holder predates this boot session, so it is deleted whole when the volume returns)`. With the volume absent, the removable block lists each lost slot from the internal control state, read-only, with the same boot-session wording.
- **State.** The lost entry adds an optional `lost` field to the record and a `lost` flag to the flush log in the removable pool's internal `state.json`. An `oxide` binary built before part 2 refuses that state (unknown field) rather than reusing a lost slot.

**Proof.** `effigy rust:oxide-removable-removal` (heavy) runs `scripts/test-oxide-removable-removal.py` as the whole command of the qualified shared proof supervisor (`bounded-test.py --limit 300 --scratch-budget-mib 512`, oxide#109): writes and evidence stay under its scratch and evidence directories, work stops 30 s before its deadline and the supervisor decides closure and cleanup. The fixture's directory-budget pool is a declared scratch pool (scratch-pools S4, oxide#212): one private `OXIDE_SCRATCH_ROOT` inside the run root on the supervisor-selected volume, the pool strictly inside it with an explicit cap, so the selector passes no `--root-parent`; a `/private/tmp` fallback run is internal, where the declaration is inert, and is not scratch-pool coverage. The script reuses the #104 script's child runner and staging. It stages this clean HEAD's `oxide` offline with 1.97.1 under an empty `RUSTC_WRAPPER`, then, with a private `HOME`, `CARGO_HOME` and budget-only scratch pool, attaches and registers one owned 128 MiB scratch APFS image at a unique `/Volumes/<name>`. Every attach and re-attach uses `hdiutil -owners on`, and a bounded read-only `/sbin/mount` observation asserts that the mounted root carries no `noowners` flag (MNT_IGNORE_OWNERSHIP), because the product refuses an ignore-ownership mount root at registration; the pure self-check pins that one option list (with the observation refusing both an ignore-ownership mount and a listing without this image) and that `Volume.attach` uses it, so the main mode and the boot-closure mode cannot diverge (oxide#191). The mounted payload is counted through the image's backing file in the scratch. Surprise removal is `hdiutil detach -force` of that image's own whole-disk device, positively bound just before (image path owns device and mount, mount is the recorded volume UUID), in three predeclared cases: an idle flushed slot survives removal and re-attach warm with an internal fallback build in between; a build whose build script blocks in its slot (its executable on the volume) is ended by the removal stop and fails, a late file write and directory creation into its slot path (by the build script when it gets that far, and by the unprivileged proof script) fail, the slot is recorded lost, `status` lists it with the volume absent, internal fallback `build`, `run` and `test` use checkout-local outputs, and after re-attach the next build deletes the lost slot whole and keeps the idle one; the same with `OXIDE_ALLOW_RUSTC_WRAPPER=1` is quarantined, kept with its bytes after re-attach and not deleted. After each removal `/Volumes` is root-owned without group or other write, the mount point is gone (or root-owned and not writable), and an unprivileged create under it is refused without creating anything. A checkout binary built while docked runs after removal. The forced detach never targets anything but the recorded scratch image. This script no longer detaches after a launch: every attach publishes one post-census detach request (oxide#191, decision `ee263cea`, [protocol](#shared-proof-supervisor)) naming that image's backing, mount, device and volume UUID, and the qualified guard performs at most one ordinary detach of exactly that image after its own post-leader census, keeping the mount, backing and evidence whenever it cannot establish closure; an in-script detach happens only before any child launch and withdraws the request first. Result lines report that request, never a detach this script did not perform. An exact-head review found the earlier success-path detach ran before the guard's census, so custody moved into the guard's post-census step. Not a benchmark or real-drive qualification: a real USB/Thunderbolt disconnect and a long-lived wrapper server are not exercised. **Run history, preserved.** The 2026-10-30 QA baseline at `1d6312d` found this selector red with `registration failed`: the product correctly refused its image with `the mounted root is not provably closed to this user: … MNT_IGNORE_OWNERSHIP`, because the script attached without `-owners on` while the newer proofs already did. That is a stale proof premise, not a product regression; oxide#191 adds the owners-on attach, the mount observation and the self-check pin. Both modes stage through the shared `layout.stage_at` helper's narrowly scoped `retain_stage_root=True` opt-in, which keeps the staged Cargo tree inside the guard's owned root for its post-leader census while the helper's default still removes it for every other selector; and a failure after any child launch, a diagnostic collection that RETURNS with any recorded failure, abort, skip or unexecuted case, and any unreaped child all keep the mount and its backing for the guard's post-leader census rather than detaching them; only a child-free failure and a fully clean collection detach ordinarily. The recorded result line follows that top-level decision in both modes (the focused mode returns a `{custody}` placeholder that `main` fills after `retain_for_census`), so a retained mount is never reported as a detach. The pure self-check drives the real helper route for both retention modes and the main-path custody matrix for both modes. With owners honored the script then reached a second stale premise — the product correctly refusing its internal directory-budget pool on the external selected volume — so the selector then passed `--root-parent /private/tmp`, the owner-accepted internal-placement exception `bd357a4c` the boot-closure mode already needed. S4 (oxide#212) replaces that recipe: the pool is a declared scratch pool on the selected volume and no `--root-parent` is passed, with native evidence re-established at the migrated head. A counted author native then passed at `2a409d5`: 29 s script time, monitored owned peak 99.7 MiB of 512 MiB, host free minimum 2196.0 GiB, the idle slot warm across removal, the held build stopped with exit 143 and its lost slot deleted whole after re-attach, and the wrapper-allowed slot quarantined and kept. That pass stays historical at that script. The exact-head review then required the uncounted keep-going diagnostic the repository policy demands before a counted native: the main mode is now a fixed `idle` → `held` → `held-delete` → `quarantine` case map with `--diagnostic` (`effigy rust:oxide-removable-removal:diagnostic`), an effect-free `--plan` (`effigy proof:removable-removal:plan`), and the nine premise dispositions the pure self-check binds to the actual fixture environment (private HOME and invoking rustup home, `OXIDE_TEST_HOME_FROM_ENV=1`, the isolation scrub, the frozen `RUSTFLAGS` with no encoded value and a refusal of an inherited `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` before staging, the owner-scoped scratch, and `TMPDIR`/`TMP`/`TEMP` on it). Its first diagnostic exposed two fixture defects rather than product findings, both now fixed and pinned: a forced detach can leave a stale *detached* hdiutil entry, so custody is defined as a *mounted* entry of this image (a real second mount still refuses), and a case that failed before its `finish` left its child in the guard's census, which the guard honestly reported as `UNCLOSED`; each case now reaps its own child positively. After that repair the diagnostic completed 4/4 cases at `558469a` (37 s script time, guarded owned peak 85.4 MiB of 512 MiB, closed, no deadline) and the counted author native passed at the same head (28 s script time, guarded owned peak 85.4 MiB of 512 MiB, host free minimum 2195.8 GiB, closed/status 0/no deadline). A second exact-head review then found two source defects, fixed without running either selector. A timed-out or otherwise unclassifiable child, writer, mount, device or custody outcome aborts the collection and is recorded as `aborted` with the remaining cases `unexecuted`; only an `oracle(...)` product expectation is recoverable, and the receipt keeps fault, abort and unexecuted truthfully. Staging now runs inside `staging_environment_scrubbed`, which removes every inherited `OXIDE_`/`XDG_`/`CARGO_`/`SCCACHE_`/`RUSTC*` key and restores only the invoking `CARGO_HOME` (set explicitly), and the fixture environment drops all `CARGO_*`/`SCCACHE_*`; the pure self-check pins both, including the staging scrub's restoration on the failure path. The `558469a`/`a0804fa` receipts stay historical at those heads. The first diagnostic at `9459ebe` then failed in setup before any case (`KeyError: 'OXIDE_PROOF_DEADLINE_NS'`): that scrub deleted the guard's own control variables, which the shared `layout.stage_at` → `work_left()` path reads to bound each staged child. That packet is historical with no credit. A narrow correction keeps exactly `GUARD_CONTROLS` (`OXIDE_PROOF_DEADLINE_NS`, `OXIDE_PROOF_EVIDENCE`) visible through the scrub while removing every fixture-facing `OXIDE_*` value, and a setup exception now records the real fault as `aborted` with all four cases `unexecuted` (a marker-style `OracleFailure` still records `failed: [[setup, …]]`) rather than an empty, fault-less receipt; the pure self-check exercises the guard-control visibility through `layout.work_left()` and both setup-abort receipt shapes. A further correction binds the run to its source before any effect: `resolve_source` requires both git calls to exit 0, a nonempty exact 40-hex commit identity and a clean tracked worktree — a nonzero exit with empty stdout is never a clean worktree — and a lookup that dies, times out or is interrupted, a nonzero exit, a non-exact identity and a dirty tracked worktree all abort inside the receipt-writing path with the fault, the abort and every planned case `unexecuted`, never reaching staging, image creation or attachment; both modes are gated the same way and the constructors refuse an unvalidated source. The pure self-check covers every source-gate branch plus the clean identity that reaches setup with exactly that source. The corrected head needs a fresh diagnostic and counted native under separate owner activation, and the earlier `UNCLOSED` packet's attached image (`/dev/disk4`, `/Volumes/OxRm05494fb1`) and its backing remain retained, untouched evidence.

**Boot-session closure proof (oxide#161).** `effigy rust:oxide-removable-boot-closure` (heavy) is the same script's focused `--boot-closure` mode under the qualified shared proof supervisor (`bounded-test.py --limit 300 --scratch-budget-mib 512`); its directory-budget pool is the same declared scratch pool on the supervisor-selected volume (scratch-pools S4, oxide#212), so no internal root is passed, and everything stays inside the same monitored 512 MiB root. It stages this HEAD's debug `oxide` offline with 1.97.1 (the one allowed seam is the debug-only `OXIDE_TEST_BOOT_MS` override, honored only under the supervisor's own canonical scratch/evidence/deadline layout and absent from release builds), registers one owned 128 MiB scratch APFS image at a unique `/Volumes/<name>` in permanent mode (the same owners-on attach and mount observation as the main mode, oxide#191) with a private `HOME` and `CARGO_HOME`, and makes every quarantine with the product's own removal path (`oxide exec` on a blocking tool, `hdiutil detach -force` of the image's positively bound device: the removal stop ends the tool and the release flush fails with the mount gone, so the lost record names the exec holder and is quarantined); no state file is edited and no other debug seam is used. Four independent cases run over separate checkouts: a quarantined `exec` slot whose override boot time follows its holder start is reported closed by `status`, an explicit `outputs adopt` of a whole-target fixture then migrates, and the next admission deletes the slot whole with the boot-session line and no record left (the oracle names the slot's own leaf, never the project-label folder, and checks the leaf gone with the folder still present); a slot whose override boot time precedes its holder start stays kept with its directory, record and status line; an unreadable override stays kept; and the operator-confirmed release (oxide#196) uses an `oxide exec` tool that forks a child which leaves the product's exec group — the removal stop signals that group — by joining the supervisor's own leader process group (`FIXTURE_LEADER_PGID`) and holding a file in its slot scratch; the product's stop cannot end it, the supervisor can census and signal it, and the proof confirms the child's group before the removal. The child publishes readiness only after it observes the scratch path absent (the detach) and then present again (the re-attach), opening a fresh handle on the returned tree; a pre-detach handle or a readiness written before removal is never accepted. It redirects its inherited standard streams to `/dev/null` and closes the other inherited descriptors before it waits, so the proof's pipe wait cannot be held open by it. With that escaped exec writer alive the release refuses without the flag, refuses with the flag (naming the process) and refuses a dry run, all with the state and ledger bytes unchanged; once the proof stops the writer, a dry run reports the clean census and changes nothing, and the release succeeds, prints its slot/bytes/census/flag line and the usual deletion line, deletes the slot leaf through the holding area with the project folder and no record left, and leaves `status` without the slot. The writer is stopped in a `finally` and bounded by its stop file. Custody is the same post-census detach request as the main mode (oxide#191): the guard detaches only that recorded scratch image, only after its own census, and keeps the mount and backing otherwise. `effigy rust:oxide-removable-boot-closure:diagnostic` is the same guard, root and limits with `--diagnostic` appended: the uncounted collection mode records reached, completed and failed cases, continues into the independent cases, stops on any child, volume, device or custody outcome it cannot classify, and writes one bounded `qualified: false` receipt beside the guard's; it is never a counted native PASS. `effigy rust:oxide-removable-boot-closure:test` runs only the script's pure `--self-check` (no child, pool, image or device): it pins the full fixture environment every boot-closure child receives, `status` included, against a child handed only the override; the deletion oracle's slot-leaf resolution — the slot's own `<profile>-<id>` leaf, resolved by the recorded id, inside its project folder and never that folder, with a counterfactual in which a resolver returning `slots/<folder>` is refused; the bounded, unqualified diagnostic receipt and its `json` document; the one owners-on attach option list every mode shares, with the `/sbin/mount` observation refusing an ignore-ownership mount and a listing without this image and `Volume.attach` pinned to it (oxide#191); the escaped exec writer's syntax, exec-group escape and supervisor-leader-group membership, its exercised detach/re-attach readiness state machine, the release oracle's `process <pid>` form, the argument dispatch; and the product's own deletion line the candidate marker quotes. The first exact-head review found the missing `json` import and reported a partial `status` environment; both apparatus defects are fixed and pinned there before any native activation. The second found the before-boot deletion oracle naming the project-label directory `slots/<folder>` rather than the slot leaf — the deleting admission creates a fresh slot of the same project in that folder, so the folder could neither prove nor disprove whole-slot deletion. That oracle now resolves the leaf before the build from the recorded id alone, requires the leaf (not the folder) to be present before the deletion, and, after the admission, requires the leaf gone, the project folder still present and the state record removed. An owner-activated `rust:oxide-removable-boot-closure` diagnostic at `8f7573b` reached 3/4 and failed: the escaped-writer case exited 143 and quarantined its slot, but the release-quarantined case refused because macOS hid `UserEventAgent` descriptor 6. The counted author and independent reviewer natives were not run at that head, and that packet is historical with no proof credit. At the published `a04d587` head, under Tom's direct-execution exception `d3d61247` (admission decision `7b4b55d3`), the clean boot-closure diagnostic completed 4/4 with zero failures and the counted author native passed; a separate fresh subagent in its own worktree ran the counted independent reviewer native and it passed, with all sessions closed by a qualified post-census detach. Archive `/Volumes/Builds/scratch/oxide/evidence-196-a04d587-20261101T014402Z` (NOTE.md, sessions.jsonl, SHA256SUMS) records the exception, executors and evidence; the earlier failed `8f7573b` packet remains historical with no proof credit, and the merge hold stays until the final review passes. Not a benchmark, real-drive qualification or an alias for full QA.

Author evidence (2026-10-22, two submissions, both spent), both under the oxide#109 guard (`bounded-test.py` SHA-256 `8e379c93…`); the selector now runs the merged oxide#114 guard (`d89115f1…`). The independent reviewer ran the proof once at final head `78b21f8` under that guard: supervisor closed with exit 0, all three removal cases passed, script time 74 s, owned peak 66.0 MiB of 512 MiB and sampled host free minimum 668.1 GiB. The held build stopped with exit 143, its slot was deleted after re-attach, the idle slot stayed warm, the quarantined slot was retained, and checkout-local fallback `run` and `test` passed. All four exact-head review gates passed. This is reviewer evidence at the final head; the author runs below retain their original heads and guard hashes. The branch was then merged with main (#114, #115) and the covering checks rerun at that head. The first, at `ef45b07` (before the removal stop), failed: the held build did not exit within 60 s after the forced detach because its build script, running from the removed volume, spun at full CPU; the supervisor reported `UNCLOSED` and kept the scratch, and SIGTERM to that build script then ended it and its Cargo. The second, at `7b45a2a`, passed in 57 s of script time (supervisor `closed`, owned bytes peak 65.4 MiB of 512 MiB, host free minimum 667.9 GiB). The removal stop sent TERM 24 ms after it sampled the missing volume, the held and quarantined builds exited 143 (Cargo ended by TERM), the lost slot's directory was still on the image after re-attach and was deleted, the quarantined slot's directory was kept, and the idle slot was reused warm. A forced detach of a disk image flushed more than a yanked cable may; neither persistence result is a claim about real drives.

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

**Surprise removal (part 2, implemented above).**
- A build whose volume vanishes fails like any I/O error, and its lease is marked lost-volume.
- **On releasing an external slot, Oxide flushes the volume** (`sync_volume_np` with full-sync-and-wait, or an equivalent the implementation cites). So a released slot's writes are on the device before it can be reused.
- A flush failure makes the slot held and non-reusable. It does not authorize deletion while writers are unknown.
- After an unclean disappearance, slots held at that moment are non-reusable. Discard them only after positive writer closure; otherwise quarantine and retain accounting. A vanished device, or Oxide's Cargo group ending while an inherited sccache server may still write, is not sufficient closure. A restart is a positive-closure fact for a holder that started before the current boot session, with the 60 s margin above; a device that merely disappeared is not. Idle, flushed slots are kept.
- The ledger never lives on the removable volume.

**Final outputs stay checkout-local by default.** `CARGO_TARGET_DIR` remains `<checkout>/target` regardless of where slot intermediates live, keeping built binaries at the expected checkout path when undocked (Tom, 2026-10-20). The one exception is an explicit per-checkout conversion under permanent drive mode, below: `target` stays the lexical `CARGO_TARGET_DIR`, but it becomes an Oxide-owned link whose real root may be on the volume.
- **Following the pool is implemented as an owned link, never a move.** oxide#098 kept outputs checkout-local; oxide#117 adds the opt-in permanent-drive placement described under [final outputs on the permanent drive](#final-outputs-on-the-permanent-drive-oxide117). Docking alone never moves final outputs, and an interrupted command is never automatically replayed.

**What moves and what doesn't:**
- **Moves by default:** slot intermediates. Final outputs stay checkout-local.
- **The sccache cache follows the pool only if #096's serialized per-build server model is adopted** (cache inside the pool). The current machine-wide server fixes its cache path at start-up. If that path is on a removable drive, a disappearance can silently recreate it on the internal disk, so it is not moved.
- **Queue worktrees and Colima's VM disk stay internal:** they hold live state that an unplug would break.
- Spotlight indexing of build locations is a host setting, recommended separately.

**Original drive:** the dedicated spare 1 TB SSD (Tom, 2026-10-20), formatted by Tom in Disk Utility as plain APFS volume `Builds` (UUID `A6AC3ED7-33EA-46B4-8082-7254CF291ED2`). Vault and its Time Machine backups stay untouched. **Decided (Tom, 2026-10-20):** a 750 GB Oxide admission cap and a 100 GB free-space reserve, both set at registration. The flush primitive is implemented (`fsync_volume_np` with full sync and wait; see part 1 above); **not yet measured:** its cost on the real drive.

### Final outputs on the permanent drive (oxide#117)

The oxide#210 shared image helper requests owners-on on every attach and re-attach and checks `/sbin/mount` read-only for `noowners` before a registering mode continues. At `6c07980`, five uncounted mode diagnostics refused registration on ignored ownership; two completed. Those receipts remain historical and none transfers to the corrected shared script.

The oxide#210 proof correction (decision b0eba56a) exercises explicit `outputs adopt` on a committed whole-target checkout as a migration to profile members, including an active writer and the `migration-p0`, `migration-staged`, `migration-marker`, `migration-backed-up`, `migration-swapped` and `migration-p4` stops and replay. The older `prepared`, `linked` and `renamed` member-transition hooks remain reachable through explicit first adoption of never-converted profile checkouts (`outputs.rs` `adopt_profile_from` calls `transition_at`); the proof names those hook families separately. The fresh proof and its Supervisor-owned native sessions remain pending at the changed head; no old evidence transfers.

Implemented for permanent drive mode; ephemeral drive mode is unchanged.
This owns final-output placement only: not compiler fingerprints,
intermediate slot limits, sccache, scheduling or admission. Device
qualification on the real drive, conversion of Tom's checkouts, data
migration and consumer adoption remain separate gates.

- **One owned link.** A converted checkout has one Oxide-owned
  `<checkout>/target` symlink. `CARGO_TARGET_DIR` stays that lexical path, so
  `target/debug` and the #106 profile rules are unchanged. Switching the
  registered drive back to ephemeral does not move or unlink a converted
  checkout: it keeps building through the committed root its owned link
  selects, never relinked, and refuses when the link no longer matches the
  receipt, or when that root is external and the command's intermediate slot
  fell back to the internal pool (run `oxide outputs detach`, or return to
  permanent mode and run `oxide outputs adopt`).
- **Roots.** The real root lives outside the pool payload:
  `<mount>/oxide-outputs/<project>-<checkout id>/` on the registered volume,
  else the checkout-local fallback `<checkout>/.oxide-outputs/local/`.
  `oxide init` idempotently adds `/.oxide-outputs/` to the workspace
  `.gitignore`, and establishing the local root refuses when Git would track
  it: the check runs before ownership is established, including inside a
  `.oxide-outputs` directory that already exists and for an already-present
  empty `.oxide-outputs/local`, which is then left unchanged.
- **First-conversion ignore prerequisite (oxide#125).** Before a first
  conversion — explicit `adopt`, automatic adoption or the replay of a
  prepared first conversion — writes any receipt, root or link, Oxide
  positively establishes this checkout's real Git work tree and the exact
  repository-relative path of the `<checkout>/target` link (no trailing slash,
  no escape). It asks Git itself, through bounded, effect-free calls with
  optional locks and prompts off and inherited repository overrides removed:
  `rev-parse --show-toplevel` for the work tree, `ls-files` for an already
  tracked path, and `check-ignore --no-index -v -z --stdin` for the ignore
  decision and its winning rule. The link path must be untracked and its
  winning rule must be neither a negation nor a trailing-slash
  (directory-only) rule. A real empty target makes Git answer for a
  *directory* with a trailing-slash rule, so that answer is never read as
  eligibility; slash and unsupported rule forms refuse even when that is a
  false negative. Unknown is never permission: missing Git, a
  non-repository, a timeout, a command error and malformed, oversized or
  ambiguous output all refuse, leaving the absent or real target, the receipt
  and the roots unchanged and naming the fix: use `/target` (or `target`), not
  `target/`, in the repository `.gitignore` (a nested workspace names its
  exact repository-relative rule). A committed link, `detach`, `status`,
  `plan` and ordinary builds are unaffected, and no already-created link is
  cleaned up automatically.
- **Eligibility versus refusal (oxide#154).** The same prerequisite decides
  *eligibility* for an automatic first conversion, asked for every member link
  that conversion would publish (`target/debug`, `target/release`; oxide#160),
  because that is what its creator checks before it creates the real `target`
  directory. When Git cannot confirm one of them — no repository, no Git, an
  unignored or negated path, an already-tracked member, a timeout, a command
  error or malformed output — the leased command is an ordinary unconverted
  checkout: it builds into the real local `target/`, intermediates stay in the
  slot, and exactly one unconverted notice is printed. Nothing else happens:
  no receipt, root, member link, prepared transition or downgrade fence is
  written, and the check runs before the barrier and the admission lock. A
  covering rule still converts, and because the published paths are members
  under the real target directory a directory-only rule such as `target/`
  genuinely covers them and is accepted. That is ineligibility, not a
  swallowed failure: explicit `oxide outputs adopt`, the replay of a prepared
  first conversion and every other conversion path still refuse with the exact
  fix, a corrupt or unreadable receipt is still an error, and a refusal after
  partial effects still stops with the prepared receipt and no member link
  rather than continuing as a local build.
- **`oxide init` diagnostic (oxide#154).** After its guidance and local-output
  work, `oxide init` asks the same effect-free probe for the first member link
  this conversion publishes and, inside a Git work tree, warns with the rule
  that covers them all (`/target`, or the workspace's repository-relative
  spelling such as `/rust/target`) when that path is unignored, tracked, or
  Git cannot answer. A directory-only rule such as `/target/` really does cover
  the member links, so it is not diagnosed. `oxide init` never edits or
  rewrites a user's `.gitignore` rule and adds no `target` rule of its own; a
  covering rule and a checkout outside Git are silent.
- **Keyed state and the binding mutex.** A command keys its slot with or
  without an owned destination when it starts. If the checkout was converted
  or detached while it waited for admission, binding refuses before Cargo
  and the command is run again. Every command in a converted checkout, in
  either drive mode, holds the checkout's binding mutex from its receipt and
  root checks until it is recorded as running, so a permanent-mode relink
  never repoints the link during another command's startup.
- **Unconverted links.** In permanent mode a checkout with no committed
  receipt (never converted, or detached) keeps a real `target`. A link there
  (a foreign link, or a marked root linked without its receipt) is never
  handed to Cargo or followed by clean: builds and selected or whole cleans
  refuse with nothing changed. Ephemeral mode keeps today's behaviour.
- **Committed link.** A live link must select the receipt's committed root.
  A link to another owned root (an older selection restored by hand) is
  foreign even when that root's marker is valid: binding refuses and changes
  nothing.
- **Ownership.** A root carries `.oxide-output.json`
  (`oxide.outputs.root/1`: checkout, checkout id, project label, placement
  recipe, kind, volume UUID, creation generation). A link whose target is
  not a real directory carrying this checkout's marker is foreign: never
  followed, adopted, swapped or cleaned through. A marker for another
  checkout, kind or volume refuses.
- **Receipt.** `<internal pool>/oxide-managed/outputs/<checkout id>.json`
  is strict, bounded (64 KiB) and holds the committed selection and
  generation, a bounded history of at most eight known roots, or exactly one
  prepared transition. The published formats are whole-target
  `oxide.outputs.transition/2` (one `target` placement) and profile
  `oxide.outputs.transition/3`; the whole-target `oxide.outputs.transition/1`
  shape oxide#117 wrote is read by the prune readers (see the [prune
  section](#pruning-owned-output-roots-oxide129)) and, for a committed
  document only, by the explicit `oxide outputs adopt` upgrade (oxide#163, see
  the explicit migration bullet below); every other reader still refuses it as
  an unsupported version and names that command. It lives in the internal pool's
  control directory whichever pool a command's slots come from, so one
  checkout never has two receipts. An unknown version or field refuses, and a
  prepared transition's link must be exactly this checkout's `target`; the
  rename destination is derived from the checkout, never from the file.
- **Downgrade fence.** Before `adopt` can publish a first link it writes the
  forward-only `owned_outputs` fence into both pools' state (creating a
  state that is absent). Every pre-feature Oxide parses state strictly, so it
  refuses the pool ("Oxide state is invalid") instead of building, running or
  cleaning through an owned link without its binding, custody and clean
  checks. The fence is never cleared; an unknown fence value refuses.
- **Transition.** Under a per-checkout binding mutex — a plain lock that no
  holder waits on, so second-slot commands still start concurrently when
  their destination is unchanged and two starters can never block each other
  — the transition waits, without the mutex, until no other command in the
  checkout may be running Cargo, then rechecks the receipt, volume, device,
  marker and generation and publishes
  `prepared`, create the destination root and marker on the verified volume,
  create the unique sibling `.target.oxide-new-<generation>` link and sync,
  validate it, atomically rename it onto `target` (a no-replace `RENAME_EXCL`
  rename when `target` was absent at classification, so a creator that appears
  in between is kept and the publish fails with no replacing fallback), then
  publish `committed`.
  Generations are exact `u64`s; receipt paths are absolute and `..`-free. No
  step recreates an absent mount point, follows a link, or unlinks by path
  text. Unknown writer custody — a lost slot whose writers were never
  positively closed, in either pool, and whose holder started in this boot
  session — refuses any relink or clean for that checkout until it is
  resolved. A record whose holder predates this boot session is closed by
  the restart. Record IDs are per pool, so a command is
  exempt only from its own record in its own pool; an equal ID in the other
  pool is another command.
- **Crash replay.** Only the exact prepared/old generations replay, and a
  building command replays only under the same cross-pool custody rule as a
  fresh transition: unknown writer custody refuses with the link, sibling,
  prepared receipt and roots unchanged, and a command that may be writing
  must close first. A live
  prepared sibling is renamed and committed; an already-published prepared
  link is committed — in both cases only after the prepared root passes the
  full check (marker kind, volume UUID, generation, live registered device,
  ancestry), so a contradiction stops with the old link, the sibling, both
  roots and the prepared receipt unchanged; a preparation that published nothing (the sibling
  positively absent) rolls back to the old selection and keeps any root it
  created; a sibling of another kind, selecting another root or unreadable
  stops with the receipt, links and roots unchanged. The sibling is classified
  before every replay mutation, so an already-renamed target commits only
  when the sibling is positively absent. The old state is a missing
  target (a first conversion has no committed selection) or the receipt's
  exact old owned link: a link that appeared after the crash, even a dangling
  one, is never renamed over. Any other state stops, keeps both roots and the
  link, and says so.
- **Slot identity.** An owned checkout's selected destination enters the slot
  compatibility key: kind, real root, volume and the generation that root has
  or will be created at (its marker's creation generation, else the next
  transition's). The binding must produce that exact root and generation, or
  the command refuses and asks to be run again. Relinking to the same root
  keeps the key; a destination change partitions it cold. An ephemeral or
  detached checkout adds nothing to the key.
- **Selection.** A command selects its destination once, when it starts (the
  external root only when the volume is present, registered, writable, above
  its reserve, the root is on its device with a matching marker and the
  command's intermediate slot comes from the removable pool, whose watchdog
  supervises the drive's removal; otherwise the local fallback. The pool is
  selected first, so a pool fallback (for example a container mismatch with
  the drive still mounted) moves outputs to the local root too), keys its slot from that destination, and then binds
  exactly it before Cargo starts. Every reused root is revalidated in full —
  this checkout's marker, kind, volume, creation generation, device and
  ancestry, so a replaced `oxide-outputs` ancestor or a mismatched marker
  refuses instead of writing onto another disk. The local root must be
  exactly `<checkout>/.oxide-outputs/local` under a real `.oxide-outputs`
  directory and resolve to that path: a symlinked ancestor, even on the same
  device, refuses reuse, establishment and clean. If the drive changed while
  the command waited, the command refuses and asks to be run again rather
  than binding a root its slot was not keyed for. No command is redirected
  mid-run. A checkout that is not converted keeps ephemeral behaviour with a
  warning in permanent mode.
- **`run` custody.** For a converted checkout, `oxide run` keeps its lease
  through the program instead of replacing itself with it: the program runs
  from its path in the bound real root (not through the link) as the
  command's own process group, which takes the terminal's foreground while it
  runs. The record names that group, so a relink waits and a clean waits
  until the whole group (including ordinary children that outlive the
  program) is observed empty; a crash of Oxide leaves the slot held until the
  group is gone. On a removable slot the removal watchdog runs through the
  program phase: a vanished drive stops the group (TERM, then KILL), the
  command fails, and a group that cannot be confirmed closed quarantines the
  slot as unknown custody. A program on the internal disk is never stopped
  for host free space. Unconverted checkouts keep `exec` after release.
- **Explicit commands.** `oxide outputs plan` is effect-free bounded JSON
  (`oxide.outputs.plan/2`, always `data_move: false`, with cooperative
  holders in either pool and the next generation). On a committed
  whole-target checkout whose owned link selects its recorded root — the one
  shape `adopt` migrates — it reports `action: migrate`, the exact member set
  the migration classifies (the same reader `adopt` uses, including members
  the root does not hold yet), the proposed placement as that same root at
  the next generation, and, while the root still carries a root/1 marker,
  `marker_upgrade` with `oxide.outputs.root/1` → `oxide.outputs.root/2` and
  `nonce: reserve` (oxide#182). The plan reserves no nonce, writes no
  receipt or marker and opens no pool. `adopt` converts only a missing or empty
  real target; a nonempty real target and a foreign symlink refuse without
  mutation, and an owned correct link is a no-op. `detach` restores a real
  empty `<checkout>/target` directory and keeps every root in either drive
  mode (oxide#124): it is the per-checkout opt-out, and it refuses a foreign
  link or unknown writer custody without change. `status` is read-only.
- **Automatic first conversion (oxide#121, profile layout since oxide#160).**
  A leased command (build, run, test, exec) converts a fresh checkout before
  its destination is keyed and before its slot is taken, when all of these hold: the command's selected pool is the
  registered permanent drive as selected at its start (not the internal
  fallback; the registration it selected must be permanent, and the volume
  and root in force now must be that same selection, so a switch is a no-op);
  the drive is usable now (writable, above its reserve, live UUID and device); the checkout has no
  owned-output receipt of any state; and `<checkout>/target` is positively
  absent (no file, directory or link, dangling links included). It runs the
  adopt barrier, downgrade fence, admission lock, roots, marker and receipt,
  with a missing-target-only transition, and releases the barrier before the
  slot is taken. Every fact is rechecked under the admission lock. A concurrent
  receipt, creator, mode or volume change makes it a no-op, and the existing
  path decides. The conversion creates the PROFILE layout (oxide#160, S4b):
  it runs the same creator an explicit `oxide outputs adopt` runs for a fresh
  checkout, so it publishes a real `target` directory holding owned
  `debug`/`release` member links into one new owned root, with the same
  receipt format and root marker, and every member's Git-ignore prerequisite
  is checked before the real target directory exists, so a refusal changes
  nothing at all. It never converts or keeps anything that already exists: an
  empty or populated directory, a link or a member-shaped entry appearing
  where a member must be created refuses, and the missing-target-only
  transition is unchanged. Existing whole-target checkouts are untouched and
  migrate only through an explicit `oxide outputs adopt`; crash replay keeps
  the layout a prepared conversion recorded. Everything else keeps today's
  behaviour and is never touched:
  a real target (empty or not), a foreign or dangling link, a file, an
  unreadable or corrupt receipt (an error, never absence), a detached
  checkout (its receipt stays, so a missing target beside it is excluded),
  ephemeral mode, an absent or unusable drive and internal fallback. A
  preparation that published nothing rolls back to a detached receipt, which
  excludes it from automatic adoption and whose commands then run locally;
  `oxide outputs adopt` still converts it. Exactly one stderr notice, `oxide: auto-adopted <target> -> <root> for
  permanent-drive outputs; no existing outputs moved`, follows the durable
  commit; replays and later builds print none. A committed adoption persists
  when a later admission, build or cancellation fails. No persisted format
  changes: the receipt, marker and fence are as before. `oxide outputs
  detach` is the per-checkout opt-out in either mode (oxide#124). A
  detached receipt is a receipt of any state, so automatic adoption never
  follows it, and `oxide outputs adopt` converts it again through the
  existing preconditions. To opt a checkout out before its first build, create
  a real empty `<workspace>/target` directory first: it is never adopted, and
  Oxide never converts an existing real target automatically. The first
  conversion's Git ignore prerequisite is part of this rule as *eligibility*
  (oxide#154): when it cannot be positively established, the command is an
  ordinary unconverted checkout with a real local target and one notice rather
  than a failed build — see [#125](#final-outputs-on-the-permanent-drive-oxide117).
  A new notice-bearing conversion still prints its one `oxide: auto-adopted`
  line; ineligibility prints no such line and no second notice.
- **Clean.** Resolves the receipt to the selected real root, requires it to be
  the current owned link and to sit on the volume the receipt names *now*
  (a replaced drive at the same mount point is refused), and applies the
  existing enumeration/foreign/symlink/device predicates there. A selected
  (`-p`, `--doc`, `--profile`, `--release`) clean follows the same receipt and
  link check, repeated after its barrier is granted and immediately before
  Cargo runs, so a foreign, replaced or prepared link is never cleaned
  through. The root, its marker and the link are kept, inactive roots are
  reported, and a missing identity, a pending transition or unknown writer
  custody refuses. `--include-foreign` cannot override that. Ephemeral clean
  is unchanged.
- **Removal.** A volume that vanishes fails the running build through #098's
  stop/lost/quarantine path. The link is never swapped mid-command, and
  neither root is copied or deleted. The next command transitions to the
  selected root once the checkout is quiescent; an unknown or contradictory
  state refuses rather than overwrites.
- **Limits.** Tools that bypass Oxide stay outside cooperative custody; no
  universal reader containment is claimed.

**Proof.** `effigy rust:permanent-outputs` runs
`scripts/test-permanent-outputs.py` as the whole command of the qualified
shared proof supervisor (`bounded-test.py --limit 300 --scratch-budget-mib
512`, oxide#109). It stages this clean HEAD's `oxide` offline with 1.97.1,
models the permanent drive as one owned 128 MiB APFS sparse image at a unique
`/Volumes/<name>`. The backing stays in the guard evidence directory until an
ordinary detach positively removes both the mount and recorded devices; unknown
custody retains it and fails the run. The same custody rule applies to every
focused mode, and `effigy proof:exec-inference:test` checks its counterfactuals.
The proof covers: the effect-free plan; refusals for a nonempty
real target and a foreign symlink with no mutation; explicit conversion and
builds, `run`, `test`, `--profile debug` and `target/debug` binaries through
the link; the local fallback while the drive is absent and redocking with
both roots kept; the git-tracked local-root refusal; a wrong recorded volume
UUID with no external write and no recreated mount point; a second command
waiting for a running build and then binding its own destination while the
running build keeps the root it selected; a long-lived `exec` and two
simultaneous commands that both reach Cargo, so the second-slot rule and a
non-blocking starter path hold; an active build script blocking an explicit
relink; a replaced `oxide-outputs` ancestor refused for an unchanged
destination with no write onto internal disk; crash replay at the prepared,
sibling and renamed points plus the contradictory-state stop, a sibling-point
replay refused before its rename when the registration names another volume,
and a crafted link that appeared after a crash; a same-device symlinked
`.oxide-outputs` refused by build and clean with the tree it names unchanged;
a build and a selected clean refused through a receiptless foreign link with
its referent (including a planted Cargo-shaped artifact) unchanged;
a pool fallback (container mismatch) with the drive present moving outputs
to the local root and redocking; a gated `oxide run` program executing from
the real external root whose ordinary child outlives it, keeping `run` held
(a relink waits, `plan` names the holder) until the child exits; a forced
detach of the owned image while a program runs from the external root,
stopped by the removal watchdog, then re-attach and redock; the last
pre-feature Oxide (`8c9b5ae`, staged offline) refusing `check` and a selected
clean on the fenced pools with the link and root unchanged (since oxide#118's
pool layout it refuses at pool accounting before parsing state; the state
fence itself is unit-tested); a build
in ephemeral mode before detach through the committed link; a crafted quarantined writer blocking both
relink and clean; selected clean refusing after its link turned foreign while
it waited, and selected and whole clean refusing a replaced or unusable
volume while preserving the root, marker and inactive roots; a second
fallback command reusing its slot warm; a pre-created empty local root
refused when Git would track it; and `detach` restoring a real directory in
ephemeral mode (permanent mode: oxide#124). Not a benchmark, real-drive
qualification or an alias for full QA.

### Owned output members at profile directories (oxide#133)

The bound v1 successor to the whole-target placement. `<checkout>/target`
itself stays a **real directory**; only positively identified profile members
become owned links. Cargo's top-level files (`CACHEDIR.TAG`,
`.rustc_info.json`), `target/tmp` and tool directories such as `target/doc`
stay real.

**Detach and target-member links (oxide#144).** `oxide outputs detach`
recognises the host-triple and every other supported member link
(`debug`, `release`, `<triple>/debug`, `<triple>/release`): it removes each
link whose destination is exactly that member's approved subpath beneath the
root its placement selects, and restores a real empty directory there. The
member suffix alone never grants ownership. A destination is classified by the
shape of its link text — the plain `<root>/<profile>` and the nested
`<root>/<triple>/<profile>` forms are both considered, most specific first —
and only a candidate root that is a real directory carrying this checkout's
owner marker makes the link owned. A wrong destination, an unmarked or
another checkout's root, a missing destination (dangling) and a relative or
otherwise malformed link text all keep the previous fail-safe refusal, with
the link, every other member and the receipt bytes unchanged; there is no
link traversal, no arbitrary removal and no force mode. A profile receipt
places its members independently, so an unselected member is a real local
directory (or one left unselected by a detach that a later adoption converts
again); the detach keeps every placement as re-adoption authority, and
re-adopting after a multi-member detach — including the host-triple member —
is the same journaled per-member transition as any other adoption. **Evidence
limit:** two uncounted diagnostics have run, both unqualified and neither a
PASS. Diagnostic 1 at source `51fcb3d` completed cases 01–15 (case 15 — the
previously refused host-triple detach — passed), failed case 16 on its
auxiliary-pool premise and skipped 17–20. Diagnostic 2 at source `ee192f7`,
after that premise was corrected, completed cases 01–17 (case 16 passed), failed
case 18 and skipped 19–20. Case 18's own premise is corrected here: the
unconverted first build cannot warm the adopted key, so the adopted real
placement gets its own seed build and an actual repeat, asserted warm in that
same parsed slot before the member is removed, re-adopted and re-keyed. Cases
19–20 have still never executed, no counted native PASS exists, and counted
author and reviewer native remain unactivated.

- **Members and destinations.** A converted checkout's `target` is real and
  each linked member selects its own approved subpath beneath the shared root
  (`R/debug`, `R/release`, `R/<triple>/debug`, `R/<triple>/release`); the root
  keeps one immutable nonce, marker and creation generation. `run` maps a
  Cargo-reported program beneath its own owned member's link only; a real
  (unlinked) member keeps Cargo's own executable path, so a mixed checkout
  never runs another member's artifact. An internal-pool fallback slot refuses
  before Cargo while an external member link is live, and a command whose fresh
  plan no longer selects a member's committed external root (a still-mounted
  volume that became unusable after the slot was keyed) refuses before Cargo
  too, with or without a fallback slot and whatever the live link looks like:
  an owned external member is never moved to another pool automatically, and
  only an explicit adopt changes where a member is placed. That holds for a
  plan the production planner derived itself, which falls back to the local
  spec when the registered volume cannot be verified, not only for a plan
  supplied to binding. An explicit adopt
  compares the live link with the desired destination (relinking under custody
  on a fallback or redock rather than trusting an old receipt match). A profile
  clean is validated before the legacy whole-target pre-gate, plans each
  member destination once, and preserves links, root markers and unrelated
  payloads. In the profile layout `<checkout>/target` is REAL, so a whole
  clean keeps the target directory itself, every owned member link and every
  real ancestor directory that holds a nested link (a `<triple>` directory),
  and removes the payload around them; keeping a kept directory never skips
  the walk inside it. A retained triple directory is itself a classification
  boundary: the entries beside its owned member link are held to the same
  foreign-entry policy, and the refusal names each of them relative to the
  target (`<triple>/<name>`), so nothing below a kept ancestor is deleted
  unexamined. An owned member root holds payload, not a target root:
  its `deps`, `incremental`, `build`, `.fingerprint`, `examples` and artifact
  files of any name are Cargo's, while an unknown directory or a symlink in it
  is still foreign and refuses. Prune eligibility and candidate construction
  are per
  `(checkout, root)` with a first-sorted representative: a root under one
  checkout is ONE owner, so its own sibling members never keep each other,
  while a reference from a different checkout does keep the root. A dangling
  member link never transitions automatically (a command on an internal
  fallback names the fallback and `oxide outputs adopt`). The one repair
  (oxide#162) belongs to a derived `oxide exec`: when the link selects exactly
  the absent member destination of this checkout's own committed, verified root
  — same root, nonce and volume, its marker/kind/volume/generation/device bound
  — the binding creates that one member directory under the same writer custody
  a member link's publication uses, and never relinks, replaces or follows
  anything. A link naming a missing root, another checkout's root, an
  unverified directory or a different nonce still refuses with
  `oxide outputs adopt` named, and every other command keeps the refusal.
  A debug-only stop after the fixed terminal image is published (before
  settlement) exercises exact-image replay.
- **Adding a member to a root that already exists.** The transition that
  created a root consumed its reservation, so a later member of the same root
  (a host `--target` triple after `debug`, say) needs none: the keyed root must
  still be one the receipt currently selects, with the same kind, volume,
  nonce and the root's own immutable creation generation, and must pass the
  full marker and device check. A placement keys the root's creation
  generation, never the placement revision that committed it. A nested
  `<triple>` member's real ancestor directory inside the checkout's `target` is
  created exclusively before its link is published; a symlink, a file or any
  other entry there refuses, and nothing outside that target is created. Every
  publication — a per-member transition, the explicit adoption, the automatic
  first conversion and the whole-target migration — creates the member's real
  directory inside the verified root before the member's link can name it, so
  a committed placement never points at an absent directory; a symlink, a file
  or any other entry at that destination refuses and is never followed
  (oxide#162).
- **A deleted `target` directory (oxide#165).** The profile layout keeps
  `<checkout>/target` itself a real directory that holds the owned member
  links, so a checkout whose `target` was deleted — a manual `rm -rf target`,
  a task-local build-output cleanup — is a positively absent target and
  nothing more. A leased command with a committed profile receipt recreates
  that one real directory with a single exclusive `mkdir` under the checkout's
  own directory descriptor, at the mode a conversion creates (0755), and then
  republishes the missing owned member links this command places — its own
  declared members, and for a derived `oxide exec` every member the receipt
  records — through the unchanged per-member transition into the same root the
  receipt commits to: the root, its marker, its nonce and its payload are
  untouched and the receipt only advances its generation. The create runs under the checkout's binding mutex after the
  same writer-custody clearance as a relink, so no cooperative command can
  race it; an entry that appears meanwhile is never replaced or followed —
  only a real directory is accepted as that creator's result, and a file or
  any link refuses. An existing `target` that is a file or a link of any kind
  refuses the profile binding (this layout requires the real directory there),
  an unreadable parent or unknown state refuses, a whole-target receipt and a
  never-converted checkout are unchanged, and no other refusal is relaxed.
  `oxide exec` and an external subcommand's own builds bind through the same
  path and get the same recreation.
- **A task-local cleanup that meets an interrupted transition (oxide#236).**
  A write-ahead transition leaves exactly one prepared placement, and the
  next permanent-mode leased command replays it under the checkout binding
  mutex and the cross-pool writer-custody gate before Cargo runs — so a
  task-local build-output cleanup that removed the whole `target` directory
  (with the member links and the prepared transition's sibling link) does
  not block the continuation. The replay reads a positively absent sibling
  and the receipt's old link state alone: nothing published rolls the
  preparation back to the committed receipt (or detached when nothing was
  committed yet) and creates nothing; a live sibling or a finished rename
  still commits under the unchanged verified-root, nonce, identity and Git
  checks; a foreign or changed member path, an unreadable component or an
  unknown writer stops with the receipt bytes, the live entries and both
  roots kept exactly as they are. Inspect with `oxide outputs status`;
  `oxide outputs adopt` can replay only after the refusal is resolved. After the rollback the binding recreates the real
  target directory (the deleted-target rule above) and republishes the
  member links into the same verified root, advancing the receipt generation
  without moving, adopting or deleting anything. The surfaces that cannot
  replay at their check point keep their refusals with the explicit recovery
  named: `oxide exec` and external subcommands refuse a prepared receipt
  before the slot is keyed (a mid-transition receipt must never key a
  slot), `oxide clean` refuses before cleaning, ephemeral-drive-mode
  binding refuses and names the permanent-registration prerequisite (ephemeral
  mode never transitions automatically), a
  guarded one-use workspace refuses with the exact guarded diagnostic
  ([one-use workspaces §4.4](one-use-workspaces.md)), and unknown custody
  refuses with everything kept. `oxide outputs adopt` in permanent mode
  replays the same receipt under its own barrier and is the documented
  recovery for those surfaces once a permanent registration is available;
  `oxide pool set-mode permanent` changes an existing registration.
  `oxide outputs status` and `outputs plan`
  report a pending transition as state, never as a refusal to recover.
- **Members.** V1 knows exactly `debug`, `release`, `<triple>/debug` and
  `<triple>/release`. A member has at most two components matching
  `[A-Za-z0-9_.-]+`, is neither `.` nor `..`, and is never absolute or an
  escape; members are unique and prefix-free, at most eight per command and 32
  per receipt. Existing real nonempty member directories are never moved or
  converted (they are used locally with a notice), a real empty member
  directory converts only under an explicit `oxide outputs adopt`, and a
  foreign link refuses. Every first link also checks the actual Git
  ignore/tracked state of `target/<member>`; a winning parent `target/` ignore
  is allowed, unknown answers refuse.
  For a receiptless, nonempty real target, `oxide outputs plan` reports the
  explicit profile adoption member by member: nonempty real directories stay
  local, missing or empty members convert, and a foreign member refuses by
  name. A later refusal can follow an earlier member's conversion; the plan
  itself writes nothing and never promises an atomic adoption. If both host
  members are nonempty and real, adoption creates no link and refuses because
  it commits no root; the plan reports that outcome.
- **Classification and declarations.** `check`, `build`, `run`, `test`,
  `clippy`, `rustc` and `fix` with `quick`/`debug` map to `debug`, `--release`
  to `release`. Plain `bench` maps to `release` (Cargo's release-based bench
  profile) while `bench --profile quick|debug` follows `dev` and maps to
  `debug`. `doc`/`rustdoc` compile the selected profile's units and are
  classified the same way, while `target/doc` stays a real tool directory.
  Every `--target`/`--target=` before `--` is parsed, deduplicated and expands
  the selected member; more than eight total refuses. Only plain triples are
  modelled: a JSON target specification, `CARGO_BUILD_TARGET`, `--config
  build.target` and a configured `build.target` are unknown, and the existing
  refusal for a modelled `--profile` other than quick/debug/release is
  unchanged. `oxide exec --output <member>` is repeatable and COMPLETE: the
  caller declares the tool's entire profile-output set, only the four member
  forms are accepted, the same validation and eight-total cap apply, and the
  tool's argv after `--` is untouched. It is OPTIONAL (oxide#160, S1): an
  `exec` that declares nothing derives its member set from this checkout's
  committed receipt instead — exactly the receipt's recorded placements,
  rendered with the same `outputs-3` lines, so the derived set and an equal
  declaration share one slot key
  ([design](exec-output-inference.md)). The derived path is read-only and
  fail-closed: the member-shaped paths under the real `target` are enumerated
  with the shipped `migration_members` traversal (member names first, so a
  real `target/debug` payload is never listed; `file_type()` and `read_link`
  only, so no link target is resolved or descended), a receipt-selected
  member must be this checkout's owned link at the recorded destination — its
  one repairable state is a link whose destination directory is absent beneath
  the checkout's own committed, verified root (same root, nonce and volume),
  which the permanent-mode binding creates under the same writer custody as a
  member link's publication; a missing member link, a real directory, another
  destination, a foreign link, an unverified or missing root and a different
  nonce all still refuse with `oxide outputs adopt` named — a receipt-recorded
  real member stays `real` with the existing notice, a link or file the receipt
  does not place refuses, and a checkout placing more than eight members
  refuses naming `oxide outputs detach`. The derived path itself reserves
  nothing when its set is non-empty, relinks nothing, converts nothing and
  creates no member; only that one absent member directory is created later,
  inside binding. A derived set that is EMPTY (whole-target, detached,
  unconverted) names no placement, and a whole-target plan there can name a
  FUTURE root — the first local fallback to an absent root — which binding
  accepts only through the exact reservation its slot was keyed with, so the
  reservation skip is scoped to a non-empty derived set
  (`outputs::needs_reservation`) and an empty one keeps the shipped
  reservation path (oxide#117, oxide#164). An external subcommand
  (`oxide nextest`, `oxide llvm-cov`, …) takes this **same** derivation through
  the same function (oxide#164): it is Cargo running `cargo-<name>`, exactly what
  `oxide exec -- cargo <name>` runs, so it gets the receipt-keyed member set, the
  #162 absent-member handling, the one bounded notice for a receipt-recorded
  real member, the reservation rule above and the same refusals, while its argv
  stays Cargo's. A tool's own directory that is not a member
  (`target/nextest`) stays a real local directory and is reported once, after
  the writer's group closes, by the bounded `tool directories kept local`
  notice: two listings of ONE directory (the real target's direct entries before
  the writer starts and after its group closes), capped at eight names, with no
  recursion, no payload walk and no link followed, and only on the
  receipt-derived profile-member layout — a never-converted, ephemeral,
  whole-target or detached checkout, a modelled command and an explicit
  `--output` declaration keep their previous stderr. A detached profile receipt
  plans no
  destination at all, so its `exec` runs locally through real directories; a
  legacy whole-target receipt and an unconverted checkout are unchanged (the
  declaration is not consulted), and a prepared, pruning or pruned receipt
  refuses. A tool that supplies its own `--target` needs the matching
  `<triple>/…` declaration in that project's selector; this task changes no
  consumer selector.
- **Custody, keys and binding.** Every create, relink, fallback return,
  detach, clean, migration and replay waits for all checkout writers under the
  existing barrier, binding mutex and positive cross-pool closure; an unknown
  writer is never downgraded to clear. The order is: classify; durably reserve
  a root nonce if needed; plan the sorted destination set; resolve the slot
  key; release pre-lease custody locks; acquire the lease; then under binding
  validate the exact surviving reservation, layout and every keyed destination
  while excluding only this command's own record. Linked profile keys render
  one `outputs-3 <nonce> <kind> <root> <volume> g<generation> <member>` line
  per owned member and `real <member>` for a real directory, so a linked and a
  real member never share a warm slot; legacy whole-target keys stay exactly
  `outputs-2`. Adding a member to a root preserves the root's nonce; a
  reservation is root-level: registry publication recognizes consumption by
  any actual selected or prepared placement, including a later sorted member
  while the first member keeps an older root (oxide#277). Root history alone
  never excuses a missing cancellation journal. The existing root identity
  and nonce checks remain at binding and commit. A reservation may not be
  cancelled while another member's
  consumer owns it. A slot KEYED for a future root binds only the exact
  uncancelled reservation it named (receipt, keyed nonce and registry row): a
  cancelled, replaced or absent one refuses inside binding before the prepared
  receipt, the root, the link or any registry write, and only an explicit
  adoption with no keyed slot may draw a fresh nonce (oxide#152). A command
  whose slot was keyed before the layout changed refuses before Cargo without an
  automatic retry.
- **Explicit profile adoption.** `oxide outputs adopt` on a checkout with no
  receipt selects the profile layout: it creates the real `target`, links the
  host `debug`/`release` pair (and every member the receipt already places)
  into one root under the member ignore check, keeps a real nonempty member
  with a notice, is idempotent, and refuses a legacy whole-target link
  untouched. `oxide outputs detach` restores real empty member directories for
  every owned member link — the plain `debug`/`release` pair and each
  `<triple>/debug` or `<triple>/release` member alike — keeps every root,
  marker and payload byte, and leaves the receipt detached with its placements
  as re-adoption authority; an unowned member link refuses the whole detach
  before any link is touched.
- **Explicit migration and marker upgrade.** `oxide outputs adopt` on a
  committed whole-target receipt is the only path that rewrites a legacy
  placement. A committed `oxide.outputs.transition/1` receipt is decoded into
  that whole-target form through the same parser and checkout identity the
  prune readers use (oxide#152) before the transaction starts (oxide#163); a
  noncommitted version-1 document, and one carrying a successor-only surface,
  an unreadable identity or an unknown field, refuses with nothing changed.
  When the committed root's owned whole-target link is not available — a lost
  or detached volume, or a link that is missing — explicit adopt takes the same
  whole-target relink path a `/2` receipt takes, and the layout migration runs
  on the next explicit adopt once the owned link is back. It is macOS-only
  (Linux remains compile-only), runs under the
  clean barrier, binding mutex and publication custody with all writers
  closed, and uses exclusive renames; no payload is moved or copied. P0
  publishes a durable prepared journal pinning the prior receipt digest, the
  exact target-link text/dev/ino, the parent identity and — for a root/1
  marker — the marker's dev/ino/SHA plus ONE freshly reserved root nonce. The
  journal is published in the successor format, so a version-1 upgrade that
  stops at any phase is replayed by the next explicit adopt exactly like any
  other migration and no version-1 document survives P0. P1
  re-verifies the root (this checkout's marker, kind, volume, live device and
  ancestry, and either the root/2 nonce or the exact pinned root/1 document),
  creates every classified member's real directory inside that root — refusing
  a symlink, a file or any other entry at a destination and creating nothing
  outside the root — and only then creates the transaction's staging directory
  with the member links into that root, recording its identity and the member
  directories it had to create. No published link ever names an absent member:
  the observed defect (oxide#162) was a migration that linked the host pair
  into a root that had never held a release build, leaving `target/release`
  dangling. The classified set is the
  host `debug`/`release` pair plus, one bounded component down, the `debug` and
  `release` directories of each positively identified plain triple directory
  in the old root; a linked triple directory, a JSON target specification, a
  custom profile, a third component and every other entry are never
  classified, and each `<triple>` becomes a real ancestor directory in the new
  target holding only its own owned member links. The root/1 marker is
  exclusively renamed to a transaction backup and the root/2 successor is
  published without replacement, preserving owner, recipe, kind, volume,
  the original root-creation generation and its original creation timestamp,
  so only the marker version and the reserved nonce change. P2 exclusively renames the old target
  link to its recorded backup and P3 exclusively publishes the staging
  directory as the real target; only the staging INODE recorded at P1 is ever
  published, so a directory replaced in between with identically shaped links
  is refused or left pending, and a creator of `target` is preserved and leaves
  the transaction pending. The COMMIT is reachable only through that same
  staged inode and requires the pinned pre-P4 backup of the old whole-target
  link (and, for a root/1 upgrade, the pinned old-marker backup) to still be
  present with its recorded inode and bytes; a lookalike target directory, a
  missing or substituted backup leaves the transaction pending instead of
  committing. P4 verifies the final real directory and exact
  member link set, re-establishes every classified member directory under the
  verified marker (so no committed placement can dangle) and commits the
  receipt, root/2 nonce and outputs-3 member references — the COMMIT point,
  never rolled back. Journaled backups are
  removed only under their pinned identities before the journal is dropped.
  Replay advances only exact recorded identities, including a crash before a
  phase record; ambiguous identity, a foreign creator, a lost volume, a
  missing required backup or an unrecognised phase stays pending. For a root/1
  upgrade the pinned old-marker backup must be POSITIVELY present under the
  recorded root with its exact inode and bytes before any pre-P4 target rename
  or staging publication, not merely recorded as a pathname, and a replaced
  backup with the same bytes but a new inode is refused. A pre-P4
  rollback restores the exact old marker from its pinned backup, the old
  target link and the published /2 receipt (the decoded whole-target form of a
  version-1 receipt, in that upgrade), removes exactly the member
  directories the journal recorded as created (and only while each is still
  the real, empty directory the migration made; one holding content or a link
  a creator put there is kept), and retires the reserved nonce;
  every rollback effect verifies the recorded parent first and removes only
  the transaction's own staged inode. The debug-only `OXIDE_TEST_MIGRATION_ROLLBACK` hook and
  `OXIDE_TEST_OUTPUTS_CRASH=migration-<phase>` crashes exist only in debug
  builds.
- **Formats and upgrade.** State version 4 carries the successor fence
  `owned_outputs: "oxide.outputs/3"`; receipts are
  `oxide.outputs.transition/3` (layout profile, one placement per member,
  root-level reservation), the registry is `oxide.outputs.registry/2` schema 2
  with one reference per member, and the plan is `oxide.outputs.plan/2`. The
  published `/2` receipt shape, whole-target link, `outputs-2` destination
  key and in-placement reservation are never redefined; a legacy registry
  reference keeps member `target` and a schema-1 document reads unchanged.
  Root/2 marker shape and existing root nonces are unchanged. The fence is
  published internal-first, then in the paired removable control, only after
  complete bounded authority reads and positive writer closure across both
  pools; a missing, inaccessible, corrupt or sticky-incomplete authority
  refuses and is never rebuilt. A `/2`-fenced pool keeps numeric state version
  3 so a state-3 binary still runs it; state 4 is forward-only after the
  successor fence. Status reports the layout and each member's live state, and
  each leased command on a legacy checkout prints ONE notice naming
  `oxide outputs adopt`. That explicit adoption upgrades a committed version-1
  whole-target receipt and publishes `/3` (oxide#163).
- **Multi-member prune and clean.** All verified members of one root under the
  same checkout identity form ONE owner; another checkout's reference keeps the
  root. A prune intent atomically marks every row of the (checkout, root)
  group and durably records the prior receipt SHA and the complete sorted group
  (member, generation, nonce, device/inode, marker digest and selection
  flags). Replay verifies that exact group and the receipt BEFORE recomputing
  each row's selection; a missing, corrupt or changed receipt, a missing or
  extra row, a contradictory identity or a pending reference keeps the root
  (pending/KEEP). The group is stored COMPLETE, never filtered to selected
  rows: the SELECTED subset is the checkout's current authority and must match
  the receipt's current placements for this root exactly (member, generation,
  nonce), while registry-only inactive history — for example a legacy `target`
  row kept after an explicit migration, with no nonce or an older marker digest
  — is separately pinned evidence that keeps its own recorded facts and prior
  role. History is never required to appear in the receipt, never invalidates
  the upgraded current members and never grants deletion authority. A complete
  group with ZERO selected rows is valid history for the existing
  released/Detached-root eligibility (an empty selected subset is not an empty
  group), and pruning a released root while the receipt currently selects a
  different root preserves that root and the receipt bytes unchanged. A
  rollback restores EACH row's own prior selection flag. The receipt's terminal
  image digest and its FIXED timestamp are durable before publication, and a
  replay accepts only the recorded preimage or that exact image. `RootIdentity.member` is only the first sorted
  representative, never the transaction scope. A whole clean plans the
  checkout target with every owned member link preserved and each owned member
  root's payload with its marker kept, never follows a generic link, and
  removes only its plan; a selected clean runs the pool watchdog and the
  watched leader/group-closure APIs, and the removable watchdog watches volume
  removal only — not a continuous free-space reserve.
- **Limits.** Migration, external roots and drive handling are macOS-only;
  Linux is compile-only. A profile-layout migration of a checkout whose root
  marker is root/1 reserves its new nonce in the migration journal (never via
  the unused-root cancellation journal). Automatic custom-profile linking,
  consumer adoption, live prune, host installation and release actions are
  outside this task.

The Supervisor's Builds diagnostic at `8acd4be` refused before any case because the candidate capability was newer than the proof's stale equality check (10 s, monitored peak 134.6 MiB; `wpo-logs/diag-8acd4be.log`). It is historical and gives no qualification credit. The correction accepts candidate capability at least 4, retains exact base capability 3, records observed values in mode receipts, and constructs the legacy migration fixture from a candidate-created root because current automatic adoption creates profile members. Fresh owner-activated diagnostic and native chains are required.

The Supervisor's Builds diagnostic at `99a30db` failed in step 11.2 with a 93-byte slot scratch path (50 s, 85.6 MiB peak, one failure and 17 dependent skips; receipt `oxide-proof.zowghq8j/evidence/profile-outputs.diagnostic-20261101T113247Z-67478.json`). It closed and remains historical with no native credit. Short unique fixture names and an effect-free 80-byte path bound correct the fixture; the failure now includes the command exit and bounded stderr. Both native chains require fresh owner activation at the reviewed head.

The Supervisor's Builds diagnostic at `cb2e399` reached case 05 and failed its stale external-help argv oracle: the fixture fake recorded `nextest --help`, because Oxide's help fast path forwards `cargo nextest --help` and drops `run`. The fake's marker establishes that the fixture tool ran; this is not evidence of host `cargo-nextest` execution. The receipt `oxide-proof.eze7owaj/evidence/profile-outputs.diagnostic-20261101T122653Z-4943.json` remains historical with no native credit. The oracle now reflects the current fast path; fresh owner-activated diagnostic and native chains remain required.

The Supervisor's Builds diagnostic at `75ccce2` failed case 05 and skipped 12 dependent cases; it is historical without native credit. The profile proof now exercises receipt-derived external commands through the owned debug/release links, forwards opaque custom arguments, and exercises derived `exec`; foreign-member refusal remains a separate control. Both native chains need fresh owner activation at the reviewed head.

The Supervisor's Builds diagnostic at `68429dd` failed only case 20: a checkout moved on the external volume cannot establish the boot-Data absence required for a prune candidate. The packet is historical without native credit. Decision 41307e95 moves that case and its live-reference keep into base-compat; both modes require fresh exact-head native chains.

The base-compat diagnostic at `a379a82` reached case 20 but failed its peer-keep reason: the fixture injected a whole-target `target` receipt beside copied profile-member registry rows. The product correctly kept the entire batch for a contradictory receipt reference; the earlier gone-root preview had succeeded before injection. This packet (`/private/tmp/oxide-proof.i6ezkq7n/evidence/profile-outputs.diagnostic-20261101T165554Z-88161.json`) is historical without native credit. The corrected fixture first asserts that whole-batch keep, then reconciles its own synthetic peer receipt and rows into matching profile members before testing the other-owner keep. This differs from oxide#217's fresh-pool registry-initialization change, which is not present at this head and concerns an adoption before a registry exists. Both native chains need fresh owner activation.

**Proof split (oxide#207).** The profile-output matrix is split into Builds-native and base-compat modes. Builds stages only the candidate and uses a private `OXIDE_SCRATCH_ROOT` under the selected supervisor root (600 s / 512 MiB); it covers cases 01–09, fixture-constructed root/1 case 11 and 12–18. A `/private/tmp` fallback does not prove external scratch-pool placement. The scoped base-compat selectors use the internal root under bd357a4c for the pinned base-created cases 10 and 11, case 19 forward-fence refusal and case 20 boot-Data checkout-absence proof (decision 41307e95; 300 s / 512 MiB under ea36e265). Fit remains unmeasured pending the separately activated diagnostic. Each mode has its own receipt and requires its own diagnostic, author and independent reviewer native at the reviewed head. The following describes the pre-split proof and its historical design, not a completed native at this head.

**Proof (oxide#133).** `scripts/test-profile-outputs.py` publishes the fixed
20-case matrix for this design, updated for the corrected distinct member
destinations (each linked member must select `root/<member>`), the positive
selected-removable and low-reserve controls, the honest top-level lifecycle and
the real peer-receipt KEEP, the two base-created legacy fixtures prepared under
one base-compatible registration before any candidate successor-fence effect,
and a case-11 registry predicate that separates the selected profile group from
retained legacy history (a history row without a nonce is non-selected history,
never a live reference), a real `run` control that executes the artifact of its
own linked member, and a specific other-owner KEEP reason in the peer-receipt
oracle; the fixture's own registry/receipt pairs agree member-for-member with
that rule; and a failure-custody control that settles only the gated children
this script created, through the existing bounded helpers, and keeps anything
unestablished unclosed and detach-blocking: one private session per side with
one owned
128 MiB scratch APFS image, a private `HOME`, `CARGO_HOME` and directory pool,
a separately staged base `4a70b83` binary and this HEAD's candidate staged
offline with 1.97.1 from authenticated pinned source, with asserted distinct
binary hashes and state-3/state-4 capability probes. It runs under the
qualified shared supervisor (`scripts/bounded-test.py`, SHA-256
`961164e797c01432fa8234497627f75e56489ef6395ef66e48824cdb0e48c052`, the
oxide#142 monitor qualified at main `791f552`; this proof was bound to its
predecessor `d89115f1…` before that qualification and those runs stay
historical) as
`bounded-test.py --root-parent /private/tmp --limit 600 --scratch-budget-mib
512 -- python3 scripts/test-profile-outputs.py`; its private fixture pools are
genuine internal directory-budget pools, which the product correctly refuses
on an external volume, so the recipe declares the owner-accepted internal root
(decision `bd357a4c`) inside one monitored owned root. The script prints its
own phase allocation and assertions with `--plan`. The root owner's scoped amendment
(2026-10-23T16:10Z) makes this proof 600 s TOTAL instead of 420 (the old 420 s
publication at 030836f6 is historical and unactivated), with a helper work
cutoff of about 565 s (its 30 s reserve plus a 5 s refusal margin) inside which
cleanup and image detach must finish; scratch stays 512 MiB and no runner or
split-proof change is granted. The published 360-540 s work estimate remains
UNMEASURED, not an established fit, and independent source-fit may still
refuse it. Its static disk allocation is 288 MiB staging peak + 128 MiB image
backing + 48 MiB fixtures + 32 MiB evidence + 16 MiB margin = 512 MiB. CPU controls support a 6 CPU / 8 GiB planning request: the two
independent stagings are sequential and each is explicitly bounded to 6 Cargo
jobs, fixture Cargo children are bounded to 2 jobs with at most two gated
writers overlapping, and the shared staging helper takes that bound explicitly
after sanitizing inherited `CARGO_BUILD_*` (its default is unchanged for other
selectors), so the proof peaks at 6 concurrent Cargo jobs. Its private 128 MiB
APFS fixture image is attached with `hdiutil attach -owners on` on every attach
and reattach so the mounted root does not ignore filesystem ownership, and the
prerequisite is asserted by a bounded read-only mount observation plus the
product's own registration refusal; no real drive, persistent host ownership
setting, mount permission or registration safety check is changed. The image
backing lives in the qualified guard's own evidence directory beside (not
inside) its monitored scratch, so the same guard root counts it within the
512 MiB allocation; an ordinary detach is used on success and after a failure,
and a failed detach fails the proof and retains the backing with its
device/UUID/image identity for the custodian. The script is fail-closed on
arguments (`--plan` and `--self-check` are effect-free, anything else refuses). The 128 MiB
image is registered with a 16 MiB reserve and a 64 MiB admission cap, a positive
control proves an external root is really selected before any external-root
oracle, and a separate deliberate low-reserve control temporarily raises the
private pool's recorded registration reserve to 200 MiB and restores its exact
bytes to prove the honest local fallback; a `/target/` parent-ignore control
proves a directory-only rule refuses the legacy link but accepts profile members;
a benign ordinary target-root file on its own checkout makes the top-level
preservation control nonvacuous (a clean of that checkout must refuse the file
as foreign and succeed after its removal), a real peer checkout receipt for the
same root proves another reference keeps it, and `CACHEDIR.TAG`/`.rustc_info.json`
are asserted real only when present.

**Evidence and acceptance limits, 2026-10-24.** Tom explicitly accepted source
`a5020b5d93e3c6d658d2660ab9c0fcb8a7fa641f` as-is after the ninth counted native
failed; this is operator acceptance, not a native or independent review PASS.
Independent exact-head source/fit reviews passed. The two author runs and all
nine counted native attempts remain FAIL with zero PASS; the single uncounted
diagnostic is spent and is not qualification evidence. Counted attempt 9 ran
the default proof under guard `961164e7` with the explicit direct physical-host
exception and quiet-host waiver. It completed 14 distinct cases (11 and 01–14,
with 11 counted once), then failed case 15 when detach rejected the host-triple
member link. Cases 16–20 remain unexecuted. The guard reported closed, status 1,
no fault or deadline, certain empty census, and 97.8 MiB monitored whole-root
peak within 512 MiB; raw EPERM signal results remain in its receipt. Closure
does not turn the assertion failure into a PASS. The helper and guard receipts
are distinct. Tom's final acceptance supersedes the earlier conditional
full-matrix requirement; it does not report that condition fulfilled. No further
native is authorized for this accepted task. The target-member detach follow-up
starts from that failure packet and must exercise cases 15–20, without inheriting
a proof allowance or claiming they already passed. Historical UNCLOSED runs and
retained unknown backing stay separate from this closed failure.

**Source correction, oxide#144.** Two shipped defects were found behind that
case-15 refusal and corrected in the source phase only (native cases 15–20
remain unactivated for this task). First, the one `target`-relative ownership
classifier derived a member link's candidate root from only the shallow
`<root>/<profile>` shape, so a real `target/<triple>/debug` link — created
correctly by a host `--target` build — was reported foreign and detached
refused it. Ownership now considers both the `<root>/<profile>` and
`<root>/<triple>/<profile>` shapes, most specific first, and still requires
this checkout's marker at the candidate root (a wrong destination, an unmarked
or another checkout's root, a missing destination or a malformed relative link
all keep the fail-safe refusal with nothing changed). Second, re-adoption after
a multi-member detach could not write its per-member write-ahead receipt: the
receipt consistency rule demanded every profile placement be selected, while a
detach deliberately leaves them all unselected. A profile receipt now requires
at least one selected placement and allows the others unselected (a real local
member, or one awaiting re-adoption); a whole-target receipt keeps the strict
rule. The fixed proof's case-15 oracles were strengthened, not weakened: they
now assert the pre-detach state (the host-triple link created by case 04's real
build), that the first detach converts it to a real directory, and that
re-adoption restores it — the exact accepted failure point. A pure/static
premise sweep maps each case-15 to case-20 premise (and the private-`HOME`,
automatic-adoption and Cargo-argv premises) to shipped source at this head. The
uncounted diagnostic mode now exists and has run twice (see the evidence limit
above): diagnostic 1 exposed the case-16 auxiliary-pool premise and diagnostic 2
the case-18 missing post-adopt warm repeat, both corrected in the proof source
rather than the product. Its slot-tree oracles resolve the numeric slot ID the
build summary names at the owned depth — `slots/<label>-<short-id>/<profile>-<id>`
— and compare `(path, dev, ino)` snapshots there, so a slot removed, re-created
or replaced under an existing project folder is detected while a listing of
project folder names would be vacuous; the mapping refuses symlinks, foreign
child names, duplicate IDs and unknown topology instead of guessing.

The script also has an explicitly opt-in `--diagnostic` collection mode. It
records recoverable oracle failures and skips dependent or tainted premises,
while unknown device/writer effects and infrastructure errors abort. Its bounded
receipt is marked diagnostic and unqualified; it never supplies counted PASS
or closure credit. The default counted mode remains fail-fast. Its case-16
watched clean runs on the internal `directory-budget-v1` store under the pool's
OWN recorded permanent registration: because drive mode comes only from that
registration, and a second registration on the one owned image is impossible by
production rule (a sub-directory is not `diskutil`'s mount point, the shared
`<mount>/oxide-pool` payload is not empty, and `pool set-mode` refuses an absent
registration), the fixture reuses the existing low-reserve pattern to raise the
recorded reserve above the image's free space. Selection then falls back to the
directory-budget store, whose watchdog is the reserve watchdog, while permanent
mode still holds; the exact recorded bytes are restored before cases 17-20. Its
oracles assert those prerequisites instead of assuming them, and a pure
counterfactual control exercises the unregistered-refusal, forced-directory-
budget and impossible-auxiliary-registration cases. The mode is
routed by its own heavy selector, `effigy rust:profile-outputs:diagnostic`
(the counted command below with `--diagnostic` appended), so the counted
selector keeps its unchanged fail-fast command; both share the same
supervisor, limit, scratch bound and activation holds.

**Proof (oxide#121).** `effigy rust:auto-adopt-outputs` runs
`scripts/test-permanent-outputs.py auto-adopt`, the focused mode of the same
fixture, as the whole command of the qualified shared supervisor (300 s,
512 MiB monitored scratch, ordinary detach) over one owned 128 MiB scratch
image. It covers eligible fresh adoption with one notice, a warm rebuild and
a second checkout's own root; the controls (non-empty, empty, foreign,
dangling and file targets; a corrupt receipt; a detached checkout with its
target missing; ephemeral mode; an absent drive; a recorded-UUID fallback);
crash replay after the sibling point, where a creator at the target is
refused and kept, and after the prepared point, which rolls back to detached;
and a committed adoption that survives a failed build. The no-replace publish
is a macOS unit test in `effigy test rust`. Its fixtures ignore `/target`,
which covers the member links a first conversion publishes. Not a benchmark,
real-drive qualification or an alias for full QA.

**Proof (oxide#125).** `effigy rust:outputs-ignore-safety` runs
`scripts/test-permanent-outputs.py ignore-safety`, the focused ignore-safety
mode of the same fixture, as the whole command of the qualified shared
supervisor (300 s, 512 MiB monitored scratch, ordinary detach) over one owned
128 MiB scratch image. It covers, for explicit adopt, automatic adoption and
prepared first-conversion replay, the member links a conversion publishes
(`target/debug`, `target/release`): `/target/`, `target/`, `**/target/`,
`/target` and `target` all publish, because a directory-only rule genuinely
covers a member path inside the real target directory (oxide#160), and a
nested workspace's `/rust/target/` publishes while an unignored nested
checkout refuses. A rule that re-includes a member, an unignored path, a
tracked member, no repository, no Git on `PATH`, a Git that does not answer in
time, a failing Git call and a malformed verbose answer all refuse with the
target, receipt and drive roots unchanged (not even the real target directory
is created). An automatic conversion Git cannot confirm is ineligibility
rather than a refusal (oxide#154), so those cases assert an unconverted build
with one notice instead. Accepted publications are invisible to `git status`
and `git ls-files`, and `detach` still restores a real directory. Not a
benchmark, real-drive qualification or an alias for full QA. The native run is
held for owner activation; the guard's rule parsing and decisions are
unit-tested in `effigy test rust`.

**Proof (oxide#154).** `effigy rust:auto-adopt-ineligible` runs
`scripts/test-permanent-outputs.py auto-ineligible`, a focused mode of the
same fixture, as the whole command of the same qualified shared supervisor
(300 s, 512 MiB monitored scratch, ordinary detach) over one owned 128 MiB
scratch image attached with ownership enabled, with `--root-parent
/private/tmp` — the owner-accepted internal-placement exception (decision
bd357a4c), because the fixture needs a genuine internal directory-budget pool
that the product correctly refuses on an external volume; the supervisor
monitors that same root. It proves expected ineligibility leaves the build
usable: for no repository, no Git on `PATH`, an unignored path, a negated
rule, an already-tracked member and a Git that does not answer, the leased
build exits 0 into a real local `target/` whose `debug` member is a real
directory holding the built binary, prints exactly one unconverted notice and
no auto-adopted line, and writes no receipt, owned member link, owned root or
downgrade fence. Explicit adoption over a real empty target still refuses each
class with the target bytes, receipt and drive roots unchanged; a corrupt
receipt is still an error that creates no target; an automatic conversion
stopped after its prepared member receipt keeps that receipt and no member
link, refuses a replay within a broken rule, and recovers only through the
documented explicit adopt (which then publishes the committed profile
layout); an existing real target stays the per-checkout opt-out; and
`oxide init` names the rule that covers the member links for an unignored or
tracked path, stays silent for a covering rule (including a directory-only
one), and leaves every rule byte-exactly as it was. Each case uses its own
checkout, so one failure never taints another premise. Its `:diagnostic` twin
is the same fixed guard, mode and limits with `--diagnostic` appended: an
uncounted keep-going collection that records reached/completed/failed/
unexecuted cases and the latched first fault in a bounded `qualified=false`
receipt beside the guard's evidence, continues only into independent cases,
aborts on an unclosed child or an exhausted deadline, and makes one ordinary
detach of the run's owned image with the mount's absence recorded before that
receipt. Each native run needs its own owner activation. Not a benchmark,
real-drive qualification or an alias for full QA.

**Proof (oxide#124).** `effigy rust:outputs-detach-permanent` runs
`scripts/test-permanent-outputs.py detach-permanent`, the focused mode of the
same fixture, as the whole command of the qualified shared supervisor (300 s,
512 MiB monitored scratch, ordinary detach) over one owned 128 MiB scratch
image. It covers: a fresh checkout's first-build adoption; `detach` waiting
for an active gated build and then restoring a real empty target, with the
root's every entry hashed before and after unchanged, a recorded `detached`
receipt, and a plan reporting `state` detached, `current_target` `real-empty`
and no selected root; a second detach changing nothing; the next builds using
the real target with no relink or notice and no write into the detached root;
explicit adopt refusing the nonempty real target; a missing target beside a
detached receipt staying unadopted; explicit adopt re-converting an empty
detached target to the same root; and a foreign link refusing detach with the
receipt still committed, plus a detach with no receipt refusing. Unit tests
cover the detached receipt planning no root in permanent mode. Not a
benchmark, real-drive qualification or an alias for full QA.

### `oxide pool describe --json`

The stable, read-only interface other tools use to find the registered Builds drive. Oxide#112 implements it. The recorded fixed native proof passed at `ade4d51` under the qualified runner `d89115f1` (run `7bca5d80c0242754ec72b928a86f84b4`), including the `reserve_bytes` assertions. That source predates describe's registration-v2/mode integration and does not validate the integrated source. Earlier passes at `1f9607c1` (run `5711ac9a89310c08052f3929e9198a20`), `034f9ac4` under the qualified #109 runner (run `580419035311c3a1ee3061c0a7a95ed6`), and `89aa9b2e` remain historical; the last two predate the host commit check that the [shared seeder](#bounded-offline-lock-seeder) later removed, and their evidence stays historical. Output on stdout, schema version 1:

```json
{
  "schema": "oxide.pool.describe/1",
  "observed_at": "2026-10-21T16:00:00Z",
  "internal": { "pool": "/Users/tom/Dev/oxide" },
  "selected": "removable",
  "removable": {
    "mount_point": "/Volumes/Builds",
    "volume_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
    "mode": "ephemeral",
    "reserve_bytes": 21474836480,
    "available": true,
    "pending_reconciliation_slots": 0,
    "pending_reconciliation_slot_ids": [],
    "unavailable_reason": null,
    "payload": "/Volumes/Builds/oxide-pool",
    "suggested_scratch_root": "/Volumes/Builds/scratch"
  }
}
```

- **`removable`** is `null` when no drive is registered, and `selected` is then `"internal"`. When the registration file exists but cannot be read, `removable` is an object whose `mount_point`, `volume_uuid`, `payload` and `suggested_scratch_root` are `null`, with `available: false` and reason `"metadata_unknown"`. Nothing identifies the drive then, so no identity is invented.
- **`mode`** comes from the same version-aware registration parser as `oxide status`: legacy version 1 without a mode means `"ephemeral"`; version 2 reports its registered `"ephemeral"` or `"permanent"` mode, even when the volume is unavailable. Reads never migrate the registration. An unreadable registration retains the schema-1 `"ephemeral"` placeholder; it does not establish the registered mode, and `available` is false.
- **`available`** is **passive eligibility**, not a prediction that the next build will open or be admitted to the pool. It is `true` only when every check passes, using read-only observations:
  - the mount point holds the registered volume's root: `facts.mounted_on` is the path and the UUID matches, so a stale directory on the internal disk never qualifies;
  - the `Binding` opens with the registered UUID, which applies the `/Volumes` mount-parent rule, and the volume is writable;
  - `diskutil` reports the registered container, outside the boot/data container, with no APFS role;
  - available space is above the registered reserve;
  - the payload directory exists;
  - the removable pool's config and ledger read cleanly, and the config matches the registration (volume, UUID, payload directory and budget).

  Every `diskutil` or `plutil` call is bounded: 5 s each and a 4 MiB output cap. A timeout, oversize output or parse failure counts as unknown. An `Evicting` record in the ledger does not make the drive unavailable: a build's open reconciles it and selects the drive anyway, the same as `oxide`'s selection (`Store::open_removable`, which never refuses on it). `describe` reconciles nothing and reports the count in `pending_reconciliation_slots` instead (oxide#174; before that it was `available: false` with reason `pending_reconciliation`, which blocked consumers at the 128-slot ceiling).
- **`pending_reconciliation_slots`** is the number of ledger records in `Evicting` (deletions a build's open reconciles), added in schema 1 by oxide#174. It is advisory and never changes `available`: `0` when none is pending, and `null` when the config and ledger were not read (an unreadable registration, or an earlier check refused). A drive with pending records and every other check passing reports `available: true` with a count of `n`.
- **`pending_reconciliation_slot_ids`** lists those recorded slot IDs as strings, or is `null` when the count is unknown. It is advisory and does not change selection.
- **`unavailable_reason`** is `null` when available. Otherwise one of: `absent` (nothing mounted at the registered path), `identity_mismatch` (a different volume, container or budget than registered), `volume_unusable` (refused by a selection rule such as the mount parent, read-only, a role or the boot container), `below_reserve`, `metadata_unknown` (a probe failed, timed out, was over cap or could not be parsed), `control_missing` (no internal control directory, config or ledger), or `payload_missing`. `pending_reconciliation` was a reason value before oxide#174 and is no longer emitted; consumers that handled it still see only the values above. Consumers branch on `available`, not on reason strings.
- **`unavailable_detail`** is a human-readable explanation, added in schema 1. Its text is not stable.
- **`reserve_bytes`** is the registered reserve in bytes, added in schema 1, and `null` when the registration cannot be read. `available` is only a snapshot of free space above it. A writer rechecks the mount, the volume UUID and free space against this value at use time.
- **`selected`** is the passive preference: `"removable"` when `available` is true, otherwise `"internal"`. It is not a promise of the next build's selection, which stays authoritative inside the build and may differ, for example after its own reconciliation.
- **`suggested_scratch_root`** is always outside `payload`. It is a suggestion, not a promise that the directory exists or a grant of write authority. Consumers create their own subdirectory, for example `<root>/<tool>/<run-id>`, and never write under `payload`.
- **Read-only.** Running `describe` mutates nothing. It never uses the build path's pool openers (`Store::open_removable`, `Pool::open`), because those reconcile recorded deletions and create control directories. It reads the removable pool's config and ledger directly (`managed::describe_volume`): no lock, no control directory, no ledger write, no accounting walk. Held records are reported as they are, with no claim about closure or accounting.
- **A snapshot.** The data is a point-in-time observation. Before writing, consumers still apply their own gate: the volume UUID, the mount root, and a device different from both `/` and `/System/Volumes/Data`.
- **Exit codes:** 0 whenever the description was produced, whether or not a drive is registered or available. Non-zero only when Oxide itself fails (for example, settings cannot be read), with the reason on stderr.
- **Compatibility:** fields may be added in schema 1. Removing or renaming a field needs `oxide.pool.describe/2`.
- **Proof.** Library tests read a private config and ledger fixture, including a pending record, and check the directory tree is unchanged afterwards. Refusals (other backends, mismatched store IDs, missing files) leave the tree unchanged. Unit tests use the existing thread-local permission-denied seam on both control files to verify that non-missing metadata failures propagate as I/O errors, rather than missing files, without changing the fixture. CLI unit tests cover legacy v1 and both v2 modes, malformed and unsupported registrations, byte/metadata/directory nonmutation, the RFC 3339 timestamp, null and unknown-registration JSON with escaping, reason tags, bounded reads and their output cap. Not yet proven: a real or tiny APFS volume. The fixed proof `rust:pool-describe` (`scripts/test-pool-describe.py`) passed at `ade4d51` (run `7bca5d80c0242754ec72b928a86f84b4`), and earlier at `1f9607c1` and `034f9ac4`. It covers four CLI case meanings, each with nonmutation: no registration, an unknown registration, an absent volume, and the usage refusal without `--json`. The absent-volume fixture now checks legacy v1 plus v2 ephemeral and permanent; the recorded native passes predate that extension. It runs inside the qualified shared runner (#109, `scripts/bounded-test.py` SHA-256 `d89115f1…`) with a 300 s limit and a 512 MiB monitored budget. Offline staging does not copy the host registry whole. The seed is selected from HEAD's `Cargo.lock` by the [shared seeder](#bounded-offline-lock-seeder): one supported crates.io namespace matched on index and cache, and the one locked Longhorn Git source's bare database candidates, bounded by the same 256 MiB aggregate. No Git runs on a host repository. Candidate selection only measures bytes; each candidate that fits is copied first through anchored no-follow descriptors with its host config and hooks skipped, a known private config generated and alternates refused, and only then must its private copy resolve the locked commit. A copied candidate that does not carry the commit is rejected and stays charged, an oversized one is not attempted, and a refusal keeps the per-candidate record. After staging, exactly one checkout at the locked rev must exist, with exactly the full locked HEAD; Cargo chooses its identifier. The seeder's record and hash are in the script's receipt, and the recorded native passes predate this migration. It does not cover `available: true`, the volume-dependent refusals, pending reconciliation through the CLI, or live diskutil bounds. Those gaps are listed in the script.

## Pruning owned output roots (oxide#129)

The focused [integrated delegation apparatus](barge-adoption.md#integrated-delegation-proof-readiness-oxide304)
uses the ordinary qualified supervisor and separately activated 300 s / 512 MiB
native selectors. The current plan explicitly refuses execution for the pinned
adapter's missing deciding checkout-volume absence support. Its effect-free
self-check and input plan grant no native,
installation or live migration credit.

The approved Barge mechanism-only extension (Tom `63ad6fc4` v2 and explicit
recovery-format ruling `60a85553` v2) is implemented behind the explicit
`oxide outputs upgrade-recovery` transition to paired `/4` and registry3.
Its [owning protocol](barge-adoption.md#connected-caller-and-recovery-source-oxide304)
keeps Oxide eligibility, exclusion and group/receipt settlement while Barge
alone deletes covered roots. Immutable request metadata and a mandatory
route survive crashes; unavailable/unknown keeps pending, with no local
fallback. Historical formats and uncovered classes retain the local protocol
below. Source implementation and covering do not establish integrated fixture
qualification, production-account lookup, installation or live adoption;
the separately activated Elevated proof sequence and merge hold remain.


**Bounded-batch qualification (oxide#256, 2026-11-03).** Source `afa28a9d`
passed the pure dispatcher controls, the uncounted diagnostic and the counted
author and distinct reviewer sessions: all 22 native cases completed, with
closed guards, certain empty censuses, no fault or deadline, and the owned
images ordinarily detached. Review `6599808998` and the retained
`planner-storage-reclamation-20261103/256-*` packets bind the evidence. The
reached proof script (`91442176…`), qualified guard (`fcfda2b0…`), Rust source,
lock and toolchain are unchanged at merged `6dcf0140`. The receipt-matched
installation `20261103T092325Z-6dcf01405018.json` records binary `4e5d52c4…`
with unchanged control-state 9 and registration 2.

The first fresh preview on that installation selected 322 of 569 eligible
roots in 262,088 bytes; 247 were deferred. Authority coverage was complete,
while byte accounting was incomplete for 107 selected roots. These are distinct:
measurement completeness does not select or authorize roots, and the observed
byte totals promise no physical reclaim. Real deletion still requires the
exact plan and all fresh product checks below.

`oxide outputs prune` is the explicit, one-shot removal of owned output roots whose checkout is positively gone. It is not part of any automatic admission or cleanup path, it owns no timer, hook or mode switch, and a normal build never prunes. The owner inventory context (52 roots, 42 GB logical, 36 checkout-gone, 21.5 GiB, 16 live) is not an allowlist, a proof, or an expected reclaimed figure: logical and allocated bytes are separate observations with coverage, source and time, and hard links or shared extents mean neither is guaranteed physical reclaim.

- **Host automation (Tom, 2026-10-26).** Oxide itself still owns no timer. On Tom's machine a host job outside Oxide (`com.tom.scratch-reaper`, daily at 04:30) runs `oxide outputs prune --dry-run` into a plan file. When the preview names at least one candidate, it runs exactly one real `--plan FILE --plan-sha256 <that preview's SHA>`. It never passes an operator list, records any non-zero exit, and never retries; legacy roots that need an operator list stay for an explicit operator run. The first live run on 2026-10-26 (installed `069b7fa6`) deleted 114 roots and kept 276. When more eligible roots exist than one preview can name, that job still runs one bounded batch; the next authorized dry-run selects the next prefix of what remains.
- **Dry run first.** `oxide outputs prune --dry-run [--operator-list FILE]` writes one bounded canonical JSON preview to stdout (`oxide.outputs.prune-preview/1`, at most 256 KiB and 512 roots) and its SHA-256 to stderr. Stdout is exactly the bytes the digest covers (no trailing newline is added), so redirecting stdout yields the plan file. It mutates nothing: it never initializes the forward format, replays an intent or writes a file. A pool whose registry is not ready refuses with exit 2; `oxide outputs adopt` (which may find the checkout already converted) establishes the format first. **Bounded batches (oxide#256).** The 512-root and 256 KiB caps are unchanged. When the evaluated eligible set exceeds either bound, the dry run selects the longest prefix of that evaluated set, in evaluation order, that still serializes inside both caps, and writes that prefix as `candidates`. `coverage.candidate_roots` is the full eligible count; `totals.candidates` is this batch; their difference is the deferred remainder, which stays eligible for a later authorized batch and is never treated as deleted. `coverage.kept_roots` and `totals.kept` are the full ineligible count. `kept` may name deferred eligible roots: a summary count row and, while they still fit, per-root rows with the deferred remainder reason. Those diagnostic rows stay capped at 512 and may shrink further so the document, including kept diagnostics, stays inside 256 KiB; shrinking them never changes the reported eligible, selected or ineligible counts. `totals.logical_bytes`, `totals.allocated_bytes` and `totals.accounting_complete` describe only this selected batch. `coverage.complete` remains the authority-snapshot flag, not a claim that `candidates` names every eligible root. A single candidate that cannot be represented in 256 KiB refuses the preview, naming that root; it is not skipped. Existing at-or-under-cap previews keep every eligible root.
- **Real run.** `oxide outputs prune --plan FILE --plan-sha256 HEX [--operator-list FILE]` requires the exact preview bytes and an unchanged operator-list byte digest. A missing or mismatched plan refuses before any effect (exit 2). The real run revalidates every predicate and applies the same deterministic selection, then requires the selected identity set (checkout, member, root, nonce, inode, marker digest) to match the plan one-to-one. Newly ineligible, changed, missing or extra identities in that selected set refuse the remaining batch; unselected eligible roots are not authorized by the plan. After exact intent replay of some planned roots, remaining plan identities must still be eligible and unchanged, and deferred roots are not backfilled into that already-authorized set. A failed fresh bounded preview after that replay is a partial completion (exit 0 listing the replayed roots), not a claim that nothing changed. The device number is an observation rather than part of that set: a plan made before a reboot or reattach still binds the same unchanged roots (see the device-renumbering rule below), and the fresh snapshot's own device is the one every later step of that run binds. Every just-before-effect ownership, custody, identity and absence recheck is unchanged.
- **Proof-authoring premises (oxide#256).** This source correction does not author or correct a native proof script, fixture or oracle. Native `rust:outputs-prune`, `rust:outputs-prune:diagnostic` and `proof:outputs-prune:test` remain owner-activated after exact-head source review; this author does not launch them. Checklist items 1–9 do not apply: no fixture child HOME/rustup, account-home control, inherited `OXIDE_*`/`XDG_*` scrub, automatic adoption, Cargo external argv, owned image, compiler flags, selected-volume TMPDIR recipe, or macOS mktemp template is introduced or changed. Source covering is compile, format, focused prune unit tests and docs check. A planner-bound diagnostic remains required before live use of the changed deletion path.
- **Exit codes.** `0` when the run completed, with the explicit deleted and kept lists; `2` for a usage error or any refusal before an effect; `3` when an intent is unsettled or contradictory, or when an operation's own proof/transaction is incomplete after a published effect, and needs `resolve-intent` or exact replay. A fresh per-root check that fails before that root's intent is journaled (the checkout reappeared, the barrier, binding, volume, reserve or marker check refused) keeps the root unchanged with its reason and the batch continues: the run exits `0` listing the exact partial completion when any root was deleted and no intent is unfinished, and `2` when every root was kept. Once a root's intent is journaled, any stop leaves it pending and exits `3`. A completed exact replay followed by a changed eligible set or a failed fresh snapshot also exits `0`, listing the replayed roots and keeping every other root. These commands do not use the front door's generic failure exit; signal handling is the existing policy and a timeout is never a success.
- **Device renumbering (oxide#216).** A recorded `st_dev` is one observation, not an identity: macOS assigns a mounted volume a different device number after a reboot or a reattach, so a registry row, plan or journal written before one still names the unchanged root. The root's identity is the verified volume UUID (through the binding and again on the owner marker), the inode, the root kind, the generation and the nonce, and the recorded marker digest; those are compared exactly and a difference in any of them still keeps the root. Reconciliation is limited to the device number, and only where every other fact matches: observation reports the live device instead of an unknown row, selected row groups may disagree about the recorded number while they are one owner, `RootSession` still requires a fresh candidate's own device (a difference there is a changed mount or root), the plan gate compares the non-device identity set, and a journal from an earlier session reopens its root by the recorded inode and the verified volume before comparing the owner marker. The intent publication refreshes the device recorded on every row of that root's group (and on its own journal group) to the run's observation in the same locked write, so a stale number heals there rather than spreading; a dry run still writes nothing. A swapped volume, a renumbered inode, a changed marker (nonce, generation or volume) or a changed digest is never reconciled, no device number is ever accepted as identity on its own, and no fact outside the device is relaxed anywhere.
- **Eligibility.** A root is only a candidate when the registry is complete and ready, the row has no pending, reservation or other live reference, and the checkout's validated receipt no longer references it as current (a released root: relinked away or detached; its receipt's own current selection is never rewritten when it is pruned), the checkout is positively absent under the protocol below, every writer is positively closed across both pools, the owner marker and nonce match the recorded identity, and the registered volume UUID and mount are verified. A registered checkout outside `/Volumes` also needs its recorded checkout volume, read through the held nearest existing ancestor's own descriptor: that ancestor must be on a local APFS or HFS volume with the recorded UUID, so a network, autofs or unknown file system, or a conflicting or unknown recorded volume, is kept. A present checkout (including a present detached one), a symlinked, non-directory or unreadable ancestor, an absent volume, an unknown writer, a marker-only legacy root, a history-only root or another selection keeps the root.
- **Current selection protects (oxide#279).** Tom's operator decision
  `dc1d6924-f196-4cde-8f2e-4c51c87ba65e` (2026-11-05) protects a root
  referenced by any validated current receipt member, even when its checkout
  is positively absent. The final exclusion runs after the original snapshot,
  root identity and all-member reference checks, so it cannot turn malformed,
  contradictory or unknown authority into a successful per-root classification.
  The locked candidate recheck applies the same exclusion after its existing
  checks and group validation. A legacy operator list does not waive current
  selection. This adds no write, deletion path, barrier or writer-closure rule.
  The [bounded comparison](barge-adoption.md#controlled-decision-comparison)
  preserves the pre-alignment disagreement and its evidence limits.


  **Absent checkout chains beneath a verified boot Data ancestor (oxide#135).** The recorded checkout path is validated before anything is walked: a relative path, a NUL, a `.` or `..` component, a repeated or trailing separator or any other non-exact absolute spelling is refused, never resolved or canonicalized. The walk then holds a descriptor for every observed component, no-follow, revalidates each named entry against the descriptor it opened (device and inode, then the named entry again), and KEEPS every descriptor of the chain it opened: at the first exact `ENOENT` it revalidates the whole named ancestry — each observed named entry, then each held descriptor — so no ancestor can be swapped between the walk and its decision. A verified mount spelling is re-verified through the same held binding after the walk, and the first exact `ENOENT` is the only positive answer. The actual leaf may be absent under a fully present ancestry. A missing **ancestor** chain is also provable, but only when the held nearest existing ancestor is a readable directory on the same device and APFS volume UUID as `/System/Volumes/Data`, read from held descriptors rather than from a reopened path; that extension never applies below a volume mount, and both `/Volumes/<name>` and its boot Data alias `/System/Volumes/Data/Volumes/<name>` (including an absent mount name) keep the existing leaf-only rule. A conflicting or unknown **recorded** checkout volume is never waived because today's surviving ancestor sits on the boot Data volume: whenever a row — or a receiptless root's owner marker, which records `checkout_volume_uuid` in the current shape — names a checkout volume, the held ancestor must be on it. The genuine legacy exception (an operator-listed row that predates the version-2 identity, and a version-1 marker without the field) adds the boot Data rule but never replaces a recorded volume, and only a row with no recorded volume at all may rely on that rule alone. The real run keeps an EPHEMERAL witness of that ancestry — the exact path, every observed component's identity, the start descriptor, the first missing component, the held ancestor facts and the held boot Data facts — and compares it again under the checkout's clean barrier and binding as an early refusal and, last, as the DECIDING comparison under publication custody immediately before the journal (for a receiptless root, before its pruning receipt). Publication custody is taken for every root, not only a receiptless one, and it is taken before that deciding comparison, so an ancestry that changed while this root waited for the lock cannot be published. A recreated checkout, a replaced ancestor, a changed first-missing component or a changed start descriptor refuses before any intent, receipt or quarantine effect, and a revision retry keeps the original witness instead of adopting a new baseline. No witness is persisted: the preview's bytes carry none, so a dry run's absence proof is never authority for an effect, and immutable ancestry from dry run through effect is explicitly not claimed. A fresh traversal that still positively proves absence is the contract, and the boundary is cooperative: Oxide's custody excludes cooperating bindings, not arbitrary filesystem creators, so this is repeated descriptor and entry revalidation, never atomic exclusion of a checkout or ancestor being recreated. On non-macOS there are no held volume facts, so neither an absent leaf nor a missing chain is ever proved and every root is kept. **Ordinary path ineligibility is per-root.** An unprovable checkout path keeps THAT root and leaves the snapshot complete, so a sibling root can still be a candidate in the same batch (a mixed batch proves both sides: a positively absent Data chain is pruned while a recorded-volume conflict and a symlinked ancestor beside it are kept, byte-identical). Incomplete required authority — a missing, malformed, contradictory or over-cap receipt or registry reference, or unknown writer custody — still refuses the whole batch. Any missing, malformed, unreadable or over-cap referenced receipt keeps the whole snapshot batch, not just that row. A checkout-local fallback below an absent checkout is recorded only; the prune never recreates a checkout.
- **Operator list.** `--operator-list` is a JSON array of absolute, duplicate-free root paths (at most 512 and 256 KiB). It waives only the receipt-history completeness for exactly those listed legacy roots; it is a distinct permission from volume identity and never replaces one. It still requires a well-formed owner marker, the actual absent checkout, closed writers, no current registry selection, a complete registry and custody snapshot, and the verified volume. A listed row that records a checkout volume must still be on it, whatever the row's age, and the boot Data rule is the genuine legacy exception's additional requirement rather than a replacement for it. A legacy recorded checkout below a volume mount is kept. Elsewhere the proof is volume-based, not a path prefix, and read from held descriptors: the nearest existing ancestor must be on the same device and APFS volume UUID as the boot Data volume (`/System/Volumes/Data`), and the no-follow walk must end in the checkout's own `ENOENT` (the missing-ancestor extension above applies); any other or unverifiable volume is kept. A moved-but-live checkout loses regenerable outputs by explicit operator choice; its next command safely re-adopts with a fresh nonce or refuses a missing root.
- **Quarantine and deletion.** Each root first gets a durable `oxide.outputs.prune-intent/1` journal entry with a fresh random 128-bit intent ID (the root's own nonce is separate and immutable). The verified root is then renamed exclusively inside the `oxide-outputs` tree to a `.prune-<intent>` leaf, deleted only through the held descriptor of the verified device. A resumed deletion opens the quarantine leaf only when its inode equals the recorded root's and it is on the verified volume — the recorded device is reconciled as above, because the exclusive rename keeps the inode while a reboot may have renumbered the volume — (no-follow traversal, links unlinked but never followed, a changed entry stops the batch), and the owner marker is unlinked last. The pass itself is the [capture-then-verify](#finder-metadata-in-pools) mechanism (oxide#155), one entry at a time: it observes the entry by no-follow `fstatat`, captures it with one exclusive rename to a private `.oxide-capture-` name, verifies the **captured** object's device, inode and type against that observation, and only then removes the private name — a file or link by `unlinkat`, a directory through a descriptor opened at the private name and revalidated by device and inode, its content deleted, its own recreated metadata drained, and the private name removed. An entry replaced after it was listed is therefore never removed or recursed into: the capture goes back under its original name and the intent stays pending, and a private capture name this pass holds no record for is kept rather than unlinked. A directory listing that ends in a `readdir` error is likewise never an ended directory: the pass stops the intent pending rather than deleting from the partial list it had collected. Every directory removal, the quarantine leaf's own removal and the check that the leaf gained nothing after the marker was unlinked use the [bounded Finder-metadata race](#finder-metadata-in-pools) (oxide#155): a tolerated metadata entry recreated in the window is verified and unlinked within the fixed bound, and every other new entry, an unverifiable one, a name that no longer names the verified leaf, a missing leaf and an exhausted bound all leave the intent pending (exit 3) with nothing cleared or rewritten. A nested directory that vanished inside the pass is finished work; the leaf's own absence is not positive deletion proof. The prune worker monitors itself (no Cargo group is assumed): the registered volume's identity and reserve and the host reserve of the internal control volume are checked at fixed points (before the rename, before the first and after the last deletion of every run or replay, every 512 deleted entries) and, once at least 1 s has elapsed since the previous check, before each directory enumeration, every 512 names while a directory is listed (at most 200,000 names per directory) and after each unlink; after each directory's names are sorted, and before the marker and the quarantine leaf are removed, a check always runs before the next write. A single directory listing or system call is not interrupted, and one delete pass has a 120 s deadline; this is cooperative checking, not a hard 1 s watchdog. Any loss stops further writes and leaves the intent pending; unknown free space is not above a reserve and stops too. The admission lock is not held during long deletes; the checkout binding barrier is. Before each root's intent, under that root's clean barrier and binding lock, the run rereads the ready registry and the root's receipt and re-runs every eligibility predicate for its row (writer custody and command records in both pools, the caller's own barrier excepted; the root's device, inode, marker, nonce and generation unchanged since that observation, with a stale recorded device refreshed in the publication as above). The intent is then published only if the registry revision still equals the one that reread saw; one changed revision gets one more locked reread, and a second keeps the root. Every registry write, including each intent phase, holds the internal publication lock. Replaying an unfinished intent (rollback, resumed deletion or a terminal receipt) takes the same checkout barrier and binding lock.
- **Journal order.** `intent` → `renamed` → `deleting` → `marker_last` → `deleted`; every phase is durable before its effect. While a transition is prepared, the registry keeps the committed root selected and adds the proposed root as pending; a missing reference is never read as non-selection. The registry terminal state (row no longer selected, `pruned_ms`) is durable before the checkout's `oxide.outputs.transition/2` receipt records `deleted`, and diagnostic observations are best-effort afterwards. The intent then records that its receipt committed; until it does, a `deleted` or `resolved` intent is still unfinished, so adopt refuses, prune replays the receipt step and `resolve-intent` finishes only that step (exit 3 if it fails). A missing quarantine or marker without positive deletion proof, or an unknown volume, is `pending` (exit 3), never inferred as done. `oxide outputs resolve-intent <intent-id>` is the only settlement verb: it never initializes the format, and it takes the checkout's clean barrier and binding lock for the recheck through the committed terminal transaction. Both leaves must be positively absent under the verified volume descriptor, the registry must be complete with the intent's exact blocking row, the checkout must select no other root, and no recorded writer may hold it in either pool. It publishes a durable `oxide.outputs.prune-resolution/1` receipt and records `resolved-by-operator` in the checkout receipt under the same custody ordering. Completed resolution rows are removed from live selection only after that proof; unfinished intents and resolutions are safety journals that are never compacted.
- **Forward format.** Prune establishes the shared forward fence first: numeric state version 3 with `owned_outputs: "oxide.outputs/2"`, receipt `oxide.outputs.transition/2` (one `target` placement member in this tranche) and marker `oxide.outputs.root/2` with the root nonce. The internal control records `initializing` then `ready` durably before the paired removable control, and only then is `outputs/registry.json` (`oxide.outputs.registry/1`, numeric schema 1) created exclusively. Version-2 root markers also record the checkout's volume UUID. Initialization, and resuming an `initializing` record, first requires positive writer closure across both pools with both admission locks held together: no active or held command record, no lost slot with unclosed writers and no clean barrier other than the initializing checkout's own. A quarantined lost slot whose holder predates the current boot session is closed by the restart and does not block initialization; it is released and deleted whole at the next admission, through the same volume-bound path (oxide#161). An older binary's in-flight command is a record in the same state, so it must close first; otherwise initialization refuses and nothing is written. A newly paired or reappearing removable control is fenced under the same check. Every paired-control branch accepts exactly the released `oxide.outputs/1` fence and the published `oxide.outputs/2` fence as predecessors of the successor fence, plus an absent fence where the control may be new, so a pool fenced by a released binary is never migrated by one branch and refused as unknown by another; any other value refuses at the shared state parser before any effect (oxide#159). A missing, unreadable, foreign or sticky-incomplete registry refuses mutation and keeps every root; it is never rebuilt from markers. Version-1 state and the version-1 receipt shape remain readable as legacy, the latter only on the prune paths above; a legacy root has no nonce and is never given one from a missing root's receipt. A prune publishes a kept root's receipt never, and a pruned root's receipt only through its existing terminal transaction (see the version-1 bullet). The numeric state version advances only with the `oxide.outputs/2` fence or its initialization record: an ordinary mutation of a pool that has not opted into owned outputs keeps the previous numeric version, and a released `oxide.outputs/1` fence does not itself advance it, so an older binary is not refused merely because a build ran. A pool whose state records `ready` but whose registry file is missing refuses every mutation and prune keeps every root; the registry is never recreated from markers or receipts. A pool adopted before this format has no registry: `oxide outputs adopt` and `oxide outputs detach` with an existing receipt initialize it behind the fence (under that closure rule) before their first receipt write, and a command's own transition or replay refuses before any effect, naming that upgrade, because a command holding a slot is itself an open writer. Explicit adoption establishes the format on **every** path that can publish a transition (oxide#217), so the first adoption on a fresh pool also converts a real nonempty target whose profile members are missing or empty: that route reaches the same transition a missing target does, and it was the one place the registry had never been initialized. Readiness is decided before the publication lock — which initialization itself takes — from the same effect-free facts the conversion dispatches on: any receipt at all (a replay, relink, migration or prepared transition may write), a missing or empty real target, and each member of a real nonempty target in the creator's own order. An adoption that refuses without publishing a transition (a foreign target, a foreign member it reaches first, a whole-target owned or dangling link) writes no registry, receipt or link, and a pool whose state records `ready` but whose registry file is missing is never rebuilt, so that pool still refuses exactly as before.
- **Nonces and keys.** Every root creation or recreation draws a fresh random 128-bit lowercase-hex nonce. The output destination key includes the nonce, root, volume UUID and recipe; legacy `outputs-1` keys stay distinct and no recreated root claims a warm legacy root. An existing root keeps its marker's nonce.
- **Registry and overflow.** Rows are keyed by checkout, placement member and root; revision advances before every changed receipt, selection or root effect, and the registry is read as one bounded whole snapshot (8192 files, 64 MiB, 10 s). The live registry is capped at 4096 rows and 4 MiB. Before a cap-exceeding unindexed selection the registry records sticky incompleteness durably; that write failing refuses adoption before its effect, and prune then keeps every root with an explicit reason. The incomplete flag is never cleared by this tranche.
- **Recorded root identity and generation (oxide#218).** A registry row records the root's own immutable creation generation, read from its owner marker when the row is written, never the receipt's committed selection generation: `transition_at` keys a selection to the marker's creation generation (oxide#152), and the marker is rewritten only by the version-1 to version-2 upgrade, which preserves it. Rows written before oxide#152 recorded the commit generation, which advances with every transition, so a long-lived root's row could say generation 2 or 3 while its marker still says 1. That one superseded shape is reconciled at observation, and only from the recorded marker digest: when the recorded digest equals the live marker's exact bytes the document — including its own nonce, volume and creation generation — is the recorded one, so the differing field is a superseded selection observation and never a claim about a re-created root. A recorded generation OLDER than the marker's (nothing writes one), a row without a recorded digest (no pin), a changed marker, a different nonce or kind, a different volume and a changed inode all still classify Unknown and keep the root. A migration's intermediate receipt writes also never rewrite a whole-target row with the root/2 marker that replaced its legacy one: a row is refreshed only from a placement that records the marker's own nonce and creation generation, so a superseded placement keeps the row's recorded history instead of writing the successor digest with an empty nonce. Within one (checkout, root) group the evaluated representative is the first LIVE reference in member order — selected, pending, reserved or already owned by an intent — because only the selected subset is the checkout's current authority; an inactive history row (a legacy `target` reference left by a profile migration) is never the representative while a live reference exists, so it cannot make the snapshot incomplete for a contradiction the group does not have. A group with no live reference keeps its first member exactly as before. This changes no other identity gate, the whole-batch incomplete-snapshot rule for a genuine contradiction, unknown writer custody, or the operator-list requirement for a row without a version-2 identity; a dry run still mutates nothing. Evidence: the six owner-marker/registry contradictions found after oxide#216 are three inactive legacy `target` rows left by profile migrations (no nonce, successor digest) and three pre-#152 whole-target rows recording commit generations 2, 2 and 3 against creation generation 1, with recorded device, inode, nonce and marker digest matching the live roots; the first class keeps its recorded history and is simply never the representative, the second is reconciled by the rule above, and no live registry, root or operator list was touched. A future live prune still needs a fresh complete preview, its exact plan SHA and a separate operator go. At the 2026-11-01 live scale (512 roots, 1177 rows) that preview is not yet achievable within the snapshot's fixed 64 MiB input budget: with the identity contradiction gone, full eligibility runs for every root and re-reads the pool state documents per root (the removable pool's ~95 KB `state.json` and the internal ~5 KB one, up to twice each in `checkout_custody` and `active_holders`), which the earlier identity reason masked — the identity gate is not the cause and this is a separate ruling, because widening the budget or reading one state document per snapshot changes the custody-freshness contract.
- **Post-218 snapshot refusal (oxide#220, diagnosis only; no code change).** The read-only preview on installed `9d2aa46` (preview `b1549017…`, revision 11349, 0.554 s) kept every root for "the snapshot exceeded its input or time budget" on the removable pool's writer-record read. The time cap is excluded: the whole command ran 0.554 s against the ten-second deadline. The byte budget is the mechanism. `authority_read` charges each read of a state document to the one shared 64 MiB snapshot budget and caches nothing. For each absent-checkout row that reaches custody, `checkout_custody` reads the internal `state.json` twice (the admission fence's load and its own load) and the removable one once (sibling state); `active_holders` then reads the internal one again and the removable one again. That is three internal and two removable reads per row, about 205 KB at the documented ~5 KB and ~95 KB sizes, so roughly 320 such rows exhaust 64 MiB. The exhausting read returns the budget message even when its own read succeeded. An earlier I/O error cannot be the origin: every error on that path is recorded before it can latch, so the first recorded error is the exhausting read. The crossing row and the cumulative byte figure were not observed. `kept_roots: 512` on that preview is the then-current `MAX_CANDIDATES` truncation in `build_preview`, not a count. oxide#256 later reports the full eligible, selected and ineligible counts and, when the eligible set exceeds 512 roots or 256 KiB, writes a usable selected prefix rather than refusing the preview; it does not change this snapshot-budget diagnosis. A fix that changes how custody state is read or budgeted changes deletion-gating custody and needs an Elevated brief plus a freshness ruling. A dedupe inside one row only trims about half the bytes and may not clear the live scale. That brief and ruling are the fresh custody reads above, implemented by oxide#235: the witnessed route reads each state document once per authority window, so the per-row re-read exhaustion no longer applies to prune.
- **Fresh custody reads (oxide#229; implemented by oxide#235, diagnostic and counted qualification accepted).** Tom’s 2026-11-01 ruling on the proposed contract was “Allow this design subject to independent review”: parsed pool state may be reused across preview rows only under both per-row admission locks with a pinned open-file identity and unchanged metadata witnessing the same atomically replaced document; changed or uncertain witnesses require a newly charged bounded read or batch refusal, dynamic process custody is rechecked per row and effect-time proofs stay fresh. This ruling authorizes neither implementation nor a live preview. The current per-row reads are deliberately not replaced by a snapshot-wide claim that writers stayed closed. A candidate design retains at most one parsed state and its open file descriptor per pool for one authority window. At *each* custody decision it takes both existing admission locks in internal-then-removable order, without creating a missing lock or fencing state, and verifies the registered pairing, control directories and lock identities through no-follow descriptors. It then checks each state pathname against the still-open descriptor: same device and inode, regular private file, owner, mode, link/size and high-resolution change metadata, with pre/post checks surrounding any actual read. The descriptor remains open across rows so an unlinked inode cannot be recycled into a false match. A changed witness requires a fresh bounded read and complete parse under the locks, charged to the same aggregate budget; an uncertain identity, unsupported metadata, changed control or lock, missing required input, invalid parse, failed lock or read, or budget/deadline exhaustion refuses the whole authority batch. A known absent, unregistered sibling is usable only with a positive no-follow absence and registration witness repeated under the internal lock on each row; ambiguous absence is unknown. A registered sibling without provable state and lock is unknown, never empty. Use non-blocking acquisition in that order: if the second lock is busy, release the first and refuse this snapshot rather than wait while holding it. Release both immediately after that row's custody decision, before path traversal or report construction. Acquisitions and checks are bounded by the existing snapshot deadline; no lock acquisition may outlive the window.

  The current front-door `Store::save` and library `persist_control_state` use atomic rename under admission; removable registration and mode changes also take internal admission. The current internal `checkout_custody` takes internal admission, whose fence can write, while its sibling read does not itself take the removable lock. These observations do not establish the whole premise. This design is conditional on a source audit and qualification proving that every supported cooperative writer, including released predecessors and the library, serializes state changes on those same lock identities and replaces the state atomically, rather than changing its inode in place. Metadata comparison is an additional tamper detector, not an ABA-proof substitute for that invariant. If any supported writer can mutate in place, if file identity or change metadata cannot be trusted on a supported filesystem, or if stable lock and directory identity cannot be proved, this reuse rule is unavailable and the batch refuses; timestamps, a pathname, a digest of an old read, and the output registry revision alone cannot witness freshness. No arbitrary same-user writer or external process is newly fenced by this proposal. A registry revision witnesses registry publication, not pool-state freshness; root, receipt, reference, nonce, generation, class and marker checks remain independent and retain their existing rules.

  Recompute each checkout's custody from both currently witnessed states at that row: retain the existing distinctions among active records, spawning/running writers, clean barriers, retired lost writers and boot-closure evidence, including each pool's own record identity. Re-evaluate any dynamic process, lock, group and boot facts at the decision point; never cache PID liveness or a prior clear result across rows. Unknown lost writers, unreadable process or lock facts, unsupported class/fence, inconsistent pool registration or unclassifiable state never become closed. A writer admitted between rows replaces a pool document under its admission lock and is seen by the next decision; a writer admitted after a row was checked can make that row's preview observation stale, as with the present per-row snapshot. A preview is not a deletion lease. The real run starts a separate authority window; before each intent its clean barrier and binding lock, fresh registry and receipt, checkout-absence witness, root identity and both-pool custody checks remain mandatory, followed by the existing registry revision compare-and-set under publication custody. No snapshot cache crosses either window or substitutes for those locked rechecks. A changed root, reference, reservation, nonce, class, volume, inode, marker or receipt retains its existing keep/refusal, including the narrow #216 and #218 reconciliations.

  **Alternatives and resource basis.** Reading each pool once without per-row witnesses misses a later admission. Reusing only within a row removes some duplicated reads but still costs approximately one 5 KB internal and one 95 KB removable document per eligible row, about 50 MiB for 512 rows before registry, receipts, markers and all other inputs; it is not a reliable solution at observed scale. Holding both admission locks across the whole snapshot would block admissions through filesystem traversal and is not this design. Raising 64 MiB or ten seconds merely postpones exhaustion. With unchanged states the proposed incremental charge is one actual read of each document, roughly 100 KB at the historical sizes, plus all other authority input reads; every replacement is charged again, including failed and partial reads. The existing 64 MiB, ten-second and per-input limits remain, and reaching the boundary fails closed. Two individually valid 32 MiB states already consume the aggregate limit before registry or receipts: valid inputs or heavy churn can still refuse, with no promise of completion at every valid size. A report's 512-kept-row cap never changes coverage or turns an incomplete batch into a complete one. The six historical marker contradictions and the separate unqualified legacy 91-row inventory are not evidence that any present root is safe.

  **Design freeze (2026-11-01).** The project planner froze this conditional contract after independent exact-head review `6565393855` of `0868684c826ac02e589b01a7e1a2cf125d27cf6d`, which verified Tom's native ruling above. The writer/lock/filesystem and no-effect dry-run premises still require the specified source audit before implementation. A failed premise requires redesign, not weaker witnesses or a larger budget. The freeze supplies no native credit, live-preview permission, operator-list approval or deletion authority.

  **Proof fixture observations.** The focused proof uses the existing supervised, private-control debug gate to pause after one row without holding its admission locks, bounded by the original authority deadline; a second debug-only observation reports the actual aggregate authority bytes, including a budget-exhausted partial reread. The fixture also stages large valid state documents and changed real-run inputs; a separate debug-only bounded pause at the pre-intent publication comparison exercises binding custody and two revision changes. The accepted diagnostic below measures their fit for that exact source and fixture. Neither hook exists in release builds. The image indication is published before attachment and the qualified guard alone receives the detach request after child custody is closed. The hooks add no production authority; their native evidence is scoped to the receipt below.

  **Diagnostic history (oxide#235).** The one Queue-routed uncounted diagnostic at `ea2832af` failed: baseline and absent-custody completed, then dynamic-writer read `removable/managed/state.json`, although the product uses `removable/oxide-managed/state.json`; six dependent cases were skipped. The supervisor closed with command status 1, no fault or deadline, 408.3 MiB peak of 512 MiB, 305.6 GiB minimum host free, and a successful post-census detach. The case receipt was `qualified: false`; this is no native qualification credit. The retained log SHA-256 is `1501d251bbcfc619658d656504c1c762b988f79a41fceaf42a564d6ec42ae118`; the Queue proof execution `2d9b42ab-6036-45aa-b72d-86177eb62f4e` owns the evidence locations and guard receipt. The corrected fixture derives the same shared managed-control name as the product and checks both existing private lock/state pairs before its cases; that correction itself is not native evidence.

  **Second diagnostic (oxide#235).** The Queue-routed uncounted diagnostic `27b37717-c65b-4ad3-805a-b3aa45210740` at `c4837ca` completed six cases, then failed the large-state fixture and skipped two dependent cases. Two individually valid 30 MiB documents exhausted the 64 MiB aggregate: readiness reads the internal document before the witnessed internal and removable reads, and the charged counter reached 67,108,864 bytes. The preview kept all three roots with incomplete coverage. The guard closed with command status 1, no fault or deadline, 468.2 MiB peak of 512 MiB, 305.1 GiB minimum host free, and detached the image successfully. The receipt remains `qualified: false` and gives no native credit; log SHA-256 `db345a34693a90890be4b19aa4a2087e864fccc2cbb074041acd79ad62c91fb6` and the Queue proof execution bind the evidence. The corrected source keeps that symmetric refusal as a control and uses 2 MiB internal and 31 MiB removable documents for a separate complete baseline and replacement-exhaustion check. This is not a promise that arbitrary large documents complete or new native evidence.

  **Accepted diagnostic and deployment boundary (oxide#235).** At source `c47328cc371627be243908b44e9ab096630db446`, Queue execution `3c86a13c-75b8-4527-8007-1ac85a702dc0` completed all nine cases with no error. Retained evidence review `6579534504` accepted the exact source and receipts. The symmetric large-state control and replacement reread both exhausted at 67,108,864 charged bytes and kept the batch; the asymmetric baseline completed at 36,711,600 bytes. The 709-root case used 2,319,235 authority bytes in 0.56 s with the 512-kept reporting cap; its per-row reread counterfactual would charge 180,165,408 bytes. The private effect deleted three fixture roots, and the immediate reproof refused. The guard closed with a certain empty census, no survivors, fault or deadline, and a successful post-census detach: 39.85 s of 360 s, 462,209,024 bytes (440.8 MiB) of 512 MiB peak, and 304.4 GiB minimum sampled host free.

  The original diagnostic remains intentionally exit 1 / `qualified: false`, with no counted qualification credit. Its log `/Volumes/Builds/scratch/oxide/proof-235-executor.8nVgfZ/diagnostic.log` has SHA-256 `df0518147ba1831ffa0da3392590fd32475c004f6ee05f6eeebd74e82c16fefd`. Under `/Volumes/Builds/scratch/oxide/oxide-proof.hpo8ba5f/evidence`, the case receipt is `6dc9d5d841927dd81100321d4a5eeaf8593c150ab64a863225dfd3083fc46fe7`, the guard receipt `6de172c8d43c242715cb406589edc2a082e5ae26290dbcdeea483c52fe7ace90`, and provenance `55b363a74b8b232ea89cf479659e4d3284e6203f4f24ff37e4a6f60cbc90731d`. They bind tree `7450c3185c8cbb143d4ad4d3803c1388f540a911`, proof script `b2c10f9cb616ddea068f882984be5a0f07de14e7075aa8851b3003f70c546266`, manifest `fceb535abdd77af83c5e9aeb80112c01444f2d2225e40108bfe5e3278e09d50b`, qualified candidate `c5782431…` and unchanged ordinary default `fcfda2b0…`. The two failed packets above remain historical and receive no credit.

  Attributed planner decision `2eee0c9a-8cea-4743-b547-4040a0863baf`, under Tom's `c8991f99` delegation, accepts the reviewed source for merge while retaining one counted qualification as a release gate before installation or live consumer activation. That separate Queue proof-only execution uses the exact merged source commit, selector `effigy rust:prune-fresh-custody`, and the same 360 s / 512 MiB / one owned 128 MiB image bounds. Counted mode prints `qualified: null`, requires all nine cases completed and `error: null`, and exits zero only after the proof's completion checks; it does not write the diagnostic case file or emit `qualified: true`. Judge the original stdout payload and qualified guard/custody receipts rather than inventing a field or modifying a receipt. The installed binary and ordinary runner routes remain unchanged until that gate is accepted. The read-only operator-list packet still requires its separate planner activation, exact live deletion requires Tom's go, and the legacy 91 roots and full released-predecessor enumeration remain explicit residuals. This evidence establishes neither universal ABA safety nor completion for arbitrary large inputs.

  **Accepted counted qualification (oxide#247).** Queue execution `90ef29a8-ec34-4114-9e5c-1e2a423dc95e` ran `effigy rust:prune-fresh-custody` once at the same immutable `c47328cc` source. Retained receipt review `46ab2d27-1768-4869-86f8-714013f7f0dd` accepted the pass: all nine distinct cases completed, `error: null`, counted `qualified: null`, child exit zero, closed guard with a certain empty joined census, no survivors, fault or deadline, and a positively reaped successful image detach. The counted scale case took 0.42 s; authority-byte and effect/refusal oracles matched the accepted diagnostic. The whole guard took about 39.8 s of 360 s, peaked at 461,479,936 of 536,870,912 monitored bytes, and sampled a minimum 317,477,711,872 host-free bytes (295.7 GiB) in 154 readings with a 0.2603 s maximum gap. Source, tree, script, manifest, lockfile and both runner pins matched the five declared inputs.

  Original evidence is retained under `/Volumes/Builds/scratch/oxide/oxide-proof.lyjda1y9/evidence`: `receipt.json` SHA-256 `e0a15fdb87282adbf0e10e7d2d6bd4e1280584eeb1a8094702ebd4a1e5961d13`, `stdout.log` `3c7b379efa8f08290ca54205cf4e46ebb0f8eb35bc7a26d9ab846f1cf6ae6d40`, provenance `57c48b3e7905bf040657462174dc2fa6e590c4e14c54acf894131f5aef230568`, image custody `344754ce8fcc313b2e44c19d6a708178f65c4d88451c32392e1184ab7a54ae31`, and detach request `d5b9ca79f33cf313892bbe89ecaf1e01ab93f28b8b473566831498853f4cf903`. The reviewer and planner independently rehashed the artifacts; the executor narrative omitted one `d` from the guard hash, and that transcription error is preserved rather than repairing a receipt. The counted case payload is the original stdout JSON, not a new diagnostic case file. External selector logs in `/Volumes/Builds/scratch/oxide/qualification-645fa4ff.7SUBrw` are separately disclosed outside the monitored root.

  Attributed planner decision `41e602eb-7df6-496f-bb8f-c6c53261b4b2` accepts this counted qualification and the ordinary receipt-matched reviewed-main Oxide reinstall under standing authority. The executable source is unchanged from the qualified head; later knowledge edits do not extend its native claim. Ordinary default `fcfda2b0…` and fixed candidate `c5782431…` remain unchanged. Installation supplies no live preview, registry census, cleanup, repair, deletion or operator-list authority. The later read-only packet remains a separate planner activation and exact live deletion still needs Tom's scoped go. The full released-predecessor enumeration and legacy 91-root residuals remain explicit; no universal ABA or arbitrary-size completion claim follows.

  **Minimum future proof.** Before authoring, independently review this design and obtain planner freeze; verify all lock/state-writer premises, the no-effect dry-run lock route, lock ordering and bounded acquisition against actual supported predecessors. If any premise fails, redesign rather than silently weakening it. A separate HIGH-complexity, Elevated implementation brief must include the proof-authoring premise checklist, source changes and focused regressions, `effigy rust:compile`, `effigy rust:compile-release`, `effigy rust:format`, `effigy test rust`, `effigy docs:check` when applicable, and the single Queue-routed uncounted diagnostic at the exact source, judged by the retained reviewer. Tom's later decision `c8991f99` supersedes the former separate counted author/reviewer chain: further qualification is a planner-owned release gate, carried by one ordinary proof-only execution and one retained receipt reviewer. The proposed focused selectors are `effigy rust:prune-fresh-custody:diagnostic` and `effigy rust:prune-fresh-custody`, each invoking one fixed script under the then-qualified shared supervisor and Effigy heavy admission, with a fixed 360 s wall-clock limit and 512 MiB monitored scratch, with at most one owned 128 MiB image; the accepted diagnostic above measures fit at its exact source, while every later execution still needs its own Queue admission. The proof must stage its own isolated controls and assert actual read bytes, locks, witnesses and complete output, not merely count calls. Counterfactuals must show that removing each witness admits a changed state or writer: unchanged reuse; replacements between rows in either pool, including admission immediately before and after the check; lock/control/path replacement, missing or unreadable sibling and invalid, oversized or partial documents; in-place-change and inode-reuse attempts; lost/unknown writers and dynamic liveness changes; changed registry revision, root/ref/nonce/class/marker, receipt and binding; large valid input and cumulative repeated-change exhaustion; and >512 keeps with incomplete coverage. Exercise preview, fresh real-run revalidation, barrier/binding recheck and publication compare-and-set, with full-tree and exact symlink-target nonmutation oracles for refusals. Measure wall time and scratch and retain failures without native credit; a diagnostic is uncounted. Existing native receipts, live-preview authority and Barge adoption gates do not transfer.

  **Implementation (oxide#235).** The current-source premise audit found that the front door's `Store::save` and the library's `fence_managed_control` replaces the document atomically by temporary-file rename while holding that control's `admission.lock` (registration and mode changes take the internal admission lock the same way), these current paths do not mutate the document in place, and supported operations preserve the lock and control directory identities. Released-predecessor coverage remains a separate audit residual; no version, fence or budget was widened. The landed seam is a per-window `CustodyWitness` in the state reader: at most one parsed state and one still-open state descriptor per pool for one prune authority window, created on the window's first custody decision and dropped with it, so nothing crosses windows. Each row takes both existing admission locks internal-first, NON-BLOCKING, without creating a missing lock and without admission fencing, through the verified no-follow control descriptor; the registration witness is re-read under the internal lock on every row (charged to the shared budget like every other snapshot input); the state pathname is then witnessed against the pinned descriptor — device, inode, regular private owner-only file, link count, size and high-resolution change metadata — with pre- and post-checks around any actual read, and a changed witness forces one fresh bounded read and full parse under the locks. A known absent, unregistered sibling is used only with a positive no-follow absence plus registration witness repeated per row (a removable control directory without a registration document is ambiguous and refuses); a registered sibling without proved control, lock or state is unknown and refuses the batch. A busy second lock releases the first and refuses the snapshot; changed control or lock identity, unsupported metadata, missing input, invalid or oversized parse, failed lock or read, and deadline or budget exhaustion all refuse the whole batch. Both locks are released before any path traversal or report construction. The verdict and holder texts are computed by one shared classifier (`custody_verdict`, `holder_lines`) so the retained unlocked `checkout_custody` route — still used by transitions and cleans — and the witnessed route classify records identically, including boot closure recomputed per row. All four prune custody sites use the witnessed route (snapshot eligibility, the receiptless custody reason, the intent-time under-barrier recheck inside its own fresh window, and the resolve-intent settlement, which runs one one-shot witness outside any window), so a prune preview no longer takes the fence-writing admission gate at all. Focused unit regressions cover unchanged reuse and re-read on replacement, replaced lock and control identity, missing-lock refusal without creation, busy internal and removable locks with first-lock release, unprovable and ambiguous sibling states, in-place changes including a same-size, same-inode re-read, hard links, symlinks, oversized and vanished documents, per-row lost-writer and boot-closure recomputation, holder-line parity, and the whole-snapshot refusal classification.
- **Status.** Default `oxide status` reads only the bounded recorded `outputs/prune-observations.json` (`oxide.outputs.prune-observations/1`, at most 256 KiB and 512 roots) in the pool section. It performs no root walk, replay, lock or reconciliation. Coverage, source and time are explicit; a partial observation never becomes a global orphan total. `--measure` does not scan output roots for prune.
- **Nonce reservation.** A building command reserves its future root's nonce after automatic adoption and before `planned_destination`/`SlotKey::resolve_in`, under the checkout's binding lock (released before the slot wait) and the internal publication lock: the receipt's `target` placement records `reserved` (transaction ID, fresh random nonce, root, kind, volume UUID, generation) and the registry a pending row with the same transaction and nonce, in one write-ahead publication. An existing root keys its marker nonce; a future root keys only its reserved nonce, never a missing root's old one. Starters for the identical future root share one uncancelled reservation; the commit that creates the root consumes it. Binding validates the exact surviving reservation (receipt, keyed nonce, registry row with no cancellation) before Cargo and refuses a cancelled, replaced or absent one with no automatic retry. `oxide outputs cancel-reservation` (and `detach`, and a starter that needs a different future root) cancels only under the checkout's binding with every writer clear, the reserved root absent and no prune intent: the registry first records the cancellation (authority invalidated, revision advanced), then the receipt commits without it, then the abandoned row is removed; an interrupted cancellation is finished only for its exact recorded transaction. A cancelled nonce is never reused. `oxide outputs plan` stays read-only. The pre-lease reservation applies binding's P4 precondition exactly, so a command that binding will refuse for it changes no authority — no `reserved` field, no generation, no updated timestamp, no link text (oxide#133 case14). A requested profile member whose committed root is missing under its owned member link (binding's `Dangling` state) also reserves nothing — its receipt bytes, registry bytes, revision, digest, generation and timestamps are unchanged — and the explicit `oxide outputs adopt` that refusal names then recovers in one command with a fresh nonce that supersedes the old committed row, which the next build uses (oxide#152). That precondition is per REQUESTED member and only for a profile-layout receipt: a requested member whose committed placement is external while this command's fresh selection is not. A member the receipt does not place, a whole-target receipt, and any command whose fresh selection is still external keep their reservation — a whole-target local fallback and a new member beside a committed external sibling both need it for `reservation_matches`. The ordering nit closed by oxide#133 is recorded here: in a profile checkout a command name whose profile outputs Oxide does not model is refused in the declaration step, BEFORE `reserve`, so it publishes no reservation at all. The remaining deferred nit is narrower: an `exec` that declares no member is still refused by `planned_destination`, i.e. after the reservation, and moving that refusal ahead of `reserve` (so it too changes nothing) is not part of oxide#144.
- **Overflow.** When a reservation would exceed the live row cap, the registry durably records sticky incompleteness and the reservation is retried once with an explicit overflow tag (registry identity, transaction, nonce, revision as provenance) and no registry row. Binding validates that exact tag against the same sticky-incomplete registry and the overflow cancellation journal; an unrelated revision advance never refuses it. Prune keeps the whole batch while the registry is incomplete, and the flag is never cleared by this tranche.
- **Version-1 receipts (oxide#152).** A root whose checkout receipt is still the whole-target `oxide.outputs.transition/1` shape must not be globally unprunable because its receipt predates the current formats. The prune snapshot, its locked rechecks, the real run's receipt reread and its intent/terminal receipt image read a valid version-1 document through the same parser and checkout identity as every other reader, and decode it into the current whole-target form. Committed is the only eligible version-1 state: a committed version-1 receipt selects its one root and every existing predicate still decides that root's fate. A valid noncommitted version-1 receipt (`detached`, or `prepared` with its pending transition) keeps its own root/reference group under those same predicates and never poisons an otherwise complete snapshot merely by being old. A version-1 document that carries a successor-only surface (a layout, placements, a root-level reservation, a prune state or a root nonce), an unreadable identity or an unknown version/field stays invalid under the existing whole-snapshot rule, keeping the whole batch with the exact receipt file named. Reading is never a migration: decoding, the dry run and the preview write nothing, keep the receipt's bytes, digest, mode and timestamps, and never give a legacy root a nonce. Only the real run of a root it actually prunes publishes a receipt, and that root's existing terminal transaction may publish the current shape because version 1 cannot express a prune state; every kept root's receipt, including a valid noncommitted legacy one, keeps its exact bytes. Other readers (bind, detach, clean, replay, status, plan) still refuse a version-1 receipt unchanged and now name `oxide outputs adopt` as the fix. That explicit adoption is the one command that upgrades a committed version-1 receipt (oxide#163): it reads the same document through this parser and checkout identity, refuses a noncommitted, successor-surface, unknown-field or foreign-identity one, and runs the shipped whole-target-to-profile migration so the current `/3` receipt is published with no payload moved, copied or deleted. A legacy row without a recorded nonce, device, inode or marker digest still needs the explicit operator list, a row with a recorded version-2 identity is never satisfied by a version-1 selection (no nonce is invented), and every marker, writer, checkout-absence, volume, containment, reservation and intent predicate is unchanged. Case 19 of `effigy rust:exec-output-inference` is the live upgrade scenario (oxide#163): it seeds a real committed version-1 receipt and root/1 marker from the product's own conversion, shows `build`, `detach` and `clean` refusing it naming adopt and `status` reporting the same refusal in its own grammar (recorded status reads no checkout state; `--measure` renders `outputs: unknown` naming adopt) with the receipt bytes, root tree and link unchanged, keeps a noncommitted version-1 control's bytes exact, then upgrades with one explicit adopt and runs a build and an exec. It has not run — each native run needs its own owner activation — so its fit is UNMEASURED, and it is not a benchmark or an alias for full QA. A legacy row without a recorded nonce, device, inode or marker digest still needs the explicit operator list, a row with a recorded version-2 identity is never satisfied by a version-1 selection (no nonce is invented), and every marker, writer, checkout-absence, volume, containment, reservation and intent predicate is unchanged.
- **Authority and accounting (oxide#134).** Prune first completes one
  ten-second, 64 MiB aggregate authority snapshot, including required
  registry, receipt, group, identity, custody and operator-list inputs.
  Uncertainty keeps the whole batch and reports incomplete authority. Only
  after that snapshot can candidate logical and allocated byte observations
  run, under a separate five-second, 50,000-entry aggregate budget. Those
  values do not select roots or authorize effects; an unfinished or unmeasured
  observation is labelled incomplete, not an empty root. The limits are
  cooperative: a filesystem syscall may overshoot. Locked per-root custody,
  identity and group checks and the revision compare-and-set still precede
  every intent. The earlier live timeout's cause was not established; the old
  interleaving is one possible cause. The focused script
  `effigy rust:prune-authority` and its uncounted `:diagnostic` twin use the
  qualified shared supervisor, 300 seconds, 512 MiB and one owned 128 MiB
  image; their fit is unmeasured. The one activated diagnostic at `10377c0`
  was spent and failed before staging: the strict offline seeder refused the
  account Cargo home's symlinked registry and Git directories. All four cases
  were unexecuted, no image was attached, and the guard closed with status 1
  and a certain empty census. Its preserved diagnostic receipt is
  `/private/tmp/oxide-proof.jqmd3cyc/evidence/prune134-diagnostic.json`
  (SHA-256 `d8f56d3d4d0648b1830ebd050e19b70431f454ac327da6a824cfc5272c16e8cc`);
  the guard receipt SHA-256 is
  `fb038e4e5b3d1f9d90aa3b5d81f93e7140054574cfe345627a71460100ed503d`.
  A second activated diagnostic at `95c10eb` was also spent, failing before
  staging because Python 3.9 rejects `realpath(strict=True)`. All four cases
  were unexecuted, no image was attached, and the guard closed with status 1
  and a certain empty census. Its diagnostic receipt is
  `/private/tmp/oxide-proof.8baumhti/evidence/prune134-diagnostic.json`
  (SHA-256 `a68cf3b0915af2923158cdfd1a3338821c189d44fe5332f9a8d3162e9d91fb4f`);
  its guard receipt SHA-256 is
  `8473f38a4bd563dfae3358650ca2023ef29648a14621b319e9ac52e3e12c0098`.
  The proof now uses Python 3.9-compatible strict path resolution, exercised
  by the pure check on an existing file and a missing path.
  A third activated diagnostic at `ca67322` completed accounting and authority
  refusals but failed revalidation: the missing-receipt fixture restored the
  file with a mode the product refused, making its subsequent authority view
  incomplete. The positive case was skipped; the guard closed with status 1
  and a certain empty census, and ordinary image detach completed. Its
  preserved diagnostic receipt is
  `/private/tmp/oxide-proof.ld5zqdhh/evidence/prune134-diagnostic.json`
  (SHA-256 `1c06244bb706622c59a814242a7436bde606ff77e8d83df30f0f1d1e227661d0`);
  the guard receipt SHA-256 is
  `e83f21273e3e0fa232e0b3cfb8f9d232909521304c2942f454774cd43e20bc14`.
  The fixture now restores exact bytes with exclusive creation and the original
  owner-only mode; its self-check exercises that restoration under the host
  umask in a private temporary directory. This spent start is not proof credit.
  Each authority-refusal oracle also requires every kept reason to name its
  planted missing, corrupt or oversized receipt, sticky registry, or injected
  unknown custody; the pure check rejects an unrelated incomplete snapshot.
  The corrected proof verifies a common canonical real cache root before and
  after strict seeding into its private Cargo home; its pure counterfactual
  is `effigy proof:prune-authority:test`. No counted native has run.
  S4 (oxide#211, decision `7aedd5af`) splits the proof. `rust:prune-authority`
  and its diagnostic (Builds mode) place the directory pool under
  `OXIDE_SCRATCH_ROOT` on the selected volume, with no `--root-parent`. They
  cover accounting, refusals, revalidation, a receiptless legacy root on the
  external volume (classified Unknown and kept, `prune.rs:3210-3216`) and a
  recorded-UUID group. `rust:prune-authority:internal` and its diagnostic keep
  only the receiptless legacy-positive case on `/private/tmp`, because its
  legacy checkout's nearest existing ancestor must be on the boot Data volume
  (`prune.rs:3276-3287`). Both modes' fit is unmeasured and both need their own
  owner activation; neither has run at the migrated head.
  The armed debug prune test hooks (`OXIDE_TEST_PRUNE_*`) fire only under the
  supervisor and only through one shared gate, `proof_evidence`
  (`src/bin/oxide/prune.rs`). That gate arms on the internal volume, or under a
  declared `OXIDE_SCRATCH_ROOT` that is a canonical strict child of the
  supervisor scratch (`declaration_allowed`). Control-directory hooks also
  require their control inside the declaration (`hook_control_allowed`). The
  EACCES, low-reserve and control hooks all route through that gate (decisions
  `40380716` and `60d4914f`). Release builds carry none of them.
  Effigy heavy admission unavailable (f6b3b50 retired host-run client;
  Queue #381 pending); not host-admitted. Any later activation needs fresh
  owner scheduler headroom, independent source-fit and separate activation.
  This does not qualify
  oxide#133's affected checkout-detach path or authorize a live prune.

- **Focused proof of absent checkout chains (oxide#135).** The one fixed proof
  is `effigy rust:prune-absence` (`scripts/test-prune-absence.py` under this
  tree's shared supervisor `94b1aba6…`, the merged oxide#151 guard plus
  oxide#191's opt-in post-census detach and oxide#197's ancestor-chain
  exclusion) with
  `--root-parent /private/tmp`, the owner-accepted internal-placement exception
  (decision `bd357a4c`): the missing-chain fixtures must be on the boot Data
  volume, and the fixture's directory-budget pool is a genuine internal pool.
  Prospective, UNMEASURED fit: 300 seconds, 512 MiB monitored scratch including
  cleanup, one owned 128 MiB image as both the ownership and the non-Data
  control, staging at two Cargo jobs, and the private pool bootstrap's ONE
  product Cargo child (its own process group, two jobs, its time and scratch
  declared in the same allocation; no other fixture runs Cargo). Its seven cases
  prove a MIXED batch — a registered profile group and a listed receiptless
  legacy root, each with multiple missing ancestors on Data, are candidates while
  a marker-recorded conflicting checkout volume and a symlinked ancestor are
  ordinary per-root keeps in the same complete snapshot — completing one real
  private prune through the journal, terminal receipts and tombstones with no
  intent left and both kept roots byte-identical; the exact list still being
  required; a missing ancestor below the attached mount kept by the leaf-only
  rule and an absent mount name kept in both `/Volumes` spellings; a non-directory
  component kept with unchanged bytes; an existing ancestor replaced by a
  different empty directory after the real run's own snapshot refused by the
  witness comparison (with checkout recreation refused as present as its distinct
  control, and the exclusively created fixture marker reset and bound to each
  race's own root); the debug-only `OXIDE_TEST_PRUNE_EACCES` injection firing and
  keeping its root against a positively proven uninjected control, with no real
  unreadable directory; the unknown-custody fault keeping the whole batch with
  the kept root's group rows, receipt bytes and registry bytes unchanged; and, on
  a positively absent missing-chain candidate, another checkout's live reference
  keeping it as ONE per-root keep in a complete snapshot while a mismatched group
  identity and a mismatched receipt each refuse the whole batch — every control
  leaving no intent, quarantine, tombstone or changed root byte. Every negative
  case asserts the exact source sentence that refused it and unchanged root
  bytes, and no case claims arbitrary-creator exclusion.
  Child custody is latched before any fallible cleanup and applies to EVERY
  child, not only an armed one: a signalled exit, any of the product's own
  unconfirmed-closure sentences (also swept from each child's recorded stderr),
  a killed or unreaped child, an unconfirmed helper closure, a non-empty session
  or any recorded custody fault marks the run's closure unknown, and ONE shared
  decision then refuses the ordinary detach (the backing is kept) and stops
  diagnostic continuation — and SETUP stops at the first latched fault, before
  any image attach, registration or fixture. Every restore (image mount
  directory, registry bytes, receipt bytes, checkout chains) goes through the
  same gate, so no fixture is written beside an unclassified writer group, and a
  classified child fault stops the run before its next fixture effect. That
  bookkeeping covers only the children this script started and is never
  presented as a process census, and the ORDER is stated honestly: the proof
  detaches from its own children's outcomes and exits, while the qualified
  supervisor's session census runs only after its leader has ended — so that
  census is authoritative for the run's reported closure but cannot retroactively
  retain an image the proof already detached, and no such retention is claimed. A
  writer that escaped its parent's group is outside the cooperative custody
  boundary both this proof and the product document. The detach itself follows
  the Oxide Supervisor's pre-detach protocol (shared `ImageCustody` in
  `scripts/test-outputs-prune.py`, opt-in so the other proofs keep their
  historical behaviour): the image is positively rebound from a fresh
  `hdiutil info` for THIS backing and a fresh `diskutil info` for its bound
  device (backing path inside the never-auto-cleaned evidence directory, mounted
  entity, bound device, volume UUID and whole-disk device), with the backing's
  recorded `(dev, ino, size)` identity — taken at attach and kept on the custody
  object — compared at that rebind AND again immediately before the pathname
  removal, so a replaced backing is retained and never unlinked by name; the
  proof's own session is censused with a bounded readable `kinfo_proc` snapshot where only
  the script itself may appear, ONE ordinary `hdiutil detach` of the bound
  device runs (never `-force`, never retried), the mount and device must then be
  positively absent, the session is censused again, and only after both clean
  censuses is the evidence backing removed and
  `removed-after-clean-pre/post-census-and-ordinary-detach` recorded. An extra
  member, an unreadable row, a row-bound hit, a rebind mismatch, a failed detach
  (including EBUSY) or a lingering device all retain the image, its backing and
  its record, with the members recorded. A process that calls `setsid` leaves
  the proof's session and is outside the census — the coordinator's own residual
  limitation — so no claim is made that such a writer is observed.

  **Authority and fit reconciliation (pre-detach protocol).** The protocol and
  its in-process census are adopted under ROOT's proof-custody apparatus
  clarification (brief v8), which itself records the Oxide Supervisor's #135
  ruling. It is proof-side custody observation only: it adds no supervisor (the
  fixed `scripts/bounded-test.py` remains the sole supervisor), no product seam,
  no persisted format, no watchdog or signal timer, and it never signals or kills
  a census member. `ImageCustody` stays opt-in: a caller that does not select the
  protocol receives the historical attach behaviour exactly — no `diskutil`
  volume probe and no new record fields — and may not take a custodied detach
  (`refused:no-protocol`).

  Every identity compared at the rebind is the ORIGINAL attach-time observation:
  image path, backing `(dev, ino, size)`, whole-disk device, mount and volume
  UUID. A missing original identity is Unknown and is never filled in from a
  fresh detach-time observation; a whole disk or volume UUID that differs from
  the attached one refuses; the backing's original identity is re-checked again
  immediately before the pathname removal, and only then is a backing inside the
  evidence directory removed. The guard's own post-leader census remains separate
  closure evidence and never authorizes a backing removal in an unknown branch,
  and no backing is ever placed in guard scratch.

  The prospective fit absorbs all of it inside the published envelope — the
  detach phase carries the rebind probes (two bounded read-only `hdiutil info` /
  `diskutil info` calls) and the two bounded censuses (30 s of the 300 s, with the
  supervisor contingency reduced to 20 s; 512 MiB unchanged) — and remains
  UNMEASURED. Outstanding ROOT-owned activations: the uncounted diagnostic, the
  counted author native and the distinct counted reviewer native are each still
  unactivated, so no native credit exists at any head and the Elevated merge hold
  stays. Heavy admission is tested normally at activation; the earlier
  "Queue #381 pending" note is withdrawn. A child that is still
  running is waited for, never mistaken for a stuck one: the armed races mutate
  the fixture while the product is inside its own 11-second delay. Each
  state-mutating case puts its own fixture back (checkout chain, replaced
  ancestor, vanished child, registry and receipt bytes) so a later batch sees
  exactly its own candidate, and NO fixture is touched while custody is unknown —
  that state is retained for the next operator and the collection aborts. An
  uncontrolled candidate set aborts the collection instead of being read as a
  case result. The uncounted diagnostic records each case and continues only into
  the two read-only cases after a recoverable failure; every other case mutates
  shared fixture or authority state, so its failure aborts rather than
  continuing into a state this run cannot prove. The pure check also parses this
  script's own source: the dispatcher must match the published case table in
  order, and every helper whose value is unpacked must be unpacked into the
  number of values it returns.
  `effigy rust:prune-absence:diagnostic` is the same guard, root, image and
  limits with `--diagnostic`: an uncounted keep-going collection with a bounded
  `qualified: false` receipt in the evidence directory, never a counted PASS.
  `effigy proof:prune-absence:test` runs only the pure `--self-check`: the real
  oracles, the mount-spelling classifier and the case/allocation table with
  in-memory counterfactuals, staging nothing and starting no child. The script's
  docstring carries the nine-premise dispositions and the authoritative case
  table. **No native has run at any head**: the author's diagnostic, counted
  author and distinct counted reviewer natives are each separately
  owner-activated (ELEVATED: this changes deletion authority), so its fit is
  unmeasured and this is not a benchmark or an alias for full QA. No live prune,
  host install, mode, settings or consumer change is authorized by this
  implementation.

  **Spent native start (preserved, not credit).** Under an explicit ROOT
  activation the ONE uncounted diagnostic ran once at `fe3228b` and stopped with
  exit 1: guard receipt `44ec0dab…` (run_id `b1907e58753d75b6c0b91a61f7803349`,
  closure closed, status 1, **no deadline**, 48.37 s of 300 s, reserve minimum
  2221.2 GiB over 191 samples, owned peak 279.2 MiB of 512 MiB, post-leader census
  certain with zero members and zero uncertain) and proof receipt `40037b5c…`
  under `/private/tmp/oxide-proof.l82e88u0/evidence`, both kept untouched. Five
  cases completed and produced the first native evidence of the protocol: the one
  real private prune deleted two roots whose checkout was positively absent (a
  registered Data missing-chain profile group and a receiptless legacy Data
  missing-chain root; `0` by exact intent replay) while the SAME complete batch
  kept six live checkouts plus a recorded-volume conflict
  (`…not on the checkout's recorded volume`) and a symlinked ancestor
  (`…not a directory`); the image-custody record `e3bbb547…` shows the full
  pre-detach protocol completed end to end (original identity rebind, clean
  pre/post session censuses, ordinary device detach, positive absence, backing
  removed only then, no stale volume, image or process). `witness-races` then
  failed on a FIXTURE ORACLE, not on the product: the recreate race's real run
  correctly refused under publication custody with "the checkout reappeared
  before the prune" and "every candidate root was kept by a fresh check before
  its intent; no output root was changed" (root unchanged, no intent, exit 2),
  while the oracle demanded the dry-run eligibility sentence "the checkout is
  present, so its root is kept". The oracle is now the pure `race_refusal_ok`
  predicate (both real-run sentences required, the eligibility keep explicitly
  rejected) with a pure counterfactual built from the spent run's own recorded
  text, and the live-checkout keep keeps its separate `dry_run_live_keep`
  oracle. That start is SPENT: it is not a counted PASS and grants no credit. The
  author's counted native, the distinct counted reviewer native and any fresh
  diagnostic each still need their own ROOT activation after an independent
  exact-head source-fit review.

  **Second spent native start and the group-mismatch fixture (preserved, not
  credit).** A second, separately activated uncounted diagnostic ran once at
  `c8fd10b` and stopped with exit 1: guard receipt `5f9697bf…` (run_id
  `c94e227c05fc987784aadf7378ee0154`, closed, status 1, **no deadline**, 51.62 s
  of 300 s, reserve minimum 2221.32 GiB over 204 samples, owned peak 278.84 MiB
  of 512 MiB, post-leader census certain with zero members) and proof receipt
  `4ba6241c…` under `/private/tmp/oxide-proof.cz_9ao0b/evidence`, both kept. Six
  of seven cases COMPLETED, including the whole `data-missing-chains` mixed batch
  with its real prune, the mount/volume boundaries, the scoped EACCES pair, the
  kept-group custody refusal, the corrected `witness-races` (its real-run refusal
  `the checkout reappeared before the prune` plus
  `every candidate root was kept by a fresh check before its intent; no output
  root was changed` matched the oracle, `prune135-033/034.stdout`), and the
  competing-owner per-root keep (`another checkout's live reference also names
  this root`, complete snapshot, zero candidates). The image-custody record
  `e2f61dca…` again shows the full pre-detach protocol end to end. The run then
  failed on the `competing-owner-and-mismatch` fixture, not on the product: the
  group sub-control edited a selected row's `nonce`, which the earlier
  receipt-placement cross-check (`prune.rs:2435-2454`, exactly one selected row
  per checkout/member/root with the same `generation` and `nonce`) catches before
  the selected-group identity loop (`prune.rs:2478-2490`) can run, so the batch
  was correctly refused as incomplete authority but with
  "a receipt reference is missing or contradictory in the registry"
  (`prune135-035.stdout 8bb1abb6…`) instead of the group sentence. The fixture
  now edits a genuinely GROUP-ONLY field, `root_ino` — the receipt's `Selected`
  reference carries only kind, root, volume_uuid, nonce and generation, so no
  receipt reference changes and only the group gate can fire — the group oracle
  keeps requiring exactly `selected group references disagree about root
  identity` with zero candidates and no effect, the separate receipt sub-control
  keeps requiring `the registry row does not match the checkout's committed
  receipt`, and a pure counterfactual proves both edits and both distinct
  sentences (each oracle rejects the other's). That start is SPENT and is not
  credit.

- **Receiptless legacy roots.** An operator-listed root that no registry row names is a candidate only when it sits directly inside the registered volume's `oxide-outputs`, carries a well-formed owner marker for that volume, its checkout has no receipt, journal or live reference, the checkout's actual leaf is absent and every writer is closed. Before its intent, the writer helper exclusively creates a `pruning` receipt bound to the intent (no adoption history is reconstructed); every bind, adopt, automatic adoption, detach, clean and replay refuses that state. The intent adds a temporary pruning row within the live caps. If the intent is not published, the receipt is discarded with no output effect; an intent-phase rollback removes both. Completion ends the receipt in `pruned`.
- **Tombstones.** After a positive completion (both the registry terminal state and the receipt's terminal state committed), the completed row and the settled intent leave the live registry in one revision, and a diagnostic entry is appended best effort to `outputs/tombstones.json` (`oxide.outputs.tombstones/1`, at most 16,384 entries and 4 MiB, oldest compacted first). The ring is never a safety journal or a live cap; unfinished intents and reservations stay in the registry and are never compacted. A recreated root always gets a fresh nonce.
- **Replay detail.** An `intent`-phase journal with its original root still present and unchanged is rolled back exactly on replay; that is not a completion, and the root is re-evaluated in the same run against the plan. An `intent`-phase journal whose root is instead found under its own quarantine leaf with the recorded device, inode and marker (a crash between the rename and its journal phase) resumes deletion. Any other unfinished phase is resumed only from its exact recorded identity, otherwise it stays pending. A symbolic link inside a quarantine stops the deletion and leaves the intent pending.
- **Proof status.** The one activated uncounted diagnostic (oxide#152, head `d52d58e5`) was SPENT and is not a PASS: case 01 failed on a fixture premise and cases 02–22 were skipped as its dependents, so `pre_counted_diagnostic` remains insufficient. The finding was a fixture, not a product, defect: case 01's version-1 fixture planted nested roots under the image's `oxide-outputs` first, so Python created that intermediate directory 0755 and the product's own `volume::dir_at` owner-only predicate correctly refused it. Case 01 now creates that container exclusively and owner-only before any nested root; an unexpected pre-existing container refuses instead of being adopted or chmodded, and the pure `--self-test` drives that seam with in-memory effects (including the old 0755 counterfactual and the refusal). A second activation (diagnostic 2, same head later corrected) then failed case 01 on a stale registry oracle: the product at this head writes `oxide.outputs.registry/2` with schema 2 (initialization creates that pair and a profile receipt promotes a legacy 1/1 document), while the case still expected registry/1 with schema 1, so cases 02–22 were skipped again and the recovered-artifact control was still unreached. Case 01 now requires the exact current pair plus `complete: true` through one extracted `registry_is_current` predicate; the legacy 1/1 document remains readable by the product as a different truth and is never accepted here, and the pure selector pins the pair against the frozen `prune.rs` literals so a product bump fails the proof loudly instead of being followed silently. The same review pass corrected two more current-write field paths of that class: a profile receipt's reservation is root-level (`reserved`) and only the legacy `/2` document carries a per-placement one, and a profile receipt places one link per member so the registry records one row per member for one root (case 03 now requires the placed member set, not a single row). The pure selector ties both rules to the frozen serializer and row-key literals and refuses the stale shapes. The third activation (diagnostic 3, head `7493674`) then reached and satisfied the recovered d246d400 artifact control for the first time (cases 01-03 completed: the current control/registry formats, the member-keyed rows and the genuine older-binary refusals), and failed case 04 on `no reservation: None`. That is a semantic premise, not a field path: the frozen product publishes no pre-lease reservation for a REQUESTED profile member whose committed placement is external while the command's fresh selection is not - binding's P4 precondition, no authority change (oxide#133 case14). Cases 04/05/22 now use an ACTUAL eligible reservation path instead: a committed profile receipt on the healthy registered volume with the selected root leaf gone, so the fresh selection is still external and its root does not exist, which is exactly where `reserve` publishes a fresh nonce before the slot key (two concurrent starters share it, `cancel-reservation` cancels it and a stale starter refuses before Cargo, and a sticky-incomplete registry makes that publication carry its overflow tag with no invented row). The old corrupt-volume/local-fallback premise is retained as a NON-RESERVING control inside case 04: no reservation, no receipt byte changes and the prune preview keeps the root. Diagnostic 3 also left the owned image attached when case 04 faulted with two paused starters unclosed; the proof now allocates the image backing and its custody record under the guard's never-auto-cleaned evidence directory BEFORE attachment, verifies that path against the guard's own published topology (one 0700 owned run root, then a scratch leaf and the evidence directory created by the guard without explicit modes, at the umask default 0755 here): this user's real canonical directory that is not group/other WRITABLE, a sibling of the exported scratch on the same device, with the containing owned root also this user's and not group/other writable as the anti-replacement boundary (0755 evidence passes; 0775/0777, a foreign owner, a writable owned root, a symlink or a device mismatch refuse), then positively re-verifies the created backing (real regular file, this user, not group/other writable, inside that directory on its device) immediately before attach, retains the raw attach plist before parsing it and requires exactly one mounted entity carrying both a mount-point and a dev-entry whose mount is this run's expected owned `/Volumes/<name>` (a real APFS attach returns four entities for one image), keeping every returned entity and refusing an unclassifiable document as UNKNOWN with the backing kept, records exactly one ordinary-detach disposition in a `finally` path (never replayed, never forced, the backing removed only after a completed detach with the mount and device positively gone), and releases and awaits every keyed starter in its case's failure path; the earlier retained-scratch incident and the separate host closeout stay historical facts, not success. The fixed 22-case native proof is `effigy rust:outputs-prune` (`scripts/test-outputs-prune.py` under the qualified shared supervisor `bc24b238…`, the merged oxide#151 supervisor with exact bootstrap `585e3395…` and suite `37ea34f0…`, with `--root-parent /private/tmp` — the owner-accepted internal-placement exception, decision `bd357a4c` v4 — because cases 10 and 21 prove checkout absence on the boot Data volume and the fixture's directory pool is a genuine internal pool; 360 s (a root-approved larger bound before any activation; static estimate 154-270 s unmeasured), 512 MiB monitored scratch, one owned 128 MiB image). `effigy rust:outputs-prune:diagnostic` is the same script, guard, root and limits with `--diagnostic`: the uncounted keep-going collection required before a counted run. It records reached/completed/failed/dependent-skipped/unexecuted case ids and the latched first fault in a bounded `qualified: false` receipt beside the guard's, continues only into cases whose declared dependencies completed, skips every later case once a failure leaves fixture, registry, writer or journal state uncertain, and aborts on an unknown writer, device, custody, protocol, infrastructure, deadline or receipt outcome — never promoting an unknown to a pass and adding no second supervisor, session, group or accounting harness. Its dispatch is one table shared with the counted run, whose default stays fail-fast. `effigy proof:outputs-prune:test` runs only the pure `--self-test`, which drives that table, the fail-fast dispatcher, the keep-going/taint/abort precedence and the bounded receipt with synthetic bodies and touches nothing. `--plan` prints its allocation and cases, and a nine-premise sweep records its private-`HOME`/rustup, account-home, environment-isolation, explicit-adoption, owned-image (`-owners on`), flag, temporary-storage and `mktemp` dispositions. The older-binary fence control's input is the recovered, independently hash/size verified copy of the exact released `d246d400` control-2 artifact at `/Users/tom/.local/share/northstar/release-audits/oxide/2026-10-25/d246d400-artifact/old-oxide-d246d400` (sha256 `bd52cb42…`, 2756576 bytes, owner recovery audit `152-recovered-artifact-owner-verification.json`; the custody JSON beside it is provenance only and nothing in it is trusted). It is read together with its original install receipt `/Users/tom/.local/state/oxide/install/20261023T095314Z-d246d400f5cc.json`, whose recorded `installed.path` is the historical `/Users/tom/.local/bin/oxide` — that path now holds the current 4df3dfd binary (sha256 `b42b46a4…`) and is provenance text only, never opened, read or copied. The two paths are separate constants, the identity checks require the receipt's pin, SHA-256 and size plus the read bytes' own SHA-256 (no-follow, pre/post identity rechecks), and only a private copy inside owned scratch is ever executed for the control-state-2 probe and the two refusals. A different identity at either input aborts naming the requirement instead of retargeting, stubbing or rebuilding; the recovery itself is an input repair, not proof or resource credit. Its case 21 is the version-1 compatibility scenario (oxide#152): a real whole-target `oxide.outputs.transition/1` receipt and a `root/1` marker seeded from the product's own receipt seed, the checkout deleted, a valid noncommitted control whose receipt bytes must not change, a dry run that must change no byte, the exact-preview real run that prunes only the committed legacy root and publishes the current receipt shape, and the prerecorded operator list that legacy identity still requires. That scenario shares the script's boot-Data-volume absence proof (case 10), whose receiptless legacy roots are case 10 while containment and reserve are case 12, and its source fit is UNMEASURED. The script docstring carries the authoritative case id list and dependency matrix, and its `--self-test` fails if either drifts from the dispatcher. It uses debug-only hooks absent from release builds: `OXIDE_TEST_PRUNE_CRASH` (`intent`, `renamed`, `deleting`, `terminal`), `OXIDE_TEST_PAUSE=<reserved|keyed|bound>:<gate>`, `OXIDE_TEST_PRUNE_LOW_RESERVE_AT=<n>` or, exclusively, `OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_ENTRIES=<n>` (the n-th check, or the first check after n entries were deleted, reads zero free space once, under the same supervisor layout guard, recorded in evidence) and `OXIDE_TEST_PRUNE_EACCES=<dir>`, which injects EACCES when the checkout walk reads inside that directory, only under the shared supervisor's deadline and canonical scratch/evidence layout and only for a canonical path inside that scratch, and records each firing in the evidence directory. The proof creates no real unreadable directory. Case 01 also runs a root-authorized read-only copy of the exact installed d246d400 binary (receipt pin, SHA-256 and size verified; only the copy executes, in the private fixture): its capability probe reports control state 2 and its `check` and `outputs adopt` refuse with fixture bytes and metadata unchanged. **Control-format pin scoping and the spent `4d5e47b` packet (oxide#216).** A control document's numeric version names the highest fence that document carries
(`Store::save`), never a binary-wide maximum: an internal pool that admitted a build
is fully fenced at the current version, while a paired removable control that only
received the owned-outputs fence from an initialization stays at the
recorded-accounting version. Case 01 therefore derives each control's expected
version from that document through one `control_state_version` predicate and scopes
the fully fenced maximum to the control that actually admitted a build; the pure
selector pins the whole ladder (`SLOT_CEILING_STATE_VERSION`,
`EVICTION_ACCOUNTING_STATE_VERSION`, `RECORDED_ACCOUNTING_STATE_VERSION`,
`PREVIOUS_STATE_VERSION`, `OLDER_STATE_VERSION`, `LEGACY_FIELDS_STATE_VERSION` and
the `Store::save` `else if` chain) to the product source. The first diagnostic at
`c06cebb` failed before any case: its declared candidate capability was control state
4 while the staged binary reported 9, so staging refused and all 22 cases were
unexecuted. The pinned head `4d5e47b` then ran one uncounted diagnostic and one
counted author native, and BOTH failed at case 01 on the paired removable control's
`state version 5` because the assertion had pinned 9 for both controls (so no clean
diagnostic exists at that head; the diagnostic reached the same assertion the counted
run did). Both packets are spent historical evidence with no native credit, retained
at `/private/tmp/oxide-proof.c54y64f5/evidence/outputs-prune-diagnostic.20261101T164746Z.7e576c.json`
(SHA-256 `4d4b954e…`), the `4d5e47b` diagnostic receipt
`/private/tmp/oxide-proof.yb9a02ax/evidence/outputs-prune-diagnostic.20261101T172717Z.607630.json`
(SHA-256 `156b0a30…`, 22/22 recorded, case 01 failed, the rest skipped) and the counted
run's supervisor receipt
`/private/tmp/oxide-proof.sv1cd9mi/evidence/receipt.json` (SHA-256 `51cb5549…`, run
`b2c078b2…`, closed, status 1, no deadline). **Classification of the `6798e5b` case-08 failure (oxide#216).** At that head one uncounted
diagnostic (`/private/tmp/oxide-proof.0qnj0n2i`, receipt SHA-256 `55c10fdb…`, source
`6798e5b`) completed cases 01–07 and failed case 08 with an empty candidate set, and the
counted author run at the same head (`/private/tmp/oxide-proof.16zijqo1`, supervisor
receipt SHA-256 `e99b216c…`, stdout SHA-256 `3f26dcdf…`) failed identically; both are
spent with no native credit. Classification from source: the stale-device
reconciliation cannot produce this. Every prune-side change it makes *removes* a
refusal (observation device equality, selected-group device equality, candidate
and plan device equality, journal replay device equality) or refreshes recorded
state; none adds a `snapshot_error`, and the one new refusal (a row with no recorded
device) is per-row. So the empty set is either a snapshot-level refusal of this
fixture state (the whole batch keeps every root for one shared reason) or a per-root
keep of the one eligible root — and this head is the first ever to execute case 08,
because every earlier head failed at case 01, so that case's premise was never
proven. The retained receipts cannot decide between the two: the failing assertion
printed only the candidate set, and a whole-batch refusal is indistinguishable there
from a per-root decision. The proof therefore now makes the premise explicit —
`World.complete` requires a COMPLETE preview for the canonical dry run,
non-reserving control, eligibility, live detached, released vanished, registered
image, and case-12 reserve and the pre- and post-symlink controls. Those assertions use
`preview_report`'s bounded root→reason map; the corrupt-receipt case requires the
whole batch. Other candidate and keep assertions do not all enforce completeness or
report the map, so the proof does not claim that coverage throughout. This correction
is diagnostic and premise-tightening only: no product behaviour is changed. The owner-activated `6d572ba` diagnostic closed its guard but failed case 04: its
control preview kept the whole batch for duplicate or contradictory group references;
cases 05–22 were skipped and no counted native was spent. Registry publication had a
separate duplicate-row path: a missing pending reference could append an already
matched selected member. It now adds only the missing pending reference; the focused
regression checks a shared selected group with one stale device. The exact cause of
the recorded case-04 packet remains subject to a fresh diagnostic, not assumed proven
by the regression. The `e76407d` diagnostic closed its guard but failed case 10:
a listed foreign root lacked an owner marker, so incomplete authority kept the
whole batch; cases 11–22 were skipped and no counted native was spent. The case
now separately checks whole-batch refusal and nonmutation for a foreign root and
a forged checkout digest, then tests the eligible boot-Data root alongside the
per-root image-checkout keep with a complete preview. This is fixture correction,
not product evidence. The `4c028a2` diagnostic completed cases 01–11 but failed
case 12: the intentional symlink replacement of a recorded root made authority
incomplete, as the no-follow observation requires. The alpha and beta lines in
its bounded report carry that shared batch reason; they do not identify which
root caused it. Case 12 now requires a complete eligible kappa before and after
the replacement, and a whole-batch refusal while the symlink exists. The refusal
checks the exact link text and a no-follow snapshot of the real root and every
entry (metadata, file digests and link targets), plus control bytes and journal
state; the pure self-test rejects deleted-content and retargeting counterfactuals.
Cases 13–22 were skipped; the packet is uncounted, and a fresh diagnostic
remains required. No live prune, dry run,
`resolve-intent`, mode, consumer or pool mutation is authorized by this implementation.

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
  2, strict FIFO waiting, idle-slot eviction before waiting. The host-wide
  limit uses the approved physical-core default and 1–32 explicit overrides
  ([storage and limits](#storage-and-space-management)). Coordinates with
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

### Foreign churn in the supervisor census

A PID is not an identity: the census identifies an occupant by `(pid, start)`.
A successful safe reread showing no row or a different start proves only that
the old identity is absent at that observation. It does not establish the
replacement's session or prove the PID stays empty. A failed or malformed
read establishes neither absence nor a foreign occupant. A successful getsid
sandwiched between matching identity reads binds that session observation to
that identity; it cannot bind an observation across PID reuse. Start-token
ordering, a readable foreign parent, and exiting or zombie state alone do not
establish foreignness.

The current ESRCH path can drop the old tracked identity when confirmation or
its final reread proves absence; a replacement is not classified by inference.
The ordinary successful-getsid path retains a tracked old identity or marks an
untracked changed identity uncertain. The next snapshot can classify a new
occupant, but a replacement observed after the snapshot must not be silently
omitted from a deciding empty round. A candidate correction must explicitly
account for that interleaving; merely dropping the old identity on a changed
reread is insufficient. Any bounded retry that cannot classify each observed
replacement must leave the round uncertain. A later clean snapshot is a new
observation, not proof that an earlier unknown was foreign.

| Observation | Defensible result |
| --- | --- |
| Same identity, bound leader session | Member, retain tracking, even in another group. |
| Same untracked identity, bound other session | Foreign under the existing ordinary rule. |
| Tracked identity, contradictory or unreadable session | Keep attribution; no foreign inference. |
| ESRCH, same identity or failed reread | Existing terminal rule if all its strict premises hold; otherwise tracked member or uncertain. |
| ESRCH, safe reread absent or replaced | Old identity absent at reread; replacement remains unclassified. |
| Successful getsid, changed identity | The session belongs to neither identity with certainty; retain tracked old attribution or mark uncertain. |
| Reused parent, orphan, unreadable identity or session | No launcher exclusion without its exact structural proof; use ordinary conservative classification. |

Counterexamples to test include replacement after an ESRCH confirmation but
before the final decision, repeated PID reuse between each bounded reread,
parent reuse between sibling observations, a same-session different-group
member, and a tracked identity whose later session appears foreign. A joined
census thread, the final and named certain-empty census messages, pipe EOF and
identity-bound custody remain independent closure requirements; no proposed
classification grants signal authority. Zombie terminal treatment retains its
existing allowlist and full paired-read requirements, not a generic exiting
shortcut. These observations record the original design investigation; the later oxide#233
candidate below remains unqualified.

**Recommendation: no-go on either papercut shortcut.** A foreign parent's
session does not bind its child's session: a child may enter the supervised
session, and a parent PID can be recycled. Neither an exiting flag nor an
unpaired zombie observation proves a nonwriter. The existing allowlisted
terminal rule is narrower and remains unchanged. Waiting for a quiet host is
not a correctness fix. Nor does dropping a changed identity and assuming a
subsequent round will run work: the custodian may decide on this round, and a
post-stop uncertain message latches the fault. In particular the current
ESRCH replacement branch records Gone and can report a certain empty round;
`test_final_replacement_tracked_not_foreign_by_inference` tests a later round
but does not require it before that first round can decide. This is a design
concern requiring independent review, not a claim that the historical packets
contained a supervised replacement.

The smallest candidate worth evaluating is a **bounded replacement worklist**
within the same census round, for both ESRCH confirmation/final replacement
and successful getsid identity-change paths. Remove only the positively absent
old `(pid, start)` from tracking; immediately carry every observed replacement
as an unclassified identity, including one from the final read. Apply ordinary
identity-bound session classification to that replacement, with the existing
ancestor/launcher rules only when their original structural premises can be
proved from appropriate fresh observations. A positive member is tracked by
its own identity even if a subsequent read contradicts it. A bound foreign
session can exclude only an untracked replacement. A fresh observed absence
can resolve that identity, not future occupants. Each further replacement
enters the same worklist; cap total identities and reads per round, never
recursively or without a fixed ceiling. Exhaustion, malformed data, failure or
an unclassifiable occupant leaves the round uncertain, not empty. Preserve
already tracked identities until their exact absence is established. The
implementation brief must settle the concrete limits and the safe interaction
with shared confirmation passes, receipt caps and census deadline. If that
cannot be achieved without weakening the existing final/named census or
fault-latching contract, retain the current conservative behavior and stop.
The #214/#212 persistent same-identity ESRCH case remains unresolved by this
candidate; no bounded reread can turn the same non-zombie row into proven
absence. This remains a design subject to independent exact-head review.

**Bounded implementation contract (oxide#230).** This design is against
`78364942c8dec9253d557cfc63a7775fe9284848`, runner SHA-256
`fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b`,
suite SHA-256 `c95d46aa0e590285c3c412294ffb215c0d45b19ba804f2243f0d2ebd62158d7b`,
and this section before this amendment. It records the frozen design; the
subsequent oxide#233 candidate is not yet qualified. The following fixes the open choices in the candidate above.

**Candidate implementation (oxide#233).** The branch implements same-round
replacement accounting using the ledger, shared read budget and fixed phase
clock described below. The receipt and census message carry bounded
`replacement_work` evidence, while the uncapped ledger determines obligations;
a late publication refuses certainty. The original snapshot/confirmation/final
zombie witnesses and the exact named and final census, EOF, joined-thread and
fault gates remain independent. The qualification suite currently contains
406 pure cases and 59 real cases by its loader: the two new real cases label
synthetic replacement observations alongside the actual kernel snapshot. They
are not kernel PID reuse, and no signal authority follows. The 382 pure / 57
real #197 claims remain historical at their own bytes.

**Failed diagnostic and fixture attribution correction.** The one Queue-activated
uncounted diagnostic at `e6ed727` recorded 464 cases, 460 completed and four
failed: both labelled synthetic replacement cases and the one- and twelve-held
zombie witnesses. Its candidate guard closed with status 1, no fault or deadline,
and monitored scratch peaked at 68,931,584 of 536,870,912 bytes. The failed
packet is preserved: `cases.json` SHA-256
`2c7d557182e3cef2ed84b9323a4611149678c66bc73bc7c879521037b5f050ab`,
`topology.json` `7fee338b62238f8af34594f8cfac0d0e7b7ff8d1f91d1f039b2c60afeb258e85`,
and guard receipt `3d918de9af3a6672f5c95203397a7b7190f60fb925446c9610a1497f9689bf1e`
under `/Volumes/Builds/scratch/oxide/oxide-supervisor-qualify.l1rp7zy1` and
`/Volumes/Builds/scratch/oxide/oxide-proof.rz6m7o9t/evidence`. Its four nested
receipts show more than 1,100 snapshot identities and 40 or more ESRCH rows:
the unchanged 20-row work/diagnostic prefixes omitted the named replacement or
held zombies, and no terminal witness fit in the shared diagnostics budget.
The counts and closure do not by themselves attribute an owned identity.

The corrected candidate adds an opt-in, test-only projection of at most twelve
fixture-named exact identities from the completed round ledger and its actual
terminal witness records. Missing or unmatched identities remain `unobserved`;
no ambient row substitutes for the named identity. The projection consumes no
kernel read and changes no classification, certainty, cap, signal or closure
decision. The two synthetic fixtures and the held-zombie cases assert that
projection; the old clipped rows, omission counts and negative controls remain.
The independent source and proof judgment and any fresh exact-head proof route
remain with the same retained Queue reviewer and owner; this correction supplies
no native qualification and does not reset the failed session.

**Corrected-head diagnostic and nested-record schema.** The separately activated
uncounted Queue diagnostic at `535ebbcb` exercised 465/465 cases successfully,
including all four previously failing fixture cases. The candidate guard closed
with status 0, no fault or deadline, and peak monitored scratch of 67.8 MiB.
The diagnostic still failed: 11/12 checks passed, with `nested-cases` rejecting
the two synthetic cases' `replacement_fixture` field as unmodelled. The packet
at `/Volumes/Builds/scratch/oxide/oxide-supervisor-qualify.lli9efki` and
`/Volumes/Builds/scratch/oxide/oxide-proof.ukf8izwb/evidence` is retained;
`cases.json` SHA-256 `b22135446fdb9fe6bc94bdbd0a1d2165125308ca74d2a65d03b3c381931aa89f`,
`topology.json` `6fd46774800df97d70593a93211e5c3541d53792fd962bed4742f95af3e3dc9e`,
and guard receipt `6d12aad516bde8dedaae50eb2daedb7d98d0b0bf4d5735a56d17cc426321b4bc`.
This is an uncounted failed diagnostic, never qualification credit. The next
source correction permits that field only for the two named synthetic cases;
the independent cases oracle still checks its exact contents and no-signal
condition, and all nested-root and receipt checks remain in force. Pure controls
also reject this field on other cases, malformed values, missing evidence,
outside-root paths and unknown fields. No native run or allowance reset follows
from this source correction; any further proof needs the Queue owner and the same
retained reviewer's exact-head judgment.

**Qualified default and isolated candidate (safe landing).** The ordinary
`scripts/bounded-test.py` is restored byte-for-byte to the deployed qualified
`fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b` source.
The unchanged oxide#233 candidate `c578243126a2470cde0dfd19530254b3ec61b4ea5bb1fa43c6bbd2d41853c2b7`
resides at `scripts/bounded-test-candidate.py` and is used only by the existing
qualification bootstrap, suite, preflight and pure tests. Those routes bind
both fixed paths and hashes, refuse absent, changed or swapped bytes, and never
fall back to the ordinary runner. Ordinary selectors, wrappers and consumer
imports retain their existing default paths and pins. Promotion of this candidate
is a separate release step after actual qualification and is not authorized here.

Queue diagnostic `0e148446` at source `8b4c2e0`, independently judged by the
same retained reviewer (review `6567647646`, verdict `v10-fresh-diagnostic-8b4c2e06`),
completed 465/465 cases and 12/12 checks with zero problems; the candidate guard
closed status 0 with no fault or deadline, 419.38 s worker wall, 71,045,120 bytes
peak monitored scratch and 288.3 GiB minimum sampled free. Intentional exit 1 and
`qualified: false` remain: this is uncounted evidence, not a counted qualification,
promotion or consumer proof. Its retained `cases.json` SHA-256
`2a9983f387703ca1654401ba4fcecf3d16bbae47712d70677e618ae9f57f34c9` and
`topology.json` `4b32f4a1b64b2d50fc3b5b70735ba806fc6cd3dd58abbf20b3f2b30903b23b9e`
are in `/Volumes/Builds/scratch/oxide/oxide-supervisor-qualify.iy_vrps8`; the
nested guard receipt SHA-256
`bfaa86c19b82f5ee0acb3ad6d1e50816f3efd05b8128c7d4fce65198d003f97d` is in
`/Volumes/Builds/scratch/oxide/oxide-proof.eziis49o/evidence`. The reviewer
assessed the pre-existing fixture-thread traceback separately (verdict
`v11-traceback-assessment`); that assessment does not migrate evidence to this
relocated source head. The earlier failed packets remain unchanged. The retained
reviewer must judge this routing revision; any changed-head diagnostic is separately
owner-activated under its fixed limits and cannot qualify deployment by itself.

The relocated source at `0ddc30650dbdb26ec5dd875d9cc9926e6f082b61` was then
tested by Queue proof-only diagnostic `41eef47b`. Its retained reviewer accepted
465/465 cases (406 pure and 59 real), 12/12 checks and zero problems, with the
candidate closed at status 0, no fault or deadline, 421.7 s worker wall,
69,967,872 bytes peak monitored scratch and 300.5 GiB minimum sampled free.
The guard receipt SHA-256 is
`01482d23938e1f614999cb3dee543d9b9aefe573aeff275ebb0d26b9d6687f1d`, retained
at `/Volumes/Builds/scratch/oxide/oxide-proof.kh7tju10/evidence/receipt.json`;
the matching `cases.json` SHA-256 is
`ea3099ff2b0bcf29850f90b3c36e5f4ac4d91bef9a159d0fc7d078d158f7582b` in
`/Volumes/Builds/scratch/oxide/oxide-supervisor-qualify.jveh3rik`.
The twelve printed check rows were captured by Queue rather than retained as
a separate disk receipt; the reviewer re-derived their inputs from the kept
artifacts. Intentional exit 1 and `qualified: false` remain: this establishes
the relocated diagnostic route, not counted qualification or consumer promotion.
The default qualified runner remains `fcfda2b0`; retained nested fault fixtures
and all historical failures stay unchanged.

**Counted candidate qualification.** The counted selector
`effigy proof:supervisor:qualify` ran once at the same `0ddc306` source in
execution `a7c3c9da`. Its actual receipt schema emits `result: "pass"` and
`problems: []`; counted mode does not emit a top-level `qualified` property.
Only diagnostic mode adds `qualified: false`. A future qualification brief
must use that actual schema, together with the source, case, custody and
resource oracles, rather than demand a nonexistent `qualified: true` field.
The exact manifest is bound by the fifth independently verified Queue input
and clean source commit; the printed `sha256` object binds the candidate,
qualified default, suite and bootstrap.

Planner release-gate acceptance `9575afc9-4a46-4302-8f33-c660977454cd`
accepts this counted evidence for the isolated `c5782431` bytes. All 465
outcomes passed, with no problems, candidate closed at status 0, no fault or
deadline, a certain empty deciding census and no survivors. Worker wall was
417.07 s, peak monitored scratch 71,032,832 bytes and minimum sampled free
304.9 GiB. Nested fault fixtures and the bootstrap base remain retained;
both startup sweeps removed nothing. The original combined log at
`/Volumes/Builds/scratch/oxide/qualification.BRF2xdS5/qualification.log` has
SHA-256 `56010506433b3eff6ba49bd62991e893942d7da856e236ae36a3496446ee80b9`;
its 166,100 bytes were outside guarded accounting. The matching guard receipt
at `/Volumes/Builds/scratch/oxide/oxide-proof.dgi2n6oi/evidence/receipt.json`
has SHA-256 `c2764ebcf79a41598a32ea5630198a9abeb02d467c251a1360d8cae785de6323`.
No receipt, missing field or reviewer finding was rewritten by the acceptance.
It qualifies only these bytes within the declared cooperative macOS scope;
it supplies no stronger group-lifetime or historical-survivor closure claim.
The ordinary default remains `fcfda2b0`. Using the qualified candidate's fixed
path in a new proof needs its own scoped consumer disposition and pin audit;
qualification alone performs no global promotion or consumer repin.

The proof-authoring premise sweep for the new cases follows their executable
route: `Real.replacement_fixture` calls `Real.guardian` with `fixture("pass")`;
that fixture is Python-only and makes no Oxide, Cargo or rustup call. Therefore
private HOME/rustup and lock generation, Oxide account-home override, inherited
Oxide configuration and automatic adoption, Cargo external argv, and compiler
flags are not applicable to these case bodies. They create no image, so the
owned-image attachment premise is not applicable. `Real.setUp`, `nested_root`
and `Real.guardian` place their `tempfile.mkdtemp` case directory and nested
supervisor evidence inside the qualification-owned root; no shell `mktemp` is
called. Existing suite cases and the enclosing bootstrap retain their own
premises, bounds and custody rules. This source sweep is not a native diagnostic.

These candidate bytes have the counted qualification and attributed planner
acceptance recorded above. Main's `fcfda2b0…` runner remains the deployed
default route; no prior receipt migrates. Under Tom's task-specific
ruling `c8991f99`, the single retained Queue reviewer judges the exact published
source and, after source approval, one separately activated source-immutable
uncounted diagnostic. Qualification and active-consumer safety are distinct
planner-owned release gates; the later counted execution is a separate
qualification disposition, not an additional reviewer native. The 600 s outer /
560 s candidate / 530 s suite bounds,
512 MiB monitored scratch and host custody remain unchanged. The persistent
same-identity non-zombie ESRCH (`37529b20`) and recycled-identity report
(`631e4462`) gain no retrospective closure or signal authority.

The project planner froze this bounded design on 2026-11-01 after independent
exact-head review `6565343955` of `ecd2129b771b74e7a29ff6f38687cad88e9418ca`.
The freeze authorizes a separately scoped implementation brief, not a native
start, qualification, consumer repin or new signal authority. The earlier
candidate's open bounds are superseded only by this bounded contract.

Each round owns one FIFO of exact identities and one ledger, independent of
receipt storage. Admit at most 4096 distinct snapshot identities and 128
additional distinct replacement identities; duplicate `(pid, start)` work is
coalesced, but a new start at the same PID consumes another replacement slot.
Duplicate snapshot PIDs or malformed identities invalidate the round; never
use a partial snapshot to remove tracked identities. A repeated observation of
an already resolved identity reopens it if its prior disposition could be stale;
retain its ledger entry and charge all new reads. Never discard an obligation
because its diagnostic row was omitted. A snapshot beyond the cap is uncertain,
with tracking retained. These are conservative availability limits, not a claim
about host process counts.

Charge every attempted process-table snapshot, PID identity read, getsid,
getsid/group read used by structural rules, and parent identity revalidation
(including failed calls) to one 131072-call round budget, before the call.
One snapshot counts as one call; each separately invoked reader counts once,
including both sides of a paired observation. No helper may bypass the ledger.
The budget accommodates 4096 existing rows at 20 confirmation reads plus a
final read (86016), leaving 45056 calls for session, structural and replacement
work; it is a ceiling, not a promise that every maximal round completes. Keep
20 confirmation attempts and 19 shared 50 ms pauses, plus the separate final
read. Start one 2 second monotonic phase deadline at round entry, never renew
it for replacement or a pass; check before and after each reader, pause and
publication. A call that returns after the deadline invalidates the round. The
2 seconds leave margin inside the existing 3 second join; neither deadline
interrupts a blocked kernel call, so an expired join still refuses closure.
The outer settlement and custody deadlines remain authoritative and are never
extended. Budget, identity-cap, deadline, malformed read and interrupted work
all set `certain: false`, `uncertain >= 1`, and invalidate terminal counts;
retain any tracking not positively resolved. Do not start an unbudgeted final
read in an attempt to rescue exhaustion.

Algorithm: validate the complete snapshot and seed the FIFO in snapshot order.
For each identity apply the existing structural rules only with their exact
fresh identity and parent proofs; a replacement has no inferred parent proof
from the old row. Otherwise read identity, getsid, identity in that order,
charging each; only matching complete identities on both sides bind the session.
A bound leader session tracks the identity, including a different group; a
bound foreign session excludes only an untracked identity. On actual ESRCH,
perform the first confirmation immediately after getsid and pair it with that
identity's own initial row. Pending identities share the existing spaced passes;
late arrivals join the next pass and receive their own 20 attempts only if the
single deadline and budget permit. Do not restart the 19-pass schedule: a late
arrival unable to complete its required reads is uncertain. Run final reads
for eligible unresolved ESRCH identities; drain replacements from *all* first,
shared, final and ordinary post-getsid reads through the same FIFO before
publishing. Every safe different-identity read proves only the old identity
absent at that instant: remove only that exact tracked identity and enqueue the
new complete row. A `None` resolves only the identity read, not future occupants.
Never transfer a session, terminal witness, parent proof or tracked attribution
to a replacement. A tracked identity remains a member despite foreign or
unreadable session until exact absence; terminal acceptance remains subject to
the existing allowlist and every paired read, with contradictions tainting the
count. If a queued identity changes again, enqueue the new identity under the
same caps. Deduplication does not erase a later observation or its obligation.
No recursive retry or fresh clock is permitted.

A round can publish certain empty only after the queue and pending/final reads
are drained and every observed identity has a positive disposition. Record a
bounded `replacement_work` summary with snapshot and replacement admitted,
reads consumed, pending count, status (`complete`, `identity_cap`, `read_cap`,
`deadline`, `invalid`, `interrupted`) and capped identity rows plus omitted
count. Counts and unresolved status come from the ledger, never the capped
rows; overflow or inability to serialize the summary is uncertain. Diagnostic
omission alone is not an exclusion. The deciding message carries the resulting
certainty and counts and is bound to the monitor's exact sent sequence as today.
A named certain-empty message cannot cure a later nonempty or uncertain final
round: both must independently be certain and empty, the census thread must
join within its existing bound, both required EOFs must be observed, and any
latched fault vetoes cleanup. Missing/unmatched messages, expired join, missing
EOF, or a post-decision uncertain/nonempty result refuse closure and keep
custody; no new signalling authority is created.

| Failure or observation | Round and closure consequence |
| --- | --- |
| First/shared/final read sees B after A | Resolve only A; B is an obligation in this round. |
| A→B→C exceeds any cap or deadline | Uncertain, even if the receipt omits C; no cleanup. |
| Bound foreign B, untracked | B may be foreign; a tracked B remains a member. |
| Bound same-session B | Track B; report member even if later reads contradict it. |
| Parent reuse, unreadable/malformed row or failed binding | No structural inference; unresolved work blocks certainty. |
| Zombie contradiction or missing paired read | No terminal acceptance; retain tracking or uncertainty. |
| Named/final divergence, expired join, missing EOF or fault | Refuse closure, keep evidence and custody. |

The minimal subsequent implementation slice changes the census and its monitor
message/receipt handling in `scripts/bounded-test.py`, pure and controlled real
fixtures in `scripts/test_supervisor.py`, and owning workflow knowledge. It
must inventory `effigy.toml` and consumer hashes; consumer pin reconciliation
is separately scoped, not implicitly authorized. Pure controls must discriminate
the old certain-empty final-replacement behavior, exercise A→B→C exhaustion,
first/shared/final replacements, foreign and same-session B, tracked A and B,
parent reuse, malformed and unreadable rows, terminal contradictions, named
versus final divergence, expired join and missing EOF. Controlled real fixtures
exercise attribution and closure without relying on kernel PID reuse, including
a same-session different-group survivor and no-signal control. The separate
persistent same non-zombie identity with getsid ESRCH (`37529b20`) remains
unknown/UNCLOSED; related recycled-identity paper `631e4462` and historical
packets gain no closure or credit. This slice is HIGH/ELEVATED because it changes
writer closure. The original design's separate reviewer-native requirement was
superseded for oxide#233 by Tom's task-specific ruling `c8991f99`; the fixed
qualification bounds remain unchanged.

| Interleaving / counterexample | Required disposition |
| --- | --- |
| Snapshot A; getsid; reread B at same PID | A absent at reread; getsid may belong to A or B. Classify B independently before an empty decision; never transfer A's session or tracking to B. |
| A tracked; B appears during confirmation or final reread | Drop only A once replacement is safely read; account for B in that round, tracking B if bound to the leader session. |
| B changes to C while B is classified | Account for C in turn; if bounded work is exhausted, uncertainty blocks closure. |
| Same untracked A, foreign session bound to A | Foreign under the ordinary rule, even if its parent is unknown; a foreign parent alone proves nothing. |
| Same tracked A, foreign session, orphan, changed parent or different group | A remains tracked; same session with a different group is still a member. |
| Parent PID reused, parent read fails, or sibling session/group read changes | No structural exclusion; ordinary classification applies, and an unreadable child stays uncertain. |
| ESRCH, unchanged non-zombie A or an exiting flag | No disappearance or terminal proof; tracked A remains a member, otherwise uncertain. |
| Allowlisted complete zombie, all paired reads unchanged | Existing terminal nonwriter rule only; retained attribution survives and eligibility is recalculated each round. Failed, changed or non-zombie reads disqualify it. |
| Interrupted worklist, failed snapshot or malformed reread | Unknown, never foreign or absent; retain attribution and block certainty. |
| Certain empty named message but later final census sees member or unknown, or join expires | Refuse cleanup; named and final messages must both be certain and empty and join must finish. Missing either pipe EOF also vetoes closure. |

The receipt evidence for command-budgets at `122493b` and removable-removal at
`73ff05b` is respectively
`/Volumes/Builds/scratch/oxide/oxide-proof.qv4pc4xe/evidence/receipt.json`
(SHA-256 `8ed940705529e0afcba79585fae184768d2088039fdfaec8730a386d994a84d7`)
and `/Volumes/Builds/scratch/oxide/oxide-proof.a746ilr6/evidence/receipt.json`
(SHA-256 `1d25ad0db500b7a0cd9956dd85b47ab93d77c1af672c0dd0b5880828eabd6bbf`).
Both record status 0 and UNCLOSED with two uncertain rows, PIDs 23420 and
23421, `zombie: false`, 20 matching confirmations and a same-identity final
read. The removable script's own result says passed and requested detach; it
does not prove supervisor closure or detach. Queue's papercut attributes the
rows to foreign Effigy test activity, but the receipts alone do not bind their
sessions. The separate #192 recycled-PID report identifies a changed token;
it does not establish which identity owned the getsid observation. All these
packets and their original conclusions remain historical.

**Future validation and rollout.** A separately approved HIGH complexity
implementation brief must apply the proof-authoring premise checklist and
specify time and scratch bounds, including fixed per-round worklist/read caps.
Pure regressions must drive the real classifier through the table above,
including replacement at the first, shared-pass and final reads; A→B→C cap
exhaustion; tracked A and B; session/read failures; ancestor and parent PID
reuse; zombie rule contradictions; named/final census divergence, join expiry
and missing EOF. Include a counterfactual proving the old certain-empty
replacement case fails the new oracle. Real qualification must use small owned
fixtures and test-only controlled seams to exercise replacement attribution and
closure end to end, alongside actual same-session different-group survivor,
foreign churn, EOF veto, join and no-signal controls; do not depend on forcing
kernel PID reuse or a quiet host. Distinguish simulated interleavings from real
kernel observations in receipts. Run the covering selectors, then the one Queue-activated uncounted keep-going
diagnostic at the exact published head after retained-reviewer source approval,
under the 600 s outer / 560 s candidate / 530 s suite and 512 MiB bounds,
reserve watchdog and custody rules. Tom's task-specific ruling `c8991f99`
supersedes the original separate author and reviewer native sessions for this
task; further qualification is a planner-owned release gate. Any changed bounds
require an explicit brief amendment, not a reset. This changes writer-closure
decisions and is Elevated. No native is authorized by this design alone.

The candidate runner remains unqualified until the applicable qualification
requirements are met. Inventory direct consumers in `effigy.toml` and all hash checks at the
implementation head; update each applicable pin only under its own approved
scope, with new consumer evidence where required. At this source head explicit
`fcfda2b0…` checks occur in `test-command-budgets.py`,
`test-holding-delete.py`, `test-outputs-prune.py`,
`test-recorded-slot-accounting.py` and `test-slot-headroom.py`. Older explicit
checks are `d89115f1…` in `test-accounting-guards.py` and
`test-status-fast.py`, and `8e379c93…` in `test-accounting-vanished.py`,
`test-storage-layout.py` and `test-max-active.py`; other scripts and
`effigy.toml` contain historical digest declarations, so reconcile those
separately rather than blindly replacing strings. Consumer-specific limits,
read-only sibling boundaries and adoption holds remain in force. Old receipts
stay bound to their actual runner bytes and limits; repinning source does not
migrate proof credit or authorize reruns, installation or consumer adoption.
