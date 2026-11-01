# Proof scratch paths and cleanup (proposed, unfrozen)

This is a design for independent review, not an implemented or qualified contract. The existing supervisor and its evidence remain authoritative. No historical root is migrated, renamed, swept or deleted by this proposal.

## Layout

On the selected volume retain the existing shared `<mount>/scratch` ancestor and private `oxide` leaf, with their existing no-follow, ownership, permission, identity and device checks. Create a new private run root `<mount>/scratch/oxide/rXXXXXXXX`, with `s` and `e` children for scratch and evidence. `XXXXXXXX` represents eight randomly generated safe characters, created exclusively; collisions retry only within a bounded creation operation. The fallback remains truthfully reported as internal, with the same short run and child names under its existing fallback parent. Neither route assumes a particular mount name. Failure to establish the selected parent retains the existing warned fallback; a qualification that requires selected storage still refuses fallback.

The pool-temp proof currently models `<parent>/oxide-proof.XXXXXXXX/scratch/pt/p/payload/slots/t-XXXXXXXX/quick-99/scratch`. Its UTF-8 byte length on `/Volumes/Builds/scratch/oxide` is 105; `test-99` and `quick-9` are 104, and `test-9` is 103. The proposed `rXXXXXXXX/s/pt/p/...` saves 17 bytes, giving a maximum 88, three bytes below the 91-byte directory bound (which reserves slash, eleven-byte socket name and terminating NUL). This is not a universal guarantee: a longer or multibyte selected mount can still exceed the bound. Calculate the actual encoded maximum before executing the socket fixture and refuse if it does not fit; do not silently route to another device or weaken the product limit. Retain the fixture's slot-ID and profile assumptions as explicit tested bounds.

Keep the marker in the exact run root, bind it to run identity, canonical path and device, and keep evidence and all nested case resources within that root. Hosting protection must precede nested windows; only their owner can conclude them. Unclosed or uncertain custody never becomes concluded or eligible for expiry. The existing bounded sweep may recognize the new layout only with the same positive marker, conclusion, age and identity checks; it must not sweep old names as part of rollout. A partial or uncertain removal retains the root and evidence. Unknown writers remain unknown. No additional signalling authority follows from a shorter name.

The qualification bootstrap independently checks selected volume, parent, device, marker, evidence topology, hosting and both sweep results; it and `scripts/test-pool-temp-scratch.py` currently pin the old layout and must be updated together with any other consumers found by a source sweep. The latter pins `/private/tmp`, the run prefix, `scratch`, path arithmetic and its selector's explicit parent. `scripts/test-account-home.py`, `scripts/test-outputs-prune.py`, `scripts/test_supervisor.py` sweep fixtures, and the layout descriptions in `AGENTS.md` and `workflow.md` also require review and coordinated updates where their assumptions apply. Other helpers that assume names or containment require the same audit; consumers must use reported paths rather than infer them where possible. Historical receipts keep their original interpretation and credit.

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

## Cleanup ownership

The retained #196 review disposition at `da754796` records a recursive deletion attempt against an inherited macOS per-user TMPDIR parent, stopped by a permission error; the extent removed is unknown. It does not establish the exact command or that Oxide's supervisor performed it. A path derived from inherited TMPDIR can name a parent that the run did not create; recursively removing that parent crosses the ownership boundary. No restoration or further cleanup is authorized by that record.

A future wrapper must snapshot each environment variable's presence and value, create its own private root exclusively below an authorized parent, and record the run token and root's device/inode identity at creation. Pass a separately constructed child environment; restore any temporary local environment changes in `finally`, including unsetting originally absent variables. Cleanup accepts only the exact recorded root created by that run, not an environment-derived path or its parent. Explicitly refuse inherited TMPDIR and its parent, shared scratch, account home and mount root, even when aliases or normalization make them appear equivalent. Revalidate parent and root through no-follow, descriptor-relative operations immediately before and during removal; replacement, symlink, device change, unreadable identity or uncertain writer closure means keep and report. A name, marker or realpath alone is not proof of custody. Removal must not traverse replacements or mounted descendants. Establish an atomic identity-safe deletion mechanism before implementation; if the platform cannot support it, retain the root rather than claim safe cleanup.

The Oxide wrapper owns only roots it creates and its child environment. The generic Queue reviewer/runtime owns its own temporary-root lifecycle, including inherited environment handling; an Oxide wrapper cannot enforce that external contract. Queue owner follow-up: specify and implement a per-run provenance-bound temporary-root and environment-restoration contract for reviewer/runtime cleanup, with refusal and replacement regressions, and retain the incident's unknown extent. This brief does not change or dispatch that repository.

## Qualification required before use

Pure tests must cover byte arithmetic including multibyte and long mounts, exact limit and overflow, both routing choices, collision and permissions, symlinks and identity replacement at each step, device mismatch, marker forgery and old roots, nested hosting and unconcluded cases, partial cleanup, inherited TMPDIR aliases and environment restoration on every exit. Counterexamples include a replaced root after an initial check and a successful child whose descendants remain unclosed; neither permits deletion. Test the bootstrap's independent topology oracle and each layout-pinning consumer, including negative controls that would pass if the old path were accidentally used.

A future approved HIGH complexity implementation brief must apply the proof-authoring premise checklist, covering Effigy selectors and an uncounted keep-going diagnostic before a counted author native, plus a distinct independently activated reviewer native for the supervisor qualification. It must state fixed wall-clock and scratch budgets, selected-volume and fallback cases, resource receipts and the exact source head. Changes to cleanup, expiry, writer closure or removable storage trigger Elevated review and independent native verification; fixture isolation does not remove that trigger. Run executable steps only under the existing host safeguards and separately activated scope. Independent exact-head design review and any necessary operator contract rulings precede implementation; no native evidence is supplied by this design.
