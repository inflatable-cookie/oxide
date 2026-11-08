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

The current-selection alignment in Oxide implements Tom's ruling under the
separately scoped Queue brief v5. A final exclusion removes validated current
selections from preview candidates and locked revalidation after the original
snapshot, identity and all-member reference checks. It checks every relevant
profile member, preserves whole-batch refusals and leaves released roots on
their existing candidate path. It changes no Barge reader, format, barrier,
writer-closure decision or deletion path. Focused regressions cover selected
present roots, nonrepresentative profile selection, contradictory/malformed
references, sibling identity disagreement and genuinely released fixtures.
The selected-plus-absent comparison remains strict and uses an absent checkout.
At corrected executable source `9331d3ca519e1972fcca91ac93c410046cd3c87c`,
the newly authorised uncounted diagnostic and subsequent counted comparison
each passed all three cases (exit 0):

| Case | Oxide | Barge |
| --- | --- | --- |
| Unselected v2 root, absent checkout, closed writers | Complete; candidate | Complete references; candidate; positive checkout absence |
| Selected root, absent checkout | Complete; kept because a current selection references the root | Complete references; kept by current registry/receipt/marker references |
| Required state contains malformed JSON (`{`) | Incomplete authority; whole batch kept for unknown writer custody | Incomplete references; kept for unknown pool/checkout custody |

Both controls exercised their intended gates, with checkout absence retained
and no unrelated policy predicate masking the selection or unknown-state result.
The four focused selection regressions passed. Changed-input Effigy compile,
release compile, format and docs checks passed; the single ordinary
`rust:test-600` suite reported 1055 passed, zero failed and five ignored.

New evidence under `/Volumes/Stash/scratch/oxide/`:

- `oxide-proof.1o4csics/evidence`: uncounted diagnostic, 35.19 s to receipt
  publication, minimum reserve 3020.5 GiB; receipt SHA-256
  `7444bb80076346a00ec4e178461f310acfa6a64bee03c86b0870f066aadd2a6b`.
- `oxide-proof.hct6slxh/evidence`: counted comparison, 8.82 s to publication,
  minimum reserve 3020.7 GiB; receipt SHA-256
  `04fb1c649fe421f0e31b92fd5af6c0a289c39b0ad912029a2d3c7db88f519734`.

Each used its own 128 MiB image under the unchanged qualified guard,
300 s/512 MiB bounds and sampled owned peak 8.1 MiB. Both guards closed with
status zero, no fault or deadline, positively reaped their ordinary detach
children and verified no remaining attachment of their backing. Backings,
actual input bytes, case reports, raw and covering logs and `AUTHOR-SHA256SUMS`
are retained. Sampling is an observation, not a quota. Final publication
changes only Markdown after that executable source; review still binds the
exact published head.

The historical results below describe the pre-alignment executable source.

The corrected selected-but-absent comparison (oxide#279) ran once at source
`359c35a250f46329b4697353557c6d74adf407d5`, under separately approved
uncounted activation `4510e9d8d7a3175423d941fdc138ba289d8b1a3c7d5f1018fcc6122a64e2ea73`.
Actual Oxide `prune::evaluate` and unchanged feature-off pinned Barge
`release::plan` disagree in the selected case. The strict no-offer assertion
failed (exit 101; two cases passed, one failed); no policy or expectation was
changed and no counted rerun was authorised.

| Case | Oxide | Barge |
| --- | --- | --- |
| Unselected v2 root, absent checkout, closed writers | Complete; candidate | Complete references; candidate; positive checkout absence |
| Selected root, absent checkout | Complete; candidate; fails the no-offer control | Complete references; kept by current registry/receipt/marker references |
| Required state contains malformed JSON (`{`) | Incomplete authority; whole batch kept for unknown writer custody | Incomplete references; kept for unknown pool/checkout custody |

One case specification maps each logical root to two physical fixture roots:
Oxide's canonical `<mount>/oxide-outputs/<root>` and a root created through
Barge's area API. Every case reports the union of both roots, actual
completeness and reasons, and retains global refusal separately from per-root
keeps/candidates. Assertions require the candidate in both engines and each
control's intended gate, rejecting clock/policy masking. The corrected selected
case keeps its checkout positively absent and changes only registry/receipt
selection. It retains the no-offer expectation for both engines. The actual
disagreement is consistent with the source: Oxide validates the committed
selection and then continues to checkout absence and custody, whereas Barge
retains current references. This is an observed bounded fixture result, not
permission to change either engine or waive acceptance at that source.

Corrected evidence: `/Volumes/Stash/scratch/oxide/oxide-proof.9hzvh3zc/evidence`;
receipt SHA-256
`f9a56974c09780b09f7c798976517bc2af04eaf8b12c6faf96249bad3c574a21`.
The guard closed without fault/deadline and positively reaped/verified ordinary
post-census detach. Time to publication was 28.47 s of the 300 s bound, sampled
owned peak 8.1 MiB/512 MiB, minimum reserve 3030.2 GiB. Actual input bytes,
all three engine reports, the failed assertion and raw logs remain retained.
Reviewer answer `72d5ad7a602518240bf7497e49995c0727905c42e533a3591b48133b13957e05`
confirms this does not satisfy selected-control acceptance or authorise a
counted run.

**Operator policy and scoped alignment.** Tom answered **"Current selection
protects"** in operator decision `dc1d6924-f196-4cde-8f2e-4c51c87ba65e`
(2026-11-05; question
`f500bff1884023e154b71a88773d059b8a9c09538f9774daa3cbdf083a7a17e6`).
A current selection must protect the root even when its checkout is positively
absent. This resolves the intended semantics; the measured Oxide discrepancy
above remains historical. The subsequent Queue brief v5 scopes only the
conservative Oxide exclusion and fresh changed-input validation recorded above. The original ruling alone
grants no implementation, native rerun, counted acceptance, merge or adoption
authority, and the alignment does not change the historical receipts.

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

Historical selected-present evidence at
`f12d65686b11fbc166e3afc3168142155b2c1f79`, under
`/Volumes/Stash/scratch/oxide/`:

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
Independent review identified that this older selected case also created the
checkout, changing the candidate's absence premise. Its 3/3 receipts do not
cover the required selected-but-absent control or establish task acceptance.

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

**`oxide.capture.manifest/1`** (historical design text; the current schema is
`/2`, see "Current shared capture wire `/2`" below) — written by the orchestrator immediately before
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

**`oxide.capture.transcript/1`** (historical design text; the current schema is
`/2`) — one per engine, always written, including on
refusal. An apparatus refusal is not a transcript at `/2`; it is an `oxide.capture.apparatus/2` diagnostic. Carries: `engine` (`barge`/`oxide`), `case`, `manifest_sha256`, the
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
  implementing `oxide.capture.transcript/1` (historical; `/2` is current), and the `[[test]]` harness
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

#### Proposed private parent-descriptor seam (review checkpoint)

This is a design for review, not an implemented seam or an equivalence claim.
The existing path-only operations cannot identify the parent actually used by
an earlier open by subsequently looking up the path. Their parent obligations
remain unproven. The test-only adapter can independently improve observations
of operations that already use a held directory descriptor.

The smallest candidate is a private read-only helper taking a parent path and
one final component, opening the directory once, and performing the existing
child open relative to that held descriptor. It returns owned parent and child
handles to the existing caller for the duration of the read. It must preserve
that caller's child flags, metadata checks, read limit, error mapping and
budget charging; no extra create, write, lock or retry is allowed. A test-only
observer may fstat each live handle once, preserve errno, record success or
failure and its additional syscall cost, and drop both with RAII. Observation
failure cannot affect the real operation. A positive child absence or refusal
binds the parent only if fstat on the held parent descriptor succeeded; on
failure the parent is unproven and no parent field is recorded. A
numeric device/inode pair after the handles close is not itself proof of
continuous identity. The transcript must retain unproven wherever the required
lifetime, process or object binding is missing.

Candidate sites are limited to capture-chain operations with a declared
`parent_required` obligation for the named leaf. `parent_required` is a
required boolean on each `mandatory_observations` item in
`oxide.capture.manifest/1` (schema, parser and adapter). For each, the existing
unproven note reports missing evidence; it does not discharge that obligation.
Even a held-parent observation discharges it only with the independently
qualified numeric-identity and lifetime premise, a matching component and
attributable operation. It does not by itself establish the child descriptor,
result, or complete input coverage.

| Capture-chain site | Candidate obligation and limits |
| --- | --- |
| `config::read_config` (`src/bin/oxide/config.rs`), reached by configuration resolution | Parent of the metadata and file-read operations on the selected config. They are separate path resolutions; each needs its own attributable parent binding. `fs::metadata` and the later file open follow the leaf link today. |
| `store::private_file(path, false)` (`src/bin/oxide/store.rs`), used by `prune::read_registry` and `outputs::read_receipt_with` | Parent of the non-create private-file open, including absent or refused child. The open is read/write with `O_NOFOLLOW`, even with `create=false`; those flags and its metadata and error checks must remain. Create callers are excluded and must keep their existing behavior. |
| `config::inspect` (`src/bin/oxide/config.rs`) | Parent of each reached initial, resolved, image, receipt and config metadata or bytes operation, if its case declares it mandatory. Their follow/no-follow choices and canonicalization are distinct; this proposal does not make them one observation. |
| `scratch_io::read_document` (`src/bin/oxide/scratch_io.rs`) | Parent of the no-follow document open, including absence or refusal. The existing child metadata is not an observation of its parent; the bytes still need binding to the opened child. |

The eleven literal non-create `private_file` calls are `store::recorded_last_use`,
`recorded_lost`, `holders`, `sibling_state`, `checked_sibling_state`,
`trusted_origin`, `load`, `read_observations`, `prune::read_registry`,
`prune::status_lines`, and `outputs::read_receipt_with`. The descriptor-read
inventory below identifies all except `prune::status_lines` as capture-chain
sites (with `holders` recorded through `active_holders`). Several also serve
ordinary product operations. Additionally `store::probe_lock` passes a
variable `create` to `private_file`, including potentially false; it is a lock
path and excluded. A function-level change to `private_file` would reach all
of these, including that lock path. The candidate instead requires explicit
per-caller routing for the inventoried capture sites; `status_lines`,
`probe_lock`, and create callers retain the current path open. That routing
still affects every production use of each selected caller, not only capture
runs, and must preserve its budget and refusal policy.

`config::read_config` is called by `config::settings` after `config_file`
selects an existing file; settings resolution serves ordinary product commands
as well as the capture chain. A seam in that function reaches every such call.
Likewise a seam in `config::inspect` reaches its product callers in
`store.rs` (lines 1992, 2132, 2177 and 2199), `adopt.rs` (lines 556 and 605),
and `main.rs` (line 2800), as well as capture. These would be production I/O
changes for those commands, not capture-only instrumentation. Neither change
may alter read budgets or refusal policy; the compatibility and Route B gates
below apply to their full reach. `scratch_io::read_document` also has callers
outside capture; a function-level change would reach all of them and requires
the same full-reach review.

These are candidates for a per-site design and counterfactual review, not an
assertion that a single helper is compatible everywhere. `adopt::read_bounded`
and its guidance and `read_for_write` callers are outside the capture chain
and outside this proposal. In particular, the pre-write read and subsequent
`write_block` remain unchanged. That reader's `fs::read` follows a leaf link;
replacing it with `O_NOFOLLOW` would change its semantics.

This candidate has **not** established compatibility. Opening a directory
separately adds a descriptor, an open, a possible resource or permission
failure, and a race boundary; anchoring the second lookup to that directory
can read a different object if an ancestor is replaced between operations.
A direct open can resolve a path that the proposed directory open refuses.
Symlink traversal, trailing slashes, root and relative paths, rename, parent
replacement, descriptor exhaustion and errors at each step need explicit
treatment per caller. Preserve each operation's existing leaf link semantics;
no single pinned parent may silently merge separately resolved observations.
Enumeration and lock opens have their own semantics and are out of scope.

Counterfactual review must distinguish: a parent replaced after directory open;
a child replaced between lstat and open; a symlink at either component; a
missing ancestor versus a missing child; `EMFILE` or `EACCES` on the extra
open; and a post-read pathname replacement. The adapter must not make any of
these pass by reopening a path, copying a fixture expectation, or promoting a
closed descriptor's numeric identity. If preserving the existing externally
observable results cannot be demonstrated, the seam is not mechanical and
must return the exact conflict for a scope ruling. Review of this proposal is
not activation of production I/O changes. Barge's `oxide-capture-parent-identity.md`
Route B and `o2-tape.md` treat descriptor-relative production state-reader
changes as a production semantics change and an operator scope question. The
planner must reconcile that condition with the 2026-11-06 approval and
`d2c34987` before any activation; implementation and its exact-head review
remain separate.

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
  `oxide.capture.transcript/2` document (`/1` historical), epochs assigned at the leaf,
  per-key occurrence bookkeeping under the fixed 64 cap, the 8192 read-row
  and 8 MiB document caps with explicit `truncation` flags, effect rows,
  clock rows, an additive bounded `classifications` array, and the
  additive bounded pre/post `fixture_capture` with its diff),
  `src/tape.rs` (the separate sink used only by library unit tests),
  `src/bin/oxide/capture_wire.rs` (strict `/2` parser and completeness),
  `docs/knowledge/contracts/capture-vectors/` (canonical hashed JSON),
  and `src/bin/oxide/tape_harness.rs` (the `[[test]]`
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
  `tape::fd_dev_ino` now dups (`F_DUPFD_CLOEXEC`) and fstats a live held
  descriptor (declared extra observation: `observe:dup` / `observe:fstat`
  classifications, fstat errno captured before Drop, RAII close). Callers: `prune::read_bounded`, `absence-walk:boot-data`,
  `root-observe:outputs-dir`, `root-observe:root-dir`. When the outputs-dir
  fstat succeeds, `root-observe:entry` binds that parent as `handle`. Other
  authority-read sites preserve the actual read outcome while marking
  descriptor and parent identity unproven. The tape does not obtain a past
  operation's parent through lexical ancestry, later pathname stat, reopen or
  fixture expectation. Existing metadata results still decorate removable
  registration, admission lock and volume path verification; existing
  `fstatat`/`fstat` outcomes still decorate the revalidation walk. Sites
  without those facts remain insufficient. The authority deadline is not
  reconstructed from wall time; its monotonic binding remains unproven until
  captured from the actual decision input.

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
  truncated; a pre/post diff cannot replace effect observations. Current
  (`/2`) refusals are engine-only: a pre-entry apparatus refusal writes an
  `oxide.capture.apparatus/2` diagnostic instead of a transcript, so no
  transcript can name `apparatus` as its source.
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

#### Implemented shared capture wire (oxide#283, superseded by `/2` below)

Test-only. Historical `/1` schema was `oxide.capture.transcript/1` and
`oxide.capture.manifest/1`. Adapter emission uses the schema identity names
(`parent_device`, `parent_inode`, `device`, `inode`, `volume_uuid`, `mount`,
`volume_device`), always-present `parent_source` / `own_source`
(`handle` | `extracted` | `context` | `none`), process fields, and `site`
alongside historical `kind` / `version`. Parser-positive is structure only
(`src/bin/oxide/capture_wire.rs`). Completeness matches each present
obligation identity field (not site/kind alone) and effects as a
`(kind, site, path)` bag. `parent_required` fails for `none` / `extracted` /
`context`; `handle` still fails while `K1_PREMISE_QUALIFIED` is false.
Missing `parent_required` is an issue, not false. Completeness without a
manifest is `no manifest`, never success from structure. Volume obligation
`device` is a string mapped to the row's `volume_device`; `window_identity`
is compared to `manifest.window.identity`, not a row key. Recorded holders
are typed (`pid` u32; `uid` u32 or null; `kernel_start` u64 or null;
`record_id` / `recorded_start_ms` / `epoch` u64; `site` non-empty). `fd` is
a non-negative i32 or null. The 8 MiB document cap is proven against a
syntactically valid transcript padded past the cap. Production I/O is
unchanged. Route B is not activated. Live held-descriptor observation uses
`F_DUPFD_CLOEXEC` and captures fstat errno before Drop.

Canonical vectors: `docs/knowledge/contracts/capture-vectors/` (SHA256SUMS).
Barge copies the files; there is no runtime cross-repo dependency.

| Vector | SHA-256 |
| --- | --- |
| `valid/path-only-unproven.json` | `d119a519c5fe050607117c43aaee32a7d9d474a965bcc0e7dae3b54a94c80563` |
| `valid/handle-parent.json` | `1eeef64d45322210b4501775f36aa9dd2e120404d5357fc942b471a72ad05a3b` |
| `valid/absent.json` | `27c1d50d7317d4d1ec01c4eacf3af853eb65976076e9cbe56ff8e99c53ef30c5` |
| `valid/refused.json` | `0e7530bd79edec6aad1499617b15f35e4622d120e1229fffa771480c62462fc6` |
| `valid/truncated.json` | `6c277878701a630394a9c2b9d6799672bda406ea21e34fd73bf62a74d0a85282` |
| `valid/volume.json` | `a5211e825a77ab7898246825ecd893e8c5687de77854fd6558ba34307df16077` |
| `invalid/unknown-version.json` | `5a9365d81d66b4b1eaddf55c773e6434d528e4530530aadb6d9572a8bf24fd69` |
| `invalid/unknown-field.json` | `c794624c469a4089b86a9669503415718f183888b876f8537f8d9795e0fd59fb` |
| `invalid/missing-site.json` | `60bbe57b5546d510621684a1d7874247ba7e2bea6cdc04cfbb71a541a93485d2` |
| `invalid/bad-provenance.json` | `d0d296b16873c798c78d0984bc848f7c57ef34df6050c09a3ec88ce75db55734` |
| `invalid/collision.json` | `586b01537500d475945f27956d9e62957256fcf004ccc0e40f928490e7a60683` |
| `invalid/source-mismatch.json` | `0c947a92549ca49ce13e89a2070034d36c7a8d91e4e233e790cfeb60e78f1b17` |
| `invalid/unknown-effect.json` | `d608c47662440ff1778b167fab4e6668c2eaeb3cc75653da1e6ea69fcd2878cb` |
| `invalid/string-device.json` | `bc96a18bb1be86b6c3b1b704f579736667b44b9482af4df7c4a41f41d184f11d` |
| `invalid/string-parent-inode.json` | `efd944bf6a6f762e72fe631bb037c0a2c8a91910f523aff505d709afd84928dd` |
| `invalid/occurrence-zero.json` | `9064a508b7c35bcf284ce82cbfa040b272a7003a80e8688618e6781066ffe010` |
| `invalid/wrong-parent.json` | `78e0c54f28ba6fb7e36ea639cca2295c61ae8915f9928a262f5c8816731be858` |
| `invalid/wrong-inode.json` | `c466a002728c7cde344b8e6a15a1fbf1fe96a36fc2fbf65a2498b183d2b70ae7` |
| `invalid/omitted-obligation.json` | `f393749cd739660f100a80bb30bc13266627c3dedef35e7e7b27b4be7c1553e0` |
| `invalid/extra-effect.json` | `16d01f5db93dea43de7267697989c09b6b4f66d9fd84b4ac56230b90f874f555` |
| `invalid/substituted-effect.json` | `da294866f4b9490890688e67456496614a6fadd0c3841f139bf34d98abf4cd5c` |
| `invalid/omitted-effect.json` | `41c6223d939f4dcb4dac7f514b16ac4662189acf3b9b37d797f1f0c71c5c3d56` |
| `invalid/string-holder-pid.json` | `1d694c59286eeef310a94f732a3e106f0149583634d1f754627628c33d7ea836` |
| `invalid/negative-fd.json` | `858c351163d68f5ccc618a829c35fe66d141e56994a96d6ddc76a00549662ea3` |
| `manifest/valid-parent-required.json` | `ee622767bfef80d78cb5025235faa8da380c10a01f7325067f1c19b094a5641a` |
| `manifest/valid-volume-identity.json` | `63a2baa98243a341b526743d8766b89b509aa4aaaf9b224df2ddd032233d9f84` |
| `manifest/invalid-missing-parent-required.json` | `b2a8f8a02a41d6bcea5b42e11aca714f6f25e2d57d92d29dd3ac1c7f0c3603e1` |
| `manifest/invalid-extra-field.json` | `f494fe90b091129428b37d4d56b033cb2a20ff35530af80eed290b038d30d999` |

| Remaining obligation | Limit |
| --- | --- |
| Path-only reads (`private_file`, `read_config`, `inspect`, `read_document`, authority path binds) | Parent `none`; `parent_required` stays unproven |
| K1 numeric identity and lifetime | Unqualified; handle parent is recorded, not discharged |
| Route B private descriptor-relative production read | Not activated; needs full-caller compatibility review |
| Caps 512 process / 8192 read / 8 MiB / 64 occurrences | Unchanged; known real-host overflow stays insufficient |
| oxide#241 / #222, private producer, Barge native/live | Still held |

Focused selector: `effigy rust:capture-wire` (`tape_prune_eval --self-check`).

#### Current shared capture wire `/2` (oxide#284)

Test-only. `/2` is the current proof input for both documents. A `/1`
transcript or manifest is refused by name (`historical /1 capture document
… is not current proof input`) and never upgraded. The `/1` section above and
its vectors stay as history. An apparatus refusal or fallback writes a
distinct `oxide.capture.apparatus/2` diagnostic; the transcript parser refuses
it, so it cannot read as a conforming capture. Parser and emitter live in
`src/bin/oxide/capture_wire.rs` and `src/bin/oxide/tape.rs`.

Shape. Every key is present; absence is an explicit `null`.

- Transcript: `schema` and `version` are both `oxide.capture.transcript/2`.
  `manifest_sha256` is `null` (no manifest bound) or lowercase SHA-256.
  Window origin and end must both be `null`: no absolute origin is proven,
  and a number there is refused (`must be null`). `outcome` is `completed` with
  an object `result`, or `refused` with `reason`, `exit_code` (u8) and
  `source` `engine`. `truncation` is exactly five booleans.
- Reads: `cap` is `null` when no cap applies, or a u64; `0` is a real zero
  cap. Identity carries all 15 fields. Parent device and inode are both
  present or both null; `parent_source` is `none` exactly when the parent is
  absent; `own_source` is `none` exactly when device and inode are absent.
  `kind` equals `site`.
- Effects: `kind` is one of the seven writes or `descriptor-effect` (an effect
  on a held descriptor whose path is unbound; `path` is `null`). `result` is
  `succeeded`, `failed`, `partial` or `unknown`. An attempt is never
  `succeeded`. Fixture-diff rows are `unknown` at epoch 0.
- Clocks: `value` is `null` when unobserved, never zero.
- Holders: `kernel_start` and `uid` are explicit `null` when unproven.
- Classifications: occurrence 1..=64.
- Fixture capture: entries use the wire names `device` and `inode`;
  `bytes_sha256` only on `file`; `rel` is a strict relative path.
- Epochs: at least 1 on state rows and unique across reads, effects, clocks,
  classifications and holders.
- Caps unchanged: 8192 rows, 8 MiB, 64 occurrences per key, 512 holders,
  4096 fixture entries per side.
- Manifest `/2`: typed against `oxide-capture-manifest.v2.schema.json`.
  Exact keys per object; a fixture entry is one of the schema's shapes,
  selected by `kind` (`file`, `directory`, `symlink`, `absent`,
  `unreadable`); an obligation identity matches one of the four complete
  alternatives (file-like, component, process, volume); an expected effect
  requires `attempted`; source digests are pinned lowercase hex and the Barge
  pin is the schema const; `window` start is not after end. The canonical
  examples in `capture-examples/v2/` are parsed by the self-check.

Completeness. One implementation, `capture_wire::assess_completeness`. The
in-window engine check delegates identity matching
(`identity_fields_match`) and effect parity (`effect_parity_issues`) to the
same functions, so the rules cannot diverge.

- Identity: every named obligation key must equal the row value; `null`
  never matches.
- Effects: one completed rule (`capture_wire::completed_effect`: not
  attempted, `succeeded`, pathed) is shared by parity and fixture-diff
  reconciliation. Others are issues: `effect not completed at <site>
  (<result>)` or `effect identity unproven at <site>`. An attempt, a
  `partial` or `failed` row, or a declared attempt (`declared attempt is not
  completed parity at <site>`) never matches. Parity is a multiset: an extra
  or repeated completed row, or a repeated declared effect, is `omitted or
  extra effects`.
- Unproven gaps are issues: `process lifetime unproven at <site>` (holder
  without kernel start), `clock unproven at <kind>`, `window origin and end
  unproven`.
- `parent_required` still fails while `K1_PREMISE_QUALIFIED` is false.
  `Completeness` is never success from structure alone.

Evidence. `docs/knowledge/contracts/capture-vectors/v2/` (`SHA256SUMS`, 48
files): eight valid transcripts, eighteen parser refusals, twelve
parser-positive completeness failures (including `partial`, `failed`,
duplicate observed and duplicate declared effects), three valid and seven
invalid manifests. Each invalid vector asserts the named refusal it is meant
to produce, not just a failure. The two canonical examples are checked too.
The emitter conformance regression `emitted_shape_check` in `tape_harness.rs`
writes every row kind through the real recorder leaves (including a reported
`partial` and `failed` effect through `effect_result_row`, and a repeated
completed effect), parses the bytes as `/2`, and refuses tampered, `/1` and
apparatus variants of those same bytes. The pure
selector is `effigy rust:capture-wire`; the regressions run there. Its
embedded engine fixtures (`tape_prune_eval` with no arguments) are not run by
this change.

Remaining limits (finite):

| Obligation | State |
| --- | --- |
| Path-only reads: parent `none`, `parent_required` | Unproven |
| K1 numeric identity and lifetime | Unqualified (`K1_PREMISE_QUALIFIED` false) |
| Recorded holders: kernel start and UID | Unproven (explicit null) |
| Window origin and end | Unproven (explicit null) |
| Unobserved clocks | Explicit null; completeness issue |
| Descriptor-only effect identity | Unproven (`path` null) |
| Writer result of a fixture diff | Unknown |
| Attempted or failed effects | Not completed parity |
| Route B private descriptor-relative read | Not activated |
| Caps 512 / 8192 / 8 MiB / 64 | Unchanged; known host overflow stays insufficient |
| In-window fixture binding (`extra or unbound observation`: path, component and volume rules) | Engine-side only in `tape.rs`; completeness does not apply it yet. oxide#287 adds one rule for a present leaf: a safe relative path label, the row's own device and inode from a declared held-descriptor fstat, the same bytes digest or observed outcome, and the declared window (`bound_leaf`). A path label is not parent proof and the parent stays absent. **Remaining gap:** the `/2` file-form obligation identity requires `parent_device` and `parent_inode` (`capture_wire.rs obligation_identity`), so the strict assessment reports `wrong identity at <site>` for a parent-free leaf and that template is not a reviewed exclusion; see [the leaf-binding section](#leaf-binding-for-enumerated-path-only-authority-reads-oxide287-implemented) |
| Barge `/2` adoption and live or native use | Dependent task; no production read changed |

### Bounded comparison evidence boundary (oxide#285 design)

**Status: design only.** No implementation, fixture, comparison, native run,
wire or parser change, production reader change, pin change or Barge write.
Nothing here passes a D8 case or a CF realization. The section fixes which
conclusions the current `/2` captures and the two actual evaluators can
support, states what is blocked, and names one weaker claim for a planner
decision. It extends the `/2` section and the D8 matrix above; where they
differ on current truth, the `/2` section and its vectors win. The separate
verdict policy (S1) and the effect-free first-batch plan are implemented
below; this section's proposal text stays as design history for them.

Labels used below: **observed** = a `/2` row or a source symbol read in this
pass; **documented** = stated in a cited contract and not re-verified here;
**unproven** = required and absent.

#### Source binding

- Oxide `main` `923d01f8` (PR #248 merge; reviewed head `ef8364b0`). Read:
  `src/bin/oxide/capture_wire.rs` (`K1_PREMISE_QUALIFIED` = false at line 22;
  caps at 24–27; `identity_key` at 1259; `completed_effect` at 1314;
  `assess_completeness` at 1402, which emits `window origin and end unproven`
  for any null window bound at 1437 and `process lifetime unproven` for a
  null holder kernel start at 1544); `src/bin/oxide/tape.rs` (the in-window
  `extra or unbound observation` note at 541); `src/bin/oxide/prune.rs`
  (`read_registry` 686, `read_operator_list` 2520, `revalidate_chain` 3455,
  `walk_checkout` 3551, `checkout_absence` 3857, `observe_row_root` 4267,
  `judge_recorded_root` 4586, `evaluate_receiptless` 7566); and
  `src/bin/oxide/store.rs` (`active_holders` 1154, `checkout_custody` 3301).
- Barge sibling checkout at `f9f81f72` (PR #47 merge; reviewed `00c701cf`),
  with `bcb1f58` (PR #46 merge) carrying the reader-compatibility analysis.
  `oxide-capture-parent-identity.md` names its own source as `b613506`, an
  ancestor of `f9f81f72`; its line numbers drift by about two lines at
  `f9f81f72` (for example `census::read_registry` is at 347, not 349). Barge
  `crates/` differs from the O1 pin `4af9360b` across 100 files (15,158
  insertions, 6 deletions). Barge's statement audit lists six non-test lines
  re-expressed under `cfg(not(test))`/`cfg(test)` splits, and separately five
  comment-only SAFETY repairs. Those five are comment-only and are not counted
  as non-test changes here. Barge's `o2-tape.md` "Production statement audit"
  records these and claims behaviour is preserved by review; that claim, and the separate
  `scripts/native/release-proof.py` change, are relied on here and not
  re-audited. Non-test code is therefore not byte-identical to the pin.
- Barge `o2-tape.md`: real-host cap measurement and the `/2` emitter. Barge
  `crates/barge/src/tape.rs` `wire_reads` (about 841–890) renumbers
  occurrences per emitted key and stops at 64.
- Tom decision `0bcedf55` v2 (answered Nov 8): defer the production
  parent-reader conversion. Recorded in Barge's Route B disposition as R-K2
  option A. Not re-asked.

#### Claim levels

Each level needs everything below it. A higher level never follows from a
lower one.

| Level | Claim | Needs | Status now |
| --- | --- | --- | --- |
| L0 | Wire and schema conformance | Parser, vectors and emitted shape agree with `/2` | Supported, structure only. Says nothing about inputs. |
| L1 | Observed-input binding | Each read or effect row binds to a manifest entry by complete identity | Partial. A leaf's own object identity can bind. Parent, holder lifetime and window origin cannot. |
| L2 | Evaluator decision agreement under declared fixture assumptions | L1 for every input the decision reads; every applicable CF realization; both raw results kept | Reachable per case after authorised execution. Not reached. |
| L3 | Complete equivalent-input claim (O2 acceptance) | L2 plus discharged parent binding for every direct read, qualified K1, holder kernel start and uid, enumeration parents, window origin, no cap overflow, source-bound export | Unreachable under current readers and `/2`. |
| L4 | Live deletion or adoption qualification | L3 plus live Builds evidence and the O3 gates | Held. Out of scope. |

A matching keep fate on one case is an L2 fact about that case. It is not
equivalence of the inputs both engines read.

#### Conflict resolution

| Item | Source fact | Disposition here | What it blocks |
| --- | --- | --- | --- |
| Direct-reader parents | The "Direct state reads" row in the shared contract requires parent identity. The 19 `read_bounded` production sites have parent `none` (documented, Barge Route B table). C1 would change refusals and selection (search-only `EACCES`, parent `EMFILE`, trailing slash, single relative component). R-K2 is answered A. | Path-only reads supply own-object facts for L1. Their parent obligation stays unproven and is named, not discharged. | L3 for every direct-read input. |
| K1 descriptor lifetime | `K1_PREMISE_QUALIFIED` = false (observed). APFS `(dev, ino)` non-reuse and stability over a descriptor lifetime are unverified (documented, Barge K1). | A handle-class parent cannot discharge `parent_required`. Held-handle facts stay own-object facts, scoped to one walk or call. No continuity claim. | L3. Any claim that a parent held across a window is the same object. |
| Operation-time process, fd and holder identity | Recorded holders carry explicit null `kernel_start` and `uid` unless observed (observed in `capture_wire.rs` 1544). Vnode rows carry the device and inode the descriptor returned and do not inherit a process identity. | Custody-dependent cases keep only fully identified process rows (observed pid, start and uid). A recorded holder with null start is named unproven and cannot feed a keep fate. | L2 for WRT-* where the fate rests on a recorded holder. |
| Enumeration parents (K4) | `read_dir` rows (receipts, intents, declarations, placements) carry parent `none` or `extracted`. | A listed name is an input only when its own leaf read binds to a manifest entry. An unbound listed entry is insufficient (CF-1). No listing completeness is asserted beyond what the fixture controls. | L3. Any complete-inventory claim. |
| Shared process-family key collapse | Emitter (Barge `wire_reads`) and parser (Oxide `identity_key`) key a read by `(site#family, path, component, parent_device, parent_inode)` (observed). Pid, fd, start, uid and volume UUID are not in the key, so process and descriptor families with null path, component and parent collapse per site family. | The 64-occurrence cap applies to the collapsed key. A truncated collapsed family is insufficient. The key is not widened; fixture process tables stay under cap by construction and say so. | L2 for custody-heavy cases on process tables of real-host size. |
| Fixed-cap overflow | Caps unchanged: 8192 read rows, 512 process rows, 8 MiB, 64 occurrences per key, 4096 fixture entries per side (observed). Real host measured in Barge `o2-tape.md`: about 4,700 per-fd rows plus process rows; the retained window emits about 1,860 of 8,192 rows and is truncated. | Overflow stays insufficient by rule. No cap change, no omitted process, no sampling. | Any real-host comparison; L3 everywhere. |
| Engine-side in-window binding | `tape.rs` records `extra or unbound observation` at 541 from the manifest bind check (file, component and volume rules). `assess_completeness` does not apply it (observed). | The L2 verdict treats that note as blocking. The check stays test-only and its shape unchanged. A verdict-policy requirement on the reconciler; no wire change. | L1 and L2 without it. |
| Window origin and clocks | `/2` window origin and end are required null; a number is refused. `assess_completeness` always reports `window origin and end unproven`, so no `/2` transcript can be complete. Unobserved clocks are null and an issue. The manifest's `start_monotonic_ns` and `end_monotonic_ns` have no transcript counterpart. | The L2 verdict names this one issue as a declared limit. It still blocks on any unobserved clock a decision reads. A later origin proof needs its own wire design and is not assumed here. | L3 for every case. Any cross-engine ordering or absolute-time claim. |
| Completed versus attempted or partial effects | `completed_effect` = not attempted, `succeeded`, pathed (observed). The recorder reports any error as `partial`. Descriptor-only effects are unpathed. Fixture-diff rows are `unknown` at epoch 0. Oxide's pool-lock `O_CREAT` open is an uncertain attempt. | Effect expectations are completed effects only. EFFECT-1 claims refusal with no completed effect. EFFECT-2 claims a fence save only where completed. Attempts and partials are named non-parity. | L2 for EFFECT-* and creation paths in WRT-*. |
| Source-bound export and capture qualification | No export receipt or launcher block is authorised or has run. The stage artifact is not a `barge.tape.export/1` receipt (documented, Barge `o2-tape.md`). Tom's `c6e3e9c8` v2 permits four capture-only Cargo selectors (`rust:compile:cargo`, `rust:compile-release:cargo`, `rust:format:cargo`, `rust:test-600:cargo`; AGENTS.md command map). They establish build and embedded-harness behaviour only. They do not establish private launcher isolation, global live-state invariance or current export production. | Every L2 run needs an independently supplied export-closure digest and receipt before its engine window. None is qualified. | Any counted L2 native before a separately activated export route. |

#### What a bounded L2 comparison can and cannot say

It has independent value, but only at L2. It runs both actual evaluators on
controlled inputs, so it can settle the EXPLAIN-OR-BLOCK rows
(`REF-ORPHAN-MARKED`, `RCP-V1`, `WRT-RETIRED-CLEAN`, the D-label routes) by
observed decision instead of source inference. It can also expose decision
divergences a source audit missed. A divergence is a real finding even without
parity. Both uses need only the rows that do not depend on the unproven
premises above, and only if the verdict policy names those exclusions.

It cannot establish, under unchanged production readers:

1. That a direct-read input was the object the production reader resolved
   through a particular parent. Path-only reads have parent `none`.
2. That a held handle's parent was the same object across a window (K1).
3. That a recorded holder was alive as its record says, unless kernel start
   was observed.
4. That an enumeration or inventory is complete.
5. Any cross-engine ordering, absolute-time or window-span claim.
6. Anything at real-host process scale under the fixed caps.
7. Writer closure or the absence of an unknown writer outside the fixture's
   own declared actors (CF-2 keeps this declared).
8. Parity for attempted, partial or unpathed effects.
9. Behaviour of Route B or any parent-bound reader. The current readers' C1
   differences stay unexercised by design.
10. Live Builds behaviour, deletion safety or O3 readiness.

#### Inference rules

- Numeric equality of device or inode before and after a window is not
  continuity (K1, reuse).
- A classification (held, closed, unknown, acquiring) is not an identity fact.
- A pathname lookup after an open is not the object the open used.
- An empty trace, writer list or listing is absence only where the fixture and
  the window bind it. Otherwise it is unknown.
- An omitted, truncated or unbound observation is insufficient, never an empty
  value.
- A refusal from either engine is that engine's outcome. It is never an absent
  root and never a match.
- A documented assumption (APFS non-reuse, an absolute `pw_dir`, a fixture
  writer's absence) is labelled as an assumption, never as observed evidence.

#### Case-family disposition (D8 and CF inventory, no new matrix)

Each family keeps every applicable CF realization. None is dropped to fit a
case. A CF applies only where its precondition exists in the family:

- CF-4 (broken or exhausted tape): every family, as a pure reconciler check.
- CF-1 (unbound extra folder): families with an enumerated set. References and
  intents (`read_intents`), receipts (`read_receipts_with`), the fence and
  declaration family (`area::read_declarations`). The control family also
  enumerates on every run: Barge's plan roots (`area::list_names`), its
  placements (`BargeReferences::read` → `cache::read_placements` →
  `area::read_directory`) and Oxide's receipts (`OxideSnapshot::read_within` →
  `read_receipts`, `read_dir(outputs)`). CTL-0 relies on that receipt read.
  The registry-shape family is excluded: its cases decide on the registry file
  itself.
- CF-3 (identical bytes, new inode): families with a root marker. Receipts and
  markers, and identity and volume.
- CF-7 (second reads under changed epochs): identity and volume (the marker
  reread under the second root-id lock), and checkout absence (Oxide's
  `revalidate_chain`).
- CF-2 (writer between runs): families with holders or custody. Writers and
  custody, admission effects, and VOL-REMOVED-DURING.
- CF-5 (identity epoch change): writers and custody, and the DYNAMIC rows
  ID-DEVICE-LIVE and VOL-REMOVED-DURING.
- CF-6 (missing admission inputs): admission effects only.

A family with no precondition for a CF names that CF as not applicable and
why; it is not silently dropped. "Strongest outcome" is the most a successful
execution could state. No row is passed here.

| Family | Cases | Actual sites (Oxide / Barge) | Own observation | Missing binding or premise | Strongest outcome after execution |
| --- | --- | --- | --- | --- | --- |
| Control and sticky batch | CTL-0, CTL-1 | Oxide `evaluate`, `require_ready`; Barge `release::plan` | Registry and marker bytes via bounded reads; enumerated names of plan roots, Barge placements and Oxide receipts | Path-only parents; enumeration parents (K4), so a listed name binds only through its own leaf read | MATCH for the no-write control and sticky-incomplete batch, with CF-1 binding over the three enumerations and CF-4 applied |
| Registry shape and caps | REG-MISSING, REG-MALFORMED, REG-OVERSIZE, REG-CONTRADICTORY, REG-OVERFLOW, CAP-1 | Oxide `read_registry`; Barge `census::read_registry`, `OxideSnapshot::read_within` | Bytes, cap, occurrence | Path-only parent. CAP-1 exceeds shared caps | MATCH or MATCH-KEPT per case. CAP-1 stays engine-local, not shared parity |
| References and intents | REF-PENDING, REF-RESERVATION, REF-CANCELLING, REF-PRUNING, REF-SELECTED-NOT-CURRENT, REF-ORPHAN-MARKED | Oxide `Row::live`, `dry_run`, `eligibility`; Barge `OxideRow::live`, `references_for`, `pending_intents` | Registry rows; intent bytes; enumerated names | K4 enumeration parent. Scope differs (whole-command refusal versus named root) | MATCH or SCOPE per case. REF-ORPHAN-MARKED stays EXPLAIN-OR-BLOCK until observed |
| Receipts and markers | RCP-MISSING, RCP-MALFORMED, RCP-NONCOMMITTED, RCP-CONTRADICTORY, RCP-V1, MRK-MISSING, MRK-MALFORMED, MRK-REPLACED, ID-NONCE, ID-GENERATION | Oxide `read_receipt_for_prune`, marker parser; Barge `reference::read_receipts_with`, `read_oxide_marker` | Bytes and digests; enumerated entries as `extracted` | K4 for receipt enumeration. Barge reads `/1` markers as unknown | MATCH-KEPT for missing and malformed. RCP-V1 stays EXPLAIN-OR-BLOCK: an observed difference is reportable, a match is not available from current source |
| Identity and volume | ID-INODE, ID-DEVICE-STALE, ID-DEVICE-LIVE, VOL-ABSENT, VOL-REMOVED-DURING, VOL-DUP-REGISTRY, ABS-UUID-CONFLICT | Oxide `judge_recorded_root`, `volume::Binding`; Barge `identity::read`, `identity::compare`, `VolumeRoot::open_with` | Own dev and inode; UUID; mount census at the declared epoch | K1 for any marker-read parent. DYNAMIC remount epochs. VOL-DUP-LIVE is a coverage limit | MATCH for ID-DEVICE-STALE (current binding decides). D-2 for ID-INODE only with a second eligible root. DYNAMIC rows only on declared outcomes |
| Checkout absence | ABS-ANCESTOR-MISSING, ABS-ANCESTOR-REPLACED, ABS-BOOT-DATA | Oxide `checkout_absence`, `walk_checkout`, `revalidate_chain`; Barge `os::prove_absence` | Component-level leaf facts (Oxide held descriptors: documented, not re-verified here; Barge path-based component facts) | Oxide held-descriptor parents are handle-class and K1-unqualified. Barge ancestor reads are path-based. Boot-Data legacy exception is internal placement only | MATCH for missing and replaced ancestor (both keep). D-3-scope for boot Data. No absence-parent claim |
| Writers and custody | WRT-LIVE-THIS, WRT-LIVE-OTHER, WRT-UNKNOWN, WRT-ACQUIRING, WRT-RETIRED-CLEAN | Oxide `checkout_custody`, `active_holders`; Barge `PoolCustody::read`, `pool_custody::classify`, `custody::take_custody_by`, `custody::decide` | Identified process rows (observed pid, start, uid); recorded holders with null kernel start | Recorded-holder lifetime unproven. Collapsed process key. Intermediate classifications differ | Keep-fate MATCH only, and only on identified process rows. WRT-RETIRED-CLEAN stays EXPLAIN-OR-BLOCK. WRT-UNKNOWN and WRT-ACQUIRING: keep-fate MATCH only |
| Fence, declaration, policy | FENCE-MISSING, FENCE-UNKNOWN, DECL-MISSING, DECL-POLICY-NONE | Oxide `outputs_initialization`, state parser; Barge `area::show`, `pool_custody::parse_state`, `decide_root` | State bytes | Path-only parent | MATCH-KEPT for missing and unknown fence. SCOPE for DECL-MISSING. CONFIG for DECL-POLICY-NONE (no parity) |
| Admission effects | EFFECT-1, EFFECT-2 | Oxide `checkout_custody` lock open, `fence_admission_state`, `ledger_view`; Barge `StateLock::shared_if_present` | Attempt rows with the attempted flag; completed only when succeeded and pathed | `O_CREAT` open is an uncertain attempt. Pending holding or eviction reconciliation is a stop, not a read | EFFECT-1: refusal and no completed effect. EFFECT-2: fence save as a named completed effect, MATCH on fates. A reconciliation stop is recorded as a stop |
| Legacy and operator list | LEG-UNLISTED, LEG-LISTED, LEG-LISTED-MARKED, LEG-RECEIPTLESS | Oxide `read_operator_list`, `evaluate_receiptless`; Barge identity and reference conditions | List bytes; receipts | K4 receipts. D-3 partition | MATCH for unlisted and receiptless. D-3 only when both actual outcomes are observed for the listed case |
| Transcript conformance | none (L0) | Oxide `capture_wire` parser and vectors; Barge emitter conformance | Shape only | Not an input claim | L0 only |

#### Recommendation and alternatives

**R1 (recommended; planner decision).** Name a weaker claim,
**O2-CAL**: owned-fixture decision agreement, window-incomplete,
parent-unproven, holder-lifetime-limited. It is L2 only and is a new
acceptance, not a relabelling of O2. Smallest path:

- **S1, Oxide test-only verdict policy.** Blocking: engine-side
  `extra or unbound` notes; an unbound effect; an unproven holder feeding a
  decision; an unobserved clock a decision reads; any cap or truncation flag;
  a missing mandatory key; a row bound to nothing. Named exclusions: window
  origin and end null (listed as a declared limit); `parent_required` for
  path-only direct reads, with each excluded site listed in the verdict. The
  exclusion never applies to a handle-class row. No wire, parser, cap or
  production change.
- **S2, Oxide test-only D8 case bodies, first batch.** Families control,
  registry shape, references, receipts and markers, identity and volume,
  checkout absence, fence and declaration, and legacy. Second batch after S1
  is reviewed: writers and custody, and admission effects. The second batch
  needs the holder-identity and effect rules applied in the verdict.
- **S3, Barge.** Any Barge case support is a linked Barge task. This task
  makes no Barge write.
- **Gates.** Pure self-tests for each blocking rule and each named exclusion,
  with counterfactuals: an unbound extra row, a null-start holder in a
  decision, a null decision clock, a numeric window origin, a truncated
  document, a missing row. Each applicable CF realization runs. Order: one
  uncounted keep-going diagnostic, then one counted author native, then an
  independent reviewer native. Classify ELEVATED for the second batch, because
  it decides writer custody and drives an admission lock create and fence save
  (AGENTS elevated trigger). The first batch's class is the planner's call.
- **Bounds.** Wall clock and scratch are the planner's to set. Proposal:
  600 s and 512 MiB monitored scratch, owned Stash scratch under the lane
  placement, at most one owned image with ordinary detach if the second batch
  attaches one. Fit is unmeasured.
- **Validation for this design.** `effigy docs:check` once, for the changed
  contract. No Rust or native check: nothing executable changed.

**R0 (alternative).** Hold O2 as it stands. No comparison until K1 is
qualified and an origin design exists. Honest, and it produces no evidence.
Choose this if no weaker claim is wanted.

**R2 (alternative, not recommended now).** Production parent-reader change
(Route B, C1, C1′ or C2). Deferred by Tom (R-K2 A). It adds refusals and
selection changes and still does not discharge L3 without a qualified K1.
Reopen only after K1 is qualified and the owner re-rules.

**R3 (alternative, larger).** A `/3` wire with an origin proof and captured
holder kernel start. A separate design. Not needed for L2 and not assumed by
R1.

**R4.** Full O2 parity. Blocked on every L3 item above.

#### Affected acceptance

- O2 full acceptance (L3): unchanged. Still blocked.
- O2-CAL (L2): does not exist as an acceptance until the planner or operator
  names it (design decision 1). Its limits are the list above. The separate
  verdict policy and the effect-free first-batch plan that such a claim would
  rest on are prepared below; they grant no comparison credit.
- O3 and the full-comparison hold: unchanged.
- Route B, R-K2 and decision `0bcedf55` v2: unchanged.
- Pin, production behaviour, `/2` wire, caps and selectors: unchanged.

#### Decisions for planner or operator (not made here)

1. Whether to name O2-CAL and accept its limits (R1), or hold (R0).
2. Whether the first batch excludes writers and custody, and admission
   effects (recommended).
3. The ELEVATED or STANDARD class for each batch, and the wall-clock and
   scratch bounds.
4. Whether any `/3` origin or holder design is wanted before a later
   comparison (R3).

No case is passed by this section. It gives no parity, timing, closure or
deployment credit.

### Limited verdict policy and first-batch plan (oxide#286 implemented)

**Status: prepared and pure-validated; not executed.** Tom's operator decision
`ba36f411` v2 selected "Prepare the limited fixture comparison (recommended)"
and the scoped Queue brief activates preparation and pure validation only.
This subsection is the current truth for design slice **S1** (the separate
verdict policy) and for the effect-free first-batch plan. No comparison ran,
no evaluator ran inside a case, no fixture exists, no Barge write happened, no
export or capture qualification is claimed, and O2 (L3), O3 and every hold are
unchanged.

**Separate verdict, preserved strict result.** The policy lives in
`src/bin/oxide/capture_verdict.rs`, which is reached only through
`src/bin/oxide/tape.rs` (the binary compiles that module under `cfg(test)`),
so a production build compiles none of it.
`capture_verdict::evaluate_limited_capture(transcript_bytes, manifest_bytes)` is
the entry point: both documents go through the canonical `/2` parsers first, so
schema, version, cap and envelope refusals are the wire's own and a malformed
input is an error rather than a verdict. The digest it records is computed from
the exact manifest bytes it parsed. `evaluate_limited(transcript, manifest,
manifest_sha256)` is the document-level form for already parser-validated input,
and returns one verdict carrying:

- the strict result, `capture_wire::assess_completeness` issues, verbatim;
- structured blocking reasons (`rule`, `site`, `detail`), never a bare string;
- the named limitations the verdict set aside;
- one evidence row per obligation site (class, bound leaf identity,
disposition).

`sufficient()` is true only with no blocking reason, so the strict result is
retained and a strict insufficiency can never read as a pass. Schema
conformance (`/2` parse), observed decision agreement and complete
equivalent-input proof stay three distinct things: the policy only speaks to
the second, at L2.

**The two reviewed exclusions.** Nothing else is excludable, and neither
exclusion waives a decision input's identity.

| Rule | Applies to | What the limitation must still name |
| --- | --- | --- |
| `window-origin-end-null` | The transcript's required-null window origin and end | Both fields as `null`; no ordering, absolute-time or window-span claim |
| `path-only-direct-read-parent` | `parent_required` at a site in the policy's fixed `KNOWN_SITES` table whose class is path-only direct read, only when that row binds its own device and inode and **only when the recorder did not report the row unbound** | The exact site and the leaf's own `(device, inode)` |

The site table names the **decision-relevant** row kinds the recorder emits,
each with its parent provenance: `authority-read@prune::read_registry`,
`authority-read@prune::read_bounded`, `authority-read@prune::observe_root_in:marker`,
`authority-read@outputs::read_receipt_with`, `authority-read@outputs::read_marker`,
`authority-read@outputs::marker_digest`,
`authority-read@removable::read_registration`, the `authority-read@store::*`
custody reads, `read-bounded:lstat`, `read-bounded:open`,
`outputs:read-marker:open`, `absence-walk:start`, `absence-walk:fstatat`,
`absence-walk:openat`, `absence-walk:boot-data`,
`absence-walk:revalidate-descriptor`, `absence-walk:revalidate-name`,
`absence-walk:binding-open`, `absence-walk:binding-verify`,
`root-observe:entry`, `root-observe:root-dir`, `root-observe:outputs-dir`,
`root-observe:marker-open`, `root-observe:binding-open`, `volume-site` and
`fixture-diff`. Any other kind an obligation names is unlisted and blocking — the table
is exact-match, never a prefix or wildcard — and kinds that are not decision
obligations today are deliberately absent (for example `config:read-config`,
`config:inspect:canonicalize`, `control-directory:open`, the `scratch-io:*`
reads, `store:open-existing*`, `pool:ledger-view`, and the dynamic
`read_row(site, …)` observations in `outputs.rs`). The prune owner-marker read is
**handle-class**, because `openat` runs from a live held root descriptor; only
the path-opened marker reader is path-only. The three volume-binding kinds are
declared dynamic observations, because `volume_row` carries the volume identity
form.

**Blocking, all structured.** A handle-class (K1) parent; a missing,
non-matching or unbound leaf identity; a missing mandatory row; any cap,
truncation or fixture-capture truncation flag; a conflicting read under one
key; an unobserved clock; a recorded holder without an observed kernel start
and uid; an extra or unbound observation; an attempted, partial, failed or
unpathed effect; an omitted, extra or substituted completed effect; an
unmatched fixture change; a non-null `/2` window value; case, digest, source
or fixture/expected topology drift; an unknown engine note; an unlisted site;
and the recorder's own two obligation sentences. An unknown rule or site is
always blocking, so nothing is silently ignored. The policy mirrors the strict
K1 rule exactly: a handle parent is discharged only when
`K1_PREMISE_QUALIFIED` holds, and blocks while it is false.

**The volume bind is the recorder's.** For volume-form rows the policy does
not re-implement `bound_volume`: it inherits the recorder's verdict through the
`extra or unbound observation` note, which is blocking. That note is emitted
only when no matching `dynamic.volumes` entry exists, so the volume-kind
control builds its bound case with the matching entry and its recorder-faithful
counterfactual with the entry absent; the entry-present-with-note pair is
labelled a policy-only check.

**The path-only exclusion needs a recorder-observed leaf.** At the oxide#286
head no first-batch input could use it: the recorder reported every path-only
file row unbound and the policy refuses the exclusion for any row the recorder
reports unbound. oxide#287 closes that except at the `/2` identity form
(below); the exclusion still never applies to a handle-class row and still
never waives a decision input's identity.

The exclusion set is a source-owned enumeration, never a caller waiver list
and never derived from warning text. The only strict issues an exclusion may
explain are the completeness module's own two fixed templates (the window
sentence, and `parent_required unproven at <site>: handle parent still needs
qualified K1` for a site with a granted path-only limitation). Every other
strict issue becomes `strict-issue-unexplained`, so the strict judgement
survives by construction rather than by review attention.

**Comparison rule.** `limited_comparison` has four states.
`Insufficient` covers: either side's inputs insufficient, a side that did not
run, a refusal (that engine's own outcome, never an absent root), or an
incomplete batch **on either side**. Identical decisions cannot override any of
these. `Divergence` means only that two sufficient, complete engines observed
different fates. `DeclaredMismatch` means the engines agree with each other but
not with the fates the manifest declares: a fixture or expectation defect, not
a cross-engine divergence. `Agreed` needs two sufficient sides, two complete
batches and the declared fates. A deferred or unexecuted case is never
agreement.

**Pure controls (50, discriminating).** Each asserts an exact rule and site,
not merely failure: the allowed path-only limitation with its site and leaf
identity while the strict result stays insufficient; a handle parent blocking
with no limitation granted; missing row; wrong leaf identity; unlisted site;
extra or unbound observation; unknown note; unobserved clock; unproven holder;
all five truncation flags and a truncated fixture capture; a conflicting read
under one key; attempted, partial, unpathed, omitted, extra, substituted and repeated
effects, so multiplicity is preserved; a numeric window origin; case, digest, source and topology drift; an
unbound leaf refusing the exclusion; two path-only obligations each excluded at
its own site with its own leaf identity; strict preservation; identical
decisions over insufficient inputs; a deferred case; a divergence; agreement
requiring two sufficient sides; an informative identity note on an undeclared
row not becoming a defect while the same sentence on a declared input blocks;
an unrequired parent leaving nothing to exclude; and, built from the recorder's
own output, a real-shaped path-only row carrying exactly that site's emitted
sentences (no own identity) that stays insufficient, the same with a bound leaf
but no fixture binding, an emitted `absence-walk:fstatat` component that is
named handle-class rather than unlisted, an emitted `absence-walk:binding-open`
volume row whose obligation binds on its volume identity, carrying the matching
`dynamic.volumes` entry the recorder's `bound_volume` rule needs, with three
counterfactuals: the entry-absent document the recorder would emit its
`extra or unbound observation` note for blocks; a policy-only check confirms
the policy treats that note as decisive even beside a matching entry (a pair the
recorder never emits, so it is a policy test, not recorder evidence); and a
different volume device is a wrong leaf identity, the malformed-obligation recorder
note,
the parser entry point refusing a `/1` transcript and an invalid manifest, a
Barge-only incomplete batch that is insufficient, and a declared mismatch that
is not a divergence. A further control compares the plan's printed blocking
vocabulary with the enum's own list, which is generated by one macro so a rule
cannot be printed twice or omitted. They run in the `--self-check` route, so
`effigy rust:capture-wire` covers them.

Two boundaries of the policy are deliberate. The manifest's
`mandatory_observations` list is the declaration of which reads the decision
consumes; the policy checks every declared obligation and every row the
recorder itself reports as unbound (`extra or unbound observation`), but it
cannot prove a manifest declared every input, so obligation completeness stays
a review obligation. And the recorder's per-row sentences "object or parent
identity unproven" and "component parent identity unproven" block only where
that row kind is a declared decision input; on any other row the recorder's own
bind verdict decides, and re-implementing that bind here would be a second,
divergent rule.

**First-batch plan, effect-free.** `oxide test --test tape_prune_eval --locked
-- --limited-plan` (equivalently the harness binary with `--limited-plan`)
prints one `oxide.capture.limited-plan/1` document and writes, stages,
evaluates and probes nothing. It lists all 50 rows of the finite D8/CF
inventory in eight first-batch families plus the two deferred families, and
per row: status (`runnable`, `insufficient`, `deferred`), the prospective
reason, the applicable CF obligations with an explicit reason for every
non-applicable one, the directories the fixture actually creates, the engine
enumerations the state triggers, the actual in-crate entrypoints, the ordered
per-case actions and the prerequisites. Current shape: **0 runnable, 43
insufficient, 7 deferred**. No case is executable today, for two independent
reasons: no export-closure digest and receipt is qualified, and no first-batch
input binds under the current recorder (below).

CF-1 applicability follows the fixture and the actual readers, not a family
label. Oxide enumerates no directory for receipts or intents: the registry
document carries the rows, each receipt is read by `read_receipt_for_prune`,
and `unfinished_intents` is a projection over those rows. The enumerations are
Barge's: `area::list_names` (plan roots), `cache::read_placements`,
`area::read_declarations`, `reference::read_receipts` over `read_dir(outputs)`
(`OxideSnapshot::read_within`), and `state::read_intents_with_deadline` over the
intents directory (`release::pending_intents`). The registry-shape, identity and
absence families enumerate nothing and name CF-1 as not applicable; the control,
references/intents, receipts/markers, fence/declaration and legacy families name
it with the enumerated set each case must bind.

Recorded insufficiencies, named rather than absorbed:

1. **No first-batch input binds under the oxide#286 recorder.** *Historical at
   that head; superseded by [oxide#287](#leaf-binding-for-enumerated-path-only-authority-reads-oxide287-implemented).*
   The recorder's in-window bind check required a row's parent device and inode
   (`tape.rs check_manifest_obligations`: `bound_file`). No file read records a
   parent: `read_row`/`read_row_owned` set none, and `authority_read_observed`
   passed a held descriptor's own device and inode only for
   `prune::read_bounded`. So every path-only file row reported `extra or unbound
   observation at <kind>`, and every `parent_required` obligation over such a row
   reported `mandatory observation missing or identity unproven at <site>`. The
   reviewed path-only exclusion covers the parent obligation only; it could not
   manufacture a fixture binding, so the plan reported zero runnable rows. The
   separately reviewed test-only recorder change that section named is oxide#287;
   no exclusion was widened to hide the gap.
2. **Checkout absence is insufficient under this policy.** Its decision reads
   component rows of a no-follow walk whose parents are live held descriptors.
   That is handle-class (K1) and no reviewed exclusion covers it, so
   ABS-ANCESTOR-MISSING and ABS-ANCESTOR-REPLACED stay insufficient even though
   the design's family table lists a keep-fate match as their strongest
   outcome. Reaching it needs qualified K1 or a `/3`-class capture, not a wider
   exclusion.
3. **CAP-1 is insufficient by rule.** It exceeds the unchanged shared caps;
   an overflowed or truncated document is never partial parity.
4. **EXPLAIN-OR-BLOCK rows** (REF-ORPHAN-MARKED, RCP-V1): a difference is
   reportable, a match is not available from current source.
5. **Declared scope or configuration differences** are not agreement: ID-INODE
   (D-2 batch versus root scope), DECL-MISSING (SCOPE), DECL-POLICY-NONE
   (CONFIG), LEG-LISTED (D-3) and VOL-DUP-REGISTRY (per-engine record only).
6. **VOL-DUP-LIVE is a declared coverage limit**: a real duplicate-UUID mount
   census cannot be produced without kernel forgery.
7. **Dynamic rows are insufficient**: ID-DEVICE-LIVE and VOL-REMOVED-DURING
   need a second mount epoch and CF-2/CF-5 writers inside one window.
8. **ABS-BOOT-DATA** needs internal boot Data placement and remains a D-3
   scope difference.

**Barge support: the exact smallest requirement.** The pinned sibling
`f9f81f72` already provides the in-crate harness: `crates/barge/tests/tape_eval.rs`
(`harness = false`) implements the fixed argv contract, calls the unchanged
`release::plan` through `tape_harness::run_bound`, requires the export binding
(`BARGE_TAPE_EXPORT_RECEIPT`, `_SHA256`, `_CLOSURE_SHA256`) and builds one
BASE-shaped fixture. What is missing is fixture breadth, not a seam:
`build_fixture(label)` materialises one fixed fixture (one volume record, one
owned root, one settled Oxide row, an absent checkout) and its manifest carries
empty `fixture.entries`, `fixture.roots` and `dynamic.writers`, so a first-batch
case's own state — registry rows and intents, receipt and marker variants,
volume records, declaration and fence, operator list, legacy rows — and the
manifest's description of it cannot be produced. The smallest remaining source
requirement is therefore a **per-case fixture builder inside
`crates/barge/src/tape_harness.rs`**, with no new schema, argv contract, target
or `release::plan` path. That is a linked Barge task; this PR makes no Barge
write and the plan prints the requirement in its `barge_support` object.

**Next occurrence: a proposal, not permission.** The plan prints it: **both
batches ELEVATED**. The first batch attaches one owned 128 MiB image at a
`/Volumes` mount so the fixture has a real external root binding, and an
attached image is removable media — that is the ELEVATED trigger even though
nothing is deleted, recycled or recovered. The second batch keeps ELEVATED for
custody plus an admission lock create and a fence save on the same attached
media. Bounds: 600 s and 512 MiB monitored scratch; the existing qualified
`fcfda2b0…` supervisor; owned Stash scratch (volume UUID
`8A660440-C919-45C6-A3F1-3365B9BD2006`), whose `<mount>/scratch/oxide` run root
and evidence directory the supervisor selects itself, with no `--root-parent`
and no `--evidence`; at most one owned 128 MiB image attached `-owners on` with
an ordinary post-census detach; and the nine native-proof premises, each with
its disposition. The 600 s and 512 MiB figures are unmeasured proposals, and no
live-host size or timing is inferred. A diagnostic, counted
author native and independent reviewer native each still need separate
activation at the reviewed head; this preparation grants none of them, and a
source-only review cannot grant comparison credit.

**Unchanged.** `/2` wire, parser acceptance, caps, `K1_PREMISE_QUALIFIED`,
production readers, locks, errors, selection, dependencies, pins, toolchain,
MSRV, the private-build/export provider and its qualification, the O2 and O3
holds, Route B and decision `0bcedf55` v2, and oxide#241/#222. Naming O2-CAL
as an acceptance stays open (design decision 1); this policy is what such a
claim would rest on and is not the claim.

### Leaf binding for enumerated path-only authority reads (oxide#287 implemented)

**Status: prepared and pure-validated; not executed.** A test-only recorder
change under the `ba36f411` v2 preparation scope. No comparison ran, no fixture
exists, no export or capture qualification is claimed, and O2 (L3), O3, Route B
and decision `0bcedf55` v2 are unchanged. Every changed statement compiles away
outside `cfg(test)`: `src/bin/oxide/tape.rs` is reached only from
`#[cfg(test)] mod tape`, and the instrumented leaves already carry their
`#[cfg(not(test))]`/`#[cfg(test)]` split.

**The declared observation set.** `prune.rs OBSERVED_PATH_ONLY_SITES` enumerates
the only sites where the production reader opens no parent and the whole input
is one live `File`: `prune::read_bounded`, `prune::read_registry`,
`outputs::read_receipt_with`, `outputs::read_marker`, `outputs::marker_digest`
and `removable::read_registration`. Only those take one declared cfg(test)
`fstat` of that exact held descriptor (`tape::fd_dev_ino`, which duplicates the
fd, restores the caller's errno and classifies its cost). Where a site stats the
path before opening (`removable::read_registration`), that precheck identity is
never borrowed: the recorded device and inode are the read descriptor's own. The
row then carries
the leaf's own `device`/`inode` with `handle` provenance and **no parent**; the
unchanged `authority_read` supplies the bytes, and nothing reopens the path or
stats it later. The handle-class `prune::observe_root_in:marker` site, the
absence-walk components and every `authority-read@store::*` custody read share
`authority_read_observed` and are deliberately absent: a leaf observation must
never stand in for a held parent or promote a deferred custody read. A failed
observation stays unknown: no identity is manufactured and `authority_unbound`
records why. A one-shot `cfg(test)`-only seam
(`prune::force_held_descriptor_observation_failure`) lets a bounded control
report exactly the `None` a failed `dup`/`fstat` returns **without touching the
descriptor**, so that branch can be exercised on a valid held file; it is
consumed by the observation statement, changes nothing the read returns, and is
absent from a release build.

**The one binding rule, gated to the declared rows.** `tape.rs bound_leaf` binds
a present leaf row to a deliberately created `fixture.entries` entry only when
the entry names the same safe relative path label (`root.join(rel) == row.path`,
every component `Normal`), the same own `device` and `inode`, the same
`bytes_sha256` for a digest outcome (or the same identity for an `Opened`
outcome), and the manifest declares its window. Eligibility is exact: only the
declared path-only authority site's own `authority-read@<site>` row, or that
site's expressly authorized associated open row (`read-bounded:open`, recorded
from the same single fstat), may bind, and only with the `handle` provenance
that observation records. Any other row kind — a pathname precheck such as
`read-bounded:lstat` or `outputs:marker:lstat` with an extracted identity, an
`outputs:read-marker:open`, a `removable:registration-held` — is not the declared
held read descriptor, keeps its existing `extra or unbound observation` note,
and so keeps the limited policy's conservative treatment of extra rows. A path
label is never parent proof: the row's parent fields stay absent, the entry's
declared parent fields are compared with nothing, and nothing in this rule can
discharge `parent_required`. Absent and refusal outcomes are never converted
into present rows, so they stay unbound.

**The parent obligation gets its own line.** When a row matches an obligation's
identity and `parent_required` is not discharged, the recorder now reports
`obligation parent_required unproven at <site>` instead of the conflated
`mandatory observation missing or identity unproven at <site>`; the row's own
per-row sentence (`parent identity unproven at <kind>`) is unchanged. The
limited policy maps the new sentence to the same rule as the per-row sentence:
it is explained by a granted `path-only-direct-read-parent` limitation **at that
exact site** and by nothing else; anywhere else it blocks as
`parent-required-unproven`. An unbound row still blocks on
`extra-unbound-observation` and can never be given the limitation.

**The identity comparison is shared, not duplicated.**
`capture_wire::path_only_identity_fields_match` compares an obligation's
identity minus `parent_device`/`parent_inode` — the two keys a path-only reader
cannot observe — and requires a non-empty `path`, so it never matches an empty
identity. Both `tape.rs check_manifest_obligations` and
`capture_verdict::evaluate_limited` use that one rule, and only for a site in
that declared set; every other site — including the helper rows the exclusion's
class covers, such as `read-bounded:lstat` — keeps the full
`identity_fields_match`, so the declared set is the one gate for both layers.
The strict `assess_completeness` is untouched and still applies the full rule.

**What still blocks, precisely.** The `/2` file form requires
`path`+`parent_device`+`parent_inode`+`device`+`inode`
(`capture_wire.rs obligation_identity`), so a parent-free leaf row can never
satisfy the strict identity comparison: `assess_completeness` reports
`wrong identity at <site>`, and that template is neither of the two reviewed
exclusions, so the verdict records `strict-issue-unexplained` and is never
sufficient. Closing that needs either a `/2` identity form for a parent-free
leaf or a new reviewed exclusion; both are outside this preparation and neither
is assumed. Together with the export gate, Barge's missing per-case fixture
builder, the unexcludable handle-class marker/absence/custody rows and the
identity-less pre-open rows (`private-file:open`, the first `read-bounded:open`
row, whose shared helper must not be observed), the plan still reports **0
runnable, 43 insufficient, 7 deferred**; its per-family reasons are updated from
source and no case is marked runnable.

**Evidence.** `effigy rust:capture-wire` runs the `--self-check` route, which now
includes `tape_harness.rs leaf_binding_check`: every window opens a real private
file, runs `prune::authority_read_observed` through the unchanged read statement
and `tape::check_manifest_obligations`, and then passes the **actually emitted**
transcript through `capture_verdict::evaluate_limited_capture` (canonical `/2`
parser → strict assessment → limited verdict). Its controls: a matching entry
binds and yields exactly one site-bound limitation whose leaf identity is the
observed `(device, inode)`, with `strict-issue-unexplained` as the only blocking
reason; same bytes with a new inode after a rename/replacement leaves the
emitted identity at the held descriptor's inode and an entry describing the
replacement object unbound; a wrong `bytes_sha256`, a different safe relative
label and a traversal label each stay unbound with the identity unchanged; a
shared-helper site outside the declared set emits no own identity and an
explicit `descriptor identity unavailable` note; a kernel-refused read keeps its
identity but is not converted into a present row; and every window asserts the
caller's `errno` sentinel survives the observation and the unchanged read. Two
more controls answer the exact-head reviews. (a) The one-shot seam forces the
declared observation to fail on a **valid** held file: the read still returns its
own bytes with the caller's errno, the emitted row carries no `device`/`inode`
and `own_source: none` plus `descriptor identity unavailable ... held-descriptor
fstat failed`, no manifest entry binds it, the canonical parser and limited
verdict keep it blocking (`extra-unbound-observation`) with no limitation, and
the next unarmed window binds again — so the seam neither leaks nor changes what
the read returned. (a2) The same seam is armed around a read that **fails** (a
directory descriptor): the returned raw error code, the bytes and the errno the
caller observes are compared with an equivalent **uninstrumented** read of the
same descriptor and must be identical, while the recorded identity stays unknown
(refusal outcome, no `device`/`inode`), the row stays unbound, and the limited
verdict stays insufficient with no limitation. The successful-observation
refusal runs the same comparison, so all three states — no observation,
observation success, observation failure — are discriminated on one error path.
(b) Through the real `outputs::read_marker_for_prune` call site, an entry that
matches the `outputs:marker:lstat` row exactly leaves that extracted metadata row
unbound (`extra or unbound observation`), while the declared
`authority-read@outputs::read_marker` row binds and keeps its limitation; the
limited verdict stays insufficient. Four deliberate mutations —
`bound_leaf` accepting any label; `bound_leaf` dropping the declared-kind and
`handle`-provenance gate; the seam becoming a no-op; and the forced-failure path
restoring the pre-observation errno instead of the read's own — each failed the
corresponding control by name before being reverted, so none of the controls is
vacuous. Two pure policy controls pin the note mapping: a schema-shaped
parent-naming obligation over a parent-free leaf yields the limitation and only
`strict-issue-unexplained`, and the parent sentence is explained at the site that
bound its leaf and blocks at the site that did not.

**Unchanged.** Production readers, the `/2` wire and parser, caps,
`K1_PREMISE_QUALIFIED`, the two reviewed exclusions, `assess_completeness`, the
comparison rule, pins, toolchain, MSRV, the private-build/export provider and
its qualification, and every hold.

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
