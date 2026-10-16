"""Build oxide-storage-lease and prove the unpublished CLI.

Usage refusals always run. On macOS, after a 20 GiB host reserve, the script
provisions exactly one fresh 128 MiB APFS image (8 MiB payload cap, 512 MiB
scratch, 120 s native phase), exercises lease/release/recover/status,
concurrent acquire, duplicate-holder replay, busy, symlink/foreign refusal,
idle reuse and mismatched backing, then detaches only after positive
completion. This is not a filesystem-fill benchmark.
"""
import json
import os
import shutil
import subprocess
import sys
import time

BINARY_NAME = "oxide-storage-lease"
IMAGE_BYTES = 128 * 1024 * 1024
CAP_BYTES = 8 * 1024 * 1024
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
NATIVE_PHASE_SECONDS = 120


def fail(message: str) -> None:
    raise RuntimeError(message)


def scratch_root() -> str:
    raw = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not raw:
        fail("OXIDE_TASK_SCRATCH_ROOT is required")
    path = os.path.realpath(raw)
    if not os.path.isdir(path):
        fail("OXIDE_TASK_SCRATCH_ROOT must be an existing directory")
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


def load_json(payload: bytes, label: str) -> dict:
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not JSON ({error})")
    if not isinstance(value, dict):
        fail(f"{label} is not an object")
    return value


def prove_native(executable: str, parent: str) -> None:
    free = host_free_bytes(parent)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB native reserve")
    pool = os.path.join(parent, "pool")
    os.mkdir(pool, 0o700)
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

    def remaining() -> int:
        return max(5, int(NATIVE_PHASE_SECONDS - (time.monotonic() - started)))

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

    fake = os.path.join(parent, "fake-pool")
    os.mkdir(fake, 0o700)
    with open(os.path.join(fake, "config.json"), "w", encoding="utf-8") as handle:
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
    code, stdout, stderr = run_cli(
        executable, ["--pool", fake, "status", "--json"], timeout=remaining()
    )
    if code not in (3, 4):
        fail(f"mismatched backing exit {code}, expected 3 or 4")

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
    shutil.rmtree(native)
    print("oxide-storage-lease CLI proof held")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        sys.stderr.write(f"{error}\n")
        sys.exit(1)
