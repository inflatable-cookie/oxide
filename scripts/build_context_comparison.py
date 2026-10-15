#!/usr/bin/env python3
"""Private opt-in launcher for the frozen incremental comparison.

Default path is plan/validate only. Live execution requires --live and a
planner-owned unused resource plan. This module does not run the live
comparison by default and is not a context resolver, cache manager, or
product API.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import select
import signal
import stat
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping

# Source pin from reviewed 050 (exact; not current main).
NUCLEUS_COMMIT = "fb2bd0449ede2d9d2089a47b0559182bd413b91f"
NUCLEUS_TREE = "b47caf2ffb7e4430fc5e78411a5b475203126194"
CARGO_LOCK_SHA256 = "ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d"
RUSTC_RELEASE = "1.97.1"
RUSTC_COMMIT_PREFIX = "8bab26f4f"
HDIUTIL = "/usr/bin/hdiutil"

SELECTOR = "check:core:test"
TEST_PACKAGE = "nucleus-core"
TEST_TARGET = "packages"
TEST_NAME = "committed_workflow_packages_parse"
EFFIGY_TEST_ARGV = (
    "effigy",
    "check:core:test",
    "--",
    "-p",
    "nucleus-core",
    "--test",
    "packages",
    "-v",
    "--",
    "--exact",
    "committed_workflow_packages_parse",
)

BUDGET_NS = 1_200_000_000_000  # 1200s whole session, including preparation.
RESERVE_NS = 5_000_000_000  # time kept to record partial/abort.
SAMPLE_CADENCE_NS = 15_000_000_000
COUNTED_CAP = 10
RAW_LOG_LIMIT = 8 * 1024 * 1024
CRITICAL_RESERVE = 64 * 1024
TOTAL_FOOTPRINT_LIMIT = 16 * 1024 ** 3
IMAGE_USED_THRESHOLD = 14 * 1024 ** 3
HOST_FREE_RESERVE = 20 * 1024 ** 3
# 16 GiB is the owned allocated total, including backing metadata and
# host-side control/evidence. The contained FS is lower so that overhead
# stays inside the approved cap.
IMAGE_CREATE_SIZE = "15g"
WALK_FILE_CAP = 65536
WALK_NS_CAP = 5_000_000_000
COPY_FILE_CAP = 4096
COPY_BYTE_CAP = 512 * 1024 * 1024
PARTIAL_MARKER = (
    "\n[oxide-build-context: log cap reached; subsequent non-critical "
    "bytes dropped; partial=true]\n"
)
_PAREN_RUNNING = re.compile(r"^\s*Running\s+\S.*\s+\(([^)]+)\)\s*$")
_BACKTICK_RUNNING = re.compile(r"^\s*Running\s+`([^`]+)`")
_FRAME_RE = re.compile(r"^\s+at\s+\S*tests/packages\.rs:\d+", re.MULTILINE)
_COMPILING_RE = re.compile(r"^\s*Compiling\s+", re.MULTILINE)
_FRESH_RE = re.compile(r"^\s*Fresh\s+", re.MULTILINE)

# Contract 010 / 049 field names. Values of REDACT are never written to logs.
ADMISSION_PRESERVE = (
    "HOST_RUN_TOKEN",
    "HOST_RUN_ID",
    "EFFIGY_HOST_RUN_ROOT",
    "EFFIGY_HOST_SCHEDULER",
    "EFFIGY_SCHEDULER_OVERRIDE",
    "EFFIGY_ADMISSION_TIMEOUT_SECS",
    "EFFIGY_HOST_SCHEDULER_RUN_TIMEOUT_SECS",
    "EFFIGY_ADMISSION_CPU_UNITS",
    "EFFIGY_ADMISSION_MEMORY_MIB",
    "EFFIGY_ADMISSION_CPU_BUDGET",
    "EFFIGY_ADMISSION_MEMORY_BUDGET_MIB",
)
REDACT_ENV = frozenset({"HOST_RUN_TOKEN"})
HOST_PASSTHROUGH = (
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TZ",
    "RUSTUP_HOME",
)
DROP_OVERRIDES = (
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
)
FORBIDDEN_CONFIG_KEYS = frozenset(
    {
        "build",
        "profile",
        "rustc",
        "rustc-wrapper",
        "rustc-workspace-wrapper",
        "target-dir",
        "build-dir",
        "incremental",
        "rustflags",
        "env",
    }
)
COUNTED_STEPS = (
    ("probe", "cargo-vV"),
    ("probe", "rustc-vV"),
    ("off", "C0"),
    ("off", "E1"),
    ("off", "E2"),
    ("off", "U"),
    ("on", "C0"),
    ("on", "E1"),
    ("on", "E2"),
    ("on", "U"),
)
EDIT_FILES = (
    "crates/nucleus-core/src/lib.rs",
    "crates/nucleus-core/tests/packages.rs",
)


class ComparisonError(Exception):
    def __init__(self, outcome: str, seam: str, message: str):
        super().__init__(message)
        self.outcome = outcome
        self.seam = seam
        self.message = message


@dataclass
class RegistryInput:
    src: Path
    rel: str


@dataclass
class ResourcePlan:
    launch_id: str
    parent: Path
    mount: Path
    backing: Path
    nucleus_source: Path
    registry_inputs: tuple[RegistryInput, ...]

    @classmethod
    def from_mapping(cls, data: Mapping[str, object]) -> "ResourcePlan":
        inputs = tuple(
            RegistryInput(src=Path(str(item["src"])), rel=str(item["rel"]))
            for item in list(data.get("registry_inputs") or [])
        )
        return cls(
            launch_id=str(data["launch_id"]),
            parent=Path(str(data["parent"])),
            mount=Path(str(data["mount"])),
            backing=Path(str(data["backing"])),
            nucleus_source=Path(str(data["nucleus_source"])),
            registry_inputs=inputs,
        )


@dataclass
class SessionClock:
    start_monotonic_ns: int
    start_utc: str
    deadline_monotonic_ns: int
    reserve_ns: int = RESERVE_NS

    def remaining_hard_ns(self, now_ns: int) -> int:
        return max(0, self.deadline_monotonic_ns - now_ns)

    def remaining_delivery_ns(self, now_ns: int) -> int:
        return max(0, self.deadline_monotonic_ns - self.reserve_ns - now_ns)

    def expired_for_delivery(self, now_ns: int) -> bool:
        return self.remaining_delivery_ns(now_ns) <= 0

    def expired_hard(self, now_ns: int) -> bool:
        return self.remaining_hard_ns(now_ns) <= 0


@dataclass
class RunResult:
    outcome: str
    spawned: bool
    argv: tuple[str, ...]
    exit_code: int | None = None
    pgid: int | None = None
    child_terminated: bool = False
    writer_closed: bool = False
    stdout: str = ""
    stderr: str = ""
    duration_ns: int = 0
    seam: str | None = None


@dataclass
class Accounting:
    logical_bytes: int = 0
    allocated_bytes: int = 0
    file_count: int = 0
    hardlink_dedup_count: int = 0
    truncated: bool = False
    apfs_clone_sharing: str = (
        "unknown; hardlink-aware (dev,ino) accounting does not prove APFS "
        "clone or snapshot sharing is absent or cannot confound allocation"
    )


@dataclass
class EvidenceLog:
    limit: int = RAW_LOG_LIMIT
    critical_reserve: int = CRITICAL_RESERVE
    chunks: list[str] = field(default_factory=list)
    used: int = 0
    critical_used: int = 0
    overflow: bool = False

    def write(self, text: str, *, critical: bool = False) -> None:
        if not text:
            return
        data = text if text.endswith("\n") else text + "\n"
        encoded = data.encode("utf-8", "replace")
        if critical:
            room = self.critical_reserve - self.critical_used
            if room <= 0:
                self.overflow = True
                return
            if len(encoded) > room:
                data = encoded[:room].decode("utf-8", "replace")
                encoded = data.encode("utf-8", "replace")
                self.overflow = True
            self.chunks.append(data)
            self.critical_used += len(encoded)
            self.used += len(encoded)
            return
        soft = self.limit - self.critical_reserve
        if self.used >= soft:
            if not self.overflow:
                self.chunks.append(PARTIAL_MARKER)
                self.used += len(PARTIAL_MARKER.encode("utf-8"))
                self.overflow = True
            return
        room = soft - self.used
        if len(encoded) > room:
            kept = encoded[:room].decode("utf-8", "replace")
            self.chunks.append(kept)
            self.chunks.append(PARTIAL_MARKER)
            self.used = soft
            self.overflow = True
            return
        self.chunks.append(data)
        self.used += len(encoded)

    def text(self) -> str:
        return "".join(self.chunks)


class RealClock:
    def monotonic_ns(self) -> int:
        return time.monotonic_ns()

    def utc_iso(self) -> str:
        return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class RealSpawner:
    def __init__(self) -> None:
        self.created_pgids: list[int] = []

    def run(
        self,
        argv: list[str],
        *,
        env: Mapping[str, str],
        cwd: str | None,
        remaining_ns: int,
        log: EvidenceLog,
        clock,
        session: SessionClock,
        poll=None,
    ) -> RunResult:
        if remaining_ns <= 0 or session.expired_hard(clock.monotonic_ns()):
            return RunResult(
                outcome="deadline",
                spawned=False,
                argv=tuple(argv),
                seam="deadline_before_spawn",
            )
        start = clock.monotonic_ns()
        proc = subprocess.Popen(
            argv,
            env=dict(env),
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        pgid = proc.pid
        self.created_pgids.append(pgid)
        stdout_buf: list[bytes] = []
        stderr_buf: list[bytes] = []
        captured = 0
        streams = [proc.stdout, proc.stderr]
        terminated = False
        try:
            while True:
                now = clock.monotonic_ns()
                hard = session.remaining_hard_ns(now)
                if hard <= 0:
                    closed = _terminate_pgid(pgid, proc)
                    terminated = True
                    if not closed:
                        return RunResult(
                            outcome="unknown",
                            spawned=True,
                            argv=tuple(argv),
                            pgid=pgid,
                            child_terminated=False,
                            writer_closed=False,
                            duration_ns=clock.monotonic_ns() - start,
                            seam="writer_closure_unknown",
                        )
                    break
                if poll is not None:
                    try:
                        poll()
                    except ComparisonError:
                        closed = _terminate_pgid(pgid, proc)
                        if not closed:
                            return RunResult(
                                outcome="unknown",
                                spawned=True,
                                argv=tuple(argv),
                                pgid=pgid,
                                child_terminated=False,
                                writer_closed=False,
                                duration_ns=clock.monotonic_ns() - start,
                                seam="writer_closure_unknown",
                            )
                        raise
                timeout = min(0.05, hard / 1_000_000_000)
                readable, _, _ = select.select(
                    [s for s in streams if s is not None], [], [], timeout
                )
                for stream in readable:
                    chunk = os.read(stream.fileno(), 65536)
                    if not chunk:
                        continue
                    captured += len(chunk)
                    text = chunk.decode("utf-8", "replace")
                    log.write(text)
                    if stream is proc.stdout:
                        if captured <= RAW_LOG_LIMIT:
                            stdout_buf.append(chunk)
                    else:
                        if captured <= RAW_LOG_LIMIT:
                            stderr_buf.append(chunk)
                if proc.poll() is not None:
                    _drain(proc.stdout, stdout_buf, log)
                    _drain(proc.stderr, stderr_buf, log)
                    break
        except Exception:
            _terminate_pgid(pgid, proc)
            raise
        code = proc.poll()
        closed = code is not None
        outcome = "deadline" if terminated else "ok"
        return RunResult(
            outcome=outcome,
            spawned=True,
            argv=tuple(argv),
            exit_code=code,
            pgid=pgid,
            child_terminated=terminated and closed,
            writer_closed=closed,
            stdout=b"".join(stdout_buf).decode("utf-8", "replace"),
            stderr=b"".join(stderr_buf).decode("utf-8", "replace"),
            duration_ns=clock.monotonic_ns() - start,
            seam="deadline" if terminated else None,
        )


def _drain(stream, buf: list[bytes], log: EvidenceLog) -> None:
    if stream is None:
        return
    try:
        chunk = stream.read()
    except Exception:
        return
    if chunk:
        buf.append(chunk)
        log.write(chunk.decode("utf-8", "replace"))


def _signal_helper(pgid: int, sig: int) -> None:
    try:
        os.killpg(pgid, sig)
        return
    except ProcessLookupError:
        return
    except PermissionError:
        pass
    try:
        os.kill(pgid, sig)
    except (ProcessLookupError, PermissionError):
        return


def _terminate_pgid(pgid: int, proc: subprocess.Popen | None = None) -> bool:
    _signal_helper(pgid, signal.SIGTERM)
    if proc is not None:
        try:
            proc.wait(timeout=0.5)
            return True
        except subprocess.TimeoutExpired:
            pass
    else:
        time.sleep(0.05)
    _signal_helper(pgid, signal.SIGKILL)
    if proc is not None:
        try:
            proc.wait(timeout=0.5)
            return True
        except subprocess.TimeoutExpired:
            return False
    try:
        os.kill(pgid, 0)
    except ProcessLookupError:
        return True
    except PermissionError:
        return False
    return False


class RealImage:
    def create(self, backing: Path, size: str, remaining_ns: int) -> str:
        if remaining_ns <= 0:
            raise ComparisonError("deadline", "deadline", "image create past deadline")
        result = subprocess.run(
            [HDIUTIL, "create", "-size", size, "-type", "SPARSE", "-fs", "APFS", str(backing)],
            capture_output=True,
            text=True,
            timeout=max(1, remaining_ns / 1_000_000_000),
        )
        if result.returncode != 0:
            raise ComparisonError("refusal", "image_create", result.stderr[-400:])
        return result.stdout

    def attach(self, backing: Path, mount: Path, remaining_ns: int) -> str:
        if remaining_ns <= 0:
            raise ComparisonError("deadline", "deadline", "image attach past deadline")
        result = subprocess.run(
            [HDIUTIL, "attach", "-nobrowse", "-mountpoint", str(mount), str(backing)],
            capture_output=True,
            text=True,
            timeout=max(1, remaining_ns / 1_000_000_000),
        )
        if result.returncode != 0:
            raise ComparisonError("refusal", "image_attach", result.stderr[-400:])
        return result.stdout

    def imageinfo(self, backing: Path, remaining_ns: int) -> str:
        if remaining_ns <= 0:
            raise ComparisonError("deadline", "deadline", "imageinfo past deadline")
        result = subprocess.run(
            [HDIUTIL, "imageinfo", str(backing)],
            capture_output=True,
            text=True,
            timeout=max(1, remaining_ns / 1_000_000_000),
        )
        if result.returncode != 0:
            raise ComparisonError("unknown", "imageinfo", result.stderr[-400:])
        return result.stdout


def parse_imageinfo(text: str) -> dict[str, int]:
    parsed: dict[str, int] = {}
    for index, raw in enumerate(text.splitlines()):
        if index > 200:
            raise ComparisonError("unknown", "imageinfo_parse", "imageinfo exceeded parse cap")
        line = raw.strip()
        for key, label in (
            ("total_bytes", "Total Bytes:"),
            ("non_empty_bytes", "Total Non-Empty Bytes:"),
            ("empty_bytes", "Total Empty Bytes:"),
        ):
            if line.startswith(label):
                number = line.split(":", 1)[1].strip().replace(",", "")
                if not number.isdigit():
                    raise ComparisonError("unknown", "imageinfo_parse", f"bad {label}")
                parsed[key] = int(number)
    if "total_bytes" not in parsed or "non_empty_bytes" not in parsed:
        raise ComparisonError("unknown", "imageinfo_parse", "missing size fields")
    return parsed


def allocated_bytes(st: os.stat_result) -> int:
    return int(st.st_blocks) * 512


def lstat_path(path: Path) -> os.stat_result:
    return os.lstat(path)


def path_is_symlink(path: Path) -> bool:
    try:
        return stat.S_ISLNK(os.lstat(path).st_mode)
    except FileNotFoundError:
        return False


def ancestor_symlink(path: Path) -> bool:
    current = path
    seen = 0
    while True:
        seen += 1
        if seen > 64:
            return True
        if path_is_symlink(current):
            return True
        parent = current.parent
        if parent == current:
            return False
        current = parent


def backing_under_mount(backing: Path, mount: Path) -> bool:
    try:
        backing.resolve().relative_to(mount.resolve())
        return True
    except ValueError:
        return False


def require_unused_path(path: Path, *, kind: str) -> None:
    if ancestor_symlink(path.parent):
        raise ComparisonError("refusal", "unsafe_path", f"{kind} ancestor is a symlink")
    try:
        st = os.lstat(path)
    except FileNotFoundError:
        return
    raise ComparisonError(
        "refusal",
        "path_exists",
        f"{kind} already exists mode={st.st_mode:o} nlink={st.st_nlink}",
    )


def validate_resource_plan(plan: ResourcePlan, *, strict_names: bool = True) -> list[str]:
    notes: list[str] = []
    if plan.mount.parent != plan.parent or plan.mount.name != "m":
        raise ComparisonError(
            "refusal",
            "containment_names",
            "mount must be <parent>/m",
        )
    if plan.backing.parent != plan.parent:
        raise ComparisonError(
            "refusal",
            "containment_names",
            "backing must be a sibling of the mount under parent",
        )
    if backing_under_mount(plan.backing, plan.mount):
        raise ComparisonError(
            "refusal",
            "backing_under_mount",
            "backing file must not reside under its own mountpoint",
        )
    if plan.backing.suffix != ".sparseimage":
        raise ComparisonError("refusal", "containment_names", "backing must be a .sparseimage")
    expected_parent = Path("/private/var/tmp") / f"oxide-build-context-cmp-{plan.launch_id}"
    if strict_names and plan.parent != expected_parent:
        raise ComparisonError(
            "refusal",
            "containment_names",
            f"parent must be {expected_parent}",
        )
    for kind, path in (
        ("parent", plan.parent),
        ("mount", plan.mount),
        ("backing", plan.backing),
    ):
        require_unused_path(path, kind=kind)
        notes.append(f"{kind}_unused={path}")
    if not plan.registry_inputs:
        raise ComparisonError(
            "refusal",
            "source_cache_unbound",
            "live plan needs exact declared registry inputs; operator CARGO_HOME is not scanned",
        )
    for item in plan.registry_inputs:
        if Path(item.rel).is_absolute() or ".." in Path(item.rel).parts:
            raise ComparisonError("refusal", "source_cache_unbound", "registry rel must stay inside scratch")
        if not item.src.is_absolute():
            raise ComparisonError("refusal", "source_cache_unbound", "registry src must be absolute")
    return notes


def cargo_config_paths(source_root: Path, mount: Path, parent: Path, cargo_home: Path) -> list[Path]:
    names = (".cargo/config", ".cargo/config.toml")
    bases = (
        source_root,
        mount,
        parent,
        Path("/private/var/tmp"),
        Path("/private/var"),
        Path("/private"),
        Path("/"),
    )
    paths = [base / name for base in bases for name in names]
    paths.append(cargo_home / "config")
    paths.append(cargo_home / "config.toml")
    return paths


def config_has_forbidden_keys(text: str) -> bool:
    for raw in text.splitlines()[:400]:
        stripped = raw.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            name = stripped[1:-1].split(".", 1)[0].strip()
            if name in FORBIDDEN_CONFIG_KEYS:
                return True
        if "=" in stripped:
            key = stripped.split("=", 1)[0].strip()
            if key in FORBIDDEN_CONFIG_KEYS:
                return True
    return False


def build_child_env(
    parent_env: Mapping[str, str],
    *,
    cargo_home: str,
    target_dir: str,
    tmp: str,
    incremental: str,
) -> dict[str, str]:
    env: dict[str, str] = {}
    for name in HOST_PASSTHROUGH + ADMISSION_PRESERVE:
        value = parent_env.get(name)
        if value is not None:
            env[name] = value
    env.update(
        {
            "CARGO_HOME": cargo_home,
            "CARGO_TARGET_DIR": target_dir,
            "TMPDIR": tmp,
            "TMP": tmp,
            "TEMP": tmp,
            "CARGO_INCREMENTAL": incremental,
            "CARGO_PROFILE_DEV_SPLIT_DEBUGINFO": "unpacked",
            "CARGO_PROFILE_TEST_SPLIT_DEBUGINFO": "unpacked",
            "RUSTUP_TOOLCHAIN": RUSTC_RELEASE,
        }
    )
    for dropped in DROP_OVERRIDES:
        env.pop(dropped, None)
    return env


def redact_env_for_log(env: Mapping[str, str]) -> dict[str, str]:
    recorded: dict[str, str] = {}
    for key, value in env.items():
        if key in REDACT_ENV:
            recorded[key] = "<redacted>"
        else:
            recorded[key] = value
    return recorded


def union_account(roots: list[Path], *, owned_roots: list[Path], clock, remaining_ns: int) -> Accounting:
    start = clock.monotonic_ns()
    owned_resolved = [root.resolve() for root in owned_roots]
    seen: set[tuple[int, int]] = set()
    acc = Accounting()
    for root in roots:
        resolved = root.resolve()
        if not any(_is_relative_to(resolved, owned) for owned in owned_resolved):
            raise ComparisonError(
                "refusal",
                "accounting_root_outside_owned",
                f"{root} is outside owned footprint; host CARGO_HOME is not scanned",
            )
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            now = clock.monotonic_ns()
            if now - start > min(WALK_NS_CAP, remaining_ns or WALK_NS_CAP):
                acc.truncated = True
                return acc
            dirnames[:] = [name for name in dirnames if not path_is_symlink(Path(dirpath) / name)]
            for name in filenames:
                acc.file_count += 1
                if acc.file_count > WALK_FILE_CAP:
                    acc.truncated = True
                    return acc
                path = Path(dirpath) / name
                if path_is_symlink(path):
                    continue
                st = os.lstat(path)
                key = (st.st_dev, st.st_ino)
                acc.logical_bytes += st.st_size
                if key in seen:
                    acc.hardlink_dedup_count += 1
                    continue
                seen.add(key)
                acc.allocated_bytes += allocated_bytes(st)
    return acc


def _is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def footprint_sum(*, backing: Path, extras: list[Path], imageinfo: dict[str, int]) -> int:
    total = 0
    try:
        total += allocated_bytes(os.lstat(backing))
    except FileNotFoundError:
        total += imageinfo.get("non_empty_bytes", 0)
    for extra in extras:
        try:
            st = os.lstat(extra)
        except FileNotFoundError:
            continue
        if stat.S_ISDIR(st.st_mode):
            for dirpath, _, filenames in os.walk(extra, followlinks=False):
                for name in filenames:
                    total += allocated_bytes(os.lstat(Path(dirpath) / name))
        else:
            total += allocated_bytes(st)
    total = max(total, imageinfo.get("non_empty_bytes", 0))
    return total


def parse_reported_executable(text: str) -> str | None:
    """Last packages-* test binary from Cargo Running lines on stdout or stderr.

    Accepts the unit-graph form ``Running tests/packages.rs (target/debug/deps/packages-<hash>)``
    and the verbose backtick form ``Running `<path> --exact ...```. rustc Running
    lines are skipped. Relative paths are returned as reported; locate resolves them.
    """
    last = None
    for line in text.splitlines():
        reported = None
        paren = _PAREN_RUNNING.match(line)
        if paren:
            reported = paren.group(1).strip()
        else:
            tick = _BACKTICK_RUNNING.match(line)
            if tick:
                reported = tick.group(1).strip().split()[0]
        if not reported:
            continue
        name = Path(reported).name
        if name == "rustc" or name.startswith("rustc-"):
            continue
        posix = reported.replace("\\", "/")
        if "/deps/" not in posix and Path(reported).parent.name != "deps":
            continue
        if not name.startswith("packages-"):
            continue
        last = reported
    return last


def locate_reported_executable(
    reported: str | None, target_dir: Path, *, cwd: Path | None = None
) -> Path:
    if not reported:
        raise ComparisonError("partial", "stale_executable_ambiguity", "no reported test executable")
    raw = Path(reported)
    candidates: list[Path] = []
    if raw.is_absolute():
        candidates.append(raw)
    else:
        if cwd is not None:
            candidates.append(cwd / raw)
        candidates.append(target_dir / raw)
        parts = raw.parts
        if parts and parts[0] == "target":
            candidates.append(target_dir.joinpath(*parts[1:]))
        if "debug" in parts:
            idx = list(parts).index("debug")
            candidates.append(target_dir.joinpath(*parts[idx:]))
    try:
        deps = (target_dir / "debug" / "deps").resolve()
    except FileNotFoundError as exc:
        raise ComparisonError("partial", "stale_executable_ambiguity", "target deps missing") from exc
    for cand in candidates:
        try:
            st = os.lstat(cand)
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode):
            continue
        try:
            resolved = Path(cand).resolve()
            resolved.relative_to(deps)
        except (ValueError, OSError):
            continue
        return cand
    raise ComparisonError(
        "partial",
        "stale_executable_ambiguity",
        "reported executable missing, not a regular file, or outside target deps",
    )


def backtrace_has_packages_frame(text: str) -> bool:
    if "packages directory" not in text:
        return False
    body = "\n".join(line for line in text.splitlines() if "panicked at" not in line)
    return _FRAME_RE.search(body) is not None


def deps_artifact_counts(path: Path) -> tuple[int, int, int]:
    files = 0
    objects = 0
    dsyms = 0
    if not path.exists():
        return 0, 0, 0
    for dirpath, dirnames, filenames in os.walk(path, followlinks=False):
        for name in filenames:
            files += 1
            if name.endswith(".o"):
                objects += 1
            if name.endswith(".dSYM"):
                dsyms += 1
        for name in dirnames:
            if name.endswith(".dSYM"):
                dsyms += 1
    return files, objects, dsyms


class CommandCounter:
    def __init__(self, cap: int = COUNTED_CAP) -> None:
        self.cap = cap
        self.plan = list(COUNTED_STEPS)
        if len(self.plan) != cap:
            raise ComparisonError("refusal", "count", f"plan is {len(self.plan)} not {cap}")
        self.delivered = 0
        self.calls: list[tuple[str, str]] = []

    def before_delivery(self, mode: str, name: str) -> None:
        if self.delivered >= self.cap:
            raise ComparisonError("refusal", "count", "11th counted call refused before spawn")
        expected = self.plan[self.delivered]
        if expected != (mode, name):
            raise ComparisonError(
                "refusal",
                "count",
                f"expected {expected} got {(mode, name)}",
            )
        self.delivered += 1
        self.calls.append((mode, name))


class ComparisonLauncher:
    def __init__(
        self,
        *,
        clock=None,
        spawner=None,
        image=None,
        parent_env: Mapping[str, str] | None = None,
        budget_ns: int = BUDGET_NS,
        reserve_ns: int = RESERVE_NS,
        space_probe=None,
        git=None,
        copy=None,
        config_paths=None,
        strict_names: bool = True,
        lock_sha256=None,
        clone_source=None,
    ) -> None:
        self.clock = clock or RealClock()
        self.spawner = spawner or RealSpawner()
        self.image = image or RealImage()
        self.parent_env = dict(parent_env if parent_env is not None else os.environ)
        self.budget_ns = budget_ns
        self.reserve_ns = reserve_ns
        self.space_probe = space_probe
        self.git = git
        self.copy = copy
        self.config_paths = config_paths or cargo_config_paths
        self.strict_names = strict_names
        self.lock_sha256 = lock_sha256
        self.clone_source = clone_source
        self.log = EvidenceLog()
        self.counter = CommandCounter()
        self.session: SessionClock | None = None
        self.resources_retained = False
        self.effects_executed = False
        self.preparation_after_clock = False
        self.outcome = "plan"
        self.seam: str | None = None
        self.helper_pgids: list[int] = []
        self.created_paths: list[Path] = []
        self.active_plan: ResourcePlan | None = None
        self.last_sample_ns = 0
        self.sample_count = 0
        self.peak_image_used = 0
        self.min_host_free: int | None = None
        self.measurements: list[dict[str, object]] = []

    def plan(self, resource_plan: ResourcePlan | None = None) -> dict[str, object]:
        notes = []
        live_ready = False
        if resource_plan is not None:
            try:
                notes.extend(validate_resource_plan(resource_plan, strict_names=self.strict_names))
                live_ready = True
            except ComparisonError as error:
                notes.append(f"seam={error.seam} {error.message}")
                live_ready = False
        counted = [
            {
                "index": index + 1,
                "mode": mode,
                "name": name,
                "argv": list(_argv_for(mode, name)),
            }
            for index, (mode, name) in enumerate(COUNTED_STEPS)
        ]
        document = {
            "effects_executed": False,
            "live": False,
            "outcome": "plan",
            "pins": {
                "nucleus_commit": NUCLEUS_COMMIT,
                "nucleus_tree": NUCLEUS_TREE,
                "cargo_lock_sha256": CARGO_LOCK_SHA256,
                "rustc_release": RUSTC_RELEASE,
                "rustc_commit_prefix": RUSTC_COMMIT_PREFIX,
                "selector": SELECTOR,
                "test": TEST_NAME,
                "hdiutil": HDIUTIL,
                "image_create_size": IMAGE_CREATE_SIZE,
            },
            "clocks": {
                "deadline_clock": "time.monotonic_ns",
                "audit_clock": "UTC",
                "budget_ns": self.budget_ns,
                "reserve_ns": self.reserve_ns,
                "note": "EPOCHREALTIME and datetime UTC are not the deadline clock",
            },
            "count": {"cap": COUNTED_CAP, "planned": counted},
            "containment": {
                "parent": "/private/var/tmp/oxide-build-context-cmp-<launch-id>",
                "mount": "<parent>/m",
                "backing": "<parent>/cmp.sparseimage",
                "backing_is_sibling_of_mount": True,
                "total_footprint_limit": TOTAL_FOOTPRINT_LIMIT,
                "image_used_threshold": IMAGE_USED_THRESHOLD,
                "host_free_reserve": HOST_FREE_RESERVE,
                "sample_cadence_ns": SAMPLE_CADENCE_NS,
                "image_create_size": IMAGE_CREATE_SIZE,
            },
            "admission_preserve": list(ADMISSION_PRESERVE),
            "redact": list(REDACT_ENV),
            "cargo_offline_readonly": False,
            "live_ready": live_ready,
            "notes": notes,
            "apfs_clone_sharing": Accounting().apfs_clone_sharing,
        }
        self.outcome = "plan"
        return document

    def run(self, *, live: bool, resource_plan: ResourcePlan | None) -> dict[str, object]:
        if not live:
            return self.plan(resource_plan)
        if resource_plan is None:
            self.outcome = "refusal"
            self.seam = "live_opt_in_incomplete"
            raise ComparisonError(
                "refusal",
                "live_opt_in_incomplete",
                "live path needs --live and a planner-owned unused resource plan",
            )
        return self._run_live(resource_plan)

    def _open_session(self) -> SessionClock:
        start = self.clock.monotonic_ns()
        utc = self.clock.utc_iso()
        session = SessionClock(
            start_monotonic_ns=start,
            start_utc=utc,
            deadline_monotonic_ns=start + self.budget_ns,
            reserve_ns=self.reserve_ns,
        )
        self.session = session
        self.last_sample_ns = start
        self.log.write(
            f"session_start utc={utc} monotonic_ns={start} deadline_ns={session.deadline_monotonic_ns}",
            critical=True,
        )
        return session

    def _now(self) -> int:
        return self.clock.monotonic_ns()

    def _remaining_delivery(self) -> int:
        assert self.session is not None
        return self.session.remaining_delivery_ns(self._now())

    def _remaining_hard(self) -> int:
        assert self.session is not None
        return self.session.remaining_hard_ns(self._now())

    def _run_live(self, plan: ResourcePlan) -> dict[str, object]:
        session = self._open_session()
        self.active_plan = plan
        try:
            validate_resource_plan(plan, strict_names=self.strict_names)
            self.preparation_after_clock = True
            host_free = self._host_free()
            if host_free < HOST_FREE_RESERVE:
                raise ComparisonError("refusal", "host_reserve", f"host free {host_free}")
            self._prepare_containment(plan)
            self.effects_executed = True
            env_off = self._mode_env(plan, "off")
            cargo = self._deliver("probe", "cargo-vV", list(_argv_for("probe", "cargo-vV")), env_off, str(plan.mount))
            self._check_cargo(cargo)
            rustc = self._deliver(
                "probe", "rustc-vV", list(_argv_for("probe", "rustc-vV")), env_off, str(plan.mount)
            )
            self._check_rustc(rustc)
            for mode in ("off", "on"):
                self._run_mode(plan, mode)
                self._sample(plan)
            self.outcome = "success"
        except ComparisonError as error:
            self.outcome = error.outcome
            self.seam = error.seam
            self.log.write(f"outcome={error.outcome} seam={error.seam} {error.message}", critical=True)
            if error.outcome in {"deadline", "unknown", "partial"}:
                self.resources_retained = True
            if error.outcome == "unknown":
                self.resources_retained = True
        if self.outcome != "success":
            self.resources_retained = True
        return self._result_document(plan, session)

    def _prepare_containment(self, plan: ResourcePlan) -> None:
        remaining = self._remaining_delivery()
        if remaining <= 0:
            raise ComparisonError("deadline", "deadline", "expired before containment")
        os.mkdir(plan.parent, 0o700)
        self.created_paths.append(plan.parent)
        st = os.lstat(plan.parent)
        if stat.S_ISLNK(st.st_mode) or st.st_uid != os.getuid() or (st.st_mode & 0o777) != 0o700:
            raise ComparisonError("refusal", "unsafe_path", "parent owner/mode/symlink check failed")
        os.mkdir(plan.mount, 0o700)
        self.image.create(plan.backing, IMAGE_CREATE_SIZE, self._remaining_hard())
        self.image.attach(plan.backing, plan.mount, self._remaining_hard())
        info_text = self.image.imageinfo(plan.backing, self._remaining_hard())
        info = parse_imageinfo(info_text)
        extras = [plan.parent / "control"]
        total = footprint_sum(backing=plan.backing, extras=extras, imageinfo=info)
        if total > TOTAL_FOOTPRINT_LIMIT:
            raise ComparisonError("refusal", "footprint", f"owned footprint {total} exceeds 16 GiB")
        if info.get("non_empty_bytes", 0) >= IMAGE_USED_THRESHOLD:
            raise ComparisonError("refusal", "image_threshold", "image used at or above 14 GiB")
        self._sample(plan)
        self._prepare_source_cache(plan)

    def _prepare_source_cache(self, plan: ResourcePlan) -> None:
        cargo_home = plan.mount / "cargo-home"
        os.makedirs(cargo_home, 0o700, exist_ok=True)
        copied_files = 0
        copied_bytes = 0
        copier = self.copy or _copy_file
        for item in plan.registry_inputs:
            dest = cargo_home / item.rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            if self._remaining_hard() <= 0:
                raise ComparisonError("deadline", "deadline", "expired during source-cache copy")
            size = copier(item.src, dest)
            copied_files += 1
            copied_bytes += size
            if copied_files > COPY_FILE_CAP or copied_bytes > COPY_BYTE_CAP:
                raise ComparisonError("refusal", "source_cache_unbound", "declared registry copy exceeded bound")
        self.log.write(f"source_cache files={copied_files} bytes={copied_bytes}", critical=True)

    def _mode_env(self, plan: ResourcePlan, mode: str) -> dict[str, str]:
        incremental = "0" if mode == "off" else "1"
        return build_child_env(
            self.parent_env,
            cargo_home=str(plan.mount / "cargo-home"),
            target_dir=str(plan.mount / f"target-{mode}"),
            tmp=str(plan.mount / f"tmp-{mode}"),
            incremental=incremental,
        )

    def _run_mode(self, plan: ResourcePlan, mode: str) -> None:
        env = self._mode_env(plan, mode)
        source = plan.mount / f"src-{mode}"
        target = plan.mount / f"target-{mode}"
        self._prepare_source(plan, mode, source)
        os.makedirs(target, exist_ok=True)
        os.makedirs(plan.mount / f"tmp-{mode}", exist_ok=True)
        self._verify_source(source)
        self._check_configs(source, plan)
        reported = None
        for step in ("C0", "E1", "E2", "U"):
            if step == "E1":
                self._edit(source, EDIT_FILES[0], " // oxide-build-context comparison edit 1")
            if step == "E2":
                self._edit(source, EDIT_FILES[1], " // oxide-build-context comparison edit 2")
            result = self._deliver(mode, step, list(EFFIGY_TEST_ARGV), env, str(source))
            streams = result.stdout + "\n" + result.stderr
            self._check_rustc_running_lines(streams)
            reported = parse_reported_executable(streams) or reported
            self._record_invocation(plan, mode, step, result, target)
            if result.outcome != "ok" or result.exit_code not in {0, None}:
                raise ComparisonError("partial", "selected_test", f"{mode} {step} failed")
        self._negative_check(source, target, env, reported)

    def _negative_check(
        self, source: Path, target: Path, env: Mapping[str, str], reported: str | None
    ) -> None:
        packages = source / "packages"
        moved = source / "packages.oxide-moved"
        restored = {"packages": False, "files": False}
        try:
            executable = locate_reported_executable(reported, target, cwd=source)
            if packages.exists():
                os.rename(packages, moved)
            result = self._run_helper(
                [str(executable), "--exact", TEST_NAME],
                env=dict(env, RUST_BACKTRACE="1"),
                cwd=str(source),
            )
            streams = result.stderr + "\n" + result.stdout
            self.log.write(
                f"negative_check exit={result.exit_code} expected_panic=packages directory",
                critical=True,
            )
            if "packages directory" not in streams:
                raise ComparisonError(
                    "partial",
                    "backtrace_frame",
                    "missing expected panic packages directory",
                )
            if not backtrace_has_packages_frame(streams):
                raise ComparisonError(
                    "partial",
                    "backtrace_frame",
                    "missing tests/packages.rs frame (panic header is not a frame)",
                )
        finally:
            if moved.exists() and not packages.exists():
                os.rename(moved, packages)
                restored["packages"] = True
            self._restore_tracked(source)
            restored["files"] = True
            self.log.write(f"restore packages={restored['packages']} files={restored['files']}", critical=True)
            self._verify_restore(source)

    def _edit(self, source: Path, rel: str, suffix: str) -> None:
        path = source / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(suffix + "\n")

    def _check_configs(self, source: Path, plan: ResourcePlan) -> None:
        cargo_home = plan.mount / "cargo-home"
        for path in self.config_paths(source, plan.mount, plan.parent, cargo_home):
            try:
                st = os.lstat(path)
            except FileNotFoundError:
                continue
            if stat.S_ISLNK(st.st_mode):
                raise ComparisonError("refusal", "config_identity", f"config symlink {path}")
            if st.st_size > 1_048_576:
                raise ComparisonError("refusal", "config_identity", f"config too large {path}")
            text = Path(path).read_text(encoding="utf-8", errors="replace")
            if config_has_forbidden_keys(text):
                raise ComparisonError("refusal", "config_identity", f"forbidden config keys in {path}")

    def _check_cargo(self, result: RunResult) -> None:
        text = result.stdout + result.stderr
        if RUSTC_RELEASE not in text:
            raise ComparisonError("refusal", "tool_identity", "cargo version mismatch")

    def _check_rustc(self, result: RunResult) -> None:
        text = result.stdout + result.stderr
        if f"release: {RUSTC_RELEASE}" not in text and RUSTC_RELEASE not in text:
            raise ComparisonError("refusal", "tool_identity", "rustc release mismatch")
        if RUSTC_COMMIT_PREFIX not in text:
            raise ComparisonError("refusal", "tool_identity", "rustc commit mismatch")

    def _prepare_source(self, plan: ResourcePlan, mode: str, dest: Path) -> None:
        if self._remaining_delivery() <= 0:
            raise ComparisonError("deadline", "deadline", "expired before source preparation")
        if self.clone_source is not None:
            self.clone_source(plan, mode, dest)
            return
        self._git(["clone", "--no-local", "--no-checkout", str(plan.nucleus_source), str(dest)], cwd=None)
        self._git(["checkout", "--detach", NUCLEUS_COMMIT], cwd=str(dest))

    def _verify_source(self, source: Path) -> None:
        head = self._git_out(["rev-parse", "HEAD"], cwd=str(source)).strip()
        tree = self._git_out(["rev-parse", "HEAD^{tree}"], cwd=str(source)).strip()
        if head != NUCLEUS_COMMIT or tree != NUCLEUS_TREE:
            raise ComparisonError("refusal", "source_identity", f"head={head} tree={tree}")
        lock = source / "Cargo.lock"
        digest = self._digest_lock(lock)
        if digest != CARGO_LOCK_SHA256:
            raise ComparisonError("refusal", "source_identity", "Cargo.lock digest mismatch")
        listed = self._git_out(["ls-files"], cwd=str(source))
        for name in (".cargo/config", ".cargo/config.toml", "rust-toolchain", "rust-toolchain.toml"):
            if name in listed.splitlines():
                raise ComparisonError("refusal", "config_identity", f"tracked {name}")

    def _digest_lock(self, path: Path) -> str:
        if self.lock_sha256 is not None:
            return self.lock_sha256(path)
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def _git(self, argv: list[str], cwd: str | None) -> None:
        self._git_out(argv, cwd=cwd)

    def _git_out(self, argv: list[str], cwd: str | None) -> str:
        if self.git is not None:
            result = self.git(argv, cwd=cwd)
            return result if isinstance(result, str) else ""
        command = ["git"]
        if cwd:
            command.extend(["-C", cwd])
        command.extend(argv)
        result = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=max(1, self._remaining_hard() / 1_000_000_000),
        )
        if result.returncode != 0:
            raise ComparisonError("refusal", "source_identity", result.stderr[-400:])
        return result.stdout

    def _restore_tracked(self, source: Path) -> None:
        argv = ["checkout", "--", *EDIT_FILES]
        if self.git is not None:
            self.git(argv, cwd=str(source))
            return
        result = subprocess.run(
            ["git", "-C", str(source), *argv],
            capture_output=True,
            text=True,
            timeout=max(1, self._remaining_hard() / 1_000_000_000),
        )
        if result.returncode != 0:
            raise ComparisonError("partial", "restore", result.stderr[-400:])

    def _verify_restore(self, source: Path) -> None:
        packages = source / "packages"
        moved = source / "packages.oxide-moved"
        if not packages.is_dir():
            raise ComparisonError("partial", "restore", "packages directory missing after restore")
        if moved.exists():
            raise ComparisonError("partial", "restore", "packages.oxide-moved still present")
        porcelain = self._git_out(["status", "--porcelain"], cwd=str(source))
        if porcelain.strip():
            raise ComparisonError(
                "partial",
                "restore",
                f"dirty after restore: {porcelain.strip()[:200]}",
            )
        digest = self._digest_lock(source / "Cargo.lock")
        if digest != CARGO_LOCK_SHA256:
            raise ComparisonError("partial", "restore", "Cargo.lock digest mismatch after restore")

    def _check_rustc_running_lines(self, text: str) -> None:
        for line in text.splitlines():
            tick = _BACKTICK_RUNNING.match(line)
            if tick is None:
                continue
            tokens = tick.group(1).strip().split()
            if not tokens:
                continue
            first = tokens[0]
            rest = " ".join(tokens[1:])
            looks_like_rustc = (
                Path(first).name == "rustc"
                or Path(first).name.startswith("rustc-")
                or "--crate-name" in rest
                or "--emit" in rest
            )
            if not looks_like_rustc:
                continue
            name = Path(first).name
            if name != "rustc" and not name.startswith("rustc-"):
                raise ComparisonError(
                    "refusal",
                    "rustc_wrapper",
                    f"rustc Running line first token is not rustc: {first}",
                )

    def _record_invocation(
        self, plan: ResourcePlan, mode: str, step: str, result: RunResult, target: Path
    ) -> None:
        deps = target / "debug" / "deps"
        incremental = target / "incremental"
        roots = [path for path in (deps, incremental) if path.exists()]
        if roots:
            acc = union_account(
                roots,
                owned_roots=[plan.mount, plan.parent],
                clock=self.clock,
                remaining_ns=self._remaining_hard(),
            )
        else:
            acc = Accounting()
        streams = result.stdout + "\n" + result.stderr
        compiling = len(_COMPILING_RE.findall(streams))
        fresh = len(_FRESH_RE.findall(streams))
        test_line = None
        for line in streams.splitlines():
            stripped = line.strip()
            if stripped.startswith("test result:"):
                test_line = stripped
        deps_files, objects, dsyms = deps_artifact_counts(deps)
        inc_files, _, _ = deps_artifact_counts(incremental)
        rec = {
            "mode": mode,
            "step": step,
            "utc_end": self.clock.utc_iso(),
            "duration_ns": result.duration_ns,
            "exit_code": result.exit_code,
            "compiling": compiling,
            "fresh": fresh,
            "test_result": test_line,
            "deps_files": deps_files,
            "object_files": objects,
            "dsym_count": dsyms,
            "incremental_files": inc_files,
            "union_logical_bytes": acc.logical_bytes,
            "union_allocated_bytes": acc.allocated_bytes,
            "union_file_count": acc.file_count,
            "union_hardlink_dedup": acc.hardlink_dedup_count,
            "apfs_clone_sharing": acc.apfs_clone_sharing,
        }
        self.measurements.append(rec)
        self.log.write(
            f"measure {mode} {step} duration_ns={result.duration_ns} exit={result.exit_code} "
            f"compiling={compiling} fresh={fresh} test={test_line} "
            f"deps_files={deps_files} o={objects} dsym={dsyms} "
            f"logical={acc.logical_bytes} allocated={acc.allocated_bytes}",
            critical=True,
        )

    def run_timed_helper(self, argv: list[str], *, env: Mapping[str, str] | None = None, cwd: str | None = None) -> RunResult:
        if self.session is None:
            self._open_session()
        return self._run_helper(argv, env=env or {}, cwd=cwd)

    def _poll_sample(self) -> None:
        if self.active_plan is None or self.session is None:
            return
        now = self._now()
        if now - self.last_sample_ns < SAMPLE_CADENCE_NS:
            return
        self._sample(self.active_plan, reason="cadence")

    def _sample(self, plan: ResourcePlan, *, reason: str = "boundary") -> None:
        if self._remaining_hard() <= 0:
            raise ComparisonError("deadline", "deadline", "expired before sample")
        host = self._host_free()
        if host < HOST_FREE_RESERVE:
            raise ComparisonError("refusal", "host_reserve", f"host free {host}")
        info = parse_imageinfo(self.image.imageinfo(plan.backing, self._remaining_hard()))
        used = info["non_empty_bytes"]
        if used >= IMAGE_USED_THRESHOLD:
            raise ComparisonError("refusal", "image_threshold", f"image used {used}")
        total = footprint_sum(backing=plan.backing, extras=[plan.parent / "control"], imageinfo=info)
        if total > TOTAL_FOOTPRINT_LIMIT:
            raise ComparisonError("refusal", "footprint", f"owned footprint {total} exceeds 16 GiB")
        self.last_sample_ns = self._now()
        self.sample_count += 1
        self.peak_image_used = max(self.peak_image_used, used)
        if self.min_host_free is None:
            self.min_host_free = host
        else:
            self.min_host_free = min(self.min_host_free, host)
        self.log.write(
            f"sample reason={reason} host_free={host} image_used={used} footprint={total} "
            f"peak_image_used={self.peak_image_used} min_host_free={self.min_host_free}",
            critical=True,
        )

    def _host_free(self) -> int:
        if self.space_probe is not None:
            value = self.space_probe()
            if value is None:
                raise ComparisonError("refusal", "host_reserve", "host free space unknown")
            return int(value)
        st = os.statvfs("/private/var/tmp")
        return int(st.f_bavail) * int(st.f_frsize)

    def _deliver(
        self, mode: str, name: str, argv: list[str], env: Mapping[str, str], cwd: str
    ) -> RunResult:
        self.counter.before_delivery(mode, name)
        if self.session is None or self.session.expired_for_delivery(self._now()):
            raise ComparisonError("deadline", "deadline", "expired before counted delivery")
        logged = redact_env_for_log(env)
        self.log.write(f"deliver {mode} {name} argv={argv} env={json.dumps(logged, sort_keys=True)}")
        result = self.spawner.run(
            argv,
            env=env,
            cwd=cwd,
            remaining_ns=self._remaining_hard(),
            log=self.log,
            clock=self.clock,
            session=self.session,
            poll=self._poll_sample,
        )
        if result.pgid is not None:
            self.helper_pgids.append(result.pgid)
        if result.outcome == "deadline":
            raise ComparisonError("deadline", "deadline", f"{mode} {name} hit deadline")
        if result.outcome == "unknown" or (result.spawned and not result.writer_closed):
            self.resources_retained = True
            raise ComparisonError("unknown", "writer_closure_unknown", f"{mode} {name} writer closure unknown")
        combined = result.stdout + result.stderr
        if argv and "effigy" in argv[0] and result.exit_code not in {0, None}:
            if "cargo test" in combined and "effigy" not in combined:
                raise ComparisonError("refusal", "raw_cargo_fallback", "raw cargo fallback is forbidden")
        return result

    def _run_helper(self, argv: list[str], *, env: Mapping[str, str], cwd: str) -> RunResult:
        if self.session is None or self.session.expired_for_delivery(self._now()):
            raise ComparisonError("deadline", "deadline", "expired before helper")
        result = self.spawner.run(
            argv,
            env=env,
            cwd=cwd,
            remaining_ns=self._remaining_hard(),
            log=self.log,
            clock=self.clock,
            session=self.session,
            poll=self._poll_sample,
        )
        if result.pgid is not None:
            self.helper_pgids.append(result.pgid)
        if result.outcome == "unknown" or (result.spawned and not result.writer_closed):
            self.resources_retained = True
        return result

    def _result_document(self, plan: ResourcePlan, session: SessionClock) -> dict[str, object]:
        return {
            "effects_executed": self.effects_executed,
            "live": True,
            "outcome": self.outcome,
            "seam": self.seam,
            "resources_retained": self.resources_retained,
            "preparation_after_clock": self.preparation_after_clock,
            "session": {
                "start_utc": session.start_utc,
                "start_monotonic_ns": session.start_monotonic_ns,
                "deadline_monotonic_ns": session.deadline_monotonic_ns,
            },
            "count_delivered": self.counter.delivered,
            "helper_pgids": list(self.helper_pgids),
            "log_overflow": self.log.overflow,
            "log": self.log.text(),
            "measurements": list(self.measurements),
            "sampling": {
                "cadence_ns": SAMPLE_CADENCE_NS,
                "sample_count": self.sample_count,
                "peak_image_used": self.peak_image_used,
                "min_host_free": self.min_host_free,
            },
            "containment": {
                "parent": str(plan.parent),
                "mount": str(plan.mount),
                "backing": str(plan.backing),
            },
        }


def _argv_for(mode: str, name: str) -> tuple[str, ...]:
    if name == "cargo-vV":
        return ("cargo", "-vV")
    if name == "rustc-vV":
        return ("rustc", "-vV")
    return EFFIGY_TEST_ARGV


def _copy_file(src: Path, dest: Path) -> int:
    data = src.read_bytes()
    dest.write_bytes(data)
    os.chmod(dest, 0o600)
    return len(data)


def load_resource_plan(path: Path) -> ResourcePlan:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ComparisonError("refusal", "resource_plan", "resource plan must be an object")
    return ResourcePlan.from_mapping(data)


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    live = "--live" in args
    plan_path = None
    if "--resource-plan" in args:
        index = args.index("--resource-plan")
        if index + 1 >= len(args):
            print("missing --resource-plan path", file=sys.stderr)
            return 2
        plan_path = Path(args[index + 1])
    resource = load_resource_plan(plan_path) if plan_path else None
    launcher = ComparisonLauncher()
    try:
        document = launcher.run(live=live, resource_plan=resource)
    except ComparisonError as error:
        document = {
            "effects_executed": launcher.effects_executed,
            "live": live,
            "outcome": error.outcome,
            "seam": error.seam,
            "message": error.message,
            "resources_retained": launcher.resources_retained,
        }
        print(json.dumps(document, indent=2, sort_keys=True))
        return 2
    print(json.dumps(document, indent=2, sort_keys=True))
    return 0 if document.get("outcome") in {"plan", "success"} else 2


if __name__ == "__main__":
    sys.exit(main())
