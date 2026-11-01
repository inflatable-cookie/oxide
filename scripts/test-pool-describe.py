#!/usr/bin/env python3
"""Focused proof for `oxide pool describe --json` (oxide#112).

Status: gate open (`RUN_BOUND` is True). Each native run is a separate owner
activation. The run history is kept in the owning contract and on PR #104, and
its counts and outcomes are not reset here. With the gate closed the script
refuses with exit 2 before any effect. `--plan` prints this docstring and has
no effect. A refusal is not a proof.

Binding: the script runs only inside the qualified shared runner (oxide#109),
`scripts/bounded-test.py`, delivered at e86906fe. Main's current copy (merged
at 5ac62f9f) has SHA-256 d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e,
which is the runner this proof is pinned to. The
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
   Cargo.lock at HEAD by the shared bounded seeder
   (`scripts/offline_seed.py`, oxide#131). Only the supported crates.io sparse
   namespace is served, matched exactly on index and cache. Each registry
   package takes one index entry and one .crate archive, with archive bytes
   checked against the lock checksum. Workspace packages are skipped. The one
   locked Longhorn Git package seeds its bare database candidates (1 to 8, one
   shared aggregate) by copying every candidate first through
   descriptor-relative no-follow reads, skipping host config and hooks,
   generating a known private config and refusing alternates; only then is the
   full locked commit resolved inside the sanitized private copy, and no host
   Git runs at all. A rejected candidate stays copied and charged; the private
   checkout's HEAD is checked after staging. Other sources, duplicate keys and
   duplicate packages refuse. Every source path is anchored per component on
   the host root's device and checked before and after its bytes are read;
   symlinks, special entries, foreign devices and changed sources refuse.
   Copies stop before any write past the 256 MiB aggregate, with at most 8 Git
   candidates, 100,000 enumerated entries, depth 64, 1 MiB per manifest read
   and 45 s of seed work. seed.json records the SHA-256 of the exact lock
   bytes, the namespace, the aggregate bytes and each copied file's digest.
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
- Seed source mutation: reads are descriptor-relative and no-follow on every
  component, and each copied file is checked before and after the read and by
  a reopened name, so a changed source refuses. A concurrent writer replacing
  a name between two different files' reads is still not one transaction: the
  seed is exactly the files that passed their own checks, never a claim about
  the host cache as a whole.
- Staging reads the host HOME, RUSTUP_HOME, toolchain and registry seed. That
  is accepted as offline fixture input. Importing the shared seeder transfers
  no qualification: this proof's receipt binds the seeder's own hash.
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

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import offline_seed  # scripts/offline_seed.py, oxide#131; its hash is in the receipt

# The gate. Opened by the owner after the apparatus PASS (53ec48e4) for the one
# authorised native run. Setting it back to False is an owner action, not a fix.
RUN_BOUND = True
CLOSED = (
    "CLOSED: RUN_BOUND is False. rust:pool-describe did not run; nothing was "
    "staged, built or executed."
)
OWNED_LIMIT_BYTES = 512 * 1024 * 1024
# The seed bounds, constants and refusals live in scripts/offline_seed.py.
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
            env["OXIDE_TEST_HOME_FROM_ENV"] = "1"
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


def seed_private_cargo_home(proof: "Proof", host_cargo_home: Path, cargo_home: Path, lock_bytes: bytes) -> dict:
    """Seed the private offline Cargo home with the shared bounded seeder.

    `scripts/offline_seed.py` (oxide#131) is the one seeder: anchored
    descriptor-relative reads, a copy-first Git candidate protocol with a
    generated private config and no host Git, the lock's checksums, and the
    256 MiB / 8 candidate / 100,000 entry / depth 64 / 45 s bounds. Its hash
    is in this proof's receipt; importing it transfers no qualification.
    """
    try:
        return offline_seed.seed_cargo_home(
            host_cargo_home, cargo_home, lock_bytes, deadline_ns=proof.deadline_ns
        )
    except offline_seed.SeedRefusal as error:
        raise SeedRefusal(str(error))


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
    host_cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    cargo_home = proof.root / "cargo-home"
    cargo_home.mkdir()
    seed = seed_private_cargo_home(proof, host_cargo_home, cargo_home, lock_bytes)
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
    for git_source in seed["git_sources"]:
        chosen = offline_seed.verify_private_checkout(
            cargo_home,
            git_source["rev"],
            [candidate["database"] for candidate in git_source["candidates"]],
            deadline_ns=proof.deadline_ns,
        )
        git_source["chosen_ident"] = chosen
        (proof.evidence / "seed.json").write_text(json.dumps(seed, indent=2))
    artifact = (proof.root / "target" / "debug" / "oxide").resolve()
    if not artifact.is_file() or proof.root not in artifact.parents:
        raise RuntimeError("the staged artifact is not inside the owned root")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    provenance = {
        "commit": commit,
        "tree": tree,
        "toolchain": channel,
        "artifact_sha256": digest,
        "lock_sha256": seed["lock_sha256"],
        "seed_aggregate_bytes": seed["aggregate_bytes"],
        "seed_entries": seed["entries"],
        "helper_sha256": {
            "scripts/offline_seed.py": hashlib.sha256(
                (REPO / "scripts" / "offline_seed.py").read_bytes()
            ).hexdigest()
        },
    }
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
    assert removable["mode"] == "ephemeral", removable
    assert removable["mount_point"] is None and removable["volume_uuid"] is None
    assert removable["unavailable_reason"] == "metadata_unknown", removable
    assert removable["available"] is False and removable["payload"] is None
    assert "reserve_bytes" in removable and removable["reserve_bytes"] is None, removable
    assert snapshot(case) == before, "describe changed the fixture"


def case_absent_volume(proof: Proof, artifact: Path) -> None:
    """Absent legacy and v2 drives retain their registered mode without writes."""
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
    for label, version, mode in [
        ("legacy", 1, "ephemeral"),
        ("ephemeral", 2, "ephemeral"),
        ("permanent", 2, "permanent"),
    ]:
        variant = dict(registration, version=version)
        if version == 2:
            variant["mode"] = mode
        case = proof.root / f"case-c-{label}"
        registration_fixture(case, json.dumps(variant))
        before = snapshot(case)
        text = describe(proof, artifact, f"case-c-{label}", case)
        removable = text["removable"]
        assert removable["mode"] == mode, removable
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
