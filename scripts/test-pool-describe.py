#!/usr/bin/env python3
"""Focused proof for `oxide pool describe --json` (oxide#112). HELD.

Status: PROPOSED and NOT RUN. The default gate below is `RUN_BOUND = False`.
With the gate closed the script refuses, exit 2, before any effect: no
staging, no fixture, no child process. Its `--plan` flag prints this
docstring and has no effect. Existence of this file is not permission to run
it, and a refusal is not a proof.

Activation requires BOTH an owner-bound qualified #109 shared runner, or
Tom's explicit task-specific exception for oxide#112, AND changing RUN_BOUND.
Neither has happened. No old-guard exception is granted here.

Proposed invocation after binding, as an outer old-guard wrapper (exact
unchanged `scripts/bounded-test.py`, SHA-256
9dffe4f3cb8cbb7de33fc50798986cd7a9860cb55b3aee66942b1a0ea9c49344):

    python3 scripts/bounded-test.py python3 scripts/test-pool-describe.py

Proposed run (300 s wall clock, 512 MiB owned bytes, 20 GiB host reserve,
heavy admission):

1. Candidate staging. One offline, locked `cargo build --bin oxide` from this
   checkout's HEAD, debug profile, into an owned `target/` and an owned
   `build/` directory (CARGO_TARGET_DIR and CARGO_BUILD_BUILD_DIR). CARGO_HOME
   is owned too, seeded by copying the host registry cache (read only). The
   toolchain is pinned to rust-toolchain.toml's channel through RUSTUP_TOOLCHAIN.
   Inherited CARGO_*, RUSTC*, RUSTFLAGS, RUSTDOC, SCCACHE_* and wrapper
   routing are scrubbed. The commit, tree and artifact SHA-256 are logged, and
   the artifact must resolve inside the owned root.
2. Every child, staging included, runs with four canonical temp variables
   (OXIDE_TASK_SCRATCH_ROOT, TMPDIR, TMP, TEMP) all equal to the owned root,
   plus a private HOME, a private OXIDE_POOL and a minimal PATH.
3. Phase-boundary checks, not continuous: host free space above 20 GiB, and
   owned bytes at or below 512 MiB (lstat walk, st_blocks), before staging,
   after staging, after each case and before cleanup. Outputs go to owned
   files, not pipes, and are counted.
4. Cases, each with exit code, schema, escaping and before/after nonmutation.
   The snapshot covers every file's bytes, every directory's presence, every
   symlink target, and each entry's mode, mtime and ctime, so an empty
   control-directory creation is caught.
   a. no registration: `removable` is null, `selected` is "internal", and the
      pool directory is not created;
   b. unknown registration (invalid `removable.json`): null identity fields,
      `unavailable_reason` "metadata_unknown", escaped `unavailable_detail`;
   c. absent volume (valid registration to a random unmounted /Volumes name):
      reason "absent", `available` false, payload `<mount>/oxide-pool`, scratch
      `<mount>/scratch` outside payload, and no /Volumes path created;
   d. CLI refusal: `oxide pool describe` without `--json` exits nonzero with
      the usage text on stderr and changes nothing.
5. Cleanup. The owned root is removed only after every case passes and every
   child has been waited for or killed. On any failure it is retained and its
   path is printed.

Timeouts: each child gets the remaining wall-clock budget. On expiry the
child's own process group (started by this script) is killed, and the run
fails. This is not a closure proof for Cargo's own descendants.

Explicit coverage gaps. These are not passes:
- `available: true`, and every volume-dependent refusal (identity mismatch,
  volume_unusable, below_reserve, payload_missing), needs a mounted APFS volume.
  This proof mounts none, so these are uncovered here. Library and unit tests
  cover them without a volume.
- Pending reconciliation and missing control through the CLI need the same
  mount. Library tests cover them.
- Live diskutil and plutil timeouts and caps are unit-tested with real
  subprocesses, not through this script.
- The host reserve and owned bytes are checked at phase boundaries only. The
  guard that would watch them continuously (`scripts/bounded_runner.py`) is not
  wired and is not qualified. The old guard is periodic and has no absolute
  deadline guarantee.
- Cargo's descendant processes are not proven closed when the leader exits.
- Staging seeds CARGO_HOME from the host registry, a read of the global cache.
  The owner decides whether that read is acceptable.
- Exit-nonzero for an Oxide internal failure (not usage) is not exercised.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

# The gate. Keep this False until the owner binds a qualified #109 runner or
# Tom's oxide#112 exception. Changing it is an owner action, not a fix.
RUN_BOUND = False
HOLD = (
    "HELD: rust:pool-describe is a proposal for oxide#112 and has not been run. "
    "It runs only after the owner binds a qualified #109 shared runner or Tom's "
    "task-specific exception, and RUN_BOUND is set to True. Nothing was staged, "
    "built or run."
)
PROOF_SECONDS = 300
OWNED_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
REPO = Path(__file__).resolve().parent.parent
TEMP_VARS = ("OXIDE_TASK_SCRATCH_ROOT", "TMPDIR", "TMP", "TEMP")
SCRUB_PREFIXES = ("CARGO", "RUSTC", "RUSTFLAGS", "RUSTDOC", "SCCACHE", "RUSTUP_TOOLCHAIN")


class Proof:
    """Shared state: the owned root, the deadline and the phase log."""

    def __init__(self) -> None:
        self.started = time.monotonic()
        self.deadline = self.started + PROOF_SECONDS
        self.root = Path(tempfile.mkdtemp(prefix="oxide-pool-describe.", dir="/private/tmp")).resolve()
        self.phases: list[str] = []
        self.clean_closure = True

    def remaining(self) -> float:
        left = self.deadline - time.monotonic()
        if left <= 0:
            raise RuntimeError("300 s wall-clock bound reached")
        return left

    def env(self, case: Path | None = None, extra: dict | None = None) -> dict:
        """Scrubbed host env plus the four temp vars and a private HOME and pool.

        A case gets its own directory under the owned root. Staging uses none.
        """
        env = {key: value for key, value in os.environ.items() if not key.startswith(SCRUB_PREFIXES)}
        for key in TEMP_VARS:
            env[key] = str(self.root)
        env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
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
        """Run one child in its own session, outputs to owned files."""
        out_path = self.root / f"{label}.stdout"
        err_path = self.root / f"{label}.stderr"
        with out_path.open("wb") as out, err_path.open("wb") as err:
            proc = subprocess.Popen(argv, env=env, cwd=cwd, stdout=out, stderr=err, start_new_session=True)
            try:
                code = proc.wait(timeout=self.remaining())
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)  # our own session, started here
                proc.wait()
                self.clean_closure = False
                raise RuntimeError(f"{label}: killed at the wall-clock bound")
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


def snapshot(root: Path) -> list[tuple]:
    """Every entry's type, mode, times and contents, so no change is hidden."""
    entries = []
    for dirpath, dirnames, filenames in os.walk(root):
        for name in sorted(dirnames + filenames):
            path = Path(dirpath, name)
            stat = os.lstat(path)
            rel = str(path.relative_to(root))
            base = (rel, stat.st_mode, stat.st_mtime_ns, stat.st_ctime_ns)
            if path.is_symlink():
                entries.append(base + ("link", os.readlink(path)))
            elif path.is_dir():
                entries.append(base + ("dir",))
            else:
                entries.append(base + ("file", hashlib.sha256(path.read_bytes()).hexdigest()))
    return sorted(entries)


def stage(proof: Proof) -> Path:
    """One offline, locked build of the candidate from this HEAD."""
    commit = subprocess.run(["git", "-C", str(REPO), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    tree = subprocess.run(["git", "-C", str(REPO), "rev-parse", "HEAD^{tree}"], capture_output=True, text=True, check=True).stdout.strip()
    channel = re.search(r'channel\s*=\s*"([^"]+)"', (REPO / "rust-toolchain.toml").read_text()).group(1)
    host_cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    cargo_home = proof.root / "cargo-home"
    registry = host_cargo_home / "registry"
    if registry.is_dir():
        if owned_bytes(registry) > 256 * 1024 * 1024:
            raise RuntimeError("host registry cache exceeds the 256 MiB seed bound")
        shutil.copytree(registry, cargo_home / "registry", symlinks=True)
    target = proof.root / "target"
    build = proof.root / "build"
    env = proof.env(
        extra={
            "CARGO_HOME": str(cargo_home),
            "CARGO_TARGET_DIR": str(target),
            "CARGO_BUILD_BUILD_DIR": str(build),
            "RUSTUP_TOOLCHAIN": channel,
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
        }
    )
    # Cargo's own invocation, resolved from the host PATH before the scrub.
    cargo = shutil.which("cargo")
    if cargo is None:
        raise RuntimeError("cargo is not on the host PATH")
    code, _, err = proof.run(
        [cargo, "build", "--offline", "--locked", "--bin", "oxide"],
        env,
        "stage",
        cwd=REPO,
    )
    if code != 0:
        raise RuntimeError(f"staging failed: {err[-400:]}")
    artifact = (target / "debug" / "oxide").resolve()
    if not artifact.is_file() or proof.root not in artifact.parents:
        raise RuntimeError("the staged artifact is not inside the owned root")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    proof.phases.append(f"stage: commit={commit} tree={tree} toolchain={channel} sha256={digest}")
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
    pool = registration_fixture(case, '{"bad": "quote\\"\n')
    before = snapshot(case)
    text = describe(proof, artifact, "case-b", case)
    removable = text["removable"]
    assert removable["mount_point"] is None and removable["volume_uuid"] is None
    assert removable["unavailable_reason"] == "metadata_unknown", removable
    assert removable["available"] is False and removable["payload"] is None
    assert snapshot(case) == before, "describe changed the fixture"
    assert pool.is_dir()


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
    proof = Proof()
    cleanup_ok = False
    try:
        proof.check_phase("before-staging")
        artifact = stage(proof)
        proof.check_phase("after-staging")
        for case in CASES:
            case(proof, artifact)
            proof.check_phase(case.__name__)
            print(f"ok {case.__name__}")
        cleanup_ok = proof.clean_closure
    finally:
        print("phases:", *proof.phases, sep="\n  ")
        if cleanup_ok:
            proof.check_phase("before-cleanup")
            shutil.rmtree(proof.root)
            print("owned root removed")
        else:
            print(f"owned root retained for inspection: {proof.root}")
    return 0 if cleanup_ok else 1


def main() -> int:
    if "--plan" in sys.argv[1:]:
        print(__doc__)
        return 0
    if not RUN_BOUND:
        print(HOLD, file=sys.stderr)
        return 2
    return run_proof()


if __name__ == "__main__":
    raise SystemExit(main())
