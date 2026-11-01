# Proposed correction: shared proof supervisor (oxide#109)

**Status, 2026-10-21: not qualified; route B applies meanwhile.** The design (revision 4) passed prospective review: independent Sol verdict `4a98d7ac` at `29f0950e`, with minor implementation conditions. The implementation at `22d1280` failed closure and deadline review `6312725112` with five findings, and route B was applied ([vision](../vision.md#authority-boundaries)). Tom then approved one focused completion phase that fixes exactly those five findings with a regression each. Review `6318598212` of that phase found one more P1, a post-gate custodian exception escaping shutdown, and Tom approved a repair of only that. Review `6319610400` of that repair found its exception handler looking up traceback source (file I/O) before latching. Under Tom's continue-until-qualified ruling (decision `561298b5`) the owner admitted that localized fix. Review `6320463419` then found readings stamped at send time rather than completion, and the custodian's own setup doing blocking filesystem work outside the deadline. The owner admitted both fixes the same way. The supervisor qualifies only on a passing independent exact-head native. The body below is the reviewed text; the [shared proof supervisor](workflow.md#shared-proof-supervisor) section records current behaviour and implementation choices.

This is revision 4. Under Tom's 2026-10-21 ruling on Queue decision `bc874ae7` ("Yeah
do it"), it is a final, narrow round that changes only the four P1s from the
review of revision 3 (`c8ab934f`):
1. the launch gate passing before the child's `setsid`;
2. fault, reap and command-status precedence;
3. the per-interpreter monotonic origin;
4. start-time exclusion.

**Stop rule:** if review raises only minor points, they are fixed during
implementation without another design round. A new P1 class switches elevated
tasks to the documented-gap route (B), with no further design rounds.

The baseline being corrected is `scripts/bounded-test.py` at `f5a998c`. This
design keeps that baseline's guards:
- the reserve fault on staleness after 1 s;
- the scratch fault on staleness after 10 s, with its monitored budget;
- bounded output draining, with discard past the forward limit;
- a pipe still held open by a writer counts as evidence against closure.

All earlier findings, deviations and failed evidence stand as recorded.
Nothing here qualifies anything.

**Amended by oxide#195 (2026-11-01).** This reviewed text is the historical
design, and its "only pinned children are ever signalled" contract was
re-opened by oxide#182's packet, which ended `UNCLOSED` with a live
Cargo-shaped group inside the leader's own cooperative session. Tom's
2026-10-31 signal-custody ruling (Queue decision `c096c8fc`) settled it: a
same-user process that is not the runner's child is not held unreaped, so a
merely observed non-child PID is never signalled, and the supervisor has no
path that converts an observation into a signal. Current behaviour is the
[shared proof supervisor](workflow.md#shared-proof-supervisor) section's
*Cooperative-session survivors* description: after the leader is reaped the
monitor records the survivor's exact row in the receipt's bounded `survivors`
list and signals nothing. A recorded row is not a verdict: a transient survivor
may be gone by the deciding census, so the run then closes and the summary says
so, while a survivor still present keeps the run `UNCLOSED` with its scratch.
Read the table rows below about "survivors" with that amendment: a survivor is
recorded and left to a boundary that can hold it, never signalled by this
runner. This is observation-only custody hardening; closing the historical
Cargo-group survivor is deferred to Queue papercut
`9ff7386d-c70e-49c7-880a-2e6c676b2eb9`.

## Contract

Pinned-leader cooperative closure (Tom, "Yes, (2)", Queue decision
`99ed1d7f`). The only numbers ever signalled are children that the custodian
holds unreaped: the proof leader's group and the monitor. Survivors and
uncertainty end the run `UNCLOSED`, with scratch kept. The cooperative
observation boundary stands, so no claim is made about completeness for
arbitrary descendants.

## Actors

- **Custodian.** Single-threaded. It owns the absolute deadline, both pins and
  every signal. After setup it makes only bounded system calls:
  - `fork`, and `kevent` with a zero or remaining-budget timeout;
  - `waitpid` with `WNOHANG` only;
  - `killpg` or `kill` on a pinned child only;
  - `select` with a remaining-budget timeout;
  - non-blocking reads and writes on its control pipes.

  It never touches the filesystem or stdio after setup.
- **Monitor.** The custodian's first child. It owns all I/O: the evidence
  directory, receipt phases, output draining, reserve and scratch sampling,
  the census and cleanup. It sends timestamped messages to the custodian over
  a non-blocking control pipe.
- **Leader.** The custodian's second child. It becomes the proof command only
  after the launch gate opens.

## Launch gate

1. The custodian creates the control pipe and both output pipes (stdout and
   stderr), then forks the **monitor**. The monitor keeps the output read ends
   and the custodian closes its copies.
2. Setup runs in the **monitor**, inside the absolute deadline and against a
   bounded setup budget:
   - create the evidence directory;
   - publish phase `started: incomplete`;
   - take the first completed reserve and scratch readings;
   - send `ready`.

   The custodian waits with `select` until the setup deadline. If `ready`
   doesn't arrive by then, or arrives with low or failed readings, the
   custodian kills its pinned monitor, reaps it with `WNOHANG`, exits non-zero
   and launches nothing.
3. The custodian forks the **leader**. The child, in order:
   1. calls `setsid`;
   2. `dup2`s the output write ends;
   3. closes everything else;
   4. writes one **session-ready** byte to an ack pipe;
   5. blocks reading the gate pipe.
4. The custodian waits for the session-ready byte with `select` until the
   setup deadline. Only after it arrives (the child's PID is now its SID and
   PGID) does the custodian register kqueue `NOTE_EXIT`. `ESRCH` means the
   child is already a zombie, and any other error means there is no detector.
   The custodian then writes one byte to the gate pipe, which can't block
   because the pipe is empty. The child `exec`s the command, or `_exit(127)`s
   on failure. Custody, meaning the pin, the session and the detector, exists
   before the proof can run. Any blocking exec work happens in the child.
5. **Cancellation before the gate opens** (setup deadline, a fault, a signal,
   no ack, or no detector) uses `kill(child_pid, SIGKILL)` on the pinned,
   unreaped child. It never uses `killpg`, because the group may not exist yet.
   The gate is never opened afterwards. The outcome is `UNCLOSED` (or "not
   launched", if the ack never arrived and the child was reaped as killed),
   with scratch kept.

## Control protocol

Every message carries `run_id` (128 random bits), the leader's PID, its
kernel start time (`kinfo_proc` `p_starttime`, used only as an identity
token), a sequence number and a stamp. A message with a wrong `run_id` or
leader identity is a fault.

**One clock everywhere:** `time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)`.
Python 3.9's `time.monotonic` has a per-interpreter origin, so it isn't used.
Every stamp, deadline, freshness check and exported fixture deadline uses this
system-wide clock. It was verified on the host on 2026-10-21: available in
Python 3.9.6, and separate interpreters read one consistent timeline.

| Message | Custodian action |
| --- | --- |
| `reading reserve=<bytes> at=<uptime_ns>` / `reading scratch=<bytes> complete=<bool> at=<uptime_ns>` | Track the newest **completed** reading. Latch a fault if no reserve reading is newer than 1 s, no scratch reading is newer than 10 s, reserve is below the threshold, or scratch is over budget. Freshness is enforced until the custodian exits, through closure and cleanup too. A heartbeat without a measurement doesn't count. |
| `output eof stdout` / `output eof stderr` | Record the pipe-EOF witness. Both are required for `closed`. |
| `census seq=<n> after=<stop-seq> members=<k> uncertain=<u> certain=<bool>` | Accepted as final only if it started after the custodian's `stopped` message. |
| `cleanup_started`, `cleanup_complete`, `monitor_complete` | Ordering checks; see the receipt phases below. |

Faults latch: detector, signal, reap, guard freshness, guard threshold,
protocol, monitor death, an uncertain census, a missing EOF witness. **No
later message can clear a latched fault.**

## Deadlines and state table

`absolute = start + limit`, `stop_at = absolute - stop_reserve` and
`exit_at = absolute - exit_margin`. The proposed values are a 20 s stop
reserve and a 2 s exit margin.

| Trigger | Custodian |
| --- | --- |
| Leader exit (`NOTE_EXIT`), latched fault, `stop_at`, or a signal to the custodian | Send `stopping`. TERM the pinned leader group. |
| + `term_grace` (bounded by `exit_at`) | KILL the pinned leader group. Send `stopped(seq)`. |
| Next | `waitpid(leader, WNOHANG)` in a `kevent`-timed loop until `exit_at`. Still unreaped means latch `reap` and keep the pin; no further signal is sent anyway. |
| Final census with `certain`, 0 members and 0 uncertain, plus both EOF witnesses | Closure established. Tell the monitor `cleanup`. Otherwise `UNCLOSED`: tell the monitor `keep`. |
| `monitor_complete`, or `exit_at` | Reap the monitor with `WNOHANG`. If it's still running at `exit_at`, KILL the pinned monitor and do one `WNOHANG` reap. Exit. |

**Precedence:**
1. Any latched fault (detector, signal, reap, guard, protocol, monitor death,
   uncertain census, missing EOF) makes closure **not established**. The
   custodian sends `keep`, never `cleanup`, and no later message changes that.
   Cleanup runs only after closure is established with no latched fault.
2. The command's own status is recorded separately, from the leader's
   `waitpid` status, and is never overwritten. An exec failure (127), a
   non-zero exit or death by signal is never success.
3. **Custodian exit code**, highest priority first:
   1. 124 if `stop_at` or `exit_at` triggered;
   2. 125 for any latched fault or `UNCLOSED`;
   3. otherwise the command's own exit status, or 128+N for signal N.

   It is 0 only when there is no fault, closure was established,
   `monitor_complete` arrived before `exit_at`, both children were reaped,
   and the command exited 0.

## Receipt phases (monitor)

Each phase is published by atomic whole-file replacement in the evidence
directory, which counts against the owned-byte budget:

1. `started: incomplete`
2. `closure_established: cleanup_pending`, or `unclosed: scratch_kept`
3. `cleanup_partial_or_unknown`, published **before the first deletion**
4. `cleanup_complete`, published only after the deletion is confirmed
5. `monitor_complete: pending_custodian`

**There is no success phase in the file.** Success is defined as phase 5
**and** custodian exit 0, as recorded by the caller. A receipt can never claim
success by itself, so the custodian timing out after the monitor publishes
can't leave a false success behind.

## Output

The monitor drains both pipes on threads. It persists up to 8 MiB per stream
to the evidence directory, counted in the owned budget, forwards up to
32 MiB, and discards and counts the rest. The pipe-EOF witness is required for
closure. If something outside the session still holds the pipe open, EOF
never arrives, and the result is `UNCLOSED`.

## Census (monitor)

Each round:
1. take a `KERN_PROC_ALL` snapshot, giving PID, PPID, PGID, UID and
   `p_starttime`;
2. call `getsid` for each candidate;
3. re-read the candidate with `KERN_PROC_PID`.

The session binding counts only if the PID and `p_starttime` are unchanged
across the `getsid` call. Each process is then classified:
- **Member:** validated session equals SID. Once a member, it stays tracked
  by (PID, `p_starttime`) until a snapshot shows that identity gone, whatever
  its UID or parent.
- **Positively foreign:** only a validated session that isn't SID, with no
  earlier membership. **Start time is never used to exclude a process.**
  `p_starttime` is calendar time and can move backwards.
- **Gone:** `getsid` returned `ESRCH`, and a later snapshot confirms that the
  (PID, `p_starttime`) identity is absent.
- **Uncertain:**
  - not validated: `getsid` failed other than as gone, the identity changed
    across `getsid`, or metadata is missing;
  - any process at all in an incomplete, failed or timed-out snapshot.

  Uncertain blocks `closed`. It is never treated as foreign, and that holds
  for first-seen processes too.

This is practical because `getsid` reaches other users' processes on macOS.
On the host on 2026-10-21, 1,068 of 1,069 processes returned a session. The
one failure was `ESRCH`, a process that had just exited. So uncertain cases are
transient, and a false `UNCLOSED` from one is accepted: the run fails closed.

## Fixture fallbacks

Every fixture process, including nested guardians and pressure consumers,
does three things first:
1. `signal.signal(SIGALRM, SIG_DFL)`;
2. `signal.pthread_sigmask(SIG_UNBLOCK, {SIGALRM})`;
3. `signal.setitimer(ITIMER_REAL, remaining)`, where `remaining` is
   `(FIXTURE_DEADLINE_NS - clock_gettime_ns(CLOCK_UPTIME_RAW)) / 1e9`.

`FIXTURE_DEADLINE_NS` is an absolute deadline on the system-wide
`CLOCK_UPTIME_RAW`, passed in the environment. It stays valid across `exec`
and in every descendant. A late grandchild therefore can't extend it, and gets a
non-positive remaining time that exits it immediately.

Required ordering: **case verdict deadline < fixture fallback deadline <
overall qualification deadline**. Proposed: case limit 60 s, fallback at case
start + 90 s, qualification 300 s. A fallback firing is observed after the
verdict and is never counted as closure.

## Failure table

| Failure | Outcome |
| --- | --- |
| Monitor setup slow or failed | Nothing launched. Monitor killed, the pin was held. Non-zero exit. |
| No `NOTE_EXIT` detector | Gate never opened. Pinned child KILLed. `UNCLOSED`. |
| Monitor blocked (census, filesystem or receipt write) | Readings go stale, which is a fault. Pinned stop. The monitor is KILLed at `exit_at` if it is still running. The receipt's last phase is not success. |
| Monitor dies | Fault. Pinned stop. No reported PID is signalled. |
| A survivor in another group, or uncertain | `UNCLOSED`, scratch kept, never signalled. |
| Escaped pipe holder | No EOF witness. `UNCLOSED`. |
| Output flood | Bounded persist, forward and discard, counted in the budget. |
| Cleanup interrupted | The phase is `cleanup_partial_or_unknown`. |
| Monitor completes, then the custodian times out | The custodian exits non-zero, so it isn't success. |
| A child is unreaped after KILL | `reap` fault. The pin is kept until exit. No further signals. |

## Proof (elevated, after independent review of this design)

**Signal evidence:** at each signal, record the target and show that it was
the pinned child at that moment.

**Cases:**
- launch-setup stall;
- stale monitoring caused by a monitor blocked in the filesystem;
- delayed reaping;
- output saturation;
- an escaped pipe holder;
- a census identity change across `getsid`;
- interrupted cleanup;
- monitor success followed by a custodian timeout;
- an inherited SIGALRM that is ignored or blocked;
- a group that cascades (`closed`);
- a separate-group survivor (`UNCLOSED`, never signalled);
- cancellation before the session-ready ack, using `kill` on the pinned PID with no `killpg`;
- an exec failure (127) and a non-zero command exit, never success;
- a latched fault followed by a later `closed` census, with no cleanup;
- a fixture deadline that holds across `exec` and in a late grandchild.

If any of this can't be met within the approved scope, the brief stops and
names the missing mechanism.

## Cargo-group survivor custody investigation

The retained oxide#182 receipt records a deadline stop of the pinned leader
 group, followed by a certain census with one member in a different group and
 an `UNCLOSED` outcome. It is historical observation, not authority to act on
 that PID. Oxide starts Cargo in its own group and its lease recovery retains a
 slot while that group is present or its state is unknown. The supervisor's
 leader pin does not pin Cargo's group. The existing direct-child detach
 handoff works only because the monitor retains its own unreaped child until
 custody is settled; it cannot adopt an already orphaned descendant.

**No-go for retrospective signalling:** no supported identity-holding signal
 primitive for an observed non-child has been established by this investigation.
 PID plus start time, parent, session or group checks are observations separated
 from `kill` by a reuse race. A kqueue process event is notification, not a
 signal handle: XNU's [process filter](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_event.c)
 attaches a knote and releases its process reference; the attachment can be
 detached on exit. It does not establish signal custody. A Mach task port or
 private audit-token signalling interface has not been established as a
 supported, available, identity-bound signal primitive for these processes;
 neither is an implementation premise. Tracing/reparenting an arbitrary
 observed process is likewise not an authorized custody transfer. Retain the
 observation-only policy and keep roots and evidence on unknown closure.

A prospective cooperative alternative is for Oxide to remain the direct
 parent of the Cargo group leader, hold that child unreaped across any group
 signal, and perform bounded cancellation and closure itself before releasing
 its lease. A supervisor stop protocol would request that action while Oxide
 is alive, reserving time before the supervisor's own pinned-group escalation.
 This is a candidate, not an approved contract: a dead or unresponsive Oxide,
 a child already reaped, an escaped group, an unreadable response, or a spent
 deadline gives no authority to signal the observed group and leaves closure
 unknown. Group custody is distinct from the supervisor leader's group. The
 holder must prevent auto-reap and concurrent wait, retain the child through
 the signal, positively reap it afterwards, and separately prove group and
 session closure; holding a leader does not contain descendants that change
 group or session. No claim covers arbitrary same-user processes.

The protocol needs an authenticated per-run channel established before Cargo
 starts, explicit ownership and bounded acknowledgements, and a single absolute
 remaining window shared by request, TERM grace, KILL, reap and final census.
 Cancellation or supervisor fault cannot create a new allowance. A lost reply,
 stale request, concurrent exit/reap, PID or group reuse, launch failure,
 unresponsive parent, group escape, and late or contradictory census must all
 keep the lease and proof resources, never imply success. A cooperative stop
 cannot repair the historical orphan or guarantee closure after its owner dies.

### Held-child source discriminator

The following is source analysis, not an approved signal protocol or native
proof. POSIX.1-2017 [definitions](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap03.html)
(§§3.295–3.301; [readable mirror](https://mirror.doc.mel.vin/posix/basedefs/V1_chap03.html))
include a zombie in the definition of a process and define a group's lifetime
as ending when its last process leaves, including at the end of that process's
lifetime. Thus, *if* the child was actually placed in the group and cannot
leave it, and its parent exclusively retains it unreaped, an exited child is
not by itself the end of that group lifetime. This conclusion depends on the
membership and exclusive-reaping premises; a recorded PID or a later
`killpg(..., 0)` observation does not establish either.

Apple's archived [setsid](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setsid.2.html)
manual describes creation of a session and a group whose ID equals the
caller's PID; its [setpgid](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setpgid.2.html)
manual says a session leader cannot change groups that way. A group leader
that is not a session leader needs a separate cooperative membership premise:
the original group can outlive its leader's departure, and a group or session
escape also makes a held leader insufficient to contain descendants. The
[signal API](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/killpg.2.html)
addresses a group, not a held descriptor or a guarantee that every descendant
is inside it. These archived descriptions and the POSIX inference must still
be independently reviewed for the particular fixture and supported macOS.

Apple's [sigaction](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/sigaction.2.html)
manual specifies that `SA_NOCLDWAIT` prevents zombie creation. An explicit
`SIG_IGN` disposition is also not a custody premise. A fixture must arrange
and retain a non-auto-reaping disposition, forbid any concurrent wait of the
child, and treat owner death, a successful reap or uncertainty in either
condition as the end of signal authority. Apple's [waitpid](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/waitpid.2.html)
manual describes child selection and reparenting after parent termination;
a successful wait is not an observation that can be undone. An unresponsive
owner cannot delegate this custody merely by publishing its PID.

In Apple's XNU source at [`f6217f891ac0`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_fork.c),
examined 2026-11-01, `forkproc` checks the process, group and session hashes
when allocating a PID under the process-list lock. In the same revision,
[`kern_exit.c`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_exit.c)
places an exiting process on the zombie list, and `reap_child_locked` removes
it from its group and process hash under that lock; its `P_NOCLDWAIT` path
permits kernel reaping. This explains that revision's relevant mechanism,
not a version-independent public guarantee or evidence of the host kernel's
exact source. No native observation or qualification follows from it.

The fixture still needs an independently reviewed, executable ownership
argument: establish group creation before signalling; show how its owned child
is prevented from changing group while live; serialize all wait paths and
signal disposition; handle actual reaping and owner failure without a signal;
and distinguish a final group/session census from descendant containment.
The resource bound, negative controls and every executable premise-checklist
item remain to be specified and reviewed. This analysis does not authorize a
speculative group signal, change the existing supervisor, or resolve the
Cargo-survivor papercut.

**Fixture retention ruling (Tom, 2026-11-01).** On the question of the
qualified supervisor potentially cleaning scratch after a fixture child leaves
its session and its owner dies, Tom chose “Stop for redesign”: preserve the
source findings and hold the fixture until a separately scoped retention
mechanism is designed. The supervisor's census is limited to its cooperative
session and previously tracked identities; an escaped, untracked child may be
classified foreign. Closure additionally depends on output-pipe EOF, but a
closed pipe is not positive reap or proof of the escaped child's identity or
writer closure. A nonzero command status alone does not force the guard's
keep decision. These are source observations, not a native reproduction or a
claim that all possible designs are impossible. The brief requires uncertain
custody to retain resources and prohibits both supervisor changes and a new
supervision harness. Do not launch this fixture or treat its pure decision
model as native evidence while the retention gap remains unresolved. This
ruling does not change the retrospective no-go or any product signal authority.

Before implementation, independently review the ownership and deadline
 contract and obtain any necessary operator ruling. First discriminate whether
 the supported macOS child and group lifetime semantics actually support a held
 Cargo leader throughout the proposed stop, including zombie, concurrent wait,
 auto-reap and group reuse cases; source inspection and bounded isolated fixture
 tests must establish this rather than infer it from a PID check. Then separately
 design and test the channel and product cancellation, and finally integrate
 the shared supervisor. Tests must force each race above, owner death, partial
 messages, deadline exhaustion, survivor and escaped-group controls, and verify
 no signal to an observational non-child and no cleanup on uncertainty.
 Each implementation slice needs a scoped brief, HIGH proof premise checklist,
 a keep-going diagnostic and exact-head author and distinct reviewer native
 qualification under the shared runner's fixed time and scratch bounds. Writer
 closure makes this Elevated. The brief must identify every shared-supervisor
 consumer and explicitly rebind only the affected proofs after qualification;
 prior receipts remain bound to their original bytes. No native execution or
 behavior change is implied by this investigation.
