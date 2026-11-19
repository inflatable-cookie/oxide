# Storage-efficiency findings

Current observations about storage cost in Oxide and in the consumer routes it
serves, with their limits. This file owns the findings. [Optimization
adoption](contracts/optimization-adoption.md) owns route policy, [build-context
policy](contracts/build-context-policy.md) owns the incremental-state policy,
[stable Cargo profiles](contracts/stable-cargo-profiles.md) owns the profile
recipes, and [workflow](contracts/workflow.md) owns command behaviour. None of
these findings is a measured storage saving, and none authorises a change in an
inspected project.

## Component preparation

Acowtancy's worktree setup (at `70b6d7c8`) prepares only the Rust target
component that the selected route declares. It no longer seeds all three host
targets. Composed selectors request an explicit union, and JS- or docs-only
setup prepares none. Unknown component names refuse. The route policy is in
[optimization
adoption](contracts/optimization-adoption.md#component-lazy-preparation).

The copy uses `cp -c -R` and falls back to `cp -R`. A compiler mismatch uses a
separate, toolchain-keyed target that is built only from a clean primary
checkout. Readiness is published only after the matching compiler and Cargo
build are confirmed. Synthetic fixtures showed that unrelated destinations stay
absent. No real Cargo or rustc run supports this result.

On APFS a clone shares data until one side diverges, and the plain-copy fallback
can keep a full copy. Allocated bytes, latency, write volume and SSD savings are
unmeasured.

Acowtancy's operations record also shows a failed selective clean. Cleaning
workspace packages on source changes left shared registry artifacts unusable
(`E0463`), and that cleanup was removed. Treat package-level clean as
unqualified. It is not an approved cleanup path for the Oxide pool.

Acowtancy's sccache 0.18.0 trial reported zero hits for fresh targets and for a
warm cache at a different target path. It is configuration-specific. sccache's
Rust notes require incremental compilation off and exclude linked crates, which
limits its value for incremental-on workflows.

## Profiles and debug access

Nucleus's root profile sets `dev` debug info to line tables and defines a
`full-debug` profile, and its guidance keeps the two routes separate.
Acowtancy's test profiles already drop dependency debug info and disable
incremental output.

Disabling incremental output can increase rebuild work. A smaller final target
alone does not show fewer writes. Oxide's own opt-in profiles are owned by
[stable Cargo profiles](contracts/stable-cargo-profiles.md), and the 2026-10-16
comparison that tested them was a no-go. Its unqualified numbers are not carried
here.

## Incremental and debug storage

### Supervisor-reported readings

Single busy-machine readings on Darwin 25.6 with APFS and rustc 1.97.1. The raw
logs were not kept, so these are attributed to the supervisor report and not
re-observed.

- Incremental on, unpacked split debug info: about 12,300 loose `deps` objects
  and about 3 GB of incremental state after a cold build and two changed no-run
  builds.
- Incremental off, unpacked: about 408 loose objects, flat after the warm build,
  and no incremental state.
- The packed row does not state its incremental setting, so it is not a clean
  comparison.

Limits: compile-only builds, no test execution, no file:line or debugger check
for packed mode. The 37.4 GB `deps` apparent size is not proven stale or
reclaimable, and it overlaps the incremental figure through hardlinks.

Cache sharing does not prevent per-slot growth. Shared warm reuse is a separate
quantity from bounded growth and actual writes.

### Direct incremental comparison (2026-10-15)

One Effigy heavy run timed a comment edit to a library and a test in Nucleus's
core package, on macOS with APFS, rustc and Cargo 1.97.1, and line-tables-only
debug info. Each mode had a fresh target. Incremental off ran first, so the
on-mode cold build benefited from warmer caches.

- A library comment edit took 37.0 s with incremental off and 7.8 s with it on,
  about 4.8 times faster.
- The on mode's whole target ended about 1.0 GiB larger in allocated space. Its
  loose `deps` objects grew from 1,053 to 1,941 across the two edits. Off mode
  stayed flat and kept no `incremental/` state.
- 968 inodes were hardlinked between `deps/` and `incremental/`, so directory
  totals must not be added.
- Unchanged test reruns compiled nothing in either mode, and both modes kept the
  expected backtrace frames.

The finding is a trade-off, not unbounded growth or a staleness finding.
Incremental buys a faster edit loop for more retained state. The edits were
comments, so the result does not predict behaviour-changing edits. The cold-time
difference cannot be isolated from the run order.

Recommendation: keep lifecycle-aware policy as the direction. Qualify a hard
per-slot budget and safe whole-idle-generation retirement. Do not substitute
selective `.o` deletion, and do not silently flip incremental state inside a
partition. Unchanged Cargo reuse helps in either mode. No SSD-write or lifetime
claim follows.

### Nucleus consumer adoption (2026-10-15)

Nucleus's managed agents build with incremental on. Its one-shot validation
keeps `CARGO_INCREMENTAL=0`. Its task workspace is deleted at closeout, which
bounds retention to the task's lifetime. That is not a byte bound during a long
task, and it does not reduce cold-build duplication. Oxide's size-limited owned
storage remains the longer-term bound. No shared slot, cross-task reuse or
production activation follows from this adoption.

## Disposable output and retention

Queue Spec 028 keeps Cargo build outputs task-local and shares downloaded Cargo
sources. Its close path removes only task-local Cargo targets and
`node_modules`, and only after clean, published-head, idle-thread and
no-live-run proofs. Unproved roots are retained and reported.

Nucleus Contract 002 keeps build-output retention separate from source, task
history and evidence. It retains failed or cancelled unmerged worktrees, and age
alone does not authorise removal. Current reviewed controls bind cleanup to
ownership and settlement proofs. No safe basis was found for shortening
retention or deleting older or uncertain resources.

## Internal SSD and drive observations (2026-10-20)

These are read-only observations from one day under heavy multi-agent load. They
are not a daily average, a wear figure or an Oxide attribution.

- The host wrote about 6.1 TB to the internal SSD in the 23 h 52 min since boot.
  Swap was zero.
- Live processes explain only about 191 GB of their lifetime writes. About 5.9
  TB came from processes that had already exited. Short-lived build, test,
  linker and Git processes are the likely writers, but that is not attributed.
  Most Rust builds on this host do not run through Oxide.
- Oxide's and Nucleus's pool images live on the internal SSD, so their writes
  are included. The SSD's wear indicator was not read.

A single run each on the dedicated `Builds` APFS volume (a USB SSD) and on the
internal SSD suggests the external drive is not a bottleneck for this workload
under current load. A cold Oxide build took 13.9 s on the external volume and
18.3 s internal, though the internal run went first and warmed the registry
cache. The small-file results reflect contention and encryption on the internal
drive. This is not a benchmark, and it says nothing about intrinsic device
speed.

## Evidence limits

The initial consumer assessments were read-only. They ran no builds, tests or
Cargo metadata, scanned no target trees, and measured no allocated storage.
Apparent target size is not allocated storage. A smaller retained target is not
proof of fewer SSD writes, and no SSD-life estimate follows from these findings.
Oxide's own measurements are single-host, single-sample and small-subject.
