#!/usr/bin/env python3
"""Focused native proof for per-deletion custody (oxide#181, design oxide#180
slice 1).

One fixed script and the whole command of the qualified shared supervisor
(`bounded-test.py --root-parent /private/tmp --limit 300
--scratch-budget-mib 512 -- python3 scripts/test-deletion-custody.py`): 300 s
wall clock including two stagings, fixtures and cleanup, 512 MiB monitored
scratch, 30 s shutdown margin. The explicit internal root is the owner-accepted
internal-placement exception (decision bd357a4c): every fixture pool is a
genuine internal `directory-budget` pool, which the product correctly refuses
on an external volume. `--plan` prints the fixed command, cases, phases, disk
allocation, concurrency and limits without any effect; an unrecognized
argument refuses before any subprocess or staging; only the bare invocation is
the counted proof. `--self-check` is pure (`proof:deletion-custody:test`);
`--diagnostic` is the uncounted keep-going collection.

Slice 1 still deletes under the pool lock: there is no out-of-lock payload
effect and no worker thread. The cases prove the foundations that let slice 2
move deletion out of the lock:

- `old-binary-refuses-fenced-pool`: three probes with the pinned merge-base
  binary (staged from `git archive`, never the working tree), each asserting
  byte-identical control/ledger and unchanged roots (and, for the pending
  object, unchanged custody bytes and payload identity): the **intermediate**
  crash-between-fences state (control version 9, ledger still the predecessor's
  policy 3), the **final** state (version 9 + policy 5), and a **ledger-only**
  probe whose control is rewritten to the predecessor's own version 8 so only
  its ledger parser can refuse a policy-5 ledger carrying `custody`. This
  binary then resumes the intermediate state: its own leasing command
  legitimately rewrites its record state (last use, next use, origin), so the
  oracle requires the version-9 fences to be retained and the recorded slot set
  and every slot root to be unchanged (never byte identity), and the ledger to
  publish policy 5. The exported base tree is asserted not to carry either
  fence.
- `custody-and-journal-at-pause`: a real byte-pressure eviction pauses at the
  durable root plan (debug-only seam inside the supervisor layout) and the
  proof reads the private custody directory, its `0600` lock and journal, the
  policy-5 fence and the absent in-ledger journal while the payload is still
  present; releasing the gate completes the deletion.
- `killed-mid-deletion-then-replay`: the same pause, then `SIGKILL` of that one
  fixture process. The record, its charge, its custody and its payload stay;
  the next command claims custody exclusively, replays, deletes the whole slot
  and drops only its matching record.
- `busy-custody-is-skipped`: with the killed state's custody lock held by the
  proof process, a concurrent command completes by warm reuse, never waiting
  for custody, while the pending record and payload are untouched; releasing
  the lock lets a following command finish the deletion.

Tiny fixtures and deterministic CLI controls do not establish the rate or the
outcome of a large or cold real pool and are not a benchmark, a removable
volume, a real-project qualification or an alias for full QA.
"""

from __future__ import annotations

import fcntl
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
EXPECTED_RUNNER_SHA256 = "bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924"
MIB = 1024 * 1024
SHUTDOWN_MARGIN_S = 30
WALL_LIMIT_S = 300
SCRATCH_BUDGET_MIB = 512
PLANT_BYTES = 5 * MIB
POOL_CAP = "10MiB"
# The staged build's one frozen rustc flag. CARGO_ENCODED_RUSTFLAGS (which
# Cargo prefers over RUSTFLAGS) and every cargo/rustc override are scrubbed
# from the inherited parent, so a hostile shell cannot change the staged
# binary.
FROZEN_RUSTFLAGS = "-C strip=symbols"
STAGE_JOBS = 1
FIXTURE_JOBS = 1
PAUSE_POINT = "journal-planned"
# The predecessor's own control-state version. The ledger-only probe rewrites
# the fixture control to this version so only the predecessor's ledger parser
# can refuse the final ledger.
PREDECESSOR_CONTROL_VERSION = 8
MAX_RECEIPT_BYTES = 64 * 1024
SELECTOR = "rust:deletion-custody"
COMMAND = [
    "python3",
    "scripts/bounded-test.py",
    "--root-parent",
    "/private/tmp",
    "--limit",
    str(WALL_LIMIT_S),
    "--scratch-budget-mib",
    str(SCRATCH_BUDGET_MIB),
    "--",
    "python3",
    "scripts/test-deletion-custody.py",
]
USAGE = """usage: test-deletion-custody.py [--plan | --self-check | --diagnostic | --help]

Without an argument this runs the counted proof under the qualified shared
supervisor. --plan prints the fixed command, cases, phases, disk allocation,
concurrency and limits without any effect. --self-check runs pure argument,
plan, constant and oracle checks in this process without staging or touching a
fixture. --diagnostic is the uncounted keep-going collection; it writes one
bounded `qualified: false` receipt and is never a counted PASS.
"""

CASES: list[str] = []
RESULTS: list[dict] = []
FAILURES: list[dict] = []
STARTED = time.perf_counter()
DIAGNOSTIC = False


def fail(message: str) -> None:
    print(f"deletion-custody proof failed: {message}", file=sys.stderr)
    sys.exit(1)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def say(message: str) -> None:
    print(f"[{time.perf_counter() - STARTED:7.1f}s] {message}", flush=True)


def exercise(name: str, body) -> None:
    """Run one independent case; in diagnostic mode record and keep going."""
    CASES.append(name)
    say(f"case {name}")
    if not DIAGNOSTIC:
        body()
        return
    try:
        body()
        RESULTS.append({"case": name, "outcome": "completed"})
    except SystemExit as error:  # `fail()` inside a case
        FAILURES.append({"case": name, "outcome": "failed", "detail": str(error)})
        say(f"case {name}: failed (uncounted diagnostic continues)")


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (
        deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    ) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        abort("work deadline reached before the supervisor's shutdown margin")
    return left


def abort(message: str) -> None:
    """An unknown outcome or exhausted deadline stops the diagnostic."""
    if DIAGNOSTIC:
        write_receipt(
            {
                "qualified": False,
                "aborted": message,
                "cases": RESULTS,
                "failures": FAILURES,
            }
        )
    fail(message)


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_bytes(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def read_json(path: str):
    try:
        return json.loads(read_bytes(path))
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def read_text(path: str) -> str:
    return read_bytes(path).decode("utf-8", errors="replace")


def write_private_atomic(path: str, payload: bytes) -> None:
    """Replace one owner-only file atomically (temp, fsync, rename, dir sync)."""
    # The pool root tolerates only `ledger.tmp-*` temp names, so the atomic
    # replace never leaves a foreign entry visible (the same prefix works for
    # the control document inside `oxide-managed`).
    temporary = os.path.join(
        os.path.dirname(path), f"ledger.tmp-{os.getpid()}-proof"
    )
    with open(temporary, "wb") as handle:
        handle.write(payload)
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    directory = os.open(os.path.dirname(path), os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def tree_digest(directory: str) -> dict:
    """Mode and content digest of one custody directory (bounded, no recursion)."""
    digest = {"mode": stat.S_IMODE(os.lstat(directory).st_mode)}
    for name in sorted(os.listdir(directory)):
        entry = os.path.join(directory, name)
        metadata = os.lstat(entry)
        digest[name] = (
            stat.S_IMODE(metadata.st_mode),
            sha256_file(entry) if stat.S_ISREG(metadata.st_mode) else None,
        )
    return digest


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


# --------------------------------------------------------------------------
# Fixed plan, argument boundary and pure oracles. None of this has an effect.


def plan_document() -> dict:
    return {
        "task": "oxide#181",
        "design": "oxide#180 slice 1 (decision 5f9744d0)",
        "selector": SELECTOR,
        "command": COMMAND,
        "counted_only_without_argument": True,
        "out_of_lock_effect": False,
        "cases": [
            {
                "name": "old-binary-refuses-fenced-pool",
                "oracle": "the merge-base binary exits nonzero on the "
                "intermediate (control v9, policy-3 ledger), final (control v9, "
                "policy-5 ledger) and ledger-only (control v8, policy-5 ledger "
                "with custody) states before any effect, with control/ledger "
                "bytes, custody bytes and slot roots unchanged; this binary "
                "resumes the intermediate state with the version-9 fences "
                "retained, policy 5 published and no slot or payload deleted",
            },
            {
                "name": "custody-and-journal-at-pause",
                "oracle": "at the durable root plan: a private custody "
                "directory, a 0600 lock and a 0600 journal naming the same "
                "nonce, the policy-5 ledger fence, no in-ledger journal and "
                "an intact payload; releasing the gate deletes the slot",
            },
            {
                "name": "killed-mid-deletion-then-replay",
                "oracle": "SIGKILL leaves the record, custody, journal and "
                "payload; the next command claims custody, deletes the slot "
                "and drops only the matching record",
            },
            {
                "name": "busy-custody-is-skipped",
                "oracle": "with the custody lock held by the proof process a "
                "concurrent command completes by warm reuse, keeps the "
                "pending record and payload; releasing the lock lets a later "
                "command finish it",
            },
        ],
        "phases": [
            {"name": "stage-candidate", "allocation": "this HEAD's debug oxide, "
             "plain Cargo offline, target/build inside scratch, deleted after"},
            {"name": "stage-base", "allocation": "the pinned merge-base tree "
             "from git archive into scratch, its own target/build, deleted after"},
            {"name": "old-binary", "allocation": "one private fixture root"},
            {"name": "pause", "allocation": "one private fixture root, two "
             "idle slots, one 5 MiB planted file"},
            {"name": "kill-replay", "allocation": "one private fixture root"},
            {"name": "busy-skip", "allocation": "one private fixture root"},
        ],
        "disk": {
            "monitored_scratch_mib": SCRATCH_BUDGET_MIB,
            "fixture_roots": 4,
            "per_fixture": "one 5 MiB planted file plus tiny std-only crate "
            "intermediates (well under 20 MiB)",
            "staged_targets": "deleted before any fixture",
        },
        "concurrency": {
            "staged_binaries": 2,
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
            "the monitored scratch. Not a claim that staging never reads the "
            "invoking Cargo home. RUSTFLAGS is frozen and "
            "CARGO_ENCODED_RUSTFLAGS plus every cargo/rustc override is "
            "scrubbed from the parent (pure hostile-parent control in "
            "--self-check).",
            "base": "git archive of git merge-base HEAD origin/main into owned "
            "scratch; the working tree is never exported or mutated",
            "fixtures": "private HOME and CARGO_HOME per fixture tree, "
            "RUSTUP_HOME explicit or derived from the invoking HOME, "
            "RUSTUP_TOOLCHAIN pinned to the builder",
        },
        "live_pool": False,
        "mounted_volume": False,
        "timing": "unmeasured; this plan claims no wall-clock figure",
    }


def parse_arguments(argv: list[str]) -> tuple[str, str | None]:
    if not argv:
        return ("proof", None)
    if len(argv) > 1:
        return ("refuse", f"at most one argument is accepted, got {argv!r}")
    mode = {
        "--plan": "plan",
        "--self-check": "self-check",
        "--diagnostic": "diagnostic",
        "--help": "help",
    }
    if argv[0] in mode:
        return (mode[argv[0]], None)
    return ("refuse", f"unrecognized argument {argv[0]!r}")


def identity(path: str) -> tuple[int, int, bool]:
    metadata = os.lstat(path)
    return (metadata.st_dev, metadata.st_ino, stat.S_ISDIR(metadata.st_mode))


CONTROL_FENCE_FIELDS = {
    "recorded_accounting": "oxide.accounting/1",
    "eviction_accounting": "oxide.eviction-accounting/1",
    "slot_ceiling": "oxide.slots/256",
    "deletion_custody": "oxide.deletion-custody/1",
}


def fence_state_verdict(payload: bytes, version: int = 9) -> tuple[bool, str]:
    """Whether a control document still carries the current forward fences.

    Pure: parses bytes only. It fails on any version rollback and on a missing,
    unknown or non-string fence, so the corrected resume oracle cannot silently
    accept a fence regression.
    """
    try:
        document = json.loads(payload)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        return (False, f"the control state is not JSON ({error})")
    if not isinstance(document, dict):
        return (False, "the control state is not a JSON object")
    if document.get("version") != version:
        return (
            False,
            f"the control version is {document.get('version')!r}, not {version}",
        )
    for name, value in CONTROL_FENCE_FIELDS.items():
        if document.get(name) != value:
            return (False, f"the {name} fence is {document.get(name)!r}, not {value!r}")
    return (True, "version-9 control state with every forward fence retained")


def resume_oracle(
    state_payload: bytes,
    roots_before: list[tuple[int, int, bool]],
    roots_after: list[tuple[int, int, bool]],
    slots_before: int,
    slots_after: int,
) -> tuple[bool, str]:
    """The fence-resume facts: no rollback and no slot or payload deletion.

    A candidate leasing command legitimately rewrites its own record state
    (last use, next use, compile origin), so byte identity is not the oracle.
    What must hold is that the control document keeps the version-9 fences and
    that the recorded slot set and every slot root are unchanged and still
    directories. Pure: compares the given bytes and identities only.
    """
    ok, reason = fence_state_verdict(state_payload)
    if not ok:
        return (False, reason)
    if slots_after != slots_before:
        return (
            False,
            f"the resume changed the ledger slot count {slots_before} -> {slots_after}",
        )
    if len(roots_before) != len(roots_after):
        return (False, "the resume changed the recorded slot root set")
    for before, after in zip(roots_before, roots_after):
        if before != after:
            return (False, "the resume changed a slot root identity")
        if not after[2]:
            return (False, "the resume removed a slot root")
    return (True, "no fence rollback and no slot or payload deleted")


def custody_entries(pool: str) -> list[str]:
    root = os.path.join(pool, "oxide-managed", "deletions")
    try:
        return sorted(os.listdir(root))
    except FileNotFoundError:
        return []


def custody_dir(pool: str, nonce: str) -> str:
    return os.path.join(pool, "oxide-managed", "deletions", nonce)


def recorded_custody(pool: str, slot_id: int):
    document = read_json(os.path.join(pool, "ledger.json")) or {}
    for slot in document.get("slots", []):
        if slot.get("id") == slot_id:
            return slot.get("custody")
    return None


def self_check() -> None:
    expect(parse_arguments([]) == ("proof", None), "the bare invocation must run the proof")
    expect(parse_arguments(["--plan"]) == ("plan", None), "--plan must be effect-free")
    expect(parse_arguments(["--self-check"]) == ("self-check", None), "--self-check must parse")
    expect(parse_arguments(["--diagnostic"]) == ("diagnostic", None), "--diagnostic must parse")
    expect(parse_arguments(["--help"]) == ("help", None), "--help must parse")
    for refused in (["--plan", "--plan"], ["proof"], ["--bogus"], [""]):
        mode, error = parse_arguments(refused)
        expect(mode == "refuse" and bool(error), f"{refused!r} must be refused")
    plan = plan_document()
    expect(plan["limits"]["wall_limit_s"] == 300, "the plan wall limit changed")
    expect(plan["limits"]["scratch_budget_mib"] == 512, "the plan scratch budget changed")
    expect(plan["limits"]["shutdown_margin_s"] == 30, "the plan margin changed")
    expect(len(plan["cases"]) == 4, "the plan case count changed")
    expect(plan["disk"]["fixture_roots"] == 4, "the plan fixture count changed")
    expect(plan["out_of_lock_effect"] is False, "slice 1 has an out-of-lock effect")
    expect(
        plan["concurrency"]["fixture_commands"].startswith("sequential"),
        "the plan concurrency changed",
    )
    expect("unmeasured" in plan["timing"], "the plan must not claim measured timing")
    expect(
        COMMAND[:2] == ["python3", "scripts/bounded-test.py"]
        and "--root-parent" in COMMAND
        and "/private/tmp" in COMMAND
        and COMMAND[-1].endswith("scripts/test-deletion-custody.py"),
        "the fixed command changed",
    )
    expect(PLANT_BYTES == 5 * MIB and POOL_CAP == "10MiB", "the fixture sizes changed")
    expect(PAUSE_POINT == "journal-planned", "the pause point changed")
    expect(
        PREDECESSOR_CONTROL_VERSION == 8,
        "the predecessor control-version probe constant changed",
    )
    # The custody layout oracle: a nonce is 32 lowercase hex characters and the
    # two files are the ones this proof reads.
    nonce = "0123456789abcdef0123456789abcdef"
    expect(
        len(nonce) == 32
        and all(character in "0123456789abcdef" for character in nonce),
        "the nonce oracle changed",
    )
    # The fence-resume oracle must fail on a rollback or a deletion and accept
    # only the retained version-9 fences. This pure control uses no fixture.
    control_document = {"version": 9, **CONTROL_FENCE_FIELDS}
    control_bytes = json.dumps(control_document).encode()
    expect(
        fence_state_verdict(control_bytes)[0],
        "the fence oracle rejected a version-9 control document",
    )
    rollback = dict(control_document)
    rollback["version"] = PREDECESSOR_CONTROL_VERSION
    expect(
        not fence_state_verdict(json.dumps(rollback).encode())[0],
        "the fence oracle accepted a version rollback",
    )
    missing = dict(control_document)
    missing.pop("deletion_custody")
    expect(
        not fence_state_verdict(json.dumps(missing).encode())[0],
        "the fence oracle accepted a document without the custody fence",
    )
    wrong = dict(control_document)
    wrong["deletion_custody"] = "oxide.deletion-custody/2"
    expect(
        not fence_state_verdict(json.dumps(wrong).encode())[0],
        "the fence oracle accepted an unknown custody fence",
    )
    roots = [(16777229, 1234, True)]
    expect(
        resume_oracle(control_bytes, roots, roots, 1, 1)[0],
        "the resume oracle rejected an unchanged fenced state",
    )
    expect(
        not resume_oracle(json.dumps(rollback).encode(), roots, roots, 1, 1)[0],
        "the resume oracle accepted a fence rollback",
    )
    expect(
        not resume_oracle(control_bytes, roots, [(16777229, 1235, True)], 1, 1)[0],
        "the resume oracle accepted a replaced slot root",
    )
    expect(
        not resume_oracle(control_bytes, roots, [(16777229, 1234, False)], 1, 1)[0],
        "the resume oracle accepted a deleted slot root",
    )
    expect(
        not resume_oracle(control_bytes, roots, roots, 1, 2)[0],
        "the resume oracle accepted a changed slot count",
    )
    expect(FROZEN_RUSTFLAGS == "-C strip=symbols", "the frozen rustc flag changed")
    # Hostile-parent control: neither the staged build nor a fixture child may
    # inherit a rustc/cargo override, an OXIDE hook or a foreign target dir.
    hostile = {
        "PATH": "/usr/bin:/bin",
        "HOME": "/hostile/home",
        "RUSTUP_HOME": "/hostile/rustup",
        "CARGO_ENCODED_RUSTFLAGS": "--cfg hostile-encoded",
        "RUSTFLAGS": "--cfg hostile",
        "CARGO_BUILD_RUSTFLAGS": "--cfg hostile-build",
        "CARGO_TARGET_DIR": "/hostile/target",
        "CARGO_BUILD_BUILD_DIR": "/hostile/build",
        "RUSTC_WRAPPER": "/hostile/wrapper",
        "RUSTC_WORKSPACE_WRAPPER": "/hostile/workspace-wrapper",
        "CARGO": "/hostile/cargo",
        "RUSTC": "/hostile/rustc",
        "SCCACHE_DIR": "/hostile/sccache",
        "OXIDE_POOL": "/hostile/pool",
        "OXIDE_PROOF_EVIDENCE": "/hostile/evidence",
    }
    staged = staging_env(hostile, "/stage")
    expect(
        "CARGO_ENCODED_RUSTFLAGS" not in staged,
        "staging inherited CARGO_ENCODED_RUSTFLAGS",
    )
    expect(staged["RUSTFLAGS"] == FROZEN_RUSTFLAGS, "staging did not freeze RUSTFLAGS")
    expect(
        "CARGO_BUILD_RUSTFLAGS" not in staged,
        "staging inherited CARGO_BUILD_RUSTFLAGS",
    )
    expect(staged["CARGO_TARGET_DIR"] == "/stage/target", "the staging target dir leaked")
    expect(staged["CARGO_BUILD_BUILD_DIR"] == "/stage/build", "the staging build dir leaked")
    expect(
        staged["RUSTC_WRAPPER"] == ""
        and "CARGO" not in staged
        and "RUSTC" not in staged
        and "SCCACHE_DIR" not in staged
        and "OXIDE_POOL" not in staged,
        "staging inherited a compiler, wrapper or hook override",
    )
    fixture = fixture_environment(hostile, "/fixture", "/fixture/pool")
    expect(
        fixture["HOME"] == "/fixture/home"
        and fixture["CARGO_HOME"] == "/fixture/cargo-home",
        "the fixture env did not isolate HOME and CARGO_HOME",
    )
    expect(
        fixture["OXIDE_POOL"] == "/fixture/pool" and fixture["PATH"] == "/usr/bin:/bin",
        "the fixture env lost its own pool or PATH",
    )
    expect(
        fixture["RUSTUP_HOME"] == "/hostile/rustup"
        and fixture["RUSTUP_TOOLCHAIN"] == BUILDER,
        "the fixture env did not pin the toolchain",
    )
    for name in (
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "CARGO",
        "RUSTC",
        "SCCACHE_DIR",
    ):
        expect(name not in fixture, f"the fixture env inherited {name}")
    expect(sha256_file(os.path.abspath(__file__)) != "", "the script digest is unreadable")
    print("self-check: ok (pure argument, plan, constant and oracle checks; not evidence)")


# --------------------------------------------------------------------------
# The proof. Everything below has effects and requires the supervisor.


STAGE_SCRUBBED = (
    "CARGO",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
)
STAGE_SCRUBBED_PREFIXES = ("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_")
FIXTURE_INHERITED = (
    "LANG",
    "LC_ALL",
    "TZ",
    "OXIDE_TASK_SCRATCH_ROOT",
    "OXIDE_PROOF_DEADLINE_NS",
    "OXIDE_PROOF_EVIDENCE",
)


def staging_env(parent: dict, stage_root: str) -> dict:
    """The staged build's environment, sanitized against a hostile parent.

    A parent CARGO_ENCODED_RUSTFLAGS (which Cargo prefers over RUSTFLAGS by
    definition), RUSTFLAGS, CARGO_BUILD_RUSTFLAGS, target/build overrides, a
    compiler or wrapper override, an OXIDE_* hook or a CARGO/RUSTC override
    must never reach the staged build: the premise is that the staged binary is
    exactly this clean HEAD's debug oxide. The one rustc flag is frozen here,
    never inherited.
    """
    env = {
        key: value
        for key, value in parent.items()
        if not key.startswith(STAGE_SCRUBBED_PREFIXES) and key not in STAGE_SCRUBBED
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage_root, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage_root, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": FROZEN_RUSTFLAGS,
        }
    )
    return env


def fixture_environment(parent: dict, root: str, pool: str) -> dict:
    """The fixture child's allowlisted environment (no filesystem effect).

    A fixture child inherits neither rustc/cargo override: the allowlist below
    is the whole environment, plus the supervisor's own facts.
    """
    env = {
        "HOME": os.path.join(root, "home"),
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "TMPDIR": root,
        "TMP": root,
        "TEMP": root,
        "OXIDE_POOL": pool,
        "OXIDE_POOL_CAP": POOL_CAP,
        "CARGO_HOME": os.path.join(root, "cargo-home"),
        "PATH": parent.get("PATH", "/usr/bin:/bin"),
        "NO_COLOR": "1",
        "CARGO_TERM_COLOR": "never",
        "CARGO_NET_OFFLINE": "true",
        "RUSTUP_HOME": parent.get("RUSTUP_HOME") or invoking_rustup_home(),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
    }
    for name in FIXTURE_INHERITED:
        if name in parent:
            env[name] = parent[name]
    return env


def stage(source: str, root: str, label: str, log: str) -> str:
    """Build `oxide` from `source` into its own fresh target and build dirs."""
    stage_root = os.path.join(root, f"stage-{label}")
    os.makedirs(stage_root)
    env = staging_env(dict(os.environ), stage_root)
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
            cwd=source,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging {label} exit {build.returncode}: {handle.read()[-3000:]}")
    built = os.path.join(stage_root, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging {label} produced no binary at {built}")
    subject = os.path.join(root, f"subject-{label}")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage_root)
    os.unlink(log)
    return subject


def stage_base(root: str) -> tuple[str, str]:
    """The pinned predecessor: the merge-base tree exported with git archive.

    This is the binary an installed predecessor would be. It never reads or
    mutates the working tree and it is built from the exact base commit.
    """
    base = git("merge-base", "HEAD", "origin/main")
    expect(len(base) == 40, f"merge-base {base!r} is not a commit")
    source = os.path.join(root, "base-source")
    os.makedirs(source)
    export = subprocess.run(
        f"git archive {base} | tar -x -C {source}",
        shell=True,
        cwd=REPO,
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    expect(export.returncode == 0, f"git archive {base} failed: {export.stderr[-2000:]}")
    expect(
        os.path.isfile(os.path.join(source, "scripts/bounded-test.py")),
        "the exported base tree is not this repository",
    )
    # Pinned-source premise: the exported tree must predate this slice's
    # fences, and this HEAD must carry them. A base that already knew the
    # custody fence could not prove a predecessor refusal.
    base_pool = read_text(os.path.join(source, "src/lease/pool.rs"))
    base_control = read_text(os.path.join(source, "src/lease/control.rs"))
    expect(
        "DELETION_CUSTODY_CAPACITY_POLICY_VERSION" not in base_pool,
        "the pinned base tree already carries the custody ledger fence",
    )
    expect(
        "deletion-custody" not in base_control,
        "the pinned base tree already carries the custody control fence",
    )
    expect(
        "DELETION_CUSTODY_CAPACITY_POLICY_VERSION" in read_text(os.path.join(REPO, "src/lease/pool.rs")),
        "this HEAD does not carry the custody ledger fence",
    )
    subject = stage(source, root, "base", os.path.join(root, "stage-base.log"))
    shutil.rmtree(source)
    return subject, base


def invoking_rustup_home() -> str:
    explicit = os.environ.get("RUSTUP_HOME")
    if explicit:
        return explicit
    return os.path.join(os.path.expanduser("~"), ".rustup")


def fixture_env(root: str, pool: str) -> dict:
    env = fixture_environment(dict(os.environ), root, pool)
    private_dir(env["HOME"])
    private_dir(env["CARGO_HOME"])
    return env


def create_project(root: str, label: str) -> str:
    path = private_dir(os.path.join(root, "projects", label))
    write_private(
        os.path.join(path, "Cargo.toml"),
        f'[package]\nname = "oxide-deletion-custody-{label}"\n'
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
    document = read_json(os.path.join(pool, "ledger.json")) or {}
    expect(bool(document), "the ledger is absent or unreadable")
    return document


def slots(pool: str) -> list[dict]:
    return ledger(pool).get("slots", [])


def slot_directory(pool: str, slot: dict) -> str:
    root = os.path.join(pool, "payload", "slots")
    if "dir" in slot:
        return os.path.join(root, slot["dir"])
    return os.path.join(root, str(slot["id"]))


def seed_two_measured_idle_slots(root: str, subject: str) -> tuple[str, dict, dict, dict]:
    """Two idle slots whose closed observations include a 5 MiB planted file.

    Each project's second warm command re-measures the slot on release, so the
    recorded size (and therefore admission pressure) includes the planted
    bytes. Total pressure then exceeds the 10 MiB cap, so the third project's
    admission evicts the oldest idle slot.
    """
    pool = os.path.join(root, "pool")
    env = fixture_env(root, pool)
    workspaces = {}
    for label in ("a", "b"):
        workspace = create_project(root, label)
        workspaces[label] = workspace
        code, output = check(subject, workspace, env, f"seed-{label}")
        expect(code == 0, f"seeding slot {label} exited {code}: {output[-3000:]}")
    document = ledger(pool)
    expect(len(document["slots"]) == 2, f"expected two slots, found {document['slots']}")
    by_rank = sorted(document["slots"], key=lambda slot: slot["rank"])
    for index, label in enumerate(("a", "b")):
        slot = by_rank[index]
        planted = os.path.join(slot_directory(pool, slot), "build", "planted.bin")
        write_bytes(planted, PLANT_BYTES)
        code, output = check(subject, workspaces[label], env, f"remeasure-{label}")
        expect(code == 0, f"re-measuring slot {label} exited {code}: {output[-3000:]}")
    document = ledger(pool)
    expect(len(document["slots"]) == 2, "re-measuring changed the slot count")
    for slot in document["slots"]:
        recorded = slot.get("recorded") or {}
        expect(
            recorded.get("unique_bytes", 0) >= PLANT_BYTES,
            f"slot {slot['id']} has no recorded planted bytes: {recorded}",
        )
        expect(slot["state"] == "idle", f"slot {slot['id']} is not idle")
    oldest = min(document["slots"], key=lambda slot: slot["rank"])
    return pool, env, workspaces, oldest


def begin_paused_admission(
    subject: str, root: str, pool: str, env: dict, gate: str
) -> subprocess.Popen:
    """Start the third project's command paused at the durable root plan."""
    workspace = create_project(root, "c")
    child_env = dict(env)
    child_env["OXIDE_TEST_DELETION_PAUSE"] = f"{PAUSE_POINT}:{gate}"
    child = subprocess.Popen(
        [subject, "check", "--offline", "-j", str(FIXTURE_JOBS)],
        cwd=workspace,
        env=child_env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return child


def wait_for_marker(gate: str, child: subprocess.Popen) -> str:
    deadline = time.monotonic() + max(1.0, work_left())
    while time.monotonic() < deadline:
        entries = [
            name
            for name in os.listdir(os.path.dirname(gate))
            if name.startswith(os.path.basename(gate) + f".{PAUSE_POINT}-")
        ]
        if entries:
            return os.path.join(os.path.dirname(gate), sorted(entries)[0])
        if child.poll() is not None:
            output = child.stdout.read() if child.stdout else ""
            abort(
                f"the paused command exited {child.returncode} before the "
                f"{PAUSE_POINT} marker appeared: {output[-3000:]}"
            )
        time.sleep(0.05)
    abort(f"the {PAUSE_POINT} marker did not appear")


def open_gate(gate: str) -> None:
    with open(gate, "w", encoding="utf-8") as handle:
        handle.write("go\n")


def reap(child: subprocess.Popen, limit: float = 120.0) -> tuple[int, str]:
    try:
        output, _ = child.communicate(timeout=min(limit, max(1.0, work_left())))
    except subprocess.TimeoutExpired:
        child.kill()
        output, _ = child.communicate(timeout=10)
        abort(f"a paused fixture command did not finish after the gate: {output[-2000:]}")
    return child.returncode, output or ""


def kill_only(child: subprocess.Popen) -> None:
    """Kill exactly this fixture process and reap it."""
    child.kill()
    try:
        child.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        abort(f"fixture process {child.pid} was not reaped")


def pending_slot(pool: str) -> dict:
    evicting = [slot for slot in slots(pool) if slot.get("state") == "evicting"]
    expect(len(evicting) == 1, f"expected one pending deletion, found {evicting}")
    return evicting[0]


def custody_oracle(pool: str, slot: dict) -> tuple[str, dict]:
    """The published custody of one pending slot, checked as the proof reads it."""
    custody = slot.get("custody")
    expect(isinstance(custody, dict), f"the pending slot has no custody: {slot}")
    nonce = custody.get("nonce", "")
    expect(
        len(nonce) == 32 and all(character in "0123456789abcdef" for character in nonce),
        f"the custody nonce is malformed: {nonce!r}",
    )
    expect(isinstance(custody.get("lock_device"), int), "the lock device is not recorded")
    expect(isinstance(custody.get("lock_inode"), int), "the lock inode is not recorded")
    directory = custody_dir(pool, nonce)
    expect(os.path.isdir(directory), f"the custody directory {directory} is absent")
    expect(
        stat.S_IMODE(os.lstat(directory).st_mode) == 0o700,
        "the custody directory is not owner-only",
    )
    lock = os.path.join(directory, "lock")
    lock_metadata = os.lstat(lock)
    expect(stat.S_ISREG(lock_metadata.st_mode), "the custody lock is not a regular file")
    expect(
        stat.S_IMODE(lock_metadata.st_mode) == 0o600,
        "the custody lock is not owner-only",
    )
    expect(lock_metadata.st_dev == custody["lock_device"], "the lock device changed")
    expect(lock_metadata.st_ino == custody["lock_inode"], "the lock inode changed")
    journal = read_json(os.path.join(directory, "journal.json"))
    expect(isinstance(journal, dict), "the custody journal is absent or unreadable")
    expect(journal.get("nonce") == nonce, "the journal names another nonce")
    expect(journal.get("slot") == slot["id"], "the journal names another slot")
    expect(
        journal.get("incarnation") == slot.get("incarnation"),
        "the journal names another generation",
    )
    expect(
        (journal.get("capture") or {}).get("root", {}).get("phase") in ("planned", "captured"),
        f"the journal has no planned root: {journal.get('capture')}",
    )
    return directory, journal


def seed_one_idle_slot(root: str, subject: str) -> tuple[str, dict, str]:
    """One idle slot of one private project, created by the candidate."""
    pool = os.path.join(root, "pool")
    env = fixture_env(root, pool)
    workspace = create_project(root, "a")
    code, output = check(subject, workspace, env, "fence-seed")
    expect(code == 0, f"seeding the fenced pool exited {code}: {output[-3000:]}")
    document = ledger(pool)
    expect(len(document["slots"]) == 1, f"expected one slot: {document['slots']}")
    expect(document["slots"][0]["state"] == "idle", "the seeded slot is not idle")
    return pool, env, workspace


def run_predecessor(base: str, workspace: str, env: dict, label: str) -> tuple[int, str]:
    """Run the pinned predecessor's own status against one fixture pool."""
    work_left()
    result = subprocess.run(
        [base, "status"],
        cwd=workspace,
        env=env,
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    combined = f"{result.stdout}{result.stderr}"
    say(f"predecessor {label}: exit {result.returncode}")
    return result.returncode, combined


def prove_old_binary(refusal_root: str, candidate: str, base: str) -> None:
    """The pinned predecessor refuses both fence states without any effect.

    Three probes, each on the real merge-base binary:

    - **intermediate** (crash between the two fences): the control state is
      version 9 with the custody fence, and the ledger is still the
      predecessor's own policy-3 shape. The predecessor must refuse at its
      control parser, before it reads the ledger.
    - **final**: control version 9 and the policy-5 ledger. Refused at control
      load, with control, ledger and every slot root byte-identical.
    - **ledger-only**: a real pending custody record with the predecessor's own
      control version (8, no custody fence), so the refusal can only come from
      its ledger parser reading policy 5 and the `custody` field. Custody
      directory, journal and payload must be byte/identity identical.
    """

    def body() -> None:
        fence_root = private_dir(os.path.join(refusal_root, "fence"))
        pool, env, workspace = seed_one_idle_slot(fence_root, candidate)
        state_path = os.path.join(pool, "oxide-managed", "state.json")
        ledger_path = os.path.join(pool, "ledger.json")
        control = read_bytes(state_path)
        final_ledger = read_bytes(ledger_path)
        document = json.loads(final_ledger)
        expect(
            document.get("capacity_policy") == 5,
            f"the candidate did not publish the policy-5 ledger fence: {document}",
        )
        expect(
            b'"deletion_custody":"oxide.deletion-custody/1"' in control,
            "the candidate did not publish the deletion-custody control fence",
        )
        roots = [slot_directory(pool, slot) for slot in document["slots"]]
        identities = [identity(path) for path in roots]

        # Intermediate: the control fence is durable, the ledger write that
        # would carry policy 5 has not happened (the predecessor's own policy 3
        # is what such a ledger holds).
        intermediate = dict(document)
        intermediate["capacity_policy"] = 3
        write_private_atomic(ledger_path, json.dumps(intermediate).encode())
        intermediate_ledger = read_bytes(ledger_path)
        intermediate_exit, output = run_predecessor(base, workspace, env, "intermediate")
        expect(
            intermediate_exit != 0,
            f"the predecessor accepted the intermediate fence state: {output[-2000:]}",
        )
        expect(read_bytes(state_path) == control, "the intermediate probe rewrote control")
        expect(
            read_bytes(ledger_path) == intermediate_ledger,
            "the intermediate probe rewrote the ledger",
        )
        expect(
            [identity(path) for path in roots] == identities
            and all(os.path.isdir(path) for path in roots),
            "the intermediate probe changed a slot root",
        )

        # Final: control version 9 and the policy-5 ledger.
        write_private_atomic(ledger_path, final_ledger)
        final_exit, output = run_predecessor(base, workspace, env, "final")
        expect(final_exit != 0, f"the predecessor accepted the final state: {output[-2000:]}")
        expect(read_bytes(state_path) == control, "the final probe rewrote control")
        expect(
            read_bytes(ledger_path) == final_ledger,
            "the final probe rewrote the ledger",
        )
        expect(
            [identity(path) for path in roots] == identities
            and all(os.path.isdir(path) for path in roots),
            "the final probe changed a slot root",
        )

        # Resume: the crash-between-fences state is recoverable by this binary
        # without deleting anything. The locked open upgrades the ledger fence;
        # the candidate's own leasing command legitimately rewrites its record
        # state (last use, next use, origin), so the facts this step requires
        # are that the control document keeps the version-9 fences and that the
        # recorded slot set and every slot root are unchanged — never byte
        # identity of the control document.
        write_private_atomic(ledger_path, json.dumps(intermediate).encode())
        resume_code, resume_output = check(candidate, workspace, env, "fence-resume")
        expect(
            resume_code == 0,
            "the candidate could not resume the intermediate fence state: "
            f"{resume_output[-3000:]}",
        )
        resumed: dict = json.loads(read_bytes(ledger_path))
        expect(
            resumed.get("capacity_policy") == 5,
            f"the resumed ledger did not publish policy 5: {resumed}",
        )
        resume_ok, resume_reason = resume_oracle(
            read_bytes(state_path),
            identities,
            [identity(path) for path in roots],
            len(document["slots"]),
            len(resumed.get("slots", [])),
        )
        expect(
            resume_ok,
            f"the resumed fence state failed its oracle: {resume_reason}",
        )
        say(f"resume oracle: {resume_reason}")

        # Ledger-only probe: a real paused deletion, killed mid-deletion, so the
        # record, custody directory, journal and payload all exist. Its control
        # state is rewritten to the predecessor's own current version (8, no
        # custody fence) so the refusal can only be the ledger parser's.
        probe_root = private_dir(os.path.join(refusal_root, "ledger-probe"))
        pool2, env2, workspaces2, oldest = seed_two_measured_idle_slots(
            probe_root, candidate
        )
        oldest_path = slot_directory(pool2, oldest)
        gate = os.path.join(probe_root, "pause-gate")
        child = begin_paused_admission(candidate, probe_root, pool2, env2, gate)
        wait_for_marker(gate, child)
        slot = pending_slot(pool2)
        expect(slot["id"] == oldest["id"], "the ledger probe paused on another slot")
        directory, _journal = custody_oracle(pool2, slot)
        kill_only(child)
        expect(pending_slot(pool2)["id"] == oldest["id"], "the pending record changed")
        state2 = os.path.join(pool2, "oxide-managed", "state.json")
        ledger2 = os.path.join(pool2, "ledger.json")
        predecessor_control = json.loads(read_bytes(state2))
        expect(
            predecessor_control.pop("deletion_custody", None)
            == "oxide.deletion-custody/1",
            "the control state had no custody fence to remove",
        )
        predecessor_control["version"] = PREDECESSOR_CONTROL_VERSION
        write_private_atomic(state2, json.dumps(predecessor_control).encode())
        control2 = read_bytes(state2)
        ledger2_before = read_bytes(ledger2)
        custody_before = tree_digest(directory)
        payload_before = identity(oldest_path)
        code, output = run_predecessor(base, workspaces2["a"], env2, "ledger-only")
        expect(
            code != 0,
            f"the predecessor accepted a policy-5 ledger with custody: {output[-2000:]}",
        )
        expect(read_bytes(state2) == control2, "the ledger probe rewrote control")
        expect(
            read_bytes(ledger2) == ledger2_before,
            "the ledger probe rewrote the ledger",
        )
        expect(
            tree_digest(directory) == custody_before,
            "the ledger probe changed the custody directory or journal",
        )
        expect(
            identity(oldest_path) == payload_before and os.path.isdir(oldest_path),
            "the ledger probe changed the pending payload",
        )
        RESULTS.append(
            {
                "case": "old-binary-refuses-fenced-pool",
                "intermediate_exit": intermediate_exit,
                "final_exit": final_exit,
                "resume_exit": resume_code,
                "resume_fence_retained": True,
                "resume_no_deletion": True,
                "resume_oracle": resume_reason,
                "ledger_only_exit": code,
                "intermediate_control_and_ledger_unchanged": True,
                "final_control_and_ledger_unchanged": True,
                "ledger_only_control_ledger_custody_payload_unchanged": True,
                "slot_identities_unchanged": True,
            }
        )

    exercise("old-binary-refuses-fenced-pool", body)


def prove_pause(pause_root: str, candidate: str) -> None:
    def body() -> None:
        pool, env, _workspaces, oldest = seed_two_measured_idle_slots(
            pause_root, candidate
        )
        oldest_path = slot_directory(pool, oldest)
        oldest_identity = identity(oldest_path)
        gate = os.path.join(pause_root, "pause-gate")
        child = begin_paused_admission(candidate, pause_root, pool, env, gate)
        marker = wait_for_marker(gate, child)
        say(f"paused at {os.path.basename(marker)}")
        slot = pending_slot(pool)
        expect(slot["id"] == oldest["id"], "the pause is not the oldest idle deletion")
        directory, journal = custody_oracle(pool, slot)
        document = ledger(pool)
        expect(
            document.get("capacity_policy") == 5,
            "the pending record was published without the policy-5 fence",
        )
        expect(
            slot.get("capture") is None,
            "the in-ledger journal is still written",
        )
        expect(
            os.path.isdir(os.path.join(directory, "..")) and os.path.isdir(directory),
            "the custody directory is not under the private holdings",
        )
        expect(
            identity(oldest_path) == oldest_identity and os.path.isdir(oldest_path),
            "the payload was touched before the gate was released",
        )
        expect(
            len(custody_entries(pool)) >= 1,
            "no custody directory was published",
        )
        open_gate(gate)
        code, output = reap(child)
        expect(code == 0, f"the released admission exited {code}: {output[-3000:]}")
        after = slots(pool)
        expect(
            all(row["id"] != oldest["id"] for row in after),
            "the released deletion did not conclude",
        )
        expect(not os.path.exists(oldest_path), "the evicted slot directory survived")
        expect(
            not os.path.isdir(directory),
            "the completed deletion kept its private custody directory",
        )
        say(f"journal bytes inspected: {len(json.dumps(journal))}")
        RESULTS.append(
            {
                "case": "custody-and-journal-at-pause",
                "exit": code,
                "paused_slot": oldest["id"],
                "custody_directory_retired": True,
                "payload_untouched_before_gate": True,
            }
        )

    exercise("custody-and-journal-at-pause", body)


def prove_kill_and_replay(kill_root: str, candidate: str) -> None:
    def body() -> None:
        pool, env, workspaces, oldest = seed_two_measured_idle_slots(kill_root, candidate)
        oldest_path = slot_directory(pool, oldest)
        gate = os.path.join(kill_root, "pause-gate")
        child = begin_paused_admission(candidate, kill_root, pool, env, gate)
        wait_for_marker(gate, child)
        slot = pending_slot(pool)
        directory, _journal = custody_oracle(pool, slot)
        expect(os.path.isdir(oldest_path), "the paused payload is already gone")
        kill_only(child)
        say(f"killed fixture process at the {PAUSE_POINT} boundary")
        # The killed process released the pool and custody locks; the record,
        # its custody, its journal and its payload stay.
        expect(os.path.isdir(oldest_path), "the killed deletion touched the payload")
        expect(os.path.isdir(directory), "the killed deletion removed its custody")
        expect(pending_slot(pool)["id"] == oldest["id"], "the pending record changed")
        # The next leasing command claims custody exclusively and replays.
        # (`status` is a passive observer: it reports the pending record and
        # never reconciles or deletes.)
        code, output = check(candidate, workspaces["a"], env, "replay")
        expect(code == 0, f"the replay failed: {output[-3000:]}")
        expect(
            all(row["id"] != oldest["id"] for row in slots(pool)),
            "the replay did not drop the matching record",
        )
        expect(not os.path.exists(oldest_path), "the replay did not delete the slot")
        expect(not os.path.isdir(directory), "the replay kept the custody directory")
        RESULTS.append(
            {
                "case": "killed-mid-deletion-then-replay",
                "killed_slot": oldest["id"],
                "replay_exit": code,
                "record_dropped": True,
                "payload_deleted": True,
            }
        )

    exercise("killed-mid-deletion-then-replay", body)


def prove_busy_skip(busy_root: str, candidate: str) -> None:
    def body() -> None:
        pool, env, workspaces, oldest = seed_two_measured_idle_slots(busy_root, candidate)
        oldest_path = slot_directory(pool, oldest)
        gate = os.path.join(busy_root, "pause-gate")
        child = begin_paused_admission(candidate, busy_root, pool, env, gate)
        wait_for_marker(gate, child)
        slot = pending_slot(pool)
        directory, _journal = custody_oracle(pool, slot)
        kill_only(child)
        say("left a pending deletion with its custody and payload")
        # The proof process is now the live custodian of that custody lock.
        lock = os.open(os.path.join(directory, "lock"), os.O_RDWR)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            pending_before = json.dumps(slot, sort_keys=True)
            lock_before = identity(os.path.join(directory, "lock"))
            journal_before = read_bytes(os.path.join(directory, "journal.json"))
            # A concurrent command completes by warm reuse of the other idle
            # slot; it never waits for custody and never touches the object.
            # The evicted slot is project a's (the oldest rank); project b's
            # idle slot is the warm reuse the concurrent command takes.
            code, output = check(candidate, workspaces["b"], env, "busy-admission")
            expect(code == 0, f"a concurrent command refused a busy custody: {output[-3000:]}")
            expect(
                pending_slot(pool)["id"] == oldest["id"],
                "the concurrent command dropped the busy record",
            )
            expect(
                json.dumps(pending_slot(pool), sort_keys=True) == pending_before,
                "the concurrent command changed the busy record",
            )
            expect(
                identity(os.path.join(directory, "lock")) == lock_before
                and read_bytes(os.path.join(directory, "journal.json")) == journal_before,
                "the concurrent command touched the busy custody",
            )
            expect(os.path.isdir(oldest_path), "the concurrent command touched the busy payload")
            expect(os.path.isdir(directory), "the concurrent command removed the busy custody")
            RESULTS.append(
                {
                    "case": "busy-custody-is-skipped",
                    "exit": code,
                    "busy_record_kept": True,
                    "payload_untouched": True,
                }
            )
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)
            os.close(lock)
        # With the lock released, a later leasing command finishes the
        # deletion. (`status` is a passive observer and never reconciles.)
        code, output = check(candidate, workspaces["a"], env, "release-replay")
        rows = slots(pool)
        expect(
            all(row["id"] != oldest["id"] for row in rows),
            "the released custody was not replayed: "
            f"check={(output[-1500:])!r} "
            f"slots={json.dumps(rows)[-2500:]} custody={custody_entries(pool)}",
        )
        expect(code == 0, f"the release replay exited {code}: {output[-3000:]}")
        expect(not os.path.exists(oldest_path), "the released replay kept the payload")
        say("released custody replayed by a later command")

    exercise("busy-custody-is-skipped", body)


def write_receipt(receipt: dict) -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    if not evidence:
        print("no supervisor evidence directory; receipt stays on stdout", file=sys.stderr)
        return ""
    evidence = os.path.abspath(evidence)
    payload = json.dumps(receipt, indent=2, sort_keys=True).encode()
    expect(len(payload) <= MAX_RECEIPT_BYTES, "the receipt exceeds its byte cap")
    stamp = f"{int(time.time() * 1000)}-{os.getpid()}"
    path = os.path.join(evidence, f"deletion-custody-receipt.{stamp}.json")
    temporary = f"{path}.tmp"
    with open(temporary, "wb") as handle:
        handle.write(payload)
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    say(f"helper receipt {path}")
    return path


def run_proof(scratch: str, commit: str, tree: str, runner_sha: str) -> None:
    candidate = stage(REPO, scratch, "candidate", os.path.join(scratch, "stage-candidate.log"))
    base, base_commit = stage_base(scratch)
    candidate_digest = sha256_file(candidate)
    base_digest = sha256_file(base)
    say(f"candidate sha256 {candidate_digest}")
    say(f"merge-base {base_commit} sha256 {base_digest}")
    expect(candidate_digest != base_digest, "the base and candidate binaries are identical")
    for name, body in (
        ("old-binary", lambda root: prove_old_binary(root, candidate, base)),
        ("pause", lambda root: prove_pause(root, candidate)),
        ("kill-replay", lambda root: prove_kill_and_replay(root, candidate)),
        ("busy-skip", lambda root: prove_busy_skip(root, candidate)),
    ):
        root = private_dir(os.path.join(scratch, f"fixture-{name}"))
        body(root)
        work_left()
    say(f"cases run: {', '.join(CASES)}")
    receipt = {
        "task": "oxide#181",
        "kind": "deletion-custody helper receipt",
        "selector": SELECTOR,
        "source_commit": commit,
        "source_tree": tree,
        "base_commit": base_commit,
        "binary_sha256": candidate_digest,
        "base_binary_sha256": base_digest,
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
            "staged_binaries": 2,
            "fixture_commands": "sequential",
            "stage_cargo_jobs": STAGE_JOBS,
            "fixture_cargo_jobs": FIXTURE_JOBS,
        },
        "authority": "This helper receipt records only the script's own case "
        "oracles. The guard's own receipt, the actual caller exit and the "
        "session closure remain separately authoritative.",
        "live_pool": False,
        "result": "pass",
    }
    if DIAGNOSTIC:
        # A diagnostic is never a PASS: main writes its `qualified: false`
        # receipt and this counted-shape document is not emitted.
        print(json.dumps(receipt, indent=2, sort_keys=True), flush=True)
        return
    write_receipt(receipt)
    print(json.dumps(receipt, indent=2, sort_keys=True), flush=True)


def main(argv: list[str]) -> int:
    global DIAGNOSTIC
    mode, error = parse_arguments(argv)
    if mode == "refuse":
        print(f"deletion-custody: {error}", file=sys.stderr)
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
    if mode == "diagnostic":
        DIAGNOSTIC = True
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
    try:
        run_proof(scratch, commit, tree, runner_sha)
    except SystemExit:
        if DIAGNOSTIC:
            write_receipt(
                {
                    "task": "oxide#181",
                    "kind": "deletion-custody diagnostic receipt",
                    "qualified": False,
                    "source_commit": commit,
                    "cases": RESULTS,
                    "failures": FAILURES,
                    "reached": CASES,
                }
            )
            return 0
        raise
    if DIAGNOSTIC:
        write_receipt(
            {
                "task": "oxide#181",
                "kind": "deletion-custody diagnostic receipt",
                "qualified": False,
                "source_commit": commit,
                "cases": RESULTS,
                "failures": FAILURES,
                "reached": CASES,
            }
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
