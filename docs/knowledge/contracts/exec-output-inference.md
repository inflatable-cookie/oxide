# Oxide exec output-member inference (proposed design)

Status: S1 and S4(b) implemented by oxide#160 (queue brief `d2242cbb…`,
operator decision `566dba6f`, Standard tier with one counted author native
and the reviewer native waived); S2, S3, S4(c) and S5 are not built and keep
their own briefs and gates. The design was frozen at Oxide `ce410e57` after
the independent exact-head review (decision `95b3bc63`) and written against
`a7ed41b`. The operator direction is Tom's design-first ruling of
2026-10-26 (Oxide Supervisor lead `d30b48c8`, parent direction `d78a7afa` /
`a96712b`). As published, this document activated nothing; the implementation
is in `src/bin/oxide/outputs.rs` (`exec_declaration`, `exec_members`, the
shared `adopt_profile_from`) and `src/bin/oxide/main.rs`, with the
`effigy rust:exec-output-inference` proof.

This document is the design for one prerequisite: **`oxide exec` must not
need `--output`**. Today a checkout that places outputs at profile members
refuses an `exec` that declares none ([workflow](workflow.md#oxide-exec-cargo-wrapping-tools)).
The approved successor makes profile members the automatic first-conversion
default and keeps `target` a real directory for tools that mount it
([vision](../vision.md#product-workflow-direction)); that default cannot ship
while the primary exec tool still needs a hand-written declaration.

Owning files this design must agree with: [vision](../vision.md) (direction,
authority, validation policy), [architecture](../architecture.md#cargo-boundary)
(storage invariants and the Cargo boundary), [workflow](workflow.md) for the
`oxide exec` contract, the
[whole-target placement](workflow.md#final-outputs-on-the-permanent-drive-oxide117)
and the [profile-member placement](workflow.md#owned-output-members-at-profile-directories-oxide133)
it succeeds, [questions](../questions.md) for the open dispositions this
design records but does not settle, and [retirements](../retired.toml) before
reusing any name.

## Outcome

**What this design achieves.**

1. `oxide exec -- <tool>` runs in a profile-layout checkout with **no
   `--output` declaration**, in every receipt state a command may run in
   (committed, detached, absent; a prepared, pruning or pruned receipt still
   refuses under the existing custody rules) and in every drive mode.
2. The tool's profile-directory writes go through the member links this
   checkout's committed receipt already owns — no new link, no reservation, no
   conversion and no deletion is performed by `exec`.
3. The slot key still names the exact owned placement the command may use, so
   a warm slot is never reused across a changed member layout or root.
4. `oxide exec --output <member>` keeps its present parsing, validation,
   "complete declaration" semantics and byte-identical slot key. It becomes
   optional, not removed: no consumer recipe changes in this design.

**What this design does not achieve, and reports instead of hiding.**

- A member the receipt does not already place (a custom profile directory, a
  target triple this checkout has never built, or a tool that builds through a
  different workspace or target directory) is **not** placed by `exec`. It
  stays a real directory on the checkout's own filesystem. That is correct and
  safe, but it is not the storage outcome Tom's no-`--output` goal implies, and
  the design says so in [the reported mismatch](#the-reported-mismatch).
- Oxide cannot prove what an opaque tool's Cargo will write before it writes
  it. This design therefore never *derives* write authority for a tool; it
  binds the authority the checkout already holds and refuses, rather than
  guessing, when the live layout contradicts it.

The alternative reading — "ask Cargo for the tool's plan first" — is not
feasible on the supported toolchains. [Feasibility](#feasibility-what-cargo-can-actually-report)
records the source evidence; [alternatives](#alternatives-and-why-they-are-rejected)
records each rejected shape and why.

## What a member is, and what it is not

A **final output member** is one profile directory under the checkout's real
`target`, in the four v1 forms `debug`, `release`, `<triple>/debug` and
`<triple>/release` (`src/bin/oxide/layout.rs`, `MAX_MEMBERS` 8 per command,
`MAX_RECEIPT_MEMBERS` 32 per receipt, `[A-Za-z0-9_.-]+` components, no `.`,
`..`, absolute or escaping component). Only these become Oxide-owned links; the
root carries the payload and `<checkout>/target` itself stays real.

Four things that are **not** members and are not placement authority:

- **Cargo intermediates.** With the shipped split Cargo writes intermediates to
  `CARGO_BUILD_BUILD_DIR` (the held slot) and only final artifacts to
  `CARGO_TARGET_DIR/<profile>/`. `apply_storage` fixes both for the tool's
  children (`src/bin/oxide/main.rs:233-253`: `CARGO_BUILD_BUILD_DIR` = slot,
  `CARGO_TARGET_DIR` = `<checkout>/target`, `CARGO_BUILD_TARGET_DIR` removed).
  Host-side artifacts (build scripts, proc macros) are intermediates; they are
  slot payload, never a member.
- **Cargo's own top-level target entries.** `CACHEDIR.TAG`,
  `.rustc_info.json`, `target/tmp`, `target/doc`, `target/package` and
  `target/<triple>`'s real ancestor directory stay real
  ([workflow](workflow.md#owned-output-members-at-profile-directories-oxide133)).
- **The intermediate slot or its build directory.** Nothing about a slot is an
  owned output member.
- **A link.** A member *link* is evidence; the destination subpath under a
  positively owned root is the authority. A member-shaped link that this
  checkout's marker does not own is foreign.

## Feasibility: what Cargo can actually report

### Stable and unavailable interfaces

The supported toolchain range is stable Rust 1.95 and later, qualified per exact
pin ([vision](../vision.md#toolchain-support)); this repository pins 1.97.1
(`rust-toolchain.toml`). On that range:

| Interface | What it reports | Verdict for this design |
| --- | --- | --- |
| `cargo metadata --no-deps` | `target_directory`, `build_directory`, `workspace_root`, workspace packages; **no profile output directories** | useful for *validating* the Cargo context, insufficient for a member set |
| `cargo metadata` (resolving) | adds the resolved graph, after dependency resolution | same gap, plus resolution side effects |
| `cargo build --unit-graph -Z unstable-options` | serialized units incl. `profile` | **nightly only**, and it resolves, probes `rustc`, may write the lock and download before emitting |
| `--message-format=json` `compiler-artifact` | per-unit artifacts, `target`, `profile`, `filenames` | emitted *for every compilation step*, i.e. after the member directory exists and a unit ran |
| `cargo build --build-plan` | removed from stable Cargo | unavailable |
| a tool's own `--help`/plan flag | tool-specific and not Cargo's | outside Oxide's boundary (no per-tool knowledge) |

Neither Cargo's CLI nor `cargo_metadata`'s stable schema yields "the profile
directories this command will write" before it writes them. The nightly unit
graph is the only command-specific plan, and even it is not non-executing: the
single-probe evidence record
[`cargo-unit-graph-and-tool-fit-041.json`](../evidence/cargo-unit-graph-and-tool-fit-041.json)
traces `compile_ws` → `create_bcx` → `resolve_ws_with_opts` (lockfile,
downloads) and `RustcTargetData::new` (`rustc --print=…`) before the emit, and
the emit then returns before `BuildRunner`.

### Evidence ledger (pinned Cargo `c980f4866`, the 1.97.1 pin this repository already cites)

| Claim | Source |
| --- | --- |
| `--unit-graph` requires unstable nightly; stable refuses it | `src/cargo/util/command_prelude.rs:818,823-826` (`fail_if_stable_opt("--unit-graph", 8002)`) |
| A metadata call can avoid resolution: `--no-deps` builds no resolve graph | `src/cargo/ops/cargo_output_metadata.rs:35-40` |
| Metadata reports `target_directory` and `build_directory` only, plus workspace facts | `src/cargo/ops/cargo_output_metadata.rs:51-56`; documented `src/doc/src/commands/cargo-metadata.md` |
| Cargo's profile-directory names are `dev`→`debug`, `test`→`debug`, `bench`→`release`; any other profile uses its own name | `src/cargo/core/profiles.rs:119-126` (`predefined_dir_names`), `:349-354` (`get_dir_name`: `dir_names` entry else the requested profile name) |
| A profile cannot rename its directory: "directory names are tied to the profile name for custom profiles" | `src/cargo/util/toml/mod.rs:2545-2557` |
| Artifact messages are post-compilation: "For every compilation step, a `compiler-artifact` message is emitted" | `src/doc/src/reference/external-tools.md:144-146` |

Sources were read from `raw.githubusercontent.com/<commit>/<path>`, one GET per
path, in the same evidence style as oxide#041: source inspection only, no Cargo
run, no probe, no build, no clone.

**Conclusion.** Members cannot be *inferred from a Cargo plan for the tool's
own request* before that request executes. A design that claims otherwise
would have to either execute the tool's build first (too late for the key,
reservation and binding), run a nightly/unstable plan (outside the supported
range and still not non-executing), or rewrite the tool's argv (changes the
tool's observable behaviour). This design therefore derives the member set from
the checkout's **durable, Cargo-shaped committed placement** and treats the
tool's own request as unobservable within the cooperative boundary.

## The design: the committed-placement plan

### Member derivation

For `oxide exec` with **no** `--output`, Oxide derives **one keyed member set**
and separately **classifies** every candidate member-shaped entry. The two sets
are deliberately different:

- the **keyed plan set** is exactly the receipt's placements (`debug`,
  `release`, `<triple>/debug`, `<triple>/release`, selected or unselected). It
  is the only set planned, keyed and bound, it is by construction a **subset of
  the receipt**, and it never grows during the session;
- the **classified set** is every candidate member path that exists under
  `target`. It is read-only validation: it can only **refuse**, never plan, key,
  convert or grant. A real directory the receipt does not record is local and
  path-stable, so it is not placed and not keyed.

| Live entry at a candidate `target/<m>` | Plan | Authority |
| --- | --- | --- |
| receipt places `m` as selected; the entry is an owned link to `<root>/<m>` | planned owned link (keyed) | used |
| receipt places `m` as selected; the entry is anything other than the exact owned link to `<root>/<m>` (missing, dangling, a real directory, another destination, unmarked/foreign) | not planned | **refuse** before Cargo (the existing binding and P4 rules); `exec` follows nothing and replaces nothing |
| receipt records `m` unselected; the entry is a real directory | planned `selected = None` (keyed `real <m>`) | kept local, one bounded notice (existing grammar) |
| receipt records `m` unselected; the entry is a link or a file | not planned | **refuse**: it contradicts the receipt |
| receipt does not record `m`; the entry is a real directory | not planned, not keyed | ignored: local, no authority |
| receipt does not record `m`; the entry is a link (owned or not) or a file | not planned | **refuse**: a link outside the receipt is a contradiction, and `exec` never writes through a link it does not own (`TargetState::Foreign` for a non-directory, `outputs.rs:1395-1396`) |
| absent | not planned | if the tool creates it, it is real and local |

Everything not in the table stays untouched: `target` itself, `CACHEDIR.TAG`,
`.rustc_info.json`, `tmp`, `doc`, `package`, a `<triple>` ancestor directory
and any entry that is not one of the four member forms.

**Candidate enumeration (the shipped traversal, no payload walk).** Candidate
member paths are enumerated with the traversal `migration_members` already uses
for this same job (`src/bin/oxide/outputs.rs:2107-2164`, unit case at
`:8059-8096`), reused rather than reinvented:

- `debug` and `release` are matched as **member names first**, so a real or
  linked `target/debug` is never treated as a triple ancestor and its payload is
  never listed. The order matters: `layout::plain_triple("debug")` is true
  (`layout.rs:157-164`), so a scan that tested triples first would walk a
  profile payload;
- `file_type()` is taken **without following a link**, so a linked entry is
  never classified as a directory and never descended;
- only a positively identified plain-triple directory contributes one bounded
  second component (`debug`, `release`), and the joined name is re-validated by
  `Member::parse`; there is no third level and no other profile name;
- the result is sorted, deduplicated and capped (`MAX_RECEIPT_MEMBERS` 32).

This design adds one step, which grants nothing: for each candidate path it
takes `symlink_metadata` and, for a link, `read_link` — the same nofollow pair
`link_state` already uses (`outputs.rs:5303-5349`). Reading a link's own text
and its own metadata is **not** following it: no link *target* is resolved,
opened, read or descended here. An unreadable entry, a read error and a
non-directory at a candidate path are unknown, never permission, so the command
refuses. Nothing is created and nothing is written.

This derivation is *Cargo-derived* in a precise sense: the member names are
Cargo's own output-directory grammar (`dev`/`test`→`debug`, `release`/`bench`
→`release`, custom profile → its own name, prefixed by a plain triple when one
is passed) applied to the placement this checkout's receipt already holds. It
contains no per-tool knowledge, no guess about which profile a tool will
choose, and no invented declaration.

### Where members become trustworthy (ordering)

The current ordering is `declaration → reserve → planned destination → slot key
→ lease → bind → spawn` (`src/bin/oxide/main.rs:420-520`). This design keeps it
and moves the *source* of the declaration, never its position. Candidate trust
points and their dispositions:

| Trust point | Trustworthy? | Disposition |
| --- | --- | --- |
| **T0** durable committed placement, before reservation | yes: durable, validated, Cargo-shaped, already the binding authority | **chosen** |
| T1 `cargo metadata --no-deps` on the workspace before the tool starts | no: reports the target/build directory, not a member set | not a trust point; used only by the rejected guard below |
| T2 the tool's first nested Cargo invocation (launcher interception) | for that invocation only, and only *after* the slot was keyed, the lease taken and the tool started | rejected, see [alternatives](#alternatives-and-why-they-are-rejected) |
| T3 Cargo's own artifact messages | post-write | reporting only, never authority |

T2 is the reason a plan-first design cannot be slotted earlier: the member set
enters the slot compatibility key (`outputs-3` lines,
`src/bin/oxide/outputs.rs:5162-5185`), and the key must be resolved before the
lease. Discovering members later would either key the slot without members (and
then silently reuse a warm slot across a changed layout, which is what the key
exists to prevent) or require a deferred-key custody inversion plus a
re-bind/re-plan loop — a much larger change to the lease, reservation and
binding barriers than this prerequisite needs.

### Slot key

The keyed set renders exactly like any other planned set: one
`outputs-3 <nonce> <kind> <root> <volume> g<generation> <member>` line per owned
member and one `real <member>` line per receipt-recorded unselected member,
sorted (`destination_facts`, `outputs.rs:5162-5185`). No new grammar, no new
prefix and no version bump: an `exec` that derives the same set a declared or
modelled command would declare produces the **same** key and can share its warm
slot. Whole-target (`outputs-2`) and unconverted keys are unchanged; an absent
or detached receipt adds no destination lines.

Consequences stated openly:

- The keyed set is the receipt's placements and nothing else, so no exec key
  depends on what a previous tool run wrote. An unrecorded real member the tool
  creates stays local; it is not placed, not keyed and not converted, and a
  later conversion is an explicit adopt or declared route that changes the
  receipt and therefore the key.
- A derived `exec` set normally contains the host pair while a modelled command
  keys only the profile it builds, so they do not share slots. That is the
  existing conservatism of the member key, not a new rule.
- A receipt that places more than eight members cannot be bound by one command
  (`MAX_MEMBERS` 8): the command refuses before Cargo and names
  `oxide outputs detach`. Whether a derived set should ever exceed the declared
  cap is an open question ([questions](../questions.md)).

### Binding and classification

Binding is unchanged in substance: under the checkout's binding mutex the
command validates the exact surviving reservation, the receipt generation, the
root marker, kind, volume, device and ancestry, and every keyed destination,
excluding only its own record. The classified set adds fail-closed validation,
all **before** Cargo and before any authority write:

1. every planned owned member must already be this checkout's owned link at
   `<root>/<member>` — binding's existing per-member check, unchanged;
2. a candidate member path outside the plan that holds any link (owned or not)
   or a non-directory entry refuses the command, so `exec` never writes through
   a link it does not own and never removes or replaces one;
3. a receipt state that is not `Committed` or `Detached` (a prepared, pruning
   or pruned receipt) refuses (existing rule).

`exec` performs **no** reservation (`outputs::reserve` is skipped: the keyed set
is a subset of the receipt, so no root nonce and no member is new), never
relinks, never converts a real directory, never deletes and never creates a
member. The classified set is validation only: it cannot add a member, extend
the key or authorize a write. There is no new "authority on refusal" path — a
refusal leaves the link, receipt, root, marker, registry and slot exactly as
they were (oxide#152's rule).

### Explicit `--output` (kept)

`oxide exec --output <member>` keeps today's behaviour byte for byte: parsing,
the four-form validation, duplicate/prefix/cap refusals, and a **complete**
declaration (a caller who declares a set declares all of it). It still enters
the key as today, still reserves a future root, and still creates a new member
link when the member is absent. It becomes optional:

- **not deprecated in code or CLI**, no warning, no behaviour change;
- documented as needed only when a member must be placed that the receipt does
  not already place (a custom profile directory, a triple the checkout has
  never built, or a deliberate subset override);
- consumer selectors that pass it keep working unchanged; deleting it is a
  separate, later decision with its own brief.

### Detached, whole-target and unconverted checkouts

| Checkout state | `exec` without `--output` |
| --- | --- |
| profile receipt, `Committed` | derived plan; member links used, real members kept local |
| profile receipt, `Detached` | no owned destination: runs locally, real directories, no link used. Today this refuses (`src/bin/oxide/outputs.rs:4780`); the design removes that over-refusal by planning nothing when no destination is desired |
| profile receipt, `Prepared`/`Pruning`/`Pruned` | refuse (existing custody rules); a pruned receipt names `oxide outputs adopt` |
| whole-target receipt | unchanged: the whole target is the destination, the declaration is not consulted (`outputs-2` key) |
| no receipt (never converted) | unchanged: ephemeral behaviour, no plan, no key destination |
| no profile receipt but a member-shaped foreign link | refuse: an unconverted checkout's `target` must be real |

## Supported and unknown cases

### Supported

| Case | Behaviour |
| --- | --- |
| `tauri dev`-shaped loop: many nested `cargo build` calls in one session | first invocation warms the slot, later ones reuse it; all write `debug`, which the receipt places; the lease is held to group closure (existing #107 rules) |
| `tauri build`: the tool runs `cargo build --release` | writes `release`; placed when the receipt places it (explicit adopt links the host pair) |
| a tool that builds both profiles in one session | both are planned and keyed; no re-bind |
| `cargo run`, `cargo test`, `cargo bench` and `--profile dev|debug|test` / `--profile release|bench` inside a tool | Cargo maps them to `debug` / `release`; both are covered by the committed host pair |
| a tool that spawns a helper which itself runs Cargo (a packaging script, a Node or Python driver) | the helper inherits the same environment, target and build directory; the same derived plan covers it and the lease covers the whole group. A nested `oxide`, which takes its own lease today, is a separate approved direction |
| a tool that also runs side tasks (a dev server, a database migration step) | they write outside `target`; the derived plan is unaffected and the child-temporary rules (#136) are unchanged |
| a cross build for a triple the checkout has already built and placed | that `<triple>/<profile>` member is in the receipt and is planned and keyed |
| tool leaves a member untouched | nothing is created, converted or reported |
| an ordinary child that outlives the tool | lease stays held until the group closes (existing) |
| checkout path containing spaces, non-UTF-8 arguments | no new path parsing; member names never contain spaces; notices use the existing bounded escaping |
| `build.rs`, proc macros, host artifacts | intermediates in the slot; no member involvement |
| `oxide clean`, `outputs detach`, `outputs prune`, `outputs adopt` during or after the session | existing barriers, custody and refusals; unchanged |
| legacy whole-target checkout | unchanged |

### Unknown, refused or reported

| Case | Disposition | Report |
| --- | --- | --- |
| the tool writes a member the receipt does not place (custom profile `staging`, a new triple, `target/<other>`) | builds locally as a real directory; no placement, no refusal | one bounded notice naming the directory and the exact remedies: clear the real directory, then `oxide exec --output <member>` (or a modelled build for that member) links it; `oxide outputs adopt` converts only an *empty* member directory |
| the tool sets its own `CARGO_TARGET_DIR` or passes `--target-dir` | final outputs leave `<checkout>/target`; intermediates still in the slot | documented cooperative limit (#107); not detected by this design |
| the tool overrides the build directory (`CARGO_BUILD_BUILD_DIR`, `build.build-dir`) | escapes Oxide's intermediate storage | documented limit; not detected |
| the tool builds another workspace, toolchain or project | outside this checkout's placement | documented limit |
| a tool daemonises or `setsid`s | outside Oxide's group closure | documented limit |
| a tool unlinks or replaces its own member links | detected next time: binding refuses and names `oxide outputs adopt` | refusal on the next command |
| a member-shaped foreign link exists | refuse before Cargo | refusal names the path and the remedy |
| a checkout placing more than eight members | refuse before Cargo | `oxide outputs detach` |
| a custom profile directory written by the tool | real local (its Cargo directory name), no vocabulary extension in this design | notice, as above |
| JSON target specification / configured `build.target` used by the tool | its member is not modelled; it stays real local | notice; no guessing |

### The reported mismatch

Tom's no-`--output` goal is **met for running**: no exec route refuses for
want of a declaration, and the primary consumer's host-pair writes are placed.
It is **not met for placement** in the three cases above (custom profile, an
unplaced triple, a tool that leaves the target directory). This design reports
that boundary instead of closing it with a guess, a permanent opaque fallback
or an extra authority grant. The operator can accept it (which is what the
primary consumer needs) or ask for one of the follow-ons in
[implementation slices](#implementation-slices-and-dependencies).

## Alternatives and why they are rejected

**A. Nightly `--unit-graph` plan per tool invocation.** Rejected: outside the
supported stable range (`fail_if_stable_opt`), requires knowing the tool's argv
first (the thing we lack), and is not non-executing even then — it resolves,
probes `rustc`, may write the lock and download
([041 evidence](../evidence/cargo-unit-graph-and-tool-fit-041.json)). It would
also make a nightly channel a prerequisite for a routine dev command.

**B. Infer members from `cargo metadata` alone.** Rejected: metadata reports
`target_directory` and `build_directory`, never a profile output directory, and
the member set depends on the *tool's* per-invocation `--profile`/`--target`.
Deriving `debug` or `release` from the exec recipe instead would be the
hardcoded profile-name heuristic this design forbids — `exec --profile quick`
selects a slot recipe, not the tool's build profile.

**C. Force `--message-format=json` on the tool's Cargo and read
`compiler-artifact`.** Rejected: it rewrites the tool's argv and stdout (tools
parse human output), and every message arrives *after* the unit compiled, so
the only thing it could authorize is a post-write conversion of a real
directory — which the profile contract forbids and `outputs adopt` already
refuses for nonempty members.

**D. Interpose a `cargo` launcher (PATH and/or `$CARGO`) and derive each
nested invocation's plan.** Rejected for this prerequisite, and recorded as a
follow-on only if the published report proves insufficient: coverage is
incomplete (a tool may call Cargo by absolute path, through rustup, or through
its own bundled toolchain), it changes the tool's environment and process
shape, the product direction defers `cargo` PATH shims, and the launcher cannot
precede the slot key without a deferred-key and re-bind inversion (T2 above).
Deriving a profile name from an observed argv would also have to model Cargo's
config layer (`build.target`, `[profile]` names, `--config`) to stay honest,
which is a second configuration resolver inside Oxide.

**E. Pre-link a Cargo-derived superset (both host profiles plus every triple).**
Rejected: it creates owned members and roots the receipt never committed,
needs an unbounded triple enumeration, and still cannot cover a request it
cannot see; it would also create empty owned directories that no adoption
chose, breaking "only explicit adoption changes placement".

**F. Observe after the run and adopt/move what the tool wrote.** Rejected:
moving a real nonempty member directory is exactly the implicit migration the
profile contract refuses, and it would need a live-output journal, a writer
barrier inside the tool session and a post-hoc relink — a larger custody change
than the problem.

**G. Keep requiring `--output`.** Rejected: it fails the operator's goal and
over-refuses a detached checkout that has nothing to guess.

**H. Give exec the whole-target layout while the profile default is active.**
Rejected: it reintroduces the whole-target link (`target` must stay real for
mounted-target tools), contradicts the approved direction and would need a
relink inside the session.

## Failure, race and cancellation matrix

Every row is decided before Cargo starts, or is an existing rule that this
design does not change. "Refuse" always means: nothing is written, the link,
receipt, root, marker, registry, reservation and slot are unchanged.

| Event | Decision |
| --- | --- |
| receipt unchanged, links valid | plan, key, lease, bind, run |
| receipt changed while queued (adopt, detach, relink, migration) | binding refuses the keyed plan: "run the command again"; no automatic retry |
| `target` itself became a link | refuse (unowned/whole-target link in a profile checkout) |
| member link dangling, wrong destination, unmarked root, another checkout's root | refuse, naming `oxide outputs adopt` |
| member path holds a file or unknown entry type | refuse |
| real nonempty member directory appeared between plan and bind at a **planned** member path | binding refuses "not the keyed placement" |
| `oxide outputs prune` intent unsettled for this checkout | refuse (existing) |
| unknown writer custody in either pool | refuse any relink or clean for the checkout; `exec` itself only reads and binds |
| second exec in the same checkout | second-slot rule: own idle compatible slot or a new one; the busy slot is never reused, released or relabelled |
| drive removed during the session | existing removal watchdog stops the group, fails the command and quarantines an unconfirmed group; the link is never swapped and no root is copied or deleted |
| Ctrl-C before spawn | existing signal status, nothing started |
| Ctrl-C during the tool | forwarded to the group; lease held to group closure |
| tool exits nonzero | the tool's exit status; a reserve stop still fails the command (#107) |
| tool leader exits, group member survives | lease held until the group is gone; crash recovery unchanged |
| `oxide` killed, tool group lives | slot held and unused; recovered only once the group is gone |
| tool unlinks its own member link | next command refuses and names `oxide outputs adopt`; no silent re-creation |
| admission or execution budget expires (#140) | existing 124/125 semantics; the derived plan changes nothing about them |
| tool writes an unplaced member | local real write; slice 2 reports it once |

## Contract, format, compatibility and migration boundaries

- **No persisted format changes.** Receipt `oxide.outputs.transition/3`,
  registry `oxide.outputs.registry/2`, root/2 marker, state version 4 and the
  `owned_outputs` fence are untouched. No new file, key or journal.
- **No key grammar change.** `outputs-3` / `outputs-2` / `real` lines only.
  Declared `exec` keys are byte-identical to today's.
- **No migration and no implicit conversion.** Existing whole-target
  checkouts keep today's behaviour until an explicit `oxide outputs adopt`
  (prerequisite c). Real nonempty members are never moved.
- **`--output` compatibility.** Complete-declaration semantics retained; a
  later deprecation is its own decision and must not silently weaken it.
- **Cargo context preserved.** `exec` makes no Cargo call of its own for the
  derived plan, so build-script execution, lockfile writes, network access and
  target mutation cannot be introduced by it. The tool's exact argv, working
  directory, exit status, `--locked`/`--frozen`/`--offline` behaviour,
  compiler identity and rustc-wrapper policy are unchanged; the only
  environment it inherits is the same `apply_storage` environment as today.
- **Barge boundary.** Interpreting Cargo's layout stays Oxide's; nothing here
  asks Barge to understand Cargo. Placement primitives remain Barge's later
  supply, and this design neither blocks nor presumes them.
- **Adjacent directions kept separate.** Nested-lease joining, idle-exec host
  permits, host permits/slot accounting and the fresh-root exact-nonce rule
  (oxide#152) are boundaries this design must not fold in; the derived plan
  reserves nothing, so it cannot conflict with #152's authority rules.

## Implementation slices and dependencies

**Dependencies (observed at `a7ed41b`).** Brief prerequisites: the auto-adopt
eligibility correction (`755757db`) and the Finder `ENOTEMPTY` correction
(`6177673f`) must have landed — verify before the implementation brief. Shipped
dependencies: profile-member placement (oxide#133) including detach (#144),
no-authority-on-refusal and future-root nonce (#152), child temporary files
(#136, which every `exec` child already gets), command budgets (oxide#140,
which an `exec` may carry), and the currently qualified shared supervisor
(oxide#151, `bc24b238…`) for the proof. Not prerequisites: nested-lease
joining, idle-exec permits, Barge's placement primitives, recorded per-slot
accounting, custom-profile vocabulary.

**S0 (this task).** This design document, its links, and one open-questions
entry. No code.

**S1: derived exec plan.** Small and reviewable:

1. a receipt-derived keyed member set for `exec` without `--output`, reusing
   the shipped `migration_members` traversal and `link_state` for the candidate
   classification (no new traversal rule, no payload walk, no link target
   followed), plus `Member`/`Declaration` for the plan. The classification runs
   for every `exec`; the receipt only decides the keyed set;
2. `planned_destination` plans nothing (instead of refusing) when a profile
   receipt desires no destination, i.e. detached — modelled commands are
   unaffected because their declarations are never empty;
3. `main.rs` wiring: for `exec`, derive when `declared_outputs` is empty,
   otherwise use `layout::declared` exactly as today;
4. remove `exec`'s "declares none" refusal in a profile checkout (it is
   replaced by derivation), and keep `refuse_unknown_layout` for external
   subcommands that are not `exec`;
5. one bounded notice when a receipt-recorded member is real (existing
   grammar), and the member-cap/refusal messages;
6. units and regressions below.

S1 is a **member-binding change inside the external-root path**, so its native
proof is **ELEVATED** (author native plus a distinct independent reviewer
native, each separately ROOT-activated), per the brief's "Elevated wherever
member binding/removable writes/receipt or writer custody change".

**S2: bounded post-run member observation (optional, separate brief).** Before
spawn, record the member-shaped paths that are absent; after group closure,
stat those paths only (≤ 8 paths, no walk) and print at most one notice naming
each new real directory and its remedies. No authority, no deletion, no
conversion. Merging S2 into S1 is acceptable if the reviewer prefers one brief;
S1 must stay correct without it.

**S3: launcher guard (not planned).** Only if S2's report shows a real gap.
Requires its own design review, its own bounded use of `cargo metadata
--no-deps` (no resolve, no lock, no download, no build script) and a decision
on PATH shims, which the product direction defers.

**S4 (downstream, not this design).** (b) automatic first conversion defaults
to profile members; (c) explicit `oxide outputs adopt` migration of existing
whole-target consumer checkouts, Acowtancy's primary checkout first by its
owners' gates. Both need their own briefs and native evidence.

**S5 (follow-on, needs a decision).** Whether a *derived* member may ever be
newly linked (requires a keyed reservation and therefore a custody change), and
whether the derived cap may exceed eight. Both are recorded in
[questions](../questions.md), not decided here.

## Test and proof plan

### Source counterfactual tests (pure, no Cargo)

1. committed receipt placing `debug`+`release`, no `--output` → derived
   declaration sorts, dedups and stays prefix-free; the rendered key has one
   `outputs-3` line per member.
2. receipt-recorded unselected member → `real <member>` key line and the local
   notice, mirroring a modelled command.
3. detached receipt → derived declaration empty, `planned_destination` returns
   `None` (not an error), key carries no destination, binding accepts no link.
4. prepared/pruning receipt → refusal, nothing planned.
5. whole-target receipt → `outputs-2` key and the whole-target plan unchanged;
   a declaration is still ignored.
6. absent receipt → unchanged ephemeral behaviour.
7. a candidate link outside the receipt (owned or foreign) → refusal naming the
   path; a candidate real directory outside the receipt is ignored and is not
   keyed, so the owned/ignored pair is non-vacuous.
8. member-shaped regular file → refusal.
9. more than eight receipt members → refusal naming `oxide outputs detach`.
10. declared `--output` parity: the same input produces a byte-identical
    declaration, key and plan as today (the derived path never runs).
11. declaration + derivation are mutually exclusive: an explicit `--output`
    still produces a *complete* set and still reserves.
12. checkout path with a space → derivation unaffected (member names only).
13. the pair (owned member link accepted, foreign member link refused) differs
    only in the link's destination, so it is non-vacuous: deleting the
    classification makes the accepted case fail.
14. a **real** `target/debug` holding payload, and a **linked** one, are never
    enumerated as a triple ancestor: no `target/debug/debug` or
    `target/debug/release` candidate appears and the payload is never listed.
    This is the ordering/nofollow assertion the shipped
    `migration_members_discover_only_bounded_triple_pairs` case does not make.
15. a receipt-recorded unselected member is planned `real <member>`; an
    unrecorded real member is in neither the plan nor the key.
16. an owned link at a candidate path the receipt does not place → refusal.

These belong beside the existing `layout.rs` and `outputs.rs` tests, run by
`effigy test rust`.

### Bounded native proof design

One fixed script, **the whole command** of the qualified shared supervisor, as
a focused mode of the existing permanent-outputs fixture (the #121/#124/#125
precedent) so image custody, registration and staging are reused rather than
rebuilt:

```sh
python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 \
  --scratch-budget-mib 512 -- \
  python3 scripts/test-permanent-outputs.py exec-inference
```

- **Selector:** `effigy rust:exec-output-inference` (heavy admission), plus an
  uncounted twin `effigy rust:exec-output-inference:diagnostic` appending
  `--diagnostic`.
- **Tier:** ELEVATED (member binding in the external-root path; the fixture
  writes to a modelled removable volume). Author native and a distinct
  independent reviewer native, each separately activated.
- **Bounds:** 300 s wall clock, 512 MiB monitored scratch, helper work cutoff
  ~270 s so cleanup and ordinary detach finish inside the deadline; one owned
  128 MiB APFS sparse image attached with `hdiutil -owners on`; normal
  admission and the owner-accepted internal monitored root
  (`--root-parent /private/tmp`, decision `bd357a4c`, Tom-approved apparatus
  correction 2026-10-26). The fixture's control pool is a genuine internal
  `directory-budget` pool, which the product correctly refuses on the selected
  external volume — the 2026-10-26 uncounted diagnostic at `171ae6fb` failed
  there before attaching its image, which is apparatus premise 8 and not a
  product defect. The internal pool, the supervisor's monitored root, the
  image backing and every fixture temp path stay inside the same monitored
  512 MiB root; the image-backed removable pool is attached with `-owners on`
  and registered as the operating pool after the internal-pool fixture. Limits
  and the scratch budget are unchanged, as is the ordinary one-shot detach.
- **Planning allocation (unmeasured):** staging peak ~288 MiB + image 128 MiB +
  fixtures 48 MiB + evidence 32 MiB + margin 16 MiB = 512 MiB; 4 Cargo jobs for
  the single staging, 1 per fixture child, sequential, at most one Cargo child
  at a time → peak 4 concurrent Cargo jobs; planning demand 4 CPU / 6 GiB.
- **Cases (fixed table, fail-fast; `--plan` prints it effect-free):**
  01 conversion via explicit adopt (host pair linked), asserted committed;
  02 `exec` with no `--output` runs a tiny fixture tool that calls the real
  `cargo build` → exit 0, `target/debug` still a link, the binary present under
  the root's `debug`, the internal checkout's real target unchanged, one
  summary line naming owned outputs;
  03 repeat → same slot, warm, no relink, no conversion;
  04 tool runs `cargo build --release` → placed under the root's `release`;
  05 declared parity: `--output debug --output release` reaches the same keyed
  slot and the same placement as 02/04;
  06 tool writes an unrecorded member (a real `target/<name>` directory it
  creates itself) → builds locally, root and receipt unchanged, no refusal
  (S2 adds the notice oracle when it lands);
  07 detached receipt → `exec` with no `--output` runs locally, nothing in the
  root, no refusal;
  08 never-converted checkout → unchanged ephemeral behaviour (control);
  09 legacy whole-target receipt → `outputs-2` key, unchanged behaviour
  (control);
  10 a member-shaped foreign link planted at `target/release` → refuses before
  Cargo with the checkout and target bytes unchanged;
  11 receipt changed while the command waits (debug pause hook + `oxide outputs
  detach`) → refuses "run the command again", nothing written;
  12 root drift (root marker removed or replaced for the *fixture's own* root) →
  refuses naming `oxide outputs adopt`;
  13 checkout path containing a space → case 02 passes end to end;
  14 `oxide outputs detach` after an exec session restores real empty member
  directories and keeps the root — no new custody, reuses the shipped detach;
  15 interrupt control: a gated tool is stopped by Ctrl-C, group closure holds
  the lease until the child exits, and the slot is released afterwards.
- **Oracles** are positive-path plus a control that must fail: cases 02/03 and
  case 10 differ only in the link's destination, so case 02's oracles fail if
  the classification is removed; no case is satisfied by "no output".
- **Receipt discipline:** the script writes a bounded helper receipt beside the
  guard's own on every outcome; it never claims guard closure, and an ordinary
  detach failure retains the backing and fails the run.

### Proof-authoring premise checklist (HIGH)

The implementation brief declares HIGH complexity. Dispositions for the fixed
script, to be traced through the executable case bodies (not this prose):

| # | Premise | Disposition for this proof |
| --- | --- | --- |
| 1 | private `HOME`/rustup | capture the invoking `RUSTUP_HOME` before replacing `HOME`, preserve the pinned 1.97.1 toolchain, `RUSTUP_AUTO_INSTALL=0`; the tiny fixture crates are std-only, so the only `--locked` call is staging this repository's committed lock |
| 2 | account-home control | `OXIDE_TEST_HOME_FROM_ENV=1` on every fixture child that must honour the private `HOME` |
| 3 | environment isolation | scrub `OXIDE_POOL`, `OXIDE_CONFIG`, `OXIDE_TEST_*`, `XDG_*` before setting only the fixture's explicit values and its debug pause hook |
| 4 | automatic adoption | every case states its receipt premise; case 08's unconverted checkout is positively *created* (real `target`, no receipt) and its receipt absence asserted |
| 5 | Cargo external argv | only relevant to the tiny fixture tool if it is invoked through a Cargo subcommand; the design's fixture tool is invoked directly, so the premise is recorded as not applicable with that reason |
| 6 | owned images | `hdiutil attach -owners on` on attach and reattach, asserted by the bounded read-only mount observation and the product's own registration refusal |
| 7 | compiler flags | `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` frozen consistently for staging and fixture children |
| 8 | temporary storage | the owner-accepted internal monitored root (`--root-parent /private/tmp`, decision `bd357a4c`): the fixture's control pool is a genuine internal `directory-budget` pool, which the product refuses on an external volume, so the internal pool, the supervisor scratch, the image backing and the fixture `TMPDIR` all stay inside one monitored 512 MiB root, with the image-backed removable pool registered afterwards as the operating pool; the qualified guard and its pinned bytes. The failed `171ae6fb` diagnostic is the apparatus evidence for this disposition |
| 9 | macOS `mktemp` | every fixture temp path uses a template under the owned scratch parent, and the executable fixture's own `mktemp` use is verified |

### Activation sequence

1. source + unit/regression coverage, `effigy rust:compile`,
   `effigy rust:format`, `effigy test rust`, `effigy docs:check`;
2. one **uncounted** `rust:exec-output-inference:diagnostic` under the qualified
   guard, its bounded `qualified: false` receipt kept, dependency/taint skips
   recorded honestly;
3. counted author native (fresh activation), then a distinct independent
   reviewer native (fresh activation);
4. independent exact-head source/fit review of the published head.

A source sweep complements the diagnostic and never replaces it. No count,
clock, scratch budget or previous allowance transfers from #107, #117, #121,
#124, #125 or #133.

## Independent-review questions

1. Is the feasibility conclusion right, given the pinned source — is there any
   *stable, non-executing* Cargo interface that reports a command's output
   profile directories, or a supported way to read one without rewriting the
   tool's argv?
2. Does deriving from the committed placement genuinely satisfy "no exec route
   needs `--output`", or does it move the requirement into an implicit
   pre-condition that the reviewer considers a silent weakening?
3. Is the candidate classification bounded and fail-closed enough: member names
   matched before triple descent (so a real `target/debug` payload is never
   listed), nofollow `file_type` and `read_link` only, unreadable entry →
   refuse, file at a candidate path → refuse — and does it ever resolve, open or
   descend a link target?
4. Is the key treatment right: same grammar, no marker, sharing with an equal
   declared set, and no sharing with a modelled subset — versus a dedicated
   `exec` key marker?
5. Is removing the "declares none" refusal for `exec` sufficient to keep
   `refuse_unknown_layout` meaningful for other external subcommands?
6. Should the derived path ever be allowed to exceed eight members, and is
   refusing with `oxide outputs detach` the right remedy?
7. Are S1's native cases the right minimum, and does case 06 (an unrecorded
   member) risk blessing a placement gap the operator has not accepted?
8. Does anything here quietly fold in nested-lease, idle-exec, Barge or
   custom-profile scope?

## Limits of this design

- It does not place outputs a tool writes outside the committed members, and it
  does not detect a tool that leaves `<checkout>/target` altogether.
- It claims nothing about a real `tauri build`/`tauri dev`; consumer dogfood and
  adoption are separate owner gates (Acow first).
- It is not a benchmark, and it establishes no storage saving.
- It grants no native run, install, host setting, consumer edit, release or
  storage cleanup, and it does not touch the operator-listed legacy roots.
- Every rule here is cooperative: a tool that leaves the recorded group, or
  rewrites its own environment, is outside the custody this design relies on.
