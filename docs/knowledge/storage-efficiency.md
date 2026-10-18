# Storage-efficiency findings

Source findings and bounded preparation evidence. This note records current
behavior and evidence limits. It does not establish measured storage savings
or authorize changes in the inspected projects.

## Component preparation

The reusable project routing requirements live in the
[optimization adoption policy](contracts/optimization-adoption.md#component-lazy-preparation).

At Acowtancy commit `70b6d7c8c83a48a85abbbd24717ed8064898cf5a`,
worktree setup no longer seeds host Rust targets. Supported host Rust selectors
prepare only their declared Farmyard, Desktop or Silo component; composed
selectors request an explicit union. JS/docs-only setup prepares none.
`effigy worktree:rust:seed --component <name>` selects a component, repeated
flags select a union, and unknown components refuse. Explicit `--path`
selection remains available. An unfiltered direct seed command remains an
operator opt-in for all three targets. Acow owns the
[route inventory and preparation policy](https://github.com/acowtancy/acowtancy-v2/blob/70b6d7c8c83a48a85abbbd24717ed8064898cf5a/docs/knowledge/operations/267-qa-speed.md#shared-compile-cache).

The copy mechanism first tries `cp -c -R` and falls back to
`cp -R`; a Rust compiler mismatch uses a separate toolchain-keyed source
target, built only when the primary checkout is clean. Cache readiness is
published after the matching compiler and Cargo build are confirmed.

The preceding source assessment at `5db16a550fc8bf572465be28d1dac11117fa7538`
identified unconditional setup of all three targets. Component selection now
avoids that unrelated preparation; it does not add a shared mutable compiler
manager. Existing no-overwrite, primary-target, compiler-matching,
dirty-primary, locking and readiness protections remain. Container target
ownership is separate.

Independent review at `91d6e4b2af18e977c8a323351004d67b799ec749`
passed all five preparation, routing, protection, isolation and scope gates.
The author and reviewer each ran 18 passing synthetic fixture tests in
separate fresh roots. The reviewer also ran the focused seed compile/format
and documentation checks; no real Cargo or rustc was invoked. Component-only
and union fixtures leave unrelated destinations absent. These fixtures prove
avoided unrelated preparation, not real component compilation or a complete
milestone QA result.

On APFS a clone can share file data until it diverges; the plain-copy fallback
can retain a full copy. Allocated bytes, ordinary-use latency, write volume and
SSD lifetime savings remain unmeasured. The evidence does not quantify them.

Acowtancy's existing Rust profiles already distinguish ordinary and test
work: Farmyard `dev` keeps incremental compilation and debug level 1, while
its `test` profile keeps workspace line tables, removes dependency debug info
and disables incremental output. Desktop also has separate dev and test
profiles. These settings are existing project policy, not evidence for a new
profile change.

Acowtancy's recorded sccache 0.18.0 trial at the pinned Farmyard toolchain
reported zero hits for fresh targets and for a warm cache at a different
target path; forced recompilation at the same path reported three hits. The
trial is configuration-specific. It does not reject shared compiler caching
for every project or configuration.

The pinned [sccache 0.18.0 Rust notes](https://github.com/mozilla/sccache/blob/v0.18.0/docs/Rust.md)
require incremental compilation to be disabled and exclude crates invoking the
system linker, including binaries and proc-macro crates. They also warn about
untracked filesystem reads by procedural macros. The [Rust implementation](https://github.com/mozilla/sccache/blob/v0.18.0/src/compiler/rust.rs)
explicitly rejects incremental arguments. These constraints limit its value
for the iterative incremental-on workflow now adopted by Nucleus; they do not
exclude eligible dependencies or one-shot workloads. An unchanged warm Cargo
slot can avoid invoking rustc at all, leaving no compiler-result lookup to
accelerate. That is a reason to identify additional cacheable rebuild work
before another trial, not a measured universal overhead claim. No `sccache`
was found on the planner's current PATH during this read-only assessment; no
installation or compilation was performed.

Acow's current operations267 also records a concrete failed selective-clean
configuration: cleaning workspace packages on source changes left shared
registry artifacts unusable (`E0463` for `getrandom` and `digest`, milestone
`1f5e26c5`), and that cleanup was removed. This is configuration-specific owner
evidence, not proof all `cargo clean -p` usage fails. It reinforces the need
to qualify a package-clean policy before offering it as dependency-only cache
isolation; it is not an approved cleanup path for the Nucleus pool.

## Profiles and debug access

At Nucleus commit `92dfb60cbae3b2eece70f03aa448e7b0cebc61d7`, the root Cargo
profile sets `dev` debug info to line tables and defines `full-debug` with full
debug info. Its validation guidance keeps those routes distinct. The workspace
does not declare a `rust-version`; no MSRV is inferred here. Acowtancy's
existing test-profile reductions and Nucleus's explicit full-debug route leave
no source-backed case for another profile edit in this assessment. Disabling
incremental output can increase rebuild work, and final target size alone
would not establish reduced disk writes. That assessment covered the inspected
consumer projects; Oxide later added its own opt-in profiles whose comparison
were recorded as a no-go. The [stable Cargo profile contract](contracts/stable-cargo-profiles.md)
owns them and the [observations below](#stable-cargo-profile-observations-2026-10-16-no-go)
are retained only as unqualified readings.

## Incremental and debug storage measurements

Supervisor-reported, single busy-machine observations on Darwin 25.6 with APFS,
rustc 1.97.1 (`8bab26f4f`) and matching Cargo. Nucleus's dev profile is
`line-tables-only` with the macOS default split-debuginfo mode (unpacked). The
raw logs were not preserved, so these figures are attributed to the supervisor
report and not re-observed here.

| Mode (cold + two changed no-run builds) | Build time | Loose `deps` objects | Incremental state | Notes |
| --- | --- | --- | --- | --- |
| Incremental on, unpacked | 60 s + 38 s = 98 s | 12,303 | about 3 GB | Object set grows across builds. |
| Incremental off, unpacked | 66 s + 107 s = 173 s | 408, unchanged after warm | none | Object set flat. |
| Packed, incremental setting not reported | 82 s + 59 s = 141 s | zero loose | about 3 GB incremental state and about 2x deps, as reported | The source report does not state the incremental setting for this row. Do not read it as incremental off. |

Method and caveats:

- Compile-only no-run builds. No test execution is in these figures.
- Packed-mode file:line and LLDB behavior were not tested.
- The packed row's incremental setting is not in the source report. It is not a
  clean comparison with the off/unpacked row, and it does not establish a packed
  effect on incremental state.
- The 37.4 GB `deps` object figure in the source report is apparent size across
  139,421 names. It is not proven stale or reclaimable. It may overlap the 38.4 GB
  incremental figure through hardlinks and multiple variants.
- Incremental off does not show lower cumulative writes. It removes retained
  incremental state; changed-crate recompilation still writes objects.
- Cache sharing does not prevent per-slot growth. Shared warm reuse is a separate
  quantity from bounded growth and actual writes.

The proposed policy, frozen one-shot comparison, and private opt-in launcher
are in the [build-context policy](contracts/build-context-policy.md). The
[direct comparison below](#direct-incremental-comparison-2026-10-15) measures
the limited edit-loop tradeoff; production policy and shared reuse remain unqualified.

## Direct incremental comparison (2026-10-15)

The planner ran the approved comparison directly, following Tom's correction
to the execution process in [vision](vision.md#authority-boundaries). One
real Effigy heavy run settled passed, exit 0, with no escaped descendants:
`a36ede60da7df4f274417117415517c934a50784b7d0227d3f6c0cabec27febb`,
epoch 1, caller `oxide:planner:direct-incremental:20261015`, request
`4822cebb-4d4f-4c9a-8a84-e9f85efcc985`.
The original preparation-through-capture window was **165.2 seconds**,
with eight executed positive test runs plus Cargo/rustc version probes
(ten counted top-level commands; nine were Cargo). Both expected negative
backtrace tests also ran, using the already-built executables.

Both modes used Nucleus `fb2bd0449ede2d9d2089a47b0559182bd413b91f`, tree
`b47caf2ffb7e4430fc5e78411a5b475203126194`, rustc/Cargo 1.97.1,
line-tables-only debug and unpacked split debug info. The command was
`effigy check:core:test -- -p nucleus-core --test packages -v -- --exact committed_workflow_packages_parse`.
Each mode had a fresh private target inside the same fixed 15g APFS image.
No existing target was scanned or changed. One source clone and contained
Cargo source cache were used sequentially; both modes restored the source
cleanly afterward. Exact setup, comments, environment handling and commands
are retained with the evidence.

Wall times below are `/usr/bin/time -l` real seconds, excluding the
measurement controller's polling overhead:

| Step | Incremental off | Incremental on |
| --- | ---: | ---: |
| Cold target | 55.72 s | 46.19 s |
| Comment appended to library source | 36.99 s | 7.75 s |
| Comment appended to selected test source | 3.00 s | 0.69 s |
| Unchanged, test executed again | 0.09 s | 0.08 s |
| Loose `deps/*.o`, cold → library edit → test edit → unchanged | 172 → 172 → 172 → 172 | 1,053 → 1,923 → 1,941 → 1,941 |
| Whole-target allocated size (`du -sk`), cold → unchanged | 1.773 → 1.785 GiB | 2.307 → 2.787 GiB |
| `deps` + `incremental` allocated union by unique inode, cold → unchanged | 1.269 → 1.282 GiB | 1.803 → 2.283 GiB |
| Selected panic has a backtrace frame with file:line | yes | yes |

Every positive invocation executed the selected test: eight passes, no
failures. The unchanged runs compiled no `nucleus-core` unit. Captured rustc
lines show line-tables-only/unpacked in both modes, with incremental state
only in the on mode. The negative tests had the expected missing-packages
panic and a real `tests/packages.rs:<line>` backtrace frame in **both** modes;
the panic header alone was not counted.

This supports a concrete tradeoff: in this short sequence, incremental made
the library edit **4.77 times faster**, while the final whole target used
about **1.00 GiB more allocated space**. Incremental off retained no
`incremental/` state and its loose object count stayed flat. Incremental on
added 888 loose objects across the two edits. This demonstrates growth in
this sequence, not unbounded growth or proof that any object is stale.

Hardlinks matter: a post-run scan found 968 shared inodes between the on
mode's `deps` and `incremental`, representing 368,418,816 allocated bytes.
The union row counts those once. Separate directory totals must not be added.
oxide#080 later measured ten edits per arm on Oxide's workspace: unpacked
added 411 names per rebuild, almost all hardlinks to live incremental work
products; packed and workspace debug-off added none ([landscape](rust-dx-landscape.md#stale-debug-object-measurement-2026-10-18)).
Allocated observations can also differ between reads: the off-mode `deps`
scan after settlement reported 1,345,179,648 allocated bytes, versus
1,376,321,536 at the unchanged-step sample, with no further build. The cause
was not investigated. These are timestamped filesystem observations, not
physical reclaim estimates.

The retained backing plus host-side files occupied about 4.28 GiB, excluding
the mounted view to avoid counting the same storage twice. The image remained
below its 14 GiB stop threshold; the 20 GiB raw host reserve was checked
throughout. Raw logs total 274,340 bytes. The fresh root/image remain preserved
at `/private/var/tmp/oxide-direct-incremental-20261015-4822cebb`; nothing was
deleted or detached.

Limits: this is one busy-machine run, off first. Extracted registry sources
and OS caches were warmer for on. The cold-time difference cannot be isolated
as an incremental effect. The edits were comments, so the result does not
predict arbitrary behavior-changing edits. It is a narrower executed-test
workflow than the supervisor's compile-only benchmark. No attributable SSD
writes, SSD lifetime, LLDB/full-debug custody, long-run object liveness or
cross-task shared-slot reuse was measured. The original four-Cargo,
45-minute shared-reuse trial remains separate.

**Recommendation:** retain lifecycle-aware policy as the direction. Iterative
agent work can benefit from incremental just as interactive human work can;
turning it off is a real speed/storage compromise. Consumer adoption is
separate from Oxide activation; Nucleus's decision below supersedes its
managed-agent stopgap without claiming a hard storage bound.
Qualify a hard per-slot budget and safe whole-idle-generation retirement;
do not substitute selective `.o` deletion or silently flip incremental state
inside a partition. Unchanged Cargo reuse remains useful in either mode.
No more comparison-launcher work is needed to make this policy decision.

Raw commands, logs, receipts, the disposable recipe and hashes are attached
under [comparison evidence](evidence/incremental-comparison-20261015/summary.json).
The [measurement recipe](evidence/incremental-comparison-20261015/measurement-recipe.txt),
[raw library-edit logs](evidence/incremental-comparison-20261015/raw/off-E1.log),
[matching on log](evidence/incremental-comparison-20261015/raw/on-E1.log),
[off backtrace](evidence/incremental-comparison-20261015/raw/off-backtrace.log),
[on backtrace](evidence/incremental-comparison-20261015/raw/on-backtrace.log) and
[authenticated settlement](evidence/incremental-comparison-20261015/follow.json)
make the result independently inspectable without another build.

## Stable Cargo profile observations (2026-10-16): no-go

The oxide#068 comparison is an explicit **no-go**. Independent exact-head
review recorded `bounded_comparison: fail` at PR 68 review comment 6220360222,
and the follow-up review at head `0d88244` (review comment 6220502241)
confirmed `opt_in_profiles: pass`, `correctness_and_debug: pass`,
`truthful_delivery: pass` and `bounded_comparison: fail` because the author ran
four fresh sessions (2, 2, 8 and 8 Cargo driver invocations, 20 in total) with
fresh roots, images, clocks and call budgets instead of the brief's one
original capped session, because one complete run was unmatched, and because
the recorded session's timer starts after submission staging. A later
valid-looking sequence cannot repair those violations, and this brief
authorizes no further session, so the gate cannot be repaired by the author.
The [stable Cargo profile contract](contracts/stable-cargo-profiles.md) owns
the profile recipe, the no-go disposition and the limits; the numbers below are
retained only as **unqualified observations**, not as the approved comparison
and not as acceptance evidence for adoption.

The observations below come from one genuine Effigy heavy-admitted run (run
class `heavy`, outcome `passed`, run
`d3b79d01e089c8647cb03db0df40dafaad52cd2925226ad416498f16feac50cb`). Two
source-identical disposable copies of the pushed implementation commit
`c762a731276c6677993002064ef5287676344fde` (tree
`c3c98d78a59cf1b8ed8973f67a66404bd0554dfd`) received the same appended public
constant. The default `test` profile and `oxide-light` each ran a cold test
build, the identical edit rebuild and an unchanged rerun; one explicit
`oxide-full-debug` cold control followed. Every positive call executed the
same probe test and feature set. Eight Cargo driver invocations ran in a
67.0-second session inside one fresh 16 GiB APFS payload volume; the 20 GiB
raw host reserve held throughout. The 67.0 seconds exclude the staging that
preceded the recorded clock, and this session is one of four rather than the
brief's single original session.

Unqualified observations (not acceptance evidence):

| Step | Default `test` | `oxide-light` |
| --- | ---: | ---: |
| Cold test build | 14.24 s; 276.3 MiB whole target | 11.98 s; 223.3 MiB whole target |
| Identical source edit rebuild | 6.41 s | 2.81 s |
| Unchanged rerun (test executed) | 0.26 s | 0.26 s |
| `deps` + `incremental` union, cold | 260.6 MiB | 208.5 MiB |
| `deps` + `incremental` union, after edit | 346.2 MiB | 293.1 MiB |
| Workspace unit debug (`-C debuginfo`) | `2` | `line-tables-only` |
| Dependency / build-time unit debug | `2` / none | none (`strip=debuginfo`) |

The unchanged rerun rebuilt no workspace unit in either mode. The
`oxide-full-debug` control compiled in 12.49 s, used a 304.2 MiB whole target
and resolved `-C debuginfo=2` for workspace, dependency, build-script and
proc-macro units. The default `test` profile resolved the same full workspace
and dependency debug information it resolved before the profiles were added,
and no default profile was overridden.

Read as, in that one session: the light profile kept the executed tests,
unwinding, assertions and overflow checks, rebuilt the small semantic edit
about 2.3 times faster, and used about 53 MiB less allocated build storage.
Those directions are not established as the brief's bounded comparison, so no
adoption follows. No attributable SSD writes, SSD lifetime, debugger-variable
custody or general project-wide speed claim follows either.

The three harness corrections that preceded the recorded session are recorded
in [attempt sessions](evidence/stable-cargo-profiles-20261016/sessions.json)
rather than hidden: each used a fresh root, fresh image, fresh clock and its
own call budget, and no running clock or cap was reset. Raw logs, the exact
recipe, the per-unit resolved settings and the authenticated run status are in
the [evidence package](evidence/stable-cargo-profiles-20261016/summary.json).
The measured build inputs are the implementation commit in the packet; later
commits are evidence and documentation only.

## Nucleus consumer adoption (2026-10-15)

Tom adopted the direct comparison for Nucleus in
[`ff43e69701f08e5a9ddf84d424a381dea8d033de`](https://github.com/inflatable-cookie/nucleus/commit/ff43e69701f08e5a9ddf84d424a381dea8d033de).
Its [Contract 002](https://github.com/inflatable-cookie/nucleus/blob/ff43e69701f08e5a9ddf84d424a381dea8d033de/docs/knowledge/contracts/002-core-requirements.md)
cites Oxide `943c6f9`: managed agents build with incremental on again;
one-shot validation retains `CARGO_INCREMENTAL=0`. This supersedes Nucleus's
earlier agent-wide incremental-off stopgap at `fb2bd04`.

Nucleus owns this policy and its closeout. Its contract states that closeout
deletes the task workspace, limiting retention to the task lifetime. That is
not a hard byte bound during a long-running task, does not prevent a task's
target from growing, and does not reduce cold-build duplication or establish
SSD-write savings. Oxide's size-limited owned build storage remains the
longer-term bound. No shared slot, cross-task target reuse, object GC, Queue
default change or production Oxide activation follows from this adoption.

## Bounded incremental comparison attempt (2026-10-15)

One checked launch packet (caller
`oxide:planner:incremental-comparison:053`, request UUID
`be2acbd8-7f3e-4488-b91d-6781ec0d6788`) submitted the frozen recipe through
the private launcher's outer bootstrap on 2026-10-15. The reviewed launcher
SHA256 `36e89aefbe6d4a69659f05f37f6963d583f5d0376fa8648a0b28929e2a4084a0`,
registry-plan digest `ce5c0381…baa` and semantic request-body digest
`c0427e75…401` all matched the brief before effects. The resource-plan names
were unused, host free space was 1,169,873,059,840 bytes (about 1.06 TiB, above
the 20 GiB reserve), and the Nucleus commit, tree and `Cargo.lock` pins resolved
exactly.

The forced read-only plan check returned `live_ready=true`. The outer bootstrap
opened its single 1200 s session at `2026-10-15T14:00:14Z`, staged exactly
`effigy.toml`, `resource-plan.json` and `binding.json` under the owned parent,
and submitted the sole heavy selector. The heavy host run was admitted by Effigy
(`admittedAt` `14:00:14.422Z`) and launched at `14:00:14.434Z`, then settled
`failed` at `14:00:15.063Z` (`exitCode 1`, 629 ms, `escapedDescendants: []`). No
capacity wait was needed (`position: null`).

The heavy host run was admitted and launched, but the inner comparison launcher
failed before its own admission validation (`verify_admission` /
`validate_staged_session`): no Cargo or rustc invocation
(`count_delivered=0`), no source clone, no APFS image and no measurement. The
captured output was
`sh: …/scripts/build_context_comparison.py: Permission denied` and
`task command failed … (code=126)`.

Observed cause: Effigy deserialises `[tasks.<name>.run]` through
`ManifestManagedRun`, where a TOML array is a *sequence of separate shell
commands*, not one argv vector. The staged manifest emitted
`run = [python3, <launcher>, "--live", …]`, so Effigy ran `python3` with no
arguments and then tried to execute the launcher script path directly. The
repository records `scripts/build_context_comparison.py` as git mode `100644`
(not executable) at the pinned base, so the direct exec failed with code 126.
The 050/051 design note that the staged entry runs "through an absolute
interpreter path" is not satisfied by an argv-shaped array.

A second, inferred seam is recorded but was not reached: Effigy created
`.effigy/{locks,runtime,reports}` inside the staged root during launch, while
the admitted inner launcher requires exactly the three staged entries and would
refuse `staged_session` on a fourth. It needs confirmation by the correction.

Disposition: a bounded failure in the inner comparison launcher before its own
admission validation, with a precise producer defect. It is not a qualified
comparison, not shared warm reuse, not evidence about
object deletion or production policy, and it produced no incremental-on/off
timing, storage, allocation or write data. The frozen methodology is unchanged;
that failed attempt supplied no comparison evidence. The later bootstrap
correction and direct measurement are distinct; the observations above do
not turn this attempt into a successful run. The
staged root and the task control directory are retained read-only for review;
nothing was deleted, detached or repaired. The exact receipts are in the
[attempt evidence record](evidence/build-context-comparison-053-attempt.json).
The original shared-reuse four-Cargo, 45-minute trial remains unstarted.

## Bootstrap correction and compiler-free smoke (2026-10-15)

A separately scoped producer correction (`oxide#054`) fixed the three observed
integration defects from the 053 attempt and proved the corrected bootstrap.
It did not run the comparison.

- **Launchability.** The staged heavy entry is now one POSIX-quoted `sh -c`
  command string instead of a TOML argv array. The 053 `run = [python3,
  <launcher>, …]` array was a *sequence* of separate shell commands in
  installed Effigy, so it executed bare `python3` and then the non-executable
  launcher path (code 126). The corrected launcher is launchable through
  installed Effigy; the comparison itself remains unrun and unqualified.
- **Runtime state.** Installed Effigy creates `.effigy/{locks,runtime,reports}`
  in the admitted root before the task command runs, so the previous
  exact-three-entry check rejected genuine state. The admitted bootstrap now
  accepts exactly that one Effigy state directory narrowly, while the pinned
  files keep their mode/owner/link/digest checks and arbitrary extra entries
  still refuse.
- **Clock.** `time.monotonic_ns()` on this host's Python 3.9.6 is process-local
  (tens of milliseconds since process start) and does not share a basis with a
  spawned child, so the 053 absolute deadline could not transfer. The deadline
  clock is now `time.clock_gettime_ns(CLOCK_MONOTONIC)`, the shared native
  basis, bound in `binding.json`. Recorded limitation: `CLOCK_MONOTONIC` does
  not advance during machine suspend, so a suspend is not charged against the
  budget.

One genuine compiler-free smoke through installed Effigy heavy admission
(caller `oxide:planner:bootstrap-smoke:054`, request UUID
`e09afaa3-0694-47a4-996a-c4077a413f60`, staged root
`/private/var/tmp/oxide-build-context-cmp-smoke-e09afaa3`) settled `passed`
(exit 0, 655 ms). The admitted inner role reached the real
admission/staged-root validation, adopted the original deadline across
processes and returned the smoke sentinel before any image, source, registry,
tool or compiler operation (`count_delivered=0`, `effects_executed=false`). The
reserved mount and backing names were never created. This is a bootstrap proof,
not a comparison: no incremental-on/off timing, storage, allocation, write,
test-execution or reuse result exists. See the
[bootstrap and smoke record](contracts/build-context-policy.md#bootstrap-correction-and-compiler-free-smoke-2026-10-15)
and the [smoke evidence record](evidence/build-context-comparison-054-smoke.json).

## Disposable output and retention

At Queue commit `7b9b840354a638a529ab023542903949b5b562ac`, Spec 028 keeps
Cargo build outputs task-local while sharing downloaded Cargo sources. Its
close path discovers package and workspace target directories and removes
only task-local Cargo targets and `node_modules` after the clean, published
head, idle-thread and no-live-run proofs. Cleanup checks configured Cargo
target paths, tracked content and symlink ownership; unproved roots are
retained and reported. Failed or cancelled unmerged worktrees are retained
under the owning workspace policy. Run evidence and preserved artifacts have
separate owners and are not disposable build output.

Nucleus Contract 002 likewise separates build-output retention from source,
task history and evidence retention. It retains failed or cancelled unmerged
worktrees for inspection and does not allow age alone to authorize removal.
The reviewed controls already bound cleanup to ownership and settlement
proofs; this review found no safe basis for shortening retention or deleting
older or uncertain resources.

## Evidence limits

The initial assessment inspected Acowtancy `5db16a550fc8bf572465be28d1dac11117fa7538`,
Paseo Northstar Queue `7b9b840354a638a529ab023542903949b5b562ac`, and Nucleus
`92dfb60cbae3b2eece70f03aa448e7b0cebc61d7`. Oxide's documentation base was
`91343a280ba275079fc5f329bb3b07d419d9cdb9`.

That read-only assessment did not run builds, tests or Cargo metadata, inspect
target trees, measure allocated storage or disk writes, or change consumer
state. The separately owned component-preparation evidence above uses small
synthetic targets and applies to Acowtancy's host routes only.
Apparent target size is not allocated storage. A smaller retained target is
not proof of fewer SSD writes, and no SSD-life estimate follows from these
findings.
