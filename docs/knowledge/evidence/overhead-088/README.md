# Normal-command overhead, round 2 (oxide#088): final no-go

The independent exact-head review measured both fixed gates above 100 ms
(check 136.071 ms, run 107.444 ms). That required gate failure decides the
outcome; the author's passing set is retained beside it, not averaged with it
or substituted for it. Nothing from this branch is merged or installed, and
its runtime and toolchain cache must not be imported.

Raw records for the like-for-like overhead gate. Summary and limits:
[landscape](../../rust-dx-landscape.md#oxide-normal-command-overhead-round-2-2026-10-19).

- `baseline.log` / `baseline.json`: the separate baseline phase,
  `effigy rust:oxide-overhead-baseline`, run once on instrumentation-only
  commit `4a11f9d6d95aec7cf756dd86d28e004535d47ae1` (current main plus
  opt-in phase tracing and the harness; no algorithm change). Two rounds,
  ten measured requests, 25 Cargo driver processes including setup and
  staging, 15.0 s, exit 0. Below the gate's three samples, so it observes
  and does not gate.
- `candidate-1.log` / `candidate-1.json`: the first focused proof,
  `effigy rust:oxide-cli`, on commit
  `d217b37840d642e891923b1bdcc90a1f81469489`. Its timing phase ran first on
  the fresh 128 MiB pool. Author's set: check 97.908 ms, run 93.974 ms. Every
  correctness case passed in that one run, but the crash-in-gate case was
  later shown to race (below), so the pass does not establish it. 65 s, exit 0.
- `review-candidate.log`: byte-for-byte copy of the independent reviewer's
  exact-head `effigy rust:oxide-cli` log for head
  `259088e391ef119105859c7e2404bb5abd096a58`. Source path
  `/private/tmp/oxide-review.4f2SrF/oxide-cli.log`, modified
  2026-10-19T08:44:25+01:00, 32,860 bytes, SHA-256
  `9f1232597694f327f4b26d18b470c136e3c7defcff5c0a6f698105ac5a0f8ea2`
  (review comment 6262314476). Copied, not re-run. Its timing phase ran first
  with the same fixture digest: check 136.071 ms (fail), run 107.444 ms (fail),
  noise 11.398 / 30.399 ms, `cargo run` minus `cargo build` 141.213 ms. The
  proof then exited 1 at `slot was not moved to B before the gate`, so later
  controls did not run.

Each JSON is the harness's one `oxide-baseline-json:` or
`oxide-candidate-json:` stdout line, re-indented; the logs are the full
Effigy output. In `baseline.log` that line follows Cargo progress output on
the same terminal line; the JSON after the marker parsed whole. Both records
carry the same fixture digest (fixture, sampling function and gate
arithmetic). `sha256.json` lists content hashes.

Review findings, all retained:

1. The toolchain cache stats the resolved compiler only after the `-vV` and
   `--print sysroot` spawns, so a compiler replaced between them could pair
   old version output with the new binary (unfixed).
2. The background pre-Cargo snapshot runs `git rev-parse --show-toplevel`
   before the lease, so the `GIT_HOLD` crash fixture can fire before the
   record reaches `acquiring`. Its assertion and cleanup race, and the
   controls after it did not run in the review.
3. Aggregate bounds are insufficient: staging artifacts outside the fixture
   parent are not counted toward 512 MiB, the host reserve is checked only at
   boundaries, not during phases, and the clock starts after provisioning
   and admission. This is not proof that a cap was exceeded.

Wall times are whole requests measured by the harness, program execution
included. Phase spans may overlap and must not be summed. Fastest-of-three
arithmetic was fixed before any result; the #079 noise statistic is
reported beside it and never added to the ceiling.
