# sccache 0.18.0 client-side mode and a stable build-path alias (oxide#101)

Scope: can sccache complement Oxide, giving cold, recycled and new-project
slots hits on dependencies compiled elsewhere, without breaking Oxide's
writer closure or slot correctness? Evidence only: no product source change,
no installation, no live sccache server, cache or host configuration touched.
The protocol was committed before any native effect
([PROTOCOL.md](PROTOCOL.md), `6322f0f`).

## Result

Safety and benefit are reported separately. Native evidence is one aborted
session; everything else is source.

| Mechanism | Safety | Basis |
| --- | --- | --- |
| Client-side mode writer closure | UNKNOWN | Source supports it, conditionally (below). Not natively exercised: session 1 ran server-side, session 2 never launched. |
| Client-side mode with `SCCACHE_LOG` set or a dist scheduler | UNSAFE | Source: client-side mode silently turns off and the server compiles (the #096 no-go path). Session 1 observed the switch. |
| Alias, slot built under a new index (no mitigation) | UNSAFE by source | Cargo documents that it does not rebuild for a changed `OUT_DIR`; fresh units keep the old alias, which then names another slot. Not natively demonstrated. |
| Alias with fixed slot-to-index affinity | UNKNOWN | Not run. By construction it never changes a slot's index. |
| Alias with invalidation on index change | UNKNOWN | Not run. An emptied slot has nothing stale to reuse. |

Benefit: INSUFFICIENT. No cross-slot hit rate or cold-slot time was
measured; the gated benefit builds never ran. Write volume: UNKNOWN.
Issue #2796 for single-level local disk: UNKNOWN (unmeasured).

Alias path preservation: the `--out-dir`, `-L` and `--extern` paths Cargo
passed to rustc kept the alias spelling (session 1, `-v` output). `OUT_DIR`
and `CARGO_*` values were not observed natively; source says they are not
canonicalized.

Upstream (read-only, 2026-10-20): 0.18.0 (2026-09-14) is the latest
release. Rust basedir normalization is unmerged: #2678, #2794 and #2918
(opened 2026-10-19) are all open. #2840 (argument basedir stripping, merged
2026-09-14) is in 0.18.0 and did not normalize Rust target paths in the
Supervisor probe.

## Pinned inputs

- sccache `v0.18.0`, commit `01c35e69b97f516c2ebfbdb418b5cd6277bde280`,
  read from the host's populated crate source cache
  (`.cargo_vcs_info.json` names that commit). The nine files #096 hashed match
  its [source-hashes.txt](../sccache-096/raw/source-hashes.txt); added here:
  `src/cache/ipc_storage.rs`
  `b5f6407df6c6d35275cb5991ac4f8df956f2036974064097e82dc9b2dbf27e25`,
  `src/cache/disk.rs`
  `c7a10401d8a2c77384e1452e6dc923a3fb3e2b45396d866b4b804def05d336af`.
- Installed `/opt/homebrew/bin/sccache` -> Cellar 0.18.0; sha256 recorded by
  each session's `start` line in the raw ledger.
- Cargo `c980f4866141969fab6254a680546a277789d6f0` (the 1.97.1 build),
  files read through the GitHub API: `src/cargo/util/context/mod.rs`
  `5414a78a…`, `src/cargo/core/compiler/custom_build.rs` `7bf93fc5…`,
  `src/cargo/core/compiler/layout.rs` `78f9fcf7…`,
  `src/cargo/core/compiler/fingerprint/mod.rs` `24fd5fc9…`
  (full hashes in [raw/source-hashes.txt](raw/source-hashes.txt)).
- Builder and fixture toolchain: Cargo/rustc 1.97.1, called directly from the
  rustup toolchain directory.

## Source trace: client-side mode (sccache 0.18.0)

- `src/commands.rs:893-944` `Command::Compile`: the client connects to, or
  starts, the server (`connect_or_start_server`), then, when
  `config.client_side_mode`, runs `do_compile_client_side` on a two-worker
  runtime in its own process.
- `src/commands.rs:662-714` `do_compile_client_side` builds an in-process
  `SccacheService` whose storage is `IpcStorage` and awaits
  `compile_direct`; only `RecordStats` is sent after the result.
- `src/server.rs:1184-1198` `compile_direct` awaits the compile body.
  `src/server.rs:1422-1666` `start_compile_task` awaits the cache write before
  returning. In client-side mode both run inside the client process: hashing
  (including rustc's `--emit dep-info` pre-pass into a `tempfile` directory,
  `src/compiler/rust.rs:241-268`), the compiler child
  (`kill_on_drop`, `src/mock_command.rs:285`), and hit restoration.
- Hit restoration (`src/cache/cache_io.rs:132-197`) writes each output to a
  `NamedTempFile` in the output directory and renames it into place; when the
  temp file cannot be created it writes the final path directly. This runs on
  the client's blocking pool. Every slot write of a wrapped compile therefore
  comes from the client process or its compiler child.
- `src/cache/ipc_storage.rs:41-135` the client's storage forwards
  `StorageGetPath`, `StorageGetRaw` and `StoragePutRaw` over the connection.
  For a disk cache `get_path` returns the cache file path, and the client
  opens and reads it itself (`:70-80`); puts send the bytes to the server.
- `src/server.rs:904-941` the server answers those requests from its own
  storage. In client-side mode it receives no `Compile` request, so it spawns
  no compiler and writes no compile output. It still writes its cache
  directory (`lru_disk_cache` temp file and rename), its socket and, if
  configured, its log.
- Server spawn: unchanged from #096. `run_server_process`
  (`src/commands.rs:96-135`) re-executes sccache with
  `SCCACHE_START_SERVER=1`, removes `SCCACHE_CLIENT_SIDE` from its
  environment and creates a startup-notify `tempdir` under `TMPDIR`;
  `daemonize` (`src/util.rs:948-957`) starts a new session unless
  `SCCACHE_NO_DAEMON=1`. The first client to find no server starts it
  (`src/commands.rs:315-345`).
- Server failure: the client holds one connection for the whole compile.
  A failed put is counted as a cache write error, not a compile failure
  (`src/server.rs:1646-1663`).
- Hazard by source: the endpoint is shared by every client that names it. A
  client without client-side mode on the same endpoint sends `Compile`, and
  the server then compiles into that client's slot from outside its group,
  as in #096. Client-side closure holds only if every client of the endpoint
  runs client-side.
- Mode switch: `src/config.rs:1279` reads `SCCACHE_CLIENT_SIDE`
  (`true`/`on`/`1`); `src/config.rs:1412-1419` then silently turns client-side
  mode off when `SCCACHE_LOG` is set or a dist scheduler is configured. The
  client then sends `Compile` to the server, the server-side path #096
  rejected. Native session 1 hit exactly this (below).

## Source trace: the alias through Cargo 1.97.1

- `src/cargo/util/context/mod.rs:696-760`: `CARGO_TARGET_DIR` is
  `cwd.join(value)` and `build.build-dir` (here from `CARGO_BUILD_BUILD_DIR`)
  is the configured path with template variables replaced. Neither is
  canonicalized; only the `{workspace-path-hash}` template canonicalizes the
  manifest path. So `--out-dir`, `-L`, `--extern` and `OUT_DIR` are built
  from the alias path as given.
- `src/cargo/core/compiler/fingerprint/dep_info.rs:291-339`
  `translate_dep_info` canonicalizes the build root and stores dep-info paths
  relative to it, and drops from rebuild tracking every env var Cargo sets for
  the command: `OUT_DIR` and build-script `rustc-env` values. Its comment:
  "if you write a binary that does `println!("{}", env!("OUT_DIR"))` we won't
  recompile that if you move the target directory."
- `src/cargo/core/compiler/custom_build.rs:679,1383-1399,946-949`: Cargo
  records the build script's `OUT_DIR` in `root-output` and, when replaying
  a fresh script's output from a moved directory, string-replaces the old
  `OUT_DIR` with the new one in every directive value. The replay corrects
  link-search and env values for the next compile, but a unit judged fresh is
  not recompiled, so absolute paths already compiled into it remain.
- Inference from those three: a slot built under alias index *i* and later
  built under index *j* reuses fresh units whose compiled code embeds
  `/alias/i/...`, which by then names another slot.

## Native session 1 (aborted, consumed)

Selector `effigy rust:sccache-client-alias` at `6322f0f` (script sha256
`42bdeaf6809a7baec9bb71606209f087d6f74b64e9bc2021f1e0c5caaa65d408`). Effigy
start 12:50:37 UTC; the harness aborted at 6.17 s and exited 1
([ledger](raw/session1/ledger.jsonl),
[Effigy output](raw/session1/effigy-output.log),
[Cargo log](raw/session1/C1.err),
[server log](raw/session1/server-1.err)).

- Tool hashes in the `start` record: sccache Cellar 0.18.0
  `bd984e18…de6e` (same as #096), cargo 1.97.1 `7672ead3…bbf5`, rustc 1.97.1
  `210df679…a4da`.
- Isolation: root `/private/tmp/oxide-sc101.b1mlro12`, no ancestor Cargo
  config or manifest, private Cargo home with only the vendored source and
  `net.offline`. This script version vendored only `cfg-if` 1.0.5 and
  `memchr` 2.8.3 (checksums matched, 108,743 archive bytes).
- Private server generation 1, PID/PGID 42021, in its own group, ready in
  11 ms.
- C1: Cargo finished `blk` in 2.48 s, rc 0, without reaching the hold. The
  server log shows `handle_client: compile` for every unit and the server
  compiling `user` ("Compiling locally", stored in cache). The clients ran
  server-side because the harness set `SCCACHE_LOG` on them (source above).
  Why the argv-gated hold did not fire in the server-run rustc was not
  traced.
- The `-v` command lines show `--out-dir`, `-L dependency=` and `--extern`
  under `/private/tmp/oxide-sc101.b1mlro12/alias/0/oxide-quick/deps`: no
  canonicalization to the slot path.
- Teardown: 2 of 2 owned groups closed, no unknown state, no sccache
  process left that was not present at start, 1 Cargo launch, owned peak
  2,523,136 bytes, minimum host free space 717,813,780,480 bytes over 24
  samples. The private server took the lingering path in teardown (members
  empty, group still answering signal 0) and closed after SIGKILL; a likely
  unreaped zombie, since the harness had not waited on it. Scratch retained.

## Amendment and native session 2 (never launched, consumed)

The amendment ([PROTOCOL.md](PROTOCOL.md#amendment-after-native-session-1-recorded-before-session-2),
`19398f6`) removed client `SCCACHE_LOG`, counted hits from the server's
storage requests, held the second macro expansion with a `ps` check, and
gated the benefit pairs on the safety results. The final apparatus is
retained at [apparatus/](apparatus/) (script blob
`470acc10915e7ded13ae4fc2551f36ee6281212c`).

Submitted 12:54 UTC; Effigy reported `capacity_timeout` at about 13:24:
"heavy run 9d24715e… was never launched"
([output](raw/session2/effigy-output.log), repeated wait line kept once).
No fixture, Cargo, server or scratch was created. That was the author's
second and last native submission.

## Not established

- Client-side closure natively: mid-compile and mid-hit kills, server death
  mid-operation, and whether any slot write follows group closure.
- The B-matrix: same-index and cross-index hits, cross-project hits, the
  `CARGO_TARGET_DIR` hash effect, the index-change hazard, both mitigations
  and their costs, and debug-info path counts.
- Benefit on this repository, cache bytes and any write counter.
- #2796 on single-level local disk. The issue report is a multi-level
  (disk + WebDAV) cache whose local level stays empty in client-side mode.
  By source, a single-level disk put goes `StoragePutRaw` -> server
  storage, but that was not measured.

## Inferences recorded for a future attempt

- Client-side closure needs every client on the endpoint to run client-side:
  a private per-owner endpoint and a scrubbed environment (no `SCCACHE_LOG`,
  no dist settings). Otherwise the server compiles into slots from outside
  the group.
- The server stays outside the group in client-side mode but writes only its
  cache directory, socket and log; its lifetime and cache integrity after a
  killed put (temp file then rename, `lru_disk_cache/mod.rs:364`) are cache
  questions, not slot closure.
- Oxide passes `CARGO_TARGET_DIR` (the checkout) in the environment and
  sccache hashes `CARGO_*`; under an alias, cross-project dependency hits
  likely still miss. Hypothesis, unmeasured.
- Rust basedir normalization, if released, would let different slot paths
  share keys without an alias. Restoring an object compiled under another
  slot's path would then carry that path (for example an embedded `OUT_DIR`);
  the same hazard class as the alias index change. Inference.

## Bounded delivery

- Author clock 12:21:24 UTC start (role admission at or before that), hard
  13:51:24.
- Setup allowance: 2 read-only inventory commands (host crate-source
  listing; the 22 locked `.crate` checksums and toolchain directories), no
  executable run, zero Cargo.
- Native submissions: 2 of 2 (one aborted harness run, one capacity
  timeout). No other compiler, Cargo or sccache execution.
- Source reading: sccache from the host crate cache; Cargo files and
  upstream PR/issue/release status through `gh` read-only.
- Delivered diff: knowledge and evidence only. The selector and script were
  removed from `effigy.toml` and `scripts/` and kept here.
