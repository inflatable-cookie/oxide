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
or consumer changes. Freshly built engine and probe targets are retained through the
occurrence and cleanup belongs solely to the qualified shared supervisor;
no private process census or in-run deletion exists, and retained-target
fit is unmeasured (a truthful refusal at activation is a valid outcome). The first activated diagnostic then contradicted two
fixture/apparatus premises, which are corrected here: the float probe now takes the
plain-Cargo baseline's own complete output and exit as its expectation (the pinned
serde_json/zmij pair mis-rounds the double below 1.0), and an extracted
`Cargo.toml.orig` is authenticated against its OWN archive member rather than the
archive's normalized `Cargo.toml`. A second diagnostic then showed the extracted-source baseline must be compared
per package: a lock-listed package extracted by Cargo after setup is admitted
only after its archive authentication, while an earlier package that changed or
vanished refuses. Both diagnostics remain spent with no compatibility credit;
no retry, pin/feature change or weakened authentication.
