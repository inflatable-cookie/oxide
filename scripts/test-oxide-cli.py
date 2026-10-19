"""Build the unpublished `oxide` front door and prove it on tiny fixtures.

Usage refusals always run without a pool. On macOS, after a 20 GiB host
reserve, the script provisions exactly one fresh 128 MiB APFS pool through
`oxide-storage-lease init` (one 128 MiB pool, 512 MiB scratch, 300 s total
proof phase including setup) and points OXIDE_POOL at it; the default pool is
never touched. Offline std-only fixture crates prove command routing, the
quick/debug profile flags (including an external subcommand's `$CARGO`
call), storage-override refusals, `clean`, cross-checkout reuse through the
freshness gate (two Git worktrees of a scratch repository: the stale-mtime
scenario rebuilds, every package under the checkout (members and in-repo
path dependencies, from `cargo metadata`) rebuilds on a path change,
a path dependency outside the repository stays fresh, an unchanged repeat
rebuilds nothing, a refused gate builds cold, and a crash inside the gate
leaves the retry gated; the gate's effect-free metadata call carries the
command's lock, network and config arguments: a fresh worktree with an
ignored lock gets it from `generate-lockfile` and warms its outside path
dependency, certain `--locked`/`--frozen` refusals end with Cargo's own
error, `--offline` is mirrored, and a stale lock or a help request falls
back cold without the gate writing a lock), FIFO waiting and the host
limit, clean waiting for active commands, `default-run`
selection (package and virtual workspace roots), Ctrl-C forwarding, own-command crash recovery
(a killed oxide whose Cargo group survives stays held; one whose group is gone
is recovered by the next command and logged; a standalone-CLI lease is never
recovered; a `setsid` escapee is shown as the documented limit),
`status` stray detection and Cargo-equal exit codes. After the image is
detached, two scratch `directory-budget` pools prove budget-only labelling,
the standalone CLI's refusal, a reserve stop on injected watchdog readings
(a bounded writer that ignores TERM is killed, the group closes and the slot
is released; an unreaped group member keeps closure unconfirmed and the slot
held until the next command recovers it) and admission that evicts a whole
idle slot on a 1 MiB budget. Further scratch directory pools prove slot file
counts (a build-script hard-linked pair counted once) and OXIDE_POOL_CAP
(refused when invalid, used on create, reported and ignored on an existing
pool; also reported on the APFS pool). Last, one fresh private CARGO_HOME
proves the status Cargo home line and `clean --cargo-cache`: refusals, a
cleanup that ran through the closed proxy with no connection, Cargo's lock
skip and a closed-proxy denial an owned control listener never sees. Setup
downloads exactly one tiny archive (cfg-if 1.0.0); nothing else may use the
network. The scratch is removed only after positive completion. Before any pool
exists, `init` / `new` adoption runs on tiny scratch directories (90 s,
64 MiB scratch) and must leave the pool uncreated. Also before the pool, one
macOS `status --launch-check` run under a Cargo that refuses and records any
call: bounded time, its temporary copies removed, well-formed output and no
Cargo; the hidden probe mode alone gets the same refused-Cargo control. No
penalty threshold is asserted. Not a benchmark and not full QA.

`--cargo-matrix <release>`, `--toolchain-context` and `--launch-check` are
separate focused proofs that run none of the above; see their functions.
`--launch-check` runs only the launch-check case: offline staging into its
own scratch (counted), the 20 GiB reserve sampled at admission and after
each phase, no pool, image or download, 300 s including cleanup.
"""
from __future__ import annotations

import errno
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import traceback

IMAGE_BYTES = 128 * 1024 * 1024
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
SOURCE_LIMIT_BYTES = 8 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
PROOF_SECONDS = 300
ADOPTION_SECONDS = 90
ADOPTION_SCRATCH_BYTES = 64 * 1024 * 1024
BUDGET_ONLY = "budget-only: admission estimate plus best-effort reserve watchdog, no hard ceiling"
# The watchdog trigger: 20 GiB reserve plus 2 GiB margin. The budget case
# injects readings around it and never consumes host space.
TRIGGER_BYTES = 22 * 1024 * 1024 * 1024
WRITER_HEADROOM_BYTES = 8 * 1024 * 1024
WRITER_BOUND_BYTES = 48 * 1024 * 1024
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
    // A descendant that leaves Cargo's process group with setsid.
    if let Some(python) = std::env::var_os("OXIDE_FIXTURE_ESCAPE") {
        std::process::Command::new(python)
            .args(["-c", ESCAPE])
            .arg(pidfile.with_extension("escape"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("escapee");
    }
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

#[test]
fn optional_writer() {
    // A bounded writer that ignores TERM, for the budget pool watchdog.
    let Some(python) = std::env::var_os("OXIDE_FIXTURE_WRITER") else {
        return;
    };
    let status = std::process::Command::new(python)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/writer.py"))
        .status()
        .expect("writer");
    assert!(status.success(), "writer {status}");
}

const ESCAPE: &str = "import os, sys, time\\nos.setsid()\\ntemp = sys.argv[1] + '.tmp'\\nopen(temp, 'w').write(str(os.getpid()))\\nos.rename(temp, sys.argv[1])\\ntime.sleep(60)\\n";
""",
    "tests/writer.py": """import os, signal, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
readings = os.environ["OXIDE_FIXTURE_WRITER_READINGS"]
base = int(os.environ["OXIDE_FIXTURE_WRITER_BASE"])
bound = int(os.environ["OXIDE_FIXTURE_WRITER_BOUND"])
chunk = b"x" * (1 << 20)
written = 0
with open(os.environ["OXIDE_FIXTURE_WRITER_OUT"], "wb") as handle:
    while written < bound:
        handle.write(chunk)
        handle.flush()
        os.fsync(handle.fileno())
        written += len(chunk)
        with open(readings + ".tmp", "w") as reading:
            reading.write(str(base - written))
        os.rename(readings + ".tmp", readings)
        time.sleep(0.05)
time.sleep(60)
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

# Freshness gate fixture: a Git workspace whose `marker` package embeds a
# data file at compile time and whose `embed` member embeds its own
# `CARGO_MANIFEST_DIR`; both tests compare the compiled value with this
# checkout's. `outside` is a path dependency outside the repository. The
# in-repo non-member path dependencies `buildrs` (default build.rs, no
# declaration, embeds current_dir()) and `pathmac` (a proc macro that
# expands to its caller's manifest dir) are checked through marker's test.
GATED = {
    "Cargo.toml": """[workspace]
members = [".", "embed", "codegen"]
default-members = [".", "embed", "codegen"]
exclude = ["vendor"]

[package]
name = "marker"
version = "0.1.0"
edition = "2021"

[dependencies]
outside = { path = "../outside" }
buildrs = { path = "vendor/buildrs" }
pathmac = { path = "vendor/pathmac" }
""",
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    ".gitignore": "/target\n",
    "data/marker.txt": "one\n",
    "src/lib.rs": """pub const MARKER: &str = include_str!("../data/marker.txt");

pub fn outside_value() -> u32 {
    outside::value()
}

pub fn buildrs_dir() -> &'static str {
    buildrs::dir()
}

pub fn caller_dir() -> &'static str {
    pathmac::caller_dir!()
}
""",
    "tests/content.rs": """#[test]
fn compiled_marker_is_this_checkouts() {
    let here = std::fs::read_to_string("data/marker.txt").expect("marker");
    assert_eq!(marker::MARKER, here);
    assert_eq!(marker::outside_value(), 5);
    let canonical = |path: &str| std::fs::canonicalize(path).expect("dir");
    assert_eq!(canonical(marker::buildrs_dir()), canonical("vendor/buildrs"));
    assert_eq!(canonical(marker::caller_dir()), canonical("."));
}
""",
    "embed/Cargo.toml": '[package]\nname = "embed"\nversion = "0.1.0"\nedition = "2021"\n',
    "embed/src/lib.rs": """pub fn dir() -> &'static str {
    env!("CARGO_MANIFEST_DIR")
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiled_path_is_this_checkouts() {
        assert_eq!(super::dir(), std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo test"));
    }
}
""",
}

# `codegen` names its build script through an escaped TOML key (`build`
# spelled with \u0075) and has no default build.rs. The script embeds its
# working directory, the package root, without the literal
# CARGO_MANIFEST_DIR; the test compares it with this checkout's.
GATED["codegen/Cargo.toml"] = (
    '[package]\nname = "codegen"\nversion = "0.1.0"\nedition = "2021"\n"b\\u0075ild" = "gen.rs"\n'
)
GATED["codegen/gen.rs"] = """fn main() {
    let here = std::env::current_dir().expect("build script cwd");
    println!("cargo:rustc-env=CODEGEN_DIR={}", here.display());
}
"""
GATED["codegen/src/lib.rs"] = """pub fn dir() -> &'static str {
    env!("CODEGEN_DIR")
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_path_is_this_checkouts() {
        let compiled = std::fs::canonicalize(super::dir()).expect("compiled dir");
        let here = std::fs::canonicalize(std::env::current_dir().expect("cwd")).expect("cwd");
        assert_eq!(compiled, here);
    }
}
"""

GATED["vendor/buildrs/Cargo.toml"] = (
    '[package]\nname = "buildrs"\nversion = "0.1.0"\nedition = "2021"\n'
)
GATED["vendor/buildrs/build.rs"] = """fn main() {
    let here = std::env::current_dir().expect("build script cwd");
    println!("cargo:rustc-env=BUILDRS_DIR={}", here.display());
}
"""
GATED["vendor/buildrs/src/lib.rs"] = """pub fn dir() -> &'static str {
    env!("BUILDRS_DIR")
}
"""
GATED["vendor/pathmac/Cargo.toml"] = (
    '[package]\nname = "pathmac"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\nproc-macro = true\n'
)
GATED["vendor/pathmac/src/lib.rs"] = """use proc_macro::TokenStream;

#[proc_macro]
pub fn caller_dir(_input: TokenStream) -> TokenStream {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("caller manifest dir");
    format!("{dir:?}").parse().expect("string literal")
}
"""

# Every unit the gate must rebuild on a checkout-path change: all packages
# under the checkout, members and in-repo path dependencies alike.
LOCAL_UNITS = (
    "marker:build",
    "marker:test",
    "content:test",
    "embed:build",
    "embed:test",
    "codegen:build",
    "codegen:test",
    "buildrs:build",
    "pathmac:build",
)

OUTSIDE = {
    "Cargo.toml": '[package]\nname = "outside"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    5\n}\n",
}

# Holds oxide inside the freshness gate: the gate's first git call is
# `rev-parse --show-toplevel`; identity resolution never makes it.
GIT_HOLD = """#!/bin/sh
if [ -n "$OXIDE_FIXTURE_GIT_HOLD" ] && [ "$1 $2" = "rev-parse --show-toplevel" ]; then
    echo $$ > "$OXIDE_FIXTURE_GIT_HOLD.tmp" && mv "$OXIDE_FIXTURE_GIT_HOLD.tmp" "$OXIDE_FIXTURE_GIT_HOLD"
    while [ ! -e "$OXIDE_FIXTURE_GIT_HOLD.release" ]; do sleep 0.1; done
fi
exec "REAL_GIT" "$@"
"""

# Lockless fixture: `Cargo.lock` is ignored, so every fresh worktree starts
# without one. `far` is a path dependency outside the repository.
LOCKLESS = {
    "Cargo.toml": """[package]
name = "lockless"
version = "0.1.0"
edition = "2021"

[dependencies]
far = { path = "../far" }
""",
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    ".gitignore": "/target\n/Cargo.lock\n",
    "src/lib.rs": """pub fn value() -> u32 {
    far::value() + 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_far() {
        assert_eq!(super::value(), 8);
    }
}
""",
}

FAR = {
    "Cargo.toml": '[package]\nname = "far"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
}

# Logs every `cargo metadata` call that is not `--no-deps`, and every
# `cargo generate-lockfile`: working directory, CARGO_NET_OFFLINE and
# arguments, then runs the real Cargo.
CARGO_LOG = """#!/bin/sh
if [ -n "$OXIDE_FIXTURE_CARGO_LOG" ]; then
    case "$1 $* " in
    "metadata "*" --no-deps "*) ;;
    "metadata "* | "generate-lockfile "*) printf '%s|%s|%s\\n' "$(pwd -P)" "${CARGO_NET_OFFLINE-unset}" "$*" >> "$OXIDE_FIXTURE_CARGO_LOG" ;;
    esac
fi
exec "REAL_CARGO" "$@"
"""

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


def build_binaries(extra: tuple = (), env: dict | None = None) -> dict:
    build = subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--bin",
            "oxide",
            "--bin",
            "oxide-storage-lease",
            *extra,
            "--message-format=json-render-diagnostics",
        ],
        stdout=subprocess.PIPE,
        text=True,
        env=env,
        timeout=remaining(),
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
    def __init__(self, oxide: str, lease: str, parent: str, pool: str = "pool", extra: dict | None = None):
        self.oxide = oxide
        self.lease = lease
        self.parent = parent
        self.pool = os.path.join(parent, pool)
        self.bin = os.path.join(parent, "bin")
        self.tmp = os.path.join(parent, "tmp")
        # Every fixture Cargo call, Oxide's and its descendants included, uses
        # this private home; product staging used the host's.
        self.cargo_home = os.path.join(parent, "cargo-home")
        for path in (self.bin, self.tmp, self.cargo_home):
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
                "CARGO_HOME": self.cargo_home,
                "PATH": self.bin + os.pathsep + env.get("PATH", ""),
                "CARGO_NET_OFFLINE": "true",
                "CARGO_TERM_COLOR": "never",
                "TMPDIR": self.tmp,
                "TMP": self.tmp,
                "TEMP": self.tmp,
            }
        )
        env.pop("OXIDE_POOL_BACKEND", None)
        env.update(extra or {})
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


# Every wait line Oxide emits: each `Wait::describe` form printed by
# `acquire` and the `clean` wait in store.rs, and the group wait in
# process.rs. Cargo's "Blocking waiting for file lock on package
# cache" and other tools' lines never match.
OXIDE_WAIT = re.compile(
    r"oxide: waiting for (?:"
    r"earlier oxide commands \(position \d+\)"
    r"|a build slot \(\d+ of \d+ host-wide in use\)"
    r"|another oxide command in this checkout"
    r"|oxide clean in this checkout"
    r"|an idle slot of this project \(cap \d+ slots\)"
    r"|a slot held by an interrupted command whose Cargo may still run"
    r"|pool capacity held by running builds"
    r"|active oxide commands in this checkout"
    r"); Ctrl-C cancels"
    r"|oxide: waiting for processes left in Cargo's process group \d+ to exit"
)


def oxide_waits(text: str) -> list:
    """Oxide's own wait lines in `text`, whole lines only."""
    return [line for line in text.splitlines() if OXIDE_WAIT.fullmatch(line)]


def prove_wait_lines() -> None:
    """The wait predicate classifies only Oxide's exact lines."""
    others = (
        "    Blocking waiting for file lock on package cache",
        "Blocking waiting for file lock on build directory",
        "info: waiting for rustup lock",
        "oxide:waiting for a build slot (1 of 1 host-wide in use); Ctrl-C cancels",
        "xoxide: waiting for another oxide command in this checkout; Ctrl-C cancels",
        "  oxide: waiting for another oxide command in this checkout; Ctrl-C cancels",
        "oxide: waiting for something else; Ctrl-C cancels",
        "oxide: waiting for a build slot (1 of 1 host-wide in use)",
    )
    for line in others:
        if oxide_waits(line):
            fail(f"wait predicate classified a non-Oxide line: {line!r}")
    own = (
        "oxide: waiting for earlier oxide commands (position 2); Ctrl-C cancels",
        "oxide: waiting for a build slot (1 of 1 host-wide in use); Ctrl-C cancels",
        "oxide: waiting for another oxide command in this checkout; Ctrl-C cancels",
        "oxide: waiting for oxide clean in this checkout; Ctrl-C cancels",
        "oxide: waiting for an idle slot of this project (cap 2 slots); Ctrl-C cancels",
        "oxide: waiting for a slot held by an interrupted command whose Cargo may still run; Ctrl-C cancels",
        "oxide: waiting for pool capacity held by running builds; Ctrl-C cancels",
        "oxide: waiting for active oxide commands in this checkout; Ctrl-C cancels",
        "oxide: waiting for processes left in Cargo's process group 4242 to exit",
    )
    mixed = "\n".join(others + own)
    if oxide_waits(mixed) != list(own):
        fail(f"wait predicate missed Oxide lines: {oxide_waits(mixed)}")
    # A real Oxide wait in otherwise ordinary stderr still fails a no-wait check.
    for line in own:
        if not oxide_waits(f"   Compiling app v0.1.0\n{line}\n    Finished `dev`"):
            fail(f"no-wait check would accept an Oxide wait: {line!r}")


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
    if sys.platform == "darwin" and "(apfs-sparse-image-v1: OS-enforced ceiling)" not in stdout:
        fail(f"the macOS default backend changed: {stdout!r}")
    code, stdout, _ = runner.run(cwd, ["status"], {"OXIDE_POOL_BACKEND": "directory-budget"})
    expect(code, 0, "status for a budget pool to be")
    if f"(directory-budget-v1: {BUDGET_ONLY})" not in stdout:
        fail(f"status did not name the requested budget backend: {stdout!r}")
    code, _, stderr = runner.run(cwd, ["status"], {"OXIDE_POOL_BACKEND": "zfs"})
    expect(code, 1, "unknown backend tag", stderr)
    if os.path.lexists(runner.pool):
        fail("usage, refusals or status created the pool")


# Records any call and refuses: the launch check must never start Cargo.
REFUSED_CARGO = """#!/bin/sh
echo "$*" >> "$OXIDE_FIXTURE_CARGO_CALLED"
exit 97
"""
LAUNCH_LIMIT_MS = 3000


def launch_samples(line: str, label: str) -> list:
    match = re.fullmatch(r"(?:none|(\d+\.\d ms(?:, \d+\.\d ms)*))", line)
    if not match:
        fail(f"launch check {label} runs are malformed: {line!r}")
    return [round(float(value[:-3]) * 10) for value in line.split(", ")] if match.group(1) else []


def prove_launch_check(runner: Runner, cwd: str) -> None:
    refused = os.path.join(runner.parent, "refused-cargo")
    os.makedirs(refused, mode=0o700)
    shim = os.path.join(refused, "cargo")
    with open(shim, "w", encoding="utf-8") as handle:
        handle.write(REFUSED_CARGO)
    os.chmod(shim, 0o700)
    called = os.path.join(runner.parent, "refused-cargo.log")
    extra = {"PATH": refused + os.pathsep + runner.env["PATH"], "OXIDE_FIXTURE_CARGO_CALLED": called}
    # Positive control: the shim sees a Cargo call.
    code, _, _ = runner.run(cwd, ["version"], extra)
    expect(code, 97, "refused-Cargo control")
    if not os.path.exists(called):
        fail("the refused Cargo recorded nothing")
    os.unlink(called)
    # The hidden probe mode exits before any other work.
    code, stdout, stderr = runner.run(cwd, ["__oxide-launch-probe"], extra)
    expect(code, 0, "hidden launch probe", stderr)
    if stdout or stderr or os.path.exists(called):
        fail(f"the hidden probe printed or started Cargo: {stdout!r} {stderr!r}")
    before = set(os.listdir(runner.tmp))
    started = time.monotonic()
    code, stdout, stderr = runner.run(cwd, ["status", "--launch-check"], extra)
    wall_ms = (time.monotonic() - started) * 1000
    expect(code, 0, "status --launch-check", stderr)
    if os.path.exists(called):
        fail(f"status --launch-check started Cargo: {read_text(called)!r}")
    left = set(os.listdir(runner.tmp)) - before
    if left:
        fail(f"status --launch-check left temporary files: {sorted(left)}")
    if stderr:
        fail(f"status --launch-check wrote to stderr: {stderr!r}")
    if sys.platform != "darwin":
        if stdout.strip() != "launch check: not applicable (it measures a macOS first-launch cost)":
            fail(f"launch check off macOS: {stdout!r}")
        return
    lines = stdout.splitlines()
    if len(lines) < 5 or not lines[0].startswith("launch check: "):
        fail(f"launch check output is malformed: {stdout!r}")
    same = re.fullmatch(r"  same file: warm-up (none|\d+\.\d ms); runs (.*)", lines[1])
    fresh = re.fullmatch(r"  fresh copies: runs (.*)", lines[2])
    took = re.fullmatch(r"  took (\d+) ms of a 3000 ms limit; temporary copies removed", lines[3])
    if not (same and fresh and took):
        fail(f"launch check output is malformed: {stdout!r}")
    elapsed_ms = int(took.group(1))
    # The reported span is the probe's own; the wall time adds one process
    # start of an already-assessed oxide.
    if elapsed_ms > LAUNCH_LIMIT_MS or wall_ms > LAUNCH_LIMIT_MS + 1000:
        fail(f"launch check exceeded its bound: reported {elapsed_ms} ms, wall {wall_ms:.0f} ms")
    same_runs = launch_samples(same.group(2), "same-file")
    fresh_runs = launch_samples(fresh.group(1), "fresh-copy")
    if lines[4].startswith("  result: inconclusive"):
        if len(lines) != 5:
            fail(f"an inconclusive launch check printed more: {stdout!r}")
        outcome = "inconclusive"
    else:
        medians = re.fullmatch(
            r"  medians: same file (\d+\.\d) ms, fresh copy (\d+\.\d) ms, difference (-?\d+\.\d) ms",
            lines[4],
        )
        if not medians or same.group(1) == "none" or len(same_runs) != 5 or len(fresh_runs) != 5:
            fail(f"launch check medians without complete samples: {stdout!r}")
        # Medians come from the raw samples (tenths of a millisecond here).
        for printed, runs in ((medians.group(1), same_runs), (medians.group(2), fresh_runs)):
            if round(float(printed) * 10) != sorted(runs)[2]:
                fail(f"launch check median {printed} is not the median of {runs}")
        difference = float(medians.group(3))
        result = lines[5] if len(lines) > 5 else ""
        if result.startswith("  result: no material penalty"):
            outcome = "no material penalty"
            if difference > 30 or len(lines) != 6:
                fail(f"launch check below threshold is malformed: {stdout!r}")
        elif result.startswith("  result: penalty "):
            outcome = "penalty"
            advice = "\n".join(lines[6:])
            for needed in (
                "cause: inferred, not measured",
                "launching app",
                "Privacy & Security → Developer Tools",
                "may need a restart",
                "has not checked or changed any setting",
                "never disable Gatekeeper globally",
            ):
                if needed not in advice:
                    fail(f"launch check advice lacks {needed!r}: {stdout!r}")
            if difference <= 30:
                fail(f"launch check advised below the threshold: {stdout!r}")
        else:
            fail(f"launch check result is malformed: {stdout!r}")
    if os.path.lexists(runner.pool):
        fail("status --launch-check created the pool")
    print(
        f"oxide launch check: {outcome}, reported {elapsed_ms} ms, wall {wall_ms:.0f} ms, "
        f"same-file runs {same.group(2)}, fresh-copy runs {fresh.group(1)}; no Cargo started, "
        "temporary copies removed"
    )


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
    # OXIDE_POOL_CAP never resizes an existing pool; a conflict is reported.
    code, stdout, stderr = runner.run(app, ["status"], {"OXIDE_POOL_CAP": "256MiB"})
    expect(code, 0, "status with a conflicting OXIDE_POOL_CAP", stderr)
    if (
        f"oxide: OXIDE_POOL_CAP 256 MiB not applied: the pool at {runner.pool} keeps its recorded 128 MiB cap" not in stderr
        or "  cap 128 MiB, " not in stdout
    ):
        fail(f"APFS cap conflict was not reported or the cap changed: {stderr!r} {stdout!r}")

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
        "  backend apfs-sparse-image-v1: OS-enforced ceiling",
        f"slot {quick_slot} idle quick {app}",
        f"slot {debug_slot} idle debug {app}",
        "waiting commands 0",
        "holds plain-Cargo deps/ or incremental/",
    ):
        if needle not in stdout:
            fail(f"status missing {needle!r}: {stdout}")

    # The APFS pool keeps its backend: a request for another refuses before
    # any lease, and summaries name the hard-bound backend.
    code, _, stderr = runner.run(app, ["build"], {"OXIDE_POOL_BACKEND": "directory-budget"})
    expect(code, 1, "backend mismatch on the APFS pool", stderr)
    if "uses apfs-sparse-image-v1; OXIDE_POOL_BACKEND applies only when a pool is created" not in stderr:
        fail(f"backend mismatch gave no explanation: {stderr!r}")
    code, _, stderr = runner.run(app, ["build"])
    expect(code, 0, "APFS summary", stderr)
    if "MiB used (apfs-sparse-image-v1)" not in stderr or "budget" in stderr:
        fail(f"APFS summary does not name its backend: {stderr[-800:]!r}")

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

    # Cross-checkout reuse: a second checkout of the same repository takes
    # the idle compatible slot through the freshness gate instead of
    # evicting; the first checkout takes it back the same way, then reuses
    # it as its own. A detached slot (after clean) is never shared.
    app2 = os.path.join(runner.parent, "checkouts", "app2")
    git(os.path.dirname(app2), "clone", "--quiet", "--no-hardlinks", app, app2)
    code, _, stderr = runner.run(app2, ["build"])
    expect(code, 0, "second checkout build", stderr)
    second = summary(stderr, "second checkout")
    if second["warmth"] != "warm" or second["slot"] != rebuilt["slot"]:
        fail(f"second checkout did not take the idle compatible slot: {second} vs {rebuilt}")
    if "evicted idle slot" in stderr or "freshness gate refreshed" not in stderr:
        fail(f"second checkout evicted or skipped the gate: {stderr[-800:]}")
    for label, gated in (("first checkout takes it back", True), ("own slot again", False)):
        code, _, stderr = runner.run(app, ["build"])
        expect(code, 0, label, stderr)
        again = summary(stderr, label)
        if again["slot"] != rebuilt["slot"] or again["warmth"] != "warm":
            fail(f"{label}: {again} vs {rebuilt}")
        if ("freshness gate refreshed" in stderr) != gated:
            fail(f"{label}: gate note {'missing' if gated else 'unexpected'}: {stderr[-800:]}")
    if len(runner.lease_status()["slots"]) != 2:
        fail("project slot cap of 2 was not kept")

    prove_waiting(runner, app, app2)
    prove_clean_waits(runner, app)
    prove_interrupt(runner, app)
    if runner.state().get("recoveries"):
        fail(f"clean exits or Ctrl-C were logged as recoveries: {runner.state()['recoveries']}")
    standalone = take_standalone_lease(runner)
    prove_recovery(runner, app)
    prove_group_gone(runner, app)
    keep_standalone_lease(runner, app, standalone)
    prove_freshness(runner)
    prove_lockless(runner)

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


def unit_freshness(stdout: str) -> dict:
    """Cargo JSON `compiler-artifact` rows: `name:build|test` to `fresh`."""
    units = {}
    for line in stdout.splitlines():
        if not line.startswith("{"):
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-artifact":
            role = "test" if message["profile"]["test"] else "build"
            units[f"{message['target']['name']}:{role}"] = message["fresh"]
    return units


def commit_lock(runner: Runner, cwd: str) -> None:
    """Write the fixture's lock file before its first commit, so the
    checkouts share one resolved graph."""
    lock = subprocess.run(
        ["cargo", "generate-lockfile", "--offline"],
        cwd=cwd,
        env=runner.environment(None),
        capture_output=True,
        text=True,
        timeout=min(60, remaining()),
    )
    expect(lock.returncode, 0, "fixture lock file", lock.stderr)


def prove_freshness(runner: Runner) -> None:
    """Two worktrees of one scratch repository share a slot through the
    freshness gate; a clone missing the slot's tree is refused and builds
    cold. Each test compares compiled values with its own checkout."""
    root = os.path.join(runner.parent, "gated")
    a, b, c = (os.path.join(root, name) for name in ("a", "b", "c"))
    write_fixture(os.path.join(root, "outside"), OUTSIDE)
    write_fixture(a, GATED)
    commit_lock(runner, a)
    git(a, "init", "--quiet")
    git(a, "add", "-A")
    git(a, "commit", "--quiet", "-m", "gated fixture")
    # Both worktrees exist before any build.
    git(a, "worktree", "add", "--quiet", b)

    def test(cwd: str, label: str):
        code, stdout, stderr = runner.run(cwd, ["test", "--message-format=json"])
        expect(code, 0, label, stderr)
        units = unit_freshness(stdout)
        print(f"freshness {label}: {summary(stderr, label)} units {units}")
        return summary(stderr, label), units, stderr

    first, _, _ = test(a, "worktree A cold")
    if first["warmth"] != "cold":
        fail(f"first gated build was not cold: {first}")
    slot = first["slot"]

    def shared(result: dict, label: str) -> None:
        if result["slot"] != slot or result["warmth"] != "warm":
            fail(f"{label} did not reuse slot {slot} warm: {result}")

    # Path change, same content: every package under the checkout rebuilds
    # and its test sees B's path; the outside path dependency stays fresh.
    result, units, stderr = test(b, "worktree B path change")
    shared(result, "worktree B")
    if "freshness gate refreshed" not in stderr or "inputs outside Git or ignored by it are not checked" not in stderr:
        fail(f"worktree B summary lacks the gate and its limit: {stderr[-800:]}")
    # Includes the escaped build key, default build.rs, current_dir() script
    # and proc-macro caller; the tests passing prove they saw B's path.
    kept = [unit for unit in LOCAL_UNITS if units.get(unit) is not False]
    if kept:
        fail(f"local units did not rebuild on the path change: {kept} of {units}")
    if units.get("outside:build") is not True:
        fail(f"outside path dependency did not stay fresh: {units}")

    # Stale-mtime scenario: A edits and builds; B's differing file is older
    # than that build, yet B rebuilds and observes its own content.
    a_marker, b_marker = (os.path.join(path, "data", "marker.txt") for path in (a, b))
    with open(a_marker, "w", encoding="utf-8") as handle:
        handle.write("two from A\n")
    result, _, _ = test(a, "worktree A edit")
    shared(result, "worktree A edit")
    b_mtime = os.stat(b_marker).st_mtime_ns
    if b_mtime >= os.stat(a_marker).st_mtime_ns:
        fail("B's marker is not older than A's edit; the stale precondition does not hold")
    result, units, _ = test(b, "stale scenario B")
    shared(result, "stale scenario B")
    if units.get("marker:build") is not False or units.get("content:test") is not False:
        fail(f"stale scenario did not rebuild the affected units: {units}")
    if units.get("outside:build") is not True:
        fail(f"outside path dependency rebuilt in the stale scenario: {units}")
    if read_text(b_marker) != "one\n" or os.stat(b_marker).st_mtime_ns <= b_mtime:
        fail("the gate changed B's content or did not refresh its mtime")

    # Unchanged repeat: nothing rebuilds.
    result, units, stderr = test(b, "unchanged repeat")
    shared(result, "unchanged repeat")
    if not units or not all(units.values()) or "freshness gate refreshed" in stderr:
        fail(f"unchanged repeat rebuilt units or ran the gate: {units}")

    # Refused gate: a clone lacking A's newer tree falls back cold to its own
    # slot; A's slot is given back intact.
    git(root, "clone", "--quiet", "--no-hardlinks", a, c)
    git(a, "commit", "--quiet", "-am", "A moves on")
    result, _, _ = test(a, "worktree A after commit")
    shared(result, "worktree A after commit")
    result, _, stderr = test(c, "refused gate")
    if "freshness gate refused another checkout's slot" not in stderr:
        fail(f"clone was not refused by the gate: {stderr[-800:]}")
    if result["warmth"] != "cold" or result["slot"] == slot:
        fail(f"refused gate did not build cold on another slot: {result}")
    result, units, _ = test(a, "worktree A keeps its slot")
    shared(result, "worktree A keeps its slot")
    if not units or not all(units.values()):
        fail(f"A's slot was disturbed by the refused gate: {units}")

    # Crash inside the gate after A's slot moved to B: the retry recovers the
    # slot and runs the gate again. Unchecked reuse would keep A's marker
    # unit and fail B's content test.
    prove_gate_crash(runner, b, slot, test)


def prove_lockless(runner: Runner) -> None:
    """The gate's metadata call carries the guarded command's lock, network
    and config arguments plus `--offline --locked` where it lacks them, so it
    never downloads or writes. A fresh worktree with an ignored lock gets it
    from `cargo generate-lockfile` with the command's own arguments and
    reuses the outside path dependency warm. Certain `--locked`/`--frozen`
    refusals end the command with Cargo's own error, slot and lock
    untouched; a stale lock the command may update, or a help request,
    refuses the slot and Cargo does its own work cold."""
    root = os.path.join(runner.parent, "lockless")
    a, b, c, d, e, f, g = (os.path.join(root, name) for name in "abcdefg")
    write_fixture(os.path.join(root, "far"), FAR)
    write_fixture(a, LOCKLESS)
    git(a, "init", "--quiet")
    git(a, "add", "-A")
    git(a, "commit", "--quiet", "-m", "lockless fixture")
    for worktree in (b, c, d, e, f, g):
        git(a, "worktree", "add", "--quiet", worktree)
    log_path = os.path.join(runner.parent, "metadata-calls.log")
    wrapper = os.path.join(runner.bin, "cargo")
    real_cargo = shutil.which("cargo", path=runner.env["PATH"])
    if not real_cargo or os.path.dirname(real_cargo) == runner.bin:
        fail("real cargo not found for the metadata-log wrapper")
    with open(wrapper, "w", encoding="utf-8") as handle:
        handle.write(CARGO_LOG.replace("REAL_CARGO", real_cargo))
    os.chmod(wrapper, 0o700)

    def lock(cwd: str):
        path = os.path.join(cwd, "Cargo.lock")
        return read_text(path) if os.path.exists(path) else None

    def calls() -> list:
        if not os.path.exists(log_path):
            return []
        with open(log_path, encoding="utf-8") as handle:
            rows = [line.rstrip("\n").split("|", 2) for line in handle]
        os.remove(log_path)
        return rows

    def oxide(cwd: str, args: list, label: str, offline: str = "true"):
        before = lock(cwd)
        extra = {"OXIDE_FIXTURE_CARGO_LOG": log_path, "CARGO_NET_OFFLINE": offline}
        code, stdout, stderr = runner.run(cwd, ["test", "--message-format=json", *args], extra)
        made = calls()
        after = lock(cwd)
        print(
            f"lockless {label}: exit {code}, CARGO_NET_OFFLINE={offline}, "
            f"lock {'present' if before else 'absent'} -> {'present' if after else 'absent'}, "
            f"gate Cargo calls {[(os.path.basename(row[0]), row[1], row[2]) for row in made]}"
        )
        return code, stdout, stderr, made, before, after

    def gate_calls(made: list, cwd: str, offline: str, wanted: list, label: str) -> None:
        manifest = f"--manifest-path {os.path.realpath(cwd)}/Cargo.toml"
        expected = [(os.path.realpath(cwd), offline, f"{call} {manifest}") for call in wanted]
        rows = [(os.path.realpath(row[0]), row[1], row[2]) for row in made]
        if rows != expected:
            fail(f"{label}: gate Cargo calls {made}, wanted {expected}")

    def warm(stdout: str, stderr: str, label: str) -> None:
        units = unit_freshness(stdout)
        result = summary(stderr, label)
        print(f"lockless {label}: {result} units {units}")
        if result["slot"] != slot or result["warmth"] != "warm":
            fail(f"{label} did not reuse slot {slot} warm: {result}")
        if units.get("far:build") is not True:
            fail(f"{label}: outside path dependency did not stay fresh: {units}")
        if units.get("lockless:build") is not False or units.get("lockless:test") is not False:
            fail(f"{label}: local units did not rebuild on the path change: {units}")

    def refused(code: int, stderr: str, cargo_error: str, label: str) -> None:
        expect(code, 101, label, stderr)
        if cargo_error not in stderr:
            fail(f"{label}: Cargo's refusal {cargo_error!r} missing from {stderr[-800:]!r}")
        if "building cold" in stderr or "oxide: quick profile" in stderr:
            fail(f"{label}: the refusal was masked as a cold build: {stderr[-800:]!r}")
        holders = [record["checkout"] for record in runner.state()["records"]]
        if os.path.realpath(c) in holders or os.path.realpath(b) not in holders:
            fail(f"{label}: the slot did not go back to worktree B unchanged: {holders}")

    def cold(code: int, stderr: str, label: str) -> None:
        if "freshness gate refused another checkout's slot" not in stderr:
            fail(f"{label}: the gate did not refuse the slot: {stderr[-800:]!r}")

    probe = "metadata --format-version 1"
    try:
        code, _, stderr, made, _, after = oxide(a, [], "worktree A cold")
        expect(code, 0, "lockless A", stderr)
        slot = summary(stderr, "lockless A")["slot"]
        if made or after is None:
            fail(f"lockless A ran the gate or left no lock: {made}")

        # Missing lock, no restriction, network allowed (a path-only graph):
        # the lock comes from generate-lockfile, then the slot is reused.
        code, stdout, stderr, made, before, after = oxide(b, [], "fresh worktree B", "false")
        expect(code, 0, "lockless B", stderr)
        gate_calls(
            made,
            b,
            "false",
            [f"{probe} --offline --locked", "generate-lockfile", f"{probe} --offline --locked"],
            "lockless B",
        )
        warm(stdout, stderr, "lockless B")
        if before is not None or after != lock(a):
            fail("worktree B did not get the lock file its build would write")

        # `--locked` with no lock file: Oxide ends exactly as plain Cargo.
        plain = subprocess.run(
            ["cargo", "test", "--locked"],
            cwd=c,
            env=runner.environment({"CARGO_TARGET_DIR": os.path.join(runner.tmp, "plain-target")}),
            capture_output=True,
            text=True,
            timeout=min(60, remaining()),
        )
        cargo_error = next(
            (line for line in plain.stderr.splitlines() if "--locked was passed" in line), None
        )
        if plain.returncode != 101 or not cargo_error or lock(c) is not None:
            fail(f"plain cargo --locked did not refuse the missing lock: {plain.stderr}")
        code, _, stderr, made, before, after = oxide(c, ["--locked"], "missing lock --locked")
        gate_calls(made, c, "true", [f"{probe} --locked --offline"], "missing lock --locked")
        refused(code, stderr, cargo_error, "missing lock --locked")
        if before is not None or after is not None:
            fail("--locked with a missing lock created one")

        # Stale lock: B's lock with the wrong `far` version. `--frozen` stays
        # locked and offline under a contradictory config and environment.
        stale = lock(b).replace('name = "far"\nversion = "0.1.0"', 'name = "far"\nversion = "0.0.1"')
        if stale == lock(b):
            fail("could not write a stale lock")
        for worktree in (c, d):
            with open(os.path.join(worktree, "Cargo.lock"), "w", encoding="utf-8") as handle:
                handle.write(stale)
        frozen = ["--frozen", "--config", "net.offline=false"]
        code, _, stderr, made, _, after = oxide(c, frozen, "stale lock --frozen", "false")
        gate_calls(made, c, "false", [f"{probe} {' '.join(frozen)}"], "stale lock --frozen")
        refused(code, stderr, "--frozen was passed to prevent this", "stale lock --frozen")
        if after != stale:
            fail("a lock refusal changed the stale lock")

        # `--offline` is mirrored even when the environment says online.
        code, stdout, stderr, made, before, after = oxide(e, ["--offline"], "fresh worktree E --offline", "false")
        expect(code, 0, "lockless E", stderr)
        gate_calls(
            made,
            e,
            "false",
            [f"{probe} --offline --locked", "generate-lockfile --offline", f"{probe} --offline --locked"],
            "lockless E",
        )
        warm(stdout, stderr, "lockless E")
        if before is not None or after != lock(a):
            fail("worktree E's offline lock generation did not match the build's")

        # Stale lock with `--locked` but online: the offline probe's refusal
        # is not certain, so the slot is refused and the cold build's Cargo
        # gives its own refusal.
        code, _, stderr, made, _, after = oxide(c, ["--locked"], "stale lock --locked online")
        gate_calls(made, c, "true", [f"{probe} --locked --offline"], "stale lock --locked online")
        cold(code, stderr, "stale lock --locked online")
        expect(code, 101, "stale lock --locked online", stderr)
        if "cannot update the lock file" not in stderr or after != stale:
            fail(f"stale --locked did not end in Cargo's own refusal: {stderr[-800:]!r}")

        # Stale lock, no restriction: the cold build updates it as usual.
        code, _, stderr, made, _, after = oxide(d, [], "stale lock plain")
        expect(code, 0, "stale lock plain", stderr)
        gate_calls(made, d, "true", [f"{probe} --offline --locked"], "stale lock plain")
        cold(code, stderr, "stale lock plain")
        if after != lock(a):
            fail("the cold build did not update the stale lock")

        # Help resolves nothing: no lock is written and no refusal is raised.
        for worktree, args, label in ((f, ["--locked", "--help"], "--locked --help"), (g, ["--help"], "--help")):
            code, stdout, stderr, made, _, after = oxide(worktree, args, label)
            expect(code, 0, label, stderr)
            wanted = f"{probe} --locked --offline" if "--locked" in args else f"{probe} --offline --locked"
            gate_calls(made, worktree, "true", [wanted], label)
            cold(code, stderr, label)
            if "Usage:" not in stdout or after is not None:
                fail(f"{label}: help was not printed or a lock was written: {stdout[:200]!r}")
    finally:
        os.remove(wrapper)


def prove_gate_crash(runner: Runner, b: str, slot: str, test) -> None:
    hold = os.path.join(runner.parent, "gate-hold.pid")
    wrapper = os.path.join(runner.bin, "git")
    real_git = shutil.which("git")
    if not real_git:
        fail("git not found for the gate-hold wrapper")
    with open(wrapper, "w", encoding="utf-8") as handle:
        handle.write(GIT_HOLD.replace("REAL_GIT", real_git))
    os.chmod(wrapper, 0o700)
    try:
        child, log = runner.start(b, ["test"], "gate-crash", {"OXIDE_FIXTURE_GIT_HOLD": hold})
        wait_for(lambda: os.path.exists(hold), "oxide held inside the freshness gate")
        git_pid = int(read_text(hold))
        moved = [record for record in runner.state()["records"] if record["checkout"] == b]
        if len(moved) != 1 or moved[0]["phase"] != "acquiring":
            fail(f"slot was not moved to B before the gate: {runner.state()['records']}")
        child.kill()
        child.wait(timeout=20)
        log.close()
        # The held wrapper is this proof's own orphaned process.
        os.kill(git_pid, signal.SIGKILL)
        wait_gone(os.kill, git_pid, "gate-hold git wrapper")
    finally:
        os.remove(wrapper)
    result, units, stderr = test(b, "retry after a crash in the gate")
    if result["slot"] != slot or result["warmth"] != "warm":
        fail(f"retry did not reuse slot {slot} warm: {result}")
    if f"recovered slot {slot} after oxide pid {child.pid} died (Cargo never started)" not in stderr:
        fail(f"retry did not recover the crashed gate's slot: {stderr[-800:]}")
    if "freshness gate refreshed" not in stderr:
        fail(f"retry skipped the freshness gate: {stderr[-800:]}")
    if units.get("marker:build") is not False or units.get("content:test") is not False:
        fail(f"retry did not rebuild the stale units: {units}")


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
    wait_for(
        lambda: any(
            line.startswith("oxide: waiting for a build slot (1 of 1 ")
            for line in oxide_waits(read_log(waiter_log))
        ),
        "host-limit wait line",
    )
    queued, queued_log = runner.start(app2, ["check"], "queued", limit)
    wait_for(
        lambda: "oxide: waiting for earlier oxide commands (position 2); Ctrl-C cancels"
        in oxide_waits(read_log(queued_log)),
        "FIFO wait line",
    )
    if len(oxide_waits(read_log(waiter_log))) != 1:
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
    detached = {record["id"] for record in runner.state()["records"] if record["checkout"] is None}
    holder, holder_log = runner.start(app, ["test"], "hold-clean", {"OXIDE_FIXTURE_HOLD": pidfile})
    wait_for(lambda: os.path.exists(pidfile), "test held during clean")
    cleaner, clean_log = runner.start(app, ["clean"], "clean-wait")
    wait_for(
        lambda: "oxide: waiting for active oxide commands in this checkout; Ctrl-C cancels"
        in oxide_waits(read_log(clean_log)),
        "clean wait line",
    )
    if cleaner.poll() is not None or not os.path.isdir(os.path.join(app, "target")):
        fail("clean ran while a command in its checkout was active")
    if any(record["checkout"] is None and record["id"] not in detached for record in runner.state()["records"]):
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


def probe(call, target: int) -> str:
    """`present`, or the errno name that `call(target, 0)` failed with.
    macOS `kill(-pgid, 0)` reports EPERM while a group holds only unreaped
    zombies and ESRCH once they are reaped, so only ESRCH means gone."""
    try:
        call(target, 0)
    except OSError as error:
        return errno.errorcode.get(error.errno, str(error.errno))
    return "present"


def process_alive(pid: int) -> bool:
    return probe(os.kill, pid) == "present"


def wait_gone(call, target: int, label: str, seconds: float = 20) -> None:
    """Wait, within a finite bound, for ESRCH. The killed processes are
    reparented to launchd, which reaps them; EPERM (zombie-only or a real
    denial) and `present` both count as not gone."""
    deadline = time.monotonic() + min(seconds, remaining())
    last = probe(call, target)
    while last != "ESRCH" and time.monotonic() < deadline:
        time.sleep(0.1)
        last = probe(call, target)
    if last != "ESRCH":
        fail(f"{label}: {call.__name__}({target}, 0) still {last} after {seconds:.0f} s, not ESRCH")


def prove_interrupt(runner: Runner, app: str) -> None:
    """Ctrl-C reaches Cargo's whole group, then the slot is released."""
    pidfile = os.path.join(runner.parent, "hold-int.pid")
    child, log = runner.start(app, ["test"], "interrupt", {"OXIDE_FIXTURE_HOLD": pidfile})
    wait_for(lambda: os.path.exists(pidfile), "interruptible test")
    with open(pidfile, encoding="utf-8") as handle:
        test_pid = int(handle.read())
    child.send_signal(signal.SIGINT)
    expect(child.wait(timeout=20), 130, "interrupted test", read_log(log))
    wait_gone(os.kill, test_pid, "test process after Ctrl-C forwarding")
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
        fail(f"Cargo's group did not survive the killed oxide: kill({test_pid}, 0) {probe(os.kill, test_pid)}")
    running = [record for record in runner.state()["records"] if record["phase"] == "running"]
    if len(running) != 1 or not running[0]["process_group"]:
        fail(f"crashed record: {runner.state()['records']}")
    group = running[0]["process_group"]
    holder = running[0].get("holder") or {}
    if holder.get("pid") != child.pid or not holder.get("started_ms") or not running[0].get("lease_id"):
        fail(f"crashed record lacks its holder or lease: {running[0]}")
    held = [slot for slot in runner.lease_status()["slots"] if slot["lease_id"] == running[0]["lease_id"]]
    if len(held) != 1 or held[0]["state"] != "held":
        fail(f"crashed lease is not held in the ledger: {held}")
    code, stdout, _ = runner.run(app, ["status"])
    if f"held by an interrupted command (oxide pid {child.pid}, Cargo group {group};" not in stdout:
        fail(f"status did not show the interrupted slot: {stdout}")
    if runner.state().get("recoveries"):
        fail("a slot was recovered while Cargo's group survived")
    follower, follower_log = runner.start(app, ["build"], "follower")
    wait_for(
        lambda: "oxide: waiting for another oxide command in this checkout; Ctrl-C cancels"
        in oxide_waits(read_log(follower_log)),
        "follower wait",
    )
    if follower.poll() is not None:
        fail("follower took a slot while Cargo's group survived")
    with open(pidfile.replace(".pid", ".release"), "w", encoding="utf-8"):
        pass
    expect(follower.wait(timeout=min(60, remaining())), 0, "follower", read_log(follower_log))
    if f"recovered slot {held[0]['slot_id']} after oxide pid {child.pid} died (Cargo group {group} empty)" not in read_log(follower_log):
        fail(f"follower did not report the recovery: {read_log(follower_log)}")
    # Recovery itself required killpg ESRCH, so the test process is reaped.
    if probe(os.kill, test_pid) != "ESRCH":
        fail(f"crash test process outlived recovery: kill({test_pid}, 0) {probe(os.kill, test_pid)}")
    recoveries = runner.state().get("recoveries", [])
    if len(recoveries) != 1 or recoveries[0]["lease_id"] != running[0]["lease_id"] or recoveries[0]["process_group"] != group:
        fail(f"recovery log: {recoveries}")
    follower_log.close()


def prove_group_gone(runner: Runner, app: str) -> None:
    """SIGKILL of oxide and its whole Cargo group: the next command recovers
    at once and status shows it. A `setsid` escapee is outside the group, so
    it does not keep the slot held: the documented residual limit."""
    pidfile = os.path.join(runner.parent, "hold-gone.pid")
    child, log = runner.start(
        app, ["test"], "gone", {"OXIDE_FIXTURE_HOLD": pidfile, "OXIDE_FIXTURE_ESCAPE": sys.executable}
    )
    escape_file = pidfile.replace(".pid", ".escape")
    wait_for(lambda: os.path.exists(pidfile) and os.path.exists(escape_file), "group-gone test")
    with open(escape_file, encoding="utf-8") as handle:
        escapee = int(handle.read())
    try:
        child.kill()
        child.wait(timeout=20)
        log.close()
        running = [record for record in runner.state()["records"] if record["phase"] == "running"]
        if len(running) != 1:
            fail(f"group-gone record: {runner.state()['records']}")
        group = running[0]["process_group"]
        try:
            os.killpg(group, signal.SIGKILL)
        except OSError as error:
            fail(f"killpg({group}, SIGKILL) on the live Cargo group failed: {errno.errorcode.get(error.errno)}")
        wait_gone(os.killpg, group, "killed Cargo group")
        if not process_alive(escapee) or os.getpgid(escapee) == group:
            fail(f"the setsid escapee did not leave Cargo's group alive: kill({escapee}, 0) {probe(os.kill, escapee)}")
        code, _, stderr = runner.run(app, ["build"])
        expect(code, 0, "build after a dead group", stderr)
        if oxide_waits(stderr):
            fail(f"build after a dead group waited: {stderr}")
        if f"after oxide pid {child.pid} died (Cargo group {group} empty)" not in stderr:
            fail(f"build after a dead group did not recover: {stderr}")
        if not process_alive(escapee):
            fail("escapee ended before recovery; the limit was not shown")
        recoveries = runner.state().get("recoveries", [])
        if len(recoveries) != 2 or recoveries[-1]["holder_pid"] != child.pid:
            fail(f"recovery log after a dead group: {recoveries}")
        code, stdout, _ = runner.run(app, ["status"])
        if stdout.count("  recovered slot ") != 2 or f"oxide pid {child.pid} died (Cargo group {group} empty)" not in stdout:
            fail(f"status did not show the recoveries: {stdout}")
    finally:
        # The escapee is this proof's own process; it holds nothing in the pool.
        try:
            os.kill(escapee, signal.SIGKILL)
        except ProcessLookupError:
            pass


def budget_summary(stderr: str, label: str) -> dict:
    found = summary(stderr, label)
    if not re.search(r"pool \d+ MiB allocated of \d+ MiB budget \(directory-budget-v1, budget-only\)", stderr):
        fail(f"{label}: summary does not label the budget-only pool: {stderr[-1200:]!r}")
    if "MiB used" in stderr or "ceiling" in stderr.replace("no hard ceiling", ""):
        fail(f"{label}: a budget-only pool claimed a ceiling: {stderr[-1200:]!r}")
    return found


def write_reading(path: str, value: int) -> None:
    with open(path + ".tmp", "w", encoding="utf-8") as handle:
        handle.write(str(value))
    os.rename(path + ".tmp", path)


RESERVE_STOP = re.compile(
    r"oxide: reserve stop \(injected test readings\): pool filesystem available (\d+) bytes fell below "
    r"the (\d+) byte trigger \(20 GiB reserve \+ 2 GiB margin\); Cargo group (\d+): (TERM(?:, then KILL after (\d+) ms)?); "
    r"sampled minimum (\d+) bytes, overshoot past trigger (\d+) bytes, stop latency (\d+ ms|closure not confirmed); "
    r"budget-only pool: best-effort stop, no guarantee against faster writers or other processes"
)


def reserve_stop(stderr: str, label: str) -> dict:
    match = RESERVE_STOP.search(stderr)
    if not match:
        fail(f"{label}: no reserve stop line in {stderr[-2000:]!r}")
    return {
        "triggering_sample": int(match.group(1)),
        "trigger": int(match.group(2)),
        "group": int(match.group(3)),
        "signals": match.group(4),
        "kill_after_ms": int(match.group(5)) if match.group(5) else None,
        "sampled_minimum": int(match.group(6)),
        "overshoot": int(match.group(7)),
        "latency": match.group(8),
    }


def running_group(runner: Runner) -> tuple:
    running = [record for record in runner.state()["records"] if record["phase"] == "running"]
    if len(running) != 1 or not running[0]["process_group"]:
        fail(f"running record: {runner.state()['records']}")
    return running[0]["process_group"], running[0]


def prove_budget(runner: Runner, tiny: Runner, app: str) -> None:
    """Budget-only directory pools: labelling, the injected-reading watchdog
    (stop, positive closure then release; unconfirmed closure holds) and
    budget admission with whole-idle-slot eviction. The standalone CLI keeps
    refusing the directory pool."""
    readings = os.path.join(runner.parent, "watchdog.readings")
    write_reading(readings, TRIGGER_BYTES + WRITER_HEADROOM_BYTES)
    watched = {"OXIDE_TEST_WATCHDOG_READINGS": readings}
    code, _, stderr = runner.run(app, ["test"], watched)
    expect(code, 0, "budget pool first test", stderr)
    if f"oxide: creating the build pool at {runner.pool} (24576 MiB budget, directory-budget-v1, {BUDGET_ONLY})" not in stderr:
        fail(f"budget pool creation was not labelled: {stderr[-1200:]!r}")
    if os.path.exists(os.path.join(runner.pool, "image.sparseimage")) or os.path.ismount(os.path.join(runner.pool, "payload")):
        fail("the budget pool created an image or a mount")
    first = budget_summary(stderr, "budget first test")
    if first["warmth"] != "cold":
        fail(f"budget first test {first}")
    code, stdout, stderr = runner.run(app, ["status"])
    expect(code, 0, "budget status", stderr)
    for needle in (
        f"  backend directory-budget-v1: {BUDGET_ONLY}",
        "  budget 24576 MiB, ",
        "  reserve watchdog: stops a build below 22 GiB available (20 GiB reserve + 2 GiB margin), best effort",
        f"slot {first['slot']} idle quick {app}",
    ):
        if needle not in stdout:
            fail(f"budget status missing {needle!r}: {stdout}")
    if "  cap " in stdout or "MiB used" in stdout:
        fail(f"budget status claimed a cap: {stdout}")
    code, _, stderr = runner.run(
        runner.parent, ["--pool", runner.pool, "status", "--json"], program=runner.lease
    )
    expect(code, 4, "standalone CLI on a budget pool", stderr)
    if "backing_mismatch" not in stderr:
        fail(f"standalone CLI refusal reason: {stderr!r}")

    # Reserve stop: a bounded writer ignores TERM; injected readings fall
    # with its own writes below the trigger. KILL closes the group, then the
    # slot is released normally.
    out = os.path.join(runner.parent, "writer.bin")
    writer = {
        **watched,
        "OXIDE_FIXTURE_WRITER": sys.executable,
        "OXIDE_FIXTURE_WRITER_OUT": out,
        "OXIDE_FIXTURE_WRITER_READINGS": readings,
        "OXIDE_FIXTURE_WRITER_BASE": str(TRIGGER_BYTES + WRITER_HEADROOM_BYTES),
        "OXIDE_FIXTURE_WRITER_BOUND": str(WRITER_BOUND_BYTES),
    }
    started = time.monotonic()
    code, _, stderr = runner.run(app, ["test"], writer)
    elapsed = time.monotonic() - started
    if code == 0:
        fail(f"a reserve stop reported success: {stderr[-1200:]!r}")
    stop = reserve_stop(stderr, "reserve stop")
    if stop["trigger"] != TRIGGER_BYTES or stop["triggering_sample"] >= TRIGGER_BYTES:
        fail(f"reserve stop trigger {stop}")
    if stop["kill_after_ms"] is None or not stop["latency"].endswith(" ms"):
        fail(f"a writer ignoring TERM was not killed and closed: {stop}")
    if stop["overshoot"] != TRIGGER_BYTES - stop["sampled_minimum"]:
        fail(f"overshoot arithmetic {stop}")
    if "could not confirm" in stderr:
        fail(f"closure was confirmed but the slot was held: {stderr[-1200:]!r}")
    if probe(os.killpg, stop["group"]) != "ESRCH":
        fail(f"the stopped group survived: {probe(os.killpg, stop['group'])}")
    budget_summary(stderr, "reserve stop")
    written = os.path.getsize(out)
    os.remove(out)
    print(
        "oxide budget watchdog (injected readings): triggering sample "
        f"{stop['triggering_sample']} bytes, at-detection overshoot {TRIGGER_BYTES - stop['triggering_sample']} bytes, "
        f"sampled minimum {stop['sampled_minimum']} bytes, overshoot past trigger {stop['overshoot']} bytes "
        f"({stop['overshoot'] // (1024 * 1024)} MiB), {stop['signals']}, stop latency {stop['latency']}, "
        f"writer wrote {written // (1024 * 1024)} MiB, command {elapsed:.1f} s"
    )
    code, stdout, _ = runner.run(app, ["status"])
    if f"slot {first['slot']} idle quick {app}" not in stdout or "interrupted" in stdout:
        fail(f"slot not released after positive closure: {stdout}")

    # Unconfirmed closure: a group member this script never reaps stays a
    # zombie, so killpg never reports ESRCH. The lease and slot stay held
    # until the group empties, then the next command recovers it.
    write_reading(readings, TRIGGER_BYTES + WRITER_HEADROOM_BYTES)
    pidfile = os.path.join(runner.parent, "hold-reserve.pid")
    child, log = runner.start(app, ["test"], "reserve-hold", {**watched, "OXIDE_FIXTURE_HOLD": pidfile})
    wait_for(lambda: os.path.exists(pidfile), "reserve hold test")
    group, record = running_group(runner)
    member = subprocess.Popen(
        ["/bin/sleep", "60"], preexec_fn=lambda: os.setpgid(0, group)
    )
    try:
        if os.getpgid(member.pid) != group:
            fail("the unreaped member did not join Cargo's group")
        write_reading(readings, TRIGGER_BYTES - 1)
        code = child.wait(timeout=min(60, remaining()))
        stderr = read_log(log)
        log.close()
        expect(code, 1, "unconfirmed closure", stderr)
        held_stop = reserve_stop(stderr, "unconfirmed closure")
        if held_stop["latency"] != "closure not confirmed" or held_stop["group"] != group:
            fail(f"unconfirmed closure report {held_stop}")
        if f"could not confirm Cargo group {group} closed after the reserve stop; slot {first['slot']} stays held" not in stderr:
            fail(f"unconfirmed closure did not hold the slot: {stderr[-1200:]!r}")
        code, stdout, _ = runner.run(app, ["status"])
        if f"held by an interrupted command (oxide pid {child.pid}, Cargo group {group}; recovered once the group is empty)" not in stdout:
            fail(f"status did not show the held slot: {stdout}")
    finally:
        member.kill()
        member.wait(timeout=20)
    wait_gone(os.killpg, group, "reserve-stopped group after reaping")
    write_reading(readings, TRIGGER_BYTES + WRITER_HEADROOM_BYTES)
    code, _, stderr = runner.run(app, ["build"], watched)
    expect(code, 0, "build after the held reserve stop", stderr)
    if f"recovered slot {first['slot']} after oxide pid {child.pid} died (Cargo group {group} empty)" not in stderr:
        fail(f"held slot was not recovered once the group emptied: {stderr[-1200:]!r}")
    if not any(entry["lease_id"] == record["lease_id"] for entry in runner.state().get("recoveries", [])):
        fail("the held reserve-stop slot recovery was not logged")
    print(f"oxide budget watchdog held unconfirmed closure: {held_stop['signals']}, slot {first['slot']} held until group {group} emptied")

    # Admission and eviction on a tiny budget: nothing stops a build past
    # it, and the next new slot evicts the idle one whole.
    code, _, stderr = tiny.run(app, ["build"], watched)
    expect(code, 0, "tiny budget build", stderr)
    if f"oxide: creating the build pool at {tiny.pool} (1 MiB budget, directory-budget-v1, {BUDGET_ONLY})" not in stderr:
        fail(f"tiny budget creation: {stderr[-1200:]!r}")
    quick = budget_summary(stderr, "tiny quick build")
    code, stdout, _ = tiny.run(app, ["status"])
    if f"slot {quick['slot']} idle quick {app}" not in stdout or "  budget 1 MiB, " not in stdout:
        fail(f"tiny budget status: {stdout}")
    slots_root = os.path.join(tiny.pool, "payload", "slots")
    if not os.listdir(os.path.join(slots_root, quick["slot"], "build")):
        fail("tiny budget slot is empty")
    code, _, stderr = tiny.run(app, ["check", "--profile", "debug"], watched)
    expect(code, 0, "tiny budget second key", stderr)
    debug = budget_summary(stderr, "tiny debug check")
    if debug["slot"] == quick["slot"] or debug["warmth"] != "cold":
        fail(f"tiny budget second key {debug} vs {quick}")
    if os.path.exists(os.path.join(slots_root, quick["slot"])):
        fail("the idle slot over budget was not evicted whole")
    if sorted(os.listdir(slots_root)) != [debug["slot"]]:
        fail(f"tiny budget slots {os.listdir(slots_root)}")
    print(f"oxide budget admission: slot {quick['slot']} over the 1 MiB budget evicted whole for slot {debug['slot']}")


LINKED_FIXTURE = {
    "Cargo.toml": '[package]\nname = "linked"\nversion = "0.1.0"\nedition = "2021"\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    # The build script writes one 64 KiB file into its OUT_DIR, inside the
    # slot, and gives it a second hard-linked name.
    "build.rs": """use std::path::Path;
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let first = Path::new(&out).join("pair-a.bin");
    let second = Path::new(&out).join("pair-b.bin");
    std::fs::write(&first, vec![7u8; 65536]).unwrap();
    let _ = std::fs::remove_file(&second);
    std::fs::hard_link(&first, &second).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
""",
    "src/lib.rs": "pub fn linked() -> u8 {\n    7\n}\n",
}
SIZE_LINE = re.compile(
    r"(\d+) (KiB|MiB) allocated \(hard links counted once\), (\d+) files \((\d+) distinct\)(, partial)?"
)


def own_walk(root: str) -> dict:
    """Read-only reference walk: file names, distinct inodes, allocation
    once per inode and per name."""
    names = 0
    inodes = {}
    by_name = 0
    pairs = {}
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in files:
            metadata = os.lstat(os.path.join(current, name))
            if not os.path.isfile(os.path.join(current, name)) or os.path.islink(os.path.join(current, name)):
                continue
            names += 1
            by_name += metadata.st_blocks * 512
            key = (metadata.st_dev, metadata.st_ino)
            inodes[key] = metadata.st_blocks * 512
            if name in ("pair-a.bin", "pair-b.bin"):
                pairs[name] = (key, metadata.st_nlink)
    return {"files": names, "distinct": len(inodes), "allocated": sum(inodes.values()), "by_name": by_name, "pairs": pairs}


def check_size(line: str, walk: dict, label: str) -> None:
    match = SIZE_LINE.search(line)
    if not match or match.group(5):
        fail(f"{label}: no complete size report in {line!r}")
    unit = 1024 if match.group(2) == "KiB" else 1024 * 1024
    reported = (int(match.group(1)), int(match.group(3)), int(match.group(4)))
    wanted = (walk["allocated"] // unit, walk["files"], walk["distinct"])
    if reported != wanted:
        fail(f"{label}: reported {reported}, own walk {wanted} ({line!r})")


def safe_readings(runner: Runner) -> dict:
    """Injected watchdog readings well above the trigger, so a host with
    little free space never stops these directory-pool builds."""
    readings = os.path.join(runner.parent, f"{os.path.basename(runner.pool)}.readings")
    write_reading(readings, TRIGGER_BYTES + WRITER_HEADROOM_BYTES)
    return {"OXIDE_TEST_WATCHDOG_READINGS": readings}


def prove_slot_files(runner: Runner) -> None:
    """`status` reports each slot's file names, distinct inodes and
    allocation counted once per inode; a hard-linked pair counts once."""
    linked = os.path.join(runner.parent, "checkouts", "linked")
    write_fixture(linked, LINKED_FIXTURE)
    commit_lock(runner, linked)
    code, _, stderr = runner.run(linked, ["build"], safe_readings(runner))
    expect(code, 0, "linked build", stderr)
    slot = budget_summary(stderr, "linked build")["slot"]
    walk = own_walk(os.path.join(runner.pool, "payload", "slots", slot))
    pairs = walk["pairs"]
    if len(pairs) != 2 or pairs["pair-a.bin"][0] != pairs["pair-b.bin"][0] or pairs["pair-a.bin"][1] != 2:
        fail(f"the hard-linked pair was not built in the slot: {pairs}")
    if walk["files"] <= walk["distinct"] or walk["allocated"] >= walk["by_name"]:
        fail(f"the slot walk shows no shared inode: {walk}")
    code, stdout, stderr = runner.run(linked, ["status"])
    expect(code, 0, "linked status", stderr)
    line = next((entry for entry in stdout.splitlines() if entry.startswith(f"  slot {slot} idle quick {linked} ")), None)
    if line is None:
        fail(f"status has no line for slot {slot}: {stdout}")
    check_size(line, walk, "slot size")
    print(
        f"oxide slot accounting: slot {slot} {walk['files']} files, {walk['distinct']} distinct, "
        f"{walk['allocated']} bytes allocated once per inode ({walk['by_name']} per name)"
    )


def prove_pool_cap(runner: Runner, app: str) -> None:
    """OXIDE_POOL_CAP: validated before effects, applied at creation only,
    a conflicting value reported for an existing pool."""
    watched = safe_readings(runner)
    code, _, stderr = runner.run(app, ["build"], {**watched, "OXIDE_POOL_CAP": "64MB"})
    expect(code, 1, "invalid OXIDE_POOL_CAP", stderr)
    if "OXIDE_POOL_CAP=64MB is not a byte count" not in stderr or os.path.lexists(runner.pool):
        fail(f"invalid OXIDE_POOL_CAP was not refused before creation: {stderr!r}")
    code, _, stderr = runner.run(app, ["build"], {**watched, "OXIDE_POOL_CAP": "64MiB"})
    expect(code, 0, "OXIDE_POOL_CAP create", stderr)
    if f"oxide: creating the build pool at {runner.pool} (64 MiB budget, directory-budget-v1, {BUDGET_ONLY})" not in stderr:
        fail(f"OXIDE_POOL_CAP was not used on creation: {stderr[-1200:]!r}")
    code, _, stderr = runner.run(app, ["build"], {**watched, "OXIDE_POOL_CAP": "128MiB"})
    expect(code, 0, "OXIDE_POOL_CAP existing pool", stderr)
    if f"oxide: OXIDE_POOL_CAP 128 MiB not applied: the pool at {runner.pool} keeps its recorded 64 MiB budget" not in stderr:
        fail(f"conflicting OXIDE_POOL_CAP was not reported: {stderr[-1200:]!r}")
    code, stdout, _ = runner.run(app, ["status"])
    if "  budget 64 MiB, " not in stdout:
        fail(f"the existing pool's budget changed: {stdout}")
    print("oxide pool cap: 64 MiB applied on creation; 128 MiB reported and not applied to the existing pool")


CFG_IF_CHECKSUM = "baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd"
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"


def cache_fixture(root: str, name: str, dependencies: str, lock: str | None, files: dict | None = None) -> str:
    path = os.path.join(root, name)
    write_fixture(path, {
        "Cargo.toml": f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n{dependencies}',
        "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
        "src/lib.rs": "",
        **(files or {}),
    })
    if lock is not None:
        write_fixture(path, {"Cargo.lock": f"version = 4\n\n{lock}"})
    return path


def lock_package(name: str, version: str, source: str | None = None, checksum: str | None = None, deps: list | None = None) -> str:
    text = f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
    if source:
        text += f'source = "{source}"\n'
    if checksum:
        text += f'checksum = "{checksum}"\n'
    if deps:
        text += "dependencies = [\n" + "".join(f' "{dep}",\n' for dep in deps) + "]\n"
    return text + "\n"


class Listener:
    """A tiny owned TCP control: counts every connection and closes it."""

    def __init__(self):
        import socket
        import threading
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen(16)
        self.port = self.server.getsockname()[1]
        self.connections = 0
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self) -> None:
        while True:
            try:
                peer, _ = self.server.accept()
            except OSError:
                return
            self.connections += 1
            peer.close()

    def close(self) -> None:
        self.server.close()


def prove_cargo_cache(runner: Runner) -> None:
    """Cargo home reporting and `clean --cargo-cache` on one fresh private
    CARGO_HOME. Setup downloads exactly one tiny archive (cfg-if 1.0.0);
    every later Cargo HTTP request goes through Oxide's closed proxy."""
    root = os.path.join(runner.parent, "cache")
    home = os.path.join(root, "cargo-home")
    os.makedirs(home, mode=0o700)
    os.chmod(root, 0o700)
    home = os.path.realpath(home)
    no_pool = os.path.join(root, "never-created-pool")
    env = dict(runner.env)
    for name in list(env):
        if name.startswith(("CARGO_NET_", "CARGO_REGISTR", "CARGO_SOURCE_", "CARGO_UNSTABLE_", "CARGO_HTTP_")) or name.lower().endswith("_proxy"):
            env.pop(name)
    env.update({"CARGO_HOME": home, "OXIDE_POOL": no_pool})

    def call(cwd: str, args: list, extra: dict | None = None, program=None, timeout: float = 60):
        result = subprocess.run(
            [program or runner.oxide, *args],
            cwd=cwd,
            env={**env, **(extra or {})},
            capture_output=True,
            text=True,
            timeout=min(timeout, remaining()),
        )
        return result.returncode, result.stdout, result.stderr

    offline = {"CARGO_NET_OFFLINE": "true"}
    cfg_if = lock_package("cfg-if", "1.0.0", CRATES_IO_SOURCE, CFG_IF_CHECKSUM)
    good = cache_fixture(root, "cache-good", 'cfg-if = "=1.0.0"\n', None)
    deny = cache_fixture(
        root, "cache-deny", 'cfg-if = "=1.0.0"\nitoa = "=1.0.11"\n',
        cfg_if + lock_package("cache-deny", "0.1.0", deps=["cfg-if", "itoa"])
        + lock_package("itoa", "1.0.11", CRATES_IO_SOURCE, "0" * 64),
    )
    ssh = cache_fixture(
        root, "cache-ssh", 'remote = { git = "ssh://git@example.invalid/remote.git" }\n',
        lock_package("cache-ssh", "0.1.0", deps=["remote"])
        + lock_package("remote", "0.1.0", "git+ssh://git@example.invalid/remote.git#" + "1" * 40),
    )
    custom = cache_fixture(
        root, "cache-custom", 'cfg-if = "=1.0.0"\n',
        cfg_if + lock_package("cache-custom", "0.1.0", deps=["cfg-if"]),
        {".cargo/config.toml": '[source.crates-io]\nreplace-with = "mirror"\n\n[source.mirror]\nregistry = "sparse+https://mirror.invalid/index/"\n'},
    )
    # A stale lock: only crates.io in the lock, an SSH git dependency in the
    # manifest. A single-quoted lock source is the same route.
    stale = cache_fixture(
        root, "cache-stale", 'cfg-if = "=1.0.0"\nremote = { git = "ssh://git@127.0.0.1:9/remote.git" }\n',
        cfg_if + lock_package("cache-stale", "0.1.0", deps=["cfg-if"]),
    )
    quoted = cache_fixture(
        root, "cache-quoted", 'remote = { git = "ssh://git@127.0.0.1:9/remote.git" }\n',
        lock_package("cache-quoted", "0.1.0", deps=["remote"])
        + "[[package]]\nname = \"remote\"\nversion = \"0.1.0\"\nsource = 'git+ssh://git@127.0.0.1:9/remote.git#" + "1" * 40 + "'\n",
    )
    for path in (good, deny, ssh, custom, stale, quoted):
        code, _, stderr = call(path, ["init"], offline)
        expect(code, 0, f"adopt {os.path.basename(path)}", stderr)

    def refused(cwd: str, reason: str, label: str, extra: dict | None = None) -> None:
        code, _, stderr = call(cwd, ["clean", "--cargo-cache"], extra)
        expect(code, 1, label, stderr)
        if "oxide: clean --cargo-cache refused: " not in stderr or reason not in stderr or "asking Cargo to clean" in stderr:
            fail(f"{label}: refusal did not name {reason!r}: {stderr!r}")

    refused(good, "Cargo.lock is missing", "missing lockfile")
    refused(ssh, "SSH and other non-HTTP git transports cannot be contained", "SSH git source")
    refused(quoted, "Cargo.lock source git+ssh://", "single-quoted SSH git source")
    refused(custom, f'Cargo config file {custom}/.cargo/config.toml sets key "source"', "custom source")

    # Setup: the one allowed download into the private home.
    write_fixture(good, {"Cargo.lock": "version = 4\n\n" + cfg_if + lock_package("cache-good", "0.1.0", deps=["cfg-if"])})
    fetched = subprocess.run(
        ["cargo", "fetch", "--locked"], cwd=good, env=env, capture_output=True, text=True, timeout=min(90, remaining())
    )
    expect(fetched.returncode, 0, "setup fetch (cfg-if 1.0.0)", fetched.stderr)
    archives = []
    for current, _, files in os.walk(os.path.join(home, "registry", "cache")):
        archives += [os.path.join(current, name) for name in files if name.endswith(".crate")]
    if [os.path.basename(path) for path in archives] != ["cfg-if-1.0.0.crate"]:
        fail(f"setup downloaded {archives}")
    import hashlib
    with open(archives[0], "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    if digest != CFG_IF_CHECKSUM:
        fail(f"cfg-if archive sha256 {digest}")
    print(f"oxide cargo-cache setup: 1 archive, cfg-if 1.0.0 from crates.io, {os.path.getsize(archives[0])} bytes, sha256 {digest}")

    refused(good, "CARGO_NET_GIT_FETCH_WITH_CLI is set: net.git-fetch-with-cli", "git-fetch-with-cli", {"CARGO_NET_GIT_FETCH_WITH_CLI": "true"})
    write_fixture(ssh, {"Cargo.lock": "version = 4\n\n" + cfg_if + lock_package("cache-ssh", "0.1.0", deps=["cfg-if"])
                        + lock_package("mine", "0.1.0", "sparse+https://registry.invalid/index/")})
    refused(ssh, "a custom registry the closed proxy is not proven to cover", "custom registry")
    refused(stale, "Cargo cannot resolve this workspace offline from Cargo.lock", "stale lock with an SSH dependency")

    code, stdout, stderr = call(good, ["status"])
    expect(code, 0, "cargo home status", stderr)
    line = next((entry for entry in stdout.splitlines() if entry.startswith(f"cargo home: {home} ")), None)
    if line is None or "Cargo cleans it itself only at the end of an online build or fetch" not in stdout:
        fail(f"status did not report the Cargo home: {stdout}")
    check_size(line, own_walk(home), "cargo home size")
    if re.search(r"last (clean|gc|auto)", stdout, re.IGNORECASE):
        fail(f"status claimed a last-cleanup time: {stdout}")

    # The fresh database primed its last pass at setup; the gate needs a
    # later whole second.
    time.sleep(1.2)
    code, _, stderr = call(good, ["clean", "--cargo-cache"])
    expect(code, 0, "cargo cleanup", stderr)
    for needle in ("every HTTP request through closed proxy 127.0.0.1:", "oxide: Cargo cleanup ran (", "oxide: cargo home before: ", "oxide: cargo home after: "):
        if needle not in stderr:
            fail(f"cargo cleanup output missing {needle!r}: {stderr!r}")
    if " TRACE gc:" in stderr:
        fail("Cargo's gc trace leaked to the caller")
    if not re.search(r"oxide: closed proxy 127\.0\.0\.1:\d+ closed 0 connection\(s\)", stderr):
        fail(f"a complete cached lockfile still reached for the network: {stderr!r}")

    # Cargo's own lock-contention skip: hold the mutate lock shared, as a
    # concurrent Cargo reading the cache would.
    import fcntl
    time.sleep(1.2)
    with open(os.path.join(home, ".package-cache-mutate"), "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_SH)
        code, _, stderr = call(good, ["clean", "--cargo-cache"], timeout=45)
    expect(code, 75, "cargo cleanup under a held cache lock", stderr)
    if "oxide: Cargo skipped cleanup: another Cargo holds the package cache lock" not in stderr or "cleanup ran" in stderr:
        fail(f"lock skip was not labelled: {stderr!r}")

    # Enforced denial: an uncached crate needs the network. A Cargo proxy
    # and the libcurl proxy variables point at an owned listener; Oxide's
    # closed port must win and the listener must see nothing.
    listener = Listener()
    control = f"http://127.0.0.1:{listener.port}"
    try:
        code, _, stderr = call(deny, ["clean", "--cargo-cache"], {
            "CARGO_HTTP_PROXY": control, "http_proxy": control, "HTTPS_PROXY": control,
            "https_proxy": control, "ALL_PROXY": control, "CARGO_NET_RETRY": "0",
            # Debug-only hook: the offline-resolution gate would refuse this
            # uncached crate first; skipping it lets the fetch meet the proxy.
            "OXIDE_TEST_SKIP_OFFLINE_RESOLUTION": "1",
        })
    finally:
        listener.close()
    closed = re.search(r"oxide: closed proxy 127\.0\.0\.1:(\d+) closed (\d+) connection\(s\)", stderr)
    if code in (0, 75) or not closed or "oxide: Cargo cleanup failed: cargo fetch exited" not in stderr:
        fail(f"uncached fetch was not a labelled failure: exit {code} {stderr!r}")
    if int(closed.group(2)) < 1 or int(closed.group(1)) == listener.port:
        fail(f"Cargo's request did not go to Oxide's closed proxy: {stderr!r}")
    if listener.connections:
        fail(f"the owned control listener saw {listener.connections} connection(s)")
    if any(name.startswith("itoa") for _, _, files in os.walk(home) for name in files):
        fail("the denied crate reached the Cargo home")
    if os.path.lexists(no_pool):
        fail("Cargo home commands created a pool")
    print("oxide cargo-cache: refusals named, cleanup ran, lock skip labelled, closed proxy denied an uncached fetch (owned control saw 0 connections)")


def take_standalone_lease(runner: Runner) -> dict:
    """A lease from the standalone CLI has no oxide record or lock holder."""
    project = os.path.join(runner.parent, "standalone", "project")
    workspace = os.path.join(runner.parent, "standalone", "workspace")
    for path in (project, workspace):
        os.makedirs(path, mode=0o700)
    code, stdout, stderr = runner.run(
        runner.parent,
        ["--pool", runner.pool, "lease", "--project", project, "--workspace", workspace,
         "--profile", "dev", "--holder", "standalone-fixture"],
        program=runner.lease,
    )
    expect(code, 0, "standalone lease", stderr)
    return json.loads(stdout)


def keep_standalone_lease(runner: Runner, app: str, grant: dict) -> None:
    """Crash sweeps ran several times; the standalone lease stays held until
    its own explicit release."""
    held = [slot for slot in runner.lease_status()["slots"] if slot["lease_id"] == grant["lease_id"]]
    if len(held) != 1 or held[0]["state"] != "held" or held[0]["holder"] != "standalone-fixture":
        fail(f"standalone lease was not kept held: {held}")
    if any(entry["lease_id"] == grant["lease_id"] for entry in runner.state().get("recoveries", [])):
        fail("standalone lease was logged as recovered")
    code, stdout, _ = runner.run(app, ["status"])
    if not re.search(rf"slot {held[0]['slot_id']} held \S+ not managed by oxide", stdout):
        fail(f"status did not show the standalone lease as held: {stdout}")
    code, _, stderr = runner.run(
        runner.parent, ["--pool", runner.pool, "release", "--lease", grant["lease_id"]], program=runner.lease
    )
    expect(code, 0, "standalone release", stderr)


# Cargo version matrix (oxide#091): exact stable pins from the official
# channel manifests, `cargo -V` and `rustc -V` first lines. Oxide itself is
# staged with its own 1.97.1; only fixture Cargo runs the tested release.
BUILDER = "1.97.1"
MATRIX = {
    "1.95.0": ("cargo 1.95.0 (f2d3ce0bd 2026-03-21)", "rustc 1.95.0 (59807616e 2026-04-14)"),
    "1.96.1": ("cargo 1.96.1 (356927216 2026-06-26)", "rustc 1.96.1 (31fca3adb 2026-06-26)"),
    "1.97.1": ("cargo 1.97.1 (c980f4866 2026-06-30)", "rustc 1.97.1 (8bab26f4f 2026-07-14)"),
    "1.98.1": ("cargo 1.98.1 (797e8a9bc 2026-08-05)", "rustc 1.98.1 (48a229cea 2026-09-01)"),
    "1.99.1": ("cargo 1.99.1 (5f94df478 2026-08-27)", "rustc 1.99.1 (58cf8f9c3 2026-10-13)"),
}
# Installed below-threshold release for the fallback phase; never installed
# by this proof.
FALLBACK = ("1.94.1", "cargo 1.94.1 (29ea6fb6a 2026-03-24)")


def reserve(parent: str, phase: str, proof: str = "matrix") -> None:
    free = host_free_bytes(parent)
    print(f"{proof} reserve {phase}: host free {free // (1024 * 1024)} MiB")
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve at {phase}")


def toolchain_identity(env: dict, cwd: str) -> tuple:
    lines = []
    for tool in ("cargo", "rustc"):
        result = subprocess.run(
            [tool, "-V"], cwd=cwd, env=env, capture_output=True, text=True, timeout=min(30, remaining())
        )
        expect(result.returncode, 0, f"{tool} -V", result.stderr)
        lines.append(result.stdout.strip())
    return tuple(lines)


def walk_dirs(root: str, names: set) -> list:
    found = []
    for path, dirs, _ in os.walk(root):
        found.extend(os.path.join(path, name) for name in dirs if name in names)
    return found


def decode_dep_info(runner: Runner, decoder: str, root: str, label: str) -> dict:
    code, stdout, stderr = runner.run(runner.parent, [root], program=decoder)
    print(f"matrix dep-info {label}: {stdout.strip()}")
    expect(code, 0, f"dep-info decode {label}", stderr)
    decoded = json.loads(stdout)
    if decoded["files"] < 1 or decoded["decoded"] != decoded["files"]:
        fail(f"dep-info {label} did not decode every file: {decoded}")
    return decoded


def stray_line(runner: Runner, cwd: str, label: str) -> str:
    code, stdout, stderr = runner.run(cwd, ["status"])
    expect(code, 0, f"status {label}", stderr)
    line = next((line for line in stdout.splitlines() if line.startswith("stray Cargo output:")), "")
    print(f"matrix stray {label}: {line}")
    return line


def prove_layout(runner: Runner, decoder: str) -> None:
    """Effective build.build-dir, stray detection and dep-info decoding
    under the tested Cargo, after the freshness scenarios."""
    root = os.path.join(runner.parent, "gated")
    a, c = os.path.join(root, "a"), os.path.join(root, "c")
    # Oxide's commands left only final outputs in the checkout target.
    leaked = [
        path
        for path in walk_dirs(os.path.join(a, "target"), {"deps", "incremental", ".fingerprint"})
        if any(files for _, _, files in os.walk(path))
    ]
    if leaked:
        fail(f"build.build-dir was not effective; intermediates in target: {leaked}")
    slots = os.path.join(runner.pool, "payload", "slots")
    units = walk_dirs(slots, {".fingerprint", "deps"})
    if not any(path.endswith(".fingerprint") for path in units) or not any(path.endswith("deps") for path in units):
        fail(f"slot build directories lack deps/ or .fingerprint/: {units}")
    print(f"matrix build-dir: {len(units)} deps/.fingerprint directories in slots, none in {a}/target")
    if not stray_line(runner, a, "after oxide only").startswith("stray Cargo output: none "):
        fail("verified Cargo with only Oxide output did not report no stray output")
    decode_dep_info(runner, decoder, slots, "slots")
    # Plain Cargo of the tested release in the clone leaves its own layout.
    plain = subprocess.run(
        ["cargo", "check", "--offline"],
        cwd=c,
        env=runner.environment(None),
        capture_output=True,
        text=True,
        timeout=min(120, remaining()),
    )
    expect(plain.returncode, 0, "plain cargo check", plain.stderr)
    line = stray_line(runner, c, "after plain cargo")
    if "holds plain-Cargo deps/ or incremental/" not in line:
        fail(f"plain Cargo output was not reported as stray: {line}")
    decode_dep_info(runner, decoder, os.path.join(c, "target"), "plain target")


def prove_fallback(runner: Runner) -> None:
    """An unverified Cargo keeps its slot with one checkout and reports
    stray output as unknown."""
    old = Runner(runner.oxide, runner.lease, runner.parent, extra={"RUSTUP_TOOLCHAIN": FALLBACK[0]})
    cargo, _ = toolchain_identity(old.env, runner.parent)
    print(f"matrix fallback toolchain: {cargo}")
    if cargo != FALLBACK[1]:
        fail(f"fallback Cargo is not {FALLBACK[1]}: {cargo}")
    root = os.path.join(runner.parent, "gated")
    a, b = os.path.join(root, "a"), os.path.join(root, "b")
    note = f"freshness gate off: {FALLBACK[1]} is not a Cargo release Oxide verified; the slot stays with this checkout"
    code, _, stderr = old.run(a, ["test"])
    expect(code, 0, "fallback A", stderr)
    first = summary(stderr, "fallback A")
    print(f"matrix fallback A: {first}")
    if first["warmth"] != "cold" or note not in stderr:
        fail(f"unverified Cargo did not keep A's new slot affine: {stderr[-800:]}")
    code, _, stderr = old.run(b, ["test"])
    expect(code, 0, "fallback B", stderr)
    second = summary(stderr, "fallback B")
    print(f"matrix fallback B: {second}")
    # Affine slots are never offered to another checkout: B builds cold on
    # its own slot, which also stays affine.
    if note not in stderr or second["warmth"] != "cold" or second["slot"] == first["slot"]:
        fail(f"B reused A's unverified slot or shared its own: {stderr[-800:]}")
    # An existing target with no plain-Cargo intermediates in it.
    os.makedirs(os.path.join(a, "target"), exist_ok=True)
    line = stray_line(old, a, "unverified Cargo")
    if not line.startswith(f"stray Cargo output: unknown in {a}/target ({FALLBACK[1]} is not a Cargo release Oxide verified"):
        fail(f"unverified Cargo did not report stray output as unknown: {line}")


def matrix_main(tested: str) -> int:
    """One logical proof: the shipped freshness gate, stray detection and
    dep-info decoder under one exact Cargo release."""
    global STARTED
    if tested not in MATRIX:
        fail(f"{tested} is not a matrix release: {sorted(MATRIX)}")
    if sys.platform != "darwin":
        fail("the bounded APFS matrix proof requires macOS")
    parent = os.path.join(scratch_root(), "cargo-matrix")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    STARTED = time.monotonic()
    reserve(parent, "admission")
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    stage = dict(os.environ)
    for name in ("CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTC_WRAPPER"):
        stage.pop(name, None)
    # Offline staging with the builder release into this proof's scratch.
    stage.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
        }
    )
    builder = toolchain_identity(stage, repo)
    print(f"matrix builder: {builder[0]} | {builder[1]}")
    if builder != MATRIX[BUILDER]:
        fail(f"builder is not {BUILDER}: {builder}")
    executables = build_binaries(("--offline", "--example", "dep_info_matrix"), stage)
    if "dep_info_matrix" not in executables:
        fail("compiled artifact dep_info_matrix was not reported")
    print(f"matrix staging: {time.monotonic() - STARTED:.0f} s (host Cargo home, offline)")
    reserve(parent, "after staging")
    runner = Runner(executables["oxide"], executables["oxide-storage-lease"], parent, extra={"RUSTUP_TOOLCHAIN": tested})
    identity = toolchain_identity(runner.env, parent)
    print(f"matrix tested: {identity[0]} | {identity[1]}")
    if identity != MATRIX[tested]:
        fail(f"tested toolchain is not {tested}: {identity}")
    code, _, stderr = runner.run(
        parent,
        ["--pool", runner.pool, "init", "--cap-bytes", str(IMAGE_BYTES), "--image-bytes", str(IMAGE_BYTES)],
        program=runner.lease,
    )
    expect(code, 0, "pool init", stderr)
    inputs = sum(len(text.encode()) for fixture in (GATED, OUTSIDE) for text in fixture.values())
    print(f"matrix fixture inputs: {inputs} bytes")
    if inputs > SOURCE_LIMIT_BYTES:
        fail("fixture inputs exceed 8 MiB")
    owned = 0
    try:
        prove_freshness(runner)
        print(f"matrix freshness gate held under {identity[0]}")
        reserve(parent, "after freshness")
        prove_layout(runner, executables["dep_info_matrix"])
        prove_fallback(runner)
        owned = tree_bytes(parent, True)
        print(f"matrix owned scratch {owned // (1024 * 1024)} MiB")
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB")
    except Exception:
        print(f"matrix proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    payload = os.path.join(runner.pool, "payload")
    detach = subprocess.run(["/usr/bin/hdiutil", "detach", payload], capture_output=True, text=True, timeout=60)
    if detach.returncode:
        fail(f"detach failed after positive completion: {detach.stderr}")
    reserve(parent, "after detach")
    elapsed = time.monotonic() - STARTED
    shutil.rmtree(parent)
    print(f"matrix proof held for {tested}: {elapsed:.0f} s, owned scratch {owned // (1024 * 1024)} MiB")
    return 0


# Toolchain-context fixture (oxide#094): a virtual workspace whose root
# selects 1.97.1 and whose `member` selects 1.96.1. Each build script reports
# the Cargo and the `rustc -vV` its build actually used.
CONTEXT_PROBE = """use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").expect("RUSTC");
    let output = Command::new(&rustc).arg("-vV").output().expect("rustc -vV");
    assert!(output.status.success(), "rustc -vV failed");
    let version = String::from_utf8(output.stdout).expect("UTF-8");
    let lines: Vec<&str> = version.lines().collect();
    println!("cargo:rustc-env=PROBE_RUSTC={}", lines.join("|"));
    println!("cargo:rustc-env=PROBE_CARGO={}", std::env::var("CARGO").expect("CARGO"));
    println!("cargo:rerun-if-changed=build.rs");
}
"""
CONTEXT = {
    "Cargo.toml": '[workspace]\nmembers = ["app", "member"]\nresolver = "2"\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "app/Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "app/build.rs": CONTEXT_PROBE,
    "app/src/lib.rs": 'pub const RUSTC: &str = env!("PROBE_RUSTC");\n',
    "member/Cargo.toml": '[package]\nname = "member"\nversion = "0.1.0"\nedition = "2021"\n',
    "member/rust-toolchain.toml": '[toolchain]\nchannel = "1.96.1"\n',
    "member/build.rs": CONTEXT_PROBE,
    "member/src/lib.rs": 'pub const RUSTC: &str = env!("PROBE_RUSTC");\n',
}
# Logs every fixture Cargo process (Oxide's and plain), then runs the real one.
CONTEXT_CARGO = """#!/bin/sh
printf '%s|%s\\n' "$(pwd -P)" "$*" >> "LOG"
exec "REAL_CARGO" "$@"
"""
CONTEXT_CARGO_LIMIT = 32
CONTEXT_LOG_LIMIT_BYTES = 32 * 1024 * 1024


class ContextLedger:
    """Sampled host minimum, owned peak and output bytes for one proof."""

    def __init__(self, parent: str):
        self.parent = parent
        self.free_min = None
        self.owned_peak = 0
        self.log_bytes = 0
        # Library slot label to Oxide record id.
        self.slots = {}

    def sample(self, phase: str) -> None:
        free = host_free_bytes(self.parent)
        owned = tree_bytes(self.parent, True)
        self.free_min = free if self.free_min is None else min(self.free_min, free)
        self.owned_peak = max(self.owned_peak, owned)
        print(f"context sample {phase}: host free {free // (1024 * 1024)} MiB, owned {owned // 1024} KiB")
        if free < HOST_RESERVE_BYTES:
            fail(f"host free {free} is below the 20 GiB reserve at {phase}")
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB at {phase}")

    def emit(self, label: str, stream: str, text: str) -> None:
        self.log_bytes += len(text.encode())
        if self.log_bytes > CONTEXT_LOG_LIMIT_BYTES:
            fail("proof output exceeds 32 MiB")
        print(f"----- {label} {stream} ({len(text.encode())} bytes) -----")
        print(text, end="" if text.endswith("\n") or not text else "\n")


def package_name(package_id: str) -> str:
    """`path+file:///x/app#0.1.0` or `path+file:///x/dir#app@0.1.0`."""
    location, _, fragment = package_id.rpartition("#")
    return fragment.split("@")[0] if "@" in fragment else location.rstrip("/").rsplit("/", 1)[-1]


def context_check(runner: Runner, ledger: ContextLedger, cwd: str, args: list, label: str, extra: dict | None = None) -> dict:
    code, stdout, stderr = runner.run(cwd, ["check", "--message-format=json", *args], extra)
    ledger.emit(label, "stdout", stdout)
    ledger.emit(label, "stderr", stderr)
    expect(code, 0, label, stderr)
    result = summary(stderr, label)
    result["fresh"] = unit_freshness(stdout)
    result["probes"] = {}
    for line in stdout.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") == "build-script-executed":
            env = dict(message.get("env", []))
            if "PROBE_RUSTC" in env:
                result["probes"][package_name(message["package_id"])] = (
                    env["PROBE_CARGO"],
                    "\n".join(env["PROBE_RUSTC"].split("|")) + "\n",
                )
    # Runs are sequential: the record this command used is the latest. Its
    # `id` is Oxide's, not the library slot in the summary; the pairing must
    # stay one-to-one.
    record = max(runner.state()["records"], key=lambda record: record["last_used"])
    paired = ledger.slots.setdefault(result["slot"], record["id"])
    if paired != record["id"] or list(ledger.slots.values()).count(record["id"]) != 1:
        fail(f"{label}: slot {result['slot']} and record {record['id']} do not pair: {ledger.slots}")
    result["record"] = record
    ledger.sample(label)
    print(
        f"context {label}: slot {result['slot']} {result['warmth']}, compatibility {record['compatibility'][:16]}, "
        f"probes { {name: (cargo, rustc.splitlines()[0]) for name, (cargo, rustc) in result['probes'].items()} }"
    )
    return result


def sha256_hex(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def expected_compatibility(identity: str, rustc: str) -> str:
    """identity.rs `SlotKey::resolve` for the quick profile, host target."""
    host = next(line[len("host: "):] for line in rustc.splitlines() if line.startswith("host: "))
    return sha256_hex(f"{identity}\nquick\n{sha256_hex(rustc)}\n{host}\ndefault")


def agree(result: dict, label: str, packages: set, cargo: str | None, rustc: str) -> str:
    """Every probed build used `rustc` (and `cargo` when given), and the
    slot's recorded compatibility digests that same `rustc -vV`. Returns it.
    A fresh build script replays the output of the run that built it."""
    probes = result["probes"]
    if set(probes) != packages:
        fail(f"{label}: probes {sorted(probes)}, expected {sorted(packages)}")
    versions = {probe[1] for probe in probes.values()}
    if len(versions) != 1:
        fail(f"{label}: builds used different compilers: {versions}")
    version = versions.pop()
    for name, (used, _) in probes.items():
        if cargo and f"/toolchains/{cargo}-" not in used:
            fail(f"{label}: {name} built with Cargo {used}, expected {cargo}")
    if version.splitlines()[0] != MATRIX[rustc][1]:
        fail(f"{label}: built with {version.splitlines()[0]}, expected {MATRIX[rustc][1]}")
    record = result["record"]
    if record["compatibility"] != expected_compatibility(record["identity"], version):
        fail(f"{label}: slot compatibility does not digest the rustc the build used")
    return version


def context_main() -> int:
    """oxide#094: slot compatibility follows the compiler the guarded Cargo
    uses in each caller context of one workspace."""
    global STARTED
    STARTED = time.monotonic()
    if sys.platform != "darwin":
        fail("the bounded APFS toolchain-context proof requires macOS")
    rustup_home = os.environ.get("RUSTUP_HOME") or os.path.expanduser("~/.rustup")
    host = "aarch64-apple-darwin" if os.uname().machine == "arm64" else "x86_64-apple-darwin"
    for release in ("1.96.1", "1.97.1"):
        if not os.path.isdir(os.path.join(rustup_home, "toolchains", f"{release}-{host}")):
            fail(f"toolchain {release} is not installed; this proof never installs one")
    real_cargo = shutil.which("cargo")
    if not real_cargo:
        fail("no cargo on PATH")
    parent = os.path.join(scratch_root(), "toolchain-context")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    ledger = ContextLedger(parent)
    ledger.sample("admission")
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    stage = dict(os.environ)
    for name in ("CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTC_WRAPPER"):
        stage.pop(name, None)
    stage.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
        }
    )
    # Setup requests: the source commit and the builder's two versions.
    source = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True, timeout=30)
    expect(source.returncode, 0, "source commit", source.stderr)
    builder = toolchain_identity(stage, repo)
    print(f"context run {os.getpid()} source {source.stdout.strip()} builder {builder[0]} | {builder[1]}")
    if builder != MATRIX[BUILDER]:
        fail(f"builder is not {BUILDER}: {builder}")
    executables = build_binaries(("--offline",), stage)
    print(f"context staging: {time.monotonic() - STARTED:.0f} s (host Cargo home, offline; 1 Cargo process)")
    ledger.sample("after staging")
    cargo_log = os.path.join(parent, "cargo.log")
    runner = Runner(
        executables["oxide"], executables["oxide-storage-lease"], parent, extra={"RUSTUP_AUTO_INSTALL": "0"}
    )
    wrapper = os.path.join(runner.bin, "cargo")
    with open(wrapper, "w", encoding="utf-8") as handle:
        handle.write(CONTEXT_CARGO.replace("LOG", cargo_log).replace("REAL_CARGO", real_cargo))
    os.chmod(wrapper, 0o755)
    root = os.path.join(parent, "workspace")
    write_fixture(root, CONTEXT)
    root = os.path.realpath(root)
    member = os.path.join(root, "member")
    inputs = sum(len(text.encode()) for text in CONTEXT.values())
    print(f"context fixture inputs: {inputs} bytes")
    if inputs > SOURCE_LIMIT_BYTES:
        fail("fixture inputs exceed 8 MiB")
    code, _, stderr = runner.run(
        parent,
        ["--pool", runner.pool, "init", "--cap-bytes", str(IMAGE_BYTES), "--image-bytes", str(IMAGE_BYTES)],
        program=runner.lease,
    )
    expect(code, 0, "pool init", stderr)
    both = {"app", "member"}
    try:
        first = context_check(runner, ledger, root, [], "root")
        newer = agree(first, "root", both, "1.97.1", "1.97.1")
        if first["warmth"] != "cold":
            fail("root did not start cold")
        record = first["record"]
        identity = sha256_hex(f"path:{root}")
        if record["checkout"] != root or record["identity"] != identity:
            fail(f"root slot is not owned by the canonical workspace {root}: {record}")
        repeat = context_check(runner, ledger, root, [], "root repeat")
        if (repeat["slot"], repeat["warmth"]) != (first["slot"], "warm") or not all(repeat["fresh"].values()) or not repeat["fresh"]:
            fail(f"root repeat was not a fresh warm reuse of slot {first['slot']}: {repeat['fresh']}")
        inner = context_check(runner, ledger, member, [], "member")
        older = agree(inner, "member", {"member"}, "1.96.1", "1.96.1")
        if inner["slot"] == first["slot"] or inner["warmth"] != "cold":
            fail(f"member context reused the root's slot {first['slot']}")
        if (inner["record"]["checkout"], inner["record"]["identity"]) != (root, identity):
            fail(f"member slot is not owned by the canonical workspace {root}: {inner['record']}")
        if inner["record"]["compatibility"] == record["compatibility"]:
            fail("different compilers produced one compatibility key")
        again = context_check(runner, ledger, member, [], "member repeat")
        if (again["slot"], again["warmth"]) != (inner["slot"], "warm") or not again["fresh"] or not all(again["fresh"].values()):
            fail(f"member repeat was not a fresh warm reuse of slot {inner['slot']}: {again['fresh']}")
        # rustup selects from the working directory; --manifest-path moves
        # only the package Cargo builds.
        pointed = context_check(runner, ledger, root, ["--manifest-path", "member/Cargo.toml"], "root --manifest-path member")
        if (pointed["slot"], pointed["warmth"]) != (first["slot"], "warm") or not pointed["fresh"] or not all(pointed["fresh"].values()):
            fail(f"--manifest-path from the root left slot {first['slot']} or rebuilt: {pointed['fresh']}")
        if pointed["probes"]:
            agree(pointed, "root --manifest-path member", {"member"}, "1.97.1", "1.97.1")
        back = context_check(runner, ledger, member, ["--manifest-path", "../app/Cargo.toml"], "member --manifest-path app")
        agree(back, "member --manifest-path app", {"app"}, "1.96.1", "1.96.1")
        if (back["slot"], back["warmth"]) != (inner["slot"], "warm"):
            fail(f"--manifest-path from the member left slot {inner['slot']}")
        # Explicit environment: RUSTC wins over the directory's rustc, and
        # RUSTUP_TOOLCHAIN over the directory's toolchain file.
        cargo_path = inner["probes"]["member"][0]
        older_rustc = os.path.join(os.path.dirname(cargo_path), "rustc")
        forced = context_check(runner, ledger, root, [], "root RUSTC=1.96.1", {"RUSTC": older_rustc})
        if agree(forced, "root RUSTC=1.96.1", both, None, "1.96.1") != older:
            fail("RUSTC reported a different 1.96.1 rustc -vV")
        if (forced["slot"], forced["warmth"]) != (inner["slot"], "warm"):
            fail(f"RUSTC=1.96.1 from the root did not reuse slot {inner['slot']}")
        pinned = context_check(runner, ledger, root, [], "root RUSTUP_TOOLCHAIN=1.96.1", {"RUSTUP_TOOLCHAIN": "1.96.1"})
        if (pinned["slot"], pinned["warmth"]) != (inner["slot"], "warm"):
            fail(f"RUSTUP_TOOLCHAIN=1.96.1 from the root did not reuse slot {inner['slot']}")
        if pinned["probes"]:
            agree(pinned, "root RUSTUP_TOOLCHAIN=1.96.1", set(pinned["probes"]), None, "1.96.1")
        if newer == older:
            fail("the two contexts reported one rustc")
        # Refusal control: Oxide never takes a toolchain from its arguments.
        before = runner.state()["records"]
        code, stdout, stderr = runner.run(member, ["+1.97.1", "check"])
        ledger.emit("+1.97.1 refusal", "stdout", stdout)
        ledger.emit("+1.97.1 refusal", "stderr", stderr)
        expect(code, 1, "+toolchain refusal", stderr)
        if "toolchain overrides are refused" not in stderr or runner.state()["records"] != before:
            fail("+toolchain was not refused before any effect")
        records = runner.state()["records"]
        print(f"context slots: {[(r['id'], r['compatibility'][:16], r['checkout']) for r in records]}")
        if len(records) != 2:
            fail(f"expected exactly two slots, found {len(records)}")
    except Exception:
        # Close the pool, keep the evidence.
        detach = subprocess.run(
            ["/usr/bin/hdiutil", "detach", os.path.join(runner.pool, "payload")],
            capture_output=True,
            text=True,
            timeout=60,
        )
        print(f"context proof detach exit {detach.returncode}; retained its owned scratch at {parent}", file=sys.stderr)
        raise
    with open(cargo_log, encoding="utf-8") as handle:
        calls = handle.read().splitlines()
    ledger.emit("fixture Cargo", "log", "\n".join(calls) + "\n")
    total = len(calls) + 2
    print(f"context Cargo processes: {total} (setup `cargo -V` 1, staging 1, fixture {len(calls)})")
    if total > CONTEXT_CARGO_LIMIT:
        fail(f"{total} Cargo processes exceed {CONTEXT_CARGO_LIMIT}")
    payload = os.path.join(runner.pool, "payload")
    detach = subprocess.run(["/usr/bin/hdiutil", "detach", payload], capture_output=True, text=True, timeout=60)
    if detach.returncode:
        fail(f"detach failed after positive completion: {detach.stderr}")
    ledger.sample("after detach")
    shutil.rmtree(parent)
    elapsed = time.monotonic() - STARTED
    print(
        f"context proof held: {elapsed:.0f} s, owned peak {ledger.owned_peak // (1024 * 1024)} MiB, "
        f"host free minimum {ledger.free_min // (1024 * 1024)} MiB, output {ledger.log_bytes} bytes, scratch removed"
    )
    if elapsed > PROOF_SECONDS:
        fail(f"proof exceeded {PROOF_SECONDS} s including cleanup")
    return 0


CLEAN_SECRET = "oxide-secret-value"
WRAPPER = """#!/bin/sh
printf '%s\\n' "$*" >> "$OXIDE_PROOF_CARGO_LOG"
if [ "$1" = clean ]; then
    echo "proof cargo: clean refused" >&2
    exit 97
fi
exec "$OXIDE_PROOF_REAL_CARGO" "$@"
"""


def tree_state(root: str) -> dict:
    """Every path under root, root included, to (kind, size, mtime_ns);
    symlinks are recorded and never followed."""
    state = {}
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in [current, *(os.path.join(current, entry) for entry in dirs + files)]:
            metadata = os.lstat(name)
            kind = "link" if os.path.islink(name) else "dir" if os.path.isdir(name) else "file"
            state[name] = (kind, metadata.st_size, metadata.st_mtime_ns)
    return state


def seed_cargo_home(home: str) -> None:
    """Copy cfg-if 1.0.0 and its sparse-index entry from the host Cargo
    home into the private one; nothing is fetched."""
    host = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    copied = 0
    for index in sorted(os.listdir(os.path.join(host, "registry", "index"))):
        crate = os.path.join(host, "registry", "cache", index, "cfg-if-1.0.0.crate")
        entry = os.path.join(host, "registry", "index", index, ".cache", "cf", "g-", "cfg-if")
        config = os.path.join(host, "registry", "index", index, "config.json")
        if not (os.path.isfile(crate) and os.path.isfile(entry)):
            continue
        with open(crate, "rb") as handle:
            digest = hashlib.sha256(handle.read()).hexdigest()
        if digest != CFG_IF_CHECKSUM:
            fail(f"host cfg-if 1.0.0 archive sha256 {digest}")
        for source in (crate, entry, config):
            if not os.path.isfile(source):
                continue
            target = os.path.join(home, os.path.relpath(source, host))
            os.makedirs(os.path.dirname(target), exist_ok=True)
            shutil.copyfile(source, target)
            copied += os.path.getsize(source)
        print(f"clean-safety seed: cfg-if 1.0.0 (sha256 {digest}) and its index entry copied from {index}, {copied} bytes, no download")
        return
    fail("the host Cargo home has no cached cfg-if 1.0.0 with its index entry; the proof does not fetch")


def prove_whole_clean(call, parent: str, triple: str, real_env: dict) -> None:
    """Whole-target clean on a real Cargo target, then with foreign
    entries: dry run equals the plan, refusal changes nothing,
    --include-foreign removes only the enumerated tree."""
    app = os.path.join(parent, "clean-app")
    write_fixture(app, {
        "Cargo.toml": '[package]\nname = "clean-app"\nversion = "0.1.0"\nedition = "2021"\n',
        "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
        "src/main.rs": "fn main() {}\n",
    })
    built = subprocess.run(
        ["cargo", "build", "--offline", "--target", triple], cwd=app, env=real_env,
        capture_output=True, text=True, timeout=min(120, remaining()),
    )
    expect(built.returncode, 0, "fixture cargo build", built.stderr)
    target = os.path.join(app, "target")
    pristine = os.path.join(parent, "clean-pristine")
    shutil.copytree(target, pristine, symlinks=True)
    print(f"clean-safety Cargo target: {sorted(os.listdir(target))}")

    def dry(label: str, args: list) -> None:
        before = tree_state(target)
        code, stdout, stderr = call(app, ["clean", "--dry-run", *args])
        expect(code, 0, label, stderr)
        listed = set(stdout.splitlines())
        if listed != set(before):
            fail(f"{label}: listed {len(listed)} paths, the tree holds {len(before)}: {sorted(listed ^ set(before))[:10]}")
        if tree_state(target) != before or "nothing was removed" not in stderr:
            fail(f"{label}: the dry run changed the tree or did not say so: {stderr!r}")
        print(f"{label}: {len(listed)} paths listed, equal to the tree, nothing changed")

    dry("Cargo-only dry run", [])
    code, _, stderr = call(app, ["clean"])
    expect(code, 0, "Cargo-only clean", stderr)
    if os.path.lexists(target) or "oxide: removed " not in stderr or "foreign" in stderr:
        fail(f"Cargo-only clean did not remove the target cleanly: {stderr!r}")

    shutil.copytree(pristine, target, symlinks=True)
    outside = os.path.join(parent, "clean-outside")
    os.makedirs(outside)
    with open(os.path.join(outside, "sentinel"), "w") as handle:
        handle.write("keep")
    write_fixture(target, {"oxide-handover/bin/tool": "reviewed binary", "oxide-handover/bin/deep/receipt.json": "{}", "notes.txt": "mine"})
    os.symlink(outside, os.path.join(target, "oxide-handover", "escape"))
    for label, args in (("foreign dry run", ["--dry-run"]), ("foreign clean", [])):
        before = tree_state(target)
        code, stdout, stderr = call(app, ["clean", *args])
        expect(code, 1, label, stderr)
        if "clean refused" not in stderr or "oxide-handover" not in stderr or "notes.txt" not in stderr or "--include-foreign" not in stderr:
            fail(f"{label}: refusal did not name the foreign entries: {stderr!r}")
        if stdout or tree_state(target) != before:
            fail(f"{label}: refusal listed or changed something")
        print(f"{label}: refused, naming oxide-handover and notes.txt, nothing changed")
    dry("foreign dry run --include-foreign", ["--include-foreign"])
    code, _, stderr = call(app, ["clean", "--include-foreign"])
    expect(code, 0, "clean --include-foreign", stderr)
    if os.path.lexists(target) or "including foreign entries (--include-foreign): notes.txt, oxide-handover" not in stderr:
        fail(f"clean --include-foreign did not remove and name the foreign entries: {stderr!r}")
    with open(os.path.join(outside, "sentinel")) as handle:
        if handle.read() != "keep":
            fail("the symlink referent changed")
    print("clean --include-foreign: target removed, foreign entries named, symlink referent intact")

    for args, needle in ((["-p", "clean-app", "--dry-run"], "lists paths only for a whole-target clean"),
                         (["--doc", "--include-foreign"], "--include-foreign applies only to a whole-target clean"),
                         (["--target", triple], "not supported for a whole-target clean")):
        code, _, stderr = call(app, ["clean", *args])
        expect(code, 1, f"clean {args}", stderr)
        if needle not in stderr:
            fail(f"clean {args} refusal did not say {needle!r}: {stderr!r}")


def prove_config_gate(call, parent: str, home: str) -> None:
    """`clean --cargo-cache` reads Cargo config: risky keys in each config
    location refuse by name, unparseable files refuse, a comments-only
    project config with an unrelated home config proceeds."""
    offline = {"CARGO_NET_OFFLINE": "true"}
    cfg_if = lock_package("cfg-if", "1.0.0", CRATES_IO_SOURCE, CFG_IF_CHECKSUM)
    root = os.path.join(parent, "config")
    good = cache_fixture(root, "config-good", 'cfg-if = "=1.0.0"\n', cfg_if + lock_package("config-good", "0.1.0", deps=["cfg-if"]))
    missing = cache_fixture(root, "config-missing", 'cfg-if = "=1.0.0"\n', None)
    ssh = cache_fixture(
        root, "config-ssh", 'remote = { git = "ssh://git@127.0.0.1:9/remote.git" }\n',
        lock_package("config-ssh", "0.1.0", deps=["remote"])
        + lock_package("remote", "0.1.0", "git+ssh://git@127.0.0.1:9/remote.git#" + "1" * 40),
    )
    stale = cache_fixture(
        root, "config-stale", 'cfg-if = "=1.0.0"\nremote = { git = "ssh://git@127.0.0.1:9/remote.git" }\n',
        cfg_if + lock_package("config-stale", "0.1.0", deps=["cfg-if"]),
    )
    for path in (good, missing, ssh, stale):
        code, _, stderr = call(path, ["init"], offline)
        expect(code, 0, f"adopt {os.path.basename(path)}", stderr)
    project = os.path.join(good, ".cargo", "config.toml")
    os.makedirs(os.path.dirname(project))
    home_config = os.path.join(home, "config.toml")
    ancestor = os.path.join(root, ".cargo", "config")

    def refused(cwd: str, needle: str, label: str, extra: dict | None = None) -> None:
        code, _, stderr = call(cwd, ["clean", "--cargo-cache"], extra)
        expect(code, 1, label, stderr)
        if "oxide: clean --cargo-cache refused: " not in stderr or needle not in stderr or "asking Cargo to clean" in stderr:
            fail(f"{label}: refusal did not name {needle!r}: {stderr!r}")
        if CLEAN_SECRET in stderr:
            fail(f"{label}: a config value reached the diagnostic")
        print(f"{label}: refused ({needle})")

    def written(path: str, text: str) -> None:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)

    risky = (
        (project, f'[source.crates-io]\nreplace-with = "{CLEAN_SECRET}"\n', f'{project} sets key "source": source replacement'),
        (project, f'[registries.mine]\nindex = "sparse+https://{CLEAN_SECRET}.invalid/"\n', 'key "registries": a custom registry'),
        (project, f'[registry]\ncredential-provider = "{CLEAN_SECRET}"\n', 'key "registry"."credential-provider": credential providers'),
        (project, f'registry.global-credential-providers = ["{CLEAN_SECRET}"]\n', 'key "registry"."global-credential-providers": credential providers'),
        (project, "\"net\".'git-fetch-with-cli' = true\n", 'key "net"."git-fetch-with-cli": net.git-fetch-with-cli'),
        (project, f'include = "{CLEAN_SECRET}.toml"\n', 'key "include": it includes another config file'),
        (home_config, "[net]\ngit-fetch-with-cli = true\n", f'{home_config} sets key "net"."git-fetch-with-cli"'),
        (ancestor, f'[build]\nrustc-wrapper = "{CLEAN_SECRET}"\n', f'{ancestor} sets key "build"."rustc-wrapper": it names a program'),
        (project, f'[net\ntoken = "{CLEAN_SECRET}"\n', f"{project} cannot be parsed as TOML 1.0"),
        (project, "net = {\n  retry = 2\n}\n", f"{project} cannot be parsed as TOML 1.0"),
    )
    for path, text, needle in risky:
        written(path, text)
        refused(good, needle, f"config {os.path.basename(os.path.dirname(path))}/{os.path.basename(path)} {text.splitlines()[0][:40]!r}")
        os.remove(path)

    # Existing refusals, with no config present.
    refused(missing, "Cargo.lock is missing", "missing lockfile")
    refused(ssh, "SSH and other non-HTTP git transports cannot be contained", "SSH git source")
    refused(stale, "Cargo cannot resolve this workspace offline from Cargo.lock", "stale lock with an SSH dependency")
    refused(good, "CARGO_NET_GIT_FETCH_WITH_CLI is set: net.git-fetch-with-cli", "git-fetch-with-cli", {"CARGO_NET_GIT_FETCH_WITH_CLI": "true"})

    written(project, '# [source.crates-io]\n# replace-with = "mirror"\n')
    written(home_config, '[build]\njobs = 2\n\n[term]\ncolor = "never"\n')
    code, _, stderr = call(good, ["clean", "--cargo-cache"])
    if code not in (0, 75) or "asking Cargo to clean" not in stderr:
        fail(f"comments-only config did not proceed to Cargo: exit {code} {stderr!r}")
    if not re.search(r"oxide: closed proxy 127\.0\.0\.1:\d+ closed 0 connection\(s\)", stderr):
        fail(f"the proceeding cleanup reached for the network: {stderr!r}")
    outcome = next((line for line in stderr.splitlines() if line.startswith(("oxide: Cargo cleanup ran", "oxide: Cargo skipped cleanup"))), None)
    if outcome is None:
        fail(f"no labelled outcome: {stderr!r}")
    print(f"comments-only project config with an unrelated home config: proceeded, exit {code}, 0 proxy connections, {outcome}")


def clean_safety_main() -> int:
    """`oxide clean` target ownership and `clean --cargo-cache` config
    parsing on scratch fixtures: no pool, image or download."""
    global STARTED
    if sys.platform != "darwin":
        fail("the clean-safety proof requires macOS")
    STARTED = time.monotonic()
    reserve(os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or "/private/tmp", "admission", "clean-safety")
    root = scratch_root()
    parent = os.path.join(root, "oxide-clean-safety")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    stage = dict(os.environ)
    for name in ("CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTC_WRAPPER"):
        stage.pop(name, None)
    stage.update({
        "RUSTUP_TOOLCHAIN": BUILDER,
        "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_NET_OFFLINE": "true",
    })
    setup = time.monotonic()
    builder = toolchain_identity(stage, repo)
    setup_seconds = time.monotonic() - setup
    print(f"clean-safety setup: 2 version calls, {setup_seconds:.1f} s, 0 MiB written")
    if builder != MATRIX[BUILDER] or setup_seconds > 30:
        fail(f"setup is not {BUILDER} within 30 s: {builder}, {setup_seconds:.1f} s")
    executables = build_binaries(("--offline",), stage)
    staged = tree_bytes(parent, False)
    print(f"clean-safety staging: {time.monotonic() - STARTED:.0f} s, {staged // (1024 * 1024)} MiB (host Cargo home, offline)")
    if staged > SCRATCH_LIMIT_BYTES:
        fail(f"staging {staged} exceeds 512 MiB")
    reserve(parent, "after staging", "clean-safety")

    real_cargo = shutil.which("cargo")
    if not real_cargo:
        fail("no cargo on PATH")
    bindir = os.path.join(parent, "bin")
    os.makedirs(bindir)
    write_fixture(bindir, {"cargo": WRAPPER})
    os.chmod(os.path.join(bindir, "cargo"), 0o755)
    cargo_log = os.path.join(parent, "cargo-calls.log")
    no_pool = os.path.join(parent, "never-created-pool")
    home = os.path.join(parent, "cargo-home")
    os.makedirs(home, mode=0o700)
    base = {name: value for name, value in os.environ.items()
            if not (name.startswith(("CARGO_NET_", "CARGO_REGISTR", "CARGO_SOURCE_", "CARGO_UNSTABLE_", "CARGO_HTTP_", "CARGO_TARGET_DIR", "CARGO_BUILD_"))
                    or name.lower().endswith("_proxy") or name in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "CARGO_HOME"))}
    # An empty private home for the fixture build and locate calls; the
    # host Cargo home serves only offline staging.
    build_home = os.path.join(parent, "build-home")
    os.makedirs(build_home, mode=0o700)
    base["CARGO_HOME"] = os.path.realpath(build_home)
    real_env = {**base, "CARGO_NET_OFFLINE": "true"}
    env = {**base, "PATH": bindir + os.pathsep + base.get("PATH", ""), "OXIDE_POOL": no_pool,
           "OXIDE_PROOF_CARGO_LOG": cargo_log, "OXIDE_PROOF_REAL_CARGO": real_cargo}

    def call(cwd: str, args: list, extra: dict | None = None, cargo_home: str | None = None):
        result = subprocess.run(
            [executables["oxide"], *args], cwd=cwd,
            env={**env, **({"CARGO_HOME": cargo_home} if cargo_home else {}), **(extra or {})},
            capture_output=True, text=True, timeout=min(60, remaining()),
        )
        return result.returncode, result.stdout, result.stderr

    triple = {"arm64": "aarch64-apple-darwin", "x86_64": "x86_64-apple-darwin"}[os.uname().machine]
    try:
        prove_whole_clean(call, parent, triple, real_env)
        reserve(parent, "after whole-target clean", "clean-safety")
        seed_cargo_home(home)
        prove_config_gate(lambda cwd, args, extra=None: call(cwd, args, extra, os.path.realpath(home)), parent, os.path.realpath(home))
        reserve(parent, "after config gate", "clean-safety")
        with open(cargo_log) as handle:
            calls = handle.read().splitlines()
        commands = {}
        for line in calls:
            word = line.split(" ")[0] if line else ""
            commands[word] = commands.get(word, 0) + 1
        print(f"clean-safety Cargo calls through the recording wrapper: {len(calls)} {dict(sorted(commands.items()))}")
        if commands.get("clean") or commands.get("fetch") != 1:
            fail("Oxide called cargo clean, or cargo fetch other than once for the proceeding cleanup")
        if os.path.lexists(no_pool):
            fail("the clean-safety proof created a pool")
        owned = tree_bytes(parent, False)
        print(f"clean-safety owned scratch: {owned // (1024 * 1024)} MiB")
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB")
    except Exception:
        print(f"clean-safety proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    shutil.rmtree(parent)
    if not os.environ.get("OXIDE_TASK_SCRATCH_ROOT"):
        os.rmdir(root)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail(f"clean-safety proof took {elapsed:.0f} s, over {PROOF_SECONDS} s")
    print(f"oxide clean-safety proof held: {elapsed:.0f} s including cleanup, owned scratch {owned // (1024 * 1024)} MiB, scratch removed")
    return 0


def launch_main() -> int:
    """The launch check alone, offline: no pool, image, download or Cargo
    home other than the host's cached registry for staging."""
    global STARTED
    if sys.platform != "darwin":
        fail("the launch-check proof requires macOS")
    STARTED = time.monotonic()
    # Sampled before this proof writes anything, on the scratch filesystem.
    reserve(os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or "/private/tmp", "admission", "launch")
    root = scratch_root()
    parent = os.path.join(root, "oxide-launch")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    stage = dict(os.environ)
    for name in ("CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTC_WRAPPER"):
        stage.pop(name, None)
    # Offline staging with the builder release into this proof's scratch,
    # so its footprint is counted.
    stage.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_NET_OFFLINE": "true",
        }
    )
    setup = time.monotonic()
    builder = toolchain_identity(stage, repo)
    setup_seconds = time.monotonic() - setup
    print(f"launch setup: 2 version calls, {setup_seconds:.1f} s, 0 MiB written")
    if builder != MATRIX[BUILDER] or setup_seconds > 30:
        fail(f"setup is not {BUILDER} within 30 s: {builder}, {setup_seconds:.1f} s")
    executables = build_binaries(("--offline",), stage)
    staged = tree_bytes(parent, False)
    print(f"launch staging: {time.monotonic() - STARTED:.0f} s, {staged // (1024 * 1024)} MiB (host Cargo home, offline)")
    if staged > SCRATCH_LIMIT_BYTES:
        fail(f"staging {staged} exceeds 512 MiB")
    reserve(parent, "after staging", "launch")
    runner = Runner(executables["oxide"], executables["oxide-storage-lease"], parent)
    app = os.path.join(parent, "app")
    os.makedirs(app, mode=0o700)
    try:
        prove_launch_check(runner, app)
        reserve(parent, "after launch check", "launch")
        owned = tree_bytes(parent, False)
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB")
    except Exception:
        print(f"launch proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    shutil.rmtree(parent)
    if not os.environ.get("OXIDE_TASK_SCRATCH_ROOT"):
        os.rmdir(root)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail(f"launch proof took {elapsed:.0f} s, over {PROOF_SECONDS} s")
    print(f"oxide launch-check proof held: {elapsed:.0f} s including cleanup, owned scratch {owned // (1024 * 1024)} MiB")
    return 0


def main() -> int:
    global STARTED
    if "--clean-safety" in sys.argv:
        if sys.argv[1:] != ["--clean-safety"]:
            fail("usage: test-oxide-cli.py --clean-safety")
        return clean_safety_main()
    if "--launch-check" in sys.argv:
        if len(sys.argv) != 2:
            fail("usage: test-oxide-cli.py --launch-check")
        return launch_main()
    if "--cargo-matrix" in sys.argv:
        if len(sys.argv) != 3 or sys.argv[1] != "--cargo-matrix":
            fail("usage: test-oxide-cli.py --cargo-matrix <exact version>")
        return matrix_main(sys.argv[2])
    if "--toolchain-context" in sys.argv:
        if sys.argv[1:] != ["--toolchain-context"]:
            fail("usage: test-oxide-cli.py --toolchain-context")
        return context_main()
    parent = os.path.join(scratch_root(), "oxide-cli")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    prove_wait_lines()
    # The proof phase includes staging the product binaries, which uses the
    # host Cargo home and this repository's cached dependencies.
    STARTED = time.monotonic()
    executables = build_binaries()
    print(f"oxide product staging: {time.monotonic() - STARTED:.0f} s (host Cargo home)")
    runner = Runner(executables["oxide"], executables["oxide-storage-lease"], parent)
    app = os.path.join(parent, "checkouts", "app")
    write_fixture(app)
    if tree_bytes(app, False) > SOURCE_LIMIT_BYTES:
        fail("fixture source exceeds 8 MiB")
    prove_usage(runner, app)
    prove_launch_check(runner, app)
    prove_adoption(runner, os.path.join(parent, "adoption"))
    if sys.platform != "darwin":
        fail("the bounded APFS oxide proof requires macOS")
    free = host_free_bytes(parent)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve")
    commit_lock(runner, app)
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
    # Budget-only directory pools on the same scratch filesystem, after the
    # APFS image is detached: never two owned images attached.
    budget = Runner(runner.oxide, runner.lease, parent, "budget-pool", {"OXIDE_POOL_BACKEND": "directory-budget"})
    tiny = Runner(
        runner.oxide,
        runner.lease,
        parent,
        "budget-tiny",
        {"OXIDE_POOL_BACKEND": "directory-budget", "OXIDE_TEST_BUDGET_BYTES": str(1024 * 1024)},
    )
    capped = Runner(runner.oxide, runner.lease, parent, "cap-pool", {"OXIDE_POOL_BACKEND": "directory-budget"})
    try:
        prove_budget(budget, tiny, app)
        prove_slot_files(budget)
        prove_pool_cap(capped, app)
        prove_cargo_cache(runner)
    except Exception:
        print(f"oxide budget proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    owned = max(owned, tree_bytes(parent, True))
    if owned > SCRATCH_LIMIT_BYTES:
        fail(f"owned fixture scratch {owned} exceeds 512 MiB")
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
    except Exception:
        traceback.print_exc()
        sys.exit(1)
