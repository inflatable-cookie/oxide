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

### Proposed #244 zombie-group witness (unaccepted design checkpoint)

The single diagnostic at `994a69d` **failed**: c01 and c02 observed
`kill(pid, 0)=0` and `killpg(pgid, 0)=EPERM`; c03 and c07 were skipped on
those dependencies. Four of eight native cases completed, all six spawned
children were positively reaped, and three source-only cases remained HELD.
The outer guard closed without fault or deadline; the nested outcome is
unknown, unqualified and uncounted, and its root remains unconcluded and kept.
The nested receipt `cargo-group-custody.6140f23968590c00/receipt.json` has
SHA256 `be0de8f905c16b708321ec4d3297f7439f916930afc25766b41de0fcc7d8c648`;
the command log has SHA256
`63ab7a67ccb7780ea895e278df2687a04cfbf79550c7ae5bfadfaf0f217f028a`.
These observations do not establish the original positive group-lifetime
claim. They do not revise the two earlier #242 packets or their counts.

Apple's archived [kill](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kill.2.html)
manual describes signal zero as error checking without signal delivery. For a
group, its EPERM description says it is returned “if any members of the
group could not be signaled”. Its
[killpg](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/killpg.2.html)
manual describes EPERM as a permission failure. Neither identifies a particular
held child as a member. In Apple's XNU revision
[`f6217f891ac0` kern_sig.c](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_sig.c),
`killpg1` finds the group then filters `SZOMB` and system processes before
its callback; the callback counts permitted targets and only sends a nonzero
signal. With no counted target the POSIX path returns EPERM. The callback's
separate exited-process branch checks `cansignal_nomac`, but that does not
undo the group iterator's zombie filter. Thus that source provides a plausible
mechanism for this observation, not evidence that the observed EPERM proves
membership or even that permission was the sole cause. The packet identifies
the macOS-only guard and its command, but neither receipt nor command log
records a macOS build, Darwin release or XNU version. At review and again at
this correction, the current host reports macOS 26.6.2 (25G83), Darwin 25.6.0,
`xnu-12377.161.14~5`, arm64 (retained review comment 6569891769, verdict
`host-version-binding`). This is a later observation, not the packet’s host
version. The version corresponding to `f6217f891ac0` has not been established,
and the reviewer found no `xnu-12377.161.14` tag. Its gap from the currently
reported kernel cannot be quantified; neither observation binds that revision
to the kernel that ran the diagnostic. Its behavior is not a public,
version-independent guarantee; no host-version equivalence is assumed.

**Candidate, not approved:** retain exclusive direct-parent custody from raw
fork through the single positive wait; establish the live child's PID, start,
parent, session and group in `HeldChild.bind` and the child report; reassert
non-auto-reap disposition and serialize every wait. During the existing
bounded window, read the qualified `Kinfo.one` before and after each observation
and require the same PID/start, direct parent, expected PGID and zombie state;
retain the actual signal-zero results as observations, not authority. Finally
require the sole positive wait with settled state recorded before publication.
The existing safe reader (`scripts/bounded-test.py` `Kinfo.one`/`parse`) has
PID, start, parent, PGID and zombie fields but **no session field**; its layout
check compares the current live process, not zombie field semantics. The
existing fixture's `wait_exit`, `observe_zombie_window`, `reap`, `settle`,
`probe_wait_live`, `case_c01`/`case_c02`, `case_c06` and `case_c07` are the
sites that would need review; `bind` currently reads group and session in
separate syscalls. This is a proposed evidence chain, not a replacement oracle.

The source discriminator below supplies a conditional lifetime argument for
its cited XNU revision: [`reap_child_locked`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/kern/kern_exit.c)
removes the child from its group and process hash, and the session leader
cannot move itself by setpgid. This is conditional on the live, exclusive
parent premise: `proc_exit` also calls `reap_child_locked` with
`REAP_DEAD_PARENT` for its zombie children when their parent exits. Group
removal can therefore happen during parent exit without its positive wait. Exclusive unreaped custody limits PID reuse, but a start token and matching parent/group observations are still
required; neither a recycled PID nor a reused PGID is a handle. A moved or
replaced identity, failed or malformed read, permission denial, missing row,
non-zombie or unverifiable session/group is unknown. A before/after pair is
not an atomic syscall and cannot alone exclude a change between reads; early
collection or an unobserved concurrent waiter destroys the custody premise.
A successful final wait proves collection then, not that prior samples or the
whole interval had the asserted group membership. A zombie being visible in
one row does not prove the reader reports its group correctly on this host.
The already-recorded rows do not contain the proposed paired parent/group
observations, so they cannot be retroactively upgraded. No observation adds
signal authority, descendant containment or permission to signal a historical
PID.

**NO-GO for equivalence at this checkpoint:** the host kernel/source binding,
zombie row semantics and interval continuity under all permitted transitions
are not established. The candidate could support a narrower sampled-identity
observation, but is not yet shown to prove the same positive group-lifetime
proposition. A future source-backed design must identify applicable kernel and
public semantics, show why identity/group cannot change between samples under
its exact custody premises, and address the separate session reads and TOCTOU.
Tom's ruling relayed by Queue planner `d070792d` on decision
`535ec990-1577-49fb-9741-480dc7434eeb` authorizes a **weaker sampled
held-identity claim for a separately reviewed design**. The intended claim is
limited to the exact direct child observed at the stated samples: under the
fixture's exclusive, unreaped parent custody, the bounded non-consuming reader
reports the bound PID/start, parent and PGID with zombie state, followed by the
single positive wait. It asserts neither continuous group lifetime nor that a
group signal would succeed, and it cannot turn EPERM into positive evidence.
Missing, malformed, denied or inconsistent facts remain unknown. The positive
held-zombie group-lifetime proposition remains an open follow-up, not a proven
result or an abandoned historical failure. This ruling authorizes design and
review, not an executable oracle change, new signal authority or a native run.

Before any implementation, the retained reviewer must assess this design.
A future reviewed implementation would need effect-free seam regressions for
positive paired rows and each missing, malformed, denied, changed PID/start,
parent, PGID, zombie and unverified-session case; counterfactuals must exercise
the real case bodies, early or second wait, receipt failure and post-reap
signal refusal, and demonstrate that EPERM alone never passes. A separately
planner-declared exact-head native diagnostic would need the same 8 native and
3 HELD cases, actual paired observations and one positive reap, the existing
300 s / 512 MiB bound, qualified guard, reserve/watchdog, ledger caps,
retention and no-new-signal rules. No fresh proof, retry, cleanup or executable
oracle change follows from this checkpoint.

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
it from its group and process hash under that lock. During parent exit the
`REAP_DEAD_PARENT` path can reap its zombie children; its `P_NOCLDWAIT` path
also permits kernel reaping. This explains that revision's relevant mechanism,
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

### Retention redesign for the held-child fixture (2026-11-02)

**Recommendation:** use the qualified predecessor's existing hosting marker and
its retained evidence subtree together. The planner froze this retention design
after retained-reviewer approval `6566191910` at `793f8a2` and merge `aad36e6`
(2026-11-02). This is a reviewed design, not an implemented protocol, a native
result, or authority to run an owner-death or escape experiment. The direct-child
and group signal-lifetime premises still need their own source review before
executable use. It is limited to the cooperative host and
the actual verified default owned-root topology below. Do not use the active
oxide#233 candidate as if it were qualified. The predecessor examined here is
`scripts/bounded-test.py` SHA-256
`fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b`;
its historical qualification and the existing receipts remain bound to their
own bytes. `proof-scratch-design.md` is not present in this checkout; the
scratch contract is the [workflow shared supervisor](workflow.md#shared-proof-supervisor)
and the pinned executable and tests cited below.

#### Source basis and scope

* `Monitor.owned_root` selects the prepared private `<mount>/scratch/oxide`
  parent, runs one bounded startup sweep, creates a fresh private root, and
  writes its owner marker before the monitor creates scratch/evidence or
  launches the proof. `Monitor.setup` sets `scratch = <root>/scratch` and the
  default `evidence = <root>/evidence`; `Monitor.run` creates evidence and
  publishes `receipt.json` before sending setup and ready. The custodian exports
  `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP`, and `TEMP` as **scratch**, and
  `OXIDE_PROOF_EVIDENCE` as evidence. Evidence is never immediate-cleaned by
  that runner, but that does not make it permanently immune to expiry or manual
  cleanup. The fixture must overwrite its own and every child's temp variables:
  inheriting the guard's scratch temp is unsafe for escaped writers.
* `protect_nested_scope` reads the existing strict marker, refuses absent,
  concluded or non-running state, and changes `nested_scope` to `hosting` with
  `write_marker`. The latter checks a private bounded marker, canonical owned
  root and expected marker inode before writing a private temporary file and
  atomically replacing the marker. An already-hosting marker is returned
  without a write. The fixture must verify the result by an independent strict
  reread, not trust the return value alone. These are path-based checks, not a
  hostile same-UID exclusion or a filesystem transaction. The code does not
  fsync the file or directory; atomic replacement is not a power-loss durability
  guarantee. Loss, torn or unreadable publication is refusal/unknown, never a
  signal or a positive closure. The initial and protected states are both
  unconcluded; neither by itself makes the root sweep-eligible. Power-loss and
  filesystem corruption beyond these source guarantees are not qualified by
  this design.
* `record_conclusion` refuses positive outer closure for a hosting marker unless
  `nested_concluded=True`. `Monitor.conclude_marker` does not pass that flag:
  on outer closure it records `nested-protected`; on uncertainty it can record
  a non-conclusion or report an unknown marker. The pure
  `test_a_hosting_root_is_never_concluded_from_outer_closure` and
  `test_a_hosting_root_with_retained_unknown_is_never_eligible` are source
  witnesses, not new test executions or native credit. No fixture in this
  design passes `nested_concluded=True`, records its own conclusion, or calls
  a sweep. A future separate owner disposition needs its own authority.
* `Monitor.run` may nevertheless `shutil.rmtree(ctx["scratch"])` on its own
  cleanup decision before `conclude_marker`; hosting does **not** protect that
  scratch leaf. An outer keep or failed/partial cleanup keeps scratch; an outer
  cleanup does not remove `<root>/evidence`. `sweep_one_root` only considers a
  strictly validated, identity-matching marker with positive conclusion and an
  eligible reason older than seven days; an unconcluded, malformed, replaced,
  unreadable or otherwise ineligible marker is skipped. `remove_marked_root`
  rechecks root/marker identity and walks without following links. A hosting
  root left unconcluded therefore is not a future expiry candidate, even after
  an outer clean exit; a manual deletion or third-party sweeper is outside this
  contract.
* `Monitor.sample_owned` calls `tree_bytes(ctx["root"])`, so both scratch and
  evidence are in the same sampled allocated-byte budget, with deduplication by
  device/inode and no symlink following. A vanished entry is skipped; other
  traversal errors are measurement faults. Sampling is not an atomic snapshot
  or a hard capacity ceiling, and accounting stops with the outer run. Retained
  writers after that point are not monitored; the fixture must have intrinsically
  bounded output and no post-owner unbounded writing. The existing reserve
  watchdog and 300 s / 512 MiB session limits are not extended.

#### Setup and ownership sequence

The future fixed fixture starts as the guard's proof leader, with the qualified
runner supplied by a separately accepted source binding. Before any fixture
resource, child, IPC endpoint or writer exists:

1. Refuse inherited adopted root, explicit `--root-parent`, explicit `--evidence`,
   fallback, unmarked root, or any unsupported topology. Read the guard's
   already-published `<evidence>/receipt.json` with a bounded no-follow regular
   file read and validate its schema and initial phase, `scratch_owned`,
   `scratch_default.source`, run ID and selected device. Compare its run ID to
   `OXIDE_PROOF_RUN_ID` and its exact root, scratch and evidence paths to the
   environment. The marker's random `scratch_marker.run_id` is a **different**
   token from the supervisor run ID: compare each to its own receipt field,
   never equate them. Refuse missing, partial, stale, changed or ambiguous facts.
2. Verify with no-follow component/descriptor checks that root, its selected
   private parent, scratch and evidence are real canonical directories on the
   same selected device, owned by the invoking UID, with root and private parent
   0700 and scratch/evidence not group/other writable; validate the receipt and
   marker as private or otherwise non-writable by group/other as appropriate.
   Check marker's root path, UID, device and inode against the opened root and
   the receipt's `scratch_marker` identity. Verify that evidence and scratch are
   distinct direct children of that root, no aliases or mount crossings. Hold
   directory descriptors and recheck name-to-descriptor identity before each
   publication and launch; replacement, unexpected mount, symlink, permission,
   or unreadable data refuses without launching. These checks assume no hostile
   same-UID actor and do not turn mode bits on an ownership-disabled volume into
   a security boundary.
3. Hash and load the exact qualified runner bytes, without importing a changed
   path after hashing (read no-follow, verify stable file identity and SHA-256,
   compile/load those verified bytes with bytecode writing disabled). Call its
   `protect_nested_scope(root, DefaultProbe())`, then independently reread the
   same root marker and verify `hosting`, `running`, null conclusion, run token,
   UID, device and inode; revalidate the held root and evidence descriptors.
   If publication, readback or binding is uncertain, refuse before creating any
   fixture resource. A partial marker temp is not authorization.
4. Exclusively create `<root>/evidence/cargo-group-custody.<random>/` as a
   0700 directory relative to the verified evidence descriptor, no symlink
   following. All private HOME, cwd, TMPDIR/TMP/TEMP, child inputs, output,
   local control files, and bounded fixture receipts live beneath it, never in
   `<root>/scratch` or external mounts. Scrub inherited `OXIDE_POOL`,
   `OXIDE_CONFIG`, `OXIDE_TEST_*`, `XDG_*`, and redirect temp and HOME for
   every child. No Cargo, rustup, images, account-home reads, live pools, or
   compiler flags are involved. Use no `mktemp` without an explicit owned
   template. Child stdout/stderr must be bounded and any files pre-created
   exclusively within this directory; do not rely on a pipe remaining open.
   Confirm the new directory identity, containment and device before launch.
5. Only then create a fresh owned direct fixture child and establish its
   independently reviewed group/membership premise. Publish a bounded receipt
   of binding and each transition in that directory; on any publication or
   custody fault abort subsequent cases and retain. No process pattern, observed
   non-child PID, old receipt or group-zero probe is signal authority.

| Interruption or failure | Required result |
| --- | --- |
| Before verified hosting | No fixture resources or children exist; refuse. A partially written marker is not a grant. |
| After hosting, before fixture directory | No fixture child exists; the root remains unconcluded, including if its owner dies. |
| During exclusive directory creation or receipt publication | No child launches until identity and complete readback pass; keep the protected root and any partial evidence. |
| After child launch, owner unresponsive/dead, reply late/missing, observation stale, or output/receipt malformed | Nested state UNKNOWN/UNCLOSED, no new signal by an observer; retain evidence and the unconcluded root regardless of outer cleanup, exit status or EOF. |
| Marker/root/evidence replacement, alias, mount, unreadability or device/ownership mismatch | Before launch refuse; after launch do not retarget or conclude, retain known resources and report UNKNOWN. Invalid marker is ineligible for the qualified sweep. Same-UID hostile replacement and independent external deletion are outside the cooperative boundary. |
| Deadline, monitor fault, reserve/budget fault or partial cleanup | No new allowance or retry; keep nested uncertainty. The guard reports its own closure/cleanup independently and may remove only scratch on its cleanup path. |
| Owner positively reaps all owned children and the separately justified final group/session facts are complete | Record the nested facts, but this first design still never concludes or deletes the hosting root. |

The receipt must bind supervisor and marker run IDs separately, exact opened
root/evidence/fixture device and inode, protection readback and publication
phase, parent/child/group identity and held interval, every attempted or refused
signal, actual direct-child reap, observed group/session facts and their
limitations. It must distinguish `outer` (guard's own closure, cleanup,
watchdog and measured bytes) from `nested` (per-fixture custody, reaps,
remaining or unknown writers, census scope and retained root). A missing or
partial nested receipt is UNKNOWN, even when the later outer receipt says
closed. A retained directory preserves evidence; it does not close a writer.
No receipt may use a positive leader as descendant containment.

The direct-child signal premise remains separate: an alive owning parent must
have itself spawned and still exclusively hold the child unreaped, prevent
`SIGCHLD` auto-reap and concurrent waits, and have positively established and
retained group membership through any group signal; a spent window, reap,
owner death, group/session escape or ambiguous binding refuses it. A group
leader alone does not prevent other-group descendants, and no signal authority
is conferred over an observed non-child. Source/pure negative controls must
precede any native case. In particular, an actual parent-death or session-escape
case is **source/pure-only** until its own bounded, no-new-authority lifetime
and containment argument establishes that any surviving fixture cannot write
unboundedly and that no unsafe cleanup or signal follows. Neither a keep-alive
pipe nor nonzero exit is such an argument.

#### Finite follow-on slices and acceptance

* **A — retention primitive, conditional.** The recommended existing mechanism
  needs no runner implementation if independent review accepts the topology,
  readback, crash windows and bounded-writer limits above. If any fails, do not
  substitute an unreviewed marker or proceed. Separately scope an opt-in
  shared-runner KEEP contract only after oxide#233's reviewed disposition and
  a safe qualified-input transition: custodian binds a bounded request to its
  own run/root and acknowledges KEEP before the child gate opens; the obligation
  is monotonic through owner death, checked before both scratch cleanup and
  conclusion/expiry, and unreadable, malformed, missing after opt-in, stale,
  late or unacknowledged state keeps/refuses. A bounded schema must name version,
  run token, root device/inode and nonce; the receipt records request, ack,
  decision and fault. Define one fixed maximum document/read and the existing
  remaining deadline, no new clock, signals, discovery or keeper. Absence of
  opt-in preserves request-free bytes and behavior. Separate pure crash/order,
  alias, replacement, malformed, legacy and custody tests plus the runner's
  independently reviewed fixed qualification chain are required. This fallback
  is design only; its exact atomic persistence and acknowledgement implementation
  must be specified and reviewed in that separately bounded slice.
* **B — fixture apparatus.** Against the accepted retention contract and exact
  qualified runner, implement the fixed case table and executable nine-item
  premise checklist, bounded receipts, fail-fast counted path and diagnostic
  that continues only with independent known fixture state and aborts on
  infrastructure, writer, device, custody or receipt uncertainty. Effect-free
  self-check and plan prove refusal and case dispatch first; independent
  exact-published-head review by the retained Queue reviewer approves ownership,
  topology, resource basis and negative controls before one source-immutable
  diagnostic through the task's `proof` route. That same reviewer judges the
  actual receipts. Tom's single-reviewer ruling `c8991f99` supersedes the older
  counted-author/distinct-reviewer execution chain for this follow-up: no extra
  reviewer or native-reader agent is launched. Any further native qualification
  is a planner-owned release gate, with missing evidence recorded honestly.
  Native bounds are 300 s / 512 MiB, no Cargo or images. Parent-death/session-escape native cases
  remain held until the separate containment argument above is accepted.
* **C — product integration.** Independently design and scope the authenticated
  per-run channel, cancellation and shared-supervisor integration, including any
  required operator contract ruling. This design neither changes product waits
  nor fixes papercut `9ff7386d`; no historical run or pass is migrated.

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

#### Slice B implemented: the fixture apparatus (2026-11-02, unqualified)

Slice B is implemented at `scripts/test-cargo-group-custody.py` with the
selectors `effigy rust:cargo-group-custody`, `:diagnostic`,
`proof:cargo-group-custody:test` (pure `--self-check`) and `:plan`. It follows
the setup-and-ownership sequence above against the pinned qualified runner
`fcfda2b0…` (missing or changed bytes refuse; only the verified bytes are
compiled and executed, with bytecode writing disabled and no path switch),
validates the guard's published receipt and topology before any fixture
resource (refusing adopted, explicit-parent/evidence, fallback, unmarked,
stale, replaced or foreign placements and any equation of the supervisor and
marker tokens), calls the runner's `protect_nested_scope` and independently
re-reads `hosting`/`running`/null-conclusion plus the root/marker binding,
and then creates `evidence/cargo-group-custody.<random>/` (0700, by evidence
descriptor, no symlink following) as the only writer target for HOME, cwd,
temp, child reports, logs, stop files and the nested receipt. The bounded
receipt (64 KiB, 32 case / 32 signal-refusal / 64 identity rows with totals
and omission counts, republished atomically — not power-loss durably — after
every transition, including the leader's own bounded TERM/INT settle) binds
the two tokens separately and the exact dev/inode
identities, and distinguishes `outer` (never claimed by the fixture) from
`nested` custody; a missing, partial or unwritable receipt aborts later
cases, and EOF, nonzero exit, an absent observed PID or an outer closed never
turn nested unknown into closed.

The native case map proves, on the fixture's own controlled direct children
only: child-created session/group membership and identity binding;
`EPERM`-refused `setpgid`/second-`setsid` escapes for the session leader;
exclusive unreaped custody with `SIGCHLD` `SIG_DFL` (the `SIG_IGN`
counterfactual demo shows kernel auto-reap, `ECHILD`, no zombie); `WNOHANG`
collecting nothing while live and `ECHILD` on a second wait; bounded
`kill`/`killpg`-0 zombie persistence with the same `(PID, start)` identity;
the prospective cooperative stop (bounded `killpg` TERM, escalating KILL,
only under positive held custody, then the single positive reap); refusal
after settled custody with no syscall; and vacuous `kill(0,0)`/`killpg(0,0)`
as the demonstration that observations are not signal handles. These cases
correspond to the source discriminator above: a zombie held unreaped by its
parent keeps its group alive until the reap, and the kernel cannot auto-reap
behind a `SIG_DFL` holder that never waits concurrently. `s01` owner death,
`s02` owner-unresponsive and `s03` descendant group escape remain HELD
source/pure-only: no bounded no-new-authority containment/lifetime argument
has been accepted, so the script refuses to execute them natively. The
apparatus is unqualified: no native run has happened, native execution is
gated on the retained Queue reviewer's exact-head source/premise approval,
the one counted-path diagnostic runs through the task's proof route (never
by author or reviewer), fit is unmeasured, and nothing here is a product
closure claim, a supervisor change, or progress on papercut `9ff7386d`.

##### Slice B correction: the consuming-wait-as-observation defect (oxide#244, unqualified)

The first two diagnostic executions of the slice-B fixture (oxide#242, 2026-11-03,
both at source `31920b1` under the accidental double launch; the superseded and
recorded packets are retained evidence at `/Volumes/Builds/scratch/oxide`) failed
with zero native cases completed. The reviewer's judgment traced it to a real
fixture defect: `HeldChild.wait_exit` polled `os.waitpid(pid, WNOHANG)`, which is
non-blocking but NOT non-consuming (Apple waitpid(2) RETURN VALUES; a poll that
returns the child has already reaped it), and marked the returned child
`held-exited` — unreaped — instead of collected. Every later step therefore ran
against a consumed child: the held-zombie window answered `ESRCH` and the single
positive reap answered `ECHILD`. The recorded packet counts stand: 0/8 native
cases completed, three children spawned and none recorded as positively reaped,
c01/c02 failed, c03 skipped on its failed dependency, c04 aborted on the `ECHILD`
uncertainty, c05–c08 unexecuted; c05 was never reached, so no environment
auto-reap fact was established or ruled out. No environment-only retry is claimed
and no receipt is retrospectively repaired or concluded.

The bounded correction (oxide#244) keeps the same qualified guard, bounds, case
map (8 native + 3 HELD source-only) and every retention/ledger oracle, and changes
only the fixture's helper and case bodies:

* `wait_exit` no longer calls `waitpid` at all. Exit is observed WITHOUT
  collecting through the safe non-consuming `kinfo_proc` reader's zombie flag
  (the same qualified observation the supervisor's census uses), bounded by the
  case cap; an absent, unreadable, not-yet-zombie or identity-replaced row is an
  unknown observation that neither settles nor collects, and the window ends in
  `Uncertain` with custody unchanged. The observation publishes an
  `exited-observed` row carrying no status; an exit status is never invented.
* The single consuming wait is `reap`'s blocking `waitpid`, which only ever runs
  after a positive exit observation (or with the state already
  `held-exited`) and marks in-memory settled truth — state, reaped flag and the
  actual collected status — before the fallible receipt row, so a publication
  failure after collection can never cause a second collection attempt, a
  repeated signal or a claimed-held child. A second reap/signal still refuses
  without a syscall.
* The consuming `waitpid(WNOHANG)` live probes in c01/c06 moved into one helper,
  `probe_wait_live`: a positive return IS the collection this call made, so it
  records the settled truth immediately and fails the case instead of raising
  while leaving held state behind; `ECHILD` and other probe errors are recorded
  unknown observations that end in `Uncertain`, never collection or liveness.
* c06's known timing premise is addressed by a positively controlled
  hold/release lifecycle instead of racing a self-exit child: a hold child that
  cannot exit before its stop file stages the `WNOHANG == 0` live probe, then
  the release, the non-consuming exit observation, the single positive reap and
  the unchanged intentional explicit post-reap `ECHILD` negative control.
* `derive_nested_outcome`'s second branch was dead code (its condition equalled
  the first branch's), so a fully reaped, receipt-published run with a failed
  case lost its truthful writer-closure fact. The branch now reads
  `receipt_ok and all_reaped`: writer closure is reported positively when every
  owned child is positively reaped under a published receipt even when a case
  failed, and a clean outer run still never turns nested unknown into closed.

The pure `--self-check` gained five seam-driven regressions (isolated
`os.waitpid`/Kinfo/receipt seams, no child, no scratch): the collecting-wait
observation is refused, a non-consuming observation preserves held identity
until the sole positive wait, an unexpectedly positive live probe settles
collected truth immediately, unknown/`ECHILD` observations never settle and the
helper never falls back to a consuming wait, receipt failure after the positive
wait retains settled truth with no extra wait or signal, and writer closure
survives failed cases. The observation, unknown-observation and receipt-failure checks discriminate
the retired helper (retained review `f33b4eaf`, verdict
`contract-counterfactual-list`).
A separate regression drives both actual c01 and c06 case bodies with a positive
live wait; restoring their retired inline waits fails that regression because
they leave collected custody marked held. The qualification dependency stays the exact
pinned runner `fcfda2b0…`; no product, supervisor or consumer bytes change, the
native diagnostic remains gated (exact-head source/premise review, actual pins
and the single-flight launch fix), and this correction carries no native credit
and no product closure claim.
