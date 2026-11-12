Added the focused Relax compatibility recheck apparatus (oxide#308): the
effect-free `research:relax:compat:test`/`:plan` selectors and the
planner-activation-gated heavy `research:relax:compat:diagnostic`/`:qualify`
selectors around the new compat modes of `scripts/relax-quick-trial.py`, with
committed fixtures under `fixtures/relax-quick-trial/compat-fmt`,
`compat-crc32` and `compat-hello`. It asks only whether the newer pinned Relax
engine (makepad/relax `6ea9d063…`, tree `514321c9…`, 2026-11-12, MIT) clears the
observed ARM64 intrinsic blocker on actual float formatting and the UNCHANGED
prior Oxide workload (`61a55ebb…`): serde_json 1.0.151/zmij 1.0.23 and
crc32fast 1.5.3 at the workload lock's exact checksums, plus the snapshot's
read-only `--help` route, compared byte for byte against plain Cargo. Freshly
built engine and probe targets are retained through an occurrence and cleanup
belongs solely to the qualified shared supervisor; no private process census or
in-run deletion exists.
The three spent uncounted author diagnostics ended in a **scoped compatibility
no-go**, recorded with the owning research notes. The completed one at source
head `7bb9dcea` (Darwin arm64, Rust 1.97.1, the unchanged `61a55ebb` workload)
ran all seven case rows: serde_json/zmij float formatting **matches** the Cargo
baseline exactly (3063 bytes, exit 1, SHA-256 `b93a9c3b…`, including that pair's
known non-roundtrip `3fefffffffffffff` → `0.9999999999999999` →
`3ff0000000000000`), crc32fast **matches** (196 bytes, exit 0, SHA-256
`a7a1aec3…`), and the unsatisfiable source-constrained control **matches**. The
unchanged Oxide workload's `--help` route does not: Cargo exits 0 with 4412
bytes, while Relax exits 101 with no stdout and 2161 stderr bytes carrying a
front-end panic (`range start is greater than range end in BTreeMap` at
`libs/relax/front/src/preload.rs:372` and `lazy.rs:469`) while parsing
`libc-0.2.190/src/unix/mod.rs`, with `serde-1.0.229` in the failure chain. The
original intrinsic blocker is therefore cleared on these probes while this pin
remains unsuitable for the quick path.
The same occurrence observed a 2237.0 MiB sampled peak of the fixed 4096 MiB
monitored scratch (no fit refusal), 149.1 s of the 600 s limit, 48/48 direct
children reaped, and a qualified supervisor closure with a joined deciding and
final certain-empty census, no survivors, fault or deadline; the bounded receipt
carries 4 of the 7 case rows as an excerpt while the hash-bound samples artifact
carries all seven. Both earlier diagnostics remain spent with no compatibility
credit: the first contradicted a Cargo-baseline oracle and an archive `.orig`
premise, and the second refused legitimately growing lock-listed extraction
before any Relax arm, which the extracted-source comparison now per-package
admits only after archive authentication. The counted author qualification was
never activated because compatibility failed, no timing or performance
comparison was measured or is justified, and the 307 no-go for its own pin stays
exactly as recorded. No product behaviour, recipe, default, dependency,
installation or consumer changes.
