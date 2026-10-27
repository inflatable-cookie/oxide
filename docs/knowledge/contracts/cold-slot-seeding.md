# Cold-slot seeding from matching dependency units (proposed design)

Status: **design only, unimplemented, not frozen.** No product code, test
proof, selector, native execution, live pool read, installation or consumer
change is authorized by this document. It is the first deliverable of oxide#137
under the ROOT design-first rebind of 2026-10-27 (Tom and the Oxide Supervisor,
standing decision `8ddaa98d`): publish a concrete design, take an independent
exact-head design review, then stop for the Supervisor's delegated freeze.
Implementation, the author and independent-reviewer native sessions, and the
merge hold are separate and unchanged.

**Revision 4** answers the CHANGES REQUIRED review of `e10b8b39` (review
comment `6449965447`, 2026-10-27): the artifact manifests gain a per-role join
through rustc's own dep-info so no other package's file can be offered (§4.2,
§5.1); the seed runs inside one `Store::seed` lock lifetime that holds the
admission barrier and donor pin through the final verification, with the
`status --measure` claim corrected and the pool-lock hold stated honestly
(§6.3, §7); the required SQLite case keeps its real scope and a new eligible
build-script case supplies the non-vacuous offered-run exclusion oracle (§16.2);
and the proof allocation is now phased, includes the recipient's own SQLite
rebuild and deletes the proof's private pool before the image phase (§16.4).
The archive-cache and reserve-gate wording are corrected too.
**Revision 3** answered the CHANGES REQUIRED review of `c60f433a` (review
comment `6449728021`, 2026-10-27): the configuration digest now follows
Cargo's own working-directory-to-root lookup with its precedence and relative
`[env]` resolution (§3.1); dependency-unit content is bounded by a
leaf-package rule, because a proc macro's ambient reads are not fingerprinted,
and that narrowing is returned to the Supervisor (§3.2); git verification
reuses the repository's sanitized bounded `freshness::Git` protocol with
cleared Git environment and refusal on unknown closure (§4.3); and the
proof allocation is corrected for coexisting archives and extracted sources,
the new dependencies and a per-case schedule with a stated basis (§16.4).
Revision 2 answered the CHANGES REQUIRED review of `62d05c36` (review
comment `6449252752`, 2026-10-27). The amendments are: a complete per-role Cargo
unit manifest including the fingerprint directory's encoded dep-info (§4.2,
§5.1); registry source verification through the cached archive bytes and a
canonical archive-to-tree comparison instead of a self-declared checksum file
(§4.3); per-file content hashes recorded from the closed producing epoch and
revalidated at donation (§4.4, §6.3); a durable unseeded condition so a seeded
recipient can never publish a catalog (§4.1); a bounded producer-context digest
covering the compiler, wrapper, flag, profile, config and environment inputs
Cargo does not fully vouch for (§3); a reserve start gate plus monitored stop
during clone and reset, with the removable path separated (§8.3); and the real
SQLite/`cc`/`OUT_DIR` proof retained with a source-derived time and disk
allocation (§16). Each finding is dispositioned in §3.3.

Written against clean pushed `main` at
`cec65daff49a2961c9b9d1dd2cad235251c02e48`. Every `file:line` anchor below is
against that commit; re-check the anchors before trusting them after any
rebind. The qualified shared proof supervisor is `scripts/bounded-test.py`
SHA-256 `bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924`
(the earlier `d89115f1…` pin is historical and qualifies only its own bytes).
Prerequisite oxide#136 (child temporary files) is merged at the `7d6e992`
merge; oxide#155's capture-and-verify deletion is in `main`.

The direction is Tom's: a cold slot is warmed by cloning matching dependency
units from other slots on the same volume, with Cargo alone judging freshness
([vision](../vision.md#product-workflow-direction), "Cold slots"). The scope is
the Queue-held brief (version 3, sha256 `6bb370e9…`) and the architect bindings
`741268dc`, `bb151a59` and `d1f281eb` recorded in it. This document does not
re-open that scope; it settles the concrete protocol the brief left to design
and states plainly where the protocol is weaker than the direction suggests.

## 1. Outcome and non-goals

**What the design achieves.** A command that leases a genuinely new slot runs
one bounded, fail-closed seeding transaction after the slot is leased and its
output destination is bound, and before any Cargo starts. It may copy, with
`clonefile` only, the dependency compile units of one positively idle donor
slot on the same volume, when donor and recipient agree on recorded toolchain,
Cargo, target, recipe, backend and pinned layout adapter, and when each
package's locked identity is unambiguous on both sides and its source content
was verified. Cargo then decides, with its own fingerprints, whether any
copied unit is fresh. Nothing else changes: the slot key, the lease ledger,
the origin format, the freshness gate, warm reuse, accounting, eviction,
cleanup, output placement and closure rules are untouched.

**Non-goals, explicitly refused here.**

- No Oxide "Fresh" mark, no fingerprint edit, no mtime patch, no dep-info
  rewrite, no insertion into a Cargo unit graph, no second resolver and no
  `--message-format` or other output-format injection.
- No eager preparation, full-target copy or whole-slot clone: only the
  classified dependency files of the plan.
- No byte-copy fallback. If a clone cannot complete, the seed is abandoned and
  the recipient is reset or the command refuses (§6.5).
- No seeding of workspace/path dependency units, final binaries, test/bench
  executables, uplifted final outputs, incremental directories, or any
  build-script *run* unit or its output (§5).
- No donor mutation, no donor lease, rank, use-counter, recorded-accounting or
  origin write, and no reading of a held slot.
- No new config key, flag, daemon, scheduler, cache cleanup or consumer
  registration, and no storage-override relaxation.
- No general provenance claim. v1 is **partial association** over an on-disk
  subset (§4.2); a donor catalog is only as trustworthy as the cooperative
  same-user contract that already protects `oxide-managed/`.

## 2. Source, interface and format matrix

Checked against the design input. "Read" means this design reads the fact;
"gap" is what the design must add without changing it.

| Fact | Where it lives today | Design relationship |
| --- | --- | --- |
| Slot key = digest(project identity + recipe + rustc-vV digest + target + backend [+ owned-output destination]) | `src/bin/oxide/identity.rs:32-62`; stored keys are pinned by the `identity.rs:661-685` regression | **Unchanged.** Seeding never writes the key or its digest, so every stored ledger and record keeps its bytes. |
| Owned-output destination in the key | `identity.rs:49-61`, `src/bin/oxide/outputs.rs:6094` (`destination_facts`) | Retained for leasing, **ignored** as a seed dimension: dependency intermediates do not depend on where finals are uplifted. |
| Control state, phase, holder, lease id, use counter | `src/bin/oxide/store.rs:155-189` | Read to name the producing generation and to run recovery; no field added (§9.4). |
| Control-state numeric fences | `store.rs:65-83` (`STATE_VERSION = 5`), `store.rs:116-117`, `store.rs:79-80` | **Not bumped** in this design (§9.4), with an explicit stop condition if a field turns out to be required. |
| Slot origin (checkout, content, anchor, uncertain paths) and `Built` | `store.rs:251-259`; `src/bin/oxide/freshness.rs:168-200`; `store.rs:3577-3591` (`trusted_origin` needs a matching `mark`) | Read, never widened. It remains the only trust record for reuse. |
| Cargo identity | `freshness.rs:75-84` (`VERIFIED_CARGO`), `freshness.rs:419-445` (`verified_cargo`) — probed per command, **not persisted** | Recorded explicitly in the new donor catalog; legacy slots have no such fact and are not donors. |
| Encoded dep-info in the fingerprint directory | Cargo's `.fingerprint/<pkg>-<hash>/dep-<kind>-<target>`, marker `01 00 00 00 ff`, version 1; the repository's own bounded reader documents the format (`src/dep_info.rs:1-30`) and the version-matrix proof decodes every `dep-*` file (`workflow.md#cargo-version-matrix`) | Part of every offered unit's manifest (§4.2, §5.1); a unit without it is not offered. |
| Cargo configuration and environment that Cargo does not fully fingerprint | `workflow.md#cargo-version-matrix` names `build.rustc`, `CARGO_BUILD_RUSTC`, `RUSTC_WORKSPACE_WRAPPER` and a kept wrapper as outside the slot key; Cargo merges `.cargo/config*` from the command's working directory to the filesystem root, then the Cargo home (`cargo-1.97.1/c980f4866` `doc/src/reference/config.md`) | A bounded context digest over the named environment variables and config tables, looked up from the command's working directory with Cargo's precedence, must be equal on both sides; anything unsupported skips (§3.1). |
| Ambient inputs a proc macro can read while compiling a dependency | not in the consuming crate's dep-info; Cargo fingerprints only a small part of the environment (`cargo-1.97.1/c980f4866` `core/compiler/fingerprint/mod.rs`) | Eligibility is restricted to lock entries with no dependency edges, so no proc macro can be invoked (§3.2); the narrowing is returned to the Supervisor. |
| Git verification process | `src/bin/oxide/freshness.rs:453-500` (`freshness::Git`): `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `submodule.recurse=0`, `core.fsmonitor=false`, bounded calls/time/output and a settle window | Reused verbatim for every seed-side git call, with cleared `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`; unknown closure refuses (§4.3). |
| Registry source integrity | Cargo verifies the downloaded `.crate` bytes against the lock checksum and writes `.cargo-ok` after unpack (`cargo-1.97.1/c980f4866` `sources/registry/download.rs`, `sources/registry/mod.rs`); it does **not** keep a per-file manifest for registry trees | Verification goes through the cached archive bytes and a canonical archive-to-tree comparison, not a self-declared checksum file (§4.3). |
| Host reserve watchdog | `store.rs:49-53` (`HOST_RESERVE_BYTES` 20 GiB at creation), `src/bin/oxide/watchdog.rs:22-30` (22 GiB trigger, `MARGIN_BYTES`), `store.rs:1716-1726` (removable pool uses a removal-only watchdog), `store.rs:4028-4038` | The seed has its own start gate and monitored stop at the same trigger; a breach or unknown reading refuses, never a cold fallback (§8.3). |
| rustc identity | Digested into the key at `identity.rs:41-61`; not independently recoverable | The in-memory `rustc -vV` text of the current command is kept for the seed identity; the catalog records its digest. No key change. |
| Slot ledger: incarnation, state, rank, `last_lease_id`, recorded size | `src/lease/pool.rs:154-215`; `src/lease/mod.rs:343-420` | Read for donor eligibility and generation binding. No ledger field or format change. |
| New-slot creation (empty `<slot>/build` only) | `pool.rs:1128-1185` | The recipient precondition: a new record's slot directory is created empty with exactly one `build/` entry. |
| Effective build/target directories | `pool.rs:1317-1332` (`build` on the slot, `CARGO_TARGET_DIR` in the checkout); `src/bin/oxide/main.rs:232-252` (`apply_storage`) | Seeding writes only under the slot's `build/`; the checkout `target/` is never written. |
| Command order: lease + gate, output binding, child temp, spawn | `main.rs:491-500`, `main.rs:522-560`, `main.rs:594-630`, `main.rs:650` | The seed transaction slots between the output binding and the child-temp choice (§6.1). |
| Nine-grant freshness loop | `main.rs:1141-1199` | Unchanged; seeding adds **no** grant and no retry. |
| Prior classification (`Fresh`/`Known`/`Unknown`) | `store.rs:3286-3359` | `Prior::Fresh` is the only seedable recipient state. |
| Recovery, quarantine, whole-slot discard, idle eviction | `store.rs:1806-1819`, `store.rs:1829-1850`, `store.rs:1963-2003`, `store.rs:2694-2712`, `pool.rs:625-660` | Reused unchanged; the seed's crash path leans on `Prior::Unknown` eviction (§6.6). |
| Record lock and admission lock | `store.rs:3720-3760`, `store.rs:4509-4560` (`probe_lock(create: false)`) | The donor pin is a non-blocking record lock plus the library pool lock (§7). |
| Per-inode accounting, hard-link union | `src/inventory.rs:179-235`; `local-lease.md#capacity-and-eviction` | Cloned files are ordinary new inodes, counted fully. No extent-sharing discount (§8). |
| Recorded per-slot observations, admission figure | `store.rs:1064-1143`; `local-lease.md#recorded-per-slot-accounting-oxide146-implemented-native-held` | The seed's bytes enter the slot's next closed record at release; no early record is fabricated. |
| Status sidecar precedent (schema, caps, generation binding, read-never-writes) | `store.rs:1078-1143` (`oxide.status-observations/1`, 256 KiB, 128 entries) | The model for the seed sidecars (§9). |
| Child temporary directory `<slot>/scratch` | `src/bin/oxide/child_temp.rs:66-180`, `child_temp.rs:317`; `workflow.md#child-temporary-files-oxide136` | Untouched; `build/` and `scratch/` are disjoint. |
| Command budgets | `workflow.md#command-time-budgets-oxide140`; `src/bin/oxide/budget.rs` | Seeding runs inside the admission window and never extends it (§8.3). |
| Hand-off / installer capability | `src/bin/oxide/handoff.rs:7-53` | No numeric version change, so no capability or receipt change (§9.4). |

## 3. Seed compatibility, separate from slot leasing

Architect binding `741268dc` separates the two predicates. Slot leasing
compatibility is **unchanged**; the seed predicate is its own record, because
leasing also protects final outputs and checkout affinity while seeding only
touches dependency intermediates.

A donor is eligible only when every dimension below is *recorded* on both
sides and equal. "Recorded" means present in the donor catalog (§4.4) and
known for the recipient command. Anything missing, malformed or unknown is
**not** eligible — never assumed equal.

| Dimension | Seeding | Why |
| --- | --- | --- |
| Pool and payload root | retained | Clone only inside one filesystem. |
| Actual volume/device (removable: mount point, volume UUID, `st_dev`) | retained | Same-volume predicate; `clonefile` is per-volume. |
| `rustc -vV` digest | retained | Compiler identity is content-affecting. |
| Cargo version string (verified release) | retained | Unit-directory grammar and fingerprint semantics are Cargo's. |
| Pinned layout adapter id (§10) | retained | Same on-disk grammar only. |
| Target triple (explicit `--target`, else host) | retained | Different target, different artifacts. |
| Oxide recipe key (`quick:dev-recipe-2`, `debug:dev-recipe-2`, `release`) | retained | Profile/recipe is in the key today. |
| Backend (`directory-budget-v1`, `apfs-volume-v1`) | retained | Different failure and flush semantics. |
| Compiler selection: `RUSTC` value and `rustc -vV` digest | retained (new fact); `build.rustc` or `CARGO_BUILD_RUSTC` present on either side | `workflow.md#cargo-version-matrix` records that the slot key does not represent a compiler taken from Cargo config or `CARGO_BUILD_RUSTC`. A configured compiler is therefore **unsupported**: the package set is skipped, never assumed equal |
| Wrapper context: the default bypass policy only | retained (new fact); a kept inherited wrapper (`OXIDE_ALLOW_RUSTC_WRAPPER=1`), any `RUSTC_WRAPPER`/`CARGO_BUILD_RUSTC_WRAPPER`, or any `RUSTC_WORKSPACE_WRAPPER`/`CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER` | A wrapper can change compiled output arbitrarily and its identity is not bounded by reading one file (a script may run more tools). v1 seeds only under the bypass policy; everything else skips |
| Compiler flags: `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, `RUSTDOCFLAGS`, `CARGO_BUILD_RUSTDOCFLAGS`, `CARGO_ENCODED_RUSTDOCFLAGS` | retained (new fact), exact value digest and presence | Content-affecting; recorded, not inferred |
| Profile and target overrides: `CARGO_PROFILE_*` and `CARGO_TARGET_*` beyond the recipe's own variables | **unsupported** | Oxide's own recipe variables are already in the recipe key; a caller override is not represented and skips |
| Bounded Cargo configuration context digest (§3.1) | retained (new fact), equal on both sides | The merged config can change the compiler, flags, linker, profile, `[env]` or source resolution; anything unsupported or unreadable skips |
| Verified Cargo home path used for registry/git sources | retained (new fact) | Cloned `.d` and fingerprint entries name absolute source paths |
| Command family (`check`-like, `build`-like, `doc`) | retained (new fact) | `check` and `build` units are different units |
| Producing slot generation (`last_lease_id`, incarnation) and the durable unseeded fact | retained | The catalog is bound to exactly one idle, **unseeded** generation (§4.1, §4.6) |
| Project identity, checkout path, compatibility digest | **ignored** | Deliberately not donor dimensions: only dependency intermediates move, and their content is fixed by locked source + toolchain, not by the checkout. |
| Owned-output destination | **ignored** | Finals never move; the recipient's own placement decides its own outputs. |
| Feature selection, `--all-targets`, `-p`, test/bench flags | **not predicted** | Cargo's own fingerprint components decide; unmatched copies stay inert (§4.5). |
| Donor rank, idle age, LRU position, recorded size | **not a dimension** | Used only for deterministic donor ordering and never mutated; never a reuse proof. |

Legacy-unknown behaviour: every slot created before this feature has no donor
catalog and is therefore **not a donor**, forever, without migration. A
recipient is unaffected: its own warm-reuse semantics come from the unchanged
key and origin machinery. There is no "assume equal" default anywhere.

**The destination exception, explicitly.** The slot key keeps the
owned-output destination as a leasing dimension (§2), and this design neither
relaxes nor extends it. The seed predicate simply does not include it, so two
slots with different owned-output destinations — or one ephemeral and one
converted checkout — may donate to each other's dependency units. Nothing about
finals moves: the recipient's own placement still decides where its binaries,
tests and uplifts go, and the donor's links, receipts and roots are untouched.

### 3.1 Configuration lookup and the producer-context digest

Cargo's own unit fingerprint binds the compiler hash, resolved flags, profile,
features, dependencies, target and path. It does **not** promise to bind the
whole inherited environment, and `workflow.md#cargo-version-matrix` names
exactly the inputs the slot key misses: `build.rustc`, `CARGO_BUILD_RUSTC`,
`RUSTC_WORKSPACE_WRAPPER` and a kept inherited wrapper. So the seed predicate
also requires a **context digest** whose lookup follows Cargo, not Oxide's
guess.

**Lookup (Cargo's own, pinned `cargo-1.97.1/c980f4866`,
`doc/src/reference/config.md`).** Cargo merges configuration files walking from
the command's **current working directory** upward through every parent to the
filesystem root, then `$CARGO_HOME/config.toml` and `$CARGO_HOME/config` at the
lowest precedence; a file closer to the working directory wins, environment
variables override files for the variables Cargo reads, and `--config` wins
over both. It does not start at the workspace root and does not stop at the
account home. The adapter therefore:

- walks from the command's inherited working directory (the same directory
  Cargo and the `rustc -vV` probe use) to `/`, collecting every existing
  `.cargo/config.toml` and `.cargo/config`, bounded to 16 files and 4 MiB of
  config bytes, with no symlink followed and no file created;
- adds `$CARGO_HOME/config.toml` and `$CARGO_HOME/config`;
- parses each with the repository's `basic-toml` dependency, reduces it to the
  `[build]`, `[profile]`, `[target]`, `[env]` and `[net]` tables, and renders
  the records in precedence order as
  `(absolute file path, table, key, canonical value)`;
- **resolves `[env]` relative values** as Cargo does: a `relative = true`
  value is made absolute against the directory that config file applies to
  (the parent of its `.cargo` directory; for `$CARGO_HOME` config, the parent
  of the Cargo home), and the resolved absolute path is what enters the
  digest. A value that cannot be resolved to a canonical path, a non-UTF-8
  path or value, or an unresolved `relative` flag makes the seed skip;
- treats `[source]`, `[patch]`, `[registries]`, `[unstable]`, the
  `replace-with` machinery, any `--config` argument, an unreadable or
  unparseable file, an oversized or duplicate-key file, and any table the
  adapter does not model as **unsupported** (skip);
- when one directory holds both `.cargo/config` and `.cargo/config.toml`, it
  follows Cargo's pinned precedence for that pair; if the adapter cannot
  establish it, it skips rather than guessing.

The digest is SHA-256 over the **effective merged** value set — one sorted
`table.key = canonical value` record per key after Cargo's precedence has been
applied, with `[env]` relative values already resolved to absolute paths — plus
the environment variables named in §3, each as `absent` or a hex digest of its
exact bytes. Merging rather than hashing the raw file concatenation avoids a
false mismatch when two working directories merely carry different irrelevant
files, while a value that actually differs still changes the digest. Both
sides must produce the same digest; the catalog records the digest and, as
review evidence only, the absolute config-file list in precedence order, so a
reviewer can see exactly what Cargo merged. Bounds: 16 config files, 4 MiB of
config bytes, 128 KiB of rendered records. Tests use synthetic config trees only, including a member
working directory, an ancestor above the account home, and a differing
relative `[env]` value.

### 3.2 Ambient inputs and the leaf-package boundary

A fixed environment list and the selected config tables still do not prove
equal compiled output for a *dependency* unit. A proc macro invoked while
compiling such a unit may read any inherited environment variable or any file
and generate different code; that access is not in the consuming crate's
rustc dep-info, and Cargo documents that only a small part of the environment
is fingerprinted. Equal locked source, Cargo/rustc identity and the listed
flags therefore do **not** establish equal output for arbitrary dependency
units, so the v1 eligible set must exclude every unit whose compilation can
invoke a proc macro.

The positive, checkable boundary is the lock's own dependency graph: a
package is eligible only when its lock entry has **no dependency edges at
all** (no normal and no build dependencies). Proc macros can only be invoked
through a dependency, so a unit with no dependency edges cannot run one; its
other compile-time inputs (`env!`, `option_env!`, `include_str!`,
`include_bytes!`, `#[path]` modules, build-script output for a same-package
script) are exactly the inputs Cargo fingerprints, and a same-package build
script's run unit is already excluded so its library rebuilds.

Consequences, stated rather than hidden:

- the eligible set is the **leaf** dependency set. Most proc-macro crates
  depend on `syn`/`quote`/`proc-macro2` and are therefore ineligible, as are
  most libraries with dependencies. Proc-macro scope is preserved
  structurally — a git or registry proc-macro crate with no dependency edges
  is eligible, and seeded proc-macro dylibs are only ever *used* to compile
  units that are themselves ineligible and rebuilt — but its practical reach
  is small;
- this narrows the approved scope, which names proc macros and dependency
  libraries generally. The design therefore **returns that narrowing to the
  Supervisor** rather than deciding it: the alternatives are the deferred
  artifact-JSON/unit-graph capture plus a reviewed proc-macro input-tracking
  mechanism, or accepting the leaf boundary for v1;
- the leaf rule is applied on both sides (donor and recipient lock entries),
  recorded as evidence in the catalog, and a non-vacuous native control proves
  that a non-leaf dependency whose proc macro reads an environment variable is
  ineligible and rebuilds with the recipient's own environment.

### 3.3 Prior review findings, resolved or dispositioned

| `0ca2bd7f` finding | Disposition in this design |
| --- | --- |
| 1. Cargo identity is probed, not recorded | Recorded per producing build in the catalog (§2, §4.1, §4.4); legacy facts stay unknown; nothing is stamped onto files an earlier generation wrote, because only an empty epoch publishes. |
| 2. Freeze an authoritative unit catalog | Bounded `SeedCatalog` sidecar with per-package locked identity, verified source digest and a relative file manifest (§4.4); no reconstruction from prefixes or fingerprint JSON alone. |
| 3. Git identity needs producing-build evidence | Pre/post content verification plus the producing-window mtime rule and donation re-verification (§4.3); the mtime-preserving adversarial residual is named, not hidden. |
| 4. Run exclusion can be simpler and less beneficial | Adopted as the frozen rule: the whole run unit and its output are excluded unconditionally, and the reach statement replaces the spike's expectation (§5.2, §12). |
| 5. One recipient transaction, never import donor trust | Durable untrusted origin, clone-then-verify, reset-or-refuse, no staging protocol (§6). |
| 6. Freeze custody at the integration point | Seed point after the nine-grant lease and output binding, before `mark_spawning`; library-scoped donor pin and pool lock (§6.1, §7). |
| 7. Bounded facts and a deliberate older-binary fence | Separate generation-bound sidecars, strict caps, no control/ledger field and an explicit stop condition if one is needed (§9). |
| 8. Bounds and executable feasibility | Finite bounds table, honest per-inode accounting and a source-derived (UNMEASURED) proof allocation (§8, §16.4). |

| `6449252752` finding (revision 2) | Amendment |
| --- | --- |
| 1. Whitelist cannot produce a fresh unit: encoded dep-info missing | §4.2 and §5.1 now freeze the complete per-role manifest: the fingerprint hash file, its `.json`, and Cargo's encoded `dep-<kind>-<target>` entry (marker `01 00 00 00 ff`, version 1 — the format the repository's own `src/dep_info.rs` documents and the version-matrix proof decodes), plus the role's artifacts. A unit whose triple is incomplete is not offered; a fingerprint-directory entry that cannot be classified, or a `dep-*` entry that does not match exactly one offered unit, makes the package ineligible. Any extra or missing entry therefore fails closed instead of yielding a stale-for-Cargo copy. |
| 2. Registry integrity was not archive-bound | §4.3 replaces `.cargo-checksum.json`. The cached `.crate` archive's bytes are hashed against the lock checksum, and the extracted tree is compared entry-by-entry against a canonical derivation of that authenticated archive (strict regular-file/directory grammar; symlinks, hardlinks, special entries, absolute or `..` paths, duplicate paths and extra/missing/extracted-only entries all fail closed). `.cargo-ok` is Cargo's unpack marker and is never treated as evidence. Verification runs before and after the producing command and again at donation, with a bounded verification cache keyed by archive checksum and tree identity. |
| 3. The catalog did not prove the current donor artifacts are the produced ones | §4.4 records a SHA-256 for every offered file, computed from the closed producing epoch, and §6.3 revalidates every offered donor file against those hashes under the donor pin and then verifies the clone against the same authenticated manifest. A changed file is an unknown donor: the whole seed is abandoned, not partially offered. |
| 4. A seeded recipient could publish a wrong catalog | §4.1 makes the unseeded condition a durable fact: a per-generation seed-attempt record, written before candidate reads and before the first clone, and no catalog is published unless that record is absent or terminal `skipped`/`reset`. A command that seeded anything publishes nothing, and a native regression proves a successfully seeded recipient never donates. |
| 5. Compatibility omitted effective producer inputs | §3 and §3.1 add the compiler selection, wrapper context, full flag set, profile/target overrides and a bounded Cargo configuration digest, with every unreadable, unsupported or unrepresentable fact skipping. |
| 6. Reserve handling turned an unsafe start into a cold build | §8.3 now gives the seed its own start gate at the existing 22 GiB watchdog trigger (`HOST_RESERVE_BYTES` + `MARGIN_BYTES`) and monitors at the watchdog's own cadence during clone and reset; a below-trigger, unknown or lost reading aborts the seed and **refuses the command before any child**, never a cold build. The removable path keeps its own removal-only watchdog and registration reserve instead of the internal space trigger. |
| 7. The plan replaced the required SQLite fixture | §16.2 keeps the real `libsqlite3-sys`-with-`bundled`-SQLite case and §16.4 supplies a source-derived time and disk allocation, including staging, seeded Cargo home, two coexisting slots, the owned image, fixtures and cleanup, with a fit gate that stops and reports instead of trimming a case. |

| `6449728021` finding (revision 3) | Amendment |
| --- | --- |
| 1. The configuration digest did not follow Cargo's lookup | §3.1 now walks from the command's working directory through every parent to the filesystem root, then the Cargo home, with Cargo's precedence, `[env]` relative resolution to canonical absolute paths, and refusal of `/.cargo/config.toml`-style ancestors it cannot model; member-cwd, ancestor-above-home and differing relative-config regressions are required. |
| 2. The fixed environment list did not bound dependency-unit content | §3.2 adds the leaf-package boundary: eligibility needs a lock entry with **no dependency edges at all**, because a proc macro can only be invoked through a dependency and its ambient reads are not fingerprinted. The narrowing of proc-macro reach is returned to the Supervisor, and a non-vacuous differing-environment/proc-macro control is required. |
| 3. Git verification had no bounded, effect-free process protocol | §4.3 now reuses the repository's sanitized `freshness::Git` protocol: exact argv/environment (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `submodule.recurse=0`, `core.fsmonitor=false`, no network, no config write, no prompts), cleared `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`, bounded calls, wall wait, stdout and settle window, and positive closure; unknown closure refuses. |
| 4. The proof allocation arithmetic was not sound | §16.4 corrects the Cargo-home figure (archives and extracted trees coexist), adds the new dependencies to staging, and gives a per-case schedule with a stated basis and a total that fits the 300 s / 512 MiB bound with margin; an unresolved conflict would return to the Supervisor. |

| `6449965447` finding (revision 4) | Amendment |
| --- | --- |
| 1. The artifact whitelist did not prove which package produced a file | §4.2 now requires a per-role join through rustc's own dep-info: exactly one `.d` whose first rule names the expected artifact and whose inputs all lie inside the package's verified source root, one fingerprint directory per package prefix, and no competing claim. A workspace/path package with a renamed equal-named lib target, a same-target different-source collision, a duplicate prefix or a duplicate `.d` claim makes the package ineligible; a role whose join cannot be proven is not offered. |
| 2. The lock protocol did not retain the admission barrier | §6.3 and §7 define one `Store::seed` lock lifetime — recipient record → removable sibling barrier → store admission → donor record (non-blocking) → pool lock — held from candidate read through the final donor revalidation and released in reverse; the pure Cargo planner runs under those locks and never takes one; the post-return donor re-read is gone; a competing clean/admission/donor transition is fenced by locks 2–4. The `status --measure` claim is corrected to lock-free, and the pool-lock hold is stated as bounded in work, not wall time. |
| 3. The SQLite oracle was vacuous for offered-run exclusion | §16.2 keeps the required SQLite case at its real scope and adds case 3: an eligible dependency-free git package with a build script whose compile unit must be offered, whose run payload and fingerprint must be absent from the plan, and whose runtime `OUT_DIR` must be the recipient's after donor removal, with a debug-only counterfactual that admits the run and must then print the donor's path. |
| 4. The disk allocation omitted the recipient's rebuild | §16.4 now uses explicit phases with lifetimes: staging; SQLite donor **and** the recipient's own SQLite rebuild coexisting; then deletion of the proof's own private pool before the image phase. Phase B peak is corrected to about 290-445 MiB, and the time schedule is about 155-290 s, with a stop-and-report gate and a Supervisor bound return. |
| Cache and reserve-gate wording | §4.3 binds a cache hit to the archive's recorded device/inode/size/mtime identity and re-derives on any change; §8.3 runs the reserve gate only when a non-empty plan exists, so a command with nothing to seed keeps today's behaviour. |

## 4. Unit provenance: the producing epoch and the donor catalog

Review finding 1–3 of `0ca2bd7f` are the reason this section exists: Cargo
identity is probed rather than recorded, `Cargo.lock` does not map packages to
unit directories, and a git cache can be dirty during the producing build.
Architect binding `bb151a59` then restricted v1 to the on-disk subset and
required the partial-association framing. The `6449252752` findings sharpen the
same boundaries: a unit manifest that omits Cargo's fingerprint-dir encoded
dep-info is not usable, and an archive check that only reads a tree's own
checksum file proves nothing.

### 4.1 Only an empty producing epoch produces a catalog

A slot qualifies as a *producing epoch* only when all of the following hold:

1. The lease that built it was a **new slot** created by the library
   (`pool.rs:1128-1185`), so `<slot>/build` started empty and no earlier
   generation's units can be inside it. `Prior::Fresh` is the front door's
   view of that fact (`store.rs:3356`). Every file the catalog can list was
   therefore written by this producing build, which is also why no file is
   ever stamped with a later Cargo identity than the one it was built with.
2. The generation is **unseeded**, durably. A per-generation seed-attempt
   record (§9.1) is written before the first clone (candidate reads are
   read-only and write nothing); a catalog is published only when that record
   is absent or
   records the terminal state `skipped`/`reset` (a lingering `intent` is never
   eligible). A generation whose record
   says `seeded` never publishes, even though it began empty: Cargo may have
   left the offered variants untouched and its workspace units rebuilt, so the
   record's files would be a donor's units described by this command's source
   and tool identity. This is the brief's empty **unseeded** producer rule, and
   it is durable rather than an in-memory flag, so a crash, a skipped seed, a
   failed seed, a former donor's reuse or an older binary's use of the slot
   cannot turn into a publication.
3. The primary child was Cargo itself (not `exec`, not an external
   subcommand), the command family is a supported family, and the command
   succeeded.
4. The command's own identity and context facts are recorded (§3.1, §4.4) and
   its Cargo is a verified release with the pinned adapter.
5. The slot is being released with a recorded `Built` origin — i.e. the
   existing trust record is written as usual.

The accepted consequence (binding `d1f281eb`) is a **short donor lifetime**:
the catalog is bound to that one idle generation and is invalidated by the
next lease of the slot (§4.6). A later changed build in the same slot never
extends or refreshes a catalog. Real donor availability is a hypothesis to
measure in the separately scoped real-project comparison, not a claim here.

### 4.2 Restricted association is partial, not provenance

The association rule for one package, applied on both sides independently:

- its **name** maps to exactly one lock entry across **all** source kinds
  (so a registry `foo` and a git `foo` make `foo` ineligible on that side);
- its lock entry has **no dependency edges at all** (§3.2), so its compilation
  cannot invoke a proc macro; a package with any dependency edge is ineligible
  on both sides;
- its lock source is a supported immutable kind: crates.io-style registry with
  a recorded `checksum`, or git with a full 40-hex revision;
- path/workspace/vendored/source-replaced and `[[patch]]`-replaced entries are
  ineligible; a lock entry without a `source` is ineligible;
- its manifest (read from the verified source root) declares exactly one
  library or proc-macro target whose name equals the package name, no other
  target with that name, and the recorded `[lib] name` (when present) equals
  the package name. A package that renames its lib target is ineligible
  because Cargo's file grammar is target-named, and two packages can expose
  the same target name;
- the donor and recipient lock entries agree on version and on
  source-plus-checksum (registry) or source-plus-full-revision (git).

**Unit and file association (the complete manifest and its join).** A file
name alone cannot prove which package produced an artifact: another package,
including a workspace/path package, may declare a library target with the same
name, and Cargo's output names are built from unit metadata with conditional
fallbacks (pinned `cargo-1.97.1/c980f4866`
`build_runner/compilation_files.rs`). The design therefore requires a
**per-role join through files the producing build itself wrote**, and rejects a
unit whenever that join is missing, duplicated or unprovable.

The join, for each role:

1. **Fingerprint directory uniqueness by prefix.** The opaque package-id hash
   in `<profile>/.fingerprint/<pkg>-<16hex>/` is never predicted or recomputed.
   There must be **exactly one** directory in the slot's `.fingerprint/` whose
   name is `<pkg>-` plus 16 lowercase hex digits. A second one (another version,
   another source, or another package with that name compiled in this slot)
   makes the package ineligible; the directory is copied verbatim once unique.
   Inside it, the unit suffix is Cargo's `<kind>-<target>` (`lib-<target>` for
   a library or proc-macro, `build-script-build` for a build-script compile
   unit) and its complete manifest is `<kind>-<target>`,
   `<kind>-<target>.json` and `dep-<kind>-<target>` (encoded dep-info, marker
   `01 00 00 00 ff`, version 1).
2. **Artifact join through rustc's own dep-info.** In `deps/`, there must be
   **exactly one** `<stem>-<hex>.d` whose first rule target names the role's
   expected output — `deps/lib<stem>-<hex>.{rlib,rmeta}` for a library,
   `deps/lib<stem>-<hex>.{dylib,so}` for a proc macro, or
   `build/<pkg>-<hex>/build-script-build` for a build-script compile unit — and
   whose dependency list contains **only** absolute paths inside the package's
   verified source root (§4.3), with at least one such path. That single file
   binds the source root, the output artifact and the unit metadata hash
   together, so an artifact produced by any other package or target cannot
   satisfy it. The offered artifacts are exactly the files sharing that
   `lib<stem>-<hex>`/`build/<pkg>-<hex>` stem and `.d`.
3. **No competing claim.** No other fingerprint directory or `.d` in the slot
   may claim the same artifact stem; a duplicate claim makes the package
   ineligible.

Every entry of the offered package's fingerprint directory must classify as an
offered unit's triple, an explicitly excluded run-unit or completion entry
(§5.1), or a `deps/` artifact in the role whitelist. Anything else — an
unknown suffix, a `dep-*` that matches no uniquely identified unit, a second
`dep-*` for one suffix, a symlink, a directory, a metadata file — makes **the
whole package** ineligible. An incomplete manifest would merely be stale to
Cargo; a mis-associated one could present a neighbour's file under the offered
unit's name, so both cases skip cold rather than guess. A role whose `.d` join
cannot be established is not offered at all; in v1 that is the build-script
compile unit if its dep-info cannot be proven, and that costs nothing because
its run unit is excluded anyway.

This is exactly the **partial association** binding `bb151a59` accepted: the
file grammar plus name uniqueness plus verified source content, not a general
package-to-artifact proof. If any of these facts is missing, the package is
skipped and the seed still proceeds for the packages that pass.

### 4.3 Source verification, including the git counterexample

Source content is verified **before and after** the producing command, and
again every time the donor is used, through the pinned registry/git adapter.
The check must bind content to the lock, not to a directory's own claim:

- **registry.** The lock's `checksum` is the SHA-256 of the `.crate` archive
  bytes; Cargo verifies exactly those bytes when it downloads and then writes
  `.cargo-ok` after unpacking (`cargo-1.97.1/c980f4866`,
  `sources/registry/download.rs` and `sources/registry/mod.rs`). The design
  therefore hashes the cached archive at
  `$CARGO_HOME/registry/cache/<index>/<name>-<version>.crate` against the lock
  checksum, and derives the expected extracted tree **from those authenticated
  archive bytes**, entry by entry, with a bounded reader: regular files and
  directories only, `ustar`/pax/gnu long names accepted, and symlinks,
  hardlinks, device nodes, duplicate paths, absolute paths, `..` components,
  and any unparseable entry cause refusal. The extracted tree must then equal
  that derivation exactly, modulo Cargo's own `.cargo-ok` marker, which is
  never evidence: a tree entry absent from the archive, an archive entry
  absent from the tree, a mode/executable-bit or content difference, or an
  extra unrecognized entry fails closed. `.cargo-checksum.json` is not used
  even when present, because it is a tree's self-declaration and Cargo does
  not write it for registry extracts;
- **archive reader adapter.** Deriving the tree needs gzip decompression and a
  tar reader inside Oxide. v1 pins that as an adapter with two new pinned
  dependencies (`flate2` with the Rust backend, and `tar` used only for
  decompression and entry iteration), added with their locked versions and
  covered by the implementation's staging seeder like every other dependency:
  no runtime network access, no external process, no new daemon or config
  key. If the project declines those dependencies, registry packages become
  ineligible and only git dependencies seed; that is a scope narrowing and
  must return to the Supervisor rather than being decided here;
- **git, through the repository's sanitized bounded process protocol.** Every
  invocation reuses `freshness::Git` (`src/bin/oxide/freshness.rs:453-500`),
  which is the reviewed effect-free adapter: the same argv and environment
  (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `-c submodule.recurse=0`,
  `-c core.fsmonitor=false`), so reading the index never rewrites it, no
  configured fsmonitor hook or daemon runs, no submodule is descended into,
  and no credential prompt can block. The seed clears `GIT_DIR`,
  `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_CONFIG_*` from the child
  environment, passes `-C <verified root>` only, uses no network and writes no
  config. Each call is bounded in wall wait, stdout and settle window, and the
  whole context is bounded in call count and total time exactly as the
  freshness gate does; a pipe held by a process Git started can never hold the
  gate past its clock, and exhausting the budget refuses. The verifier reads
  `rev-parse HEAD`, `status --porcelain` (no optional locks, no hooks) and the
  tracked tree digest, and requires: the locked revision, an empty status, no
  initialized or modified submodule, and a matching digest. A dirty,
  ambiguous, submodule-bearing or unreadable checkout makes the package
  ineligible, and an unconfirmed child closure is a **refusal**, never a cold
  skip. Present cleanliness alone is never accepted as producing-build
  evidence;
- **uniform failures.** A missing cached archive (pruned cache, vendored or
  source-replaced source), an ambiguous registry-index directory, an
  unreadable or oversized tree, or a tree over the content cap makes the
  package ineligible. Nothing is repaired, re-downloaded or fetched.

The `0ca2bd7f` counterexample — a git cache checkout compiled while locally
modified and restored afterwards — is handled by the producing-window rule,
not by trusting present cleanliness:

- the pre-verification runs after the freshness gate and before Cargo starts,
  and records the package's canonical tree digest (sorted relative paths,
  entry type, executable bit, size and content hash), which must equal the
  archive-derived digest (registry) or the locked-revision digest (git);
- the post-verification runs after Cargo's group closed and requires the same
  digest **and** that no eligible source file's mtime is later than the
  producing start. A restore (git checkout, editor write, re-extraction)
  updates mtimes and therefore drops the package;
- donation re-verifies the same digest for the offered packages, and a
  mismatch drops the package (or, for the offered files themselves, abandons
  the seed; §4.4, §6.3).

**Verification cache.** Re-deriving every archive on every build would be
unbounded work. A bounded sidecar (`oxide-managed/seeds/sources.json`, §9.1)
records `(cargo_home, archive relative path, archive device/inode/size/mtime,
archive SHA-256, tree root device/inode, tree digest, verified-at)`. A hit
requires **all** of: the archive file's identity (device, inode, size and
mtime) is unchanged from the recorded verification, its recorded SHA-256
equals the lock checksum, and the tree's own identity and digest match. The
hit therefore does not re-read the archive: it binds to the archive's recorded
immutable identity, and any identity change (replaced, rewritten, missing,
unreadable or untrackable) forces a full re-read, re-hash and re-derivation,
or a skip. The cache is not a trust root — it records only facts an earlier
full derivation established — and a same-user rewrite that preserves all four
identity fields is the same cooperative residual stated below. At most 64 MiB
of archive-derived content is verified per producing build, in deterministic
lock order; packages beyond that budget are not catalogued in that build.

**Residual, stated plainly:** a same-user writer that modifies a source file
during the producing build and restores the old content while preserving the
original mtime and size is not detected. That is outside the cooperative
foreground-workflow contract Oxide already relies on for closure, origins and
`setsid` escapees; this design does not claim an adversarial same-user fence
and does not present it as proven.

### 4.4 What a donor catalog records (bounded)

Written while the producing lease is still held and before release, into
`<pool>/oxide-managed/seeds/<slot-id>.catalog.json` (§9.1). Strict schema,
`deny_unknown_fields`, generation envelope plus:

- envelope: schema version, `store_id`, slot id, incarnation, producing
  `lease_id`, producing record id and use mark, producing start/finish wall
  times, and the seed-attempt record's terminal state (§4.1);
- producing identity and context: Cargo version, rustc digest and `RUSTC`
  value, target, recipe key, backend, adapter id, the §3.1 context digest and
  its absolute config-file list in precedence order, the flag digests, Cargo
  home, command family and command class;
- per package: name, version, source string, checksum or full revision,
  uniqueness evidence for the donor lock, the **leaf evidence** (the lock entry
  has no dependency edges, §3.2), source-root kind, and the **canonical source
  tree digest** verified at both ends of the producing window (§4.3);
- per unit: role (`lib`, `proc-macro`, `build-script-compile`), target name,
  and the exact relative file list (the manifest of §4.2 plus the role's
  artifacts), each with size, mtime and a **SHA-256 content digest computed
  from the closed producing epoch**.

The per-file digests are the donor stale-content check the brief requires.
They are computed after Cargo's group closed and before the catalog is
published, over the same file set the manifest names, bounded by the
producing-time hashing cap (§8.1). A file that later changes in place while
keeping its size and mtime therefore no longer matches its recorded digest;
§6.3 revalidates every offered file under the donor pin and abandons the seed
on any mismatch instead of offering a changed file.

Caps: 2 MiB per catalog, 2048 packages, 8,192 file entries, at most three
units per package. A catalog that would exceed a cap is not written (the slot
simply never donates), and the producing-time hashing budget decides how many
of the offered files get a digest: a unit whose files were not all hashed is
not offered. Nothing is offered from a catalog without recorded hashes.

### 4.5 Over-offer, and no expected unit hash

Binding `741268dc` removed the recipient-hash oracle. Oxide does **not**
compute Cargo's package-id hash, unit metadata hash or fingerprint directory
name. It offers, from one donor, **every** unit the donor tree holds for the
allowlisted packages (the allowlist is the recipient lock's eligible package
identities per §4.2). Cargo chooses which of those its own unit actually uses:

- a package-id-hash directory the recipient never names stays inert;
- a fingerprint whose recorded components differ (features, rustflags,
  profile, deps, path/target hash) is stale to Cargo and is rebuilt in place;
- a fingerprint that matches is fresh **because Cargo says so**, for the
  dependencies the donor actually compiled.

Unmatched copies stay as ordinary counted bytes until the ordinary whole-slot
eviction removes them (binding `741268dc`: no per-unit pruning in v1).

### 4.6 Lifetime and invalidation

The catalog is eligible only while the ledger still names the producing
generation: state `Idle`, no `lease_id`, and `last_lease_id` equal to the
catalog's `lease_id`. Every later grant mints a new lease id, so the first
reuse of the slot invalidates the catalog atomically with no deletion race.
The catalog file itself is overwritten by that generation's own producing
release if it qualifies again; otherwise it stays as ineligible text until the
slot is evicted. Nothing reads a catalog of a generation it does not match.

## 5. Which units and files, and what is excluded

### 5.1 Roles in v1

The fingerprint directory is `<slot>/build/<profile>/.fingerprint/<pkg>-<16hex>/`
and the artifacts live under `<slot>/build/<profile>/`. `<target_underscored>`
is the target name with `-` replaced by `_`; `<hex>` is Cargo's opaque unit
metadata hash, copied verbatim and never predicted.

Every offered unit requires its **join file**: `deps/<target_underscored>-<hex>.d`
(library, proc macro) or `deps/build_script_build-<hex>.d` (build script),
whose first rule target names the artifact and whose inputs are all inside the
verified source root (§4.2). The `.d` is required, not optional; a unit
without a proven join is not offered.

| Role | Complete fingerprint triple (inside the package directory) | Artifacts (with the required `.d`) |
| --- | --- | --- |
| registry/git dependency library | `lib-<target>`, `lib-<target>.json`, `dep-lib-<target>` | `deps/lib<target_underscored>-<hex>.{rlib,rmeta}`, joined by `deps/<target_underscored>-<hex>.d` |
| proc macro | `lib-<target>`, `lib-<target>.json`, `dep-lib-<target>` | `deps/lib<target_underscored>-<hex>.{dylib,so}`, joined by `deps/<target_underscored>-<hex>.d`. A `.dSYM` bundle is **excluded** |
| build-script **compile** unit | `build-script-build`, `build-script-build.json`, `dep-build-script-build` | `build/<pkg>-<hex>/build-script-build`, joined by `deps/build_script_build-<hex>.d`; if that join cannot be proven the role is simply not offered (its run is excluded anyway) |
| build-script **run** unit and its output | **excluded entirely** | `run-build-script-build-script-build*`, `dep-run-build-script-build-script-build`, `build/<pkg>-<hex>/{output,root-output,stderr}`, `build/<pkg>-<hex>/out/**`, `invoked.timestamp` |
| workspace/path package units | **excluded** | never classified |
| final outputs, uplifted binaries, test/bench executables | **excluded** | live in the checkout `target/` or are final uplifts |
| `incremental/**` | **excluded** | |

Every role below additionally requires the §3.2 leaf boundary: a package with
any dependency edge is not offered, so a proc-macro dylib can be seeded only
from a dependency-free proc-macro package, and it can only ever be *used* to
compile units that are themselves ineligible and rebuilt.

The three fingerprint entries are the unit's complete Cargo-visible state and
are all required: the hash file, the encoded components, and the encoded
dep-info whose absence Cargo 1.97.1 already treats as stale. These names are
the adapter's, and §10 requires them to be confirmed against a real 1.97.1
build before any seeding is enabled. A unit with any
of them missing is not offered. Artifacts are matched by the `<hex>` variants
of the allowlisted stems; unlisted stems, unknown suffixes, symlinks,
directories, Finder metadata and `.dSYM` bundles are never copied, and a
fingerprint-directory entry that classifies as none of the rows above makes
the package ineligible (§4.2).

### 5.2 Run units are excluded unconditionally in v1

Review finding 4 of `0ca2bd7f` is adopted as the frozen rule: in these
absolute build roots a build script's `root-output` names an absolute
`OUT_DIR`, so ordinary run records are ineligible whatever their stdout
directives contain. v1 therefore does **not** build a path detector, an
escaping-aware parser or a partial run payload. The whole-run exclusion makes
the "absolute path" definition degenerate on purpose: the recorded
`root-output` of an ordinary run record *is* the absolute build root here, so
any escaping-aware detector could only ever reject what this rule already
rejects; the conservative reading of "may contain an absolute path" is
"exclude it". The **entire** run unit and its output are excluded: fingerprint,
`output`, `root-output`, `out/**`, `stderr` and completion markers. A missing or
unreadable `output`/`root-output` cannot rescue a package, because eligibility
never looks at them.

Consequences are stated as product truth, not hidden:

- a package with a build script always reruns its run unit in the recipient,
  and its own lib unit is then stale, so the seeded lib and compile units for
  that package are inert;
- absent run output makes the run stale and staleness propagates to consuming
  units (pinned Cargo fingerprint source), so `serde`, `libc`, bundled SQLite
  and every `cc`/`OUT_DIR` user rebuild in the recipient;
- **"12 dependency packages Fresh" from the oxide#123 spike is not the
  expected v1 result.** It remains a bounded exploration, not qualification.

### 5.3 What makes Cargo reuse a seeded unit

Only Cargo's own checks: fingerprint components, output existence, and source
mtimes against the preserved reference mtimes. `clonefile` preserves mtimes,
so the donor's internal ordering is preserved inside the recipient slot. For
the recipient's **own** sources (workspace and path units, which are never
seeded) newer mtimes force a rebuild, and Oxide never touches a recipient
source file to prevent that. Registry and git dependency units read the
shared Cargo-home sources, recorded by absolute path and preserved by the
clone, so a *new* checkout can still reuse those units — subject to §5.2's
run-unit exclusion.

## 6. Recipient transaction, ordering and crash safety

Review finding 5 of `0ca2bd7f` recommended a durable-untrusted recipient; this
design adopts it and adds no staging/publication protocol.

### 6.1 Position in the command

After `lease_with_gate` returns a `Prior::Fresh` lease and after
`outputs::bind_for_command` succeeded, and **before** `child_temp::choose` and
`store.mark_spawning` (`main.rs:522-650`). Therefore:

- the recipient is this checkout's exclusive new slot, its destination is
  bound exactly as the slot was keyed, and no writer of any kind exists yet;
- no child temporary directory exists yet, so a seed abort has nothing to
  reconcile with oxide#136's `<slot>/scratch` custody;
- `mark_spawning` still happens only once a trusted launch is intended, so
  the existing "Cargo may have run unrecorded" semantics are untouched.

Seeding runs at most **once** per command. It adds no grant to the nine-grant
loop, and a gate refusal, eviction or reacquisition never triggers a second
attempt.

### 6.2 Preconditions (all must hold, else skip)

The order in which these are evaluated, and where the durable attempt record
and the reserve gate sit, is §6.3's; a precondition is never evaluated by
writing to the recipient.

- recipient: `reused == false` / `Prior::Fresh`, `<slot>/build` is a real
  owner-only directory on the pool's device and contains **no entries**;
- the recipient's Cargo is a verified release with the pinned adapter; the
  command family is supported; the command is not `exec` or an external
  subcommand;
- the recipient `Cargo.lock` exists, is a regular owner-owned file within the
  size bound, parses, and yields a non-empty eligible allowlist (§4.2). It is
  read only; nothing writes a lockfile, and `--locked`, `--frozen`, `--offline`
  and config arguments are never altered;
- at least one donor catalog is eligible (§3, §7) and overlaps the allowlist;
  the donor candidates are chosen by rank before any overlap is known, so a
  graph whose only overlap sits outside the eight newest candidates is a
  stated skip, not an unbounded search;
- the recipient's §3.1 context digest is supported and equal to the donor's;
- the seed start gate passes (§8.3): the internal pool's host reading is
  present and at or above the watchdog trigger. A missing or below-trigger
  reading is **not** a skip — it refuses the command before any child.

If a *precondition* fails, the command prints one `seeding skipped: <reason>`
line and builds cold. A skip is never an error, never retried and never
converted into a refusal. The reserve and ownership gates are not
preconditions: they refuse (§8.3, §13).

### 6.3 Split of responsibility, one lock lifetime, and the clone

Reading Cargo semantics stays in the front door; custody stays in the lease
library. The two are joined by **one `Store::seed` entry point with one lock
lifetime**, so no recheck ever happens after custody is released.

1. **Read-only preparation (front door, no pool lock, no writes).** The
   command reads the recipient lock, computes the allowlist (§4.2) and the
   producer context digest (§3.1), and — from the store's own state — the
   candidate donor slot ids by rank. If any
   precondition or cap fails, or the allowlist is empty, it prints one
   `seeding skipped: <reason>` and stops here: nothing is written, no reserve
   gate runs, and a command with nothing to seed keeps today's behaviour.
2. **Store entry, untrusted origin and attempt record (front door).**
   `Store::seed` takes, in the established order, the removable sibling barrier
   (internal admission) and then the store admission lock, and keeps both for
   the whole transaction. Under those locks it passes the §8.3 reserve gate,
   writes the per-generation attempt record as `intent`, and writes the
   untrusted origin (`mark_seed_start`): `<pool>/oxide-managed/origins/<record>.json`
   with `origin: null` and the record's current mark, synced. With the mark
   unchanged this already reads as `Prior::Unknown` via `trusted_origin`
   (`store.rs:3577-3591`), so a crash from here on leaves a slot no later
   command may reuse as warm, and the ordinary `Prior::Unknown` path evicts it
   whole. The origin is superseded only by the normal `release` write of a
   `Built` origin after Cargo closes — never by the seed. Every control-state
   read and write in this interval uses private helpers that assume the lock
   already held, so no second `flock` is taken.
3. **One library call with one pool-lock lifetime.** `managed::seed` takes the
   pool lock **once** and holds it until the final verification completes. In
   that lifetime it: reads at most eight donor catalogs (generation-checked);
   invokes a caller-supplied **planner** (the front door's Cargo semantics:
   context equality, overlap, deterministic one-donor choice, relative-name
   plan, the `.d` join and the §4.3 donation source re-verification — the
   catalog's recorded canonical tree digests, with archive re-derivation only
   when the cache identity changed, all under the donation caps; the planner
   takes no pool or admission lock itself); takes the donor record lock
   non-blocking with `create: false`
   (`store.rs:4509`); re-validates the whole donor predicate (§7), the
   recipient's `Held` generation, the empty recipient `build/`, the same
   payload root and device, and, for a removable pool, the volume binding;
   clones the plan with `clonefile` only, hashing each donor file against the
   catalog's recorded digest and each clone against the same digest; then, as
   the **last act before releasing**, re-reads the donor's ledger state and
   requires it still `Idle` and generation-matched. Any donor transition,
   `EXDEV`, `ENOSPC`, `EDQUOT`, digest mismatch, unexpected file type,
   permission change or unreadable path aborts the whole seed. No partial unit
   is ever published: either the entire plan verifies against the
   producing-epoch manifest or nothing stays.
4. **Release order.** The library drops the pool lock, then the donor record
   lock; `Store::seed` then writes the terminal attempt state and the
   last-seed sidecar and drops the admission lock and the sibling barrier, in
   reverse order. The recipient record lock is the command's and stays held
   until it releases or exits.
5. **Publication.** On complete success the library returns a receipt (units,
   files, logical bytes, donor identity, catalog digest, elapsed) **while both
   the pool lock and the donor record lock were still held through the final
   donor check**; the command then writes the `seeded` attempt state, the
   bounded last-seed sidecar (§9.2), and runs Cargo. The recipient's origin is
   still the untrusted marker until `release` writes the trusted one.

Nothing in this sequence re-reads a donor after custody has been released, and
no admission, clean or donor transition can interleave between selection and
the final verification: the donor record lock is the same lock every command of
that checkout takes before using the slot, and the admission lock is the one
recovery, eviction, ticket handling and `oxide clean` take before touching
records or the ledger.

### 6.4 What the recipient holds afterwards

The cloned files are ordinary owned entries of the recipient slot: they are
counted by its next closed recording, they are read by Cargo, and they are
removed with the slot by ordinary whole-slot eviction, clean, or a discarded
untrusted generation. No reference to the donor remains — no hard link, no
symlink, no shared extent bookkeeping (APFS may share extents, which Oxide
neither measures nor claims, §8).

### 6.5 Failure: reset the owned recipient, or refuse

- **Skip** (no donor, no overlap, an ineligible package, a cap or the seed's
  own cooperative deadline): nothing was written to the recipient; the attempt
  record becomes `skipped`; build cold.
- **Fault after a partial clone** (any clone, digest or verify error): the
  library resets exactly the recipient it owns — the plan's relative names
  deepest-first plus the directories the plan created, keeping the
  library-created `build/` directory itself — using the existing
  owned-deletion traversal, then verifies `<slot>/build` has no entries. If the
  reset verifies, the attempt record becomes `reset` and the command builds
  cold in the same slot: no tenth grant, no new record, and the recipient
  never carries partial state into Cargo. A crash *during* the reset leaves the
  untrusted generation in place, so the next command's `Prior::Unknown`
  whole-slot eviction (with oxide#155's capture-and-verify deletion) finishes
  it; the seed never deletes outside its own plan.
- **Reserve breach or unknown reading during the seed** (§8.3): the seed is
  abandoned, the recipient is reset as above, and the command **refuses**
  before any child. A reserve fault is never converted into a cold build.
- **Unknown ownership or an incomplete reset** (a foreign entry appeared, the
  device or volume identity changed, a path refused, a digest mismatch left
  the state unclassifiable): the command **refuses**, prints the reason and
  exits non-zero. It does not run Cargo over a half-seeded slot and does not
  delete what it cannot prove it owns; the untrusted origin and the
  `intent`/`seeded` attempt record keep the generation from being reused or
  published later.

### 6.6 Crash matrix

| Crash point | Durable state | Next command |
| --- | --- | --- |
| before the untrusted origin | new record, phase `Acquiring`, empty slot | existing recovery; slot has no trusted origin → `Prior::Unknown` → evicted |
| after the untrusted origin, before the first clone | untrusted origin, empty build | same as above (nothing to clean) |
| during cloning | untrusted origin plus partial `build/` | same as above: the generation is never reused, the whole slot is evicted |
| after a complete seed, before `mark_spawning` | untrusted origin, complete seed | whole slot evicted; no partial trust, no Cargo |
| after `mark_spawning`, during Cargo | untrusted/stale origin, Cargo may have run | existing recovery; `Prior::Unknown` eviction |
| after Cargo, during catalog write/release | previous (untrusted) origin | eviction; a catalog is never made eligible by a generation the ledger does not confirm |

The poison is the persisted untrusted origin plus the generation binding, not a
new recovery engine. The design adds no new crash-recovery decision and keeps
the existing writer-closure and quarantine rules.

## 7. Donor closure and lock order

Donor eligibility is evaluated from the ledger and the control record only,
under the locks, with no recovery performed to make a donor eligible:

- ledger state exactly `Idle`, `lease_id` absent, `last_lease_id` equal to the
  catalog's producing lease, not `Held`, not `Evicting`, not retired;
- control record exactly `Idle`, no holder, no lease id, no pending recovery,
  no `retired`/`lost` marker, and no live `oxide clean` for the slot's
  checkout;
- the donor record lock is free and already exists (`probe_lock` with
  `create: false`): a missing lock file, a held lock, or a peer's recovery
  means the donor is skipped, never waited for;
- donor and recipient payload roots resolve through no-follow descriptors to
  real owner-only directories under the same payload root, with the same
  `st_dev`; for `apfs-volume-v1` the verified volume binding must also match
  (mount point and volume UUID);
- no donor mutation of any kind: no rank, `last_used`, `recorded`, origin,
  holder, phase, seal or cleanup write, and no `touch` of donor files.

**Lock order, one lifetime.** Outermost to innermost, held from before the
candidate read until after the final donor revalidation, and released in
reverse:

1. the **recipient record lock**, which is the command's own and is held from
   its lease to its exit;
2. the **removable sibling barrier** (internal admission), when the selected
   pool is the removable one — the same internal→removable order `attempt`
   uses, unchanged;
3. the **store admission lock**, which recovery, eviction, ticket handling and
   `oxide clean` all take before they touch a record or the ledger;
4. the **donor record lock**, non-blocking and `create: false`, so a donor that
   is held, whose lock file is missing, or whose lock a peer's recovery holds
   is skipped rather than waited for;
5. the **library pool lock**, taken once and held to the end of the
   transaction.

Every transition that could move a donor out of `Idle` — another command of
that checkout leasing or releasing the donor record, `oxide clean`, admission
eviction, recovery, or ticket handling — needs at least one of locks 2–4, so
none can interleave between selection and the final check. The pool-lock hold
is bounded in **work** (≤ 4,096 files, ≤ 384 MiB cloned, ≤ 8 catalogs, one
donor pin), not in wall time: a blocked filesystem call cannot be pre-empted,
and this design does not claim otherwise. The command's own admission and
execution budgets and the proof supervisor's wall clock bound the run; they do
not bound the lock, and a stuck syscall would hold it until the process is
stopped, which is the same exposure any library mutation has today.

`status --measure` is **passive and lock-free** (`workflow.md#passive-snapshot-and-measurement`,
oxide#128) and never waits on this lock; only an admission or another library
mutation does.

If any check fails while the transaction runs (the donor changed phase, the
volume vanished, a digest mismatched), the seed is abandoned inside the same
lock lifetime: the recipient is reset as in §6.5, the attempt state becomes
`reset`, and the command either builds cold (pure seed fault) or refuses
(reserve, ownership, device or custody fault, §13).

## 8. Bounds, accounting, budget and watchdog

### 8.1 Bounds are finite and declared

| Bound | Value | Notes |
| --- | --- | --- |
| seed attempts per command | 1 | no retry, no extra grant |
| donor candidates examined | 8 | chosen by most recent rank then slot id; catalogs only |
| donor catalogs read | ≤ 8 × 2 MiB | strict schema; unknown → ineligible |
| recipient lock parsed | 4 MiB, 2048 packages | read only; no Cargo call |
| producer context digest | 16 config files (working directory to `/`, then the Cargo home), 4 MiB config bytes, 128 KiB rendered keys | unreadable/unsupported/unresolvable → skip |
| lock lifetime | recipient record → sibling barrier → admission → donor record (non-blocking) → pool lock | released in reverse; pool hold bounded in work, not wall time (§7) |
| git source verification | `freshness::Git` bounds: 64 calls, 60 s total, 30 s per call, 16 MiB stdout, 2 s settle | exhaustion or unconfirmed closure → refuse |
| producing source verification (pre and post) | 2048 packages, 100,000 entries, 128 MiB file content, 64 MiB archive-derived content, 4 s each | deterministic lock order; overflow packages are not catalogued |
| producing-time artifact hashing | 8,192 files, 256 MiB, 4 s | a unit without all digests is not offered |
| catalog | 2 MiB, 2048 packages, 8,192 files, 3 units per package | overflow: no catalog |
| plan | 256 units, 4,096 files, 384 MiB logical bytes | overflow: skip |
| artifact join (`.d` first-rule parse) | 1 MiB per `.d`, 4,096 inputs per unit, 32,768 inputs total | unparseable, duplicated or out-of-root input → package ineligible |
| source-verification cache | 2,048 records, 8 MiB sidecar, identity-checked per hit | missing/untrackable identity → full re-derivation or skip |
| donation source verification | 128 MiB content, 3 s | packages beyond the remaining cap are dropped from the plan, not the whole seed |
| seed work (clone + digest verify) | 5 s cooperative deadline, checked between filesystem calls | a blocked syscall cannot be pre-empted; not a hard lock bound |
| hashed bytes at donation | 2 × cloned bytes (donor + recipient copy) | the donor copy is compared with the producing-epoch digest; the clone with the same digest |
| seed sidecars total (`catalog`, `attempt`, `last`, `sources`) | 24 MiB | write refuses, or the oldest catalog is dropped, when exceeded |
| diagnostics | 2 KiB sidecar, one summary line ≤ 240 bytes | generated from post-reset state only |

### 8.2 Accounting is honest

Cloned files are new inodes with full logical and allocated bytes. The pool's
recorded per-slot figure gains them at the recipient's next closed record
(written by the release walk, `local-lease.md` §"Recorded per-slot
accounting"); nothing fabricates an early record. The donor's record is
untouched, so its figure keeps describing the donor's own bytes. APFS may share
extents between the two slots; Oxide does not measure extent sharing, does not
deduct it, and makes **no free-space or write-savings claim**. A cross-slot
hard-link pair would be counted once by the `(device, inode)` union, but
seeding never creates hard links.

### 8.3 Reserve: a start gate, a monitored stop, and two backends

The owning rule is a monitored start/abort threshold, not a guarantee
([architecture](../architecture.md#storage-invariants)). The existing product
refuses **pool creation** below `HOST_RESERVE_BYTES` (20 GiB,
`store.rs:4028-4038`) and stops a running build below the watchdog trigger
`HOST_RESERVE_BYTES + MARGIN_BYTES` (22 GiB, `watchdog.rs:22-30,113-140`). The
seed writes up to 384 MiB **before** that watchdog exists, so it carries its
own gate at the same trigger and never converts a bad reading into a cold
build:

- **internal `directory-budget-v1` pool.** The gate runs **only when a seed is
  actually possible**: after the read-only preparation has produced a
  non-empty plan (§6.3 step 1) and before the attempt record and the first
  clone. It samples the payload's filesystem with the watchdog's own reader;
  unknown, below 22 GiB or unreadable means the command **refuses before any
  child** with the reserve-stop diagnostic, and no attempt record is written.
  A command with nothing to seed never samples this gate, so its behaviour is
  exactly today's. While cloning and
  resetting, the seed samples at the watchdog's interval (about 1 s, and at
  least between every 64 copied files) with the same trigger; a breach or
  lost reading aborts the seed, resets the recipient and refuses the command.
  The sampled minimum and the crossing are reported like a watchdog stop.
- **removable `apfs-volume-v1` pool.** Its watchdog is a removal detector and
  has no space trigger (`store.rs:1716-1726`), and the volume's own capacity
  is its hard limit. The seed therefore requires the verified mount/volume-UID
  binding and the registration's reserve reading (the existing admission
  rule), monitors the binding at the same cadence, and aborts and resets on
  any binding or removal failure; the command then follows the existing
  lost-volume path rather than a generic cold build.
- **Deliberate asymmetry, stated for the Supervisor.** This gate is the
  seeding path's precondition; a command that seeds nothing keeps today's
  behaviour exactly. Unifying the start gate for every command would be a
  separate scoped change, and this design does not make it.

The pool budget stays soft: a started command's seed may pass the cap exactly
as its Cargo may, and the seed's bytes are visible to the next admission. The
command's admission budget is never extended: an expiry before the clone means
no seed; an expiry during it abandons and resets the seed and releases the
slot exactly as the existing expired-admission path does (no child, exit 124).
That release records no trusted origin, so the seeded generation is evicted by
the next command: the seed is never treated as a trusted build.

An unknown ownership, accounting, device or custody fact is a **refusal**
(§6.5), not a cold fallback: a safety fault must not be disguised as a
harmless miss. A removable-volume loss is neither a skip nor a reserve refusal;
it follows the existing removal and quarantine rules.

## 9. Persistence, visibility and format fences

### 9.1 Sidecars, not control-state fields

Four new owner-only sidecars under the existing `oxide-managed/` control
directory (mode `0700`, atomic temp + rename + `sync`, never in the payload):

- `<pool>/oxide-managed/seeds/<slot-id>.catalog.json` — the donor catalog
  (§4.4), at most one per ledger slot id;
- `<pool>/oxide-managed/seeds/<slot-id>.attempt.json` — the per-generation seed
  attempt (§4.1, §6.3): generation envelope plus a terminal state
  (`intent`, `seeded`, `skipped`, `reset`) and, when `seeded`, the donor slot,
  unit/file/byte counts and the catalog digest. Never deleted for its
  generation, so a later command cannot claim an unseeded epoch;
- `<pool>/oxide-managed/seeds/last.json` — the last completed seed
  (§9.2), schema `oxide.cold-seed/1`, at most 2 KiB;
- `<pool>/oxide-managed/seeds/sources.json` — the bounded source-verification
  cache (§4.3): at most 2048 records of `(cargo_home, archive relative path,
  archive device/inode/size/mtime, archive SHA-256, tree device/inode, tree
  digest, verified-at)`; a hit requires every identity field to be unchanged,
  and an untrackable identity forces a full re-derivation or a skip.

Each carries `store_id` (and the slot sidecars their slot id, incarnation and
lease id) and is only read when they match the current ledger generation. All
are bound by the caps in §8.1 and validate every field's length, like the
status-observations sidecar (`store.rs:1078-1143`). A sidecar that is missing,
oversized, malformed, symlinked, foreign-owned, unknown-version or
generation-mismatched is **ignored**, never repaired and never adopted. The
`attempt` record is the only one whose absence is a positive fact (an unseeded
legacy generation) rather than an unknown.

Orphan cleanup is bounded: when a catalog, attempt or last-seed is written, at
most eight sidecar files whose slot id is no longer in the ledger are removed.
Slot ids are monotonic and never reused, so this cannot delete a live record's
sidecar. When the four sidecar families together would exceed 24 MiB, the
least recently produced catalog is dropped (at most eight per write) or the
new catalog is not written.

### 9.2 Visibility

- A completed seed prints one line:
  `oxide: seeded <N> units (<M> MiB) from slot <X>`, where **N** is the number
  of completely cloned compile units and **M** is the logical cloned bytes.
  Neither number is a reused-unit count, a Cargo freshness claim or a
  physical-saving figure; the design says so in the text and the summary
  wording does not say "reused" or "saved".
- A miss prints `seeding skipped: <reason>` with a bounded reason token
  (no donor, donor busy, no overlap, non-leaf or ambiguous name, unsupported
  context or layout, unverified source, cap, deadline). Reserve, ownership,
  device and custody faults are refusals, not skip tokens (§8.3, §13). The
  reason token also covers the ambiguous-name and leaf skips that binding
  `bb151a59` requires the summary and status to report.
- `oxide status` shows at most one bounded recorded line from `last.json`,
  labelled as recorded: the recipient slot, the donor slot, units, MiB, file
  count, elapsed and when. It does not claim which copies Cargo kept fresh —
  Oxide cannot know that — and default status keeps its "recorded facts only,
  no walk, no lock, no reconciliation" property; the sidecar read is the same
  class of read as `state.json` and never mutates or reconciles.

### 9.3 Legacy and downgrade behaviour

An older binary is safe against a pool this feature touched because:

- it ignores the two sidecars (not part of any strict format it parses);
- a partially seeded generation carries no trusted origin, so the old binary
  evicts it by its own existing rule;
- a completed seed followed by a successful build is an ordinary trusted slot
  with an ordinary origin — the old binary may reuse it exactly as the new one
  may, because Cargo wrote that origin and Oxide never asserted freshness.

Nothing is migrated: existing pools have no sidecars, no catalog and therefore
no donor, and a pool that never seeds writes neither file.

### 9.4 Numeric fence and stop condition

No ledger, config, registration, receipt, origin, marker or state field is
added, so no numeric control-state version changes and the installed-binary
hand-off capability numbers stay exactly as they are
(`src/bin/oxide/handoff.rs:41-53`). This is deliberate: an unknown extra field
in `state.json` would make today's strict parser refuse the pool without
triggering the numeric-version hand-off, so the design keeps new facts out of
that format instead of bumping it.

**Stop condition:** if implementation finds it cannot express a needed fact
outside the control state, it must stop and return to design. Adding a state
field requires `STATE_VERSION = 6`, explicit legacy reads, forward-only writes,
the hand-off/capability update and installer fixture coverage.

## 10. Cargo layout adapter and unsupported cases

The adapter is pinned to the same Cargo the version-matrix table already
verifies (`workflow.md#cargo-version-matrix`):

- adapter id `cargo-1.97.1/c980f4866/build-dir-v1`;
- assumptions, all pinned-source or fixture-checked: `CARGO_BUILD_BUILD_DIR`
  is the slot's `build/`, Cargo creates `<profile>/` beneath it, fingerprints
  live in `<profile>/.fingerprint/<pkg>-<hex>/` with the three-entry unit
  manifest of §5.1, artifacts in `<profile>/deps/`, build scripts in
  `<profile>/build/<pkg>-<hex>/`, the profile directory name is Cargo's, and
  uplift copies finals to `CARGO_TARGET_DIR`;
- **manifest qualification is a precondition, not an assumption.** Before any
  seeding is enabled, the implementation must confirm, against a real 1.97.1
  build of a tiny fixture, that every offered unit's complete triple is
  present with exactly the names §5.1 lists (including the encoded
  `dep-<kind>-<target>`), that nothing else in the package fingerprint
  directory is needed for Cargo to consider the unit fresh, and that the
  package's `deps/` artifacts are the ones its fingerprint names. A mismatch
  means no seeding (never a guessed manifest), and it is recorded as an
  adapter-qualification failure in the implementation brief;
- the archive-reader adapter (§4.3) is qualified on the same tiny fixtures
  with its negative cases: a tampered archive, an extra tree file, a missing
  tree file, a symlink/hardlink entry, a `..` path and duplicate paths all
  refuse;
- a build-dir v2 layout (`-Zbuild-dir-new-layout`), any Cargo outside the
  verified table, any non-verified or rebuilt Cargo, and any release without
  this adapter's qualification means **no seeding** — never a disabled or
  altered normal Cargo;
- Rust 1.100 keeps its own seam: a new adapter id, its own qualification and
  its own proof. oxide#083 stays held until an actual stable 1.100.0 exists;
  no pre-release toolchain is installed or probed, and nothing here is shipped
  nightly behaviour.

Unsupported but harmless inputs skip: `exec` and external subcommands, `doc`
against an adapter that does not classify rustdoc units, a modelled JSON
target spec, a `--target-dir` (already refused), a command family mismatch, a
missing lock, and any package/unit outside the whitelist.

## 11. Interfaces with other contracts

- **oxide#136 child temporary files.** The seed's only payload writes are under
  `<slot>/build/**`, before the owned temp directory exists; `scratch/` custody, its marker, its
  accounting, its release removal after positive closure and the crash
  leftover rules are untouched. A seed reset never touches `scratch/`.
- **oxide#140 command budgets.** The seed is inside the admission window,
  never extends it, and an expiry behaves exactly as today (no child, exit
  124). The seed's own cooperative deadline is a skip reason, not a new budget
  axis.
- **oxide#133 owned outputs.** Seeding never publishes, reads or writes a
  member link, receipt, reservation or root; the recipient's binding is
  already complete before seeding and the destination is not a seed
  dimension.
- **Idle-exec D-4 (decision `890d6175`).** D-4 keeps run-slot and temp custody
  as they are and assigns the slow cold respawn to seeding. Concretely:
  a second command in a checkout whose session holds a slot may lease a new
  slot, and that new slot can be seeded **only** from a positively idle
  compatible slot. It can never seed from the held session's slot, because a
  held slot has no closure evidence and an `exec`/`run` session may itself run
  Cargo. So D-4's respawn is accelerated when another idle compatible slot
  exists (for example the project's other idle slot, or the session's own
  earlier slot once that session has ended and released), and it is not
  accelerated while the only donor is the held one. Reading a held slot is
  offered here as an explicit **rejected** alternative (§14) and as an open
  review question (§17), not as scope.
- **Prune, clean and eviction.** `oxide clean` waits for the recipient lease
  as today. Owned-output pruning never sees these sidecars. Whole-slot
  eviction/clean of a donor simply makes it stop donating; no cleanup is
  coupled to seeding.
- **Installed-binary hand-off and install.** Unchanged (§9.3); installation
  stays planner-gated.
- **One-use workspaces (sibling lead `eb8bdd5c`).** That sibling owns
  classification, admission, quota and output locality. Its only interface here
  is fixed: classification is explicit, a safe donor pin outranks eviction,
  only an empty unseeded verified producing epoch publishes a catalog, and
  unknown accounting or custody never lends quota. This design does not
  implement or assume one-use classification; a one-use slot is neither a
  special donor nor a special recipient until that sibling's design says so
  under its own review.

## 12. Counterexamples and the honest reach limit

1. **The #123 spike is not the result.** Its 12-Fresh measurement came from a
   fixture in which the donor's whole target-like tree was cloned beside the
   build; v1 excludes every run unit and does not touch `target/`, so
   build-script packages (including SQLite/`cc`/`OUT_DIR` users) rebuild.
   Because SQLite is not a leaf and is never offered, the required SQLite case
   proves Cargo's own rerun for an *ineligible* package; the non-vacuous
   exclusion proof is the eligible dependency-free build-script fixture of
   §16.2 case 3.
2. **A new checkout gets only what is genuinely shareable.** Workspace and
   path units always rebuild, and every excluded run unit and its consumers
   rebuild. Registry/git dependency units whose shared Cargo-home sources are
   older can be fresh even for a fresh checkout. What is never true is that
   seeding makes a recipient's own artifacts fresh, and Oxide never touches
   recipient source files to manufacture freshness, so the reach is bounded by
   §3.2, §5.2 and Cargo's own checks.
3. **One same-name dependency blocks that name.** Two registry versions, or a
   registry plus a git source of one name, make the name ineligible on both
   sides; this is the price of partial association.
4. **A same-named target in another package cannot satisfy the join.** A
   workspace or path package that declares `[lib] name = "cfg-if"` produces a
   `libcfg_if-<hex>` artifact too, but its dep-info binds that artifact to its
   own checkout root, not to the verified registry root, and its fingerprint
   directory carries a different prefix. The join (§4.2) rejects both, so no
   excluded artifact is offered; a duplicate fingerprint prefix or duplicate
   `.d` claim also makes the package ineligible.
5. **The leaf boundary removes most of the graph.** Any package with a normal
   or build dependency edge is ineligible (§3.2), so practical reach is narrow
   and most proc-macro crates never seed. That is the price of a positively
   supported ambient-input boundary, and the narrowing is returned to the
   Supervisor rather than decided here.
6. **Configuration is looked up where Cargo looks.** A `.cargo/config.toml` in
   a member working directory, an ancestor above the account home or at the
   filesystem root is part of the digest (§3.1), so two commands with different
   working directories cannot be accepted as equal contexts merely because
   their workspace roots match.
7. **A producing build that downloads or extracts sources excludes those
   packages** until the archive and tree verification succeeds: a file newer
   than the producing start drops the package, and a tree verified for the
   first time in this build is catalogued only within the 64 MiB
   archive-derivation budget.
8. **A dirty git cache compiled and restored** is caught when the restore
   updates mtimes (§4.3); the mtime-preserving adversarial case remains a
   stated residual.
9. **A tampered registry tree is caught by the archive, not by itself.** An
   extra or altered file, a symlink and a pre-existing local edit all fail the
   archive-to-tree comparison even when the tree carries a plausible checksum
   file (§4.3); a source directory name is never evidence.
10. **A context difference skips rather than guesses.** A configured compiler,
    a workspace or kept wrapper, a flag/profile override, `--config`, a source
    replacement or a `[patch]` makes the seed skip, so a project whose build
    depends on them simply never donates (§3.1).
11. **A seeded recipient never donates.** Its attempt record says `seeded`
    durably, so the design cannot attribute a donor's units to the recipient's
    own source and tool identity (§4.1).
12. **Copy-on-write is not a saving claim.** Cloned bytes are counted in full
    until a measurement proves otherwise, and this design does not measure
    physical savings.
13. **A held donor cannot be read.** So D-4's in-session respawn is not helped
    by the session's own slot (§11).
14. **Debug information** of a recipient's *new* links uses the recipient's own
    cloned object files, so it does not point into the donor; binaries the donor
    already linked keep pointing into the donor slot, exactly as they do today.
    No debugger behaviour is claimed by this design.

## 13. Skip, refuse and fail-closed matrix

| Situation | Result |
| --- | --- |
| no eligible donor / no catalog / no overlap / package ineligible | skip, cold build, one reason |
| package not a leaf, or its name is ambiguous on either side | skip, cold build, the reason names it |
| the artifact join is missing, duplicated, ambiguous or names a file outside the verified source root | skip, cold build (the package is not offered) |
| unsupported Cargo, adapter, command family, `exec`/external tool | skip, cold build |
| recipient lock missing, empty allowlist, malformed or oversized | skip, cold build (Cargo may create the lock itself) |
| unsupported producer context (compiler, wrapper, flags, profile, config, source replacement, patch) | skip, cold build |
| cap or the seed's own cooperative deadline exceeded | skip (abandon and reset if cloning started) |
| a competing mount could not be fenced, or the admission/donor locks could not be taken in order | **refuse** (unknown custody) |
| donor archive or git tree fails verification; catalog digest mismatch | skip the package, or abandon and reset when an offered file/mode changed |
| **internal pool host reserve unknown or below the 22 GiB trigger, before or during the seed** | **refuse**; the seed is abandoned and reset, and no child starts |
| removable volume binding or registration reserve fails, or the mount is lost | abandon and reset; the existing removal/quarantine path runs, never a generic cold build |
| donor became held/evicting/retired during the transaction, volume mismatch | abandon, reset recipient, cold build |
| a clone, digest, file-type or reset check fails | reset; if the reset verifies, cold build; otherwise **refuse** |
| donor path or volume identity cannot be established | **refuse** (unknown ownership is not a cold fallback) |
| recipient `build/` is not empty or not an owned directory | **refuse** |

A refusal prints one bounded reason and exits non-zero; it never deletes
anything it cannot prove it owns and never starts Cargo on unverified state.

## 14. Alternatives rejected

- **Trust a registry tree's own `.cargo-checksum.json`, or Cargo's `.cargo-ok`
  marker, as source integrity evidence.** Both prove nothing about the archive
  the lock authenticated; §4.3 derives the tree from the verified archive
  instead. A system `tar`/`gzip` subprocess was also rejected in favour of a
  bounded in-process reader with strict entry grammar.
- **Publish a catalog from a generation that seeded anything.** Cargo may leave
  offered variants untouched, so the files would be a donor's units described
  by another command's source and tool identity; §4.1's attempt record forbids
  it.
- **Treat a below-trigger or unknown reserve reading as a harmless miss.** The
  seed refreshes starts and refuses instead (§8.3).
- **Write "fresh" marks or patch fingerprints/mtimes.** Forbidden by the
  brief, and it would replace Cargo's judgment with Oxide's.
- **Capture build artifact JSON to build a full catalog.** Deferred by
  binding `bb151a59`; it would change caller-visible output handling and needs
  its own output-preservation contract.
- **Reconstruct Cargo's package-id/unit hashes.** A second fingerprint engine;
  binding `741268dc` removed the need by over-offering.
- **Seed a held donor (the D-4 shortcut).** No closure evidence, an `exec` may
  itself run Cargo, and it would let a seed read a slot a running program may
  still be using; rejected.
- **Reacquire and rebuild on seed failure.** Spends a tenth grant; resetting
  the owned recipient in place is bounded and cheaper.
- **A staging directory plus a publication protocol.** More crash states for no
  benefit: a new slot with an untrusted origin is already safe to abandon.
- **Byte-copy fallback, hard-link fallback, `ditto`/`cp -c` shells.** Only
  `clonefile` on the same volume.
- **Flag, config key or env toggle to turn seeding on/off.** Not authorized;
  eligibility is derived from recorded facts.
- **Pruning unused copies per unit.** Left counted until ordinary whole-slot
  eviction (binding `741268dc`).

## 15. Implementation slices (not authorized by this design)

1. **Registry verification adapter.** The pinned `flate2` + `tar` archive
   reader with its strict entry grammar, the canonical tree digest, the
   bounded source-verification cache, and the archive/tree/git negatives. Pure
   and tiny-fixture regressions; no Cargo yet.
2. **Library custody.** `managed::seed(pool, request, planner)` and
   `managed::publish_seed_catalog`: one pool-lock lifetime across candidate
   read, planner call, non-blocking donor pin, path and device validation,
   per-file digest revalidation, clonefile-only copy, final donor recheck and
   reset-or-refuse, with the bounded caps. Unit regressions on tiny owned
   fixtures, no Cargo.
3. **Front door.** `Store::seed` with the one lock lifetime of §6.3/§7
   (recipient record → sibling barrier → admission → library pool), lock
   parsing, the §3.1 context digest, allowlist and dep-info join, source
   verification, catalog production with per-file hashes at release, the
   reserve gate, attempt record, summary/status sidecars, skip reasons, and
   the seed point in `main.rs`. No user-visible change when nothing is
   eligible.
4. **Proof apparatus.** The fixed script, its pure self-test and its
   diagnostic mode, then the owner-activated natives.
5. **Knowledge truth.** Merge the implemented contract into
   `workflow.md` and turn this document's status line into an implementation
   record; `vision.md`'s "approved, not built" bullet changes only then.

Each slice needs its own approved Queue brief, risk tier and covering checks.
This design mints none of that.

## 16. Proof plan

Tier: **HIGH** authoring complexity, **ELEVATED** (the recipient reset deletes
storage, and one case writes removable media). Author and independent reviewer
natives are separately owner-activated; before the first counted native the
author runs one separately activated, uncounted `--diagnostic` of the same
fixed script, and a source sweep never replaces it.

### 16.1 Pure checks (no child, no Cargo, no pool)

Lock parsing, the uniqueness rule, the **leaf rule** (a lock entry with any
dependency edge is ineligible on either side, including build and dev edges
the lock records) and the manifest target-name rule; the complete per-role
triple (hash, `.json`, `dep-…`) with every negative (missing `dep-…`, an extra
`dep-…`, a duplicate suffix, an unclassifiable fingerprint-dir entry, symlink,
`..`, absolute path, unknown suffix, `.dSYM`, Finder metadata, oversized);
archive-reader source-verification-cache identity controls (unchanged
identity hits without re-reading, a changed size/mtime/inode/device forces a
re-derivation, and an untrackable identity skips);
**artifact-join negatives**: two fingerprint directories with the same package
prefix, a `.d` whose first rule names a different artifact, two `.d` files
claiming one stem, a `.d` whose inputs are outside the verified source root or
empty, a workspace/path package with a renamed equal-named lib target, a
same-target different-source collision, and a build-script compile unit whose
`.d` join cannot be proven (not offered);
archive-reader negatives (tampered archive, extra tree file, missing tree file,
symlink/hardlink/device entry, duplicate path, absolute path, pax/gnu long
name, truncation); git verification negatives (dirty, wrong revision,
submodule, unreadable) and the **exact argv/environment** of every git call
(`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `submodule.recurse=0`,
`core.fsmonitor=false`, `-C` only, cleared `GIT_DIR`/`GIT_WORK_TREE`/
`GIT_INDEX_FILE`/`GIT_CONFIG_*`) plus a fake-git control that would run a
configured fsmonitor hook and a bounded-budget exhaustion refusal; the §3.1
context digest with a synthetic config tree: equal contexts, differing flag,
differing table, **member working directory**, **ancestor above the account
home**, a config at the filesystem root, **differing relative `[env]` values**,
both `config`/`config.toml` in one directory, unsupported `--config`,
`[source]`, `[patch]`, `[registries]`, `[unstable]`, and unreadable/oversized/
non-UTF-8 config; catalog schema, leaf and source evidence, per-file digest
records, caps and generation binding; the durable attempt record's states and
the unseeded publication rule; producing-window mtime rule; plan intersection
and determinism; reset list derivation; reserve gate and monitored-stop
decision points against a fake reader; **lock-lifetime seams**: the pure
planner never takes a lock, the release order is pool → donor record →
admission → barrier, and a fake admission/donor transition injected between
selection and the final check aborts instead of producing a receipt; receipt
and summary/status rendering and their caps; adapter dispatch and
unsupported-version skip; donor predicate matrix (held, retired, evicting,
pending recovery, rank/lease mismatch, volume mismatch, missing lock, `seeded`
attempt); and the fail-closed matrix of §13.

### 16.2 Native cases (one fixed script)

On tiny private fixtures with private `HOME`, `CARGO_HOME`, `OXIDE_POOL`,
`OXIDE_TEST_HOME_FROM_ENV=1`, the invoking rustup home preserved, and a
private vendored/offline Cargo home seeded by the shared bounded seeder. The
four canonical temporary variables follow the repository recipe: the
supervisor's `OXIDE_TASK_SCRATCH_ROOT` and `OXIDE_TEST_INTERNAL_ROOT` point at
one fresh internal root for the genuine internal pool, and `TMPDIR`, `TMP` and
`TEMP` point at a separate fresh directory so ordinary test temp still follows
the selected-volume rule.

1. **SQLite/`cc`/`OUT_DIR` (required by the brief, retained), reported at its
   real scope.** A private workspace whose only dependency path is
   `libsqlite3-sys` with the `bundled` feature (the SQLite amalgamation
   compiled by `cc`, exposed through `OUT_DIR` and the link-search replay).
   The donor epoch builds it; the recipient's run must show the run unit
   rerunning, SQLite recompiling or its consumer rebuilding, and the program's
   runtime `env!("OUT_DIR")` value and SQLite version correct after the donor
   slot is deleted. SQLite has dependency edges and is **never offered**, so
   this case proves Cargo's own rerun for an ineligible package, not the
   exclusion of an offered run unit; the proof receipt says so, and case 3
   supplies the non-vacuous exclusion oracle;
2. **leaf registry positive and complete manifest.** A small registry leaf
   (`cfg-if`, no dependency edges) is offered with its full triple
   (hash/`.json`/`dep-…`) plus artifacts; two checkouts share one lock and
   Cargo home, the second leases a new slot that is seeded, and Cargo reports
   the offered dependency unit fresh (`--message-format` is caller-supplied by
   the proof, never injected). A debug-only proof control that drops only the
   `dep-…` entry from the plan must make the same unit rebuild, proving the
   manifest is the complete one;
3. **run exclusion for an OFFERED unit (non-vacuous oracle).** A dependency-free
   git dependency `seedctl` with a build script that writes a generated file
   into `OUT_DIR` and emits `cargo:rustc-env=SEEDCTL_OUT_DIR=<OUT_DIR>`, where
   the generated source embeds that same absolute path. The receipt must list
   the package's **build-script compile** unit as offered while the plan
   contains no `output`, `root-output`, `out/**` or run fingerprint; the
   recipient's run unit reruns, the crate recompiles, and the program prints
   the **recipient's** `OUT_DIR` and the donor's path is absent from the
   output. The counterfactual is a debug-only plan override that admits the
   run payload and fingerprint: the same program must then print the
   **donor's** `OUT_DIR`, which fails the recipient-path assertion — proving
   the oracle can distinguish a copied run from a rerun. Donor deletion before
   the recipient build is part of the case;
4. **ambient-input control (leaf boundary, non-vacuous).** A dependency with
   an edge whose proc macro reads an inherited environment variable is
   **ineligible**; two recipient builds under different values of that
   variable both run cold and produce the two different values, proving the
   leaf rule prevents the donor's expansion-time ambient input from leaking
   into the recipient's code. Its control is the leaf case 2;
5. **leaf proc macro over git.** An owned scratch git repository holds a
   dependency-free proc-macro crate; it is verified by the sanitized git
   protocol, offered as a leaf, and the recipient's expansion compiles and
   runs. A non-leaf proc-macro crate is ineligible (its `syn`/`quote` edges),
   and a configured `core.fsmonitor` hook in that repository is never run;
6. build-script **compile** unit: offered, but the excluded run unit makes it
   inert — the package's lib is rebuilt (the non-vacuous control);
7. donor deleted before the recipient builds: the recipient still compiles and
   runs, with no link to the donor;
8. changed dependency: a recipient lock with a different version or source does
   not offer that unit, and the build is correct;
9. held donor: a gated writer holds the only compatible donor → `donor busy`
   skip, the recipient builds cold, and the donor is unchanged;
10. **differing context.** A donor built under a different recorded
   compiler/Cargo identity, with a different `RUSTFLAGS`, or from a different
   working directory whose member `.cargo/config.toml` sets a different
   `[build] rustflags`/`[env]` value, is not eligible and the recipient builds
   cold; the equal-context control still seeds;
11. crash mid-seed: a debug-only hook (absent from release builds, scoped like
    the existing `OXIDE_TEST_*` hooks) crashes after the first clone; the next
    command must evict the untrusted generation, run no Cargo over partial
    state, and build cold;
12. caps each skip without donor mutation: logical bytes, units, files and the
    seed's own deadline;
13. git identity negatives: a dirty, submodule or differently-revisioned
    checkout is ineligible; a same-name registry/git collision is ineligible;
14. unsupported layout: a fake `cargo` that reports an unverified version
    makes the command skip seeding while Cargo still runs normally;
15. no donor writes: donor tree content digest, its catalog, ledger bytes and
    control bytes are identical before and after a seed, and its rank, use mark
    and recorded figure are unchanged;
16. removable path: one owned 128 MiB APFS image (`hdiutil attach -owners on`)
    registered as a removable pool seeds a tiny `cfg-if` workspace across two
    of its slots and detaches ordinarily, proving the same-volume predicate and
    that the recipient's bytes are on the image;
17. registry tamper controls: the cached `.crate` replaced with a different
    archive (checksum mismatch), the extracted tree with one extra file, one
    altered file, one symlink and one missing file each make the package
    ineligible and the command cold — never a seeded copy;
18. seeded recipient never donates: after a successful seed and build, the
    recipient's `attempt` record says `seeded` and a third cold slot must not
    receive a catalog from it, even though its build began empty and
    succeeded;
19. reserve gate: an injected below-trigger reading (the existing debug-only
    free-space seam) makes a command whose only available donor would seed
    **refuse before any child**; a mid-seed breach aborts, resets and refuses;
    the non-seeding control still runs Cargo as today;
20. partial-trust negative: a fault-injected partial recipient is never reused
    as warm by a later command.

21. **artifact collision controls.** A workspace/path package that declares
    `[lib] name = "cfg-if"` beside a genuine registry `cfg-if`, and a
    same-target dependency from a second source, must leave the eligible
    package ineligible (duplicate fingerprint prefix) or produce a plan that
    contains only the verified registry artifacts; the proof asserts the
    workspace artifact's path is absent from the plan and that the recipient
    still builds and runs correctly.

### 16.3 Runner, bounds, clocks and activation

- Selector (proposed, created only in the implementation slice):
  `effigy rust:cold-slot-seeding` = one fixed script and the whole command of
  the qualified shared supervisor,
  `python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-cold-slot-seeding.py`.
  The private pool is a genuine internal `directory-budget-v1` pool, so the
  recipe declares the owner-accepted internal-placement exception (decision
  `bd357a4c`) inside the same monitored owned root; the supervisor bytes are
  the `bc24b238…` pin above, with `--limit 300` and a 512 MiB monitored
  scratch budget including cleanup and the one owned image.
- `effigy rust:cold-slot-seeding:diagnostic` appends the script's
  `--diagnostic`: the uncounted keep-going collection required before a counted
  run (records reached/completed/failed/dependent-skipped/unexecuted cases and
  the latched first fault in a bounded `qualified: false` receipt, continues
  only into dependency-independent cases, aborts on unknown custody, device,
  accounting, deadline or receipt outcomes).
- `effigy proof:cold-slot-seeding:test` runs only the script's pure
  `--self-check`/`--self-test` with synthetic seams: no child, pool, image or
  Cargo.
- Proposed clocks: author 120 minutes and reviewer 45 minutes cumulative
  including admission waits, covering checks, the native and cleanup, with no
  reset and no extension; the independent design review ratifies feasibility
  before promotion.
- Every receipt states: "Effigy heavy admission unavailable (f6b3b50 retired
  host-run client; Queue #381 pending); not host-admitted". Counting follows
  the standing rule: anything reaching the guard spends the proof; a
  positively pre-admission busy with the guard never started is effect-free and
  at most one per new quiet window, never in a loop.
- Activation sequence: clean published source with hashes and preparation
  ready → a fresh owner-observed scheduler headroom receipt → explicit, counted
  activation separately for each side (author, then independent reviewer). The
  request goes through the authenticated blocked-to-owner path, never to Tom
  directly, and no native is activated by dispatch, by this document or by
  design review.
- The nine-item [proof-authoring premise checklist](workflow.md#proof-authoring-premise-checklist)
  is traced through the case bodies and helpers in the script, not in plan
  text.

### 16.4 Fixed-script time and disk allocation (source-derived, UNMEASURED)

The script runs three explicit phases and reports the monitored peak of each;
nothing is assumed to overlap silently, and the proof's own private pool is
deleted between phases.

**Phase A — staging only.** Debug staging of this HEAD's `oxide` (offline
1.97.1, 4 jobs, stripped) with the new archive-reader dependencies: 60-85 MiB
(the #122 recipe measured 58.3 MiB before them).

**Phase B — SQLite donor and recipient on the proof's private internal pool.**
Coexisting for the whole phase: staging (60-85), the seeded private
`CARGO_HOME` with `.crate` archives **and** extracted sources at once
(50-70; SQLite is about 10 MiB compressed and about 30 MiB extracted, plus
`libsqlite3-sys`, `cc`, `cfg-if`, `flate2`, `tar` and the tiny git fixtures),
the donor slot with SQLite intermediates (90-140), the recipient slot after its
own SQLite rebuild (90-140), and fixtures/logs (≤ 8). Phase B peak is therefore
about **290-445 MiB**, not the earlier 230-345, and it is the worst case in the
script. The 480 MiB stop threshold leaves 35 MiB of headroom under the
512 MiB budget; reaching it stops the run and reports instead of trimming a
case.

**Phase C — removable image.** The script first deletes **its own** private
internal pool directory (fixture cleanup of a tree it created, not product
eviction; recorded with its measured bytes and the post-delete peak), then
attaches one owned 128 MiB sparse image and runs the tiny `cfg-if` seed case.
Coexisting: staging (60-85), Cargo home (50-70), image backing (≤ 128), tiny
fixtures (≤ 8) — about **120-290 MiB**. Ordinary detach, backing removed only
after a completed detach.

Time, per required case, against the 300 s wall clock (estimates; the basis
names the measured analogue or the work performed):

| Case | Estimate | Basis |
| --- | --- | --- |
| staging `oxide` once, including the two new dependencies | 40-55 s | #122 measured 37 s for the same recipe without them |
| 1 SQLite donor epoch (`libsqlite3-sys` bundled) plus fixture bins | 8-20 s | #123's SQLite+serde donor build measured 7.97 s; `cc` compiles one amalgamation |
| 1 SQLite seeded recipient (run reruns, SQLite recompiles, consumer rebuilds) | 8-20 s | same compile repeated; #123's post-donor-delete build measured 0.55 s without an excluded run |
| 2 leaf positive, `dep-…` counterfactual, 3 offered-run oracle with its counterfactual, 4 ambient control, 5 leaf proc macro over git | 4-12 s each, 20-50 s | tiny Cargo calls over `cfg-if`-sized crates and the dependency-free build script |
| 6, 7, 8, 13 build-script inertness, donor-deleted, changed dependency, git negatives | 2-6 s each, 10-24 s | tiny fixtures, one Cargo call each |
| 9, 10 held donor and differing context | 2-5 s each, 5-10 s | a gated writer and one Cargo call |
| 11, 12, 14, 18, 19, 20 crash, caps, unsupported layout, seeded-recipient, reserve, partial trust | 1-4 s each, 10-24 s | fixture copies, mostly no Cargo |
| 15 no donor writes | 2-4 s | one seed plus digests |
| 16 removable image: delete internal pool, attach, seed, detach | 10-18 s | owned sparse image attach/detach plus a tiny build |
| 17 registry tamper controls, 21 artifact collisions | 2-4 s each, 10-20 s | archive/tree/plan mutations, no or one tiny Cargo call |
| cleanup, receipts and the phase table inside the guard | 5-10 s | bounded writes and one detach |

The total is about 130-255 s, with a worst case near 260 s when every case runs
at its upper estimate. That is inside the 300 s bound with a margin of about
40 s, so
the design states it as a plausible accounting, not a demonstrated fit: the
script prints the phase and case tables, and if the first diagnostic's
projection or measurement exceeds 275 s it **stops and reports**. If the
required SQLite and offered-run cases cannot fit even so, the conflict returns
to the Supervisor for a scoped bound amendment before freeze; this design does
not substitute a cheaper fixture and does not treat the separate real-project
comparison as the proof.

## 17. Independent review questions

1. Is the §3.2 leaf boundary accepted as the positive ambient-input boundary,
   with proc-macro reach returned to the Supervisor rather than silently
   narrowed — or does the Supervisor want the broader dependency graph, which
   would need the deferred artifact-JSON/unit-graph capture plus a reviewed
   proc-macro input-tracking mechanism?
2. Is the §4.2 artifact join accepted as exact per role — one fingerprint
   directory per package prefix, exactly one `.d` whose first rule names the
   artifact and whose inputs are all inside the verified source root, no
   competing claim, and no offering when the join is unprovable?
3. Is the §6.3/§7 single lock lifetime accepted as the brief's admission
   barrier: recipient record → sibling barrier → store admission → donor record
   (non-blocking) → pool lock, held through the final donor revalidation and
   released in reverse, with the pure planner taking no lock and the pool hold
   bounded in work rather than wall time?
4. Is the §4.3 archive-to-tree rule sound — cached `.crate` bytes against the
   lock checksum, a canonical derivation with strict entry grammar, a cache hit
   bound to the archive's recorded device/inode/size/mtime identity — and is the
   pinned `flate2` + `tar` reader acceptable as a new locked dependency?
5. Is the complete per-role manifest in §5.1, including the required `.d` join,
   accepted as the completeness and association boundary?
6. Is recording per-file producing-epoch SHA-256 digests (§4.4) with a
   256 MiB / 4 s cap the right stale-content evidence, and is abandoning the
   whole seed on one mismatch the intended fail-closed behaviour?
7. Is the durable per-generation seed-attempt record (§4.1, §9.1) accepted as
   the unseeded fact, including an older binary reusing a slot?
8. Is the §8.3 reserve treatment accepted: a 22 GiB start gate and monitored
   stop that refuses rather than cold-builds, run only when a seed is possible,
   with the removable volume on its own removal-only watchdog and registration
   reserve, and the non-seeding path deliberately unchanged?
9. Do the phased §16.4 figures (Phase B peak about 290-445 MiB of 512 MiB, time
   about 130-260 s of 300 s, with the required SQLite and offered-run cases
   retained) make the bound plausible, or should the conflict go to the
   Supervisor before freeze?
10. Is the D-4 interface in §11 the intended reading — seeding cannot help
    while the only donor is the session's held slot — or should a scoped
    follow-up consider a read-only held-slot donor under stricter closure?

## 18. Limits of this design

- It establishes no native evidence, no selected-volume fit, no timing, no
  reach, no saving and no benefit. The separate measured real-Effigy
  seeded-versus-unseeded comparison stays non-gating and separately scoped.
- Its eligible set is the leaf dependency set. That is narrower than the
  approved scope's wording about dependency libraries and proc macros, and the
  narrowing is returned to the Supervisor rather than decided here.
- It does not implement the registry archive reader, the dep-info join parser
  or the configuration walker; it names the dependency, the strictness and the
  skip rules the implementation must prove.
- It does not qualify removable media beyond the pinned adapter's own proofs,
  does not touch a real drive, and makes no persistence claim about physical
  disconnection.
- It does not remove the cooperative same-user residual, the `setsid` escape
  class, the source-tamper-with-preserved-mtime case, the cache-identity case
  with preserved metadata, or the control-directory trust that already covers
  origins.
- It does not decide one-use workspace classification, admission, quota or
  output locality (sibling lead `eb8bdd5c`), and it does not extend the
  nested-lease permit or idle-exec custody designs.
- It does not authorize oxide#138, which stays dispatch-held until this design
  is frozen, and it does not move oxide#083.
