#!/usr/bin/env python3
"""Effect-free custody decision checks for the proposed oxide#232 fixture.

This is preliminary apparatus, not a native proof. It creates no processes and
cannot signal. The fixed native case driver and its receipts remain to be
implemented and independently reviewed before any native execution.
"""
from __future__ import annotations

import json
import sys
from dataclasses import dataclass, replace


@dataclass(frozen=True)
class Custody:
    # These are facts established by fixture control, not PID observations.
    own_direct_child: bool = True
    parent_alive: bool = True
    exclusive_wait: bool = True
    no_auto_reap: bool = True
    child_unreaped: bool = True
    group_created: bool = True
    membership_held: bool = True
    observation_consistent: bool = True
    window_open: bool = True


def signal_decision(state: Custody, target: str) -> tuple[bool, str]:
    """Fail closed; a held child by itself does not establish group custody."""
    if target not in ("child", "group"):
        return False, "unsupported target"
    for name in ("own_direct_child", "parent_alive", "exclusive_wait",
                 "no_auto_reap", "child_unreaped", "observation_consistent",
                 "window_open"):
        if not getattr(state, name):
            return False, name
    if target == "group":
        for name in ("group_created", "membership_held"):
            if not getattr(state, name):
                return False, name
    return True, "permitted by fixture premises"


def closure_decision(*, reaped: bool, census: str, escaped: bool,
                     uncertain: bool) -> bool:
    """A successful signal, reap alone, or partial census is not closure."""
    return reaped and census == "certain-empty" and not escaped and not uncertain


CASES = (
    "live child", "exited but unreaped child", "automatic reap refused",
    "concurrent wait refused", "parent dead or unresponsive", "group escape",
    "session escape", "descendant in another group", "stale or ambiguous observation",
    "spent stop window", "actual reap", "final group and session census",
)

PREMISES = (
    "1 private HOME and rustup: no Rust, rustup, Cargo or toolchain fixture planned",
    "2 account-home control: no Oxide invocation planned",
    "3 environment isolation: native child environments must be explicitly scrubbed",
    "4 automatic adoption: no checkout or Oxide build planned",
    "5 Cargo external argv: no Cargo invocation planned",
    "6 owned images: no image or attachment planned",
    "7 compiler flags: no compiler or Cargo invocation planned",
    "8 temporary storage: native receipt must use supervisor selected-volume root",
    "9 macOS mktemp: no mktemp invocation planned; native paths must stay in owned root",
)


def self_check() -> None:
    baseline = Custody()
    assert signal_decision(baseline, "child")[0]
    assert signal_decision(baseline, "group")[0]
    for name in ("own_direct_child", "parent_alive", "exclusive_wait", "no_auto_reap",
                 "child_unreaped", "observation_consistent", "window_open"):
        for target in ("child", "group"):
            assert signal_decision(replace(baseline, **{name: False}), target) == (False, name)
    for name in ("group_created", "membership_held"):
        broken = replace(baseline, **{name: False})
        assert signal_decision(broken, "child")[0]
        assert signal_decision(broken, "group") == (False, name)
    assert not signal_decision(baseline, "observed non-child")[0]
    assert closure_decision(reaped=True, census="certain-empty", escaped=False, uncertain=False)
    assert not closure_decision(reaped=False, census="certain-empty", escaped=False, uncertain=False)
    assert not closure_decision(reaped=True, census="unknown", escaped=False, uncertain=False)
    assert not closure_decision(reaped=True, census="certain-empty", escaped=True, uncertain=False)
    assert not closure_decision(reaped=True, census="certain-empty", escaped=False, uncertain=True)
    assert len(CASES) == len(set(CASES)) == 12
    assert len(PREMISES) == 9
    print("custody decision self-check: OK (pure; no native credit)")


def plan() -> None:
    print(json.dumps({"state": "incomplete apparatus; no native execution",
                      "source_review": "required before native",
                      "cases_to_implement": CASES, "premise_dispositions": PREMISES,
                      "native_bounds": "300 seconds; 512 MiB; qualified supervisor; heavy admission",
                      "authoring_bounds": "900 seconds; 128 MiB private scratch",
                      "unresolved": ["case drivers and ownership handshakes", "census and receipts",
                                     "diagnostic isolation and abort semantics", "native selectors"]}, indent=2))


if __name__ == "__main__":
    if sys.argv == [sys.argv[0], "--self-check"]:
        self_check()
    elif sys.argv == [sys.argv[0], "--plan"]:
        plan()
    else:
        raise SystemExit("incomplete apparatus: only --self-check or --plan are available")
