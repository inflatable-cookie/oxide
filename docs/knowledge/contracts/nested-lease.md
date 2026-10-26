# Proposed design: nested Oxide joins the outer lease and borrows its host permit (oxide#157)

**Status, 2026-10-26: proposed design. Not implemented, not frozen.** This
document is the design deliverable for oxide#157. It changes no product
behaviour and authorizes no code, native run, host setting, live pool,
consumer change, runtime install or cleanup. It is not current product truth:
the shipped behaviour stays what
[workflow](workflow.md#what-one-command-does-local-executor) and
[local lease](local-lease.md) say, where a nested `oxide` takes an
independent lease. An independent exact-head design review must pass the four
gates (operator scope and boundary, feasibility and source evidence, custody
and failure design, test and proof plan) and the Oxide Supervisor then gives
the design freeze (Tom, decision `95b3bc63`); only then may an implementation
brief be written. Authority for the direction is Tom via Supervisor
`3cb9b620`, decision `17a1c828-2735-4a9d-a78b-651e8621f733`, recorded in
[vision](../vision.md#product-workflow-direction).

Owning knowledge: this file owns the nested-join contract. The
[front door](workflow.md#what-one-command-does-local-executor) keeps owning
command classes, admission, waiting and closure; the
[local lease](local-lease.md) keeps owning the pool, ledger, slots, recorded
accounting and the front-door control directory;
[architecture](../architecture.md#storage-invariants) keeps owning the
storage invariants and the authority boundary. Where this design needs a new
durable field or a new control record it says so and defers the exact
implementation choice to the reviewed implementation brief.

## 1. The contract in one page

- **One authority object per leased command generation.** A leased `oxide`
  command issues, for its own process subtree only, one *lease token*: a
  128-bit nonce in the child environment **plus an inherited, unlinked,
  owner-only descriptor** holding the binding facts, **plus durable state**
  (a record `permit` field, the record's held lock and the ledger's held
  lease). Any one of the three without the others is not authority. An
  environment string alone is not authority ([§3](#3-the-lease-token-is-possession-plus-live-state)).
- **Join** — a nested `oxide` whose computed slot key equals the token's
  key (workspace, recipe, toolchain, target, destination) joins the outer
  lease: no ledger lease, no slot, no host permit, no walk, no eviction, no
  freshness gate, no output binding, no release. It writes into the outer
  slot in a **private build directory** and registers a **writer ticket** so
  the outer cannot publish closure, accounting or a flush while it is live.
- **Borrow** — a nested `oxide` whose key differs gets its own slot through
  the unchanged admission path, and **borrows the outer host permit** so
  nesting cannot deadlock the host limit. Borrowing changes exactly one
  input: the host active-build count. It bypasses no reserve, budget,
  accounting, cap, clean barrier, ownership, freshness, output-custody or
  flush rule.
- **Never**: a joined or borrowing command never deletes, releases, recovers,
  evicts, flushes, relabels or rebinds anything it does not own; a nested
  command never releases the outer lease or the outer permit; a permit is
  never duplicated.
- **Bounded**: four joined levels below one outer command, eight live joined
  writers per generation and eight live borrows per root permit. A token or
  command past a bound refuses with exit 1; it is never quietly converted
  into an independent lease.
- **Stale, foreign, malformed or unknown** evidence is ignored, never
  permission: the command takes the independent path. Where an independent
  wait could only be satisfied by the ancestor that is waiting for it, the
  command refuses with an actionable message instead of waiting
  ([§6](#6-waiting-deadlock-and-cancellation)).
- **Unchanged**: exclusive slot ownership, conservative recovery, idle-only
  eviction, recorded per-slot accounting, the removable flush and
  surprise-removal fences, output custody and the operator's storage
  invariants.

### Terms

| Term | Meaning |
| --- | --- |
| Outer command | The top-level leased `oxide` that holds the ledger lease, the record, the host permit and the pool selection. |
| Nested command | Any `oxide` process started below the outer command (build script, Cargo, a tool under `oxide exec`, a test, a helper). |
| Lease token | The nonce + inherited descriptor + durable record `permit` described in §3. |
| Writer ticket | One durable record of a joined writer's phase and process group, inside the outer generation's control directory. |
| Permit | The host-wide active-build count slot the outer already holds (`Store::attempt`'s `active < max_active`). |
| Generation | One record lease: lease ID, slot ID and incarnation from the ledger. |

## 2. Source facts this design builds on

Every mechanism below already exists in this repository; none is a new
promise about Cargo or the kernel.

| Fact | Source | Why the design depends on it |
| --- | --- | --- |
| A command holds an exclusive `flock` on its record lock for its whole life; the kernel drops it when the process dies. | `Store::acquire`/`Lease::_lock`, `probe_lock`, [crash recovery](workflow.md#crash-recovery) | The *live* proof of the outer command. A token whose record lock is free is stale by definition. |
| The record names the holder (PID, start time, lease holder), the phase and Cargo's process group; `settle` recovers only with a free lock, matching own lease and an empty recorded group. | `store::settle`, `store::return_path` | The join ticket reuses exactly these phases, so a joined writer's crash is recovered like an own-command crash, never optimistically. |
| The front-door record's `phase` moves `Acquiring → Spawning → Running → Idle/Retired`, and `mark_spawning`/`mark_running` are durable before and after the spawn. | `Store::mark_spawning`, `Store::mark_running` | A ticket that dies between spawn and group record must keep custody, exactly as a record does. |
| Admission counts `active` records plus the sibling pool's, refuses at `max_active`, and waits with FIFO tickets. | `Store::attempt`, `Store::acquire`, [storage and space management](workflow.md#storage-and-space-management) | The one input a borrow replaces. |
| Every mutating command and every release takes the pool **admission lock**; `attempt` validates and grants inside it. | `Store::admission`, `Store::release` | Serializes permit validation against the parent's release, so a permit cannot be duplicated or freed early. |
| `managed::snapshot` reads config and ledger with no pool lock, no reconciliation and no write. | [passive snapshot](local-lease.md#passive-snapshot-and-measurement-oxide128) | A nested command may validate the outer lease without taking a lock or mutating the pool. |
| The recorded per-slot figure is published only by the atomic `Held → Idle` replacement of the slot it closes. | [recorded per-slot accounting](local-lease.md#recorded-per-slot-accounting-oxide146-implemented-native-held) | Closure must include joined writers, or a measured idle slot would be published while a nested writer still wrote. |
| `release` order is: owned scratch removal → removable flush → ledger release → record idle. | `Store::release`, `Store::discard_scratch`, `Store::flush_before_idle` | The ticket gate and the closure check insert before this sequence and change nothing in it. |
| Cargo takes a per-profile lock in the build directory it uses (`.cargo-lock` / `.cargo-build-lock`) and holds it for the build. | [clean classification](workflow.md#clean), pinned Cargo `c980f486` | Sharing one build directory between an outer Cargo and a nested compile would deadlock; §4.3 derives the nested build directory. |
| A leased command's children get a chosen `TMPDIR`/`TMP`/`TEMP`; an inherited qualified directory is kept, never created or removed. | [child temporary files](workflow.md#child-temporary-files-oxide136) | A joined command inherits the outer's directory and must not create or remove one. |
| `apply_storage` sets `CARGO_BUILD_BUILD_DIR`, `CARGO_TARGET_DIR`, the recipe variables and the wrapper policy on the child. | `main::apply_storage`, [inherited rustc wrapper](workflow.md#inherited-rustc-wrapper) | The joined command re-derives the same values from the same environment, never a second policy. |
| The front door's control state is a bounded, strictly parsed `state.json` with a forward-only version fence. | `store::State`, `STATE_VERSION`, [installed hand-off](workflow.md#installed-binary-hand-off-oxide122) | One new optional record field needs a version bump and a hand-off trigger, not a silent change. |
| Debug builds expose observation and fault hooks; release builds have none. | `inventory::walk_observer` and the `OXIDE_TEST_*` controls | The proof uses debug staging only, and no product decision may depend on a hook. |
| macOS and Linux both expose a process's parent and start time (`sysctl(KERN_PROC_PID)` `kinfo_proc`; `/proc/<pid>/stat`). | [process containment](../qualification-readiness.md#process-containment) and the shared supervisor's `kinfo_proc` use | The subtree check of §3.5; a platform that cannot answer refuses the join. |

## 3. The lease token is possession plus live state

### 3.1 Why an environment string cannot be the authority

An inherited variable is copyable text. It survives in a shell, a saved CI
environment, a `println!("{:?}", env)` log, a copied `.env` and a stale
parent. Treating it as authority would let a command join a lease that is
already released, one from an unrelated checkout, or one whose outer process
is dead. The design therefore requires three independent factors, and every
one of them is checked by the nested command before it does anything:

1. **Possession** of an unlinked, owner-only descriptor created by the outer
   for this generation. There is no path to open it, so a copied string
   cannot recreate it. A descriptor can only arrive by inheritance from the
   outer's own subtree (or by an explicit same-UID hand-off, which §3.6
   places outside the cooperative boundary).
2. **A live outer generation**: the record exists and is active, its
   `permit.nonce` equals the token, its record lock is **still held**, and
   the ledger still shows the same lease ID, slot and incarnation for that
   project. A dead, released, replaced or recovered generation fails at
   least one of these.
3. **Process-subtree relation**: a bounded walk of the caller's ancestors
   (PID + start time, so PID reuse cannot satisfy it) reaches the token's
   issuer PID. A token replayed into an unrelated process, a daemonized
   helper that left the subtree, or a `setsid` escape fails this.

The threat model is the existing cooperative same-UID one
([architecture](../architecture.md#storage-invariants),
[containment](../qualification-readiness.md#process-containment)): a
same-UID process that can read the pool can already write the slot, so the
design does not claim to stop it. What it must stop is **mistaken** trust:
stale text, a copied value, a wrong pool, a recycled PID, a released
generation, a crashed outer and a token presented outside the subtree. Those
are exactly the checks above, and each has its own counterfactual in §13.

### 3.2 The three parts

**Environment (locator and reconciliation only).**

```text
OXIDE_LEASE_TOKEN=<32 lowercase hex, 128 random bits>
OXIDE_LEASE_TOKEN_FD=<integer 3..=1023>
```

Both are set on every child an issuing command spawns for a leased command. A child
that rewrites, clears or forges them loses the join, not correctness.

**Descriptor (the capability).** The issuing command creates
`<control>/nested/<record>/.token-<nonce>`, mode `0600`, writes one JSON
document `oxide.nested.token/1`, `fsync`s it, clears `FD_CLOEXEC` on the
descriptor so the whole subtree inherits it, and unlinks the name. The
document is at most 4 KiB with UTF-8 byte caps like the record's:

| Field | Meaning |
| --- | --- |
| `version` | exactly `1` |
| `nonce` | the issuing record's `permit.nonce` (the generation nonce; a joined issuer keeps the root record's) |
| `store_id` | the granting pool's recorded store identity |
| `record` | the issuing record's id (the root record, or a borrowing command's own record; a joined issuer keeps the root record) |
| `permit_root` | `{ store_id, record, nonce }`: the permit a borrowing descendant records as its parent. A record-holding issuer sets it to its own permit triple when `parent` is null, else to its `parent`; a joined issuer passes its own `permit_root` through unchanged |
| `lease_id`, `slot_id`, `incarnation` | the held ledger generation |
| `checkout`, `compatibility` | the canonical workspace and the slot key |
| `issuer_pid`, `issuer_started_ms` | the process that issued **this** token: an outer issuing depth 1, or a joined command re-issuing for its own children |
| `issuer_session` | the issuing process's session id (`getsid(0)`) |
| `group` | the issuer's child group, once recorded (`null` before) |
| `depth` | the joined level the holder would become **within the issuing record's generation**: a record-holding command issues `1`; a joined command at level `d` issues `d + 1`. A token above `MAX_JOINED_LEVELS` (4) refuses; it never grants and never falls back |
| `issued_ms` | wall clock, an observation only |

Unknown fields, unknown versions and malformed values refuse the join. The
document carries no secret; its authority comes from being *unopenable by
path* and from the other two factors. Every issuer creates a **new**
unlinked descriptor, so a derived token is a distinct capability. A joined
issuer keeps the root record's `nonce`, so every derived token in one
generation dies together; a record-holding issuer mints a fresh nonce for its
own generation.

**Durable state.** `Record` gains one optional field:

```text
permit = { nonce, issued_ms, depth,
           parent: null | { store_id, record, nonce } }
```

The command's own `nonce` is fresh per command and addresses its subtree. For
a record-holding command `parent` is `null`; a borrowing command records its
parent permit reference here, which is exactly what §5.2 counts and §5.4
shows. `depth` is the record's own joined level within its generation (`0`
for a record-holding command that joined nothing). Every borrow anywhere in
the tree references the **root** permit (the token's `permit_root`), not its
immediate parent, so the coverage chain is one level deep and cannot be held
open by an intermediate command.
The field is cleared when the record leaves the active
phases and re-minted by every command that takes the record. `State` gains no
other field: the gate, its closing marker and the tickets are files under the
generation's control directory, so closure never depends on parsing a large
document.

### 3.3 Issuance

There are exactly two issuers, and both run the same steps through one code
path; they differ only in the record mutation and the depth they stamp.

**Steps (every issuer).** After the child-temporary decision and before the
child is spawned:

1. Create the descriptor: a fresh `0600` file
   `<control>/nested/<record>/.token-<random>`, write `oxide.nested.token/1`
   with `depth = own level + 1` (a record-holding command's own level is `0`),
   and `issuer_pid`, `issuer_started_ms`, `issuer_session` from this
   process; `fsync`, clear `FD_CLOEXEC`, unlink.
2. Add `OXIDE_LEASE_TOKEN` (the token's own `nonce`, which is the issuing
   record's `permit.nonce`) and `OXIDE_LEASE_TOKEN_FD` to the child `Command`
   immediately before spawn.
3. For a **record-holding** command (top-level, independent or borrowing)
   only: write `permit` into its record under the admission lock (fresh
   nonce; `parent` = `null` or the borrowed reference), before the descriptor
   exists. A command that crashes after this leaves an active record whose
   permit the next release or recovery clears.
4. For a **record-holding** command only: create
   `<control>/nested/<record>/` (`0700`), its empty `gate` file (the lock
   target) and its `writers/` directory, with the control directory's
   existing owner-only, no-symlink checks. Closure later creates the sibling
   `closed` marker under the gate lock. A borrowing command does create this:
   it is the root of its own generation and its joined children write into
   its own slot.

**Record-holding issuance** (token depth 1). Every command that holds a
record — a top-level command, an independent fallback or a borrowing command —
writes its own `permit` (step 3), creates its own generation directory
(step 4), and issues a depth-1 token for its own subtree. A borrow therefore
starts a **new** generation with its own depth counter; recursion across
borrows is bounded by the eight-live-borrows-per-root-permit rule (§5.2), not
by the joined-level counter.

**Joined re-issue** (token depth `d + 1`). A joined command that validated a
token of depth `d` issues a depth-`d + 1` token for its own children, because
it is the only process that can prove its own subtree: it does **not** write
a record or a `permit`, does **not** touch the gate or the ticket, does
**not** mint a nonce, and does **not** create a second generation directory.
It reuses the facts it validated (`store_id`, `record`, `nonce`, `lease_id`,
`slot_id`, `incarnation`, `checkout`, `compatibility`) and adds only its own
issuer identity and `depth + 1`. Every derived descriptor is unlinked like
any other, so a grandchild holds a distinct capability whose ancestor check
can succeed. A joined command at level 4 issues a depth-5 token, which the
depth rule refuses rather than silently converting a fifth level into an
independent lease.

**Borrowing command.** A borrowing command has its own record, lease and
permit, so it is an ordinary issuer for its own subtree: its children join
*its* lease (depth 1 relative to it), and a descendant borrow records the
root permit carried in `permit_root`, never the intermediate record. That
keeps the coverage chain one level deep (§5.2), makes the borrow fan-out
countable at the root, and needs no cross-store stitching.

Nothing is issued for a command that will not spawn (refusals, admission
expiry, budget expiry before the launch attempt, a gate refusal): those
paths release the lease as they do today and write no permit.

### 3.4 Lifetime, revocation and bounded age

The token's authority ends at the **earliest** of:

- the record leaving `Acquiring`/`Spawning`/`Running` (release, retirement,
  recovery, eviction, quarantine);
- the record lock becoming free (the issuing record's holder died);
- the `closed` marker being created (the record holder started its closeout);
- `permit.nonce` changing (a later command took the record);
- the issuing process exiting.

There is deliberately **no wall-clock TTL**. A clock cannot tell whether the
outer is alive, and a TTL would keep authority alive after a crash — the one
case that must fail. The bound on age is the outer command's own life, which
its admission/execution budget (if any) and the host limit already bound.
A token found in an environment after the outer ended is stale by
construction.

Revocation is therefore free: close the gate, clear `permit`, release.
Removing the directory is cleanup, never the revocation itself.

### 3.5 Validation, in order

The nested command performs these checks after resolving its own workspace
and slot key, and before any pool write, any wait, any output effect or any
spawn. `F` means "ignore the token and take the independent path (§6)";
`B` means borrow-eligible; a named refusal is exit 1 and is never converted
into a fallback.

| # | Check | Failure |
| --- | --- | --- |
| V1 | Both variables present, well formed, `FD` in range. | F |
| V2 | `fstat`: regular file, owner is the effective user, no group/other write, size ≤ 4 KiB, link count 0. | F |
| V3 | Read exactly from the descriptor (bounded, one read retry on `EINTR`), parse strictly; `nonce` equals the environment value (constant-time compare). | F |
| V4 | The selected pool's recorded `store_id` equals the token's, and the record lives in that pool's control directory. | F |
| V5 | A passive snapshot shows the ledger lease held with the token's `lease_id`, `slot_id`, `incarnation` on the token's project, and `state.json` shows that record active with `permit.nonce` equal. | F |
| V6 | A non-blocking exclusive `flock` on the record lock fails with `EWOULDBLOCK` (still held). A free lock, a missing lock file or any other error is not liveness. | F |
| V7 | The bounded ancestor walk (`≤ 64` hops, each PID with its start time) reaches `issuer_pid`, and this process's session id (`getsid(0)`) equals `issuer_session`. A caller that left the session with `setsid` keeps its ancestry only if the chain still reaches the issuer; the session check refuses it either way. | F |
| V8 | `token.depth ≤ 4` (`MAX_JOINED_LEVELS`). Checked only after V1–V7 held, so a depth-5 token minted by a level-4 joined command **refuses**; it never becomes an independent lease. | exit 1 |
| V9 | This command's computed key equals the token's `checkout` and `compatibility`. | B if unequal (V5–V8 already held) |
| V10 | For a join only: the `closed` marker is absent, and the inherited `TMPDIR` is the qualified directory this command would keep (§4.2). | The independent path when the gate is closed; exit 1 before the spawn when the inherited temporary directory is unqualified |

V5 and V6 are separate on purpose: the ledger can still name a lease whose
holder died, and the record can be active with a lock already dropped. Both
are required.

The ancestor walk is one small platform helper: `sysctl(KERN_PROC_PID)` /
`kinfo_proc` on macOS, `/proc/<pid>/stat` on Linux. It reads only the caller's
own ancestry. A platform, permission or parsing failure yields **no
ancestors**, so the join is refused (unknown is never permission). The walk
is bounded in hops and in time (one deadline for the whole check) and prints
nothing on success.

### 3.6 What the token cannot do

- It cannot be exchanged for a slot: join takes none, borrow takes one
  through normal admission only.
- It cannot name a pool, path or slot the nested command did not verify.
- It cannot outlive the generation, and it cannot be renewed by a child: a
  joined command re-issues a *derived* token for its own subtree with its own
  issuer identity and `depth + 1` and the same root record/nonce, but never
  mints a nonce and never writes durable state (§3.3).
- It cannot authorize deletion, release, recovery, eviction, flush, relabel,
  rebind, adoption, detach or prune. Those commands never join
  ([§4.2](#42-what-a-joined-command-does-and-does-not-do)).
- A same-UID process that deliberately inspects and re-sends another
  process's descriptor is outside the cooperative boundary and gains nothing
  it could not already do by writing the slot path directly. The design says
  so rather than implying a sandbox.

## 4. Join

### 4.1 Eligibility

Join requires V1–V10 and **exact key equality**: same canonical workspace,
same compatibility digest (repository identity, recipe, resolved `rustc -vV`,
target triple, codegen backend, owned-output destination) and the same pool
generation. "Same workspace, recipe and toolchain" in
[vision](../vision.md#product-workflow-direction) is exactly this equality;
nothing is compared by string prefix or by label.

Joinable classes are the leased ones
([command time budgets](workflow.md#command-time-budgets-oxide140)): `check`,
`build`, `test`, `clippy`, `run`, `doc`, `bench`, the other building
built-ins, `exec` and external subcommands. Non-leased commands
(`status`, `plan`, `fmt` passthrough, `init`, `new`, `clean`, `outputs *`,
`pool *`, `clean --cargo-cache`) never join; they run as today except for the
refusals in §6.

### 4.2 What a joined command does and does not do

| Step | Joined command |
| --- | --- |
| Pool open / selection | Passive snapshot only (V4/V5); no pool lock, no reconciliation, no creation. |
| Workspace, key, toolchain | Computed as today (one `cargo locate-project`, one `rustc -vV`). |
| Output reservation, adoption, binding | **Not run.** The outer already bound this checkout; a joined command verifies the key's destination digest and nothing else. `auto_adopt`, `reserve`, `cancel-reservation` and `bind_for_command` are skipped, so a joined command publishes no receipt, registry row or nonce. |
| Slot/ledger lease | **Not taken.** No ledger write, no admission, no walk, no eviction, no ticket in the waiter list. |
| Freshness gate | **Not run**, because the joined build directory is empty (§4.3): there is no stale unit to refresh. |
| Build directory | `<slot>/nested/<ticket>/build` (§4.3). |
| `CARGO_TARGET_DIR` | `<checkout>/target`, as today; the shared final-output uplift and "latest artifact wins" rule are unchanged. |
| Recipe and wrapper environment | The same `apply_storage` values, re-derived from the inherited environment; the wrapper policy is the outer's, never re-decided. A joined command cannot turn a bypassed wrapper back on or an allowed one off. |
| Child temporary directory | The outer's chosen directory, inherited and kept; the joined command creates none, removes none and never touches the outer's scratch. An unqualified inherited value refuses before the spawn. |
| Reserve / removal watchdog | **Not started.** The outer's watchdog covers the subtree; the outer's stop reaches the joined process through the group, and the joined process forwards to its own group. |
| Budgets | Its own flags may bound the joined command; the outer's window still covers the whole subtree and is never paused or reset ([Q-018](../questions.md#q-018--how-do-per-command-admission-and-execution-budgets-behave-after-freeze)). No inheritance. |
| Signals | Own handler, own child group, forwards TERM/INT/HUP like any leased command. |
| Writer ticket | Created before the spawn (§4.4). |
| `run` | The program runs inside the same joined custody: no release-then-exec (there is no lease to release), the ticket stays until the program's group is empty, and the program never takes the terminal foreground (§6.3). |
| Release, recover, evict, flush, origin | **Never.** The outer is the only releaser; the joined command writes no origin and no recorded accounting. |
| Summary line | One line naming the join and the outer slot; elapsed time and the nested command's own exit code, no pool-use line (nothing was admitted). |
| Exit code | Its own child's, unchanged. A join refusal is exit 1. |

### 4.3 The private build directory

A joined compile must **not** use the outer's build directory. Cargo takes an
exclusive per-profile lock in the build directory it uses and holds it for
the whole build, including while build scripts and test binaries run
([clean classification](workflow.md#clean) encodes the same pinned facts). A
nested compile started by a build script or a test therefore cannot acquire
the lock its own ancestor holds: sharing one build directory is a guaranteed
deadlock, not a slow path.

The joined command therefore gets
`<slot>/nested/<ticket>/build`, created by it under the slot it does not own
but is licensed to write, with its own `oxide.nested.build/1` marker
(`store_id`, `record`, `nonce`, ticket id, depth). Consequences, all
deliberate:

- It is **cold**: empty at start, so no freshness gate and no origin is
  needed, and no fingerprint is shared with the outer.
- Two joined compiles share no build directory, so neither waits on the
  other's build lock and no ancestor's lock is ever in the path; the shared
  `<checkout>/target` keeps its documented brief uplift serialization.
- It is removed by the joined command after its own group closes, before the
  ticket is unlinked. A removal that cannot be completed is reported and the
  directory is **kept**: it stays counted in the slot, is measured by the
  outer's release, and leaves with whole-slot eviction. Nothing is inferred
  from it.
- It is counted once: it lives inside the slot, so the existing
  `(device, inode)` union and the whole-slot walk include it and hard links
  are not double-counted.
- It is not reused across generations. A persistent nested build directory
  would carry units built from one checkout's absolute paths into a later
  generation of the same slot, which is precisely the cross-checkout hazard
  the [freshness gate](workflow.md#cross-checkout-freshness) exists to
  prevent. Cold-per-command is the conservative answer and is stated as a
  trade: a nested same-workspace compile repeats work that today's second
  slot might have found warm, and it does so only when the caller is already
  inside a build of that workspace.

### 4.4 The writer ticket and closure

Join adds writers to a generation whose closure the outer must decide, so it
adds one durable ticket per joined command.

**Files.** Under `<control>/nested/<record>/`:

- `gate` — the lock target, present from issuance; `closed` — the durable
  marker created once, under the gate lock, to close the generation;
  `writers/<id>` — one ticket per joined writer.

**Registration (the only race that matters).** Registration takes an
exclusive `flock` on `gate`, then checks that `closed` is still absent,
writes `writers/.tmp-<id>`, `fsync`s, takes an exclusive
`flock` on the ticket descriptor, renames it to `writers/<id>`, and releases
the gate lock. The outer's closeout takes the same exclusive `flock` on
`gate`, creates `closed`, and only then scans `writers/`.
Because both sides hold the same lock across decide-and-act, every ticket is
either visible to the scan or refused at registration. A refused
registration takes the independent path (with §6's no-wait rule where it
applies) and never waits.

**Live ticket.** The joined command holds its ticket `flock` for its whole
life and records `spawning` before the spawn and `running` with the group
after it, mirroring `mark_spawning`/`mark_running`.

**Closeout.** On exit the joined command waits for its own group to be empty,
removes its build directory, unlinks its ticket and reports. A crash leaves
the ticket (its lock released by the kernel) with its recorded phase and
group.

**The outer's decision.** After its own group is empty and before
`discard_scratch`, the outer closes the gate and reads every ticket:

| Ticket state | Outer's action |
| --- | --- |
| lock free, phase `running`, recorded group `ESRCH` | closed: remove the ticket |
| lock free, phase `registering` or `spawning` (no group recorded) | **not closed**: keep the slot held, exit 1 |
| lock free, recorded group alive or `EPERM` | **not closed**: keep the slot held, exit 1 |
| lock held (a joined `oxide` outlived the outer group, e.g. `setsid`) | **not closed**: keep the slot held, exit 1 |
| malformed, foreign, unreadable | **not closed**: keep the slot held, exit 1 |

The outer does not wait for a ticket: a live ticket outside its group can
only be an escapee, and waiting would trade a bounded failure for an
unbounded one. Held custody keeps every byte, the recorded accounting record
unpublished, and the existing recovery rule in charge.

**Recovery.** `settle` gains one conjunct: a record whose generation has any
ticket that is not positively closed is never recovered. Recovery then
requires, for every ticket, a free ticket lock **and** an empty recorded
group, plus the existing record-lock, own-lease and record-group conditions.
`oxide pool recover-interrupted` (the explicit verb) keeps its meaning for a
human decision but refuses while a joined writer's group may still run.
`unclosed_writers` gains the reasons "a joined writer's group may still run"
and "a joined writer's ticket could not be read", so removable-loss deletion
is never authorized by a join.

### 4.5 Exactly-once release

Only the record holder releases. Its release remains one call, in the
unchanged order, after the ticket gate and the closure decision. A joined
command has no `release` path, no lease ID and no record, so there is no
second releaser to race. If the record holder dies, exactly one later command
may recover the generation, and only under §4.4's ticket + group + lock rule;
the recovery is logged once by the existing pending-recovery record.

## 5. Borrow

### 5.1 Eligibility

Borrow requires V1–V8 (possession, a live generation in the **same pool**,
the same nonce, session and subtree membership, and a usable depth), plus any
key — including a different workspace, recipe, toolchain, target or
destination. V9's key comparison is unequal in the normal case; the nested
command then runs the **whole** existing admission path.

Borrow is refused when the nested command's own pool selection differs from
the token's pool (a drive removed between the outer's start and the nested
launch, a changed `OXIDE_POOL`, a config change). A different pool means a
different store, ledger and state; the design will not stitch a permit across
two stores. The command then falls back to the independent path and §6.

### 5.2 The accounting rule

`Store::attempt` computes:

```text
active = records in this pool that are active and not covered by a live permit
       + sibling pool's same figure
```

`Record.permit.parent` is the only input this design adds, and it is exactly
the triple `{ store_id, record, nonce }` the token carries in `permit_root`:

- a record with `parent: null` is a root permit holder and is always counted;
- a record is **covered** only when `parent` is present and, in the same
  locked snapshot, a record with that store and record id is active in either
  pool and its `permit.nonce` equals the referenced nonce;
- anything else — an idle parent, a freed lock, a recovered parent, a
  mismatched nonce, a missing record, an unreadable sibling state, a
  malformed reference — counts the record itself.

The rule is conservative in both directions:

- a borrowed command is admitted while its root permit is held, so nesting
  cannot deadlock `max_active = 1`;
- if the root releases (or dies and is recovered) while a borrowed command
  still runs, the next admission counts the borrower as its own active
  command. It is never free.

**Borrow fan-out.** In the same locked snapshot, a borrow is admitted only
when fewer than `MAX_BORROWS_PER_PERMIT` (8) active records in the two pools
name this exact permit reference as `parent`; the ninth refuses with exit 1
and names the limit. The count is taken under the admission lock that also
grants the borrow, so a race cannot exceed it by more than the existing
cross-pool one. Because every borrow in a tree references the **root** permit
(§3.2), the count is per outer generation and an intermediate command cannot
open a fresh allowance. Root permits and joins have their own bounds
(§6.2); the three bounds are independent.

The parent's release is unchanged and takes the same admission lock that
validates and grants a borrow, so a borrow and a parent release are totally
ordered: a permit is never duplicated and never released early. Nothing in a
borrow touches the parent's record, lease, ledger row or permit: the outer's
permit ends exactly when it ends today.

### 5.3 What a borrow never bypasses

Unchanged and mandatory for a borrowing command: pool selection and its
verification, the host reserve check, the soft budget and byte-pressure
eviction, `PROJECT_SLOT_CAP` and the pool slot ceiling, recorded per-slot
accounting and every `unknown_accounting` refusal, foreign-entry and
ownership checks, the checkout clean barrier, the freshness gate, output
adoption/reservation/binding, child temporary files, the reserve and
surprise-removal watchdogs, budgets, signals and the full release order
including flush. Only the host active-build count is substituted.

A borrow adds no deletion authority: the borrower's own admission may evict
the same idle whole slots it could evict as an independent command, and it
never touches the outer's held slot.

### 5.4 Status and observation

Default `oxide status` stays lock-free and walk-free. For a record with a
permit it prints one extra fact from the recorded field, for example
`permit borrowed from record 7 (nested depth 1)`, using `permit.parent.record`
and `permit.depth`; it does not probe tickets, PIDs or groups. `--measure` may
additionally list the generation's tickets (names, phases, groups) since it is
already the diagnostic path. A borrowed record shows as an ordinary active
slot with its own provenance; the outer's line does not claim the borrower.

## 6. Waiting, deadlock and cancellation

### 6.1 The nested no-wait rule

A command that **positively** detects it is executing inside a live Oxide
generation — the ancestor walk of V7 reaches an active record whose holder
PID and start time match, in either pool, independent of any token — and
whose next admission attempt returns one of

- `Wait::Checkout` (this checkout's busy slot is the ancestor's slot),
- `Wait::Clean` (a clean whose completion needs this checkout's lease, which
  the ancestor holds),
- `Wait::HostLimit` when every counted active command is an ancestor, or
- `Wait::ProjectCap` where the only candidate is the ancestor's slot,

**refuses with exit 1** naming the ancestor and the reason, instead of
waiting. Waiting can only be satisfied by the ancestor, and the ancestor is
waiting for this process. The same rule covers a same-key token whose
join was refused (V10 closed gate), an invalid token in a same-key nested
call, a nested `oxide clean`, and a nested `outputs adopt|detach` in the
ancestor's checkout: those refuse immediately with an actionable message
rather than queuing behind the process that spawned them.

Waits that are **not** implicated by an ancestor keep today's behaviour and
FIFO fairness: another checkout's clean, another project's cap, another
command's held slot, capacity held by unrelated builds. A budgeted command
already polls and abandons its ticket; an unbudgeted one may sit in such a
wait, and the outer's own window (if any) still covers it.

### 6.2 Depth and fan-out bounds

| Bound | Value | Behaviour |
| --- | --- | --- |
| Joined levels below one outer command | 4 | Each record-holding command issues token depth 1 for its own generation; a joined command at level `d` issues `d + 1`; a token at depth 5 (issued by the level-4 command) refuses with exit 1. Fallback to an independent slot is **not** allowed, because unbounded recursion (a build script that calls `oxide`, whose build script calls `oxide`) would otherwise consume every slot and permit. |
| Live joined writers per generation | 8 | A ninth join refuses with exit 1. The writer set is read under the ticket gate; a closed ticket never counts. |
| Live borrows per root permit | 8 | A ninth borrow refuses with exit 1. Counted under the admission lock from both pools' active records whose `permit.parent` is this permit (§5.2). |
| Ancestor hops | 64 | Beyond it: no ancestors, so no join and no borrow (independent path). |
| Token document | 4 KiB | Larger or unparsable: no join. |
| Ticket scan | 32 tickets, 4 KiB each | More tickets than the bound: the outer keeps the slot held (never silently ignores one). |

These are product constants in the reviewed implementation, not settings.

### 6.3 Budgets, signals and cancellation

- A joined command's admission happens without a slot wait, so its
  `--admission-budget` covers only its own preparation. Its execution window
  is its own. Neither extends, pauses or resets the outer's window.
- The outer's expiry stops the outer group; the joined process receives TERM
  in-group, forwards it to its own group, closes within the existing
  TERM → 2 s → KILL → finite-observation escalation and exits 125 or its
  child's status. If the joined group outlives it, the ticket keeps custody
  and the outer keeps the slot.
- Ctrl-C before a spawn keeps `128 + signal`. A joined command never spawns
  after its own admission expiry.
- A cancelled admission removes its ticket and its build directory if it
  had created one; it holds no lease, so nothing else is released.
- **Terminal foreground.** A nested command never takes the terminal's
  foreground process group: the outer leased command, or its `run` program,
  owns it (`process::start_foreground_group`). A joined or borrowing command
  starts its child as an ordinary group, and a nested `run`-class program
  whose standard input is a terminal is given `/dev/null`, exactly as
  `oxide exec` does, so nesting can neither stop on a terminal read nor
  steal the foreground from the ancestor.

## 7. Failure, race and cancellation matrix

| # | Event | Detected by | Outcome | Custody |
| --- | --- | --- | --- | --- |
| 1 | Stale token after the outer released | V5 (record idle / lease gone) | independent path | none |
| 2 | Token after the outer crashed | V6 (lock free) | independent path | the crashed generation stays for existing recovery |
| 3 | Token copied without the descriptor | V1/V2 | independent path | none |
| 4 | Descriptor present, nonce tampered | V3 | independent path | none |
| 5 | Token from another pool or checkout's store | V4 | independent path | none |
| 6 | Token presented outside the subtree | V7 | independent path | none |
| 7 | PID reuse satisfies a forged chain | V7 start-time compare | independent path | none |
| 8 | Token for another generation of the same record | V3 + V5 `permit.nonce` | independent path | none |
| 9 | Join refused because the gate closed | V10 | refuse (same key) or borrow (different key) | outer continues its closeout |
| 10 | Two joined writers start together | gate lock ordering | both register, separate build dirs | both ticketed |
| 11 | Joined writer registers as the outer closes | gate lock ordering | registration refused | fallback/refuse, no lost writer |
| 12 | Joined command crashes before spawn | ticket phase `registering` | outer keeps the slot held | ticket remains for recovery |
| 13 | Joined command crashes after spawn, before group record | ticket phase `spawning` | outer keeps the slot held | recovery via explicit `recover-interrupted` |
| 14 | Joined group outlives the joined `oxide` | ticket lock free, group alive | outer keeps the slot held | recovery requires `ESRCH` |
| 15 | Joined `oxide` escapes with `setsid` | ticket lock held, group empty | outer keeps the slot held and exits 1 | existing `setsid` residual limit, now visible |
| 16 | Outer receives Ctrl-C | existing handler | TERM to outer group; joined forwards | closure or held |
| 17 | Outer budget expiry | existing stop | TERM/KILL outer group; tickets decide | held if any ticket unclosed |
| 18 | Removable volume vanishes with a joined writer | outer removal watchdog | stop the group; tickets checked | quarantine: `unclosed_writers` names the join |
| 19 | Flush failure on release | `flush_before_idle` | slot retired, lease held | unchanged, after tickets |
| 20 | Parent releases while a borrower runs | admission-lock ordering | borrower counts itself at the next admission | never free |
| 21 | Borrower daemonizes and the parent exits | no ticket (borrow has none) | borrower's own record keeps its lease | its own recovery rules |
| 22 | Nested same-key call whose token is invalid | §6.1 | exit 1 naming the ancestor | nothing taken |
| 23 | Token depth 5 (a fifth joined level) | V8 | exit 1 | nothing taken |
| 24 | Ninth live joined writer | ticket count | exit 1 | nothing taken |
| 24a | Ninth live borrow for one permit | §5.2 count under the admission lock | exit 1 | nothing taken |
| 24b | Borrow whose parent reference is malformed or unresolvable | §5.2 coverage rule | the record counts itself | nothing taken away |
| 25 | Torn `state.json` read (V5) | one reread, then refuse | independent path | none |
| 26 | Mixed binaries: outer predates join | no token at all | today's independent lease | none |
| 27 | Mixed binaries: nested predates join | it cannot read the fenced state (hand-off or refusal), and ignores the token | hand-off/refusal, else today's independent lease | outer's window |
| 28 | Recovery of a generation with tickets | §4.4 conjunct | refused while any ticket is not closed | held |
| 29 | Joined command's build directory cannot be removed | its own closeout | report, keep the directory | measured and evicted with the slot |
| 30 | `oxide clean` in the ancestor's checkout | §6.1 | exit 1 | nothing changed |

## 8. Locks and ordering

New lock: `<control>/nested/<record>/gate`, `flock`, exclusive, held only for
one decision. Nothing holds it while Cargo or a tool runs.

| Actor | Locks held, in order | Notes |
| --- | --- | --- |
| Outer admission (unchanged) | admission → pool lock (library) → record lock acquired inside | borrow validation and grant share this |
| Outer issuance | admission → record save | permit written before any directory exists |
| Outer closeout | admission (after group empty) → gate (brief) → record save → library release | gate never held across the library call |
| Joined registration | gate → ticket flock → gate release | no admission lock, no pool lock |
| Nested child start | none | an ordinary process group; never the terminal foreground |
| Joined exit | its own ticket lock (held throughout) | unlinks ticket last |
| Recovery | admission → record lock probe → ledger → tickets | tickets after the ledger view, before release |
| Any `status` | none | passive snapshot only, never the gate |

Rules that follow and must be tested:

1. No lock is ever taken in the opposite order. The gate is innermost and
   never held while a pool or record lock is taken.
2. A joined command never takes the admission lock, so it cannot block a
   peer's admission, release, clean or recovery.
3. The record lock stays the outer's exclusivity proof; the ticket lock is
   only a joined writer's liveness proof.
4. The checkout binding mutex, clean barrier and output publication locks are
   never taken by a joined command.

## 9. Interactions with the existing fences

- **Recorded accounting.** The outer's `Held → Idle` replacement publishes
  the measured figure after tickets close, so a measured idle slot never
  includes a still-running or unknown nested writer. The joined build
  directory lives in the slot and is counted once.
- **Capacity and eviction.** A join performs no admission and therefore no
  eviction; a borrow performs the normal one. Neither can evict a held slot,
  the outer's included.
- **Removable selection and flush.** A joined command never selects, flushes
  or re-verifies a volume; it writes into the outer's already verified slot.
  The outer's flush runs after ticket closure, so the durability fence still
  covers nested writes. A joined command starts no removal watchdog; the
  outer's stop reaches it through the group, and an unclosed ticket
  quarantines instead of deleting.
- **Surprise removal.** `unclosed_writers` gains the join reasons; a
  generation with an unclosed ticket is quarantined, never deleted.
- **Owned outputs.** A joined command publishes nothing: no adoption, no
  reservation, no registry row, no link swap. Its final artifacts land in the
  shared `target` under the documented "latest artifact wins" rule. In a
  profile checkout a joined command of an unmodelled command name refuses for
  the same reason the outer would, without publishing anything.
- **Clean and prune.** A same-checkout clean refuses while nested (§6.1);
  other checkouts' cleans and prunes are unaffected, and prune keeps any root
  whose writer record is open.
- **Installed hand-off and version fence.** `Record.permit` needs the next
  `STATE_VERSION` and the existing hand-off trigger, and an older installed
  binary must be upgraded before a permit-bearing pool is served. A nested
  binary that predates join ignores the token (row 27): correct, but it may
  wait; the outer's budget bounds that.
- **Environment hygiene.** `OXIDE_LEASE_TOKEN` and
  `OXIDE_LEASE_TOKEN_FD` join the names a fixture must scrub, alongside the
  existing `OXIDE_*` set ([premise checklist](workflow.md#proof-authoring-premise-checklist)
  item 3).

## 10. Alternatives considered and rejected

| Alternative | Why it is not chosen |
| --- | --- |
| Environment string only | Copyable, stale-able and forgeable; the brief forbids it and §3.1 shows it cannot distinguish a dead outer from a live one. |
| Path-addressed secret file (readable by the child) | Any same-UID process can read or copy it, and it says nothing about the subtree; a copied file would join. |
| Live broker: the outer serves join requests on a socketpair | Strongest (the outer itself decides, with kernel peer credentials), but it adds a servicing loop to every outer wait path (`wait_leader`, `collect_run`, watchdog, budget stop) and a new blocking surface inside the outer's own critical path. It is the right design only if a hostile same-UID model is ever adopted; the descriptor + live-state + ancestry trio reaches the same three-factor property with no new service. |
| `LOCAL_PEERPID`/`SO_PEERCRED` as the primary check | Credentials are fixed when the connection is made and do not follow the current holder across `fork`/`exec`; they are usable only with the broker above. |
| Inherit the storage environment and drop the lease idea | No closure custody, no exactly-once release, no accounting validity: the outer could publish an idle measurement while a nested group still writes. |
| Give the nested command a second ledger lease on the same slot | Breaks "one owned slot generation, one holder"; the ledger has one holder and release would be ambiguous. |
| Keep today's independent lease everywhere | The storage churn and the `max_active` self-deadlock the operator asked to remove. It stays as the *fallback*, not the policy. |
| A permit object that is copied into the child | Duplicates authority and needs its own release, so a lost child frees a permit early. A derived count with a live-parent check has neither failure. |
| Permit counter decremented by the child on exit | A child that dies without decrementing leaks the permit forever; a count derived from live state cannot leak. |
| Wall-clock TTL token | Keeps authority alive after a crash and expires valid work; lifetime is the generation, not a clock. |
| Share the outer build directory | Guaranteed Cargo lock deadlock for the build-script and test cases (§4.3). |
| Per-depth persistent nested build directories (warm across commands) | Carries one checkout's absolute-path units into a later generation of the same slot — the cross-checkout hazard the freshness gate exists for. |
| Unbounded nesting | A build script that calls `oxide` recurses; depth and fan-out bounds are product constants. |
| Unbounded wait for a live ticket at outer closeout | Trades a bounded, honest held-slot failure for an unbounded hang; the custody rule already handles it. |
| Let a nested command run `clean`/`outputs adopt` by waiting | The wait can only be satisfied by the ancestor that spawned it. Refusing is the only non-deadlocking answer. |
| Mint the permit in a resident helper/daemon | A new service the operator has not approved, and the retired-daemon precedent stands ([retirements](../retired.toml)). |
| Trust a same-UID signed token (HMAC with a pool secret) | The secret must be readable by the verifying child, hence by any same-UID process; it adds a key-management surface without changing the cooperative boundary. |

## 11. Decisions taken here, and what stays open

Each row is a product choice this design makes and therefore submits to
review; none may be treated as settled by the design's existence.

| # | Decision | Rejected alternative |
| --- | --- | --- |
| D1 | Authority = nonce + inherited unlinked descriptor + live record/lease/ancestry. | String-only; path secret; broker. |
| D2 | No wall-clock TTL; lifetime is the generation. | Fixed TTL. |
| D3 | Join requires exact key equality; any difference borrows. | Same-checkout-only join; same-repository join. |
| D4 | A joined command gets a private, cold build directory in the slot and no freshness gate. | Shared build directory; persistent nested directory. |
| D5 | Join is default-on for leased commands; no user override. | Opt-in flag; opt-out flag (which could reintroduce the deadlock). |
| D6 | Only joins register tickets; borrows rely on their own record. | Ticketing borrowers and making the outer wait for them. |
| D7 | Outer never waits for a live ticket; it keeps the slot held. | Bounded wait; implicit release. |
| D8 | Nested no-wait refusal for ancestor-implicated waits. | Unbounded wait; automatic second slot. |
| D9 | Four joined levels, 8 live joined writers and 8 live borrows per root permit. | Unbounded; per-project cap reuse. |
| D10 | A join does not run `auto_adopt`, `reserve` or binding. | Replaying the outer's output transition. |
| D11 | Borrow requires the same pool/store. | Cross-pool permit stitching. |
| D12 | Refusals exit 1 with an actionable message. | A new exit code (124/125 stay the budget meanings). |
| D13 | A borrow references the **root** permit (flat, one-level coverage) and is capped at 8 per root permit. | Chained parent references with per-level caps (an intermediate command would hold an allowance open and the count would be per level). |
| D14 | Ancestry is checked together with the issuer's session id, so a `setsid` caller is refused even when its ancestry survives. | Ancestry alone. |

Still open, deliberately not decided here:

- **Deadline inheritance.** A joined command does not inherit the outer's
  remaining window; whether an inner inheritance interface should exist stays
  [Q-018](../questions.md#q-018--how-do-per-command-admission-and-execution-budgets-behave-after-freeze).
- **The idle-exec permit item.** [Vision](../vision.md#product-workflow-direction)
  also approves "idle exec sessions release their host permit" as a separate
  design. If that lands, a borrow must require a *held* permit epoch, not
  merely an active exec record; the implementation brief must state which
  lands first and re-check this seam.
- **Hostile same-UID model.** Out of scope, as everywhere in Oxide
  ([architecture](../architecture.md#storage-invariants)).
- **Platforms.** macOS and Linux only; any platform without a bounded
  parent/start-time query refuses the join and keeps the fallback.
- **Windows, containers, sandboxed macOS.** No claim. A sandbox that denies
  the ancestry or descriptor read yields the fallback.
- **Whether a joined compile should ever be warm.** Left to a future,
  separately reviewed design if the repeat cost matters.

## 12. Implementation slices and dependencies

Design-only: these slices are **not** authorized. They exist so review can
judge the shape and a later brief can be scoped.

| Slice | Content | Depends on |
| --- | --- | --- |
| S1 | Pure token codec, ticket codec, limits, ordered validation V1–V10 against injected seams (snapshot, lock probe, ancestor chain). No effects. | this design |
| S2 | Issuance in every record-holding command (top-level, independent or borrowing): `permit` with its own nonce and optional parent reference, generation directory, descriptor, environment, `STATE_VERSION` bump and hand-off trigger. | S1; installed-binary hand-off gate for any host install |
| S3 | Join: admission branch, private build directory and marker, ticket lifecycle, no gate/no origin/no release, summary and status. | S2 |
| S4 | Closure: gate protocol, ticket decision, `settle`/`unclosed_writers` extension, exactly-once release, recovery. | S3 |
| S5 | Borrow: `permit_root` propagation, permit reference, covered-record counting, the root fan-out bound, the no-wait rule and diagnostics. | S2, S4 |
| S6 | Knowledge: replace the proposed text with current truth, link the proof, record the limits. | S1–S5 reviewed |

Dependencies outside this task: oxide#146's recorded per-slot accounting is
merged and is the accounting this design extends; correction lead
`755757db`'s disposable staged workspaces are the harness route for staged
fixtures ([workflow](workflow.md#dogfooding-in-oxides-repository) records its
interim workaround); the installed hand-off gate governs any host install;
consumer adoption, host settings and real-drive qualification stay separate.

## 13. Proposed test and proof plan

### 13.1 Pure and unit coverage (no child, no pool, no image)

- Token and ticket codecs: every field cap, unknown field, unknown version,
  truncation, over-size, invalid UTF-8.
- Validation matrix V1–V10 with injected descriptor facts, `state.json`
  copies, ledger snapshots, lock probes, session ids and ancestor chains,
  including: a recycled PID with a different start time; a `setsid` caller
  whose ancestry survives but whose session does not; a token from a later
  generation; a torn `state.json`; a free record lock; an unreadable pool; a
  depth-5 token that must refuse rather than fall back.
- Counting: covered vs uncovered records across both pools, a released
  parent, a recovered parent, a mismatched nonce, a malformed or missing
  `parent` reference, an unreadable sibling, and the borrow fan-out at 7, 8
  and 9 live borrows for one root permit.
- Gate protocol: registration vs close in every interleaving (a
  deterministic two-thread unit test on the lock, not a timing test).
- Issuance: depth stamping and the level-4 → depth-5 refusal, a fresh
  descriptor per issuer carrying the same generation nonce, `permit_root`
  propagation through a joined issuer, the `permit` shape for a borrowing
  record, and a borrow starting its own generation counter.
- Decision functions: depth, both fan-out bounds, ticket state → closed/held,
  the no-wait classification.
- `STATE_VERSION` fence: previous version readable, new `permit` field
  defaulted, older reader refuses.

### 13.2 Native proof (proposed selector `rust:nested-lease`)

One short fixed script, `scripts/test-nested-lease.py`, as the whole command
of the qualified shared supervisor under heavy admission:

```text
python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 \
    --scratch-budget-mib 512 -- python3 scripts/test-nested-lease.py
```

The script stages this HEAD's debug `oxide` offline with 1.97.1 into
scratch-owned target and build directories. Its fixture **internal** pool is
a genuine `directory-budget-v1` pool, which the product correctly refuses on
an external volume, so the selector passes `--root-parent /private/tmp` (the
owner-accepted internal-placement exception, decision `bd357a4c` v4
precedent); that exception is a scoped owner decision in the implementation
brief, not something this design grants. The pool the fixtures *build into*
is **one owned 128 MiB APFS scratch image** registered as the removable
backend, so the join, flush and removal fences are real; it is attached with
`-owners on`, its custody record lives under the evidence directory, there
is one recorded ordinary detach, and the backing is removed only after a
completed detach. Fixtures use a private `HOME`, `CARGO_HOME`,
`OXIDE_POOL`, `OXIDE_CONFIG` and temp, with the invoking rustup home passed
through, all inside the guard's monitored scratch.

Cases (internal, tiny, std-only crates; each fixture checks its own premise):

1. **Join, same key.** A build script invokes the staged `oxide check`; it
   joins: no new ledger slot, no second record, no walk, the outer's permit
   count unchanged, the nested build directory inside the outer slot, the
   nested summary names the join, and the outer's release publishes one
   measured idle slot after the ticket closes.
2. **Borrow, different workspace.** The same build script invokes `oxide
   check` in a second checkout: it takes its own slot, its record names the
   borrowed permit, and with `OXIDE_MAX_ACTIVE=1` the outer's permit is not
   exceeded and no wait occurs.
3. **Stale token.** The outer exits; the token and descriptor are exported
   to a later standalone `oxide check` in the same tree: it takes an
   independent lease (a new slot) and prints no join.
4. **Leaked token.** A double-forked helper that leaves the subtree (its own
   session, reparented) re-execs `oxide` with both variables and the
   descriptor: no join, independent path.
5. **Tampered/fake token.** An unrelated file as `FD`, a wrong nonce, a
   token for another pool: no join, independent path, nothing written.
6. **Depth and fan-out.** A recursive build script succeeds through four
   joined levels and the fifth refuses; no more than eight joined writers are
   live at once; a ninth borrow for one permit refuses while eight are
   admitted.
7. **Exactly-once release.** Two joined writers, one killed un-reaped: the
   outer keeps the slot held, `status` names it, the next command recovers it
   only after the group is `ESRCH`, and exactly one recovery entry is logged.
8. **Flush fence.** Join into a removable slot: the outer flushes only after
   every ticket closes; the joined command never flushes; a forced flush
   failure retires the slot with the lease held.
9. **Surprise removal.** With a joined writer held, `hdiutil detach -force`
   of the image's own positively bound device: the outer stops its group, the
   slot is quarantined (never deleted) because a ticket is unclosed, and the
   next command falls back internally.
10. **No-wait refusal.** A nested `oxide clean` (or an invalid-token
    same-key call) exits 1 naming the ancestor instead of hanging; the
    outer's group then closes normally.
11. **Grandchild join.** A joined command's own build script (second level)
    validates the derived descriptor and joins: it is ticketed at the outer
    generation, the summary names depth 2, the outer's closeout covers both
    tickets, and a level-4 issuer's depth-5 token refuses rather than
    falling back to an independent slot.

Bounds: 300 s including staging and cleanup; 512 MiB monitored scratch
including the 128 MiB image; ordinary detach only; the supervisor owns
closure, the deadline and cleanup. The fit is **UNMEASURED**; if staging plus
eleven cases cannot fit, the implementation brief's scoped amendment raises
the limit *before* activation, never during a run.

### 13.3 Gates, tiers and the nine premises

- **Tier: HIGH.** The brief declares HIGH and applies the fixed
  [premise checklist](workflow.md#proof-authoring-premise-checklist).
- **Tier: ELEVATED for S3–S5.** A join changes *writer closure* and a borrow
  decides which command may be admitted against a permit; S4 also changes
  crash recovery, and this proof writes to removable media. The trigger is
  stated in the brief, not inferred from fixture isolation. One author
  native and one **distinct independent reviewer** native are separately
  owner-activated.
- **Diagnostic before counting.** The script carries an opt-in
  `--diagnostic` keep-going collection: it records recoverable oracle
  failures, skips only dependency-tainted cases, aborts on an unknown child,
  ticket, custody, device, protocol or deadline outcome, and writes a
  distinctly `qualified: false` receipt that is never a PASS. It is a
  separate activation from the counted run.
- **No native run, activation or resource is granted by this document.**

| Premise (fixed checklist) | Proposed disposition for `test-nested-lease.py` |
| --- | --- |
| 1. Private HOME and rustup | Capture the invoking `RUSTUP_HOME` before replacing `HOME`; pass it to staging and every fixture child; `RUSTUP_AUTO_INSTALL=0`; every `--locked` call only on the repository or a fixture with its own established lock. |
| 2. Account-home control | `OXIDE_TEST_HOME_FROM_ENV=1` on every fixture child with a private `HOME`. |
| 3. Environment isolation | Scrub `OXIDE_POOL`, `OXIDE_CONFIG`, `OXIDE_TEST_*`, `XDG_*`, and the new `OXIDE_LEASE_TOKEN*` names before setting the fixture's own values. |
| 4. Automatic adoption | Not applicable: the proof registers the removable backend and converts no checkout; any case that would adopt first creates and checks its own premise. |
| 5. Cargo external argv | A fake external subcommand case, if used, expects the subcommand name as its first argument. |
| 6. Owned images | `hdiutil attach -owners on`; custody record under evidence before attach; one recorded ordinary detach; backing removed only after a completed detach. |
| 7. Compiler flags | Freeze `CARGO_ENCODED_RUSTFLAGS`/`RUSTFLAGS` consistently for staging and fixtures; an inherited encoded value never overrides the declared flags. |
| 8. Temporary storage | `OXIDE_TASK_SCRATCH_ROOT`, `TMPDIR`, `TMP`, `TEMP` all one fresh canonical directory under the supervisor's scratch; the image lives inside the same monitored root. |
| 9. macOS `mktemp` | Supply a template under that scratch parent and verify the fixture used it. |

The source sweep must trace these through each case body and every child
environment; it complements the uncounted diagnostic and cannot replace it.

## 14. Supported, unsupported and unknown

**Supported by this design (once implemented and proven):**

- A nested leased `oxide` in the same workspace/recipe/toolchain joins, with
  no new slot or permit, extended closure and unchanged storage safety.
- A nested leased `oxide` anywhere else takes its own slot and borrows the
  outer permit, without bypassing any admission, budget, ownership or flush
  rule.
- Stale, copied, tampered, foreign-pool and out-of-subtree tokens fall back
  to independent behaviour; ancestor-implicated waits refuse instead of
  deadlocking.
- Crash, signal, budget-stop, removable-loss and recovery custody for joined
  writers, with exactly-once release.

**Unsupported (refused, not silently degraded):**

- Join or borrow deeper than the depth bound, beyond either live-writer or
  live-borrow fan-out bound, without a qualified inherited temporary
  directory, or from a platform that cannot supply ancestry.
- `clean`, `outputs adopt|detach|prune`-class mutations, `pool *`,
  `recover-interrupted` and `init`/`new` joining or borrowing; same-checkout
  `clean`/`adopt`/`detach` while nested refuse.
- Any claim about hostile same-UID code, containers, Windows, or sandbox
  denial paths beyond the stated fallback.

**Falls back rather than refusing:** a token whose pool differs from the
nested command's selected pool, and any stale, copied, tampered or
foreign-generation token. These take the existing independent path, with
§6.1's refusal only where an independent wait would be satisfied by the
ancestor.

**Unknown (measured later, not assumed):**

- The real cost profile: staging plus cases against a 300 s bound; the cold
  nested compile's time and bytes; a real Tauri `dev` session's nested
  calls.
- Whether [idle exec permit release](../vision.md#product-workflow-direction)
  lands first and changes the borrow's permit-epoch check.
- Whether `tauri` honours `CARGO_BUILD_BUILD_DIR` at all (already unverified
  for [oxide exec](workflow.md#oxide-exec-cargo-wrapping-tools)).

## 15. Non-claims

This design claims no implementation, no native evidence, no
qualification, no install, no pool or drive interaction, no consumer change
and no performance benefit. It does not prove any Cargo behaviour beyond the
pinned facts cited in §2, does not make Oxide a containment mechanism, and
does not weaken exclusive ownership, conservative recovery, idle-only
eviction, recorded accounting, flush ordering, output custody or the
operator's ["no live cleanup without scoped approval"](../vision.md#authority-boundaries)
rule. Every bound, constant and case list above is a proposal for review,
not a validated number.
