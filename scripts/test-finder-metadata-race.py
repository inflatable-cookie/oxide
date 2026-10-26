#!/usr/bin/env python3
"""Focused proof for the bounded Finder-metadata race in owned-tree deletion
(oxide#155).

A short fixed script and the whole command of the qualified shared proof
supervisor:

    effigy rust:finder-metadata-race

whose recipe is `python3 scripts/bounded-test.py --limit 300
--scratch-budget-mib 512 -- python3 scripts/test-finder-metadata-race.py`.
Admission, the wall-clock limit, the host reserve, the owned scratch bound,
child sessions and cleanup belong to the supervisor; this script starts no
session, signals no group and keeps no clock or scratch of its own. It writes
only inside `OXIDE_TASK_SCRATCH_ROOT`.

It stages this HEAD's debug `oxide` offline with the pinned 1.97.1 toolchain
through plain Cargo, and then, over private `HOME`, `CARGO_HOME`, `TMPDIR` and
directory pools, drives *whole-idle-slot eviction* at a real slot tree:

- case `control`: two admissions under a 1 MiB budget. The second evicts the
  first project's idle whole slot with no race armed. The evicted slot's
  directory is gone and `status` records no pending deletion. This is the
  fixture premise: without it every raced case below could pass for the wrong
  reason.
- case `metadata`: the same eviction, with the debug-only race armed so
  `.DS_Store` and `._*` appear after the walk listed the slot and before its
  removal. The eviction completes, the slot is gone, no deletion is pending,
  and the evidence directory records that the injection fired.
- case `foreign`: the same eviction raced with an ordinary file. The admission
  refuses, the slot and the new file are untouched, and `status` still reports
  the recorded deletion as pending reconciliation.
- case `link`: raced with a metadata-named symbolic link. It refuses, the link
  is neither followed nor removed, and the deletion stays pending.
- case `exhaust`: raced with tolerated metadata on every attempt. The fixed
  bound is used up and the deletion stays pending; it never becomes a success.

The debug-only injection (`OXIDE_TEST_FINDER_RACE`, `*:<kind>`) is absent from
release builds and only arms under the supervisor's own layout, which the
supervisor exports; each firing is appended to `finder-race-fired` in its
evidence directory.

Limits, stated rather than implied:

- The prune quarantine caller (`oxide outputs prune`) is NOT covered here: its
  deletion runs on a registered removable volume and needs one owned scratch
  APFS image plus a private registration. That half of the brief's focused
  native is not authored, so it has no source fit and no receipt.
- `oxide clean` is deliberately unchanged (see
  docs/knowledge/contracts/workflow.md, Clean): it removes exactly its
  enumerated plan, so new content is left and counted.
- The uncounted keep-going diagnostic and both counted natives each need their
  own owner activation; this script has not run. Its fit is unmeasured.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import os
import shutil
import subprocess
import time

MIB = 1024 * 1024
POOL_BUDGET = 1 * MIB
WORK_MARGIN_SECONDS = 25
DEADLINE_NS = int(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "0") or 0)
FIXTURE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    # The slot's recorded figure has to exceed the pool budget, so the second
    # admission evicts it. A build script writing a few MiB makes that
    # independent of how [profile] sizing treats a one-file crate.
    "build.rs": (
        "use std::io::Write;\n"
        "fn main() {\n"
        "    println!(\"cargo:rerun-if-changed=build.rs\");\n"
        "    let out = std::env::var(\"OUT_DIR\").expect(\"OUT_DIR\");\n"
        "    let path = std::path::Path::new(&out).join(\"filler\");\n"
        "    let mut file = std::fs::File::create(path).expect(\"filler\");\n"
        "    file.write_all(&vec![0u8; 3 * 1024 * 1024]).expect(\"write\");\n"
        "}\n"
    ),
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    if DEADLINE_NS <= 0:
        fail("OXIDE_PROOF_DEADLINE_NS is missing; run this through effigy rust:finder-metadata-race")
    left = DEADLINE_NS / 1_000_000_000 - time.time() - WORK_MARGIN_SECONDS
    if left <= 5:
        fail("the supervisor's deadline leaves no work window")
    return left


def scratch_root() -> str:
    root = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not root:
        fail("OXIDE_TASK_SCRATCH_ROOT is missing; run this through effigy rust:finder-metadata-race")
    root = os.path.abspath(root)
    if not os.path.isdir(root):
        fail(f"owned scratch {root} is not a directory")
    return root


def ensure_scratch(root: str) -> None:
    """One owned scratch directory; nothing outside it is written."""
    os.makedirs(root, exist_ok=True)


def stage(root: str) -> str:
    stage = os.path.join(root, "stage")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_"))
        and key
        not in (
            "CARGO",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTFLAGS",
            "RUSTUP_TOOLCHAIN",
            "RUSTUP_AUTO_INSTALL",
        )
    }
    env.update(
        {
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_INCREMENTAL": "0",
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
        }
    )
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    log = os.path.join(root, "stage.log")
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", "4", "--locked", "--offline", "--bin", "oxide"],
            cwd=repo,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=remaining(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    if not os.path.isfile(built):
        fail(f"staging produced no binary at {built}")
    subject = os.path.join(root, "oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage, ignore_errors=True)
    os.unlink(log)
    return subject


def case_env(root: str, case: str, race: str | None) -> tuple[dict, str, str]:
    base = os.path.join(root, "cases", case)
    home = os.path.join(base, "home")
    cargo_home = os.path.join(base, "cargo-home")
    temp = os.path.join(base, "temp")
    pool = os.path.join(base, "pool")
    for path in (home, cargo_home, temp):
        os.makedirs(path, exist_ok=True)
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("OXIDE_", "CARGO_", "RUSTC_", "RUSTUP_"))
    }
    env.update(
        {
            "HOME": home,
            "CARGO_HOME": cargo_home,
            "TMPDIR": temp,
            "TMP": temp,
            "TEMP": temp,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": str(POOL_BUDGET),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    # The supervisor's own layout is what arms the debug-only injection.
    for key in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE"):
        if key in os.environ:
            env[key] = os.environ[key]
    if race is None:
        env.pop("OXIDE_TEST_FINDER_RACE", None)
    else:
        env["OXIDE_TEST_FINDER_RACE"] = race
    return env, pool, base


def workspace(base: str, name: str) -> str:
    path = os.path.join(base, "workspaces", name)
    if not os.path.isdir(path):
        os.makedirs(os.path.join(path, "src"), exist_ok=True)
        for relative, contents in FIXTURE.items():
            with open(os.path.join(path, relative), "w", encoding="utf-8") as handle:
                handle.write(contents)
    return path


def run(oxide: str, args: list[str], env: dict, cwd: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [oxide, *args],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=remaining(),
    )


def slots(pool: str) -> dict:
    """Slot folders under the payload root, keyed by their own name."""
    root = os.path.join(pool, "payload", "slots")
    found = {}
    for entry in sorted(os.listdir(root)) if os.path.isdir(root) else []:
        path = os.path.join(root, entry)
        if os.path.isdir(path):
            found[entry] = sum(
                1 for _, _, files in os.walk(path) for _ in files
            )
    return found


def main() -> int:
    root = scratch_root()
    ensure_scratch(root)
    oxide = stage(root)

    results = []
    for case, race, expect in (
        ("control", None, "complete"),
        ("metadata", "*:metadata", "complete"),
        ("foreign", "*:foreign", "pending"),
        ("link", "*:link", "pending"),
        ("exhaust", "*:repeat", "pending"),
    ):
        env, pool, base = case_env(root, case, race)
        first = run(oxide, ["check"], env, workspace(base, "a"))
        if first.returncode:
            fail(f"{case}: the first admission failed: {first.stderr[-1500:]}")
        before = slots(pool)
        if len(before) != 1:
            fail(f"{case}: the first admission left {len(before)} slots, expected one")
        victim = next(iter(before))
        second = run(oxide, ["check"], env, workspace(base, "b"))
        after = slots(pool)
        evidence = ""
        evidence_dir = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
        evidence_path = os.path.join(evidence_dir, "finder-race-fired")
        if race is not None and evidence_dir and os.path.isfile(evidence_path):
            with open(evidence_path, encoding="utf-8") as handle:
                evidence = handle.read()
        status = run(oxide, ["status"], env, base)

        if expect == "complete":
            if second.returncode:
                fail(f"{case}: the eviction did not complete: {second.stderr[-1500:]}")
            if victim in after:
                fail(f"{case}: the evicted slot {victim} is still present")
            if "recorded deletion pending reconciliation" in status.stdout:
                fail(f"{case}: a deletion stayed pending after a complete eviction")
            if race is not None and "Metadata" not in evidence:
                fail(
                    f"{case}: the armed race never fired; the case proves nothing "
                    f"(evidence {evidence_path!r})"
                )
        else:
            if second.returncode == 0:
                fail(f"{case}: an admission succeeded over a refused deletion")
            if victim not in after:
                fail(f"{case}: the refused slot {victim} was removed anyway")
            if "recorded deletion pending reconciliation" not in status.stdout:
                fail(f"{case}: the refused deletion is not reported as pending")
            if case == "foreign":
                path = os.path.join(pool, "payload", "slots", victim)
                found = [
                    os.path.join(dirpath, name)
                    for dirpath, _, files in os.walk(path)
                    for name in files
                    if name == "foreign.txt"
                ]
                if not found:
                    fail(f"{case}: the foreign entry was deleted instead of kept")
            if case == "link":
                path = os.path.join(pool, "payload", "slots", victim)
                links = [
                    os.path.join(dirpath, name)
                    for dirpath, dirnames, files in os.walk(path)
                    for name in dirnames + files
                    if name == ".DS_Store" and os.path.islink(os.path.join(dirpath, name))
                ]
                if not links:
                    fail(f"{case}: the metadata-named link is gone")
            expected = {"metadata": "Metadata", "foreign": "Foreign", "link": "Link", "exhaust": "Metadata"}[case]
            if expected not in evidence:
                fail(
                    f"{case}: the armed race never fired; the case proves nothing "
                    f"(evidence {evidence_path!r})"
                )

        results.append(
            f"{case}: {'completed' if expect == 'complete' else 'pending'}, "
            f"victim {victim}, slots before {len(before)} after {len(after)}, "
            f"exit {second.returncode}"
        )
        print(results[-1], flush=True)

    print(f"proof: {len(results)} cases passed", flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:  # noqa: BLE001 - one bounded failure line
        print(f"proof failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
