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
- `src/compiler/rust.rs:1208` `-C incremental` is `CannotCache`; Cargo passes
  it for local path units under an incremental profile, so quick-profile
  workspace units always compile in the client. `src/commands.rs:902-912` the
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
- Workspace units under the quick profile are never cached; only non-local
  dependencies can hit.

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

Compatibility: UNKNOWN. Benefit: INSUFFICIENT. Write volume: UNKNOWN.

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
