#!/usr/bin/env python3
"""Focused ordinary-CLI proof for oxide#130.

A short fixed script under the qualified shared proof supervisor
(`scripts/bounded-test.py`, SHA-256
d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e), which
owns the 300 s deadline, 512 MiB monitored scratch, process closure and
cleanup. It reuses the reviewed #104/#127 staging helper and guarded child
runner and adds no supervision of its own.

Inside owned scratch it stages this HEAD's debug `oxide` offline with 1.97.1,
once unstamped and once with the installer's `OXIDE_BUILD_PIN` /
`OXIDE_BUILD_INSTANCE` stamp (the second build reuses the same target and
recompiles only the binary). With a recording, refusing fake `cargo` it then
proves:

- `--version`/`-V` print Oxide's identity, an honest pin (`unknown` for an
  unstamped build) and, for a receipt-matching private install, verified
  provenance with the receipt digest kept separate from the binary digest;
  Cargo still owns stdout order, arguments, cwd, environment and exit code,
  and `--list` stays a direct pass.
- `init` readiness acknowledges `:cargo` tasks and `-cargo` suites once per
  invocation, separately from actionable plain-Cargo selectors, and stays
  honest about a malformed manifest; nothing is persisted or edited.
- `deny`, `audit`, `outdated` and `machete` forward unchanged, without a pool.
- a pool-creation reserve refusal and a reserve stop report bounded, read-only
  capacity diagnostics (typed, error, timeout, oversized answer and a real
  positive control) while the raw 20 GiB refusal text and decision stay.

No image, mount, real registration, install, network or host state is
involved: the pool creation refusal is reached through the debug-only
free-space injection. Not a benchmark and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import importlib.util
import json
import os
import shutil
import signal
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("pool_layout", os.path.join(HERE, "test-oxide-pool-layout.py"))
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)
Proof = layout.Proof
expect, fail = layout.expect, layout.fail
work_left = layout.work_left

BUILDER = "1.97.1"
PROOF_SECONDS = 300
STARTED = time.monotonic()
PIN_VARIABLE = "OXIDE_BUILD_PIN"
INSTANCE_VARIABLE = "OXIDE_BUILD_INSTANCE"
CANDIDATE_MARKER = b"__oxide-capacity-probe"
GIB = 1024 * 1024 * 1024
results = []
ENV: dict = {}
FAKE = """#!/bin/sh
{
  printf 'argv=%s\\n' "$*"
  printf 'cwd=%s\\n' "$PWD"
  printf 'pool=%s\\n' "${OXIDE_POOL-unset}"
  printf 'target=%s\\n' "${CARGO_TARGET_DIR-unset}"
} >> "${FAKE_CARGO_LOG:?}"
printf 'FAKE-CARGO-STDOUT %s\\n' "$*"
printf 'FAKE-CARGO-STDERR\\n' >&2
exit "${FAKE_CARGO_EXIT:-7}"
"""
EFFIGY = """[tasks]
"rust:compile:cargo" = "cargo check --workspace --locked"
"rust:compile" = "cargo check --workspace --locked"
"legacy" = "cargo build"
# "commented" = "cargo test"
[test.suites.rust-cargo]
run = "cargo test --workspace"
[test.suites.rust]
run = "cargo test --workspace"
"""


def write(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def read(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def digest(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def manifest(directory: str, name: str) -> None:
    write(os.path.join(directory, "Cargo.toml"), f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n')
    write(os.path.join(directory, "src", "lib.rs"), "pub fn value() -> u32 {\n    7\n}\n")


def git(directory: str) -> None:
    os.makedirs(directory, exist_ok=True)
    env = {"PATH": os.environ["PATH"], "HOME": ENV["HOME"], "GIT_CONFIG_NOSYSTEM": "1"}
    result = subprocess.run(["git", "init", "-q"], cwd=directory, env=env, capture_output=True, timeout=30)
    expect(result.returncode == 0, f"git init failed in {directory}")


def environment(root: str) -> dict:
    real_home = os.path.expanduser("~")
    scratch = os.environ["OXIDE_TASK_SCRATCH_ROOT"]
    env = {
        "PATH": os.environ["PATH"],
        "HOME": os.path.join(root, "home"),
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "CARGO_HOME": os.path.join(root, "cargo-home"),
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", os.path.join(real_home, ".rustup")),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "OXIDE_POOL": os.path.join(root, "pool-absent"),
        "OXIDE_TASK_SCRATCH_ROOT": scratch,
        "TMPDIR": scratch,
        "TMP": scratch,
        "TEMP": scratch,
        "LANG": "C",
    }
    for name in ("HOME", "CARGO_HOME"):
        os.makedirs(env[name], exist_ok=True)
    return env


def case(name: str, condition: bool, detail: str = "") -> None:
    results.append((name, condition))
    print(("PASS " if condition else "FAIL ") + name + (f": {detail}" if detail and not condition else ""), flush=True)
    expect(condition, f"{name} {detail}")


def run(proof: Proof, binary: str, args: list, cwd: str, env: dict | None = None, timeout: float = 120) -> tuple:
    result = proof.run([binary, *args], cwd=cwd, env=env or ENV, timeout=timeout)
    return (
        result.returncode,
        result.stdout.decode(errors="replace"),
        result.stderr.decode(errors="replace"),
    )


def stage_root(proof: Proof) -> str:
    root = os.path.join(proof.root, "stage")
    os.makedirs(root)
    return root


def cargo_build(proof: Proof, source: str, label: str, root: str, stamp: tuple | None) -> str:
    """Build `oxide` from `source` into `root`, optionally stamped, and copy
    the executable to `root/bin/<label>`."""
    stage = os.path.join(root, f"run-{label}")
    os.makedirs(stage)
    env = dict(os.environ)
    for name in list(env):
        if name.startswith(("SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_", PIN_VARIABLE, INSTANCE_VARIABLE)):
            env.pop(name)
        if name in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            env.pop(name)
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(root, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(root, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_INCREMENTAL": "0",
        }
    )
    if stamp:
        env[PIN_VARIABLE], env[INSTANCE_VARIABLE] = stamp
    started = time.monotonic()
    result = proof.run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide", "--message-format=json-render-diagnostics"],
        cwd=source,
        env=env,
        timeout=200,
    )
    print(result.stderr.decode(errors="replace")[-3000:], flush=True)
    print(f"staged {label} in {time.monotonic() - started:.0f} s", flush=True)
    expect(result.returncode == 0, f"{label} staging exit {result.returncode}")
    built = None
    for line in result.stdout.decode(errors="replace").splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "oxide" and message.get("executable"):
            built = message["executable"]
    expect(built is not None and os.path.realpath(built).startswith(root + "/"), f"{label} executable {built} is not in {root}")
    copy = os.path.join(root, "bin", label)
    os.makedirs(os.path.dirname(copy), exist_ok=True)
    shutil.copy2(built, copy)
    shutil.rmtree(stage)
    return copy


def fake_cargo(proof: Proof, label: str) -> str:
    directory = os.path.join(proof.root, "fake", label)
    path = os.path.join(directory, "cargo")
    write(path, FAKE)
    os.chmod(path, 0o755)
    return directory


def recorded(path: str) -> dict:
    values = {}
    for line in read(path).splitlines():
        key, _, value = line.partition("=")
        values.setdefault(key, []).append(value)
    return {key: value[-1] for key, value in values.items()}


def identity(proof: Proof, root: str) -> None:
    """`--version` and `-V` identity, then Cargo's own version."""
    work = os.path.join(root, "identity")
    os.makedirs(work)
    fake = fake_cargo(proof, "identity")
    log = os.path.join(work, "cargo.log")
    env = {**ENV, "PATH": fake + os.pathsep + ENV["PATH"], "FAKE_CARGO_LOG": log}
    for args, label in ((["--version"], "long"), (["-V"], "short")):
        code, out, err = run(proof, proof.binary, args, work, env)
        lines = out.splitlines()
        identity_lines = [line for line in lines if line.startswith(("oxide ", "  build:", "  provenance:"))]
        cargo_lines = [line for line in lines if line.startswith("FAKE-CARGO-STDOUT")]
        case(f"unstamped --version runs caller Cargo with its exit code ({label})", code == 7 and cargo_lines == [f"FAKE-CARGO-STDOUT {' '.join(args)}"], f"{code} {out} {err}")
        ordered = bool(cargo_lines) and len(identity_lines) == 3 and lines.index(identity_lines[-1]) < lines.index(cargo_lines[0])
        case(f"unstamped --version prints identity before Cargo's line ({label})", ordered, out)
        case(f"unstamped --version says pin unknown ({label})", "source pin unknown" in out and "provenance: unknown" in out and "verified" not in out, out)
        case(f"unstamped --version keeps Cargo's stderr ({label})", "FAKE-CARGO-STDERR" in err, err)
        case(f"unstamped --version passes the arguments unchanged ({label})", recorded(log)["argv"] == " ".join(args), read(log))
    # `--list` stays a direct pass with no identity lines.
    code, out, _ = run(proof, proof.binary, ["--list"], work, env)
    case("--list is a direct Cargo pass", code == 7 and out.startswith("FAKE-CARGO-STDOUT --list") and "oxide " not in out, out)


def install(stamp: tuple, bytes_path: str, root: str) -> tuple:
    """A private HOME with the stamped binary installed and its receipt."""
    home = os.path.join(root, "home")
    destination = os.path.join(home, ".local/bin/oxide")
    receipts = os.path.join(home, ".local/state/oxide/install")
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    os.makedirs(receipts, exist_ok=True)
    shutil.copy2(bytes_path, destination)
    os.chmod(destination, 0o755)
    pin, instance = stamp
    receipt = {
        "pin": pin,
        "build_instance": instance,
        "installed": {"path": destination, "sha256": digest(destination), "size": os.path.getsize(destination)},
    }
    receipt_path = os.path.join(receipts, "20260101T000000Z-aaaaaaaaaaaa.json")
    write(receipt_path, json.dumps(receipt, indent=2) + "\n")
    os.chmod(receipt_path, 0o600)
    return home, destination, receipt_path


def provenance(proof: Proof, stamp: tuple, unstamped: str, root: str) -> None:
    """Verified provenance, then every near miss as `unverified`."""
    work = os.path.join(root, "provenance")
    os.makedirs(work)
    home, destination, receipt_path = install(stamp, proof.stamped, root)
    fake = fake_cargo(proof, "provenance")
    log = os.path.join(work, "cargo.log")
    env = {**ENV, "OXIDE_TEST_HOME_FROM_ENV": "1", "HOME": home, "PATH": fake + os.pathsep + ENV["PATH"], "FAKE_CARGO_LOG": log}
    code, out, _ = run(proof, destination, ["--version"], work, env)
    receipt_digest = digest(receipt_path)
    pin, instance = stamp
    case(
        "stamped --version names its source pin and instance",
        code == 7 and f"source pin {pin}" in out and f"installer instance {instance}" in out,
        out,
    )
    case(
        "a receipt-matching install is verified with both digests kept apart",
        "provenance: verified install receipt 20260101T000000Z-aaaaaaaaaaaa.json" in out
        and f"installed binary sha256 {digest(destination)}" in out
        and f"receipt sha256 {receipt_digest}" in out
        and receipt_digest != digest(destination),
        out,
    )
    # A same-pin replacement at the same path is never authenticated: the
    # bytes no longer match the receipt, so path and hash alone prove nothing.
    replacement = os.path.join(work, "replacement")
    with open(proof.stamped, "rb") as source, open(replacement, "wb") as target:
        shutil.copyfileobj(source, target)
        target.write(b"\0")
    shutil.copy2(replacement, destination)
    os.chmod(destination, 0o755)
    _, out, _ = run(proof, destination, ["--version"], work, env)
    case(
        "a same-pin replacement is unverified, never the checkout HEAD",
        f"source pin {pin}" in out
        and "provenance: unverified" in out
        and "size differs" in out
        and "verified install receipt" not in out,
        out,
    )
    # An unstamped build stays unknown even when it sits at the installed path.
    shutil.copy2(unstamped, destination)
    os.chmod(destination, 0o755)
    _, out, _ = run(proof, destination, ["--version"], work, env)
    case("an unstamped build at the installed path stays unknown", "provenance: unknown" in out and "verified" not in out, out)
    # A rebuilt binary with the same pin but another instance is unverified.
    shutil.copy2(proof.stamped, destination)
    os.chmod(destination, 0o755)
    write(receipt_path, json.dumps({**json.loads(read(receipt_path)), "build_instance": "0" * 32}, indent=2) + "\n")
    os.chmod(receipt_path, 0o600)
    _, out, _ = run(proof, destination, ["--version"], work, env)
    case("another build instance is unverified", "provenance: unverified" in out and "build instance" in out, out)
    # A historical, non-private receipt is read but not authenticated.
    write(receipt_path, json.dumps({"pin": pin, "build_instance": instance, "installed": {"path": destination, "sha256": digest(destination), "size": os.path.getsize(destination)}}, indent=2) + "\n")
    os.chmod(receipt_path, 0o644)
    _, out, _ = run(proof, destination, ["--version"], work, env)
    case("a non-private receipt is unverified", "provenance: unverified" in out and "owner-only" in out, out)
    # No receipt at all.
    os.remove(receipt_path)
    _, out, _ = run(proof, destination, ["--version"], work, env)
    case("a missing receipt is unverified", "provenance: unverified" in out, out)


def readiness(proof: Proof, root: str) -> None:
    """`:cargo` escapes acknowledged once per run, warnings kept separate."""
    work = os.path.join(root, "readiness")
    os.makedirs(work)
    project = os.path.join(work, "project")
    git(project)
    manifest(project, "readiness")
    write(os.path.join(project, "effigy.toml"), EFFIGY)
    effigy_before = digest(os.path.join(project, "effigy.toml"))
    code, out, err = run(proof, proof.binary, ["init"], project)
    text = out + err
    acknowledged = [line for line in text.splitlines() if "acknowledged once for this run" in line]
    case("init succeeds on a manifest with escapes", code == 0, text)
    case(
        "escapes are acknowledged once with both selector forms",
        len(acknowledged) == 1
        and "2 intentional plain-cargo escape selector(s)" in acknowledged[0]
        and "tasks.rust:compile:cargo" in acknowledged[0]
        and "test.suites.rust-cargo" in acknowledged[0],
        text,
    )
    warnings = [line for line in text.splitlines() if line.startswith("    ")]
    case(
        "actionable selectors are still reported",
        any("tasks.rust:compile:" in line for line in warnings)
        and any("tasks.legacy:" in line for line in warnings)
        and any("test.suites.rust:" in line for line in warnings)
        and not any("rust:compile:cargo" in line for line in warnings)
        and not any("rust-cargo" in line for line in warnings),
        text,
    )
    case(
        "a comment is not a plain-cargo route",
        "commented" not in text,
        text,
    )
    _, out, _ = run(proof, proof.binary, ["init"], project)
    case("the acknowledgment is per invocation, not persisted", len([line for line in out.splitlines() if "acknowledged once for this run" in line]) == 1, out)
    case("readiness never edits the manifest", digest(os.path.join(project, "effigy.toml")) == effigy_before)
    # A malformed manifest stays honest instead of guessing a group.
    broken = os.path.join(work, "broken")
    git(broken)
    manifest(broken, "broken")
    write(os.path.join(broken, "effigy.toml"), '[tasks\n"x:cargo" = "cargo check"\n')
    code, out, err = run(proof, proof.binary, ["init"], broken)
    text = out + err
    case(
        "a malformed effigy.toml is reported unclassified",
        code == 0 and "is not parseable as TOML" in text and "unclassified" in text and "acknowledged once" not in text,
        text,
    )


def passthrough(proof: Proof, root: str) -> None:
    """The four scanner tools forward unchanged, with no pool."""
    work = os.path.join(root, "passthrough")
    os.makedirs(work)
    project = os.path.join(work, "project")
    manifest(project, "passthrough")
    fake = fake_cargo(proof, "passthrough")
    pool = os.path.join(work, "passthrough-pool")
    for name in ("deny", "audit", "outdated", "machete"):
        log = os.path.join(work, f"{name}.log")
        env = {
            **ENV,
            "PATH": fake + os.pathsep + ENV["PATH"],
            "FAKE_CARGO_LOG": log,
            "OXIDE_POOL": pool,
            "CARGO_TARGET_DIR": "/sentinel/target",
            "OXIDE_ALLOW_RUSTC_WRAPPER": "1",
        }
        code, out, err = run(proof, proof.binary, [name, "--some-flag", "value"], project, env)
        seen = recorded(log)
        case(
            f"{name} forwards unchanged with Cargo's exit code",
            code == 7
            and seen.get("argv") == f"{name} --some-flag value"
            and seen.get("cwd") == project
            and seen.get("pool") == pool
            and seen.get("target") == "/sentinel/target"
            and out.startswith(f"FAKE-CARGO-STDOUT {name} --some-flag value")
            and "FAKE-CARGO-STDERR" in err,
            f"{code} {seen} {out} {err}",
        )
        case(f"{name} leased nothing and created no pool", not os.path.exists(pool), pool)


def capacity(proof: Proof, root: str) -> None:
    """Bounded diagnostics on the refusal and the reserve-stop report."""
    work = os.path.join(root, "capacity")
    os.makedirs(work)
    project = os.path.join(work, "project")
    manifest(project, "capacity")
    refused = os.path.join(work, "refused-pool")
    base = {
        **ENV,
        "OXIDE_POOL": refused,
        "OXIDE_TEST_POOL_FREE_BYTES": "1024",
    }
    for injection, expected, label in (
        ("ok:1073741824:2147483648", "important use 1.0 GiB", "typed"),
        ("ok:none:none", "important use unknown", "unknown values"),
        ("error:unsupported", "does not report the property", "error class"),
        ("hang", "did not answer within 2 s; it was killed and reaped", "timeout"),
        ("flood", "combined output exceeded its 8 KiB bound", "oversized"),
    ):
        env = {**base, "OXIDE_TEST_CAPACITY_INJECT": injection}
        code, out, err = run(proof, proof.binary, ["check"], project, env)
        text = out + err
        case(
            f"a creation refusal reports {label} capacity diagnostics",
            code != 0
            and "refusing to create the pool: host free space 0 MiB is below the 20 GiB reserve" in text
            and "capacity diagnostics:" in text
            and expected in text,
            text,
        )
        case(
            f"the raw reserve decision is unchanged with {label}",
            "raw reserve decision is unchanged" in text or "not guaranteed free space" in text,
            text,
        )
        case(f"no pool was created for {label}", not os.path.exists(refused), refused)
    # Real positive control: the executing artifact answers for a real path.
    code, out, err = run(proof, proof.binary, ["check"], project, base)
    text = out + err
    case(
        "the real capacity probe answers or says unknown",
        code != 0 and ("macOS volume resource values" in text or "capacity diagnostics: unknown" in text),
        text,
    )
    # A reserve stop reports diagnostics after the writers closed.
    pool = os.path.join(work, "stopped-pool")
    readings = os.path.join(work, "readings")
    write(readings, str(22 * GIB - 4096))
    env = {
        **ENV,
        "OXIDE_POOL": pool,
        "OXIDE_TEST_WATCHDOG_READINGS": readings,
        "OXIDE_TEST_CAPACITY_INJECT": "ok:536870912:none",
    }
    code, out, err = run(proof, proof.binary, ["check"], project, env)
    text = out + err
    case(
        "a reserve stop appends the capacity diagnostic after closure",
        code != 0
        and "reserve stop" in text
        and "capacity diagnostics:" in text
        and "important use 0.5 GiB" in text,
        text,
    )


def account_home(proof: Proof, root: str) -> None:
    """The installed default: the pool and config follow the account home, not
    HOME. A controlled account home stands in for the real one, so nothing
    here can reach the operator's pool."""
    # Handshake first: the staged artifact must report compiled proof controls
    # before any storage-reading call relies on them.
    for binary in (proof.binary, proof.stamped):
        probe = subprocess.run([binary, "__oxide-build-capabilities"], capture_output=True, text=True, timeout=10)
        expect(probe.returncode == 0 and json.loads(probe.stdout).get("test_controls") is True,
               f"{binary} does not report compiled proof controls: {probe.stdout!r}")
    account = os.path.realpath(os.path.join(root, "account-home"))
    harness = os.path.join(root, "harness-home")
    for path in (account, harness):
        os.makedirs(path, exist_ok=True)
    base = {k: v for k, v in ENV.items() if k not in ("OXIDE_TEST_HOME_FROM_ENV", "OXIDE_POOL")}
    env = {**base, "HOME": harness, "OXIDE_TEST_ACCOUNT_HOME": account}
    code, out, err = run(proof, proof.binary, ["status"], root, env=env, timeout=60)
    case("an overridden HOME resolves config from the account home", code == 0
         and f"config: {account}/Library/Application Support/oxide/config/config.toml" in out, out + err)
    case("an overridden HOME resolves the default pool from the account home", code == 0
         and f"pool: {account}/Library/Caches/oxide/build-pool" in out, out)
    case("a differing HOME prints the account-home notice", code == 0
         and f"home: HOME is {harness}; Oxide's pool and config use the account home {account}" in out, out)
    case("nothing is created under the harness HOME", not os.path.exists(os.path.join(harness, "Library")))
    explicit = os.path.join(root, "explicit-pool")
    code, out, err = run(proof, proof.binary, ["status"], root, env={**env, "OXIDE_POOL": explicit}, timeout=60)
    case("an explicit OXIDE_POOL still wins and silences the notice", code == 0
         and f"pool: {explicit} (from OXIDE_POOL)" in out and "home: HOME is" not in out, out + err)
    link = os.path.join(root, "account-link")
    os.symlink(account, link)
    for label, target in (("a symlinked account control", link), ("an account control outside scratch", "/Users")):
        code, out, err = run(proof, proof.binary, ["status"], root, env={**env, "OXIDE_TEST_ACCOUNT_HOME": target}, timeout=60)
        case(f"{label} refuses", code != 0 and "OXIDE_TEST_ACCOUNT_HOME refused" in err, out + err)
    code, out, err = run(proof, proof.binary, ["status"], root, env={**env, "OXIDE_TEST_HOME_FROM_ENV": "1"}, timeout=60)
    case("mixed proof controls refuse", code != 0 and "are both set" in err, out + err)
    code, out, err = run(proof, proof.binary, ["status"], root, env={**env, "OXIDE_TEST_ACCOUNT_HOME": "!unavailable"}, timeout=60)
    case("an unavailable account home refuses instead of using HOME", code != 0
         and "the account home directory is unavailable" in err and harness not in out, out + err)
    case("the refusal created nothing under the harness HOME", not os.path.exists(os.path.join(harness, "Library")))


def main() -> int:
    if sys.platform != "darwin":
        fail("the everyday CLI proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the runner's scratch")
    expect(os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ, "not under the qualified runner")
    root = tempfile.mkdtemp(prefix="everyday-cli.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, lambda signum, frame: sys.exit(128 + signum))
    global ENV
    ENV = environment(root)
    proof = Proof(root)
    proof.env = ENV
    # The pin the installer would authenticate: this proof's own HEAD commit.
    head = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(len(head) == 40, f"HEAD {head!r} is not a commit")
    instance = "0123456789abcdef0123456789abcdef"
    stamp = (head, instance)
    try:
        stage = stage_root(proof)
        proof.binary = cargo_build(proof, repo, "unstamped", stage, None)
        with open(proof.binary, "rb") as handle:
            expect(CANDIDATE_MARKER in handle.read(), "the staged binary lacks the capacity probe")
        proof.stamped = cargo_build(proof, repo, "stamped", stage, stamp)
        unstamped = digest(proof.binary)
        stamped = digest(proof.stamped)
        expect(unstamped != stamped, "the stamped build is not a different binary")
        shutil.rmtree(os.path.join(stage, "target"))
        shutil.rmtree(os.path.join(stage, "build"))
        proof.phase("after staging")
        identity(proof, root)
        provenance(proof, stamp, proof.binary, root)
        readiness(proof, root)
        passthrough(proof, root)
        capacity(proof, root)
        account_home(proof, root)
    except BaseException as failure:
        print(f"everyday CLI proof failed after {time.monotonic() - STARTED:.0f} s: {failure!r}; scratch {root} left to the runner", file=sys.stderr)
        raise
    passed = sum(1 for _, ok in results if ok)
    elapsed = time.monotonic() - STARTED
    print(f"everyday CLI proof: {passed}/{len(results)} cases passed in {elapsed:.0f} s; scratch closure and cleanup are the shared runner's")
    expect(elapsed <= PROOF_SECONDS, f"proof exceeded {PROOF_SECONDS} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
