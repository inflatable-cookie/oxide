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
of the one global allowance), retain a bounded attributable bootstrap
provenance packet BEFORE the inventory assertion can refuse (oxide#253: the
actual passed allowlist environment, cwd, argv, tool identities, source+lock
bindings, ambient-Cargo-configuration scan, nested guard receipt reference and
the resolved artifact; refusals keep the packet), run the inventory-bracketed
private status dry gate, re-snapshot every retained root, and export the
wrapper-consumable artifacts plus the remaining measured allowance. Any
unknown refuses without repair, retry or attribution.

Cross-checkout preparation binding (operator `af038a7e` v2; planner
relative-path correction): preparation may run at one Queue checkout while
the diagnostic runs at the eventual merged-source proof-only checkout, so
the diagnostic never resolves the receipt's recorded script path (it may be
relative and would otherwise resolve in the wrong cwd, or pass on shape
alone). Preparation instead writes a deterministic `source-binding.json`
beside the packet manifest positively binding the whole guard-recorded
command, the preparation script's actual SHA-256, the qualified supervisor
bytes and the packet manifest digest; the diagnostic requires the
planner-bound `--expect-source-binding` digest, re-reads both retained
documents, and accepts only identical reviewed source bytes at any checkout.

Candidate route (`--candidate`, oxide#254; operator decision `d9aec625`):
the same guarded preparation, issuance and private status gate, but the
private debug Oxide is the RETAINED artifact from the spent replacement
diagnostic (`replacement-runtime.0xj2w99b/oxide-source/target/debug/oxide`,
SHA-256 and byte size planner-bound) instead of a new bootstrap build. Its
origin, exact source/lock/toolchain/artifact binding and the retained nested
closure are validated from planner-digested retained documents BEFORE any
staging; the artifact is then copied only inside the fresh guard-owned root,
rehashed, and symlinked as `bin/oxide`. No build, install, cleanup or repair
happens and the origin tree is never written. The named linker/assembler
tools and the pinned 1.97.1 toolchain are audited host inputs for this route:
planner-bound canonical paths plus content hashes, absent stays absent, and
any changed or unlisted binding refuses; hashing binds admission identity and
never confines a host tool's writes. The wrapper-consumable issued documents
and debug receipt are NOT derived here: the accepted wrapper binds a debug
receipt to an in-root bootstrap build, so a retained candidate cannot honestly
fill it; capture-side handoff stays on the capture tasks. The prior session's
failed isolation stays historical; this route takes its own fresh account
inventory baseline and refuses any inode/content delta.

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

import ctypes
import errno
import hashlib
import io
import struct
import json
import math
import os
import pwd
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import tarfile
import time

# ---------------------------------------------------------------------------
# Pinned identities (accepted sources; compatibility mirrors, not new schemas)
# ---------------------------------------------------------------------------

OXIDE_PIN_COMMIT = "3de241325dd286ae44a5759fa6825b05a81c7b4e"
OXIDE_PIN_TREE = "452a9c2dc6b1432c643c5053655e029955977754"
# Source-approved accepted Barge launcher head (`scripts/tape/admission.py`)
# including its cap revision; retained review run 9c36470a-54ab-44a8-9e15-28ce1789e9c4.
BARGE_ACCEPTED_HEAD = "60c2c793e0743ebb389dd433cec53251d00a0b87"
BARGE_RECIPE_HEAD = "249f0e4c904b692de13668af77fd697ff7c5f7fc"
BARGE_RECIPE_TREE = "1c8cdc151baa5794f5dcfcda4cc801031820dbcb"
SUPERVISOR_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
BARGE_TASK = "9f435fb3-4374-4f23-ba6d-7c5e6869f4ae"
OXIDE_TASK = "d41de092-b0b0-495d-b159-8c4d1730d6b4"
BARGE_SCRATCH = Path("/Volumes/Builds/scratch/barge")
OXIDE_SCRATCH = Path("/Volumes/Builds/scratch/oxide")
SCRATCH_ROOT = Path("/Volumes/Builds/scratch")
BARGE_CAP_BYTES = 3 * 1024 * 1024 * 1024
OXIDE_CAP_BYTES = 8 * 1024 * 1024 * 1024
# Prospective aggregate Builds scratch grants (operator decision af038a7e v2,
# 2026-11-02); the earlier 2 GiB / 6 GiB sessions and their evidence keep
# their original historical bounds and are never silently raised.
HISTORIC_BARGE_CAP_BYTES = 2 * 1024 * 1024 * 1024
HISTORIC_OXIDE_CAP_BYTES = 6 * 1024 * 1024 * 1024
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
SCHEMA_SOURCE_BINDING = "oxide.private-build-validation.source-binding/1"
SCHEMA_HOST_TOOLS = "oxide.private-build-validation.host-tools/1"
SCHEMA_CANDIDATE_RECEIPT = "oxide.private-build-validation.candidate/1"
SCHEMA_BARGE_AUDIT = "barge.validation-source-audit/1"
SCHEMA_BARGE_LEDGER = "barge.private-roots/1"
SCHEMA_BARGE_BLOCK = "barge.validation-block/1"
# Operator ruling d9aec625-ecc1-4dcf-a0be-737f85d9eca0 (2026-11-03, "Approve
# proposed validation route"): the pinned 1.97.1 toolchain and the named
# linker/assembler tools are audited host inputs for the candidate route only.
# They are host tools, not private bindings; content hashes bind admission
# identity and never confine their writes.
CANDIDATE_RULING = "d9aec625-ecc1-4dcf-a0be-737f85d9eca0"
# The retained candidate from the spent replacement diagnostic (oxide#252
# build, oxide#253 failed isolation). Path relative to the planner-bound
# origin root; the artifact itself is a regular file (bin/oxide is the
# symlink). It is evidence, never qualified by existing.
CANDIDATE_ORIGIN_RELATIVE = Path("oxide-source/target/debug/oxide")
CANDIDATE_PROVENANCE_PATH = Path("evidence/candidate-provenance.json")
MAX_CANDIDATE_PROVENANCE_BYTES = 256 * 1024
# The retained build log is one JSON line per Cargo message, capped like the
# guarded bootstrap output evidence.
MAX_CANDIDATE_LOG_BYTES = 32 * 1024 * 1024

# The accepted Barge launcher (`scripts/tape/admission.py` at 60c2c79, the
# source-approved cap revision) enforces exactly these selectors and component
# names; mirrors, kept
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
MAX_AUDIT_SOURCE_BYTES = 64 * 1024 * 1024
MAX_ADMISSION_ARTIFACT_BYTES = 256 * 1024 * 1024
INVENTORY_ROW_CAP = 50000
INVENTORY_CONTENT_CAP = 256 * 1024 * 1024
RETAINED_ROW_CAP = 200000
ALLOC_ENTRY_CAP = 500000
LIVE_ACCOUNT_ROOT = Path(pwd.getpwuid(os.getuid()).pw_dir) / "Library/Application Support/oxide"

# Bootstrap provenance packet (oxide#253): internal bounded evidence, written
# inside the monitored task root BEFORE the live-inventory assertion can
# refuse, so a real isolation failure still names the audited bootstrap path.
# It is not a public schema and no wrapper consumes it; it widens no oracle
# and weakens none. The packet records only the audited allowlist actually
# passed to the child — inherited values are never recorded, only key names.
PROVENANCE_PATH = Path("evidence/bootstrap-provenance.json")
MAX_PROVENANCE_BYTES = 256 * 1024
MAX_PROVENANCE_ENTRIES = 32
MAX_PROVENANCE_CONFIG_BYTES = 1024 * 1024
MAX_PRIVATE_STATE_BYTES = 64 * 1024
MAX_ANCESTOR_LEVELS = 64
# Named linker/assembler tools cargo/rustc resolve on PATH. A private
# binding is an executable regular file inside the private root, or a
# symlink whose resolved target is an executable regular file inside the
# private root. A dangling symlink, a non-executable file, and a symlink
# to a host tool are not private bindings. Absence is not a violation.
BOOTSTRAP_PATH_TOOLS = ("cc", "c++", "clang", "gcc", "ld", "as", "ar", "ranlib")
PRIVATE_HOME_STATE = Path("Library/Application Support/oxide/state/pools.json")
# The complete key set child_environment may pass; the provenance builder
# refuses any passed key outside it (a positive private binding per key).
BOOTSTRAP_ENV_KEYS = frozenset((
    "EFFIGY_ADMISSION_HOOK", "QUEUE_ADMISSION_SOCKET",
    *BUILD_ENV_KEYS,
    "PATH", "TMP", "TEMP", "OXIDE_POOL_BACKEND", "OXIDE_POOL_CAP",
    "BARGE_PRIVATE_ROOT", "BARGE_PRIVATE_OXIDE", "BARGE_PINNED_RUSTC",
    "BARGE_PRIVATE_WRAPPER_SHA256", "RUSTUP_AUTO_INSTALL", "RUSTUP_NO_UPDATE_CHECK",
    "PYTHONDONTWRITEBYTECODE",
))


class GateRefusal(Exception):
    """A bounded refusal: the gate stops without repair, retry or blame."""


def now_ns() -> int:
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


# ---------------------------------------------------------------------------
# Primitives: digests, canonical private paths, canonical documents
# ---------------------------------------------------------------------------

def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def deadline_check(deadline_ns: int | None) -> None:
    if deadline_ns is not None and now_ns() >= deadline_ns:
        raise GateRefusal("effective deadline exhausted")


def sha256_file(path: Path, cap: int = MAX_DOC_BYTES, *, deadline_ns: int | None = None) -> str:
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
            deadline_check(deadline_ns)
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
        try:
            if other.parent.resolve(strict=True) != other.parent:
                raise GateRefusal("retained root parent is not canonical")
        except OSError as error:
            raise GateRefusal(f"retained root parent unreadable: {other}: {error}") from None
        rows = []
        size = 0

        def on_error(error: OSError, other=other):
            raise GateRefusal(f"retained root unreadable: {other}: {error}")

        try:
            st_root = os.lstat(other)
        except OSError as error:
            raise GateRefusal(f"retained root unreadable: {other}: {error}") from None
        if stat.S_ISREG(st_root.st_mode):
            if st_root.st_uid != os.geteuid() or st_root.st_mode & 0o444 == 0:
                raise GateRefusal(f"foreign or unreadable retained file: {other}")
            if count >= RETAINED_ROW_CAP or (deadline_ns is not None and now_ns() > deadline_ns):
                raise GateRefusal("retained snapshot row or time cap reached")
            try:
                fd = os.open(other, os.O_RDONLY | os.O_NOFOLLOW)
            except OSError as error:
                raise GateRefusal(f"retained file unreadable: {other}: {error}") from None
            try:
                opened = os.fstat(fd)
                if (_stable(st_root), st_root.st_ctime_ns) != (_stable(opened), opened.st_ctime_ns):
                    raise GateRefusal("retained file changed before reading")
                h = hashlib.sha256()
                length = 0
                while True:
                    if deadline_ns is not None and now_ns() > deadline_ns:
                        raise GateRefusal("retained file exceeded its time bound")
                    data = os.read(fd, 1024 * 1024)
                    if not data:
                        break
                    length += len(data)
                    if length > INVENTORY_CONTENT_CAP:
                        raise GateRefusal("retained file exceeds content cap")
                    h.update(data)
                after = os.fstat(fd)
                named = os.lstat(other)
                if ((_stable(opened), opened.st_ctime_ns) != (_stable(after), after.st_ctime_ns)
                        or (_stable(after), after.st_ctime_ns) != (_stable(named), named.st_ctime_ns)):
                    raise GateRefusal("retained file changed during reading")
            except OSError as error:
                raise GateRefusal(f"retained file unreadable: {other}: {error}") from None
            finally:
                os.close(fd)
            count += 1
            size = max(st_root.st_size, st_root.st_blocks * 512)
            rows.append([".", *(_stable(st_root)), st_root.st_uid, st_root.st_gid,
                         st_root.st_blocks, st_root.st_ctime_ns, h.hexdigest()])
        elif not stat.S_ISDIR(st_root.st_mode):
            raise GateRefusal(f"retained root is not a regular file or directory: {other}")

        for directory, dirs, files in (os.walk(other, followlinks=False, onerror=on_error)
                                        if stat.S_ISDIR(st_root.st_mode) else []):
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
        elif stat.S_ISREG(st.st_mode):
            if st.st_uid != os.geteuid() or st.st_mode & 0o444 == 0:
                raise GateRefusal("foreign or unreadable private file")
            try:
                fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
                try:
                    opened = os.fstat(fd)
                    after = os.lstat(path)
                    if ((_stable(st), st.st_ctime_ns) != (_stable(opened), opened.st_ctime_ns)
                            or (_stable(opened), opened.st_ctime_ns) != (_stable(after), after.st_ctime_ns)):
                        raise GateRefusal("private file changed while counted")
                finally:
                    os.close(fd)
            except OSError as error:
                raise GateRefusal(f"private file unreadable: {path}: {error}") from None
    return total


# ---------------------------------------------------------------------------
# Environment: audited admission binding and the scrubbed child environment
# ---------------------------------------------------------------------------

def audited_admission(inherited: dict, audit: dict, *, deadline_ns: int | None = None) -> dict:
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
                    or sha256_file(path, cap=MAX_ADMISSION_ARTIFACT_BYTES, deadline_ns=deadline_ns) != item["sha256"]):
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
    rustup_home = inherited.get("RUSTUP_HOME") or (str(Path(inherited["HOME"]) / ".rustup") if inherited.get("HOME") else "")
    if inherited.get("RUSTUP_TOOLCHAIN") != TOOLCHAIN or not rustup_home or not Path(rustup_home).is_absolute():
        raise GateRefusal("declared Rust toolchain binding missing in the invoking environment")
    env = {key: inherited[key] for key in ("RUSTUP_TOOLCHAIN", "PATH") if key in inherited}
    env["RUSTUP_HOME"] = rustup_home
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
            sources.append({"path": str(real), "sha256": sha256_file(real, cap=MAX_AUDIT_SOURCE_BYTES), "lines": lines})
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
        raise GateRefusal("private scratch cap must be the approved 3 GiB")
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


def live_audit_host(*, effigy: Path, git_tool: Path, rustc_tool: Path,
                    python: Path, wrapper_hash: str, oxide_hash: str, deadline_ns: int | None = None) -> dict:
    cap = 256 * 1024 * 1024
    return {"effigy_sha256": sha256_file(effigy, cap=cap, deadline_ns=deadline_ns),
            "git_sha256": sha256_file(git_tool, cap=cap, deadline_ns=deadline_ns),
            "rustc_sha256": sha256_file(rustc_tool, cap=cap, deadline_ns=deadline_ns),
            "supervisor_sha256": SUPERVISOR_SHA256,
            "wrapper_sha256": wrapper_hash, "oxide_sha256": oxide_hash,
            "cargo_sha256": sha256_file((rustc_tool.parent / "cargo").resolve(strict=True), cap=cap, deadline_ns=deadline_ns),
            "python_sha256": sha256_file(python, cap=cap, deadline_ns=deadline_ns)}


def bind_barge_documents(block_template: dict, audit_template: dict, oxide_hash: str) -> tuple:
    """Derive only the deferred artifact and audit digests from issued templates."""
    if (not isinstance(block_template, dict) or not isinstance(audit_template, dict)
            or block_template.get("oxide_sha256", "missing") is not None
            or block_template.get("audit_sha256", "missing") is not None
            or audit_template.get("oxide_sha256", "missing") is not None
            or not isinstance(block_template.get("authorization_reference"), str)
            or not block_template["authorization_reference"].startswith("derived-artifact-v1:")
            or not block_template["authorization_reference"][len("derived-artifact-v1:"):].strip()
            or not isinstance(oxide_hash, str) or not HEX64.fullmatch(oxide_hash)):
        raise GateRefusal("issued templates must defer exactly the bootstrap artifact binding")
    audit = dict(audit_template, oxide_sha256=oxide_hash)
    audit_bytes = canonical_json(audit)
    block = dict(block_template, oxide_sha256=oxide_hash,
                 audit_sha256=sha256_bytes(audit_bytes))
    return block, audit


def validate_barge_audit(audit, *, private, host: dict, deadline_ns: int | None = None) -> None:
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
                    or not path.is_file() or sha256_file(path, cap=MAX_AUDIT_SOURCE_BYTES, deadline_ns=deadline_ns) != expected):
                raise GateRefusal(f"{name}: source changed or unreadable")


def validate_barge_ledger(ledger, *, private_root: Path, scratch: Path,
                          historic: Path = BARGE_HISTORIC_LEDGER,
                          scratch_glob: str = "041-pr.*",
                          required_roots=None, deadline_ns: int | None = None) -> list:
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
                or sha256_file(path, deadline_ns=deadline_ns) != item["sha256"]):
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


def preparation_packet_dir(prep_options: dict, prep_scratch: Path) -> Path:
    """Locate the retained preparation packet named by the recorded command."""
    text = prep_options.get("packet_dir") if isinstance(prep_options, dict) else None
    if not isinstance(text, str) or not text.strip():
        raise GateRefusal("the prepared command names no packet directory")
    packet_dir = Path(text)
    try:
        if (not packet_dir.is_absolute() or packet_dir.is_symlink()
                or packet_dir.resolve(strict=True) != packet_dir or not packet_dir.is_dir()):
            raise GateRefusal("preparation packet directory is missing or aliased")
    except OSError as error:
        raise GateRefusal(f"preparation packet directory is unreadable: {error}") from None
    if not packet_dir.is_relative_to(prep_scratch) or packet_dir == prep_scratch:
        raise GateRefusal("preparation packet must live inside the retained preparation root")
    return packet_dir


def validate_preparation_binding(*, binding, prep_command: list, prep_run_id,
                                 prep_scratch: Path, packet_dir: Path,
                                 current_script_sha256: str, barge_root: Path,
                                 oxide_root: Path, barge_head: str, oxide_head: str,
                                 oxide_product_head: str,
                                 audit_inputs_sha256: str, deadline_ns: int | None = None) -> dict:
    """Bind the preparation's actual bytes, guard, command and packet manifest.

    The recorded script path is never resolved here: it may be relative (the
    historical receipt names `scripts/test-private-build-validation.py`, which
    would resolve in this process's cwd and pass without any byte evidence) or
    absolute at a checkout that no longer exists. The binding document instead
    carries the actual SHA-256 of the preparation script bytes, computed in
    the preparation process itself, and this diagnostic compares it with its
    own running bytes: identical reviewed source at a different checkout is
    accepted; changed script or guard bytes, an edited command token, a
    mismatched or edited manifest or packet document refuses.
    """
    if (not isinstance(binding, dict) or set(binding) != {
            "schema", "command", "script_sha256", "supervisor_sha256",
            "packet_manifest_sha256"}):
        raise GateRefusal("preparation source binding is missing or malformed")
    if binding["schema"] != SCHEMA_SOURCE_BINDING:
        raise GateRefusal("preparation source binding schema mismatch")
    if binding["supervisor_sha256"] != SUPERVISOR_SHA256:
        raise GateRefusal("preparation source binding does not name the qualified guard bytes")
    if binding["command"] != prep_command:
        raise GateRefusal("preparation source binding does not match the whole prepared command")
    if binding["script_sha256"] != current_script_sha256:
        raise GateRefusal("preparation source binding does not match this "
                          "diagnostic's reviewed script bytes")
    manifest_path = packet_dir / "packet-manifest.json"
    try:
        manifest = read_json_bound(manifest_path, binding["packet_manifest_sha256"])
    except OSError as error:
        raise GateRefusal(f"preparation packet manifest is missing or unreadable: {error}") from None
    expected_keys = {"supervised_run_id", "supervisor_deadline_ns", "clock", "packet_dir",
                     "barge_root", "oxide_root", "barge_head", "oxide_head",
                     "oxide_product_head", "audit_inputs_sha256", "documents", "state"}
    if set(manifest) != expected_keys or manifest["state"] != (
            "unissued preparation; planner binding and review come first") \
            or type(manifest["supervisor_deadline_ns"]) is not int:
        raise GateRefusal("preparation packet manifest shape mismatch")
    expected = {"supervised_run_id": prep_run_id, "packet_dir": str(packet_dir),
                "barge_root": str(barge_root), "oxide_root": str(oxide_root),
                "barge_head": barge_head, "oxide_head": oxide_head,
                "oxide_product_head": oxide_product_head,
                "audit_inputs_sha256": audit_inputs_sha256,
                "clock": "CLOCK_UPTIME_RAW"}
    for key, value in expected.items():
        if manifest[key] != value:
            raise GateRefusal(f"preparation packet manifest does not bind {key}")
    documents = manifest["documents"]
    if not isinstance(documents, dict) or not documents or len(documents) > 64:
        raise GateRefusal("preparation packet manifest document bindings are missing or oversized")
    for name, digest in documents.items():
        if (not isinstance(name, str) or not name.endswith(".json")
                or Path(name).name != name or len(name) > 200
                or not isinstance(digest, str) or not HEX64.fullmatch(digest)):
            raise GateRefusal("preparation packet manifest document binding is malformed")
    for name, digest in sorted(documents.items()):
        try:
            if sha256_file(canonical_file(packet_dir / name), deadline_ns=deadline_ns) != digest:
                raise GateRefusal(f"preparation packet document changed: {name}")
        except OSError as error:
            raise GateRefusal(f"preparation packet document {name} is missing or unreadable: "
                              f"{error}") from None
    return manifest


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


def verify_inventory(before: list, after: list, phase: str = "unspecified",
                     evidence: Path | None = None) -> None:
    old = {row[0]: row for row in before}
    new = {row[0]: row for row in after}
    if len(old) != len(before) or len(new) != len(after):
        raise GateRefusal("duplicate live inventory path; isolation unproven")
    if old != new:
        differences = []
        total = 0
        for name in sorted(old.keys() | new.keys()):
            left, right = old.get(name), new.get(name)
            if left == right:
                continue
            total += 1
            if len(differences) < 8:
                kind = "added" if left is None else "removed" if right is None else "changed"
                fields = []
                if left is not None and right is not None:
                    if left[1] != right[1]:
                        fields.append("metadata")
                    if left[2:] != right[2:]:
                        fields.append("content-or-target")
                differences.append({"path_sha256": sha256_bytes(name.encode("utf-8", "surrogateescape")),
                                    "kind": kind, "fields": fields})
        detail = {"phase": phase, "changed_rows": total, "rows": differences,
                  "omitted": total - len(differences)}
        if evidence is not None:
            payload = canonical_json({"phase": phase, "before": before, "after": after,
                                      "difference": detail})
            if len(payload) > MAX_DOC_BYTES:
                raise GateRefusal("live inventory changed; isolation failed; evidence exceeds bound")
            private_write(evidence, Path(f"live-inventory-{phase}.json"), payload)
        raise GateRefusal("live inventory changed; isolation failed; " +
                          canonical_json(detail).decode("utf-8"))


# ---------------------------------------------------------------------------
# Source archives (read-only Git, exact tree and mode binding)
# ---------------------------------------------------------------------------

def git_output(repo: Path, *args: str, deadline_ns: int | None = None) -> bytes:
    env = {"PATH": "/usr/bin:/bin", "HOME": "/nonexistent", "GIT_CONFIG_NOSYSTEM": "1",
           "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_OPTIONAL_LOCKS": "0"}
    timeout = None if deadline_ns is None else (deadline_ns - now_ns()) / 1_000_000_000
    if timeout is not None and timeout <= 0:
        raise GateRefusal("Git deadline exhausted before launch")
    try:
        result = subprocess.run(["/usr/bin/git", "-C", str(repo), *args], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
                                timeout=timeout)
    except subprocess.TimeoutExpired:
        raise GateRefusal("Git deadline exhausted") from None
    if result.returncode != 0 or len(result.stdout) > 64 * 1024 * 1024:
        raise GateRefusal(f"read-only git {args[0]} failed or exceeded its bound")
    return result.stdout


def audit_retained_roots(manifest: dict) -> list:
    """Validate explicit task-private roots before any retained-tree walk."""
    roots = manifest.get("retained_roots") if isinstance(manifest, dict) else None
    if not isinstance(roots, list) or not roots or len(roots) > 64:
        raise GateRefusal("audit manifest needs a bounded retained-root list")
    paths = []
    for value in roots:
        if not isinstance(value, str):
            raise GateRefusal("audit retained root is not a path")
        path = Path(value)
        if (not path.is_absolute() or ".." in path.parts
                or not any(path != base and path.is_relative_to(base)
                           for base in (BARGE_SCRATCH, OXIDE_SCRATCH))):
            raise GateRefusal("audit retained root escapes task scratch")
        try:
            info = os.lstat(path)
            resolved = path.resolve(strict=True)
        except OSError:
            raise GateRefusal("audit retained root is missing or unreadable") from None
        if (resolved != path or not stat.S_ISDIR(info.st_mode)
                or info.st_uid != os.geteuid() or info.st_mode & 0o022):
            raise GateRefusal("audit retained root is aliased, foreign or writable by others")
        if path in paths:
            raise GateRefusal("audit retained roots contain a duplicate")
        paths.append(path)
    return paths


def stage_audit_inputs(root: Path, manifest: dict, *, deadline_ns: int | None = None) -> None:
    """Copy only explicit, digest-bound audit inputs into an owned phase root."""
    entries = manifest.get("inputs") if isinstance(manifest, dict) else None
    if not isinstance(entries, list) or not entries or len(entries) > 256:
        raise GateRefusal("audit input manifest is missing or too large")
    total = 0
    seen = set()
    for entry in entries:
        deadline_check(deadline_ns)
        if not isinstance(entry, dict) or set(entry) != {"source", "destination", "sha256"}:
            raise GateRefusal("malformed audit input")
        source = canonical_file(Path(entry["source"]))
        destination = Path(entry["destination"])
        digest = entry["sha256"]
        if (destination.is_absolute() or len(destination.parts) < 2
                or destination.parts[0] not in ("effigy-source", "toolchain", "dependency-source")
                or any(part in ("", ".", "..") for part in destination.parts)
                or str(destination) in seen or not isinstance(digest, str)
                or not HEX64.fullmatch(digest)):
            raise GateRefusal("audit input destination or digest invalid")
        seen.add(str(destination))
        size = source.stat().st_size
        total += size
        if size > MAX_AUDIT_SOURCE_BYTES or total > 256 * 1024 * 1024:
            raise GateRefusal("audit inputs exceed the bounded copy budget")
        if sha256_file(source, cap=MAX_AUDIT_SOURCE_BYTES, deadline_ns=deadline_ns) != digest:
            raise GateRefusal("audit input source changed")
        data = source.read_bytes()
        if sha256_bytes(data) != digest:
            raise GateRefusal("audit input changed while copied")
        private_write(root, destination, data)


def stage_git_tree(repo: Path, root: Path, commit: str, expected_tree: str, *, deadline_ns: int | None = None) -> dict:
    """Stage one bounded Git archive into an empty private root and verify it."""
    if any(root.iterdir()):
        raise GateRefusal("source archive destination is not empty")
    data = git_output(repo, "archive", "--format=tar", commit, deadline_ns=deadline_ns)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
        members = archive.getmembers()
        if len(members) > 50000:
            raise GateRefusal("source archive contains too many members")
        deferred = []
        for member in members:
            if deadline_ns is not None and now_ns() >= deadline_ns:
                raise GateRefusal("source archive deadline exhausted")
            relative = Path(member.name.rstrip("/"))
            if (relative.is_absolute() or not relative.parts
                    or any(p in ("", ".", "..") for p in relative.parts)):
                raise GateRefusal("unsafe source archive member")
            if member.isdir():
                private_dir(root, relative)
            elif member.isfile():
                if member.size > 64 * 1024 * 1024:
                    raise GateRefusal("source archive member exceeds bound")
                stream = archive.extractfile(member)
                if stream is None:
                    raise GateRefusal("source archive member unreadable")
                path = private_write(root, relative, stream.read(member.size + 1))
                if path.stat().st_size != member.size:
                    raise GateRefusal("source archive member size mismatch")
                os.chmod(path, 0o700 if member.mode & 0o111 else 0o600)
            elif member.issym():
                deferred.append((relative, member.linkname))
            else:
                raise GateRefusal("unsupported source archive member")
        for relative, target in deferred:
            if deadline_ns is not None and now_ns() >= deadline_ns:
                raise GateRefusal("source archive deadline exhausted")
            private_dir(root, relative.parent)
            os.symlink(target, root / relative)
    return extract_git_tree(repo, root, commit, expected_tree, deadline_ns=deadline_ns)


def extract_git_tree(repo: Path, root: Path, commit: str, expected_tree: str, *, deadline_ns: int | None = None) -> dict:
    """Materialize and verify every archived file and mode against the tree."""
    tree = git_output(repo, "rev-parse", commit + "^{tree}", deadline_ns=deadline_ns).decode("ascii").strip()
    resolved = git_output(repo, "rev-parse", commit + "^{commit}", deadline_ns=deadline_ns).decode("ascii").strip()
    if tree != expected_tree or resolved != commit:
        raise GateRefusal("source commit or tree mismatch")
    records = git_output(repo, "ls-tree", "-rz", "--full-tree", commit, deadline_ns=deadline_ns).split(b"\0")
    expected = {}
    for record in records:
        if deadline_ns is not None and now_ns() >= deadline_ns:
            raise GateRefusal("source verification deadline exhausted")
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
    actual, actual_dirs = set(), set()
    for path in root.rglob("*"):
        if deadline_ns is not None and now_ns() >= deadline_ns:
            raise GateRefusal("source membership deadline exhausted")
        relative = path.relative_to(root).as_posix()
        if path.is_file() or path.is_symlink():
            actual.add(relative)
        if path.is_dir() and not path.is_symlink():
            actual_dirs.add(relative)
    if actual != set(expected):
        raise GateRefusal("source archive membership mismatch")
    expected_dirs = {parent.as_posix() for name in expected
                     for parent in Path(name).parents if parent != Path(".")}
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

    def __init__(self, max_seconds: int, deadline_ns: int | None, state_dir: Path | None,
                 started_ns: int | None = None):
        if type(max_seconds) is not int or max_seconds <= 30:
            raise GateRefusal("the allowance needs a maximum above the supervisor floor")
        self.max_seconds = max_seconds
        self.deadline_ns = deadline_ns
        self.state_dir = state_dir
        self.started_ns = now_ns() if started_ns is None else started_ns
        if self.started_ns > now_ns():
            raise GateRefusal("allowance start is in the future")
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
        remaining = max(0, (self.started_ns + self.max_seconds * 1_000_000_000 - now_ns()) // 1_000_000_000)
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
        cap = min(self.bootstrap_started_ns + BOOTSTRAP_MAX_SECONDS * 1_000_000_000,
                  self.started_ns + self.max_seconds * 1_000_000_000)
        if self.deadline_ns is not None:
            cap = min(cap, self.deadline_ns)
        return cap

    def finish_bootstrap(self):
        if self.bootstrap_started_ns is None or self.bootstrap_finished_ns is not None:
            raise GateRefusal("bootstrap window is not open")
        self.bootstrap_finished_ns = now_ns()
        self._record("bootstrap-window", self.bootstrap_started_ns, self.bootstrap_finished_ns)

    def document(self) -> dict:
        return {
            "clock": "CLOCK_UPTIME_RAW",
            "max_seconds": self.max_seconds,
            "spent_seconds": (now_ns() - self.started_ns + 999_999_999) // 1_000_000_000,
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
        roots.add(path)
    roots.update(BARGE_SCRATCH.glob("041-pr.*"))
    for name in ("planner-041-approved-block-20261102", "planner-041-author-error-20261102",
                 "planner-041-revision-disposition-20261102",
                 "planner-041-validation-unblock-20261102", "planner-unblock-041-20261102"):
        candidate = BARGE_SCRATCH / name
        if candidate.exists():
            roots.add(candidate)
    roots.update(Path(p) for p in extra_roots)
    if any(not path.is_absolute() or path == BARGE_SCRATCH or not path.is_relative_to(BARGE_SCRATCH)
           or ".." in path.parts for path in roots):
        raise GateRefusal("Barge discovery cannot include the shared scratch base")
    roots = {path for path in roots if not any(parent != path and parent in path.parents
                                                for parent in roots)}
    evidence = []
    for path in (BARGE_HISTORIC_LEDGER, BARGE_PACKET / "root-discovery.json",
                 BARGE_PACKET / "queue-evidence.json",
                 BARGE_PACKET / "decision-evidence.json", BARGE_PACKET / "pr-41.json"):
        evidence.append((path, sha256_file(canonical_file(path))))
    return sorted(roots), evidence


OXIDE_DISCOVERY_PACKET = OXIDE_SCRATCH / "planner-041-240-unblock-20261102"
OXIDE_OWNER_PACKET = OXIDE_SCRATCH / "planner-041-240-owner-disposition-20261102"


CORRECTED_OXIDE_PACKET_SHA256 = "1c60e2374b76e9004ac99f27bcb0297623cabddb6d2422e29cbc26cd040bb54a"


def oxide_packet_roots(packet: dict, scratch: Path) -> tuple:
    """Consume attributed candidates, retaining unknown dispositions as unknown."""
    if (not isinstance(packet, dict) or packet.get("task") != OXIDE_TASK
            or packet.get("complete") is not False
            or not isinstance(packet.get("candidates"), list)):
        raise GateRefusal("corrected Oxide discovery packet is malformed")
    roots, unknown = set(), []
    for item in packet["candidates"]:
        if not isinstance(item, dict) or not isinstance(item.get("path"), str):
            raise GateRefusal("corrected Oxide discovery candidate is malformed")
        path = Path(item["path"])
        if not path.is_absolute() or path == scratch or not path.is_relative_to(scratch) or ".." in path.parts:
            raise GateRefusal("corrected Oxide candidate escapes scratch")
        disposition = item.get("disposition")
        if disposition == "attributed":
            if item.get("kind") not in ("file", "directory") or item.get("size_complete") is not True:
                raise GateRefusal("attributed Oxide candidate lacks its recorded shape")
            roots.add(path)
        elif disposition == "unknown":
            roots.add(path)  # conservatively count, without asserting attribution
            unknown.append(f"unresolved candidate: {path}: {item.get('basis', 'unknown')}")
        elif disposition != "not-attributable":
            raise GateRefusal("corrected Oxide candidate has unknown disposition")
    proposed = packet.get("retained_roots")
    if (not isinstance(proposed, list) or not proposed
            or packet.get("proposed_budget_retained_roots") != proposed
            or not set(map(Path, proposed)).issubset(roots)):
        raise GateRefusal("corrected Oxide retained roots do not match attributed candidates")
    if not isinstance(packet.get("unknown"), list) or any(not isinstance(x, str) for x in packet["unknown"]):
        raise GateRefusal("corrected Oxide packet unknowns are missing")
    return roots, unknown + packet["unknown"]


def discover_oxide_roots(extra_roots: list, packet_path: Path,
                         expected_sha256: str = CORRECTED_OXIDE_PACKET_SHA256) -> tuple:
    """Read the reviewed packet, retaining all attributed evidence and unknowns."""
    if not packet_path.is_absolute() or not packet_path.is_relative_to(OXIDE_SCRATCH):
        raise GateRefusal("corrected Oxide packet must be retained under Oxide scratch")
    packet_path = canonical_file(packet_path)
    packet = read_json_bound(packet_path, expected_sha256)
    roots, unknown = oxide_packet_roots(packet, OXIDE_SCRATCH)
    review = OXIDE_SCRATCH / "planner-240-review-disposition-20261102/queue-detail.json"
    roots.update((OXIDE_DISCOVERY_PACKET, OXIDE_OWNER_PACKET, packet_path, review))
    roots.update(Path(p) for p in extra_roots)
    roots = {path for path in roots if not any(parent != path and parent in path.parents
                                                for parent in roots)}
    unknown.extend(["quiescence of retained roots requires actual guarded observation",
                    "shared live pool/output sizes remain unknown and unread",
                    "current #240 launcher is not source-approved"])
    evidence = {str(packet_path): expected_sha256,
                str(canonical_file(review)): sha256_file(canonical_file(review))}
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
      --oxide-head HEX40 --oxide-product-head HEX40 --barge-repo REPO --barge-head HEX40
      --barge-root ROOT --oxide-root ROOT --audit-inputs PATH --expect-audit-inputs HEX64
  test-private-build-validation.py --diagnostic --oxide-repo REPO --oxide-head HEX40 --oxide-product-head HEX40
      --barge-repo REPO --barge-head HEX40 --barge-root ROOT --oxide-root ROOT
      --expect-audit HEX64 --expect-ledger HEX64 --expect-budget HEX64 --expect-history HEX64
      --expect-oxide-binding HEX64 --preparation-receipt PATH --expect-preparation HEX64
      --expect-source-binding HEX64 [--planner-reference STRING]
  test-private-build-validation.py --candidate <all --diagnostic arguments>
      --candidate-origin ROOT --expect-candidate HEX64 --candidate-bytes N
      --candidate-receipt PATH --expect-candidate-receipt HEX64
      --candidate-log PATH --expect-candidate-log HEX64
      --candidate-lock-sha256 HEX64 --candidate-toolchain-sha256 HEX64
      --host-tools PATH --expect-host-tools HEX64
  prepare environment: OXIDE_CORRECTED_DISCOVERY_PACKET (exact reviewed JSON)
  diagnostic/candidate environment: OXIDE_PROOF_DEADLINE_NS, OXIDE_PROOF_RUN_ID,
      OXIDE_PROOF_EVIDENCE, OXIDE_TASK_SCRATCH_ROOT (exported by the qualified
      supervisor), BARGE_VALIDATION_BLOCK[+_SHA256],
      BARGE_VALIDATION_SOURCE_AUDIT[+_SHA256], BARGE_VALIDATION_ROOT_LEDGER[+_SHA256],
      BARGE_VALIDATION_EFFIGY, BARGE_VALIDATION_GIT, BARGE_VALIDATION_SUPERVISOR,
      BARGE_PINNED_RUSTC, TAPE_BLOCK_APPROVAL_SHA256, RUSTUP_HOME, RUSTUP_TOOLCHAIN"""


def parse_args(argv: list) -> dict:
    modes = ("--self-test", "--plan", "--prepare", "--diagnostic", "--candidate", "--claim-oxide")
    if not argv or argv[0] not in modes:
        raise GateRefusal("unknown or missing mode; individual actions are disabled\n" + USAGE)
    options: dict = {"mode": argv[0][2:]}
    required = {
        "claim-oxide": ("block", "digest"),
        "prepare": ("packet_dir", "oxide_repo", "oxide_head", "oxide_product_head", "barge_repo", "barge_head",
                    "audit_inputs", "expect_audit_inputs",
                    "barge_root", "oxide_root"),
        "diagnostic": ("oxide_repo", "oxide_head", "oxide_product_head", "barge_repo", "barge_head", "barge_root",
                       "oxide_root", "expect_audit", "expect_ledger", "expect_budget",
                       "expect_history", "expect_oxide_binding", "preparation_receipt", "expect_preparation",
                       "expect_source_binding"),
    }.get(options["mode"], ())
    if options["mode"] == "candidate":
        required = ("oxide_repo", "oxide_head", "oxide_product_head", "barge_repo", "barge_head", "barge_root",
                    "oxide_root", "expect_audit", "expect_ledger", "expect_budget",
                    "expect_history", "expect_oxide_binding", "preparation_receipt", "expect_preparation",
                    "expect_source_binding", "candidate_origin", "expect_candidate", "candidate_bytes",
                    "candidate_receipt", "expect_candidate_receipt", "candidate_log",
                    "expect_candidate_log", "candidate_lock_sha256", "candidate_toolchain_sha256",
                    "host_tools", "expect_host_tools")
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
    for key in ("expect_audit", "expect_ledger", "expect_budget", "expect_history", "expect_oxide_binding",
                "expect_preparation", "expect_audit_inputs", "expect_source_binding", "digest",
                "expect_candidate", "expect_candidate_receipt", "expect_candidate_log",
                "candidate_lock_sha256", "candidate_toolchain_sha256", "expect_host_tools"):
        if key in options and not HEX64.fullmatch(options[key]):
            raise GateRefusal(f"--{key.replace('_', '-')} must be 64 hex")
    if "candidate_bytes" in options:
        try:
            options["candidate_bytes"] = int(options["candidate_bytes"])
        except ValueError:
            raise GateRefusal("--candidate-bytes must be an integer byte size") from None
        if type(options["candidate_bytes"]) is not int or options["candidate_bytes"] <= 0:
            raise GateRefusal("--candidate-bytes must be a positive integer byte size")
    for key in ("oxide_head", "oxide_product_head", "barge_head"):
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


def process_row(pid: int) -> dict:
    """Read one Darwin kinfo_proc row; identity start is never a clock.

    Match the qualified supervisor's size-probe plus slack and bounded ENOMEM
    retries. Only an exact, matching final row is an identity; every other
    result refuses with bounded numeric diagnostics.
    """
    libc = ctypes.CDLL(None, use_errno=True)
    mib = (ctypes.c_int * 4)(1, 14, 1, pid)
    row_size = 648
    for attempt in range(3):
        size = ctypes.c_size_t(0)
        if libc.sysctl(mib, 4, None, ctypes.byref(size), None, 0):
            raise GateRefusal(f"process identity size query failed: errno={ctypes.get_errno()} size={size.value}")
        reported = size.value
        if reported > 1024 * 1024:
            raise GateRefusal(f"process identity size exceeds bound: size={reported}")
        capacity = reported + 64 * row_size
        buf = ctypes.create_string_buffer(capacity)
        size = ctypes.c_size_t(capacity)
        if libc.sysctl(mib, 4, buf, ctypes.byref(size), None, 0):
            error = ctypes.get_errno()
            if error == errno.ENOMEM and attempt < 2:
                continue
            raise GateRefusal(f"process identity read failed: errno={error} size={size.value} capacity={capacity}")
        if size.value != row_size:
            raise GateRefusal(f"process identity row size mismatch: size={size.value} expected={row_size}")
        raw = buf.raw[:size.value]
        sec, usec = struct.unpack_from("<qi", raw)
        actual_pid = struct.unpack_from("<i", raw, 40)[0]
        if actual_pid != pid:
            raise GateRefusal(f"process identity PID mismatch: expected={pid} observed={actual_pid}")
        return {"pid": actual_pid, "start": (sec, usec),
                "ppid": struct.unpack_from("<i", raw, 560)[0],
                "pgid": struct.unpack_from("<i", raw, 564)[0], "zombie": raw[36] == 5}
    raise GateRefusal("process identity read failed: bounded retries exhausted")


def process_argv(pid: int) -> list:
    """Read bounded KERN_PROCARGS2 argv, refusing malformed observations."""
    libc = ctypes.CDLL(None, use_errno=True)
    mib = (ctypes.c_int * 3)(1, 49, pid)
    buf = ctypes.create_string_buffer(262144)
    size = ctypes.c_size_t(len(buf))
    if libc.sysctl(mib, 3, buf, ctypes.byref(size), None, 0) or size.value < 8:
        raise GateRefusal("supervisor arguments unavailable")
    data = buf.raw[:size.value]
    argc = struct.unpack_from("i", data)[0]
    if not 1 <= argc <= 128:
        raise GateRefusal("supervisor argument count invalid")
    index = data.find(b"\0", 4)
    if index < 0:
        raise GateRefusal("supervisor executable path malformed")
    index += 1
    while index < len(data) and data[index] == 0:
        index += 1
    args = data[index:].split(b"\0", argc)
    if len(args) < argc + 1:
        raise GateRefusal("supervisor arguments truncated")
    return [part.decode("utf-8", errors="strict") for part in args[:argc]]


def process_executable(pid: int) -> Path:
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    buf = ctypes.create_string_buffer(4096)
    size = libproc.proc_pidpath(pid, buf, len(buf))
    if size <= 0 or size >= len(buf):
        raise GateRefusal("supervisor executable unavailable")
    return Path(os.fsdecode(buf.value)).resolve(strict=True)


def require_same_interpreter(parent_pid: int) -> None:
    """Compare the kernel's executable image for both processes."""
    if process_executable(parent_pid) != process_executable(os.getpid()):
        raise GateRefusal("parent binding: interpreter executable mismatch")


def resolve_parent_binding(path: str | Path, label: str) -> Path:
    try:
        return Path(path).resolve(strict=True)
    except (OSError, ValueError):
        raise GateRefusal(f"parent binding: {label} unavailable") from None


def supervisor_context() -> tuple:
    """Bind the direct parent and live supervisor receipt before any effect."""
    evidence = Path(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
    scratch = Path(os.environ.get("OXIDE_TASK_SCRATCH_ROOT", ""))
    run_id = os.environ.get("OXIDE_PROOF_RUN_ID", "")
    deadline = os.environ.get("OXIDE_PROOF_DEADLINE_NS", "")
    if (not re.fullmatch(r"[0-9a-f]{32}", run_id) or not deadline.isdigit()
            or not evidence.is_absolute() or not evidence.is_dir()
            or not scratch.is_absolute() or not scratch.is_dir()):
        raise GateRefusal("missing qualified supervisor context")
    me = process_row(os.getpid())
    parent = process_row(os.getppid())
    if (me["pid"] != os.getpid() or me["ppid"] != parent["pid"]
            or me["pgid"] != me["pid"] or me["zombie"] or parent["zombie"]):
        raise GateRefusal("not the supervisor's direct session leader")
    argv = process_argv(parent["pid"])
    if len(argv) < 8 or "--" not in argv:
        raise GateRefusal("supervisor command line is malformed")
    divider = argv.index("--")
    if divider < 6 or len(argv[divider + 1:]) < 2:
        raise GateRefusal("supervisor command is incomplete")
    supervisor = resolve_parent_binding(argv[1], "supervisor path")
    supervisor_flags = argv[2:divider]
    if len(supervisor_flags) not in (4, 6):
        raise GateRefusal("parent binding: supervisor flag count mismatch")
    if len(supervisor_flags) == 6:
        if supervisor_flags[4] != "--evidence":
            raise GateRefusal("parent binding: evidence flag mismatch")
        if resolve_parent_binding(supervisor_flags[5], "evidence path") != evidence.resolve(strict=True):
            raise GateRefusal("parent binding: evidence argument mismatch")
    require_same_interpreter(parent["pid"])
    if supervisor.name != "bounded-test.py":
        raise GateRefusal("parent binding: supervisor filename mismatch")
    try:
        supervisor_digest = sha256_file(supervisor, cap=512 * 1024)
    except (OSError, GateRefusal):
        raise GateRefusal("parent binding: supervisor digest unavailable") from None
    if supervisor_digest != SUPERVISOR_SHA256:
        raise GateRefusal("parent binding: supervisor digest mismatch")
    if argv[2] != "--limit":
        raise GateRefusal("parent binding: limit flag mismatch")
    if not argv[3].isdigit() or not 1 <= int(argv[3]) <= 1200:
        raise GateRefusal("parent binding: invalid limit value")
    if argv[4] != "--scratch-budget-mib":
        raise GateRefusal("parent binding: scratch flag mismatch")
    if not argv[5].isdigit() or not 1 <= int(argv[5]) <= 1024:
        raise GateRefusal("parent binding: invalid scratch budget")
    if resolve_parent_binding(argv[divider + 2], "child script") != resolve_parent_binding(sys.argv[0], "current script"):
        raise GateRefusal("parent binding: child script mismatch")
    if argv[divider + 3:] != sys.argv[1:]:
        raise GateRefusal("parent binding: child argument mismatch")
    receipt = json.loads((evidence / "receipt.json").read_bytes())
    if receipt.get("phase") != "started: incomplete":
        raise GateRefusal("parent binding: receipt phase mismatch")
    if receipt.get("run_id") != run_id:
        raise GateRefusal("parent binding: receipt run identity mismatch")
    if receipt.get("command") != argv[divider + 1:]:
        raise GateRefusal("parent binding: receipt command mismatch")
    if receipt.get("evidence") != str(evidence):
        raise GateRefusal("parent binding: receipt evidence path mismatch")
    if receipt.get("scratch") != str(scratch):
        raise GateRefusal("parent binding: receipt scratch path mismatch")
    if receipt.get("limit_s") != int(argv[3]):
        raise GateRefusal("parent binding: receipt limit mismatch")
    start = receipt.get("start_ns")
    if type(start) is not int:
        raise GateRefusal("parent binding: receipt start is invalid")
    if not start + int(argv[3]) * 1_000_000_000 <= int(deadline) <= start + (int(argv[3]) + 600) * 1_000_000_000:
        raise GateRefusal("parent binding: receipt deadline mismatch")
    if start > now_ns():
        raise GateRefusal("parent binding: receipt start is in the future")
    if process_row(parent["pid"]) != parent:
        raise GateRefusal("parent binding: parent identity changed")
    return evidence, run_id, int(deadline), scratch


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


def claim_issued_token(block_path: Path, expected_digest: str, run_id: str) -> dict:
    """Consume a planner-issued companion once, including across evidence roots.

    The fixed exclusive hard-link name is the irreversible claim; a crash at
    any point after it is created burns the token. Never remove it or retry.
    """
    parent = block_path.parent
    directory = os.lstat(parent)
    block = os.lstat(block_path)
    if (parent.resolve(strict=True) != parent or block_path.resolve(strict=True) != block_path
            or not stat.S_ISDIR(directory.st_mode) or directory.st_uid != os.geteuid()
            or directory.st_mode & 0o022 or not stat.S_ISREG(block.st_mode)
            or block.st_uid != os.geteuid() or block.st_mode & 0o022
            or sha256_file(block_path) != expected_digest):
        raise GateRefusal("issued block or parent is unsafe or changed")
    token = parent / (block_path.name + ".token")
    claimed = parent / (block_path.name + ".token.claimed")
    token_st = os.lstat(token)
    if (not stat.S_ISREG(token_st.st_mode) or token_st.st_uid != os.geteuid()
            or token_st.st_mode & 0o077 or token_st.st_nlink != 1
            or sha256_file(token, cap=65) != sha256_bytes((expected_digest + "\n").encode("ascii"))):
        raise GateRefusal("issued companion token is unsafe or not bound to the block")
    try:
        os.link(token, claimed, follow_symlinks=False)
    except FileExistsError:
        raise GateRefusal("issued block token was already claimed") from None
    directory_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)
    linked = os.lstat(claimed)
    if (linked.st_dev, linked.st_ino) != (token_st.st_dev, token_st.st_ino):
        raise GateRefusal("claimed token identity changed; claim remains burned")
    return {"block": str(block_path), "token": str(token), "claim": str(claimed),
            "run_id": run_id, "block_sha256": expected_digest,
            "block_identity": [block.st_dev, block.st_ino, block.st_mtime_ns, block.st_uid]}


def run_supervised(*, supervisor: Path, python: Path, command: list, cwd: Path, env: dict,
                   limit_seconds: int, scratch_mib: int, evidence: Path,
                   allowance: Allowance, phase: str, log_dir: Path | None = None) -> tuple:
    """One nested qualified-supervised child; its receipt must close cleanly."""
    if limit_seconds <= 30 or scratch_mib < 1:
        raise GateRefusal(f"{phase} has no wall or scratch allowance left")
    log_dir = log_dir or evidence.parent
    stdout_path = log_dir / (phase + "-stdout.log")
    stderr_path = log_dir / (phase + "-stderr.log")
    allowance.begin(phase)
    result = None
    try:
        with stdout_path.open("xb") as out, stderr_path.open("xb") as err:
            remaining = allowance.remaining_seconds()
            actual_limit = min(limit_seconds, remaining)
            if actual_limit <= 30:
                raise GateRefusal(f"{phase} has less than the supervisor wall floor")
            argv = [str(python), str(supervisor), "--limit", str(actual_limit),
                    "--scratch-budget-mib", str(scratch_mib), "--evidence", str(evidence),
                    "--", *map(str, command)]
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


def bind_outer_deadline(receipt: dict, supervisor_deadline: int, issued_remaining: int,
                        oxide_max: int) -> int:
    """Bind the qualified outer limit to both issued wall authorities."""
    limit, started = receipt.get("limit_s"), receipt.get("start_ns")
    # The qualified guard serializes --limit as a float. Preserve the receipt;
    # accept only an exact integral finite value for integer wall arithmetic.
    if type(limit) is float and math.isfinite(limit) and limit.is_integer():
        limit = int(limit)
    if (type(limit) is not int or type(started) is not int
            or type(supervisor_deadline) is not int or type(issued_remaining) is not int
            or type(oxide_max) is not int or limit <= 30
            or limit > issued_remaining or limit > oxide_max
            or started <= 0 or now_ns() >= supervisor_deadline):
        raise GateRefusal("outer supervisor limit exceeds issued remaining authority or floor")
    effective = min(supervisor_deadline, started + limit * 1_000_000_000,
                    started + issued_remaining * 1_000_000_000,
                    started + oxide_max * 1_000_000_000)
    if now_ns() >= effective:
        raise GateRefusal("outer supervisor effective deadline exhausted")
    return effective


def inactive_roots(ledger_roots: list, oxide_roots: list, barge_root: Path, oxide_root: Path) -> list:
    return [p for p in ledger_roots + oxide_roots if p not in (barge_root, oxide_root)]


def run_status_after_sentinels(create_sentinels, run_status):
    """No status invocation can precede successful sentinel preparation."""
    sentinels = create_sentinels()
    return sentinels, run_status()


# ---------------------------------------------------------------------------
# Bootstrap provenance (oxide#253): bounded, attributable evidence retained
# before the live-inventory assertion can refuse. Internal evidence only.
# ---------------------------------------------------------------------------

def live_account_reachability(values: list, *, account_root: Path) -> dict:
    """Whether any recorded allowed value could name the live account root."""
    prefix = str(account_root)
    hits: list = []
    seen: set = set()
    for value in values:
        raw = str(value)
        parts = [raw]
        if os.pathsep in raw:
            parts.extend(raw.split(os.pathsep))
        for text in parts:
            if not text or text in seen:
                continue
            if text == prefix or text.startswith(prefix + os.sep):
                seen.add(text)
                if len(hits) < 4:
                    hits.append(text[:512])
    return {"account_root": prefix, "state": "none" if not hits else "recorded",
            "values": hits}


def tool_identity(path: Path, cap: int, *, deadline_ns: int | None = None) -> dict:
    """A bounded live identity for one allowed build executable."""
    try:
        info = path.stat()
        digest = sha256_file(path, cap=cap, deadline_ns=deadline_ns)
    except OSError as error:
        raise GateRefusal(f"provenance tool unreadable: {path}: {error}") from None
    return {"path": str(path), "bytes": info.st_size, "sha256": digest}


def symlink_target(path: Path) -> str:
    """The recorded no-follow target of one private bin symlink."""
    try:
        return os.readlink(path)
    except OSError:
        return "absent"


def private_home_state_observation(private_root: Path) -> dict:
    """Observe the private home's own account-state file (never the live one).

    A product invocation under the scrubbed child environment would write the
    private path, not the live account; recording it makes that distinction
    decidable from retained evidence alone.
    """
    path = private_root / "home" / PRIVATE_HOME_STATE
    entry = {"path": str(path)}
    try:
        info = path.lstat()
    except FileNotFoundError:
        entry["exists"] = False
        return entry
    except OSError as error:
        entry.update({"exists": True, "state": "unreadable",
                      "error": error.__class__.__name__})
        return entry
    if not stat.S_ISREG(info.st_mode):
        entry.update({"exists": True, "state": "not-regular"})
        return entry
    try:
        entry.update({"exists": True, "bytes": info.st_size,
                      "sha256": sha256_file(path, cap=MAX_PRIVATE_STATE_BYTES)})
    except (OSError, GateRefusal) as error:
        entry.update({"exists": True, "state": "unreadable",
                      "error": error.__class__.__name__})
    return entry


def _observe_cargo_config(path: Path, *, inside: bool, kind: str,
                          entries: list, violations: list) -> int:
    """One no-follow Cargo config observation. Returns omitted count (0 or 1).

    Absence (ENOENT / ENOTDIR) is recorded with no violation. Any other
    lstat or digest error is unreadable: it is not a private binding and
    is not recorded as absent.
    """
    entry = {"path": str(path), "ambient": not inside}

    def retain():
        if len(entries) < MAX_PROVENANCE_ENTRIES:
            entries.append(entry)
            return 0
        return 1

    try:
        info = path.lstat()
    except FileNotFoundError:
        entry["state"] = "absent"
        return retain()
    except NotADirectoryError:
        entry["state"] = "absent"
        return retain()
    except OSError as error:
        entry["state"] = "unreadable"
        entry["error"] = error.__class__.__name__
        violations.append("unreadable-cargo-config:" + str(path)[:512])
        return retain()
    if not stat.S_ISREG(info.st_mode):
        entry["state"] = "not-regular"
        violations.append(kind + str(path)[:512])
        return retain()
    try:
        digest = sha256_file(path, cap=MAX_PROVENANCE_CONFIG_BYTES)
    except (OSError, GateRefusal) as error:
        entry["state"] = "unreadable"
        entry["error"] = error.__class__.__name__
        violations.append("unreadable-cargo-config:" + str(path)[:512])
        return retain()
    entry["state"] = "present"
    entry["bytes"] = info.st_size
    entry["sha256"] = digest
    violations.append(kind + str(path)[:512])
    return retain()


def ambient_cargo_config_scan(start: Path, *, private_root: Path,
                              cargo_home: Path | None = None,
                              deadline_ns: int | None = None) -> dict:
    """Observe Cargo configuration on the build cwd's ancestor chain and CARGO_HOME.

    Cargo discovers `.cargo/config.toml` from the working directory upward and
    then `$CARGO_HOME/config.toml`. A configuration above the private root, or
    a CARGO_HOME outside it, would be silently inherited ambient policy with no
    positive private binding. A present file inside the private tree is planted
    policy whose content this gate does not bind. Either finding refuses
    BEFORE the launch; the recorded observation is retained in the provenance
    packet. Absence is recorded. An unreadable path is a violation, not
    absence. PATH-resolved linker and assembler tools are a separate scan.
    """
    chain: list = []
    node = Path(start).resolve(strict=True)
    complete = False
    while len(chain) < MAX_ANCESTOR_LEVELS:
        deadline_check(deadline_ns)
        chain.append(node)
        parent = node.parent
        if parent == node:
            complete = True
            break
        node = parent
    entries: list = []
    violations: list = []
    inside_levels = 0
    omitted_entries = 0
    for node in chain:
        deadline_check(deadline_ns)
        inside = node == private_root or node.is_relative_to(private_root)
        if inside:
            inside_levels += 1
        kind = "ambient-cargo-config:" if not inside else "planted-cargo-config:"
        for name in (".cargo/config.toml", ".cargo/config"):
            omitted_entries += _observe_cargo_config(
                node / name, inside=inside, kind=kind,
                entries=entries, violations=violations)
    if cargo_home is not None:
        home = Path(cargo_home)
        inside = home == private_root or home.is_relative_to(private_root)
        kind = "ambient-cargo-config:" if not inside else "planted-cargo-home-config:"
        for name in ("config.toml", "config"):
            omitted_entries += _observe_cargo_config(
                home / name, inside=inside, kind=kind,
                entries=entries, violations=violations)
    return {"scan_root": str(Path(start).resolve(strict=True)), "levels": len(chain),
            "chain_complete": complete, "inside_private_levels": inside_levels,
            "cargo_home": None if cargo_home is None else str(cargo_home),
            "entries": entries, "omitted_entries": omitted_entries,
            "violations": violations}


def _path_inside_private(path: Path, private_root: Path) -> bool:
    try:
        return path == private_root or path.is_relative_to(private_root)
    except ValueError:
        return False


def _usable_regular_file(path: Path) -> bool:
    """True when path is an executable regular file (follows a final symlink)."""
    try:
        info = path.stat()
    except OSError:
        return False
    return stat.S_ISREG(info.st_mode) and os.access(path, os.X_OK)


def _classify_path_slot(candidate: Path, info: os.stat_result,
                        *, private_root: Path) -> dict | None:
    """Classify one PATH slot, or None so the search continues.

    A usable binding is an executable regular file. A symlink is usable
    only when its resolved target is an executable regular file. Private
    requires that usable file to sit inside the private root. A dangling
    symlink or non-executable slot is skipped (execvp continues). A
    symlink to a host executable is ambient, not private.
    """
    if stat.S_ISLNK(info.st_mode):
        try:
            raw_target = os.readlink(candidate)
        except OSError as error:
            return {"state": "unreadable", "path": str(candidate)[:512],
                    "error": error.__class__.__name__}
        try:
            resolved = candidate.resolve(strict=True)
        except OSError:
            return None
        if not _usable_regular_file(resolved):
            return None
        entry = {"path": str(candidate)[:512],
                 "resolved_path": str(resolved)[:512],
                 "symlink_target": str(raw_target)[:512]}
        entry["state"] = ("private" if _path_inside_private(resolved, private_root)
                          else "ambient")
        return entry
    if not stat.S_ISREG(info.st_mode):
        return None
    if not os.access(candidate, os.X_OK):
        return None
    return {"path": str(candidate)[:512],
            "state": ("private" if _path_inside_private(candidate, private_root)
                      else "ambient")}


def path_executable_scan(path_value: str, *, private_root: Path,
                         cwd: Path | None = None, deadline_ns: int | None = None) -> dict:
    """Resolve named linker/assembler tools on the child PATH.

    A private binding is an executable regular file inside the private
    root, or a symlink whose resolved target is an executable regular
    file inside the private root. A first-match usable slot outside the
    private root, or an unreadable slot, is an ambient executable path:
    the existing pre-launch refusal records it. A dangling symlink or
    non-executable file is skipped so a later host match can refuse.
    Absence is recorded and is not a violation. An empty PATH component
    is the child cwd (POSIX). Proc-macro and build-script crates compile
    to ``target/`` under the private root and inherit this PATH; unnamed
    further host spawns cannot be bound without executing the bootstrap.
    """
    components = str(path_value).split(os.pathsep)
    entries: list = []
    violations: list = []
    omitted_entries = 0
    for name in BOOTSTRAP_PATH_TOOLS:
        deadline_check(deadline_ns)
        entry = {"name": name, "state": "absent"}
        for component in components:
            deadline_check(deadline_ns)
            if component == "":
                if cwd is None:
                    continue
                candidate = Path(cwd) / name
            else:
                candidate = Path(component) / name
            try:
                info = candidate.lstat()
            except FileNotFoundError:
                continue
            except NotADirectoryError:
                continue
            except OSError as error:
                entry = {"name": name, "path": str(candidate)[:512],
                         "state": "unreadable", "error": error.__class__.__name__}
                violations.append("unreadable-executable:" + name + ":"
                                  + str(candidate)[:512])
                break
            classified = _classify_path_slot(candidate, info,
                                             private_root=private_root)
            if classified is None:
                continue
            entry = {"name": name, **classified}
            if classified["state"] == "ambient":
                violations.append("ambient-executable:" + name + ":"
                                  + classified["path"])
            elif classified["state"] == "unreadable":
                violations.append("unreadable-executable:" + name + ":"
                                  + classified["path"])
            break
        if len(entries) < MAX_PROVENANCE_ENTRIES:
            entries.append(entry)
        else:
            omitted_entries += 1
    return {"entries": entries, "omitted_entries": omitted_entries,
            "violations": violations}


def attach_path_executables(ambient: dict, path_value: str,
                            *, private_root: Path,
                            cwd: Path | None = None, deadline_ns: int | None = None) -> dict:
    """Merge the PATH-tool scan into the pre-launch ambient findings."""
    scan = path_executable_scan(path_value, private_root=private_root, cwd=cwd, deadline_ns=deadline_ns)
    bound = dict(ambient)
    bound["path_executables"] = scan
    bound["violations"] = list(ambient.get("violations", [])) + list(scan["violations"])
    return bound


# ---------------------------------------------------------------------------
# Candidate route (oxide#254): retained-artifact bindings and audited host tools
# ---------------------------------------------------------------------------

def validate_host_tools_document(document: dict, *, rustc_tool: Path,
                                 cargo_tool: Path, deadline_ns: int | None = None) -> dict:
    """Validate the planner-issued audited host-input bindings.

    The document is the reviewed input operator ruling d9aec625 authorizes for
    the candidate route: the pinned 1.97.1 rustc/cargo pair and any subset of
    the named linker/assembler tools that actually resolves on the child PATH.
    Every binding names one canonical executable regular file plus its live
    content hash and byte size; the live files must still match. Absent tools
    are omitted (absence is never listed). These are audited HOST inputs, not
    private bindings, and the hashes do not confine the tools' writes.
    """
    if (not isinstance(document, dict) or document.get("schema") != SCHEMA_HOST_TOOLS
            or document.get("route") != "candidate"
            or document.get("ruling") != CANDIDATE_RULING
            or set(document) != {"schema", "route", "ruling", "toolchain", "tools"}):
        raise GateRefusal("host-tools document is not the candidate-route ruling binding")
    toolchain = document.get("toolchain")
    if not isinstance(toolchain, dict) or set(toolchain) != {"rustc", "cargo"}:
        raise GateRefusal("host-tools document must bind the pinned toolchain pair")
    for name, live in (("rustc", rustc_tool), ("cargo", cargo_tool)):
        deadline_check(deadline_ns)
        entry = toolchain[name]
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "bytes"}:
            raise GateRefusal(f"host-tools toolchain {name} binding is malformed")
        path = Path(entry["path"])
        if (not path.is_absolute() or path.is_symlink() or path.resolve(strict=True) != path
                or path != live or not path.is_file()):
            raise GateRefusal(f"host-tools toolchain {name} does not bind the live executable")
        info = path.stat()
        if (type(entry["bytes"]) is not int or entry["bytes"] <= 0
                or entry["bytes"] != info.st_size
                or not isinstance(entry["sha256"], str) or not HEX64.fullmatch(entry["sha256"])
                or sha256_file(path, cap=256 * 1024 * 1024, deadline_ns=deadline_ns) != entry["sha256"]):
            raise GateRefusal(f"host-tools toolchain {name} digest or size changed")
    tools = document.get("tools")
    if not isinstance(tools, list):
        raise GateRefusal("host-tools tool list is missing")
    names: list = []
    for entry in tools:
        deadline_check(deadline_ns)
        if not isinstance(entry, dict) or set(entry) != {"name", "path", "sha256", "bytes"}:
            raise GateRefusal("host-tools entry is malformed")
        name = entry["name"]
        if not isinstance(name, str) or name not in BOOTSTRAP_PATH_TOOLS:
            raise GateRefusal(f"host-tools entry names an unlisted tool: {name!r}")
        if name in names:
            raise GateRefusal(f"host-tools entry duplicates {name}")
        names.append(name)
        path = Path(entry["path"])
        if (not path.is_absolute() or path.is_symlink() or path.resolve(strict=True) != path
                or not path.is_file()):
            raise GateRefusal(f"host-tools {name} binding is not a canonical file")
        info = path.stat()
        if (type(entry["bytes"]) is not int or entry["bytes"] <= 0
                or entry["bytes"] > MAX_ADMISSION_ARTIFACT_BYTES or entry["bytes"] != info.st_size
                or not isinstance(entry["sha256"], str) or not HEX64.fullmatch(entry["sha256"])
                or sha256_file(path, cap=MAX_ADMISSION_ARTIFACT_BYTES, deadline_ns=deadline_ns) != entry["sha256"]):
            raise GateRefusal(f"host-tools {name} digest or size changed")
        if not os.access(path, os.X_OK):
            raise GateRefusal(f"host-tools {name} binding is not executable")
    return {"schema": SCHEMA_HOST_TOOLS, "ruling": CANDIDATE_RULING,
            "toolchain": {"rustc": str(rustc_tool), "cargo": str(cargo_tool)},
            "tools": sorted(names) or None}


def reconcile_host_tools(ambient: dict, document: dict, *, deadline_ns: int | None = None) -> tuple:
    """Adjudicate the PATH-tool scan into the candidate route's final findings.

    Takes the merged pre-launch ambient dict (its `path_executables` scan and
    its raw `violations`) and the audited host-input bindings. Returns
    (observation, final_violations): the raw scan's `ambient-executable:`
    findings are REPLACED by this document's verdict — a first-match usable
    slot outside the private root is accepted only when the document binds
    exactly that resolved path with exactly that live content hash and size;
    a documented tool that is absent, changed or resolved elsewhere refuses,
    and a live ambient match with no documented binding refuses. Every other
    raw finding (ambient or planted Cargo configuration, unreadable
    executables) is carried through unchanged, so an unreadable slot still
    refuses and a private binding stays allowed: the private-tool requirement
    is replaced only for the reviewed host bindings, never disabled globally.
    Absent and undocumented stays absent.
    """
    if (not isinstance(document, dict) or document.get("schema") != SCHEMA_HOST_TOOLS):
        raise GateRefusal("host-tools document is required for the candidate route")
    scan = ambient.get("path_executables", {}) if isinstance(ambient, dict) else {}
    bound: dict = {}
    for entry in document.get("tools", []):
        if isinstance(entry, dict) and isinstance(entry.get("name"), str):
            bound[entry["name"]] = entry
    observation: list = []
    adjudicated: list = []
    for entry in scan.get("entries", []):
        name = entry.get("name")
        state = entry.get("state")
        recorded = {"name": name, "state": state}
        if state == "ambient":
            resolved = Path(entry.get("resolved_path") or entry.get("path", ""))
            binding = bound.get(name)
            try:
                live_sha256 = sha256_file(resolved, cap=MAX_ADMISSION_ARTIFACT_BYTES, deadline_ns=deadline_ns)
                live_bytes = resolved.stat().st_size
            except (OSError, GateRefusal) as error:
                recorded["state"] = "unreadable"
                recorded["error"] = error.__class__.__name__
                adjudicated.append("unreadable-host-tool:" + name)
            else:
                if (binding is None or Path(binding["path"]) != resolved
                        or binding["sha256"] != live_sha256 or binding["bytes"] != live_bytes):
                    recorded["bound"] = bool(binding)
                    adjudicated.append("unbound-ambient-executable:" + name + ":"
                                       + str(resolved)[:512])
                else:
                    recorded.update({"resolved_path": str(resolved), "sha256": live_sha256,
                                     "bytes": live_bytes,
                                     "note": "audited host input; not private; hash binds "
                                             "admission identity and does not confine writes"})
        elif state == "private":
            recorded["resolved_path"] = entry.get("resolved_path") or entry.get("path")
        # A scan-unreadable slot already carries its own raw
        # `unreadable-executable:` finding, kept below; no second verdict.
        if len(observation) < MAX_PROVENANCE_ENTRIES:
            observation.append(recorded)
    documented_names = set(bound)
    live_names = {entry.get("name") for entry in scan.get("entries", [])
                  if entry.get("state") == "ambient"}
    stale = sorted(documented_names - live_names)
    if stale:
        adjudicated.append("documented-host-tool-not-live:" + ",".join(stale[:8]))
    # The candidate route's violations: every raw finding except the
    # ambient-executable ones this document adjudicates, then the verdicts.
    violations = [finding for finding in ambient.get("violations", [])
                  if isinstance(finding, str)
                  and not finding.startswith("ambient-executable:")] + adjudicated
    return observation, violations


def validate_candidate_issuance(block: dict, audit: dict, candidate_sha256: str) -> str:
    """The candidate route takes complete issued documents, not derivations.

    The derivation rule (`derived-artifact-v1:` plus JSON-null artifact and
    audit digests) authorizes filling those fields from one supervised
    bootstrap build. The candidate route runs no build, so its issued block
    and audit must already name the retained candidate digest and the audit
    digest; anything deferred, null, or derivation-referenced refuses.
    """
    if (not isinstance(block, dict) or not isinstance(audit, dict)
            or not isinstance(candidate_sha256, str) or not HEX64.fullmatch(candidate_sha256)):
        raise GateRefusal("candidate issuance inputs are malformed")
    for name, document in (("block", block), ("audit", audit)):
        if document.get("oxide_sha256") != candidate_sha256:
            raise GateRefusal(f"candidate {name} does not name the retained candidate digest")
    reference = block.get("authorization_reference")
    if (not isinstance(reference, str) or not reference.strip()
            or reference.startswith("derived-artifact-v1:")):
        raise GateRefusal("candidate route requires a non-derivation authorization reference")
    return sha256_bytes(canonical_json(audit))


def validate_candidate_origin(*, origin_root: Path, options: dict, ledger_roots: list,
                              pinned_lock_sha256: str,
                              pinned_toolchain_sha256: str, deadline_ns: int | None = None) -> dict:
    """Validate the retained candidate's origin, bindings and nested closure.

    Reads only: the origin root must be a canonical directory under Barge
    scratch, distinct from every active root and covered by the retained-root
    ledger; the retained nested guard receipt must prove the fixed offline
    build closed with a certain empty census in THIS root; the retained build
    log must name exactly one pinned-main artifact resolving to the canonical
    retained path inside the origin root; and the artifact, lock and
    toolchain bytes must equal the planner bindings, which must equal the
    pinned commit's own bytes. Nothing under the origin is written; a
    mismatch refuses before any staging.
    """
    deadline_check(deadline_ns)
    try:
        origin_root.resolve(strict=True)
    except OSError as error:
        raise GateRefusal(f"candidate origin root is unavailable: {error}") from None
    if (origin_root.resolve(strict=True) != origin_root or origin_root.is_symlink()
            or not origin_root.is_dir()):
        raise GateRefusal("candidate origin root must be a canonical directory")
    covered = any(origin_root == root or origin_root.is_relative_to(root) for root in ledger_roots)
    if not covered:
        raise GateRefusal("candidate origin root is not covered by the retained-root ledger")
    expected_lock = options["candidate_lock_sha256"]
    expected_toolchain = options["candidate_toolchain_sha256"]
    if expected_lock != pinned_lock_sha256 or expected_toolchain != pinned_toolchain_sha256:
        raise GateRefusal("candidate lock or toolchain binding differs from the pinned source")
    receipt_path = Path(options["candidate_receipt"])
    receipt = read_json_bound(receipt_path, options["expect_candidate_receipt"])
    validate_supervisor_receipt(receipt, list(BARGE_BOOTSTRAP_ARGV), origin_root)
    log_path = Path(options["candidate_log"])
    try:
        log_stat = os.lstat(log_path)
    except OSError as error:
        raise GateRefusal(f"retained build log is unavailable: {error}") from None
    if not stat.S_ISREG(log_stat.st_mode) or log_stat.st_size > MAX_CANDIDATE_LOG_BYTES:
        raise GateRefusal("retained build log is not a bounded regular file")
    if sha256_file(log_path, cap=MAX_CANDIDATE_LOG_BYTES, deadline_ns=deadline_ns) != options["expect_candidate_log"]:
        raise GateRefusal("retained build log does not match its bound digest")
    log_text = log_path.read_text(encoding="utf-8", errors="strict")
    facts = bootstrap_artifact_facts(log_text, oxide_source=origin_root / "oxide-source",
                                     private_root=origin_root, deadline_ns=deadline_ns)
    if facts["violations"]:
        raise GateRefusal(artifact_refusal_message(facts))
    artifact = facts["artifact"]
    canonical_artifact = origin_root / CANDIDATE_ORIGIN_RELATIVE
    if Path(artifact["resolved_path"]) != canonical_artifact:
        raise GateRefusal("retained build log does not name the canonical retained artifact path")
    if artifact["sha256"] != options["expect_candidate"]:
        raise GateRefusal("retained artifact digest differs from the bound candidate digest")
    if type(options["candidate_bytes"]) is not int or options["candidate_bytes"] <= 0 \
            or artifact["bytes"] != options["candidate_bytes"]:
        raise GateRefusal("retained artifact size differs from the bound candidate size")
    origin_lock = origin_root / "oxide-source/Cargo.lock"
    origin_toolchain = origin_root / "oxide-source/rust-toolchain.toml"
    try:
        if sha256_file(origin_lock, cap=MAX_DOC_BYTES, deadline_ns=deadline_ns) != expected_lock:
            raise GateRefusal("origin Cargo.lock differs from the bound candidate source")
        if sha256_file(origin_toolchain, cap=MAX_DOC_BYTES, deadline_ns=deadline_ns) != expected_toolchain:
            raise GateRefusal("origin rust-toolchain.toml differs from the bound candidate source")
    except GateRefusal:
        raise
    except OSError as error:
        raise GateRefusal(f"origin source binding is unreadable: {error}") from None
    census = receipt.get("census", {})
    outcome = receipt.get("outcome", {})
    return {
        "root": str(origin_root),
        "artifact": {"path": str(canonical_artifact), "sha256": artifact["sha256"],
                     "bytes": artifact["bytes"]},
        "lock_sha256": expected_lock, "toolchain_sha256": expected_toolchain,
        "guard": {"run_id": receipt.get("run_id"), "limit_s": receipt.get("limit_s"),
                  "start_ns": receipt.get("start_ns"), "published_ns": receipt.get("published_ns"),
                  "closure": outcome.get("closure"), "status": outcome.get("status"),
                  "census_certain": census.get("certain"), "census_members": census.get("members"),
                  "census_uncertain": census.get("uncertain"),
                  "argv_matches": receipt.get("command") == list(BARGE_BOOTSTRAP_ARGV),
                  "receipt_path": str(receipt_path),
                  "receipt_sha256": options["expect_candidate_receipt"],
                  "log_path": str(log_path), "log_sha256": options["expect_candidate_log"]},
        "note": "retained evidence from the spent replacement diagnostic; the artifact is "
                "not qualified by existing and that session's failed isolation stays "
                "historical and unrepaired",
    }


def copy_candidate_artifact(source: Path, fresh_root: Path, expected_sha256: str,
                            expected_bytes: int, *, deadline_ns: int | None = None) -> dict:
    """Copy the retained candidate into the fresh guard-owned root, rehash it.

    The source must be the planner-digest-bound regular executable file
    validated by validate_candidate_origin; the copy is created owner-only,
    owner-executable (0o700, so `bin/oxide` can actually exec it) and
    exclusive, rehashed in full, mode-checked, and both endpoint identities
    are re-checked after the transfer so a same-name swap, truncation or
    in-place change during the copy refuses. The origin tree is never
    written.
    """
    try:
        before = os.lstat(source)
    except OSError as error:
        raise GateRefusal(f"candidate source vanished before the copy: {error}") from None
    if not stat.S_ISREG(before.st_mode):
        raise GateRefusal("candidate source is not a regular file (symlink swap refused)")
    if not os.access(source, os.X_OK):
        raise GateRefusal("candidate origin artifact is not executable")
    relative = Path("candidate") / CANDIDATE_ORIGIN_RELATIVE.name
    try:
        candidate_dir = private_dir(fresh_root, Path("candidate"))
        destination = fresh_root / relative
        fd = os.open(candidate_dir, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except GateRefusal:
        raise
    except OSError as error:
        raise GateRefusal(f"candidate copy destination is unavailable: {error}") from None
    out = None
    try:
        out = os.open(relative.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                      0o700, dir_fd=fd)
        source_fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            opened = os.fstat(source_fd)
            if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns) != \
                    (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns):
                raise GateRefusal("candidate source changed identity before the copy")
            digest = hashlib.sha256()
            copied = 0
            while True:
                deadline_check(deadline_ns)
                block = os.read(source_fd, 1024 * 1024)
                if not block:
                    break
                copied += len(block)
                if copied > MAX_ADMISSION_ARTIFACT_BYTES:
                    raise GateRefusal("candidate source exceeds the artifact bound mid-copy")
                view = memoryview(block)
                while view:
                    written = os.write(out, view)
                    view = view[written:]
            after = os.fstat(source_fd)
            if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != \
                    (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns):
                raise GateRefusal("candidate source changed identity during the copy")
        finally:
            os.close(source_fd)
    except FileExistsError:
        raise GateRefusal("candidate copy destination already exists") from None
    except OSError as error:
        raise GateRefusal(f"candidate copy failed: {error}") from None
    finally:
        os.close(fd)
        if out is not None:
            try:
                os.close(out)
            except OSError:
                pass
    copy_digest = sha256_file(destination, cap=MAX_ADMISSION_ARTIFACT_BYTES, deadline_ns=deadline_ns)
    if copy_digest != expected_sha256:
        raise GateRefusal("candidate copy digest differs from the retained candidate")
    copy_stat = os.lstat(destination)
    if copy_stat.st_size != expected_bytes:
        raise GateRefusal("candidate copy size differs from the retained candidate")
    if not stat.S_IMODE(copy_stat.st_mode) & 0o100 or copy_stat.st_mode & 0o077:
        raise GateRefusal("candidate copy is not an owner-only executable")
    try:
        final = os.lstat(source)
    except OSError as error:
        raise GateRefusal(f"candidate source vanished after the copy: {error}") from None
    if (final.st_dev, final.st_ino, final.st_size, final.st_mtime_ns) != \
            (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns):
        raise GateRefusal("candidate source changed identity after the copy")
    return {"origin_path": str(source), "origin_identity": [before.st_dev, before.st_ino],
            "origin_mode": stat.S_IMODE(before.st_mode),
            "path": str(destination), "bytes": copy_stat.st_size,
            "sha256": copy_digest, "mode": stat.S_IMODE(copy_stat.st_mode)}


def build_candidate_provenance(inputs: dict, *, origin: dict, guard: dict, artifact: dict,
                               ambient: dict, allowance: dict, written_ns: int) -> dict:
    """Assemble the bounded candidate provenance document (internal evidence).

    Mirrors the bootstrap provenance packet's rule (oxide#253): gathered from
    the audited allowlist actually passed, retained inside the monitored root
    BEFORE the status gate can refuse, bounded in bytes, and never recording
    inherited secret values. The audited host inputs are recorded with their
    bound digests and labelled as host tools, never as private bindings.
    """
    private_root = Path(inputs["private_root"])
    cwd = Path(inputs["cwd"])
    if not (cwd == private_root or cwd.is_relative_to(private_root)):
        raise GateRefusal(f"provenance cwd is not under the private root: {cwd}")
    env = inputs["env"]
    if not isinstance(env, dict) or set(env) - BOOTSTRAP_ENV_KEYS:
        raise GateRefusal("provenance environment carries keys outside the "
                          "audited allowlist: " +
                          ",".join(sorted(set(env) - BOOTSTRAP_ENV_KEYS)[:8]))
    inherited_names = list(inputs["inherited_keys"])
    reachability = live_account_reachability(
        [env[key] for key in sorted(env)] + [str(cwd)],
        account_root=inputs["account_root"])
    return {
        "kind": "candidate-provenance",
        "written_ns": written_ns,
        "gate_run_id": inputs["run_id"],
        "ruling": CANDIDATE_RULING,
        "origin": origin,
        "command": {
            "argv": list(inputs["argv"]),
            "argv_sha256": sha256_bytes(canonical_json(list(inputs["argv"]))),
            "cwd": str(cwd),
        },
        "child_environment": {
            "allowed_keys": sorted(env),
            "values": {key: env[key] for key in sorted(env)},
            "inherited_key_names": inherited_names[:MAX_PROVENANCE_ENTRIES],
            "inherited_key_count": len(inherited_names),
            "live_account_reachability": reachability,
        },
        "admission": {"hook": env.get("EFFIGY_ADMISSION_HOOK", ""),
                      "socket": env.get("QUEUE_ADMISSION_SOCKET", "")},
        "tools": inputs["tools"],
        "audited_host_tools": inputs["host_tools"],
        "bin_symlinks": {
            "cargo": symlink_target(private_root / "bin/cargo"),
            "effigy": symlink_target(private_root / "bin/effigy"),
            "oxide": symlink_target(private_root / "bin/oxide"),
        },
        "sources": inputs["sources"],
        "ambient": ambient,
        "bounds": {"limit_seconds": inputs["limit_seconds"],
                   "scratch_mib": inputs["scratch_mib"],
                   "pool_cap_bytes": inputs["pool_cap_bytes"]},
        "guard": guard,
        "artifact": artifact,
        "allowance": allowance,
        "private_home_state": private_home_state_observation(private_root),
    }


def write_candidate_provenance(private_root: Path, document: dict) -> Path:
    """Retain the candidate packet before any later refusal (oxide#253 rule)."""
    data = canonical_json(document)
    if len(data) > MAX_CANDIDATE_PROVENANCE_BYTES:
        raise GateRefusal(f"candidate provenance packet exceeds its "
                          f"{MAX_CANDIDATE_PROVENANCE_BYTES}-byte bound; nothing retained")
    return private_write(private_root, CANDIDATE_PROVENANCE_PATH, data)


def refuse_candidate_launch(inputs: dict, ambient: dict, origin: dict, allowance: dict) -> None:
    """Refuse before the status gate when the pre-launch scan recorded a finding.

    Covers ambient or planted Cargo configuration, an unreadable config path,
    and a PATH-resolved named tool without an exact audited host-input binding
    (or without a private binding). The candidate packet is retained first.
    """
    if not ambient["violations"]:
        return
    document = build_candidate_provenance(
        inputs, origin=origin, guard={"state": "not-launched"},
        artifact={"state": "not-launched"},
        ambient=ambient, allowance=allowance, written_ns=now_ns())
    write_candidate_provenance(inputs["private_root"], document)
    raise GateRefusal("ambient configuration or executable path lacks a private "
                      "or audited host binding: " + ambient["violations"][0])


def artifact_refusal_message(facts: dict) -> str:
    """The bounded refusal text for the recorded artifact findings."""
    for kind in facts["violations"]:
        if kind.startswith("artifact-count:"):
            return "the bootstrap log does not name exactly one oxide artifact"
        if kind.startswith("artifact-escaped:"):
            return "bootstrap artifact escaped the private root: " + kind.split(":", 1)[1]
        if kind.startswith("artifact-missing:"):
            return "bootstrap artifact is missing or unreadable: " + kind.split(":", 1)[1]
        if kind.startswith("artifact-oversize:"):
            return "bootstrap artifact exceeds the digest bound: " + kind.split(":", 1)[1]
    return "bootstrap artifact could not be resolved"


def bootstrap_artifact_facts(log_text: str, *, oxide_source: Path,
                             private_root: Path, deadline_ns: int | None = None) -> dict:
    """Resolve the bootstrap artifact from the build log with bounded findings.

    The accepted single-artifact rule is unchanged: exactly one
    compiler-artifact message for the pinned `src/bin/oxide/main.rs`, a `bin`
    target, an executable path inside the private root. Malformed log lines
    are counted and retained — never silently accepted, never a crash.
    """
    main_rs = str(oxide_source / "src/bin/oxide/main.rs")
    candidates: list = []
    oxid_entries: list = []
    candidate_count = 0
    malformed_lines = 0
    truncated = False
    for line in log_text.splitlines():
        deadline_check(deadline_ns)
        try:
            item = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            malformed_lines += 1
            continue
        if not isinstance(item, dict):
            malformed_lines += 1
            continue
        target = item.get("target", {})
        if (item.get("reason") == "compiler-artifact" and isinstance(target, dict)
                and target.get("name") == "oxide" and "bin" in target.get("kind", [])
                and item.get("executable")):
            entry = {"src_path": str(target.get("src_path", "")),
                     "path": str(item["executable"])[:512]}
            if len(oxid_entries) < 4:
                oxid_entries.append(entry)
            if entry["src_path"] == main_rs:
                candidate_count += 1
                if len(candidates) < MAX_PROVENANCE_ENTRIES:
                    candidates.append(entry)
                else:
                    truncated = True
    violations: list = []
    if candidate_count != 1:
        violations.append(f"artifact-count:{candidate_count}")
        if candidate_count == 0 and oxid_entries:
            violations.append("artifact-src-path-mismatch")
    if truncated:
        violations.append("artifact-candidates-truncated")
    artifact = None
    if candidate_count == 1:
        raw = candidates[0]["path"]
        try:
            staged = Path(raw).resolve(strict=True)
        except OSError:
            violations.append(f"artifact-missing:{raw}")
        else:
            if not staged.is_relative_to(private_root):
                violations.append(f"artifact-escaped:{str(staged)[:512]}")
            else:
                try:
                    digest = sha256_file(staged, cap=256 * 1024 * 1024, deadline_ns=deadline_ns)
                except GateRefusal as error:
                    deadline_check(deadline_ns)
                    kind = ("artifact-oversize:" if "exceeds the document bound"
                            in str(error) else "artifact-missing:")
                    violations.append(kind + raw)
                except OSError:
                    violations.append(f"artifact-missing:{raw}")
                else:
                    artifact = {"path": raw, "resolved_path": str(staged),
                                "bytes": staged.stat().st_size,
                                "sha256": digest, "src_path": main_rs}
    return {"main_rs_binding": main_rs, "candidates": candidates,
            "candidate_count": candidate_count, "oxide_bin_entries": oxid_entries,
            "malformed_lines": malformed_lines, "violations": violations,
            "artifact": artifact}


def build_bootstrap_provenance(inputs: dict, *, guard: dict, artifact: dict,
                               ambient: dict, allowance: dict,
                               written_ns: int) -> dict:
    """Assemble the bounded bootstrap provenance document (internal evidence).

    Records only what was actually passed to the child: the scrubbed
    allowlist environment and its exact key set, exact argv, cwd, resolved
    tool identities, source and lock bindings, the ambient-configuration
    scan, the nested guard receipt reference and the resolved artifact.
    Pre-scrub inherited environment VALUES are never recorded; only their
    key names document the scrub. Construction facts (argv, allowlist,
    private cwd) refuse on drift; live findings are recorded and judged at
    the call site.
    """
    private_root = Path(inputs["private_root"])
    cwd = Path(inputs["cwd"])
    if not (cwd == private_root or cwd.is_relative_to(private_root)):
        raise GateRefusal(f"provenance cwd is not under the private root: {cwd}")
    if list(inputs["argv"]) != list(BARGE_BOOTSTRAP_ARGV):
        raise GateRefusal("provenance argv is not the fixed bootstrap argv")
    env = inputs["env"]
    if not isinstance(env, dict) or set(env) - BOOTSTRAP_ENV_KEYS:
        raise GateRefusal("provenance environment carries keys outside the "
                          "audited allowlist: " +
                          ",".join(sorted(set(env) - BOOTSTRAP_ENV_KEYS)[:8]))
    inherited_names = list(inputs["inherited_keys"])
    reachability = live_account_reachability(
        [env[key] for key in sorted(env)] + [str(cwd)],
        account_root=inputs["account_root"])
    return {
        "kind": "bootstrap-provenance",
        "written_ns": written_ns,
        "gate_run_id": inputs["run_id"],
        "command": {
            "argv": list(inputs["argv"]),
            "argv_sha256": sha256_bytes(canonical_json(list(inputs["argv"]))),
            "cwd": str(cwd),
        },
        "child_environment": {
            "allowed_keys": sorted(env),
            "values": {key: env[key] for key in sorted(env)},
            "inherited_key_names": inherited_names[:MAX_PROVENANCE_ENTRIES],
            "inherited_key_count": len(inherited_names),
            "live_account_reachability": reachability,
        },
        "admission": {"hook": env.get("EFFIGY_ADMISSION_HOOK", ""),
                      "socket": env.get("QUEUE_ADMISSION_SOCKET", "")},
        "tools": inputs["tools"],
        "bin_symlinks": {
            "cargo": symlink_target(private_root / "bin/cargo"),
            "effigy": symlink_target(private_root / "bin/effigy"),
            "oxide": symlink_target(private_root / "bin/oxide"),
        },
        "sources": inputs["sources"],
        "seed": inputs["seed"],
        "ambient": ambient,
        "bounds": {"limit_seconds": inputs["limit_seconds"],
                   "scratch_mib": inputs["scratch_mib"],
                   "pool_cap_bytes": inputs["pool_cap_bytes"]},
        "guard": guard,
        "artifact": artifact,
        "allowance": allowance,
        "private_home_state": private_home_state_observation(private_root),
    }


def write_bootstrap_provenance(private_root: Path, document: dict) -> Path:
    """Retain the packet inside the monitored root before any later refusal."""
    data = canonical_json(document)
    if len(data) > MAX_PROVENANCE_BYTES:
        raise GateRefusal(f"provenance packet exceeds its "
                          f"{MAX_PROVENANCE_BYTES}-byte bound; nothing retained")
    return private_write(private_root, PROVENANCE_PATH, data)


def refuse_bootstrap_launch(inputs: dict, ambient: dict, allowance: dict) -> None:
    """Refuse before the launch when the ambient scan recorded a finding.

    Covers ambient or planted Cargo configuration, an unreadable config
    path, and a PATH-resolved named linker/assembler without a private
    PATH-slot binding. The packet is retained first.
    """
    if not ambient["violations"]:
        return
    document = build_bootstrap_provenance(
        inputs, guard={"state": "not-launched"}, artifact={"state": "not-built"},
        ambient=ambient, allowance=allowance, written_ns=now_ns())
    write_bootstrap_provenance(inputs["private_root"], document)
    raise GateRefusal("ambient configuration or executable path lacks a "
                      "private binding: " + ambient["violations"][0])


def resolve_bootstrap_artifact(log_text: str, inputs: dict, *, guard: dict,
                               ambient: dict, allowance: dict, deadline_ns: int | None = None) -> dict:
    """Resolve the artifact, retain provenance first, refuse on any finding.

    On the success path nothing is written here: the diagnostic completes the
    packet once, after the guarded closure, still before the inventory
    assertion. On any artifact finding the packet is retained first so a real
    validation failure keeps the attributable bootstrap evidence.
    """
    facts = bootstrap_artifact_facts(log_text, oxide_source=inputs["cwd"],
                                     private_root=inputs["private_root"], deadline_ns=deadline_ns)
    if facts["violations"]:
        document = build_bootstrap_provenance(
            inputs, guard=guard,
            artifact={"state": "unresolved", "findings": facts},
            ambient=ambient, allowance=allowance, written_ns=now_ns())
        write_bootstrap_provenance(inputs["private_root"], document)
        raise GateRefusal(artifact_refusal_message(facts))
    return facts


def bootstrap_guard_facts(receipt: dict, *, receipt_path: Path, log_path: Path,
                          limit_seconds: int, scratch_mib: int,
                          expected_argv: list, deadline_ns: int | None = None) -> dict:
    """The bounded reference to the nested guard receipt and build log."""
    census = receipt.get("census", {})
    outcome = receipt.get("outcome", {})
    return {
        "state": "closed",
        "receipt_path": str(receipt_path),
        "receipt_sha256": sha256_file(receipt_path, cap=MAX_DOC_BYTES, deadline_ns=deadline_ns),
        "run_id": receipt.get("run_id"),
        "limit_s": receipt.get("limit_s"),
        "start_ns": receipt.get("start_ns"),
        "published_ns": receipt.get("published_ns"),
        "limit_seconds": limit_seconds,
        "scratch_mib": scratch_mib,
        "argv_matches_command": receipt.get("command") == list(expected_argv),
        "census_certain": census.get("certain"),
        "census_members": census.get("members"),
        "census_uncertain": census.get("uncertain"),
        "outcome": {key: outcome.get(key) for key in
                    ("closure", "status", "fault", "deadline", "launched")},
        "log_path": str(log_path),
        "log_sha256": sha256_file(log_path, cap=32 * 1024 * 1024, deadline_ns=deadline_ns),
    }


def diagnostic(options: dict) -> int:
    """The planner's one proofExecution: refuse before any effect on any gap.

    `--diagnostic` runs the fixed supervised bootstrap and derives the issued
    documents from the built artifact. `--candidate` (oxide#254, operator
    ruling d9aec625) runs the same guarded preparation, issuance, accounting,
    private status gate and isolation brackets, but binds the retained debug
    artifact from the spent replacement diagnostic instead of building: its
    origin, source/lock/toolchain/artifact binding and retained nested closure
    are validated before any staging, the artifact is copied only inside the
    fresh guard-owned root and rehashed, the named linker/assembler tools and
    the pinned toolchain are audited host inputs under the planner-bound
    host-tools document, and no wrapper-consumable derivation happens.
    """
    mode = "candidate" if options["mode"] == "candidate" else "bootstrap"
    python = Path(sys.executable).resolve(strict=True)
    evidence, run_id, deadline_ns, adopted_scratch = supervisor_context()
    allowance_dir = evidence / "private-build-validation.allowance"
    barge_root = root_guard(options["barge_root"], BARGE_SCRATCH)
    oxide_root = root_guard(options["oxide_root"], OXIDE_SCRATCH)
    barge_repo = repo_guard(options["barge_repo"])
    oxide_repo = repo_guard(options["oxide_repo"])
    barge_head = options["barge_head"]
    oxide_head = options["oxide_head"]
    outer_receipt = json.loads((evidence / "receipt.json").read_bytes())
    if (barge_root.resolve(strict=True) != barge_root or oxide_root.resolve(strict=True) != oxide_root):
        raise GateRefusal("task roots must be canonical existing directories")
    if adopted_scratch != barge_root or not evidence.is_relative_to(barge_root):
        raise GateRefusal("the outer supervisor must monitor the complete Barge task root and evidence")
    if oxide_head != OXIDE_PIN_COMMIT:
        raise GateRefusal("Oxide block head must equal the source built by the bootstrap")
    if mode == "candidate" and options["candidate_origin"]:
        origin_root = root_guard(options["candidate_origin"], BARGE_SCRATCH)
        if origin_root in (barge_root, oxide_root):
            raise GateRefusal("candidate origin root must be distinct from the active task roots")

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
    oxide_block = json.loads(oxide_block_path.read_bytes())
    # Charge preparation's complete supervised wall time plus the conservative
    # 120-second author-check reservation; review/issue idle time is not a run.
    prep_path = canonical_file(Path(options["preparation_receipt"]))
    prep = read_json_bound(prep_path, options["expect_preparation"])
    prep_command = prep.get("command")
    if (not isinstance(prep_command, list) or len(prep_command) < 3
            or prep_command[2] != "--prepare"):
        raise GateRefusal("preparation receipt has no matching whole command")
    prep_options = parse_args(prep_command[2:])
    for field in ("barge_head", "oxide_head", "oxide_product_head", "barge_root", "oxide_root"):
        if prep_options[field] != options[field]:
            raise GateRefusal("preparation receipt does not bind the diagnostic inputs")
    prep_scratch = Path(prep.get("scratch", ""))
    if (not prep_scratch.is_relative_to(BARGE_SCRATCH) or prep_scratch == barge_root
            or not prep_path.is_relative_to(prep_scratch)):
        raise GateRefusal("preparation needs a distinct retained Barge phase root")
    validate_supervisor_receipt(prep, prep_command, prep_scratch)
    prep_start, prep_end = prep.get("start_ns"), prep.get("published_ns")
    if type(prep_start) is not int or type(prep_end) is not int or not 0 < prep_start <= prep_end <= now_ns():
        raise GateRefusal("preparation wall-clock evidence is invalid")
    prep_seconds = (prep_end - prep_start + 999_999_999) // 1_000_000_000
    issued_block = read_json_bound(block_path, block_digest)
    issued_remaining = parse_block_head(issued_block) - 120 - prep_seconds
    deadline_ns = bind_outer_deadline(outer_receipt, deadline_ns, issued_remaining,
                                      oxide_block.get("prospective_max_seconds"))
    allowance = Allowance(issued_remaining, deadline_ns, None,
                          started_ns=outer_receipt["start_ns"])
    deadline_check(deadline_ns)
    oxide_budget_roots = validate_oxide_budget(budget_doc, run_root=oxide_root, scratch=OXIDE_SCRATCH)
    validate_oxide_history(history_doc, budget_doc)
    deadline_check(deadline_ns)
    oxide_commit_tree = extract_commit_identity(oxide_repo, options["oxide_product_head"],
                                                deadline_ns=deadline_ns)
    validate_oxide_block(oxide_block, run_root=oxide_root, commit=oxide_commit_tree[0],
                         tree=oxide_commit_tree[1],
                         binding_sha256=options["expect_oxide_binding"],
                         budget_sha256=options["expect_budget"],
                         history_sha256=options["expect_history"],
                         planner_reference=options.get("planner_reference",
                                                       OXIDE_PLANNER_REFERENCE))

    # Live executable bindings (read-only facts; any drift refuses).
    effigy = direct_effigy(Path(os.environ.get("BARGE_VALIDATION_EFFIGY", "")))
    git_tool = Path(os.environ.get("BARGE_VALIDATION_GIT", "")).resolve(strict=True)
    if git_tool != Path("/usr/bin/git").resolve(strict=True):
        raise GateRefusal("audited Git does not match the actual source reader")
    supervisor_tool = Path(os.environ.get("BARGE_VALIDATION_SUPERVISOR", "")).resolve(strict=True)
    rustc_tool = Path(os.environ.get("BARGE_PINNED_RUSTC", "")).resolve(strict=True)
    if sha256_file(supervisor_tool, cap=512 * 1024, deadline_ns=deadline_ns) != SUPERVISOR_SHA256:
        raise GateRefusal("qualified supervisor bytes are not the pinned qualified version")
    inherited = dict(os.environ)
    audit_doc = read_json_bound(audit_path, audit_digest)
    admission_env = audited_admission(inherited, audit_doc, deadline_ns=deadline_ns)
    invoking_cargo_home = os.environ.get("CARGO_HOME", "")
    if mode != "candidate" and (not invoking_cargo_home or not Path(invoking_cargo_home).is_dir()):
        raise GateRefusal("invoking CARGO_HOME is required as the offline seed source")

    # Cross-checkout source binding (planner relative-path correction): the
    # recorded script path is never resolved here. The packet's binding
    # document positively binds the preparation's actual script bytes, the
    # qualified guard bytes, the whole recorded command and the packet
    # manifest; identical reviewed source bytes at a different checkout pass.
    try:
        current_script_sha256 = sha256_file(Path(sys.argv[0]).resolve(strict=True),
                                            cap=MAX_AUDIT_SOURCE_BYTES, deadline_ns=deadline_ns)
    except (OSError, GateRefusal):
        raise GateRefusal("the running diagnostic script is unavailable") from None
    packet_dir = preparation_packet_dir(prep_options, prep_scratch)
    try:
        binding = read_json_bound(canonical_file(packet_dir / "source-binding.json"),
                                  options["expect_source_binding"])
    except OSError as error:
        raise GateRefusal(f"preparation source binding is missing or unreadable: {error}") from None
    deadline_check(deadline_ns)
    validate_preparation_binding(
        binding=binding, prep_command=prep_command, prep_run_id=prep.get("run_id"),
        prep_scratch=prep_scratch, packet_dir=packet_dir,
        current_script_sha256=current_script_sha256,
        barge_root=barge_root, oxide_root=oxide_root, barge_head=barge_head,
        oxide_head=oxide_head, oxide_product_head=prep_options["oxide_product_head"],
        audit_inputs_sha256=prep_options["expect_audit_inputs"], deadline_ns=deadline_ns)
    deadline_check(deadline_ns)
    # Fixed companion claims survive fresh evidence directories and burn on uncertainty.
    claim = {"gate_id": run_id,
             "barge_block_id": json.loads(block_path.read_bytes()).get("block_id"),
             "oxide_one_use_id": oxide_block.get("one_use_id"),
             "barge_block_sha256": block_digest, "oxide_block_sha256": approval,
             "started_ns": allowance.started_ns}
    if not block_path.is_relative_to(barge_root):
        raise GateRefusal("Barge block and token must be inside the monitored task root")



    # Re-read the deterministic documents and validate them against the
    # accepted launcher rules with live digests.
    allowance.begin("documents")
    ledger_doc = read_json_bound(ledger_path, ledger_digest)
    ledger_roots = validate_barge_ledger(ledger_doc, private_root=barge_root, scratch=BARGE_SCRATCH, deadline_ns=deadline_ns)
    allowance.finish("documents")

    # Candidate origin validation (oxide#254): retained-artifact bindings and
    # nested closure are proven from planner-digested documents BEFORE any
    # staging; the host-tools document binds the audited host inputs of
    # operator ruling d9aec625 against their live executables.
    if mode == "candidate":
        allowance.begin("candidate-origin")
        pinned_lock_sha256 = sha256_bytes(git_output(oxide_repo, "show", OXIDE_PIN_COMMIT + ":Cargo.lock", deadline_ns=deadline_ns))
        pinned_toolchain_sha256 = sha256_bytes(git_output(
            oxide_repo, "show", OXIDE_PIN_COMMIT + ":rust-toolchain.toml", deadline_ns=deadline_ns))
        cargo_tool = (rustc_tool.parent / "cargo").resolve(strict=True)
        host_tools_doc = read_json_bound(canonical_file(Path(options["host_tools"])),
                                         options["expect_host_tools"])
        validate_host_tools_document(host_tools_doc, rustc_tool=rustc_tool, cargo_tool=cargo_tool, deadline_ns=deadline_ns)
        origin_facts = validate_candidate_origin(
            origin_root=origin_root, options=options, ledger_roots=ledger_roots,
            pinned_lock_sha256=pinned_lock_sha256,
            pinned_toolchain_sha256=pinned_toolchain_sha256, deadline_ns=deadline_ns)
        if origin_root == prep_scratch or origin_root in prep_scratch.parents \
                or prep_scratch in origin_root.parents:
            raise GateRefusal("candidate origin root overlaps the preparation phase root")
        allowance.finish("candidate-origin")

    # Aggregate accounting before any child: the caps must fit what remains.
    # Both effective figures are measured clamps, disclosed in the receipt;
    # neither is a claim that a phase fits, and neither is silently raised.
    allowance.begin("account")
    barge_outside = allocated_bytes([p for p in ledger_roots if p != barge_root],
                                    BARGE_CAP_BYTES, deadline_ns)
    if prep_scratch not in ledger_roots:
        raise GateRefusal("preparation root is absent from the Barge ledger")
    barge_monitored = allocated_bytes([barge_root], BARGE_CAP_BYTES, deadline_ns)
    barge_effective_mib = clamp_mib(BARGE_CAP_BYTES,
                                    barge_outside,
                                    BOOTSTRAP_MAX_MIB)
    if barge_effective_mib < 1:
        raise GateRefusal("retained Barge evidence leaves no private scratch budget")
    supervisor_receipt = json.loads((evidence / "receipt.json").read_bytes())
    if supervisor_receipt.get("owned_bytes", {}).get("budget", 0) > barge_effective_mib * 1024 * 1024:
        raise GateRefusal("outer supervisor scratch budget exceeds the measured Barge remainder")
    oxide_caps = oxide_effective_caps(oxide_budget_roots, oxide_root)
    oxide_active_bytes = allocated_bytes([oxide_root], OXIDE_CAP_BYTES, deadline_ns)
    prep_bytes = allocated_bytes([prep_scratch], OXIDE_CAP_BYTES, deadline_ns)
    oxide_claim_mib = max(1, oxide_active_bytes // (1024 * 1024) + 1)
    oxide_caps["preparation_bytes"] = prep_bytes
    oxide_caps["active_oxide_bytes"] = oxide_active_bytes
    oxide_caps["effective_whole_block_mib"] = clamp_mib(
        OXIDE_CAP_BYTES, oxide_caps["retained_outside_bytes"] + prep_bytes + oxide_claim_mib * 1024 * 1024,
        BOOTSTRAP_MAX_MIB)
    if oxide_caps["effective_whole_block_mib"] < 1:
        raise GateRefusal("retained Oxide evidence leaves no whole-block scratch budget")
    if supervisor_receipt.get("owned_bytes", {}).get("budget", 0) > oxide_caps["effective_whole_block_mib"] * 1024 * 1024:
        raise GateRefusal("outer supervisor budget exceeds the measured Oxide remainder")
    retained_before = retained_snapshot(inactive_roots(ledger_roots, oxide_budget_roots,
                                                        barge_root, oxide_root), SCRATCH_ROOT,
                                        deadline_ns)
    live_before = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
    allowance.finish("account")

    # Burn each issued token under its own task root's guard. The outer guard
    # continues to monitor the Barge root while the short Oxide guard runs.
    allowance.require(10, "oxide-token")
    claim["tokens"] = [claim_issued_token(block_path, block_digest, run_id)]
    oxide_claim_evidence = oxide_root / "claim-evidence"
    claim_env = dict(os.environ, OXIDE_TASK_SCRATCH_ROOT=str(oxide_root))
    claim_command = [str(python), str(Path(__file__).resolve(strict=True)),
                     "--claim-oxide", "--block", str(oxide_block_path), "--digest", approval]
    claim_receipt, claim_out, claim_argv = run_supervised(
        supervisor=supervisor_tool, python=python, command=claim_command,
        cwd=oxide_repo, env=claim_env, limit_seconds=min(60, allowance.remaining_seconds()),
        scratch_mib=oxide_claim_mib,
        evidence=oxide_claim_evidence, allowance=allowance, phase="oxide-claim",
        log_dir=barge_root)
    validate_supervisor_receipt(claim_receipt, claim_command, oxide_root)
    claim["tokens"].append(json.loads(claim_out))
    private_dir(evidence, Path("private-build-validation.allowance"))
    allowance.state_dir = allowance_dir
    private_write(allowance_dir, Path("claim.json"), canonical_json(claim))

    # Materialize the prepared roots; the archives are verified byte-exactly.
    allowance.begin("setup")
    for root in (barge_root, oxide_root):
        st = os.lstat(root)
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_mode & 0o077:
            raise GateRefusal(f"prepared task root is not owner-only: {root}")
    oxide_source = private_dir(barge_root, Path("oxide-source"))
    barge_source = private_dir(barge_root, Path("barge-source"))
    _, oxide_tree = extract_commit_identity(oxide_repo, OXIDE_PIN_COMMIT, deadline_ns=deadline_ns)
    if oxide_tree != OXIDE_PIN_TREE:
        raise GateRefusal("pinned Oxide tree mismatch")
    oxide_files = stage_git_tree(oxide_repo, oxide_source, OXIDE_PIN_COMMIT, oxide_tree, deadline_ns=deadline_ns)
    barge_tree = extract_commit_identity(barge_repo, barge_head, deadline_ns=deadline_ns)[1]
    barge_files = stage_git_tree(barge_repo, barge_source, barge_head, barge_tree, deadline_ns=deadline_ns)
    if oxide_files.get("scripts/bounded-test.py") != SUPERVISOR_SHA256:
        raise GateRefusal("pinned Oxide archive does not carry the qualified supervisor bytes")
    wrapper_hash = barge_files.get("scripts/tape/private-wrapper.py")
    if not wrapper_hash:
        raise GateRefusal("Barge archive does not carry the private wrapper")
    audit_inputs = read_json_bound(canonical_file(Path(prep_options["audit_inputs"])),
                                   prep_options["expect_audit_inputs"])
    staged_inputs = {"inputs": [dict(entry, source=str(prep_scratch / entry["destination"]))
                                for entry in audit_inputs["inputs"]]}
    stage_audit_inputs(barge_root, staged_inputs, deadline_ns=deadline_ns)
    block_template = read_json_bound(block_path, block_digest)
    if mode == "candidate":
        # The candidate route takes COMPLETE issued documents: the retained
        # candidate digest is planner-bound before any staging, so nothing is
        # derived here and a derivation-style issuance refuses.
        block_doc = block_template
        issued_audit_digest = validate_candidate_issuance(block_doc, audit_doc,
                                                          options["expect_candidate"])
        validate_barge_audit(audit_doc, private=barge_root, host=live_audit_host(
            effigy=effigy, git_tool=git_tool, rustc_tool=rustc_tool, python=python,
            wrapper_hash=wrapper_hash, oxide_hash=options["expect_candidate"], deadline_ns=deadline_ns), deadline_ns=deadline_ns)
    else:
        block_doc, audit_preview = bind_barge_documents(block_template, audit_doc, "0" * 64)
        issued_audit_digest = sha256_bytes(canonical_json(audit_doc))
        validate_barge_audit(audit_preview, private=barge_root, host=live_audit_host(
            effigy=effigy, git_tool=git_tool, rustc_tool=rustc_tool, python=python,
            wrapper_hash=wrapper_hash, oxide_hash="0" * 64, deadline_ns=deadline_ns), deadline_ns=deadline_ns)
    if audit_doc.get("child_path") != str(barge_root / "bin") + os.pathsep + inherited.get("PATH", ""):
        raise GateRefusal("audited child PATH does not match the effective environment")
    if block_doc.get("barge_tree") != barge_tree:
        raise GateRefusal("issued Barge tree does not match the built archive")
    if mode == "candidate":
        max_seconds = validate_barge_block(block_doc, head=barge_head, wrapper_hash=wrapper_hash,
                                           oxide_hash=options["expect_candidate"],
                                           audit_hash=issued_audit_digest,
                                           ledger_hash=ledger_digest)
        if oxide_files["Cargo.lock"] != origin_facts["lock_sha256"] \
                or oxide_files["rust-toolchain.toml"] != origin_facts["toolchain_sha256"]:
            raise GateRefusal("staged source does not reproduce the candidate's "
                              "source/lock/toolchain binding")
    else:
        max_seconds = validate_barge_block(block_doc, head=barge_head, wrapper_hash=wrapper_hash,
                                           oxide_hash="0" * 64,
                                           audit_hash=sha256_bytes(canonical_json(audit_preview)),
                                           ledger_hash=ledger_digest)
    for name in ("home", "cache", "tmp", "scratch", "bin"):
        private_dir(barge_root, Path(name))
    private_write(barge_root, Path("config.toml"), b"")
    cache_root = private_dir(barge_root, Path("cache"))
    if mode == "candidate":
        # No build: the retained artifact is copied into this fresh
        # guard-owned root, rehashed and symlinked as the private oxide. The
        # origin tree is never written and no offline seed runs.
        copy_facts = copy_candidate_artifact(
            Path(origin_facts["artifact"]["path"]), barge_root,
            options["expect_candidate"], options["candidate_bytes"], deadline_ns=deadline_ns)
        os.symlink(copy_facts["path"], barge_root / "bin/oxide")
    else:
        sys.path.insert(0, str(oxide_source / "scripts"))
        try:
            from offline_seed import seed_cargo_home  # type: ignore
            seed_cargo_home(Path(invoking_cargo_home), cache_root,
                            (oxide_source / "Cargo.lock").read_bytes(),
                            deadline_ns=deadline_ns)
        finally:
            sys.path.pop(0)
    allowance.finish("setup")




    # The single debug bootstrap: exact argv under the nested qualified
    # supervisor, inside the 300 s window and the measured, clamped budget.
    # Before any launch the bounded provenance inputs are gathered and any
    # ambient Cargo configuration or PATH-resolved host tool without a
    # private binding refuses with its finding retained (oxide#253); after
    # the guarded closure the completed packet is retained BEFORE the
    # inventory assertion can refuse.
    if mode == "candidate":
        # Candidate derivation (oxide#254): no build. The copy already
        # happened inside the guarded setup; the same scrubbed allowlist
        # environment is built, the pre-launch scans must clear with exact
        # audited host-input bindings (ruling d9aec625) or private bindings,
        # and the candidate packet is retained before the status gate can
        # refuse. The audited host inputs are host tools, never private
        # bindings, and their hashes do not confine their writes.
        allowance.require(60, "private-status")
        allowance.begin("candidate-derivation")
        os.symlink(rustc_tool.parent / "cargo", barge_root / "bin/cargo")
        os.symlink(effigy, barge_root / "bin/effigy")
        env = child_environment(dict(os.environ), private=barge_root,
                                oxide=barge_root / "bin/oxide", rustc=rustc_tool,
                                wrapper_hash=wrapper_hash,
                                pool_cap=block_doc["pool_cap_bytes"],
                                admission_env=admission_env)
        ambient = attach_path_executables(
            ambient_cargo_config_scan(oxide_source, private_root=barge_root,
                                      cargo_home=Path(env["CARGO_HOME"]), deadline_ns=deadline_ns),
            env["PATH"], private_root=barge_root, cwd=oxide_source, deadline_ns=deadline_ns)
        host_observation, candidate_findings = reconcile_host_tools(ambient, host_tools_doc, deadline_ns=deadline_ns)
        ambient = dict(ambient)
        ambient["host_tool_bindings"] = host_observation
        # The adjudicated findings REPLACE the raw scan verdicts: a bound
        # audited host input no longer carries its ambient-executable refusal.
        ambient["violations"] = candidate_findings
        candidate_tools = {
            "python": tool_identity(python, 256 * 1024 * 1024, deadline_ns=deadline_ns),
            "supervisor": tool_identity(supervisor_tool, 512 * 1024, deadline_ns=deadline_ns),
            "rustc": tool_identity(rustc_tool, 256 * 1024 * 1024, deadline_ns=deadline_ns),
            "cargo": tool_identity(cargo_tool, 256 * 1024 * 1024, deadline_ns=deadline_ns),
            "effigy": tool_identity(effigy, 256 * 1024 * 1024, deadline_ns=deadline_ns),
            "git": tool_identity(git_tool, 64 * 1024 * 1024, deadline_ns=deadline_ns),
        }
        provenance_inputs = {
            "run_id": run_id, "private_root": barge_root,
            "account_root": LIVE_ACCOUNT_ROOT,
            "argv": [str(effigy), STATUS_SELECTOR], "cwd": barge_source, "env": env,
            "inherited_keys": sorted(os.environ),
            "tools": candidate_tools,
            "host_tools": host_observation,
            "sources": {
                "oxide_pin_commit": OXIDE_PIN_COMMIT, "oxide_pin_tree": oxide_tree,
                "oxide_source": str(oxide_source), "oxide_staged_files": len(oxide_files),
                "lock_sha256": oxide_files["Cargo.lock"],
                "toolchain_sha256": oxide_files["rust-toolchain.toml"],
                "barge_head": barge_head, "barge_tree": barge_tree,
                "barge_source": str(barge_source), "barge_staged_files": len(barge_files),
                "wrapper_sha256": wrapper_hash,
                "block_sha256": block_digest, "issued_audit_sha256": audit_digest,
                "issued_audit_document_sha256": issued_audit_digest,
                "issued_ledger_sha256": ledger_digest, "oxide_block_sha256": approval,
                "audit_inputs_sha256": prep_options["expect_audit_inputs"],
                "host_tools_document_sha256": options["expect_host_tools"],
                "candidate_sha256": options["expect_candidate"],
                "candidate_bytes": options["candidate_bytes"],
                "origin": origin_facts,
            },
            "limit_seconds": min(300, allowance.remaining_seconds()),
            "scratch_mib": min(barge_effective_mib, oxide_caps["effective_whole_block_mib"], 512),
            "pool_cap_bytes": block_doc["pool_cap_bytes"],
        }
        refuse_candidate_launch(provenance_inputs, ambient, origin_facts,
                                allowance.document())
        write_candidate_provenance(barge_root, build_candidate_provenance(
            provenance_inputs, origin=origin_facts, guard={"state": "pre-launch"},
            artifact={"state": "copied", **copy_facts},
            ambient=ambient, allowance=allowance.document(), written_ns=now_ns()))
        oxide_hash = options["expect_candidate"]
        env.update({"BARGE_VALIDATION_SOURCE_AUDIT": str(audit_path),
                    "BARGE_VALIDATION_SOURCE_AUDIT_SHA256": audit_digest,
                    "BARGE_VALIDATION_BLOCK": str(block_path),
                    "BARGE_VALIDATION_BLOCK_SHA256": block_digest,
                    "BARGE_VALIDATION_ROOT_LEDGER": str(ledger_path),
                    "BARGE_VALIDATION_ROOT_LEDGER_SHA256": ledger_digest})
        allowance.finish("candidate-derivation")
        live_after_derivation = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
        verify_inventory(live_before, live_after_derivation, "candidate-derivation", evidence)
    else:
        # The single debug bootstrap: exact argv under the nested qualified
        # supervisor, inside the 300 s window and the measured, clamped budget.
        # Before any launch the bounded provenance inputs are gathered and any
        # ambient Cargo configuration or PATH-resolved host tool without a
        # private binding refuses with its finding retained (oxide#253); after
        # the guarded closure the completed packet is retained BEFORE the
        # inventory assertion can refuse.
        allowance.begin_bootstrap()
        bootstrap_mib = min(barge_effective_mib, oxide_caps["effective_whole_block_mib"],
                            BOOTSTRAP_MAX_MIB)
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
        try:
            cargo_tool = (rustc_tool.parent / "cargo").resolve(strict=True)
        except OSError as error:
            raise GateRefusal(f"bootstrap cargo is unresolved: {error}") from None
        ambient = attach_path_executables(
            ambient_cargo_config_scan(oxide_source, private_root=barge_root,
                                      cargo_home=Path(env["CARGO_HOME"]), deadline_ns=deadline_ns),
            env["PATH"], private_root=barge_root, cwd=oxide_source, deadline_ns=deadline_ns)
        provenance_inputs = {
            "run_id": run_id, "private_root": barge_root,
            "account_root": LIVE_ACCOUNT_ROOT,
            "argv": list(BARGE_BOOTSTRAP_ARGV), "cwd": oxide_source, "env": env,
            "inherited_keys": sorted(os.environ),
            "tools": {
                "python": tool_identity(python, 256 * 1024 * 1024, deadline_ns=deadline_ns),
                "supervisor": tool_identity(supervisor_tool, 512 * 1024, deadline_ns=deadline_ns),
                "rustc": tool_identity(rustc_tool, 256 * 1024 * 1024, deadline_ns=deadline_ns),
                "cargo": tool_identity(cargo_tool, 256 * 1024 * 1024, deadline_ns=deadline_ns),
                "effigy": tool_identity(effigy, 256 * 1024 * 1024, deadline_ns=deadline_ns),
                "git": tool_identity(git_tool, 64 * 1024 * 1024, deadline_ns=deadline_ns),
            },
            "sources": {
                "oxide_pin_commit": OXIDE_PIN_COMMIT, "oxide_pin_tree": oxide_tree,
                "oxide_source": str(oxide_source), "oxide_staged_files": len(oxide_files),
                "lock_sha256": oxide_files["Cargo.lock"],
                "toolchain_sha256": oxide_files["rust-toolchain.toml"],
                "barge_head": barge_head, "barge_tree": barge_tree,
                "barge_source": str(barge_source), "barge_staged_files": len(barge_files),
                "wrapper_sha256": wrapper_hash,
                "block_sha256": block_digest, "issued_audit_sha256": audit_digest,
                "issued_ledger_sha256": ledger_digest, "oxide_block_sha256": approval,
                "audit_inputs_sha256": prep_options["expect_audit_inputs"],
            },
            "seed": {"source_cargo_home": invoking_cargo_home,
                     "cache_root": str(cache_root)},
            "limit_seconds": bootstrap_limit, "scratch_mib": bootstrap_mib,
            "pool_cap_bytes": block_doc["pool_cap_bytes"],
        }
        refuse_bootstrap_launch(provenance_inputs, ambient, allowance.document())
        bootstrap_receipt, bootstrap_log, bootstrap_argv = run_supervised(
            supervisor=supervisor_tool, python=python,
            command=list(BARGE_BOOTSTRAP_ARGV), cwd=oxide_source, env=env,
            limit_seconds=bootstrap_limit, scratch_mib=bootstrap_mib,
            evidence=bootstrap_evidence, allowance=allowance, phase="bootstrap")
        validate_supervisor_receipt(bootstrap_receipt, list(BARGE_BOOTSTRAP_ARGV), barge_root)
        # The nested launch may have consumed time since its earlier proposal.
        provenance_inputs["limit_seconds"] = bootstrap_receipt["limit_s"]
        guard_facts = bootstrap_guard_facts(
            bootstrap_receipt,
            receipt_path=bootstrap_evidence / "receipt.json",
            log_path=bootstrap_evidence.parent / "bootstrap-stdout.log",
            limit_seconds=bootstrap_receipt["limit_s"], scratch_mib=bootstrap_mib,
            expected_argv=list(BARGE_BOOTSTRAP_ARGV), deadline_ns=deadline_ns)
        facts = resolve_bootstrap_artifact(bootstrap_log, provenance_inputs,
                                           guard=guard_facts, ambient=ambient,
                                           allowance=allowance.document(), deadline_ns=deadline_ns)
        staged = Path(facts["artifact"]["resolved_path"])
        oxide_hash = facts["artifact"]["sha256"]
        os.symlink(staged, barge_root / "bin/oxide")
        allowance.finish_bootstrap()
        write_bootstrap_provenance(barge_root, build_bootstrap_provenance(
            provenance_inputs, guard=guard_facts,
            artifact={"state": "resolved", "main_rs_binding": facts["main_rs_binding"],
                      "path": facts["artifact"]["path"],
                      "resolved_path": facts["artifact"]["resolved_path"],
                      "bytes": facts["artifact"]["bytes"], "sha256": oxide_hash,
                      "symlinked_to": str(barge_root / "bin/oxide")},
            ambient=ambient, allowance=allowance.document(), written_ns=now_ns()))

        # Live isolation bracket: the bootstrap must have changed nothing.
        live_after_bootstrap = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
        verify_inventory(live_before, live_after_bootstrap, "bootstrap", evidence)

        # The external, one-use templates authorize this exact derivation only.
        # The accepted wrapper receives complete, immutable schema documents;
        # no guessed binary digest is needed before the single bootstrap.
        block_doc, issued_audit = bind_barge_documents(block_template, audit_doc, oxide_hash)
        validate_barge_audit(issued_audit, private=barge_root, host=live_audit_host(
            effigy=effigy, git_tool=git_tool, rustc_tool=rustc_tool, python=python,
            wrapper_hash=wrapper_hash, oxide_hash=oxide_hash, deadline_ns=deadline_ns), deadline_ns=deadline_ns)
        audited_admission(inherited, issued_audit, deadline_ns=deadline_ns)
        issued_audit_bytes = canonical_json(issued_audit)
        issued_block_bytes = canonical_json(block_doc)
        issued_dir = private_dir(barge_root, Path("issued"))
        issued_audit_path = private_write(issued_dir, Path("source-audit.json"), issued_audit_bytes)
        issued_block_path = private_write(issued_dir, Path("validation-block.json"), issued_block_bytes)
        issued_audit_digest = sha256_bytes(issued_audit_bytes)
        env.update({"BARGE_VALIDATION_SOURCE_AUDIT": str(issued_audit_path),
                    "BARGE_VALIDATION_SOURCE_AUDIT_SHA256": issued_audit_digest,
                    "BARGE_VALIDATION_BLOCK": str(issued_block_path),
                    "BARGE_VALIDATION_BLOCK_SHA256": sha256_bytes(issued_block_bytes),
                    "BARGE_VALIDATION_ROOT_LEDGER": str(ledger_path),
                    "BARGE_VALIDATION_ROOT_LEDGER_SHA256": ledger_digest})

    # The dry gate: the real private status through the same child route.
    allowance.require(60, "private-status")
    gate_limit = min(300, allowance.remaining_seconds())
    gate_mib = min(barge_effective_mib, oxide_caps["effective_whole_block_mib"], 512)
    gate_evidence = private_dir(barge_root, Path("evidence/private-status"))
    status_command = [str(effigy), STATUS_SELECTOR]
    def create_sentinels():
        state = private_dir(barge_root / "home", Path("Library/Application Support/oxide/state"))
        cache_probe_root = private_dir(barge_root / "home", Path("Library/Caches/oxide"))
        registry_probe = private_write(barge_root, Path(f"registry-probe-{block_doc['block_id']}"),
                                       block_doc["block_id"].encode())
        cache_probe = private_write(cache_probe_root, Path("build-pool"), block_doc["block_id"].encode())
        private_write(state, Path("pools.json"), canonical_json([str(registry_probe)]))
        return registry_probe, cache_probe

    (registry_probe, cache_probe), (status_receipt, status_out, status_argv) = run_status_after_sentinels(
        create_sentinels, lambda: run_supervised(
            supervisor=supervisor_tool, python=python, command=status_command,
            cwd=barge_source, env=env, limit_seconds=gate_limit, scratch_mib=gate_mib,
            evidence=gate_evidence, allowance=allowance, phase="private-status"))
    validate_supervisor_receipt(status_receipt, status_command, barge_root)
    verify_probe(status_out, home=barge_root / "home", config=barge_root / "config.toml",
                 pool=barge_root / "scratch/pool", registry_probe=registry_probe,
                 cache_probe=cache_probe)

    # Final isolation bracket and retained-root quiescence observation.
    final_live = live_inventory(LIVE_ACCOUNT_ROOT, deadline_ns)
    verify_inventory(live_before, final_live, "private-status", evidence)
    retained_after = retained_snapshot(inactive_roots(ledger_roots, oxide_budget_roots,
                                                        barge_root, oxide_root), SCRATCH_ROOT, deadline_ns)
    if retained_before != retained_after:
        raise GateRefusal("retained roots changed during the gate; quiescence unproven")

    # Full block validation now that the built oxide digest exists. The
    # candidate route already validated the complete issuance against the
    # planner-bound candidate digest during setup, before the copy.
    if mode == "bootstrap":
        max_seconds = validate_barge_block(block_doc, head=barge_head, wrapper_hash=wrapper_hash,
                                           oxide_hash=oxide_hash, audit_hash=issued_audit_digest,
                                           ledger_hash=ledger_digest)

    # Exports: the wrapper-consumable debug receipt and the measured
    # remaining allowance, then the honest gate receipt. The candidate route
    # exports the honest candidate receipt instead: no wrapper-consumable
    # debug receipt exists because the accepted wrapper binds one to an
    # in-root bootstrap build, which the candidate route intentionally does
    # not run.
    exports_dir = private_dir(barge_root, Path("exports"))
    live_before_doc = canonical_json(live_before)
    live_after_doc = canonical_json(final_live)
    live_before_export = private_write(exports_dir, Path("live-before.json"), live_before_doc)
    live_after_export = private_write(exports_dir, Path("live-after.json"), live_after_doc)
    if mode == "candidate":
        candidate_doc = {
            "schema": SCHEMA_CANDIDATE_RECEIPT,
            "mode": "candidate",
            "qualification": "unqualified",
            "ruling": CANDIDATE_RULING,
            "origin": origin_facts,
            "copy": copy_facts,
            "binding": {
                "commit": OXIDE_PIN_COMMIT, "tree": oxide_tree,
                "lock_sha256": oxide_files["Cargo.lock"],
                "toolchain_sha256": oxide_files["rust-toolchain.toml"],
                "rustc_sha256": sha256_file(rustc_tool, cap=256 * 1024 * 1024, deadline_ns=deadline_ns),
                "cargo_sha256": sha256_file(cargo_tool, cap=256 * 1024 * 1024, deadline_ns=deadline_ns),
                "oxide_sha256": oxide_hash,
                "files": oxide_files,
            },
            "toolchain_note": "the pinned toolchain and named linker/assembler tools are "
                              "audited host inputs (operator ruling " + CANDIDATE_RULING + "); "
                              "content hashes bind admission identity and never confine "
                              "their writes",
            "build": {"state": "not-built",
                      "note": "no bootstrap in the candidate route; the accepted wrapper "
                              "binds a debug receipt to an in-root cargo build, so no "
                              "wrapper-consumable debug receipt is derived here and "
                              "capture-side handoff stays on the capture tasks"},
            "live_inventory": {"before_sha256": sha256_bytes(live_before_doc),
                               "after_sha256": sha256_bytes(live_after_doc)},
            "status_phase": STATUS_SELECTOR,
            "bounds": {"max_seconds": max_seconds,
                       "pool_cap_bytes": block_doc["pool_cap_bytes"],
                       "scratch_cap_bytes": block_doc["scratch_cap_bytes"]},
        }
        debug_receipt = candidate_doc
    else:
        build_doc = {
            "command": list(BARGE_BOOTSTRAP_ARGV),
            "exit_code": 0,
            "artifact_sha256": oxide_hash,
            "source_tree": oxide_tree,
            "rustc_sha256": sha256_file(rustc_tool, cap=256 * 1024 * 1024, deadline_ns=deadline_ns),
            "cwd": str(oxide_source),
            "lock_sha256": oxide_files["Cargo.lock"],
            "toolchain_sha256": oxide_files["rust-toolchain.toml"],
            "cargo_sha256": sha256_file((rustc_tool.parent / "cargo").resolve(strict=True), cap=256 * 1024 * 1024, deadline_ns=deadline_ns),
            "environment": {key: env[key] for key in BUILD_ENV_KEYS},
            "log_path": str(bootstrap_evidence.parent / "bootstrap-stdout.log"),
            "log_sha256": sha256_file(bootstrap_evidence.parent / "bootstrap-stdout.log", deadline_ns=deadline_ns),
            "supervisor_receipt_path": str(bootstrap_evidence / "receipt.json"),
            "supervisor_receipt_sha256": sha256_file(bootstrap_evidence / "receipt.json", deadline_ns=deadline_ns),
            "live_before_path": str(live_before_export),
            "live_before_sha256": sha256_bytes(live_before_doc),
            "live_after_path": str(live_after_export),
            "live_after_sha256": sha256_bytes(live_after_doc),
        }
        debug_receipt = build_debug_receipt(binding={
            "commit": OXIDE_PIN_COMMIT, "tree": oxide_tree,
            "lock_sha256": oxide_files["Cargo.lock"],
            "toolchain_sha256": oxide_files["rust-toolchain.toml"],
            "rustc_sha256": sha256_file(rustc_tool, cap=256 * 1024 * 1024, deadline_ns=deadline_ns),
            "oxide_sha256": oxide_hash,
            "files": oxide_files,
        }, build=build_doc)
    debug_bytes = canonical_json(debug_receipt)
    allowance.require(10, "publication")
    publication_start = now_ns()
    published_allowance = allowance.document()
    published_allowance["remaining_seconds"] = max(0, published_allowance["remaining_seconds"] - 10)
    published_allowance["publication_reserve_seconds"] = 10
    receipt_name = ("candidate-oxide-receipt.json" if mode == "candidate"
                    else "debug-oxide-receipt.json")
    exports = {
        f"{'candidate_receipt' if mode == 'candidate' else 'debug_receipt'}":
            str(private_write(exports_dir, Path(receipt_name), debug_bytes)),
        f"{'candidate_receipt' if mode == 'candidate' else 'debug_receipt'}_sha256":
            sha256_bytes(debug_bytes),
        "remaining_allowance": published_allowance,
        "authorized_barge_max_seconds": max_seconds,
        "pure_checks_conservative_charge_seconds": 120,
        "preparation_seconds": prep_seconds,
        "preparation_receipt_sha256": options["expect_preparation"],
        "preparation_source_binding_sha256": options["expect_source_binding"],
        "preparation_packet_dir": str(packet_dir),
        "live_before_path": str(live_before_export),
        "live_before_sha256": sha256_bytes(live_before_doc),
        "live_after_path": str(live_after_export),
        "live_after_sha256": sha256_bytes(live_after_doc),
    }
    if mode == "candidate":
        exports.update({
            "candidate_copy_path": copy_facts["path"],
            "candidate_copy_sha256": copy_facts["sha256"],
            "candidate_origin_root": origin_facts["root"],
            "origin_guard_receipt_sha256": origin_facts["guard"]["receipt_sha256"],
            "origin_build_log_sha256": origin_facts["guard"]["log_sha256"],
            "host_tools_document_sha256": options["expect_host_tools"],
            "wrapper_consumables": "not derived in the candidate route: the accepted "
                                   "wrapper binds a debug receipt to an in-root cargo "
                                   "build, so issued documents stay with the capture "
                                   "tasks' own route",
        })
    else:
        exports.update({
            "issued_barge_block": str(issued_block_path),
            "issued_barge_block_sha256": sha256_bytes(issued_block_bytes),
            "issued_barge_audit": str(issued_audit_path),
            "issued_barge_audit_sha256": issued_audit_digest,
        })
    commands = [
        {"phase": "oxide-claim", "argv": claim_argv, "exit": 0},
    ]
    if mode == "candidate":
        decisions = [
            "candidate route (oxide#254): the private oxide is the retained debug "
            "artifact from the spent replacement diagnostic, validated for origin, "
            "source/lock/toolchain/artifact binding and retained nested closure "
            "before staging, copied only inside this fresh guard-owned root and "
            "rehashed; no bootstrap build ran and the origin tree was never written",
            "operator ruling " + CANDIDATE_RULING + " authorizes the pinned 1.97.1 "
            "toolchain and the named linker/assembler tools as audited host inputs "
            "for this route only: planner-bound canonical paths plus live content "
            "hashes, absent stays absent, changed or unlisted bindings refuse; they "
            "are host tools, not private bindings, and hashing does not confine "
            "their writes; ambient Cargo configuration and the environment allowlist "
            "refuse unchanged",
            "the retained failed isolation of the origin session stays historical "
            "and unrepaired; this run took its own fresh account inventory baseline "
            "and any inode/content delta would refuse",
            f"the private status gate runs the accepted Barge archive selector "
            f"{STATUS_SELECTOR} from the extracted barge-source root against the "
            f"candidate copy",
        ]
    else:
        decisions = [
            "host admission for the bootstrap is the gate task's own heavy Effigy "
            "admission through the audited hook, with the audited pair preserved in "
            "every child environment; no second admission seam and no archive edit",
            f"the dry status gate runs the accepted Barge archive selector "
            f"{STATUS_SELECTOR} from the extracted barge-source root",
        ]
        commands.append({"phase": "bootstrap", "argv": bootstrap_argv, "exit": 0})
    commands.append({"phase": STATUS_SELECTOR, "argv": status_argv, "exit": 0})
    candidate_observations = ({
        "route": "candidate",
        "ruling": CANDIDATE_RULING,
        "candidate_sha256": options["expect_candidate"],
        "candidate_copy_sha256": copy_facts["sha256"],
        "origin_guard_receipt_sha256": origin_facts["guard"]["receipt_sha256"],
        "origin_build_log_sha256": origin_facts["guard"]["log_sha256"],
        "origin_lock_sha256": origin_facts["lock_sha256"],
        "origin_toolchain_sha256": origin_facts["toolchain_sha256"],
        "host_tools_document_sha256": options["expect_host_tools"],
    } if mode == "candidate" else {})
    receipt_doc = gate_receipt(
        gate_id=run_id,
        blocks={"barge_block_sha256": block_digest, "oxide_block_sha256": approval},
        allowance=published_allowance,
        bindings={"effigy": sha256_file(effigy, cap=256 * 1024 * 1024, deadline_ns=deadline_ns), "oxide": oxide_hash,
                  "rustc": sha256_file(rustc_tool, cap=256 * 1024 * 1024, deadline_ns=deadline_ns), "git": sha256_file(git_tool, cap=256 * 1024 * 1024, deadline_ns=deadline_ns),
                  "supervisor": SUPERVISOR_SHA256, "wrapper": wrapper_hash},
        commands=commands,
        observations={"oxide_claim_receipt_sha256": sha256_file(oxide_claim_evidence / "receipt.json", deadline_ns=deadline_ns),
                      "live_inventory_sha256": sha256_bytes(live_before_doc),
                      "retained_snapshot_sha256": sha256_bytes(canonical_json(retained_after)),
                      "barge_aggregate_bytes": barge_outside + barge_monitored,
                      "barge_effective_bootstrap_mib": barge_effective_mib,
                      "oxide_retained_outside_bytes": oxide_caps["retained_outside_bytes"],
                      "oxide_effective_whole_block_mib": oxide_caps["effective_whole_block_mib"],
                      "oxide_effective_note": "measured clamp of the "
                      f"{OXIDE_CAP_BYTES // (1024 ** 3)} GiB prospective grant minus "
                      "retained bytes (operator af038a7e v2); the historical 6 GiB "
                      "measurement (5591.8 MiB retained, about 552 MiB effective) "
                      "stands in review 6584174560, and no figure claims that a "
                      "phase fits",
                      "barge_root_monitored_by": "outer qualified supervisor adopted scratch; "
                      "periodic samples do not establish an instantaneous peak",
                      **candidate_observations},
        exports=exports, failures=[], decisions=decisions)
    private_write(allowance_dir, Path("receipt.json"), canonical_json(receipt_doc))
    if now_ns() - publication_start > 10 * 1_000_000_000:
        raise GateRefusal("publication reserve exhausted; exported allowance is unusable")
    print("private-build-validation gate completed; receipt "
          f"{allowance_dir / 'receipt.json'}")
    return 0


def extract_commit_identity(repo: Path, commit: str, *, deadline_ns: int | None = None) -> tuple:
    if not isinstance(commit, str) or not HEX40.fullmatch(commit):
        raise GateRefusal(f"source head must be 40 hex: {commit!r}")
    tree = git_output(repo, "rev-parse", commit + "^{tree}", deadline_ns=deadline_ns).decode("ascii").strip()
    resolved = git_output(repo, "rev-parse", commit + "^{commit}", deadline_ns=deadline_ns).decode("ascii").strip()
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
    if oxide_head != OXIDE_PIN_COMMIT:
        raise GateRefusal("Oxide build head must equal the Barge-pinned source")
    oxide_product_head = options["oxide_product_head"]
    # Preparation is a supervised whole command: every walk and marker below
    # is inside the qualified supervisor's coverage and bound to its identity.
    evidence_dir, run_id, deadline_ns, adopted_scratch = supervisor_context()
    guard_command = json.loads((evidence_dir / "receipt.json").read_bytes()).get("command")
    if (not isinstance(guard_command, list) or len(guard_command) < 3
            or not all(isinstance(part, str) for part in guard_command)
            or guard_command[2] != "--prepare"):
        raise GateRefusal("the guard receipt does not record the whole prepared command")
    if (not adopted_scratch.is_relative_to(BARGE_SCRATCH) or adopted_scratch == barge_root
            or not evidence_dir.is_relative_to(adopted_scratch)
            or not packet_dir.is_relative_to(adopted_scratch)):
        raise GateRefusal("preparation needs a distinct guarded Barge phase root")
    audit_manifest_path = canonical_file(Path(options["audit_inputs"]))
    audit_manifest = read_json_bound(audit_manifest_path, options["expect_audit_inputs"])
    explicit_retained = audit_retained_roots(audit_manifest)
    active_roots = (barge_root, oxide_root, adopted_scratch)
    if any(p == active or p in active.parents or active in p.parents
           for p in explicit_retained for active in active_roots):
        raise GateRefusal("audit retained root overlaps an active or future root")
    input_paths = [audit_manifest_path, *explicit_retained]
    inputs = audit_manifest.get("inputs")
    if not isinstance(inputs, list):
        raise GateRefusal("audit input manifest is malformed")
    for item in inputs:
        if not isinstance(item, dict) or not isinstance(item.get("source"), str):
            raise GateRefusal("audit input source is malformed")
        path = Path(item["source"])
        if path.is_relative_to(SCRATCH_ROOT):
            input_paths.append(path)
    barge_extra = [str(barge_root), str(adopted_scratch)] + [str(p) for p in input_paths if p.is_relative_to(BARGE_SCRATCH)]
    oxide_extra = [str(oxide_root)] + [str(p) for p in input_paths if p.is_relative_to(OXIDE_SCRATCH)]
    barge_roots, barge_evidence = discover_barge_roots(barge_extra)
    oxide_packet = Path(os.environ.get("OXIDE_CORRECTED_DISCOVERY_PACKET", ""))
    oxide_roots, oxide_evidence, oxide_unknown = discover_oxide_roots(oxide_extra, oxide_packet)
    barge_retained = allocated_bytes([p for p in barge_roots
                                      if p not in (barge_root, adopted_scratch)],
                                     BARGE_CAP_BYTES, deadline_ns)
    oxide_retained = allocated_bytes(oxide_roots, OXIDE_CAP_BYTES, deadline_ns)
    # The explicitly supplied roots are shared preparation history. Count them
    # against both grants, without counting either tree twice within a grant.
    barge_retained += allocated_bytes([p for p in explicit_retained if p.is_relative_to(OXIDE_SCRATCH)],
                                      BARGE_CAP_BYTES, deadline_ns)
    oxide_retained += allocated_bytes([p for p in explicit_retained if p.is_relative_to(BARGE_SCRATCH)],
                                      OXIDE_CAP_BYTES, deadline_ns)
    prep_budget = json.loads((evidence_dir / "receipt.json").read_bytes()).get("owned_bytes", {}).get("budget")
    allowed_mib = min(clamp_mib(BARGE_CAP_BYTES, barge_retained, BOOTSTRAP_MAX_MIB),
                      clamp_mib(OXIDE_CAP_BYTES, oxide_retained, BOOTSTRAP_MAX_MIB))
    print(json.dumps({"phase": "preparation-resource-observation",
                      "barge_retained_bytes": barge_retained,
                      "oxide_retained_bytes": oxide_retained,
                      "effective_preparation_mib": allowed_mib,
                      "guard_budget_bytes": prep_budget}, sort_keys=True), flush=True)
    if type(prep_budget) is not int or prep_budget <= 0 or prep_budget > allowed_mib * 1024 * 1024:
        raise GateRefusal("preparation guard budget exceeds the measured aggregate remainder")
    os.makedirs(packet_dir, mode=0o700, exist_ok=False)
    for root in (adopted_scratch, oxide_root):
        st = os.lstat(root)
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_mode & 0o077:
            raise GateRefusal(f"prepared task root is not owner-only: {root}")
    oxide_source = private_dir(adopted_scratch, Path("oxide-source"))
    barge_source = private_dir(adopted_scratch, Path("barge-source"))
    audit_inputs = read_json_bound(canonical_file(Path(options["audit_inputs"])),
                                   options["expect_audit_inputs"])
    stage_audit_inputs(adopted_scratch, audit_inputs)
    _, oxide_tree = extract_commit_identity(oxide_repo, OXIDE_PIN_COMMIT)
    if oxide_tree != OXIDE_PIN_TREE:
        raise GateRefusal("pinned Oxide tree mismatch")
    oxide_files = stage_git_tree(oxide_repo, oxide_source, OXIDE_PIN_COMMIT, oxide_tree)
    barge_tree = extract_commit_identity(barge_repo, barge_head)[1]
    barge_files = stage_git_tree(barge_repo, barge_source, barge_head, barge_tree)
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
            oxide_root=oxide_root, commit=oxide_product_head, tree=extract_commit_identity(
                oxide_repo, oxide_product_head)[1],
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
                for name, (_analysis, pairs) in AUDIT_COMPONENT_SOURCES.items()},
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
        "oxide_product_head": oxide_product_head,
        "audit_inputs_sha256": options["expect_audit_inputs"],
        "documents": {name: sha256_file(packet_dir / name)
                      for name in sorted(documents)},
        "state": "unissued preparation; planner binding and review come first",
    }))
    # The cross-checkout source binding pins the preparation's actual script
    # bytes (resolved in this preparation process, where the recorded path is
    # meaningful), the whole guard-recorded command verbatim, the qualified
    # guard bytes and the manifest digest; the planner binds its digest at
    # issue and the diagnostic compares script bytes, never script paths.
    try:
        script_sha256 = sha256_file(Path(sys.argv[0]).resolve(strict=True),
                                    cap=MAX_AUDIT_SOURCE_BYTES)
    except (OSError, GateRefusal):
        raise GateRefusal("the running gate script is unavailable for its source binding") from None
    private_write(packet_dir, Path("source-binding.json"), canonical_json({
        "schema": SCHEMA_SOURCE_BINDING,
        "command": guard_command,
        "script_sha256": script_sha256,
        "supervisor_sha256": SUPERVISOR_SHA256,
        "packet_manifest_sha256": sha256_file(packet_dir / "packet-manifest.json"),
    }))
    print(f"prepared packet: {packet_dir}")
    for name in sorted(documents):
        print(f"  {name} sha256={sha256_file(packet_dir / name)}")
    print(f"  source-binding.json sha256="
          f"{sha256_file(packet_dir / 'source-binding.json')}")
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
    "missing, malformed or escaped bootstrap artifact",
    "ambient Cargo configuration above the private root",
    "unreadable Cargo configuration on the ancestor chain",
    "PATH-resolved host linker or assembler without a private binding",
    "candidate origin absent, changed, symlink-swapped or not ledger-covered",
    "candidate origin guard receipt or build log changed or not closed",
    "candidate lock/toolchain binding differs from the pinned source",
    "derivation-style issuance in the candidate route",
    "audited host input changed, unlisted, or documented but not live",
    "candidate copy digest or size drift, or origin identity change",
)


def plan(options: dict) -> int:
    print("shared private-build validation gate (oxide#248): plan")
    print(f"  supervisor bytes pinned: {SUPERVISOR_SHA256}")
    print(f"  oxide pin: {OXIDE_PIN_COMMIT} tree {OXIDE_PIN_TREE}")
    print(f"  barge accepted admission source (cap revision): {BARGE_ACCEPTED_HEAD}")
    print(f"  barge recipe head: {BARGE_RECIPE_HEAD} tree {BARGE_RECIPE_TREE}")
    print(f"  caps: barge {BARGE_CAP_BYTES} bytes, oxide {OXIDE_CAP_BYTES} bytes "
          f"(prospective aggregate grants, operator af038a7e v2; historical "
          f"2/6 GiB evidence keeps its original bounds), "
          f"bootstrap {BOOTSTRAP_MAX_SECONDS}s/{BOOTSTRAP_MAX_MIB}MiB, "
          f"prospective block {PROSPECTIVE_MAX_SECONDS}s")
    print(f"  schemas: {SCHEMA_GATE_RECEIPT}, {SCHEMA_BARGE_AUDIT}, "
          f"{SCHEMA_BARGE_LEDGER}, {SCHEMA_BARGE_BLOCK}")
    print(f"  bootstrap provenance: {PROVENANCE_PATH} retained inside the "
          f"monitored root before the isolation refusal (internal evidence, "
          f"{MAX_PROVENANCE_BYTES}-byte bound; inherited secret values never recorded)")
    print("  refusal controls exercised by --self-test:")
    for line in REFUSAL_CASES:
        print("   -", line)
    print("  proposed diagnostic command (planner binds every placeholder):")
    print("    OXIDE_TASK_SCRATCH_ROOT=<barge-root> python3 scripts/bounded-test.py --limit <issued-seconds> "
          "--scratch-budget-mib <reviewed-mib-at-most-512> -- "
          "python3 scripts/test-private-build-validation.py --diagnostic "
          "--oxide-repo <repo> --oxide-head " + OXIDE_PIN_COMMIT + " "
          "--oxide-product-head <approved-head> --barge-repo <repo> --barge-head <head> "
          "--barge-root <barge-root> --oxide-root <oxide-root> "
          "--expect-audit <sha256> --expect-ledger <sha256> --expect-budget <sha256> "
          "--expect-history <sha256> --expect-oxide-binding <sha256> "
          "--preparation-receipt <path> --expect-preparation <sha256> "
          "--expect-source-binding <sha256> "
          "--planner-reference <reference>")
    print("  candidate route (oxide#254; operator ruling " + CANDIDATE_RULING + "): the "
          "diagnostic arguments above plus --candidate-origin <retained root> "
          "--expect-candidate <sha256> --candidate-bytes <n> --candidate-receipt <path> "
          "--expect-candidate-receipt <sha256> --candidate-log <path> "
          "--expect-candidate-log <sha256> --candidate-lock-sha256 <sha256> "
          "--candidate-toolchain-sha256 <sha256> --host-tools <path> "
          "--expect-host-tools <sha256>")
    print("    the retained artifact is copied only inside the fresh guard-owned root and "
          "rehashed; no build, install, cleanup or repair runs; the named linker/assembler "
          "tools and pinned toolchain are audited host inputs (hashing binds admission "
          "identity and never confines their writes); the wrapper-consumable debug receipt "
          "is not derived")
    print("  proposed selectors: proof:private-build-validation:test (pure), "
          "proof:private-build-validation:plan (effect-free), "
          "proof:private-build-validation:diagnostic (heavy), "
          "proof:private-build-validation:candidate (heavy, planner-bound)")
    return 0


# ---------------------------------------------------------------------------
# Self-test: mocked counterfactuals only; effect-free outside its fixture
# ---------------------------------------------------------------------------

class Fixture:
    """A disposable fixture tree for the pure checks."""

    def __init__(self):
        base = os.environ.get("TMPDIR", "")
        if not base or not Path(base).is_absolute() or not Path(base).is_dir():
            raise GateRefusal("fixture TMPDIR must be an existing absolute directory")
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

    # The issued remainder bounds the qualified outer guard before any claim.
    from unittest.mock import Mock, patch
    with patch(__name__ + ".now_ns", return_value=11_000_000_000):
        sample_receipt = {"limit_s": 100, "start_ns": 10_000_000_000}
        bound = expect_ok("smaller issued outer limit", bind_outer_deadline,
                          sample_receipt, 210_000_000_000, 150, 1200)
        if bound != 110_000_000_000:
            failures.append("effective outer deadline did not use the smaller guard")
        floor_bound = expect_ok("31-second issued outer limit", bind_outer_deadline,
                                dict(sample_receipt, limit_s=31), 210_000_000_000, 31, 1200)
        if floor_bound != 41_000_000_000:
            failures.append("the minimum valid outer limit was not accepted")
        expect_ok("31-second issued allowance", Allowance, 31, floor_bound, None,
                  started_ns=10_000_000_000)
        expect_refusal("30-second issued allowance", Allowance, 30, floor_bound, None,
                       started_ns=10_000_000_000)
        expect_refusal("outer floor refuses", bind_outer_deadline,
                       dict(sample_receipt, limit_s=30), 210_000_000_000, 150, 1200)
        expect_refusal("outer above issuance refuses", bind_outer_deadline,
                       dict(sample_receipt, limit_s=151), 210_000_000_000, 150, 1200)
        expect_refusal("outer above oxide authority refuses", bind_outer_deadline,
                       sample_receipt, 210_000_000_000, 150, 99)
        # Exercise the actual guard's JSON number, not a recast integer fixture.
        numeric_receipt = json.loads('{"limit_s": 990.0, "start_ns": 10000000000}')
        raw_receipt = dict(numeric_receipt)
        float_bound = expect_ok("guard float limit", bind_outer_deadline,
                                numeric_receipt, 2_000_000_000_000, 990, 1200)
        int_bound = expect_ok("equivalent integer limit", bind_outer_deadline,
                              dict(numeric_receipt, limit_s=990), 2_000_000_000_000, 990, 1200)
        if float_bound != int_bound or float_bound != 1_000_000_000_000:
            failures.append("integral guard float changed the wall deadline")
        if numeric_receipt != raw_receipt or type(numeric_receipt['limit_s']) is not float:
            failures.append("guard receipt was mutated")
        numeric_token = Mock()
        def bind_numeric_then_claim(receipt):
            bind_outer_deadline(receipt, 2_000_000_000_000, 990, 1200)
            numeric_token()
        invalid_limits = (990.5, float('nan'), float('inf'), float('-inf'),
                          True, False, '990', None, 30.0, 991.0, 1201.0)
        for bad in invalid_limits:
            expect_refusal(f"invalid guard limit {bad!r} before claim", bind_numeric_then_claim,
                           dict(numeric_receipt, limit_s=bad))
        expect_refusal("missing guard limit before claim", bind_numeric_then_claim,
                       {"start_ns": numeric_receipt['start_ns']})
        if numeric_token.called:
            failures.append("invalid numeric guard value reached the token")
        expect_refusal("expired guard float refuses", bind_outer_deadline,
                       numeric_receipt, 11_000_000_000, 990, 1200)
        expect_refusal("float limit above oxide authority", bind_outer_deadline,
                       numeric_receipt, 2_000_000_000_000, 990, 989)
        expect_refusal("float issued allowance remains invalid", bind_outer_deadline,
                       numeric_receipt, 2_000_000_000_000, 990.0, 1200)
        expect_refusal("float supervisor deadline remains invalid", bind_outer_deadline,
                       numeric_receipt, 2_000_000_000_000.0, 990, 1200)
        expect_refusal("float oxide authority remains invalid", bind_outer_deadline,
                       numeric_receipt, 2_000_000_000_000, 990, 1200.0)
        expect_refusal("float guard start remains invalid", bind_outer_deadline,
                       dict(numeric_receipt, start_ns=10_000_000_000.0),
                       2_000_000_000_000, 990, 1200)
        token = Mock()
        def bind_then_claim(receipt):
            bind_outer_deadline(receipt, 210_000_000_000, 150, 1200)
            token()
        expect_refusal("floor refuses before token", bind_then_claim,
                       dict(sample_receipt, limit_s=30))
        expect_refusal("overspend refuses before token", bind_then_claim,
                       dict(sample_receipt, limit_s=151))
        if token.called:
            failures.append("invalid outer bound reached the token")
        earlier = expect_ok("enclosing deadline wins", bind_outer_deadline,
                            sample_receipt, 80_000_000_000, 150, 1200)
        if earlier != 80_000_000_000:
            failures.append("effective outer deadline ignored enclosing authority")
    empty_source = fixture.root / "deadline-source"
    empty_source.mkdir()
    with patch(__name__ + ".now_ns", return_value=100), \
         patch(__name__ + ".subprocess.run") as child:
        expect_refusal("staging Git respects deadline", stage_git_tree, fixture.root,
                       empty_source, "a" * 40, "b" * 40, deadline_ns=99)
        if child.called:
            failures.append("expired staging launched Git")
        expect_refusal("final inventory respects deadline", live_inventory,
                       private_root, 99)
    (private_root / "deadline-digest").write_bytes(b"digest")
    with patch(__name__ + ".now_ns", return_value=100):
        expect_refusal("expired final digest refuses", sha256_file, private_root / "deadline-digest",
                       deadline_ns=99)
    # Time can elapse after a caller's initial sample; never launch at <=30s.
    allowance_stub = Mock()
    allowance_stub.remaining_seconds.return_value = 30
    with patch(__name__ + ".subprocess.run") as child:
        expect_refusal("post-read nested guard floor", run_supervised,
                       supervisor=Path('/guard'), python=Path('/python'), command=['child'],
                       cwd=private_root, env={}, limit_seconds=60, scratch_mib=1,
                       evidence=private_root, allowance=allowance_stub,
                       phase="floor-control", log_dir=private_root)
        if child.called:
            failures.append("nested guard launched at or below 30 seconds")
    nested_evidence = fixture.root / "nested-floor-evidence"
    nested_evidence.mkdir()
    (nested_evidence / "receipt.json").write_text('{}')
    allowance_stub.remaining_seconds.return_value = 45
    with patch(__name__ + ".subprocess.run", return_value=Mock(returncode=0)) as child:
        observed = expect_ok("nested guard uses sampled remaining", run_supervised,
                             supervisor=Path('/guard'), python=Path('/python'), command=['child'],
                             cwd=private_root, env={}, limit_seconds=60, scratch_mib=1,
                             evidence=nested_evidence, allowance=allowance_stub,
                             phase="floor-positive", log_dir=private_root)
        if not child.called or not observed or observed[2][3] != '45':
            failures.append("nested guard did not bind the sampled remaining seconds")

    # A retained ledger can name loose files without broadening to scratch.
    case_base = fixture.root / "historic-barge"
    case_base.mkdir()
    case_packet = case_base / "packet"
    case_packet.mkdir()
    historic_dir = case_base / "041-budget-ledger-20261102"
    historic_dir.mkdir()
    run_root = case_base / "new-run"
    run_root.mkdir()
    required_root = case_base / "b041-final.n0hH3Cez"
    required_root.mkdir()
    loose = [case_base / "compile.log", case_base / "tape.log"]
    for file in loose:
        file.write_bytes(b"historic log")
    sibling = case_base / "unrelated.log"
    sibling.write_bytes(b"unrelated")
    history_file = write_doc(historic_dir, "ledger.json", {
        "taskId": BARGE_TASK,
        "known_inner_runtime_logs": [{"path": str(file)} for file in loose]})
    for name in ("root-discovery.json", "queue-evidence.json", "decision-evidence.json", "pr-41.json"):
        (case_packet / name).write_bytes(b"{}")
    with patch(__name__ + ".BARGE_SCRATCH", case_base), \
         patch(__name__ + ".BARGE_HISTORIC_LEDGER", history_file), \
         patch(__name__ + ".BARGE_PACKET", case_packet):
        discovered = expect_ok("loose log discovery", discover_barge_roots, [str(run_root)])
    if discovered:
        roots, discovered_evidence = discovered
        if (case_base in roots or sibling in roots or not set(loose).issubset(roots)):
            failures.append("discovery widened a loose log to shared scratch")
        ledger = build_barge_ledger(roots=roots, evidence=discovered_evidence,
                                    review_reference="fixture review")
        expect_ok("loose files conform to ledger", validate_barge_ledger, ledger,
                  private_root=run_root, scratch=case_base, historic=history_file,
                  required_roots=[historic_dir, required_root, case_packet])
        expect_ok("loose-file allocation", allocated_bytes, roots, BARGE_CAP_BYTES)
        loose_bytes = expect_ok("exact loose-file allocation", allocated_bytes, loose, BARGE_CAP_BYTES)
        if loose_bytes != sum(path.stat().st_blocks * 512 for path in loose):
            failures.append("loose-file allocation included unrelated siblings")
        snapshot = expect_ok("loose-file snapshot", retained_snapshot, loose, case_base)
        if snapshot and (len(snapshot) != 2 or any(row["rows"] != 1 or
                            row["allocated_or_logical_bytes"] <= 0 for row in snapshot)):
            failures.append("loose files did not contribute one counted snapshot row each")
        replacement = case_base / "replacement"
        replacement.write_bytes(b"historic log")
        os.replace(replacement, loose[0])
        changed = expect_ok("replaced loose file snapshot", retained_snapshot, loose, case_base)
        if snapshot == changed:
            failures.append("same-content loose-file replacement went undetected")
        os.chmod(loose[0], 0)
        expect_refusal("unreadable loose file refused", retained_snapshot, loose, case_base)
        expect_refusal("unreadable loose file allocation refused", allocated_bytes, loose, BARGE_CAP_BYTES)
        os.chmod(loose[0], 0o600)
        loose[0].unlink()
        expect_refusal("missing loose file refused", retained_snapshot, loose, case_base)

    # Names and dispositions from corrected comment 6583995069, represented
    # only by fresh fixture paths. The real packet remains an external input.
    names = ["task240.JMzntJWF", "q.EOYt", "oxide240.CVGVXlGm", "v.GcY4",
             "x.PFjS", "residue-240-20261102"]
    actual_roots = [str(oxide_scratch / name) for name in names]
    aux_files = [str(oxide_scratch / name) for name in (
        "task240-current-b853", "task240-validation-f521a2ff", "task240-validation-febaeea5",
        "pr240-audit.FL1RHRml", "pr240-body.pv8ksjFP", "pr240-review.gM5cNBCo")]
    aux_dirs = [str(oxide_scratch / name) for name in (
        "review-230", "planner-240-7da-review-disposition-20261102",
        "planner-240-bf2-continuation-20261102", "planner-240-build-premise-20261102",
        "planner-240-clarification-20261102", "planner-240-home-boundary-20261102",
        "planner-240-owner-disposition-20261102", "planner-240-recovery-reconcile-20261102",
        "planner-240-root-disposition-20261102")]
    corrected = {"task": OXIDE_TASK, "complete": False,
                 "retained_roots": actual_roots,
                 "proposed_budget_retained_roots": actual_roots,
                 "unknown": ["mixed evidence unresolved"],
                 "candidates": ([{"path": path, "disposition": "attributed",
                                  "kind": "directory", "size_complete": True} for path in actual_roots]
                                + [{"path": path, "disposition": "attributed", "kind": "file", "size_complete": True} for path in aux_files]
                                + [{"path": path, "disposition": "attributed", "kind": "directory", "size_complete": True} for path in aux_dirs]
                                + [{"path": str(oxide_scratch / "mixed"), "disposition": "unknown", "basis": "mixed"}])}
    parsed = expect_ok("corrected Oxide packet", oxide_packet_roots, corrected, oxide_scratch)
    if parsed and (parsed[0] != set(map(Path, actual_roots + aux_files + aux_dirs + [str(oxide_scratch / "mixed")])) or not parsed[1]):
        failures.append("corrected Oxide packet omitted attributed inputs or unknowns")
    packet_file = write_doc(oxide_scratch, "corrected.json", corrected)
    oxide_review_dir = oxide_scratch / "planner-240-review-disposition-20261102"
    oxide_review_dir.mkdir()
    (oxide_review_dir / "queue-detail.json").write_bytes(b"review 6584174560")
    with patch(__name__ + ".OXIDE_SCRATCH", oxide_scratch), \
         patch(__name__ + ".OXIDE_DISCOVERY_PACKET", oxide_scratch / "discovery"), \
         patch(__name__ + ".OXIDE_OWNER_PACKET", oxide_scratch / "owner"):
        oxide_discovered = expect_ok("digest-bound corrected Oxide discovery", discover_oxide_roots,
                                     [], packet_file, sha256_file(packet_file))
        expect_refusal("corrected Oxide packet digest drift", discover_oxide_roots,
                       [], packet_file, "0" * 64)
    if oxide_discovered and not set(map(Path, actual_roots + aux_files + aux_dirs)).issubset(oxide_discovered[0]):
        failures.append("Oxide discovery omitted corrected attributed evidence")
    expect_refusal("corrected packet cannot claim completeness", oxide_packet_roots,
                   dict(corrected, complete=True), oxide_scratch)
    expect_refusal("corrected packet cannot escape scratch", oxide_packet_roots,
                   dict(corrected, candidates=[{"path": str(oxide_scratch.parent), "disposition": "attributed"}]), oxide_scratch)

    # Exercise the real prepare document assembly and manifest, with only the
    # external supervisor and Git interfaces replaced by fixture observations.
    loose[0].write_bytes(b"historic log")
    for path in map(Path, actual_roots + aux_dirs + [str(oxide_scratch / "mixed"),
                                                       str(oxide_scratch / "discovery"),
                                                       str(oxide_scratch / "owner")]):
        path.mkdir(exist_ok=True)
    for path in map(Path, aux_files):
        path.write_bytes(b"retained fixture")
    prep_root = case_base / "prep"
    prep_root.mkdir(mode=0o700)
    prep_evidence = prep_root / "evidence"
    prep_evidence.mkdir(mode=0o700)
    future_barge = case_base / "future"
    future_barge.mkdir(mode=0o700)
    future_oxide = oxide_scratch / "future"
    future_oxide.mkdir(mode=0o700)
    oxide_repo_fixture = fixture.root / "oxide-repo"
    barge_repo_fixture = fixture.root / "barge-repo"
    for repo in (oxide_repo_fixture, barge_repo_fixture):
        repo.mkdir()
        (repo / ".git").mkdir()
    input_file = case_base / "audit-source"
    input_file.write_bytes(b"audit source")
    prior_partial = case_base / "prior-partial"
    prior_partial.mkdir(mode=0o700)
    prior_payload = prior_partial / "partial-source.bin"
    prior_payload.write_bytes(b"retained payload" * 1024)
    input_manifest = write_doc(oxide_scratch, "prepare-audit-inputs.json", {"inputs": [{
        "source": str(input_file), "destination": "effigy-source/admission.rs",
        "sha256": sha256_file(input_file)}], "retained_roots": [str(prior_partial)]})
    with patch(__name__ + ".BARGE_SCRATCH", case_base), patch(__name__ + ".OXIDE_SCRATCH", oxide_scratch):
        expect_ok("owned retained root accepted", audit_retained_roots,
                  {"retained_roots": [str(prior_partial)]})
        expect_refusal("shared base refused", audit_retained_roots, {"retained_roots": [str(case_base)]})
        expect_refusal("outside retained root refused", audit_retained_roots, {"retained_roots": [str(fixture.root)]})
        expect_refusal("missing retained root refused", audit_retained_roots, {"retained_roots": [str(case_base / "absent")]})
        alias = case_base / "partial-alias"
        os.symlink(prior_partial, alias)
        expect_refusal("aliased retained root refused", audit_retained_roots, {"retained_roots": [str(alias)]})
        expect_refusal("duplicate retained root refused", audit_retained_roots,
                       {"retained_roots": [str(prior_partial), str(prior_partial)]})
    unsafe_manifest = write_doc(oxide_scratch, "unsafe-audit-inputs.json", {"inputs": [{
        "source": str(input_file), "destination": "effigy-source/admission.rs",
        "sha256": sha256_file(input_file)}], "retained_roots": [str(case_base)]})
    prepare_options = {"packet_dir": str(prep_root / "packet"),
                       "oxide_repo": str(oxide_repo_fixture), "oxide_head": OXIDE_PIN_COMMIT,
                       "oxide_product_head": "6e8ca1f07a3e26be9e96ff55fd774b0f8185165a",
                       "barge_repo": str(barge_repo_fixture), "barge_head": BARGE_ACCEPTED_HEAD,
                       "barge_root": str(future_barge), "oxide_root": str(future_oxide),
                       "audit_inputs": str(input_manifest), "expect_audit_inputs": sha256_file(input_manifest)}
    # The historical preparation receipt records a RELATIVE script path with
    # the interpreter's own argv; the fixture mirrors that exact shape.
    fixture_command = ["python3", "scripts/test-private-build-validation.py", "--prepare",
                       "--packet-dir", str(prep_root / "packet"),
                       "--oxide-repo", str(oxide_repo_fixture), "--oxide-head", OXIDE_PIN_COMMIT,
                       "--oxide-product-head", "6e8ca1f07a3e26be9e96ff55fd774b0f8185165a",
                       "--barge-repo", str(barge_repo_fixture), "--barge-head", BARGE_ACCEPTED_HEAD,
                       "--barge-root", str(future_barge), "--oxide-root", str(future_oxide),
                       "--audit-inputs", str(input_manifest),
                       "--expect-audit-inputs", sha256_file(input_manifest)]
    write_doc(prep_evidence, "receipt.json", {"owned_bytes": {"budget": 128 * 1024 * 1024},
                                              "command": fixture_command,
                                              "run_id": "c" * 32,
                                              "phase": "started: incomplete"})
    def fixture_git_identity(repo, commit):
        return commit, OXIDE_PIN_TREE if commit == OXIDE_PIN_COMMIT else "b" * 40
    def fixture_git_tree(repo, root, commit, tree):
        return {"scripts/tape/private-wrapper.py": "a" * 64} if repo == barge_repo_fixture else {}
    from contextlib import redirect_stdout
    from io import StringIO
    prep_stdout = StringIO()
    with patch(__name__ + ".BARGE_SCRATCH", case_base), \
         patch(__name__ + ".OXIDE_SCRATCH", oxide_scratch), \
         patch(__name__ + ".SCRATCH_ROOT", fixture.root), \
         patch(__name__ + ".BARGE_HISTORIC_LEDGER", history_file), \
         patch(__name__ + ".BARGE_PACKET", case_packet), \
         patch(__name__ + ".OXIDE_DISCOVERY_PACKET", oxide_scratch / "discovery"), \
         patch(__name__ + ".OXIDE_OWNER_PACKET", oxide_scratch / "owner"), \
         patch.object(discover_oxide_roots, "__defaults__", (sha256_file(packet_file),)), \
         patch.dict(os.environ, {"OXIDE_CORRECTED_DISCOVERY_PACKET": str(packet_file)}), \
         patch(__name__ + ".supervisor_context", return_value=(prep_evidence, "c" * 32,
                                                                now_ns() + 120_000_000_000, prep_root)), \
         patch(__name__ + ".extract_commit_identity", side_effect=fixture_git_identity), \
         patch(__name__ + ".stage_git_tree", side_effect=fixture_git_tree), \
         patch(__name__ + ".allocated_bytes", wraps=allocated_bytes) as allocation_spy, \
         redirect_stdout(prep_stdout):
        prepared = expect_ok("real prepare document assembly", prepare, prepare_options)
        prior_calls = [call for call in allocation_spy.call_args_list if prior_partial in call.args[0]]
        if {call.args[1] for call in prior_calls} != {BARGE_CAP_BYTES, OXIDE_CAP_BYTES}:
            failures.append("prepare did not count prior partial root against both grants")
        unsafe_options = dict(prepare_options, packet_dir=str(prep_root / "unsafe-packet"),
                              audit_inputs=str(unsafe_manifest), expect_audit_inputs=sha256_file(unsafe_manifest))
        expect_refusal("prepare refuses a shared scratch base", prepare, unsafe_options)
        if (prep_root / "unsafe-packet").exists():
            failures.append("prepare wrote a packet after unsafe-root refusal")
    if prepared != 0:
        failures.append("prepare did not finish its document assembly")
    else:
        observations = [json.loads(line) for line in prep_stdout.getvalue().splitlines() if line.startswith("{")]
        if (len(observations) != 1 or observations[0].get("phase") != "preparation-resource-observation"
                or observations[0].get("guard_budget_bytes") != 128 * 1024 * 1024
                or observations[0].get("barge_retained_bytes", 0) < prior_payload.stat().st_blocks * 512
                or observations[0].get("oxide_retained_bytes", 0) < prior_payload.stat().st_blocks * 512
                or type(observations[0].get("effective_preparation_mib")) is not int):
            failures.append("prepare did not report the measured resource observation")
        manifest_path = prep_root / "packet/packet-manifest.json"
        manifest_doc = json.loads(manifest_path.read_bytes())
        expected_documents = {"barge-root-ledger.UNISSUED.json", "oxide-budget.UNISSUED.json",
                              "oxide-history-discovery.json", "oxide-block.UNISSUED.json",
                              "barge-block.UNISSUED.json", "source-audit.COMPONENTS.json"}
        if set(manifest_doc["documents"]) != expected_documents:
            failures.append("prepare manifest does not bind every proposed document")
        for name, digest in manifest_doc["documents"].items():
            if sha256_file(manifest_path.parent / name) != digest:
                failures.append(f"prepare manifest digest mismatch: {name}")
        components = json.loads((manifest_path.parent / "source-audit.COMPONENTS.json").read_bytes())
        expected_components = {name: [[str(path), lines] for path, lines in pairs]
                               for name, (_analysis, pairs) in AUDIT_COMPONENT_SOURCES.items()}
        if components["component_sources"] != expected_components:
            failures.append("prepare audit components do not match their actual source pairs")
        history = json.loads((manifest_path.parent / "oxide-history-discovery.json").read_bytes())
        if history["complete"] is not False or not history["unknown"]:
            failures.append("prepare misrepresented unresolved discovery")
        barge_proposal = json.loads((manifest_path.parent / "barge-block.UNISSUED.json").read_bytes())
        oxide_proposal = json.loads((manifest_path.parent / "oxide-block.UNISSUED.json").read_bytes())
        ledger_proposal = json.loads((manifest_path.parent / "barge-root-ledger.UNISSUED.json").read_bytes())
        if str(prior_partial) not in ledger_proposal["roots"]:
            failures.append("prepare ledger omitted prior partial root")
        else:
            counted = expect_ok("prior partial root allocated", allocated_bytes, [prior_partial], BARGE_CAP_BYTES)
            if counted is None or counted < prior_payload.stat().st_blocks * 512:
                failures.append("prior partial payload was not counted")
        budget_proposal = json.loads((manifest_path.parent / "oxide-budget.UNISSUED.json").read_bytes())
        if any(barge_proposal[key] is not None for key in ("block_id", "authorization_reference",
                                                             "oxide_sha256", "audit_sha256", "ledger_sha256")):
            failures.append("prepare unexpectedly issued a Barge block")
        if any(oxide_proposal[key] is not None for key in ("one_use_id", "binding_sha256",
                                                            "ledger_sha256", "history_sha256")):
            failures.append("prepare unexpectedly issued an Oxide block")
        if ledger_proposal["review_reference"] is not None or budget_proposal["history_reference"] is not None:
            failures.append("prepare unexpectedly approved a discovery ledger")
        if manifest_doc["state"] != "unissued preparation; planner binding and review come first":
            failures.append("prepare manifest did not preserve its unissued state")
        # The cross-checkout source binding: prepare must bind its actual
        # running bytes, the whole guard-recorded command, the qualified guard
        # bytes and the manifest digest, and the validator must accept the
        # identical source and refuse every mismatch.
        binding_path = prep_root / "packet/source-binding.json"
        binding_doc = json.loads(binding_path.read_bytes())
        running_digest = sha256_file(Path(sys.argv[0]).resolve(strict=True),
                                     cap=MAX_AUDIT_SOURCE_BYTES)
        binding_kwargs = dict(prep_command=fixture_command, prep_run_id="c" * 32,
                              prep_scratch=prep_root, packet_dir=manifest_path.parent,
                              barge_root=future_barge, oxide_root=future_oxide,
                              barge_head=BARGE_ACCEPTED_HEAD, oxide_head=OXIDE_PIN_COMMIT,
                              oxide_product_head="6e8ca1f07a3e26be9e96ff55fd774b0f8185165a",
                              audit_inputs_sha256=sha256_file(input_manifest))
        if (set(binding_doc) != {"schema", "command", "script_sha256", "supervisor_sha256",
                                 "packet_manifest_sha256"}
                or binding_doc["schema"] != SCHEMA_SOURCE_BINDING
                or binding_doc["command"] != fixture_command
                or binding_doc["script_sha256"] != running_digest
                or binding_doc["supervisor_sha256"] != SUPERVISOR_SHA256
                or binding_doc["packet_manifest_sha256"] != sha256_file(manifest_path)):
            failures.append("prepare did not bind its actual source, guard, command and manifest")
        expect_ok("prepared binding accepts the identical running source",
                  validate_preparation_binding, binding=binding_doc,
                  current_script_sha256=running_digest, **binding_kwargs)
        absolute_command = ["python3",
                            "/planning-checkout/scripts/test-private-build-validation.py",
                            *fixture_command[2:]]
        absolute_kwargs = dict(binding_kwargs, prep_command=absolute_command)
        expect_ok("absolute-path preparation binds the same source bytes",
                  validate_preparation_binding, binding=dict(binding_doc, command=absolute_command),
                  current_script_sha256=running_digest, **absolute_kwargs)
        expect_refusal("changed source bytes refuse across checkouts",
                       validate_preparation_binding, binding=binding_doc,
                       current_script_sha256="0" * 64, **binding_kwargs)
        expect_refusal("guard bytes mismatch refuses", validate_preparation_binding,
                       binding=dict(binding_doc, supervisor_sha256="0" * 64),
                       current_script_sha256=running_digest, **binding_kwargs)
        edited_command = list(fixture_command)
        edited_command[edited_command.index("--oxide-root") + 1] = str(case_base / "elsewhere")
        expect_refusal("edited command token refuses", validate_preparation_binding,
                       binding=binding_doc, current_script_sha256=running_digest,
                       **dict(binding_kwargs, prep_command=edited_command))
        expect_refusal("manifest root mismatch refuses", validate_preparation_binding,
                       binding=binding_doc, current_script_sha256=running_digest,
                       **dict(binding_kwargs, oxide_root=case_base / "other-oxide"))
        expect_refusal("edited manifest refuses", validate_preparation_binding,
                       binding=dict(binding_doc, packet_manifest_sha256="1" * 64),
                       current_script_sha256=running_digest, **binding_kwargs)
        expect_refusal("missing binding refuses", validate_preparation_binding,
                       binding=None, current_script_sha256=running_digest, **binding_kwargs)
        expect_refusal("malformed binding refuses", validate_preparation_binding,
                       binding={"schema": "other/1"}, current_script_sha256=running_digest,
                       **binding_kwargs)
        expect_refusal("packet outside the preparation root refuses", preparation_packet_dir,
                       {"packet_dir": str(case_base / "outside-packet")}, prep_root)
        expect_refusal("missing packet directory refuses", preparation_packet_dir,
                       {"packet_dir": str(prep_root / "absent-packet")}, prep_root)
        expect_refusal("packet at the preparation root itself refuses", preparation_packet_dir,
                       {"packet_dir": str(prep_root)}, prep_root)
        tampered = manifest_path.parent / "barge-block.UNISSUED.json"
        saved_bytes = tampered.read_bytes()
        tampered.write_bytes(saved_bytes + b" ")
        expect_refusal("edited packet document refuses", validate_preparation_binding,
                       binding=binding_doc, current_script_sha256=running_digest, **binding_kwargs)
        tampered.write_bytes(saved_bytes)
        saved_manifest = manifest_path.read_bytes()
        manifest_path.unlink()
        expect_refusal("missing packet manifest refuses", validate_preparation_binding,
                       binding=binding_doc, current_script_sha256=running_digest, **binding_kwargs)
        manifest_path.write_bytes(saved_manifest)

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
    historic_cap = dict(issued_block, scratch_cap_bytes=HISTORIC_BARGE_CAP_BYTES)
    expect_refusal("historical 2 GiB cap is not the approved cap", validate_barge_block,
                   historic_cap, head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    beyond_cap = dict(issued_block, scratch_cap_bytes=BARGE_CAP_BYTES + 1)
    expect_refusal("scratch cap beyond the approved grant", validate_barge_block,
                   beyond_cap, head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                   oxide_hash=full_host["oxide_hash"], audit_hash=full_host["audit_hash"],
                   ledger_hash=full_host["ledger_hash"])
    exact_pool = dict(issued_block, pool_cap_bytes=BARGE_CAP_BYTES)
    exact_pool_bound = expect_ok("pool cap at the approved grant accepted",
                                 validate_barge_block, exact_pool,
                                 head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                                 oxide_hash=full_host["oxide_hash"],
                                 audit_hash=full_host["audit_hash"],
                                 ledger_hash=full_host["ledger_hash"])
    if exact_pool_bound != issued_block["max_seconds"]:
        failures.append("pool cap at the approved grant did not return the block bound")
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
    template_block = dict(issued_block, oxide_sha256=None, audit_sha256=None,
                          authorization_reference="derived-artifact-v1:reviewed-issue")
    template_audit = dict(good_audit, oxide_sha256=None)
    derived = expect_ok("deferred artifact documents bind after bootstrap", bind_barge_documents,
                        template_block, template_audit, "a" * 64)
    if derived is not None:
        final_block, final_audit = derived
        if (final_block["oxide_sha256"] != "a" * 64
                or final_audit["oxide_sha256"] != "a" * 64
                or final_block["audit_sha256"] != sha256_bytes(canonical_json(final_audit))):
            failures.append("derived audit and block did not bind the actual artifact")
        expect_ok("derived block retains accepted schema", validate_barge_block, final_block,
                  head=BARGE_RECIPE_HEAD, wrapper_hash=host["wrapper_sha256"],
                  oxide_hash="a" * 64, audit_hash=final_block["audit_sha256"],
                  ledger_hash=full_host["ledger_hash"])
    expect_refusal("prefilled artifact is not a deferred authorization", bind_barge_documents,
                   dict(template_block, oxide_sha256="b" * 64), template_audit, "a" * 64)
    expect_refusal("missing derivation authority", bind_barge_documents,
                   dict(template_block, authorization_reference="ordinary-block"),
                   template_audit, "a" * 64)
    expect_refusal("malformed artifact digest", bind_barge_documents,
                   template_block, template_audit, "bad")
    expect_ok("valid audit", validate_barge_audit, good_audit, private=private_root, host={})
    large_source = private_write(private_root, Path("toolchain/cargo"), b"x" * (MAX_DOC_BYTES + 1))
    large_audit = json.loads(canonical_json(good_audit))
    large_audit["components"]["rustup_cargo"]["sources"] = [{
        "path": str(large_source), "sha256": hashlib.sha256(b"x" * (MAX_DOC_BYTES + 1)).hexdigest(),
        "lines": "digest-bound binary"}]
    large_pairs = {name: (analysis, [("toolchain/cargo", "digest-bound binary")])
                   for name in BARGE_AUDIT_COMPONENTS}
    built_large = expect_ok("build audit with a large component", build_barge_audit,
                            private=private_root, component_sources=large_pairs,
                            host={key: "1" * 64 for key in ("effigy_sha256", "oxide_sha256",
                                  "wrapper_sha256", "git_sha256", "rustc_sha256", "supervisor_sha256")},
                            admission={"hook": "hook", "socket": "/socket", "executables": [], "runtime_files": []},
                            child_facts={"child_path": "/bin", "cargo_sha256": "2" * 64,
                                         "python_sha256": "3" * 64})
    if built_large is not None and built_large["components"]["rustup_cargo"]["sources"][0] != \
            large_audit["components"]["rustup_cargo"]["sources"][0]:
        failures.append("large audit component builder digest mismatch")
    expect_ok("audit accepts a component larger than a document", validate_barge_audit,
              large_audit, private=private_root, host={})
    large_audit["components"]["rustup_cargo"]["sources"][0]["sha256"] = "0" * 64
    expect_refusal("large audit component digest drift", validate_barge_audit,
                   large_audit, private=private_root, host={})
    with large_source.open("r+b") as large_file:
        large_file.truncate(MAX_AUDIT_SOURCE_BYTES + 1)
    expect_refusal("audit component exceeds source bound", validate_barge_audit,
                   large_audit, private=private_root, host={})
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
    admission_large = private_write(private_root, Path("bin/admission-large"), b"a" * (MAX_DOC_BYTES + 1))
    admission_hash = hashlib.sha256(b"a" * (MAX_DOC_BYTES + 1)).hexdigest()
    for field in ("admission_executables", "admission_runtime_files"):
        large_binding = json.loads(canonical_json(good_audit))
        large_binding[field] = [{"path": str(admission_large), "sha256": admission_hash}]
        expect_ok(f"large {field} accepted", audited_admission, inherited, large_binding)
        large_binding[field][0]["sha256"] = "0" * 64
        expect_refusal(f"large {field} digest drift", audited_admission, inherited, large_binding)
        large_binding[field][0]["sha256"] = admission_hash
        with admission_large.open("r+b") as file:
            file.truncate(MAX_ADMISSION_ARTIFACT_BYTES + 1)
        expect_refusal(f"oversized {field} refused", audited_admission, inherited, large_binding)
        with admission_large.open("r+b") as file:
            file.truncate(MAX_DOC_BYTES + 1)
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
    with_home = dict(no_rustup, HOME="/invoking-home")
    fallback = expect_ok("invoking HOME rustup fallback", child_environment, with_home,
                         private=private_root, oxide=private_root / "bin/oxide",
                         rustc=Path("/rustup/bin/rustc"), wrapper_hash=host["wrapper_sha256"],
                         pool_cap=268435456, admission_env=admission_pair)
    if fallback is not None and fallback["RUSTUP_HOME"] != "/invoking-home/.rustup":
        failures.append("rustup fallback did not preserve invoking HOME")
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

    # Bound audit inputs cannot redirect or drift while copied.
    audit_stage = fixture.root / "audit-stage"
    audit_stage.mkdir(mode=0o700)
    audit_input = fixture.root / "audit-input"
    audit_input.write_bytes(b"source evidence")
    item = {"source": str(audit_input), "destination": "effigy-source/admission.rs",
            "sha256": sha256_file(audit_input)}
    expect_ok("audit source copied by digest", stage_audit_inputs, audit_stage, {"inputs": [item]})
    bad_input = dict(item, destination="../escape", sha256="0" * 64)
    expect_refusal("audit input escapes root", stage_audit_inputs, audit_stage, {"inputs": [bad_input]})
    bad_input = dict(item, destination="toolchain/drift", sha256="0" * 64)
    expect_refusal("audit input digest drift", stage_audit_inputs, audit_stage, {"inputs": [bad_input]})

    # Persistent claim survives a new evidence directory and refuses replay.
    token_dir = fixture.root / "issued"
    token_dir.mkdir(mode=0o700)
    issued = token_dir / "block.json"
    issued.write_bytes(b"issued-block")
    os.chmod(issued, 0o600)
    issued_digest = sha256_file(issued)
    token = token_dir / "block.json.token"
    token.write_text(issued_digest + "\n")
    os.chmod(token, 0o600)
    expect_ok("issued token claimed", claim_issued_token, issued, issued_digest, "1" * 32)
    expect_refusal("issued token replay refused", claim_issued_token, issued, issued_digest, "2" * 32)

    # Unrecorded intervals and overlapping bootstrap observations still consume
    # only one wall-clock allowance, including work outside named phases.
    from unittest.mock import patch
    with patch(__name__ + ".now_ns") as clock:
        clock.return_value = 10_000_000_000
        wall = Allowance(1200, None, None)
        wall.begin_bootstrap()
        clock.return_value += 5_000_000_000
        wall.begin("bootstrap")
        clock.return_value += 10_000_000_000
        wall.finish("bootstrap")
        wall.finish_bootstrap()
        clock.return_value += 7_000_000_000
        if wall.remaining_seconds() != 1178:
            failures.append("overlap or unrecorded work changed wall-clock charge")

    events = []
    expect_ok("sentinels precede status", run_status_after_sentinels,
              lambda: events.append("sentinels"), lambda: events.append("status"))
    if events != ["sentinels", "status"]:
        failures.append("status ran before sentinels")
    events.clear()
    def failed_sentinels():
        raise GateRefusal("synthetic sentinel refusal")
    expect_refusal("failed sentinel prevents status", run_status_after_sentinels,
                   failed_sentinels, lambda: events.append("status"))
    if events:
        failures.append("status ran after sentinel refusal")
    if inactive_roots([private_root, historic_root], [oxide_run_root],
                      private_root, oxide_run_root) != [historic_root]:
        failures.append("active roots included in retained quiescence bracket")

    # --- allowance: exhaustion, reset and replay refusals -------------------
    allowance = Allowance(1200, None, None)
    allowance.begin("setup")
    allowance.finish("setup")
    expect_ok("allowance tracks spend", lambda: allowance.remaining_seconds() <= 1200)
    expect_refusal("bootstrap deadline before opening", allowance.bootstrap_deadline_ns)
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
    expect_ok("inventory order does not alter equality", verify_inventory, before, list(reversed(before)))
    expect_refusal("duplicate inventory paths refuse", verify_inventory, before + [before[0]], before)
    expect_refusal("inventory inequality refuses", verify_inventory, before, after)
    failure_evidence = private_dir(private_root, Path("inventory-evidence"))
    expect_refusal("inventory mismatch preserves evidence", verify_inventory,
                   before, after, "bootstrap", failure_evidence)
    recorded = json.loads((failure_evidence / "live-inventory-bootstrap.json").read_bytes())
    if (recorded["before"] != before or recorded["after"] != after
            or recorded["difference"]["changed_rows"] < 1):
        failures.append("inventory refusal did not preserve the actual comparison")
    try:
        verify_inventory(before, after, "bootstrap")
        failures.append("inventory diagnostic accepted a change")
    except GateRefusal as refusal:
        message = str(refusal)
        if ("bootstrap" not in message or "metadata" not in message
                or "path_sha256" not in message or "pools.json" in message):
            failures.append("inventory diagnostic lost its bounded value-free change facts")
    synthetic_before = [[f"./private-{i}", [1, i, 0, 1, 1], "digest"] for i in range(12)]
    try:
        verify_inventory(synthetic_before, [], "private-status")
        failures.append("removed inventory rows were accepted")
    except GateRefusal as refusal:
        detail = json.loads(str(refusal).split("; ", 2)[2])
        if (detail["changed_rows"] != 12 or detail["omitted"] != 4
                or len(detail["rows"]) != 8 or any(row["kind"] != "removed" for row in detail["rows"])):
            failures.append("inventory diagnostic did not bound removed rows")
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
    complete_diagnostic = ["--diagnostic", "--oxide-repo", "/r", "--oxide-head", "a" * 40,
                           "--oxide-product-head", "b" * 40, "--barge-repo", "/b",
                           "--barge-head", "c" * 40, "--barge-root", str(BARGE_SCRATCH / "x"),
                           "--oxide-root", str(OXIDE_SCRATCH / "x"), "--expect-audit", "d" * 64,
                           "--expect-ledger", "e" * 64, "--expect-budget", "f" * 64,
                           "--expect-history", "1" * 64, "--expect-oxide-binding", "2" * 64,
                           "--preparation-receipt", "/p/receipt.json",
                           "--expect-preparation", "3" * 64, "--expect-source-binding", "4" * 64]
    if expect_ok("complete diagnostic arguments parse", parse_args, complete_diagnostic) is None:
        failures.append("complete diagnostic arguments refused")
    binding_flag = complete_diagnostic.index("--expect-source-binding")
    expect_refusal("diagnostic without the source binding expectation", parse_args,
                   complete_diagnostic[:binding_flag] + complete_diagnostic[binding_flag + 2:])
    expect_refusal("non-hex source binding expectation", parse_args,
                   complete_diagnostic[:binding_flag + 1] + ["nothex"])

    # --- clamp arithmetic (review 6584174560 finding 2; af038a7e v2 grants) -
    historic_retained = 5591 * 1024 * 1024 + 838_860
    expect_ok("historical 552 MiB clamp at the reported retained total", clamp_mib,
              HISTORIC_OXIDE_CAP_BYTES, historic_retained, BOOTSTRAP_MAX_MIB)
    if clamp_mib(HISTORIC_OXIDE_CAP_BYTES, historic_retained, BOOTSTRAP_MAX_MIB) != 552:
        failures.append("the historical 6 GiB clamp figure moved")
    if clamp_mib(OXIDE_CAP_BYTES, historic_retained, BOOTSTRAP_MAX_MIB) != BOOTSTRAP_MAX_MIB:
        failures.append("the prospective 8 GiB grant must still clamp to the per-run bootstrap cap")
    if clamp_mib(BARGE_CAP_BYTES, 2 * 1024 * 1024 * 1024 + 512 * 1024 * 1024,
                 BOOTSTRAP_MAX_MIB) != 512:
        failures.append("the prospective Barge grant did not clamp to its measured remainder")
    expect_refusal("clamp with retained at the full historical grant", clamp_mib,
                   HISTORIC_OXIDE_CAP_BYTES, HISTORIC_OXIDE_CAP_BYTES, BOOTSTRAP_MAX_MIB)
    expect_refusal("clamp over the prospective Barge grant", clamp_mib,
                   BARGE_CAP_BYTES, BARGE_CAP_BYTES + 1, BOOTSTRAP_MAX_MIB)
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

    # The size query is an allocation estimate, not an exact row. The final
    # PID observation remains strict, and kernel errors never become "gone".
    class FakeKinfo:
        def __init__(self, *, estimate=1296, read_errno=0, read_failures=0,
                     returned=648, observed_pid=42, size_errno=0):
            self.estimate, self.read_errno = estimate, read_errno
            self.read_failures, self.returned = read_failures, returned
            self.observed_pid, self.size_errno = observed_pid, size_errno
            self.probes = self.reads = 0

        def sysctl(self, mib, count, output, size_ptr, new, newlen):
            size = ctypes.cast(size_ptr, ctypes.POINTER(ctypes.c_size_t))
            if output is None:
                self.probes += 1
                size.contents.value = self.estimate
                if self.size_errno:
                    ctypes.set_errno(self.size_errno)
                    return -1
                return 0
            self.reads += 1
            if self.reads <= self.read_failures:
                ctypes.set_errno(self.read_errno)
                return -1
            raw = bytearray(max(648, self.returned))
            struct.pack_into("<qi", raw, 0, 12, 34)
            struct.pack_into("<i", raw, 40, self.observed_pid)
            struct.pack_into("<i", raw, 560, 10)
            struct.pack_into("<i", raw, 564, 42)
            ctypes.memmove(output, bytes(raw), min(len(raw), len(output)))
            size.contents.value = self.returned
            return 0

    def exercise_kinfo(fake):
        from unittest.mock import patch
        with patch(__name__ + ".ctypes.CDLL", return_value=fake):
            return process_row(42)

    oversized_estimate = FakeKinfo()
    row = expect_ok("larger size estimate with one exact row", exercise_kinfo, oversized_estimate)
    if row is not None and (row["pid"], row["start"]) != (42, (12, 34)):
        failures.append("process row did not preserve identity")
    transient = FakeKinfo(read_errno=errno.ENOMEM, read_failures=1)
    expect_ok("transient ENOMEM retries once", exercise_kinfo, transient)
    if transient.reads != 2 or transient.probes != 2:
        failures.append("process row did not re-probe after ENOMEM")
    expect_refusal("process size syscall failure", exercise_kinfo, FakeKinfo(size_errno=errno.EPERM))
    expect_refusal("process read syscall failure", exercise_kinfo,
                   FakeKinfo(read_errno=errno.EPERM, read_failures=1))
    expect_refusal("process ENOMEM retry exhaustion", exercise_kinfo,
                   FakeKinfo(read_errno=errno.ENOMEM, read_failures=3))
    expect_refusal("zero process row remains unknown", exercise_kinfo, FakeKinfo(returned=0))
    expect_refusal("multiple process rows remain unknown", exercise_kinfo, FakeKinfo(returned=1296))
    expect_refusal("wrong process identity remains unknown", exercise_kinfo, FakeKinfo(observed_pid=43))
    expect_refusal("unbounded process size estimate", exercise_kinfo, FakeKinfo(estimate=1024 * 1024 + 1))
    for fake, expected in ((FakeKinfo(size_errno=errno.EPERM), f"errno={errno.EPERM} size=1296"),
                           (FakeKinfo(returned=0), "size=0 expected=648"),
                           (FakeKinfo(read_errno=errno.EPERM, read_failures=1), f"errno={errno.EPERM} size=")):
        try:
            exercise_kinfo(fake)
            failures.append("process diagnostic control unexpectedly succeeded")
        except GateRefusal as refusal:
            if expected not in str(refusal):
                failures.append("process refusal omitted bounded syscall diagnostics")


    # Real kernel-fact positive control: no process spawn, no mocked path.
    kernel_image = expect_ok("current process kernel image", process_executable, os.getpid())
    if kernel_image is not None:
        expect_ok("same kernel process image", require_same_interpreter, os.getpid())

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
        expect_refusal("forged supervisor environment alone refuses", supervisor_context)
        # Pure positive and mismatch controls use fabricated kernel observations,
        # never a live proof or child process.
        from unittest.mock import patch
        supervisor_file = fixture.root / "bounded-test.py"
        supervisor_file.write_bytes(b"mock")
        proof_argv = [sys.executable, sys.argv[0], *sys.argv[1:]]
        parent_argv = [sys.executable, str(supervisor_file), "--limit", "1200",
                       "--scratch-budget-mib", "512", "--", *proof_argv]
        start = now_ns() - 1_000_000_000
        os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(start + 1200 * 1_000_000_000)
        receipt = {"phase": "started: incomplete", "run_id": "a" * 32,
                   "command": proof_argv, "evidence": str(evidence_dir),
                   "scratch": str(evidence_dir), "limit_s": 1200, "start_ns": start}
        (evidence_dir / "receipt.json").write_bytes(canonical_json(receipt))
        me = {"pid": os.getpid(), "ppid": os.getppid(), "pgid": os.getpid(),
              "zombie": False, "start": (1, 1)}
        parent = {"pid": os.getppid(), "ppid": 1, "pgid": os.getppid(),
                  "zombie": False, "start": (1, 0)}
        with patch(__name__ + ".process_row", side_effect=lambda pid: me if pid == os.getpid() else parent), \
             patch(__name__ + ".process_argv", return_value=parent_argv), \
             patch(__name__ + ".process_executable", return_value=Path(sys.executable).resolve(strict=True)), \
             patch(__name__ + ".sha256_file", return_value=SUPERVISOR_SHA256):
            expect_ok("bound direct supervisor and receipt", supervisor_context)
            os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(start + 1250 * 1_000_000_000)
            expect_ok("granted admission extension retains actual start", supervisor_context)
            os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(start + 1200 * 1_000_000_000)
            os.environ["OXIDE_PROOF_RUN_ID"] = "b" * 32
            expect_refusal("foreign supervisor run id", supervisor_context)
            os.environ["OXIDE_PROOF_RUN_ID"] = "a" * 32
            os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(start + 1199 * 1_000_000_000)
            expect_refusal("foreign supervisor deadline", supervisor_context)

        # The actual issued form uses a 120 s/128 MiB direct supervisor with
        # --prepare and a complete argument tail; all observations are mocks.
        issued_tail = ["--prepare", "--packet-dir", "/fixture/packet",
                       "--oxide-repo", "/fixture/oxide", "--oxide-head", OXIDE_PIN_COMMIT,
                       "--oxide-product-head", "6e8ca1f07a3e26be9e96ff55fd774b0f8185165a",
                       "--barge-repo", "/fixture/barge", "--barge-head", BARGE_ACCEPTED_HEAD,
                       "--barge-root", "/fixture/barge-root", "--oxide-root", "/fixture/oxide-root",
                       "--audit-inputs", "/fixture/audit.json", "--expect-audit-inputs", "a" * 64]
        issued_child = [sys.executable, sys.argv[0], *issued_tail]
        other_supervisor = fixture.root / "not-bounded.py"
        other_supervisor.write_bytes(b"mock")
        other_script = fixture.root / "other-script.py"
        other_script.write_bytes(b"mock")
        issued_parent = [sys.executable, str(supervisor_file), "--limit", "120",
                         "--scratch-budget-mib", "128", "--", *issued_child]
        issued_receipt = dict(receipt, command=issued_child, limit_s=120)
        (evidence_dir / "receipt.json").write_bytes(canonical_json(issued_receipt))
        os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(start + 120 * 1_000_000_000)
        real_process_executable = process_executable
        with patch.object(sys, "argv", [sys.argv[0], *issued_tail]), \
             patch(__name__ + ".process_row", side_effect=lambda pid: me if pid == os.getpid() else parent), \
             patch(__name__ + ".process_argv", return_value=issued_parent) as parent_args, \
             patch(__name__ + ".process_executable",
                   side_effect=lambda pid: real_process_executable(pid) if pid == os.getpid() else kernel_image) as parent_exe, \
             patch(__name__ + ".sha256_file", return_value=SUPERVISOR_SHA256) as supervisor_digest:
            expect_ok("issued 120s 128MiB preparation shape", supervisor_context)
            controls = [
                ("interpreter executable mismatch", None, "executable"),
                ("supervisor digest mismatch", None, "digest"),
                ("supervisor filename mismatch", (1, str(other_supervisor)), None),
                ("limit flag mismatch", (2, "--wrong-limit"), None),
                ("invalid limit value", (3, "1201"), None),
                ("scratch flag mismatch", (4, "--wrong-scratch"), None),
                ("invalid scratch budget", (5, "1025"), None),
                ("child script mismatch", (8, str(other_script)), None),
                ("child argument mismatch", (-1, "changed"), None),
            ]
            for expected, change, special in controls:
                changed_argv = list(issued_parent)
                if change is not None:
                    changed_argv[change[0]] = change[1]
                parent_args.return_value = changed_argv
                parent_exe.side_effect = (lambda pid: real_process_executable(pid) if pid == os.getpid()
                                          else supervisor_file if special == "executable" else kernel_image)
                supervisor_digest.return_value = "0" * 64 if special == "digest" else SUPERVISOR_SHA256
                try:
                    supervisor_context()
                    failures.append(f"{expected}: expected refusal")
                except GateRefusal as refusal:
                    if expected not in str(refusal):
                        failures.append(f"{expected}: wrong diagnostic {refusal}")
            parent_args.return_value = issued_parent
            parent_exe.side_effect = lambda pid: real_process_executable(pid) if pid == os.getpid() else kernel_image
            supervisor_digest.return_value = SUPERVISOR_SHA256
            for field, value, message in (("command", ["other"], "receipt command mismatch"),
                                          ("scratch", "/other", "receipt scratch path mismatch"),
                                          ("limit_s", 121, "receipt limit mismatch")):
                mutated_receipt = dict(issued_receipt, **{field: value})
                (evidence_dir / "receipt.json").write_bytes(canonical_json(mutated_receipt))
                try:
                    supervisor_context()
                    failures.append(f"{message}: expected refusal")
                except GateRefusal as refusal:
                    if message not in str(refusal):
                        failures.append(f"{message}: wrong diagnostic {refusal}")

    finally:
        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

    # --- bootstrap provenance (oxide#253) ----------------------------------
    # A successful bootstrap's provenance packet survives a later real
    # validation failure, and missing, malformed and escaped artifacts refuse
    # with the packet retained. Mocked counterfactuals only.
    def provenance_case(name):
        root = fixture.root / f"provenance-{name}"
        root.mkdir()
        os.chmod(root, 0o700)
        source = root / "oxide-source"
        source.mkdir()
        os.chmod(source, 0o700)
        (source / "target/debug").mkdir(parents=True)
        return root, source

    def provenance_inputs_for(root, source, *, env=None, argv=None):
        return {"run_id": "b" * 32, "private_root": root,
                "account_root": fixture.root / "account-not-real",
                "argv": list(BARGE_BOOTSTRAP_ARGV) if argv is None else argv,
                "cwd": source,
                "env": env or {"HOME": str(root / "home"),
                               "CARGO_HOME": str(root / "cache"),
                               "RUSTUP_TOOLCHAIN": TOOLCHAIN},
                "inherited_keys": ["HOME", "CARGO_HOME", "MY_SECRET_TOKEN"],
                "tools": {"python": {"path": str(sys.executable), "bytes": 1,
                                     "sha256": "a" * 64}},
                "sources": {"oxide_pin_commit": OXIDE_PIN_COMMIT},
                "seed": {"source_cargo_home": "/mock/cargo-home",
                         "cache_root": str(root / "cache")},
                "limit_seconds": 300, "scratch_mib": 512,
                "pool_cap_bytes": 1024 * 1024}

    provenance_guard = {"state": "closed", "run_id": "c" * 32}
    provenance_allowance = {"bootstrap_started_ns": 1,
                            "bootstrap_max_seconds": BOOTSTRAP_MAX_SECONDS}

    def artifact_line(path, source):
        return canonical_json({"reason": "compiler-artifact",
                               "executable": str(path),
                               "target": {"name": "oxide", "kind": ["bin"],
                                          "src_path": str(source / "src/bin/oxide/main.rs")}}).decode().rstrip("\n")

    success_root, success_source = provenance_case("success")
    success_artifact = success_source / "target/debug/oxide"
    success_artifact.write_bytes(b"mock-oxide-artifact")
    good_log = "\n".join((
        artifact_line(success_artifact, success_source),
        "   Compiling oxide-storage v0.1.0",
        "not json at all",
        "[1,2]",
        artifact_line(success_source / "target/debug/other", fixture.root),
        canonical_json({"reason": "build-script-executed"}).decode().rstrip("\n"),
    )) + "\n"
    ambient_clean = ambient_cargo_config_scan(
        success_source, private_root=success_root,
        cargo_home=success_root / "cache")
    if ambient_clean["violations"] or not ambient_clean["chain_complete"] \
            or ambient_clean["inside_private_levels"] < 2:
        failures.append("clean ancestor scan recorded a violation or an incomplete chain")
    success_facts = expect_ok("single artifact facts", resolve_bootstrap_artifact,
                              good_log, provenance_inputs_for(success_root, success_source),
                              guard=provenance_guard, ambient=ambient_clean,
                              allowance=provenance_allowance)
    if success_facts is not None:
        if (success_facts["candidate_count"] != 1 or success_facts["violations"]
                or success_facts["malformed_lines"] != 3
                or len(success_facts["oxide_bin_entries"]) != 2):
            failures.append(f"artifact facts mis-parsed: {success_facts}")
        elif success_facts["artifact"]["sha256"] != sha256_bytes(b"mock-oxide-artifact"):
            failures.append("artifact digest mismatch")
        else:
            document = build_bootstrap_provenance(
                provenance_inputs_for(success_root, success_source), guard=provenance_guard,
                artifact={"state": "resolved", "sha256": success_facts["artifact"]["sha256"],
                          "symlinked_to": str(success_root / "bin/oxide")},
                ambient=ambient_clean, allowance=provenance_allowance, written_ns=now_ns())
            write_bootstrap_provenance(success_root, document)
            before_rows = [["/x", [1, 1, 1, 1, 1], "d"]]
            after_rows = [["/x", [1, 2, 1, 1, 1], "d"]]
            try:
                verify_inventory(before_rows, after_rows, "bootstrap",
                                 success_root / "evidence")
                failures.append("real inventory failure after provenance: expected a refusal")
            except GateRefusal as refusal:
                if "isolation failed" not in str(refusal):
                    failures.append("real inventory failure after provenance: "
                                    f"wrong diagnostic {refusal}")
            retained = json.loads((success_root / PROVENANCE_PATH).read_bytes())
            if (retained.get("kind") != "bootstrap-provenance"
                    or retained.get("artifact", {}).get("sha256")
                    != success_facts["artifact"]["sha256"]
                    or retained.get("guard") != provenance_guard
                    or retained.get("child_environment", {}).get(
                        "live_account_reachability", {}).get("state") != "none"
                    or "MY_SECRET_TOKEN" not in retained.get(
                        "child_environment", {}).get("inherited_key_names", [])):
                failures.append("retained provenance lost its bounded facts after refusal")
            if b"s3cr3t-value" in (success_root / PROVENANCE_PATH).read_bytes():
                failures.append("provenance packet recorded an inherited secret value")
    expect_refusal("provenance argv drift refuses", build_bootstrap_provenance,
                   provenance_inputs_for(success_root, success_source,
                                         argv=["cargo", "build"]),
                   guard=provenance_guard, artifact={"state": "not-built"},
                   ambient=ambient_clean, allowance=provenance_allowance,
                   written_ns=now_ns())
    expect_refusal("provenance env outside the audited allowlist refuses",
                   build_bootstrap_provenance,
                   provenance_inputs_for(success_root, success_source,
                                         env={"HOME": "/x", "MY_SECRET_TOKEN": "s3cr3t-value"}),
                   guard=provenance_guard, artifact={"state": "not-built"},
                   ambient=ambient_clean, allowance=provenance_allowance,
                   written_ns=now_ns())
    outside_inputs = provenance_inputs_for(success_root, success_source)
    outside_inputs["cwd"] = fixture.root
    expect_refusal("provenance cwd outside the private root refuses",
                   build_bootstrap_provenance, outside_inputs,
                   guard=provenance_guard, artifact={"state": "not-built"},
                   ambient=ambient_clean, allowance=provenance_allowance,
                   written_ns=now_ns())
    reach_root = fixture.root / "account-not-real"
    reach_root.mkdir()
    reach_document = build_bootstrap_provenance(
        provenance_inputs_for(success_root, success_source,
                              env={"HOME": str(reach_root / "leak")}),
        guard=provenance_guard, artifact={"state": "not-built"},
        ambient=ambient_clean, allowance=provenance_allowance, written_ns=now_ns())
    if reach_document["child_environment"]["live_account_reachability"].get("state") != "recorded" \
            or not reach_document["child_environment"]["live_account_reachability"].get("values"):
        failures.append("live-account reachability was not recorded")
    path_reach = build_bootstrap_provenance(
        provenance_inputs_for(success_root, success_source,
                              env={"HOME": str(success_root / "home"),
                                   "PATH": "/bin:" + str(reach_root / "tools")}),
        guard=provenance_guard, artifact={"state": "not-built"},
        ambient=ambient_clean, allowance=provenance_allowance, written_ns=now_ns())
    if path_reach["child_environment"]["live_account_reachability"].get("state") != "recorded":
        failures.append("PATH component reachability was not recorded")
    oversized = build_bootstrap_provenance(
        provenance_inputs_for(success_root, success_source), guard=provenance_guard,
        artifact={"state": "not-built"}, ambient=ambient_clean,
        allowance=provenance_allowance, written_ns=now_ns())
    oversized["padding"] = "x" * (MAX_PROVENANCE_BYTES + 1)
    expect_refusal("oversized provenance refuses without retention",
                   write_bootstrap_provenance, success_root, oversized)
    retained_path = success_root / PROVENANCE_PATH
    if not retained_path.is_file():
        failures.append("oversized provenance check: success packet was never retained")
    elif b"padding" in retained_path.read_bytes():
        failures.append("oversized provenance replaced the retained packet")
    scrubbed = child_environment(
        {"HOME": "/real/home", "RUSTUP_TOOLCHAIN": TOOLCHAIN,
         "RUSTUP_HOME": "/real/rustup", "PATH": "/usr/bin:/bin",
         "OXIDE_CONFIG": "/leak/config.toml", "MY_SECRET_TOKEN": "s3cr3t-value",
         "EFFIGY_ADMISSION_HOOK": "/hook", "QUEUE_ADMISSION_SOCKET": "/socket"},
        private=success_root, oxide=success_root / "bin/oxide",
        rustc=fixture.root / "rustc", wrapper_hash="w" * 64, pool_cap=1024,
        admission_env={"EFFIGY_ADMISSION_HOOK": "/hook",
                       "QUEUE_ADMISSION_SOCKET": "/socket"})
    if (set(scrubbed) - BOOTSTRAP_ENV_KEYS or "MY_SECRET_TOKEN" in scrubbed
            or scrubbed.get("OXIDE_CONFIG") != str(success_root / "config.toml")
            or scrubbed.get("HOME") != str(success_root / "home")):
        failures.append("scrubbed child environment left the audited allowlist")

    # Ambient Cargo configuration above the private root refuses before the
    # launch, with the finding retained; a planted configuration refuses too.
    (fixture.root / ".cargo").mkdir()
    (fixture.root / ".cargo/config.toml").write_bytes(b'[alias]\nx = "y"\n')
    ambient_root, ambient_source = provenance_case("ambient")
    ambient_bad = ambient_cargo_config_scan(ambient_source, private_root=ambient_root)
    if not any(v.startswith("ambient-cargo-config:") for v in ambient_bad["violations"]):
        failures.append("ambient configuration scan missed the ancestor config")
    expect_refusal("ambient config refuses before launch", refuse_bootstrap_launch,
                   provenance_inputs_for(ambient_root, ambient_source),
                   ambient_bad, {"bootstrap_started_ns": 5})
    ambient_packet = json.loads((ambient_root / PROVENANCE_PATH).read_bytes())
    if (ambient_packet.get("guard") != {"state": "not-launched"}
            or ambient_packet.get("artifact") != {"state": "not-built"}
            or not ambient_packet.get("ambient", {}).get("violations")):
        failures.append("pre-launch refusal packet lost its findings")
    expect_ok("clean ambient scan proceeds", refuse_bootstrap_launch,
              provenance_inputs_for(ambient_root, ambient_source),
              {"violations": []}, {"bootstrap_started_ns": 5})
    planted_root, planted_source = provenance_case("planted")
    (planted_root / ".cargo").mkdir()
    (planted_root / ".cargo/config.toml").write_bytes(b"")
    planted = ambient_cargo_config_scan(planted_source, private_root=planted_root)
    kinds = {v.split(":", 1)[0] for v in planted["violations"]}
    if kinds != {"ambient-cargo-config", "planted-cargo-config"}:
        failures.append(f"planted configuration scan mis-recorded: {planted['violations']}")
    cargo_home_root, cargo_home_source = provenance_case("cargo-home")
    cache = cargo_home_root / "cache"
    cache.mkdir()
    (cache / "config.toml").write_bytes(b"[build]\njobs = 1\n")
    cargo_home_scan = ambient_cargo_config_scan(
        cargo_home_source, private_root=cargo_home_root, cargo_home=cache)
    if not any(v.startswith("planted-cargo-home-config:") for v in cargo_home_scan["violations"]):
        failures.append("CARGO_HOME config scan missed the private cache config")
    outside_cache = fixture.root / "host-cargo-home"
    outside_cache.mkdir()
    (outside_cache / "config.toml").write_bytes(b"[build]\njobs = 2\n")
    outside_home_scan = ambient_cargo_config_scan(
        cargo_home_source, private_root=cargo_home_root, cargo_home=outside_cache)
    if not any(v.startswith("ambient-cargo-config:") and "host-cargo-home" in v
               for v in outside_home_scan["violations"]):
        failures.append("outside CARGO_HOME config was not recorded as ambient")

    # Unreadable Cargo config is a violation, never recorded as absent.
    unread_root, unread_source = provenance_case("unreadable-config")
    unread_dir = unread_root / ".cargo"
    unread_dir.mkdir()
    (unread_dir / "config.toml").write_bytes(b'[alias]\nx = "y"\n')
    os.chmod(unread_dir, 0o600)
    try:
        unread_scan = ambient_cargo_config_scan(
            unread_source, private_root=unread_root)
        unread_entries = [entry for entry in unread_scan["entries"]
                          if entry["path"] == str(unread_dir / "config.toml")]
        if not unread_entries or unread_entries[0].get("state") != "unreadable":
            failures.append("unreadable cargo config recorded as "
                            f"{unread_entries or 'missing'}")
        if not any(v.startswith("unreadable-cargo-config:")
                   and str(unread_dir / "config.toml") in v
                   for v in unread_scan["violations"]):
            failures.append("unreadable cargo config produced no violation")
        expect_refusal("unreadable cargo config refuses before launch",
                       refuse_bootstrap_launch,
                       provenance_inputs_for(unread_root, unread_source),
                       unread_scan, {"bootstrap_started_ns": 5})
    finally:
        os.chmod(unread_dir, 0o700)

    # PATH-resolved host tools without a private PATH-slot binding use the
    # existing pre-launch refusal. Private-bin first match is a binding.
    host_tools = fixture.root / "host-tools"
    host_tools.mkdir()
    (host_tools / "cc").write_bytes(b"fake-cc")
    os.chmod(host_tools / "cc", 0o755)
    path_root, path_source = provenance_case("path-ambient")
    (path_root / "bin").mkdir()
    ambient_path = attach_path_executables(
        {"violations": []},
        str(path_root / "bin") + os.pathsep + str(host_tools),
        private_root=path_root)
    cc_entries = [entry for entry in ambient_path["path_executables"]["entries"]
                  if entry["name"] == "cc"]
    if not cc_entries or cc_entries[0].get("state") != "ambient":
        failures.append(f"host PATH cc recorded as {cc_entries or 'missing'}")
    if not any(v.startswith("ambient-executable:cc:") for v in ambient_path["violations"]):
        failures.append("host PATH cc produced no ambient-executable violation")
    expect_refusal("host PATH cc refuses before launch", refuse_bootstrap_launch,
                   provenance_inputs_for(path_root, path_source),
                   ambient_path, {"bootstrap_started_ns": 5})
    priv_root, priv_source = provenance_case("path-private")
    priv_bin = priv_root / "bin"
    priv_bin.mkdir()
    (priv_bin / "cc").write_bytes(b"private-cc")
    os.chmod(priv_bin / "cc", 0o755)
    private_path = attach_path_executables(
        {"violations": []},
        str(priv_bin) + os.pathsep + str(host_tools),
        private_root=priv_root)
    priv_cc = [entry for entry in private_path["path_executables"]["entries"]
               if entry["name"] == "cc"]
    if not priv_cc or priv_cc[0].get("state") != "private":
        failures.append(f"private PATH cc recorded as {priv_cc or 'missing'}")
    if any(v.startswith("ambient-executable:cc:") for v in private_path["violations"]):
        failures.append("private PATH cc was treated as ambient")
    expect_ok("private PATH cc proceeds", refuse_bootstrap_launch,
              provenance_inputs_for(priv_root, priv_source),
              private_path, {"bootstrap_started_ns": 5})
    inner_root, inner_source = provenance_case("path-inner-symlink")
    inner_bin = inner_root / "bin"
    inner_bin.mkdir()
    inner_tools = inner_root / "tools"
    inner_tools.mkdir()
    (inner_tools / "cc").write_bytes(b"inner-cc")
    os.chmod(inner_tools / "cc", 0o755)
    os.symlink(inner_tools / "cc", inner_bin / "cc")
    inner_path = attach_path_executables(
        {"violations": []},
        str(inner_bin) + os.pathsep + str(host_tools),
        private_root=inner_root)
    inner_cc = [entry for entry in inner_path["path_executables"]["entries"]
                if entry["name"] == "cc"]
    if not inner_cc or inner_cc[0].get("state") != "private":
        failures.append(f"inner PATH cc symlink recorded as {inner_cc or 'missing'}")
    expect_ok("inner PATH cc symlink proceeds", refuse_bootstrap_launch,
              provenance_inputs_for(inner_root, inner_source),
              inner_path, {"bootstrap_started_ns": 5})
    link_root, link_source = provenance_case("path-host-symlink")
    link_bin = link_root / "bin"
    link_bin.mkdir()
    os.symlink(host_tools / "cc", link_bin / "cc")
    link_path = attach_path_executables(
        {"violations": []},
        str(link_bin) + os.pathsep + str(host_tools),
        private_root=link_root)
    link_cc = [entry for entry in link_path["path_executables"]["entries"]
               if entry["name"] == "cc"]
    if not link_cc or link_cc[0].get("state") != "ambient":
        failures.append(f"host PATH cc symlink recorded as {link_cc or 'missing'}")
    expect_refusal("host PATH cc symlink refuses before launch",
                   refuse_bootstrap_launch,
                   provenance_inputs_for(link_root, link_source),
                   link_path, {"bootstrap_started_ns": 5})
    dang_root, dang_source = provenance_case("path-dangling")
    dang_bin = dang_root / "bin"
    dang_bin.mkdir()
    os.symlink("nowhere", dang_bin / "cc")
    dang_path = attach_path_executables(
        {"violations": []},
        str(dang_bin) + os.pathsep + str(host_tools),
        private_root=dang_root)
    dang_cc = [entry for entry in dang_path["path_executables"]["entries"]
               if entry["name"] == "cc"]
    if not dang_cc or dang_cc[0].get("state") != "ambient":
        failures.append(f"dangling PATH cc recorded as {dang_cc or 'missing'}")
    expect_refusal("dangling PATH cc continues to host and refuses",
                   refuse_bootstrap_launch,
                   provenance_inputs_for(dang_root, dang_source),
                   dang_path, {"bootstrap_started_ns": 5})
    quiet_root, _quiet_source = provenance_case("path-dangling-quiet")
    quiet_bin = quiet_root / "bin"
    quiet_bin.mkdir()
    os.symlink("nowhere", quiet_bin / "cc")
    quiet_scan = path_executable_scan(str(quiet_bin), private_root=quiet_root)
    quiet_cc = [entry for entry in quiet_scan["entries"] if entry["name"] == "cc"]
    if not quiet_cc or quiet_cc[0].get("state") != "absent" or quiet_scan["violations"]:
        failures.append(f"dangling PATH cc without host recorded as {quiet_cc}")
    noexec_root, noexec_source = provenance_case("path-nonexec")
    noexec_bin = noexec_root / "bin"
    noexec_bin.mkdir()
    (noexec_bin / "cc").write_bytes(b"not-exec")
    os.chmod(noexec_bin / "cc", 0o644)
    noexec_path = attach_path_executables(
        {"violations": []},
        str(noexec_bin) + os.pathsep + str(host_tools),
        private_root=noexec_root)
    noexec_cc = [entry for entry in noexec_path["path_executables"]["entries"]
                 if entry["name"] == "cc"]
    if not noexec_cc or noexec_cc[0].get("state") != "ambient":
        failures.append(f"non-executable PATH cc recorded as {noexec_cc or 'missing'}")
    expect_refusal("non-executable PATH cc continues to host and refuses",
                   refuse_bootstrap_launch,
                   provenance_inputs_for(noexec_root, noexec_source),
                   noexec_path, {"bootstrap_started_ns": 5})
    empty_root, empty_source = provenance_case("path-empty-component")
    (empty_root / "cc").write_bytes(b"cwd-cc")
    os.chmod(empty_root / "cc", 0o755)
    empty_path = attach_path_executables(
        {"violations": []}, os.pathsep,
        private_root=empty_root, cwd=empty_root)
    empty_cc = [entry for entry in empty_path["path_executables"]["entries"]
                if entry["name"] == "cc"]
    if not empty_cc or empty_cc[0].get("state") != "private":
        failures.append(f"empty PATH component cwd cc recorded as {empty_cc or 'missing'}")
    expect_ok("empty PATH component private cwd cc proceeds", refuse_bootstrap_launch,
              provenance_inputs_for(empty_root, empty_source),
              empty_path, {"bootstrap_started_ns": 5})
    absent_root, absent_source = provenance_case("path-absent")
    (absent_root / "bin").mkdir()
    absent_path = path_executable_scan(str(absent_root / "bin"),
                                       private_root=absent_root)
    if absent_path["violations"]:
        failures.append(f"absent PATH tools produced violations: {absent_path['violations']}")
    if any(entry["state"] != "absent" for entry in absent_path["entries"]):
        failures.append("absent PATH tools were not recorded absent")
    unread_path_root, unread_path_source = provenance_case("path-unreadable")
    unread_bin = unread_path_root / "bin"
    unread_bin.mkdir()
    (unread_bin / "cc").write_bytes(b"hidden-cc")
    os.chmod(unread_bin, 0o600)
    try:
        unread_path = path_executable_scan(str(unread_bin),
                                           private_root=unread_path_root)
        unread_cc = [entry for entry in unread_path["entries"]
                     if entry["name"] == "cc"]
        if not unread_cc or unread_cc[0].get("state") != "unreadable":
            failures.append(f"unreadable PATH cc recorded as {unread_cc or 'missing'}")
        if not any(v.startswith("unreadable-executable:cc:")
                   for v in unread_path["violations"]):
            failures.append("unreadable PATH cc produced no violation")
        expect_refusal("unreadable PATH cc refuses before launch",
                       refuse_bootstrap_launch,
                       provenance_inputs_for(unread_path_root, unread_path_source),
                       attach_path_executables({"violations": []}, str(unread_bin),
                                               private_root=unread_path_root),
                       {"bootstrap_started_ns": 5})
    finally:
        os.chmod(unread_bin, 0o700)

    # Candidate route (oxide#254): retained-artifact bindings, complete
    # issuance, audited host inputs and the copy/rehash controls, all mocked.
    cand = fixture.root / "candidate-origin"
    cand.mkdir()
    os.chmod(cand, 0o700)
    cand_source = cand / "oxide-source"
    (cand_source / "target/debug").mkdir(parents=True)
    (cand_source / "Cargo.lock").write_bytes(b"lock-bytes")
    (cand_source / "rust-toolchain.toml").write_bytes(b"toolchain-bytes")
    cand_artifact = cand / CANDIDATE_ORIGIN_RELATIVE
    cand_artifact.parent.mkdir(parents=True, exist_ok=True)
    cand_artifact.write_bytes(b"candidate-bytes")
    os.chmod(cand_artifact, 0o755)

    def cand_log_line(path, source):
        return json.dumps({"reason": "compiler-artifact",
                           "target": {"name": "oxide", "kind": ["bin"],
                                      "src_path": str(source / "src/bin/oxide/main.rs")},
                           "executable": str(path)})

    def cand_receipt_for(root):
        return {"command": list(BARGE_BOOTSTRAP_ARGV), "run_id": "c" * 32,
                "limit_s": 299, "start_ns": 1, "published_ns": 2,
                "scratch": str(root),
                "outcome": {"closure": "closed", "status": 0, "fault": None,
                            "deadline": False, "launched": True},
                "census": {"certain": True, "members": 0, "uncertain": 0}}

    def cand_options(root, *, receipt=None, log=None, artifact_sha=None, bytes_=None,
                     lock_sha=None, toolchain_sha=None):
        receipt = cand_receipt_for(root) if receipt is None else receipt
        log_text = (cand_log_line(root / CANDIDATE_ORIGIN_RELATIVE, root / "oxide-source") + "\n") \
            if log is None else log
        return {
            "candidate_receipt": str(root / "receipt.json"),
            "expect_candidate_receipt": sha256_bytes(canonical_json(receipt)),
            "candidate_log": str(root / "log.json"),
            "expect_candidate_log": sha256_bytes(log_text.encode()),
            "candidate_lock_sha256": lock_sha or sha256_bytes(b"lock-bytes"),
            "candidate_toolchain_sha256": toolchain_sha or sha256_bytes(b"toolchain-bytes"),
            "expect_candidate": artifact_sha or sha256_bytes(b"candidate-bytes"),
            "candidate_bytes": bytes_ or len(b"candidate-bytes"),
        }

    def cand_origin_case(name):
        root = fixture.root / f"cand-origin-{name}"
        root.mkdir()
        os.chmod(root, 0o700)
        source = root / "oxide-source"
        (source / "target/debug").mkdir(parents=True)
        (source / "Cargo.lock").write_bytes(b"lock-bytes")
        (source / "rust-toolchain.toml").write_bytes(b"toolchain-bytes")
        write_doc(root, "receipt.json", cand_receipt_for(root))
        (root / "log.json").write_bytes(
            (cand_log_line(root / CANDIDATE_ORIGIN_RELATIVE, source) + "\n").encode())
        return root

    write_doc(cand, "receipt.json", cand_receipt_for(cand))
    (cand / "log.json").write_bytes(
        (cand_log_line(cand_artifact, cand_source) + "\n").encode())
    good_options = cand_options(cand)
    origin_facts = expect_ok("candidate origin validates", validate_candidate_origin,
                             origin_root=cand, options=good_options, ledger_roots=[cand],
                             pinned_lock_sha256=good_options["candidate_lock_sha256"],
                             pinned_toolchain_sha256=good_options["candidate_toolchain_sha256"])
    if origin_facts is None or origin_facts["artifact"]["sha256"] != good_options["expect_candidate"]:
        failures.append("candidate origin facts lost the artifact binding")
    expect_refusal("candidate origin outside the ledger refuses", validate_candidate_origin,
                   origin_root=cand, options=good_options, ledger_roots=[fixture.root / "other"],
                   pinned_lock_sha256=good_options["candidate_lock_sha256"],
                   pinned_toolchain_sha256=good_options["candidate_toolchain_sha256"])
    expect_refusal("candidate lock binding off the pinned source refuses",
                   validate_candidate_origin, origin_root=cand, options=good_options,
                   ledger_roots=[cand], pinned_lock_sha256=sha256_bytes(b"other-lock"),
                   pinned_toolchain_sha256=good_options["candidate_toolchain_sha256"])
    missing_origin = cand_origin_case("missing-artifact")
    expect_refusal("candidate origin missing artifact refuses", validate_candidate_origin,
                   origin_root=missing_origin, options=cand_options(missing_origin),
                   ledger_roots=[missing_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    swapped_origin = cand_origin_case("symlink-swapped")
    swapped_target = swapped_origin / "elsewhere"
    swapped_target.write_bytes(b"candidate-bytes")
    (swapped_origin / CANDIDATE_ORIGIN_RELATIVE).symlink_to(swapped_target)
    expect_refusal("symlink-swapped candidate refuses", validate_candidate_origin,
                   origin_root=swapped_origin, options=cand_options(swapped_origin),
                   ledger_roots=[swapped_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    changed_origin = cand_origin_case("changed-artifact")
    (changed_origin / CANDIDATE_ORIGIN_RELATIVE).write_bytes(b"changed-bytes")
    expect_refusal("changed candidate digest refuses", validate_candidate_origin,
                   origin_root=changed_origin, options=cand_options(changed_origin),
                   ledger_roots=[changed_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    wrong_size_origin = cand_origin_case("wrong-size")
    expect_refusal("candidate size mismatch refuses", validate_candidate_origin,
                   origin_root=wrong_size_origin,
                   options=cand_options(wrong_size_origin, bytes_=7),
                   ledger_roots=[wrong_size_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    zero_log_origin = cand_origin_case("zero-artifact-log")
    expect_refusal("log without the artifact refuses", validate_candidate_origin,
                   origin_root=zero_log_origin,
                   options=cand_options(zero_log_origin, log="{\"reason\":\"unknown\"}"),
                   ledger_roots=[zero_log_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    double_log_origin = cand_origin_case("double-artifact-log")
    double_log = cand_log_line(double_log_origin / CANDIDATE_ORIGIN_RELATIVE,
                               double_log_origin / "oxide-source") + "\n" + \
        cand_log_line(double_log_origin / "oxide-source/target/debug/other",
                      double_log_origin / "oxide-source")
    expect_refusal("ambiguous candidate log refuses", validate_candidate_origin,
                   origin_root=double_log_origin,
                   options=cand_options(double_log_origin, log=double_log),
                   ledger_roots=[double_log_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    foreign_log_origin = cand_origin_case("foreign-path-log")
    foreign_log = cand_log_line(fixture.root / "outside" / "oxide",
                                foreign_log_origin / "oxide-source")
    expect_refusal("log naming a foreign artifact path refuses", validate_candidate_origin,
                   origin_root=foreign_log_origin,
                   options=cand_options(foreign_log_origin, log=foreign_log),
                   ledger_roots=[foreign_log_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    wrong_receipt_origin = cand_origin_case("wrong-receipt")
    wrong_receipt = cand_receipt_for(wrong_receipt_origin)
    wrong_receipt["outcome"]["closure"] = "UNCLOSED"
    write_doc(wrong_receipt_origin, "receipt.json", wrong_receipt)
    expect_refusal("unclosed candidate guard refuses", validate_candidate_origin,
                   origin_root=wrong_receipt_origin,
                   options=cand_options(wrong_receipt_origin, receipt=wrong_receipt),
                   ledger_roots=[wrong_receipt_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    drifted_receipt_origin = cand_origin_case("drifted-receipt")
    write_doc(drifted_receipt_origin, "receipt.json", cand_receipt_for(drifted_receipt_origin))
    drifted_options = cand_options(drifted_receipt_origin)
    drifted_options["expect_candidate_receipt"] = sha256_bytes(b"not-the-receipt")
    expect_refusal("changed retained guard receipt refuses", validate_candidate_origin,
                   origin_root=drifted_receipt_origin, options=drifted_options,
                   ledger_roots=[drifted_receipt_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    drifted_log_origin = cand_origin_case("drifted-log")
    (drifted_log_origin / "log.json").write_bytes(b"changed log")
    drifted_log_options = cand_options(drifted_log_origin)
    expect_refusal("changed retained build log refuses", validate_candidate_origin,
                   origin_root=drifted_log_origin, options=drifted_log_options,
                   ledger_roots=[drifted_log_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    wrong_lock_origin = cand_origin_case("wrong-lock")
    (wrong_lock_origin / "oxide-source/Cargo.lock").write_bytes(b"different-lock")
    expect_refusal("origin lock drift refuses", validate_candidate_origin,
                   origin_root=wrong_lock_origin, options=cand_options(wrong_lock_origin),
                   ledger_roots=[wrong_lock_origin],
                   pinned_lock_sha256=sha256_bytes(b"lock-bytes"),
                   pinned_toolchain_sha256=sha256_bytes(b"toolchain-bytes"))
    symlink_origin = fixture.root / "cand-origin-symlink"
    symlink_origin.symlink_to(cand)
    expect_refusal("symlinked origin root refuses", validate_candidate_origin,
                   origin_root=symlink_origin, options=good_options, ledger_roots=[cand],
                   pinned_lock_sha256=good_options["candidate_lock_sha256"],
                   pinned_toolchain_sha256=good_options["candidate_toolchain_sha256"])

    # Candidate copy: rehash, exclusive create, owner-executable mode and
    # identity re-checks.
    copy_root = fixture.root / "candidate-copy-root"
    copy_root.mkdir()
    os.chmod(copy_root, 0o700)
    copy_facts = expect_ok("candidate copy rehashes", copy_candidate_artifact,
                           cand_artifact, copy_root, good_options["expect_candidate"],
                           good_options["candidate_bytes"])
    if copy_facts is None or (copy_root / "candidate/oxide").read_bytes() != b"candidate-bytes":
        failures.append("candidate copy lost the artifact bytes")
    if copy_facts is not None and copy_facts["sha256"] != good_options["expect_candidate"]:
        failures.append("candidate copy digest mismatch was not recorded")
    copied_path = copy_root / "candidate/oxide"
    if not copied_path.is_file() or not os.access(copied_path, os.X_OK) \
            or copied_path.stat().st_mode & 0o077:
        failures.append("candidate copy is not an owner-only executable")
    if copy_facts is not None and copy_facts.get("mode") != stat.S_IMODE(copied_path.stat().st_mode):
        failures.append("candidate copy mode fact disagrees with the file")
    noexec_origin = cand_origin_case("noexec-origin")
    (noexec_origin / CANDIDATE_ORIGIN_RELATIVE).write_bytes(b"candidate-bytes")
    os.chmod(noexec_origin / CANDIDATE_ORIGIN_RELATIVE, 0o644)
    expect_refusal("non-executable origin artifact refuses the copy", copy_candidate_artifact,
                   noexec_origin / CANDIDATE_ORIGIN_RELATIVE, fixture.root / "copy-root-noexec",
                   sha256_bytes(b"candidate-bytes"), len(b"candidate-bytes"))
    expect_refusal("second candidate copy refuses", copy_candidate_artifact,
                   cand_artifact, copy_root, good_options["expect_candidate"],
                   good_options["candidate_bytes"])
    expect_refusal("symlinked candidate source refuses the copy", copy_candidate_artifact,
                   swapped_origin / CANDIDATE_ORIGIN_RELATIVE, fixture.root / "copy-root-2",
                   sha256_bytes(b"candidate-bytes"), len(b"candidate-bytes"))
    expect_refusal("candidate copy digest drift refuses", copy_candidate_artifact,
                   cand_artifact, fixture.root / "copy-root-3",
                   sha256_bytes(b"other-bytes"), len(b"candidate-bytes"))

    # Complete issuance only: derivation-style documents refuse.
    cand_digest = good_options["expect_candidate"]
    complete_audit = {"schema": SCHEMA_BARGE_AUDIT, "review_reference": "review",
                      "oxide_sha256": cand_digest}
    complete_block = {"schema": SCHEMA_BARGE_BLOCK, "oxide_sha256": cand_digest,
                      "audit_sha256": sha256_bytes(canonical_json(complete_audit)),
                      "authorization_reference": "candidate-route-v1:issue-ref"}
    expect_ok("complete candidate issuance binds", validate_candidate_issuance,
              complete_block, complete_audit, cand_digest)
    derived_block = dict(complete_block, authorization_reference="derived-artifact-v1:issue")
    expect_refusal("derivation reference refuses in the candidate route",
                   validate_candidate_issuance, derived_block, complete_audit, cand_digest)
    expect_refusal("null candidate digest in the audit refuses",
                   validate_candidate_issuance, complete_block,
                   dict(complete_audit, oxide_sha256=None), cand_digest)
    expect_refusal("block digest mismatch refuses", validate_candidate_issuance,
                   dict(complete_block, oxide_sha256=sha256_bytes(b"other")),
                   complete_audit, cand_digest)

    # Audited host inputs: planner-bound canonical paths plus live digests.
    fake_rustc = fixture.root / "fake-rustc"
    fake_cargo = fixture.root / "fake-cargo"
    fake_rustc.write_bytes(b"fake-rustc-bytes")
    fake_cargo.write_bytes(b"fake-cargo-bytes")
    os.chmod(fake_rustc, 0o755)
    os.chmod(fake_cargo, 0o755)
    not_executable = fixture.root / "not-executable"
    not_executable.write_bytes(b"not-executable-bytes")
    host_cc_path = host_tools / "cc"
    host_tool_binding_cc = {"name": "cc", "path": str(host_cc_path),
                            "sha256": sha256_file(host_cc_path, cap=MAX_ADMISSION_ARTIFACT_BYTES),
                            "bytes": host_cc_path.stat().st_size}

    def toolchain_binding(path):
        info = path.stat()
        return {"path": str(path), "sha256": sha256_file(path, cap=256 * 1024 * 1024),
                "bytes": info.st_size}

    def host_tools_document(tools):
        return {"schema": SCHEMA_HOST_TOOLS, "route": "candidate", "ruling": CANDIDATE_RULING,
                "toolchain": {"rustc": toolchain_binding(fake_rustc),
                              "cargo": toolchain_binding(fake_cargo)},
                "tools": tools}

    expect_ok("host-tools document validates", validate_host_tools_document,
              host_tools_document([host_tool_binding_cc]),
              rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("host-tools schema drift refuses", validate_host_tools_document,
                   dict(host_tools_document([]), schema="other/1"),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("host-tools ruling drift refuses", validate_host_tools_document,
                   dict(host_tools_document([]), ruling="other"),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("unlisted host tool name refuses", validate_host_tools_document,
                   host_tools_document([dict(host_tool_binding_cc, name="make")]),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("duplicate host tool refuses", validate_host_tools_document,
                   host_tools_document([host_tool_binding_cc, host_tool_binding_cc]),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("changed host tool digest refuses", validate_host_tools_document,
                   host_tools_document([dict(host_tool_binding_cc, sha256=sha256_bytes(b"stale"))]),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("changed host tool size refuses", validate_host_tools_document,
                   host_tools_document([dict(host_tool_binding_cc, bytes=1)]),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("non-executable host tool binding refuses", validate_host_tools_document,
                   host_tools_document([dict(host_tool_binding_cc, path=str(not_executable),
                                             sha256=sha256_file(not_executable, cap=MAX_ADMISSION_ARTIFACT_BYTES),
                                             bytes=not_executable.stat().st_size)]),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("toolchain digest drift refuses", validate_host_tools_document,
                   dict(host_tools_document([]),
                        toolchain={"rustc": dict(toolchain_binding(fake_rustc), sha256=sha256_bytes(b"stale")),
                                   "cargo": toolchain_binding(fake_cargo)}),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)
    expect_refusal("toolchain path drift refuses", validate_host_tools_document,
                   dict(host_tools_document([]),
                        toolchain={"rustc": toolchain_binding(fake_cargo),
                                   "cargo": toolchain_binding(fake_cargo)}),
                   rustc_tool=fake_rustc, cargo_tool=fake_cargo)

    host_doc_match = host_tools_document([host_tool_binding_cc])
    reconciled, reconciled_violations = expect_ok(
        "bound host tool reconciles", reconcile_host_tools, ambient_path,
        host_doc_match)
    if reconciled_violations:
        failures.append(f"bound host tool left violations: {reconciled_violations}")
    cc_observation = [entry for entry in (reconciled or []) if entry.get("name") == "cc"]
    if not cc_observation or cc_observation[0].get("state") != "ambient" or \
            "not private" not in cc_observation[0].get("note", ""):
        failures.append("audited host tool was not recorded as a host input")
    _, unbound_violations = expect_ok(
        "unbound host tool refuses", reconcile_host_tools,
        ambient_path, host_tools_document([]))
    if not any(v.startswith("unbound-ambient-executable:cc:") for v in unbound_violations):
        failures.append("unbound ambient host tool produced no violation")
    _, stale_violations = expect_ok(
        "changed host tool binding refuses", reconcile_host_tools,
        ambient_path,
        host_tools_document([dict(host_tool_binding_cc, sha256=sha256_bytes(b"stale"))]))
    if not any(v.startswith("unbound-ambient-executable:cc:") for v in stale_violations):
        failures.append("changed host tool binding produced no violation")
    _, stale_doc_violations = expect_ok(
        "documented absent host tool refuses", reconcile_host_tools,
        {"path_executables": {"entries": [{"name": "cc", "state": "absent"}],
                              "violations": []}, "violations": []},
        host_tools_document([host_tool_binding_cc]))
    if not any(v.startswith("documented-host-tool-not-live:cc") for v in stale_doc_violations):
        failures.append("documented but absent host tool produced no violation")
    priv_reconciled, priv_reconciled_violations = expect_ok(
        "private binding stays allowed", reconcile_host_tools,
        private_path, host_tools_document([]))
    if priv_reconciled_violations:
        failures.append(f"private binding was not allowed: {priv_reconciled_violations}")
    expect_refusal("host-tools document missing for reconcile refuses",
                   reconcile_host_tools, ambient_path, {"schema": "other"})

    # Composition regression (review 6598012622, host_tool_prelaunch_composition):
    # scan -> adjudication -> pre-launch control must compose: a matching
    # host-tools document leaves NO violation, the raw ambient-executable
    # findings are replaced rather than accumulated, and unrelated findings
    # survive unchanged into the same final list.
    composed_root, composed_source = provenance_case("candidate-composed")
    (composed_root / "bin").mkdir()
    composed_ambient = attach_path_executables(
        {"violations": []},
        str(composed_root / "bin") + os.pathsep + str(host_tools),
        private_root=composed_root)
    raw_ambient_findings = list(composed_ambient["violations"])
    if not any(v.startswith("ambient-executable:cc:") for v in raw_ambient_findings):
        failures.append("composition fixture lost the raw ambient-executable finding")
    composed_observation, composed_violations = expect_ok(
        "matching document clears the composed findings", reconcile_host_tools,
        composed_ambient, host_doc_match)
    if composed_violations:
        failures.append(f"composed matching document left violations: {composed_violations}")
    with_config = dict(composed_ambient,
                       violations=["ambient-cargo-config:found"] + raw_ambient_findings)
    _, config_violations = expect_ok(
        "unrelated findings survive adjudication", reconcile_host_tools,
        with_config, host_doc_match)
    if config_violations != ["ambient-cargo-config:found"]:
        failures.append(f"candidate composition changed unrelated findings: {config_violations}")

    # Candidate provenance packet: retained before a refusal, bounded.
    def candidate_inputs_for(root, source):
        return {"run_id": "d" * 32, "private_root": root,
                "account_root": fixture.root / "account-not-real",
                "argv": ["/effigy", STATUS_SELECTOR], "cwd": source,
                "env": {"HOME": str(root / "home"), "RUSTUP_TOOLCHAIN": TOOLCHAIN},
                "inherited_keys": ["HOME", "PATH"], "tools": {}, "host_tools": [],
                "sources": {}, "limit_seconds": 10, "scratch_mib": 64,
                "pool_cap_bytes": 1024}

    packet_root, packet_source = provenance_case("candidate-packet")
    candidate_document = expect_ok("candidate provenance builds", build_candidate_provenance,
                                   candidate_inputs_for(packet_root, packet_source),
                                   origin={"root": str(cand)}, guard={"state": "pre-launch"},
                                   artifact={"state": "copied"}, ambient={"violations": []},
                                   allowance={"clock": "CLOCK_UPTIME_RAW"}, written_ns=now_ns())
    if candidate_document is None or candidate_document.get("kind") != "candidate-provenance" \
            or candidate_document.get("ruling") != CANDIDATE_RULING:
        failures.append("candidate provenance packet lost its kind or ruling")
    expect_ok("candidate provenance retained", write_candidate_provenance,
              packet_root, candidate_document)
    if not (packet_root / CANDIDATE_PROVENANCE_PATH).is_file():
        failures.append("candidate provenance packet was not retained")
    padded_candidate = dict(candidate_document, padding="x" * (MAX_CANDIDATE_PROVENANCE_BYTES + 1))
    expect_refusal("oversized candidate provenance refuses without retention",
                   write_candidate_provenance, packet_root, padded_candidate)
    refuse_root, refuse_source = provenance_case("candidate-refuse")
    expect_refusal("candidate pre-launch finding refuses with packet retained",
                   refuse_candidate_launch, candidate_inputs_for(refuse_root, refuse_source),
                   {"violations": ["ambient-cargo-config:found"], "host_tool_bindings": []},
                   {"root": str(cand)}, {"clock": "CLOCK_UPTIME_RAW"})
    if not (refuse_root / CANDIDATE_PROVENANCE_PATH).is_file():
        failures.append("candidate refusal lost its retained packet")
    refused_packet = json.loads((refuse_root / CANDIDATE_PROVENANCE_PATH).read_bytes())
    if refused_packet.get("guard") != {"state": "not-launched"}:
        failures.append("candidate refusal packet lost its not-launched guard state")
    clean_root, clean_source = provenance_case("candidate-clean")
    expect_ok("clean candidate scan proceeds", refuse_candidate_launch,
              candidate_inputs_for(clean_root, clean_source),
              {"violations": [], "host_tool_bindings": []},
              {"root": str(cand)}, {"clock": "CLOCK_UPTIME_RAW"})
    gate_root, gate_source = provenance_case("candidate-clean-gate")
    gate_ambient = dict(composed_ambient, host_tool_bindings=composed_observation,
                        violations=composed_violations)
    expect_ok("matching host tools pass the pre-launch control", refuse_candidate_launch,
              candidate_inputs_for(gate_root, gate_source), gate_ambient,
              {"root": str(cand)}, {"clock": "CLOCK_UPTIME_RAW"})

    # Candidate argument binding: every planner-bound digest is required.
    expect_refusal("candidate without the origin refuses", parse_args,
                   ["--candidate", "--oxide-repo", "/r"])
    expect_refusal("candidate with a non-hex digest refuses", parse_args,
                   ["--candidate", "--oxide-repo", "/r", "--oxide-head", "a" * 40,
                    "--oxide-product-head", "b" * 40, "--barge-repo", "/b", "--barge-head", "c" * 40,
                    "--barge-root", "/Volumes/Builds/scratch/barge/x", "--oxide-root", "/Volumes/Builds/scratch/oxide/x",
                    "--expect-audit", "d" * 64, "--expect-ledger", "e" * 64, "--expect-budget", "f" * 64,
                    "--expect-history", "0" * 64, "--expect-oxide-binding", "1" * 64,
                    "--preparation-receipt", "/p", "--expect-preparation", "2" * 64,
                    "--expect-source-binding", "3" * 64, "--candidate-origin", "/o",
                    "--expect-candidate", "short", "--candidate-bytes", "5",
                    "--candidate-receipt", "/cr", "--expect-candidate-receipt", "4" * 64,
                    "--candidate-log", "/cl", "--expect-candidate-log", "5" * 64,
                    "--candidate-lock-sha256", "6" * 64, "--candidate-toolchain-sha256", "7" * 64,
                    "--host-tools", "/ht", "--expect-host-tools", "8" * 64])
    expect_refusal("candidate with a non-integer size refuses", parse_args,
                   ["--candidate", "--oxide-repo", "/r", "--oxide-head", "a" * 40,
                    "--oxide-product-head", "b" * 40, "--barge-repo", "/b", "--barge-head", "c" * 40,
                    "--barge-root", "/Volumes/Builds/scratch/barge/x", "--oxide-root", "/Volumes/Builds/scratch/oxide/x",
                    "--expect-audit", "d" * 64, "--expect-ledger", "e" * 64, "--expect-budget", "f" * 64,
                    "--expect-history", "0" * 64, "--expect-oxide-binding", "1" * 64,
                    "--preparation-receipt", "/p", "--expect-preparation", "2" * 64,
                    "--expect-source-binding", "3" * 64, "--candidate-origin", "/o",
                    "--expect-candidate", "9" * 64, "--candidate-bytes", "many",
                    "--candidate-receipt", "/cr", "--expect-candidate-receipt", "4" * 64,
                    "--candidate-log", "/cl", "--expect-candidate-log", "5" * 64,
                    "--candidate-lock-sha256", "6" * 64, "--candidate-toolchain-sha256", "7" * 64,
                    "--host-tools", "/ht", "--expect-host-tools", "8" * 64])

    # Artifact refusal controls: each uses its own private root so log paths
    # and the provenance cwd/src binding agree, and each retains the packet.
    def artifact_refusal_case(name, root, source, log_text, expected):
        try:
            resolve_bootstrap_artifact(log_text,
                                       provenance_inputs_for(root, source),
                                       guard=provenance_guard,
                                       ambient={"violations": []},
                                       allowance=provenance_allowance)
        except GateRefusal as refusal:
            if expected not in str(refusal):
                failures.append(f"{name}: wrong refusal {refusal}")
        else:
            failures.append(f"{name}: expected a refusal, got success")
            return None
        packet = json.loads((root / PROVENANCE_PATH).read_bytes())
        if packet.get("artifact", {}).get("state") != "unresolved" \
                or not packet.get("artifact", {}).get("findings", {}).get("violations"):
            failures.append(f"{name}: refusal packet lost the artifact findings")
        return packet

    missing_root, missing_source = provenance_case("missing-holder")
    artifact_refusal_case(
        "missing artifact", missing_root, missing_source,
        artifact_line(missing_source / "target/debug/gone", missing_source),
        "missing or unreadable")
    escaped_file = fixture.root / "escaped-oxide"
    escaped_file.write_bytes(b"x")
    escaped_root, escaped_source = provenance_case("escaped-holder")
    artifact_refusal_case(
        "escaped artifact", escaped_root, escaped_source,
        artifact_line(escaped_file, escaped_source),
        "escaped the private root")
    double_root, double_source = provenance_case("double-holder")
    first = double_source / "target/debug/one"
    first.write_bytes(b"1")
    second = double_source / "target/debug/two"
    second.write_bytes(b"2")
    artifact_refusal_case(
        "ambiguous artifact", double_root, double_source,
        artifact_line(first, double_source) + "\n" + artifact_line(second, double_source),
        "the bootstrap log does not name exactly one oxide artifact")
    mismatch_root, mismatch_source = provenance_case("mismatch-holder")
    mismatch_packet = artifact_refusal_case(
        "artifact src-path mismatch", mismatch_root, mismatch_source,
        artifact_line(mismatch_source / "target/debug/other", fixture.root),
        "the bootstrap log does not name exactly one oxide artifact")
    if mismatch_packet is not None and "artifact-src-path-mismatch" not in \
            mismatch_packet["artifact"]["findings"]["violations"]:
        failures.append("src-path mismatch diagnosis was not retained")
    malformed_root, malformed_source = provenance_case("malformed-holder")
    malformed_packet = artifact_refusal_case(
        "malformed bootstrap log", malformed_root, malformed_source,
        "not json\n[1,2]\n",
        "the bootstrap log does not name exactly one oxide artifact")
    if malformed_packet is not None and \
            malformed_packet["artifact"]["findings"].get("malformed_lines") != 2:
        failures.append("malformed log line count was not retained")

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
    if options["mode"] == "claim-oxide":
        _, run_id, _, scratch = supervisor_context()
        block = Path(options["block"])
        if block.parent != scratch or block.name != "block-authorization.json":
            raise GateRefusal("Oxide token is outside the monitored task root")
        print(json.dumps(claim_issued_token(block, options["digest"], run_id), sort_keys=True))
        return 0
    return diagnostic(options)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except GateRefusal as refusal:
        print(f"private-build-validation refused: {refusal}", file=sys.stderr)
        sys.exit(2)
    except (KeyError, OSError, ValueError, TypeError, UnicodeError,
            subprocess.SubprocessError) as error:
        print(f"private-build-validation error: {type(error).__name__}: {error}",
              file=sys.stderr)
        sys.exit(3)
