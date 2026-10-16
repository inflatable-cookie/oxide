"""Verify the opt-in oxide-light / oxide-full-debug Cargo profiles (oxide#068).

Runs the workspace profile probe under one named profile and checks the debug
settings Cargo actually resolved for workspace, normal dependency and
build-time (build script / proc macro) units. A profile name or an inheritance
statement is not accepted as proof: the script parses the rustc invocations
Cargo emitted for this build. The probe itself proves workspace panic frames
keep file:line and that panics still unwind.

The script also refuses to run if the default dev/test/release profiles or the
pinned stable toolchain were changed. This is a focused proof, not a benchmark
and not an alias for full QA.

Usage: python3 scripts/test-cargo-profiles.py --profile oxide-light
"""

import argparse
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROBE_TEST = "profile_backtrace"
PROBE_TEST_NAME = "profile_probe_keeps_workspace_frames_and_unwind"
CARGO_TIMEOUT_SECONDS = 900
OUTPUT_CAP_BYTES = 8 * 1024 * 1024

# Expected resolved debug encoding per unit class. `None` means "no debug
# information at all" (Cargo emits `-C strip=debuginfo` for an explicit
# `debug = false`).
EXPECTED_DEBUG = {
    "oxide-light": {"workspace": "line-tables-only", "dependency": None, "build_time": None},
    "oxide-full-debug": {"workspace": "2", "dependency": "2", "build_time": "2"},
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8") as handle:
        return handle.read()


def check_manifest_unchanged() -> None:
    manifest = read_text(os.path.join(REPO_ROOT, "Cargo.toml"))
    for name in ("dev", "test", "release"):
        if re.search(r"^\[profile\.%s\]" % re.escape(name), manifest, re.MULTILINE):
            fail("default %s profile was overridden; opt-in profiles must not change it" % name)
    for profile in EXPECTED_DEBUG:
        if not re.search(r"^\[profile\.%s\]" % re.escape(profile), manifest, re.MULTILINE):
            fail("expected opt-in profile %r is missing" % profile)
    toolchain = read_text(os.path.join(REPO_ROOT, "rust-toolchain.toml"))
    if 'channel = "1.97.1"' not in toolchain:
        fail("pinned stable toolchain channel changed")


def proof_target_dir() -> str:
    """A fresh private target directory, removed once the proof completes.

    Cargo only prints an unit's rustc flags when it recompiles that unit, so a
    warm shared target directory would make the flag proof vacuous. The proof
    therefore compiles into its own fresh directory instead of touching the
    repository's shared target directory.
    """
    base = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not base or not os.path.isdir(base):
        base = None
        for candidate in ("/private/tmp", tempfile.gettempdir()):
            if os.path.isdir(candidate):
                base = candidate
                break
    return tempfile.mkdtemp(prefix="oxide-profile-proof.", dir=base)


def run_cargo(profile: str, target_dir: str) -> str:
    argv = [
        "cargo",
        "test",
        "--locked",
        "--profile",
        profile,
        "--test",
        PROBE_TEST,
        "-v",
    ]
    # Verify the profile's own settings: ambient one-shot overrides such as
    # CARGO_INCREMENTAL=0 must not stand in for the profile under test.
    env = dict(os.environ)
    for key in (
        "CARGO_INCREMENTAL",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ):
        env.pop(key, None)
    for key in [key for key in env if key.startswith("CARGO_PROFILE_")]:
        env.pop(key, None)
    env["CARGO_TARGET_DIR"] = target_dir
    process = subprocess.Popen(
        argv,
        cwd=REPO_ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    try:
        stdout, _ = process.communicate(timeout=CARGO_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        fail("cargo test --profile %s exceeded %d seconds" % (profile, CARGO_TIMEOUT_SECONDS))
    output = stdout[:OUTPUT_CAP_BYTES].decode("utf-8", "replace")
    if process.returncode:
        fail(
            "cargo test --profile %s --test %s failed with exit %d:\n%s"
            % (profile, PROBE_TEST, process.returncode, output[-8192:])
        )
    return output


def rustc_units(output: str) -> list:
    units = []
    for line in output.splitlines():
        match = re.search(r"Running `([^`]*)`", line)
        if not match:
            continue
        argv = shlex.split(match.group(1))
        if not argv or os.path.basename(argv[0]) != "rustc":
            continue
        crate = None
        source = None
        crate_types = []
        flags = {}
        index = 1
        while index < len(argv):
            arg = argv[index]
            if arg == "--crate-name" and index + 1 < len(argv):
                crate = argv[index + 1]
                index += 2
                continue
            if arg == "--crate-type" and index + 1 < len(argv):
                crate_types.append(argv[index + 1])
                index += 2
                continue
            if arg == "-C" and index + 1 < len(argv):
                raw = argv[index + 1]
                index += 2
            elif arg.startswith("-C") and len(arg) > 2:
                raw = arg[2:]
                index += 1
            else:
                if not arg.startswith("-") and arg.endswith(".rs") and source is None:
                    source = arg
                index += 1
                continue
            key, _, value = raw.partition("=")
            flags[key] = value if value else True
        if crate is None or source is None:
            continue
        units.append(
            {
                "crate": crate,
                "source": os.path.abspath(os.path.join(REPO_ROOT, source)),
                "crate_types": crate_types,
                "flags": flags,
            }
        )
    return units


def classify(unit: dict) -> str:
    if "proc-macro" in unit["crate_types"] or unit["crate"] == "build_script_build":
        return "build_time"
    if unit["source"] == REPO_ROOT or unit["source"].startswith(REPO_ROOT + os.sep):
        return "workspace"
    return "dependency"


def debug_encoding(unit: dict):
    debuginfo = unit["flags"].get("debuginfo")
    if debuginfo is not None:
        return debuginfo
    if unit["flags"].get("strip") == "debuginfo":
        return None
    return "missing"


def verify_units(profile: str, units: list) -> dict:
    expected = EXPECTED_DEBUG[profile]
    seen = {"workspace": [], "dependency": [], "build_time": []}
    for unit in units:
        seen[classify(unit)].append(unit)
    for unit_class, members in seen.items():
        if not members:
            fail("no %s unit was compiled; the check would be vacuous" % unit_class)
        want = expected[unit_class]
        for unit in members:
            got = debug_encoding(unit)
            if got != want:
                fail(
                    "%s unit %s resolved debug=%r, expected %r (flags: %r)"
                    % (unit_class, unit["crate"], got, want, unit["flags"])
                )
            if unit_class == "workspace":
                if unit["flags"].get("panic") == "abort":
                    fail("workspace unit %s was compiled with panic=abort" % unit["crate"])
                if "incremental" not in unit["flags"]:
                    fail("workspace unit %s has no incremental setting; the iterative profile must keep it on" % unit["crate"])
                if "strip" in unit["flags"]:
                    fail("workspace unit %s was compiled with strip=%r" % (unit["crate"], unit["flags"]["strip"]))
                opt_level = unit["flags"].get("opt-level")
                if opt_level not in (None, "0"):
                    fail("workspace unit %s resolved opt-level=%r, expected 0" % (unit["crate"], opt_level))
                for flag in ("debug-assertions", "overflow-checks"):
                    if unit["flags"].get(flag) in ("off", "no", "false"):
                        fail("workspace unit %s disabled %s" % (unit["crate"], flag))
    return {name: sorted({unit["crate"] for unit in members}) for name, members in seen.items()}


def verify_probe_ran(output: str) -> None:
    if ("test %s ... ok" % PROBE_TEST_NAME) not in output:
        fail("the profile probe test did not report success:\n%s" % output[-4096:])
    if "test result: ok. 1 passed" not in output:
        fail("the profile probe test did not execute exactly once")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True, choices=sorted(EXPECTED_DEBUG))
    args = parser.parse_args()

    check_manifest_unchanged()
    target_dir = proof_target_dir()
    try:
        output = run_cargo(args.profile, target_dir)
    finally:
        shutil.rmtree(target_dir, ignore_errors=True)
    verify_probe_ran(output)
    units = rustc_units(output)
    classes = verify_units(args.profile, units)
    print("profile %s verified" % args.profile)
    for unit_class in ("workspace", "dependency", "build_time"):
        print("  %s units: %s" % (unit_class, ", ".join(classes[unit_class])))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as error:
        print("profile check failed: %s" % error, file=sys.stderr)
        sys.exit(1)
