"""Effigy-owned covering proof: build the real child, then run bounded fixtures."""
import json
import os
import subprocess
import sys

subprocess.run(["node", "scripts/qualification-vectors.mjs"], check=True, timeout=30)

build = subprocess.run(
    ["cargo", "build", "--offline", "--locked", "--features", "private-qualification-fixture",
     "--bin", "oxide-qualified-runner", "--bin", "oxide-storage-runtime",
     "--bin", "oxide-registry-acquisition", "--bin", "oxide-ordinary-cargo-attempt",
     "--message-format=json"],
    stdout=subprocess.PIPE, text=True, timeout=300,
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
            output = rendered[:remaining]
            sys.stderr.write(output)
            diagnostic_bytes += len(output)
    sys.exit(build.returncode)
executables = {}
for line in build.stdout.splitlines():
    message = json.loads(line)
    if message.get("reason") == "compiler-artifact" and message["target"]["name"] in (
        "oxide-qualified-runner", "oxide-storage-runtime", "oxide-registry-acquisition",
        "oxide-ordinary-cargo-attempt"
    ):
        executables[message["target"]["name"]] = message.get("executable")
for name in ("oxide-qualified-runner", "oxide-storage-runtime", "oxide-registry-acquisition",
             "oxide-ordinary-cargo-attempt"):
    if not executables.get(name):
        raise RuntimeError(f"compiled artifact {name} was not reported")
environment = dict(
    os.environ,
    OXIDE_QUALIFICATION_CHILD=executables["oxide-qualified-runner"],
    OXIDE_STORAGE_RUNTIME=executables["oxide-storage-runtime"],
    OXIDE_ORDINARY_CARGO_ATTEMPT=executables["oxide-ordinary-cargo-attempt"],
)
result = subprocess.run(
    ["python3", "scripts/bounded-test.py", "cargo", "test", "--offline", "--locked", "--features", "private-qualification-fixture",
     "--lib", "private_qualification", "--", "--test-threads=1", "--nocapture"],
    env=environment, timeout=330,
)
if result.returncode:
    sys.exit(result.returncode)
process_result = subprocess.run(
    ["python3", "scripts/bounded-test.py", "cargo", "test", "--offline", "--locked",
     "--features", "private-qualification-fixture", "--lib", "private_ordinary_cargo_process",
     "--", "--test-threads=1", "--nocapture"],
    env=environment, timeout=330,
)
if process_result.returncode:
    sys.exit(process_result.returncode)
unit_result = subprocess.run(
    ["python3", "scripts/bounded-test.py", "cargo", "test", "--offline", "--locked",
     "--features", "private-qualification-fixture", "--lib", "ordinary_cargo::route_controls",
     "--", "--test-threads=1", "--nocapture"],
    env=environment, timeout=330,
)
sys.exit(unit_result.returncode)
