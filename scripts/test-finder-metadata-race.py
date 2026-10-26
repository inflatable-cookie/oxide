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
group, and keeps no clock, reserve reading or scratch of its own. Fixture
scratch is written only inside `OXIDE_TASK_SCRATCH_ROOT`; image backings and
their custody records only inside `OXIDE_PROOF_EVIDENCE`.

`--root-parent /private/tmp` is the owner-accepted internal-placement exception
(decision `bd357a4c` v4): every fixture pool here is a genuine *internal*
directory-budget pool, which the product correctly refuses on an external
volume. It stays inside the same monitored 512 MiB owned root.

Stages this HEAD's debug `oxide` offline with the pinned 1.97.1 toolchain
through plain Cargo, then drives the debug-only race injection
(`OXIDE_TEST_FINDER_RACE`, absent from release builds, armed only under the
supervisor's exported layout) at exactly the boundary the product documents —
after the caller's listing, before the removal:

- `lease_control`: two admissions under a 1 MiB budget; the second evicts the
  first project's idle whole slot with no race armed. The slot *leaf* is gone
  and no deletion is pending. Fixture premise.
- `lease_metadata`: the same eviction raced with `.DS_Store`/`._*`. The
  eviction completes and the evidence records the firing.
- `lease_foreign`: raced with an ordinary file. The admission refuses, the slot
  leaf and the new file stay, and `status` reports the recorded deletion as
  pending reconciliation.
- `lease_link`: raced with a metadata-named symbolic link. It refuses, the link
  is neither followed nor removed, the deletion stays pending.
- `lease_exhaust`: raced with tolerated metadata on *every* attempt. The fixed
  bound is used up and the deletion stays pending; never a success.
- `prune_metadata`: one owned 128 MiB APFS image, registered as the permanent
  outputs drive, with one real private Git checkout on it (ignored `target`)
  whose first build is asserted to have adopted an owned root and member link.
  The checkout is then removed and `oxide outputs prune` runs with exactly its
  preview plan: the deletion completes, no quarantined leaf is left, the live
  registry keeps no intent for that root and no row selects it.
- `prune_foreign`: the same, raced with an ordinary file. The run exits 3, the
  `.prune-<intent>` leaf is kept with the file untouched, and the registry's
  intent for that quarantine is in a non-terminal phase with
  `receipt_settled: false` and no `deleted`/`resolved` terminal state.
- `prune_exhaust`: the same, raced with recreation on every attempt: exit 3 and
  the leaf kept, with the same unfinished journal assertions.
- `prune_missing_quarantine`: a pending intent whose quarantine leaf is then
  removed by an "unknown writer" and replayed: the run exits 3, never reports
  completion, and the intent stays unfinished.
- `prune_replaced_quarantine`: a pending intent whose quarantine leaf is
  replaced by a different directory (same name, new inode): the replay exits 3,
  the replacement and its content are untouched, and the intent stays
  unfinished.

Limits, stated rather than implied: `oxide clean` is deliberately unchanged (it
removes exactly its enumerated plan, so new content is left and counted) and has
no case here. No run of this script has passed: its fit is UNMEASURED, and its
uncounted diagnostic and both counted author and independent reviewer natives
each need their own owner activation.

Diagnostic safety (`--diagnostic`): only a *recoverable oracle* failure is
recorded and followed by the next independent case. An unknown writer, device,
custody, detach or deadline outcome aborts the whole collection immediately,
retains every image backing and its custody record, and writes a bounded
`qualified: false` receipt with reached, completed, failed, skipped and
unexecuted cases plus the abort reason. `subprocess` timeouts are unknown
closure, never a recoverable failure.

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
4. automatic adoption: `workspace()` makes a real private Git repository whose
   `.gitignore` covers `target`, so the first conversion is eligible; each
   prune case then asserts the adopted owned root, the checkout's member link
   into it and the committed registry row before any race or prune.
5. Cargo external argv: not applicable, no external subcommand is invoked.
6. owned images: `ImageCustody` writes a custody record inside the guard's
   evidence directory before its first effect, creates the backing there,
   attaches it with `hdiutil attach -owners on`, parses the raw plist and
   requires exactly one mounted entity whose mount point is this run's expected
   `/Volumes/<name>` and whose device entry is a block device matching the
   mount's `st_dev`; detach is ordinary only, and the backing is removed only
   after the mount and device are positively gone.
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

import glob
import hashlib
import json
import os
import plistlib
import secrets
import shutil
import stat
import subprocess
import time

MIB = 1024 * 1024
BUILDER = "1.97.1"
POOL_BUDGET = 1 * MIB
IMAGE_MIB = 128
RESERVE = "16MiB"
IMAGE_CAP = "64MiB"
SHUTDOWN_MARGIN_SECONDS = 25
MUTED = ("OXIDE_", "XDG_", "CARGO_", "GIT_", "RUSTUP_", "RUSTC", "RUSTFLAGS")
TERMINAL_PHASES = {"deleted", "resolved"}
UNFINISHED_PHASES = {"intent", "renamed", "deleting", "marker_last"}
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
# The first conversion publishes owned member links, so the checkout has to be
# a real Git repository whose ignore rules cover them.
GITIGNORE = "/target\n"
CASES = (
    "lease_control",
    "lease_metadata",
    "lease_foreign",
    "lease_link",
    "lease_exhaust",
    "prune_metadata",
    "prune_foreign",
    "prune_exhaust",
    "prune_missing_quarantine",
    "prune_replaced_quarantine",
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
    "prune_missing_quarantine": "*:foreign",
    "prune_replaced_quarantine": "*:foreign",
}
COMPLETING = {"lease_control", "lease_metadata", "prune_metadata"}
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INVOKING_RUSTUP_HOME = os.environ.get("RUSTUP_HOME") or os.path.join(
    os.environ.get("HOME", ""), ".rustup"
)


class Recoverable(Exception):
    """An oracle failed. Only an independent case may follow."""


class Unknown(Exception):
    """Custody, device, writer or deadline uncertainty: abort, retain, report."""


def fail(message: str) -> None:
    raise Recoverable(message)


def unknown(message: str) -> None:
    raise Unknown(message)


def scratch_root() -> str:
    root = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not root:
        unknown("OXIDE_TASK_SCRATCH_ROOT is missing; run this through effigy")
    root = os.path.abspath(root)
    if not os.path.isdir(root):
        unknown(f"owned scratch {root} is not a directory")
    return root


def evidence_dir() -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    if not evidence:
        unknown("OXIDE_PROOF_EVIDENCE is missing; run this through effigy")
    evidence = os.path.abspath(evidence)
    if not os.path.isdir(evidence):
        unknown(f"evidence {evidence} is not a directory")
    facts = os.stat(evidence)
    if facts.st_uid != os.getuid() or facts.st_mode & 0o022:
        unknown(f"evidence {evidence} is not an owned, non-writable directory")
    return evidence


def work_left() -> float:
    """What is left of the supervisor's work window.

    `OXIDE_PROOF_DEADLINE_NS` is in the supervisor's own `CLOCK_UPTIME_RAW`
    domain, not wall time, so the remaining window is measured with the same
    clock the guard used. A spent window is unknown closure, not a failure.
    """
    deadline = int(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "0") or 0)
    if deadline <= 0:
        unknown("OXIDE_PROOF_DEADLINE_NS is missing; run this through effigy")
    left = (
        deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    ) / 1e9 - SHUTDOWN_MARGIN_SECONDS
    if left <= 5:
        unknown("the supervisor's work window is spent")
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
    bound = min(timeout, work_left())
    try:
        return subprocess.run(
            command,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=bound,
        )
    except subprocess.TimeoutExpired:
        # A child that outlived its bound has unknown closure: never continue.
        unknown(f"child {command[0]} timed out after {bound:.0f}s; closure is unknown")


def git(env: dict, cwd: str, *args: str):
    return run(
        [
            "git",
            "-c",
            "user.email=proof@oxide.invalid",
            "-c",
            "user.name=oxide proof",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            *args,
        ],
        cwd=cwd,
        env=env,
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


def workspace(env: dict, base: str, name: str) -> str:
    """A tiny private Git checkout whose ignore rule covers `target`."""
    path = os.path.join(base, "workspaces", name)
    os.makedirs(os.path.join(path, "src"), exist_ok=True)
    for relative, contents in CRATE.items():
        with open(os.path.join(path, relative), "w", encoding="utf-8") as handle:
            handle.write(contents)
    with open(os.path.join(path, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write(GITIGNORE)
    for argv in (("init", "-q"), ("add", "-A"), ("commit", "-q", "-m", "fixture")):
        result = git(env, path, *argv)
        if result.returncode:
            fail(f"cannot prepare the private Git fixture: {result.stderr[-800:]}")
    return path


def slot_leaves(pool: str) -> dict:
    """Every slot *leaf* (`<profile>-<n>`), never its project-label folder."""
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


def require_fired(case: str, mark: int, kind: str) -> None:
    if kind not in evidence_fired(mark):
        fail(f"{case}: the armed {kind} race never fired; the case proves nothing")


def registry(env: dict) -> tuple[str, dict]:
    """The internal live registry this fixture's private store wrote."""
    matches = glob.glob(
        os.path.join(env["HOME"], "**", "outputs", "registry.json"), recursive=True
    )
    if len(matches) != 1:
        fail(f"the fixture left {len(matches)} registries, expected one: {matches}")
    path = matches[0]
    try:
        with open(path, encoding="utf-8") as handle:
            document = json.load(handle)
    except Exception as error:  # noqa: BLE001 - one bounded failure line
        fail(f"the fixture registry is unreadable ({error!r})")
    if document.get("complete") is not True:
        fail("the fixture registry is not complete")
    return path, document


def intent_for(env: dict, leaf: str) -> tuple[dict, dict]:
    _, document = registry(env)
    intents = [
        intent
        for intent in document.get("intents") or []
        if intent.get("quarantine") == leaf
    ]
    if len(intents) != 1:
        fail(f"the registry holds {len(intents)} intents for {leaf}, expected one")
    return document, intents[0]


def lease_case(oxide: str, root: str, case: str, race: str | None) -> str:
    env, base = case_env(root, case, race)
    mark = evidence_mark()
    first = run([oxide, "check"], cwd=workspace(env, base, "a"), env=env)
    if first.returncode:
        fail(f"{case}: the first admission failed: {first.stderr[-1500:]}")
    before = slot_leaves(env["OXIDE_POOL"])
    if len(before) != 1:
        fail(
            f"{case}: the first admission left {len(before)} slot leaves, expected one: {sorted(before)}"
        )
    victim = next(iter(before))
    second = run([oxide, "check"], cwd=workspace(env, base, "b"), env=env)
    after = slot_leaves(env["OXIDE_POOL"])
    status = run([oxide, "status"], cwd=base, env=env)
    pending = "recorded deletion pending reconciliation" in status.stdout

    if case == "lease_control" or case == "lease_metadata":
        if second.returncode:
            fail(f"{case}: the eviction did not complete: {second.stderr[-1500:]}")
        if victim in after:
            fail(f"{case}: the evicted slot leaf {victim} is still present")
        if pending:
            fail(f"{case}: a deletion stayed pending after a complete eviction")
        if race is not None:
            require_fired(case, mark, "Metadata")
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
    require_fired(
        case,
        mark,
        {"lease_foreign": "Foreign", "lease_link": "Link", "lease_exhaust": "Metadata"}[case],
    )
    return f"{case}: pending, victim {victim}, exit {second.returncode}"


class ImageCustody:
    """One owned sparse image whose backing and custody record live in the
    guard's evidence directory, written before the first effect."""

    def __init__(self, case: str):
        self.case = case
        directory = os.path.join(evidence_dir(), "images")
        os.makedirs(directory, mode=0o700, exist_ok=True)
        facts = os.stat(directory)
        if facts.st_uid != os.getuid() or facts.st_mode & 0o077:
            unknown(f"image custody directory {directory} is not owner-only")
        self.directory = directory
        self.name = f"oxide155-{case}-{secrets.token_hex(4)}"
        self.backing = os.path.join(directory, f"{self.name}.sparseimage")
        self.record = os.path.join(directory, f"{self.name}-custody.json")
        self.expected_mount = f"/Volumes/{self.name}"
        self.mount = None
        self.device_entry = None
        self.device = None
        self.state = {"case": case, "backing": self.backing, "phases": []}
        self.write("declared", expected_mount=self.expected_mount)

    def write(self, phase: str, **facts) -> None:
        self.state["phases"].append({"phase": phase, **facts})
        with open(self.record, "w", encoding="utf-8") as handle:
            json.dump(self.state, handle, indent=2, sort_keys=True)
            handle.write("\n")

    def attach(self) -> str:
        created = run(
            [
                "hdiutil",
                "create",
                "-size",
                f"{IMAGE_MIB}m",
                "-fs",
                "APFS",
                "-type",
                "SPARSE",
                "-quiet",
                self.backing,
            ],
            cwd=self.directory,
            env=scrubbed(os.environ),
            timeout=60,
        )
        if created.returncode:
            unknown(f"{self.case}: cannot create the owned image: {created.stderr[-800:]}")
        facts = os.lstat(self.backing)
        if not stat.S_ISREG(facts.st_mode) or facts.st_uid != os.getuid():
            unknown(f"{self.case}: the created backing is not an owned regular file")
        self.write("created", size=facts.st_size, inode=facts.st_ino, device=facts.st_dev)
        attached = run(
            [
                "hdiutil",
                "attach",
                "-owners",
                "on",
                "-nobrowse",
                "-plist",
                self.backing,
            ],
            cwd=self.directory,
            env=scrubbed(os.environ),
            timeout=60,
        )
        if attached.returncode:
            unknown(f"{self.case}: cannot attach the owned image: {attached.stderr[-800:]}")
        self.write(
            "attach-raw",
            plist_sha256=hashlib.sha256(attached.stdout.encode()).hexdigest(),
        )
        try:
            document = plistlib.loads(attached.stdout.encode())
        except Exception as error:  # noqa: BLE001 - custody is unknown here
            unknown(f"{self.case}: the attach plist is unreadable ({error!r}); retained")
        mounted = [
            entity
            for entity in document.get("system-entities", [])
            if entity.get("mount-point") and entity.get("dev-entry")
        ]
        if len(mounted) != 1:
            unknown(
                f"{self.case}: the attach returned {len(mounted)} mounted entities, expected one; retained"
            )
        mount = mounted[0]["mount-point"]
        entry = mounted[0]["dev-entry"]
        if mount != self.expected_mount:
            unknown(
                f"{self.case}: the attach mounted {mount}, not the expected {self.expected_mount}; retained"
            )
        try:
            mount_facts = os.stat(mount)
            entry_facts = os.stat(entry)
        except OSError as error:
            unknown(f"{self.case}: cannot bind the mount to its device ({error}); retained")
        if not stat.S_ISBLK(entry_facts.st_mode):
            unknown(f"{self.case}: the mount's device entry {entry} is not a block device; retained")
        if entry_facts.st_rdev != mount_facts.st_dev:
            unknown(
                f"{self.case}: the mount {mount} is device {mount_facts.st_dev}, "
                f"not its recorded device entry {entry} ({entry_facts.st_rdev}); retained"
            )
        self.mount, self.device_entry, self.device = mount, entry, entry_facts.st_rdev
        self.write(
            "attached",
            mount=mount,
            device_entry=entry,
            device=self.device,
            mount_mode=oct(mount_facts.st_mode),
        )
        return mount

    def detach(self) -> None:
        """Ordinary detach only, then positive device/mount absence, then the
        backing. Never forced, never retried."""
        if self.mount is None:
            self.write("detach-unnecessary")
            return
        attached = os.path.isdir(self.mount) and os.path.exists(self.device_entry)
        result = run(
            ["hdiutil", "detach", self.mount],
            cwd="/",
            env=scrubbed(os.environ),
            timeout=60,
        )
        if result.returncode:
            self.write("detach-failed", stderr=result.stderr[-400:], retained=True)
            unknown(f"{self.case}: the ordinary detach of {self.mount} failed; retained")
        if os.path.exists(self.mount) or os.path.exists(self.device_entry):
            self.write("detach-unproven", retained=True)
            unknown(
                f"{self.case}: the mount or its device entry is still present after detach; retained"
            )
        self.write("detached", previously_attached=attached)
        os.unlink(self.backing)
        self.write("backing-removed")

    def retain(self, reason: str) -> None:
        self.write("retained", reason=reason[-400:])


def adopt_and_assert(oxide: str, env: dict, checkout: str, mount: str) -> tuple:
    """Build the fresh Git checkout and assert the conversion really happened."""
    built = run([oxide, "check"], cwd=checkout, env=env)
    if built.returncode:
        fail(f"the fixture build on the drive failed: {built.stderr[-1500:]}")
    outputs = os.path.join(mount, "oxide-outputs")
    if not os.path.isdir(outputs):
        fail("the first build published no owned outputs tree on the drive")
    roots = [
        entry
        for entry in sorted(os.listdir(outputs))
        if os.path.isdir(os.path.join(outputs, entry)) and not entry.startswith(".prune-")
    ]
    if len(roots) != 1:
        fail(f"the build left {len(roots)} owned roots on the drive, expected one: {roots}")
    root_path = os.path.join(outputs, roots[0])
    link = os.path.join(checkout, "target")
    if not os.path.islink(link):
        fail(f"the converted checkout has no owned member link at {link}")
    target = os.readlink(link)
    if not target.startswith(root_path):
        fail(f"the member link {link} points at {target}, not the owned root {root_path}")
    _, document = registry(env)
    rows = json.dumps(document.get("rows") or [])
    if root_path not in rows:
        fail(f"the committed registry names no row for the adopted root {root_path}")
    return outputs, root_path


def preview_plan(oxide: str, env: dict, base: str) -> tuple[str, str]:
    preview = run([oxide, "outputs", "prune", "--dry-run"], cwd=base, env=env)
    if preview.returncode:
        fail(f"the prune dry run refused: {preview.stderr[-1500:]}")
    digest = ""
    for line in preview.stderr.splitlines():
        for word in line.replace(":", " ").split():
            if len(word) == 64 and all(c in "0123456789abcdef" for c in word):
                digest = word
    if not digest:
        fail(f"the dry run printed no plan digest: {preview.stderr[-800:]}")
    plan = os.path.join(base, "plan.json")
    with open(plan, "w", encoding="utf-8", newline="") as handle:
        handle.write(preview.stdout)
    return plan, digest


def prune_case(oxide: str, root: str, case: str, race: str | None) -> str:
    env, base = case_env(root, case, race)
    mark = evidence_mark()
    custody = ImageCustody(case)
    try:
        mount = custody.attach()
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
        shutil.copytree(workspace(env, base, "a"), checkout)
        outputs, root_path = adopt_and_assert(oxide, env, checkout, mount)
        # The checkout must be positively gone before the root is eligible.
        shutil.rmtree(checkout)
        if os.path.exists(checkout):
            fail(f"{case}: the fixture checkout was not removed")
        plan, digest = preview_plan(oxide, env, base)
        real = run(
            [oxide, "outputs", "prune", "--plan", plan, "--plan-sha256", digest],
            cwd=base,
            env=env,
        )
        quarantined = sorted(
            entry for entry in os.listdir(outputs) if entry.startswith(".prune-")
        )
        kept_root = os.path.isdir(root_path)
        result = settle_case(
            oxide,
            env,
            base,
            case,
            mark,
            real,
            outputs,
            root_path,
            quarantined,
            kept_root,
            plan,
            digest,
        )
        custody.detach()
        return result
    except Unknown as error:
        custody.retain(f"unknown outcome: {error}")
        raise
    except Recoverable as error:
        # A recoverable oracle failure still means this fixture's image has to
        # leave cleanly, or its absence is unknown: treat a failed detach as
        # unknown.
        try:
            custody.detach()
        except Unknown as detach_error:
            custody.retain(f"after a recoverable failure: {detach_error}")
            raise detach_error from error
        raise


def settle_case(
    oxide: str,
    env: dict,
    base: str,
    case: str,
    mark: int,
    real,
    outputs: str,
    root_path: str,
    quarantined: list,
    kept_root: bool,
    plan: str,
    digest: str,
) -> str:
    if case == "prune_metadata":
        if real.returncode:
            fail(f"{case}: the prune did not complete: {real.stderr[-1500:]}")
        if quarantined:
            fail(f"{case}: a quarantined leaf was left behind: {quarantined}")
        if kept_root:
            fail(f"{case}: the pruned root is still present")
        _, document = registry(env)
        if json.dumps(document.get("intents") or []).find(root_path) >= 0:
            fail(f"{case}: the live registry still holds an intent for {root_path}")
        if root_path in json.dumps(document.get("rows") or []):
            fail(f"{case}: the live registry still selects the pruned root {root_path}")
        require_fired(case, mark, "Metadata")
        return f"{case}: deleted, exit {real.returncode}"

    if real.returncode != 3:
        fail(
            f"{case}: an unsettled deletion did not exit 3 "
            f"(exit {real.returncode}: {real.stderr[-1500:]})"
        )
    if not quarantined:
        fail(f"{case}: the pending intent left no quarantined leaf")
    if kept_root:
        fail(f"{case}: the root is still under its live name while a quarantine exists")
    leaf = os.path.join(outputs, quarantined[0])
    document, intent = intent_for(env, leaf)
    phase = intent.get("phase")
    if phase not in UNFINISHED_PHASES:
        fail(f"{case}: the journal phase is {phase!r}, not an unfinished one")
    if intent.get("receipt_settled") is not False:
        fail(f"{case}: the intent claims a settled receipt: {intent.get('receipt_settled')}")
    if phase in TERMINAL_PHASES:
        fail(f"{case}: the journal reached a terminal phase {phase!r}")

    if case == "prune_foreign":
        if not [
            path for path in tree_files(leaf) if os.path.basename(path) == "foreign.txt"
        ]:
            fail(f"{case}: the foreign entry was deleted instead of kept")
        require_fired(case, mark, "Foreign")
        return f"{case}: pending, quarantine {quarantined[0]}, phase {phase}"

    if case == "prune_exhaust":
        require_fired(case, mark, "Metadata")
        return f"{case}: pending, quarantine {quarantined[0]}, phase {phase}"

    # The two replay cases start from this pending intent and mutate its
    # quarantine the way an unknown writer would.
    if case == "prune_missing_quarantine":
        shutil.rmtree(leaf)
        if os.path.exists(leaf):
            fail(f"{case}: cannot remove the quarantine for this control")
    elif case == "prune_replaced_quarantine":
        moved = leaf + ".moved"
        os.rename(leaf, moved)
        os.makedirs(leaf)
        with open(os.path.join(leaf, "replacement.txt"), "w", encoding="utf-8") as handle:
            handle.write("replacement\n")
    else:
        fail(f"{case}: unknown prune case")

    replay = run(
        [oxide, "outputs", "prune", "--plan", plan, "--plan-sha256", digest],
        cwd=base,
        env=env,
    )
    if replay.returncode != 3:
        fail(
            f"{case}: a replay over a mutated quarantine did not exit 3 "
            f"(exit {replay.returncode}: {replay.stderr[-1500:]})"
        )
    document, intent = intent_for(env, leaf)
    phase = intent.get("phase")
    if phase in TERMINAL_PHASES:
        fail(f"{case}: the replay declared completion ({phase!r})")
    if case == "prune_replaced_quarantine":
        if not os.path.isfile(os.path.join(leaf, "replacement.txt")):
            fail(f"{case}: the replacement directory or its content was removed")
    return f"{case}: pending after replay, phase {phase}"


def plan() -> int:
    print("cases:")
    for case in CASES:
        race = RACES[case]
        print(f"  {case:<26} race {race or 'none (control)'}")
    print(
        "allocation: 5 internal 1 MiB directory pools on the boot Data volume "
        "(the selector's internal-placement exception), one private HOME/CARGO_HOME "
        "and TMPDIR per case, 5 owned 128 MiB sparse images with custody records in "
        "the guard's evidence directory, one attached at a time and ordinarily "
        "detached, staged debug oxide in owned scratch"
    )
    print("premises: 9 traced in this script's docstring")
    print("diagnostic: recoverable failures continue; unknown custody/device/writer/deadline aborts")
    return 0


def diagnostic_receipt(reached, completed, failed, skipped, unexecuted, abort) -> str:
    receipt = {
        "proof": "oxide#155-finder-metadata-race",
        "qualified": False,
        "mode": "diagnostic",
        "reached": reached,
        "completed": completed,
        "failed": failed,
        "skipped": skipped,
        "unexecuted": unexecuted,
        "aborted": abort is not None,
        "abort": abort,
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

    reached, completed, failed, skipped = [], [], [], []
    abort = None
    for index, case in enumerate(CASES):
        reached.append(case)
        try:
            line = (
                lease_case(oxide, root, case, RACES[case])
                if case.startswith("lease_")
                else prune_case(oxide, root, case, RACES[case])
            )
            completed.append(case)
            print(line, flush=True)
        except Unknown as error:
            abort = f"{case}: {type(error).__name__}: {error}"
            skipped.extend(CASES[index + 1 :])
            break
        except Recoverable as error:
            text = f"{type(error).__name__}: {error}"
            if not diagnostic:
                raise
            failed.append(case)
            print(f"{case}: FAILED {text}", flush=True)
        except Exception as error:  # noqa: BLE001 - unknown outcome, never continued
            abort = f"{case}: unexpected {type(error).__name__}: {error}"
            skipped.extend(CASES[index + 1 :])
            break

    if diagnostic:
        receipt = diagnostic_receipt(
            reached,
            completed,
            failed,
            skipped,
            [case for case in CASES if case not in reached],
            abort,
        )
        print(
            f"diagnostic: qualified=false, reached {len(reached)}, completed "
            f"{len(completed)}, failed {len(failed)}, skipped {len(skipped)}, "
            f"aborted {abort is not None}, receipt {receipt}",
            flush=True,
        )
        return 0 if not failed and abort is None else 1
    if abort is not None:
        raise Unknown(abort)
    print(f"proof: {len(completed)} cases passed", flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Unknown as error:
        print(f"proof aborted (unknown outcome, nothing continued): {error}", file=sys.stderr, flush=True)
        sys.exit(1)
    except Recoverable as error:
        print(f"proof failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
    except Exception as error:  # noqa: BLE001 - one bounded failure line
        print(f"proof failed (unexpected): {type(error).__name__}: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
