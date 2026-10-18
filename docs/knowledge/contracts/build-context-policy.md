# Build-context policy (proposed)

Status: proposed context-resolution design, approved as design direction on
2026-10-15 in [vision](../vision.md#authority-boundaries). No automatic context
resolver is activated by this document. The current [lean local CLI](local-lease.md)
does enforce a bounded payload and retire known idle slots; its profile label
does not set Cargo debug or incremental values. Earlier qualification
mechanisms are retired; their historical findings remain in
[qualification readiness](../qualification-readiness.md). This policy does not
require those mechanisms, and the bounded storage capability remains in use.

Consumer policy remains consumer-owned. Tom adopted incremental-on for Nucleus managed agents and incremental-off for one-shot validation in Nucleus `ff43e69701f08e5a9ddf84d424a381dea8d033de`, superseding its `fb2bd04` agent stopgap. [Storage efficiency](../storage-efficiency.md#nucleus-consumer-adoption-2026-10-15) owns that adoption receipt and its limits. This does not activate Oxide slots or imply a Queue policy change.

## Principle

Choose build context from the expected lifetime of the edit/cache state and the diagnostics the workflow needs. Do not choose it from whether the caller is a human or an agent.

| Context | Expected lifetime | Diagnostics required | Candidate default |
| --- | --- | --- | --- |
| `one-shot` | Candidate, CI-style or throwaway build. Little or no reuse expected. | Test execution, assertions, backtrace file:line (line tables). | Incremental off. Line-table debug. |
| `iterative` | Sustained edit/check/test loop in one retained exclusive slot. | Same as one-shot. | Incremental on. Line-table debug. |
| `evidence` / `full-debug` | Explicitly requested capture or debugger session. | Full debug info, source lookup, native binding. | Explicit opt-in only. Never inferred. |

Human and agent workflows both fit these contexts. A supervised human loop can be `one-shot`. An unattended agent loop can be `iterative`. Identity labels never select the context.

## Resolution order

Context resolution runs before slot selection and lease. The resolved effective tuple is fixed for the lease.

1. **Workflow declaration.** The `oxide` command class and profile ([workflow](workflow.md)), or a consumer's own workflow declaration, names its context. This is proposed. No shipped resolver or CLI flag exists for it.
2. **Explicit evidence request.** `evidence` / `full-debug` is requested explicitly per invocation, for example through the `debug` profile. It is never a fallback or a side effect of another context.
3. **Effective inputs.** Oxide resolves what Cargo and rustc will actually see: profile values, `CARGO_PROFILE_*` and `CARGO_INCREMENTAL` environment, Cargo configuration and `rustflags`, feature set, toolchain provenance, allowlisted environment.
4. **Conflict rule.** An explicit effective input beats a context default. If an explicit input conflicts with the declared context (for example `CARGO_INCREMENTAL=1` in a `one-shot` declaration), the request refuses. Oxide does not silently pick a winner. Silent override of a consumer stopgap is forbidden.
5. **Partition selection.** The resolved tuple chooses the slot. No adaptive switching by history, size or time.

Unknown or ambiguous inputs fail closed.

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

The [local lease](local-lease.md#capacity-and-eviction) already evicts known idle whole slots at lease time and recycles a compatible idle slot at half the cap. Context-aware budgets or retirement policy beyond that is a candidate implementation that needs its own brief.

## Ownership, liveness and release

- Oxide releases a slot only on the writer-closed `release` or `recover` rules of the [local lease](local-lease.md), or, for Oxide's own commands, the qualified recovery mechanism proposed in [workflow](workflow.md#crash-recovery).
- Unknown ownership, crash, PID absence, age, compiler `.lock` files or process disappearance never authorize release or evidence removal. This matches [vision](../vision.md#authority-boundaries) and [architecture](../architecture.md#storage-invariants).
- Budget pressure returns an actionable refusal with an inventory. It does not clean up.

## Upstream garbage collection

- **rustc session GC (verified in rustc 1.97.1 source, 8bab26f4f).** It operates only inside a crate's incremental directory. It removes `s-*-working` session directories older than about ten seconds, finalized session directories except the newest per crate when unlocked, and orphan lock files. It leaves unknown entries alone. It therefore manages incremental sessions and that incremental session build output. It does not manage general `deps` liveness, Cargo variants, or debug-referenced objects outside the incremental session set.
- **Cargo global cache GC.** Covers registry and Git sources. It is not build-artifact GC.

Consequences:

- rustc session GC is not general deps liveness. Neither upstream GC manages Oxide slots, Cargo variants, retained executables with debug references, or `deps` objects.
- A future Oxide fine-grained GC is deferred. It needs a complete pinned liveness analysis for Cargo variants, incremental work products and every retained executable's debug references.
- No periodic clean churn. No selective `*.o` deletion. Unpacked debug info can refer to object files, so removing them can break later debugging and backtraces.

## Debug context

- The everyday line-table default keeps backtrace file:line. Assertions and test execution are not changed by context.
- `full-debug` is explicit. It is required for debugger variable/type inspection and for native evidence claims. It is not required to measure ordinary Cargo cost.
- **Packed-at-capture / dSYM** is a candidate, not a default. It has real costs: dsymutil time, extra bytes, native identity binding between executable and dSYM, and source custody of referenced paths. No retained Oxide mechanism provides that binding.

## Not claimed

- No shipped context resolver, CLI or protocol.
- No shared warm slot or generation retirement beyond the local lease's existing idle-slot rules, and no selective object deletion.
- No global Cargo or rustc configuration change.
- No Effigy change. Effigy stays language-neutral. Oxide resolves Rust context and passes selector and env only.
- No change to frozen Contract010.
- No consumer source, task, admission or settlement ownership.

## Comparison recipe (frozen methodology)

This section defines one bounded comparison. It is authorized by the 2026-10-15 ruling in [vision](../vision.md#authority-boundaries). Normal execution is pinned before launch. Tom subsequently authorized direct planner execution for this comparison in [vision](../vision.md#authority-boundaries), superseding another execution brief. The [direct measurements](../storage-efficiency.md#direct-incremental-comparison-2026-10-15) record the resulting limited edit-loop evidence. This does not rerun the original four-Cargo, 45-minute reuse proof. Production policy stays proposed.

### Callable

Private measurement launcher: `scripts/build_context_comparison.py`. Covering synthetic selector: `effigy build-context:comparison:test`. That selector never enters the live path.

```sh
python3 scripts/build_context_comparison.py
python3 scripts/build_context_comparison.py --plan --resource-plan <planner-owned-json>
python3 scripts/build_context_comparison.py --live --resource-plan <planner-owned-json>
python3 scripts/build_context_comparison.py --live --admitted-root <parent> --resource-plan <staged-plan-json>
python3 scripts/build_context_comparison.py --live --smoke --resource-plan <planner-owned-json>
```

Default is plan/validate. Planned metadata does not prove effects. Live requires `--live` and a planner-owned unused resource plan naming `launch_id`, `parent`, `mount`, `backing` and `nucleus_source`. Registry inputs are optional: absent declared inputs the launcher derives the exact named inputs from the pinned lock metadata (see below), and a missing named input refuses `source_cache_unbound`. Without `--admitted-root` the launcher takes the outer bootstrap role; with it, the launcher takes the admitted inner role. This PR does not pass `--live`.

### Heavy admission and the staged root

The comparison runs admitted through one explicit heavy entry. The launcher's outer bootstrap (no `--admitted-root`) records the session clock, then creates a uniquely owned staged root at the resource plan's `parent`:

- `effigy.toml` — a fixed manifest whose only task is `build-context:comparison:live` with `admission = "heavy"`. Installed Effigy deserialises `[tasks.<name>.run]` through `ManifestManagedRun` (`crates/effigy-manifest/src/task_runtime.rs`): a bare TOML string is **one** command, and a TOML array is a *sequence of separate shell commands joined with `&&`*. Each command is executed with `sh -c` (`crates/effigy-process/src/lifecycle/spawn.rs`). The staged entry is therefore a single string with the absolute interpreter, the launcher and every argument POSIX single-quote escaped; it is never an argv-shaped array. The absolute interpreter is used because a scheduler launch does not rely on a forwarded `PATH`.
- `resource-plan.json` — a full loadable resource plan (`launch_id`, `parent`, `mount`, `backing`, `nucleus_source`, the effective `registry_inputs`, and the pure registry-plan view), so the inner `--resource-plan` loads through the ordinary `load_resource_plan` path.
- `binding.json` — the persisted caller label, request UUID, request-body digest, root, launcher digest, source pins, registry-plan digest, the bound `mode` (`live` or `smoke`) and the bound clock basis.

The outer bootstrap mints a fresh request UUID (or reuses `--request-id`) and constructs the real Effigy submitter; the admitted inner role and every plan/default path construct none. `--caller` overrides the label.

The outer bootstrap then runs the existing top-level selector from that root:

```sh
EFFIGY_CALLER=<label> effigy --host-run-request-id <uuid> build-context:comparison:live
```

Effigy classifies `admission = "heavy"` as heavy (Effigy `src/runner/execute/entry.rs`, `ManifestTaskAdmission::Heavy`; contract 049). Contract 010 fixes the token root as `"the run's canonical cwd"`, so the staged root is the token root. Every nested `effigy check:core:test` call runs with cwd under `<parent>/m/src-<mode>` and therefore satisfies the nested parent check (`current_cwd == token_root || current_cwd.starts_with(token_root)`). No plain-Python outer is left unadmitted, no token is minted, and the root is never widened to all of `/private/var/tmp`.

Before the task command runs, installed Effigy creates its own state root `.effigy/` inside the admitted run root (`locks` from `src/runner/locking/io.rs`, `runtime/tasks/active` and `reports/tasks/...` from `crates/effigy-runtime/src/task_status.rs`). The admitted inner launcher (with `--admitted-root`) refuses `admission_unverified` unless it is passed the staged parent and a present host-run identity. It accepts the staged root only as the exact owned bootstrap: directory mode `0700`, owner `uid`, no symlinks, exactly the three pinned entries with no extra names, and digests recomputed from the reviewed manifest template and the pure registry plan. The single `.effigy` entry is then accepted as supported runtime state, and only when it is a real directory owned by `uid`, not group/world writable, whose top-level entries are all within `{locks, runtime, reports}` and each a real directory. Any other root name, any other `.effigy` name, a symlink or a wrong owner still refuses `staged_session`; the three pinned files keep their own mode/owner/link/digest checks. `.effigy` is never a blanket ignore of unknown content.

### Durable request identity

Before delivery the outer bootstrap persists `binding.json` with the caller label, request UUID, request-body digest, root, launcher digest, source pins and registry-plan digest. It passes the same UUID through the existing `--host-run-request-id` flag. On a lost or ambiguous submit reply it queries the exact caller/UUID with `effigy tasks request status --caller <label> --request-id <uuid> --json`; a found request is then followed with `effigy tasks request follow ...` to its settlement, and an authenticated `unknown_run` (`authenticated_absence`) replays the original argv and UUID (bounded, never a fresh request identity). Every other reply is held and reported as `unknown` with resources retained. Submit, status and follow are deadline-bounded and log the remaining time. Effigy does not persist the caller's body, so the launcher owns that binding.

### Source pin

- Nucleus commit `fb2bd0449ede2d9d2089a47b0559182bd413b91f`, tree `b47caf2ffb7e4430fc5e78411a5b475203126194`.
- `Cargo.lock` sha256 `ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d`. This is identical at reported `af475bc` and `9565ced`.
- Current native `main` resolves to this commit. The pin does not depend on that. Do not assume a later main matches.
- Toolchain: rustc 1.97.1 at `8bab26f4f`, Cargo matching. Nucleus has no `rust-version` and no `rust-toolchain` file. No MSRV is inferred.

### Clone and verification

1. The outer bootstrap owns `PARENT=/private/var/tmp/oxide-build-context-cmp-<launch-id>` as the staged session root before submission. The admitted run accepts that exact root; it never creates a second root. `$PARENT/m` is the mount, `$PARENT/cmp.sparseimage` the backing, both siblings under the owned parent.
2. For each mode, `git clone --no-local --no-checkout <nucleus-path> $PARENT/m/src-<mode>`. This reads the native repository and writes only inside the mount. Never create a worktree in the native repository and never move its refs.
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
2. Ancestor config: Cargo searches the working directory and its parents. For `$PARENT/m/src-<mode>`, check `test -e` for `.cargo/config` and `.cargo/config.toml` in `$PARENT/m/src-<mode>`, `$PARENT/m`, `$PARENT`, `/private/var/tmp`, `/private/var`, `/private` and `/`, and the same two names under the contained `$CARGO_HOME`. Read any file that exists. It stops the run if it sets `build`, `profile`, `rustc`, `rustc-wrapper`, `rustc-workspace-wrapper`, `target-dir`, `build-dir`, `incremental`, `rustflags` or `env`. Presence of an unrelated key is recorded and is not a stop. This is a fixed path list, not whole-host cache or source discovery.
3. Wrapper and override variables are absent by construction. The child environment is an allowlist merge from the parent, not `env -i`. It preserves genuine Effigy/scheduler admission fields from contracts 010 and 049: `HOST_RUN_TOKEN`, `HOST_RUN_ID`, `EFFIGY_HOST_RUN_ROOT`, `EFFIGY_HOST_SCHEDULER`, `EFFIGY_SCHEDULER_OVERRIDE`, `EFFIGY_ADMISSION_TIMEOUT_SECS`, `EFFIGY_HOST_SCHEDULER_RUN_TIMEOUT_SECS`, and the `EFFIGY_ADMISSION_*` budget hints. It does not synthesize a token or envelope. `HOST_RUN_TOKEN` is redacted in ordinary logs. `RUSTC`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `RUSTFLAGS` and `CARGO_BUILD_*` stay dropped. `EFFIGY_INTERNAL_CONTAINER_HANDOFF` is not authority.
4. Actual rustc: in the measured `-v` output, each `Running ... rustc` line must start with the rustc path or rustup proxy resolved in Tool identity step 1, with no wrapper prefix. A different path or a prefix stops the run. This check is part of each measured invocation, so it adds no probe. Missing selector admission is a stop. There is no raw Cargo fallback.

### Selected test and selector

- Route: the existing Nucleus Effigy selector `check:core:test`, which runs `cargo test --offline {args}`. Run it from the mode's source root as `effigy check:core:test -- -p nucleus-core --test packages -v -- --exact committed_workflow_packages_parse`. The first `--` is Effigy's separator. The second passes `--exact` to the test binary. The pinned Nucleus instructions use the same first separator (`AGENTS.md`, `crates/README.md`).
- Selected test: `committed_workflow_packages_parse` in `crates/nucleus-core/tests/packages.rs`. It is a single test. It reads `packages/` at the repository root. It executes and asserts `checked > 0`.
- Effigy admission: the run must show effigy's admission (for example `waiting for QA capacity`) or an admitted start. If the selector is not admitted through effigy, stop. Do not run a direct cargo fallback.
- Tests must execute. The success line must show `1 passed`. No-run output is compile evidence only, and no no-run builds are planned.

### Environment

Use a fixed allowlist merged onto preserved admission identity. Record only variable names and effective values for the flags below. Do not dump other environment values or secrets. `HOST_RUN_TOKEN` is recorded as present and redacted.

Allowlist, both modes:
- `PATH`, `HOME`, `RUSTUP_HOME`: host toolchain identity.
- `CARGO_HOME=$MOUNT/cargo-home`: fresh contained writable Cargo state. `--offline` does not make a host `CARGO_HOME` kernel read-only. The launcher does not chmod, remount, or recursively inventory the operator `CARGO_HOME`.
- Declared `registry_inputs` only: copy exact named files into `$MOUNT/cargo-home/...` under a 4096-file / 512 MiB bound. When the resource plan declares no inputs, the launcher derives the exact plan purely from the pinned lock metadata (committed 038 record, same `Cargo.lock` sha256 `ab2ed097…`): every locked crates.io `[[package]]` maps to `<cache>/index.crates.io-1949cf8c6b5b557f/<name>-<version>.crate` and to the Cargo sparse-index cache file under `<index>/index.crates.io-1949cf8c6b5b557f/.cache/<shard>/<name>`, plus the namespace `config.json`. Only the two canonical named roots are used; any other path refuses. A missing named input refuses `source_cache_unbound`. Unrestricted cache copy, a host `CARGO_HOME` walk, new Cargo metadata probes, namespace fallback and network are not fallbacks.
- `CARGO_TARGET_DIR=$MOUNT/target-<mode>`.
- `TMPDIR`, `TMP`, `TEMP` = `$MOUNT/tmp-<mode>`.
- `CARGO_INCREMENTAL=0` for `off` mode, `=1` for `on` mode. `RUSTUP_TOOLCHAIN=1.97.1`.
- `CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=unpacked`, `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=unpacked`.
- No `CARGO_BUILD_BUILD_DIR`. No shared target. No `RUSTFLAGS`. No consumer-specific environment beyond the listed names. The benchmark's explicit off/on values do not define consumer defaults; the subsequent Nucleus adoption is recorded above.

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
2. Run the exact executable path reported by the measured `-v` output on **stdout+stderr**, under the same admitted helper process scope. Cargo writes `Running` lines to stderr. Accept the unit-graph form `Running tests/packages.rs (target/debug/deps/packages-<hash>)` and the verbose backtick form `Running \`<path> --exact ...\``. Skip rustc `Running` lines. Resolve a relative path against the invocation cwd and `CARGO_TARGET_DIR`, including by dropping a leading `target/` against `CARGO_TARGET_DIR`. Do not pick the first `packages-*` glob or a stale binary. Ambiguous or missing reported paths are `stale_executable_ambiguity` and partial. `RUST_BACKTRACE=1 --exact committed_workflow_packages_parse`. Record the exit code, the expected panic `packages directory`, and a backtrace frame matching `at …tests/packages.rs:<line>` after stripping `panicked at` header lines. The panic header is not a frame.
3. Restore by renaming `packages.oxide-moved` back to `packages/`.
4. Restore the two edited tracked files to the pinned commit with `git -C <src> checkout -- crates/nucleus-core/src/lib.rs crates/nucleus-core/tests/packages.rs`. This removes the E1 and E2 comments. It changes file mtimes, but no further Cargo invocation runs in that clone after U, so it does not affect the measured runs.
5. Verify `git -C <src> status --porcelain` is empty and `shasum -a 256 Cargo.lock` still matches the lock pin. Any remaining difference is recorded as a partial result. Do not repair it with another Cargo invocation.

The clone's state is part of the evidence. It is not reused by any other measurement.

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

- One monotonic start and one UTC audit mark, taken before any staging, live image, source, tool, or evidence preparation. `EPOCHREALTIME` and UTC are not the deadline clock. A UTC jump does not extend the session.
- **Clock basis.** The deadline clock is `time.clock_gettime_ns(CLOCK_MONOTONIC)`; `binding.json` binds the basis string `time.clock_gettime_ns:CLOCK_MONOTONIC`, and the admitted run refuses `clock_basis` if the running clock does not match the recorded basis. This is the genuine shared, non-adjustable native basis: a parent and its spawned child observe the same values on one timeline. It is **not** `time.monotonic_ns()`: on the observed host's Python 3.9.6 `time.monotonic_ns()` reports a process-local value (tens of milliseconds since process start) that does not share a basis between parent and child, so it cannot carry an absolute deadline across processes. Recorded limitation: `CLOCK_MONOTONIC` (like `time.monotonic`) does not advance while the machine is suspended, so a suspend is not charged against the budget. No portability beyond the observed host is claimed.
- Total live budget: 1_200_000_000_000 ns (20 minutes) from that monotonic mark, including staging, capacity wait, probes, builds, tests, measurement and capture. The cap is not reset and not extended. Five seconds are reserved to record partial or abort evidence.
- `binding.json` carries the original absolute UTC start, the absolute monotonic deadline and the clock basis. The admitted run adopts that deadline and never re-bases it on its own start: source-cache planning, bootstrap staging and capacity waiting spend from the same budget, and an admitted entry that begins after the deadline refuses before any effect. A genuine capacity wait that yields no launch is reported `partial` with `never_launched`, not a fresh budget.
- Independent review (up to 20 minutes) is outside this live budget.
- Every subprocess, lock, read, copy and poll uses remaining time against the original deadline. A second operation after expiry refuses before spawn.
- On breach, stop new delivery and terminate only helper-created child process groups (`killpg` of the spawned session, never kill-by-name). If positive writer closure is absent, retain uncertain resources; do not unmount, delete or release. Deadline, refusal, partial, unknown and success stay distinct. This scope is cooperative foreground Cargo/test behavior, not daemon ownership proof of arbitrary descendants.

### Owned footprint and containment

Unused names, exclusive create, owner `uid`, mode `0700`, no symlink/hardlink surprises:

| Role | Path |
| --- | --- |
| parent | `/private/var/tmp/oxide-build-context-cmp-<launch-id>` |
| mount | `<parent>/m` |
| backing | `<parent>/cmp.sparseimage` |

The backing file is a sibling of the mount directory. It must not reside under its own mountpoint. Source clones, off/on targets, temp, contained `CARGO_HOME`, and retained evidence live inside the mount.

- Image: one fresh APFS sparse image via pinned `/usr/bin/hdiutil create -size 15g -type SPARSE -fs APFS`, `attach -nobrowse -mountpoint`, and `imageinfo`, with finite size-field parsing. Create size is 15g so backing metadata, control output and any host-side evidence temp stay inside the approved 16 GiB **owned allocated total**. Do not raise 16 GiB. Failure branches in tests inject stub `imageinfo` metadata; this PR creates no image.
- Hard ceiling: allocated bytes of the backing file plus host-side control/evidence extras, and `imageinfo` non-empty bytes, must stay ≤ 16 GiB (`17179869184`). Lower contained capacity honours that total.
- Sampled threshold: abort or refuse when image used space reaches 14 GiB. Sample at a fixed 15s monotonic cadence (`SAMPLE_CADENCE_NS`) from the helper wait loop, plus boundary samples at preparation and after each mode. Record `peak_image_used` and `min_host_free`. A cadence breach terminates only the helper-created child process group, then refuses. Write errors, quota errors, and missing/unknown samples stop the run.
- Host reserve: refuse start if measured host free space is below 20 GiB or unknown. Sample it before the run, at each cadence/boundary sample and after the run. Abort on breach.
- Not allowed: a second volume, spare, fallback, full cache copy, recursive operator `CARGO_HOME` size scan, or old-target read/delete. No global quota or settings change.
- After evidence capture the image stays until independent review. Detach happens only after settlement with positive writer closure. Unknown closure retains resources. Image file deletion is a later separate decision.

### Measurements

Per invocation:
- command, cwd, effective env names and flag values, Cargo and rustc versions;
- the config check results from "Loaded Cargo configuration" and the `Running ... rustc` path observed in `-v` output;
- UTC start and end, monotonic duration, exit code;
- `Compiling` and `Fresh` counts from `-v` output;
- the test result line;
- the rustc flag lines for the workspace crates (bounded excerpt).

Per target and per mode, after each invocation:
- per-directory logical bytes, allocated bytes and file counts for `target/debug/deps` and `target/debug/incremental`, and the union across both deduplicated by `(st_dev, st_ino)`; a hardlink is counted as a file twice and added to allocated bytes once;
- file count under `deps/` and count of `*.o` there;
- `.dSYM` count (expected zero under unpacked);
- a truncated walk budget marks the measurement `partial`, never a positive result.
Walks are bounded in file count and time and stay inside the owned parent/mount. Host `CARGO_HOME` is not a walk root.

Per measured invocation, positive proof requires: outcome `ok`, exit `0`, the exact selected-test line `test <name> ... ok` and `test result: ok. 1 passed; 0 failed; …`; every rustc `Running` line matching the resolved `rustc` path with no wrapper prefix; `-C debuginfo=line-tables-only` (or `1`) and `-C split-debuginfo=unpacked` on every rustc invocation; `-C incremental` present in `on` and absent or `off` in `off`; the expected workspace `Compiling nucleus-core` on C0/E1/E2; and, on U, no workspace `Compiling` while the test still executes. U is a no-op rebuild, so no rustc `Running` line is required there. A missing or truncated critical flag or test identity is `partial`, not a pass.

Per mode:
- image used and free before, after and sampled;
- host free before, after and sampled;
- attributable writes: unknown. The only available counters are machine-wide and shared with other workers. They are recorded as an unattributable host delta, if the existing non-privileged read works. They are not per-run write counts.

Limitations to state with results:
- Hardlink-aware `(dev,ino)` accounting does not prove APFS clone or snapshot sharing is absent or cannot confound allocation, including when this recipe created none.
- Effigy and Cargo control-plane state is separately identified and is not claimed as per-run SSD I/O. Attributable writes remain unknown unless an existing non-privileged counter is trustworthy.
- Raw logs cap at 8 MiB with an explicit partial marker. Critical failure and selected-test identity keep a reserved tail so cap exhaustion cannot silently drop them.
- Stream reads are deadline-aware and bounded; a child that exits while a descendant still holds the pipe is drained only within a bounded grace and reported with `writer_closed=false` (unknown closure, resources retained), never inferred closed from the parent's exit. Large output cannot grow captured buffers past the cap or block past the deadline. Only helper-created process groups are terminated; no process-name kills and no claim about arbitrary descendants.

### Evidence and raw limits

- Raw evidence cap: 8 MiB total. Logs are truncated with an explicit marker if they exceed the cap. Truncation is reported as partial evidence.
- No environment secrets in evidence. Only the allowlisted names and flags.
- Source contents needed for reproduction stay inside the contained mount, inside the 16 GiB owned footprint. They are not copied to evidence beyond the cap.
- Report every cap, refusal, partial, timeout or sampling gap as its own line. Do not reclassify them as success.

### Stop rules

Stop and report, without retry, if any of the following happens:
- tool or tree identity mismatch;
- effigy does not admit the selector;
- the containment image cannot be created or mounted under the name checks;
- host free space is below 20 GiB or unmeasured;
- image used reaches the 14 GiB threshold;
- a measured invocation fails or the selected test does not pass;
- the 20-minute monotonic deadline or the 10-invocation cap is reached (in-flight helper children are terminated; new delivery does not wait for a build to finish).

Stop and report, without changing the design, if the execution brief cannot pin any of the above.

### Single named seam

The launcher binds the 050 gaps: whole-session monotonic deadline, sibling backing file, 16 GiB owned total including overhead, admission-preserving environment, counted deliveries before spawn, declared registry inputs instead of a host `CARGO_HOME` walk, exact reported-executable location from both streams, backtrace frame vs panic header, post-restore porcelain/lock verification, live union accounting, and 15s cadence 14 GiB / 20 GiB peak sampling. The 051 follow-up binds five concrete integration defects: one explicit heavy staged entry with the submission cwd as the authenticated token root; caller/UUID/body/root binding persisted before delivery with authenticated-absence-only replay; a pure lock-to-archive/index registry plan over the canonical namespaces; positive per-measurement proofs on `target/debug/deps` and `target/debug/incremental`; and deadline-aware bounded stream reads. Reusable-launcher use still needs an unused owned resource plan and its normal reviewed scope. Tom separately authorized direct planner execution of this comparison; its disposable recipe is evidence, not the product launcher or a production qualification.

Missing declared registry inputs, an unbindable named copy, or an unsupported Effigy inheritance seam is a refuse with that seam name. Do not invent unrestricted cache copy or bypass admission.

### Attempt record (2026-10-15)

One checked launch packet ran this recipe through the outer bootstrap on
2026-10-15 (caller `oxide:planner:incremental-comparison:053`, request UUID
`be2acbd8-7f3e-4488-b91d-6781ec0d6788`). The heavy host run was admitted and
launched by Effigy, then settled `failed` after 629 ms: the inner comparison
launcher failed before its own admission validation, with no Cargo or rustc
invocation. The staged manifest's `run` array was executed by Effigy as a
sequence of shell
commands rather than as one argv vector, so the non-executable launcher script
was executed directly (`Permission denied`, code 126). The recipe above stays
frozen and unchanged. This attempt supplied no measurement; the correction
and later direct observations are separate. See
[storage efficiency](../storage-efficiency.md#bounded-incremental-comparison-attempt-2026-10-15)
and the [attempt evidence record](../evidence/build-context-comparison-053-attempt.json).

### Bootstrap correction and compiler-free smoke (2026-10-15)

A separately scoped producer correction (`oxide#054`) fixed the three observed
050/051 integration defects, then proved the corrected bootstrap with one
compiler-free heavy run. It did not run the comparison and creates no
comparison result.

1. **Command shape.** The staged manifest now emits one `sh -c` command string
   with the absolute interpreter, launcher and every argument POSIX
   single-quote escaped (`run = "…"`), not a TOML array of argv words. A
   synthetic shell-execution fixture runs the generated command through
   `/bin/sh -c` with spaces, quotes, `;`, `$`, `*` and a newline and asserts the
   child receives the exact argv. Installed Effigy `--plan` on the generated
   selector reports the same single command.
2. **Runtime state.** `.effigy/{locks,runtime,reports}` created by installed
   Effigy in the admitted root is accepted narrowly, as described above. Bad
   root, tamper, symlink, wrong-owner and extra-entry cases still refuse.
3. **Clock.** The deadline clock is now
   `time.clock_gettime_ns(CLOCK_MONOTONIC)` and its basis is bound in
   `binding.json`. A native parent/child regression proves the shared basis, a
   delayed child adopts the original deadline without a reset, and an entry
   that begins after the original deadline refuses before effects.

One genuine compiler-free smoke ran through the installed Effigy heavy
admission: caller `oxide:planner:bootstrap-smoke:054`, request UUID
`e09afaa3-0694-47a4-996a-c4077a413f60`, staged root
`/private/var/tmp/oxide-build-context-cmp-smoke-e09afaa3`, mode `smoke` bound
in the staged request. The heavy host run (`2ba120c93a39c5468e937e594a63c138885bccf3a796db0b4f54c111208a2a03`,
epoch 1) settled `passed` with exit `0`. The admitted inner role reached
`verify_admission` and `validate_staged_session` against the real installed
runtime state, adopted the original deadline
(`25892414356000`, remaining `119465832000` against the parent's
`119990944000`) and returned the smoke sentinel
`oxide-build-context-smoke-reached-v1` before any image, source, registry,
tool or compiler operation (`count_delivered=0`, `effects_executed=false`,
empty helper list). The reserved mount and backing names were never created.
The smoke is not a comparison result and no Cargo or rustc ran. Both the
staged root and the smoke control directory are retained read-only for review;
nothing was deleted, detached or repaired. Receipt:
[smoke evidence record](../evidence/build-context-comparison-054-smoke.json).
The smoke supplies no timing/storage comparison. Tom later directed the
planner to measure directly rather than dispatch another execution brief;
[storage efficiency](../storage-efficiency.md#direct-incremental-comparison-2026-10-15)
owns those observations.

## Evidence limits for this design

- The design rests on the supervisor's reported single-machine observations, a read-only inspection of three Nucleus commits and one stopgap, two pinned rustc source reads, and the installed Effigy source and contracts 010/049 for the heavy classification and token-root check. Its raw logs were not available.
- Synthetic fixtures cover launcher enforcement, staging, admission and submission with stub adapters. They are not a live comparison. The genuine smoke proves bootstrap entry only. The separate direct run supplies narrow real edit-loop evidence, not production qualification.
- The supervisor's larger compile-only timings are not reproduced. The direct executed-test workflow is narrower and uses comment-only edits, with retained raw output.
