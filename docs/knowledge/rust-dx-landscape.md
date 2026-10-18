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
changed during this check. Merely having nightly does not qualify the backend.

## Quick-run strategies

Read-only research, 2026-10-18, for the `quick` run strategies in the
[workflow](contracts/workflow.md#profiles-and-strategies). None is ready to
replace "quick build, then execute" as the default.

| Strategy | State | Fit for `oxide run --profile quick` |
| --- | --- | --- |
| Quick build + execute (stable LLVM, opt-level 0, line tables, incremental) | Shipped. On Linux, [Rust 1.90 made LLD the default linker](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/) for `x86_64-unknown-linux-gnu`; the [preview post](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable) reports 7x faster linking and 40% less end-to-end time on a ripgrep incremental rebuild. | Default and fallback. |
| [Cranelift AOT](https://github.com/rust-lang/rustc_codegen_cranelift) | Nightly-only preview component `rustc-codegen-cranelift-preview`; aarch64 macOS supported; unwinding unsupported on macOS (`panic=abort` by default), experimental on Linux. Cargo's [`codegen-backend`](https://doc.rust-lang.org/cargo/reference/unstable.html#codegen-backend) can be set per package, so dependencies can stay on LLVM. | Candidate for `run` only. One build cannot mix rustc versions, so a nightly strategy needs its own slot and a second dependency set, costing storage. It also needs a nightly toolchain plus component install, which is a host change. Not for `test` on macOS. |
| [Cranelift JIT](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md) | "Highly experimental"; every dependency must be a dynamic library; no incremental compilation, so it may be slower than AOT. | Not a near-term candidate. |
| Hot patching ([Subsecond](https://docs.rs/subsecond/)) | Experimental. It patches only code wrapped in `subsecond::call` and only the tip crate, and needs the Dioxus CLI (`dx serve --hotpatch`) ([Dioxus 0.7](https://dioxuslabs.com/blog/release-070)). | Not a generic strategy: it requires source changes. Possible later opt-in for long-running apps. |

Implications: the foundation ships quick build + execute. The only near
candidate is Cranelift for workspace crates. It needs a bounded measurement on a
real edit-to-run loop that counts the separate nightly dependency set and the
abort-on-panic compromise. Until then, measure where quick-run time goes
(frontend, codegen, link) before choosing a strategy. The installed
nightly-2026-08-05 has no Cranelift component; Tom approved installing it for
the bounded measurement on 2026-10-18 ([vision](vision.md#product-workflow-direction)).

### Other run-path levers

Any rustc-based JIT or interpreter still runs parsing, macro expansion and
type/borrow checking. Those only remove code generation and linking, so
`check` time is the floor. The measurement should report check, build and link
time separately before any backend is credited.

| Lever | Finding | Fit |
| --- | --- | --- |
| No-op fast path | Oxide can record the last successful build's content identity per checkout and binary (the freshness gate records it). If unchanged, it executes without invoking Cargo, skipping Cargo's fingerprint walk. | In Oxide's control; small, safe; after the gate. |
| Optimized dependencies in `quick` | `package."*".opt-level` makes heavy dependencies run faster. Shared slots pay that compile cost once across worktrees. It needs `--config`, because the environment cannot express `package."*"`. | Candidate; measure. Every modelled command must use the same recipe, or the slot holds two dependency sets. |
| Workspace feature unification | Selecting different packages builds dependency variants with different features: churn and extra storage. A 2026-10-18 probe on 1.97.1 showed `resolver.feature-unification` is ignored without `-Zfeature-unification` ([RFC 3692](https://rust-lang.github.io/rfcs/3692-feature-unification.html)). The stable route is a workspace-hack crate, which is a project change. | `oxide status` could report duplicate dependency variants; the fix stays per project. |
| Faster linkers | macOS uses Apple's linker. [Wild](https://github.com/davidlattimore/wild) is Linux-only, has no incremental linking yet, and its Mach-O port is not ready. | Linux: LLD default already; wild later. macOS: nothing to add. |
| Dynamic dependency linking | A dylib wrapper crate re-exporting dependencies cuts link time for large graphs, but changes linking and runtime loading. | Per-project opt-in only. |
| Parallel frontend (`-Zthreads`) | Nightly; speeds frontend work on large crates. | Same nightly dependency-set cost as Cranelift; a possible extra arm of that measurement. |
| Watch mode | Rebuilds and restarts on save; cuts latency, not work. | Human loop; low value for agents. |
| [danluu/rust-interpreter](https://github.com/danluu/rust-interpreter) | rustc checks the code, then selected MIR is lowered to bytecode and run in an interpreter or AArch64 JIT. Self-described as "extremely vibe coded"; "cannot yet replace `cargo test`"; no unwinding, threads, FFI or `should_panic`; "no fallback" for unsupported code. Its results are mixed: 20% faster on one five-edit pilot, 2.7x slower on a compute-heavy target. Needs pinned nightly with `rustc-dev`; no license stated. | Not usable or adoptable. It shows the approach can work, and its ceiling is still the check time. |
| Miri / [Cranelift JIT](https://github.com/rust-lang/rustc_codegen_cranelift/blob/main/docs/usage.md) | Miri is a UB checker that runs far slower than native code. Cranelift JIT is covered above. | Not speed tools. |

## Upstream churn and disk developments (2026-10-18)

**Stale debug objects on macOS.** [cargo#16665](https://github.com/rust-lang/cargo/issues/16665) (open) reports `deps/` growth from old artifacts making incremental builds about 6× slower, and one `cargo clean` removing 345 GiB. Commenters attribute much of it to macOS unpacked debug info. Since 1.88, preserved `*.rcgu.o` objects get per-invocation names and are never overwritten. That root cause is a user claim, not a maintainer confirmation. A 2026-10-18 probe on 1.97.1 (tiny crate, four rebuilds) found:
- `debug = "line-tables-only"` with the default unpacked split debug info left 24 `*.rcgu.o` files in `deps/`;
- `debug = false` left none;
- `split-debuginfo = "packed"` left none and produced one `.dSYM`.

Implication: the `quick` recipe accumulates stale objects in long-lived shared slots, in proportion to workspace rebuilds. That costs disk and possibly build speed. The choice between packed debug info (extra `dsymutil` link time), debug off for rebuilt units, and slot recycling needs a measurement. Do not delete files inside a slot; the build-dir layout is Cargo-internal.

| Development | State | Relevance |
| --- | --- | --- |
| [Build-dir layout v2](https://blog.rust-lang.org/2026/03/13/call-for-testing-build-dir-layout-v2) | Per-unit `build/<package>/<hash>/` directories; scheduled stable in Cargo 1.100 (2026-11-12, per the beta changelog). | Unblocks Cargo's stale-unit cleanup ([cargo#5026](https://github.com/rust-lang/cargo/issues/5026)) and finer locking. It changes intermediate paths, so Oxide's plain-Cargo stray detection (`deps/`, `incremental/`) must be re-qualified at the upgrade. |
| [Cross-workspace cache goal](https://rust-lang.github.io/rust-project-goals/2026/cargo-cross-workspace-cache.html) | 2026 goal: design and a nightly prototype; conservative at first. | The upstream replacement for Oxide's per-project slots ([Cargo relationship ruling](vision.md#relationship-with-cargo)). Not available yet. |
| [Relink, don't rebuild](https://rust-lang.github.io/goals/2026/roadmap-fast-builds.html) | Did not complete in 2025H2; on the 2026 fast-builds roadmap; claims 5–10× fewer downstream rebuilds for body-only edits. | The largest future edit-loop win; nothing for Oxide to emulate safely. |
| [`hint-mostly-unused`](https://blog.rust-lang.org/inside-rust/2025/07/15/call-for-testing-hint-mostly-unused/) | Nightly; per-dependency hint that defers code generation for unused items; regresses if applied globally. | Narrow, profile-guided; only helps when the dependency is built. |
| Cargo home GC ([1.88](https://blog.rust-lang.org/2025/06/26/Rust-1.88.0/)) | Stable automatic cleanup of downloaded/cached files (3 months / 1 month unused). Never runs with `--offline` or `--frozen`. The target dir is not covered. | Agent and Oxide proofs often run `--offline`, so their Cargo home may never be cleaned. Worth surfacing in `oxide status`. |
| [Cargo script](https://rust-lang.github.io/goals/2026/cargo-script.html) | Frontmatter stabilization in progress; not stable on 1.97.1. | Single-file scripts; each script otherwise gets its own target dir. |

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
- Debug info is the open condition. Until a binary is relinked, its macOS debug map (`N_OSO`) points at A's `deps/*.o`. That includes the `target/debug/*` outputs Cargo hard-links into place from B.
  - It harms nothing while A still exists unchanged, as a clone source would.
  - Once A is gone or rebuilt, debugging an unchanged binary can lose its debug info.
  - The probe did not run a debugger.

Clone cost (Oxide repository, 2,126 files, 244 MiB logical):
- `clonefile` took 12.6 ms. Host free space fell 1.1 MiB from before the clone to after the rename.
- All 300 sampled files kept the same first physical extent (`F_LOG2PHYS_EXT`). A plain-copy control file moved to a new extent, which is direct evidence of copy-on-write sharing. Inodes differed; sizes, modes and mtimes matched.
- Host free space fell another 152 MB during the edit rebuild: the clone diverges by roughly what the rebuild writes.
- Host free space is shared and noisy. These figures are not exact clone allocation or SSD writes.

Implication: on this host and toolchain, cloning a slot per worktree, or warm-starting from main's slot, is mechanically sound for Cargo freshness and rebuild correctness on both layouts. A product design still has to settle the debug-map condition: keep the source slot unchanged while clones exist, or accept relinking. One fixture and one repository do not prove universal portability, debugger behaviour or savings. Two recipe slips are corrected in the summary:
- the layout v2 build script is named `build_script_build`, so the recipe's run counter missed it;
- the hit classifier mislabels v2 unit outputs under `build/<pkg>/<hash>/out/`.

## Product assessment

Planner recommendation: a small coherent entry point over evidence-backed mechanisms, with three capabilities: see and explain costs, bound and reuse owned storage, and apply qualified improvements with clear trade-offs. The reusable storage core must work for ordinary developers as well as orchestrators. A daemon, editor surface or UI is still an open choice; Tom's [workflow direction](vision.md#product-workflow-direction) sets an `oxide build` / `oxide run` front door as the design target.

The broad product goal is confirmed; detailed numeric targets and mechanism choices are not. For every candidate, compare cold/new-worktree/small-edit/branch-switch/test cycles and longer-lived use. Track feedback latency, peak/retained disk space, actual observed write volume, memory/swap, contention and manual interventions. Bound measurement overhead and preserve evidence. Do not infer SSD lifetime from those metrics.

Ease-of-use evaluation should include setup steps, configuration burden, diagnostic clarity, predictable behavior under capacity pressure and recovery effort. Prefer fewer developer decisions over exposing every compiler knob. Required source/test/release behavior stays visible and preserved. No published command names or installed behavior are implied by this research; executable naming remains subject to [release](contracts/release.md) review.
