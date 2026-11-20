# `oxide exec` output-member inference

Status: implemented. `oxide exec` without `--output` derives a tool's output
members from the checkout's committed receipt (oxide#160, S1, and the profile
default S4b). oxide#164 applies the same derivation to external Cargo
subcommands. The code is in `src/bin/oxide/outputs/exec.rs` (`exec_declaration`,
`exec_members`, `lease_declaration`, `needs_reservation`) and the tool-directory
notice (`tool_directories_note`).

Not built: the optional post-run observation S2, the launcher guard S3 (not
planned), migration of existing whole-target checkouts beyond the explicit
`oxide outputs detach` followed by `oxide outputs adopt` (S4c, gated), and
derived link creation or a member cap above eight (S5). S5 is open in
[Q-019](../questions.md#q-019--how-should-oxide-exec-know-a-tools-profile-output-members).

This file owns how `oxide exec` derives a tool's output members. The
[workflow](workflow.md#oxide-exec-cargo-wrapping-tools) owns the `oxide exec`
contract. The placements it derives from are owned by
[profile-member placement](workflow.md)
and [whole-target placement](workflow.md).
[Architecture](../architecture.md#cargo-boundary) owns the storage invariants
and the Cargo boundary. The direction is in
[vision](../vision.md#product-workflow-direction).

## Why the set comes from the receipt

Cargo's stable interfaces do not report a command's profile output directories
before the command writes them. `cargo metadata` reports the target and build
directories only. The `--unit-graph` plan is nightly-only, and it is not
non-executing. Artifact messages arrive after a unit has written. Reading a
plan from the tool's own request would need a rewritten argv, which changes
the tool's behaviour. So `exec` treats the tool's request as unobservable and
derives the member set from the checkout's committed placement, which the
binding already trusts.

## Members

A member is one profile directory under the checkout's real `target`, in one of
four forms: `debug`, `release`, `<triple>/debug` or `<triple>/release`. Each
component matches `[A-Za-z0-9_.-]+`, and no component may be `.`, `..`, absolute
or escaping. A command binds at most 8 members, and a receipt records at most
32. Only members become owned links.

These are never members: `target` itself, Cargo's top-level entries
(`CACHEDIR.TAG`, `.rustc_info.json`, `tmp`, `doc`, `package`, a triple's ancestor
directory) and the slot's intermediate build directory. Host-side artifacts are
slot payload.

## Derivation

For `exec` without `--output` in a profile-layout checkout, two sets are
computed. They are deliberately different.

- **Keyed set.** Exactly the receipt's placements: selected members become owned
  links, and receipt-recorded unselected members stay real. It is a subset of
  the receipt, it is the only set planned and bound, and it does not change
  during the session.
- **Classified set.** Every existing candidate member path under `target`. It
  can only refuse. It never plans, keys, converts or grants.

Candidates come from the existing profile member traversal. `debug` and `release` are
matched as member names before any triple descent, so a profile's payload is
never listed. Links are not followed. Each candidate gets `symlink_metadata`,
and a link gets `read_link`. A link target is never resolved, opened or
descended. An unreadable entry, or a non-directory at a candidate path, refuses.

| Entry at a candidate path | Outcome |
| --- | --- |
| Receipt places it; the entry is the owned link to its root | Planned and keyed |
| Receipt places it; the owned link's destination directory is absent inside the verified, committed root | Planned; binding creates that one directory under writer custody (oxide#162) |
| Receipt places it; anything else (missing, real, foreign, wrong root or nonce) | Refuse before Cargo |
| Receipt records it unselected; real directory | Keyed `real <member>`, one bounded notice |
| Receipt records it unselected; link or file | Refuse |
| Receipt does not record it; real directory | Ignored: local, not keyed |
| Receipt does not record it; any link or file | Refuse |
| Absent | Not planned; a directory the tool creates is real and local |

Checkout states:

| Checkout state | `exec` without `--output` |
| --- | --- |
| Profile receipt, committed | Derived plan |
| Profile receipt, detached | Empty set; runs locally with real directories, no owned destination |
| Profile receipt, prepared or pruned | Refuse under the existing custody rules; pruned names `oxide outputs adopt` |
| Whole-target receipt | Unchanged; the declaration is not consulted (`outputs-2` key) |
| Never converted | Unchanged ephemeral behaviour; no destination in the key |
| Member-shaped foreign link, no profile receipt | Refuse: an unconverted checkout's `target` must be real |

## Binding and reservation

Binding runs under the checkout's binding mutex and validates everything it
validated before: the surviving reservation, receipt generation, root marker,
kind, volume, device, ancestry and each keyed destination. Before Cargo starts,
each planned owned member must be this checkout's owned link. A candidate with a
link or a non-directory entry outside the plan refuses. A receipt in any state
other than committed or detached refuses. A refusal changes nothing: the link,
receipt, root, marker, registry, reservation and slot stay as they were.

Reservation is skipped only when the derived set is non-empty
(`needs_reservation`). An empty set keeps the shipped reservation path, because a
whole-target plan can name a future root (oxide#117, oxide#164).

## Slot key

The keyed set uses the existing key grammar: one `outputs-3` line per owned
member and one `real <member>` line per receipt-recorded unselected member. No
new prefix and no version bump. An `exec` that derives the same set as a declared
command shares its key and warm slot. A derived set normally contains the host
pair, so it does not share a slot with a modelled command that builds only one
profile.

## `--output`

`oxide exec --output <member>` keeps its behaviour: parsing, validation, the
complete-declaration rule, the same key, future-root reservation, and creation
of a member link when it is absent. It is optional, not deprecated. Use it only
to place a member that the receipt does not place, such as a custom profile
directory, a triple the checkout has never built, or a deliberate subset.

## What reaches the user

- A member the receipt does not place stays a real local directory. It is not
  refused. A bounded notice names it once, with the remedies: clear the real
  directory and declare it with `--output`, or use a modelled build for it.
- A receipt-recorded real member gets a separate unplaced-member notice.
- Notices list at most eight names and never recurse.

## Limits

- A tool that writes outside the receipt's members (a custom profile, an
  unplaced triple, `target/<other>`) builds locally. The goal of running without
  `--output` is met. Placement is met only for members the receipt places.
- A tool that sets `CARGO_TARGET_DIR` or `--target-dir`, or overrides the build
  directory, escapes Oxide's storage. A tool that daemonises or calls `setsid`
  leaves group closure. A tool that builds another workspace or toolchain is
  outside this placement. These are cooperative limits. The derivation does not
  detect them.
- A tool that unlinks its own member link is caught on the next command, which
  refuses and names `oxide outputs adopt`. Nothing is silently re-created.

## Not built and open

- **S2, post-run observation.** Optional. The shipped tool-directory notice covers
  the reporting need. Further observation is not specified here.
- **S3, launcher guard.** Not planned. It would need PATH or `$CARGO`
  interception, which the product direction defers, and its own review.
- **S4c, migration of whole-target checkouts.** Gated, and not part of this
  design. Explicit `oxide outputs detach` followed by `oxide outputs adopt` is
  the only route, starting with the primary consumer checkout under its owners'
  gates.
- **S5, open in Q-019.** Whether a derived member may ever be newly linked, which
  needs a keyed reservation and a custody change, and whether a derived set may
  exceed eight members.
- A real Tauri dogfood and consumer adoption are separate owner gates.

## Not claimed

No storage saving, benchmark, or claim about a real `tauri build` or `tauri dev`.
The derivation is cooperative. A tool that leaves its recorded group or rewrites
its environment is outside the custody this design relies on.
