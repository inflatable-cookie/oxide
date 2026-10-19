# Normal-command overhead, round 3 (oxide#092): author-measured no-go

The author's first implementation proof failed fixed gate 1: the median
paired difference of `oxide check` minus `cargo check` was 142.656 ms
against a 100 ms ceiling. Gate 2 passed at 81.556 ms. The brief stops on any
measured candidate gate failure, so there was no tuning, rescore, replay or
second proof, and no product code from this branch is accepted or installed.
Summary and limits:
[landscape](../../rust-dx-landscape.md#oxide-normal-command-overhead-round-3-2026-10-19).

The gate was fixed in the brief before any result: on the std-only fixture,
after one warm-up of each, nine interleaved pairs (Oxide first in even
pairs, Cargo first in odd ones); the statistic is the median of the paired
differences. Plain Cargo ran `--profile oxide-quick` with Oxide's quick
profile definitions, its own private target directory, the same private
`CARGO_HOME`, offline environment, lock file and toolchain. Every measured
request exited 0 without compilation markers in retained stderr; each run
printed `timing x`. These are preserved author observations.

- `baseline.log` / `baseline.json`: the separate baseline phase,
  `effigy rust:oxide-overhead-baseline`, run once on instrumentation-only
  commit `5cc76cdf95e6de9807d7c3fcfc4bbedf41aba605` (current main plus the
  opt-in phase trace and the harness; no algorithm change). 11.8 s from
  admission (staging 5.2 s), 10 top-level requests, 17 Cargo processes,
  owned scratch 80,736,256 bytes, minimum sampled host free
  823,309,017,088 bytes over 47 samples, exit 0. One pair per gate, so it
  observes and does not gate: check +355.949 ms, run +272.397 ms.
- `candidate-1.log` / `candidate-1.json`: the first focused proof,
  `effigy rust:oxide-cli`, on commit
  `16c89f8fc078b57f0e1cc3db094e9c2bb10dd9a1`. The timing phase ran first on
  the fresh 128 MiB pool; every later correctness case ran and passed,
  including the crash-in-gate case on its deterministic hook, before the
  proof exited 1 on the recorded no-go. The exact elapsed seconds and scratch
  bytes were not printed. Source control flow and that terminal failure
  indicate traversal of the 300 s and 512 MiB checks, ordinary detach and
  child-tree removal; none was independently observed. The two scratch
  inventories are checkpoints, not continuous peak measurements. The enclosing
  temporary parent was left empty according to the author. Minimum sampled
  host free: 779,957 MiB over 450 samples. Log written
  2026-10-19T17:00:38+01:00.

Each JSON is the harness's single labelled stdout record, re-indented. It
retains request wall times, stdout, stderr with timing lines removed and
outer whitespace trimmed, and parsed phase fields. Original request stderr
containing the timing lines is not separately preserved. `sha256.json` lists
content hashes. Both records carry the same sampling digest: fixture and exact
sampling/gate-function text, excluding other helpers, environment and globals.

Source commits are the author's recorded attribution. The baseline records a
staged-binary SHA-256; the candidate record has no staged-binary hash or embedded
source-head receipt. Stored hashes and matching sampling digests authenticate
preserved records and source consistency, not an independent attestation of
which binary executed. Raw JSON, logs and their hashes remain unchanged.

Version/setup allowance: two logged commands per phase (`cargo -V`,
`rustc -vV` in the fixture environment: 1.97.1, `aarch64-apple-darwin`).
Oxide's Cargo processes are counted from its `cargo.*` spans; a plain
request is one.

The reviewer measurement the brief makes decisive was not requested: the
author-measured failure already stops the task.

The independent evidence-only review ran no compile, format, Linux compile,
Rust test suite, docs check, Cargo matrix, profile, lease or inventory proof.
No separate ordinary-validation receipts are supplied in these six files;
product staging does not replace those checks. The author reported skipping
ordinary compile/format/full Rust/lease checks after the stop, and reported a
focused development unit test; neither is independently executed evidence here.
One baseline and one candidate proof were used. No second candidate proof,
tuning, replay, rescore or reviewer-native proof was performed; remaining proof
allowance was unused. Correctness-case completion is inferred from the author
log and pinned harness, not a successful overall selector result or independent
runtime approval. Product correctness and semantic equivalence remain unaccepted;
no all-four-product-PASS, adoption, installation or merge follows.
