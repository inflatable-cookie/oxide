# Qualification readiness: containment and capacity

Design-only. This file answers the host/consumer gates that [architecture](architecture.md) left unproved: descendant containment after cancel/crash/lost reply/restart, and whether the proposed 10 GiB disposable cap and 20 GiB host free-space reserve can actually be enforced. It does not authorize a pilot, consumer adoption, or host installation. Saving or merging it grants none of those. Unmerged worker code is not an authority.

Subsequent operator ruling, 2026-10-09: Tom approved a bounded documentation-only completion/fencing follow-up and selected a monitored 20 GiB start/abort threshold. [Architecture](architecture.md#budget-and-observations) owns that current resource policy; the guarantee-versus-threshold asks below describe the original readiness disposition and are now answered. No permanent-quarantine or single-held-session fallback has been accepted, and no pilot execution is authorized. The enforced disposable-footprint cap and usable native completion proof remain gates.

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

**Proposed (not observed, not implemented):** signalling (not completeness), `disp_tmp` redirect, 10 GiB accounting, experimental path-omitting partition, six-scenario map, and the blocked/unresolved disposition below. A cache hit is never validation success.

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

**Signalling mechanism (not a completeness proof):** compose Contract 010's process-group and start-identity discipline with a slot-rooted file/cwd observer. Use it to deliver SIGTERM/SIGKILL to known members and to record what the host can see. It is the strongest user-space *control* this host offers without installing a service or changing frozen Contract 010. It does **not** enumerate every descendant.

When the workflow is a Queue/Effigy-admitted host run, inherit that run's process group and start identity. Do not `setpgid` the Cargo/rustc tree out of the consumer group: that would look like an escape to Contract 010 and would change consumer process accounting. Oxide adds slot/checkout/capture/tmp path scans and extra escapee records; it does not replace the consumer supervisor.

When the workflow is a local adapter outside a consumer host run, the adapter creates the group with `POSIX_SPAWN_SETPGROUP`, records pgid plus start identity, and is the sole signalling owner.

`sandbox-exec` is rejected. An APFS volume can fail an unmount while FDs are open; that is a current-open-file observation, not a process-tree proof, and it does not stop a later reopen-by-path after remount.

### Proof obligation

Native macOS, with the APIs this brief may use, **cannot prove that every build/test descendant has ended**. `NOTE_TRACK` is unsupported. Process-group membership, start-identity environment, `proc_listchildpids`, `ps`, `lsof`, and cwd scans observe only processes they can still see. A descendant that leaves the group (`setsid` / new `posix_spawn` group), drops the identity variable, closes every slot/checkout/capture/tmp descriptor, `chdir`s away, and later reopens a remembered path is not in that set. The selected Nucleus planning+diagnostics selector also omits the observer tests that spawn extra process groups, so the candidate workflow does not even exercise the known extra-group case.

Therefore the observational conjunction (empty pgid + recorded escapees dead + quiet `lsof`/cwd + capture receipt) is **not** a containment proof. Using it as a `reconcile` success predicate can pass while that descendant is still live. That predicate is refused.

Distinguish:

- **Observation:** pid, pgid, lock file, `kill -0`, `ps`, `lsof`, unmount-busy. These locate some work and support signalling. Their absence is not idle and is not completeness.
- **Containment proof:** a record that accounts for every descendant that could still write. This host has no supported mechanism that produces that record.

**Release rule:** after any launch of Cargo/rustc/tests, and after cancellation, client crash, daemon crash, lost reply, or restart, the slot becomes or stays `uncertain` and remains **unavailable for reuse and eviction** for the rest of this protocol. There is no observational path to `idle`. Age, vanished PID, empty group, quiet scan, daemon restart, leader exit, or a test verdict never releases it. `reconcile` may confirm and persist `uncertain`; it must not transition to `idle` or `evictable`. Idempotent `(lease_id, reconcile_id)` still applies: identical evidence returns the same `uncertain` result; changed evidence is rejected.

Operator destruction of a quarantined tree or volume, without remounting that path for a new lease, stays outside this protocol as [architecture](architecture.md#durable-ownership-and-recovery) already states.

**Pilot measurement without claiming completeness:** scenarios 2–4 may share one still-held exclusive lease (one `acquire`, cargo A then cargo B under that lease, `finalize` of the session still leaves the slot `uncertain` because descendants are unproven). That measures Cargo reuse under exclusive admission. It is not a descendant-complete release, not idle eviction, and not crash-recovery reuse. Concurrent second claimants are refused while `leased` or `uncertain`.

Capture evidence remains required for consumer settlement and does not make the slot idle. Oxide does not mint that receipt.

## Capacity

The two numbers are different controls. Sampling and preflight `statfs` are observations. `RLIMIT_FSIZE` caps one file, not a tree. User quotas for uid 501 are none. Host concurrent writes (other Queue tasks, Time Machine, ordinary use) can consume Data free space at any time.

### Temporary output (must be in the 10 GiB footprint)

Cargo/rustc/cc and tests write intermediates under `CARGO_BUILD_BUILD_DIR`, finals under `CARGO_TARGET_DIR` (including `CARGO_TARGET_TMPDIR` for integration tests, documented in Cargo's environment-variable reference), and **OS temporary files** via `std::env::temp_dir()`, which on this host follows `TMPDIR` (typically a per-user directory under `/var/folders/…/T/`). Native tools may also read `TMP` and `TEMP`. Cargo's own config reference shows `[env] TMPDIR = { value = "…", force = true }` as the supported way to override that for processes Cargo launches.

Without redirecting those temps into the disposable trees, they bypass both a sampled 10 GiB walk and an APFS volume that only holds checkouts, slot and captures.

**Required redirect for any 10 GiB claim:** before spawning Effigy/Cargo, force `TMPDIR`, `TMP` and `TEMP` to one dedicated directory `disp_tmp` on the same filesystem as the slot (and on the quota volume if that volume exists). Covered trees for occupied-byte accounting then are: checkout A, checkout B, the exclusive slot, `disp_tmp`, and that session's capture/log directories. Shared `CARGO_HOME` downloads stay outside the cap and must not be deleted to make room.

Residual bypasses after that redirect: a tool that hard-codes `/tmp` or `/private/tmp`, or that ignores `TMPDIR`; APFS metadata and snapshots outside the walk; writes between samples.

### 10 GiB disposable cap

On the existing Data volume the cap is accounting plus abort, not kernel enforcement:

- **Occupied bytes:** walk the covered trees, including `disp_tmp`, and sum unique `(device, inode)` using `st_blocks * 512`. File lengths and `du` without inode dedup over-count hard links. APFS compression, clone extents and snapshots can make this an upper or lower bound; record the method. Unknown size is unknown, not zero.
- **Acquire refusal.** Refuse if occupied + declared per-scenario headroom would exceed 10 GiB, or if occupied is unknown.
- **In-run abort.** Resample at acquire, compile end, test end, capture end, finalize, and on a bounded timer no slower than once per minute while leased. Crossing 10 GiB aborts, leaves the slot `uncertain` (unavailable), and reports. No automatic retry or expansion.
- **Writes.** Occupied-space deltas are not SSD write counts. If `iostat` on the APFS physical store can be sampled around a scenario without extra tools, record the delta; otherwise record write attribution as unavailable. An APFS quota still would not cap write amplification.

**Coverage:** the named trees including `disp_tmp`, at sample instants, plus whatever actually lands under the forced temp vars. **Overhead:** tree walks of a few-GiB slot are seconds. **Residual races:** a compiler can write past 10 GiB between samples; ignored-`TMPDIR` tools; concurrent writers on Data.

**Smallest kernel adjustment for the 10 GiB cap only** (separate authority; this task must not create it):

```text
diskutil apfs addVolume <container> APFS <name> -quota 10g
```

from `diskutil(8)`. `-quota` limits that volume's file data. Place checkouts, slot, `disp_tmp` and captures on it, and force `TMPDIR`/`TMP`/`TEMP` there. Residual: other writers if the volume is not exclusive; clone/snapshot accounting; hardcoded `/tmp`; still no write-volume cap. `-reserve` guarantees space **to** this volume; it does not keep space free for the rest of the host.

Nucleus's documented 2.3 GiB unique artifact sample for the planning+diagnostics pair does not include `disp_tmp` or two source checkouts. Preflight after the isolated cold run must measure the full covered set. If it exceeds 10 GiB, stop and report.

### 20 GiB host free-space reserve — unresolved

This reserve is a property of the shared Data volume, not of the disposable trees. An exclusive `-quota 10g` volume does not enforce it. `-reserve` on that volume is the opposite polarity. Preflight `statfs` `f_bavail` and a periodic abort when `f_bavail` drops below 20 GiB are the same sampling the brief already calls insufficient: any other process can consume the reserve between samples, and this brief cannot stop those processes.

There is no supported, in-authority mechanism on this multi-writer laptop that **guarantees** 20 GiB remain free. Architecture's rule that an unenforceable bound means "do not start a build" therefore applies to a *guaranteed* 20 GiB reserve.

This is not folded into the 10 GiB quota adjustment. It needs an explicit Tom decision before any execution brief:

1. Treat 20 GiB as a heuristic start/abort threshold only (not a guarantee), and say so in the execution brief; or
2. Keep the guarantee requirement, in which case the resource gate stays blocked on this host.

## Candidate selector, head and toolchain

Recorded for a later execution brief. **Not owner consent.** Source-owner coordination and Nucleus lead `lead-1965` (compiler-wrapper) remain gates. Do not prompt active Nucleus threads. No assumed MSRV. No new toolchain install.

| Item | Candidate | Prerequisites still missing |
| --- | --- | --- |
| Selector | Nucleus Effigy `check:core:test` with args `-p nucleus-core --test planning --test build_diagnostics` | Nucleus owner agreement that this exact selector/args may run in two disposable checkouts under Oxide's slot. |
| Why this selector | Named in Nucleus `AGENTS.md` Validate examples. Already measured on this host (Nucleus `crates/README.md`). Exercises serde derive (proc macros), bundled rusqlite (native build script/C), 58 planning tests plus diagnostics, and source-line backtraces. Fits the six-scenario / 90-minute / 10 GiB bounds better than full `qa:core:test`. | Omits observer orphan-group tests. That is a completeness gap, not a reason to prefer `qa:core:test`: extra groups still would not close the identity-dropping hole. |
| Exact head | Inspection snapshot: Nucleus `bca75ed0143f96cd20b88ff72a421f1fb5f4ef19` (tree `67ecef91c907ca4085d94ea242b62f678c92993f`, 2026-10-09). Re-pin at execution approval; do not treat this SHA as still `main`. | Owner freeze of that SHA (or a later named SHA) for both disposable checkouts, matching lockfile, no extra patches except any invalidation patch the owner approves for scenario 4. |
| Toolchain | Already-installed `rustc 1.97.1 (8bab26f4f)` / `cargo 1.97.1 (c980f4866)`, `RUSTUP_TOOLCHAIN=1.97.1`, host `aarch64-apple-darwin`. Pin in the disposable environment because Nucleus has no `rust-toolchain.toml`. | Confirm the same sysroot on both checkouts. Do not install. Scenario 6 may use already-installed `1.96.1` only as a key-separation/refusal probe, and may skip a second compiler build. |
| Paths | Two disposable checkout directories, one slot directory, one `disp_tmp` directory, and per-run capture directories, all on Data (or on a separately authorized 10 GiB quota volume that holds **all** of these). Private `CARGO_TARGET_DIR` per checkout/run. `CARGO_BUILD_BUILD_DIR` = the slot. `TMPDIR`, `TMP` and `TEMP` forced to `disp_tmp`. | Exact absolute paths in the execution brief. No links into real Nucleus/Queue checkouts. |
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

Stay inside six worker scenarios, 90 minutes worker, 30 minutes independent reviewer, and the 10 GiB occupied abort (including `disp_tmp`). Do not treat a 20 GiB free-space *guarantee* as in force. No automatic retries. Every scenario that actually builds runs the selected tests in the same lease. A cache hit cannot skip tests or mint a pass. Scenarios 2–4 share one still-held lease; they do not finalize to `idle` between them.

Minimum evidence is what can **falsify** ownership, reuse, path/proc-macro correctness, always-run tests, and exact-run binding. Timing and occupied bytes are recorded; they are not success.

| # | Scenario | What must be in the capture | Falsifier |
| --- | --- | --- | --- |
| 1 | Isolated cold reference, private build directory, checkout A | Wall time; occupied of covered trees including `disp_tmp`; write observation or `unavailable`; full test execution record; consumer receipt binding binaries/logs/receipts to SHA, tree, selector, toolchain 1.97.1, checkout A. Slot untouched. | Tests skipped; receipt missing or bound to the wrong source; slot written; temps left in the default `/var/folders` TMPDIR. |
| 2 | Shared-slot cold at A, experimental variant | Same as 1, plus lease id, partition digest without path, `CARGO_BUILD_BUILD_DIR` = slot, Cargo rebuild/reuse summary. Tests executed. | Tests skipped; another claimant touched the slot; lease not durable before first rustc. |
| 3 | Same-head warm at B, same slot | Tests executed at B. Cargo reuse/rebuild summary. **Path proof:** captured test output plus `strings` (or equivalent) of the executed `planning` and `build_diagnostics` binaries under B's `CARGO_TARGET_DIR`. Checkout B's absolute path must appear where those binaries embed paths; checkout A's absolute path must not. `build_diagnostics` passing on `build_diagnostics.rs:<line>` alone is not path proof (`CARGO_MANIFEST_DIR` is compile-time; A remaining on disk can hide stale reuse). Proc-macro/native: rusqlite bundled and serde derive units either rebuild for B or, if reused, still pass the path proof. Receipt bound to B's checkout identity and the same SHA/tree/toolchain. | Tests skipped; binaries executed from A; embedded A paths in B's run; tests pass against A's tree via baked `CARGO_MANIFEST_DIR`. That is failed portability, not a hit. |
| 4 | Controlled tracked-source change in one disposable checkout | Owner-approved patch or a recorded content edit (not only `touch`) with new tree digest. Affected units rebuild. Tests executed on the new tree. Receipt bound to the new digest. Slot remains exclusive. | Tests skipped; receipt still names the old tree; rebuild of affected units not evidenced. **Needs owner-approved source change.** `touch`-only is weaker and does not prove content invalidation; use it only if the owner refuses a patch, and name the weaker claim. |
| 5 | Effective configuration change | Change `RUSTFLAGS` or an equivalent allowlisted config input. Acquire must use a new partition key or refuse while incompatible state is present. No claim of a build result if only key-separation is shown. | Old partition reused; silent omit of the changed input. No extra scenario. |
| 6 | Toolchain control | `RUSTUP_TOOLCHAIN=1.96.1` (already installed) must not acquire the 1.97.1 partition. Refusal or a distinct key is success. Do not install a toolchain. Skip a second full compiler build if time/space would break the cap; key-separation is enough. | 1.96.1 run writes the 1.97.1 slot. |

**In-session proofs, not extra scenarios** (same 90-minute budget):

- Concurrent second claimant refused while `leased` or `uncertain`; slot bytes unchanged.
- Idempotent acquire/finalize after a deliberately dropped reply; the slot stays `uncertain` after launch, not `idle`.
- Daemon or client kill during an in-flight build; restart leaves the slot `uncertain` with **no** observational `reconcile` success path to `idle` or `evictable`. Empty pgid, quiet `lsof`, or vanished PID must not release it. That is the interruption proof this host can actually run.
- Budget abort does not evict a launched slot. It refuses new acquire and turns the in-flight lease `uncertain`. Idle eviction after a Cargo/test launch is not in the proven set.

**Reviewer (30 minutes):** replay at most one warm (scenario 3) and one meaningful miss (4, 5 or 6), checking receipts, path strings, test execution, `disp_tmp` inclusion in occupied bytes, and that an interrupted run remains unavailable. Independent exact-head review of this design is separate and precedes any execution brief.

**Fixture / different-workflow cases** (do not expand the matrix):

- Path-absolute assertion: committed `build_diagnostics` is insufficient; capture binary strings as above rather than adding a Nucleus test.
- Observer orphan groups: present in full `qa:core:test`, not in the planning+diagnostics pair. Switching to `qa:core:test` would exercise extra process groups and still would not close the identity-dropping/path-leaving hole. Do not expand the matrix to chase a completeness proof this host cannot give.
- Generated code beyond serde/rusqlite: this selector does not exercise a workspace `build.rs` of Nucleus itself. That is a coverage limit, not a seventh scenario. Do not swap in another repo to chase proc-macro breadth.
- Descendant-complete release: would need a kernel tree (cgroup, supported sandbox, or equivalent) or a fixture whose processes cannot `setsid`/drop identity/leave path — Cargo/rustc/tests are not that fixture. Not a sixth-scenario substitute.

## Disposition

**Not proceed-to-execution.** Two gates stay unresolved until Tom rules. This file is still the documentation answer 001 lacked; saving or merging it grants no pilot, installation or adoption authority.

| Gate | Status | Why |
| --- | --- | --- |
| Descendant-complete release after interruption (and after any Cargo/test launch) | **Blocked** on native macOS with supported APIs | Hidden identity-dropping/path-leaving descendants can evade group/identity/`lsof` scans, so those scans must not be a `reconcile` success predicate. Interrupted and post-launch slots stay `uncertain` and unavailable. The planning+diagnostics selector does not include observer orphan-group tests. |
| 10 GiB disposable footprint | Protocol proposed, not yet authorized | Include `disp_tmp` via forced `TMPDIR`/`TMP`/`TEMP`. Accounting abort on Data; kernel cap only if a separately authorized `-quota 10g` volume holds checkouts, slot, `disp_tmp` and captures. Hardcoded `/tmp` remains a residual bypass. |
| 20 GiB host free-space **guarantee** | **Unresolved / blocked as a guarantee** | Quota volumes do not enforce host free space. Sampling/preflight cannot. Needs Tom's choice below. |

**Smallest asks for Tom (do not start a build until answered):**

1. Containment: accept permanent `uncertain` (unavailable) after launch/interrupt as the macOS protocol, and treat descendant-complete idle/eviction as out of this pilot; or keep completeness as a requirement, in which case qualification stays blocked until a kernel tree exists.
2. Reserve: accept 20 GiB as a heuristic start/abort threshold only; or keep it as a guarantee, in which case the resource gate stays blocked on this shared Data volume.

Cross-path Cargo measurement under one still-held lease can be scoped later only if (1) is the permanent-uncertain choice. It would not be a containment pass.

### Missing prerequisites

1. Independent exact-head review of this revision against the brief.
2. Tom's two rulings above.
3. Nucleus owner coordination: selector/args, frozen SHA, disposable-checkout permission, scenario-4 patch policy, and `lead-1965` disposition. No parallel compiler wrapper.
4. Reviewed 003 storage-core and 004 adapter implementations to run against; this file does not wait on their merge, and they must not treat this text as execution authority.
5. Separate execution brief only after (2): exact paths including `disp_tmp`, 10 GiB abort (and quota volume if chosen), 20 GiB policy as Tom ruled, partition variant flag, six-scenario script that never reconciles a launched slot to idle, crash proof that unavailable sticks, 90/30-minute hard stops.

Queue/Nucleus keep admission, identity, settlement, retention and capture. Oxide owns how the slot is leased and why it stays unavailable when descendants cannot be proven.
