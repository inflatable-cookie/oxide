"""The one opt-in macOS filesystem-full proof (oxide#055).

Runs a single bounded native test through Effigy heavy admission. Per
participant it creates exactly one fresh owned 128 MiB APFS sparse image in a
fresh task-owned scratch directory, bounds the writer payload at 128 MiB with
1 MiB IO buffers and the native phase at 120 seconds, after a 20 GiB host
reserve. On any failure the owned scratch is retained and its path printed; no
existing filesystem or volume outside the scratch is touched.
"""
import json
import os
import shutil
import subprocess
import sys

SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024


def host_free_bytes(path: str) -> int:
    result = subprocess.run(["/bin/df", "-k", path], capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f"df preflight failed: {result.stderr}")
    line = [line for line in result.stdout.splitlines() if line.strip()][-1]
    return int(line.split()[3]) * 1024


def main() -> int:
    scratch = subprocess.run(
        ["mktemp", "-d", "/private/tmp/oxide-bounded-native.XXXXXX"],
        capture_output=True, text=True, timeout=30, check=True,
    ).stdout.strip()
    free = host_free_bytes(scratch)
    if free < HOST_RESERVE_BYTES:
        shutil.rmtree(scratch)
        print(f"host free {free} is below the 20 GiB native reserve; nothing was created")
        return 3
    print(f"owned scratch: {scratch} (host free {free})")
    environment = dict(
        os.environ,
        OXIDE_TASK_SCRATCH_ROOT=scratch,
        TMPDIR=scratch,
        TMP=scratch,
        TEMP=scratch,
    )
    try:
        build = subprocess.run(
            ["cargo", "test", "--offline", "--locked", "--features",
             "private-qualification", "--lib", "--no-run", "--message-format=json"],
            cwd=os.getcwd(), env=environment, timeout=900,
            stdout=subprocess.PIPE, stderr=None, text=True,
        )
        if build.returncode:
            for line in build.stdout.splitlines():
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if message.get("reason") == "compiler-message" and message.get("message", {}).get("rendered"):
                    sys.stderr.write(message["message"]["rendered"] + "\n")
            print("native proof test binary failed to compile", file=sys.stderr)
            return build.returncode
        # Compile the real runtime child as well; the test launches it by env.
        runtime = subprocess.run(
            ["cargo", "build", "--offline", "--locked", "--features",
             "private-qualification", "--bin", "oxide-storage-runtime", "--message-format=json"],
            cwd=os.getcwd(), env=environment, timeout=600,
            stdout=subprocess.PIPE, stderr=None, text=True,
        )
        if runtime.returncode:
            print("runtime child failed to compile", file=sys.stderr)
            return runtime.returncode
        executable = None
        for line in runtime.stdout.splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if (message.get("reason") == "compiler-artifact"
                    and message.get("target", {}).get("name") == "oxide-storage-runtime"):
                executable = message.get("executable")
        if not executable:
            print("runtime child artifact was not reported", file=sys.stderr)
            return 1
        environment["OXIDE_STORAGE_RUNTIME"] = executable
        environment["OXIDE_BOUNDED_NATIVE_RUN"] = "1"
        result = subprocess.run(
            ["python3", "scripts/bounded-test.py", "cargo", "test", "--offline", "--locked",
             "--features", "private-qualification", "--lib",
             "private_qualification_runtime::bounded_native_payload_filesystem_full_refuses_under_open_lease",
             "--", "--exact", "--ignored", "--test-threads=1", "--nocapture"],
            cwd=os.getcwd(), env=environment, timeout=600,
        )
        if result.returncode:
            print(f"native proof retained its owned scratch at {scratch}", file=sys.stderr)
            return result.returncode
        shutil.rmtree(scratch)
        print("native bounded proof passed; owned scratch removed")
        return 0
    except subprocess.TimeoutExpired:
        print(f"native proof exceeded its supervision bound; retained {scratch}", file=sys.stderr)
        return 124


if __name__ == "__main__":
    sys.exit(main())
