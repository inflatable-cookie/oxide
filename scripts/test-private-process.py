"""Effigy-owned covering proof: build the real child, then run bounded fixtures."""
import json
import os
import subprocess
import sys

subprocess.run(["node", "scripts/qualification-vectors.mjs"], check=True, timeout=30)

build = subprocess.run(
    ["cargo", "build", "--offline", "--locked", "--features", "private-qualification",
     "--bin", "oxide-qualified-runner", "--bin", "oxide-storage-runtime", "--message-format=json"],
    stdout=subprocess.PIPE, text=True, timeout=300,
)
if build.returncode:
    sys.exit(build.returncode)
executables = {}
for line in build.stdout.splitlines():
    message = json.loads(line)
    if message.get("reason") == "compiler-artifact" and message["target"]["name"] in (
        "oxide-qualified-runner", "oxide-storage-runtime"
    ):
        executables[message["target"]["name"]] = message.get("executable")
for name in ("oxide-qualified-runner", "oxide-storage-runtime"):
    if not executables.get(name):
        raise RuntimeError(f"compiled artifact {name} was not reported")
environment = dict(
    os.environ,
    OXIDE_QUALIFICATION_CHILD=executables["oxide-qualified-runner"],
    OXIDE_STORAGE_RUNTIME=executables["oxide-storage-runtime"],
)
result = subprocess.run(
    ["python3", "scripts/bounded-test.py", "cargo", "test", "--offline", "--locked", "--features", "private-qualification",
     "--lib", "private_qualification", "--", "--test-threads=1", "--nocapture"],
    env=environment, timeout=330,
)
sys.exit(result.returncode)
