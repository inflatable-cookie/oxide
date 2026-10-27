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

**Revision 13** binds the Oxide Supervisor's design-review disposition
(2026-10-27, Queue brief v4) in three places. **Case 19 E now has one exact,
reachable matched boundary**: the existing debug-only
`OXIDE_TEST_PAUSE=keyed:<gate>` point (debug build only, `<gate>` under the
supervisor scratch), with subject and control running the *same* guarded command,
checkout and fixture transitions through it, and differing in exactly one stated
variable — the receipt/journal state **at the gate baseline** (`Prepared` planted
while paused versus `Detached`). Because the baseline is taken at the gate, the
plant's own writes and its id are outside the measured window, and the deferred
design input is a **field-scoped delta** over a named set (receipt bytes,
member/target links and readlinks, siblings, markers, reservations, registry root
entries, `cleans`, the
`waiters/` listing and `next_id`); the record phase, `next_use`, origin and the
checkout's local `target/` are outside it, so the successful control's later
`mark_spawning`/Cargo effects cannot enter the delta. The counterfactual is
**oracle deferred to the implementation proof brief per decision `dd090b31`**: it
is not established here, and the deferred brief must satisfy the five acceptance
criteria listed under case 19 E (per-allocation accounting replaces any fixed
`+1`/`+2` reading). Premise 6 (owned images)
is recorded as not applicable to E, and **no new pause seam is proposed** (the
earlier `leased:<gate>` idea is withdrawn). **The reserve/prune evidence is
split**: the native prune case runs on the **internal directory-budget pool** with
a fixture-driven `OXIDE_POOL_CAP` and one-use `Only` protection, the removable
side asserts only its shipped refusal, and the removable reserve-prune protection
is explicitly **not** claimed natively — it is covered by pure checks and unit
regressions with injected registration and size facts plus the existing removable
proofs, and the brief's requirement stays with the owner. **The bound is labelled
provisional**: 480 s / 512 MiB is an unmeasured estimate, the implementation brief
fixes the bound after the fixed script's first separately activated uncounted
diagnostic, and any final bound above 300 s requires Tom's separate ruling, which
this design does not grant.
**Revision 12** answers the CHANGES REQUIRED design review of `3df605d`
(comment `6454298750`, 2026-10-27) with two corrections. **Case 19's part E
oracle is reachable and honest**: a guarded build that reaches binding has
already registered a wait ticket and taken a lease, and `register_ticket`
(`store.rs:3630-3647`) advances and saves `next_id`, while the refusal's release
path touches the record and saves state — so a whole-command byte-identical
comparison (including `next_id`) cannot hold and is no longer claimed. The
command-level oracle is now a **matched-control delta**: the same guarded command
runs twice at the same `keyed` pause, once with nothing planted and once with the
transition planted; the ordinary lease/ticket/record deltas must match and the
test-specific delta must be empty (no clean mark, no `waiters/` entry from the
refusal, no receipt/link/marker/root/reservation change, no id beyond the
control's count). The exact `next_id` invariance is claimed only for the
**verb-level** refusals — guarded `detach`, `cancel-reservation` and the binding
revalidation — which happen before any mark exists. An exact post-lease
command-level baseline would need one new debug-only `leased:<gate>` pause point
between the lease and `outputs::bind_for_command`; it is **proposed for scoped
review**, not assumed, and the matched-control delta is the fallback oracle.
**The guarded-verb ordering is now one unambiguous contract** in every summary,
matrix and slice: `adopt` refuses unconditionally at the effect-free precheck and
never enters custody (state selects only the text); `detach` and
`cancel-reservation` revalidate the receipt and migration journal under the
checkout binding mutex **before any clean mark, id, `waiters/` file or state
save**, and only then take the `Barrier`-mode barrier across their
fence/cancellation/replay with a bounded non-blocking acquisition; a leased
command binding revalidates inside its own mutex before `gated_replay`; `clean`
keeps its refusal and the guarded text. The residual sentences that put an
`adopt` revalidation after a barrier or a `detach` revalidation under the barrier
are removed.
**Revision 11** answers the CHANGES REQUIRED design review of `281af04`
(comment `6454103541`, 2026-10-27). **The guarded `Prepared` refusal no longer
writes durable control state.** The `Barrier` mode suppressed tickets and
recovery but still allocated a clean mark before revalidating, and that
allocation advances `next_id` through `State::take_id` (`store.rs:383-386`) while
`abandon_clean` (`store.rs:2805-2810`) keeps the advance — so the "nothing
changed" claim and case 19's digest were false on exactly the race the
revalidation exists for. The design now fixes the **order**: fast precheck →
**checkout binding mutex** → **authoritative revalidation there, before any id
allocation or state write** (a `Prepared` receipt or pending migration refuses
with the exact diagnostic, having created no mark, no id and no waiter lock) →
**bounded non-blocking barrier attempts** (a blocking acquisition while holding
the mutex would invert the shipped order and could deadlock; releasing between
attempts keeps it acyclic, and budget expiry refuses without a mark) → the
`Barrier`-mode barrier held across the effects, with the live/dead/unknown
classifier deciding before the mark. The id and lock-file lifecycle is stated for
the refusal, proceeding, concurrent and crash cases. Case 19 gains part **E** — a
`Detached` receipt, a `keyed` pause, a transition published while the guarded
command is paused, then the revalidation refusal — with a **matched-control
delta** oracle: the control and test runs' ordinary lease/ticket/record deltas
match and the test-specific output-transition delta is empty (no clean mark, no
`waiters/` file from the refusal, no receipt/link/marker/root/reservation change,
no id beyond the control's count). The `next_id` invariance is claimed exactly
only for the **verb-level** refusals, which happen before any mark. An exact
post-lease command-level baseline needs the proposed `leased` pause point and is
returned for scoped review; `detach`/`cancel-reservation` have no pause point, so
their identical order is asserted purely. **The two returned owner items are unchanged and not
adopted:** the removable reserve-path wiring/protection question and the 480 s /
512 MiB bound fit.
**Revision 10** answers the CHANGES REQUIRED design review of `389a510`
(comment `6453913676`, 2026-10-27) with two corrections. **The non-recovering
barrier is now finite**: a bare `Barrier` mode that kept the shipped busy test
while removing recovery would wait forever on a same-checkout dead record (a free
record lock over a held lease) or a dead clean mark, because nothing would ever
clear them and an unbudgeted verb has no deadline. The mode now **classifies
every busy fact without mutating** — live when a waiter/record lock is held or a
clean's recorded group is non-empty (the same predicates `drop_dead_tickets`
already uses, `store.rs:3676-3693`), dead when that lock is free or missing with
no live group, unknown on a probe or read error — and decides finitely: no fact
→ one mark; all live → wait for a holder that can progress; **any dead → a
bounded non-progress refusal** with its own diagnostic, no wait and no mutation;
any unknown → refusal. Case 19 B3 now drives a dead record, a dead clean mark and
an unknown state against a `Committed` receipt, beside a live-holder wait
control. **Case 19 now reaches the custody it claims**: previously B3 began with
a pending `Prepared` receipt (which the fast precheck refuses before `begin_clean`)
and B1 began with a `Committed` receipt (which §4.2 refuses for a guarded
command), so neither established the revalidation. B1 uses a **`Detached`**
receipt (the guarded command is allowed to proceed and takes the binding mutex)
with the peer's `bound` marker and the guarded command's own later marker as
positive observations; B2 uses a **`Committed`** receipt so a guarded `detach`
enters the barrier and waits on a live peer; B3 uses **`Committed`** plus dead or
unknown facts; and A remains the separate precheck control.
**Revision 9** answers the CHANGES REQUIRED design review of `1f46767`
(comment `6453776549`, 2026-10-27) with three corrections. **Unknown seed
accounting is no longer converted into a cold build**: §8.3 now separates a
**known** complete measurement whose estimate does not fit (the only case the
brief permits to skip cold) from an **unknown** measurement — an unreadable
member root, a failed ownership/device/boundary check, partial coverage, a stale
mapping — which **refuses the command before any clone** with the predecessor's
shipped error class, exactly as the frozen contract requires of unknown
ownership/accounting/device/custody; case 24 adds the non-vacuous unreadable
regression and any loosening is returned to the owner. **The guarded Prepared
refusal no longer relies on a mutating barrier**: `Store::begin_clean`
(`store.rs:2708-2760`) drops dead tickets and runs `recover_settled` before
creating its mark, so the design adds one bounded `begin_clean` **mode** —
`Recovering` for every existing caller, `Barrier` (no ticket drop, no recovery,
no write while busy) for guarded `detach`/`cancel-reservation`, held across their
effects — with a regression that a dead lease survives the refusal
byte-identically and is recovered later by an ordinary command, and the
owner-return alternative stated if the freeze rejects the seam. **The v5
field-presence fence is now implementable**: `validate_state` sees only the
deserialized `State`, where a defaulted `class`/`class_reason` is
indistinguishable from absence, so `Store::load` gains one bounded
presence-aware decode (`serde_json::Value`/raw schema over the same
≤ `MAX_STATE_BYTES` bytes) that refuses a sub-6 document carrying either field in
any value, with six regression inputs including both explicit defaults.
**Revision 8** answers the CHANGES REQUIRED design review of `b130728`
(comment `6453630462`, 2026-10-27) with two corrections. **The normal `Prefer`
fallback now names its eligibility explicitly**: `evict_one_idle`
(`pool.rs:1786`) sees only ledger-idle, unleased rows and cannot see
control-record facts, so `Prefer` is redefined as (1) an eligible **evictable**
member, then (2) the ordinary #118 order with **every verified one-use pair
removed** — evictable, protected or unverified — leaving normal slots and idle
class-unknown orphans eligible exactly as before, and failing on the shipped
error when that set is empty instead of ever falling through to a member. The
protected pairs reach the library as an explicit exclusion set, and case 7 gains
the non-vacuous variants (protected member ranked first survives; the orphan is
chosen under #118; an empty fallback set refuses). **The version-5 serialization
is now defined safely for ordinary normal records**: `#[serde(default)]` alone
would emit `class: normal` into a version-5 document that an installed v5
reader's preflight cannot see and whose strict parser then rejects after output
effects, so both new fields use
`skip_serializing_if = "class_is_normal"` / `is_none` — a normal record written
by a v6 binary omits them and stays at version 5, a `one_use` record always
carries them with the durable fence at version 6, `validate_state` refuses a
sub-6 document that carries either field, and a pure serializer regression plus a
native legacy-read control with the identified older artifact (`489eecc`) prove
it. The candidate 480 s / 512 MiB bound and the returned reserve requirement are
unchanged owner decisions.
**Revision 7** answers the CHANGES REQUIRED design review of `f6276a4`
(comment `6453458971`, 2026-10-27) with three corrections. **Cross-pool eviction
custody is dropped from the victim predicate**, because the two directions are
not symmetric: a removable admission holds the internal admission lock first,
but an internal admission holds only its own and reads the removable sibling's
state **without** its lock (`with_sibling`/`sibling_state`,
`store.rs:2509-2525`; `sibling_barrier` returns no lock when `barrier` is false,
`2652-2654`), and a removable command can move `Acquiring → Spawning` under only
its own admission lock (`1803-1814`). The design now specifies an executable
two-direction protocol: the predicate uses only admission-guarded facts of the
pool that owns the slot (plus a non-blocking record-lock probe and an active
same-pool record for the checkout), never a cross-pool fact, with a stop
condition for any future cross-pool decision, and three non-vacuous regressions
(same-pool phase protection, cross-pool inertness, no recursive acquisition).
**The proof-control predicate is stated consistently**: the readers and the
capability boolean are the *same* `feature = "proof-controls"` predicate (not
`debug_assertions`), so the boolean describes whether the readers exist in that
artifact; a feature-enabled **release** artifact reports `true` and the installer
refuses it before replacement, which is the guarantee — no artifact built without
the feature contains a reader, and no installed binary has one. **The resource
gates are reconciled**: the supervisor's `--limit` wall clock and its monitored
512 MiB scratch budget are **enforced** gates (a breach fails the run), while the
per-phase estimates and other counters are observations; the stale revision-4
"420 s" references are marked superseded, and the 480 s estimate remains
unmeasured pending the freeze.
**Revision 6** answers the CHANGES REQUIRED design review of `0265ad1`
(comment `6453290716`, 2026-10-27) with two corrections. **The legacy fence is
now scoped-recognised by the pool opener**: `<control>/removable/state.json` is a
top-level entry of the removable library pool, so the candidate's own
`refuse_foreign_pool_entries` would refuse it at `Pool::open` (`pool.rs:762`),
`Pool::open_passive` (`810`), `snapshot` (`522`) and `describe_volume` (`583`).
The design adds one validator to that single allowlist — exactly the name
`state.json`, owner-only regular non-symlink file under a 4 KiB cap, bounded
object whose `version` is the one-use fence constant (6), `fence =
"oxide.one-use/1"` and this pool's `store_id`, anything else `unexpected_entry`
as today — validated and then
inert, never read as state, with candidate controls on all four open paths and
refusals for every invalid or foreign fence; the older-reader ordering is
unchanged. **Guarded `adopt` now refuses entirely at the effect-free precheck**
(it always refuses; state selects only the text), so case 19 no longer asserts a
barrier wait for it: binding-mutex exclusion uses a converted checkout whose
receipt-bearing binding positively holds the mutex across `bound` (with the
receipt-absent non-premise recorded), barrier exclusion uses
`detach`/`cancel-reservation` against an active peer, and the prepared transition
is committed by an explicitly **unguarded** invocation
(`OXIDE_TEST_TEMP_ROOTS=[]`, `OXIDE_TEST_WORKSPACE_CLASS=normal`) before the
guarded refusal is asserted.
**Revision 5** answers the CHANGES REQUIRED design review of `0de3d22`
(comment `6453147896`, 2026-10-27) with four corrections. **The pre-effect
hand-off now covers the real removable path**: the installed reader peeks
`<control>/removable/state.json` while the removable store loads and saves
`<control>/removable/oxide-managed/state.json`, and a pool whose only state is
removable has no internal document to carry a version — so the design adds a
durable **legacy fence document** at the peeked path (version N+1, written before
the first v6 removable state write), corrects the candidate's own peek to the
real path, and proves both with a positive control (older artifact exits 75
before any output effect), a negative control (fence removed → the old artifact
effects then fails at load) and a candidate control, using the identified
pre-feature artifact `489eecc` staged by role on the real control paths. **Seed
headroom is now measured fresh inside the seed transaction** under its own
custody, never inherited from admission, because admission releases its lock
before binding and two one-use commands could otherwise share a stale remainder
and over-clone; unknown or unfit headroom skips cold, and #137's
untrusted-recipient and failed-walk dispositions are preserved. **The reserve
wiring is withdrawn and returned to the owner**: making the prune branch reachable
would add a deletion behaviour the brief forbids inventing, so the design keeps
the shipped refusal, asserts it natively as a negative case, specifies the
class-scoped policy purely, and returns the reserve-path protection requirement
plus its wiring change for a scoped ruling. **Byte pressure is stated correctly**
for retries: a removable pool's effective cap is the recorded registration cap,
not `OXIDE_POOL_CAP`, so the retry premises use the effective cap. The declared
bound moves provisionally to 480 s with a proposed ~430 s cutoff for the added
older-artifact staging (~296 s typical / ~370 s upper, ~298 MiB) — an unmeasured
estimate, not an approved limit (revision 13 labels it provisional).
**Revision 4** answers the CHANGES REQUIRED design review of `0fb9396`
(comment `6452964156`, 2026-10-27) with three corrections. **The eviction
custody facts no longer reacquire locks**: `Store::checkout_custody` takes
`sibling_admission` and this store's admission on fresh descriptors, both of
which `Store::attempt` already holds, so calling it there would block on the
same process's own `flock`. The design now specifies one **in-custody reader**
(§5.5.2) that consumes the two state snapshots the admission already holds, with
the inherited internal-then-this-store lock order, `Unknown` for an unreadable
sibling, the same `Clear`/`Writers`/`Unknown` verdicts, no lock or file creation,
and a regression on both backends. **The reserve case was reworked** (superseded
by revision 5, which withdrew it and returned the requirement to the owner): a
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
(~267 s typical / ~335 s upper of the 420 s bound, ~258 MiB of 512; superseded
by revision 5's 480 s bound and ~296 s / ~298 MiB).

**Revision 4 also recorded one product fact** (revision 5 changed its
disposition): the removable reserve prune is **unreachable in the current
source** — `attempt` sets `LeaseRequest.reserve_bytes` from
`crate::removable::registration(self)` (`store.rs:3277-3280`), and for a
removable-store admission `self.control()` is `<owner>/removable/oxide-managed`
(`removable.rs:30`, `store.rs:1546-1547`) while the registration is written at
`<owner>/removable.json` (`removable.rs:28,298`) — so the request carries no
reserve and `unknown_removable_step` refuses with the incomplete scan
(`pool.rs:2848-2852`) before any prune. Revision 5 **withdraws** the wiring item
and the reserve case and returns them to the owner (§10.2, §19).
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
retry cases; the earlier "not induced" limitation is withdrawn (that case is
itself withdrawn in revision 5) and the 420 s /
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
bound is revised to **420 s** with the arithmetic in §16.4 (superseded by
revision 5's provisional 480 s), never silently extended.
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
| Removable pool-root allowlist | `pool.rs:3424-3457` (`refuse_foreign_pool_entries`), called from `snapshot` (`522`), `describe_volume` (`583`), `Pool::open` (`762`) and `Pool::open_passive` (`810`) | **One scoped, validated exception** for the legacy fence document; every other name keeps its shipped refusal (§10.4). |
| Ledger record and fence | `src/lease/mod.rs:45` (`MAX_SLOTS = 128`), `54-58` (`CAPACITY_POLICY_VERSION = 2`), `src/lease/pool.rs:169-232` (`Ledger`, `SlotRecord`, `RecordedSize`), `src/lease/mod.rs:295-435` (`SlotStatus`, `LedgerSlot`, `PassiveSnapshot`) | **Format unchanged, no bump.** Class lives in Oxide state; the library receives verified membership (§6.2, §10.2). |
| Accounting walks | `src/inventory.rs:436` (`inventory_directory`), `460` (`scope_directory`), the `(device, inode)` union; `local-lease.md#capacity-and-eviction` | **One bounded multi-root union walk added** for the slice (§6.4). |
| Owned-output receipt, prepared transition | `src/bin/oxide/outputs.rs:397-425` (`Receipt`), `225-232` (`Prepared`), `358-377` (`ReceiptState`), `64-76` (`RECEIPT_VERSION /2`, `PROFILE_RECEIPT_VERSION /3`, `MARKER_VERSION /2`, `PLAN_VERSION /2`) | **Format unchanged.** States are read exactly as today (§4.2). |
| Automatic first conversion | `outputs.rs:3843-3886` (`AutoFacts`/`auto_adopts`), `3888-3970` (`auto_adopt`), called from `main.rs:414` | **Skipped for a guarded workspace, before any effect** (§4.1). |
| Reservation and declared destination | `outputs.rs:5851` (`reserve`), `5355` (`lease_declaration`), `5265` (`planned_destination`), `6094` (`destination_facts`) | **Guarded commands reserve nothing and plan a local destination** (§4.1). |
| Binding and prepared replay | `outputs.rs:4196-4270` (`bind_for_command`), `4301-4324` (`gated_replay`), `7203-7330` (`replay`) | **Guard branch added**; the three production replay sites refuse before replay (§4.4). |
| Output mutation verbs | `outputs.rs:2037-2100` (`adopt_command`: barrier `2057`, fence `2075`, migration replay `2081`, receipt replay `2099`), `3983-4030` (`detach_command`: barrier `3993`, fence `4000`, `cancel_reservation` `4009`, replay `4014`), `5988-6010` (`cancel_command`), `8450-8470` (`clean_placement`, `Prepared` refusal at `8467`) | **Fast precheck, then an authoritative revalidation under the verb's own custody before every effect and before any id allocation**: `adopt` refuses entirely at the effect-free precheck (it always refuses; state selects only the text) and never enters custody; detach and cancel-reservation refuse a `Prepared`/pending-migration receipt **under the checkout binding mutex, before any clean mark exists** (so no id is consumed), and only then take the barrier for their fence/cancellation/replay; clean keeps its refusal with the guarded text (§4.1, §4.4, §4.5). |
| Proof staging helper | `scripts/test-oxide-pool-layout.py:96` (`stage_at`, imported by eight scripts) | **Explicit per-call `features` parameter**; a helper default never adds a feature, and each caller's role and fixture controls are edited where they live (§11.2). |
| Custody and closure facts | `store.rs:2572` (`checkout_custody`, an **external-command** rule and not a victim premise), `store.rs:2509-2525` (`with_sibling`/`sibling_state`: the sibling is read without its lock), `store.rs:2652-2654` (`sibling_barrier` returns no lock when `barrier` is false), `store.rs:4500` (`probe_lock`; non-creating use at `3155`), `store.rs:630-639` (`settle`), `store.rs:3374-3420` (`recover_settled`), `store.rs:399-406` (`settle_idle`), `store.rs:1803-1825` (`mark_spawning` takes this pool's admission) | **Read** by the front door to derive the same-pool `evictable` subset (§5.5.1, §5.5.2); the library never reads them. |
| Per-checkout throughput custody | `outputs.rs:6151-6167` (`barrier` = `begin_clean`, `release_barrier`), `outputs.rs:4243` (checkout binding mutex), `outputs.rs:4008,5873,6000` (other binding-mutex sites) | **Read.** Guarded revalidation runs under one of these before any effect (§4.1, §4.6). |
| Debug proof controls (existing) | `main.rs:327-352` (`OXIDE_TEST_PAUSE` = `<point>:<gate>`, debug only; `bound` fires **after** `bind_for_command`); `outputs.rs:90,7191-7200` (`OXIDE_TEST_OUTPUTS_CRASH` = a `crash_point` name, debug only; `prepared`/`migration-p0` stop after the durable prepared write); `src/inventory.rs:1402-1445,1559-1600` (`OXIDE_TEST_WALK_MUTATIONS`: ≤4 specs, ≤8192 bytes, canonical paths under the owned scratch/evidence roots, `delay_ms` 1..=65,000, one delay per spec) | **Reused, not extended**, for the custody-exclusion, prepared-state and removable unknown-figure native cases (§4.6, §16.2 cases 8 and 19). |
| Mark spawning / release / scratch discard | `store.rs:1803-1825` (`mark_spawning`), `1826-1850` (`release`), `1878-1930` (`discard_scratch`, `flush_before_idle`) | **Read.** Class never changes at release; the slot keeps its class record (§7.2). |
| Hand-off and capability probe | `src/bin/oxide/handoff.rs:39-72` (`__oxide-build-capabilities`, `hidden`), `96` (`preflight`), `112-163` (`Observed`, `observe`, `peek_version`: internal state, legacy `removable/state.json`, registration); the real removable state is `<control>/removable/oxide-managed/state.json` (`store.rs:1534-1548`, `lease/mod.rs:84`) | **Add one boolean `proof_controls`**, the installer refusal, the corrected candidate peek and the durable **legacy fence document** the installed reader actually observes (§10.4). |
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

`adopt` is the exception, and the exception is total: a guarded workspace
**always** refuses `adopt`, whatever the receipt state, so the decision is made
entirely at the effect-free precheck and the verb never enters custody. State
selects only *which text* is printed (§4.4 precedence), never *whether* it
refuses; a state change between the precheck and any later point cannot turn a
refusal into an effect, because there is no later point.

The custody differs per verb, and each is the lock the shipped code already uses
to exclude the writers of that state:

| Verb | Custody that excludes concurrent transitions | Authoritative revalidation point | Effects it therefore precedes |
| --- | --- | --- | --- |
| `oxide outputs adopt` (`outputs.rs:2037`) | **none — never entered** | at the effect-free precheck, unconditionally (state selects only the text) | barrier `2057`, fence `2075`, migration replay `2081`, receipt/migration replay `2099`, any reservation change |
| `oxide outputs detach` (`3993`) | checkout binding mutex (`store.lock_binding`) for the revalidation, then the clean barrier in **`Barrier`** mode (non-blocking attempts, held across the effects) | **under the binding mutex, before any mark exists**; the busy-barrier classification runs before the mark too | fence `4000`, `cancel_reservation` `4009`, replay `4014` |
| `oxide outputs cancel-reservation` (`5988`) | checkout binding mutex, then the clean barrier in **`Barrier`** mode | **under the binding mutex, before any mark exists**; classification before the mark too | cancellation journal, receipt write |
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

**The shipped barrier mutates, and a non-recovering one could wait forever — so
the guarded revalidation needs both a non-recovering and a *finite* acquisition.**
`Store::begin_clean` (`store.rs:2708-2760`) calls `drop_dead_tickets` and
`recover_settled` **before** it creates the clean mark, and `recover_settled` can
flush, recover a dead command's held lease and write records plus recovery
evidence. A guarded `detach` that acquired the shipped barrier and then
revalidated would therefore have changed slot state before its refusal — so the
asserted no-change guarantee would not follow from that order. Simply removing
the recovery is not enough either: the shipped busy test treats *any* active
record or `clean` entry for the checkout as busy on every poll, so a **dead**
record (a free record lock over a held lease) or a **dead** clean mark (a free
waiter lock with no live group) would never clear itself and an unbudgeted verb
would wait indefinitely. Two seams are needed — a `begin_clean` **mode** and a
**revalidation order that precedes any mark** — and both are bounded and
non-mutating on the refusal path:

**The order: revalidate before any mark exists.** The barrier's exclusion *is* a
clean mark, and creating that mark writes durable state: `begin_clean` allocates
an id through `State::take_id` (`store.rs:383-386`), which advances `next_id`
before the mark is saved (`store.rs:2734-2748`), and `abandon_clean`
(`store.rs:2805-2810`) removes the mark but **keeps the advanced `next_id`**. A
refusal taken *after* a mark therefore changes durable control state and consumes
an id on every attempt, which the brief's "nothing changed" text does not allow.
The design fixes the **order**, not the claim:

1. **Fast precheck** (read-only, no effect).
2. **Checkout binding mutex** (`store.lock_binding`, one durable per-checkout
   lock file, created once and reused; holding it writes no control state). It
   excludes a concurrent command's binding, which is the writer that can publish
   a `Prepared` receipt under the mutex alone (`outputs.rs:4214-4235`).
3. **Authoritative revalidation under the binding mutex, before any id
   allocation or state write.** The receipt and the migration journal are
   re-read; `Prepared` (or a pending migration) → release the mutex and **refuse
   with the exact guarded diagnostic having written nothing at all**: no clean
   mark, no `next_id` advance, no `waiters/<id>.lock`, no state save. This is the
   only place the guarded `Prepared` refusal is decided, and it is decided before
   the barrier exists.
4. **Barrier acquisition, non-blocking and bounded.** A blocking acquisition
   while holding the binding mutex would invert the shipped order (a normal
   `detach` holds the barrier and then takes the binding mutex, `outputs.rs:4008`)
   and could deadlock. The guarded verb therefore *tries* the barrier
   (`begin_clean(Barrier)` with a non-blocking attempt), releases the binding
   mutex between attempts, and retries on the shipped poll until a fixed attempt
   budget or the caller's admission budget expires; on expiry it **refuses**
   finitely with a bounded "the checkout barrier is busy; retry" message, again
   having written nothing (no mark was created, because a busy checkout takes no
   mark).
5. **Barrier held (`Barrier` mode) → run the effects.** Only now are the mark and
   its id allocated, which is the same state change any shipped clean/barrier
   makes and is legitimate because this verb proceeds. `Barrier` still suppresses
   the barrier's opportunistic `drop_dead_tickets`/`recover_settled`, and every
   busy fact was classified before the mark:
   - *live* (a held waiter or record lock, or a recorded non-empty group) → wait
     for a holder that can progress (the live-holder control, interruptible and
     bounded by any caller budget/watchdog);
   - *dead* (a free or missing waiter/record lock with no live group) →
     **refuse with the bounded non-progress diagnostic** before any mark exists;
   - *unknown* (probe/read error, unreadable state or sibling state) → **refuse**
     (unknown custody protected).
6. **No post-barrier `Prepared` revalidation is needed.** Command bindings are
   excluded by the binding mutex and then by the barrier (which waits for the
   checkout's commands); adopt and the prune transitions are excluded by the
   publication guard the shipped effect path takes (`outputs.rs:4011`), and any
   adopt that ran between steps 3 and 4 made our barrier attempt fail, so step 3
   re-read the receipt on the retry. A `Prepared` seen at the effect read would
   therefore be a custody defect: the design **stops and refuses** rather than
   replaying, and states honestly that such a refusal follows a mark.

**ID and lock-file lifecycle, stated.** On the *refusal* path: nothing is
allocated (no id, no mark, no lock file; the binding mutex file may be created
once, on first use, and is never removed). On the *proceeding* path: exactly one
id and one `waiters/<id>.lock` per verb, as today; the lock is removed with the
mark by `abandon_clean`/`finish_clean`; `next_id` advances once per proceeding
verb and never per refusal. On a **crash** while the mark exists, the mark and
its lock file remain and the next ordinary command's recovering barrier drops
them (`store.rs:3676-3693`: free lock, no live group), while a guarded verb's
`Barrier` classifier refuses on that dead mark until an ordinary command clears
it. Repeated refusals are therefore idempotent in the control state, including
`next_id`.

**What "nothing changed" means, precisely — and it now includes `next_id`.**
`adopt` refuses at the effect-free precheck and never reaches a barrier. On
`detach` and `cancel-reservation` **every refusal named in this section happens
before a mark is allocated**: the `Prepared` revalidation refusal (step 3), the
dead-fact and unknown-custody refusals (step 5, decided before the mark), and the
bounded busy-barrier refusal after the attempt budget (step 4). So none of them
allocates an id, advances `next_id`, creates a `waiters/<id>.lock`, drops a
ticket, recovers a lease, writes a receipt, link, sibling, root, marker,
reservation, adoption fence, migration-journal step or slot state, or replays
anything; the only lock any of them touches is the checkout binding mutex, whose
single durable file may be created on first use and is never removed. The mark
and its id exist only on the **proceeding** path, exactly as for a shipped
`oxide clean`.

**The `next_id` claim is scoped to the verb-level refusals.** It is exact for
`detach`, `cancel-reservation` and the guarded command's binding revalidation,
because those refusals happen before any mark exists. It is **not** a claim about
a whole command: a guarded *build* that reaches binding has already registered a
wait ticket and acquired a lease, and `register_ticket` (`store.rs:3630-3647`)
advances and saves `next_id`, while the refusal's release path
(`main.rs:535-541`, `store.rs:1826-1843`) touches the record and saves state. So a
command-level before/after comparison cannot be byte-identical and this design
never claims it is.

> **Oracle deferred — decision `dd090b31`, review `6454799468`.** The
> command-level oracle sketched in the rest of this subsection, and the
> "baseline" wording it rests on, are **not a proved oracle and are not
> acceptance evidence**. They are marked here as *oracle deferred to the
> implementation proof brief per decision `dd090b31`*; the binding requirements
> for that later brief are the five acceptance criteria listed under native case
> 19 E (§16.2). Nothing in this design claims the command-level oracle is
> satisfied, and the direct verb-level no-mark/no-id assertions below are the only
> oracle claims this document makes.

The sketch, retained only as design input for that brief, is a
**matched-control delta**:

- run the guarded command twice at the same `keyed` pause — once with no planted
  transition (the control) and once with the transition planted while paused (the
  test);
- compare the two runs' **deltas** from the same pre-run baseline: the ordinary
  lease/ticket/record lifecycle deltas must match, and the **test-specific delta**
  must show zero output-transition effect — no clean mark, no `waiters/<id>.lock`,
  no receipt, link, marker, root or reservation change, and no id consumed
  *beyond the control run's count*;
- keep the direct, non-vacuous verb-level assertions (a `detach` or
  `cancel-reservation` refusal allocates no mark and no id; repeated such
  refusals are byte-identical including `next_id`).

An exact post-lease baseline would make the command-level comparison exact, but it
needs one **new debug-only pause point** immediately after `lease_with_gate` and
before `outputs::bind_for_command` (a `leased:<gate>` marker, bounded like
`bound`: marker file under the owned scratch, 120 s self-release, absent from
release builds). That seam is **proposed for scoped review**, not assumed; if it
is rejected, the matched-control delta above is the oracle and the design does not
claim an exact command-level `next_id`.

If the freeze instead prefers the shipped barrier order to be kept exactly (so
the guarded revalidation stays after a mark), that conflict is **returned to the
owner**: the "nothing changed" text cannot then hold, because every refusal would
advance `next_id` and create and remove a waiter lock. The design does not claim
that variant.

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

- **`oxide outputs adopt`** on a guarded workspace **refuses unconditionally at
the effect-free precheck and never enters custody** — it allocates no mark, no
id and no lock, and no later verb state can turn the refusal into an effect. The
receipt state selects only the *text* (§4.1 precedence): `Prepared` or a pending
migration journal → the exact diagnosis; every other state → the general refusal
naming that this workspace is one-use/temporary and cannot own an output root.
- **`oxide outputs detach`** remains available and safe exactly as shipped: for
a `Committed` placement whose live path is the exact owned link it becomes a
real directory again (profile layout: every owned member link becomes a real
directory, `target` stays real) with **no root moved, copied or deleted**, and
for a valid `Detached` receipt it is the existing no-op. On a guarded workspace the receipt and the migration journal are re-read
**under the checkout binding mutex, before any clean mark, id, `waiters/` file
or state save exists**, and a `Prepared` receipt (or pending migration journal)
refuses per §4.4; only after that passing revalidation does the verb take the
`Barrier`-mode barrier (bounded non-blocking acquisition) across its
fence/cancellation/replay. A normal checkout keeps the shipped order.
- **`oxide clean`** keeps its refusal for `Prepared`; on a guarded workspace its
text is the exact guarded diagnostic (§4.4). Everything else about clean is
unchanged, and clean is not a one-use-specific verb.
- **`oxide outputs cancel-reservation`** is the one additional receipt-writing
verb, and it follows the same order as `detach`: the receipt and migration
journal are re-read **under the checkout binding mutex, before any clean mark or
id exists**, and a guarded workspace refuses a `Prepared` receipt or pending
journal there (same diagnostic, no advice). Only a passing revalidation takes
the `Barrier`-mode barrier and runs the shipped cancellation; it publishes no
root and selects nothing.
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

The guard is evaluated in these places, and only the custody ones are
authoritative:

1. during planning (`planned_destination`, `main.rs:456`), where the expected
   destination is `None` for a guarded command. This is the fast precheck: it
   can refuse, but it can never authorise. For a guarded workspace it is the
   **whole** decision for `adopt`, which never enters custody (§4.1);
2. inside `bind_for_command` **under the checkout's binding mutex**
   (`store.lock_binding`, `outputs.rs:4243`) before `gated_replay`, where the
   receipt, the migration journal and every relevant component are re-read;
3. for `detach` and `cancel-reservation`, under the **checkout binding mutex
   first** (before any mark), then the clean barrier in its `Barrier` mode held
   across their effects (§4.1). The shipped recovering barrier is unchanged for
   every other caller.

A transition published between (1) and (2)/(3) — by a concurrent adopt, detach,
cancel-reservation or command binding — is therefore always seen by the
authoritative revalidation, which refuses with the guard's own diagnostic or,
for a changed destination, the shipped "this checkout was … while this command
waited … nothing was built" shape. A guarded command never proceeds on the
strength of the precheck, and a slot keyed for one destination never binds
another. `bind_for_command`'s `expected = None` path already exists
(`keyed_state_matches(None, expected)`), so a guarded command fails closed if a
receipt appeared while it waited.

**Regression (native case 19).** Every part names a **transaction state that lets
the guarded operation pass its fast precheck**, with positive observations for the
custody it claims. All controls are existing debug facilities, so the design
proposes **no new seam**:

| Part | Checkout state (passes the precheck) | Custody premise, positively observed | Assertion |
| --- | --- | --- | --- |
| **A** precheck control | receipt `Prepared` / pending migration (from `OXIDE_TEST_OUTPUTS_CRASH=prepared\|migration-p0`) | none — deliberately does **not** enter custody | exact diagnostic from the fast precheck; digest identical. Labelled as the precheck control, not as barrier coverage |
| **B1** binding mutex | receipt **`Detached`** (§4.2 lets a guarded command proceed) | peer paused at `OXIDE_TEST_PAUSE=bound:<gate>`: because a receipt-bearing binding takes `store.lock_binding` (`outputs.rs:4214-4235`) and keeps it until `mark_running` (`main.rs:701`), the peer holds the mutex at `bound` (`main.rs:545`). Observed by the peer's own `bound` marker file, then the guarded command stays alive and writes nothing; after the gate opens the **guarded command's own `bound` marker appears**, proving it acquired the mutex and passed the pause | no outputs write while blocked; both `bound` markers observed; final state matches |
| **B2** barrier, live holder | receipt **`Committed`** (guarded `detach` is allowed; `cancel-reservation` too) | live peer paused at `bound` = an active same-checkout record; the guarded verb enters `begin_clean(Barrier)`, finds only **live** facts, and waits | the verb has not exited while the gate is closed and wrote nothing; after release it completes with the shipped detach output |
| **B3** barrier, dead facts | receipt **`Committed`** (so the precheck passes) | (i) a same-checkout **dead record**: free record lock over a held lease; (ii) a same-checkout **dead clean mark**: free waiter lock, no live group; (iii) unreadable state/sibling | **bounded refusal** with the non-progress diagnostic inside the case's wall clock; record, lease and recovery log byte-identical; the dead lease still present. After an ordinary command recovers it, the same guarded `detach` proceeds |
| **E** in-custody revalidation | **one boundary**: the existing `OXIDE_TEST_PAUSE=keyed:<gate>` point (debug build only, `<gate>` under the supervisor scratch); same guarded command, same checkout, **`Detached`** receipt in both runs, same fixture transitions through the boundary | baseline is taken **at the gate** (so the fixture's own plant and its id are excluded); the single differing variable is the receipt state at that baseline — `Prepared` (planted while paused) for the subject, `Detached` for the control; both then open the gate and run to exit | exact diagnostic **from the revalidation** (the positive `keyed` marker shows the precheck had accepted); oracle is a **field-scoped delta over exactly**: receipt bytes, member/target links and readlinks, sibling links, root markers, reservations, registry root entries, `cleans`, the `waiters/` listing and `next_id` — **oracle deferred: per-allocation accounting per acceptance criteria 1–2 (decision `dd090b31`)** — no fixed `+1`/`+2` expectation is claimed, no clean-mark detection is claimed, and the deferred brief must satisfy all five criteria. Record phase/`next_use`/origin/local `target/` are **outside** the compared set, so the control's successful later effects cannot enter the delta. Premise 6 (owned images) does **not** apply to E and is recorded so. No new seam |
| **C** commit | prepared transition committed by an explicitly **unguarded** invocation (`OXIDE_TEST_TEMP_ROOTS=[]`, `OXIDE_TEST_WORKSPACE_CLASS=normal`) | — | the shipped `oxide outputs adopt` replays and commits; the guarded invocation afterwards sees `Committed` and refuses per §4.2 |

Two premises are recorded as **not** established, and the case does not assert
them: a **receipt-absent** bind returns `OutputBinding::unchecked` with no mutex,
so no exclusion is claimed there; and a guarded `adopt` never reaches a barrier
(it always refuses at the precheck), so B2/B3 use `detach`/`cancel-reservation`.

`detach` and `cancel-reservation` have **no debug pause point**, so their
in-custody revalidation cannot be interleaved deterministically with an existing
control. Part **E** therefore drives the identical step-3 order through a guarded
**command** (whose binding revalidation shares it), and the two output verbs'
order is asserted **purely** (binding mutex → revalidate → refuse with no id
allocation) plus the repeated-refusal idempotence. A native interleaving for
those two verbs would need one new pause point inside their binding section and
is **returned for scoped review** rather than claimed.

Both controls are debug-only and inert in a release build; they are existing
product test facilities, not new product behaviour. If the freeze wants a
literally interleaved observation of the *effect* window, that needs a **new**
debug seam inside the barrier and must be proposed and reviewed separately — this
design does not claim one.

**Committing the prepared transition, explicitly unguarded.** Guarded `adopt`
can never replay it, so the commit runs as a **deliberately unguarded
invocation** of the same build with the proof controls that remove both
predicates: `OXIDE_TEST_TEMP_ROOTS=[]` and `OXIDE_TEST_WORKSPACE_CLASS=normal`
(feature build only; `guarded = false`). That invocation runs the shipped
`oxide outputs adopt`, replays the recorded transition and commits it; the
*guarded* invocation afterwards sees `Committed` and refuses per §4.2. The
production route is the same idea without controls: the transition belongs to a
checkout that is no longer a temporary workspace. The case states this premise
rather than implying a guarded adopt can replay.

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

### 5.1 The record field, its exact on-disk form and its fence

`Record` (`store.rs:155`, `#[serde(deny_unknown_fields)]`) gains two fields with
an **explicit omission rule**, because a v5 reader's `Record` denies unknown
fields and its preflight only compares numeric versions:

```text
#[serde(default, skip_serializing_if = "class_is_normal")]
class: WorkspaceClass                       // "normal" | "one_use"
#[serde(default, skip_serializing_if = "Option::is_none")]
class_reason: Option<String>                // bounded, <= MAX_DIAGNOSTIC_BYTES
```

`#[serde(default)]` alone is **not** enough: it makes legacy documents parse but
still serialises the field, so an ordinary **normal** record rewritten by a v6
binary would emit `"class":"normal"` into a **version-5** document that an
installed v5 reader cannot see coming (its preflight sees 5, not newer) and that
its strict parser then rejects after output effects. The representation is
therefore:

| Record | On-disk bytes | State version |
| --- | --- | --- |
| legacy normal (no class) | **neither field present** | 5 or below |
| normal, written by a v6 binary | **neither field present** (`class_is_normal` and `is_none` both skip) | 5 or below, unchanged |
| `one_use` | `"class":"one_use"`, plus `"class_reason"` when a reason exists | **6**, fence present |

So a document containing either field **always** has the durable
`one_use_workspaces: "oxide.one-use/1"` fence and numeric version 6, and a
document without the fence contains neither field. Ordering, for **every** state
save in the front door:

1. the one-use fence is written durably (its own atomic save) **before** the
   first state document that carries a class-bearing record;
2. `save()` advances the numeric version to **`STATE_VERSION = 6`** when that
   fence is present (`store.rs:3776-3804`'s named-fence rule), and leaves a
   normal-only pool at its existing version (5 or below);
3. a version-5-or-below document never contains `class`/`class_reason`, so a
   v5 reader's ordinary rewrite stays readable by a v5 binary.

Reading is defensive in both directions, and the field **presence** check needs
its own pass, because `Store::load` (`store.rs:3744-3774`) hands
`validate_state` only the deserialized `State`: with `#[serde(default)]`, an
explicit `"class":"normal"` is indistinguishable from an absent field and an
explicit `"class_reason":null` from absence. `#[serde(default)]` alone therefore
makes the described fence necessary; it does not implement it. The load path
gains one **bounded presence-aware decode** before the typed parse:

- over the *same* bounded bytes (≤ `MAX_STATE_BYTES`) and with no re-serialisation
  and no walk of the payload, it parses the document once as
  `serde_json::Value` (or an equivalent `#[serde(default)]` raw schema) and, for
  a numeric `version` below 6, refuses when **any** object in `records` has a
  `class` key with any value (including `"normal"`) **or** a `class_reason` key
  with any value (including `null`);
- at version 6 with the fence, presence is expected and the typed parse decides
  as before; at version 6 without the fence it refuses (existing rule);
- the pass is effect-free and bounded, and it runs **before** `validate_state`;
  `validate_state` keeps its structural checks (unknown class value, reason
  bound, `one_use` facts) and no longer carries the presence rule, which it
  cannot see;
- regression inputs cover **both explicit default values as well as absence**: v5
  with no fields (accepted), v5 with `"class":"normal"` (refused), v5 with
  `"class_reason":null` (refused), v5 with `"class":"one_use"` (refused), v6 with
  the fence and a class (accepted), v6 without the fence (refused).

**Proof of the legacy serializer.** A pure regression serialises (a) a freshly
created normal record and (b) a rewritten normal-only state and asserts the bytes
contain neither `"class"` nor `"class_reason"`, and that a round trip through the
v5-shaped `Record` succeeds. A native control (case 14) has the **identified
older artifact** (`489eecc`, state 5) read and rewrite a state written by the
candidate's ordinary normal creation: it must succeed as a legacy read with no
hand-off, while the same artifact refuses a v6 one-use state before any output
effect.

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
| 4 | no other record **in this pool** for `record.checkout` is active (`Acquiring`/`Spawning`/`Running`), and no `Retired` record's `lost.unclosed` is open for this boot session. Same-pool only, from the snapshot this admission holds (§5.5.2); cross-pool state is never a premise | control state | protected |
| 5 | `process_group` is `None`: a recorded group means a writer may still exist. `settle_idle` (`store.rs:399-406`) clears it, and `recover_settled`'s `Reset` (`store.rs:3397-3402`) only drops a pending recovery — it leaves the active record and its group in place, so phase-`Idle` plus no recorded group is the positive closure fact | control state | protected |
| 6 | the ledger slot is `Idle`, `lease_id` is `None`, and its `(slot_id, incarnation)` equals the member pair | ledger view | protected |
| 7 | no donor pin covers the slot (§8.2) and no `oxide clean` for that checkout is recorded | control state | protected |

### 5.5.2 The in-custody reader and the two custody directions

`Store::checkout_custody` (`store.rs:2572-2637`) is **not** usable from inside an
admission. It takes `sibling_admission()` and then `self.admission()` on fresh
descriptors (`store.rs:2639-2654`); `Store::attempt` already holds its own
admission lock (`store.rs:2978`), and for a **removable** admission it also holds
the internal one through `sibling_barrier` (`store.rs:2656-2680`). On macOS
`flock` is per open file description, so a second acquisition from the same
process does not succeed recursively: it **blocks**, which would make every
admission with a one-use member hang instead of admitting or refusing.

**The two directions are not symmetric, and the victim predicate must respect
that.** `with_sibling` documents it (`store.rs:2509-2525`): the other pool's
state is "read without its own lock". `sibling_barrier` returns no lock at all
when `sibling.barrier` is `false` (`store.rs:2652-2654`), and the internal store
carries exactly that (`removable::select` pairs it with `barrier = false`). So:

| Admission | Locks actually held | Stable cross-pool snapshot? |
| --- | --- | --- |
| **removable** store | internal sibling admission first, then this store's admission | **yes** for both pools |
| **internal** store | this store's admission only; the removable sibling is read unlocked | **no** |

The shipped lock order is internal-admission → this-store-admission, and a
recursive re-acquisition is forbidden; that leaves no way for an internal
admission to hold the removable lock too. A removable command can moreover move
`Acquiring → Spawning` under only its own admission lock (`store.rs:1803-1814`)
after an internal snapshot said `Clear`, so **an unlocked cross-pool read can
never be a victim premise**.

**The protocol, executable and symmetric-safe:** the `evictable` predicate uses
**only facts of the pool that owns the slot**, all of them guarded by that pool's
admission lock — which the front-door admission holds across the whole library
call — plus one non-blocking probe of the member's own record lock:

```text
fn member_is_evictable(
    this_state: &State,        // the owning pool's state, loaded under its
                               // admission lock by Store::attempt
    member: &LedgerSlot,       // the pool lock's row for (slot_id, incarnation)
    checkout: &Path,
    own: u64,
) -> bool                      // every clause below must hold
```

- the member's owning record (same pool, same `affinity_dir`) has phase `Idle`,
  no holder, no `lease_id`, no `pending_recovery`, no `retired`, no `lost`, and
  `process_group` is `None` (§5.5.1 items 1, 2 and 5);
- `probe_lock(record_lock, false) == Probe::Free` — non-blocking, so it cannot
  deadlock and it needs no admission lock (§5.5.1 item 3);
- **no other record *in this pool* for that checkout is active**
  (`Acquiring`/`Spawning`/`Running`) and no `Retired` record's
  `lost.unclosed` is open for this boot session; the caller's own record id is
  excluded exactly as `checkout_custody`'s `own` argument does. This is the
  conservative, same-pool form of "the checkout is busy": it protects *all* of
  this pool's one-use slots for that checkout, and it replaces the removed
  cross-pool clause;
- the ledger row is `Idle`, unleased, and matches the pair (§5.5.1 item 6);
- no donor pin covers the slot and no `oxide clean` for the checkout is recorded
  (§5.5.1 item 7).

**Cross-pool facts are used for nothing on the victim path.** A one-use slot
lives in exactly one pool, so only a writer *of that pool* can write the victim's
tree; a command using the other pool writes the other pool's slot. The checkout
level fact `checkout_custody` adds is an *output* custody rule (a relink must not
race a writer), not a slot-deletion premise. If a future one-use decision ever
needs a cross-pool fact, the design **stops**: it must first acquire that pool's
admission lock in the shipped internal-first order (never recursively), re-read
the snapshot under it, and re-validate immediately before the deletion.

**Transitions after admission cannot invalidate the decision.** The member's own
record cannot change between the snapshot and the deletion, because every
front-door record transition (`mark_spawning` at `store.rs:1803-1825`,
`mark_running`, `release`, `recover_settled`) takes **that pool's** admission
lock, which the admission holds for the whole library call; the record's lock
probe is non-blocking; and the ledger row is re-checked under the pool lock
immediately before `retire_idle_slot` (§5.5.1 "Revalidation through the
deletion"). A cross-pool transition is irrelevant by construction, and the
regression asserts exactly that.

**Required regressions.** Three, all non-vacuous:

1. *Same-pool interleaving.* A one-use member whose owning record moves
   `Idle → Acquiring → Spawning` while an admission runs is **protected**: the
   admission refuses (or waits for that progress-capable holder) and the slot's
   root and ledger row are byte-identical. This is the case the admission lock
   makes stable, and it is driven by pausing the peer at `bound` (case 19's B2
   machinery) plus a pure check of the predicate on each phase.
2. *Cross-pool interleaving is inert.* A **removable** command of the same
   checkout moves `Acquiring → Spawning` while an **internal** admission with a
   one-use member runs: the internal admission's decision and result are
   unchanged (its member is evicted or retained exactly as in the control run
   with no peer), the removable command's record and slot are untouched, and a
   pure check asserts the predicate reads no sibling state at all.
3. *No recursive acquisition.* A pure/regression assertion that the in-custody
   reader opens no admission file, creates nothing and attempts no `flock`, and
   that an internal and a removable admission with a member each reach
   admit/evict/refuse inside the guard's deadline (cases 5, 7 and 8).

`Store::checkout_custody` keeps its signature and its existing callers; the
external-command relink path is unchanged. Nothing about unknown-writer
protection weakens: the predicate is the *same facts for the same pool*, with the
cross-pool clause removed because it was never stable for an internal admission.

The `evictable` set is passed to the library as its **only** allowed one-use
victim source; membership in `members` alone is never enough. The **protected**
pairs are passed too, as an explicit *exclusion* set for every normal `Prefer`
fallback (§6.7), so the ledger-only predicate can never reach a member the
checklist protects. A member that is counted
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
deleting a slot that may still have a writer. **The same-pool scope is
deliberate:** the removable sibling's state is read without its lock
(`store.rs:2509-2525`), so a removable transition cannot be a stable premise for
an internal eviction; §5.5.2 states why that is sound (slots are per pool) and
what a future cross-pool decision would have to acquire first.

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
                                // Prefer { preferred, excluded }
                                //   -- evictable members first, then the
                                //      ordinary order with `excluded` removed
                                // Only(&MemberSet)    -- evictable members only
slice: Option<OneUseSlice>,     // Some for every new-slot admission and every
                                // OneUse admission; None where the brief
                                // forbids a measurement (normal warm reuse)

pub struct MemberSet(/* at most MAX_SLOTS (slot_id, incarnation) pairs */);
// Two sets, never interchangeable: `accounting` counts bytes and slots,
// `evictable` is the only *one-use* victim source and is derived by the front
// door's control-record custody checklist (§5.5.1); `excluded` is every
// verified one-use pair and is subtracted from the normal Prefer fallback
// (§6.7). `evictable ⊆ excluded ⊆ accounting`.
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

The front door passes the library **both** sets: the `evictable` pairs as the
only one-use victim source, and **every** verified one-use pair (evictable,
protected and unverified) as the exclusion set that the normal `Prefer` fallback
subtracts before ranking anything (§6.7). The mapping is stable because the front
door holds its control-state admission lock (`store.attempt`, `store.rs:2978`)
across the whole library call, so no other Oxide command can create, release or
evict a slot of this pool while the membership is being used, and the exclusion
set cannot go stale behind the call. The library itself reads no Oxide record and cannot
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
warm reuse. The measured union is an **observation at admission**, not a
reservation: nothing survives the admission lock to reserve slice bytes for a
later step, and the seed transaction re-measures under its own custody (§8.3). Concretely, with 16 idle one-use members within the byte bound: a
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
| normal, new slot | yes (`Prefer`) | preferred **evictable** member first, then the fallback set with **every one-use pair removed** (normal slots and idle class-unknown orphans only) | may reclaim idle one-use slots first; continues with a reported overage/lower bound when it must; refuses on its ordinary reasons when the fallback set is empty |
| one-use, warm reuse | yes (`Only`, candidate excluded) | the verified member slot only | admissible at 16 members while bytes fit and `unknown == 0`; a non-member idle slot is refused, never relabelled |
| one-use, new slot | yes (`Only`) | new slot, `member_count <= 15`, union within the slice, `unknown == 0` | refuses or waits per §6.9 |

A guarded-from-membership command whose admission class is forced `normal` by a
proof control is the "normal" rows; the local-output guard is unaffected (§3.6).

### 6.7 Every pressure path, named

**The three victim sets, exactly.** They are disjoint by construction and the
library selects from exactly one of them per admission class:

- **`Prefer` (normal admission).** Step 1: an eligible **evictable** one-use
  member, oldest `(rank, slot_id)` first (§6.2). Step 2 — the fallback — the
  ordinary #118 `(rank, slot_id)` order over ledger-idle, unleased rows with
  **every verified one-use pair removed**, whether evictable, protected or
  unverified. So the fallback may pick a **normal** slot or a positively idle
  **class-unknown orphan** exactly as today, and it can **never** pick a one-use
  member. `evict_one_idle` (`pool.rs:1786`) cannot see control-record facts, so
  this exclusion is applied by the class-scoped selector the library uses for
  every pressure path — not by the ledger-only predicate. If no row survives the
  exclusion, the path fails with its shipped error (`Full`,
  `LedgerExhausted`, `Busy`, `unknown_accounting`) and **never** falls through to
  a one-use member.
- **`Only` (one-use admission).** An eligible **evictable** one-use member only,
  oldest first; if there is none, the path fails exactly as it fails today when
  no victim is eligible. A normal slot, a class-unknown orphan, a protected
  member and an unverified pair are all outside this set.
- **`Any` (the default for a library caller that passes no policy).** Today's
  behaviour, unchanged, for callers outside this feature; Oxide always passes
  `Prefer` or `Only` (§6.1), so `Any` never reaches a one-use pool through the
  front door.

**Why the explicit exclusion is load-bearing.** `evict_one_idle` filters on
ledger-idle and unleased rows only; it cannot see a member's record phase, lock,
holder, pin or `process_group`. Without step 2's exclusion, a `Prefer` fallback
would select exactly the ledger-idle one-use member the checklist calls
*protected* — a member whose record lock is held, or whose custody is uncertain —
whenever no preferred member is eligible. The selector therefore computes the
protected pairs once (§6.2) and subtracts them from the fallback candidate set
before it ranks anything.

| Path (current source) | Normal admission | One-use admission |
| --- | --- | --- |
| slot count ceiling `evict_idle_until(MAX_SLOTS - 1)` (`pool.rs:1084`) | `Prefer` | `Only` (a normal victim is impossible) |
| byte budget `evict_idle_until_under_cap` (`pool.rs:1759`) | `Prefer` | `Only` |
| project-cap replacement `replace_idle_slot_for_admission` (`pool.rs:1399`) | named victim from the **normal-only** front-door list, re-verified as a non-member | not reachable (one-use is not subject to the project cap) |
| removable reserve route `admit_unknown_removable` (`pool.rs:1455`) | `Prefer` | `Only`. The route's prune branch is **unreachable today** (no reserve is carried, §10.2); the policy is specified so a scoped wiring could not pick a normal victim for one-use, and this design changes no behaviour |
| post-eviction pressure re-read (`allocate`, `pool.rs:1088-1115`) | `Prefer`: preferred member, then the fallback set (normal slots and idle class-unknown orphans, **all one-use pairs removed**) | `Only`; a `Full` that remains is a safety refusal, never a normal victim |
| retry after a victim refusal | stops with the shipped error (`Busy`/`unknown_accounting`) | stops; never falls through to a normal victim |
| unknown accounting (`recorded_total`, `pool.rs:1546`) | unchanged refusal (`unknown_accounting`) | unchanged refusal |
| class-unknown ledger rows (§5.6) | continues with the lower-bound observation; no stall | **refuses** `unknown_accounting` |
| count replacement with only bound gaps on a removable pool (`pressure_known_or_bounded_removable`) | unchanged | unchanged |

No row ever lets a one-use admission select a normal victim; no row ever lets a
normal admission's fallback select a one-use member — protected, unverified or
evictable — because every one-use pair is removed from that set; and no row
stops a normal admission *solely* because the one-use slice is over (it continues
with a measured overage or a lower-bound note and reaches its ordinary decision).
A normal admission may still be **refused** for its own ordinary reasons, or stop
on a safety failure of its selected victim, exactly as today.

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
- **the seed's headroom is measured fresh inside the seed transaction, never
  inherited from admission.** The admission-time union is only a *precondition
  for attempting* seeding; it is not an amount reserved. Admission releases its
  lock before output binding, and #137's seed transaction runs later
  (`cold-slot-seeding.md` §6.1-6.3), so two one-use commands can both be admitted
  while their new slots are empty, both carry the same stale headroom, and then
  clone one after the other. The seed decision therefore re-measures, under the
  seed transaction's own custody (its donor-record-then-pool lock, which
  serialises clones), the bounded union over the **current** verified membership —
  every member's cloned bytes already on disk plus the recipient's own slot — and
  requires the frozen estimate to fit that fresh remainder. **Two different
  outcomes, never conflated:**

  - a **known** measurement that is complete and positively shows the estimate
    does not fit the remaining slice → **skip seeding** with a bounded reason and
    build cold in the same slot. That is the only case the brief permits to skip
    cold;
  - an **unknown** measurement — an unreadable member root, a failed
    ownership/device/boundary check, a partial coverage, a stale or missing
    mapping, or any other safety/accounting fault in this new pre-clone walk →
    **refuse the command**, before the first clone, with the predecessor's
    shipped error class (`unknown_accounting`, ownership, device or custody
    refusal). This is exactly what the frozen contract requires of unknown
    ownership, accounting, device or custody (`cold-slot-seeding.md` §8.3, §13);
    it is not a harmless miss and it is **never** converted into a cold build in
    the same slot. Loosening this needs an owner ruling or a revised brief, not a
    design choice here.

  #137's untrusted-recipient rule and its failed-post-clone-walk dispositions are
  unchanged: only a failed **post-clone recipient walk** abandons seeding and
  cold-builds (releasing and retaining the untrusted generation first), every
  other post-clone fault refuses and retains, and an abandoned generation keeps
  counting toward the slice until it is evicted or closed.

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

Per-record migration is the omission rule of §5.1: a record with no `class` reads
`normal`, which is the protected class (§5.7), and a **normal** record written by
a v6 binary omits both new fields so a v5 reader's ordinary rewrite stays
readable. The fence, not the per-record default, is what tells a *newer* binary
that this state may contain one-use records, and any document carrying either
field always carries the fence at version 6.

### 10.2 Ledger, receipts and registration unchanged

- Ledger: format and `CAPACITY_POLICY_VERSION = 2` unchanged (§6.11).
- **A wiring gap is recorded and returned to the owner, not changed here.** A
  removable admission carries **no** reserve today: `attempt` reads
  `registration(self.control())` (`store.rs:3277-3280`) while the registration
  lives in the internal control (`removable.rs:28,298`), and a removable store's
  `control()` is `<control>/removable/oxide-managed` (`store.rs:1546-1547`,
  `removable.rs:30`). `unknown_removable_step` therefore always takes its
  no-reserve branch and refuses with the incomplete scan
  (`pool.rs:2848-2852`), so **the reserve prune never runs**. Wiring the reserve
  would make a previously unreachable branch delete slots — a *new* cleanup
  behaviour, which the brief expressly forbids inventing ("if a reserve path does
  not evict today, do not invent a cleanup to satisfy it"). This design therefore
  leaves the path's behaviour exactly as shipped and **returns the reserve-path
  protection requirement, together with that wiring change, to the owner for a
  scoped ruling or a revised brief** (§19). No format, reserve value, floor or
  deletion behaviour changes here.
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

`proof_controls` is exactly `cfg!(feature = "proof-controls")`, and **the readers
are `#[cfg(feature = "proof-controls")]`** — the same predicate on both sides, so
the boolean describes precisely whether the *readers exist in that artifact*.
`debug_assertions` is not part of the predicate: a debug build without the
feature has no readers and reports `false`, and a release build with the feature
has readers and reports `true`. (Every proof that needs a control therefore
stages with the feature, including any debug fixture; the static edits in §11.2
name that per role.)

The installer (`scripts/oxide-install.py`) refuses a candidate whose probe output
lacks the field, does not parse as a JSON boolean, or reports `true` **before any
replacement**, and it stages its own build with the feature explicitly disabled,
recording the artifact evidence in its receipt. So **no installed binary ever has
a reader**, even though a feature-enabled release artifact can exist and report
`true`. Older receipts that
lack the field remain valid for the existing replace-ownership match only; a
receipt without `proof_controls` cannot authorize installing a candidate
(matching the brief: "older pins lacking that artifact-probe field cannot be
installed by the revised installer"). This design performs no install.

### 10.4 Pre-effect hand-off: the real removable path and its legacy fence

`handoff::preflight` runs at `main.rs:203`, before any store command arm. It
peeks the internal state (`<control>/state.json`), a removable document at
`<control>/removable/state.json` and the registration (`handoff.rs:122-135`,
`STATE_FILE`, `POOL_DIR`, `REGISTRATION`). **But the removable store's state is
not at that path.** `Store::open_removable` sets `control =
<control>/removable/oxide-managed` (`store.rs:1534-1548`, `MANAGED_NAME` at
`lease/mod.rs:84`), so the removable state a v6 binary loads and saves is
`<control>/removable/oxide-managed/state.json`. The legacy peek therefore
**cannot see** a newer removable state, which is a real gap for an already
installed reader:

- a pool whose only state is the removable one (a user who only ever built via a
  registered drive) has no internal `state.json` to carry a version, so an
  installed v5 reader's preflight observes nothing newer, runs `auto_adopt` and
  binding, and only then refuses at its strict `Store::load`. That is exactly the
  "refuses after output effects" outcome the brief forbids.

Two changes, in a fixed order, close it:

1. **A durable legacy fence document, scoped-recognised by the pool opener.**
   Before the first v6 write to `<control>/removable/oxide-managed/state.json`,
   the v6 binary atomically publishes a bounded, owner-only document at
   `<control>/removable/state.json` whose `version` is `N+1` (6) and which also
   records the pool's `store_id` and the fence name (`oxide.one-use/1`).
   `peek_version` reads only the numeric `version` through a plain
   `serde_json::Value` (`handoff.rs:148-163`), so any #122-era reader that peeks
   that path sees `6 > 5` and hands off **before** its own output effects. The
   fence is never removed by this feature and is written once per pool.

   That path is also the **root of the removable library pool**, and
   `refuse_foreign_pool_entries` (`pool.rs:3424-3457`) does not allow
   `state.json` there. Without a scoped exception the candidate would refuse its
   own pool at `snapshot` (`pool.rs:522`), `describe_volume` (`583`), `Pool::open`
   (`762`) and `Pool::open_passive` (`810`) — so the fence would break the pool
   it protects. The design therefore adds **one validator**, called from that
   single allowlist, with these exact rules:

   - the name must be exactly `state.json`; nothing else becomes acceptable, and
     every other name keeps today's `unexpected_entry` refusal;
   - it must be a regular, owner-only, non-symlink file opened `O_NOFOLLOW`,
     bounded by a small cap (4 KiB), with no group/other permission bits — the
     same custody rule `open_control_file` uses for hand-off documents;
   - it must parse as a bounded object with exactly three fields: `version`
     equal to the **one-use fence constant** (6 — the version this feature
     publishes, which an installed v5 reader sees as newer and a v6 or later
     build recognises as its own known fence), `fence` equal to
     `"oxide.one-use/1"`, and `store_id` (bounded text) equal to the pool's own
     `store_id`, read from the config/ledger with the existing bounded readers;
   - **any** deviation — wrong name, symlink, wrong owner or mode, oversize,
     unparseable, an unknown `fence` string, a `version` that is not the fence
     constant (including a higher number this build did not publish), a missing or
     non-numeric version, a missing or mismatched `store_id`, or an unreadable
     config/ledger — is `unexpected_entry` exactly as today. The validator never repairs, never ignores an error and
     never waits;
   - the file is **validated and then ignored**: the library never reads it as
     state or ledger, never writes it and never deletes it. The candidate's own
     `peek_version` ignores it because `6 > 6` is false.

   So the fence is neither a ledger nor a second state document, and a stranger
   cannot use it to place an arbitrary file in the pool: it must already be a
   well-formed fence for *this* pool, and it is inert otherwise. A v5 binary that
   somehow gets past its preflight still refuses the pool at this same allowlist,
   which is fail-closed: no build runs against a v6 removable pool.
2. **The candidate's own peek is corrected** to read the real removable state
   path as well (`<control>/removable/oxide-managed/state.json`), so a future
   version bump is observed through the path the store actually uses. This alone
   would not help already-installed binaries — which is why (1) carries the
   ordering guarantee.

**Ordering** (all durable before the state they protect): legacy fence document →
removable v6 state write → first class-bearing removable record → any later
command's effects. Because the fence precedes the v6 state, an installed reader
that can ever meet v6 removable state has already been signalled at the path it
actually reads. Pre-#122 binaries still cannot be assumed pre-effect fenced; that
limit is unchanged and the planner must confirm the installed receipt covers #122
before any host install of this format.

**Proof (native case 15)** uses an **identified older artifact**: this
repository's own pre-feature commit `489eecc` (`STATE_VERSION = 5`, no
one-use fence) staged by role from its unchanged source and flags (A3-style, no
`proof-controls`). On the **real** control paths, with a private `HOME` and
control:

- **positive control**: with the legacy fence document present and the removable
  state at v6, the older artifact exits 75 (hand-off refusal) **before** any
  output effect — no receipt, no root, no link, no outputs fence — and its
  stderr names the re-run line;
- **negative control**: with the fence removed and the same v6 removable state,
  the older artifact's preflight observes nothing newer, performs its output
  effect and only then refuses at `Store::load`. This negative control is what
  proves the fence is load-bearing; it runs on a throwaway fixture pool and its
  effect is deleted with that fixture;
- **candidate control**: the v6 candidate itself neither hands off nor refuses
  because of the fence, reads its real removable state, and classifies normally;
  a pure stub reader reproduces the old peek's `6 > 5` decision without a second
  binary.
The internal path keeps its existing positive control (a v6 internal state with
no matching installed receipt → exit 75, no effect).

**Fence validation controls (same case, candidate build).** Because the fence
lives at the removable **pool root**, the candidate must accept it through the
scoped validator and nothing else: `oxide status`/measure (`snapshot`,
`Pool::open_passive`), a leased build (`Pool::open`) and `oxide describe`
(`describe_volume`) each succeed with a valid fence present, and each **refuses**
the pool with the shipped `unexpected_entry` for a fence that is symlinked,
group- or other-readable, oversized, unparseable, of an unknown fence name, of a
non-numeric version, or carrying another pool's `store_id` — and for any other
foreign entry at that root. No case relies on the fence being ignored as an
arbitrary file.

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
enables it) gates exactly two readers, both in the front door and both
`#[cfg(feature = "proof-controls")]`. The guarantee is about the **feature**, not
the profile: an artifact built without it contains no reader at all, and an
artifact built with it reports `proof_controls = true` and is refused by the
installer before any replacement.

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

- No artifact built without `proof-controls` contains a control reader, and no
  feature-off build reads either variable. A feature-enabled artifact (debug or
  release) does contain them and reports `true`, which is exactly why the
  installer refuses `true`, missing and mistyped values before replacement.
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
   with the exact guarded diagnostic before any barrier, fence, reservation
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
12. **The reserve/prune evidence is split, and its limits are stated.** The
    native prune case runs on the **internal directory-budget pool**, where the
    fixture drives a real budget (`OXIDE_POOL_CAP`) and asserts one-use `Only`
    pruning with no normal victim (case 8). The **removable** side is asserted for
    its *shipped refusal* only, and the removable **reserve-prune protection is
    not claimed natively**: a removable admission carries no reserve today
    (`store.rs:3277-3280` reads the registration from the removable store's
    control while it lives in the internal control), so
    `unknown_removable_step` refuses with the incomplete scan and the prune branch
    never runs. That branch's class-scoped policy and step are covered by pure
    checks and unit regressions with **injected registration and size facts**,
    plus the existing removable proofs — never presented as native evidence, and
    never a substitute for the brief's reserve requirement, which stays with the
    owner.

13. **A retry is not a second attempt at a victim.** After a failure-stop or a
    protected-slice refusal the retried admission performs the whole class-scoped
    selection again, with the premises it needs — a fresh member and real byte
    pressure (the effective cap, which for a removable pool is the **recorded
    registration cap** and not `OXIDE_POOL_CAP`; `store.rs:1534-1553,3270-3280`,
    `pool.rs:925-936`) — and it never resumes at the next candidate or converts
    an earlier failure into a normal victim (§16.2 cases 26, 27).
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
| normal `Prefer` fallback with a protected/unverified one-use pair ranked first | the pair is **excluded** from the fallback set; an eligible normal slot or idle class-unknown orphan is selected, or the ordinary refusal fires |
| version-5-or-below state carrying `class`/`class_reason` | **refuses** (a document that skipped its fence) |
| internal directory-budget pool over its fixture-driven budget, one-use admission | prune eligible idle one-use members in rank order, never a normal slot; refuse on the ordinary reason when only protected members remain |
| removable unknown-figure admission (no reserve is carried today) | refuses with the shipped incomplete scan and evicts nothing; the removable reserve-prune protection is **not** claimed natively (unit-level injected-fact coverage only) and the brief's requirement is returned to the owner (§10.2, §16.1, §19) |
| any admission whose retry needs byte pressure on a removable pool | the effective cap is the **recorded registration cap**; `OXIDE_POOL_CAP` does not override an existing removable pool |
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
| valid legacy fence at the removable pool root | the candidate validates and ignores it; `Pool::open`, `Pool::open_passive`, `snapshot` and `describe_volume` all succeed |
| invalid or foreign fence at the removable pool root | shipped `unexpected_entry` refusal, exactly as any other foreign pool entry; the validator never repairs, ignores or waits |
| seed headroom **known** and unfit | seed skipped, cold build in the same slot |
| seed headroom **unknown** (unreadable member, ownership/device/boundary fault, partial walk) | **refuses before any clone** with the shipped error class; never a cold build |
| guarded `detach`/`cancel-reservation`, `Prepared` at the binding-mutex revalidation | exact diagnostic with **no mark, no id, no `waiters/` file**; `next_id` unchanged across repeated refusals |
| guarded `detach`/`cancel-reservation`, barrier busy past the attempt budget | bounded "barrier busy; retry" refusal, no mark |
| guarded `detach`/`cancel-reservation` barrier, all busy facts **live** | wait for the live holder like today, writing nothing |
| guarded `detach`/`cancel-reservation` barrier, any busy fact **dead** (active record with a free/missing record lock; clean or ticket with a free/missing waiter lock and no live group) | **bounded non-progress refusal**, no wait, no mutation; the dead lease/mark is byte-identical and an ordinary command recovers it |
| guarded `detach`/`cancel-reservation` barrier, **unknown** custody (probe/read error, unreadable state or sibling) | **refuses** (unknown stayed protected) |

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
  `clean` refusal text, plan/status wording, the forward fence and version 6, and
  the two §4.1 seams: the `begin_clean` **mode** (`Recovering` for every existing
  caller; `Barrier` for guarded verbs, with its **live/dead/unknown busy
  classifier and bounded non-progress refusal**) and the **revalidation order**
  that decides the `Prepared` refusal under the checkout binding mutex *before*
  any mark, id or `waiters/` file exists, with a bounded non-blocking barrier
  attempt afterwards. Regressions cover a dead record, a dead clean mark, a live
  holder, unknown custody, the busy-barrier refusal, and **verb-level**
  repeated refusals (byte-identical including `next_id`); the command-level case
  uses a matched-control delta, since a build's ticket/lease lifecycle advances
  `next_id` independently of the output path, and an exact post-lease baseline
  would need the proposed `leased:<gate>` pause point (returned for scoped
  review).
- **S2 record class and front-door scoping.** `Record.class`/`class_reason` with
  the omission rule (§5.1), the bounded **presence-aware decode** in
  `Store::load` that refuses a sub-6 document carrying either field, the fence,
  project-cap and sharing class filters, membership and class-unknown derivation,
  status/summary visibility.
- **S3 library admission protocol.** `LeaseRequest` additions, the `MemberSet`
  and `evictable` split with the in-custody custody reader (§5.5.2), `restore`
  with its two thresholds and reuse-candidate exclusion, the union walk, the
  class-scoped pressure paths, the `one_use_budget` and `unknown_accounting`
  refusals, and the class-scoped policy applied to the reserve route's victim
  choice (`admit_unknown_removable` passes the admission's `Prefer`/`Only` to the
  same selector). The route's own behaviour is **not** changed: its reserve
  branch is unreachable today and wiring it is returned to the owner (§10.2,
  §19).
- **S4 proof controls.** The feature, the two readers, the capability field and
  the installer refusal, the `stage_at` feature parameter and the static edits
  to the exposed scripts.
- **S5 seeding interaction.** Binds S3's headroom rule to #137's actual
  interface, including the split between a **known-unfit** estimate (skip cold,
  the brief permits) and an **unknown** measurement (refuse, the frozen
  contract's rule; §8.3). **Blocked on oxide#137's implementation**, and the counted native
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
  with version 5 refuses; a version-5-or-below document carrying `class` or
  `class_reason` refuses; bounded reason and membership sizes.
- The serializer omission rule of §5.1: a fresh normal record and a rewritten
  normal-only state serialise with **neither** `class` nor `class_reason`, a
  `one_use` record carries both the class and the v6 fence, and a round trip
  through the v5-shaped `Record` succeeds for the normal bytes.
- The **presence-aware decode** of §5.1 over six inputs: v5 absent (accept),
  v5 `"class":"normal"` (refuse), v5 `"class_reason":null` (refuse), v5
  `"class":"one_use"` (refuse), v6 with fence and class (accept), v6 without
  fence (refuse); plus a bounded-input assertion (the pass runs once, over the
  same ≤ `MAX_STATE_BYTES` bytes, with no re-serialisation).
- The seed headroom split of §8.3: a complete measurement that does not fit
  yields the cold skip; an unreadable member root, an ownership/device failure
  or a partial walk yields the shipped refusal **before** any clone, and the two
  are distinguishable in the outcome.
- The `begin_clean` mode: `Recovering` still drops dead tickets and recovers a
  dead lease (`store.rs:3676-3693`); `Barrier` performs neither, writes nothing
  while the checkout is busy, and classifies each busy fact as live (held waiter
  lock, or a recorded non-empty group), dead (free or missing waiter/record lock
  with no live group) or unknown (probe/read error). Pure cases assert: no busy
  fact → one mark; all live → wait; **any dead → the bounded non-progress
  refusal** with no mutation; any unknown → refusal. `Recovering` keeps today's
  behaviour for every existing caller.
- The **order** of §4.1: a `Prepared` receipt observed at the binding-mutex
  revalidation yields the exact diagnostic, and **the refusal step itself** adds
  no `next_id`, no `waiters/` entry, no receipt/link/marker/root/reservation
  change (the mark and its id are allocated only after a passing revalidation);
  the busy barrier is bounded by the attempt/deadline budget and refuses without
  a mark; a `Prepared` appearing at the effect read is a custody defect that
  refuses rather than replaying; and the matched-control delta separates a
  build's ordinary ticket/lease delta from any output-transition delta.
- The fallback exclusion of §6.7: with a protected (or unverified) one-use pair
  ranked before an eligible normal slot, an idle class-unknown orphan and a
  second evictable member, `Prefer` never returns the protected pair; the
  orphan and the normal slot remain eligible; an empty fallback set yields the
  shipped refusal.
- The request/policy shapes: `Any` is the default, a member set is bounded, a
  policy with an empty set is a no-op, `Only` with no eligible victim fails,
  and the two slice thresholds (new slot needs a free place; warm reuse at the
  cap is admissible).
- The restore decision table: 16 idle members and a new slot reclaims the oldest
  then admits; a warm reuse at 16 admits without reclaiming; a protected-only
  slice is `Full`; a class-unknown row yields `LowerBound`, never `NotNeeded`.
- The reserve route's policy and step, **unit-level only**: a source-level check
  that `admit_unknown_removable` passes the admission's `Prefer`/`Only` to the
  same class-scoped victim selector, plus unit regressions that drive
  `unknown_removable_step` with **injected registration and size facts** (a
  modelled reserve, a modelled free-space reading and an injected member set) for
  the above-reserve, at/below-reserve, floor and refusal branches. These are
  **not** native evidence: removable reserve-prune is not natively exercised by
  the fixed proof (§16.2 case 8, §19).
- The union walker: shared `(device,inode)` set across roots, duplicate root
  refused, link not followed, partial coverage reported, entry/deadline bound.
- The slice arithmetic: `min(budget/10, 32 GiB)`, floor of 1, 16 members,
  15 for a new slot.
- The record→slot mapping: stale incarnation, missing record, missing slot, a
  foreign project and an unreadable record all classify unknown.
- The legacy fence validator: a valid fence for this pool is accepted by the
  pool-root allowlist; a symlinked, group/other-readable, oversized,
  unparseable, unknown-fence, non-numeric-version or mismatched-`store_id`
  document and any other foreign entry each refuse with `unexpected_entry`; the
  fence is never read as state; and `peek_version` ignores a version equal to the
  build's own.
- Guarded `adopt` refuses at the effect-free precheck for **every** receipt state
  (only the text differs), and never reaches a barrier; `detach`,
  `cancel-reservation` and a command binding each revalidate under their custody
  — `detach`/`cancel-reservation` under the **checkout binding mutex before any
  clean mark or id**, the command binding inside its own mutex before
  `gated_replay` — and `adopt` never reaches custody at all.
- The custody premises of case 19: a receipt-bearing binding holds the checkout
  mutex across `bound`, a receipt-absent binding holds none, and a paused peer is
  an active record for the barrier.
- The victim predicate of §5.5.2 over each record phase, `process_group`,
  `retired`/`lost`, an active same-pool record for the checkout and each record
  lock probe state; a counterfactual that hands it a "cross-pool writer" changes
  nothing (the predicate never consults sibling state), and a stubbed
  `sibling_state()` asserts it is never called on the victim path.
- The capability JSON shape and the installer's `proof_controls` acceptance and
  refusal matrix (pure; the synthetic `dogfood:scripts:test` fixture covers the
  same decisions without an install), including a **feature-enabled release**
  artifact that must report `proof_controls = true` (proving the boolean tracks
  the feature, not the profile) and be refused by the installer, and a feature-off
  debug artifact that must report `false`.

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
7. normal **fallback-set** protection, byte pressure: one-use members present, a
   normal new slot under byte pressure reclaims an evictable one-use member
   first. Three non-vacuous variants:
   - a **protected** member (ledger-idle, record lock held by a live fixture
     process) ranked **before** an eligible one-use member and before an eligible
     normal slot: the protected member survives byte-identically, and the
     admission either reclaims the next evictable member or, with none left,
     selects the **safe fallback** (the normal slot) — never the protected
     member. This is the case the bare `evict_one_idle` predicate would get
     wrong.
   - with no evictable member left and an idle **class-unknown orphan** ranked
     before a normal slot, the fallback selects the **orphan** under the shipped
     #118 eligibility (the orphan is not a one-use pair, so it is not excluded).
   - with the fallback set empty (a protected member and a held normal slot), the
     admission **refuses on its ordinary reason** and the protected member is
     byte-identical; it never falls through.
   With every one-use member held (no fallback candidate), a normal admission
   still continues into the ordinary decision and reports a measured overage
   rather than stalling on the slice.
8. **budget-driven prune on the internal directory-budget pool** (the pool whose
   budget and watchdog the fixture can actually drive), plus the removable-path
   **refusal**:
   - *internal, fixture-driven byte pressure*: a private `directory-budget-v1`
     pool created with a small recorded cap (`OXIDE_POOL_CAP`), one idle normal
     slot and two idle one-use members. A **one-use** new-slot admission over the
     budget prunes eligible one-use members in rank order under `Only` and never a
     normal slot; with only protected members left it refuses on the ordinary
     reason and every normal slot is byte-identical. This is the native
     reserve-style prune evidence: the pressure is a real, fixture-driven budget,
     not an injected reading.
   - *removable, today's behaviour asserted*: on the owned image's registered and
     **selected** removable pool (positive selected-removable control) with one
     idle normal slot and one idle one-use member, a new-slot admission made to see
     a bounds-only pool figure (`OXIDE_TEST_WALK_MUTATIONS`,
     `DelayBeforeReadDir` on the payload root, 6,000 ms > the 5 s removable figure
     bound) **refuses with the shipped incomplete scan and evicts nothing**: no
     member is retired, the normal slot is byte-identical, and the message carries
     `elapsed_deadline`. This pins the removable path's shipped behaviour (no
     reserve is carried today, so no prune happens) and is the evidence that this
     design invents no cleanup.
   **Removable reserve-prune is deliberately not claimed natively.** The brief's
   removable reserve-prune *protection* is a product requirement that this proof
   does not exercise: today's removable admission reads the registration from the
   removable store's control (`store.rs:3277-3280`, `removable.rs:614-615`) while
   the registration lives in the internal control, so the unknown-size route
   refuses without a reserve (`pool.rs:2848-2854`) and the prune branch never
   runs. The class-scoped policy that branch would use is covered by **pure
   checks and unit regressions with injected registration and size facts**
   (`admit_unknown_removable` driven through the step function with a modelled
   reserve/free-space reading and an injected member set), together with the
   existing removable proofs; it is **not** native evidence and the design says
   so wherever it is cited (§16.1, §19). Resolving the brief's requirement or
   changing the wiring stays with the owner.

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
14. state class/version and the legacy serializer: a version-5 fixture state
    reads as normal; a version-6 document without the fence refuses; a
    version-5-or-below document that carries `class`/`class_reason` refuses; an
    unknown class refuses; the capability probe reports state version 6. **Legacy
    read control with the identified older artifact** (`489eecc`, state 5): the
    candidate performs an ordinary **normal** creation and rewrite, the old
    artifact reads and rewrites that state successfully **with no hand-off** (the
    document contains neither new field, proven by the pure serializer check),
    while the same artifact refuses a v6 one-use state before any output effect
    (case 15's positive control).
15. pre-effect hand-off on the **real** paths, with an identified older
    artifact: pre-feature `489eecc` (state 5) staged by role, no feature. Internal
    positive control: a v6 internal state and no matching installed receipt →
    exit 75 before any output, receipt, root or slot effect. Removable positive
    control: the legacy fence document at
    `<control>/removable/state.json` with the removable state at
    `<control>/removable/oxide-managed/state.json` at v6 → the older artifact
    exits 75 **before** its output effect. Removable negative control: the same
    v6 removable state with the fence removed → the older artifact performs its
    output effect and only then refuses at `Store::load` (the gap the fence
    closes). Candidate control: the v6 candidate ignores the fence, reads the
    real removable path and classifies normally; a pure stub reader reproduces
    the old `6 > 5` decision. The same case runs the §10.4 **fence-validation
    controls**: the candidate accepts a valid fence through `Pool::open`,
    `Pool::open_passive`, `snapshot` and `describe_volume`, and refuses each
    invalid or foreign one with the shipped `unexpected_entry`.

16. guarded outputs, absent/real: a temp workspace with no receipt, and with a
    real local `target`, builds locally; no receipt, root or link is created.
17. guarded outputs, valid `Detached`: runs locally, requires no historical
    volume, and a planted link at a relevant component refuses with nothing
    changed.
18. guarded outputs, `Committed` in two shapes: a profile pair of exact owned
    links, and a mixed receipt with one owned member and one real nonempty
    member. Both **refuse** the build; detach then restores real directories with
    the root bytes unchanged; the following build runs locally.
19. guarded `Prepared` and custody exclusion, with existing debug controls only and
    every premise reaching the custody it claims (the §4.6 table):
    - **(A) precheck control.** A peer runs `oxide outputs adopt` with
      `OXIDE_TEST_OUTPUTS_CRASH=prepared` (second variant `=migration-p0`),
      leaving a real `Prepared` receipt / migration journal from the product's own
      write-ahead path. Every guarded verb — `outputs adopt`, `outputs detach`,
      `outputs cancel-reservation`, a normal command and `outputs clean` —
      refuses with the exact diagnostic, and the bounded digest of the receipt,
      link, sibling, marker, reservation and root is identical before and after.
      The case states that this part exercises the **fast precheck**, not the
      barrier.
    - **(B1) binding mutex, positively observed twice.** The checkout holds a
      valid **`Detached`** receipt, so a guarded command is allowed to proceed
      (§4.2). A peer command in that checkout is paused at
      `OXIDE_TEST_PAUSE=bound:<gate>`: a receipt-bearing binding takes
      `store.lock_binding` (`outputs.rs:4214-4235`) and keeps it until
      `mark_running` (`main.rs:701`), so the peer holds the mutex at `bound`
      (`main.rs:545`), observed by its own `bound` marker. The guarded command
      then stays alive and writes nothing; when the gate opens, the **guarded
      command's own `bound` marker** appears, proving it acquired the mutex and
      passed the pause, and its final outcome matches the state. The
      **receipt-absent** case is recorded as a non-premise
      (`OutputBinding::unchecked`, no mutex) and asserts no exclusion.
    - **(B2) barrier, live holder.** The checkout holds a **`Committed`** receipt
      (guarded `detach` and `cancel-reservation` are allowed). A live peer paused
      at `bound` is an active same-checkout record, so the guarded verb enters
      `begin_clean(Barrier)`, finds only **live** facts and waits. Positive
      observations: the verb has not exited and wrote nothing while the gate is
      closed, and after release it completes with the shipped detach output.
    - **(B3) barrier, dead facts → bounded refusal.** With a **`Committed`**
      receipt (so the precheck passes) and (i) a same-checkout **dead record**
      (free record lock over a held lease), (ii) a same-checkout **dead clean
      mark** (free waiter lock, no live group), or (iii) an unreadable state or
      sibling state, the guarded `detach`/`cancel-reservation` refuses with the
      **non-progress** diagnostic inside the case's wall clock, without mutating:
      the record, lease, recovery log and clean mark are byte-identical and the
      dead lease is still present. After an ordinary command recovers the dead
      lease (or the dead mark clears), the same guarded verb proceeds. Unknown
      custody is the (iii) refusal.
    - **(E) in-custody revalidation — ORACLE DEFERRED (decision `dd090b31`,
      review `6454799468`).** The construction below is *design input only*: it is
      **not** a proved oracle and is **not** acceptance evidence. The oracle is
      deferred to the implementation proof brief per decision `dd090b31`, and that
      brief must satisfy **all five** acceptance criteria at the end of this item
      before any counted native may claim it. Nothing in this case asserts that
      those criteria are met.
      *Design input:* the boundary is
      the **existing** debug-only `OXIDE_TEST_PAUSE=keyed:<gate>` point
      (`main.rs:475`, after the key and before `lease_with_gate`), restricted to
      the shared supervisor scratch (`<gate>` is a path under
      `OXIDE_TASK_SCRATCH_ROOT`; the point is `#[cfg(debug_assertions)]` only, so
      the build restriction is a **debug** candidate, and a release artifact
      ignores the variable). Subject and control are the *same* guarded command in
      the *same* checkout with a **`Detached`** receipt, run through the *same*
      fixture transitions and the *same* lifecycle through that boundary:
      1. the command starts, passes the fast precheck (`Detached`), and pauses at
         `keyed`; its `keyed` marker is the **positive proof that the precheck
         passed**;
      2. **baseline is taken at the gate**, after any fixture action, so nothing
         the fixture does is inside the measured window;
      3. the gate opens and the run continues to its binding decision.
      The **single differing variable** is the receipt/journal state at that
      baseline: the **subject** has a `Prepared` receipt (the fixture publishes it
      with the unguarded `crash_point` invocation *while the command is paused*,
      so the plant and its own id are **before the baseline** and excluded), the
      **control** still has the `Detached` receipt. Nothing else differs; both
      runs use the same gate, the same fixture, the same command and the same
      environment.
      **Proposed oracle sketch (field-scoped delta; deferred — see the header and
      the acceptance criteria).** It is not an executable oracle yet. The
      compared set is the *output-transition* field set only:
      `outputs/*.json` receipt bytes, every `target`/member link and its
      readlink, every sibling link, every root marker, every reservation row,
      the `outputs registry` root entries, the `cleans` array in `state.json`,
      the `waiters/` directory listing, and the numeric `next_id`. For each run
      the delta is `state(after exit) − state(at baseline)` over exactly that
      set. The observation window **ends when the command exits**, so the
      successful control's later effects are excluded by construction: the
      record phase, `next_use`, the origin and the checkout's local `target/`
      contents are **not in the compared set** (they are the lease/build
      lifecycle, and the control legitimately advances them at `mark_spawning`,
      `main.rs:545,649-652`); `origins/` and the `tickets` array are likewise
      lifecycle files (both runs register and release exactly one ticket and may
      write one origin, so they cancel by construction and are excluded). The
      **`waiters/` directory is included** precisely because a clean mark's own
      lock file would appear there even though the mark's row lives in `cleans`.
      **Deferred expectation (design input only — not an oracle).** The sketch
      *expects* the subject's delta to equal the control's on that set with no
      clean mark, no `waiter` entry and no receipt/link/marker/root/reservation
      change, and it *illustrates* the forbidden effect as `cleans` +1 plus a
      `next_id` step. Those illustrative figures are **not** asserted: the
      command-level oracle is **oracle deferred to the implementation proof brief
      per decision `dd090b31`**, and per-allocation accounting per acceptance
      criteria 1–2 replaces any fixed increment. The illustration is retained
      only as design input and is not acceptance evidence.
      The refusal's **text** is asserted exactly, and the deferred brief must
      establish that the refusal is the revalidation rather than the precheck
      using the `keyed` marker plus the fact that the precheck had already
      accepted a `Detached` receipt; this design only names that premise.
      **Premise-6 entry.** Proof-authoring premise 6 (owned images,
      `hdiutil attach -owners on`) **does not apply to case E**: no image is
      mounted in this part; it is recorded as such, and it does apply to case 8's
      removable sub-case and case 21, which state their `-owners on` and
      read-only mount observation.
      **No new seam is proposed for E.** The earlier `leased:<gate>` idea is
      withdrawn: with the baseline at the `keyed` gate and the field-scoped
      delta, the oracle is exact without it. The **verb-level** claims stay direct
      and non-vacuous: a guarded `detach` or `cancel-reservation` refusal
      allocates no clean mark and no id, and repeated such refusals are
      byte-identical including `next_id` (asserted purely and natively where the
      receipt is planted before the verb starts).
      **Acceptance criteria for the deferred oracle (all five required; review
      `6454799468`, decision `dd090b31`):**
      1. **Reusable-record premise, positively verified in both runs.** The case
         must establish and check the *same* existing matching idle record (and
         any other stable relevant state) in subject and control, so the
         legitimate id allocations are known: one ticket id (`store.rs:1759`,
         `3639`) plus, only when no reusable record exists, one record id
         (`store.rs:3178-3184`). A correct refusal on a fresh record must not fail
         the oracle or be misread as a clean mark.
      2. **Per-allocation accounting, not a fixed `+1`.** The expected `next_id`
         increment must be derived from the legitimate allocations observed in
         that run and must *discriminate* the forbidden clean mark; the case may
         not assert a constant increment.
      3. **One consistent, reachable boundary and explicit matched setup.** The
         §4.1 sketch (same pre-run baseline, optional `leased` seam) and this
         case's gate baseline must be reconciled into a single boundary, with an
         **executable** subject/control setup — the plant runs only in the
         subject, so "identical fixture transitions" must be built, not asserted.
      4. **A non-vacuous observation of the transient.** Endpoint snapshots at the
         gate and after exit cannot observe a clean mark and waiter lock that the
         shipped barrier creates and later removes (`store.rs:2738-2748`,
         `2805-2810`, `outputs.rs:6151-6167`); the deferred oracle must observe
         the transient while it would exist (or prove its absence by an equivalent
         in-run observation) and must exclude the plant's own transitions.
      5. **Exclusion of the successful control's lifecycle.** The comparison must
         exclude the control's post-boundary `mark_spawning`/Cargo effects by
         construction, together with the plant, so the alleged guarded-refusal
         delta contains only the refusal's own effects.
    - **(C) commit, explicitly unguarded.** The prepared transition is committed
      by a deliberately **unguarded** invocation of the same build with
      `OXIDE_TEST_TEMP_ROOTS=[]` and `OXIDE_TEST_WORKSPACE_CLASS=normal` (feature
      build only, `guarded = false`), which runs the shipped `oxide outputs
      adopt` and replays it; the guarded invocation afterwards sees `Committed`
      and refuses per §4.2. The production route is the same idea without
      controls: the checkout is no longer a temporary workspace.
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
    enter the next member union, and a **known-unfit** headroom skips to a cold
    build in the same slot. Non-vacuously separated from it: an **unreadable**
    member root (a planted unreadable directory inside an idle member) during the
    fresh pre-clone headroom walk **refuses** the command with the shipped
    `unknown_accounting`/ownership error before any clone, the recipient slot
    stays empty and untrusted-free, and nothing is seeded; the same fixture with
    the directory readable takes the known-unfit or fit path. This pins the
    predecessor's unknown-is-not-a-miss rule.
25. victim custody in both directions (§5.5.1, §5.5.2):
    - *same pool, ledger-idle / control-uncertain*: a fixture plants a one-use
      slot whose ledger row is `Idle` while its control record is still active and
      its **record lock is held** by a live fixture process (a real "a command may
      be writing" state: `recover_settled` skips it, `settle` would `Keep`). It is
      reported as a **protected** member; a one-use admission that needs its bytes
      refuses (no wait: no progress-capable holder), the slot's root and ledger row
      are byte-identical, and a normal admission continues with it counted.
      Releasing the lock and letting the next command's `recover_settled` settle
      the record makes it evictable, and the following one-use admission reclaims
      it.
    - *same pool, phase interleaving*: a member whose owning record moves
      `Idle → Acquiring → Spawning` while the admission runs (the peer is paused at
      `bound`) is **protected** for every non-idle phase, the admission refuses or
      waits for that progress-capable holder, and the slot is byte-identical. The
      same assertions run as a pure check over the predicate for each phase.
    - *cross-pool interleaving is inert*: a **removable** command of the same
      checkout moves `Acquiring → Spawning` while an **internal** admission with a
      one-use member runs. The internal admission's outcome is identical to a
      control run with no peer (same victim, same bytes, or the same refusal), the
      removable command's record and slot are untouched, and a pure check asserts
      the predicate reads **no sibling state** and takes no sibling lock.
      Together with the pure "no recursive acquisition" check (no admission file
      opened, nothing created, no `flock` attempted), this is the interleaving
      regression §5.5.2 requires — the cross-pool fact is excluded because it
      cannot be stable under an internal admission.
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
  `bounded-test.py --root-parent /private/tmp --limit 480 --scratch-budget-mib
  512 -- python3 scripts/test-one-use-workspaces.py`. The `--limit 480` wall
  clock and the 512 MiB monitored scratch budget are **enforced** by the guard
  (a breach fails the run); the per-phase estimates in §16.4 are observations. The explicit internal root
  is the owner-accepted internal-placement exception (decision `bd357a4c`)
  because a private `directory-budget-v1` pool is a genuine internal pool that
  the product correctly refuses on an external volume; it stays inside one
  monitored 512 MiB owned root.
- **The bound is provisional and unmeasured — 480 s / 512 MiB is not an approved
  limit.** The image attach/detach cycle, the 128-slot ceiling fixture, the 6 s
  removable walk delay and the **third staging** (the fixed older artifact
  `489eecc`, state 5, built from its own source and flags with no shared target)
  are why the arithmetic is above the 300 s default (§16.4: ~296 s typical,
  ~370 s upper). The implementation brief fixes the bound **after** the fixed
  script's first separately activated uncounted diagnostic measures actual fit;
  any final bound above **300 s requires a separate operator ruling from Tom**,
  and none is granted by this design or by any review. Until then this document
  states an estimate, never an approved clock, and makes no claim that 300 s
  fits.
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
| Stage the **older artifact** for the hand-off case (fixed `489eecc`, state 5, no feature) by role | ~35 s | separate target, its own source and flags (A3); no dependency reuse across commits |
| 27 tiny fixture cases, including the 128-slot hand-made ceiling pool | ~124 s | ~4–6 s per case, ~10 s for the ceiling pool, ~6 s for the removable unknown-figure negative case (one 6 s walk delay) and ~6 s for the failure-stop/retry cases; the older-artifact hand-off case adds a fixed pre-feature staging (~37 s, included in the staging row) |
| Image: attach (`-owners on`), registration, selected control, ordinary detach | ~18 s | two attach/detach cycles at ~9 s each |
| Fixture creation, member walks, digests, receipt | ~25 s | bounded metadata work on tiny trees |
| Cleanup | ~20 s | inside the owned root, with the image custody record |
| **Total** | **~296 s typical, ~370 s upper** (provisional, UNMEASURED) | arithmetic only; a work cutoff near 430 s is a **proposal** pending the diagnostic and Tom's ruling above 300 s |

Disk, inside the 512 MiB monitored root: one owned 128 MiB APFS sparse image
(no free-space filler is needed); three staged targets — the feature and
feature-disabled builds share one (~60 MiB) and the fixed older artifact uses its
own (~40 MiB); the 128-slot ceiling pool with one tiny file per slot plus ≤ 24
medium tiny private slots ≈ 50 MiB; fixtures, logs and the bounded receipt
≈ 20 MiB. Total ≈ **298 MiB of 512**.

**Two of these are enforced gates, not observations.** The supervisor's
wall-clock limit (`--limit`) and its monitored scratch budget
(`--scratch-budget-mib 512`) are limits the qualified guard **enforces**: a run
that exceeds either fails (a deadline breach stops work; a monitored-bytes
breach is a failure), and that is the brief's and the validation policy's
time-and-budget gate. The per-phase figures above, the staged-artifact byte
counts, Cargo-call counts and monitor timings are **observations**, reported by
the receipt and never acceptance gates. The 480 s / 512 MiB pair is
**provisional and unmeasured**: the fixed script's first separately activated
uncounted diagnostic measures fit, the implementation brief fixes the final
bound, and any final bound above **300 s needs Tom's separate ruling** (none is
granted here). If the measured work cannot fit, the bound is revised before a
counted activation — never during one.

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
4. Is the race-safe protocol of §4.1/§4.6 sound: `adopt` decided entirely at the
   effect-free precheck because it always refuses (state selecting only the
   text); `detach`/`cancel-reservation` revalidating under the **checkout binding
   mutex before any mark** (so the `Prepared` refusal writes nothing, including
   `next_id`), then acquiring the barrier with **bounded non-blocking attempts**;
   and a command binding revalidating under its mutex — with custody mutual
   exclusion for every writer, a stop condition, and the id/lock lifecycle
   stated? Is the guarded `clean` refusal text (retained refusal, no adopt
   advice) acceptable?
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
   unknown-accounting and retry paths: does the normal `Prefer` fallback now
   remove **every** verified one-use pair (protected, unverified and evictable)
   while keeping normal slots and idle class-unknown orphans eligible under
   #118, with the shipped refusal when that set is empty? Are cases 5, 7 and 8's
   fixtures feasible, safe and non-vacuous as written?
10. Is N+1 = 6 with a durable one-use fence the right numeric fence, and is the
    installer's `proof_controls` refusal correct for older receipts?
11. Is the staging inventory (§11.2) complete now that helper-mediated stagers
    are explicit and `stage_at` takes a per-call feature, including the
    suspended `test-oxide-cli.py` gap and every capability assertion?
12. Is the **provisional** 480 s / 512 MiB estimate (~296 s typical, ~370 s
    upper, ~298 MiB) a sensible starting point for the 27 cases, understanding
    that the implementation brief fixes the bound after the fixed script's first
    separately activated uncounted diagnostic measures fit and that any final
    bound above 300 s needs Tom's separate ruling?
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
16. Is the two-direction custody protocol (§5.5.2) right: the victim predicate
    uses only the owning pool's admission-guarded facts (phase, holder/lease/
    recovery/retired/lost, `process_group`, an active same-pool record for the
    checkout, the non-blocking record-lock probe) and **no** cross-pool fact,
    because the internal store reads the removable sibling without its lock
    (`store.rs:2509-2525`, `2652-2654`)? Are the three regressions
    (same-pool phase protection, cross-pool inertness, no recursive acquisition)
    sufficient and non-vacuous?
17. Is the reserve disposition right: the prune branch is unreachable today
    (no reserve is carried; `store.rs:3277-3280` versus `removable.rs:28,298`),
    wiring it would be new deletion behaviour the brief forbids inventing, so the
    design leaves it as shipped, asserts the shipped refusal natively (case 8),
    specifes the class-scoped policy purely, and **returns the reserve-path
    protection requirement and the wiring change to the owner** for a scoped
    ruling (§19)? Is that the correct Queue disposition rather than a design
    claim?
18. Are case 19's premises now reachable **and positively observed**: a
    `Detached` receipt so the guarded command passes the precheck, a peer at
    `bound` holding the binding mutex (observed by its marker, then by the guarded
    command's own marker), a `Committed` receipt so a guarded `detach` enters the
    live-holder barrier wait, and a `Committed` receipt plus a dead record/dead
    clean/unknown state so the guarded verb reaches the **bounded non-progress
    refusal**? Is the `Barrier` classifier (live/dead/unknown from the shipped
    lock-and-group predicates, no mutation) the right finite protocol, and if the
    freeze rejects the `begin_clean` mode seam, is returning that conflict to the
    owner the intended disposition?
19. Is the scoped fence validator (§10.4) the right shape — one allowlisted name,
    strict ownership/schema/identity validation, the same `unexpected_entry`
    failure-stop for anything else, never read as state, with candidate controls
    on `Pool::open`, `Pool::open_passive`, `snapshot` and `describe_volume`?
20. Is the seed-headroom split of §8.3 the right reading of the frozen
    contract: a complete measured shortfall skips cold, while an unreadable
    member root or an ownership/device/partial-walk fault refuses before any
    clone? Is case 24's non-vacuous pair (unreadable refuses; readable takes the
    known path) sufficient evidence?
21. Is the presence-aware decode of §5.1 the right mechanism — one bounded
    `serde_json::Value`/raw pass before the typed parse, refusing a sub-6
    document that carries `class` or `class_reason` in any value — and are the
    six regression inputs (including both explicit default values) the right
    set?
22. Are the HIGH complexity, premise checklist and separately activated
    uncounted diagnostic correctly placed before the first counted native?

## 19. Limits of this design

- No implementation, native, install, consumer, host or live-pool authority is
  granted; the merge hold and both counted natives remain.
- The time and disk figures are source-derived arithmetic, not measurements; the
  fit is UNMEASURED until the one activation per side runs; the 480 s / 512 MiB
  pair is **provisional**, the implementation brief fixes the final bound after
  the fixed script's first separately activated uncounted diagnostic measures it,
  and any final bound above **300 s needs Tom's separate ruling**.
- The seeding interaction (§8) is a requirement on a frozen but unimplemented
  interface; case 24 is mandatory, so the counted native waits for oxide#137's
  implementation or moves into a scope split.
- The removable and guarded-state sub-cases depend on three **existing** debug
  controls (`OXIDE_TEST_WALK_MUTATIONS`, `OXIDE_TEST_PAUSE`,
  `OXIDE_TEST_OUTPUTS_CRASH`) staying available and unchanged; if any is removed
  or narrowed, those cases return to the owner for a scoped ruling rather than
  being dropped or reported unexecuted.
- **Reserve-prune evidence is split, and the removable half is not native.**
  The **native** prune case is the internal directory-budget pool with a
  fixture-driven budget (case 8a). The **removable reserve-prune protection is
  not natively exercised** by this proof: a removable admission carries no
  reserve today (`store.rs:3277-3280` reads the registration from the removable
  store's control while it lives in the internal control), so
  `unknown_removable_step` refuses with the incomplete scan and the prune branch
  never runs (case 8b asserts exactly that shipped refusal). That branch's
  class-scoped policy and step are covered by **pure checks and unit regressions
  with injected registration and size facts**, together with the existing
  removable proofs. **Returned to the owner (not a design claim):** the brief's
  reserve-path protection requirement is not discharged by any of that — neither
  the shipped-refusal sub-case nor the injected unit facts are native evidence —
  and reaching it would add a deletion behaviour the brief forbids inventing.
  Only an owner ruling or a revised brief can change how it is discharged
  (§10.2, §16.2 case 8, §16.1). Everything else the brief lists remains in the
  fixed case list. The concurrency case claims custody **exclusion**, never an
  interleaved observation.
- **Two small seams are specified and need their own review.** (1)
  `begin_clean` gains a mode so the guarded barrier performs no ticket drop and
  no recovery, **plus** a non-mutating live/dead/unknown classifier that turns a
  dead record or dead clean mark into a bounded refusal instead of an unbounded
  wait, and the guarded verbs **revalidate before any mark exists** so a
  `Prepared` refusal allocates no id; every existing caller keeps `Recovering`.
  A native interleaving of that revalidation for `detach`/`cancel-reservation`
  would need one new pause point inside their binding section and is returned for
  scoped review. (2)
  `Store::load` gains one bounded presence-aware decode before the typed parse.
  Both are safety-bearing changes outside the pure guard logic; if the freeze
  rejects either, the affected claim returns to the owner rather than being
  weakened silently. Unknown seed accounting is **not** a seam: it keeps the
  frozen contract's refusal.
- **Cross-pool custody is deliberately not a victim premise.** The internal store
  reads the removable sibling's state without its lock, so no internal admission
  can establish a stable cross-pool fact without acquiring it in the shipped
  internal-first order; the design therefore protects on same-pool facts only and
  states the stop condition for any future cross-pool decision (§5.5.2).
- **The legacy fence needs its own review.** It relaxes one refusal in the
  library's pool-root allowlist (a validated `state.json` fence) and writes a new
  document into the removable pool root; the design specifies strict validation
  and failure-stop, but that allowlist change is safety-bearing and must be
  reviewed on its own, with the §10.4 controls. No ledger field, `capacity_policy`
  number or receipt/marker format changes.
- No arbitrary-write containment, no byte-saving claim, no free-space guarantee
  and no claim that a one-use slot's warm build is as fast as a normal one.
- Platform scope follows the existing proofs: macOS fixtures with the pool's
  own volume rules (including one owned 128 MiB scratch image); Linux compile
  coverage only.
