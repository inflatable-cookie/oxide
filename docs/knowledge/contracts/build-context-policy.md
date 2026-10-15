# Build-context policy (proposed)

Status: proposed design, approved as design direction on 2026-10-15 in [vision](../vision.md#authority-boundaries). Nothing here is active. Shipped ordinary-Cargo behavior stays in [ordinary Cargo storage](ordinary-cargo-storage.md) and [reuse identity](ordinary-cargo-reuse-identity.md). The current ordinary prototype does not evict, does not enforce a new budget, and does not know about incremental state. Those are candidate implementation work, not shipped ability.

Consumer defaults do not change. Nucleus's managed-agent stopgap (`CARGO_INCREMENTAL=0`, Nucleus commit `fb2bd04`) stays in force until a separately reviewed adoption.

## Principle

Choose build context from the expected lifetime of the edit/cache state and the diagnostics the workflow needs. Do not choose it from whether the caller is a human or an agent.

| Context | Expected lifetime | Diagnostics required | Candidate default |
| --- | --- | --- | --- |
| `one-shot` | Candidate, CI-style or throwaway build. Little or no reuse expected. | Test execution, assertions, backtrace file:line (line tables). | Incremental off. Line-table debug. |
| `iterative` | Sustained edit/check/test loop in one retained exclusive slot. | Same as one-shot. | Incremental on. Line-table debug. |
| `evidence` / `full-debug` | Explicitly requested capture or debugger session. | Full debug info, source lookup, native binding. | Explicit opt-in only. Never inferred. |

Human and agent workflows both fit these contexts. A supervised human loop can be `one-shot`. An unattended agent loop can be `iterative`. Identity labels never select the context.

## Resolution order

Context resolution runs before partition selection, signing or acquire. The resolved effective tuple is fixed at acquire.

1. **Consumer workflow declaration.** A consumer-authorized selector or workflow names its context. This is proposed. No shipped CLI, protocol or accepting issuer exists for it.
2. **Explicit evidence request.** `evidence` / `full-debug` is requested per invocation through the same authorized route. It is never a fallback or a side effect of another context.
3. **Effective inputs.** Oxide resolves what Cargo and rustc will actually see: profile values, `CARGO_PROFILE_*` and `CARGO_INCREMENTAL` environment, Cargo configuration and `rustflags`, feature set, toolchain provenance, allowlisted environment.
4. **Conflict rule.** An explicit effective input beats a context default. If an explicit input conflicts with the declared context (for example `CARGO_INCREMENTAL=1` in a `one-shot` declaration), the request refuses. Oxide does not silently pick a winner. Silent override of a consumer stopgap is forbidden.
5. **Partition selection.** The resolved tuple chooses the slot. No adaptive switching by history, size or time.

Unknown or ambiguous inputs fail closed, as in the current adapter's validation boundary.

## Exact compatibility

The partition key includes, in addition to the existing project/component, toolchain, host/target, profile, features, configuration/environment, and source-root identity:

- effective incremental state (on/off);
- effective debug level and split-debuginfo mode;
- effective codegen-units and profile name;
- the resolved `rustflags`/configuration digest and the environment digest;
- Cargo and rustc provenance.

Two requests that differ only in incremental state are different partitions. Cargo treats a changed compile input as different unit state, so mixing states in one slot is not a reuse path. It is a rebuild source.

Rules:

- **No identity shortcut.** A context name, a human/agent label, or a stopgap flag is never identity. Only the resolved tuple is.
- **No mid-lease toggles.** Effective values are fixed at acquire. A changed value is a new request and a new partition, not a change inside a leased slot.
- **No adaptive oscillation.** Oxide does not turn incremental on or off because a slot grew, aged or slowed.
- **No default activation.** Context selection is inert until a reviewed adoption names it for a workflow.

## Storage semantics

Keep three quantities separate. Do not add them.

- **Shared warm reuse.** Unchanged-unit hits inside one exact partition. Generic Cargo fingerprint reuse works without rustc incremental state. Measure it as rebuilt-unit counts and latency, not bytes.
- **Bounded growth.** Retained logical bytes, allocated bytes, file counts and incremental-state bytes per partition, sampled over time. This is occupancy, not writes.
- **Actual writes.** Bytes written to disk. Unknown unless an attributable measurement exists under the owner's permission. Never inferred from occupancy deltas, final binary size or SSD-life estimates.

Caveats:

- Incremental off removes per-run incremental state. It does not promise lower cumulative writes. A changed crate still recompiles without incremental reuse, and that can write more. The comparison measures this.
- Changed-crate cost remains with or without incremental. Shared warm reuse does not remove it.
- APFS clones and snapshots can share data. Hardlinks count once per measurement. Report allocated bytes by unique inode.

## Caps, peak and idle

- **Hard footprint containment.** An enforced ceiling the OS write path honours: a fixed-size APFS sparse image, or an equivalent filesystem quota. Writes beyond it fail. This is the only hard cap.
- **Sampled threshold.** A monitored level that refuses new acquire or aborts a run. It is not a guarantee against writers outside the containment. A threshold and a hard limit are not interchangeable.
- **Peak space.** Maximum occupancy during an in-flight run, including unfinished writes. Budgets must cover peak, not only settled size.
- **Idle retention.** Only released, idle slots are candidates for retention decisions. Leased, uncertain or evidence-bearing storage is never a candidate.
- **Evidence outranks budget.** Budget pressure refuses or reports. It never deletes evidence or consumer-retained output.

Current behavior is non-evicting. Adding budget enforcement or generation retirement is a candidate implementation that needs its own brief.

## Ownership, liveness and release

- Oxide releases a slot only on authenticated settlement under the existing lease contract.
- Unknown ownership, crash, PID absence, age, compiler `.lock` files or process disappearance never authorize release or evidence removal. This matches [vision](../vision.md#authority-boundaries) and [architecture](../architecture.md#storage-and-ownership).
- Budget pressure returns an actionable refusal with an inventory. It does not clean up.

## Upstream garbage collection

- **rustc session GC (verified in rustc 1.97.1 source, 8bab26f4f).** It operates only inside a crate's incremental directory. It removes `s-*-working` session directories older than about ten seconds, finalized session directories except the newest per crate when unlocked, and orphan lock files. It leaves unknown entries alone. It does not touch Cargo `deps`, build outputs or debug-referenced objects.
- **Cargo global cache GC.** Covers registry and Git sources. It is not build-artifact GC.

Consequences:

- Neither upstream GC manages Oxide slots, Cargo variants, incremental work products or retained executables with debug references.
- A future Oxide fine-grained GC is deferred. It needs a complete pinned liveness analysis for Cargo variants, incremental work products and every retained executable's debug references.
- No periodic clean churn. No selective `*.o` deletion. Unpacked debug info can refer to object files, so removing them can break later debugging and backtraces.

## Debug context

- The everyday line-table default keeps backtrace file:line. Assertions and test execution are not changed by context.
- `full-debug` is explicit. It is required for debugger variable/type inspection and for native evidence claims. It is not required to measure ordinary Cargo cost.
- **Packed-at-capture / dSYM** is a candidate, not a default. It has real costs: dsymutil time, extra bytes, native identity binding between executable and dSYM, and source custody of referenced paths. It is not proof of the current fixture issuer.

## Not claimed

- No shipped context resolver, CLI or protocol.
- No shared warm slot, automatic generation retirement or selective object deletion.
- No global Cargo or rustc configuration change.
- No Effigy change. Effigy stays language-neutral. Oxide's adapter resolves Rust context and passes selector and env only.
- No change to frozen Contract010.
- No consumer source, task, admission or settlement ownership.

## Comparison recipe (frozen methodology)

This section defines one bounded comparison. It is authorized by the 2026-10-15 ruling in [vision](../vision.md#authority-boundaries). The execution brief pins it again before launch. This section does not run anything. It does not rerun the original four-Cargo, 45-minute reuse proof.

### Source pin

- Nucleus commit `fb2bd0449ede2d9d2089a47b0559182bd413b91f`, tree `b47caf2ffb7e4430fc5e78411a5b475203126194`.
- `Cargo.lock` sha256 `ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d`. This is identical at reported `af475bc` and `9565ced`.
- Current native `main` resolves to this commit. The pin does not depend on that. Do not assume a later main matches.
- Toolchain: rustc 1.97.1 at `8bab26f4f`, Cargo matching. Nucleus has no `rust-version` and no `rust-toolchain` file. No MSRV is inferred.

### Clone and verification

1. Create `OWNED=<fresh-owned-root>` (defined below).
2. For each mode, `git clone --no-local --no-checkout <nucleus-path> $OWNED/m/src-<mode>`. This reads the native repository and writes only inside the owned root. Never create a worktree in the native repository and never move its refs.
3. `git -C <src> checkout --detach fb2bd0449ede2d9d2089a47b0559182bd413b91f`.
4. Verify `git -C <src> rev-parse HEAD^{tree}` equals the tree pin. Verify `shasum -a 256 Cargo.lock` equals the lock pin. Verify `git status --porcelain` is empty. Any mismatch stops the run.

### Tool identity

Probes count toward the invocation cap (see below).

1. `command -v cargo rustc`; record resolved paths and `shasum -a 256` of each binary.
2. `rustc -vV`: `release` must be `1.97.1` and `commit-hash` must start with `8bab26f4f`.
3. `cargo -vV`: record the version line. The Cargo version must match the rustc release line of the pinned toolchain.
4. Mismatch stops the run before any Cargo build.

### Loaded Cargo configuration and rustc identity

Checked before the mode's C0 invocation, without running Cargo. Record names and results only.

1. Repository config: `git -C <src> ls-files` must contain no `.cargo/config`, `.cargo/config.toml`, `rust-toolchain` or `rust-toolchain.toml`. Any match stops the run. The pinned commit contains none (checked read-only at `fb2bd04`).
2. Ancestor config: Cargo searches the working directory and its parents. For `$OWNED/m/src-<mode>`, check `test -e` for `.cargo/config` and `.cargo/config.toml` in `$OWNED/m/src-<mode>`, `$OWNED/m`, `$OWNED`, `/private/var/tmp`, `/private/var`, `/private` and `/`, and the same two names under `$CARGO_HOME`. Read any file that exists. It stops the run if it sets `build`, `profile`, `rustc`, `rustc-wrapper`, `rustc-workspace-wrapper`, `target-dir`, `build-dir`, `incremental`, `rustflags` or `env`. Presence of an unrelated key is recorded and is not a stop.
3. Wrapper and override variables are absent by construction. `env -i` with the allowlist below excludes `RUSTC`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `RUSTFLAGS` and `CARGO_BUILD_*`.
4. Actual rustc: in the measured `-v` output, each `Running ... rustc` line must start with the rustc path or rustup proxy resolved in Tool identity step 1, with no wrapper prefix. A different path or a prefix stops the run. This check is part of each measured invocation, so it adds no probe.

### Selected test and selector

- Route: the existing Nucleus Effigy selector `check:core:test`, which runs `cargo test --offline {args}`. Run it from the mode's source root as `effigy check:core:test -- -p nucleus-core --test packages -v -- --exact committed_workflow_packages_parse`. The first `--` is Effigy's separator. The second passes `--exact` to the test binary. The pinned Nucleus instructions use the same first separator (`AGENTS.md`, `crates/README.md`).
- Selected test: `committed_workflow_packages_parse` in `crates/nucleus-core/tests/packages.rs`. It is a single test. It reads `packages/` at the repository root. It executes and asserts `checked > 0`.
- Effigy admission: the run must show effigy's admission (for example `waiting for QA capacity`) or an admitted start. If the selector is not admitted through effigy, stop. Do not run a direct cargo fallback.
- Tests must execute. The success line must show `1 passed`. No-run output is compile evidence only, and no no-run builds are planned.

### Environment

Use a fixed allowlist with `env -i`. Record only variable names and effective values for the flags below. Do not dump other environment values or secrets.

Allowlist, both modes:
- `PATH`, `HOME`, `CARGO_HOME`, `RUSTUP_HOME`: host toolchain and shared registry, read-only by intent (`--offline`).
- `CARGO_TARGET_DIR=$OWNED/m/target-<mode>`.
- `TMPDIR`, `TMP`, `TEMP` = `$OWNED/m/tmp-<mode>`.
- `CARGO_INCREMENTAL=0` for `off` mode, `=1` for `on` mode.
- `CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=unpacked`, `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=unpacked`.
- No `CARGO_BUILD_BUILD_DIR`. No shared target. No `RUSTFLAGS`. No Nucleus stopgap env beyond the listed names.

Expected effective values, verified from the `-v` rustc lines of the measured invocations:
- debug is `line-tables-only` from the manifest's `[profile.dev]`. The test profile inherits dev.
- `-C incremental=` present for `on`, absent for `off`.
- `-C split-debuginfo=unpacked` in both modes.

### Mutation sequence

Both modes run the same sequence in the same order: `off` first, then `on`. Record machine load (`uptime`) before each mode.

| Step | Source change | Expected |
| --- | --- | --- |
| C0 | none, cold fresh target | builds all, test passes |
| E1 | append `// oxide-build-context comparison edit 1` to `crates/nucleus-core/src/lib.rs` | library and test binary rebuild, test passes |
| E2 | append `// oxide-build-context comparison edit 2` to `crates/nucleus-core/tests/packages.rs` | test binary rebuilds, test passes |
| U | none | no `Compiling` lines for the workspace; test still executes and passes |

Edits are comment-only. They change compile inputs, not behavior. This comparison does not qualify behavior-bearing path correctness or source invalidation for real projects.

### Negative backtrace check (not a Cargo invocation)

After U, per mode:

1. Rename `packages/` to `packages.oxide-moved` in the owned source clone. The test's read of that directory then fails at its `expect("packages directory")`.
2. Run the built test binary directly from its path under `$OWNED/m/target-<mode>/debug/deps/packages-*` with `RUST_BACKTRACE=1 --exact committed_workflow_packages_parse`. Record the exit code and the panic output. Expected: panic, with a backtrace frame at `tests/packages.rs:<line>`.
3. Restore by renaming back. Verify `git status --porcelain` is empty again.

This is a direct execution of an already-built test binary. It is not a Cargo or rustc invocation. It is counted separately in the evidence (at most one per mode). It is the only planned backtrace check. No panicking build is planned, and no extra Cargo build is added to obtain one.

Under `unpacked` the test binary's debug info refers to `.o` files under `deps/`. Those files must stay in place for this check. Record whether the frame resolves to file:line. Do not claim LLDB or native full-debug custody. Leave them unqualified.

### Invocation list (countable)

Cargo and rustc invocations, including probes. Cap: 10 total.

| # | Mode | Invocation | Counted |
| --- | --- | --- | --- |
| 1 | — | `cargo -vV` | yes |
| 2 | — | `rustc -vV` | yes |
| 3 | off | C0 `effigy check:core:test ...` | yes |
| 4 | off | E1 | yes |
| 5 | off | E2 | yes |
| 6 | off | U | yes |
| 7 | on | C0 | yes |
| 8 | on | E1 | yes |
| 9 | on | E2 | yes |
| 10 | on | U | yes |

Total = 10. No retry, no extra probe. Any additional invocation stops the run and is reported. Direct test-binary runs for the negative check are separate, at most one per mode.

The preferred plan was three edits per mode (cold+3+1 = 5). It is reduced to two edits to fit the combined cap with probes. This reduction is explicit.

### Time cap

- One original UTC timestamp and one monotonic start mark, taken at the start of live preparation.
- Total live budget: 20 minutes from that mark, including preparation, probes, builds, tests, measurement and capture. The cap is not reset and not extended.
- Independent review (up to 20 minutes) is outside this live budget.
- Per-invocation durations are recorded from wall and monotonic clocks. Monotonic measurement uses the shell's `$EPOCHREALTIME` or an equivalent.
- If the deadline arrives mid-run, stop after the current invocation completes. Record a partial result. Do not start further invocations.

### Owned footprint and containment

- Fresh owned root: `/private/var/tmp/oxide-build-context-cmp-<launch-id>`. The launch-id is generated at launch. The execution brief checks with `test ! -e` that the path and its image file are unused before any live effect.
- Containment: one fresh APFS sparse disk image of 16 GiB (hard ceiling), mounted at the owned root. It contains both clones, both target directories, temp directories, logs, instrumentation and evidence. Creation and mount follow the procedure already used for the standalone pilot's fixed sparse image, with size changed to 16 GiB. The procedure is named and checked by the execution brief. It is not created here.
- Sampled threshold: abort or refuse when the image's used space reaches 14 GiB, sampled at a fixed interval recorded in evidence. This is an abort threshold, not the hard limit.
- Host reserve: refuse start if measured host free space is below 20 GiB or unknown. Sample it before the run, at each sample and after the run. Abort on breach. Overshoot and unrelated-writer races are recorded, not eliminated.
- Excluded from the image: the shared host `CARGO_HOME` registry, which stays read-only by intent under `--offline`. Its size is recorded before and after as a targeted directory check. Writes there are not bounded by the image. This is stated as a limit.
- Not allowed: any second volume, spare, data copy, or fallback target. No inspection or deletion of prior task targets or the user's targets. No global quota or settings change.
- After evidence capture the image stays until independent review. Detach happens only after settlement. Image file deletion is a later separate decision.

### Measurements

Per invocation:
- command, cwd, effective env names and flag values, Cargo and rustc versions;
- the config check results from "Loaded Cargo configuration" and the `Running ... rustc` path observed in `-v` output;
- UTC start and end, monotonic duration, exit code;
- `Compiling` and `Fresh` counts from `-v` output;
- the test result line;
- the rustc flag lines for the workspace crates (bounded excerpt).

Per target and per mode, after each invocation:
- logical bytes (`find -type f`, `stat` size);
- allocated bytes by unique inode (hardlink-aware, one `du` per directory);
- file count under `deps/` and count of `*.o` there;
- incremental directory bytes;
- `.dSYM` count (expected zero under unpacked).

Per mode:
- image used and free before, after and sampled;
- host free before, after and sampled;
- attributable writes: unknown. The only available counters are machine-wide and shared with other workers. They are recorded as an unattributable host delta, if the existing non-privileged read works. They are not per-run write counts.

Limitations to state with results:
- APFS clones and snapshots are not created by this recipe, so sharing does not confound counts. Other tasks' writes can share the host counter.
- Effigy and Cargo may write their own host state. That is not measured.

### Evidence and raw limits

- Raw evidence cap: 8 MiB total. Logs are truncated with an explicit marker if they exceed the cap. Truncation is reported as partial evidence.
- No environment secrets in evidence. Only the allowlisted names and flags.
- Source contents needed for reproduction stay inside the 16 GiB image. They are not copied to evidence beyond the cap.
- Report every cap, refusal, partial, timeout or sampling gap as its own line. Do not reclassify them as success.

### Stop rules

Stop and report, without retry, if any of the following happens:
- tool or tree identity mismatch;
- effigy does not admit the selector;
- the containment image cannot be created or mounted under the name checks;
- host free space is below 20 GiB or unmeasured;
- image used reaches the 14 GiB threshold;
- a measured invocation fails or the selected test does not pass;
- the 20-minute deadline or the 10-invocation cap is reached.

Stop and report, without changing the design, if the execution brief cannot pin any of the above.

### Single named seam

The one precise missing seam is containment. No shipped Oxide mechanism creates, mounts, measures or enforces a per-comparison footprint. The comparison depends on the manually specified sparse-image procedure above. Launch is blocked until the execution brief names the exact commands and passes the unused-name check. The selector route itself is not a seam, but its admission must be observed, not assumed.

## Evidence limits for this design

- The design rests on the supervisor's reported single-machine observations, a read-only inspection of three Nucleus commits and one stopgap, and two pinned rustc source reads. Its raw logs were not available.
- No Cargo, rustc or test run supports the design. The comparison has not run.
- Reported timings (see [storage efficiency](../storage-efficiency.md#incremental-and-debug-storage-measurements)) are not reproduced.
