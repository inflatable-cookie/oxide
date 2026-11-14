#!/usr/bin/env python3
"""Rust/Cargo 1.100 foundation qualification (oxide#083).

Task-owned recipe for `effigy proof:foundation:rust-1-100:diagnostic`.

Modes:

- `--plan`: the fixed command, phases, cases, allocation, premise
  dispositions and readiness. Reads repository files only.
- `--self-check`: pure controls (no child, compiler, Cargo, toolchain, pool,
  image or scratch beyond one removed private temp directory under TMPDIR).
- `--gate`: the pre-guard activation gate the selector runs first. It refuses
  (exit 2) unless the planner bound one occurrence; at this head no binding
  exists, so the selector refuses before the guard creates anything.
- `--diagnostic --toolchain <1.97.1|1.100.0>`: one guarded phase. Uncounted,
  keep-going, always nonzero, `qualified: false`. Never run in the source
  phase.

The six focused oracles that the retired `rust:oxide-cli` proof carried are
migrated onto the shipped directory-budget-v1 pool: a legacy helper is reused
only where its state readers resolve to `ledger.json` / `oxide-managed/
state.json` and its expected text is still in product source; routing,
profiles, status and the unverified-Cargo fallback are rewritten here. It
never installs a toolchain and never restores a retired product route.
"""

import ast
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

GUARD = "scripts/bounded-test.py"
GUARD_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
LEGACY = "scripts/test-oxide-cli.py"
FRESHNESS = "src/bin/oxide/freshness.rs"
MAIN = "src/bin/oxide/main.rs"
DEP_INFO = "src/dep_info.rs"

PINNED = "1.97.1"
TESTED = "1.100.0"
TOOLCHAINS = (PINNED, TESTED)
PINNED_IDENTITY = ("cargo 1.97.1 (c980f4866 2026-06-30)", "rustc 1.97.1 (8bab26f4f 2026-07-14)")
# Owner-verified 2026-11-13 from the official channel manifest
# (static.rust-lang.org/dist/channel-rust-1.100.0.toml).
TESTED_RUSTC = "rustc 1.100.0 (0ef1bc386 2026-11-09)"
TESTED_MANIFEST_SHA256 = "3051e859570f64a8c5acb9d09e87425db52c7d97287c67e8f6d57d7d34ef9258"
# Cargo source pin for 1.100.0: the src/tools/cargo gitlink of rust-lang/rust
# at 0ef1bc386 (primary source in the evidence README; the manifest's
# `0.101.0 (3acc79c0c 2026-11-05)` agrees). The installed `cargo -V` must start
# with this prefix or the phase aborts; the `1.100.0` part is convention until
# the installed binary shows it.
TESTED_CARGO_PIN = "3acc79c0c841844675bc53a605922dbfc441e750"
TESTED_CARGO_PREFIX = "cargo 1.100.0 (3acc79c0c "

MIB = 1024 * 1024
LIMITS = {
    "focused_invocations_total": 3,
    "effect_phase_seconds": 300,
    "guard_work_seconds": 270,
    "aggregate_scratch_bytes": 512 * MIB,
    "pool_cap_bytes": 128 * MIB,
    "fixture_source_bytes": 8 * MIB,
    "raw_log_bytes": 32 * MIB,
    "host_reserve_bytes": 20 * 1024 * MIB,
    "probe_cargo_calls_per_participant": 8,
    "probe_seconds_aggregate": 90,
    "probe_scratch_bytes": 64 * MIB,
}
RECEIPT_BYTES = 1 * MIB  # per phase; two phases stay far inside 32 MiB
TAIL_BYTES = 4096

PHASE = (
    "python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- "
    "python3 scripts/foundation-rust-1-100.py --diagnostic --toolchain {toolchain}"
)
GATE = (
    'python3 scripts/foundation-rust-1-100.py --gate --binding "$FOUNDATION_083_BINDING" '
    '--sha256 "$FOUNDATION_083_BINDING_SHA256"'
)
# Diagnostic phase statuses (all nonzero; the ordinary guard passes the
# command's own status through only for a closed run within its bounds and
# otherwise returns 124 deadline / 125 fault, refusal or UNCLOSED / 128+N).
DIAGNOSTIC_DONE = 3        # receipt written, case collection not aborted
DIAGNOSTIC_INCOMPLETE = 4  # receipt written, but setup fault or aborted collection
DIAGNOSTIC_NO_RECEIPT = 5  # receipt could not be written
DIAGNOSTIC_REFUSED = 2     # refused before any effect
# The whole selector: the gate, then phase 1; phase 2 runs ONLY if phase 1's
# guard status is DIAGNOSTIC_DONE (closed, within bounds, complete receipt).
# Any other status (124, 125, 128+N, refusal, crash, incomplete, no receipt)
# stops the selector with that status; phase 2's own status is the final one,
# never overwritten. The third allowed invocation is a spare
# that the selector never spends.
COMMAND = (
    f"{GATE} && {{ {PHASE.format(toolchain=PINNED)}; s=$?; "
    f'echo "foundation-083 phase {PINNED} status $s"; [ "$s" -eq {DIAGNOSTIC_DONE} ] || exit "$s"; '
    f"{PHASE.format(toolchain=TESTED)}; s=$?; "
    f'echo "foundation-083 phase {TESTED} status $s"; exit "$s"; }}'
)


def phase_decision(phase: int, status) -> tuple:
    """What COMMAND does with a phase's guard status: ("continue", None) or
    ("stop", exit). Mirrors the shell exactly; the self-check binds both."""
    if not isinstance(status, int) or isinstance(status, bool) or not 0 <= status <= 255:
        return ("stop", 125)  # a missing or malformed status is never closure
    if phase == 1 and status == DIAGNOSTIC_DONE:
        return ("continue", None)
    return ("stop", status)

# Planned invocations: at most three TOTAL across both toolchains.
INVOCATIONS = (
    ("I1", PINNED, "guarded phase 1: stage + every focused case on the pinned toolchain"),
    ("I2", TESTED, "guarded phase 2: stage + every focused case + probes on 1.100.0"),
    ("I3", "spare", "never spent by the selector; no retry-until-green"),
)

# Case table: (key, depends-on, applies-to, body, origin).
CASES = (
    ("profiles", (), "both", "case_profiles", "rewritten from prove_native (APFS init/cap/oxide-quick path retired)"),
    ("routing", ("profiles",), "both", "case_routing", "rewritten from prove_routing (slot dir from ledger; no APFS backend)"),
    ("status", ("profiles",), "both", "case_status", "rewritten: current slot_line/stray formats from main.rs"),
    ("clean-cross-checkout", ("profiles",), "both", "case_clean_cross", "rewritten from prove_native tail (no cap-of-2; #118 cap is 12)"),
    ("waiting", ("clean-cross-checkout",), "both", "legacy.prove_waiting + prove_clean_waits", "reused: state.json tickets/cleans"),
    ("interrupt", ("profiles",), "both", "legacy.prove_interrupt", "reused: ledger slots via shim"),
    ("crash-recovery", ("profiles",), "both", "legacy.prove_recovery", "reused: state.json records/recoveries + ledger lease_id"),
    ("group-gone", ("crash-recovery",), "both", "legacy.prove_group_gone", "reused: state.json records"),
    ("freshness-gate", (), "verified", "legacy.prove_freshness (+prove_gate_crash)", "reused: verified Cargo only"),
    ("layout", ("freshness-gate",), "verified", "case_layout", "legacy.prove_layout with stray read via `status --measure` (recorded status measures nothing)"),
    ("fallback", (), "unverified", "case_fallback", "rewritten from prove_fallback for the tested Cargo itself"),
    ("layout-observation", ("fallback",), "unverified", "case_layout_observation", "new: measured stray holds-or-unknown (never none), observed dirs, shipped decoder over v1 and reshaped v2 dep-info"),
    ("probes", (), "tested", "case_probes", "new: version, plain build, rcgu x4 within 8 Cargo calls"),
)

# Probe calls (1.100.0 only), counted conservatively including rustc -V.
PROBES = (
    ("version", "cargo -V and rustc -V", 2),
    ("plain build", "baseline `cargo build --offline` of the tiny crate (stray, dep-info, rcgu baseline)", 1),
    ("rcgu", "four rebuilds after touching src/main.rs", 4),
)

# Plain Cargo calls INSIDE the focused invocations (as in the original
# rust:oxide-cli / cargo-matrix proofs). They are bounded by the phase guard,
# not by the 8-call probe cap, and are listed so nothing is hidden.
IN_PLANE_CARGO = (
    ("both", "staging: cargo -V, rustc -V of the 1.97.1 builder, one `cargo build --locked --offline`"),
    ("1.97.1", "fixture toolchain identity: cargo -V, rustc -V under RUSTUP_TOOLCHAIN=1.97.1"),
    ("both", "commit_lock: `cargo generate-lockfile --offline` per fixture (app; gated in freshness-gate or gated-fallback)"),
    ("both", "routing: one plain `cargo test` for the exit-code comparison"),
    ("1.97.1", "layout: one plain `cargo check --offline`"),
    ("1.100.0", "layout-observation: one plain `cargo check --offline`"),
)

# Product text the oracles depend on; each must exist in source.
SOURCE_NEEDLES = (
    (MAIN, '"oxide: {} profile, {strategy} strategy, slot {slot} {}, '),
    (MAIN, '"  slot {} {state}{class} {} project {} {owner} {}, {place}"'),
    (MAIN, '"  slots {}, waiting commands {}"'),
    (MAIN, '"stray Cargo output: unknown in {} ({reason}; its layout is unchecked)"'),
    (MAIN, "holds plain-Cargo deps/ or incremental/"),
    (MAIN, "budget-only"),
    (MAIN, '"no checkout affinity"'),
    (FRESHNESS, '"freshness gate off: {note}; the slot stays with this checkout"'),
    (FRESHNESS, '"freshness gate refreshed {} file(s)'),
)

# HIGH premise checklist (workflow.md#proof-authoring-premise-checklist),
# traced through the executable bodies: (item, disposition, source, symbol).
PREMISES = (
    ("1 private HOME/rustup", "legacy.scrubbed captures RUSTUP_HOME before HOME; RUSTUP_AUTO_INSTALL=0; locks from offline generate-lockfile in each fixture before --locked", LEGACY, "scrubbed"),
    ("2 account-home control", "legacy.Runner sets OXIDE_TEST_HOME_FROM_ENV=1 with a private HOME", LEGACY, "Runner"),
    ("3 environment isolation", "scrubbed drops OXIDE_POOL/OXIDE_CONFIG/OXIDE_TEST_*/XDG_*; fixture_environment drops every other OXIDE_* and CARGO_* override", "scripts/foundation-rust-1-100.py", "fixture_environment"),
    ("4 automatic adoption", "applies only in permanent drive mode; no drive is registered in the private HOME, so no conversion can occur", None, None),
    ("5 Cargo external argv", "routing's cargo-oxprobe reads only env (PROBE script ignores argv)", LEGACY, "PROBE"),
    ("6 owned images", "no image: the pool is directory-budget-v1 under the guard root", None, None),
    ("7 compiler flags", "fixture_environment and staging_environment set RUSTFLAGS='' and remove CARGO_ENCODED_RUSTFLAGS", "scripts/foundation-rust-1-100.py", "staging_environment"),
    ("8 temporary storage", "phase parent is a private dir under the guard's OXIDE_TASK_SCRATCH_ROOT; TMPDIR is parent/tmp; legacy scratch_root/mktemp is never called", "scripts/foundation-rust-1-100.py", "phase_parent"),
    ("9 macOS mktemp", "tempfile.mkdtemp(dir=...) only; no mktemp(1)", "scripts/foundation-rust-1-100.py", "phase_parent"),
)

UNVERIFIED_FACTS = (
    "fixture rust-toolchain.toml pins 1.97.1; RUSTUP_TOOLCHAIN in the fixture env overrides it (rustup precedence)",
    "dep_info_matrix only finds .fingerprint/<unit>/dep-*; v2 files (build/<pkg>/<meta>/fingerprint/dep-*, Cargo 3acc79c0c layout.rs) are copied unchanged into that shape for the shipped decoder; zero found is insufficient, never a pass",
    "layout v2 creates no <profile>/deps or .fingerprint but still <profile>/incremental (Cargo 3acc79c0c BuildDirLayout::prepare); for an unverified Cargo report_stray prints holds when incremental/ has files, else unknown, never none; the observation records which",
    "fit is UNMEASURED: historical rust:oxide-cli (65 s) and cargo-matrix (33-67 s) ran on the retired image pool and give no current figure",
)


class LedgerError(RuntimeError):
    pass


# ----------------------------------------------------------------- readers


def read(rel: str) -> bytes:
    with open(os.path.join(REPO, rel), "rb") as handle:
        return handle.read()


def sha256(rel: str) -> str:
    return hashlib.sha256(read(rel)).hexdigest()


def read_json(path: str, label: str, limit: int = 4 * MIB) -> dict:
    try:
        with open(path, "rb") as handle:
            raw = handle.read(limit + 1)
    except OSError as error:
        raise LedgerError(f"{label} unreadable: {error}") from None
    if len(raw) > limit:
        raise LedgerError(f"{label} exceeds {limit} bytes")
    try:
        document = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise LedgerError(f"{label} is not JSON: {error}") from None
    if not isinstance(document, dict) or not isinstance(document.get("version"), int):
        raise LedgerError(f"{label} has no integer version")
    return document


def ledger_view(pool: str) -> dict:
    """The shipped pool ledger, in the shape the legacy helpers read from the
    retired standalone CLI's `status --json`: `slots` rows with `id`,
    `state` and `lease_id`. Coverage is not a ledger fact, so it is None."""
    document = read_json(os.path.join(pool, "ledger.json"), "ledger.json")
    slots = document.get("slots")
    if not isinstance(slots, list):
        raise LedgerError("ledger.json slots is not a list")
    for row in slots:
        if not isinstance(row, dict) or not isinstance(row.get("id"), int) or not isinstance(row.get("state"), str) \
                or "lease_id" not in row:
            raise LedgerError(f"ledger.json slot row is partial: {row!r}"[:300])
    # The retired CLI's rows named the slot `slot_id` (a string); the ledger
    # names it `id` (an integer). Reused helpers read `slot_id`, so each row
    # gets that alias; the product's recovery line prints the same number.
    return {"slots": [{**row, "slot_id": str(row["id"])} for row in slots], "coverage": None}


def control_state(pool: str) -> dict:
    document = read_json(os.path.join(pool, "oxide-managed", "state.json"), "state.json")
    for key in ("records", "tickets", "cleans"):
        if not isinstance(document.get(key), list):
            raise LedgerError(f"state.json {key} is not a list")
    if not isinstance(document.get("recoveries", []), list):
        raise LedgerError("state.json recoveries is not a list")
    return document


def slot_dir(pool: str, slot: str) -> str:
    """A slot's recorded directory below payload/slots (oxide#104 nested
    layout), from the ledger, never guessed from the ID."""
    for row in ledger_view(pool)["slots"]:
        if str(row["id"]) == str(slot):
            place = row.get("dir") or str(row["id"])
            if not isinstance(place, str) or place.startswith("/") or ".." in place.split("/"):
                raise LedgerError(f"slot {slot} has an unsafe recorded dir {place!r}")
            return os.path.join(pool, "payload", "slots", place)
    raise LedgerError(f"slot {slot} is not in the ledger")


def slot_line(slot: str, state: str, profile: str, owner: str) -> re.Pattern:
    """`oxide status` slot line (main.rs slot_line): id, state, profile,
    project label, owner, size, place."""
    return re.compile(rf"^  slot {re.escape(str(slot))} {state} {profile} project \S+ {re.escape(owner)} ", re.M)


# ------------------------------------------------------------- environments

DROPPED = ("CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "CARGO_BUILD_RUSTC",
           "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "RUSTC",
           "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN")


def fixture_environment(env: dict, toolchain: str) -> dict:
    """Freeze the fixture child environment on top of the legacy Runner's."""
    out = {k: v for k, v in env.items()
           if k not in DROPPED and not (k.startswith("OXIDE_") and k not in FIXTURE_OXIDE)}
    out.update({"RUSTUP_TOOLCHAIN": toolchain, "RUSTFLAGS": "", "RUSTUP_AUTO_INSTALL": "0",
                "OXIDE_POOL_CAP": "128MiB"})
    for key in FIXTURE_OXIDE:
        if key in env:
            out[key] = env[key]
    return out


FIXTURE_OXIDE = ("OXIDE_POOL", "OXIDE_POOL_CREATE", "OXIDE_TEST_HOME_FROM_ENV")


def staging_environment(env: dict, target: str) -> dict:
    out = {k: v for k, v in env.items() if k not in DROPPED and not k.startswith(("OXIDE_", "XDG_"))}
    out.update({"RUSTUP_TOOLCHAIN": PINNED, "RUSTFLAGS": "", "RUSTUP_AUTO_INSTALL": "0",
                "CARGO_TARGET_DIR": target, "CARGO_PROFILE_DEV_DEBUG": "0"})
    for key in ("OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE", "OXIDE_TASK_SCRATCH_ROOT"):
        if key in env:
            out[key] = env[key]
    return out


def phase_parent(env: dict) -> str:
    raw = env.get("OXIDE_TASK_SCRATCH_ROOT")
    if not raw or not os.path.isdir(raw):
        raise RuntimeError("OXIDE_TASK_SCRATCH_ROOT from the qualified guard is required")
    root = os.path.realpath(raw)
    if root.startswith("/private/var/folders") or root == os.path.realpath(os.path.expanduser("~")):
        raise RuntimeError(f"scratch root {root} is not an owned guard root")
    return tempfile.mkdtemp(prefix="f083-", dir=root)


STOP_RESERVE_S = 30


def uptime_ns() -> int:
    """The ordinary guard's one clock (bounded-test.py now_ns)."""
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


def work_seconds(env: dict, now_uptime_ns: int) -> float:
    """Work seconds left: the guard's ABSOLUTE deadline (exported in
    CLOCK_UPTIME_RAW ns as start + 300 s) minus now on the same clock, minus
    the 30 s stop reserve the script keeps for the guard. Time already spent
    in setup and the gate is therefore charged; nothing is reset."""
    raw = env.get("OXIDE_PROOF_DEADLINE_NS")
    if not raw or not raw.isdigit():
        raise RuntimeError("OXIDE_PROOF_DEADLINE_NS from the qualified guard is required")
    left = (int(raw) - now_uptime_ns) / 1e9 - STOP_RESERVE_S
    return max(0.0, min(LIMITS["guard_work_seconds"], left))


def install_clock(module, env: dict, clock) -> None:
    """Every legacy helper bounds its children with `remaining()`; replace it
    with the guard-deadline remainder read fresh on the guard's clock."""
    def remaining() -> float:
        left = work_seconds(env, clock())
        if left <= 0:
            raise RuntimeError("work window before the guard's stop reserve is spent")
        return left
    module.remaining = remaining


# ---------------------------------------------------------------- activation


def parse_binding(text: str, digest: str, head: str) -> dict:
    """A planner-issued one-occurrence binding: JSON naming this task, the
    exact head, the selector and the decision reference; its SHA-256 must
    match the separately issued digest."""
    if not text or not digest:
        raise ValueError("no planner binding issued")
    if hashlib.sha256(text.encode()).hexdigest() != digest:
        raise ValueError("binding digest mismatch")
    try:
        binding = json.loads(text)
    except json.JSONDecodeError as error:
        raise ValueError(f"binding is not JSON: {error}") from None
    want = {"task": "oxide#083", "selector": "proof:foundation:rust-1-100:diagnostic", "head": head}
    for key, value in want.items():
        if not isinstance(binding, dict) or binding.get(key) != value:
            raise ValueError(f"binding {key} does not match {value!r}")
    if not isinstance(binding.get("decision"), str) or not binding["decision"]:
        raise ValueError("binding names no planner decision")
    return binding


def toolchain_installed(rustup_home: str, toolchain: str) -> bool:
    try:
        return any(name.startswith(f"{toolchain}-") for name in os.listdir(os.path.join(rustup_home, "toolchains")))
    except OSError:
        return False


def gate(argv: list) -> int:
    args = dict(zip(argv[1::2], argv[2::2])) if len(argv) % 2 == 1 else {}
    try:
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True,
                              timeout=10).stdout.strip()
        parse_binding(args.get("--binding", ""), args.get("--sha256", ""), head)
        home = os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
        for toolchain in TOOLCHAINS:
            if not toolchain_installed(home, toolchain):
                raise ValueError(f"toolchain {toolchain} is not installed; this proof never installs one")
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"refused before any effect: {error}", file=sys.stderr)
        return 2
    return 0


# --------------------------------------------------------------------- cases


def load_legacy():
    spec = importlib.util.spec_from_file_location("oxide_cli_legacy", os.path.join(REPO, LEGACY))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def directory_runner(legacy, oxide: str, parent: str, toolchain: str):
    class DirectoryRunner(legacy.Runner):
        """legacy.Runner without the retired lease CLI: state comes from the
        shipped ledger and control state; the env is frozen."""

        def __init__(self):
            super().__init__(oxide, None, parent)
            self.env = fixture_environment(self.env, toolchain)

        def lease_status(self) -> dict:
            return ledger_view(self.pool)

        def state(self) -> dict:
            return control_state(self.pool)

    return DirectoryRunner()


def case_profiles(legacy, runner, app, ctx) -> None:
    code, _, stderr = runner.run(app, ["build", "-v"])
    legacy.expect(code, 0, "quick build", stderr)
    first = legacy.summary(stderr, "quick build")
    if (first["profile"], first["strategy"], first["warmth"]) != ("quick", "build", "cold"):
        legacy.fail(f"quick build summary {first}")
    if "budget-only" not in stderr:
        legacy.fail("summary does not name the directory-budget backend")
    units = legacy.debuginfo(stderr)
    for crate in ("app", "helper"):
        if units.get(crate) != "line-tables-only":
            legacy.fail(f"quick {crate} debuginfo {units.get(crate)!r}")
    if not os.path.isfile(os.path.join(app, "target", "debug", "app")):
        legacy.fail("quick build did not place the binary in target/debug (oxide#160)")
    code, _, stderr = runner.run(app, ["build"])
    legacy.expect(code, 0, "warm build", stderr)
    if legacy.summary(stderr, "warm build") != {**first, "warmth": "warm"}:
        legacy.fail("second build was not warm on the same slot")
    code, _, stderr = runner.run(app, ["check", "--profile", "debug", "-v"])
    legacy.expect(code, 0, "debug check", stderr)
    debug = legacy.summary(stderr, "debug check")
    if debug["profile"] != "debug" or debug["warmth"] != "cold" or debug["slot"] == first["slot"]:
        legacy.fail(f"debug check summary {debug}")
    units = legacy.debuginfo(stderr)
    for crate in ("app", "helper", "build_script_build"):
        if units.get(crate) != "2":
            legacy.fail(f"debug {crate} debuginfo {units.get(crate)!r}")
    ctx["quick"], ctx["debug"] = first["slot"], debug["slot"]


def case_routing(legacy, runner, app, ctx) -> None:
    quick = ctx["quick"]
    build_dir = os.path.join(slot_dir(runner.pool, quick), "build")
    probe = os.path.join(runner.bin, "cargo-oxprobe")
    with open(probe, "w", encoding="utf-8") as handle:
        handle.write(legacy.PROBE)
    os.chmod(probe, 0o700)
    for cargo_profile, expected in (("oxide-quick", "line-tables-only"), ("oxide-debug", "2")):
        code, stdout, stderr = runner.run(app, ["oxprobe", cargo_profile])
        legacy.expect(code, 0, f"external {cargo_profile}", stderr)
        if f"BUILD_DIR={build_dir}" not in stdout:
            legacy.fail(f"external subcommand build dir: {stdout!r}")
        units = legacy.debuginfo(stderr)
        for crate in ("app", "helper"):
            if units.get(crate) != expected:
                legacy.fail(f"external {cargo_profile} {crate} debuginfo {units.get(crate)!r}")
        if legacy.summary(stderr, "external")["slot"] != quick:
            legacy.fail("external subcommand did not use the quick slot")
    code, _, stderr = runner.run(app, ["test"])
    legacy.expect(code, 0, "quick test", stderr)
    tested = legacy.summary(stderr, "quick test")
    if tested["strategy"] != "build+execute" or tested["slot"] != quick:
        legacy.fail(f"quick test summary {tested}")
    failing = {"OXIDE_FIXTURE_FAIL": "1"}
    oxide_code, _, _ = runner.run(app, ["test"], failing)
    plain = subprocess.run(["cargo", "test"], cwd=app,
                           env={**{k: v for k, v in runner.env.items() if k != "OXIDE_POOL"}, **failing},
                           capture_output=True, text=True, timeout=min(120, legacy.remaining()))
    if oxide_code != plain.returncode or oxide_code != 101:
        legacy.fail(f"failing test exit oxide={oxide_code} cargo={plain.returncode}")
    code, stdout, stderr = runner.run(app, ["run", "--", "exit:7", "hello"])
    legacy.expect(code, 7, "run exit code", stderr)
    if "app 42 exit:7 hello" not in stdout:
        legacy.fail(f"run output {stdout!r}")
    code, stdout, stderr = runner.run(app, ["run", "--bin", "other"])
    legacy.expect(code, 0, "run --bin other", stderr)
    if stdout.strip() != "other 2":
        legacy.fail(f"run --bin other output {stdout!r}")
    if any(slot["state"] != "idle" for slot in ledger_view(runner.pool)["slots"]):
        legacy.fail("run left a slot held after executing the program")
    virtual = os.path.join(runner.parent, "checkouts", "virtual")
    legacy.write_fixture(virtual, legacy.VIRTUAL)
    for cwd, want in ((virtual, "virtual app"), (os.path.join(virtual, "tool"), "virtual tool")):
        code, stdout, stderr = runner.run(cwd, ["run"])
        legacy.expect(code, 0, f"run from {cwd}", stderr)
        if stdout.strip() != want:
            legacy.fail(f"default-run from {cwd} output {stdout!r}")
    ledger = os.path.join(runner.pool, "ledger.json")
    with open(ledger, "rb") as handle:
        before = handle.read()
    code, stdout, stderr = runner.run(app, ["metadata", "--format-version", "1", "--no-deps"])
    legacy.expect(code, 0, "plain metadata", stderr)
    if "app" not in [p["name"] for p in json.loads(stdout)["packages"]] or "oxide:" in stderr:
        legacy.fail("metadata passthrough output changed")
    for args in (["build", "--target-dir", "x"], ["test", "--config", "build.target-dir='x'"]):
        code, _, stderr = runner.run(app, args)
        legacy.expect(code, 1, f"refusal {args}", stderr)
    with open(ledger, "rb") as handle:
        if handle.read() != before:
            legacy.fail("plain passthrough or a refusal changed the ledger")


def case_status(legacy, runner, app, ctx) -> None:
    code, stdout, stderr = runner.run(app, ["status"])
    legacy.expect(code, 0, "status", stderr)
    for pattern in (slot_line(ctx["quick"], "idle", "quick", app), slot_line(ctx["debug"], "idle", "debug", app)):
        if not pattern.search(stdout):
            legacy.fail(f"status lacks {pattern.pattern!r}: {stdout[-1500:]}")
    for needle in (f"pool: {runner.pool}", "waiting commands 0"):
        if needle not in stdout:
            legacy.fail(f"status missing {needle!r}")
    with open(os.path.join(runner.pool, "ledger.json"), "rb") as handle:
        before = handle.read()
    runner.run(app, ["status"])
    with open(os.path.join(runner.pool, "ledger.json"), "rb") as handle:
        if handle.read() != before:
            legacy.fail("default status changed the ledger")


def case_clean_cross(legacy, runner, app, ctx) -> None:
    quick = ctx["quick"]
    code, _, stderr = runner.run(app, ["clean"])
    legacy.expect(code, 0, "clean", stderr)
    if os.path.exists(os.path.join(app, "target")):
        legacy.fail("clean left the checkout target")
    if not os.listdir(os.path.join(slot_dir(runner.pool, quick), "build")):
        legacy.fail("clean emptied the slot")
    code, stdout, _ = runner.run(app, ["status"])
    if not slot_line(quick, "idle", "quick", "no checkout affinity").search(stdout):
        legacy.fail(f"clean kept affinity: {stdout[-1500:]}")
    code, _, stderr = runner.run(app, ["build"])
    legacy.expect(code, 0, "build after clean", stderr)
    rebuilt = legacy.summary(stderr, "build after clean")
    if rebuilt["slot"] == quick or rebuilt["warmth"] != "cold":
        legacy.fail(f"a detached slot was reused after clean: {rebuilt}")
    app2 = os.path.join(runner.parent, "checkouts", "app2")
    legacy.git(os.path.dirname(app2), "clone", "--quiet", "--no-hardlinks", app, app2)
    code, _, stderr = runner.run(app2, ["build"])
    legacy.expect(code, 0, "second checkout build", stderr)
    second = legacy.summary(stderr, "second checkout")
    if ctx["verified"]:
        if second["warmth"] != "warm" or second["slot"] != rebuilt["slot"] or "freshness gate refreshed" not in stderr:
            legacy.fail(f"second checkout did not take the idle slot through the gate: {stderr[-800:]}")
    elif second["slot"] == rebuilt["slot"] or "freshness gate off: " not in stderr:
        legacy.fail(f"unverified Cargo shared an affine slot: {stderr[-800:]}")
    ctx["app2"] = app2


STRAY_PREFIX = "stray Cargo output:"


def stray_measured(legacy, runner, cwd: str, label: str) -> str:
    """The stray line comes only from `oxide status --measure`: recorded
    status prints `not measured by status` (main.rs print_checkout)."""
    code, stdout, stderr = runner.run(cwd, ["status", "--measure"])
    legacy.expect(code, 0, f"status --measure {label}", stderr)
    return stray_line_of(stdout)


def stray_line_of(stdout: str) -> str:
    lines = [line for line in stdout.splitlines() if line.startswith(STRAY_PREFIX)]
    if len(lines) != 1:
        raise RuntimeError(f"expected one stray line, got {len(lines)}")
    if "not measured by status" in lines[0] or "scan unavailable" in lines[0] or "(partial scan)" in lines[0]:
        raise RuntimeError(f"stray line is not a complete measurement: {lines[0]}")
    return lines[0]


def unverified_stray_ok(line: str, target: str, cargo: str) -> str:
    """For an unverified Cargo, report_stray prints `holds` when deps/ or
    incremental/ holds files (layout v2 keeps <profile>/incremental) and
    `unknown` otherwise; `none` is never valid. Returns which."""
    if line.startswith(f"{STRAY_PREFIX} {target} holds plain-Cargo deps/ or incremental/ ("):
        return "holds"
    if line.startswith(f"{STRAY_PREFIX} unknown in {target} ({cargo} is not a Cargo release"):
        return "unknown"
    raise RuntimeError(f"unverified Cargo stray line is neither holds nor unknown: {line}")


def case_layout(legacy, runner, app, ctx) -> None:
    """legacy.prove_layout with the stray reads through `status --measure`."""
    root = os.path.join(runner.parent, "gated")
    a, c = os.path.join(root, "a"), os.path.join(root, "c")
    leaked = [path for path in legacy.walk_dirs(os.path.join(a, "target"), {"deps", "incremental", ".fingerprint"})
              if any(files for _, _, files in os.walk(path))]
    if leaked:
        legacy.fail(f"build.build-dir was not effective; intermediates in target: {leaked}")
    slots = os.path.join(runner.pool, "payload", "slots")
    units = legacy.walk_dirs(slots, {".fingerprint", "deps"})
    if not any(p.endswith(".fingerprint") for p in units) or not any(p.endswith("deps") for p in units):
        legacy.fail(f"slot build directories lack deps/ or .fingerprint/: {units}")
    line = stray_measured(legacy, runner, a, "after oxide only")
    ctx["observed"]["stray_oxide_only"] = line
    if not line.startswith(f"{STRAY_PREFIX} none "):
        legacy.fail(f"verified Cargo with only Oxide output did not report none: {line}")
    legacy.decode_dep_info(runner, ctx["decoder"], slots, "slots")
    plain = subprocess.run(["cargo", "check", "--offline"], cwd=c, env=runner.environment(None),
                           capture_output=True, text=True, timeout=min(120, legacy.remaining()))
    legacy.expect(plain.returncode, 0, "plain cargo check", plain.stderr)
    line = stray_measured(legacy, runner, c, "after plain cargo")
    ctx["observed"]["stray_plain"] = line
    if "holds plain-Cargo deps/ or incremental/" not in line:
        legacy.fail(f"plain Cargo output was not reported as stray: {line}")
    legacy.decode_dep_info(runner, ctx["decoder"], os.path.join(c, "target"), "plain target")


def case_fallback(legacy, runner, app, ctx) -> None:
    root = os.path.join(runner.parent, "gated-fallback")
    a, b = os.path.join(root, "a"), os.path.join(root, "b")
    legacy.write_fixture(os.path.join(root, "outside"), legacy.OUTSIDE)
    legacy.write_fixture(a, legacy.GATED)
    legacy.commit_lock(runner, a)
    legacy.git(a, "init", "--quiet")
    legacy.git(a, "add", "-A")
    legacy.git(a, "commit", "--quiet", "-m", "gated fixture")
    legacy.git(a, "worktree", "add", "--quiet", b)
    cargo = ctx["identity"][0]
    note = f"freshness gate off: {cargo} is not a Cargo release Oxide verified; the slot stays with this checkout"
    code, _, stderr = runner.run(a, ["test"])
    legacy.expect(code, 0, "fallback A", stderr)
    first = legacy.summary(stderr, "fallback A")
    if first["warmth"] != "cold" or note not in stderr:
        legacy.fail(f"unverified Cargo did not keep A's slot affine: {stderr[-800:]}")
    code, _, stderr = runner.run(b, ["test"])
    legacy.expect(code, 0, "fallback B", stderr)
    second = legacy.summary(stderr, "fallback B")
    if note not in stderr or second["warmth"] != "cold" or second["slot"] == first["slot"]:
        legacy.fail(f"B reused A's unverified slot: {stderr[-800:]}")
    ctx["fallback_root"] = a


def case_layout_observation(legacy, runner, app, ctx) -> None:
    a = ctx["fallback_root"]
    plain = subprocess.run(["cargo", "check", "--offline"], cwd=a, env=runner.environment(None),
                           capture_output=True, text=True, timeout=min(120, legacy.remaining()))
    legacy.expect(plain.returncode, 0, "plain cargo check", plain.stderr)
    layout = {name: bool(legacy.walk_dirs(os.path.join(a, "target"), {name})) for name in ("deps", "incremental", ".fingerprint", "build")}
    ctx["observed"]["plain_layout_dirs"] = layout
    line = stray_measured(legacy, runner, a, "unverified plain cargo")
    ctx["observed"]["stray"] = line
    ctx["observed"]["stray_kind"] = unverified_stray_ok(line, os.path.join(a, "target"), ctx["identity"][0])
    shaped = reshape_v2_dep_info([os.path.join(a, "target"), os.path.join(runner.pool, "payload", "slots")],
                                 os.path.join(runner.parent, "dep-info-v2"))
    ctx["observed"]["v2_dep_info_files"] = shaped
    code, out, err = runner.run(runner.parent, [os.path.join(a, "target"), os.path.join(runner.pool, "payload", "slots"),
                                                os.path.join(runner.parent, "dep-info-v2")], program=ctx["decoder"])
    ctx["observed"]["dep_info"] = {"exit": code, "stdout": out[-TAIL_BYTES:], "stderr": err[-TAIL_BYTES:]}
    if code < 0:
        legacy.fail(f"decoder died on signal {-code}")
    try:
        decoded = json.loads(out)
    except json.JSONDecodeError:
        legacy.fail("decoder printed no JSON summary")
    if decoded.get("files", 0) < 1:
        legacy.fail("no .fingerprint/<unit>/dep-* found: decoder exercise insufficient under this layout")


V2_DEP_INFO_FILES = 4096
V2_DEP_INFO_BYTES = 16 * MIB


def reshape_v2_dep_info(roots: list, dest: str) -> int:
    """Layout v2 keeps dep-info at build/<pkg>/<meta>/fingerprint/dep-*; the
    shipped walker (examples/dep_info_matrix.rs) only finds
    .fingerprint/<unit>/dep-*. Copy each v2 file, unchanged and bounded, into
    dest/.fingerprint/<n>/ so the SHIPPED decoder judges its bytes. No Rust
    change, no decoder change; a v1 tree contributes nothing here."""
    count = 0
    for root in roots:
        for path, dirs, files in os.walk(root):
            dirs[:] = [d for d in dirs if not os.path.islink(os.path.join(path, d))]
            if os.path.basename(path) != "fingerprint" or os.path.basename(os.path.dirname(os.path.dirname(path))) == "":
                continue
            for name in sorted(files):
                source = os.path.join(path, name)
                if not name.startswith("dep-") or os.path.islink(source) or not os.path.isfile(source):
                    continue
                if count >= V2_DEP_INFO_FILES or os.path.getsize(source) > V2_DEP_INFO_BYTES:
                    raise RuntimeError("v2 dep-info over the decoder's file or byte ceiling")
                target = os.path.join(dest, ".fingerprint", str(count))
                os.makedirs(target, mode=0o700)
                shutil.copyfile(source, os.path.join(target, name))
                count += 1
    return count


def probe_gate(calls_made: int, started: float, now: float) -> float:
    """The probe's own budget: at most 8 Cargo calls (the tested toolchain's
    version pair is charged up front) and a 90 s window that starts when the
    probe case starts, not at phase start. Returns the seconds left."""
    left = LIMITS["probe_seconds_aggregate"] - (now - started)
    if calls_made >= LIMITS["probe_cargo_calls_per_participant"] or left <= 0:
        raise RuntimeError(f"probe call or time budget exhausted ({calls_made} calls, {now - started:.1f} s)")
    return left


def case_probes(legacy, runner, app, ctx) -> None:
    calls = ctx["probe_calls"]
    started = time.monotonic()
    root = tempfile.mkdtemp(prefix="probe-", dir=runner.parent)
    with open(os.path.join(root, "Cargo.toml"), "w", encoding="utf-8") as handle:
        handle.write('[package]\nname = "rcgu"\nversion = "0.1.0"\nedition = "2021"\n')
    os.makedirs(os.path.join(root, "src"))
    main = os.path.join(root, "src", "main.rs")

    def cargo(args: list) -> subprocess.CompletedProcess:
        left = probe_gate(calls[0], started, time.monotonic())
        calls[0] += 1
        return subprocess.run(["cargo", *args], cwd=root, env=runner.environment({"CARGO_TARGET_DIR": os.path.join(root, "target")}),
                              capture_output=True, text=True, timeout=min(60, left, legacy.remaining()))

    counts = []
    for step in range(5):
        with open(main, "w", encoding="utf-8") as handle:
            handle.write(f"fn main() {{ println!(\"{step}\"); }}\n")
        result = cargo(["build", "--offline"])
        legacy.expect(result.returncode, 0, f"probe build {step}", result.stderr)
        counts.append(sum(1 for _, _, files in os.walk(os.path.join(root, "target")) for f in files if f.endswith(".rcgu.o")))
    size = legacy.tree_bytes(root, False)
    ctx["observed"]["rcgu_counts"] = counts
    ctx["observed"]["probe_seconds"] = round(time.monotonic() - started, 1)
    ctx["observed"]["probe_bytes"] = size
    if size > LIMITS["probe_scratch_bytes"]:
        legacy.fail(f"probe scratch {size} exceeds 64 MiB")


CASE_BODIES = {
    "profiles": case_profiles, "routing": case_routing, "status": case_status,
    "clean-cross-checkout": case_clean_cross, "fallback": case_fallback,
    "layout-observation": case_layout_observation, "probes": case_probes, "layout": case_layout,
}


def legacy_body(legacy, key: str):
    def waiting(r, app, ctx):
        legacy.prove_waiting(r, app, ctx["app2"])
        legacy.prove_clean_waits(r, app)
    table = {
        "waiting": waiting,
        "interrupt": lambda r, app, ctx: legacy.prove_interrupt(r, app),
        "crash-recovery": lambda r, app, ctx: legacy.prove_recovery(r, app),
        "group-gone": lambda r, app, ctx: legacy.prove_group_gone(r, app),
        "freshness-gate": lambda r, app, ctx: legacy.prove_freshness(r),
    }
    return table[key]


def selected_cases(verified: bool, tested: bool) -> list:
    keep = []
    for key, deps, applies, _, _ in CASES:
        if applies == "both" or (applies == "verified" and verified) or (applies == "unverified" and not verified) \
                or (applies == "tested" and tested):
            keep.append((key, deps))
    return keep


def collect(cases: list, run_case) -> dict:
    """Keep-going: a failed case taints its dependents (skipped); an
    exception that is not a case failure aborts the rest (unexecuted)."""
    outcome, order = {}, []
    aborted = False
    for key, deps in cases:
        order.append(key)
        if aborted:
            outcome[key] = {"state": "unexecuted"}
            continue
        if any(outcome.get(dep, {}).get("state") != "completed" for dep in deps if dep in outcome):
            outcome[key] = {"state": "skipped", "reason": "dependency did not complete"}
            continue
        try:
            run_case(key)
            outcome[key] = {"state": "completed"}
        except (RuntimeError, LedgerError, AssertionError) as error:
            outcome[key] = {"state": "failed", "reason": str(error)[-TAIL_BYTES:]}
        except BaseException as error:  # unknown outcome: stop, keep resources
            outcome[key] = {"state": "aborted", "reason": f"{type(error).__name__}: {error}"[-TAIL_BYTES:]}
            aborted = True
    return {"order": order, "cases": outcome, "aborted": aborted}


def bounded_receipt(document: dict) -> bytes:
    raw = json.dumps(document, indent=1, sort_keys=True).encode()
    if len(raw) > RECEIPT_BYTES:
        document = {**document, "cases": {k: {"state": v.get("state")} for k, v in document.get("cases", {}).items()},
                    "observed": {"omitted": "receipt over 1 MiB"}, "truncated": True}
        raw = json.dumps(document, indent=1, sort_keys=True).encode()
    return raw


def diagnostic(toolchain: str) -> int:
    """One guarded phase. Never run in the source phase; the selector's gate
    refuses before the guard starts until a planner binding exists."""
    if toolchain not in TOOLCHAINS:
        print(f"usage: --toolchain {' | '.join(TOOLCHAINS)}", file=sys.stderr)
        return DIAGNOSTIC_REFUSED
    # Re-check the planner binding inside the guard before any effect; the
    # selector's outer gate already refused if it was absent.
    if not os.environ.get("FOUNDATION_083_BINDING") or not os.environ.get("FOUNDATION_083_BINDING_SHA256"):
        print("refused before any effect: no planner binding issued", file=sys.stderr)
        return DIAGNOSTIC_REFUSED
    if gate(["--gate", "--binding", os.environ["FOUNDATION_083_BINDING"],
             "--sha256", os.environ["FOUNDATION_083_BINDING_SHA256"]]) != 0:
        return DIAGNOSTIC_REFUSED
    legacy = load_legacy()
    env = dict(os.environ)
    legacy.STARTED = time.monotonic()  # elapsed reporting only
    install_clock(legacy, env, uptime_ns)
    legacy.remaining()  # refuse before any effect if the window is already spent
    receipt = {"schema": "oxide.foundation-083/1", "toolchain": toolchain, "qualified": False,
               "guard_sha256": sha256(GUARD), "observed": {}}
    evidence = env.get("OXIDE_PROOF_EVIDENCE")
    try:
        parent = phase_parent(env)
        legacy.reserve(parent, "admission", "f083")
        stage = staging_environment(env, os.path.join(parent, "stage-target"))
        builder = legacy.toolchain_identity(stage, REPO)
        if builder != PINNED_IDENTITY:
            legacy.fail(f"builder is not {PINNED}: {builder}")
        executables = legacy.build_binaries(("--offline", "--example", "dep_info_matrix"), stage)
        runner = directory_runner(legacy, executables["oxide"], parent, toolchain)
        identity = legacy.toolchain_identity(runner.env, parent)
        if toolchain == PINNED and identity != PINNED_IDENTITY:
            legacy.fail(f"tested toolchain is not {PINNED}: {identity}")
        if toolchain == TESTED and (identity[1] != TESTED_RUSTC or not identity[0].startswith(TESTED_CARGO_PREFIX)):
            legacy.fail(f"tested toolchain is not {TESTED}: {identity}")
        verified = any(identity[0].startswith(entry) for entry in verified_cargo())
        receipt.update({"identity": identity, "verified": verified,
                        "oxide_sha256": hashlib.sha256(open(executables["oxide"], "rb").read()).hexdigest()})
        app = os.path.join(parent, "checkouts", "app")
        legacy.write_fixture(app)
        if legacy.tree_bytes(app, False) > LIMITS["fixture_source_bytes"]:
            legacy.fail("fixture source exceeds 8 MiB")
        legacy.commit_lock(runner, app)
        legacy.git(app, "init", "--quiet")
        legacy.git(app, "add", "-A")
        legacy.git(app, "commit", "--quiet", "-m", "fixture")
        ctx = {"identity": identity, "verified": verified, "decoder": executables["dep_info_matrix"],
               "observed": receipt["observed"], "probe_calls": [2]}

        def run_case(key: str) -> None:
            body = CASE_BODIES.get(key) or (lambda r, a, c, f=legacy_body(legacy, key): f(r, a, c))
            if key in CASE_BODIES:
                body(legacy, runner, app, ctx)
            else:
                body(runner, app, ctx)
            legacy.reserve(parent, f"after {key}", "f083")
            owned = legacy.tree_bytes(parent, True)
            if owned > LIMITS["aggregate_scratch_bytes"]:
                raise RuntimeError(f"owned scratch {owned} exceeds 512 MiB")
            receipt["observed"]["owned_peak"] = max(owned, receipt["observed"].get("owned_peak", 0))

        receipt.update(collect(selected_cases(verified, toolchain == TESTED), run_case))
        receipt["observed"]["probe_cargo_calls"] = ctx["probe_calls"][0]
    except BaseException as error:
        receipt["setup_fault"] = f"{type(error).__name__}: {error}"[-TAIL_BYTES:]
    receipt["elapsed_s"] = round(time.monotonic() - legacy.STARTED, 1)
    status = phase_status(receipt)
    receipt["phase_status"] = status
    written = False
    if evidence and os.path.isdir(evidence):
        try:
            with open(os.path.join(evidence, f"foundation-083-{toolchain}.json"), "wb") as handle:
                handle.write(bounded_receipt(receipt))
            written = True
        except OSError as error:
            print(f"receipt not written: {error}", file=sys.stderr)
    print(json.dumps({k: receipt.get(k) for k in ("toolchain", "verified", "aborted", "setup_fault", "elapsed_s", "phase_status")}))
    # Scratch, pool and slots stay inside the guard root for its census; the
    # guard's retention policy owns cleanup. Always nonzero: uncounted.
    return status if written else DIAGNOSTIC_NO_RECEIPT


def phase_status(receipt: dict) -> int:
    """DONE only for a complete, non-aborted collection; ordinary case
    failures inside it stay keep-going results in the receipt."""
    if receipt.get("setup_fault") or receipt.get("aborted") is not False or "cases" not in receipt:
        return DIAGNOSTIC_INCOMPLETE
    return DIAGNOSTIC_DONE


# ------------------------------------------------------------ pure controls


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
    top = [ast.unparse(s) for s in main.body if not isinstance(s, (ast.If, ast.Global))]
    found["default"] = bool(top) and top[0] == "fail(RETIRED_POOL_PROOF)"
    return found


def legacy_names() -> set:
    tree = ast.parse(read(LEGACY).decode())
    return {target.id for node in tree.body if isinstance(node, ast.Assign) for target in node.targets
            if isinstance(target, ast.Name)} | {"STARTED", "PROOF_SECONDS"}


def legacy_reads_retired(name: str) -> bool:
    """Whether a legacy helper calls the retired lease CLI (`program=runner.lease`
    or a standalone lease helper) directly rather than via the shimmed readers."""
    tree = ast.parse(read(LEGACY).decode())
    node = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == name)
    text = ast.unparse(node)
    return bool(re.search(r"runner\.lease\b", text)) or any(t in text for t in ("take_standalone_lease", "keep_standalone_lease", "apfs-sparse-image", "hdiutil"))


def self_check() -> int:
    checks = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        checks.append((name, bool(ok), detail))

    def raises(fn, *args) -> bool:
        try:
            fn(*args)
        except (LedgerError, ValueError, RuntimeError):
            return True
        return False

    check("guard digest", sha256(GUARD) == GUARD_SHA256, sha256(GUARD))
    refusals = legacy_refusals()
    for mode in ("--cargo-matrix", "--toolchain-context", "default"):
        check(f"suspended {mode} still refuses before its body", refusals.get(mode) is True)
    table = verified_cargo()
    check("VERIFIED_CARGO keeps the five pins", len(table) == 5 and any(e.startswith("cargo 1.97.1 (c980f4866 ") for e in table))
    check("1.100.0 not verified: phase 2 selects fallback cases", not any(e.startswith("cargo 1.100.0") for e in table))
    keys2 = [k for k, _ in selected_cases(False, True)]
    keys1 = [k for k, _ in selected_cases(True, False)]
    check("phase 1 runs gate+layout, no fallback/probes", "freshness-gate" in keys1 and "layout" in keys1
          and "fallback" not in keys1 and "probes" not in keys1)
    check("phase 2 runs fallback+observation+probes, no shared gate", "fallback" in keys2 and "probes" in keys2
          and "freshness-gate" not in keys2)
    retained = {"profiles", "routing", "status", "clean-cross-checkout", "waiting", "interrupt", "crash-recovery", "group-gone"}
    check("every retained oracle runs on both toolchains", retained <= set(keys1) and retained <= set(keys2))
    dep = read(DEP_INFO).decode()
    check("dep-info decoder still pins version 1", "version 1" in dep and "refuses" in dep)
    for rel, needle in SOURCE_NEEDLES:
        check(f"oracle text in {rel}: {needle[:48]}", needle in read(rel).decode())
    for _, _, _, body, _ in CASES:
        for name in re.findall(r"legacy\.(\w+)", body) + re.findall(r"\+(prove_\w+)", body):
            check(f"legacy {name} exists and avoids retired surfaces", name in functions(LEGACY) and not legacy_reads_retired(name))
    for name in ("prove_native", "prove_routing", "prove_fallback", "prove_layout", "stray_line"):
        check(f"legacy {name} is replaced, not reused", not any(name in b for _, _, _, b, _ in CASES))
    check("legacy prove_routing/native carry retired text (why rewritten)", legacy_reads_retired("prove_native"))
    main_text = read(MAIN).decode()
    check("recorded status measures no stray output (why --measure)",
          'println!("stray Cargo output: not measured by status; use `oxide status --measure`");' in main_text)
    check("legacy stray_line reads recorded status (why not reused)", "['status']" in ast.unparse(
        next(n for n in ast.parse(read(LEGACY).decode()).body if isinstance(n, ast.FunctionDef) and n.name == "stray_line")))
    used = {node.attr for node in ast.walk(ast.parse(read("scripts/foundation-rust-1-100.py").decode()))
            if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name) and node.value.id == "legacy"}
    check("no foundation case calls legacy stray_line/prove_layout", not used & {"stray_line", "prove_layout", "prove_fallback"})
    check("every legacy attribute used exists in the legacy source", used <= functions(LEGACY) | legacy_names())
    check("product recovery line matches the reused oracle", '"oxide: recovered slot {} after oxide pid {} died ({})"' in read("src/bin/oxide/store.rs").decode())
    t, cargo = "/w/a/target", "cargo 1.100.0 (3acc79c0c 2026-11-05)"
    check("unverified stray: holds accepted", unverified_stray_ok(
        f"stray Cargo output: {t} holds plain-Cargo deps/ or incremental/ (3 files, 0 MiB); `oxide clean` removes it", t, cargo) == "holds")
    check("unverified stray: unknown accepted", unverified_stray_ok(
        f"stray Cargo output: unknown in {t} ({cargo} is not a Cargo release Oxide verified; its layout is unchecked)", t, cargo) == "unknown")
    check("unverified stray: none refused", raises(unverified_stray_ok, f"stray Cargo output: none in {t}", t, cargo))
    check("unverified stray: other target refused", raises(unverified_stray_ok,
          f"stray Cargo output: /x/target holds plain-Cargo deps/ or incremental/ (1 files, 0 MiB)", t, cargo))
    for bad in ("outputs: x\n", "stray Cargo output: not measured by status; use `oxide status --measure`\n",
                "stray Cargo output: scan unavailable (e)\n", "stray Cargo output: none in /t (partial scan)\n",
                "stray Cargo output: none in /t\nstray Cargo output: none in /u\n"):
        check(f"stray line refused: {bad[:40]!r}", raises(stray_line_of, bad))
    check("one complete stray line accepted", stray_line_of("pool: p\nstray Cargo output: none in /t\n") == "stray Cargo output: none in /t")
    for _, _, rel, symbol in PREMISES:
        if rel:
            check(f"premise source {symbol} exists", symbol in functions(rel) or symbol in read(rel).decode())

    with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as tmp:
        pool = os.path.join(tmp, "pool")
        os.makedirs(os.path.join(pool, "oxide-managed"))
        ledger = os.path.join(pool, "ledger.json")
        state = os.path.join(pool, "oxide-managed", "state.json")

        def put(path, text):
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(text)
        check("missing ledger refuses", raises(ledger_view, pool))
        put(ledger, '{"version":1,"slots":[{"id":3,"state":"idle","lease_id":null,"dir":"app-1/quick-3"}]}')
        check("valid ledger reads", ledger_view(pool)["slots"][0]["id"] == 3)
        check("ledger rows carry the slot_id alias reused helpers read", ledger_view(pool)["slots"][0]["slot_id"] == "3")
        check("slot dir comes from the recorded nested dir", slot_dir(pool, "3").endswith("payload/slots/app-1/quick-3"))
        check("unknown slot refuses", raises(slot_dir, pool, "9"))
        put(ledger, '{"version":1,"slots":[{"id":3,"state":"idle","lease_id":null,"dir":"../x"}]}')
        check("escaping recorded dir refuses", raises(slot_dir, pool, "3"))
        for bad in ('{"version":1,"slots":[{"id":3,"state":"idle"}]}', '{"version":1,"slots":{}}',
                    '{"slots":[]}', '{"version":1,"slots":[', "[]"):
            put(ledger, bad)
            check(f"partial/malformed ledger refuses: {bad[:30]}", raises(ledger_view, pool))
        put(ledger, "{" + " " * (4 * MIB) + "}")
        check("oversized ledger refuses", raises(ledger_view, pool))
        put(state, '{"version":9,"records":[],"tickets":[],"cleans":[]}')
        check("valid state reads", control_state(pool)["records"] == [])
        put(state, '{"version":9,"records":[],"tickets":[]}')
        check("state without cleans refuses", raises(control_state, pool))
        put(state, '{"version":9,"records":[],"tickets":[],"cleans":[],"recoveries":{}}')
        check("state with non-list recoveries refuses", raises(control_state, pool))
        v2 = os.path.join(tmp, "t", "build", "app", "abc123", "fingerprint")
        os.makedirs(v2)
        put(os.path.join(v2, "dep-lib-app"), "x")
        put(os.path.join(v2, "invoked.timestamp"), "")
        v1 = os.path.join(tmp, "t", "debug", ".fingerprint", "app-1")
        os.makedirs(v1)
        put(os.path.join(v1, "dep-lib-app"), "y")
        dest = os.path.join(tmp, "shaped")
        check("v2 reshape copies only build/*/*/fingerprint/dep-* files",
              reshape_v2_dep_info([os.path.join(tmp, "t")], dest) == 1
              and os.listdir(os.path.join(dest, ".fingerprint", "0")) == ["dep-lib-app"])
        os.symlink(os.path.join(v2, "dep-lib-app"), os.path.join(v2, "dep-link"))
        check("v2 reshape ignores symlinks", reshape_v2_dep_info([os.path.join(tmp, "t")], os.path.join(tmp, "s2")) == 1)
        check("phase parent needs the guard root", raises(phase_parent, {}))
        parent = phase_parent({"OXIDE_TASK_SCRATCH_ROOT": tmp})
        check("phase parent is a private dir inside the guard root", os.path.dirname(parent) == os.path.realpath(tmp))

    hostile = {"CARGO_ENCODED_RUSTFLAGS": "-Cfoo", "RUSTFLAGS": "-Dwarnings", "RUSTC_WRAPPER": "/x", "OXIDE_CONFIG": "/c",
               "OXIDE_MAX_ACTIVE": "1", "CARGO_TARGET_DIR": "/t", "RUSTUP_TOOLCHAIN": "nightly", "XDG_STATE_HOME": "/x",
               "OXIDE_POOL": "/p", "OXIDE_POOL_CREATE": "1", "OXIDE_TEST_HOME_FROM_ENV": "1", "PATH": "/bin"}
    fixture = fixture_environment(hostile, TESTED)
    check("fixture env freezes flags and toolchain", fixture["RUSTFLAGS"] == "" and "CARGO_ENCODED_RUSTFLAGS" not in fixture
          and fixture["RUSTUP_TOOLCHAIN"] == TESTED and fixture["RUSTUP_AUTO_INSTALL"] == "0")
    check("fixture env drops foreign OXIDE_* and wrappers, keeps fixture pool controls",
          "OXIDE_CONFIG" not in fixture and "OXIDE_MAX_ACTIVE" not in fixture and "RUSTC_WRAPPER" not in fixture
          and "CARGO_TARGET_DIR" not in fixture and fixture["OXIDE_POOL"] == "/p" and fixture["OXIDE_POOL_CAP"] == "128MiB")
    stage = staging_environment({**hostile, "OXIDE_PROOF_DEADLINE_NS": "1"}, "/s")
    check("staging env is the pinned builder with frozen flags", stage["RUSTUP_TOOLCHAIN"] == PINNED and stage["RUSTFLAGS"] == ""
          and "CARGO_ENCODED_RUSTFLAGS" not in stage and "OXIDE_POOL" not in stage and "XDG_STATE_HOME" not in stage
          and stage["OXIDE_PROOF_DEADLINE_NS"] == "1" and stage["CARGO_TARGET_DIR"] == "/s")
    check("work window needs the guard deadline", raises(work_seconds, {}, 0))
    up, epoch = 987_654 * 10**9, 1_763_100_000 * 10**9  # synthetic uptime vs an unrelated epoch stamp
    guard_env = {"OXIDE_PROOF_DEADLINE_NS": str(up + 300 * 10**9)}  # guard: absolute = start + 300 s
    check("uptime deadline at phase start leaves 300 - 30 reserve", work_seconds(guard_env, up) == 270)
    check("an epoch stamp is a different domain (the old bug read 0)", work_seconds(guard_env, epoch) == 0)
    check("late setup leaves only the original remainder minus reserve", work_seconds(guard_env, up + 250 * 10**9) == 20)
    check("spent window (inside the reserve) leaves 0", work_seconds(guard_env, up + 280 * 10**9) == 0)

    class Fake:
        pass
    ticks = [up + 100 * 10**9]
    fake = Fake()
    install_clock(fake, guard_env, lambda: ticks[0])
    check("installed remaining() reads the guard clock fresh", fake.remaining() == 170)
    ticks[0] = up + 290 * 10**9
    check("installed remaining() refuses once only the reserve is left", raises(fake.remaining))
    tree = ast.parse(read("scripts/foundation-rust-1-100.py").decode())
    diag = ast.unparse(next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "diagnostic"))
    check("diagnostic installs the guard clock and checks it before effects",
          "install_clock(legacy, env, uptime_ns)" in diag and diag.index("legacy.remaining()") < diag.index("phase_parent("))
    check("no wall-clock or reset deadline anywhere in the recipe",
          not any(isinstance(n, ast.Attribute) and n.attr in ("time_ns", "PROOF_SECONDS") for n in ast.walk(tree)))
    check("uptime_ns is the guard's clock", "CLOCK_UPTIME_RAW" in ast.unparse(
          next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "uptime_ns"))
          and "time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)" in read(GUARD).decode())

    head = "a" * 40
    good = json.dumps({"task": "oxide#083", "selector": "proof:foundation:rust-1-100:diagnostic", "head": head, "decision": "d1"})
    digest = hashlib.sha256(good.encode()).hexdigest()
    check("binding accepted only when exact", parse_binding(good, digest, head)["decision"] == "d1")
    check("absent binding refuses", raises(parse_binding, "", "", head))
    check("digest mismatch refuses", raises(parse_binding, good, "0" * 64, head))
    check("other head refuses", raises(parse_binding, good, digest, "b" * 40))
    nodec = json.dumps({"task": "oxide#083", "selector": "proof:foundation:rust-1-100:diagnostic", "head": head})
    check("binding without decision refuses", raises(parse_binding, nodec, hashlib.sha256(nodec.encode()).hexdigest(), head))
    check("missing toolchain dir is not installed", not toolchain_installed("/nonexistent", TESTED))

    def run_case_factory(script):
        def run_case(key):
            effect = script.get(key)
            if effect == "fail":
                raise RuntimeError("oracle")
            if effect == "abort":
                raise KeyboardInterrupt()
        return run_case
    cases = selected_cases(True, False)
    result = collect(cases, run_case_factory({"profiles": "fail"}))
    check("failed root taints dependents, independents still run",
          result["cases"]["profiles"]["state"] == "failed" and result["cases"]["routing"]["state"] == "skipped"
          and result["cases"]["freshness-gate"]["state"] == "completed")
    result = collect(cases, run_case_factory({"status": "abort"}))
    check("unknown outcome aborts the rest unexecuted",
          result["aborted"] and result["cases"]["status"]["state"] == "aborted"
          and result["cases"]["layout"]["state"] == "unexecuted")
    big = bounded_receipt({"cases": {"x": {"state": "failed", "reason": "r"}}, "observed": {"blob": "x" * (2 * MIB)}})
    check("receipt is bounded to 1 MiB", len(big) <= RECEIPT_BYTES and b'"truncated": true' in big)

    check("probe window starts at the probe case: fresh window admits", probe_gate(2, 1000.0, 1000.0) == 90)
    check("probe window: late phase start is irrelevant", probe_gate(2, 5000.0, 5010.0) == 80)
    check("probe window: 90 s spent refuses", raises(probe_gate, 3, 1000.0, 1090.0))
    check("probe calls: the 9th refuses", raises(probe_gate, 8, 0.0, 1.0))
    check("probe calls: 2 identity + 5 builds stays within 8", probe_gate(6, 0.0, 1.0) > 0)
    probe_src = ast.unparse(next(n for n in ast.parse(read("scripts/foundation-rust-1-100.py").decode()).body
                                 if isinstance(n, ast.FunctionDef) and n.name == "case_probes"))
    check("case_probes stamps its own window and uses probe_gate",
          "started = time.monotonic()" in probe_src and "probe_gate(" in probe_src and "probe_start" not in probe_src)

    alloc = allocation()
    check("at most three focused invocations, selector spends two", alloc["focused_invocations"] <= 3 and COMMAND.count("--diagnostic") == 2)
    check("probe calls total 7 of 8 (rcgu baseline + four = 5)", alloc["probe_cargo_calls"] == 7 and alloc["rcgu_calls"] == 5)
    check("two phases' receipts within 32 MiB raw logs", 2 * RECEIPT_BYTES <= LIMITS["raw_log_bytes"])
    check("command: gate first, ordinary guard, original caps", COMMAND.startswith(GATE + " && ")
          and COMMAND.count(f"python3 {GUARD} --limit 300 --scratch-budget-mib 512 -- ") == 2)
    check("phase 1 DONE continues", phase_decision(1, DIAGNOSTIC_DONE) == ("continue", None))
    for status in (124, 125, 128 + 9, 128 + 15, DIAGNOSTIC_REFUSED, 1, DIAGNOSTIC_INCOMPLETE, DIAGNOSTIC_NO_RECEIPT, 0):
        check(f"phase 1 status {status} stops with that status", phase_decision(1, status) == ("stop", status))
    for status in (None, "3", -1, 256, True):
        check(f"phase 1 malformed status {status!r} stops as 125", phase_decision(1, status) == ("stop", 125))
    for status in (DIAGNOSTIC_DONE, 124, 125, 137):
        check(f"phase 2 status {status} is the final exit, not overwritten", phase_decision(2, status) == ("stop", status))
    between = COMMAND.split(PHASE.format(toolchain=PINNED), 1)[1].split(PHASE.format(toolchain=TESTED), 1)[0]
    check("shell stops after phase 1 unless DONE", between == f'; s=$?; echo "foundation-083 phase {PINNED} status $s"; '
          f'[ "$s" -eq {DIAGNOSTIC_DONE} ] || exit "$s"; ')
    check("shell's final exit is phase 2's own status (no unconditional exit 1)",
          COMMAND.endswith(f'; s=$?; echo "foundation-083 phase {TESTED} status $s"; exit "$s"; }}') and "exit 1" not in COMMAND)
    check("DONE is distinct from 0, crash 1, refusal, 124/125 and signals", DIAGNOSTIC_DONE not in (0, 1, 2, 124, 125)
          and len({DIAGNOSTIC_DONE, DIAGNOSTIC_INCOMPLETE, DIAGNOSTIC_NO_RECEIPT, DIAGNOSTIC_REFUSED}) == 4)
    check("phase status DONE only for a complete collection", phase_status({"cases": {}, "aborted": False}) == DIAGNOSTIC_DONE)
    check("case failures inside a closed phase stay DONE (keep-going)",
          phase_status({"cases": {"a": {"state": "failed"}}, "aborted": False}) == DIAGNOSTIC_DONE)
    check("aborted collection is INCOMPLETE", phase_status({"cases": {}, "aborted": True}) == DIAGNOSTIC_INCOMPLETE)
    check("setup fault is INCOMPLETE", phase_status({"setup_fault": "x"}) == DIAGNOSTIC_INCOMPLETE)
    check("missing collection is INCOMPLETE", phase_status({}) == DIAGNOSTIC_INCOMPLETE)
    declared = re.search(r'\[tasks\."proof:foundation:rust-1-100:diagnostic"\]\nadmission = "heavy"\nrun = \'(.*)\'\n',
                         read("effigy.toml").decode())
    check("effigy selector is heavy and runs exactly COMMAND", bool(declared) and declared.group(1) == COMMAND)
    saved = {k: os.environ.pop(k, None) for k in ("FOUNDATION_083_BINDING", "FOUNDATION_083_BINDING_SHA256")}
    try:
        check("diagnostic refuses without a binding (before any child)", diagnostic(TESTED) == 2)
        check("diagnostic refuses an unknown toolchain", diagnostic("1.99.1") == 2)
    finally:
        os.environ.update({k: v for k, v in saved.items() if v is not None})

    failed = [c for c in checks if not c[1]]
    for name, ok, detail in checks:
        print(f"{'ok  ' if ok else 'FAIL'} {name}" + (f" [{detail}]" if detail and not ok else ""))
    print(f"self-check: {len(checks) - len(failed)}/{len(checks)} passed")
    return 1 if failed else 0


def allocation() -> dict:
    return {
        "focused_invocations": len([i for i in INVOCATIONS if i[1] != "spare"]),
        "probe_cargo_calls": sum(calls for _, _, calls in PROBES),
        # baseline build + four rebuilds; the probe crate has no dependencies
        # and no lock call
        "rcgu_calls": dict((k, c) for k, _, c in PROBES)["plain build"] + dict((k, c) for k, _, c in PROBES)["rcgu"],
    }


def plan() -> int:
    print("oxide#083 foundation qualification on Rust/Cargo 1.100 (layout v2)")
    print("selector: effigy proof:foundation:rust-1-100:diagnostic (heavy)")
    print(f"command:  {COMMAND}")
    print(f"guard:    {GUARD} sha256 {sha256(GUARD)} (ordinary qualified default)")
    print(f"toolchains: pinned {PINNED} {PINNED_IDENTITY}; tested {TESTED} ({TESTED_RUSTC}, manifest {TESTED_MANIFEST_SHA256[:16]}...)")
    print(f"tested cargo must start {TESTED_CARGO_PREFIX!r} (pin in evidence README)")
    print("invocations (at most three TOTAL in ONE Queue occurrence; each phase has its own 300 s guard clock):")
    for key, chain, what in INVOCATIONS:
        print(f"  {key} {chain}: {what}")
    print("cases (key | deps | applies | body | origin):")
    for key, deps, applies, body, origin in CASES:
        print(f"  {key} | {','.join(deps) or '-'} | {applies} | {body} | {origin}")
    print("probes (phase 2, per participant):")
    for key, what, calls in PROBES:
        print(f"  {key}: {what} ({calls})")
    alloc = allocation()
    print(f"  total {alloc['probe_cargo_calls']} of 8 calls (rcgu {alloc['rcgu_calls']}), 90 s, 64 MiB;"
          " the version pair is the tested toolchain identity check")
    print("plain Cargo calls inside the focused invocations (guard-bounded, not probe calls):")
    for chain, what in IN_PLANE_CARGO:
        print(f"  {chain}: {what}")
    print("allocation per phase (ceilings, not measurements): staging target + copied oxide, private CARGO_HOME,")
    print("  directory pool (OXIDE_POOL_CAP 128 MiB), fixtures <= 8 MiB, probe <= 64 MiB, receipt <= 1 MiB;")
    print("  owned bytes checked after every case against 512 MiB; 20 GiB reserve sampled before and after every case")
    print("teardown: none in-script; the pool, slots and staging stay in the guard root for its post-leader census")
    print("fit: UNMEASURED")
    for fact in UNVERIFIED_FACTS:
        print(f"  premise: {fact}")
    print("premise checklist:")
    for item, disposition, rel, symbol in PREMISES:
        where = f" ({rel}:{symbol})" if rel else ""
        print(f"  {item}: {disposition}{where}")
    print("readiness: source-complete for review; the gate refuses until a planner binding and both toolchains exist")
    return 0


def main(argv: list) -> int:
    if argv == ["--plan"]:
        return plan()
    if argv == ["--self-check"]:
        return self_check()
    if argv and argv[0] == "--gate":
        return gate(argv)
    if len(argv) == 3 and argv[:2] == ["--diagnostic", "--toolchain"]:
        return diagnostic(argv[2])
    print("usage: foundation-rust-1-100.py --plan | --self-check | --gate --binding B --sha256 D | "
          "--diagnostic --toolchain T", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
