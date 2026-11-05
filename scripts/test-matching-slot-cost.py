#!/usr/bin/env python3
"""#280 private cost investigation, HIGH; 300 s / 512 MiB qualified guard.

Premises, traced through main/fixture and its sole Oxide child:
1. Compilation uses ordinary managed storage and the invoking Cargo/rustup homes,
   offline/locked 1.97.1, auto-install disabled. No fixture Cargo crate/lock exists:
   the ignored library test calls inventory directly, spawning no children.
2. No fixture account API or private HOME; account-home override is inapplicable.
3. Child scrubs OXIDE_*/XDG_* except the guard's three protocol variables, then
   adds only the two test-only controls. No inherited pool/config/test control.
4. Ordinary Oxide compilation may auto-adopt this checkout. This is not an
   unconverted-checkout oracle; fixture inventory calls no adoption code.
5. No external Cargo subcommand or fake tool. 6. No image or mount operations.
7. Both flag encodings frozen empty for the ordinary managed compilation.
8. Fixture, test temp and observation writes are under the qualified selected-
   volume scratch. Compiler output remains ordinary Oxide-managed storage.
9. Python mkdtemp uses explicit dir, no macOS mktemp default or fixed /tmp.

No payload is read, pool walked, or production code changed. Fresh-created
60,000-file Cargo shapes differ from the unavailable historical tree. A warm
second walk is a result cross-check, not a cold-device benchmark. Source
selection counts do not count syscalls. No universal throughput acceptance.
--diagnostic keeps going across independent oracle failures in the test, always
returns 1 and qualified:false. Infrastructure failures abort and retain their
actual exit/log, without a replacement run. The shared guard alone supervises
processes, deadlines, reserve and cleanup. This script never deletes fixtures.
"""
import hashlib
import json
import os
import stat
from pathlib import Path
import subprocess
import sys
import tempfile
import time

RUNNER_HASH = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
COMMAND = ["oxide", "test", "--lib", "--locked", "--offline",
           "inventory::tests::matching_slot_cost_fixture", "--",
           "--ignored", "--exact", "--nocapture", "--test-threads=1"]


def main():
    if sys.argv[1:] not in ([], ["--diagnostic"], ["--self-check"]):
        raise SystemExit("usage: test-matching-slot-cost.py [--diagnostic | --self-check]")
    # Exercise the exact no-follow API before creating any fixture or child.
    # Path.stat(follow_symlinks=...) is unavailable on the host's Python 3.9.6.
    own_metadata = Path(__file__).lstat()
    if sys.version_info < (3, 8) or not stat.S_ISREG(own_metadata.st_mode):
        raise SystemExit("requires Python >=3.8 and an ordinary driver file")
    if not isinstance(own_metadata.st_blocks, int):
        raise SystemExit("allocated-block metadata unavailable")
    if sys.argv[1:] == ["--self-check"]:
        print(json.dumps(dict(python=sys.version, metadata_api="Path.lstat", passed=True)))
        return 0
    diagnostic = bool(sys.argv[1:])
    repo = Path(__file__).resolve().parent.parent
    if hashlib.sha256((repo / "scripts/bounded-test.py").read_bytes()).hexdigest() != RUNNER_HASH:
        raise SystemExit("qualified guard mismatch")
    scratch = Path(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    evidence = Path(os.environ["OXIDE_PROOF_EVIDENCE"])
    if not scratch.is_dir() or scratch.resolve() != scratch or not evidence.is_dir():
        raise SystemExit("missing canonical supervisor scratch/evidence")
    if not str(scratch).startswith("/Volumes/Stash/scratch/oxide/"):
        raise SystemExit("not selected Stash scratch")
    os.environ["OXIDE_PROOF_DEADLINE_NS"]  # refuse unsupervised invocation
    base = Path(tempfile.mkdtemp(prefix="matching-cost-", dir=scratch))
    tree = base / "tree"
    tree.mkdir()
    started = time.monotonic()
    counts = dict(entries=0, files=0, logical=0, allocated=0, directories=0)

    def directory(relative):
        path = tree / relative
        # Only known fixture parents, never paths from a real target.
        if path != tree and not path.exists():
            directory(str(Path(relative).parent))
            path.mkdir()
            counts["directories"] += 1
            counts["entries"] += 1
        return path

    def files(relative, number):
        parent = directory(relative)
        for index in range(number):
            leaf = parent / f"oxide_fixture_unit_{index:04x}-0123456789abcdef.o"
            with leaf.open("xb") as output:
                output.write(b"x")
            metadata = leaf.lstat()
            counts["allocated"] += metadata.st_blocks * 512
            counts["files"] += 1
            counts["entries"] += 1
            counts["logical"] += 1

    files("debug/deps", 16000)
    for unit in range(160):
        files(f"debug/incremental/fixture_{unit:04x}-0123456789abcdef/s-session/working", 160)
        files(f"debug/.fingerprint/fixture_{unit:04x}-0123456789abcdef", 40)
    for unit in range(80):
        files(f"debug/build/fixture_{unit:04x}-0123456789abcdef/out", 150)
    # One long-name/deeper path and a genuine cross-category hardlink oracle.
    files("debug/" + "/".join(["representative_name_0123456789"] * 12) + "/deps", 1)
    original = tree / "debug/deps/oxide_fixture_unit_0000-0123456789abcdef.o"
    os.link(original, tree / "debug/.fingerprint/shared-hardlink")
    counts["entries"] += 1
    counts["files"] += 1
    counts["logical"] += 1
    if counts["entries"] > 80000:
        raise SystemExit("fixture exceeds entry limit")
    (base / "expected.json").write_text(json.dumps(counts))
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("OXIDE_", "XDG_")) or key in
           ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS")}
    env.update(TMPDIR=str(base), TMP=str(base), TEMP=str(base),
               RUSTUP_TOOLCHAIN="1.97.1", RUSTUP_AUTO_INSTALL="0",
               CARGO_NET_OFFLINE="true", RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS="",
               OXIDE_TEST_MATCHING_COST_ROOT=str(base))
    if diagnostic:
        env["OXIDE_TEST_MATCHING_COST_DIAGNOSTIC"] = "1"
    # Do not capture unbounded data in memory, reset a clock, or supervise a
    # process tree here. The unchanged qualified supervisor owns those rules.
    log = evidence / "matching-cost-child.log"
    with log.open("xb") as output:
        child = subprocess.run(COMMAND, cwd=repo, env=env, stdout=output, stderr=subprocess.STDOUT)
    observations = base / "observations.json"
    result = json.loads(observations.read_text()) if observations.exists() else None
    receipt = dict(qualified=False, diagnostic=diagnostic, command=COMMAND,
                   child_exit=child.returncode, fixture=str(base), counts=counts,
                   wall_seconds=time.monotonic() - started,
                   observations=result,
                   source_sha256={str(path): hashlib.sha256((repo / path).read_bytes()).hexdigest()
                                  for path in ["src/inventory.rs", "scripts/test-matching-slot-cost.py", "scripts/bounded-test.py"]})
    (evidence / "matching-cost.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt), flush=True)
    return 1 if diagnostic else (0 if child.returncode == 0 and result and result["passed"] else 1)


if __name__ == "__main__":
    raise SystemExit(main())
