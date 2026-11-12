Added the focused Relax compatibility recheck apparatus (oxide#308): the
effect-free `research:relax:compat:test`/`:plan` selectors and the
planner-activation-gated heavy `research:relax:compat:diagnostic`/`:qualify`
selectors around the new compat modes of `scripts/relax-quick-trial.py`, with
committed fixtures under `fixtures/relax-quick-trial/compat-fmt`,
`compat-crc32` and `compat-hello`. It asks only whether the newer pinned Relax
engine (makepad/relax `6ea9d063…`, tree `514321c9…`, 2026-11-12) clears the
observed ARM64 intrinsic blocker on actual float formatting and the UNCHANGED
prior Oxide workload (`61a55ebb…`): serde_json 1.0.151/zmij 1.0.23 and
crc32fast 1.5.3 at the workload lock's exact checksums, plus the snapshot's
read-only `--help` route, compared byte for byte against plain Cargo. The 307
no-go for its own pin stays exactly as recorded; nothing ran, no timing is
measured, and no product behaviour, recipe, default, dependency, installation
or consumer changes.
