# sccache 0.18.0 sharing against Oxide slot writer closure (oxide#096)

Scope: whether a cache shared across builds can keep Oxide's exclusive-slot
release and recovery semantics, and what it costs on a tiny fixture. Evidence
only: no production Rust change, no auto-detection or enabling, no wrapper
chaining, no installation. The user's and Effigy's machine-wide sccache
server, cache and config were never started, stopped, queried or read.

## Pinned inputs

- sccache tag `v0.18.0`, annotated tag object `e969b1c00e64`, commit
  `01c35e69b97f516c2ebfbdb418b5cd6277bde280`. Source tarball sha256
  `9891ebff5e0def98f7fe40653b59f00a7d345160fe240f54a2d26009f9955586`.
  File hashes in [raw/source-hashes.txt](raw/source-hashes.txt).
- Installed tool: `/opt/homebrew/bin/sccache` →
  `/opt/homebrew/Cellar/sccache/0.18.0/bin/sccache`, sha256
  `bd984e1857b9587f3574ff2d13cd59acf2d92982d4684e0e40bcbbc0bbf1de6e`,
  `sccache 0.18.0`. Setup allowance used: 3 commands, 1 s, zero Cargo
  ([raw/setup.log](raw/setup.log)); `--version` ran with private HOME,
  config, cache and socket.
- Builder `cargo 1.97.1` / `rustc 1.97.1`, the package's declared
  `rust-version`.

## Source trace (pinned commit; documentation, source and native kept apart)

Server lifecycle:

- `src/commands.rs:96-127` `run_server_process`: the client re-executes its
  own binary with `SCCACHE_START_SERVER=1`; no process-group change.
- `src/commands.rs:761-776` the started process calls `daemonize`, then
  `server::start_server`. `src/util.rs:948-957`: `daemonize` calls
  `daemonix::Daemonize::start` (fork and new session) unless
  `SCCACHE_NO_DAEMON=1`. `src/util.rs:903-930` closes inherited descriptors
  above stderr either way.
- A full-tree search of the tarball finds no `setsid`, `setpgid`, `setpgrp`
  or `process_group` call; `daemonix` (`src/util.rs:956`) is the only session
  change. Compilers are spawned with `kill_on_drop(true)` and no group change
  (`src/mock_command.rs:264-292`).
- `src/server.rs:78,99-105` idle shutdown after `SCCACHE_IDLE_TIMEOUT`
  seconds, default 600, 0 disables. `src/server.rs:552` runs with a never-ready
  shutdown future: no signal handler, so SIGINT/SIGTERM use the default
  action. `src/server.rs:704-752` after an idle or `--stop-server` shutdown the
  server waits at most 10 s for live connections, then exits.
- `src/commands.rs:57-66` `SCCACHE_SERVER_UDS` selects a Unix socket
  endpoint; otherwise TCP port 4226 or `SCCACHE_SERVER_PORT`.

Per-request behaviour:

- `src/server.rs:1098-1131` each connection's request task is an
  `AbortOnDropHandle`: a client disconnect aborts the compile future.
  Aborting drops the spawned compiler, whose `kill_on_drop` sends SIGKILL to
  that one PID only; descendants (for example a linker) are not signalled.
- `src/compiler/compiler.rs:655-656` and `src/cache/cache_io.rs:132-140` a
  cache hit restores outputs into Cargo's output directory from the server's
  blocking pool (`spawn_blocking`). Tokio documents that an aborted task does
  not stop a blocking closure already running; that library behaviour is
  cited, not traced here.
- `src/server.rs:1642-1666` the cache write is awaited before
  `CompileFinished` is returned: on normal completion the server finishes its
  cache write before the client returns to Cargo.
- `src/commands.rs:571-616` an unhandled or non-cacheable request, or a
  server that disconnects before finishing, makes the client run `rustc`
  itself, inside Cargo's group.
- `src/commands.rs:657-716,918` an opt-in client-side mode
  (`SCCACHE_CLIENT_SIDE`) runs the compile in the client and forwards only
  cache I/O to the server. It is outside this brief's candidate list and was
  not assessed.

Rust caching:

- Documentation (`docs/Rust.md`) says `link` must be among `--emit`
  values. Source differs: `src/compiler/rust.rs:1287` accepts `metadata`
  without `link` and `:1625-1636` handles metadata-only output, so `check`
  units are cacheable by source.
- `src/compiler/rust.rs:1208` `-C incremental` is `CannotCache`. The
  earlier inference here, that quick-profile workspace units therefore always
  compile in the client, is contradicted by native proof 2: under the
  wrapper, the `app` workspace unit was hashed, compiled by the server and
  stored. That unit evidently received no `-C incremental`; the Cargo-side
  cause was not traced, and incremental output is therefore not shown to be
  unchanged under the wrapper. `src/commands.rs:902-912` the
  client exits 1 if `CARGO_INCREMENTAL=1` or `CARGO_BUILD_INCREMENTAL=1` is in
  its environment; Oxide sets neither (no match in `src/`).
- Hash key (`src/compiler/rust.rs:1503-1596`): arguments except `-L`,
  `--extern`, `--out-dir`, `--check-cfg`, `--diagnostic-width`; inputs and
  extern rlib contents; `CARGO_*` environment except `CARGO_MAKEFLAGS`,
  `CARGO_REGISTRIES_*`, `CARGO_BUILD_JOBS`, `CARGO_ENCODED_RUSTFLAGS`; and
  the compile's working directory. Target and slot paths are excluded, but
  the working directory and `CARGO_MANIFEST_DIR` are included. For a
  registry dependency both live under `CARGO_HOME`, so they agree across
  checkouts that share one Cargo home; for local packages they are the
  checkout path. This is inferred from source, not traced in Cargo.
- `docs/Local.md`: one server per local disk cache; concurrent servers race.

## Candidate assessment (before native effects)

(a) Shared daemon, per-request completion. NO-GO by source. The daemon is in
its own session, outside every Oxide Cargo group. Normal completion is
ordered (cache write, then response), but a cancelled or crashed command
only aborts the request future: the direct compiler gets SIGKILL with no
reap or descendant signal, and a running cache-hit restoration continues.
Nothing tells the disconnected client or Oxide when those writers end, so
Oxide cannot attribute closure before release or recovery. Idle statistics,
the server PID or Cargo's exit are not closure.

Quarantine (architect e127de94). NO-GO for the shared daemon. The only
source-supported descendant model is the server's own process group
(sessions are created only by `daemonix`; nothing below the server changes
group). For a shared daemon that group ends only when the daemon exits, at
600 s idle by default or on an explicit stop, and any other build keeps it
alive. Closure of the server generation that wrote into a quarantined slot is
therefore unbounded while builds continue: unbounded quarantine is no-go.
The exact generation is identifiable (PID plus start time), but its exit is
not the closure of its orphaned compiler children unless its group is empty.

(b) Serialized foreground server per lease over one owned persistent cache.
SOURCE-SUPPORTED, to be proved natively. With `SCCACHE_NO_DAEMON=1` the
server stays in the group of whoever started it; nothing below it changes
group. A launcher run as Cargo's group leader puts the server, every
server-spawned compiler and every restoration thread inside Oxide's Cargo
group. Oxide's shipped closure (`killpg(group, 0)` reporting ESRCH before
normal release or crash recovery) then covers all server-side writers without
any sccache-specific signal. Costs and limits, stated up front:

- One server per cache means one wrapped lease at a time per cache. A
  concurrent lease must wait, use a private cache (no sharing) or run
  unwrapped. The cache lock must outlive the server even when the launcher or
  Oxide dies; sccache provides no such lock, and this harness does not model
  one. Concurrency is therefore UNRUN and its arrangement UNKNOWN.
- The server's lifetime extends the slot hold: a killed launcher leaves a live
  server holding the slot until its idle timeout.
- (Corrected after proof 2.) Quick-profile workspace units were not refused
  as incremental under the wrapper; see Result.

## Protocol for the native proof (recorded before effects)

Selector `effigy rust:sccache-compatibility`
([script](../../../../scripts/test-sccache-compatibility.py)). Admission clock
starts before staging; the 20 GiB host reserve is sampled at admission and
every 250 ms; 300 s, 512 MiB owned, 8 MiB fixture source, 32 MiB retained
output, one 128 MiB APFS pool, 64 MiB cache, 96 Cargo driver processes.

1. Staging: clean checkout required; Cargo.lock's 22 cached archives (checksums
   verified) and index entries copied into a private `CARGO_HOME`; offline
   `cargo build --locked` with 1.97.1. The checkout's ancestors include the
   machine-wide `~/.cargo/config.toml`, so staging sets `RUSTC_WRAPPER=""`
   and points every sccache variable at an empty private directory, which must
   still be empty afterwards. Receipt: commit, tree, toolchain, binary hashes.
2. Fixture: `app` depending on `cfg-if =1.0.0` (seeded by copy), private HOME
   and Cargo home, all `SCCACHE_*` and wrapper variables removed (names
   logged, never values), private empty `SCCACHE_CONF`, private
   `SCCACHE_DIR` and per-launch socket.
3. A task-owned `cargo` launcher, found first on PATH by the unchanged staged
   `oxide`, starts `sccache --start-server` with `SCCACHE_NO_DAEMON=1`,
   records its process group members (PID, group, start time), runs the real
   Cargo with `RUSTC_WRAPPER`, records stats and stops the server. Oxide's
   metadata calls and unwrapped controls go straight to Cargo.
4. Scenarios: S1 miss; S2 a second checkout path and cold slot (dependency
   hit expected); S3 workspace source change; S4 the same fixture without
   wrapper (slot key comparison); S5 Ctrl-C at Oxide with a live server in
   the group; S6 the launcher (Cargo's group leader) SIGKILLed with the
   server alive (slot must stay held until the group is empty; idle 12 s);
   S7 Oxide SIGKILLed with the server alive (held, then recovered by the next
   command only after the group is empty). After every command each recorded
   server PID must be ESRCH and every slot idle; a live server after Oxide
   returned is a decisive NO-GO that stops the proof with resources held.
5. Cost: three interleaved pairs of new cold slots, unwrapped versus wrapped
   on the warm shared cache, alternating order, server start and stop inside
   the wrapped timing. All samples retained; median paired difference
   reported; no threshold. Cache apparent and allocated bytes and file count
   reported separately.

Not modelled, so withheld: an in-flight compile or cache-hit restoration at
the moment of cancellation (no deterministic hold point in a server-side
compile within scope), a changed registry dependency (one cached version),
concurrent leases on one cache, existing wrapper preservation, and write
bytes. Bytes written need an attributable counter over the whole owned
process set; the server is reparented away from the launcher, so
`rusage`-style child totals miss it, and no privileged tracing is allowed.
Write volume is UNKNOWN.

## Result

Compatibility: UNKNOWN. Benefit: NO-BENEFIT observed for cross-slot sharing on
this fixture, with the cause inferred; INSUFFICIENT overall. Write volume:
UNKNOWN. The native proof-2 record follows the proof-1 history below.

- Shared daemon (a) and quarantine: NO-GO by source, above. No native effect
  was needed or taken.
- Foreground server per lease (b): source-supported only. Native proof 1 was
  submitted at 10:05 UTC through `effigy rust:sccache-compatibility`, waited
  at "waiting for QA capacity, position 1" and ended at 10:35 UTC with exit 1:
  `capacity_timeout: heavy run 826638d6… was never launched because capacity
  did not free in time; this is a capacity timeout, not a validation failure`
  ([raw/proof1-admission.log](raw/proof1-admission.log); the repeated wait
  line is kept once and colour codes are stripped, nothing else changed). The
  harness never started: no staging, pool, server or Cargo process, and no
  owned resources exist. The submission counts as one of the two author
  proofs. No second submission: 18 minutes of the author clock remained, the
  brief allows a second attempt only for a harness or setup correction, and
  another admission wait of that length would overrun the clock.
- Unrun, so not claimed: every native scenario S1–S7, the three cost pairs,
  cache bytes, per-unit hit/miss classes and the slot-key comparison.

Open question for the reviewer or a later bounded run: do S1–S7 hold on
real hardware (every recorded server PID ESRCH before release; the slot held
while a killed launcher's server lives; recovery only after the group
empties), and does the cross-checkout dependency hit appear? Separately, an
integration would still need a crash-safe cache lock that outlives the
server, a decision for concurrent leases, wrapper preservation and an
attributable write counter.

## Bounded delivery

- Author clock: first run start 09:53:36 UTC; work stopped near 10:40 UTC,
  inside 60 minutes including the 30-minute admission wait.
- Setup allowance: 3 commands, 1 s, zero Cargo. Proofs: 1 submitted,
  0 launched, 0 Cargo driver processes, no pool, image, server or scratch
  beyond the empty validation directory Effigy never used.
- Harness checks: `python3 -m py_compile` and an import of the module
  (which loads `test-oxide-cli.py`) passed. The harness has never run
  natively; its scenarios, assertions and cleanup are unexercised.
- `effigy docs:check`: exit 0.
- Source reads: pinned files and the tag tarball in a private
  `/private/tmp` scratch, outside the repository.

## Revision after exact-head review 6289051691

Review verdicts: closure_evidence_truthful fail, identity_and_isolation fail,
cost_evidence_truthful pass, bounded_delivery fail. Harness corrections, made
inside the original author clock with no native proof and no further
submission (the second proof slot stays unused; about 12 minutes of the
clock remained, less than the previous admission wait):

1. Launcher substitution used the bare word `SCCACHE`, rewriting every
   `SCCACHE_*` name and the wrapped/unwrapped guard. Placeholders are now
   `@SCCACHE@`, `@REAL_CARGO@` and `@LOG@`; the harness refuses a rendered
   launcher that keeps a placeholder, contains `/sccache_`, or lacks
   `SCCACHE_NO_DAEMON=1` or the guard. A synthetic render keeps all nine
   private `SCCACHE_*` names, and `sh -n` accepts it. Neither Cargo nor
   sccache ran.
2. Failure teardown relied on the server PID registry. It now detaches only
   after positive closure: every owned group (the staging build, every Cargo
   group leader in the launcher log, every Oxide record's process group) and
   every registered server must report ESRCH within 10 s. Anything else,
   including an unreadable state, keeps the image and scratch held, and their
   IDs are printed.
3. A reserve breach only set a flag, and aggregate size was checked at
   checkpoints only, after the staging Cargo home was removed. The monitor
   now samples the reserve every 250 ms and owned bytes every fourth sample
   (about 1 s), staging included. On either breach it SIGKILLs every owned
   group and registered server. Staging runs as its own process group, and
   owned bytes are checked before the staging Cargo home is removed.

The harness still has never run natively. Compatibility stays UNKNOWN,
benefit INSUFFICIENT and write volume UNKNOWN.

## Revision after exact-head review 6289170915

Verdicts: closure_evidence_truthful fail, identity_and_isolation pass,
cost_evidence_truthful pass, bounded_delivery fail. Two harness corrections,
again with no native proof (the author clock ended at 10:53:36 UTC):

1. The monitor thread now wraps all accounting. A failed or racing reserve or
   tree reading records a breach and SIGKILLs every owned group and server;
   it no longer ends the thread silently.
2. A staging timeout SIGKILLs the staging Cargo group, reaps the leader,
   waits up to 10 s for the group to report ESRCH, prints its state and keeps
   the scratch held before failing.

Compatibility stays UNKNOWN, benefit INSUFFICIENT and write volume UNKNOWN.
The harness has still never run natively.

## Owner correction phase (2026-10-20, review 6289278937)

Owner ruling at 10:57:58 UTC under Tom's standing routine-correction
authority: one separate phase, author until 11:42:58 UTC, three named
harness repairs, no proof replenishment. The original clock, the spent
10:05 UTC submission and reviews 6289051691, 6289170915 and 6289278937 stay
as recorded. Phase start 10:58:32 UTC; no owned resources were live (the
only `/private/tmp/oxide-validation.*` directory from this task, created at
10:05 UTC, was empty; no `sc096` process or image).

1. Clock and supervision. Effects stop at 255 s; 45 s inside the 300 s
   proof are reserved for teardown. `stage_build` checks the clock before
   creating the child; from then on any exception (clock, breach, artifact
   parse, interrupt) settles the staging group with a bounded SIGKILL, reap
   and ESRCH wait before propagating. Success also requires the whole group
   to be gone. `main` runs every effect under one `try` that settles all
   owned writers by the reserved deadline, detaches the image only after
   positive closure and otherwise prints the held identities.
2. Writer discovery. Registered children, the launcher log, Oxide's state
   records and registered servers are read independently. A malformed,
   unreadable or racing source becomes an UNKNOWN entry that keeps the
   resources held; it does not stop signals to the other known targets, and
   one failed signal does not stop the rest. A target seen ESRCH is marked
   settled and never signalled again, so a PID reused after an owned writer
   exited is not hit (the monitor scans every 250 ms; reuse within that
   window is the remaining limit). A monitor-thread failure kills the
   writers and interrupts the main thread. Owned-byte walks count a file
   deleted between listing and `lstat` as zero; any other error is a breach.
3. Temp containment. `TMPDIR`, `TMP`, `TEMP` and `OXIDE_TASK_SCRATCH_ROOT`
   are bound to `<parent>/tmp` before staging or any helper, and every child
   environment inherits them.

Regressions: `effigy rust:sccache-compatibility:synthetic`
([script](../../../../scripts/test_sccache_harness.py)), 7 cases with real
`sh`/`sleep` children in a fresh `/private/tmp` directory: clock expiry
after the staging child exists, parse failure with a live group member, an
interrupt during staging, malformed state plus an unreadable log plus an
injected signal failure (the other child and the server are still killed;
three UNKNOWN entries are reported), monitor failure (main interrupted,
writer killed), sentinel temp paths absent from staging, base and fixture
environments, and the launcher render. All 7 passed. The first run errored
before any assertion: Effigy's `python3` is 3.9.6, which has no
`Popen(process_group=)`, so the earlier native harness would also have
failed at staging. The child is now started with `start_new_session=True`.

## Native proof 2 (author's last allowed submission)

`effigy rust:sccache-compatibility` at clean head `8c62748`, submitted
11:03:16 UTC, admitted 11:04:29 UTC, exit 1 after about 35 s. Raw output:
[raw/proof2-effigy-output.log](raw/proof2-effigy-output.log) (ANSI codes
stripped, nothing else). The per-launch group listings, stats, start and stop
files are copied verbatim into [raw/proof2-launches](raw/proof2-launches).
The `*.server-extract.log` files are grep extracts of the server debug logs
(parse, hash, hit/miss, store lines), not full logs.

Staging: 22 locked archives seeded (checksums verified), offline 1.97.1, no
wrapper. The private stage sccache directory stayed empty. Staging receipt:
`oxide` sha256 `9a80e67e…`, `oxide-storage-lease` `eb318eb0…`. Owned peak
before seed removal 90 MiB; host free minimum 697,854 MiB.

| Case | Result |
| --- | --- |
| S1 miss (`w1`, slot 1 cold, 1.73 s) | `cfg_if` and `app` both hashed, missed, compiled by the server, stored; 3 requests not cacheable (`-vV`, `--print`). |
| S2 second checkout (`w2`, slot 2 cold, 1.06 s) | No hit: `cfg_if` key `091ac32e…` differs from S1's `ea034695…`. |
| S3 workspace change (slot 2 warm, 0.83 s) | `app` rebuilt (1 miss, stored); `cfg_if` Cargo-fresh. |
| S4 no wrapper (`w3`, slot 3 cold, 0.59 s) | Ran. The slot-key comparison was not reached, so it is unreported. |
| S5 Ctrl-C | Stopped by harness assertion before the signal (see below). |
| S6, S7, three cost pairs | Not run. |

Native observation, from the raw `*.group` listings: every wrapped launch's
foreground server (`/opt/homebrew/bin/sccache`, PPID 1 after
`--start-server` exited) had the launcher's process group ID, i.e. it was
inside Oxide's Cargo group (launches 86828, 87121, 87371 and 88007). Oxide
releases a slot only after `killpg(group, 0)` reports ESRCH, so for S1–S3 the
server's exit before release follows from that shipped check plus the
observed membership. That is an inference: the harness's own per-server ESRCH
assertion was vacuous.

Harness defect that stopped the run: server identity matched the canonical
Cellar path, but `ps` reports the invoked `/opt/homebrew/bin/sccache`. No
server was registered, the S1–S3 "server gone" assertions checked nothing,
and S5's "exactly one server" assertion failed. That is a harness failure,
not a writer-closure observation. After it, supervision SIGKILLed the owned
groups, found every owned target ESRCH and detached the image (exit 0). A
later read-only check found server PIDs 86844, 87125, 87374 and 88014 ESRCH.
Scratch `/private/tmp/oxide-cli.OdGPci/sc096` (staging target, fixtures,
full server logs) is retained, not deleted.

No-hit cause, inferred: Oxide sets `CARGO_BUILD_BUILD_DIR` (slot) and
`CARGO_TARGET_DIR` (checkout) for Cargo (`src/bin/oxide/main.rs:127-128`).
Cargo's environment reaches the wrapper, and sccache hashes every `CARGO_*`
variable except four (`src/compiler/rust.rs:1580-1594`). Per-slot values
would therefore change every key. Environments were not diffed.

Workspace incremental: the `app` unit was cached rather than refused as
incremental, so quick-profile incremental behaviour under the wrapper is not
shown to be unchanged. Cause untraced.

Proof accounting: author submissions 2 of 2 used (one capacity timeout, one
harness failure). Under the owner phase there is no retry and no further
repair. Cargo drivers this run: staging 1, toolchain `-V` 1, launcher log
entries for S1–S5. Remaining question: whether a corrected identity match
plus a cache key independent of slot and checkout paths can show both
closure (S5–S7) and a cross-slot hit.
