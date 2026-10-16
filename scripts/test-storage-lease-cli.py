"""Build oxide-storage-lease and prove the unpublished CLI.

Usage refusals always run. On macOS, after a 20 GiB host reserve, the script
provisions exactly one fresh 128 MiB APFS image (128 MiB declared cap, 512 MiB
scratch, 120 s native phase), exercises lease/release/recover/status and
capacity sampling, concurrent acquire, duplicate-holder replay, busy,
symlink/foreign refusal, idle reuse and mismatched backing, then detaches only
after positive completion. This is not a filesystem-fill benchmark.
"""
import json
import os
import shutil
import subprocess
import sys
import time

BINARY_NAME = "oxide-storage-lease"
# The declared cap must equal the image size so the volume size is the
# enforced ceiling. The measured verified capacity (APFS metadata overhead
# below this size) is the effective hard bound.
IMAGE_BYTES = 128 * 1024 * 1024
CAP_BYTES = 128 * 1024 * 1024
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
NATIVE_PHASE_SECONDS = 120


def fail(message: str) -> None:
    raise RuntimeError(message)


def scratch_root() -> str:
    raw = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if raw:
        path = os.path.realpath(raw)
        if not os.path.isdir(path):
            fail("OXIDE_TASK_SCRATCH_ROOT must be an existing directory")
        return path
    # Under heavy admission the outer environment is not forwarded; provision
    # one fresh canonical private scratch directory instead of failing.
    result = subprocess.run(
        ["mktemp", "-d", "/private/tmp/oxide-storage-lease-cli.XXXXXX"],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode:
        fail("mktemp scratch failed")
    path = os.path.realpath(result.stdout.strip())
    os.chmod(path, 0o700)
    if not os.path.isdir(path):
        fail("provisioned scratch is not a directory")
    return path


def host_free_bytes(path: str) -> int:
    result = subprocess.run(
        ["/bin/df", "-k", path], capture_output=True, text=True, timeout=30
    )
    if result.returncode:
        fail(f"df preflight failed: {result.stderr}")
    line = [line for line in result.stdout.splitlines() if line.strip()][-1]
    return int(line.split()[3]) * 1024


def build_binary() -> str:
    build = subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--bin",
            BINARY_NAME,
            "--message-format=json",
        ],
        stdout=subprocess.PIPE,
        text=True,
        timeout=300,
    )
    if build.returncode:
        diagnostic_bytes = 0
        for line in build.stdout.splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            rendered = message.get("message", {}).get("rendered")
            if message.get("reason") == "compiler-message" and rendered:
                remaining = 64 * 1024 - diagnostic_bytes
                if remaining <= 0:
                    break
                sys.stderr.write(rendered[:remaining])
                diagnostic_bytes += min(len(rendered), remaining)
        sys.exit(build.returncode)
    executable = None
    for line in build.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            message.get("reason") == "compiler-artifact"
            and message.get("target", {}).get("name") == BINARY_NAME
            and message.get("executable")
        ):
            executable = message["executable"]
    if not executable:
        fail(f"compiled artifact {BINARY_NAME} was not reported")
    return executable


def run_cli(executable: str, args: list[str], timeout: int = 60) -> tuple[int, bytes, bytes]:
    result = subprocess.run(
        [executable, *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    return result.returncode, result.stdout, result.stderr


def prove_usage(executable: str) -> None:
    code, stdout, stderr = run_cli(executable, ["--help"])
    if code != 0 or b"oxide-storage-lease" not in stdout:
        fail(f"help failed exit {code}")
    code, stdout, stderr = run_cli(executable, [])
    if code != 1:
        fail(f"missing command exit {code}, expected 1")
    code, stdout, stderr = run_cli(
        executable, ["--pool", "/no/such/pool", "status"]
    )
    if code not in (1, 4):
        fail(f"missing pool exit {code}, expected 1 or 4")

    usage = os.path.join(scratch_root(), "storage-lease-cli", "usage")
    os.makedirs(usage, exist_ok=True)
    empty = os.path.join(usage, "empty-init")
    os.mkdir(empty, 0o700)
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", empty, "init", "--cap-bytes", "8", "--image-bytes", "4"],
    )
    if code != 1:
        fail(f"undersized init exit {code}, expected 1")
    if b"image-bytes" not in stderr:
        fail(f"undersized init diagnostic {stderr!r}")
    if os.path.exists(os.path.join(empty, "lock")):
        fail("undersized init created a lock file")

    leftover = os.path.join(usage, "lock-only")
    os.mkdir(leftover, 0o700)
    os.close(os.open(os.path.join(leftover, "lock"), os.O_CREAT | os.O_WRONLY, 0o600))
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", leftover, "init", "--cap-bytes", "8", "--image-bytes", "4"],
    )
    if code != 1:
        fail(f"lock-only undersized init exit {code}, expected 1")
    if b"empty pool directory" in stderr:
        fail(f"lock-only leftover refused as nonempty: {stderr!r}")
    if b"image-bytes" not in stderr:
        fail(f"lock-only leftover missed cap check: {stderr!r}")


def load_json(payload: bytes, label: str) -> dict:
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not JSON ({error})")
    if not isinstance(value, dict):
        fail(f"{label} is not an object")
    return value


def snapshot_capacity_state(pool: str, slot_root: str) -> list[tuple[str, bytes]]:
    snapshots = []
    for name in ("config.json", "ledger.json", "receipt.json"):
        path = os.path.join(pool, name)
        with open(path, "rb") as handle:
            snapshots.append((name, handle.read()))
    for current, dirs, files in os.walk(slot_root, followlinks=False):
        for name in sorted(dirs + files):
            path = os.path.join(current, name)
            relative = os.path.relpath(path, pool)
            if os.path.islink(path):
                fail(f"capacity fixture unexpectedly contains a symlink: {relative}")
            if os.path.isfile(path):
                with open(path, "rb") as handle:
                    snapshots.append((relative, handle.read()))
    return sorted(snapshots, key=lambda item: item[0])


def fixture_footprint_bytes(root: str) -> int:
    total = 0
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            path = os.path.join(current, name)
            if os.path.islink(path):
                continue
            if os.path.isfile(path):
                metadata = os.stat(path)
                total += max(metadata.st_size, metadata.st_blocks * 512)
    return total


def prove_native(executable: str, parent: str) -> None:
    free = host_free_bytes(parent)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB native reserve")
    pool = os.path.join(parent, "pool")
    os.mkdir(pool, 0o700)
    os.close(os.open(os.path.join(pool, "lock"), os.O_CREAT | os.O_WRONLY, 0o600))
    project = os.path.join(parent, "project")
    workspace_a = os.path.join(parent, "ws-a")
    workspace_b = os.path.join(parent, "ws-b")
    project_b = os.path.join(parent, "project-b")
    for path in (project, workspace_a, workspace_b, project_b):
        os.mkdir(path)
    started = time.monotonic()
    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool",
            pool,
            "init",
            "--cap-bytes",
            str(CAP_BYTES),
            "--image-bytes",
            str(IMAGE_BYTES),
        ],
        timeout=NATIVE_PHASE_SECONDS,
    )
    if code != 0:
        fail(f"init failed exit {code} stderr={stderr!r}")
    info = load_json(stdout, "init")
    if info.get("backend") != "apfs-sparse-image-v1":
        fail("init did not pin apfs-sparse-image-v1")
    if info.get("cap_bytes") != CAP_BYTES:
        fail("init cap mismatch")
    verified = info.get("verified_capacity_bytes")
    if not isinstance(verified, int) or verified <= 0 or verified > CAP_BYTES:
        fail(f"init verified capacity is not an enforced bound below cap: {verified!r}")

    # A smaller declared cap on a larger image is not OS-enforced and must
    # refuse before any host effect beyond the empty pool directory.
    refused_pool = os.path.join(parent, "refused-pool")
    os.mkdir(refused_pool, 0o700)
    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool",
            refused_pool,
            "init",
            "--cap-bytes",
            str(8 * 1024 * 1024),
            "--image-bytes",
            str(IMAGE_BYTES),
        ],
        timeout=NATIVE_PHASE_SECONDS,
    )
    if code != 1:
        fail(f"smaller-cap init exit {code}, expected 1")

    def remaining() -> int:
        return max(5, int(NATIVE_PHASE_SECONDS - (time.monotonic() - started)))

    payload_root = os.path.join(pool, "payload")
    metadata_names = (".fseventsd", ".Spotlight-V100", ".Trashes", ".TemporaryItems")
    initially_absent = [
        name for name in metadata_names
        if not os.path.lexists(os.path.join(payload_root, name))
    ]
    delayed_metadata_name = initially_absent[0] if initially_absent else None
    # Give macOS time to add filesystem metadata after init. If this host does
    # not create an initially absent known directory, add that exact directory
    # in this fresh proof image. The Rust fixture covers the deterministic
    # post-init transition on hosts where all known directories already exist.
    time.sleep(3)
    payload_stat = os.stat(payload_root)
    if delayed_metadata_name is not None:
        delayed_metadata = os.path.join(payload_root, delayed_metadata_name)
        metadata_deadline = min(time.monotonic() + 5, started + NATIVE_PHASE_SECONDS - 5)
        while not os.path.lexists(delayed_metadata) and time.monotonic() < metadata_deadline:
            time.sleep(0.25)
        if not os.path.lexists(delayed_metadata):
            try:
                os.mkdir(delayed_metadata)
            except FileExistsError:
                pass
        metadata_stat = os.lstat(delayed_metadata)
        if not os.path.isdir(delayed_metadata) or os.path.islink(delayed_metadata):
            fail("delayed APFS metadata entry is not a real directory")
        if metadata_stat.st_dev != payload_stat.st_dev:
            fail("delayed APFS metadata directory is on another volume")

    code, stdout, stderr = run_cli(
        executable, ["--pool", pool, "status", "--json"], timeout=remaining()
    )
    if code != 0:
        fail(f"status after delayed filesystem metadata failed: exit {code} stderr={stderr!r}")
    delayed_status = load_json(stdout, "status after delayed metadata")
    if delayed_status.get("slots") or delayed_status.get("coverage") != "complete":
        fail("delayed OS metadata changed empty-slot accounting")
    if delayed_status.get("payload_unique_allocated_bytes") != 0:
        fail("delayed OS metadata was counted as a lease slot")

    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool",
            pool,
            "lease",
            "--project",
            project,
            "--workspace",
            workspace_a,
            "--profile",
            "dev",
            "--holder",
            "holder-a",
        ],
        timeout=remaining(),
    )
    if code != 0:
        fail(f"lease failed exit {code} stderr={stderr!r}")
    grant = load_json(stdout, "lease")
    if grant.get("version") != 1 or not grant.get("lease_id"):
        fail("lease JSON missing identity")
    env = grant.get("env") or {}
    if "CARGO_BUILD_BUILD_DIR" not in env or "CARGO_TARGET_DIR" not in env:
        fail("lease omitted exact environment")
    if env["CARGO_TARGET_DIR"] != os.path.join(workspace_a, "target"):
        fail("CARGO_TARGET_DIR must stay private to the workspace")

    sample_path = os.path.join(env["CARGO_BUILD_BUILD_DIR"], "capacity-sample.bin")
    with open(sample_path, "wb") as handle:
        handle.write(b"capacity fixture" * 256)
    slot_root = os.path.dirname(env["CARGO_BUILD_BUILD_DIR"])
    before_capacity = snapshot_capacity_state(pool, slot_root)
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", pool, "status", "--capacity", "--json"],
        timeout=remaining(),
    )
    capacity_exit = code
    capacity_report = load_json(stdout, "capacity status")
    if capacity_report.get("version") != 1:
        fail("capacity status did not use envelope version 1")
    notes = capacity_report.get("notes") or []
    if not any("not filesystem headroom" in note for note in notes):
        fail("capacity JSON did not label legacy full as a cap comparison")
    nested = capacity_report.get("status")
    if not isinstance(nested, dict):
        fail("capacity status omitted nested ordinary status")
    expected_status_fields = {
        "version", "store_id", "backend", "cap_bytes", "payload_logical_bytes",
        "payload_unique_allocated_bytes", "coverage", "full", "slots",
    }
    if set(nested) != expected_status_fields:
        fail(f"nested ordinary status shape changed: {set(nested)!r}")
    if nested.get("full") is not False:
        fail("legacy full must retain its declared-cap comparison for a small slot")
    if nested.get("coverage") != "complete" or not nested.get("slots"):
        fail("capacity report lost complete ordinary slot accounting")
    slot = next(
        (item for item in nested["slots"] if item.get("slot_id") == grant["slot_id"]),
        None,
    )
    if not slot or slot.get("state") != "held" or slot.get("logical_bytes", 0) < 4096:
        fail("capacity report did not retain the held slot observation")
    observed = capacity_report.get("capacity")
    if not isinstance(observed, dict):
        fail("capacity report omitted observations")
    if observed.get("declared_cap_bytes") != CAP_BYTES:
        fail("capacity report declared cap mismatch")
    if observed.get("verified_capacity_bytes") != verified:
        fail("capacity report changed the verified payload ceiling")
    for name in ("payload_free_bytes", "payload_available_bytes"):
        sample = observed.get(name) or {}
        value = sample.get("value_bytes")
        if not isinstance(value, int) or not 0 <= value <= verified:
            fail(f"{name} is not a raw payload byte sample: {sample!r}")
    image_logical = observed.get("image_logical_bytes", {}).get("value_bytes")
    if not isinstance(image_logical, int) or image_logical < 0:
        fail("image logical st_size is not an exact byte count")
    image_allocated = observed.get("image_allocated_bytes", {}).get("value_bytes")
    if not isinstance(image_allocated, int) or image_allocated < 0:
        fail("image st_blocks allocation is not an exact byte count")
    host_available = observed.get("host_available_bytes") or {}
    if host_available.get("value_bytes") is None and not host_available.get("unknown_reason"):
        fail("unknown host available capacity omitted its reason")
    if observed.get("coverage") not in ("complete", "partial"):
        fail(f"unexpected capacity coverage: {observed.get('coverage')!r}")
    expected_partial = (
        nested.get("coverage") != "complete" or observed.get("coverage") != "complete"
    )
    if (capacity_exit == 2) != expected_partial:
        fail(f"capacity status partial exit mismatch: exit {capacity_exit}")
    if capacity_exit not in (0, 2):
        fail(f"capacity status failed exit {capacity_exit}: {stderr!r}")
    if snapshot_capacity_state(pool, slot_root) != before_capacity:
        fail("capacity status changed config, ledger, receipt or held slot bytes")

    code, stdout, stderr = run_cli(
        executable,
        ["--pool", pool, "status", "--capacity"],
        timeout=remaining(),
    )
    if code != capacity_exit:
        fail(f"human capacity partial exit mismatch: {code} != {capacity_exit}")
    human = stdout.decode("utf-8")
    if "legacy cap" not in human or "not reservations" not in human:
        fail("human capacity output omitted its scope and limits")

    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool", pool, "status", "--capacity", "--json",
            "--max-output-bytes", "64",
        ],
        timeout=remaining(),
    )
    if code != 2 or len(stdout) > 64:
        fail(f"64-byte capacity JSON cap failed: exit {code}, bytes {len(stdout)}")
    if not load_json(stdout, "64-byte capacity JSON").get("truncated"):
        fail("64-byte capacity JSON did not mark truncation")
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", pool, "status", "--capacity", "--max-output-bytes", "64"],
        timeout=remaining(),
    )
    if code != 2 or len(stdout) > 64:
        fail(f"64-byte capacity human cap failed: exit {code}, bytes {len(stdout)}")
    stdout.decode("utf-8")

    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool",
            pool,
            "lease",
            "--project",
            project,
            "--workspace",
            workspace_a,
            "--profile",
            "dev",
            "--holder",
            "holder-a",
        ],
        timeout=remaining(),
    )
    if code != 0:
        fail(f"duplicate lease failed exit {code}")
    replay = load_json(stdout, "duplicate")
    if replay.get("lease_id") != grant["lease_id"]:
        fail("duplicate holder allocated a second lease")

    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool",
            pool,
            "lease",
            "--project",
            project,
            "--workspace",
            workspace_a,
            "--profile",
            "dev",
            "--holder",
            "holder-b",
        ],
        timeout=remaining(),
    )
    if code != 2:
        fail(f"busy exit {code}, expected 2")

    first, second = [
        subprocess.Popen(
            [
                executable,
                "--pool",
                pool,
                "lease",
                "--project",
                project_b,
                "--workspace",
                workspace_b,
                "--profile",
                "full-debug",
                "--holder",
                holder,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        for holder in ("holder-c", "holder-d")
    ]
    results = []
    for child in (first, second):
        out, err = child.communicate(timeout=remaining())
        results.append((child.returncode, out, err))
    wins = [item for item in results if item[0] == 0]
    if len(wins) != 1:
        fail(f"concurrent acquire wins={len(wins)} results={results!r}")

    code, stdout, stderr = run_cli(
        executable,
        ["--pool", pool, "release", "--lease", grant["lease_id"]],
        timeout=remaining(),
    )
    if code != 0:
        fail(f"release failed exit {code}")
    receipt = load_json(stdout, "release")
    if receipt.get("state") != "idle" or receipt.get("replay") is True:
        fail("release did not idle the lease")
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", pool, "release", "--lease", grant["lease_id"]],
        timeout=remaining(),
    )
    if code != 0 or load_json(stdout, "release-replay").get("replay") is not True:
        fail("release replay was not idempotent")
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", pool, "recover", "--lease", grant["lease_id"]],
        timeout=remaining(),
    )
    if code != 0 or load_json(stdout, "recover").get("replay") is not True:
        fail("recover replay failed")

    code, stdout, stderr = run_cli(
        executable,
        [
            "--pool",
            pool,
            "lease",
            "--project",
            project,
            "--workspace",
            workspace_b,
            "--profile",
            "dev",
            "--holder",
            "holder-e",
        ],
        timeout=remaining(),
    )
    if code != 0:
        fail(f"idle reuse failed exit {code}")
    reused = load_json(stdout, "reuse")
    if reused.get("slot_id") != grant["slot_id"] or not reused.get("reused_slot"):
        fail("idle different workspace did not reuse the compatible slot")
    if reused.get("workspace") != workspace_b:
        fail("workspace observation disappeared")

    link = os.path.join(parent, "pool-link")
    os.symlink(pool, link)
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", link, "status", "--json"],
        timeout=remaining(),
    )
    if code != 4:
        fail(f"symlink pool exit {code}, expected 4")
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", link, "status", "--capacity", "--json"],
        timeout=remaining(),
    )
    if code != 4:
        fail(f"capacity symlink pool exit {code}, expected 4")

    fake = os.path.join(parent, "fake-pool")
    os.mkdir(fake, 0o700)
    fake_config = os.path.join(fake, "config.json")
    with open(fake_config, "w", encoding="utf-8") as handle:
        json.dump(
            {
                "version": 1,
                "backend": "apfs-sparse-image-v1",
                "store_id": "aa" * 16,
                "cap_bytes": CAP_BYTES,
                "image_bytes": IMAGE_BYTES,
            },
            handle,
        )
    os.chmod(fake_config, 0o600)
    os.close(os.open(os.path.join(fake, "lock"), os.O_CREAT | os.O_WRONLY, 0o600))
    code, stdout, stderr = run_cli(
        executable, ["--pool", fake, "status", "--json"], timeout=remaining()
    )
    if code not in (3, 4):
        fail(f"mismatched backing exit {code}, expected 3 or 4")
    code, stdout, stderr = run_cli(
        executable,
        ["--pool", fake, "status", "--capacity", "--json"],
        timeout=remaining(),
    )
    if code != 4:
        fail(f"capacity missing-backing exit {code}, expected 4")

    code, stdout, stderr = run_cli(
        executable, ["--pool", pool, "status", "--json"], timeout=remaining()
    )
    if code != 0:
        fail(f"status failed exit {code} stderr={stderr!r}")
    report = load_json(stdout, "status")
    if report.get("backend") != "apfs-sparse-image-v1":
        fail("status omitted verified backend")
    if report.get("coverage") == "unknown":
        fail("status claimed unknown accounting on a fresh pool")

    detach = subprocess.run(
        ["/usr/bin/hdiutil", "detach", os.path.join(pool, "payload")],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if detach.returncode:
        fail(f"detach failed after positive completion: {detach.stderr}")
    if time.monotonic() - started > NATIVE_PHASE_SECONDS:
        fail("native phase exceeded 120 seconds")


def main() -> int:
    parent = os.path.join(scratch_root(), "storage-lease-cli")
    os.makedirs(parent, exist_ok=True)
    executable = build_binary()
    prove_usage(executable)
    if sys.platform != "darwin":
        fail("bounded APFS lease proof requires macOS")
    native = os.path.join(parent, "native")
    os.makedirs(native, exist_ok=True)
    try:
        prove_native(executable, native)
    except Exception:
        print(f"native lease proof retained its owned scratch at {native}", file=sys.stderr)
        raise
    footprint = fixture_footprint_bytes(native)
    if footprint > SCRATCH_LIMIT_BYTES:
        fail(f"native fixture footprint {footprint} exceeds the 512 MiB scratch cap")
    shutil.rmtree(native)
    print("oxide-storage-lease CLI proof held")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        sys.stderr.write(f"{error}\n")
        sys.exit(1)
