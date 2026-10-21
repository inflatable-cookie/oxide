#!/usr/bin/env python3
"""Focused proof for `oxide pool describe --json` (oxide#112). HELD.

Status: PROPOSED. This script refuses before any effect. It stages nothing,
builds nothing, creates no fixture and touches no volume until the owner
binds either a qualified #109 shared runner or Tom's task-specific exception
for oxide#112. Existence of this file is not permission to run it.

`--plan` prints the cases, bounds and coverage gaps below. It has no effect.

Proposed run, once bound (300 s wall clock, 512 MiB owned scratch, 20 GiB host
reserve, heavy admission, canonical /private/tmp scratch):
1. Stage the candidate `oxide` offline from this checkout's HEAD into a fresh
   owned target directory, debug profile, `cargo build --offline --locked
   --bin oxide`, and log the commit, tree and binary hash. Cargo's home is
   the host's read-only cache; no network, no install, no global change.
2. Every child runs with a private HOME, OXIDE_POOL and TMPDIR under the
   owned scratch, and a minimal PATH.
3. Cases, each checking exit code, JSON schema, escaping and nonmutation of
   the internal pool tree before and after:
   a. no registration: `removable` is null, `selected` is "internal", and
      the pool path is not created;
   b. unknown registration (invalid `removable.json`): `removable` keeps null
      `mount_point`, `volume_uuid`, `payload` and `suggested_scratch_root`,
      reason `metadata_unknown`, escaped `unavailable_detail`;
   c. absent volume (valid registration to a random unmounted name under
      /Volumes): reason `absent`, `available` false, `selected` "internal",
      `payload` is `<mount>/oxide-pool`, and the scratch root
      `<mount>/scratch` lies outside `payload`; /Volumes is only read, and
      the name is not created.

Coverage gaps, reported as gaps and never as passes:
- `available: true`, every volume-dependent refusal (identity mismatch,
  volume_unusable, below_reserve, payload_missing) and the pending
  reconciliation and missing-control cases need a mounted APFS volume at a
  /Volumes path. This proof does not mount one. Those cases are covered only
  by library and unit tests, which need no volume.
- Live `diskutil` and `plutil` timeouts and output caps are covered by unit
  tests with real subprocesses (`head`, `echo`), not by this script.
- The 20 GiB host reserve and the 512 MiB owned-bytes cap are sampled at
  phase boundaries only. The guard that would watch them continuously
  (`scripts/bounded_runner.py`) is not wired here and is not qualified.
- The wall-clock bound is a deadline checked before each child. It is not
  an enforced kill of a running child.
- Exit-nonzero behaviour for an Oxide failure is not exercised.
- Old-guard option, if the owner grants an exception: `scripts/bounded-test.py`
  at SHA 9dffe4f3cb8cbb7de33fc50798986cd7a9860cb55b3aee66942b1a0ea9c49344. Its
  known gaps (no continuous reserve or scratch monitoring, no enforced
  scratch budget or cleanup, process-group signalling without positive
  identity at timeout, no absolute deadline) would apply, and must be
  disclosed with the exception.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import json
import os
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

HOLD = (
    "HELD: rust:pool-describe is a proposal for oxide#112. It runs only after the "
    "owner binds a qualified #109 shared runner or Tom's task-specific exception. "
    "Nothing was staged, built or run."
)
PROOF_SECONDS = 300
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
PLAN = __doc__


def plan() -> None:
    print(PLAN)


def tree(root: Path) -> list[tuple[str, bytes]]:
    files = []
    for path in sorted(root.rglob("*")):
        if path.is_file() and not path.is_symlink():
            files.append((str(path.relative_to(root)), path.read_bytes()))
    return files


def run_describe(binary: Path, env: dict, deadline: float) -> dict:
    if time.monotonic() > deadline:
        raise SystemExit("wall-clock bound reached before a child ran")
    done = subprocess.run(
        [str(binary), "pool", "describe", "--json"],
        env=env,
        capture_output=True,
        timeout=max(1.0, deadline - time.monotonic()),
        check=False,
    )
    if done.returncode != 0:
        raise SystemExit(f"describe exited {done.returncode}: {done.stderr!r}")
    description = json.loads(done.stdout)
    assert description["schema"] == "oxide.pool.describe/1", description
    assert description["observed_at"].endswith("Z"), description
    assert description["removable"] is None or description["removable"]["mode"] == "ephemeral"
    return description


def case_no_registration(binary: Path, scratch: Path, deadline: float) -> None:
    pool = scratch / "pool"
    env = {"HOME": str(scratch / "home"), "OXIDE_POOL": str(pool), "PATH": "/usr/bin:/bin"}
    before = tree(scratch)
    description = run_describe(binary, env, deadline)
    assert description["removable"] is None and description["selected"] == "internal"
    assert not pool.exists(), "describe created the pool"
    assert tree(scratch) == before, "describe changed the fixture"


def case_unknown_registration(binary: Path, scratch: Path, deadline: float) -> None:
    pool = scratch / "pool2"
    control = pool / "oxide-managed"
    control.mkdir(parents=True)
    os.chmod(pool, 0o700)
    os.chmod(control, 0o700)
    # Not JSON: the detail must be escaped on output, not echoed raw.
    (control / "removable.json").write_text('{"bad": "quote\\"\n')
    os.chmod(control / "removable.json", 0o600)
    env = {"HOME": str(scratch / "home"), "OXIDE_POOL": str(pool), "PATH": "/usr/bin:/bin"}
    before = tree(scratch)
    removable = run_describe(binary, env, deadline)["removable"]
    assert removable["mount_point"] is None and removable["volume_uuid"] is None
    assert removable["unavailable_reason"] == "metadata_unknown"
    assert removable["available"] is False and removable["payload"] is None
    assert tree(scratch) == before, "describe changed the fixture"


def case_absent_volume(binary: Path, scratch: Path, deadline: float) -> None:
    pool = scratch / "pool3"
    control = pool / "oxide-managed"
    control.mkdir(parents=True)
    os.chmod(pool, 0o700)
    os.chmod(control, 0o700)
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
    path = control / "removable.json"
    path.write_text(json.dumps(registration))
    os.chmod(path, 0o600)
    env = {"HOME": str(scratch / "home"), "OXIDE_POOL": str(pool), "PATH": "/usr/bin:/bin"}
    before = tree(scratch)
    description = run_describe(binary, env, deadline)
    removable = description["removable"]
    assert removable["unavailable_reason"] == "absent", removable
    assert removable["available"] is False and description["selected"] == "internal"
    assert removable["payload"] == f"{mount}/oxide-pool"
    assert removable["suggested_scratch_root"] == f"{mount}/scratch"
    assert not os.path.exists(mount), "describe created a /Volumes path"
    assert tree(scratch) == before, "describe changed the fixture"


CASES = [case_no_registration, case_unknown_registration, case_absent_volume]


def main() -> int:
    if "--plan" in sys.argv[1:]:
        plan()
        return 0
    # The gate. Nothing above this line has an effect. Staging, fixtures and
    # the cases below run only after the owner replaces HOLD with a bound
    # runner or exception and sets RUN_BOUND to True.
    RUN_BOUND = False
    if not RUN_BOUND:
        print(HOLD, file=sys.stderr)
        return 2
    # Unreachable while held. Fixtures live only under a fresh private dir.
    scratch_root = Path(tempfile.mkdtemp(prefix="oxide-pool-describe.", dir="/private/tmp"))
    deadline = time.monotonic() + PROOF_SECONDS
    binary = scratch_root / "target" / "debug" / "oxide"  # staging not wired here
    for case in CASES:
        case_root = Path(tempfile.mkdtemp(prefix=f"{case.__name__}.", dir=scratch_root))
        case(binary, case_root, deadline)
        print(f"ok {case.__name__}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
