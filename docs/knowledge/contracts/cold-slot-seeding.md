# Cold-slot seeding

This contract owns cold-slot seeding: how a newly leased slot is warmed by
cloning dependency compile units from one positively idle donor slot on the
same volume, and the safety rules that bound it. Slot leasing, the ledger,
origins, accounting, eviction and output placement belong to
[local-lease](local-lease.md) and the [workflow](workflow.md); seeding does not
change them. Cargo alone decides whether a copied unit is fresh.

**Status: implemented and default-on.** `COLD_SEED_QUALIFIED` in
`src/bin/oxide/main.rs` is true, so seeding runs unless `OXIDE_COLD_SEED` is
exactly `0`. The items under [Not implemented](#12-not-implemented) are out of
scope. This contract states no measured speed, reach or disk saving.

## 1. What seeding does

- It runs once per command, after a new slot is leased and its output binding
  is complete, and before any child temporary directory or Cargo start. It adds
  no lease grant, no retry and no admission budget.
- It applies only to a new slot: `Prior::Fresh`, with `<slot>/build` empty.
- It copies the classified dependency files of one donor catalog (plain
  registry library compile units, see section 3) with `clonefile`. There is no
  byte-copy, hard-link or shell-copy fallback.
- It writes nothing else. It never adds a fresh mark, edits a fingerprint,
  mtime or dep-info file, changes a Cargo unit graph or output format, or
  touches config, flags or caches.
- It never writes the slot key, the ledger, the origin format or the donor. The
  recipient's origin stays untrusted until its normal release writes a `Built`
  origin.

Never seeded: workspace and path units, final binaries, test and bench
executables, uplifted outputs, incremental directories, `target/`, and any
build-script run unit or its output.

## 2. Compatibility

A donor is eligible only when each dimension below is recorded on both sides
and equal. Missing, malformed or unknown is never assumed equal.

- Pool payload root and device. On the removable pool, also the verified
  volume binding (mount point and volume UUID).
- `rustc -vV` digest and the `RUSTC` value.
- Cargo version (verified release) and the pinned layout adapter (section 10).
- Target triple, Oxide recipe key, backend, verified Cargo home, and command
  family.
- The producer context digest (section 2.1), and the compiler, wrapper and flag
  facts below.

Skipped, with a named reason and a cold build:

- a configured compiler (`build.rustc`, `CARGO_BUILD_RUSTC`);
- any `RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`
  or `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`, and a kept inherited wrapper
  (`OXIDE_ALLOW_RUSTC_WRAPPER=1`). Only the default bypass policy seeds;
- `CARGO_PROFILE_*` and `CARGO_TARGET_*` overrides beyond the recipe's own
  variables, and `--config` arguments;
- `[source]`, `[patch]`, `[registries]`, `[unstable]` and any `replace-with`
  source replacement;
- `exec`, external subcommands and unsupported command families.

Not dimensions: project identity, checkout path, the owned-output destination
(finals never move), feature selection and test flags (Cargo decides; unmatched
copies stay inert), and donor rank, idle age or size (used only for ordering).

Recorded compiler and flag facts are digests. The compiler selection, the full
set of compiler flag variables (`RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`,
`CARGO_BUILD_RUSTFLAGS`, `RUSTDOCFLAGS` and their variants) are compared by
presence and exact value digest.

### 2.1 Producer context digest

Cargo's unit fingerprint does not bind the whole inherited environment, and the
[Cargo version matrix](workflow.md#cargo-version-matrix) names the inputs the
slot key misses. The context digest therefore follows Cargo's own lookup:

- Walk from the command's working directory to `/`, collecting each
  `.cargo/config.toml` and `.cargo/config`. Then add `$CARGO_HOME/config.toml`
  and `$CARGO_HOME/config`. A closer file wins. Where one directory holds both
  names, Cargo's pinned precedence applies, or the seed skips.
- Reduce each file to `[build]`, `[profile]`, `[target]`, `[env]` and `[net]`.
  Resolve relative `[env]` values against the directory of their config file.
  Any table the adapter does not model, and any unreadable, oversized or
  duplicate-key file, skips.
- The digest is SHA-256 over the sorted effective merged values, plus each named
  environment variable as `absent` or a digest of its exact bytes.
- Bounds: 16 config files, 4 MiB of config bytes, 128 KiB of rendered records.
  No symlink is followed and no file is created.

## 3. Eligible units

A package is eligible only when all of these hold, on each side independently:

- Its normalized name (`-` replaced by `_`) maps to exactly one lock entry
  across all source kinds. A collision, such as `foo-bar` beside `foo_bar`, a
  registry entry beside a Git entry of the same name, or two versions, makes the
  name ineligible.
- Its lock entry has no dependency edges at all, normal or build. A proc macro
  can only be invoked through a dependency, so a leaf cannot run one. Most
  packages are therefore not eligible.
- Its source is a crates.io-style registry entry with a recorded checksum. Git,
  path, workspace, vendored, source-replaced, `[[patch]]`-replaced and
  sourceless entries are ineligible.
- Its verified manifest declares exactly one plain library target (not a proc
  macro) named by the normalized name. A genuinely renamed target is ineligible.
- Donor and recipient agree on version and on source plus checksum.

A unit is offered only when its complete set is present:

- Fingerprint triple in `<profile>/.fingerprint/<literal package>-<16hex>/`:
  `lib-<target>`, `lib-<target>.json` and `dep-lib-<target>`. The package
  directory uses the literal lock name; the unit suffix uses the normalized
  name.
- Artifacts `<profile>/deps/lib<target>-<hex>.rmeta` and, when the producing
  command emitted one, `.rlib`. `cargo check` emits the metadata only, and a
  check-built donor is a supported donor, so an rmeta-only unit is offered
  whole. Every recorded artifact must be joined by its own rule.
- The join file `<profile>/deps/<target>-<hex>.d`. Every rule is parsed, and
  rule order is irrelevant. Output rules must name the `.d` or an artifact and
  stay inside the unit's output directory, and their prerequisites must lie
  entirely inside the verified source root. rustc's empty phony source rules are
  accepted only when they match an accepted prerequisite inside that root. Any
  other rule, a duplicate artifact claim or an unclassifiable entry makes the
  package ineligible.
- The 16-hex unit identifier in the fingerprint directory, the artifact stem and
  the `.d` stem must be the same value. Oxide never predicts that hash.

**Layout v2 units.** Under `cargo-1.100.0/3acc79c0c/build-dir-v2` a library unit
is the directory `<profile>/build/<literal package>/<16hex>/`. It is offered only
when it holds exactly the entries `fingerprint/` and `out/`, and:

- `fingerprint/` holds the triple `lib-<target>`, `lib-<target>.json` and
  `dep-lib-<target>`, and may hold `output-lib-<target>` and `invoked.timestamp`.
  Any other entry makes the unit unusable.
- `out/` holds the joining `<target>-<hex>.d`, at least one of
  `lib<target>-<hex>.rmeta` and `.rlib`, and any rustc split-debuginfo objects
  (`<target>-<hex>.<codegen-unit>-cgu.<index>.rcgu.o`). Any other entry or a
  file that cannot be verified makes the unit unusable.

Layout v2 clones every file in an eligible library unit with its own digest
check. The `.d`, `.rmeta` and `.rlib` must satisfy the same join rules as
layout v1. Rustc leaves the split-debuginfo objects out of its dep-info rules,
so Oxide checks their names against the unit and verifies them by digest. The
existing file and byte caps apply to the complete unit. A profile that also
holds layout v1 output (`deps/` or `.fingerprint/`) is refused.

Several fingerprint directories for one package are per-unit variants. A
run-only directory is ignored and never copied. A directory that mixes units or
holds an unclassifiable entry is unusable. Build-script, unit-grouped, custom
`--out-dir` and custom extra-filename layouts skip.

Never offered in v1: build-script compile and run units, proc macros, Git and
Git-revision sources, path and workspace units, any unit whose `.d` names a
file outside the verified source root (including generated `OUT_DIR` inputs),
and final, test, bench or incremental outputs. Packages with build scripts
rebuild, and their dependents rebuild as Cargo decides. Each excluded package
is named in the skip output, and the command continues as an ordinary cold
build.

## 4. Source verification

**Registry.** The cached `.crate` archive under
`$CARGO_HOME/registry/cache/<index>/` must hash to the lock checksum. The
expected extracted tree is derived from those archive bytes by a strict
in-process reader (`flate2` with the Rust backend and `tar`, both pinned in
`Cargo.toml`). Only regular files and directories are accepted. Symlinks,
hardlinks, device nodes, duplicate paths, absolute or `..` paths, and
unparseable entries refuse. The extracted tree must match that derivation
exactly, ignoring Cargo's own `.cargo-ok` marker. `.cargo-checksum.json` is not
evidence.

- A missing archive, an ambiguous index directory, a vendored or
  source-replaced source, or an oversized tree makes the package ineligible.
  Nothing is fetched, repaired or re-downloaded.
- Verification runs before and after the producing command, and again at
  donation. A source file whose mtime is later than the producing start drops
  the package.
- Results are cached in `seeds/sources.json` (at most 2048 records, 8 MiB),
  keyed by archive device, inode, size and mtime, and by archive SHA-256. Any
  identity change forces full re-derivation or a skip.

**Git.** Product lock parsing rejects Git sources before any verification, so
nothing seeds from Git. The dormant Git verifier, its hint discovery and its
closure checks were removed (red-pen 1, oxide#340); seeding is registry-only.

**Residual.** A same-user writer that changes a source during the producing
build and restores the old bytes, preserving mtime and size, is not detected.
Seeding relies on the same cooperative same-user model as Oxide's closure and
origin handling. It is not an adversarial fence.

## 5. Donor catalog

### 5.1 Production

A catalog is written only by a producing epoch. All of these must hold:

- The lease created the slot as a new, empty `<slot>/build`.
- The generation's seed-attempt record is absent, or in the terminal state
  `skipped` or `reset`. A generation that seeded anything, or whose attempt is
  still `intent`, never publishes. A seeded recipient therefore never becomes a
  donor.
- The primary child was Cargo itself, the command family is supported, and the
  command succeeded.
- The command's identity and context facts are recorded, and Cargo is the
  pinned verified release.
- The slot is released with a recorded `Built` origin.
- The catalog is built from post-Cargo verified sources. A source first seen
  during the build is accepted only after the release-time unchanged check.

The catalog records each offered file's SHA-256, computed at producing time. A
unit without every digest is not offered. Excluded packages are counted in a
bounded rejection record. A package that cannot be classified is recorded as
unknown, never as zero.

The file is `<pool>/oxide-managed/seeds/<slot-id>.catalog.json`, schema
`oxide.cold-seed.catalog/1`, with unknown fields rejected. Caps are 2 MiB, 2048
packages, 8192 file entries and 3 units per package. A catalog that would exceed
a cap is not written, and the slot never donates.

### 5.2 Lifetime

A catalog is valid only while the ledger shows its slot `Idle`, with no lease id
and `last_lease_id` equal to the catalog's producing lease. Every new grant mints
a new lease id, so the first reuse invalidates the catalog. A later build never
refreshes an older catalog.

### 5.3 Over-offer

A donor offers every unit it holds for the recipient's allowlisted packages.
Oxide does not compute Cargo's package-id hash, unit metadata hash or
fingerprint directory name. Cargo decides which copies it uses. Unused copies
stay as counted bytes and seeding itself deletes nothing: the slot is reclaimed
by the whole-slot Barge eviction every slot is subject to. Layout-v2 warm-slot
stale-unit pruning is a separate pass on **release**, not on a seeded
recipient ([layout v2 pruning](workflow.md#stale-unit-pruning-layout-v2)).

## 6. Recipient transaction

### 6.1 Preconditions

Each is checked read-only. Any failure is a skip, and the command builds cold.

- The recipient is a new `Prior::Fresh` slot. `<slot>/build` is an owner-only
  real directory on the pool's device with no entries.
- Cargo is a verified release with the pinned adapter. The command family is
  supported, and the command is not `exec` or an external subcommand.
- The recipient `Cargo.lock` is a regular owner file, at most 4 MiB and 2048
  packages, and parses. It is read only. `--locked`, `--frozen`, `--offline` and
  config arguments are never altered, and nothing writes a lockfile.
- The allowlist is non-empty, and at least one catalog overlaps it.

### 6.2 Sequence

1. **Read-only preparation.** Read the lock, compute the allowlist and context
   digest, and read the candidate donor ids by rank. Take no locks and write
   nothing.
2. **Admission lifetime.** Take the removable sibling barrier when applicable,
   then the store admission lock. Hold both to the end of the transaction.
3. **Candidate read.** Under the pool lock alone, confirm the recipient is still
   `Held` by this lease, and read at most 8 generation-checked catalogs chosen by
   rank. An idle slot whose catalog is missing or ineligible is skipped without
   consuming that budget, and no more idle slots are examined than the pool's
   slot ceiling. Release the pool lock. Take no donor lock and mutate nothing.
4. **Selection.** Use catalog data only. Filter by context digest, intersect the
   allowlist, apply the recorded join, and choose one donor by greatest overlap,
   then rank, then slot id. Selection reads no donor payload.
5. **Reserve gate, then durable marks.** Sample the start gate only when the plan
   is non-empty (section 8). Then write the attempt record as `intent` and the
   untrusted origin, synced. A crash from here leaves a slot that no later
   command reuses as warm.
6. **Seed transaction.** Take the donor record lock without blocking and without
   creating the lock file, then the pool lock. Re-read the donor catalog and
   reject it if its generation, lease id or bytes changed. Recheck the donor
   predicate and the recipient's `Held` generation. Open and hash every planned
   donor file under the pin, and recheck its `.d` rules against the recorded
   join. Clone with `clonefile`, hash each clone against the same digest, and
   confirm the donor is still `Idle` and generation-matched.
7. **Post-clone walk, before Cargo.** Walk the recipient's slot with the bounds in
   section 8. Publish only on a complete, ownership-clean walk, and record it as
   an `observed-while-held` fact. That fact is never a closed record and never an
   admission figure.
8. **Release order.** Drop the pool lock, then the donor record lock. Write the
   terminal attempt state (`seeded` on success) and the last-seed sidecar. Then
   drop the admission lock and the barrier. The recipient record lock belongs to
   the command and is held until it exits.
9. **Publication.** On success, print the summary line and run Cargo.

### 6.3 Donor predicate and lock order

A donor is eligible only when all of these hold, judged from the ledger and
control record under the locks. Nothing is recovered to make a donor eligible.

- Ledger state is `Idle`, with no lease id, and `last_lease_id` equal to the
  catalog's producing lease. The slot is not `Held`, `Evicting` or retired.
- Control record is `Idle`, with no holder, no pending recovery, no retired or
  lost marker, and no recorded `oxide clean` for its checkout.
- The donor's record lock file exists and is free. A missing, held or recovering
  donor is skipped, never waited for.
- Donor and recipient payload roots are real owner-only directories under the
  same root, on the same device. On the removable pool, the volume binding must
  also match.
- The donor is never written: no rank, last-used, origin, holder, phase or
  touch.

Lock order, outermost first: recipient record lock; removable sibling barrier
(when applicable); store admission lock; pool lock for the candidate read alone;
then donor record lock (non-blocking), then pool lock for the transaction. Locks
release in reverse order. Any competing acquire, eviction or release between the
two library calls is caught by the catalog re-read, the ledger recheck and the
digest checks. The seed then aborts rather than read or clone a slot that is no
longer positively idle.

The pool lock is bounded in work (at most 4096 files, 384 MiB and 8 catalogs,
with one donor pin), not in wall time. A blocked filesystem call cannot be
pre-empted. Passive status reads never take these locks.

## 7. Outcomes

| When | Outcome |
| --- | --- |
| A precondition, cap or deadline fails, or the donor is busy or its custody unknown, before the first write into the recipient | `oxide: seeding skipped: <token>`. The attempt becomes `skipped`, and the command builds cold in the same empty slot. |
| The reserve gate is below the trigger or unknown, before the first write | Refuse before any child. No marker and no clone are written. |
| After the first write, the post-clone walk is incomplete or fails (bound, ownership, device, boundary or read) | Abandon seeding only. Publish nothing. Release the recipient with no trusted origin, through the ordinary release (a removable pool flushes before idle, and a failed flush retires the slot). Never reuse it. Build cold in a newly admitted slot under normal admission. No extra grant. |
| After the first write, any other fault: clone, digest or file-type error, a changed donor file, donor transition, reserve breach, lost reading or unreadable path | Record `reset`, meaning abandoned and not repaired. Release the recipient with no trusted origin. Refuse with a non-zero exit. Cargo never runs in that slot, and seeding deletes nothing. |
| Unknown ownership, accounting, device or custody fact | Refuse. An unknown fact is never a cold fallback. |
| Recipient `build/` is not an empty owned directory | Refuse. Nothing is cleaned. |
| Removable volume lost | The existing removal and quarantine path runs. |

A refusal reuses no lease, adds no grant and does no rebind. The retained
generation is counted by the next admission walk, or reclaimed by the
whole-slot Barge eviction that treats it as `Prior::Unknown`. It is never read
as zero.

Crash points: a crash after the untrusted origin, during the clone, or during
the release leaves the untrusted origin and attempt record in place. No trusted
origin is written, and the next command evicts the whole slot through the
ordinary whole-slot Barge eviction. A crash after `mark_spawning` but before the process
group is recorded stays held for explicit recovery, as in
[crash recovery](workflow.md#crash-recovery). Seeding adds no deletion
mechanism of its own.

## 8. Reserve and bounds

**Reserve.** The internal `directory-budget-v1` pool uses the watchdog's own
reader and trigger, `HOST_RESERVE_BYTES` plus `MARGIN_BYTES` (22 GiB). The gate
is sampled only with a non-empty plan, before the attempt record and first
clone. During the clone and any abandon, the seed samples at the watchdog's
interval. A breach, an unknown reading or an unreadable reading aborts the seed,
retains the untrusted generation, and refuses.

The removable `apfs-volume-v1` pool has a removal-only watchdog with no space
trigger. The seed verifies the mount and volume-UID binding and the
registration's reserve before its first write, and monitors the binding during
the seed. Loss follows the removal path, not a generic cold build.

A command that seeds nothing never samples this gate, so its behaviour is
unchanged. The seed's gate is a precondition of seeding alone.

**Budgets.** The command's admission and execution budgets are never extended.
The seed's own cooperative work budget covers donor revalidation, cloning and
digest verification; it is a ceiling a caller may lower and never a figure that
extends admission. Expiry before the first recipient write means no seed and the
same slot builds cold. Expiry between the first write and the last verified
clone records `reset` and refuses: the untrusted generation is retained and
released without a trusted origin, never repaired (section 7). Once every
planned clone has verified, the work budget no longer refuses a seed, because
no clone work remains; the post-clone walk keeps its own separate bound. The
pool's space budget stays soft, as it is for Cargo.

| Item | Bound |
| --- | --- |
| Seed attempts per command | 1, no retry |
| Donor catalogs read | 8, each at most 2 MiB |
| Recipient `Cargo.lock` | 4 MiB, 2048 packages |
| Producer context config | 16 files, 4 MiB of bytes, 128 KiB rendered |
| Seed plan | 256 units, 4096 files, 384 MiB logical bytes |
| Artifact `.d` join file | 1 MiB |
| Seed work budget | 5 s for donor revalidation, clone and digest verification; a caller may lower it, never raise it |
| Post-clone walk | 2,000,000 entries, depth 64, 60 s |
| Source-verification cache | 2048 records, 8 MiB |
| Seed sidecar family | 24 MiB of the four known families; at most 8 orphaned sidecars or abandoned temp files removed per write |

## 9. Persistence and visibility

**Sidecars.** Four families live under `<pool>/oxide-managed/seeds/`. They are
owner-only, written by temp file, rename and sync, and never stored in the
payload:

- `<slot-id>.catalog.json`, the donor catalog;
- `<slot-id>.attempt.json`, the per-generation attempt record;
- `last.json`, the last completed seed (schema `oxide.cold-seed/1`);
- `sources.json`, the source-verification cache.

An interrupted atomic write leaves one `catalog.tmp-*` file. It is not a
sidecar: it never counts against the family ceiling, and a later write reaps
one older than an hour. A newer temp may belong to a concurrent writer and is
left alone.

Each binds the store id and generation, and is read only when it matches the
current ledger generation. A missing, oversized, malformed, symlinked,
foreign-owned, unknown-version or mismatched sidecar is ignored. It is never
repaired or adopted. The attempt record is the only sidecar whose absence is a
positive fact, meaning an unseeded legacy generation.

Attempt states are `intent`, `seeded`, `skipped` and `reset`. `reset` means
abandoned, not repaired.

**No control-state change.** Seeding adds no ledger, config, origin, marker or
state field. Therefore `STATE_VERSION` and the hand-off capability numbers do not
change. If an implementation needs a fact that cannot live outside control state,
it must stop and return to design rather than bump the format.

**Output.**

- Success prints `oxide: seeded <N> units (<M> MiB) from slot <X>`. N counts
  completely cloned compile units, and M counts logical cloned bytes. The output
  never says reused, fresh or saved, because it makes no Cargo freshness claim.
- Donor candidates rejected in the producing epoch appear as a bounded skip
  suffix. Unknown counts are reported separately from zero.
- A miss prints `oxide: seeding skipped: <reason>` with a fixed set of reason
  tokens. `unsupported unit` names a plan for a unit v1 never offers (a
  non-library role, a non-registry source, or a package that is not a unique
  leaf).
- `oxide status` shows at most one line from `last.json`, labelled as recorded.
  Default status stays free of walks and locks, and it never reconciles.

**Legacy and downgrade.** An older binary ignores the sidecars. A partially
seeded generation has no trusted origin, so the older binary evicts it by its
existing rule. A completed seed followed by a successful build is an ordinary
trusted slot. Nothing is migrated, and a pool that never seeds writes neither
file.

## 10. Cargo layout adapters

Each build-directory layout has one adapter, and the producer identity names
it. A donor and a recipient must record the same adapter, so a layout v1 donor
never seeds a layout v2 recipient, or the reverse (section 2).

| Adapter id | Cargo | Layout | Unit shape |
| --- | --- | --- | --- |
| `cargo-1.97.1/c980f4866/build-dir-v1` | 1.97.1 | v1: `<profile>/deps`, `<profile>/.fingerprint` | section 3, layout v1 |
| `cargo-1.100.0/3acc79c0c/build-dir-v2` | 1.100.0 | v2: `<profile>/build/<package>/<hex>/{fingerprint,out}` | section 3, layout v2 |

Both adapters assume that `CARGO_BUILD_BUILD_DIR` is the slot's `build/`, which
Oxide always sets.

- The layout is read from what Cargo wrote. A profile holding the other layout's
  output is refused, not guessed at. Cargo 1.100.0's private opt-out
  (`__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT`) writes layout v1 there, so a
  command that sets it is refused in the producer context and builds cold.
- Any other Cargo version, an unverified or rebuilt Cargo, or a release without
  an adapter means no seeding. A new Cargo release needs its own adapter id and
  qualification.
- Layout v2 was checked against real Cargo 1.100.0 output (section 13). A cloned
  unit directory is `Fresh` in a different build directory. Cargo rebuilds it when
  the copied artifact is missing, when the fingerprint hash is altered, or when
  the dep-info is missing. In the same checks, aging a cloned file's mtime did
  not make Cargo rebuild a registry unit, so the seed does not rely on mtimes:
  the fingerprint and the outputs carry the decision.

## 11. Interfaces with other contracts

- **Child temporary files** ([workflow](workflow.md)).
  Seeding writes only under `<slot>/build/`, before the owned temporary directory
  exists. `scratch/` is untouched.
- **Command budgets** ([workflow](workflow.md)).
  Seeding runs inside the admission window and never extends it.
- **Owned outputs.** Seeding never reads or writes member links, receipts,
  reservations or roots. The output destination is not a seed dimension.
- **Held slots.** A held slot is never a donor, because it has no closure
  evidence and a running `exec` or `run` session may itself run Cargo. A second
  command in a checkout whose session holds a slot can seed only from a
  positively idle donor.
- **Clean, prune and eviction.** `oxide clean` waits for the recipient lease, as
  before. Evicting a donor only stops its donation.
- **Hand-off and install.** Unchanged. Installation stays planner-gated.

## 12. Not implemented

- Git, path and workspace sources; build-script compile and run units; proc-macro
  units; libraries whose `.d` names `OUT_DIR` or other outside-root inputs. A
  broader scope needs a new design and review.
- Reading a held slot as a donor.
- Per-unit pruning of unused copies inside seeding, and any Cargo package-id or
  unit hash computation. The separate layout-v2 release-time pruning of stale
  units is not part of seeding.
- A config key, flag or environment toggle beyond the `OXIDE_COLD_SEED=0`
  opt-out.
- A unified start gate for commands that seed nothing.
- Any measured speed, reach or disk saving. None is claimed here.

## 13. Tests

Under the lean validation policy ([vision](../vision.md#validation-policy)), the
ordinary tests are:

- library: `src/lease/seed/tests.rs`, the layout validation in
  `src/lease/seed/catalog.rs`, and `src/seed_source/tests.rs`;
- front door: the producer tests for both layouts in
  `src/bin/oxide/seed_catalog.rs`, the layout-mismatch selection test in
  `src/bin/oxide/seed_plan.rs`, the adapter and opt-out tests in
  `src/bin/oxide/seed_context.rs`, and the seed tests in
  `src/bin/oxide/store/tests.rs`;
- end to end on macOS, `src/bin/oxide/store/seed_layout_v2_tests.rs`: a real
  Cargo 1.100.0 donor publishes a catalog and is released idle. The cold
  recipient receives every file in the registry library unit, including its
  split-debuginfo objects. Cargo reports it `Fresh`, the binary links and runs,
  and Cargo rebuilds the unit when the profile changes, an artifact is missing
  or the fingerprint is altered. The offline test uses an unambiguous registry
  source in the invoking Cargo cache. It skips when the pinned Cargo or an
  eligible cached test package is unavailable;
- default-on and opt-out behaviour (`src/bin/oxide/main.rs`).
  `tests/cold_seed_default_off.rs` builds a fixture crate on a fresh private pool
  with `OXIDE_COLD_SEED=0` and asserts a successful build, no seeding notice and
  no seed artifact. It is self-contained: no staged baseline binary and no
  `#[ignore]`. Scratch-pool status is covered separately in
  `tests/scratch_status.rs`.
