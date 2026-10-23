#!/usr/bin/env python3
"""Focused native proof for guarded accounting reads (oxide#141).

One fixed script and the whole command of the qualified shared supervisor
(`bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
scripts/test-accounting-guards.py`): 300 s wall clock including staging,
fixtures and cleanup, 512 MiB monitored scratch, 30 s shutdown margin, one
staged binary, one fixture command at a time, and one Cargo job per build.
`--plan` prints the exact command, cases, phases, disk allocation,
concurrency and limits without any effect; an unrecognized argument refuses
before any subprocess or staging; only the bare invocation is the counted
proof.

Staging uses the *invoking* Cargo home with `--offline` (its target and build
directories sit inside the monitored scratch and are deleted before the
fixtures). That is not a claim that staging never reads the invoking Cargo
home. Each fixture tree then has its own private `HOME` and `CARGO_HOME` and
always binds `RUSTUP_HOME` (explicit or derived from the invoking `HOME`
before it is replaced) plus `RUSTUP_TOOLCHAIN=1.97.1`, so fixture Cargo does
not depend on the calling shell.

Every fixture is a private scratch directory tree with its own `HOME`,
`CARGO_HOME`, `OXIDE_POOL` and a tiny std-only crate per project. It creates
its own pool on first use, never touches a live pool, `~/.local/bin/oxide`, a
mounted volume or host settings, and it never runs `status --measure`, a
live-pool inventory or a scan of another task's storage. The cases are
end-to-end CLI controls only:

- `union-hardlink-counted-once`: two idle whole slots share one hard-linked
  **4 MiB** file; a third project's admission must count that inode once,
  stay under the **6 MiB** budget and evict nothing.
- `union-separate-copies-press-byte-cap`: the identical layout with two
  independent 4 MiB files exceeds the same 6 MiB budget, so the same
  admission must evict exactly one idle whole slot. This makes the previous
  case non-vacuous: the budget and eviction path are live.
- `refusal-incomplete-accounting`: an unreadable directory inside an idle
  slot makes the whole-payload walk partial, so the same admission must
  refuse with **exactly exit 1** (the front door maps a leased-command
  failure to exit 1, not the retired standalone CLI's 3), naming
  `unknown_accounting` and `unreadable_entry`, and must leave the exact
  ledger bytes, every slot-root identity and the planted payload unchanged.

Tiny fixtures and deterministic CLI controls do not establish the rate or the
outcome of a large or cold real pool, do not measure the net effect of the
guarded reads, and are not a benchmark, a real-project qualification or an
alias for full QA. This helper cannot invent closure: the guard's own receipt
and the actual caller exit and session closure stay separately authoritative.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"
EXPECTED_RUNNER_SHA256 = "d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e"
MIB = 1024 * 1024
SHUTDOWN_MARGIN_S = 30
WALL_LIMIT_S = 300
SCRATCH_BUDGET_MIB = 512
# The cheap crate leaves a sub-megabyte build directory per slot, so a 6 MiB
# budget separates one 4 MiB inode counted once from two independent 4 MiB
# copies with slack on both sides.
POOL_CAP = "6MiB"
INODE_BYTES = 4 * MIB
FIXTURE_JOBS = 1
STAGE_JOBS = 1
# `src/bin/oxide/main.rs` returns exit 1 for any leased-command error, so an
# `AccountingIncomplete` refusal surfaces as exit 1 here; the retired
# standalone lease CLI's exit 3 is not this front door's public exit.
EXIT_REFUSAL = 1
MAX_RECEIPT_BYTES = 64 * 1024
SELECTOR = "rust:accounting-guards"
COMMAND = [
    "python3",
    "scripts/bounded-test.py",
    "--limit",
    str(WALL_LIMIT_S),
    "--scratch-budget-mib",
    str(SCRATCH_BUDGET_MIB),
    "--",
    "python3",
    "scripts/test-accounting-guards.py",
]
USAGE = """usage: test-accounting-guards.py [--plan | --self-check | --help]

Without an argument this runs the counted proof under the qualified shared
supervisor. --plan prints the fixed command, cases, phases, disk allocation,
concurrency and limits without any effect. --self-check runs pure argument,
plan, constant and identity checks in this process without staging or
touching a fixture.
"""
CASES: list[str] = []
RESULTS: list[dict] = []
PHASE_STARTED = time.perf_counter()


def fail(message: str) -> None:
    print(f"accounting-guards proof failed: {message}", file=sys.stderr)
    sys.exit(1)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def say(message: str) -> None:
    print(f"[{time.perf_counter() - PHASE_STARTED:7.1f}s] {message}", flush=True)


def case(name: str) -> None:
    CASES.append(name)
    say(f"case {name}")


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (
        deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    ) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the supervisor's shutdown margin")
    return left


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_bytes(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def private_dir(path: str) -> str:
    os.makedirs(path, exist_ok=True)
    os.chmod(path, 0o700)
    return path


def write_private(path: str, data: str) -> None:
    private_dir(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(data)
    os.chmod(path, 0o600)


def write_bytes(path: str, length: int) -> None:
    with open(path, "wb") as handle:
        block = b"x" * MIB
        remaining = length
        while remaining:
            piece = min(remaining, len(block))
            handle.write(block[:piece])
            remaining -= piece
    os.chmod(path, 0o600)


def read_text(path: str) -> str:
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except FileNotFoundError:
        return ""


# --------------------------------------------------------------------------
# Fixed plan, argument boundary and pure helpers. None of this has an effect.


def plan_document() -> dict:
    """The fixed command, cases, allocation, concurrency and limits."""
    return {
        "task": "oxide#141",
        "selector": SELECTOR,
        "command": COMMAND,
        "counted_only_without_argument": True,
        "cases": [
            {
                "name": "union-hardlink-counted-once",
                "oracle": "one 4 MiB inode hard-linked across two idle whole "
                "slots stays under the 6 MiB budget: exit 0, one new slot, "
                "both idle slot directories and their identities unchanged",
            },
            {
                "name": "union-separate-copies-press-byte-cap",
                "oracle": "two independent 4 MiB copies exceed the same budget: "
                "exit 0, exactly one idle whole slot evicted, one left",
            },
            {
                "name": "refusal-incomplete-accounting",
                "oracle": "an unreadable owned directory inside an idle slot: "
                "exact exit 1, unknown_accounting and unreadable_entry named, "
                "exact ledger bytes and every slot-root identity unchanged, "
                "planted payload unchanged",
            },
        ],
        "phases": [
            {"name": "stage", "allocation": "this HEAD's debug oxide, plain "
             "Cargo offline, target/build inside scratch, target deleted"},
            {"name": "union", "allocation": "one private fixture root, two idle "
             "slots, one 4 MiB inode"},
            {"name": "pressure", "allocation": "one private fixture root, two "
             "idle slots, two 4 MiB files"},
            {"name": "refusal", "allocation": "one private fixture root, two "
             "idle slots, one 4 KiB planted payload"},
        ],
        "disk": {
            "monitored_scratch_mib": SCRATCH_BUDGET_MIB,
            "fixture_roots": 3,
            "per_fixture": "two 4 MiB files plus tiny std-only crate "
            "intermediates (well under 20 MiB)",
            "staged_target": "deleted before any fixture",
        },
        "concurrency": {
            "staged_binaries": 1,
            "fixture_commands": "sequential, one at a time",
            "stage_cargo_jobs": STAGE_JOBS,
            "fixture_cargo_jobs": FIXTURE_JOBS,
        },
        "limits": {
            "wall_limit_s": WALL_LIMIT_S,
            "scratch_budget_mib": SCRATCH_BUDGET_MIB,
            "shutdown_margin_s": SHUTDOWN_MARGIN_S,
        },
        "provenance": {
            "staging": "invoking Cargo home, --offline; target and build inside "
            "the monitored scratch. This is not a claim that staging never "
            "reads the invoking Cargo home.",
            "fixtures": "private HOME and CARGO_HOME per fixture tree, "
            "RUSTUP_HOME explicit or derived from the invoking HOME, "
            "RUSTUP_TOOLCHAIN pinned to the builder",
        },
        "live_pool": False,
        "mounted_volume": False,
        "timing": "unmeasured; this plan claims no wall-clock figure",
    }


def parse_arguments(argv: list[str]) -> tuple[str, str | None]:
    """Fail-closed argument boundary: at most one known mode, no effects."""
    if not argv:
        return ("proof", None)
    if len(argv) > 1:
        return ("refuse", f"at most one argument is accepted, got {argv!r}")
    mode = {"--plan": "plan", "--self-check": "self-check", "--help": "help"}
    if argv[0] in mode:
        return (mode[argv[0]], None)
    return ("refuse", f"unrecognized argument {argv[0]!r}")


def identity(path: str) -> tuple[int, int, bool]:
    """A slot root's observed identity: device, inode and directory kind."""
    metadata = os.lstat(path)
    return (metadata.st_dev, metadata.st_ino, stat.S_ISDIR(metadata.st_mode))


def tree_oracle(path: str) -> dict:
    """A bounded digest of one tiny planted payload (no unknown recursion)."""
    entries = []
    for name in sorted(os.listdir(path)):
        entry = os.path.join(path, name)
        metadata = os.lstat(entry)
        if stat.S_ISREG(metadata.st_mode):
            entries.append(
                {
                    "name": name,
                    "kind": "file",
                    "size": metadata.st_size,
                    "sha256": sha256_file(entry),
                }
            )
        else:
            entries.append(
                {
                    "name": name,
                    "kind": "other",
                    "mode": stat.S_IMODE(metadata.st_mode),
                }
            )
    return {"mode": stat.S_IMODE(os.lstat(path).st_mode), "entries": entries}


def self_check() -> None:
    """Pure checks of the plan, the argument boundary and the oracles."""
    expect(parse_arguments([]) == ("proof", None), "the bare invocation must run the proof")
    expect(parse_arguments(["--plan"]) == ("plan", None), "--plan must be effect-free")
    expect(parse_arguments(["--self-check"]) == ("self-check", None), "--self-check must parse")
    expect(parse_arguments(["--help"]) == ("help", None), "--help must parse")
    for refused in (["--plan", "--plan"], ["proof"], ["--bogus"], [""]):
        mode, error = parse_arguments(refused)
        expect(mode == "refuse" and bool(error), f"{refused!r} must be refused")
    plan = plan_document()
    expect(plan["limits"]["wall_limit_s"] == 300, "the plan wall limit changed")
    expect(plan["limits"]["scratch_budget_mib"] == 512, "the plan scratch budget changed")
    expect(plan["limits"]["shutdown_margin_s"] == 30, "the plan margin changed")
    expect(len(plan["cases"]) == 3, "the plan case count changed")
    expect(plan["disk"]["fixture_roots"] == 3, "the plan fixture count changed")
    expect(
        plan["concurrency"]["fixture_commands"].startswith("sequential"),
        "the plan concurrency changed",
    )
    expect("unmeasured" in plan["timing"], "the plan must not claim measured timing")
    expect(EXIT_REFUSAL == 1, "the refusal exit oracle changed")
    expect(INODE_BYTES == 4 * MIB, "the inode size changed")
    expect(POOL_CAP == "6MiB", "the pool budget changed")
    expect(
        COMMAND[-1].endswith("scripts/test-accounting-guards.py")
        and "--limit" in COMMAND
        and "--scratch-budget-mib" in COMMAND,
        "the fixed command changed",
    )
    expect(sha256_file(os.path.abspath(__file__)) != "", "the script digest is unreadable")
    device, inode, is_directory = identity(os.path.abspath(__file__))
    expect(device > 0 and inode > 0, "the identity oracle returned no device or inode")
    expect(not is_directory, "the identity oracle mistook this file for a directory")
    print("self-check: ok (pure argument, plan, constant and identity checks; not evidence)")


# --------------------------------------------------------------------------
# The counted proof. Everything below has effects and requires the supervisor.


def stage_subject(root: str) -> str:
    """Stage this clean HEAD's debug `oxide` with plain Cargo, offline.

    The invoking Cargo home supplies the offline registry; the staged target
    and build directories stay inside the monitored scratch and are deleted
    once the binary is copied out.
    """
    stage = os.path.join(root, "stage")
    log = os.path.join(root, "stage.log")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            [
                "cargo",
                "build",
                "-j",
                str(STAGE_JOBS),
                "--locked",
                "--offline",
                "--bin",
                "oxide",
            ],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(root, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage)
    os.unlink(log)
    return subject


def invoking_rustup_home() -> str:
    """The invoking rustup home, derived before a fixture HOME replaces it."""
    explicit = os.environ.get("RUSTUP_HOME")
    if explicit:
        return explicit
    return os.path.join(os.path.expanduser("~"), ".rustup")


def fixture_env(root: str, pool: str) -> dict:
    """A minimal allowlist: no host Oxide, XDG or Cargo setting leaks in."""
    home = private_dir(os.path.join(root, "home"))
    env = {
        "HOME": home,
        "TMPDIR": root,
        "TMP": root,
        "TEMP": root,
        "OXIDE_POOL": pool,
        "OXIDE_POOL_CAP": POOL_CAP,
        "CARGO_HOME": private_dir(os.path.join(root, "cargo-home")),
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "NO_COLOR": "1",
        "CARGO_TERM_COLOR": "never",
        "CARGO_NET_OFFLINE": "true",
        # Fixture Cargo must resolve the pinned toolchain without depending on
        # the calling shell: HOME is replaced above, so both values are
        # explicit.
        "RUSTUP_HOME": invoking_rustup_home(),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
    }
    for name in (
        "LANG",
        "LC_ALL",
        "TZ",
        "OXIDE_TASK_SCRATCH_ROOT",
        "OXIDE_PROOF_DEADLINE_NS",
        "OXIDE_PROOF_EVIDENCE",
    ):
        if name in os.environ:
            env[name] = os.environ[name]
    return env


def create_project(root: str, label: str) -> str:
    path = private_dir(os.path.join(root, "projects", label))
    write_private(
        os.path.join(path, "Cargo.toml"),
        f'[package]\nname = "oxide-accounting-guards-{label}"\n'
        'version = "0.1.0"\nedition = "2021"\n',
    )
    write_private(os.path.join(path, "src", "lib.rs"), "pub fn fixture() {}\n")
    return path


def check(subject: str, workspace: str, env: dict, label: str) -> tuple[int, str]:
    work_left()
    result = subprocess.run(
        [subject, "check", "--offline", "-j", str(FIXTURE_JOBS)],
        cwd=workspace,
        env=env,
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    output = f"{result.stdout}{result.stderr}"
    say(f"{label}: exit {result.returncode}, {len(output)} bytes of output")
    return result.returncode, output


def ledger(pool: str) -> dict:
    return json.loads(read_text(os.path.join(pool, "ledger.json")))


def ledger_path(pool: str) -> str:
    return os.path.join(pool, "ledger.json")


def slot_directory(pool: str, slot: dict) -> str:
    slots_root = os.path.join(pool, "payload", "slots")
    if "dir" in slot:
        return os.path.join(slots_root, slot["dir"])
    return os.path.join(slots_root, str(slot["id"]))


def seed_two_idle_slots(root: str, subject: str) -> tuple[str, dict, list[str]]:
    """Two idle whole slots of two other projects, each released and idle."""
    pool = os.path.join(root, "pool")
    env = fixture_env(root, pool)
    for label in ("a", "b"):
        workspace = create_project(root, label)
        code, output = check(subject, workspace, env, f"seed-{label}")
        expect(code == 0, f"seeding slot {label} exited {code}: {output[-3000:]}")
    document = ledger(pool)
    slots = document["slots"]
    expect(len(slots) == 2, f"expected two seeded slots, found {len(slots)}")
    expect(
        all(slot["state"] == "idle" and not slot.get("lease_id") for slot in slots),
        f"a seeded slot is not idle: {json.dumps(slots)[:800]}",
    )
    return pool, env, [slot_directory(pool, slot) for slot in slots]


def prove_union(root: str, subject: str) -> None:
    case("union-hardlink-counted-once")
    pool, env, slots = seed_two_idle_slots(root, subject)
    expect(len(slots) == 2, f"expected two idle slot directories, found {slots}")
    before_identities = [identity(path) for path in slots]
    shared = os.path.join(slots[0], "build", "shared.bin")
    write_bytes(shared, INODE_BYTES)
    os.link(shared, os.path.join(slots[1], "build", "shared.bin"))
    before = len(ledger(pool)["slots"])
    workspace = create_project(root, "c")
    code, output = check(subject, workspace, env, "admit-with-hardlink")
    expect(code == 0, f"admission with a hard-linked pair exited {code}: {output[-3000:]}")
    expect("cannot lease a slot" not in output, f"admission refused: {output[-3000:]}")
    after = ledger(pool)["slots"]
    expect(
        len(after) == before + 1,
        "the hard-linked inode was counted twice, so an idle whole slot was evicted",
    )
    expect(
        [identity(path) for path in slots] == before_identities,
        "an idle slot root identity changed although the union was under budget",
    )
    RESULTS.append(
        {
            "case": "union-hardlink-counted-once",
            "exit": code,
            "slots": len(after),
            "idle_root_identities_unchanged": True,
        }
    )


def prove_separate_copies(root: str, subject: str) -> None:
    case("union-separate-copies-press-byte-cap")
    pool, env, slots = seed_two_idle_slots(root, subject)
    write_bytes(os.path.join(slots[0], "build", "copy-a.bin"), INODE_BYTES)
    write_bytes(os.path.join(slots[1], "build", "copy-b.bin"), INODE_BYTES)
    workspace = create_project(root, "c")
    code, output = check(subject, workspace, env, "admit-under-byte-pressure")
    expect(code == 0, f"admission under byte pressure exited {code}: {output[-3000:]}")
    after = ledger(pool)["slots"]
    expect(
        len(after) == 2,
        f"byte pressure did not evict exactly one idle whole slot: {len(after)} slots left",
    )
    survivors = [path for path in slots if os.path.isdir(path)]
    expect(
        len(survivors) == 1,
        f"exactly one idle slot directory must remain, found {survivors}",
    )
    RESULTS.append(
        {
            "case": "union-separate-copies-press-byte-cap",
            "exit": code,
            "slots": len(after),
            "idle_slot_directories_remaining": len(survivors),
        }
    )


def prove_refusal(root: str, subject: str) -> None:
    case("refusal-incomplete-accounting")
    pool, env, slots = seed_two_idle_slots(root, subject)
    blocked = os.path.join(slots[0], "build", "blocked")
    private_dir(blocked)
    planted = os.path.join(blocked, "payload.bin")
    write_bytes(planted, 4096)
    # Capture every oracle before the directory is made unreadable.
    before_ledger = read_bytes(ledger_path(pool))
    before_identities = [identity(path) for path in slots]
    before_planted = tree_oracle(blocked)
    before_planted_identity = identity(planted)
    restored = False
    try:
        os.chmod(blocked, 0o000)
        workspace = create_project(root, "c")
        code, output = check(subject, workspace, env, "admit-with-unreadable-slot")
        expect(
            code == EXIT_REFUSAL,
            f"an incomplete whole-payload walk must refuse with exit "
            f"{EXIT_REFUSAL}, got {code}: {output[-3000:]}",
        )
        expect(
            "unknown_accounting" in output,
            f"the refusal did not name unknown_accounting: {output[-3000:]}",
        )
        expect(
            "unreadable_entry" in output,
            f"the refusal did not name unreadable_entry: {output[-3000:]}",
        )
        expect(
            read_bytes(ledger_path(pool)) == before_ledger,
            "the incomplete-accounting refusal rewrote the ledger bytes",
        )
        expect(
            [identity(path) for path in slots] == before_identities,
            "the incomplete-accounting refusal changed a slot-root identity",
        )
        expect(
            all(os.path.isdir(path) for path in slots),
            "the incomplete-accounting refusal changed a slot directory",
        )
        RESULTS.append(
            {
                "case": "refusal-incomplete-accounting",
                "exit": code,
                "reason": "unknown_accounting + unreadable_entry",
                "ledger_bytes_unchanged": True,
                "slot_root_identities_unchanged": True,
            }
        )
    finally:
        # Honest custody: a failed restoration fails the run, never a pass.
        os.chmod(blocked, 0o700)
        restored = True
    expect(restored, "the deliberately unreadable directory was not restored")
    expect(
        tree_oracle(blocked) == before_planted,
        "the planted payload did not survive the refusal unchanged",
    )
    expect(
        identity(planted) == before_planted_identity,
        "the planted payload identity changed across the refusal",
    )


def write_receipt(receipt: dict, scratch: str) -> str:
    """Write this helper's uniquely named, bounded receipt beside the guard's.

    The guard's own `receipt.json`, the actual caller exit and the session
    closure stay separately authoritative; this document only records the
    script's own case oracles.
    """
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    expect(
        bool(evidence),
        "OXIDE_PROOF_EVIDENCE is not set; a supervised proof writes its "
        "receipt into the supervisor's evidence directory",
    )
    evidence = os.path.abspath(evidence)
    expect(os.path.isdir(evidence), f"evidence directory {evidence} does not exist")
    parent = os.path.dirname(evidence)
    expect(
        evidence.startswith(scratch)
        or (
            os.path.basename(evidence) == "evidence"
            and os.path.basename(scratch) == "scratch"
            and os.path.dirname(scratch) == parent
        ),
        f"evidence directory {evidence} is not the supervisor's",
    )
    payload = json.dumps(receipt, indent=2, sort_keys=True).encode()
    expect(
        len(payload) <= MAX_RECEIPT_BYTES,
        f"the receipt is {len(payload)} bytes, over its {MAX_RECEIPT_BYTES} cap",
    )
    stamp = f"{int(time.time() * 1000)}-{os.getpid()}"
    path = os.path.join(evidence, f"accounting-guards-receipt.{stamp}.json")
    expect(not os.path.exists(path), f"receipt path {path} already exists")
    temporary = f"{path}.tmp"
    with open(temporary, "wb") as handle:
        handle.write(payload)
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    return path


def run_proof(scratch: str, commit: str, tree: str, runner_sha: str) -> None:
    subject = stage_subject(scratch)
    digest = sha256_file(subject)
    say(f"subject sha256 {digest}")
    expect(
        len(digest) == 64 and all(character in "0123456789abcdef" for character in digest),
        "the staged subject digest is malformed",
    )
    for name in ("union", "pressure", "refusal"):
        root = private_dir(os.path.join(scratch, f"fixture-{name}"))
        if name == "union":
            prove_union(root, subject)
        elif name == "pressure":
            prove_separate_copies(root, subject)
        else:
            prove_refusal(root, subject)
        work_left()
    say(f"cases run: {', '.join(CASES)}")
    receipt = {
        "task": "oxide#141",
        "kind": "accounting-guards helper receipt",
        "selector": SELECTOR,
        "source_commit": commit,
        "source_tree": tree,
        "binary_sha256": digest,
        "script_sha256": sha256_file(os.path.abspath(__file__)),
        "guard_sha256": runner_sha,
        "command": COMMAND,
        "cases": RESULTS,
        "limits": {
            "wall_limit_s": WALL_LIMIT_S,
            "scratch_budget_mib": SCRATCH_BUDGET_MIB,
            "shutdown_margin_s": SHUTDOWN_MARGIN_S,
        },
        "concurrency": {
            "staged_binaries": 1,
            "fixture_commands": "sequential",
            "stage_cargo_jobs": STAGE_JOBS,
            "fixture_cargo_jobs": FIXTURE_JOBS,
        },
        "authority": "This helper receipt records only the script's own case "
        "oracles. The guard's own receipt, the actual caller exit and the "
        "session closure remain separately authoritative and are not "
        "determined here.",
        "live_pool": False,
        "result": "pass",
    }
    path = write_receipt(receipt, scratch)
    say(f"helper receipt {path}")
    print(json.dumps(receipt, indent=2, sort_keys=True), flush=True)


def main(argv: list[str]) -> int:
    mode, error = parse_arguments(argv)
    if mode == "refuse":
        print(f"accounting-guards: {error}", file=sys.stderr)
        print(USAGE, file=sys.stderr)
        return 2
    if mode == "help":
        print(USAGE, end="")
        return 0
    if mode == "plan":
        print(json.dumps(plan_document(), indent=2, sort_keys=True))
        return 0
    if mode == "self-check":
        self_check()
        return 0
    runner = os.path.join(REPO, "scripts", "bounded-test.py")
    runner_sha = sha256_file(runner)
    expect(
        runner_sha == EXPECTED_RUNNER_SHA256,
        f"supervisor {runner_sha} is not the qualified {EXPECTED_RUNNER_SHA256}",
    )
    dirty = git("status", "--porcelain")
    expect(
        dirty == "",
        f"the worktree is not clean, so the staged source would not be HEAD: {dirty[:400]}",
    )
    commit = git("rev-parse", "HEAD")
    tree = git("rev-parse", "HEAD^{tree}")
    branch = git("branch", "--show-current")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or fail(
        "OXIDE_TASK_SCRATCH_ROOT is not set"
    )
    scratch = os.path.abspath(scratch)
    expect(
        scratch.startswith("/private/tmp/") or scratch.startswith("/tmp/"),
        f"scratch root {scratch} is not a private temporary directory",
    )
    expect(os.path.isdir(scratch), f"scratch root {scratch} is not a directory")
    print(
        f"host: project={git('remote', 'get-url', 'origin')} commit={commit} "
        f"branch={branch} cpu={os.cpu_count()} load={os.getloadavg()}"
    )
    run_proof(scratch, commit, tree, runner_sha)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
