# Proof scratch paths and cleanup (proposed, unfrozen)

This is a partial no-go for independent review, not an implementation-ready, frozen or qualified contract. The existing supervisor and its evidence remain authoritative. No historical root is migrated, renamed, swept or deleted by this proposal. The source inventory below is not yet complete, and identity-safe removal has not been established. Neither the proposed creation protocol nor its qualification sequence authorizes implementation.

## Layout

On the selected volume retain the existing shared `<mount>/scratch` ancestor and private `oxide` leaf, with their existing no-follow, ownership, permission, identity and device checks. Create a new private run root `<mount>/scratch/oxide/rXXXXXXXX`, with `s` and `e` children for scratch and evidence. `XXXXXXXX` represents eight randomly generated safe characters, created exclusively; collisions retry only within a bounded creation operation. The fallback remains truthfully reported as internal, with the same short run and child names under its existing fallback parent. Neither route assumes a particular mount name. Failure to establish the selected parent retains the existing warned fallback; a qualification that requires selected storage still refuses fallback.

The pool-temp proof currently models `<parent>/oxide-proof.XXXXXXXX/scratch/pt/p/payload/slots/t-XXXXXXXX/quick-99/scratch`. Its UTF-8 byte length on `/Volumes/Builds/scratch/oxide` is 105; `test-99` and `quick-9` are 104, and `test-9` is 103. The proposed `rXXXXXXXX/s/pt/p/...` saves 17 bytes, giving a maximum 88, three bytes below the 91-byte directory bound (which reserves slash, eleven-byte socket name and terminating NUL). This is not a universal guarantee: a longer or multibyte selected mount can still exceed the bound. Calculate the actual encoded maximum before executing the socket fixture and refuse if it does not fit; do not silently route to another device or weaken the product limit. Retain the fixture's slot-ID and profile assumptions as explicit tested bounds.

Keep the marker in the exact run root, bind it to run identity, canonical path and device, and keep evidence and all nested case resources within that root. Hosting protection must precede nested windows; only their owner can conclude them. Unclosed or uncertain custody never becomes concluded or eligible for expiry. Any future bounded sweep may recognize the new layout only with the same positive marker, conclusion, age and identity checks and a separately established identity-safe deletion mechanism; it must not sweep old names as part of rollout. A partial or uncertain removal retains the root and evidence. Unknown writers remain unknown. No additional signalling authority follows from a shorter name.

The qualification bootstrap independently checks selected volume, parent, device, marker, evidence topology, hosting and both sweep results; it and `scripts/test-pool-temp-scratch.py` currently pin the old layout and must be updated together with any other consumers found by a source sweep. The latter pins `/private/tmp`, the run prefix, `scratch`, path arithmetic and its selector's explicit parent. `scripts/test-account-home.py`, `scripts/test-outputs-prune.py`, `scripts/test_supervisor.py` sweep fixtures, and the layout descriptions in `AGENTS.md` and `workflow.md` also require review and coordinated updates where their assumptions apply. Other helpers that assume names or containment require the same audit; consumers must use reported paths rather than infer them where possible. Historical receipts keep their original interpretation and credit.

### Proposed creation and topology protocol

For a future brief, the short component is exactly `r[0-9a-f]{8}`.
Generate four cryptographically random bytes for each attempt, encode lowercase
hex, and create the directory exclusively relative to the verified parent
descriptor with mode 0700. Retry only `EEXIST`, at most eight attempts; random
source failure, other errors, exhaustion, failed no-follow open or mismatched
owner, mode, device or inode refuse. Create `s` and `e` exclusively with mode
0700 under the held root; never accept an existing entry. The name is not the
run ID: use a separate 16-byte random ID encoded as 32 lowercase hex digits,
as the current marker does. A collision never adopts the colliding entry.

The selected-volume route independently resolves and validates the existing
shared ancestor and private leaf on the selected device, then creates the
root. If selected placement cannot be established, report its reason and
use the existing warned internal fallback only where that proof allows it;
qualification requiring selected storage refuses fallback. An explicit
`--root-parent` remains an explicit, separately reported route, including
the approved internal-placement exception. Adopted scratch is not a fresh
owned root: retain its existing nested evidence semantics and do not infer
ownership or expiry from its environment. Failed creation on either owned
route keeps and reports any created object whose safe removal cannot be
established; do not silently reuse it. Failure to establish the fallback
refuses launch. Never switch devices merely to fit a socket path.

Before creating children or launching a writer, publish an owner-only bounded
`.oxide-scratch.json` in the root. The proposed marker retains version 1's
exact keys and types: `version`, `run_id`, canonical `root`, `owner_uid`,
`device`, `inode`, `created_unix`, nullable `concluded_unix`, `outcome`,
`closure`, `kept_reason`, and `nested_scope`. Bind the recorded identity
to the opened root and parent device; require a regular owner-only marker,
bounded to 4096 bytes, with the existing validated states. Initial state is
running, no conclusion, no closure and exclusive scope. Publish atomically;
a partial or unverifiable marker refuses launch and keeps the root. A future
implementation must bind its exported run ID, marker and receipt to the
*same* run, rather than assume the current separate custodian and marker
tokens are interchangeable.

The monitor publishes the actual source, parent, root, scratch, evidence,
run ID, device and marker state from its own successful setup in the receipt;
the child environment exports those actual scratch/evidence paths and deadline.
Those exports are hints, not authority: the independent bootstrap derives the
selected parent and device separately and validates opened objects, marker and
receipt against them. Consumers must validate their own authorization and
containment; a valid-looking marker is not a secret or a defence against a
hostile same-UID process. Reject mismatched IDs, devices, marker identity,
noncanonical paths, symlinks, partial sibling pairs and a new root combined
with old children or vice versa. Where an operation needs proof of custody
rather than merely choosing an evidence path, a reported topology alone cannot
authorize it.

Protect a hosting root before any nested window or nested resource is created.
Only the authority that owns that window may conclude its resources and the
hosting root. Evidence, nested fixtures and retained resources remain inside
the monitored root; an unclosed or uncertain descendant prevents conclusion
and expiry. For a partial creation, failed publication or partial deletion,
keep the surviving root and record what is known; never mark it positively
closed on inference. Existing version-1 markers and old names retain their
original meaning and evidence. Rollout performs no migration or deletion.
Any later bounded sweep of new names requires the existing positive closure,
allowed reason, identity and strictly-over-seven-day gates and the separate
identity-safe deletion resolution below. No shorter name alone widens legacy
eligibility; unknown or forged markers remain ineligible. The current sweep's
one-second and 4096-step ceilings remain ceilings, not permission to remove.

### Compatibility and ordering

The exact owned sibling pairs are legacy `oxide-proof.<legacy>/scratch`
and `evidence`, or new `r[0-9a-f]{8}/s` and `e`. Do not form a
Cartesian product of accepted names. Preserve the distinct adopted nested
case only where that consumer already accepts it, with component containment
and its existing independent safety checks. A forged pair that merely looks
right is not custody; reject it wherever provenance is required. Keep
historical permissive names in their historical test/receipt interpretation,
not as new-grammar proof. Existing deployed and historical pins remain
unchanged until a separately approved consumer slice selects a compatible
qualified source; no receipt or native credit migrates.

Future runner work must mechanically depend on oxide#233's independently
reviewed and qualified result, then rebind to its actual source and recheck
the census and cleanup contracts. First qualify the runner and bootstrap
together with backward-compatible validators and negative controls; only
after that can individually briefed consumers adopt and run their own proofs.
Do not repin a consumer silently or run two concurrent modifications to the
shared runner. Independent exact-head review and planner freeze precede
implementation; a docs merge itself is neither freeze nor qualification.

### Executable layout gates identified

The old spelling is executable policy, not just a receipt convention.
`src/bin/oxide/config.rs::supervisor_scratch` requires canonical existing
`oxide-proof.*/scratch` and sibling `evidence` directories and a parseable
`u64` deadline before enabling the account-home proof control. Its controlled
home must also be a canonical directory strictly inside scratch. The separate
`src/bin/oxide/store.rs::supervisor_layout_paths` predicate uses the same
spelling and deadline rule for debug proof controls; its negative tests and the
config control tests must migrate with their respective predicates. Neither
predicate authenticates a supervisor merely from these environment values.

`scripts/test-account-home.py` independently checks the old scratch spelling;
`scripts/qualify-supervisor.py` is the independent bootstrap oracle, not just
an ordinary child. `scripts/test-outputs-prune.py` has a separate image-custody
allocation and synthetic sibling/mode/device tests; its historical layout
commentary is not itself an authority to relax those checks. Updating these
consumers requires explicitly testing old, new and mixed layouts. This inventory
is partial: other proof gates and ownership primitives still require audit
before an implementation contract can be frozen.

### Further source constraints

| Source | Current constraint and migration consequence |
| --- | --- |
| `src/supervisor.rs::armed` and its tests; `src/bin/oxide/admission.rs` tests | Accept nested evidence or the old named siblings, with a parseable deadline. Admission and debug hooks use this shared decision; preserve adopted nested runs separately and test mixed siblings. Recognition is not authentication. |
| `src/inventory.rs` mutation control | Independently accepts nested evidence or old siblings, and requires scratch under `/private/tmp`. The internal-placement restriction is a real control, not a short-name assumption to remove. |
| `scripts/test-accounting-vanished.py` | Requires internal scratch, nested evidence or old siblings and matching temp variables; update its sibling discriminator without erasing the internal exception. |
| `scripts/test-command-budgets.py`, `scripts/test-accounting-guards.py` | Receipt writers accept nested evidence or old named siblings. Test new and mixed sibling names and component containment, including character-prefix impostors. |
| `scripts/test-recorded-slot-accounting.py` | Evidence must be a real sibling outside scratch and not a filesystem-root child; tests include wrong export and both containment directions. Names are not its discriminator; preserve those negative controls. |
| `scripts/test-profile-outputs.py` | Checks common parent containment and excludes evidence inside scratch; preserve those predicates and their negative controls. |
| `scripts/test-outputs-prune.py` | Image custody requires canonical sibling paths, owner, non-writable modes and same device; the exact names are not the custody check. Preserve these checks and synthetic refusals. |
| `scripts/test-pool-temp-scratch.py` | Pins old prefix, child names, and eight-character random spelling in its supervisor topology and socket model; other `scratch` matches describe product slot scratch and must remain unchanged. |
| `scripts/test_supervisor.py` | Contains old-name sweep roots, negative controls, bootstrap oracles and glob assertions. Retain old fixtures as historical/negative controls while explicitly adding new cases; do not reinterpret old receipts. |

This table supplements, rather than completes, the consumer inventory. A full
audit must also classify every environment gate and source pin before changing
any consumer.

### Consumer and pin audit at planning base `bd89999`

The explicit runner digest checks at this base are catalogued by the
[replacement-census contract](workflow.md#shared-proof-supervisor):
`test-command-budgets.py`, `test-holding-delete.py`,
`test-outputs-prune.py`, `test-recorded-slot-accounting.py` and
`test-slot-headroom.py` name `fcfda2b0…`; `test-accounting-guards.py`
and `test-status-fast.py` name `d89115f1…`;
`test-accounting-vanished.py`, `test-storage-layout.py` and
`test-max-active.py` name `8e379c93…`. These are individual source
requirements, not permission to update the constants. Historical declarations
in scripts, `effigy.toml`, AGENTS and workflow can describe different
receipts; distinguish an assertion against running bytes from historical text.
The #227 review records the then-qualified `fcfda2b0…` bytes; #197's
controlling native chain is bound to `c1c4446` and its recorded
text-only successors, while #192's chain is bound to `a19fb6c` and its
specified documentation-only successor. The older #151 `bc24b238…` receipts
remain evidence of their own bytes. None automatically qualifies the
post-#233 runner or a new layout. The current checked-out source, the
deployed selector command, and a historical receipt are separate facts.

| Consumer class | Source and constraint to preserve |
| --- | --- |
| Shared Rust arming and callers | `src/supervisor.rs`, `src/proof.rs`, `src/bin/oxide/admission.rs`, `src/owned_delete_hook.rs`, `src/lease/seed/mod.rs`, `src/bin/oxide/{prune,store,seed_plan}.rs`: arming controls admission reports and proof hooks, while each hook retains its own additional scope. Tests in the shared module and admission exercise nested and sibling cases. |
| Independent Rust gates | `config.rs`, `store.rs`, and `inventory.rs` have the independent restrictions above; do not replace them with mere receipt presence. `prune.rs`, `one_use.rs`, `lease/pool.rs`, `handoff.rs` and `adopt.rs` also use scratch containment for their own test or product controls. Their path boundaries are not blanket proof-root authority. |
| Scratch and evidence pass-through | `offline_seed.py`, `test-storage-inventory-cli.py`, `test-oxide-cli.py`, `test-status-fast.py`, `test-holding-delete.py`, `test-pool-describe.py`, `test-oxide-wrapper-bypass.py`, `test-installed-handoff.py`, `test-scratch-pool.py`, `test-front-door-accounting.py`, `test-cold-slot-seeding.py` and `test-finder-metadata-race.py` use reported paths, create fixtures under scratch, write evidence, or pass controls into isolated children. Existing canonicality, containment, deadline and digest checks remain effective even where no component spelling is inferred. |
| Guard-presence and temp equality | `test-drive-mode.py`, `test-everyday-cli.py`, `test-init-guidance.py`, `test-freshness-submodules.py`, `test-oxide-pool-layout.py`, `test-oxide-removable-removal.py`, `test-permanent-outputs.py`, `test-profile-outputs.py`, `test-seed-default-off.py`, `test-storage-layout.py`, `test-max-active.py`, `test-slot-headroom.py` and `test-oxide-removable-pool.py` check or propagate scratch/evidence/deadline and, where applicable, temp equality or image custody. Do not convert a presence check into an assertion of provenance. |
| Synthetic and historical references | `test_supervisor.py`, `test_dogfood_scripts.py`, `test_build_context_comparison.py`, `test-cargo-profiles.py` and the pure seams of the named consumers contain fixture paths or old topology controls. Retain counterfactuals deliberately; change an assertion only when its meaning changes. AGENTS and workflow describe deployed and historical behaviour, not a command to rewrite old receipts. |
| Qualified bootstrap and route ownership | `qualify-supervisor.py::topology` independently compares suite record, expected selected parent and device; `bounded-test.py` owns selected/fallback routing, receipt, sweep and marker. `rust-test-600.py` owns its two explicitly created roots and passes adopted scratch; it does not own the inherited environment parent. |

The review of #227 at `a52e487` (comment `6565393813`) independently
confirmed the #196 retained report: a recursive removal was attempted against
an inherited per-user temporary parent and stopped on a permission error; the
extent is unknown. That review establishes neither the exact command nor
supervisor attribution. The source audit here does not inspect or repair the
live parent.

### Deletion mechanism: current no-go

The current `scripts/bounded-test.py::empty_directory_entries` opens
subdirectories with `O_NOFOLLOW` and checks device and owner, but unlinks or
removes a *name* after checking it. `remove_marked_root` likewise checks the
root before a later pathname `rmdir`. Replacement between those operations
can select a different entry. Descriptor-relative traversal and two identity
observations alone do not close that gap. The current
`scripts/rust-test-600.py` wrapper uses `shutil.rmtree(...,
ignore_errors=True)` on its two created roots after a successful child and on
some setup failures; it is not evidence of identity-atomic cleanup.

`src/owned_delete.rs` supplies a source-grounded alternative building block:
a same-directory exclusive rename (`renameatx_np(RENAME_EXCL)` on macOS,
`renameat2(RENAME_NOREPLACE)` on Linux), with bounded collision retries and
journaled captures. Capturing may itself capture a replacement: its identity
must be checked *after* capture and a foreign capture retained under journal
custody, not deleted. Its existence does not prove the Python supervisor has
an integrated, qualified recursive deletion protocol or exclude a cooperating
owner moving a name after capture. Until a future design binds every removal
to a protected captured entry, defines recovery and proves the cooperative
boundary and mount/writer checks, the short-layout implementation must
**KEEP and report** rather than use these primitives as claimed proof. It
must not remove a replacement, retry an ambiguous deletion, or treat an
unreadable observation as absence. Partial work remains retained and reported.
This is a precise no-go for the new cleanup mechanism, not a retroactive
judgment of historical receipts.

## Cleanup ownership

The retained #196 review disposition at `da754796` records a recursive deletion attempt against an inherited macOS per-user TMPDIR parent, stopped by a permission error; the extent removed is unknown. It does not establish the exact command or that Oxide's supervisor performed it. A path derived from inherited TMPDIR can name a parent that the run did not create; recursively removing that parent crosses the ownership boundary. No restoration or further cleanup is authorized by that record.

A future wrapper must snapshot each environment variable's presence and value, create its own private root exclusively below an authorized parent, and record the run token and root's device/inode identity at creation. Pass a separately constructed child environment; restore any temporary local environment changes in `finally`, including unsetting originally absent variables. Cleanup accepts only the exact recorded root created by that run, not an environment-derived path or its parent. Explicitly refuse inherited TMPDIR and its parent, shared scratch, account home and mount root, even when aliases or normalization make them appear equivalent. Revalidate parent and root through no-follow, descriptor-relative operations immediately before and during removal; replacement, symlink, device change, unreadable identity or uncertain writer closure means keep and report. A name, marker or realpath alone is not proof of custody. Removal must not traverse replacements or mounted descendants. Establish an atomic identity-safe deletion mechanism before implementation; if the platform cannot support it, retain the root rather than claim safe cleanup.

The future wrapper snapshots presence and the exact byte value of
`TMPDIR`, `TMP`, `TEMP`, `OXIDE_TASK_SCRATCH_ROOT`,
`OXIDE_TEST_INTERNAL_ROOT`, `OXIDE_SCRATCH_ROOT`, and
`OXIDE_PROOF_EVIDENCE` before any local mutation. It constructs the child
environment from a copy, replacing only its declared controls; on all exit and
exception paths any local mutations are restored exactly, including removal
of originally absent keys. Record the parent descriptor's device and inode,
the newly created root's device and inode, and the random creation token at
exclusive creation; never reconstruct provenance from child output or an
environment value. Compare identities as well as resolved paths when refusing
the inherited temporary directory, its parent, account home, shared scratch,
mount root and aliases. If a protected path cannot be resolved or its
identity cannot be read, refuse rather than guess. A child exit code of zero
is not positive descendant closure. The existing dual-root wrapper copies
its environment and refuses inherited controls, but its path-based recursive
cleanup does not prove this future provenance contract.

The Oxide wrapper owns only roots it creates and its child environment. The generic Queue reviewer/runtime owns its own temporary-root lifecycle, including inherited environment handling; an Oxide wrapper cannot enforce that external contract. Queue owner follow-up: specify and implement a per-run provenance-bound temporary-root and environment-restoration contract for reviewer/runtime cleanup, with refusal and replacement regressions, and retain the incident's unknown extent. This brief does not change or dispatch that repository.

## Qualification required before use

Pure tests must cover byte arithmetic including multibyte and long mounts, exact limit and overflow, both routing choices, collision and permissions, symlinks and identity replacement at each step, device mismatch, marker forgery and old roots, nested hosting and unconcluded cases, partial cleanup, inherited TMPDIR aliases and environment restoration on every exit. Counterexamples include a replaced root after an initial check and a successful child whose descendants remain unclosed; neither permits deletion. Test the bootstrap's independent topology oracle and each layout-pinning consumer, including negative controls that would pass if the old path were accidentally used.

A future approved HIGH complexity implementation brief must apply the proof-authoring premise checklist, covering Effigy selectors and an uncounted keep-going diagnostic before a counted author native, plus a distinct independently activated reviewer native for the supervisor qualification. It must state fixed wall-clock and scratch budgets, selected-volume and fallback cases, resource receipts and the exact source head. Changes to cleanup, expiry, writer closure or removable storage trigger Elevated review and independent native verification; fixture isolation does not remove that trigger. Run executable steps only under the existing host safeguards and separately activated scope. Independent exact-head design review and any necessary operator contract rulings precede implementation; no native evidence is supplied by this design.


### Finite follow-up and qualification sequence

1. Resolve and independently review the remaining custody no-go, including
   whether an integrated exclusive capture and durable journal can protect
   every descendant and root through removal under the cooperative owner
   boundary. Unsupported primitives, renamed or replaced entries, symlinks,
   mount/device changes, unreadable metadata, failures, or unclosed writers
   mean KEEP. A same-UID hostile actor is outside this boundary; no signal or
   reparenting authority is added. Operator contract questions before a freeze:
   is retaining every root acceptable until such a mechanism is qualified,
   and which owner will specify the separate Queue reviewer/runtime cleanup
   contract? No answer is implied by this proposal.
2. After #233's qualified reviewed result, a HIGH/ELEVATED shared-runner slice
   implements the agreed layout and any separately approved custody mechanism,
   applies the proof-authoring premise checklist, and runs the covering pure
   supervisor and docs selectors plus appropriate compile/format/tests for
   changed code. Its tests exercise both exact sibling pairs and mixed/forged
   pairs, selected and fallback routes including mocked/unclassifiable facts,
   exclusive collisions and exhaustion, long and multibyte paths at and
   above the exact limit, mode/owner/identity replacement, marker forgery,
   hosting and UNCLOSED retention, setup and partial cleanup failure, aliases,
   mount changes and environment restoration including absent variables.
3. At the final published source head, run one owner-activated uncounted
   keep-going `effigy proof:supervisor:qualify:diagnostic`, then one counted
   author `effigy proof:supervisor:qualify`, then a distinct independently
   activated reviewer qualification. Use the fixed 600 s outer deadline,
   560 s candidate limit, 530 s suite alarm and 512 MiB monitored scratch,
   unless explicitly amended before that run. Record exact source, executor,
   outcomes, closure, resource figures and evidence; a failed diagnostic is
   history, not counted native credit. Follow the applicable Effigy admission
   and host safeguards, or a specifically authorized exception; do not reuse
   an old receipt or reset a running clock.
4. After that qualification, separate consumer briefs pin the qualified source,
   migrate their gates and negative controls, and run their own covering
   selectors and focused proofs at their specified bounds. In particular,
   `proof:pool-temp:test` and the separately authorized pool-temp diagnostic
   and native cover that consumer; image, account-home and other consumers
   require their own scoped proof. No consumer-specific native is implied by
   the supervisor qualification. Independent exact-head design review must
   judge these mechanism, migration and deadline decisions before freeze.


### Remaining decisions before implementation

The matrix above identifies concrete layout predicates but is not a complete
classification of all proof-environment consumers and their pinned source
versions. In particular, a future audit must enumerate each environment reader
and distinguish a topology predicate from a path pass-through, an internal
placement requirement, a synthetic negative control or historical text. It
must identify the deployed supervisor and consumer pins from immutable receipts
and decide an exact compatibility staging order without changing those pins.
Until that is independently reviewed, no consumer migration is authorized.

The deletion no-go is independent of that inventory. The supported exclusive
rename is a candidate capture operation, not a conditional unlink-by-inode:
the protocol must establish protection for the captured name and every
descendant, durable recovery after each partial step, mount-boundary detection
(including same-device mounts), and positive writer closure. If that cannot
be established under the cooperative boundary, retain and report all affected
roots. An operator decision is needed only if that retention is unacceptable
or if a new safety boundary or material scope is proposed. The generic Queue
owner must decide its own reviewer/runtime cleanup contract; this document
does not prescribe its implementation or grant dispatch authority.
