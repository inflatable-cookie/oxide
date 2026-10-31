# Local lease library (standalone CLI retired)

**Retired, oxide#102 (Tom, 2026-10-20).** The standalone
`oxide-storage-lease` binary, its `rust:storage-lease-cli` selector and the
`apfs-sparse-image-v1` backend are gone from this repository. The library
(`oxide_storage::lease::managed`) keeps the pool, ledger, leases, release,
recover, status, idle eviction and the `oxide` control directory below, and
opens only `directory-budget-v1` and `apfs-volume-v1` pools; a pool recording
`apfs-sparse-image-v1`, or holding an image or receipt file, refuses with
`backing_mismatch` before anything is written. `LeaseRequest.budget_bytes`
carries the caller's soft budget for one admission. Current pool, budget and
recovery behaviour is owned by
[workflow](workflow.md#budget-only-default-pool-and-user-configuration);
`oxide pool recover-interrupted <slot>` replaced the CLI's explicit `recover`.
The sections on the CLI, image backing, capacity status, exit codes and the
reviewed handover below are history: Nucleus's installed binary and the
reviewed handover copies outside this repository are retained, untouched.

### Retired lease-CLI fixture audit (papercut 17674724, closed deprecated)

Papercut `17674724-b4d7-414d-916f-e53d277f5ee8` asked for the retired
standalone lease CLI fixture's scratch cleanup. Tom approved conditional
deprecation (Supervisor `083047a8`). The original subject is bound to oxide#064
/ PR #64 review `6213212075` at source `ecc185bc`:
`scripts/test-storage-lease-cli.py`'s `scratch_root` (line 32) called an
external `mktemp`, and `main` (line 439) and `prove_usage` (line 135) each
called it. Current main `d246d40` has no such script, no
`rust:storage-lease-cli` selector and no `oxide-storage-lease` binary;
[retirements](../retired.toml) records exactly that script, selector and
binary as retired on 2026-10-20. Both adviser inventories found no execution
route. The retained `src/lease/tests.rs` `Fixture::new` does execute through
`test rust`, but it is a distinct Rust fixture using `std::env::temp_dir`, not
the original repeated-`mktemp` path, and it is not declared retired or fixed.
The suspended `oxide-cli` modes fail before effects; `clean-safety` and
`launch-check` each allocate one fallback root and preserve failure evidence.
Disposition: closed as deprecated with this bounded inventory evidence. No
historical temporary-directory cleanup and no fix claim follows, and no broad
temporary-file refactor or Rust-fixture cleanup is implied.

Former status: unpublished Oxide-only product command. Same-UID callers take exclusive
owned slots on a verified APFS payload volume through a short-lived OS lock and
a durable JSON ledger. There is no resident daemon, HMAC credential, issuer,
callback service, Cargo runner, scheduler or Git manager. A focused pinned
Nucleus `dev` workflow demonstrated cross-worktree registry reuse under
ordinary Cargo and cooperative writer-closed release. The [consumer boundary](project-adoption.md#current-nucleus-integration-boundary)
and [raw-backed evidence](../evidence/oxide-064-cross-path-qualification.json)
record the qualified scope and limits. Consumer integration remains owner-run;
this is not an installed service or a general source-invalidation guarantee.

Publication naming remains [Q-005](../questions.md#q-005--what-names-are-safe-to-publish).
The package stays `publish = false`. The binary name `oxide-storage-lease` is
internal. Capacity semantics were owned by the bounded-storage contract,
retired with the image backend. Consumer adoption remains
[project adoption](project-adoption.md).

## Callable

Library (`oxide_storage::lease`) and one unpublished binary built by the
ordinary workspace:

```sh
oxide-storage-lease --pool <path> init --cap-bytes <n> --image-bytes <n>
oxide-storage-lease --pool <path> lease --project <canonical-root> \
    --workspace <checkout> --profile <dev|full-debug> --holder <id>
oxide-storage-lease --pool <path> release --lease <id>
oxide-storage-lease --pool <path> recover --lease <id>
oxide-storage-lease --pool <path> status [--capacity] [--json] [--max-output-bytes <n>]
```

Covering CLI proof is `effigy rust:storage-lease-cli`: it builds the binary,
exercises usage refusals, then on macOS with a 20 GiB host reserve provisions
one fresh 128 MiB APFS image and runs lease/release/recover/status, capacity
sampling, concurrent acquire, duplicate-holder replay, busy, symlink/foreign
refusal, idle reuse, delayed APFS metadata arrival and mismatched-backing
refusal. Rust lease tests cover injected zero/unknown samples, hardlinks,
metadata separation and byte-for-byte observational behavior. Synthetic
fixtures do not claim native quota behavior. It is not a filesystem-fill
benchmark and not full QA.

## Pool

`--pool` is required. It is the owner-only control directory (mode `0700`,
caller uid). Init creates it empty, or tightens an empty caller-owned directory
to `0700`. A directory whose only occupant is the CLI's leftover `lock` file
from a failed init is still init-able. An existing pool cannot have its cap
raised and cannot be reinitialized. Arbitrary directories are not reinterpreted
as bounded.

Pinned at init and recorded in `config.json`:

| Field | Meaning |
| --- | --- |
| `version` | `1` |
| `backend` | exactly `apfs-sparse-image-v1` |
| `store_id` | 32 lowercase hex chars, never reused |
| `cap_bytes` | declared payload size; must equal `image_bytes` at init; callers cannot raise it |
| `image_bytes` | sparse image size; must equal the declared cap so the volume size is the enforced ceiling |

Control files stay on the host: `config.json`, `ledger.json`, `lock`,
`image.sparseimage`, `receipt.json`. The payload mount is `<pool>/payload`.
Slots live only under `<pool>/payload/slots/<id>/`. Ledger and receipt are
atomic replace plus `sync`. The exclusive `flock` is held only while the CLI
runs. Cargo and tests run after the process exits; a finished CLI is not writer
completion.

Directory-only roots are not a product mode. Tests may inject a private
directory solely to exercise the ledger; that path is not compiled into the
CLI and does not claim an OS hard bound.

The lean pool accepts these exact APFS metadata directories alongside
`slots`: `.fseventsd`, `.Spotlight-V100`, `.Trashes` and `.TemporaryItems`.
Each must be a real directory on the verified payload volume; the operating
system may own it. The pool does not recurse into or inventory these entries,
and every other payload-root entry refuses. Their writes still consume the
APFS volume's enforced capacity.

## Oxide front door control directory

The unpublished [`oxide`](workflow.md#implemented-foundation-core) command
leases through this library. It keeps its own state in `<pool>/oxide-managed/`,
an owner-only real directory. Pool opens accept that one extra entry; a
symlink there refuses like any other. The CLI never reads or writes it, and a
pool without it opens as before. Oxide passes one private directory under it
as each lease's `--project`, so library reuse follows Oxide's records: a
checkout's own slot, or another checkout's idle slot only through the
[freshness gate](workflow.md#cross-checkout-freshness). Its
automatic crash recovery touches only leases whose ID or holder string its
own control record names.

Oxide calls the library's `managed` entry points (`lease`, `release`,
`recover`, `status`, `snapshot`, `measure`, `evict_idle_slot`,
`init_budget_pool`, `init_volume_pool`, `flush_volume`). They behave as
the functions below on an APFS pool and also open the budget-only
`directory-budget-v1` pool ([workflow](workflow.md#directory-budget-v1-as-shipped))
and the removable `apfs-volume-v1` pool ([workflow](workflow.md#removable-build-volume)),
whose config adds a `volume` object (mount point, volume UUID, payload
directory); other configs serialize unchanged.
The CLI and the plain library functions keep refusing both as
`backing_mismatch` (exit 4), so Nucleus and other CLI consumers see no
change. A directory pool's config records `backend: directory-budget-v1`,
the budget as `cap_bytes` and `image_bytes: 0`; an older build refuses it.
`OXIDE_POOL_CAP` chooses the cap of a pool the front door creates
([workflow](workflow.md#implemented-foundation-core)); this CLI's
`init --cap-bytes` and every existing pool's recorded cap are unchanged.

Oxide resolves its host-wide active-build limit once per command from
`OXIDE_MAX_ACTIVE`, `[pool] max_active`, or the approved default. The limit
continues to count active commands across internal and removable pools using
the existing state and admission rules. `oxide status` reports the effective
value and whether it came from the environment, the selected user config, the
physical-core formula or its fallback; see the workflow's
[user configuration](workflow.md#budget-only-default-pool-and-user-configuration).

The library also exports `evict_idle_slot` for Oxide's per-project slot cap
(twelve slots per project identity, oxide#177; [workflow](workflow.md#storage-and-space-management)).
It retires one recorded idle slot whole through the same durable eviction path
as capacity eviction, and returns busy for a held slot. The CLI does not
expose it. Ledger v1, receipt v2, JSON and exits are unchanged. An earlier
`oxide-storage-lease` build refuses a pool that has `oxide-managed` as an
unknown entry; use the current build against pools the front door touched.

Proposed, not implemented: the front door would add one
`oxide-managed/nested/<record>/` generation directory per leased command
(with a `gate` lock file and `writers/` tickets) and one optional `nested`
field on the control-state record (the generation nonce, its depth and any
borrowed root permit reference), so a nested `oxide` can join that generation
or borrow its host permit. The ledger, lease identity, slots, accounting and
release order are unchanged; the field is control state, not a lease, and it
is distinct from the idle-exec design's `permit` (held/released) fact. The [nested lease design](nested-lease.md) owns the rules, the state
fence and the proof plan; its independent design review passed (comment
`6412288358`) and the operator's design freeze is pending.

## Commands

**init** checks cap and image size, and refuses unsupported platforms, before
any host effect. A declared cap that differs from the image size refuses:
a smaller cap on a larger volume is admission only and is not OS-enforced.
It then provisions one sparse APFS image, attaches it at
`payload`, writes the capacity receipt, pins the cap, and returns JSON
`PoolInfo`. The receipt's measured `verified_capacity_bytes` is the effective
OS hard bound; it sits slightly below the declared size by APFS metadata
overhead, and writers cannot exceed the declared ceiling through it. A volume
that measures larger than declared refuses without migration. Receipts that
predate this rule (version 1, or cap not equal to image size) refuse as
mismatched backing. A leftover empty `lock` from a failed init does not prevent retry.

**lease** requires canonical existing project and workspace directories,
`dev` or `full-debug`, and an opaque holder. Project is stable identity;
workspace is the actual checkout and remains observed. A compatible idle slot
(same project and profile) is the reuse candidate, including a different
workspace. Exact duplicate holder+project+profile+workspace while held
reconciles to the same lease id instead of allocating a second lease.
Conflicting reuse of a held holder refuses. A different holder against a held
compatible slot is busy. Identity is `v1.<store>.<slot>.<incarnation>.<seq>`
and is never reused. The lease is committed before paths are returned.

Returned environment:

- `CARGO_BUILD_BUILD_DIR` — slot `build` directory on the payload volume
- `CARGO_TARGET_DIR` — `<workspace>/target` (private final output)

The CLI does not invoke Cargo, force incremental/debug/feature flags, remap
rustc paths, or copy source into the slot.

**release** is the same-UID caller declaring every writer using this lease has
ended. It is not QA success. Replay against an already-idle lease is
access-free and idempotent.

**recover** is the same writer-closed declaration for held occupancy the owner
treats as abandoned or uncertain. The CLI and library never infer abandoned
from TTL, PID, age or holder loss; status reports such occupancy as `held`
until release or recover. Automatic TTL reuse is not implemented. The one
exception is outside this CLI: the `oxide` front door calls `recover` for a
lease it took itself once that command's record lock is free and its Cargo
process group is empty ([workflow crash recovery](workflow.md#crash-recovery)).
A lease taken through this CLI is never recovered that way.

**status** lists finite slots with holder, observed workspace, occupancy,
logical and unique allocated bytes, pool cap, and full/unknown. Coverage is
`complete`, `partial` or `unknown`. Unknown accounting is honest. Each walk
also reports its gap kinds, entries visited, elapsed time and bounds
(`payload_accounting`, and `accounting` per slot; see
[accounting walks](#capacity-and-eviction)). No payload
or secret content is printed. The legacy `full` field compares owned-slot
unique allocated bytes with the declared cap. It retains that documented
meaning and does not establish physical filesystem headroom.

**status --capacity** reports the same ordinary status inside a separate
version-1 JSON envelope (`version`, `status`, `capacity`, `notes`) or concise
human output. The nested legacy `PoolStatus` fields do not change. The capacity
section reports the declared cap and receipt-verified payload ceiling, raw
payload filesystem free and unprivileged available bytes, the pinned sparse
image's logical and allocated file bytes, and host filesystem available bytes
at the pool directory. Byte fields are exact integer byte counts or `null`
with a bounded `unknown_reason`; zero is a measured zero. The source fields
are `statfs f_bfree * f_bsize` and `statfs f_bavail * f_bsize` on the exact
verified payload mount, image `st_size`, image `st_blocks * 512`, and host
`statfs f_bavail * f_bsize`. Unsupported or failed non-payload samples remain
unknown. `coverage` is complete only when each capacity observation is known.

Capacity mode takes the existing pool lock without creating it, verifies the
already-pinned receipt, image identity and mounted APFS root, then samples
metadata only. A missing, replaced, unmounted, symlinked or unsafe backing
refuses; it does not attach an image or sample a replacement filesystem. It
does not reconcile or remove an `Evicting` slot, or write the config, ledger,
receipt or payload. Existing `Evicting` state remains visible and makes the
nested accounting coverage unknown. Held slot accounting remains observational.
Writers may still mutate held payload files while the command samples them, so
the report is not an atomic snapshot.

The payload filesystem free/available sample is not a reservation, build-fit
prediction, attributable write total or guaranteed reclaimable space. Do not
add slot accounting to image allocation or subtract slots from the nominal cap
to infer physical free space. Image `st_blocks` is the host-reported allocation
of that file, not APFS unique extents or snapshot accounting. Private target and
temporary files, control files, OS metadata and snapshots are not combined
into a whole-host cost; total host cost and Finder purgeable space remain
unknown. Capacity reporting grants no storage ownership or cleanup authority.
The envelope notes explicitly retain the legacy meaning of `status.full` and
the limits of sampled capacity and image allocation.
Either capacity observation coverage or nested slot coverage that is partial
or unknown, and any output truncation, returns exit 2.

## Capacity and eviction

### Whole-tree holding deletion (oxide#190)

Eviction no longer walks a slot tree under the pool lock. The measured problem
(2026-10-29, live Builds pool) was #155's per-entry capture, verify and unlink
with a journal kept inside the ledger: the ledger was rewritten and fully
synced for **every** entry while the pool lock was held, so a 15.5 GB slot held
the lock for about 23 minutes at about 60 entries per second and every
admission and release on the host waited. The slot tree is Oxide's own private,
owner-only directory, so per-entry journaling was unnecessary.

**Under the pool lock (milliseconds).** The victim is chosen exactly as before
(an idle, positively writer-closed, verified slot; unknown or partial slots stay
ineligible). The slot root is verified as the real owner-only directory the
ledger records, on the slots root's own device; the descriptor of that root is
opened without following a link and the identity it reports is bound durably and
**carried to the rename boundary**, so a root a racer replaced after the check
refuses (`UnsafePath`) instead of being held and later deleted. `renameat` then
moves the tree **whole** into a private holding area on the same device:
`<payload>/.oxide-holding/<store>-<slot>-<incarnation>-<nonce>`. The area is
created owner-only (`0700`) and verified (real directory, caller's own, payload
device). One durable revised ledger write then records one holding row — name,
slot, incarnation, the directory identity the rename moved, the recorded bytes
and when it was recorded — and drops the slot row.

Two synced ledger writes in total, both O(1): one **rename intent** before the
rename (the derived name and the verified identity) and the row swap after it.
Nothing is written per entry — the tree is a private directory that becomes
unreachable from every slot path the moment it is renamed, which is what made
per-entry journaling unnecessary. The brief asked for one sync; the Oxide
Supervisor accepted two on 2026-10-30 (decision `1e522ecc`) and clarified on the
same day (decision `e0f63aa4`) that the authority is **O(1) durable transactions
per eviction, not per-entry durability**, and that it does not waive the
under-one-second lock interval. The intent is what makes a crash between the
rename and the row recoverable **by identity** rather than by a name. Any figure
quoted for the lock-held interval therefore covers the intent write, the rename
and the row write together.

The nonce is the generation's own recorded `last_lease_id`, not a fresh random
token. Every part of the entry name is durable ledger state, and the intent is
what makes each window recoverable **by identity rather than by name**: a crash
before the intent leaves an untouched idle slot that is still a candidate; a
crash between the intent and the rename leaves a stale intent that the next open
clears (nothing moved, nothing adopted); a crash between the rename and the row
swap leaves the idle row, its intent and the tree under the derived name, which
the next open adopts only because the entry's identity is the one the intent
bound; a crash after the row leaves a charged, isolated row a later command
finishes. Nothing in any window is concluded from an absent name or from a name
alone.

**Recovery order and the legacy migration window.** `Pool::open` reconciles the
holding area **before** it replays a recorded deletion, and the legacy replay
verifies its pending root in **O(1)**: the root is opened without following a
link and its identity, owner, mode and device are checked (against the journal
when one exists) — the recursive inventory preflight #155 needed is gone, so one
large legacy pending tree can no longer hold the pool lock across every admission
on the host. Per-object refusals belong to the outside-lock deletion, which keeps
anything it cannot delete and reports it. Adoption therefore
covers every generation whose tree is already in the holding area, including a
legacy `Evicting` row interrupted between its migration rename and the write
that would have recorded the holding row: that row is adopted (its per-entry
journal, if any was already resolved, is dropped with it) and the pool is not
left blocked behind a root its own rename moved. A tree still at its recorded
root adopts nothing, so an unreplayed legacy journal keeps its own record until
the replay decides it.

**Adoption is always identity-bound; a name alone adopts nothing.** Every
generation that can be a rename source — an **ordinary idle eviction** and a
recorded legacy deletion, both through the same `retire_idle_slot`/migration
call — carries a durable `hold_intent` naming the derived entry and the root
identity the writer verified, written before the rename. On the ordinary path
that is the one intent write the eviction makes before its rename; the row swap
that drops the slot row and records the holding row follows the rename. Recovery adopts an interrupted
rename only when the entry's own `(device, inode)` **is** that bound identity.
A stranger at the derived name, an entry whose identity differs, and an entry an
observation error could not classify are all never adopted: their record, its
charge and any journal stay, the pool keeps its refusal, and only a replay (for a
legacy row) or the operator decides. An `Evicting` row with no intent adopts
nothing at all, and a held row is never a rename source.

**A private capture name is never ordinary content.** The recursive #155
inventory preflight that once refused such a tree ran under the pool lock and is
gone; the same rule now lives in the outside-lock deletion. A name matching the
private capture prefix (`.oxide-capture-…`) inside a held tree is #155 journal
state that no record names any more, so it is **kept as uncertain**: the entry
stays in place, the tree stays held and charged, and the pass reports it. It is
never unlinked as ordinary content, and no locked walk is needed to notice it.

**A rename that moved the wrong object is contained as well.** The source root's
descriptor identity is compared with the caller's verified identity before the
rename and with the moved entry afterwards. If a swap happened inside that
window, the foreign tree is moved to `.unverified-<derived name>`:
`holding_name_is_bounded` refuses a leading `.`, so no generation can derive that
name and no recorded row names it. Whether or not that containment rename
succeeds, the identity binding above already makes the entry unadoptable; the
containment additionally keeps a foreign tree out of the reachable name space and
the command refuses. The payload walk keeps charging those bytes, and the
generation's own row stays recorded (its root is wherever the racer put it), so
the slot is never silently freed and no foreign bytes are ever deleted.

**Only a positively classified miss is absence.** A recorded holding row is
dropped when its entry is gone with `NotFound`. Any other `lstat` failure
(`EACCES`, `EIO`, an over-bound name) is **unknown**, never absence: the row and
its charge are kept, the pool refuses rather than concluding, and a pass that
cannot open a recorded entry keeps it, reports it and deletes nothing else. A
holding entry is therefore never reported as deleted, and never stops being
accounted, because an error could not be classified. The one positive absence
that finishes rows without deleting anything is the **holding area itself**: the
area probe reads the payload first, a locked open holds the pool lock (so no
other command can be creating it), and no tree can live in an area that does not
exist — so each recorded row there is finished and stops being charged instead of
keeping its charge forever because its directory vanished outside Oxide's
control. An absent removable volume never reaches that conclusion, because the
open that gets there has already resolved the payload.

**Asynchronous delete, outside the lock.** The command that evicted the slot
deletes that tree on a worker thread it owns while its own writer runs, and
joins it before exit with a bounded wait whose progress is printed (60 s, then
the entry is left recorded for a later command). The traversal holds
descriptors only: `unlinkat`/`AT_REMOVEDIR` against a held parent, `O_NOFOLLOW`
directories, never a path lookup, never a followed link (a symlink is unlinked
as a leaf), never another device. An owned directory the owner left read-only
or without its read bit is entered through a search-only descriptor and its own
mode gains the owner bits, exactly as the retired path did; the claimed tree's
**own root** is repaired the same way before its children are touched, so a slot
tree the owner left owner-no-write is still deleted whole instead of staying
charged forever. Each listing batch opens the directory afresh (never a `dup`,
which would share the offset and make the next batch read an empty directory),
so a refused entry can never be mistaken for an empty one. An already-missing
entry counts as done, so a pass is idempotent. When the entry directory is gone,
one brief locked write drops its row. A regression proves the device boundary
both ways on one tree by injecting the device the traversal compares against: the
foreign-device child directory is refused by name and kept while an ordinary file
beside it is removed, and the same tree with its real device empties completely.

**Progress does not depend on admission.** The refusal is re-evaluated for every
positive removal, including one the *pre-admission* pass made after the refusal was
decided (that pass runs beside the admission, so a removal — not a pass — is the
trigger). A command therefore never retries against an unchanged figure and never
gives up while a removal has already changed it. Takeover order is
least-recently-attempted first, recorded as one O(1) write when a pass concludes
with a row still in place, so an undeletable row cannot starve a later deletable
one even while every admission is capacity-blocked and no grant or release
advances the rotation.

**Custody and publication.** The evicting admission reports exactly the holding
entries it created, and a command that evicts a slot by ID names that entry
through the same derivation, so a command always publishes **its own** eviction
instead of selecting a row by rotation; the rotation remains only the takeover
path for entries a command did not create. A claim takes an exclusive
**non-blocking** `flock` on the holding entry's own directory descriptor, under a
brief pool lock, and holds it for the whole deletion. A second command that finds it held claims nothing and deletes
nothing: a live custodian is never waited for, and nothing infers custody from a
PID, an age or a timeout. Each pass claims at most one entry: the name its own
command published first, otherwise the **least-recently-attempted** recorded row
(with the oldest row breaking a tie), so a permanently refused entry never keeps
another entry from being deleted by a later command. That order is independent of
grants and releases: a pass that concludes with a row still in place records the
attempt in one O(1) write, so the rotation advances even while every admission is
capacity-blocked. A deleted entry's descriptor is closed with its last name gone,
so a later pass can never re-acquire it.

**One bounded victim per decision.** Because a held tree keeps its bytes charged,
the figure cannot fall inside one locked attempt. Byte pressure therefore selects
**one** victim, releases the lock, and the command publishes that tree and
re-evaluates from the new figure — up to a small fixed number of rounds, each
removing exactly one tree. Nothing evicts a warm idle slot ahead of the decision
that needs it, and a pool that cannot converge refuses with a bounded capacity
result. The count ceilings (slot and project) still evict only as many whole idle
slots as they need.

**Accounting.** A holding row is charged its recorded bytes (the generation's
closed observation, else the largest recorded slot, at least one byte) in the
admission capacity decision **until it is positively removed** — an eviction
moves bytes, it never frees them early, and no admission decides as if a held
tree were already gone. A holding row is neither a slot nor a member: it counts
toward neither the slot ceiling nor a project's slot cap, and the payload scope
walk isolates the recorded entries so nothing is counted twice. An **unrecorded**
entry in the holding area is never adopted and never deleted; it stays counted by
the payload walk (so its bytes are charged rather than free) and is reported. A
command whose admission is refused because of such bytes finishes a pending
deletion outside the pool lock and **re-evaluates**: a positive removal — not a
pass — is the trigger, so the admission is retried for a removal the
pre-admission pass made after the refusal as well, and each later round publishes
one more tree and re-evaluates again, up to a small fixed bound of four rounds. A
round that removes nothing keeps the last refusal. It never assumes the bytes are
free. An undeletable tree therefore keeps
its charge and its admission refuses with a bounded capacity result rather than
exceeding the budget. `oxide status` lists each held entry with its recorded
bytes, its age and one presence check — no walk, no custody, no deletion.

**Per-object failure.** An entry the traversal cannot delete (permission,
`EBUSY`, another owner, another device, a depth beyond the bound, an unknown
entry type) stays in the holding area, still recorded and still charged, and is
reported by name in the pass's bounded diagnostics. It never blocks admission
that has capacity and never blocks another holding entry: the attempt marker moves
it behind the others in the takeover order above.

**Mixed versions.** The fence comes first (#177's rule): control state version
9 always carries the forward-only `oxide.holding/1` fence, and the admission
gate makes it durable before any locked pool open or ledger write that can
record a holding row or a rename intent. An older binary hands off to a receipt-matching successor
(at its own `control_state_version` capability) or refuses; it never meets a
holding row with a schema it cannot represent. The library ledger's `holding`
row is omitted while empty, so a pool that never evicted keeps the exact bytes
an older binary wrote.

**Legacy migration.** A `#155` per-entry journal can still exist in a ledger
written by an older binary; its row carries the same durable `hold_intent` the
idle path uses, written before its rename, and adoption is bound by it. The first locked open resolves it exactly as #155's
replay rules allow (`reconcile_root_capture`, deepest-first rollback of recorded
captures, recorded mode restoration, `RootCaptureState::Absent`/`Uncertain` and
any recorded foreign capture staying pending) and then, instead of starting
another per-entry journal, moves the resolved root whole into the holding area
and drops the slot row in one write. An uncertain record is never reinterpreted:
it keeps its record and starts no holding row. `Pool::open` never starts a new
`#155` journal.

**Retired machinery.** Whole-tree capture, private-name journaling, per-entry
removal and its mode-widening guard are retired from the eviction path and
deleted with their eviction-path regressions; the shared descriptor-relative
owned-delete primitives (`owned_delete`) that `clean`, `prune` and the #155
replay still use, and their own unit tests, are unchanged. The test-only seams
that existed only for the retired per-entry path went with it — the per-entry
unlink, permission-repair, child-reopen and root-capture crash hooks and their
regressions — so no dead hook remains in `src/test_fs.rs`. `oxide clean` and the
other owned-delete callers keep their own rules: the holding approach is not
applied to them by this change, because a workspace target tree can contain
bytes whose ownership and writer closure are not a private slot's.

**Selectors and their oracles.** `effigy rust:holding-delete` is the focused
native proof (one fixed script under the qualified shared supervisor,
`--root-parent /private/tmp` for its genuine internal fixture pool, 300 s,
512 MiB), `effigy rust:holding-delete:diagnostic` its uncounted keep-going twin,
and `effigy proof:holding-delete:test` its self-check (one private temporary
directory, removed before it returns; no child, pool or staging), which includes the
progress-estimator counterfactual: a one-tick and a two-bucket window are rejected,
a sustained multi-bucket window yields its average rate, and the old one-tick
estimator is shown to fail where the corrected one is discriminating. Every native
run needs its own owner activation, and the counted run and the independent
reviewer native of the Elevated gate are separate. Both counted natives passed
4/4 cases at `2999860` (#190), each after a clean uncounted diagnostic, with the
eviction-deciding pool-lock interval held 20-29 ms.

**Premise checklist, recorded before any activation.** The script carries the
nine items of the [proof-authoring premise
checklist](workflow.md#proof-authoring-premise-checklist) in `PREMISE_CHECKLIST`,
and `--plan` prints them: private `HOME` and rustup (the invoking `RUSTUP_HOME`,
or the invoking `HOME/.rustup` when unset, is read before any private `HOME`
exists and passed to staging and every fixture with `RUSTUP_TOOLCHAIN` pinned and
`RUSTUP_AUTO_INSTALL=0`; staging is the only `--locked` call, against this
repository's committed lock, while the fixture crate is dependency-free and no
fixture invocation passes `--locked`); the account-home control (`OXIDE_TEST_HOME_FROM_ENV=1`
beside the private `HOME`); environment isolation (the staging sweep drops every
`CARGO_TARGET_`/`CARGO_BUILD_`/`CARGO_ENCODED_`/`SCCACHE_`/`OXIDE_`/`XDG_` key and
every compiler override, and the fixture environment is built from a fixed
allow-list); automatic adoption, Cargo's external-subcommand argv and owned
images (all three inapplicable, because no case uses permanent outputs, invokes
an external subcommand or touches an image); compiler flags (`RUSTFLAGS` frozen
with the encoded forms dropped); temporary storage (the documented
`--root-parent /private/tmp` internal-placement exception for the genuine
internal fixture pool, with per-tree `TMPDIR`/`TMP`/`TEMP`); and macOS `mktemp`
(no shell `mktemp` anywhere; the self-check's two temporary directories come from
`tempfile.mkdtemp`, which honours the supervisor's `TMPDIR`, and are removed).
The pure self-check binds the map to the source it names — all nine items in
order, every inapplicable item carrying its reason, every applicable disposition
naming a helper the script really defines — and it binds item 1's lock claim to
the two places it can drift: `create_project` must write a manifest with no
`[dependencies]` table and no lock of its own, and the only `--locked` Cargo
invocation in the script's effectful region must be staging's. A counterfactual
manifest that declares a dependency, and a second `--locked` call, each fail that
check, so a disposition cannot drift into a claim no case body supports.
Recording them activates nothing: the uncounted
diagnostic, the counted author native and the reviewer native remain separately
owner-gated, and none has run at this head.

Its concurrency fixture is asymmetrical: a 70,000-entry victim tree (above the
brief's 20,000-entry floor, sized so its deletion outlasts the bound's two-second
slack even at a conservative 15,000-unlinks/s floor) beside an 8,000-entry
competitor tree, under a 290 MiB cap the victim alone stays below and only the pair
exceeds — so both seeds record without pressure and exactly one eviction is forced.
Its concurrency case measures progress under the deleter's own non-blocking
custody with one cheap `listdir` count of the held tree's planted bucket
directories — never a full-entry walk per sample — and estimates the rate
only once at least three buckets are gone, as the average over the whole
first-to-last window, so a single 20 ms tick cannot satisfy it (the counter is
quantized: a bucket disappears only when its last entry is unlinked). The
competitor then starts while most buckets remain, and its warm-reuse admission
must complete with the evicting command alive, its holding row still recorded and
the held tree still non-empty (all read at the completion instant), inside the
in-lock control bound, with the estimated remaining deletion time required to
exceed that bound. Its diagnostic mode leaves every fixture command — and
therefore its descendants — in the qualified shared supervisor's own session and
process group: the script creates no session and never signals a group, because
session closure and the census that decides it belong to that supervisor. Each
child it starts is waited for, or killed by its own PID and reaped, and only a
positive reap counts as closure; an unreaped child stays tracked and ends the run
rather than continuing with unknown custody, and no closure of a descendant is
ever claimed. A recoverable oracle failure therefore continues the diagnostic
only when the failed case never started a fixture command at all: once such a case
has run a child, reaping the leader proves nothing about its descendants, whose
closure belongs to the supervisor's census, so the run aborts and every case not
reached stays `unexecuted`.

The proof's lock figure is the **whole eviction-deciding pool-lock interval**:
from the evicting command's pool-lock acquisition to that eviction's row write,
so it covers the fence, the ledger read, reconciliation, admission bookkeeping,
the byte-pressure decision and the recorded-total walks that feed it
(oxide#177/#186), the victim selection, and then the intent write, the
identity-verified rename and the row write. It is measured by a debug-only
observation armed only inside the supervisor's scratch layout, so an inner
fragment — and the rename transaction alone — cannot pass as the whole interval.
This is the interval decision `e0f63aa4` refuses to waive ("the under-one-second
lock interval is not waived"): the figure the counted native must show under a
second includes the accounting walks the eviction decision makes, not only the
rename. A command that evicts more than once under one lock records a figure
that also covers the evictions before it. Its killed-deleter case runs the
later command as a **warm reuse of the fixture's own idle slot**, not a new
project: the fixture is over its cap on purpose (the victim's recorded charge
plus the reusable slot exceeds the cap), so a new admission would legitimately
retire an idle whole slot — the very tree that case's oracle requires to survive
— and the case would prove the fixture's arithmetic rather than the takeover. A
reuse takes no capacity decision, so the command's only effect on a slot is
finishing the abandoned holding entry; the case asserts that the killed command
was reaped, that the row and its partly deleted tree survived, that the later
command reported and finished it, that no holding row remained, that the takeover
added no slot, that its own slot came back idle, and that slot's tree still
exists — and in the same run it confirms a reuse completes while a charged
holding row sits above the pool's byte figure, which is what case 3's concurrent
admission measures. Its concurrency oracle is positive
and falsifiable: the competitor's admission is observed by the pool's own new
ledger row appearing while the deleting process is alive and the held tree is
provably still being deleted (a measured, decreasing, non-zero entry count under
the deleter's custody), and that admission must stay inside three times the same
admission measured on a separate pressure-free pool plus two seconds — an
in-lock deletion would delay it by the whole deletion and fail that control, and
the control's own numbers are recorded. Its diagnostic mode keeps explicit
accounting: a recoverable oracle failure marks that case `failed` and the
independent cases still run, an unclassified outcome (unexpected exception,
unreaped child, exhausted deadline) marks the case `aborted` and the rest
`unexecuted`, and a case that never ran is never a pass. Staging drops Cargo's
encoded forms (including `CARGO_ENCODED_RUSTFLAGS`) and fixes every flag it
sets.

The proof's throughput figures are observations of a 20,000-entry fixture, not a
real pool's rate; the retired path's incident figure is recorded as a labelled
historical reference, never as a live comparison. This is still not a latency
guarantee: a short rename and one synced ledger write can take time on a slow
volume.


Current pools are budget-only (`directory-budget-v1`, `apfs-volume-v1`):
the budget is a soft admission figure with no OS ceiling, and current
budget, watchdog and removable behaviour is owned by
[workflow](workflow.md#budget-only-default-pool-and-user-configuration).
Admission uses this admission's budget (`LeaseRequest.budget_bytes`, else
the recorded one) and the [storage inventory](storage-inventory.md)
unique-allocated figure for owned slots, a `(device, inode)` union so a hard
link counts once; filesystem metadata directories remain outside that slot
accounting. Control/ledger headroom stays outside the payload and is bounded
(`MAX_LEDGER_BYTES` 8 MiB, `MAX_SLOTS` 256). Eviction happens only at lease
time and only for known owned idle whole slots, in the pool's recorded
least-recently-used order (oxide#118, [below](#slot-headroom-and-ranked-idle-replacement-oxide118)). Held occupancy, foreign
entries, symlinked pool/payload/slots/slot roots, unknown accounting and
mismatched backing refuse. During a durably recorded whole-slot deletion, an
owned nested symlink entry may be unlinked as a leaf after its parent and
entry ownership are verified. The target is never followed or removed.
Deletion never touches project or workspace paths. A deletion restores owner read, write and search on each owned real directory it removes, so read-only directories a build or test left inside a slot (such as Effigy's `.effigy/state/objects/<hash>` under a slot's child scratch) cannot leave a recorded deletion unfinishable; foreign directories and symlinks still refuse, and a symlink is unlinked as a leaf, never followed. A lost delete is
reconciled only for the recorded evicting slot path.

An incomplete or unverifiable pending eviction remains isolated from reuse,
adoption and deletion. For an existing journal, recorded directory modes are
restored through identity-verified descriptors around the completeness
preflight; a failed preflight retains the journal and deletes nothing. An
unjournaled root receives no mode change or capture before a complete
preflight. Its admission charge is the maximum of its recorded
size and the largest recorded slot (these budget-only pools have no per-slot
byte ceiling). No observed-byte term is used: pathname inventory does not hold
a no-follow root for a descriptor-relative walk. Arbitrary replacement of
that root is outside this accounting guarantee: the byte budget is soft and
may overshoot the charge. The existing registered reserve and running-build
reserve watchdog remain the free-space protection.

**An unmeasured idle slot at admission is charged, not refused (oxide#186).**
When an admission walks an idle slot that has no current record and that walk
stops partial — an elapsed deadline, an entry, depth or mount bound, an
unreadable entry — the slot is charged
`max(the bytes it observed before it stopped, its current-generation record if
any, the largest recorded slot in the pool)`, at least one byte and never the
whole budget. It is **kept**: the charge lives only in that admission, no
record is published, and the slot is never a victim, never reused and never
adopted on that walk. That holds for a **named count replacement** too: the
front door names a victim by recorded rank and the value decides, not the name,
so a slot whose own walk was partial is never retired. Substituting an unrelated
slot would delete storage the caller did not choose, and admitting without the
replacement would break the slot-count decision the caller asked to keep
(`PROJECT_SLOT_CAP`), so that admission alone refuses — `full`, bounded, naming
the slot — and nothing is deleted or allocated. The
admission otherwise continues under the charge and refuses only itself, with a
bounded capacity reason that names the slot (`full`, at most eight IDs then a
count), when the pool genuinely cannot fit; a charge that
alone exhausts the budget refuses before deleting anything.

A **count** decision follows the same rule. The pool-wide slot ceiling
(`MAX_SLOTS`) is met only by retiring a verified idle whole slot, so when the
admission has charged a slot conservatively and no verified victim is left,
the ceiling cannot be met: that admission alone refuses with the same bounded
reason, naming the unmeasured slot and carrying no byte figures (a count
decision has none), while nothing is deleted or allocated. Whenever every slot
was measured, the shipped `LedgerExhausted` refusal stands unchanged. This removes a
refusal and narrows eviction eligibility: it adds **no** deletion path or
authority. Everything shared still refuses globally — the ledger, config,
backing and fence, the pool scope pass, a slot root that cannot even be
opened, a held slot's live figure, the reuse candidate's own required-complete
walk, and a removable pool's bounded walks (its unknown-size reserve route is
unchanged).

`status` reports such a slot as **`not measured (walk incomplete)`** and never
with a figure. The front door records only the incomplete outcome, as one
diagnostic fact in the oxide#128 sidecar bound to the walked generation; the
charge itself is never persisted, the ledger total stays a stated lower bound
over the slot, and nothing derives admission, ownership or deletion authority
from the fact. A later complete walk of the same generation supersedes it
normally: a release writes the ledger record (which status reports first) and a
reuse walk writes its own fact. The same fact is recorded when a
`capacity_charged` refusal is what stopped the admission, so the slot that
blocked it is visible afterwards.

**A command's child temporary directory is inside its slot (oxide#136).**
`<slot>/scratch` is an ordinary owned entry of the slot tree, so the same
unique-allocated `(device, inode)` union counts it once with the rest of the
slot, its capacity walk covers it, and whole-slot eviction, crash recovery
and lost-volume retirement remove or keep it with the slot they already own.
Its own ownership marker (pool store identity, slot, incarnation) says whose
directory it is; it is never a closure proof. The one earlier removal is at
release, after positive writer closure — and a `run` program that uses an
owned directory keeps the lease until its own group is empty precisely so
that no competing lease can reconcile it while it writes. An entry the marker
does not claim is kept and reported, never deleted, and a removal that cannot
be completed keeps the slot held rather than marking it idle. An inherited
`TMPDIR` is outside Oxide's ownership entirely: it is neither accounted as
the pool's nor removed.

**Accounting walks (oxide#108).** Every pool accounting walk uses the same
pool-specific bounds: 2,000,000 entries, depth 64 below the walked root and a
60 s cooperative deadline. The whole-payload walk starts at the payload root,
two levels above the slot content directories (`slots` and a project folder,
oxide#104), so it uses depth 66 and slot contents keep 64
([Finder metadata](workflow.md#finder-metadata-in-pools), oxide#105). They are separate from the inventory tool's
defaults, which are unchanged. The callers:

| Walk | Gate |
| --- | --- |
| The matching idle slot's own walk before reuse | Must be complete, or the lease refuses |
| The one slot a release or recovery closes (oxide#146) | Best effort: an incomplete walk publishes no record and never fails the release |
| A slot with no current record, and every held slot, at admission | An idle slot on the internal directory pool that stops partial is charged conservatively for that admission and kept, never a victim (oxide#186); a held slot's live figure, a removable pool's bounded walks and a root that cannot be opened must be complete, or the lease refuses (`unknown_accounting`) |
| `status --measure`: the whole payload and each slot | Observational; reports coverage, causes and time. Default `oxide status` walks nothing (oxide#128) |

Admission itself no longer walks the payload: it sums the closed per-slot
records plus one shallow scope pass over everything outside the slot roots
([recorded per-slot accounting](#recorded-per-slot-accounting-oxide146-implemented-native-held)).
When every record is current that is O(slots) metadata work and **zero
inventory walks**; only a slot without a current record is walked, once, under
the same bounds as any slot walk and one cumulative 60 s admission deadline.

**Removable unknown-size route (2026-10-25, superseding the volume-used
fallback).** A removable `apfs-volume-v1` payload bounds the walks its own
figure needs with `VOLUME_WALK_MS` (5 s) for a new slot or a count
replacement. When those walks are incomplete with **only** `elapsed_deadline`
or `entry_limit` gaps, the pool size is genuinely unknown and the reserve
route decides — never the volume's used bytes, and never a fixed eviction
count:

- free space **above** the registered reserve **and** at or above the 10 GiB
  hard floor: admit with one bounded note, nothing evicted, cap unverified;
- otherwise, while an eligible idle whole slot remains: retire the
  least-recently-used one whole, re-verify the binding and re-read free space,
  and repeat (bounded by the remaining eligible slots and the caller's
  deadline);
- with nothing left to prune: free space at or above the floor admits with a
  warning naming the free, reserve and floor figures, and below the floor it
  refuses with the original incomplete scan.

The floor wins over a lower legacy reserve, so nothing admits below it merely
because free space is above the reserve. Any other gap (unreadable, missing or
unusable metadata, mount boundary, depth limit, root problems), every
incomplete pool figure on the internal `directory-budget-v1` pool, and every
mixed or non-bound gap refuses before anything is deleted. The reason the
bound exists at all is unchanged: a 127-slot USB pool could not be walked
within 60 s. Recorded per-slot accounting ([below](#recorded-per-slot-accounting-oxide146-implemented-native-held))
means this route is reached only while some slot still lacks a current record.

**Count limits are count pressure.** A per-project or pool slot-cap
replacement may proceed when the pool figure is complete **or** when a
removable pool's figure is unknown for bound gaps only; an internal, mixed or
non-bound incompleteness refuses before any deletion. Count replacement
authorisation never leaks into a byte-pressure decision.

**The admission figure (oxide#146).** `Pool::recorded_total()` is
`pool_scope()` plus one figure per ledger slot: a held slot is always walked
fresh, a current idle record is used as it stands, and an idle slot with no
current record is walked once and its figure published in one revised write
*before* the admission decision, so progress survives a refusal. `pool_scope()`
is a separate walker (`inventory::scope_directory`) that hard-stops at every
recorded slot root — by ledger membership, for both the legacy `<id>` and the
labelled `<folder>/<profile>-<id>` layouts — so each regular file is counted
exactly once and out-of-slot metadata, strays and orphan regions are counted
without granting ownership. Its debug-only observation is
`OXIDE_TEST_SCOPE_LOG`, separate from the walk log, so "zero inventory walks"
stays a checkable claim. Cross-slot hard links are counted once per slot: that
overcount is accepted, so the sum is never below the inode union, equality
holds for disjoint slots, and an idle slot may be evicted earlier than the
union alone would require. A current record is a measurement at a closing
transition, not proof of immutable bytes, writer closure or volume closure, and
it is trusted until its generation changes or a held/recycle/evicting
transition clears it. The pressure figure is that sum for **both** backings:
a shared removable volume's other occupants are not this pool's usage, so they
never move a pool toward its cap and never evict its idle slots. Free space on
a removable volume is a separate reserve signal; the below-reserve *selection*
fallback and the running-build reserve watchdog keep their current behaviour,
and widening them is the approved follow-up rather than part of this change.

**How one walk reads (oxide#141).** The walk itself is unchanged in scope,
bounds, policy and refusal classes. Each enumerated regular file or directory
is read immediately from the live directory entry the walk holds, when its
absolute path name is shorter than the platform `PATH_MAX` including its
terminating NUL, and with the former full-path no-follow read when it is not;
directories are still read by path name, so no long-path completeness is
claimed. Every directory that is read is also sampled by path name before its
entries are read and again after they were enumerated, and must still be the
directory observed when it was classified (kind, root device, observed inode).
A mismatch is `unsupported_metadata` (another kind or inode), `mount_boundary`
(another device) or, for a non-root `NotFound`, one `vanished` observation; a
directory that fails its sample is not read, and after enumeration only its
newly queued children are discarded while observed entries and bytes are kept.
`ENOTDIR` and `ELOOP` are never `vanished`. The samples do not bind the
directory `read_dir` opened and are not an atomic snapshot, and the extra two
path-name reads per directory cost real time. This removes some redundant
ancestor path resolution; it is not a measured speed claim and not a guarantee
that a large or cold pool meets the 60 s deadline, so a walk that reaches the
deadline still refuses exactly as before. The inventory's own
[per-entry and boundary-sample rules](storage-inventory.md#per-entry-reads-and-directory-boundary-samples-oxide141)
own the details. Admission walk counts are unchanged.

A walk that reaches a bound, meets an unreadable entry, missing or unusable
metadata, a directory whose path-name identity changed at either of its two
boundary samples, or a mount boundary, or cannot open its root is incomplete.
It refuses with reason `unknown_accounting` (exit 3, unchanged), and the error
names the gap kinds (`entry_limit`, `depth_limit`, `elapsed_deadline`,
`mount_boundary`, `unreadable_entry`, `missing_metadata`,
`unsupported_metadata`, `root_missing`, `root_not_directory`,
`root_unreadable`, `root_symlink`) with counts, entries visited, elapsed
milliseconds and the bounds used. No path or name is printed. An incomplete
walk's bytes are never used for admission and never make a slot an eviction
candidate; `status` shows them as a lower bound. A measured zero stays
distinct from unknown.

**Enumerated paths that vanish (oxide#113).** If a non-root path enumerated by
the same walk returns `NotFound` at deferred `read_dir`, `file_type` or
no-follow metadata, the walk increments a separate `vanished` count rather
than a gap. A walk with only these observations is complete for the names it
could still read, and its bytes may be used by the existing admission and
whole-idle-slot eviction rules. The scan and its result remain a best-effort,
non-atomic path observation; `vanished` does not prove that inode allocation
was physically freed or is reclaimable. Bytes for other paths already
observed, including a link to the same inode, remain counted. A root
`NotFound` is still `root_missing` and refuses even if that root was opened
earlier in the walk. Iterator errors without a path identity, `EACCES`, other
metadata errors, bounds, mount boundaries and all ownership, foreign-entry,
symlink and identity refusals retain their existing categories. `status` and
`unknown_accounting` refusal text show `vanished xN` even alongside a real
gap, and state that allocation reclamation is not inferred. The count is not
part of ledger/config persistence; new readers accept
older scan JSON with the field absent, while older strict readers may reject a
new scan that includes a nonzero count.

Only a slot whose own walk completes can be reused; other
`unknown_accounting` refusals (invalid identities, ledger, ownership or
backing failures, foreign entries) are unchanged.

The bounds are finite, not an arbitrary-size guarantee. The entry bound
covers about 7,800 entries in each of a full 256-slot pool, not the
60,657-entry single-project slot reported for Farmyard (2026-10-20); a pool
beyond it refuses until idle slots are evicted or cleaned. A walk over a slow
or cold volume can reach the deadline;
`status` reports each walk's elapsed time. One admission may walk the payload
more than once (the front door's `status`, then the library's admission
walk), so its cost grows with pool size. The 50,000-entry, depth-24, 5 s
bounds before oxide#108 refused real build caches (Praxis's 76,669-entry and
Farmyard's 60,657-entry trees, Supervisor report), including reuse of an
existing slot above 50,000 entries.

Before leasing a compatible idle slot, the library inventories that exact
slot's complete unique allocated bytes and keeps the warm generation; the
half-budget recycling rule is retired (oxide#118). Incomplete or unknown
accounting refuses the lease. A held same-holder replay keeps its exact lease
and path, and another holder remains busy without touching the held slot.
Byte pressure is handled by admitting a new slot, not by discarding a
compatible warm one: with the attributable pool figure — the recorded per-slot
sum plus the shallow scope pass, never the shared volume's total — at or above
the budget, the oldest eligible idle whole slot is removed until that figure is
under it, and `Full` refuses when only held slots remain.


### Ledger-only view for front-door membership (oxide#110, lead `e4c74399`)

Implemented as a routine correction with no admission, closure, recovery or deletion change. The design passed independent prospective review at `edc4f54`. Its focused native proof `rust:front-door-accounting` passed once at source `38f79ac` (host run `9d11cd31`, exit 0, 169 s wall; script 168 s including cleanup, sampled peak scratch 125 MiB) under the old `scripts/bounded-test.py` exception route. The Effigy client lost the scheduler (`scheduler_unreachable`, exit 75) and truncated the stream; the full output was recovered from the scheduler afterwards. An earlier submission was cancelled before launch.

**Problem it removed.** The library's `status` runs one whole-payload walk plus one walk per slot. Until oxide#110 the front door called it on ordinary commands where it needed only ledger membership. From source at `3842239`:

| Call site | What it used from status | Now |
| --- | --- | --- |
| `Store::open_at` (internal pool, every command) | `backend` | ledger view |
| `Store::open_removable` (selection through `usable`, every command while registered) | `backend` | ledger view |
| `Store::attempt` (acquisition affinity) | `slots[].project` | ledger view |
| `Store::recover_settled` | held `slots[]` by project | ledger view |
| `Store::recover_interrupted` | held `slots[]`: `slot_id`, `lease_id`, project | ledger view |
| `removable::internal_holds` | held `slots[]` by ID | ledger view |
| `Store::payload_use` (the per-command result line) | `payload_unique_allocated_bytes`, `cap_bytes` | removed; the lease's own reuse walk |

On a removable pool, one ordinary command paid several full walks, and a waiting command paid them on every 200 ms poll. At 201,948 entries one walk took 3.4 s.

**Ledger view.** `managed::ledger_view(pool) -> LedgerView`:
- **Opening:** the unchanged `Pool::open`: canonical root, foreign-entry and retired-image refusals, pool lock, config and backing verification, ledger identity checks, the slot limit and `reconcile_evicting`. The view is therefore **not side-effect-free**: recorded-deletion reconciliation runs exactly as for `status`.
- **Per-record checks:** under the same pool lock and in ledger order, it keeps status's cheap, non-recursive checks with the same errors, through one shared helper: `slot_path` refuses a zero slot ID (`unknown_accounting`), and a slot-root `symlink_metadata` error other than `NotFound` propagates (`io`). A record whose root is missing, held or idle, stays a member: no closure is inferred and no refusal is added. A symlinked root or partial accounting stays non-gating, as in status.
- **Returns:** `store_id`, `backend`, `cap_bytes`, `reconciliation_blocked` and each slot's `slot_id`, `incarnation`, occupancy, `project`, `workspace`, `profile`, `holder` and `lease_id`, the same values status shows. The type has **no byte or coverage fields**, so a skipped walk cannot read as zero or complete. Selection, affinity, wait and recovery decisions, record-lock probes, process-group checks, retirement checks and admission-lock order are unchanged.

**Full accounting stays** for `oxide status --measure` (oxide#128: the
library's passive `measure`, the same payload and per-slot walks made without
the pool lock or reconciliation) and for every library admission, reuse,
recycling and eviction walk, with unchanged bounds. `oxide init`/`new`
readiness no longer makes one: since oxide#182 it reads no pool state at all
and reports `use not measured (readiness reads no pool state)`. Default
`oxide status` no longer walks, opens the pool lock or reconciles:
it reads the [passive snapshot](#passive-snapshot-and-measurement-oxide128).

**Result line.** It walks nothing. `managed::lease_measured` returns the grant and, only when the library's reuse walk took an idle slot, that walk's unique allocated bytes with the exact slot ID and incarnation. The line shows that figure with its provenance, `slot 5 was 7900 MiB when leased`; `new slot` after allocation or recycling; `slot size unknown` for a held same-holder replay, where `reused_slot` is true but nothing was measured. It then shows this command's soft budget and `pool total: see oxide status`. Nothing walks a slot after release, when another command may hold it.

**Effect.** An ordinary reuse command walks only its target slot, the library's existing reuse walk. Since recorded per-slot accounting (oxide#146) a new-slot admission walks only the slots whose records are missing or stale, over one shallow payload scope pass, so an all-recorded admission walks no inventory at all; the release then walks the slot it closed, which is the one walk an ordinary new-slot command makes (oxide#177). Waits and recovery walk nothing, and the recovered command's own re-lease walk is the only extra one.

**Observation hook.** Debug builds only, absent from release builds such as the installed `oxide`: `inventory::walk_observer` records the root of every `inventory_directory` walk, the one entry point of library accounting, `status --measure`, Cargo-home measurement and the stray check, on a per-thread recorder for tests and, when `OXIDE_TEST_WALK_LOG` names an absolute file, one line per walk. A shallow scope pass (`scope_directory`, the recorded-accounting admission basis since oxide#146) is recorded separately on `OXIDE_TEST_SCOPE_LOG`, so "no payload walk happened" stays checkable while the scope pass stays observable. It changes no walk. Walk counts show routing only; they are not a resource gate. The same debug-only hook can inject a scoped fixture fault for a proof: a one-shot `delay_before_read_dir` of 1..=65,000 ms (aggregate-bounded, marked fired before it sleeps and reported by a debug marker so a proof can prove activation). Injections are gated on the guard's own canonical `/private/tmp` scratch and evidence variables plus `OXIDE_PROOF_DEADLINE_NS`; a malformed, misapplied or out-of-scope injection installs nothing, and release builds have no hook.

**Proof.** Standard tier. Library tests show that the view equals status's membership field for field with no walk while `managed::status` still walks the payload and every slot; that reuse walks only its slot and returns that measurement, a held replay walks and measures nothing, and an all-recorded admission walks no inventory while the release walks the slot it closed (`release_records_the_slot_and_all_recorded_admission_walks_nothing`, `missing_records_walk_only_the_unrecorded_slot_and_publish_progress`); that a zero slot ID, a slot-root lookup error and a foreign pool entry refuse the view and status alike; and that a missing held root stays a member while partial accounting and a symlinked root stay non-gating and reuse still refuses incomplete accounting. The unreadable-walk and slot-root refusal cases inject `io::ErrorKind::PermissionDenied` at exact canonical-path `read_dir` and `slot_root` `symlink_metadata` calls with a current-test-thread `cfg(test)` guard and hit assertions. These cases preserve the library's gap and refusal mapping, but do not prove kernel permission enforcement or an operating-system `EACCES` response. Front-door recovery rules take the view's slot type unchanged. The native proof `effigy rust:front-door-accounting` covers, on one private pool with an idle slot of more than 200,000 entries, the walk contract in force (#146, #186, #190): a new slot makes no admission walk and one shallow payload scope pass and its release walks only the closed slot; ordinary reuse walks exactly its target slot at admission and again at release; default `status` walks nothing while `status --measure` walks the payload and then every slot; `oxide new` readiness walks nothing and makes no scope pass; a live-holder and killed-holder `recover-interrupted` refusal makes no walk; a waiting build walks nothing, then re-leases the recovered slot (one walk), has the freshness gate refuse it, evicts the untrusted slot whole and builds cold. Its byte-pressure eviction case was retired to `rust:recorded-slot-accounting` case D and `rust:holding-delete`; its hard-link half was not covered there — D evicts independent slots and E is a cross-slot overcount — so the same-slot retention control is restored natively here as its own case on its own pool: one retained slot holds an 8 MiB file with four names for one inode, its release publishes the inode union (about 11.1 MiB) and not the per-name sum (about 36.3 MiB), an owned 12 MiB filler slot supplies the pressure under the pool's documented 16 MiB `OXIDE_POOL_CAP`, and an admission over that cap evicts the filler slot whole (one eviction per attempt, deleted outside the pool lock, then retried), keeps the retained slot, its directory and all four names, and admits a new slot; per-name accounting would refuse the same admission. The case never plants a foreign entry in a payload root, which the product refuses by design. The pure self-check pins the admission oracle and rejects every counterfactual (a per-name record, a record or union below one file, a per-name sum equal to the union, a cap above the per-name sum, a cap below the union and a filler too small for pressure), requires the case to use `OXIDE_POOL_CAP`, forbids any payload-root foreign entry or debug budget override, and keeps its eviction and retention assertions. Writer closure is never something the script can conclude: once any `oxide` child has been launched, every later failure in any case (including the independent hard-link case) and a Cargo group that still has a member after its wait are unknown writer outcomes — the collection aborts, no later case runs, the receipt keeps the fault and every remaining case `unexecuted`, and the scratch is retained for the guard's later certain census; only a failure before the first launch may be recoverable, and the pure self-check asserts that continuation only at a zero launch count. The successful path never removes the run root or the staging tree either: both stay inside the guard's owned root for the post-leader census and the guard's retention policy owns cleanup, which the pure check pins by the absent `rmtree`, a retained root after a successful run, and — through the `run_staging` seam, without launching a writer — a `stage` that keeps its target tree and copied binary. Removable selection is covered by the shared refusal tests only: no native session attaches a volume. **Run history, preserved.** The proof passed once at `38f79ac` under the pre-#146 contract (host run `9d11cd31`, 169 s wall, sampled peak scratch 125 MiB), with the Effigy client losing the scheduler (`scheduler_unreachable`, exit 75) so the output was recovered from the scheduler. The 2026-10-30 QA baseline at `1d6312d` found it red on the retired whole-payload admission oracle (`new slot: walks ['slots/1'], expected ['payload']`), not a product regression. oxide#191 refreshed every walk oracle, added the scope-log channel, a `--self-check` (with the pure `effigy proof:front-door:test` selector and an effect-free `proof:front-door:plan`), and retired the eviction case to its owners. Three further counted runs spent on oracle corrections that the self-check now pins: the measure audit also walks the checkout's target and Cargo home (not pool roots), and a same-checkout build with a busy record now takes a second slot instead of waiting, so the waiting fact uses one host-wide slot and the recovery fact follows a positively empty Cargo group. The counted author native then passed at `4ca12e0` (40 s including cleanup, sampled peak owned scratch 281 MiB of 512 MiB, every walk oracle as above, and a complete 200,073-entry `status --measure` after recovery), but its fixture still lived in its own `/private/tmp` scratch. **Exact-head review blocked on that**: the proof now runs as the whole command of the guard with `--root-parent /private/tmp` (the owner-accepted internal-placement exception `bd357a4c`), creates everything inside `OXIDE_TASK_SCRATCH_ROOT`, stops 30 s before the guard deadline, and adds the fixed case map, the uncounted `--diagnostic` keep-going collection (`effigy rust:front-door-accounting:diagnostic`) and the nine premise dispositions the pure self-check binds to its actual staging and fixture environments (private HOME and the invoking rustup home, `OXIDE_TEST_HOME_FROM_ENV=1`, the isolation scrub, the frozen `RUSTFLAGS` with no encoded value, and `TMPDIR`/`TMP`/`TEMP` on one owned directory). That diagnostic completed 5/5 cases at `dcf1a0d` (47 s, guard-monitored owned peak 179.3 MiB of 512 MiB, closed, no deadline) and the counted author native passed at `558469a` (51 s including cleanup, guarded owned peak 182.7 MiB of 512 MiB, host free minimum 2195.6 GiB, closed/status 0/no deadline). The earlier `4ca12e0` pass stays history at the script that ran it. **A second exact-head review found two source defects, fixed without running either selector.** A child that does not finish in its bound is an `UnknownOutcome` — `communicate_bounded`, `wait_child`, `wait_for`, `wait_text` and `stage` signal and reap what they can and then raise `uncertain_child`, because this script cannot conclude a descendant group's closure; the diagnostic therefore aborts and records `aborted` with the remaining cases `unexecuted` instead of treating the timeout as a recoverable oracle failure, and its single bounded receipt is written in every outcome including a failure before the first case ran — through `finish_run`, which the pure check drives end to end through `main`, so a successful diagnostic whose receipt cannot be written raises instead of returning 0 while a failing run keeps its own error. And `staging_environment` now drops every inherited `CARGO_*`/`SCCACHE_*` value, restoring only the invoking `CARGO_HOME` explicitly (set by `invoking_cargo_home()`; the account directory this repository's committed lock resolves its offline registry cache in) — the pure self-check pins that, the timeout classification at each waiting site, and the abort/fault/unexecuted receipt semantics. The `dcf1a0d` diagnostic and the `558469a`/`a0804fa` counted natives stay historical at those heads; the corrected head needs a fresh diagnostic and counted native under separate owner activation. A further correction binds the run to its source before any effect, exactly like the removal proof: `resolve_source` requires both git calls to exit 0, an exact 40-hex commit identity and a clean tracked worktree, and a failing or interrupted call, a nonzero exit (an empty stdout from a failed status is never a clean worktree), an empty or non-exact identity and a dirty tracked worktree abort inside the receipt path with the fault and all six cases `unexecuted` (the five walk/recovery cases plus the hard-link retention case), before any scratch, staging, child or fixture exists; the pure self-check covers each branch and the no-effect outcome.

### Passive snapshot and measurement (oxide#128)

Default `oxide status` must stay fast and read-only on a large pool, so it
does not call `status`, `ledger_view` or `Store::open`, all of which take the
pool lock and run recorded-deletion reconciliation. `managed::snapshot(pool)`
is a separate read:

- **Opening:** canonical owner-only pool root, the unchanged foreign-entry and
  retired-image refusals, then config and ledger reads. It creates no lock,
  takes no lock, waits for no writer and writes nothing, so a snapshot
  returns while another command holds the pool lock.
- **Validation shared with `Pool::open`:** backend shape, config/ledger
  identity, the slot ceiling, and every record's ID, incarnation and recorded
  directory. A directory pool's payload root must still be the real private
  payload directory on the pool's device; a removable pool's recorded volume
  is reported as recorded and never probed, so an absent volume still yields
  its ledger.
- **Torn reads:** each file is replaced atomically, so one snapshot may
  straddle a concurrent replacement. A mismatched config/ledger pair is read
  again once; a second mismatch refuses as `backing_mismatch`. It is never
  repaired and never waits.
- **Returns:** `store_id`, backend, recorded cap, the recorded payload root,
  the recorded volume binding, ledger membership in ledger order, and the
  slot IDs recorded `Evicting`. A pending deletion is reported as pending
  reconciliation, distinct from idle and held; nothing infers writer closure
  or current eligibility.

`managed::measure(pool)` is the diagnostic measurement: the same recorded
snapshot plus one whole-payload walk and one walk per slot, using the
existing entry and depth bounds. The caller's remaining share of one
cumulative command budget becomes a single deadline (clamped to the
accounting bound) that every walk recomputes against, so the payload walk and
all slot walks stay inside it rather than each starting a fresh bound; past
the deadline a walk keeps a 1 ms bound and reports an elapsed-deadline gap. It
takes no pool lock, creates none, reconciles nothing and writes nothing, so a
recorded `Evicting` slot is walked as it stands and makes coverage unknown.
After walking it re-reads the pair once; if any slot's ID, incarnation,
directory or occupancy changed, coverage is unknown rather than pairing a
walk with a different generation. Neither entry point persists anything.

The front door records diagnostic facts from walks an acquiring command
already makes, in
`<pool>/oxide-managed/status-observations.json` (schema
`oxide.status-observations/1`, ≤512 KiB, 256 retained recent facts, each
bound to store ID, slot ID, incarnation, exact recorded directory, wall-clock
time, allocated bytes, source and measuring lease). There are two sources: the
reuse walk (`lease-reuse`, with its measured figure) and an admission walk that
stopped partial (`admission-walk-incomplete`, which carries **no** figure — its
`allocated_bytes` is written as 0 and read by nothing, and it reads as
`not measured (walk incomplete)`; oxide#186). The sidecar is an
auxiliary diagnostic inside an already-tolerated directory: it bumps no core
format, an older binary may ignore it (or refuse its unknown source and report
every fact as not measured, which never claims a size), and a write failure
changes no admission, ownership or exit status. A missing, unreadable, foreign,
corrupt, unknown-version or mismatched sidecar reads as not measured, never as
a fallback scan. Default status never creates or refreshes it and `--measure`
never persists.

### Slot headroom and ranked idle replacement (oxide#118)

Implemented from the approved Queue briefs (Tom, 2026-10-26: "Oh yeah, raise
the slots!", then "Ah yeah, switch to your solution"; Tom, 2026-10-28 raised
the ceiling in oxide#177: 256 pool slots and twelve per project). Slot headroom
only: `MAX_SLOTS` is 256 and Oxide's project cap is twelve; neither build
concurrency nor `max_active` changed, there is no new configuration, and the
soft byte budget and the 95 GB registered reserve are unchanged.

**Representation.** `MAX_LEDGER_BYTES` is 8 MiB. Every bounded field keeps its
cap: paths 1024 bytes, holder 128, lease ID 160, slot directory component 80.
A compact JSON ledger with every field at its ceiling, escaped sixfold and with
20-digit integers, is proven by unit tests to fit the cap and round-trip.
`MAX_LEDGER_BYTES` is the **acceptance** bound: the read and write paths refuse
a larger ledger, so every accepted ledger is at most 8 MiB.

The measured state the current writer can reach from an empty ledger is
serialized at every bounded maximum: the all-idle 256-slot ledger (numeric
fields at their full 20-digit width including `cap_bytes`, every string at its
cap including the widest accepted `lease_id`) measures 4,213,726 bytes. That is
the figure this writer's ordinary eviction path reaches: since oxide#190 it
writes one holding row and two O(1) synced writes per eviction and starts no
journal at all. A `DeletionJournal` is now only **legacy input**: a ledger an
older writer left can still carry one — with **one** maximal journal (96 capture
records and 32 mismatched captures, chains of 64 capture private names, name
fields at `PATH_MAX` 1024 bytes in `NAME_MAX` 255-byte components,
`expected_mode` at the unrestricted u32 maximum, maximal sixfold escapes) it
measures 5,625,456 bytes on an `Evicting` record that retains its closed
observation, and the reader and the replay both accept it. The writer refuses
an over-bound journal object per object and puts it back without deleting it;
the reader refuses a ledger that carries one. Ordinary evictions go through
`retire_idle_slot`, which refuses while reconciliation is blocked or a legacy
journal is still outstanding (so the guard covers `evict_idle_slot` too), and
every open reconciles the holding area and then all `Evicting` records before
admitting anything.

**Recovery moves trees; it never starts a journal.** Every `Pool::open`
reconciles the holding area first (`reconcile_holding`: adoption only against a
recorded intent-verified identity, rows dropped only when positively absent) and
then the `Evicting` records. For a legacy `Evicting` record,
`reconcile_slot_capture` resolves the #155 journal by #155's own rules —
`Concluded` drops the record, `Resume` continues only after the recorded root is
restored at its recorded name, `Blocked` keeps the record and starts nothing. A
`Resume` then verifies the restored root in O(1) with `pending_root_verified`
and moves it whole into the holding area, exactly as an ordinary eviction does.
No path in this binary creates or persists a `SlotCaptureJournal`: the
per-entry capture, journal start and in-lock per-entry removal, and the
recovery functions that started a journal (`remove_owned_slot`, `begin`,
`pending_root_ready`), no longer exist, and a blocked or unclassifiable pending
root keeps its record instead of starting one. The front door's admission gate
and the library's own locked open make the version-9 control fence (which
carries `oxide.holding/1`) durable before any reconciliation (see the fence
paragraph), so recovery, holding moves and ledger writes are all preceded by
the hand-off fence.

**Worst-valid byte bound (oxide#177; Supervisor ruling 49081d79).**
`MAX_LEDGER_BYTES` *is* the worst-valid byte bound: the read and write paths
refuse a larger ledger, so a larger valid document is impossible by definition
and any otherwise-valid ledger at or below 8 MiB is accepted, including one
that carries more than one `Evicting` journal (each slot's journal is validated
independently and no journal count is bounded). No count rule is added and the
cap does not grow for multi-journal states.

The measured *ordinary-path writer-reachable* maxima are reported separately:
the all-idle figure above, and 5,625,456 bytes with this writer's maximal single
journal on an `Evicting` record that retains its closed observation, leaving
**2,763,152 bytes headroom** under the cap. An earlier write can still have
left more than one journal in a pool, and the reader accepts any
otherwise-valid ledger at or below this cap — a write above 8 MiB is refused —
so the 5,625,456-byte state is what this writer reaches on its ordinary path. The journal name fields keep their enforced `PATH_MAX` 1024 bytes /
`NAME_MAX` 255-byte component bounds at writer and reader. Control
state is bounded
separately: 512 records (one per slot with the previous 2× headroom), 128 wait
tickets, 128 clean marks, 16 recoveries, 16 flushes, per-object compact
ceilings (record 40 KiB, recovery 9 KiB, holder 5 KiB, lost 8 KiB, flush
8 KiB, clean 7 KiB, ticket 128 B, state header 4 KiB) and a 32 MiB file,
whose gross object sum is 22,188,032 bytes. Wait tickets and clean marks stay
at 128 because they bound concurrent waiting commands, not slots. Ranks and
slot IDs keep their full `u64` headroom with checked exhaustion; the ceiling
raise changes no ID bound. The two representations are envelopes for valid
state, not a total control-directory bound: preexisting orphan origins and
crash-left `ledger.tmp-*` files stay outside them, and nothing here authorizes
cleanup.

**Permanent fences.** A legacy ledger (no `capacity_policy`/`next_rank`, rank
0) and a version-1 control state stay readable and are never migrated by
`status`. The first revised grant, release or recovery, or an explicit
eviction, installs the library ledger's `capacity_policy: 2` fence (the
recorded-accounting representation, oxide#146) and rank sequence atomically
with that mutation; the Store-controlled mutation's own recorded-accounting
control fence is already durable first (see
[recorded per-slot accounting](#recorded-per-slot-accounting-oxide146-implemented-native-held)).
A slot table above the legacy 128 carries `capacity_policy: 4` (oxide#177),
written forward-only, in the same atomic replacement that first records the
129th slot. An older binary's policy check accepts only 1–3 (1 and 2 from
oxide#146, 3 from oxide#175), so it refuses a `4` ledger with
`unknown_accounting` (exit 3) before any mutation; a ledger above 128 slots
without the fence is a state no writer produces and refuses too. A pool that
stays at or below 128 slots never carries `4`, so its ledger stays readable by
an older binary. The ledger alone cannot order that switch against the
[#122 hand-off](workflow.md#installed-binary-hand-off-oxide122), which reads
control state rather than the ledger, so the mutating-command admission gate
makes a forward-only **version-9** control fence durable as the command's
first write (the oxide#177 `oxide.slots/256` fence, beside the oxide#175
eviction-accounting, oxide#146 recorded-accounting and oxide#190
`oxide.holding/1` fences; Oxide Supervisor
ruling 4c9984a1, fence first). The gate runs under the admission lock and
before any locked `Pool::open` (which reconciles the holding area and recorded
deletions and writes the ledger), so every such effect — ordinary eviction,
recovery, a holding move and ledger write — is preceded by the
durable fence. A locked `Pool::open` also raises the same fence itself, under
the pool lock, before its first reconciliation: pre-Store entries
(`Store::open_at`, `Store::open_removable` and `recover-interrupted`'s ledger
read) and a direct library open therefore cannot write to a holding-era pool
before it.
That library path writes only the version and the three fence fields, taking
the admission lock non-blocking and refusing rather than reconciling under
contention; an absent control state (a library fixture) and passive
`status`/snapshot readers are never fenced. A present but non-string or
non-matching fence value (explicit JSON `null` included), a version/fence combination no front door writer
produces (including a version-6 document without its one-use fence), or an
unsupported version such as 0, refuses without repair: only a fence field
genuinely absent from an older document is inserted. The fence path and the
front door's `Store::load` share one parse-and-validate function
(`lease::control::parse_state`), so a structurally malformed document (for
example a non-array `records` field) is refused identically by both. An older binary with version ≤ 7 then hands off to a
receipt-matching successor or refuses before it reads the ledger, so no
predecessor ledger-size argument is needed. An already-version-8 control is
not rewritten; a failed fence write refuses the command with no ledger or
journal write. Passive `status` and snapshot readers never take this gate and
never fence, so only a pool this binary never mutates stays at version 5. A completed legacy `Evicting`
reconciliation may still finish in the legacy representation. Before a revised write, every
stored field is validated against the raised caps; an oversized legacy field
refuses with the original file, ownership and quarantine bytes untouched,
never truncated or dropped.

**Hand-off trigger.** The numeric `version` of a control `state.json` (the
internal pool's and the removable pool's) and of `removable.json` is also the
front-door hand-off trigger (oxide#122). A newer binary that writes version 3,
for example, lets an older binary hand off once to a receipt-matching
installed successor whose recorded capabilities cover that version, instead of
only refusing. Only a numeric `u32` above the running binary's version
participates; malformed, foreign, oversized, unreadable and differently-fenced
documents keep the existing refusal, and a future incompatible format must
bump its numeric version to participate. The hand-off itself is pre-effect
only: see [installed-binary hand-off](workflow.md#installed-binary-hand-off-oxide122).

**Ranked LRU.** Each pool keeps one monotonic idle-rank sequence. A new actual
grant and a first held-to-idle release or recovery stamp a fresh rank in the
same atomic ledger write; holder/grant replay and release/recovery replay do
not advance it. Legacy ranks are 0 and sort oldest. Eviction order is
ascending `(rank, numeric slot id)`. Counter exhaustion refuses before any
deletion or transition, including the explicit `evict_idle_slot` path used for
an untrusted or lost slot; there is no wrap, rebase or cross-pool comparison.
Store spawn touches stay separate freshness-origin marks and the wall-clock
`last_used_ms` remains an observation.

**Admission.** Every allocated ledger entry counts: held, idle,
retired-backed and unresolved `Evicting`. Compatible idle reuse stays first,
with its complete target walk. Below both count ceilings and without byte
pressure nothing is evicted, so the old 32/2 counts no longer churn slots.
At the pool ceiling, the least-recently-used *eligible* idle whole slot is
replaced and the new slot is built cold. That is **count** pressure, and it is
authorised separately from bytes: a complete pool figure or a removable pool's
bound-only unknown both suffice, while an internal, mixed or non-bound
incompleteness refuses before any deletion. A **recorded byte-pressure**
admission instead compares the attributable pool sum with the budget, uses the
global idle order, and recomputes the recorded figure after each deletion; a
removable pool whose own size is unknown takes the reserve/floor route and
never a byte-cap comparison. Each deletion must reduce membership and an admission is bounded by
the entries it saw initially. Held, retired, uncertain (live or unattributable
lock), foreign and unmeasurable slots are never eligible, and a safety failure
stops admission instead of skipping to another slot. With no safely eligible
idle slot, the lease refuses honestly; it does not promise a deletion through
a failed safety gate. At the twelve-slot project ceiling Oxide replaces that
project's oldest eligible idle whole slot, which also frees pool capacity.
The front door chooses that victim by recorded rank but asks the library to
retire it as part of the same admission: the count-pressure gate of the
previous paragraph and rank headroom are checked before any deletion, and a
refusal leaves the victim and every metadata record in place. The whole-payload
walk reported by `status --measure` remains an audit view, not admission
input.
A command waits only when a specific held compatible slot of its own checkout
exists under the existing identity and freshness rules; an unrelated build's
completion is not assumed to free a count or byte ceiling. The freshness loop
is bounded at thirteen grants: the twelve project slots a refusal can try plus
one cold fallback, with sharing off after the first refusal and the existing
discard/unshare before reacquisition.

**Proof and evidence.** `effigy rust:slot-headroom` under the qualified shared
supervisor (with `--root-parent /private/tmp`, the owner-accepted
internal-placement exception bd357a4c, because its fixtures create a genuine
internal directory-budget pool that the product correctly refuses on an
external volume) covers the fences, no-migration reads, rank exhaustion, a 256-slot
pool's 257th and 258th replacements, the twelve-slot project cap with a specific
held-slot wait and one LRU idle replacement, compatible reuse, no eviction
below both ceilings, byte-pressure LRU with a held slot excluded, and a
foreign payload-entry refusal. It also drives a project replacement whose
payload walk is incomplete (a depth-bound gap, with the victim deliberately
left unrecorded because recorded accounting trusts a current idle record) and
checks that the victim survives and membership is unchanged; every fixture child stays in the
qualified supervisor's session and the script starts no session or group of
its own, so the runner owns closure on any fault. With every slot's record
current it also checks oxide#177's accounting-cost gate: a new-slot admission
at 256 walks no inventory, the evicted slot is never walked, a release walks
only its own slot, and default `status` at 256 walks nothing. The maximal
accepted ledger serialization — the legacy one-journal `Evicting` state,
5,625,456 bytes with 2,763,152 bytes headroom under the 8 MiB
worst-valid byte bound, which since oxide#190 is only input an older writer
left (this writer starts no journal) — escaped round-trip and oversized-field refusals are
unit tests in `effigy test rust`, as is a
fixed 1000 ms ceiling on a whole 128-slot and 256-slot recorded-ledger round
trip with a journal (bounded read via the passive snapshot, ranked ordering,
the actual atomic write and its read-back; the ceiling is generous for the
included `fsync`, which is host I/O), reporting both measured figures. `effigy rust:slot-headroom:diagnostic` is the same fixed guard and
limits with `--diagnostic`: an uncounted keep-going collection that records
reached/completed/failed/skipped phases and the latched first fault in a
bounded `qualified: false` receipt, continues only into phases whose declared
dependencies completed, and aborts on an unknown outcome or an unclosed proof
child. `effigy proof:slot-headroom:test` runs only its pure `--self-check`
(no child, scratch, pool or evidence write). Not covered natively: the thirteen-grant freshness bound is a
bounded-loop inspection, and no large live inventory or physical
multi-million-entry performance is claimed. A counted author native passed
50/50 cases at `4b0a485` (#177).

### Recorded per-slot accounting (oxide#146: implemented, native held)

Tom's 2026-10-25 ruling, "We need a much faster way to do the accounting
too" (Queue decision `8c6b8319-b20e-4a78-b987-c09dee69886c`), approves a
different policy from the exact-cache proposal below. The sections below are
both the reviewed design (frozen at `c4b6447`) and the implemented behaviour;
where the implementation had to correct the design, it says so. The parked
exact-union design further down stays history and is not resurrected: the
accepted policy is the opposite one, cross-slot hard-link **overcount** and
trust of a recorded idle slot until its generation changes.

**What shipped.** `SlotRecord.recorded` carries one closed observation, the
`capacity_policy` fence is now **2** (version 1 stays readable and is upgraded
by the next revised write; oxide#177 adds the raised-ceiling policy 4 above
128 slots, and oxide#175's eviction accounting is policy 3), and the front
door's control `STATE_VERSION` is **8**, carrying the recorded-accounting,
eviction-accounting and raised-ceiling fences written durably by the first
mutating command's admission gate and, for a pre-Store entry, by the locked
`Pool::open` itself before reconciliation. `Pool::recorded_total()` replaces the whole-payload admission walk
with `pool_scope()` (a new `inventory::scope_directory` entry point with a
ledger-membership hard stop, observed on its own debug-only
`OXIDE_TEST_SCOPE_LOG`) plus one figure per slot, and `Pool::pressure()` is
that same pool figure for **both** backings: a shared removable volume's other
occupants are not this pool's usage. `release_or_recover`
measures the one slot it closes and publishes the record in the same atomic
`Held -> Idle` replacement; every transition out of `Idle` clears it. Default
`oxide status` reports the ledger's recorded total and per-slot record as the
admission basis, separately from the oxide#128 sidecar, and `--measure` stays
the full-walk audit.

**Corrections the implementation made to this design.** (1) A *current* record
is trusted at admission, so an entry that only a fresh walk would find
unreadable no longer blocks a new-slot admission; the whole-idle-slot deletion
then either succeeds or fails honestly and leaves a recorded `Evicting` slot
for reconciliation. Reuse of that same idle slot still walks it and still
refuses. (2) `pool_scope()` needed its own walker: `extra_depth` only raises
`max_depth` and files at the bound are still counted, so any depth bound would
either descend into legacy `slots/<id>` roots or report a `DepthLimit` gap at
labelled `slots/<folder>/<profile>-<id>` roots. (3) The 5 s
`VOLUME_WALK_MS` bound applies to the walks a removable pool's figure needs, so
a pool too large to measure reaches the unknown-size reserve route instead of
timing out mid-walk.

**Evidence state.** Source and unit regressions are in this change. The
focused proof `scripts/test-recorded-slot-accounting.py` and its three
selectors are **held**: no counted native has run, its fit is unmeasured, and
the required uncounted diagnostic is separately activated. The ELEVATED
author and independent reviewer natives remain outstanding.

#### Problem and final shape

Before this change one ordinary command paid at least one whole-payload walk:
`allocate` walked the payload before any eviction and again after it, a count
replacement walked it before deleting, and `evict_idle_until` walked it before
each deletion. A removable payload too large to walk inside 5 s then took the
retired volume-used fallback, which compared the whole volume's used bytes with
the pool cap. Measured cost is the one recorded for the outage: a 127-slot USB
pool could not be walked within the 60 s bound, so every new-slot admission
refused.

After this design, admission and byte-pressure eviction are arithmetic over
the ledger: **O(slots) metadata work, zero inventory walks when every slot's
record is current**. Only a slot without a current record is walked, one
bounded walk each, and only when that admission actually needs its number. No
whole-payload admission, readiness or result-line scan survives.

#### What is recorded

One closed observation per slot, inside the existing ledger record (no second
accounting file, no sidecar format):

| Field | Meaning |
| --- | --- |
| `for_lease` | the slot's `last_lease_id` at the closing transition |
| `unique_allocated_bytes` | the walk's `(device, inode)` union allocated bytes |
| `logical_bytes` | the walk's logical bytes, reported only |
| `entries` | entries visited, reported only |
| `at_ms` | wall clock at measurement; an observation, never a validity input |

The record is written **only** in the atomic ledger replacement that moves the
slot `Held → Idle`, in the same JSON object as `state: Idle`, `lease_id: null`
and the freshly stamped rank. It is cleared in the same replacement as every
transition out of `Idle` (`take_idle`, `retire_idle_slot` → `Evicting`). It is
never written for a `Held` slot and never adopted across incarnations.

Size. At the 128-slot ceiling `MAX_LEDGER_BYTES` stayed 2 MiB and the record
cost at most `128 × (``"recorded":{…}`` ≈ 960 escaped lease bytes + 4 × 20-digit
integers)`, about 131 KiB at every bounded field's ceiling; oxide#177 later
raised the cap to 8 MiB with the 256-slot ceiling (see the slot-headroom
representation above), and the worst-valid-ledger unit proofs carry both the
record and the reachable deletion-journal state there. If a future proof cannot fit the cap, a **fit blocker is
reported** and the bound is not silently raised.

#### Validity: what makes a record current

A record is current only when all of these hold, evaluated under the pool
lock in the same open that uses it:

1. `slot.state == Idle` and `slot.lease_id.is_none()`;
2. `record.for_lease == slot.last_lease_id` — the generation fence. Every
   `Held` transition mints a new `last_lease_id`, so a leftover record can
   never match a later generation even if a future edit forgot to clear it;
3. the slot's recorded root still resolves: `<payload>/slots/<dir>` exists and
   is a real directory (`symlink_metadata`, no follow). A root that is another
   kind is counted by `pool_scope` exactly as today's whole-payload walk
   counts it (never its target) and its record is never
   published or used; a missing root is a complete zero, as today;
4. the pool has no unresolved `Evicting` record and `reconciliation_blocked`
   is false.

Anything else is **not current**: never zero, never free space. A slot whose
record is not current is walked before its number is used; an idle slot on the
internal directory pool whose walk stops partial is charged conservatively for
that admission and kept (oxide#186), and every other incomplete walk keeps
today's `unknown_accounting` refusal exactly.

The one deliberate relaxation, restated because it is the whole point: a
current record is trusted even though a same-UID process could have changed
the slot's contents since it was written. The record is a size observation at
release, **not** proof of immutable bytes, writer closure or volume closure.
Newly unreadable entries, new Finder metadata and cross-slot link changes
inside an otherwise recorded idle slot are not re-detected per admission. The
invalidation paths are: the next lease and its release, `retire_idle_slot` /
recycle / eviction, `oxide status --measure` (audit, below), and any crash or
held transition. Nothing else claims to bound later same-user mutation.

#### Lifecycle, locks and publication

Admission walks already run under the pool lock (`Pool::open` takes it, then
`allocate` walks), so **no lock order changes**: the internal admission
barrier still precedes the removable one, nothing takes admission inside the
pool lock, and no new lock or file is added. Recovery, retirement and recycle
keep their existing order.

| Event | Ledger result (one atomic replace + sync) | Crash before write | Crash after write |
| --- | --- | --- | --- |
| Allocate a new slot | `Held`; no record | previous state | `Held`, no record; recovery as today |
| Same-holder replay | nothing written | — | — |
| Reuse a compatible idle slot (own walk, unchanged) | `Held`, record **cleared**; the walk's figure is the result line's `measured` | still idle, record intact | `Held`, no record |
| Release / recover after positive writer closure | `Idle`, rank stamped, plus the record if the walk completed, else no record | still `Held`; the next closure attempt walks again | `Idle` with (or honestly without) a record; an idle-lease replay writes nothing and keeps the record |
| Removable flush fails (`flush_before_idle`) | retired / lost, **no record**; the lease stays held | unchanged | unchanged |
| Admission refreshes missing records | one write carrying every record this admission completed, **before** the admission decision, so progress survives a refusal | no change | records durable |
| Evict / recycle / reconciliation | `Evicting`, record cleared, then the record is removed after deletion as today | unchanged | as today |
| `status --measure` | nothing written | — | — |
| Upgrade a legacy ledger | `capacity_policy: 2` installed by the first revised write; held slots unaffected (always walked) | still legacy | one-way fence, below |

**Release ordering with oxide#136 and removable flush.** The front door already
does, in this order and with the admission lock held: positive group closure →
`discard_scratch` (remove the owned `TMPDIR`) → `flush_before_idle` (flush the
bound volume to the device with `volume::fsync_volume_np`; the volume stays
mounted, and only a successful flush returns `true`) → `release(&pool,
lease_id)` (`store.rs:1622-1638`). The record walk is inserted **after** that
flush and **before** the `Held → Idle` replacement:

1. bounded walk of that one slot (`inventory_sizes_with(slot_path, 0, limits)`,
the same call `slot_accounting` uses), outside no lock it did not already hold;
2. re-read the in-memory record and require `state == Held`,
`lease_id` is ours, `last_lease_id` unchanged, `incarnation` unchanged;
3. only then write `Idle` + rank + record in one replacement.

Step 2 is the conditional publication: a slot that is no longer ours is never
stamped with a figure. Because the slot is held by this command, its writers
are positively closed, and a competing command cannot take or recover it, the
walk observes quiescence to the same degree as `status`. That is a same-UID
cooperative trust statement, not an isolation proof. A failed or incomplete
walk publishes **no** record and **no diagnostic**: release still succeeds
exactly as today, and `status` shows the slot as unrecorded. No persisted
diagnostic is added, because it would be a second representation with no
admission use.

#### Admission and byte pressure

The pool's pressure figure is `recorded_total()`:

```
recorded_total() = pool_scope() + Σ slot_bytes(slot)      over the ledger

slot_bytes: Held                    -> fresh bounded slot walk (required complete)
            Idle + current record   -> record.unique_allocated_bytes  (no walk)
            Idle + missing/invalid  -> one bounded slot walk; complete: published
                                       before the decision; partial on the internal
                                       directory pool: charged conservatively for
                                       this admission, never a record or a victim
                                       (oxide#186)
            Evicting / blocked      -> unknown -> refuse (today's rules)
```

**`pool_scope()` is a new walker, not an `extra_depth` walk.** `extra_depth`
(`pool.rs:2072-2078`) only adds to `max_depth`, and `max_depth` gates directory
descent while files in a directory at the bound are still counted
(`inventory.rs:640-716`). So `inventory_sizes_with(payload, 2, ..)` *is* today's
whole-payload walk: it descends into legacy `slots/<id>` roots at depth 2, and a
depth-2 bound would instead report a `DepthLimit` gap at labelled
`slots/<folder>/<profile>-<id>` roots. Neither is the shallow figure this design
needs, so Phase B adds a separate entry point with an explicit **stop
predicate**, and `payload_accounting()` keeps its current call for
`status --measure` only:

1. read the payload root's entries; count each regular file (tolerated Finder
   metadata such as `.DS_Store`/`._*`, and any stray file) into one
   `(device, inode)` union;
2. descend `slots/`, and one project-folder level below it, counting the files
   directly at those levels;
3. classify every directory entry it meets against **ledger membership**, not
   record validity: a relative path equal to a recorded slot root — `<id>`
   (legacy) or `<folder>/<profile>-<id>` — is a **hard stop**: not descended
   into and not counted, because that slot's own walk or record owns its files;
4. any other directory (a stray, an orphan, an unrecorded non-ledger region) is
   descended and counted under the normal entry, depth and deadline bounds, so
   nothing outside a slot can silently undercount.

The classifier is path-based and layout-agnostic, so legacy and labelled roots
are treated identically and no root is walked twice. Two consequences are
stated rather than assumed. Directories contribute **no** allocated bytes to
any union — both walks count only regular files
(`inventory.rs:715-716`, `record_file`) — so a slot-root directory inode cannot
be double-counted, only its files, and the hard stop is what prevents that.
And the figure is a partition of the payload's files: every regular file under
`<payload>` is counted exactly once, by `pool_scope` or by the one slot that
owns it, with cross-slot hard links the only deliberate overcount.

`pool_scope()` carries the same `InventoryLimits` as a slot walk and shares the
admission's cumulative deadline. Any gap refuses exactly as today. It costs
O(slots + metadata) and is not a payload walk. It is observed separately (see
the proof oracles) so "zero inventory walks" stays a checkable claim.

One cumulative `ACCOUNTING_MAX_ELAPSED_MS` (60 s) deadline covers every walk of
one admission, via the existing `walk_limits`/`deadline` mechanism `measure`
already uses, so a legacy 256-slot pool cannot turn one admission into 256
fresh minutes. Slots the deadline leaves unmeasured stay unrecorded and are
charged conservatively for that admission (oxide#186); a walk that must still
be complete — a held slot's live figure or the scope pass — refuses
`unknown_accounting` naming `elapsed_deadline`, today's refusal for that walk
arrived at by slot walks instead of one union walk.

Byte pressure then follows today's rules unchanged (`budget` from
`LeaseRequest.budget_bytes` else the recorded cap): under budget, admit; at or
over, evict the least-recently-used **eligible** idle whole slot, remeasure,
and refuse `Full` when only held / retired / uncertain slots remain, or the
bounded capacity reason naming the unmeasured slots when one was charged
conservatively (oxide#186). One deletion must reduce the figure by at least
the victim's recorded bytes; a
non-reduction refuses `unknown_accounting` instead of looping. Held, foreign,
symlinked, uncertain and unmeasurable slots — including one this admission
charged conservatively — are never candidates, and nothing
here deletes held or unknown storage, active scratch, outputs or evidence.

**Hard links.** A slot's own walk unions `(device, inode)`, so a link pair
inside one slot counts once, as today. Across slots the recorded sum counts
such an inode once per slot. That overcount is accepted: it can only make the
figure larger, so pressure arrives earlier and an idle slot may be evicted
earlier than the union would have required — never later, and never a smaller
figure than reality. The invariant asserted in tests is `sum >= full inode
union` for the same settled state, equality when the slots are disjoint.

**Out-of-slot metadata.** Tolerated Finder metadata beside and between slots,
stray non-slot entries and orphan directories under `<payload>` are counted by
`pool_scope()` and attribute to no slot. They can therefore never silently
undercount, and never grant ownership: they are not eviction candidates and
the existing foreign-entry, symlink, root-identity and no-follow refusals are
unchanged.

#### Removable pools

`apfs-volume-v1` admission uses the same **pool-scoped** figure as the
directory backend: `recorded_total()` only. The bound volume's total used
bytes are **not** pool usage — another tool's cache, scratch or Desktop files
on the same volume are not this pool's bytes and can never push it over its cap
or trigger an idle eviction (Tom, 2026-10-25, after a live outage where old
byte accounting evicted warm slots for foreign growth). Free space on the
volume is a separate signal: the existing below-reserve selection fallback and
build-time reserve watchdog are unchanged.

An unknown pool size is honest and never zero. When the pool's own walk stops
at its own bounds (`ElapsedDeadline` or `EntryLimit` only), the reserve route
applies and it **is** allowed to prune: free space above the reserve (and at or
above the floor) admits with a bounded note and no eviction; free space at or
below the reserve — and any free space below the hard floor while anything is
left to prune — retires the eligible idle whole slots in the existing LRU order,
re-reading free space and re-verifying the binding after each; with nothing left
to prune, free space **at or above the 10 GiB hard floor** admits with a warning
and below it refuses with the original incomplete scan. The floor is a
fixed product constant for this change — not a registration field, selection or
watchdog setting — and it wins over any lower legacy reserve, so nothing
admits below it. Every other incomplete shape, an unverifiable binding, unknown
free or reserve facts, and the internal directory pool's own incompleteness
refuse exactly as before; a partial scan is never published as a record and the
cap stays stated as unverified on this route.

Provenance is printed (`recorded n/m slots`). Free space, the reserve and the
hard floor are printed in binary GiB, the same units they are defined in, and
the cap comparison never includes the volume's total used bytes. Unknown free or
reserve facts refuse `unknown_accounting` on the unknown route, and an absent,
mismatched or unverifiable backing still refuses before any effect. The
below-reserve *selection* fallback and the running-build reserve watchdog keep
their existing behaviour; widening them to prune and to use a registration floor
field is the approved follow-up, not part of this change.

**Fallback disposition.** The retired `admit_by_volume_usage` path, its
decision type and its fixed eight-eviction loop are gone: the whole-volume-used
figure is no longer a budget input anywhere. What remains is one pure decision,
`unknown_removable_step`, and the pool-side loop that prunes eligible idle
whole slots while free space is at or below the reserve or below the floor.
The 5 s `VOLUME_WALK_MS` bound still applies to the walks a pool figure needs,
so a huge removable pool reaches the unknown route rather than timing out
mid-measurement.

#### Status, audit and provenance

Default `oxide status` (`managed::snapshot`) already walks nothing, takes no
lock and reconciles nothing. It gains **display only** and must name its two
sources, which are never mixed:

| Source | What it is | Shown as |
| --- | --- | --- |
| **Ledger record** (this design) | the admission basis, written by releases and recorded admissions | per-slot bytes with `recorded` / `unrecorded`, and one pool `recorded total` over `recorded n of m slots` |
| **`oxide-managed/status-observations.json`** (oxide#128) | a diagnostic reuse-walk fact or an incomplete admission-walk fact (oxide#186), up to 512 KiB / 256 facts | the existing per-slot line with its age, source and lease, `not measured (walk incomplete)`, or `not measured` |

The two are labelled separately and never summed into one number: the #128
sidecar stays ineligible for a pool total, and the ledger total never borrows a
sidecar fact. Only the ledger total carries admission meaning, and even it is a
recorded observation — not free space, not a writer- or volume-closure claim.
An `unrecorded` slot makes the total a stated lower bound. That includes a slot
whose admission walk stopped partial: it is charged conservatively for that
admission only, publishes no record (oxide#186) and keeps its existing
`not measured` per-slot line. Default status never
publishes, refreshes, reconciles or creates a lock, and a snapshot may still
straddle a concurrent replacement and says so.

**Owning sentence.** `workflow.md` used to state that default status reports
recorded sidecar facts and that "recorded facts are never summed into a pool
total". The same change rewrites exactly that sentence in
[storage and space management](workflow.md#storage-and-space-management) to
name the two sources above, because the binary now does show a ledger total:
that older sentence was true of the sidecar only and would otherwise
contradict the implementation.

`oxide status --measure` keeps the full payload and per-slot walk as the
audit: unchanged bounds, no lock, no publication, coverage/cause/elapsed
reporting. Audit never mutates the ledger and never publishes a record.

No `--verify` mode is added. Migration therefore converges through leases:
releasing a slot publishes its record, and an admission publishes every record
it completes. A pool of never-reused idle slots converges one admission at a
time (each admission making durable progress and refusing honestly while the
cumulative deadline leaves any slot unmeasured), or is cleaned/evicted by its
owner. That cost is stated rather than hidden behind a background sweep, which
this design does not add.

#### Compatibility and version fencing

`CAPACITY_POLICY_VERSION` becomes **2**, written by the first revised ledger
replacement that also writes a record or reads a `2` ledger. Both representations
fence the one-way step:

- an older binary's `deny_unknown_fields` ledger parse rejects `recorded` and
  refuses `unknown_accounting` (exit 3) before any write or deletion;
- if a parse ever succeeded, `validate_ledger_read` already requires
  `capacity_policy == CAPACITY_POLICY_VERSION`, so a `2` ledger refuses there;
- a new binary reads a `capacity_policy: 1` or legacy ledger unchanged: every
  slot is simply unrecorded, held slots are walked as today, and the first
  revised write installs 2.

The one-way switch needs ordering the ledger alone cannot provide, because the
front door's hand-off trigger reads control `state.json` /
`removable.json`, not the library ledger (oxide#122). Hand-off requires a
peeked numeric version **greater than the running binary's**, so the value is
the version this gate writes, **5** (`RECORDED_ACCOUNTING_STATE_VERSION`,
`store.rs:74`; 4 is the oxide#133 owned-outputs fence an installed binary
already writes). The [one-use workspaces](one-use-workspaces.md) fence made
version **6** the one-use version, written only when that feature first writes
a class-bearing record, so a recorded-accounting-only pool stays at 5. The 3 an earlier
revision of this section proposed was a *downgrade* from that shipped 4 and
would leave an installed version-4 binary exposed to `deny_unknown_fields` on
`recorded`. The fence is installed by the front door's mutating-command
admission gate, so it is durable before any library write can fence the
ledger; an owned-outputs-only save still derives version 4. Phase B therefore writes control `version: 5` **before** the first
recorded ledger write, so an installed version-4 binary hands off to a
receipt-matching successor instead of refusing the pool it was just handed.
The raised-ceiling fence (oxide#177) repeats that ordering for `capacity_policy:
4`: the admission gate writes control version 8 (with the `oxide.slots/256`
fence) as the command's first write, before the locked open and before that
same admission can record the 129th slot, and a later ordinary save keeps
version 8 because the fences are part of the document (oxide#190 later raises
the same document to version 9, which also carries `oxide.holding/1`). A
crash between the two writes leaves control 5 with a v1 ledger, which is a
normal state for the new binary and a hand-off for the older one; the reverse
order would strand it. The old binary's own refusal remains the honest
fallback when no receipt-matching successor exists. This is the only
cross-file ordering in the design, it changes one existing proof's input, and
Phase B re-runs `rust:installed-handoff` for that reason — its
version-3-trigger fixtures need a version-5 trigger and a version-4 control
that keeps the existing refusal. Existing held IDs, lease-ID
format, receipt and status fields, `MAX_SLOTS`, rank sequence, ledger bounds
and every output contract are otherwise unchanged.

**Lost replies.** A retried `release` after a lost reply hits the idle replay
path, which writes nothing and keeps the record. A lost reply *before* the
write means the retry performs the walk and transition again — same figure or
an honestly absent one. A lost `allocate` reply leaves a `Held` slot with no
record; the next recovery closes it and records it. No path treats a missing
reply as a size.

#### Source map (this tree)

| Concern | Where |
| --- | --- |
| Ledger/config/limits, fences | `src/lease/mod.rs` (`LEASE_VERSION`, `CAPACITY_POLICY_VERSION`, `MAX_LEDGER_BYTES`, `MAX_SLOTS`) |
| Record and ledger types | `pool.rs` `Ledger`, `SlotRecord`, `SlotPersistState` (`:153-201`) |
| Admission | `Pool::attempt` reuse walk (`:865`), `Pool::allocate` (`:949-1051`), `payload_check` (`:1336`), `payload_accounting` (`:1536`), `slot_accounting` (`:1559`) |
| Eviction | `evict_idle_until_under_cap` (`:1368`), `evict_one_idle` (`:1395`), `retire_idle_slot` (`:1420`), `evict_idle_until` (`:1282`), `replace_idle_slot_for_admission` (`:1260`) |
| Release/recover | `Pool::release_or_recover` (`:1057`) |
| Status/audit | `Pool::accounted` (`:1102`), `managed::snapshot`, `managed::measure` (`:446-492`) |
| Removable unknown route | `unknown_removable_step`, `admit_unknown_removable`, `bound_gaps_only`, `pressure_known_or_bounded_removable`, `UNKNOWN_REMOVABLE_FLOOR_BYTES` (`10 * 1024^3`), `VOLUME_WALK_MS` |
| Front-door release order | `store.rs` `release` (`:1622`), `unshare` (`:1640`), `discard_scratch` (`:1674`), `flush_before_idle` (`:1689`) |
| Walks and their bounds | `inventory.rs` `inventory_directory` (`:436`), the walk observer call (`:438`), directory descent vs file counting (`:640-716`), `Sizes::require_complete` (`:1854`), `walk_limits` (`:1523`), `extra_depth` (`:2072-2078`) |
| Debug-only observations | `inventory::walk_observer` (`:1354`), `OXIDE_TEST_WALK_LOG`, `delay_before_read_dir`, plus the new debug-only `OXIDE_TEST_SCOPE_LOG` for `pool_scope()` |
| Control state fence | `store.rs` `STATE_VERSION`: 8 is the raised-ceiling fence (oxide#177), 7 the eviction-accounting fence (oxide#175), 6 the [one-use workspaces](one-use-workspaces.md) fence, 5 the recorded-accounting fence; an owned-outputs successor alone is version 4 (`RECORDED_ACCOUNTING_FENCE`, `ONE_USE_FENCE`, `EVICTION_ACCOUNTING_FENCE`, `SLOT_CEILING_FENCE`, hand-off trigger in `handoff.rs`) |
| Qualified guard | `scripts/bounded-test.py`, sha256 `bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924` ([oxide#151](workflow.md#shared-proof-supervisor)); this tree's `d000a445…` bytes add oxide#191's opt-in post-census detach and are unqualified until their own qualifications pass; the oxide#147 `5e5b6617…` and oxide#145 `a6d73326…` bytes are history |

#### Acceptance matrix

Unit/regression (`effigy test rust`), against the real library entry points, not
decision mocks where a real path exists:

1. **Zero-walk admission.** All records current → `allocate` makes **no**
   `inventory_directory` call at all, so the existing walk observer is empty,
   and the figure equals the recorded sum plus `pool_scope()`. Because
   `pool_scope()` is its own entry point it records on the separate debug-only
   `OXIDE_TEST_SCOPE_LOG`; the oracle requires exactly one scope line naming
   the payload root and zero walk-log lines. Without that split the "empty
   observer" claim would be untestable: `inventory_directory` records every
   root at `inventory.rs:438`, whichever caller asked.
2. **Missing-only walks.** Two of five slots unrecorded → exactly those two
   slot walks plus one `pool_scope()`, and no walk of the three recorded roots
   (asserted per root, not by count alone); the completed figures are published
   before the decision and survive a refusal.
2b. **Scope classification.** Legacy `<id>` and labelled `<folder>/<profile>-<id>`
   roots are both hard stops, a recorded root is never descended into, a stray
   sibling directory is counted once, and swapping every slot's contents for a
   different size changes the total by exactly the recorded deltas — the
   regression that falsifies a depth-bounded substitute for the stop predicate.
3. **Held isolation.** A held slot is always walked; a held slot never reports
   an idle record and a record for a held slot is never published or used.
4. **Invalidation.** Reuse, recycle, eviction and recovery clear the record in
   the transition write; a leftover `for_lease` mismatch is never current;
   `Evicting` / `reconciliation_blocked` refuses unknown.
5. **Sum vs union.** Single-slot hard-link pair, cross-slot pair, duplicate
   content in two slots and disjoint slots: `sum >= union`, equality when
   disjoint, and the extra slots pressure the byte budget (non-vacuous: an
   eviction that the union would not have caused).
6. **Non-zero/zero discipline.** Malformed, oversized, unknown, stale,
   mismatched-incarnation and unsupported records are never zero; a measured
   zero stays a measured zero; an incomplete walk refuses with its gap kinds.
7. **Serialization.** Worst-valid 256-slot ledger round-trips under
   `MAX_LEDGER_BYTES`; an oversized field refuses with the file untouched.
8. **Compatibility.** `capacity_policy: 1`, legacy (no fence), `2`, `3` and
   `4` ledgers; held IDs preserved; the admission gate writes control version
   9 (recorded, eviction-accounting, raised-ceiling and holding fences) as every
   mutating command's first write, and a locked `Pool::open` raises it before
   its reconciliation, holding move or ledger write, so a pool this binary mutates
   never feeds a predecessor a ledger it cannot represent or a control version
   it cannot read; an
   owned-outputs-only state derives version 4 (proved on `save` directly, not
   through a path that takes admission), a v4 control still hands off, a v5
   document without the recorded fence or with an unknown fence value refuses,
   and version 8 requires the raised-ceiling fence; old-binary refusal is
   exercised as a ledger parse/validation refusal, not a mutation.
9. **Removable combination.** The cap uses pool bytes only, so a volume whose
   unrelated used bytes exceed the cap still admits with no eviction while the
   pool is below it; the unknown-size route acts on free space against the
   reserve and the 10 GiB floor (prune, warn-admit or refuse), never on
   whole-volume usage, and only for bound-only gaps.
10. **Release failure paths.** Incomplete walk, conditional-recheck mismatch and
    a lost reply all leave the pool consistent and never publish a figure.

Native (`effigy rust:recorded-slot-accounting`, one fixed script
`scripts/test-recorded-slot-accounting.py`, the whole command of the qualified
shared supervisor): tiny private fixtures only, debug staging with
`debug_assertions` on, private `HOME`/`CARGO_HOME`, explicit `OXIDE_POOL`:

- A: all-recorded new-slot admission with an empty `OXIDE_TEST_WALK_LOG` over
  the admission phase and exactly one `OXIDE_TEST_SCOPE_LOG` line (zero
  payload/slot walks), on a pool whose idle slot is large enough that a walk
  would be visible. Every admission oracle in the proof is read while the
  gated command is still `Held`, because a release always measures the slot it
  closes; the release walk is then asserted separately, so neither claim can
  be satisfied by the other phase. The
  count-replacement branch of the same `evict_one_idle` / `retire_idle_slot`
  code is proved by the library regression
  `pool_ceiling_replaces_the_oldest_idle_slot_and_keeps_the_count`, which now
  asserts an empty walk log: neither the 256-slot pool ceiling nor the
  twelve-slot project cap has a cheap CLI fixture, and `OXIDE_POOL_CAP` is a
  byte budget, not a slot count. Byte pressure in the CLI proof comes from that
  documented budget at pool creation, never from a proof-invented variable;
- B: one unrecorded idle slot → exactly one slot walk, published, and the
  second admission of the same shape walks nothing;
- C: release publishes a record and the next reuse clears it, with the result
  line's provenance text checked;
- D: a non-vacuous byte-pressure eviction of one owned idle whole slot beside a
  held slot and a foreign out-of-slot entry, both untouched byte-for-byte. D
  first proves its own placement from the product's serialized `config.json`
  (`backend: "directory-budget-v1"` and no `volume` entry) through
  `recorded_backend()`, which refuses a missing, malformed, unknown or
  removable backend by name; the ledger records no backend at all, so no case
  may read one from there;
- E: cross-slot hard-link overcount (sum ≥ union) plus a refusal case with
  untouched bytes/inodes;
- F: legacy ledger convergence across two admissions and one crash replay;
- G: default `status` shows recorded totals and walks nothing; `--measure`
  walks and still writes nothing.

Every case applies the nine-item
[proof-authoring premise checklist](workflow.md#proof-authoring-premise-checklist)
with an executable location or a concrete not-applicable reason: the script
records all nine in `PREMISE_CHECKLIST` (private HOME and rustup, account-home
control, environment isolation, no adopting conversion, no external Cargo
subcommand, no image, frozen compiler flags, the internal-root temporary
storage, no `mktemp`), `--self-check` asserts the map is complete, and the
diagnostic receipt carries it. The uncounted `--diagnostic` gate below runs
before the first counted native. The
brief's own six review subjects (recorded accounting, release observation and
crash fencing, admission/eviction/ownership, migration/status/compatibility,
proof source fit, exact-head natives) each map to a section above.

**Diagnostic mode.** `scripts/test-recorded-slot-accounting.py --diagnostic`
(selector `effigy rust:recorded-slot-accounting:diagnostic`, same supervisor
bounds, separately activated) declares a step/dependency/shared-premise map,
records recoverable oracle failures and continues only through
cases whose prerequisites are intact, writes one uniquely named bounded
receipt in the evidence root labelled `mode: diagnostic`,
`qualified: false`, with reached/completed/failed/skipped/unexecuted counts,
every reason, the taint set, the primary and secondary errors, and explicit
incomplete counts at its bounds. Counted default stays fail-fast. Any unknown
child/writer/device effect, pending custody, timeout, infrastructure or
protocol failure aborts rather than continuing; nothing is repaired by
assumption; a non-zero direct leader exit is never read as descendant closure.
A case that dies of an unexpected exception is recorded as that case's
failure before any state or receipt is computed and then aborts the
collection, so a reached-but-crashed case is never reported as a pass and its
successors are `not_run_aborted`; interrupts stay outside that handler. Every
abort goes through the one `mark_abort()` state, so the durable receipt and
the console cannot disagree about `aborted` or `collection_complete`. Pure
counterfactuals drive the same dispatch/case/receipt logic for fail-fast
unchanged, record-and-continue, dependency/taint skip, safety abort, an
unexpected exception in a middle and in the last planned case, a pre-case
setup failure, the shadowing-local abort regression and first-fault
precedence.

#### Phase and resource estimate (UNMEASURED)

| Phase | Work | Wall (est.) | Scratch peak (est.) |
| --- | --- | --- | --- |
| P0 | premise self-check, fixture construction | 5 s | 5 MiB |
| P1 | offline debug `oxide` staging with 1.97.1 using the reviewed oxide#140 recipe (4 Cargo jobs, 16 codegen units, `debug-assertions` on, `debug = 0`, opt-level `s`, incremental off, symbol-stripped, inside owned scratch) | 55 s | 90 MiB |
| P2 | A/B/C/F/G cases | 90 s | 30 MiB |
| P3 | D/E pressure, hard-link and refusal cases | 45 s | 30 MiB |
| P4 | evidence write, cleanup | 15 s | 5 MiB |
| | **total** | **210 s of a 270 s work window** | **~120 MiB of 512 MiB** |

300 s total with the supervisor's 30 s stop margin and 512 MiB monitored
scratch are unchanged; every write stays inside the owned root, no session,
group or census is started. The supervisor this tree runs is `scripts/bounded-test.py` sha256
`d000a445632ba9e4817b9f3251856abc6ca051408c60acb46eb094aaa11ff76f`: the
qualified oxide#151 guard (merged `af08749`, sha256 `bc24b238…`) plus
oxide#191's opt-in post-census image detach, so those added bytes are
unqualified until their own diagnostic, author and reviewer qualifications pass
([shared proof supervisor](workflow.md#shared-proof-supervisor)). The earlier
oxide#147 `5e5b6617`, oxide#145 `a6d73326` and `961164e7` bytes are history and
are not used.
These figures
are planning arithmetic, not measurements: the proof's own `--plan` prints them
and any activation re-measures and reports a **fit blocker** rather than
raising either bound. Both debug-only observation hooks — the existing walk log
and the new scope log — are `debug_assertions`-gated, which the staging recipe
keeps on and the fixture checks through the built-in capability probe before
any case asserts a count, and a release build must refuse control presence at
any value.

**Recipe binding.** The proof needs a genuine internal `directory-budget-v1`
pool, so both heavy commands pass `--root-parent /private/tmp` explicitly
(Tom's answered internal-root exception); a pool under a selected volume would
be refused by the product itself. Staging is one function with the real argv
and environment, and `--self-check` mutates it (one job, one codegen unit,
profile drift, a leaked encoded-flags value, a build directory outside the
stage) so the pure check fails if the recipe regresses. Fixtures keep one Cargo
job and run sequentially; the configured jobs are build parallelism, never a
CPU cap or an admission-demand claim.

#### Phase B envelope

Files: `src/lease/mod.rs`, `src/lease/pool.rs`, `src/lease/tests.rs`,
`src/inventory.rs` (the `pool_scope()` entry point and its debug-only scope
log), `src/bin/oxide/store.rs` (control `STATE_VERSION` and release ordering),
`src/bin/oxide/main.rs` / `describe.rs` (status and result-line provenance),
`effigy.toml`, `scripts/test-recorded-slot-accounting.py`, this file, the
`workflow.md` hotfix paragraph and its `storage-and-space-management` status
sentence, and the `AGENTS.md` selector map. Covering
selectors: `rust:compile`, `rust:compile-linux`, `rust:format`, `test rust`,
`docs:check`, the re-run `rust:installed-handoff`, and the new
`rust:recorded-slot-accounting` / `:diagnostic` / `proof:recorded-slot-accounting:test`.
Validation is **ELEVATED** because byte pressure decides a deletion: one author
native and one distinct independent reviewer native, each separately ROOT
activated on exact clean published source, no shared allowance, and the
uncounted diagnostic required before the first counted native. Failure or an
unknown launch stops; no refund and no automatic re-run. The hand-off proof's
version inputs moved to this tree's fence (control 5, newer-than-self 6, an
equal 5 deliberately not a trigger) and it needs its own safe diagnostic before
a counted rerun. No live-pool migration, cleanup, installation, release or
consumer change follows from this implementation.

### Historical parked exact cache (lead `cbb2824d`)

Parked on 2026-10-21, not implemented. Sol's re-review of revision 2 (`d578447`) was again changes required. Transient hard-link mutation means an idle slot's contents and link topology cannot be shown to be unchanged cheaply, and out-of-lock walks need snapshot and conditional-commit rules. An exact cache therefore needs an immutability mechanism or an explicit policy relaxation. The per-command cost is addressed first by removing the front door's non-authoritative whole-payload walks, a separate correction. Both review verdicts are kept on the lead. Revision 2 text follows for reference. Revision 2 answers Sol's prospective review of `5852670` (changes required: shared-inode mutation, metadata outside slots, transition rules). Revision 2 drops revision 1's held baselines and overcounting sums.

**Goal.** Stop walking every idle slot on every command, while keeping today's admission figures. Today each command walks the whole payload at least twice: once in the front door's selection status and once at library admission. Measured on the Builds pool: 95,020 entries in 1.2 s, then 201,948 entries in 3.4 s an hour later.

**What is cached.** A closed observation per idle slot: `{incarnation, closing lease ID, unique bytes, logical bytes, entries, shared inodes, measured-at}`.
- *Shared inodes* counts regular inodes whose `nlink` exceeds the links found inside that slot.
- An incomplete walk stores no observation. It stores a diagnostic instead: gap causes, an observed lower bound and the time.
- Ledger records stay within `MAX_LEDGER_BYTES`. The worst-case serialized size is proven in tests.

**Admission figure.** The fast path is the sum of three parts:
1. fresh shallow accounting of the payload root and `slots/` entries: the tolerated macOS metadata from oxide#105, never control-root bytes;
2. a fresh walk of each **held** slot, as today, so held growth still counts;
3. the cached observation of each **idle** slot.

The fast path applies only when every idle slot has a valid observation with zero shared inodes, every held-slot walk shows zero shared inodes, and every shallow metadata entry has `nlink == 1`. Under those conditions the slots are pairwise disjoint: an inode in two slots has links outside each, so it shows as shared in both. The sum then equals today's `(device, inode)` union.

Otherwise admission uses the **union fallback**: fresh bounded walks of the root entries and every slot, unioned as today. The fallback also refreshes the observations of idle slots whose walks complete. Shared-inode mutation therefore never reaches the sums or subtractions. In Sol's B/A case, B closes with shared inodes greater than zero, and every later admission and eviction takes the union path. There is no persistent inode census.

**Validity.** An observation is valid only while all of these hold:
- the slot is idle and unleased;
- its incarnation and last lease ID equal the observation's;
- its root `(device, inode)` and the removable volume binding are unchanged;
- it is younger than the maximum age (proposed 24 h).

Anything else invalidates it.

**Transitions.** Every row is one atomic, synced ledger replacement unless stated otherwise.

| Event | Ledger result | Crash before / after the write |
| --- | --- | --- |
| Allocate a new slot | Held; no observation | Unchanged / held new slot (recovery as today) |
| Acquire or replay an idle slot | Held; observation **cleared in the same write that changes `last_lease_id`** | Idle with observation / held without one |
| Close or recover (after positive writer closure and, on removable pools, a successful flush; the walk runs through the bound slot fd before the write) | Idle, plus the new observation if the walk completed, or the diagnostic if not | Still held, so closure or recovery repeats the walk / idle, settled |
| Removable flush fails | Retired or held, as today; no observation | Unchanged |
| Lazy refresh (idle, observation missing or expired): walk outside the pool lock, then recheck under it | Observation or diagnostic, **written before and independently of the admission decision**, so progress survives a refusal | No change / refreshed |
| Recycle, eviction or reconciliation | Evicting, observation cleared, then the record is removed after deletion as today | As today |
| Upgrade a v1 ledger | v2 with no observations; held slots are unaffected because they are always walked fresh | The first v2 write is the durable upgrade; before it, the ledger is still v1 |
| `status --verify` | May publish observations for **idle** slots that still match under the lock; never for held slots | No change / refreshed |

**Rules.**
- **Refresh per admission:** each admission refreshes at most two missing or expired observations, oldest first. If any stay missing, that admission takes the union fallback, so behaviour matches today.
- **Reuse:** reusing a compatible idle slot still walks only that slot, as today. It is never blocked by other slots' missing observations.
- **Locking:** the internal admission barrier comes before the removable admission, and pool locking stays inside admission. Nothing blocks on a record-owner lock while holding admission, or takes admission from inside the pool lock. Walks run outside the pool lock and publish only after a recheck.
- **Missing observations:** these are reported as "not cached". They are never a selection failure, and they never cause a new fallback to the internal pool. The existing explicit selection fallbacks are unchanged.
- **Front-door paths:** one library accounting result serves every path, with no private walks: selection, `status`, the per-command result line, `slot_size` and the recursive reporting at `main.rs:664`, project-cap eviction, shared-slot handback, untrusted-slot discard, normal and reserve-stop release, automatic and explicit recovery, and removable flush and retirement.
- **`status --verify`:** each walk uses the existing bounds, with an overall deadline (proposed 300 s). A partial result is reported as partial, and only completed idle walks are published.
- **Ledger version:** the ledger version is independent of the lease and config wire identities, and existing held IDs are preserved. Old binaries refuse a v2 ledger without changing it.

**Policy change for Tom.** One remains: **delayed detection inside idle slots.** Between measurements (at most the maximum age, and also at every reuse, recycle or `status --verify`), two things go unseen:
- a newly unreadable entry or mount boundary inside an idle slot;
- Finder metadata bytes added inside an idle slot.

Today a new allocation would detect the first, and count the second, immediately. Revision 2 has no other admission change: held slots still count fresh, unions stay exact, and duplicate accounting adds no evictions.

**Proof (after #109 qualification, elevated, `rust:pool-accounting-cache`).**
- A pool of at least 200,000 entries, including a slot of at least 60,000, where fast-path admission walks only the held slots and the root.
- Each transition row and both crash sides.
- Lazy refresh persisting across a refusal.
- A migrated held slot.
- Cross-slot links, including mutation followed by eviction, taking the union path.
- Root metadata, including a hard-linked entry.
- `status --verify` running alongside a held build.
- Removable flush failure and reattachment.
- Measured close-time and acquire-time cost.

History, retired with the image backend (oxide#102): the verified APFS
sparse-image volume was the hard aggregate bound, the declared cap had to
equal the image size, and `verified_capacity_bytes` was the effective OS
bound. Current pools have no such ceiling.

Nothing in the library stops a leased build from filling its filesystem:
writes then fail with the OS's own error and the lease stays held. The front
door's best-effort host-reserve watchdog is owned by
[workflow](workflow.md#directory-budget-v1-as-shipped). Host reserve, OS
metadata and private workspace `target`/temp bytes are distinct from managed
payload bytes; status does not add them together. Deleting an idle slot frees
its space on the payload filesystem. No build-size distribution or failure
percentage is established. Selective `cargo clean -p` is not part of this
interface.

## Exit codes

| Code | When |
| --- | --- |
| 0 | init/lease/release/recover success; complete status and capacity observations |
| 1 | usage, invalid input, I/O, unsupported platform |
| 2 | busy or holder conflict; also partial/truncated status |
| 3 | full, unknown accounting, or ledger/slot exhaustion |
| 4 | unsafe path, symlink, foreign uid, or backing mismatch |
| 5 | unknown lease identity |

Lease/release/recover/init emit one versioned JSON object on stdout. Errors
go to stderr as `oxide-storage-lease: <reason>: <message>`.

## Reviewed local handover artifact

A packaging-only rebuild from merged reviewed source
`556171b63b69c32a89ec927ca2763b78f18efbc1` (reviewed head
`f75cbb4d0efd11034adb3d1dd826df9bb993ecf0`, PR #67) is available at:

```text
/Users/tom/.local/share/northstar/release-audits/oxide/2026-10-19/lease-handover/f75cbb4d0efd11034adb3d1dd826df9bb993ecf0/oxide-storage-lease
```

SHA-256: `ddceaf502589ea760a86edec8f47e3563675727f754c00888127eb4074535045`.

On 2026-10-19, the four handover binaries were copied, without rebuilding or
moving their originals, out of disposable `target/` into that durable audit
root. `manifest.json` beside the source-ref directories records source and
copy paths, sizes, modes and matching SHA-256 hashes. The historical copies are:

| Source ref directory | Binary SHA-256 |
| --- | --- |
| `376844daedd363ff5c2441e3ba12d1dfbe4e54b0` | `637a68b740b0ad424a085bd42b5b30feee129962334831c3cb4ee8f60e087785` |
| `8941ae075356b456957d5678b77dd919f34eb65f` | `9b0b0fc853c8607a94ccec765b84323d25ab42e0dcb95a40e1446b5a6cf12fbd` |
| `ecc185bc77ce382d5a832656166a144822a1bc6a` | `9d468378af13b1438227d93c5b166ab638ce5a19adfe2ead3857ee4f6085d1a8` |
| `f75cbb4d0efd11034adb3d1dd826df9bb993ecf0` | `ddceaf502589ea760a86edec8f47e3563675727f754c00888127eb4074535045` |

This preserves existing artifacts; it grants no new validation or installation
credit. Nucleus's installed copy and live use were not read or changed.

This canonical-checkout rebuild has its own hash; the experimental binary
hash remains unchanged in the evidence. No native fixture, consumer Cargo
experiment, pool initialization or installation was performed by packaging.

This optional diagnostic upgrade adds `status --capacity` to the earlier
`637a68b7…` recycling artifact. The APFS metadata correction and half-cap idle
recycling remain present. Existing receipt-v2/ledger-v1 pools reopen without
re-init or migration; legacy lease/status JSON and exits remain compatible.
The independent reviewer ran all covering selectors successfully, including
235 Rust tests (three environment-gated probes ignored) and a fresh native
CLI proof with capacity sampling and byte-for-byte non-mutation checks.
The Nucleus owner reports this diagnostic artifact installed and hash-checked,
with no adapter restart or consumer code change. The [consumer boundary](project-adoption.md#first-consumer-use-and-apfs-metadata-correction)
owns its live-pool observation and remaining first-task gate. The diagnostic
does not expand the pool, clean packages, compact the image or change
admission/ownership policy.

Lease JSON v1 includes `lease_id`, `store_id`, `slot_id`, `incarnation`,
`project`, `workspace`, `profile`, `holder`, `state`, `reused_slot` and the
`env` object with the two exact variable names above. Release/recover returns
`{version:1, lease_id, state:"idle", replay}`. Status uses `slots`,
`payload_logical_bytes`, `payload_unique_allocated_bytes`, `coverage`, `full`
and pool identity/cap fields. Actual serialization is JSON with quoted keys.
The profile label selects storage; callers must use the corresponding Cargo
profile themselves. Apply the returned environment through actual Effigy
projection; an outer assignment alone is insufficient for an admitted run.

Replay of the latest released lease returns success; older/evicted lease IDs
may return exit 5 without touching any new holder. Do not treat an unknown
or mistyped ID as evidence that the intended writer ended. A held compatible
project/profile returns busy rather than allowing a concurrent writer.

A recorded deletion also journals every capture it makes (oxide#155). This is the
retired shape: since oxide#190 no deletion starts a journal — a legacy record
like this one is still read and resolved by the replay below, and a resumed
deletion moves its verified root whole into the holding area. The slot's
ledger record carries the root plan — the planned private name, the root's`(device, inode)`, the verified parent's `(device, inode)` and the root's full
`st_mode`, written before the exclusive rename, then the captured phase after it
and a `removed` phase only once the root's own removal completed — beside a
record for every nested capture: its private and observed names, its verified
`(device, inode)` and full mode, and the chain of captured private names that
locates its parent. A `removed` record follows each successful unlink, and a
capture that is not the observed object is recorded as a foreign capture before
any put-back attempt. Replay never unlinks: it restores the recorded modes, puts
every recorded capture back under its observed name deepest-first, then puts the
root back with its recorded mode and retries the deletion. Only a recorded root
removal completes the record; two absent names without it stay pending, as does
an unrecorded private name or any recorded foreign capture, and new leases stay
refused. A record on anything but an `Evicting` slot, or in a shape no writer
produces, refuses the ledger read. Every journal write is atomic and synced with
the rest of the ledger, so a crash between any two steps leaves the records
findable.

If an interrupted `Evicting` deletion cannot safely remove its recorded slot
root or an owned entry, the generation remains `Evicting` and unavailable.
Status stays bounded and reports unknown accounting; an unrelated held lease
can still be released. New leases remain refused until the recorded deletion
can safely finish. A later open resumes only that recorded slot path. Do not
edit a live ledger, follow/delete a foreign target, or use TTL to clear a
refusal. Failed provisioning with files beyond the lock can also need explicit
owner recovery. The focused real run did not exercise eviction or census slot
symlinks. These limits are separate from its observed reuse result.

## Limits

- Holder ≤ 128 printable ASCII bytes without `/`
- Paths ≤ 1024 bytes, absolute, canonical, no symlinks
- Status output default 65536 bytes (minimum 64)
- Finite slots 256 (a ledger above 128 carries `capacity_policy: 4`) and a
  8 MiB ledger; new slots refuse at exhaustion rather than dropping active state
- Init refuses a declared cap that differs from the image size; pre-rule pools
  refuse as mismatched backing without migration

## Not claimed

- No general Queue, Effigy or editor integration; the consumer-owned Nucleus
  integration and its first observed task are recorded in the [consumer boundary](project-adoption.md#observed-ordinary-task-lifecycle)
- No general cross-worktree/source-invalidation guarantee beyond the recorded
  pinned dev workflow; timestamp-restored input-only changes were not isolated
- No debug retention after release; released bytes are disposable
- No publication, install, service, global settings or frozen Contract 010 change
- No automatic process-group kill, TTL release or foreign cleanup
