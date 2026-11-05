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
8. Fixture and observations stay in a fresh 0700 child of current evidence.
   Guard sample_owned uses tree_bytes(ctx[root]), including evidence; cleanup
   rmtree targets only ctx[scratch]. Scratch/evidence are canonical siblings.
   Child TMPDIR stays in fresh scratch; compiler output is ordinary managed.
9. Python mkdtemp uses explicit dir, no macOS mktemp default or fixed /tmp.

No payload is read, pool walked, or production code changed. Fresh-created
smaller-fixture Cargo shapes (7,172 file names) differ from the unavailable
historical tree. A warm
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
        print(json.dumps(dict(python=sys.version, metadata_api="Path.lstat",
                              scale="smaller-fixture", file_names=7172, entries=7350,
                              max_depth=14, passed=True)))
        return 0
    diagnostic = bool(sys.argv[1:])
    repo = Path(__file__).resolve().parent.parent
    if hashlib.sha256((repo / "scripts/bounded-test.py").read_bytes()).hexdigest() != RUNNER_HASH:
        raise SystemExit("qualified guard mismatch")
    scratch = Path(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    evidence = Path(os.environ["OXIDE_PROOF_EVIDENCE"])
    if (not scratch.is_dir() or scratch.resolve() != scratch or
            not evidence.is_dir() or evidence.resolve() != evidence or
            evidence.parent != scratch.parent or evidence == scratch):
        raise SystemExit("missing canonical supervisor scratch/evidence")
    if not str(scratch).startswith("/Volumes/Stash/scratch/oxide/"):
        raise SystemExit("not selected Stash scratch")
    os.environ["OXIDE_PROOF_DEADLINE_NS"]  # refuse unsupervised invocation
    base = Path(tempfile.mkdtemp(prefix="matching-cost-", dir=evidence))
    child_temp = Path(tempfile.mkdtemp(prefix="matching-temp-", dir=scratch))
    if stat.S_IMODE(base.lstat().st_mode) != 0o700 or base.lstat().st_uid != os.getuid():
        raise SystemExit("fixture must be private and owned")
    tree = base / "tree"
    tree.mkdir()
    started = time.monotonic()
    counts = dict(entries=0, files=0, logical=0, allocated=0, directories=0)
    nodes = {}

    def signature(path):
        meta = path.lstat()
        identity = [meta.st_dev, meta.st_ino, meta.st_mode]
        if stat.S_ISDIR(meta.st_mode):
            return identity
        return identity + [meta.st_size, meta.st_blocks,
                           meta.st_mtime_ns // 10**9, meta.st_mtime_ns % 10**9,
                           meta.st_ctime_ns // 10**9, meta.st_ctime_ns % 10**9]

    root_identity = signature(tree)

    def directory(relative):
        path = tree / relative
        # Only known fixture parents, never paths from a real target.
        if path != tree and not path.exists():
            directory(str(Path(relative).parent))
            path.mkdir()
            counts["directories"] += 1
            counts["entries"] += 1
            nodes[str(path.relative_to(tree))] = signature(path)
        return path

    def record_file(leaf):
        metadata = leaf.lstat()
        key = (metadata.st_dev, metadata.st_ino)
        if key not in inodes:
            counts["allocated"] += metadata.st_blocks * 512
            inodes.add(key)
        counts["files"] += 1
        counts["entries"] += 1
        counts["logical"] += metadata.st_size
        nodes[str(leaf.relative_to(tree))] = signature(leaf)

    inodes = set()

    def files(relative, number):
        parent = directory(relative)
        for index in range(number):
            leaf = parent / f"oxide_fixture_unit_{index:04x}-0123456789abcdef.o"
            with leaf.open("xb") as output:
                output.write(b"x")
            record_file(leaf)

    # Create the two exclusive metadata controls before bulk population, not
    # after a long creation window. An existing name still refuses; this does
    # not fence unknown writers or make a conflicting file ours.
    for relative in [".DS_Store", "debug/.DS_Store"]:
        directory(str(Path(relative).parent))
        leaf = tree / relative
        with leaf.open("xb") as output:
            output.write(b"oxide-280-deliberately-created-metadata\n")
        record_file(leaf)

    files("debug/deps", 2048)
    for unit in range(32):
        files(f"debug/incremental/fixture_{unit:04x}-0123456789abcdef/s-session/working", 96)
        files(f"debug/.fingerprint/fixture_{unit:04x}-0123456789abcdef", 16)
    for unit in range(16):
        files(f"debug/build/fixture_{unit:04x}-0123456789abcdef/out", 96)
    # One long-name/deeper path and a genuine cross-category hardlink oracle.
    files("debug/" + "/".join(["representative_name_0123456789"] * 12) + "/deps", 1)
    original = tree / "debug/deps/oxide_fixture_unit_0000-0123456789abcdef.o"
    alias = tree / "debug/.fingerprint/shared-hardlink"
    original_identity = signature(original)[:3]
    os.link(original, alias)
    if signature(original)[:3] != original_identity:
        raise SystemExit("hardlink creation changed original identity")
    # Deliberate link creation changes ctime; record that known operation,
    # never enumerate additions and adopt them into the expected manifest.
    nodes[str(original.relative_to(tree))] = signature(original)
    record_file(alias)
    if counts["entries"] != 7350 or counts["files"] != 7172 or counts["files"] > 8192:
        raise SystemExit("fixture exceeds fixed layout")
    # Independent tiny negative control: freeze intended set BEFORE adding
    # the deliberately unplanned name. No oracle counts are adjusted for it.
    negative = base / "negative"
    negative.mkdir(mode=0o700)
    planned = negative / "planned"
    with planned.open("xb") as output:
        output.write(b"planned")
    negative_manifest = dict(root=signature(negative), nodes={"planned": signature(planned)})
    with (negative / "unplanned").open("xb") as output:
        output.write(b"must fail the fixed-name oracle")
    counts.update(nodes=nodes, root=root_identity, negative=negative_manifest)
    with (base / "expected.json").open("x") as output:
        json.dump(counts, output)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("OXIDE_", "XDG_")) or key in
           ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS")}
    env.update(TMPDIR=str(child_temp), TMP=str(child_temp), TEMP=str(child_temp),
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
    receipt = dict(qualified=False, diagnostic=diagnostic, scale="smaller-fixture", command=COMMAND,
                   child_exit=child.returncode, fixture=str(base), retained_in_evidence=True,
                   counts={key: value for key, value in counts.items() if key not in ("nodes", "negative")},
                   expected_sha256=hashlib.sha256((base / "expected.json").read_bytes()).hexdigest(),
                   wall_seconds=time.monotonic() - started,
                   observations=result,
                   source_sha256={str(path): hashlib.sha256((repo / path).read_bytes()).hexdigest()
                                  for path in ["src/inventory.rs", "scripts/test-matching-slot-cost.py", "scripts/bounded-test.py"]})
    (evidence / "matching-cost.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt), flush=True)
    return 1 if diagnostic else (0 if child.returncode == 0 and result and result["passed"] else 1)


if __name__ == "__main__":
    raise SystemExit(main())
