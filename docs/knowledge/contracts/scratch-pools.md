# Scratch build pools on an external validation volume (proposed design)

**Status: proposed design, design-only.** This contract records a design for an
explicit, UUID-bound `directory-budget-v1` "scratch pool" on a verified external
volume, so validation and proof fixtures can keep their scratch on the Builds
drive instead of declaring the internal-root exception (decision `bd357a4c`) for
`/private/tmp`. Nothing here is implemented: no code, selector, recipe, host
setting, real pool or native run exists, and the internal-root exception stays
current truth until each slice below lands and passes its own review. Oxide#173.

Owning neighbours: [workflow](workflow.md#temporary-scratch-placement) owns
proof and validation scratch placement and the current exception;
[workflow](workflow.md#budget-only-default-pool-and-user-configuration) owns pool
selection, budget and the external-volume refusal;
[workflow](workflow.md#removable-build-volume) owns the removable pool, which
this design leaves alone; [local-lease](local-lease.md) owns the pool's ledger,
eviction and closure rules, also unchanged.

Tom, 2026-10-28, on barge#017's internal-pool exception: "this needs fixing so
we can keep it on Builds next time".

## Problem

`config::refuse_unsafe_pool_path` refuses a pool path whose nearest existing
ancestor canonicalizes under `/Volumes` (or whose mount root does), for good
reason: a user's default `directory-budget-v1` pool must not silently live on a
removable volume, where it disappears with the drive. The refusal is not
bendable (there is no debug-only bypass), so every proof whose subject *is* a
genuine `directory-budget-v1` pool has had to place that pool on the internal
SSD and declare `--root-parent /private/tmp` in its selector.

The pool backend is what those proofs exercise, not internal placement. This
design keeps the protection for real pools and admits a *different, explicitly
declared* pool class for scratch use only.

## Scope and non-goals

- Product change: one new path-admission branch with an explicit ordering, one
  environment setting, one pool-local binding document and one host-side
  identity record. No new backend, ledger, config or registry format.
- Not changed: the ordinary external-volume refusal message, the Git-work-tree
  and traversal refusals, the built-in default pool and its location, the soft
  budget, the host-reserve watchdog, Time Machine exclusion, eviction, writer
  closure, child temp placement, removable registration, drive mode, permanent
  outputs, `status` walk rules and every frozen serialized format.
- Not authorised: any real pool or drive change, any host setting, any install,
  any consumer. `bd357a4c` remains in force for proofs that genuinely need the
  boot Data volume premise, and for every proof until its own migration slice.

## The opt-in

One environment variable, `OXIDE_SCRATCH_ROOT` (macOS and Linux alike; no config
key, no `[pool]` field, and no setting a later invocation inherits — the host
identity record below is proof of placement, not configuration).

- It names **one** absolute, canonical, real directory: the declared scratch
  root. It is the declaration *and* the opt-in; there is no separate boolean, so
  there is no "flag without root" or "root without flag" state, and an empty or
  relative value is a refusal, never "off".
- It is read only when the ordinary admission has already refused for the
  **external-volume** reason. It cannot widen, soften or reorder any other
  refusal, and it is never consulted for a path on the boot volume (where
  ordinary admission already succeeds). This is why one fixture recipe works on
  both placements: on the supervisor's `/private/tmp` fallback the declaration is
  inert, on the Builds drive it admits the pool.
- Environment only, deliberately: a config-file declaration would persist and be
  inherited by an ordinary build after one experiment. The environment form is
  per-invocation and is what every consumer (proof scripts, Barge validation)
  needs. `OXIDE_POOL` and `OXIDE_POOL_CAP` set the same precedent.
- The pool must still be *selected explicitly*: `OXIDE_POOL` or the user config
  file's `[pool] path`. A built-in default pool (`Source::Default`) is refused
  under this route (`scratch-declaration-default-pool`), so no home or XDG change
  can move the default pool onto an external volume through it.
- A scratch pool must declare its **budget** explicitly (`OXIDE_POOL_CAP` or
  `[pool] budget`); the built-in 100 GB default refuses
  (`scratch-budget-required`). A scratch pool never gets the large default by
  accident.

## Placement rules

1. The declared root must be an absolute, canonical (no symlink component),
   real directory owned by this user, on a non-boot volume, with no group or
   other **write** bit, and it must not be a mount root itself.
2. The pool path must be **strictly inside** the root: the root itself, a path
   that escapes it, or a path that only shares a name prefix refuses.
3. The pool's parent must already exist inside the root; Oxide creates only the
   pool leaf (0700) and never a missing ancestor under an external root.
4. The root's volume must be a genuine mounted external volume: mount root
   directly under `/Volumes` with a one-component name, `f_mntonname` equal to
   that mount root (a stale `/Volumes/<name>` directory is never accepted),
   APFS, not the boot volume, not read-only, with a readable `ATTR_VOL_UUID`.
   `/Volumes` itself must pass `judge_parent`: root-owned, no group or other
   write bit, no extended ACL entries, no flags beyond `UF_HIDDEN`, and the
   caller must not be root.
5. The volume is verified by the removable backend's own primitive,
   `volume::Binding::open(mount_root, Some(uuid))` — `mount_name`, the
   `/Volumes` parent rule, a held descriptor of the mount root, `judge`, the
   parent's-device inequality and a `verify()` recheck. No new probe, no
   `diskutil` call, no shell-out. This is the same verification registration
   performs, so "a registered or verified volume" is satisfied by verification
   alone and no registration needs to exist.

### Admission order

Today `refuse_unsafe_pool_path` checks the external volume **before** the Git
work tree, so an external path inside a work tree reports the external reason.
The scratch branch must not inherit that as a hole:

1. `.`/`..` and the nearest existing ancestor, unchanged and first.
2. The external-volume judgement, unchanged. Only the scratch branch may lift
   **this** refusal, and only for a declared, verified path.
3. When the branch lifts it, the Git-work-tree check (today's
   `canonical.ancestors()` walk for `.git`) **runs and must pass** on the same
   canonical ancestor. A work tree then refuses with today's Git message
   (`git-work-tree`) and creates nothing. With no declaration the external
   message still wins, exactly as today, so rows 1–3 of the matrix keep
   byte-identical output.
4. The mount-root judgement is then replaced by the stricter `Binding`
   verification below; a path on some other mounted volume that is not under
   `/Volumes` still refuses (`scratch-volume-unverified`).
5. Only after those pass are the root, binding and record verified, and only
   then does the store open or create anything.

On the Builds drive macOS ignores ownership (`MNT_IGNORE_OWNERSHIP`), so the
UID/mode rules are consistency and tamper guards for a cooperative same-user
host, exactly as [workflow](workflow.md#temporary-scratch-placement) already
records for the scratch subtree. No behaviour is built on ownership enforcement.

## Volume identity binding

A scratch pool is bound at creation and re-verified on every open through **two**
records, because one of them must remain readable when the volume is gone:

- **On-volume binding.** On first use the creating command makes the pool leaf
  exclusively (0700), then writes `<pool>/oxide-managed/scratch.json` atomically
  (0600, private temp plus one replace) **before** the library initialises the
  pool payload. Schema `oxide.scratch-pool/1`, strict (`deny_unknown_fields`),
  16 KiB bound, containing exactly: `schema`, `volume_uuid`, `mount_point`
  (canonical), `scratch_root` (canonical), `pool` (canonical), `device`
  (`st_dev`), `owner` (uid) and `created_ms`. It proves the volume's own bytes
  agree with the placement; the frozen pool `config.json`, ledger and control
  files are untouched.
- **Host identity record.** The same command then writes
  `<account state root>/scratch-pools/<sha256 of the normalised pool path>.json`
  (state directory 0700, file 0600, atomic replace, 4 KiB bound, strict schema
  `oxide.scratch-pool-record/1` with `schema`, `pool` (normalised absolute path),
  `volume_uuid`, `mount_point`, `scratch_root` and `created_ms`). This is the
  record that survives detach, so the absent-volume refusal and `status` can
  name the volume. It is written for **every** scratch pool regardless of
  selection source — the known-pools list (`pools.json`) keeps its current rule
  and still does not remember an `OXIDE_POOL` pool; this record is identity
  metadata, not a pool list. It is keyed by the **normalised** absolute pool
  path, never re-canonicalised, because the volume and the whole path may be
  absent; at creation `canonicalize` of the existing ancestor must equal the
  normalised ancestor, so the key is the canonical path then.
- **Record ordering and repair.** The leaf, the on-volume binding, the host
  record and only then the pool init are the write order. A failure writing the
  host record removes only the leaf this invocation just created and refuses
  (`scratch-record-unwritable`). A later **leased** open with a present volume
  and a matching on-volume binding but no host record writes that record once
  (bounded, atomic) and proceeds; `status` never repairs and never writes, so a
  pool whose record is missing still reports as a scratch pool without a UUID.
- **Re-verified.** Every later **leased** open (`check`, `build`, `test`,
  `run`, `exec`, `clean`, recovery) requires all of: the declaration names the
  recorded root; the recorded mount point still binds through
  `Binding::open(mount_point, Some(volume_uuid))`; the pool path is still
  strictly inside both `mount_point` and `scratch_root`; the on-volume binding
  is a real regular file of this user, not a symlink, not oversized, of the
  known schema; and — when a host record is present — it agrees with the
  on-volume binding on UUID, mount point, root and pool. Any difference refuses
  (`scratch-record-mismatch`, `scratch-binding-invalid`); nothing is repaired,
  adopted or rewritten. So an edited host record cannot retarget a pool and a
  copied pool cannot inherit a record. A **read-only** `status` / `--measure`
  looks the pool up in the host record **before** the path judgement, applies
  the same verification when the volume is present, needs no declaration, and
  never leases, creates or writes. Verification runs immediately before the
  open. The design claims volume identity and placement, not TOCTOU-freedom:
  the pool's own I/O keeps the existing path-based rules, and S3 may create the
  leaf through `Binding::dir_at` as hardening, not as a promise.
- **Absent volume.** When the mount point is gone, or the nearest existing
  ancestor of the pool path is on another device (including `/Volumes` on the
  boot disk after an ordinary detach), a leased open refuses before any create,
  walk, eviction, lock or write. With a host record: `the scratch pool at <path>
  is bound to volume <uuid> at <mount>; that volume is absent`. Without one:
  `the scratch pool's volume is absent and no host record matches <path>;
  nothing was created`. A read-only `status` / `--measure` does not refuse: it
  reports `unavailable` (row 24) and leaves the exit status unchanged. There is
  no internal fallback, no shadow directory at the absent path and no recreated
  mount point.
- **Incomplete.** A leaf holding a valid binding but no pool `config.json` is a
  prepared, uninitialised scratch pool: the creating invocation completes it, a
  later invocation refuses (`scratch-incomplete`) and names the leaf. A pool
  root at a scratch path with no binding is not a scratch pool: with the
  declaration present it refuses as `scratch-binding-missing` and is never
  adopted, initialised or chmodded; without one it takes the ordinary external
  refusal.
- **Corroboration, not dependency.** A registered removable volume passes these
  checks unchanged, so a scratch pool on it verifies identically, and a scratch
  pool may coexist with the removable payload there: the two pools are separate
  (`<pool>/payload/slots` versus the recorded payload directory), and neither
  selects or evicts the other. The design reads no registration document, needs
  no internal pool to exist, and does not depend on Barge's stage-4 volume
  identity: when that identity document exists it can be compared as an extra
  read-only signal in a later slice, never as a precondition.

## Refusal matrix

Every row refuses before any create, write, lock, walk or child. Each row names
its refusal class and has one unit case; rows 6–8 and 11–12 deliberately share
a class.

| # | Condition | Class |
| --- | --- | --- |
| 1 | Path has `.`/`..`, or no existing ancestor | unchanged traversal/ancestor refusal |
| 2 | Internal path inside a Git work tree | unchanged `git-work-tree` refusal (declaration never consulted) |
| 3 | On an external volume, no `OXIDE_SCRATCH_ROOT` | unchanged `external-volume` refusal, naming `oxide pool register-removable` |
| 4 | External path inside a Git work tree, declaration present | `git-work-tree`: the branch lifts only the external refusal, then the Git check refuses |
| 5 | External, declaration present, pool source is the built-in default | `scratch-declaration-default-pool` |
| 6 | Declared root not absolute/canonical/real/symlink/other-device | `scratch-root-unsafe` |
| 7 | Declared root not owned by this user, or group/other writable | `scratch-root-unsafe` |
| 8 | Declared root is a mount root, or its volume is the boot volume | `scratch-root-unsafe` |
| 9 | Pool path equals the root, escapes it, or is a prefix-lookalike | `scratch-path-outside-root` |
| 10 | Pool parent missing under the root | `scratch-parent-missing` |
| 11 | Volume not a mount root / stale directory / not APFS / boot / read-only / no UUID / another mounted volume | `scratch-volume-unverified` |
| 12 | Mount name not one component under `/Volumes`, or `/Volumes` unsafe | `scratch-volume-unverified` |
| 13 | Existing pool at a scratch path without `scratch.json` | `scratch-binding-missing` |
| 14 | Binding unreadable, symlinked, foreign-owned, oversized, malformed, unknown schema, wrong key | `scratch-binding-invalid` |
| 15 | Binding UUID, mount point, root, device or owner differs from the observed facts | `scratch-binding-mismatch` |
| 16 | Host record unreadable, symlinked, foreign-owned, oversized, malformed, unknown schema, wrong pool key | `scratch-record-invalid` |
| 17 | Host record disagrees with the on-volume binding or the observed facts | `scratch-record-mismatch` |
| 18 | Host record cannot be written at creation | `scratch-record-unwritable` (removes only the leaf this invocation just created) |
| 19 | Bound volume or mount absent; ancestor on another device | `scratch-volume-absent`, naming the UUID when a host record exists |
| 20 | Prepared leaf without pool config on a later invocation | `scratch-incomplete` |
| 21 | Scratch path with the built-in budget default | `scratch-budget-required` |
| 22 | `OXIDE_SCRATCH_ROOT` empty, relative, with `..`, or unreadable | `scratch-root-unsafe` |
| 23 | Leased command on an existing scratch pool without the declaration | unchanged `external-volume` refusal (#3); the mode never persists; read-only `status`/`--measure` identifies it from the host record instead |
| 24 | `status` of a recorded scratch pool whose volume is absent | `unavailable (scratch volume <uuid> absent)` from the host record, no walk, no write, exit unchanged |

Rows 1–3 and 23 keep today's exact messages; row 5 refuses the declaration
rather than the path. Row 24 is a report, not a new status failure: `status`
already reads known pools and registrations read-only.

## Default-pool protection (why it is not weakened)

1. The built-in default pool is refused under the scratch route outright (row
   5). Even when the account home itself sits on an external volume — where the
   default path does reach that refusal — the declaration is refused, so the
   route can never turn the default pool into a scratch pool.
2. Every existing refusal keeps its message, and no refusal is reordered for an
   ordinary path. The scratch branch is the one test that may lift the
   external-volume refusal, and it is immediately followed by the Git-work-tree
   check ("Admission order"), so an admitted path has passed every other
   refusal — the Git check is not skipped just because it used to sit behind the
   external one.
3. The route needs four independent facts at once: an explicit per-invocation
   declaration, an explicit pool selection, a pool strictly inside that declared
   root, and a verified non-boot APFS volume with matching on-volume binding and
   host record. No single input enables it.
4. The accepted class is visibly different: pool creation prints one bounded
   notice (`creating a scratch build pool on volume …; not for ordinary project
   builds`), and `status` labels it `pool: <path> (scratch pool, volume <uuid>)`.
5. A scratch pool gains no new authority: same ledger, eviction, held-slot
   exclusion, unknown-writer conservatism, watchdog, closure and accounting
   rules. It is not the removable pool, is never selected by drive mode, has no
   flush or quarantine semantics, and permanent outputs are unchanged.
6. Residual risk, stated rather than hidden: an operator who sets
   `OXIDE_SCRATCH_ROOT` and points `OXIDE_POOL` into it gets a real-looking
   directory pool on an external volume. That is the opt-in. It is
   per-invocation, cannot be inherited from a config file, is announced on
   creation and in `status`, and cannot touch the default pool.

## Status, selection and diagnostics

`oxide status` gains one bounded text label and no new walk, derived from the
host identity record only, so it works with the volume absent and never writes:
a scratch pool prints `pool: <path> (scratch pool, volume <uuid>)`; a recorded
scratch pool whose volume is absent prints `pool: <path> unavailable
(scratch volume <uuid> absent)` and still walks nothing; `status --measure`
keeps its existing walk rules and still writes nothing. With **no** host record
`status` has no scratch fact and reports exactly as today (the ordinary
external refusal for a known pool), and it never invents one: a leased open
repairs a missing record. No JSON schema changes: `--json` and
`pool describe --json` keep their current keys and add no scratch object.
`remember_pool` and the known-pools list keep their current rule (an
`OXIDE_POOL` pool is still not listed); the scratch host record above is a
separate identity document, not that list. Refusals print the class in words,
never a raw path dump, bounded and escaped as today.

## Migration list

`bd357a4c` is only about `--root-parent /private/tmp`. Pool premise is read from
each script's own selector text and case bodies.

**Can drop the explicit internal root** (the premise is a
`directory-budget-v1` pool, not internal placement):

| Selector(s) | Premise today |
| --- | --- |
| `rust:installed-handoff` (#122) | two cases create a real directory pool |
| `rust:profile-outputs` (#133) + `:diagnostic` | 20 private directory-budget fixture pools |
| `rust:pool-temp-scratch` (#136) + `:diagnostic` | directory-budget pool plus one owned image; no boot-Data premise |
| `rust:recorded-slot-accounting` (#146) + `:diagnostic` | genuine directory-budget pool, verified from the product's own config |
| `rust:exec-output-inference` (#160) + `:diagnostic` | control pool is a directory-budget pool |
| `rust:auto-adopt-ineligible` (#154) + `:diagnostic` | control pool is a directory-budget pool |
| `rust:prune-authority` (#134) + `:diagnostic` | internal pool placement only |
| `rust:oxide-removable-boot-closure` (#161) + `:diagnostic` | directory-budget base pool plus one owned image |
| `rust:finder-metadata-race` (#155) + `:diagnostic` | every fixture pool is a directory-budget pool |
| `rust:command-budgets` (#140) + `:diagnostic` | `OXIDE_POOL_BACKEND=directory-budget` fixture pools |

**Keep the internal root** (the premise is the boot Data volume or
non-`/Volumes` placement, which no scratch pool replaces):

| Selector/recipe | Genuine premise |
| --- | --- |
| `rust:outputs-prune` (#129/#152) + `:diagnostic` | cases 10 and 21 prove checkout absence on the boot Data volume |
| `rust:prune-absence` (#135) + `:diagnostic` | the missing-ancestor chain must sit on the boot Data volume |
| the `AGENTS.md` unit-suite recipe (`OXIDE_TEST_INTERNAL_ROOT`) | three macOS fixtures take the boot Data volume and `MNT_IGNORE_OWNERSHIP` premises; `config::tests::git_work_trees_and_aliases_refuse` needs a root off `/Volumes` |

Not in scope: proofs that never carried `bd357a4c` but assert their own
`/private/tmp` scratch (`rust:oxide-exec`, `rust:front-door-accounting`,
`rust:pool-scale`, `rust:accounting-vanished`, `rust:accounting-guards`,
`rust:status-fast`, `rust:oxide-finder-metadata`, `rust:oxide-pool-layout`).
They migrate, if at all, in their own briefs with their own activations.

## Recipe changes (design, not applied)

The supervisor needs no change: with no caller root it already puts its owned
root in `<mount>/scratch/oxide` of the selected volume, and falls back to
`/private/tmp` when none is usable. For each migrated proof:

1. The selector drops `--root-parent /private/tmp`; its admission, its wall
   clock, its own monitored scratch budget (512 MiB for most, 64 MiB for
   `rust:installed-handoff`, 600 s for `rust:profile-outputs`, 420 s for
   `rust:finder-metadata-race`) and cleanup are unchanged, and each migrated
   selector still states its pool premise in its own comment.
2. The script sets `OXIDE_SCRATCH_ROOT` to one private directory inside its own
   `OXIDE_TASK_SCRATCH_ROOT` run root (created 0700, canonical, same device) and
   keeps `OXIDE_POOL` strictly inside it, with an explicit `OXIDE_POOL_CAP`.
   No absolute mount path, no second scratch root, nothing outside the monitored
   root. The host identity record lands in that fixture's own private state
   root, so a changed private `HOME` is a changed record home (see "Limits of
   this design"); no real account state is touched.
3. Where the script has a pure `proof:*:test` self-check, it pins that
   environment contract, so a script left half-migrated fails an effect-free
   check instead of running. Where it has none, the `--plan` or dogfood check
   named in S4 is the effect-free pin instead.
4. Each migrated proof states in its own text that a fallback run to
   `/private/tmp` exercises the internal path where the declaration is inert;
   that is not scratch-pool coverage and is reported as such.
5. Already-passed evidence stays bound to its activated recipe. A migrated
   selector needs a short independent recipe review, not a re-run of a previously
   passed native; every HELD proof keeps its own separate owner activation.
6. `AGENTS.md` and [workflow](workflow.md#temporary-scratch-placement) are
   amended in the same slice: the exception paragraph is scoped to the keep
   list, and the mirrored selector text loses its exception note as each proof
   migrates.

## Implementation slices

Each slice is its own Queue brief with independent exact-head review; none is
authorised by this document. "Wall" is the selector's own limit; "scratch" the
monitored scratch budget.

**S1 — typed path judgement and the pure scratch admission. STANDARD.**
Split `refuse_unsafe_pool_path` into a typed judgement (`Traversal`,
`Ancestor`, `ExternalVolume`, `GitWorkTree`) evaluated in the documented order
with the messages unchanged, plus one pure function that decides the scratch
branch over injected facts (declaration, pool selection source, root facts,
mount facts, on-volume binding, host record). The function returns the lifted
external refusal **only** when the Git-work-tree recheck on the same canonical
ancestor also passes. No behaviour change yet: the branch is unreachable until
S2 wires it. Tests: one unit case per matrix row (1–24, with the record rows
against injected documents), byte-equality of rows 1–3 and 23, row 4 (Git
refusal produced after the lift), the two-record agreement and mismatch rules,
the absent-volume refusal with and without a host record, and unchanged ledger,
config and registry formats. Selectors: `effigy rust:compile`,
`effigy rust:format`, `effigy test rust` (the AGENTS.md unit recipe), and
`effigy docs:check` when docs change. 300 s, 512 MiB.

**S2 — the opt-in setting. STANDARD.** Read `OXIDE_SCRATCH_ROOT`, apply the
scope rules (default-pool refusal, budget requirement, environment-only) and
wire the S1 branch into the four `store.rs` call sites, including the
`status`/`open_recorded` path. Read the host record for the `status` label and
the absent-volume refusal; never write from `status`. Tests: precedence and
inertness on internal paths, rows 5, 16, 17, 21–24, `status` zero-write and
zero-walk regression (including an absent volume with and without a record), and
one CLI case on a private internal fixture showing the declaration changes
nothing there. Selectors: as S1. 300 s, 512 MiB.

**S3 — creation, binding, record and open on a verified volume. ELEVATED.**
This slice enables a `directory-budget-v1` pool on removable media, so the
actual trigger is: its admission walk decides whole-slot deletion on a volume
outside the boot disk. Write and verify the on-volume binding and the host
identity record in the documented order, refuse the absent, mismatched, invalid
and incomplete states, repair a missing record only on a leased open, look the
record up before the path judgement on the read-only path, and label `status`.

Covering product checks for the brief: `effigy rust:compile`,
`effigy rust:format`, `effigy test rust`, and `effigy docs:check` (this slice
changes workflow and AGENTS text). Native checks: a new pure selector
`effigy proof:scratch-pool:test` (`--self-check`, creates nothing); new heavy
selectors `effigy rust:scratch-pool:diagnostic` (uncounted, required first, HIGH
premise checklist) and `effigy rust:scratch-pool` (counted author native), both
one fixed script and the whole command of the qualified shared supervisor
(`bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
scripts/test-scratch-pool.py`), 300 s, 512 MiB, one owned 128 MiB APFS image
attached with `hdiutil -owners on` and ordinarily detached, private `HOME`,
`CARGO_HOME` and config. **ELEVATED also requires one distinct independent
reviewer native `effigy rust:scratch-pool` at the exact head**, in addition to
the reviewer's exact-head source review of the author's evidence; the author's
native and the reviewer's are separate owner activations.

Cases: (01) first leased command on an *unregistered* scratch volume creates the
leaf, both records and the pool, and builds; (02) warm reuse from the same
declaration; (03) the same pool without the declaration takes the ordinary
external refusal, with binding, record and pool bytes unchanged; (04) pool path
equal to the root, a sibling-prefix path and a missing parent each refuse with
nothing created, and an external path inside a Git work tree refuses with the
Git message while an internal one keeps today's; (05) no explicit budget
refuses; (06) a declared root that is a symlink, group-writable, or a mount root
refuses, while a declared root on the boot volume is inert and the same recipe
builds an ordinary internal pool; (07) ordinary detach: `status` reports
`unavailable (scratch volume <uuid> absent)` with no walk and no write, a leased
command refuses naming the same UUID from the host record, no shadow directory
appears and the mount point is not recreated; (08) re-attach the same image:
same UUID, warm reuse; (09) a *different* image at the same mount name refuses
by UUID; (10) a copied pool (binding edited to another UUID, then the original
bytes restored) refuses and the pool is untouched; (11) a regular directory left
at the mount name (a stale mount point) is not a mounted volume root and refuses
as unverified; (12) the host record removed while the volume is present: `status` writes
nothing and reports exactly as today (no scratch fact), and a leased open
repairs the record once, after which `status` shows the UUID label; (13) the
host record edited to another UUID refuses as a mismatch with both records and
the pool bytes unchanged; (14) an
unreadable, malformed, symlinked or oversized host record refuses as invalid;
(15) detach with no host record refuses without a UUID and still creates
nothing. Each case reports its own oracle; the diagnostic records
reached/completed/failed/skipped and the latched first fault in a bounded
`qualified: false` receipt.

**S4 — recipe migration. STANDARD.** The ten drops and three keeps above, the
per-script `OXIDE_SCRATCH_ROOT` wiring, the pure self-check pins, and the
`AGENTS.md` / `workflow.md` amendments. Each selector keeps **its own** wall
clock and scratch budget; there is no uniform figure:

| Selector | Limit / scratch | Pure check available |
| --- | --- | --- |
| `rust:installed-handoff` | 120 s / 64 MiB | none; `scripts/test_dogfood_scripts.py` `HandoffSelectorTests` pins its root, limits and body and must change with it |
| `rust:profile-outputs` (+ diag) | 600 s / 512 MiB | none; `--plan` only |
| `rust:pool-temp-scratch` (+ diag) | 300 s / 512 MiB | `proof:pool-temp:test` |
| `rust:recorded-slot-accounting` (+ diag) | 300 s / 512 MiB | `proof:recorded-slot-accounting:test` |
| `rust:exec-output-inference` (+ diag) | 300 s / 512 MiB | `proof:exec-inference:test` |
| `rust:auto-adopt-ineligible` (+ diag) | 300 s / 512 MiB | none; `--plan` only (shared #117 fixture) |
| `rust:prune-authority` (+ diag) | 300 s / 512 MiB | `proof:prune-authority:test` |
| `rust:oxide-removable-boot-closure` (+ diag) | 300 s / 512 MiB | `rust:oxide-removable-boot-closure:test` |
| `rust:finder-metadata-race` (+ diag) | 420 s / 512 MiB | `rust:finder-metadata-race:test` |
| `rust:command-budgets` (+ diag) | 300 s / 512 MiB | `proof:command-budgets:test` |

Where a pure check exists, the slice runs it and pins the new environment
contract there. Where none exists (`installed-handoff`, `profile-outputs`,
`auto-adopt-ineligible`) the slice does **not** invent one and does **not**
re-run a native: it relies on that proof's effect-free `--plan` (or the dogfood
selector test, for `installed-handoff`, which is updated in the same commit),
plus the short independent recipe review. Selectors: `effigy docs:check`, the
pure checks in the table, `effigy dogfood:scripts:test`, and the recipe review.
No new activation, and no re-run of a previously passed native. 300 s, no
scratch (pure and plan checks only).

**S5 — record the retirement (optional, docs-only). STANDARD.** Amend the
`bd357a4c` paragraph to the keep list and, if the owner retires the exception
itself, add the `retired.toml` entry in the same change. `effigy docs:check`.

## Limits of this design

- Unmeasured: no case has run, so the native proof's fit, its image usage and
  its wall clock are planning figures only.
- A scratch pool inside an owned run root is deleted with that root; a pool
  kept outside a run root is the harness owner's retention problem, not a
  product policy, and this design sets none.
- The binding proves volume identity and placement, not that the pool's bytes
  are disposable: eviction and the soft budget still govern what may be deleted.
- Linux has no `/Volumes` mount facts in this product and `volume::Binding::open`
  is macOS-only, so the scratch branch refuses there instead of admitting
  anything. That is a unit-tested refusal, not a portability claim, and the
  internal-root recipes stay the only Linux story.
- The host identity record belongs to the invoking account state root. A
  different state root (a private-`HOME` fixture, a moved state directory) sees
  no record: a present volume is still admitted from the on-volume binding and
  the record is repaired on the next leased open, but an absent volume is then
  refused without a UUID and `status` has no scratch line. A crash between the
  two creation writes leaves the same state until a leased open repairs it.
  That degradation is reported, never hidden.

## Review gate map

- `default_pool_protection_unweakened` — "Default-pool protection", "Admission
  order", refusal matrix rows 1–5 and 23.
- `explicit_opt_in_and_volume_binding` — "The opt-in", "Placement rules",
  "Volume identity binding" (both records), matrix rows 13–19.
- `refusal_matrix_and_absent_volume` — the 24-row "Refusal matrix" (rows 4,
  16–19 and 24 in particular) and S3 cases 07 and 12–15.
- `migration_list_and_internal_premise_cases` — "Migration list" and
  "Recipe changes".
- `finite_slices_and_validation_plan` — "Implementation slices" (per-slice
  tiers, selectors, wall and scratch budgets, the S4 per-selector table, and
  S3's covering product checks plus the independent elevated reviewer native)
  and "Limits of this design".
