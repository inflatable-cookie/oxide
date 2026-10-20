# Rust development experience: research landscape

Read-only research, checked 2026-10-09. This file owns findings and candidate remedies, not the project plan. [Vision](vision.md) owns the desired experience; Queue owns order, briefs and execution authority. Oxide has run bounded qualification work; [qualification readiness](qualification-readiness.md) records its scope and limits. Tool availability and compatibility must be checked on selected pinned versions before adoption.

## Evidence and implications

The Rust Project's [2025 compiler performance survey](https://blog.rust-lang.org/2025/09/10/rust-compiler-performance-survey-2025-results/) identifies distinct problems in incremental feedback, type checking/IDE use and clean/CI builds. It also highlights linking, debug information, editor/build contention and limited visibility into build costs. This supports researching several workflows rather than assuming one cache can solve them all. Survey findings are not measurements of Tom's machine or a diagnosis of his incremental-cache degradation.

Cargo's accepted [2026 cross-workspace cache goal](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html) seeks shared build units, reduced duplication and eventual cleanup. Its stated sequence is to design and implement an experimental nightly cache, initially excluding build scripts and proc macros, then expand coverage and work toward stabilization; the goal says stabilization may not finish in the 2026 period. The linked [Cargo cache tracking issue](https://github.com/rust-lang/cargo/issues/5931) remains open and records design work. This is a real upstream direction, but it does not establish a supported stable cross-workspace cache, its exact current nightly coverage, or fit with Queue/Nucleus ownership.

Cargo does provide a useful stable building block today: the [Cargo 1.91 changelog](https://doc.rust-lang.org/cargo/CHANGELOG.html) (2025-10-30, stabilize `build.build-dir` #15833 #15840) marks `build.build-dir` stable, and the [configuration reference](https://doc.rust-lang.org/stable/cargo/reference/config.html#buildbuild-dir) documents `CARGO_BUILD_BUILD_DIR` and separate storage for intermediate artifacts. Cargo's [build-cache reference](https://doc.rust-lang.org/stable/cargo/reference/build-cache.html) says the build-dir layout is internal and subject to change. This relocates intermediates; it does not give Oxide an exclusive lease, bounded storage, cross-path correctness proof, cleanup policy or exact-run evidence. A global `CARGO_TARGET_DIR` alone still shares final output/cleanup scope and does not supply those ownership rules; Cargo's goal also documents those drawbacks.

The smallest useful gap-filling mechanism is therefore to qualify stable build-dir redirection on the exact pinned Cargo version, with an Oxide lease/budget/consumer adapter around it. Do not reimplement Cargo fingerprints or artifact selection. Keep this redirection replaceable. Revisit it when a supported Cargo release documents the needed shared-unit behavior and coverage, and measured exact-head tests show that it preserves the same build-script/proc-macro/path correctness, bounded lifecycle, exclusive ownership and run evidence. Roadmap acceptance, nightly availability or build-dir stability by themselves are not replacement evidence. This applies the [Cargo relationship ruling](vision.md#relationship-with-cargo): use what works now, fill the practical gap, and let upstream capabilities replace the temporary mechanism when proven. Follow the [older artifact-cache issue](https://github.com/rust-lang/cargo/issues/5931) and the [build-artifact collection issue](https://github.com/rust-lang/cargo/issues/13136) as context, not adoption proof.

Cargo currently documents [separate build/target directories](https://doc.rust-lang.org/cargo/reference/build-cache.html), but its intermediate layout is internal and subject to change. [Automatic global cache collection](https://doc.rust-lang.org/cargo/reference/config.html#global-caches) covers registry/Git sources, not build-artifact tracking. Pinned Cargo 1.97.1 therefore has no supported way to share compiled dependency units while keeping workspace, build-script, incremental and test outputs private. Queue Spec 028 already shares `CARGO_HOME` downloads; that is source reuse, not compiled-artifact reuse. A dependency-only compiled cache is not a next Oxide delivery on this toolchain. Moving intermediate storage remains a candidate mechanism for the proposed command front door, not sufficient ownership, portability or budgeting policy. The existing [storage invariants](architecture.md) continue to apply. [Qualification readiness](qualification-readiness.md) preserves the observed cross-path and freshness limits.

## Problem and solution map

All remedies below are candidates for qualification, not selected defaults or permission to change repositories.

| Recurring problem | Existing mechanism or evidence | Oxide opportunity and qualification limit |
| --- | --- | --- |
| New worktree repeats compilation and multiplies outputs | Cargo build-dir separation and upstream cross-workspace goal above | Bounded reusable storage with durable exclusive leases; prove cross-path correctness and preserve run outputs. Begin with project/component partitions; broader cross-project reuse is a separate proof. |
| Drive fills; cleaning destroys useful state and triggers more writes | Cargo source GC differs from build GC | Explain occupied, active, uncertain and reclaimable storage. Budget owned slots and refuse unsafe allocation; do not equate age with safe deletion. Distinguish retention from build cache. |
| Incremental builds grow or lose their benefit | [Cargo incremental settings](https://doc.rust-lang.org/cargo/reference/profiles.html#incremental) trade extra disk state for reuse | Compare sustained edit cycles with and without incremental compilation per workload; measure deterioration before attributing it to size, corruption or compiler behavior. No blanket disable/clean policy. |
| Unexpected rebuilds and misses | [Cargo FAQ](https://doc.rust-lang.org/cargo/faq.html#why-is-cargo-rebuilding-my-code) documents fingerprint logs and feature/configuration changes | Explain the changed inputs and cost in plain language; distinguish a valid miss from failed reuse. Version-gate diagnostics, and avoid heuristics that label real compiler errors cache corruption. |
| Debug artifacts dominate time/storage | [Cargo performance guide](https://doc.rust-lang.org/cargo/guide/build-performance.html) describes lean debug information with a full-debug option | Qualify distinct everyday feedback, test and debugging intents. Keep useful backtraces, assertions and full debugging available; measure actual writes, not only final binary size. |
| Link time dominates small edits | Performance guide above discusses LLD/mold/wild with platform and native-library caveats | Detect the real linker and bottleneck, then benchmark supported alternatives. Linux ELF results do not establish macOS Mach-O benefit. |
| Large dependency graphs, feature drift and duplicate versions | [Cargo timings](https://doc.rust-lang.org/cargo/reference/timings.html) reveals critical paths and duplicate compilation | Report expensive units, duplicate versions and feature variants. Suggest minimal reviewed changes; feature removal/unification can alter behavior or mask bugs. |
| Proc macros, generated code and native builds repeat work | [Cargo build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection) default to broad invalidation unless inputs are declared | Diagnose rerun causes and expensive native tools. Correct input tracking and stable generation need proof; never hide relevant file/environment changes to manufacture hits. |
| Generic code and macros produce excessive compiler work | [Rust Performance Book](https://nnethercote.github.io/perf-book/compile-times.html) discusses macro size and generic IR analysis | Connect observed bottlenecks to actionable project advice. Code/crate-boundary changes require owner review; cache management cannot eliminate intrinsically expensive changed code. |
| Editor checks compete with builds or duplicate targets | [rust-analyzer configuration](https://rust-analyzer.github.io/book/configuration.html#rust-analyzer.cargo.targetDir) explicitly trades separate target storage for avoiding contention | Integrate editor/check use into owned storage choices without deadlocks or surprise builds. Preserve diagnostics; separate check/build work cannot simply be assumed reusable. |
| Test feedback is slow or opaque | [Nextest build archives](https://nexte.st/docs/ci-features/archiving/) separate test building and execution | Integrate tested filtering/reporting and exact-run test bundles instead of writing another runner. Keep doctests and other required coverage explicit; tests must execute and pass at the required source/run identity. |
| Container builds have their own duplicate caches and invalidation | [BuildKit cache guidance](https://docs.docker.com/build/cache/optimize/) and [cargo-chef](https://github.com/LukeMathWalker/cargo-chef) offer cache mounts and dependency layers | Coordinate with Effigy on Linux routes and Colima storage. cargo-chef requires matching Rust versions and stable working directories; image layers do not establish safe live worktree-cache sharing. Host and Linux artifact identities stay distinct. |
| Toolchain, target, SDK and configuration setup is fragile | [rustup override rules](https://rust-lang.github.io/rustup/overrides.html), Cargo configuration hierarchy and [cross](https://github.com/cross-rs/cross) | Show the effective tools/configuration, missing prerequisites and supported paths. Offer reproducible setup without silent global mutation or unsupported toolchain assumptions. |
| Dependency maintenance has too many separate checks | [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) covers dependency policy | Offer optional coherent reporting using existing tools; repository policy remains authoritative. Automatic dependency upgrades/removal are not established scope. |
| Debugging async behavior is hard | [Tokio console](https://github.com/tokio-rs/console) supplies async observability | Explore optional integration and explain instrumentation requirements. It is Tokio-specific, not a general solution for every Rust runtime. |

## Cargo build-message diagnostic boundary

Cargo's [JSON message contract](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages)
provides an observation surface without interpreting mutable compiler internals.
Artifact rows distinguish up-to-date artifacts from steps that ran rustc and
report selected compiler profile fields. A report can describe those observed
settings; the stream does not supply the complete effective Cargo configuration
or why each unit became dirty. Treat package IDs as opaque identities and avoid
turning message counts into a complete unit graph, elapsed time or writes.

Build-script result rows can be emitted from cached results, so they do not
prove a script or native compiler reran. Build-finished success describes the
build stage; test execution can follow it and still fail. A retained log is
user-supplied diagnostic input, not authenticated exact-run QA evidence.
Unrecognized, mixed, malformed or truncated input needs explicit coverage
limits rather than guessed success, cache-hit rates or rebuild causes. No
settings change, new build or host scan is necessary to analyze a named log.

## Cargo command interception

Read-only research plus a disposable `/private/tmp` dispatch probe on pinned
Cargo 1.97.1 / rustup 1.29.1, 2026-10-18, for the
[workflow direction](vision.md#product-workflow-direction). Cargo offers no
supported way to replace its own built-in commands; interception must happen
before Cargo (front door) or through Cargo's documented execution hooks (inner).

| Mechanism | Catches literal `cargo build`? | Observed / documented behavior |
| --- | --- | --- |
| `[alias] build = …` | No | Cargo warns ``user-defined alias `build` is ignored, because it is shadowed by a built-in command``. |
| `cargo-build` on `PATH` | No | Built-in wins silently. |
| `cargo-oxide` on `PATH` | Only `cargo oxide …` | Cargo passes `oxide` plus the remaining arguments and sets `CARGO` to the real toolchain cargo ([external tools](https://doc.rust-lang.org/cargo/reference/external-tools.html#custom-subcommands)). |
| `[alias] b = "oxide build"`, `r = "oxide run"` | Only `cargo b` / `cargo r` | User aliases override Cargo's built-in short aliases; both dispatched to `cargo-oxide`. |
| rustup toolchain whose `bin/cargo` is an Oxide shim | Yes | A `rust-toolchain.toml` `path` toolchain made `cargo build` and `cargo run` reach the shim; `cargo +stable` bypassed it. rustup sets `RUSTUP_TOOLCHAIN` to the shim toolchain, so nested proxy calls re-enter it. `components`, `targets` and `profile` have no effect on path toolchains ([overrides](https://rust-lang.github.io/rustup/overrides.html)); every tool (`cargo-clippy`, `rustfmt`, `rustdoc`, `rust-analyzer`) must be provided. |
| `cargo` earlier on `PATH`, or a shell alias/function | Yes, for that environment | Depends on each agent, editor and CI shell inheriting it; shell functions do not reach non-interactive processes. |
| `build.build-dir`, `build.target-dir`, `build.rustc-wrapper`, `target.<triple>.runner`, `[env]` (config or `CARGO_*` env) | Inner hooks only | One probe received every rustc invocation via `RUSTC_WRAPPER` and the built binary plus arguments via the runner; intermediates and final outputs landed in separately injected directories. These settings propagate to nested Cargo calls from tools such as nextest. |

Implications for design:

- A front door handles user experience; inner hooks keep execution inside
  Oxide-owned storage. Child tools call the real cargo through `CARGO` and
  bypass any shim, so coverage depends on environment/config injection, not on
  intercepting every process.
- A shim must call the real cargo by absolute path and mark re-entry, otherwise
  `RUSTUP_TOOLCHAIN` causes recursion. It must also resolve the toolchain the
  project would otherwise select; a committed `path` toolchain or custom
  toolchain name is machine-specific and would break machines without Oxide.
- rust-analyzer resolves cargo from `CARGO`, `PATH` or `$CARGO_HOME/bin`
  through the rustup proxy, so a toolchain shim also receives editor
  `cargo check` and `cargo metadata` calls unless `check.overrideCommand` and
  `cargo.buildScripts.overrideCommand` route them deliberately.
- The runner hook separates "build" from "execute" for `cargo run` and
  `cargo test`, which is the natural seam for running a binary somewhere other
  than where it was built. Cargo itself has no remote-build facility; remote
  builds mean running Cargo elsewhere or distributing rustc units.

Tom's [2026-10-18 follow-up](vision.md#product-workflow-direction) defers
transparent interception (toolchain shim, `PATH` shim, shell alias) until the
foundation works consistently, in favour of explicit `oxide` commands plus
agent guidance ([workflow](contracts/workflow.md)). The inner hooks remain
the execution mechanism behind those commands. No mechanism is installed or
configured by this research.

## Experimental frontier

The Cargo performance guide also describes nightly parallel frontend and feature-unification experiments. [Cranelift's backend](https://github.com/rust-lang/rustc_codegen_cranelift) offers a faster-codegen candidate but has platform/feature limits; its documentation warns about unwinding support on macOS/Windows. These require opt-in qualification and cannot silently change test/runtime semantics. Hot reload, incremental linking and compiler-level invalidation improvements need targeted upstream/tool research rather than a universal promise.

JIT and alternative execution are explicitly within the product research scope under Tom's ruling in [vision](vision.md). Fast Cranelift ahead-of-time code generation, JIT execution and hot reload are distinct candidates. [Cranelift's JIT usage documentation](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md#jit-mode) describes executing without creating an executable, but warns that the mode is highly experimental, lacks incremental compilation and may be slower than ahead-of-time builds; dependencies need dynamic-library availability. Those are qualification questions, not grounds to exclude JIT.

Compare representative edit/check/run/test loops across supported approaches. Count preparation and dependency builds, time to first useful feedback, repeated-edit latency, runtime/test duration, disk writes, memory/swap and restart/setup effort. Investigate other JIT solutions and hot reload with explicit coverage of supported platforms, FFI, proc macros, generated code, panic/unwinding, debugging and state lifetime. Attribute exact source, backend and execution mode to development results; determine which required checks must also run on the authoritative production backend. This does not authorize experiments or change existing validation contracts.

Remote execution/cache could reduce local compilation, but adds transfer/storage cost, source trust, credentials, latency and a new failure surface. RAM-backed intermediates could reduce some file writes but introduce memory pressure, volatility and possible swap; neither approach is selected. A user should not need to administer infrastructure to obtain the core experience.

## Stable profile and backend eligibility

Read-only recheck, 2026-10-16: the [Cargo build-performance guide](https://doc.rust-lang.org/cargo/guide/build-performance.html#reduce-amount-of-generated-debug-information)
recommends workspace line tables, dependency debug disabled and an explicit
high-quality-debug route as a possible faster/smaller development tradeoff.
Cargo's [profile override rules](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)
make `package."*"` apply to non-workspace members; named overrides,
build overrides, configuration and environment precedence still matter.
The tradeoff loses dependency source/type/variable debugging in that light
build and requires a rebuild for richer debugging. It is a candidate to
measure, not a benefit established on every project.

At this check Oxide's manifest has no custom profiles and uses pinned stable
1.97.1. An explicit custom light profile can leave existing dev/test/release
defaults intact, preserve incremental compilation, assertions, overflow checks
and panic unwinding, retain workspace backtrace lines and reduce dependency
debug output. A separately explicit full-debug profile must resolve full debug
for the relevant workspace, dependency and build-time units; a profile name or
inheritance statement alone is not proof. Compare the same tests and semantic
edit on identical source, and verify actual Cargo/rustc debug settings and
backtraces. This neither configures Nucleus nor adds context policy to Effigy.

The upstream [Cranelift README](https://github.com/rust-lang/rustc_codegen_cranelift#not-yet-supported)
lists aarch64 macOS as supported but still excludes macOS panic unwinding;
panic-abort is its default. Its [JIT mode](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md#jit-mode)
remains experimental, lacks incremental compilation and requires dynamically
available dependencies. Those constraints prevent claiming a drop-in replacement
for the current unwind-dependent test loop. A narrower run-only or another
platform experiment remains possible; JIT is not retired. The local toolchain
list includes nightly-2026-08-05, but its installed-component list contains no
Cranelift backend; no component, compiler or global override was installed or
changed during this check. Merely having nightly does not qualify the backend. The component was installed on 2026-10-18 for the [quick-run measurement](#cranelift-measurement-2026-10-18).

## Quick-run strategies

Read-only research, 2026-10-18, for the `quick` run strategies in the
[workflow](contracts/workflow.md#profiles-and-strategies). None is ready to
replace "quick build, then execute" as the default.

| Strategy | State | Fit for `oxide run --profile quick` |
| --- | --- | --- |
| Quick build + execute (stable LLVM, opt-level 0, line tables, incremental) | Shipped. On Linux, [Rust 1.90 made LLD the default linker](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/) for `x86_64-unknown-linux-gnu`; the [preview post](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable) reports 7x faster linking and 40% less end-to-end time on a ripgrep incremental rebuild. | Default and fallback. |
| [Cranelift AOT](https://github.com/rust-lang/rustc_codegen_cranelift) | Nightly-only preview component `rustc-codegen-cranelift-preview`; aarch64 macOS supported; unwinding unsupported on macOS (`panic=abort` by default), experimental on Linux. Cargo's [`codegen-backend`](https://doc.rust-lang.org/cargo/reference/unstable.html#codegen-backend) can be set per package, so dependencies can stay on LLVM. | Measured 2026-10-18: no warm edit-loop gain on Oxide's workspace, and `catch_unwind` in Cranelift code stops working ([measurement](#cranelift-measurement-2026-10-18)). Not recommended for `run` on small workspaces; a heavy project needs its own reading. A nightly strategy needs its own slot and a second dependency set, plus a nightly toolchain and component. Not for `test`. |
| [Cranelift JIT](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md) | "Highly experimental"; every dependency must be a dynamic library; no incremental compilation, so it may be slower than AOT. | Not a near-term candidate. |
| Hot patching ([Subsecond](https://docs.rs/subsecond/)) | Experimental. It patches only code wrapped in `subsecond::call` and only the tip crate, and needs the Dioxus CLI (`dx serve --hotpatch`) ([Dioxus 0.7](https://dioxuslabs.com/blog/release-070)). | Not a generic strategy: it requires source changes. Possible later opt-in for long-running apps. |

Implications: the foundation ships quick build + execute. Cranelift for
workspace crates was the only near candidate; the
[measurement below](#cranelift-measurement-2026-10-18) found no edit-loop gain
on Oxide's own workspace.

### Cranelift measurement (2026-10-18)

One bounded session (oxide#075) on Oxide's repository at `dc84d03`, macOS
aarch64, under Effigy heavy admission. Raw evidence:
[`evidence/cranelift-quick-run-075/`](evidence/cranelift-quick-run-075/summary.json).
Tom approved the session and the one component install
([vision](vision.md#product-workflow-direction)). `rustup component add
rustc-codegen-cranelift-preview --toolchain nightly-2026-08-05` succeeded
(11.4 MB installed into that toolchain); no other host change was made.

Loop per arm: a cold `cargo build --bin oxide`, then three edits to the
`--help` text in `src/bin/oxide/main.rs`. Each edit was followed by `check`,
`build`, and a direct `oxide --help` that confirmed the edited text. Every
call used the quick recipe, `--locked --offline`, a private target directory
and an explicit environment. The host's `CARGO_INCREMENTAL=0` was removed, so
workspace units built incrementally.

| Arm | Toolchain and backend | Cold build | Warm check | Warm build | Direct run | Target after loop |
| --- | --- | --- | --- | --- | --- | --- |
| A | stable 1.97.1, LLVM | 9.26 s | 0.100 s | 0.307 s | 0.290 s | 167.0 MiB |
| B | nightly-2026-08-05, LLVM | 5.91 s | 0.093 s | 0.310 s | 0.299 s | 155.0 MiB |
| C | nightly, Cranelift for `oxide-storage` only | 4.46 s | 0.096 s | 0.318 s | 0.304 s | 206.5 MiB (with test targets) |
| D | nightly, LLVM, `RUSTFLAGS=-Zthreads=8` | 2.95 s | 0.112 s | 0.341 s | 0.289 s | 155.1 MiB |

Warm figures are means of edits 2 and 3. Every warm step rebuilt exactly one
unit, the `oxide` bin. The first check after a cold build also builds 16
additional check units, including the workspace library: 1.58 / 1.47 / 1.46 / 0.97 s for A–D.

Receipts:
- Arm C's verbose rustc lines show `-Z codegen-backend=cranelift` only on the
  `oxide_storage` and `oxide` units. The cold-build log records 29 other
  compiler invocations for dependencies and build scripts, all on LLVM.
- Arm D shows `-Zthreads=8` on every unit.
- No unit received a `-C panic` flag.

Findings:
- **Warm builds cost more than checking.** A warm check takes about 0.10 s
  and a warm build about 0.31 s. Their roughly 0.21 s difference does not
  isolate frontend, codegen or link costs. Cranelift saved nothing in the
  observed edit loop: C's warm build was 0.01 s slower than B's.
- **Panic semantics change even with `panic=unwind`.** Arm C's `cargo test`
  ran 79 unit tests and all passed. The unwind probe
  (`tests/profile_backtrace.rs`) failed: a `catch_unwind` inside
  Cranelift-compiled code did not catch an overflow panic. The panic instead
  reached libtest's LLVM-compiled harness, which reported the test failed. The
  process did not abort. Exit 101.
- **A second dependency set costs storage.** Each nightly arm holds its own
  dependency units: 63.5 MiB beside stable's 67.2 MiB on this small graph. A
  Cranelift strategy needs that set in its own slot, plus the component and a
  nightly toolchain.
- **`-Zthreads=8` showed no warm gain.** D's warm build was 0.03 s slower than
  B's. Its cold build and first check were faster, but those single readings
  are confounded by run order.
- **The run step is as large as the build step.** Every direct run executed a
  freshly linked binary, and `--help` took 0.28–0.32 s. The cause was not
  isolated.

Recommendation: do not implement a Cranelift-for-workspace `quick` run
strategy on this evidence. On this small workspace warm build time did not
improve, while the strategy costs nightly, a second dependency set and changed
panic semantics. A heavy, codegen-bound project could differ. That needs its own
owner-coordinated, approved reading, and this result does not predict it.

Limits:
- One small subject (31 units) and one sample per reading, on macOS aarch64
  only.
- No write-volume or SSD claim.
- Arms ran in order A, B, C, D, and cold times fall in that order: A paid for
  cold sources and a cold compiler. Cold readings therefore credit neither
  backend nor flag.
- The nightly Cargo already uses the build-dir layout v2 (`build/<package>/<hash>/out`),
  so its byte split by unit name is not identical in layout to stable's.
- The session continued once after a harness defect. It stopped before any
  Cargo call, after the install, and resumed on the same clock, root and
  counters: 29 measured plus 2 version Cargo calls, about 110 s of the 50
  minutes, 0.7 GiB peak owned scratch.

### Other run-path levers

Any rustc-based JIT or interpreter still runs parsing, macro expansion and
type/borrow checking. Those only remove code generation and linking, so
`check` time is the floor. The measurement should report check, build and link
time separately before any backend is credited.

| Lever | Finding | Fit |
| --- | --- | --- |
| No-op fast path | Oxide can record the last successful build's content identity per checkout and binary (the freshness gate records it). If unchanged, it executes without invoking Cargo, skipping Cargo's fingerprint walk. | Not adopted: the attempted fast path did not pass its fixed timing gates ([profile](#oxide-run-overhead-profile-2026-10-19)). |
| Optimized dependencies in `quick` | `package."*".opt-level` can make heavy dependencies run faster. Shared slots would pay that compile cost once across worktrees. It needs `--config`, because the environment cannot express `package."*"`. Measured 2026-10-18 on Oxide's workspace: about 3.3 s extra cold build and no test-runtime gain ([measurement](#optimized-dependency-measurement-2026-10-18)). | Not for the default recipe. A dependency-compute-bound project needs its own reading. Every modelled command must use the same recipe, or the slot holds two dependency sets. |
| Workspace feature unification | Selecting different packages builds dependency variants with different features: churn and extra storage. A 2026-10-18 probe on 1.97.1 showed `resolver.feature-unification` is ignored without `-Zfeature-unification` ([RFC 3692](https://rust-lang.github.io/rfcs/3692-feature-unification.html)). The stable route is a workspace-hack crate, which is a project change. | `oxide status` could report duplicate dependency variants; the fix stays per project. |
| Faster linkers | macOS uses Apple's linker. [Wild](https://github.com/davidlattimore/wild) is Linux-only, has no incremental linking yet, and its Mach-O port is not ready. | Linux: LLD default already; wild later. macOS: nothing to add. |
| Dynamic dependency linking | A dylib wrapper crate re-exporting dependencies cuts link time for large graphs, but changes linking and runtime loading. | Per-project opt-in only. |
| Parallel frontend (`-Zthreads`) | Nightly; speeds frontend work on large crates. Measured 2026-10-18 as arm D: no warm edit gain on Oxide's workspace ([measurement](#cranelift-measurement-2026-10-18)). | Same nightly dependency-set cost as Cranelift; not worth it for small workspaces. |
| Watch mode | Rebuilds and restarts on save; cuts latency, not work. | Human loop; low value for agents. |
| [danluu/rust-interpreter](https://github.com/danluu/rust-interpreter) | rustc checks the code, then selected MIR is lowered to bytecode and run in an interpreter or AArch64 JIT. Self-described as "extremely vibe coded"; "cannot yet replace `cargo test`"; no unwinding, threads, FFI or `should_panic`; "no fallback" for unsupported code. Its results are mixed: 20% faster on one five-edit pilot, 2.7x slower on a compute-heavy target. Needs pinned nightly with `rustc-dev`; no license stated. | Not usable or adoptable. It shows the approach can work, and its ceiling is still the check time. |
| Miri / [Cranelift JIT](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md) | Miri is a UB checker that runs far slower than native code. Cranelift JIT is covered above. | Not speed tools. |

### Optimized-dependency measurement (2026-10-18)

One bounded session (oxide#076) on Oxide's repository at `22d2f77`, stable
1.97.1, macOS aarch64, under Effigy heavy admission. Raw evidence:
[`evidence/optimized-deps-quick-076/`](evidence/optimized-deps-quick-076/summary.json).
Tom approved the session ([vision](vision.md#product-workflow-direction)).

Arms, each in its own target directory: A is the `quick` recipe; B adds
`--config 'profile.oxide-quick.package."*".opt-level=1'`; C uses `opt-level=2`.
Loop per arm: a cold `cargo test --workspace --all-targets --no-run`, then three
edits to `MAX_REFERENCES` in `src/dep_info.rs`, each followed by the same
`--no-run`. Every call used `--locked --offline` and an explicit environment;
the source was reset to the pin between arms. After every build the reported
test executables (`profile.test` artifacts only) ran directly with default
membership. All four macOS scratch variables pointed at one root.

| Arm | Dependency opt-level | Cold `--no-run` | Warm `--no-run` | Library suite (libtest) | `oxide` bin suite | Target after cold / after loop |
| --- | --- | --- | --- | --- | --- | --- |
| A | 0 | 4.24 s | 0.80 s | 2.11 s | 0.33–0.35 s | 190.3 / 264.7 MiB |
| B | 1 | 7.58 s | 0.82 s | 2.22 s | 0.34–0.35 s | 187.5 / 262.6 MiB |
| C | 2 | 7.45 s | 0.89 s | 2.20 s | 0.33–0.37 s | 191.7 / 266.4 MiB |

Warm figures are means of the three edits. Library-suite figures are libtest's
own `finished in` times, means of those edits. Every edit rebuilt the same 9
workspace units and left 29 fresh.

Receipts:
- Tests passed in every cycle of every arm: 52 library, 36 `oxide` bin and 1
  profile probe; 0 ignored, 0 failed. The two storage bins have no tests.
- Workspace units had identical rustc settings in all arms. All 29 dependency
  units (20 libraries, 8 build scripts, `serde_derive`) carried the arm's
  `-C opt-level`. B and C also got an explicit `-C debug-assertions=on`, which
  keeps the debug assertions that A has implicitly at opt-level 0. No other
  flag differed.

Findings:
- **No test-runtime gain.** The library suite averaged 2.1–2.2 s in every arm;
  B and C were not faster than A. The `oxide` bin suite was unchanged.
  Oxide's tests are not bound by dependency compute.
- **Cold cost rises by 3.2–3.3 s per slot (+76–79%).** A ran first, with cold
  sources and compiler, so the real gap may be larger.
- **`package."*"` also optimizes build-time units.** It took precedence over
  `build-override` for build scripts and the proc-macro, which adds compile
  cost with no runtime return.
- **Storage did not change measurably** (within 4.2 MiB after cold, 3.8 MiB
  after the loop).
- **Process start dominates the noise.** Seven of 60 test-binary runs took
  2.0–3.6 s more than libtest reported, including zero-test binaries. All
  arms were affected; the cause was not isolated. Harness wall totals per
  cycle (3.5–7.0 s) therefore do not compare arms.

Recommendation: do not add dependency optimization to the default `quick`
recipe. On this workspace it costs about 3.3 s per cold slot and returns
nothing. A dependency-compute-bound project (heavy hashing, compression or
parsing in tests) could differ. That needs its own approved reading, and an
override would have to exclude build-time units.

Cross-worktree amortization was not observed; this session used one
checkout. Estimate, assumptions only: if every modelled command in a shared
slot uses the same recipe, a slot pays the extra cold cost once. A per-run
saving of *g* seconds then breaks even after about 3.3 / *g* runs in that slot.
Here *g* was not positive, so no number of worktrees recovers the cost.

Limits:
- One small subject (38 units), three warm samples per arm, macOS aarch64
  only, on a shared host (one-minute load 7.6–10.9).
- Arms ran in order A, B, C.
- No write-volume or SSD claim.
- Session: 12 measured plus 1 version Cargo call, 93 s of the 30 minutes,
  811 MiB peak owned scratch, 0.7 MiB raw logs.

## Upstream churn and disk developments (2026-10-18)

**Stale debug objects on macOS.** [cargo#16665](https://github.com/rust-lang/cargo/issues/16665) (open) reports `deps/` growth from old artifacts making incremental builds about 6× slower, and one `cargo clean` removing 345 GiB. Commenters attribute much of it to macOS unpacked debug info. Since 1.88, preserved `*.rcgu.o` objects get per-invocation names and are never overwritten. That root cause is a user claim, not a maintainer confirmation. A 2026-10-18 probe on 1.97.1 (tiny crate, four rebuilds) found:
- `debug = "line-tables-only"` with the default unpacked split debug info left 24 `*.rcgu.o` files in `deps/`;
- `debug = false` left none;
- `split-debuginfo = "packed"` left none and produced one `.dSYM`.

Implication: the `quick` recipe accumulates stale objects in long-lived shared slots, in proportion to workspace rebuilds. The [2026-10-18 measurement](#stale-debug-object-measurement-2026-10-18) found the growth is mostly hardlinked names, not bytes, and recommends packed debug info if a fix is adopted. Do not delete files inside a slot; the build-dir layout is Cargo-internal.

| Development | State | Relevance |
| --- | --- | --- |
| [Build-dir layout v2](https://blog.rust-lang.org/2026/03/13/call-for-testing-build-dir-layout-v2) | Per-unit `build/<package>/<hash>/` directories; scheduled stable in Cargo 1.100 (2026-11-12, per the beta changelog). Cargo 1.99.0-nightly (2026-07-29) already uses it by default ([measurement](#cranelift-measurement-2026-10-18)). | Unblocks Cargo's stale-unit cleanup ([cargo#5026](https://github.com/rust-lang/cargo/issues/5026)) and finer locking. It changes intermediate paths, so Oxide's plain-Cargo stray detection (`deps/`, `incremental/`) must be re-qualified at the upgrade. |
| [Cross-workspace cache goal](https://rust-lang.github.io/rust-project-goals/2026/cargo-cross-workspace-cache.html) | 2026 goal: design and a nightly prototype; conservative at first. | The upstream replacement for Oxide's per-project slots ([Cargo relationship ruling](vision.md#relationship-with-cargo)). Not available yet. |
| [Relink, don't rebuild](https://rust-lang.github.io/goals/2026/roadmap-fast-builds.html) | Did not complete in 2025H2; on the 2026 fast-builds roadmap; claims 5–10× fewer downstream rebuilds for body-only edits. | The largest future edit-loop win; nothing for Oxide to emulate safely. |
| [`hint-mostly-unused`](https://blog.rust-lang.org/inside-rust/2025/07/15/call-for-testing-hint-mostly-unused/) | Nightly; per-dependency hint that defers code generation for unused items; regresses if applied globally. | Narrow, profile-guided; only helps when the dependency is built. |
| Cargo home GC ([1.88](https://blog.rust-lang.org/2025/06/26/Rust-1.88.0/)) | Stable automatic cleanup of downloaded/cached files (3 months / 1 month unused). Never runs with `--offline` or `--frozen`. The target dir is not covered. | Agent and Oxide proofs often run `--offline`, so their Cargo home may never be cleaned. Trigger rules and an `oxide` recommendation: [Cargo home cleanup triggers](#cargo-home-cleanup-triggers-2026-10-18-probe). |
| [Cargo script](https://rust-lang.github.io/goals/2026/cargo-script.html) | Frontmatter stabilization in progress; not stable on 1.97.1. | Single-file scripts; each script otherwise gets its own target dir. |

## Stale debug-object measurement (2026-10-18)

One bounded session (oxide#080) on Oxide's repository at `669fd1e`, stable
1.97.1, macOS aarch64, under Effigy heavy admission. Raw evidence:
[`evidence/stale-debug-objects-quick-080/`](evidence/stale-debug-objects-quick-080/summary.json).
Tom approved the session ([vision](vision.md#product-workflow-direction)).

Arms, each in its own target directory, all on the `quick` recipe
(`oxide-quick` inherits `dev`, `debug = "line-tables-only"`):
- U: as-is, with the default unpacked split debug info.
- P: plus `CARGO_PROFILE_OXIDE_QUICK_SPLIT_DEBUGINFO=packed`.
- N: plus `--config 'profile.oxide-quick.package.oxide-storage.debug=false'`;
  `oxide-storage` is the only workspace package.

Loop per arm: a cold `cargo build --workspace --timings`, then ten edits to
`MAX_REFERENCES` in `src/dep_info.rs`, each followed by the same build. Every
call used `--locked --offline -v` and an explicit environment. The target was
observed read-only after every build. Then a scratch-only integration test,
one uncaught panic through two `#[inline(never)]` helpers, was built with
`cargo test --no-run` and run directly with `RUST_BACKTRACE=1`.

| Arm | `*.rcgu.o` in `deps/`, cold → 10 edits | `deps/` entries after 10 | Target unique allocated, cold / 10 edits | Cold build | Warm build median (range) | `oxide` bin unit, warm median | Probe frames with file:line |
| --- | --- | --- | --- | --- | --- | --- | --- |
| U | 430 → 4,540 (+411 per edit) | 4,611 | 133.1 / 171.2 MiB | 4.27 s | 0.59 s (0.58–0.77) | 0.34 s | 3 of 3 |
| P | 0 → 0 | 74, plus 3 `.dSYM` | 147.6 / 186.7 MiB | 5.33 s | 0.75 s (0.70–0.81) | 0.49 s | 3 of 3 |
| N | 19 → 19 (dependencies only) | 90 | 119.4 / 156.3 MiB | 4.79 s | 0.62 s (0.58–1.31) | 0.35 s | 0 of 3 |

Unit times are Cargo's own `--timings` durations. Unique allocated counts
`st_blocks` once per inode. Every arm's `incremental/` roughly doubled on the
first edit and then held steady.

Receipts:
- Arm control: each cold build had the same 33 units, and no rustc setting
  other than the debug flags differed; the recipe would have stopped
  otherwise.
  - U: the 4 workspace units and 15 dependency libraries had
    `-C debuginfo=line-tables-only -C split-debuginfo=unpacked`. Build scripts
    and the 6 build-time units had no debug flags.
  - P: the same, with `packed`.
  - N: the 4 workspace units (library, 3 bins) had no debug flags. Every
    dependency unit kept U's flags. Cargo added no `-C strip`.
- Every edit rebuilt the same 4 workspace units and left 29 fresh.
- Panic: every arm exited 101 and printed the correct location,
  `tests/oxide_080_panic_probe.rs:5:5`.
  - U and P resolved all 8 short-backtrace frames to file:line, including the
    3 probe frames at their exact lines. P read the `.dSYM` beside the test
    binary.
  - N named all 8 frames but resolved only 3 standard-library frames; the 3
    probe frames had no file:line.

Findings:
- **U accumulates names, not bytes, on this edit.** Each rebuild added 411
  `*.rcgu.o` names, one per codegen unit of the rebuilt incremental workspace
  units.
  - After ten edits and the panic build, 4,561 names pointed at 461 inodes
    (14.1 MiB allocated). Summed per name they claim 121 MiB apparent.
  - 433 of those inodes are also named in `incremental/`: rustc hardlinks
    reused work products into `deps/` under per-invocation names.
  - Only 28 inodes (2.5 MiB) were held by `deps/` names alone, and 19 of them
    are dependency objects that every arm keeps.
  - Unique target allocation grew about 0.1 MiB per edit after the first.
  - The current binaries' debug maps referenced 260 `deps/` objects in every
    cycle. The rest were not referenced by those binaries; that does not make
    them unreferenced elsewhere or safe to delete.
- **Accumulation did not slow builds within ten cycles.** U's warm builds
  stayed at 0.58–0.77 s up to 4,611 `deps/` entries.
- **P stops accumulation for about 0.16 s per rebuild.** The median warm build
  rose 0.16 s (+26%) and the cold build 1.07 s.
  - The three bin units each took 0.09–0.15 s longer; the library did not.
    Those durations cover codegen, link and, in P, `dsymutil`. The tool output
    does not separate them, so no link-only figure is claimed.
  - P's target was the largest, 15.5 MiB above U after ten edits: 18 MiB of
    `.dSYM`, more `incremental/`, fewer `deps/` bytes.
- **N loses the backtrace lines `quick` exists to keep.** It stops
  accumulation at no measurable time cost: median within 0.03 s of U. Edits
  8–10 took 0.91–1.31 s while the one-minute load rose to 7.6. But workspace
  frames lose file:line. The panic location survives because it is compiled
  into the panic, not read from debug info.

Recommendation: keep `quick`'s line tables.
- If a fix is adopted, use `split-debuginfo = "packed"` (P). It leaves no
  stale objects and keeps backtrace quality, for about 0.16 s per rebuild and
  15 MiB per slot here.
- Reject N for `quick`: it removes required debugging behaviour.
- U remains acceptable on this evidence. Its growth is mostly hardlinked
  names, which Oxide's inventory, deduplicating by `(dev, ino)`, counts once.
- Whole-slot recycling stays with the existing ownership policy; this
  session did not test it.

Adoption needs a separate decision; no recipe changed.

Limits:
- One small subject (33 units), one edit shape (a constant in one file,
  touching few codegen units), ten cycles per arm, macOS aarch64, shared host
  (one-minute load 4.3–7.6).
- A larger edit changes more codegen units, and each changed unit's old object
  can then be held by a stale name alone. U's byte growth here is a lower
  bound. The [cargo#16665](https://github.com/rust-lang/cargo/issues/16665)
  reports come from large long-lived projects; this session neither confirms
  nor refutes them.
- Arms ran in order U, P, N; U ran first, with cold caches.
- `st_blocks` cannot see APFS clone sharing. No write-volume, SSD or debugger
  claim.
- Session: 36 measured plus 2 version Cargo calls, 162 s of the 40 minutes,
  549 MiB peak owned scratch, 3.3 MiB raw logs.
- The recipe continued once after a harness defect: its debug-map observer
  read `.pid` from a finished `CompletedProcess`. Attempt 1 had run both
  version calls and U's cold build. The continuation reused the same root,
  packet, clock and counters. It re-verified U's source and re-observed U's
  target (identical) before going on.

## Cargo home cleanup triggers (2026-10-18 probe)

oxide#084 read Cargo 1.97.1's automatic cleanup at the pinned commit `c980f4866141969fab6254a680546a277789d6f0` and ran one confirming session on a fresh scratch `CARGO_HOME`. Source hashes, the recipe, the session log and per-call traces are in [cargo-home-gc-trigger-084](evidence/cargo-home-gc-trigger-084/session.log).

**Source-proven rules** (links are to the pinned commit):
- **Call sites.** A grep of every `src/cargo` and `src/bin` Rust file at the pinned commit (257 files, 3.8 MB; [hashes](evidence/cargo-home-gc-trigger-084/cargo-source-full-sha256.txt)) finds exactly two `auto_gc` calls: at the end of [`compile_ws`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_compile/mod.rs#L192) — after the build context is created (resolution and downloads), before compilation, so a build that later fails still cleans — and at the end of [`ops::fetch`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_fetch.rs#L81).
- **Commands that reach it** (all `compile_ws` / `ops::fetch` callers in that grep):

  | Command | Path | Returns before GC when |
  | --- | --- | --- |
  | `build` | [`ops::compile`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/build.rs#L68) | resolution or download fails; `--unit-graph` ([returns first](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_compile/mod.rs#L188-L191), nightly) |
  | `check` (and `cargo clippy`, which runs `cargo check`; Clippy's source was not read) | [`ops::compile`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/check.rs#L57) | same |
  | `rustc` | [`ops::compile`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/rustc.rs#L110) | `--print` ([returns first](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/rustc.rs#L86-L90), nightly) |
  | `run` | [`ops::run`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/run.rs#L87) → [`ops::compile`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_run.rs#L94); scripts via [`exec_manifest_command`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/run.rs#L96-L196) need `-Zscript` | same as `build` |
  | `test`, `bench` | [`run_tests`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/test.rs#L113) / [`run_benches`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/bench.rs#L78) → [`compile_tests`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_test.rs#L104-L105) | same (`--no-run` still compiles) |
  | `doc`, `rustdoc` | [`doc`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/doc.rs#L63) / [`rustdoc`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/rustdoc.rs#L86) → [`ops::compile`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_doc.rs#L59) | same |
  | `fix` | [`ops::fix`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/fix.rs#L104-L108) → [`ops::compile`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/fix/mod.rs#L218); `-Zfix-edition` via [`fix_edition`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/fix/fix_edition.rs#L41) | same; with nightly `-Zfix-edition`, packages not all at the start edition are [skipped with `Ok(())`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/fix/fix_edition.rs#L19-L48) |
  | `install` | [`install_one`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_install.rs#L335) → [`compile_ws`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_install.rs#L393) | `--list` ([prints installed packages instead](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/install.rs#L235-L238)); every requested package already installed and up to date ([`Ok(None)`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_install.rs#L190-L202), [also](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_install.rs#L296-L304)) |
  | `package`, `publish` | [`do_package`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_package/mod.rs#L344-L347) → [`run_verify`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_package/verify.rs#L95) → `compile_with_exec`; publish passes [`verify`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/registry/publish.rs#L163-L167) through | `--no-verify`; `package --list` ([prints the file list instead of adding any output](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/cargo_package/mod.rs#L302-L339), so the verify loop has nothing to compile); `publish` when every selected member is `publish = false` and the selection allows that (`--workspace`, `--exclude` or a virtual-workspace default): [warns "nothing to publish" and returns](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/ops/registry/publish.rs#L85-L123) |
  | `fetch` | [`ops::fetch`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/fetch.rs#L24) | resolution or download fails |

  No other command calls either function, so `metadata`, `tree`, `update`, `generate-lockfile`, `vendor`, `clean`, `add`/`remove`, `uninstall`, `search`, `login`, `owner` and `yank` never trigger automatic cleanup. Any error before the call (resolution, download, argument or manifest failures) also skips it. The "returns before GC" column comes from inspecting every `return`, `bail!` and `Ok(None)` between each listed entry point and the GC call in the pinned files.
- **Offline skip.** `auto_gc` [returns immediately](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L51-L58) when `network_allowed()` is false. [`network_allowed`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/util/context/mod.rs#L1250-L1262) is false under `--frozen` or offline; [offline](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/util/context/mod.rs#L1206-L1212) is `--offline` or `net.offline` config (so `CARGO_NET_OFFLINE=true` too). `--locked` alone does not skip.
- **Lock skip.** It also returns silently when the package cache's mutate-exclusive lock [is already held](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L73-L80), for example by a concurrent Cargo.
- **Frequency gate.** [`cache.auto-clean-frequency`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L279-L301) defaults to [`1 day`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L37). [`always`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L347-L360) is 0 s; `never` disables; otherwise `N seconds/minutes/hours/days/weeks/months` ([month = 2,629,746 s](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L379-L397)). The gate is [`last_auto_gc + frequency < now`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L517-L533) in whole seconds, checked once per process. A successful non-dry-run pass [writes `last_auto_gc`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L537-L541).
- **New homes wait a day.** Creating the database [primes `last_auto_gc` with the current time](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L291-L305), so a fresh home's first default cleanup is at least a day later. `last_auto_gc` therefore means "last automatic pass, or database creation": the value alone cannot show a cleanup ever ran (call 2 below primed it and skipped).
- **Age thresholds.** Extracted sources (`registry/src`, git checkouts) [1 month](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L30-L32); downloaded data (`.crate` archives, index cache, git databases) [3 months](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L33-L35). Overriding them via `cache.global-clean` [needs `-Zgc`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/gc.rs#L177-L244); stable always uses the defaults.
- **Orphan removal ignores age.** Each automatic pass [syncs the database with disk](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L566-L578) first. A registry index or git database that is tracked but [missing from disk](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L748-L762) has its whole child trees queued for deletion [without an age test](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L866-L893): `registry/cache/<registry>` and `registry/src/<registry>` for an index, `git/checkouts/<name>` for a git database. Deleting `registry/index/<registry>` by hand therefore makes the next pass remove that registry's recent `.crate` files and sources too. Tracked child rows missing on disk are [dropped from the database](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L727-L746) without touching files.
- **What is tracked.** A SQLite database at [`$CARGO_HOME/.global-cache`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L134) records last use and size for registry indexes, `.crate` files, extracted sources, git databases and git checkouts ([tables](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L136-L140)). Cleaning first [adds untracked files](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L713-L808) with the current time, so files from an older Cargo are not eligible for at least a month. Database open or read-only failures are [silent](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/global_cache_tracker.rs#L1812-L1822) unless `-vv`. The target directory and `~/.cargo/bin` are not covered.
- **No-network online runs.** A sparse registry only revalidates index files when [an update was requested](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/sources/registry/http_remote.rs#L291-L298); otherwise [the local copy is used](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/sources/registry/http_remote.rs#L460-L474). An online command whose lockfile and cache are complete therefore needs no network and still reaches `auto_gc`.
- **Manual cleanup is nightly.** [`cargo clean gc`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/bin/cargo/commands/clean.rs#L172-L179) fails on stable without `-Zgc`.

**Confirming session** (21:20:23–21:20:27Z; 6 of 6 Cargo calls, all exit 0; Effigy heavy admission; one download, `cfg-if` 1.0.0, 7,934 bytes, sha256 `baf1de43…a40fd`, matching the lockfile checksum). Each call traced with `CARGO_LOG=gc=trace`; `last_auto_gc` was read read-only after each. Calls 4–6 set `CARGO_HTTP_PROXY=http://127.0.0.1:9` (closed port) so any network attempt would fail.

| Call | Command | `gc` trace | `last_auto_gc` |
| --- | --- | --- | --- |
| 1 | `cargo -V` | — | no database |
| 2 | `fetch` (online, default frequency) | `last auto gc was 1792358423, skipping` | primed 1792358423 |
| 3 | `fetch --offline`, `always` | `running offline, auto gc disabled` | unchanged |
| 4 | `check --frozen`, `always`, closed proxy | `running offline, auto gc disabled` | unchanged |
| 5 | `fetch`, `always`, closed proxy | `last auto gc was 1792358423, running`; db sync; cleaning thresholds 1 / 3 months | 1792358425 |
| 6 | `check`, `always`, closed proxy | `last auto gc was 1792358425, running` | 1792358427 |

Observed: the skip and run rules above, the day-long priming, and that an online `fetch` or `check` with a complete lockfile ran cleanup through a closed proxy without error. Nothing was age-eligible and no tracked parent was missing, so no file was deleted; deletion behaviour rests on source alone.

**Recommendation** (implemented in oxide#085 as [Cargo home hygiene](contracts/workflow.md#cargo-home-hygiene): no registry/git split; the run reads Cargo's `gc` trace, forces a closed proxy and refuses routes the proxy cannot contain):
- `oxide status` should report the Cargo home's size (`du` of `CARGO_HOME`, default `~/.cargo`), split into `registry/` and `git/`. Do not report a "last cleanup" time: Cargo's only record, `global_data.last_auto_gc`, is set at database creation as well as after a pass, and reading it means opening Cargo's internal database. At most, a strictly read-only read could be shown as "Cargo's GC baseline (last automatic pass or database creation)"; the simpler default is to omit it.
- `oxide clean --cargo-cache` can trigger Cargo's own stable cleanup without network or database access: run `cargo fetch --config 'cache.auto-clean-frequency="always"'` without `--offline` / `--frozen` in a workspace whose lockfile is complete. A dependency-free scratch package should need no registry read at all; that variant was not run. Cleanup removes entries past the fixed 1 / 3-month thresholds plus any orphaned registry or git child trees (see above), and it skips silently when another Cargo holds the cache lock — `oxide` cannot confirm it ran without reading the database or Cargo's `gc` trace.
- Any command run with `--offline`, `--frozen` or `net.offline` never cleans, so agents and proofs that always pass them never clean that home. (`cargo metadata`, which Oxide's freshness gate runs offline, never triggers cleanup in any mode.) Any ordinary online build or fetch past the daily gate still cleans.

## Cloning a warm build directory (2026-10-18 probe)

oxide#081 tested whether a warm build directory still works after an APFS clone to a new path. It ran one heavy-admitted 32-second session using 13 of 14 experimental Cargo calls and both setup calls. Raw evidence is in [apfs-clone-relocation-20261018](evidence/apfs-clone-relocation-20261018/summary.json).

Setup:
- Injection matched `oxide`'s: `CARGO_BUILD_BUILD_DIR` pointed at the slot and one private `CARGO_TARGET_DIR` stayed fixed.
- `clonefile(2)` copied build dir A to B. A was then renamed away so any dangling reference would fail loudly.
- The same checkout was then built against B.
- Fixture: a build script that writes `include!`d code and a static library into `OUT_DIR`, emits `rustc-link-search` into it, a path proc-macro and one integration test.

| Subject | Verdict | Evidence |
| --- | --- | --- |
| Fixture, Cargo 1.97.1 layout | Relocatable | After the clone, all 8 units were fresh and the binaries passed. A source-only edit rebuilt only `probe-app` and linked the static library from B. A build-input edit reran the build script. A final no-op was fresh. |
| Fixture, nightly-2026-08-05 `-Z build-dir-new-layout` (layout v2) | Relocatable | Same sequence and results. |
| Oxide repository at `dc84d03`, stable, default dev profile | Relocatable for Cargo | After the clone, all 38 units were fresh in 0.25 s. A `src/lib.rs` edit rebuilt only the 9 `oxide-storage` units in 1.7 s; the cold build took 5.0 s. |

How the paths behave:
- Cargo stays correct because it does not keep A's path in the files it checks:
  - no `.fingerprint` file in any arm named A;
  - B's cloned build-script `output` and `root-output` still named A's `OUT_DIR`, but Cargo rewrote the replayed link-search path to B.
- A's path survives elsewhere:
  - rustc's make-style `.d` files of unchanged units;
  - unrerun dependency `root-output` files;
  - `serde`/`serde_core` metadata source maps that name A's generated `private.rs`;
  - proc-macro dylib install names.
- Debug info is the open condition. Until a binary is relinked, its macOS debug map (`N_OSO`) points at A's `deps/*.o`. That includes the `target/debug/*` outputs Cargo uplifts from B (on macOS APFS these are clones, not hard links; see the [final-output placement probe](#final-output-placement-probe-2026-10-18)).
  - It harms nothing while A still exists unchanged, as a clone source would.
  - Once A is gone or rebuilt, debugging an unchanged binary can lose its debug info.
  - The probe did not run a debugger.

Clone cost (Oxide repository, 2,126 files; source `st_blocks` sum 244 MiB, which counts allocated blocks, not apparent size, and cannot see clone sharing):
- `clonefile` took 12.6 ms. Host free space fell 1.1 MiB from before the clone to after the rename.
- All 300 sampled files kept the same first physical extent (`F_LOG2PHYS_EXT`). A plain-copy control file moved to a new extent, which is direct evidence of copy-on-write sharing. Inodes differed; sizes, modes and mtimes matched.
- Host free space fell another 152 MB over the edit-rebuild interval. The host is shared, so this is not attributed to the rebuild or to clone divergence.
- Host free space is shared and noisy. These figures are not exact clone allocation or SSD writes.

Implication: on this host and toolchain, cloning a slot per worktree, or warm-starting from main's slot, is mechanically sound for Cargo freshness and rebuild correctness on both layouts. A product design still has to settle the debug-map condition: keep the source slot unchanged while clones exist, or accept relinking. One fixture and one repository do not prove universal portability, debugger behaviour or savings. Two recipe slips are corrected in the summary:
- the layout v2 build script is named `build_script_build`, so the recipe's run counter missed it;
- the hit classifier mislabels v2 unit outputs under `build/<pkg>/<hash>/out/`.

## Final-output placement probe (2026-10-18)

Question: when the target directory shares a volume with the build directory, does Cargo hard-link final outputs instead of copying them, and how many bytes per worktree does that save over the current layout (build directory on the pool, target directory in the checkout; [workflow](contracts/workflow.md#what-one-command-does-local-executor))?

Subject: Oxide at `22d2f77`, stable 1.97.1, `--offline`, default `dev`/`test` profiles, no edits. One fresh 4 GiB APFS sparse image, two layouts built sequentially into disjoint directories:
- X (cross-volume): build directory on the image, target directory on the host volume.
- S (same volume): both on the image.

Per layout: one cold `cargo build` and one `cargo test --no-run`. The recorded data per final file: device, inode, `st_nlink`, size, allocation, APFS private size and clone ID (`getattrlist`), plus its content-equal build-directory peer.

| | X (cross-volume) | S (same volume) |
| --- | --- | --- |
| Hard links | None: `st_nlink` 1; no target file shares device and inode with a build file | None, same evidence |
| Uplifted lib and three bins | Independent copies; private bytes equal allocation | APFS clones of the `deps/` files: private size 0, shared clone ID, refcount 2 |
| Target-dir bytes not shared with build dir | 15,589,376 | 20,480 (`.d` files and `CACHEDIR.TAG`) |
| Test binaries | Five built; all stay in the build directory; none uplifted | Same |
| Image-use delta | 212,426,752 (build dir only) | 212,029,440 (build and target dirs) |

Result: in this run, Cargo hard-linked no uplifted output in either layout. On the same APFS volume it cloned them, which cost almost nothing. Cargo's own source at the measured commit explains the observation: on macOS, `cargo-util` `_link_or_copy` removes the destination and then calls `fs::copy`, which uses `fclonefileat` on APFS. If that copy fails with `EAGAIN`, it falls back to `hard_link`, so hard links remain possible on macOS; this run never hit that path. The source comment cites a Gatekeeper race with hard-linked binaries ([cargo#7821](https://github.com/rust-lang/cargo/issues/7821), [cargo#10060](https://github.com/rust-lang/cargo/issues/10060)). Other Unix systems take the `hard_link` path; Linux was not measured.

Per-worktree saving for this subject: 15,568,896 bytes (about 14.8 MiB), or 6.7% of the 231 MB clone-aware total. The image deltas agree: S's target directory added no measurable image use (S − X = −397,312 bytes, within APFS accounting noise). The saving should grow with final binary size and debug info; only this subject was measured. Here, all five test-harness executables stayed in the build directory and none was uplifted, so this subject's test path saved nothing; only its `build`/`run` outputs benefited.

`cargo test --no-run` re-uplifted the three fresh bins (new inodes) because integration tests need them. In X that means a fresh full copy on the host volume. In S it means a new clone. No write volume was measured.

Limits: one subject on one host, one cold build plus one `test --no-run` per layout, APFS only. No repeat, edit or branch-switch builds; no test execution; no Linux run. Clones share blocks only until either side is rewritten. Placement, lease and default behavior are unchanged. A same-volume target directory would also move final outputs off the checkout and into the pool, which affects the privacy and lifetime rules in [workflow](contracts/workflow.md#what-one-command-does-local-executor). Evidence: [final-output-links-082](evidence/final-output-links-082/summary.json).

## `oxide run` overhead profile (2026-10-19)

A prototype profiled the skip path and normal leased no-op on one std-only
fixture over a fresh 128 MiB APFS pool. It used opt-in phase tracing on one
macOS host. This is retained negative fixture knowledge; the implementation
is not merged. Preserved records: [baseline](evidence/run-overhead-086/baseline.json),
[candidate](evidence/run-overhead-086/candidate-1.json) and
[preservation notes](evidence/run-overhead-086/README.md).

| Fastest of three | Instrumentation-only baseline | Candidate |
| --- | --- | --- |
| Skip, including program execution | 24.621 ms | 23.902 ms |
| Leased no-op, including program execution | 434.051 ms | 283.113 ms |
| Plain `cargo build` no-op, without program execution | 22.659 ms | 16.239 ms |
| Difference: leased minus plain build | 411.392 ms | 266.873 ms |

Both fixed candidate gates failed. Skip gate part 1 passed, but part 2
failed: 23.902 ms exceeds 16.239 ms plus the fixed 4.462 ms noise allowance.
The ordinary-path gate required a difference at most 205.696 ms (half the
baseline); the observed reduction was about 35%. There is no rescoring,
criterion relaxation or claim about a larger project, SSD writes or CPU cost.

The baseline phase recorded 9.061 seconds, nine measured requests and 18
actual Cargo invocations including setup/staging. Both records retain the same
fixture digest. Candidate timing ran before the remaining correctness cases;
the focused proof then stopped at `prove_routing`'s project-slot-count
assertion because its timing fixture had added another project. Registry and
outside-path correctness cases did not execute. Existing passing selectors and
partial observations do not supply missing end-to-end acceptance.

What the phase trace shows:

- The trace-initialization to pre-exec marker interval was about 291–294 ms for the
  two shorter baseline leased samples (361 ms for the outlier), and 146–210 ms
  in the candidate. This includes child Cargo/rustc work, waits and syncs;
  it is not exclusively Oxide CPU time. Phase spans are nested/overlapping and
  must not be summed as disjoint costs.
- Individual Git commands commonly took about 6–10 ms. The prototype's baseline
  repeated Git reads across identity, snapshots and record publication.
  Ledger saves commonly took several milliseconds; rustup-proxied `rustc`
  identity/sysroot commands and Cargo workspace-resolution commands also had
  visible process costs. These are measured categories, not a claim that
  removing any call would preserve semantics.
- Subtracting the pre-exec marker from total leased duration leaves roughly
  128–147 ms in the shorter samples and about 1.025 s in the baseline outlier.
  The marker is emitted before the `exec` call; this residual does not isolate
  successful exec, loader/security checks, program execution or scheduling.
  Skip samples leave about 4–5 ms. No plain `cargo run` sample was collected.
- Pinned Cargo has a fresh-unit artifact-uplift path and a platform-dependent
  [link/copy implementation](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/crates/cargo-util/src/paths.rs).
  Output replacement and first-execution checks are hypotheses for part of the
  residual, not measured causes. No per-invocation output-identity or syscall
  proof establishes that every no-op replaced a binary. The build-only baseline
  does not execute the program; that comparison was fixed before the run and
  remains the failed gate's definition.

Candidate changes attempted workspace-root reuse from existing metadata,
parallel identity/toolchain/input reads, sharing the pre/post-Cargo snapshot
between the gate and skip record, and removing the wait ticket in its granting
save. The author reports no fsync removal. These changes are preserved source,
not independently accepted semantic equivalence or adopted improvements.

Cargo-relative source findings at pinned `c980f4866`:

- The [compiler call site](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/mod.rs)
  passes the unit's local-source classification to dep-info translation.
  [Translation](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/dep_info.rs)
  skips package-relative files when `allow_package` is false. Thus merely seeing
  `CheckDepInfo` does not prove that all registry/git package files are followed.
- [Final-artifact dep-info generation](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/output_depinfo.rs)
  treats non-local package changes as lockfile-identity changes and omits their
  ordinary package inputs. This does not expose every input Cargo can follow.
- [Build-script fingerprints](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/mod.rs)
  can follow `rerun-if-changed` paths. Non-local dep-info may also retain paths
  outside its package/build roots. Those inputs were not covered by this
  prototype's selected public-output strategy.

The prototype allowed registry/git packages without build scripts through the
lock/source identity and refused those with build scripts. Its new eligibility
cases were not reached, and the outside-package input limitation remained
unresolved. This is source-analysis and design material, not a universal
Cargo-equivalent skip qualification. The fixed no-go and preserved correctness
repairs do not authorize another run, product merge or dogfooding sequence change.

## Oxide normal-command overhead, round 2 (2026-10-19)

oxide#088 measured Oxide's added time on a normal no-op like for like against
plain Cargo, on one std-only fixture over a fresh 128 MiB APFS pool, on one
macOS host. **Final result: no-go.** Raw records:
[evidence](evidence/overhead-088/README.md). The gate was fixed before any
result: fastest of three `oxide check` minus fastest `cargo check` at most
100 ms, and fastest `oxide run -- x` minus fastest `cargo run -- x` at most
100 ms. The branch had no skip path, so every `oxide run` was the normal
leased path. Plain Cargo used a private target directory, the same private
`CARGO_HOME`, offline environment and toolchain.

| Fastest sample, ms | Baseline (2 rounds, observes only) | Author candidate | Independent review |
| --- | --- | --- | --- |
| `oxide check` | 342.830 | 115.162 | 157.890 |
| `cargo check` | 23.135 | 17.255 | 21.819 |
| Gate 1 difference | 319.694 | 97.908 (pass) | 136.071 (fail) |
| `oxide run -- x` | 414.255 | 243.982 | 273.297 |
| `cargo run -- x` | 161.344 | 150.008 | 165.852 |
| Gate 2 difference | 252.911 | 93.974 (pass) | 107.444 (fail) |
| `cargo run` minus `cargo build` (information only) | 108.929 | 130.243 | 141.213 |

The baseline ran on instrumentation-only `4a11f9d`. Both candidate sets ran
the product source of `d217b37` (the reviewed head `259088e` adds docs and
evidence only) with the same fixture digest. The independent exact-head set
failed both gates, and that required failure decides the no-go; the sets are
not averaged or substituted, and no noise waiver applies. The #079 noise
statistic was 25.995 / 31.965 ms in the author's set and 11.398 / 30.399 ms
in the review's. The difference between sets is not attributed to host load:
none was measured.

The review also found:

1. The toolchain cache statted the resolved compiler only after its `-vV`
   and sysroot spawns, so a compiler replaced in between could pair old
   version output with the new binary. Unfixed.
2. The background pre-Cargo snapshot ran its first Git call before the lease,
   so the crash-in-gate fixture's hold could fire before the record reached
   `acquiring`. The review's proof exited 1 there; later controls did not run.
   The author's passing run does not establish that case.
3. Aggregate bounds evidence was insufficient: off-fixture staging was not
   counted toward 512 MiB, the reserve was checked only at boundaries, and the
   clock started after provisioning and admission. No cap overrun is shown.

What the phase traces show (author's fastest samples; spans overlap):

- Baseline `oxide check` reached its end marker at 338.6 ms: workspace
  20.7 ms, three identity Git calls 28.2 ms, `rustc -vV` 27.0 ms, the
  pre-Cargo snapshot's three Git calls 74.9 ms, seven state saves of
  7.5–13.5 ms each plus two origin writes, Cargo 58.7 ms against a plain
  23.1 ms (the wait loop polled every 20 ms), and the post-Cargo snapshot
  43.9 ms.
- The candidate reached it at 112.2 ms with Git calls 9 to 6, a cached
  toolchain read, five state saves and overlapped snapshots. What remained
  was mostly serial durable saves, the lease library and process spawns.
- These are profile observations of one host, not evidence that the cuts
  reliably meet the gate. No cause is claimed for the time after the
  pre-exec marker. The #086 build-only baseline is not comparable with these
  like-for-like figures.

None of this is merged or installed. The branch is kept only as a knowledge
donor: do not import its runtime, its toolchain cache or its eligibility
reasoning. Later work starts from current main.

Tom's later 2026-10-19 [ruling](vision.md#product-workflow-direction)
pauses the unmerged skip path. Its branch and committee capture contract are
preserved; the design has unresolved filesystem-timestamp and toolchain-resolution
questions and is not implementation-ready. The planned build-script readers are
withdrawn, and skip/readers version qualification is not admitted. The next
approved work is normal-path overhead reduction and an opt-in launch diagnostic;
no candidate optimization or skip code is adopted by that decision.

## Oxide normal-command overhead, round 3 (2026-10-19)

oxide#092 retried the like-for-like gate with #088's reductions imported and
changes intended to address its defects, using a paired method fixed before any result: on one
std-only fixture over a fresh 128 MiB APFS pool, one warm-up of each, then
nine interleaved pairs (order alternating), statistic the median paired
difference, ceiling 100 ms per gate. Plain Cargo used Oxide's quick profile,
a private target directory, the same private `CARGO_HOME`, offline
environment, lock file and toolchain. **Result: no-go on the author's own
proof.** Raw records: [evidence](evidence/overhead-092/README.md).

| Paired no-op, ms | Baseline (1 pair, observes only) | Candidate (9 pairs) |
| --- | --- | --- |
| Gate 1: `oxide check` minus `cargo check` | 355.949 | median 142.656 (fail) |
| Gate 2: `oxide run -- x` minus `cargo run -- x` | 272.397 | median 81.556 (pass) |

Gate 1's nine differences were 132.5–211.9 ms. Gate 2's ranged from
−224.545 to +338.156 ms. Plain run ranged 223.415–623.755 ms and Oxide
run 321.175–561.571 ms. The fixed statistic passes this captured set; the
variation establishes neither a stable overhead bound nor its cause. The brief stops on any measured
candidate failure, so there was no second proof, tuning, replay or reviewer
measurement. Nothing from the branch is merged or installed.

Candidate changes, unaccepted: #088's combined Git reads, run metadata
naming the workspace, overlapped snapshots, ticket only when waiting and
wait-poll backoff; Git reads in parallel; a toolchain cache resolved in the
caller's directory and environment, recording only when the resolved
compiler and every input observe the same before and after `rustc -vV`; and
a debug-only gate hold replacing the racy Git-wrapper hold. Unit regressions
were added for the bracket and caller context. The author's log and pinned
harness indicate that its correctness cases completed once before the overall
timing failure; no independent product/runtime review was performed. Ordinary
compile, format, full Rust tests and lease proof were not run after the stop.
The evidence-only review ran no validation and grants no semantic-equivalence
or product acceptance. Remaining native proof allowance stayed unused.

What the candidate traces show (fastest check sample, 163.6 ms, end marker
at 158.6 ms; spans overlap):

- Captured plain-check and Git-operation times were higher than the reported
  #088 samples: 30.3–37.9 ms versus 17–23 ms for plain check, and 14–16 ms
  versus about 7 ms for Git operations. Host load was not measured; no cause
  for the difference is established.
- Before Cargo: `cargo locate-project` 25.6 ms, then identity and snapshot
  Git reads in parallel 16.5 ms, a cached toolchain read 0.8 ms.
- Two origin operations, five state saves (7.487–12.520 ms each), and lease
  library lease/release spans sum to 70.972 ms. These enclosing operation
  durations overlap Cargo and the post-Cargo Git snapshot; they do not isolate
  sync latency or additive critical-path cost. State saves sync the file then
  the control directory; origin writes sync their temporary file before rename.
  The installed Rust 1.97.1 macOS implementation routes `sync_all` through
  `F_FULLFSYNC`; no syscall trace was taken. Durability changes need separate
  design authority; these observations do not prove a redesign necessary for
  every possible reduction.
- Oxide's own Cargo call took 44.3 ms against the 30–38 ms plain request.
- The first measured check's version span was about 65 ms. Source requires an
  existing `target/` for its best-effort cache write; this is consistent with
  a cold warm-up not populating the cache, not an independently observed miss.

## macOS new-executable launch cost (2026-10-19)

A Supervisor probe on this host (macOS, Apple Silicon; tiny `rustc -O` binary; 10 runs each) measured process launch time:

| Launch | Median | Range |
| --- | --- | --- |
| The same file again | 2.3 ms | 1.9–295.8 ms (the 295.8 ms outlier was the first run) |
| A fresh `copyfile` of identical content | 101.6 ms | 98.6–124.5 ms |
| An APFS clone (`cp -c`) | 117.1 ms | 113.2–122.2 ms |

Every newly written executable pays about 100 ms on its first launch, even with identical content and even as a copy-on-write clone. This is consistent with macOS XProtect/Gatekeeper first-execution assessment ([nextest: antivirus and Gatekeeper](https://nexte.st/book/antivirus-gatekeeper); [Rust forum report](https://users.rust-lang.org/t/cargo-run-slow-on-macos-when-binary-already-built/117450)). The probe did not trace the system daemon itself.

The gap is consistent with the 128–147 ms after-exec residual in the [#086 profile](#oxide-run-overhead-profile-2026-10-19). Pinned Cargo rewrites uplifted binaries, but the probe did not isolate the cause of that residual or measure rebuilt test binaries.

The documented mitigation is a host setting: add the app responsible for launching builds to Privacy & Security → Developer Tools (for example `sudo spctl developer-mode enable-terminal` for Terminal). Some reports say it is incomplete. Here the agent process chain is `Paseo.app` → Paseo Supervisor → Paseo Daemon → agent → shell. Confirming the responsible app needs root.

Do not disable Gatekeeper globally. No host setting was changed by this probe.

### oxide launch check runs (2026-10-19)

oxide#093 ships this measurement as [`oxide status --launch-check`](contracts/workflow.md#launch-check). Author runs on this host, under the running Paseo, with a 4.6 MB quick-profile `oxide`:

- Completed checks took 1.2–1.8 s. Same-file medians were 1.7–3.5 ms; fresh-copy medians 193–298 ms; differences 191–295 ms. The ancestry named `Paseo.app`, which Tom says is already listed in Developer Tools. The penalty persists with it listed and Paseo not restarted, consistent with the earlier re-probe.
- Some checks hit the 3 s limit before five fresh copies ran and reported `inconclusive`: two of three right after one build, and the first of the two focused proof runs (`effigy rust:oxide-cli`, 2,750 ms reported) on its freshly built binary. One such warm-up measured 1,834 ms. Other fresh builds measured normally, including the second proof run: 1,201 ms, same-file median 1.6 ms, fresh-copy median 198.0 ms. The cause was not traced; host load average was about 11.
- After the bounded-supervision revision, three dev checks took 919–987 ms: same-file medians 2.1–2.6 ms, fresh-copy medians 149.3–158.1 ms, differences 147.2–155.6 ms. With the measuring helper process, two took 995–1,051 ms with differences 155.8–164.4 ms. The author proof runs above used the first implementation. The review side's focused offline proof (`rust:oxide-cli:launch-check`, 5 s, 68 MiB staged) measured 1,053 ms with a material penalty on the bounded-supervision revision.
- A newly written two-line `#!/bin/sh` script paid the same: about 155 ms on first launch, then 6–8 ms. The cost is not specific to Mach-O binaries.
- Two freshly linked tiny `rustc -O` binaries first launched in 258–270 ms; a copy of each in 150–164 ms. A rebuilt binary has new code-signature content and a copy does not, so the check is likely a lower bound for a rebuild's cost. That is inferred from two samples.

No setting was read or changed.

## sccache 0.18.0 sharing and slot closure (2026-10-20)

oxide#096 traced pinned sccache 0.18.0 (commit `01c35e69`) against Oxide's
slot closure ([evidence](evidence/sccache-096/README.md), [Q-017](questions.md#q-017--can-sccache-sharing-preserve-oxides-slot-writer-closure)).

- The default shared detached daemon is outside every Cargo group and gives
  no per-request closure after cancellation. It SIGKILLs only the direct
  compiler, and a running cache-hit restoration continues. That makes it a
  source no-go for release or recovery under current Oxide semantics, and
  quarantine behind it is unbounded. Other future sharing designs are not
  ruled out.
- Serial foreground servers over one persistent cache can share by source.
  Natively, each wrapped command's server sat inside Oxide's Cargo process
  group. Closure after cancellation or a crash, and concurrency, are
  unproven.
- Benefit is limited by source: linking crates are not cached, and the hash
  includes the working directory and `CARGO_*` variables. On the oxide#096
  fixture a registry dependency had no hit in a second Oxide slot, and its
  key differed. Per-slot `CARGO_BUILD_BUILD_DIR` is the source-based
  hypothesis; environments were not diffed. The workspace unit was hashed
  and stored although sccache refuses incremental units, cause untraced.
  Source caches `check` metadata-only units although the Rust notes say
  `link` is required.
- Write volume was not attributable within scope: UNKNOWN. Cache bytes are
  not writes.

Compatibility is UNKNOWN and benefit INSUFFICIENT. The three cost pairs did
not run; the S1–S3 times (1.73, 1.06, 0.83 s) are single observations, not
cost samples.

## sccache with Oxide: modes and path sensitivity (2026-10-20)

Supervisor probes on sccache 0.18.0. Each used an isolated private `SCCACHE_DIR` and `SCCACHE_SERVER_UDS`, a tiny crate with the `cfg-if` and `itoa` registry dependencies, `--offline`, and `CARGO_INCREMENTAL=0`. The live machine server was never touched.

- **Client-side mode exists** (`SCCACHE_CLIENT_SIDE`, since 0.17.0). Per the [0.18.0 architecture doc](https://docs.rs/crate/sccache/0.18.0/source/docs/Architecture.md), the CLI process runs the compile pipeline and the compiler; the daemon only holds the storage backend and stats, and each cache operation is forwarded over IPC. Under Oxide this puts every slot writer inside the build's process group, which is the closure #096 could not get from server-side mode. That is source-unverified for 0.18.0.
  - Known issues: [#2796](https://github.com/mozilla/sccache/issues/2796) ("local cache is not populated", open) and [#2804](https://github.com/mozilla/sccache/issues/2804) (stats, closed 2026-10-19).
  - In the probe, client-side mode stored cache entries but reported no hit or miss counts.
- **Rust cache hits are target-path sensitive.** Server-side stats:
  - first build at target path A: 0 hits, 2 misses;
  - the same path again after removal: +2 hits;
  - a different target path B: +2 misses, no hits.
  - Setting `SCCACHE_BASEDIRS` to the parent of both target directories gave the same result. 0.18.0 contains basedir code, but it did not normalize this Rust case. Upstream Rust basedir stripping ([#2678](https://codspeed.io/mozilla/sccache/branches/vercel%3Aupstream-pr%2Frust-basedirs)) was not observed in this build. A related correctness bug exists for C/C++ ([#2863](https://github.com/mozilla/sccache/issues/2863)).
- **Consequence:** every Oxide slot has its own path and recycled slots get new IDs, so sccache gives cross-slot or cold-slot hits only if the paths Cargo passes to rustc are identical. As shipped, it adds little to Oxide-managed builds.
- **Live machine server reading (2026-10-20 ~13:00):** Rust hit rate 70.7% (16,525 hits). Average compile 1.957 s, average hit read 0.001 s. Non-cacheable reasons: crate-type 9,826, incremental 1,014. Under the path finding, these hits come from repeated builds at unchanged target paths. That is an inference, not attributed.

Paths to complementarity, unqualified:
1. Upstream Rust basedir normalization.
2. An Oxide stable build-path alias: each concurrently active lease builds through a fixed per-index alias path to its slot, so sccache keys match across slots and projects. Combined with client-side mode for writer closure. Needs proof that Cargo does not canonicalize the alias, and an account of debug-info path effects.

### Client-side mode and alias qualification (oxide#101, 2026-10-20)

[Evidence](evidence/sccache-101/README.md). Knowledge only: one native session aborted and the second never launched (Effigy capacity timeout), so nearly everything below is source.

- **Client-side closure: UNKNOWN.** Source (sccache `01c35e69`) supports it: the client process hashes, runs rustc and restores hits into the slot (temp file then rename); the server only answers storage requests and writes its cache directory. Not exercised natively.
- **Silent fallback: UNSAFE.** `config.rs:1412-1419` turns client-side mode off whenever `SCCACHE_LOG` is set or a dist scheduler is configured; the server then compiles into slots from outside the Cargo group. Native session 1 hit this. Any integration must own the endpoint and scrub the client environment.
- **Alias spelling survives.** Cargo 1.97.1 does not canonicalize `CARGO_TARGET_DIR` or `build.build-dir` (source); `--out-dir`, `-L` and `--extern` kept the alias natively. `OUT_DIR`/`CARGO_*` values not observed natively.
- **Index change: UNSAFE by source.** Cargo drops `OUT_DIR` and build-script env from rebuild tracking (`fingerprint/dep_info.rs:308-339`, which documents the moved-target case), so a slot rebuilt under a new index keeps fresh units that embed the old alias, by then another slot's. Not demonstrated natively. Affinity and invalidation mitigations: UNKNOWN, not run.
- **Benefit: INSUFFICIENT; writes UNKNOWN.** No hit rate or cold-slot time measured. #2796 (local level empty in client-side mode) was reported on a multi-level cache; single-level local disk is unmeasured.
- **Upstream:** 0.18.0 is the latest release; Rust basedir normalization (#2678, #2794, #2918) is unmerged. If released it would remove the need for an alias but carry the same embedded-path hazard for restored objects (inference).

## Product assessment

Planner recommendation: a small coherent entry point over evidence-backed mechanisms, with three capabilities: see and explain costs, bound and reuse owned storage, and apply qualified improvements with clear trade-offs. The reusable storage core must work for ordinary developers as well as orchestrators. A daemon, editor surface or UI is still an open choice; Tom's [workflow direction](vision.md#product-workflow-direction) sets an `oxide build` / `oxide run` front door as the design target.

The broad product goal is confirmed; detailed numeric targets and mechanism choices are not. For every candidate, compare cold/new-worktree/small-edit/branch-switch/test cycles and longer-lived use. Track feedback latency, peak/retained disk space, actual observed write volume, memory/swap, contention and manual interventions. Bound measurement overhead and preserve evidence. Do not infer SSD lifetime from those metrics.

Ease-of-use evaluation should include setup steps, configuration burden, diagnostic clarity, predictable behavior under capacity pressure and recovery effort. Prefer fewer developer decisions over exposing every compiler knob. Required source/test/release behavior stays visible and preserved. No published command names or installed behavior are implied by this research; executable naming remains subject to [release](contracts/release.md) review.
