# Rust development experience: research landscape

Read-only research, checked 2026-10-09. This file owns findings and candidate remedies, not the project plan. [Vision](vision.md) owns the desired experience; Queue owns order, briefs and execution authority. Oxide has run bounded qualification work; [qualification readiness](qualification-readiness.md) records its scope and limits. Tool availability and compatibility must be checked on selected pinned versions before adoption.

## Evidence and implications

The Rust Project's [2025 compiler performance survey](https://blog.rust-lang.org/2025/09/10/rust-compiler-performance-survey-2025-results/) identifies distinct problems in incremental feedback, type checking/IDE use and clean/CI builds. It also highlights linking, debug information, editor/build contention and limited visibility into build costs. This supports researching several workflows rather than assuming one cache can solve them all. Survey findings are not measurements of Tom's machine or a diagnosis of his incremental-cache degradation.

Cargo's accepted [2026 cross-workspace cache goal](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html) seeks shared build units, reduced duplication and eventual cleanup. Its stated sequence is to design and implement an experimental nightly cache, initially excluding build scripts and proc macros, then expand coverage and work toward stabilization; the goal says stabilization may not finish in the 2026 period. The linked [Cargo cache tracking issue](https://github.com/rust-lang/cargo/issues/5931) remains open and records design work. This is a real upstream direction, but it does not establish a supported stable cross-workspace cache, its exact current nightly coverage, or fit with Queue/Nucleus ownership.

Cargo does provide a useful stable building block today: the [Cargo 1.91 changelog](https://doc.rust-lang.org/cargo/CHANGELOG.html) (2025-10-30, stabilize `build.build-dir` #15833 #15840) marks `build.build-dir` stable, and the [configuration reference](https://doc.rust-lang.org/stable/cargo/reference/config.html#buildbuild-dir) documents `CARGO_BUILD_BUILD_DIR` and separate storage for intermediate artifacts. Cargo's [build-cache reference](https://doc.rust-lang.org/stable/cargo/reference/build-cache.html) says the build-dir layout is internal and subject to change. This relocates intermediates; it does not give Oxide an exclusive lease, bounded storage, cross-path correctness proof, cleanup policy or exact-run evidence. A global `CARGO_TARGET_DIR` alone still shares final output/cleanup scope and does not supply those ownership rules; Cargo's goal also documents those drawbacks.

The smallest useful gap-filling mechanism is therefore to qualify stable build-dir redirection on the exact pinned Cargo version, with an Oxide lease/budget/consumer adapter around it. Do not reimplement Cargo fingerprints or artifact selection. Keep this redirection replaceable. Revisit it when a supported Cargo release documents the needed shared-unit behavior and coverage, and measured exact-head tests show that it preserves the same build-script/proc-macro/path correctness, bounded lifecycle, exclusive ownership and run evidence. Roadmap acceptance, nightly availability or build-dir stability by themselves are not replacement evidence. This applies the [Cargo relationship ruling](vision.md#relationship-with-cargo): use what works now, fill the practical gap, and let upstream capabilities replace the temporary mechanism when proven. Follow the [older artifact-cache issue](https://github.com/rust-lang/cargo/issues/5931) and the [build-artifact collection issue](https://github.com/rust-lang/cargo/issues/13136) as context, not adoption proof.

Cargo currently documents [separate build/target directories](https://doc.rust-lang.org/cargo/reference/build-cache.html), but its intermediate layout is internal and subject to change. [Automatic global cache collection](https://doc.rust-lang.org/cargo/reference/config.html#global-caches) covers registry/Git sources, not build-artifact tracking. Pinned Cargo 1.97.1 therefore has no supported way to share compiled dependency units while keeping workspace, build-script, incremental and test outputs private. Queue Spec 028 already shares `CARGO_HOME` downloads; that is source reuse, not compiled-artifact reuse. A dependency-only compiled cache is not a next Oxide delivery on this toolchain. Moving intermediate storage is still the useful mechanism candidate, not sufficient ownership, portability or budgeting policy. The existing [storage invariants](architecture.md) continue to apply. The [Task 019 design](qualification-readiness.md#task-019-useful-bounded-reuse-across-changing-tasks-design-only-2026-10-10) records that disposition.

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

Tom's [2026-10-18 follow-up](vision.md#product-workflow-direction) rejects
transparent interception (toolchain shim, `PATH` shim, shell alias) in favour of
explicit `oxide build` / `oxide run` plus agent guidance. The inner hooks remain
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
changed during this check. Merely having nightly does not qualify the backend.

## Product assessment

Planner recommendation: a small coherent entry point over evidence-backed mechanisms, with three capabilities: see and explain costs, bound and reuse owned storage, and apply qualified improvements with clear trade-offs. The reusable storage core must work for ordinary developers as well as orchestrators. A daemon, editor surface or UI is still an open choice; Tom's [workflow direction](vision.md#product-workflow-direction) sets an `oxide build` / `oxide run` front door as the design target.

The broad product goal is confirmed; detailed numeric targets and mechanism choices are not. For every candidate, compare cold/new-worktree/small-edit/branch-switch/test cycles and longer-lived use. Track feedback latency, peak/retained disk space, actual observed write volume, memory/swap, contention and manual interventions. Bound measurement overhead and preserve evidence. Do not infer SSD lifetime from those metrics.

Ease-of-use evaluation should include setup steps, configuration burden, diagnostic clarity, predictable behavior under capacity pressure and recovery effort. Prefer fewer developer decisions over exposing every compiler knob. Required source/test/release behavior stays visible and preserved. No published command names or installed behavior are implied by this research; executable naming remains subject to [release](contracts/release.md) review.
