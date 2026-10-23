#!/usr/bin/env python3
"""Focused native proof for guarded accounting reads (oxide#141).

One fixed script and the whole command of the qualified shared supervisor
(`bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
scripts/test-accounting-guards.py`): 300 s wall clock including staging,
fixtures and cleanup, 512 MiB monitored scratch, 30 s shutdown margin, one
staged binary and one fixture command at a time. It stages this HEAD's debug
`oxide` offline with the pinned 1.97.1 into its own scratch target, copies the
binary out and deletes that target before the fixtures.

Every fixture is a private scratch directory tree with its own `HOME`,
`CARGO_HOME`, `OXIDE_POOL` and a tiny std-only crate per project. It creates
its own pool on first use, never touches a live pool, `~/.local/bin/oxide`,
`~/.cargo`, an install receipt, a mounted volume or host settings, and it
never runs `status --measure`, a live-pool inventory or a scan of another
task's storage. The cases are end-to-end CLI controls only:

- `union-hardlink-counted-once`: two idle whole slots share one hard-linked
  file of 3 MiB; a third project's admission must count that inode once, stay
  under the 4 MiB budget and evict nothing.
- `union-separate-copies-press-byte-cap`: the identical layout with two
  independent 3 MiB files exceeds the same budget, so the same admission must
  evict exactly one idle whole slot. This makes the previous case
  non-vacuous: the budget and eviction path are live.
- `refusal-incomplete-accounting`: an unreadable directory inside an idle slot
  makes the whole-payload walk partial, so the same admission must refuse with
  exit 3, `unknown_accounting` and `unreadable_entry`, and must change neither
  the ledger nor a slot directory.

Tiny fixtures and deterministic CLI controls do not establish the rate or the
outcome of a large or cold real pool, do not measure the net effect of the
guarded reads, and are not a benchmark, a real-project qualification or an
alias for full QA.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
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
# budget separates a 4 MiB inode counted once from two independent 4 MiB
# copies with slack on both sides.
POOL_CAP = "6MiB"
LINK_BYTES = 4 * MIB
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


def stage_subject(root: str) -> str:
    """Stage this clean HEAD's debug `oxide` with plain Cargo, offline.

    The staged target is deleted once the binary is copied out, keeping the
    monitored scratch far below the 512 MiB gate.
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
            ["cargo", "build", "-j", "1", "--locked", "--offline", "--bin", "oxide"],
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
        "RUSTUP_AUTO_INSTALL": "0",
    }
    for name in (
        "LANG",
        "LC_ALL",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
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
        [subject, "check", "--offline"],
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
    shared = os.path.join(slots[0], "build", "shared.bin")
    write_bytes(shared, LINK_BYTES)
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
        all(os.path.isdir(path) for path in slots),
        "an idle slot directory disappeared although the union was under budget",
    )
    RESULTS.append({"case": "union-hardlink-counted-once", "slots": len(after)})


def prove_separate_copies(root: str, subject: str) -> None:
    case("union-separate-copies-press-byte-cap")
    pool, env, slots = seed_two_idle_slots(root, subject)
    write_bytes(os.path.join(slots[0], "build", "copy-a.bin"), LINK_BYTES)
    write_bytes(os.path.join(slots[1], "build", "copy-b.bin"), LINK_BYTES)
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
    RESULTS.append({"case": "union-separate-copies-press-byte-cap", "slots": len(after)})


def prove_refusal(root: str, subject: str) -> None:
    case("refusal-incomplete-accounting")
    pool, env, slots = seed_two_idle_slots(root, subject)
    blocked = os.path.join(slots[0], "build", "blocked")
    private_dir(blocked)
    write_bytes(os.path.join(blocked, "payload.bin"), 4096)
    os.chmod(blocked, 0o000)
    try:
        before = ledger(pool)["slots"]
        workspace = create_project(root, "c")
        code, output = check(subject, workspace, env, "admit-with-unreadable-slot")
        expect(
            code == 3,
            f"an incomplete whole-payload walk must refuse with exit 3, got {code}: "
            f"{output[-3000:]}",
        )
        expect(
            "unknown_accounting" in output,
            f"the refusal did not name unknown_accounting: {output[-3000:]}",
        )
        expect(
            "unreadable_entry" in output,
            f"the refusal did not name unreadable_entry: {output[-3000:]}",
        )
        after = ledger(pool)["slots"]
        expect(
            [slot["id"] for slot in after] == [slot["id"] for slot in before],
            "the incomplete-accounting refusal changed the ledger",
        )
        expect(
            all(os.path.isdir(path) for path in slots),
            "the incomplete-accounting refusal changed a slot directory",
        )
        RESULTS.append({"case": "refusal-incomplete-accounting", "exit": code})
    finally:
        os.chmod(blocked, 0o700)


def run_cases(scratch: str, commit: str, runner_sha: str) -> str:
    subject = stage_subject(scratch)
    digest = sha256_file(subject)
    say(f"subject sha256 {digest}")
    expect(
        re.fullmatch(r"[0-9a-f]{64}", digest) is not None,
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
    receipt = {
        "task": "oxide#141",
        "source_commit": commit,
        "source_tree": subprocess.run(
            ["git", "rev-parse", "HEAD^{tree}"],
            cwd=REPO,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip(),
        "binary_sha256": digest,
        "helper_sha256": {
            "scripts/bounded-test.py": runner_sha,
            "scripts/test-accounting-guards.py": sha256_file(os.path.abspath(__file__)),
        },
        "allocation": {
            "supervisor": "scripts/bounded-test.py (qualified shared supervisor)",
            "wall_limit_s": WALL_LIMIT_S,
            "scratch_budget_mib": SCRATCH_BUDGET_MIB,
            "shutdown_margin_s": SHUTDOWN_MARGIN_S,
            "phases": ["stage", "union", "pressure", "refusal"],
            "concurrency": "one staged binary; one fixture command at a time",
            "live_pool": False,
            "mounted_volume": False,
        },
        "cases": RESULTS,
        "result": "pass",
    }
    return json.dumps(receipt, indent=2, sort_keys=True)


def main() -> None:
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
    receipt = run_cases(scratch, commit, runner_sha)
    say(f"cases run: {', '.join(CASES)}")
    print(receipt, flush=True)


if __name__ == "__main__":
    main()
