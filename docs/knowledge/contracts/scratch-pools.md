# Scratch build pools on an external validation volume

This contract owns the scratch pool class: an explicitly declared, UUID-bound
`directory-budget-v1` pool on a verified external volume, for validation and
fixture scratch. It is the only route that admits that pool type on an external
volume. Pool selection, the external-volume refusal and scratch placement are owned by
[workflow](workflow.md); the ledger, eviction and closure rules are owned by
[local lease](local-lease.md).

**Status: implemented** (admission, opt-in, creation, binding, record and leased
open). Proof selectors that used this pool are retired, so this contract covers
product behaviour only.

## The opt-in

`OXIDE_SCRATCH_ROOT` names one absolute, canonical, real directory: the declared
scratch root. It is the declaration and the opt-in. There is no config key, no
boolean and no persisted setting, so a later invocation cannot inherit it. An
empty, relative or unreadable value refuses; it is never "off".

- It is read only after ordinary admission has refused for the external-volume
  reason. It cannot widen or reorder any other refusal, and it is inert for a
  path on the boot volume.
- The pool must be selected explicitly (`OXIDE_POOL` or the user config
  `[pool] path`). The built-in default pool refuses under this route.
- The pool must declare its budget explicitly (`OXIDE_POOL_CAP` or `[pool]
  budget`). The built-in default budget refuses.
- Debug-only product proof hooks arm only when the environment carries the proof
  supervisor's layout: an absolute deadline, a canonical owned scratch root, and an
  evidence directory nested in that scratch or in its `scratch`/`evidence`
  sibling. The layout check is always compiled, but the hooks that act on it
  exist only in debug builds; release builds carry none.

## Placement

1. The declared root is an absolute, canonical, real directory owned by this
   user, on a non-boot volume, with no group or other write bit, and not a mount
   root.
2. The pool path lies strictly inside the root. The root itself, an escaping
   path and a name-prefix lookalike all refuse.
3. The pool's parent must already exist inside the root. Oxide creates only the
   pool leaf, with mode 0700, never a missing ancestor under an external root.
4. The root's volume must be a verified external volume: a one-component mount
   directly under `/Volumes` whose mount point matches, APFS, not the boot
   volume, not read-only, with a readable volume UUID. Verification uses the
   removable backend's own binding primitive; no shell-out. `/Volumes` itself
   must pass the ownership and mode check.
5. Ownership and mode checks are consistency and tamper guards for a cooperative
   same-user host. macOS ignores ownership on the Builds drive, so no behaviour
   depends on them being enforced.

## Admission order

- Traversal and ancestor checks run first, unchanged.
- The external-volume judgement follows. Only the scratch branch lifts it, and
  only for a declared, verified path.
- When it lifts, the Git work-tree check runs on the same canonical ancestor and
  must pass. A path inside a work tree refuses with the Git message.
- With no declaration, the external-volume refusal stands unchanged.
- Root, binding and host record are verified before the store opens or creates
  anything.

## Volume identity binding

Identity is the volume UUID. Mount facts are not identity: a device number
changes on remount, so it is never stored or compared. The same UUID remounted
at the same mount point is warm reuse.

- **On-volume binding.** `<pool>/oxide-managed/scratch.json`, schema
  `oxide.scratch-pool/1`, strict and size-bounded. It records the volume UUID,
  mount point, scratch root, pool path and creation time. It is written
  atomically before the pool payload is initialised. The pool's own config,
  ledger and control files are untouched.
- **Host identity record.** `<account state root>/scratch-pools/<sha256 of the
  normalised pool path>.json`, schema `oxide.scratch-pool-record/1`, written for
  every scratch pool. It survives detach, so the absent-volume refusal and
  `status` can name the volume. It is identity metadata, not a pool list.
- **Write order.** Leaf, then on-volume binding, then host record, then pool
  initialisation. If the host record cannot be written, the invocation removes
  only the leaf it just created and refuses.
- **Repair.** A leased open with the volume present, a matching on-volume
  binding and no host record writes the record once. `status` never writes.
- **Re-verification.** Every leased open requires the declaration to name the
  recorded root, the recorded mount point to still bind to the recorded UUID,
  the pool to be strictly inside both, the on-volume binding to be a regular,
  non-symlink, bounded file of the known schema, and any host record to agree on
  UUID, mount point, root and pool. A difference refuses. Nothing is repaired,
  adopted or rewritten, so an edited record cannot retarget a pool and a copied
  pool cannot inherit a record.
- **Absent volume.** When the mount point is gone, or the pool's nearest existing
  ancestor is on another device, a leased open refuses before any create, walk,
  eviction, lock or write. With a host record the refusal names the volume UUID;
  without one it states that no record matches and nothing was created. There is
  no internal fallback, shadow directory or recreated mount point.
- **Incomplete.** A leaf with a valid binding but no pool config is a prepared,
  uninitialised pool. The creating invocation completes it; a later invocation
  refuses. A pool directory at a scratch path with no binding is never adopted,
  initialised or chmodded.
- **Removable volumes.** A registered removable volume passes these checks
  unchanged. The checks read no registration document and need no internal pool.

## Refusal classes

Refusals name a class, not a raw path dump. The main classes:

| Class | Condition |
| --- | --- |
| `scratch-root-unsafe` | Declared root empty, relative, unreadable, symlinked, on another device, a mount root, or not owned and private |
| `scratch-declaration-default-pool` | Declaration combined with the built-in default pool |
| `scratch-budget-required` | No explicit budget |
| `scratch-path-outside-root` | Pool path equals the root, escapes it or is a prefix lookalike |
| `scratch-parent-missing` | Pool parent missing under the root |
| `scratch-volume-unverified` | Root volume is not a verified external APFS mount |
| `scratch-binding-missing` | Pool at a scratch path without an on-volume binding |
| `scratch-binding-invalid` | Binding unreadable, symlinked, foreign-owned, oversized, malformed or unknown schema |
| `scratch-binding-mismatch` | Binding UUID, mount point, root or pool differs from the observed facts |
| `scratch-record-invalid` | Host record unreadable, malformed or keyed to another pool |
| `scratch-record-mismatch` | Host record disagrees with the binding or the observed facts |
| `scratch-record-unwritable` | Host record cannot be written at creation |
| `scratch-incomplete` | Prepared pool without config on a later invocation |

The traversal, ancestor, external-volume and Git messages keep their existing
wording when no declaration is present.

## Read-only status

`status` and `--measure` never lease, create or write. A scratch fact exists only
when a host record exists:

- record present, volume present, binding verifies: `pool: <path> (scratch pool,
  volume <uuid>)`;
- record present, binding missing, invalid or mismatched, or volume absent:
  `unavailable` with the class, and no walk;
- no record: the pool takes the ordinary path rules, with no scratch line.

A missing record is repaired only by the next leased open, so until then the
pool has no scratch line. JSON output keys do not change.

## Default-pool protection

- The built-in default pool is refused under this route, even when the account
  home sits on an external volume.
- Only the scratch branch may lift the external-volume refusal, and the Git check
  always follows it. No refusal is reordered for an ordinary path.
- Admission needs four facts together: an explicit per-invocation declaration, an
  explicit pool selection, a pool strictly inside the declared root, and a
  verified non-boot APFS volume whose binding and record agree.
- Creation prints one notice that the pool is not for ordinary project builds,
  and `status` labels it.
- A scratch pool gains no authority: the same ledger, eviction, held-slot
  exclusion, unknown-writer conservatism, watchdog and closure rules apply. It is
  never selected by drive mode and is not the removable pool.
- Residual risk: an operator who points `OXIDE_POOL` into a declared root gets a
  directory pool on an external volume. That is the opt-in. It is per invocation
  and cannot touch the default pool.

## Limits

- Identity and placement are checked immediately before each open. The design
  claims no freedom from time-of-check races: the pool's own I/O keeps the
  path-based rules.
- The binding proves identity and placement, not that the bytes may be deleted.
  Eviction and the soft budget decide that.
- A scratch pool inside an owned run root goes with that root. A pool kept
  outside one is the harness owner's retention problem.
- Linux has no `/Volumes` mount facts here, so the scratch branch refuses there.
  That is a refusal, not a portability claim.
- The host record belongs to the invoking account's state root. A different
  state root sees no record: a present volume is still admitted from the
  on-volume binding, and an absent volume is refused without a UUID.

Covering checks are the ordinary selectors: `rust:compile`, `rust:compile-release`,
`rust:format`, `rust:barge-adoption`, and `docs:check` when docs change.
