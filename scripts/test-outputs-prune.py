#!/usr/bin/env python3
"""Owned-output prune proof (oxide#129): fixed script and allocation.

This file publishes the exact native plan required before activation. It is
NOT run: `RUN_BOUND` is False, so every mode except `--plan` refuses without
creating, mounting or deleting anything. Activation is a separate owner gate
(after exact-head review), under the qualified shared supervisor
(`scripts/bounded-test.py`, SHA-256
d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e):
300 s absolute, 512 MiB monitored owned scratch, work stopped 30 s before the
exported deadline, ordinary detach.

Static allocation (one 128 MiB owned APFS sparse image, private HOME,
CARGO_HOME and directory pools; no real builds, consumer, Windows, provider or
install):

    staging (constrained debug oxide recipe)      288 MiB
    APFS sparse image (payload + quotas)          128 MiB
    fixtures                                       32 MiB
    evidence                                       32 MiB
    margin                                         32 MiB
    ---------------------------------------------  ------
    total                                         512 MiB

Staging recipe: this HEAD's debug `oxide`, offline 1.97.1, `-j1`, one codegen
unit, no debug info, stripped/size-optimised (the #122 precedent, 58-64 MiB
observed, not a measured fit for this proof). Expected staging 37-60 s, 21
cases 60-120 s, attach/setup 20 s, cleanup 30 s: 147-230 s, UNMEASURED. If the
fixed script cannot fit, STOP and propose one honest larger bound before
activation; never split, extend or add a session.

Case allocation (21 = the original 19-case matrix plus two resolve-intent
cases):

  2  forward fence + initialization: internal-first initializing/ready,
     paired-control fence, exclusive registry creation, missing/unreadable
     registry refuses mutation and prune keeps; older numeric state refusal.
  4  nonce/reservation/cancel/recreation key: fresh 128-bit nonce per root
     creation and recreation, legacy root without a nonce, legacy outputs-1 key
     distinct from outputs-2, recreated root never claims the old key.
  3  registry/snapshot/overflow: bounded whole snapshot, malformed/unreadable
     receipt keeps the batch, sticky-incomplete row cap and prune keeps all,
     unindexed selection records its receipt transaction.
  3  eligibility/operator-list/absence: present checkout kept, unreadable or
     unmounted ancestor kept, operator-list legacy exception (well-formed
     marker, absent checkout, no selection, closed writers), duplicates and
     relative paths refuse.
  2  custody/containment/removal: active writer kept, foreign/symlink root or
     descendant stops, cross-device stop, marker unlinked last, reserve loss
     between batches stops with exit 3.
  4  crash boundaries: after intent, after rename, during delete, after
     deleted proof; replay only the exact intent; missing proof stays pending.
  1  completion/diagnostic/logical-allocated: terminal registry + receipt
     ordering, passive `oxide status` observation, logical/allocated bytes
     labelled with coverage.
  2  resolve-intent: (a) both leaves absent, no other selection, no writer
     publishes `resolved-by-operator` and distinguishes resolved from
     deleted; (b) present root/quarantine or a held reference refuses with the
     intent unchanged.

Every case is a tiny owned fixture; the proof asserts exit codes 0/2/3,
byte-for-byte nonmutation for refused and dry-run cases, exact intent/journal
phases, quarantine/marker state, registry revision movement, and no write
outside owned scratch. It does not prove a live cleanup, real-drive
qualification, host installation or product acceptance.
"""

import sys

RUN_BOUND = False
SUPERVISOR = "d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e"
LIMIT_SECONDS = 300
SCRATCH_MIB = 512
CASES = 21

ALLOCATION = [
    ("staging (constrained debug oxide)", 288),
    ("APFS sparse image", 128),
    ("fixtures", 32),
    ("evidence", 32),
    ("margin", 32),
]

CASE_ALLOCATION = [
    ("forward fence + initialization", 2),
    ("nonce / reservation / cancel / recreation key", 4),
    ("registry / snapshot / overflow", 3),
    ("eligibility / operator list / absence", 3),
    ("custody / containment / removal", 2),
    ("crash boundaries", 4),
    ("completion / diagnostic / logical-allocated", 1),
    ("resolve-intent", 2),
]


def plan() -> int:
    print("oxide#129 owned-output prune proof plan (NOT activated)")
    print(f"supervisor bounded-test.py sha256 {SUPERVISOR}")
    print(f"wall limit {LIMIT_SECONDS} s; monitored owned scratch {SCRATCH_MIB} MiB")
    print("static allocation:")
    total = 0
    for label, mib in ALLOCATION:
        total += mib
        print(f"  {label:<42} {mib:>4} MiB")
    print(f"  {'total':<42} {total:>4} MiB")
    print(f"cases ({CASES}):")
    for label, count in CASE_ALLOCATION:
        print(f"  {count:>2}  {label}")
    assert sum(count for _, count in CASE_ALLOCATION) == CASES
    assert total == SCRATCH_MIB
    print("activation: separate owner gate after exact-head review; this script refuses")
    return 0


def main(argv: list[str]) -> int:
    if argv == ["--plan"]:
        return plan()
    print(
        "test-outputs-prune: the fixed native proof is awaiting owner activation; "
        "only --plan is available",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
