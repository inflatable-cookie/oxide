# One-use workspaces beside normal slots (proposed design)

Status: **design only, unimplemented, unfrozen.** No product code, proof script,
selector, native execution, live-pool read, installation, consumer change or
host setting is authorized by this document. It is the first deliverable of
oxide#138 under the ROOT design-first rebind of 2026-10-27 (Queue brief v3
`sha256:f4523cf9…`): publish a concrete design in Oxide's owning knowledge
tree, run `effigy docs:check`, publish a clean exact head, and stop for an
independent exact-head design review and the Supervisor's delegated freeze.
Implementation, the author and independent-reviewer native sessions, and the
mechanical merge hold are separate and unchanged.

**Revision 1.** Written against clean pushed `main`
`489eecc79398a1c0bfb17fd879fb5513914a6d52`, static reads only. Prerequisites
oxide#136 (child temporary files) is merged and oxide#137's design of record
([cold-slot seeding](cold-slot-seeding.md), freeze decision `37711dda`) is
merged but **unimplemented**; this design cites that contract's frozen
interface and treats its absence from source as a named dependency, never as a
shipped interface (§8). Tom approved the one-use handling via Supervisor
`73f7c3b4` / decision `4b74c0b7`; architect bindings `3f09ebf2`, `c635b791`,
`66fa8331` bind the shape, and the localized source-review amendments A1–A5
(`5eda747c`) and the wording review `7718ec1f` are applied. Neither committee
review nor this document is native qualification.

## 1. Outcome and non-goals

**What the design achieves.** A command whose canonical Cargo workspace is a
disposable temporary checkout — or whose caller explicitly opts in — keeps the
warm-build experience it has today, but:

- its slot is a **one-use** slot, governed by a bounded per-pool slice and never
  given a normal slot's place;
- no admission, retry, count, project-cap, budget or reserve path may evict a
  **normal** slot to make room for it;
- it never publishes or adopts an owned output root: its final outputs stay real
  local directories, so a disposable checkout cannot leave a durable link,
  receipt or root behind;
- it still receives conservative cold-slot seeding once that interface exists,
  and the seed's bytes are counted in the slice;
- a normal command is unaffected in its key, its warm reuse, its outputs, its
  project cap and its eviction preferences, except that an idle one-use slot is
  now the *preferred* victim before a normal victim when capacity is tight.

**Non-goals, explicitly refused here.**

- No cold-only substitute: a one-use workspace keeps warm reuse, a real slot and
  the ordinary build path. "Guarded" never means "build without a slot".
- No new daemon, broker, scheduler, cache cleanup, journal format, recovery
  engine or consumer registration.
- No change to `SlotKey.compatibility`, to the checkout identity digest, or to
  any stored slot key.
- No claim that one-use slots save disk space; APFS extent sharing between
  cloned files is unmeasured and no byte figure here depends on it.
- No universal containment of arbitrary Cargo or tool writes. The output guard
  classifies the components a command's own declaration names, refuses a link
  or non-directory there, and claims nothing about what a tool writes outside
  them (§4.7).
- No producer/consumer adoption, no install or receipt write, no live-pool
  mutation, no mode/config/scheduler change, and no release or tag.
- No relabel, relink, move or deletion of any existing receipt, link, root or
  slot in order to make a one-use workspace work.

## 2. Source, interface and format matrix

Read at `489eecc`. "Read" means this design depends on the fact as it stands;
"gap" is what the implementation must add without silently changing the read
fact.

| Fact | Where it lives today | Design relationship |
| --- | --- | --- |
| Canonical workspace root | `src/bin/oxide/identity.rs:73-93` (`locate_workspace`: `cargo locate-project --workspace`, then `fs::canonicalize`), called from `src/bin/oxide/main.rs:385` | **Read.** Classification consumes this canonical path only; a symlinked alias therefore already resolves before the test. A resolution failure keeps its existing error and is never guessed `normal`. |
| Slot key | `identity.rs:18-62` (`SlotKey::resolve_in`) | **Unchanged.** No class, temp root or guard fact enters the key. |
| Project identity | `identity.rs:214-242` (`Identity::infer`, Git root commit + prefix, else origin, else path) | **Read, not changed.** A temp clone of a repository shares its identity, which is why the project cap must become class-scoped (§5.3). |
| Control records and phase | `src/bin/oxide/store.rs:139-153` (`Phase`), `155-189` (`Record`) | **Add one field and one state fence (§5.1).** |
| Control-state numeric version | `store.rs:74` (`STATE_VERSION = 5`), `3744-3774` (`load`), `3776-3804` (`save`, which advances the number only with a named fence), `3832-3990` (`validate_state`) | **N+1 = 6**, with a durable `oxide.one-use/1` fence written before the first class-bearing record (§10.1). |
| Project slot cap | `store.rs:55` (`PROJECT_SLOT_CAP = 8`), `3129-3171` (count and replacement) | **Class-scoped:** the cap and its victim list count normal records only (§5.3). |
| Record matching, second-slot rule, sharing | `store.rs:3017-3070` (`matches`, own-record selection), `3533-3568` (`shared_candidate`) | **Class equality added to both**, so a class change is a distinct record and classes never borrow each other (§5.2, §5.4). |
| Admission of a lease | `src/lease/pool.rs:925-1070` (`lease`), `1072-1190` (`allocate`), `482-497` (`lease`/`lease_measured`/`release`) | **Read.** Restore and class-scoped victims slot in under the pool lock (§6). |
| Idle enumeration and eviction | `pool.rs:1759-1784` (`evict_idle_until_under_cap`), `1786-1806` (`evict_one_idle`), `1808-1845` (`retire_idle_slot`), `1399-1418` (`replace_idle_slot_for_admission`), `1455-1510` (`admit_unknown_removable`) | **Read and parameterized**, never reimplemented (§6.7). |
| Pool pressure figure | `pool.rs:1515-1544` (`pressure`), `1546-1578` (`recorded_total`), `1580-1680` (`slot_figure`) | **Read.** The recorded per-slot policy stays the pool figure; the one-use slice is a separate, bounded member measure (§6.4). |
| Ledger record and fence | `src/lease/mod.rs:45` (`MAX_SLOTS = 128`), `54-58` (`CAPACITY_POLICY_VERSION = 2`), `src/lease/pool.rs:169-232` (`Ledger`, `SlotRecord`, `RecordedSize`), `src/lease/mod.rs:295-435` (`SlotStatus`, `LedgerSlot`, `PassiveSnapshot`) | **Format unchanged, no bump.** Class lives in Oxide state; the library receives verified membership (§6.2, §10.2). |
| Accounting walks | `src/inventory.rs:436` (`inventory_directory`), `460` (`scope_directory`), the `(device, inode)` union; `local-lease.md#capacity-and-eviction` | **One bounded multi-root union walk added** for the slice (§6.4). |
| Owned-output receipt, prepared transition | `src/bin/oxide/outputs.rs:397-425` (`Receipt`), `225-232` (`Prepared`), `358-377` (`ReceiptState`), `64-76` (`RECEIPT_VERSION /2`, `PROFILE_RECEIPT_VERSION /3`, `MARKER_VERSION /2`, `PLAN_VERSION /2`) | **Format unchanged.** States are read exactly as today (§4.2). |
| Automatic first conversion | `outputs.rs:3843-3886` (`AutoFacts`/`auto_adopts`), `3888-3970` (`auto_adopt`), called from `main.rs:414` | **Skipped for a guarded workspace, before any effect** (§4.1). |
| Reservation and declared destination | `outputs.rs:5851` (`reserve`), `5355` (`lease_declaration`), `5265` (`planned_destination`), `6094` (`destination_facts`) | **Guarded commands reserve nothing and plan a local destination** (§4.1). |
| Binding and prepared replay | `outputs.rs:4196-4270` (`bind_for_command`), `4301-4324` (`gated_replay`), `7203-7330` (`replay`) | **Guard branch added**; the three production replay sites refuse before replay (§4.4). |
| Explicit adopt / detach / clean | `outputs.rs:2037-2100` (`adopt_command`, replay at `2099`), `3983-4030` (`detach_command`, replay at `4014`), `8450-8470` (`clean_placement`, existing `Prepared` refusal at `8467`) | **Adopt refuses on guarded; detach allowed except `Prepared`; clean's shipped refusal retained** (§4.5). |
| Custody check | `store.rs:2572` (`checkout_custody`) | **Read.** Guarded binding repeats receipt/path validation under the same binding custody (§4.6). |
| Mark spawning / release / scratch discard | `store.rs:1803-1825` (`mark_spawning`), `1826-1850` (`release`), `1878-1930` (`discard_scratch`, `flush_before_idle`) | **Read.** Class never changes at release; the slot keeps its class record (§7.2). |
| Hand-off and capability probe | `src/bin/oxide/handoff.rs:39-72` (`__oxide-build-capabilities`, `hidden`), `96` (`preflight`), called from `main.rs:203` | **Add one boolean `proof_controls`** and the installer refusal (§10.4). |
| Proof-control parsing precedent | `src/bin/oxide/config.rs:159-205` (`proof_controls`: debug-only, exact values, malformed refuses) | **Pattern reused** for the new controls (§11.1). |
| Removable registration | `src/bin/oxide/removable.rs:33-34` (`REGISTRATION_VERSION = 2`) | **Unchanged.** Registration facts are read for the unsafe-broad test (§3.3). |
| Child temporary directory | `src/bin/oxide/child_temp.rs:66-100` (`Identity`), `153-200` (`choose`, `choose_with`, `inheritable`), `main.rs:594-630` | **Unchanged, and never a classification input** (§9.1). |
| Cold-slot seeding interface | [cold-slot seeding](cold-slot-seeding.md) §3, §6.1-6.3, §8, §9 (frozen design, no source) | **Cited, not assumed present.** Named dependency (§8). |
| Idle-exec D-4 | [idle exec sessions](idle-exec-sessions.md) §14 (option (c): preserve #136, no slot or temp custody change) | **Preserved**; one-use adds no custody change (§9.3). |
| Installer capability read | `scripts/oxide-install.py:207-219` (`state_capabilities`), `352-372` (receipt fields) | **Add the `proof_controls` refusal**; synthetic tests in `dogfood:scripts:test` (§11.4). |

## 3. Workspace classification

### 3.1 What is classified

Exactly one thing: the **canonical Cargo workspace root** produced by
`identity::locate_workspace` (`main.rs:385`). Classification happens once per
command, after the pool/registration selection (`main.rs:405`) and **before**
`outputs::auto_adopt` (`main.rs:414`), the first step that can write an
owned-output effect.

It is never derived from, and never reads:

- a child temporary path, an inherited `TMPDIR` value used for a child, or the
  `<slot>/scratch` directory (`#136` chooses that *after* the lease);
- a slot directory, project folder, profile label, rank, mtime, file name or
  pool shape;
- a receipt, link, root marker or output plan;
- the current working directory, when it differs from the workspace root
  (`--manifest-path` may point elsewhere; `locate_workspace` answers for the
  manifest, and that is the classified root).

A child environment can therefore never make a normal workspace one-use, and a
one-use workspace is still one-use when its children receive a pool-resident
temporary directory (§9.1).

### 3.2 Candidate roots

Four candidate sources, in this order, each read as raw `OsString`/`OsStr`
bytes with **no lossy conversion** (`OsStrExt::as_bytes`, matching the
repository's existing path handling):

| # | Source | Absent/empty |
| --- | --- | --- |
| 1 | literal `/private/tmp` | never absent |
| 2 | literal `/private/var/folders` | never absent |
| 3 | inherited `TMPDIR` (this command's environment) | not a candidate |
| 4 | inherited `OXIDE_TASK_SCRATCH_ROOT` | not a candidate |

`TMP` and `TEMP` are deliberately not candidates: `#136` publishes the chosen
value on all three aliases, so a conflicting alias is not evidence about where
a workspace lives. The macOS user default `TMPDIR`
(`/var/folders/…`) canonicalizes to `/private/var/folders/…`, which is why the
literal form is listed.

### 3.3 Canonicalization, deduplication, unsafe-broad dropping

For each candidate:

1. the value must be a nonempty absolute path; a relative or empty value is
   **dropped with a bounded escaped reason**, not interpreted against the
   working directory;
2. `fs::canonicalize` is attempted, following resolvable symlinks; an
   unresolvable, non-directory or unreadable path is **dropped with a bounded
   escaped reason**; a canonical root must be an existing directory;
3. canonical roots are deduplicated by their byte path (one entry per real
   directory);
4. a root is **unsafe-broad** and is dropped, with a bounded escaped reason,
   when it is:
   - the filesystem root `/`;
   - a volume root, meaning exactly one component below `/Volumes`
     (`/Volumes/<name>`), including a mounted drive's root;
   - the account home (`HOME` as this command sees it — the proof-controlled
     home when a proof sets one — canonicalized when possible, and the
     account-home default when configured);
   - an **ancestor of** a registered pool payload, or a path **inside** one. The
     payloads are the internal pool's payload root and the removable
     registration's recorded mount point plus payload directory, read
     effect-free from the selection this command already made;
5. **Every dropped root is reported exactly once** as one bounded line, e.g.
   `oxide: one-use root skipped: /private/tmp/repo (inside a registered pool)`,
   with the reason text escaped through the existing `wrapper::shown` helper.
   Dropping is per root: all remaining accepted roots still apply.

**Unavailable or corrupt pool facts never justify guessing a safe root.** If
the internal pool path or the removable registration cannot be read well enough
to decide the ancestor/inside test, the candidate root is **dropped** (reported
with its reason); it is not assumed safe. This is the conservative direction:
an unclassified workspace stays `normal` and keeps today's eviction behaviour,
whereas a wrong "safe" root would mark a normal checkout as one-use and change
its outputs and slot class.

A dropped root is **neither classification authority nor a universal
link-denial prefix**: it never matches a workspace, and it never becomes a path
prefix that a later check treats as "owned by a temp root". Only accepted roots
are used, and only by the component test below.

### 3.4 Membership: strict component descendant

The canonical workspace matches an accepted root only when

```text
workspace.strip_prefix(root).map(|rest| !rest.as_os_str().is_empty())
```

holds with **path components** — never a string prefix. So `/private/tmpfoo` is
not under `/private/tmp`, `root` itself does **not** match (a command whose
workspace *is* `/private/tmp` is not a one-use workspace; it is a pathological
checkout that this design leaves normal), and `..` can never appear because both
sides are canonicalized. Literal symlinked aliases **are** recognized, because
`locate_workspace` already returns the canonical root: a command invoked
through `/tmp/alias` where the real workspace lives under `/private/tmp` matches
the real path.

### 3.5 Explicit opt-in

The only production opt-in is the exact value:

```text
OXIDE_ONE_USE_WORKSPACE=1
```

Any other value (`0`, `true`, an empty string, arbitrary text) is **ignored and
reported once** as an ignored value; it is never an opt-out and never a refusal.
The opt-in can only add one-use classification; it can never remove the
temporary-membership classification.

### 3.6 Class and guard are computed separately

Two booleans, deliberately not one:

| Predicate | Definition | What it controls |
| --- | --- | --- |
| `one_use` | `membership || opt_in` | the **admission class**: which slot belongs to the one-use slice (§5, §6) |
| `guarded` | `membership || one_use` | the **local-output guard** (§4) |

`guarded` is implied by membership alone, so a proof control or future caller
that *forces* the admission class to `normal` still cannot disable the local
output guard. Conversely, explicit opt-in forces both. Class never weakens the
guard, and the guard never changes a slot's class.

### 3.7 Order in the command

```text
main.rs:203   handoff::preflight            (state/registration version gate)
main.rs:385   identity::locate_workspace    (canonical workspace)
main.rs:405   removable::select             (pool + effect-free registration facts)
              classify workspace            (new; §3)
main.rs:414   outputs::auto_adopt           (skipped when guarded)
main.rs:430   outputs::lease_declaration    (guard branch: local declaration)
main.rs:444   outputs::reserve              (skipped when guarded)
main.rs:456   outputs::planned_destination  (guard branch: no owned destination)
main.rs:462   SlotKey::resolve_in           (guard leaves destination empty)
main.rs:487   lease_with_gate               (class-scoped admission)
main.rs:522   outputs::bind_for_command     (guard branch: local checks only)
main.rs:594   child_temp::choose            (unchanged; never a classifier)
main.rs:650   store.mark_spawning           (unchanged)
```

Classification precedes every output, admission and seeding effect, and the
existing pre-effect version preflight (`handoff::preflight`) still runs before
all of them. `locate_workspace` keeps its current error on failure; the command
fails exactly as it does today, and no class is guessed.

### 3.8 What classification does not decide

- It does not decide whether a build is allowed, which pool is selected, which
  profile or target is used, or where final outputs go beyond "locally".
- It does not delete, create, rename, link, reserve, or read a slot payload.
- It does not read or write the receipt, and it does not require one.
- It does not look at a child temporary directory, and it is not re-evaluated
  when the child environment is built.

## 4. Guarded local outputs

### 4.1 Why the guard sits before the reservation and the key

The first effect a leased command can perform on final outputs is
`auto_adopt` (`main.rs:414`), which may publish a link, a root, a receipt and a
downgrade fence. For a guarded workspace it is **skipped entirely and silently**
except for the bounded local-output note below: no barrier, no fence, no
receipt, no root, no link, no notice that an adoption was skipped.

The reservation (`outputs::reserve`, `main.rs:444`) is skipped: a guarded
command never reserves a future root, so it never occupies a nonce or a
registry row. `outputs::lease_declaration` and `outputs::planned_destination`
take a guard branch that produces a **local** declaration (every relevant member
is `real`), so `destination_facts` returns no owned destination and the slot key
is exactly the key an unconverted checkout has. This preserves the existing
rule that a slot's key describes where Cargo writes: a guarded slot is keyed for
a local `target`, and nothing later repoints it.

### 4.2 Receipt and path states

The receipt is read through the existing bounded validation
(`Context::receipt`: private file, schema version, checkout identity,
generation, nonce and root checks). Absence is allowed. The guard then decides
from both the receipt state and the live paths, for **every relevant
component** (§4.3):

| Receipt state | Live component | Guarded command |
| --- | --- | --- |
| absent | absent or a real directory (empty or nonempty) | **runs** with local outputs |
| absent | a link of any kind (owned, foreign, dangling) or a non-directory | **refuses**, naming the path and its kind; nothing changed |
| valid `Detached` (`selected = None`, `prepared = None` for every placement) | absent or real directories | **runs**; the receipt's historical roots are **not** required to exist and are never re-selected or re-linked |
| valid `Detached` with any inserted link | link present | **refuses** (§4.2 rule above); it is not a repair opportunity |
| `Committed`, placement selected | every relevant component is the exact owned link to that member's recorded destination | **refuses**: a guarded workspace writes local outputs; the existing ownership-checked `oxide outputs detach` is the one safe verb (it keeps roots and restores real empty directories) and this refusal names it |
| `Committed`, per-member local fallback (real nonempty member) | that component real and nonempty | **runs** locally for that member; an existing real nonempty directory is never converted |
| `Committed`, one placement owned and another real | mixed | **refuses**: the command could write through the owned component |
| `Prepared` (any placement) | anything | **refuses before replay** with the exact diagnostic (§4.4) |
| malformed, unsupported version, mismatched checkout identity, `Pruning`, or an unsettled prune intent | — | **refuses** through the existing validation/refusal, unchanged |
| `Pruned` | — | treated as no selection; the live-component rows above decide |

Two properties are load-bearing. A **committed exact owned link is never
relinked, moved, adopted, converted or deleted** to make a guarded build work;
the design has no data movement at all. And **no receipt or link is ever
deleted** to make one-use work.

### 4.3 Relevant components

The guard inspects, with `symlink_metadata` (no follow), this bounded set:

- `<workspace>/target`;
- every profile member the command's own layout declaration names, using the
  shipped classifier (`outputs::lease_declaration` → `layout::classify`):
  `debug`, `release`, `<triple>/debug`, `<triple>/release` at most.

Each must be absent, or a real directory (checked without following a link).
A symbolic link — owned, foreign or dangling — and any other file type
**refuses**. An unreadable path refuses. The inspection is metadata only: no
directory listing, no recursion, no payload walk, no link followed.

### 4.4 `Prepared`: exact diagnostic at all three replay sites

Today an interrupted transition is replayed at three production sites:

| Site | Today |
| --- | --- |
| explicit adopt (`outputs.rs:2099`) | `replay(&context, receipt)?` |
| explicit detach (`outputs.rs:4014`) | `replay(&context, receipt)?` |
| command binding (`outputs.rs:4253` → `gated_replay` at `4301`) | `replay(context, receipt)` when custody is `Clear` |

For a **guarded** workspace the guard runs **before** each of them and refuses
with exactly this one line, before any read of a sibling, marker or root and
before any rename or write:

```text
prepared output transition unresolved; this guarded workspace cannot replay it; nothing changed
```

The receipt bytes, the live link or absence, the sibling, the root marker, the
root tree and the generation are all left exactly as found (the native case
compares a bounded before/after digest). **No adopt or detach repair advice is
attached**, and no new recovery mechanism is introduced. The only recovery is an
operator's explicit, deliberate action outside the guarded workspace, or running
the command from the now-normal workspace the transition belongs to. `clean`
keeps its **existing** refusal (`outputs.rs:8467`), whose advice leads to that
same explicit guarded refusal; changing that wording is out of this design's
scope and is recorded as a known wording residual (§12).

### 4.5 Adopt, detach, clean, plan and status

- **`oxide outputs adopt`** on a guarded workspace **refuses before any
  effect**, naming that this workspace is one-use/temporary and cannot own an
  output root. It does not replay, migrate, reserve, fence or publish anything.
- **`oxide outputs detach`** remains available and safe exactly as shipped: for
  a `Committed` placement whose live path is the exact owned link it becomes a
  real directory again (profile layout: every owned member link becomes a real
  directory, `target` stays real) with **no root moved, copied or deleted**, and
  for a valid `Detached` receipt it is the existing no-op. `Prepared` refuses
  per §4.4.
- **`oxide clean`** keeps its shipped refusal for `Prepared`; everything else
  about clean is unchanged. Clean is not a one-use-specific verb.
- **`oxide outputs plan`** on a guarded workspace prints a bounded local plan:
  the existing mode/state lines plus one line stating that this workspace is
  temporary/one-use and that final outputs are local (no owned root is
  selected). It never recommends adopt.
- **`oxide status`** appends a bounded one-use line per affected slot or
  checkout (§10.5), never a savings claim.
- The leased command's existing one warning surface is reused for the one
  bounded note `oxide: one-use workspace: local outputs (<reason>)`, printed at
  most once; the shipped "not converted; `oxide outputs adopt` performs it"
  warning is **not** printed for a guarded workspace.

### 4.6 Races

The guard is evaluated in both places the shipped code already evaluates the
destination:

1. during planning (`planned_destination`, `main.rs:456`), where the expected
   destination is `None` for a guarded command;
2. again inside `bind_for_command` under the checkout's binding custody
   (`store.lock_binding`), where the receipt state and every relevant component
   are re-read and re-classified.

If the receipt/path state changed between the two (a concurrent explicit
detach, a peer's conversion, a planted link), the second check refuses with the
existing "this checkout was … while this command waited … nothing was built"
shape or the guard's own refusal. The command never proceeds on the strength of
the first read. `bind_for_command`'s `expected = None` path already exists
(`keyed_state_matches(None, expected)`), so a guarded command fails closed if a
receipt appeared.

### 4.7 What the guard does not claim

- It does not contain arbitrary writes. A tool that creates
  `<workspace>/target/<x>` as a link after the check, or writes outside
  `target`, is outside this guarantee.
- It does not verify the contents of a real local directory, and it does not
  delete anything there. "Real nonempty stays local" is the whole rule.
- It does not make a guarded checkout's local `target` private or exclusive; two
  concurrent commands in one checkout remain governed by today's binding lock
  and clean rules.

## 5. Slot class in Oxide state

### 5.1 The record field and its fence

`Record` (`store.rs:155`) gains:

```text
class: WorkspaceClass      // serde enum: "normal" | "one_use"
class_reason: Option<String>  // bounded diagnostic, <= MAX_DIAGNOSTIC_BYTES
```

`class` is `#[serde(default)]` so a legacy document parses; an **unknown
variant refuses** through serde (`deny_unknown_fields` plus an unknown enum
variant both refuse), and `validate_state` additionally refuses a class value it
does not know, a reason over its bound, and a `one_use` record whose reason or
facts are not bounded text.

The class is durable before it is trusted: `State` gains a forward-only fence

```text
one_use_workspaces: Option<String>   // "oxide.one-use/1"
```

which is written **before** the first class-bearing record, exactly as
`recorded_accounting` (`oxide.accounting/1`) precedes the library's
policy-2 write. `save()` (`store.rs:3776-3804`) advances the numeric version to
**`STATE_VERSION = 6`** when that fence is present; version 5 remains the
predecessor for a pool that never meets one-use (§10.1).

### 5.2 Class is fixed; a class change is a different record

- Class is written when a record is created and **never relabelled**. There is
  no code path that changes an existing record's class.
- Record matching (`store.rs:3017-3030`) adds class equality:
  `matches(record) = same checkout && same compatibility && record.class ==
  incoming.class`. An existing record of the other class is **not** reused,
  relabelled, evicted, shared or released by this command.
- A class change therefore produces a **distinct record** with its own affinity
  directory and its own slot. The compatibility digest itself is unchanged (a
  class is never part of the key); only the record lookup refuses to match
  across classes. The old record keeps its slot under its own class and its own
  rules (normal: project cap and ordinary eviction; one-use: the slice).
- A slot's class is therefore a function of exactly one record's immutable
  field. It is **never inferred** from a directory name, a project folder, a
  profile label, a rank, an mtime or a payload shape.

### 5.3 The eight-slot project cap counts normal slots only

`project_slots` (`store.rs:3129-3145`) and the replacement candidate list
(`store.rs:3100-3120`) filter `record.class == Normal`. Consequences, all
deliberate:

- a one-use slot never consumes one of a repository's eight normal slots;
- deleting a one-use slot can never relieve the normal project cap — the
  replacement list contains no one-use record, so the cap path can only evict a
  normal slot (and refuses honestly when none is safe);
- the class-scoped identity filter is by *repository* identity, so a temporary
  clone of the same repository no longer competes with its normal checkouts for
  the cap;
- `MAX_SLOTS = 128` still bounds every slot, one-use included.

### 5.4 Sharing is class-matched

`shared_candidate` (`store.rs:3533-3568`) adds class equality, so the freshness
gate's cross-checkout share can take another checkout's idle slot only of the
same class. A normal command never borrows a one-use slot (whose build may be
about to be deleted) and a one-use command never borrows or relabels a normal
slot. Class-matched sharing between two one-use checkouts is allowed and keeps
today's gate, origin and custodian rules unchanged.

### 5.5 Bounded membership for the library

Under the admission lock (`store.attempt`, `store.rs:2978`) the front door
derives, from the control state and the ledger view it already reads:

```text
members = for each record with class == OneUse:
              for each ledger slot whose project == affinity_dir(record.id):
                  (slot_id, incarnation)
```

bounded by `MAX_SLOTS` (128) pairs and encoded as a fixed-size vector. Each pair
is verified by the library against the ledger under the pool lock (§6.2). A
slot with no matching record, a record with no slot, a stale incarnation or an
unreadable ledger row is **class-unknown**, never a member and never free
quota.

### 5.6 Class-unknown orphans

A ledger slot whose project has no Oxide record (a foreign or orphan slot, or a
slot left after its record was forgotten) is class-unknown:

- it is **never** an `Only` or preferred victim;
- it is **not** counted in the slice, so it is not presented as free quota;
- under a normal admission's `Any` fallback it keeps today's #118 whole-idle
  eligibility and the library's ownership/accounting checks, exactly as before;
  this design does not newly pin it.

### 5.7 Legacy records

A record written before this feature has no class. It reads as **protected
normal**: normal slots are never slice-evicted, never counted against the
one-use slice, and keep their project-cap behaviour. Its next command in a
temporary workspace classifies `one_use`, which differs from the record's
class, so it takes a **distinct record** and the legacy record's slot stays
normal under today's rules. Nothing is relabelled and nothing is deleted to
make the new class work.

## 6. Library admission protocol

### 6.1 Request additions

`LeaseRequest` (`pool.rs:37-70`) gains three fields; every construction site
must adapt deliberately (`store.rs:3255-3280` is the product one):

```text
class: AdmissionClass,          // Normal | OneUse  (from §3)
victims: VictimPolicy,          // Any (default, today's behaviour)
                                // Prefer(&MemberSet)  -- one-use first, then Any
                                // Only(&MemberSet)    -- one-use only
slice: Option<OneUseSlice>,     // Some for every new-slot admission and every
                                // OneUse admission; None where the brief
                                // forbids a measurement (normal warm reuse)

pub struct MemberSet(/* at most MAX_SLOTS (slot_id, incarnation) pairs */);
pub struct OneUseSlice {
    pub max_slots: usize,       // 16
    pub max_bytes: u64,         // min(admission_budget / 10, 32 GiB), at least 1
}
pub enum RestoreOutcome { NotNeeded, Within { bytes: u64 }, Over { bytes: u64 },
                          Unmeasured { reason: String },
                          Full { bytes: Option<u64>, limit: u64, held: usize } }
```

`Any` is the constructor default so every other library caller keeps today's
behaviour byte for byte. `Prefer` and `Only` take a borrowed `MemberSet`;
`Membership` is validated before use, so a policy can never name a slot the
ledger does not confirm.

Layer split, deliberately: the library reports **ledger facts** (measured union,
bound, how many non-reclaimable members are `Held`); the front door decides
whether a held member is positively progress-capable (§6.9), because phases,
record locks and custody are Oxide control-state facts the library never reads.

### 6.2 Membership binding and verification

Under the pool lock, before any victim decision:

1. each pair is looked up in the ledger; a ledger slot with that `slot_id` must
   exist and its `incarnation` must equal the pair's. `(slot_id, incarnation)`
   is the library's whole binding: the front door derived the pair from a
   record's own affinity directory under the control-state admission lock, and
   the library confirms the ledger identity it will act on;
2. a pair that does not verify is **dropped** from the candidate set (and
   reported as unverified in the restore observation); it is never replaced by
   another slot;
3. a verified pair becomes an **accounting member** immediately, but is an
   **eviction candidate** only while it is also `Idle`, `lease_id` is `None`,
   no donor pin covers it (§8.2), and the existing
   `validate_idle_slot_scope` ownership/device/accounting checks pass;
4. `Held`, `Retired`, `Evicting`, interrupted, unknown-custody and pinned
   (donor-locked) members continue to **count** toward the slice, count and
   known overage; they are never made free quota because they cannot be
   deleted.

The mapping is stable because the front door holds its control-state admission
lock (`store.attempt`, `store.rs:2978`) across the whole library call, so no
other Oxide command can create, release or evict a slot of this pool while the
membership is being used. The library itself reads no Oxide record and cannot
verify a record lock; that is the caller's custody, stated here so no reader
assumes the library proves it.

A safety failure on a selected victim (a foreign entry, a device mismatch, an
incomplete walk) **stops** the decision with the shipped error. It never skips
to a different, less-checked victim.

### 6.3 Restore runs before admission

`lease` (`pool.rs:925`) gains, in this order:

1. **Restore**, when `slice.is_some()`:
   - if the member set is empty: no walk, no write, outcome `NotNeeded`
     (a pool with no one-use slot pays nothing);
   - otherwise measure the bounded member union (§6.4), then reclaim eligible
     idle one-use victims from the **oldest** `(rank, slot_id)` first while
     either `member_count > 16` or `union_bytes > max_bytes`, re-measuring after
     each deletion (the same loop shape as
     `evict_idle_until_under_cap`, with the member predicate);
   - every deletion goes through the shipped `retire_idle_slot`
     (`pool.rs:1808`), so rank headroom, journalled capture, ownership and
     `Evicting` reconciliation are exactly today's;
2. **warm reuse**: the shipped idle-search branch. For `Normal` nothing
   changes — no slice measurement, no member walk (the #110 target-only path);
   for `OneUse` the reuse search requires the candidate slot to be a verified
   member and the restore outcome not to be `Over`/`Unmeasured` (§6.9);
3. **new slot**: `allocate` (`pool.rs:1072`). The shipped order is kept: an
   optional named project-cap replacement, then the count ceiling
   `MAX_SLOTS - 1`, then the pressure decision. Each of those now takes the
   admission's policy (§6.7).

So restore runs before **every** new-slot admission (normal or one-use) and
before **every** one-use admission including warm reuse, and never on a normal
warm reuse.

### 6.4 Measuring the member union

The pool figure stays the recorded per-slot sum
(`recorded_total`, `pool.rs:1546`), unchanged. The slice needs a different
number: the unique allocated bytes of the member slots **as one union**, because
the brief requires a cross-member hard link to be counted once.

New bounded function in `src/inventory.rs` (a mode of the existing walker, not a
new walker): `union_sizes_with(roots: &[PathBuf], limits) -> Sizes`, which walks
each root with **one shared `(device, inode)` set**, one shared entry budget,
one shared depth limit and one shared cooperative deadline, never follows a
link, never crosses a mount and refuses a duplicate or non-directory root.
Bounds are the existing admission walk limits (`pressure_limits()`, 60 s
internal / 5 s removable, 2,000,000 entries, depth 64 below each slot root).
Partial coverage is reported as partial with its causes and is **never** read as
zero.

Per-member recorded figures are deliberately **not** used for the union: a
record is per slot and counts hard links only within its own walk, so summing
records would overcount a cross-member pair. A current record is still a cheap
hint for *which* slots to expect bytes in, never for the slice figure.

### 6.5 Finite slice bounds

| Bound | Value | Why |
| --- | --- | --- |
| One-use slots per pool | **16** | hard count ceiling, independent of `MAX_SLOTS` (128) and the normal eight-slot project cap |
| Slice bytes | **min(admission_budget / 10, 32 GiB)**, at least 1 | a tenth of this admission's effective soft budget, capped so a large pool cannot reserve an unbounded disposable region |
| Member set size | `MAX_SLOTS` (128) pairs | the ledger's own slot bound |
| Member roots walked | at most the member count (≤ 16 slots) | one bounded walk, shared budget |
| Diagnostic reason | `MAX_DIAGNOSTIC_BYTES` | the shipped field bound |

The slice is an **admission bound**, not an OS ceiling and not a runtime limit: a
running build's writes may exceed it, exactly as the pool budget may be exceeded
while a build writes. No "Fresh", savings or free-space guarantee is implied.

### 6.6 Warm reuse and new slots

| Admission | Restore | Slot chosen | Notes |
| --- | --- | --- | --- |
| normal, warm reuse | **none** | shipped #110 target-only walk | never measures members; never reports slice bytes it did not observe |
| normal, new slot | yes (`Prefer`) | shipped order, class-aware victims | may reclaim idle one-use slots first; continues with a reported overage when it must |
| one-use, warm reuse | yes (`Only`) | verified member slot only | a non-member idle slot is refused, never relabelled |
| one-use, new slot | yes (`Only`) | new slot, count < 16, union within the slice | refuses or waits per §6.9 |

A guarded-from-membership command whose admission class is forced `normal` by a
proof control is the "normal" rows; the local-output guard is unaffected (§3.6).

### 6.7 Every pressure path, named

`Prefer` = "an eligible verified one-use idle member first (oldest rank first),
then today's Any order". `Only` = "an eligible verified one-use idle member
only; if there is none, this path fails exactly as it fails today when no victim
is eligible".

| Path (current source) | Normal admission | One-use admission |
| --- | --- | --- |
| slot count ceiling `evict_idle_until(MAX_SLOTS - 1)` (`pool.rs:1084`) | `Prefer` | `Only` (a normal victim is impossible) |
| byte budget `evict_idle_until_under_cap` (`pool.rs:1759`) | `Prefer` | `Only` |
| project-cap replacement `replace_idle_slot_for_admission` (`pool.rs:1399`) | named victim from the **normal-only** front-door list, re-verified as a non-member | not reachable (one-use is not subject to the project cap) |
| removable reserve route `admit_unknown_removable` (`pool.rs:1455`) | `Prefer` | `Only` |
| post-eviction pressure re-read (`allocate`, `pool.rs:1088-1115`) | unchanged; a `Full` that remains is today's `Full` | unchanged; a `Full` that remains is a safety refusal, never a normal victim |
| retry after a victim refusal | stops with the shipped error (`Busy`/`unknown_accounting`) | stops; never falls through to a normal victim |
| unknown accounting (`recorded_total`, `pool.rs:1546`) | unchanged refusal (`unknown_accounting`) | unchanged refusal |
| count replacement with only bound gaps on a removable pool (`pressure_known_or_bounded_removable`) | unchanged | unchanged |

No row ever lets a one-use admission select a normal victim, and no row lets a
normal admission be *stopped* solely because the one-use slice is over.

### 6.8 Normal admission continues

If restore cannot bring the one-use slice within its bound (all remaining
members are held, retired, evicting or pinned), a **normal** new-slot admission:

- reports a bounded overage line naming the measured union, the bound and the
  protected member count (or `unmeasured` with its reason);
- preserves every protected slot;
- continues into the unchanged ordinary count/byte/reserve decision, which may
  still evict by `Prefer` where a member is eligible, and may still refuse for
  the ordinary reasons (`Full`, `LedgerExhausted`, `unknown_accounting`).

An incomplete union walk is reported as `unmeasured` (never as zero and never as
an exact overage), and does not by itself fail a normal admission.

### 6.9 One-use full: wait for progress, never wait indefinitely

After restore, a one-use admission is **full** when 16 members exist, or the
union exceeds the slice with no eligible member left to reclaim, or either
number is unknown. The library reports `RestoreOutcome::Full { bytes, limit,
held }` (or the shipped `AccountingIncomplete`) and the front door decides:

- if `held > 0` **and** at least one held member's owning record is positively
  progress-capable — its record lock is held by a live process, its phase is
  `Acquiring`/`Spawning`/`Running`, and the custody facts do not report unknown
  writer closure — the command takes the existing bounded wait (`Busy` →
  `Wait::Uncertain` for that specific member). That is a wait for a holder that
  can finish, not for a deletion a safety gate forbids;
- otherwise (no held member, no progress-capable holder, or an unknown
  measurement) the admission **refuses** with a dedicated bounded error naming
  the measured
  union/limit and the protected members. There is no indefinite wait and no
  fall-through to a normal victim.

This is a new public error variant (`LeaseError::OneUseBudget { measured:
Option<u64>, limit: u64 }`, reason `one_use_budget`, exit code 3 alongside
`Full`/`unknown_accounting`). It is listed for exact review because it is a
public type change; the alternative — reusing `Full` — would print a false "pool
cap" message.

### 6.10 Locks, ownership and failure-stop are unchanged

- The membership set is read by the front door under its **admission lock**, and
  every pair is re-verified by the library under the **pool lock**. The front
  door's record lock stays held for the whole command, so the mapping cannot be
  repointed underneath.
- Lock order is unchanged: admission → record lock → pool lock, and the
  library's internal order (pool lock, then capture journal) is untouched.
- `validate_idle_slot_scope` (device, ancestry, recorded root, incarnation,
  project, profile), `ensure_capacity_policy`, `next_rank`, the journalled
  capture, `remove_owned_slot` and the `Evicting` reconciliation are used
  exactly as shipped. No new deletion machinery, no new capture name, no new
  journal.
- Any safety failure on a selected victim stops; it never substitutes.

### 6.11 No library format change

The ledger format and `CAPACITY_POLICY_VERSION = 2` are **unchanged**, and this
design deliberately does not add a ledger field, because:

- a slot's class is a durable function of exactly one Oxide record's immutable
  field, read under the admission lock;
- every class-scoped library operation carries a bounded membership list that
  the library verifies against the ledger itself, so no on-disk class fact is
  needed to make a safe decision;
- an unverified, missing or stale mapping is class-unknown, which is never a
  victim and never free quota;
- a different library caller that passes no membership gets exactly today's
  `Any` behaviour.

**Stop condition.** If implementation finds a class-scoped decision that needs a
class fact *inside* the ledger (for example, to count a member whose Oxide
record is gone while its slot survives), the work stops and returns to design;
the class must not be silently duplicated into the ledger, and a ledger fence
must not be bumped without its own review.

## 7. Crash, closure and recovery

### 7.1 Acquiring and spawning crashes

A crash while `Acquiring` or `Spawning` keeps today's rules exactly:
`recover_settled` (`store.rs:3374-3420`) decides `Keep`/`Reset`/`Retire` from the
record, the ledger and positive group emptiness; a lease with no trusted origin
stays untrusted (`Prior::Unknown`) and is evicted whole by the next admission;
an unknown writer custody refuses. The class is part of the record, so a
recovered record can never be reclassified by the recovery: recovery neither
writes nor changes `class`.

### 7.2 Release, scratch discard and flush

`Store::release` (`store.rs:1826-1850`) is unchanged in order and outcome: owned
child scratch is discarded after positive closure, a removable pool flushes
before idle (and retires the slot instead of idling it on a flush failure), the
library release records its closed observation, and the origin is written.
None of those writes touches `class`. A one-use slot therefore goes idle as a
one-use slot with its closed record, and a failed flush retires it exactly as
any other slot.

### 7.3 Deletion and retained generations

Deletion is the shipped `Evicting` transaction with its journalled capture
(`pool.rs:1808-1900`, `reconcile_evicting`). A retained interrupted generation,
an `Evicting` member, a quarantined lost slot and an untrusted member all keep
counting toward the slice while they exist, and are never candidates. The slice
therefore never turns "cannot delete safely" into "free bytes".

### 7.4 No class is ever shared, borrowed or converted

There is no path that changes a record's class, moves a slot between classes,
makes a one-use slot reusable by a normal command (or the reverse), or converts
a one-use slot into a normal donor. The unit and native cases in §16 assert
each direction, including after a crash: a recovered one-use record keeps its
class and a recovered normal record keeps its own.

## 8. Interaction with cold-slot seeding (oxide#137)

### 8.1 Position

The frozen seeding design places its transaction **after** the lease and the
output binding, and **before** `child_temp::choose` and `mark_spawning`
(`cold-slot-seeding.md` §6.1; `main.rs:522-650`). This design adds nothing at
that position and moves nothing there: a guarded command has no owned
destination to bind, and the binding step becomes the local guard (§4), so the
seeding transaction's own preconditions (a new empty recipient slot, no child
temporary directory yet, no writer) hold identically.

### 8.2 One-use receives seeds

Guarded local outputs and the one-use admission class **do not disable
seeding**, and seeding eligibility never disables a one-use protection: the
slice, the local outputs and the class-scoped eviction hold whatever the
seeding decision is. A one-use recipient is a `Prior::Fresh` new slot like any
other and takes the frozen design's skip/cold or clone/verify path.

**Donor pin versus the slice, named here as a required interface property.**
The frozen design pins a donor by taking the donor's record lock before the pool
lock (`cold-slot-seeding.md` §7) and never mutates it. A one-use slot may be a
donor. Because the library reads no Oxide record, a record-lock pin is invisible
to it, so either (a) the front door must omit a donor-pinned member from the
eviction candidate set it passes for that admission, or (b) #137 must publish a
fact the front door can read under its own admission lock. Whichever is chosen,
the pinned member's bytes still count toward the slice, the pin never touches a
normal donor's LRU or release facts, and a pinned member is never "free"
quota. The implementation brief must bind the chosen mechanism to #137's actual
pin; a pin that a concurrent admission can delete is a defect, not a limit.

### 8.3 Slice accounting includes seed bytes

The frozen design's failure path abandons a seed (cold build, or refusal) and
never publishes a partial one (`cold-slot-seeding.md` §6.5). For this design's
slice:

- the seed's cloned bytes are ordinary new inodes and are counted by the next
  member-union walk (§6.4); no early record is fabricated;
- the seed's own eligibility check gets the remaining headroom (slice minus the
  measured union). If the headroom cannot be measured, or the frozen estimate
  cannot safely fit the remaining admission policy, seeding is **skipped** with
  its own bounded reason and the command builds cold in the same slot — it never
  violates normal protection and never invents free bytes;
- an abandoned seed leaves the untrusted generation counting toward the slice
  until it is evicted or closed, exactly as any other retained member.

Actual reach is measured later; no native figure in this design claims a seed
hit or a byte saving.

### 8.4 The interface is frozen but unimplemented

Source at `489eecc` contains **no** seeding interface: there is no
`Store::seed`, no donor catalog, no sidecar, no attempt record. The frozen
contract is a design of record, not a shipped API. This design therefore:

- binds the *requirements* on that interface (recipient class, remaining
  headroom, skip-never-violate, counted bytes, abandoned-generation accounting,
  donor-pin visibility to the slice);
- names the **implementation dependency**: oxide#137 must land its interface
  before the seeding-interaction slice of oxide#138 can be implemented or
  proven natively (§15, and case 22 of §16.2);
- refuses a cold-only substitute. If the owner or the freeze prefers to land
  #138's guard/slot/admission work before #137's interface, that is a **scope
  split** requiring a new brief, not a silent no-op seam.

### 8.5 What must be re-checked at dispatch

If #137 lands before #138's implementation, the implementation brief must
re-read its actual entry point, its recipient precondition (empty `<slot>/build`),
its skip/refuse outcomes, its attempt record, its donor pin and its accounting
hooks, and bind this design's five requirements to those exact names. Drift
returns to design.

## 9. Child scratch (#136) and idle-exec D-4

### 9.1 Classification precedes the child environment

`child_temp::choose` (`child_temp.rs:153`) runs after the lease
(`main.rs:594`). The classifier runs before it (§3.7) and never reads a child
temporary path, so:

- a **normal** workspace whose children receive a pool-resident
  `TMPDIR`/`TMP`/`TEMP` stays normal (the pool-resident directory is Oxide's,
  never a workspace root);
- a **one-use** workspace stays one-use when an inherited same-volume `TMPDIR`
  is kept for its children;
- no slot, project or class decision depends on the chosen child directory.

### 9.2 The one-use slot's own scratch

`<slot>/scratch` remains an ordinary owned entry of the slot: created inside the
held slot, counted once in that slot's own accounting and included in the
member-union walk because it is inside the slot root, removed at release after
positive closure, and kept with the slot on a crash. Nothing about the slice
changes `#136`'s custody, marker, removal or refusal rules.

### 9.3 Idle-exec D-4 is preserved

D-4 (`idle-exec-sessions.md` §14) keeps slot write custody while an owned
temporary directory exists and releases nothing early. This design adds no
custody change, no permit change and no temp placement change. A `run` program
that holds an owned temp keeps its lease, so its slot — one-use or normal — is
`Held` and therefore never a deletion candidate; the slice counts it. When the
group closes, ordinary release applies.

## 10. Persistence, visibility and forward fence

### 10.1 Control state: N+1 = 6

| Item | Today | This design |
| --- | --- | --- |
| `STATE_VERSION` | 5 (`store.rs:74`), the recorded-accounting fence | **6**, the one-use fence |
| Predecessor | 4 (owned outputs /3), 3, 2, 1 | 5 becomes a readable predecessor |
| Durable marker | `recorded_accounting: "oxide.accounting/1"` | adds `one_use_workspaces: "oxide.one-use/1"`, written before the first class-bearing record |
| Version rule | `save()` advances only with a named fence | the one-use fence advances to 6; a state with neither fence stays below 6 |
| Unknown class/version | — | **refuses safely**: an unknown class variant, an unknown fence value, a missing fence at version 6, or a version above 6 all refuse before use |

Per-record migration is the serde default: a record with no `class` reads
`normal`, which is the protected class (§5.7). The fence, not the per-record
default, is what tells a *newer* binary that this state may contain one-use
records.

### 10.2 Ledger, receipts and registration unchanged

- Ledger: format and `CAPACITY_POLICY_VERSION = 2` unchanged (§6.11).
- Receipts: `oxide.outputs.transition/2` (whole target) and `/3` (profile),
  marker `oxide.outputs.root/2`, plan `oxide.outputs.plan/2` — all unchanged.
  No new receipt state, field or nonce semantics.
- Registration: `REGISTRATION_VERSION = 2` unchanged.
- Origins, status observations, prune registry, migration journal: unchanged.

### 10.3 Capability probe and the installer

`__oxide-build-capabilities` (`handoff.rs:39-72`) gains one field:

```json
{"control_state_version":6,"registration_version":2,"proof_controls":false}
```

`proof_controls` is compiled in (`cfg!(feature = "proof-controls")`), so it is
an artifact property, not an environment claim. The installer
(`scripts/oxide-install.py`) refuses a candidate whose probe output lacks the
field, does not parse as a JSON boolean, or reports `true` **before any
replacement**, and it stages its own build with the feature explicitly
disabled, recording the artifact evidence in its receipt. Older receipts that
lack the field remain valid for the existing replace-ownership match only; a
receipt without `proof_controls` cannot authorize installing a candidate
(matching the brief: "older pins lacking that artifact-probe field cannot be
installed by the revised installer"). This design performs no install.

### 10.4 Pre-effect hand-off covers both state paths

`handoff::preflight` runs at `main.rs:203`, before any store command arm, and
peeks the internal state, the removable state and the removable registration.
A version-6 (or one-use-fenced) document therefore hands off or refuses **before**
any output, reservation, receipt or slot effect on **both** the internal and the
removable path. The native case plants a version-6 document in each control
directory with no matching installed receipt and proves exit 75 with no effect
(§16.2, case 14). Pre-#122 binaries cannot be assumed pre-effect fenced; the
existing installer QA-window and old-running-binary limits are unchanged, and
the planner must confirm the installed receipt covers #122 before any host
install of this format.

### 10.5 Visibility

- **Summary / admission note**: at most one bounded line for a guarded command
  (`one-use workspace: local outputs (<reason>)`) and at most one for the slice
  (`one-use slots: <n> of 16, <measured> of <bound>` or
  `one-use slots: unmeasured (<reason>)` or `over by <measured>`). A **normal
  warm summary never claims slice bytes it did not measure**; a normal new-slot
  admission reports only what restore measured.
- **`oxide status`**: per slot, a bounded `one_use` label and, where the pool
  has members, the recorded/known slice observation or `unmeasured`; per
  checkout, the outputs line says `local (one-use workspace)` when applicable.
  Status may reconcile existing `Evicting` deletions, so it is not described as
  effect-free.
- **Seed result**: the shipped seeding skip/result line when that interface
  exists; this design adds no new seed verbosity.
- No savings, free-space or "Fresh" claim appears anywhere.

## 11. Proof-only controls and production isolation

### 11.1 The feature and its two controls

A new non-default Cargo feature `proof-controls` (default off; no other feature
enables it) gates exactly two readers, both in the front door and both behind
the feature so a release build contains no reader at all:

| Control | Shape | Rules |
| --- | --- | --- |
| `OXIDE_TEST_WORKSPACE_CLASS` | `normal` \| `one-use` | sets the **admission class only**; `normal` alone never disables the independent guard (§3.6); any other value refuses before effects |
| `OXIDE_TEST_TEMP_ROOTS` | JSON array of at most 4 UTF-8 paths, at most 4096 bytes total | **replaces the candidate list only**; every canonical/absolute/unsafe-broad rule still applies; missing uses the production roots; malformed (bad JSON, wrong type, relative/empty entry, over the caps) refuses before effects |

To preserve the existing normal-owned-link fixtures, a feature build that sets
`OXIDE_TEST_WORKSPACE_CLASS=normal` **and** `OXIDE_TEST_TEMP_ROOTS=[]` has
neither temporary membership nor opt-in, so its fixtures stay normal in both
slot and outputs. A forced `one_use` or the production opt-in still produces
local outputs.

A **feature-disabled** build never inspects either variable: setting them
changes nothing, and the production classification runs from the four
production roots.

### 11.2 Static edits to the exposed proof scripts

The exposed proof scripts that stage a **current** candidate have fixtures under
temporary or scratch roots, which would otherwise be classified one-use. The
brief's "twenty" is a floor; the inventory below is the exact set verified in
`scripts/` at `489eecc` (twenty-eight current-candidate stagers plus two
exceptions). The rule is a **static** edit to the staging call and to the
fixture child environment, plus source/covering validation — **not** a mass
native rerun:

- a current-candidate staging call adds `--features proof-controls`;
- a staged artifact that must prove `proof_controls=false` (an installer
  capability control) is built **feature-disabled** on purpose, and any
  capability assertion in that script is updated to the value that artifact
  really reports;
- each fixture child that must keep today's normal-owned-link behaviour receives
  `OXIDE_TEST_WORKSPACE_CLASS=normal` and `OXIDE_TEST_TEMP_ROOTS=[]`;
- the **new** combined matrix (§16.2) is the only place that exercises
  membership, the opt-in, a forced class and the guard together.

Exact current-candidate staging inventory (verified in `scripts/` at
`489eecc`; every one of these builds `oxide` from this HEAD, so each needs the
explicit feature decision):

| Script | Role | Edit |
| --- | --- | --- |
| `test-account-home.py` | current HEAD, debug + release artifacts | feature on the debug artifact; the release artifact stays feature-disabled because that script proves the installer's `proof_controls=false` acceptance |
| `test-accounting-guards.py` | current HEAD debug | feature + class/roots |
| `test-accounting-vanished.py` | current HEAD debug | same |
| `test-cargo-profiles.py` | current HEAD | same |
| `test-command-budgets.py` | current HEAD debug | same |
| `test-everyday-cli.py` | current HEAD debug, unstamped + stamped | feature + class/roots; its capability assertions expect `proof_controls=true` |
| `test-finder-metadata-race.py` | current HEAD debug | same |
| `test-freshness-submodules.py` | current HEAD | same |
| `test-front-door-accounting.py` | current HEAD debug | same |
| `test-init-guidance.py` | current HEAD | same |
| `test-installed-handoff.py` | current HEAD debug (plain Cargo) | same |
| `test-max-active.py` | current HEAD | same |
| `test-outputs-prune.py` | current HEAD debug (installed-handoff recipe) | same |
| `test-oxide-exec.py` | current HEAD | same |
| `test-oxide-finder-metadata.py` | current HEAD | same |
| `test-oxide-pool-layout.py` | current HEAD **(candidate role)** + fixed `BASE = 850260c` **(historical role)** | feature only on the current candidate; the base stays its own source and flags |
| `test-oxide-removable-pool.py` | current HEAD debug | same |
| `test-oxide-removable-removal.py` | current HEAD debug | same |
| `test-oxide-wrapper-bypass.py` | current HEAD | same |
| `test-permanent-outputs.py` | current HEAD + fixed `PRE_FEATURE = 8c9b5ae` | feature only on the current candidate; the pre-feature artifact keeps its source and flags |
| `test-pool-describe.py` | current HEAD | same |
| `test-pool-scale.py` | current HEAD + `git merge-base HEAD origin/main` reference | feature on the current candidate; the reference gets the feature **iff** that source's manifest declares it, otherwise unchanged |
| `test-pool-temp-scratch.py` | current HEAD debug | same (and its own `TMPDIR` fixtures must not change class) |
| `test-profile-outputs.py` | current HEAD + fixed `4a70b83` | feature only on the current candidate |
| `test-recorded-slot-accounting.py` | current HEAD debug | same |
| `test-slot-headroom.py` | current HEAD | same |
| `test-status-fast.py` | current HEAD debug | same |
| `test-storage-layout.py` | current HEAD | same |
| `test-oxide-cli.py` | current HEAD, **suspended** by oxide#102 (retired image-pool provisioning; refuses before any effect) | **no edit inferred**; its static edit is bound only after its source route is re-established, otherwise the precise held gap is reported |
| `test-storage-inventory-cli.py` | builds the inventory binary, not `oxide` | no feature edit; assert it stays unaffected |

Rules that follow from the table:

- a **fixed historical** source (`850260c`, `8c9b5ae`, `4a70b83`) never gets a
  feature its own manifest does not declare;
- a **dynamic reference** (`merge-base HEAD origin/main`) gets the feature only
  if *that* source declares it, decided at run time from the staged manifest,
  not by a blanket flag;
- the **suspended** `test-oxide-cli.py` is not reactivated and not silently
  edited; its gap is reported.

Old evidence stays historical. Each edited selector is marked in its owning
proof record/contract as **next-needed-run requalification**; no repository
planning or status mirror is created, and no "current native success" is
claimed for a statically edited selector.

### 11.3 Production isolation

- No release build contains a control reader, and no feature-off build reads
  either variable.
- The feature changes nothing about the production classification, the slice,
  the guard, the state format or the receipt format.
- Installer refusal of a `proof_controls` artifact is proven **synthetically**
  in `dogfood:scripts:test`; no native run invokes the real installer and no
  `~/.local/bin/oxide` or receipt is touched.

## 12. Counterexamples and honest limits

1. **A temp workspace inside a registered pool payload.** The payload root is
   dropped as unsafe-broad, so a checkout stored inside a pool slot is
   unclassified (normal). Deliberate: Oxide's own storage is not a workspace
   location, and marking it one-use would let a build delete its own pool
   neighbourhood.
2. **A temp workspace under an unreadable registration.** The registration facts
   cannot prove the payload test, so the candidate root is dropped; the command
   is normal. The conservative direction is a normal slot, never a wrong
   one-use classification.
3. **A workspace that *is* `/private/tmp`.** Equality is not membership; it stays
   normal. The design refuses to classify a root itself because that would make
   every child of a broad directory one-use by accident.
4. **A normal workspace whose Cargo children get a pool `TMPDIR`.** Still
   normal: the child directory is not an input (§9.1).
5. **A committed owned link in a temp checkout.** The guarded build refuses and
   names detach. Detach keeps the root and restores a real directory; the root's
   bytes stay until the operator prunes them (a separate, explicitly-authorized
   verb). This design does not delete them.
6. **A prepared transition in a temp checkout.** The command refuses with the
   exact guarded diagnostic and changes nothing. There is no automatic repair;
   the operator must act deliberately.
7. **A one-use slice full of held slots.** The command waits only for a
   positively progress-capable holder and otherwise refuses; a long-lived `run`
   server can therefore block one-use admission for its lifetime. That is the
   intended protection, not a defect, and it never touches a normal slot.
8. **Cross-member hard links.** Counted once by the union walk; per-member
   records would overcount. APFS extent sharing between `clonefile` copies is
   **not measured**, so no byte saving is claimed and the slice may be
   conservative.
9. **A one-use slot larger than the whole slice.** It is admitted while the
   slice admits it, then exceeds the bound as it writes (the bound is an
   admission bound). The next admission measures the overage and reclaims it if
   it is idle, or reports it if it is protected.
10. **A tool that writes through `target` after the guard.** Not contained
    (§4.7).
11. **`clean`'s wording.** Its shipped `Prepared` refusal still advises
    `oxide outputs adopt`, which for a guarded workspace refuses with the exact
    diagnostic. The advice chain is honest (it lands on the refusal), and
    changing that line is outside this design; it is recorded as a wording
    residual for the implementation brief.
12. **Pending papercuts.** A temp checkout's intentional Cargo escapes and the
    positively-vanished-checkout idle-slot preference (papercuts `84f2bc18`,
    `dab1b5b8`) are folded here; §17 states exactly what this design does and
    does not deliver, and neither is claimed closed by design.

## 13. Skip, refuse and fail-closed matrix

| Situation | Result |
| --- | --- |
| `locate_workspace` fails | today's error; no class guessed |
| candidate root relative, empty, unresolvable, non-directory | dropped, one bounded report; other roots still apply |
| candidate root unsafe-broad (`/`, `/Volumes/<name>`, HOME, pool ancestor/inside) | dropped, one bounded report |
| pool/registration facts unreadable for the broad test | root dropped, one bounded report; normal |
| opt-in value other than `1` | ignored, one bounded report; membership still applies |
| proof control malformed (feature build) | **refuses before effects** |
| proof controls set on a feature-disabled build | inert; production classification |
| guard: relevant component is a link or non-directory | **refuses**, nothing changed |
| guard: `Committed` exact owned link | **refuses**, names detach |
| guard: `Prepared` at adopt/detach/bind | **refuses** with the exact diagnostic, nothing changed |
| guard: malformed/mismatched/unsupported receipt | **refuses** through the shipped validation |
| explicit adopt on a guarded workspace | **refuses before any effect** |
| automatic adoption on a guarded workspace | **skipped**, no receipt write, no notice |
| normal warm reuse | unchanged, no slice measurement |
| normal new slot with slice overage | restoration + `Prefer`, then continue with a reported overage |
| one-use admission, slice full, progress-capable holder | wait (`Busy` → existing bounded wait) |
| one-use admission, slice full otherwise / unknown measurement | **refuses** (`one_use_budget` or the shipped `unknown_accounting`) |
| any class-scoped victim fails an ownership/accounting check | **stops**; never substitutes |
| class-unverified membership pair | dropped from candidates, reported; never free quota |
| unknown class value / version / fence | **refuses safely** |
| version-6 state with an older installed binary | pre-effect hand-off or exit 75; no output or slot effect |
| seed headroom unknown or unfit | seed skipped, cold build in the same slot |

## 14. Alternatives rejected

- **Class in the ledger with a capacity-policy bump.** Rejected for this scope:
  the class is an Oxide product concept whose authority is the control record,
  and every class-scoped library decision can be made safe from a verified
  bounded membership list. A ledger field would duplicate the fact, force a
  second forward fence and make a foreign library consumer's format change.
  The stop condition in §6.11 keeps the door open if implementation proves it
  necessary.
- **Classify from the child temporary directory.** Rejected: `#136` chooses the
  child directory after the lease, and a pool-resident child temp would
  misclassify a normal workspace.
- **Classify from a path prefix or a filename.** Rejected: string prefixes
  match `/private/tmpfoo`, and names are not authority.
- **Guarding by refusing temp workspaces entirely.** Rejected: it loses the
  primary use (a scratch checkout still deserves a warm build), and the brief
  forbids a cold-only substitute.
- **Letting a guarded build adopt and then detach.** Rejected: it writes a
  receipt, root, link and downgrade fence into a disposable checkout and then
  moves data the design has no authority to move.
- **One global class-aware LRU instead of a slice.** Rejected: a temp workspace's
  slot would still compete for the same count and byte ceilings, so a burst of
  temp checkouts could evict normal warm slots.
- **A per-invocation bespoke accounting or supervision harness for the proof.**
  Rejected by the validation policy: the proof is one fixed script under the
  qualified shared supervisor.
- **A cold-only rollout before seeding exists.** Rejected explicitly by the
  brief and by §8.4.

## 15. Implementation slices (not authorized by this design)

- **S1 classification and guard.** `classify`, the guarded plan/declaration/
  binding branch, the local-output refusal table, the exact `Prepared`
  diagnostic at the three sites, plan/status wording, the forward fence and
  version 6.
- **S2 record class and front-door scoping.** `Record.class`/`class_reason`, the
  fence, project-cap and sharing class filters, membership derivation,
  status/summary visibility.
- **S3 library admission protocol.** `LeaseRequest` additions, the `MemberSet`
  verification, `restore`, the union walk, the class-scoped pressure paths, the
  `one_use_budget` refusal.
- **S4 proof controls.** The feature, the two readers, the capability field and
  the installer refusal, and the static edits to the exposed scripts.
- **S5 seeding interaction.** Binds S3's headroom rule to #137's actual
  interface. **Blocked on oxide#137's implementation**; a scope split or a wait
  is required, and this slice never ships as a no-op.

Each slice is implemented, tested and reviewed under its own approved brief with
its own validation tier and native activation.

## 16. Proof plan

### 16.1 Pure checks (no child, no pool, no Cargo)

- The classification table: canonical alias, equality, component prefix
  (`/private/tmpfoo`), each unsafe-broad root, dedup, relative/empty/absent
  inputs, unreadable registration, both opt-in values, and the
  class-versus-guard matrix.
- The receipt/path guard table of §4.2, including every refusal and the exact
  `Prepared` diagnostic string.
- The state shape: a version-5 document reads as normal; a version-6 document
  without the fence refuses; an unknown class variant refuses; a class fence
  with version 5 refuses; bounded reason and membership sizes.
- The request/policy shapes: `Any` is the default, a member set is bounded, a
  policy with an empty set is a no-op, `Only` with no eligible victim fails.
- The union walker: shared `(device,inode)` set across roots, duplicate root
  refused, link not followed, partial coverage reported, entry/deadline bound.
- The slice arithmetic: `min(budget/10, 32 GiB)`, floor of 1, 16-slot count.
- The record→slot mapping: stale incarnation, missing record, missing slot, and
  a foreign project all classify unknown.
- The capability JSON shape and the installer's `proof_controls` acceptance and
  refusal matrix (pure; the synthetic `dogfood:scripts:test` fixture covers the
  same decisions without an install).

### 16.2 Native cases (one fixed script)

One short fixed script, the whole command of the qualified shared proof
supervisor, with tiny std-only fixtures, private `HOME`/`CARGO_HOME`/pool and a
`cfg-if` 1.0.0 crate seeded offline from the verified host cache for the
seeding-interface case. Fixed case list:

1. classification: canonical alias under a root; root equality; component
   prefix; `/`, `/Volumes/<x>`, HOME, pool ancestor and pool-inside all dropped
   with one report each; dedup; unreadable registration drops a root.
2. opt-in: `1` classifies, `0`/`true`/text are ignored and reported, and the
   membership classification survives an ignored opt-out.
3. repeat and class change: the same checkout used twice as one-use (warm
   reuse), then normal, then one-use again; three distinct records, no relabel,
   no borrow, no shared slot.
4. one-use admission and growth: fill 16 one-use slots on a tiny budget, grow
   one past the slice, then prove the next admission restores idle members
   oldest-first and admits.
5. normal-slot protection, count path: a 128-slot ceiling pool of tiny slots (normal and one-use mixed, one small file each); a normal admission at the ceiling reclaims an idle one-use victim first, and a one-use admission at the ceiling reclaims only a one-use victim or refuses — no normal slot is ever its victim.
6. normal-slot protection, project-cap path: at the eight-slot normal cap with
   one-use slots eligible, the replacement deletes exactly one idle **normal**
   slot (the front-door list has no one-use candidate) while one-use slots
   survive.
7. normal-slot protection, byte path: one-use members present, normal new slot
   under byte pressure reclaims one-use first, then a normal victim if still
   over — and with every one-use member held, it continues with a measured
   overage and no stall.
8. normal-slot protection, reserve path: absent-volume removable fixture; the
   reserve prune prefers one-use victims and never a normal one.
9. one-use `Only` under pressure with no eligible one-use victim: refuses, and
   every normal slot's bytes and rank are byte-identical afterwards.
10. normal warm reuse target-only: with members present, a normal warm reuse
    logs no member walk (debug walk log) and its summary claims no slice bytes.
11. one-use full: a progress-capable held member makes admission wait, then
    succeed when released; an unknown/interrupted member makes it refuse with
    `one_use_budget`; the slot bytes of every normal slot are unchanged.
12. crash and recovery: kill while `Acquiring` and while `Spawning`; the
    recovered one-use record keeps its class, the untrusted generation is
    evicted whole, and a normal record's recovery is unchanged.
13. state class/version: a version-5 fixture state reads as normal; a version-6
    document without the fence refuses; an unknown class refuses; a fenced
    version-6 state refuses an unknown version — all with no effect.
14. pre-effect hand-off, both paths: a version-6 document planted in the internal
    control and in a private removable control, with no matching installed
    receipt: exit 75 before any output, receipt, root or slot effect.
15. guarded outputs, absent/real: a temp workspace with no receipt, and with a
    real local `target`, builds locally; no receipt, root or link is created.
16. guarded outputs, valid `Detached`: runs locally, requires no historical
    volume, and a planted link at a relevant component refuses with nothing
    changed.
17. guarded outputs, `Committed` exact owned link: the build refuses and names
    detach; detach then restores real directories with the root bytes unchanged;
    the following build runs locally.
18. guarded `Prepared` at all three sites: adopt, detach and a normal command
    each refuse with the exact diagnostic, and the bounded digest of the
    receipt, link, sibling, marker and root is identical before and after.
19. permanent drive mode: a guarded temp checkout in permanent mode neither
    converts nor prints the unconverted-adopt warning, while a normal checkout
    in the same run converts (positive control).
20. proof controls: a feature build with `normal` + `[]` roots stays normal; a
    feature build with a forced `one_use` and production roots stays local;
    `normal` alone with a temp workspace stays local-output; a malformed value
    refuses before effects.
21. feature isolation: the feature-disabled artifact ignores both variables,
    classifies a temp workspace from production roots, and guards its outputs;
    the two staged artifacts report `proof_controls` true and false.
22. seeding interface (only when oxide#137's interface exists): a one-use
    recipient receives the frozen skip/seed path, its seeded bytes enter the
    next member union, and an unfit headroom skips to a cold build. **Absent
    interface: recorded `unexecuted`, which the counted native must report
    honestly.**

Every case uses its own private pool and checkout; the script's own work cutoff
is the guard deadline minus 40 s; cleanup stays inside the supervisor's owned
root. Native failure stops at the first fault; no retry is authorized by this
text.

### 16.3 Runner, bounds and activation

- Tier: **ELEVATED**. The trigger is storage deletion/recycling: the slice
  reclaims whole idle one-use slots, and the class-scoped count/byte/reserve
  paths delete owned storage. Fixture isolation does not remove that trigger, so
  the author runs one counted native and a distinct reviewer runs one
  independent counted native.
- One fixed script, the whole command of the qualified shared supervisor:
  `bounded-test.py --root-parent /private/tmp --limit 300 --scratch-budget-mib
  512 -- python3 scripts/test-one-use-workspaces.py`. The explicit internal root
  is the owner-accepted internal-placement exception (decision `bd357a4c`)
  because a private `directory-budget-v1` pool is a genuine internal pool that
  the product correctly refuses on an external volume; it stays inside one
  monitored 512 MiB owned root.
- Cumulative author bound **180 min** from actual activation, including
  authoring, admission/waits, coverings, native, cleanup and report; reviewer
  **60 min**. No historical clock is active or reset. Exactly **one** combined
  native per side, each with its own separately counted owner activation and
  headroom; no extra 6/14 selector starts.
- The script's SHA-256 and the exact case list are published in the
  implementation PR **before** activation.
- Coverings for implementation: `rust:compile`, `rust:format`, the targeted Rust
  suite under the four private temp variables, `docs:check` when docs change,
  `dogfood:scripts:test` for the installer change, source-only syntax/staging
  integrity, and `rust:compile-linux` if portable paths changed. No full `qa`,
  no unrelated completed proof repeated.
- Current interim heavy-admission disclosure and `Busy` counting remain owner
  authority at run time and must be verified, not assumed, at activation.

### 16.4 Fixed-script time and disk allocation (source-derived, UNMEASURED)

| Phase | Estimate | Basis |
| --- | --- | --- |
| Stage current feature artifact (offline, 1.97.1) | ~37 s | measured at `rust:installed-handoff` (58.3 MiB peak) |
| Stage feature-disabled artifact, shared target dependencies | ~37 s | same, with dependency reuse (inference, not a budget guarantee) |
| 22 tiny fixture cases (tiny std crates, one `cfg-if` copy) | ~95 s | ~4–6 s per case with a private pool and one Cargo job |
| Fixture creation, member walks, digests, receipt | ~20 s | bounded metadata work on tiny trees |
| Cleanup | ~15 s | inside the owned root |
| **Total** | **~204 s, upper ~260 s of 300** | arithmetic only; the script's work cutoff is ~255 s (300 − 40 s reserve − margin) |

Disk, inside the 512 MiB monitored root: staged targets ≈ 60 MiB shared (the
second build reuses dependencies); a 128-slot ceiling pool with one tiny file
per slot plus ≤ 24 medium tiny private slots ≈ 50 MiB; fixtures, logs and the
bounded receipt ≈ 20 MiB. **No sparse image, mount or removable write is
required**: the removable class cases use a private metadata registration with
an absent `/Volumes` path (`rust:drive-mode`'s fixture shape).
Total ≈ **130 MiB of 512**, with the earlier ~300 MiB reviewer estimate treated
as a conservative ceiling. These are estimates; the qualified guard measures
actual owned bytes and time and the receipt reports them, and neither figure is
an acceptance gate.

## 17. Folded papercuts

Both obligations stay in this task; no fix is claimed from design:

- `84f2bc18` — the docs must explain the intentional Cargo escapes that keep a
  disposable temp or private-target crate working. This design's §4.7 and §12
  state the boundary honestly (no universal containment) and §4.5 states the
  local-output behaviour; the operator-facing wording lands with S1's docs
  change, not here.
- `dab1b5b8` — positively vanished checkouts prefer an idle slot victim. This
  design's slice reclaim and `Prefer` policy is **slot eviction preference**,
  distinct from oxide#129's output-**root** deletion; #129 never satisfies it.
  The exact preference for a vanished checkout is delivered only if S2/S3 land
  and the native case proves it; otherwise the residual is preserved for a
  scoped follow-up rather than silently closed.

Held/unknown/retired writer protection, the canonical-path rule and the
positive-absence policy are unchanged by either papercut.

## 18. Independent review questions

1. Is the class-versus-guard split (§3.6) sufficient to keep a proof-forced
   `normal` class from disabling the local-output guard, including at the
   binding re-check?
2. Is the classification's conservative dropping of a root when the pool facts
   are unreadable the right direction, and is every dropped root reported
   exactly once without a universal prefix denial?
3. Does the guard's relevant-component set (§4.3) match what the layout
   declaration can name for every supported command family, and is refusing a
   committed owned link (rather than detaching automatically) the right call?
4. Is the exact `Prepared` diagnostic placed before every effect at all three
   sites, and is the retained `clean` wording acceptable as a residual?
5. Is "no ledger field, no capacity-policy bump" (§6.11) genuinely safe for
   every class-scoped decision, and is the stop condition precise enough?
6. Are the slice restore, `Prefer`/`Only` and one-use-full rules complete for
   the count, project-cap, byte, reserve, unknown-accounting and retry paths,
   with no row able to select a normal victim for one-use admission?
7. Is the union walk (§6.4) the correct accounting basis, and is its partial
   coverage handled honestly for normal (continue, reported) and one-use
   (refuse) admissions?
8. Is N+1 = 6 with a durable one-use fence the right numeric fence, and is the
   installer's `proof_controls` refusal correct for older receipts?
9. Is the static-edits-plus-no-mass-rerun rule (§11.2) correctly scoped per
   staging role, including the suspended `test-oxide-cli.py` gap?
10. Is the 300 s / 512 MiB bound with ~204–260 s and ~130 MiB credible for the
    22 fixed cases, and is the seeding case's "unexecuted" honest outcome
    acceptable, or must the freeze split S5 out first?

## 19. Limits of this design

- No implementation, native, install, consumer, host or live-pool authority is
  granted; the merge hold and both counted natives remain.
- The time and disk figures are source-derived arithmetic, not measurements; the
  fit is UNMEASURED until the one activation per side runs.
- The seeding interaction (§8) is a requirement on a frozen but unimplemented
  interface; S5 cannot land before oxide#137 does, and the last native case can
  be unexecuted until then.
- No arbitrary-write containment, no byte-saving claim, no free-space guarantee
  and no claim that a one-use slot's warm build is as fast as a normal one.
- Platform scope follows the existing proofs: macOS fixtures with the pool's
  own volume rules; Linux compile coverage only.
- The `clean` wording residual (§4.4, §12) is knowingly left for the
  implementation brief.
