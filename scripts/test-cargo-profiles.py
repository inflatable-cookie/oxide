"""Profile proofs: the opt-in Cargo profiles (oxide#068) and Oxide's recipes (oxide#106).

Two parts in one session, both checking the rustc flags Cargo actually
printed for workspace, normal dependency and build-time (build script / proc
macro) units. A profile name or an inheritance statement is not accepted as
proof.

1. Opt-in profile: plain Cargo runs the workspace probe
   (`tests/profile_backtrace.rs`) under `oxide-light` or `oxide-full-debug`
   in a fresh private target directory. The probe proves workspace panic
   frames keep file:line and that panics still unwind. The script refuses if
   the default dev/test/release profiles or the pinned toolchain changed.
2. Oxide recipes: stages this clean HEAD's `oxide` offline with 1.97.1, then
   on a tiny offline fixture (bin with a build script, path dependency,
   proc macro, the same probe as an integration test, a harness-less bench)
   with a private HOME, CARGO_HOME and OXIDE_POOL proves, for the session's
   primary Oxide profile (`quick` for `oxide-light`, `debug` for
   `oxide-full-debug`) and the other one:
   - `oxide build` resolves the recipe through Cargo's `dev` profile and puts
     the binary in `target/debug/`, never `target/oxide-quick|debug/`;
   - `oxide test` builds the probe under `test`, which inherits the recipe,
     and the probe passes;
   - the other profile takes a distinct slot of the same checkout, both
     slots keep intermediates under their own `build/debug/`, and the latest
     build wins `target/debug/app` (a warm repeat restores the first bytes);
   - `oxide run` runs that binary;
   - an external subcommand's own `$CARGO build`, given no profile flag,
     inherits the quick recipe even when the caller's environment sets other
     `dev` debug values;
   - the deprecated `oxide-quick` / `oxide-debug` definitions still build,
     into their own `target/oxide-*` directories;
   - light session: `oxide bench` keeps Cargo's release-based bench profile
     and `oxide build --release` the release profile; full-debug session: an
     explicit `oxide bench --profile debug` builds the bench under `dev`.

Bounds: 300 s and 512 MiB owned scratch including cleanup; the 20 GiB host
reserve and owned scratch are sampled at phase boundaries only. It runs
under `scripts/bounded-test.py`. No download, install or live pool. Not a
benchmark and not full QA. Scratch is removed only after positive completion.

Usage: python3 scripts/test-cargo-profiles.py --profile oxide-light
"""

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time

BUILDER = "1.97.1"
PROOF_SECONDS = 300
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
MIB = 1024 * 1024
STARTED = time.monotonic()
PEAK = {"bytes": 0, "phase": "none"}
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROBE_TEST = "profile_backtrace"
PROBE_TEST_NAME = "profile_probe_keeps_workspace_frames_and_unwind"
OUTPUT_CAP_BYTES = 8 * 1024 * 1024

# Expected resolved debug encoding per unit class for the opt-in profiles.
# `None` means "no debug information at all" (Cargo emits
# `-C strip=debuginfo` for an explicit `debug = false`).
EXPECTED_DEBUG = {
    "oxide-light": {"workspace": "line-tables-only", "dependency": None, "build_time": None},
    "oxide-full-debug": {"workspace": "2", "dependency": "2", "build_time": "2"},
}

# Oxide's recipes on Cargo's `dev` profile. "none" is no `-C debuginfo` or
# `debuginfo=0`: Cargo's own `dev` build-override default.
RECIPES = {
    "quick": {"workspace": "line-tables-only", "dependency": "line-tables-only", "build_time": "none"},
    "debug": {"workspace": "2", "dependency": "2", "build_time": "2"},
}
SESSIONS = {"oxide-light": ("quick", "debug"), "oxide-full-debug": ("debug", "quick")}
FIXTURE_CLASSES = {
    "app": "workspace",
    "probe": "workspace",
    "b": "workspace",
    "dep": "dependency",
    "mac": "build_time",
    "build_script_build": "build_time",
}

FIXTURE = {
    "Cargo.toml": """[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
dep = { path = "dep" }
mac = { path = "mac" }

[[bench]]
name = "b"
harness = false

[workspace]
""",
    "rust-toolchain.toml": '[toolchain]\nchannel = "%s"\n' % BUILDER,
    "build.rs": 'fn main() {\n    println!("cargo:rerun-if-changed=build.rs");\n}\n',
    "src/main.rs": 'fn main() {\n    println!("{} {}", mac::marker!(), dep::value());\n}\n',
    "benches/b.rs": 'fn main() {\n    println!("bench-ran debug_assertions={}", cfg!(debug_assertions));\n}\n',
    "dep/Cargo.toml": '[package]\nname = "dep"\nversion = "0.1.0"\nedition = "2021"\n',
    "dep/src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    "mac/Cargo.toml": '[package]\nname = "mac"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\nproc-macro = true\n',
    "mac/src/lib.rs": """use proc_macro::TokenStream;

#[proc_macro]
pub fn marker(_input: TokenStream) -> TokenStream {
    "\\"app-ran\\"".parse().unwrap()
}
""",
}

# Cargo runs an external subcommand as `cargo-probe probe [args]` with $CARGO.
EXTERNAL = """#!/bin/sh
shift
echo "probe-env dev_debug=${CARGO_PROFILE_DEV_DEBUG-unset} build_override=${CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG-unset} args=$*"
if [ "$1" = "--cargo-profile" ]; then
    exec "$CARGO" build -v --offline --profile "$2"
fi
exec "$CARGO" build -v --offline
"""


def fail(message: str) -> None:
    raise RuntimeError(message)


def say(message: str) -> None:
    print("profiles %6.1fs %s" % (time.monotonic() - STARTED, message), flush=True)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail("proof exceeded %d s" % PROOF_SECONDS)
    return left


def reserve(path: str, phase: str) -> None:
    stat = os.statvfs(path)
    free = stat.f_bavail * stat.f_frsize
    say("reserve %s: host free %d MiB" % (phase, free // MIB))
    if free < HOST_RESERVE_BYTES:
        fail("host free %d is below the 20 GiB reserve at %s" % (free, phase))


def owned(parent: str, phase: str) -> None:
    total = 0
    seen = set()
    for current, dirs, files in os.walk(parent, followlinks=False):
        for name in dirs + files:
            stat = os.lstat(os.path.join(current, name))
            if (stat.st_dev, stat.st_ino) not in seen:
                seen.add((stat.st_dev, stat.st_ino))
                total += stat.st_blocks * 512
    if total > PEAK["bytes"]:
        PEAK.update(bytes=total, phase=phase)
    say("owned scratch %s: %d MiB" % (phase, total // MIB))
    if total > SCRATCH_LIMIT_BYTES:
        fail("owned scratch %d exceeds 512 MiB at %s" % (total, phase))


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def run(argv: list, cwd: str, env: dict, label: str) -> str:
    """Run one owned child to completion; its output, or a failure."""
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        stdout, _ = process.communicate(timeout=min(150, remaining()))
    except subprocess.TimeoutExpired:
        # `oxide` forwards TERM to Cargo's group and releases after it closes.
        process.terminate()
        try:
            process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
        fail("%s did not finish in time" % label)
    output = stdout[:OUTPUT_CAP_BYTES].decode("utf-8", "replace")
    say("%s: exit %d" % (label, process.returncode))
    if process.returncode:
        fail("%s failed with exit %d:\n%s" % (label, process.returncode, output[-6000:]))
    return output


def check_manifest_unchanged() -> None:
    with open(os.path.join(REPO_ROOT, "Cargo.toml")) as handle:
        manifest = handle.read()
    for name in ("dev", "test", "release"):
        if re.search(r"^\[profile\.%s\]" % re.escape(name), manifest, re.MULTILINE):
            fail("default %s profile was overridden; opt-in profiles must not change it" % name)
    for profile in EXPECTED_DEBUG:
        if not re.search(r"^\[profile\.%s\]" % re.escape(profile), manifest, re.MULTILINE):
            fail("expected opt-in profile %r is missing" % profile)
    with open(os.path.join(REPO_ROOT, "rust-toolchain.toml")) as handle:
        if 'channel = "%s"' % BUILDER not in handle.read():
            fail("pinned stable toolchain channel changed")


def rustc_units(output: str, root: str) -> list:
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
                "source": os.path.abspath(os.path.join(root, source)),
                "crate_types": crate_types,
                "flags": flags,
            }
        )
    return units


# Part 1: the opt-in profiles, unchanged in substance since oxide#068.


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


def check_workspace_dev(unit: dict) -> None:
    """Dev behaviour an iterative recipe keeps for a workspace unit."""
    flags = unit["flags"]
    if flags.get("panic") == "abort":
        fail("workspace unit %s was compiled with panic=abort" % unit["crate"])
    if "incremental" not in flags:
        fail("workspace unit %s has no incremental setting; the iterative profile must keep it on" % unit["crate"])
    if "strip" in flags:
        fail("workspace unit %s was compiled with strip=%r" % (unit["crate"], flags["strip"]))
    if flags.get("opt-level") not in (None, "0"):
        fail("workspace unit %s resolved opt-level=%r, expected 0" % (unit["crate"], flags.get("opt-level")))
    for flag in ("debug-assertions", "overflow-checks"):
        if flags.get(flag) in ("off", "no", "false"):
            fail("workspace unit %s disabled %s" % (unit["crate"], flag))


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
                check_workspace_dev(unit)
    return {name: sorted({unit["crate"] for unit in members}) for name, members in seen.items()}


def verify_probe_ran(output: str, label: str) -> None:
    if ("test %s ... ok" % PROBE_TEST_NAME) not in output:
        fail("%s: the profile probe test did not report success:\n%s" % (label, output[-4096:]))
    if "test result: ok. 1 passed" not in output:
        fail("%s: the profile probe test did not execute exactly once" % label)


def prove_opt_in(profile: str, parent: str, env: dict) -> None:
    # Cargo prints a unit's rustc flags only when it compiles it, so the proof
    # builds into its own fresh target directory.
    target = os.path.join(parent, "opt-in-target")
    output = run(
        ["cargo", "test", "--locked", "--offline", "--profile", profile, "--test", PROBE_TEST, "-v"],
        REPO_ROOT,
        {**env, "CARGO_TARGET_DIR": target, "CARGO_BUILD_BUILD_DIR": target},
        "opt-in cargo test --profile %s" % profile,
    )
    verify_probe_ran(output, profile)
    classes = verify_units(profile, rustc_units(output, REPO_ROOT))
    say("opt-in profile %s verified" % profile)
    for unit_class in ("workspace", "dependency", "build_time"):
        say("  %s units: %s" % (unit_class, ", ".join(classes[unit_class])))
    owned(parent, "after opt-in profile")
    shutil.rmtree(target)


# Part 2: Oxide's recipes.


def stage(parent: str, env: dict) -> str:
    target = os.path.join(parent, "stage")
    run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
        REPO_ROOT,
        {**env, "CARGO_TARGET_DIR": target, "CARGO_BUILD_BUILD_DIR": target, "CARGO_PROFILE_DEV_DEBUG": "0"},
        "stage oxide",
    )
    out = os.path.join(parent, "bin", "oxide")
    shutil.copy2(os.path.join(target, "debug", "oxide"), out)
    say("staged %s: sha256 %s" % (out, sha256(out)))
    owned(parent, "after staging")
    shutil.rmtree(target)
    return out


def write_fixture(root: str) -> None:
    with open(os.path.join(REPO_ROOT, "tests", "profile_backtrace.rs")) as handle:
        probe = handle.read()
    marker = 'const PROBE_FILE: &str = "tests/profile_backtrace.rs";'
    if marker not in probe:
        fail("the workspace probe no longer names its file as expected")
    files = dict(FIXTURE)
    files["tests/probe.rs"] = probe.replace(marker, 'const PROBE_FILE: &str = "tests/probe.rs";')
    for relative, text in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)


def level(unit: dict) -> str:
    value = unit["flags"].get("debuginfo")
    return "none" if value in (None, "0") else value


def by_class(units: list, label: str) -> dict:
    seen = {"workspace": [], "dependency": [], "build_time": []}
    for unit in units:
        unit_class = FIXTURE_CLASSES.get(unit["crate"])
        if unit_class is None:
            fail("%s: unexpected unit %s" % (label, unit["crate"]))
        seen[unit_class].append(unit)
    return seen


def report(label: str, units: list) -> None:
    for unit in units:
        flags = unit["flags"]
        say(
            "  %s unit %s: debuginfo=%s opt-level=%s strip=%s incremental=%s"
            % (label, unit["crate"], flags.get("debuginfo", "-"), flags.get("opt-level", "-"),
               flags.get("strip", "-"), "yes" if "incremental" in flags else "no")
        )


def verify_recipe(output: str, root: str, recipe: str, label: str, classes, crates=()) -> None:
    units = rustc_units(output, root)
    report(label, units)
    seen = by_class(units, label)
    for unit_class in classes:
        if not seen[unit_class]:
            fail("%s: no %s unit was compiled; the check would be vacuous" % (label, unit_class))
    for crate in crates:
        if crate not in {unit["crate"] for unit in units}:
            fail("%s: unit %s was not compiled" % (label, crate))
    for unit_class, members in seen.items():
        for unit in members:
            if level(unit) != RECIPES[recipe][unit_class]:
                fail("%s: %s unit %s resolved debuginfo %s, expected the %s recipe's %s (flags %r)"
                     % (label, unit_class, unit["crate"], level(unit), recipe,
                        RECIPES[recipe][unit_class], unit["flags"]))
            if unit_class == "workspace":
                check_workspace_dev(unit)


def verify_optimised(output: str, root: str, label: str, crates: list) -> None:
    units = rustc_units(output, root)
    report(label, units)
    seen = by_class(units, label)
    names = {unit["crate"] for unit in units}
    for crate in crates:
        if crate not in names:
            fail("%s: unit %s was not compiled" % (label, crate))
    for unit in seen["workspace"] + seen["dependency"]:
        if unit["flags"].get("opt-level") != "3" or level(unit) != "none":
            fail("%s: unit %s is not release-shaped: %r" % (label, unit["crate"], unit["flags"]))


class Oxide:
    def __init__(self, parent: str, binary: str, env: dict):
        self.binary = binary
        self.pool = os.path.join(parent, "pool")
        self.env = {**env, "OXIDE_POOL": self.pool}

    def call(self, cwd: str, args: list, extra: dict = None) -> str:
        return run([self.binary, *args], cwd, {**self.env, **(extra or {})}, "oxide " + " ".join(args))

    def slots(self) -> dict:
        with open(os.path.join(self.pool, "ledger.json")) as handle:
            return {str(slot["id"]): slot for slot in json.load(handle)["slots"]}


def summary(output: str, label: str, profile: str) -> tuple:
    match = re.search(r"oxide: (\w+) profile, [\w+]+ strategy, slot (\d+) (warm|cold)", output)
    if not match:
        fail("%s: no summary line in %r" % (label, output[-2000:]))
    if match.group(1) != profile:
        fail("%s: ran as the %s profile, expected %s" % (label, match.group(1), profile))
    return match.group(2), match.group(3)


def binary(checkout: str, directory: str) -> str:
    path = os.path.join(checkout, "target", directory, "app")
    if not os.path.isfile(path):
        fail("no final binary at %s" % path)
    return sha256(path)


def prove_checkout(oxide: Oxide, q: str, primary: str, other: str) -> None:
    flag = [] if primary == "quick" else ["--profile", primary]
    other_flag = [] if other == "quick" else ["--profile", other]
    all_classes = ("workspace", "dependency", "build_time")

    output = oxide.call(q, ["build", "-v", *flag])
    first_slot, warmth = summary(output, "primary build", primary)
    if warmth != "cold":
        fail("the first %s build was not a new slot" % primary)
    verify_recipe(output, q, primary, "%s build" % primary, all_classes, ("app", "dep", "mac", "build_script_build"))
    first = binary(q, "debug")
    for legacy in ("oxide-quick", "oxide-debug"):
        if os.path.exists(os.path.join(q, "target", legacy)):
            fail("an Oxide %s build wrote target/%s" % (primary, legacy))
    say("%s build: slot %s, target/debug/app sha256 %s" % (primary, first_slot, first))

    output = oxide.call(q, ["test", "-v", *flag, "--test", "probe"])
    slot, _ = summary(output, "primary test", primary)
    if slot != first_slot:
        fail("%s test took slot %s, not the checkout's %s slot %s" % (primary, slot, primary, first_slot))
    verify_probe_ran(output, "%s test" % primary)
    verify_recipe(output, q, primary, "%s test" % primary, ("workspace",), ("probe",))

    output = oxide.call(q, ["build", "-v", *other_flag])
    other_slot, warmth = summary(output, "other build", other)
    if other_slot == first_slot or warmth != "cold":
        fail("the %s build reused slot %s (%s) instead of a distinct new slot" % (other, other_slot, warmth))
    verify_recipe(output, q, other, "%s build" % other, all_classes, ("app", "dep", "mac", "build_script_build"))
    second = binary(q, "debug")
    if second == first:
        fail("target/debug/app did not change to the %s build" % other)
    say("%s build: distinct slot %s, target/debug/app now sha256 %s" % (other, other_slot, second))

    output = oxide.call(q, ["build", *flag])
    slot, warmth = summary(output, "warm repeat", primary)
    if slot != first_slot or warmth != "warm":
        fail("the %s repeat took slot %s %s, expected warm slot %s" % (primary, slot, warmth, first_slot))
    if binary(q, "debug") != first:
        fail("the warm %s repeat did not restore its own target/debug/app" % primary)
    say("%s repeat: warm slot %s, latest build wins target/debug/app" % (primary, slot))

    output = oxide.call(q, ["run", *flag])
    if "app-ran 7" not in output:
        fail("oxide run did not run the binary: %s" % output[-2000:])

    for slot in (first_slot, other_slot):
        build = os.path.join(oxide.pool, "payload", "slots", slot, "build")
        if not os.path.isdir(os.path.join(build, "debug", "deps")):
            fail("slot %s has no build/debug/deps" % slot)
        for legacy in ("oxide-quick", "oxide-debug"):
            if os.path.exists(os.path.join(build, legacy)):
                fail("slot %s holds build/%s from an Oxide profile build" % (slot, legacy))
    say("intermediates: slots %s (%s) and %s (%s) each under build/debug" % (first_slot, primary, other_slot, other))

    if primary == "debug":
        output = oxide.call(q, ["bench", "-v", "--profile", "debug", "--bench", "b"])
        slot, _ = summary(output, "debug bench", "debug")
        if slot != first_slot:
            fail("the debug bench took slot %s, not %s" % (slot, first_slot))
        verify_recipe(output, q, "debug", "debug bench", ("workspace",), ("b",))
        if "bench-ran debug_assertions=true" not in output:
            fail("the debug bench did not run with debug assertions")


def prove_children(oxide: Oxide, x: str, light: bool) -> None:
    # The caller's own dev settings must not stand in for the recipe.
    output = oxide.call(x, ["probe"], {"CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG": "full"})
    if "probe-env dev_debug=line-tables-only build_override=unset args=" not in output:
        fail("the external subcommand did not inherit the quick recipe: %s" % output[-2000:])
    child_slot, _ = summary(output, "external", "quick")
    verify_recipe(output, x, "quick", "external $CARGO build", ("workspace", "dependency", "build_time"))
    binary(x, "debug")
    say("external subcommand: slot %s, its $CARGO build used the quick recipe, target/debug/app" % child_slot)

    for legacy, recipe in (("oxide-quick", "quick"), ("oxide-debug", "debug")):
        output = oxide.call(x, ["probe", "--cargo-profile", legacy])
        verify_recipe(output, x, recipe, "legacy %s" % legacy, ("workspace", "dependency", "build_time"))
        say("legacy %s: target/%s/app sha256 %s" % (legacy, legacy, binary(x, legacy)))

    if light:
        output = oxide.call(x, ["bench", "-v", "--bench", "b"])
        slot, _ = summary(output, "bench", "quick")
        verify_optimised(output, x, "bench", ["b", "dep"])
        if "bench-ran debug_assertions=false" not in output:
            fail("the default bench did not run release-shaped")
        say("bench: slot %s, Cargo's release-based bench profile" % slot)

        output = oxide.call(x, ["build", "-v", "--release"])
        slot, _ = summary(output, "release", "release")
        verify_optimised(output, x, "release", ["app", "dep"])
        say("release: slot %s, target/release/app sha256 %s" % (slot, binary(x, "release")))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True, choices=sorted(EXPECTED_DEBUG))
    args = parser.parse_args()
    if sys.platform != "darwin":
        fail("the profile proof requires macOS")
    check_manifest_unchanged()
    if subprocess.run(["git", "status", "--porcelain"], cwd=REPO_ROOT, capture_output=True,
                      text=True, timeout=30).stdout.strip():
        fail("the checkout is not clean, so HEAD does not name its source")
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO_ROOT, capture_output=True,
                          text=True, timeout=30).stdout.strip()
    reserve("/private/tmp", "admission")
    made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-profile-proof.XXXXXX"],
                          capture_output=True, text=True, timeout=30)
    if made.returncode:
        fail("mktemp failed")
    parent = os.path.realpath(made.stdout.strip())
    os.chmod(parent, 0o700)
    say("source %s, scratch %s, session %s" % (head, parent, args.profile))
    try:
        # Plain Cargo: no inherited wrapper, profile override or placement.
        cargo_env = {name: value for name, value in os.environ.items()
                     if not name.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "CARGO_PROFILE_", "RUSTC",
                                             "SCCACHE", "OXIDE_"))
                     and name not in ("CARGO", "CARGO_INCREMENTAL", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
                                      "RUSTUP_TOOLCHAIN")}
        cargo_env.update({"RUSTUP_TOOLCHAIN": BUILDER, "RUSTC_WRAPPER": "", "CARGO_NET_OFFLINE": "true",
                          "CARGO_TERM_COLOR": "never"})
        prove_opt_in(args.profile, parent, cargo_env)
        reserve(parent, "after opt-in profile")

        os.makedirs(os.path.join(parent, "bin"))
        oxide_binary = stage(parent, cargo_env)
        external = os.path.join(parent, "bin", "cargo-probe")
        with open(external, "w") as handle:
            handle.write(EXTERNAL)
        os.chmod(external, 0o700)
        reserve(parent, "after staging")

        home = os.path.join(parent, "home")
        for path in (home, os.path.join(parent, "cargo-home"), os.path.join(parent, "tmp")):
            os.makedirs(path, mode=0o700)
        env = {name: value for name, value in cargo_env.items()
               if not name.startswith("CARGO_") and name not in ("RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER")}
        env.update({"HOME": home, "CARGO_HOME": os.path.join(parent, "cargo-home"),
                    "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup"),
                    "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never", "RUSTC_WRAPPER": "",
                    "TMPDIR": os.path.join(parent, "tmp"),
                    "PATH": os.path.join(parent, "bin") + os.pathsep + os.environ.get("PATH", "")})
        oxide = Oxide(parent, oxide_binary, env)
        q = os.path.join(parent, "checkouts", "q")
        x = os.path.join(parent, "checkouts", "x")
        write_fixture(q)
        write_fixture(x)
        primary, other = SESSIONS[args.profile]
        prove_checkout(oxide, q, primary, other)
        owned(parent, "after checkout proof")
        reserve(parent, "after checkout proof")
        prove_children(oxide, x, primary == "quick")
        owned(parent, "after child proof")
        reserve(parent, "after child proof")
        states = {slot: record["state"] for slot, record in oxide.slots().items()}
        say("ledger slot states: %s" % states)
        if any(state != "idle" for state in states.values()):
            fail("a slot is not idle after every command exited: %s" % states)
    except Exception:
        print("profile proof retained its owned scratch at %s" % parent, file=sys.stderr)
        raise
    shutil.rmtree(parent)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail("profile proof took %.0f s, over %d s" % (elapsed, PROOF_SECONDS))
    print("profile proof %s held: source %s, %.0f s including cleanup, peak owned scratch sampled %d MiB (%s), "
          "scratch removed" % (args.profile, head, elapsed, PEAK["bytes"] // MIB, PEAK["phase"]))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as error:
        print("profile check failed: %s" % error, file=sys.stderr)
        sys.exit(1)
