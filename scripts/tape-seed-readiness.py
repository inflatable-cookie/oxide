#!/usr/bin/env python3
"""Seed-only readiness control for the observation's two Cargo.lock inputs (oxide#294).

One fixed orchestration command that seeds the Oxide and the Barge lock, one
after the other, from one EXPLICIT seed source. Nothing else runs: no Cargo, no
resolution, no staging build, no engine, no observation and no image. The shared
bounded seeder (`scripts/offline_seed.py`) is the only code that reads the host
cache, and it reads it through descriptors and private copies only.

- Each lock's bytes are read from its bound commit, read-only, and authenticated
  before any seed: the commit's tree, the `Cargo.lock` blob and its SHA-256 must
  all be the bound values. The Barge side also binds its reviewed source commit
  to the same tree, so an identical-tree merge is the only accepted alias.
- Both seeds share ONE absolute deadline: the guard's own deadline less a fixed
  30 s shutdown margin. Each keeps the seeder's own 45 s cumulative work cap and
  its own limits, and nothing is reset between the two.
- The second lock runs only when the first ended with a clean outcome and the
  shared window is still open. An unknown-closure first outcome keeps the
  second lock not-run, with the reason named.
- Every seed writes into its own fresh private destination inside the guard's
  owned run root. The explicit host source is read, never written.

The report records, per lock: the bound identities, the attempt status
(completed, refused or not-run), the seeder's manifest summary or its refusal,
the seeder's provenance (requested source, opened identities, the first failed
component, operation and errno), elapsed time and the candidate commit facts.
`qualified` is false unconditionally. A seed that completes proves current copy
and verification facts only: not the historical cause of any earlier refusal,
Cargo sparse-index acceptance, a Git namespace choice, compilation, build fit,
an observation, a comparison or a qualification. Guard closure is judged by the
guard's post-leader census, never by this script.

This file is source preparation. `--plan` and `--self-test` are effect-free
(the self-test writes nothing outside a temporary directory it removes), and
`--control` refuses before any effect until a planner binds one occurrence.

Usage:
    tape-seed-readiness.py --plan
    tape-seed-readiness.py --self-test
    tape-seed-readiness.py --control --binding <path> --sha256 <hex>
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(SCRIPTS)
SCHEMA = "oxide.capture.seed-readiness/1"
BINDING_SCHEMA = "oxide.capture.seed-readiness-binding/1"
SUPERVISOR_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
SELECTED_VOLUME_UUID = "8A660440-C919-45C6-A3F1-3365B9BD2006"
LIMIT_SECONDS = 180
SCRATCH_MIB = 512
SHUTDOWN_MARGIN_SECONDS = 30
PER_SEED_WORK_SECONDS = 45
REPORT_NAME = "seed-readiness.json"
SIDES = ("oxide", "barge")
# The Oxide lock digest this preparation was reviewed against. The bound commit's
# blob must match the binding; the digest here is a plan-time reference only.
OXIDE_REFERENCE_LOCK_SHA256 = "e7dcf4647e41e3a1b5449e107ac1dac9a1da98076a868a10b1bdfeafa6f37b28"
CLAIM = (
    "none: current seed copy and verification facts for two locks; no historical "
    "cause, Cargo sparse-index acceptance, Git namespace choice, compilation, build "
    "fit, observation, comparison or qualification"
)
GIT_TIMEOUT_S = 10
GIT_OUTPUT_CAP = 64 * 1024
GIT_ENV = {
    "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_TERMINAL_PROMPT": "0",
    "GIT_NO_LAZY_FETCH": "1",
    "GIT_OPTIONAL_LOCKS": "0",
}
USAGE = (
    "usage: tape-seed-readiness.py [--plan | --self-test | --control --binding <path> --sha256 <hex>]"
)


def _load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, os.path.join(SCRIPTS, filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The observation driver's binding and seam helpers are reused, not copied.
obs = _load("tape_embedded_observation", "tape-embedded-observation.py")
import offline_seed  # noqa: E402  (the shared seeder, on sys.path beside this file)

# The reviewed Barge custody source, its identical-tree merge and its lock digest are
# the observation's own record (never copied here). The lock blob is the one value
# this control adds: `git rev-parse 05b4b52:Cargo.lock`, checked read-only at authoring.
BARGE_REFERENCE = {
    "reviewed_source": obs.BARGE_CUSTODY_REVIEWED["review_head"],
    "commit": obs.BARGE_CUSTODY_REVIEWED["runtime_commit"],
    "tree": obs.BARGE_CUSTODY_REVIEWED["tree"],
    "lock_blob": "e188bee6c4d641a707492d22d7443eab0eda10a7",
    "lock_sha256": obs.BARGE_CUSTODY_REVIEWED["lock_sha256"],
}

SHUTDOWN_NS = SHUTDOWN_MARGIN_SECONDS * 1_000_000_000

# The nine proof-authoring premises (workflow.md checklist), each with the source
# site that applies it or the reason it does not apply to a seed-only control.
PREMISES = (
    ("private HOME and rustup", "not-applicable: no Cargo, rustc or toolchain child runs. "
     "Seed Git children get their own private HOME (offline_seed `_run_git`, seed-home under the "
     "destination) and ReadinessSeams.git_bytes uses GIT_ENV, which sets no HOME."),
    ("account-home control", "not-applicable: no Oxide product child runs; the seeder and the git "
     "queries never read the account home."),
    ("environment isolation", "applicable: ReadinessSeams.git_bytes runs git with GIT_ENV only "
     "(fixed PATH, GIT_CONFIG_NOSYSTEM, GIT_CONFIG_GLOBAL=/dev/null, no terminal prompt, no lazy "
     "fetch, no optional locks); offline_seed `_run_git` sanitizes its own environment the same way."),
    ("automatic adoption", "not-applicable: no product checkout, target or `oxide` build runs."),
    ("Cargo external argv", "not-applicable: no Cargo subcommand is invoked."),
    ("owned images", "not-applicable: no image is attached; the owned root is the guard's scratch "
     "and evidence siblings, checked by obs.guard_context."),
    ("compiler flags", "not-applicable: no staging build runs, so no RUSTFLAGS or encoded flags "
     "are set; the held staging path keeps its own frozen flags."),
    ("temporary storage", "applicable: the run root is derive_paths(...)['run_root'] under the "
     "guard's selected scratch root; the self-test's real-seed fixtures use tempfile with "
     "dir=OXIDE_TASK_SCRATCH_ROOT; the report goes to the guard's evidence directory."),
    ("macOS mktemp", "applicable: the self-test calls tempfile.mkdtemp with an explicit dir, never "
     "a bare mktemp, so the fixture location is the supplied scratch root and nothing reads TMPDIR "
     "for it."),
)


def say(text: str) -> None:
    print(text, flush=True)


# ---------------------------------------------------------------------------
# Binding
# ---------------------------------------------------------------------------


def parse_binding(payload: bytes) -> dict:
    """One planner-bound binding document. Every field is exact; nothing absolute
    is taken for an output path, and the seed source is named explicitly."""
    try:
        document = json.loads(payload)
    except ValueError as error:
        raise obs.Obstruction(f"the binding document is not JSON: {error}")
    if not isinstance(document, dict) or document.get("schema") != BINDING_SCHEMA:
        raise obs.Obstruction("the binding document is not the current schema")
    if not obs.safe_component(document.get("planner_reference")):
        raise obs.Obstruction("the binding names no planner reference")
    seed_source = document.get("seed_source")
    if not isinstance(seed_source, str) or not os.path.isabs(seed_source):
        raise obs.Obstruction("the binding's seed source is not an absolute path")
    for side in SIDES:
        entry = document.get(side)
        if not isinstance(entry, dict):
            raise obs.Obstruction(f"the binding has no {side} object")
        for key, length in (("commit", 40), ("tree", 40), ("lock_blob", 40), ("lock_sha256", 64)):
            if not obs.is_hex(entry.get(key), length):
                raise obs.Obstruction(f"the binding's {side} {key} is not exact")
    barge = document["barge"]
    if not isinstance(barge.get("sibling"), str) or not os.path.isabs(barge["sibling"]):
        raise obs.Obstruction("the binding's Barge sibling is not an absolute path")
    if not obs.is_hex(barge.get("reviewed_source"), 40):
        raise obs.Obstruction("the binding's reviewed Barge source is not exact")
    guard = document.get("guard")
    if not isinstance(guard, dict):
        raise obs.Obstruction("the binding has no guard object")
    if guard.get("supervisor_sha256") != SUPERVISOR_SHA256:
        raise obs.Obstruction("the binding does not name the qualified supervisor")
    if guard.get("limit_seconds") != LIMIT_SECONDS or guard.get("scratch_mib") != SCRATCH_MIB:
        raise obs.Obstruction("the binding's guard bounds are not this route's fixed bounds")
    volume = document.get("volume")
    if not isinstance(volume, dict) or volume.get("uuid") != SELECTED_VOLUME_UUID:
        raise obs.Obstruction("the binding does not name the selected Stash volume")
    paths = document.get("paths")
    if not isinstance(paths, dict) or not obs.safe_component(paths.get("run_root_name")):
        raise obs.Obstruction("the binding's run root name is not one safe component")
    if not obs.safe_component(paths.get("report_name")):
        raise obs.Obstruction("the binding's report name is not one safe component")
    return document


def load_binding(seams, options: dict) -> tuple[dict, bytes]:
    path, expected = options.get("binding"), options.get("sha256")
    if not path or not expected:
        raise obs.Obstruction("one binding path and its digest are both required")
    if not obs.is_hex(expected, 64):
        raise obs.Obstruction("the supplied binding digest is not a SHA-256")
    try:
        payload = seams.read_bytes(path)
    except OSError as error:
        raise obs.Obstruction(f"the binding document is unreadable: {error}")
    actual = obs.sha256_bytes(payload)
    if actual != expected:
        raise obs.Obstruction(f"the binding document digest is {actual}, not the supplied {expected}")
    return parse_binding(payload), payload


# ---------------------------------------------------------------------------
# Authenticated lock bytes (read-only git objects)
# ---------------------------------------------------------------------------


def locked_bytes(seams, repo: str, entry: dict, side: str) -> bytes:
    """The lock at the bound commit, authenticated before any seed. Read-only
    object queries only: rev-parse and cat-file, no fetch, no hook, no write."""
    tree = seams.git_text(repo, "rev-parse", "--verify", f"{entry['commit']}^{{tree}}")
    if tree != entry["tree"]:
        raise obs.Obstruction(f"the {side} commit's tree is {tree}, not the bound {entry['tree']}")
    blob = seams.git_text(repo, "rev-parse", "--verify", f"{entry['commit']}:Cargo.lock")
    if blob != entry["lock_blob"]:
        raise obs.Obstruction(f"the {side} Cargo.lock blob is {blob}, not the bound {entry['lock_blob']}")
    payload = seams.git_bytes(repo, "cat-file", "blob", blob)
    actual = obs.sha256_bytes(payload)
    if actual != entry["lock_sha256"]:
        raise obs.Obstruction(f"the {side} Cargo.lock digest is {actual}, not the bound {entry['lock_sha256']}")
    return payload


def verify_barge_reviewed(seams, binding: dict) -> None:
    """The reviewed custody source and the bound commit must share the bound tree."""
    barge = binding["barge"]
    reviewed = seams.git_text(barge["sibling"], "rev-parse", "--verify",
                              f"{barge['reviewed_source']}^{{tree}}")
    if reviewed != barge["tree"]:
        raise obs.Obstruction(
            f"the reviewed Barge source tree is {reviewed}, not the bound {barge['tree']}")


# ---------------------------------------------------------------------------
# Seams: real effects, and the fakes the self-test substitutes
# ---------------------------------------------------------------------------


class ReadinessSeams(obs.Seams):
    """Real effects: read-only git queries and the shared seeder. The clock is
    CLOCK_UPTIME_RAW, the clock the guard's deadline and the seeder both use."""

    def now_ns(self):
        return offline_seed.now_ns()

    def git_bytes(self, repo, *args):
        argv = ["git", "-C", repo, *args]
        try:
            done = subprocess.run(argv, env=dict(GIT_ENV), stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, timeout=GIT_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            raise obs.Obstruction(f"git {args[0]} did not finish within {GIT_TIMEOUT_S} s")
        if done.returncode != 0:
            raise obs.Obstruction(f"git {args[0]} failed in {repo}")
        if len(done.stdout) > (GIT_OUTPUT_CAP if args[0] != "cat-file" else 4 * 1024 * 1024):
            raise obs.Obstruction(f"git {args[0]} output exceeds its bound")
        return done.stdout

    def git_text(self, repo, *args):
        return self.git_bytes(repo, *args).decode("utf-8", "replace").strip()


def seed_summary(seed: dict) -> dict:
    """Counts, identities and one digest of the file manifest; the file lists
    themselves stay in the seeder's manifest, never in this report."""
    candidates = [c for source in seed["git_sources"] for c in source["candidates"]]
    manifest = list(seed["files"]) + [f for c in candidates for f in c["files"]]
    digest = hashlib.sha256(json.dumps(manifest, sort_keys=True).encode()).hexdigest()
    return {
        "lock_sha256": seed["lock_sha256"],
        "packages": seed["packages"],
        "aggregate_bytes": seed["aggregate_bytes"],
        "entries": seed["entries"],
        "manifest_bytes": seed["manifest_bytes"],
        "registry_files": len(seed["files"]),
        "manifest_sha256": digest,
        "git": [
            {
                "repository": source["repository"],
                "rev": source["rev"],
                "candidates": [
                    {
                        "database": c["database"],
                        "copied": c["copied"],
                        "commit_present": c["commit_present"],
                        "rejected": c["rejected"] is not None,
                        "skipped": c["skipped"] is not None,
                        "bytes": c["bytes"],
                        "filecount": c["filecount"],
                    }
                    for c in source["candidates"]
                ],
            }
            for source in seed["git_sources"]
        ],
        "limits": seed["limits"],
    }


def tree_signature(root: str) -> list:
    """Every entry under `root` with its mode, size, inode and both times. A read
    leaves ctime alone; a write, chmod, rename or link changes it."""
    rows = []
    for directory, dirnames, filenames in os.walk(root):
        for name in sorted(dirnames + filenames):
            path = os.path.join(directory, name)
            info = os.lstat(path)
            rows.append((os.path.relpath(path, root), info.st_mode, info.st_size,
                         info.st_ino, info.st_mtime_ns, info.st_ctime_ns))
    return rows


def closure_unknown(provenance: dict) -> bool:
    return (provenance.get("refusal") or {}).get("closure") == "unknown"


# ---------------------------------------------------------------------------
# The one occurrence
# ---------------------------------------------------------------------------


def lock_entry(side: str, binding: dict) -> dict:
    fields = binding[side]
    return {
        "side": side,
        "commit": fields["commit"],
        "tree": fields["tree"],
        "lock_blob": fields["lock_blob"],
        "lock_sha256": fields["lock_sha256"],
        "status": "not-run",
        "reason": None,
        "closure": None,
        "elapsed_seconds": None,
        "destination": None,
        "provenance": {},
        "seed": None,
    }


def run_locks(binding: dict, paths: dict, seams, deadline_ns: int) -> list[dict]:
    """Seed both locks in order, each into its own private destination, under
    one shared deadline. The second lock waits for a clean first outcome."""
    entries = []
    repos = {"oxide": REPO, "barge": binding["barge"]["sibling"]}
    for index, side in enumerate(SIDES):
        entry = lock_entry(side, binding)
        entries.append(entry)
        if index:
            first = entries[0]
            if first["closure"] == "unknown":
                entry["reason"] = "the first lock's closure is unknown; the second is not run"
                continue
            if seams.now_ns() >= deadline_ns:
                entry["reason"] = "the shared seed window closed before this lock; not run"
                continue
        destination = os.path.join(paths["run_root"], f"seed-{side}", "cargo-home")
        entry["destination"] = destination
        started = seams.now_ns()
        try:
            lock_bytes = locked_bytes(seams, repos[side], binding[side], side)
        except obs.Obstruction as error:
            entry.update(status="refused", reason=str(error))
            entry["elapsed_seconds"] = round((seams.now_ns() - started) / 1e9, 3)
            continue
        seams.make_dirs(os.path.dirname(destination))
        provenance = entry["provenance"]
        try:
            seed = seams.seed(binding["seed_source"], destination, lock_bytes, deadline_ns,
                              provenance=provenance)
        except obs.Obstruction as error:
            entry.update(status="refused", reason=str(error),
                         closure="unknown" if closure_unknown(provenance) else "clean")
        else:
            entry.update(status="completed", seed=seed_summary(seed), closure="clean")
        entry["elapsed_seconds"] = round((seams.now_ns() - started) / 1e9, 3)
    return entries


def control(options: dict, seams=None, environ=None) -> int:
    """Run the one control. Returns the process status: 0 when both locks completed
    and the report was published; 1 when the report was published but a lock did
    not complete; 2 when the control refused before any effect or could not
    publish its report."""
    seams = seams or ReadinessSeams()
    report = None
    try:
        binding, payload = load_binding(seams, options)
        guard = obs.guard_context(seams, environ)
        paths = obs.derive_paths(binding, guard)
        deadline_ns = guard["deadline_ns"] - SHUTDOWN_NS
        if seams.exists(paths["run_root"]) or seams.exists(paths["report"]):
            raise obs.Obstruction("this occurrence's root or report already exists; refusing")
        if seams.now_ns() >= deadline_ns:
            raise obs.Obstruction("the guard's deadline leaves no seed window")
        verify_barge_reviewed(seams, binding)
        seams.make_root(paths["run_root"])
    except obs.Obstruction as error:
        say(f"seed-readiness: refused before any effect: {error}")
        return 2

    report = {
        "schema": SCHEMA,
        "qualified": False,
        "claim": CLAIM,
        "planner_reference": binding["planner_reference"],
        "binding_sha256": obs.sha256_bytes(payload),
        "seed_source": {"requested": binding["seed_source"], "role": obs.SEED_ROLE},
        "bounds": {"limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB,
                   "shutdown_margin_seconds": SHUTDOWN_MARGIN_SECONDS,
                   "per_seed_work_seconds": PER_SEED_WORK_SECONDS,
                   "fit": "unmeasured"},
        "guard": {"run_id": guard["run_id"], "scratch_root": guard["scratch_root"],
                  "evidence": guard["evidence"]},
        "locks": [],
        "outcome": None,
        "closure": "judged by the guard's post-leader census; this control never concludes it",
    }
    state = "completed"
    try:
        report["locks"] = run_locks(binding, paths, seams, deadline_ns)
    except obs.Obstruction as error:
        state = "refused"
        report["gaps"] = [str(error)]
    except Exception as error:  # the report is still published before propagating
        report["outcome"] = {"state": "aborted", "locks": {}}
        report["gaps"] = [f"{type(error).__name__}: {error}"]
        publish(seams, paths, report, "aborted")
        raise
    statuses = {entry["side"]: entry["status"] for entry in report["locks"]}
    if state == "completed" and any(statuses.get(side) != "completed" for side in SIDES):
        state = "incomplete"
    report["outcome"] = {"state": state, "locks": statuses}
    if not publish(seams, paths, report, state):
        return 2
    return 0 if state == "completed" else 1


def publish(seams, paths: dict, report: dict, state: str) -> bool:
    try:
        seams.write_json(paths["report"], report)
    except OSError as error:
        say(f"seed-readiness: the report could not be published: {error}")
        return False
    say(f"seed-readiness: {state} report {paths['report']}; qualified=false")
    return True


# ---------------------------------------------------------------------------
# Plan
# ---------------------------------------------------------------------------


def plan_document() -> dict:
    return {
        "schema": SCHEMA,
        "mode": "plan (effect-free)",
        "selector": "proof:seed-readiness:control (heavy, one occurrence after activation)",
        "command": (
            "bounded-test.py --limit 180 --scratch-budget-mib 512 -- python3 "
            "scripts/tape-seed-readiness.py --control --binding \"$SEED_READINESS_BINDING\" "
            "--sha256 \"$SEED_READINESS_BINDING_SHA256\""
        ),
        "bounds": {"limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB,
                   "shutdown_margin_seconds": SHUTDOWN_MARGIN_SECONDS,
                   "shared_window_seconds": LIMIT_SECONDS - SHUTDOWN_MARGIN_SECONDS,
                   "per_seed_work_seconds": PER_SEED_WORK_SECONDS,
                   "sequence": "oxide lock, then barge lock, one shared deadline",
                   "fit": "unmeasured"},
        "seed_source": {"input": "absolute binding seed_source (named explicitly, seed source only)",
                        "role": obs.SEED_ROLE},
        "locks": {
            "oxide": {"lock_sha256_reference": OXIDE_REFERENCE_LOCK_SHA256,
                      "authenticated_by": "bound commit tree, Cargo.lock blob and SHA-256"},
            "barge": dict(BARGE_REFERENCE,
                          authenticated_by="reviewed source tree equals the bound tree; "
                                           "commit tree, Cargo.lock blob and SHA-256 bound"),
        },
        "not_run": ["Cargo or rustc", "resolution", "staging build", "engine", "observation",
                    "image or pool", "Barge writes", "host cache writes"],
        "premises": [{"premise": name, "disposition": text} for name, text in PREMISES],
        "claim": CLAIM,
    }


def plan() -> int:
    say(json.dumps(plan_document(), indent=2, sort_keys=True))
    return 0


# ---------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------


class FakeReadiness(ReadinessSeams):
    """Scripted effects: no git, seed, Cargo or host read beyond the script."""

    def __init__(self, script: dict) -> None:
        self.script = script
        self.calls: list = []
        self.clock = script.get("now", 10 ** 12)

    def now_ns(self):
        return self.clock

    def read_bytes(self, path):
        self.calls.append(("read_bytes", path))
        if path not in self.script["files"]:
            raise OSError("no such fixture document")
        return self.script["files"][path]

    def git_text(self, repo, *args):
        self.calls.append(("git", repo, args))
        key = (repo, args)
        if key not in self.script["git"]:
            raise obs.Obstruction(f"git {args[0]} failed in {repo}")
        value = self.script["git"][key]
        return value.decode() if isinstance(value, bytes) else value

    def git_bytes(self, repo, *args):
        self.calls.append(("git-bytes", repo, args))
        return self.script["git"][(repo, args)]

    def exists(self, path):
        return path in self.script.get("exists", set())

    def isdir(self, path):
        return True

    def mode(self, path):
        return 0o700

    def realpath(self, path):
        return path

    def sha256(self, path):
        return self.script["sha"][path]

    def make_root(self, path):
        self.calls.append(("make_root", path))

    def make_dirs(self, path):
        self.calls.append(("make_dirs", path))

    def seed(self, host_cargo_home, cargo_home, lock_bytes, deadline_ns, provenance=None):
        self.calls.append(("seed", host_cargo_home, cargo_home, deadline_ns))
        behaviour = self.script["seed"][lock_bytes]
        self.clock += behaviour.get("advance_ns", 0)
        if provenance is not None:
            provenance.update(behaviour.get("provenance", {}))
        if behaviour.get("refuse"):
            raise obs.Obstruction(f"the private offline seed refused: {behaviour['refuse']}")
        return behaviour["seed"]

    def write_json(self, path, document):
        self.calls.append(("write_json", path))
        if self.script.get("write_fail"):
            raise OSError("the report destination is unwritable")
        self.script["written"] = document


SELF_TEST_SOURCE = "/self-test/explicit-seed-source"
SELF_TEST_SIBLING = "/self-test/barge"
GUARD_ROOT = "/guard/root"
GUARD_DEADLINE = 10 ** 12 + 180 * 1_000_000_000


def _binding(oxide_lock: bytes, barge_lock: bytes) -> dict:
    return {
        "schema": BINDING_SCHEMA,
        "planner_reference": "self-test-reference",
        "seed_source": SELF_TEST_SOURCE,
        "oxide": {"commit": "1" * 40, "tree": "2" * 40, "lock_blob": "3" * 40,
                  "lock_sha256": obs.sha256_bytes(oxide_lock)},
        "barge": {"sibling": SELF_TEST_SIBLING, "commit": BARGE_REFERENCE["commit"],
                  "reviewed_source": BARGE_REFERENCE["reviewed_source"],
                  "tree": BARGE_REFERENCE["tree"], "lock_blob": BARGE_REFERENCE["lock_blob"],
                  "lock_sha256": obs.sha256_bytes(barge_lock)},
        "guard": {"supervisor_sha256": SUPERVISOR_SHA256, "limit_seconds": LIMIT_SECONDS,
                  "scratch_mib": SCRATCH_MIB},
        "volume": {"uuid": SELECTED_VOLUME_UUID},
        "paths": {"run_root_name": "seed-readiness", "report_name": REPORT_NAME},
    }


def _env() -> dict:
    return {
        "OXIDE_TASK_SCRATCH_ROOT": GUARD_ROOT + "/scratch",
        "OXIDE_PROOF_EVIDENCE": GUARD_ROOT + "/evidence",
        "OXIDE_PROOF_RUN_ID": "self-test-run",
        "OXIDE_PROOF_DEADLINE_NS": str(GUARD_DEADLINE),
    }


def _script(binding: dict, oxide_lock: bytes, barge_lock: bytes, seeds: dict) -> dict:
    """Scripted git, files and seed behaviour for one occurrence."""
    payload = json.dumps(binding, sort_keys=True).encode()
    sha = {os.path.join(REPO, "scripts", "bounded-test.py"): SUPERVISOR_SHA256}
    git = {
        (REPO, ("rev-parse", "--verify", "1" * 40 + "^{tree}")): "2" * 40,
        (REPO, ("rev-parse", "--verify", "1" * 40 + ":Cargo.lock")): "3" * 40,
        (REPO, ("cat-file", "blob", "3" * 40)): oxide_lock,
        (SELF_TEST_SIBLING, ("rev-parse", "--verify", BARGE_REFERENCE["reviewed_source"] + "^{tree}")): BARGE_REFERENCE["tree"],
        (SELF_TEST_SIBLING, ("rev-parse", "--verify", BARGE_REFERENCE["commit"] + "^{tree}")): BARGE_REFERENCE["tree"],
        (SELF_TEST_SIBLING, ("rev-parse", "--verify", BARGE_REFERENCE["commit"] + ":Cargo.lock")): BARGE_REFERENCE["lock_blob"],
        (SELF_TEST_SIBLING, ("cat-file", "blob", BARGE_REFERENCE["lock_blob"])): barge_lock,
    }
    return {"files": {"/binding.json": payload}, "git": git, "sha": sha, "seed": seeds,
            "exists": set(), "now": 10 ** 12}


def _seed_ok(lock: bytes, *, commit_present: bool = True) -> dict:
    return {"seed": {
        "lock_sha256": obs.sha256_bytes(lock), "packages": 1, "aggregate_bytes": 814,
        "entries": 51, "manifest_bytes": 300, "files": [{"path": "registry/index/config.json",
                                                          "sha256": "a" * 64, "bytes": 10}],
        "git_sources": [{"repository": "seedrepo", "rev": "4" * 40, "candidates": [
            {"database": "seedrepo-0123456789abcdef", "copied": True, "bytes": 500,
             "filecount": 1, "commit_present": commit_present, "rejected": None,
             "skipped": None, "files": [{"path": "git/db/x/HEAD", "sha256": "b" * 64, "bytes": 5}]}],
            "packages": ["seedrepo 1.2.3"]}],
        "limits": {"work_seconds": 45.0}}}


def _refusal(*, closure: bool = False) -> dict:
    refusal = {"component": "index.crates.io-1949cf8c6b5b557f", "component_index": 2,
               "operation": "open directory without following links (O_NOFOLLOW|O_DIRECTORY)",
               "errno": "ENOENT", "kind": "unknown"}
    if closure:
        refusal["closure"] = "unknown"
    return {"schema": "oxide.seed-provenance/1", "requested_source": SELF_TEST_SOURCE,
            "refusal": refusal, "git": [], "registry": {}, "source": None}


def self_test() -> int:
    checks = []

    def check(name, condition, detail=""):
        checks.append((name, bool(condition), detail))

    oxide_lock = b'version = 3\n\n[[package]]\nname = "seedcrate"\n'
    barge_lock = b'version = 3\n\n[[package]]\nname = "barge"\n'
    binding = _binding(oxide_lock, barge_lock)
    options = {"binding": "/binding.json", "sha256": obs.sha256_bytes(json.dumps(binding, sort_keys=True).encode())}

    def run(seeds: dict, *, write_fail: bool = False, script_edit=None):
        script = _script(binding, oxide_lock, barge_lock, seeds)
        script["write_fail"] = write_fail
        if script_edit:
            script_edit(script)
        fake = FakeReadiness(script)
        code = control(options, fake, _env())
        return code, fake, script

    # Both locks complete, one shared deadline, distinct destinations.
    seeds = {oxide_lock: _seed_ok(oxide_lock), barge_lock: _seed_ok(barge_lock)}
    code, fake, script = run(seeds)
    seed_calls = [call for call in fake.calls if call[0] == "seed"]
    check("both locks complete and the report is published", code == 0 and script["written"]["outcome"]["state"] == "completed", code)
    check("two seeds run, one per lock, in order",
          [call[1] for call in seed_calls] == [SELF_TEST_SOURCE, SELF_TEST_SOURCE]
          and seed_calls[0][2] != seed_calls[1][2], seed_calls)
    check("the two locks use distinct private destinations inside the owned root",
          len({call[2] for call in seed_calls}) == 2
          and all(call[2].startswith(GUARD_ROOT + "/scratch/") for call in seed_calls), seed_calls)
    expected_deadline = GUARD_DEADLINE - SHUTDOWN_NS
    check("both seeds share one absolute deadline, the guard's less the 30 s margin",
          all(call[3] == expected_deadline for call in seed_calls), [call[3] for call in seed_calls])
    check("the report names the explicit seed source as seed source only",
          script["written"]["seed_source"]["requested"] == SELF_TEST_SOURCE
          and script["written"]["seed_source"]["role"] == obs.SEED_ROLE)
    check("the report is never qualified", script["written"]["qualified"] is False)
    check("no Cargo, rustc, engine or observation call is made",
          all(call[0] in ("read_bytes", "git", "git-bytes", "make_root", "make_dirs", "seed", "write_json")
              for call in fake.calls))
    check("lock bytes come from the bound commit, read-only",
          all(call[0] in ("git", "git-bytes") and call[2][0] in ("rev-parse", "cat-file")
              for call in fake.calls if call[0] in ("git", "git-bytes")))
    check("the report carries the seed summary for both locks",
          all(entry["seed"]["lock_sha256"] == entry["lock_sha256"] for entry in script["written"]["locks"]))

    # A first-lock clean refusal does not stop the second lock.
    seeds = {oxide_lock: dict(_seed_ok(oxide_lock), refuse="required seed directory is missing",
                              provenance=_refusal()),
             barge_lock: _seed_ok(barge_lock)}
    code, fake, script = run(seeds)
    locks = {entry["side"]: entry for entry in script["written"]["locks"]}
    check("a clean first refusal still runs the second lock",
          locks["barge"]["status"] == "completed" and locks["oxide"]["status"] == "refused"
          and code == 1, (code, {k: v["status"] for k, v in locks.items()}))
    check("the first refusal keeps the seeder's provenance in the report",
          locks["oxide"]["provenance"]["refusal"]["component_index"] == 2
          and locks["oxide"]["provenance"]["refusal"]["errno"] == "ENOENT"
          and locks["oxide"]["provenance"]["requested_source"] == SELF_TEST_SOURCE)
    check("a clean refusal is closure clean", locks["oxide"]["closure"] == "clean")

    # An unknown-closure first outcome keeps the second lock not-run.
    seeds = {oxide_lock: dict(_seed_ok(oxide_lock), refuse="git did not answer", provenance=_refusal(closure=True)),
             barge_lock: _seed_ok(barge_lock)}
    code, fake, script = run(seeds)
    locks = {entry["side"]: entry for entry in script["written"]["locks"]}
    seed_count = len([call for call in fake.calls if call[0] == "seed"])
    check("an unknown-closure first outcome keeps the second lock not-run",
          locks["barge"]["status"] == "not-run" and seed_count == 1, (locks["barge"]["status"], seed_count))
    check("the not-run lock names its reason", "closure is unknown" in (locks["barge"]["reason"] or ""))
    check("an incomplete occurrence exits nonzero but still publishes",
          code == 1 and script["written"]["outcome"]["state"] == "incomplete")

    # Partial records after the second seed fails.
    seeds = {oxide_lock: _seed_ok(oxide_lock),
             barge_lock: dict(_seed_ok(barge_lock), refuse="no host barge database resolves the locked commit",
                              provenance=_refusal())}
    code, fake, script = run(seeds)
    locks = {entry["side"]: entry for entry in script["written"]["locks"]}
    check("a second-lock refusal keeps the first lock's completed seed",
          locks["oxide"]["status"] == "completed" and locks["oxide"]["seed"]["aggregate_bytes"] == 814)
    check("a second-lock refusal keeps its own partial provenance",
          locks["barge"]["status"] == "refused" and locks["barge"]["provenance"]["refusal"]["errno"] == "ENOENT")

    # Shared window: the first seed spends time; the second lock gets the same
    # absolute deadline (no fresh clock) and still runs while the window is open.
    seeds = {oxide_lock: dict(_seed_ok(oxide_lock), advance_ns=100 * 1_000_000_000),
             barge_lock: _seed_ok(barge_lock)}
    code, fake, script = run(seeds)
    seed_calls = [call for call in fake.calls if call[0] == "seed"]
    locks = {entry["side"]: entry for entry in script["written"]["locks"]}
    check("the second lock runs inside the shared window after the first spent 100 s",
          locks["barge"]["status"] == "completed" and code == 0
          and [call[3] for call in seed_calls] == [expected_deadline, expected_deadline],
          (code, [call[3] for call in seed_calls]))

    # Shared window: the first seed runs the window out; the second lock is not run,
    # its reason names the closed window, and the occurrence is incomplete.
    seeds = {oxide_lock: dict(_seed_ok(oxide_lock), advance_ns=160 * 1_000_000_000),
             barge_lock: _seed_ok(barge_lock)}
    code, fake, script = run(seeds)
    locks = {entry["side"]: entry for entry in script["written"]["locks"]}
    seed_count = len([call for call in fake.calls if call[0] == "seed"])
    check("a first seed that closes the shared window keeps the second lock not-run",
          locks["barge"]["status"] == "not-run" and seed_count == 1
          and "window closed" in (locks["barge"]["reason"] or "")
          and code == 1, (code, seed_count, locks["barge"]))

    # An already-closed window refuses before any effect, with no run root and no seed.
    def expired(script):
        script["now"] = expected_deadline + 1
    code, fake, script = run({oxide_lock: _seed_ok(oxide_lock), barge_lock: _seed_ok(barge_lock)},
                             script_edit=expired)
    check("an already-closed shared window refuses before any run root or seed",
          code == 2 and not any(call[0] in ("make_root", "seed") for call in fake.calls), (code, fake.calls))

    # A bound commit whose tree differs refuses that lock before its seed runs.
    def wrong_commit_tree(script):
        script["git"][(SELF_TEST_SIBLING, ("rev-parse", "--verify", BARGE_REFERENCE["commit"] + "^{tree}"))] = "9" * 40
    code, fake, script = run({oxide_lock: _seed_ok(oxide_lock), barge_lock: _seed_ok(barge_lock)},
                             script_edit=wrong_commit_tree)
    locks = {entry["side"]: entry for entry in script["written"]["locks"]}
    check("a commit tree that differs from the bound tree refuses that lock and runs no seed for it",
          locks["barge"]["status"] == "refused" and "tree" in (locks["barge"]["reason"] or "")
          and not any(call[0] == "seed" and call[2].endswith("seed-barge/cargo-home") for call in fake.calls))

    # A reviewed-source tree that differs refuses the whole occurrence before any effect.
    def wrong_reviewed(script):
        script["git"][(SELF_TEST_SIBLING, ("rev-parse", "--verify", BARGE_REFERENCE["reviewed_source"] + "^{tree}"))] = "9" * 40
    code, fake, script = run({oxide_lock: _seed_ok(oxide_lock), barge_lock: _seed_ok(barge_lock)},
                             script_edit=wrong_reviewed)
    check("a reviewed-source tree mismatch refuses before any run root or seed",
          code == 2 and not any(call[0] in ("make_root", "seed") for call in fake.calls), (code, fake.calls))

    def wrong_blob(script):
        script["git"][(REPO, ("cat-file", "blob", "3" * 40))] = b"tampered\n"
    code, fake, script = run({oxide_lock: _seed_ok(oxide_lock), barge_lock: _seed_ok(barge_lock)}, script_edit=wrong_blob)
    locks = {entry["side"]: entry for entry in (script or {}).get("written", {}).get("locks", [])}
    check("a lock whose bytes differ from the bound digest refuses and runs no seed",
          locks.get("oxide", {}).get("status") == "refused"
          and "digest" in (locks.get("oxide", {}).get("reason") or "")
          and not any(call[0] == "seed" and call[2].endswith("seed-oxide/cargo-home") for call in fake.calls))

    # A bound binding with a non-absolute seed source or wrong bounds refuses.
    def refuses(name, mutate):
        bad = json.loads(json.dumps(binding))
        mutate(bad)
        try:
            parse_binding(json.dumps(bad).encode())
        except obs.Obstruction as error:
            check(name, True, str(error))
            return
        except Exception as error:  # noqa: BLE001 - an unexpected class is a failure
            check(name, False, f"unexpected {type(error).__name__}: {error}")
            return
        check(name, False, "the malformed binding was accepted")

    refuses("a non-object volume refuses as a binding refusal", lambda b: b.update(volume="SELECTED"))
    refuses("a non-object guard refuses as a binding refusal", lambda b: b.update(guard=["x"]))
    refuses("a non-object paths refuses as a binding refusal", lambda b: b.update(paths=None))
    refuses("a relative seed source refuses", lambda b: b.update(seed_source="relative/cache"))
    refuses("a 900 s limit is not this route's bound", lambda b: b["guard"].update(limit_seconds=900))
    refuses("a wrong volume refuses", lambda b: b.update(volume={"uuid": "0" * 8}))
    refuses("a short lock digest refuses", lambda b: b["oxide"].update(lock_sha256="abc"))
    refuses("a missing reviewed source refuses", lambda b: b["barge"].update(reviewed_source="x"))

    # A report publication failure is nonzero.
    code, fake, script = run({oxide_lock: _seed_ok(oxide_lock), barge_lock: _seed_ok(barge_lock)}, write_fail=True)
    check("a report publication failure is nonzero", code == 2, code)

    # The explicit source is read, never written: the seed destination is private.
    check("the seed source is never a destination",
          all(call[2] != SELF_TEST_SOURCE for call in fake.calls if call[0] == "seed"))

    check("the nine proof-authoring premises each name a site or a not-applicable reason",
          len(PREMISES) == 9 and all(text.startswith(("applicable:", "not-applicable:"))
                                     for _name, text in PREMISES))

    # The real seeder on two fixture locks: explicit source, private
    # destinations, a clean first seed and a refused second with its provenance.
    real = self_test_real_seeder()
    checks.extend(real)

    for name, ok, detail in checks:
        say(f"self-test: {name}: {'ok' if ok else 'FAILED ' + str(detail)}")
    failed = [name for name, ok, _ in checks if not ok]
    if failed:
        say(f"seed-readiness self-test: {len(failed)} failed: {', '.join(failed)}")
        return 1
    say(f"seed-readiness self-test: {len(checks)} controls passed; no Cargo, rustc, engine, "
        "observation, image or host write ran")
    return 0


def self_test_real_seeder() -> list:
    """Two fixture locks through the shared seeder on one tiny fixture host: an
    explicit host source, a private destination, one clean seed and one refused
    seed whose partial provenance survives. The host tree is unchanged afterwards.
    Real host locks are not read here."""
    results = []
    # Under the qualified supervisor the fixtures stay inside its owned scratch root.
    base = tempfile.mkdtemp(prefix="seed-readiness-",
                            dir=os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or None)
    try:
        host, rev, checksum = offline_seed._fixture_host(pathlib.Path(base) / "a")
        before = tree_signature(str(host))
        lock_a = offline_seed._lock_text(rev, checksum).encode()
        lock_b = offline_seed._lock_text("0" * 40, checksum).encode()
        seams = obs.Seams()
        deadline = offline_seed.now_ns() + 120 * 1_000_000_000
        dest_a = os.path.join(base, "seed-oxide", "cargo-home")
        dest_b = os.path.join(base, "seed-barge", "cargo-home")
        prov_a, prov_b = {}, {}
        seed_a = seams.seed(str(host), dest_a, lock_a, deadline, provenance=prov_a)
        refused = None
        try:
            seams.seed(str(host), dest_b, lock_b, deadline, provenance=prov_b)
        except obs.Obstruction as error:
            refused = str(error)
        after = tree_signature(str(host))
        results.append(("real seed: explicit source, private destination, first lock complete",
                        seed_a["packages"] == 1 and prov_a["requested_source"] == str(host)
                        and prov_a["destination"] == dest_a and prov_a["refusal"] is None, prov_a))
        results.append(("real seed: the second lock refuses with its partial provenance kept",
                        refused is not None and prov_b["refusal"] is not None
                        and prov_b["source"] is not None and prov_b["registry"]["config_json"] is not None,
                        refused))
        results.append(("real seed: the explicit host source is not written (names, modes, sizes, inodes and times)",
                        before == after and len(before) > 0, (len(before), len(after))))
        results.append(("real seed: the seed summary is bounded and names no file list",
                        len(json.dumps(seed_summary(seed_a))) < 16 * 1024, len(json.dumps(seed_summary(seed_a)))))
    except Exception as error:  # a broken fixture is a failed control, not a pass
        results.append(("real seed controls ran", False, f"{type(error).__name__}: {error}"))
    finally:
        shutil.rmtree(base, ignore_errors=True)
    return results


def main(argv: list) -> int:
    if argv == ["--plan"]:
        return plan()
    if argv == ["--self-test"]:
        return self_test()
    if argv and argv[0] == "--control":
        options = {}
        index = 1
        while index < len(argv):
            if argv[index] in ("--binding", "--sha256") and index + 1 < len(argv):
                options[argv[index][2:]] = argv[index + 1]
                index += 2
            else:
                say(USAGE)
                return 2
        return control(options)
    say(USAGE)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
