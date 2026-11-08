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
  transcript under this run's own `BARGE_TEST_SCRATCH_ROOT`. Its export-gated
  `--fixture-root` argv is deliberately not used.

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
SCRATCH_MIB = 1024
SHUTDOWN_MARGIN_SECONDS = 60
ARCHIVE_ENTRY_CAP = 20000
ARCHIVE_BYTE_CAP = 128 * MIB
BOUNDED_REFUSAL_MARKER = "bounded refusal (not qualification)"
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

BINDING_TEMPLATE = {
    "schema": BINDING_SCHEMA,
    "planner_reference": "<the Queue reference that activates this ONE occurrence>",
    "oxide": {"commit": "<40 lowercase hex>", "tree": "<40 lowercase hex>",
              "lock_sha256": "<64 lowercase hex>"},
    "barge": {"sibling": "<absolute read-only Barge checkout>",
              "commit": "<40 lowercase hex>", "tree": "<40 lowercase hex>",
              "lock_sha256": "<64 lowercase hex>"},
    "toolchain": {"rustc": "<absolute path>", "rustc_sha256": "<64 lowercase hex>",
                  "cargo": "<absolute path>", "cargo_sha256": "<64 lowercase hex>",
                  "version": BUILDER},
    "guard": {"supervisor_sha256": SUPERVISOR_SHA256,
              "limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB},
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
        try:
            done = subprocess.run(argv, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, timeout=timeout_s)
        except subprocess.TimeoutExpired:
            raise UnknownOutcome(f"{argv[0]} did not finish in its bound")
        stdout = done.stdout.decode("utf-8", "replace")
        stderr = done.stderr.decode("utf-8", "replace")
        return {
            "code": done.returncode,
            "signal": None if done.returncode >= 0 else -done.returncode,
            "stdout": stdout[:TEXT_CAP],
            "stdout_truncated": len(stdout) > TEXT_CAP,
            "stderr": stderr[:TEXT_CAP],
            "stderr_truncated": len(stderr) > TEXT_CAP,
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
    """Every inherited product, tool and VCS setting is dropped first."""
    return {
        name: value
        for name, value in environ.items()
        if not name.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "RUSTC",
                                "GIT_"))
        and name not in ("CARGO", "CARGO_INCREMENTAL", "RUSTFLAGS",
                         "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN")
    }


def staging_environment(parent, environ=None):
    """The one staging environment: private HOME, Cargo home and temp, the
    invoking rustup home, the pinned toolchain and one frozen flag value."""
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
    """The qualified guard's own context. Nothing runs outside it."""
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
        if seams.mode(path) & 0o077:
            raise Obstruction(f"the guard's {label} is not owner-only: {path}")
    if os.path.commonpath([os.path.realpath(scratch), os.path.realpath(evidence)]) != \
            os.path.realpath(scratch):
        raise Obstruction("the guard's evidence directory is outside its scratch root")
    supervisor = seams.sha256(os.path.join(REPO, "scripts", "bounded-test.py"))
    if supervisor != SUPERVISOR_SHA256:
        raise Obstruction(
            f"the running supervisor bytes are {supervisor}, not the qualified "
            f"{SUPERVISOR_SHA256}")
    return {"scratch_root": scratch, "evidence": evidence, "run_id": run_id,
            "deadline_ns": int(deadline), "supervisor_sha256": supervisor}


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


def stage(seams, *, parent, cwd, lock_path, cargo_argv, target, label):
    """One bounded offline staging build into the owned root, then the exact
    reported executable and its digest. The staging tree is retained: only the
    guard's post-leader census can conclude descendant closure."""
    seams.make_dirs(parent)
    env = staging_environment(parent)
    try:
        lock_bytes = seams.read_bytes(lock_path)
    except OSError as error:
        raise Obstruction(f"the {label} lock is unreadable: {error}")
    seed = seams.seed(invoking_cargo_home(), env["CARGO_HOME"], lock_bytes,
                      seams.deadline_ns())
    out_path = os.path.join(parent, "stage.json")
    err_path = os.path.join(parent, "stage.stderr")
    argv = list(cargo_argv) + ["--message-format=json"]
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
    return {
        "cargo_argv": argv,
        "cwd": cwd,
        "seed": seed,
        "log": {"path": out_path, "sha256": seams.sha256(out_path)},
        "stderr": {"path": err_path, "sha256": seams.sha256(err_path)},
        "executable": {"path": executable, "sha256": seams.sha256(executable),
                       "bytes": seams.size(executable)},
    }


def stage_oxide(seams, paths):
    return stage(
        seams,
        parent=os.path.join(paths["oxide"], "stage"),
        cwd=REPO,
        lock_path=os.path.join(REPO, "Cargo.lock"),
        cargo_argv=["cargo", "test", "--test", "tape_prune_eval", "--no-run",
                    "--locked", "--offline"],
        target="tape_prune_eval",
        label="Oxide",
    )


def stage_barge(seams, binding, paths):
    source = paths["barge_source"]
    archive = seams.extract(binding["barge"]["sibling"], binding["barge"]["commit"],
                            source)
    lock_path = os.path.join(source, "Cargo.lock")
    if seams.sha256(lock_path) != binding["barge"]["lock_sha256"]:
        raise Obstruction("the archived Barge lock is not the bound lock")
    staged = stage(
        seams,
        parent=os.path.join(paths["barge"], "stage"),
        cwd=source,
        lock_path=lock_path,
        cargo_argv=["cargo", "test", "-p", "barge", "--test", "tape_eval",
                    "--no-run", "--locked", "--offline"],
        target="tape_eval",
        label="Barge",
    )
    staged["archive"] = archive
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
    run_root = paths["barge_run"]
    seams.make_dirs(run_root)
    argv = [staged["executable"]["path"]]
    env = engine_environment(os.path.join(paths["barge"], "engine"),
                             extra={"BARGE_TEST_SCRATCH_ROOT": run_root})
    result = seams.run_captured(
        argv, cwd=paths["barge"], env=env,
        timeout_s=max(1, seams.work_left() - SHUTDOWN_MARGIN_SECONDS))
    marker = "barge.tape.exercise/1: retained "
    reported = None
    for line in result["stderr"].splitlines():
        if line.startswith(marker):
            reported = line[len(marker):].strip()
    retained = retained_inside(run_root, reported) if reported else None
    return {"argv": argv, "engine": result, "reported": reported,
            "retained": retained,
            "retention_refused": bool(reported) and retained is None,
            "fixture": run_root}


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


def build_report(*, binding, guard, paths, envelope, oxide, barge, oxide_parse,
                 barge_parse, closure, gaps):
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
                } if barge else None,
                "classification": classification(
                    barge["engine"] if barge else None,
                    bounded_refusal_marker=BOUNDED_REFUSAL_MARKER),
                "retained_transcript": {
                    "reported": barge["reported"] if barge else None,
                    "inside_own_root": barge["retained"] if barge else None,
                    "retention_refused": barge["retention_refused"] if barge else None,
                },
                "parse": barge_parse,
            },
        },
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
    """Run the one observation occurrence. Every refusal happens before the
    engine windows, and any unclassifiable outcome keeps everything."""
    seams = seams or Seams()
    gaps = []
    binding, _payload = read_binding(options.get("binding"), options.get("sha256"),
                                     seams)
    guard = guard_context(seams, environ)
    facts = source_facts(binding, seams, guard["scratch_root"])
    envelope = verify_source_bindings(binding, facts, guard)
    paths = derive_paths(binding, guard)
    if seams.exists(paths["run_root"]) or seams.exists(paths["report"]):
        raise Obstruction("this occurrence's root or report already exists; refusing")
    if seams.work_left() <= SHUTDOWN_MARGIN_SECONDS:
        raise Obstruction("the guard's deadline leaves no work window")
    seams.make_root(paths["run_root"])
    seams.make_root(paths["oxide"])
    seams.make_root(paths["barge"])
    oxide_staged = stage_oxide(seams, paths)
    barge_staged = stage_barge(seams, binding, paths)
    envelope["oxide"]["stage"] = oxide_staged
    envelope["barge"]["stage"] = barge_staged

    oxide = run_oxide(seams, paths, oxide_staged)
    if oxide["observation"] is None:
        gaps.append("Oxide wrote no observation report; only its engine exit and "
                    "output were collected")
    elif (oxide["observation"]["document"] or {}).get("qualified") is not False:
        gaps.append("the Oxide observation document did not declare itself unqualified")

    barge = run_barge(seams, paths, barge_staged)
    if barge["retention_refused"]:
        gaps.append(
            "Barge reported a retained transcript outside this run's own root; it "
            "was not read and no fate is recorded for that side")

    oxide_parse = None
    if oxide["observation"] is not None:
        oxide_parse = parse_capture(
            seams, oxide_staged,
            root=oxide["fixture"],
            transcript=os.path.join(oxide["fixture"], "fixture-base",
                                    "transcript.json"),
            manifest=os.path.join(oxide["fixture"], "fixture-base", "manifest.json"),
            report=os.path.join(oxide["fixture"], "parse.json"))
        if oxide_parse is None:
            gaps.append("Oxide retained no transcript; its side has no parser result")
        elif oxide_parse["report"] is None:
            gaps.append("the canonical parse pass wrote no report for the Oxide side")
        elif oxide_parse["report"]["document"] is None:
            gaps.append("the Oxide parse report is unparsable and was not repaired")

    barge_parse = None
    if barge["retained"]:
        barge_root = paths["barge_run"]
        manifest = os.path.join(os.path.dirname(barge["retained"]), "manifest.json")
        barge_parse = parse_capture(
            seams, oxide_staged, root=barge_root, transcript=barge["retained"],
            manifest=manifest, report=os.path.join(barge_root, "parse.json"))
        if barge_parse is not None and barge_parse["report"] is not None \
                and barge_parse["report"]["document"] is None:
            gaps.append("the Barge parse report is unparsable and was not repaired")
        if not seams.isfile(manifest):
            gaps.append(
                "Barge's no-argument route retains no manifest document, so no strict "
                "or limited verdict applies to that side; the exact smallest linked "
                "Barge change is to retain the manifest bytes `embedded_exercise` "
                "already holds beside its retained transcript")
    elif not barge["retention_refused"]:
        gaps.append("Barge reported no retained transcript; its side has no fate")

    closure = {"state": "guarded",
               "reason": "the qualified guard's post-leader census, not this script, "
                         "decides descendant closure; every root stays retained"}
    report = build_report(binding=binding, guard=guard, paths=paths,
                          envelope=envelope, oxide=oxide, barge=barge,
                          oxide_parse=oxide_parse, barge_parse=barge_parse,
                          closure=closure, gaps=gaps)
    try:
        seams.write_json(paths["report"], report)
    except OSError as error:
        raise Obstruction(f"the observation report cannot be written: {error}")
    say(f"observation: report {paths['report']}; qualified=false; "
        f"oxide exit {oxide['engine']['code']}, barge exit {barge['engine']['code']}")
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
            "components": "two offline staging builds with debug info off, two "
                          "private bounded offline seeds, two engine windows, two "
                          "parse passes, transcripts and reports, and shutdown "
                          "margin",
            "fit": "unmeasured: this preparation runs no staging build or engine, so "
                   "the proposal is derived from the route and the proven staging "
                   "recipe, not measured; only a scoped amendment lowers it",
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
        self.calls.append(("run_logged", list(argv)))
        return self.script.get("stage_exit", 0)

    def run_captured(self, argv, **kwargs):
        self.calls.append(("run_captured", list(argv)))
        if "--observe-base" in argv:
            return dict(self.script["oxide"])
        if "--parse-capture" in argv:
            return dict(self.script["parse"])
        return dict(self.script["barge"])

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
        return path in self.script.get("isfiles", set())

    def isdir(self, path):
        return path in self.script.get("isdirs", set())

    def mode(self, path):
        return 0o700

    def size(self, path):
        return 1

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

    def write_json(self, path, document):
        self.calls.append(("write_json", path))
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


def fixture_binding(facts):
    document = json.loads(json.dumps(BINDING_TEMPLATE))
    document["planner_reference"] = "290-fixture"
    document["oxide"] = {"commit": facts["oxide"]["commit"],
                         "tree": facts["oxide"]["tree"],
                         "lock_sha256": facts["oxide"]["lock_sha256"]}
    document["barge"] = {"sibling": "/siblings/barge",
                         "commit": facts["barge"]["commit"],
                         "tree": facts["barge"]["tree"],
                         "lock_sha256": facts["barge"]["lock_sha256"]}
    document["toolchain"].update({
        "rustc": "/rust/rustc", "rustc_sha256": facts["toolchain"]["rustc_sha256"],
        "cargo": "/rust/cargo", "cargo_sha256": facts["toolchain"]["cargo_sha256"],
    })
    return document


FIXTURE_ENV = {
    "OXIDE_TASK_SCRATCH_ROOT": "/Volumes/Stash/scratch/oxide/guard-run",
    "OXIDE_PROOF_EVIDENCE": "/Volumes/Stash/scratch/oxide/guard-run/evidence",
    "OXIDE_PROOF_RUN_ID": "fixture",
    "OXIDE_PROOF_DEADLINE_NS": str(10 ** 12),
}

FIXTURE_GUARD = {
    "scratch_root": "/Volumes/Stash/scratch/oxide/guard-run",
    "evidence": "/Volumes/Stash/scratch/oxide/guard-run/evidence",
    "run_id": "fixture", "deadline_ns": 10 ** 12,
    "supervisor_sha256": SUPERVISOR_SHA256,
}


def scripted_occurrence(binding, facts, *, barge_stderr, stage_exit=0):
    """One fully scripted occurrence: no real file, child or host read."""
    run_root = os.path.join(FIXTURE_GUARD["scratch_root"], "tape-observation")
    oxide_root = os.path.join(run_root, "oxide", "fixture")
    barge_run = os.path.join(run_root, "barge", "run")
    retained = os.path.join(barge_run, "barge-test-x.transcript.json")
    transcript = os.path.join(oxide_root, "fixture-base", "transcript.json")
    manifest = os.path.join(oxide_root, "fixture-base", "manifest.json")
    observation_report = os.path.join(oxide_root, "observation.json")
    oxide_parse_report = os.path.join(oxide_root, "parse.json")
    barge_parse_report = os.path.join(barge_run, "parse.json")
    oxide_executable = os.path.join(run_root, "oxide", "stage", "target", "debug",
                                    "deps", "tape_prune_eval-1")
    barge_executable = os.path.join(run_root, "barge", "stage", "target", "debug",
                                    "deps", "tape_eval-1")
    execution = {
        "code": 0, "signal": None, "stdout": "", "stderr": "",
        "stdout_truncated": False, "stderr_truncated": False, "timed_out": False,
    }

    def artifact(target, executable):
        return json.dumps({"reason": "compiler-artifact",
                           "target": {"name": target},
                           "executable": executable})

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
            "bounded-test.py": SUPERVISOR_SHA256,
        },
        "volume": facts["volume"],
        "exists": {"/siblings/barge"},
        "isdirs": {FIXTURE_GUARD["scratch_root"], FIXTURE_GUARD["evidence"]},
        "isfiles": {retained, transcript, manifest, observation_report,
                    oxide_parse_report, barge_parse_report,
                    oxide_executable, barge_executable,
                    "/rust/rustc", "/rust/cargo"},
        "texts": {
            "oxide/stage/stage.json": artifact("tape_prune_eval", oxide_executable),
            "barge/stage/stage.json": artifact("tape_eval", barge_executable),
        },
        "files": {
            "/binding.json": json.dumps(binding).encode(),
            "Cargo.lock": b"",
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
        },
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
    check("the plan marks fit unmeasured",
          plan_doc["bounds"]["fit"].startswith("unmeasured"))

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
                                              REPORT_NAME))
    refuses("a missing guard context", lambda: guard_context(
        FakeSeams({"digests": {}}),
        {"OXIDE_TASK_SCRATCH_ROOT": "", "OXIDE_PROOF_EVIDENCE": "",
         "OXIDE_PROOF_RUN_ID": "", "OXIDE_PROOF_DEADLINE_NS": ""}),
        "qualified shared supervisor")

    # 5. Retention: only a path inside the caller's own root, never by mtime.
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

    # 6. Classification keeps apparatus, refusal, raw exit and closure apart.
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

    # 7. Equal and unequal raw fates are both unqualified and record no claim.
    def report_with(barge_outcome):
        return build_report(
            binding=binding, guard=FIXTURE_GUARD, paths=paths, envelope=envelope,
            oxide=None, barge=None,
            oxide_parse={"report": {"document": {"outcome": {"state": "completed"}}}},
            barge_parse={"report": {"document": {"outcome": barge_outcome}}},
            closure={"state": "guarded"}, gaps=[])
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

    # 8. Caps and truncation are visible, never a complete-looking document.
    capped = build_report(binding=binding, guard=FIXTURE_GUARD, paths=paths,
                          envelope=envelope, oxide=None, barge=None,
                          oxide_parse=None, barge_parse=None,
                          closure={"state": "guarded"},
                          gaps=[f"g{index}" for index in range(GAP_CAP + 3)])
    check("gap overflow is counted", len(capped["gaps"]) == GAP_CAP + 1
          and "further gap(s) omitted" in capped["gaps"][-1])

    # 9. Refusals happen before any engine.
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
    check("a spent window staged nothing",
          not any(call[0] in ("run_logged", "make_root") for call in fake.calls))
    staged_bad = scripted_occurrence(binding, facts, barge_stderr="")
    staged_bad["stage_exit"] = 101
    fake = FakeSeams(staged_bad)
    refuses("a failing staging build refuses",
            lambda: observe({"binding": "/binding.json",
                             "sha256": sha256_bytes(binding_bytes)}, fake,
                            FIXTURE_ENV),
            "staging exited 101")
    check("a failing staging build ran no engine",
          not any(call[0] == "run_captured" for call in fake.calls))

    # 10. A complete scripted occurrence writes one unqualified report that
    #     keeps a nonzero engine exit and a refused retention as observations.
    script = scripted_occurrence(
        binding, facts,
        barge_stderr=BOUNDED_REFUSAL_MARKER + ": x\n"
                     "barge.tape.exercise/1: retained /elsewhere/x.transcript.json\n")
    fake = FakeSeams(script)
    code = observe({"binding": "/binding.json",
                    "sha256": sha256_bytes(binding_bytes)}, fake, FIXTURE_ENV)
    written = script["written"]
    check("a scripted occurrence completes", code == 0 and written is not None)
    check("the written report is unqualified and claims nothing",
          written["qualified"] is False
          and written["fates"]["comparison"].startswith("not-claimed")
          and written["controls"][0]["case"] == "BASE"
          and written["controls"][1]["case"] == "CTL-0")
    check("an outside-retention report is refused, not read",
          written["windows"]["barge"]["retained_transcript"]["retention_refused"] is True
          and written["windows"]["barge"]["retained_transcript"]["inside_own_root"] is None
          and written["fates"]["barge"]["state"] == "absent"
          and any("outside this run's own root" in gap for gap in written["gaps"]))
    windows = [call[1] for call in fake.calls if call[0] == "run_captured"]
    check("each engine ran at most once",
          sum(1 for argv in windows if "--observe-base" in argv) == 1
          and sum(1 for argv in windows if len(argv) == 1) == 1)

    script = scripted_occurrence(
        binding, facts,
        barge_stderr=BOUNDED_REFUSAL_MARKER + ": cap exhausted\n"
                     "barge.tape.exercise/1: retained "
                     "/Volumes/Stash/scratch/oxide/guard-run/tape-observation/"
                     "barge/run/barge-test-x.transcript.json\n")
    script["barge"] = dict(script["barge"], code=2)
    script["oxide"] = dict(script["oxide"], code=101)
    fake = FakeSeams(script)
    observe({"binding": "/binding.json", "sha256": sha256_bytes(binding_bytes)},
            fake, FIXTURE_ENV)
    written = script["written"]
    check("a nonzero Oxide exit is kept as an observation",
          written["windows"]["oxide"]["classification"]["state"] == "nonzero-exit"
          and written["windows"]["oxide"]["classification"]["exit_code"] == 101)
    check("a reported Barge refusal has its own state",
          written["windows"]["barge"]["classification"]["state"]
          == "bounded-capture-refusal")
    check("both fates are still raw and unqualified",
          written["qualified"] is False
          and written["fates"]["oxide"]["state"] == "observed"
          and written["fates"]["barge"]["state"] == "observed")

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
            print(f"observation: refused before any engine: {error}", file=sys.stderr)
            return 2
        except UnknownOutcome as error:
            print(f"observation: closure unknown; everything kept: {error}",
                  file=sys.stderr)
            return 2
    say(USAGE)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
