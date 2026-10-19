# Slot compiler identity in the caller's context (oxide#094)

Scope: the `rustc -vV` query in the slot compatibility key. It now runs in
the command's inherited working directory and environment, where the guarded
Cargo runs, instead of the canonical workspace root. Workspace ownership and
repository identity are unchanged. No cache, skip path, durability or pool
change; nothing from the #092 donor branch. No claim about real projects,
other toolchains or Linux.

## Result

`effigy rust:oxide-toolchain-context`, run 2: pass, 11 s from script start
including staging, detach and scratch removal. Source `cf893da`; the final
head differs only by `rustfmt` line wrapping in two unit tests and these
documents. Builder `cargo 1.97.1 (c980f4866 2026-06-30)` / `rustc 1.97.1
(8bab26f4f 2026-07-14)`, staged offline from the host Cargo home.

Fixture: a std-only virtual workspace, root `rust-toolchain.toml` 1.97.1,
`member/rust-toolchain.toml` 1.96.1, both installed already. Each member's
build script reports `$CARGO` and `$RUSTC -vV`. The script recomputes the
quick-profile key from the build script's `rustc -vV` and the record's
identity, and compares it with the slot's recorded compatibility.

| Context | Cargo used | rustc used | Slot |
| --- | --- | --- | --- |
| root | 1.97.1 | 1.97.1 | 1 cold, key `96c8d1d3…` |
| root repeat | 1.97.1 | 1.97.1 | 1 warm, every unit fresh |
| `member/` | 1.96.1 | 1.96.1 | 2 cold, key `784e95ac…` |
| `member/` repeat | 1.96.1 | 1.96.1 | 2 warm, every unit fresh |
| root, `--manifest-path member/Cargo.toml` | 1.97.1 | 1.97.1 | 1 warm, every unit fresh |
| `member/`, `--manifest-path ../app/Cargo.toml` | 1.96.1 | 1.96.1 | 2 warm |
| root, `RUSTC=<1.96.1 rustc>` | 1.97.1 | 1.96.1 | 2 warm |
| root, `RUSTUP_TOOLCHAIN=1.96.1` | 1.96.1 | 1.96.1 | 2 warm |
| `member/`, `oxide +1.97.1 check` | none | none | refused, state unchanged |

Every key matched the recomputed digest of the compiler the build used. Both
slots record checkout `<scratch>/workspace` and identity
`sha256("path:<scratch>/workspace")`. By source, the previous query ran at the
root, so the member context would have digested 1.97.1 and been offered
slot 1; that was not rerun.

Bounds: 20 Cargo processes (setup `cargo -V` 1, staging 1, fixture 18, from
the wrapper log), cap 32. Setup requests 3 (`git rev-parse HEAD`, builder
`cargo -V`, `rustc -V`). One 128 MiB APFS pool, detached before removal.
Owned peak 88 MiB of 512 MiB; fixture inputs 1444 bytes; output 29755 bytes.
Host free minimum 774270 MiB against the 20 GiB reserve. `RUSTUP_AUTO_INSTALL=0`
and a private empty `CARGO_HOME` in fixture subprocesses; no download.

Run 1 failed in the harness, not the product: it looked up the state record
by the summary's slot number, which is the library slot, not Oxide's record
`id` (`raw/proof-1.log`). The root build had already passed: its build
scripts reported 1.97.1, and the recorded key `65f921e6…` equals the
recomputed digest, checked offline from the raw output. Its pool was
detached by hand and its scratch removed. The script now pairs each summary
slot with the latest-used record, one to one.

## Not covered

`build.rustc`, `build.rustc-wrapper`, `CARGO_BUILD_RUSTC`, `RUSTC_WRAPPER`,
`RUSTC_WORKSPACE_WRAPPER`, and a `PATH` mixing installations stay outside
the key; see [workflow](../../contracts/workflow.md#cargo-version-matrix).
