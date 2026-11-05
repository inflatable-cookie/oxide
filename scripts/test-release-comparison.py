#!/usr/bin/env python3
"""Three actual-engine calibrations; existing image kit and guard own custody.

HIGH premise checklist (the Rust case body contains no child commands):
1/2: Rust engines receive explicit fixture Store/AccountFacts; no default-home
lookup, rustup or fixture Cargo call. Outer oxide test uses ordinary managed
compiler storage and existing pinned 1.97.1/locked inputs, without staging.
3: scrub inherited product/test/XDG controls before the fixed outer command;
only comparison mount/UUID and guard controls reach the embedded cases.
4/5: no adoption/build inside a checkout; no external Cargo subcommand.
6: existing ImageCustody attaches -nobrowse -owners on, retains raw plist;
only the unchanged guard may detach, through the existing atomic protocol.
7: clear both compiler flag encodings consistently; no staged binary.
8/9: guard-selected Stash scratch/evidence; mkdtemp(dir=...) is explicit.
One 128MiB image plus tiny fixture files and retained evidence, 512MiB guard
sampling budget. No internal fixture exception. Not a quota or full parity.
"""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import plistlib
import secrets
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("outputs_prune_image_kit", HERE / "test-outputs-prune.py")
kit = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = kit
spec.loader.exec_module(kit)


def work_left():
    left = (int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
            - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - 30
    if left <= 5:
        raise RuntimeError("comparison reached guard shutdown margin")
    return left


def atomic(path, value):
    temporary = path.with_suffix(".new")
    with temporary.open("x") as output:
        json.dump(value, output)
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)


def main():
    diagnostic = sys.argv[1:] == ["--diagnostic"]
    if sys.argv[1:] not in ([], ["--diagnostic"]):
        raise RuntimeError("only --diagnostic is supported")
    evidence = Path(os.environ["OXIDE_PROOF_EVIDENCE"])
    scratch = Path(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    kit.verify_evidence_root(str(evidence), str(scratch))
    assert hashlib.sha256((HERE / "bounded-test.py").read_bytes()).hexdigest() == \
        "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
    assert scratch.is_relative_to("/Volumes/Stash/scratch/oxide")
    assert Path("/Volumes/Stash").resolve() == Path("/Volumes/Stash")
    stash = subprocess.run(["/usr/sbin/diskutil", "info", "-plist", "/Volumes/Stash"],
                           capture_output=True, timeout=min(10, work_left()), check=True)
    info = plistlib.loads(stash.stdout)
    assert info["VolumeUUID"].upper() == "8A660440-C919-45C6-A3F1-3365B9BD2006"
    assert info["FilesystemType"].lower() == "apfs"
    assert info.get("Internal") is False and info.get("GlobalPermissionsEnabled") is True
    assert not os.statvfs("/Volumes/Stash").f_flag & os.ST_RDONLY
    assert info["MountPoint"] == "/Volumes/Stash"
    prefix = Path(tempfile.mkdtemp(prefix="comparison-", dir=scratch))
    name = "o279-" + secrets.token_hex(4)
    assert not os.path.lexists("/Volumes/" + name)
    kit.permanent = sys.modules[__name__]
    custody = kit.ImageCustody(name, custody_protocol=True)
    custody.allocate(str(evidence), str(scratch))
    atomic(evidence / "image-custody.json", {"version": 1, "backings": [custody.backing]})
    mount = custody.attach()
    assert custody.volume_uuid and custody.device and custody.backing_identity
    assert Path(mount).resolve() == Path(mount)
    mounted = subprocess.run(["/usr/sbin/diskutil", "info", "-plist", custody.device],
                             capture_output=True, timeout=min(10, work_left()), check=True)
    mounted_info = plistlib.loads(mounted.stdout)
    assert mounted_info["FilesystemType"].lower() == "apfs"
    assert mounted_info["MountPoint"] == mount
    assert mounted_info["VolumeUUID"].upper() == custody.volume_uuid.upper()
    assert mounted_info.get("GlobalPermissionsEnabled") is True
    print("owned image capacity", os.statvfs(mount).f_blocks * os.statvfs(mount).f_frsize,
          "backing allocated bytes", os.stat(custody.backing).st_blocks * 512, flush=True)
    # Both engines independently verify the actual image mount/UUID. No alias.
    os.mkdir(os.path.join(mount, "oxide-outputs"), 0o700)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("XDG_", "OXIDE_TEST_"))
           and key not in ("OXIDE_POOL", "OXIDE_CONFIG", "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS")}
    env.update(OXIDE_COMPARISON_MOUNT=mount, OXIDE_COMPARISON_UUID=custody.volume_uuid.upper(),
               TMPDIR=str(prefix), TMP=str(prefix), TEMP=str(prefix),
               RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS="", RUSTUP_AUTO_INSTALL="0")
    print("source", subprocess.check_output(["git", "rev-parse", "HEAD"], text=True,
                                           timeout=min(5, work_left())).strip(),
          "Barge pin 4af9360bd09629e970c143080ca159de8eef2562", flush=True)
    command = ["oxide", "test", "--bin", "oxide", "--locked", "release_comparison_",
               "--", "--ignored", "--test-threads=1", "--nocapture"]
    # libtest keeps going through all three independent cases on either occurrence.
    with (evidence / "comparison.log").open("xb") as log:
        result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT,
                                timeout=work_left())
    # The guard decides census, closure and detach; no manual cleanup or retry.
    atomic(evidence / "detach-request.json", {
        "version": 1, "backing": custody.backing, "mount": mount,
        "device": custody.device, "volume_uuid": custody.volume_uuid.lower(),
        "note": "three controlled comparison cases; guard owns detach",
    })
    print(f"comparison diagnostic={diagnostic} child_exit={result.returncode}; evidence={evidence}")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
