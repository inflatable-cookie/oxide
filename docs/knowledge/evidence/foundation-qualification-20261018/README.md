# Foundation qualification, 2026-10-18 (oxide#074)

One session of [workflow Q1–Q7](../../contracts/workflow.md#foundation-qualification)
on Oxide's own repository. Verdict: **partial**. All seven scenarios held,
but the bounds evidence is insufficient: there was no continuous monotonic
session clock, and two pre-session `cargo -V` probes exceed the cumulative
Cargo cap. See [Disclosures](#disclosures). This is correctness and lifecycle
evidence. It makes no speed, storage-saving or SSD claim.

## Files

| File | Content |
| --- | --- |
| `recipe.py` | The one-shot recipe that ran. SHA-256 `60841f63…0c0f`, recorded at start in `session.jsonl`; committed as `8dc6b15` before admission was granted. |
| `scratch-effigy.toml`, `launch.txt`, `effigy.log` | The task-owned scratch manifest (`admission = "heavy"`), the start stamp and Effigy's output with exit code. |
| `calls.jsonl` | Every counted invocation: argv, cwd, the full environment allowlist, PID, exit, duration, output bytes. |
| `NN-<label>.stdout` / `.stderr` | Raw Cargo JSON, test harness and `oxide` output per call. |
| `NN-<label>.rows.json` | Cargo `compiler-artifact` rows (fresh or rebuilt), test results and marker lines parsed from that stdout. |
| `boundaries.jsonl` | At each boundary: host free space, payload use, per-slot bytes, image backing bytes, pool files outside the payload, checkout `target/`, staging, test temp, evidence bytes, marker mtimes. |
| `session.jsonl`, `git.jsonl` | Staging identity and hashes, pool config, checkouts, Q5 process table, teardown. |
| `marker.patch` | The disposable marker commit applied only in scratch checkout A (B is its worktree). |
| `summary.json` | Values derived from the files above. The raw files remain the authority. |
| `sha256.json`, `.gitattributes` | Digests of every other file here; raw output is exempt from whitespace checks so it stays byte-exact. |

## Session

- Start (wall) `1792356980090771000` ns, before the scratch root existed.
  Deadline +30 min. Heavy admission granted at 297.3 s. Session ended at
  356.8 s, after teardown.
- Session counts: setup Cargo build 1, lease init 1, `oxide` 9, plain
  `cargo check` 1. Nothing failed and nothing was retried. The cumulative
  Cargo count also includes the two pre-session probes under
  [Disclosures](#disclosures).
- Pin `dc84d039cd397c8292f37f4201e3916fa2ecd1d3`, tree
  `448bed9adba200dc4489060637a3bdb05addaddf`, in a pristine `--no-local`
  scratch clone. Setup: `cargo build --release --locked --offline --bin oxide
  --bin oxide-storage-lease`, exit 0, 5.9 s, staging total 68.7 MiB.
  - `oxide`: SHA-256 `70782a0f9262c29b4197eae499d29d077a60296d8e986e4974686e56e13d093e`.
  - `oxide-storage-lease`: SHA-256 `22c70070e80536e5746395507949901bd44f98de5cc077df4676a2414da6a5e0`.
  - Toolchain `1.97.1-aarch64-apple-darwin`, from the checkout's
    `rust-toolchain.toml`: `cargo` `7672ead3…bbf5`, `rustc` `210df679…a4da`.
- Pool: `oxide-storage-lease --pool <root>/pool init --cap-bytes 4294967296
  --image-bytes 4294967296`. Backend `apfs-sparse-image-v1`, store
  `046a404b97d113788bfa839a8fedf10d`, verified capacity 4085211136 bytes.
  Every `oxide` call used it through `OXIDE_POOL`.
- Every call used the allowlisted environment in `calls.jsonl`:
  `CARGO_NET_OFFLINE=true`; `TMPDIR`, `TMP`, `TEMP` and
  `OXIDE_TASK_SCRATCH_ROOT` all pointed to one canonical `/private/tmp`
  directory. `oxide test` ran as `test --locked --offline --message-format=json
  -- --show-output`.

## Scenarios

Units: the 10 local units are the `oxide-storage` lib, three binaries, their
test harnesses and the two integration tests. The 29 registry units are the
dependency graph. The 81 tests are the whole suite plus the marker test.

| # | Call(s) | Cargo rows (local / registry) | Result |
| --- | --- | --- | --- |
| Q1 | `oxide test` in A | 10 rebuilt / 29 rebuilt; slot 1 cold | 81 passed. Marker: A path, A content. |
| Q2 | `oxide test` in B | 10 rebuilt / **29 fresh**; slot 1 warm, gate refreshed 149 files (path change) | 81 passed. Marker compiled and runtime both B's path, B's content. |
| Q3 | Edit A's marker, `oxide test` A, then B | A: 10 rebuilt / 29 fresh, marker `edited in A`. B: 10 rebuilt / 29 fresh, gate refreshed 149 files | B's marker compiled and runtime both `original marker`, B path. No stale success. |
| Q4 | `oxide test` in B | **10 fresh / 29 fresh** | 81 passed. Nothing rebuilt. |
| Q5 | `oxide test` in A, SIGKILL `oxide` only; retry | Killed run: Cargo group 80444 survived (reparented to PID 1), finished 10 rebuilt / 29 fresh, 81 passed. Retry: 10 rebuilt / 29 rebuilt, slot 2 cold | Retry waited, then `recovered slot 1 after oxide pid 80373 died (Cargo group 80444 empty)`, refused to trust slot 1 (no trusted build record), evicted it, built cold, 81 passed. Group gone afterwards. |
| Q6 | Plain `cargo check` in B, then `oxide status` in B | Check: 4 rebuilt / 29 rebuilt into `B/target`, exit 0 | Status: `B/target holds plain-Cargo deps/ or incremental/ (72 files, 41 MiB)`. |
| Q7 | `oxide status` in A | — | Cap 4096 MiB, 197 of 3895 MiB used, accounting Complete. One slot, idle. 0 waiting, nothing cleaning. Recovery of slot 1 logged. |

Q5 timing: `oxide` was killed 0.415 s after it started. Cargo's stderr had
already printed `Compiling oxide-storage`, and the process table recorded
oxide 80373 with Cargo 80444 as leader of its own group. SIGKILL went only to
PID 80373.

Marker mtimes (ns) show the stale-mtime shape in Q3:
- At creation: A `…285442873692`, B `…285531517104`.
- B was set to `…295153856000` by Q2's gate.
- A's edit came later, at `…300822826198`.
- So when Q3b started, B's unchanged file was older than A's edit and than
  the slot's last build. The gate then set it to `…307872123000`.

## Bytes at each boundary (MiB, allocated)

| Boundary | Host free (GiB) | Payload used | Slots | Image backing | A `target/` | B `target/` |
| --- | --- | --- | --- | --- | --- | --- |
| preflight | 716.6 | — | — | — | — | — |
| checkouts (pool ready) | 716.6 | 15.6 | — | 13.0 | — | — |
| after Q1 | 716.3 | 196.4 | 1: 183.8 | 227.0 | 7.1 | — |
| after Q2 | 716.2 | 302.0 | 1: 287.9 | 354.0 | 7.1 | 7.1 |
| after Q3a | 716.2 | 328.4 | 1: 313.2 | 415.0 | 7.1 | 7.1 |
| after Q3b | 716.1 | 354.1 | 1: 338.6 | 485.0 | 7.1 | 7.1 |
| after Q4 | 716.1 | 354.2 | 1: 338.6 | 485.0 | 7.1 | 7.1 |
| after Q5 | 716.2 | 197.9 | 2: 185.3 | 501.0 | 7.1 | 7.1 |
| after Q6 check | 716.1 | 197.9 | 2: 185.3 | 501.0 | 7.1 | 57.9 |
| after Q7 | 716.1 | 197.9 | 2: 185.3 | 501.0 | 7.1 | 57.9 |

- Payload is the mounted data. Image backing is the sparse file's allocation
  on the host. They overlap and must not be added together. After the Q5
  eviction the backing file did not shrink.
- Outside the pool: staging 68.7 MiB; test temp under 1 MiB; evidence
  0.6 MiB (32 MiB cap). The whole scratch root just before removal was
  646 MiB, backing file included.
- Pool control files outside the payload were walked with a directory walker.
  It records 0 for plain files (`config.json`, `ledger.json`, `receipt.json`,
  `lock`, and `image.sparseimage`, whose bytes are in the image column), so
  those rows understate a few KiB. `oxide-managed/` held 8 KiB in 9 entries.

## Teardown

- Q7 showed every slot idle, 0 waiting, nothing cleaning and complete
  accounting.
- `lsof` on the payload mount listed no open files (exit 1).
- `hdiutil detach` without force succeeded (`"disk4" ejected`).
- The scratch root, including the pool, and the test temp directory were then
  removed. No owned resource remains.

## Disclosures

- **No continuous monotonic clock (gate shortfall).**
  - The launcher took its start stamp with `time.monotonic_ns()` and
    `time.time_ns()` before creating the scratch root. Python 3.9's monotonic
    clock on macOS is process-relative: the launcher read 7626375 ns
    (`launch.txt`), so it cannot be compared with any other process.
  - The recipe was then changed to use the wall-clock start while it was still
    waiting for admission and had made no effects (`8dc6b15`; the recorded
    SHA-256 is that file).
  - A monotonic clock therefore covers only the recipe process, from admission
    at 297.3 s to the end at 356.8 s. In that span each call's monotonic and
    wall durations agree within 1 ms (`calls.jsonl`).
  - The 297.3 s before admission, covering the scratch root, the manifest and
    the admission wait, has wall-clock evidence only.
  - The brief requires one original monotonic clock, so this falls short of
    it. It is not something to explain away.
- **Pre-session Cargo probes (cumulative cap exceeded).**
  - The blocked preflight under brief version 3 ran `rustc -V; cargo -V` in
    the task worktree, then `rustc -V && cargo -V` from `/private/var/tmp`.
  - Each printed `rustc 1.97.1 (8bab26f4f 2026-07-14)` and
    `cargo 1.97.1 (c980f4866 2026-06-30)`. They ran before any task-owned
    effect and built nothing.
  - The brief counts version probes inside the caps. Cumulatively that is 4
    Cargo calls against a cap of 2 (one setup build, one Q6 check).
  - These receipts come from the author's session transcript only; no
    retained log independently records them.
  - The resumed session ran no other Cargo, `rustc` or `oxide` probes.
- **Gate path in Q3.** Every slot move here was also a checkout-path change.
  The gate therefore refreshed every in-checkout package file, which already
  covered B's unchanged marker. The content-only stale-mtime refresh, without
  a path change, was not exercised separately. Q3's must-hold condition was
  still met.
- Host free space stayed near 716 GiB, so the 20 GiB reserve check was never
  close to binding.
- **Consequence.** The scenario results stand as observed. Qualification is
  partial until an operator rules: either accept these two deviations, or
  authorize a new session. A future recipe should read one system-wide
  monotonic source (for example `time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)`)
  in both the launcher and the recipe, and run no probes outside the caps.
