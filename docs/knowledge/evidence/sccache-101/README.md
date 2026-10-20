# sccache 0.18.0 client-side mode and a stable build-path alias (oxide#101)

Scope: can sccache complement Oxide, giving cold, recycled and new-project
slots hits on dependencies compiled elsewhere, without breaking Oxide's
writer closure or slot correctness? Evidence only: no product source change,
no installation, no live sccache server, cache or host configuration touched.
The protocol was committed before any native effect
([PROTOCOL.md](PROTOCOL.md), `6322f0f`).

@@RESULT@@

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

@@NATIVE@@
