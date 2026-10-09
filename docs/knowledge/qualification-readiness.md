# Qualification readiness: containment and capacity

Design-only. This file answers the host/consumer gates that [architecture](architecture.md) left unproved: descendant containment after cancel/crash/lost reply/restart, and whether the proposed 10 GiB disposable cap and 20 GiB host free-space reserve can actually be enforced. It does not authorize a pilot, consumer adoption, or host installation. Saving or merging it grants none of those. Unmerged worker code is not an authority.

Ownership: this file. Protocol semantics stay in [architecture](architecture.md). Scope and authorization stay in [vision](vision.md). Open product choices stay in [questions](questions.md).

Inspection was read-only: committed Oxide, Nucleus and Queue documentation/config, installed toolchain metadata, Darwin headers and man pages, and `statfs`/`diskutil`/`quota` queries. No compilation, daemon, checkout, quota, volume, sandbox or service was created.

## Observed facts versus proposed control

**Observed (this inspection, 2026-10-09, native arm64 macOS 26.6.2 build 25G83):**

- Host writable volume is APFS Data (`disk3s5`). `statfs`/`df` reported about 681 GiB available; `diskutil info` reported about 732 GiB container free. User quotas for uid 501: none (`quota -v`).
- Darwin `sys/event.h` states `NOTE_TRACK`, `NOTE_TRACKERR` and `NOTE_CHILD` are no longer supported as of 10.5. `kqueue(2)` still documents `EVFILT_PROC` `NOTE_EXIT` / `NOTE_FORK` / `NOTE_EXEC` on a known PID. `sandbox-exec(1)` is deprecated.
- `posix_spawnattr_setpgroup(3)`, `setpgid(2)`, `killpg(2)` and `proc_listchildpids` are present. `killpg`/`kill(-pgid, 0)` returning `ESRCH` means the kernel currently sees no member of that group.
- `setrlimit(2)` `RLIMIT_FSIZE` caps one file's size, not a directory tree. `diskutil apfs addVolume` documents `-quota` (volume file-data maximum) and `-reserve` (space guaranteed to that volume). Creating a volume is outside this brief.
- Nucleus `main` at inspection: commit `bca75ed0143f96cd20b88ff72a421f1fb5f4ef19`, tree `67ecef91c907ca4085d94ea242b62f678c92993f`. No root `rust-toolchain.toml`; `rustup show` in that checkout reported the rustup default `stable-aarch64-apple-darwin`. Crates use edition 2024 and do not declare `rust-version`.
- Already installed: `rustc 1.97.1 (8bab26f4f 2026-07-14)` / `cargo 1.97.1 (c980f4866 2026-06-30)` at `~/.rustup/toolchains/1.97.1-aarch64-apple-darwin`, host `aarch64-apple-darwin`. Also installed: `1.96.1`. Oxide pins 1.97.1 in [`rust-toolchain.toml`](../../rust-toolchain.toml); that is Oxide's toolchain, not Nucleus's.
- Nucleus `effigy.toml` names `qa:core:test` as `cargo test --workspace --offline` and `check:core:test` as `cargo test --offline {args}`. Nucleus `crates/README.md` records a 2026-10-09 bounded comparison of `check:core:test -- -p nucleus-core --test planning --test build_diagnostics` at about 2.3 GiB unique artifact bytes (line-tables profile) on this host and rustc 1.97.1. `nucleus-core` depends on `serde` with `derive` and `rusqlite` with `bundled`. `tests/build_diagnostics.rs` asserts a source-line backtrace containing `build_diagnostics.rs:<line>`, not the absolute checkout path. `tests/packages.rs` and `tests/definition_v4.rs` use compile-time `env!("CARGO_MANIFEST_DIR")`.
- Queue/Nucleus Contract 010 (Nucleus `docs/knowledge/contracts/010-host-run-scheduling.md`, Queue `server/process-group.ts`) launches a host run in a new process group, records pgid plus start identity, signals the group, records `setsid` escapees, and treats an empty group as host-process death, not container death. Frozen Contract 010 is unchanged by this design.
- Nucleus observer docs (`experiments/recovery-core/OBSERVER.md`) state that cwd/open-file/`kill -0` scans cannot treat `Unknown` as empty, and that a process which changes cwd and drops worktree files disappears from that scan. Contract 002 forbids sharing mutable build directories and assigns shared Rust build/cache ownership to Oxide.

**Proposed (not observed, not implemented):** the guardian, accounting cap, experimental path-omitting partition, and six-scenario evidence map below. A cache hit is never validation success.

## Process containment

### Candidate mechanisms

| Mechanism | What it actually is | Fit for this native macOS pilot |
| --- | --- | --- |
| Dedicated process group (`POSIX_SPAWN_SETPGROUP` / `setpgid`, then `killpg`) | Kernel grouping for signal delivery. `ESRCH` on `kill(-pgid, 0)` is a current empty-group observation. | Supported. Necessary. Insufficient alone: `setsid`, `posix_spawn` with a new group, and test fixtures that create their own group leave the recorded pgid. |
| Start-identity env inherited by the tree, plus pid+start-time (Contract 010) | User-space birth marker. Proves a live PID is the same process, not a recycled PID. | Supported. Necessary for signalling and escapee tracking. A process can drop the env. Whole-second macOS start times accept same-second PID reuse. |
| Slot-path `lsof`/cwd scan (Nucleus observer shape) | Observation of processes whose cwd or open file lies under the slot, checkout or capture root. | Supported. Necessary for writers that left the group but still hold slot files. `Unknown` (denied `ps`/`lsof`, SIP hiding) is not quiet. A writer that closed every slot FD and `chdir`'d away disappears. |
| `kqueue` `NOTE_TRACK` | Follow forks automatically. | **Not supported** on this host. Darwin `sys/event.h`: no longer supported as of 10.5. Do not plan on it. |
| `kqueue` `NOTE_EXIT`/`NOTE_FORK` on known PIDs | Events for PIDs already attached. `NOTE_FORK` does not reliably name every child. | Optional helper for the leader and already-known children. Not a tree proof. |
| `proc_listchildpids` | Direct children of one PID, racy. | Optional helper. Not a tree proof. |
| `sandbox-exec` / App Sandbox / Endpoint Security / VM | Kernel or framework confinement. | Deprecated, entitlement-heavy, or a different experiment. Out of this brief. |
| Linux cgroups | Kernel tree + resource control. | Not present on this macOS host. |

**Recommended supported mechanism:** compose Contract 010's process-group and start-identity discipline with a slot-rooted file/cwd observer. That is the strongest user-space control this host offers without installing a service, creating a volume, or changing frozen Contract 010.

When the workflow is a Queue/Effigy-admitted host run, inherit that run's process group and start identity. Do not `setpgid` the Cargo/rustc tree out of the consumer group: that would look like an escape to Contract 010 and would change consumer process accounting. Oxide adds slot/checkout/capture path scans and extra escapee records; it does not replace the consumer supervisor.

When the workflow is a local adapter outside a consumer host run, the adapter creates the group with `POSIX_SPAWN_SETPGROUP`, records pgid plus start identity, and is the sole signalling owner.

`sandbox-exec` is rejected. A new APFS volume is a capacity tool, not descendant containment.

### Proof obligation

After cancellation, client crash, daemon crash, lost reply, or restart, a slot stays `leased` or becomes `uncertain`. It becomes reusable or evictable only after **all** of the following authenticated records exist for that lease. Any missing, conflicting, or `Unknown` item keeps the slot uncertain. Age, vanished PID, empty group, daemon restart, or a test verdict never releases it.

1. **Group emptiness.** `kill(-pgid, 0)` returned `ESRCH` for the recorded pgid, or every identity-matched member is proven dead by pid+start-identity. A failed or denied scan is `Unknown`, not empty.
2. **Escapees dead.** Every process recorded as leaving the group, plus every process observed with cwd or an open file under the slot, checkout, or capture root and carrying the start identity (or otherwise attributable to the lease), is proven dead by pid+start-identity. `proc_listchildpids` and `ps` listings are discovery aids, not completeness proofs.
3. **Slot quiet.** A same-UID `lsof`/cwd scan of the slot, both checkouts and the capture root is `Observed` and quiet, with the Nucleus observer write-exclusion premise recorded for other UIDs. `Unknown` keeps the slot uncertain.
4. **Capture evidence.** The consumer's authenticated capture receipt (digests bound to run/source/selector/toolchain) is present, or capture is durably recorded as not started. Oxide does not mint that receipt. Missing capture with an ended process still leaves storage uncertain if the run executed.
5. **Idempotent reconcile.** The same `(lease_id, reconcile_id)` with byte-identical evidence returns the same result. Changed evidence under that ID is rejected.

Distinguish:

- **Observation:** pid, pgid, lock file, `kill -0`, `ps`, `lsof`. These locate work. Their absence is not idle.
- **Containment proof:** the conjunction above, durable, fail-closed. macOS does not give this brief a kernel tree that cannot escape. Qualification therefore proves a fail-closed guardian, not cgroup-equivalent confinement.

Residual hole (must be named in the execution brief): a process that leaves the group, drops the identity env, closes every slot FD, `chdir`s away, and later reopens the slot by path. rustc/Cargo do not typically `setsid`; Nucleus observer tests deliberately spawn other groups. Treat that class as `uncertain` if a cancellation or crash hits during those tests.

If a production descendant still writes after the guardian has claimed quiet, that is a blocking qualification failure, not a reason to weaken the proof.

## Capacity

The 10 GiB disposable footprint and 20 GiB host free-space reserve **cannot be kernel-enforced on the existing Data volume** under this brief's authority. Periodic sampling and preflight `statfs` are observations. Host concurrent writes (other Queue tasks, Time Machine, ordinary use) can invalidate a reserve between samples. `RLIMIT_FSIZE` does not cap a tree. User quotas are not enabled.

### Proposed control (accounting, fail-closed)

Covered trees: disposable checkout A, disposable checkout B, the one exclusive intermediate slot, and that session's capture/log directories. Shared `CARGO_HOME` downloads and the operator's other checkouts are outside the 10 GiB cap and must not be deleted to make room.

- **Occupied bytes:** walk those trees and sum unique `(device, inode)` using `st_blocks * 512`. File lengths and `du` without inode dedup over-count hard links. APFS compression, clone extents and snapshots can make this an upper or lower bound; record the method and that limitation. Unknown size is unknown, not zero.
- **Host free space:** `statfs` `f_bavail * f_frsize` (or `f_bsize` if `f_frsize` is 0) on the Data volume that holds the trees. This is a snapshot.
- **Acquire refusal.** Refuse if occupied + a declared per-scenario headroom would exceed 10 GiB, or if free space is below 20 GiB. Do not start a build when either bound is unknown.
- **In-run abort.** Resample at acquire, compile end, test end, capture end, finalize, and on a bounded timer no slower than once per minute while leased. Crossing 10 GiB occupied or 20 GiB free-space aborts the workflow, leaves the slot `uncertain` until the containment proof, and reports. No automatic retry or expansion.
- **Writes.** Occupied-space deltas are not SSD write counts. If `iostat` on the APFS physical store can be sampled around a scenario without extra tools, record the delta as an observation; otherwise record write attribution as unavailable. An APFS quota would still not cap write amplification.

**Coverage:** only the named trees, only at sample instants. **Overhead:** tree walks of a few-GiB slot are expected to be seconds, not the dominant budget. **Residual races:** a compiler can write past 10 GiB between samples; any other process can consume the 20 GiB host reserve at any time; APFS metadata and snapshots outside the walk are uncounted.

This is an honest abort policy, not a reserve guarantee.

### Smallest scoped adjustment if Tom needs a hard cap

Separately authorize an exclusive APFS volume on the existing container:

```text
diskutil apfs addVolume <container> APFS <name> -quota 10g
```

from `diskutil(8)`. `-quota` limits that volume's file data. Put checkouts, slot and captures on it. `-reserve` guarantees space **to** the volume; it does not keep 20 GiB free for the rest of the host. Keep the 20 GiB host reserve as preflight-plus-abort on Data (`f_bavail ≥ 20 GiB` in addition to the empty quota volume). Residual: other writers to that volume if it is not exclusive; clone/snapshot accounting; still no write-volume cap; still no protection of Data against concurrent host processes.

Do not pretend the accounting control is that volume. This task must not create it.

Nucleus's documented 2.3 GiB unique artifact sample for the planning+diagnostics pair leaves room under 10 GiB for two source checkouts, private `CARGO_TARGET_DIR` finals, one shared slot and captures, if the selector stays that narrow. Full `qa:core:test` (`cargo test --workspace --offline`) is larger and may not fit; preflight must measure occupied after the isolated cold run before sharing the slot. If the cold run already exceeds the cap, stop and report; do not widen the cap inside the session.

## Candidate selector, head and toolchain

Recorded for a later execution brief. **Not owner consent.** Source-owner coordination and Nucleus lead `lead-1965` (compiler-wrapper) remain gates. Do not prompt active Nucleus threads. No assumed MSRV. No new toolchain install.

| Item | Candidate | Prerequisites still missing |
| --- | --- | --- |
| Selector | Nucleus Effigy `check:core:test` with args `-p nucleus-core --test planning --test build_diagnostics` | Nucleus owner agreement that this exact selector/args may run in two disposable checkouts under Oxide's slot. |
| Why this selector | Named in Nucleus `AGENTS.md` Validate examples. Already measured on this host (Nucleus `crates/README.md`). Exercises serde derive (proc macros), bundled rusqlite (native build script/C), 58 planning tests plus diagnostics, and source-line backtraces. Fits the six-scenario / 90-minute / 10 GiB bounds better than full `qa:core:test`. | If the owner instead requires `qa:core:test`, preflight the isolated cold occupied size and wall time; observer tests spawn extra process groups and are a containment stress, not a reason to skip the guardian. |
| Exact head | Inspection snapshot: Nucleus `bca75ed0143f96cd20b88ff72a421f1fb5f4ef19` (tree `67ecef91c907ca4085d94ea242b62f678c92993f`, 2026-10-09). Re-pin at execution approval; do not treat this SHA as still `main`. | Owner freeze of that SHA (or a later named SHA) for both disposable checkouts, matching lockfile, no extra patches except any invalidation patch the owner approves for scenario 4. |
| Toolchain | Already-installed `rustc 1.97.1 (8bab26f4f)` / `cargo 1.97.1 (c980f4866)`, `RUSTUP_TOOLCHAIN=1.97.1`, host `aarch64-apple-darwin`. Pin in the disposable environment because Nucleus has no `rust-toolchain.toml`. | Confirm the same sysroot on both checkouts. Do not install. Scenario 6 may use already-installed `1.96.1` only as a key-separation/refusal probe, and may skip a second compiler build. |
| Paths | Two disposable checkout directories and one slot directory, all on Data (or on the separately authorized quota volume). Private `CARGO_TARGET_DIR` per checkout/run. `CARGO_BUILD_BUILD_DIR` = the slot. | Exact absolute paths in the execution brief. No links into real Nucleus/Queue checkouts. |
| Oxide authoring | Repository RustQuality profile is `strict` with empty deviations; Oxide pins 1.97.1. | Irrelevant to this docs task. Any later Rust in 003/004 follows that profile. The pilot compiles Nucleus, not Oxide. |

`CARGO_HOME` may stay the per-user download cache Queue already shares. That is not slot sharing.

## Experimental cross-path partition

Default compatibility key **includes checkout-path identity** (or Cargo's `{workspace-path-hash}` if redirection is ever left to Cargo). That default stays conservative.

The **approved experimental variant** for this pilot only:

- One exclusive slot, one compatibility key with checkout-path **omitted**, used by both disposable checkouts A and B.
- Acquire records `partition_variant = experimental_cross_path` on the lease. A claimant without that variant, or with a different path-including key, is refused.
- Adapter sets `CARGO_BUILD_BUILD_DIR` to that slot path for A and B. It does not use `{workspace-path-hash}` for this variant. `CARGO_TARGET_DIR` and capture remain per checkout/run.
- Only scenarios 2, 3 and 4 (and the in-session failure proofs that reuse the slot) use this variant. Scenario 1 uses a private isolated build directory, not the slot. Scenarios 5 and 6 must change the key or be refused; they do not silently stay on the experimental key.

If scenario 3's path/proc-macro/generated-code evidence fails, stop cross-path claims, keep the slot path-isolated, and report reduced benefit. Path isolation does not meet the cross-worktree objective. Do not expand the matrix to chase a hit.

## Six-scenario evidence map

Stay inside six worker scenarios, 90 minutes worker, 30 minutes independent reviewer, 10 GiB occupied, 20 GiB free-space abort. No automatic retries. Every scenario that actually builds runs the selected tests in the same lease. A cache hit cannot skip tests or mint a pass.

Minimum evidence is what can **falsify** ownership, reuse, path/proc-macro correctness, always-run tests, and exact-run binding. Timing and occupied bytes are recorded; they are not success.

| # | Scenario | What must be in the capture | Falsifier |
| --- | --- | --- | --- |
| 1 | Isolated cold reference, private build directory, checkout A | Wall time; occupied/reserved; write observation or `unavailable`; full test execution record; consumer receipt binding binaries/logs/receipts to SHA, tree, selector, toolchain 1.97.1, checkout A. Slot untouched. | Tests skipped; receipt missing or bound to the wrong source; slot written. |
| 2 | Shared-slot cold at A, experimental variant | Same as 1, plus lease id, partition digest without path, `CARGO_BUILD_BUILD_DIR` = slot, Cargo rebuild/reuse summary. Tests executed. | Tests skipped; another claimant touched the slot; lease not durable before first rustc. |
| 3 | Same-head warm at B, same slot | Tests executed at B. Cargo reuse/rebuild summary. **Path proof:** captured test output plus `strings` (or equivalent) of the executed `planning` and `build_diagnostics` binaries under B's `CARGO_TARGET_DIR`. Checkout B's absolute path must appear where those binaries embed paths; checkout A's absolute path must not. `build_diagnostics` passing on `build_diagnostics.rs:<line>` alone is not path proof (`CARGO_MANIFEST_DIR` is compile-time; A remaining on disk can hide stale reuse). Proc-macro/native: rusqlite bundled and serde derive units either rebuild for B or, if reused, still pass the path proof. Receipt bound to B's checkout identity and the same SHA/tree/toolchain. | Tests skipped; binaries executed from A; embedded A paths in B's run; tests pass against A's tree via baked `CARGO_MANIFEST_DIR`. That is failed portability, not a hit. |
| 4 | Controlled tracked-source change in one disposable checkout | Owner-approved patch or a recorded content edit (not only `touch`) with new tree digest. Affected units rebuild. Tests executed on the new tree. Receipt bound to the new digest. Slot remains exclusive. | Tests skipped; receipt still names the old tree; rebuild of affected units not evidenced. **Needs owner-approved source change.** `touch`-only is weaker and does not prove content invalidation; use it only if the owner refuses a patch, and name the weaker claim. |
| 5 | Effective configuration change | Change `RUSTFLAGS` or an equivalent allowlisted config input. Acquire must use a new partition key or refuse while incompatible state is present. No claim of a build result if only key-separation is shown. | Old partition reused; silent omit of the changed input. No extra scenario. |
| 6 | Toolchain control | `RUSTUP_TOOLCHAIN=1.96.1` (already installed) must not acquire the 1.97.1 partition. Refusal or a distinct key is success. Do not install a toolchain. Skip a second full compiler build if time/space would break the cap; key-separation is enough. | 1.96.1 run writes the 1.97.1 slot. |

**In-session proofs, not extra scenarios** (same 90-minute budget):

- Concurrent second claimant refused; slot bytes unchanged.
- Idempotent acquire/finalize after a deliberately dropped reply.
- Daemon or client kill during an in-flight build; restart keeps the slot `uncertain` until the containment proof; empty pgid alone does not release.
- Budget abort evicts only a reconciled idle slot; active/uncertain remains.

**Reviewer (30 minutes):** replay at most one warm (scenario 3) and one meaningful miss (4, 5 or 6), checking receipts, path strings, test execution and the uncertain-after-crash record. Independent exact-head review of this design is separate and precedes any execution brief.

**Fixture / different-workflow cases** (do not expand the matrix):

- Path-absolute assertion: committed `build_diagnostics` is insufficient; capture binary strings as above rather than adding a Nucleus test.
- Observer orphan groups: present in full `qa:core:test`, not in the planning+diagnostics pair. If the owner requires `qa:core:test`, cancellation during those tests is the containment stress case; budget time for it by dropping a less informative scenario only with explicit execution-brief edit, not ad hoc.
- Generated code beyond serde/rusqlite: this selector does not exercise a workspace `build.rs` of Nucleus itself. That is a coverage limit, not a seventh scenario. Do not swap in another repo to chase proc-macro breadth.

## Disposition

**Proceed to scoped execution approval**, with honest accounting limits, after the missing prerequisites below. Not blocked on macOS process APIs: the recommended guardian is supported and fail-closed. Blocked only if Tom requires a kernel-hard 10 GiB/20 GiB **guarantee**; then the smallest next ask is the exclusive APFS `-quota 10g` volume plus the still-racy 20 GiB Data preflight, under a new brief.

This is not a claim that the pilot ran, that reuse works, or that consumers may adopt.

### Missing prerequisites

1. Independent exact-head review of this file against the brief.
2. Nucleus owner coordination: selector/args, frozen SHA, disposable-checkout permission, scenario-4 patch policy, and `lead-1965` disposition. No parallel compiler wrapper.
3. Reviewed 003 storage-core and 004 adapter implementations to run against; this file does not wait on their merge, and they must not treat this text as execution authority.
4. Separate execution brief: exact paths, cap/reserve abort policy, partition variant flag, six-scenario script, crash/concurrency proofs, 90/30-minute hard stops, and either accounting-only honesty or the APFS-volume adjustment.
5. Preflight on the chosen trees immediately before execution: occupied method, free space ≥ 20 GiB, cold-run fit under 10 GiB. If unenforceable or already over, stop and report.

Queue/Nucleus keep admission, identity, settlement, retention and capture. Oxide owns how the slot is leased, observed and fail-closed on this host.
