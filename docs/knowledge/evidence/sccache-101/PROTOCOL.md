# oxide#101 protocol (recorded before native effects)

Recorded 2026-10-20 ~12:35 UTC, before any native session.

## Apparatus

- Selector `effigy rust:sccache-client-alias` (safety) and
  `effigy rust:sccache-client-alias:benefit` (benefit), both running
  `scripts/probe-sccache-client-alias.py`. Temporary; removed from the
  delivered diff, retained under this directory.
- Plain Cargo 1.97.1 (`~/.rustup/toolchains/1.97.1-aarch64-apple-darwin`,
  called directly, no rustup proxy) stands in for `oxide`. It gets Oxide's
  quick recipe environment (`--profile oxide-quick`,
  `CARGO_PROFILE_OXIDE_QUICK_INHERITS=dev`,
  `CARGO_PROFILE_OXIDE_QUICK_DEBUG=line-tables-only`) and Oxide's two
  directory variables: `CARGO_BUILD_BUILD_DIR` = the alias, `CARGO_TARGET_DIR`
  = the checkout's `target`. No Oxide binary, pool, image or lease runs.
- A slot is a directory `slots/<name>` holding a `SLOT_ID` file. An alias is a
  symlink `alias/<index>` -> slot, replaced atomically.
- sccache: `/opt/homebrew/bin/sccache` 0.18.0, client-side mode
  (`SCCACHE_CLIENT_SIDE=1`) for every wrapped Cargo.

## Isolation

- One fresh canonical root `mkdtemp(/private/tmp/oxide-sc101.*)`. Every
  project, slot, alias, private `HOME`, `CARGO_HOME`, sccache cache, config,
  socket (`<root>/s`) and log lives under it. `TMPDIR`, `TMP`, `TEMP` and
  `OXIDE_TASK_SCRATCH_ROOT` are `<root>/tmp`.
- Child environments are built from scratch (no inherited variables), so no
  inherited `RUSTC_WRAPPER`, `SCCACHE_*`, credential or cloud variable
  reaches a child. `SCCACHE_CONF` is an empty private file.
- Before Cargo runs, the harness checks that no ancestor of `<root>/proj`
  has `.cargo/config`, `.cargo/config.toml` or `Cargo.toml`, and aborts
  otherwise. Cargo reads ancestors of its working directory and then
  `$CARGO_HOME/config.toml`, which is private and only replaces crates.io
  with a vendored directory and sets `net.offline`.
- Inputs: named `.crate` archives from the host's populated cache, sha256
  checked against Oxide's `Cargo.lock` before extraction (safety: `cfg-if`
  1.0.5 and `memchr` 2.8.3; benefit: all 22 locked crates). No download.
- The live sccache server, its cache and `~/.cargo/config.toml` are never
  contacted or read. `ps` listings are read-only; pre-existing sccache PIDs
  are recorded and never signalled.

## Owned identities

- Every child is started in a new session; its PID is its process group.
- The private server is started by the harness in the foreground
  (`SCCACHE_START_SERVER=1`, `SCCACHE_NO_DAEMON=1`, idle timeout 0), in its
  own group, one recorded generation at a time.
- Clients also get `SCCACHE_NO_DAEMON=1`, so a server a client might start
  stays inside that build's group, where group closure covers it.
- Closure of a group means `killpg(pgid, 0)` reports ESRCH. Teardown
  SIGKILLs and settles every known group independently. A group that does
  not close is recorded with its members, its bytes retained, and the run
  fails.

## Matrix (safety session)

| Case | What | Handshake |
| --- | --- | --- |
| C1 | SIGKILL build group mid-compile: `user` (cacheable, non-incremental) is a miss and its rustc is held inside a proc-macro | macro holds only when rustc's own argv has `--emit=…link` (not sccache's `--emit dep-info` hashing pre-pass, `rust.rs:241-268`), writes `entered` with rustc's PID and argv, then waits for `release` (60 s cap) |
| C2 | SIGKILL the private server while the client's rustc is held, then release | same |
| C3 | SIGKILL build group mid-hit: `memchr` hit restored into a fresh slot | read-only `deps/` forces sccache's direct-write fallback; the rlib path is a FIFO; first byte readable -> SIGSTOP group -> drain -> SIGKILL. Mid-write iff writer not closed and drained < rlib size |
| B1 | p1, index 0 -> sA | cold |
| B2 | p1, index 0 -> sB | cold slot, same index |
| B3 | p1, index 1 -> sC | cold slot, other index |
| B4 | p2, index 0 -> sD | other project, same index |
| B4b/c | p1 then p2, index 0, target dir by `--config` instead of env | isolates `CARGO_TARGET_DIR` in the hash |
| B5 | hazard: index 0 -> sB, then sA (built under index 0) built under index 1 | check `Fresh`/`Dirty`, the binary's compiled and runtime markers, recorded paths |
| B6 | affinity: sB keeps index 0 | expect fresh and correct |
| B7 | invalidation: sA emptied, built under index 1 | cost: cold slot with hits |
| B8 | no-op repeat of B7 | reference |

After each closure case: slot snapshots at hold, at closure and later, and
the server's open files (`lsof -p`) under `slots/`. Hits and misses come
from the client's debug log lines (`[crate]: Cache hit|miss in`) that Cargo
forwards on stderr.

## Benefit session (only if a model is safe)

Copy of this repository (`git archive HEAD`: manifests, lock, `src`),
index 0, one fresh slot per build, deleted after its group closes. W0
populates the cache (not a sample). Pairs in fixed order: P1 U,W; P2 W,U;
P3 U,W (U = unwrapped, W = wrapped warm cache, server start and stop inside
the timed window). Quick recipe unchanged; workspace units stay incremental.
Recorded per build: time, hit/miss by crate, CannotCache reasons, slot bytes,
cache allocated bytes and file count, `ru_oublock` of reaped children
(operations, not bytes). No threshold; median paired difference reported.

## Bounds

300 s monotonic per session from harness start through teardown, new work
stops at 255 s; 1 GiB owned bytes; 96 Cargo launches; 8 MiB inputs; 32 MiB
logs; host free space >= 20 GiB sampled every 250 ms. Breach -> SIGKILL
every owned group and report.
