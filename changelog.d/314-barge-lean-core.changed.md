Pin the merged lean Barge storage core with its adopted-root seam and add
Oxide's storage layer over it: one Barge account per pool, volume identity
through Oxide's mount-root seam (a stale drive directory is absent, a different
volume is a refusal), `oxide/pool` with per-project sub-areas and
`oxide/outputs` declarations that keep existing folders, marked roots,
exclusive/shared holds, durable pins, reserve/floor pruning, marker-checked
trash and status. Existing Oxide slot and output directories are adopted in
place through `Area::adopt_root`, keeping their path and bytes, with the repeat
idempotent and foreign, symlinked, writable or missing folders refused.

Delete the capture/equivalence apparatus (observation harness, capture wire and
limited verdict policy, release comparison, private-build validation and
seed-readiness selectors with their scripts and contracts) and rewrite the Barge
adoption contract to the lean boundary.

The pool, output, prune and removable engines do not yet call this layer for
their slot/root lifecycle; that replacement remains outstanding, so this entry
claims no behaviour change to their storage yet.
