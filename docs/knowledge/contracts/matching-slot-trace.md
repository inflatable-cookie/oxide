# Scoped filesystem trace investigation (oxide#280)

**SOURCE-ONLY ALARM CORRECTION. Execution and merge remain held.** Planner
ruling `658b48d4` authorizes correction of the demonstrated alarm race and pure
boundary controls. No restaging, trace/probe/cost occurrence or privileged action
follows. All historical failures, omitted observations, closure uncertainty and
charges remain unchanged. Admission is unrestored; the cost proof is missing.

## Cooperative stop correction

The probe now checks a19s cooperative stop, leaving1s before its unchanged20s
hard self-alarm. Each polling sleep is the smaller of250ms and the positive
remaining cooperative time. At/past the stop, no sleep or next metadata read is
started. Cancellation/restoration follows the normal completion path. A blocked
or delayed call can still cross the hard alarm; that remains a `TimeoutError`
refusal, never normal completion. OS scheduling and blocked calls are not given
new guarantees by the1s margin. No deadline is enlarged or reset.

The existing observation helper still raises on the first changed/missing
identity; no additional creation, read or sleep follows that refusal. Shared
clock gate freshness, guard300s/512MiB bounds and shutdown margin, actual root
bindings, strict path parser, row caps and separate tracer custody are unchanged.
The hard20s alarm,120s readiness allowance and tracer30s+5s margin are unchanged.
This corrects only the private probe's normal cooperative completion boundary.

`effigy proof:matching-slot-trace:check` passed **152 pure checks**. Virtual-time
controls exercise alarm delivery enabled/disabled, a100ms final sleep, exact and
past cooperative boundaries, a delayed read reaching the hard20s alarm and an
identity change just before the cooperative stop. A hard timeout retains its
failed-read row and refusal. Existing admission, privacy, strict full/trimmed
path, identity and custody controls remain. No actual execution validates the
corrected bytes. Historical timeouts and unknown causation remain historical.

## Timeout and filtering diagnosis

The replacement at `8398f805` accepted the shared-clock gate (age34,957,750ns),
created three40-byte controls and recorded243 observations, retaining40 and
omitting203. Retained identities match; the receipt records `TimeoutError`.
The guard closed status1 without fault/deadline. Collector UNKNOWN/handoff and
later sudo wait0 do not establish privileged descendant closure.

The executed probe at `8398f805` sets its cooperative stop to20s, arms a20s self-alarm that raises
`TimeoutError`, and unconditionally sleeps250ms after each polling round. That
sleep can cross the alarm boundary before the next stop check. The outer
exception handler then writes a refusal even when retained metadata matches.
The diagnosis at `17bf616c` added pure controls of that probe function with virtual time and injected
signal delivery: the same unchanged metadata reaches20s and returns0 without
alarm delivery, but becomes a preserved `TimeoutError` refusal when the alarm
fires during the final sleep. This establishes a classification race in source.
It does not identify which instruction raised the historical exception: its
receipt has only the exception class, not a message, traceback or signal fact.
The retained matching rows also say nothing about the203 omitted rows.

Filtering accepts all three complete exact path shapes in pure controls,
including their dots, underscores and169/175/176-byte lengths. The parser has
no160-byte path cap. Retained filtered evidence contains719 events/279,449B:
639 name the122-byte challenge, and the longest retained pathname is159 bytes,
the probe directory itself. None names any of the three exact file paths.
Operations are669 `lstat64`,25 `stat64`,24 `getattrlist` and one `mkdir`.
Reported labels are695 `Python` and24 `Finder`; every retained `Finder` event
is an attribute query of a directory. These are process-label/operation facts,
not verified actor identities or evidence of file mutation. No thread suffix
is interpreted as a PID. No mutation is established, and none is ruled out.

The installed `/usr/share/man/man1/fs_usage.1` documents wider formatting with
`-w` and default left-side pathname trimming. It does not establish a concrete
wide-mode160-byte limit here. Pure controls show that simulated left-trimmed
versions losing the session prefix are rejected/unparsed or discarded, while
complete paths match. This preserves privacy and identity; the parser must not
adopt a filename suffix as an owned path. Raw discarded output was deliberately
never retained, so these facts support a trimming hypothesis but do not prove
why the three paths are absent. Unsupported format/operation, utility drops,
and incomplete sampling remain unresolved alternatives. Final counters include
798,971 unparsed lines and71 utility-warning/drop indications, so zero matching
file events is not absence evidence. No additional host/global trace was read.

Disposition of the original design proposals:

- The cooperative-stop correction above implements the alarm-race proposal
  in source only, preserving hard timeouts as failures and all deadlines.
- Consider a short exclusive child basename inside the already fresh bound
  guard evidence root, avoiding a repeated32-byte nonce there. A `p/` layout
  would give126/132/133-byte paths under this historical root, without suffix
  matching or new root authority. This only removes one candidate display
  limitation; it needs separately scoped source/preparation review and actual
  format evidence before any execution. No fixed display limit is inferred.

The diagnosis at `17bf616c` passed **140 pure checks**, including both original
timer-delivery outcomes and full/trimmed path controls, with the runtime kit then
unchanged. Those results stay bound to their source. The correction above adds
boundary controls; no production algorithm, policy, supervisor, acceptance rule
or historical receipt changes. The same reviewer judges the correction; no
proposal or passing pure control grants a retry.

## Shared clock correction

Gate issuance and comparison now both use the unchanged qualified guard's
`now_ns()` reader: integer `CLOCK_UPTIME_RAW` nanoseconds. Python3.9 macOS
`monotonic_ns()` has no shared-process-origin contract suitable for this gate.
The new gate explicitly names its clock and `issued_uptime_ns`; the freshness
window remains inclusive zero through3,000,000,000ns. Future or older timestamps,
wrong clock, malformed values and session/run/identity/hash/path mismatches
refuse. Old `issued_mono_ns` records are never converted, adopted or rewritten.
Collector-local elapsed observation timing remains local; guard deadlines,
120s readiness allowance,20s probe bound and shutdown margins are unchanged.

Probe receipts now retain a bounded `readiness` decision: accepted/reason,
clock and, for a valid clock comparison, issued/observed uptime and age.
Malformed/unreadable gates, abort before gate, readiness timeout and inadequate
guard shutdown margin have explicit refusal reasons. An accepted gate with
insufficient remaining guard time returns refusal before probe effects.

`effigy proof:matching-slot-trace:check` passed **123 pure checks** at this
correction. Injected differing process clock origins have no effect on the
shared-clock decision; exact3s, expired/future/wrong-clock/malformed gates and
persisted rejection reasons are covered. Existing stop-on-first-change,
privacy/bounds, custody, admission-context and nonmutation controls remain.
No actual clock/probe/trace experiment or restaging validates this correction.

The previous occurrence's gate was published, but its probe reported
`readiness_refused_no_probe_files`, zero rows, author/Effigy exit1. Its guard
closed status1 without fault/deadline. The collector exceeded30+5s observation
margin, retained UNKNOWN/operator handoff, then recorded sudo wait0 while
privileged descendant closure stayed unproven. The original gate contains
`issued_mono_ns=3681133083`; the contemporaneous comparison value and rejection
reason were not recorded. The clock-contract defect is source-proven, but its
causal role in that refusal remains **unknown**. No actor attribution or trace,
probe, cost, runtime-admission or closure credit follows this correction.

Same retained reviewer must judge the changed source and pure evidence.
The session and exact commands below are **historical, already used** and bind
older executable bytes. They are not staging or activation for this correction.
No further execution, cleanup, install or consumer QA is authorized.

## Historical preparation packet (already used; do not execute)

The fixed [kit](../../../scripts/matching-slot-trace.py) and
[pure controls](../../../scripts/test_matching_slot_trace.py) were staged from
pushed source `8a7b8c27596de7f90c269b732c56287627aa3634`. No/default/self-check
or prepare mode launches privileged/native work. The manual heavy selector is
not a queue proof execution or automatic dispatch; it is held for separately
activated operator use only. The existing guard is byte-identical and reused
as a supervisor and, only in later operator modes, as the known-PID identity
reader. No thread-to-process census or new signal authority is added.

Session identity: device **16777245**, inode **57996837**, UID **501**, mode
**0700**. `kit/` inode **57996838**, `runs/` inode **57996839**, same device/UID/mode.
Stash UUID was read from single-volume metadata and device identity was checked
against `/Volumes/Stash`; no pool, old fixture or account payload was scanned.
Staged sources are owner-only executable/readable; control documents are0600.

Historical preparation controls passed **96** checks, including privacy/redaction, strict
formats and thread-not-PID semantics, incremental/overlong/partial/capped
streams, warning/drop indications, wrong/stale session gates, path escape and
replacement, fresh/expired gate decisions, mocked nullable wait status and
foreground timeout handoff, separate guard/tracer outcomes, retained worker
context, missing hook/auth/socket, stale admission (including an empty marker),
wrong cwd/catalog/context, explicit executable/cwd launch, immediate post-close
replacement/disappearance, later changed/missing observations and nonmutation.
A late-change control verifies refusal facts survive the bounded row cap.
The first correction self-check had a test-source SyntaxError before controls
ran; its context-manager setup was corrected. The next90 controls passed, then
changed late-refusal retention controls brought the final total to96. They provide
no actual trace/probe/cost/closure evidence. The retained pure log and
`preparation-evidence.json` are in the session below. Actual format support,
root descendant identity/exit and syscall coverage remain unqualified.

Custody checklist for independent preparation review:

- Only OS `/usr/bin/fs_usage` with fixed30s argv may run privileged, via
  operator-authenticated ordinary sudo. No repository code as root.
- The retained Nucleus author alone starts normal heavy admission with its
  inherited authenticated hook unchanged. Entry rejects any EFFIGY_ADMITTED,
  absent hook/context, wrong task, changed hook/source or socket binding.
  Checkout cwd/identity, catalog SHA and executable sources are verified before
  launch, and Effigy gets an explicit exact cwd and bound executable. The child
  rechecks context and requires Effigy's marker; that advisory marker alone is
  never treated as a real grant. Actual admission still comes from the unchanged
  supported hook, with run identity bound in the private launch receipt. Tom
  starts only authentication and the collector; no hook settings are copied.
- Normal heavy admission/qualified300s512MiB guard reaches owned ready state
  before the tracer starts. Fresh challenge/gate is session/run/PID-start bound;
  no probe files before an owned-prefix metadata event is observed within5s.
- Probe has <=20s observation checks and a self-alarm; uninterruptible kernel
  calls may delay delivery. No expired-clock reset; guard remains independent.
- Root tracer job stays outside ordinary-UID guard custody. Record actual
  observable launcher PID/start and sudo wait status, never invent privileged
  descendant closure. Timeout/fault retains UNKNOWN and foreground human custody.
- Only parsed owned lexical paths and allowed operation/time/result/process/
  thread metadata are retained. Foreign same-line paths are omitted; raw global
  output is neither stored, hashed nor printed. Cap512KiB; lines4096B. Missing,
  malformed, dropped, truncated or ambiguous observations are inconclusive.
- At the FIRST changed/missing identity or creation refusal, stop before further
  probe creation or reads. Keep creation-FD and refusal facts, including the last
  refusal row when observations were capped. The independent fixed30s tracer
  may finish its already-running window. No recreation/adoption/overwrite,
  retry, cleanup, attribution by guess or cost QA. This supersedes the earlier
  informational suggestion to continue read-only polling after mutation.
- SAME retained reviewer must accept exact source, these operational files and
  pure evidence before planner separately brings Tom activation/readiness steps.
  No ready-to-merge, native credit, install or consumer QA follows preparation.

All nine workflow premises are traced in the kit module docstring: no Rust,
Cargo, account API/adoption, external subcommand, image or compiler flag effect;
only this explicit verified Stash parent, private controls and explicit temp
parents. Preparation itself creates no .DS_Store/probe files, guard or trace.

## Review copy of staged operator instructions

The following is the preserved historical review copy, not an activation request. The byte-identical
operator copy is `OPERATOR.md` inside the exact session path below; its hash is
`7947a4c1132ea31af4cc654b45c137604d2de11ba3962fbf0afe15cf0df103a9`.
Test-source SHA256 is
`192bf0f15ca4368f7f04d4ab1d1e3e73dfdbdae33eba2a9fa768ca5c4bcd43cb`.
Final pure-log SHA256 is `ee0ae547e108f10126a93496e7c9b715bab1e8ad2a7efb847bab11f4ef46a674`.
The session manifest also binds the exact checkout/catalog and hashes of the
inherited supported admission context, without copying settings or credentials.

Four earlier preparation directories with nonce prefixes55121f9d,e8b13b42,
c32ffbbd,ba8f4aaf remain retained and never activated. The session below was subsequently used for the one inconclusive occurrence.
Earlier unused sessions remain retained; no new session is prepared here. Review6668209287 is addressed by this source and restaging;
actual admission and trace/probe/counted evidence remain missing and held.

```text
# Oxide280: HELD pending exact-head review AND separate planner activation

Session: /Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11
Nonce: cb33170a187dee590b8105ef6c5e365f
Session manifest SHA256: 6c36f6bb1565e9c450eaf0c3caa4e9a4e97de8d90d16717d535eeabe1fc70dba
Source head: 8a7b8c27596de7f90c269b732c56287627aa3634
Kit SHA256: 20ccfa98d28b22be68956989de9558be41dc157d7a235a6f8baef9fbb9613cf4
Guard SHA256: fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b
Stash UUID: 8A660440-C919-45C6-A3F1-3365B9BD2006
Root-parent (later guard allocates one fresh child): /Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11/runs

Do not run these activation commands during preparation or review.
After reviewer acceptance, the planner must separately activate this ONE occurrence.
All code below runs as your ordinary user. Never run Python, shell, or kit with sudo.

Retained Nucleus author environment ONLY, after separate activation. This is
NOT a command for Tom's terminal. The retained author starts the nonprivileged
route with its original inherited authenticated Nucleus hook/context, unchanged.
Absent hook, inherited EFFIGY_ADMITTED (even empty), wrong task/context or cwd,
changed hook/catalog/executable sources are refusals BEFORE launch/guard effects.
Do not copy/export/unset/replace hook settings or admission markers to pass.
Exact checkout/catalog SHA256: /Volumes/Stash/queue/workspaces/16c152a62ccb87c9377238d7fe90d68a0a143604f0caaf9577efd2bdb18d5e9a/ns-b2c68e24-53b8-4f24-b4ee-ec18cb74a999 / 830bc940d68dc923ae4969f3dd0e7f40171740d44e5fd8a466a123cf191e394f
Inherited hook SHA256 (settings not copied): f67a0d478f69c37df946335d1d8d91058edaf5b7b016e1a72ffb8c03d24337cc
cd '/Volumes/Stash/queue/workspaces/16c152a62ccb87c9377238d7fe90d68a0a143604f0caaf9577efd2bdb18d5e9a/ns-b2c68e24-53b8-4f24-b4ee-ec18cb74a999'

'/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3.9' '/Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11/kit/matching-slot-trace.py' --operator-start '/Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11' 'cb33170a187dee590b8105ef6c5e365f' '6c36f6bb1565e9c450eaf0c3caa4e9a4e97de8d90d16717d535eeabe1fc70dba'
Admission may wait before any trace clock starts. Leave it foreground; do not retry.
Wait for `GUARD READY` naming actual root/evidence and the120s ready window.
No probe files exist yet. The retained author keeps the command foreground and reports GUARD READY to Tom.
Tom does not start heavy admission or the probe.

Tom's terminal, only after the author's GUARD READY message, authenticate interactively:
/usr/bin/sudo -v
Then immediately (within the ready window) start the unprivileged collector:
'/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3.9' '/Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11/kit/matching-slot-trace.py' --operator-collect '/Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11' 'cb33170a187dee590b8105ef6c5e365f' '6c36f6bb1565e9c450eaf0c3caa4e9a4e97de8d90d16717d535eeabe1fc70dba'
The collector launches ONLY this privileged command, once:
/usr/bin/sudo -n /usr/bin/fs_usage -w -f filesys -t 30
No raw trace reaches your terminal or any file. Do not pipe fs_usage yourself.
Collector requires a fresh session challenge event within5s of launch before
opening the probe gate. No readiness => no .DS_Store probe effects.
Probe observes metadata for <=20s after gate, within its cumulative300s guard
allowance (120s readiness maximum plus30s shutdown margin; no clock reset).
Probe also arms its own20s SIGALRM (no other process signalled); blocked kernel
operations may delay delivery, and that uncertainty is not hidden. The unchanged
300s guard remains the independent outer bound. Trace has30s timeout plus5s observation margin. No second trace or probe attempt.

Abort: press Ctrl-C in Tom's collector terminal (your terminal/sudo owns privileged cancellation).
Kit never signals the tracer or another process. The probe alarms only itself.
It writes the session abort indication and stops probe
work cooperatively. If sudo/job has not exited by35s, collector retains UNKNOWN,
prints its exact observable PID/start and stays foreground awaiting you. Press
Ctrl-C in Tom's collector terminal to request normal sudo interruption and wait for its exit. If it
still does not exit, leave the author command and collector terminal open and return the UNKNOWN receipt,
PID/start to the planner; do not use a pattern kill, detach, disown or another
root command. Root fs_usage descendants may not be independently observable;
a sudo wait status is recorded at that level, never invented root closure.
The retained author separately waits for real guard completion; tracer exit is NOT probe
closure and probe exit is NOT tracer closure. No cleanup of retained resources.

View ONLY bounded receipts (never a raw trace):
'/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3.9' '/Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11/kit/matching-slot-trace.py' --receipts '/Volumes/Stash/scratch/oxide/trace280-cb33170a187dee590b8105ef6c5e365f-c0ce2a11' 'cb33170a187dee590b8105ef6c5e365f' '6c36f6bb1565e9c450eaf0c3caa4e9a4e97de8d90d16717d535eeabe1fc70dba'

Privacy/limits: only structurally parsed ASCII metadata events naming this exact
fresh session prefix are retained (512KiB cap,4096B line bound). Other paths on
matching lines are omitted. Discard/unparsed/overlong/truncated/warning/drop
counters remain explicit. Unknown formats/truncation/missing events/identity
ambiguity are INCONCLUSIVE, not proof no actor exists. fs_usage TOD microseconds,
collector monotonic arrival and kernel PID/start identity tokens have different
bases; never order them as one causal clock. Process suffix is a reported THREAD
ID per local fs_usage manpage, not a verified PID. No historical holder claim.
Probe STOPS at the first changed/missing identity or creation refusal, before
any further creation or probe read; bounded refusal/descriptor rows are retained.
The independent fixed30s tracer may finish its existing window. No recreation,
adoption or second attempt. No cost fixture, installation, live readiness, QA or merge follows.
```
