# Project adoption boundary

This contract records the boundary between Oxide's storage facility and
consumer-owned integration. Oxide owns the storage mechanisms. Queue, Nucleus
and other consumers own task admission, waiting, scheduling, host execution,
settlement and their own lease lifecycle. [Workflow](workflow.md) owns the
explicit `oxide` command surface, and [local lease](local-lease.md) owns the
lease library's semantics, capacity and eviction rules.

Consumer adoption is separate work. It needs its own owner brief and exact
workflow qualification. No production Queue or Nucleus binding, consumer
configuration key, service deployment or installation follows from this
document. For project-owned component-lazy preparation, see the [optimization
adoption policy](optimization-adoption.md), which needs no shared slot or
consumer lifecycle integration.

## Recommendation

Tom's 2026-10-18 direction selects explicit `oxide` commands as the developer
front door ([vision](../vision.md#product-workflow-direction)). The
consumer-owned adapter route was retired with the protected route on the same
day. Its lasting rules still apply:

- Never point shared Cargo configuration or `CARGO_TARGET_DIR` at an Oxide slot.
  Relocate intermediates per invocation through `build.build-dir`, and keep
  final outputs, test output and cleanup scope private.
- Oxide adds no global shell, `PATH`, Cargo, editor or Effigy interception.
- Build reuse never skips the requested tests, and a cache hit is not validation
  success.
- An invocation that cannot establish its storage or inputs falls back to a
  private build or refuses before Cargo. Neither claims reuse. A private cold
  fallback must omit every acquired shared path.

## Stable source-path lifecycle

The retired design proposed a finite consumer-owned pool of physical
task-worktree paths, reused after archive under strict physical identity. It was
never implemented as a consumer API. Queue Spec 028 keeps build outputs
task-local and shares only Cargo downloads and Git objects. Nucleus Contract 002
keeps distinct task workspaces and branches. Cross-path freshness and
path-embedding limits are in [qualification
findings](../qualification-readiness.md#cargo-freshness).

## Readiness, admission and acquire ordering

Queue and Nucleus keep task admission, waiting, scheduling, host execution and
settlement authority. Any consumer adapter must preserve those owners'
lifecycles, and it may acquire a lease only under a separately approved
integration. Waiting must not reserve storage. A busy race is the consumer's to
recover, normally by private cold fallback. Oxide provides no readiness service,
host scheduler, project registration or consumer credential.

## Current Nucleus integration boundary

- Nucleus planning and development belong to its own Supervisor, under Tom's
  2026-10-15 ruling. Send substantive coordination there, and do not prompt
  active workers.
- On 2026-10-16 Tom replaced the earlier signed-attempt and callback integration
  with a same-user local CLI. That CLI was retired from this repository in
  oxide#102 on 2026-10-20. The library remains, and Nucleus may break
  temporarily. Its designed refusal path is a cold private build.
- Owner-reported history from 2026-10-16 to 2026-10-20: Nucleus leased slots on
  a live pool through a parent Cargo config, fixed a first-use payload metadata
  defect in Oxide, and gated release on the closeout retirement receipt. It then
  ran one ordinary task. These are owner reports. Oxide did not rerun the
  consumer workload or inspect the pool. They show placement-time reuse for one
  task. They do not show per-build speedup, a production size distribution,
  SSD-write savings or cancellation recovery.
- Pre-merge cancellation leaves the slot occupancy held. Later same-project
  acquisitions are busy and fall back to a private cold build. Recovery is an
  explicit owner `recover` after all writers have ended. Idle agents, PIDs,
  elapsed time and a logged command do not establish closure. Nucleus owns any
  cancellation-retirement correction.
- Pool sizing is an operator or consumer budget choice. A build can exceed the
  remaining payload during a lease, and there is no automatic mid-build
  fallback. See [local lease capacity](local-lease.md#capacity-and-eviction).

## Focused cross-worktree result

The 2026-10-16 result used one shared `dev` slot and Nucleus's planning and
diagnostics workflow. A cold checkout took about 48 s and rebuilt 73 units. A
different-path checkout took about 18 s and rebuilt only its changed planning
test, with the SQLite system crate reused. Selected tests ran every time. The
unchanged rerun rebuilt nothing. Four Cargo calls were used. The union footprint
was about 3.5 GB.

Limits: footprint was sampled at run boundaries, not continuously. The old,
equal-mtime include files accompanied fresh Rust test edits, so input-only
timestamp invalidation was not isolated. No native `.a` hash, slot symlink
census, full-debug live run or live eviction result was captured. The result
supports consumer integration for the observed ordinary-Cargo workflow. It is
not a general source-path, native-write, SSD-lifetime or production claim.

## Environment projection prerequisite

Effigy's default forwarded runtime environment includes `CARGO_TARGET_DIR` and
`CARGO_INCREMENTAL`. It excludes `CARGO_BUILD_BUILD_DIR`, `TMPDIR`, `TMP` and
`TEMP`, as read from Effigy `b8a6409a`. Explicit selected-task environment
references can carry those values, but Nucleus's core test selector declared
none. Setting only the outer process environment is therefore insufficient.

Any consumer scope must establish the exact admitted projection through the
existing execution surface, and bind the final values before submission. Do not
add Rust cache policy to Effigy or assume global forwarding. Synthetic callback
tests do not prove the projection. This is a retained prerequisite, not
authority to change Effigy or Nucleus.

## Historical protected-route references

Nucleus removed its never-live disabled Oxide consumer code in its 2026-10-15
simplification reset. Oxide retired its own protected route, workflow entry
point and runner bridge on 2026-10-18. Do not restore them as an activation
shortcut. A future consumer integration starts from the reviewed handover and
the current owners.
