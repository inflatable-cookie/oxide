# Barge adoption: Oxide's output roots and the release prune

**Status (2026-11-01): design frozen; `O1` landed (oxide#219).** Tom's F6
decisions (`8058e38a`, with Q-A to Q-H answered in `12729764` and Q-D in
`55636dc8`) freeze this design. Oxide's library pin and offline seeding are in
place under [D1](#d1-integration-surface-rust-library-at-a-pinned-commit) and the
[landed O1 note](#d1-integration-surface-rust-library-at-a-pinned-commit). Later
slices (`O2`, `O3`, `O4`, `S8-*`) are not landed. No output placement, prune,
admission, host job, install or Builds registration has changed.

The text below is the pre-freeze proposal. Its "proposed design" and "changes
no code" statements describe the 2026-11-01 proposal only and are kept as dated
evidence. Where a later slice lands, its own note in this file is the current
truth. Until `O3` lands, `oxide outputs prune` stays the authority for Oxide's
output roots.

The contract decides how Oxide uses Barge, which Oxide areas Barge declares,
how the prune switches to Barge's release gate, what happens to the 91 legacy
roots, and how the space guard and fallback fit. It then splits the work into
finite slices with per-slice validation, operator gates and rollback.

The Barge side is owned by Barge's own contracts (`stage1-design.md`,
`stage3-space-policy.md`, `stage4-design.md`, `host-job.md`,
`t9-real-drive-qualification.md`, `consumption.md`). Where this file and those
disagree, they win and this file is corrected. Oxide's current behaviour is
owned by [workflow](workflow.md#pruning-owned-output-roots-oxide129) and
[local lease](local-lease.md). The authority boundary is in
[vision](../vision.md#storage) and [architecture](../architecture.md#storage-invariants).

## Sources and pins

- Barge `main` at `ce1aef1`: the contracts above, and the code under
  `crates/barge` and `crates/barge-cli`. Code claims below name the file and
  line at that commit.
- Oxide planning checkout at `caf44ac`. Oxide product source at `56a57ba`, read
  for file and line references (`src/bin/oxide/prune.rs`,
  `src/bin/oxide/removable.rs`).
- Read-only observations, not re-run and not changed:
  - the T9b packet's `report.txt` (`/Volumes/Builds/scratch/barge/t9-preview-eu8ya3b2`);
  - the scratch reaper's latest Oxide prune preview (2026-11-01 04:55);
  - the 2026-10-26 live prune preview under Oxide's release-audit folder.
- Rulings the Barge contracts cite: `88cc123e` (design freezes go to the Oxide
  Supervisor), `ff13f2ef` (the 91 legacy roots stay held), `765f84c6` (identity
  moved to Barge), `d9a9cd37` (explicit identify only), and the T9 and host-job
  go decisions `40afbe99` and `e0d05447`. The F6 start is Tom's 2026-11-01
  ruling `df20a860`.

## What is landed

Checked against the code, not only the contracts.

| Capability | Status | Where |
| --- | --- | --- |
| Root identity record, verification, `area root identify` | Landed (F1) | `identity.rs:420` |
| Volume locator, absence primitive, ancestor proof | Landed (F2) | `release.rs`, `evidence.rs` |
| Release gate plan, `barge.prune.plan/1`, reference readers | Landed (F3) | `release.rs:338`; `reference.rs:2055` |
| Intent evidence and replay document surface | Landed as evidence only (F4) | `state.rs`; the effect verb is absent |
| Oxide registry census and drift report | Landed (F5) | `census.rs:290`; `barge inspect` |
| Space prune, guard, recheck, `maintain --space` | Landed (T5–T7) | `space/guard.rs:404,451`; `main.rs:1155` |
| Area declaration | Landed; a re-declare bumps `generation` | `area.rs:2622`; no undeclare verb |
| Volume register and update (including `--root-dir`) | Landed | `volume.rs`; `main.rs` |
| Builds registration | Registered `root_dir=barge`, durability `kept` (T9b withdrawal) | T9b `report.txt` |

`barge prune` without `--plan` refuses with "the real release run, its intent
evidence and its replay are stage 4 F4; nothing was planned"
(`crates/barge-cli/src/main.rs:1385`). That message is accurate and it is the
main finding below.

## Findings that block F6 as written

These are facts the decisions depend on. Items G1–G4 are gaps in the Barge
contracts. The Barge delegate (the Oxide Supervisor) must rule on them before
the matching Barge slice is queued. The rest of this design holds either way.

- **G1 — No release effect is planned.** `stage4-design.md` says F4 "did not gain
  an effect verb", and the slice table (`F1`–`F6`) has no slice that adds one.
  F6 says Oxide "switches its prune to Barge's gate". That needs a deletion
  path that is not designed. This contract adds it as slice `R1`.
- **G2 — `maintain` would act on any declared age policy.** `build_plan` takes
  each declared area's policy and skips only `None` (`prune.rs:997`);
  `prune_age_seconds` maps `OldestIdleAfter` to a candidate age (`prune.rs:1100`).
  Declaring `oxide/outputs` with an age policy would let the nightly maintain
  delete Oxide roots by idleness. That is wrong for outputs. Slice `R2` fixes it
  before any Oxide area is declared.
- **G3 — No release-only policy exists.** `PrunePolicy` has `None` and
  `OldestIdleAfter` only (`area.rs:118`). Condition 4 of the release gate needs
  a policy that allows release by reference rather than by idle age. `R2` adds it.
- **G4 — No operator-list waiver exists.** Barge keeps any root whose reference
  set is incomplete and carries no operator list. Oxide's 2026-10-26 preview
  keeps 91 roots with "the root predates the version-2 registry identity; only
  an explicit operator list may prune it". Under Barge's rules those 91 can
  never be released without a new ruling (D4).
- **G5 — Builds `root_dir` is `barge`.** Oxide's roots sit at the drive root
  (`oxide-outputs/`, `oxide-pool/`). An area override is relative to `root_dir`,
  and `..` is refused (`stage1-design.md`, path containment). Declaring Oxide's
  outputs under `root_dir=barge` would mean moving data. Changing `root_dir`
  with `volume update --root-dir` is a registry write that moves nothing
  (`main.rs`, update branch). D7 decides it.
- **G6 — Oxide's writer-custody reference is not readable by Barge.** Oxide's
  release rule requires every writer positively closed "across both pools"
  (`workflow.md`, prune eligibility). Barge reads holds and same-uid process
  facts. It does not read Oxide's pool ledger. The 2026-10-26 preview has one
  row kept for "writer custody is unknown" from the other pool. Slice `R4` adds
  a read-only reader for that fact, and any unknown keeps the root.
- **G7 — Builds is `kept`.** T9b withdrew durability to `kept`, so Barge refuses
  expendable work there (`refused_boundary`). Release condition 4 needs
  `expendable`. Restoring it is a separate operator go (T9 says "do not restore
  without a new go").
- **G8 — The reaper calls Oxide's prune.** `reap.py` runs
  `oxide outputs prune --dry-run`, then one real `--plan` run, after its own
  scratch sweep (`reap.py:156-183`, called at `:222`). The job must not change
  until D3 has a defined cutover.

**Live state, observed read-only.** The 2026-11-01 04:55 Oxide preview has zero
candidates and `complete: false`. It reports 660 of 1091 registry roots scanned,
and shows 512 kept rows (the preview's cap). All 512 carry the reason "the
registry snapshot is incomplete (the recorded root's device or inode changed);
the whole batch is kept". The 2026-10-26 preview had 114 candidates. Its kept
reasons were 111 "checkout absence is unknown (ancestor missing)", 91 legacy,
45 present checkout, 23 device or inode changed, and one each for the others.
This design does not investigate why the batch is now kept. The F5 census
(gate `G0`) is the evidence for that.

## Decisions

### D1. Integration surface: Rust library at a pinned commit

Oxide links the `barge` library at an exact 40-hex commit, as Barge's
`consumption.md` requires for Rust consumers. This is already the ruled
direction: Oxide's [vision](../vision.md#storage) names Barge's library for the
Builds pool, output placement, scratch and pruning. The pin is re-set only by a
reviewed Queue brief. The lock is
committed. Offline builds take the locked Git candidate from Oxide's bounded
seeder, the same path as any locked Git dependency.

Alternatives:

- **CLI and JSON from Oxide.** Rejected for the gate. Each evaluation would spawn
  a process and parse a second copy of the plan, and the two processes would
  hold the locks in turn. The CLI stays the surface for the host job and for
  Effigy, Queue and other non-Rust tools (`consumption.md`).
- **Copy Barge's gate into Oxide.** Rejected. That is the drift `stage4` exists
  to end.

Absent Barge: the library is compiled in, so absence is a registration fact.
An unregistered Builds volume, a missing state directory or a volume the
registry cannot verify makes Oxide's prune refuse with exit 2 and name the
missing piece. After `O3` lands, the evaluator never runs as a fallback for
Barge's refusal. It runs only for the legacy class under D4, if that class is
kept.

Version rules: a Barge pin changes only with a brief that names the commit,
its schema versions and the selectors that prove it. A Barge schema Oxide does
not read is `unknown` and keeps the root. Oxide never reads Barge's state
directory itself.

**Landed: `O1` (oxide#219).** `Cargo.toml` declares `barge` by full rev
`4af9360bd09629e970c143080ca159de8eef2562`, the merge commit of Barge PR #39 on
`main`. That commit contains R1 (`d0a88c8`, `db0bc59`), R2 (`fb3002c`), R4
(`0d9b419`, `d4b5d90`) and the PR #39 release-proof corrections (`cc8d142` to
`346076e`). The feature-off library is used; `proof-synthetic-account` stays off.
`Cargo.lock` gains only the `barge` package, depending on the already-pinned
`serde`, `serde_json` and `libc`. The bounded seeder serves the locked Barge
source beside Longhorn, one database per repository. The pin is not a release,
and no output placement, prune, admission or host behaviour changed.

### D2. Areas

| Area | Declared in | Folder | Absent | Prune | Reserve | Why |
| --- | --- | --- | --- | --- | --- | --- |
| `oxide/outputs` | `F6` (`O3`) | `oxide-outputs` (override) | `wait` | `release_on_reference` (`R2`) | `excluded` | Roots are released by reference, not idleness. Oxide's per-checkout fallback is not one fixed path, so `FallbackLocal` cannot express it. |
| `oxide/pool` | later (`S8-pool`) | `oxide-pool` (override) | `wait` or `fail` | `none` | `excluded` | Slots are leased, ledgered and evicted by Oxide under its own lock. Barge deleting a slot would be a second deleter (see D3). |
| Oxide scratch (`scratch/oxide`, `.oxide-scratch.json`) | later (`S8-scratch`) | `scratch/oxide` | n/a | delegated | n/a | Foreign marker: read-only and kept without a recorded delegation (`stage1-design.md`, compatibility table). |

Notes on the outputs row:

- `reserve: excluded` is the proposal (Q-B). With `counts`, pressure would
  reclaim idle output roots whose checkout is still present. Oxide has no idle
  clock for outputs, and that behaviour is not ruled. The space path therefore
  reclaims only Barge's own scratch in F6, as T9b assumed.
- `release_on_reference` is new. Condition 4 of the gate reads it as "allowed by
  release", and `maintain` and the space path never consider it (`R2`).
  An `OldestIdleAfter { age: 0 }` alternative was rejected: it would look like an
  idle policy in every report, and `maintain` would act on it.
- A declaration's authority is the stored `barge.area/1` record, written once by
  `barge area declare`. A library caller must present the same declaration or
  refuse. That answers O-003 in favour of the stored record (Q-G).
- No undeclare verb exists. Rollback is a re-declare with `prune: none` and
  `reserve: excluded`, which bumps `generation`.

### D3. Prune: from Oxide's evaluator to Barge's gate

**Reference set.** Barge's release gate reads these Oxide facts in place and
never writes them:

1. the output registry, `outputs/registry.json` (`oxide.outputs.registry/2`, and
   `/1`), its rows, `selected`, `pending`, reservation and cancelling flags,
   `intents`, `cancelled_overflow` and incompleteness. Bounded by the census caps;
2. each checkout receipt (`oxide.outputs.transition/2` and `/1`): committed
   selection, nonce, generation and kind. A `/1` whole-target receipt (oxide#152)
   is read and kept unless its own rule frees it;
3. each root marker (`oxide.outputs.root/2`, `/1`): kind, checkout, volume UUID,
   nonce and generation, in Oxide's order;
4. Oxide's pool-ledger writer custody (`R4`);
5. the checkout path's absence chain, through Barge's absence primitive (`F2`);
6. Oxide's control-state fence (state version 3, `owned_outputs: oxide.outputs/2`).
   An unknown fence keeps every root it covers.

**Cutover (`O3`).** `oxide outputs prune` becomes a thin caller over Barge's
plan and, for the real run, over `R1`'s effect. It keeps its flags, its exit
mapping (documented in the brief) and its refusal messages where they still fit.
Its own evaluator is kept only for the legacy class under D4. One deleter runs
at a time: a real run takes Barge's `volume-<uuid>.lock` through the library, and
the host job calls one path, never both. Oxide's pending `prune-intent` journal
entries are settled by Oxide's own replay (`oxide outputs resolve-intent`) before
the first Barge real run. Barge keeps a root that Oxide still marks pending, and
it never settles an Oxide intent.

**Parity.** Before `O3`, a read-only shadow (`O2`) runs both evaluators on the
same snapshot and reports every difference. The intentional differences are:

- **D-1, device-only drift.** Oxide keeps the whole batch when any row's device
  changed. Barge reads a device change as a remount (`Verified`, with
  `remount.device_changed`), and keeps only a root whose inode changed. Barge's
  rule follows the carried finding (`stage4-design.md`, "What the finding does
  and does not prove"). It differs from Oxide's, so it needs Tom's acceptance (Q-E).
- **D-2, inode drift.** Oxide keeps the batch. Barge keeps that one root.
- **D-3, legacy roots.** Oxide releases them only under an operator list. Barge
  keeps them until D4 is decided.

Any other difference blocks `O3`.

**Host job.** The reaper's `prune_outputs` leaves the job only at `G-F1`, after
`O3` is proven. The barge wrapper in `host-job.md` runs `maintain --space` only
after `R2`, so `maintain` cannot reach a release-class policy.

### D4. The 91 legacy roots

The 91 are the rows Oxide keeps as "predates the version-2 registry identity".
They have no Barge identity record, so Barge's gate keeps them as
`identity_record_missing` (condition 2). Three options:

- **A. Keep them, Oxide-side (recommended default).** Oxide's evaluator stays for
  this class only, under its own operator list. Keeping them is a valid outcome.
  `O4` (retire the evaluator) waits until this decision is made.
- **B. Release them through Barge.** Needs three things. First, a Tom ruling
  that a named, digest-bound operator attestation may waive receipt-history
  completeness for named roots. That is a new evidence kind for condition 5, and
  it is the one place this contract would break "no override" (`stage4`). Second,
  `barge area root identify` per root (O4-002, explicit, custody `Safe`). Third,
  `R1` and `R3`.
- **C. Keep the evaluator permanently for this class.** Simpler than B. The cost
  is one more evaluator for the life of the drive.

The operator list in B is produced by a read-only census command. It takes F5's
census and the 91 rows, requires "legacy, no current selection, marker
well-formed, checkout absent under the chain proof", and writes a file whose
digest Tom approves exactly. The list is an input to `barge prune --plan`. It is
never the source of authority, and the plan rejects any listed root whose
evidence has changed. Releases run in batches of at most 10. Each root is
re-proved and has its own custody and intent, so one unknown root keeps only
itself. Any effect whose state is uncertain stops the batch and keeps the rest.

### D5. Space guard and recheck

**F6 adds no guard call.** The release gate does not need the volume's
admission decision, and a deletion driven by admission would be a second prune
path.

**`S8-pool` adds the guard.** Oxide's admission calls
`barge space guard --volume <uuid> --area oxide/pool` and, at checkpoints,
`barge space recheck --guard <token>`. The guard requires a declared area
(`main.rs:1179`), which is why `oxide/pool` is declared in that slice and not
earlier. The token is untrusted and is re-measured. The checkpoint bound is for
that slice's brief to fix. This contract proposes a recheck after each slot
grant and about every 2 s during a build, with a wall budget, and never at the
250 ms watchdog cadence.

Oxide keeps its cap arithmetic (O-012). The guard reports `area_bytes` and does
not enforce Oxide's cap. Oxide's host-free 20 GiB watchdog stays Oxide's, since
it is a host guard, not the volume's reserve.

### D6. Fallback retirement (O-016)

Oxide's `removable.rs:955` refuses a volume at or below its reserve, and the
selection then falls back to the internal pool. `local-lease.md` records that
fallback as current (around line 787). Vision already rules "low space never
sends a build to the internal pool" (approved, not built; [vision](../vision.md#storage)).

Order, in `S8-pool`:

1. the guard runs in admission in shadow, logging its decision beside Oxide's, for
   one measured window;
2. the guard decides, and Oxide refuses instead of falling back, with the floor
   refusal kept;
3. the reserve fallback code is removed.

`local-lease.md` changes in the same PR as step 2. No step touches the floor
rule or removes data.

### D7. Builds registration

The question is whether Oxide's areas keep today's paths or move under
`root_dir=barge`. Moving data is out of scope.

- **Recommended (Q-A).** `volume update <uuid> --root-dir ""`, then
  `folders.scratch = "barge/scratch"` and `folders.caches = "barge/caches"`, then
  the area overrides `oxide-outputs` and `oxide-pool`. Oxide's roots keep their
  paths, Barge's own scratch stays off the shared `scratch/` folder, and nothing
  moves. It is a registry write with a generation bump and needs a go.
- **Alternative.** Keep `root_dir=barge` and move Oxide's roots under it. Rejected
  here: it moves hundreds of roots, and the T9 procedure forbids data moves
  without a separate go.

Builds also holds other tools' folders (`caches/`, `colima/`, `paseo/`,
`queue/`, `home-scratch-archive/`, `scratch/`). None is an area. They stay
unmanaged and are reported as kept or unknown.

### D8. Oxide rules Barge must match or document

Every Oxide eligibility rule has one of three dispositions: matched by a named
Barge rule, documented as D-1 to D-3, or blocking. The matrix is `O2`'s deliverable,
and it covers at least: registry completeness; `pending`, reservation and
cancelling rows; overflow rows; `/1` and `/2` receipts; `/1` and `/2` markers;
nonce and marker identity; checkout absence, including the ancestor-missing class
(111 rows in 2026-10-26); the boot-Data legacy rule; volume UUID and mount
verification; writer custody across both pools (G6); and the operator list (D4).

### O2 capture boundary (design)

The O1-pinned production `barge::release::plan` reads its volume registry and
constructs `OxideSnapshot` inside the planner; its request does not accept an
already captured Oxide snapshot. Oxide's `evaluate` likewise reads receipts and
other authority while evaluating. Calling the two in sequence therefore does
not establish that they evaluated identical facts. A shadow must bind the
registry, receipts, markers, volume and absence observations, writer custody,
control fence and declaration to one bounded capture and its observation window.
It must report insufficient evidence wherever either evaluator's actual inputs
cannot be bound to that capture. A digest of only the files that happen to be
read, or a stable-looking pair of sequential plans, is not proof of identical
inputs. This constraint does not authorize redirecting production account trust
or enabling Barge's synthetic-account feature in the production dependency.

#### Fixture capture and comparison seam (design)

A synthetic comparison uses one fresh, privately owned fixture tree and one
observation window. Its capture is a versioned, bounded manifest, serialized in
one deterministic order and SHA-256 bound. It names the exact Oxide source and
Barge dependency revisions, the window's start and end, the fixture volume and
root mapping, the declaration and volume registry, both Oxide pools and their
control fences and ledgers, the output registry, each receipt and marker, and
each absence and custody observation. File entries bind the exact bytes and
object identity or an explicit missing/unreadable result; observations carry
which actor made them and their outcome. Enumeration, byte, row and time caps
are part of the manifest. A cap, read failure, missing mandatory fact, ambiguous
root mapping or unclassified actor makes the capture insufficient rather than
an empty value.

Neither evaluator may be replaced by an independently rewritten release rule.
The Oxide side calls its actual prune evaluator; the Barge side calls the
feature-off pinned `barge::release::plan`. Its test-only account and platform
adapters may point only at the fixture, and must not be compiled into the
production dependency. Setup completes before the capture window, and no
fixture producer or case helper may mutate authority while the two evaluations
run. Check the fixture tree before and after, including the names, types,
contents and symlink targets, and bind the observations that are not file bytes
separately. A matching pair of tree hashes alone cannot rule out an intervening
change, a mount change, or a writer appearing and disappearing. The fixture
must positively establish exclusive control over each relevant input and
adapter observation throughout the window; anything not so established makes
the comparison insufficient. Barge reads some files directly inside `plan`, so
an outer manifest does not by itself prove which bytes that planner consumed.

Compare the union of roots under an explicit one-to-one identity mapping, not
just the intersection or a capped preview. Preserve each evaluator's actual
outcome, completeness, reason and coverage. Missing or duplicate rows, a
refusal before enumeration, differing bindings, a truncated report or a
non-equivalent observation is insufficient. A known D-1, D-2 or D-3 label may
only be assigned when the captured facts and the two actual results establish
that precise case; an unexplained difference remains blocking. This fixture
seam establishes no live snapshot: production account trust stays unchanged,
and an explicit live invocation without attributable equivalent inputs must
report insufficient. Neither comparison invokes a delete path or changes the
ordinary prune route.

A read-only filesystem image alone is not yet an established capture seam:
Oxide's `store::private_file` opens authority files read-write even when its
caller only reads them, so a read-only mount can cause its actual evaluator to
refuse before the intended comparison. Barge's public planner does not accept
an `OxideSnapshot`, and its own testkit is crate-private. A proposed test-only
adapter must show how both actual evaluator calls receive equivalent file,
mount and writer observations without changing production account trust or
silently replacing the semantics under test. Until that is demonstrated, an
immutable-looking fixture and matching pre/post digests are insufficient.

The proof must demonstrate the boundary, not just the manifest parser: include
an unchanged controlled fixture and counterfactuals that change an input,
drop a binding, exceed a cap, fail an observation, or introduce an uncontrolled
writer. These counterfactuals must produce insufficient rather than parity.
The proof's nine premise dispositions must point to the actual case and helper
bodies, and its diagnostic remains one separately activated Queue proof at the
reviewed head under the amended task authority. No fixture result qualifies a
live Builds comparison or the later O3 cutover.

**Synthetic comparison boundary (Tom, 2026-11-01).** Use exclusively owned
fixtures with attributable equivalent inputs for O2. Live comparisons without
an adequate capture seam remain insufficient. O3 remains blocked until O2's
parity requirements are satisfied. This ruling changes neither production
account trust nor the separate G0 gate.

### D8 comparison cases and source boundaries

These are required comparisons, not parity evidence. Each case needs the same
bounded facts on both sides and must retain each evaluator's own result and
reason. A refusal is not equivalent to an eligible root. An unreadable or
unbound observation makes the comparison insufficient, even if both sides keep.
The sources here are Oxide's `src/bin/oxide/prune.rs` and Barge's O1 pin
`4af9360bd09629e970c143080ca159de8eef2562`, especially
`crates/barge/src/{release,reference,pool_custody}.rs`.

| Input or rule | Required positive and counterfactual cases |
| --- | --- |
| Registry | Complete and valid; incomplete, missing, malformed, oversized or contradictory. The batch must not be presented as complete when authority is unknown. |
| Current references | Separately exercise selected, pending, reservation, cancelling and pruning, plus an inactive row; test shared roots across checkouts and members. |
| Overflow | Sticky incompleteness and cancelled overflow, including an unfinished cancellation. Do not equate an unexamined overflow with no reference. |
| Receipts | Both supported predecessor and current formats, committed selection, history, preparation and reservation; missing, malformed and contradictory receipts. Preserve any format-specific keep. |
| Markers | Both `/1` and `/2`, absent, malformed and replaced markers. Barge's `read_oxide_marker` reports `/1` unknown; that is not evidence that it parsed `/1` as `/2`. |
| Root identity | Match and individually change nonce, generation, kind, marker digest and inode. Oxide checks committed receipt and recorded marker identity; Barge's reference reader uses root paths for reference attribution and does not compare the row nonce to Barge's nonce. A difference needs an actual per-root observation, not an assumed match. |
| Checkout absence | Present, positively absent, missing ancestor, unreadable or replaced ancestor, and the listed legacy boot-Data exception. Preserve volume binding and distinguish unknown from absence. |
| Volume | Same UUID, remount, duplicate UUID and absent volume; independently change root device and inode. A volume refusal is not an empty matching root set. |
| Writer custody | Closed, held and unknown in each pool, including a writer in the other pool and unattributed custody. The existing writer-snapshot refusal packet `c9bf3953` is a coverage limit, not a parity result. |
| Control fence | Known ready fence, missing fence and unknown version or owner; preserve any whole-batch refusal. |
| Intents | Pending Oxide intent and settled intent, including intent naming a root otherwise eligible. Oxide's dry run refuses an unfinished journal before evaluation; Barge's reference reader marks a named unfinished intent current. Compare those different scopes honestly. |
| Operator list and legacy | Listed and unlisted legacy roots, receiptless legacy, and a nonlegacy counterfactual. The list does not waive other checks and is not a Barge history waiver. |

The custody readers also have distinct intermediate classifications that need
counterfactuals before a result can be called a match. At the O1 pin,
`PoolCustody::classify` marks an acquiring record held and a retired record
without a lost fact unknown. Oxide's `Store::checkout_custody` counts spawning
and running records and skips retired records without an unresolved lost writer.
Oxide's prune eligibility then makes additional `active_holders` checks, so
these intermediate differences alone do not establish different final plan
outcomes. Exercise the actual full evaluators with the same attributable
fixture facts; if a final difference is observed and cannot be explained by
D-1 through D-3, report it as blocking rather than folding it into a match.

The frozen D-1 prose predates Oxide's device reconciliation. In current
`judge_recorded_root`, a stale recorded device alone is accepted when the
verified volume, inode and marker identity still match; a device change during
this run remains subject to the current binding. Therefore a device-only fixture
must test the actual circumstances and must not be forced to disagree. D-2's
batch-versus-root scope and D-3's legacy partition likewise need demonstrated
results. Any other disagreement remains unexplained and blocks O3.

## Failure and race matrix

"Keeps" means the root, its marker, its registry row and any pending intent are
left unchanged and reported. Stage 1, 3 and 4 rules still apply.

| Trigger | Detection | Action |
| --- | --- | --- |
| Oxide intent pending on a root at cutover | registry `intents` or `pending` row | keep that root; Oxide's replay settles it first |
| Oxide allocates a reservation while a Barge plan waits | reservation flag at the effect re-proof | keep the root; the plan continues |
| Oxide recreates the checkout's root between plan and effect | nonce or generation changes; old row not selected | keep; re-prove from scratch |
| Both evaluators run in one window | one shared volume lock taken by the real run | the second refuses `locked` and changes nothing |
| Oxide's `.prune-<intent>` leaf appears in an area folder | Barge's unit scan | keep the leaf and report it as Oxide intent residue; never a Barge root |
| A root's writer lives in the other pool, unconfirmed | `R4` ledger custody unknown | keep; report the slot |
| Oxide's registry is incomplete | snapshot flag | keep the whole batch that names those roots (D-1 defines the cases) |
| Oxide's fence version is unknown | fence check | keep every root it covers |
| `oxide/outputs` declared, `maintain` sees `release_on_reference` | `R2` policy check | no candidate, ever |
| Barge volume absent at the effect | verification | pending intent if written, else refuse; nothing deleted |
| Builds mounted under another name | UUID and mount census | `Verified` with `mount_moved`; the record is not rewritten |
| Two mounts of the Builds UUID | duplicate census | `duplicate_volume_uuid`; refuse; keep all |
| A checkout reappears between plan and effect | absence re-proof | keep |
| Oxide's root is a `/1` whole-target receipt root | receipt read | keep unless D4 allows |
| Batch of 10 hits an uncertain effect | the effect's outcome is unknown | stop the batch; keep the rest; no retry |
| Operator list edited after approval | list digest | refuse before any effect |
| Shadow evaluators disagree | `O2` report | block `O3` until each difference is matched or documented |
| Guard token replayed after its window | guard token check (`S8-pool`) | refuse; re-measure |
| Reaper runs while a real run is in flight | host job lock | the second run waits within its own deadline, then refuses |
| Cancel during plan | signal | stop after the current durable step; nothing to leave |
| Cancel during effect | signal after intent | leave the intent; exit non-zero; replay settles |

## Slices and dependencies

Finite and in order. Each slice lands with its tests and its doc update. None
authorizes a real run.

| Slice | Repo | Content | Depends on | Tier |
| --- | --- | --- | --- | --- |
| `R1` | Barge | Release effect: real `barge prune` run bound to plan bytes and digest, per-root re-proof of six conditions, `barge.intent/2` before rename, S5 custody and whole-unit delete, replay | F3, F4 evidence, S5 | ELEVATED |
| `R2` | Barge | `release_on_reference` policy; `maintain` and the space path skip it; counterfactual test with the policy present | F3 | ELEVATED (narrows what maintain may delete) |
| `R3` | Barge | Operator-attested history waiver for named legacy roots | R1, R2, D4 ruling | ELEVATED, conditional |
| `R4` | Barge | Read-only reader for Oxide pool-ledger writer custody; any lost or unknown fact is unknown | F3 | Standard |
| `O1` | Oxide | Library pin, committed lock, seeded offline build; no behaviour change | Barge commit with R1, R2 and R4 | Standard |
| `O2` | Oxide | Read-only shadow of both evaluators; the D8 matrix; never deletes | O1, F3, F5 | Standard |
| `O3` | Oxide | Thin caller over R1; single-deleter lock; intent handling; evaluator kept for the legacy class | O2 clean, R1, R2, R4 | ELEVATED |
| `O4` | Oxide | Retire the evaluator once D4 is decided | O3, D4 | Standard, after its own gate |
| `S8-pool` | Oxide | Declare `oxide/pool`; guard and recheck; retire the reserve fallback (D5, D6) | O3, Oxide cap arithmetic (O-012) | Standard; ELEVATED only if it touches slot eviction |
| `S8-scratch` | Oxide, Barge | Delegation for `scratch/oxide` | a recorded delegation | Separate design |
| `S8-registration` | Oxide, Barge | Move `removable.json` and `pools.json` registration to Barge | S8-pool, both sides agreeing | Separate design |

Dependencies that are not slices here: F5's census (landed; its evidence is
`G0`), Barge's identify verb (landed), and Oxide's `resolve-intent` (landed).

## Validation per slice

Tiers follow Oxide's [validation policy](../vision.md#validation-policy) and
Barge's equivalent. Each brief names its selectors, wall limit and scratch
budget. The figures here are proposals for the brief author.

- **R1, ELEVATED.** It deletes whole owned roots and decides custody. Barge's
  author native runs a fixture delete under Barge's own supervised runner, and
  one distinct independent reviewer native repeats it. Evidence covers the six
  conditions, custody, the intent, replay under a changed device, and the
  refusal of each unknown. Counterfactual tests: a missing identity record, a
  live hold, a pending Oxide intent, and an unreadable reference each keep the
  root.
- **R2, ELEVATED.** Unit and counterfactual tests: `maintain` with the policy
  present yields zero candidates; the space path does the same; the policy is
  not a reserve participant. The effect is narrowing, so the reviewer
  native confirms the deletion set does not grow.
- **R3, ELEVATED, conditional.** The same as R1, with the waiver tested for
  each named root and for an unnamed one.
- **R4, Standard.** Fixture ledgers: each lost, unknown, unreadable and malformed
  state keeps the root; a closed state does not.
- **O1, Standard.** `effigy rust:compile`, `effigy rust:compile-release` (the
  pin is unconditional code), `effigy rust:format`, `effigy test rust`,
  `effigy docs:check`. No native.
- **O2, Standard.** A new shadow proof under the shared supervisor, with a
  bounded wall time and a scratch budget named by its brief. The author runs
  it once; the reviewer checks the exact source and the matrix.
- **O3, ELEVATED.** Unit tests for the thin caller's exit mapping, the lock,
  the intent order and the legacy-class route. A native on fixtures: one real
  release of one v2 root, a batch, an injected uncertain effect, and a
  replayed plan. The reviewer native is distinct.
- **O4, Standard.** Parity proof retained as a regression, and the removed
  evaluator's tests replaced by the Barge gate's.
- **S8-pool.** The guard shadow window (D6) is evidence, not a native; the
  enforcement step needs the native named in its brief.

Operator gates are real-data runbooks, not briefs. Each follows the shape of
Barge's T9 qualification procedure (`t9-real-drive-qualification.md`, in Barge's
repository): a fresh
packet, `SHA256SUMS`, a plan digest, one go point per effect, and stop rules.

## Operator gates

| Gate | What it authorizes | Real data? |
| --- | --- | --- |
| `G0` | A read-only `barge inspect` census of Oxide's registry on Builds, for D-1 and D-2 evidence | Read-only; still a go |
| `G-B` | `volume update --root-dir ""` and the scratch and cache overrides (D7) | Registry write |
| `G-C` | `barge area declare` for `oxide/outputs` | Barge state only |
| `G-D` | Durability back to `expendable` on Builds (G7) | Registry write |
| `G-E` | The first real release of one v2 root, bound to one plan digest | Deletes one root |
| `G-E2` | Batches of v2 roots after `G-E` is reviewed | Deletes roots |
| `G-F1` | Remove `prune_outputs` from the reaper (G8) | Host change |
| `G-F2` | Add `barge maintain --space` to the wrapper in `host-job.md` | Host change |
| `G-G` | The legacy class under D4 option B | Deletes roots; needs a ruling |
| `G-H` | Retire the Oxide evaluator (`O4`) | Code only, after its gate |

Each gate needs its own go. A go for one does not cover the next.

## Rollout order and rollback

| Step | Effect | Rollback |
| --- | --- | --- |
| 0. Rulings: G1–G4 and Q-A to Q-H | None | n/a |
| 1. Barge `R2`, `R4`, then `R1` | Code | Revert; nothing on disk changes |
| 2. `G0` census | Read-only | n/a |
| 3. `O1` pin | Build only | Revert the pin; the previous Oxide binary still runs |
| 4. `G-B`, `G-C` | Barge state only | Re-declare the area with `prune: none` and `reserve: excluded`; set `root_dir` back by a second update. Nothing moved, so nothing to restore |
| 5. `O2` shadow | Read-only | Remove the shadow flag |
| 6. `G-D` durability | Registry write | A new go to set `kept`. T9's withdrawal is the precedent |
| 7. `O3` plus `G-E` | Deletes one root | Not reversible for that root. The rollback is the flag back to the evaluator, plus the root's recovery by the next Oxide command, which re-adopts with a fresh nonce or refuses a missing root |
| 8. `G-E2` batches | Deletes roots | Stop the batch. Do not restore deleted roots |
| 9. `G-F1` and `G-F2` | Host job | Unload, restore the archived plist, reload and compare hashes (`host-job.md`) |
| 10. D4 and `O4` | Depends on the option | Option A changes nothing. Option B is as step 7 |
| 11. `S8-pool` | Admission path | Flag back; the reserve fallback stays until its own step |

The rule for every real-data step: a rollback can stop future effects. It cannot
restore an effect that already happened. So each step's first run is one root or
one plan, and each stop point is a go.

## Open questions for Tom

Each has a recommendation. None blocks the design as a whole.

- **Q-A. Builds `root_dir`.** `""` with overrides (recommended) or keep `barge`
  and move roots?
- **Q-B. Outputs reserve.** `excluded` (recommended) or `counts`?
- **Q-C. Release from maintain.** Never automatic (recommended): release runs only
  from a reviewed plan. Or may maintain release idle-eligible roots?
- **Q-D. The 91 legacy roots.** Option A (recommended, keep Oxide-side), B (a
  Barge attestation waiver, which needs a ruling against "no override"), or C?
- **Q-E. Parity D-1 and D-2.** Accept that Barge keeps a root for an inode
  change and tolerates a device-only change, where Oxide keeps the batch?
- **Q-F. Pool slots.** Stay Oxide-owned in F6 (recommended) and declared later
  in `S8-pool`?
- **Q-G. Declaration authority.** The stored record wins and a library caller
  must match it (recommended)?
- **Q-H. Census first.** Run `G0` before anything else, to find out why the
  batch is kept now?

Barge items for the Oxide Supervisor to rule before queueing: G1 (no release
effect slice), G2 (maintain acting on declared policies), G3 (the release-only
policy kind), G4 (the waiver's status).

## Review gates

| Gate | Where it is met |
| --- | --- |
| `design_complete` | Decisions D1–D8, slices, gates, rollback, open questions |
| `consistent_with_barge_contracts` | Sources and pins; G1–G4 name every disagreement |
| `data_safety` | D3 single deleter; the matrix; R1, R2, R4; per-root effects |
| `rollout_and_rollback` | Rollout order and rollback table |
| `finite_slices` | Slice table and dependency column |
| `docs_current` | Index entries; Q-021 in questions; no current-truth claims changed |
