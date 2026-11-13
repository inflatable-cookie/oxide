#!/usr/bin/env python3
"""Rust/Cargo 1.100 foundation qualification (oxide#083): plan and readiness.

This is the task-owned recipe for `effigy proof:foundation:rust-1-100:diagnostic`.
At this head it is SOURCE/CONTROL only:

- `--plan` prints the fixed command, cases, allocation, clocks, premise
  dispositions and the readiness gap. It reads source files only.
- `--self-check` runs pure controls over this repository's own source: the
  guard digest, the suspended-selector refusals, the shipped verified-Cargo
  table, the dep-info decoder's version gate, the reused helper inventory and
  the allocation arithmetic. It starts no child, compiler, Cargo, toolchain,
  pool, image or scratch.
- `--diagnostic` refuses before any effect. The composition is not ready: the
  focused oracles it would reuse still provision through the retired
  standalone lease CLI and APFS image backend (oxide#102). See READINESS.

It never installs a toolchain and never restores a retired product route.
"""

import ast
import hashlib
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

GUARD = "scripts/bounded-test.py"
GUARD_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
LEGACY = "scripts/test-oxide-cli.py"
FRESHNESS = "src/bin/oxide/freshness.rs"
DEP_INFO = "src/dep_info.rs"

PINNED = "1.97.1"
TESTED = "1.100.0"
# Owner-verified 2026-11-13 from the official channel manifest
# (static.rust-lang.org/dist/channel-rust-1.100.0.toml, SHA-256 below). The
# Cargo `-V` prefix and Cargo source pin are NOT known here: they are read from
# the installed toolchain, never guessed (brief: no guessed manifest).
TESTED_RUSTC = "rustc 1.100.0 (0ef1bc386 2026-11-09)"
TESTED_MANIFEST_SHA256 = "3051e859570f64a8c5acb9d09e87425db52c7d97287c67e8f6d57d7d34ef9258"

MIB = 1024 * 1024
LIMITS = {
    "focused_invocations_total": 3,
    "effect_phase_seconds": 300,
    "aggregate_scratch_bytes": 512 * MIB,
    "pool_cap_bytes": 128 * MIB,
    "fixture_source_bytes": 8 * MIB,
    "raw_log_bytes": 32 * MIB,
    "host_reserve_bytes": 20 * 1024 * MIB,
    "probe_cargo_calls_per_participant": 8,
    "probe_seconds_aggregate": 90,
    "probe_scratch_bytes": 64 * MIB,
}

COMMAND = (
    "python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- "
    "python3 scripts/foundation-rust-1-100.py --diagnostic"
)

# Planned invocations of the single proof: at most three TOTAL across both
# toolchains. Each is one supervised effect phase (staging included).
INVOCATIONS = (
    ("I1", PINNED, "focused oxide proof on the pinned toolchain"),
    ("I2", TESTED, "focused oxide proof on 1.100.0"),
    ("I3", "reserve", "one spare; never a retry-until-green"),
)

# Each focused oracle the brief retains, the legacy helper that holds its
# executable body today, and whether that body is usable under the shipped
# directory-budget backend as written.
CASES = (
    ("routing", "prove_routing", "lease_status/state.json"),
    ("profiles", "prove_native", "lease_status/state.json + standalone lease"),
    ("checkout-affine and cross-checkout freshness", "prove_freshness", "state.json records"),
    ("gate crash", "prove_gate_crash", "state.json records"),
    ("crash recovery", "prove_recovery", "lease_status + state.json recoveries"),
    ("group gone", "prove_group_gone", "state.json records"),
    ("status", "prove_native", "standalone lease status --json"),
    ("effective build-dir, stray, dep-info", "prove_layout", "payload/slots path only"),
    ("unverified-Cargo fallback", "prove_fallback", "none (oxide summary/status lines)"),
)

# Probes outside the focused selector: at most 8 Cargo calls per participant.
PROBES = (
    ("version", "cargo -V and rustc -V under +1.100.0", 2),
    ("stray/dep-info", "one plain `cargo build --offline` on a tiny std-only crate", 1),
    ("rcgu", "baseline + four rebuilds of the same tiny crate", 5),
)

# The 9-item HIGH premise checklist (workflow.md#proof-authoring-premise-checklist)
# against the legacy bodies this composition would reuse.
PREMISES = (
    ("1 private HOME/rustup", "scrubbed() captures RUSTUP_HOME before HOME; RUSTUP_AUTO_INSTALL=0", LEGACY, "scrubbed"),
    ("2 account-home control", "Runner sets OXIDE_TEST_HOME_FROM_ENV=1", LEGACY, "Runner"),
    ("3 environment isolation", "scrubbed() drops OXIDE_POOL/OXIDE_CONFIG/OXIDE_TEST_*/XDG_*", LEGACY, "scrubbed"),
    ("4 automatic adoption", "not applicable: no permanent drive mode is registered", None, None),
    ("5 Cargo external argv", "not applicable: no external subcommand case retained", None, None),
    ("6 owned images", "not applicable after migration: no image; legacy bodies use the retired image", None, None),
    ("7 compiler flags", "GAP: legacy staging does not freeze RUSTFLAGS/CARGO_ENCODED_RUSTFLAGS", LEGACY, "matrix_main"),
    ("8 temporary storage", "GAP: legacy scratch_root() predates the qualified selected-volume guard", LEGACY, "scratch_root"),
    ("9 macOS mktemp", "no mktemp call in the reused bodies", None, None),
)

READINESS = (
    "NOT READY. Every retained focused oracle except the layout and fallback "
    "phases reads the retired pool's `oxide-managed/state.json` records or the "
    "standalone lease CLI's `status --json`, and staging builds the "
    "retired lease binary. The shipped directory-budget backend has neither "
    "surface, so an equivalent migration means rewriting each oracle against "
    "`oxide status` and the current ledger, plus premise items 7 and 8. That "
    "is larger than this source phase; no oracle was weakened to fit. "
    f"Separately, {TESTED} is not in VERIFIED_CARGO, so on {TESTED} the "
    "shipped gate must run its unverified fallback (affine slot, stray "
    "`unknown`); a shared-slot verdict needs the Cargo source-pin check first."
)


def read(rel: str) -> bytes:
    with open(os.path.join(REPO, rel), "rb") as handle:
        return handle.read()


def sha256(rel: str) -> str:
    return hashlib.sha256(read(rel)).hexdigest()


def functions(rel: str) -> set:
    tree = ast.parse(read(rel).decode())
    return {node.name for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.ClassDef))}


def verified_cargo() -> list:
    text = read(FRESHNESS).decode()
    block = re.search(r"VERIFIED_CARGO: \[&str; \d+\] = \[(.*?)\];", text, re.S)
    if not block:
        raise AssertionError("VERIFIED_CARGO table not found")
    return re.findall(r'"(cargo [^"]+)"', block.group(1))


def legacy_refusals() -> dict:
    """For each suspended legacy mode, whether main() refuses before its body."""
    tree = ast.parse(read(LEGACY).decode())
    main = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "main")
    found = {}
    for node in main.body:
        if not isinstance(node, ast.If):
            continue
        source = ast.unparse(node.test)
        mode = next((m for m in ("--cargo-matrix", "--toolchain-context") if m in source), None)
        if not mode:
            continue
        calls = [ast.unparse(s) for s in node.body if isinstance(s, (ast.Expr, ast.Return))]
        refuse = next((i for i, c in enumerate(calls) if c == "fail(RETIRED_POOL_PROOF)"), None)
        body = next((i for i, c in enumerate(calls) if c.startswith("return ")), None)
        found[mode] = refuse is not None and body is not None and refuse < body
    # The default (rust:oxide-cli) path: fail(RETIRED_POOL_PROOF) is the first
    # top-level statement after the mode dispatch.
    top = [ast.unparse(s) for s in main.body if not isinstance(s, (ast.If, ast.Global))]
    found["default"] = bool(top) and top[0] == "fail(RETIRED_POOL_PROOF)"
    return found


def allocation() -> dict:
    probe_calls = sum(calls for _, _, calls in PROBES)
    return {
        "focused_invocations": len(INVOCATIONS),
        "probe_cargo_calls": probe_calls,
        "rcgu_calls": dict((k, c) for k, _, c in PROBES)["rcgu"],
    }


def self_check() -> int:
    checks = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        checks.append((name, ok, detail))

    check("guard digest", sha256(GUARD) == GUARD_SHA256, sha256(GUARD))
    refusals = legacy_refusals()
    for mode in ("--cargo-matrix", "--toolchain-context", "default"):
        check(f"suspended {mode} refuses before its body", refusals.get(mode) is True, str(refusals.get(mode)))
    table = verified_cargo()
    check("VERIFIED_CARGO has the five pinned releases", len(table) == 5, ", ".join(table))
    check(f"{PINNED} is verified", any(e.startswith(f"cargo {PINNED} (") for e in table))
    check(f"{TESTED} is not verified (fallback expected)", not any(e.startswith(f"cargo {TESTED} ") for e in table))
    dep = read(DEP_INFO).decode()
    check("dep-info decoder still pins version 1 and refuses others", "version 1" in dep and "refuses" in dep)
    names = functions(LEGACY)
    for _, helper, _ in CASES:
        check(f"legacy helper {helper} exists", helper in names)
    for _, _, rel, symbol in PREMISES:
        if rel:
            check(f"premise source {symbol} exists", symbol in functions(rel))
    alloc = allocation()
    check("at most three focused invocations", alloc["focused_invocations"] <= LIMITS["focused_invocations_total"])
    check("probe calls within 8", alloc["probe_cargo_calls"] <= LIMITS["probe_cargo_calls_per_participant"],
          str(alloc["probe_cargo_calls"]))
    check("rcgu counts baseline + four rebuilds as 5", alloc["rcgu_calls"] == 5)
    check("command names the qualified guard and caps",
          COMMAND.startswith(f"python3 {GUARD} --limit {LIMITS['effect_phase_seconds']} --scratch-budget-mib 512 -- "))
    check("diagnostic refuses (not ready)", diagnostic(dry=True) == 2)
    failed = [c for c in checks if not c[1]]
    for name, ok, detail in checks:
        print(f"{'ok  ' if ok else 'FAIL'} {name}" + (f" [{detail}]" if detail and not ok else ""))
    print(f"self-check: {len(checks) - len(failed)}/{len(checks)} passed")
    return 1 if failed else 0


def plan() -> int:
    print("oxide#083 foundation qualification on Rust/Cargo 1.100 (layout v2)")
    print(f"selector: effigy proof:foundation:rust-1-100:diagnostic (heavy)")
    print(f"command:  {COMMAND}")
    print(f"guard:    {GUARD} sha256 {sha256(GUARD)} (ordinary qualified default)")
    print(f"toolchains: pinned {PINNED}; tested {TESTED} ({TESTED_RUSTC}, manifest {TESTED_MANIFEST_SHA256[:16]}...)")
    print("cargo -V prefix and source pin for 1.100.0: read from the installed toolchain, not assumed")
    print("invocations (at most three TOTAL, single proof):")
    for key, chain, what in INVOCATIONS:
        print(f"  {key} {chain}: {what}")
    print("focused cases -> legacy body -> retired surface it reads:")
    for case, helper, surface in CASES:
        print(f"  {case}: {LEGACY}:{helper} -> {surface}")
    print("probes outside the focused selector (per participant):")
    for key, what, calls in PROBES:
        print(f"  {key}: {what} ({calls} Cargo calls)")
    print(f"  total {allocation()['probe_cargo_calls']} of {LIMITS['probe_cargo_calls_per_participant']}; "
          f"{LIMITS['probe_seconds_aggregate']} s; {LIMITS['probe_scratch_bytes'] // MIB} MiB")
    print("limits: " + ", ".join(f"{k}={v}" for k, v in LIMITS.items()))
    print("fit: UNMEASURED (no staging, build or proof has run for this recipe)")
    print("premise checklist:")
    for item, disposition, rel, symbol in PREMISES:
        where = f" ({rel}:{symbol})" if rel else ""
        print(f"  {item}: {disposition}{where}")
    print("expected 1.100 verdicts from shipped source, pending evidence:")
    print("  freshness gate: unverified fallback (slot affine, gate off)")
    print("  stray detection: `unknown` for an unverified Cargo")
    print("  dep-info: version-1 decode or a clean refusal; never guessed")
    print(f"readiness: {READINESS}")
    return 0


def diagnostic(dry: bool = False) -> int:
    if not dry:
        print(f"refused before any effect: {READINESS}", file=sys.stderr)
    return 2


def main(argv: list) -> int:
    if argv == ["--plan"]:
        return plan()
    if argv == ["--self-check"]:
        return self_check()
    if argv == ["--diagnostic"]:
        return diagnostic()
    print("usage: foundation-rust-1-100.py --plan | --self-check | --diagnostic", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
