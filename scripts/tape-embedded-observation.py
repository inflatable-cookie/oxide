#!/usr/bin/env python3
"""Observation-only embedded control diagnostic (oxide#289).

One fixed orchestration command that observes ONE embedded control on each
actual engine, at most once per engine, sequentially, inside the qualified
shared proof supervisor's own supervised group:

- Oxide: the test-only `tape_prune_eval --observe-base` route, which builds
  only the existing `Case::Base` fixture and runs only the existing
  `--fixture-root/--manifest/--transcript-out/--case BASE` engine chain.
- Barge: its existing no-argument `tape_eval` embedded route, which builds its
  CTL-0 fixture, calls `run` with `embedded-unverified` and retains the
  transcript under this run's own `BARGE_TEST_SCRATCH_ROOT`. The route runs
  under the explicit retained-custody opt-in of the reviewed Barge revision
  (`BARGE_CUSTODY_REVIEWED`, Barge 05b4b52, tree 68f96990), so the fixture
  survives for the guard's census. Its export-gated `--fixture-root` argv is
  deliberately not used.

The two case IDs stay distinct, the two fixtures stay physically distinct and
their deliberate setup differences are recorded, never normalised into a
common-input assumption and never named a policy difference. Matching fates
award nothing: there is no agreement, equivalence, timing, writer-closure,
export, deletion or deployment claim anywhere in the report, and `qualified`
is false unconditionally. Different fates are raw observations to investigate,
not a proved divergence.

This file is source preparation. Nothing here ran an engine, a fixture, a
staging build or the diagnostic: `--plan` and `--self-test` are effect-free and
`--observe` refuses before any effect until a planner supplies one bound
binding document.

Usage:
    tape-embedded-observation.py --plan
    tape-embedded-observation.py --self-test
    tape-embedded-observation.py --observe --binding <path> --sha256 <hex>
"""
from __future__ import annotations

import hashlib
import json
import os
import plistlib
import subprocess
import sys
import tarfile
import time

sys.dont_write_bytecode = True

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCHEMA = "oxide.capture.embedded-control-observation/1"
BINDING_SCHEMA = "oxide.capture.embedded-control-binding/1"
CLAIM = (
    "none: one raw observation per engine; no agreement, equivalence, timing, "
    "writer-closure, export, deletion or deployment credit"
)
SUPERVISOR_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
SELECTED_VOLUME_UUID = "8A660440-C919-45C6-A3F1-3365B9BD2006"
BUILDER = "1.97.1"
MIB = 1024 * 1024
REPORT_NAME = "embedded-control-observation.json"
TEXT_CAP = 64 * 1024
GAP_CAP = 64
# One occurrence: two offline staging builds with debug info off, two private
# bounded offline seeds, two engine windows, two parse passes and shutdown
# margin, all inside the guard's limit.
LIMIT_SECONDS = 900
SCRATCH_MIB = 1536
SHUTDOWN_MARGIN_SECONDS = 60
# Raw evidence is copied out of the guard's scratch leaf (which the guard
# deletes on a closed run) into its retained evidence subtree, under the same
# monitored budget. Past these caps the copy is omitted and named; it is never
# silently dropped and never replaced by a summary.
RETENTION_CAP_BYTES = 768 * MIB
RETENTION_FILE_CAP_BYTES = 384 * MIB
SUPERVISOR_PATH_MARKER = "barge.tape.exercise/1: kept fixture "
ARCHIVE_ENTRY_CAP = 20000
ARCHIVE_BYTE_CAP = 128 * MIB
BOUNDED_REFUSAL_MARKER = "bounded refusal (not qualification)"
TRANSCRIPT_MARKER = "barge.tape.exercise/1: retained "
USAGE = (
    "usage: tape-embedded-observation.py "
    "[--plan | --self-test | --observe --binding <path> --sha256 <hex>]"
)

# The two controls. Each side has exactly one, and no argument can add one.
CONTROLS = (
    {
        "engine": "oxide",
        "case": "BASE",
        "route": "tape_prune_eval --observe-base (test-only)",
        "fixture": "a fresh owner-only fixture inside this run's Oxide root: a "
                   "budget-only directory pool, fenced control state, admission "
                   "lock, receipt/marker pairs for a present checkout and an absent "
                   "one, and an unlisted legacy root",
        "expected": "on shared-volume scratch the present fixture binds no external "
                    "root, so the honest BASE outcome is a completed, incomplete "
                    "batch that keeps every root; that is a fixture-placement fact, "
                    "not a policy difference",
    },
    {
        "engine": "barge",
        "case": "CTL-0",
        "route": "tape_eval with no arguments (embedded exercise)",
        "fixture": "a fresh fixture inside this run's Barge run root: one real "
                   "volume record on the selected non-boot APFS volume, one area "
                   "declaration, one owned root created through the real platform, "
                   "one settled Oxide row whose checkout is absent",
        "expected": "the unchanged `release::plan` path over the real host object; "
                    "a recorded capture-cap refusal is a valid observed outcome",
    },
)

# The nine-item proof-authoring premise checklist, traced through this route.
PREMISES = (
    (
        "private HOME and rustup",
        "applicable: `staging_environment` captures the invoking RUSTUP_HOME (or "
        "HOME/.rustup) before any private HOME exists, and every staging build and "
        "engine child gets it with RUSTUP_TOOLCHAIN=1.97.1 and RUSTUP_AUTO_INSTALL=0; "
        "HOME and CARGO_HOME are per-engine private directories inside this run's "
        "owned root. Each lock is seeded into its private CARGO_HOME by the shared "
        "bounded offline seeder before the one --locked call; Cargo creates the "
        "resolved checkout inside that private home, so nothing is downloaded or "
        "installed.",
    ),
    (
        "account-home control",
        "not applicable on the Barge side (no Oxide account facts). On the Oxide "
        "side the engine child is invoked through the harness's unchanged "
        "`engine_child`, which sets OXIDE_TEST_HOME_FROM_ENV=1 beside the private "
        "fixture HOME, exactly as the existing self-check case already does.",
    ),
    (
        "environment isolation",
        "applicable: `staging_environment` and `engine_environment` drop every "
        "inherited OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC*/GIT_ key plus CARGO, "
        "CARGO_INCREMENTAL, RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS and "
        "RUSTUP_TOOLCHAIN, then set only the declared private values.",
    ),
    (
        "automatic adoption",
        "not applicable: no permanent-drive mode, no Git repository in either "
        "fixture and no outputs conversion; the Oxide fixture keeps its target local.",
    ),
    (
        "Cargo external argv",
        "not applicable: no external Cargo subcommand runs; the two staged test "
        "binaries are invoked directly with their own fixed argv.",
    ),
    (
        "owned images",
        "not applicable: this route attaches no image and mounts nothing. Both "
        "fixtures need only the selected volume the guard already chose.",
    ),
    (
        "compiler flags",
        "applicable: RUSTFLAGS is the fixed empty value and "
        "CARGO_ENCODED_RUSTFLAGS is removed for both staging builds; the engine "
        "children inherit neither and run no compiler.",
    ),
    (
        "temporary storage",
        "applicable: every root is a fresh directory inside the qualified "
        "supervisor's OXIDE_TASK_SCRATCH_ROOT on the selected volume, and "
        "TMPDIR/TMP/TEMP name per-engine directories inside it. A /private/tmp "
        "guard fallback refuses before any effect rather than reporting coverage.",
    ),
    (
        "macOS mktemp",
        "applicable: the guard creates its own run root and this script creates "
        "each owned root with os.mkdir under it; no fixed /tmp name, and the engine "
        "children inherit the owned TMPDIR.",
    ),
)

# The reviewed Barge custody source this route binds (oxide#290). Barge049
# (`cefb5d3e`, Barge PR #49) was independently reviewed `ready_to_merge` at
# `05b4b52`, tree `68f96990` (review run `1808bf0e`). Its merge `d9eac3b`, the
# pinned sibling head, carries the same tree, so tree identity is the
# source-equivalence proof: a binding names the merge commit, its tree must be
# the reviewed tree, and the archived lock and the lifetime files must match
# these digests before any staging. The gate is source-owned; no binding,
# environment value or marker can open it.
BARGE_CUSTODY_REVIEWED = {
    "linked_task": "cefb5d3e-579c-41e3-89f4-0e3b90ecb39a",
    "review_run": "1808bf0e-51ed-4b45-8470-f29222b40086",
    "revision": "05b4b5290852a0c9c697005794f1fda78f1b4420",
    "tree": "68f96990d6e190852fe61e298d76b6aebdb31076",
    "lock_sha256": "4cb40dd563cf078088000e5f72edfe23a606b7cb89477d1aabb1792386c475c0",
    "files": {
        "testkit": "396d0eb9bbe9de201e8448485ae661b41682999cae32e879248875218d6da44f",
        "tape_harness": "83f51163c74705e3d1abcae2650601eb066f57a17d87593f53291616ab39d45c",
        "tape_eval": "c1ed42f8e76a050233e5de7d5fa14ea794ca069b0d49cf6199819c4826895c19",
    },
}
# The lifetime property holds only under the explicit, source-owned opt-in: the
# driver sets both variables for the Barge child and no other route keeps a
# fixture. Without them the no-argument route is the ordinary self-removing one.
BARGE_CUSTODY_OPT_IN_ENV = "BARGE_TAPE_CUSTODY"
BARGE_CUSTODY_OPT_IN_VALUE = "barge.tape.kept-fixture/1"
BARGE_SCRATCH_ROOT_ENV = "BARGE_TEST_SCRATCH_ROOT"
BARGE_CUSTODY_PROPERTY = (
    "fixture-kept-from-creation-under-explicit-opt-in: with "
    "BARGE_TAPE_CUSTODY=barge.tape.kept-fixture/1 and this invocation's own "
    "BARGE_TEST_SCRATCH_ROOT, the fixture leaves TempDir ownership inside its "
    "creation call before any fallible step, so the guard's post-leader census and "
    "not the process owns removal; the kept path is printed in one bounded line; "
    "manifest.json is published inside it before the window runs; without the "
    "opt-in the ordinary self-removing route is unchanged")
BARGE_CUSTODY_FILES = {
    "testkit": os.path.join("crates", "barge", "src", "testkit.rs"),
    "tape_harness": os.path.join("crates", "barge", "src", "tape_harness.rs"),
    "tape_eval": os.path.join("crates", "barge", "tests", "tape_eval.rs"),
}


BINDING_TEMPLATE = {
    "schema": BINDING_SCHEMA,
    "planner_reference": "<the Queue reference that activates this ONE occurrence>",
    "oxide": {"commit": "<40 lowercase hex>", "tree": "<40 lowercase hex>",
              "lock_sha256": "<64 lowercase hex>"},
    "barge": {"sibling": "<absolute read-only Barge checkout>",
              "commit": "<40 lowercase hex>", "tree": "<40 lowercase hex>",
              "lock_sha256": "<64 lowercase hex>",
              "custody": {"linked_task": "<linked Barge custody task>",
                          "reviewed_revision": "<40 lowercase hex>",
                          "property": BARGE_CUSTODY_PROPERTY}},
    "toolchain": {"rustc": "<absolute path>", "rustc_sha256": "<64 lowercase hex>",
                  "cargo": "<absolute path>", "cargo_sha256": "<64 lowercase hex>",
                  "version": BUILDER},
    "guard": {"supervisor_sha256": SUPERVISOR_SHA256,
              "limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB,
              "topology": "<root>/scratch and <root>/evidence, siblings"},
    "volume": {"uuid": SELECTED_VOLUME_UUID},
    "paths": {"run_root_name": "tape-observation", "report_name": REPORT_NAME},
}


class Obstruction(Exception):
    """An apparatus, binding or guard refusal: no verdict and no retry."""


class UnknownOutcome(Exception):
    """An outcome this route cannot classify: closure unknown, keep everything."""


def say(text):
    print(text, flush=True)


def sha256_bytes(payload):
    return hashlib.sha256(payload).hexdigest()


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def is_hex(value, length):
    return (isinstance(value, str) and len(value) == length
            and all(char in "0123456789abcdef" for char in value))


def safe_component(name):
    return (isinstance(name, str) and name not in ("", ".", "..")
            and "/" not in name and "\\" not in name and "\0" not in name)


# ---------------------------------------------------------------------------
# Seams. The real implementations are the only ones that run a child, read the
# host or write a file; `--self-test` substitutes scripted fakes.
# ---------------------------------------------------------------------------


class Seams:
    """Real effects of the one observation occurrence."""

    def now_ns(self):
        return time.monotonic_ns()

    def deadline_ns(self):
        return int(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "0"))

    def work_left(self):
        if not self.deadline_ns():
            raise Obstruction("OXIDE_PROOF_DEADLINE_NS is missing: not under the guard")
        return (self.deadline_ns() - self.now_ns()) / 1e9

    def run_logged(self, argv, *, cwd, env, out_path, err_path, timeout_s):
        with open(out_path, "wb") as out, open(err_path, "wb") as err:
            try:
                return subprocess.run(argv, cwd=cwd, env=env, stdout=out,
                                      stderr=err, timeout=timeout_s).returncode
            except subprocess.TimeoutExpired:
                raise UnknownOutcome(f"{argv[0]} did not finish in its bound")

    def run_captured(self, argv, *, cwd, env, timeout_s):
        """One bounded engine window. A timeout returns the output the child did
        produce, marked `timed_out`, so partial evidence is never discarded; the
        caller decides that an unfinished window is an unknown outcome."""
        def bounded(raw):
            if raw is None:
                return "", False
            text_value = raw.decode("utf-8", "replace") if isinstance(raw, bytes) else str(raw)
            return text_value[:TEXT_CAP], len(text_value) > TEXT_CAP

        try:
            done = subprocess.run(argv, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, timeout=timeout_s)
        except subprocess.TimeoutExpired as expired:
            stdout, stdout_truncated = bounded(expired.stdout)
            stderr, stderr_truncated = bounded(expired.stderr)
            return {
                "code": None, "signal": None,
                "stdout": stdout, "stdout_truncated": stdout_truncated,
                "stderr": stderr, "stderr_truncated": stderr_truncated,
                "timed_out": True,
            }
        stdout, stdout_truncated = bounded(done.stdout)
        stderr, stderr_truncated = bounded(done.stderr)
        return {
            "code": done.returncode,
            "signal": None if done.returncode >= 0 else -done.returncode,
            "stdout": stdout,
            "stdout_truncated": stdout_truncated,
            "stderr": stderr,
            "stderr_truncated": stderr_truncated,
            "timed_out": False,
        }

    def query(self, argv, *, cwd=None, timeout_s=60):
        try:
            done = subprocess.run(argv, cwd=cwd, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, timeout=timeout_s)
        except subprocess.TimeoutExpired:
            raise Obstruction(f"{argv[0]} did not finish in its bound")
        return {"code": done.returncode,
                "stdout": done.stdout.decode("utf-8", "replace"),
                "stderr": done.stderr.decode("utf-8", "replace")}

    def volume_facts(self, path):
        result = self.query(["diskutil", "info", "-plist", path], timeout_s=20)
        if result["code"] != 0:
            raise Obstruction(f"the volume facts for {path} are unreadable")
        document = plistlib.loads(result["stdout"].encode("utf-8"))
        return {"uuid": document.get("VolumeUUID"),
                "mount": document.get("MountPoint"),
                "boot": bool(document.get("BootVolume"))}

    def read_bytes(self, path):
        with open(path, "rb") as handle:
            return handle.read()

    def read_text(self, path):
        with open(path, "r", errors="replace") as handle:
            return handle.read()

    def exists(self, path):
        return os.path.exists(path)

    def isfile(self, path):
        return os.path.isfile(path)

    def isdir(self, path):
        return os.path.isdir(path) and not os.path.islink(path)

    def mode(self, path):
        return os.stat(path).st_mode

    def size(self, path):
        return os.path.getsize(path)

    def sha256(self, path):
        return sha256_file(path)

    def file_kind(self, path):
        """The path's own type, without following a final symlink."""
        import stat as stat_module
        mode = os.lstat(path).st_mode
        return {"regular": stat_module.S_ISREG(mode),
                "dir": stat_module.S_ISDIR(mode),
                "symlink": stat_module.S_ISLNK(mode)}

    def make_root(self, path):
        os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
        try:
            os.mkdir(path, 0o700)
        except FileExistsError:
            raise Obstruction(f"{path} already exists; refusal before any effect")

    def make_dirs(self, path):
        os.makedirs(path, mode=0o700, exist_ok=True)

    def write_json(self, path, document):
        with open(path, "w") as handle:
            json.dump(document, handle, indent=2, sort_keys=True)
            handle.write("\n")

    def write_text(self, path, text):
        with open(path, "w") as handle:
            handle.write(text)

    def copy(self, source, destination):
        with open(source, "rb") as reader, open(destination, "xb") as writer:
            while True:
                block = reader.read(1 << 20)
                if not block:
                    break
                writer.write(block)

    def extract(self, sibling, commit, destination):
        """Extract one verified commit with no write to the sibling checkout."""
        archive = subprocess.run(
            ["git", "-C", sibling, "archive", "--format=tar", commit],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=180)
        if archive.returncode != 0:
            raise Obstruction(
                "the pinned Barge commit cannot be archived: "
                + archive.stderr.decode("utf-8", "replace")[-400:])
        self.make_dirs(destination)
        root = os.path.realpath(destination)
        member_file = os.path.join(destination, ".archive.tar")
        with open(member_file, "wb") as handle:
            handle.write(archive.stdout)
        entries = 0
        written = 0
        try:
            with tarfile.open(member_file, "r:") as tar:
                for member in tar:
                    entries += 1
                    if entries > ARCHIVE_ENTRY_CAP:
                        raise Obstruction("the Barge archive exceeds its entry cap")
                    target = os.path.realpath(os.path.join(destination, member.name))
                    if not (target == root or target.startswith(root + os.sep)):
                        raise Obstruction(f"archive member escapes: {member.name}")
                    if member.issym():
                        link = os.path.realpath(
                            os.path.join(os.path.dirname(target), member.linkname))
                        if not (link == root or link.startswith(root + os.sep)):
                            raise Obstruction(f"archive link escapes: {member.name}")
                        self.make_dirs(os.path.dirname(target))
                        os.symlink(member.linkname, target)
                        continue
                    if member.isdir():
                        self.make_dirs(target)
                        continue
                    if not member.isfile():
                        raise Obstruction(
                            f"archive member is not a file: {member.name}")
                    written += member.size
                    if written > ARCHIVE_BYTE_CAP:
                        raise Obstruction("the Barge archive exceeds its byte cap")
                    self.make_dirs(os.path.dirname(target))
                    tar.extract(member, destination)
        finally:
            os.unlink(member_file)
        return {"entries": entries, "bytes": written}

    def seed(self, host_cargo_home, cargo_home, lock_bytes, deadline_ns):
        import offline_seed
        try:
            return offline_seed.seed_cargo_home(
                host_cargo_home, cargo_home, lock_bytes, deadline_ns=deadline_ns)
        except offline_seed.SeedRefusal as error:
            raise Obstruction(f"the private offline seed refused: {error}")


def invoking_rustup():
    return os.environ.get("RUSTUP_HOME") or os.path.join(
        os.path.expanduser("~"), ".rustup")


def invoking_cargo_home():
    return os.environ.get("CARGO_HOME") or os.path.join(
        os.path.expanduser("~"), ".cargo")


def scrubbed(environ):
    """Every inherited product, tool and VCS setting is dropped first. The
    Barge custody opt-in is dropped too: only `run_barge` sets it, for its own
    child, so a stray inherited value can never reach a staging or engine child."""
    return {
        name: value
        for name, value in environ.items()
        if not name.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "RUSTC",
                                "GIT_", "BARGE_"))
        and name not in ("CARGO", "CARGO_INCREMENTAL", "RUSTFLAGS",
                         "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN")
    }


def staging_environment(parent, environ=None, toolchain=None):
    """The one staging environment: private HOME, Cargo home and temp, the
    invoking rustup home, the pinned toolchain and one frozen flag value.

    When the bound toolchain is supplied, `RUSTC` names its exact `rustc` and
    `PATH` is the fixed system path only, so neither an inherited `cargo` nor an
    inherited `rustc` can build the artifacts the envelope reports.
    """
    env = scrubbed(dict(os.environ if environ is None else environ))
    home = os.path.join(parent, "home")
    cargo_home = os.path.join(parent, "cargo-home")
    tmp = os.path.join(parent, "tmp")
    for path in (home, cargo_home, tmp):
        os.makedirs(path, mode=0o700, exist_ok=True)
    env.update({
        "HOME": home,
        "CARGO_HOME": cargo_home,
        "RUSTUP_HOME": invoking_rustup(),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_INCREMENTAL": "0",
        "CARGO_TERM_COLOR": "never",
        "RUSTC_WRAPPER": "",
        "RUSTFLAGS": "",
        "CARGO_TARGET_DIR": os.path.join(parent, "target"),
        "CARGO_BUILD_BUILD_DIR": os.path.join(parent, "target"),
        "TMPDIR": tmp,
        "TMP": tmp,
        "TEMP": tmp,
    })
    if toolchain:
        env["RUSTC"] = toolchain["rustc"]
        env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
    return env


def engine_environment(parent, environ=None, extra=None):
    env = scrubbed(dict(os.environ if environ is None else environ))
    for name in ("home", "tmp"):
        os.makedirs(os.path.join(parent, name), mode=0o700, exist_ok=True)
    env.update({
        "HOME": os.path.join(parent, "home"),
        "TMPDIR": os.path.join(parent, "tmp"),
        "TMP": os.path.join(parent, "tmp"),
        "TEMP": os.path.join(parent, "tmp"),
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
    })
    env.update(extra or {})
    return env


# ---------------------------------------------------------------------------
# Binding document
# ---------------------------------------------------------------------------


def parse_binding(payload):
    """Parse one planner-bound binding document. Every field is exact."""
    try:
        document = json.loads(payload)
    except ValueError as error:
        raise Obstruction(f"the binding document is not JSON: {error}")
    if not isinstance(document, dict) or document.get("schema") != BINDING_SCHEMA:
        raise Obstruction("the binding document is not the current schema")
    if not safe_component(document.get("planner_reference")):
        raise Obstruction("the binding names no planner reference")
    for side in ("oxide", "barge"):
        entry = document.get(side)
        if not isinstance(entry, dict):
            raise Obstruction(f"the binding has no {side} object")
        if not is_hex(entry.get("commit"), 40) or not is_hex(entry.get("tree"), 40):
            raise Obstruction(f"the binding's {side} commit or tree is not exact")
        if not is_hex(entry.get("lock_sha256"), 64):
            raise Obstruction(f"the binding's {side} lock digest is not exact")
    sibling = document["barge"].get("sibling")
    if not isinstance(sibling, str) or not os.path.isabs(sibling):
        raise Obstruction("the binding's Barge sibling is not an absolute path")
    custody = document["barge"].get("custody")
    if not isinstance(custody, dict):
        raise Obstruction(
            "the binding names no reviewed Barge custody revision; the pinned "
            "no-argument route deletes its fixture on every early return and panic")
    if not is_hex(custody.get("reviewed_revision"), 40):
        raise Obstruction("the binding's Barge reviewed custody revision is not exact")
    if not safe_component(custody.get("linked_task")):
        raise Obstruction("the binding names no linked Barge custody task")
    if custody.get("property") != BARGE_CUSTODY_PROPERTY:
        raise Obstruction("the binding does not name the required custody property")
    toolchain = document.get("toolchain")
    if not isinstance(toolchain, dict):
        raise Obstruction("the binding has no toolchain object")
    for name in ("rustc", "cargo"):
        if not isinstance(toolchain.get(name), str) or not os.path.isabs(toolchain[name]):
            raise Obstruction(f"the binding's {name} path is not absolute")
        if not is_hex(toolchain.get(name + "_sha256"), 64):
            raise Obstruction(f"the binding's {name} digest is not exact")
    if toolchain.get("version") != BUILDER:
        raise Obstruction(f"the binding's toolchain is not {BUILDER}")
    guard = document.get("guard")
    if not isinstance(guard, dict):
        raise Obstruction("the binding has no guard object")
    if guard.get("supervisor_sha256") != SUPERVISOR_SHA256:
        raise Obstruction("the binding does not name the qualified supervisor")
    if guard.get("limit_seconds") != LIMIT_SECONDS or guard.get("scratch_mib") != SCRATCH_MIB:
        raise Obstruction("the binding's guard bounds are not this route's fixed bounds")
    if document.get("volume", {}).get("uuid") != SELECTED_VOLUME_UUID:
        raise Obstruction("the binding does not name the selected Stash volume")
    paths = document.get("paths")
    if not isinstance(paths, dict) or not safe_component(paths.get("run_root_name")):
        raise Obstruction("the binding's run root name is not one safe component")
    if not safe_component(paths.get("report_name")):
        raise Obstruction("the binding's report name is not one safe component")
    return document


def read_binding(path, expected_sha256, seams=None):
    seams = seams or Seams()
    if not path or not expected_sha256:
        raise Obstruction("one binding path and its digest are both required")
    if not is_hex(expected_sha256, 64):
        raise Obstruction("the supplied binding digest is not a SHA-256")
    try:
        payload = seams.read_bytes(path)
    except OSError as error:
        raise Obstruction(f"the binding document is unreadable: {error}")
    actual = sha256_bytes(payload)
    if actual != expected_sha256:
        raise Obstruction(
            f"the binding document digest is {actual}, not the supplied "
            f"{expected_sha256}")
    return parse_binding(payload), payload


def guard_context(seams=None, environ=None):
    """The qualified guard's own context and its actual default topology.

    The qualified supervisor's default owned-root setup creates
    `<root>/scratch` and `<root>/evidence` as siblings
    (`bounded-test.py` `Monitor.setup`), and the evidence directory is created
    with the inherited umask, so it is normally 0755 rather than owner-only.
    This route therefore binds that exact sibling layout, requires both
    directories to be real, canonical and not group- or other-writable, and
    refuses an adopted scratch root or an explicit evidence directory, which
    this route is not activated with.
    """
    seams = seams or Seams()
    environ = dict(os.environ if environ is None else environ)
    scratch = environ.get("OXIDE_TASK_SCRATCH_ROOT")
    evidence = environ.get("OXIDE_PROOF_EVIDENCE")
    run_id = environ.get("OXIDE_PROOF_RUN_ID")
    deadline = environ.get("OXIDE_PROOF_DEADLINE_NS")
    if not scratch or not evidence or not deadline or not run_id:
        raise Obstruction(
            "this route runs only under the qualified shared supervisor: "
            "OXIDE_TASK_SCRATCH_ROOT, OXIDE_PROOF_EVIDENCE, OXIDE_PROOF_RUN_ID "
            "and OXIDE_PROOF_DEADLINE_NS are all required")
    for label, path in (("scratch root", scratch), ("evidence directory", evidence)):
        real = os.path.realpath(path)
        if real != path:
            raise Obstruction(f"the guard's {label} is not canonical: {path}")
        if not seams.isdir(path):
            raise Obstruction(f"the guard's {label} is not a real directory: {path}")
        if seams.mode(path) & 0o022:
            raise Obstruction(
                f"the guard's {label} is group- or other-writable: {path}")
    root = os.path.dirname(scratch)
    if (os.path.basename(scratch) != "scratch"
            or os.path.basename(evidence) != "evidence"
            or os.path.dirname(evidence) != root):
        raise Obstruction(
            "the guard's scratch and evidence are not the qualified default "
            "sibling layout <root>/scratch and <root>/evidence; an adopted "
            "scratch root or an explicit evidence directory is not this route's "
            "topology")
    if not seams.isdir(root) or seams.mode(root) & 0o022:
        raise Obstruction(f"the guard's run root {root} is not a safe directory")
    supervisor = seams.sha256(os.path.join(REPO, "scripts", "bounded-test.py"))
    if supervisor != SUPERVISOR_SHA256:
        raise Obstruction(
            f"the running supervisor bytes are {supervisor}, not the qualified "
            f"{SUPERVISOR_SHA256}")
    return {"root": root, "scratch_root": scratch, "evidence": evidence,
            "run_id": run_id, "deadline_ns": int(deadline),
            "supervisor_sha256": supervisor}


def derive_paths(binding, guard):
    """Every owned path, derived from the guard's own roots. Nothing absolute
    comes from the binding, so no caller can name a path outside the run."""
    run_root = os.path.join(guard["scratch_root"], binding["paths"]["run_root_name"])
    return {
        "run_root": run_root,
        "oxide": os.path.join(run_root, "oxide"),
        "barge": os.path.join(run_root, "barge"),
        "oxide_fixture": os.path.join(run_root, "oxide", "fixture"),
        "barge_run": os.path.join(run_root, "barge", "run"),
        "barge_source": os.path.join(run_root, "barge", "source"),
        "report": os.path.join(guard["evidence"], binding["paths"]["report_name"]),
        "evidence_retained": os.path.join(guard["evidence"],
                                          binding["paths"]["run_root_name"]),
        "guard_receipt": os.path.join(guard["evidence"], "receipt.json"),
    }


# ---------------------------------------------------------------------------
# Source binding
# ---------------------------------------------------------------------------


def source_facts(binding, seams=None, scratch_root=None):
    """Every live fact the binding must match. One bounded query per fact."""
    seams = seams or Seams()

    def git(repo, *args):
        result = seams.query(["git", "-C", repo, *args], timeout_s=60)
        if result["code"] != 0:
            raise Obstruction(f"git {' '.join(args)} failed in {repo}")
        return result["stdout"].strip()

    facts = {"oxide": {}, "barge": {}, "toolchain": {}, "volume": {}}
    facts["oxide"]["commit"] = git(REPO, "rev-parse", "HEAD")
    facts["oxide"]["tree"] = git(REPO, "rev-parse", "HEAD^{tree}")
    facts["oxide"]["status"] = git(REPO, "status", "--porcelain", "--untracked-files=no")
    facts["oxide"]["lock_sha256"] = seams.sha256(os.path.join(REPO, "Cargo.lock"))
    sibling = binding["barge"]["sibling"]
    if not seams.exists(sibling):
        raise Obstruction(f"the pinned Barge sibling is absent: {sibling}")
    facts["barge"]["sibling"] = sibling
    facts["barge"]["commit"] = git(sibling, "rev-parse", "HEAD")
    facts["barge"]["tree"] = git(sibling, "rev-parse", "HEAD^{tree}")
    facts["barge"]["status"] = git(sibling, "status", "--porcelain",
                                   "--untracked-files=no")
    facts["barge"]["lock_sha256"] = seams.sha256(os.path.join(sibling, "Cargo.lock"))
    for name in ("rustc", "cargo"):
        path = binding["toolchain"][name]
        if not seams.isfile(path):
            raise Obstruction(f"the bound {name} is absent: {path}")
        facts["toolchain"][name] = path
        facts["toolchain"][name + "_sha256"] = seams.sha256(path)
    result = seams.query([binding["toolchain"]["rustc"], "--version"], timeout_s=30)
    facts["toolchain"]["version_text"] = result["stdout"].strip()
    facts["volume"] = seams.volume_facts(scratch_root or "/")
    return facts


def verify_source_bindings(binding, facts, guard):
    """Refuse on any source, toolchain, volume or guard drift. No retry and no
    newest-artifact selection: the staged binary comes from these bytes."""
    for side in ("oxide", "barge"):
        bound = binding[side]
        for key in ("commit", "tree", "lock_sha256"):
            if facts[side].get(key) != bound[key]:
                raise Obstruction(
                    f"the {side} {key} is {facts[side].get(key)}, not the bound "
                    f"{bound[key]}")
        if facts[side].get("status"):
            raise Obstruction(
                f"the {side} tracked worktree is not clean; the bound source is not "
                "what would be staged")
    for name in ("rustc", "cargo"):
        if facts["toolchain"][name] != binding["toolchain"][name]:
            raise Obstruction(f"the bound {name} path moved")
        if facts["toolchain"][name + "_sha256"] != binding["toolchain"][name + "_sha256"]:
            raise Obstruction(f"the bound {name} bytes changed")
    if BUILDER not in facts["toolchain"]["version_text"]:
        raise Obstruction(
            f"the bound rustc is not {BUILDER}: {facts['toolchain']['version_text']}")
    volume = facts["volume"]
    if volume["uuid"] != binding["volume"]["uuid"]:
        raise Obstruction(
            f"the guard's scratch volume is {volume['uuid']}, not the bound "
            f"{binding['volume']['uuid']}")
    if volume["boot"] or volume["mount"] in (None, "", "/"):
        raise Obstruction("the guard's scratch sits on the boot volume or has no mount")
    mount = os.path.realpath(volume["mount"])
    scratch = os.path.realpath(guard["scratch_root"])
    if os.path.commonpath([mount, scratch]) != mount:
        raise Obstruction("the guard's scratch root is outside the bound volume")
    return {
        "oxide": dict(facts["oxide"]),
        "barge": dict(facts["barge"]),
        "toolchain": dict(facts["toolchain"]),
        "volume": dict(volume),
        "guard": {"supervisor_sha256": guard["supervisor_sha256"],
                  "limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB},
    }


# ---------------------------------------------------------------------------
# Staging
# ---------------------------------------------------------------------------


def artifact_from_log(text, target):
    """The exact executable Cargo reported for one target. Never a directory
    scan and never a newest-file pick."""
    found = None
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            document = json.loads(line)
        except ValueError:
            continue
        if (document.get("reason") == "compiler-artifact"
                and document.get("target", {}).get("name") == target
                and document.get("executable")):
            found = document["executable"]
    return found


def stage(seams, *, parent, cwd, lock_path, cargo_argv, target, label, toolchain,
          artifacts):
    """One bounded offline staging build into the owned root with the bound
    `cargo` and `rustc`, then the exact reported executable and its digest. The
    staging tree is retained in place: only the guard's post-leader census can
    conclude descendant closure.

    The stage log and stderr paths are recorded in `artifacts` *before* Cargo is
    spawned, so a nonzero exit or a timeout still leaves them known to the
    failure-path retention and their actual output is retained."""
    seams.make_dirs(parent)
    out_path = os.path.join(parent, "stage.json")
    err_path = os.path.join(parent, "stage.stderr")
    prefix = label.lower()
    artifacts[f"{prefix}.stage.log"] = out_path
    artifacts[f"{prefix}.stage.stderr"] = err_path
    env = staging_environment(parent, toolchain=toolchain)
    try:
        lock_bytes = seams.read_bytes(lock_path)
    except OSError as error:
        raise Obstruction(f"the {label} lock is unreadable: {error}")
    seed = seams.seed(invoking_cargo_home(), env["CARGO_HOME"], lock_bytes,
                      seams.deadline_ns())
    argv = [toolchain["cargo"], *cargo_argv, "--message-format=json"]
    code = seams.run_logged(argv, cwd=cwd, env=env, out_path=out_path,
                            err_path=err_path,
                            timeout_s=max(1, seams.work_left() - SHUTDOWN_MARGIN_SECONDS))
    if code != 0:
        raise Obstruction(
            f"{label} staging exited {code}; see {out_path} and {err_path}")
    try:
        log_text = seams.read_text(out_path)
    except OSError as error:
        raise Obstruction(f"the {label} staging log is unreadable: {error}")
    executable = artifact_from_log(log_text, target)
    if not executable or not seams.isfile(executable):
        raise Obstruction(f"{label} staging reported no executable for {target}")
    target_root = os.path.realpath(env["CARGO_TARGET_DIR"])
    if os.path.commonpath([target_root, os.path.realpath(executable)]) != target_root:
        raise Obstruction(f"{label} reported an executable outside its target root")
    artifacts[f"{prefix}.executable"] = executable
    return {
        "cargo_argv": argv,
        "cargo": toolchain["cargo"],
        "rustc": toolchain["rustc"],
        "path": env["PATH"],
        "cwd": cwd,
        "seed": seed,
        "log": {"path": out_path, "sha256": seams.sha256(out_path)},
        "stderr": {"path": err_path, "sha256": seams.sha256(err_path)},
        "executable": {"path": executable, "sha256": seams.sha256(executable),
                       "bytes": seams.size(executable)},
    }


def stage_oxide(seams, binding, paths, artifacts):
    return stage(
        seams,
        parent=os.path.join(paths["oxide"], "stage"),
        cwd=REPO,
        lock_path=os.path.join(REPO, "Cargo.lock"),
        cargo_argv=["test", "--test", "tape_prune_eval", "--no-run",
                    "--locked", "--offline"],
        target="tape_prune_eval",
        label="Oxide",
        toolchain=binding["toolchain"],
        artifacts=artifacts,
    )


def require_reviewed_custody(binding):
    """The source gate: refuse before any archive, staging write or engine unless
    a reviewed Barge custody revision is bound in this source. Nothing a binding
    contains can open it."""
    custody = binding["barge"]["custody"]
    if globals()["BARGE_CUSTODY_REVIEWED"] is None:
        raise Obstruction(
            "owner-gated impediment: no reviewed Barge custody revision is bound in "
            "this source, so this occurrence refuses before any engine. The pinned "
            "route's TempDir destructor removes the fixture on every early return "
            f"and panic. Required property: {BARGE_CUSTODY_PROPERTY}. Linked task: "
            f"{custody['linked_task']}")


def verify_barge_custody(binding, source, seams):
    """Refuse unless the archived Barge source is the reviewed custody source.

    The property is not verifiable from bytes: a comment or an unrelated method
    could satisfy any text search. Safety rests on source review of the reviewed
    tree, which `BARGE_CUSTODY_REVIEWED` binds: the bound commit may be a merge or
    a rebase only if its tree is the reviewed tree (tree identity covers every
    byte), the binding's custody block names the reviewed revision and property,
    and the archived lock and each lifetime file match their reviewed digests.
    Everything is checked against the archive before any staging write.
    """
    require_reviewed_custody(binding)
    custody = binding["barge"]["custody"]
    reviewed = globals()["BARGE_CUSTODY_REVIEWED"]
    if custody["reviewed_revision"] != reviewed["revision"]:
        raise Obstruction(
            "the bound Barge custody revision is not the reviewed revision "
            f"({reviewed['revision']})")
    if binding["barge"]["tree"] != reviewed["tree"]:
        raise Obstruction(
            "the bound Barge tree is not the reviewed custody tree "
            f"({reviewed['tree']}); the commit is not the reviewed source")
    if custody["property"] != BARGE_CUSTODY_PROPERTY:
        raise Obstruction("the binding does not name the required custody property")
    lock_path = os.path.join(source, "Cargo.lock")
    if not seams.isfile(lock_path):
        raise Obstruction("the archived Barge tree has no Cargo.lock")
    if seams.sha256(lock_path) != reviewed["lock_sha256"]:
        raise Obstruction("the archived Barge lock is not the reviewed custody lock")
    for name, relative in BARGE_CUSTODY_FILES.items():
        path = os.path.join(source, relative)
        if not seams.isfile(path):
            raise Obstruction(f"the archived Barge tree has no {relative}")
        if seams.sha256(path) != reviewed["files"][name]:
            raise Obstruction(
                f"the archived Barge {relative} is not the reviewed custody revision")
    return {"linked_task": custody["linked_task"],
            "review_run": reviewed["review_run"],
            "reviewed_revision": reviewed["revision"],
            "reviewed_tree": reviewed["tree"],
            "property": BARGE_CUSTODY_PROPERTY,
            "files": {name: reviewed["files"][name] for name in BARGE_CUSTODY_FILES}}


def verify_toolchain_unchanged(binding, seams):
    """The bound `cargo` and `rustc` bytes are re-read after staging, so a tool
    that changed under the run cannot be reported as the bound one."""
    for name in ("rustc", "cargo"):
        if seams.sha256(binding["toolchain"][name]) != \
                binding["toolchain"][name + "_sha256"]:
            raise Obstruction(
                f"the bound {name} changed while this occurrence was running")


def prepare_barge(seams, binding, paths):
    """Archive the bound Barge commit and validate it before any staging or
    engine: the reviewed custody source, its lock and its lifetime files."""
    source = paths["barge_source"]
    archive = seams.extract(binding["barge"]["sibling"], binding["barge"]["commit"],
                            source)
    retention = verify_barge_custody(binding, source, seams)
    if seams.sha256(os.path.join(source, "Cargo.lock")) != binding["barge"]["lock_sha256"]:
        raise Obstruction("the archived Barge lock is not the bound lock")
    return {"source": source, "archive": archive, "retention": retention}


def stage_barge(seams, binding, paths, artifacts, prepared):
    """One bounded offline staging build of the archive `prepare_barge` already
    validated."""
    source = prepared["source"]
    lock_path = os.path.join(source, "Cargo.lock")
    staged = stage(
        seams,
        parent=os.path.join(paths["barge"], "stage"),
        cwd=source,
        lock_path=lock_path,
        cargo_argv=["test", "-p", "barge", "--test", "tape_eval",
                    "--no-run", "--locked", "--offline"],
        target="tape_eval",
        label="Barge",
        toolchain=binding["toolchain"],
        artifacts=artifacts,
    )
    staged["archive"] = prepared["archive"]
    staged["retention"] = prepared["retention"]
    return staged


# ---------------------------------------------------------------------------
# The two engine windows and the canonical parse passes
# ---------------------------------------------------------------------------


def read_document(seams, path):
    """One retained JSON document. An unparsable document stays unparsable: it
    is never repaired, summarised or replaced by an empty one."""
    payload = seams.read_bytes(path)
    record = {"path": path, "bytes": len(payload), "sha256": sha256_bytes(payload)}
    try:
        record["document"] = json.loads(payload.decode("utf-8"))
    except (ValueError, UnicodeDecodeError) as error:
        record["document"] = None
        record["unparsable"] = str(error)
    return record


def safe_artifact(seams, root, path, kind="file"):
    """A path is readable only when it is a real, non-symlink artifact of the
    requested kind that resolves inside `root`. A symlinked final component is
    refused even when its target is inside, so nothing is ever read or copied
    through a link the caller did not create."""
    if not path:
        return None
    try:
        facts = seams.file_kind(path)
    except OSError:
        return None
    if facts["symlink"]:
        return None
    if kind == "file" and not facts["regular"]:
        return None
    if kind == "dir" and not facts["dir"]:
        return None
    return retained_inside(root, path)


def retained_inside(root, reported):
    """A retained path the engine reports must resolve inside the caller's own
    root. Anything else is unknown retention: it is named, never read."""
    if not reported:
        return None
    root = os.path.realpath(root)
    candidate = os.path.realpath(reported)
    if candidate == root or not candidate.startswith(root + os.sep):
        return None
    return candidate


def run_oxide(seams, paths, staged):
    root = paths["oxide_fixture"]
    seams.make_dirs(root)
    report = os.path.join(root, "observation.json")
    argv = [staged["executable"]["path"], "--observe-base", "--root", root,
            "--report-out", report]
    env = engine_environment(os.path.join(paths["oxide"], "engine"))
    result = seams.run_captured(
        argv, cwd=paths["oxide"], env=env,
        timeout_s=max(1, seams.work_left() - SHUTDOWN_MARGIN_SECONDS))
    observation = read_document(seams, report) if seams.isfile(report) else None
    return {"argv": argv, "engine": result, "observation": observation,
            "fixture": root}


def run_barge(seams, paths, staged):
    """The bound no-argument embedded route, run under the explicit retained-custody
    opt-in. The child gets exactly the opt-in and this run's own root; no other
    Barge variable survives `scrubbed`, and no argv is added."""
    run_root = paths["barge_run"]
    seams.make_dirs(run_root)
    argv = [staged["executable"]["path"]]
    env = engine_environment(os.path.join(paths["barge"], "engine"),
                             extra={BARGE_CUSTODY_OPT_IN_ENV: BARGE_CUSTODY_OPT_IN_VALUE,
                                    BARGE_SCRATCH_ROOT_ENV: run_root})
    result = seams.run_captured(
        argv, cwd=paths["barge"], env=env,
        timeout_s=max(1, seams.work_left() - SHUTDOWN_MARGIN_SECONDS))
    reported = None
    for line in result["stderr"].splitlines():
        if line.startswith(TRANSCRIPT_MARKER):
            reported = line[len(TRANSCRIPT_MARKER):].strip()
    retained = safe_artifact(seams, run_root, reported)
    kept_lines = [line for line in result["stderr"].splitlines()
                  if line.startswith(SUPERVISOR_PATH_MARKER)]
    kept = kept_lines[0][len(SUPERVISOR_PATH_MARKER):].strip() if len(kept_lines) == 1 \
        else None
    kept_fixture = safe_artifact(seams, run_root, kept, kind="dir")
    manifest = None
    if kept_fixture:
        manifest = safe_artifact(seams, run_root,
                                 os.path.join(kept_fixture, "manifest.json"))
    return {"argv": argv, "engine": result, "reported": reported,
            "retained": retained,
            "retention_refused": bool(reported) and retained is None,
            "kept_lines": len(kept_lines),
            "kept_fixture": kept_fixture,
            "fixture_kept": bool(kept_fixture),
            "manifest": manifest,
            "fixture": run_root}


def require_barge_custody_record(barge):
    """Every missing, partial or unsafe Barge custody fact is an apparatus
    obstruction, not a recorded fate. The retained evidence is still written by
    `observe`'s finish path, which runs on every exit."""
    engine = barge["engine"]
    if engine["stderr_truncated"]:
        raise Obstruction(
            "Barge's output exceeded its capture bound, so its custody line and "
            "transcript line may be missing; partial output is retained and no fate "
            "is recorded")
    if barge["kept_lines"] != 1:
        raise Obstruction(
            f"Barge announced {barge['kept_lines']} kept-fixture lines, not exactly "
            "one; no custody record exists")
    if not barge["fixture_kept"]:
        raise Obstruction(
            "the announced kept fixture is not a safe directory inside this run's "
            "own root; it was not read and no custody record exists")
    if engine["code"] != 0 or engine["signal"] is not None:
        raise Obstruction(
            f"Barge exited {engine['code']} signal {engine['signal']} before a "
            "complete window; its publication or engine failure is retained")
    if barge["retention_refused"]:
        raise Obstruction(
            "Barge reported a retained transcript that is not a safe regular file "
            "inside this run's own root; it was not read")
    if not barge["retained"]:
        raise Obstruction("Barge reported no retained transcript inside its kept fixture")
    if barge["manifest"] is None:
        raise Obstruction(
            "the kept fixture holds no safe manifest.json, so no strict or limited "
            "verdict applies; the window's own manifest is missing")


def parse_capture(seams, staged, *, root, transcript, manifest, report):
    """One canonical `/2` parser pass over a retained document. No engine."""
    if not seams.isfile(transcript):
        return None
    argv = [staged["executable"]["path"], "--parse-capture", "--root", root,
            "--transcript", transcript]
    if manifest and seams.isfile(manifest):
        argv += ["--manifest", manifest]
    argv += ["--report-out", report]
    env = engine_environment(os.path.join(root, "parse"))
    result = seams.run_captured(argv, cwd=os.path.dirname(root), env=env,
                                timeout_s=120)
    document = read_document(seams, report) if seams.isfile(report) else None
    return {"argv": argv, "engine": result, "report": document}


def retain(seams, paths, stage_artifacts, windows, parses, gaps):
    """Copy the raw occurrence evidence out of the guard's scratch leaf, which
    the guard deletes on a closed run, into its retained evidence subtree.

    This runs on *every* exit, including a staging, engine or parse failure, so
    evidence already produced is never lost. Every source is re-validated as a
    real, non-symlink artifact inside the owned run root before it is read, and
    a source that is missing, outside that root or of the wrong type is recorded
    with its reason and is neither read nor copied. An artifact past the
    retention budget is omitted and named. Nothing is summarised, repaired or
    replaced by a hash, and nothing is deleted from the scratch tree.
    """
    oxide, barge = windows
    target = paths["evidence_retained"]
    oxide_fixture = os.path.join(paths["oxide_fixture"], "fixture-base")
    artifacts = [
        ("oxide.transcript", os.path.join(oxide_fixture, "transcript.json")),
        ("oxide.manifest", os.path.join(oxide_fixture, "manifest.json")),
        ("oxide.observation", os.path.join(paths["oxide_fixture"], "observation.json")),
        ("oxide.parse", os.path.join(paths["oxide_fixture"], "parse.json")),
        ("barge.transcript", (barge or {}).get("retained")),
        ("barge.manifest", (barge or {}).get("manifest")),
        ("barge.parse", os.path.join(paths["barge_run"], "parse.json")),
    ]
    # Stage logs and the executable are known before or as soon as they exist,
    # because `stage` registers them before spawning Cargo; a failed build's own
    # output is therefore retained rather than lost with the scratch leaf.
    for label in sorted(stage_artifacts):
        artifacts.append((label, stage_artifacts[label]))
    try:
        seams.make_root(target)
    except Obstruction as error:
        gaps.append(f"the retained-evidence tree could not be created: {error}")
        return {"root": target, "bytes": 0, "entries": [], "omitted": 0,
                "state": "not-created", "reason": str(error)}
    entries = []
    total = 0
    for label, source in artifacts:
        inside = safe_artifact(seams, paths["run_root"], source) if source else None
        if not inside:
            entries.append({
                "label": label, "source": source, "retained": False,
                "reason": "not present inside the owned run root as a regular file; "
                          "nothing was read or copied"})
            if label in ("barge.transcript", "barge.manifest"):
                gaps.append(f"the Barge {label.split('.')[1]} was not a safe artifact "
                            "inside the owned run root, so it was not read")
            continue
        size = seams.size(inside)
        if size > RETENTION_FILE_CAP_BYTES or total + size > RETENTION_CAP_BYTES:
            entries.append({"label": label, "source": inside, "bytes": size,
                            "retained": False,
                            "reason": "exceeds the retained-evidence budget; only the "
                                      "scratch copy exists and the guard may delete it"})
            gaps.append(f"raw evidence not retained within the budget: {label}")
            continue
        destination = os.path.join(target, f"{label}.{os.path.basename(inside)}")
        try:
            seams.copy(inside, destination)
        except OSError as error:
            entries.append({"label": label, "source": inside, "bytes": size,
                            "retained": False,
                            "reason": f"the copy failed: {error}"})
            gaps.append(f"raw evidence could not be retained: {label}")
            continue
        total += size
        entries.append({"label": label, "source": inside, "retained": True,
                        "path": destination, "bytes": size,
                        "sha256": seams.sha256(destination)})
    for side, window in (("oxide", oxide), ("barge", barge)):
        if window is None:
            continue
        for stream in ("stdout", "stderr"):
            text_value = window["engine"].get(stream) or ""
            destination = os.path.join(target, f"{side}.{stream}.txt")
            seams.write_text(destination, text_value)
            total += len(text_value.encode("utf-8", "replace"))
            entries.append({"label": f"{side}.{stream}", "retained": True,
                            "path": destination,
                            "bytes": len(text_value.encode("utf-8", "replace")),
                            "sha256": seams.sha256(destination)})
    return {"root": target, "bytes": total, "entries": entries, "state": "created",
            "omitted": sum(1 for entry in entries if entry["retained"] is False)}


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def classification(engine, bounded_refusal_marker=None):
    """Engine exit, tool exit and closure stay three separate observations."""
    if engine is None:
        return {"state": "not-run", "exit_code": None, "signal": None}
    if engine.get("timed_out"):
        return {"state": "unknown", "exit_code": engine.get("code"), "signal": None,
                "reason": "the window did not finish in its bound"}
    if engine.get("code") == 0:
        return {"state": "completed", "exit_code": 0, "signal": None}
    bounded = (bounded_refusal_marker is not None
               and bounded_refusal_marker in (engine.get("stderr") or ""))
    return {
        "state": "bounded-capture-refusal" if bounded else "nonzero-exit",
        "exit_code": engine.get("code"),
        "signal": engine.get("signal"),
    }


def fate_of(parse):
    """The raw engine outcome, verbatim. Missing is missing, never empty."""
    if not parse or not parse.get("report"):
        return {"state": "absent",
                "reason": "no retained capture document was parsed"}
    document = parse["report"].get("document")
    if document is None:
        return {"state": "unparsable",
                "reason": parse["report"].get("unparsable")
                or "the retained document was not parsed",
                "qualified": False}
    return {"state": "observed",
            "outcome": document.get("outcome"),
            "parser": document.get("parser", {}).get("transcript"),
            "qualified": False}


def closure_record(guard_receipt):
    """This script never concludes closure. Only the guard's post-leader census
    can, and its receipt states it; the two are distinct fields."""
    return {
        "script": "does-not-conclude",
        "reason": "the qualified guard's post-leader census decides descendant "
                  "closure and may delete its own scratch afterwards; this script "
                  "neither concludes nor claims it",
        "guard_receipt": {"path": guard_receipt,
                          "state": "written-by-the-guard-after-this-report",
                          "closure": "not-read-by-this-script"},
    }


def build_report(*, binding, guard, paths, envelope, oxide, barge, oxide_parse,
                 barge_parse, retained, outcome, closure, gaps):
    """One bounded, honest report. `qualified` is false unconditionally and no
    field records agreement, equivalence or divergence."""
    bounded_gaps = list(gaps)
    if len(bounded_gaps) > GAP_CAP:
        omitted = len(bounded_gaps) - GAP_CAP
        bounded_gaps = bounded_gaps[:GAP_CAP] + [f"{omitted} further gap(s) omitted"]
    return {
        "schema": SCHEMA,
        "qualified": False,
        "claim": CLAIM,
        "planner_reference": binding["planner_reference"],
        "controls": list(CONTROLS),
        "premises": [{"premise": name, "disposition": text}
                     for name, text in PREMISES],
        "bounds": {"limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB,
                   "shutdown_margin_seconds": SHUTDOWN_MARGIN_SECONDS,
                   "fit": "unmeasured"},
        "guard": {
            "scratch_root": guard["scratch_root"],
            "evidence": guard["evidence"],
            "run_id": guard["run_id"],
            "deadline_ns": guard["deadline_ns"],
            "supervisor_sha256": guard["supervisor_sha256"],
            "receipt": {"path": paths["guard_receipt"],
                        "state": "written-by-the-guard-after-this-report"},
        },
        "paths": dict(paths),
        "envelope": envelope,
        "retained": retained,
        "windows": {
            "oxide": {
                "case": "BASE",
                "argv": oxide["argv"] if oxide else None,
                "fixture": oxide["fixture"] if oxide else None,
                "outer": oxide["engine"] if oxide else None,
                "engine": ((oxide["observation"] or {}).get("document") or {}
                           ).get("engine_run") if oxide else None,
                "classification": classification(oxide["engine"] if oxide else None),
                "observation": oxide["observation"] if oxide else None,
                "parse": oxide_parse,
            },
            "barge": {
                "case": "CTL-0",
                "argv": barge["argv"] if barge else None,
                "fixture": barge["fixture"] if barge else None,
                "outer": barge["engine"] if barge else None,
                "engine": {
                    "state": "in-process",
                    "note": "the Barge embedded exercise and its engine are one "
                            "process; the raw outcome is in the retained transcript",
                    "timed_out": barge["engine"].get("timed_out") if barge else None,
                    "exit_code": barge["engine"].get("code") if barge else None,
                    "signal": barge["engine"].get("signal") if barge else None,
                } if barge else None,
                "classification": classification(
                    barge["engine"] if barge else None,
                    bounded_refusal_marker=BOUNDED_REFUSAL_MARKER),
                "retained_transcript": {
                    "reported": barge["reported"] if barge else None,
                    "inside_own_root": barge["retained"] if barge else None,
                    "retention_refused": barge["retention_refused"] if barge else None,
                },
                "fixture_custody": {
                    "kept_fixture": barge["kept_fixture"] if barge else None,
                    "fixture_kept": barge["fixture_kept"] if barge else None,
                    "manifest": barge["manifest"] if barge else None,
                },
                "parse": barge_parse,
            },
        },
        "outcome": outcome,
        "fates": {
            "oxide": fate_of(oxide_parse),
            "barge": fate_of(barge_parse),
            "comparison": "not-claimed: equal and unequal raw fates are both "
                          "observations; no agreement, equivalence or divergence is "
                          "recorded",
        },
        "setup_differences": setup_differences(),
        "closure": closure,
        "gaps": bounded_gaps,
    }


def setup_differences():
    return [
        "Oxide BASE keeps a directory pool, fenced control state, an admission lock "
        "and receipt/marker pairs for a present and an absent checkout; Barge CTL-0 "
        "keeps one real volume record, one declaration, one owned root created "
        "through the real platform and one settled Oxide row.",
        "Oxide BASE's roots live under its fixture's own `oxide-outputs`; Barge "
        "CTL-0's owned root is created from its volume record beneath the fixture "
        "volume root.",
        "On Stash scratch the Oxide fixture's own binding is not a `/Volumes/<name>` "
        "mount, so its honest BASE outcome is a completed incomplete batch that "
        "keeps every root. Barge CTL-0 exercises its real plan over the real host "
        "object. These are fixture-placement facts, never a policy difference and "
        "never evidence of agreement.",
    ]


# ---------------------------------------------------------------------------
# The one occurrence
# ---------------------------------------------------------------------------


def observe(options, seams=None, environ=None):
    """Run the one observation occurrence.

    Evidence reached before a failure is retained and a bounded failure report is
    written before the failure propagates, so a clean guard census can never
    delete the only copy of an already-produced transcript, manifest, log,
    binary or engine output. The script still never concludes closure.
    """
    seams = seams or Seams()
    gaps = []
    reached = {"phase": "binding", "binding": None, "guard": None, "paths": None,
               "envelope": None, "stage_artifacts": {}, "oxide": None,
               "barge": None, "oxide_parse": None, "barge_parse": None}

    def finish(outcome, failure=None):
        """Retain what was reached and publish the report. Returns whether the
        report was published; the caller must not report success when it was
        not, so the guard can never close a status 0 with no observation."""
        binding = reached["binding"]
        guard = reached["guard"]
        paths = reached["paths"]
        if binding is None or guard is None or paths is None:
            # Nothing was written and no owned path exists, so there is nothing
            # to retain or to report against: the refusal itself is the record.
            return False
        published = True
        if not seams.exists(paths["report"]):
            try:
                retained = retain(seams, paths, reached["stage_artifacts"],
                                  (reached["oxide"], reached["barge"]),
                                  (reached["oxide_parse"], reached["barge_parse"]),
                                  gaps)
            except (Obstruction, OSError) as error:
                gaps.append(f"the retained-evidence collection failed: {error}")
                retained = {"root": paths["evidence_retained"], "bytes": 0,
                            "entries": [], "omitted": 0, "state": "failed",
                            "reason": str(error)}
            report = build_report(
                binding=binding, guard=guard, paths=paths,
                envelope=reached["envelope"] or {},
                oxide=reached["oxide"], barge=reached["barge"],
                oxide_parse=reached["oxide_parse"], barge_parse=reached["barge_parse"],
                retained=retained, outcome=outcome,
                closure=closure_record(paths["guard_receipt"]), gaps=gaps)
            try:
                seams.write_json(paths["report"], report)
            except OSError as error:
                say(f"observation: the report could not be published: {error}")
                published = False
            else:
                say(f"observation: {outcome['state']} report {paths['report']} "
                    f"(phase {outcome['phase']}); qualified=false")
        if failure is not None:
            say(f"observation: {type(failure).__name__} at {outcome['phase']}: {failure}")
        return published

    try:
        binding, _payload = read_binding(options.get("binding"),
                                         options.get("sha256"), seams)
        reached["binding"] = binding
        guard = guard_context(seams, environ)
        reached["guard"] = guard
        reached["phase"] = "source"
        facts = source_facts(binding, seams, guard["scratch_root"])
        envelope = verify_source_bindings(binding, facts, guard)
        reached["envelope"] = envelope
        paths = derive_paths(binding, guard)
        reached["paths"] = paths
        if seams.exists(paths["run_root"]) or seams.exists(paths["report"]):
            raise Obstruction(
                "this occurrence's root or report already exists; refusing")
        if seams.work_left() <= SHUTDOWN_MARGIN_SECONDS:
            raise Obstruction("the guard's deadline leaves no work window")
        require_reviewed_custody(binding)
        seams.make_root(paths["run_root"])
        seams.make_root(paths["oxide"])
        seams.make_root(paths["barge"])

        # The Barge archive is validated against the reviewed custody source
        # before either staging build, so no write to a staged tree precedes it.
        reached["phase"] = "barge-source"
        barge_prepared = prepare_barge(seams, binding, paths)

        reached["phase"] = "staging"
        oxide_staged = stage_oxide(seams, binding, paths, reached["stage_artifacts"])
        verify_toolchain_unchanged(binding, seams)
        barge_staged = stage_barge(seams, binding, paths, reached["stage_artifacts"],
                                   barge_prepared)
        verify_toolchain_unchanged(binding, seams)
        envelope["oxide"]["stage"] = oxide_staged
        envelope["barge"]["stage"] = barge_staged

        reached["phase"] = "oxide-window"
        oxide = run_oxide(seams, paths, oxide_staged)
        reached["oxide"] = oxide
        if oxide["engine"]["timed_out"]:
            gaps.append("the Oxide window did not finish in its bound; its partial "
                        "captured output is kept and its closure is unknown")
            raise UnknownOutcome("the Oxide window did not finish in its bound")
        if oxide["observation"] is None:
            gaps.append("Oxide wrote no observation report; only its engine exit and "
                        "output were collected")
        elif (oxide["observation"]["document"] or {}).get("qualified") is not False:
            gaps.append(
                "the Oxide observation document did not declare itself unqualified")

        reached["phase"] = "barge-window"
        barge = run_barge(seams, paths, barge_staged)
        reached["barge"] = barge
        if barge["engine"]["timed_out"]:
            gaps.append("the Barge window did not finish in its bound; its partial "
                        "captured output is kept and its closure is unknown")
            raise UnknownOutcome("the Barge window did not finish in its bound")
        reached["phase"] = "barge-custody"
        require_barge_custody_record(barge)

        reached["phase"] = "parse"
        if oxide["observation"] is not None:
            reached["oxide_parse"] = parse_capture(
                seams, oxide_staged, root=oxide["fixture"],
                transcript=os.path.join(oxide["fixture"], "fixture-base",
                                        "transcript.json"),
                manifest=os.path.join(oxide["fixture"], "fixture-base",
                                      "manifest.json"),
                report=os.path.join(oxide["fixture"], "parse.json"))
            oxide_parse = reached["oxide_parse"]
            if oxide_parse is None:
                gaps.append("Oxide retained no transcript; its side has no parser result")
            elif oxide_parse["report"] is None:
                gaps.append("the canonical parse pass wrote no report for the Oxide side")
            elif oxide_parse["report"]["document"] is None:
                gaps.append("the Oxide parse report is unparsable and was not repaired")

        barge_root = paths["barge_run"]
        reached["barge_parse"] = parse_capture(
            seams, oxide_staged, root=barge_root, transcript=barge["retained"],
            manifest=barge["manifest"],
            report=os.path.join(barge_root, "parse.json"))
        barge_parse = reached["barge_parse"]
        if barge_parse is None or barge_parse["report"] is None:
            raise Obstruction(
                "the canonical parse pass wrote no report for the Barge side; its kept "
                "transcript has no parser verdict")
        if barge_parse["report"]["document"] is None:
            gaps.append("the Barge parse report is unparsable and was not repaired")

    except Obstruction as error:
        finish({"state": "refused", "phase": reached["phase"], "reason": str(error)},
               error)
        raise
    except UnknownOutcome as error:
        finish({"state": "unknown", "phase": reached["phase"], "reason": str(error)},
               error)
        raise
    except BaseException as error:  # retain what exists before propagating
        finish({"state": "aborted", "phase": reached["phase"],
                "reason": f"{type(error).__name__}: {error}"}, error)
        raise

    if not finish({"state": "collected", "phase": "complete", "reason": None}):
        raise Obstruction(
            "the observation report could not be published; the guard must not "
            "record a closed success")
    return 0


# ---------------------------------------------------------------------------
# Plan
# ---------------------------------------------------------------------------


def plan_document():
    return {
        "schema": "oxide.capture.embedded-control-plan/1",
        "qualified": False,
        "claim": CLAIM,
        "route": f"python3 scripts/bounded-test.py --limit {LIMIT_SECONDS} "
                 f"--scratch-budget-mib {SCRATCH_MIB} -- "
                 "python3 scripts/tape-embedded-observation.py --observe "
                 "--binding \"$TAPE_OBSERVATION_BINDING\" "
                 "--sha256 \"$TAPE_OBSERVATION_BINDING_SHA256\"",
        "selection_refuses_before_effect": True,
        "binding_template": BINDING_TEMPLATE,
        "commands": {
            "oxide_stage": ["cargo", "test", "--test", "tape_prune_eval",
                            "--no-run", "--locked", "--offline",
                            "--message-format=json"],
            "oxide_engine": ["<staged tape_prune_eval>", "--observe-base", "--root",
                             "<run>/oxide/fixture", "--report-out",
                             "<run>/oxide/fixture/observation.json"],
            "barge_stage": ["cargo", "test", "-p", "barge", "--test", "tape_eval",
                            "--no-run", "--locked", "--offline",
                            "--message-format=json"],
            "barge_engine": ["<staged tape_eval>", "(no arguments)"],
            "parse_pass": ["<staged tape_prune_eval>", "--parse-capture", "--root",
                           "<run>/<side>", "--transcript", "<retained>",
                           "[--manifest <retained manifest>]",
                           "--report-out", "<run>/<side>/parse.json"],
            "engine_windows": "one per side, sequential, inside the guard's own group",
        },
        "controls": list(CONTROLS),
        "setup_differences": setup_differences(),
        "locations": {
            "binding_and_guard": "scripts/tape-embedded-observation.py "
                                 "parse_binding/read_binding/guard_context/"
                                 "source_facts/verify_source_bindings",
            "owned_paths": "scripts/tape-embedded-observation.py derive_paths "
                           "(guard roots only; no absolute path from a binding)",
            "staging": "scripts/tape-embedded-observation.py staging_environment/"
                       "stage/stage_oxide/stage_barge with "
                       "scripts/offline_seed.py seed_cargo_home",
            "oxide_fixture": "src/bin/oxide/tape_harness.rs Case::Base "
                             "(fixture_spec/prepare_case/build_fixture)",
            "oxide_engine": "src/bin/oxide/tape_harness.rs --observe-base "
                            "(engine_child, run_engine, observation_document, "
                            "then run_case for the case itself)",
            "oxide_parser": "src/bin/oxide/tape_harness.rs --parse-capture "
                            "(capture_wire parse_transcript/parse_manifest, "
                            "assess_completeness, "
                            "capture_verdict::evaluate_limited_capture)",
            "barge_engine": "crates/barge/tests/tape_eval.rs main -> "
                            "tape_harness::embedded_exercise -> run/run_bound "
                            "with embedded-unverified",
            "barge_fixture": "crates/barge/src/tape_harness.rs build_fixture/"
                             "build_fixture_setup/base_declaration/"
                             "fixture_oxide_records",
            "barge_retention": "crates/barge/src/tape_harness.rs retain_transcript "
                               "(prints barge.tape.exercise/1: retained <path>)",
            "supervisor": "scripts/bounded-test.py (qualified fcfda2b0)",
        },
        "checks_before_effect": [
            "the four guard controls are present and the supervisor's own bytes are "
            "the qualified digest",
            "the guard scratch root and evidence directory are canonical owner-only "
            "directories inside the selected volume",
            "the binding digest matches the supplied digest and every field is exact",
            "the Oxide commit, tree, lock digest and tracked worktree match the "
            "binding",
            "the Barge sibling commit, tree, lock digest and tracked worktree match "
            "the binding",
            "rustc and cargo are the bound paths, bytes and 1.97.1 version",
            "the guard's scratch volume is the bound Stash volume and not the boot "
            "volume",
            "the derived run root and report do not already exist and the work "
            "window exceeds the shutdown margin",
        ],
        "report_schema": {
            "schema": SCHEMA,
            "always_false": "qualified",
            "keys": ["claim", "planner_reference", "controls", "premises", "bounds",
                     "guard", "paths", "envelope", "windows", "fates",
                     "setup_differences", "closure", "gaps"],
        },
        "premises": [{"premise": name, "disposition": text}
                     for name, text in PREMISES],
        "bounds": {
            "limit_seconds": LIMIT_SECONDS,
            "scratch_mib": SCRATCH_MIB,
            "shutdown_margin_seconds": SHUTDOWN_MARGIN_SECONDS,
            "allocation_scratch_mib": {
                "oxide staging target in scratch (no debug info, incremental off)": 448,
                "barge staging target (14-package workspace)": 192,
                "two private seeded Cargo homes": 192,
                "engine windows: fixtures, transcripts, observation and parse reports": 32,
                "retained evidence copies (transcripts, manifest, logs, executables)": 416,
                "slack": 256,
            },
            "allocation_seconds": {
                "two cold sequential staging builds in scratch": 420,
                "two engine windows": 120,
                "two canonical parse passes": 20,
                "retained-evidence copies": 30,
                "two offline seeds at the seeder's own 45 s cumulative cap": 90,
                "shutdown margin": 60,
                "slack": 160,
            },
            "task_owned_measurements": {
                "source": "this task's own covering runs at f2a97f12 (guard receipts "
                          "retained under the Stash scratch evidence directories)",
                "rust:capture-wire guard receipts": "300 s limit, closed, status 0, "
                                                    "owned-byte peak 36 KiB to 3.7 MiB "
                                                    "(the build itself dogfoods the "
                                                    "pool, outside the guard scratch), "
                                                    "host free minimum about 2.87 TiB",
                "same target's own build+execute line": "18.3 s warm, quick profile",
                "rust:compile line": "12.67 s warm workspace check",
            },
            "basis": "the covering figures bound the wall clock only loosely because they "
                     "build in the pool; they are used as a warm lower bound, and the "
                     "staging figures are explicit allowances rather than measurements",
            "uncertainty": "the in-scratch staging trees are the largest uncertainty: "
                           "the repository's recorded in-scratch staging peaks for the "
                           "same oxide debug recipe are 179.3 and 182.7 MiB for a "
                           "binary build, and this route's test-target plus two seeds "
                           "plus retained copies are allowed roughly twice that with "
                           "slack. The diagnostic records its own measured owned bytes "
                           "and retained-evidence bytes in its report, so a later bound "
                           "revision can replace the allowance with a measurement",
        },
        "barge_custody": {
            "state": "source-bound at the reviewed revision; the one occurrence is not "
                     "activated and no engine, fixture or staging build has run",
            "reviewed": {
                "linked_task": BARGE_CUSTODY_REVIEWED["linked_task"],
                "review_run": BARGE_CUSTODY_REVIEWED["review_run"],
                "revision": BARGE_CUSTODY_REVIEWED["revision"],
                "tree": BARGE_CUSTODY_REVIEWED["tree"],
                "lock_sha256": BARGE_CUSTODY_REVIEWED["lock_sha256"],
                "files": dict(BARGE_CUSTODY_REVIEWED["files"]),
            },
            "pinned_sibling_merge": "d9eac3b573fb5d431e29160844290093030bfc19 (PR #49 "
                                    "merge; same tree as the reviewed revision, so tree "
                                    "identity is the source-equivalence proof)",
            "required_custody_property": BARGE_CUSTODY_PROPERTY,
            "opt_in": {"env": BARGE_CUSTODY_OPT_IN_ENV,
                       "value": BARGE_CUSTODY_OPT_IN_VALUE,
                       "root_env": BARGE_SCRATCH_ROOT_ENV,
                       "set_by": "run_barge, for the Barge child only; never inherited"},
            "child_environment": "the scrub drops every inherited BARGE_ key; the child "
                                 "receives only the opt-in and this run's own root; argv "
                                 "is the bound executable with no arguments",
            "source_gate": "require_reviewed_custody refuses while no reviewed revision "
                           "is bound in this source; verify_barge_custody then checks the "
                           "bound custody revision, the bound tree against the reviewed "
                           "tree, the property name, the archived lock and each lifetime "
                           "file against the reviewed digests, all before any staging",
            "implemented_at": "Barge 05b4b52 (merged d9eac3b): TempDir::keep, the "
                              "opt-in-gated retained branch of embedded_exercise, the "
                              "kept-fixture line, manifest-before-run",
            "barge_side_failures": "a missing, duplicate or unsafe custody line, a missing "
                                   "or unsafe manifest, a missing Barge parse report, a "
                                   "nonzero or signalled exit, a truncated output, an unsafe "
                                   "transcript path and a timed-out window are apparatus "
                                   "obstructions (retained, unqualified, nonzero), never "
                                   "fates. The Oxide side keeps its recorded-observation "
                                   "semantics for its own report and exit",
            "readiness_fields_for_the_planner": {
                "note": "issued only after this source is merged; none is pre-issued here "
                        "and no field below is a valid input yet",
                "oxide": "the merged head's commit, tree and Cargo.lock digest",
                "barge": {"sibling": "the authenticated read-only sibling checkout",
                          "commit": "d9eac3b573fb5d431e29160844290093030bfc19",
                          "tree": BARGE_CUSTODY_REVIEWED["tree"],
                          "lock_sha256": BARGE_CUSTODY_REVIEWED["lock_sha256"],
                          "custody": {"linked_task": BARGE_CUSTODY_REVIEWED["linked_task"],
                                      "reviewed_revision": BARGE_CUSTODY_REVIEWED["revision"],
                                      "property": BARGE_CUSTODY_PROPERTY}},
                "toolchain": "the bound rustc and cargo absolute paths and digests at 1.97.1",
                "guard": "the fixed limits and the unchanged supervisor digest",
                "volume": SELECTED_VOLUME_UUID,
                "planner_reference": "the Queue reference that activates this one occurrence",
            },
            "residual_risks": [
                "tree identity covers the reviewed source, but git archive is trusted to "
                "honour the tree; the sibling carries no .gitattributes",
                "the kept fixture stays in the monitored scratch until the guard's own "
                "closed-run cleanup; retained evidence is the manifest and transcript, "
                "not the fixture tree",
                "the staging, seed and engine allowances are unmeasured; the diagnostic "
                "records its own owned bytes",
            ],
            "fit": "unmeasured: the 900 s / 1536 MiB envelope is unchanged and was not "
                   "re-measured against the qualified supervisor",
        },
        "guard_topology": "the qualified supervisor's default owned-root layout: "
                          "<root>/scratch and <root>/evidence as siblings, evidence "
                          "created with the inherited umask (normally 0755), both "
                          "canonical and not group- or other-writable; an adopted "
                          "scratch root or an explicit evidence directory refuses",
        "toolchain_binding": "staging runs the bound cargo by absolute path with RUSTC "
                             "set to the bound rustc and PATH fixed to the system path, "
                             "and both tool digests are re-read after staging, so an "
                             "inherited cargo or rustc cannot build the reported "
                             "artifacts",
        "failure_retention": "retain and the report run on every exit, before any "
                             "failure propagates: a staging, engine or parse failure "
                             "still copies the evidence already produced and writes a "
                             "bounded report whose outcome names the phase and whose "
                             "closure says the guard's census, not this script, decides. "
                             "Each stage log and stderr path is registered before Cargo "
                             "is spawned, so a failed build's own output is retained; a "
                             "timed-out engine window returns the partial output it "
                             "produced, which is retained and recorded as unknown "
                             "closure; and an unpublished report is a nonzero "
                             "apparatus outcome, never a closed success",
        "artifact_safety": "every retained source is re-validated as a real, "
                           "non-symlink regular file (or directory) resolving inside "
                           "the owned run root before it is read; a symlinked or "
                           "outside path is recorded with its reason and never read",
        "retained_evidence": {
            "root": "<guard evidence>/<run_root_name>",
            "budget": {"aggregate_bytes": RETENTION_CAP_BYTES,
                       "per_file_bytes": RETENTION_FILE_CAP_BYTES},
            "contents": "transcripts, the emitted manifest, the observation and parse "
                        "reports, stage logs and stderr, both executables and both "
                        "engines' captured stdout/stderr",
            "omission": "an artifact past the budget is named and its scratch copy is "
                        "not promised past closure; nothing is summarised into a hash "
                        "and no missing artifact is replaced",
        },
        "awards": "nothing: no agreement, equivalence, timing, writer-closure, "
                  "export, deletion or deployment credit; a matching outcome is not "
                  "acceptance",
        "not_used": [
            "Oxide's no-argument and broad --diagnostic routes",
            "Barge's export-gated --fixture-root argv",
            "any image, mount, detach, sudo, trace or new host setting",
            "any live pool, account root, registry or cache payload input",
        ],
    }


def plan():
    say(json.dumps(plan_document(), indent=2, sort_keys=True))
    return 0


# ---------------------------------------------------------------------------
# Pure self-test
# ---------------------------------------------------------------------------


class FakeSeams(Seams):
    """Scripted effects for the pure controls. It never runs a child, reads a
    host fact or writes a file."""

    def __init__(self, script):
        self.script = script
        self.calls = []

    def now_ns(self):
        return 0

    def deadline_ns(self):
        return self.script.get("deadline_ns", 10 ** 12)

    def work_left(self):
        return self.script.get("work_left", 600)

    def run_logged(self, argv, **kwargs):
        self.calls.append(("run_logged", list(argv), dict(kwargs.get("env") or {})))
        return self.script.get("stage_exit", 0)

    def run_captured(self, argv, **kwargs):
        self.calls.append(("run_captured", list(argv),
                           dict(kwargs.get("env") or {})))
        if "--observe-base" in argv:
            label = "oxide"
        elif "--parse-capture" in argv:
            label = "parse"
        else:
            label = "barge"
        if self.script.get("raise_on") == label:
            raise UnknownOutcome(f"the {label} window did not finish in its bound")
        return dict(self.script[label])

    def query(self, argv, **kwargs):
        self.calls.append(("query", list(argv)))
        joined = " ".join(argv)
        value = self.script.get("queries", {}).get(joined)
        if value is None:
            return {"code": 1, "stdout": "", "stderr": "not scripted"}
        return dict(value)

    def volume_facts(self, path):
        return dict(self.script.get(
            "volume", {"uuid": SELECTED_VOLUME_UUID, "mount": "/Volumes/Stash",
                       "boot": False}))

    def read_bytes(self, path):
        self.calls.append(("read_bytes", path))
        files = self.script.get("files", {})
        if path in files:
            return files[path]
        for key, value in files.items():
            if key in path:
                return value
        raise OSError(f"not scripted: {path}")

    def read_text(self, path):
        self.calls.append(("read_text", path))
        texts = self.script.get("texts", {})
        for key, value in texts.items():
            if key in path:
                return value
        raise OSError(f"not scripted: {path}")

    def exists(self, path):
        return path in self.script.get("exists", set())

    def isfile(self, path):
        return (path in self.script.get("isfiles", set())
                and path not in self.script.get("missing", set()))

    def isdir(self, path):
        return path in self.script.get("isdirs", set())

    def mode(self, path):
        return self.script.get("modes", {}).get(path, 0o700)

    def size(self, path):
        matches = [(key, value) for key, value in
                   self.script.get("sizes", {}).items() if key in path]
        if not matches:
            return 1
        return max(matches, key=lambda entry: len(entry[0]))[1]

    def sha256(self, path):
        self.calls.append(("sha256", path))
        matches = [(key, value) for key, value in self.script.get("digests", {}).items()
                   if key in path]
        if not matches:
            return "0" * 64
        return max(matches, key=lambda entry: len(entry[0]))[1]

    def make_root(self, path):
        self.calls.append(("make_root", path))

    def make_dirs(self, path):
        self.calls.append(("make_dirs", path))

    def file_kind(self, path):
        if path in self.script.get("missing", set()):
            raise OSError(f"not scripted: {path}")
        kinds = self.script.get("kinds", {})
        if path in kinds:
            return dict(kinds[path])
        return {"regular": True, "dir": False, "symlink": False}

    def copy(self, source, destination):
        self.calls.append(("copy", source))

    def write_text(self, path, text):
        self.calls.append(("write_text", path))

    def write_json(self, path, document):
        self.calls.append(("write_json", path))
        if self.script.get("write_fail"):
            raise OSError("the report destination is unwritable")
        self.script["written"] = document

    def extract(self, sibling, commit, destination):
        self.calls.append(("extract", sibling))
        return {"entries": 1, "bytes": 1}

    def seed(self, host_cargo_home, cargo_home, lock_bytes, deadline_ns):
        self.calls.append(("seed", cargo_home))
        return {"lock_sha256": sha256_bytes(lock_bytes), "packages": 0,
                "aggregate_bytes": 0}


def fixture_facts():
    return {
        "oxide": {"commit": "1" * 40, "tree": "2" * 40,
                  "lock_sha256": "3" * 64, "status": ""},
        "barge": {"commit": "4" * 40, "tree": "5" * 40,
                  "lock_sha256": "6" * 64, "status": ""},
        "toolchain": {"rustc": "/rust/rustc", "cargo": "/rust/cargo",
                      "rustc_sha256": "7" * 64, "cargo_sha256": "8" * 64,
                      "version_text": "rustc 1.97.1 (fixture)"},
        "volume": {"uuid": SELECTED_VOLUME_UUID, "mount": "/Volumes/Stash",
                   "boot": False},
    }


def fixture_binding(facts, *, custody=None):
    document = json.loads(json.dumps(BINDING_TEMPLATE))
    document["planner_reference"] = "290-fixture"
    document["oxide"] = {"commit": facts["oxide"]["commit"],
                         "tree": facts["oxide"]["tree"],
                         "lock_sha256": facts["oxide"]["lock_sha256"]}
    document["barge"] = {"sibling": "/siblings/barge",
                         "commit": facts["barge"]["commit"],
                         "tree": facts["barge"]["tree"],
                         "lock_sha256": facts["barge"]["lock_sha256"],
                         "custody": custody or {
                             "linked_task": "barge-custody-fixture",
                             "reviewed_revision": facts["barge"]["commit"],
                             "property": BARGE_CUSTODY_PROPERTY,
                         }}
    document["toolchain"].update({
        "rustc": "/rust/rustc", "rustc_sha256": facts["toolchain"]["rustc_sha256"],
        "cargo": "/rust/cargo", "cargo_sha256": facts["toolchain"]["cargo_sha256"],
    })
    return document


GUARD_ROOT = "/Volumes/Stash/scratch/oxide/guard-run"
FIXTURE_ENV = {
    "OXIDE_TASK_SCRATCH_ROOT": GUARD_ROOT + "/scratch",
    "OXIDE_PROOF_EVIDENCE": GUARD_ROOT + "/evidence",
    "OXIDE_PROOF_RUN_ID": "fixture",
    "OXIDE_PROOF_DEADLINE_NS": str(10 ** 12),
}

FIXTURE_GUARD = {
    "root": GUARD_ROOT,
    "scratch_root": GUARD_ROOT + "/scratch",
    "evidence": GUARD_ROOT + "/evidence",
    "run_id": "fixture", "deadline_ns": 10 ** 12,
    "supervisor_sha256": SUPERVISOR_SHA256,
}


def scripted_occurrence(binding, facts, *, barge_stderr, stage_exit=0,
                        sizes=None, extra_isfiles=()):
    """One fully scripted occurrence: no real file, child or host read."""
    run_root = os.path.join(FIXTURE_GUARD["scratch_root"], "tape-observation")
    oxide_root = os.path.join(run_root, "oxide", "fixture")
    barge_run = os.path.join(run_root, "barge", "run")
    barge_source = os.path.join(run_root, "barge", "source")
    retained = os.path.join(barge_run, "barge-test-x.transcript.json")
    kept_fixture = os.path.join(barge_run, "barge-test-x")
    kept_manifest = os.path.join(kept_fixture, "manifest.json")
    transcript = os.path.join(oxide_root, "fixture-base", "transcript.json")
    manifest = os.path.join(oxide_root, "fixture-base", "manifest.json")
    observation_report = os.path.join(oxide_root, "observation.json")
    oxide_parse_report = os.path.join(oxide_root, "parse.json")
    barge_parse_report = os.path.join(barge_run, "parse.json")
    oxide_executable = os.path.join(run_root, "oxide", "stage", "target", "debug",
                                    "deps", "tape_prune_eval-1")
    barge_executable = os.path.join(run_root, "barge", "stage", "target", "debug",
                                    "deps", "tape_eval-1")
    oxide_log = os.path.join(run_root, "oxide", "stage", "stage.json")
    oxide_err = os.path.join(run_root, "oxide", "stage", "stage.stderr")
    barge_log = os.path.join(run_root, "barge", "stage", "stage.json")
    barge_err = os.path.join(run_root, "barge", "stage", "stage.stderr")
    execution = {
        "code": 0, "signal": None, "stdout": "", "stderr": "",
        "stdout_truncated": False, "stderr_truncated": False, "timed_out": False,
    }

    def artifact(target, executable):
        return json.dumps({"reason": "compiler-artifact",
                           "target": {"name": target},
                           "executable": executable})

    files = {
        "/binding.json": json.dumps(binding).encode(),
        "Cargo.lock": b"",
        os.path.join(barge_source, "crates", "barge", "src", "testkit.rs"): b"",
        os.path.join(barge_source, "crates", "barge", "src",
                     "tape_harness.rs"): b"",
        observation_report: json.dumps(
            {"schema": "oxide.capture.embedded-observation/1",
             "qualified": False}).encode(),
        oxide_parse_report: json.dumps(
            {"schema": "oxide.capture.embedded-capture-parse/1",
             "qualified": False,
             "outcome": {"state": "completed"},
             "parser": {"transcript": {"state": "parsed", "reason": None}}},
            sort_keys=True).encode(),
        barge_parse_report: json.dumps(
            {"schema": "oxide.capture.embedded-capture-parse/1",
             "qualified": False,
             "outcome": {"state": "refused", "reason": "bounded capture refusal"},
             "parser": {"transcript": {"state": "parsed", "reason": None}}},
            sort_keys=True).encode(),
    }
    return {
        "deadline_ns": 10 ** 12,
        "work_left": 600,
        "stage_exit": stage_exit,
        "oxide": dict(execution),
        "barge": dict(execution, stderr=barge_stderr),
        "parse": dict(execution),
        "queries": {
            f"git -C {REPO} rev-parse HEAD": {
                "code": 0, "stdout": facts["oxide"]["commit"], "stderr": ""},
            f"git -C {REPO} rev-parse HEAD^{{tree}}": {
                "code": 0, "stdout": facts["oxide"]["tree"], "stderr": ""},
            f"git -C {REPO} status --porcelain --untracked-files=no": {
                "code": 0, "stdout": "", "stderr": ""},
            "git -C /siblings/barge rev-parse HEAD": {
                "code": 0, "stdout": facts["barge"]["commit"], "stderr": ""},
            "git -C /siblings/barge rev-parse HEAD^{tree}": {
                "code": 0, "stdout": facts["barge"]["tree"], "stderr": ""},
            "git -C /siblings/barge status --porcelain --untracked-files=no": {
                "code": 0, "stdout": "", "stderr": ""},
            "/rust/rustc --version": {
                "code": 0, "stdout": facts["toolchain"]["version_text"], "stderr": ""},
        },
        "digests": {
            "/siblings/barge/Cargo.lock": "6" * 64,
            "barge/source/Cargo.lock": "6" * 64,
            "Cargo.lock": "3" * 64,
            "rustc": "7" * 64,
            "cargo": "8" * 64,
            "testkit.rs": "a" * 64,
            "tape_harness.rs": "b" * 64,
            "tape_eval.rs": "c" * 64,
            "bounded-test.py": SUPERVISOR_SHA256,
        },
        "volume": facts["volume"],
        "kinds": {kept_fixture: {"regular": False, "dir": True, "symlink": False}},
        "exists": {"/siblings/barge"},
        "isdirs": {GUARD_ROOT, FIXTURE_GUARD["scratch_root"],
                   FIXTURE_GUARD["evidence"], kept_fixture,
                   os.path.join(run_root, "oxide"), os.path.join(run_root, "barge")},
        "modes": {GUARD_ROOT: 0o700, FIXTURE_GUARD["scratch_root"]: 0o700,
                  FIXTURE_GUARD["evidence"]: 0o755},
        "isfiles": {retained, transcript, manifest, observation_report,
                    oxide_parse_report, barge_parse_report,
                    oxide_executable, barge_executable, oxide_log, barge_log,
                    oxide_err, barge_err,
                    kept_manifest, "/rust/rustc", "/rust/cargo",
                    os.path.join(barge_source, "crates", "barge", "src",
                                 "testkit.rs"),
                    os.path.join(barge_source, "crates", "barge", "src",
                                 "tape_harness.rs"),
                    os.path.join(barge_source, "crates", "barge", "tests",
                                 "tape_eval.rs"),
                    os.path.join(barge_source, "Cargo.lock")} | set(extra_isfiles),
        "texts": {
            "oxide/stage/stage.json": artifact("tape_prune_eval", oxide_executable),
            "barge/stage/stage.json": artifact("tape_eval", barge_executable),
            "testkit.rs": "impl TempDir {\n    pub fn keep(mut self) -> PathBuf {}\n}\n",
            "tape_harness.rs": "fn embedded_exercise() {\n"
                               "    let kept = fixture.dir.keep();\n"
                               "    std::fs::write(dir.join(\"manifest.json\"), &fixture.manifest);\n"
                               "}\n",
        },
        "files": files,
        "sizes": sizes or {},
        "written": None,
    }


def self_test():
    """Pure apparatus controls: no engine, compiler, fixture, image or native
    campaign runs. Each control names the exact refusal it must produce."""
    checks = []

    def check(name, condition, detail=""):
        checks.append((name, bool(condition), detail))

    def refuses(name, call, needle=None):
        try:
            call()
        except Obstruction as error:
            check(name, needle is None or needle in str(error), str(error))
        except Exception as error:  # noqa: BLE001 - an unexpected class is a failure
            check(name, False, f"unexpected {type(error).__name__}: {error}")
        else:
            check(name, False, "no refusal")

    # 1. Exactly one control per side, distinct case IDs, nothing can add one.
    check("two controls, one per engine", len(CONTROLS) == 2
          and {entry["engine"] for entry in CONTROLS} == {"oxide", "barge"})
    check("case ids stay distinct", CONTROLS[0]["case"] == "BASE"
          and CONTROLS[1]["case"] == "CTL-0")
    plan_doc = plan_document()
    check("the plan is a distinct unqualified document",
          plan_doc["schema"] == "oxide.capture.embedded-control-plan/1"
          and plan_doc["qualified"] is False
          and plan_doc["binding_template"]["schema"] == BINDING_SCHEMA)
    check("the plan has one engine window per side",
          plan_doc["commands"]["engine_windows"].endswith("inside the guard's own group")
          and len(plan_doc["controls"]) == 2)
    check("the plan carries an allocation and its uncertainty",
          sum(plan_doc["bounds"]["allocation_scratch_mib"].values()) <= SCRATCH_MIB
          and sum(plan_doc["bounds"]["allocation_seconds"].values()) <= LIMIT_SECONDS
          and plan_doc["bounds"]["task_owned_measurements"]
          and plan_doc["bounds"]["uncertainty"])
    check("the plan records the source-bound Barge custody readiness",
          plan_doc["barge_custody"]["state"].startswith("source-bound")
          and plan_doc["barge_custody"]["reviewed"]["revision"]
          == BARGE_CUSTODY_REVIEWED["revision"]
          and plan_doc["barge_custody"]["required_custody_property"]
          == BARGE_CUSTODY_PROPERTY
          and plan_doc["barge_custody"]["opt_in"]["value"] == BARGE_CUSTODY_OPT_IN_VALUE
          and "not this script" in plan_doc["failure_retention"])
    check("the plan records the guard topology and the toolchain binding",
          "scratch" in plan_doc["guard_topology"]
          and plan_doc["toolchain_binding"].startswith("staging runs the bound cargo"))

    # 2. Binding: malformed, missing and drifted documents refuse.
    facts = fixture_facts()
    binding = fixture_binding(facts)
    binding_bytes = json.dumps(binding).encode()
    refuses("missing binding path and digest",
            lambda: read_binding("", "", FakeSeams({})), "required")
    refuses("non-hex binding digest",
            lambda: read_binding("/binding.json", "z" * 64, FakeSeams({})), "SHA-256")
    refuses("a binding digest mismatch",
            lambda: read_binding("/binding.json", "0" * 64,
                                 FakeSeams({"files": {"/binding.json": binding_bytes}})),
            "not the supplied")
    for mutated, needle in (
        ({"schema": "oxide.capture.embedded-control-binding/2"}, "current schema"),
        ({"planner_reference": ""}, "planner reference"),
        ({"oxide": {"commit": "1" * 39}}, "not exact"),
        ({"barge": {"sibling": "relative/path"}}, "absolute path"),
        ({"toolchain": {"rustc": "rustc"}}, "not absolute"),
        ({"toolchain": {"version": "1.98.1"}}, "toolchain is not"),
        ({"guard": {"supervisor_sha256": "0" * 64}}, "qualified supervisor"),
        ({"guard": {"limit_seconds": 1}}, "fixed bounds"),
        ({"guard": {"scratch_mib": 4096}}, "fixed bounds"),
        ({"volume": {"uuid": "00000000-0000-0000-0000-000000000000"}},
         "Stash volume"),
        ({"paths": {"run_root_name": "../escape"}}, "safe component"),
        ({"paths": {"report_name": "a/b"}}, "safe component"),
    ):
        candidate = json.loads(json.dumps(binding))
        for key, value in mutated.items():
            if isinstance(value, dict) and isinstance(candidate.get(key), dict):
                candidate[key].update(value)
            else:
                candidate[key] = value
        refuses(f"binding refuses {sorted(mutated)[0]}",
                lambda candidate=candidate: parse_binding(
                    json.dumps(candidate).encode()), needle)
    check("the fixed binding parses",
          parse_binding(binding_bytes)["planner_reference"] == "290-fixture")
    refuses("non-JSON binding", lambda: parse_binding(b"{"), "not JSON")
    missing_custody = fixture_binding(facts)
    del missing_custody["barge"]["custody"]
    refuses("a binding without the Barge custody revision", lambda: parse_binding(
        json.dumps(missing_custody).encode()), "reviewed Barge custody revision")
    for custody, needle in (
        ({"linked_task": "x", "reviewed_revision": "4" * 39,
          "property": BARGE_CUSTODY_PROPERTY}, "not exact"),
        ({"linked_task": "", "reviewed_revision": "4" * 40,
          "property": BARGE_CUSTODY_PROPERTY}, "linked Barge custody task"),
        ({"linked_task": "x", "reviewed_revision": "4" * 40,
          "property": "anything"}, "required custody property"),
    ):
        refuses("a binding with an unusable Barge custody block",
                lambda custody=custody: parse_binding(json.dumps(
                    fixture_binding(facts, custody=custody)).encode()), needle)

    # 3. Source, toolchain, volume, worktree and guard drift all refuse.
    envelope = verify_source_bindings(binding, facts, FIXTURE_GUARD)
    check("matching source facts bind",
          envelope["oxide"]["commit"] == "1" * 40
          and envelope["guard"]["supervisor_sha256"] == SUPERVISOR_SHA256)
    for side, key in (("oxide", "commit"), ("oxide", "tree"),
                      ("oxide", "lock_sha256"), ("barge", "commit"),
                      ("barge", "tree"), ("barge", "lock_sha256")):
        drifted = fixture_facts()
        drifted[side][key] = "9" * len(drifted[side][key])
        refuses(f"{side} {key} drift",
                lambda drifted=drifted: verify_source_bindings(
                    binding, drifted, FIXTURE_GUARD), "not the bound")
    for mutation, needle in (
        (("status", " M src/main.rs"), "not clean"),
        (("toolchain", "rustc_sha256", "9" * 64), "bytes changed"),
        (("toolchain", "cargo", "/other/cargo"), "path moved"),
        (("toolchain", "version_text", "rustc 1.96.1"), "not 1.97.1"),
        (("volume", "uuid", "00000000-0000-0000-0000-000000000000"), "scratch volume"),
        (("volume", "boot", True), "boot volume"),
        (("volume", "mount", "/"), "boot volume"),
    ):
        mutated = fixture_facts()
        if mutation[0] == "status":
            mutated["oxide"]["status"] = mutation[1]
        elif mutation[0] == "toolchain":
            mutated["toolchain"][mutation[1]] = mutation[2]
        else:
            mutated["volume"][mutation[1]] = mutation[2]
        refuses(f"binding drift in {mutation[0]} {mutation[1]}",
                lambda mutated=mutated: verify_source_bindings(
                    binding, mutated, FIXTURE_GUARD), needle)

    # 4. Owned paths derive only from the guard's own roots.
    paths = derive_paths(binding, FIXTURE_GUARD)
    check("paths stay under the guard roots",
          paths["run_root"] == os.path.join(FIXTURE_GUARD["scratch_root"],
                                            "tape-observation")
          and paths["report"] == os.path.join(FIXTURE_GUARD["evidence"],
                                              REPORT_NAME)
          and paths["evidence_retained"] == os.path.join(FIXTURE_GUARD["evidence"],
                                                         "tape-observation"))
    refuses("a missing guard context", lambda: guard_context(
        FakeSeams({"digests": {}}),
        {"OXIDE_TASK_SCRATCH_ROOT": "", "OXIDE_PROOF_EVIDENCE": "",
         "OXIDE_PROOF_RUN_ID": "", "OXIDE_PROOF_DEADLINE_NS": ""}),
        "qualified shared supervisor")

    # 5. The guard's actual default sibling topology is the one accepted.
    topology = FakeSeams({
        "digests": {"bounded-test.py": SUPERVISOR_SHA256},
        "isdirs": {GUARD_ROOT, FIXTURE_GUARD["scratch_root"],
                   FIXTURE_GUARD["evidence"],
                   GUARD_ROOT + "/scratch/supervisor-evidence.x"},
        "modes": {GUARD_ROOT: 0o700, FIXTURE_GUARD["scratch_root"]: 0o700,
                  FIXTURE_GUARD["evidence"]: 0o755,
                  GUARD_ROOT + "/scratch/supervisor-evidence.x": 0o700},
    })
    context = guard_context(topology, FIXTURE_ENV)
    check("the real sibling layout is accepted, evidence at 0755 included",
          context["root"] == GUARD_ROOT
          and context["evidence"] == FIXTURE_GUARD["evidence"])
    refuses("a nested evidence directory is not this topology",
            lambda: guard_context(topology, dict(
                FIXTURE_ENV,
                OXIDE_PROOF_EVIDENCE=FIXTURE_GUARD["scratch_root"])),
            "sibling layout")
    refuses("an adopted scratch root's evidence directory is refused",
            lambda: guard_context(topology, dict(
                FIXTURE_ENV,
                OXIDE_PROOF_EVIDENCE=GUARD_ROOT + "/scratch/supervisor-evidence.x")),
            "sibling layout")
    writable = FakeSeams({
        "digests": {"bounded-test.py": SUPERVISOR_SHA256},
        "isdirs": {GUARD_ROOT, FIXTURE_GUARD["scratch_root"],
                   FIXTURE_GUARD["evidence"]},
        "modes": {GUARD_ROOT: 0o700, FIXTURE_GUARD["scratch_root"]: 0o777,
                  FIXTURE_GUARD["evidence"]: 0o755},
    })
    refuses("a group- or other-writable scratch root is refused",
            lambda: guard_context(writable, FIXTURE_ENV), "other-writable")
    refuses("a non-canonical scratch root is refused",
            lambda: guard_context(topology, dict(
                FIXTURE_ENV, OXIDE_TASK_SCRATCH_ROOT=GUARD_ROOT + "/scratch/../scratch")),
            "not canonical")
    refuses("a supervisor digest mismatch is refused",
            lambda: guard_context(FakeSeams({
                "digests": {"bounded-test.py": "0" * 64},
                "isdirs": {GUARD_ROOT, FIXTURE_GUARD["scratch_root"],
                           FIXTURE_GUARD["evidence"]},
                "modes": {GUARD_ROOT: 0o700, FIXTURE_GUARD["scratch_root"]: 0o700,
                          FIXTURE_GUARD["evidence"]: 0o755}}), FIXTURE_ENV),
            "not the qualified")

    # 6. Retention: only a path inside the caller's own root, never by mtime.
    check("retention inside the root is accepted",
          retained_inside("/run/barge", "/run/barge/x.transcript.json")
          == "/run/barge/x.transcript.json")
    check("retention outside the root is refused",
          retained_inside("/run/barge", "/elsewhere/x.transcript.json") is None)
    check("retention through a parent escape is refused",
          retained_inside("/run/barge", "/run/barge/../x.json") is None)
    check("retention of the root itself is refused",
          retained_inside("/run/barge", "/run/barge") is None)
    check("no reported path selects nothing",
          retained_inside("/run/barge", None) is None)

    # 7. Classification keeps apparatus, refusal, raw exit and closure apart.
    check("zero exit completes", classification({"code": 0})["state"] == "completed")
    check("a raw nonzero exit is kept",
          classification({"code": 101})["state"] == "nonzero-exit")
    check("a reported bounded refusal has its own state",
          classification({"code": 2, "stderr": BOUNDED_REFUSAL_MARKER + ": x"},
                         bounded_refusal_marker=BOUNDED_REFUSAL_MARKER)
          ["state"] == "bounded-capture-refusal")
    check("a timeout is unknown closure",
          classification({"code": None, "timed_out": True})["state"] == "unknown")
    check("a missing window is not-run",
          classification(None)["state"] == "not-run")

    # 8. Equal and unequal raw fates are both unqualified and record no claim.
    def report_with(barge_outcome):
        return build_report(
            binding=binding, guard=FIXTURE_GUARD, paths=paths, envelope=envelope,
            oxide=None, barge=None,
            oxide_parse={"report": {"document": {"outcome": {"state": "completed"}}}},
            barge_parse={"report": {"document": {"outcome": barge_outcome}}},
            retained={"root": paths["evidence_retained"], "bytes": 0, "entries": [],
                      "omitted": 0},
            outcome={"state": "collected", "phase": "complete", "reason": None},
            closure=closure_record(paths["guard_receipt"]), gaps=[])
    equal = report_with({"state": "completed"})
    unequal = report_with({"state": "refused", "reason": "bounded"})
    for label, report in (("equal", equal), ("unequal", unequal)):
        check(f"{label} fates stay unqualified", report["qualified"] is False)
        for absent in ("agreement", "equivalence", "divergence"):
            check(f"{label} fates record no {absent}",
                  absent not in report and absent not in report["fates"])
        check(f"{label} fates record no comparison claim",
              report["fates"]["comparison"].startswith("not-claimed")
              and "comparison" not in report)
    check("unequal fates are raw observations",
          unequal["fates"]["barge"]["outcome"]["state"] == "refused")
    check("missing parses are absent, not empty", fate_of(None)["state"] == "absent")
    check("an unparsable document is not repaired",
          fate_of({"report": {"document": None, "unparsable": "bad"}})["state"]
          == "unparsable")
    junk = FakeSeams({"files": {"junk.json": b"{"}})
    broken = read_document(junk, "junk.json")
    check("a broken retained document keeps its absence",
          broken["document"] is None and "unparsable" in broken)

    # 9. Caps and truncation are visible, never a complete-looking document.
    capped = build_report(binding=binding, guard=FIXTURE_GUARD, paths=paths,
                          envelope=envelope, oxide=None, barge=None,
                          oxide_parse=None, barge_parse=None,
                          retained={"root": paths["evidence_retained"], "bytes": 0,
                                    "entries": [], "omitted": 0},
                          outcome={"state": "collected", "phase": "complete",
                                   "reason": None},
                          closure=closure_record(paths["guard_receipt"]),
                          gaps=[f"g{index}" for index in range(GAP_CAP + 3)])
    check("gap overflow is counted", len(capped["gaps"]) == GAP_CAP + 1
          and "further gap(s) omitted" in capped["gaps"][-1])

    # 10. Refusals happen before any engine.
    fake = FakeSeams({"files": {}})
    refuses("no binding refuses before any effect",
            lambda: observe({"binding": "", "sha256": ""}, fake, FIXTURE_ENV),
            "required")
    check("no engine or root was touched by the refused occurrence",
          not any(call[0] in ("run_captured", "make_root", "run_logged")
                  for call in fake.calls))
    spent = scripted_occurrence(binding, facts, barge_stderr="")
    spent["work_left"] = 1
    fake = FakeSeams(spent)
    refuses("a spent work window refuses before staging",
            lambda: observe({"binding": "/binding.json",
                             "sha256": sha256_bytes(binding_bytes)}, fake,
                            FIXTURE_ENV),
            "no work window")
    check("a spent window staged and ran nothing", not any(
        call[0] in ("run_logged", "run_captured", "make_dirs") for call in fake.calls))
    check("a spent window still writes a bounded refusal report",
          spent["written"] is not None
          and spent["written"]["outcome"]["state"] == "refused"
          and spent["written"]["outcome"]["phase"] == "source")
    # 11. The source gate binds the reviewed Barge custody revision. The shipped
    #     constant is the Barge049 review at 05b4b52 (tree 68f96990, run 1808bf0e);
    #     the fixture binding is not that source and refuses before any staging.
    barge_source = paths["barge_source"]
    check("the source binds the reviewed Barge custody revision and tree",
          BARGE_CUSTODY_REVIEWED["revision"] == "05b4b5290852a0c9c697005794f1fda78f1b4420"
          and BARGE_CUSTODY_REVIEWED["tree"] == "68f96990d6e190852fe61e298d76b6aebdb31076"
          and BARGE_CUSTODY_REVIEWED["review_run"] == "1808bf0e-51ed-4b45-8470-f29222b40086"
          and set(BARGE_CUSTODY_FILES) == set(BARGE_CUSTODY_REVIEWED["files"]))
    shipped = FakeSeams(scripted_occurrence(binding, facts, barge_stderr=""))
    refuses("the fixture binding is refused by the shipped reviewed source",
            lambda: verify_barge_custody(binding, barge_source, shipped),
            "not the reviewed revision")
    closed_fake = FakeSeams(scripted_occurrence(binding, facts, barge_stderr=""))
    original_custody = globals()["BARGE_CUSTODY_REVIEWED"]
    globals()["BARGE_CUSTODY_REVIEWED"] = None
    try:
        refuses("a closed source gate refuses with the owner-gated impediment",
                lambda: observe({"binding": "/binding.json",
                                 "sha256": sha256_bytes(binding_bytes)}, closed_fake,
                                FIXTURE_ENV),
                "owner-gated impediment")
    finally:
        globals()["BARGE_CUSTODY_REVIEWED"] = original_custody
    check("a closed gate runs no staging, archive or engine",
          not any(call[0] in ("run_logged", "run_captured") for call in closed_fake.calls)
          and not any(call[0] == "make_root" and call[1] == paths["run_root"]
                      for call in closed_fake.calls))

    # The fixture reviewed revision is the fixture binding's own revision, tree
    # and lock, so every refusal below is caused by the one value it varies.
    fixture_reviewed = {
        "linked_task": "barge-custody-fixture",
        "review_run": "fixture-review",
        "revision": facts["barge"]["commit"],
        "tree": facts["barge"]["tree"],
        "lock_sha256": facts["barge"]["lock_sha256"],
        "files": {"testkit": "a" * 64, "tape_harness": "b" * 64, "tape_eval": "c" * 64},
    }

    def custody_seams(*, digests=None, drop=()):
        script = scripted_occurrence(binding, facts, barge_stderr="")
        script["digests"].update(digests or {})
        script["isfiles"] = set(script["isfiles"]) - set(drop)
        return FakeSeams(script)

    globals()["BARGE_CUSTODY_REVIEWED"] = fixture_reviewed
    try:
        # 12. An exact reviewed revision, tree, lock and file set is accepted; a
        #     different commit with the reviewed tree is accepted by tree identity.
        check("an exact reviewed revision, tree, lock and file set is accepted",
              verify_barge_custody(binding, barge_source, custody_seams())
              ["reviewed_revision"] == facts["barge"]["commit"])
        merged = fixture_binding(facts)
        merged["barge"]["commit"] = "7" * 40
        check("a merge with the reviewed tree is the same source by tree identity",
              verify_barge_custody(merged, barge_source, custody_seams())
              ["reviewed_tree"] == facts["barge"]["tree"])
        for name, candidate, seams, needle in (
            ("a binding naming another custody revision",
             fixture_binding(facts, custody={
                 "linked_task": "barge-custody-fixture", "reviewed_revision": "9" * 40,
                 "property": BARGE_CUSTODY_PROPERTY}),
             custody_seams(), "not the reviewed revision"),
            ("a binding whose tree is not the reviewed tree",
             fixture_binding(facts), None, "not the reviewed custody tree"),
            ("a binding with another custody property",
             fixture_binding(facts, custody={
                 "linked_task": "barge-custody-fixture",
                 "reviewed_revision": facts["barge"]["commit"],
                 "property": "anything"}),
             custody_seams(), "required custody property"),
            ("an archived lock that is not the reviewed lock",
             binding, custody_seams(digests={"barge/source/Cargo.lock": "0" * 64}),
             "not the reviewed custody lock"),
            ("an archived lifetime file outside the reviewed digests",
             binding, custody_seams(digests={"testkit.rs": "d" * 64}),
             "not the reviewed custody revision"),
            ("an archived entry point that is missing",
             binding, custody_seams(drop={os.path.join(barge_source, "crates", "barge",
                                                       "tests", "tape_eval.rs")}),
             "has no crates"),
        ):
            if name == "a binding whose tree is not the reviewed tree":
                candidate["barge"]["tree"] = "8" * 40
                seams = custody_seams()
            refuses(name, lambda candidate=candidate, seams=seams: verify_barge_custody(
                candidate, barge_source, seams), needle)

        # The observe path: a wrong bound value refuses before any staging write.
        bad_tree = fixture_binding(facts)
        bad_tree["barge"]["tree"] = "8" * 40
        bad_tree_bytes = json.dumps(bad_tree).encode()
        bad_tree_fake = FakeSeams(scripted_occurrence(bad_tree, facts, barge_stderr=""))
        bad_tree_fake.script["files"]["/binding.json"] = bad_tree_bytes
        bad_tree_fake.script["queries"][f"git -C /siblings/barge rev-parse HEAD^{{tree}}"] \
            = {"code": 0, "stdout": "8" * 40, "stderr": ""}
        refuses("a wrong bound tree refuses the observation before any staging",
                lambda: observe({"binding": "/binding.json",
                                 "sha256": sha256_bytes(bad_tree_bytes)}, bad_tree_fake,
                                FIXTURE_ENV),
                "not the reviewed custody tree")
        check("a refused tree runs no staging or engine",
              not any(call[0] in ("run_logged", "run_captured")
                      for call in bad_tree_fake.calls))

        # 12b. The Barge child gets exactly the opt-in and this run's own root, and
        #      no other Barge variable survives the scrub. Its argv is the bound
        #      executable alone: no fixture root or export-gated argument.
        kept_path = os.path.join(paths["barge_run"], "barge-test-x")
        kept_manifest_path = os.path.join(kept_path, "manifest.json")
        retained_line = ("barge.tape.exercise/1: retained "
                         + os.path.join(paths["barge_run"], "barge-test-x.transcript.json"))
        kept_line = "barge.tape.exercise/1: kept fixture " + kept_path
        ok_stderr = BOUNDED_REFUSAL_MARKER + ": x\n" + retained_line + "\n" + kept_line + "\n"

        def barge_case(stderr, *, code=0, truncated=False, manifest=True):
            script = scripted_occurrence(binding, facts, barge_stderr=stderr)
            script["barge"]["code"] = code
            script["barge"]["stderr_truncated"] = truncated
            if not manifest:
                script["missing"] = {kept_manifest_path}
            return script

        env_fake = FakeSeams(barge_case(ok_stderr))
        code = observe({"binding": "/binding.json",
                        "sha256": sha256_bytes(binding_bytes)}, env_fake, FIXTURE_ENV)
        barge_calls = [call for call in env_fake.calls if call[0] == "run_captured"
                       and "--observe-base" not in call[1]
                       and "--parse-capture" not in call[1]]
        check("the Barge window runs once, with only the bound executable as argv",
              code == 0 and len(barge_calls) == 1
              and barge_calls[0][1] == [os.path.join(paths["barge"], "stage", "target",
                                                     "debug", "deps", "tape_eval-1")])
        barge_env = barge_calls[0][2] if barge_calls else {}
        check("the Barge child receives the exact opt-in and its own run root",
              barge_env.get(BARGE_CUSTODY_OPT_IN_ENV) == BARGE_CUSTODY_OPT_IN_VALUE
              and barge_env.get(BARGE_SCRATCH_ROOT_ENV) == paths["barge_run"])
        check("no other Barge variable reaches the child",
              sorted(name for name in barge_env if name.startswith("BARGE_"))
              == sorted([BARGE_CUSTODY_OPT_IN_ENV, BARGE_SCRATCH_ROOT_ENV]))
        check("the scrub drops an inherited Barge opt-in and root",
              scrubbed({"BARGE_TAPE_CUSTODY": "barge.tape.kept-fixture/1",
                        "BARGE_TEST_SCRATCH_ROOT": "/elsewhere", "PATH": "/p"})
              == {"PATH": "/p"})

        # 12c. Every missing, duplicate, partial or unsafe Barge custody fact is an
        #      apparatus obstruction: nothing is fated, the evidence is retained and
        #      the report is bounded, unqualified and written before the refusal.
        barge_refusals = (
            ("a duplicate kept-fixture line",
             barge_case(kept_line + "\n" + kept_line + "\n" + retained_line + "\n"),
             "kept-fixture lines"),
            ("a missing kept-fixture line",
             barge_case(BOUNDED_REFUSAL_MARKER + ": x\n" + retained_line + "\n"),
             "kept-fixture lines"),
            ("a nonzero Barge exit",
             barge_case(ok_stderr, code=1), "exited 1"),
            ("a publication failure before any transcript",
             barge_case(kept_line + "\n", code=1), "exited 1"),
            ("a missing manifest inside the kept fixture",
             barge_case(ok_stderr, manifest=False), "no safe manifest.json"),
            ("an output that exceeded its capture bound",
             barge_case(ok_stderr, truncated=True), "capture bound"),
            ("a kept fixture outside this run's root",
             barge_case(BOUNDED_REFUSAL_MARKER + ": x\n" + retained_line + "\n"
                        + "barge.tape.exercise/1: kept fixture /elsewhere/barge-test-x\n"),
             "not a safe directory"),
        )
        parse_missing = barge_case(ok_stderr)
        parse_missing["missing"] = {os.path.join(paths["barge_run"], "parse.json")}
        barge_refusals += (
            ("a missing Barge parse report",
             parse_missing, "no report for the Barge side"),
        )
        for name, script, needle in barge_refusals:
            fake = FakeSeams(script)
            refuses(f"a Barge window with {name} is an obstruction",
                    lambda fake=fake: observe({"binding": "/binding.json",
                                               "sha256": sha256_bytes(binding_bytes)},
                                              fake, FIXTURE_ENV),
                    needle)
            written = script["written"]
            check(f"a Barge window with {name} writes a bounded refused report",
                  written is not None and written["qualified"] is False
                  and written["outcome"]["state"] == "refused"
                  and written["outcome"]["phase"] == ("parse" if "parse report" in name
                                                      else "barge-custody")
                  and ("make_root", paths["evidence_retained"]) in fake.calls)

        # 13. A complete scripted occurrence: bound tools, retained raw
        #     evidence, one unqualified report, at most one window per engine.
        script = scripted_occurrence(
            binding, facts,
            barge_stderr=BOUNDED_REFUSAL_MARKER + ": x\n"
                         "barge.tape.exercise/1: retained "
                         + os.path.join(paths["barge_run"],
                                        "barge-test-x.transcript.json") + "\n"
                         "barge.tape.exercise/1: kept fixture "
                         + os.path.join(paths["barge_run"], "barge-test-x") + "\n")
        fake = FakeSeams(script)
        code = observe({"binding": "/binding.json",
                        "sha256": sha256_bytes(binding_bytes)}, fake, FIXTURE_ENV)
        written = script["written"]
        check("a scripted occurrence completes", code == 0 and written is not None)
        check("the written report is unqualified and claims nothing",
              written["qualified"] is False
              and written["outcome"]["state"] == "collected"
              and written["fates"]["comparison"].startswith("not-claimed")
              and written["controls"][0]["case"] == "BASE"
              and written["controls"][1]["case"] == "CTL-0")
        check("the report separates the script from guard closure",
              written["closure"]["script"] == "does-not-conclude"
              and written["closure"]["guard_receipt"]["closure"]
              == "not-read-by-this-script")
        check("each engine ran at most once",
              sum(1 for call in fake.calls
                  if call[0] == "run_captured" and "--observe-base" in call[1]) == 1
              and sum(1 for call in fake.calls
                      if call[0] == "run_captured" and len(call[1]) == 1) == 1)
        stages = [call for call in fake.calls if call[0] == "run_logged"]
        check("staging runs the bound cargo, not an inherited one",
              len(stages) == 2 and all(call[1][0] == "/rust/cargo" for call in stages))
        check("staging directs cargo to the bound rustc and a fixed PATH",
              all(call[2].get("RUSTC") == "/rust/rustc"
                  and call[2].get("PATH") == "/usr/bin:/bin:/usr/sbin:/sbin"
                  and call[2].get("CARGO_HOME", "").startswith(paths["run_root"])
                  for call in stages))
        check("the Barge fixture and manifest are kept",
              written["windows"]["barge"]["fixture_custody"]["fixture_kept"] is True
              and written["windows"]["barge"]["fixture_custody"]["manifest"] is not None)
        retained = written["retained"]
        labels = {entry["label"] for entry in retained["entries"]}
        check("raw evidence is retained in the guard's evidence subtree",
              retained["root"] == paths["evidence_retained"]
              and retained["omitted"] == 0
              and {"oxide.transcript", "oxide.manifest", "oxide.observation",
                   "oxide.parse", "barge.transcript", "barge.manifest",
                   "barge.parse", "oxide.executable", "barge.executable",
                   "oxide.stdout", "barge.stderr"} <= labels)
        check("retained copies stay inside the evidence subtree",
              all(entry.get("path", "").startswith(paths["evidence_retained"] + "/")
                  for entry in retained["entries"] if entry["retained"]))

        # 14. Honest retention omissions.
        capped_script = scripted_occurrence(
            binding, facts,
            barge_stderr="barge.tape.exercise/1: retained "
                         + os.path.join(paths["barge_run"],
                                        "barge-test-x.transcript.json") + "\n"
                         "barge.tape.exercise/1: kept fixture "
                         + os.path.join(paths["barge_run"], "barge-test-x") + "\n")
        capped_script["sizes"] = {"barge/stage/target": RETENTION_FILE_CAP_BYTES + 1}
        fake = FakeSeams(capped_script)
        observe({"binding": "/binding.json", "sha256": sha256_bytes(binding_bytes)},
                fake, FIXTURE_ENV)
        written = capped_script["written"]
        entries = {entry["label"]: entry for entry in written["retained"]["entries"]}
        check("an over-budget artifact is omitted and named, never summarised",
              entries["barge.executable"]["retained"] is False
              and any("not retained within the budget" in gap
                      for gap in written["gaps"])
              and written["qualified"] is False)
        check("the remaining raw evidence is still retained",
              entries["oxide.transcript"]["retained"] is True
              and entries["barge.transcript"]["retained"] is True)

        # 15. Failure paths retain what was reached and write a bounded report.
        timeout_script = scripted_occurrence(
            binding, facts, barge_stderr="barge: partial output before the stop\n")
        timeout_script["barge"] = dict(timeout_script["barge"], code=None,
                                       timed_out=True,
                                       stderr="barge: partial output before the stop\n")
        fake = FakeSeams(timeout_script)
        try:
            observe({"binding": "/binding.json",
                     "sha256": sha256_bytes(binding_bytes)}, fake, FIXTURE_ENV)
        except UnknownOutcome as error:
            check("a Barge timeout aborts the collection",
                  "did not finish" in str(error))
        failure = timeout_script["written"]
        check("a Barge timeout still writes a bounded failure report",
              failure is not None and failure["outcome"]["state"] == "unknown"
              and failure["outcome"]["phase"] == "barge-window"
              and failure["qualified"] is False)
        entries = {entry["label"]: entry for entry in failure["retained"]["entries"]}
        check("evidence reached before the Barge timeout is retained",
              entries["oxide.transcript"]["retained"] is True
              and entries["oxide.executable"]["retained"] is True
              and failure["retained"]["state"] == "created")
        check("a timed-out engine's partial output is kept, not discarded",
              failure["windows"]["barge"]["engine"]["timed_out"] is True
              and failure["windows"]["barge"]["outer"]["stderr"]
              == "barge: partial output before the stop\n"
              and entries["barge.stderr"]["retained"] is True
              and entries["barge.stdout"]["retained"] is True
              and any("partial captured output is kept" in gap
                      for gap in failure["gaps"]))
        check("the failure report separates the script from guard closure",
              failure["closure"]["script"] == "does-not-conclude"
              and failure["closure"]["guard_receipt"]["closure"]
              == "not-read-by-this-script")

        parse_script = scripted_occurrence(binding, facts, barge_stderr=ok_stderr)
        parse_script["raise_on"] = "parse"
        fake = FakeSeams(parse_script)
        try:
            observe({"binding": "/binding.json",
                     "sha256": sha256_bytes(binding_bytes)}, fake, FIXTURE_ENV)
        except UnknownOutcome:
            pass
        failure = parse_script["written"]
        entries = {entry["label"]: entry for entry in failure["retained"]["entries"]}
        check("a parse failure keeps both engine transcripts and reports",
              failure["outcome"]["phase"] == "parse"
              and entries["oxide.transcript"]["retained"] is True
              and entries["oxide.executable"]["retained"] is True)

        staged_bad = scripted_occurrence(binding, facts, barge_stderr=ok_stderr)
        staged_bad["stage_exit"] = 101
        fake = FakeSeams(staged_bad)
        refuses("a failing staging build refuses",
                lambda: observe({"binding": "/binding.json",
                                 "sha256": sha256_bytes(binding_bytes)}, fake,
                                FIXTURE_ENV),
                "staging exited 101")
        check("a failing staging build ran no engine",
              not any(call[0] == "run_captured" for call in fake.calls))

        stage_script = scripted_occurrence(binding, facts, barge_stderr=ok_stderr)
        stage_script["stage_exit"] = 101
        # The archive that prepare_barge validates stays present: the failure is
        # the staging build's, not the source's.
        stage_script["missing"] = set(stage_script["isfiles"]) - {
            os.path.join(paths["oxide"], "stage", "stage.json"),
            os.path.join(paths["oxide"], "stage", "stage.stderr"),
            os.path.join(paths["barge"], "stage", "stage.json"),
            os.path.join(paths["barge"], "stage", "stage.stderr"),
            "/rust/rustc", "/rust/cargo",
        } - {path for path in stage_script["isfiles"]
             if path.startswith(paths["barge_source"])}
        fake = FakeSeams(stage_script)
        try:
            observe({"binding": "/binding.json",
                     "sha256": sha256_bytes(binding_bytes)}, fake, FIXTURE_ENV)
        except Obstruction:
            pass
        failure = stage_script["written"]
        check("a staging failure still writes a refusal report",
              failure is not None and failure["outcome"]["state"] == "refused"
              and failure["outcome"]["phase"] == "staging"
              and failure["qualified"] is False)
        engine_labels = {"oxide.transcript", "oxide.manifest", "oxide.observation",
                         "oxide.parse", "barge.transcript", "barge.manifest",
                         "barge.parse", "oxide.executable", "barge.executable"}
        check("a staging failure claims no engine artifact it never produced",
              all(entry["retained"] is False
                  for entry in failure["retained"]["entries"]
                  if entry["label"] in engine_labels))
        entries = {entry["label"]: entry for entry in failure["retained"]["entries"]}
        check("a failed staging build keeps its own log and stderr",
              entries["oxide.stage.log"]["retained"] is True
              and entries["oxide.stage.stderr"]["retained"] is True
              and failure["outcome"]["phase"] == "staging")

        # 16. A symlinked or outside artifact is never read or copied.
        symlink_script = scripted_occurrence(
            binding, facts,
            barge_stderr="barge.tape.exercise/1: retained "
                         + os.path.join(paths["barge_run"],
                                        "barge-test-x.transcript.json") + "\n"
                         "barge.tape.exercise/1: kept fixture "
                         + os.path.join(paths["barge_run"], "barge-test-x") + "\n")
        symlink_script["kinds"][os.path.join(paths["barge_run"], "barge-test-x",
                                             "manifest.json")] = {
            "regular": True, "dir": False, "symlink": True}
        fake = FakeSeams(symlink_script)
        refuses("a symlinked Barge manifest is an obstruction, not read",
                lambda: observe({"binding": "/binding.json",
                                 "sha256": sha256_bytes(binding_bytes)}, fake,
                                FIXTURE_ENV),
                "no safe manifest.json")
        written = symlink_script["written"]
        entries = {entry["label"]: entry for entry in written["retained"]["entries"]}
        check("a symlinked Barge manifest is refused, not read",
              written["outcome"]["state"] == "refused"
              and written["windows"]["barge"]["fixture_custody"]["manifest"] is None
              and entries["barge.manifest"]["retained"] is False)
        outside_script = scripted_occurrence(
            binding, facts,
            barge_stderr="barge.tape.exercise/1: retained /elsewhere/x.json\n"
                         "barge.tape.exercise/1: kept fixture "
                         + os.path.join(paths["barge_run"], "barge-test-x") + "\n")
        fake = FakeSeams(outside_script)
        refuses("an outside retained transcript is an obstruction, not read",
                lambda: observe({"binding": "/binding.json",
                                 "sha256": sha256_bytes(binding_bytes)}, fake,
                                FIXTURE_ENV),
                "not a safe regular file")
        written = outside_script["written"]
        entries = {entry["label"]: entry for entry in written["retained"]["entries"]}
        check("an outside retained transcript is refused, not read",
              written["windows"]["barge"]["retained_transcript"]["retention_refused"]
              is True
              and written["outcome"]["state"] == "refused"
              and entries["barge.transcript"]["retained"] is False)

        # 17. An unwritable report is an apparatus failure, never a success.
        publish_script = scripted_occurrence(
            binding, facts,
            barge_stderr="barge.tape.exercise/1: retained "
                         + os.path.join(paths["barge_run"],
                                        "barge-test-x.transcript.json") + "\n"
                         "barge.tape.exercise/1: kept fixture "
                         + os.path.join(paths["barge_run"], "barge-test-x") + "\n")
        publish_script["write_fail"] = True
        fake = FakeSeams(publish_script)
        refuses("an unwritable report is an apparatus failure, not a success",
                lambda: observe({"binding": "/binding.json",
                                 "sha256": sha256_bytes(binding_bytes)}, fake,
                                FIXTURE_ENV),
                "could not be published")
        check("the failed publication still retained the reached evidence",
              ("make_root", paths["evidence_retained"]) in fake.calls
              and any(call[0] == "copy" for call in fake.calls)
              and publish_script["written"] is None)

        # 18. A changed bound tool during the run refuses.
        changed_tools = FakeSeams({"digests": {"rustc": "9" * 64, "cargo": "8" * 64}})
        refuses("a changed bound toolchain refuses",
                lambda: verify_toolchain_unchanged(binding, changed_tools),
                "changed while this occurrence was running")
    finally:
        globals()["BARGE_CUSTODY_REVIEWED"] = original_custody

    failures = [(name, detail) for name, passed, detail in checks if not passed]
    for name, passed, detail in checks:
        say(f"self-test: {name}: {'ok' if passed else 'FAILED ' + detail}")
    if failures:
        say(f"self-test: {len(failures)} control(s) failed of {len(checks)}")
        return 1
    say(f"self-test: {len(checks)} controls passed; no engine, compiler, fixture, "
        "image or native campaign ran")
    return 0


def main(argv):
    if argv == ["--plan"]:
        return plan()
    if argv == ["--self-test"]:
        return self_test()
    if argv and argv[0] == "--observe":
        options = {}
        index = 1
        while index < len(argv):
            if argv[index] not in ("--binding", "--sha256") or index + 1 >= len(argv):
                say(USAGE)
                return 2
            options[argv[index][2:]] = argv[index + 1]
            index += 2
        try:
            return observe(options)
        except Obstruction as error:
            print(f"observation: refused: {error}", file=sys.stderr)
            return 2
        except UnknownOutcome as error:
            print("observation: collection unknown; this script does not conclude "
                  f"closure and the guard's census decides it: {error}",
                  file=sys.stderr)
            return 2
        except BaseException as error:  # unexpected: retain, report, then fail
            print(f"observation: unexpected {type(error).__name__}: {error}",
                  file=sys.stderr)
            return 2
    say(USAGE)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
