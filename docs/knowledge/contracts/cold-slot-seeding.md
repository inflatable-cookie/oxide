# Cold-slot seeding from matching dependency units (proposed design)

Status: **design frozen; implemented in the unpublished `oxide` by oxide#167,
oxide#168 and oxide#169, with the v1 offer set narrowed by Revision 15.** This document was the design deliverable of
oxide#137 under the ROOT design-first rebind (standing decision `8ddaa98d`).
The frozen protocol is implemented: the library custody calls, the front door
transaction, the debug-only proof controls and the 24-case proof script exist
at the heads built by those tasks. The default-on flip (`COLD_SEED_QUALIFIED`)
is the isolated one-line product commit `bd61f7f`; later proof-script-only
corrections do not change it, so it stays in the merge head and is verified by
the reviewer there. The merge is held until the owner-activated uncounted
diagnostic, one counted author native and one separately activated independent
reviewer native pass at the exact merge head. No live pool, installation, consumer adoption or real-drive
qualification is authorized by this record.

**Revision 14** applies Supervisor amendment `fe432517`/`8daceb1a`
(2026-10-28), after the 9d71384 diagnostic exposed two design defects in the
frozen text. **A1′:** the pinned Cargo 1.97.1 build-script compile unit's hashed
rustc output and dep-info are `build/<literal package>-<hex>/build_script_build-<hex>`
and `...-<hex>.d` (**underscores**), while the unhashed uplift remains
`build/<literal package>-<hex>/build-script-build` and must be byte-identical
to the hashed output; the earlier hyphenated hashed name is not produced by
the pinned Cargo and is now a negative fixture. **Catalog timing:** a catalog is
published from the **post-Cargo** verified sources; a source that first
appears during the build (a fresh registry archive is extracted, or a Git
checkout is created, only while Cargo runs) is accepted after the release-time
unchanged-since check, and a pre-Cargo entry that also appears post-Cargo must
match kind, roots, digest and macro flag or the package is skipped. The
§§4.1/§4.4 empty-unseeded-producing-epoch and per-generation rules are
unchanged. **OUT_DIR boundary:** a library whose `.d` names generated `OUT_DIR`
inputs remains **unoffered** in v1; case 3's offered unit is the package's
build-script compile unit, and the seedctl library is the negative exclusion
control. The proof's case 3 oracle, the pure build-script fixture and the
catalog/product regressions follow these names; the per-case elapsed timing is
recorded in the uncounted diagnostic receipt for a measured bound projection.

**Revision 15** applies Tom's operator decision `e3540b1c` (2026-10-28),
recorded by the Oxide Supervisor: v1 offers **only plain registry library
compile units**. A unit is offered only when its lock entry is a crates.io
registry package with a recorded checksum, its normalized name is unique, it
has **no dependency edges**, its verified manifest declares a plain (not
proc-macro) library, its fingerprint triple is the library triple, its
artifacts are the `.rmeta`/`.rlib` pair, and every `.d` input is inside the
verified source root. Git/Git-revision sources, build-script compile and run
units, proc-macro units, and any unit whose `.d` names an `OUT_DIR` or other
outside-root input are **not offered**; each skip is named and the command
continues as an ordinary cold build. The case 05 proc-macro reuse defect is
fixed by never classifying or selecting a proc-macro unit, with product
regressions on both the catalog producer and plan selection. Cases 01
(non-leaf SQLite package; its registry leaf libraries may seed), 03 and 06
(Git/build-script/`OUT_DIR` seedctl; no catalog), and 05 (proc macro) are
negative controls that assert the target package is unoffered and the
ordinary build is correct; positives use plain registry libraries. Case 11's
post-crash `FileNotFoundError` was a proof-path error: the helper read
`pool/oxide-managed/ledger.json`, while the product's `LEDGER_NAME` is
`pool/ledger.json`; the helper and its synthetic mutation now use the root
path, and the shared collector records a bounded innermost function/line
without masking the original exception. No product defect was involved.
A donor path that is absent (evicted or its slot directory vanished) before the
first recipient write is `DonorPayloadMissing` and skips cold; an existing
foreign-owned, linked, wrong-type or wrong-device donor path remains an
`UnsafePath` refusal. The selector bound is **600 s** for
`rust:cold-slot-seeding` and `:diagnostic` only (Tom decision `1527723f`,
superseding the 480 s bound from `e3540b1c`), with
the 30 s script margin, 512 MiB monitored scratch, heavy admission, host
reserve, custody and ordinary detach unchanged; any bound above 600 s needs
another operator ruling. Revision 15 supersedes the build-script,
proc-macro and Git offers of revisions 11–14 for v1; their text is retained
as history. The default-on flip remains isolated behind this task's merge
hold.

The bounded skip message prefers an ambiguous dependency name over the root
package's generic non-leaf rejection. The collision proof supplies its resolved
lock before Cargo starts, so it reaches that classification rather than the
missing-lock precondition. A deleted donor payload may still have a selectable
catalog: the control-only selection discovers the missing payload under the
donor pin and skips cold. Neither that skip nor a seeded recipient's retained
bytes permit it to publish a catalog. A non-seeding command bypasses the seed
reserve gate but retains the ordinary Cargo watchdog, including its TERM exit
under injected low readings. Rejection counts are grouped by normalized name;
unknown `units` are omitted in catalog JSON, while a measured zero is explicit.

**Revision 13** applies Supervisor A2′ at head `c022dc6` (review comment
`6451786564`, 2026-10-27): the limit-gap/donor-figure publication exception is
deleted everywhere (§6.3 step 7, §8.1, §8.2, §13, case 24 and the revision
table). A seed is published **only** after a complete, ownership-clean,
generation-bound walk of the recipient before Cargo, recorded
`observed-while-held` and never closed. Any incomplete or failed walk — bound,
ownership, device, boundary or read — **abandons seeding only**: nothing is
published, the cloned recipient is marked untrusted and retained for the
existing journaled whole-slot eviction, it is never reused, and the command
proceeds as an **ordinary cold build under normal admission** with no extra,
reserved or failure-specific grant; normal wait/refusal and the reserve start
gate are unchanged. A1 and A3 are unchanged.
**Revision 12** answers the CHANGES REQUIRED review of `61a633e` (review
comment `6451613293`, 2026-10-27): **the pinned build-script manifest is rebound
to its source** — the extra filename hash applies to rustc's output *and* its
dep-info inside the build-script directory, so the hashed files are
`build/<literal package>-<hex>/build_script_build-<hex>` and `...-<hex>.d`, with
the unhashed uplift `build/<literal package>-<hex>/build-script-build` in that
same directory (byte-identical required); **the incomplete-walk fallback is
tightened** — the seed publishes only on a complete ownership-clean held-slot
walk — revision 12 briefly allowed a limit-only gap with the donor's recorded
figure as a proven upper bound; **that substitution is deleted by revision 13
(A2′)**; and **the
post-clone cold-build/extra-grant exception is removed** — that path now
refuses, releasing the lease through the ordinary (flush-aware) release, with no
rebind, no gate re-entry, no extra grant, and the retained slot accounted by the
next admission walk or the journaled `Prior::Unknown` eviction. This tightens
the earlier A2 cold-build wording because unknown accounting must not become a
cold miss.
**Revision 11** applied Supervisor decision `5073c632`'s three binding
amendments to the exact head `2cec1af`: **A1** the pinned custom-build layout is
corrected — the hashed rustc output and its `.d` are
`build/<literal package>-<hex>/build_script_build-<hex>` and
`build/<literal package>-<hex>/build_script_build-<hex>.d`, while the separate
unhashed uplift is `build/<literal package>-<hex>/build-script-build` in that
same directory and is accepted only when it is
byte-identical to the hashed output (missing, misplaced or non-identical → the
role is not offered); **A2** a bounded, generation-bound **post-clone byte walk
runs before Cargo**, recorded as `observed-while-held` and never as a closed
record; revision 12 tightened A2's cold-build wording to a refusal because
unknown accounting must not become a cold miss — **revision 13 (A2′) deletes the
donor-figure substitution entirely and makes an incomplete or failed walk
abandon seeding only, publishing nothing and continuing as an ordinary cold
build under normal admission, with the untrusted recipient retained**; **A3** seeding never blocks a
build — a positively leased/evicting donor or unknown custody before the first
clone means no seed, a `donor busy` skip and a cold build, with case 22 aligned
to §§6.5/13, while the reserve start-gate refusal is unchanged. Revision 11 adds
case 24 and the walk/refusal measurements, so the current arithmetic is **271 s
upper / 135 s lower of the 300 s** (29 s at the top), still UNMEASURED with the
diagnostic mandatory.
**Revision 10** answered the CHANGES REQUIRED review of `767f3f6` (review
comment `6451189343`, 2026-10-27): the build-script compile role now has its
exact pinned manifest — revised in revision 11: the hashed rustc output
`build/<literal package>-<hex>/build_script_build-<hex>` with
`build/<literal package>-<hex>/build_script_build-<hex>.d`, plus Cargo's
unhashed uplift `build/<literal package>-<hex>/build-script-build`
inside the same hashed directory — with both files recorded and re-verified,
and the pure fixture plus native oracle corrected (§4.2, §5.1, §16.1, §16.2);
the held-recipient reset story is replaced by a policy that never deletes
inside a `Held` slot: abandon before the first clone is a cold skip, and every
post-first-clone fault (including a reserve breach or a crash) records `reset`
as *abandoned*, retains the untrusted generation and **refuses**, leaving the
whole slot to the existing journaled `Prior::Unknown` eviction, with no new
capture format, replay journal or downgrade fence, and the `Spawning` crash row
corrected to explicit recovery (§6.5, §6.6, §13); and §8.2 now states the
actual oxide#146 policy — independently measured per-slot records, the inode
union only within one slot's walk, accepted cross-slot hard-link overcount, no
seed-created hard links, no extent-sharing or free-space claim, and the
post-seed membership/byte recheck against that policy with an incomplete walk
reported as unknown rather than zero.
**Revision 9** answers the CHANGES REQUIRED review of `b4f374b` (review
comment `6451045780`, 2026-10-27): the dep-info grammar now distinguishes
**output rules** (the `.d` itself plus the whitelisted role artifacts) from
rustc 1.97.1's **empty phony source rules** (`passes.rs:653-666`), accepting a
phony target only when it is byte-identical to an already accepted
prerequisite and resolves inside the verified source root, and rejecting any
other unexpected, nonempty, out-of-root or unmatched rule (§4.2); for every
supported role the fingerprint-directory hash and the artifact/`.d` hash must
be the same 16-hex `unit_id` rendering, with every other shape skipping cold;
a complete representative pinned dep-info fixture with its trailing phony
targets is a required non-vacuous pure positive, alongside the `cfg-if` and
build-script positives, and the positive native oracle stays; and the budget
statement keeps the corrected 265 s upper / 133 s lower of the 300 s with 35 s
remaining, marked UNMEASURED with the diagnostic still mandatory.
**Revision 8** answered the CHANGES REQUIRED review of `88e9a2c` (review
comment `6450792089`, 2026-10-27): the adapter now freezes **two name
identities** — the fingerprint package directory uses the **literal** lock
package name (`.fingerprint/cfg-if-<hash>/`,
`compilation_files.rs:231-245,278-281`) while the unit suffix, `dep-` entry and
artifact stem use the **normalized** target name (`lib-cfg_if`,
`deps/libcfg_if-<hex>.rmeta`, `targets.rs:135-168`). Normalization stays the
uniqueness/collision rule across all source kinds, the literal name selects the
directory prefix, and the association is restricted to the named role/layout
cases (unit-grouped build directories, custom `--out-dir`, custom
extra-filename and other layouts skip). A pure grammar regression pins
`cfg-if-<hash>/lib-cfg_if` beside the `foo-bar`/`foo_bar` collision, and cases
2 and 16 are reconciled with that grammar. The time arithmetic is corrected:
the upper estimates sum to **265 s of the 300 s**, leaving 35 s; the diagnostic
is still required and fit stays UNMEASURED.
**Revision 7** answered the CHANGES REQUIRED review of `6c3d5a07` (review
comment `6450625847`, 2026-10-27): the supported target name is the **normalized**
package name (`-` → `_`), matching pinned Cargo's inferred lib naming and its
rejection of a hyphenated explicit name, so `cfg-if`/`cfg_if` is supported and
a genuinely renamed target is unsupported; uniqueness and collision checks
apply after normalization across all source kinds, and the collision fixture
uses the valid `[lib] name = "cfg_if"` (§4.2); and the ambiguous-name skip
count now has a source — the producing epoch records a bounded, generation-bound
**rejection record** with the candidate-unit count it can classify for each
excluded name (or `units_unknown`), which the summary, `last.json` and
`oxide status` report as donor-side unit counts, never package counts and never
inferred from an unpinned catalog or the recipient allowlist (§4.2, §4.4,
§9.2).
**Revision 6** answered the CHANGES REQUIRED review of `effc985` (review
comment `6450400975`, 2026-10-27): the artifact join now parses **all** rules of
rustc's dep-info and looks in the unit's own output directory — the `.d`'s own
rule is first in the ordinary pinned shape, `.rmeta` and `.rlib` emits both
join, and a build-script `.d` lives under `build/<literal package>-<hex>/` (§4.2, §5.1);
the join is **established at producing time and recorded in the catalog**, so
selection reads catalog data only and never touches the donor payload, with
every artifact, `.d` and fingerprint file re-read and re-verified under the
donor pin (§4.2, §6.3); and the summary, `last.json` and `oxide status` now
carry bounded generation-bound **skip-count buckets** including the
ambiguous-name count that binding `bb151a59` requires (§9.2).
**Revision 5** answered the CHANGES REQUIRED review of `916cc851` (review
comment `6450220961`, 2026-10-27): the package-wide fingerprint uniqueness rule
is replaced by **per-unit classification** — a package may legitimately have
several `<literal package>-<hex>` directories (library, build script, feature variants),
each is either one complete unit or an excluded run-only directory, and
artifacts join to their verified source root through rustc's own `.d` — so the
required `seedctl` build-script case is achievable (§4.2); the contradictory
donor-lock order is replaced by one executable sequence — one admission
lifetime across a read-only pool-locked candidate call, effect-free selection,
and a mutating donor-record-then-pool call whose catalog is re-read and
rejected if it changed (§6.3, §7); and the reserve gate now sits after a real
plan and before the durable markers, so a command with nothing to seed samples
nothing (§6.3 step 5, §8.3).
**Revision 4** answered the CHANGES REQUIRED review of `e10b8b39` (review
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
during clone and abandon, with the removable path separated (§8.3); and the real
SQLite/`cc`/`OUT_DIR` proof retained with a source-derived time and disk
allocation (§16). Each finding is dispositioned in §3.3.

Written against clean pushed `main` at
`cec65daff49a2961c9b9d1dd2cad235251c02e48`. Every `file:line` anchor below is
against that commit; re-check the anchors before trusting them after any
rebind. The shared proof supervisor is `scripts/bounded-test.py`
SHA-256 `addaae5ac1434b3a6c623db3e0230c0a0607c192fc1f0e6055613f484d749b9d`
(the qualified oxide#151 guard `bc24b238…` plus oxide#191's opt-in post-census
image detach, unqualified until its own qualifications pass; the earlier
`d89115f1…` pin is historical and qualifies only its own bytes).
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
- No byte-copy fallback. If a clone cannot complete, the seed is abandoned
  before the first clone, or the command refuses and retains the untrusted
  generation (§6.5).
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
| Encoded dep-info in the fingerprint directory | Cargo's `.fingerprint/<literal package>-<hash>/dep-<kind>-<normalized target>`, marker `01 00 00 00 ff`, version 1; the repository's own bounded reader documents the format (`src/dep_info.rs:1-30`) and the version-matrix proof decodes every `dep-*` file (`workflow.md#cargo-version-matrix`) | Part of every offered unit's manifest (§4.2, §5.1); a unit without it is not offered. |
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

Consequences, stated rather than hidden (Revision 15):

- the v1 eligible set is **plain registry library compile units only**: the
  lock entry is a crates.io registry package with a checksum, has no
  dependency edges, and the verified manifest declares a plain library.
  Proc-macro, build-script compile, Git/Git-revision, path/workspace and any
  outside-root `.d` unit are **not offered at all**, so a proc macro can never
  be seeded or reused from a donor and a build-script compile unit never
  enters a plan;
- the leaf rule is still applied on both sides (the entry has no dependency
  edges), and the registry-only rule is applied on both sides; a non-leaf
  package and a registry leaf's proc-macro or build-script sibling remain
  ineligible with a named skip;
- the positive controls are plain registry leaf libraries (for example
  `cfg-if`); the non-leaf SQLite package and the Git/build-script/`OUT_DIR`
  seedctl package are negative controls. Broader seeding (build scripts,
  proc macros, Git, outside-root inputs) needs a later design and task, not a
  silent widening here.

### 3.3 Prior review findings, resolved or dispositioned

| `0ca2bd7f` finding | Disposition in this design |
| --- | --- |
| 1. Cargo identity is probed, not recorded | Recorded per producing build in the catalog (§2, §4.1, §4.4); legacy facts stay unknown; nothing is stamped onto files an earlier generation wrote, because only an empty epoch publishes. |
| 2. Freeze an authoritative unit catalog | Bounded `SeedCatalog` sidecar with per-package locked identity, verified source digest and a relative file manifest (§4.4); no reconstruction from prefixes or fingerprint JSON alone. |
| 3. Git identity needs producing-build evidence | Pre/post content verification plus the producing-window mtime rule and donation re-verification (§4.3); the mtime-preserving adversarial residual is named, not hidden. |
| 4. Run exclusion can be simpler and less beneficial | Adopted as the frozen rule: the whole run unit and its output are excluded unconditionally, and the reach statement replaces the spike's expectation (§5.2, §12). |
| 5. One recipient transaction, never import donor trust | Durable untrusted origin, clone-then-verify, abandon-or-refuse, no staging protocol (§6). |
| 6. Freeze custody at the integration point | Seed point after the thirteen-grant lease and output binding, before `mark_spawning`; library-scoped donor pin and pool lock (§6.1, §7). |
| 7. Bounded facts and a deliberate older-binary fence | Separate generation-bound sidecars, strict caps, no control/ledger field and an explicit stop condition if one is needed (§9). |
| 8. Bounds and executable feasibility | Finite bounds table, honest per-inode accounting and a source-derived (UNMEASURED) proof allocation (§8, §16.4). |

| `6449252752` finding (revision 2) | Amendment |
| --- | --- |
| 1. Whitelist cannot produce a fresh unit: encoded dep-info missing | §4.2 and §5.1 now freeze the complete per-role manifest: the fingerprint hash file, its `.json`, and Cargo's encoded `dep-<kind>-<target>` entry (marker `01 00 00 00 ff`, version 1 — the format the repository's own `src/dep_info.rs` documents and the version-matrix proof decodes), plus the role's artifacts. A unit whose triple is incomplete is not offered; a fingerprint directory that mixes units or holds an unclassifiable entry is unusable, and a `dep-*` that matches no single suffix makes that directory unusable (§4.2's per-unit rule, revision 5). A run-only directory is classified and never copied. Any extra or missing entry therefore fails closed instead of yielding a stale-for-Cargo copy. |
| 2. Registry integrity was not archive-bound | §4.3 replaces `.cargo-checksum.json`. The cached `.crate` archive's bytes are hashed against the lock checksum, and the extracted tree is compared entry-by-entry against a canonical derivation of that authenticated archive (strict regular-file/directory grammar; symlinks, hardlinks, special entries, absolute or `..` paths, duplicate paths and extra/missing/extracted-only entries all fail closed). `.cargo-ok` is Cargo's unpack marker and is never treated as evidence. Verification runs before and after the producing command and again at donation, with a bounded verification cache keyed by archive checksum and tree identity. |
| 3. The catalog did not prove the current donor artifacts are the produced ones | §4.4 records a SHA-256 for every offered file, computed from the closed producing epoch, and §6.3 revalidates every offered donor file against those hashes under the donor pin and then verifies the clone against the same authenticated manifest. A changed file is an unknown donor: the whole seed is abandoned, not partially offered. |
| 4. A seeded recipient could publish a wrong catalog | §4.1 makes the unseeded condition a durable fact: a per-generation seed-attempt record, written before candidate reads and before the first clone, and no catalog is published unless that record is absent or terminal `skipped`/`reset`. A command that seeded anything publishes nothing, and a native regression proves a successfully seeded recipient never donates. |
| 5. Compatibility omitted effective producer inputs | §3 and §3.1 add the compiler selection, wrapper context, full flag set, profile/target overrides and a bounded Cargo configuration digest, with every unreadable, unsupported or unrepresentable fact skipping. |
| 6. Reserve handling turned an unsafe start into a cold build | §8.3 now gives the seed its own start gate at the existing 22 GiB watchdog trigger (`HOST_RESERVE_BYTES` + `MARGIN_BYTES`) and monitors at the watchdog's own cadence during clone; a below-trigger, unknown or lost reading aborts the seed and **refuses the command before any child**, never a cold build. The removable path keeps its own removal-only watchdog and registration reserve instead of the internal space trigger. |
| 7. The plan replaced the required SQLite fixture | §16.2 keeps the real `libsqlite3-sys`-with-`bundled`-SQLite case and §16.4 supplies a source-derived time and disk allocation, including staging, seeded Cargo home, two coexisting slots, the owned image, fixtures and cleanup, with a fit gate that stops and reports instead of trimming a case. |

| `6449728021` finding (revision 3) | Amendment |
| --- | --- |
| 1. The configuration digest did not follow Cargo's lookup | §3.1 now walks from the command's working directory through every parent to the filesystem root, then the Cargo home, with Cargo's precedence, `[env]` relative resolution to canonical absolute paths, and refusal of `/.cargo/config.toml`-style ancestors it cannot model; member-cwd, ancestor-above-home and differing relative-config regressions are required. |
| 2. The fixed environment list did not bound dependency-unit content | §3.2 adds the leaf-package boundary: eligibility needs a lock entry with **no dependency edges at all**, because a proc macro can only be invoked through a dependency and its ambient reads are not fingerprinted. The narrowing of proc-macro reach is returned to the Supervisor, and a non-vacuous differing-environment/proc-macro control is required. |
| 3. Git verification had no bounded, effect-free process protocol | §4.3 now reuses the repository's sanitized `freshness::Git` protocol: exact argv/environment (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `submodule.recurse=0`, `core.fsmonitor=false`, no network, no config write, no prompts), cleared `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`, bounded calls, wall wait, stdout and settle window, and positive closure; unknown closure refuses. |
| 4. The proof allocation arithmetic was not sound | §16.4 corrects the Cargo-home figure (archives and extracted trees coexist), adds the new dependencies to staging, and gives a per-case schedule with a stated basis and a total that fits the 300 s / 512 MiB bound with margin; an unresolved conflict would return to the Supervisor. |

| `6449965447` finding (revision 4) | Amendment |
| --- | --- |
| 1. The artifact whitelist did not prove which package produced a file | §4.2 classifies each `<literal package>-<hex>` directory **per unit** and joins each artifact set through rustc's own `.d` whose first rule names the artifact and whose inputs all lie inside the package's verified source root; a same-target workspace/path package resolves to its own root and is rejected, a duplicate `.d` claim on one stem makes the package ineligible, and a role whose join cannot be proven is not offered. (Revision 5 replaced the earlier one-directory rule, which wrongly rejected legitimate per-unit variants and the build-script case.) |
| 2. The lock protocol did not retain the admission barrier | §6.3 and §7 define one `Store::seed` lock lifetime — recipient record → removable sibling barrier → store admission → donor record (non-blocking) → pool lock — held from candidate read through the final donor revalidation and released in reverse; the pure Cargo planner runs under those locks and never takes one; the post-return donor re-read is gone; a competing clean/admission/donor transition is fenced by locks 2–4. The `status --measure` claim is corrected to lock-free, and the pool-lock hold is stated as bounded in work, not wall time. |
| 3. The SQLite oracle was vacuous for offered-run exclusion | §16.2 keeps the required SQLite case at its real scope and adds case 3: an eligible dependency-free git package with a build script whose compile unit must be offered, whose run payload and fingerprint must be absent from the plan, and whose runtime `OUT_DIR` must be the recipient's after donor removal, with a debug-only counterfactual that admits the run and must then print the donor's path. |
| 4. The disk allocation omitted the recipient's rebuild | §16.4 now uses explicit phases with lifetimes: staging; SQLite donor **and** the recipient's own SQLite rebuild coexisting; then deletion of the proof's own private pool before the image phase. Phase B peak is corrected to about 290-445 MiB, and the time schedule is about 155-290 s, with a stop-and-report gate and a Supervisor bound return. |
| Cache and reserve-gate wording | §4.3 binds a cache hit to the archive's recorded device/inode/size/mtime identity and re-derives on any change; §8.3 runs the reserve gate only when a non-empty plan exists, so a command with nothing to seed keeps today's behaviour. |

| `6450220961` finding (revision 5) | Amendment |
| --- | --- |
| 1. The one-directory rule excluded legitimate per-unit variants | §4.2 replaces package-wide uniqueness with per-unit classification: several `<literal package>-<hex>` directories are legitimate (library, build script, feature variants); each must be one complete unit or an excluded run-only directory; artifacts are joined through the `.d` whose first rule names them and whose inputs are inside the verified source root; a mixed/unclassifiable directory is unusable and a duplicate `.d` claim makes the package ineligible. The `seedctl` build-script case is therefore achievable, and its native control asserts a two-directory package is offered. |
| 2. The donor lock order was contradictory | §6.3/§7 now give one executable sequence: one admission lifetime (recipient record → sibling barrier → admission) spanning call 1 (`seed_candidates`, pool lock alone, read-only), the effect-free selection/reserve gate/markers, and call 2 (`seed`, donor record non-blocking → pool lock, held through the final check, released pool then donor). The catalog is re-read in call 2 and rejected if its generation, lease id or bytes changed. |
| 3. The reserve gate depended on a plan that did not yet exist | Selection (step 4) produces the plan from call 1's catalogs before the gate; step 5 samples the gate only with a non-empty plan and only then writes `intent` and the untrusted origin. A command with no plan samples nothing and keeps today's behaviour; a positive plan with a below-trigger reading refuses before any child. |

| `6450400975` finding (revision 6) | Amendment |
| --- | --- |
| 1. The `.d` join rejected ordinary pinned units | §4.2/§5.1 parse **every** rule (rustc writes the `.d`'s own rule first), accept `.rmeta` and `.rlib` emits and proc-macro `.dylib`/`.so`, and locate a build-script `.d` under `build/<literal package>-<hex>/`; the join is recorded at producing time and re-verified under the pin. Pure and native controls cover the ordinary shapes, so the `cfg-if` positive and the `seedctl` offered build-script unit are achievable. |
| 2. Selection read donor payload before the pin | §6.3 step 4 uses catalog data only (the recorded join), reads no donor payload, and the donation source re-verification reads the shared Cargo home; §6.3 step 6 performs every donor payload read under the donor record lock and pool lock, hashing each file and re-checking the `.d` rules against the recorded join. §7 states that call 1 reads control state only, and a native competing-`managed::lease` case proves an acquire/eviction between calls 1 and 2 aborts without reading or cloning. |
| 3. Ambiguous-name visibility was missing from status | §9.2 adds bounded skip-count buckets to the summary, `last.json` and the recorded `oxide status` line (up to eight fixed tokens, `u32` counts), including a mixed successful-plus-ambiguous seed and a zero-skip control; no walk or reconciliation is added. |

| `6450625847` finding (revision 7) | Amendment |
| --- | --- |
| 1. The package/target equality rule excluded the required positive | §4.2 now uses the normalized target name (`package_name.replace("-", "_")`), matching pinned Cargo's inferred lib naming and its rejection of a hyphenated explicit `[lib] name`; a genuinely renamed target stays ineligible, and uniqueness/collisions are evaluated after normalization across all source kinds. The `cfg-if` positive and the removable case are supported, and case 21 uses the valid `[lib] name = "cfg_if"` collision fixture. |
| 2. The ambiguous-name unit count had no source | §4.2/§4.4 add a bounded generation-bound **rejection record** written while the producing lease is held: per excluded normalized name, its reason and the number of candidate compile units classifiable at producing time, or `units_unknown`. §9.2 reports those donor-side unit counts (never package counts, never from an unpinned catalog or the recipient allowlist) in the summary, `last.json` and the recorded status line, with an explicit unknown bucket; case 23 covers a mixed success-plus-ambiguous donor and the zero and unknown controls. |

| `6450792089` finding (revision 8) | Amendment |
| --- | --- |
| 1. The fingerprint directory prefix used the normalized target name | §4.2/§5.1 and §10 now separate the **literal package-directory identity** (`.fingerprint/<literal package>-<hex>/`, `build/<literal package>-<hex>/`) from the **normalized target identity** (unit suffix `lib-<target>`, `dep-lib-<target>`, artifact stems `lib<target>-<hex>`), with the opaque `<16hex>` never predicted, normalization still the uniqueness/collision rule across all source kinds, and association restricted to the named role/layout cases. Pure grammar regression: `cfg-if-<hash>/lib-cfg_if` plus `foo-bar`/`foo_bar`; native cases 2 and 16 reconciled with that grammar. |
| 2. Time arithmetic corrected | §16.4 now states the upper estimates sum to 265 s of the 300 s (lower about 133 s), leaving 35 s; the uncounted diagnostic remains required and fit remains UNMEASURED, with no counted-native implication. |

| `6451045780` finding (revision 9) | Amendment |
| --- | --- |
| 1. The all-rules predicate rejected rustc's phony source targets | §4.2 now separates output rules from rustc's empty phony source rules, accepts a phony target only when it is byte-identical to an accepted prerequisite and inside the verified source root, and still rejects any unexpected, nonempty, out-of-root or unmatched rule plus duplicate artifact claims. §4.4's join record keeps the phony set separate from artifacts, and §5.1/§10 name the shape. |
| 2. Hash relationship was implicit | §4.2/§10 state that for every supported role the fingerprint-directory `<hex>` and the artifact/`.d` `<hex>` are the same 16-hex `unit_id` rendering; a differing pair, a non-16-hex value or a shape with either optional choice absent skips cold, and neither hash is predicted. |
| 3. No representative dep-info positive | §16.1 carries a complete pinned 1.97.1 dep-info fixture including the trailing phony source rules as a required positive beside the `cfg-if` and build-script fixtures, with counterfactuals (phony handling removed, an unmatched phony target, an out-of-root phony target) that must fail, and the positive native oracle retained. |
| 4. Budget arithmetic restated | §16.4 states the upper estimates sum to 265 s and the lower to about 133 s of the 300 s bound, leaving 35 s; both are marked UNMEASURED and the uncounted diagnostic remains mandatory, with no counted-native implication. |

| `6451189343` finding (revision 10) | Amendment |
| --- | --- |
| 1. The build-script role could not satisfy its own join | Revision 10 proposed hashed `deps/build-script-build-<hex>` plus an uplift under `build/`; revision 11 moved the hashed output under `build/`; **revision 12 rebinds the manifest to the pinned `output_dir`/`uplift_to` source**: the hashed rustc output and `.d` are `build/<literal package>-<hex>/build_script_build-<hex>` and `...-<hex>.d`, with the unhashed uplift `build/<literal package>-<hex>/build-script-build` in the same directory, kept fail-closed. The pure fixture and case 3 were corrected again; unsupported shapes skip. |
| 2. The held-recipient reset had no crash-replay journal | §6.5/§6.6/§13 delete the in-slot reset entirely: the design never renames or deletes inside a `Held` slot. Pre-clone failures are `skipped` cold builds; every post-first-clone fault (clone, digest, file type, donor transition, reserve breach, crash) records `reset` as *abandoned*, retains the untrusted generation and refuses, leaving deletion to the existing journaled `Prior::Unknown` eviction. No new capture name, intent format, replay boundary or downgrade fence is added, and a crash in `Spawning` before the group is recorded stays held for explicit recovery. |
| 3. The accounting statement contradicted the current policy | §8.2 now states oxide#146: per-slot independently measured records, the inode union only within one slot's walk, accepted cross-slot hard-link overcount, no seed-created hard links, no extent-sharing or free-space discount, and the post-seed membership/byte recheck against that figure with an incomplete walk reported as unknown rather than zero. |

| Supervisor `5073c632` (revision 11) | Amendment |
| --- | --- |
| A1 pinned custom-build layout | §4.2/§5.1/§10 and the pure/native oracles use the hashed `build/<literal package>-<hex>/build_script_build-<hex>` plus `...-<hex>.d`, with the unhashed uplift `build/<literal package>-<hex>/build-script-build` in the same directory kept fail-closed (byte-identical required). Revision 12 rebinds this to the pinned `output_dir`/`uplift_to` source. |
| A2 post-clone byte walk | §6.3 step 7 adds the bounded generation-bound walk before Cargo, recorded as `observed-while-held` (never a closed record, §9.1); revision 13 (A2′) publishes only on a **complete ownership-clean** walk, with no limit-gap or donor-figure substitution, and any incomplete or failed walk abandons seeding only, retains the untrusted recipient and continues as an ordinary cold build under normal admission with no extra grant (§8.1, §8.2, §13, case 24). |
| A3 seeding never blocks a build | §7/§6.5/§13 and case 22 now make a leased/evicting donor or unknown custody before the first clone a `donor busy` skip with a cold build; the reserve start-gate refusal is unchanged (case 19). |

| `6451613293` finding (revision 12) | Amendment |
| --- | --- |
| 1. The build-script manifest named files Cargo does not produce | §4.2/§5.1/§10, the pure fixture and case 3 now use the hashed `build/<literal package>-<hex>/build_script_build-<hex>` and `...-<hex>.d` from the pinned `output_dir`/`FileType`/rustc-preparation source, with the unhashed uplift `build/<literal package>-<hex>/build-script-build` in the same directory (byte-identical required, `.d` never naming the uplift). |
| 2. Incomplete walk treated as known accounting | §6.3 step 7/§8.2 require a complete ownership-clean walk to publish. Revision 12 briefly allowed a limit-only gap with the donor's recorded figure; **revision 13 (A2′) deletes that substitution entirely**: any incomplete or failed walk (bound, ownership, device, read) abandons seeding only, publishes nothing, retains the untrusted recipient and continues as an ordinary cold build under normal admission with no extra grant. Case 24 and its controls were rewritten again. |
| 3. Post-clone lease/gate fallback ambiguous | The cold-build/extra-grant path is removed: no reserved grant, no rebind, no gate re-entry, and the retained slot's accounting/disposal boundaries are named. Revision 13 (A2′) keeps that no-extra-grant rule while making an incomplete or failed *walk* abandon seeding only and continue as an ordinary cold build under normal admission. |

| Supervisor A2′ (revision 13) | Amendment |
| --- | --- |
| Complete-walk-only publication | §6.3 step 7, §8.1, §8.2 and §13 now publish only on a complete ownership-clean recipient walk; the limit-gap/donor-figure substitution is deleted. Any incomplete or failed walk abandons seeding only — no publication, untrusted recipient retained and never reused, then an ordinary cold build under normal admission with the ordinary grant bound and no extra grant — and case 24 proves those four facts for ownership, read, device, entry-limit and outright-failure injections. |

| Supervisor `fe432517` (revision 14) | Amendment |
| --- | --- |
| A1′ pinned Cargo 1.97.1 build-script names | The hashed rustc output and `.d` are `build/<literal package>-<hex>/build_script_build-<hex>` and `...-<hex>.d` (underscores); the unhashed uplift `build-script-build` remains byte-identical and fail-closed; the hyphenated hashed name is a negative fixture. §4.2/§5.1/§10/§16.1/§16.2 and the product classifier/pure fixtures follow. |
| Catalog publication timing | The catalog is built from the post-Cargo verified sources; a newly appearing source is accepted only after the release-time unchanged-since check, and a pre-Cargo entry that changes identity is skipped; §§4.1/§4.4 invariants are unchanged. |
| OUT_DIR libraries unoffered | A library whose `.d` names generated `OUT_DIR` inputs remains unoffered in v1; case 3's offered unit is the build-script compile unit and its library is the negative control. |
| Per-case timing | The uncounted diagnostic receipt records each reached case's elapsed seconds and an observation-only projection; it is not an acceptance gate. |

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
6. The catalog is built from the **post-Cargo** verified sources. A source
   first observed after Cargo (a fresh registry archive is extracted, or a Git
   checkout is created, only during the build) is accepted only after the
   release-time unchanged-since check; a package that also appeared before
   Cargo must have the same kind, roots, tree digest and macro flag, or it is
   skipped. A pre-Cargo pass alone can never catalogue a first build, and the
   release-time pass is the closed producing epoch's evidence.

The accepted consequence (binding `d1f281eb`) is a **short donor lifetime**:
the catalog is bound to that one idle generation and is invalidated by the
next lease of the slot (§4.6). A later changed build in the same slot never
extends or refreshes a catalog. Real donor availability is a hypothesis to
measure in the separately scoped real-project comparison, not a claim here.

### 4.2 Restricted association is partial, not provenance

The association rule for one package, applied on both sides independently:

- its **normalized name** maps to exactly one lock entry across **all** source
  kinds; the literal package name of that entry supplies the fingerprint
  directory prefix, while the normalized name supplies the unit suffix and
  artifact stem (the two identities above). Cargo normalizes an inferred
  library target as `package_name.replace("-", "_")`
  (`cargo-1.97.1/c980f4866` `src/cargo/util/toml/targets.rs:135-170`) and
  rejects an explicit `[lib]` name containing a hyphen
  (`targets.rs:1055-1060`), so the design's name is
  `package_name.replace("-", "_")`, not the literal package name: `cfg-if`
  has target `cfg_if` and is supported. Uniqueness is applied after
  normalization, so `foo-bar` and `foo_bar` collide and both become
  ineligible on that side, as does a registry `foo` beside a git `foo`;
- its lock entry has **no dependency edges at all** (§3.2), so its compilation
  cannot invoke a proc macro; a package with any dependency edge is ineligible
  on both sides;
- its lock source is a **crates.io-style registry with a recorded
  `checksum`**. Git/Git-revision, path, workspace, vendored and replaced
  sources are ineligible in v1 (Revision 15);
- path/workspace/vendored/source-replaced and `[[patch]]`-replaced entries are
  ineligible; a lock entry without a `source` is ineligible;
- its manifest (read from the verified source root) declares exactly one
  plain library target (not a proc macro) whose name is the normalized
  package name, and
  an explicit `[lib] name`, when present, must equal that same normalized
  name (so a hyphenated explicit name is unsupported by Cargo itself). A
  package that declares a different lib target name — a genuine rename — is
  ineligible in v1: the pinned adapter proves the inferred normalization, not
  an arbitrary rename, and two packages can expose the same target name;
- the donor and recipient lock entries agree on version and on
  source-plus-checksum (registry only in v1).

**Unit and file association (two identities, per-unit classification, `.d`
join).** Three facts must not be guessed: which package produced an artifact,
which fingerprint directory belongs to which unit, and which name form each
Cargo path uses. A file name alone proves none of them — another package,
including a workspace/path package, may declare a library target with the same
name, and Cargo's output and fingerprint names are built from unit metadata
with conditional fallbacks. Pinned `cargo-1.97.1/c980f4866` uses **two distinct
identities**:

- the **package-directory identity is the literal lock package name**, dashes
  preserved: `compilation_files.rs:231-245,278-281` builds
  `.fingerprint/<package>-<16hex>/` (so `cfg-if-<hash>` for `cfg-if`);
- the **target identity is the normalized name** (`package_name.replace("-",
  "_")`, `targets.rs:135-168`), used for the fingerprint unit suffix
  (`lib-cfg_if`), the encoded `dep-lib-cfg_if` entry and the artifact stem
  (`deps/libcfg_if-<hex>.rmeta`, `deps/cfg_if-<hex>.d`). An explicit `[lib]
  name` must equal that normalized name; Cargo rejects a hyphen there.

The `<16hex>` is Cargo's opaque unit metadata hash and is never predicted. The
fingerprint directory is **per unit**, not per package: a dependency with its
own library and build script legitimately has several such directories, and so
do ordinary feature/profile variants. Normalization is used for **uniqueness
and collisions** across all lock source kinds; when a normalized name resolves
to exactly one package, that package's literal name is the only directory
prefix accepted. The design classifies **per unit**, derives the artifact
association from rustc's own dep-info *at producing time*, records it in the
catalog, and lets Cargo's own component comparison decide freshness.

**Supported role/layout restriction (Revision 15).** Cargo exposes unit-id
and extra-filename choices separately, so the adapter accepts only the layout
it names for v1: fingerprint directories directly under
`<profile>/.fingerprint/` and plain registry library artifacts under
`<profile>/deps/`. A build-script directory under
`<profile>/build/<package>-<hex>/`, a proc-macro artifact, a unit-grouped build
directory, a custom `--out-dir`, a custom extra-filename or any other layout
skips the package — the grammar is not generalised. The build-script
paragraphs below are historical revision 11–14 text and are not offered in
v1.

The rules, applied inside the slot's `<profile>/` when the producing lease is
held:

1. **Classify every `<package>-<16hex>` fingerprint directory by unit.** For
   the eligible package's **literal** name `<package>`, each candidate
   directory must be exactly one of:
   - **one complete unit** — the triple for a whitelisted role
     (`lib-<target>`, `.json`, `dep-lib-<target>` for a library or proc macro;
     `build-script-build`, `.json`, `dep-build-script-build` for a
     build-script compile unit) plus only explicitly excluded completion
     entries (§5.1); or
   - **a run-only directory** — every entry is an excluded run-unit or
     completion entry (`run-build-script-build-script-build*`, `output`,
     `root-output`, `out/**`, `stderr`, `invoked.timestamp`), which is
     **never copied** and is not an error.

   Several directories for one role are legitimate variants and are all
   offered; an inert one simply fails Cargo's component comparison. A
   directory that mixes units, contains a `dep-*` that matches no single
   suffix of its own unit, or holds any entry that classifies as none of the
   above makes **that directory** unusable, and a package with no usable
   directory is not offered.
2. **Derive the artifact join from rustc's dep-info, parsing *all* rules.**
   Cargo passes `--emit=dep-info,metadata` or `--emit=dep-info,link`, and
   rustc's ordered output types put the dep-info file first, so the **first**
   rule target is the `.d` file itself, not the artifact
   (`cargo-1.97.1/c980f4866` `core/compiler/mod.rs`; rustc 1.97.1
   `rustc_session/src/config.rs` and `rustc_interface/src/passes.rs`). The join
   therefore never looks at rule order. The dep-info file lives under the
   unit's own output directory: `deps/` for a library or proc-macro unit and
   `build/<package>-<hex>/` for a build-script compile unit (Cargo's
   custom-build units take that directory as `output_dir`). Its file name is
   rustc's `<target-stem>-<hex>.d` with the **normalized** target stem (for a
   build script, the literal target stem `build-script-build`).

   Pinned 1.97.1 also writes an **empty phony rule for every input file**
   after the output rules (`passes.rs:653-666`: `src/lib.rs:` with no
   prerequisites, repeated for each source), and Cargo reads that file for its
   translation (`core/compiler/mod.rs:289-296,454-465`). Those source-file
   targets are not artifacts, so the grammar separates the two kinds of rule
   instead of requiring every target to be inside the slot:

   - **output rules.** A rule whose target is the `.d` file itself, or one of
     the whitelisted role artifacts — `deps/lib<stem>-<hex>.rmeta` (the
     ordinary `metadata` emit), `deps/lib<stem>-<hex>.rlib`,
     `deps/lib<stem>-<hex>.{dylib,so}` for a proc macro, or
     `build/<package>-<hex>/build_script_build-<hex>`. At least one output rule must
     name a whitelisted artifact, every output target must resolve inside the
     unit's own output directory, and its prerequisites must be non-empty and
     **entirely** inside the package's verified source root (§4.3); a
     same-target workspace/path package resolves to its own checkout root and
     is rejected. The stem is the normalized target identity, while the
     build-script directory keeps the literal package name;
   - **phony source rules.** A rule with **no prerequisites** whose target is
     not the `.d` and not an output artifact is accepted only when that target
     is byte-identical to a path already accepted as a prerequisite in an
     output rule, and resolves inside the same verified source root. It
     contributes no artifact and no prerequisite of its own;
   - **everything else is rejected.** A rule that is neither an accepted
     output rule nor a matched phony source rule — a nonempty unexpected
     target, an out-of-root or unresolvable phony target, a phony target that
     was never a prerequisite, a target outside the slot that is not a phony
     source rule, or a duplicate/conflicting claim — makes the package
     ineligible. Exactly one `.d` in the directory may claim the artifact
     path; a second claim makes the package ineligible.

   **Hash relationship per role.** For a dependency library or proc-macro
   unit Cargo derives the fingerprint directory's `<hex>`, the rustc output
   stem and the `.d` stem from the same `unit_id`, and only when both optional
   choices are enabled (`compilation_files.rs` unit-id and extra-filename
   paths). The adapter requires those three to be the same 16 lowercase hex
   value, and rejects a shape where they differ, where either optional choice
   is absent, or where any is not 16 hex digits.

   A build-script **compile** unit has its own pinned layout, and both the rustc
   output and the uplift live in the *same* build-script directory
   (`compilation_files.rs` `output_dir` and `uplift_to`). The extra filename
   hash applies to rustc's output and to the dep-info name
   (`target_info.rs` `FileType` naming; `core/compiler/mod.rs` rustc
   preparation), so in the supported non-unit-grouped layout the compiled
   executable is `build/<literal package>-<hex>/build_script_build-<hex>` and
   its dep-info is `build/<literal package>-<hex>/build_script_build-<hex>.d`,
   with the stem `<hex>` equal to the fingerprint directory suffix (the same
   `unit_id`). Cargo then **uplifts** it to the unhashed
   `build/<literal package>-<hex>/build-script-build` in that same directory.
   Both are role files and both are recorded in the catalog: the join is proven
   by the `.d` rules on the hashed rustc output (never on the uplift), and the
   uplift is accepted only when it is a regular file in that hashed directory
   whose content is byte-identical to the hashed output (same size and SHA-256).
   A missing, misplaced, differently hashed or non-identical uplift skips the
   role cold. Neither hash is predicted; the equality is a checked property of
   the producing slot, and every other shape (including a unit-grouped build
   directory) skips cold.

   This binds the source root, the artifact and the unit's dep-info together.
   The catalog records the `.d`'s relative path, the artifact paths and the
   produced **join record** (its output-rule targets, the accepted phony
   source targets, the prerequisite roots and the file's SHA-256), so
   selection can decide association without reading the donor payload (§6.3).
   The join record keeps the phony targets separate from artifacts, so a later
   re-verification can prove the phony set still matches the prerequisite set
   without treating a source file as an output. For a build-script compile
   role it also records the uplifted
   `build/<literal package>-<hex>/build-script-build` as a role file with its
   own digest; the uplift is never a `.d` rule target.
3. **Re-verify under the pin.** Every artifact, `.d` and fingerprint file the
   plan names is re-read under the donor pin and donor record lock (§6.3 step
   6), hashed against the catalog, and its `.d` rules re-checked against the
   recorded join record. A donor file that changed, or a `.d` whose rules no
   longer match, aborts the whole seed; nothing is copied on catalog data
   alone.

**Rejected candidates are recorded while the producing lease is held.** An
excluded lock entry (ambiguous normalized name, non-leaf, unsupported source,
join unprovable, run-only, or beyond a cap) never becomes an offered unit, so
selection cannot count it later from an unpinned catalog. The producing epoch
therefore writes a bounded rejection record into the catalog, per excluded
package: its normalized name (for reporting), its literal name (for the
directory grammar), the reason token and the number of **candidate compile
units** it can classify for that package at producing time (the
`<literal-package>-<16hex>` directories that hold a complete whitelisted-role
triple). A package whose unit count
cannot be classified is recorded with `units_unknown: true` rather than a
zero, so an unknown is never reported as a count of zero. The counts are
donor-side facts about this catalog; a recipient-side allowlist exclusion is a
different predicate and is not folded into them.

A role whose join cannot be established is not offered. Nothing here predicts
Cargo's package-id or unit metadata hash: the artifact identity comes from
rustc's own dep-info, the fingerprint directory is classified by its own unit
suffix, and Cargo alone decides whether the pair is fresh.

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
- per package: literal lock name (the directory prefix) and normalized target
  name (the unit suffix and artifact stem), version, source string, checksum
  or full
  revision, uniqueness evidence for the donor lock (after normalization), the
  **leaf evidence** (the lock entry has no dependency edges, §3.2), source-root
  kind, and the **canonical source tree digest** verified at both ends of the
  producing window (§4.3);
- a bounded **rejection record** for excluded lock entries (§4.2): normalized
  name, reason token, candidate-unit count and `units_unknown` when it could
  not be classified — at most 256 entries and 4,096 counted units, names
  ≤ 64 bytes; the counts feed the summary and status skip buckets (§9.2);
- per unit: role (`lib`, `proc-macro`, `build-script-compile`), target name,
  the `.d` relative path and the **join record** (its rule targets, prerequisite
  roots and the `.d`'s SHA-256), and the exact relative file list (the manifest
  of §4.2 plus the role's artifacts), each with size, mtime and a **SHA-256
  content digest computed from the closed producing epoch**.

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

The fingerprint directory is
`<slot>/build/<profile>/.fingerprint/<literal package>-<16hex>/` (dashes
preserved, e.g. `cfg-if-<hash>`) and the artifacts live under
`<slot>/build/<profile>/`. The unit suffix and artifact stem use the **normalized** target name
(`cfg_if`), never the literal package name. `<target_underscored>` is that
normalized target name; `<hex>` is Cargo's opaque unit metadata hash, copied
verbatim and never predicted.

Every offered unit requires its **join file**: rustc's dep-info in the unit's
own output directory — `deps/<target_underscored>-<hex>.d` for a library or
proc macro, and `build/<literal package>-<hex>/build_script_build-<hex>.d` for
a build-script compile unit (its rustc output is the hashed
`build/<literal package>-<hex>/build_script_build-<hex>`, which Cargo uplifts
to the unhashed `build/<literal package>-<hex>/build-script-build` in that same
directory, §4.2). All of its rules are parsed; rule order is irrelevant because
rustc writes the `.d`'s own rule first, and the empty phony rule rustc writes
for each input file is matched against the accepted prerequisite set rather
than rejected as an output (§4.2). The `.d` is required, not optional; a unit
without a proven join is not offered.

| Role in v1 | Complete fingerprint triple (inside the package directory) | Artifacts (with the required `.d`) |
| --- | --- | --- |
| plain registry dependency library | `lib-<target>`, `lib-<target>.json`, `dep-lib-<target>` | `deps/lib<target_underscored>-<hex>.{rmeta,rlib}`, joined by `deps/<target_underscored>-<hex>.d` |
| proc-macro, build-script compile/run, Git/Git-revision, workspace/path, outside-root `.d` units | **excluded entirely** | never classified or offered in v1; each skip is named and the command builds normally |
| final outputs, uplifted binaries, test/bench executables, incremental | **excluded** | live in the checkout `target/` or are final uplifts |

A library whose `.d` names a generated `OUT_DIR` input (a build script's
generated source, for example) remains **unoffered** in v1: the join's
prerequisites must sit inside the verified source root, and a generated input
does not. The package is not rescued by any other role: v1 never offers its
build-script compile or run unit, and never offers a proc macro. Case 3's
seedctl package is a negative control, not a build-script positive.

The three fingerprint entries are the unit's complete Cargo-visible state and
are all required: the hash file, the encoded components, and the encoded
dep-info whose absence Cargo 1.97.1 already treats as stale. These names are
the adapter's, and §10 requires them to be confirmed against a real 1.97.1
build before any seeding is enabled. A unit with any of them missing is not
offered. Artifacts are matched by the `<hex>` variants of the allowlisted
library stems **and** by the `.d` join of §4.2; unlisted stems, unknown
suffixes, symlinks, directories, Finder metadata and `.dSYM` bundles are never
copied, and a fingerprint-directory entry that classifies as none of the
rows above makes that directory unusable — a package with no usable directory
is not offered (§4.2). A build-script-only directory and a proc-macro library
directory are skipped by name; they never become a `SeedRole` offer.

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

Seeding runs at most **once** per command. It adds no grant to the thirteen-grant
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
- at least one donor catalog overlaps the allowlist after the §4.2 join and
  the §3.1 context check; the candidates are chosen by rank before any overlap
  is known, so a graph whose only overlap sits outside the eight newest
  candidates is a stated skip, not an unbounded search (step 4 evaluates it);
- the seed start gate passes (§8.3): the internal pool's host reading is
  present and at or above the watchdog trigger. A missing or below-trigger
  reading is **not** a skip — it refuses the command before any child. The
  gate is sampled only after step 4 produced a non-empty plan (§6.3 step 5).

If a *precondition* fails, the command prints one `seeding skipped: <reason>`
line and builds cold. A skip is never an error, never retried and never
converted into a refusal. The reserve and ownership gates are not
preconditions: they refuse (§8.3, §13).

### 6.3 Split of responsibility, the effect-free selection, and the clone

Cargo semantics stay in the front door; custody stays in the lease library. A
single `Store::seed` entry point owns **one admission lifetime**, and the
library exposes two calls with one consistent lock order: a read-only candidate
read that takes the pool lock alone, and a mutating seed that takes the donor
record lock *before* the pool lock. Selection is effect-free and happens before
the durable marker; nothing is written and no reserve is sampled unless a real
plan exists.

1. **Read-only preparation (front door, no locks, no writes).** The command
   reads the recipient lock, computes the allowlist (§4.2) and the producer
   context digest (§3.1), and reads the candidate donor slot ids by rank from
   the store's own state. If any precondition or cap fails, or the allowlist is
   empty, it prints one `seeding skipped: <reason>` and stops: no reserve
   sample, no marker.
2. **Admission lifetime (front door).** `Store::seed` takes the removable
   sibling barrier (internal admission) and then the store admission lock, in
   the established internal→removable order, and keeps **both** until the whole
   transaction ends.
3. **Effect-free candidate read (library call 1, pool lock alone).**
   `managed::seed_candidates` takes the pool lock, validates that the recipient
   is still `Held` by this lease, reads at most eight donor catalogs chosen by
   rank (each generation-checked against the ledger), releases the pool lock and
   returns their bounded bytes. It takes no donor record lock and mutates
   nothing.
4. **Selection and plan (front door code, catalog data only, no locks, no
   writes).** The planner filters candidates by context-digest equality,
   intersects the allowlist, applies the **recorded** per-unit join and
   source-root evidence from the catalog (§4.2, §4.3), chooses **one** donor
   deterministically (greatest overlap, then most recent rank, then slot id) and
   builds the bounded relative-name plan. It reads **no donor payload**: the
   `.d`, fingerprint and artifact association it uses is the join record the
   producing epoch wrote into the catalog, so selection cannot touch a slot that
   a competing command may have taken. Donation source re-verification reads the
   shared Cargo home, never the donor slot. If no plan exists, the command
   releases the admission lock, prints `seeding skipped: <reason>` and builds
   cold — **the reserve gate is never sampled**, which is what preserves
   today's behaviour for a command with nothing to seed.
5. **Reserve gate, then durable markers (front door, admission lock held).**
   Only with a non-empty plan does `Store::seed` sample the §8.3 gate. A
   below-trigger, unknown or unreadable reading **refuses before any child**
   with the reserve-stop diagnostic: no marker, no clone, no Cargo. Otherwise
   it writes the per-generation attempt record as `intent` and the untrusted
   origin (`mark_seed_start`):
   `<pool>/oxide-managed/origins/<record>.json` with `origin: null` and the
   record's current mark, synced. With the mark unchanged this already reads as
   `Prior::Unknown` via `trusted_origin` (`store.rs:3577-3591`), so a crash from
   here on leaves a slot no later command may reuse as warm. Both writes use
   private helpers that assume the admission lock already held, so no second
   `flock` is taken. The untrusted origin is superseded only by the normal
   `release` write of a `Built` origin after Cargo closes — never by the seed.
6. **Seed transaction (library call 2, donor record lock then pool lock).**
   `managed::seed` takes the donor's record lock non-blocking with
   `create: false` (`store.rs:4509`), then the pool lock, and holds both to the
   end. Under them it re-reads the selected donor's catalog and **rejects it if
   its generation, lease id or bytes differ from the plan's** (the gap between
   calls 1 and 6 is verified, not trusted), re-validates the whole donor
   predicate (§7), the recipient's `Held` generation, the empty recipient
   `build/`, the same payload root and device, and, for a removable pool, the
   volume binding. **Every donor payload read happens here, under the pin**:
   each planned artifact, `.d` and fingerprint file is opened, hashed against
   the catalog's recorded digest and, for the `.d`, its rules re-checked against
   the recorded join record before anything is copied. It then clones the plan
   with `clonefile` only and hashes each clone against the same digest, and
   re-reads the donor's ledger state, requiring it still `Idle` and
   generation-matched. Any donor transition, digest mismatch, `EXDEV`,
   `ENOSPC`, `EDQUOT`, unexpected file type, permission change or unreadable
   path abandons the whole seed: no partial unit is ever published.
7. **Post-clone recipient walk, before Cargo (publish only on a complete
   ownership-clean walk).** With the recipient still held and the donor pin in
   place, the library walks the recipient's own slot with the existing bounded
   slot-walk limits (2,000,000 entries, depth 64, the library's 60 s
   cooperative slot deadline) and records the result as an
   **`observed-while-held`** fact bound to this store, slot, incarnation and
   lease generation — never as a closed record and never as a reuse
   measurement. The seed is published **only** when that walk is complete and
   ownership-clean: every entry owned by this user, on the pool's device,
   readable, inside the slot, with the coverage the capacity contract requires
   for admission (`local-lease.md#capacity-and-eviction`, §"Recorded per-slot
   accounting"). There is **no limit-gap, donor-figure or other substitution**: a
   bound-limited, ownership, device, boundary, read or any other incomplete or
   failed walk **abandons the seeding attempt** — it publishes nothing, writes
   no `seeded` claim and no last-seed sidecar, and marks the cloned recipient
   untrusted with no trusted origin. The generation is retained (never reused,
   never trusted) and the ordinary journaled `Prior::Unknown` whole-slot
   eviction removes it later. The command then proceeds as an **ordinary cold
   build under normal admission**: the normal wait, host-limit, capacity and
   reserve start-gate behaviour applies, the ordinary thirteen-grant loop is
   unchanged and **no extra, reserved or failure-specific grant exists**. If
   normal admission waits or refuses, the command follows that outcome.
8. **Release order.** The library drops the pool lock, then the donor record
   lock; `Store::seed` then writes the terminal attempt state and the last-seed
   sidecar and drops the admission lock and the sibling barrier, in reverse
   order. The recipient record lock is the command's and stays held until it
   releases or exits.
9. **Publication.** On complete success the receipt (units, files, logical
   bytes, donor identity, catalog digest, elapsed) is returned **while the pool
   lock and the donor record lock were both still held through the final donor
   check**; the command then writes the `seeded` attempt state, the bounded
   last-seed sidecar (§9.2), and runs Cargo. The recipient's origin is still the
   untrusted marker until `release` writes the trusted one.

No donor payload is read outside the pin, and no donor is re-read after custody
is released. Every transition that could move a donor out of `Idle` needs the
donor record lock, the admission lock, or the pool lock, and each is either held
or explicitly revalidated at every boundary: a competing `managed::lease`
acquire, eviction or release that slipped between calls 1 and 6 is caught by the
exact catalog re-read, the ledger recheck and the per-file digest checks, and
the seed aborts rather than reading or cloning a slot that is no longer
positively idle.

### 6.4 What the recipient holds afterwards

The cloned files are ordinary owned entries of the recipient slot: they are
counted by its next closed recording, they are read by Cargo, and they are
removed with the slot by ordinary whole-slot eviction, clean, or a discarded
untrusted generation. No reference to the donor remains — no hard link, no
symlink, no shared extent bookkeeping (APFS may share extents, which Oxide
neither measures nor claims, §8).

### 6.5 Failure: abandon before the first clone or on a failed recipient walk; refuse every other post-clone fault

Seeding never renames or deletes inside the recipient, because the library's
journaled deletion is `Evicting`-only with no lease (`src/lease/pool.rs:1963-1981`),
an unrecorded `.oxide-capture-*` name is refused (`src/lease/mod.rs:913-928`),
and owned deletion needs caller-owned durable intent while unjournaled residue
is never consumed (`src/owned_delete.rs:37-48,580-585`). There is therefore no
in-slot "reset" path at all. The outcome classes are:

- **Abandon before the first clone.** A skip (no donor, no overlap, an
  ineligible package, a cap, the seed's own cooperative deadline) or a
  selection/plan/source failure writes nothing to the recipient: the attempt
  record becomes `skipped` and the command builds cold in the same empty slot.
  A reserve or ownership fault instead refuses before any child (§8.3, §13).
- **First-clone boundary.** The first `clonefile` is the point of no return for
  *trust*: before it, abandoning is free and the command still builds cold; after
  it, the recipient is untrusted. What happens next depends on the fault: a
  failed recipient walk abandons the seed and cold-builds (§ below), while every
  other post-clone fault refuses.
- **After the first clone, a failed recipient walk abandons seeding and
  cold-builds.** A bound-limited, ownership, device, boundary, read or otherwise
  incomplete or failed post-clone walk (§6.3 step 7) is not a command failure:
  the seed is abandoned, nothing is published, and the cloned recipient is
  marked untrusted with no trusted origin. It is **released first through the
  ordinary release path** — including the removable flush-before-idle, which
  retires the slot instead of marking it idle if the flush fails — and is never
  reused and never built in. The command then continues as an **ordinary cold
  build in a normally admitted slot**: the normal wait, host-limit, capacity and
  reserve start-gate behaviour and the ordinary thirteen-grant loop apply, with no
  extra, reserved or failure-specific grant. If normal admission waits or
  refuses, the command follows that outcome.
- **After the first clone, every other fault refuses and retains.** A clone,
  digest or file-type error, a donor transition, a reserve breach or an
  unreadable path stops the seed, records the attempt as `reset` (meaning
  abandoned, not repaired), releases the recipient lease with no trusted
  origin, and the command **refuses** with a non-zero exit. (The failed-walk
  case above is the one post-clone outcome that does not refuse.) There is no second lease, no extra
  grant, no rebind and no fresh-slot cold build on this path. It does not start Cargo over the
  partially seeded slot and it does not delete anything in it: the untrusted
  origin and the `reset` attempt record keep the generation from being reused
  or published, and the next command's ordinary `Prior::Unknown` path evicts
  the whole slot through the existing journaled deletion, which is the only
  deletion machinery this design uses. Retention until that eviction is the
  documented cost of a post-clone fault; no new capture format, journal, replay
  boundary or downgrade fence is introduced.
- **Crash during the clone or the release.** The refusal path's outcome: the
  durable untrusted origin and the `intent`/`reset` attempt record survive, no
  trusted origin is written, no Cargo runs in the slot, and the next command
  evicts the generation whole. A crash during the **reset-record write** itself
  leaves the previous state (`intent`) or the origin, both non-trusted and
  non-publishable.

> **Implementation note (freeze amendment `eccede9f`).** Slice 2 takes the
> point of no return to be the first write of any kind into the recipient,
> including directory creation: before it a fault skips; from it onward a
> fault refuses and retains, while a failed recipient walk still abandons and
> cold-builds in a newly admitted slot.

**Refuse-path boundaries (release, flush, rebind, accounting).** Because the
refusal path takes no second lease, each boundary is named once:

- **Release.** `Store::release` runs the ordinary path with no trusted origin:
  the owned child temp is discarded if present (it is not, since the seed
  precedes `child_temp::choose`), and a removable pool is **flushed before
  idle** exactly as any release; a failed flush retires the slot
  (`Phase::Retired`) instead of marking it idle, and the untrusted generation is
  then handled by the existing retired/lost rules.
- **No rebind.** The output binding published before the seed is dropped with
  the lease; there is no second bind, no relink and no gate re-entry. The
  freshness gate already ran before the seed and is not re-run for this
  command.
- **No grant.** The refusal does not re-enter the lease loop, so the thirteen-grant
  bound, the host active-build count and the admission ticket are all
  unchanged; a `--admission-budget`/`--execution-budget` expiry is reported by
  its own existing path.
- **Accounting of the retained slot.** The incomplete generation is never
  counted as zero and never read as a closed record. The next admission walks
  a held slot fresh (as it always does) or, once the record is idle and
  untrusted, evicts it through the journaled whole-slot deletion, which removes
  its bytes and its capture residue under the #155 rules. The post-clone
  `observed-while-held` fact stays a bounded observation of that generation and
  dies with it.

**Failed-walk boundaries (the one post-clone path that cold-builds).** Release,
flush, retention and admission are named once: the untrusted recipient is
released through `Store::release` with no trusted origin (removable
flush-before-idle; a failed flush retires it), so no child is ever started in
it; the release also drops the output binding, and the cold build is an
ordinary fresh lease from the normal loop (normal waits, capacity, reserve gate
and grant bound). The abandoned slot is idle-but-untrusted and is evicted
before any reuse; its next admission figure is a walk or the journaled
deletion, never a zero.

### 6.6 Crash matrix

| Crash point | Durable state | Next command |
| --- | --- | --- |
| before the untrusted origin | new record, phase `Acquiring`, empty slot | existing recovery; slot has no trusted origin → `Prior::Unknown` → evicted |
| after the untrusted origin, before the first clone | untrusted origin, empty build | same as above (nothing to clean) |
| during cloning | untrusted origin plus partial `build/` | same as above: the generation is never reused, the whole slot is evicted |
| after a complete seed, before `mark_spawning` | untrusted origin, complete seed | whole slot evicted; no partial trust, no Cargo |
| after `mark_spawning`, before the process group is recorded | untrusted/stale origin, `Spawning` record | **stays held for explicit recovery** (`workflow.md#crash-recovery`); automatic eviction is not guaranteed |
| during Cargo, group recorded | untrusted/stale origin, Cargo may have run | existing recovery by group emptiness; then `Prior::Unknown` eviction |
| after Cargo, during catalog write/release | previous (untrusted) origin | eviction; a catalog is never made eligible by a generation the ledger does not confirm |

The poison is the persisted untrusted origin plus the generation binding, not a
new recovery engine. The design adds no new crash-recovery decision, no new
deletion intent format and no new capture name, and keeps the existing
writer-closure, quarantine and explicit-recovery rules.

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

**Lock order, one admission lifetime, two library calls.** Outermost to
innermost:

1. the **recipient record lock**, the command's own, held from its lease to its
   exit;
2. the **removable sibling barrier** (internal admission), when the selected
   pool is the removable one — the same internal→removable order `attempt`
   uses, unchanged, held for the whole transaction;
3. the **store admission lock**, which recovery, ticket handling and
   `oxide clean` take before they touch a record or the ledger, held for the
   whole transaction;
4. **call 1, read-only:** the **library pool lock** alone, taken and released
   around the candidate read and the recipient `Held` check. It reads control
   state (the generation-bound catalogs) only and never opens the donor's
   `deps/`, `.fingerprint/` or `build/`, so no donor payload is touched while
   the donor is unpinned;
5. **call 2, mutating:** the **donor record lock** (non-blocking,
   `create: false`) and then the **library pool lock**, both held through the
   clone and the final donor revalidation and released in that reverse order
   (pool, then donor record);
6. the admission lock and the sibling barrier are released last, by
   `Store::seed`.

A donor that is held, whose lock file is missing or whose lock a peer's
recovery holds is skipped, never waited for. Every transition that could move a
donor out of `Idle` — another command of that checkout leasing or releasing the
donor record, `oxide clean`, admission eviction, recovery or ticket handling —
needs the donor record lock, the admission lock or the pool lock. Nothing is
assumed across the gap between calls 1 and 2: the mutating call re-reads the
selected catalog and rejects a changed generation, lease id or bytes, and
re-reads the donor's ledger state before releasing. **A competitor that slipped
in means no seed, never a blocked build**: a donor that is positively leased or
evicting, or whose custody is unknown, before the first clone yields
`seeding skipped: donor busy` (or the matching custody reason) and a cold build
in the same empty slot. The §8.3 reserve start gate keeps its own refusal
behaviour. The pool-lock hold
is bounded in **work** (≤ 4,096 files, ≤ 384 MiB cloned, ≤ 8 catalogs, one
donor pin), not in wall time: a blocked filesystem call cannot be pre-empted,
and this design does not claim otherwise. The command's own admission and
execution budgets and the proof supervisor's wall clock bound the run; they do
not bound the lock, and a stuck syscall would hold it until the process is
stopped, which is the same exposure any library mutation has today.

`status --measure` is **passive and lock-free** (`workflow.md#passive-snapshot-and-measurement`,
oxide#128) and never waits on this lock; only an admission or another library
mutation does.

If a check fails while the transaction runs, the outcome follows §6.5: a donor
that changed phase, became leased or evicting, or whose custody is unknown
before the first clone is a **skip** (`seeding skipped: donor busy`, cold build
in the same slot), while a post-first-clone fault (digest, clone, device,
volume loss, reserve breach) records `reset`, retains the untrusted generation
and refuses (§13). A reserve start-gate failure keeps its own refusal.

## 8. Bounds, accounting, budget and watchdog

### 8.1 Bounds are finite and declared

| Bound | Value | Notes |
| --- | --- | --- |
| seed attempts per command | 1 | no retry, no extra grant |
| donor candidates examined | 8 | chosen by most recent rank then slot id; catalogs only |
| donor catalogs read | ≤ 8 × 2 MiB | strict schema; unknown → ineligible |
| recipient lock parsed | 4 MiB, 2048 packages | read only; no Cargo call |
| producer context digest | 16 config files (working directory to `/`, then the Cargo home), 4 MiB config bytes, 128 KiB rendered keys | unreadable/unsupported/unresolvable → skip |
| lock lifetime | recipient record → sibling barrier → admission (whole transaction); call 1 pool lock alone; call 2 donor record (non-blocking) → pool lock | released in reverse; pool holds bounded in work, not wall time (§7) |
| git source verification | `freshness::Git` bounds: 64 calls, 60 s total, 30 s per call, 16 MiB stdout, 2 s settle | exhaustion or unconfirmed closure → refuse |
| producing source verification (pre and post) | 2048 packages, 100,000 entries, 128 MiB file content, 64 MiB archive-derived content, 4 s each | deterministic lock order; overflow packages are not catalogued |
| producing-time artifact hashing | 8,192 files, 256 MiB, 4 s | a unit without all digests is not offered |
| catalog | 2 MiB, 2048 packages, 8,192 files, 3 units per package | overflow: no catalog |
| plan | 256 units, 4,096 files, 384 MiB logical bytes | overflow: skip |
| artifact join (all `.d` rules parsed) | 1 MiB per `.d`, 4,096 rules and inputs per unit, 32,768 inputs total | unparseable, duplicated or out-of-root input → package ineligible |
| skip-count buckets | 8 tokens plus an unknown count, `u32` each, in the summary and `last.json` | donor-side unit counts from the catalog; no walk |
| post-clone recipient walk | 2,000,000 entries, depth 64, the library's 60 s cooperative slot deadline | must be complete and ownership-clean to publish; `observed-while-held`, never a closed record; any incomplete or failed walk abandons seeding only |
| catalog rejection record | 256 entries, 4,096 counted units, names ≤ 64 bytes | at producing time only; overflow entries are recorded as `unknown` |
| source-verification cache | 2,048 records, 8 MiB sidecar, identity-checked per hit | missing/untrackable identity → full re-derivation or skip |
| donation source verification | 128 MiB content, 3 s | packages beyond the remaining cap are dropped from the plan, not the whole seed |
| seed work (clone + digest verify) | 5 s cooperative deadline, checked between filesystem calls | a blocked syscall cannot be pre-empted; not a hard lock bound |
| hashed bytes at donation | 2 × cloned bytes (donor + recipient copy) | the donor copy is compared with the producing-epoch digest; the clone with the same digest |
| seed sidecars total (`catalog`, `attempt`, `last`, `sources`) | 24 MiB | write refuses, or the oldest catalog is dropped, when exceeded |
| diagnostics | 2 KiB sidecar, one summary line ≤ 240 bytes | generated from post-abandon state only |

### 8.2 Accounting follows the recorded per-slot policy

Cloned files are new inodes with full logical and allocated bytes. Seeding
never creates a hard link, so it introduces no inode sharing at all.

The current admission figure is the **sum of independently measured per-slot
records** (oxide#146, `local-lease.md#recorded-per-slot-accounting-oxide146-implemented-native-held`):
each slot is measured and recorded on its own, the `(device, inode)` union
applies **within one slot's own walk**, and cross-slot hard-link overcount is
deliberately accepted by that policy. Seeding therefore needs no cross-slot
union and claims none. The recipient's cloned bytes enter its own next closed
record, written by the release walk; nothing fabricates an early record, and
the donor's record is untouched, so its figure keeps describing the donor's
own bytes. APFS may share extents between the two slots; Oxide does not measure
extent sharing, does not deduct it, and makes **no free-space or write-savings
claim**.

**Post-clone walk before Cargo.** The brief-required byte recheck runs *after
the last clone and before Cargo*, inside the same held generation
(§6.3 step 7): the library walks the recipient's own slot with the existing
bounded slot-walk limits and records the figure as `observed-while-held`. That
record is **not** a closed record: it never becomes the admission figure, never
masks a missing closed record, and is replaced by the release walk's closed
record when the slot next goes idle. The seed publishes only on a **complete
ownership-clean** walk; there is no donor-size, limit-gap or other substitution
for an incomplete or failed one. Any bound-limited, ownership, device, boundary,
read or other failure abandons the seeding attempt: nothing is published, the
cloned recipient is marked untrusted and retained, and the command proceeds as
an ordinary cold build under normal admission — normal waits, capacity and
reserve start-gate behaviour, and the ordinary thirteen-grant loop, with no extra or
reserved grant. The retained generation is never reused; the ordinary journaled
`Prior::Unknown` eviction removes it later, and until then it is not counted as
zero (the next admission walks a held slot fresh, and an idle untrusted slot is
evicted before it can be reused). No pool size, extent-sharing or free-space
saving is inferred from the seed.

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
  actually possible**: after the effect-free selection has produced a
  non-empty plan (§6.3 step 4) and before the attempt record, the untrusted
  origin and the first clone (step 5). It samples the payload's filesystem with the watchdog's own reader;
  unknown, below 22 GiB or unreadable means the command **refuses before any
  child** with the reserve-stop diagnostic, and no attempt record is written.
  A command with nothing to seed never samples this gate, so its behaviour is
  exactly today's. While cloning and
  abandoning, the seed samples at the watchdog's interval (about 1 s, and at
  least between every 64 copied files) with the same trigger; a breach or
  lost reading aborts the seed, retains the untrusted generation and refuses
  the command.
  The sampled minimum and the crossing are reported like a watchdog stop.
- **removable `apfs-volume-v1` pool.** Its watchdog is a removal detector and
  has no space trigger (`store.rs:1716-1726`), and the volume's own capacity
  is its hard limit. The seed therefore requires the verified mount/volume-UID
  binding and the registration's reserve reading (the existing admission
  rule), monitors the binding at the same cadence, and aborts and abandons on
  any binding or removal failure; the command then follows the existing
  lost-volume path rather than a generic cold build.
- **Deliberate asymmetry, stated for the Supervisor.** This gate is the
  seeding path's precondition; a command that seeds nothing keeps today's
  behaviour exactly. Unifying the start gate for every command would be a
  separate scoped change, and this design does not make it.

> **Implementation note (Oxide Supervisor ruling, PR #159 head `394c0c6`).**
> Slice 2 implemented the internal `directory-budget-v1` gate and seeding path
> under the Standard tier. **Elevated slice 4 (oxide#169)** removed that
> interim skip: `apfs-volume-v1` seeding now re-verifies the registration's
> mount/volume-UID binding and reserve before the durable marks, opens the
> verified volume payload under the pool lock for call 2, and monitors the
> binding at the watchdog cadence; the removable bullet above is the contract
> it implements. The interim "seeding not yet supported on a removable pool"
> reason no longer exists in the product.

The pool budget stays soft: a started command's seed may pass the cap exactly
as its Cargo may, and the seed's bytes are visible to the next admission. The
command's admission budget is never extended: an expiry before the clone means
no seed; an expiry during it abandons the seed and releases the
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
  (`intent`, `seeded`, `skipped`, `reset` — `reset` means *abandoned, not
  repaired*, §6.5) and, when `seeded`, the donor slot,
  unit/file/byte counts and the catalog digest. Never deleted for its
  generation, so a later command cannot claim an unseeded epoch;
- `<pool>/oxide-managed/seeds/last.json` — the last completed seed
  (§9.2), schema `oxide.cold-seed/1`, at most 2 KiB: generation envelope,
  unit/file/byte/elapsed facts and the bounded skip-count buckets;
- an `observed-while-held` fact inside the attempt record or the pool's
  observation sidecar: store, slot, incarnation, lease id, entry count, logical
  and allocated bytes, the walk's coverage and the bounded limits it used. It is
  bound to this held generation, is never a closed record and is never read as
  an admission figure (§6.3 step 7, §8.2);
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

### 9.2 Visibility, including ambiguous-name skip counts

Binding `bb151a59` requires the summary **and** status to report candidate
units skipped for an ambiguous name, so a successful seed that also dropped
ambiguity must say so. The counts are bounded, generation-bound and recorded
with the seed.

- A completed seed prints one line:
  `oxide: seeded <N> units (<M> MiB) from slot <X>`, where **N** is the number
  of completely cloned compile units and **M** is the logical cloned bytes.
  Neither number is a reused-unit count, a Cargo freshness claim or a
  physical-saving figure; the wording never says "reused" or "saved".
- When the chosen donor's catalog carries rejected-candidate units, the same
  line appends one bounded suffix: `; skipped <K> donor candidate units:
  ambiguous name (<a>), non-leaf (<b>), join unprovable (<c>), unverified
  source (<d>), unsupported source (<e>), run-only (<f>), cap (<g>)`, with
  only non-zero buckets printed and at most eight buckets from a fixed token
  set. `K` is the
  sum of the buckets. The counts are the catalog's recorded rejection facts
  (§4.2, §4.4) — donor-side **unit** counts, never package counts, and never
  inferred from the recipient allowlist.
- A rejection the producing epoch could not classify is reported as
  `unknown (<n>)` in its own bucket and is never added to `K` as if it were
  zero units, so the summary distinguishes "no ambiguous units" from "not
  measured".
- `last.json` records, beside the generation envelope, the same buckets and
  counts (integers, ≤ 8 entries plus the unknown count, each ≤ `u32`), so the
  numbers are bounded and survive a later command. A seed with no rejected
  candidates records an empty set, not a missing field.
- A miss prints `seeding skipped: <reason>` with the same bounded reason
  tokens. Reserve, ownership, device and custody faults are refusals, not skip
  tokens (§8.3, §13).
- `oxide status` renders at most one bounded recorded line from `last.json`,
  labelled as recorded: recipient slot, donor slot, units, MiB, file count,
  elapsed, when, and the same skip suffix when non-empty (truncated to one
  line, ≤ 240 bytes). It does not claim which copies Cargo kept fresh, and
  default status keeps its "recorded facts only, no walk, no lock, no
  reconciliation" property; the sidecar read is the same class of read as
  `state.json` and never mutates or reconciles.

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
  live in `<profile>/.fingerprint/<literal package>-<hex>/` with the
  three-entry unit manifest of §5.1 whose suffix and artifact stem use the
  **normalized** target name, whose fingerprint-directory and
  artifact/dep-info hashes are the same 16-hex `unit_id` rendering, and whose
  dep-info carries rustc's empty phony source rules, and only the §4.2
  v1 role/layout is accepted: a plain registry library's `.rmeta`/`.rlib`
  pair in `<profile>/deps/` joined by its `<profile>/deps/<target>-<hex>.d`.
  Proc-macro artifacts, build-script outputs and uplifts, Git/Git-revision
  sources and any outside-root `.d` input are not accepted; the profile
  directory name is Cargo's, and uplift copies finals to `CARGO_TARGET_DIR`;
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
  leftover rules are untouched. Abandoning a seed never touches `scratch/`.
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
   own checkout root, not to the verified registry root, so the `.d` join
   (§4.2) rejects it. Several fingerprint directories for one package are
   legitimate per-unit variants and are all offered; only a directory that
   mixes units or holds unclassifiable entries is unusable, and a duplicate
   `.d` claim on one artifact stem makes the package ineligible.
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
| cap or the seed's own cooperative deadline exceeded before the first clone | skip, cold build |
| a competing mount could not be fenced, or the admission/donor locks could not be taken in order | **refuse** (unknown custody) |
| donor archive or git tree fails verification; catalog digest mismatch before the first clone | skip the package, cold build |
| **internal pool host reserve unknown or below the 22 GiB trigger, before the first clone** | **refuse**; no marker and no clone is written |
| **reserve breach or unknown reading after the first clone** | abandon the seed, retain the untrusted generation and **refuse**; the ordinary journaled eviction removes it later |
| removable volume binding or registration reserve fails before the first clone | refuse; the existing removal/quarantine path runs |
| removable volume lost after the first clone | the existing removal/quarantine path runs; the untrusted generation is retained, never a generic cold build |
| donor became held/evicting/retired before the first clone, or volume mismatch | **skip** (`donor busy`), cold build; seeding never blocks the build |
| donor custody unknown before the first clone (a competing acquire, eviction or release) | **skip**, cold build with the custody reason; no donor payload read, no clone |
| post-clone walk incomplete, unknown or failed (bound, ownership, device, boundary or read) | **abandon seeding only**: publish nothing, release the untrusted recipient through the ordinary release (removable flush-before-idle; retire on failed flush), never reuse or build in it, then cold-build in a normally admitted slot; no extra grant |
| the ordinary cold build's normal admission waits or refuses | the normal wait/refusal outcome applies; the abandoned recipient is still retained and untrusted |
| a clone, digest or file-type check fails after the first clone | abandon, retain the untrusted generation, **refuse**; no Cargo runs in the slot |
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
- **Reacquire and rebuild on seed failure.** Spends a tenth grant; refusing
  and retaining the untrusted generation for the ordinary journaled eviction
  needs no new deletion machinery.
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
2. **Library custody.** `managed::seed_candidates` (call 1: pool lock alone,
   read-only) and `managed::seed(pool, request)` (call 2: non-blocking donor
   record lock then pool lock, held to the end) plus
   `managed::publish_seed_catalog`: catalog generation recheck, path and device
   validation, per-file digest revalidation, clonefile-only copy, final donor
   recheck and abandon-or-refuse, with the bounded caps. Unit regressions on tiny
   owned fixtures, no Cargo.
3. **Front door.** `Store::seed` with the one admission lifetime of §6.3/§7
   (recipient record → sibling barrier → admission across both calls), lock
   parsing, the §3.1 context digest, allowlist and per-unit `.d` join, source
   verification, catalog production with per-file hashes at release, the
   reserve gate after a non-empty plan, attempt record, summary/status
   sidecars, skip reasons, and the seed point in `main.rs`. No user-visible
   change when nothing is eligible.
4. **Proof apparatus.** The fixed script, its pure self-test and its
   diagnostic mode, then the owner-activated natives.
5. **Knowledge truth.** Merge the implemented contract into
   `workflow.md` and turn this document's status line into an implementation
   record; `vision.md`'s "approved, not built" bullet changes only then.

Each slice needs its own approved Queue brief, risk tier and covering checks.
This design mints none of that.

> **Implementation note (Oxide Supervisor ruling, PR #159 head `394c0c6`).**
> Slice 2 implemented the internal `directory-budget-v1` pool under the
> Standard tier, and **Elevated slice 4 (oxide#169)** landed the removable
> `apfs-volume-v1` support code together with the fixed proof script, its pure
> apparatus and its removable-media native case, as one slice. This knowledge
> record is folded into that slice. The frozen behaviour of §§6–8 is unchanged.
> The product now implements slices 1–4; slice 5 is this record and the
> workflow/vision truth, with the default flip as the isolated one-line product
> commit `bd61f7f` (not necessarily the last proof-script commit).

## 16. Proof plan

Tier: **HIGH** authoring complexity, **ELEVATED** (a post-clone abandon keeps
an untrusted generation that the ordinary journaled eviction later deletes,
and one case writes removable media). Author and independent reviewer
natives are separately owner-activated; before the first counted native the
author runs one separately activated, uncounted `--diagnostic` of the same
fixed script, and a source sweep never replaces it.

### 16.1 Pure checks (no child, no Cargo, no pool)

**Revision 15:** only plain registry library positives remain. Every build-script,
proc-macro, Git and outside-root `OUT_DIR` item below is a **negative** check,
not an offered positive; the producer and plan-selection regressions prove that
none of those units is ever offered or selected.

Lock parsing, the uniqueness rule, the **leaf rule** (a lock entry with any
dependency edge is ineligible on either side, including build and dev edges
the lock records) and the manifest target-name rule; the complete per-role
triple (hash, `.json`, `dep-…`) with every negative (missing `dep-…`, an extra
`dep-…`, a duplicate suffix, an unclassifiable fingerprint-dir entry, symlink,
`..`, absolute path, unknown suffix, `.dSYM`, Finder metadata, oversized);
archive-reader source-verification-cache identity controls (unchanged
identity hits without re-reading, a changed size/mtime/inode/device forces a
re-derivation, and an untrackable identity skips);
**per-unit classification**: several `<literal package>-<hex>` directories for one package
(role variants) are all offered; a run-only directory is classified, never
copied, and does not block the package; a mixed-unit directory or an
unclassifiable entry makes that directory unusable; **artifact join, all rules
parsed**: a `.d` whose first rule is the `.d` itself (the ordinary rustc shape)
still joins through its later artifact rule; `--emit=dep-info,metadata`
(`.rmeta`) and `link` (`.rlib`) both join; a proc-macro `.dylib` is never offered; a
build-script hash/uplift pair is never offered; the hyphenated hashed name is a
negative fixture; a library `.d` naming a generated
`OUT_DIR` input is not offered (positive package without generated inputs plus
that negative); a source first observed only post-Cargo is accepted, while a
pre-Cargo entry that changed identity is skipped; rustc's **empty
phony source rules** are matched against the accepted prerequisite set and
contribute no artifact, while a phony rule whose target was never a
prerequisite, is out of root or is nonempty is rejected; the
fingerprint-directory hash and the artifact/`.d` hash must be the same 16-hex
`unit_id` rendering, and a differing or non-16-hex pair skips; conflicts — two
`.d` files claiming one artifact, prerequisites outside the verified source
root or empty, an unexpected nonempty target outside the slot, a `.d` whose
recorded join record no longer matches;
a workspace/path package with a renamed equal-named lib target and a
same-target different-source collision are rejected; a build-script compile
unit whose join cannot be proven is not offered; **skip counts**: summary and
`last.json` buckets for ambiguous name and the other reasons, mixed with a
successful seed;
archive-reader negatives (tampered archive, extra tree file, missing tree file,
symlink/hardlink/device entry, duplicate path, absolute path, pax/gnu long
name, truncation); git verification negatives (dirty, wrong revision,
submodule, unreadable) and the **exact argv/environment** of every git call
(`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `submodule.recurse=0`,
`core.fsmonitor=false`, `-C` only, cleared `GIT_DIR`/`GIT_WORK_TREE`/
`GIT_INDEX_FILE`/`GIT_CONFIG_*`) plus a fake-git control that would run a
configured fsmonitor hook and a bounded-budget exhaustion refusal;
**name grammar**: a hyphenated package is offered from
`.fingerprint/cfg-if-<hash>/lib-cfg_if` while the artifact stem is `libcfg_if`
and the dep-info is `cfg_if-<hash>.d` (literal directory, normalized target);
a package whose literal name carries an underscore is unaffected; a
literal-name directory with a normalized-name suffix mismatch is not joined; an explicit hyphenated `[lib] name` is unsupported;
`foo-bar` and `foo_bar` collide after normalization and both become ineligible;
a genuinely renamed lib target is ineligible; the rejection record's
candidate-unit counts and `units_unknown`; the §3.1 context digest with a
synthetic config tree: equal contexts, differing flag,
differing table, **member working directory**, **ancestor above the account
home**, a config at the filesystem root, **differing relative `[env]` values**,
both `config`/`config.toml` in one directory, unsupported `--config`,
`[source]`, `[patch]`, `[registries]`, `[unstable]`, and unreadable/oversized/
non-UTF-8 config; catalog schema, leaf and source evidence, per-file digest
records, caps and generation binding; the durable attempt record's states and
the unseeded publication rule; producing-window mtime rule; plan intersection
and determinism; the abandon/retention decision; reserve gate and
monitored-stop
decision points against a fake reader; **lock-lifetime seams**: the planner
never takes a lock and never reads donor payload; call 1 takes only the pool
lock and reads catalogs only; call 2 takes the donor record then the pool lock
and releases in the reverse order; the admission lock spans both calls; and a
fake donor transition, catalog change or per-file digest change injected
between calls 1 and 2 aborts instead of producing a receipt; receipt
and summary/status rendering and their caps; adapter dispatch and
unsupported-version skip; donor predicate matrix (held, retired, evicting,
pending recovery, rank/lease mismatch, volume mismatch, missing lock, `seeded`
attempt); and the fail-closed matrix of §13.

**Representative pinned 1.97.1 dep-info (positive fixtures, non-vacuous).** The
pure suite parses these complete shapes, with the output rules followed by
rustc's empty phony source rules, and requires a *successful* join for a
`cfg-if` leaf (literal `cfg-if-<hash>` directory, `lib-cfg_if` unit,
`libcfg_if-<hex>.rmeta` artifact) and for a build-script compile unit
(hashed rustc output `build/seedctl-<hex>/build_script_build-<hex>` with
`build/seedctl-<hex>/build_script_build-<hex>.d`, plus Cargo's unhashed uplift
`build/seedctl-<hex>/build-script-build`; the hyphenated hashed pair is the
negative fixture):

```make
deps/libcfg_if-<hex>.rmeta: /root/.cargo/registry/src/index.crates.io-<idx>/cfg-if-1.0.0/src/lib.rs
/root/.cargo/registry/src/index.crates.io-<idx>/cfg-if-1.0.0/src/lib.rs:

build/seedctl-<hex>/build_script_build-<hex>: /root/.cargo/registry/src/index.crates.io-<idx>/seedctl-0.1.0/build.rs
/root/.cargo/registry/src/index.crates.io-<idx>/seedctl-0.1.0/build.rs:
```

(The real files write the `.d`'s own output rule first and one empty phony rule
per input; the fixtures cover both the `.d` rule and the phony rules, and the
build-script fixture's `deps/` output stem carries the same `<hex>` as the
fingerprint directory while the uplift is only accepted inside
`build/<literal package>-<hex>/`.) The counterfactual runs the same fixtures
with phony-target handling removed and must **fail** with an unexpected-target
rejection, so the regression is non-vacuous. Further fixtures add a phony
target that is not a prerequisite (must fail), an out-of-root phony target
(must fail), a build-script uplift outside its hashed directory or with a
different hash (must fail), and a build-script `.d` that names the uplift
instead of the hashed rustc output (must fail). The positive native oracle of
cases 2 and 3 remains the end-to-end counterpart.

### 16.2 Native cases (one fixed script)

On tiny private fixtures with private `HOME`, `CARGO_HOME`, `OXIDE_POOL`,
`OXIDE_TEST_HOME_FROM_ENV=1`, the invoking rustup home preserved, and a
private vendored/offline Cargo home seeded by the shared bounded seeder. The
four canonical temporary variables follow the repository recipe: the
supervisor's `OXIDE_TASK_SCRATCH_ROOT` and `OXIDE_TEST_INTERNAL_ROOT` point at
one fresh internal root for the genuine internal pool, and `TMPDIR`, `TMP` and
`TEMP` point at a separate fresh directory so ordinary test temp still follows
the selected-volume rule.

1. **SQLite negative package control (retained).** A private workspace whose
   dependency is `libsqlite3-sys` with the `bundled` feature (the SQLite
   amalgamation compiled by `cc`, exposed through `OUT_DIR` and the link-search
   replay). The donor epoch builds it; `libsqlite3-sys` has dependency edges
   and is **never offered**, so no catalog entry or plan names it. Its leaf
   `cc`/`pkg-config`/`vcpkg` registry libraries are ordinary v1 positives and
   may seed. The recipient's `libsqlite3-sys` build script reruns, the program's
   runtime `env!("OUT_DIR")` value and SQLite version are correct after the
   donor slot is deleted, and the oracle asserts the negative package is
   unoffered rather than requiring a global no-seed;
2. **leaf registry positive and complete manifest.** A small registry leaf
   (`cfg-if`, no dependency edges) is offered from its **literal-name**
   directory `.fingerprint/cfg-if-<hash>/` with the **normalized** unit suffix
   and artifacts (`lib-cfg_if`, `dep-lib-cfg_if`, `deps/libcfg_if-<hex>.rmeta`
   or `.rlib`, `deps/cfg_if-<hex>.d`), i.e. its full triple
   (hash/`.json`/`dep-…`) plus artifacts; two checkouts share one lock and
   Cargo home, the second leases a new slot that is seeded, and Cargo reports
   the offered dependency unit fresh (`--message-format` is caller-supplied by
   the proof, never injected). A debug-only proof control that drops only the
   `dep-…` entry from the plan must make the same unit rebuild, proving the
   manifest is the complete one;
3. **Git/build-script/`OUT_DIR` negative control.** A dependency-free git
   `seedctl` with a build script that writes a generated file into `OUT_DIR`
   and embeds that absolute path. v1 rejects the Git source and never offers a
   build-script compile/run unit or the `OUT_DIR` library, so the donor
   publishes **no catalog**, the recipient prints a named seeding skip, and
   the ordinary build reruns the script and prints the recipient's own
   `OUT_DIR`; the donor's path is absent. There is no `admit-run` positive
   counterfactual in v1;
4. **ambient-input control (leaf boundary, non-vacuous).** A dependency with
   an edge whose proc macro reads an inherited environment variable is
   **ineligible**; two recipient builds under different values of that
   variable both run cold and produce the two different values, proving the
   leaf rule prevents the donor's expansion-time ambient input from leaking
   into the recipient's code. Its control is the leaf case 2;
5. **Proc-macro negative control.** An owned scratch git repository holds a
   proc-macro crate; v1 never verifies it as a seed source, offers it, or
   selects its unit. The recipient builds it cold and expands with the
   recipient's own value; a non-leaf proc-macro fixture is likewise
   unoffered, and its artifact must not be reused. The product regression
   proves a proc-macro unit is never classified or selected; a configured
   `core.fsmonitor` hook is outside the v1 path because Git sources are
   ineligible;
6. **Git/build-script/`OUT_DIR` negative control (cold lib).** The same
   ineligible seedctl package publishes no catalog; the recipient's package
   library is rebuilt and its build script reruns in the ordinary cold build;
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
12. caps each skip without donor mutation: logical bytes, units and files.
    The work-deadline control is injected after the first clone and must
    **refuse**, retaining the untrusted recipient and its ledger/attempt
    evidence, not perform the A2′ incomplete-walk abandon;
13. git identity negatives: a dirty, submodule or differently-revisioned
    checkout is ineligible; a same-name registry/git collision is ineligible.
    Both collision checkouts have a resolved lock before Cargo starts and use
    `--locked`; the skip names the ambiguity even when the non-leaf root
    package comes first in that lock;
14. unsupported layout: a fake `cargo` that reports an unverified version
    makes the command skip seeding while Cargo still runs normally;
15. no donor writes: donor tree content digest, its catalog, ledger bytes and
    control bytes are identical before and after a seed, and its rank, use mark
    and recorded figure are unchanged. Compare the donor's full control record
    (canonical JSON) and raw origin sidecar, excluding recipient changes to the
    shared state document;
16. removable path: one owned 128 MiB APFS image (`hdiutil attach -owners on`)
    registered as a removable pool seeds a tiny `cfg-if` workspace (literal
    directory `cfg-if-<hash>`, normalized unit `lib-cfg_if`, artifact
    `deps/libcfg_if-<hex>.rmeta`) across two
    of its slots and detaches ordinarily, proving the same-volume predicate and
    that the recipient's bytes are on the image. Case completion uses the same
    collection retention gate as final cleanup: an earlier diagnostic failure
    retains the image and aborts, even when the removable case itself succeeded.
    The pure apparatus replays that interleaving through the case body;
17. registry tamper controls: the cached `.crate` replaced with a different
    archive (checksum mismatch), the extracted tree with one extra file, one
    altered file, one symlink and one missing file each make the package
    ineligible and the command cold — never a seeded copy;
18. seeded recipient never donates: after a successful seed and build, the
    recipient's `attempt` record says `seeded` and a third cold slot must not
    receive a catalog from it, even though its build began empty and
    succeeded. Removing only the original donor's leaf leaves its catalog
    selectable, so the third command reports `donor payload missing` and
    compiles the dependency cold; the seeded recipient still has no catalog;
19. reserve gate: an injected below-trigger reading (the existing debug-only
    free-space seam) makes a command whose only available donor would seed
    **refuse before any child**; a mid-seed breach aborts, retains the
    untrusted generation and refuses;
    the non-seeding control still starts Cargo as today, then its ordinary
    reserve watchdog sends TERM and Oxide returns 143. That control must name
    the ordinary watchdog and its Cargo group, never a seed-gate stop;
20. partial-trust negative: a fault-injected partial recipient is never reused
    as warm by a later command.

21. **artifact collision and workspace negative control.** A workspace/path
    package that declares `[lib] name = "cfg_if"` (a valid explicit name, and
    the normalization of the registry `cfg-if`) beside a genuine registry
    `cfg-if` must leave the plan containing only the verified registry
    artifact: the workspace/path package is ineligible in v1, the proof
    asserts its artifact path is absent from the plan and that the recipient
    still builds and runs correctly. The workspace fingerprint's actual hash
    identifies its emitted `libcfg_if` paths, and its dep-info must name the
    workspace source after both paths are canonicalized (relative dep-info
    paths resolve from the workspace); the pure apparatus covers an owned
    alias. Those artifact paths are absent from the selected catalog's exact
    file manifest, whose count must match the seed receipt. The Git/build-script seedctl package is a
    second negative control (no catalog, no seed), not a per-unit positive.
22. **competing library acquire between selection and the pin (no seed, no
    block).** With a debug-only pause ordered between call 1 and call 2, a
    second process runs a real `oxide` command that leases or evicts the
    selected donor (or a direct library `managed::lease` on it). Per §7 the
    seeded command must **not** block the build: it prints
    `seeding skipped: donor busy` (or the matching custody reason), reads no
    donor payload, clones nothing, and builds cold in the same empty slot with
    the ordinary exit code. The adversary's own slot state is unchanged.
    Without the interleaving the same fixture seeds normally, so the control is
    non-vacuous. The reserve-start-gate refusal (case 19) is unchanged and is
    not produced by this race.
23. **ambiguous-name visibility.** The donor workspace compiles one eligible
    leaf (`cfg-if`) and a normalized-name collision pair (`foo-bar` and
    `foo_bar`, both ineligible on that side). The producing epoch records the
    rejection with the **candidate unit count** it can classify for that name
    (two fingerprint directories), and the catalog carries it. The recipient
    seeds `cfg-if`: the seed summary (separate from the ordinary profile
    summary) shows the seeded counts **and** the bounded
    suffix naming `ambiguous name (2)`, and `oxide status` renders the same
    recorded bucket from `last.json` without any walk or reconciliation. A
    second donor compiles only `cfg-if` (`build -p cfg-if`): its unbuilt root
    app has a known zero candidate count, so it renders no suffix and an empty
    bucket set. In the workspace donor, the app binary and a planted
    unclassifiable `baz-qux` fingerprint each have an omitted `units` field,
    rendering `unknown (2)` instead of zero. The collision pair is one
    normalized-name rejection with two candidates, not two rejection rows.
    The pure apparatus accepts omitted unknowns and rejects substituted zeros
    or a lost grouped count; it also distinguishes the seed and profile lines.
24. **post-clone walk (publish only on a complete ownership-clean walk).** Two
    controls on one fixture. First, a complete ownership-clean walk publishes
    the seed with the `observed-while-held` fact; `oxide status` shows the
    normal recorded line and the next release writes the closed record. Second,
    each injected failure — an ownership error, an unreadable entry, a device
    boundary, an entry-limit truncation (bound) and a walk that fails outright —
    must prove the same four things: **no publication** (no `seeded` claim, no
    last-seed sidecar, no summary "seeded" line and the reported
    `seeding skipped`/abandon reason), the cloned recipient **marked untrusted,
    released through the ordinary release path and retained** (never reused,
    never built in, removed only by the ordinary journaled `Prior::Unknown`
    eviction), and the command completing as an **ordinary cold build in a
    normally admitted slot** with the ordinary grant bound and no extra or
    reserved grant. The controls are non-vacuous: the same
    fixture with no injection publishes normally. A gated Cargo build exposes
    the successful attempt's generation-bound observation while its ledger row
    is held with no closed record; status shows the recorded seed, and release
    subsequently writes the closed record for that lease. For each gap the
    ordinary `Prior::Unknown` diagnostic must name the reset attempt's slot and
    its missing trusted build record, followed by removal of that whole leaf
    and ledger row and a cold `cfg-if` build in another slot. The isolated
    control's durable use-counter delta accounts for the three ordinary grants
    (seed, unknown retry, cold fallback), two releases and one Cargo spawn;
    deleted grants remain included in that count and the nine-grant bound.

### 16.3 Runner, bounds, clocks and activation

Tom decision `1527723f` raises only these two selectors to 600 s. The 480 s
uncounted diagnostic at `86208d1` remains historical with no counted credit:
case 21 failed its dep-info source assertion, case 22 aborted at the work
cutoff, and cases 23–24 were unexecuted. That run established neither complete
fit nor a product PASS. Case 21 now compares canonical source paths; a fresh
exact-head review precedes a separately owner-activated 600 s diagnostic and,
only after a clean complete diagnostic, the separately activated author and
independent reviewer natives. The 512 MiB budget, 30 s margin, safeguards and
Elevated merge hold are unchanged.

- Selector (created by oxide#169):
  `effigy rust:cold-slot-seeding` = one fixed script and the whole command of
  the qualified shared supervisor,
  `python3 scripts/bounded-test.py --root-parent /private/tmp --limit 600 --scratch-budget-mib 512 -- python3 scripts/test-cold-slot-seeding.py`.
  The private pool is a genuine internal `directory-budget-v1` pool, so the
  recipe declares the owner-accepted internal-placement exception (decision
  `bd357a4c`) inside the same monitored owned root; the supervisor bytes are
  the `addaae5a…` pin above, with `--limit 600` (Tom's decision `1527723f`, this selector and its diagnostic twin only; any higher bound needs another operator ruling) and a 512 MiB monitored
  scratch budget including cleanup and the one owned image.
- `effigy rust:cold-slot-seeding:diagnostic` appends the script's
  `--diagnostic`: the uncounted keep-going collection required before a counted
  run (records reached/completed/failed/dependent-skipped/unexecuted cases and
  the latched first fault in a bounded `qualified: false` receipt, continues
  only into dependency-independent cases, aborts on unknown custody, device,
  accounting, deadline or receipt outcomes).
- The cold script derives its own work clock from the supervisor deadline with
  the 30 s shutdown margin (the imported finder helper's 300 s constant is
  overridden only in this selectors' process), so the 600 s bound applies to
  both the guard and the script.
- `effigy proof:cold-slot-seeding:test` runs only the script's pure
  `--self-check`/`--self-test` with synthetic seams: no child, pool, image or
  Cargo.
- Proposed clocks: author 120 minutes and reviewer 45 minutes cumulative
  including admission waits, covering checks, the native and cleanup, with no
  reset and no extension; the independent design review ratifies feasibility
  before promotion.
- Every receipt states that it ran under the qualified shared supervisor's
  heavy admission (the retired host-run client `f6b3b50` and Queue #381 are
  historical; admission has been healthy since 24 Oct 2026). Counting follows
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

**Run history.** A counted author native of `effigy rust:cold-slot-seeding`
passed **24/24 cases at `cb7cd97`** (#169), after the case-21 canonical-source
correction and the 600 s bound; that run's guard receipt holds its time,
scratch and reserve figures. The earlier 480 s uncounted diagnostic at
`86208d1` stays historical with no credit. A further native at a changed head
still needs its own owner activation.

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

Time, per required case, against the 600 s wall clock (estimates; the basis
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
| 22 competing acquire between selection and the pin (skip, cold build) | 3-6 s | a debug pause plus one competing `oxide` command and a tiny cold build |
| 23 ambiguous-name visibility | 2-4 s | one tiny build plus summary/status rendering |
| 24 post-clone walk (two controls) | 2-6 s | a tiny walked slot and the retained generations after a failed walk |
| cleanup, receipts and the phase table inside the guard | 5-10 s | bounded writes and one detach |

Revision 15 raised only these selectors from 300 s to **480 s**; decision
`1527723f` now sets 600 s. The following 271 s arithmetic and 275 s stop
proposal are historical, superseded by that approved bound and the fresh
exact-head diagnostic requirement. The upper estimates sum to **271 s of the earlier 300 s bound** (the lower estimates to
about 135 s), leaving 29 s at the top of the range. That is the arithmetic of a
plausible accounting, **not** a demonstrated fit: the script prints the phase
and case tables, and if the first required uncounted **diagnostic** run's
projection or measurement exceeds 275 s it **stops and reports**. The
diagnostic remains mandatory before a further counted native at a changed
head; these allocation figures were never measured at the head that wrote
them (the counted pass at `cb7cd97` is recorded in 16.3). If the
required SQLite and offered-run cases cannot fit even so, the conflict returns
to the Supervisor for a scoped bound amendment before freeze; this design does
not substitute a cheaper fixture and does not treat the separate real-project
comparison as the proof.

## 17. Independent review questions

1. Is the §4.2 build-script role manifest accepted — the hashed rustc output
   `build/<literal package>-<hex>/build_script_build-<hex>` with its
   same-directory `.d` and the unhashed, byte-identical
   `build/<literal package>-<hex>/build-script-build` uplift — together with the
   §6.3 step 7 post-clone walk (publish only on a complete ownership-clean walk;
   any incomplete or failed walk abandons seeding only, retains the untrusted
   recipient and continues as an ordinary cold build under normal admission
   with no extra grant) and the §7 rule that a leased/evicting/unknown donor is
   a `donor busy` skip rather than a block?
2. Is the §6.5 policy accepted in place of an in-slot reset — a failed
   recipient walk abandons the seed, releases the untrusted recipient and
   cold-builds in a normally admitted slot, while every other post-first-clone
   fault refuses and leaves the untrusted generation to the existing journaled
   eviction?
3. Is the §4.2 dep-info grammar accepted — output rules distinguished from
   rustc's empty phony source rules, phony targets matched to accepted
   prerequisites inside the verified source root, and the fingerprint /
   artifact `unit_id` hash equality required for every supported role — with
   the representative pinned fixture as the non-vacuous pure positive?
4. Is the §3.2 leaf boundary accepted as the positive ambient-input boundary,
   with proc-macro reach returned to the Supervisor rather than silently
   narrowed — or does the Supervisor want the broader dependency graph, which
   would need the deferred artifact-JSON/unit-graph capture plus a reviewed
   proc-macro input-tracking mechanism?
5. Is the §4.2 **two-identity** rule accepted — the fingerprint package
   directory uses the literal lock name (`.fingerprint/cfg-if-<hash>/`) while
   the unit suffix and artifact stem use the normalized target
   (`lib-cfg_if`), normalization is the uniqueness/collision rule across all
   source kinds, the association is restricted to the named role/layout
   cases, and the rejection record reports donor-side candidate-unit counts
   (or `units_unknown`) rather than package counts?
6. Is the §4.2 per-unit association accepted — each `<literal package>-<hex>` directory
   classified as one unit or a run-only directory (variants offered, run
   payload never copied); each artifact set joined by parsing **all** rules of
   the unit's dep-info in its own output directory (the `.d`'s own rule first,
   `.rmeta`/`.rlib`/proc-macro/built-script shapes), requiring the artifact
   target and sources entirely inside the verified source root; the join
   recorded at producing time and re-verified under the pin; and no offering
   when the join is unprovable?
7. Is the §6.3/§7 sequencing accepted as the brief's admission barrier: one
   admission lifetime (recipient record → sibling barrier → admission) across
   call 1 (pool lock alone, **catalog bytes only, no donor payload**), the
   effect-free selection from the recorded join and the reserve gate, then call
   2 (donor record non-blocking → pool lock) where **every donor file is read
   and hashed** and held through the final donor revalidation, with a competing
   `managed::lease` acquire/eviction between the calls aborting the seed and
   the pool hold bounded in work rather than wall time?
8. Is the §4.3 archive-to-tree rule sound — cached `.crate` bytes against the
   lock checksum, a canonical derivation with strict entry grammar, a cache hit
   bound to the archive's recorded device/inode/size/mtime identity — and is the
   pinned `flate2` + `tar` reader acceptable as a new locked dependency?
9. Is the complete per-role manifest in §5.1, including the required `.d` join,
   accepted as the completeness and association boundary?
10. Is recording per-file producing-epoch SHA-256 digests (§4.4) with a
   256 MiB / 4 s cap the right stale-content evidence, and is abandoning the
   whole seed on one mismatch the intended fail-closed behaviour?
11. Is the durable per-generation seed-attempt record (§4.1, §9.1) accepted as
   the unseeded fact, including an older binary reusing a slot?
12. Is the §8.3 reserve treatment accepted: the gate sampled only after the
   effect-free selection produced a non-empty plan and before the durable
   markers, a 22 GiB start gate and monitored stop that refuses rather than
   cold-builds, the removable volume on its own removal-only watchdog and
   registration reserve, and a no-plan command keeping today's behaviour (no
   sample, no marker)?
13. Is the §9.2 bounded skip-count reporting accepted as satisfying binding
   `bb151a59`'s ambiguous-name visibility for both the summary and `oxide
   status`, without adding a walk or reconciliation?
14. Do the phased §16.4 figures (Phase B peak about 290-445 MiB of 512 MiB, and
   upper time estimates summing to 271 s of the earlier 300 s (v1 is now 600 s) with the required SQLite
   and offered-run cases retained) leave enough room for an unmeasured fit
   whose diagnostic is still mandatory, or should the conflict go to the
   Supervisor before freeze?
15. Is the D-4 interface in §11 the intended reading — seeding cannot help
    while the only donor is the session's held slot — or should a scoped
    follow-up consider a read-only held-slot donor under stricter closure?

## 18. Limits of this design

- It establishes no native evidence, no selected-volume fit, no timing, no
  reach, no saving and no benefit. The separate measured real-Effigy
  seeded-versus-unseeded comparison stays non-gating and separately scoped.
- Its eligible set is the leaf dependency set. That is narrower than the
  approved scope's wording about dependency libraries and proc macros, and the
  narrowing is returned to the Supervisor rather than decided here.
- It does not implement the registry archive reader, the dep-info rule parser
  or the configuration walker; it names the dependency, the strictness and the
  skip rules the implementation must prove.
- Selection trusts the join record the producing epoch wrote; a producing epoch
  whose `.d` shapes differ from the pinned adapter writes no catalog, so the
  cost of an adapter mistake is an inert donor, never a wrong clone.
- Supported target names are Cargo's normalized inferred names only; a package
  that genuinely renames its lib target is ineligible in v1 rather than mapped
  by a second grammar. The literal package name and the normalized target are
  separate identities (§4.2); the adapter does not assume every hash suffix
  corresponds to both.
- Skip counts are donor-side catalog facts. They describe what the producing
  epoch rejected while it held the lease; they are not a recipient-side
  allowlist measurement and they never imply donor reach.
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
