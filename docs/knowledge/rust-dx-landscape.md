# Rust development experience: research landscape

Read-only research findings and candidate remedies for Rust build, check and test feedback in Oxide's development workflow. This file owns the sourced findings, their evidence limits and the candidate remedies that remain open. [Vision](vision.md) owns the desired experience. Queue and Nucleus own plans, briefs and execution. [Workflow](contracts/workflow.md) owns the shipped command contracts, and [qualification readiness](qualification-readiness.md) owns the storage correctness findings.

Findings are dated 2026-10-09 to 2026-11-12 and apply to Cargo 1.97.1 on macOS arm64 unless stated. Check tool behaviour on the pinned version before adopting anything. No candidate here is a selected default or permission to change a repository.

## Evidence limits

Measurements come from one macOS arm64 host on APFS, with small subjects, one or a few samples per arm, and a shared, loaded machine. Treat each timing as an observation, not a benchmark. Survey and upstream statements are cited as published and are not measurements of this host. Apparent size is not allocated storage, occupied-space deltas are not write volume, and nothing here establishes SSD writes or SSD lifetime.

## Evidence and implications

- The Rust Project's [2025 compiler performance survey](https://blog.rust-lang.org/2025/09/10/rust-compiler-performance-survey-2025-results/) separates incremental feedback, type checking and IDE use, clean and CI builds, linking, debug information and editor contention. Several workflows need separate remedies, not one cache.
- Cargo's [2026 cross-workspace cache goal](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html) is an experimental upstream direction for shared build units. It may not stabilise in 2026, and it is not a supported cache today ([cargo#5931](https://github.com/rust-lang/cargo/issues/5931) is open).
- `build.build-dir` (`CARGO_BUILD_BUILD_DIR`) has been stable since Cargo 1.91 ([changelog](https://doc.rust-lang.org/cargo/CHANGELOG.html)). It relocates intermediates, but the layout is internal and may change ([build-cache reference](https://doc.rust-lang.org/stable/cargo/reference/build-cache.html)). Alone it gives no lease, bounded storage, cross-path proof, cleanup policy or run evidence. A global `CARGO_TARGET_DIR` shares final output and cleanup scope and supplies none of those rules either.
- Oxide's mechanism is build-dir redirection behind its own lease, budget and consumer adapter. Do not reimplement Cargo fingerprints or artifact selection. Keep the redirection replaceable, and revisit it when a supported Cargo release documents shared-unit behaviour and exact-head tests prove the same correctness, lifecycle and ownership. Roadmap acceptance or nightly availability is not replacement evidence.
- On Cargo 1.97.1 nothing supported shares compiled dependency units while keeping workspace, build-script, incremental and test outputs private, so a dependency-only compiled cache is not a next delivery. Queue Spec 028 shares `CARGO_HOME` downloads, which is source reuse, not compiled-artifact reuse.

## Problem and solution map

Each remedy is a candidate for qualification, not a selected default.

| Recurring problem | Existing mechanism | Candidate and limit |
| --- | --- | --- |
| New worktrees repeat compilation and multiply outputs | Cargo build-dir separation; cross-workspace goal | Bounded reusable storage with durable exclusive leases. Start with project or component partitions and prove cross-path correctness first. |
| Storage fills, and cleaning destroys useful state | Cargo source GC differs from build GC | Explain occupied, active, uncertain and reclaimable storage. Budget owned slots and refuse unsafe allocation. Age is not proof of safe deletion. |
| Incremental builds grow or lose their benefit | [Cargo incremental settings](https://doc.rust-lang.org/cargo/reference/profiles.html#incremental) | Compare sustained edit cycles with and without incremental per workload. No blanket disable or clean policy. |
| Unexpected rebuilds | [Cargo FAQ](https://doc.rust-lang.org/cargo/faq.html#why-is-cargo-rebuilding-my-code) | Explain the changed inputs and their cost. Separate a valid miss from failed reuse. Never call a real compiler error cache corruption. |
| Debug artifacts dominate time and storage | [Cargo performance guide](https://doc.rust-lang.org/cargo/guide/build-performance.html) | Qualify distinct feedback, test and debugging intents. Keep backtraces, assertions and full debugging. Measure writes, not only binary size. |
| Link time dominates small edits | Performance guide: LLD, mold, wild | Detect the real linker and benchmark supported options. Linux results do not establish macOS Mach-O benefit. |
| Large graphs, feature drift, duplicate versions | [Cargo timings](https://doc.rust-lang.org/cargo/reference/timings.html) | Report expensive units, duplicate versions and feature variants. Feature changes are project decisions because they can alter behaviour. |
| Build scripts and proc macros repeat work | [Build-script change detection](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection) | Diagnose rerun causes. Never hide relevant file or environment changes to manufacture hits. |
| Generic code and macros cost compiler work | [Rust Performance Book](https://nnethercote.github.io/perf-book/compile-times.html) | Connect bottlenecks to project advice. Crate-boundary changes need owner review. |
| Editor checks compete with builds | [rust-analyzer target dir](https://rust-analyzer.github.io/book/configuration.html#rust-analyzer.cargo.targetDir) | Integrate editor checks into owned storage without deadlocks or surprise builds. Check and build outputs are not assumed reusable. |
| Test feedback is slow or opaque | [Nextest archives](https://nexte.st/docs/ci-features/archiving/) | Use exact-run test bundles and reporting rather than a new runner. Doctests and required coverage stay explicit. |
| Container builds duplicate caches | [BuildKit cache guidance](https://docs.docker.com/build/cache/optimize/); [cargo-chef](https://github.com/LukeMathWalker/cargo-chef) | Coordinate with Effigy on Linux routes and Colima storage. Image layers do not establish safe live worktree sharing. Host and Linux artifact identities stay distinct. |
| Toolchain and target setup is fragile | [rustup overrides](https://rust-lang.github.io/rustup/overrides.html); [cross](https://github.com/cross-rs/cross) | Show the effective tools, configuration and missing prerequisites. No silent global mutation. |
| Dependency checks are scattered | [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) | Optional coherent reporting. Repository policy stays authoritative. Automatic upgrades are out of scope. |
| Async behaviour is hard to observe | [Tokio console](https://github.com/tokio-rs/console) | Explore optional integration. It is Tokio-specific. |

## Cargo build-message boundary

Cargo's [JSON message contract](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages) is an observation surface, not a complete record. Artifact rows separate up-to-date artifacts from steps that ran rustc, but they do not give the full configuration or why a unit became dirty. Build-script result rows can come from cached results, so they do not prove a script reran. Build-finished success covers only the build stage, and tests can still fail afterwards. Message counts are not a unit graph, elapsed time or write volume. A retained log is user-supplied diagnostic input, not exact-run QA evidence. Unrecognised, mixed, malformed or truncated input needs explicit coverage limits, not guessed hit rates or rebuild causes.

## Cargo command interception

Cargo offers no supported way to replace its own built-in commands. Interception must happen before Cargo (a front door) or through Cargo's execution hooks (inner). Probed on Cargo 1.97.1 and rustup 1.29.1 on 2026-10-18:

- An `[alias] build` entry is ignored with a warning because the built-in shadows it. A `cargo-build` binary on `PATH` is silently shadowed.
- A `cargo-oxide` binary on `PATH` catches only `cargo oxide …`. Aliases `b` and `r` catch only `cargo b` and `cargo r`.
- A rustup path toolchain whose `cargo` is a shim catches `cargo build` and `cargo run`, but `cargo +stable` bypasses it. It must supply every tool, and it is machine-specific, so it must not be committed.
- Inner hooks (`build.build-dir`, `build.target-dir`, `build.rustc-wrapper`, `target.<triple>.runner`, `[env]`) reach nested Cargo calls. They are the execution mechanism behind explicit commands.

A front door handles the experience, and inner hooks keep execution in Oxide-owned storage. Child tools call real cargo through `CARGO`, so coverage depends on environment injection, not on intercepting every process. A shim must call the real cargo by absolute path and mark re-entry, or `RUSTUP_TOOLCHAIN` causes recursion. rust-analyzer reaches cargo through the same routes unless `check.overrideCommand` and `cargo.buildScripts.overrideCommand` are set. Tom's 2026-10-18 follow-up defers transparent interception in favour of explicit `oxide` commands plus agent guidance ([workflow](contracts/workflow.md)). No interception mechanism is installed.

## Experimental frontier

- Cargo's performance guide describes nightly parallel frontend and feature-unification experiments. They need opt-in qualification and cannot silently change test or runtime semantics.
- [Cranelift](https://github.com/rust-lang/rustc_codegen_cranelift) is a faster-codegen candidate with platform limits. Its macOS support excludes panic unwinding, and its default is `panic=abort`.
- JIT and alternative execution are in product research scope under [vision](vision.md). Cranelift's JIT mode is highly experimental, has no incremental compilation and needs dynamically available dependencies. These are qualification questions, not grounds to exclude JIT.
- A comparison should cover representative edit, check, run and test loops. Count preparation and dependency builds, time to first feedback, repeated-edit latency, disk writes, memory and swap, and restart effort. Attribute exact source, backend and execution mode, and run required checks on the production backend.
- Remote execution cuts local compilation but adds transfer, credentials, latency and a new failure surface. RAM-backed intermediates cut some writes but add memory pressure and volatility. Neither is selected. Users should not need to administer infrastructure for the core experience.

## Stable profile and backend eligibility

- Cargo's build-performance guide suggests workspace line tables, dependency debug disabled, and an explicit full-debug route as a possible faster or smaller trade-off. The light build loses dependency debugging. This is a candidate to measure, not a benefit proven on every project.
- Cargo's [profile override rules](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides) make `package."*"` apply to non-workspace members. Named overrides, build overrides, configuration and environment precedence still matter.
- A light profile can keep dev, test and release defaults intact while preserving incremental compilation, assertions, overflow checks and unwinding. A full-debug profile must resolve full debug for every unit. A profile name alone is not proof. Recipes are owned by [stable Cargo profiles](contracts/stable-cargo-profiles.md).
- Cranelift's [JIT mode](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md#jit-mode) rules out a drop-in replacement for an unwind-dependent test loop. A narrower run-only experiment remains possible.

## Quick-run strategies

These are the `quick` run strategies in the [workflow](contracts/workflow.md#profiles-and-strategies). None is ready to replace quick build followed by execute as the default.

| Strategy | State | Fit for `oxide run --profile quick` |
| --- | --- | --- |
| Quick build and execute (LLVM, opt-level 0, line tables, incremental) | Shipped. On Linux, [Rust 1.90 made LLD the default linker](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/). | Default and fallback. |
| [Cranelift AOT](https://github.com/rust-lang/rustc_codegen_cranelift) | Nightly preview. Per-package `codegen-backend` lets dependencies stay on LLVM. | Not recommended for `run` on small workspaces (below). Not for `test`. |
| [Cranelift JIT](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md) | Highly experimental. | Not a near-term candidate. |
| Hot patching ([Subsecond](https://docs.rs/subsecond/)) | Experimental. Patches only code in `subsecond::call` and the tip crate. | Not generic. It needs source changes, so it is a possible later opt-in for long-running apps. |

### Cranelift measurement (2026-10-18)

One bounded session compared stable LLVM, nightly LLVM, and nightly with Cranelift for the storage crate only. Warm builds took about 0.31 s in every arm, and the Cranelift arm was 0.01 s slower than LLVM. Warm checks took about 0.10 s, so the build step adds most of the edit-loop cost. Cranelift also changed panic behaviour: a `catch_unwind` inside Cranelift-compiled code did not catch an overflow panic, even with `panic=unwind`. A second nightly dependency set cost about 63.5 MiB beside stable's 67.2 MiB. Recommendation: no Cranelift-for-workspace quick strategy on this evidence. A heavy, codegen-bound project needs its own reading.

### Relax engine (2026-11)

[Relax](https://github.com/makepad/relax) is a dependency-free JIT compiler and runner that replaces the rustc, LLVM, linker and Cargo pipeline for Cargo packages. Two pinned revisions were tested through a bounded, uncounted apparatus that is no longer in the tree. At `aa540c14` (2026-11-11), the run stopped on 20 unresolved `core::arch::aarch64` intrinsics in `zmij`, the float formatter behind the pinned `serde_json`, and then on the guard's owned-scratch budget. At `6ea9d063` (2026-11-12), the float blocker was cleared and the CRC32 probes matched, but the unchanged Oxide workload panicked in Relax's front end while parsing `libc`. No timing or performance comparison was measured. Both are scoped compatibility no-go results for the tested pins and the arm64 macOS setup. Relax is not integrated, adopted or installed, and it is not a Cargo replacement. A revisit needs an upstream fix for the front-end panic and its own approved scope.

## Other run-path levers

Any rustc-based JIT or interpreter still runs parsing, macro expansion and type and borrow checking, so check time is the floor. A measurement must report check, build and link time separately.

| Lever | Finding and fit |
| --- | --- |
| No-op fast path (skip Cargo when the last build's identity is unchanged) | Not adopted. The prototype did not pass its fixed timing gates ([run overhead](#oxide-run-overhead-profile-2026-10-19)). |
| Optimised dependencies in `quick` (`package."*".opt-level`) | Not for the default recipe. On Oxide's workspace it added about 3.3 s (+76–79%) to each cold slot and gave no test-runtime gain. It also optimised build-time units. |
| Workspace feature unification | Packages built together can build dependency variants with different features. On 1.97.1 the resolver setting needs `-Zfeature-unification` ([RFC 3692](https://rust-lang.github.io/rfcs/3692-feature-unification.html)). The stable route is a workspace-hack crate, a project change. `oxide status` could report duplicate variants. |
| Faster linkers | macOS uses Apple's linker. [Wild](https://github.com/davidlattimore/wild) is Linux-only, has no incremental linking and has no Mach-O port. Linux already defaults to LLD. |
| Dynamic dependency linking | A dylib wrapper cuts link time for large graphs but changes linking and loading. Per-project opt-in only. |
| Parallel frontend (`-Zthreads`) | Nightly. No warm edit gain on Oxide's workspace, with the same dependency-set cost. |
| Watch mode | Cuts latency, not work. Low value for agents. |
| [danluu/rust-interpreter](https://github.com/danluu/rust-interpreter) | Experimental. It cannot replace `cargo test`, has no unwinding, threads, FFI or `should_panic`, and states no license. Not adoptable. |
| Miri | A UB checker, far slower than native code. Not a speed tool. |

## Upstream churn and disk developments (2026-10-18 to 2026-10-20)

- **Build-dir layout v2** moves intermediates into per-unit `build/<package>/<hash>/` directories. It is scheduled for stable in Cargo 1.100 (2026-11-12, per the beta changelog). Oxide has not qualified 1.100. The layout unblocks Cargo's stale-unit cleanup ([cargo#5026](https://github.com/rust-lang/cargo/issues/5026)), and Oxide's stray detection must be requalified when it lands.
- **Relink, don't rebuild** claims 5–10× fewer downstream rebuilds for body-only edits. It did not complete in 2025H2, and there is nothing safe for Oxide to emulate yet.
- **`hint-mostly-unused`** is a nightly per-dependency hint that regresses if applied globally.
- **Cargo home GC** (Rust 1.88) cleans downloaded and cached files automatically. It never runs with `--offline` or `--frozen`, and it does not cover the target directory. See [cleanup triggers](#cargo-home-cleanup-triggers-2026-10-18-probe).

## Stale debug-object measurement (2026-10-18)

Measured on Oxide's workspace with ten edits per arm. Limits: one small subject, one edit shape and a shared host.

- With the default unpacked split debug info, each rebuild added about 411 `*.rcgu.o` names to `deps/`. Most were hardlinks to live `incremental/` products. The growth was mostly names, not bytes, and it did not slow warm builds within ten cycles.
- `split-debuginfo = "packed"` stopped accumulation at about 0.16 s per rebuild and kept file:line backtraces, with about 15 MiB more target space per slot.
- Turning off workspace debug info also stopped accumulation at no measurable time cost, but workspace frames lost file:line. It is rejected for `quick`.

Recommendation: keep `quick`'s line tables. If a fix is adopted, use packed debug info. Do not delete files inside a slot, because the build-dir layout is Cargo-internal. Whole-slot recycling stays under the ownership policy and was not tested.

## Cargo home cleanup triggers (2026-10-18 probe)

Source-read rules for Cargo 1.97.1, confirmed by one session with a closed proxy.

- Automatic cleanup runs only at the end of an online build, check, run, test, doc, fix, install, package or fetch that gets past resolution and download. `metadata`, `tree`, `update`, `clean` and `add` never clean.
- It is skipped under `--offline`, `--frozen` or `net.offline`. `--locked` alone does not skip it. A held cache lock also skips it silently.
- The default frequency is one day, checked against `last_auto_gc`. Creating the database primes that value, so a new home waits at least a day. The value cannot prove that a cleanup ever ran.
- On stable, the fixed thresholds are one month for extracted sources and three months for downloads and indexes.
- Each pass first syncs the database with disk. A registry index or git database that is tracked but missing from disk has its child trees queued for deletion without an age test. Deleting `registry/index/<registry>` by hand therefore makes the next pass remove that registry's recent `.crate` files and sources too.
- `cargo clean gc` is nightly-only on stable.

Oxide's rules ([workflow](contracts/workflow.md#cargo-home-hygiene)): `oxide status` reports the Cargo home's size but not a last-cleanup time, because that record lives in Cargo's internal database. `oxide clean --cargo-cache` asks Cargo for one pass with `cache.auto-clean-frequency="always"`, without `--offline` or `--frozen`. Oxide never deletes inside the Cargo home itself. Commands run with `--offline` or `--frozen` never clean that home.

## Cloning a warm build directory (2026-10-18 probe)

An APFS clone of a warm build directory stayed relocatable for Cargo freshness on a fixture and on Oxide's repository, on both layouts. Cargo does not keep the old path in the files it checks. Some files still name the source path, though, including dep-info for unchanged units and serde source maps. Debug maps keep pointing at the source slot's `deps/*.o` until a binary is relinked, so once that slot is gone or rebuilt, debugging an unchanged binary can lose its debug info. No debugger was run. Clone a slot per worktree only while the source slot stays unchanged, or with an accepted relink.

## Final-output placement (2026-10-18 probe)

On the same APFS volume, Cargo cloned uplifted final outputs instead of copying them. On one subject this saved about 14.8 MiB per worktree compared with a target directory on another volume. Test harness executables were not uplifted, so only `build` and `run` outputs benefited. Linux was not measured. Moving final outputs into the pool would change the privacy and lifetime rules in [workflow](contracts/workflow.md#what-one-command-does-local-executor).

## oxide run overhead profile (2026-10-19)

A prototype skip path and a normal leased no-op were profiled on a std-only fixture. The skip branch was not merged. Its fixed gates failed: it ran slower than a plain build-only no-op beyond the noise allowance, and the ordinary leased path fell by about 35% against a target of half. Most of the time before the pre-exec marker went to child Cargo and rustc work, Git reads and ledger saves. The cost after exec was not isolated. Tom's 2026-10-19 ruling paused the skip path, and its build-script readers were withdrawn. The branch is kept as knowledge only.

## Oxide normal-command overhead, round 2 (2026-10-19)

A like-for-like test of Oxide's added time on a normal no-op against plain Cargo ended in no-go. The author's candidate passed one gate. The independent exact-head review failed both: `oxide check` exceeded `cargo check` by 136 ms against a 100 ms ceiling, and `oxide run` exceeded `cargo run` by 107 ms. That failure decided the result. The review also found a toolchain cache that could pair old version output with a replaced compiler, a background snapshot that could start before the lease, and insufficient bounds evidence. The branch is a knowledge donor only.

## Oxide normal-command overhead, round 3 (2026-10-19)

A paired retry with round 2's reductions failed the author's own check gate, with a median paired difference of 143 ms against a 100 ms ceiling. The run gate passed at 82 ms. The brief stopped at the first measured failure, so no independent product review ran. Nothing from this branch is merged or installed.

## macOS new-executable launch cost (2026-10-19)

A newly written executable pays about 100 ms on its first launch, even with identical content and even as an APFS clone. Re-running the same file costs about 2 ms. This matches macOS first-execution assessment (XProtect or Gatekeeper). The documented host setting, adding the launching app under Privacy & Security → Developer Tools, is reported as incomplete by some users. Do not disable Gatekeeper globally. For a rebuilt binary the penalty is likely a lower bound.

### oxide launch check runs (2026-10-19)

`oxide status --launch-check` ships this measurement (oxide#093). It is opt-in and advisory. Author runs were noisy: some hit the 3 s limit and reported `inconclusive`, and the penalty persisted after the launching app was listed. A two-line `#!/bin/sh` script paid a similar first-launch cost, so the effect is not specific to Mach-O. The cause was not traced.

## sccache (2026-10-20)

Source and probe findings for sccache 0.18.0. Compatibility is unknown and benefit is insufficient, so no sccache integration is adopted. The open question is tracked as [Q-017](questions.md#q-017--can-sccache-sharing-preserve-oxides-slot-writer-closure).

- The default shared daemon sits outside every Cargo process group and gives no per-request closure after cancellation. That makes it a no-go for release or recovery under current Oxide semantics.
- Client-side mode (`SCCACHE_CLIENT_SIDE`) keeps each cache writer inside the build's process group. It is supported by source but was not exercised natively. It silently turns off when `SCCACHE_LOG` is set or a distributed scheduler is configured, so an integration must own the endpoint and scrub the client environment.
- Rust cache hits depend on the target path. Each Oxide slot has its own path, so hits across slots happen only when the paths match. `SCCACHE_BASEDIRS` did not normalise this for Rust in the tested build. As shipped, sccache adds little to Oxide-managed builds.
- Linking crates are not cached, incremental units are refused, and the key includes the working directory and `CARGO_*` variables. Write volume is unknown, and cache bytes are not writes.
- A candidate complement is a fixed per-index build-path alias combined with client-side mode. It needs proof that Cargo does not canonicalise the alias, and an account of debug-info path effects.

## Rust 1.98–1.100 relevance (2026-10-20)

- Cargo 1.98 stabilised nothing relevant. New Clippy lints can fail `-D warnings` runners.
- Cargo 1.99 adds a built-in `debug` profile that matches `dev` for now, as preparation for moving `dev` away from full debugging. Incremental compilation is off by default when `CI` is set. There are no build-dir, GC, linking or debug-info default changes.
- Cargo 1.100 (2026-11-12) makes the new build-dir layout the default. That unblocks artifact GC and cross-workspace caching, and Oxide's stray detection depends on it. Oxide has not qualified it.
- Each rustc change gives every slot a new compatibility key, so a toolchain bump costs a cold cycle. Moving from 1.97 to 1.99 brings no measured speed or disk benefit. A single bump straight to 1.100 costs one cold cycle instead of two.

## Product assessment

Planner recommendation: a small, coherent entry point over evidence-backed mechanisms, with three capabilities. It should show and explain costs, bound and reuse owned storage, and apply qualified improvements with clear trade-offs. The reusable storage core must serve ordinary developers as well as orchestrators. A daemon, editor surface or UI is still an open choice. Tom's [workflow direction](vision.md#product-workflow-direction) sets an explicit `oxide build` and `oxide run` front door as the design target.

The broad product goal is confirmed. Numeric targets and mechanism choices are not. For each candidate, compare cold, new-worktree, small-edit, branch-switch and test cycles, and longer-lived use. Track feedback latency, peak and retained disk, observed write volume, memory and swap, contention and manual interventions. Do not infer SSD lifetime from these metrics.

Ease of use should weigh setup steps, configuration burden, diagnostic clarity, predictable behaviour under capacity pressure and recovery effort. Prefer fewer developer decisions over exposing every compiler knob. Required source, test and release behaviour stays visible. This research implies no command names or installed behaviour, and executable naming remains subject to [release](contracts/release.md) review.
