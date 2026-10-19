# Normal-command overhead, round 2 (oxide#088)

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
  the fresh 128 MiB pool; every correctness case then passed. 65 s, exit 0.

Each JSON is the harness's one `oxide-baseline-json:` or
`oxide-candidate-json:` stdout line, re-indented; the logs are the full
Effigy output. In `baseline.log` that line follows Cargo progress output on
the same terminal line; the JSON after the marker parsed whole. Both records
carry the same fixture digest (fixture, sampling function and gate
arithmetic). `sha256.json` lists content hashes.

Wall times are whole requests measured by the harness, program execution
included. Phase spans may overlap and must not be summed. Fastest-of-three
arithmetic was fixed before any result; the #079 noise statistic is
reported beside it and never added to the ceiling.
