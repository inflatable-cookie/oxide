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

Task 017 adds an ordered `RelativeSourcePrefix` to the accepted logical map.
Each prefix names its authorized roots, Cargo compiler working directory and
package directory. The resolver checks normalized non-traversing paths, a
canonical package manifest under every named root, and the requested root's
actual workspace context. The complete ordered absolute-A, absolute-B and
relative flag vector and relative context contribute to policy, map and build
identity. Strict-physical mode rejects relative entries. The fixture declares
explicit `dev` and `test` profiles with `debug=2`, `strip=none` and
`split-debuginfo=packed`; these values are also part of effective environment
identity.

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
source <reference> <compiler-debug|file-macro|panic-location> <observed-path>
oso-object <artifact-reference> <captured-nm-reference> <absolute-object-path>
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
fixture observations. Apple `OSO` records are object/debug-artifact references,
not compiler source paths: every extracted absolute path is tied to its scanned
artifact and the captured raw `nm` output. The complete raw `nm` output remains
in the private capture. For a Cargo-reported test executable, the producer
selects its exact sibling `<executable-name>.dSYM` and captures every regular
file in that bundle under a bounded private name manifest; unrelated sibling
bundles do not make discovery ambiguous. It compares executable and
DWARF-payload UUID/architecture sets, hashes both originals and captures, and
checks the required DWARF sections before binding source-lookup evidence to the
companion digest. The exact bundle name is only a discovery rule and does not
establish a match by itself. The hook remains a plain file, so its identity row
alone does not authenticate the claimed observations.

The v2 baseline selected Cargo's exact `probe-<hash>.dSYM`, captured its three
regular files, and read the same dashed UUID and `arm64` architecture from the
executable and DWARF payload. The companion scan found `__debug_info`,
`__debug_line` and `__debug_abbrev`. However, `dwarfdump --debug-info` exceeded
the producer's 1 MiB scanner-output bound; the capture records the bounded
error and truncated output. The source extractor therefore did not complete,
`debug-companion` remained `bound=false`, and no full-debug logical source
lookup was established. This is the observed v2 layout and scanner limit, not
evidence that the companion lacked debug information.

The bounded project-source scanner uses installed `dwarfdump --show-sources`
and a selective logical-prefix `--name/--regex --show-parents` query instead of
full dependency/type DWARF dumps. For the fixed fixture test, a named-function
query selects the actual `DW_TAG_subprogram` and its declared project source,
ignoring same-name namespaces. Its parent compilation unit supplies the exact
`DW_AT_stmt_list` offset and logical working directory; a section-offset query
then requires the project's file/directory entry and nonzero source-line rows.
Missing, ambiguous, truncated or over-limit context fails closed. Raw outputs,
tool hashes/versions and exact argument records remain separate from runtime
paths and raw `nm` object references. UUID, original/captured hashes, full
companion capture and nonzero required sections still bind this observation.
This focused observation proves lookup of the required project source; it does
not claim complete enumeration of dependency DWARF types or source files.

The test-local v3 driver owns a single live `std::time::Instant` before any
experimental setup. Its create-new clock records session identity, the elapsed
millisecond origin and 1,200,000 ms deadline, original UTC audit stamps and zero
of four invocations. No prestarted/resumable clock or UTC reconstruction is
accepted. An independent 20-second timer and protocol boundaries sample host
free space and disposable allocations during setup, scanner and capture waits.
Samples retain UTC plus elapsed milliseconds in `setup-tmp/host-space-samples.tsv`;
a completed-sample gap over 30 seconds or any observation/write failure latches
a stop. Negative or uncertain paths preserve the new mount and image.

**Trust boundary:** the hook is a plain file in the run's Cargo target
directory. Its identity row is compared with the adapter's accepted policy,
map, build identity, tree, lease and run IDs, then copied into the private
capture. That binds the declarations to this run but does not authenticate
which process wrote them or prove that a claimed compile unit, debug section,
source value, generated input or opened runtime resource was actually
observed. Artifact bytes and stdout/stderr are independently copied and
digested by the collector. The 014 producer binds its records to the owned
command's start identity and raw outputs, while the driver independently checks
the fixture values against requested-tree file digests. Task 017 v4 observed these checks on baseline and changed-source runs, then
stopped at a real stale-source failure in the shared slot. The complete reuse
sequence has not qualified; independent exact-head review remains required.

In logical mode, origins from other authorized roots and trees are accepted as declarations; the qualifier does not independently establish which source contents produced a reused unit. Real qualification therefore needs genuine compile-origin observations and the source-miss control, not merely authorized root IDs or a matching hook identity row.

| Required evidence | Observation required by the schema | Collector and current coverage |
| --- | --- | --- |
| Compile origins | Producer-declared build-unit origin root IDs and tree digests. | The 014 producer parses Cargo artifact events and preserves unit origins in a slot ledger; the stopped run emitted no Cargo events or origin rows, so coverage remains insufficient. |
| Binary artifacts and full-debug sections | Producer-declared safe slot/target relative artifact paths, scanner version/digest, and individual `.debug_info`, `.debug_line`, `.debug_abbrev` section records. | The producer discovers Cargo artifacts, captures raw `dwarfdump`/`nm` output and derives section records. No artifact was emitted or scanned in 014, so full-debug coverage remains unverified. |
| Compiler debug paths | Producer-declared source paths extracted from binary debug information. Logical names must resolve to a requested-tree file; physical paths must match the selected physical-path policy and be contained. | The resolver checks requested-tree containment and the producer extracts paths from debug information. No scanner result exists from 014. |
| `file!()` values | Producer-declared values observed from the executed binary, kept as their own class. | The driver compares emitted values with independently hashed requested-tree files. The fixture binary did not run in 014. |
| Panic locations | Producer-declared panic locations captured from the executed binary, kept as their own class. | The driver checks panic paths separately from debug and `file!()` paths. No panic output was produced in 014. |
| Apple OSO | Producer-declared scanner result: `not emitted` or `observed`, plus one `oso-object` row per absolute linker-object reference. | Raw `nm` output is privately captured. Each extracted object path is bound to an artifact and its raw `nm` capture; it is not admitted as a requested-tree compiler source path. Task 017 v4 preserved raw `nm` output and completely bound/scanned the baseline and changed-source test companions; no OSO entries were emitted in those captures. |
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

#### Task 016 execution result (2026-10-10)

Interpretation for the next qualification: the observed relative compiler names do not establish wrong-source execution or failure of cross-worktree reuse. The [rustc remapping guide](https://doc.rust-lang.org/rustc/remap-source-paths.html#relative-paths) explains that Cargo can supply relative source paths and recommends remapping both relative and absolute prefixes. Any relative mapping must be explicitly tied to the registered workspace/component and its compiler working directory, included in the complete ordered flag/map identity before compilation, and resolve to the exact requested source tree; do not merely rewrite captured values or accept arbitrary relative paths. Likewise, empty debug sections in a Mach-O executable alone do not prove that full debug information was disabled: [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#split-debuginfo) documents macOS's default `unpacked` debug layout. Qualification must inspect and independently preserve the actual debug companion/object evidence, bound to the exact executable, or qualify an explicit supported full-debug `packed` profile. This corrects the proof setup; it does not establish that the captured run had complete debug information or useful reuse.

The owned fixture correction formats the build script's `PathBuf` observations
through `Path::display()`, retaining one literal path per line. The exact
`probe/build.rs` file is included as a module in the ordinary Rust test target,
so `effigy rust:compile` and `effigy test rust` typecheck the implementation
before the experiment. A non-ignored regression pins the corrected fixture
tree at
`sha256:09059f6e2df5022525ef60916178d84584a22e20b422c94950ddebd50bfa5a95`
and verifies that it is committed. The proc macro, generated source, and test
fixture had no other straightforward compile defects. `effigy rust:compile`
and `effigy rust:format` exited 0; the final ordinary `effigy test rust` run
passed 48 tests with 0 failures and 2 ignored. The ignored probe selector and
its Effigy/Cargo separators were confirmed with `effigy test --plan` before
launch.

The fresh session used
`/private/var/tmp/oxide-rust-probe-fixture-20261010`, a fixed 8 GiB sparse APFS
image mounted at `m`. The recorded monotonic window was
`2026-10-10T13:55:03Z`–`2026-10-10T14:15:03Z`. `events.tsv` records one
`reference-a` invocation of
`test --offline --locked --message-format=json-render-diagnostics -- --nocapture`
with Cargo 1.97.1 / rustc 1.97.1. The source tree at A, B and the committed
fixture matched the new frozen digest. Cargo exited 0 after executing the
fixture test (`1 passed, 0 failed`); six units were rebuilt. The manifest's
captured stdout/stderr digests and the three raw scanner-capture digests match
independent SHA-256 recomputation.

The emitted manifest, include, build-script input, generated output, proc-macro
input and runtime-resource paths and values matched A and the baseline marker.
With the complete ordered A+B remap flags present, however, `file!()` emitted
`probe/tests/probe.rs` and the panic location emitted
`probe/tests/probe.rs:54`, rather than the accepted logical path
`/oxide-source-map-probe/probe/tests/probe.rs`. The verifier therefore recorded
`values_match=false`. Separately, the final `probe` test executable had no
debug sections: its captured `dwarfdump --section-sizes` output reports total
size 0, and the captured debug-info/debug-line outputs contain only the Mach-O
header. No `.dSYM`, `.dwo` or `.dwp` files were present in the image. This is
missing required final-binary source-lookup evidence; `scan_complete=true`
means only that the bounded scanner commands finished.

Because the value gate failed, the local run could not settle lease
`a77955e9b31cd97b1547d7a894ad8e17` on `slot-0000000000000001`. The fail-closed
runner retained the uncertain lease and left the image mounted with all raw
captures. The outer ignored-probe test exited 101 after this refusal, even
though the owned Cargo test itself passed. Source invalidation, the shared-slot
cold-A/warm-B pair, cross-path reuse and useful storage reduction were not
reached. The observation is a concrete failure of the pre-registered logical
path predicate plus insufficient final-binary debug evidence; it is not a
positive reuse result.

Resource observations remained within the session bounds: minimum sampled
host free space was 321207545856 bytes; image capacity was 8380178432 bytes;
final in-image use was 14618624 bytes; sparse backing allocation was
31461376 bytes and bounded outside allocation was 12288 bytes. The run stopped
at 27257 ms after 1 of 4 allowed probe invocations. No uncertain lease was
reconciled, detached, cleared or replayed. The image and evidence remain at the
path above. Any further attempt needs a new scoped Queue record and fresh
resources. For ad hoc worktrees, checkout-isolated build directories remain
the safe fallback; they do not establish cross-worktree storage relief.

#### Task 017 execution result (2026-10-10)

The implementation registers relative compiler prefix `probe` against roots
`a` and `b`, with Cargo working directory `.` and package directory `probe`.
The ordered flags recorded before compilation were the complete absolute A,
absolute B, and relative `probe` mappings. The resolver regression checks exact
flag order, path-context digest separation, and refusal for traversal,
unregistered roots, duplicate prefixes and an incorrect workspace context.
`effigy rust:compile` and `effigy rust:format` exited 0. The final ordinary
`effigy test rust` run passed 52 tests with 0 failures and 2 ignored. The
corrected committed fixture tree was
`sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9`;
the run manifest bound repository head
`08b4354d15cfe79f8e41385e9baa6d750992d5cf` and that fixture digest.

The new bounded session used
`/private/var/tmp/oxide-rust-probe-paths-20261010`, an 8 GiB sparse APFS image
mounted at `m`. Its original monotonic window was
`2026-10-10T14:32:00Z`–`2026-10-10T14:52:00Z`; the evidence records one of four
allowed fixture Cargo invocations. The single `reference-a` command used Cargo
`1.97.1 (c980f4866 2026-06-30)` and rustc
`1.97.1 (8bab26f4f 2026-07-14)`, exited 0 after the fixture test passed, and
rebuilt six units. Its run elapsed 4334 ms; the whole session stopped after
14926 ms. The observed `file!()` value was
`/oxide-source-map-probe/probe/tests/probe.rs`, and the panic location was
`/oxide-source-map-probe/probe/tests/probe.rs:54`. The independently read
baseline marker, generated output, proc-macro input and runtime value matched;
the producer recorded `values_match=true`. These observations support the
logical behavior-path result with the complete map present, but do not isolate
the relative flag from the two absolute flags.

The full-debug baseline gate did not qualify. Cargo reported the final `probe`
test executable and a sibling `.dSYM` filename, but the scanner searched the
executable directory, found multiple sibling `.dSYM` bundles, and stopped with
`Cargo artifact has ambiguous sibling dSYM bundles`. It therefore did not
inspect or capture the Cargo-reported companion, compare UUID/architecture
sets, hash-bind the companion, or establish logical source lookup from the
captured debug data. The raw executable scans, partial `dwarfdump`/`nm` outputs,
preflight report, Cargo stdout/stderr, manifest and events remain in the image.
`scan_complete=false` and `debug-companion ... bound=false` are the recorded
outcome. This is a scanner setup blocker; it does not show that the compiler
omitted full debug information or that the cache reused wrong sources.

Lease `bcb8b00be80f5deb527a92b384033f52` finalized `Idle` after the baseline
run. Because preflight scanning was incomplete, source invalidation, measured
key controls, shared cold A and warm B did not run; no reuse or storage-benefit
claim is supported. The preserved evidence records host free space no lower
than 300283924480 bytes, APFS capacity 8380178432 bytes, mounted use 29020160
bytes at stop, sparse backing allocation 46141440 bytes, and outside allocation
12288 bytes. No 20 GiB host or 10 GiB disposable threshold was reached. Per-run
write attribution was unavailable.

The runner detached the image on this failed-but-idle path. The worker
reattached the same image read-only to inspect the stored evidence and left it
mounted; the backing image and captured files remain untouched. That negative
path must preserve the mount in a future session. The fixed root is now
occupied, the original session is closed, and no further fixture invocation
or clock reset was made. No existing 005/014/015/016 image or lease was
modified.

Task 017 dispositions: `relative_mapping` passed for the observed logical
`file!()` and panic paths with the complete flag vector; `baseline_tests`,
`launch_contract`, `session_clock` and `resource_bounds` passed. The required
`debug_capture_binding` and `baseline_path_proof` remain insufficient because
the companion scanner stopped on ambiguity. `source_invalidation`,
`cross_path_correctness`, `ownership_capture` for the full sequence and
`reuse_result` remain insufficient or not reached. This task does not establish
useful recurring-run reduction. Checkout-isolated storage remains the safe
fallback until a separately scoped attempt can inspect the Cargo-reported
debug companion and complete the source/reuse sequence.

The exact-head review found two proof-setup defects. Debug discovery scanned
all sibling `.dSYM` directories instead of selecting the Cargo-reported
executable's exact `<name>.dSYM`; the implementation and ordinary regression
now select that companion while ignoring unrelated siblings. The probe now
derives `CARGO_ENCODED_RUSTFLAGS` from the resolved source map's ordered
effective flags and checks those flags against the later mapped A resolution;
an ordinary regression asserts the Cargo encoding matches the resolved vector.
These code corrections do not change the original run's evidence or establish
debug binding, full-debug source lookup, source invalidation or cross-path
reuse. The later Queue-recorded v2 continuation result follows.

#### Task 017 continuation session v2 execution result (2026-10-10)

The continuation used the new root
`/private/var/tmp/oxide-rust-probe-proof-613d9b91-v2`, session ID
`oxide-probe-session-613d9b91-v2`, and the fixed clock recorded before setup:
`2026-10-10T15:19:29Z`–`2026-10-10T15:39:29Z`, four fixture invocations.
The independent committed fixture hash was
`sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9`.
The first selector attempt stopped before image creation because the stored
monotonic markers were increasing but their delta did not encode 20 minutes.
The marker file was not edited and the window was not reset; the guard now
reconstructs its monotonic timer from elapsed time in the unchanged UTC pair.
The next exact ignored-selector run started at 15:31:45Z, within that original
window, and used the committed head recorded by its run manifest,
`534204882e12c2230f24360cab8b93a3635cdcf2`.

Cold A was the only fixture Cargo/test invocation. Cargo 1.97.1 and rustc
1.97.1 compiled six units, the fixture test exited 0 with `1 passed`, and its
`file!()` and panic-location values were
`/oxide-source-map-probe/probe/tests/probe.rs` and
`/oxide-source-map-probe/probe/tests/probe.rs:54`. The manifest directory,
include/build inputs, generated value, proc-macro input and runtime value all
matched the baseline requested tree. The resolver-derived flag vector was used
for the build. The run lease finalized `Idle`.

The exact Cargo-reported test executable companion was selected. Its UUID
`27877A8A-EA00-3288-BD92-070FE9878F6F` and `arm64` architecture matched the
captured DWARF payload; all three required debug sections were present and the
bundle file hashes were recorded. The 1 MiB `dwarfdump --debug-info` output
bound stopped compile-unit/source-path extraction, leaving
`scan_complete=false` and `bound=false`. No full-debug logical source lookup
was captured. The probe stopped at this first baseline gate; source-change B,
shared-slot cold A and warm B did not run. The original session allows no
replay or root reset.

The image has 8,380,178,432 bytes capacity, 36,184,064 bytes in use at stop,
53,481,472 bytes allocated backing storage, and 20,480 bytes outside allocation.
The minimum recorded host free space was 240,012,070,912 bytes, above the
20 GiB threshold. Host samples were not maintained at 30-second intervals
during setup, so the resource-bound observation is insufficient despite the
observed free-space and allocation values. The session clock file also retains
its raw monotonic marker delta of 1,200,000,000, while the UTC interval records
the authorized 1,200 seconds; the implementation kept the original UTC
deadline and reconstructed a monotonic timer without changing the file. These
clock and sampling limitations are recorded rather than treated as passes.
After the settled lease, the runner detached the image; it was reattached
read-only at the same `m` path for inspection. The image and raw captures remain
preserved. No previous session image or lease was touched.

For v2, `relative_mapping` and `baseline_tests` are supported by the observed
flag vector and passing cold-A test. `debug_capture_binding` and
`baseline_path_proof` remain insufficient because DWARF source lookup did not
complete. `session_clock` and `resource_bounds` remain insufficient for the
recorded limitations. `source_invalidation`, `cross_path_correctness`, the
full `ownership_capture` sequence and `reuse_result` were not reached. This
run does not qualify cross-worktree reuse or useful storage reduction. The bounded scanner correction keeps the original output limit; any further
execution uses a fresh scoped Queue session and resources.

#### Task 017 continuation session v3 observations (2026-10-10)

The bounded read-only tool preflight on v2's accessible companion retained
147,604 bytes of new captures under
`/private/tmp/oxide-017-v3-preflight.p2d_a0kf`. Every scanner exited 0 within
10 seconds and below the unchanged 1 MiB command bound. The function query
also emitted same-name namespaces; the parser selects the actual subprogram's
CU offset instead. Raw function/parent context, selected line table, source
list, section sizes, UUIDs, tool version, arguments and executable/companion
hashes are recorded in the scratch manifest. These are authoring observations,
not retrofitted v2 receipts.

V3 ran the committed implementation at
`35a476b8b5b57c1311ad3d8912e4de4ffa1bb67d` under the new root
`/private/var/tmp/oxide-rust-probe-proof-613d9b91-v3`, session
`oxide-probe-session-613d9b91-v3`. The actual full commit is also recorded in
each run's private manifest. Its live `Instant` started before setup at UTC
`2026-10-10T16:04:08Z`, with elapsed deadline 1,200,000 ms and UTC audit deadline
`16:24:08Z`. The unchanged packed fixture digest was
`sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9`.
The driver's frozen-tree/committed-source guards succeeded before Cargo. An
additional independent Python prelaunch checksum attempt used string rather
than path-component sorting and failed its digest assertion; the shell still
launched Effigy. The corrected independent checker matched the digest after
the run, so that extra check is post-run evidence, not a prelaunch attestation.

Two of the four allowed fixture invocations ran, in order. Private cold A
rebuilt six units, executed the fixture test with `1 passed`, and printed the
accepted logical `file!()` and panic paths. Requested-tree values matched.
The exact packed companion's arm64 UUID
`48014E44-7CC2-38B9-9627-F66B88BCFE0E` matched its executable. Its independently
captured DWARF payload digest was
`sha256:9c3fb2b29441038d3d462bc6592f66238f37367c413ed242fbf4cd9543efd650`.
The raw project-subprogram/CU context and its selected line table establish
`probe/tests/probe.rs` under logical working directory
`/oxide-source-map-probe`; the map resolves that to the frozen A source.
All required sections were nonzero, and the complete private bundle manifest,
UUID, original/capture hashes and source lookup produced `bound=true` and
`scan_complete=true`. The baseline capture occupies 10,550,929 logical bytes.

The changed B marker produced tree
`sha256:784b4c701a7b752a11e0b5b99e7fff36f796af643cd3e8514ae16147b237ee26`.
B rebuilt three affected library/test units, its fixture test passed, and all
include/build/generated/proc-macro/runtime values were `changed-marker` at the
requested B paths. Independent file-byte checks recorded `values_match=true`.
Its test/companion UUID was `870AA164-EAF9-33FC-A4E0-1F6F54540EB5`; complete
companion binding and source lookup also succeeded. The capture occupies
10,550,742 logical bytes.

The driver nevertheless stopped at the source control: it required the
unchanged build-script *compiler artifact* to be non-fresh. Cargo reported that
artifact `fresh=true`, plus `build-script-executed` and the expected newly
changed generated/proc-macro bytes. That rejection is an overstrict harness
predicate, not observed stale-source execution or cache unsuitability. The
later correction requires an affected test rebuild, Cargo's bound output
directory, changed generated/proc-macro values and the existing independent
byte checks; an execution row alone cannot prove rerun because Cargo may emit
cached build-script output. Ordinary coverage includes a fresh script binary
with changed outputs, stale values, missing execution context and a wrong slot.
The new predicate was not executed in v3; its failed receipt remains unchanged.

Both leases finalized Idle. B was restored to the frozen tree only after its
private capture and settlement. The driver ended at about 16,380 ms; shared
cold A, warm B and cheap configuration/toolchain controls were not reached.
UTC/elapsed host samples were at 45, 1,786, 2,074, 11,798, 16,223 and 16,357 ms,
with a largest gap of 9,724 ms. The minimum sampled host free space was
249,785,610,240 bytes. At stop, APFS capacity was 8,380,178,432 bytes, mounted
use 48,685,056 bytes, sparse backing allocation 73,404,416 bytes and outside
allocation 16,384 bytes. No threshold was reached; write attribution remains
unavailable. The negative path preserved the image and mount. All earlier
session resources and captures remain unchanged.

The baseline tests and full-debug logical lookup are now observed, while
source-control acceptance with the corrected predicate, the full ownership
sequence and cross-path reuse remain unqualified. This is a stopped harness
qualification, not a negative mechanism decision or useful storage-reduction
claim. The ended live driver is not resumable; no clock reset, replay, third or
fourth invocation, or v4 session was started. Further live execution requires a
fresh scoped Queue session. The earlier
[review](https://github.com/inflatable-cookie/oxide/pull/17#issuecomment-6098812074)
applies to its recorded head; the changed producer and these observations still
require independent exact-head review.

#### Task 017 continuation session v4 observations (2026-10-10)

The fresh Queue-recorded v4 session ran at committed producer head
`e4daf01b7d7e5ba2a36148b9c0b0cb556f82a13a`, after strict RustQuality routing,
covering ordinary checks and the exact ignored Effigy plan passed. Before
launch, an independent Python hash used relative path-component ordering and
little-endian length framing, checked every fixture byte against that committed
head, and matched the frozen
`sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9`.
The hash and plan were checked in separate successful steps before launch;
there was no failed prerequisite followed by a compiler invocation. Authoring
captures are at `/private/tmp/oxide-017-v4-validation.ejavfjg9`, including
`verify-fixture.py`, `fixture-prelaunch.json`, `live-plan.txt`, `closeout.json`,
`live-v4.log` and the read-only inspection `v4-inspection.json`.

The driver owned one live `Instant` before setup. Session
`oxide-probe-session-613d9b91-v4` began at `2026-10-10T16:22:40Z`, with the
original audit deadline `16:42:40Z` and monotonic deadline 1,200,000 ms.
The session root is `/private/var/tmp/oxide-rust-probe-proof-613d9b91-v4`;
its image, mounted sources, slot and captures remain preserved. Earlier
sessions were neither mutated nor reattached. Three of four fixture
invocations ran; the ended driver cannot be resumed or supply another live
invocation without clock reconstruction. No v5 session is authorized.

| Ordered invocation | Actual observation | Settlement |
| --- | --- | --- |
| Private baseline A | Six units rebuilt; one fixture test passed; Cargo elapsed 2,884 ms. Logical `file!()` and panic paths, requested-tree behavior and full-debug project source lookup matched A. | Idle |
| Changed B | Three affected library/test units rebuilt; one fixture test passed; Cargo elapsed 1,023 ms. The unchanged build-script binary was fresh, while independently changed source/tree, generated bytes and build/generated/proc-macro/runtime values matched B. | Idle; B then restored and checked against the frozen tree |
| Requested shared cold A | The allocator reacquired the control slot, so this was actually warm reuse of changed-B intermediates. Zero units rebuilt; the test executed and failed; Cargo elapsed 207 ms, exit 101. | Uncertain; retained |
| Warm B | Not invoked after the preceding semantic failure. | Not reached |

For both successful runs, the exact packed `.dSYM` was completely privately
captured, UUID/hash-bound and source-scanned with nonzero required sections.
Baseline test UUID was `474982C8-990F-3333-BE90-6179C10116A3`, executable digest
`sha256:1122543c74d89b173b9455c267e9f2a60c1e60badbb61fd3506f1b14273498cf`
and debug-file digest
`sha256:e7429757aa1b6e65ec5055d90e526b88870d8fefecc8df53d3463473a9d579eb`.
Changed-B UUID was `2F149CFB-1B45-3D6C-8559-0FFBC95248FC`; its executable digest
was `sha256:11e5d59e7217c115a49372cab08692a1ad5ed2435795cc311f5f88c4dd5e8322`
and debug-file digest
`sha256:44781aea50b1efa076c10c25b358e4bc1f6ae717f30cb148233be762a2e4e083`.
Raw bindings, CU/working-directory/file/line queries, arguments and complete
companions are under `m/runs/{reference-a,source-miss-b}/capture`. The source
control and cheap configuration/toolchain key-separation controls passed their
actual predicates. No extra fixture smoke compile was used.

The third run is a concrete stale-source behavior failure after a qualified
baseline, not a scanner or compile setup failure. Its Cargo rows claim A and
`fresh=true` for all six units, but its privately captured executable and
debug-file hashes equal changed B. The fixture assertion at logical
`/oxide-source-map-probe/probe/tests/probe.rs:18` failed with runtime value
`baseline-marker` and compiled expectation `changed-marker`. Both requested
source markers are baseline bytes; retained generated output still names B
and contains `BUILD_VALUE = "changed-marker"`. Raw stdout/stderr hashes match
the private manifest. The test fails before emitting its normal observation
rows, so the qualifier conservatively reports missing behavior/compile-origin
evidence and Fail, and settlement becomes Uncertain. The image/slot must not
be reused, evicted, detached or released on this result. No repair or replay
was attempted after that predicate failed.

This does not establish the intended separate cold-A/warm-B pair: its first
shared invocation was already warm. It nevertheless demonstrates that this
observed shared-target transition accepted stale changed-source artifacts.
The candidate therefore has no qualified cross-path reuse or storage benefit.
Any next mechanism decision must preserve requested-tree behavior and consider
a narrower checkout-isolated boundary; an alternate mechanism is not
implemented here. Production remains checkout-isolated, and consumer source
path/lifecycle coordination and representative usefulness remain separate.

The independent timer also sampled during the live session. Completed
UTC/elapsed samples were at 59, 1,832, 2,102, 11,761, 16,008, 20,171 and
23,025 ms; the largest gap was 9,659 ms. Minimum sampled host free space was
240,210,411,520 bytes. Final sampled APFS capacity was 8,380,178,432 bytes,
mounted use 67,846,144 bytes, backing allocation 88,084,480 bytes and outside
allocation 16,384 bytes. No threshold was reached; write attribution is
unavailable. The driver stopped in about 23 seconds without resetting its
clock/count. No remaining-slot replay can run after its lifetime ended.

The [independent exact-head review](https://github.com/inflatable-cookie/oxide/pull/17#issuecomment-6099729811)
at `d8c2ded43197b7fefa4619f8f643fc134ce7d7bf` confirms the baseline mapping,
debug binding/source lookup, launch, clock and resource gates. It records
source invalidation, cross-path correctness and reuse as failures, and the
complete ownership/capture sequence as insufficient. Effigy `rust:compile`,
`rust:format` and `docs:check` passed; `test rust` passed 61 ordinary tests with
two live probes ignored. Compile retained 125 existing library dead-code
warnings. These checks do not qualify shared reuse.

Read-only inspection of the preserved v4 files recorded A's unchanged marker
mtime `1791649362229659522` ns, older than the changed-B build-script
fingerprint mtime `1791649372393953572` ns, with relative `rerun-if-changed`
input `marker.txt` and generated output still carrying B's changed value.
Task 018 pinned that chain against Cargo 1.97.1 (`c980f4866`) and the
allocator/partition code; see
[stale-source diagnosis](#task-018-stale-source-diagnosis-and-proposed-compile-origin-admission-2026-10-10).
Do not mutate retained fingerprints, touch consumer sources, or start another
live probe from these resources.

#### Task 018 stale-source diagnosis and proposed compile-origin admission (2026-10-10)

Design-only. No compiler, fixture, image, slot or timestamp mutation. Evidence
is the preserved v4 root
`/private/var/tmp/oxide-rust-probe-proof-613d9b91-v4`, author inspection
`/private/tmp/oxide-017-v4-validation.ejavfjg9`, committed producer
`src/project/resolve.rs` / `src/project/tests/cargo_source_probe.rs`, and
pinned Cargo 1.97.1 (`c980f4866`) fingerprint sources. This is not a generic
Cargo defect finding and does not claim that all cross-worktree reuse is
impossible.

##### Allocator, partition and compile-origin transition

Logical mapped mode builds `path_identity` as
`disposable-root:{id}:{root_digest}:{map_digest}` and omits the requested root
ID (`source_scope` is empty unless strict-physical). A, B and requested-A
therefore shared partition
`sha256:6d5af6571b28e2085d984887f1f98133cf69fb6311bf55ddb03638acb0151951`.
`execute_owned` acquires against that partition and sets the exclusive slot as
the Cargo build directory. After Idle settlement, the allocator reuses the
idle slot. Source-tree digest is recorded on the run binding and is not an
acquire admission check against the slot's last compile origin.

| Step | Requested root / tree | Slot | `slot_reused` | Cargo | Settlement |
| --- | --- | --- | --- | --- | --- |
| Private baseline A | `a` / `sha256:550e62ce…e2e9` | `slot-0000000000000001` | false | 6 rebuilt | Idle, lease `9be223bb…` |
| Changed B | `b` / `sha256:784b4c70…ee26` | same | true | 3 rebuilt | Idle, lease `7c94a4d8…`; B then restored to `550e62ce…` |
| Requested shared A | `a` / `sha256:550e62ce…e2e9` | same | true | 0 rebuilt, six `compiler-artifact` rows `fresh=true`, exit 101 | Uncertain, lease `ce5fc27e…`, retained |

Step 3 was warm control-slot reuse because the mapped partition is the same
for every authorized root and the previous lease was Idle. The intended
separate cold-A/warm-B pair was not reached.

Requested-A privately captured executable
`sha256:11e5d59e7217c115a49372cab08692a1ad5ed2435795cc311f5f88c4dd5e8322` and
debug-file `sha256:44781aea50b1efa076c10c25b358e4bc1f6ae717f30cb148233be762a2e4e083`
equal changed B. The test panicked at logical
`/oxide-source-map-probe/probe/tests/probe.rs:18` with runtime
`baseline-marker` versus compiled `changed-marker`. Retained
`debug/build/oxide-source-probe-6c594a47bb112b5d/out/generated.rs` still names
B (`BUILD_INPUT_PATH=…/m/b/probe/marker.txt`, `BUILD_VALUE="changed-marker"`).
Rustc dep-info still records
`# env-dep:CARGO_MANIFEST_DIR=/private/var/tmp/oxide-rust-probe-proof-613d9b91-v4/m/b/probe`.

##### Cargo 1.97.1 freshness inputs (proven)

Pinned Cargo fingerprints in the retained slot:

- Run-custom-build
  `.fingerprint/oxide-source-probe-6c594a47bb112b5d/run-build-script-build-script-build.json`
  (mtime `1791649372393953572` ns):
  `local: [{RerunIfChanged: {output: "debug/build/oxide-source-probe-6c594a47bb112b5d/output", paths: ["marker.txt"]}}]`.
  `output` mtime is `1791649372242345471` ns. A's `probe/marker.txt` mtime is
  `1791649362229659522` ns, older than `output`, so `RerunIfChanged` is fresh.
  Cargo `c980f4866` `find_stale_file` treats `path_mtime <= reference_mtime` as
  fresh, so equal timestamps are also a hit. B's restored marker is
  `1791649376357229122` ns, newer than `output`; a same-root request at B after
  restore would have been dirty. The failure is the cross-root request whose
  relative files are older or equal.
- Library/test units use `CheckDepInfo` with `checksum: false`. Encoded
  Cargo dep-info lists package-relative `marker.txt`, `src/lib.rs`,
  `tests/probe.rs`, `build.rs` and slot-relative `generated.rs`, with
  `nenv=0`. `translate_dep_info` in Cargo `c980f4866` drops rustc-synthesized
  env vars such as `CARGO_MANIFEST_DIR` because they are assumed tracked
  elsewhere. They are not.
- Fingerprint `path` is the workspace-relative target src path. Cargo 1.97.1
  fingerprints never hash absolute checkout paths so that renaming a project
  directory can reuse artifacts
  ([fingerprint module](https://github.com/rust-lang/cargo/blob/c980f4866/src/cargo/core/compiler/fingerprint/mod.rs)).

Official [rerun-if-changed](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-changed)
compares file mtimes to the last run. Unstable
[checksum-freshness](https://doc.rust-lang.org/cargo/reference/unstable.html#checksum-freshness)
needs nightly rustc, replaces rustc dep-info mtimes with checksums, and does
not replace build-script `RerunIfChanged` mtime comparison. It is not a
stable 1.97.1 fix.

Cargo still did the right forward miss at B: newer relative `marker.txt`
reran the script and rebuilt the three affected units. Relative fingerprints
inside one package root remain useful. They do not bind compile origin across
mapped roots.

##### Proven causes versus remaining limits

Proven:

1. Oxide admitted an idle mapped slot across different compile-origin trees
   and roots, then executed Cargo against those intermediates.
2. Cargo 1.97.1 treated relative `marker.txt` and relative rustc inputs as
   fresh because their mtimes were older than the B-written `output` /
   dep-info anchors (`path_mtime <= reference_mtime`, so equals are fresh
   too), and it stored no `CARGO_MANIFEST_DIR` in encoded dep-info.
3. The executed test binary and generated/proc-macro/runtime values were B's.

Not proven, and not claimed: that every cross-worktree reuse is impossible,
that Cargo unit invalidation is generally broken, or that touching sources or
editing fingerprints would be a supported fix.

##### Smallest fail-closed pre-execution contract

Compare two Oxide-only boundaries; do not adopt sccache, nightly checksum
flags, source touching, fingerprint edits, or post-execution path checks as
the admission gate.

1. **Compile-origin admission (recommended).** After a successful
   compile-through-capture, bind the idle slot to last compile origin:
   `requested_root_id` plus `source_tree_digest`, freshly computed from the
   requested source before acquire, inside the existing
   compatibility partition. Acquire selects an Idle slot only when
   `partition + requested_root_id + source_tree_digest` all match. It must
   not pick a different-origin idle slot in that partition (today's
   least-recently-used partition match in `store.rs` is the defect). No
   origin match is a miss: create a new exclusive empty slot in the same
   partition, or refuse mapped reuse and use a private checkout-isolated
   fallback. Never execute the mismatched intermediates, never touch
   sources, and never edit fingerprints. Same-origin retry remains a warm
   candidate and still needs Cargo's unit fingerprints plus
   full-debug/behavior checks. A later request for a retained prior origin
   may warm-hit that prior slot; that is not a hit on the mismatched slot.
   New origin slots and retained prior-origin idle slots both count against
   the existing budget. `make_room` may evict other proven-idle slots only;
   leased or uncertain state is never evicted, and budget exhaustion refuses
   the new slot ([Budget](architecture.md#budget-and-observations),
   [Q-003](questions.md#q-003--what-partitions-budgets-and-reconciliation-proofs-are-sufficient)).
2. **Tree-digest-only admission.** Allow a different root when the tree
   digest matches. That would admit same-content warm B, while retained
   `generated.rs`, `CARGO_MANIFEST_DIR`, include and proc-macro paths still
   name the previous root. The behavior-bearing path contract forbids it.

Recommend (1). Retained benefit: true same-origin warm retry and exclusive
ownership, with fail-closed source transitions. Cost: each distinct origin
may occupy another full-debug slot until idle eviction; mapped A/B hits are
refused, so ad hoc worktree duplication stays unsolved until a different
mechanism exists. Production keys stay checkout-isolated. Logical
compiler-name remapping may remain for debug/`file!()`/panic names; it is
not a reuse ticket.

The existing strict-physical adapter mode is a simpler reference: it already
puts requested-root identity in the compatibility partition. The proposed
origin gate additionally binds exact source content before execution. Neither
boundary shares compiled work between fresh physical worktree paths; under
the proposed logical contract, the common partition contributes centralized
budget accounting and logical compiler names, not cross-root warm hits.
Implementing and proving that safeguard alone would not satisfy Tom's first
foothold. A useful-reuse design must explain how changing tasks preserve
dependency work and bound storage without accepting another root's compiled
behavior paths or stale source inputs. Consumer-owned stable physical paths
and a separately qualified dependency-reuse boundary remain design options,
not selected implementations or permission to modify consumers.

Stable Cargo can keep relative unit invalidation inside one origin and can
cold-partition on rustflag/map changes. It cannot see older or equal
relative content at another package root, cannot treat synthesized
`CARGO_MANIFEST_DIR` as a freshness input, and cannot make checksum-freshness
a 1.97.1 build-script fix. Cross-root reuse therefore needs consumer-owned
stable paths, a content-addressed mechanism, or an operator decision to stop
remapping; those are outside this correction.

##### Proposed follow-up proof (not authorized here)

Fresh Queue session; no v4 attach, detach, replay or resource reuse.
Implementation remains a later scoped brief.

Envelope: 20 minutes including setup and the four compiler/test invocations,
then 20 minutes independent review. One new task-owned 8 GiB APFS sparse
image at `/private/var/tmp/oxide-rust-probe-proof-018-origin` (capacity
8,380,178,432 bytes). Separate 10 GiB disposable ceiling over image,
checkouts, slot, redirected TMPDIR/TMP/TEMP and captures. Monitored 20 GiB
host free-space start/abort threshold. Driver-owned monotonic
`std::time::Instant` from before setup, 1,200,000 ms deadline, max four
Cargo invocations, independent sampling at protocol boundaries and on a
20-second timer with a completed-sample gap of at most 30 seconds covering
setup, scanning and capture. Pin already-installed cargo 1.97.1
(`c980f4866`) and rustc 1.97.1 (`8bab26f4f`); selector
`test --offline --locked --message-format=json-render-diagnostics -- --nocapture`;
full-debug `packed` fixture profile; frozen fixture digest
`sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9`.
While a partition slot is leased or uncertain, a second claimant is refused
without touching slot bytes. Each run records rebuilt-unit counts, elapsed
ms, slot measured bytes, image used/backing allocation, outside allocation
and sampled host free space. Full-debug companion binding and project-source
lookup run on every successful compile. Stop and retain on first uncertain
lease, budget/host breach or missing evidence.

Acquire under the proposed rule: match `partition + requested_root_id +
source_tree_digest`. Record each acquire's slot id, origin and whether it
was a new slot.

1. Cold A at tree T0 / root A: new slot-1, bind origin A+T0, tests and
   full-debug lookup pass, generated/proc-macro/runtime paths are A's, Idle.
2. Warm A at T0 / root A: acquire must return slot-1, zero rebuilt units,
   hashes and behavior match step 1.
3. Changed B at tree T1 / root B: no origin match. Acquire must not return
   slot-1. Create slot-2 if budget allows after `make_room` of other
   proven-idle slots, else refuse mapped reuse (private fallback). B compiles
   only on that clean miss path and matches B. Record whether slot-1 remained
   Idle or was proven-idle-evicted. Leave root A at T0 with its original
   older marker mtime. Do not set A's `probe/marker.txt` equal to slot-2's
   `output` mtime: that timestamp is later than slot-1's `output`, so a
   same-origin retry of slot-1 would rebuild and destroy the warm-hit check.
4. Requested A at T0 / root A:
   - If slot-1 is still Idle with origin A+T0, acquire must return slot-1
     (same-origin warm hit, zero rebuilt units, hashes equal step 1, not
     step 3).
   - If slot-1 was proven-idle-evicted, this is a miss: new empty slot or
     private fallback; Cargo may cold-compile T0. Acquire must not return
     slot-2.
   Older timestamps are v4 evidence. Equal timestamps do not get a fifth
   compile or a same-root restore step. Acquire matches origin and tree
   digest before Cargo, so `find_stale_file` treating
   `path_mtime <= reference_mtime` as fresh cannot admit slot-2 for T0, and
   a same-root restore to T0 may hit only a retained T0-origin slot, never
   the changed T1-origin slot. Origin/digest admission is the safety gate; this proof
   does not show that Cargo itself rebuilds when mtimes are equal and
   content differs.

Success: (2) is a warm hit on slot-1; (3) never executes A's intermediates
as B and never returns slot-1 for B; (4) never returns slot-2 or executes B
artifacts; every successful compile has full-debug binding and
generated/proc-macro/runtime paths for the requested origin; exclusive
second-claim refusal holds; clocks, samples and resource caps hold.
Failure: any acquire of a different-origin slot, any execution of stale
artifacts, any origin-mismatch warm hit, missing debug/behavior evidence on
a successful compile, or a resource/clock gate miss. Uncertain ownership
stays retained. This proof cannot claim everyday storage relief or
cross-root usefulness.

#### Task 019 useful bounded reuse across changing tasks (design only, 2026-10-10)

Design-only. No compiler, fixture, image, slot, timestamp, fingerprint or
consumer mutation. Investigation used the named repository knowledge, the
reviewed PR18 diagnosis, pinned Cargo 1.97.1 (`c980f4866`) freshness already
recorded in Task 018, Queue Spec 028 / `worktree-cleanup.ts`, and Nucleus
Contract 002. The recovery/freshness correction also reads pinned Cargo's
[`dep_info.rs`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/dep_info.rs),
[`manifest.rs`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/manifest.rs)
and [`sources/path.rs`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/sources/path.rs).
The earlier correction read pinned Cargo
[`fingerprint/mod.rs`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/mod.rs)
build-script handling and the [change-detection](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection)
docs. This is not execution authority and does not replay the 018
four-invocation proof.

Tom's first foothold is duplicate dependency compilation and storage across
successive agent tasks with changing source. Exact-root/exact-tree retry is a
safety fallback; it does not satisfy useful reuse.

##### Two candidates

Start from existing strict-physical mode: the adapter already puts requested
checkout-path identity in the compatibility partition, so two different
physical roots cannot share a slot. Logical mapped partitions omit that root
ID; Task 018 proved that sharing them lets Cargo 1.97.1 execute stale
cross-root artifacts. Do not treat that mapped hit as storage relief.

| Candidate | Mechanism in pinned 1.97.1 | Useful-reuse result |
| --- | --- | --- |
| **(a) Bounded consumer-owned physical path reuse** | Keep one exclusive Oxide `CARGO_BUILD_BUILD_DIR` slot per registered compile root. After ordinary consumer archive, a new task/workspace/run/attempt occupies that same physical worktree. Cargo relative fingerprints and `CARGO_MANIFEST_DIR` stay at that path, so unchanged registry/path-dep units can stay fresh while changed workspace units rebuild. Strict-physical partition matching already keys the slot on that path. | **Recommend.** Different-source tasks at a recycled root can retain unmodified dependency work. Store count is registered roots times active compatibility partitions, independent of task count. |
| **(b) Supported dependency-only reuse boundary** | A private task-sensitive output tree plus a shared compiled-dependency store. | **No usable supported mechanism.** Stop the survey. |

Pinned Cargo 1.97.1 documents `CARGO_BUILD_BUILD_DIR` in the
[configuration reference](https://doc.rust-lang.org/stable/cargo/reference/config.html#buildbuild-dir).
The [Cargo 1.91 changelog](https://doc.rust-lang.org/cargo/CHANGELOG.html)
records the `build.build-dir` stabilization. The layout is internal and has
no documented split that keeps registry or path-dep units while isolating
workspace crates, build-script output, incremental state or test executables.
[Automatic global caches](https://doc.rust-lang.org/cargo/reference/config.html#global-caches)
cover registry/Git *sources*; Queue Spec 028 already shares those through
`CARGO_HOME` and keeps compiled outputs checkout-local. Candidate (b)
rejection rests on that 1.97.1 internal-layout fact. The 2026
cross-workspace cache goal is recorded in
[the landscape](rust-dx-landscape.md) as experimental direction, not a
supported 1.97.1 capability; this task did not re-fetch that goal page.
Unstable `checksum-freshness` does not replace build-script
`RerunIfChanged` mtimes. `sccache` is already rejected. Copying `deps/` or
reimplementing fingerprints is forbidden. A compiler wrapper is Nucleus
lead-1965, out of scope.

Candidate (b) therefore cannot be a next delivery on this toolchain. Reassess
only when a *supported* Cargo release documents shared compiled units with
build-script/proc-macro coverage and passes the same ownership, path,
evidence and budget gates. A roadmap is not that release.

##### Recommended next delivery: (a)

Use strict-physical identity. One exclusive slot per stable compile root.
Acquire matches `partition` (which already includes canonical checkout path /
`requested_root_id`). Compute `source_tree_digest` from the requested tree
*before* acquire and record it on the run binding; do **not** put it in the
acquire match. A different tree at the same root reuses that idle slot. A
different physical root is a different partition and a miss.

Task 018's `partition + requested_root_id + source_tree_digest` match is the
cross-root safety fallback. Shipping it as the useful-reuse admission would
create a new full-debug slot per head and fail this task's useful_reuse gate.
Keep it only as the documented fail-closed rule if logical mapped mode is
used at all; it is not the foothold architecture.

**How unmodified dependency work survives a different-source task.** Registry
and git dependency sources live under shared `CARGO_HOME` (already Queue
policy). Their compiled units live in the root's Oxide slot. A later task at
the same physical path, with a different workspace tree, leaves those
dependency sources and fingerprints unchanged, so Cargo can keep them fresh.
Workspace units whose inputs actually changed rebuild. Behavior-bearing
`CARGO_MANIFEST_DIR`, `include!`, build-script, `OUT_DIR`, generated,
proc-macro and runtime paths still name that same physical root, so retained
generated strings are not a cross-root lie. This is Cargo's in-origin
invalidation, which Task 018 recorded as working at changed B.

**Storage bound versus reduced writes.** Exclusive build-store count is
registered roots times the configured finite number of retained compatibility
partitions (profile, features, toolchain, configuration, environment),
independent of how many tasks have run. Superseded partitions consume the cap
until proven-idle eviction; a full budget waits/refuses. This count excludes
private-fallback directories: they require separate consumer-approved count/byte
limits and normal retention/cleanup, or fallback refuses. Concurrent mapped
workspaces cannot exceed the pool. Occupied bytes are still the full-debug slot plus that
root's checkout, redirected temps and per-run captures; that footprint is
unmeasured. Reduced *writes* happen only for units Cargo keeps fresh; a
recycled path whose workspace crates all change still rewrites those crates
and still saves dependency work. Do not promise zero rebuilds at a different
path, and do not promise zero rebuilds when every workspace file is rewritten
with a new mtime.

**What remains possible.**

- Same root, same tree: warm retry (control; not useful_reuse).
- Same root, different tree, ordinary git materialization: useful warm if
  unchanged dependency units stay fresh and requested-tree behavior holds.
- Different root, any tree: miss / private fallback (strict-physical).
- Consumer cannot recycle paths: duplication unsolved; path-isolated
  fallback preserves safety and does not satisfy the foothold.

##### Fail-closed freshness and compiled-path correctness

Graph and unit-invalidation authority stays with Cargo once a run is
admitted. Reuse of existing bytes and cold initialization of a proven-empty
owned generation are distinct admission modes. Oxide's job is to refuse
execution when Cargo 1.97.1 would treat changed inputs as fresh, and to keep
exclusive
compile-through-test-and-capture ownership.

Pre-execution gates, in order, after durable acquire and before spawn:

1. **Compatibility.** Toolchain, host/target, profile, features, effective
   Cargo configuration and allowlisted environment must match the partition.
   A change is a new partition or a refusal. Unknown relevant inputs refuse
   managed reuse (`input_unknown`); they are not omitted to manufacture a
   hit.
2. **Root.** Canonical requested path must equal the slot's compile root.
   Cross-root is a miss; never execute that slot.
3. **Source-tree digest.** Freshly hashed from the requested tree before
   acquire. Bind it to the run/attempt receipt. Same digest is a same-tree
   retry candidate. Different digest is still the same slot.
4. **Mtime-stale content (the Task 018 hole, now at one root).** Cargo
   `find_stale_file` treats `path_mtime <= reference_mtime` as fresh, including
   equals, and encoded dep-info drops `CARGO_MANIFEST_DIR`. A naive "any
   source mtime is old" refuse would also reject ordinary `git checkout` /
   `git reset --hard`, because unchanged files keep their old mtimes. That
   would destroy useful reuse.
5. **Supported gate for (4):** at acquire, record admitted_tree_digest
   `D_admit` from the requested tree (gate 3). Before every later reuse of
   existing bytes, including same-tree retry, check each stored input against
   every consuming unit's reference: the encoded dep-info file for
   `CheckDepInfo`, or build-script `output` for `RerunIfChanged`. Persist
   those unit/path associations and reference identities in the map. If the
   current mtime is `<=` any corresponding reference, its hash must equal the
   stored hash. For identity files and declared behavior files without a
   Cargo unit association, use the maximum reference across all allowed units;
   this is deliberately more conservative. Missing/unknown references refuse.
   A different tree digest alone is not a miss. Mismatch or unreadable
   current file with a stored hash: refuse *before* Cargo, settle the no-spawn lease with proof, keep the slot
   Idle with its previous origin, category `source_stale`. Newer mtimes rely
   on Cargo's supported input tracking, not this conditional hash check.
   Missing/invalid fingerprint/map files, unknown format, or uncovered input
   classes refuse reuse (`input_unknown`), with no execution of those bytes.
   Only the separate cold path below can proceed without an old map. Cap
   4096 files or 1 MiB, whichever first, applies to fingerprint-named
   workspace inputs, declared behavior-path files, workspace identity
   files (`Cargo.toml`, `Cargo.lock`, in-tree toolchain and `.cargo`
   config), and the recursive expansion of declared directories (a
   subset of tracked files plus any untracked members of those
   directories). Nucleus currently has 1003
   tracked files (140 `.rs`) and Oxide 46 tracked: that cap is expected to
   bind for those fingerprint-named subsets; if a workspace or directory
   expansion exceeds it, that slot cannot be reused.

   Persist or replace the bound map only on successful
   compile-through-test-and-capture finalize whose tree digest still
   equals `D_admit`. File hashes are of that admitted tree. Any other
   terminal outcome of a lease that spawned Cargo — compile error, crash,
   cancel, missing capture, failed finalize, lease loss, or Uncertain —
   must not persist or replace the map; mark it invalid so the next reuse
   is `input_unknown`. Successful rebind requires the cold recovery path
   below, followed by successful compile-through-test-and-capture with the
   admitted tree unchanged. A tree change during the lease is the same
   invalidation. Reconcile-to-Idle after Uncertain does
   not restore the previous bound map; the map stays invalid and the next
   reuse is `input_unknown`. A pre-spawn refusal does not spawn Cargo and
   leaves the previous bound map and origin intact.

   Cargo truncates the failing unit's fingerprint before rebuilding, so
   that unit stays dirty. The hole is other units that finished successfully
   inside the failed A1 run: their fingerprints/dep-info can now reference A1
   at `r1` while Oxide still holds map M0 bound to A0. An mtime-preserving
   restore of A0 could match M0 and be `<= r1`, letting Cargo retain those
   successful A1 units. Non-success invalidation closes that hole. Durably
   mark the map pending/unusable for subsequent leases before Cargo spawn;
   only successful finalize publishes a valid map, so a lost outcome cannot
   silently preserve M0 as usable.

   **Cold initialization and recovery (proposed, not authorized here).**
   Missing reuse evidence is distinct from missing ownership proof. An invalid
   map forbids reuse of existing bytes, not all future compilation at that
   owned root. Use whole-generation eviction/replacement, not surgical
   fingerprint edits or `cargo clean -p`:

   | State / evidence | Permitted transition | Result |
   | --- | --- | --- |
   | Proven Idle, valid map | Acquire exclusively and pass reuse gates | Existing bytes may remain Cargo-fresh across different trees |
   | Cargo spawned, non-success or changed admitted tree | Invalidate map; settle to Idle only with supported writer-completion and terminal/capture proof, otherwise Uncertain | No reuse of these bytes |
   | Uncertain, another lease, corrupt ownership/accounting, or unknown writer | Preserve bytes/reservation and refuse reuse/eviction | Reconciliation required; invalid map grants no cleanup authority |
   | Proven Idle, invalid/missing map, known owned path, explicit recovery authority and enforceable budget | Serialize whole-slot `make_room` eviction: durably record evicting, delete only that owned generation, verify deletion/emptiness before retiring its reservation; acquire a new exclusive empty generation within the same root/partition allowance | Cold admission; no untrusted bytes retained |
   | Missing recovery authority, capacity, empty-generation proof or input eligibility | Wait/refuse; interrupted deletion stays unavailable and reserved until reconciled | No forced release or extra generation growth |
   | Exclusively owned, proven-empty generation | Pass root/source/toolchain/config/environment, declared input/path eligibility and budget gates; compile/test/capture cold within that boundary and validate discovered coverage at finalize | Only successful finalize with unchanged `D_admit` establishes a new valid map |

   An empty generation needs no prior fingerprint/map. Emptiness replaces
   prior-byte reuse evidence, not full input/config/path checks. Eligibility
   comes from the registered trusted workflow's complete declared input
   boundary (the owned fixture for the first slice), checked before spawn.
   Unresolved/excluded classes refuse before spawn; discovery outside that
   boundary fails qualification and cannot publish a valid map. Cold generations
   and interrupted evictions
   count under existing slot/count/byte reservations. Replace only after
   deleting the predecessor; never create a parallel generation while its
   ownership is uncertain. Consumer source and private evidence are never
   deletion targets. Recovery loses dependency work once; later different-tree
   tasks can reuse the successfully rebound generation.

   This is Oxide-owned admission evidence, not a Cargo fingerprint edit,
   source touch, or emitted-path rewrite.

   **Workspace identity files.** Pinned Cargo `c980f4866` uses
   `Precalculated` / `pkg_fingerprint` for rustdoc and old-style scripts with
   no rerun-if directives. For path packages, `PathSource::fingerprint` /
   `last_modified_file` mtimes the package file list, which includes
   `Cargo.toml` and `Cargo.lock`. First slice excludes rustdoc and old-style
   scripts. Its ordinary compilations use `CheckDepInfo`; its new-style
   script uses `RerunIfChanged` / `RerunIfEnvChanged`. Those allowed units have
   no blanket manifest/lockfile mtime check. Under these exclusions:

   | Class | Cargo / partition coverage | Conditional map and limits |
   | --- | --- | --- |
   | Workspace/member `Cargo.toml`: compile settings | Cargo unit identities cover package name/version; fingerprint/profile/target/dependency hashes cover features, declared features, lints via `lint_rustflags`, profiles, target/edition, flags, rustc and resolved dependency identities; gate 1 binds effective config/environment | Include file in map. Old/equal-mtime differences refuse; newer changes rely on named Cargo/partition coverage |
   | Manifest metadata used by Rust `env!` / `option_env!` | `translate_dep_info` retains `# env-dep` keys selected by `ManifestMetadata::should_track` / `MetadataEnvs`: `CARGO_PKG_DESCRIPTION`, `CARGO_PKG_HOMEPAGE`, `CARGO_PKG_REPOSITORY`, `CARGO_PKG_LICENSE`, `CARGO_PKG_LICENSE_FILE`, `CARGO_PKG_AUTHORS`, `CARGO_PKG_RUST_VERSION`, `CARGO_PKG_README` | Cargo compares referenced metadata values independently of manifest mtime. This is not map protection; synthesized `CARGO_MANIFEST_DIR` is separately omitted and requires the unchanged physical root |
   | Other manifest fields/comments/unused keys | No universal content hash. Noncompile-relevant text need not rebuild; a script reading manifest bytes must declare `rerun-if-changed` | Map conservatively refuses old/equal-mtime differences. Newer undeclared reads are not covered; those workflows are excluded |
   | Workspace `Cargo.lock` | Resolved dependency identities/unit dependency fingerprints propagate relevant changes | Map protects old/equal-mtime text differences only. Newer text with unchanged resolution need not rebuild; a script reading lockfile bytes must declare that read |
   | In-tree `rust-toolchain` / `rust-toolchain.toml` | Gate 1 resolved toolchain and Cargo rustc hash | Present file is mapped; only old/equal-mtime differences refuse. Newer unused pins/comments are not compile inputs when resolved toolchain is unchanged |
   | In-tree `.cargo/config.toml` / `.cargo/config` | Gate 1 effective Cargo configuration/environment | Present file is mapped; only old/equal-mtime differences refuse. Newer non-effective text is not a promised rebuild |
   | User/global Cargo config under `CARGO_HOME` | Gate 1 resolved configuration | Out of tree map; unresolved relevant inputs refuse |

   Inventory identity-file presence at every reuse; missing/extra/unreadable
   entries refuse conservatively (`source_stale`, or `input_unknown` for
   unestablished coverage). Present files use the conservative maximum unit
   reference above. First-slice fixture has workspace/member manifests and
   `Cargo.lock`, no toolchain or `.cargo` config. Present files are mapped
   under the same cap. Metadata tracking covers values referenced in dep-info,
   not arbitrary manifest reads by scripts, proc macros or native tools.
6. **Generated / proc-macro / build-script / native / external.** Declared
   `rerun-if-changed` and rustc dep-info paths are resolved, then admitted
   to the map or refused, before spawn on a later acquire and before
   persisting a map at finalize.

   **Containment.** Each path is resolved to a canonical location against
   the package root. If that location is outside both the requested
   compile-root tree and the exclusive slot (`CARGO_BUILD_BUILD_DIR`,
   including `OUT_DIR`): refuse (`input_unknown`). Do not omit the path
   to manufacture a hit. Slot-owned generated files are in the map as
   slot-relative entries; they are not requested-tree files. A symlink
   whose follow escapes the requested tree and the slot is out-of-tree
   and refuses.

   **Directories.** Official
   [rerun-if-changed](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-changed)
   says a directory path is scanned recursively. Pinned Cargo `c980f4866`
   `find_stale_file` calls
   [`paths::mtime_recursive`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/crates/cargo-util/src/paths.rs)
   (`WalkDir` with `follow_links`), which takes the max mtime of the
   walk and does not hash members. Hashing only the directory path as
   one map entry would miss a child whose content changed with
   `mtime <=` the fingerprint reference — the Task 018 hole for
   directory inputs. Required rule: expand a declared directory
   recursively into per-file hashes under the same cap. Walk error,
   unreadable member, or an expansion that would exceed the cap:
   refuse (`input_unknown`); do not truncate the walk and do not skip
   members the way Cargo's walker ignores permission errors. At every later
   reuse, re-walk that directory: extra files,
   missing files, or hash mismatch on any member whose current mtime is
   `<=` the fingerprint reference → `source_stale` before Cargo.

   **Detectable unknown.** After a successful compile, parse the slot's
   build-script `output` (and fingerprint local entries). If any
   in-scope package's `run-custom-build` unit has zero
   `rerun-if-changed` and zero `rerun-if-env-changed` lines, do not
   persist a reuse map for that slot, and the next acquire refuses
   managed reuse (`input_unknown`) before spawn. Pinned Cargo
   `c980f4866`
   [`fingerprint/mod.rs`](https://github.com/rust-lang/cargo/blob/c980f4866141969fab6254a680546a277789d6f0/src/cargo/core/compiler/fingerprint/mod.rs)
   treats that case as old-style: an mtime change of any package file
   reruns the script; otherwise the fingerprint only tracks the listed
   rerun-if items. Official
   [change-detection](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection)
   docs match. The first-slice fixture emits
   `cargo:rerun-if-changed=marker.txt` (a file, not a directory), so it
   is eligible without directory expansion. Residual, not a refusal: a
   script that declares some rerun-if items but also reads an
   undeclared file, env var, or network resource. Oxide cannot observe
   those reads. First slice excludes such workflows. Later consumers
   admit them only with explicit owner/Tom acceptance that undeclared
   reads are out of coverage; they are not claimed hits and not
   `input_unknown` refusals. First-slice selectors also exclude rustdoc
   (doc/doctest modes) and build-script overrides. Unknown unit kinds/formats
   or unproven coverage refuse. Allowed-unit metadata is encoded dep-info
   paths/env dependencies, fingerprint unit/dependency identities, script
   `output` rerun-if file/env declarations and their reference files. This
   is a pinned bounded parser, not a universal graph/read tracker.
   `cargo clean -p` is not selected; whole-owned-generation cold recovery
   needs no surgical dependency-preservation claim.
7. **Runtime resource paths.** Same physical root, so retained paths are
   correct only if the new tree actually has those files. Content changes
   with newer mtimes rebuild or are re-read at test time; content changes
   with old mtimes hit gate 5.
8. **Post-execution tests are not admission.** A failing test never
   authorizes the slot as a hit. Always run the requested tests. Private
   captures bind this attempt, source digest, root, lease and artifact
   hashes.

Uncertainty: the digest map covers files Cargo named, declared behavior
paths, recursive expansion of declared directories, and the workspace
identity files in the table above, bound to the admitted pre-acquire tree
digest, persisted only after successful compile-through-test-and-capture
with that digest unchanged. A non-successful Cargo spawn invalidates the
map. Proven-Idle authorized whole-generation replacement can then initialize
cold and rebind; ownership uncertainty still refuses. Equal-mtime same-content
files are allowed (ordinary unchanged git files). Clock-step backwards on
changed content is refused. Named
dep-info/rerun-if paths outside the requested tree and the exclusive slot
refuse. Residual: undeclared extra reads by a script that also emitted
some rerun-if; a mid-lease edit that reverts before finalize (Cargo
`invoked.timestamp` is Cargo's mid-build protection). Replacement
criterion: retire the map when a supported Cargo release's checksum
freshness covers both rustc dep-info *and* build-script `RerunIfChanged`,
including synthesized `CARGO_MANIFEST_DIR`, recursive directory members,
and manifest/lockfile content, on the pinned toolchain, and a fresh
qualification passes the same gates.

Do not touch all sources, edit Cargo internals, rewrite emitted values,
ignore paths, or grow the root map.

##### Physical path reuse and consumer identity

A recycled path is still a new task, workspace, branch, run and attempt.
Oxide never creates, resets, selects, retains or cleans source. There is no
second compilation copy and no transfer of source ownership.

Existing facts, not proposed APIs:

- By reading of Spec 028 and `worktree-cleanup.ts`: Queue Spec 028 shares
  `CARGO_HOME` downloads, keeps compiled outputs checkout-local, and
  removes in-checkout `target` directories only after closeout proofs. It
  skips a `target_directory` outside the worktree and then blocks
  cleanup-complete. Point `CARGO_BUILD_BUILD_DIR` at the Oxide slot
  *outside* the checkout and keep `CARGO_TARGET_DIR` private *inside* the
  checkout: Spec 028 can delete per-run finals and will not discover the
  Oxide slot as a target. This design departs from Spec 028's rationale
  that "Cargo build outputs stay local to each task checkout":
  intermediates live in the Oxide slot outside the checkout; finals stay
  checkout-local via `CARGO_TARGET_DIR`. That departure needs Queue Spec
  028 owner / Tom approval. It is not a Queue change in this task.
- Nucleus Contract 002 keeps one workspace/branch/worktree identity per
  task; "multi-task workspace reuse is not part of the initial model."
  Failed or cancelled never-merged worktrees stay retained. Shared Git
  objects do not authorize shared mutable build directories. Bounded
  retention of regenerable build caches is in-contract, with Oxide named as
  the shared Rust cache owner. Sequential exclusive *path* reuse after
  ordinary archive is not an existing 002 API.
- Lead `4468cf49-0f5e-4a84-a746-535969c26ddf` remains a proposal, not
  owner-agreed API.

Minimum *proposed* consumer-managed-Git/lifecycle changes, each needing its
own owner approval:

1. Finite registered pool of canonical task-worktree paths per opted-in
   repository/workflow. Create a checkout only when a task takes that slot.
2. Assign a path exclusively to one live workspace. Recycle it only after
   ordinary archive, artifact preservation, and exact source retention prove
   the path empty of that workspace. No live checkout reset.
3. Materialize the new tree with ordinary git checkout / `reset --hard` /
   linked-worktree create, so content that actually changed receives a
   current mtime. Snapshot restores that preserve historical mtimes on
   changed content are ineligible and must private-fallback.
4. Dirty, unmerged, failed, cancelled or uncertain work keeps its path.
   It cannot fill the pool. The request waits, or uses a private
   checkout-isolated build with no reuse claim. Age alone does not recycle.
5. Authenticated task/attempt envelope for Oxide acquire, plus a
   pre-`host.run` return-to-pending on `busy`, as already proposed in
   [project adoption](contracts/project-adoption.md#stable-source-path-lifecycle).
   Frozen Contract 010 is unchanged. Q-012 remains open until Queue
   managed-Git/worktree owners and Nucleus 002/010 owners approve those
   bindings.

Without (1)–(4), Oxide can still qualify same-root sequential reuse on a
task-owned fixture path. Everyday Queue/Nucleus relief stays unclaimed.

##### Smallest next implementation slice and bounded proof

Oxide-only. Strict-physical. Existing fixture
`fixtures/cargo-source-map-probe` at frozen T0
`sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9`
and the already-observed marker-only T1
`sha256:784b4c701a7b752a11e0b5b99e7fff36f796af643cd3e8514ae16147b237ee26`.
No new fixture members. Retained dependency work is the unchanged
`oxide-source-probe-macro` path-dep (and any registry units if present). If
step 2 rebuilds that proc-macro, useful_reuse fails. Logical remap flags are
out of this proof.

Not authorized here. A later scoped Queue brief must name these resources and authorize the
whole-owned-generation recovery policy before dispatch. No live eviction or
recovery is authorized by this design task. Carry forward: one fresh fixed 8 GiB APFS sparse image at
`/private/var/tmp/oxide-rust-probe-proof-019-reuse` (capacity
8,380,178,432 bytes), 10 GiB disposable ceiling, monitored 20 GiB host
free-space start/abort, exclusive compile-through-test-and-capture, per-run
full-debug/test/evidence, driver-owned `Instant` from before setup with
1,200,000 ms deadline, at most four compiler/test invocations, 20 minutes
including setup plus 20 minutes independent review, independent sampling at
protocol boundaries and on a 20-second timer with a completed-sample gap of
at most 30 seconds covering setup, scanning and capture. Pin already-installed
cargo 1.97.1 (`c980f4866`) / rustc 1.97.1 (`8bab26f4f`); selector
`test --offline --locked --lib --bins --tests --message-format=json-render-diagnostics -- --nocapture`;
explicit target selection excludes doctests/rustdoc without changing the frozen
fixture. Full-debug `packed` fixture profile. One physical root `R`. Distinct
`(run_id, attempt_id)` per step. While a partition slot is leased or
uncertain, a second claimant is refused without touching slot bytes. Do not
attach, detach, mutate or author a v5 of any 005/014–018 image.

Acquire under the useful-reuse rule: match `partition` (strict-physical root).
Record slot id, requested tree digest, attempt identity, rebuilt-unit set,
elapsed ms, slot bytes, image used/backing, outside allocation and sampled
host free space. Full-debug companion binding and project-source lookup on
every successful compile.

1. **Cold T0 / attempt 1 at R.** New slot-1. Workspace units rebuild.
   Tests and full-debug lookup pass at R. Re-hash the tree at finalize;
   persist the input-digest map only if it still equals the admitted T0
   digest, and bind the map to that digest. Record presence of
   `rerun-if-changed=marker.txt` (a file) in the slot build-script
   `output` as eligibility evidence, and that every named dep-info and
   rerun-if path resolved inside the requested tree or the exclusive
   slot. Idle.
2. **Useful warm T1 / attempt 2 at R.** Different tree, new task/run/attempt
   identity, same physical path. Acquire must return slot-1. `oxide-source-probe-macro`
   must be Cargo-fresh. Affected probe library/test/build-script units
   rebuild as required for the marker change. Tests pass with T1 values.
   `CARGO_MANIFEST_DIR` and generated/proc-macro/runtime paths name R, not a
   previous sibling. Full-debug lookup of T1. Idle. This is the useful_reuse
   observation; same-source retry cannot substitute for it.
3. **Same-tree T1 retry / attempt 3 at R.** Acquire returns slot-1, zero
   rebuilt units, hashes equal step 2. Control only.
4. **Stale-mtime refusal, no compiler.** Restore T0 *content* for
   `probe/marker.txt` while forcing its mtime `<=` slot-1's build-script
   `output` reference. Tree digest differs. Acquire may return slot-1; the freshness
   gate must refuse before Cargo, leave slot-1 Idle with the T1 origin, and
   start no compiler. This does not consume a compiler invocation.
5. **Non-success then restore (compiler 4, then no compiler).** Re-materialize
   T1 at R (ordinary git checkout so T1 files have current mtimes). Overlay a
   syntax error on existing `probe/src/lib.rs`; no new fixture member.
   Distinct attempt identity. Acquire returns slot-1. Spawn the compile; it
   must fail before successful compile-through-test-and-capture finalize.
   The bound map must be marked invalid. If the slot is Uncertain, stop:
   map invalid and unavailable is the observation; do not restore or spawn
   further. If Idle: restore T0 content for `probe/src/lib.rs` and
   `probe/marker.txt` with mtimes `<=` their applicable recorded references
   (prior dep-info for the failing library; build-script `output` for the
   marker, possibly rewritten by a successful unit inside the failed run).
   Do not assume the failing unit created a valid new reference. A reuse
   request may acquire slot-1 but must refuse
   `input_unknown` before Cargo (invalid map), start no compiler, and
   leave the map invalid. Inspect the recovery decision without deleting or
   replacing: proven Idle plus scoped eviction authority and budget permits
   the cold transition above; absent a prerequisite, record the exact
   wait/refusal. There is no fifth compiler invocation. Completing cold
   recovery, tests/capture and successful map rebind is explicitly deferred
   to a separately scoped future proof. This step demonstrates invalidation /
   refusal and an inspectable transition proposal, not successful recovery.

Success: (1) is cold; (2) retains the proc-macro and executes T1, not T0;
(3) is a true same-tree warm hit; (4) never executes; (5) never executes
after the failed compile (invalid map, or Uncertain), and records cold
recovery eligibility or its exact owner/budget/input refusal; exclusive
second-claim refusal, clocks, samples and resource caps hold. Failure: a
new slot for T1, proc-macro rebuilt at (2), execution of T1 artifacts at
(4) or of A1 artifacts after (5)'s restore, any cross-root/logical-map
hit, missing debug/behavior evidence on a successful compile, a bound map
still valid after the failed compile, or a resource/clock miss. Uncertain
ownership stays retained. This proof still cannot claim everyday Queue
storage relief; that needs the consumer path pool.

If a later dispatch needs consumer contact, a different mechanism, more than
four compiles, or mutation of preserved probes, stop and obtain that ruling
before launch. Standing Oxide-only authority in [vision](vision.md#authority-boundaries)
covers an equivalent scoped brief for this slice; it does not cover consumer
edits, installation, or adopting the architecture as production.

##### Disposition

Recommend candidate (a). Candidate (b) has no supported 1.97.1 mechanism.
The next implementation is the Oxide freshness map plus the five-step
same-root proof above (four compiler invocations plus one no-compiler
refusal, with the fifth step using the remaining compiler for a
non-success then restore). The slice also implements cold-generation
admission / whole-generation recovery; a live successful post-failure rebind
is not proved within four invocations and requires a separately bounded
future proof. This is an inspectable design, not an unrun recovery success
claim. Consumer path-pool work stays a separate owner
brief under Q-012. Production keys stay checkout-isolated until that proof
and the consumer contract both pass. Path-isolated fallback remains the
safe default for ad hoc unique worktrees and leaves the duplication
problem unsolved.

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
