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

- Product change: one new path-admission branch, one environment setting, one
  pool-local binding document. No new backend, ledger, config or registry
  format.
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
key, no `[pool]` field, no persisted setting).

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

On the Builds drive macOS ignores ownership (`MNT_IGNORE_OWNERSHIP`), so the
UID/mode rules are consistency and tamper guards for a cooperative same-user
host, exactly as [workflow](workflow.md#temporary-scratch-placement) already
records for the scratch subtree. No behaviour is built on ownership enforcement.

## Volume identity binding

A scratch pool is bound at creation and re-verified on every open, so the pool
cannot drift to another volume or be copied:

- **Recorded.** On first use the creating command makes the pool leaf
  exclusively (0700), then writes `<pool>/oxide-managed/scratch.json` atomically
  (0600, private temp plus one replace) **before** the library initialises the
  pool payload. Schema `oxide.scratch-pool/1`, strict (`deny_unknown_fields`),
  16 KiB bound, containing exactly: `schema`, `volume_uuid`, `mount_point`
  (canonical), `scratch_root` (canonical), `pool` (canonical), `device`
  (`st_dev`), `owner` (uid) and `created_ms`. The frozen pool `config.json`,
  ledger and control files are untouched; this is the only new document.
- **Re-verified.** Every later open (leased command, `status`, `--measure`,
  recovery) requires all of: the recorded mount point still binds through
  `Binding::open(mount_point, Some(volume_uuid))`, the pool path is still
  strictly inside both `mount_point` and `scratch_root`, the declaration names
  that same root, and the document is a real regular file of this user, not a
  symlink, not oversized, of the known schema. Any difference refuses; nothing
  is repaired, adopted or rewritten. Verification runs immediately before the
  open. The design claims volume identity and placement, not TOCTOU-freedom:
  the pool's own I/O keeps the existing path-based rules, and S3 may create
  the leaf through `Binding::dir_at` as hardening, not as a promise.
- **Absent volume.** If the mount point is gone, or the nearest existing
  ancestor of the pool path is on another volume (including `/Volumes` on the
  boot disk after an ordinary detach), the open refuses before any create,
  walk, eviction, lock or write: `the scratch pool at <path> is bound to volume
  <uuid> at <mount>; that volume is absent`. There is no internal fallback, no
  shadow directory at the absent path and no recreated mount point.
- **Incomplete.** A leaf holding a valid binding but no pool `config.json` is a
  prepared, uninitialised scratch pool: the creating invocation completes it, a
  later invocation refuses (`scratch-incomplete`) and names the leaf. A pool
  root at a scratch path with no binding is not a scratch pool and takes the
  ordinary external refusal (`scratch-binding-missing`).
- **Corroboration, not dependency.** A registered removable volume passes these
  checks unchanged, so a scratch pool on it verifies identically, and a scratch
  pool may coexist with the removable payload there: the two pools are separate
  (`<pool>/payload/slots` versus the recorded payload directory), and neither
  selects or evicts the other. The design reads no registration document, needs
  no internal pool to exist, and does not depend on Barge's stage-4 volume
  identity: when that identity document exists it can be compared as an extra
  read-only signal in a later slice, never as a precondition.

## Refusal matrix

Every row refuses before any create, write, lock, walk or child, and every row
has a distinct internal class name so unit tests map one-to-one.

| # | Condition | Class |
| --- | --- | --- |
| 1 | Path has `.`/`..`, or no existing ancestor | unchanged traversal/ancestor refusal |
| 2 | Inside a Git work tree | unchanged `git-work-tree` refusal (declaration never consulted) |
| 3 | On an external volume, no `OXIDE_SCRATCH_ROOT` | unchanged `external-volume` refusal, naming `oxide pool register-removable` |
| 4 | External, declaration present, pool source is the built-in default | `scratch-declaration-default-pool` |
| 5 | Declared root not absolute/canonical/real/symlink/other-device | `scratch-root-unsafe` |
| 6 | Declared root not owned by this user, or group/other writable | `scratch-root-unsafe` |
| 7 | Declared root is a mount root, or its volume is the boot volume | `scratch-root-unsafe` |
| 8 | Pool path equals the root, escapes it, or is a prefix-lookalike | `scratch-path-outside-root` |
| 9 | Pool parent missing under the root | `scratch-parent-missing` |
| 10 | Volume not a mount root / stale directory / not APFS / boot / read-only / no UUID | `scratch-volume-unverified` |
| 11 | Mount name not one component under `/Volumes`, or `/Volumes` unsafe | `scratch-volume-unverified` |
| 12 | Existing pool at a scratch path without `scratch.json` | `scratch-binding-missing` |
| 13 | Binding unreadable, symlinked, foreign-owned, oversized, malformed, unknown schema, wrong key | `scratch-binding-invalid` |
| 14 | Binding UUID, mount point, root, device or owner differs from the observed facts | `scratch-binding-mismatch` |
| 15 | Bound volume or mount absent; ancestor on another volume | `scratch-volume-absent` |
| 16 | Prepared leaf without pool config on a later invocation | `scratch-incomplete` |
| 17 | Scratch path with the built-in budget default | `scratch-budget-required` |
| 18 | `OXIDE_SCRATCH_ROOT` empty, relative, with `..`, or unreadable | `scratch-root-unsafe` |
| 19 | Existing scratch pool opened without the declaration | unchanged `external-volume` refusal (#3); the mode never persists |
| 20 | `status` of a recorded scratch pool whose volume is absent | `unavailable (<reason>)` line, no walk, no write, exit unchanged |

Rows 1, 2, 3 and 19 keep today's exact messages. Row 20 is a report, not a new
status failure: `status` already reads known pools and registrations read-only.

## Default-pool protection (why it is not weakened)

1. The built-in default pool is refused under the scratch route outright (row
   4). Even when the account home itself sits on an external volume — where the
   default path does reach that refusal — the declaration is refused, so the
   route can never turn the default pool into a scratch pool.
2. `refuse_unsafe_pool_path` keeps every refusal and message it has, in the same
   order, and the scratch branch is one extra test *after* all of them.
3. The route needs three independent facts at once: an explicit per-invocation
   declaration, an explicit pool selection, and a pool strictly inside that
   declared root on a verified non-boot APFS volume. No single input enables it.
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

`oxide status` gains one bounded text label and no new walk: a scratch pool
prints `pool: <path> (scratch pool, volume <uuid>)`; a recorded scratch pool
whose volume is absent prints `pool: <path> unavailable (scratch volume absent)`
and still walks nothing; `status --measure` keeps its existing walk rules and
still writes nothing. No JSON schema changes: `--json` and `pool describe --json`
keep their current keys and add no scratch object. `remember_pool` records the
path exactly as today (so `status` can report an absent-volume scratch pool),
and a fixture's `OXIDE_POOL` selection is still not recorded. Refusals print the
class in words, never a raw path dump, bounded and escaped as today.

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

1. The selector drops `--root-parent /private/tmp`; limits, admission, the
   512 MiB monitored scratch and cleanup are unchanged, and each migrated
   selector still states its pool premise in its own comment.
2. The script sets `OXIDE_SCRATCH_ROOT` to one private directory inside its own
   `OXIDE_TASK_SCRATCH_ROOT` run root (created 0700, canonical, same device) and
   keeps `OXIDE_POOL` strictly inside it, with an explicit `OXIDE_POOL_CAP`.
   No absolute mount path, no second scratch root, nothing outside the monitored
   root.
3. The script's pure `proof:*:test` self-check pins that environment contract,
   so a script left half-migrated fails an effect-free check instead of running.
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
`GitWorkTree`, `ExternalVolume`) with the messages unchanged, plus one pure
function that decides the scratch branch over injected facts (declaration, pool
selection source, root facts, mount facts, binding). No behaviour change yet:
the scratch function is unreachable until S2 wires it. Tests: the whole refusal
matrix as pure cases, message equality for rows 1–3 and 19, and the ledger,
config and registry formats unchanged. Selectors: `effigy rust:compile`,
`effigy rust:format`, `effigy test rust` (the AGENTS.md unit recipe), and
`effigy docs:check` when docs change. 300 s, 512 MiB.

**S2 — the opt-in setting. STANDARD.** Read `OXIDE_SCRATCH_ROOT`, apply the
scope rules (default-pool refusal, budget requirement, environment-only,
`status` label, creation notice, bounded refusal text) and wire the S1 branch
into the four `store.rs` call sites. Tests: precedence and inertness on internal
paths, rows 4, 17, 18, 19, 20, `status` zero-write and zero-walk regression, and
one CLI case on a private internal fixture that shows the declaration changes
nothing there. Selectors: as S1. 300 s, 512 MiB.

**S3 — creation, binding and open on a verified volume. ELEVATED.** This slice
enables a `directory-budget-v1` pool on removable media, so the actual trigger
is: its admission walk decides whole-slot deletion on a volume outside the boot
disk. Write and verify `oxide-managed/scratch.json`, refuse the absent,
mismatched, invalid and incomplete states, and label `status`. New pure selector
`effigy proof:scratch-pool:test` (`--self-check`, creates nothing); new heavy
selectors `effigy rust:scratch-pool:diagnostic` (uncounted, required first,
HIGH premise checklist) and `effigy rust:scratch-pool` (counted), both one fixed
script and the whole command of the qualified shared supervisor
(`bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
scripts/test-scratch-pool.py`), 300 s, 512 MiB, one owned 128 MiB APFS image
attached with `hdiutil -owners on` and ordinarily detached, private `HOME`,
`CARGO_HOME` and config. Cases: (01) first leased command on an *unregistered*
scratch volume creates the leaf, the binding and the pool, and builds; (02)
warm reuse from the same declaration; (03) the same pool without the declaration
takes the ordinary external refusal, with the binding and pool bytes unchanged;
(04) pool path equal to the root, a sibling-prefix path and a missing parent each
refuse with nothing created, and a root inside a Git work tree keeps the
ordinary Git refusal; (05) no explicit budget refuses; (06) a declared root that
is a symlink, group-writable, or a mount root refuses, while a declared root on
the boot volume is inert and the same recipe builds an ordinary internal pool;
(07) ordinary detach: `status` reports unavailable with no walk and no write, a
leased command refuses, no shadow directory appears and the mount point is not
recreated; (08) re-attach the same image: same UUID, warm reuse; (09) a
*different* image at the same mount name refuses by UUID; (10) a copied pool
(binding edited to another UUID, then the original bytes restored) refuses and
the pool is untouched; (11) a regular directory left at the mount name (a stale
mount point) is not a mounted volume root and refuses as unverified. Each case
reports its own oracle; the diagnostic records
reached/completed/failed/skipped and the latched first fault in a bounded
`qualified: false` receipt.

**S4 — recipe migration. STANDARD.** The ten drops and three keeps above, the
per-script `OXIDE_SCRATCH_ROOT` wiring, the pure self-check pins, and the
`AGENTS.md` / `workflow.md` amendments. Selectors: `effigy docs:check`, each
migrated proof's existing `proof:*:test`, and the short independent recipe
review. No native re-run of a previously passed proof and no new activation.
300 s, no scratch (pure checks only).

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

## Review gate map

- `default_pool_protection_unweakened` — "Default-pool protection", refusal
  matrix rows 1–4 and 19.
- `explicit_opt_in_and_volume_binding` — "The opt-in", "Placement rules",
  "Volume identity binding".
- `refusal_matrix_and_absent_volume` — "Refusal matrix" and binding case 07.
- `migration_list_and_internal_premise_cases` — "Migration list" and
  "Recipe changes".
- `finite_slices_and_validation_plan` — "Implementation slices" (tiers,
  selectors, wall, scratch) and "Limits of this design".
