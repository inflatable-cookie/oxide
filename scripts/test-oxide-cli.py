"""Build the unpublished `oxide` front door and prove it on tiny fixtures.

Usage refusals always run without a pool. On macOS, after a 20 GiB host
reserve, the script provisions exactly one fresh 128 MiB APFS pool through
`oxide-storage-lease init` (one 128 MiB pool, 512 MiB scratch, 300 s total
proof phase including setup) and points OXIDE_POOL at it; the default pool is
never touched. Offline std-only fixture crates prove command routing, the
quick/debug profile flags (including an external subcommand's `$CARGO`
call), storage-override refusals, `clean`, checkout-affine reuse, FIFO
waiting and the host limit, clean waiting for active commands, `default-run`
selection (package and virtual workspace roots), Ctrl-C forwarding, own-command crash recovery,
`status` stray detection and Cargo-equal exit codes. The image is detached
and the scratch removed only after positive completion. Before any pool
exists, `init` / `new` adoption runs on tiny scratch directories (90 s,
64 MiB scratch) and must leave the pool uncreated. Not a benchmark and not
full QA.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time

IMAGE_BYTES = 128 * 1024 * 1024
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
SOURCE_LIMIT_BYTES = 8 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
PROOF_SECONDS = 300
ADOPTION_SECONDS = 90
ADOPTION_SCRATCH_BYTES = 64 * 1024 * 1024
BEGIN = "<!-- oxide:begin -->"
END = "<!-- oxide:end -->"
# Set after the oxide binaries compile; ordinary compiler artifacts are
# outside the proof-phase bound.
STARTED = 0.0

FIXTURE = {
    "Cargo.toml": """[package]
name = "app"
version = "0.1.0"
edition = "2021"
default-run = "app"

[dependencies]
helper = { path = "helper" }
""",
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    ".gitignore": "/target\n",
    "build.rs": "fn main() {\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    "src/main.rs": """fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("app {} {}", helper::double(21), args.join(" "));
    if let Some(code) = args.first().and_then(|arg| arg.strip_prefix("exit:")) {
        std::process::exit(code.parse().expect("exit code"));
    }
}
""",
    "src/bin/other.rs": "fn main() {\n    println!(\"other {}\", helper::double(1));\n}\n",
    "tests/basic.rs": """use std::path::PathBuf;
use std::time::Duration;

#[test]
fn passes() {
    assert_eq!(helper::double(2), 4);
}

#[test]
fn optional_failure() {
    assert!(std::env::var_os("OXIDE_FIXTURE_FAIL").is_none(), "requested failure");
}

#[test]
fn optional_hold() {
    let Some(pidfile) = std::env::var_os("OXIDE_FIXTURE_HOLD").map(PathBuf::from) else {
        return;
    };
    let temp = pidfile.with_extension("tmp");
    std::fs::write(&temp, std::process::id().to_string()).expect("pid");
    std::fs::rename(&temp, &pidfile).expect("pid rename");
    let release = pidfile.with_extension("release");
    for _ in 0..600 {
        if release.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("hold timed out");
}
""",
    "helper/Cargo.toml": """[package]
name = "helper"
version = "0.1.0"
edition = "2021"
""",
    "helper/src/lib.rs": "pub fn double(value: u32) -> u32 {\n    value * 2\n}\n",
}

# Virtual workspace: `default-members = ["app"]`, whose `default-run` picks
# `app` over its second binary; `tool` is another member with a binary.
VIRTUAL = {
    "Cargo.toml": '[workspace]\nmembers = ["app", "tool"]\ndefault-members = ["app"]\nresolver = "2"\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "app/Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\ndefault-run = "app"\n',
    "app/src/main.rs": 'fn main() {\n    println!("virtual app");\n}\n',
    "app/src/bin/other.rs": 'fn main() {\n    println!("virtual other");\n}\n',
    "tool/Cargo.toml": '[package]\nname = "tool"\nversion = "0.1.0"\nedition = "2021"\n',
    "tool/src/main.rs": 'fn main() {\n    println!("virtual tool");\n}\n',
}

# External subcommand: reports the injected build dir and runs `$CARGO`.
PROBE = """#!/bin/sh
# Cargo passes the subcommand name first.
echo "BUILD_DIR=$CARGO_BUILD_BUILD_DIR"
exec "$CARGO" check --profile "$2" -v
"""


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail(f"proof phase exceeded {PROOF_SECONDS} s")
    return left


def scratch_root() -> str:
    raw = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if raw:
        path = os.path.realpath(raw)
        if not os.path.isdir(path):
            fail("OXIDE_TASK_SCRATCH_ROOT must be an existing directory")
        return path
    # Heavy admission does not forward the outer environment; provision one
    # fresh canonical private scratch directory instead.
    result = subprocess.run(
        ["mktemp", "-d", "/private/tmp/oxide-cli.XXXXXX"],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode:
        fail("mktemp scratch failed")
    path = os.path.realpath(result.stdout.strip())
    os.chmod(path, 0o700)
    return path


def host_free_bytes(path: str) -> int:
    result = subprocess.run(["/bin/df", "-k", path], capture_output=True, text=True, timeout=30)
    if result.returncode:
        fail(f"df preflight failed: {result.stderr}")
    line = [line for line in result.stdout.splitlines() if line.strip()][-1]
    return int(line.split()[3]) * 1024


def build_binaries() -> dict:
    build = subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--bin",
            "oxide",
            "--bin",
            "oxide-storage-lease",
            "--message-format=json-render-diagnostics",
        ],
        stdout=subprocess.PIPE,
        text=True,
        timeout=300,
    )
    if build.returncode:
        sys.exit(build.returncode)
    executables = {}
    for line in build.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        name = message.get("target", {}).get("name")
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            executables[name] = message["executable"]
    for name in ("oxide", "oxide-storage-lease"):
        if name not in executables:
            fail(f"compiled artifact {name} was not reported")
    return executables


class Runner:
    def __init__(self, oxide: str, lease: str, parent: str):
        self.oxide = oxide
        self.lease = lease
        self.parent = parent
        self.pool = os.path.join(parent, "pool")
        self.bin = os.path.join(parent, "bin")
        self.tmp = os.path.join(parent, "tmp")
        for path in (self.bin, self.tmp):
            os.makedirs(path, mode=0o700, exist_ok=True)
        env = dict(os.environ)
        for name in (
            "CARGO",
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTUP_TOOLCHAIN",
            "OXIDE_MAX_ACTIVE",
        ):
            env.pop(name, None)
        env.update(
            {
                "OXIDE_POOL": self.pool,
                "PATH": self.bin + os.pathsep + env.get("PATH", ""),
                "CARGO_NET_OFFLINE": "true",
                "CARGO_TERM_COLOR": "never",
                "TMPDIR": self.tmp,
                "TMP": self.tmp,
                "TEMP": self.tmp,
            }
        )
        self.env = env

    def environment(self, extra: dict | None) -> dict:
        env = dict(self.env)
        env.update(extra or {})
        return env

    def run(self, cwd: str, args: list, extra: dict | None = None, program=None):
        result = subprocess.run(
            [program or self.oxide, *args],
            cwd=cwd,
            env=self.environment(extra),
            capture_output=True,
            text=True,
            timeout=min(120, remaining()),
        )
        return result.returncode, result.stdout, result.stderr

    def start(self, cwd: str, args: list, label: str, extra: dict | None = None):
        log = open(os.path.join(self.parent, f"{label}.stderr"), "w+", encoding="utf-8")
        child = subprocess.Popen(
            [self.oxide, *args],
            cwd=cwd,
            env=self.environment(extra),
            stdout=subprocess.DEVNULL,
            stderr=log,
            text=True,
        )
        return child, log

    def lease_status(self) -> dict:
        code, stdout, stderr = self.run(
            self.parent, ["--pool", self.pool, "status", "--json"], program=self.lease
        )
        if code != 0:
            fail(f"standalone lease status exit {code}: {stderr}")
        return json.loads(stdout)

    def state(self) -> dict:
        with open(os.path.join(self.pool, "oxide-managed", "state.json"), encoding="utf-8") as handle:
            return json.load(handle)


def expect(code: int, wanted: int, label: str, stderr: str = "") -> None:
    if code != wanted:
        fail(f"{label}: exit {code}, expected {wanted}; stderr tail: {stderr[-2000:]}")


def summary(stderr: str, label: str) -> dict:
    match = re.search(
        r"oxide: (\w+) profile, ([\w+]+) strategy, slot (\d+) (warm|cold), ", stderr
    )
    if not match:
        fail(f"{label}: no summary line in {stderr[-2000:]!r}")
    return {
        "profile": match.group(1),
        "strategy": match.group(2),
        "slot": match.group(3),
        "warmth": match.group(4),
    }


def debuginfo(stderr: str) -> dict:
    """Map crate name to its `-C debuginfo` value from `cargo -v` output."""
    units = {}
    for line in stderr.splitlines():
        if "Running `" not in line or "--crate-name" not in line:
            continue
        crate = re.search(r"--crate-name (\S+)", line).group(1)
        value = re.search(r"-C debuginfo=(\S+)", line)
        if re.search(r"-C strip=debuginfo", line) and not value:
            units[crate] = None
        else:
            units[crate] = value.group(1) if value else "missing"
    return units


def wait_for(predicate, label: str, seconds: float = 60) -> None:
    deadline = time.monotonic() + min(seconds, remaining())
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    fail(f"timed out waiting for {label}")


def read_log(log) -> str:
    log.flush()
    log.seek(0)
    return log.read()


def write_fixture(root: str, files: dict = FIXTURE) -> None:
    for relative, text in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)


def git(cwd: str, *args: str) -> None:
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=oxide fixture",
            "-c",
            "user.email=fixture@oxide.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            *args,
        ],
        cwd=cwd,
        check=True,
        capture_output=True,
        timeout=60,
    )


def tree_bytes(root: str, skip_mounts: bool) -> int:
    total = 0
    for current, dirs, files in os.walk(root, followlinks=False):
        if skip_mounts:
            dirs[:] = [name for name in dirs if not os.path.ismount(os.path.join(current, name))]
        for name in files:
            path = os.path.join(current, name)
            if os.path.islink(path):
                continue
            metadata = os.lstat(path)
            allocated = metadata.st_blocks * 512
            # The sparse backing counts by allocation, everything else by
            # the larger of size and allocation.
            total += allocated if name == "image.sparseimage" else max(metadata.st_size, allocated)
    return total


def prove_usage(runner: Runner, cwd: str) -> None:
    code, stdout, _ = runner.run(cwd, ["--help"])
    expect(code, 0, "help")
    if "oxide status" not in stdout:
        fail("help omitted status")
    code, stdout, _ = runner.run(cwd, ["version"])
    expect(code, 0, "plain version passthrough")
    if not stdout.startswith("cargo "):
        fail(f"version passthrough printed {stdout!r}")
    for args in (
        ["build", "--target-dir", "elsewhere"],
        ["test", "--target-dir=elsewhere"],
        ["build", "--config", "build.build-dir='elsewhere'"],
        ["check", "--config=build.target-dir=\"elsewhere\""],
        ["nextest", "run", "--config", "extra.toml"],
        ["clean", "--target-dir", "elsewhere"],
        ["+nightly", "build"],
        ["-v", "build"],
    ):
        code, _, stderr = runner.run(cwd, args)
        expect(code, 1, f"refusal {args}", stderr)
        if "refused" not in stderr and "after the command" not in stderr:
            fail(f"refusal {args} gave no explanation: {stderr!r}")
    code, stdout, _ = runner.run(cwd, ["status"])
    expect(code, 0, "status without a pool")
    if "not created" not in stdout:
        fail(f"status without a pool: {stdout!r}")
    if os.path.lexists(runner.pool):
        fail("usage, refusals or status created the pool")


def snapshot(root: str) -> dict:
    """Relative path to bytes for every file under root."""
    files = {}
    for current, _, names in os.walk(root):
        for name in names:
            path = os.path.join(current, name)
            with open(path, "rb") as handle:
                files[os.path.relpath(path, root)] = handle.read()
    return files


def read_text(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def managed_block(text: str) -> str:
    start = text.find(BEGIN + "\n")
    end = text.find(END + "\n")
    if start < 0 or end < start or text.count(BEGIN) != 1 or text.count(END) != 1:
        fail(f"no single managed block in {text!r}")
    block = text[start : end + len(END) + 1]
    if "`oxide` wherever you would write `cargo`" not in block:
        fail(f"block lacks the command rule: {block!r}")
    return block


def prove_adoption(runner: Runner, root: str) -> None:
    """`init` / `new` adopt in one command: Cargo only when needed, one
    marked AGENTS.md block at the workspace root, readiness printed and no
    pool created."""
    started = time.monotonic()
    os.makedirs(root, mode=0o700)
    # rustup reads this for every fixture below: the pinned toolchain.
    with open(os.path.join(root, "rust-toolchain.toml"), "w", encoding="utf-8") as handle:
        handle.write('[toolchain]\nchannel = "1.97.1"\n')

    # Empty directory: Cargo package plus block; Cargo.toml is Cargo's own.
    code, stdout, stderr = runner.run(root, ["init", "--vcs", "none", "--name", "demo", "empty"])
    expect(code, 0, "init empty", stderr)
    plain = subprocess.run(
        ["cargo", "init", "--vcs", "none", "--name", "demo", "plain"],
        cwd=root,
        env=runner.env,
        capture_output=True,
        text=True,
        timeout=min(60, remaining()),
    )
    expect(plain.returncode, 0, "plain cargo init", plain.stderr)
    empty = os.path.join(root, "empty")
    adopted = snapshot(empty)
    block = managed_block(adopted.pop("AGENTS.md").decode())
    if adopted != snapshot(os.path.join(root, "plain")) or read_text(os.path.join(empty, "AGENTS.md")) != block:
        fail("init in an empty directory wrote more than Cargo's package and the block")
    for needle in (
        f"created the Oxide block in {empty}/AGENTS.md",
        "toolchain: rustc 1.97.1",
        "identity: the path",
        f"pool: {runner.pool} not created",
        "effigy: no effigy.toml",
    ):
        if needle not in stdout:
            fail(f"init readiness missing {needle!r}: {stdout}")

    # Re-run: byte-identical tree, file not rewritten.
    agents = os.path.join(empty, "AGENTS.md")
    before, mtime = snapshot(empty), os.stat(agents).st_mtime_ns
    code, stdout, stderr = runner.run(empty, ["init"])
    expect(code, 0, "init re-run", stderr)
    if snapshot(empty) != before or os.stat(agents).st_mtime_ns != mtime or "already current" not in stdout:
        fail(f"init re-run changed the tree: {stdout}")

    # Existing package: block only; Cargo-only arguments refuse untouched.
    existing = os.path.join(root, "existing")
    write_fixture(
        existing,
        {
            "Cargo.toml": '[package]\nname = "existing"\nversion = "0.1.0"\nedition = "2021"\n',
            "src/lib.rs": "pub fn value() -> u32 {\n    1\n}\n",
            "AGENTS.md": "# Existing guidance\n\nKeep this line.",
            "effigy.toml": '[tasks]\n"rust:compile" = "cargo check --workspace"\n"rust:test" = "oxide test"\n# "old" = "cargo build"\n',
        },
    )
    git(existing, "init", "--quiet")
    git(existing, "add", "-A")
    git(existing, "commit", "--quiet", "-m", "fixture")
    before = snapshot(existing)
    for args in (["init", "--lib"], ["init", "--name", "other", "."], ["init", "--vcs=git"]):
        code, _, stderr = runner.run(existing, args)
        expect(code, 1, f"refusal {args}", stderr)
        if "Refused Cargo-only arguments" not in stderr:
            fail(f"{args} refusal gave no explanation: {stderr!r}")
    if snapshot(existing) != before:
        fail("a refused init changed the existing package")
    code, stdout, stderr = runner.run(existing, ["init"])
    expect(code, 0, "init existing", stderr)
    after = snapshot(existing)
    text = after.pop("AGENTS.md").decode()
    del before["AGENTS.md"]
    if after != before or text != "# Existing guidance\n\nKeep this line.\n\n" + managed_block(text):
        fail(f"init on an existing package changed more than the appended block: {text!r}")
    for needle in (
        "added the Oxide block to",
        "identity: Git root commit",
        "still calls plain cargo (reported, not edited",
        'line 2: "rust:compile" = "cargo check --workspace"',
    ):
        if needle not in stdout:
            fail(f"existing-package readiness missing {needle!r}: {stdout}")
    if "line 3" in stdout or "line 4" in stdout:
        fail(f"effigy report named an oxide selector or a comment: {stdout}")

    # Text outside the markers is preserved; a stale block is refreshed.
    stale = "# Head\r\nintro\n" + BEGIN + "\nstale words\n" + END + "\nafter the block\n"
    with open(os.path.join(existing, "AGENTS.md"), "w", encoding="utf-8", newline="") as handle:
        handle.write(stale)
    code, stdout, stderr = runner.run(existing, ["init"])
    expect(code, 0, "init refresh", stderr)
    text = read_text(os.path.join(existing, "AGENTS.md"))
    with open(os.path.join(existing, "AGENTS.md"), "rb") as handle:
        raw = handle.read().decode()
    if "refreshed the Oxide block" not in stdout or raw != "# Head\r\nintro\n" + managed_block(text) + "after the block\n":
        fail(f"refresh changed text outside the markers: {raw!r}")
    broken = raw + BEGIN + "\n"
    with open(os.path.join(existing, "AGENTS.md"), "w", encoding="utf-8", newline="") as handle:
        handle.write(broken)
    code, _, stderr = runner.run(existing, ["init"])
    expect(code, 1, "init with malformed markers", stderr)
    with open(os.path.join(existing, "AGENTS.md"), "rb") as handle:
        if handle.read().decode() != broken or "fix the markers by hand" not in stderr:
            fail(f"malformed markers were edited or unexplained: {stderr!r}")

    # A new member inside a workspace: Cargo adds it; the block goes to the
    # workspace root, never the member.
    workspace = os.path.join(root, "ws")
    write_fixture(
        workspace,
        {
            "Cargo.toml": '[workspace]\nmembers = ["core"]\nresolver = "2"\n',
            "core/Cargo.toml": '[package]\nname = "core"\nversion = "0.1.0"\nedition = "2021"\n',
            "core/src/lib.rs": "",
        },
    )
    member = os.path.join(workspace, "member")
    os.makedirs(member)
    code, stdout, stderr = runner.run(member, ["init", "--vcs", "none"])
    expect(code, 0, "init workspace member", stderr)
    if os.path.lexists(os.path.join(member, "AGENTS.md")):
        fail("init wrote the block into the member instead of the workspace root")
    managed_block(read_text(os.path.join(workspace, "AGENTS.md")))
    if '"member"' not in read_text(os.path.join(workspace, "Cargo.toml")):
        fail("Cargo did not add the member to the workspace")
    if f"created the Oxide block in {workspace}/AGENTS.md" not in stdout:
        fail(f"member init reported another location: {stdout}")

    # `new`: Cargo's package then adoption, including inside the workspace;
    # a failing `cargo new` returns Cargo's exit and adopts nothing.
    code, stdout, stderr = runner.run(root, ["new", "--vcs", "none", "--lib", "fresh"])
    expect(code, 0, "new", stderr)
    fresh = os.path.join(root, "fresh")
    if not os.path.isfile(os.path.join(fresh, "src", "lib.rs")):
        fail("new did not create Cargo's library package")
    managed_block(read_text(os.path.join(fresh, "AGENTS.md")))
    before = snapshot(workspace)
    code, stdout, stderr = runner.run(workspace, ["new", "--vcs", "none", "tools"])
    expect(code, 0, "new workspace member", stderr)
    if os.path.lexists(os.path.join(workspace, "tools", "AGENTS.md")):
        fail("new wrote the block into the member")
    if snapshot(workspace)["AGENTS.md"] != before["AGENTS.md"] or "already current" not in stdout:
        fail("new in a workspace did not keep the root block current")
    before = snapshot(fresh)
    code, _, stderr = runner.run(root, ["new", "--vcs", "none", "fresh"])
    expect(code, 101, "new over an existing directory", stderr)
    if snapshot(fresh) != before:
        fail("a failed new changed the existing directory")

    if os.path.lexists(runner.pool):
        fail("adoption created the pool")
    elapsed = time.monotonic() - started
    scratch = tree_bytes(root, False)
    if elapsed > ADOPTION_SECONDS or scratch > ADOPTION_SCRATCH_BYTES:
        fail(f"adoption proof exceeded its bounds: {elapsed:.0f} s, {scratch} bytes")
    print(f"oxide adoption proof held: {elapsed:.0f} s, scratch {scratch // 1024} KiB")


def prove_native(runner: Runner, app: str) -> None:
    code, stdout, stderr = runner.run(
        runner.parent,
        ["--pool", runner.pool, "init", "--cap-bytes", str(IMAGE_BYTES), "--image-bytes", str(IMAGE_BYTES)],
        program=runner.lease,
    )
    expect(code, 0, "pool init", stderr)
    slots_root = os.path.join(runner.pool, "payload", "slots")

    # Modelled build: quick profile flags reach rustc for every unit.
    code, _, stderr = runner.run(app, ["build", "-v"])
    expect(code, 0, "quick build", stderr)
    first = summary(stderr, "quick build")
    if (first["profile"], first["strategy"], first["warmth"]) != ("quick", "build", "cold"):
        fail(f"quick build summary {first}")
    units = debuginfo(stderr)
    for crate in ("app", "helper"):
        if units.get(crate) != "line-tables-only":
            fail(f"quick {crate} debuginfo {units.get(crate)!r}; units {units}")
    if not os.path.isfile(os.path.join(app, "target", "oxide-quick", "app")):
        fail("quick build did not place the binary in the private target dir")
    quick_slot = first["slot"]
    code, _, stderr = runner.run(app, ["build"])
    expect(code, 0, "warm build", stderr)
    if summary(stderr, "warm build") != {**first, "warmth": "warm"}:
        fail(f"second build was not warm on slot {quick_slot}: {stderr[-500:]}")

    # Debug profile: full debug including the build-script unit.
    code, _, stderr = runner.run(app, ["check", "--profile", "debug", "-v"])
    expect(code, 0, "debug check", stderr)
    debug = summary(stderr, "debug check")
    if debug["profile"] != "debug" or debug["warmth"] != "cold" or debug["slot"] == quick_slot:
        fail(f"debug check summary {debug}")
    units = debuginfo(stderr)
    for crate in ("app", "helper", "build_script_build"):
        if units.get(crate) != "2":
            fail(f"debug {crate} debuginfo {units.get(crate)!r}; units {units}")

    prove_routing(runner, app, quick_slot, debug["slot"])


def prove_routing(runner: Runner, app: str, quick_slot: str, debug_slot: str) -> None:
    slots_root = os.path.join(runner.pool, "payload", "slots")
    # Building passthrough: an external subcommand's $CARGO sees the
    # environment-only profile definitions and the injected build dir.
    probe = os.path.join(runner.bin, "cargo-oxprobe")
    with open(probe, "w", encoding="utf-8") as handle:
        handle.write(PROBE)
    os.chmod(probe, 0o700)
    for cargo_profile, expected in (("oxide-quick", "line-tables-only"), ("oxide-debug", "2")):
        code, stdout, stderr = runner.run(app, ["oxprobe", cargo_profile])
        expect(code, 0, f"external {cargo_profile}", stderr)
        if f"BUILD_DIR={slots_root}/{quick_slot}/build" not in stdout:
            fail(f"external subcommand build dir: {stdout!r}")
        units = debuginfo(stderr)
        for crate in ("app", "helper"):
            if units.get(crate) != expected:
                fail(f"external {cargo_profile} {crate} debuginfo {units.get(crate)!r}")
        if summary(stderr, "external")["slot"] != quick_slot:
            fail("external subcommand did not use the quick slot")

    # Test holds the lease through execution; exit codes equal Cargo's.
    code, _, stderr = runner.run(app, ["test"])
    expect(code, 0, "quick test", stderr)
    tested = summary(stderr, "quick test")
    if tested["strategy"] != "build+execute" or tested["slot"] != quick_slot:
        fail(f"quick test summary {tested}")
    failing = {"OXIDE_FIXTURE_FAIL": "1"}
    oxide_code, _, stderr = runner.run(app, ["test"], failing)
    plain = subprocess.run(
        ["cargo", "test"],
        cwd=app,
        env={**{k: v for k, v in runner.env.items() if k != "OXIDE_POOL"}, **failing},
        capture_output=True,
        text=True,
        timeout=min(120, remaining()),
    )
    if oxide_code != plain.returncode or oxide_code != 101:
        fail(f"failing test exit oxide={oxide_code} cargo={plain.returncode}")

    # Run releases before executing; the program's exit code is returned.
    # With two binaries, `default-run` picks `app` as Cargo would.
    code, stdout, stderr = runner.run(app, ["run", "--", "exit:7", "hello"])
    expect(code, 7, "run exit code", stderr)
    if "app 42 exit:7 hello" not in stdout:
        fail(f"run output {stdout!r}")
    code, stdout, stderr = runner.run(app, ["run", "--bin", "other"])
    expect(code, 0, "run --bin other", stderr)
    if stdout.strip() != "other 2":
        fail(f"run --bin other output {stdout!r}")
    if any(slot["state"] != "idle" for slot in runner.lease_status()["slots"]):
        fail("run left a slot held after executing the program")

    # Plain passthrough and refusals take no lease.
    ledger = os.path.join(runner.pool, "ledger.json")
    with open(ledger, "rb") as handle:
        before = handle.read()
    code, stdout, stderr = runner.run(app, ["metadata", "--format-version", "1", "--no-deps"])
    expect(code, 0, "plain metadata", stderr)
    names = [package["name"] for package in json.loads(stdout)["packages"]]
    if "app" not in names or "oxide:" in stderr:
        fail("metadata passthrough output changed")
    for args in (["build", "--target-dir", "x"], ["test", "--config", "build.target-dir='x'"]):
        code, _, stderr = runner.run(app, args)
        expect(code, 1, f"refusal {args}", stderr)
    with open(ledger, "rb") as handle:
        if handle.read() != before:
            fail("plain passthrough or a refusal changed the ledger")

    # Status: pool, slots and the plain-Cargo stray target/.
    code, stdout, stderr = runner.run(app, ["status"])
    expect(code, 0, "status", stderr)
    for needle in (
        f"pool: {runner.pool}",
        f"slot {quick_slot} idle quick {app}",
        f"slot {debug_slot} idle debug {app}",
        "waiting commands 0",
        "holds plain-Cargo deps/ or incremental/",
    ):
        if needle not in stdout:
            fail(f"status missing {needle!r}: {stdout}")

    # Clean removes private outputs and affinity; slots stay intact.
    slots_before = sorted(os.listdir(slots_root))
    code, _, stderr = runner.run(app, ["clean"])
    expect(code, 0, "clean", stderr)
    if os.path.exists(os.path.join(app, "target")):
        fail("clean left the private target dir")
    if sorted(os.listdir(slots_root)) != slots_before or not os.listdir(
        os.path.join(slots_root, quick_slot, "build")
    ):
        fail("clean changed or emptied a slot")
    code, stdout, _ = runner.run(app, ["status"])
    if f"slot {quick_slot} idle quick no checkout affinity" not in stdout:
        fail(f"clean kept affinity: {stdout}")
    code, _, stderr = runner.run(app, ["build"])
    expect(code, 0, "build after clean", stderr)
    rebuilt = summary(stderr, "build after clean")
    if rebuilt["warmth"] != "cold" or "evicted idle slot" not in stderr:
        fail(f"build after clean was not a cold evicting build: {stderr[-800:]}")

    # Checkout affinity: a second checkout of the same repository builds cold
    # on its own slot; each checkout then reuses only its own slot.
    app2 = os.path.join(runner.parent, "checkouts", "app2")
    git(os.path.dirname(app2), "clone", "--quiet", "--no-hardlinks", app, app2)
    code, _, stderr = runner.run(app2, ["build"])
    expect(code, 0, "second checkout build", stderr)
    second = summary(stderr, "second checkout")
    if second["warmth"] != "cold" or second["slot"] == rebuilt["slot"]:
        fail(f"second checkout reused another checkout's slot: {second} vs {rebuilt}")
    if "evicted idle slot" not in stderr:
        fail("second checkout at the project cap did not evict an idle slot")
    for cwd, wanted in ((app, rebuilt), (app2, second)):
        code, _, stderr = runner.run(cwd, ["build"])
        expect(code, 0, "affine rebuild", stderr)
        again = summary(stderr, "affine rebuild")
        if again["slot"] != wanted["slot"] or again["warmth"] != "warm":
            fail(f"checkout {cwd} did not reuse its own slot: {again} vs {wanted}")
    if len(runner.lease_status()["slots"]) != 2:
        fail("project slot cap of 2 was not kept")

    prove_waiting(runner, app, app2)
    prove_clean_waits(runner, app)
    prove_interrupt(runner, app)
    prove_recovery(runner, app)

    # `run` from a virtual workspace root follows Cargo's default members.
    virtual = os.path.join(runner.parent, "checkouts", "virtual")
    write_fixture(virtual, VIRTUAL)
    code, stdout, stderr = runner.run(virtual, ["run"])
    expect(code, 0, "virtual workspace run", stderr)
    if stdout.strip() != "virtual app":
        fail(f"virtual workspace run output {stdout!r}")
    code, stdout, stderr = runner.run(os.path.join(virtual, "tool"), ["run"])
    expect(code, 0, "member run", stderr)
    if stdout.strip() != "virtual tool":
        fail(f"member run output {stdout!r}")

    status = runner.lease_status()
    if status.get("coverage") != "complete" or any(slot["state"] != "idle" for slot in status["slots"]):
        fail(f"final standalone status: {status}")
    code, stdout, _ = runner.run(app, ["status"])
    if "waiting commands 0" not in stdout or "stray Cargo output: none" not in stdout:
        fail(f"final status: {stdout}")


def prove_waiting(runner: Runner, app: str, app2: str) -> None:
    """Host limit 1: a second command waits, a third queues behind it and
    Ctrl-C cancels it; releasing the holder lets the waiter finish."""
    limit = {"OXIDE_MAX_ACTIVE": "1"}
    pidfile = os.path.join(runner.parent, "hold-wait.pid")
    holder, holder_log = runner.start(
        app, ["test"], "hold-wait", {**limit, "OXIDE_FIXTURE_HOLD": pidfile}
    )
    wait_for(lambda: os.path.exists(pidfile), "holding test")
    waiter, waiter_log = runner.start(app2, ["build"], "waiter", limit)
    wait_for(lambda: "waiting for a build slot (1 of 1" in read_log(waiter_log), "host-limit wait line")
    queued, queued_log = runner.start(app2, ["check"], "queued", limit)
    wait_for(
        lambda: "waiting for earlier oxide commands (position 2)" in read_log(queued_log),
        "FIFO wait line",
    )
    if read_log(waiter_log).count("waiting for") != 1:
        fail("waiter printed more than one waiting line")
    queued.send_signal(signal.SIGINT)
    expect(queued.wait(timeout=15), 130, "cancelled waiter", read_log(queued_log))
    if waiter.poll() is not None:
        fail("waiter finished while the host limit was held")
    with open(pidfile.replace(".pid", ".release"), "w", encoding="utf-8"):
        pass
    expect(holder.wait(timeout=min(60, remaining())), 0, "holding test", read_log(holder_log))
    expect(waiter.wait(timeout=min(60, remaining())), 0, "waiter", read_log(waiter_log))
    summary(read_log(waiter_log), "waiter")
    if runner.state()["tickets"]:
        fail("wait tickets remained after the queue drained")
    for log in (holder_log, waiter_log, queued_log):
        log.close()


def prove_clean_waits(runner: Runner, app: str) -> None:
    """Clean waits for an active command in its checkout, then removes the
    private outputs and drops only idle affinity."""
    pidfile = os.path.join(runner.parent, "hold-clean.pid")
    holder, holder_log = runner.start(app, ["test"], "hold-clean", {"OXIDE_FIXTURE_HOLD": pidfile})
    wait_for(lambda: os.path.exists(pidfile), "test held during clean")
    cleaner, clean_log = runner.start(app, ["clean"], "clean-wait")
    wait_for(
        lambda: "waiting for active oxide commands in this checkout" in read_log(clean_log),
        "clean wait line",
    )
    if cleaner.poll() is not None or not os.path.isdir(os.path.join(app, "target")):
        fail("clean ran while a command in its checkout was active")
    if any(record["checkout"] is None for record in runner.state()["records"]):
        fail("clean detached affinity while a command was active")
    with open(pidfile.replace(".pid", ".release"), "w", encoding="utf-8"):
        pass
    expect(holder.wait(timeout=min(60, remaining())), 0, "test held during clean", read_log(holder_log))
    expect(cleaner.wait(timeout=min(60, remaining())), 0, "waiting clean", read_log(clean_log))
    if os.path.exists(os.path.join(app, "target")):
        fail("waiting clean left the private target dir")
    if "dropped this checkout's slot affinity" not in read_log(clean_log):
        fail("waiting clean did not drop the idle affinity")
    if runner.state()["cleans"]:
        fail("clean mark remained after clean finished")
    for log in (holder_log, clean_log):
        log.close()


def process_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def prove_interrupt(runner: Runner, app: str) -> None:
    """Ctrl-C reaches Cargo's whole group, then the slot is released."""
    pidfile = os.path.join(runner.parent, "hold-int.pid")
    child, log = runner.start(app, ["test"], "interrupt", {"OXIDE_FIXTURE_HOLD": pidfile})
    wait_for(lambda: os.path.exists(pidfile), "interruptible test")
    with open(pidfile, encoding="utf-8") as handle:
        test_pid = int(handle.read())
    child.send_signal(signal.SIGINT)
    expect(child.wait(timeout=20), 130, "interrupted test", read_log(log))
    if process_alive(test_pid):
        fail("the test process survived Ctrl-C forwarding")
    if any(slot["state"] != "idle" for slot in runner.lease_status()["slots"]):
        fail("interrupted command did not release its slot")
    log.close()


def prove_recovery(runner: Runner, app: str) -> None:
    """A killed oxide leaves its slot held while Cargo's group survives; the
    next command waits, then recovers once the group is empty."""
    pidfile = os.path.join(runner.parent, "hold-crash.pid")
    child, log = runner.start(app, ["test"], "crash", {"OXIDE_FIXTURE_HOLD": pidfile})
    wait_for(lambda: os.path.exists(pidfile), "crash test")
    with open(pidfile, encoding="utf-8") as handle:
        test_pid = int(handle.read())
    child.kill()
    child.wait(timeout=20)
    log.close()
    if not process_alive(test_pid):
        fail("Cargo's group did not survive the killed oxide")
    running = [record for record in runner.state()["records"] if record["phase"] == "running"]
    if len(running) != 1 or not running[0]["process_group"]:
        fail(f"crashed record: {runner.state()['records']}")
    code, stdout, _ = runner.run(app, ["status"])
    if "held by an interrupted command" not in stdout:
        fail(f"status did not show the interrupted slot: {stdout}")
    follower, follower_log = runner.start(app, ["build"], "follower")
    wait_for(lambda: "waiting for another oxide command in this checkout" in read_log(follower_log), "follower wait")
    if follower.poll() is not None:
        fail("follower took a slot while Cargo's group survived")
    with open(pidfile.replace(".pid", ".release"), "w", encoding="utf-8"):
        pass
    expect(follower.wait(timeout=min(60, remaining())), 0, "follower", read_log(follower_log))
    if "recovered slot" not in read_log(follower_log):
        fail("follower did not record the recovery")
    if process_alive(test_pid):
        fail("crash test process outlived recovery")
    follower_log.close()


def main() -> int:
    global STARTED
    parent = os.path.join(scratch_root(), "oxide-cli")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    executables = build_binaries()
    STARTED = time.monotonic()
    runner = Runner(executables["oxide"], executables["oxide-storage-lease"], parent)
    app = os.path.join(parent, "checkouts", "app")
    write_fixture(app)
    if tree_bytes(app, False) > SOURCE_LIMIT_BYTES:
        fail("fixture source exceeds 8 MiB")
    prove_usage(runner, app)
    prove_adoption(runner, os.path.join(parent, "adoption"))
    if sys.platform != "darwin":
        fail("the bounded APFS oxide proof requires macOS")
    free = host_free_bytes(parent)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve")
    git(app, "init", "--quiet")
    git(app, "add", "-A")
    git(app, "commit", "--quiet", "-m", "fixture")
    try:
        prove_native(runner, app)
    except Exception:
        print(f"oxide proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    payload = os.path.join(runner.pool, "payload")
    mounted = tree_bytes(payload, False)
    owned = tree_bytes(parent, True)
    if owned > SCRATCH_LIMIT_BYTES:
        fail(f"owned fixture scratch {owned} exceeds 512 MiB")
    detach = subprocess.run(
        ["/usr/bin/hdiutil", "detach", payload], capture_output=True, text=True, timeout=60
    )
    if detach.returncode:
        fail(f"detach failed after positive completion: {detach.stderr}")
    elapsed = time.monotonic() - STARTED
    shutil.rmtree(parent)
    print(
        f"oxide CLI proof held: {elapsed:.0f} s, owned scratch {owned // (1024 * 1024)} MiB "
        f"(backing allocation included), mounted payload {mounted // (1024 * 1024)} MiB"
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        sys.stderr.write(f"{error}\n")
        sys.exit(1)
