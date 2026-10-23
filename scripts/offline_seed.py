#!/usr/bin/env python3
"""Bounded private offline Cargo seed from one Cargo.lock (oxide#131).

One shared helper for proof fixtures that need a private, offline CARGO_HOME
without a download. It starts from the #112 bounded seed (`seed_from_lock`,
`seed_git`, `copy_bounded` in `scripts/test-pool-describe.py`) and corrects two
defects that proof disclosed rather than closed:

- **Anchored reads.** Every source path is opened descriptor-relative with
  `O_NOFOLLOW` on each component from one opened root, on the same device as
  that root. A regular file is checked before and after its bytes are read
  (identity, size, mtime and ctime), and the name is reopened afterwards to
  prove it still resolves to the same inode. A changed, symlinked, special,
  foreign-device or unknown source refuses.
- **No host Git.** The old read-only prefilter ran `git rev-parse` against a
  host repository, whose own local config, includes, hooks and alternates were
  then in play. Now no Git ever runs on a host repository. Candidate selection
  only measures a candidate's regular-file bytes through the same anchored
  descriptors, which reads no config, hook, include or alternate and
  establishes no commit. Every candidate that fits the remaining aggregate is
  copied first through the anchored bounded copier, host `config` and `hooks`
  are never copied, a known private config is generated, and
  `objects/info/alternates` refuses. Only then is the locked commit read
  inside the sanitized private copy, with system and global config disabled,
  hooks and transports disabled and lazy fetch off. A copied candidate that
  does not carry the locked commit is rejected and its bytes stay charged; a
  candidate measured over the remaining budget is not attempted, its entries
  stay charged, and its record says so. The seed refuses only when no
  attempted candidate resolves the commit. No byte, entry or metadata charge
  is reset per attempt and copied bytes are never removed.

Bounds, all charged across the whole seed with no per-attempt reset:

- 256 MiB of copied bytes, checked before each write and before the generated
  private git config is written;
- at most 8 Git candidates for one locked source, refused above that before
  any copy;
- at most 100,000 enumerated entries and a depth of 64 while copying a Git
  candidate; no unbounded list, sort or tree copy;
- at most 1 MiB for one manifest read and 8 MiB of manifest reads in total;
- at most 64 KiB of Git command output, and 10 s for one Git command;
- at most 45 s of cumulative seed work, and never past a caller's absolute
  `CLOCK_UPTIME_RAW` deadline when one is supplied.

Registry input is exactly the lock selection from the one supported crates.io
sparse namespace: `config.json`, one sparse-index `.cache` entry per locked
package and one `.crate` archive per package whose bytes must match the lock
checksum. Workspace and path packages are skipped; any other source, a
duplicate or malformed lock entry, and a crate name, version or checksum that
cannot be a safe path component refuses.

Not claimed: this seed is offline fixture input, not source qualification. A
caller's proof stays bound to the exact sources it ran; importing this helper
transfers no qualification. The registry namespace is fixed; a signed or
alternative registry is out of scope. Cargo still decides freshness after
staging.

Run `python3 scripts/offline_seed.py --self-test ROOT` for the synthetic
cases: registry+git positive, a copied-but-rejected candidate, a measured and
unattempted oversized candidate, a wrong commit, a missing candidate, a
malformed lock, symlinked inputs and the byte, candidate and entry cap
negatives. They create their fixtures only under ROOT and touch no host cache.
"""
from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import errno
import hashlib
import os
import re
import shutil
import stat
import subprocess
import time
from pathlib import Path

MIB = 1024 * 1024
CHUNK = 1 * MIB
# The only registry sources the seed serves: crates.io, git index or sparse.
CRATES_SOURCES = (
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
)
# The one supported crates.io sparse namespace, matched on index and cache.
CRATES_NAMESPACE = "index.crates.io-1949cf8c6b5b557f"
# Host config, hooks and alternates are never copied. This is the private config.
GIT_CONFIG = b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n"
NAME_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")
VERSION_RE = re.compile(r"^[0-9]+[.][0-9]+[.][0-9]+(?:[-+][0-9A-Za-z.+-]+)?$")
CHECKSUM_RE = re.compile(r"^[0-9a-f]{64}$")
REV_RE = re.compile(r"^[0-9a-f]{40}$")
BACKSLASH = chr(92)
PATH_BASE = "/usr/bin:/bin:/usr/sbin:/sbin"
GIT_TIMEOUT_S = 10


class SeedRefusal(RuntimeError):
    """The seed cannot be built safely from the lock and the host cache."""


def now_ns() -> int:
    """The shared runner's clock: CLOCK_UPTIME_RAW integer nanoseconds."""
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


class Limits:
    """Every seed bound in one place; synthetic cases may lower them."""

    def __init__(
        self,
        aggregate_bytes: int = 256 * MIB,
        git_candidates: int = 8,
        entries: int = 100_000,
        depth: int = 64,
        manifest_bytes: int = 1 * MIB,
        manifest_aggregate_bytes: int = 8 * MIB,
        git_output_bytes: int = 64 * 1024,
        work_seconds: float = 45.0,
    ) -> None:
        self.aggregate_bytes = aggregate_bytes
        self.git_candidates = git_candidates
        self.entries = entries
        self.depth = depth
        self.manifest_bytes = manifest_bytes
        self.manifest_aggregate_bytes = manifest_aggregate_bytes
        self.git_output_bytes = git_output_bytes
        self.work_seconds = work_seconds

    def document(self) -> dict:
        return {
            "aggregate_bytes": self.aggregate_bytes,
            "git_candidates": self.git_candidates,
            "entries": self.entries,
            "depth": self.depth,
            "manifest_bytes": self.manifest_bytes,
            "manifest_aggregate_bytes": self.manifest_aggregate_bytes,
            "git_output_bytes": self.git_output_bytes,
            "work_seconds": self.work_seconds,
        }


LIMITS = Limits()


class Budget:
    """Aggregate charges and clocks for one seed. Nothing is reset per attempt."""

    def __init__(self, limits: Limits, deadline_ns: int | None = None) -> None:
        self.limits = limits
        self.deadline_ns = deadline_ns
        self.used = 0
        self.entries_used = 0
        self.manifest_used = 0
        self.started = time.monotonic()

    def check(self, what: str) -> None:
        if self.deadline_ns is not None and now_ns() >= self.deadline_ns:
            raise SeedRefusal(f"{what}: reached the caller's absolute deadline")
        elapsed = time.monotonic() - self.started
        if elapsed > self.limits.work_seconds:
            raise SeedRefusal(
                f"{what}: seed work exceeded {self.limits.work_seconds:g} s in total"
            )

    def remaining_seconds(self) -> float:
        left = self.limits.work_seconds - (time.monotonic() - self.started)
        if self.deadline_ns is not None:
            left = min(left, (self.deadline_ns - now_ns()) / 1_000_000_000)
        return max(left, 0.0)

    def charge_entries(self, count: int, what: str) -> None:
        self.check(what)
        self.entries_used += count
        if self.entries_used > self.limits.entries:
            raise SeedRefusal(
                f"{what}: more than {self.limits.entries} enumerated entries in the seed"
            )

    def charge_manifest(self, size: int, what: str) -> None:
        self.check(what)
        if size > self.limits.manifest_bytes:
            raise SeedRefusal(
                f"{what}: {size} bytes exceeds the {self.limits.manifest_bytes}-byte single-manifest read bound"
            )
        self.manifest_used += size
        if self.manifest_used > self.limits.manifest_aggregate_bytes:
            raise SeedRefusal(
                f"{what}: more than {self.limits.manifest_aggregate_bytes} manifest bytes read in the seed"
            )

    def charge_write(self, size: int, what: str) -> None:
        """Check before the write, then charge. Rejected candidates keep theirs."""
        self.check(what)
        if self.used + size > self.limits.aggregate_bytes:
            raise SeedRefusal(
                f"{what}: copied bytes would exceed the {self.limits.aggregate_bytes}-byte aggregate bound; "
                "refused before the write"
            )
        self.used += size


def sparse_index_path(name: str) -> tuple[str, ...]:
    """The sparse-index path of a crate, lowercase, as Cargo lays it out."""
    lower = name.lower()
    if len(lower) == 1:
        return ("1", lower)
    if len(lower) == 2:
        return ("2", lower)
    if len(lower) == 3:
        return ("3", lower[0], lower)
    return (lower[:2], lower[2:4], lower)


def _check_part(part: str, label: str) -> str:
    """A name that may be opened relative to a directory descriptor."""
    if (
        not part
        or part in (".", "..")
        or "/" in part
        or BACKSLASH in part
        or "\0" in part
    ):
        raise SeedRefusal(f"{label}: unsafe path component {part!r}")
    return part


class Anchor:
    """One opened root; every read below it is descriptor-relative, no-follow."""

    def __init__(self, path: Path, label: str) -> None:
        try:
            self.fd = os.open(str(path), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        except FileNotFoundError:
            raise SeedRefusal(f"{label} is missing: {path}")
        except OSError as error:
            raise SeedRefusal(f"{label} cannot be opened without following links: {path}: {error}")
        info = os.fstat(self.fd)
        if not stat.S_ISDIR(info.st_mode):
            os.close(self.fd)
            raise SeedRefusal(f"{label} is not a directory: {path}")
        self.label = label
        self.dev = info.st_dev
        self.identity = (info.st_dev, info.st_ino)

    def close(self) -> None:
        os.close(self.fd)

    def __enter__(self) -> "Anchor":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def open_dir(self, parts: tuple[str, ...]) -> int:
        fd = os.dup(self.fd)
        for part in parts:
            _check_part(part, self.label)
            try:
                child = os.open(
                    part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
                )
            except FileNotFoundError:
                os.close(fd)
                raise SeedRefusal(f"required seed directory is missing: {'/'.join(parts)}")
            except OSError as error:
                os.close(fd)
                reason = "a symlink or link loop" if error.errno == errno.ELOOP else "not a real directory"
                raise SeedRefusal(
                    f"seed directory is {reason}: {'/'.join(parts)}"
                )
            info = os.fstat(child)
            if not stat.S_ISDIR(info.st_mode) or info.st_dev != self.dev:
                os.close(child)
                os.close(fd)
                raise SeedRefusal(
                    f"seed directory is not a real directory on the seed's device: {'/'.join(parts)}"
                )
            os.close(fd)
            fd = child
        return fd

    def open_file(self, parts: tuple[str, ...]) -> tuple[int, os.stat_result]:
        if not parts:
            raise SeedRefusal(f"{self.label}: empty file path")
        dfd = self.open_dir(parts[:-1])
        try:
            fd = os.open(
                _check_part(parts[-1], self.label),
                os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                dir_fd=dfd,
            )
        except FileNotFoundError:
            raise SeedRefusal(f"required seed input is missing: {'/'.join(parts)}")
        except OSError as error:
            reason = (
                "is a symlink or link loop and is refused"
                if error.errno == errno.ELOOP
                else f"cannot be opened without following links: {error}"
            )
            raise SeedRefusal(f"seed input {reason}: {'/'.join(parts)}")
        finally:
            os.close(dfd)
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            os.close(fd)
            raise SeedRefusal(f"seed input is not a regular file: {'/'.join(parts)}")
        if info.st_dev != self.dev:
            os.close(fd)
            raise SeedRefusal(
                f"seed input is not on the seed root's device: {'/'.join(parts)}"
            )
        return fd, info

    def file_identity(self, parts: tuple[str, ...]) -> tuple[int, int]:
        fd, _ = self.open_file(parts)
        try:
            info = os.fstat(fd)
            return (info.st_dev, info.st_ino)
        finally:
            os.close(fd)


def _copy_file(
    anchor: Anchor,
    parts: tuple[str, ...],
    destination: Path,
    budget: Budget,
    expected_sha256: str | None,
    label: str,
) -> dict:
    """Copy one anchored regular file in bounded chunks, before/after checked."""
    budget.charge_entries(1, label)
    fd, before = anchor.open_file(parts)
    try:
        budget.check(label)
        if budget.used + before.st_size > budget.limits.aggregate_bytes:
            raise SeedRefusal(
                f"{label}: copied bytes would exceed the {budget.limits.aggregate_bytes}-byte "
                "aggregate bound; refused before the write"
            )
        destination.parent.mkdir(parents=True, exist_ok=True)
        digest = hashlib.sha256()
        written = 0
        with os.fdopen(fd, "rb", closefd=True) as reader, open(destination, "xb") as writer:
            fd = -1
            while True:
                chunk = reader.read(CHUNK)
                if not chunk:
                    break
                budget.charge_write(len(chunk), label)
                written += len(chunk)
                digest.update(chunk)
                writer.write(chunk)
            after = os.fstat(reader.fileno())
        changed = (
            before.st_dev != after.st_dev
            or before.st_ino != after.st_ino
            or before.st_size != after.st_size
            or before.st_mtime_ns != after.st_mtime_ns
            or before.st_ctime_ns != after.st_ctime_ns
        )
        if changed or written != before.st_size:
            raise SeedRefusal(
                f"{label}: the source changed while it was copied: {'/'.join(parts)}"
            )
    finally:
        if fd >= 0:
            os.close(fd)
    if anchor.file_identity(parts) != (before.st_dev, before.st_ino):
        raise SeedRefusal(
            f"{label}: the source name no longer resolves to the copied inode: {'/'.join(parts)}"
        )
    actual = digest.hexdigest()
    if expected_sha256 is not None and actual != expected_sha256:
        raise SeedRefusal(
            f"checksum mismatch for {parts[-1]}: copied bytes {actual}, lock {expected_sha256}"
        )
    return {"sha256": actual, "bytes": written}


def _git_db_name(url: str) -> str:
    """Cargo's local Git database name: the repository name from the URL."""
    name = url.rstrip("/").rsplit("/", 1)[-1]
    if name.endswith(".git"):
        name = name[: -len(".git")]
    if not NAME_RE.match(name):
        raise SeedRefusal(f"locked Git source has an unsafe repository name: {name!r}")
    return name


def _run_git(home: Path, args: list, budget: Budget, label: str) -> tuple[int, str, str]:
    """Git over a private copy only: sanitized environment, bounded output/time.

    No session is started. A timeout kills that direct child PID and reports
    closure unknown, so a caller never treats it as a clean stop.
    """
    git = shutil.which("git")
    if git is None:
        raise SeedRefusal("git is not on the host PATH")
    env = {
        "PATH": PATH_BASE,
        "HOME": str(home),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_NO_LAZY_FETCH": "1",
        "GIT_OPTIONAL_LOCKS": "0",
    }
    timeout = max(0.1, min(GIT_TIMEOUT_S, budget.remaining_seconds()))
    budget.check(label)
    child = subprocess.Popen(
        [git, *args], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    try:
        out, err = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        child.kill()
        child.communicate()
        raise SeedRefusal(
            f"{label}: git did not answer within {timeout:.1f} s; that child's closure is unknown"
        )
    if len(out) > budget.limits.git_output_bytes or len(err) > budget.limits.git_output_bytes:
        raise SeedRefusal(f"{label}: git output exceeds the 64 KiB bound")
    return (
        child.returncode,
        out.decode("utf-8", "replace"),
        err.decode("utf-8", "replace"),
    )


def _git_args(git_dir: Path, *rest: str) -> list:
    return [
        "-c",
        "protocol.allow=never",
        "-c",
        "core.hooksPath=/dev/null",
        "--git-dir",
        str(git_dir),
        *rest,
    ]


def _rev_parse(home: Path, git_dir: Path, spec: str, budget: Budget, label: str) -> str:
    code, out, _ = _run_git(
        home, _git_args(git_dir, "rev-parse", "--verify", spec), budget, label
    )
    return out.strip() if code == 0 else ""


def parse_lock_packages(text: str, budget: Budget) -> tuple[list[dict], list[dict]]:
    """Registry and pinned Git packages of one Cargo.lock, as its fixed grammar.

    Top-level `version = N`, `[[package]]` tables with basic string keys and
    `dependencies` arrays of basic strings, which are skipped. A table without
    `source` is a workspace or path package and is skipped; it still needs a
    name and version. Refused: any other source, a malformed or path-unsafe
    name, version, checksum or revision, an unknown key, a repeated key in one
    table, a repeated top-level version, and a repeated package.
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

    registry: list[dict] = []
    git_packages: list[dict] = []
    seen: set[tuple[str, str]] = set()
    for table in tables:
        if "name" not in table or "version" not in table:
            raise SeedRefusal("a package table without a name or version is malformed")
        if "source" not in table:
            if "checksum" in table:
                raise SeedRefusal(f"{table['name']} carries a checksum but no source")
            continue
        source = table["source"]
        if source.startswith("git+"):
            package = _locked_git_package(table)
            if (package["name"], package["version"]) in seen:
                raise SeedRefusal(f"duplicate lock entry for {package['name']} {package['version']}")
            seen.add((package["name"], package["version"]))
            git_packages.append(package)
            continue
        if source not in CRATES_SOURCES:
            raise SeedRefusal(f"unsupported registry source: {source}")
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
        registry.append(
            {
                "name": name,
                "version": version,
                "source": source,
                "checksum": checksum,
            }
        )
    budget.charge_entries(len(registry) + len(git_packages), "lock packages")
    return registry, git_packages


def _locked_git_package(table: dict) -> dict:
    """One pinned Git source: exact URL, name and full 40-hex locked commit."""
    source = table["source"][len("git+"):]
    if "#" not in source:
        raise SeedRefusal(f"locked Git crate {table['name']} has no revision")
    url, revision = source.rsplit("#", 1)
    if not REV_RE.match(revision):
        raise SeedRefusal(f"locked Git crate {table['name']} has no full 40-hex revision")
    repository_url = url.split("?", 1)[0]
    return {
        "name": table["name"],
        "version": table["version"],
        "source": table["source"],
        "url": url,
        "repository_url": repository_url,
        "repository": _git_db_name(repository_url),
        "rev": revision,
    }


def _list_names(anchor: Anchor, parts: tuple[str, ...], budget: Budget, label: str) -> list[str]:
    dfd = anchor.open_dir(parts)
    try:
        names = []
        with os.scandir(dfd) as entries:
            for entry in entries:
                budget.charge_entries(1, label)
                names.append(entry.name)
        return sorted(names)
    finally:
        os.close(dfd)


def _walk_git(
    anchor: Anchor,
    parts: tuple[str, ...],
    rel: tuple[str, ...],
    budget: Budget,
    depth: int,
    on_file,
) -> None:
    """Walk one bare database through anchored descriptors, bounded.

    Host `config` and `hooks` are skipped at the top level: the private config
    is generated. Alternates, symlinks, special entries and foreign devices
    refuse. Every enumerated entry and every level beyond `limits.depth` is
    charged to the one seed budget; nothing is listed or walked unbounded.
    `on_file(host_parts, rel_parts, stat)` decides what happens to a file.
    """
    if depth > budget.limits.depth:
        raise SeedRefusal(f"Git database depth exceeds {budget.limits.depth}: {'/'.join(parts)}")
    dfd = anchor.open_dir(parts)
    try:
        names = []
        with os.scandir(dfd) as entries:
            for entry in entries:
                budget.charge_entries(1, "git database entry")
                names.append(entry.name)
        for name in sorted(names):
            _check_part(name, "git database entry")
            if not parts and name in ("config", "hooks"):
                continue
            if parts == ("objects", "info") and name in ("alternates", "http-alternates"):
                raise SeedRefusal(f"Git database alternates entry refused: {'/'.join(parts + (name,))}")
            try:
                info = os.stat(name, dir_fd=dfd, follow_symlinks=False)
            except FileNotFoundError:
                raise SeedRefusal(f"Git database entry disappeared: {'/'.join(parts + (name,))}")
            child = parts + (name,)
            child_rel = rel + (name,)
            if stat.S_ISDIR(info.st_mode):
                if info.st_dev != anchor.dev:
                    raise SeedRefusal(
                        f"Git database directory is not on the seed root's device: {'/'.join(child)}"
                    )
                _walk_git(anchor, child, child_rel, budget, depth + 1, on_file)
            elif stat.S_ISREG(info.st_mode):
                on_file(child, child_rel, info)
            else:
                raise SeedRefusal(
                    f"Git database entry is a symlink or special file: {'/'.join(child)}"
                )
    finally:
        os.close(dfd)


def _measure_git(anchor: Anchor, parts: tuple[str, ...], budget: Budget) -> int:
    """Admission measurement only: the regular-file bytes a candidate holds.

    This is not a commit or content check and never reads host Git config. A
    candidate measured over the remaining byte budget is not attempted; its
    entries stay charged and the record names the reason.
    """
    total = 0

    def count(_host_parts: tuple, _rel_parts: tuple, info: os.stat_result) -> None:
        nonlocal total
        total += info.st_size

    _walk_git(anchor, parts, (), budget, 0, count)
    return total


def _seed_registry(
    host: Anchor, cargo_home: Path, packages: list[dict], budget: Budget
) -> dict:
    """The lock selection from the one supported crates.io sparse namespace."""
    budget.check("registry seed")
    namespace = CRATES_NAMESPACE
    index_parts = ("registry", "index", namespace)
    cache_parts = ("registry", "cache", namespace)
    # Both selected namespace directories must be real directories, checked
    # before any descent. Other namespaces are never listed or traversed.
    for parts, label in (
        (index_parts, "host index namespace"),
        (cache_parts, "host cache namespace"),
    ):
        os.close(host.open_dir(parts))
    dest_index = cargo_home / "registry" / "index" / namespace
    dest_cache = cargo_home / "registry" / "cache" / namespace
    manifest: list[dict] = []

    def record(parts: tuple[str, ...], destination: Path, checksum: str | None) -> None:
        copied = _copy_file(host, parts, destination, budget, checksum, "registry seed file")
        manifest.append(
            {
                "path": str(destination.relative_to(cargo_home)),
                "sha256": copied["sha256"],
                "bytes": copied["bytes"],
            }
        )

    record(index_parts + ("config.json",), dest_index / "config.json", None)
    for package in packages:
        rel = sparse_index_path(package["name"])
        record(index_parts + (".cache", *rel), dest_index / ".cache" / Path(*rel), None)
        archive = f"{package['name']}-{package['version']}.crate"
        record(cache_parts + (archive,), dest_cache / archive, package["checksum"])
    return {"namespace": namespace, "files": manifest}


def verify_seed_tree(cargo_home: Path, seed: dict) -> None:
    """The private seed holds exactly the selected files, nothing else."""
    expected = {entry["path"] for entry in seed["files"]}
    for candidate in seed["git"]["candidates"]:
        expected.update(entry["path"] for entry in candidate["files"])
    actual = set()
    for root in ("registry", "git"):
        base = cargo_home / root
        if not base.is_dir():
            continue
        for dirpath, _dirnames, filenames in os.walk(base):
            for name in filenames:
                actual.add(str(Path(dirpath, name).relative_to(cargo_home)))
    if actual != expected:
        raise SeedRefusal(
            "the private seed differs from the lock selection: "
            f"extra {sorted(actual - expected)[:3]}, missing {sorted(expected - actual)[:3]}"
        )


def _seed_git(
    host: Anchor, cargo_home: Path, git_packages: list[dict], budget: Budget
) -> dict:
    """Copy bare-database candidates, then verify the locked commit privately.

    No host Git runs: candidate selection only measures regular-file bytes
    through anchored descriptors, which reads no host config, hook, include or
    alternate and establishes no commit. Every candidate that fits the
    remaining aggregate is copied first (host `config` and `hooks` skipped, a
    known private config generated, alternates refused), and only then is the
    locked commit read inside the sanitized private copy. A candidate that does
    not resolve stays copied and charged; a candidate measured over the
    remaining budget is not attempted, its entries stay charged, and its
    record says so. No byte, entry or metadata charge is reset per attempt.
    """
    if not git_packages:
        return {"candidates": []}
    # One locked Git source per seed keeps the aggregate accounting legible;
    # more than that is an unsupported lock shape, not a silent partial seed.
    if len(git_packages) > 1:
        raise SeedRefusal("more than one locked Git source; the seed serves one")
    package = git_packages[0]
    host_db = ("git", "db")
    candidate_re = re.compile(r"^" + re.escape(package["repository"]) + r"-[0-9a-f]{16}$")
    names = [
        name
        for name in _list_names(host, host_db, budget, "host git database root")
        if candidate_re.match(name)
    ]
    if not 1 <= len(names) <= budget.limits.git_candidates:
        raise SeedRefusal(
            f"expected 1 to {budget.limits.git_candidates} host {package['repository']} bare databases, found {len(names)}"
        )
    candidates: list[dict] = []
    home = cargo_home / "seed-home"
    home.mkdir(parents=True, exist_ok=True)
    attempted = 0
    try:
        for index, name in enumerate(names):
            label = f"git candidate {index}"
            budget.check(label)
            parts = host_db + (name,)
            destination = cargo_home / "git" / "db" / name
            record = {
                "database": name,
                "measured_bytes": 0,
                "copied": False,
                "bytes": 0,
                "filecount": 0,
                "commit_present": None,
                "rejected": None,
                "config_sha256": None,
                "generated": True,
                "skipped": None,
                "files": [],
            }
            candidates.append(record)
            measured = _measure_git(host, parts, budget)
            record["measured_bytes"] = measured
            if budget.used + measured > budget.limits.aggregate_bytes:
                record["skipped"] = (
                    "measured bytes would exceed the remaining 256 MiB aggregate; not attempted"
                )
                continue
            attempted += 1
            before = budget.used

            def copy_one(child: tuple, child_rel: tuple, _info: os.stat_result) -> None:
                copied = _copy_file(
                    host,
                    child,
                    destination.joinpath(*child_rel),
                    budget,
                    None,
                    "git database file",
                )
                record["files"].append(
                    {
                        "path": f"git/db/{name}/{'/'.join(child_rel)}",
                        "sha256": copied["sha256"],
                        "bytes": copied["bytes"],
                    }
                )

            _walk_git(host, parts, (), budget, 0, copy_one)
            budget.charge_write(len(GIT_CONFIG), label)
            (destination / "config").write_bytes(GIT_CONFIG)
            digest = hashlib.sha256(GIT_CONFIG).hexdigest()
            record["files"].append(
                {
                    "path": f"git/db/{name}/config",
                    "sha256": digest,
                    "bytes": len(GIT_CONFIG),
                    "generated": True,
                }
            )
            record.update(
                copied=True,
                bytes=budget.used - before,
                filecount=len(record["files"]),
                config_sha256=digest,
            )
            if not (destination / "HEAD").is_file():
                raise SeedRefusal(f"private copy of {name} has no HEAD")
            resolved = _rev_parse(
                home,
                destination,
                f"{package['rev']}^{{commit}}",
                budget,
                f"git-rev-commit-{index}",
            )
            record["commit_present"] = resolved == package["rev"]
            if not record["commit_present"]:
                # A copied candidate that does not carry the locked commit is
                # rejected, not a failure: its bytes stay charged and its
                # record says why. The seed refuses only if none resolves.
                record["rejected"] = "the private copy does not resolve the locked commit"
    except SeedRefusal:
        raise
    if not any(record["commit_present"] for record in candidates):
        if not attempted:
            raise SeedRefusal(
                f"no host {package['repository']} database fits the remaining 256 MiB aggregate"
            )
        raise SeedRefusal(
            f"no host {package['repository']} database resolves the locked commit {package['rev']}"
        )
    return {
        "source": package["source"],
        "repository": package["repository"],
        "rev": package["rev"],
        "name": package["name"],
        "version": package["version"],
        "candidates": candidates,
    }


def seed_cargo_home(
    host_cargo_home: Path | str,
    cargo_home: Path | str,
    lock_bytes: bytes,
    *,
    limits: Limits = LIMITS,
    deadline_ns: int | None = None,
) -> dict:
    """Seed one private offline Cargo home from the lock and the host cache.

    Reads only the host cache named by `host_cargo_home`, never runs a network
    command, never writes to the host cache and never removes a charged byte.
    Returns the manifest the caller keeps as evidence.
    """
    budget = Budget(limits, deadline_ns)
    budget.charge_manifest(len(lock_bytes), "Cargo.lock")
    try:
        text = lock_bytes.decode("utf-8")
    except UnicodeDecodeError:
        raise SeedRefusal("Cargo.lock is not UTF-8")
    registry_packages, git_packages = parse_lock_packages(text, budget)
    destination = Path(cargo_home)
    destination.mkdir(parents=True, exist_ok=True, mode=0o700)
    with Anchor(Path(host_cargo_home), "host CARGO_HOME") as host:
        registry = _seed_registry(host, destination, registry_packages, budget)
        git = _seed_git(host, destination, git_packages, budget)
    seed = {
        "lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
        "namespace": registry["namespace"],
        "packages": len(registry_packages),
        "aggregate_bytes": budget.used,
        "entries": budget.entries_used,
        "manifest_bytes": budget.manifest_used,
        "files": registry["files"],
        "git": git,
        "limits": limits.document(),
    }
    verify_seed_tree(destination, seed)
    return seed


def verify_private_checkout(
    cargo_home: Path | str,
    rev: str,
    idents: list[str],
    *,
    limits: Limits = LIMITS,
    deadline_ns: int | None = None,
) -> str:
    """After staging: exactly one private checkout at the locked rev.

    Only the seeded identifiers are considered, and the checkout must be
    inside the private Cargo home. The returned identifier is Cargo's choice.
    """
    if not REV_RE.match(rev):
        raise SeedRefusal(f"not a full 40-hex revision: {rev!r}")
    budget = Budget(limits, deadline_ns)
    checkouts_root = Path(cargo_home) / "git" / "checkouts"
    if not checkouts_root.is_dir():
        raise SeedRefusal("private Cargo home has no checkouts after staging")
    found = sorted(
        name
        for name in os.listdir(checkouts_root)
        if name in idents and (checkouts_root / name / rev[:7]).is_dir()
    )
    if len(found) != 1:
        raise SeedRefusal(
            f"expected exactly one private checkout at {rev[:7]}, found {found}"
        )
    checkout = checkouts_root / found[0] / rev[:7]
    git_dir = checkout / ".git"
    if git_dir.is_symlink() or not git_dir.is_dir():
        raise SeedRefusal("private checkout has no real .git directory to verify")
    head = _rev_parse(checkout, git_dir, "HEAD", budget, "git-rev-checkout")
    if head != rev:
        raise SeedRefusal(f"private checkout HEAD is {head or 'unknown'}, not the locked {rev}")
    return found[0]


# --------------------------------------------------------------------------
# Synthetic cases. Fixtures are created only under the caller's root.


def _write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(data)


def _git(repo: Path, *args: str, cwd: Path | None = None) -> str:
    environment = {
        "PATH": PATH_BASE,
        "HOME": str(repo),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_AUTHOR_NAME": "oxide",
        "GIT_AUTHOR_EMAIL": "oxide@example.invalid",
        "GIT_COMMITTER_NAME": "oxide",
        "GIT_COMMITTER_EMAIL": "oxide@example.invalid",
    }
    result = subprocess.run(
        [shutil.which("git") or "git", *args],
        cwd=str(cwd or repo),
        env=environment,
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode:
        raise RuntimeError(f"fixture git {args} failed: {result.stderr[-500:]}")
    return result.stdout.strip()


def _fixture_host(root: Path, *, archive: bytes = b"synthetic crate archive\n") -> tuple[Path, str, str]:
    """One synthetic host cache: registry input and one bare Git database."""
    host = root / "host-cargo-home"
    namespace = CRATES_NAMESPACE
    index = host / "registry" / "index" / namespace
    cache = host / "registry" / "cache" / namespace
    _write(index / "config.json", b'{"dl":"https://example.invalid","api":null}\n')
    entry = b'{"name":"seedcrate","vers":"1.2.3","deps":[],"cksum":"' + hashlib.sha256(archive).hexdigest().encode() + b'"}\n'
    _write(index / ".cache" / "se" / "ed" / "seedcrate", entry)
    _write(cache / "seedcrate-1.2.3.crate", archive)

    work = root / "git-work"
    work.mkdir(parents=True)
    (work / "README").write_text("synthetic\n")
    _git(work, "init", "-q", "-b", "main")
    _git(work, "add", "README")
    _git(work, "commit", "-q", "-m", "synthetic")
    rev = _git(work, "rev-parse", "HEAD")
    db = host / "git" / "db" / "seedrepo-0123456789abcdef"
    db.parent.mkdir(parents=True)
    _git(work, "init", "-q", "--bare", str(db))
    _git(work, "push", "-q", str(db), "HEAD:refs/heads/main")
    _git(db, "symbolic-ref", "HEAD", "refs/heads/main")
    # A hostile host config: a host-side rev-parse would not read this as a
    # valid repository, and its include must never reach the private copy.
    _write(db / "config", b"[core]\n\trepositoryformatversion = 99\n[include]\n\tpath = /nonexistent\n")
    return host, rev, hashlib.sha256(archive).hexdigest()


def _lock_text(rev: str, checksum: str, *, name: str = "seedcrate", version: str = "1.2.3") -> str:
    return (
        "# synthetic\nversion = 4\n\n[[package]]\n"
        f'name = "{name}"\nversion = "{version}"\n'
        'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
        f'checksum = "{checksum}"\n\n[[package]]\n'
        'name = "seedapp"\nversion = "0.1.0"\n\n[[package]]\n'
        'name = "seedgit"\nversion = "0.1.0"\n'
        'source = "git+https://example.invalid/seedrepo.git?tag=v1#' + rev + '"\n'
    )


def self_test(root: Path | str) -> list[dict]:
    """Run the seeder's synthetic cases; return one record per case.

    Each case gets its own fresh directory under `root`. A failure raises
    `SeedRefusal`/`AssertionError` with the case name so a proof reports it
    rather than claiming the seeder.
    """
    base = Path(root)
    base.mkdir(parents=True, exist_ok=True)
    records: list[dict] = []
    deadline = now_ns() + 120 * 1_000_000_000

    def case(name: str, body) -> None:
        directory = base / name
        directory.mkdir(parents=True)
        try:
            detail = body(directory)
        except Exception as error:
            raise RuntimeError(f"offline seed case {name} failed: {error}") from error
        records.append({"case": name, "passed": True, "detail": detail or ""})

    def expect_refusal(body, wanted: str) -> dict:
        try:
            body()
        except SeedRefusal as error:
            if wanted not in str(error):
                raise AssertionError(f"refused with {str(error)!r}, wanted {wanted!r}")
            return {"refusal": str(error)[:200]}
        raise AssertionError(f"did not refuse; wanted {wanted!r}")

    def positive(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        cargo_home = directory / "cargo-home"
        seed = seed_cargo_home(host, cargo_home, _lock_text(rev, checksum).encode(), deadline_ns=deadline)
        assert seed["packages"] == 1, seed["packages"]
        assert seed["aggregate_bytes"] > 0
        assert (cargo_home / "registry" / "cache" / CRATES_NAMESPACE / "seedcrate-1.2.3.crate").read_bytes() == b"synthetic crate archive\n"
        config = (cargo_home / "git" / "db" / "seedrepo-0123456789abcdef" / "config").read_bytes()
        assert config == GIT_CONFIG, config
        assert b"include" not in config
        assert seed["git"]["candidates"][0]["commit_present"] is True
        assert seed["git"]["candidates"][0]["generated"] is True
        return f"seed {seed['aggregate_bytes']} bytes, {seed['entries']} entries"

    def wrong_commit(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        cargo_home = directory / "cargo-home"
        other = "0" * 40
        outcome = expect_refusal(
            lambda: seed_cargo_home(host, cargo_home, _lock_text(other, checksum).encode(), deadline_ns=deadline),
            "resolves the locked commit",
        )
        assert (cargo_home / "git" / "db" / "seedrepo-0123456789abcdef").is_dir(), "the rejected candidate was not copied and charged"
        return outcome["refusal"]

    def missing(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        shutil.rmtree(host / "git")
        return expect_refusal(
            lambda: seed_cargo_home(host, directory / "cargo-home", _lock_text(rev, checksum).encode(), deadline_ns=deadline),
            "missing",
        )["refusal"]

    def malformed(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        outcomes = []
        for text, wanted in (
            (_lock_text(rev, checksum).replace("checksum = ", "checksum = \"zz"), "not a basic string"),
            (_lock_text(rev, checksum).replace(checksum, "0" * 64), "checksum mismatch"),
            (_lock_text("abc", checksum), "no full 40-hex revision"),
            (_lock_text(rev, checksum).replace("registry+https://github.com/rust-lang/crates.io-index", "registry+https://example.invalid/index"), "unsupported registry source"),
        ):
            outcomes.append(expect_refusal(lambda t=text: seed_cargo_home(host, directory / "malformed", t.encode(), deadline_ns=deadline), wanted))
        return outcomes[0]["refusal"]

    def symlinked_input(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        archive = host / "registry" / "cache" / CRATES_NAMESPACE / "seedcrate-1.2.3.crate"
        elsewhere = directory / "elsewhere.crate"
        archive.rename(elsewhere)
        archive.symlink_to(elsewhere)
        refusal = expect_refusal(
            lambda: seed_cargo_home(host, directory / "cargo-home", _lock_text(rev, checksum).encode(), deadline_ns=deadline),
            "symlink",
        )["refusal"]
        # The same test for a Git database entry, with a clean registry tree.
        host2, rev2, checksum2 = _fixture_host(directory / "git-case")
        db = host2 / "git" / "db" / "seedrepo-0123456789abcdef"
        (db / "objects" / "link").symlink_to(elsewhere)
        expect_refusal(
            lambda: seed_cargo_home(host2, directory / "git-case" / "cargo-home", _lock_text(rev2, checksum2).encode(), deadline_ns=deadline),
            "symlink",
        )
        return refusal

    def caps(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        outcomes = []
        outcomes.append(
            expect_refusal(
                lambda: seed_cargo_home(
                    host,
                    directory / "bytes-capped",
                    _lock_text(rev, checksum).encode(),
                    limits=Limits(aggregate_bytes=4),
                    deadline_ns=deadline,
                ),
                "aggregate bound",
            )
        )
        blocked = directory / "bytes-capped" / "registry" / "cache" / CRATES_NAMESPACE / "seedcrate-1.2.3.crate"
        assert not blocked.exists(), "a write past the byte bound happened"
        second = host / "git" / "db" / "seedrepo-fedcba9876543210"
        shutil.copytree(host / "git" / "db" / "seedrepo-0123456789abcdef", second)
        outcomes.append(
            expect_refusal(
                lambda: seed_cargo_home(
                    host,
                    directory / "candidates-capped",
                    _lock_text(rev, checksum).encode(),
                    limits=Limits(git_candidates=1),
                    deadline_ns=deadline,
                ),
                "found 2",
            )
        )
        outcomes.append(
            expect_refusal(
                lambda: seed_cargo_home(
                    host,
                    directory / "entries-capped",
                    _lock_text(rev, checksum).encode(),
                    limits=Limits(entries=2),
                    deadline_ns=deadline,
                ),
                "enumerated entries",
            )
        )
        return outcomes[0]["refusal"]

    def second_database(host: Path, directory: Path, name: str, payload: bytes) -> str:
        """A second bare database whose own commit is not the locked one."""
        work = directory / f"work-{name}"
        work.mkdir(parents=True)
        (work / "PAYLOAD").write_bytes(payload)
        _git(work, "init", "-q", "-b", "main")
        _git(work, "add", "PAYLOAD")
        _git(work, "commit", "-q", "-m", "other")
        db = host / "git" / "db" / name
        db.parent.mkdir(parents=True, exist_ok=True)
        _git(work, "init", "-q", "--bare", str(db))
        _git(work, "push", "-q", str(db), "HEAD:refs/heads/main")
        _git(db, "symbolic-ref", "HEAD", "refs/heads/main")
        return _git(work, "rev-parse", "HEAD")

    def rejected_candidate(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        other = second_database(host, directory, "seedrepo-fedcba9876543210", b"other\n")
        assert other != rev
        cargo_home = directory / "cargo-home"
        seed = seed_cargo_home(host, cargo_home, _lock_text(rev, checksum).encode(), deadline_ns=deadline)
        copied = [c for c in seed["git"]["candidates"] if c["copied"]]
        rejected = [c for c in seed["git"]["candidates"] if c["rejected"]]
        assert len(copied) == 2 and len(rejected) == 1, "unexpected candidate outcomes"
        assert rejected[0]["bytes"] > 0 and rejected[0]["commit_present"] is False
        assert (cargo_home / "git" / "db" / rejected[0]["database"]).is_dir(), "the rejected copy was removed"
        assert seed["aggregate_bytes"] == (
            sum(c["bytes"] for c in copied) + sum(f["bytes"] for f in seed["files"])
        ), seed["aggregate_bytes"]
        return f"one of {len(copied)} copied candidates rejected, {rejected[0]['bytes']} bytes kept charged"

    def oversized_candidate(directory: Path) -> str:
        host, rev, checksum = _fixture_host(directory)
        second_database(host, directory, "seedrepo-fedcba9876543210", os.urandom(2 * MIB))
        seed = seed_cargo_home(
            host,
            directory / "cargo-home",
            _lock_text(rev, checksum).encode(),
            limits=Limits(aggregate_bytes=1 * MIB),
            deadline_ns=deadline,
        )
        skipped = [c for c in seed["git"]["candidates"] if c["skipped"]]
        assert len(skipped) == 1 and skipped[0]["copied"] is False, "unexpected candidate outcomes"
        assert seed["aggregate_bytes"] <= 1 * MIB
        return f"oversized candidate measured {skipped[0]['measured_bytes']} bytes and not attempted"

    case("registry-git-positive", positive)
    case("rejected-candidate", rejected_candidate)
    case("oversized-candidate", oversized_candidate)
    case("wrong-commit", wrong_commit)
    case("missing-candidate", missing)
    case("malformed-lock", malformed)
    case("symlinked-input", symlinked_input)
    case("cap-negatives", caps)
    return records


def main(argv: list) -> int:
    if len(argv) == 3 and argv[1] == "--self-test":
        root = argv[2]
        if not os.path.isabs(root):
            print("the self-test root must be absolute", file=sys.stderr)
            return 2
        records = self_test(root)
        for record in records:
            print(f"offline-seed case {record['case']}: passed {record['detail']}")
        print(f"offline-seed self-test: {len(records)} synthetic cases passed under {root}")
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
