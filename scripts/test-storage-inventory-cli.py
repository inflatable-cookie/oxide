"""Build oxide-storage-inventory and prove the CLI output-byte ceiling."""
import json
import os
import subprocess
import sys

BINARY_NAME = "oxide-storage-inventory"
JSON_TRUNCATED_FALLBACK = b'{"truncated":true,"partial":true}\n'
HUMAN_TRUNCATION_MARKER = b"[output truncated at max_output_bytes]"
DEFAULT_OUTPUT_CAP = 65536
TINY_OUTPUT_CAP = 64


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


def make_fixture(parent: str, label: str, names: list[str]) -> str:
    root = os.path.join(parent, label)
    os.makedirs(root, exist_ok=True)
    for relative in names:
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(b"x" * 10)
    return root


def run_cli(executable: str, root: str, extra: list[str]) -> tuple[int, bytes, bytes]:
    result = subprocess.run(
        [executable, "--root", root, *extra],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=30,
    )
    return result.returncode, result.stdout, result.stderr


def require_utf8(payload: bytes, label: str) -> str:
    try:
        return payload.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"{label} is not valid UTF-8 ({error})")
    raise AssertionError


def prove_tiny(executable: str, root: str, extra: list[str], json_mode: bool) -> None:
    code, stdout, stderr = run_cli(
        executable, root, ["--max-output-bytes", str(TINY_OUTPUT_CAP), *extra]
    )
    label = "json" if json_mode else "human"
    if code != 2:
        fail(f"tiny {label} exit {code}, expected 2 (stderr {len(stderr)} bytes)")
    if len(stdout) > TINY_OUTPUT_CAP:
        fail(f"tiny {label} wrote {len(stdout)} bytes, cap {TINY_OUTPUT_CAP}")
    text = require_utf8(stdout, f"tiny {label}")
    if json_mode:
        try:
            value = json.loads(text)
        except json.JSONDecodeError as error:
            fail(f"tiny json is not valid JSON ({error})")
        if value != {"truncated": True, "partial": True} and stdout != JSON_TRUNCATED_FALLBACK:
            fail("tiny json must be the truncated fallback object")
        if value.get("truncated") is not True or value.get("partial") is not True:
            fail("tiny json must set truncated and partial")
    elif HUMAN_TRUNCATION_MARKER not in stdout:
        fail("tiny human output omitted the truncation marker")


def prove_normal(executable: str, root: str) -> None:
    code, stdout, stderr = run_cli(executable, root, [])
    if code != 0:
        fail(f"normal human exit {code}, expected 0 (stderr {len(stderr)} bytes)")
    if len(stdout) > DEFAULT_OUTPUT_CAP:
        fail(f"normal human wrote {len(stdout)} bytes, cap {DEFAULT_OUTPUT_CAP}")
    require_utf8(stdout, "normal human")
    if HUMAN_TRUNCATION_MARKER in stdout:
        fail("normal human output claimed truncation")

    code, stdout, stderr = run_cli(executable, root, ["--json"])
    if code != 0:
        fail(f"normal json exit {code}, expected 0 (stderr {len(stderr)} bytes)")
    if len(stdout) > DEFAULT_OUTPUT_CAP:
        fail(f"normal json wrote {len(stdout)} bytes, cap {DEFAULT_OUTPUT_CAP}")
    text = require_utf8(stdout, "normal json")
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        fail(f"normal json is not valid JSON ({error})")
    if not isinstance(value, dict) or "root" not in value:
        fail("normal json omitted the report root")
    if value.get("truncated") is True:
        fail("normal json claimed truncation")


def main() -> int:
    parent = os.path.join(scratch_root(), "storage-inventory-cli")
    os.makedirs(parent, exist_ok=True)
    executable = build_binary()
    tiny = make_fixture(parent, "tiny", ["debug/deps/f.rlib"])
    prove_tiny(executable, tiny, [], json_mode=False)
    prove_tiny(executable, tiny, ["--json"], json_mode=True)
    multibyte = make_fixture(
        parent,
        "é" * 80,
        ["debug/deps/f.rlib", "α-note/debug/incremental/session.o"],
    )
    prove_tiny(executable, multibyte, [], json_mode=False)
    prove_tiny(executable, multibyte, ["--json"], json_mode=True)
    prove_normal(executable, tiny)
    print("oxide-storage-inventory CLI output ceiling held")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        sys.stderr.write(f"{error}\n")
        sys.exit(1)
