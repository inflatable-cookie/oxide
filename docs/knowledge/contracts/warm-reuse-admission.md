# Normal warm-reuse admission

This contract owns the rule that grants a compatible Normal idle slot without a
recursive walk of its contents. The ledger, holds and writer-closure rules stay
in [local lease](local-lease.md).

**Status: implemented.** The Normal branch is in source. Covering checks are
`rust:compile`, `rust:compile-release`, `rust:format`, `rust:barge-adoption`, and
`docs:check` when docs change.

## Policy

A compatible Normal idle slot is granted from its recorded root and its
exclusive lease state, without measuring its contents. The rule applies to
`directory-budget-v1` internal pools and `apfs-volume-v1` removable pools. The
acquisition reports **slot size unknown**.

This removes a descendant-completeness precondition. It is a policy choice, not a
measured speed-up.

The grant is a cooperative, root-bound lease. It is not containment of Cargo's
descendant filesystem access. A build can meet a symlink or nested mount that
Oxide did not see, and no completed scan was ever a write fence. Do not describe
it as a sandbox.

## What the grant relies on

- The compatible idle ledger row for the requested project and profile, with its
  exact slot and incarnation, and no active lease. Held, uncertain, incompatible,
  quarantined or unresolved generations get no reuse route.
- A non-waiting exclusive Barge hold on the idle root, taken before the grant is
  returned. A live holder makes the row unavailable now.
- The pool lock, under which the row moves from idle to held and the exclusive
  lease is persisted before return.
- The existing gates: backing and reserve checks before child launch, the
  host-reserve and removable-removal watchdogs, owned Cargo-group closure, and
  flush and release ordering. A skipped walk skips none of them.

Not verified in source: the earlier design also resampled root and parent
device, inode and mode before the grant. No such separate step appears in the
Normal branch, so treat that resampling as absent until a reviewed change adds it.

## Descendant facts

The grant makes no claim about descendants:

- **Mount boundaries.** Not checked before the grant. Deletion's device checks
  remain. The grant confers no authority
  over an unobserved nested mount.
- **Symlinks.** Root and parent symlinks refuse. Descendant symlinks are not
  inspected; Cargo keeps its ordinary path behaviour.
- **Unreadable or missing descendants.** Do not refuse the grant. Cargo's own
  outcome propagates. Root and parent unreadability still refuses.
- **Concurrent mutation.** No stability claim. Independent writers remain subject
  to the custody, refusal and recovery rules.
- **Limits.** No walk, and no completeness claim, on this branch.

A future requirement for descendant device uniformity before any Cargo write
would need a different contract and an enforcement boundary that a finite root
check cannot provide.

## Reporting

- A Normal grant records no new observation. It performs no reporting walk and
  substitutes no stale ledger figure for its own measurement.
- Recorded status observations keep their source, age and generation. They are
  historical and grant no deletion authority.
- Release measurement still runs and publishes only complete results.

## Required cases

- On both backends, a compatible Normal idle slot is granted with no inventory
  call, including with unreadable, missing, symlinked or cross-device descendants.
- A missing or wrong root, parent, device, owner, mode, volume identity, ledger
  row or incarnation refuses.
- A held or incompatible generation, and any recovery uncertainty, refuses as it
  does today.
- Reserve refusal, watchdog stop and removal, group closure and release ordering
  remain effective.

Review is one independent exact-head check, because the change can lead to later
writes on removable media. The grant's sampling limits are stated, not proven.
