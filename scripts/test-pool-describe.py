#!/usr/bin/env python3
"""Focused proof for `oxide pool describe --json` (oxide#112).

Status: gate open (`RUN_BOUND` is True). The owner cleared the apparatus gate
at 53ec48e4 and the one authorised native run is recorded on PR #104. With the
gate closed the script refuses with exit 2 before any effect. `--plan` prints
this docstring and has no effect. A refusal is not a proof.

Binding: the script runs only inside the qualified shared runner (oxide#109),
`scripts/bounded-test.py`, delivered at e86906fe with SHA-256
8e379c93416c6e06b7f06bec7f86dba4fca4c8900b1cc87f1eb6303d39897a9d. The
selector runs it as

    python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-pool-describe.py

Under the runner the script takes these from the environment:
- OXIDE_TASK_SCRATCH_ROOT, with the same value in TMPDIR, TMP and TEMP. Every
  staging, fixture and output write goes under this monitored root. Nothing
  is created in a sibling /private/tmp tree.
- OXIDE_PROOF_EVIDENCE, a directory the runner never removes. Child logs,
  provenance and any unknown-case evidence go here.
- OXIDE_PROOF_DEADLINE_NS, an absolute CLOCK_UPTIME_RAW deadline. The script
  reads it directly and keeps a shutdown margin before it. It does not start
  a fresh 300 s.
Without these variables the script refuses. It never falls back to its own
clock or its own root.

Closure and cleanup belong to the runner. The script deletes nothing. Its
children inherit the runner's cooperative session: the script starts no
session, sends no group signal and runs no per-child supervision. A child
timeout kills only the direct child PID, records closure unknown and fails.
The runner then keeps the scratch.

Plan:
1. Staging: one offline, locked `cargo build --bin oxide` from HEAD, debug
   profile, into owned `target/` and `build/` (CARGO_TARGET_DIR,
   CARGO_BUILD_BUILD_DIR). CARGO_HOME is owned. Its seed is selected from
   Cargo.lock at HEAD. Only the supported crates.io sparse namespace is served,
   matched exactly on index and cache. Each registry package takes one index
   entry and one .crate archive, with archive bytes checked against the lock
   checksum. Workspace packages are skipped. The one locked Longhorn Git package
   seeds only its bare database candidates (1 to 8, one shared aggregate), each with a private config, and its full locked
   commit must resolve before staging; the private checkout's HEAD is checked
   after staging. Other sources, duplicate keys and duplicate packages refuse.
   The host CARGO_HOME and registry roots are checked
   before descent; symlinks and special entries refuse. Copies stop before any
   write past the 256 MiB aggregate. seed.json records the SHA-256 of the exact
   lock bytes, the namespace, the aggregate bytes and each copied file's digest.
   RUSTUP_TOOLCHAIN is the
   rust-toolchain.toml channel, RUSTUP_AUTO_INSTALL=0, and inherited
   CARGO*/RUSTC*/RUSTFLAGS/RUSTDOC/SCCACHE*/OXIDE_* routing is scrubbed.
   Staging keeps the host HOME and RUSTUP_HOME (explicit read exposure, not
   private). The commit, tree and artifact SHA-256 go to the evidence
   directory.
2. Each case: exit code, schema, escaping, and before/after nonmutation of
   the case tree (bytes, directories, symlinks, mode and times).
   a. no registration: `removable` is null, `selected` is "internal", the
      pool directory is not created;
   b. unknown registration (invalid `removable.json`): null identity fields,
      `unavailable_reason` "metadata_unknown", escaped `unavailable_detail`;
   c. absent volume (valid registration to a random unmounted /Volumes name):
      reason "absent", `available` false, payload `<mount>/oxide-pool`,
      scratch `<mount>/scratch` outside payload, no /Volumes path created;
   d. CLI refusal: `oxide pool describe` without `--json` exits nonzero with
      the usage text on stderr and changes nothing.
3. Phase checks: host free space above 20 GiB and owned bytes at or below
   512 MiB, at phase boundaries. These are samples. The runner's monitor is
   the enforcing check.

Explicit coverage gaps. Not passes:
- `available: true`, and every volume-dependent refusal (identity mismatch,
  volume_unusable, below_reserve, payload_missing), needs a mounted APFS
  volume. This proof mounts none. Library and unit tests cover them without a
  volume.
- Pending reconciliation and missing control through the CLI need the same
  mount. Library tests cover them.
- Live `diskutil` and `plutil` output caps are unit-tested with real
  subprocesses, not through this script. The 5 s timeout has no test anywhere:
  its code path is written but unexercised, so no timeout behaviour is claimed.
- Cargo descendants: this script makes no closure claim for them. The runner
  decides closure, and only for the cooperative session. Arbitrary same-user
  writers that leave the session are outside its promise.
- Seed copy race: the chain checks and each copy are not one no-follow
  operation. A concurrent change to the shared cache between them
  could redirect the copy. Disclosed, not closed.
- Staging reads the host HOME, RUSTUP_HOME, toolchain and registry seed. That
  is accepted as offline fixture input.
- Exit-nonzero for an internal Oxide failure is not exercised. Only the usage
  refusal is.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import time
import uuid
from pathlib import Path

# The gate. Opened by the owner after the apparatus PASS (53ec48e4) for the one
# authorised native run. Setting it back to False is an owner action, not a fix.
RUN_BOUND = True
CLOSED = (
    "CLOSED: RUN_BOUND is False. rust:pool-describe did not run; nothing was "
    "staged, built or executed."
)
OWNED_LIMIT_BYTES = 512 * 1024 * 1024
# Aggregate bound on every byte the seeder copies, checked before each write.
SEED_AGGREGATE_BYTES = 256 * 1024 * 1024
SEED_CHUNK_BYTES = 1024 * 1024
# The only registry sources the seed serves: crates.io, git index or sparse.
CRATES_SOURCES = ("registry+https://github.com/rust-lang/crates.io-index", "sparse+https://index.crates.io/")
# The one locked Git package (oxide#112): exact source, name, version and full
# commit. Only its bare database candidates are seeded, never a host checkout.
LONGHORN_SOURCE = "git+https://github.com/inflatable-cookie/longhorn.git?tag=v0.4.1#5caf1e2c763588d6b7b73bbf9c24cc2d83cffe41"
LONGHORN_NAME = "longhorn-storage-layout"
LONGHORN_VERSION = "0.4.1"
LONGHORN_REV = "5caf1e2c763588d6b7b73bbf9c24cc2d83cffe41"
GIT_DB_RE = re.compile(r"^longhorn-[0-9a-f]{16}$")
GIT_CANDIDATE_LIMIT = 8
# Host config, hooks and alternates are never copied. The private config is this.
GIT_CONFIG = b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n"
# The one supported crates.io sparse namespace: the name Cargo gives this host's
# sparse index and cache, as build_context_comparison.CARGO_REGISTRY_NAMESPACE.
CRATES_NAMESPACE = "index.crates.io-1949cf8c6b5b557f"
NAME_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")
VERSION_RE = re.compile(r"^[0-9]+[.][0-9]+[.][0-9]+(?:[-+][0-9A-Za-z.+-]+)?$")
CHECKSUM_RE = re.compile(r"^[0-9a-f]{64}$")
BACKSLASH = chr(92)
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
# Shutdown margin kept before the runner's absolute deadline.
DEADLINE_MARGIN_NS = 30 * 1_000_000_000
REPO = Path(__file__).resolve().parent.parent
ROOT_VAR = "OXIDE_TASK_SCRATCH_ROOT"
TEMP_VARS = (ROOT_VAR, "TMPDIR", "TMP", "TEMP")
# Runner-owned values that stay: the evidence directory and the absolute deadline.
RUNNER_KEEP = ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS")
SCRUB_PREFIXES = ("CARGO", "RUSTC", "RUSTFLAGS", "RUSTDOC", "SCCACHE", "RUSTUP_TOOLCHAIN", "OXIDE_")
PATH_BASE = "/usr/bin:/bin:/usr/sbin:/sbin"


def now_ns() -> int:
    """The runner's clock: CLOCK_UPTIME_RAW, the same clock as its deadline."""
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


class Proof:
    """Shared state: the runner's root, evidence and absolute deadline."""

    def __init__(self) -> None:
        missing = [name for name in (ROOT_VAR,) + RUNNER_KEEP if not os.environ.get(name)]
        if missing:
            raise RuntimeError(f"run under the qualified shared runner; missing {', '.join(missing)}")
        self.root = Path(os.environ[ROOT_VAR])
        if os.path.realpath(self.root) != str(self.root):
            raise RuntimeError(f"{ROOT_VAR} must be canonical")
        self.evidence = Path(os.environ["OXIDE_PROOF_EVIDENCE"])
        self.deadline_ns = int(os.environ["OXIDE_PROOF_DEADLINE_NS"]) - DEADLINE_MARGIN_NS
        self.phases: list[str] = []
        self.closure_known = True

    def remaining(self) -> float:
        left = self.deadline_ns - now_ns()
        if left <= 0:
            raise RuntimeError("runner deadline reached before the next step")
        return left / 1_000_000_000

    def env(self, case: Path | None = None, extra: dict | None = None) -> dict:
        """Scrubbed host env. Runner values stay; temp vars point into the root.

        Every inherited OXIDE_* except the two runner values is removed, so no
        host OXIDE_CONFIG or debug or test override reaches a child. A case
        gets a private HOME and OXIDE_POOL. Staging gets neither: it keeps the
        host HOME and RUSTUP_HOME, as disclosed.
        """
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(SCRUB_PREFIXES) or key in RUNNER_KEEP
        }
        for key in TEMP_VARS:
            env[key] = str(self.root)
        env["RUSTUP_AUTO_INSTALL"] = "0"
        env["PATH"] = PATH_BASE
        if case is not None:
            env["HOME"] = str(case / "home")
            env["OXIDE_POOL"] = str(case / "pool")
        if extra:
            env.update(extra)
        return env

    def check_phase(self, label: str) -> None:
        free = shutil.disk_usage("/").free
        if free <= HOST_RESERVE_BYTES:
            raise RuntimeError(f"{label}: host free space at or below the 20 GiB reserve")
        owned = owned_bytes(self.root)
        if owned > OWNED_LIMIT_BYTES:
            raise RuntimeError(f"{label}: owned bytes {owned} exceed 512 MiB")
        self.phases.append(f"{label}: owned={owned} free={free}")

    def run(self, argv: list[str], env: dict, label: str, cwd: Path | None = None) -> tuple[int, str, str]:
        """Run one child in the inherited session, logs in the evidence directory.

        No session or group is started here. A timeout kills only this direct
        child PID and leaves closure unknown: nothing is deleted and the runner
        keeps the scratch.
        """
        out_path = self.evidence / f"{label}.stdout"
        err_path = self.evidence / f"{label}.stderr"
        with out_path.open("wb") as out, err_path.open("wb") as err:
            proc = subprocess.Popen(argv, env=env, cwd=cwd, stdout=out, stderr=err)
            try:
                code = proc.wait(timeout=self.remaining())
            except subprocess.TimeoutExpired:
                proc.kill()
                self.closure_known = False
                raise RuntimeError(f"{label}: runner deadline reached; closure unknown")
        return code, out_path.read_text(errors="replace"), err_path.read_text(errors="replace")


def owned_bytes(root: Path) -> int:
    """Allocated bytes under root. A walk error raises: no partial total."""
    total = os.lstat(root).st_blocks * 512
    for dirpath, dirnames, filenames in os.walk(root, onerror=_raise):
        for name in dirnames + filenames:
            total += os.lstat(os.path.join(dirpath, name)).st_blocks * 512
    return total


def _raise(error: OSError) -> None:
    raise error


class SeedRefusal(RuntimeError):
    """The seed cannot be built safely from the lock and the host cache."""


def parse_lock_packages(text: str) -> tuple[list[dict], dict]:
    """Registry packages of a Cargo.lock as {name, version, source, checksum}.

    Fixed grammar: top-level `version = N`, `[[package]]` tables with basic
    string keys, and `dependencies` arrays of basic strings, which are
    skipped. A table without `source` is a workspace or path package and is
    skipped; it still needs a name and version. Refused: any other source, a
    malformed or path-unsafe name, version or checksum, an unknown key, a
    repeated key in one table, a repeated top-level version, and a repeated
    (name, version) package. Nothing is overwritten.
    """
    tables: list[dict] = []
    current: dict | None = None
    fields: set[str] = set()
    top_version_seen = False
    in_array = False
    for lineno, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if BACKSLASH in line:
            raise SeedRefusal(f"Cargo.lock:{lineno}: escapes are not supported")
        if in_array:
            if line == "]":
                in_array = False
            elif not re.match(r'^"[^"]*",?$', line):
                raise SeedRefusal(f"Cargo.lock:{lineno}: unsupported dependency entry")
            continue
        if not line or line.startswith("#"):
            continue
        if line == "[[package]]":
            if current is not None:
                tables.append(current)
            current = {}
            fields = set()
            continue
        match = re.match(r"^([a-z_]+) = (.*)$", line)
        if not match:
            raise SeedRefusal(f"Cargo.lock:{lineno}: unsupported line")
        key, value = match.groups()
        if current is None:
            if key == "version" and value.isdigit() and not top_version_seen:
                top_version_seen = True
                continue
            raise SeedRefusal(f"Cargo.lock:{lineno}: unsupported or repeated top-level key {key}")
        if key not in ("name", "version", "source", "checksum", "dependencies"):
            raise SeedRefusal(f"Cargo.lock:{lineno}: unsupported key {key}")
        if key in fields:
            raise SeedRefusal(f"Cargo.lock:{lineno}: duplicate {key} in one package")
        fields.add(key)
        if key == "dependencies":
            if value == "[]":
                continue
            if value == "[":
                in_array = True
                continue
            raise SeedRefusal(f"Cargo.lock:{lineno}: unsupported dependencies value")
        string = re.match(r'^"([^"]*)"$', value)
        if not string:
            raise SeedRefusal(f"Cargo.lock:{lineno}: {key} is not a basic string")
        current[key] = string.group(1)
    if in_array:
        raise SeedRefusal("Cargo.lock: unterminated dependencies array")
    if current is not None:
        tables.append(current)

    packages: list[dict] = []
    git_found: list[dict] = []
    seen: set[tuple[str, str]] = set()
    for table in tables:
        if "name" not in table or "version" not in table:
            raise SeedRefusal("a package table without a name or version is malformed")
        if "source" not in table:
            if "checksum" in table:
                raise SeedRefusal(f"{table['name']} carries a checksum but no source")
            continue  # workspace or path package: nothing to seed
        if table["source"] == LONGHORN_SOURCE:
            if (table.get("name"), table.get("version")) != (LONGHORN_NAME, LONGHORN_VERSION):
                raise SeedRefusal("the locked Git source names an unexpected package")
            git_found.append(table)
            continue
        if table["source"] not in CRATES_SOURCES:
            raise SeedRefusal(f"unsupported registry source: {table['source']}")
        name = table["name"]
        version = table["version"]
        checksum = table.get("checksum", "")
        if not NAME_RE.match(name):
            raise SeedRefusal(f"unsafe crate name: {name!r}")
        if not VERSION_RE.match(version):
            raise SeedRefusal(f"unsafe crate version: {version!r}")
        if not CHECKSUM_RE.match(checksum):
            raise SeedRefusal(f"malformed checksum for {name} {version}")
        if (name, version) in seen:
            raise SeedRefusal(f"duplicate lock entry for {name} {version}")
        seen.add((name, version))
        packages.append({"name": name, "version": version, "source": table["source"], "checksum": checksum})
    if len(git_found) != 1:
        raise SeedRefusal(f"expected exactly one locked Longhorn Git package, found {len(git_found)}")
    return packages, git_found[0]


def sparse_index_path(name: str) -> str:
    """The sparse-index path of a crate, lowercase, as Cargo lays it out."""
    lower = name.lower()
    if len(lower) == 1:
        return f"1/{lower}"
    if len(lower) == 2:
        return f"2/{lower}"
    if len(lower) == 3:
        return f"3/{lower[0]}/{lower}"
    return f"{lower[:2]}/{lower[2:4]}/{lower}"


def lstat_real_dir(path: Path, label: str) -> None:
    try:
        mode = os.lstat(path).st_mode
    except FileNotFoundError:
        raise SeedRefusal(f"{label} is missing: {path}")
    if stat.S_ISLNK(mode):
        raise SeedRefusal(f"{label} is a symlink, refused before following it: {path}")
    if not stat.S_ISDIR(mode):
        raise SeedRefusal(f"{label} is not a directory: {path}")


def crates_namespace(host_registry: Path) -> str:
    """The supported crates.io namespace, selected by its exact name.

    CRATES_NAMESPACE is selected on both index and cache. Other entries in
    either root are not listed, traversed or copied, and their presence is not
    ambiguity. A missing or non-directory selected path refuses in seed_from_lock.
    """
    for kind in ("index", "cache"):
        lstat_real_dir(host_registry / kind, f"host {kind} root")
    return CRATES_NAMESPACE


def check_chain(base: Path, rel: str) -> Path:
    """Every component from base to rel is a real directory, the last a real file.

    Checked before any copy from that path. Accepted caveat: this is not a
    no-follow open of the whole chain. A concurrent change could still race it.
    """
    current = base
    parts = rel.split("/")
    for index, part in enumerate(parts):
        current = current / part
        try:
            mode = os.lstat(current).st_mode
        except FileNotFoundError:
            raise SeedRefusal(f"required seed input is missing: {current}")
        if stat.S_ISLNK(mode):
            raise SeedRefusal(f"symlink in the seed chain, refused: {current}")
        if index == len(parts) - 1:
            if not stat.S_ISREG(mode):
                raise SeedRefusal(f"seed input is not a regular file: {current}")
        elif not stat.S_ISDIR(mode):
            raise SeedRefusal(f"seed chain component is not a directory: {current}")
    return current


class SeedBudget:
    """Aggregate bytes copied from the host cache: the 256 MiB bound."""

    def __init__(self, limit: int) -> None:
        self.used = 0
        self.limit = limit


def copy_bounded(source: Path, destination: Path, budget: SeedBudget, expected_sha256: str | None) -> str:
    """Copy source in bounded chunks, refusing before any write past the budget.

    The source opens with O_NOFOLLOW and O_NONBLOCK, and must be a regular
    file once open, so a special entry cannot block the open. The digest covers
    the bytes actually written, and an expected checksum must match them.
    Destination is created exclusively under the owned root.
    """
    try:
        fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError as error:
        raise SeedRefusal(f"cannot open seed input without following links: {source}: {error}")
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise SeedRefusal(f"seed input is not a regular file once open: {source}")
    digest = hashlib.sha256()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with os.fdopen(fd, "rb") as reader, open(destination, "xb") as writer:
        while True:
            chunk = reader.read(SEED_CHUNK_BYTES)
            if not chunk:
                break
            if budget.used + len(chunk) > budget.limit:
                raise SeedRefusal("seed aggregate would exceed the 256 MiB bound; refused before the write")
            budget.used += len(chunk)
            writer.write(chunk)
            digest.update(chunk)
    actual = digest.hexdigest()
    if expected_sha256 is not None and actual != expected_sha256:
        raise SeedRefusal(f"checksum mismatch for {source.name}: copied bytes {actual}, lock {expected_sha256}")
    return actual


def git_env(home: Path) -> dict[str, str]:
    """A git environment built from nothing: no inherited GIT_* or host config."""
    return {
        "PATH": PATH_BASE,
        "HOME": str(home),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_TERMINAL_PROMPT": "0",
    }


def git_rev_parse(proof: "Proof", git_dir: Path, spec: str, home: Path, label: str) -> str:
    """Read-only: the object name `spec` resolves to in `git_dir`, via Proof.run."""
    git = shutil.which("git")
    if git is None:
        raise SeedRefusal("git is not on the host PATH")
    code, out, _ = proof.run([git, "--git-dir", str(git_dir), "rev-parse", "--verify", spec], git_env(home), label)
    if code != 0:
        raise SeedRefusal(f"git rev-parse {spec} failed in the private Longhorn database")
    return out.strip()


def copy_git_tree(source: Path, destination: Path, budget: SeedBudget, rel: str, base: str, files: list[dict]) -> None:
    """Copy regular files and real directories of a bare database, bounded.

    Symlinks and special entries refuse. The top-level config and hooks are
    skipped: config is regenerated, hooks are never copied.
    """
    for entry in sorted(os.scandir(source / rel), key=lambda item: item.name):
        if not rel and entry.name in ("config", "hooks"):
            continue
        child = f"{rel}/{entry.name}" if rel else entry.name
        if child.startswith("objects/info/") and "alternates" in entry.name:
            raise SeedRefusal(f"longhorn database alternates entry refused: {child}")
        info = os.lstat(source / child)
        if stat.S_ISDIR(info.st_mode):
            (destination / child).mkdir(parents=True, exist_ok=True)
            copy_git_tree(source, destination, budget, child, base, files)
        elif stat.S_ISREG(info.st_mode):
            digest = copy_bounded(source / child, destination / child, budget, None)
            files.append({"path": f"{base}/{child}", "sha256": digest})
        else:
            raise SeedRefusal(f"longhorn database entry is a symlink or special file: {child}")


def git_commit_present(proof: "Proof", git_dir: Path, rev: str, home: Path, label: str) -> bool:
    """Read-only: whether `rev` resolves to a commit in `git_dir`. Missing is False."""
    git = shutil.which("git")
    if git is None:
        raise SeedRefusal("git is not on the host PATH")
    code, out, _ = proof.run([git, "--git-dir", str(git_dir), "rev-parse", "--verify", f"{rev}^{{commit}}"], git_env(home), label)
    return code == 0 and out.strip() == rev


def seed_git(proof: "Proof", host_cargo_home: Path, cargo_home: Path, budget: SeedBudget) -> dict:
    """Seed every locked-source Longhorn bare database candidate, bounded.

    One to GIT_CANDIDATE_LIMIT candidates are accepted; more refuse before any
    copy. All candidates share the one aggregate budget, and no charged byte is
    removed. Each copy has a private config, and each records whether the
    locked full commit is present. At least one candidate must have it. Cargo
    chooses its own identifier later; nothing here guesses it.
    """
    host_git = host_cargo_home / "git"
    lstat_real_dir(host_git, "host git root")
    host_db_root = host_git / "db"
    lstat_real_dir(host_db_root, "host git database root")
    names = [name for name in sorted(os.listdir(host_db_root)) if GIT_DB_RE.match(name)]
    if not 1 <= len(names) <= GIT_CANDIDATE_LIMIT:
        raise SeedRefusal(f"expected 1 to {GIT_CANDIDATE_LIMIT} longhorn bare databases, found {len(names)}")
    candidates: list[dict] = []
    files: list[dict] = []
    for index, name in enumerate(names):
        source = host_db_root / name
        lstat_real_dir(source, "host longhorn bare database")
        if not stat.S_ISREG(os.lstat(source / "HEAD").st_mode):
            raise SeedRefusal(f"longhorn database {name} HEAD is not a regular file")
        if os.path.lexists(source / "objects" / "info" / "alternates"):
            raise SeedRefusal(f"longhorn database {name} uses alternates; refused")
        base = f"git/db/{name}"
        destination = cargo_home / "git" / "db" / name
        destination.mkdir(parents=True, exist_ok=True)
        before = budget.used
        candidate_files: list[dict] = []
        copy_git_tree(source, destination, budget, "", base, candidate_files)
        if budget.used + len(GIT_CONFIG) > budget.limit:
            raise SeedRefusal("seed aggregate would exceed the 256 MiB bound; refused before the write")
        budget.used += len(GIT_CONFIG)
        (destination / "config").write_bytes(GIT_CONFIG)
        digest = hashlib.sha256(GIT_CONFIG).hexdigest()
        candidate_files.append({"path": f"{base}/config", "sha256": digest, "generated": True})
        present = git_commit_present(proof, destination, LONGHORN_REV, destination, f"git-rev-commit-{index}")
        candidates.append({
            "database": name,
            "bytes": budget.used - before,
            "filecount": len(candidate_files),
            "commit_present": present,
            "config_sha256": digest,
            "generated": True,
        })
        files.extend(candidate_files)
    if not any(candidate["commit_present"] for candidate in candidates):
        raise SeedRefusal(f"no private longhorn database resolves the locked commit {LONGHORN_REV}")
    return {"source": LONGHORN_SOURCE, "rev": LONGHORN_REV, "candidates": candidates, "files": files}


def verify_private_checkout(proof: "Proof", cargo_home: Path, idents: list[str]) -> str:
    """After staging: exactly one checkout at the locked rev, with the exact HEAD.

    Only the seeded identifiers are considered, and the chosen checkout must be
    inside the private Cargo home. The returned identifier is Cargo's choice.
    """
    checkouts_root = cargo_home / "git" / "checkouts"
    lstat_real_dir(checkouts_root, "private longhorn checkouts root")
    found = [
        name for name in sorted(os.listdir(checkouts_root))
        if GIT_DB_RE.match(name) and os.path.isdir(checkouts_root / name / LONGHORN_REV[:7])
    ]
    if len(found) != 1 or found[0] not in idents:
        raise SeedRefusal(f"expected exactly one private checkout at {LONGHORN_REV[:7]}, found {found}")
    ident = found[0]
    checkout = checkouts_root / ident / LONGHORN_REV[:7]
    lstat_real_dir(checkout, "private longhorn checkout")
    git_dir = checkout / ".git"
    if not os.path.lexists(git_dir):
        raise SeedRefusal("private checkout has no .git, so its HEAD cannot be verified")
    lstat_real_dir(git_dir, "private checkout git directory")
    head = git_rev_parse(proof, git_dir, "HEAD", checkout, "git-rev-checkout")
    if head != LONGHORN_REV:
        raise SeedRefusal(f"private checkout HEAD is {head}, not the locked {LONGHORN_REV}")
    return ident


def seed_from_lock(proof: "Proof", host_cargo_home: Path, packages: list[dict], git_package: dict, cargo_home: Path, lock_sha256: str) -> dict:
    """Copy only what Cargo.lock needs from the one supported crates.io namespace.

    Inputs: the namespace config.json, one sparse index entry per package
    (under the namespace's .cache), and one .crate archive per package whose
    bytes must match the lock checksum. The host registry root and namespace
    directories are lstat-checked before any descent. Only this function reads
    the host cache. CARGO_HOME stays owned and offline; nothing falls back to
    the host. The index and cache copies share one namespace name.

    Accepted caveat: the chain checks and each copy are not one no-follow
    operation. A concurrent change to the host cache between them could still
    redirect a copy.
    """
    host_registry = host_cargo_home / "registry"
    lstat_real_dir(host_registry, "host registry root")
    namespace = crates_namespace(host_registry)
    index_ns = host_registry / "index" / namespace
    cache_ns = host_registry / "cache" / namespace
    lstat_real_dir(index_ns, "host index namespace")
    lstat_real_dir(cache_ns, "host cache namespace")
    budget = SeedBudget(SEED_AGGREGATE_BYTES)
    dest_index = cargo_home / "registry" / "index" / namespace
    dest_cache = cargo_home / "registry" / "cache" / namespace
    manifest: list[dict] = []

    check_chain(index_ns, "config.json")
    digest = copy_bounded(index_ns / "config.json", dest_index / "config.json", budget, None)
    manifest.append({"path": "index/config.json", "sha256": digest})
    for package in packages:
        rel = sparse_index_path(package["name"])
        check_chain(index_ns, f".cache/{rel}")
        digest = copy_bounded(index_ns / ".cache" / rel, dest_index / ".cache" / rel, budget, None)
        manifest.append({"path": f"index/.cache/{rel}", "sha256": digest})
        archive = f"{package['name']}-{package['version']}.crate"
        check_chain(cache_ns, archive)
        digest = copy_bounded(cache_ns / archive, dest_cache / archive, budget, package["checksum"])
        manifest.append({"path": f"cache/{archive}", "sha256": digest})
    git = seed_git(proof, host_cargo_home, cargo_home, budget)
    compare_seed(cargo_home, namespace, packages, [entry["path"] for entry in git["files"]])
    return {
        "lock_sha256": lock_sha256,
        "namespace": namespace,
        "aggregate_bytes": budget.used,
        "files": manifest,
        "git": git,
    }


def compare_seed(cargo_home: Path, namespace: str, packages: list[dict], git_paths: list[str]) -> None:
    """The owned seed holds exactly the selected files under the fixed roots.

    Checkouts are absent at this point: Cargo creates them only when staging.
    """
    expected = {f"registry/index/{namespace}/config.json"}
    expected.update(f"registry/index/{namespace}/.cache/{sparse_index_path(p['name'])}" for p in packages)
    expected.update(f"registry/cache/{namespace}/{p['name']}-{p['version']}.crate" for p in packages)
    expected.update(git_paths)
    actual = set()
    for root in ("registry", "git"):
        for dirpath, _dirnames, filenames in os.walk(cargo_home / root):
            for name in filenames:
                actual.add(str(Path(dirpath, name).relative_to(cargo_home)))
    if actual != expected:
        raise SeedRefusal(
            f"owned seed differs from the lock selection: extra {sorted(actual - expected)[:3]}, "
            f"missing {sorted(expected - actual)[:3]}"
        )


def snapshot(root: Path) -> list[tuple]:
    """Every entry's type, mode, times and contents, so no change is hidden."""
    entries = []
    for dirpath, dirnames, filenames in os.walk(root):
        for name in sorted(dirnames + filenames):
            path = Path(dirpath, name)
            info = os.lstat(path)
            rel = str(path.relative_to(root))
            base = (rel, info.st_mode, info.st_mtime_ns, info.st_ctime_ns)
            if path.is_symlink():
                entries.append(base + ("link", os.readlink(path)))
            elif path.is_dir():
                entries.append(base + ("dir",))
            else:
                entries.append(base + ("file", hashlib.sha256(path.read_bytes()).hexdigest()))
    return sorted(entries)


def stage(proof: Proof) -> Path:
    """One offline, locked build of the candidate from this HEAD."""
    git = ["git", "-C", str(REPO)]
    commit = subprocess.run(git + ["rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    tree = subprocess.run(git + ["rev-parse", "HEAD^{tree}"], capture_output=True, text=True, check=True).stdout.strip()
    channel = re.search(r'channel\s*=\s*"([^"]+)"', (REPO / "rust-toolchain.toml").read_text()).group(1)
    lock_bytes = subprocess.run(git + ["show", "HEAD:Cargo.lock"], capture_output=True, check=True).stdout
    lock_sha256 = hashlib.sha256(lock_bytes).hexdigest()
    packages, git_package = parse_lock_packages(lock_bytes.decode())
    host_cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    lstat_real_dir(host_cargo_home, "host CARGO_HOME")
    cargo_home = proof.root / "cargo-home"
    cargo_home.mkdir()
    seed = seed_from_lock(proof, host_cargo_home, packages, git_package, cargo_home, lock_sha256)
    (proof.evidence / "seed.json").write_text(json.dumps(seed, indent=2))
    cargo = shutil.which("cargo")
    if cargo is None:
        raise RuntimeError("cargo is not on the host PATH")
    env = proof.env(
        extra={
            "CARGO_HOME": str(cargo_home),
            "CARGO_TARGET_DIR": str(proof.root / "target"),
            "CARGO_BUILD_BUILD_DIR": str(proof.root / "build"),
            "RUSTUP_TOOLCHAIN": channel,
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
            "PATH": f"{Path(cargo).parent}:{PATH_BASE}",
        }
    )
    code, _, err = proof.run([cargo, "build", "--offline", "--locked", "--bin", "oxide"], env, "stage", cwd=REPO)
    if code != 0:
        raise RuntimeError(f"staging failed: {err[-400:]}")
    chosen = verify_private_checkout(proof, cargo_home, [c["database"] for c in seed["git"]["candidates"]])
    seed["git"]["chosen_ident"] = chosen
    (proof.evidence / "seed.json").write_text(json.dumps(seed, indent=2))
    artifact = (proof.root / "target" / "debug" / "oxide").resolve()
    if not artifact.is_file() or proof.root not in artifact.parents:
        raise RuntimeError("the staged artifact is not inside the owned root")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    provenance = {"commit": commit, "tree": tree, "toolchain": channel, "artifact_sha256": digest}
    (proof.evidence / "provenance.json").write_text(json.dumps(provenance, indent=2))
    proof.phases.append(f"stage: {provenance}")
    return artifact


def registration_fixture(case: Path, body: str | None) -> Path:
    """A private case directory holding a pool control dir and an optional file."""
    case.mkdir()
    (case / "home").mkdir()
    pool = case / "pool"
    control = pool / "oxide-managed"
    control.mkdir(parents=True)
    os.chmod(pool, 0o700)
    os.chmod(control, 0o700)
    if body is not None:
        path = control / "removable.json"
        path.write_text(body)
        os.chmod(path, 0o600)
    return pool


def describe(proof: Proof, artifact: Path, label: str, case: Path) -> dict:
    code, out, err = proof.run([str(artifact), "pool", "describe", "--json"], proof.env(case), label)
    if code != 0:
        raise RuntimeError(f"{label}: exit {code}: {err[-400:]}")
    text = json.loads(out)
    assert text["schema"] == "oxide.pool.describe/1", text
    assert text["observed_at"].endswith("Z"), text
    if text["removable"] is not None:
        assert text["removable"]["mode"] == "ephemeral", text
    return text


def case_no_registration(proof: Proof, artifact: Path) -> None:
    case = proof.root / "case-a"
    case.mkdir()
    (case / "home").mkdir()  # no pool directory at all: describe must not create one
    before = snapshot(case)
    text = describe(proof, artifact, "case-a", case)
    assert text["removable"] is None and text["selected"] == "internal", text
    assert snapshot(case) == before, "describe changed the fixture"


def case_unknown_registration(proof: Proof, artifact: Path) -> None:
    case = proof.root / "case-b"
    registration_fixture(case, '{"bad": "quote\\"\n')
    before = snapshot(case)
    text = describe(proof, artifact, "case-b", case)
    removable = text["removable"]
    assert removable["mount_point"] is None and removable["volume_uuid"] is None
    assert removable["unavailable_reason"] == "metadata_unknown", removable
    assert removable["available"] is False and removable["payload"] is None
    assert "reserve_bytes" in removable and removable["reserve_bytes"] is None, removable
    assert snapshot(case) == before, "describe changed the fixture"


def case_absent_volume(proof: Proof, artifact: Path) -> None:
    mount = f"/Volumes/oxide-describe-{uuid.uuid4().hex[:12]}"
    registration = {
        "version": 1,
        "mount_point": mount,
        "volume_uuid": str(uuid.uuid4()).upper(),
        "container_uuid": str(uuid.uuid4()).upper(),
        "reserve_bytes": 1024,
        "cap_bytes": 4096,
        "quota_bytes": None,
        "capacity_bytes": None,
        "registered_ms": 0,
    }
    case = proof.root / "case-c"
    registration_fixture(case, json.dumps(registration))
    before = snapshot(case)
    text = describe(proof, artifact, "case-c", case)
    removable = text["removable"]
    assert removable["unavailable_reason"] == "absent", removable
    assert removable["available"] is False and text["selected"] == "internal"
    assert removable["payload"] == f"{mount}/oxide-pool"
    assert removable["suggested_scratch_root"] == f"{mount}/scratch"
    reserve = removable["reserve_bytes"]
    assert type(reserve) is int and reserve == 1024, removable
    assert not os.path.exists(mount), "describe created a /Volumes path"
    assert snapshot(case) == before, "describe changed the fixture"


def case_cli_refusal(proof: Proof, artifact: Path) -> None:
    case = proof.root / "case-d"
    registration_fixture(case, None)
    before = snapshot(case)
    code, _, err = proof.run([str(artifact), "pool", "describe"], proof.env(case), "case-d")
    assert code != 0, "usage refusal must exit nonzero"
    assert "describe --json" in err, err[-200:]
    assert snapshot(case) == before, "refusal changed the fixture"


CASES = [case_no_registration, case_unknown_registration, case_absent_volume, case_cli_refusal]


def run_proof() -> int:
    """Run the phases. Nothing is deleted: the runner decides closure and cleanup."""
    proof = Proof()
    summary: dict = {"status": "running", "cases": {}}
    try:
        proof.check_phase("before-staging")
        artifact = stage(proof)
        proof.check_phase("after-staging")
        for case in CASES:
            case(proof, artifact)
            proof.check_phase(case.__name__)
            summary["cases"][case.__name__] = "ok"
            print(f"ok {case.__name__}")
        summary["status"] = "passed"
        return 0
    except Exception as error:  # the runner keeps the root and this evidence
        summary["status"] = f"failed: {error}"
        print(f"failed: {error}", file=sys.stderr)
        return 1
    finally:
        summary["phases"] = proof.phases
        summary["closure_known"] = proof.closure_known
        (proof.evidence / "summary.json").write_text(json.dumps(summary, indent=2))


def main() -> int:
    if "--plan" in sys.argv[1:]:
        print(__doc__)
        return 0
    if not RUN_BOUND:
        print(CLOSED, file=sys.stderr)
        return 2
    return run_proof()


if __name__ == "__main__":
    raise SystemExit(main())
