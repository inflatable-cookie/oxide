# Opt-in stable Cargo development profiles

This contract owns Oxide's own opt-in Cargo profiles for everyday Rust
feedback: `oxide-light` and its explicit `oxide-full-debug` control. They are
an explicitly named recipe on the pinned stable Cargo 1.97.1 toolchain. They do
not change the default `dev`/`test`/`release` profiles, any consumer, any
global Cargo configuration or the pinned toolchain, and nothing selects them
automatically. The [Rust DX landscape](../rust-dx-landscape.md#stable-profile-and-backend-eligibility)
owns the upstream eligibility finding; the no-go disposition below owns the
recorded observations, which are not acceptance evidence.

## Declared profiles

Both profiles inherit `dev`, so incremental compilation, `panic = "unwind"`,
debug assertions, overflow checks and optimization level 0 are kept for
workspace units. `oxide-light` additionally keeps only workspace line tables
and disables debug information for non-workspace dependency and build-time
units. `oxide-full-debug` explicitly restores full debug information for the
workspace, dependency and build-time units instead of inheriting a reduced
override.

```toml
[profile.oxide-light]
inherits = "dev"
debug = "line-tables-only"
incremental = true
opt-level = 0
panic = "unwind"
debug-assertions = true
overflow-checks = true

[profile.oxide-light.package."*"]
debug = false

[profile.oxide-light.build-override]
debug = false

[profile.oxide-full-debug]
inherits = "dev"
debug = "full"
incremental = true
opt-level = 0
panic = "unwind"
debug-assertions = true
overflow-checks = true

[profile.oxide-full-debug.package."*"]
debug = "full"

[profile.oxide-full-debug.build-override]
debug = "full"
```

## Adoption recipe

Adoption is per invocation; there is no default, environment variable or
Cargo configuration change, and no consumer selects these profiles:

```sh
cargo test --profile oxide-light --test profile_backtrace     # iterative feedback
cargo check --profile oxide-light --all-targets --locked      # type/borrow feedback
cargo test --profile oxide-full-debug --test profile_backtrace # richer debugging
```

Each named profile writes its own artifact directory under the checkout's
target directory (`target/oxide-light`, `target/oxide-full-debug`), separate
from the ordinary `target/debug` tree. Switching profiles therefore does not
invalidate the default build, and a light build never reuses or overwrites the
full-debug artifacts. The first use of a profile is a cold build in its own
directory.

## Debugging tradeoff

In `oxide-light` builds, dependency and build-time (build script and proc
macro) units carry no debug information. Dependency source/type/variable
debugging and dependency stack frames resolved from debug information are
therefore unavailable in a light build, and workspace frames are the only
frames guaranteed to keep `file:line`. Rich debugging requires an explicit
`oxide-full-debug` rebuild in its separate artifact directory. Oxide makes no
debugger-variable, type-inspection or old-source-retention guarantee beyond
the evidence actually exercised.

Workspace behavior that the light profile keeps is proven by the focused
native probe in `tests/profile_backtrace.rs`: a panic inside a workspace helper
still unwinds to `catch_unwind` with its original payload, the panic's own
backtrace resolves more than one workspace frame to `file:line`, and debug
assertions and overflow checks remain active. The panic's initial location
alone is not accepted as that proof.

## Verification selectors

`effigy rust:profile-light` and `effigy rust:profile-full-debug` each compile
the workspace plus the probe into a fresh private target directory and check
the resolved rustc settings Cargo actually passed, not the profile name or an
inheritance statement:

- workspace units: `-C debuginfo=line-tables-only` and `-C incremental=` for
  `oxide-light`; `-C debuginfo=2` for `oxide-full-debug`;
- dependency units and build-time units: no debug information (`-C
  strip=debuginfo`) for `oxide-light`; `-C debuginfo=2` for
  `oxide-full-debug`;
- no `panic=abort` and no stripping of workspace units in either profile.

The script also refuses to run if a default `dev`/`test`/`release` profile or
the pinned `1.97.1` toolchain channel changed, so a passing selector is
evidence that the opt-in profiles were added rather than defaults replaced.
The selectors are focused profile proofs, not benchmarks and not an alias for
full QA.

## Qualification disposition: no-go (2026-10-16)

The brief's `bounded_comparison` gate is **not met**, and this task records an
explicit **no-go** for the bounded profile comparison. Independent exact-head
review recorded the violation at PR 68 review comment 6220360222
(`bounded_comparison: fail`, with a `truthful_delivery: fail` that the next
head superseded), and the follow-up review at head
`0d882443564f3384cf8bea0473386f5a77a09fc8` (review comment 6220502241)
confirmed `opt_in_profiles: pass`, `correctness_and_debug: pass`,
`truthful_delivery: pass` and `bounded_comparison: fail`, stating that a later
valid-looking sequence cannot repair the violation. The reviewer found that the
author ran four fresh sessions (2, 2, 8 and 8 Cargo driver invocations, 20 in
total) across four fresh roots, images, clocks and call budgets instead of the
brief's one original capped session, that one complete run was unmatched
(attempt 3 rebuilt zero workspace units in its light edit step), and that the
recorded session's timer starts after submission staging. The later session's
figures are therefore retained only as unqualified observations and must not be
read as the approved comparison.

The recorded session's timer starts inside the measurement recipe at run
launch, after the root, packet and recipe were staged and after Effigy
admission. Its 67.0-second total therefore excludes that staging and is not the
"original monotonic 20-minute TOTAL session including setup" recorded before
any effects that the brief requires. The attempt accounting and the diagnosed
defect of each session are retained in the
[attempt-session record](../evidence/stable-cargo-profiles-20261016/sessions.json).
A future comparison requires a fresh, separately scoped record; the existing
roots and images are closed and are not reused. Because the
one-original-session cap was exceeded and this brief authorizes no further
session, the author cannot repair the `bounded_comparison` gate by another
measurement; the disposition is a no-go, and the merge-or-close decision
belongs to the planner and operator.

No adoption, default change, consumer decision or policy change follows from
these observations. The profiles and selectors themselves remain opt-in and
are unaffected by this no-go; only the comparison claim is withdrawn.

## Unqualified observations (2026-10-16, not acceptance evidence)

One bounded session compared the default `test` profile with `oxide-light`, and
ran one explicit `oxide-full-debug` control, on two source-identical disposable
copies of the pushed implementation at the same commit with the same appended
public-constant edit. Every positive call executed the same probe test and the
same feature set. The [evidence package](../evidence/stable-cargo-profiles-20261016/summary.json)
holds the raw logs, the exact recipe and the per-unit resolved settings. These
numbers describe what happened in that session; they are not the brief's
bounded comparison and are not acceptance evidence.

| Step | Default `test` | `oxide-light` |
| --- | ---: | ---: |
| Cold test build | 14.24 s; 276.3 MiB whole target | 11.98 s; 223.3 MiB whole target |
| Identical source edit rebuild | 6.41 s | 2.81 s |
| Unchanged rerun (test executes) | 0.26 s | 0.26 s |
| `deps` + `incremental` union, cold | 260.6 MiB | 208.5 MiB |
| `deps` + `incremental` union, after edit | 346.2 MiB | 293.1 MiB |

The unchanged rerun rebuilt no workspace unit in either mode, so the 0.26 s
figures include Cargo's freshness check and the executed test. The explicit
`oxide-full-debug` control compiled in 12.49 s with a 304.2 MiB whole target
and resolved `-C debuginfo=2` for workspace, dependency, build-script and
proc-macro units; the default `test` profile resolved the same full workspace
and dependency debug information it resolved before this change.

Read as: in that one session the light profile kept the same executed tests and
unwinding behavior, rebuilt one small semantic edit about 2.3 times faster than
the default profile, and used about 53 MiB less allocated build storage in this
small workspace. Because the session was one of four and its clock started
after staging, even this light-versus-default direction is not established as
the brief's bounded comparison. This is not a benchmark, a sustained study, an
SSD-write or SSD-lifetime measurement, or a general speed claim, and it does
not measure a consumer or shared cache.

## Limits

- The comparison claim is withdrawn (no-go); the observations above are
  unqualified and one of four sessions rather than one original capped session.
- One busy-machine session on the maintainer host; the default profile ran
  before the light profile, so extracted registry and OS caches were warmer for
  the later mode.
- The edit was one appended public constant; the result does not predict
  arbitrary behavior-changing edits or large dependency graphs.
- No attributable disk writes, no self-managed cache-write reduction and no
  universal benefit for other projects are claimed.
- The measured build inputs are the implementation commit recorded in the
  evidence packet; later commits are evidence and documentation only.
- Consumer adoption, default-profile changes and host installation remain
  separate, owner-approved scopes.
