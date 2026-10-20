"""Prove that leased `oxide` builds bypass an inherited rustc wrapper (oxide#099).

Stages `oxide` offline with 1.97.1 (an explicit empty RUSTC_WRAPPER, so the
host's wrapper never runs), then on tiny std-only fixtures with a private
HOME, CARGO_HOME, scratch directory pool and targets: a fixture Cargo config
sets `build.rustc-wrapper` to a recording fake wrapper. Plain `cargo check`
invokes it (control); leased `check`, `build`, `test` and an external
subcommand's `$CARGO build` never do, nor do environment-only,
`CARGO_BUILD_RUSTC_WRAPPER` and `--config` wrappers. A recording
`RUSTC_WORKSPACE_WRAPPER` still runs, `oxide clippy` runs the installed
clippy-driver, `OXIDE_ALLOW_RUSTC_WRAPPER=1` restores the wrapper with its
warning, other values are ignored, and summary and `status` lines are
checked, including a Cargo home holding both `config` and `config.toml`
(Cargo reads only `config`). No real sccache, image or download. 300 s
including cleanup, 512 MiB owned scratch. Every child is settled on any
failure after its spawn.

Host reserve: the 20 GiB reserve is sampled at admission and after each
phase. The fixture builds run under Oxide's own host-reserve watchdog (scratch
directory pool). Plain-Cargo staging has no existing host-reserve watchdog
covering it: this script only checks the reserve before and after staging
(observed staging write 68 MiB). That gap is reported, not filled with a
bespoke supervisor. Not a benchmark and not full QA.
"""
from __future__ import annotations

import os
import shutil
import signal
import subprocess
import sys
import time
import traceback

PROOF_SECONDS = 300
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
BUILDER = "1.97.1"
BUILDER_VERSIONS = ("cargo 1.97.1 (c980f4866 2026-06-30)", "rustc 1.97.1 (8bab26f4f 2026-07-14)")
STARTED = time.monotonic()

RECORDER = """#!/bin/sh
# Records one line per rustc invocation, then runs it.
printf '%s\\n' "$*" >> "LOG"
exec "$@"
"""

EXTERNAL = """#!/bin/sh
# An external subcommand: report the inherited wrapper, then build via $CARGO.
printf 'child RUSTC_WRAPPER=[%s]\\n' "${RUSTC_WRAPPER-unset}"
exec "$CARGO" build
"""

FIXTURE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/main.rs": (
        "fn answer() -> i32 {\n    return 42;\n}\n\n"
        "fn main() {\n    println!(\"{}\", answer());\n}\n\n"
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn answers() {\n"
        "        assert_eq!(super::answer(), 42);\n    }\n}\n"
    ),
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail(f"proof exceeded {PROOF_SECONDS} s")
    return left


def reserve(path: str, phase: str) -> None:
    stats = os.statvfs(path)
    free = stats.f_bavail * stats.f_frsize
    print(f"wrapper-bypass reserve {phase}: {free // (1024 ** 3)} GiB free")
    if free < HOST_RESERVE_BYTES:
        fail(f"host free space {free} is below the 20 GiB reserve at {phase}")


def tree_bytes(root: str) -> int:
    total = 0
    for current, _, files in os.walk(root, followlinks=False):
        for name in files:
            try:
                total += os.lstat(os.path.join(current, name)).st_blocks * 512
            except FileNotFoundError:
                pass
    return total


def run(argv: list, cwd: str, env: dict, limit: float = 90) -> tuple:
    """One child in its own session. Every failure after the spawn (deadline,
    timeout, read error, interrupt) settles it before raising: TERM to its
    process group (Oxide forwards TERM to its Cargo group), then KILL. A group
    that still exists afterwards is reported as unclosed, never as closed."""
    timeout = min(limit, remaining())
    child = subprocess.Popen(
        argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, errors="replace", start_new_session=True,
    )
    try:
        stdout, stderr = child.communicate(timeout=timeout)
    except BaseException:
        settle(child, argv)
        raise
    return child.returncode, stdout, stderr


def group_gone(pgid: int) -> bool | None:
    try:
        os.killpg(pgid, 0)
        return False
    except ProcessLookupError:
        return True
    except PermissionError:
        return None


def settle(child: subprocess.Popen, argv: list) -> None:
    """TERM, then KILL, the child's whole process group; each step waits up
    to 10 s for the group to empty, reaping the leader."""
    state = None
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(child.pid, sig)
        except ProcessLookupError:
            pass
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            child.poll()
            state = group_gone(child.pid)
            if state is not False:
                break
            time.sleep(0.1)
        if state is not False:
            break
    child.poll()
    verdict = {True: "settled", False: "UNCLOSED", None: "closure UNKNOWN"}[state]
    print(f"wrapper-bypass: process group {child.pid} of {argv[:3]} {verdict} after a failure", file=sys.stderr)
    print("wrapper-bypass: a Cargo group Oxide started is closed only if Oxide forwarded TERM before KILL", file=sys.stderr)


def lines(path: str) -> list:
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as handle:
        return handle.read().splitlines()


def write(path: str, text: str, mode: int = 0o644) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    os.chmod(path, mode)


def stage(parent: str, repo: str) -> str:
    env = {name: value for name, value in os.environ.items()
           if not name.startswith("SCCACHE_") and name not in (
               "CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC",
               "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER")}
    env.update({
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTC_WRAPPER": "",
        "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_NET_OFFLINE": "true",
    })
    versions = tuple(run([tool, "-V"], repo, env, 30)[1].strip() for tool in ("cargo", "rustc"))
    print(f"wrapper-bypass setup: {versions}")
    if versions != BUILDER_VERSIONS:
        fail(f"builder is not {BUILDER}: {versions}")
    code, _, stderr = run(["cargo", "build", "--locked", "--offline", "--bin", "oxide"], repo, env, 200)
    if code:
        fail(f"staging build failed: {stderr[-2000:]}")
    oxide = os.path.join(parent, "stage-target", "debug", "oxide")
    if not os.access(oxide, os.X_OK):
        fail("staged oxide is missing")
    return oxide


def prove(oxide: str, parent: str) -> None:
    app = os.path.join(parent, "app")
    for relative, text in FIXTURE.items():
        write(os.path.join(app, relative), text)
    bindir = os.path.join(parent, "bin")
    outer_log = os.path.join(parent, "outer.log")
    workspace_log = os.path.join(parent, "workspace.log")
    outer = os.path.join(bindir, "fake-wrapper")
    workspace = os.path.join(bindir, "workspace-wrapper")
    write(outer, RECORDER.replace("LOG", outer_log), 0o755)
    write(workspace, RECORDER.replace("LOG", workspace_log), 0o755)
    write(os.path.join(bindir, "cargo-wrapcheck"), EXTERNAL, 0o755)
    home = os.path.join(parent, "home")
    cargo_home = os.path.join(parent, "cargo-home")
    os.makedirs(home, mode=0o700)
    config = os.path.join(cargo_home, "config.toml")
    write(config, f'[build]\nrustc-wrapper = "{outer}"\n')
    tmp = os.environ["TMPDIR"]
    base = {name: value for name, value in os.environ.items()
            if not (name.startswith(("SCCACHE_", "OXIDE_", "XDG_", "CARGO_"))
                    or name in ("RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN"))}
    base.update({
        "HOME": home,
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup"),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_HOME": cargo_home,
        "CARGO_NET_OFFLINE": "true",
        "CARGO_TERM_COLOR": "never",
        "OXIDE_POOL": os.path.join(parent, "pool"),
        "OXIDE_POOL_CAP": "64MiB",
        "PATH": bindir + os.pathsep + os.environ.get("PATH", ""),
        "OXIDE_TASK_SCRATCH_ROOT": tmp, "TMPDIR": tmp, "TMP": tmp, "TEMP": tmp,
    })
    main_rs = os.path.join(app, "src", "main.rs")

    def touch() -> None:
        now = time.time() + 2
        os.utime(main_rs, (now, now))

    def leased(args: list, label: str, extra: dict | None = None, wanted_outer: int = 0) -> str:
        touch()
        before = len(lines(outer_log))
        code, stdout, stderr = run([oxide, *args], app, {**base, **(extra or {})})
        if code:
            fail(f"{label}: exit {code}\n{stderr[-3000:]}")
        if "Checking app" not in stderr and "Compiling app" not in stderr:
            fail(f"{label}: the fixture crate was not compiled\n{stderr[-2000:]}")
        calls = len(lines(outer_log)) - before
        summary = [line for line in stderr.splitlines() if line.startswith("oxide: ") and " profile, " in line]
        print(f"wrapper-bypass {label}: exit 0, fake wrapper calls {calls}, summary {summary}")
        if (calls == 0) != (wanted_outer == 0):
            fail(f"{label}: fake wrapper calls {calls}, wanted {'some' if wanted_outer else 'none'}")
        if len(summary) != 1:
            fail(f"{label}: expected one summary line, got {summary}")
        return summary[0] + "\n" + stdout + stderr

    # Control: plain Cargo in the same fixture uses the configured wrapper.
    touch()
    code, _, stderr = run(["cargo", "check"], app, {**base, "CARGO_TARGET_DIR": os.path.join(parent, "plain-target")})
    plain = len(lines(outer_log))
    print(f"wrapper-bypass plain cargo check: exit {code}, fake wrapper calls {plain}")
    if code or plain == 0:
        fail(f"plain cargo did not use the fixture wrapper\n{stderr[-2000:]}")

    named = f"rustc wrapper bypassed: {outer}"
    for args in (["check"], ["build"], ["test"]):
        text = leased(args, f"config wrapper, oxide {args[0]}")
        if named not in text:
            fail(f"oxide {args[0]}: summary does not name the bypassed wrapper")
    text = leased(["wrapcheck"], "external subcommand $CARGO build")
    if "child RUSTC_WRAPPER=[]" not in text or named not in text:
        fail("the external subcommand did not inherit the empty wrapper")
    no_config = os.path.join(parent, "cargo-home-empty")
    os.makedirs(no_config)
    for label, args, extra in (
        ("RUSTC_WRAPPER only", ["check"], {"RUSTC_WRAPPER": outer}),
        ("CARGO_BUILD_RUSTC_WRAPPER only", ["check"], {"CARGO_BUILD_RUSTC_WRAPPER": outer}),
        ("--config only", ["check", "--config", f"build.rustc-wrapper='{outer}'"], {}),
    ):
        text = leased(args, label, {"CARGO_HOME": no_config, **extra})
        if named not in text:
            fail(f"{label}: the bypassed wrapper was not named")
    # Both names in one Cargo home: Cargo reads `config` and ignores the
    # `.toml`, whose empty wrapper must not hide the real one.
    dual = os.path.join(parent, "cargo-home-dual")
    write(os.path.join(dual, "config.toml"), '[build]\nrustc-wrapper = ""\n')
    write(os.path.join(dual, "config"), f'[build]\nrustc-wrapper = "{outer}"\n')
    text = leased(["check"], "dual config files, opt-in", {"CARGO_HOME": dual, "OXIDE_ALLOW_RUSTC_WRAPPER": "1"}, wanted_outer=1)
    if f"rustc wrapper active: {outer}, unqualified" not in text:
        fail("dual config files: the opt-in summary does not name the active wrapper")
    text = leased(["check"], "dual config files", {"CARGO_HOME": dual})
    if named not in text:
        fail("dual config files: the bypassed wrapper was not named")
    code, stdout, _ = run([oxide, "status"], app, {**base, "CARGO_HOME": dual})
    if code or f"rustc wrapper: inherited {outer} (from {os.path.join(dual, 'config')}); bypassed" not in stdout:
        fail(f"dual config files: status is wrong\n{stdout}")
    print("wrapper-bypass dual config files status: names the wrapper from config, not config.toml")
    text = leased(["check"], "no inherited wrapper", {"CARGO_HOME": no_config})
    if "rustc wrapper" in text:
        fail("a summary named a wrapper where none is inherited")

    before = len(lines(workspace_log))
    leased(["check"], "workspace wrapper preserved", {"RUSTC_WORKSPACE_WRAPPER": workspace})
    if len(lines(workspace_log)) == before:
        fail("the recording workspace wrapper did not run")

    text = leased(["clippy"], "oxide clippy")
    if "clippy::needless_return" not in text or named not in text:
        fail(f"oxide clippy did not run clippy-driver's lints\n{text[-2000:]}")
    print("wrapper-bypass oxide clippy: clippy::needless_return reported by clippy-driver")

    text = leased(["check"], "opt-in OXIDE_ALLOW_RUSTC_WRAPPER=1", {"OXIDE_ALLOW_RUSTC_WRAPPER": "1"}, wanted_outer=1)
    if f"rustc wrapper active: {outer}, unqualified" not in text:
        fail("the opt-in summary does not warn that the wrapper is active and unqualified")
    text = leased(["check"], "OXIDE_ALLOW_RUSTC_WRAPPER=true ignored", {"OXIDE_ALLOW_RUSTC_WRAPPER": "true"})
    if named not in text or "is not 1, so ignored" not in text:
        fail("a non-1 opt-in value was not reported as ignored")

    code, stdout, _ = run([oxide, "status"], app, base)
    wanted = f"rustc wrapper: inherited {outer} (from {config}); bypassed for oxide builds"
    print(f"wrapper-bypass status: exit {code}, {[line for line in stdout.splitlines() if 'rustc wrapper' in line]}")
    if code or wanted not in stdout:
        fail(f"status does not report the inherited wrapper and its state\n{stdout}")
    code, stdout, _ = run([oxide, "status"], app, {**base, "OXIDE_ALLOW_RUSTC_WRAPPER": "1"})
    if code or f"rustc wrapper: inherited {outer} (from {config}); kept for oxide builds by OXIDE_ALLOW_RUSTC_WRAPPER=1, unqualified" not in stdout:
        fail(f"opt-in status is wrong\n{stdout}")

    # Plain passthrough is untouched: no lease and no summary.
    code, stdout, stderr = run([oxide, "pkgid"], app, base)
    if code or "oxide:" in stderr or "app" not in stdout:
        fail(f"plain passthrough changed: exit {code} {stderr}")
    print(f"wrapper-bypass plain passthrough pkgid: exit 0, no lease; total fake wrapper calls {len(lines(outer_log))}")


def main() -> int:
    if sys.platform != "darwin":
        fail("the wrapper-bypass proof requires macOS")
    reserve("/private/tmp", "admission")
    made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-wrapper.XXXXXX"],
                          capture_output=True, text=True, timeout=30)
    if made.returncode:
        fail("mktemp failed")
    root = os.path.realpath(made.stdout.strip())
    os.chmod(root, 0o700)
    os.environ.update({"OXIDE_TASK_SCRATCH_ROOT": root, "TMPDIR": root, "TMP": root, "TEMP": root})
    parent = os.path.join(root, "proof")
    os.makedirs(parent, mode=0o700)
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    try:
        oxide = stage(parent, repo)
        staged = tree_bytes(root)
        print(f"wrapper-bypass staging: {time.monotonic() - STARTED:.0f} s, {staged // (1024 * 1024)} MiB")
        if staged > SCRATCH_LIMIT_BYTES:
            fail(f"staging {staged} exceeds 512 MiB")
        reserve(root, "after staging")
        prove(oxide, parent)
        reserve(root, "after fixtures")
        owned = tree_bytes(root)
        print(f"wrapper-bypass owned scratch: {owned // (1024 * 1024)} MiB")
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB")
    except Exception:
        print(f"wrapper-bypass proof retained its owned scratch at {root}", file=sys.stderr)
        raise
    shutil.rmtree(root)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail(f"proof took {elapsed:.0f} s, over {PROOF_SECONDS} s")
    print(f"oxide wrapper-bypass proof held: {elapsed:.0f} s including cleanup, owned scratch {owned // (1024 * 1024)} MiB, scratch removed")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
