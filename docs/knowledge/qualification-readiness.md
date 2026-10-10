# Qualification readiness: containment and capacity

This file records the host/consumer gates that [architecture](architecture.md) left unproved. The approved cooperative foreground-workflow contract now has an unpublished native fixture implementation and six bounded process scenarios in the Oxide project adapter. Independent review at exact head `1f76bc1f5d3ee8cdbfc552abecb3c1c30d82eda1` accepted all five required proof dimensions and replayed healthy reuse plus cancellation/retained-writer captures. Covering compile, format, docs and all 31 tests passed at that head. This qualifies the fixed cooperative fixture boundary only, not a real Cargo workflow, cross-worktree reuse, consumer adoption or host installation. Review evidence: [PR8 review](https://github.com/inflatable-cookie/oxide/pull/8#issuecomment-6083800151); native replay captures were retained at `/private/var/tmp/oxtEcZrT`.

Subsequent operator ruling, 2026-10-09: Tom approved a bounded documentation-only completion/fencing follow-up and selected a monitored 20 GiB start/abort threshold. [Architecture](architecture.md#budget-and-observations) owns that current resource policy; the guarantee-versus-threshold asks below describe the original readiness disposition and are now answered. No permanent-quarantine or single-held-session fallback has been accepted, and no real-build pilot execution is authorized. The native fixture proof was independently accepted within its stated limits; an enforced disposable cap for real Cargo remains a separate gate.

Ownership: this file. Protocol semantics stay in [architecture](architecture.md). Scope and authorization stay in [vision](vision.md). Open product choices stay in [questions](questions.md).

Subsequent pilot ruling, 2026-10-09: Tom approved the prepared standalone oxide#005 Cargo trial, frozen disposable Nucleus source/marker patch, minimal experimental Cargo/local-run boundary and one task-owned 8 GiB APFS sparse image. [Vision](vision.md#authority-boundaries) owns that new authority and its exclusions. Earlier pending-pilot/consumer-mapping statements below describe the pre-approval design; actual consumer mapping remains a rollout gate, while this trial uses honest local qualification identities. Approval is not evidence that the image capacity/output coverage, real Cargo completion or cross-path correctness has passed: those are worker preconditions and reviewed trial outcomes. No setup/build was performed by recording this ruling.

Subsequent supported-workflow ruling, 2026-10-09: Tom approved the narrower [cooperative foreground-workflow model](architecture.md#supported-writer-model) and its bounded native-fixture implementation/qualification. The broad writer-model findings below describe the earlier design and remain evidence that arbitrary same-user write fencing is unqualified. The fixture implementation now creates attempt-bound completion evidence from an owned native child and separately captured output. Its six independently accepted scenarios cover healthy release/reacquire, active contention, cancellation/restart uncertainty, a retained writer, capture/attempt binding, and preflight refusal. The ruling and task authorize no real Cargo/consumer pilot, installation or adoption; authority and bounds remain in [vision](vision.md#authority-boundaries) and the approved Queue brief.

## Cooperative native fixture boundary

The adapter's unpublished native runner starts only its fixed test-harness
fixture entry point. It clears the child environment, forces `TMPDIR`, `TMP`
and `TEMP` to the canonical task-owned scratch directory, and refuses paths
that resolve through a symlink or escape that root. Each scenario is
construction-bounded to at most eight owned children, sixteen regular files,
one MiB per file, 64 KiB each for stdout/stderr, and 10 GiB total across the
task-owned fixture, temporary and capture trees. The fixed child writes one
bounded slot file; the parent bounds stream memory and drains the pipes. A
pre-launch budget walk reserves the worst-case fixture output, while the
scenario evidence walk records actual files and bytes. This fixture bound is
not an enforceable quota for a real Cargo command and must not be generalized
to arbitrary project output.

Before each native child, after it, at lease/storage protocol boundaries, and
at 30-second intervals while waiting, the fixture samples host free space.
Unknown readings or less than 20 GiB refuse or abort before healthy completion;
samples and exact temporary/capture paths are retained with the fixture. The
readings describe the observed host state and cannot prevent a concurrent
unrelated writer from reducing free space between samples.

The direct child handle, PID and per-spawn marker are checked together. Normal
exit, bounded output capture, the fixture source/definition digest and exact
repository head are recorded. A consumer receipt is accepted only when its
host-run, run and attempt IDs and digest match the native capture bundle. The
storage core verifies the completion evidence against the active lease token,
slot ID and exact slot path before it releases the slot. A terminal pass is
not enough: the capture-mismatch case has a successful native child exit but
keeps the slot uncertain. The retained-writer case returns incomplete execution
and also keeps the slot uncertain, including after that known fixture child is
later reaped.

This mechanism relies on the approved workflow promise. It observes and joins
the fixture child it launched; it does not enumerate arbitrary same-user
processes or prove that an unregistered process could not write the slot.
Daemonization, handle transfer, undisclosed background writers and direct
access outside the adapter are unsupported bypasses and remain unprovable.
There is no PID/age/quiet-scan release path. Cancellation, missing evidence,
daemon/client restart and mismatched capture stay uncertain. Real Cargo,
consumer lifecycle, path portability, host isolation, and production resource
enforcement require separate gates.

Historical read-only readiness investigation (2026-10-09): committed Oxide, Nucleus and Queue documentation/config, installed toolchain metadata, Darwin headers and man pages, and `statfs`/`diskutil`/`quota` queries. That investigation also reviewed Apple's public App Sandbox, Endpoint Security and System Extensions documentation and the XNU VFS implementation. It ran no compilation and created or installed no daemon, checkout, quota, volume, sandbox or service.

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

**Earlier proposals (not observed, not implemented):** signalling (not completeness), `disp_tmp` redirect, 10 GiB cap, a path-omitting partition and a six-scenario Cargo map. They were not execution authority. The cooperative healthy-release boundary was later qualified for the fixed workflow and Cargo S1–S3; cross-path portability and invalidation remained unproved. A cache hit is never validation success.

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
| `sandbox-exec` | Legacy sandbox profile launcher. | Deprecated by its man page; not a supported foundation. |
| Linux cgroups | Kernel tree + resource control. | Not present on this macOS host. |

### Public macOS fencing candidates reviewed

These are the two concrete public-API candidates examined for this follow-up. Neither currently proves that a healthy command has relinquished every way to write the reusable slot.

| Candidate | Available mechanism and boundary | Why it does not establish safe release |
| --- | --- | --- |
| Inherited App Sandbox | A signed macOS app can launch child tools with sandbox inheritance. The child target uses `com.apple.security.app-sandbox` and `com.apple.security.inherit`; it inherits static sandbox rights, not rights granted dynamically after launch (such as PowerBox file access). | The static permission to write remains with any surviving child; parent exit does not revoke it. It does not account for an already-open or passed descriptor. Ordinary projects choose source paths at runtime, and Cargo build scripts, proc macros and tests execute project code; supporting those paths and tools would require compatibility work and a broad, fixed access policy. Repackaging/running the workflow inside an app sandbox changes the current command-line workflow. Static sandbox rights are therefore a research lead, not a completion proof. |
| Endpoint Security system extension | Public ES APIs can authorize selected operations and report process, open, close, descriptor and write events. Apple documents a descendant-scoped client for a process subtree, but marks it preliminary/Beta; that client makes other processes invisible. The regular ES entitlement must be requested from Apple. The client is packaged as a system extension and activation may require user approval. | The documented file write event is `NOTIFY_WRITE`, not an authorization event; `AUTH_OPEN` authorizes opening a file, not later writes through an existing descriptor. Thus an open/inherited/mapped or transferred writer is not shown to be revocable at healthy completion. A descendant-scoped client also cannot account for an unrelated same-user process. Its entitlement, system-extension lifecycle, approval, event-loss handling and ongoing compatibility support add a substantial host service boundary. This candidate is not available under the current design-only authority and is not a proof as documented. |

Apple's App Sandbox guide describes child inheritance and its static-rights limit ([App Sandbox inheritance](https://developer.apple.com/library/archive/documentation/Miscellaneous/Reference/EntitlementKeyReference/Chapters/EnablingAppSandbox.html)). Endpoint Security documents its authorization and notification event families, including `AUTH_OPEN` and `NOTIFY_WRITE` ([event types](https://developer.apple.com/documentation/endpointsecurity/es_event_type_t), [open authorization](https://developer.apple.com/documentation/endpointsecurity/es_event_type_auth_open), [write notification](https://developer.apple.com/documentation/endpointsecurity/es_event_type_notify_write)). Apple's descendant client covers the descendant subtree only and is marked preliminary ([`es_new_descendants_client`](https://developer.apple.com/documentation/endpointsecurity/es_new_descendants_client%28_%3A_%3A%29)). Its entitlement is requested from Apple ([Endpoint Security entitlement](https://developer.apple.com/documentation/BundleResources/Entitlements/com.apple.developer.endpoint-security.client)); Apple documents system-extension packaging and activation ([installing system extensions](https://developer.apple.com/documentation/systemextensions/installing-system-extensions-and-drivers?changes=_11), [activation request](https://developer.apple.com/documentation/systemextensions/ossystemextensionrequest/activationrequest%28forextensionwithidentifier%3Aqueue%3A%29?language=objc)).

**Inference from the documented event boundary and XNU VFS implementation:** an authorization decision on `open` cannot, by itself, revoke write access already obtained through a file descriptor; [XNU routes `write(2)` through the existing file's write operation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_vnops.c). ES `NOTIFY_WRITE` reports that write, but does not authorize or deny it. The API/event documentation reviewed does not establish fencing for a writable mapping or a descriptor passed outside the monitored subtree. This is a gap in the reviewed public mechanisms, not a claim that no future macOS facility could provide a proof.

### Writer model and completion proof

The baseline threat model includes all four classes: cooperating Cargo/Effigy commands; accidental daemonization or inherited writers; arbitrary descendants that change process group, identity or working directory; and unrelated processes running as the same user. The latter three are possible writers, not accepted exclusions. A per-user file permission or Oxide lease does not prevent another process with that same user identity from opening the slot. A descendant may preserve write capability in an inherited descriptor even after the launcher exits. Queue/Nucleus admission and a successful consumer claim describe run authority; they do not fence storage writers.

Ordinary adoption would need to state that every build, test, build script and proc-macro process that can touch the slot is launched under the adapter's ownership; no out-of-band same-user writer accesses it; and no descendant daemonizes, changes group/identity/cwd to evade supervision, retains a descriptor or writable mapping, or passes a slot handle past completion. Those are proposed workflow promises, not operator-approved policy. They are difficult to guarantee for arbitrary project code and do not become enforceable merely by documenting them. Treating any class above as out of scope requires Tom's separate ruling and a contract update before implementation or adoption.

A healthy release needs all of the following, independently:

1. The authenticated consumer terminal record and exact-run capture receipt/digests are durable and bound to the admitted run, source, selector and attempt.
2. A supported guardian proves every process with slot write capability has ended, or an OS-enforced fence revokes every such capability, including inherited/open descriptors, writable mappings, descriptor transfer and path reopen.
3. The daemon durably records `idle` before replying; a competing acquire remains refused until that commit.

Root exit, zero exit status, consumer completion, capture completion, vanished PID, empty process group, quiet scans and a test verdict each prove less than this. The two public mechanisms above do not discharge item 2 under the baseline threat model. Therefore this task recommends **no healthy-release mechanism** on native macOS and does not recommend permanent quarantine as the product design.

Interrupted, cancelled, restarted, daemon/client-crashed and lost-reply outcomes remain separate: persist `uncertain`, reserve the bytes, and refuse reuse and eviction. Reconciliation may preserve `uncertain`; neither a later clean scan nor an inferred terminal result converts it to `idle`. No contract relaxation is proposed.

**Signalling mechanism (not a completeness proof):** compose Contract 010's process-group and start-identity discipline with a slot-rooted file/cwd observer. Use it to deliver SIGTERM/SIGKILL to known members and to record what the host can see. It is useful for diagnosis and best-effort cancellation. It does **not** enumerate every descendant or revoke a write capability.

When the workflow is a Queue/Effigy-admitted host run, inherit that run's process group and start identity. Do not `setpgid` the Cargo/rustc tree out of the consumer group: that would look like an escape to Contract 010 and would change consumer process accounting. Oxide adds slot/checkout/capture/tmp path scans and extra escapee records; it does not replace the consumer supervisor.

When the workflow is a local adapter outside a consumer host run, the adapter creates the group with `POSIX_SPAWN_SETPGROUP`, records pgid plus start identity, and is the sole signalling owner.

`sandbox-exec` is rejected. An APFS volume can fail an unmount while FDs are open; that is a current-open-file observation, not a process-tree proof, and it does not stop a later reopen-by-path after remount.

### Proof obligation

For the public mechanisms reviewed here, **there is no proof that every writer has ended or lost write access**. `NOTE_TRACK` is unsupported. Process-group membership, start-identity environment, `proc_listchildpids`, `ps`, `lsof`, and cwd scans observe only processes they can still see. A descendant that leaves the group (`setsid` / new `posix_spawn` group), drops the identity variable, closes every slot/checkout/capture/tmp descriptor, `chdir`s away, and later reopens a remembered path is not in that set. The Endpoint Security descendant client is preliminary and excludes unrelated processes; its public write event is notification-only. The selected Nucleus planning+diagnostics selector also omits the observer tests that spawn extra process groups, so the candidate workflow does not even exercise the known extra-group case.

Therefore the observational conjunction (empty pgid + recorded escapees dead + quiet `lsof`/cwd + capture receipt) is **not** a containment proof. Using it as a `reconcile` success predicate can pass while that descendant is still live. That predicate is refused.

Distinguish:

- **Observation:** pid, pgid, lock file, `kill -0`, `ps`, `lsof`, unmount-busy. These locate some work and support signalling. Their absence is not idle and is not completeness.
- **Containment proof:** a record that accounts for every descendant that could still write. This host has no supported mechanism that produces that record.

**Current release disposition:** an ordinary healthy run may become `idle` only after the three-part proof above. No mechanism in this review supplies that proof, so this design is blocked before a normal release/reacquire qualification can run. If execution is interrupted, cancelled, the client or daemon crashes/restarts, or a reply is lost, the slot becomes or stays `uncertain`, reserves its bytes, and is unavailable for reuse and eviction. There is no observational path from those unknown outcomes to `idle`. Age, vanished PID, empty group, quiet scan, daemon restart, leader exit, capture or a test verdict never releases it. `reconcile` may confirm and persist `uncertain`; it must not transition to `idle` or `evictable`. Idempotent `(lease_id, reconcile_id)` still applies: identical evidence returns the same `uncertain` result; changed evidence is rejected.

Operator destruction of a quarantined tree or volume, without remounting that path for a new lease, stays outside this protocol as [architecture](architecture.md#durable-ownership-and-recovery) already states.

Capture evidence remains required for consumer settlement and does not make the slot idle. Oxide does not mint that receipt. A single-held-session Cargo measurement is not offered as a substitute for normal release/reacquire; it does not satisfy this task's outcome.

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

Nucleus's documented 2.3 GiB unique artifact sample for the planning+diagnostics pair used the line-tables profile; it does not establish full-debug size and excludes `disp_tmp` and two source checkouts. Preflight after the isolated cold run must measure the full covered set. If it exceeds 10 GiB, stop and report.

### 20 GiB host free-space threshold — settled

Tom ruled on 2026-10-09 to use 20 GiB as a monitored start/abort threshold, not a guarantee against unrelated host writers. [Architecture](architecture.md#budget-and-observations) owns the policy: refuse to start if free space is below the threshold or cannot be measured; sample during execution, name the interval and abort behavior, and report a detected breach or unavailable reading. The protocol must record expected sampling overshoot and unrelated-writer races. This threshold does not enforce or replace the separate 10 GiB disposable-footprint cap, and does not authorize APFS volume creation.

The remaining resource gate is the enforced disposable-footprint cap: include both fixture/checkouts as applicable, slot, redirected temporary output and captures; demonstrate the cap for every covered path; do not claim that sampled accounting is a hard cap. A separately authorized APFS quota volume is one possible enforcement boundary, not authority granted here. If the cap is not enforceable for the selected setup, do not start.

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

## Earlier path-omitting partition proposal (superseded; not authorized)

This proposal predates run 5 and omits checkout identity without normalizing
compiler source names. The portability design below supersedes it as the next
candidate. Its details remain here as rejected history, not current execution
authority or a second proposed mechanism.

Default compatibility key **includes checkout-path identity** (or Cargo's `{workspace-path-hash}` if redirection is ever left to Cargo). That default stays conservative.

The **previously proposed experimental variant** for a later pilot, pending the separate approvals below:

- One exclusive slot, one compatibility key with checkout-path **omitted**, used by both disposable checkouts A and B.
- Acquire records `partition_variant = experimental_cross_path` on the lease. A claimant without that variant, or with a different path-including key, is refused.
- Adapter sets `CARGO_BUILD_BUILD_DIR` to that slot path for A and B. It does not use `{workspace-path-hash}` for this variant. `CARGO_TARGET_DIR` and capture remain per checkout/run.
- Each scenario that builds in the shared slot must acquire, execute/tests, prove healthy finalization to `idle`, and reacquire for the next scenario. Scenario 1 uses a private isolated build directory, not the slot. Scenarios 5 and 6 must change the key or be refused; they do not silently stay on the experimental key. No scenarios share a still-held lease.

This path-omitting variant cannot run until Q-013 has a qualified healthy-release proof. Failure or uncertainty holds the slot; do not report a warm hit from a lease that was never safely released.

If scenario 3's path/proc-macro/generated-code evidence fails, stop cross-path claims, keep the slot path-isolated, and report reduced benefit. Path isolation does not meet the cross-worktree objective. Do not expand the matrix to chase a hit.

## Six-scenario evidence map

This is a **prior evidence map only**, not an executable brief. Execution requires a separately approved release/reacquire mechanism, exact paths, an enforced 10 GiB disposable cap and the monitored 20 GiB start/abort policy. Stay inside six worker scenarios, 90 minutes worker, 30 minutes independent reviewer and no automatic retries. Every real build runs the selected tests in that run; a cache hit cannot skip tests or mint a pass. Scenarios 2–4 require normal healthy finalization and a fresh acquire between runs. Lost reply, cancel, crash or restart leaves the slot uncertain and stops the session.

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
- Idempotent acquire/finalize after a deliberately dropped reply; because the outcome is unknown to the caller, the slot stays `uncertain` after launch and the session stops, not `idle`.
- Daemon or client kill during an in-flight build; restart leaves the slot `uncertain` with **no** observational `reconcile` success path to `idle` or `evictable`. Empty pgid, quiet `lsof`, or vanished PID must not release it. That is the interruption proof this host can actually run.
- Budget abort does not evict a launched slot. It refuses new acquire and turns the in-flight lease `uncertain`. Eviction still requires a separately demonstrated `idle` proof; age, a healthy-looking scan or a prior build does not supply it.

**Reviewer (30 minutes):** replay at most one warm (scenario 3) and one meaningful miss (4, 5 or 6), checking receipts, path strings, test execution, `disp_tmp` inclusion in occupied bytes, and that an interrupted run remains unavailable. Independent exact-head review of this design is separate and precedes any execution brief.

**Fixture / different-workflow cases** (do not expand the matrix):

- Path-absolute assertion: committed `build_diagnostics` is insufficient; capture binary strings as above rather than adding a Nucleus test.
- Observer orphan groups: present in full `qa:core:test`, not in the planning+diagnostics pair. Switching to `qa:core:test` would exercise extra process groups and still would not close the identity-dropping/path-leaving hole. Do not expand the matrix to chase a completeness proof this host cannot give.
- Generated code beyond serde/rusqlite: this selector does not exercise a workspace `build.rs` of Nucleus itself. That is a coverage limit, not a seventh scenario. Do not swap in another repo to chase proc-macro breadth.
- Descendant-complete release: would need a kernel tree (cgroup, supported sandbox, or equivalent) or a fixture whose processes cannot `setsid`/drop identity/leave path — Cargo/rustc/tests are not that fixture. Not a sixth-scenario substitute.

## Disposition

**Historical broad-model disposition; real-build pilot remains held.** The earlier design identified no supported macOS mechanism that proves healthy release for arbitrary descendants, transferred handles and unrelated same-user writers. The later cooperative contract explicitly excludes those behaviors and has an independently accepted native fixture implementation. That limited work does not prove a general fence or authorize a real Cargo/consumer pilot. Unknown outcomes remain unavailable; no permanent-held-session design or host installation is authorized.

| Gate | Status | Why |
| --- | --- | --- |
| Cooperative fixture release/reacquire | **Independently accepted within fixed fixture scope** | The review receipt above establishes owned-child completion, durable finalize/reuse and the supported uncertainty/refusal cases at its exact head. It does not qualify arbitrary commands or real Cargo. |
| Arbitrary same-user writer fencing | **Unsupported and unqualified** | App Sandbox's inherited static rights do not revoke a live child's access. Endpoint Security's documented write event is notification-only; neither reviewed candidate accounts for all writers under the historical broad model. |
| Interrupted/cancelled/crash/restart/lost reply | **Unavailable** | Keep the lease `uncertain`, reserve its bytes, and refuse reuse/eviction. Reaping a known fixture process later does not retroactively settle an ambiguous lease. |
| Fixture disposable footprint | **Bounded for fixed fixtures only** | Construction limits and path walks cover the task-owned fixtures, forced temporary directory, slot and captures. This does not provide a quota for arbitrary Cargo or production use. |
| Real Cargo 10 GiB disposable footprint | **Still a prerequisite** | A real workflow needs separately qualified enforcement for its full output tree; sampled accounting is not a hard cap. No volume or host setup is authorized here. |
| 20 GiB host free-space threshold | **Settled as monitored start/abort** | Follow [architecture](architecture.md#budget-and-observations). It is not a guarantee against unrelated host writers. |

**Next gate:** extend the accepted completion boundary to an explicitly approved real Cargo workflow and establish an enforceable full-footprint cap before measurement. The pilot still needs separate authorization, exact source/path/toolchain pins and path-correctness checks. Actual consumer adoption needs its owner-approved lifecycle mapping; the current held pilot has not been authorized to bypass that gate or substitute a standalone local binding.

### Missing prerequisites

1. The fixed fixture boundary has passed independent exact-head review. Any real-command extension needs its own implementation evidence and review; do not borrow the fixture verdict as Cargo qualification.
2. A separate real-build qualification brief with explicit execution authority, a selected native Cargo/Effigy workflow and consumer-owner coordination. The six fixture scenarios in this task do not authorize it.
3. An enforceable 10 GiB cap covering the real build slot, forced temporary output and consumer-owned capture paths, plus the settled monitored 20 GiB threshold. No volume or system extension is authorized.
4. Exact-head path-correctness and generated-code/proc-macro checks before any cross-path reuse claim; failed portability means path-isolated partitions and lower expected benefit.
5. Queue/Nucleus owner mapping for adapter identity, scheduling, cancellation, settlement and capture. Preserve frozen Contract010, Nucleus Contract002, and the existing pilot hold until that mapping is reviewed.

## Standalone Cargo pilot attempt: preflight and storage (2026-10-09)

Queue task 005 was dispatched under Tom's standalone Cargo pilot approval. This
section records what ran before any Cargo scenario. No Cargo build, test run or
slot lease was executed, so this section adds no reuse, rebuild, path or
benefit evidence.

Passed:

- Source pin: `bca75ed0143f96cd20b88ff72a421f1fb5f4ef19` and tree
  `67ecef91c907ca4085d94ea242b62f678c92993f` are present in the read-only
  Nucleus object store. The pinned `effigy.toml` defines
  `check:core:test` as `cargo test --offline {args}`.
- Toolchain: `/Users/tom/.cargo/bin/cargo` and `rustc` report 1.97.1. The
  rustup 1.96.1 install is present for the key-separation control.
- Offline inputs: `cargo fetch --offline --locked` and
  `cargo metadata --offline --locked` succeed in the disposable clone.
- Storage: task root `/private/var/tmp/oxide-cargo-8e87a74d` was fresh, mode
  0700. `hdiutil create -size 8g -type SPARSE -fs APFS` made
  `pilot.sparseimage`. `hdiutil attach -nobrowse` mounted `disk5s1` at `m/`.
  `diskutil` reported an APFS container of 8,380,178,432 bytes, about 7.8 GiB.
  Every disposable path, including the two clones, lives on that container, so
  the container's capacity bounds their writes. Shared `CARGO_HOME` is excluded
  existing state. Overhead, snapshots and backing-file growth are not measured.
- Clones: two `git clone --no-local` copies of the pinned commit, `m/a` and
  `m/b`, each at the pinned tree with a clean status.
- Host free space: about 752 GiB free on the boot volume (`df`) at setup. The
  20 GiB monitored threshold was not reached.
- Image lifecycle: the image was detached with ordinary `hdiutil detach` of its
  recorded device. The task root and sparse file are retained for a continuation.

Blocked before Cargo: the reviewed native completion path cannot yet qualify a
real Cargo tree. Current limits, verified by reading `src/project/native.rs`:

- `NativeCompletionCertificate::matches` accepts at most 16 free-space samples.
  At the 30-second interval that covers about eight minutes, so a longer
  build would be refused.
- The fixture budget walk allows 16 files, 1 MiB per file and eight children.
  Real Cargo output exceeds those bounds.
- The start identity is a per-spawn value echoed by the child. Cargo and rustc
  cannot echo an Oxide marker. An env-marker plus recorded process start
  identity would be a different proof and needs a narrow ruling.

Next gate: a ruling on the Cargo start-identity model and the sample/budget
bounds, then a Cargo-specific executor with its own review. Scenario 1 starts
only after that. The volume and enforceable cap are ready to use.

## Standalone Cargo pilot: runner landed, execution not run (2026-10-09)

Oxide now contains an experimental registered Cargo runner (`src/project/cargo.rs`)
and an ignored standalone pilot driver (`src/project/tests/cargo_pilot.rs`). Both
are unpublished and test-only in use. The runner launches the pinned Effigy
selector in its own process group, binds a per-spawn start marker, samples host
free space every 30 seconds, and issues a native completion certificate only after
a normal exit with no leftover group members. Any other outcome returns an error,
so the adapter keeps the lease uncertain.

Two deliberate choices:

- The storage completion bound of 16 free-space samples is unchanged. When full,
  adjacent samples merge to their minimum. Every raw reading stayed at or above the
  20 GiB threshold, or the run aborted.
- Trial-only partition identity is `PathIdentity::DisposableRoot`. Checkouts under
  one declared root share a key. Environment, toolchain and configuration digests
  still separate partitions. The default remains `PathIdentity::Checkout`.

Validation at this head: `effigy test rust` passed with 33 tests and 1 ignored
pilot test. The suite needs `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP` and `TEMP` set
to one fresh canonical directory. Without them, 27 reviewed fixture and storage tests
fail on environment assertions.

At this checkpoint, the six-scenario Cargo pilot had not executed: the run was
denied by the host auto-mode classifier, so no Cargo scenario ran and this
checkpoint recorded no reuse, rebuild, path or benefit evidence. Preflight
evidence is recorded above. The later run-5 record below supersedes this
pre-run disposition.

### Pilot execution record (2026-10-09, partial)

Launched through the approved Effigy two-separator command after standing approval. Three attempts ran past the launch:

- Run 2 stopped in scenario 1. The runner's 20 GiB check read the disposable APFS volume (about 7 GiB free), not host free space. No Cargo ran.
- Run 3 stopped in scenario 1 with the same check. The stale per-run temp directory from run 2 blocked `create_dir`. No Cargo ran.
- Run 4 completed scenario 1 with exit 0, against an empty build directory. Host free space was about 746 GiB. The reference took 129,943 ms wall time and rebuilt 47 units. It then stopped at the daemon bind: the storage root `m/storage` was never created.

Scenarios 2 to 6 did not run. The session was at roughly 65 of 90 minutes, so a rerun was not taken. Scenario 1 is cold evidence only. Its build directory now exists, so any rerun would measure warm. No reuse, rebuild, path or benefit claim is made.

### Pilot execution record, run 5 (2026-10-09, partial)

Run 5 used the verified two-separator launch on a fresh `m/storage` root. Run 4's scenario-1 directories were moved aside, not deleted. Results from `evidence/events-run4.log` and `evidence/events.log`:

- Scenario 1, cold reference, private build directory: exit 0. Wall time 94,323 ms. 47 units rebuilt.
- Scenario 2, slot cold at checkout A: disposition Finished. Finalization Idle. Wall time 87,704 ms.
- Scenario 3, warm at checkout B on the same slot: disposition Finished. Same slot. Same partition digest. Cargo reported 0 rebuilt units. Tests ran: 1 and 60 passed. Wall time 58,546 ms.
- Path check on scenario-3 binaries in the slot's `debug/deps` (read-only byte scan): `planning-*` contains 484 occurrences of the checkout-A path and no checkout-B path. `build_diagnostics-*` contains 56 A-path occurrences and none for B. This is a stale-path result for cross-checkout reuse, which the brief treats as a path-correctness failure.

Task 012 repeated that inspection read-only against the preserved binaries at
`/private/var/tmp/oxide-cargo-8e87a74d/m/storage/slots/slot-0000000000000001/debug/deps`.
The `planning-f7ede499516ce6dc` binary (87,494,872 bytes) contains 484 raw
byte occurrences of `/private/var/tmp/oxide-cargo-8e87a74d/m/a` and zero of
`/private/var/tmp/oxide-cargo-8e87a74d/m/b`. The
`build_diagnostics-2846f5df3c9bf798` binary (1,108,048 bytes) contains 56
occurrences of A and zero of B. These are physical checkout-A strings in the
actual preserved artifacts; they were not remapped to a logical prefix. This
confirms the prior strict-path failure and is historical pilot evidence, not
evidence from the task-owned fixture collector.

The driver stopped at its path proof. It looked in `runs/s3/target/debug/deps`, but test binaries live under the slot's build directory. The driver never reached scenario 4. Scenarios 4 to 6 and the controls did not run. No qualified reuse benefit is claimed.

Independent review at exact head `8e087796d29cbc7f201d31e3caf6e9ab7b9d84b2` accepted this partial negative qualification result; the [review receipt](https://github.com/inflatable-cookie/oxide/pull/9#issuecomment-6088502764) owns its verdicts and evidence. Cooperative Cargo completion, approved image/host-threshold bounds and local receipt binding passed for S1–S3. Cross-path correctness and adoption-grade measured benefit failed; invalidation was insufficient. The reviewer classified the embedded A strings as rustc source paths used for debug/panic metadata, with no `CARGO_MANIFEST_DIR` string or B load/rpath observed. This is not proof of wrong-source execution on identical trees, but it fails the approved strict path proof.

The lookup fix was checked without another pilot run. It logs path counts and continues rather than refusing stale paths; any future qualification driver must enforce its agreed portability gate. The trial key also uses a constant `disposable-root` component after checking containment, so it does not distinguish independent disposable roots. Neither behavior is qualified for production. Default keys retain checkout identity.

### Disposition of the partial pilot (2026-10-09)

This is an honest partial negative result for the standalone pilot. The original 90-minute experiment window expired before scenarios 4 to 6 and the in-session controls ran. No further Cargo scenario or reviewer replay is authorized under this disposition. The reviewer identified the observed A paths as debug/panic metadata; behavior-bearing paths in changed-source/generated-code cases remain unqualified.

Missing before any reuse claim:

- Scenarios 4 to 6 and the concurrent-refusal, identity-replay, uncertainty and idle-eviction controls. Scenario 5's configuration key separation and scenario 6's toolchain separation were never recorded.
- Source-content-miss evidence. The planned miss in checkout B never ran.
- Portable debug/panic source identity, plus behavior-bearing path correctness under a source change. Passing tests against identical trees does not qualify those cases.
- Completion coverage for the Cargo process tree. The runner records one process group leader and a per-spawn marker, not a full descendant census. Leftover members make the lease uncertain, but the process tree was not separately audited.
- Write and performance attribution. Occupied bytes and elapsed times are single-run observations on a shared host. Storage deltas are not write volume or SSD lifetime.

Artifact ownership: each run's captures, manifest, stdout/stderr digests and final binaries are kept under that run's own capture directory. Test binaries live under the slot's build directory.

Production keys keep checkout identity, leaving the duplication problem unsolved for production. The trial-only key has the independent-root limitation recorded above and is not a production safety guarantee.

Reviewer verdict subjects: cargo_completion, resource_bounds, cross_path_correctness, invalidation, evidence_binding and measured_benefit. The reviewer may return fail or insufficient where evidence is missing. Future portability remediation or a fresh experiment needs a separate bounded plan after review.

## Portable source identity proposal (design only, 2026-10-09)

### What the run-5 path result means

The reviewed byte scan found rustc source-name strings for checkout A in the
reused planning and build-diagnostics test binaries. Those strings are used by
debug information and Rust source-location metadata, including panic locations.
The scan did not show a B load path or rpath, and the reviewer found no
CARGO_MANIFEST_DIR string in these two binaries. With identical source trees,
the passing tests show that those tests executed; they do not show what a
changed source, generated input, or runtime resource path would do.

Keep these classes separate:

- **Compiler source identity:** debug information, diagnostics, file!() and
  panic-location paths. The last two are observable program values as well as
  debugger metadata.
- **Behavior-bearing paths:** CARGO_MANIFEST_DIR, OUT_DIR, paths used by
  include!, build-script or proc-macro inputs and outputs, generated code,
  and runtime resource lookup. Rustc path remapping does not rewrite arbitrary
  environment strings or generated contents, and does not change which files
  a build script or test opens.
- **Debugger lookup:** a logical path in debug information is useful only if
  the run can map it back to the exact source tree that owns the run receipt.
  This lookup has not been implemented or qualified.

The selected Nucleus tests do not cover every behavior-bearing path. Other
Nucleus tests use env!("CARGO_MANIFEST_DIR"); the reviewed pilot did not run
those tests or a changed-source control. Generated-code, proc-macro and
path-sensitive runtime behavior therefore remain unqualified.

### Two mechanisms

| Mechanism | Shipped support and fingerprint behavior | Coverage and limit |
| --- | --- | --- |
| **Compiler path remapping with an explicit root map** | The installed pinned rustc 1.97.1 accepts stable --remap-path-prefix and --remap-path-scope options; the exact compiler source lists both as stable. The rustc book describes best-effort rewriting of compiler output, including debug information and macro expansions. Cargo 1.97.1 stores Rust flags in its freshness fingerprint and marks a unit dirty when the flag vector changes. Its source omits these flags from the unit filename hash when remapping is present, but that does not omit them from freshness checking. A map with different FROM roots on A and B therefore misses. The same complete, ordered map containing both pre-authorized roots can be supplied at both paths; a newly added root changes the flags and causes a safe miss. | Keeps full debug information and maps compiler source names to one logical prefix. The default scope also remaps macro paths: file!() and panic-location strings become logical paths. That behavior change needs an operator ruling. Remapping does not normalize CARGO_MANIFEST_DIR, build-script environment, arbitrary generated strings or runtime resource paths. Rust's documentation also warns that remapping is best effort and does not rewrite Apple linker OSO entries. No debugger source-map behavior is assumed. |
| **Stable authorized compilation namespace** | Cargo 1.97.1 has no documented option that presents arbitrary physical worktrees as one stable filesystem source root. A namespace would need to present the authorized tree at the same path for Cargo, build scripts, rustc and test processes, then resolve that logical path to the current tree. | Could cover environment and runtime paths as well as compiler names if the namespace really preserves that view. A symlink or mount-based view, Cargo path canonicalization, process access and safe rebinding would need separate host support and qualification. That broader host/source-view contract and any installation are outside this task. It is not the first mechanism to pursue here. |

Cargo's [build.rustflags reference](https://doc.rust-lang.org/cargo/reference/config.html#buildrustflags)
documents the Cargo entry point for passing compiler options. Primary-source
checks for the shipped behavior were the installed rustc
--help -v, rustc 1.97.1 source at
[the pinned compiler commit](https://github.com/rust-lang/rust/blob/8bab26f4f68e0e26f0bb7960be334d5b520ea452/compiler/rustc_session/src/config.rs#L1866-L1881),
and the [rustc path-remapping guide](https://doc.rust-lang.org/rustc/remap-source-paths.html).
Cargo 1.97.1 source at
[the pinned fingerprint implementation](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/mod.rs#L1092-L1101)
checks changed Rust flags, while
[unit metadata construction](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/build_runner/compilation_files.rs#L836-L876)
explains why remap flags are treated specially for unit filenames. These are
shipped mechanisms, not evidence that the proposed adapter or source map
exists.

### Recommendation and operator decision

For the first narrow qualification, use rustc path remapping to a stable
logical source prefix, with one fixed mapping set containing every authorized
checkout root for that qualification. Register A and B before compiling at
either path. Preserve all effective Rust flags and Cargo configuration when
adding the map; if they cannot be resolved, refuse managed reuse. Bind the map
version and full mapping-set digest into the Oxide compatibility identity;
pass the same complete flag vector to both invocations. Never strip a root
path or remap flag from identity to manufacture a hit. If the set changes,
Cargo must see the changed flags and rebuild. This uses no source or build-tree
copy. It can qualify reuse among a finite, pre-registered set of worktrees; it
does not yet solve arbitrary newly created roots that arrive one at a time.

The experimental local adapter now resolves a finite ordered source-root map
and carries its policy and map digests with the complete effective Rust-flag
vector. Root IDs bind to canonical declared paths. Map or flag changes create a
separate partition; a missing effective flag vector, full-debug intent, or any
source/configuration/toolchain miss control refuses before a shared partition
is resolved. Logical mode requires the recorded accepted policy ID. Strict
physical mode keeps each requested root in its own partition.

The run receipt records the complete ordered map, chosen root and logical
prefix, compile-origin root/tree identities, requested source tree, slot lease,
binary digests and capture digest. The qualifier keeps debug-information,
compiler source, `file!()`, panic-location, and Apple OSO paths distinct from
manifest, include, build-script, `OUT_DIR`, generated, proc-macro and runtime
paths. Missing or unresolved evidence is insufficient; mismatches fail. This
mapping does not prove behavior-path correctness or debugger integration.
Small task-owned fixtures exercise these predicates only. The registered Cargo
runner forwards the effective remap flags and records map digests, but does not
provide the binary and path evidence required to qualify mapped reuse; such a
run remains uncertain. No real Cargo portability result is established. Keep
full debug info; do not use debuginfo=0, strip symbols, or remove path flags to
hide a failure.

**Registered-root cost in Queue's workflow:** disposable Queue worktrees
normally receive a new physical root per task. Because the complete ordered
root map and its digest are compatibility inputs, adding a worktree changes
the compiler flags and creates a cold partition for that map. Nucleus documents
about 2.3 GiB of unique artifacts for the planning+diagnostics selector with
the **line-tables profile**. This is a reference measurement only: the
proposal requires full debug information, and its per-map storage cost is not
measured. Do not use the line-tables figure as a full-debug estimate or
capacity guarantee. Each changed map still needs a cold partition; the
full-debug artifact and storage cost must be measured in a newly authorized
pilot. Until safe idle eviction, old partitions continue to occupy storage.
Evicting an old partition frees its measured space but discards its reuse
value; if that map is needed again it must be rebuilt. These profile-specific
partition costs are additional to checkouts, temporary output and captures.

The stable checkout pool is a **Queue-owned prerequisite**, not an Oxide
capability: Queue would need to keep a finite set of worktree paths and assign
future runs back to those registered roots. This workflow change needs Queue
ownership and a separate Queue brief; Oxide cannot provide or assume it. Reuse
can amortize registration and cold-build cost only when that pool is actually
reused across tasks. A stream of one-use roots keeps creating partitions and
can spend storage on old and new generations without reducing total work. No
stable Queue checkout pool is established by this design. A fresh qualification
must measure root-set reuse and partition occupancy. If Queue cannot maintain
the pool, stop this candidate for its ad hoc worktrees, retain checkout-
isolated partitions, and report that the dominant duplication problem remains
unsolved. Do not grow the root map silently or claim arbitrary-root reuse.

**Operator ruling, 2026-10-10:** Tom accepted logical compiler source names,
file!() and panic locations bound to each run's exact source tree, and the
Oxide-only experimental qualifier implementation. [Vision](vision.md#authority-boundaries)
owns the exact ruling and scope. This replaces the B-absolute-path requirement
for explicitly selected experimental logical mode; it does not qualify reuse
or authorize another real-project pilot. Behavior-bearing paths still need to
resolve and behave against the exact requested tree; any stale A resource path,
unresolved path class or missing source evidence fails closed. Production
partitions remain checkout-isolated.

A real-project brief must name the path gate before running. In **strict mode** (the
physical-path policy retained as an explicit option), pass only when every inspected
reference to this project's source in the reused B artifacts contains B's
exact absolute root and none contains A's root; third-party and toolchain paths
are classified separately. The full-debug scan and exact B source lookup must
be present, tests must execute at B, and the run receipt must bind the
compile-origin tree, requested B tree, slot lease and artifact digest; private
captures must bind to that B run. An observed A or other project-source root
is a failure; a missing artifact, scan or binding is insufficient. The
recommended fixed A+B remap emits a logical project path, so it cannot pass
this strict predicate; unless a mechanism preserves B-absolute paths, no
cross-path claim can pass strict mode.

In **logical mode**, use Tom's accepted exact-run source-map contract.
Real-project execution still needs its separately approved scope. Pass only when every inspected project-source reference in compiler
source/debug fields contains the agreed logical prefix and neither physical
root, the run-private mapping binds that prefix to B's exact tree and resolves
every inspected source to the expected B file, and `file!()`/panic-location
values match the accepted contract. An observed physical-root path, including
one inside either registered checkout, is a mismatch and fails; an absent
observation remains insufficient. If an Apple linker OSO entry is emitted,
it must satisfy the selected gate too; the documented rustc remap does not
rewrite it, so a physical A/B entry fails unless a separately supported,
identity-bound treatment is approved. A real producer must supply a path-behavior
evidence matrix showing that in-scope `CARGO_MANIFEST_DIR`, `include!`,
build-script inputs and runtime resources resolve against B, while `OUT_DIR`,
generated outputs and proc-macro inputs belong to the active leased generation
and reflect B's exact source tree. No behavior-bearing path may resolve to
stale A, and the selected tests must pass at B. An observed mismatch fails; a
required but unobservable path class or missing evidence is insufficient and
fails closed.
A logical-path result does not claim interactive debugger integration; if
that is part of the accepted contract, the pilot must stop until an exact-B
debugger source lookup is available and qualified.

### Registered Cargo source-map evidence schema

`src/project/source_map.rs` collects a run-private receipt from the resolved
map, partition, run binding, lease, private capture and typed workflow
observations. It computes binary and log digests from captured files, checks
relative references and canonical containment, and writes
`source-map-evidence.txt` with provenance, identity digests, observed classes,
paths and missing classes. The report is capped at 64 KiB; a collection can
name at most 128 binary artifacts, 256 combined observations, and 8 GiB of
captured binary data. It does not accept a caller's qualification verdict.
The owned completion certificate must bind the same run, lease, capture paths,
output references and log digests before the existing qualifier can pass.

Task 014 added a test-only producer in `src/project/source_map_probe.rs` and
`src/project/tests/cargo_source_probe.rs`. It reads Cargo JSON output, preserves
compile-origin rows, scans emitted binaries with the installed Apple tools, and
compares values printed by the executed fixture with independently hashed
requested-tree files. Task 015 repaired launch and ran one owned Cargo
invocation against that producer; the fixture build script then failed, so
the remaining sequence did not run. Both attempts and their limits are
recorded below.

The optional `CARGO_TARGET_DIR/source-map-observations-v1.txt` hook uses
UTF-8, one tab-separated record per line, and the version header below. Paths
are absolute observations except artifact paths, which are safe relative names
under the declared slot or target root. The identity row binds the accepted
policy, complete map, full build identity, requested tree and this exact run.
The collector parses at most 64 KiB and 256 records.

```text
oxide-cargo-source-map-observations-v1
identity <policy-digest> <map-digest> <build-identity-digest> <requested-tree-digest> <lease-id> <run-id> <attempt-id> <host-run-id>
origin <authorized-root-id> <tree-digest>
artifact <reference> <slot|target> <relative-path>
scan <reference> <scanner-version> <binary-digest>
section <reference> <debug-section>
source <reference> <compiler-debug|file-macro|panic-location|apple-oso> <observed-path>
behavior <class> <resolved-path> <source-tree-digest> <generation-lease-id|->
apple-oso <not-emitted|observed>
```

Each line shown after the header is a tab-separated record. The collector
resolves artifact paths under their declared roots, copies their bytes into
the private capture with create-new semantics, and computes the captured
digest itself. It derives full-debug completeness from the three required
section records; no `complete` flag is accepted. Source and behavior paths are
resolved against the requested tree and checked for canonical containment.
The task-owned producer writes this hook from its Cargo, scanner, and executed
fixture observations. The hook remains a plain file, so its identity row alone
does not authenticate the claimed observations.

**Trust boundary:** the hook is a plain file in the run's Cargo target
directory. Its identity row is compared with the adapter's accepted policy,
map, build identity, tree, lease and run IDs, then copied into the private
capture. That binds the declarations to this run but does not authenticate
which process wrote them or prove that a claimed compile unit, debug section,
source value, generated input or opened runtime resource was actually
observed. Artifact bytes and stdout/stderr are independently copied and
digested by the collector. The 014 producer binds its records to the owned
command's start identity and raw outputs, while the driver independently checks
the fixture values against requested-tree file digests. Those checks have not
run end to end, so genuine, independently validated Cargo observations are
still required before reuse can qualify.

In logical mode, origins from other authorized roots and trees are accepted as declarations; the qualifier does not independently establish which source contents produced a reused unit. Real qualification therefore needs genuine compile-origin observations and the source-miss control, not merely authorized root IDs or a matching hook identity row.

| Required evidence | Observation required by the schema | Collector and current coverage |
| --- | --- | --- |
| Compile origins | Producer-declared build-unit origin root IDs and tree digests. | The 014 producer parses Cargo artifact events and preserves unit origins in a slot ledger; the stopped run emitted no Cargo events or origin rows, so coverage remains insufficient. |
| Binary artifacts and full-debug sections | Producer-declared safe slot/target relative artifact paths, scanner version/digest, and individual `.debug_info`, `.debug_line`, `.debug_abbrev` section records. | The producer discovers Cargo artifacts, captures raw `dwarfdump`/`nm` output and derives section records. No artifact was emitted or scanned in 014, so full-debug coverage remains unverified. |
| Compiler debug paths | Producer-declared source paths extracted from binary debug information. Logical names must resolve to a requested-tree file; physical paths must match the selected physical-path policy and be contained. | The resolver checks requested-tree containment and the producer extracts paths from debug information. No scanner result exists from 014. |
| `file!()` values | Producer-declared values observed from the executed binary, kept as their own class. | The driver compares emitted values with independently hashed requested-tree files. The fixture binary did not run in 014. |
| Panic locations | Producer-declared panic locations captured from the executed binary, kept as their own class. | The driver checks panic paths separately from debug and `file!()` paths. No panic output was produced in 014. |
| Apple OSO | Producer-declared scanner result: `not emitted` or each emitted OSO path. | The producer records bounded `nm` output and classifies OSO paths. No binary scan was completed in 014. |
| Manifest directory | Producer-declared effective manifest directory and requested-tree digest. | The producer records the active manifest from the owned command. No command executed in 014. |
| Include input | Producer-declared exact file consumed by `include!` or related source inclusion. | The fixture emits and checks the included value against its requested-tree file. No fixture test ran in 014. |
| Build-script input | Producer-declared inputs consumed by the build script. | The producer correlates Cargo build-script output with the fixture's requested input. No Cargo build-script event was observed in 014. |
| `OUT_DIR` | Producer-declared generated directory and active lease. | The producer binds the generated directory to Cargo's build-script event and active lease. No build-script event was observed in 014. |
| Generated output | Producer-declared generated files and their active lease. | The fixture reports and checks its generated value under the active lease. No fixture test ran in 014. |
| Proc-macro input | Producer-declared proc-macro inputs and active generation. | The producer correlates the fixture's macro input and value with its build artifact. No proc-macro observation was produced in 014. |
| Runtime resource | Producer-declared file opened by a test/application and its requested-tree digest. | The fixture opens and reports its runtime resource; the driver checks the reported content/path. No fixture binary ran in 014. |

#### Task 014 execution result (2026-10-10)

The approved fixture tree was frozen at
`sha256:2dd89eba1d35184d65283e0eb28c008791f7ccc022879c9af0b28fb780e14132`.
After bounded setup, the first owned `reference-a` request acquired lease
`e0bd67897ed0b10eeb4a880d73fcb938`; `evidence/events.tsv` records
`run-stop\treference-a\tRejected` and that the lease was retained. Source
inspection identifies the pre-launch rejection: `probe_environment` includes
`TMPDIR`, while `CargoWorkflowRunner` rejects caller entries for variables it
sets itself. That check runs before its command temp directory, stdout/stderr
files and child process are created. The run capture and target directories
contain no outputs. The active lease was not settled or reused.

The resumed preflight recorded host free space of 461380890624 bytes, an APFS
filesystem capacity of 8380178432 bytes within the 8 GiB virtual image, 114688
bytes in-image, and 69632 bytes outside it. No resource threshold breach was
observed. This is setup evidence only; the first reference workflow did not
reach Cargo. Zero of the four approved fixture Cargo invocations completed,
and the remaining sequence was stopped with the lease retained. The mount,
private directories, raw evidence and lease remain preserved at
`/private/var/tmp/oxide-rust-probe-20261010`.

Verdicts from this run are **insufficient** for source invalidation,
cross-path correctness, actual Cargo/binary/scanner observations,
ownership/capture completion and reuse result. It does not establish whether
checkout-sensitive workspace units could be rebuilt while retaining reusable
dependency work; no artifacts were produced to inspect. That alternative was
not implemented. The producer code is present, but real reuse and storage
benefit remain unqualified.

#### Task 015 execution result (2026-10-10)

Launch correction: `CargoWorkflowRunner::validate_plan` now runs the
caller/identity contract before storage acquire or child creation.
`probe_environment` no longer supplies `TMPDIR`; the runner still forces
`TMPDIR`/`TMP`/`TEMP`, `CARGO_BUILD_BUILD_DIR`, `CARGO_TARGET_DIR` and
`OXIDE_CARGO_START_MARKER`. The non-ignored regression
`probe_plan_passes_runner_validation_before_acquire_and_rejects_forced_overrides`
exercises that real `validate_plan` path and still rejects caller `TMPDIR`
and `CARGO_TARGET_DIR` overrides. Probe identity is the workflow name
`oxide-cargo-source-map-probe`, not a Queue task UUID. Producer parser,
scanner and origin-ledger tests use representative Cargo JSON, malformed
input and a preserved ledger on fresh artifacts; they do not invoke Cargo.

Session clock file, written with `create_new` after retry-root/`setup-tmp`
creation and before image attach:
`/private/var/tmp/oxide-rust-probe-retry-20261010/setup-tmp/session-clock.txt`
records `start_utc=2026-10-10T13:23:06Z`, `deadline_utc=2026-10-10T13:43:06Z`,
`max_session_secs=1200`, `invocation_count=0`, `max_invocations=4`. Independent
review 6098048774 failed `session_clock`: that run started `Instant` and
stamped the file only after host free-space preflight, fixture freeze/verify
and retry-root setup, so those steps sat outside the 20-minute monotonic
window. The driver now captures `Instant` and machine start/deadline first,
persists those original stamps as soon as `setup-tmp` exists, then runs host
preflight and fixture freeze under the same Instant. The recorded 015 clock
file and image are unchanged; this session is not replayed. The 014 image and
lease `e0bd67897ed0b10eeb4a880d73fcb938` were not touched. Local labels for
this attempt were `oxide-local-probe-retry-20261010` /
`oxide-local-probe-image-retry-20261010`.

Observed fixture tree at A, B and the committed
`fixtures/cargo-source-map-probe` was
`sha256:2dd89eba1d35184d65283e0eb28c008791f7ccc022879c9af0b28fb780e14132`,
matching the frozen declaration. Manifest `head=d58e22439a8de0e939d30af541efe57ed4c175cb`
is the Oxide repository HEAD claimed for the local run; the fixture tree is
not a git checkout, so that head is a declared label, not an observed
fixture commit.

One genuine owned Cargo invocation ran: `cargo-invocation	1	reference-a`,
selector `test --offline --locked --message-format=json-render-diagnostics -- --nocapture`,
pinned `cargo 1.97.1 (c980f4866 2026-06-30)` / `rustc 1.97.1 (8bab26f4f 2026-07-14)`,
process 35989, elapsed 828 ms, exit 101. Lease
`c9d13410ea6450abbadef700f70fe727` on `slot-0000000000000001` finalized
`Uncertain` and remains held. Raw stdout/stderr, preflight observations,
scanner captures and `evidence/events.tsv` are on the still-mounted image
at `/private/var/tmp/oxide-rust-probe-retry-20261010`.

Exact compile failure from `stderr.log`: `probe/build.rs:21` formats a
`PathBuf` with `{input}` (`error[E0277]`: `PathBuf` doesn't implement
`Display`). Cargo compiled `oxide-source-probe-macro` (two rebuilt
proc-macro artifacts) and stopped the `oxide-source-probe` build script.
Fixture tests did not execute. Preflight scanners ran on
`liboxide_source_probe_macro-174fa59f4284649b.dylib` (`debuginfo: 0`);
`llvm-dwarfdump --section-sizes` reported total debug size 0. Probe
values `generated`, `proc_macro_input`, `file_macro` and `panic_location`
were missing. `scan_complete=true` records that those bounded scanner
commands finished, not that full-debug source lookup was present.

Resource samples stayed inside bounds: host free 342718029824 bytes at
mount preflight, APFS capacity 8380178432, in-image used 24576 then
4820992, backing allocation 15732736 then 20975616, outside 12288.
No 20 GiB host or 10 GiB disposable breach. Session elapsed 8696 ms at
stop. Remaining three invocation slots were not used; the existing retry
root forbids a second launch in this session.

The ignored probe test returned `ok` because `finish_if_idle` returned
`Ok` after preserving the uncertain lease. That helper now returns `Err`
so a later stop cannot look like harness success. The recorded 015 run
used the earlier helper; evidence, not the ignored-test status, is the
result.

Source invalidation, cross-path correctness and reuse were not reached.
This does not show whether dependency-only reuse or a stable
consumer-owned compilation path would help: the workspace crate never
built. The committed fixture as frozen cannot compile under the pinned
1.97.1 toolchain. A further attempt needs a new frozen tree, new root
and new session clock; this window and image are closed. No adoption.

### Bounded real-project follow-up (execution not authorized)

The Oxide-only experimental implementation is present in the local adapter and
qualifier, with small fixture coverage. It does not wire a consumer or install
a host service. The proposed real-project qualification below still needs its
own execution scope. A fresh pilot must preserve separate compile-origin and
requested-tree records and keep each final binary, stdout/stderr, test output,
manifest and receipt in that run's private capture. The slot retains only
reusable Cargo intermediates and their compile-origin evidence. Use the
existing reviewed lease, resource, local receipt and cooperative completion
machinery without widening it.

The brief must enumerate the behavior-path classes in scope for its selector,
identify the test or artifact evidence for each, and name any excluded class;
an unlisted class is not covered. Keep the consumer-facing source-identity,
miss and refusal semantics independent of rustc remapping and its flag/map
encoding, so a later supported Cargo mechanism can replace it without changing
Queue or Nucleus workflows.

A fresh Queue brief could qualify five ordered scenarios. Begin with all three
miss controls; do not begin the shared-slot pair unless each control passes:

1. **Source miss:** build the exact approved baseline tree at A into a private
   reference build directory and run tests; then apply one freshly approved
   marker change at B, reuse that directory, and prove affected units rebuild
   before running tests. Record both tree digests. Restore B only after the
   run settles and evidence is captured, with fresh restore authority, then
   verify B is exactly back at baseline.
2. **Configuration miss:** prove effective-configuration key separation or
   refusal without building.
3. **Toolchain miss:** prove toolchain key separation or refusal without
   building or installing a toolchain. Use already-installed 1.96.1 only if
   fresh preflight confirms it.
4. **Cold shared-slot run:** build the approved baseline at A in the exclusive
   shared slot and run tests.
5. **Different-path warm run:** run the same baseline at B in that slot, run
   tests at B, and require zero appropriate rebuilds, the preselected strict
   or operator-approved logical path gate, and exact-run captures.

Before authorizing scenario 5, the brief must record Tom's accepted logical
path contract and exact qualification policy. The fixed A+B map emits logical paths, so if Tom declines
that contract and no B-absolute-path-preserving mechanism is in scope, stop
after the miss controls; do not run the cross-path pair or report portability.

The complete A+B path map must be fixed before scenario 1 and remain identical
for every compile. The source, selector, exact toolchains, paths, map and one
marker patch require fresh source-owner and Queue approval. The candidate from
005 is Nucleus commit bca75ed0143f96cd20b88ff72a421f1fb5f4ef19, tree
67ecef91c907ca4085d94ea242b62f678c92993f, selector
check:core:test -- -p nucleus-core --test planning --test build_diagnostics,
Cargo 1.97.1 (c980f4866) and rustc 1.97.1 (8bab26f4f); re-pin and freeze them
again at approval. The old pilot's execution authority, image, slot and time
window do not carry forward. In particular, do not reuse or alter the 005
image or its contents.

Proposed fresh bounds are one new task-owned fixed 8 GiB APFS image, two
independent disposable checkouts, one exclusive slot, one private reference
build directory, and per-run private captures. The image's filesystem capacity
is capped at 8 GiB. The separate 10 GiB ceiling counts the entire disposable
footprint, including the image and any task files outside it; no data may spill
outside that footprint. Enforce both limits and stop at the first reached.
The fresh brief must count the sparse image's allocated backing bytes once,
plus task-owned files outside the image, under the 10 GiB measurement method;
it must separately measure in-image use against the 8 GiB capacity without
double-counting the same bytes. If the selected host cannot bound and reconcile
sparse backing allocation, refuse the pilot.
Retain the monitored 20 GiB host free-space start/abort threshold. Allow 90
minutes for the worker including setup and all five scenarios, then 30 minutes
for exact-head independent review; no retry, scenario expansion or budget
extension. Run tests on every scenario that builds. Stop at the first
miss-control failure, path-gate failure, uncertain ownership/capture or
resource threshold; preserve the slot and evidence. The independent reviewer
inspects the receipts and replays at most one warm run and one meaningful miss
within the 30-minute cap.

This scope can qualify only the selected workflow and pre-registered roots.
If the logical-path decision is declined, the strict A/B path requirement
continues to fail. If the registered-root map cannot be held identical across
the pair, if behavior-bearing paths cannot be shown correct, or if the
workflow needs a stable filesystem namespace, stop and return that precise
prerequisite rather than adopt path-isolated partitions as a solution to
worktree duplication.

## Preserved real-build pilot outline (not authorized)

This earlier outline remains a separate, unapproved real-build pilot. The five proposed portability scenarios above are a separate design proposal; they do not authorize Cargo performance measurement or a consumer run. Any later pilot needs its own Queue brief, host/consumer scope, and independent review. Do not inherit execution authority from the fixture task.

1. **Healthy release and reacquire:** one task-owned writer performs a bounded write, exits normally, and produces a capture receipt. The mechanism must prove no stale process or handle can mutate the slot; finalize durably; then a competing run reacquires and verifies the expected prior contents. Root exit or capture alone is an explicit negative control, not success.
2. **Competing ownership:** while leased, concurrent second claimants are refused without changing slot bytes. After a proven healthy release, racing claimants yield exactly one owner, and the durable ledger matches the winner.
3. **Cancellation, crash and restart:** interrupt the writer; separately terminate the client and daemon at documented boundaries; restart and replay the same operation IDs. Each ambiguous result stays `uncertain`, retains its budget reservation and refuses both acquire and eviction.
4. **Retained-handle and escaped-writer fixtures:** cover inherited and duplicated descriptors, writable mapping if supported, a child that changes group/identity/cwd, path reopen after apparent completion, descriptor transfer to an unrelated same-user process, and a separately started same-user writer. After healthy release, every stale write/reopen must be denied or isolated from the currently reusable generation. Any unexplained writer or unobservable handle fails the proof.
5. **Capture binding:** bind the fixture source commit/tree, selector and arguments, exact toolchain, checkout/slot paths, consumer/run/attempt IDs and output digests. Preserve per-run stdout/stderr and receipt independently of the intermediate slot. Capture completion never substitutes for writer completion.
6. **Review replay:** at most one healthy release/reacquire case and one retained-handle or interruption case, independently at the exact reviewed head. Reviewer challenges the release proof, event-loss/restart coverage and whether fixture behavior covers each claimed writer class.

For that separate pilot, count the entire disposable footprint, including fixture roots, slot, forced `TMPDIR`/`TMP`/`TEMP` directory and captures, against an enforceable 10 GiB cap. Require at least 20 GiB free at start, monitor at an interval no slower than one minute and at protocol boundaries, and abort/report if a reading is below 20 GiB or unavailable; record sampling overshoot and unrelated-writer races. Neither this outline nor the cooperative fixture task authorizes volume creation, system-extension installation, entitlement requests, consumer builds or pilot execution.

Queue/Nucleus keep admission, identity, settlement, retention and capture. Oxide owns slot leases and the proof that all writers have ended or been fenced; otherwise the slot stays unavailable.
