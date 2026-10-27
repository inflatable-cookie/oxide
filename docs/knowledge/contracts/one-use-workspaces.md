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

**Revision 4** answers the CHANGES REQUIRED design review of `0fb9396`
(comment `6452964156`, 2026-10-27) with three corrections. **The eviction
custody facts no longer reacquire locks**: `Store::checkout_custody` takes
`sibling_admission` and this store's admission on fresh descriptors, both of
which `Store::attempt` already holds, so calling it there would block on the
same process's own `flock`. The design now specifies one **in-custody reader**
(§5.5.2) that consumes the two state snapshots the admission already holds, with
the inherited internal-then-this-store lock order, `Unknown` for an unreadable
sibling, the same `Clear`/`Writers`/`Unknown` verdicts, no lock or file creation,
and a regression on both backends. **The reserve case is now achievable**: a
~128 MiB image is always below the fixed 10 GiB floor, so
`unknown_removable_step` can only `Evict` then `Refuse` there; the case therefore
asserts one-use (`Only`) protection — retire the eligible idle members in rank
order, never the normal slot, then refuse with the shipped incomplete scan —
followed by a byte-pressure retry, and it explicitly does **not** claim the
unreachable `Admit` branch. **The concurrent-transition case is source-grounded**:
`OXIDE_TEST_PAUSE` is `<point>:<gate>` and `bound` fires after binding returns, so
the case uses it for custody *exclusion* (a paused peer blocks the guarded verbs)
and uses the product's own write-ahead stop,
`OXIDE_TEST_OUTPUTS_CRASH=prepared|migration-p0`, to produce a real persisted
`Prepared` receipt or migration journal that every guarded verb must refuse
without changing anything; no new seam is proposed, and an interleaved
observation is explicitly not claimed. The time/disk allocation is recalculated
(~267 s typical / ~335 s upper of the 420 s bound, ~258 MiB of 512).

**Revision 4 also records one product fact the reserve case exposed.** The
removable reserve prune is **unreachable in the current source**: `attempt` sets
`LeaseRequest.reserve_bytes` from `crate::removable::registration(self)`
(`store.rs:3277-3280`), and for a removable-store admission `self.control()` is
`<owner>/removable/oxide-managed` (`removable.rs:30`, `store.rs:1546-1547`)
while the registration is written at `<owner>/removable.json`
(`removable.rs:28,298`). `registration` therefore returns `None`, the request
carries no reserve, and `unknown_removable_step` takes its
`(available, reserve)` miss branch and refuses with the incomplete scan
(`pool.rs:2848-2852`) before any prune. The design therefore adds an explicit S3
item — bind the reserve fact from the **internal** control for a removable
admission — flagged as a behaviour change on a currently unreachable path that
needs its own review, and states that the native reserve case exists only once
that item lands; without it, the reserve case is returned to the owner as a
scoped case/bound change rather than dropped or reported unexecuted (§16.2 case
8, §19).
**Revision 3** answers the CHANGES REQUIRED design review of `1ac22ca`
(comment `6452712728`, 2026-10-27) with five corrections. **Guarded verbs are now
race-safe**: a fast, effect-free precheck is explicitly never authoritative, and
the authoritative revalidation runs under the verb's own custody — the clean
barrier for adopt/detach/cancel-reservation, the checkout binding mutex for a
command's binding — before any fence, cancellation or replay, with the custody
mutual-exclusion property named, a stop condition if a writer lacks it, a
precise "nothing changed" scope, a `Prepared`-before-general diagnostic
precedence at adopt, and a concurrent-transition regression using the shipped
debug pause gate. **Victim eligibility now binds control-record custody**: the
front door derives a separate `evictable` subset from record phase, lock,
holder/lease/recovery facts, `process_group`, `checkout_custody` and donor/clean
facts, the library victimises only that subset and stops rather than
substituting, and a ledger-idle/control-active crash state is a protected,
counted member. **Reserve and retry are now required native cases**: the reserve
route is driven with the product's existing debug walk mutation and pause gate
(above-reserve admit, at-reserve prune of exactly one one-use member, then the
honest below-floor refusal, then a retry — superseded by revision 4's sub-floor
premises, which withdraw the unreachable admit branch and the
one-member-then-two-intact oracle), with dedicated failure-stop and
retry cases; the earlier "not induced" limitation is withdrawn and the 420 s /
512 MiB arithmetic is revised to ~275 s / ~345 s upper and ~290 MiB. **The
staging inventory is now complete for current candidates**, including all six
current-HEAD roles in `test-permanent-outputs.py` (`:589`, `:1583`, `:1837`,
`:2297`, `:4775`, `:6238`) beside its fixed pre-feature role. **The class-change
oracle is corrected**: one-use → warm reuse of the same record/slot → a distinct
normal record → a warm return to the *original* one-use record, exactly two
records, never a third.
**Revision 2** answers the CHANGES REQUIRED design review of `2a4f6a9`
(comment `6452514295`, 2026-10-27) by correcting six defects: the slice now has
**two thresholds** (a new slot restores down to 15 members and a warm reuse is
admissible at the cap), so 16 idle members no longer refuse a 17th workspace;
**class-unknown rows** make the slice a lower bound and refuse a one-use
admission instead of reading as empty capacity; a **committed receipt refuses in
every member shape**, the guarded `Prepared` check runs before the barrier,
fence, reservation cancellation and both replays at all **four** mutation sites
(revision 3 replaces that ordering with the in-custody protocol of §4.1),
`clean` keeps its refusal without adopt advice, and every modelled
`target/<triple>` directory is inspected itself so an intermediate link is never
followed; the fixed proof now uses **one owned 128 MiB image** so the removable
and permanent-mode cases are genuinely reachable, and it states that the
unknown-figure reserve route is not induced (withdrawn in revision 3, which
binds it as case 8); the seeding case is **mandatory**
(no unexecuted case in a counted native) with the alternative expressed only as
a scope split, and the proof declares **HIGH** complexity, the fixed premise
checklist and a separately activated uncounted diagnostic; and the staging
inventory now lists **helper-mediated** `stage_at` callers explicitly, with a
per-call feature parameter and per-call fixture controls. The declared fixed
bound is revised to **420 s** with the arithmetic in §16.4, put to the freeze
for acceptance rather than silently extending anything.
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
| Output mutation verbs | `outputs.rs:2037-2100` (`adopt_command`: barrier `2057`, fence `2075`, migration replay `2081`, receipt replay `2099`), `3983-4030` (`detach_command`: barrier `3993`, fence `4000`, `cancel_reservation` `4009`, replay `4014`), `5988-6010` (`cancel_command`), `8450-8470` (`clean_placement`, `Prepared` refusal at `8467`) | **Fast precheck, then an authoritative revalidation under the verb's own custody before every effect**: adopt refuses after the barrier and before the fence/replays; detach and cancel-reservation refuse a `Prepared`/pending-migration receipt under the barrier before their fence/cancellation/replay; clean keeps its refusal with the guarded text (§4.1, §4.4, §4.5). |
| Proof staging helper | `scripts/test-oxide-pool-layout.py:96` (`stage_at`, imported by eight scripts) | **Explicit per-call `features` parameter**; a helper default never adds a feature, and each caller's role and fixture controls are edited where they live (§11.2). |
| Custody and closure facts | `store.rs:2572` (`checkout_custody`), `store.rs:4500` (`probe_lock`; non-creating use at `3155`), `store.rs:630-639` (`settle`), `store.rs:3374-3420` (`recover_settled`), `store.rs:399-406` (`settle_idle`) | **Read** by the front door to derive the `evictable` subset (§5.5.1); the library never reads them. |
| Per-checkout throughput custody | `outputs.rs:6151-6167` (`barrier` = `begin_clean`, `release_barrier`), `outputs.rs:4243` (checkout binding mutex), `outputs.rs:4008,5873,6000` (other binding-mutex sites) | **Read.** Guarded revalidation runs under one of these before any effect (§4.1, §4.6). |
| Debug proof controls (existing) | `main.rs:327-352` (`OXIDE_TEST_PAUSE` = `<point>:<gate>`, debug only; `bound` fires **after** `bind_for_command`); `outputs.rs:90,7191-7200` (`OXIDE_TEST_OUTPUTS_CRASH` = a `crash_point` name, debug only; `prepared`/`migration-p0` stop after the durable prepared write); `src/inventory.rs:1402-1445,1559-1600` (`OXIDE_TEST_WALK_MUTATIONS`: ≤4 specs, ≤8192 bytes, canonical paths under the owned scratch/evidence roots, `delay_ms` 1..=65,000, one delay per spec) | **Reused, not extended**, for the custody-exclusion, prepared-state and reserve native cases (§4.6, §16.2 cases 8 and 19). |
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

### 4.1 Why the guard sits before every output effect

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

The guard is not only a *read*, and a read alone cannot make its refusal
race-safe. Each output verb performs effects in a fixed order — the clean
barrier, the forward fence (`prune::ensure_ready`), a stale reservation's
cancellation journal, then a transition replay — and **another process can
publish a `Prepared` receipt or a migration journal between an unlocked read and
the barrier**. So the guard has two parts, and only the second one is
authoritative:

1. a **fast precheck** at the earliest safe point, with no effect at all. It may
   refuse early (and then literally nothing, not even a clean mark, was
   created), but it never authorises a command to proceed;
2. an **authoritative revalidation inside the verb's own custody and before any
   effect**, re-reading the receipt and every relevant component (and the
   migration journal) once the custody that excludes concurrent output
   transactions is held. Only this revalidation can let the verb continue, and
   only it produces the "nothing changed" refusal.

The custody differs per verb, and each is the lock the shipped code already uses
to exclude the writers of that state:

| Verb | Custody that excludes concurrent transitions | Authoritative revalidation point | Effects it therefore precedes |
| --- | --- | --- | --- |
| `oxide outputs adopt` (`outputs.rs:2037`) | clean barrier (`outputs.rs:2057`, `begin_clean`) | immediately after the barrier is acquired | fence `2075`, migration replay `2081`, receipt/migration replay `2099`, any reservation change |
| `oxide outputs detach` (`3993`) | clean barrier (`3993`) | after the barrier, before the closure's first effect | fence `4000`, `cancel_reservation` `4009`, replay `4014` |
| `oxide outputs cancel-reservation` (`5988`) | clean barrier (`5998`) | after the barrier | binding lock, cancellation journal, receipt write |
| leased command binding (`4196`) | checkout binding mutex (`4243`) | inside the mutex, before `gated_replay` (`4253`) | `gated_replay`, `replay` (`4301`), any prepared transition |
| automatic first conversion (`3888`, called at `main.rs:414`) | clean barrier (`3936`) | never: the whole verb is **skipped** for a guarded workspace | barrier, fence `3941`, adoption transaction |

Two properties make that sound, and both are verified in S1 rather than assumed:

- every writer of a receipt's `prepared` field and of the migration journal
  takes the same per-checkout custody — the clean barrier for the output verbs
  and for the prune transitions that write a checkout receipt
  (`prune.rs:4596-4621`), and the binding mutex for a command's binding — so no
  transition can appear between the authoritative revalidation and the effect it
  guards. A receipt state written by any other path (`Pruning`, `Pruned`, an
  unsettled prune intent) is refused by §4.2's table, so a writer outside that
  custody can only cause a refusal, never a wrong build;
- barrier and binding custody exclude each other for one checkout (a barrier
  waits for the checkout's commands to finish, and a command that starts after
  a barrier waits), so neither kind of writer can slip past the other.

If implementation finds a writer that takes neither custody, or a verb whose
effect runs after its custody is released, the work **stops** and returns to
design; the ordering claim is not widened to cover it.

**What "nothing changed" means, precisely.** The fast-path refusal creates
nothing. The in-custody refusal creates and then abandons one control-state
clean mark (the shipped `barrier`/`release_barrier`, `outputs.rs:6151-6167`);
it writes **no** receipt, link, sibling, root, marker, reservation, adoption
fence, migration-journal step or slot state, and it replays nothing. The
regression compares a bounded digest of every one of those before and after,
plus the clean mark's absence.

**Diagnostic precedence at `adopt`.** `adopt` on a guarded workspace always
refuses, but the text depends on state, decided in this order:

1. receipt `Prepared`, or a pending migration journal (or both) → the exact
   `Prepared` diagnostic below;
2. any other state (absent, `Committed`, `Detached`, `Pruned`, malformed,
   mismatched, `Pruning`, unsettled intent) → the general guarded `adopt`
   refusal described in §4.5.

So the brief's exact diagnostic is never shadowed by the general refusal, and
the general refusal never offers adopt advice for a `Prepared` transition.

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
| `Committed`, in any shape — every placement owned, some owned and some real, or a real nonempty member | any | **refuses**: a committed receipt selects an owned root, so a guarded workspace never builds locally through it. The existing ownership-checked `oxide outputs detach` is the one safe verb (it keeps roots and restores real empty directories) and this refusal names it |
| `Prepared` (any placement, including a pending layout migration journal) | anything | **refuses before replay and before every other effect** with the exact diagnostic (§4.4) |
| malformed, unsupported version, mismatched checkout identity, `Pruning`, or an unsettled prune intent | — | **refuses** through the existing validation/refusal, unchanged |
| `Pruned` | — | terminal evidence with no selection (the shipped `clean_placement` treats it as unconverted); the live-component rows above decide |

Two properties are load-bearing. A **committed receipt never becomes a guarded
local build**, in any member shape: the per-member local fallback that the
shipped layout allows for a *normal* checkout (§4.5) is not available to a
guarded one, because the brief requires committed receipts to refuse. And **no
receipt or link is ever deleted** to make one-use work; only the existing
ownership-checked detach may turn an owned link back into a real directory.

### 4.3 Relevant components, inspected hierarchically

The guard inspects, with `symlink_metadata` (no follow), this bounded set, and
every path is reached **through its parent chain**, so an intermediate link can
never be followed:

- `<workspace>/target`;
- `<workspace>/target/<triple>` when the command's target triple is modelled —
  this directory itself, not only its children, is inspected. Checking
  `target/<triple>/debug` alone would resolve `target/<triple>` first and follow
  a triple symlink;
- every profile member the command's own layout declaration names, using the
  shipped classifier (`outputs::lease_declaration` → `layout::classify`):
  `debug`, `release`, `<triple>/debug`, `<triple>/release` at most.

A member path is inspected only when every ancestor from `<workspace>/target`
down to its parent is a **real directory**; the first link, dangling link or
non-directory in that chain is the refusing component and is named in the
refusal. Each inspected path must be absent or a real directory; an unreadable
path refuses. The inspection is metadata only: no directory listing, no
recursion, no payload walk, no link followed.

### 4.4 `Prepared`: exact diagnostic at all three replay sites

Today an interrupted transition is replayed at three production sites:

| Site | Today |
| --- | --- |
| explicit adopt (`outputs.rs:2099`) | `replay(&context, receipt)?` |
| explicit detach (`outputs.rs:4014`) | `replay(&context, receipt)?` |
| command binding (`outputs.rs:4253` → `gated_replay` at `4301`) | `replay(context, receipt)` when custody is `Clear` |

For a **guarded** workspace the authoritative revalidation (§4.1) runs under
the verb's custody **before** each of them and before every effect that precedes
them, and refuses with exactly this one line, before any read of a sibling,
marker or root and before any rename, fence or reservation write:

```text
prepared output transition unresolved; this guarded workspace cannot replay it; nothing changed
```

The receipt bytes, the live link or absence, the sibling, the root marker, the
root tree and the generation are all left exactly as found (the native case
compares a bounded before/after digest). **No adopt or detach repair advice is
attached**, and no new recovery mechanism is introduced. The only recovery is an
operator's explicit, deliberate action outside the guarded workspace, or running
the command from the now-normal workspace the transition belongs to.

**`clean` keeps its refusal and loses the advice.** The shipped
`clean_placement` refuses a `Prepared` receipt
(`outputs.rs:8467`) with a message that tells the operator to run
`oxide outputs adopt`. That advice cannot work in a guarded workspace, and the
brief forbids adopt/detach repair advice for a `Prepared` transition, so on a
guarded workspace the refusal is **retained** (same state, same refusal, nothing
changed) but its text is the exact guarded diagnostic above, without advice. A
normal checkout keeps the shipped wording byte for byte.

### 4.5 Adopt, detach, clean, plan and status

- **`oxide outputs adopt`** on a guarded workspace **refuses**, with the
diagnostic chosen by §4.1's precedence (a `Prepared` receipt or pending
migration journal gets the exact diagnosis; every other state gets the general
refusal naming that this workspace is one-use/temporary and cannot own an output
root). The fast precheck refuses before any effect at all; if state changed
before custody, the authoritative revalidation refuses after the barrier and
still writes no receipt, root, link, marker, reservation or fence.
- **`oxide outputs detach`** remains available and safe exactly as shipped: for
a `Committed` placement whose live path is the exact owned link it becomes a
real directory again (profile layout: every owned member link becomes a real
directory, `target` stays real) with **no root moved, copied or deleted**, and
for a valid `Detached` receipt it is the existing no-op. On a guarded workspace a
`Prepared` receipt (or a pending migration journal) is re-read **under the
barrier, before the fence/cancellation/replay** and refuses per §4.4; a normal
checkout keeps the shipped order.
- **`oxide clean`** keeps its refusal for `Prepared`; on a guarded workspace its
text is the exact guarded diagnostic (§4.4). Everything else about clean is
unchanged, and clean is not a one-use-specific verb.
- **`oxide outputs cancel-reservation`** is the one additional receipt-writing
verb: a guarded workspace refuses it when the receipt is `Prepared` or a
migration journal is pending (same diagnostic, no advice), because cancelling
would rewrite a receipt that carries an unresolved transition. Otherwise the
shipped cancellation runs unchanged; it publishes no root and selects nothing.
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

### 4.6 Races, and the concurrent-transition regression

The guard is evaluated in three places, and only the last two are authoritative:

1. during planning (`planned_destination`, `main.rs:456`), where the expected
   destination is `None` for a guarded command. This is the fast precheck: it
   can refuse, but it can never authorise;
2. inside `bind_for_command` **under the checkout's binding mutex**
   (`store.lock_binding`, `outputs.rs:4243`) before `gated_replay`, where the
   receipt, the migration journal and every relevant component are re-read;
3. for the output verbs, under the **clean barrier** before their first effect
   (§4.1).

A transition published between (1) and (2)/(3) — by a concurrent adopt, detach,
cancel-reservation or command binding — is therefore always seen by the
authoritative revalidation, which refuses with the guard's own diagnostic or,
for a changed destination, the shipped "this checkout was … while this command
waited … nothing was built" shape. A guarded command never proceeds on the
strength of the precheck, and a slot keyed for one destination never binds
another. `bind_for_command`'s `expected = None` path already exists
(`keyed_state_matches(None, expected)`), so a guarded command fails closed if a
receipt appeared while it waited.

**Regression (native case 19).** Two *existing* debug controls already reach the
states this protocol must survive, so the design proposes **no new seam**:

- `OXIDE_TEST_PAUSE` is parsed as `<point>:<gate>` (`main.rs:333-352`: the value
  is `strip_prefix(&format!("{point}:"))`), and `bound` fires **after**
  `bind_for_command` returns (`main.rs:545`). It therefore holds a *command*,
  not a prepared transition, and case 19 uses it for exactly that: a peer
  command paused at `bound:<gate>` keeps the checkout occupied while the guarded
  verbs run. Each guarded `adopt`/`detach`/`cancel-reservation` must **wait** for
  the checkout (its barrier) and a guarded command binding must wait for the
  binding mutex; none may proceed or write while the peer is paused. Releasing
  the gate lets them run against the final state. This asserts the custody
  **exclusion** that makes the precheck-to-effect window safe; it does not claim
  to observe an interleaving, because both custody paths make one impossible.
- `OXIDE_TEST_OUTPUTS_CRASH` (the `crash_point` control,
  `outputs.rs:90,7191-7200`, debug-only) stops a transition **after its durable
  prepared receipt write and before any link effect** (`crash_point("prepared")` at `outputs.rs:6984`,
  `crash_point("migration-p0")` at `outputs.rs:2651`). The peer therefore leaves
  a real `Prepared` receipt or migration journal written by the product's own
  write-ahead path — stronger evidence than a hand-planted file. Every guarded
  verb (`adopt`, `detach`, `cancel-reservation`, a normal command, and `clean`)
  must then refuse with the exact diagnostic, with the bounded digest of the
  receipt, links, siblings, markers, roots and reservations identical before and
  after; a following explicit `oxide outputs adopt` replays and commits it, after
  which the guarded verbs see `Committed` and refuse per §4.2.

Both controls are debug-only and inert in a release build; they are existing
product test facilities, not new product behaviour. If the freeze wants a
literally interleaved observation, that needs a **new** debug seam inside the
barrier and must be proposed and reviewed separately — this design does not claim
one.

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
derives, from the control state and the ledger view it already reads, **three**
bounded sets:

```text
members   = for each record with class == OneUse:
                for each ledger slot whose project == affinity_dir(record.id):
                    (slot_id, incarnation)
evictable = the subset of members whose owning record positively establishes
                control-record closure at this admission (below)
unknown   = for each ledger slot that no record owns (no record's affinity
                directory equals its project, or its owning record is
                unreadable): (slot_id, incarnation)
```

bounded by `MAX_SLOTS` (128) pairs each and encoded as fixed-size vectors. Each
pair is verified by the library against the ledger under the pool lock (§6.2).
A slot with a stale incarnation or a missing ledger row is **unverified** and is
reported as such; it is never a victim and never free quota.

`members`, `evictable` and `unknown` are disjoint (`evictable ⊆ members`), and
together with the verified **normal** slots the members and unknown rows account
for every ledger row exactly once. That totality is what makes the slice figure
honest: the slice is either the verified member union with **no** class-unknown
row (`known`), or the same union labelled a **lower bound** beside the number of
unattributed rows (`lower bound, k unknown`). It is never presented as a complete
figure, and never as zero headroom created by an only-unknown pool.

**A ledger row is not a writer-closure proof.** The ledger's `Idle` state and
the control record's phase are written by **separate** atomic updates
(`store.rs:1826-1850`: the library release marks the slot idle, then
`Store::save` marks the record idle), so a crash between them leaves a ledger-idle
slot whose record is still active. `recover_settled` (`store.rs:3374-3420`)
deliberately **keeps** an active record when `settle` (`store.rs:630-639`) cannot
prove closure — a non-empty, unrecorded or non-owned group is `Settle::Keep`, not
a recovery. And the library itself reads no Oxide record, phase or lock, so it
**cannot** establish writer closure from the ledger alone. The `evictable`
subset is therefore derived here, and a member that fails any item of the
checklist is a **protected member**: still counted in the slice, never a victim.

### 5.5.1 The evictable checklist (control-record custody)

A member is evictable only when **every** item below holds, each read by the
front door at this admission **from state it already holds**; any doubt means
protected. The reader is specified in §5.5.2 — in particular it must **not**
call `Store::checkout_custody`, because that method takes locks this admission
already holds.

| # | Fact | Where | Doubt means |
| --- | --- | --- | --- |
| 1 | record phase exactly `Idle` (`store.rs:139-153`) | control state | protected |
| 2 | no `holder`, no `lease_id`, no `pending_recovery`, no `retired`, no `lost` | control state | protected |
| 3 | the owning record lock is free and not missing: `probe_lock(lock_path, false) == Probe::Free` (`probe_lock` at `store.rs:4500`; the same non-creating probe the project-cap path uses at `store.rs:3155`). `Missing` is **not** free | control state | protected |
| 4 | **in-custody custody facts** (§5.5.2) for `record.checkout` are `Clear` — no other command of **that** checkout may be writing, no lost-slot custody with unclosed writers. Computed from the two state snapshots this admission already holds, never by calling `Store::checkout_custody` | control state | protected |
| 5 | `process_group` is `None`: a recorded group means a writer may still exist. `settle_idle` (`store.rs:399-406`) clears it, and `recover_settled`'s `Reset` (`store.rs:3397-3402`) only drops a pending recovery — it leaves the active record and its group in place, so phase-`Idle` plus no recorded group is the positive closure fact | control state | protected |
| 6 | the ledger slot is `Idle`, `lease_id` is `None`, and its `(slot_id, incarnation)` equals the member pair | ledger view | protected |
| 7 | no donor pin covers the slot (§8.2) and no `oxide clean` for that checkout is recorded | control state | protected |

### 5.5.2 The in-custody reader (no lock reacquisition)

`Store::checkout_custody` (`store.rs:2572-2637`) is **not** usable here. It takes
`sibling_admission()` and then `self.admission()` on fresh descriptors
(`store.rs:2639-2654`), and `Store::attempt` already holds both — the internal
sibling admission lock through `sibling_barrier` (`store.rs:2656-2680`) and this
store's admission lock (`store.rs:2978`). On macOS `flock` is per open file
description, so a second acquisition from the same process does not succeed
recursively: it **blocks**, which would make every ordinary admission with a
one-use member hang instead of admitting or refusing. The removable path holds
both locks and would repeat the same mistake twice.

The design therefore specifies one new **in-custody reader** with no lock of its
own:

```text
fn checkout_custody_in_custody(
    this_state: &State,              // already loaded by Store::attempt
    sibling_state: Option<&State>,   // bounded read of the sibling state file,
                                     // under the sibling admission lock attempt
                                     // already holds
    checkout: &Path,
    own: u64,
) -> CheckoutCustody
```

Rules, all inherited from `checkout_custody` so the facts do not diverge:

- it takes **no** lock, opens **no** admission file and creates nothing;
- lock order is the order already held: internal sibling admission → this
  store's admission. The snapshot is taken once, at the top of the admission,
  after `recover_settled`, and is not re-read mid-decision;
- a sibling whose state file is **unreadable** is `Unknown` (never `Clear`), and
  `Unknown` protects every member;
- the caller's own record id is exempt only in this store, exactly as
  `checkout_custody`'s `own` argument does;
- `Spawning`/`Running` records of that checkout are `Writers`; a `Retired`
  record's `lost.unclosed` that is not closed by this boot session is `Unknown`;
- the reader is the *only* place the checklist's item 4 comes from, and a
  member whose record lives in the sibling pool uses the same function with that
  pool's snapshot.

**Required regression.** Cases 5 and 7 (internal) and case 8 (removable, once its
S3 reserve wiring premise lands) each run an admission with at least one one-use
member and assert that it reaches its ordinary decision — admit, evict or
refuse — **within the guard's deadline** and never blocks on a re-acquired
admission lock. A pure regression asserts that
the in-custody reader takes no lock (no lock file is created, no `flock` is
attempted) and returns the same verdict as `checkout_custody` for each of the
`Clear`/`Writers`/`Unknown` fixtures. `Store::checkout_custody` keeps its
existing signature and its existing callers; nothing about unknown-writer
protection changes — the reader is a refactor of the same facts, not a weaker
predicate.

The `evictable` set is passed to the library as its **only** allowed victim
source; membership in `members` alone is never enough. A member that is counted
but protected is reported in the bounded observation (`protected: n`) and never
silently turned into free quota.

**Revalidation through the deletion.** The library re-verifies, under the pool
lock and immediately before the deletion, that the pair is still a ledger row
whose `incarnation` matches, still `Idle` and unleased, and still passes
`validate_idle_slot_scope`. A pair that no longer verifies **stops** the
deletion (`UnknownAccounting`/`Busy` as shipped) — the library never falls
through to the next candidate, because the next candidate's custody was not
established for this moment. The front door holds the admission lock across the
whole call, so no other Oxide command can transition a slot while this happens;
an external writer is excluded by the shipped positive-closure rules the
checklist above re-uses.

**The ledger-idle/control-uncertain counterexample.** A slot's ledger row is
`Idle` while its owning record still says a command may be live: the record lock
is held by a running command, `recover_settled` therefore `continue`s without
settling it, and `settle` (`store.rs:630-639`) would `Keep` a ledger-`Held`
record whose group is not positively empty. The library's predicate
(`Idle && no lease_id`) accepts that row; the checklist does not: phase is not
`Idle` (or the lock is not `Free`), so the member is **protected** — counted,
never a victim, and a one-use admission that needs its bytes refuses (it is not
a progress-capable holder either, because its custody is uncertain) rather than
deleting a slot that may still have a writer.

Two narrower facts must be stated exactly, because they bound what the
checklist proves:

- a crash between the library's `Held → Idle` write and `Store::save`
  (`store.rs:1826-1850`) is *not* left active by the shipped recovery: with the
  ledger already idle, `settle` returns `Reset`, and `recover_settled` then
  calls `settle_idle` (`store.rs:3448`) — so the record becomes `Idle` only
  through that positive settle, and only then does it pass the checklist;
- a record kept by `Settle::Keep` always has a ledger-`Held` slot, which the
  existing ledger-only predicate already refuses.

So the checklist does not duplicate the ledger; it adds the control-record facts
the library cannot see. Native case 25 drives the live-record-lock state.

### 5.6 Class-unknown rows and the slice

A ledger slot that no record owns (a foreign or orphan slot, or one left after
its record was forgotten) has **unknown class**:

- it is **never** an `Only` or preferred victim, so it can never be deleted on
a one-use admission;
- it is **not attributed to the slice**: its bytes are neither counted as
member bytes nor claimed as free capacity;
- therefore the slice observation for a pool holding any unknown row is
`lower bound`, and an **empty verified member set with unknown rows is not
`NotNeeded`** — it is `LowerBound { bytes: 0, unknown: k }` (§6.3);
- a **one-use admission** in that state refuses with the shipped
`UnknownAccounting` safety refusal (naming the `k` unattributed rows), because
the slice bound cannot be proven. This is the brief's rule that unknown
membership/accounting stays explicit and unknown class is not free quota; the
access path is unchanged — a normal admission's `Any`/`Prefer` fallback may
retire a positively idle whole orphan under the existing #118 eligibility and
ownership/accounting checks, after which the one-use slice is `known` again;
- under a **normal** admission's `Any`/`Prefer` fallback it keeps today's #118
whole-idle eligibility exactly as before; this design does not newly pin it, and
a normal new-slot admission continues with the lower-bound observation rather
than stalling.

A pool whose every ledger row has a verified owning record (normal or one-use)
has an empty `unknown` set, so its slice figure is `known` even when it has no
one-use members at all.

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
// Two sets, never interchangeable: `accounting` counts bytes and slots,
// `evictable` is the only victim source and is derived by the front door's
// control-record custody checklist (§5.5.1). `evictable ⊆ accounting`.
pub struct OneUseSlice {
    pub max_slots: usize,       // 16 verified members
    pub max_bytes: u64,         // min(admission_budget / 10, 32 GiB), at least 1
    pub kind: SliceAdmission,   // NewSlot | WarmReuse
    pub unknown: usize,         // class-unknown ledger rows (§5.6)
}
pub enum SliceAdmission { NewSlot, WarmReuse }
pub enum RestoreOutcome {
    NotNeeded,                                   // no members and no unknown rows
    Within { bytes: u64 },                       // members within the byte bound
    Over { bytes: u64 },                         // members over the byte bound
    Full { bytes: u64, held: usize },            // a threshold is reached and no
                                                 // eligible victim remains
    LowerBound { bytes: u64, unknown: usize },   // class-unknown rows present
    Unmeasured { reason: String },
}
```

`Any` is the constructor default so every other library caller keeps today's
behaviour byte for byte. `Prefer` and `Only` take a borrowed `MemberSet`;
`Membership` is validated before use, so a policy can never name a slot the
ledger does not confirm.

**Two thresholds, not one.** `max_slots` bounds the number of **existing**
verified members; a **new** one-use slot needs one free place, so its admission
requires `member_count <= max_slots - 1` after restoration, while a **warm
reuse** adds no slot and is admissible at `member_count == max_slots`. A slice is
"full" for a warm reuse only when its bytes cannot be brought within the bound;
it is full for a new slot when either threshold cannot be met. Restoring to
`max_slots` and then refusing a new slot — the defect this revision corrects —
is not a state this design has.

Layer split, deliberately: the library reports **ledger facts** (measured union,
bound, how many non-reclaimable members are `Held`, how many rows are
class-unknown); the front door decides whether a held member is positively
progress-capable (§6.9), because phases, record locks and custody are Oxide
control-state facts the library never reads.

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
3. a verified pair is an **accounting member** immediately. It is an
   **eviction candidate** only when it is *also* in the front door's `evictable`
   set (§5.5.1 — record phase `Idle`, no holder/lease/recovery/retired/lost, a
   free and existing record lock, `Clear` custody, no recorded process group,
   no donor pin, no clean mark) *and* the ledger row is `Idle` with no
   `lease_id` *and* the existing `validate_idle_slot_scope`
   ownership/device/accounting checks pass. A member in `accounting` but not in
   `evictable` is **protected**;
4. `Held`, `Retired`, `Evicting`, interrupted, control-uncertain, unknown-custody
   and pinned (donor-locked) members continue to **count** toward the slice,
   count and known overage; they are never made free quota because they cannot
   be deleted. The library cannot verify control-record custody itself and does
   not try: it trusts only the `evictable` subset, and it re-checks that subset
   against the ledger under the pool lock.

The mapping is stable because the front door holds its control-state admission
lock (`store.attempt`, `store.rs:2978`) across the whole library call, so no
other Oxide command can create, release or evict a slot of this pool while the
membership is being used. The library itself reads no Oxide record and cannot
verify a record lock; that is the caller's custody, stated here so no reader
assumes the library proves it.

A safety failure on a selected victim (a foreign entry, a device mismatch, an
incomplete walk, a pair that left the `evictable` set, a ledger transition to
`Held`) **stops** the decision with the shipped error. It never skips to a
different, less-checked victim, and it never falls through to a candidate whose
custody was not established for this moment.

### 6.3 Restore runs before admission

`lease` (`pool.rs:925`) gains, in this order:

1. **Restore**, when `slice.is_some()`:
   - if the member set is empty and `unknown == 0`: no walk, no write, outcome
     `NotNeeded` (a pool with no one-use slot pays nothing);
   - if the member set is empty and `unknown > 0`: no member walk is possible,
     the outcome is `LowerBound { bytes: 0, unknown }`, and a one-use admission
     refuses per §5.6 / §6.9 while a normal admission continues;
   - otherwise measure the bounded member union (§6.4), then reclaim eligible
     idle one-use victims from the **oldest** `(rank, slot_id)` first while any
     of these holds, re-measuring after each deletion:
     - `member_count > max_slots` (invariant repair, on any admission);
     - the admission is a **new one-use slot** and `member_count >= max_slots`
       (one place must be free for it);
     - `union_bytes > max_bytes`;
   - the **reuse candidate** — the idle member slot this warm reuse would take
     (same project and profile) — is never a victim; its bytes still count, and
     if it is the only eligible victim left the byte bound cannot be met by
     eviction and the outcome is `Full`;
   - every deletion goes through the shipped `retire_idle_slot`
     (`pool.rs:1808`), so rank headroom, journalled capture, ownership and
     `Evicting` reconciliation are exactly today's;
   - a pool with `unknown > 0` is reported `LowerBound` even when every member
     is within the bound, and never `Within`;
2. **warm reuse**: the shipped idle-search branch. For `Normal` nothing
   changes — no slice measurement, no member walk (the #110 target-only path);
   for `OneUse` the reuse search requires the candidate slot to be a verified
   member and the restore outcome to allow reuse (§6.9);
3. **new slot**: `allocate` (`pool.rs:1072`). The shipped order is kept: an
   optional named project-cap replacement, then the count ceiling
   `MAX_SLOTS - 1`, then the pressure decision. Each of those now takes the
   admission's policy (§6.7).

So restore runs before **every** new-slot admission (normal or one-use) and
before **every** one-use admission including warm reuse, and never on a normal
warm reuse. Concretely, with 16 idle one-use members within the byte bound: a
17th workspace's new-slot admission reclaims the oldest idle member (because
`16 >= 16`) and then admits; a warm reuse of an existing member admits without
reclaiming anything; and only a slice whose remaining members are all protected
is full.

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
| One-use members per pool | **16** | hard count ceiling, independent of `MAX_SLOTS` (128) and the normal eight-slot project cap |
| New one-use slot needs | `member_count <= 15` after restore | the new slot must have a free place; reaching 16 by eviction and then refusing is not allowed |
| Warm reuse needs | a **verified member** candidate, `member_count <= 16` | it adds no slot; 16 members with a protected candidate is admissible if the bytes fit |
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
| normal, new slot | yes (`Prefer`) | shipped order, class-aware victims | may reclaim idle one-use slots first; continues with a reported overage/lower bound when it must |
| one-use, warm reuse | yes (`Only`, candidate excluded) | the verified member slot only | admissible at 16 members while bytes fit and `unknown == 0`; a non-member idle slot is refused, never relabelled |
| one-use, new slot | yes (`Only`) | new slot, `member_count <= 15`, union within the slice, `unknown == 0` | refuses or waits per §6.9 |

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
| class-unknown ledger rows (§5.6) | continues with the lower-bound observation; no stall | **refuses** `unknown_accounting` |
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
an exact overage), and does not by itself fail a normal admission. A
`LowerBound` observation (class-unknown rows present, §5.6) is likewise reported
as a lower bound with the unattributed count and never as free bytes; it does not
by itself fail a normal admission, and its rows keep their shipped #118
`Any`/`Prefer` eligibility.

### 6.9 One-use full: wait for progress, never wait indefinitely

After restore, a one-use admission's **full** condition depends on its kind:

| Admission | Full when | Not full when |
| --- | --- | --- |
| new slot | `member_count` is still 16 with no eligible idle victim left to reclaim, or the bytes exceed the bound with no eligible victim left, or the measurement is unknown, or `unknown > 0` | an eligible idle member was reclaimed and the counts fit |
| warm reuse | the byte bound is exceeded and the only remaining eligible victim is the reuse candidate itself, or the measurement is unknown, or `unknown > 0` | the candidate is a verified member and the bytes fit, **including at 16 members** |

The library reports `RestoreOutcome::Full { bytes, held }` (or the shipped
`AccountingIncomplete` for an incomplete walk, or `LowerBound` when
class-unknown rows exist) and the front door decides:

- `LowerBound` with `unknown > 0`: a one-use admission **refuses** with the
  shipped `UnknownAccounting` safety refusal naming the `k` unattributed rows
  (§5.6). There is no wait: no member's deletion can change an unknown row, and
  an unknown byte total must not be spent as if it were bounded. A normal
  admission is unaffected (§6.8);
- `Full` with at least one **progress-capable** member holder — its record lock
  is held by a live process, its phase is `Acquiring`/`Spawning`/`Running`, and
  the custody facts do not report unknown writer closure (so it is a held, not a
  control-uncertain, member) — takes the existing bounded wait (`Busy` →
  `Wait::Uncertain` for that specific member). That is a wait for a holder that
  can finish, not for a deletion a safety gate forbids. A **control-uncertain**
  member (§5.5.1) is never progress-capable: it is protected and counted, and it
  is not a reason to wait;
- `Full` otherwise (no progress-capable holder), or `Unmeasured`: the admission
  **refuses** with a dedicated bounded error naming
  the measured union, the bound and the protected member count. There is no
  indefinite wait and no fall-through to a normal victim.

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
a control-uncertain member (§5.5.1), an `Evicting` member, a quarantined lost
slot and an untrusted member all keep counting toward the slice while they
exist, and none is ever a candidate. The slice therefore never turns "cannot
delete safely" into "free bytes".

The control-record side matters as much as the ledger side. `settle`
(`store.rs:630-639`) keeps a record whose group is non-empty, unrecorded or not
owned (and such a record's slot is ledger-`Held`, so it is already no
candidate); `recover_settled` (`store.rs:3374-3448`) skips a record whose lock is
held and settles the rest. Because the ledger's `Held → Idle` write and the
control record's idle write are separate atomic updates
(`store.rs:1826-1850`), the two sides can disagree between crashes, and the
library can read only one of them. This design's `evictable` checklist (§5.5.1)
reads the control record, not just the ledger, and protects every disagreement.
The native case that drives the live-lock disagreement is case 25.

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
- names the **implementation dependency and the proof consequence**: oxide#137
  must land its interface **before** oxide#138's counted native, because the
  fixed case list includes the one-use seeding interaction and the brief forbids
  a cold-only rollout. An unexecuted required case can never be a counted PASS,
  so this design offers **no** "unexecuted is acceptable" route (§16.3);
- offers the one legitimate alternative: a **scope split for a new brief** that
  lands #138's guard/slot/admission slices with their own counted native and its
  own required-case list, and moves the seeding interaction into a successor
  brief. That is an owner/Queue decision, recorded as a split, never a silent
  no-op seam or a weakened case list.

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
- **One wiring defect is corrected, with no format change:** a removable
  admission must carry the registration's reserve, which today it does not
  (`store.rs:3277-3280` reads `registration(self.control())`, but the file lives
  in the internal control — `removable.rs:28,298`, `POOL_DIR`/`MANAGED_NAME` at
  `removable.rs:30`/`lease/mod.rs:84`). S3 binds the reserve from the internal
  control for a removable admission, so `unknown_removable_step`'s reserve branch
  becomes reachable. This is a behaviour change on a currently unreachable path,
  reviewed on its own; the format, the reserve value and the floor are unchanged
  and no cleanup is invented.
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
(§16.2, case 15). Pre-#122 binaries cannot be assumed pre-effect fenced; the
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

Every exposed proof that stages a **current** candidate has fixtures under
temporary or scratch roots, which would otherwise be classified one-use. The
rule is a **static** edit to the staging call and to the fixture child
environment, plus source/covering validation — **not** a mass native rerun:

- a current-candidate staging call adds `--features proof-controls`, passed
  **explicitly per call site**;
- a staged artifact that must prove `proof_controls=false` (an installer
  capability control) is built **feature-disabled** on purpose, and any
  capability assertion in that script is updated to the value that artifact
  really reports;
- each fixture child that must keep today's normal-owned-link behaviour receives
  `OXIDE_TEST_WORKSPACE_CLASS=normal` and `OXIDE_TEST_TEMP_ROOTS=[]`;
- the **new** combined matrix (§16.2) is the only place that exercises
  membership, the opt-in, a forced class and the guard together.

The inventory is split by **how** a script builds, because a shared helper must
not decide a caller's role. Verified in `scripts/` at `489eecc`.

**A. Helper-mediated stagers.** Two shared staging helpers build `oxide` with
their own fixed argv, and neither may change a caller's role by default.

- `scripts/test-oxide-pool-layout.py:96` (`stage_at`, argv `cargo build
  --locked --offline --bin oxide`): the module is imported by eight scripts and
  seven of them call `stage_at`.
- `scripts/test-installed-handoff.py:191` (`stage_subject`, the
  installed-handoff recipe with `--bin oxide`) is imported by
  `scripts/test-outputs-prune.py:1728` (plus its own proof at `:850` and the
  synthetic `test_dogfood_scripts.py` reference).

Each gains an explicit `features: str | None = None` parameter whose default
adds **no** feature, so no historical or unrelated role changes by accident.

| Caller | Role | Feature |
| --- | --- | --- |
| `test-oxide-pool-layout.py:210-211` | current HEAD **candidate**; fixed `BASE = 850260c` | candidate only |
| `test-permanent-outputs.py` — **seven** calls: `:589` (counted), `:1583` (`detach-permanent`), `:1837` (`auto-adopt`), `:2297` (`ignore-safety`), `:4775` (`exec-inference`), `:6238` (`auto-ineligible`), and `:778` (fixed `PRE_FEATURE = 8c9b5ae`) | six current-HEAD candidates, each its own role, **plus** the fixed historical pre-feature role; every focused mode has scratch fixtures | feature + class/roots on all six current candidates (each call, not a helper default); the pre-feature role keeps its source and flags; all seven are marked next-needed-run |
| `test-profile-outputs.py:2695-2697` | current HEAD **candidate**; fixed `BASE = 4a70b83` | candidate only |
| `test-oxide-removable-removal.py:358,621` | current HEAD candidate (and boot-closure mode) | candidate only |
| `test-drive-mode.py:172` | current HEAD candidate; fixture workspace, pool, `HOME`, `CARGO_HOME` and `TMPDIR` under owned scratch (`scripts/test-drive-mode.py:174-230`) | candidate only, plus class/roots for its fixtures |
| `test-outputs-prune.py:1728` (through `test-installed-handoff.py:191`) | current HEAD candidate with an explicit capability dict | candidate only, plus class/roots; capability assertions updated |
| `test-freshness-submodules.py:577` | current HEAD candidate | candidate only |
| `test-init-guidance.py:291` | current HEAD candidate | candidate only |

**B. Scripts that load the helper for other facts and stage themselves.** They
get the feature on their own current build call and their own fixture controls;
a helper edit supplies neither.

| Script | Role | Edit |
| --- | --- | --- |
| `test-account-home.py:226` (loads the helper at `:654` for source facts) | current HEAD, debug + release | feature on the debug build; the release build stays feature-disabled (it proves the installer's `proof_controls=false` acceptance) |
| `test-everyday-cli.py:157` (loads the helper at `:52`) | current HEAD debug, unstamped + stamped | feature on both; its capability assertions expect `proof_controls=true` |

**C. Direct current-candidate stagers** (each owns its build call):

| Script | Role | Edit |
| --- | --- | --- |
| `test-accounting-guards.py` | current HEAD debug | feature + class/roots |
| `test-accounting-vanished.py` | current HEAD debug | feature + class/roots |
| `test-cargo-profiles.py:356` | current HEAD | feature + class/roots |
| `test-command-budgets.py:1846` | current HEAD debug | feature + class/roots; capability assertions updated |
| `test-finder-metadata-race.py:454` | current HEAD debug | feature + class/roots |
| `test-front-door-accounting.py:125` | current HEAD debug | feature + class/roots |
| `test-installed-handoff.py:192` | current HEAD debug (plain Cargo) | feature + class/roots; capability assertions updated |
| `test-max-active.py:187` | current HEAD | feature + class/roots |
| `test-oxide-exec.py:151` | current HEAD | feature + class/roots |
| `test-oxide-finder-metadata.py:214` | current HEAD | feature + class/roots |
| `test-oxide-removable-pool.py` | current HEAD debug | feature + class/roots |
| `test-oxide-wrapper-bypass.py` | current HEAD | feature + class/roots |
| `test-pool-describe.py:281` | current HEAD | feature + class/roots |
| `test-pool-scale.py:122` | current HEAD candidate + `git merge-base HEAD origin/main` reference | feature on the candidate; the reference only if that source declares it |
| `test-recorded-slot-accounting.py` | current HEAD debug | feature + class/roots; capability assertions updated |
| `test-slot-headroom.py:182` | current HEAD | feature + class/roots |
| `test-status-fast.py` | current HEAD debug | feature + class/roots |
| `test-storage-layout.py:196` | current HEAD | feature + class/roots |
| `test-pool-temp-scratch.py` | current HEAD debug | feature + class/roots; its own `TMPDIR` fixtures must not change class |

**D. Deliberately excluded.**

| Script | Why | Edit |
| --- | --- | --- |
| `test-oxide-cli.py` | **suspended** by oxide#102 (retired image-pool provisioning; refuses before any effect) | **no edit inferred**; its static edit is bound only after its source route is re-established, otherwise the precise held gap is reported |
| `test-storage-inventory-cli.py` | builds `oxide-storage-inventory`, not `oxide` | no feature edit; assert it stays unaffected |
| `test_dogfood_scripts.py` / `scripts/oxide-install.py` | the synthetic installer tests | gain the `proof_controls` acceptance/refusal fixtures; never a real install |

Rules that follow from the inventory:

- a **fixed historical** source (`850260c`, `8c9b5ae`, `4a70b83`) never gets a
  feature its own manifest does not declare, and a helper default must never add
  one;
- a **dynamic reference** (`merge-base HEAD origin/main`) gets the feature only
  if *that* source declares it, decided at run time from the staged manifest,
  not by a blanket flag;
- a **helper change alone decides nothing**: each call site passes its own
  features, and each fixture's class/root control is edited where that fixture
  lives, not inherited from a helper;
- every script that asserts a capability value is listed above with an explicit
  expectation update; the suspended `test-oxide-cli.py` is not reactivated and
  not silently edited, and its gap is reported.

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
5. **A committed receipt in a temp checkout, in any member shape.** The guarded
   build refuses — including the shipped per-member local fallback shape — and
   names detach. Detach keeps the root and restores real directories; the root's
   bytes stay until the operator prunes them (a separate, explicitly-authorized
   verb). This design does not delete them and does not convert the checkout.
6. **A prepared transition in a temp checkout.** Every mutating verb refuses
   with the exact guarded diagnostic before its barrier, fence, reservation
   cancellation or replay, and changes nothing. There is no automatic repair and
   no repair advice; the operator must act deliberately.
7. **A one-use slice full of protected slots.** The command waits only for a
   positively progress-capable holder and otherwise refuses; a long-lived `run`
   server can therefore block one-use admission for its lifetime. That is the
   intended protection, not a defect, and it never touches a normal slot.
   **Exact-cap idle members are not full**: a new slot reclaims the oldest idle
   member and a warm reuse needs no free place (§6.3, §6.9).
8. **A class-unknown row blocks one-use admission until it is gone.** A pool
   holding an orphan slot (no owning record) has a `LowerBound` slice, so a
   one-use admission refuses with `unknown_accounting` rather than spending an
   unproven bound. The route out is deliberately ordinary: a normal admission's
   `Any`/`Prefer` path may retire a positively idle whole orphan under the
   shipped #118 checks (or `oxide status`/`oxide clean` name it), after which the
   slice is `known` again. This is the conservative direction the brief requires;
   the cost is that a stray orphan pauses one-use work, not that a normal slot is
   ever taken.
9. **Cross-member hard links.** Counted once by the union walk; per-member
   records would overcount. APFS extent sharing between `clonefile` copies is
   **not measured**, so no byte saving is claimed and the slice may be
   conservative.
10. **A one-use slot larger than the whole slice.** It is admitted while the
    slice admits it, then exceeds the bound as it writes (the bound is an
    admission bound). The next admission measures the overage and reclaims it if
    it is idle, or reports it if it is protected.
11. **A tool that writes through `target` after the guard.** Not contained
    (§4.7); the hierarchical inspection of §4.3 catches a link *at* one of the
    declared components, not one created later.
12. **The reserve route is driven natively on a sub-floor volume once S3 wires
    the reserve, and its reachable outcomes are stated.** A ~128 MiB image always has less than the
    fixed 10 GiB floor, so `unknown_removable_step` can only return `Evict`
    (while an eligible slot remains) or `Refuse`; `Admit`/`AdmitAtFloor` are
    unreachable by design and are **not** claimed. The case therefore asserts
    the protection that matters: a one-use (`Only`) admission with a genuine
    bounds-only gap retires exactly the eligible idle one-use members in rank
    order, never the normal slot, then refuses with the shipped incomplete scan.
    A normal (`Prefer`) admission on the same volume legitimately falls back to a
    normal victim once the members are gone — that is `Prefer`, not a
    protection failure (§16.2 case 8).
13. **A retry is not a second attempt at a victim.** After the reserve refusal
    (or a failure-stop, or a protected-slice refusal) the retried admission
    performs the whole class-scoped selection again, with the premises it needs
    — a fresh member and an explicit `OXIDE_POOL_CAP` byte pressure for the
    ordinary path — and it never resumes at the next candidate or converts an
    earlier failure into a normal victim (§16.2 cases 8b, 26, 27).
14. **A ledger-idle slot is not proof of writer closure.** The ledger's idle
    write and the control record's idle write are separate, so the two sides can
    disagree; a record whose lock is held (or whose group is not positively
    empty) is skipped or kept by the shipped recovery. The `evictable` checklist
    reads the control record and protects the slot, and a record is evictable
    only after a positive settle has made it `Idle` (§5.5.1, §16.2 case 25).
15. **`clean` on a guarded `Prepared` receipt.** The refusal is retained and its
    text becomes the exact guarded diagnostic, so it names no adopt step and
    offers no repair route (§4.4).
16. **Pending papercuts.** A temp checkout's intentional Cargo escapes and the
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
| guard: relevant component (or an intermediate `<triple>`) is a link or non-directory | **refuses**, nothing changed |
| guard: `Committed` receipt in any member shape | **refuses**, names detach |
| guard: `Prepared` at adopt/detach/cancel-reservation/bind, and `clean` | **refuses before every effect** with the exact diagnostic, nothing changed |
| guard: malformed/mismatched/unsupported receipt, `Pruning`, unsettled prune intent | **refuses** through the shipped validation |
| explicit adopt on a guarded workspace | **refuses** — fast precheck before any effect, or the in-custody revalidation before the fence/replays; a `Prepared` state takes the exact diagnostic first (§4.1) |
| automatic adoption on a guarded workspace | **skipped**, no receipt write, no notice |
| normal warm reuse | unchanged, no slice measurement |
| normal new slot with slice overage or class-unknown rows | restoration + `Prefer`, then continue with a measured overage or a lower-bound note |
| removable reserve route on a **sub-floor** volume, one-use admission, bounds-only gap (after the S3 reserve wiring) | prune eligible idle **one-use** members in rank order, then refuse with the original incomplete scan; a normal slot is never the victim |
| removable reserve route, normal admission, same volume | may legitimately fall back to a normal victim after the members are gone (`Prefer`), then refuse below the floor |
| removable admission in **today's** source (no reserve wired) | the step returns the incomplete scan before any prune; the reserve case is returned to the owner, never reported `unexecuted` in a counted run |
| retried admission after a failure-stop | full class-scoped reselection; never the next candidate, never a normal victim |
| member whose control record is active (ledger idle, custody uncertain) | **protected**: counted, never a victim, not a wait reason |
| one-use new slot with 16 idle members | reclaim the oldest idle member, then admit |
| one-use warm reuse with 16 members | admits when the candidate is a member and the bytes fit |
| one-use admission, protected slice, progress-capable holder | wait (`Busy` → existing bounded wait) |
| one-use admission, protected slice otherwise / unknown measurement | **refuses** (`one_use_budget` or `AccountingIncomplete`) |
| one-use admission with any class-unknown row | **refuses** `unknown_accounting`; normal admission unaffected |
| any class-scoped victim fails an ownership/accounting check | **stops**; never substitutes |
| unverified membership pair (stale incarnation, missing row) | dropped from candidates, reported; never free quota |
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
  diagnostic before every effect at all four mutation sites, the guarded
  `clean` refusal text, plan/status wording, the forward fence and version 6.
- **S2 record class and front-door scoping.** `Record.class`/`class_reason`, the
  fence, project-cap and sharing class filters, membership and class-unknown
  derivation, status/summary visibility.
- **S3 library admission protocol.** `LeaseRequest` additions, the `MemberSet`
  and `evictable` split with the in-custody custody reader (§5.5.2), `restore`
  with its two thresholds and reuse-candidate exclusion, the union walk, the
  class-scoped pressure paths, the `one_use_budget` and `unknown_accounting`
  refusals, and the **reserve wiring correction** for a removable admission
  (§10.2) — a reviewed behaviour change on a currently unreachable branch, with
  its own unit regression (`unknown_removable_step` reached with a real reserve)
  and the native reserve case it enables.
- **S4 proof controls.** The feature, the two readers, the capability field and
  the installer refusal, the `stage_at` feature parameter and the static edits
  to the exposed scripts.
- **S5 seeding interaction.** Binds S3's headroom rule to #137's actual
  interface. **Blocked on oxide#137's implementation**, and the counted native
  cannot run before it: an unexecuted required case is not a pass. The one
  legitimate alternative is a **scope split** in a new brief that lands S1–S4
  with their own counted native and required-case list and carries S5 into a
  successor. A no-op seam or a cold-only substitute is not an option.

Each slice is implemented, tested and reviewed under its own approved brief with
its own validation tier and native activation.

## 16. Proof plan

### 16.1 Pure checks (no child, no pool, no Cargo)

- The classification table: canonical alias, equality, component prefix
  (`/private/tmpfoo`), each unsafe-broad root, dedup, relative/empty/absent
  inputs, unreadable registration, both opt-in values, and the
  class-versus-guard matrix.
- The receipt/path guard table of §4.2 in every shape (including a committed
  receipt with a real nonempty member, which now refuses), the hierarchy rule of
  §4.3, and the exact `Prepared` diagnostic string with no adopt advice in the
  guarded `clean` refusal.
- The state shape: a version-5 document reads as normal; a version-6 document
  without the fence refuses; an unknown class variant refuses; a class fence
  with version 5 refuses; bounded reason and membership sizes.
- The request/policy shapes: `Any` is the default, a member set is bounded, a
  policy with an empty set is a no-op, `Only` with no eligible victim fails,
  and the two slice thresholds (new slot needs a free place; warm reuse at the
  cap is admissible).
- The restore decision table: 16 idle members and a new slot reclaims the oldest
  then admits; a warm reuse at 16 admits without reclaiming; a protected-only
  slice is `Full`; a class-unknown row yields `LowerBound`, never `NotNeeded`.
- The reserve route's policy: `admit_unknown_removable` passes the admission's
  `Prefer`/`Only` to the same class-scoped victim selector (source-level check
  of that call, and the pure step function's cases).
- The union walker: shared `(device,inode)` set across roots, duplicate root
  refused, link not followed, partial coverage reported, entry/deadline bound.
- The slice arithmetic: `min(budget/10, 32 GiB)`, floor of 1, 16 members,
  15 for a new slot.
- The record→slot mapping: stale incarnation, missing record, missing slot, a
  foreign project and an unreadable record all classify unknown.
- The capability JSON shape and the installer's `proof_controls` acceptance and
  refusal matrix (pure; the synthetic `dogfood:scripts:test` fixture covers the
  same decisions without an install).

### 16.2 Native cases (one fixed script)

One short fixed script, the whole command of the qualified shared proof
supervisor, with tiny std-only fixtures, private `HOME`/`CARGO_HOME`/pool, one
`cfg-if` 1.0.0 crate seeded offline from the verified host cache for the seeding
case, and one owned 128 MiB scratch APFS image for the removable, reserve and
permanent-mode cases. Fixed case list, 27 cases (no case may be reported
`unexecuted` in a counted run, §16.3):

1. classification: canonical alias under a root; root equality; component
   prefix; `/`, `/Volumes/<x>`, HOME, pool ancestor and pool-inside each dropped
   with one bounded report; dedup; an unreadable registration drops a root.
2. opt-in: `1` classifies, `0`/`true`/text are ignored and reported, and the
   membership classification survives an ignored opt-out.
3. repeat and class change, asserted exactly as §5.2 describes: the same
   checkout with the same key is used as one-use twice (the second is a **warm
   reuse of the same record and slot**), then as normal (a **distinct** normal
   record with its own slot, the one-use slot untouched), then as one-use again
   (the **original one-use record and slot are recovered warm**, not a third
   record). Assertions: exactly two records exist across the four commands, the
   one-use record's id and slot id are identical at step 1 and step 4, no record
   changed class, no slot was borrowed or relabelled, and the normal record's
   slot is byte-identical after the one-use commands.
4. slice admission, exact cap: 16 idle members within the byte bound, then a
   17th one-use workspace's **new** slot reclaims the **oldest** idle member and
   admits (16 again); a **warm reuse** at 16 admits without reclaiming; one
   member grows past the byte bound and the next admission restores by bytes.
5. normal count-ceiling protection: a hand-made 128-slot pool (127 idle
   normal slots and one idle one-use member, each a tiny real root with a
   matching record); a normal admission at the ceiling reclaims the one-use
   member first, and a one-use admission at the ceiling reclaims only a one-use
   member or refuses.
6. normal project-cap protection: at the eight-slot **normal** cap with eligible
   one-use members, the replacement deletes exactly one idle **normal** slot
   while the one-use members survive (the front-door list has no one-use
   candidate).
7. normal byte-pressure protection: one-use members present, a normal new slot
   under byte pressure reclaims one-use first, then a normal victim if still
   over — and with every one-use member protected it continues with a measured
   overage and no stall.
8. one-use **`Only` reserve protection**, on the owned image's registered and
   **selected** removable pool (positive selected-removable control), with one
   idle normal slot and two idle one-use members in that pool. **Premise owned by
   S3:** the admission carries the registration's reserve, which the current
   source does not do for a removable store (§10.2 and the revision-4 note);
   this case exists only once that wiring item lands. If it does not, the case is
   returned to the owner as a scoped case/bound change and is **not** reported
   `unexecuted` inside a counted run. The image is
   ~128 MiB, so `available` is far below the fixed 10 GiB floor and
   `unknown_removable_step` (`pool.rs:2827-2885`) can only reach `Evict` while
   an eligible slot remains, then `Refuse` — `Admit`/`AdmitAtFloor` are
   unreachable on a sub-floor volume **by design**, and this case does not claim
   them:
   - (a) *unknown figure, one-use admission*: the existing debug walk mutation
     (`OXIDE_TEST_WALK_MUTATIONS`, `DelayBeforeReadDir` on the payload root,
     6,000 ms > the 5 s removable figure bound) makes the pool figure a
     bounds-only gap. Because the admission is `Only`, the reserve loop retires
     **exactly the eligible one-use members in rank order (both)** and never the
     normal slot, then refuses with the original incomplete scan once nothing is
     left to prune. Assertions: both member roots and ledger rows are gone, the
     normal slot's ledger row and payload bytes are identical, the refusal is
     the shipped incomplete scan, and no extra cleanup ran.
   - (b) *retry, ordinary byte pressure*: a fresh one-use member is created, the
     delay is disarmed (the figure is complete again) and the pool gets a small
     `OXIDE_POOL_CAP`. The retried one-use admission takes the **ordinary byte
     path**, reclaims that one-use member first and admits; the normal slot
     survives byte-identically. Separate failure-stop and retry cases are 26 and
     27, and the internal-pool count/byte paths are cases 5 and 7, so the reserve
     route's `Only` protection is non-vacuous evidence rather than a shared
     selector claim.
   A **normal** admission on the same sub-floor volume may legitimately fall
   back to a normal victim after the one-use members are gone (that is `Prefer`,
   not a protection failure); this case therefore asserts the one-use `Only`
   path, which is the protection the brief names. Because the volume is below
   the 10 GiB floor, `Admit`/`AdmitAtFloor` are not reachable and are not
   claimed; the design does not enlarge the image, lower the floor or invent
   free-space evidence to reach them.
9. one-use `Only` under pressure with no eligible one-use victim: refuses, and
   every normal slot's ledger bytes and payload digest are identical afterwards.
10. normal warm reuse target-only: with members present, a normal warm reuse
    logs no member walk (debug walk log) and its summary claims no slice bytes.
11. one-use full: a progress-capable held member makes admission wait, then
    succeed when released; an interrupted/unknown member makes it refuse with
    `one_use_budget`; every normal slot's bytes are unchanged.
12. class-unknown rows: a planted orphan slot (a ledger slot no record owns) is
    reported by status as an unattributed row; a one-use admission refuses with
    `unknown_accounting` and nothing changes; then a **normal** admission under a
    small byte budget drives the ordinary `Any` path to retire that positively
    idle whole orphan (non-vacuous eviction), after which the one-use slice is
    `known` and the next one-use admission succeeds.
13. crash and recovery: kill while `Acquiring` and while `Spawning`; the
    recovered one-use record keeps its class, the untrusted generation is evicted
    whole, and a normal record's recovery is unchanged.
14. state class/version: a version-5 fixture state reads as normal; a version-6
    document without the fence refuses; an unknown class refuses; the capability
    probe reports state version 6.
15. pre-effect hand-off, both paths: a version-6 document planted in the internal
    control and in a private removable control, with no matching installed
    receipt: exit 75 before any output, receipt, root or slot effect.

16. guarded outputs, absent/real: a temp workspace with no receipt, and with a
    real local `target`, builds locally; no receipt, root or link is created.
17. guarded outputs, valid `Detached`: runs locally, requires no historical
    volume, and a planted link at a relevant component refuses with nothing
    changed.
18. guarded outputs, `Committed` in two shapes: a profile pair of exact owned
    links, and a mixed receipt with one owned member and one real nonempty
    member. Both **refuse** the build; detach then restores real directories with
    the root bytes unchanged; the following build runs locally.
19. guarded `Prepared` and custody exclusion, with existing debug controls only:
    - a peer `oxide outputs adopt` runs with
      `OXIDE_TEST_OUTPUTS_CRASH=prepared` (and, in a second variant,
      `=migration-p0`), leaving a real `Prepared` receipt / migration journal
      written by the product's own write-ahead path. Every guarded verb —
      `outputs adopt`, `outputs detach`, `outputs cancel-reservation`, a normal
      command and `outputs clean` — refuses with the exact diagnostic, and the
      bounded digest of the receipt, link, sibling, marker, reservation and root
      is identical before and after. A following explicit `oxide outputs adopt`
      replays and commits it, after which the guarded verbs see `Committed` and
      refuse per §4.2.
    - a peer command is paused with `OXIDE_TEST_PAUSE=bound:<gate>` (the value is
      `<point>:<gate>`; `bound` fires after `bind_for_command` returns). While it
      is paused, a guarded `adopt`/`detach`/`cancel-reservation` **waits** for the
      checkout's barrier and a guarded command binding waits for the binding
      mutex — neither proceeds nor writes. Releasing the gate lets them run
      against the final state. This asserts custody **exclusion**; an
      interleaved observation is impossible by construction and is not claimed.
20. guard hierarchy: a `target/<triple>` symlink (owned and dangling) refuses
    and is never followed, a linked member under a real triple directory
    refuses, and the link's referent is byte-identical afterwards.
21. permanent drive mode on the owned image: a guarded temp checkout does not
    convert, writes no receipt/root/link and prints no adopt advice, while a
    normal checkout in the same run **does** convert (positive control on the
    same selected volume).
22. proof controls: a feature build with `normal` + `[]` roots stays normal; a
    feature build with a forced `one_use` and production roots stays
    local-output; `normal` alone with a temp workspace stays local-output; a
    malformed value refuses before effects.
23. feature isolation: the feature-disabled artifact ignores both variables,
    classifies a temp workspace from production roots, and guards its outputs;
    the two staged artifacts report `proof_controls` true and false.
24. seeding interaction (mandatory, §16.3): a one-use recipient receives the
    frozen skip/seed path once oxide#137's interface exists, its seeded bytes
    enter the next member union, and an unfit headroom skips to a cold build in
    the same slot.
25. control-uncertain victim custody (§5.5.1): a fixture plants a one-use
    slot whose ledger row is `Idle` while its control record is still active and
    its **record lock is held** by a live fixture process (a real "a command may
    be writing" state: `recover_settled` skips it, `settle` would `Keep`). It is
    reported as a **protected** member. A one-use admission that needs its bytes
    refuses (no wait: no progress-capable holder), the slot's root and ledger row
    are byte-identical afterwards, and a normal admission continues with the
    protected member counted. Releasing the fixture's lock and letting the next
    command's `recover_settled` settle the record makes the member evictable, and
    the following one-use admission reclaims it.
26. failure-stop and victim substitution: at byte pressure the LRU-selected
    idle one-use victim is made unremovable (an unowned foreign entry planted
    inside it). The admission **stops** with the shipped ownership error, never
    substituting the second one-use member and never a normal slot; both are
    byte-identical afterwards. After the foreign entry is removed, the same
    admission retried reclaims the oldest member and admits.
27. retry protection: a one-use admission that refused with `one_use_budget`
    (protected holder) succeeds when retried after that holder releases, and a
    one-use admission retried while only normal idle slots exist still refuses
    and never touches a normal slot.

Every case in this list is a **required** case: §16.3 fixes the activation
order (uncounted diagnostic first) and forbids an `unexecuted` required case in
a counted run. Every case uses its own private pool and checkout; cleanup stays
inside the supervisor's owned root, and the image is detached ordinarily with
its custody record written before each effect. Native failure stops at the first fault; no
retry is authorized by this text.

### 16.3 Runner, bounds, complexity and activation

- Tier: **ELEVATED**. The trigger is storage deletion/recycling: the slice
  reclaims whole idle one-use slots, and the class-scoped count/byte paths
  delete owned storage. Fixture isolation does not remove that trigger, so the
  author runs one counted native and a distinct reviewer runs one independent
  counted native.
- **Complexity: HIGH.** This is a proof-authoring task under the vision
  validation policy, so before the first counted native the author must:
  - apply the fixed [proof-authoring premise
    checklist](workflow.md#proof-authoring-premise-checklist) to every
    executable case body **and helper**, recording each item's source location
    and disposition: the invoking `RUSTUP_HOME` captured before a private
    `HOME`, `OXIDE_TEST_HOME_FROM_ENV=1` wherever a private home is honoured,
    the `OXIDE_*`/`XDG_*`/`CARGO_*` scrub, an explicitly created unconverted
    checkout for the positive-control case, `hdiutil attach -owners on` on every
    attach/reattach with a read-only mount observation, one frozen
    `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`, the internal monitored scratch root,
    and `mktemp` templates under the owned parent; plus the three **existing**
    debug controls this proof reuses: `OXIDE_TEST_PAUSE` (`<point>:<gate>`,
    debug-only, 120 s self-release, marker path under the owned scratch; `bound`
    fires after binding returns), `OXIDE_TEST_OUTPUTS_CRASH` (a `crash_point`
    name, debug-only, leaving a durable `Prepared` receipt or migration journal),
    and `OXIDE_TEST_WALK_MUTATIONS` (debug-only, requires
    `OXIDE_PROOF_DEADLINE_NS`, a canonical `OXIDE_TASK_SCRATCH_ROOT` and
    `OXIDE_PROOF_EVIDENCE` under one owned root, at most 4 specs / 8192 bytes,
    canonical absolute paths, one `delay_ms` in 1..=65,000) — each armed only
    inside its own fixture and disarmed afterwards;
  - run a **separately activated, uncounted keep-going diagnostic** of the same
    fixed script before any counted run. The script provides a minimal opt-in
    `--diagnostic` mode that records reached/completed/failed/skipped/aborted/
    unexecuted cases and the first fault, continues only into
    dependency-independent cases, aborts on unknown child/writer/custody,
    deadline or receipt outcomes, and writes a bounded `qualified: false`
    receipt; its execution needs its own owner activation, pins and window, and
    it is never a PASS. A source sweep complements it and never replaces it.
- One fixed script, the whole command of the qualified shared supervisor:
  `bounded-test.py --root-parent /private/tmp --limit 420 --scratch-budget-mib
  512 -- python3 scripts/test-one-use-workspaces.py`. The explicit internal root
  is the owner-accepted internal-placement exception (decision `bd357a4c`)
  because a private `directory-budget-v1` pool is a genuine internal pool that
  the product correctly refuses on an external volume; it stays inside one
  monitored 512 MiB owned root.
- **Fixed bound: 420 s, not the 300 s default.** The image attach/detach cycles,
  the 128-slot ceiling fixture and the reserve route's two real 6 s walk delays
  are the reasons; §16.4 gives the arithmetic and the 380 s work cutoff. This is a declared fixed higher bound under the
  brief's rule, put to the freeze for acceptance before any activation — never a
  silent extension, and 300 s is not claimed to fit.
- Cumulative author bound **180 min** from actual activation, including
  authoring, admission/waits, coverings, diagnostic, native, cleanup and report;
  reviewer **60 min**. No historical clock is active or reset. Exactly **one**
  combined native per side, each with its own separately counted owner
  activation and headroom; no extra 6/14 selector starts.
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
| 27 tiny fixture cases, including the 128-slot hand-made ceiling pool | ~130 s | ~4–6 s per case, ~10 s for the ceiling pool, ~11 s for the reserve sub-cases (one 6 s walk delay plus a fresh-member retry with byte pressure) and ~6 s for the failure-stop/retry cases |
| Image: attach (`-owners on`), registration, selected control, ordinary detach | ~18 s | two attach/detach cycles at ~9 s each |
| Fixture creation, member walks, digests, receipt | ~25 s | bounded metadata work on tiny trees |
| Cleanup | ~20 s | inside the owned root, with the image custody record |
| **Total** | **~267 s, upper ~335 s of 420** | arithmetic only; the script's work cutoff is 380 s |

Disk, inside the 512 MiB monitored root: one owned 128 MiB APFS sparse image
(no free-space filler is needed: the sub-floor premise is what drives the
prune-then-refuse path); staged targets ≈ 60 MiB shared (the second build reuses
dependencies); the 128-slot ceiling pool with one tiny file per slot plus ≤ 24
medium tiny private slots ≈ 50 MiB; fixtures, logs and the bounded receipt
≈ 20 MiB. Total ≈ **258 MiB of 512**.

These are estimates; the qualified guard measures actual owned bytes and time
and the receipt reports them, and neither figure is an acceptance gate. If the
diagnostic shows the work cannot finish inside 380 s, the owner must revise the
fixed bound **before** a counted activation — never during one.

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
3. Does the guarded table (§4.2) now match the brief for every committed shape,
   and is refusing a mixed or real-member committed receipt (rather than
   treating that member as local) the intended reading?
4. Is the race-safe protocol of §4.1/§4.6 sound at all four mutation sites: a
   fast precheck that never authorises, an in-custody revalidation before the
   fence, reservation cancellation and every replay, custody mutual exclusion for
   every writer with a stop condition, and the `Prepared`-before-general
   diagnostic precedence at adopt? Is the guarded `clean` refusal text (retained
   refusal, no adopt advice) acceptable?
5. Is the hierarchical component inspection (§4.3) sufficient for a modelled
   triple and its members, and does it refuse without following an intermediate
   link?
6. Are the two slice thresholds (§6.1, §6.3, §6.9) correct: a new slot reclaims
   down to 15, a warm reuse is admissible at 16, and a protected-only slice is
   full?
7. Is the class-unknown rule (§5.5, §5.6, §6.9) the right reading of "not free
   quota": lower-bound reporting for normal admissions, an explicit
   `unknown_accounting` refusal for one-use admission, and no new pinning of an
   orphan?
8. Is "no ledger field, no capacity-policy bump" (§6.11) still genuinely safe
   now that membership and unknown rows are both derived under the admission
   lock, and is the stop condition precise enough?
9. Are `Prefer`/`Only` complete for the count, project-cap, byte, reserve,
   unknown-accounting and retry paths, with no row able to select a normal
   victim for one-use admission, and are the reserve/retry fixtures of §16.2
   cases 8, 26 and 27 feasible, safe and non-vacuous as written?
10. Is N+1 = 6 with a durable one-use fence the right numeric fence, and is the
    installer's `proof_controls` refusal correct for older receipts?
11. Is the staging inventory (§11.2) complete now that helper-mediated stagers
    are explicit and `stage_at` takes a per-call feature, including the
    suspended `test-oxide-cli.py` gap and every capability assertion?
12. Is the fixed **420 s** bound with ~275 s typical / ~345 s upper and
    ~290 MiB credible for the 27 cases including the owned image, the 128-slot
    fixture and the two 6 s reserve delays, or must the freeze revise it (and
    the case list) before activation?
13. Is the seeding dependency handled correctly by making case 24 mandatory and
    offering only a scope split — no unexecuted case in a counted native?
14. Is the race-safe guard protocol (§4.1, §4.6) sound: the fast precheck is
    never authoritative, the in-custody revalidation precedes every effect, the
    custody mutual exclusion holds for every writer, and the adopt diagnostic
    precedence puts the exact `Prepared` text first?
15. Is the `evictable` custody checklist (§5.5.1) complete enough that the
    library can never delete a ledger-idle slot whose control record has not
    positively closed, and is "protected, counted, not a wait reason" the right
    treatment?
16. Is the in-custody custody reader (§5.5.2) the right shape: no lock of its
    own, internal-then-this-store order already held, the same verdicts as
    `checkout_custody`, `Unknown` for an unreadable sibling, and a regression
    that an internal and a removable admission with a member reach a decision
    without re-acquiring either admission lock?
17. Is the reserve disposition right: the prune branch is unreachable today
    because a removable admission carries no reserve (`store.rs:3277-3280` versus
    `removable.rs:28,298`), S3 corrects that wiring as a reviewed behaviour change,
    and the native case (§16.2 case 8) — one-use `Only`, bounds-only gap, sub-floor
    image, members retired in rank order then refusal, then a byte-pressure retry
    — exists only after that correction, otherwise it returns to the owner? Is the
    explicit statement that `Admit` is unreachable on a sub-floor volume
    acceptable?
18. Is custody **exclusion** (the `bound` pause) plus the product's own
    write-ahead stop (`OXIDE_TEST_OUTPUTS_CRASH=prepared|migration-p0`) enough
    for the race-safety assertion, given that an interleaved observation would
    need a new seam?
19. Are the HIGH complexity, premise checklist and separately activated
    uncounted diagnostic correctly placed before the first counted native?

## 19. Limits of this design

- No implementation, native, install, consumer, host or live-pool authority is
  granted; the merge hold and both counted natives remain.
- The time and disk figures are source-derived arithmetic, not measurements; the
  fit is UNMEASURED until the one activation per side runs, and the declared
  420 s bound needs the freeze's acceptance before activation.
- The seeding interaction (§8) is a requirement on a frozen but unimplemented
  interface; case 24 is mandatory, so the counted native waits for oxide#137's
  implementation or moves into a scope split.
- The reserve and guarded-state sub-cases depend on three **existing** debug
  controls (`OXIDE_TEST_WALK_MUTATIONS`, `OXIDE_TEST_PAUSE`,
  `OXIDE_TEST_OUTPUTS_CRASH`) staying available and unchanged; if any is removed
  or narrowed, those cases return to the owner for a scoped ruling rather than
  being dropped or reported unexecuted.
- The reserve case additionally depends on the S3 reserve-wiring correction
  (§10.2): today the removable admission carries no reserve, so the prune branch
  is unreachable. Without that correction the case is returned to the owner; it
  is never reported `unexecuted` in a counted run and the reserve protection is
  then claimed only at unit/source level. The reserve case claims the
  prune-then-refuse protection on a sub-floor volume, never the unreachable
  admit branch; the concurrency case claims custody **exclusion**, never an
  interleaved observation.
- No arbitrary-write containment, no byte-saving claim, no free-space guarantee
  and no claim that a one-use slot's warm build is as fast as a normal one.
- Platform scope follows the existing proofs: macOS fixtures with the pool's
  own volume rules (including one owned 128 MiB scratch image); Linux compile
  coverage only.
