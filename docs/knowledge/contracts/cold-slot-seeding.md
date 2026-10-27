# Cold-slot seeding from matching dependency units (proposed design)

Status: **design only, unimplemented, not frozen.** No product code, test
proof, selector, native execution, live pool read, installation or consumer
change is authorized by this document. It is the first deliverable of oxide#137
under the ROOT design-first rebind of 2026-10-27 (Tom and the Oxide Supervisor,
standing decision `8ddaa98d`): publish a concrete design, take an independent
exact-head design review, then stop for the Supervisor's delegated freeze.
Implementation, the author and independent-reviewer native sessions, and the
merge hold are separate and unchanged.

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
| Host reserve watchdog | `store.rs:49-53` (`HOST_RESERVE_BYTES`), `src/bin/oxide/watchdog.rs` | Sampled around the seed; a reserve breach skips the seed (§8.3). |

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
| Inherited rustc-wrapper policy (bypass / allowed) | retained (new fact) | A wrapper can change outputs. |
| `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `RUSTDOCFLAGS` (present/absent and exact value digest) | retained (new fact) | Content-affecting; recorded, not inferred. |
| Verified Cargo home path used for registry/git sources | retained (new fact) | Cloned `.d`/fingerprint entries name absolute source paths. |
| Command family (`check`-like, `build`-like, `doc`) | retained (new fact) | `check` and `build` units are different units. |
| Producing slot generation (`last_lease_id`, incarnation) | retained | The catalog is bound to exactly one idle generation (§4.6). |
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

### 3.1 Prior review findings, resolved or dispositioned

| `0ca2bd7f` finding | Disposition in this design |
| --- | --- |
| 1. Cargo identity is probed, not recorded | Recorded per producing build in the catalog (§2, §4.1, §4.4); legacy facts stay unknown; nothing is stamped onto files an earlier generation wrote, because only an empty epoch publishes. |
| 2. Freeze an authoritative unit catalog | Bounded `SeedCatalog` sidecar with per-package locked identity, verified source digest and a relative file manifest (§4.4); no reconstruction from prefixes or fingerprint JSON alone. |
| 3. Git identity needs producing-build evidence | Pre/post content verification plus the producing-window mtime rule and donation re-verification (§4.3); the mtime-preserving adversarial residual is named, not hidden. |
| 4. Run exclusion can be simpler and less beneficial | Adopted as the frozen rule: the whole run unit and its output are excluded unconditionally, and the reach statement replaces the spike's expectation (§5.2, §12). |
| 5. One recipient transaction, never import donor trust | Durable untrusted origin, clone-then-verify, reset-or-refuse, no staging protocol (§6). |
| 6. Freeze custody at the integration point | Seed point after the nine-grant lease and output binding, before `mark_spawning`; library-scoped donor pin and pool lock (§6.1, §7). |
| 7. Bounded facts and a deliberate older-binary fence | Separate generation-bound sidecars, strict caps, no control/ledger field and an explicit stop condition if one is needed (§9). |
| 8. Bounds and executable feasibility | Finite bounds table, honest per-inode accounting and a source-derived (UNMEASURED) proof allocation (§8, §16.3). |

## 4. Unit provenance: the producing epoch and the donor catalog

Review finding 1–3 of `0ca2bd7f` are the reason this section exists: Cargo
identity is probed rather than recorded, `Cargo.lock` does not map packages to
unit directories, and a git cache can be dirty during the producing build.
Architect binding `bb151a59` then restricted v1 to the on-disk subset and
required the partial-association framing.

### 4.1 Only an empty producing epoch produces a catalog

A slot qualifies as a *producing epoch* only when all of the following hold:

1. The lease that built it was a **new slot** created by the library
   (`pool.rs:1128-1185`), so `<slot>/build` started empty and no earlier
   generation's units can be inside it. `Prior::Fresh` is the front door's
   view of that fact (`store.rs:3356`). Every file the catalog can list was
   therefore written by this producing build, which is also why no file is
   ever stamped with a later Cargo identity than the one it was built with.
2. The primary child was Cargo itself (not `exec`, not an external
   subcommand), the command family is a supported family, and the command
   succeeded.
3. The command's own identity facts are recorded (§4.4) and its Cargo is a
   verified release with the pinned adapter.
4. The slot is being released with a recorded `Built` origin — i.e. the
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

This is exactly the **partial association** binding `bb151a59` accepted: the
file grammar plus name uniqueness plus verified source content, not a general
package-to-artifact proof. If any of these facts is missing, the package is
skipped and the seed still proceeds for the packages that pass.

### 4.3 Source verification, including the git counterexample

Source content is verified **before and after** the producing command, and
again when the donor is used, always through the pinned registry/git adapter:

- registry: the extracted root's Cargo-written `.cargo-checksum.json` must
  name the lock's package checksum, and every file it lists must hash to the
  recorded value, over the full tree;
- git: the checkout must be at the locked revision, `git status --porcelain`
  empty, no declared submodule initialized or modified, and the tracked tree
  digest recomputed from the index and worktree must match;
- anything else (a bare checkout Cargo cannot resolve, an ambiguous
  registry-index directory, an unreadable tree, a submodule, a tree over the
  content cap) makes the package ineligible.

The `0ca2bd7f` counterexample — a git cache checkout compiled while locally
modified and restored afterwards — is handled by the producing-window rule,
not by trusting present cleanliness:

- the pre-verification runs after the freshness gate and before Cargo starts,
  and records each eligible source file's size, mtime and content digest;
- the post-verification runs after Cargo's group closed and requires the same
  digest set **and** that no eligible source file's mtime is later than the
  producing start. A restore (git checkout, editor write, re-extraction)
  updates mtimes and therefore drops the package;
- donation re-verifies the offered packages' trees to the digest recorded in
  the catalog.

**Residual, stated plainly:** a same-user writer that modifies a source file
during the producing build and restores the old content while preserving the
original mtime and size is not detected. That is outside the cooperative
foreground-workflow contract Oxide already relies on for closure, origins and
`setsid` escapees; this design does not claim an adversarial same-user fence
and does not present it as proven. Registry sources are further constrained by
Cargo's own checksum manifest, which the producing build and the donation both
re-read.

### 4.4 What a donor catalog records (bounded)

Written while the producing lease is still held and before release, into
`<pool>/oxide-managed/seeds/<slot-id>.catalog.json` (§9.1). Strict schema,
`deny_unknown_fields`, generation envelope plus:

- envelope: schema version, `store_id`, slot id, incarnation, producing
  `lease_id`, producing record id and use mark, producing start/finish wall
  times;
- producing identity: Cargo version, rustc digest, target, recipe key,
  backend, adapter id, wrapper policy, the three flags digests, Cargo home,
  command family and command class;
- per package: name, version, source string, checksum or full revision,
  uniqueness evidence for the donor lock, source-root kind and recorded tree
  digest, and the unit list;
- per unit: role (`lib`, `proc-macro`, `build-script-compile`), target name,
  and the exact relative file list (fingerprint entries plus artifacts), each
  with size and mtime at catalog time.

Caps: 1 MiB per catalog, 2048 packages, 16,384 file entries, at most three
units per package. A catalog that would exceed a cap is not written (the slot
simply never donates). The catalog deliberately stores **no** file content
digest for the copied artifacts: hashing happens at donation time over the
bytes actually offered (§6.3), which is where the integrity claim belongs.

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

| Role | Seeded | Files (relative to the slot's `build/<profile>/`) |
| --- | --- | --- |
| registry/git dependency library | yes | `.fingerprint/<pkg>-<id>/lib-<target>` and `.json`; `deps/lib<target_underscored>-<hex>.{rlib,rmeta,d}` |
| proc macro | yes | same fingerprint entries; `deps/lib<target_underscored>-<hex>.{dylib,so,d}` — a `.dSYM` bundle is **excluded**, and only regular files from the whitelist are copied |
| build-script **compile** unit | yes (inert in v1) | `.fingerprint/<pkg>-<id>/build-script-build` and `.json`; `build/<pkg>-<id>/build-script-build`; `deps/build_script_build-<hex>.d` |
| build-script **run** unit and its output | **no** | `run-build-script-build-script-build*`, `build/<pkg>-<id>/{output,root-output,out/**,stderr}`, `invoked.timestamp` |
| workspace/path package units | **no** | never classified |
| final outputs, uplifted binaries, test/bench executables | **no** | they live in the checkout `target/` or are final uplifts |
| `incremental/**` | **no** | |

The fingerprint directory's hash suffix is copied verbatim; the file names are
matched by the fixed grammar `<pkg>` (dashes preserved for directories),
`<target_underscored>` and exactly 16 lowercase hex digits. Anything that does
not match the whitelist — an unknown suffix, a directory, a symlink, a
Finder-metadata file, a `.dSYM` bundle, an unexpected target kind — is never
copied. Because missing files can only make a unit stale, omitting an
unclassified entry is always the safe direction.

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
- the pool's host reserve reads above the 20 GiB threshold, and the seed's
  declared bytes fit the remaining bounded budget (§8).

If any precondition fails, the command prints one `seeding skipped: <reason>`
line and builds cold. A skip is never an error, never retried and never
converted into a refusal.

### 6.3 Split of responsibility, donor pin, plan and clone

Two library entry points and one front-door step keep "arbitrary mutable paths"
out of the front door and Cargo semantics out of the lease library:

1. **Candidate catalogs (library, read only).** `managed::seed_candidates`
   takes the pool lock, validates that the recipient is still `Held` by this
   lease, reads at most eight donor catalogs — chosen by most recent rank then
   slot id, each generation-checked against the ledger — and returns their
   bounded bytes. It takes no donor record lock and mutates nothing.
2. **Choice, plan and source re-verification (front door, read only).** The
   command parses the catalogs, intersects their package identities with the
   recipient allowlist and picks **one** donor deterministically: greatest
   overlap, then most recent rank, then slot id. It then builds a bounded plan
   of *relative* names for that donor's allowlisted units (no absolute path, no
   `.`/`..` component, no symlink component; caps in §8.1), and re-verifies the
   offered packages' source trees to the catalog's digests under the donation
   source cap (§4.3). All of this is read-only against the recipient lock, the
   Cargo home and the donor catalog; a plan or source failure skips without
   writing anything.
3. **Untrusted recipient, always before the first clone.** Immediately before
   the seed transaction the command persists
   `<pool>/oxide-managed/origins/<record>.json` with `origin: null` and the
   current use mark through the existing writer, and syncs it. With the mark
   unchanged this already reads as `Prior::Unknown` via `trusted_origin`
   (`store.rs:3577-3591`); a crash after this point therefore leaves a slot that
   no later command may reuse as warm, and the ordinary `Prior::Unknown` path
   evicts it whole. A small `Store` operation (`mark_seed_start`) writes it
   under the admission lock with the record's own current mark, so the front
   door never computes or writes a trust record itself. That untrusted record
   is superseded only by the normal `release` write of a `Built` origin after
   Cargo closes — never by the seed, and never by a successful copy.
4. **Seed transaction (library, custody).** `managed::seed` receives the
   recipient lease, the donor slot/incarnation/lease identity and that relative
   plan. It takes the donor's record lock non-blocking with `create: false`
   (`store.rs:4509`, `probe_lock`) and then the pool lock, in that order, and
   re-verifies the whole donor predicate (§7) plus the recipient's `Held`
   generation, the empty recipient `build/`, the same payload root/device and,
   for a removable pool, the volume binding. A candidate that changed between
   steps 1 and 4 is abandoned, never trusted. The front door never opens a
   donor path, and the library never interprets Cargo semantics. Inside that
   transaction, and clonefile only, for each planned file the library verifies
   the donor file (regular, owner-owned, inside the donor's `build/` through
   no-follow descriptors, on the payload device, size and mtime matching the
   catalog) and hashes it, clones it into the recipient (`clonefile`,
   device-local, no copy fallback), then hashes the recipient copy and
   compares. Any mismatch, `EXDEV`, `ENOSPC`, `EDQUOT`, an unexpected file
   type, a permission change or an unreadable path aborts the whole seed. No
   partial unit is ever published: either the entire plan verifies or nothing
   stays.
5. **Publication.** On complete success the library returns a receipt (units,
   files, logical bytes, donor identity, catalog digest, elapsed) and the
   command re-reads the two safety facts it can still check — the donor's
   ledger/control state and the host free-space reserve — before continuing.
   A donor that left `Idle`, an unreadable reading or a reserve breach after
   the clone makes the seed a failure, handled exactly as §6.5 (reset, cold
   build). The pool's recorded per-slot figure is not rewritten here; the
   recipient's next closed record accounts the bytes honestly (§8.2). On
   success the command writes the bounded last-seed sidecar (§9.2) and
   continues. The recipient's origin is still the untrusted marker; the normal
   `gate.finish`/`release` writes the trusted origin after Cargo closes.

### 6.4 What the recipient holds afterwards

The cloned files are ordinary owned entries of the recipient slot: they are
counted by its next closed recording, they are read by Cargo, and they are
removed with the slot by ordinary whole-slot eviction, clean, or a discarded
untrusted generation. No reference to the donor remains — no hard link, no
symlink, no shared extent bookkeeping (APFS may share extents, which Oxide
neither measures nor claims, §8).

### 6.5 Failure: reset the owned recipient, or refuse

- **Skip** (no donor, no overlap, an ineligible package, a cap, reserve or
  deadline): nothing was written; build cold.
- **Fault after a partial clone** (any clone/verify error): the library resets
  exactly the recipient it owns — the plan's relative names deepest-first plus
  the directories the plan created, keeping the library-created `build/`
  directory itself — using the existing owned-deletion traversal, then
  verifies `<slot>/build` has no entries. If the reset verifies, the command
  builds cold in the same slot: no tenth grant, no new record, and the
  recipient never carries partial state into Cargo. A crash *during* the reset
  leaves the untrusted generation in place, so the next command's
  `Prior::Unknown` whole-slot eviction (with oxide#155's capture-and-verify
  deletion) finishes it; the seed never deletes outside its own plan.
- **Unknown ownership or an incomplete reset** (a foreign entry appeared, the
  device or volume identity changed, a path refused): the command **refuses**,
  prints the reason and exits non-zero. It does not run Cargo over a
  half-seeded slot and does not delete what it cannot prove it owns; the
  untrusted origin keeps the generation from being reused later.

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

Lock order: **recipient record lock (held for the command) → donor record lock
(non-blocking, `create: false`) → library pool lock**, then the library's
existing internal→removable ordering, which is unchanged. Because the donor
pin never blocks and the pool lock is the library's own short-lived `flock`,
there is no new wait cycle and no cycle back from the pool lock to a record
lock (existing recovery reads the ledger under the pool lock and releases it
before probing record locks). The seed holds the pool lock for its own bounded
work (§8.3), so a concurrent admission or `status --measure` waits at most that
bound; `oxide clean` already waits for the recipient's lease.

If any check fails while the transaction runs (the donor changed phase, the
volume vanished, the pool lock could not be taken within its bound), the seed
is abandoned: the recipient is reset as in §6.5 and Cargo runs cold.

## 8. Bounds, accounting, budget and watchdog

### 8.1 Bounds are finite and declared

| Bound | Value | Notes |
| --- | --- | --- |
| seed attempts per command | 1 | no retry, no extra grant |
| donor candidates examined | 8 | chosen by most recent rank then slot id; catalogs only |
| donor catalogs read | ≤ 8 × 1 MiB | strict schema; unknown → ineligible |
| recipient lock parsed | 4 MiB, 2048 packages | read only; no Cargo call |
| producing source snapshot (pre and post) | 2048 packages, 100,000 entries, 128 MiB content, 3 s each | deterministic lock order; overflow packages are not catalogued |
| catalog | 1 MiB, 2048 packages, 16,384 files, 3 units per package | overflow: no catalog |
| plan | 256 units, 4,096 files, 384 MiB logical bytes | overflow: skip |
| donation source verification | 128 MiB content, 3 s | packages beyond the remaining cap are dropped from the plan, not the whole seed |
| seed work (clone + verify) | 5 s cooperative deadline, checked between filesystem calls | a blocked syscall cannot be pre-empted; not a hard lock bound |
| hashed bytes | 2 × cloned bytes (donor + recipient copy) | plus the donation source verification above |
| last-seed + catalog sidecars total | 16 MiB | write refuses when exceeded |
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

### 8.3 Budget and reserve

The pool budget stays soft: a started command's seed may pass the cap exactly
as its Cargo may, and the seed's bytes are visible to the next admission. The
seed **skips** (never refuses and never spills elsewhere) when:

- the measured host free space is below `HOST_RESERVE_BYTES` (20 GiB) before
  the clone or after it, or the reading is unavailable;
- the removable volume's binding no longer verifies;
- the command's admission budget is already expired, or expires during the
  seed. In that case the seed is not started or is abandoned, the slot is
  released exactly as the existing expired-admission path does (no child
  starts, exit 124), and no window is extended. That release records no
  trusted origin, so the seeded generation is evicted by the next command: the
  seed is never treated as a trusted build.

An unknown ownership, accounting or device fact is a **refusal** (§6.5), not a
cold fallback: a safety fault must not be disguised as a harmless miss.

## 9. Persistence, visibility and format fences

### 9.1 Sidecars, not control-state fields

Two new owner-only sidecars under the existing `oxide-managed/` control
directory (mode `0700`, atomic temp + rename + `sync`, never in the payload):

- `<pool>/oxide-managed/seeds/<slot-id>.catalog.json` — the donor catalog
  (§4.4), at most one per ledger slot id;
- `<pool>/oxide-managed/seeds/last.json` — the last completed seed
  (§9.2), schema `oxide.cold-seed/1`, at most 2 KiB.

Each carries `store_id`, slot id, incarnation and lease id and is only read
when they match the current ledger generation. Both are bound by the same caps
and validate every field's length, like the status-observations sidecar
(`store.rs:1078-1143`). A sidecar that is missing, oversized, malformed,
symlinked, foreign-owned, unknown-version or generation-mismatched is
**ignored**, never repaired and never adopted.

Orphan cleanup is bounded: when a catalog or last-seed is written, at most
eight sidecar files whose slot id is no longer in the ledger are removed. Slot
ids are monotonic and never reused, so this cannot delete a live record's
sidecar.

### 9.2 Visibility

- A completed seed prints one line:
  `oxide: seeded <N> units (<M> MiB) from slot <X>`, where **N** is the number
  of completely cloned compile units and **M** is the logical cloned bytes.
  Neither number is a reused-unit count, a Cargo freshness claim or a
  physical-saving figure; the design says so in the text and the summary
  wording does not say "reused" or "saved".
- A miss prints `seeding skipped: <reason>` with a bounded reason token
  (no donor, donor busy, no overlap, unsupported layout, unverified source,
  cap, reserve, deadline, refused ownership).
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
  live in `<profile>/.fingerprint/<pkg>-<hex>/` with per-unit files, artifacts
  in `<profile>/deps/`, build scripts in `<profile>/build/<pkg>-<hex>/`, the
  profile directory name is Cargo's, and uplift copies finals to
  `CARGO_TARGET_DIR`;
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

- **oxide#136 child temporary files.** Seeding writes only `<slot>/build/**`
  before the owned temp directory exists; `scratch/` custody, its marker, its
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
2. **A new checkout gets only what is genuinely shareable.** Workspace and
   path units always rebuild, and every excluded run unit and its consumers
   rebuild. Registry/git dependency units whose shared Cargo-home sources are
   older can be fresh even for a fresh checkout. What is never true is that
   seeding makes a recipient's own artifacts fresh, and Oxide never touches
   recipient source files to manufacture freshness, so the reach is bounded by
   §5.2 and by Cargo's own checks — exactly what the separate measured
   comparison must observe rather than assume.
3. **One same-name dependency blocks that name.** Two registry versions, or a
   registry plus a git source of one name, make the name ineligible on both
   sides; this is the price of partial association.
4. **A producing build that downloads or extracts sources excludes those
   packages.** Their files are newer than the producing start, so the mtime
   rule drops them; donors are strongest when the Cargo home is already warm.
5. **A dirty git cache compiled and restored** is caught when the restore
   updates mtimes (§4.3); the mtime-preserving adversarial case remains a
   stated residual.
6. **Copy-on-write is not a saving claim.** Cloned bytes are counted in full
   until a measurement proves otherwise, and this design does not measure
   physical savings.
7. **A held donor cannot be read.** So D-4's in-session respawn is not helped
   by the session's own slot (§11).
8. **Debug information** of a recipient's *new* links uses the recipient's own
   cloned object files, so it does not point into the donor; binaries the donor
   already linked keep pointing into the donor slot, exactly as they do today.
   No debugger behaviour is claimed by this design.

## 13. Skip, refuse and fail-closed matrix

| Situation | Result |
| --- | --- |
| no eligible donor / no catalog / no overlap / package ineligible | skip, cold build, one reason |
| unsupported Cargo, adapter, command family, `exec`/external tool | skip, cold build |
| recipient lock missing, empty allowlist, malformed or oversized | skip, cold build (Cargo may create the lock itself) |
| cap, reserve or seed deadline exceeded | skip (abandon and reset if cloning started) |
| donor became held/evicting/retired during the transaction, volume mismatch | abandon, reset recipient, cold build |
| a clone, hash, file-type or reset check fails | reset; if the reset verifies, cold build; otherwise **refuse** |
| donor path or volume identity cannot be established | **refuse** (unknown ownership is not a cold fallback) |
| recipient `build/` is not empty or not an owned directory | **refuse** |

A refusal prints one bounded reason and exits non-zero; it never deletes
anything it cannot prove it owns and never starts Cargo on unverified state.

## 14. Alternatives rejected

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

1. **Library custody.** `managed::seed` / `managed::publish_seed_catalog` /
   donor-catalog read entry point: pool-lock-scoped donor pin, path and device
   validation, clonefile-only copy, per-file hashing, reset-or-refuse,
   bounded caps. Unit regressions on tiny owned fixtures, no Cargo.
2. **Front door.** Lock parsing, allowlist and source verification, catalog
   production at release, summary/status sidecars, skip reasons, the seed point
   in `main.rs`. No user-visible change when nothing is eligible.
3. **Proof apparatus.** The fixed script, its pure self-test and its
   diagnostic mode, then the owner-activated natives.
4. **Knowledge truth.** Merge the implemented contract into
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

Lock parsing and the uniqueness rule; manifest target-name rule; file-grammar
classification including every negative (symlink, `..`, absolute, unknown
suffix, `.dSYM`, Finder metadata, oversized); catalog schema, caps and
generation binding; producing-window mtime rule; plan intersection and
determinism; reset list derivation; receipt and summary/status rendering and
their caps; adapter dispatch and unsupported-version skip; donor predicate
matrix (held, retired, evicting, pending recovery, rank/lease mismatch, volume
mismatch, missing lock); and the fail-closed matrix of §13.

### 16.2 Native cases (one fixed script)

On tiny private fixtures with private `HOME`, `CARGO_HOME`, `OXIDE_POOL`,
`OXIDE_TEST_HOME_FROM_ENV=1`, the invoking rustup home preserved, and a
private vendored/offline Cargo home seeded by the shared bounded seeder. The
four canonical temporary variables follow the repository recipe: the
supervisor's `OXIDE_TASK_SCRATCH_ROOT` and `OXIDE_TEST_INTERNAL_ROOT` point at
one fresh internal root for the genuine internal pool, and `TMPDIR`, `TMP` and
`TEMP` point at a separate fresh directory so ordinary test temp still follows
the selected-volume rule.

1. registry library positive: two checkouts with the same lock and Cargo home
   on one pool; the first builds the donor epoch, the second leases a new slot
   that is seeded, and Cargo reports the offered dependency unit fresh for the
   second checkout (`--message-format` is caller-supplied by the proof, never
   injected);
2. `cc`/`OUT_DIR` run-unit control: the run unit is excluded, the build script
   reruns, its consumer rebuilds, and the program's runtime `env!("OUT_DIR")`
   value is correct after the donor is gone;
3. proc macro: a proc-macro dependency's dylib and fingerprint are offered,
   and a macro expansion compiles and runs;
4. build-script compile unit: offered, but the excluded run unit makes it
   inert — the package's lib is rebuilt (the non-vacuous control);
5. donor deleted before the recipient builds: the recipient still compiles and
   runs, with no link to the donor;
6. changed dependency: a recipient lock with a different version or source does
   not offer that unit, and the build is correct;
7. held donor: a gated writer holds the only compatible donor → `donor busy`
   skip, the recipient builds cold, and the donor is unchanged;
8. cross-toolchain: a donor built under a different recorded compiler/Cargo
   identity is not eligible;
9. crash mid-seed: a debug-only hook (absent from release builds, scoped like
   the existing `OXIDE_TEST_*` hooks) crashes after the first clone; the next
   command must evict the untrusted generation, run no Cargo over partial
   state, and build cold;
10. byte, unit, file and time caps each skip without donor mutation;
11. git identity: a fully verified git dependency seeds; a dirty or
    differently-revisioned checkout does not; a same-name registry/git
    collision is ineligible;
12. unsupported layout: a fake `cargo` that reports an unverified version
    makes the command skip seeding while Cargo still runs normally;
13. no donor writes: donor tree content digest, ledger bytes and control bytes
    are identical before and after a seed, and its rank, use mark and recorded
    figure are unchanged;
14. removable path: one owned 128 MiB APFS image (`hdiutil attach -owners on`)
    registered as a removable pool seeds across two of its slots and detaches
    ordinarily, proving the same-volume predicate and that the recipient's
    bytes are on the image;
15. partial-trust negative: a fault-injected partial recipient is never reused
    as warm by a later command.

**Deviation to be frozen by the reviewer:** case 2 uses the `cc` crate with a
tiny C source rather than a bundled-SQLite-sized build, because a real SQLite
compile does not fit the 300 s bound with staging. It exercises the same
mechanism (build script, absolute `OUT_DIR`, link-search replay, runtime
`OUT_DIR`). A bundled-SQLite case belongs to the separate measured
real-project comparison, not this proof.

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
- Source-derived allocation (UNMEASURED): one debug staging of this HEAD's
  `oxide` offline with 1.97.1 (~37 s measured for the #122 recipe), 15 tiny
  cases at roughly 3–10 s each with at most two Cargo jobs and sequential
  fixtures, one image attach/detach (~5 s), cleanup inside the guard. Static
  estimate 150–230 s of the 300 s window; the script must verify the actual
  fit before activation.
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

## 17. Independent review questions

1. Is the §4.3 producing-window rule (pre/post content digests plus "no source
   mtime later than the producing start", on top of Cargo's checksum manifest
   or git revision) enough to freeze partial association, or does the
   mtime-preserving residual need a stronger producing-time witness?
2. Is refusing run units unconditionally (§5.2) accepted as the frozen v1
   boundary, including the reach statement that build-script packages never
   benefit?
3. Is a sidecar-only representation (§9) accepted instead of a
   `STATE_VERSION = 6` control fence, given the three safety cases in §9.3 and
   the stop condition?
4. Is the donor pin order (record lock non-blocking, then pool lock) safe with
   the current recovery and clean paths, and is the 5 s pool-lock occupancy
   bound acceptable?
5. Is the D-4 interface in §11 the intended reading — seeding cannot help while
   the only donor is the session's held slot — or should a scoped follow-up
   consider a read-only held-slot donor under stricter closure rules?
6. Does the reviewer accept the `cc`-based case 2 deviation, or require a
   bundled-SQLite fixture with a larger owner-approved bound?
7. Are the §8 bounds and the §16.3 static allocation plausible for one 300 s,
   512 MiB session with one image?

## 18. Limits of this design

- It establishes no native evidence, no selected-volume fit, no timing, no
  reach, no saving and no benefit. The separate measured real-Effigy
  seeded-versus-unseeded comparison stays non-gating and separately scoped.
- It does not qualify removable media beyond the pinned adapter's own proofs,
  does not touch a real drive, and makes no persistence claim about physical
  disconnection.
- It does not remove the cooperative same-user residual, the `setsid` escape
  class, or the control-directory trust that already covers origins.
- It does not decide one-use workspace classification, admission, quota or
  output locality (sibling lead `eb8bdd5c`), and it does not extend the
  nested-lease permit or idle-exec custody designs.
- It does not authorize oxide#138, which stays dispatch-held until this design
  is frozen, and it does not move oxide#083.
