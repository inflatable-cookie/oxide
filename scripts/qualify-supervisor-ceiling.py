#!/usr/bin/env python3
"""Run the ceiling supervisor's ordinary full suite in fresh Stash scratch."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

SCRIPTS = Path(__file__).resolve().parent
MIB = 1024 * 1024


def command(root, pins, ceiling):
    return [sys.executable, str(SCRIPTS / "bounded-test-ceiling.py"),
            "--limit", "560", "--scratch-budget-mib", "384", "--root-parent", str(root),
            "--ceiling-uptime-ns", str(ceiling), "--", sys.executable,
            str(SCRIPTS / "test_supervisor_ceiling_qualification.py"), "--pins-sha256", pins]


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if len(argv) != 2 or argv[0] != "--pins-sha256":
        raise SystemExit("expected --pins-sha256 DIGEST")
    pins = argv[1]
    manifest = (SCRIPTS / "ceiling-qualification-pins.json").read_bytes()
    if hashlib.sha256(manifest).hexdigest() != pins:
        raise SystemExit("source pin mismatch")
    scratch = Path(os.environ["TMPDIR"]).resolve()
    if not str(scratch).startswith("/Volumes/Stash/") or not scratch.is_dir():
        raise SystemExit("fresh Stash scratch is required")
    if shutil.disk_usage(scratch).free < 22 * 1024 ** 3:
        raise SystemExit("less than 22 GiB free on scratch volume")
    root = Path(tempfile.mkdtemp(prefix="ceiling-proof.", dir=scratch))
    outcome = root / "cases.json"
    env = dict(os.environ)
    owned = root / "owned"
    owned.mkdir(mode=0o700)
    env["OXIDE_TASK_SCRATCH_ROOT"] = str(owned)
    env["OXIDE_SUPERVISOR_CASES"] = str(outcome)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    start = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    print("ceiling proof scratch:", root, flush=True)
    result = subprocess.run(command(root, pins, start + 560 * 10**9), env=env)
    print("candidate exit:", result.returncode, flush=True)
    if result.returncode or not outcome.is_file():
        return result.returncode or 1
    raw = outcome.read_bytes()
    if len(raw) > MIB:
        raise SystemExit("case outcome file exceeds 1 MiB")
    document = json.loads(raw)
    outcomes = document.get("outcomes")
    expected = document.get("pure_count", -1) + document.get("real_count", -1)
    if not isinstance(outcomes, list) or len(outcomes) != expected:
        raise SystemExit("case outcomes incomplete")
    print("case outcomes:", json.dumps(document, separators=(",", ":")), flush=True)
    return 0 if all(row[1] == "pass" for row in outcomes) else 1


if __name__ == "__main__":
    sys.exit(main())
