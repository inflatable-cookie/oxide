#!/usr/bin/env python3
"""Focused proof for the bounded Finder-metadata race in owned-tree deletion
(oxide#155).

A short fixed script and the whole command of the qualified shared proof
supervisor:

    effigy rust:finder-metadata-race
    effigy rust:finder-metadata-race:diagnostic

whose recipes are `python3 scripts/bounded-test.py --root-parent /private/tmp
--limit 300 --scratch-budget-mib 512 -- python3
scripts/test-finder-metadata-race.py [--diagnostic]`. Admission, the
wall-clock limit, the host reserve, the owned scratch bound, child sessions and
cleanup belong to the supervisor; this script starts no session, signals no
group, and keeps no clock, reserve reading or scratch of its own. It writes only
inside `OXIDE_TASK_SCRATCH_ROOT` and `OXIDE_PROOF_EVIDENCE`.

`--root-parent /private/tmp` is the owner-accepted internal-placement exception
(decision `bd357a4c` v4): every fixture pool here is a genuine *internal*
directory-budget pool, which the product correctly refuses on an external
volume. It stays inside the same monitored 512 MiB owned root.

Stages this HEAD's debug `oxide` offline with the pinned 1.97.1 toolchain
through plain Cargo, then drives the debug-only race injection
(`OXIDE_TEST_FINDER_RACE`, absent from release builds and armed only under the
supervisor's exported layout) at exactly the boundary the product documents —
after the caller's listing, before the removal:

- `lease_control`: two admissions under a 1 MiB budget; the second evicts the
  first project's idle whole slot with no race armed. The slot *leaf* is gone
  and no deletion is pending. Fixture premise.
- `lease_metadata`: the same eviction raced with `.DS_Store`/`._*`. The
  eviction completes and the evidence directory records the firing.
- `lease_foreign`: raced with an ordinary file. The admission refuses, the slot
  leaf and the new file stay, and `status` reports the recorded deletion as
  pending reconciliation.
- `lease_link`: raced with a metadata-named symbolic link. It refuses, the link
  is neither followed nor removed, the deletion stays pending.
- `lease_exhaust`: raced with tolerated metadata on *every* attempt. The fixed
  bound is used up and the deletion stays pending; never a success.
- `prune_metadata`: one owned 128 MiB APFS image, registered as the permanent
  outputs drive, with one checkout built on it and then removed. `oxide outputs
  prune` (exact preview plan) completes the deletion with metadata recreated in
  the quarantine; no quarantined leaf is left.
- `prune_foreign`: the same, raced with an ordinary file. The run exits 3 with
  its intent pending, the quarantined leaf is kept and the file is untouched.
- `prune_exhaust`: the same, raced with recreation on every attempt. The run
  exits 3 and the leaf is kept.

Limits, stated rather than implied: `oxide clean` is deliberately unchanged (it
removes exactly its enumerated plan, so new content is left and counted) and has
no case here. No run of this script has passed: its fit is UNMEASURED, and its
uncounted diagnostic and both counted author and independent reviewer natives
each need their own owner activation.

Nine-premise checklist (docs/knowledge/contracts/workflow.md
#proof-authoring-premise-checklist), traced to executable source:

1. private HOME and rustup: `INVOKING_RUSTUP_HOME` is captured before any HOME
   override and handed to staging and every fixture child with
   `RUSTUP_TOOLCHAIN=1.97.1` and `RUSTUP_AUTO_INSTALL=0`; fixture crates have
   no dependencies and every fixture Cargo call is unlocked, so no lock is
   generated for a `--locked` call; only the staging call is `--locked` and it
   uses this repository's own committed lock.
2. account-home control: `OXIDE_TEST_HOME_FROM_ENV=1` in `case_env`.
3. environment isolation: `case_env` drops every inherited `OXIDE_*`, `XDG_*`,
   `CARGO_*`, `GIT_*`, `RUSTUP_*`, `RUSTC*` and `RUSTFLAGS` before setting only
   the fixture's explicit values, then re-adds the supervisor's own
   `OXIDE_TASK_SCRATCH_ROOT`, `OXIDE_PROOF_DEADLINE_NS`,
   `OXIDE_PROOF_EVIDENCE` (required for the hook guard) and, per case,
   `OXIDE_TEST_FINDER_RACE`.
4. automatic adoption: the prune cases let the first build adopt the fresh
   checkout and then assert the receipt state before deleting the checkout; the
   lease cases adopt nothing.
5. Cargo external argv: not applicable, no external subcommand is invoked.
6. owned images: every prune case attaches its own sparse image with
   `hdiutil attach -owners on`, parses the raw plist, requires exactly one
   mounted entity carrying this run's expected mount point, and detaches it
   ordinarily in a `finally` path.
7. compiler flags: `RUSTFLAGS` is frozen for staging and every fixture child and
   `CARGO_ENCODED_RUSTFLAGS` is removed everywhere.
8. temporary storage: scratch and `TMPDIR`/`TMP`/`TEMP` come from the
   supervisor's owned scratch; the selector declares the internal-placement
   exception above.
9. macOS mktemp: no `mktemp` is used; each case makes its own directory with a
   random suffix under the owned scratch.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import json
import os
import plistlib
import secrets
import shutil
import subprocess
import time

MIB = 1024 * 1024
GIB = 1024 * MIB
BUILDER = "1.97.1"
POOL_BUDGET = 1 * MIB
IMAGE_MIB = 128
RESERVE = "16MiB"
IMAGE_CAP = "64MiB"
SHUTDOWN_MARGIN_SECONDS = 25
MUTED = ("OXIDE_", "XDG_", "CARGO_", "GIT_", "RUSTUP_", "RUSTC", "RUSTFLAGS")
CRATE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    # The slot's recorded figure has to exceed the pool budget so the second
    # admission evicts it; a build script writing a few MiB makes that
    # independent of how a one-file crate is sized.
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
CASES = (
    "lease_control",
    "lease_metadata",
    "lease_foreign",
    "lease_link",
    "lease_exhaust",
    "prune_metadata",
    "prune_foreign",
    "prune_exhaust",
)
# The race each case arms: `repeat` recreates on every attempt, so the fixed
# bound is exercised; `None` is the unraced premise control.
RACES = {
    "lease_control": None,
    "lease_metadata": "*:metadata",
    "lease_foreign": "*:foreign",
    "lease_link": "*:link",
    "lease_exhaust": "*:repeat",
    "prune_metadata": "*:metadata",
    "prune_foreign": "*:foreign",
    "prune_exhaust": "*:repeat",
}
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INVOKING_RUSTUP_HOME = os.environ.get("RUSTUP_HOME") or os.path.join(
    os.environ.get("HOME", ""), ".rustup"
)


def fail(message: str) -> None:
    raise RuntimeError(message)


def scratch_root() -> str:
    root = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not root:
        fail("OXIDE_TASK_SCRATCH_ROOT is missing; run this through effigy")
    root = os.path.abspath(root)
    if not os.path.isdir(root):
        fail(f"owned scratch {root} is not a directory")
    return root


def evidence_dir() -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    if not evidence:
        fail("OXIDE_PROOF_EVIDENCE is missing; run this through effigy")
    evidence = os.path.abspath(evidence)
    if not os.path.isdir(evidence):
        fail(f"evidence {evidence} is not a directory")
    return evidence


def work_left() -> float:
    """What is left of the supervisor's work window.

    `OXIDE_PROOF_DEADLINE_NS` is in the supervisor's own `CLOCK_UPTIME_RAW`
    domain, not wall time, so the remaining window is measured with the same
    clock the guard used.
    """
    deadline = int(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "0") or 0)
    if deadline <= 0:
        fail("OXIDE_PROOF_DEADLINE_NS is missing; run this through effigy")
    left = (
        deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    ) / 1e9 - SHUTDOWN_MARGIN_SECONDS
    if left <= 5:
        fail("the supervisor's work window is spent")
    return left


def toolchain_env() -> dict:
    """The pinned, offline toolchain and frozen compiler flags."""
    return {
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_HOME": INVOKING_RUSTUP_HOME,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
        "RUSTFLAGS": "-C strip=symbols",
    }


def scrubbed(env_source: dict) -> dict:
    return {
        key: value
        for key, value in env_source.items()
        if not key.startswith(MUTED) and key not in ("RUSTFLAGS", "RUSTUP_TOOLCHAIN")
    }


def run(command: list, *, cwd: str, env: dict, timeout: float = 120.0):
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=min(timeout, work_left()),
    )


def stage(root: str) -> str:
    stage_dir = os.path.join(root, "stage")
    env = scrubbed(os.environ)
    env.update(toolchain_env())
    env.update(
        {
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage_dir, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage_dir, "build"),
            "CARGO_INCREMENTAL": "0",
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "16",
        }
    )
    log = os.path.join(root, "stage.log")
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", "4", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage_dir, "target", "debug", "oxide")
    if not os.path.isfile(built):
        fail(f"staging produced no binary at {built}")
    subject = os.path.join(root, "oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage_dir, ignore_errors=True)
    os.unlink(log)
    return subject


def case_root(root: str, case: str) -> str:
    path = os.path.join(root, "cases", case, secrets.token_hex(4))
    os.makedirs(path, exist_ok=True)
    return path


def case_env(root: str, case: str, race: str | None) -> tuple[dict, str]:
    base = case_root(root, case)
    home = os.path.join(base, "home")
    cargo_home = os.path.join(base, "cargo-home")
    temp = os.path.join(base, "temp")
    for path in (home, cargo_home, temp):
        os.makedirs(path, exist_ok=True)
    env = scrubbed(os.environ)
    env.update(toolchain_env())
    env.update(
        {
            "HOME": home,
            "CARGO_HOME": cargo_home,
            "TMPDIR": temp,
            "TMP": temp,
            "TEMP": temp,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "OXIDE_POOL": os.path.join(base, "pool"),
            "OXIDE_POOL_CAP": str(POOL_BUDGET),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    # The supervisor's own layout is what arms the debug-only injection.
    for key in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE"):
        if key in os.environ:
            env[key] = os.environ[key]
    if race is not None:
        env["OXIDE_TEST_FINDER_RACE"] = race
    return env, base


def workspace(base: str, name: str) -> str:
    path = os.path.join(base, "workspaces", name)
    os.makedirs(os.path.join(path, "src"), exist_ok=True)
    for relative, contents in CRATE.items():
        with open(os.path.join(path, relative), "w", encoding="utf-8") as handle:
            handle.write(contents)
    return path


def slot_leaves(pool: str) -> dict:
    """Every slot *leaf* (`<profile>-<n>`), never its project-label folder.

    A slot lives at `payload/slots/<project folder>/<profile>-<n>`; the folder
    is a label, and only the leaf inside it is one slot.
    """
    root = os.path.join(pool, "payload", "slots")
    leaves = {}
    if not os.path.isdir(root):
        return leaves
    for folder in sorted(os.listdir(root)):
        directory = os.path.join(root, folder)
        if not os.path.isdir(directory):
            continue
        for entry in sorted(os.listdir(directory)):
            path = os.path.join(directory, entry)
            if os.path.isdir(path):
                leaves[path] = sum(1 for _, _, files in os.walk(path) for _ in files)
    return leaves


def tree_files(path: str) -> list:
    return [
        os.path.join(dirpath, name)
        for dirpath, _, names in os.walk(path)
        for name in names
    ]


def evidence_mark() -> int:
    path = os.path.join(evidence_dir(), "finder-race-fired")
    return os.path.getsize(path) if os.path.isfile(path) else 0


def evidence_fired(mark: int) -> str:
    """Only what this case appended: an earlier case's firing proves nothing."""
    path = os.path.join(evidence_dir(), "finder-race-fired")
    if not os.path.isfile(path):
        return ""
    with open(path, encoding="utf-8") as handle:
        handle.seek(mark)
        return handle.read()


def lease_case(oxide: str, root: str, case: str, race: str | None) -> str:
    env, base = case_env(root, case, race)
    mark = evidence_mark()
    first = run([oxide, "check"], cwd=workspace(base, "a"), env=env)
    if first.returncode:
        fail(f"{case}: the first admission failed: {first.stderr[-1500:]}")
    before = slot_leaves(env["OXIDE_POOL"])
    if len(before) != 1:
        fail(f"{case}: the first admission left {len(before)} slot leaves, expected one: {sorted(before)}")
    victim = next(iter(before))
    second = run([oxide, "check"], cwd=workspace(base, "b"), env=env)
    after = slot_leaves(env["OXIDE_POOL"])
    status = run([oxide, "status"], cwd=base, env=env)
    pending = "recorded deletion pending reconciliation" in status.stdout

    if case in ("lease_control", "lease_metadata"):
        if second.returncode:
            fail(f"{case}: the eviction did not complete: {second.stderr[-1500:]}")
        if victim in after:
            fail(f"{case}: the evicted slot leaf {victim} is still present")
        if pending:
            fail(f"{case}: a deletion stayed pending after a complete eviction")
        if race is not None and "Metadata" not in evidence_fired(mark):
            fail(f"{case}: the armed race never fired; the case proves nothing")
        return f"{case}: completed, victim {victim}, slots {len(before)}->{len(after)}"

    if second.returncode == 0:
        fail(f"{case}: an admission succeeded over a refused deletion")
    if victim not in after:
        fail(f"{case}: the refused slot leaf {victim} was removed anyway")
    if not pending:
        fail(f"{case}: the refused deletion is not reported as pending")
    if case == "lease_foreign" and not [
        path for path in tree_files(victim) if os.path.basename(path) == "foreign.txt"
    ]:
        fail(f"{case}: the foreign entry was deleted instead of kept")
    if case == "lease_link" and not [
        os.path.join(dirpath, name)
        for dirpath, dirnames, names in os.walk(victim)
        for name in dirnames + names
        if name == ".DS_Store" and os.path.islink(os.path.join(dirpath, name))
    ]:
        fail(f"{case}: the metadata-named link is gone")
    expected = "Metadata" if case == "lease_exhaust" else ("Foreign" if case == "lease_foreign" else "Link")
    if expected not in evidence_fired(mark):
        fail(f"{case}: the armed race never fired; the case proves nothing")
    return f"{case}: pending, victim {victim}, exit {second.returncode}"


def attach(root: str, case: str) -> tuple:
    """One owned 128 MiB sparse image, attached with ownership enabled."""
    backing = os.path.join(root, "images", f"{case}-{secrets.token_hex(4)}.sparseimage")
    os.makedirs(os.path.dirname(backing), exist_ok=True)
    create = run(
        ["hdiutil", "create", "-size", f"{IMAGE_MIB}m", "-fs", "APFS", "-type", "SPARSE", "-quiet", backing],
        cwd=os.path.dirname(backing),
        env=scrubbed(os.environ),
        timeout=60,
    )
    if create.returncode:
        fail(f"{case}: cannot create the owned image: {create.stderr[-800:]}")
    attached = run(
        ["hdiutil", "attach", "-owners", "on", "-nobrowse", "-plist", backing],
        cwd=os.path.dirname(backing),
        env=scrubbed(os.environ),
        timeout=60,
    )
    if attached.returncode:
        fail(f"{case}: cannot attach the owned image: {attached.stderr[-800:]}")
    try:
        document = plistlib.loads(attached.stdout.encode())
    except Exception as error:  # noqa: BLE001 - one bounded failure line
        fail(f"{case}: the attach plist is unreadable ({error!r}); the backing is kept at {backing}")
    mounted = [
        entity
        for entity in document.get("system-entities", [])
        if entity.get("mount-point") and entity.get("dev-entry")
    ]
    if len(mounted) != 1:
        fail(
            f"{case}: the attach returned {len(mounted)} mounted entities, expected one; "
            f"the backing is kept at {backing}"
        )
    mount = mounted[0]["mount-point"]
    if not os.path.isdir(mount):
        fail(f"{case}: the attached mount point {mount} is not a directory")
    return backing, mount


def detach(case: str, mount: str) -> None:
    """Ordinary detach only; never forced, never retried."""
    result = run(
        ["hdiutil", "detach", mount],
        cwd="/",
        env=scrubbed(os.environ),
        timeout=60,
    )
    if result.returncode:
        fail(f"{case}: the ordinary detach of {mount} failed: {result.stderr[-800:]}")


def prune_case(oxide: str, root: str, case: str, race: str | None) -> str:
    env, base = case_env(root, case, race)
    mark = evidence_mark()
    backing, mount = attach(base, case)
    try:
        register = run(
            [
                oxide,
                "pool",
                "register-removable",
                mount,
                "--reserve",
                RESERVE,
                "--cap",
                IMAGE_CAP,
                "--mode",
                "permanent",
            ],
            cwd=base,
            env=env,
        )
        if register.returncode:
            fail(f"{case}: cannot register the owned drive: {register.stderr[-1500:]}")
        checkout = os.path.join(mount, "checkout")
        shutil.copytree(
            workspace(base, "a"),
            checkout,
        )
        built = run([oxide, "check"], cwd=checkout, env=env)
        if built.returncode:
            fail(f"{case}: the fixture build on the drive failed: {built.stderr[-1500:]}")
        outputs = os.path.join(mount, "oxide-outputs")
        roots = [
            entry
            for entry in sorted(os.listdir(outputs))
            if os.path.isdir(os.path.join(outputs, entry)) and not entry.startswith(".prune-")
        ]
        if len(roots) != 1:
            fail(f"{case}: the build left {len(roots)} owned roots on the drive, expected one: {roots}")
        root_path = os.path.join(outputs, roots[0])
        # The checkout must be positively gone before the root is eligible.
        shutil.rmtree(checkout)
        if os.path.exists(checkout):
            fail(f"{case}: the fixture checkout was not removed")
        preview = run([oxide, "outputs", "prune", "--dry-run"], cwd=base, env=env)
        if preview.returncode:
            fail(f"{case}: the dry run refused: {preview.stderr[-1500:]}")
        digest = ""
        for line in preview.stderr.splitlines():
            for word in line.replace(":", " ").split():
                if len(word) == 64 and all(c in "0123456789abcdef" for c in word):
                    digest = word
        if not digest:
            fail(f"{case}: the dry run printed no plan digest: {preview.stderr[-800:]}")
        plan = os.path.join(base, "plan.json")
        with open(plan, "w", encoding="utf-8", newline="") as handle:
            handle.write(preview.stdout)
        real = run(
            [oxide, "outputs", "prune", "--plan", plan, "--plan-sha256", digest],
            cwd=base,
            env=env,
        )
        quarantined = [
            entry
            for entry in sorted(os.listdir(outputs))
            if entry.startswith(".prune-")
        ] if os.path.isdir(outputs) else []
        kept_root = os.path.isdir(root_path)

        if case == "prune_metadata":
            if real.returncode:
                fail(f"{case}: the prune did not complete: {real.stderr[-1500:]}")
            if quarantined:
                fail(f"{case}: a quarantined leaf was left behind: {quarantined}")
            if kept_root:
                fail(f"{case}: the pruned root is still present")
            if "Metadata" not in evidence_fired(mark):
                fail(f"{case}: the armed race never fired; the case proves nothing")
            return f"{case}: deleted, exit {real.returncode}"

        if real.returncode != 3:
            fail(f"{case}: an unsettled deletion did not exit 3 (exit {real.returncode}: {real.stderr[-1500:]})")
        if not quarantined:
            fail(f"{case}: the pending intent left no quarantined leaf")
        if kept_root:
            fail(f"{case}: the root is still under its live name while a quarantine exists")
        leaf = os.path.join(outputs, quarantined[0])
        if case == "prune_foreign" and not [
            path for path in tree_files(leaf) if os.path.basename(path) == "foreign.txt"
        ]:
            fail(f"{case}: the foreign entry was deleted instead of kept")
        expected = "Foreign" if case == "prune_foreign" else "Metadata"
        if expected not in evidence_fired(mark):
            fail(f"{case}: the armed race never fired; the case proves nothing")
        return f"{case}: pending, quarantine {quarantined[0]}, exit {real.returncode}"
    finally:
        detach(case, mount)


def plan() -> int:
    print("cases:")
    for case in CASES:
        race = RACES[case]
        print(f"  {case:<16} race {race or 'none (control)'}")
    print(
        "allocation: 5 internal 1 MiB directory pools on the boot Data volume "
        "(the selector's internal-placement exception), one private HOME/CARGO_HOME "
        "and TMPDIR per case, 3 owned 128 MiB sparse images attached -owners on "
        "one at a time and ordinarily detached, staged debug oxide in owned scratch"
    )
    print("premises: 9 traced in this script's docstring")
    return 0


def diagnostic_receipt(completed: list, failed: list, faults: dict) -> str:
    receipt = {
        "proof": "oxide#155-finder-metadata-race",
        "qualified": False,
        "mode": "diagnostic",
        "completed": completed,
        "failed": failed,
        "unexecuted": [case for case in CASES if case not in completed and case not in failed],
        "faults": {case: text[-800:] for case, text in faults.items()},
    }
    path = os.path.join(evidence_dir(), "finder-metadata-race-diagnostic.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(receipt, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return path


def main(argv: list) -> int:
    if argv == ["--plan"]:
        return plan()
    diagnostic = False
    if argv:
        if argv != ["--diagnostic"]:
            fail(f"unknown argument {argv[0]!r}; known: --plan, --diagnostic")
        diagnostic = True

    root = scratch_root()
    evidence_dir()
    work_left()
    oxide = stage(root)
    print(f"staged oxide from this HEAD: {oxide}", flush=True)

    completed, failed, faults = [], [], {}
    for case in CASES:
        try:
            line = (
                lease_case(oxide, root, case, RACES[case])
                if case.startswith("lease_")
                else prune_case(oxide, root, case, RACES[case])
            )
            completed.append(case)
            print(line, flush=True)
        except Exception as error:  # noqa: BLE001 - one bounded failure line per case
            text = f"{type(error).__name__}: {error}"
            if not diagnostic:
                raise
            failed.append(case)
            faults[case] = text
            print(f"{case}: FAILED {text}", flush=True)

    if diagnostic:
        receipt = diagnostic_receipt(completed, failed, faults)
        print(
            f"diagnostic: qualified=false, {len(completed)} completed, "
            f"{len(failed)} failed, receipt {receipt}",
            flush=True,
        )
        return 1 if failed else 0
    print(f"proof: {len(completed)} cases passed", flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Exception as error:  # noqa: BLE001 - one bounded failure line
        print(f"proof failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
