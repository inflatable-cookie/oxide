# Barge adoption: Oxide's output roots and the release prune

**Status (2026-11-01): design frozen; `O1` landed (oxide#219).** Tom's F6
decisions (`8058e38a`, with Q-A to Q-H answered in `12729764` and Q-D in
`55636dc8`) freeze this design. Oxide's library pin and offline seeding are in
place under [D1](#d1-integration-surface-rust-library-at-a-pinned-commit) and the
[landed O1 note](#d1-integration-surface-rust-library-at-a-pinned-commit). Later
slices (`O2`, `O3`, `O4`, `S8-*`) are not landed. No output placement, prune,
admission, host job, install or Builds registration has changed.

**Capture audit accepted (2026-11-02).** PR #222's source-feasibility design
checkpoint was accepted at `0a51530` and merged as `150369b`, with its carried
predicate regression and all five covering checks verified by the retained
reviewer. This accepts the audit's current-scope no-go; it does not establish a
shared input seam, implement the shadow comparator or supply parity/native
evidence. The actual O2 comparison still needs its separately scoped capture
contract and implementation. O3 remains gated on clean equivalent-input D8
evidence; landing this checkpoint does not satisfy that prerequisite. A native
diagnostic at a future implementation head is not a missing proof of this
design checkpoint.

Oxide's test-only capture apparatus uses a single re-rooted test crate for
binary and library leaves. The shared capture contract below remains the
acceptance authority; this composition alone is not parity evidence. O3
remains held on equivalent-input D8 evidence.

The stable-build repair keeps the production authority-budget assignments
inside supported cfg blocks, passes each registration reader's existing control
path only to test recording, and gives the test chain a mutable authority scope.
It changes no policy or descriptor operation. The earlier failed Cargo check
and retained receipts remain historical; this repair supplies no private-launcher,
export or parity qualification.

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

## Controlled decision comparison

Earlier actual-engine calibration receipts (oxide#279) passed at source
`f12d65686b11fbc166e3afc3168142155b2c1f79`: one uncounted replacement
`rust:release-comparison:diagnostic`, followed by one counted
`rust:release-comparison`. Both run actual Oxide `prune::evaluate` and the
unchanged feature-off pinned Barge `release::plan`, never copied policy.
Independent review identified that the selected case also created the checkout,
changing the candidate's absence premise. Those receipts therefore do not
cover the required selected-but-absent control or establish task acceptance.

| Case | Oxide | Barge |
| --- | --- | --- |
| Unselected v2 root, absent checkout, closed writers | Complete; candidate | Complete references; candidate; positive checkout absence |
| Selected root, present checkout | Complete; kept because checkout is present | Complete references; kept by current registry/receipt/marker references |
| Required state contains malformed JSON (`{`) | Incomplete authority; whole batch kept for unknown writer custody | Incomplete references; kept for unknown pool/checkout custody |

One case specification maps each logical root to two physical fixture roots:
Oxide's canonical `<mount>/oxide-outputs/<root>` and a root created through
Barge's area API. Every case reports the union of both roots, actual
completeness and reasons, and retains global refusal separately from per-root
keeps/candidates. Assertions require the candidate in both engines and each
control's intended gate, rejecting clock/policy masking. The corrected selected
case keeps its checkout positively absent and changes only registry/receipt
selection. It retains the no-offer expectation for both engines. No native
occurrence at this corrected premise is authorised by the spent allowances;
its actual outcomes and counted acceptance remain missing. Source inspection
suggests a possible difference: Oxide validates the committed selection and
then continues to checkout absence and custody, whereas Barge retains current
references. This is a prediction, not an executed outcome or permission to
change either engine.

One fresh 128 MiB APFS image per occurrence supplies real kernel-verified
mount/device/UUID facts to both engines. Its backing/evidence remain under
Stash's qualified guard root. `scripts/test-release-comparison.py` reuses the
owned-image kit: custody indication before attachment, `-nobrowse -owners on`,
raw attach retention and guard-only post-census detach. No deletion verb runs.
The guard remains byte-identical `fcfda2b0…`; each occurrence has 300 s and
512 MiB bounds. Barge's empty descriptor observation/self identity and the
empty lease ledgers are explicitly synthetic. Account, registry, receipt,
marker and control files are fictional and fixture-owned. Compilation uses
ordinary managed storage; no internal fixture root is needed.

The shared unknown premise is source-traced: Oxide's
`PoolWitness::read_state` passes the required state bytes to its actual parser
and refuses the authority batch on invalid JSON. Barge's
`PoolCustody::read_pool` passes the same bytes to `parse_state`, records
pool-wide unknown, and `OxideSnapshot::references_for` makes references
incomplete. The comparison-local markers carry `checkout_volume_uuid` before
digest derivation, matching the production root/2 format. The request clock
is actual post-setup time; unchanged `ReleaseOnReference` has no minimum
age/cooldown but requires known activity no later than that clock.

Successful evidence under `/Volumes/Stash/scratch/oxide/`:

- `oxide-proof._ad3hi_s/evidence`: uncounted diagnostic, 3/3 cases, exit 0,
  37.72 s to receipt publication, minimum reserve 3044.6 GiB, sampled owned
  peak 8.1 MiB; receipt SHA-256
  `955a106ebc77a6ccd5e3868f9a906a5ce7c7e9a6bf215d7ea61189bbdb632c6f`.
- `oxide-proof.t402z4xm/evidence`: counted comparison, 3/3 cases, exit 0,
  8.39 s to publication, minimum reserve 3043.7 GiB, sampled peak 8.1 MiB;
  receipt SHA-256
  `727ff9468cf116d7234061564d0d0d0b73cfa7a8de595d17c335ec3c02f6f8c7`.

Both guards closed with no fault or deadline, positively reaped their detach
children and verified ordinary post-census detach. Backings and evidence are
retained. Sampling is not a quota. The case reports retain actual fixture
input bytes, adapter facts and both engine results; ordinary logs bind source,
pin, exits and guard outcomes.

**Preserved limits and failed history.** Feature-off Barge's `MountPoint`
constructor is private and its synthetic testkit is crate-private.
Oxide's `observe_row_root` derives the mount from the root's grandparent and
`Binding::open` accepts exactly `/Volumes/<name>`; nested private scratch
cannot satisfy that layout. Brief v3 authorised real Stash verification but
could not solve Oxide's layout. Brief v4 authorised the dedicated owned image;
no synthetic constructor, forged binding, alias or live output namespace was
used.

The first uncounted occurrence at `7d36faaa` failed because `now_ms=1000`
predated real activity, masking all Barge gates (`oxide-proof.xypmxi9_/evidence`,
exit 101, 66.36 s). The clock-corrected replacement at `5254a9ec` exposed a
fixture inconsistency: the registry recorded the actual checkout UUID while
prune tests' `root_marker` omitted it. Barge correctly refused conflicting
`Some(UUID)`/`None` absence claims (`oxide-proof.scz62jay/evidence`, exit 101,
23.65 s). Both guards closed and detached ordinarily; neither failure earns
counted credit. These packets and their original limits remain historical.

The historical missing-state control also stays distinct: Barge permits an
absent state beside an empty ledger, while Oxide requires a state witness.
Its final Barge decision was masked by the volume conflict, so no unmasked
missing-state parity is claimed. Planner continuation
`562fafa54ae0be9b606e378a7eb5d7c4537295d3312ab162ca9c59a6da20e719`
authorised only comparison-local marker repair, a source-traced shared unknown
fact and one replacement diagnostic, with counted execution gated on adequate
results. That correction did not change either policy.

This is bounded decision calibration, not identical consumed observations,
full O2/D8/CF parity, live qualification, adoption or delegation. The mandatory
parent/descriptor gaps here and in Barge's
`oxide-capture-parent-identity.md` remain unproven; oxide#241 and oxide#222
remain held. A shared specification and retained input bytes do not close
those gaps.

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
same snapshot and reports every difference. The source-bound differences and candidate differences are:

- **D-1, device-only drift.** Current Oxide reconciles a stale recorded device
  when the verified volume, inode and marker match. Barge can report a remount
  as `Verified` with `remount.device_changed`. A live remount remains subject
  to each engine's current binding; a difference requires observed evidence
  before Q-E can decide it.
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
account trust nor the separate G0 gate. Tom further ruled that the planner
will design a test-only equivalence seam before this comparison can be
qualified; the ruling does not accept equivalence or qualify O3.

### Leaf-observation tape: source feasibility audit (2026-11-02)

**Disposition: NO-GO for implementation under the current scope.** The
candidate is an audit design, not an established seam or a parity result. The
Oxide source is the PR branch based on `bd89999`; Barge is the production O1
pin `4af9360bd09629e970c143080ca159de8eef2562`. The sibling checkout at
`39064baf` has no `crates/barge/src` diff from that pin. Both engines must run
their actual policy and strict file parsers. A tape cannot return a parsed
receipt, a custody verdict, an absence verdict, a plan row, or an expected
eligibility decision. Each cell below names the leaf work needed for a future
seam; it is not evidence that a hook exists or that the case passes.

| Boundary | Source path and observed behavior | Feasibility and required binding |
| --- | --- | --- |
| Oxide entry, pool and registration | `prune::dry_run` -> `paired_store` -> `internal_store`, `Store::open_existing_with`/`open_at`, `removable::coordinate` and `registration`; `require_ready` -> `outputs_initialization` -> `load` and `read_registry`. Unfinished intents refuse before `evaluate`. | Real private pool, configuration, ledger, registration, control state and registry bytes must pass their existing readers. Bind account/config resolution, presence and identity of each path, selected pool, sibling relation and registry revision to the manifest. Missing or unreadable sibling is not automatically an absent registration. `open_existing_with` uses `repair=false`, but `open_at` calls `lease::ledger_view` -> `Pool::open` -> `open_inner(reconcile=true, fence_control=true)`: it takes the pool lock, may write the managed control fence and reconciles holding and evicting records. The holding pass can adopt or finish records; the evicting pass can replay recorded work. A prepared clean fixture may take no such effect, but pending reconciliation or a missing control fence is a concrete mutation-risk counterfactual before `evaluate`. Trace `scratch_io::evaluate` and all opening reads too; do not substitute passive `snapshot` and call it the actual path. |
| Oxide authority and receipt reads | `AuthorityScope` and `authority_read` impose a 10 s / 64 MiB aggregate; `evaluate` caches `outputs::read_receipt_for_prune` by checkout and uses `verify_group_against_receipt`; `store::private_file` opens read-write, no-follow and checks owner/mode. `read_operator_list`, marker reads and pool state readers have distinct bounds. | Keep real descriptor opens, ownership checks, bounded reads, version parsing and cross-record validation. Test-only hooks need to record the opened object's complete path/parent/object identity, read bytes or refusal, cap and every occurrence, then bind them to the common manifest without replacing parser results. Direct reads not passing `authority_read` must also be inventoried. A writable private file is not proof it was unchanged; a read-only mount makes `private_file` fail. |
| Oxide root and absence | `observe_row_root`/`observe_root_in`, `judge_recorded_root`, `checkout_absence`, `walk_checkout` and `revalidate_chain`; `volume::Binding::open`/`verify` and `observe_held` obtain volume facts. These use real no-follow `openat`/`fstatat`, held descriptors and mount/UUID observations; the boot Data exception is conditional and never waives a conflicting recorded UUID. | Tape only leaf syscall outcomes, identities, component names, UUID/mount and boot Data facts, keyed by the held object and shared epoch. The actual walker, revalidation, marker parser and identity judge remain in place. Preserve missing ancestor versus missing leaf and descriptor-versus-name checks. A canned `Absent`, `Verified` or root verdict is forbidden. No existing cross-engine tape seam is shown. |
| Oxide writers and locks | `eligibility` calls `Store::checkout_custody` and then `active_holders` for both controls. Custody takes sibling and local admission locks, loads state, uses `boot_session`/`closed_by_boot` and classifies records; `active_holders` reads state separately. | Bind both pools' real state bytes and lock-object identities, lock results, boot and any process/group observations to the same epochs. Do not inject `CheckoutCustody`. Crucially `checkout_custody` calls `admission` -> `lock_admission` -> `open_admission_file(create=true)` and `fence_admission_state`: an absent lock can be created, and an older state lacking accounting/slot/holding fences can be saved. Pre-existing lock plus fully fenced state may avoid those writes; an absent-lock or unfenced counterfactual is a real attempted mutation, not a read-only pass. Do not hide it with a mock or repair the case between engines. |
| Oxide enumeration, time and accounting | `evaluate` uses representative groups, checks complete receipt/group/identity authority and caps; `account_candidates` walks `inventory_directory` after the authority decision; `build_preview` caps kept rows. `AuthorityScope`/inventory use `Instant`, and output timestamps use `process::now_ms`. | Tape enumeration outcomes, object identities and policy-clock readings without substituting roots or omitting unreported rows. A capped preview is not a complete per-root set: comparison needs the full bounded evaluated set and must say when either side truncated. Keep real wall and reserve deadlines outside the policy tape. Accounting is diagnostic, not eligibility, but any direct walk still belongs in the read/containment transcript or must be excluded by an explicitly reviewed entrypoint. |
| Barge setup and durable records | `release::plan` -> private `plan_observed`: `read_registry`, `area::show`, `VolumeRoot::open_with`, `resolve_by`, `list_names`, `BargeReferences::read`, `OxideSnapshot::read_within`, `pending_intents`, then `decide_root`. `StatePaths::resolve` selects account state; `reference::control_directory`/`inspect::observe_oxide_at` discover pools and `outputs`; `state::read_bounded_inner`, `reference::read_receipts` and `pool_custody::read_document` directly use filesystem metadata, directory enumeration and file reads outside `DirOps`. | Real fixture registry, declarations, references, intents, control discovery, pools, state, ledgers, receipts and markers must reach the existing parsers. `AccountFacts` can select a test fixture only inside Barge's own test build; production account resolution stays intact. Test-only tracing must cover the direct `std::fs`/`File` paths as well as `DirOps`, recording each actual read, descriptor identity, metadata, presence and enumeration result against the manifest. Merely passing mocked `DirOps` or matching pre/post hashes misses those direct reads and ABA changes. Instrumentation must preserve actual open/read/validation behavior; an unobserved read is insufficient. |
| Barge root, holds and identity | `decide_root` takes existing volume/root/unit shared locks, may re-read a marker after acquiring a second root-id lock, reads `area::read_holds`, `identity::read`/`verify`, `area::scan_activity`, `reference::read_oxide_marker`, and calls `custody::take_custody_by`. The latter re-reads registry/grants, opens the folder and unit, verifies the marker and scans custody. | Tape leaf path-component, no-follow descriptor facts, directory membership, root/marker IDs, owner, device/inode and UUID, file/hold observations, process identity and activity-time inputs at each named epoch. Keep the actual identity, hold, activity, custody and reference algorithms; second reads are distinct occurrences that must map to a common ordered observation, never independently chosen return sequences. `StateLock::shared_if_present` reads existing locks without creation, but lock outcomes and contention remain dynamic. |
| Barge absence and process census | `release::prove_references` delegates each reference to `os::prove_absence`; `AbsenceVolume` combines volume facts, UUID lookup and mount root. `ProcessObserver` provides effective uid, boot, process-start identity, platform verification and `scan_open_descriptors`; macOS `scan_processes`/`scan_table` classify processes, retry unreadable listings and handle foreign uid, blind spots and dead handles. | The existing `scan_open_descriptors` trait returns an already classified `DescriptorScan`, not a primitive leaf. Supplying it from a tape would bypass part of the real custody observation algorithm. A separately scoped Barge **in-crate** test seam must instead expose the process table, per-PID identity/start/uid, per-fd listings and vnode outcomes, retry/clock epochs and platform-signature leaf facts below that scan, while retaining actual classification, bounds and retries. Tape component-level absence facts, never a complete `AbsenceEvidence`. The existing `scan_open_descriptors_with` is `pub(crate)`/`cfg(test)` and does not establish the whole two-engine seam. |
| Barge clocks and limits | `plan_observed` uses `Instant` for the plan deadline, `state::Deadline` uses its own `Instant`, `decide_root` calls `state::now_ms` for marker and activity scans and compares to request `now_ms`; locks and process scans have their own timing. | Bind policy-relevant clock/age readings and each observation epoch; preserve real monotonic session, lock, reserve and shutdown limits independently. A tape must never extend a real deadline or authorize a host effect. A sleep, delayed callback or unclassified live clock read cannot be silently treated as the same observation. |

**Tape and reconciliation contract to design, not yet implemented.** A key must
include observation kind, fixture/root identity, complete object or process
identity including start/boot/volume UUID where relevant, and an explicit epoch
and occurrence. A component path is bound to the observed parent and exact
entry, not just a string; a process is not identified by PID alone. The owner
publishes a versioned, size- and count-bounded file manifest and tape under one
private fixture, closes setup writers, records real fixture custody and starts
one window. Both in-crate executables read that same root and invoke their own
actual entrypoints, without restoring or copying it between runs. Actual reads
and tape requests are recorded separately in bounded transcripts; the
reconciler rejects a missing/unclassified/extra live read, wrong epoch,
conflicting result, malformed or oversized tape, unbound file identity, write,
changed fixture or incomplete root/receipt/row enumeration as **insufficient**.
The union of rows needs an explicit one-to-one volume/root mapping; a command
refusal or truncated row is not an absent or matching root. No generic sequence
may give each engine convenient but incompatible facts. Positive control of all
writers and every observation remains an unproven premise; hashes alone do not
prove it, and this audit does not claim a complete read interception.

**D8 case-to-reader obligations.** These are proposed cases, not observed
results. Every valid input is written as real fixture bytes and read by the
above parser paths; each malformed, omitted or contradictory counterpart is
also exercised through those paths. Every case retains both actual results and
raw reasons, with unknown/unmapped input marked insufficient:

| D8 family | Actual path and controlled counterfactual |
| --- | --- |
| Registry and overflow | Oxide `read_registry`/`Registry::validate`/`evaluate`; Barge `reference::read_registry` and `OxideSnapshot::references_for`. Complete, sticky-incomplete, missing, malformed, cap and cancelled-overflow/reservation cases; bind registry bytes, file identity and list coverage. |
| Selected, pending, cancelling and intent | Oxide `Row::live`, `dry_run`, `eligibility`; Barge `OxideRow::live`, `references_for`, `pending_intents`. Exercise individual flags and groups plus unfinished versus settled intent. Preserve Oxide's whole-command early refusal versus Barge's named-root reference; never invent missing per-root results. |
| Receipt and marker formats | Oxide `read_receipt_for_prune`, `observe_root_in` and actual marker parser; Barge `read_receipt`, `read_oxide_marker`. Supply both supported formats, missing and malformed bytes, and their real parser limitations; Barge's root/1 unknown is not a successful parse. |
| Identity and D-1/D-2 | Oxide `judge_recorded_root`/group verification and Barge `identity::verify`/`compare`, `row_names_root`. Independently change nonce, generation, digest, inode, recorded device and same-UUID mount epoch. Assert each raw reason; do not fabricate device drift disagreement after Oxide's reconciliation. |
| Checkout absence | Oxide `checkout_absence`/`walk_checkout` and Barge `prove_references`/`os::prove_absence`: present, missing leaf, missing ancestor, boot Data legacy, changed ancestry and conflicting UUID. Replay component observations and held identities, never a verdict; an unsupported anchor is insufficient. |
| Volume | Oxide `Binding`/registration verification and Barge `VolumeRoot::open_with`/UUID lookup: same UUID remounted, duplicate UUID, absent mount and mismatched device. Bind exact mount census and epoch; the real image's own custody uses real facts, not tape. |
| Both-pool writer custody | Oxide `checkout_custody`/`active_holders` and Barge `PoolCustody::read`/`classify` plus `custody::decide`. Cover closed, live, lost, unknown, acquiring, retired, clean and unattributed records in both pools. Their intermediate classifications differ; only complete final actual results can establish a difference. Writer-snapshot refusal `c9bf3953` remains a limit, not parity. |
| Fence, declaration and policy | Oxide `outputs_initialization`, state parser and admission fences; Barge `area::show`, `pool_custody::parse_state`, `decide_root` policy/activity. Cover ready/unknown/missing control fence, exact stored declaration and absence, and a fully fenced no-write control. Detect admission lock creation/fence save as an effect, not a successful read-only comparison. |
| Legacy/operator list D-3 | Oxide `read_operator_list`/`evaluate_receiptless` and Barge identity and reference conditions. Cover listed/unlisted/receiptless legacy and nonlegacy counterfactual; retain D-3 only when actual outcomes and inputs establish it, and do not treat Barge's named-root age waiver as an Oxide legacy waiver. |

**Mandatory negative controls.** The future harness must cause and recognize:
(1) an escaped direct untaped read, including Barge's `state::read_bounded_inner`
path; (2) a controlled writer introduced between runs, including a writer that
later closes; (3) replaced file with identical bytes, changed parent or changed
opened descriptor identity; (4) a valid-looking tape missing one required leaf,
a duplicate/conflicting key and an exhausted byte/row/time cap; (5) PID reuse or
changed start/boot, a changed mount/device/UUID epoch and remount with the same
UUID; (6) absent admission lock and missing fence causing a real create/save
attempt, and pending holding or eviction reconciliation reached through
`ledger_view`, recorded as effects and a stop; and (7) Barge's marker reread under
its second lock and Oxide's descriptor/name ancestry revalidation seeing
inconsistent epochs. Every one must result in insufficient or an honest effect
counterexample, never parity. The oracle must itself use real kernel facts for
child custody, process signalling, attachment identity, detach and cleanup;
taped policy facts confer no such authority.

**Cross-repository and proof boundary.** At the pin, Barge `lib.rs` exposes
`testkit` only as `pub(crate)` under `cfg(test)`; Oxide's dependency cannot
import it. Two separately built in-crate test executables need a shared,
versioned tape/manifest specification, bounded export and an orchestrator that
uses their actual source and verifies the exact source/lock digests. The
smallest proposed follow-up is a separately scoped Barge change adding
`cfg(test)`-only leaf instrumentation and a bounded in-crate test entrypoint,
plus corresponding Oxide-only test instrumentation and a read transcript.
It must explicitly resolve how to bind that Barge test source to the O1
production pin without treating changed production code or a duplicated policy
as the pin. A separate Oxide scope must decide the admission-lock/fence write
counterexample without stubbing it; a production read-only custody/API change,
if needed, is material and needs Tom's ruling. No Barge edit, pin change or new
production seam is authorized here. Until all direct reads, identities, lock
semantics, dynamic leaves and no-write behavior are accounted for, there is no
complete equivalent-input mechanism and no clean O2 evidence.

The prospective fixed proof has only the existing qualified supervisor,
default Builds routing, 300 s / 512 MiB, at most one owned 128 MiB image and one
ordinary detach, and separate real image custody; its time and disk fit are
unmeasured. Exact source staging and export of both in-crate executables, their
lockfiles and dependency caches, as well as transcript and fixture size, need a
separately reviewed bounded implementation; no native, image or proof ran in this
audit. All nine workflow proof-authoring premises are **unassessed in actual
case bodies**, because no harness exists: private HOME/rustup and locks;
account-home controls; environment scrubbing; auto-adoption; Cargo external
argv; owned image and detach; compiler flags; temp placement; and explicit
macOS mktemp templates. Each needs its actual helper location and disposition
before proof, with real containment facts and the single Queue diagnostic route.
No existing evidence, the earlier live-pool eviction, or this source audit is a
native pass; G0 and O3 remain separately gated.

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

### Shared capture contract and implementation slices (F6-O2 design, 2026-11-02)

**Disposition: design, not a seam and not parity.** This section turns the
source audit above into the concrete contract an implementer can follow. It
binds the merged PR222 checkpoint/source (Oxide `main` merge `150369b`, whose
audit is `0a51530` and whose carried flag-predicate regression is
`prune::tests::shadow_reference_flags_and_incomplete_registry`) and the exact
O1 Barge pin `4af9360bd09629e970c143080ca159de8eef2562`. The sibling Barge
checkout at `39064baf` has no `crates/` diff from that pin, so every source
claim below reads identically at both. It specifies the two cfg(test)-only
observation seams, the versioned capture/transcript schemas with exact caps,
the reconciliation oracle, the finite D8 case matrix, the seven mandatory
counterfactual realizations, and three reviewable implementation slices. It
authorizes no implementation, no production API or behaviour change, no pin
change, no native proof and no live read. O2 parity stays unproved and the
O3 dispatch hold stands until later accepted actual comparison evidence
releases it. Native proof is not a requirement or a missing proof of this
design PR; the future implementation declares one exact-head Queue proof that
its own retained reviewer judges.

#### Entry point and no-write decision (resolved)

The comparison calls each engine's actual decision chain against one privately
owned fixture; it never injects parsed records, never stubs a custody or
absence verdict, never copies a policy engine, and never enables Barge's
`proof-synthetic-account` feature in the production dependency. Three
source-backed alternatives were compared:

- **Chosen: test-only entrypoints over the actual evaluators, with real
  prepared fixture stores.** The Oxide harness calls, in process and in the
  frozen order, the actual `prune::dry_run` chain —
  `AuthorityScope::begin` (the real 10 s / 64 MiB authority budget,
  `prune.rs` `SNAPSHOT_TIME`/`SNAPSHOT_MAX_BYTES`), `prune::paired_store`,
  optional `prune::read_operator_list`, `prune::require_ready`, the
  `registry.unfinished_intents()` pre-evaluate refusal through
  `prune::unsettled`, and `prune::evaluate`. A cfg(test) adapter follows
  that call order and exports every `Evaluation` entry (root identity, candidate
  or kept reason) and `complete`, or the actual pre-evaluate refusal. It does
  not call `dry_run`, `build_preview`, or redirect stdout; it produces no preview
  digest. The actual evaluator and its accounting walk run unchanged. The adapter
  must be checked against the `dry_run` call order and refusal branches. The Barge harness calls
  the actual `barge::release::plan` exactly as `barge-cli` does
  (`main.rs:1484`): `StatePaths` at the fixture (explicit `override_dir`, the
  production-frozen fixture seam), a test `AccountFacts` returning fixture
  paths (test build only; production account resolution untouched), the real
  platform object for all five trait arguments, a real `ReleaseRequest` with
  `ReleaseBounds::design()` and `now_ms: state::now_ms()`, and serializes the
  returned `PruneReport` into its transcript.
- **Rejected: the full CLI command paths** (`oxide outputs prune --dry-run`,
  `barge prune --plan`). Their decision content is identical to the entrypoints
  above; what they add is exit plumbing and stdout rendering, and the CLI
  surfaces only the capped kept-row preview rather than the full bounded
  evaluated set the comparison binds. A CLI wrapper would add no authority and
  one more process to keep inside the window.
- **Rejected: a production read-only reader or custody correction.** None is
  required by this contract. `store::private_file` opens read-write, but the
  fixture is a writable private directory, so nothing fails; the admission
  lock and fence behaviour of `Store::checkout_custody`
  (`sibling_admission`/`admission` → `lock_admission` →
  `open_admission_file(create=true)` → `fence_admission_state`) is a real,
  bounded effect that a prepared fixture avoids in the control and that two
  declared cases exercise and record honestly. If a future implementation finds
  a premise the test-only seams cannot satisfy, the minimal production change
  goes to Tom as an exact reviewed proposal — an operator scope question with
  the file, symbol and behaviour named — never as a silent inference from this
  design. This contract raises no new operator scope question; Q-E (D-1/D-2)
  and the O3 gate remain the decision points.

Effects policy. Every case is **STATIC** (one shared fixture instance, both
engines' windows on the same bytes and identities, effects forbidden) or
**DYNAMIC** (each engine runs its own fresh instance materialized from the same
declared spec, declared effects and declared actor lifecycles, parity claimed
only on the declared outcome surfaces). A STATIC case with any effect row is
insufficient; an effect case's actual effect set must equal its declared set
exactly. Effects are never repaired between engines, and an attempted write is
never read as read-only success.

#### The two observation seams (cfg(test)-only, in-crate)

Both seams are **observation tapes, not injection stubs**: they record what the
real syscalls and parsers actually returned, and feed nothing back. The real
evaluator policies and strict parsers remain the code under test. Each addition
inside a production function body is a separate `#[cfg(test)]`-attributed
statement calling a cfg(test) recorder, so a production build compiles zero new
code and every existing statement stays byte-for-byte unchanged; the covering
release-profile check and the diff review prove both halves. Barge's
testkit stays `pub(crate)` under `cfg(test)` (`lib.rs`); nothing becomes
`pub`; Oxide's dependency keeps the feature-off library with no new feature
enabled.

Barge seam (B-S1) — the leaf facts the audit named, each recorded at its
source symbol:

| Leaf | Source symbol at the O1 pin | Recorded facts |
| --- | --- | --- |
| Process table | `MacOs::process_table` (sysctl `KERN_PROC_ALL`) | row identities (pid, effective uid, start), bound, epoch |
| Per-process identity | `ProcessObserver::effective_uid`/`boot_identity`/`process_start` (`macos.rs`) | uid, boot id, `(pid, start)` or positively-gone, epoch |
| Per-fd listing and vnode | `PROC_PIDLISTFDS` / `PROC_PIDFDVNODEPATHINFO` reads under `scan_processes`/`scan_table` | each fd's vnode identity or errno class, retry/backoff occurrence, epoch |
| Platform verification | `ProcessObserver::verify_platform_binary` | answer class per classified identity, epoch |
| Direct state reads | `state::read_bounded_inner`, `state::read_intents`, `registry::read`, `area::show`, `BargeReferences::read`, `reference::read_registry`/`read_receipts_with`/`read_oxide_marker`, `pool_custody::read_document`, `identity::read`, `area::read_holds` | path, parent identity, bytes digest or refusal class, cap, occurrence, epoch |
| Locks | `StateLock::shared_if_present`, `LockSet::observe_registry`/`observe_volume`/`observe_unit`/`observe_root` | lock path, taken/held/absent outcome, epoch |
| Enumeration and activity | `area::list_names`, `area::scan_activity_within`, `identity::verify`, `area::VolumeRoot::open_with`, `area::resolve_by` (via `DirOps` leaves in `os/macos.rs`) | entries with component identity, activity times, volume/mount facts, epochs |
| Custody | `custody::decide` (under `take_custody_by`), `PoolCustody::read`/`classify`, `os::prove_absence` | intermediate classification and the leaf observations above, epochs |
| Caller clock | `ReleaseRequest.now_ms` value | the value bound to the declared window |

Oxide seam (X-S1) — the same discipline at Oxide's read and effect sites:

| Leaf | Source symbol | Recorded facts |
| --- | --- | --- |
| Authority reads | `prune::authority_read` + `AuthorityScope` | bytes digest, cap charge, budget state, epoch |
| Store and pool open | `Store::open_existing_with`/`open_at`, `lease::pool::Pool::ledger_view` → `Pool::open`/`open_inner`, `removable::coordinate`/`registration` | lock outcomes, **effect rows** (fence write, holding/evicting reconciliation), sibling identity, epochs |
| Control and registry | `Store::outputs_initialization`, `Store::load`/`save`, `prune::read_registry` | bytes digests, **effect rows** for every save, epochs |
| Receipts and markers | `outputs::read_receipt_for_prune`/`read_receipt_with`, `outputs::marker_digest` | bytes digest or refusal class, cap, epochs |
| Absence walk | `prune::checkout_path`/`walk_checkout`/`revalidate_chain`, `judge_recorded_volume`, `judge_boot_data` | every no-follow `openat`/`fstatat` component identity, walk start, epoch |
| Root observation | `prune::observe_row_root`/`observe_root_in`, `judge_recorded_root`, `judge_root_identity`, `volume::Binding::open`/`open_in`/`verify` | mount census and `fstatfs` facts, UUID binding, marker identity, epochs |
| Writer custody | `Store::checkout_custody`, `Store::sibling_admission`/`admission`/`lock_admission`/`open_admission_file`/`fence_admission_state`/`fence_recorded_accounting`, `store::active_holders`, `boot_session`/`closed_by_boot` | lock/effect rows, state bytes digests, record classifications, epochs |
| Enumeration and clocks | `prune::account_candidates` → `inventory::inventory_directory`, `scratch_io::evaluate` read sites, `prune::read_operator_list` | walk roots and outcomes, cap charges, epochs |

**Unobserved or extra direct reads are rejected.** The reconciler accepts a
transcript read row only when its identity key binds to a manifest entry, a
declared dynamic fact, or a declared external allowance; anything else — a
direct read that skipped a tape site, a read of an object absent from the
manifest, an entry appearing in an enumeration that the manifest does not
declare — classifies the whole comparison **insufficient**, with the offending
row named. Each mandatory obligation names the engine and source site that must observe it.
A common fact can have obligations for both engines, but an engine is not required
to read a leaf used only by the other. Every observed occurrence must bind to
the same manifest fact; missing an applicable obligation is insufficient. Static
source-site inventory and review of all direct read paths are required alongside
transcript reconciliation: a transcript alone cannot detect an untaped read.

#### Manifest field proposal

[`oxide-capture-manifest.schema.json`](oxide-capture-manifest.schema.json)
and [`manifest-example.json`](capture-examples/manifest-example.json) propose
one machine-readable representation for the semantic contract below. They
are unaccepted until the retained reviewer judges the shape. Examples are
illustrative, with placeholder identities and digests; they are not evidence.
The Barge proposal's `barge.volume_uuid`, `namespace`, `area`, optional
`named_roots` and `volume_mount`, and its optional `fixture.volume` are
represented. The authoritative aggregate 32 MiB fixture-byte limit and
cross-field, identity, source-binding and refusal obligations are semantic
checks in addition to JSON Schema. An invalid or unbound manifest can never
establish parity. X-S2 must reconcile the reviewed shape with B-S1 before use.

#### Serialized schemas and caps

One manifest and two transcripts per case window, JSON, deterministic field
order, SHA-256 bound in every header.

**`oxide.capture.manifest/1`** — written by the orchestrator immediately before
each engine's window. Carries: `case`; the window identity; the source binding
(Oxide commit with a clean tracked worktree, per-file source digests over
`src/`, `Cargo.lock`, `rust-toolchain.toml`; Barge pin `4af9360`, Barge slice
commit, per-file `crates/barge/src` digests, the staged lockfile digest, both
toolchain versions, the exported test-executable digests); the fixture custody
(root path, owner-only mode, root dev/inode); the fixture entries, each as
`{rel, kind, bytes_sha256, size, dev, ino, parent_dev, parent_ino, mode}` or an
explicit `{absent}` or `{unreadable, reason}` result; the declared dynamic
facts (the fixture volume's UUID with its real mount point, device and
`f_mntonname`; writer rows as `(pid, start, uid, descriptor targets,
lifecycle)`; the external allowances — account-state parents, toolchain home,
fixture-root parent — by path class); and the case's `mandatory_observations`
(engine, source site, identity key) obligations and expected-effect rows. Caps: at most 4096 file entries,
32 MiB of bound bytes, 64 roots, 512 process rows, 64 mount rows, 256
mandatory keys. A cap, a read failure on a mandatory fact, an ambiguous
root-to-area mapping, or an unclassified actor makes the manifest
insufficient — never an empty value.

**`oxide.capture.transcript/1`** — one per engine, always written, including on
refusal. Carries: `engine` (`barge`/`oxide`), `case`, `manifest_sha256`, the
entrypoint identity (symbol and source binding), the outcome
(`completed`/`refused` with the engine's own refusal vocabulary and exit
code), the engine's actual result in its own document (the full bounded `Evaluation` entries and completeness, or the serialized `PruneReport`, with a truncation marker when either
side's own cap cut rows), `effects` rows (`{kind:
create|save|fence|reconcile|lock-create|rename|unlink, path, epoch}`), and
`reads` rows `{kind, identity, outcome, cap, epoch, occurrence}` where
`identity` is the complete object identity — absolute path, parent dev/ino and
own dev/ino for files; component name plus observed parent identity for path
components; `(pid, start, uid)` for processes; UUID plus mount epoch for
volumes — and `outcome` is a bytes digest, an explicit absent, or a refusal
errno class. Clock rows (`now_ms`, monotonic) are separate and bound to the
declared window. Caps: 8192 read rows, 8 MiB, at most 64 occurrences per
identity key, 512 process rows. An exhausted cap is recorded as truncation and
makes the comparison insufficient, never quietly complete.

**Key grammar and epochs.** A key is `(kind, complete object or process
identity)`; a component path is bound to its observed parent identity, never to
a string alone, and a process is never identified by a PID without its start
identity. Epochs are engine-local, monotonic, assigned at the leaf. The
reconciler never compares raw epochs or raw read order across engines: it
compares the bound fact SET and per-key occurrence behaviour. Different read
orders reconcile; a missing key, an extra key, or any occurrence whose outcome
differs from the manifest — including a second read of the same object whose
result differs from the first within one engine's window — is insufficient.
No engine is ever handed an invented return sequence: both read the real
fixture through their real syscalls, and the manifest records what is really
there once.

#### Reconciliation oracle (the acceptance test)

The reconciler, in order, and insufficient on any failure with the reason
named: (1) verify both manifests' fixture custody against the tree as it
exists now (identity, not hash-only: same bytes under a changed inode, parent
or opened-descriptor identity fails); (2) verify the source binding — the
staged sources and binaries digest-match the manifest, the Barge slice
commit's non-cfg(test) diff against `4af9360` is empty, and Oxide's worktree
is clean at the recorded commit; (3) for each engine, bind every read row
(manifest, declared dynamic, or declared allowance) and demand every mandatory
key; (4) demand the declared effect set exactly — empty for STATIC cases;
(5) compare the engines' actual per-root fates against the case's declared
expectation, accepting only `MATCH` (equal keep/release fate with reasons
preserved), a pre-declared `D-1`/`D-2`/`D-3` label demonstrated by the
captured facts, `SCOPE` (same fate, pre-declared different surface
granularity, e.g. Oxide's whole-command refusal beside Barge's named-root
row), `CONFIG` (declared configuration difference, no parity claim), or
`EXPLAIN-OR-BLOCK` (a difference that must be explained from current source
before O3 or it blocks, recorded with both raw results). A refusal from either
engine is that engine's actual outcome; it is never an absent root and never a
match.

#### The finite D8 case matrix

Fixture base (`BASE`): one private fixture tree on one real selected volume,
holding one real Oxide internal directory pool (ledger and control with all
four admission fences set, `admission.lock` present, empty records), a
complete `oxide.outputs.registry/2` with three roots — R-A (present checkout,
committed `/3` receipt, valid `/2` marker, full identity), R-B (positively
absent checkout on the same volume, otherwise eligible), R-C (legacy row: no
nonce/dev/ino/digest, absent checkout, unlisted) — and the matching Barge
state (`barge.registry/1` volume record naming the real volume UUID,
expendable, `oxide/outputs` declared with `release_on_reference`, empty intent
journal, lock files present). Ordinary cases use at most eight roots. Cap boundaries are separate engine-local refusal tests, not shared parity cases. All valid
inputs are real fixture bytes read by the real parser paths; each malformed
counterpart is exercised through the same paths. Expected fates below are
source-bound declarations the oracle checks; both engines' raw results are
always recorded.

| ID | Family / delta | Oxide expectation | Barge expectation | Relationship |
| --- | --- | --- | --- | --- |
| CTL-0 | BASE no-write control (STATIC) | R-A kept (present), R-B candidate, R-C kept (legacy) | R-A kept, R-B release-eligible, R-C kept | MATCH, effects empty, full binding |
| CTL-1 | registry sticky-incomplete | whole batch kept, incomplete snapshot | whole batch kept | MATCH |
| CAP-1 | more than 8192 representative registry rows | `evaluate` keeps every representative and sets complete=false | not asserted: its root cap concerns a different enumeration | Engine-local cap test; fixture exceeds shared manifest caps, so no shared parity claim. A separate Barge test exercises `PLAN_ROOTS` with its actual root enumeration. |
| REG-MISSING | registry file absent | `require_ready` refusal, command refused | `OxideRegistry::Unknown`, roots kept | MATCH-KEPT + SCOPE |
| REG-MALFORMED | registry bytes invalid | refusal | Unknown, kept | MATCH-KEPT |
| REG-OVERSIZE | registry over its read cap | budget exhaustion → incomplete/kept | `read_bounded_inner` cap → Unknown, kept | MATCH-KEPT |
| REG-CONTRADICTORY | two live rows, one root, two checkouts | both roots kept (shared-reference rule) | both kept | MATCH |
| REG-OVERFLOW | sticky incompleteness + cancelled-overflow tag | whole batch kept | whole batch kept | MATCH |
| REF-PENDING | row `pending` | kept (prepared transition) | kept (not current) | MATCH + SCOPE |
| REF-RESERVATION | row `reservation` | kept | kept | MATCH |
| REF-CANCELLING | row `cancelling` | kept | kept | MATCH |
| REF-PRUNING | row `pruning`, intent unfinished | pre-evaluate `unsettled` refusal | pending intent names the root, kept | MATCH-KEPT + SCOPE (whole-command vs named-root, pre-declared) |
| REF-SELECTED-NOT-CURRENT | row selected, receipt not selecting | snapshot error, whole batch kept | reference incomplete, kept | MATCH-KEPT + SCOPE |
| REF-ORPHAN-MARKED | root folder + valid marker, no registry row | not evaluated (outside registry scope; recorded as observation) | per actual gate: released only if its reference set is positively empty and identity verifies | EXPLAIN-OR-BLOCK |
| RCP-V1 | committed `/1` whole-target receipt + `/1` marker, recorded identity | eligible candidate (committed v1 is eligible; oxide#152 rules) | `read_oxide_marker` reports `/1` unknown → kept | EXPLAIN-OR-BLOCK (O3 needs an operator ruling or the D-4/D-3 route; never silently folded) |
| RCP-MISSING | receipt file absent | snapshot error, whole batch kept | incomplete, kept | MATCH-KEPT + SCOPE |
| RCP-MALFORMED | receipt bytes invalid | snapshot error, kept | Unknown, kept | MATCH-KEPT |
| RCP-NONCOMMITTED | prepared/detached receipt | kept | kept (not current) | MATCH |
| RCP-CONTRADICTORY | receipt identity ≠ registry checkout | snapshot error, whole batch kept | reference mismatch, kept | MATCH-KEPT + SCOPE |
| MRK-MISSING | root marker absent | unknown root state → kept | identity read fails → kept | MATCH |
| MRK-MALFORMED | marker bytes invalid | kept | kept (`/1`-unknown is not a parse) | MATCH |
| MRK-REPLACED | marker bytes ≠ recorded digest | identity mismatch → this root kept | verify fails → kept | MATCH |
| ID-NONCE | row nonce ≠ receipt/marker nonce | error → batch kept | kept | MATCH-KEPT + SCOPE |
| ID-GENERATION | generation or kind mismatch | error → batch kept | kept | MATCH-KEPT + SCOPE |
| ID-INODE | root inode changed vs recorded | identity error makes snapshot incomplete; whole batch kept | that root kept | D-2 (batch versus root; demonstrate with a second otherwise eligible root) |
| ID-DEVICE-STALE | recorded device stale; UUID, inode, marker match | accepted by `judge_recorded_root` reconciliation → candidate | remount `Verified` → eligible | MATCH (binds current source, never the stale D-1 prose) |
| ID-DEVICE-LIVE | volume remounted mid-window (DYNAMIC) | current binding decides (expected unknown → kept) | `Verified` with remount evidence | D-1 (declared; Q-E owns acceptance) |
| ABS-ANCESTOR-MISSING | checkout parent directory missing | kept: absence unknown (the 111-row class) | absence chain incomplete → kept | MATCH |
| ABS-ANCESTOR-REPLACED | ancestor directory replaced, new inode | `revalidate_chain` refuses → kept | identity/absence fails → kept | MATCH |
| ABS-BOOT-DATA | listed legacy row, recorded checkout on the boot Data volume (internal-placement fixture) | legacy exception admits boot-Data absence → candidate | no registered volume for the boot-Data path → kept | D-3-scope (declared) |
| ABS-UUID-CONFLICT | recorded volume UUID ≠ live mount | binding fails → kept | `VolumeRoot` refusal → kept | MATCH |
| VOL-ABSENT | fixture volume detached | roots unobservable → batch kept, incomplete | `VolumeState::Absent`, permanent loud report, no rows observed | MATCH-KEPT + SCOPE |
| VOL-REMOVED-DURING | detach between windows (DYNAMIC) | each window binds its own mount census | same | declared dynamic; parity only on declared outcomes |
| VOL-DUP-REGISTRY | two volume records, one UUID (document level) | n/a (Oxide has no such document) | registry duplicate → refusal/unknown, kept | declared per-engine record, no parity claim |
| VOL-DUP-LIVE | real duplicate-UUID mount census | coverage limit (below) | census refusal covered by Barge in-crate units | coverage limit, not parity |
| WRT-LIVE-THIS | running record + live fd, this pool | `Writers` → kept | custody not safe (ledger + descriptor scan) → kept | MATCH on keep fate only; Oxide binds the ledger record and pid if present, Barge binds `(pid, start)` and descriptor targets |
| WRT-LIVE-OTHER | running record in the registered removable sibling | removable-pool holder refusal → kept | both pools read → kept | MATCH |
| WRT-UNKNOWN | retired record, unresolved lost writer | `Unknown` → error → whole batch kept | classify unknown → kept | MATCH-KEPT + SCOPE |
| WRT-ACQUIRING | spawning record | writers → kept | acquiring classified held → kept | MATCH |
| WRT-RETIRED-CLEAN | retired record, no lost fact, no holders | skipped → custody clear → may proceed to candidate | retired without lost fact classified unknown → kept | EXPLAIN-OR-BLOCK |
| FENCE-MISSING | outputs control fence absent | `require_ready` refusal (not initialized), nothing written | control unknown → every covered root kept | MATCH-KEPT |
| FENCE-UNKNOWN | unknown fence version/owner | keep/refuse batch | keep every covered root | MATCH |
| EFFECT-1 | `admission.lock` absent (DYNAMIC) | missing existing lock refuses custody; incomplete batch, every root kept; no completed effect | `shared_if_present` observes nothing; no effect | specific refusal and no completed effect; per-engine record |
| EFFECT-2 | admission state missing accounting fences (DYNAMIC) | pool open fences old managed state before read-only custody; evaluation proceeds | custody parse unaffected → proceeds | MATCH on fates + declared Oxide effects |
| DECL-MISSING | area not declared | unaffected (candidate) | command refusal: no release policy | SCOPE (declared refusal; no per-root parity) |
| DECL-POLICY-NONE | area declared `prune: none` | candidate | kept by policy | CONFIG (configuration difference, no parity claim) |
| LEG-UNLISTED | R-C unlisted (BASE) | kept: legacy, operator list required | kept: D-3 partition | MATCH |
| LEG-LISTED | R-C listed | legacy exception evaluated; checkout absent → candidate | kept (no waiver at the pin) | D-3 (declared; Q-D owns acceptance) |
| LEG-LISTED-MARKED | full identity recorded, listed | evaluated normally → candidate | normal gate | MATCH |
| LEG-RECEIPTLESS | legacy + receipt missing | snapshot error → kept | kept | MATCH |

#### Mandatory counterfactual realizations (the audit's seven)

Every one must end **insufficient or as an honest effect counterexample** —
never parity. Each names its realization and oracle.

1. **Escaped direct untaped read** (including Barge's `state::read_bounded_inner`
   path). Pure: reconciler self-tests reject a transcript row binding to no
   manifest entry, allowance or dynamic declaration. Native: CF-1 plants an
   undeclared extra folder in the enumerated area; the engines' directory
   listings observe it, no manifest entry binds it → insufficient, named.
2. **Controlled writer between runs, including one that later closes.** CF-2
   starts a declared holder (real process, real descriptor) for window A only;
   window B's manifest records it absent. Each engine binds its own window's
   custody facts; cross-window parity is not claimed (declared dynamic), and an
   unbound custody observation is insufficient.
3. **Same bytes, changed identity.** CF-3 replaces R-B's marker (identical
   bytes, new inode) after manifest capture; the engine's read binds a foreign
   identity → insufficient. The harness then re-captures cleanly and re-runs,
   proving the detection is not a harness false positive.
4. **Broken or exhausted tape.** Pure: a transcript missing one mandatory
   leaf, a duplicate/conflicting key, and an over-cap document are each
   rejected with the named reason. Engine-local cap tests exercise Oxide representative overflow and Barge root overflow independently; neither claims partial parity.
5. **Identity epoch changes.** CF-5 binds `(pid, start)`: a second writer
   process (naturally different start) never reconciles as the first; the pure
   self-test rejects same-pid/different-start reuse. ID-DEVICE-LIVE and
   VOL-REMOVED-DURING bind the changed mount/device/UUID epoch; a same-UUID
   remount must reconcile as a declared remount, never as a fresh identity.
6. **Missing admission inputs stay read-only.** EFFECT-1 must record the
   missing internal admission lock's custody refusal on every kept row in an
   incomplete batch, with no candidate, completed effect or persistent diff.
   EFFECT-2 preserves positive fence-save coverage through the real locked pool
   open, including completed temporary create/save and final state rename. The pool-lock `O_CREAT` open remains an uncertain
   attempt, never completed creation or write parity. Pending holding or
   eviction reconciliation reached through `ledger_view` is still a stop,
   not a read. Missing-lock custody provides no positive lock-create coverage.
7. **Second reads under changed epochs.** Barge's marker reread under its
   second root-id lock and Oxide's ancestry revalidation must each appear as a
   distinct bound occurrence whose outcome equals the first and the manifest;
   the pure self-test rejects an internally inconsistent reread. Coverage
   asserts the reread actually happened.

**Coverage limits (declared, never parity).** A real duplicate-UUID mount
census cannot be produced from synthetic fixtures without kernel forgery: the
census-side refusal is covered by Barge's in-crate unit tests, and
VOL-DUP-LIVE stays a limit for the G0-style live route. A descriptor that dies
mid-scan (the `DeadHandle` path) and platform-binary blind spots are Barge
platform-verification units; the cross-engine window covers the
stable-holder path only. Synthetic equivalent-input evidence establishes the
seam and the case outcomes; it never qualifies a live Builds comparison or the
O3 cutover, and taped observations never authorize signal, image attach/detach,
deletion or any custody conclusion — the future proof's real image custody
uses real kernel facts, not the tape.

#### Premise dispositions for the future proof

The nine-item proof-authoring premise checklist applies to the future
implementation's executable case bodies; this section fixes the intended
dispositions so the implementation brief inherits them: (1) private HOME and
rustup with the invoking `RUSTUP_HOME` preserved and the staged Barge build
tree built offline from its committed lock; (2) `OXIDE_TEST_HOME_FROM_ENV=1`
on the Oxide engine child, Barge fixture selection through explicit
`StatePaths`/`AccountFacts` only; (3) the orchestrator scrubs inherited
`OXIDE_*`, `BARGE_*`, `XDG_*`, `CARGO_*` and `SCCACHE_*` values before setting
fixture facts; (4) automatic adoption premises are positively built by fixture
code, no build runs in the window; (5) no Cargo external subcommand is
invoked — not applicable; (6) volume cases use one owned 128 MiB image,
`hdiutil attach -owners on`, ordinary detach, real custody facts; (7) frozen
`RUSTFLAGS` with `CARGO_ENCODED_RUSTFLAGS` removed for both stagings;
(8) fixture, transcripts and build scratch under the qualified supervisor's
selected-volume routing, same-volume `TMPDIR`; (9) `mktemp` templates under
the owned scratch parent. Each disposition's actual helper location is
recorded at implementation before any counted run; the single Queue diagnostic
route and the shared supervisor's bounds (300 s / 512 MiB, at most one image
and one ordinary detach) stay as frozen, with fit unmeasured and reviewed at
implementation.

#### Implementation slices

Finite, in order, each landing with its own tests and doc updates. None
authorizes a real run, a pin change or a production behaviour change.

- **B-S1 (Barge repository): cfg(test)-only leaf tape and in-crate harness.**
  Files: the cfg(test) recorder module and the leaf record statements in
  `crates/barge/src/os/macos.rs` (process table, per-fd and vnode leaves,
  `DirOps`/`MountRoot`/`VolumeFacts`/`ProcessObserver` leaves,
  `verify_platform_binary`), `state.rs` (`read_bounded_inner`, lock outcomes),
  `reference.rs` (`read_registry`, `read_receipts_with`, `read_oxide_marker`,
  `control_directory_with`, `pools_holding`), `pool_custody.rs`
  (`read_document`, `classify`), `identity.rs` (`read`, `verify`), `area.rs`
  (`show`, `read_holds`, `scan_activity_within`), `registry.rs` (`read`),
  `custody.rs` (`decide` observation points), a cfg(test) transcript writer
  implementing `oxide.capture.transcript/1`, and the `[[test]]` harness
  (proposed `crates/barge/tests/tape_eval.rs`) whose argv contract is
  `--fixture-root --manifest --transcript-out --case`, which builds the
  fixture `StatePaths` and `AccountFacts`, calls the real `release::plan` with
  the real platform object, and always writes its transcript. Predecessor and
  source binding: the O1 pin `4af9360`; the PR's diff adds only
  `#[cfg(test)]` statements and cfg(test) modules, with a per-file digest map
  in the PR description proving every production statement byte-unchanged.
  Dependencies: none outside Barge. Validation: Barge's own covering
  selectors (compile, release-profile check, format, its test suite) plus
  pure tape self-tests (recorder round-trip, schema serialization, a
  no-public-surface assertion). Stopping criteria: all green, production
  digests unchanged, the harness runs its embedded unit fixture end to end.
  No Oxide-side work depends on it except X-S2 integration.
- **X-S1 (Oxide repository): cfg(test)-only leaf tape, transcript adapters,
  in-crate harness.** Files: the cfg(test) recorder and leaf record/effect
  statements in `src/bin/oxide/prune.rs` (the `dry_run` chain, authority
  reads, absence walk, root observation, judges, `account_candidates`,
  `read_registry`, `read_operator_list`), `src/bin/oxide/store.rs`
  (`private_file`, `load`/`save` as effect rows, admission lock and fence
  paths, `checkout_custody`, `active_holders`, `boot_session`),
  `src/bin/oxide/outputs.rs` (`read_receipt_for_prune`, `marker_digest`),
  `src/bin/oxide/removable.rs` (`coordinate`, `registration`), `src/volume.rs`
  (`Binding` opens/verify, mount census, `fstatfs`), `src/lease/pool.rs`
  (`ledger_view`, `Pool::open`/`open_inner` lock and reconcile effect rows),
  `src/inventory.rs` (`inventory_directory`), `src/bin/oxide/scratch_io.rs`
  (read sites), a cfg(test) manifest/transcript writer, and the `[[test]]`
  harness (proposed `tests/tape_prune_eval.rs`) with the same argv contract,
  following the `dry_run` decision prefix through the actual `evaluate` and exporting the full `Evaluation` or pre-evaluate refusal, without calling `dry_run`, rendering a preview, or redirecting stdout. Predecessor
  and source binding: Oxide at the merged checkpoint `150369b` (or its
  successor main at brief time), diff discipline identical to B-S1.
  Dependencies: none for landing; the schema constants are fixed by this
  document. Validation: `effigy rust:compile`, `effigy rust:compile-release`,
  `effigy rust:format`, `effigy test rust` (the cfg(test) additions must
  compile away in the release profile), plus in-crate pure tape self-tests.
  Stopping criteria: all covering selectors green, the harness proves its
  transcript and effect rows on a unit fixture.
- **X-S2 (Oxide repository): bounded orchestration, reconciliation and the
  D8 comparison, read-only reporting.** Files: `scripts/test-barge-capture.py`
  (proposed): fixture builder and one owner (creation, custody records,
  setup-writer closure — every child waited and reaped, pids and starts
  recorded), manifest capture, engine invocation through the two exported
  harness executables (staged from the exact Barge slice commit via the
  seeded offline Git candidate plus a committed build-tree lockfile, and the
  exact Oxide head, with digest verification before any window), the
  reconciler implementing the oracle above, and the read-only difference
  report naming every unexplained difference; pure self-test modes for the
  reconciler, the fixture spec compiler and the staging verifier
  (`--self-test`, `--plan`; effect-free). Predecessor binding: B-S1 and X-S1
  landed. Validation: its own pure selectors plus `effigy docs:check`; the
  counted native (shared supervisor, the frozen 300 s / 512 MiB / one image /
  one ordinary detach bounds, unless a reviewed brief amends them with
  measurements) is that implementation's own separately activated Queue proof
  — it is not part of this design and no `awaitingProofs` is recorded for it.
  Stopping criteria: pure selectors green, the report renders for a synthetic
  unit fixture, no fixture, pool or image touched outside pure mode.

#### Oxide test-only capture apparatus

Tom decision `c6e3e9c8` v2 (2026-11-04) separates capture delivery from
private launcher qualification. The separately activated capture-only Effigy
selectors `rust:compile:cargo`, `rust:compile-release:cargo`,
`rust:format:cargo`, and `rust:test-600:cargo` cover the build and the existing
unit and embedded tape harness through Cargo. The suite retains its qualified
600-second / 512 MiB guard and dual-root recipe. These checks do not establish
private launcher isolation, global live-state invariance or current export
production; those remain unproven and separately gated, as does X-S2 adoption.
Historical private receipts and failures retain their original meanings.
Ordinary Rust selectors continue to use Oxide.

For the planner-activated Oxide direct batch, a fresh owned 9 GiB outer Cargo
root holds `HOME`, `CARGO_HOME` and `CARGO_TARGET_DIR`; `CARGO_NET_OFFLINE=true`,
Rust 1.97.1 and the existing `5cbf42e1` lock-only offline seeder bind the
inputs. The suite wrapper inherits those Cargo paths, while creating and
validating its own internal 512 MiB fixture root and sibling temporary root;
the four inherited root/evidence controls must be absent before it starts.
The outer qualified guard enforces the independent batch and Cargo-output
bounds. A plain outer command obtains real heavy Effigy admission separately
for each of the four Cargo selectors, with no outer heavy reservation; only
bounded 45-second / 256 MiB offline seeding and pre-effect metadata precede
them. No Cargo or compiler starts before its selector's admission grant.
This is the prospective recipe, not evidence that a batch ran or that the
private launcher or global live-state checks passed.

The harness is under review against the shared capture contract. A passing
release-profile compile is needed to establish compile-away; a source claim
alone is insufficient.

- **Files.** `src/bin/oxide/tape.rs` (the binary tape engine: the
  `oxide.capture.transcript/1` document, epochs assigned at the leaf,
  per-key occurrence bookkeeping under the fixed 64 cap, the 8192 read-row
  and 8 MiB document caps with explicit `truncation` flags, effect rows,
  clock rows, an additive bounded `classifications` array, and the
  additive bounded pre/post `fixture_capture` with its diff),
  `src/tape.rs` (the separate sink used only by library unit tests), and `src/bin/oxide/tape_harness.rs` (the `[[test]]`
  `tape_prune_eval` target, `harness = false`). Recorder statements sit in
  `prune.rs` (authority scope and budget, `authority_read` core/bind,
  `read_registry`, `read_bounded`, the no-follow absence walk start,
  `fstatat`/`openat`/revalidation component rows keyed to observed parent
  identities, `HeldFacts`, the recorded-volume and boot-Data judges,
  `observe_row_root`/`observe_root_in` binding, entry, root-directory and
  marker rows, `judge_recorded_root`/`judge_root_identity`,
  `account_candidates`), `store.rs` (`private_file` opens and creates,
  `load`/`holders` authority binds, `save` as `save@control-state` effects,
  `outputs_initialization`, `checkout_custody` entry/record/verdict
  classifications, admission lock rows, the fence effect-kind overrides,
  `open_existing_mode`/`open_at`/`ledger_view` pool rows, `boot_session`
  and `closed_by_boot`), `outputs.rs` (`read_receipt_with`,
  `marker_digest`), `removable.rs` (`coordinate`, the registration open and
  read), `scratch_io.rs` (`read_document` open/meta/bytes rows, the volume
  binding), and — library-side — `src/volume.rs` (`Binding::open_in`
  parent/mount/observe/verify rows), `src/inventory.rs` (walk start and
  root identity) and `src/lease/pool.rs` + `src/lease/mod.rs`
  (`ledger_view`, the pool lock, reconcile entry classification,
  `save@ledger` and `fence@managed-control` effects).
- **Descriptor-read inventory.** The test-only descriptor wrapper currently
  covers `prune::read_registry`, `prune::read_bounded`, the root marker read,
  `store::recorded_last_use`, `recorded_lost`, `active_holders`,
  `sibling_state`, `sibling_state_checked`, `trusted_origin`, `load`, and
  `read_observations`, `outputs::read_receipt_with`, `marker_digest` and
  `read_marker`, and `removable::read_registration`. The non-test read
  statements remain in their original cfg branches. Parent identities are
  not established by a later path lookup: where the operation did not bind a
  parent, the descriptor row carries an explicit unproven note. Decision
  86249e2c permits one test-only fstat on the held descriptor for identity
  observation only; its failure does not change the real read or policy. This
  inventory is not proof that every direct read in the contract is taped.
  Only `prune::read_bounded` uses the approved additional held-descriptor
  fstat. Other authority-read sites preserve the actual read outcome while
  marking descriptor and parent identity unproven. The tape does not issue
  later path or descriptor syscalls to manufacture identities; it uses the
  existing metadata result for the removable registration, admission lock and
  volume path verification, and existing `fstatat`/`fstat` outcomes for the
  revalidation walk. Sites without those facts remain insufficient. The
  authority deadline is not reconstructed from wall time; its monotonic binding
  remains unproven until captured from the actual decision input.
- **Schema coordination.** The proposed shared manifest carries Barge's
  `barge.volume_uuid`, `namespace`, `area`, optional `named_roots` and
  `volume_mount`. Barge's small consumption surface ignores the remaining
  envelope; Oxide's proposal additionally requires the window, source,
  fixture, dynamic, mandatory-observation, expected-effect and expected
  fields. The minimal Barge fixture alone is therefore not a complete shared
  manifest. Retained review of one common shape remains required before either
  harness treats it as an accepted interface. Effect rows use one fixed
  kind (`create`, `save`, `fence`, `reconcile`, `lock-create`, `rename` or
  `unlink`), with a separate source site and an explicit attempted flag;
  attempted creation is not a successful effect. The actual pool-lock open
  uses `O_CREAT` without `O_EXCL` even with a prepared lock. A prior pathname
  metadata observation does not prove what that later open did, so the tape
  reports an uncertain create attempt and marks the observation insufficient;
  BASE can have zero confirmed fixture changes without earning parity. The
  managed-fence path also creates and writes a randomly named temporary file
  before its rename: the recorder names those actual temporal effects, and a
  final fixture diff only corroborates the persistent replacement. The shared
  manifest's exact predeclared path rule cannot predict that random name; this
  remains an explicit reconciliation gap, never an invented path or a match.
  The all-kinds manifest example exercises the proposed vocabulary.
- **Preflight observation.** The tape window begins before fixture configuration
  resolution because that resolution reads config and pool state. Actual config
  and private-file read bytes or errors are recorded without a second read;
  descriptor and parent identities remain explicitly unproven where the real
  operation did not observe them. The absolute monotonic origin/end and the
  authority deadline remain unproven, not a synthetic zero or wall clock.
  Authority begin, complete and read checks, and accounting's start,
  per-candidate and completion checks, record the actual `Instant` each
  policy branch already consumed as an offset from the tape's own monotonic
  start; a short-circuited branch does not invent a sample. These offsets are
  not the unknown absolute origin or deadline.
  Fixture snapshot read errors or the 32 MiB byte bound mark the snapshot
  truncated; a pre/post diff cannot replace effect observations. Refused
  transcripts identify `source` as `apparatus` for pre-entry validation or
  `engine` for the actual prune chain; truncation preserves that distinction.
- **Fixture configuration.** Preparation uses `config::Facts` with injected
  platform-directory inputs; the evaluator child uses the existing debug-only
  `OXIDE_TEST_HOME_FROM_ENV=1` control and explicit fixture paths. Its
  environment accepts only the fixture HOME, fixed system PATH and those
  Oxide controls; extra variables or a different PATH refuse. The child
  must check its controls and resolved state and pool before entering the
  engine. This proves no production account-home equivalence. The isolated
  validation handoff is the shared provider described below. Local tape
  selectors refuse: only a planner-bound, independently qualified shared
  packet and exact-head route can authorize a later execution. The previous
  task-local sequence and its input schemas are retired, not qualified.
  The previous `pools.json` inode replacements, including 2072777179 to
  2072840885 with identical bytes, remain failed isolation gates; the writer
  is unknown and no repair occurred. The later shared diagnostic #252 also
  failed its registry identity gate, and #253 owns its source/provenance audit.
  Neither failure grants proof credit.

  The capture manifest remains a proposal: descriptor targets require path,
  parent and object identity, and descriptor number; file entries need a
  digest or explicit unreadable refusal. `oxide_tree` names the source tree;
  volume obligations bind their declared window. Retained review and Barge
  reconciliation are still needed before either side treats the shape as
  accepted. The original caps and capture semantics remain unchanged.

  Historical private Builds limits of 2, 3, 4 and 6 GiB, the 1200-second
  covering allowance, retained files and failed packets remain history. The
  6 GiB session reported 5591.8 MiB retained and an approximately 552 MiB
  whole-block clamp; that point observation is not a current reservation or
  peak. The later 3 GiB and 4 GiB Barge / 8 GiB Oxide grants were
  prospective at the time; current ceilings are 5 GiB Barge / 9 GiB Oxide.
  The 300 s / 1024 MiB debug bootstrap cap remains a maximum, not a fit claim.
- **Harness composition.** The harness crate re-roots the actual library and
  binary modules and aliases itself as `oxide_storage`, sharing one tape.
  Library observations must come from their actual leaf sites; a fixture diff
  cannot replace them. The harness's negative control reruns the actual BASE
  engine in a fresh fixture and diverts the reached pool-lock observation to a
  separate bounded test sink. Its transcript explicitly says insufficient;
  the ordinary BASE run must contain the same library site on the shared tape.
  This source regression has not yet been executed. `HoldingSweep` belongs to the release command, while
  `paired_store` calls `removable::coordinate`, which only reads registration
  and constructs the sibling store.
  Narrow `cfg(test)`/`cfg(not(test))` splits are permitted at individual
  observation sites: the non-test statement remains verbatim, while the test
  branch calls the real operation once and records its returned value or error.
  Descriptor reads must bind the held descriptor, not a later path lookup.
- **Source map for this correction.** `config::config_file` and `inspect`
  preserve their non-test metadata, canonicalization and read statements;
  their test arms execute each corresponding operation once, record its real
  result, preserve errno and retain the original branch order and short circuit.
  `lease::canonical_existing_dir`, `fence_managed_control`, the private-file
  reader and `pool::lock_pool_inner` keep their non-test calls verbatim and
  observe actual test-arm results; parent identity is unproven where no held
  parent was observed. Pool entry names are recorded from the actual iterator,
  and each existing listed-entry metadata result is taped without a second read.
  At `pool::refuse_foreign_pool_entries` and
  `pool::validate_lease_payload_entries`, test-only local `fs` bindings wrap
  the same real `read_dir` call and its original iterator: open success/error,
  each returned entry/error and observed end are recorded without a second
  syscall. The payload wrapper also records the real metadata calls. Non-test
  loop statements stay verbatim, and parent identity is unproven because the
  iterator does not expose a bound parent. Inventory's actual directory-open,
  returned names, type/metadata results, guard samples, root verification and
  filesystem-free result are likewise observed without replaying those calls.
  This is a source claim, not an executed inventory. The engine fixture is a fresh
  re-exec with `env_clear`; `test_fs` injection and race hooks start as empty
  thread-local cells, and `volume::verified_facts` starts `None`. The harness
  does not arm those adapters. Fault-injection debug environment hooks are
  absent from that child environment; the existing test-home control is set.
  This documents those reachable seams, not a complete cfg
  audit or proof that another subprocess kept the same environment.
  `lease::persist_private_file` and
  `persist_control_state` retain the non-test write/sync/rename statements;
  test arms record temp creation, actual write result, cleanup when it succeeds
  and the final replacement separately, including a write before later sync
  failure. `store::save` similarly distinguishes an uncertain create/truncate,
  write result and final rename. `store::private_file` and
  `store::open_control_directory` record the actual open and held-descriptor
  metadata result separately. `scratch_io::read_document` records actual
  open, metadata and read errors rather than a generic refusal;
  `removable::read_registration` records `openat` and held metadata, and
  `outputs` records marker open/lstat results alongside the authority read.
  The source adds component names to tape identities, binds proposed volume
  facts to the manifest's named window, and checks mandatory identities,
  extra/unbound reads and exact declared-effect multisets after the engine
  returns. A fixture diff checks persistent observed changes only; it cannot
  account for vanished temporary effects. Actual parsed holder records have a
  separate 512-row bound; their stored wall-clock starts are never promoted to
  kernel process-start identities, and missing kernel starts and UIDs remain
  explicit. Process identities and external allowance identities that the
  row/schema cannot bind remain insufficient;
  these checks are not a cross-engine oracle or parity verdict. Note storage
  and the distinct-key table are bounded and mark truncation.
  Descriptor-relative recovery
  records the actual `stat_at` result without fabricating its parent, and its
  reached chmod and put-back sites note the real success or failure as an
  unbound effect; a descriptor-only result without a path is insufficient. In
  `open_verified_step` the observer runs only after the restoration guard is
  armed, preserving its custody ordering. These changes are source-only;
  the statement map below identifies the pool-open recovery and error paths.
  Subprocess environment closure needs the separate executable and inventory
  gates and cannot be inferred from a transcript or fixture diff.
- **Prefix reachability and effect boundaries.** `Pool::open` calls
  `open_inner(root, false, true, true)`: canonical root and directory
  enumeration precede the lock, managed-control fence, config, backend
  opening and ledger read; holding reconciliation then precedes evicting
  reconciliation. A prepared fence can avoid a fence write, but the lock's
  `O_CREAT` is still an attempted effect. A pending reconciliation can write
  and must not be treated as a static read. `prepare_new_pool`,
  `verify_scratch_initializer` and `refuse_occupied_init_directory` are
  initializer paths; `open_inner` does not call them. `holding_names` is used
  by initialization and the holding-deletion traversal, not the open prefix;
  the evaluator does not call that deleter. These are call-site distinctions,
  not a claim that arbitrary pending reconciliation is effect-free.
  The separate `cfg(not(test))` statements in `config`, `store`, `scratch_io`,
  `outputs`, `removable`, `inventory`, `lease`, `lease::pool`, `owned_delete`
  and `volume` are the production side of the equivalence review. The test
  arms must be checked operation by operation against those statements,
  including error conversion, errno, short circuit, ownership and descriptor
  lifetime. A source map and candidate hooks are present; their complete
  equivalence and transitive coverage have not been independently established.
  The following statement map is for the actual chain, including error exits;
  links name the source containing both the non-test statement and its test
  observation. It does not assert that an unbound row meets an identity
  obligation or that a failed write succeeded.

  | Source and reached statements | Production/test relationship and outcome |
  | --- | --- |
  | [`prune.rs`](../../../src/bin/oxide/prune.rs), `tape_entry::run_chain`, `paired_store`, `require_ready`, `evaluate` | The test entry calls the real chain in that order. Authority clocks and bounded reads have one real call in each cfg arm; read error, cap, missing registry, unsettled state and evaluation result retain the real branch. `dry_run`, preview rendering and deletion entrypoints are not called. |
  | [`config.rs`](../../../src/bin/oxide/config.rs), `config_file`, `read_config`, `inspect` | Metadata, canonicalization and actual file read retain the non-test statements; test arms record the corresponding result or error before the same propagation/short circuit. |
  | [`store.rs`](../../../src/bin/oxide/store.rs), `private_file`, `open_control_directory`, `holders`, `save`, `open_admission_file`, `lock_admission_inner` | The original non-test opens, descriptor metadata, state write/sync/rename and lock statements remain in their cfg arms. Test arms record the one real result; rename failure is an attempt and successful state replacement is recorded before directory sync. The `O_CREAT` cases remain attempts when creation is not proven. Holder wall starts do not establish kernel starts. |
  | [`scratch_io.rs`](../../../src/bin/oxide/scratch_io.rs), `read_document`; [`outputs.rs`](../../../src/bin/oxide/outputs.rs), `read_receipt_with`, `marker_digest`, `read_marker`; [`removable.rs`](../../../src/bin/oxide/removable.rs), `read_registration`, `coordinate` | Test arms observe the actual open, metadata, bounded read, marker and registration results; non-test statements and their error returns remain. `coordinate` selects the paired store; it does not invoke a holding deleter. |
  | [`lease/mod.rs`](../../../src/lease/mod.rs), `canonical_existing_dir`, `read_private_file_bounded`, `fence_managed_control`, `persist_private_file`, `persist_control_state`, `random_hex` | Non-test calls retain canonicalization, bounded read, entropy, create/write/sync/rename and cleanup order. The test branch records real create/write, failed rename/cleanup attempts, successful cleanup and the replacement before the directory sync. The managed admission open is an attempt, not a claimed new lock. A failed sync is propagated even if the replacement was already recorded. |
  | [`lease/pool.rs`](../../../src/lease/pool.rs), `Pool::open_inner`, `lock_pool_inner`, `read_config`, `read_ledger`, `open_budget_payload`, `open_volume_payload`, `refuse_foreign_pool_entries`, `validate_lease_payload_entries` | Root canonicalization and listing precede the real lock and fence; config and ledger use the private-file reader. Backend probes, listing results/errors, lock open/metadata/flock and actual held metadata are taped at the corresponding operations. Unsupported backing, bad metadata, listing and lock errors keep their original returns. The non-test directory iterators and syscall statements remain, with test-only wrappers calling each original iterator once. |
  | [`lease/pool.rs`](../../../src/lease/pool.rs), `reconcile_holding`, `reconcile_evicting`, `reconcile_slot_capture`, `record_slot_capture`, `persist_revised`, `persist`, `hold_slot_tree_with_phase` | A stale or adopted holding row changes the ledger only through `persist_revised` → `persist` → `persist_private_file`; a pending eviction can record intent, restore captures, create a holding directory, rename a tree and persist a revised ledger. Create and rename results are taped where attempted; failed checks retain/block the record. Successful rename is taped before subsequent verification and sync; a containment rename is separately recorded. A prepared pool with no pending rows does not take these recovery branches. |
  | [`owned_delete.rs`](../../../src/owned_delete.rs), `observe_entry`, `open_verified_step`, `restore_root_capture`, `put_back`; [`lease/pool.rs`](../../../src/lease/pool.rs), `rollback_captures` | Recovery uses the real held-descriptor metadata and no-follow operations. Where chmod or rename has no observed path binding, the actual success/failure is an explicit unbound effect, not an invented pathname. The mode restoration guard is armed before the test observer runs. The outside-lock `delete_holding_children` and `holding_names` deleter path is not called by `open_inner` or the evaluator. |
  | [`inventory.rs`](../../../src/inventory.rs), `inventory_directory`, `inventory_with`, `Walker`, `probe_filesystem_free`; [`volume.rs`](../../../src/volume.rs), binding/open/verification | Existing root, iterator, metadata, deadline and filesystem-free results feed the same accounting and refusal branches. Test rows record returned results, not a second inventory or a recomputed policy; volume descriptor and parent fields stay unproven where the original operation supplies none. |
  | [`tape.rs`](../../../src/bin/oxide/tape.rs), [`tape_harness.rs`](../../../src/bin/oxide/tape_harness.rs), [`src/tape.rs`](../../../src/tape.rs), [`lib.rs`](../../../src/lib.rs) | The tape and harness are test-only; the library-side module is `cfg(all(test, unix))`. The re-rooted harness resolves the real modules into one sink. Production release compile-away remains an execution obligation, not a consequence inferred from this table. |

  In main's `custody_verdict`, the production `lost.unclosed.as_deref()`
  filter is assigned to `unclosed` before the same `if let Some(why)` branch.
  This reshaping preserves the one `closed_by_boot` call, short circuit and
  returned custody value; only the test arm records that actual call result.
  This is a source equivalence claim, still requiring release compilation.

  Initializer-only `prepare_new_pool`, `verify_scratch_initializer` and
  `refuse_occupied_init_directory` are reached by initialization, not by
  `Pool::open_inner`. `HoldingSweep` and its actual unlink traversal belong to
  release, not the `run_chain` call graph. A recovery rollback can still do
  descriptor-only chmod or rename, so it is not classified as a no-op.
  In the pending-eviction path, `recorded_root_is_untracked` observes each
  actually reached original/private `symlink_metadata` in its original
  short-circuit order; `require_slot_parent` observes the two device reads;
  `holding_device` observes its real metadata result before the existing
  fallback; holding moves and slot accounting record their own metadata
  results. Errors remain errors or the same original conservative fallback.
- **The entry.** `prune::tape_entry::run_chain` (cfg(test)) follows the
  `dry_run` prefix exactly — `AuthorityScope::begin`, `paired_store`, the
  optional `read_operator_list`, `require_ready`, the unfinished-intent
  refusal through `unsettled`, then `evaluate` — and exports every actual
  `Evaluation` entry (root identity, candidate or kept reason, legacy flag)
  and the completeness flag, or the actual pre-evaluate refusal with the
  engine's exit code (2 registry/fence refusal, 3 unsettled, 1 chain
  error). It never calls `dry_run`, never renders a preview, never
  redirects stdout. The static source-site inventory is the list above;
  the descriptor-bound read sites and their failure paths still require
  source and execution review. An unbound or unknown observation is never
  parity, and over-cap occurrences explicitly mark truncation.
- **The harness.** Argv contract exactly
  `--fixture-root --manifest --transcript-out --case`; exit 0 when the
  transcript is written (any engine outcome), 2 on harness failure. No
  arguments (the `oxide test` route) runs the embedded unit fixture end to
  end: the pure tape checks (deterministic field order, epoch and
  occurrence bookkeeping, both caps' explicit truncation, the unbound
  authority-read note, effect/clock/classification rows) plus the BASE,
  EFFECT-1 (absent admission lock), EFFECT-2 (missing accounting fences),
  REGISTRY-MISSING and UNSETTLED-INTENT unit fixtures, each a fresh
  exclusively owned fixture and one engine child (this executable
  re-invoked with a scrubbed environment: private HOME plus
  `OXIDE_TEST_HOME_FROM_ENV=1`, private `OXIDE_CONFIG`, `OXIDE_POOL`,
  `OXIDE_POOL_CAP`, `OXIDE_SCRATCH_ROOT` only when the fixture sits under
  `/Volumes`), every child waited and reaped. `--self-check` runs only the
  pure checks.
- **Honest fixture limits.** The engine's `Binding::open` accepts exactly
  `/Volumes/<name>`, so an external root binds only when the fixture's own
  `oxide-outputs` sits at a dedicated mount root. On a shared volume's
  scratch (or the internal fallback) the embedded BASE is expected to record
  an incomplete batch with every root kept and the engine's own reason; the
  strong BASE surface (present checkout kept, positively absent checkout's
  root an actual candidate, unlisted legacy root kept, complete snapshot)
  requires a fixture that is its own attached proof volume. A prepared fixture
  carries the four admission fences, `outputs_init` ready, the pool lock and
  the admission lock. BASE expects no confirmed effect and an empty fixture
  diff, but the real pool-lock `O_CREAT` open remains an uncertain attempt,
  so its transcript is insufficient for parity. The historical EFFECT names do not promise writes: EFFECT-1 requires the
  specific missing-lock custody refusal and an incomplete whole-batch keep;
  EFFECT-2 retains its declared completed transient create/save and persistent
  fence-save assertions, plus the surviving state diff and unchanged existing
  lock. `PoolWitness::take_admission` opens only an existing lock and `read_state`
  is read-only, so there is no positive lock-create coverage on that custody
  route. However, earlier `Store::open_at` calls library `ledger_view`, whose
  locked `Pool::open_inner` reaches `fence_managed_control`: EFFECT-2's old
  state triggers the real managed fence write before custody. Pure effect
  bookkeeping/reconciliation controls and all completeness/parity limits stay.
  `--diagnostic` keeps going through all five embedded cases and the
  disconnected BASE control, reports each result, retains its fixture root,
  and always exits nonzero with `qualified=false`; the no-argument suite
  remains fail-fast. The fixed `proof:tape-prune:embedded-diagnostic` selector
  runs only that harness under the qualified guard at 600 s / 512 MiB.
- **Validation and export.** The final executable and its build route must be
  identified from the reviewed source and recorded with its digest, lock and
  toolchain before X-S2 consumes it. The final head still needs its covering
  compile, release compile, format, Rust suite, docs and focused selectors.
  No earlier fixture run supplies that evidence. The fixed nine-premise
  checklist remains mandatory for future execution: private HOME/rustup,
  account control, environment scrubbing, no unintended adoption, Cargo argv,
  image custody if applicable, frozen compiler flags, temporary placement and
  explicit macOS mktemp templates. These are conditions to verify, not proof
  that the current apparatus satisfies them.

Deliverable note for O3: an `EXPLAIN-OR-BLOCK` case that demonstrates a real
final disagreement (currently `REF-ORPHAN-MARKED`, `RCP-V1`,
`WRT-RETIRED-CLEAN`) is not folded into a match. Each is either explained from
current source in the comparison report or recorded as blocking, and O3 stays
held while any blocking row stands. `D-1`/`D-2` (Q-E) and `D-3` (Q-D) keep
their existing operator decision points.

## Shared private-build validation gate (oxide#248)

The capture tasks' private debug-build validation shares one common
canonical provider in this repository: `scripts/test-private-build-validation.py`
(oxide#248, project-owner decision `a451b488`). It owns the common admission
prerequisites for Oxide #240/PR230 and Barge #041/PR41 so neither product
worker re-implements the gate. The capture Rust source itself is unchanged;
this apparatus authorizes nothing by existing and grants no cleanup, deletion
or deployment authority.

**Artifact interface.** The provider prepares and validates exactly the
runtime documents the two reviewed wrappers consume — no new competing
schema:

- `barge.validation-source-audit/1`, `barge.private-roots/1` and
  `barge.validation-block/1` (the accepted Barge launcher shapes, pinned to
  the source-approved cap revision at
  `60c2c793e0743ebb389dd433cec53251d00a0b87`, retained review run
  `9c36470a-54ab-44a8-9e15-28ce1789e9c4`), built from private source
  copies staged under the prepared task root, with component analyses bound
  to the audited source sites and host executable digests measured live;
- provider-emitted Oxide compatibility proposals: `budget.json`,
  `history-discovery.json` and `block-authorization.json`. These remain
  unissued inputs owned by the provider; the removed local launcher does not
  consume or independently approve their shape;
- one gate receipt, `oxide.private-build-validation/1`, whose qualification
  state is always `unqualified` — receipt judgment is retained Queue review,
  never this tool.

Documents are deterministic (sorted keys, compact separators, trailing
newline, no timestamps, no live sizes). Barge discovery preserves individually
attributed loose historical logs as exact file entries, never their shared
scratch parent. A file entry is opened without following links and its owner,
identity, metadata and content are observed in each retained snapshot; a
missing, unreadable or replaced file refuses. Neither the snapshot nor its
allocation count establishes historical peak or continuous quiescence.

For preparation the planner supplies `OXIDE_CORRECTED_DISCOVERY_PACKET` as
the canonical path, under Oxide scratch, of the exact compact JSON bytes from
PR 230 comment 6583995069 (SHA-256
`1c60e2374b76e9004ac99f27bcb0297623cabddb6d2422e29cbc26cd040bb54a`,
no trailing newline). The provider checks the digest and consumes every
attributed candidate, including the six task roots, individual evidence files
and attributed review/planner directories; it also accounts for the packet,
review export and actual audit-input files. The review 6584174560 and packet
retain unknown mixed attribution and other gaps; unresolved candidates are
counted conservatively without asserting ownership. The proposal remains
`complete: false` and does not authorize execution. No unrelated scratch root
or shared pool is swept, and no prior point-in-time size is a present peak.

**Execution model.** Preparation and the diagnostic use separate Barge
phase roots, with the qualified supervisor adopting and sampling the entire
active root, including its evidence and exports. The diagnostic verifies a
closed preparation receipt and charges its wall time and a conservative 120 s
pure-check reservation against both prospective blocks. **Source-bound
cross-checkout preparation** (operator decision `af038a7e` v2; planner
relative-path correction): the authorized continuation prepares at one Queue
checkout and executes at the eventual merged-source proof-only checkout, so
the diagnostic no longer compares script paths — the recorded preparation
path may be relative (`scripts/test-private-build-validation.py`), and a
path comparison could pass in the diagnostic's own cwd without any byte
evidence, or error on an absolute path at a checkout that no longer exists.
Preparation instead writes a deterministic `source-binding.json`
(`oxide.private-build-validation.source-binding/1`) beside the packet
manifest, positively binding the whole guard-recorded command verbatim, the
preparation script's actual SHA-256 (computed in the preparation process,
where its recorded path is meaningful), the qualified supervisor bytes
(`fcfda2b0…`) and the packet manifest digest; it prints that binding's
digest for the planner to bind. The diagnostic requires the planner-bound
`--expect-source-binding` digest, re-reads the binding and the manifest from
the retained packet (whose directory it locates only from the recorded
command, never by resolving the recorded script path), verifies its own
running script bytes against the bound digest, re-checks every manifest
binding (run identity, packet, roots, heads, audit-input digest, unissued
state and each named packet document digest), and refuses a changed script
or guard, an edited command token, mismatched heads/roots/options, a
malformed or missing binding or manifest, an edited manifest or packet
document, or an unclosed preparation. Identical reviewed bytes at a
different checkout are accepted; an absolute basename, an environment
assertion or a removed check would not be a source binding. It consumes each
planner-issued companion token once, under the relevant task root's guard;
the exclusive persistent claim is retained even after refusal. A direct
entry requires a matching live supervisor parent, command and receipt, not
just exported environment variables. Parent-command and receipt mismatches name
the failed predicate without printing the command or environment. The process
reader compares the parent and child interpreter images using the kernel
for both. It treats the kernel size probe as an allocation estimate, retries
a bounded `ENOMEM`, and requires one exact matching returned row; errors and
returned sizes are bounded refusal
diagnostics, never evidence that an unknown process is absent. Root discovery
and block proposals are unissued; a planner must resolve their unknowns and bind actual inputs before
execution. A closed runner receipt and matching live inventories still require
independent judgment; the diagnostic itself never grants qualification.

The Barge bootstrap builds pinned Oxide `3de2413`; the independent Oxide
product head is a separate input bound to the Oxide block. The global
allowance includes setup, bootstrap, status and exports. The diagnostic
brackets the retained inactive roots and account inventory, with inode and
content equality; uncertainty refuses. The supervisor samples scratch usage,
so a sample maximum is evidence, not an instantaneous peak measurement.

**Host admission.** `EFFIGY_ADMISSION_HOOK` and `QUEUE_ADMISSION_SOCKET` are
preserved exactly as the independent audit binds them; an unset hook is not
host admission. The audited Effigy must be the direct executable, never an
unbound shebang shim. The bootstrap's host admission is the gate task's own
heavy Effigy admission through the audited hook, with the audited pair
preserved in every child environment; no second admission seam exists and no
Git-verified source archive is ever edited.

The explicit audit manifest accepts at most 128 retained-root names; the earlier
64-name limit is historical. Each name still passes the existing containment,
identity and metadata checks, and per-walk entry, depth and deadline bounds are
unchanged. No retained root may be omitted to fit this name budget.

The current prospective private-validation ceiling is **5 GiB for Barge** and
9 GiB for Oxide. The preceding 3 GiB and 4 GiB Barge / 8 GiB Oxide grants
and their packets remain historical. Each run still clamps its budget to the
aggregate ceiling minus retained bytes; the private pool remains 256 MiB and the native guard is at
most 512 MiB. The increase adds no cleanup or runtime authorization and does
not repin Barge's retained fact-only component.

**Resource clamps (operator decision `af038a7e` v2; review 6584174560).**
Whole-block scratch is a measured clamp of the applicable aggregate grant
minus retained bytes. Decision `af038a7e` v2 granted Barge 3 GiB and Oxide
8 GiB; the subsequent 4 GiB Barge / 8 GiB Oxide grant is historical. The
current prospective ceilings are 5 GiB Barge / 9 GiB Oxide, as stated above.
These grants do not renew or raise any earlier session: the historical
2 GiB / 6 GiB runs and their evidence (Oxide's reported 5591.8 MiB retained
total, about 552 MiB effective against the 6 GiB grant) keep their original
bounds and stand as historical measurement only. The effective figure is
measured at runtime, disclosed in the receipt, never silently raised, and
never a claim that a phase fits: an impossible fit is an exact measured
shortfall and refuses.

**Selectors.** `proof:private-build-validation:test` (pure mocked
counterfactuals covering every gate/refusal control: missing/unissued/duplicate
block, exhausted time/disk, cross-process id/deadline mismatch, wrong
source/digest/child env, absent admission hook/socket/runtime binding,
unknown/incomplete/non-quiescent roots, empty/malformed inventory,
same-content inode replacement, failed closure, real-tree accounting and
clamp arithmetic (historical and prospective grants), exact/over-cap
acceptance and refusal, packet/schema conformance, supervisor-context
binding, the cross-checkout source binding: identical reviewed bytes at
a different checkout accepted; changed script/guard bytes, an edited command
or manifest or packet document, mismatched heads/roots/options and a
missing or malformed binding refused; and bootstrap provenance (oxide#253):
a successful bootstrap packet survives a later real inventory refusal;
missing, malformed and escaped artifacts refuse with the packet retained;
ambient or planted Cargo configuration without a private content binding
refuses before launch);
`proof:private-build-validation:plan` (effect-free);
`proof:private-build-validation:diagnostic` (heavy, whole command under the
qualified supervisor). The diagnostic declaration carries no issued binding
and refuses before any effect until the planner amends the command with the
bound inputs. The exact command template the planner binds at issue:

    OXIDE_TASK_SCRATCH_ROOT=<guarded-barge-root> python3 scripts/bounded-test.py --limit 1200 --scratch-budget-mib <reviewed-mib-at-most-512> -- \
      python3 scripts/test-private-build-validation.py --diagnostic \
      --oxide-repo <canonical oxide checkout> --oxide-head 3de241325dd286ae44a5759fa6825b05a81c7b4e \
      --oxide-product-head <40-hex independently approved Oxide product head> \
      --barge-repo <canonical barge checkout> --barge-head <40-hex> \
      --barge-root /Volumes/Builds/scratch/barge/<prepared root> \
      --oxide-root /Volumes/Builds/scratch/oxide/<prepared root> \
      --expect-audit <sha256> --expect-ledger <sha256> \
      --expect-budget <sha256> --expect-history <sha256> --expect-oxide-binding <sha256> \
      --preparation-receipt <closed guard receipt> --expect-preparation <sha256> \
      --expect-source-binding <sha256> \
      [--planner-reference <string>]

Preparation is a separate guarded document phase with a fresh adopted Barge
root. The planner binds `--prepare --packet-dir <path inside preparation root>`
plus the same repository, head and planned task-root arguments shown above.
The Oxide task root must already exist before preparation. Retain the audit
input manifest through diagnostic execution. Its `--audit-inputs <manifest>
--expect-audit-inputs <sha256>` names bounded
source copies, each with `source`, private relative `destination` and `sha256`;
the diagnostic recopies and rechecks those same bytes from the retained phase.
Each audit component source is bounded to 64 MiB for both construction and
validation, matching the per-file staging limit; documents retain their 8 MiB limit.
Admission executables and runtime files use the 256 MiB host-artifact digest
bound and must still match their canonical paths and audited digests.
An inventory mismatch still refuses isolation. Its bounded refusal identifies
the phase, total changed rows and up to eight hashed paths with change kinds;
it does not expose inventory values or establish which actor made a change.
Comparison keys rows by unique path, independent of traversal order. On a
mismatch the guarded private evidence directory retains the actual before and
after rows and difference summary, capped at 8 MiB; excess refuses without
claiming that the evidence was saved. Duplicate paths also refuse.
The same digest-bound manifest requires `retained_roots`, a nonempty list of
at most 64 canonical, owned, non-group-writable task-private directories under
the Barge or Oxide scratch subtree. It must name entire prior preparation and
future-runtime roots, including partial archives, not just receipt files.
The guard validates the list before any tree walk or packet write and counts
the named shared history against both grants. Before copying, it emits one
bounded stdout observation of the Barge and Oxide retained bytes, effective
preparation clamp and chosen guard budget; those are current observations,
not a historical peak. Bases, aliases, escapes, missing roots and
active/future-root overlaps refuse. The planner still verifies
attribution, completeness and quiescence; a list alone does not establish them.
It preserves that phase's receipt and packet, resolves every unknown, reviews
the source audit and then issues the documents and companion tokens. The
preparation manifest binds each unissued document digest; audit component
entries name the source pairs rather than their accompanying analysis text. The
preparation root is retained and counted before the diagnostic. All command
and environment bindings are fixed before dispatch; changing the source needs
new source review. The candidate diagnostic binds its qualified outer guard to the separately
issued remaining allocation after the preparation charge: the actual guard
limit must exceed 30 seconds and may not exceed either issued authority. The
qualified guard serializes its limit as a JSON number such as `990.0`; the gate
accepts a finite integral float as that exact integer without changing the receipt.
Its effective deadline is the earliest of the guard deadline and the issued wall
bounds; bounded source staging, input walks and final inventories use it. Each
nested guard samples the remaining time again immediately before launch and
refuses at 30 seconds or less. Historical 1200-second packets retain their
original limits; this check does not create a new allowance.

The displayed diagnostic scratch budget is a planner-bound
clamp, and the repository selector's 512 MiB default refuses if it does not
fit the measured remainders.

with the issued documents and their digests in the environment
(`BARGE_VALIDATION_BLOCK[+_SHA256]`, `BARGE_VALIDATION_SOURCE_AUDIT[+_SHA256]`,
`BARGE_VALIDATION_ROOT_LEDGER[+_SHA256]`, `BARGE_VALIDATION_EFFIGY`,
`BARGE_VALIDATION_GIT`, `BARGE_VALIDATION_SUPERVISOR`, `BARGE_PINNED_RUSTC`,
`TAPE_BLOCK_APPROVAL_SHA256`, `EFFIGY_ADMISSION_HOOK`,
`QUEUE_ADMISSION_SOCKET`, `CARGO_HOME`, `RUSTUP_TOOLCHAIN`, and optionally
`RUSTUP_HOME`). The Barge phase root must be the outer guard's adopted scratch
root, including its evidence; the short Oxide claim phase's guard adopts the
Oxide root. The planner binds the appropriate measured
whole-root caps and includes the preparation root in the retained ledger.
Before the diagnostic the planner creates the canonical owner-only Oxide root
and places `budget.json`, `history-discovery.json`, and the issued
`block-authorization.json` directly there; the last has an owner-only
`block-authorization.json.token` companion. In the guarded Barge root the
planner places the reviewed source-audit template, the root ledger, and the
issued Barge block template, plus its `<block-name>.token` companion. Each
token contains its block-template digest and a newline. Both the Barge block
template's `oxide_sha256` and `audit_sha256` and the audit template's
`oxide_sha256` must be JSON null. Its `authorization_reference` starts with
`derived-artifact-v1:` followed by the external issue reference: this
explicitly authorizes the provider to fill only those fields from the one
supervised bootstrap and the resulting canonical audit digest. The provider
writes the resulting complete wrapper-compatible audit and block to the Barge
root's `issued/` directory, records the template and derived digests, and the
wrapper receives the derived bytes and digests. No second build, issuer or
pre-run binary digest is assumed. The planner reviews the template and this
specific derivation before issuing; the provider cannot extend its bounds.
A fixed exclusive hard-link claim burns each token across runs; a partial
claim is never retried. `CARGO_HOME` identifies
the invoking offline seed; `RUSTUP_HOME` falls back to the invoking
`HOME/.rustup` before HOME is replaced, and `RUSTUP_TOOLCHAIN` is pinned.
The audited admission hook, socket, direct Effigy, Git, supervisor, rustc and
child PATH must all match the issued audit. The planner supplies and reviews
source component copies, host provenance, the complete root ledger and both
issued blocks; proposed documents alone are not an admission packet.
`planner_reference` must be confirmed by the planner at binding (the
proposal carries the brief-v11 string; the held brief is now v12).

**Status and the approved continuation (oxide#249; operator decision
`af038a7e` v2, 2026-11-02).** The gate's source landed source-only as
#248/PR234 (merged `1854c581`); it grants no qualification. The first
proofExecution, Queue execution `0feea5af` at `67eea662`, failed after its
debug bootstrap: the guard closed with exit 2, no fault or deadline, and a
certain empty census, on the post-bootstrap live-inventory bracket (receipt
`/Volumes/Builds/scratch/barge/248-retained-runtime.pz0dqxo7/supervisor-evidence.09j75k94/receipt.json`,
SHA-256 `9dee671b093f1e3a2f9cabe6f48c5f45abf5b5ac31cb8c02377da7cb24c976e0`);
the single original bootstrap grant is spent, the compared inventories were
not persisted, and no qualification or acceptance is claimed. That failure
packet and the earlier preparation packet (`248-retained-prepare.i0okptqk`,
whose receipt names a relative script path) are historical evidence: old
packets, receipts and manifests are untouched and carry no binding under the
source-bound route, and consumed claims are never re-opened. The ruling
approves ONE further shared Queue diagnostic and at most ONE additional
300 s private debug bootstrap, inside the SAME at-most-749 s remaining to
each block (1200 s minus 451 s already charged, including the failed run's
58 s). The Barge cap revision's safe checks deduct a further 4 s,
conservatively charged to both blocks, leaving at most 745 s before the
continuation's own checks and preparation, with the prospective 3 GiB / 8 GiB
caps above. Preparation for that
continuation may run at one Queue checkout and the diagnostic at the
eventual merged-source proof-only checkout under the source-bound
`--expect-source-binding` route. No new clock, reset, refund, cleanup,
install, production redirect, live shared-pool/output read or additional
runtime is granted; any inode/content delta, unknown custody or exceeded
bound still refuses, and the retained reviewer judges the actual receipt
before the planner releases the gate. Both capture tasks remain manually
held pending the actual new proof-only qualification and its required
covering evidence; merged source-only #248 and the failed native `0feea5af`
are distinct facts and neither is a qualification.

**Bootstrap provenance and the spent replacement diagnostic (oxide#253).**
The #249 continuation ran once as shared-gate task `7eeabf3a` / executor
`912ec96f` at source `c0255171`. Full `CLOCK_UPTIME_RAW` 156.045200042 s,
charged 157 s, remaining 370 s per block. Outer guard closed, status 2, no
fault or deadline, certain empty census. Nested debug bootstrap closed,
status 0. The compared inventories were persisted this time
(`/Volumes/Builds/scratch/barge/replacement-runtime.0xj2w99b/supervisor-evidence.1g94y4v2/live-inventory-bootstrap.json`,
SHA-256 `1c88b191eaf52a9f2363ac4c3021785b75a45beadba1cb1d89f1736a92b5eed3`;
guard receipt SHA-256
`08a7c12435ff7d4598253c38d48a54395981928fa7c541e376b949d05137ac4a`).
`./state/pools.json` inode `2074628791` → `2074629593` with equal content
hash `c76baa06454558963eb87c05b13274d41f465eb21c4710d13c11f8b25637f493`,
plus the parent `./state` mtime change. That proves the isolation bracket
failed. It does not identify a writer: overlap, current private-home
emptiness, and source-only absence arguments do not name an actor. Planner
analysis
`/Volumes/Builds/scratch/oxide/planner-041-240-owner-disposition-20261102/replacement-diagnostic-issue.r6p9orys/planner-isolation-failure.json`
(SHA-256 `45974f1bde2346fb51c448cc79d33e7ea9fb5fe0332484ff6f3d4d41413a84f7`)
records the same limit. Operator `2df45ce0`'s one replacement diagnostic
and one additional bootstrap are spent. No further native, bootstrap,
preparation, status or inventory run is authorized from this gate.

Recorded during that run, not reconstructed: argv was the fixed
`cargo build --offline --locked --bin oxide --message-format=json`; the
nested receipt named that command, limit 299 s, root
`/Volumes/Builds/scratch/barge/replacement-runtime.0xj2w99b`; stderr
recorded cwd as that root's `oxide-source` (`Finished dev` for
`oxide-storage`); the JSON log named exactly one `oxide` bin artifact at
`oxide-source/target/debug/oxide` whose `src_path` was
`oxide-source/src/bin/oxide/main.rs`; that file's digest is
`c6137a1a1fe1027dd2e90e9c5ee3bd95f2db0472b8edb3bfb4c04f0af29256e1`
(20513160 bytes); `bin/oxide` is a symlink to it. The artifact is evidence
only: it is not executed, relocated, installed or qualified by existing.

Reconstructed from the retained tree and current source, and labelled as
such: `bin/cargo` and `bin/effigy` are symlinks to the invoking rustup
1.97.1 cargo and the audited Effigy; private `HOME` is empty;
`$CARGO_HOME/config.toml` and every `.cargo/config.toml` on the cwd
ancestor chain to `/` are absent; the offline seed populated
`cache/registry` and `cache/git` from the invoking CARGO_HOME; `RUSTC` is
the pinned 1.97.1 rustc. Child PATH is private `bin` plus the inherited
suffix. Named linker and assembler tools (`cc`, `c++`, `clang`, `gcc`,
`ld`, `as`, `ar`, `ranlib`) whose first usable PATH match is not a
private binding use the existing pre-launch refusal. A private binding
is an executable regular file inside the private root, or a symlink
whose resolved target is an executable regular file inside the private
root. A dangling symlink, a non-executable file, and a symlink to a
host `cc` are not private bindings: execvp continues or runs the host
tool, and the scan refuses the ambient match. An unreadable Cargo
config path is a violation, not absence. Proc-macro and build-script
crates compile to `target/` under the private root and inherit the same
child PATH; `rustc` itself is bound. Unnamed further host spawns those
crates may perform cannot be bound without executing the bootstrap, so
they are not invented as privately proven. The scrubbed child
environment (`HOME`, `CARGO_HOME`, `TMPDIR`, `OXIDE_CONFIG`,
`OXIDE_POOL`, and the rest of the allowlist) redirects processes that
honor those variables. It does not confine a build-script, proc-macro,
linker, or other child that opens an absolute path such as
`~/Library/Application Support/oxide`. The spent run used the inherited
PATH suffix and closed status 0; that is historical reconstruction, not
a forward authorization to skip the refusal. Registry resolution is the
seeded private cache (offline, locked). Account-home for live inventory
is `~/Library/Application Support/oxide`, independent of the scrubbed
`HOME`; `OXIDE_TEST_HOME_FROM_ENV=1` is on the product child allowlist.
A cargo build of `--bin oxide` is not itself a product invocation.

Unexcluded candidate writer classes for the `pools.json` inode
replacement (none is identified as the writer): cargo and rustc (bound
host binaries that can open any path); build-scripts executed by cargo;
proc-macro crates executed by rustc; PATH-resolved linker and assembler
tools if they ran; the nested supervisor, python, and the gate process;
concurrent host processes overlapping the live account; macOS services
touching `~/Library/Application Support/oxide`. The built `oxide`
binary is not a candidate: it was not executed.

The source gap this task closes: the spent diagnostic retained the
inventory mismatch and the nested receipt, but dropped the attributable
bootstrap inputs (allowlist env, cwd, argv, tool identities, source+lock
bindings, ambient Cargo scan, guard receipt reference, artifact
path/digest) before the isolation refusal. The existing bootstrap call site
now writes `evidence/bootstrap-provenance.json` inside the monitored root
after a successful nested closure and before `verify_inventory`, and on
pre-launch ambient/planted Cargo-config refusal or artifact
missing/malformed/escaped/ambiguous findings. The packet is internal
bounded evidence (`bootstrap-provenance`, 256 KiB cap), not a public schema
or wrapper hook. It records the audited allowlist values actually passed
and inherited key names; it never records inherited secret values. An
ambient or planted Cargo configuration, a `$CARGO_HOME/config.toml`
outside the private root, an unreadable config path, or a PATH-resolved
named linker/assembler whose first usable match is not an executable
regular file inside the private root, still uses the existing bounded
refusal.

Next minimal evidence requirement: an operator-authorized runtime whose
private `bin` binds the named linker and assembler tools (or a recorded
operator exception for a specific host tool), then a nested bootstrap
closure with the provenance packet retained, then the isolation bracket.
The writer of `pools.json` remains unknown until that packet plus the
nested log and inventories name one. The provenance packet lets that
route assess the bootstrap path when isolation refuses; it does not
identify a writer, lower the oracle, repair the baseline, skip bootstrap
or weaken one-use claims. Honest missing runtime evidence at this head
remains missing and blocks qualification.

**Candidate-input qualification route (oxide#254; operator decision
`d9aec625-ecc1-4dcf-a0be-737f85d9eca0` v2, 2026-11-03, "Approve proposed
validation route").** The same provider gains a `--candidate` mode that
binds the RETAINED debug artifact instead of building one. The planner
binds every input at dispatch: the origin root
(`/Volumes/Builds/scratch/barge/replacement-runtime.0xj2w99b`), the
artifact digest `c6137a1a1fe1027dd2e90e9c5ee3bd95f2db0472b8edb3bfb4c04f0af29256e1`
and its 20513160 bytes, the retained nested guard receipt
(`evidence/bootstrap/receipt.json`, command-bound, closed, status 0,
certain empty census, scratch naming that root) and build log digests, the
retained `Cargo.lock`/`rust-toolchain.toml` digests (which must equal the
pinned `3de2413` commit's own bytes, so the staged source reproduces the
build source), and the audited host-tools document. Origin, bindings and
retained nested closure are validated BEFORE any staging; the artifact is
then copied only inside the fresh guard-owned task root, rehashed, and
symlinked as `bin/oxide`. Nothing under the origin is written; there is no
automatic rebuild, install, cleanup or repair. The ruling explicitly
allows the pinned Rust 1.97.1 toolchain and the named Apple
linker/assembler tools (`cc`, `c++`, `clang`, `gcc`, `ld`, `as`, `ar`,
`ranlib`) as audited HOST inputs for this route: the planner-bound
host-tools document names each resolved canonical path plus its live
content hash and size; absent stays absent; a changed, unlisted or
documented-but-not-live binding refuses; a private binding stays allowed.
They are not private bindings and the hashes do not confine their writes;
ambient Cargo configuration refusals, the explicit environment allowlist
and the pre-launch packet-retained refusal (oxide#253 rule, now
`evidence/candidate-provenance.json`) are unchanged. No other inherited
tool, environment or config trust exception follows. The route takes its
own fresh live-inventory baseline around the private status gate and the
retained-root quiescence snapshot; any inode/content delta, ambiguous
closure, missing root accounting or cap breach refuses, including
identical-content inode replacement. The origin session's failed
isolation stays historical and unrepaired; a fresh baseline never erases
it, and unrelated host activity is not presumed to excuse a delta. The
wrapper-consumable issued documents and the debug receipt are NOT derived
in this route: the accepted wrapper binds a debug receipt to an in-root
cargo build (fixed argv, supervisor receipt naming the private root),
which a retained candidate cannot honestly fill, so capture-side handoff
integration stays on the capture tasks' own route and both modes'
fail-closed controls stay intact. The gate receipt stays honest
(`qualification` always unqualified; retained review judges). Final
capture wire/schema limitations, X-S2/O3, production pins/features and
live-data restrictions are unchanged. Prospective accounting under
`d9aec625` v2: ONE cumulative 1200-second continuation charged to BOTH
capture blocks. Earlier provider and owner evidence reservations total 30 s;
two consumer source-check reservations add 20 s, leaving at most 1150 s
unallocated before actual preparation/runtime. These are conservative
reservations, not measured runtime; the earlier 309-second ledger and every
historical charge remain with no reset or refund. Prospective
aggregate scratch caps for that ruling were Barge 3 GiB and Oxide 8 GiB and
refused when retained bytes left no space. The later Barge 4 GiB / Oxide 8 GiB
correction and current 5 GiB / 9 GiB ceilings are recorded above and do not
rewrite those historical packets. At most ONE Queue-routed candidate isolation qualification and then at most ONE safe covering batch per
capture task at its exact approved head, each with the qualified guard
(`fcfda2b0…` unchanged) and fresh one-use claims; no automatic retry,
native image or bootstrap. Source approval grants no native credit. The
selector `proof:private-build-validation:candidate` (heavy) carries the
fixed unbound declaration and refuses before any effect until the planner
amends the command with every bound argument.

The shared provider accepts `OXIDE_PRIVATE_BUILD_PROGRESS=1` for bounded
stage begin/end timestamps on its existing stderr during preparation and the
candidate diagnostic. These fixed-name observations identify the last entered
operation; they do not grant admission, alter deadlines or establish a pass.
With the switch unset the provider emits no progress records.

**Capture handoff.** The obsolete #240 local sequence and admission schemas
are removed. `scripts/private_build_candidate.py` is the source-owned
candidate consumer: Barge may stage and hash this exact component alongside
`scripts/test-private-build-validation.py` and invoke
`verify_candidate_facts(gate, candidate, qualification, digests, product_head,
product_tree)`. It returns the producer's remaining seconds or raises the
provider's `GateRefusal`; the caller must first read and hash the actual
documents and must independently enforce its own fixed selector contract,
guard, claim, paths and limits. Oxide's `verify_documents` and `consume`
add its fixed selectors and guarded execution; Barge must not copy that fact
policy or treat the fact function alone as admission. In particular, Barge's
`candidate_runtime` caller must pass the producer gate, candidate and separate
qualification documents, the actual byte digests (`gate`, `candidate`,
`guard`, `case_guard`, `case_log`, `live_before`, `live_after`) and Barge's
bound product head/tree. It must replace its expected `verify_candidate`
boolean-result interface with this integer-returning fact call; its existing
guard, single-use claim, source/tool verification, offline dependency setup,
live inventory and Barge selector/cap checks remain independently required.
No positive execution or admission field may be synthesized from the returned
number. The component imports the shared provider from its own directory, so
both staged source files must be independently hash-bound. The earlier reviewed Barge
fact-only pair remains bound to its original hashes; the Oxide deadline-aware
provider change does not migrate or requalify that pair. The thin tape entry accepts only
`--input <path> --sha256 <planner-digest>` under the shared supervisor. The
input's `paths` and `digests` bind the producer gate and candidate receipts,
outer and status-case guard receipts, both live inventories, independent
qualification and fresh issuance. The input also names the product checkout
and exact head. Qualification is a separate planner-bound positive judgment
over those artifacts, product head/tree and retained roots; the producer's
candidate receipt remains `unqualified` with `build.state = not-built`.
Issuance binds that judgment, exact selectors, candidate, component/provider
and tool digests, private roots, limits and a fresh one-use claim. Missing,
changed, expired, unguarded or spent inputs refuse before product execution.

The retained private debug front door was compiled from commit
`3de241325dd286ae44a5759fa6825b05a81c7b4e`; the compile and test selectors
operate on the separately bound, staged PR head and tree. The candidate remains
unqualified until its actual retained qualification. Its identity is not the
installed release binary. Host-tool hashes bind executable identity, not a
filesystem confinement guarantee.

The 256 batching change updates preview selection and real-run authorization,
which lie beyond the capture harness prefix. It does not change the reached
`evaluate` source-site or cfg map. Its production and test changes remain part
of the exact source to be covered by the later checks.

For a capture run the planner supplies `TAPE_CAPTURE_LIMIT_SECONDS` before
Effigy admission. A missing limit refuses. The consumer uses the minimum of
the live guard deadline, the independently issued expiry and the invocation
start plus its maximum. Git subprocesses use the remaining deadline as their
timeout and archive members are checked against it. The allowance must leave
at least 32 seconds before the claim and 31 seconds before each nested guard;
a spent issuance refuses. Admission waits do not rewrite an expiry or resurrect
a claim. A post-claim refusal burns
the one-use issuance. The guard must adopt an owner-only, canonical root below
`/Volumes/Builds/scratch/oxide-validation`; the Rust suite temporary directory
is that root's `capture-covering/tmp`. Its separately approved internal fixture
root is still used by the nested suite guard. These bounds do not confine host
tools' filesystem writes.

After the existing provider's guard and exclusive claim, the consumer stages
the exact product Git tree and uses the existing offline seeder on its bound
lock into a private Cargo home. It runs the four named covering selectors
under the shared supervisor, preserving the approved suite's validation-temp
and internal roots and bracketing the work with live and retained inventories.
Equal retained snapshots are not continuous-peak proof. Host-tool hashes bind
identity, not writes. `proof:tape-prune:cover` is the guarded entry; the
unbound declaration refuses. This new source component and the producer's
separate case/guard outputs require exact-head retained review, a real positive
qualification and planner-issued inputs before use. The shared diagnostic #252
failed its live registry-identity gate; #253 independently owns source and
provenance audit. Neither source integration nor the candidate receipt grants
runtime, native, parity or merge credit.

**Premise dispositions (HIGH, proof-apparatus authoring).** (1) Private
HOME/CARGO_HOME with the invoking `RUSTUP_HOME` captured before replacement,
pinned 1.97.1, `RUSTUP_AUTO_INSTALL=0`, offline lock from the extracted
archive, seeded by the shared bounded seeder from the invoking CARGO_HOME —
never a download. (2) `OXIDE_TEST_HOME_FROM_ENV=1` on every product child.
(3) The child environment is an explicit allowlist that drops inherited
`OXIDE_*`, `XDG_*`, `CARGO_*`, `SCCACHE_*` values and passes only the audited
admission pair plus the declared controls. (4)–(5) No automatic adoption and
no Cargo external argv are involved. (6) No image, attach or detach. (7)
`RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` are not inherited (the allowlist
drops them; the bootstrap is a plain offline debug build). (8) The gate's own
scratch lives under the supervisor's adopted root; task roots live under the
task scratch bases with the gate's own accounting. (9) Fixture temp in the
self-test uses a `mktemp` template under `OXIDE_TASK_SCRATCH_ROOT` or the
temp directory, resolved canonically.

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
- **Q-E. Parity D-1 and D-2.** Does the demonstrated inode-drift batch-versus-root difference warrant acceptance?
  Stale device-only drift alone is reconciled by current Oxide; a live remount
  needs observed evidence before any D-1 difference can be ruled on.
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

The embedded harness derives its allowed scratch namespace from the qualified
launcher's TMPDIR: `<selected-volume>/scratch/oxide` or `oxide-validation`.
It rechecks canonical paths, directory ownership, non-writable shared ancestors,
non-boot placement and private fixture components; engine children receive only
that TMPDIR plus their fixture configuration. A namespace lookalike or escape
is refused. This replaces the historical host-specific `/Volumes/Builds` gate.
Planner ruling c4de881a (2026-11-04) permits this test-only correction and one
replacement 600 s / 512 MiB uncounted diagnostic; the failed pre-case diagnostic
remains charged and retained, and the counted suite stays gated on the corrected
diagnostic's case results.

Planner ruling 0f894fd8 (2026-11-04) permits forwarding TMPDIR in the otherwise
unchanged child environment allowlist and one further 600 s / 512 MiB diagnostic.
Diagnostic fixtures live under the existing guard evidence directory, preserving
containment and guard cleanup/custody. Both earlier failed sessions stay charged;
their logs/receipts remain, but their scratch was removed after clean closure.

Planner ruling 7f28a33a (2026-11-04) permits restoring the original EFFECT-2
positive-effect checks after the retained diagnostic exposed its real pool-open
fence write, while keeping EFFECT-1's specific refusal. One additional bounded
uncounted diagnostic is authorized; all earlier runs remain charged. Production
and guard behavior are unchanged; the counted suite still follows a clean
all-case diagnostic.

The retained embedded diagnostic at `9807864b` exercised all five cases and the
disconnected BASE control with no case failures; its exit 1 and `qualified=false`
are intentional and give no qualification/parity credit. The following Rust
changes only wrap those same expressions. The ordinary suite at `87ce6a6d`
passed 354 library tests but failed three of 673 binary tests (two fresh-custody
fixtures reported a busy admission lock; the seed-crash fixture failed at
`before-origin`). Cargo stopped before the embedded harness, so this is no green
ordinary-suite or counted embedded-harness evidence. Planner ruling 3c8bff6f
(2026-11-05) requires preserving both suite roots and publishing the actual
failure for retained review/disposition; no retry or `store.rs` correction is
authorized by this task.

#### Ordinary fixture isolation

The three store fixtures that require an immediately free, just-closed flock
run in individual exact-test child processes: the fresh-custody replacement
and disappearing-state tests, and the seed-crash trust test. Other tests keep
their existing parallel execution. Process creation in a parallel test can
inherit an open file description until exec; closing the parent's descriptor
alone does not release that description's flock. The fixture child runs only
after exec, before it opens its own fixture locks. There is no production lock
change, retry, sleep or weakened refusal.

The cfg(test) `fixture_description_control` runs in its own exact-test process
and forks one owned child. That child reports its actual PID/PPID, both inherited
lock descriptor numbers, device/inode and unchanged CLOEXEC flags. Until exec it
uses only fixed async-signal-safe operations; two bounded gates let the parent
observe the real custody refusal while the child holds admission, then clear
custody for the pre-Cargo `Acquiring` phase but a different `Fresh` slot while
the child retains the record lock alone.
The child then executes one fixed system command. The parent positively reaps
that exact direct child before asserting old-slot recovery as `Unknown` and a
clear released fixture. A caught oracle panic closes the gate and reaps before
resuming; a child is never treated as closed merely because the parent dropped
its descriptors. The qualified guard supplies the outer bound and custody.

This demonstrates a process/descriptor hazard that targeted after-exec fixture
isolation avoids; historical causation remains **UNKNOWN**. The former same-process
`try_clone` control and the old slot-1/slot-2 observation alone did not establish
process inheritance. The seed-crash fixture still requires its exact original
slot/directory, `reused=true` and `Prior::Unknown`, before discard and cold
allocation. No original trust or custody oracle is relaxed.

The green ordinary suite at `4f3cd6c9` reached all embedded cases. Its
`cold_seed_default_off` target reports **2 passed, 2 ignored**; the earlier PR
statement that no tests were ignored was incorrect. The ignored-test policy is
unchanged. Brief v3 authorizes one changed-input process-control diagnostic and,
after it passes, one new final-head suite. All preceding evidence is preserved.

The amended owner disposition (brief v2, 2026-11-05) permits this cfg(test)-only
repair and changed-input validation on the same PR, prospectively superseding
the earlier no-retry/no-store-test-correction rulings. Historical Builds
transcripts are unavailable provenance, not required current review inputs;
no reconstruction or attribution from unchanged source is claimed. Current
review uses immutable source and checksum-verified retained Stash evidence.
All earlier failures and limits remain historical and spent.

Planner ruling 4c207fce (2026-11-05) permits one replacement 300 s / 512 MiB
process diagnostic after the new control incorrectly expected `Writers` for
pre-Cargo `Acquiring`. The real `Clear` custody verdict is distinct from the
still-held record's availability. That failed run remains charged; the owned
child's positive reap and observed descriptors remain evidence, not a pass.
Original custody/seed oracles, CLOEXEC and historical uncertainty are preserved.
