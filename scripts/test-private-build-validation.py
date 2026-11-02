#!/usr/bin/env python3
"""Shared private-build validation admission/preparation gate (oxide#248).

One small apparatus closing the common private-build validation blocker for
Oxide #240/PR230 (`scripts/tape-isolation-check.py`) and Barge #041/PR41
(`scripts/tape/private-wrapper.py`). It prepares and binds, from exact
sources and executables:

- deterministic, digest-bound source/head/tree/lock/toolchain/binary/manifest
  source-audit documents whose component sources are private copies inside the
  task root (`barge.validation-source-audit/1`, the accepted Barge shape);
- a complete attributable task-private root discovery ledger for both capture
  tasks (`barge.private-roots/1`, plus the Oxide `budget.json` /
  `history-discovery.json` shapes their accepted launchers consume), built
  from retained Queue records and packets, with honest unknowns;
- externally authenticated one-use execution blocks: this tool never issues,
  self-signs or fabricates one. It validates the planner-issued documents by
  externally supplied digests and claims them exactly once;
- one gate receipt (`oxide.private-build-validation/1`) whose qualification
  state is always honest: the tool reports completed observations and
  refusals; it never grants itself a qualified property.

Runtime route (`--diagnostic`, the planner's ONE proofExecution): under the
qualified shared supervisor (`scripts/bounded-test.py`, SHA-256 pinned below),
verify the outer supervisor context and every issued binding, claim the
blocks, materialize the prepared roots, re-derive every binding live, bracket
the live account state with bounded identity+content inventories, run ONE
debug-Oxide bootstrap (exact argv, nested qualified supervisor, at most 300 s
of the one global allowance), run the inventory-bracketed private status dry
gate, re-snapshot every retained root, and export the wrapper-consumable
artifacts plus the remaining measured allowance. Any unknown refuses without
repair, retry or attribution.

Modes an author may run (effect-free, through Effigy selectors): `--self-test`
(mocked counterfactuals only; fixtures under a fresh `TMPDIR` template) and
`--plan` (no effect). `--prepare` is a planner-owned bounded document round;
this tool never runs it on its own authority.

Clock: one global prospective allowance on `CLOCK_UPTIME_RAW` (the same
cross-process clock the supervisor exports as `OXIDE_PROOF_DEADLINE_NS`);
never per-process monotonic-origin subtraction, never a phase reset, never a
replay after uncertainty. No cleanup, no deletion, no live shared-pool
inspection, no installed-Oxide build outside the issued block.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time

# ---------------------------------------------------------------------------
# Pinned identities (accepted sources; compatibility mirrors, not new schemas)
# ---------------------------------------------------------------------------

OXIDE_PIN_COMMIT = "3de241325dd286ae44a5759fa6825b05a81c7b4e"
OXIDE_PIN_TREE = "452a9c2dc6b1432c643c5053655e029955977754"
BARGE_ACCEPTED_HEAD = "864fc8e878ca039805733b82253fbbd3f50b8193"
BARGE_RECIPE_HEAD = "249f0e4c904b692de13668af77fd697ff7c5f7fc"
BARGE_RECIPE_TREE = "1c8cdc151baa5794f5dcfcda4cc801031820dbcb"
SUPERVISOR_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
BARGE_TASK = "9f435fb3-4374-4f23-ba6d-7c5e6869f4ae"
OXIDE_TASK = "d41de092-b0b0-495d-b159-8c4d1730d6b4"
BARGE_SCRATCH = Path("/Volumes/Builds/scratch/barge")
OXIDE_SCRATCH = Path("/Volumes/Builds/scratch/oxide")
SCRATCH_ROOT = Path("/Volumes/Builds/scratch")
BARGE_CAP_BYTES = 2 * 1024 * 1024 * 1024
OXIDE_CAP_BYTES = 6 * 1024 * 1024 * 1024
PROSPECTIVE_MAX_SECONDS = 1200
BOOTSTRAP_MAX_SECONDS = 300
BOOTSTRAP_MAX_MIB = 1024
OXIDE_PLANNER_REFERENCE = (
    "brief-v11:d133e22524df289aba98b74d0c87995d1902a594b5d574edb8035e0a1d12d0d4"
)
BARGE_HISTORIC_LEDGER = BARGE_SCRATCH / "041-budget-ledger-20261102/ledger.json"
BARGE_PACKET = BARGE_SCRATCH / "041-admission-packet.TUzTonsu"
BARGE_REQUIRED_ROOTS = (
    BARGE_SCRATCH / "b041-final.n0hH3Cez",
    BARGE_SCRATCH / "041-budget-ledger-20261102",
)
BARGE_BOOTSTRAP_ARGV = [
    "cargo", "build", "--offline", "--locked", "--bin", "oxide",
    "--message-format=json",
]
STATUS_SELECTOR = "rust:test:tape-private-status"
TOOLCHAIN = "1.97.1"

SCHEMA_GATE_RECEIPT = "oxide.private-build-validation/1"
SCHEMA_BARGE_AUDIT = "barge.validation-source-audit/1"
SCHEMA_BARGE_LEDGER = "barge.private-roots/1"
SCHEMA_BARGE_BLOCK = "barge.validation-block/1"

# The accepted Barge launcher (`scripts/tape/admission.py` at 864fc8e)
# enforces exactly these selectors and component names; mirrors, kept
# deliberately identical so one issued packet serves both wrappers.
BARGE_SAFE_SELECTORS = frozenset((
    "rust:compile", "rust:compile:release", "rust:format", "rust:lint",
    "rust:test:tape:pure", "rust:test:tape-stage", "rust:test:tape-export",
    "rust:test:tape-export:pure", "rust:test:tape-admission:pure",
    "docs:check",
))
BARGE_AUDIT_COMPONENTS = (
    "oxide_config", "oxide_registry", "oxide_scratch", "oxide_child",
    "effigy_admission", "rustup_cargo", "compiler_linker", "build_scripts",
    "git_source", "retained_roots",
)
BUILD_ENV_KEYS = (
    "HOME", "CARGO_HOME", "TMPDIR", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "RUSTC",
    "OXIDE_CONFIG", "OXIDE_POOL", "OXIDE_SCRATCH_ROOT", "OXIDE_TASK_SCRATCH_ROOT",
    "OXIDE_TEST_HOME_FROM_ENV", "CARGO_NET_OFFLINE",
)
HEX64 = re.compile(r"^[0-9a-f]{64}$")
HEX40 = re.compile(r"^[0-9a-f]{40}$")
BLOCK_ID = re.compile(r"^[A-Za-z0-9_-]{1,80}$")

MAX_DOC_BYTES = 8 * 1024 * 1024
INVENTORY_ROW_CAP = 50000
INVENTORY_CONTENT_CAP = 256 * 1024 * 1024
RETAINED_ROW_CAP = 200000
ALLOC_ENTRY_CAP = 500000
LIVE_ACCOUNT_ROOT = Path("/Users/tom/Library/Application Support/oxide")


class GateRefusal(Exception):
    """A bounded refusal: the gate stops without repair, retry or blame."""


def now_ns() -> int:
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


# ---------------------------------------------------------------------------
# Primitives: digests, canonical private paths, canonical documents
# ---------------------------------------------------------------------------

def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path, cap: int = MAX_DOC_BYTES) -> str:
    """Digest one regular file without following a final symlink."""
    st = os.lstat(path)
    if not stat.S_ISREG(st.st_mode):
        raise GateRefusal(f"digest target is not a regular file: {path}")
    if st.st_size > cap:
        raise GateRefusal(f"digest target exceeds the document bound: {path}")
    h = hashlib.sha256()
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino, opened.st_size) != (st.st_dev, st.st_ino, st.st_size):
            raise GateRefusal(f"digest target changed while opened: {path}")
        while True:
            block = os.read(fd, 1024 * 1024)
            if not block:
                break
            h.update(block)
        after = os.fstat(fd)
        if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != (
                opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns):
            raise GateRefusal(f"digest target changed while read: {path}")
    finally:
        os.close(fd)
    return h.hexdigest()


def canonical_file(path: Path) -> Path:
    """A real, non-symlink, canonical regular file."""
    resolved = path.resolve(strict=True)
    if path.is_symlink() or resolved != path or not path.is_file():
        raise GateRefusal(f"not a canonical regular file: {path}")
    return resolved


def private_dir(root: Path, relative: Path) -> Path:
    """Create owner-only directory components with no symlink traversal."""
    if relative.is_absolute() or any(part in ("", ".", "..") for part in relative.parts):
        raise GateRefusal(f"invalid private directory component: {relative}")
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in relative.parts:
            try:
                os.mkdir(component, 0o700, dir_fd=fd)
            except FileExistsError:
                pass
            except NotADirectoryError:
                raise GateRefusal(f"private directory component is not a directory: "
                                  f"{root / relative}") from None
            try:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            except NotADirectoryError:
                raise GateRefusal(f"private directory component is not a directory: "
                                  f"{root / relative}") from None
            os.close(fd)
            fd = child
            info = os.fstat(fd)
            if info.st_uid != os.geteuid() or info.st_mode & 0o077:
                raise GateRefusal(f"private directory is not owner-only: {root / relative}")
    finally:
        os.close(fd)
    return root / relative


def private_write(root: Path, relative: Path, data: bytes) -> Path:
    """Write one new owner-only file without following any symlink."""
    private_dir(root, relative.parent)
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in relative.parts[:-1]:
            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        out = os.open(relative.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                      0o600, dir_fd=fd)
        with os.fdopen(out, "wb") as handle:
            handle.write(data)
    except FileExistsError:
        raise GateRefusal(f"private write target already exists: {root / relative}") from None
    finally:
        os.close(fd)
    return root / relative


def canonical_json(value) -> bytes:
    """The one document encoding every digested artifact uses."""
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def read_json_bound(path: Path, expected: str) -> dict:
    """Mirror of the accepted launcher rule: digest first, then parse."""
    if not isinstance(expected, str) or not HEX64.fullmatch(expected):
        raise GateRefusal("an independent 64-hex digest is required for every issued document")
    data = path.read_bytes()
    if len(data) > MAX_DOC_BYTES or sha256_bytes(data) != expected:
        raise GateRefusal(f"issued document digest or size bound mismatch: {path}")
    value = json.loads(data)
    if not isinstance(value, dict):
        raise GateRefusal(f"issued document is not an object: {path}")
    return value


# ---------------------------------------------------------------------------
# Inventories and accounting (bounded, no-follow, unknown refuses)
# ---------------------------------------------------------------------------

def _stable(st: os.stat_result) -> list:
    return [st.st_dev, st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns]


def live_inventory(live_root: Path, deadline_ns: int | None = None) -> list:
    """Bounded identity+content inventory of the live account state.

    Mirrors the accepted launchers: regular files carry SHA-256, symlinks carry
    their target, directories and the root carry stability metadata. An empty
    or missing root is a refusal, never an empty proof. Inode identity is
    recorded, so a same-bytes inode replacement cannot pass as equality.
    """
    rows: list = []
    total = 0
    nofollow = os.O_NOFOLLOW
    directory = os.O_DIRECTORY

    def bound():
        if deadline_ns is not None and now_ns() > deadline_ns:
            raise GateRefusal("live inventory exceeded its time bound")
        if len(rows) >= INVENTORY_ROW_CAP:
            raise GateRefusal("live inventory row cap reached")

    def walk(fd: int, rel: str):
        nonlocal total
        before = os.fstat(fd)
        rows.append([rel, _stable(before)])
        bound()
        with os.scandir(fd) as entries:
            names = sorted(entry.name for entry in entries)
        for name in names:
            bound()
            st = os.stat(name, dir_fd=fd, follow_symlinks=False)
            child = f"{rel}/{name}" if rel else name
            if stat.S_ISDIR(st.st_mode):
                sub = os.open(name, os.O_RDONLY | nofollow | directory, dir_fd=fd)
                try:
                    if _stable(st) != _stable(os.fstat(sub)):
                        raise GateRefusal("live inventory directory replaced during walk")
                    walk(sub, child)
                finally:
                    os.close(sub)
            elif stat.S_ISREG(st.st_mode):
                total += st.st_size
                if total > INVENTORY_CONTENT_CAP or len(rows) >= INVENTORY_ROW_CAP:
                    raise GateRefusal("live inventory content or row cap reached")
                sub = os.open(name, os.O_RDONLY | nofollow, dir_fd=fd)
                try:
                    if _stable(st) != _stable(os.fstat(sub)):
                        raise GateRefusal("live inventory file replaced during walk")
                    h = hashlib.sha256()
                    while True:
                        bound()
                        data = os.read(sub, 1024 * 1024)
                        if not data:
                            break
                        h.update(data)
                    if _stable(st) != _stable(os.fstat(sub)):
                        raise GateRefusal("live inventory file changed during read")
                finally:
                    os.close(sub)
                rows.append([child, _stable(st), h.hexdigest()])
            elif stat.S_ISLNK(st.st_mode):
                rows.append([child, _stable(st), os.readlink(name, dir_fd=fd)])
            else:
                raise GateRefusal(f"unsupported live inventory entry: {child}")
            if _stable(st) != _stable(os.stat(name, dir_fd=fd, follow_symlinks=False)):
                raise GateRefusal("live inventory entry changed during walk")
        if _stable(before) != _stable(os.fstat(fd)):
            raise GateRefusal("live inventory directory changed during walk")

    try:
        fd = os.open(live_root, os.O_RDONLY | nofollow | directory)
    except FileNotFoundError:
        raise GateRefusal("live account root missing; isolation unproven") from None
    try:
        walk(fd, ".")
    finally:
        os.close(fd)
    if len(rows) <= 1:
        raise GateRefusal("live inventory is empty; isolation unproven")
    return rows


def retained_snapshot(roots: list, scratch_base: Path, deadline_ns: int | None = None) -> list:
    """Bounded metadata-only snapshot of every retained private root.

    Equal snapshots do not exclude transient growth and never prove a
    continuous peak; they are this gate's bracketing observation only.
    """
    snapshots = []
    count = 0
    for other in roots:
        if other == scratch_base or not other.is_relative_to(scratch_base):
            raise GateRefusal(f"retained root escapes its scratch base: {other}")
        rows = []
        size = 0

        def on_error(error: OSError, other=other):
            raise GateRefusal(f"retained root unreadable: {other}: {error}")

        for directory, dirs, files in os.walk(other, followlinks=False, onerror=on_error):
            for name in [".", *sorted(dirs), *sorted(files)]:
                path = Path(directory) if name == "." else Path(directory) / name
                st = os.lstat(path)
                if st.st_uid != os.geteuid() or not (
                        stat.S_ISDIR(st.st_mode) or stat.S_ISREG(st.st_mode)
                        or stat.S_ISLNK(st.st_mode)):
                    raise GateRefusal(f"unknown or foreign retained entry under {other}")
                count += 1
                if count > RETAINED_ROW_CAP or (deadline_ns is not None and now_ns() > deadline_ns):
                    raise GateRefusal("retained snapshot row or time cap reached")
                if name != ".":
                    size += max(st.st_size, st.st_blocks * 512)
                row = [str(path.relative_to(other)), st.st_dev, st.st_ino, st.st_mode,
                       st.st_uid, st.st_gid, st.st_size, st.st_blocks,
                       st.st_mtime_ns, st.st_ctime_ns]
                if stat.S_ISLNK(st.st_mode):
                    row.append(os.readlink(path))
                rows.append(row)
        rows.sort(key=lambda row: row[0])
        encoded = json.dumps(rows, separators=(",", ":"), ensure_ascii=True).encode()
        snapshots.append({"root": str(other), "allocated_or_logical_bytes": size,
                          "rows": len(rows), "inventory_sha256": sha256_bytes(encoded)})
    if not snapshots:
        raise GateRefusal("no retained-root snapshot was taken")
    return snapshots


def allocated_bytes(paths: list, cap: int, deadline_ns: int | None = None) -> int:
    """Count allocated blocks without following links; missing is unknown."""
    seen: set = set()
    total = 0
    entries = 0

    def charge(st: os.stat_result):
        nonlocal total, entries
        entries += 1
        if entries > ALLOC_ENTRY_CAP or (deadline_ns is not None and now_ns() > deadline_ns):
            raise GateRefusal("allocation inventory entry or time cap reached")
        key = (st.st_dev, st.st_ino)
        if key not in seen:
            seen.add(key)
            total += st.st_blocks * 512
        if total > cap:
            raise GateRefusal(f"private scratch exceeds its {cap} byte aggregate cap")
        if not (stat.S_ISDIR(st.st_mode) or stat.S_ISREG(st.st_mode) or stat.S_ISLNK(st.st_mode)):
            raise GateRefusal("unsupported private-root entry")

    def walk(fd: int):
        before = os.fstat(fd)
        with os.scandir(fd) as children:
            names = [child.name for child in children]
        for name in names:
            st = os.stat(name, dir_fd=fd, follow_symlinks=False)
            charge(st)
            if stat.S_ISDIR(st.st_mode):
                child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                try:
                    opened = os.fstat(child)
                    if (st.st_dev, st.st_ino) != (opened.st_dev, opened.st_ino):
                        raise GateRefusal("private directory changed while counted")
                    walk(child)
                finally:
                    os.close(child)
            after = os.stat(name, dir_fd=fd, follow_symlinks=False)
            if (st.st_dev, st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns) != (
                    after.st_dev, after.st_ino, after.st_mode, after.st_size, after.st_mtime_ns):
                raise GateRefusal("private entry changed while counted")
        after = os.fstat(fd)
        if (before.st_dev, before.st_ino, before.st_mtime_ns) != (
                after.st_dev, after.st_ino, after.st_mtime_ns):
            raise GateRefusal("private directory changed while counted")

    for path in paths:
        if path.parent.resolve(strict=True) != path.parent:
            raise GateRefusal(f"private-root parent is not canonical: {path}")
        st = os.lstat(path)
        charge(st)
        if stat.S_ISDIR(st.st_mode):
            fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                opened = os.fstat(fd)
                if (st.st_dev, st.st_ino) != (opened.st_dev, opened.st_ino):
                    raise GateRefusal("private root changed while counted")
                walk(fd)
            finally:
                os.close(fd)
    return total


# ---------------------------------------------------------------------------
# Environment: audited admission binding and the scrubbed child environment
# ---------------------------------------------------------------------------

def audited_admission(inherited: dict, audit: dict) -> dict:
    """Preserve only the independently audited host admission route.

    An unset or unequaled hook is not host admission; the executables and
    runtime files the audit binds are live-hashed here, and any drift refuses.
    """
    hook = inherited.get("EFFIGY_ADMISSION_HOOK")
    socket = inherited.get("QUEUE_ADMISSION_SOCKET")
    if (not isinstance(hook, str) or not hook.strip()
            or not isinstance(socket, str) or not Path(socket).is_absolute()
            or hook != audit.get("admission_hook") or socket != audit.get("admission_socket")):
        raise GateRefusal("audited host admission hook and socket required in the environment")
    for name in ("admission_executables", "admission_runtime_files"):
        files = audit.get(name)
        if not isinstance(files, list) or not files:
            raise GateRefusal(f"{name} bindings missing from the source audit")
        for item in files:
            if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
                raise GateRefusal(f"invalid {name} binding")
            path = Path(item["path"])
            if (not path.is_absolute() or path.is_symlink()
                    or path.resolve(strict=True) != path or not path.is_file()
                    or not isinstance(item["sha256"], str) or not HEX64.fullmatch(item["sha256"])
                    or sha256_file(path) != item["sha256"]):
                raise GateRefusal(f"{name} is missing or changed on this host")
    return {"EFFIGY_ADMISSION_HOOK": hook, "QUEUE_ADMISSION_SOCKET": socket}


def direct_effigy(path: Path) -> Path:
    """Bind the executable itself, never an unbound shebang shim."""
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise GateRefusal("audited Effigy must be a canonical executable file")
    with resolved.open("rb") as handle:
        head = handle.read(2)
    if head == b"#!":
        raise GateRefusal("direct audited Effigy executable required, not a script launcher")
    return resolved


def child_environment(inherited: dict, *, private: Path, oxide: Path, rustc: Path,
                      wrapper_hash: str, pool_cap: int, admission_env: dict) -> dict:
    """Construct the allowlisted scrubbed environment (accepted launcher rule).

    Never inherits OXIDE_*, XDG_*, CARGO_*, SCCACHE_* or any other unlisted
    control; only the audited admission pair passes through, from the audit.
    """
    if inherited.get("RUSTUP_TOOLCHAIN") != TOOLCHAIN or not inherited.get("RUSTUP_HOME"):
        raise GateRefusal("declared Rust toolchain binding missing in the invoking environment")
    env = {key: inherited[key] for key in ("RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "PATH")
           if key in inherited}
    if set(admission_env) != {"EFFIGY_ADMISSION_HOOK", "QUEUE_ADMISSION_SOCKET"}:
        raise GateRefusal("host admission environment is not exactly the audited pair")
    env.update(admission_env)
    env.update({
        "HOME": str(private / "home"),
        "CARGO_HOME": str(private / "cache"),
        "TMPDIR": str(private / "tmp"),
        "TMP": str(private / "tmp"),
        "TEMP": str(private / "tmp"),
        "OXIDE_CONFIG": str(private / "config.toml"),
        "OXIDE_POOL": str(private / "scratch/pool"),
        "OXIDE_TASK_SCRATCH_ROOT": str(private),
        "OXIDE_SCRATCH_ROOT": str(private / "scratch"),
        "OXIDE_POOL_BACKEND": "directory-budget",
        "OXIDE_POOL_CAP": str(pool_cap),
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "BARGE_PRIVATE_ROOT": str(private),
        "BARGE_PRIVATE_OXIDE": str(oxide),
        "BARGE_PINNED_RUSTC": str(rustc),
        "RUSTC": str(rustc),
        "BARGE_PRIVATE_WRAPPER_SHA256": wrapper_hash,
        "CARGO_NET_OFFLINE": "true",
        "RUSTUP_AUTO_INSTALL": "0",
        "RUSTUP_NO_UPDATE_CHECK": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PATH": str(private / "bin") + os.pathsep + env.get("PATH", ""),
    })
    return env


# ---------------------------------------------------------------------------
# Document builders (deterministic; no timestamps, no live sizes)
# ---------------------------------------------------------------------------

def audit_component(sources: list, analysis: str,
                    conclusion: str = "private-destinations-proven") -> dict:
    if conclusion != "private-destinations-proven":
        raise GateRefusal("an audit component may only conclude what its sources prove")
    if not isinstance(analysis, str) or len(analysis.strip()) < 40:
        raise GateRefusal("audit component analysis is missing or too short")
    return {"sources": sources, "conclusion": conclusion, "analysis": analysis}


def build_barge_audit(*, private: Path, component_sources: dict, host: dict,
                      admission: dict, child_facts: dict) -> dict:
    """Build `barge.validation-source-audit/1` from private source copies.

    `component_sources` maps each of the ten required components to
    `(analysis, [(relative_path, lines), ...])` inside `private`; every file
    must already exist there and is digest-bound. `host` carries the live
    executable digests; `admission` the hook/socket/executables/runtime
    bindings; `child_facts` the child PATH and Cargo/Python digests.
    """
    components = {}
    for name in BARGE_AUDIT_COMPONENTS:
        spec = component_sources.get(name)
        if not spec:
            raise GateRefusal(f"audit component {name} has no source binding")
        analysis, pairs = spec
        sources = []
        for relative, lines in pairs:
            path = private / relative
            real = canonical_file(path)
            if not real.is_relative_to(private):
                raise GateRefusal(f"audit source escapes the private root: {relative}")
            sources.append({"path": str(real), "sha256": sha256_file(real), "lines": lines})
        components[name] = audit_component(sources, analysis)
    return {
        "schema": SCHEMA_BARGE_AUDIT,
        "review_reference": None,
        "oxide_commit": OXIDE_PIN_COMMIT,
        "oxide_tree": OXIDE_PIN_TREE,
        "effigy_sha256": host["effigy_sha256"],
        "oxide_sha256": host["oxide_sha256"],
        "wrapper_sha256": host["wrapper_sha256"],
        "git_sha256": host["git_sha256"],
        "rustc_sha256": host["rustc_sha256"],
        "supervisor_sha256": host["supervisor_sha256"],
        "admission_hook": admission["hook"],
        "admission_socket": admission["socket"],
        "admission_executables": admission["executables"],
        "admission_runtime_files": admission["runtime_files"],
        "child_path": child_facts["child_path"],
        "cargo_sha256": child_facts["cargo_sha256"],
        "python_sha256": child_facts["python_sha256"],
        "components": {name: components[name] for name in BARGE_AUDIT_COMPONENTS},
    }


def build_barge_ledger(*, roots: list, evidence: list, review_reference) -> dict:
    """Build `barge.private-roots/1`; the review reference is planner-bound."""
    return {
        "schema": SCHEMA_BARGE_LEDGER,
        "task": BARGE_TASK,
        "review_reference": review_reference,
        "discovery_evidence": [{"path": str(path), "sha256": digest}
                               for path, digest in evidence],
        "roots": sorted(str(path) for path in roots),
    }


def build_oxide_budget(*, roots: list, history_reference) -> dict:
    """Build the accepted launcher's `budget.json` shape."""
    return {
        "task": OXIDE_TASK,
        "history_reference": history_reference,
        "covering_seconds_charged": 1200,
        "retained_roots": sorted(str(path) for path in roots),
    }


def build_oxide_history(*, roots: list, evidence: dict, complete, unknown: list) -> dict:
    """Build the accepted launcher's `history-discovery.json` shape."""
    return {
        "task": OXIDE_TASK,
        "complete": bool(complete),
        "retained_roots": sorted(str(path) for path in roots),
        "evidence": dict(sorted(evidence.items())),
        "unknown": sorted(unknown),
    }


def build_oxide_block_proposal(*, oxide_root: Path, commit: str, tree: str,
                               binding_sha256, budget_sha256, history_sha256,
                               one_use_id, quiescent) -> dict:
    """Build the accepted launcher's block proposal; planner fields stay null."""
    return {
        "task": OXIDE_TASK,
        "root": str(oxide_root),
        "planner_reference": OXIDE_PLANNER_REFERENCE,
        "original_admission_charge_seconds": 1200,
        "prospective_max_seconds": 1200,
        "source_commit": commit,
        "source_tree": tree,
        "binding_sha256": binding_sha256,
        "ledger_sha256": budget_sha256,
        "history_sha256": history_sha256,
        "one_use_id": one_use_id,
        "retained_roots_quiescent": quiescent,
    }


def build_barge_block_proposal(*, head: str, tree: str, wrapper_sha256: str,
                               audit_sha256, ledger_sha256, block_id,
                               authorization_reference, max_seconds=PROSPECTIVE_MAX_SECONDS,
                               pool_cap_bytes=None, selectors: list | None = None) -> dict:
    """Build `barge.validation-block/1` in the retained unissued-proposal form."""
    return {
        "schema": SCHEMA_BARGE_BLOCK,
        "task": BARGE_TASK,
        "block_id": block_id,
        "authorization_reference": authorization_reference,
        "head": head,
        "barge_tree": tree,
        "wrapper_sha256": wrapper_sha256,
        "oxide_sha256": None,
        "audit_sha256": audit_sha256,
        "ledger_sha256": ledger_sha256,
        "max_seconds": max_seconds,
        "scratch_cap_bytes": BARGE_CAP_BYTES,
        "pool_cap_bytes": pool_cap_bytes,
        "selectors": selectors if selectors is not None else [STATUS_SELECTOR],
    }


def build_debug_receipt(*, binding: dict, build: dict) -> dict:
    """Build the debug-Oxide receipt shape the accepted Barge launcher checks."""
    return {
        "profile": "debug",
        "test_home_from_env": True,
        "source_commit": binding["commit"],
        "source_tree": binding["tree"],
        "lock_sha256": binding["lock_sha256"],
        "toolchain_sha256": binding["toolchain_sha256"],
        "rustc_sha256": binding["rustc_sha256"],
        "executable_sha256": binding["oxide_sha256"],
        "source_files": binding["files"],
        "build": build,
    }


# ---------------------------------------------------------------------------
# Validators (rule-for-rule mirrors of the accepted launchers)
# ---------------------------------------------------------------------------

def validate_barge_block(block, *, head, wrapper_hash, oxide_hash, audit_hash,
                         ledger_hash) -> int:
    required = {"schema", "task", "block_id", "authorization_reference", "head",
                "barge_tree", "wrapper_sha256", "oxide_sha256", "audit_sha256",
                "ledger_sha256", "max_seconds", "scratch_cap_bytes",
                "pool_cap_bytes", "selectors"}
    if not isinstance(block, dict) or set(block) != required \
            or block.get("schema") != SCHEMA_BARGE_BLOCK:
        raise GateRefusal("incomplete validation block")
    if block["task"] != BARGE_TASK or block["head"] != head:
        raise GateRefusal("block task or head mismatch")
    if not isinstance(block["barge_tree"], str) or not HEX40.fullmatch(block["barge_tree"]):
        raise GateRefusal("exact Barge tree is required")
    if not isinstance(block["block_id"], str) or not BLOCK_ID.fullmatch(block["block_id"]):
        raise GateRefusal("invalid block identity")
    if not isinstance(block["authorization_reference"], str) \
            or not block["authorization_reference"].strip():
        raise GateRefusal("operator authorization reference required")
    for key, actual in (("wrapper_sha256", wrapper_hash), ("oxide_sha256", oxide_hash),
                        ("audit_sha256", audit_hash), ("ledger_sha256", ledger_hash)):
        if not isinstance(actual, str) or not HEX64.fullmatch(actual) or block[key] != actual:
            raise GateRefusal(f"block {key} mismatch")
    if type(block["max_seconds"]) is not int or block["max_seconds"] < 60:
        raise GateRefusal("explicit bounded seconds required")
    if block["max_seconds"] > PROSPECTIVE_MAX_SECONDS:
        raise GateRefusal("block exceeds the one authorized prospective maximum")
    if block["scratch_cap_bytes"] != BARGE_CAP_BYTES:
        raise GateRefusal("private scratch cap must remain 2 GiB")
    if type(block["pool_cap_bytes"]) is not int or not 0 < block["pool_cap_bytes"] <= BARGE_CAP_BYTES:
        raise GateRefusal("private pool budget is missing or exceeds scratch cap")
    selectors = block["selectors"]
    if (not isinstance(selectors, list) or not selectors
            or any(not isinstance(selector, str) for selector in selectors)
            or selectors[0] != STATUS_SELECTOR
            or len(set(selectors)) != len(selectors)
            or any(selector not in BARGE_SAFE_SELECTORS for selector in selectors[1:])):
        raise GateRefusal("block selectors are missing, duplicated or outside the safe route")
    if "rust:test:tape-stage" in selectors and "rust:test:tape-export" in selectors:
        raise GateRefusal("export already invokes the stage selector")
    return block["max_seconds"]


def validate_barge_audit(audit, *, private, host: dict) -> None:
    if not isinstance(audit, dict) or audit.get("schema") != SCHEMA_BARGE_AUDIT:
        raise GateRefusal("source audit missing")
    if not isinstance(audit.get("review_reference"), str) or not audit["review_reference"].strip():
        raise GateRefusal("independent source audit review reference required")
    if audit.get("oxide_commit") != OXIDE_PIN_COMMIT or audit.get("oxide_tree") != OXIDE_PIN_TREE:
        raise GateRefusal("Oxide source identity mismatch")
    for key, expected in host.items():
        if not isinstance(expected, str) or not HEX64.fullmatch(expected) \
                or audit.get(key) != expected:
            raise GateRefusal(f"audit {key} mismatch")
    components = audit.get("components")
    if not isinstance(components, dict) or set(components) != set(BARGE_AUDIT_COMPONENTS):
        raise GateRefusal("transitive audit components incomplete")
    for name, component in components.items():
        if (not isinstance(component, dict)
                or set(component) != {"sources", "conclusion", "analysis"}):
            raise GateRefusal(f"{name}: malformed audit component")
        if component["conclusion"] != "private-destinations-proven":
            raise GateRefusal(f"{name}: private destination unresolved")
        if not isinstance(component["analysis"], str) or len(component["analysis"].strip()) < 40:
            raise GateRefusal(f"{name}: source analysis missing")
        sources = component["sources"]
        if not isinstance(sources, list) or not sources:
            raise GateRefusal(f"{name}: source evidence missing")
        for item in sources:
            if not isinstance(item, dict) or set(item) != {"path", "sha256", "lines"}:
                raise GateRefusal(f"{name}: invalid source binding")
            path = Path(item["path"])
            expected = item["sha256"]
            if (not path.is_absolute() or not path.is_relative_to(private)
                    or not isinstance(expected, str) or not HEX64.fullmatch(expected)
                    or not isinstance(item["lines"], str) or not item["lines"].strip()):
                raise GateRefusal(f"{name}: invalid source identity")
            if (path.is_symlink() or path.resolve(strict=True) != path
                    or not path.is_file() or sha256_file(path) != expected):
                raise GateRefusal(f"{name}: source changed or unreadable")


def validate_barge_ledger(ledger, *, private_root: Path, scratch: Path,
                          historic: Path = BARGE_HISTORIC_LEDGER,
                          scratch_glob: str = "041-pr.*",
                          required_roots=None) -> list:
    if not isinstance(ledger, dict) or ledger.get("schema") != SCHEMA_BARGE_LEDGER \
            or ledger.get("task") != BARGE_TASK:
        raise GateRefusal("task private-root ledger missing")
    if not isinstance(ledger.get("review_reference"), str) \
            or not ledger["review_reference"].strip():
        raise GateRefusal("retained-root discovery review reference required")
    evidence = ledger.get("discovery_evidence")
    if not isinstance(evidence, list) or not evidence:
        raise GateRefusal("retained-root discovery evidence missing")
    for item in evidence:
        if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
            raise GateRefusal("malformed root-discovery evidence")
        path = Path(item["path"])
        if (not path.is_absolute() or not path.is_relative_to(scratch) or path.is_symlink()
                or path.resolve(strict=True) != path or not path.is_file()
                or not isinstance(item["sha256"], str) or not HEX64.fullmatch(item["sha256"])
                or sha256_file(path) != item["sha256"]):
            raise GateRefusal("root-discovery evidence is missing or changed")
    roots = ledger.get("roots")
    if not isinstance(roots, list) or not roots or any(not isinstance(p, str) for p in roots):
        raise GateRefusal("private-root ledger is empty or malformed")
    paths = [Path(p) for p in roots]
    if any(not p.is_absolute() or p == scratch or not p.is_relative_to(scratch) for p in paths):
        raise GateRefusal("private-root ledger escapes scratch")
    required = set(required_roots if required_roots is not None else BARGE_REQUIRED_ROOTS) \
        | {private_root}
    if not required.issubset(set(paths)) or len(set(paths)) != len(paths):
        raise GateRefusal("known retained roots or new root missing")
    if not historic.is_file():
        raise GateRefusal("historical retained ledger is unavailable")
    history = json.loads(historic.read_text(encoding="utf-8"))
    if history.get("taskId") != BARGE_TASK:
        raise GateRefusal("historical ledger task mismatch")
    known = [Path(item["path"]) for item in history.get("known_inner_runtime_logs", [])]
    if not known:
        raise GateRefusal("historical retained log list unavailable")
    known.extend(sorted(scratch.glob(scratch_glob)))
    for path in known:
        if not any(path == root or root in path.parents for root in paths):
            raise GateRefusal("identified task-owned evidence omitted from root ledger")
    for item in evidence:
        item_path = Path(item["path"])
        if not any(item_path == root or root in item_path.parents for root in paths):
            raise GateRefusal("root-discovery evidence is not counted")
    for index, left in enumerate(paths):
        if any(left in right.parents or right in left.parents for right in paths[index + 1:]):
            raise GateRefusal("overlapping private roots are ambiguous")
    return paths


def validate_oxide_budget(budget, *, run_root: Path, scratch: Path) -> list:
    if (not isinstance(budget, dict) or budget.get("task") != OXIDE_TASK
            or not isinstance(budget.get("history_reference"), str)
            or not budget["history_reference"].strip()):
        raise GateRefusal("oxide budget ledger is missing its task or history reference")
    charged = budget.get("covering_seconds_charged")
    if not isinstance(charged, int) or isinstance(charged, bool) or charged != 1200:
        raise GateRefusal("original allowance must be charged as 1200, not measured")
    roots = budget.get("retained_roots")
    if not isinstance(roots, list):
        raise GateRefusal("oxide budget retained-root list is missing")
    paths = [Path(p) for p in roots]
    if (len(paths) < 5 or len(paths) != len(set(paths)) or run_root not in set(paths)
            or any(not p.is_absolute() or p == scratch or not p.is_relative_to(scratch)
                   for p in paths)):
        raise GateRefusal("incomplete retained-root list")
    if any(a != b and a.is_relative_to(b) for a in paths for b in paths):
        raise GateRefusal("overlapping retained roots")
    return paths


def validate_oxide_history(history, budget) -> None:
    if (not isinstance(history, dict) or history.get("task") != OXIDE_TASK
            or history.get("complete") is not True
            or history.get("retained_roots") != budget.get("retained_roots")):
        raise GateRefusal("incomplete root discovery or roots disagree with the budget ledger")
    evidence = history.get("evidence")
    if not isinstance(evidence, dict) or not evidence:
        raise GateRefusal("root discovery evidence is missing")
    for name, digest in evidence.items():
        if not isinstance(name, str) or not name.strip() or not isinstance(digest, str) \
                or not HEX64.fullmatch(digest):
            raise GateRefusal("malformed root-discovery evidence digest")
    unknown = history.get("unknown")
    if not isinstance(unknown, list) or any(not isinstance(u, str) for u in unknown):
        raise GateRefusal("root discovery unknowns must be an honest string list")


def clamp_mib(cap_bytes: int, retained_bytes: int, per_run_cap_mib: int) -> int:
    """The effective whole-block scratch: what retained evidence leaves.

    This is a clamp, never a claim that a phase fits the remainder and never
    a silent cap increase; a non-positive remainder is an exact shortfall.
    """
    if (type(cap_bytes) is not int or type(retained_bytes) is not int
            or type(per_run_cap_mib) is not int or cap_bytes <= 0 or retained_bytes < 0):
        raise GateRefusal("clamp inputs must be real byte and MiB figures")
    if retained_bytes >= cap_bytes:
        raise GateRefusal(f"retained evidence fills the aggregate grant; "
                          f"{retained_bytes} of {cap_bytes} bytes leaves no run budget")
    effective_mib = (cap_bytes - retained_bytes) // (1024 * 1024)
    return max(0, min(effective_mib, per_run_cap_mib))


def validate_oxide_block(block, *, run_root: Path, commit: str, tree: str,
                         binding_sha256: str, budget_sha256: str,
                         history_sha256: str,
                         planner_reference: str = OXIDE_PLANNER_REFERENCE) -> str:
    fields = {"task", "root", "planner_reference", "original_admission_charge_seconds",
              "prospective_max_seconds", "source_commit", "source_tree",
              "binding_sha256", "ledger_sha256", "history_sha256", "one_use_id",
              "retained_roots_quiescent"}
    if not isinstance(block, dict) or set(block) != fields:
        raise GateRefusal("invalid block authorization fields")
    if not isinstance(planner_reference, str) or not planner_reference.strip():
        raise GateRefusal("the expected planner authorization reference must be bound")
    if (block.get("task") != OXIDE_TASK or block.get("root") != str(run_root)
            or block.get("planner_reference") != planner_reference
            or block.get("original_admission_charge_seconds") != 1200
            or block.get("prospective_max_seconds") != 1200
            or block.get("source_commit") != commit or block.get("source_tree") != tree
            or block.get("binding_sha256") != binding_sha256
            or block.get("ledger_sha256") != budget_sha256
            or block.get("history_sha256") != history_sha256
            or block.get("retained_roots_quiescent") is not True):
        raise GateRefusal("planner block does not bind this context, history and source")
    one_use = block.get("one_use_id")
    if not isinstance(one_use, str) or not HEX64.fullmatch(one_use):
        raise GateRefusal("invalid one-use block identity")
    return one_use


def validate_supervisor_receipt(receipt, expected_command: list, private: Path) -> None:
    outcome = receipt.get("outcome", {})
    census = receipt.get("census", {})
    if (receipt.get("command") != expected_command or receipt.get("scratch") != str(private)
            or outcome.get("closure") != "closed" or outcome.get("status") != 0
            or outcome.get("fault") is not None or outcome.get("deadline") is not False
            or outcome.get("launched") is not True or census.get("certain") is not True
            or census.get("members") != 0 or census.get("uncertain") != 0):
        raise GateRefusal("supervisor receipt does not prove successful closure")


def verify_probe(stdout: str, *, home: Path, config: Path, pool: Path,
                 registry_probe: Path, cache_probe: Path) -> None:
    """Check the real recorded-status output, including both unique sentinels."""
    lines = stdout.splitlines()
    required = (
        f"config: {config} (selected by OXIDE_CONFIG)",
        f"  home: HOME {home} (proof control OXIDE_TEST_HOME_FROM_ENV)",
        f"other pool path: {registry_probe} (unrecognised; left alone)",
        f"other pool path: {cache_probe} (unrecognised; left alone)",
    )
    if any(lines.count(line) != 1 for line in required):
        raise GateRefusal("effective config, home, state or cache sentinel mismatch")
    pools = [line for line in lines if line.startswith("pool: ")]
    if len(pools) != 1 or not pools[0].startswith(f"pool: {pool} (") or "unavailable" in pools[0]:
        raise GateRefusal("effective private pool was not positively resolved")
    if any("cannot open:" in line for line in lines):
        raise GateRefusal("status refused the private pool")


def verify_inventory(before: list, after: list) -> None:
    if before != after:
        raise GateRefusal("live inventory changed; isolation failed")


# ---------------------------------------------------------------------------
# Source archives (read-only Git, exact tree and mode binding)
# ---------------------------------------------------------------------------

def git_output(repo: Path, *args: str) -> bytes:
    env = {"PATH": "/usr/bin:/bin", "HOME": "/nonexistent", "GIT_CONFIG_NOSYSTEM": "1",
           "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_OPTIONAL_LOCKS": "0"}
    result = subprocess.run(["/usr/bin/git", "-C", str(repo), *args], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if result.returncode != 0 or len(result.stdout) > 64 * 1024 * 1024:
        raise GateRefusal(f"read-only git {args[0]} failed or exceeded its bound")
    return result.stdout


def extract_git_tree(repo: Path, root: Path, commit: str, expected_tree: str) -> dict:
    """Materialize and verify every archived file and mode against the tree."""
    tree = git_output(repo, "rev-parse", commit + "^{tree}").decode("ascii").strip()
    resolved = git_output(repo, "rev-parse", commit + "^{commit}").decode("ascii").strip()
    if tree != expected_tree or resolved != commit:
        raise GateRefusal("source commit or tree mismatch")
    records = git_output(repo, "ls-tree", "-rz", "--full-tree", commit).split(b"\0")
    expected = {}
    for record in records:
        if not record:
            continue
        header, raw = record.split(b"\t", 1)
        mode, kind, blob = header.decode("ascii").split(" ")
        name = raw.decode("utf-8")
        if (kind != "blob" or mode not in ("100644", "100755", "120000")
                or not HEX40.fullmatch(blob) or name in expected):
            raise GateRefusal("unsupported source tree member")
        relative = Path(name)
        if relative.is_absolute() or any(part in ("", ".", "..") for part in relative.parts):
            raise GateRefusal("invalid source tree path")
        path = root / relative
        current = root
        for component in relative.parts[:-1]:
            current = current / component
            if current.is_symlink() or not current.is_dir():
                raise GateRefusal("source tree parent is not a real directory")
        if mode == "120000":
            if not path.is_symlink():
                raise GateRefusal("source symlink mismatch")
            content = os.fsencode(os.readlink(path))
        else:
            if path.is_symlink() or not path.is_file():
                raise GateRefusal("source file mismatch")
            if bool(path.stat().st_mode & 0o111) != (mode == "100755"):
                raise GateRefusal("source executable mode mismatch")
            content = path.read_bytes()
        if hashlib.sha1(b"blob " + str(len(content)).encode("ascii")
                        + b"\0" + content).hexdigest() != blob:
            raise GateRefusal("source bytes differ from the Git tree")
        expected[name] = sha256_bytes(content)
    actual = {p.relative_to(root).as_posix() for p in root.rglob("*")
              if p.is_file() or p.is_symlink()}
    if actual != set(expected):
        raise GateRefusal("source archive membership mismatch")
    expected_dirs = {parent.as_posix() for name in expected
                     for parent in Path(name).parents if parent != Path(".")}
    actual_dirs = {p.relative_to(root).as_posix() for p in root.rglob("*")
                   if p.is_dir() and not p.is_symlink()}
    if actual_dirs != expected_dirs:
        raise GateRefusal("source archive directory mismatch")
    return expected


# ---------------------------------------------------------------------------
# The one global allowance (CLOCK_UPTIME_RAW; never reset, never replayed)
# ---------------------------------------------------------------------------

class Allowance:
    """One global prospective allowance across setup/bootstrap/gates/exports.

    Bounded by the issued block maximum and, under the qualified supervisor,
    its exported absolute `OXIDE_PROOF_DEADLINE_NS`. Spend records are
    exclusive, append-only files; a second claim or any reset refuses.
    """

    def __init__(self, max_seconds: int, deadline_ns: int | None, state_dir: Path | None):
        if type(max_seconds) is not int or max_seconds < 60:
            raise GateRefusal("the allowance needs an explicit bounded maximum")
        self.max_seconds = max_seconds
        self.deadline_ns = deadline_ns
        self.state_dir = state_dir
        self.started_ns = now_ns()
        self.spent_ns = 0
        self.bootstrap_started_ns: int | None = None
        self.bootstrap_finished_ns: int | None = None
        self._phase_started: int | None = None
        self._records = 0
        if state_dir is not None:
            existing = sorted(state_dir.glob("allowance.*.json"))
            if existing:
                raise GateRefusal("allowance records already exist; no second claim or reset")

    def remaining_seconds(self) -> int:
        remaining = self.max_seconds - self.spent_ns // 1_000_000_000
        if self.deadline_ns is not None:
            remaining = min(remaining, max(0, (self.deadline_ns - now_ns()) // 1_000_000_000))
        return max(0, remaining)

    def require(self, need_seconds: int, phase: str):
        if self.remaining_seconds() < need_seconds:
            raise GateRefusal(f"authorized prospective block exhausted before {phase}")

    def begin(self, phase: str):
        self.require(1, phase)
        if self._phase_started is not None:
            raise GateRefusal(f"phase {phase} started while another phase is open")
        self._phase_started = now_ns()

    def finish(self, phase: str):
        if self._phase_started is None:
            raise GateRefusal(f"phase {phase} finished without its begin")
        end = now_ns()
        self.spent_ns += end - self._phase_started
        self._record(phase, self._phase_started, end)
        self._phase_started = None

    def begin_bootstrap(self):
        if self.bootstrap_started_ns is not None:
            raise GateRefusal("the single debug bootstrap was already started; no replay")
        self.require(1, "bootstrap")
        self.bootstrap_started_ns = now_ns()

    def bootstrap_deadline_ns(self) -> int:
        if self.bootstrap_started_ns is None:
            raise GateRefusal("bootstrap window claimed before it started")
        cap = self.bootstrap_started_ns + BOOTSTRAP_MAX_SECONDS * 1_000_000_000
        if self.deadline_ns is not None:
            cap = min(cap, self.deadline_ns)
        return cap

    def finish_bootstrap(self):
        if self.bootstrap_started_ns is None or self.bootstrap_finished_ns is not None:
            raise GateRefusal("bootstrap window is not open")
        self.bootstrap_finished_ns = now_ns()
        self.spent_ns += self.bootstrap_finished_ns - self.bootstrap_started_ns
        self._record("bootstrap", self.bootstrap_started_ns, self.bootstrap_finished_ns)

    def document(self) -> dict:
        return {
            "clock": "CLOCK_UPTIME_RAW",
            "max_seconds": self.max_seconds,
            "spent_seconds": self.spent_ns // 1_000_000_000,
            "remaining_seconds": self.remaining_seconds(),
            "bootstrap_max_seconds": BOOTSTRAP_MAX_SECONDS,
            "bootstrap_started_ns": self.bootstrap_started_ns,
            "bootstrap_finished_ns": self.bootstrap_finished_ns,
            "deadline_ns": self.deadline_ns,
            "started_ns": self.started_ns,
        }

    def _record(self, phase: str, start_ns: int, end_ns: int):
        if self.state_dir is None:
            return
        self._records += 1
        private_write(self.state_dir, Path(f"allowance.{self._records:04d}.json"),
                      canonical_json({
                          "started_ns": self.started_ns,
                          "max_seconds": self.max_seconds,
                          "spent_ns": self.spent_ns,
                          "phase": phase, "start_ns": start_ns, "end_ns": end_ns,
                          "bootstrap_started_ns": self.bootstrap_started_ns,
                      }))


# ---------------------------------------------------------------------------
# Discovery: candidate roots from retained records, honestly unknown
# ---------------------------------------------------------------------------

def discover_barge_roots(extra_roots: list) -> tuple:
    """Candidate Barge roots from the retained ledger, records and globs.

    Returns (roots, evidence) where evidence pairs retained record files with
    their digests. Existence and quiescence are NOT claimed here: the ledger's
    review reference and the issued block carry that admission.
    """
    if not BARGE_HISTORIC_LEDGER.is_file():
        raise GateRefusal("retained Barge budget ledger is unavailable for discovery")
    history = json.loads(BARGE_HISTORIC_LEDGER.read_text(encoding="utf-8"))
    if history.get("taskId") != BARGE_TASK:
        raise GateRefusal("retained Barge budget ledger names another task")
    roots = {BARGE_SCRATCH / "041-budget-ledger-20261102",
             BARGE_SCRATCH / "b041-final.n0hH3Cez",
             BARGE_PACKET}
    for item in history.get("known_inner_runtime_logs", []):
        path = Path(item["path"])
        if not path.is_relative_to(BARGE_SCRATCH):
            raise GateRefusal("retained log escapes Barge scratch")
        roots.add(path if path.is_dir() else path.parent)
    roots.update(BARGE_SCRATCH.glob("041-pr.*"))
    for name in ("planner-041-approved-block-20261102", "planner-041-author-error-20261102",
                 "planner-041-revision-disposition-20261102",
                 "planner-041-validation-unblock-20261102", "planner-unblock-041-20261102"):
        candidate = BARGE_SCRATCH / name
        if candidate.exists():
            roots.add(candidate)
    roots.update(Path(p) for p in extra_roots)
    evidence = []
    for path in (BARGE_HISTORIC_LEDGER, BARGE_PACKET / "root-discovery.json",
                 BARGE_PACKET / "queue-evidence.json",
                 BARGE_PACKET / "decision-evidence.json", BARGE_PACKET / "pr-41.json"):
        evidence.append((path, sha256_file(canonical_file(path))))
    return sorted(roots), evidence


OXIDE_DISCOVERY_PACKET = OXIDE_SCRATCH / "planner-041-240-unblock-20261102"
OXIDE_OWNER_PACKET = OXIDE_SCRATCH / "planner-041-240-owner-disposition-20261102"


def discover_oxide_roots(extra_roots: list) -> tuple:
    """Candidate Oxide #240 roots from the retained planner snapshots.

    Task-owned Builds roots recorded by Queue's run/cleanup records for task
    d41de092 plus the retained planner packet directories. Names or reference
    matches alone are not ownership proofs; the discovery stays honest about
    that and the planner's issued quiescence admission decides.
    """
    roots = {OXIDE_DISCOVERY_PACKET, OXIDE_OWNER_PACKET,
             OXIDE_SCRATCH / "planner-clock-and-residue-20261102"}
    for name in ("oxide240.CVGVXlGm", "residue-240-20261102", "v.GcY4", "x.PFjS"):
        candidate = OXIDE_SCRATCH / name
        if candidate.exists():
            roots.add(candidate)
    roots.update(sorted(OXIDE_SCRATCH.glob("tape-prune-*")))
    roots.update(Path(p) for p in extra_roots)
    # Review 6584174560 finding 8: the planner-240 directories and review-230
    # are non-root evidence (disposition pending), and oxide-233-fix.* is not
    # attributable. They are named honestly in the history unknowns, never
    # silently counted, and never read.
    non_root_evidence = sorted(
        str(p.name) for p in OXIDE_SCRATCH.glob("planner-240-*")
        if p.is_dir() and p not in roots) + sorted(
        str(p.name) for p in OXIDE_SCRATCH.glob("review-230*")
        if p.is_dir() and p not in roots)
    non_attributable = sorted(str(p.name) for p in OXIDE_SCRATCH.glob("oxide-233-fix.*")
                              if p not in roots)
    unknown = [
        "quiescence of every retained root is not measured by discovery; the "
        "planner's issued admission decides",
        "shared live pool/output sizes stay unknown and unread",
        "the current #240 launcher head is recorded, not approved",
        "planner_reference must be confirmed by the planner at binding; the "
        "proposal carries the brief-v11 string and the held brief is now v12",
    ]
    if non_root_evidence:
        unknown.append("planner-240/review-230 directories are retained non-root "
                       "evidence with disposition pending, outside this ledger: "
                       + ", ".join(non_root_evidence))
    if non_attributable:
        unknown.append("directories not attributable to any task, never read or "
                       "counted: " + ", ".join(non_attributable))
    evidence = {}
    for packet, name in ((OXIDE_DISCOVERY_PACKET, "240-queue-detail.json"),
                         (OXIDE_DISCOVERY_PACKET, "240-task-next.json"),
                         (OXIDE_DISCOVERY_PACKET, "240-source-prep-result.json"),
                         (OXIDE_DISCOVERY_PACKET, "240-resource-amend-result.json"),
                         (OXIDE_DISCOVERY_PACKET, "041-queue-detail.json"),
                         (OXIDE_DISCOVERY_PACKET, "041-task-next.json"),
                         (OXIDE_DISCOVERY_PACKET, "041-validation-spend-decision.json"),
                         (OXIDE_OWNER_PACKET, "240-owner-source-disposition.json")):
        path = packet / name
        if path.is_file():
            evidence[str(canonical_file(path))] = sha256_file(path)
    if not evidence:
        raise GateRefusal("no retained Oxide discovery evidence is available")
    return sorted(roots), evidence, unknown


def oxide_effective_caps(oxide_budget_roots: list, run_root: Path) -> dict:
    """Measure retained Oxide bytes and derive the disclosed effective caps."""
    outside = allocated_bytes([p for p in oxide_budget_roots if p != run_root],
                              OXIDE_CAP_BYTES)
    effective_mib = clamp_mib(OXIDE_CAP_BYTES, outside, OXIDE_CAP_BYTES // (1024 * 1024))
    return {"retained_outside_bytes": outside,
            "effective_whole_block_mib": effective_mib}


# ---------------------------------------------------------------------------
# Receipt
# ---------------------------------------------------------------------------

def gate_receipt(*, gate_id: str, blocks: dict, allowance: dict, bindings: dict,
                 commands: list, observations: dict, exports: dict, failures: list,
                 decisions: list) -> dict:
    return {
        "schema": SCHEMA_GATE_RECEIPT,
        "gate_id": gate_id,
        "qualification": {"state": "unqualified",
                          "reason": "receipt judgment is retained Queue review, never this tool"},
        "blocks": blocks,
        "allowance": allowance,
        "bindings": bindings,
        "commands": commands,
        "observations": observations,
        "exports": exports,
        "failures": failures,
        "decisions": decisions,
    }


# ---------------------------------------------------------------------------
# Modes
# ---------------------------------------------------------------------------

USAGE = """usage:
  test-private-build-validation.py --self-test
  test-private-build-validation.py --plan
  test-private-build-validation.py --prepare --packet-dir DIR --oxide-repo REPO
      --oxide-head HEX40 --barge-repo REPO --barge-head HEX40
      --barge-root ROOT --oxide-root ROOT
  test-private-build-validation.py --diagnostic --oxide-repo REPO --oxide-head HEX40
      --barge-repo REPO --barge-head HEX40 --barge-root ROOT --oxide-root ROOT
      --expect-audit HEX64 --expect-ledger HEX64 --expect-budget HEX64 --expect-history HEX64
      [--planner-reference STRING]
  diagnostic environment: OXIDE_PROOF_DEADLINE_NS, OXIDE_PROOF_RUN_ID,
      OXIDE_PROOF_EVIDENCE, OXIDE_TASK_SCRATCH_ROOT (exported by the qualified
      supervisor), BARGE_VALIDATION_BLOCK[+_SHA256],
      BARGE_VALIDATION_SOURCE_AUDIT[+_SHA256], BARGE_VALIDATION_ROOT_LEDGER[+_SHA256],
      BARGE_VALIDATION_EFFIGY, BARGE_VALIDATION_GIT, BARGE_VALIDATION_SUPERVISOR,
      BARGE_PINNED_RUSTC, TAPE_BLOCK_APPROVAL_SHA256, RUSTUP_HOME, RUSTUP_TOOLCHAIN"""


def parse_args(argv: list) -> dict:
    modes = ("--self-test", "--plan", "--prepare", "--diagnostic")
    if not argv or argv[0] not in modes:
        raise GateRefusal("unknown or missing mode; individual actions are disabled\n" + USAGE)
    options: dict = {"mode": argv[0][2:]}
    required = {
        "prepare": ("packet_dir", "oxide_repo", "oxide_head", "barge_repo", "barge_head",
                    "barge_root", "oxide_root"),
        "diagnostic": ("oxide_repo", "oxide_head", "barge_repo", "barge_head", "barge_root",
                       "oxide_root", "expect_audit", "expect_ledger", "expect_budget",
                       "expect_history"),
    }.get(options["mode"], ())
    index = 1
    while index < len(argv):
        flag = argv[index]
        if not flag.startswith("--") or index + 1 >= len(argv):
            raise GateRefusal(f"malformed argument {flag!r}\n" + USAGE)
        options[flag[2:].replace("-", "_")] = argv[index + 1]
        index += 2
    for flag in required:
        if flag not in options:
            raise GateRefusal(f"{options['mode']} requires --{flag.replace('_', '-')}\n" + USAGE)
    for key in ("expect_audit", "expect_ledger", "expect_budget", "expect_history"):
        if key in options and not HEX64.fullmatch(options[key]):
            raise GateRefusal(f"--{key.replace('_', '-')} must be 64 hex")
    for key in ("oxide_head", "barge_head"):
        if key in options and not HEX40.fullmatch(options[key]):
            raise GateRefusal(f"--{key.replace('_', '-')} must be 40 hex")
    return options

def repo_guard(path_text: str) -> Path:
    repo = Path(path_text)
    if not repo.is_absolute() or repo.resolve(strict=True) != repo \
            or not (repo / ".git").exists():
        raise GateRefusal(f"source repository must be a canonical checkout: {path_text}")
    return repo


def root_guard(path_text: str, scratch: Path) -> Path:
    root = Path(path_text)
    if not root.is_absolute() or root == scratch or not root.is_relative_to(scratch):
        raise GateRefusal(f"task root must be a direct child plan under {scratch}: {path_text}")
    if any(part in ("", ".", "..") for part in root.relative_to(scratch).parts[1:]):
        raise GateRefusal("task root path has unsafe components")
    return root


def supervisor_context() -> tuple:
    """Require the qualified supervisor's exported context; no fallback.

    Every effect-carrying mode runs only as the whole command of the qualified
    supervisor: its run identity binds every claim and allowance record, and
    its reserve/scratch watchdog covers the whole command from the first walk.
    """
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    run_id = os.environ.get("OXIDE_PROOF_RUN_ID", "")
    deadline = os.environ.get("OXIDE_PROOF_DEADLINE_NS", "")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    if len(run_id) != 32 or not all(c in "0123456789abcdef" for c in run_id):
        raise GateRefusal("missing outer qualified supervisor run identity; no unguarded execution")
    if not deadline.isdigit():
        raise GateRefusal("missing outer qualified supervisor deadline; no unguarded execution")
    evidence_path = Path(evidence) if evidence else None
    if evidence_path is None or not evidence_path.is_absolute() or not evidence_path.is_dir():
        raise GateRefusal("supervisor evidence directory is absent")
    scratch_path = Path(scratch) if scratch else None
    if scratch_path is None or not scratch_path.is_absolute() or not scratch_path.is_dir():
        raise GateRefusal("the qualified supervisor's scratch root is absent")
    return evidence_path, run_id, int(deadline), scratch_path


def env_document(name: str, digest_name: str, under: Path) -> tuple:
    path_text = os.environ.get(name, "")
    expected = os.environ.get(digest_name, "")
    if not path_text or not HEX64.fullmatch(expected):
        raise GateRefusal(f"missing {name} or {digest_name} binding")
    path = Path(path_text)
    if path.is_symlink() or path.resolve(strict=True) != path or not path.is_file() \
            or not path.is_relative_to(under):
        raise GateRefusal(f"{name} must be a retained private file under {under}")
    return path, expected


def run_supervised(*, supervisor: Path, python: Path, command: list, cwd: Path, env: dict,
                   limit_seconds: int, scratch_mib: int, evidence: Path,
                   allowance: Allowance, phase: str) -> tuple:
    """One nested qualified-supervised child; its receipt must close cleanly."""
    if limit_seconds < 1 or scratch_mib < 1:
        raise GateRefusal(f"{phase} has no wall or scratch allowance left")
    argv = [str(python), str(supervisor), "--limit", str(limit_seconds),
            "--scratch-budget-mib", str(scratch_mib), "--evidence", str(evidence),
            "--", *map(str, command)]
    stdout_path = evidence.parent / (phase + "-stdout.log")
    stderr_path = evidence.parent / (phase + "-stderr.log")
    allowance.begin(phase)
    result = None
    try:
        with stdout_path.open("xb") as out, stderr_path.open("xb") as err:
            result = subprocess.run(argv, cwd=str(cwd), env=env, check=False,
                                    stdout=out, stderr=err)
    finally:
        allowance.finish(phase)
    if result is None or result.returncode != 0:
        code = getattr(result, "returncode", "missing")
        raise GateRefusal(f"{phase} did not close successfully (exit {code})")
    if stdout_path.stat().st_size > 32 * 1024 * 1024 or stderr_path.stat().st_size > 8 * 1024 * 1024:
        raise GateRefusal(f"{phase} output exceeds its retained evidence bound")
    receipt_doc = json.loads((evidence / "receipt.json").read_text(encoding="utf-8"))
    return receipt_doc, stdout_path.read_text(encoding="utf-8", errors="strict"), argv


def parse_block_head(block: dict) -> int:
    """Minimal early field checks before the full post-bootstrap validation."""
    if not isinstance(block, dict) or block.get("task") != BARGE_TASK:
        raise GateRefusal("issued Barge block does not name this task")
    if not isinstance(block.get("block_id"), str) or not BLOCK_ID.fullmatch(block["block_id"]):
        raise GateRefusal("issued Barge block identity is malformed")
    pool_cap = block.get("pool_cap_bytes")
    if type(pool_cap) is not int or not 0 < pool_cap <= BARGE_CAP_BYTES:
        raise GateRefusal("issued Barge block pool budget is missing or out of range")
    max_seconds = block.get("max_seconds")
    if type(max_seconds) is not int or not 60 <= max_seconds <= PROSPECTIVE_MAX_SECONDS:
        raise GateRefusal("issued Barge block wall bound is missing or out of range")
    return max_seconds


def diagnostic(options: dict) -> int:
    """The planner's one proofExecution: refuse before any effect on any gap."""
    python = Path(sys.executable).resolve(strict=True)
    evidence, run_id, deadline_ns, adopted_scratch = supervisor_context()
    allowance_dir = evidence / "private-build-validation.allowance"
    barge_root = root_guard(options["barge_root"], BARGE_SCRATCH)
    oxide_root = root_guard(options["oxide_root"], OXIDE_SCRATCH)
    barge_repo = repo_guard(options["barge_repo"])
    oxide_repo = repo_guard(options["oxide_repo"])
    barge_head = options["barge_head"]
    oxide_head = options["oxide_head"]

    # Issued documents first: nothing is created before every binding checks.
    block_path, block_digest = env_document("BARGE_VALIDATION_BLOCK",
                                            "BARGE_VALIDATION_BLOCK_SHA256", BARGE_SCRATCH)
    audit_path, audit_digest = env_document("BARGE_VALIDATION_SOURCE_AUDIT",
                                            "BARGE_VALIDATION_SOURCE_AUDIT_SHA256", BARGE_SCRATCH)
    ledger_path, ledger_digest = env_document("BARGE_VALIDATION_ROOT_LEDGER",
                                              "BARGE_VALIDATION_ROOT_LEDGER_SHA256", BARGE_SCRATCH)
    if sha256_file(audit_path) != options["expect_audit"] \
            or sha256_file(ledger_path) != options["expect_ledger"]:
        raise GateRefusal("issued audit or ledger does not match the bound expectation")
    oxide_block_path = oxide_root / "block-authorization.json"
    approval = os.environ.get("TAPE_BLOCK_APPROVAL_SHA256", "")
    if not HEX64.fullmatch(approval) or sha256_file(oxide_block_path) != approval:
        raise GateRefusal("oxide block authorization is missing or does not match its digest")
    if sha256_file(oxide_root / "budget.json") != options["expect_budget"] \
            or sha256_file(oxide_root / "history-discovery.json") != options["expect_history"]:
        raise GateRefusal("oxide budget or history document does not match the bound expectation")
    budget_doc = json.loads((oxide_root / "budget.json").read_bytes())
    history_doc = json.loads((oxide_root / "history-discovery.json").read_bytes())
    oxide_budget_roots = validate_oxide_budget(budget_doc, run_root=oxide_root, scratch=OXIDE_SCRATCH)
    validate_oxide_history(history_doc, budget_doc)
    oxide_block = json.loads(oxide_block_path.read_bytes())
    oxide_commit_tree = extract_commit_identity(oxide_repo, oxide_head)
    validate_oxide_block(oxide_block, run_root=oxide_root, commit=oxide_commit_tree[0],
                         tree=oxide_commit_tree[1],
                         binding_sha256=oxide_block.get("binding_sha256"),
                         budget_sha256=options["expect_budget"],
                         history_sha256=options["expect_history"],
                         planner_reference=options.get("planner_reference",
                                                       OXIDE_PLANNER_REFERENCE))

    # Live executable bindings (read-only facts; any drift refuses).
    effigy = direct_effigy(Path(os.environ.get("BARGE_VALIDATION_EFFIGY", "")))
    git_tool = Path(os.environ.get("BARGE_VALIDATION_GIT", "")).resolve(strict=True)
    supervisor_tool = Path(os.environ.get("BARGE_VALIDATION_SUPERVISOR", "")).resolve(strict=True)
    rustc_tool = Path(os.environ.get("BARGE_PINNED_RUSTC", "")).resolve(strict=True)
    if sha256_file(supervisor_tool, cap=512 * 1024) != SUPERVISOR_SHA256:
        raise GateRefusal("qualified supervisor bytes are not the pinned qualified version")
    inherited = dict(os.environ)
    audit_doc = read_json_bound(audit_path, audit_digest)
    admission_env = audited_admission(inherited, audit_doc)
    invoking_cargo_home = os.environ.get("CARGO_HOME", "")
    if not invoking_cargo_home or not Path(invoking_cargo_home).is_dir():
        raise GateRefusal("invoking CARGO_HOME is required as the offline seed source")

    # The one-use claim across both issued blocks, in this gate's evidence.
    allowance = Allowance(parse_block_head(json.loads(block_path.read_bytes())),
                          deadline_ns, allowance_dir)
    claim = {"gate_id": run_id,
             "barge_block_id": json.loads(block_path.read_bytes()).get("block_id"),
             "oxide_one_use_id": oxide_block.get("one_use_id"),
             "barge_block_sha256": block_digest, "oxide_block_sha256": approval,
             "started_ns": allowance.started_ns}
    private_write(allowance_dir, Path("claim.json"), canonical_json(claim))

    # Materialize the prepared roots; the archives are verified byte-exactly.
    allowance.begin("setup")
    for root in (barge_root, oxide_root):
        st = os.lstat(root)
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_mode & 0o077:
            raise GateRefusal(f"prepared task root is not owner-only: {root}")
    oxide_source = private_dir(barge_root, Path("oxide-source"))
    barge_source = private_dir(barge_root, Path("barge-source"))
    _, oxide_tree = extract_commit_identity(oxide_repo, OXIDE_PIN_COMMIT)
    oxide_files = extract_git_tree(oxide_repo, oxide_source, OXIDE_PIN_COMMIT, oxide_tree)
    barge_tree = extract_commit_identity(barge_repo, barge_head)[1]
    barge_files = extract_git_tree(barge_repo, barge_source, barge_head, barge_tree)
    if oxide_files.get("scripts/bounded-test.py") != SUPERVISOR_SHA256:
        raise GateRefusal("pinned Oxide archive does not carry the qualified supervisor bytes")
    wrapper_hash = barge_files.get("scripts/tape/private-wrapper.py")
    if not wrapper_hash:
        raise GateRefusal("Barge archive does not carry the private wrapper")
    for name in ("home", "cache", "tmp", "scratch", "bin"):
        private_dir(barge_root, Path(name))
    private_write(barge_root, Path("config.toml"), b"")
    cache_root = private_dir(barge_root, Path("cache"))
    sys.path.insert(0, str(oxide_source / "scripts"))
    try:
        from offline_seed import seed_cargo_home  # type: ignore
        seed_cargo_home(Path(invoking_cargo_home), cache_root,
                        (oxide_source / "Cargo.lock").read_bytes(),
                        deadline_ns=deadline_ns)
    finally:
        sys.path.pop(0)
    allowance.finish("setup")

    # Re-read the deterministic documents and validate them against the
    # accepted launcher rules with live digests.
    allowance.begin("documents")
    ledger_doc = read_json_bound(ledger_path, ledger_digest)
    validate_barge_ledger(ledger_doc, private_root=barge_root, scratch=BARGE_SCRATCH)
    allowance.finish("documents")

    # Aggregate accounting before any child: the caps must fit what remains.
    # Both effective figures are measured clamps, disclosed in the receipt;
    # neither is a claim that a phase fits, and neither is silently raised.
    allowance.begin("account")
    ledger_roots = [Path(p) for p in ledger_doc["roots"]]
    barge_outside = allocated_bytes([p for p in ledger_roots if p != barge_root],
                                    BARGE_CAP_BYTES, deadline_ns)
    barge_monitored = allocated_bytes([barge_root], BARGE_CAP_BYTES,
                                      allowance.bootstrap_deadline_ns())
    barge_effective_mib = clamp_mib(BARGE_CAP_BYTES,
                                    barge_outside + barge_monitored,
                                    BOOTSTRAP_MAX_MIB)
    if barge_effective_mib < 1:
        raise GateRefusal("retained Barge evidence leaves no private scratch budget")
    oxide_caps = oxide_effective_caps(oxide_budget_roots, oxide_root)
    if oxide_caps["effective_whole_block_mib"] < 1:
        raise GateRefusal("retained Oxide evidence leaves no whole-block scratch budget")
    retained_before = retained_snapshot([p for p in ledger_roots + oxide_budget_roots
                                         if p not in (barge_root, oxide_root)], SCRATCH_ROOT,
                                        deadline_ns)
    live_before = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
    allowance.finish("account")

    # The single debug bootstrap: exact argv under the nested qualified
    # supervisor, inside the 300 s window and the measured, clamped budget.
    block_doc = json.loads(block_path.read_bytes())
    allowance.begin_bootstrap()
    bootstrap_mib = barge_effective_mib
    bootstrap_limit = max(1, min(BOOTSTRAP_MAX_SECONDS,
                                 (allowance.bootstrap_deadline_ns() - now_ns()) // 1_000_000_000))
    bootstrap_evidence = private_dir(barge_root, Path("evidence/bootstrap"))
    env = child_environment(dict(os.environ), private=barge_root,
                            oxide=barge_root / "bin/oxide", rustc=rustc_tool,
                            wrapper_hash=wrapper_hash,
                            pool_cap=block_doc["pool_cap_bytes"],
                            admission_env=admission_env)
    os.symlink(rustc_tool.parent / "cargo", barge_root / "bin/cargo")
    os.symlink(effigy, barge_root / "bin/effigy")
    bootstrap_receipt, bootstrap_log, bootstrap_argv = run_supervised(
        supervisor=supervisor_tool, python=python,
        command=list(BARGE_BOOTSTRAP_ARGV), cwd=oxide_source, env=env,
        limit_seconds=bootstrap_limit, scratch_mib=bootstrap_mib,
        evidence=bootstrap_evidence, allowance=allowance, phase="bootstrap")
    validate_supervisor_receipt(bootstrap_receipt, list(BARGE_BOOTSTRAP_ARGV), barge_root)
    artifacts = []
    for line in bootstrap_log.splitlines():
        try:
            item = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue
        target = item.get("target", {})
        if (item.get("reason") == "compiler-artifact" and target.get("name") == "oxide"
                and "bin" in target.get("kind", [])
                and target.get("src_path") == str(oxide_source / "src/bin/oxide/main.rs")
                and item.get("executable")):
            artifacts.append(Path(item["executable"]))
    if len(artifacts) != 1:
        raise GateRefusal("the bootstrap log does not name exactly one oxide artifact")
    staged = artifacts[0].resolve(strict=True)
    if not staged.is_relative_to(barge_root):
        raise GateRefusal("bootstrap artifact escaped the private root")
    oxide_hash = sha256_file(staged)
    os.symlink(staged, barge_root / "bin/oxide")
    allowance.finish_bootstrap()

    # Live isolation bracket: the bootstrap must have changed nothing.
    live_after_bootstrap = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
    verify_inventory(live_before, live_after_bootstrap)

    # The dry gate: the real private status through the same child route.
    gate_limit = max(60, min(300, allowance.remaining_seconds()))
    gate_mib = max(1, min(barge_effective_mib, clamp_mib(
        BARGE_CAP_BYTES, allocated_bytes(ledger_roots, BARGE_CAP_BYTES, deadline_ns), 512)))
    gate_evidence = private_dir(barge_root, Path("evidence/private-status"))
    status_command = [str(effigy), STATUS_SELECTOR]
    state = private_dir(barge_root / "home", Path("Library/Application Support/oxide/state"))
    cache_probe_root = private_dir(barge_root / "home", Path("Library/Caches/oxide"))
    registry_probe = private_write(barge_root, Path(f"registry-probe-{block_doc['block_id']}"),
                                   block_doc["block_id"].encode())
    cache_probe = private_write(cache_probe_root, Path("build-pool"), block_doc["block_id"].encode())
    private_write(state, Path("pools.json"), canonical_json([str(registry_probe)]))
    status_receipt, status_out, status_argv = run_supervised(
        supervisor=supervisor_tool, python=python, command=status_command,
        cwd=barge_source, env=env, limit_seconds=gate_limit, scratch_mib=gate_mib,
        evidence=gate_evidence, allowance=allowance, phase="private-status")
    validate_supervisor_receipt(status_receipt, status_command, barge_root)
    verify_probe(status_out, home=barge_root / "home", config=barge_root / "config.toml",
                 pool=barge_root / "scratch/pool", registry_probe=registry_probe,
                 cache_probe=cache_probe)

    # Final isolation bracket and retained-root quiescence observation.
    final_live = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
    verify_inventory(live_before, final_live)
    retained_after = retained_snapshot([p for p in ledger_roots + oxide_budget_roots
                                         if p not in (barge_root, oxide_root)], SCRATCH_ROOT, deadline_ns)
    if retained_before != retained_after:
        raise GateRefusal("retained roots changed during the gate; quiescence unproven")

    # Full block validation now that the built oxide digest exists.
    max_seconds = validate_barge_block(block_doc, head=barge_head, wrapper_hash=wrapper_hash,
                                       oxide_hash=oxide_hash, audit_hash=audit_digest,
                                       ledger_hash=ledger_digest)

    # Exports: the wrapper-consumable debug receipt and the measured
    # remaining allowance, then the honest gate receipt.
    exports_dir = private_dir(barge_root, Path("exports"))
    live_before_doc = canonical_json(live_before)
    live_after_doc = canonical_json(final_live)
    build_doc = {
        "command": list(BARGE_BOOTSTRAP_ARGV),
        "exit_code": 0,
        "artifact_sha256": oxide_hash,
        "source_tree": oxide_tree,
        "rustc_sha256": sha256_file(rustc_tool),
        "cwd": str(oxide_source),
        "lock_sha256": oxide_files["Cargo.lock"],
        "toolchain_sha256": oxide_files["rust-toolchain.toml"],
        "cargo_sha256": sha256_file(rustc_tool.parent / "cargo"),
        "environment": {key: env[key] for key in BUILD_ENV_KEYS},
        "log_path": str(bootstrap_evidence.parent / "bootstrap-stdout.log"),
        "log_sha256": sha256_file(bootstrap_evidence.parent / "bootstrap-stdout.log"),
        "supervisor_receipt_path": str(bootstrap_evidence / "receipt.json"),
        "supervisor_receipt_sha256": sha256_file(bootstrap_evidence / "receipt.json"),
        "live_before_path": str(private_write(exports_dir, Path("live-before.json"),
                                              live_before_doc)),
        "live_before_sha256": sha256_bytes(live_before_doc),
        "live_after_path": str(private_write(exports_dir, Path("live-after.json"), live_after_doc)),
        "live_after_sha256": sha256_bytes(live_after_doc),
    }
    debug_receipt = build_debug_receipt(binding={
        "commit": OXIDE_PIN_COMMIT, "tree": oxide_tree,
        "lock_sha256": oxide_files["Cargo.lock"],
        "toolchain_sha256": oxide_files["rust-toolchain.toml"],
        "rustc_sha256": sha256_file(rustc_tool),
        "oxide_sha256": oxide_hash,
        "files": oxide_files,
    }, build=build_doc)
    debug_bytes = canonical_json(debug_receipt)
    exports = {
        "debug_receipt": str(private_write(exports_dir, Path("debug-oxide-receipt.json"),
                                           debug_bytes)),
        "debug_receipt_sha256": sha256_bytes(debug_bytes),
        "remaining_allowance": allowance.document(),
        "authorized_barge_max_seconds": max_seconds,
    }
    commands = [
        {"phase": "bootstrap", "argv": bootstrap_argv, "exit": 0},
        {"phase": STATUS_SELECTOR, "argv": status_argv, "exit": 0},
    ]
    decisions = [
        "host admission for the bootstrap is the gate task's own heavy Effigy "
        "admission through the audited hook, with the audited pair preserved in "
        "every child environment; no second admission seam and no archive edit",
        f"the dry status gate runs the accepted Barge archive selector "
        f"{STATUS_SELECTOR} from the extracted barge-source root",
    ]
    receipt_doc = gate_receipt(
        gate_id=run_id,
        blocks={"barge_block_sha256": block_digest, "oxide_block_sha256": approval},
        allowance=allowance.document(),
        bindings={"effigy": sha256_file(effigy), "oxide": oxide_hash,
                  "rustc": sha256_file(rustc_tool), "git": sha256_file(git_tool),
                  "supervisor": SUPERVISOR_SHA256, "wrapper": wrapper_hash},
        commands=commands,
        observations={"live_inventory_sha256": sha256_bytes(live_before_doc),
                      "retained_snapshot_sha256": sha256_bytes(canonical_json(retained_after)),
                      "barge_aggregate_bytes": barge_outside + barge_monitored,
                      "barge_effective_bootstrap_mib": barge_effective_mib,
                      "oxide_retained_outside_bytes": oxide_caps["retained_outside_bytes"],
                      "oxide_effective_whole_block_mib": oxide_caps["effective_whole_block_mib"],
                      "oxide_effective_note": "measured clamp of the 6 GiB grant minus "
                      "retained bytes; about 552 MiB at the reported 5591.8 MiB "
                      "retained total, never a claim that a phase fits",
                      "barge_root_monitored_by": "this gate's allocated-bytes walks, "
                      "outside the supervisor's own scratch; disclosed, not hidden"},
        exports=exports, failures=[], decisions=decisions)
    private_write(allowance_dir, Path("receipt.json"), canonical_json(receipt_doc))
    print("private-build-validation gate completed; receipt "
          f"{allowance_dir / 'receipt.json'}")
    return 0


def extract_commit_identity(repo: Path, commit: str) -> tuple:
    if not isinstance(commit, str) or not HEX40.fullmatch(commit):
        raise GateRefusal(f"source head must be 40 hex: {commit!r}")
    tree = git_output(repo, "rev-parse", commit + "^{tree}").decode("ascii").strip()
    resolved = git_output(repo, "rev-parse", commit + "^{commit}").decode("ascii").strip()
    if resolved != commit or not HEX40.fullmatch(tree):
        raise GateRefusal(f"source commit identity unavailable: {commit}")
    return commit, tree


# ---------------------------------------------------------------------------
# Preparation round (planner-owned; document-only, no execution)
# ---------------------------------------------------------------------------

def prepare(options: dict) -> int:
    """Build the deterministic document packet; no execution, no live reads.

    Writes only under the fresh packet directory and the prepared task roots.
    The blocks it emits are UNISSUED proposals: their identity, authorization
    and digest commitments stay null for the planner to bind.
    """
    packet_dir = Path(options["packet_dir"])
    if not packet_dir.is_absolute() or packet_dir.exists():
        raise GateRefusal("--packet-dir must be a fresh absolute directory")
    barge_root = root_guard(options["barge_root"], BARGE_SCRATCH)
    oxide_root = root_guard(options["oxide_root"], OXIDE_SCRATCH)
    barge_repo = repo_guard(options["barge_repo"])
    oxide_repo = repo_guard(options["oxide_repo"])
    barge_head = options["barge_head"]
    oxide_head = options["oxide_head"]
    # Preparation is a supervised whole command: every walk and marker below
    # is inside the qualified supervisor's coverage and bound to its identity.
    evidence_dir, run_id, deadline_ns, adopted_scratch = supervisor_context()
    os.makedirs(packet_dir, mode=0o700, exist_ok=False)

    barge_roots, barge_evidence = discover_barge_roots([str(barge_root)])
    oxide_roots, oxide_evidence, oxide_unknown = discover_oxide_roots([str(oxide_root)])
    for root in (barge_root, oxide_root):
        os.makedirs(root, mode=0o700, exist_ok=False)
        st = os.lstat(root)
        if st.st_mode & 0o077:
            raise GateRefusal(f"prepared task root is not owner-only: {root}")
    oxide_source = private_dir(barge_root, Path("oxide-source"))
    barge_source = private_dir(barge_root, Path("barge-source"))
    _, oxide_tree = extract_commit_identity(oxide_repo, OXIDE_PIN_COMMIT)
    oxide_files = extract_git_tree(oxide_repo, oxide_source, OXIDE_PIN_COMMIT, oxide_tree)
    barge_tree = extract_commit_identity(barge_repo, barge_head)[1]
    barge_files = extract_git_tree(barge_repo, barge_source, barge_head, barge_tree)
    wrapper_hash = barge_files.get("scripts/tape/private-wrapper.py")
    if not wrapper_hash:
        raise GateRefusal("Barge archive does not carry the private wrapper")

    host = {
        "effigy_sha256": None,
        "rustc_sha256": None,
        "wrapper_sha256": wrapper_hash,
        "supervisor_sha256": SUPERVISOR_SHA256,
        "git_sha256": sha256_file(Path("/usr/bin/git")),
        "oxide_sha256": None,
    }
    documents = {
        "barge-root-ledger.UNISSUED.json": build_barge_ledger(
            roots=barge_roots, evidence=barge_evidence, review_reference=None),
        "oxide-budget.UNISSUED.json": build_oxide_budget(
            roots=oxide_roots, history_reference=None),
        "oxide-history-discovery.json": build_oxide_history(
            roots=oxide_roots, evidence=oxide_evidence, complete=False,
            unknown=oxide_unknown),
        "oxide-block.UNISSUED.json": build_oxide_block_proposal(
            oxide_root=oxide_root, commit=oxide_head, tree=extract_commit_identity(
                oxide_repo, oxide_head)[1],
            binding_sha256=None, budget_sha256=None, history_sha256=None,
            one_use_id=None, quiescent=False),
        "barge-block.UNISSUED.json": build_barge_block_proposal(
            head=barge_head, tree=barge_tree, wrapper_sha256=wrapper_hash,
            audit_sha256=None, ledger_sha256=None, block_id=None,
            authorization_reference=None, pool_cap_bytes=None),
        "source-audit.COMPONENTS.json": {
            "schema": SCHEMA_BARGE_AUDIT,
            "review_reference": None,
            "oxide_commit": OXIDE_PIN_COMMIT,
            "oxide_tree": oxide_tree,
            "host": host,
            "component_sources": {name: [
                [str(relative), lines] for relative, lines in pairs]
                for name, pairs in AUDIT_COMPONENT_SOURCES.items()},
            "note": "components are bound at the diagnostic run from the private "
                    "copies this packet stages; host digests above are null until "
                    "measured, and any drift refuses at execution",
        },
    }
    for name, document in documents.items():
        data = canonical_json(document)
        private_write(packet_dir, Path(name), data)
    private_write(packet_dir, Path("packet-manifest.json"), canonical_json({
        "supervised_run_id": run_id,
        "supervisor_deadline_ns": deadline_ns,
        "clock": "CLOCK_UPTIME_RAW",
        "packet_dir": str(packet_dir),
        "barge_root": str(barge_root), "oxide_root": str(oxide_root),
        "barge_head": barge_head, "oxide_head": oxide_head,
        "documents": {name: sha256_file(packet_dir / name)
                      for name in sorted(documents)},
        "state": "unissued preparation; planner binding and review come first",
    }))
    print(f"prepared packet: {packet_dir}")
    for name in sorted(documents):
        print(f"  {name} sha256={sha256_file(packet_dir / name)}")
    print("the packet carries no issued block; planner binding and review come first")
    return 0


# The audit's component sources, as private-copy paths inside the Barge task
# root. Analyses are bound with the documents at the diagnostic run; each
# names the actual source site that establishes the component's conclusion.
AUDIT_COMPONENT_SOURCES = {
    "oxide_config": ("The pinned Oxide resolver reads OXIDE_CONFIG, the proof "
                     "HOME and the pool settings only from the private paths "
                     "this copy names; no account default is reachable.",
                     [("oxide-source/src/bin/oxide/config.rs", "resolution and proof controls")]),
    "oxide_registry": ("State, pool registration and ledger reads open only "
                       "under the private HOME state root this copy names.",
                       [("oxide-source/src/bin/oxide/store.rs", "state and ledger readers")]),
    "oxide_scratch": ("Scratch evaluation and OXIDE_SCRATCH_ROOT handling "
                      "resolve only inside the private scratch this copy names.",
                      [("oxide-source/src/bin/oxide/scratch_io.rs", "scratch evaluation")]),
    "oxide_child": ("The child CLI dispatch and command construction route "
                    "the leased path through the private environment only.",
                    [("oxide-source/src/bin/oxide/main.rs", "dispatch and child paths")]),
    "effigy_admission": ("The audited Effigy admission plan reads the hook and "
                         "socket preserved by this gate's environment; the "
                         "direct executable is live-hashed in the same audit.",
                         [("effigy-source/admission.rs", "hook plan and decide")]),
    "rustup_cargo": ("Cargo and rustup binaries are digest-bound here and the "
                     "child environment pins RUSTUP_HOME, the toolchain and "
                     "auto-install off, so no host cache is reachable.",
                     [("toolchain/cargo", "digest-bound binary"),
                      ("toolchain/rustc", "digest-bound binary")]),
    "compiler_linker": ("The pinned compiler and linker invocations receive "
                        "only the scrubbed environment and write only into "
                        "the private target and cache roots.",
                        [("toolchain/rustc", "digest-bound binary")]),
    "build_scripts": ("Every locked build script runs offline from the seeded "
                      "private cache this packet binds; the lock and cache "
                      "manifest are digest-bound with the sources.",
                      [("oxide-source/Cargo.lock", "entire lock"),
                       ("oxide-source/scripts/offline_seed.py", "bounded seeder")]),
    "git_source": ("Source archives are extracted read-only from the exact "
                   "commit and tree this audit binds; no host repository is "
                   "written and no host Git config is read.",
                   [("barge-source/scripts/tape/admission.py", "archive verification")]),
    "retained_roots": ("Every retained task root is enumerated in the "
                       "companion ledger with its discovery evidence and "
                       "review reference; unknowns stay explicit.",
                       [("barge-source/scripts/tape/private-wrapper.py",
                         "ledger consumption")]),
}


# ---------------------------------------------------------------------------
# Plan (effect-free)
# ---------------------------------------------------------------------------

REFUSAL_CASES = (
    "missing, unissued or duplicated block identity",
    "exhausted wall or disk budget",
    "cross-process id/deadline mismatch",
    "wrong source, digest or child environment",
    "absent admission hook, socket or runtime binding",
    "unknown, incomplete or non-quiescent retained roots",
    "empty or malformed live inventory",
    "same-content inode replacement",
    "failed or uncertain child closure",
)


def plan(options: dict) -> int:
    print("shared private-build validation gate (oxide#248): plan")
    print(f"  supervisor bytes pinned: {SUPERVISOR_SHA256}")
    print(f"  oxide pin: {OXIDE_PIN_COMMIT} tree {OXIDE_PIN_TREE}")
    print(f"  barge accepted admission source: {BARGE_ACCEPTED_HEAD}")
    print(f"  barge recipe head: {BARGE_RECIPE_HEAD} tree {BARGE_RECIPE_TREE}")
    print(f"  caps: barge {BARGE_CAP_BYTES} bytes, oxide {OXIDE_CAP_BYTES} bytes, "
          f"bootstrap {BOOTSTRAP_MAX_SECONDS}s/{BOOTSTRAP_MAX_MIB}MiB, "
          f"prospective block {PROSPECTIVE_MAX_SECONDS}s")
    print(f"  schemas: {SCHEMA_GATE_RECEIPT}, {SCHEMA_BARGE_AUDIT}, "
          f"{SCHEMA_BARGE_LEDGER}, {SCHEMA_BARGE_BLOCK}")
    print("  refusal controls exercised by --self-test:")
    for line in REFUSAL_CASES:
        print("   -", line)
    print("  proposed diagnostic command (planner binds every placeholder):")
    print("    python3 scripts/bounded-test.py --limit 1200 --scratch-budget-mib 512 -- "
          "python3 scripts/test-private-build-validation.py --diagnostic")
    print("  proposed selectors: proof:private-build-validation:test (pure), "
          "proof:private-build-validation:plan (effect-free), "
          "proof:private-build-validation:diagnostic (heavy)")
    return 0


# ---------------------------------------------------------------------------
# Self-test: mocked counterfactuals only; effect-free outside its fixture
# ---------------------------------------------------------------------------

class Fixture:
    """A disposable fixture tree for the pure checks."""

    def __init__(self):
        base = os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or tempfile.gettempdir()
        self.root = Path(tempfile.mkdtemp(prefix="private-build-validation-test.",
                                          dir=base)).resolve(strict=True)
        os.chmod(self.root, 0o700)

    def close(self):
        shutil.rmtree(self.root, ignore_errors=True)


def write_doc(directory: Path, name: str, value) -> Path:
    data = canonical_json(value)
    path = directory / name
    path.write_bytes(data)
    return path


def self_test() -> int:
    fixture = Fixture()
    failures: list = []
    scratch = fixture.root / "barge-scratch"
    oxide_scratch = fixture.root / "oxide-scratch"
    scratch.mkdir()
    oxide_scratch.mkdir()
    private_root = scratch / "gate.root"
    private_root.mkdir()
    os.chmod(private_root, 0o700)

    def expect_refusal(label, function, *args, **kwargs):
        try:
            function(*args, **kwargs)
        except GateRefusal as refusal:
            if not str(refusal).strip():
                failures.append(f"{label}: refusal without a bounded reason")
            return
        except Exception as error:  # noqa: BLE001 - the harness reports the class
            failures.append(f"{label}: wrong exception {type(error).__name__}: {error}")
            return
        failures.append(f"{label}: expected a refusal, got success")

    def expect_ok(label, function, *args, **kwargs):
        try:
            return function(*args, **kwargs)
        except GateRefusal as refusal:
            failures.append(f"{label}: unexpected refusal: {refusal}")
        except Exception as error:  # noqa: BLE001
            failures.append(f"{label}: unexpected {type(error).__name__}: {error}")
        return None

    # --- primitives ---------------------------------------------------------
    sample = private_write(private_root, Path("docs/sample.txt"), b"sample bytes\n")
    if sha256_file(sample) != hashlib.sha256(b"sample bytes\n").hexdigest():
        failures.append("sha256_file digest mismatch")
    os.symlink(sample, private_root / "docs/sample-link")
    expect_refusal("digest refuses a symlink", sha256_file, private_root / "docs/sample-link")
    expect_refusal("private_dir escapes root", private_dir, private_root, Path("../escape"))
    expect_refusal("private_dir absolute", private_dir, private_root, Path("/etc"))
    expect_refusal("private_write over existing file", private_write, private_root,
                   Path("docs/sample.txt"), b"again")
    foreign = private_root / "docs/foreign"
    foreign.write_bytes(b"x")
    os.chmod(foreign, 0o644)
    expect_refusal("foreign-mode private dir component", private_dir, private_root,
                   Path("docs/foreign/inner"))

    doc = {"b": 1, "a": [1, 2]}
    doc_path = write_doc(private_root, Path("docs/doc.json"), doc)
    expect_ok("read_json_bound round trip", read_json_bound, doc_path, sha256_file(doc_path))
    expect_refusal("read_json_bound digest mismatch", read_json_bound, doc_path, "0" * 64)
    expect_refusal("read_json_bound non-hex digest", read_json_bound, doc_path, "zz")
    other = private_root / "docs/doc2.json"
    other.write_bytes(canonical_json([1, 2]))
    expect_refusal("read_json_bound non-object", read_json_bound, other, sha256_file(other))

    # --- block validation: unissued/duplicate and wrong bindings ------------
    host = {"wrapper_sha256": "3" * 64}
    base_block = build_barge_block_proposal(
        head=BARGE_RECIPE_HEAD, tree=BARGE_RECIPE_TREE, wrapper_sha256=host["wrapper_sha256"],
        audit_sha256="a" * 64, ledger_sha256="b" * 64, block_id="gate-block-1",
        authorization_reference="operator ruling 578e728a v2", pool_cap_bytes=268435456,
        selectors=[STATUS_SELECTOR, "rust:format"])
    issued_block = dict(base_block, oxide_sha256="2" * 64)
    full_host = dict(host, effigy_sha256="1" * 64, oxide_hash="2" * 64,
                     audit_hash="a" * 64, ledger_hash="b" * 64)
    expect_ok("valid block", validate_barge_block, issued_block, head=BARGE_RECIPE_HEAD,
              wrapper_hash=host["wrapper_sha256"], oxide_hash=full_host["oxide_hash"],
              audit_hash=full_host["audit_hash"], ledger_hash=full_host["ledger_hash"])
    unissued = build_barge_block_proposal(head=BARGE_RECIPE_HEAD, tree=BARGE_RECIPE_TREE,
                                          wrapper_sha256=host["wrapper_sha256"],
                                          audit_sha256=None, ledger_sha256=None,
                                          block_id=None, authorization_reference=None)
    expect_refusal("unissued block (null identity)", validate_barge_block, unissued,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    missing = {k: v for k, v in issued_block.items() if k != "ledger_sha256"}
    expect_refusal("missing block field", validate_barge_block, missing,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    extra = dict(issued_block, extra_field=1)
    expect_refusal("extra block field", validate_barge_block, extra,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    wrong_head = dict(issued_block, head="0" * 40)
    expect_refusal("wrong head", validate_barge_block, wrong_head, head=BARGE_RECIPE_HEAD,
                   wrapper_hash=host["wrapper_sha256"], oxide_hash=full_host["oxide_hash"],
                   audit_hash=full_host["audit_hash"], ledger_hash=full_host["ledger_hash"])
    wrong_digest = dict(issued_block, audit_sha256="c" * 64)
    expect_refusal("wrong audit digest", validate_barge_block, wrong_digest,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    exhausted = dict(issued_block, max_seconds=59)
    expect_refusal("block below the minimum bound", validate_barge_block, exhausted,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    over_max = dict(issued_block, max_seconds=1800)
    expect_refusal("block above the authorized maximum", validate_barge_block, over_max,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    bad_cap = dict(issued_block, scratch_cap_bytes=1024)
    expect_refusal("altered scratch cap", validate_barge_block, bad_cap,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    no_pool = dict(issued_block, pool_cap_bytes=None)
    expect_refusal("missing pool cap", validate_barge_block, no_pool,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    big_pool = dict(issued_block, pool_cap_bytes=BARGE_CAP_BYTES + 1)
    expect_refusal("pool cap above scratch", validate_barge_block, big_pool,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    wrong_oxide = dict(issued_block, oxide_sha256="0" * 64)
    expect_refusal("staged oxide digest mismatch", validate_barge_block, wrong_oxide,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    dup_selectors = dict(issued_block, selectors=[STATUS_SELECTOR, "rust:format", "rust:format"])
    expect_refusal("duplicate selector", validate_barge_block, dup_selectors,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    unsafe_selector = dict(issued_block, selectors=[STATUS_SELECTOR, "rust:test"])
    expect_refusal("selector outside the safe route", validate_barge_block, unsafe_selector,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    stage_export = dict(issued_block, selectors=[STATUS_SELECTOR, "rust:test:tape-stage",
                                                 "rust:test:tape-export"])
    expect_refusal("stage+export selector conflict", validate_barge_block, stage_export,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    first_wrong = dict(issued_block, selectors=["rust:format"])
    expect_refusal("first selector is not the status gate", validate_barge_block, first_wrong,
                   head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])

    # --- audit validation ---------------------------------------------------
    source_a = private_write(private_root, Path("oxide-source/src/bin/oxide/config.rs"),
                             b"pub fn facts() {}\n")
    sources = [{"path": str(source_a), "sha256": sha256_file(source_a), "lines": "1-1"}]
    analysis = ("the resolver reads only the OXIDE_CONFIG path inside the private "
                "root, proven from this exact copy")
    good_audit = {
        "schema": SCHEMA_BARGE_AUDIT,
        "review_reference": "retained review 881c1ef9",
        "oxide_commit": OXIDE_PIN_COMMIT, "oxide_tree": OXIDE_PIN_TREE,
        "admission_hook": "hook-command", "admission_socket": "/sockets/admission.sock",
        "admission_executables": [{"path": str(sample), "sha256": sha256_file(sample)}],
        "admission_runtime_files": [{"path": str(sample), "sha256": sha256_file(sample)}],
        "child_path": "/bin:/usr/bin", "cargo_sha256": "7" * 64, "python_sha256": "8" * 64,
        "components": {name: audit_component([dict(sources[0])], analysis)
                       for name in BARGE_AUDIT_COMPONENTS},
    }
    expect_ok("valid audit", validate_barge_audit, good_audit, private=private_root, host={})
    missing_component = dict(good_audit,
                             components={k: v for k, v in good_audit["components"].items()
                                         if k != "retained_roots"})
    expect_refusal("audit with a missing component", validate_barge_audit, missing_component,
                   private=private_root, host={})
    bad_conclusion = json.loads(canonical_json(good_audit))
    bad_conclusion["components"]["retained_roots"]["conclusion"] = "assumed"
    expect_refusal("audit conclusion not proven", validate_barge_audit, bad_conclusion,
                   private=private_root, host={})
    short_analysis = json.loads(canonical_json(good_audit))
    short_analysis["components"]["retained_roots"]["analysis"] = "trust me"
    expect_refusal("audit analysis too short", validate_barge_audit, short_analysis,
                   private=private_root, host={})
    no_review = dict(good_audit, review_reference="")
    expect_refusal("audit without review reference", validate_barge_audit, no_review,
                   private=private_root, host={})
    wrong_pin = dict(good_audit, oxide_commit="0" * 40)
    expect_refusal("audit with wrong oxide pin", validate_barge_audit, wrong_pin,
                   private=private_root, host={})
    host_mismatch = json.loads(canonical_json(good_audit))
    expect_refusal("audit host digest mismatch", validate_barge_audit, host_mismatch,
                   private=private_root, host={"effigy_sha256": "1" * 64})
    drifted = json.loads(canonical_json(good_audit))
    drifted["components"]["retained_roots"]["sources"][0]["sha256"] = "9" * 64
    expect_refusal("audit source digest drift", validate_barge_audit, drifted,
                   private=private_root, host={})
    escaping = json.loads(canonical_json(good_audit))
    escaping["components"]["retained_roots"]["sources"][0]["path"] = "/etc/hosts"
    expect_refusal("audit source outside the private root", validate_barge_audit, escaping,
                   private=private_root, host={})

    # --- admission binding: absent hook/socket/runtime ----------------------
    inherited = {"EFFIGY_ADMISSION_HOOK": "hook-command",
                 "QUEUE_ADMISSION_SOCKET": "/sockets/admission.sock"}
    expect_ok("audited admission present", audited_admission, inherited, good_audit)
    expect_refusal("admission hook unset", audited_admission,
                   {"QUEUE_ADMISSION_SOCKET": "/sockets/admission.sock"}, good_audit)
    expect_refusal("admission hook mismatch", audited_admission,
                   {"EFFIGY_ADMISSION_HOOK": "other-hook",
                    "QUEUE_ADMISSION_SOCKET": "/sockets/admission.sock"}, good_audit)
    expect_refusal("admission socket relative", audited_admission,
                   {"EFFIGY_ADMISSION_HOOK": "hook-command",
                    "QUEUE_ADMISSION_SOCKET": "sockets/admission.sock"}, good_audit)
    changed_runtime = json.loads(canonical_json(good_audit))
    changed_runtime["admission_runtime_files"][0]["sha256"] = "a" * 64
    expect_refusal("runtime file binding drift", audited_admission, inherited, changed_runtime)

    # --- direct effigy binding ----------------------------------------------
    binary = fixture.root / "effigy-binary"
    binary.write_bytes(b"\xcf\xfa\xed\xfe-mach-o")
    os.chmod(binary, 0o755)
    expect_ok("direct effigy binary accepted", direct_effigy, binary)
    shim = fixture.root / "effigy-shim"
    shim.write_bytes(b"#!/bin/sh\nexec real-effigy\n")
    os.chmod(shim, 0o755)
    expect_refusal("shebang shim refused", direct_effigy, shim)
    not_exec = fixture.root / "effigy-quiet"
    not_exec.write_bytes(b"\xcf\xfa\xed\xfe-mach-o")
    os.chmod(not_exec, 0o644)
    expect_refusal("non-executable refused", direct_effigy, not_exec)

    # --- child environment: wrong scrub / missing toolchain -----------------
    good_inherited = {"RUSTUP_HOME": "/rustup", "RUSTUP_TOOLCHAIN": TOOLCHAIN,
                      "PATH": "/usr/bin:/bin",
                      "EFFIGY_ADMISSION_HOOK": "hook-command",
                      "QUEUE_ADMISSION_SOCKET": "/sockets/admission.sock",
                      "OXIDE_POOL": "/host-pool", "XDG_CONFIG_HOME": "/host-xdg",
                      "CARGO_HOME": "/host-cargo", "SCCACHE_CACHE": "/host-sccache"}
    admission_pair = {"EFFIGY_ADMISSION_HOOK": "hook-command",
                      "QUEUE_ADMISSION_SOCKET": "/sockets/admission.sock"}
    env = expect_ok("child environment built", child_environment, good_inherited,
                    private=private_root, oxide=private_root / "bin/oxide",
                    rustc=Path("/rustup/toolchains/1.97.1-a/bin/rustc"),
                    wrapper_hash=host["wrapper_sha256"], pool_cap=268435456,
                    admission_env=admission_pair)
    if env is not None:
        if env.get("OXIDE_POOL") != str(private_root / "scratch/pool"):
            failures.append("child environment kept a host OXIDE_POOL")
        if "XDG_CONFIG_HOME" in env or "SCCACHE_CACHE" in env or "CARGO_HOME" not in env:
            failures.append("child environment scrub failed")
        if env.get("OXIDE_TEST_HOME_FROM_ENV") != "1":
            failures.append("child environment lost the test-home control")
        if env.get("CARGO_NET_OFFLINE") != "true" or env.get("RUSTUP_AUTO_INSTALL") != "0":
            failures.append("child environment lost its offline and no-install controls")
        if not env["PATH"].startswith(str(private_root / "bin")):
            failures.append("child PATH does not prefer the private bin")
        if env.get("HOME") != str(private_root / "home"):
            failures.append("child HOME is not the private home")
    bad_toolchain = dict(good_inherited, RUSTUP_TOOLCHAIN="1.96.0")
    expect_refusal("child environment with wrong toolchain", child_environment, bad_toolchain,
                   private=private_root, oxide=private_root / "bin/oxide",
                   rustc=Path("/rustup/bin/rustc"), wrapper_hash=host["wrapper_sha256"],
                   pool_cap=268435456, admission_env=admission_pair)
    no_rustup = {k: v for k, v in good_inherited.items() if k != "RUSTUP_HOME"}
    expect_refusal("child environment without rustup home", child_environment, no_rustup,
                   private=private_root, oxide=private_root / "bin/oxide",
                   rustc=Path("/rustup/bin/rustc"), wrapper_hash=host["wrapper_sha256"],
                   pool_cap=268435456, admission_env=admission_pair)
    expect_refusal("child environment with a partial admission pair", child_environment,
                   good_inherited,
                   private=private_root, oxide=private_root / "bin/oxide",
                   rustc=Path("/rustup/bin/rustc"), wrapper_hash=host["wrapper_sha256"],
                   pool_cap=268435456,
                   admission_env={"EFFIGY_ADMISSION_HOOK": "hook-command"})

    # --- ledger validation: unknown/incomplete/non-quiescent roots ----------
    historic_root = scratch / "041-budget-ledger-20261102"
    (scratch / "b041-final.n0hH3Cez").mkdir()
    historic_root.mkdir()
    log_dir = scratch / "run.G9v5IH"
    log_dir.mkdir()
    known_log = private_write(log_dir, Path("rust-test.log"), b"inner log\n")
    historic = {"taskId": BARGE_TASK,
                "known_inner_runtime_logs": [{"path": str(known_log),
                                              "reported_inner_seconds": [1.0]}]}
    historic_path = private_write(historic_root, Path("ledger.json"),
                                  canonical_json(historic))
    evidence = [(historic_path, sha256_file(historic_path))]
    fixture_required = [scratch / "b041-final.n0hH3Cez", historic_root]
    good_roots = [scratch / "b041-final.n0hH3Cez", historic_root,
                  private_root, log_dir]
    ledger = build_barge_ledger(roots=good_roots, evidence=evidence,
                                review_reference="discovery review r1")
    expect_ok("ledger accepted", validate_barge_ledger, ledger, private_root=private_root,
              scratch=scratch, historic=historic_path, required_roots=fixture_required)
    expect_refusal("ledger without its review reference", validate_barge_ledger,
                   dict(ledger, review_reference=None), private_root=private_root,
                   scratch=scratch, historic=historic_path, required_roots=fixture_required)
    omitted = dict(ledger, roots=[str(p) for p in good_roots if p != log_dir])
    expect_refusal("ledger omitting known evidence", validate_barge_ledger, omitted,
                   private_root=private_root, scratch=scratch, historic=historic_path,
                   required_roots=fixture_required)
    no_new_root = dict(ledger, roots=[str(p) for p in good_roots if p != private_root])
    expect_refusal("ledger missing the new root", validate_barge_ledger, no_new_root,
                   private_root=private_root, scratch=scratch, historic=historic_path,
                   required_roots=fixture_required)
    overlap = dict(ledger, roots=[str(p) for p in good_roots] + [str(log_dir / "deeper")])
    expect_refusal("ledger with overlapping roots", validate_barge_ledger, overlap,
                   private_root=private_root, scratch=scratch, historic=historic_path,
                   required_roots=fixture_required)
    escaped = dict(ledger, roots=[str(p) for p in good_roots] + [str(fixture.root / "elsewhere")])
    expect_refusal("ledger root outside scratch", validate_barge_ledger, escaped,
                   private_root=private_root, scratch=scratch, historic=historic_path,
                   required_roots=fixture_required)
    drifted_evidence = dict(ledger, discovery_evidence=[
        (historic_path, "0" * 64)])
    expect_refusal("ledger evidence digest drift", validate_barge_ledger, drifted_evidence,
                   private_root=private_root, scratch=scratch, historic=historic_path,
                   required_roots=fixture_required)
    wrong_task_historic = private_write(historic_root, Path("other.json"),
                                        canonical_json(dict(historic, taskId="other")))
    expect_refusal("historic ledger for another task", validate_barge_ledger, ledger,
                   private_root=private_root, scratch=scratch,
                   historic=wrong_task_historic, required_roots=fixture_required)
    pr_root = scratch / "041-pr.abcd"
    pr_root.mkdir()
    glob_roots = [str(p) for p in good_roots] + [str(scratch / "041-pr.wxyz")]
    (scratch / "041-pr.wxyz").mkdir()
    expect_refusal("ledger omitting a 041-pr.* root", validate_barge_ledger,
                   dict(ledger, roots=glob_roots),
                   private_root=private_root, scratch=scratch, historic=historic_path,
                   required_roots=fixture_required)
    expect_ok("ledger covering the glob root", validate_barge_ledger,
              dict(ledger, roots=glob_roots + [str(pr_root)]),
              private_root=private_root, scratch=scratch, historic=historic_path,
              required_roots=fixture_required)

    # --- oxide side: budget/history/block -----------------------------------
    oxide_run_root = oxide_scratch / "gate.oxide"
    oxide_run_root.mkdir()
    os.chmod(oxide_run_root, 0o700)
    others = [oxide_scratch / f"r{i}" for i in range(1, 5)]
    for path in others:
        path.mkdir()
    budget = build_oxide_budget(roots=[oxide_run_root, *others],
                                history_reference="queue records 20261102")
    expect_ok("oxide budget valid", validate_oxide_budget, budget,
              run_root=oxide_run_root, scratch=oxide_scratch)
    few = dict(budget, retained_roots=budget["retained_roots"][:3])
    expect_refusal("oxide budget with too few roots", validate_oxide_budget, few,
                   run_root=oxide_run_root, scratch=oxide_scratch)
    measured = dict(budget, covering_seconds_charged=1180)
    expect_refusal("oxide budget charging measured seconds", validate_oxide_budget, measured,
                   run_root=oxide_run_root, scratch=oxide_scratch)
    missing_root = dict(budget, retained_roots=[str(p) for p in others])
    expect_refusal("oxide budget without the run root", validate_oxide_budget, missing_root,
                   run_root=oxide_run_root, scratch=oxide_scratch)
    overlap = dict(budget, retained_roots=budget["retained_roots"]
                   + [str(oxide_scratch / "r1/sub")])
    expect_refusal("oxide budget with overlapping roots", validate_oxide_budget, overlap,
                   run_root=oxide_run_root, scratch=oxide_scratch)
    no_history_ref = dict(budget, history_reference="")
    expect_refusal("oxide budget without history reference", validate_oxide_budget,
                   no_history_ref, run_root=oxide_run_root, scratch=oxide_scratch)
    history = build_oxide_history(roots=budget["retained_roots"],
                                  evidence={"snapshot.json": "c" * 64}, complete=True,
                                  unknown=["shared sizes unknown"])
    expect_ok("oxide history valid", validate_oxide_history, history, budget)
    incomplete = dict(history, complete=False)
    expect_refusal("incomplete oxide history", validate_oxide_history, incomplete, budget)
    mismatched = dict(history, retained_roots=budget["retained_roots"][:-1])
    expect_refusal("oxide history roots disagree", validate_oxide_history, mismatched, budget)
    bad_evidence = dict(history, evidence={"snapshot.json": "nothex"})
    expect_refusal("oxide history malformed evidence", validate_oxide_history, bad_evidence,
                   budget)
    binding_sha = "d" * 64
    block_oxide = build_oxide_block_proposal(
        oxide_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
        binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64,
        one_use_id="3" * 64, quiescent=True)
    expect_ok("oxide block valid", validate_oxide_block, block_oxide,
              run_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
              binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64)
    expect_refusal("oxide block without quiescence admission", validate_oxide_block,
                   dict(block_oxide, retained_roots_quiescent=False),
                   run_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
                   binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64)
    expect_refusal("oxide block bound to another root", validate_oxide_block,
                   dict(block_oxide, root=str(oxide_scratch / "other")),
                   run_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
                   binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64)
    expect_refusal("oxide block with a mismatched source commit", validate_oxide_block,
                   dict(block_oxide, source_commit="0" * 40),
                   run_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
                   binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64)
    expect_refusal("oxide block with an unconfirmed planner reference", validate_oxide_block,
                   block_oxide, run_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
                   binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64,
                   planner_reference="brief-v12:planner-binds-at-issue")
    unissued_oxide = build_oxide_block_proposal(
        oxide_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
        binding_sha256=None, budget_sha256=None, history_sha256=None,
        one_use_id=None, quiescent=False)
    expect_refusal("unissued oxide block", validate_oxide_block, unissued_oxide,
                   run_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
                   binding_sha256=binding_sha, budget_sha256="1" * 64, history_sha256="2" * 64)

    # --- allowance: exhaustion, reset and replay refusals -------------------
    allowance = Allowance(1200, None, None)
    allowance.begin("setup")
    allowance.finish("setup")
    expect_ok("allowance tracks spend", lambda: allowance.remaining_seconds() <= 1200)
    expect_ok("first bootstrap window opens", allowance.begin_bootstrap)
    allowance.bootstrap_deadline_ns()
    expect_refusal("second bootstrap refused", allowance.begin_bootstrap)
    expect_ok("bootstrap window closes once", allowance.finish_bootstrap)
    expect_refusal("double bootstrap finish refused", allowance.finish_bootstrap)
    tight = Allowance(60, now_ns() + 1_000_000_000, None)
    expect_refusal("exhausted deadline refuses a phase", tight.begin, "setup")
    state_dir = fixture.root / "state"
    state_dir.mkdir()
    counted = Allowance(1200, None, state_dir)
    counted.begin("bind")
    counted.finish("bind")
    expect_refusal("existing allowance records refuse a second claim",
                   Allowance, 1200, None, state_dir)
    no_begin = Allowance(1200, None, None)
    expect_refusal("finish without begin refuses", no_begin.finish, "bind")

    # --- live inventory: empty, malformed, same-content inode replacement ---
    live_root = fixture.root / "live-root"
    (live_root / "state").mkdir(parents=True)
    (live_root / "state/pools.json").write_bytes(b"[]")
    rows = expect_ok("live inventory of a small tree", live_inventory, live_root)
    if rows is not None and not any(row[0] == "./state/pools.json" for row in rows):
        failures.append("live inventory missed a regular file")
    empty_root = fixture.root / "empty-live"
    empty_root.mkdir()
    expect_refusal("empty live inventory", live_inventory, empty_root)
    expect_refusal("missing live root", live_inventory, fixture.root / "absent-live")
    special = live_root / "state/fifo"
    os.mkfifo(special)
    expect_refusal("unsupported live entry", live_inventory, live_root)
    special.unlink()
    before = expect_ok("live inventory again", live_inventory, live_root)
    same_bytes_inode = live_root / "state/pools.json"
    st_before = os.lstat(same_bytes_inode)
    data = same_bytes_inode.read_bytes()
    os.remove(same_bytes_inode)
    same_bytes_inode.write_bytes(data)
    st_after = os.lstat(same_bytes_inode)
    if st_before.st_ino == st_after.st_ino:
        failures.append("fixture failed to replace the inode; counterfactual not exercised")
    after = expect_ok("live inventory after inode replacement", live_inventory, live_root)
    if before is not None and after is not None and before == after:
        failures.append("same-content inode replacement passed as equality")
    expect_ok("inventory equality on an unchanged tree", verify_inventory, before, before)
    expect_refusal("inventory inequality refuses", verify_inventory, before, after)
    retained_before = expect_ok("retained snapshot", retained_snapshot, [private_root], scratch)
    os.rename(private_root / "docs/sample.txt", private_root / "docs/renamed.txt")
    retained_after = expect_ok("retained snapshot after change", retained_snapshot,
                               [private_root], scratch)
    if retained_before is not None and retained_after is not None \
            and retained_before == retained_after:
        failures.append("retained snapshot missed a rename")
    os.rename(private_root / "docs/renamed.txt", private_root / "docs/sample.txt")

    # --- supervisor receipt: failed/uncertain closure -----------------------
    good_receipt = {"command": ["status"], "scratch": str(private_root),
                    "outcome": {"closure": "closed", "status": 0, "fault": None,
                                "deadline": False, "launched": True},
                    "census": {"certain": True, "members": 0, "uncertain": 0}}
    expect_ok("supervisor receipt accepted", validate_supervisor_receipt, good_receipt,
              ["status"], private_root)
    for mutation, label in (
            ({"outcome": {"closure": "unclosed", "status": 0, "fault": None,
                          "deadline": False, "launched": True}}, "unclosed outcome"),
            ({"outcome": {"closure": "closed", "status": 1, "fault": None,
                          "deadline": False, "launched": True}}, "nonzero status"),
            ({"outcome": {"closure": "closed", "status": 0, "fault": "x",
                          "deadline": False, "launched": True}}, "latched fault"),
            ({"outcome": {"closure": "closed", "status": 0, "fault": None,
                          "deadline": True, "launched": True}}, "deadline reached"),
            ({"outcome": {"closure": "closed", "status": 0, "fault": None,
                          "deadline": False, "launched": False}}, "not launched"),
            ({"census": {"certain": False, "members": 0, "uncertain": 0}}, "uncertain census"),
            ({"census": {"certain": True, "members": 1, "uncertain": 0}}, "members left"),
            ({"census": {"certain": True, "members": 0, "uncertain": 1}}, "uncertain member"),
            ({"command": ["other"]}, "command mismatch"),
            ({"scratch": str(private_root / "elsewhere")}, "scratch mismatch")):
        mutated = json.loads(canonical_json(good_receipt))
        mutated.update(mutation)
        expect_refusal(f"supervisor receipt with {label}", validate_supervisor_receipt,
                       mutated, ["status"], private_root)

    # --- probe: sentinel output ---------------------------------------------
    probe_out = "\n".join([
        f"config: {private_root / 'config.toml'} (selected by OXIDE_CONFIG)",
        f"  home: HOME {private_root / 'home'} (proof control OXIDE_TEST_HOME_FROM_ENV)",
        f"pool: {private_root / 'scratch/pool'} (directory-budget, 256 MiB cap)",
        f"other pool path: {private_root / 'registry-probe'} (unrecognised; left alone)",
        f"other pool path: {private_root / 'cache-probe'} (unrecognised; left alone)",
    ])
    probe_kwargs = dict(home=private_root / "home", config=private_root / "config.toml",
                        pool=private_root / "scratch/pool",
                        registry_probe=private_root / "registry-probe",
                        cache_probe=private_root / "cache-probe")
    expect_ok("probe output accepted", verify_probe, probe_out, **probe_kwargs)
    expect_refusal("probe with unavailable pool", verify_probe,
                   probe_out.replace("(directory-budget, 256 MiB cap)", "(unavailable)"),
                   **probe_kwargs)
    expect_refusal("probe missing sentinel", verify_probe,
                   probe_out.replace(f"other pool path: {private_root / 'cache-probe'} "
                                     "(unrecognised; left alone)", ""), **probe_kwargs)
    expect_refusal("probe with a status refusal line", verify_probe,
                   probe_out + "\ncannot open: something", **probe_kwargs)

    # --- argument and context guards ----------------------------------------
    expect_refusal("unknown mode", parse_args, ["--exec"])
    expect_refusal("diagnostic without expectations", parse_args,
                   ["--diagnostic", "--oxide-repo", "/r", "--oxide-head", "a" * 40,
                    "--barge-repo", "/b", "--barge-head", "b" * 40,
                    "--barge-root", str(BARGE_SCRATCH / "x"),
                    "--oxide-root", str(OXIDE_SCRATCH / "x")])
    expect_refusal("non-hex expectation", parse_args,
                   ["--diagnostic", "--expect-audit", "nothex", "--oxide-repo", "/r",
                    "--oxide-head", "a" * 40, "--barge-repo", "/b",
                    "--barge-head", "b" * 40, "--barge-root", str(BARGE_SCRATCH / "x"),
                    "--oxide-root", str(OXIDE_SCRATCH / "x")])
    expect_refusal("non-hex head", parse_args,
                   ["--prepare", "--packet-dir", "/tmp/p", "--oxide-repo", "/r",
                    "--oxide-head", "short", "--barge-repo", "/b", "--barge-head",
                    "b" * 40, "--barge-root", str(BARGE_SCRATCH / "x"),
                    "--oxide-root", str(OXIDE_SCRATCH / "x")])
    expect_refusal("task root outside its scratch", root_guard, "/tmp/x", BARGE_SCRATCH)
    expect_refusal("task root equal to scratch", root_guard, str(BARGE_SCRATCH), BARGE_SCRATCH)

    # --- clamp arithmetic (review 6584174560 finding 2) ---------------------
    expect_ok("552 MiB clamp at the reported retained total", clamp_mib,
              OXIDE_CAP_BYTES, 5591 * 1024 * 1024 + 838_860, BOOTSTRAP_MAX_MIB)
    if clamp_mib(OXIDE_CAP_BYTES, 5591 * 1024 * 1024 + 838_860, BOOTSTRAP_MAX_MIB) > 552:
        failures.append("clamp exceeded the effective whole-block figure")
    expect_refusal("clamp with retained at the full grant", clamp_mib,
                   OXIDE_CAP_BYTES, OXIDE_CAP_BYTES, BOOTSTRAP_MAX_MIB)
    expect_refusal("clamp over the grant", clamp_mib,
                   OXIDE_CAP_BYTES, OXIDE_CAP_BYTES + 1, BOOTSTRAP_MAX_MIB)
    expect_refusal("clamp with a non-integer figure", clamp_mib,
                   OXIDE_CAP_BYTES, 1024 * 1024, "1024")
    if clamp_mib(4 * 1024 * 1024, 1024 * 1024, 1) != 1:
        failures.append("clamp did not apply the per-run cap")
    if clamp_mib(4 * 1024 * 1024, 1024 * 1024, 16) != 3:
        failures.append("clamp exceeded the real remainder")

    # --- real-tree accounting controls (review finding 9) -------------------
    tree = fixture.root / "real-tree"
    (tree / "deep").mkdir(parents=True)
    (tree / "deep/data.bin").write_bytes(b"d" * 4096)
    os.link(tree / "deep/data.bin", tree / "hardlink.bin")
    os.symlink("deep/data.bin", tree / "link.bin")
    real_bytes = expect_ok("allocated bytes on a real tree", allocated_bytes,
                           [tree], OXIDE_CAP_BYTES)
    single = expect_ok("allocated bytes of one inode", allocated_bytes,
                       [tree / "deep/data.bin"], OXIDE_CAP_BYTES)
    if real_bytes is None or single is None or real_bytes != single:
        failures.append("hard links were not counted once")
    foreign_tree = fixture.root / "foreign-tree"
    foreign_tree.mkdir()
    os.chmod(foreign_tree, 0o700)
    # An unsupported entry inside a retained root refuses without a verdict.
    os.mkfifo(foreign_tree / "pipe")
    expect_refusal("unsupported retained entry", retained_snapshot,
                   [foreign_tree], fixture.root)
    foreign_tree.joinpath("pipe").unlink()
    expect_refusal("retained snapshot escapes its scratch", retained_snapshot,
                   [fixture.root / "real-tree"], fixture.root / "elsewhere")
    link_tree = fixture.root / "link-tree"
    (link_tree / "inside").mkdir(parents=True)
    (link_tree / "inside/f.txt").write_bytes(b"y")
    os.symlink("inside/f.txt", link_tree / "alias")
    snap = expect_ok("retained snapshot records symlinks", retained_snapshot,
                     [link_tree], fixture.root)
    if snap is not None:
        stable_digest = snap[0]["inventory_sha256"]
        os.rename(link_tree / "alias", link_tree / "moved-link")
        moved = expect_ok("retained snapshot after a symlink move", retained_snapshot,
                          [link_tree], fixture.root)
        if moved is None or moved[0]["inventory_sha256"] == stable_digest:
            failures.append("retained snapshot missed a symlink rename")
        os.rename(link_tree / "moved-link", link_tree / "alias")

    # --- packet/schema conformance (review finding 1) -----------------------
    shapes = {
        "barge.validation-block/1": (build_barge_block_proposal(
            head=BARGE_RECIPE_HEAD, tree=BARGE_RECIPE_TREE,
            wrapper_sha256=host["wrapper_sha256"], audit_sha256=None,
            ledger_sha256=None, block_id=None, authorization_reference=None),
            {"schema", "task", "block_id", "authorization_reference", "head",
             "barge_tree", "wrapper_sha256", "oxide_sha256", "audit_sha256",
             "ledger_sha256", "max_seconds", "scratch_cap_bytes", "pool_cap_bytes",
             "selectors"}),
        "barge.private-roots/1": (build_barge_ledger(
            roots=[private_root], evidence=[(doc_path, "0" * 64)],
            review_reference=None),
            {"schema", "task", "review_reference", "discovery_evidence", "roots"}),
        "oxide budget": (build_oxide_budget(roots=[oxide_run_root, *others],
                                            history_reference=None),
                         {"task", "history_reference", "covering_seconds_charged",
                          "retained_roots"}),
        "oxide history": (build_oxide_history(
            roots=budget["retained_roots"], evidence={}, complete=False,
            unknown=["u"]), {"task", "complete", "retained_roots", "evidence", "unknown"}),
        "oxide block": (build_oxide_block_proposal(
            oxide_root=oxide_run_root, commit="e" * 40, tree="f" * 40,
            binding_sha256=None, budget_sha256=None, history_sha256=None,
            one_use_id=None, quiescent=False),
            {"task", "root", "planner_reference", "original_admission_charge_seconds",
             "prospective_max_seconds", "source_commit", "source_tree",
             "binding_sha256", "ledger_sha256", "history_sha256", "one_use_id",
             "retained_roots_quiescent"}),
        "gate receipt": (gate_receipt(
            gate_id="0" * 32, blocks={}, allowance={}, bindings={}, commands=[],
            observations={}, exports={}, failures=[], decisions=[]),
            {"schema", "gate_id", "qualification", "blocks", "allowance",
             "bindings", "commands", "observations", "exports", "failures",
             "decisions"}),
    }
    for label, (document, expected_keys) in shapes.items():
        if set(document) != expected_keys:
            failures.append(f"{label} shape does not match its enforced field set: "
                            f"{sorted(set(document) ^ expected_keys)}")
    if set(good_audit) != {"schema", "review_reference", "oxide_commit", "oxide_tree",
                           "admission_hook", "admission_socket", "admission_executables",
                           "admission_runtime_files", "child_path", "cargo_sha256",
                           "python_sha256", "components"}:
        failures.append("audit shape does not match its enforced field set")
    built_audit = expect_ok("audit builder produces the full document", build_barge_audit,
                            private=private_root,
                            component_sources={name: (analysis, [("oxide-source/src/bin/oxide/config.rs", "1-1")])
                                               for name in BARGE_AUDIT_COMPONENTS},
                            host={"effigy_sha256": "1" * 64, "oxide_sha256": "2" * 64,
                                  "wrapper_sha256": "3" * 64, "git_sha256": "4" * 64,
                                  "rustc_sha256": "5" * 64, "supervisor_sha256": "6" * 64},
                            admission={"hook": "hook-command", "socket": "/sockets/admission.sock",
                                       "executables": [{"path": str(sample), "sha256": sha256_file(sample)}],
                                       "runtime_files": [{"path": str(sample), "sha256": sha256_file(sample)}]},
                            child_facts={"child_path": "/bin:/usr/bin", "cargo_sha256": "7" * 64,
                                         "python_sha256": "8" * 64})
    if built_audit is not None:
        built_audit["review_reference"] = "retained review 881c1ef9"
        expect_ok("built audit validates", validate_barge_audit, built_audit,
                  private=private_root,
                  host={"effigy_sha256": "1" * 64, "oxide_sha256": "2" * 64,
                        "wrapper_sha256": "3" * 64, "git_sha256": "4" * 64,
                        "rustc_sha256": "5" * 64, "supervisor_sha256": "6" * 64})

    # --- supervisor context binding (review finding 3b) ---------------------
    saved_env = {k: os.environ.get(k) for k in ("OXIDE_PROOF_EVIDENCE",
                                                "OXIDE_PROOF_RUN_ID",
                                                "OXIDE_PROOF_DEADLINE_NS",
                                                "OXIDE_TASK_SCRATCH_ROOT")}
    try:
        for key in saved_env:
            os.environ.pop(key, None)
        expect_refusal("supervisor context without a run identity", supervisor_context)
        os.environ["OXIDE_PROOF_RUN_ID"] = "short"
        expect_refusal("supervisor context with a malformed run id", supervisor_context)
        os.environ["OXIDE_PROOF_RUN_ID"] = "a" * 32
        expect_refusal("supervisor context without a deadline", supervisor_context)
        os.environ["OXIDE_PROOF_DEADLINE_NS"] = "later"
        expect_refusal("supervisor context with a non-numeric deadline", supervisor_context)
        os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(now_ns() + 1_000_000_000)
        expect_refusal("supervisor context without evidence", supervisor_context)
        os.environ["OXIDE_PROOF_EVIDENCE"] = str(fixture.root / "absent-evidence")
        expect_refusal("supervisor context with absent evidence", supervisor_context)
        evidence_dir = fixture.root / "supervised-evidence"
        evidence_dir.mkdir()
        os.environ["OXIDE_PROOF_EVIDENCE"] = str(evidence_dir)
        os.environ["OXIDE_TASK_SCRATCH_ROOT"] = str(evidence_dir)
        resolved = expect_ok("supervisor context accepted", supervisor_context)
        if resolved is None or resolved[1] != "a" * 32:
            failures.append("supervisor context did not bind the run identity")
    finally:
        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

    fixture.close()
    if failures:
        for failure in failures:
            print(f"self-test failure: {failure}")
        print(f"private-build-validation self-test: {len(failures)} failures")
        return 1
    print("private-build-validation self-test: all mocked gate and refusal controls passed")
    return 0


def main(argv: list) -> int:
    options = parse_args(list(argv))
    if options["mode"] == "self-test":
        return self_test()
    if options["mode"] == "plan":
        return plan(options)
    if options["mode"] == "prepare":
        return prepare(options)
    return diagnostic(options)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except GateRefusal as refusal:
        print(f"private-build-validation refused: {refusal}", file=sys.stderr)
        sys.exit(2)
    except (KeyError, OSError, ValueError, TypeError, UnicodeError,
            subprocess.SubprocessError) as error:
        print(f"private-build-validation refused: {type(error).__name__}: {error}",
              file=sys.stderr)
        sys.exit(2)
