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
copies one named cached archive (cfg-if 1.0.0) offline; nothing may use the
network. The scratch is removed only after positive completion. Before any pool
exists, `init` / `new` adoption runs on tiny scratch directories (90 s,
64 MiB scratch) and must leave the pool uncreated. These cases run with
OXIDE_NO_SKIP=1, so every `run` takes the leased path. Not a benchmark and not
full QA.

`--run-skip` is the separate focused proof of the `oxide run` no-op fast path
(effigy rust:oxide-run-skip): its own fresh 128 MiB APFS pool and 300 s clock
from admission, zero-child skips under RLIMIT_NPROC 0, every key class
falling through, registry eligibility and refusal from named cached inputs in
a private CARGO_HOME, and #079's fixed two-part timing gate with raw samples.
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


def build_binaries(staging: str | None = None) -> dict:
    """Stage the product binaries. With `staging`, the build is offline and
    its target directory and temporary files live under that owned path."""
    command = [
        "cargo",
        "build",
        "--locked",
        "--bin",
        "oxide",
        "--bin",
        "oxide-storage-lease",
        "--message-format=json-render-diagnostics",
    ]
    env = None
    if staging:
        command.append("--offline")
        tmp = os.path.join(staging, "tmp")
        os.makedirs(tmp, mode=0o700)
        env = dict(os.environ)
        env.pop("CARGO_BUILD_TARGET_DIR", None)
        env.pop("CARGO_BUILD_BUILD_DIR", None)
        env.update(
            {
                "CARGO_TARGET_DIR": os.path.join(staging, "target"),
                "CARGO_INCREMENTAL": "0",
                "TMPDIR": tmp,
                "TMP": tmp,
                "TEMP": tmp,
            }
        )
    build = subprocess.run(
        command,
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
    def __init__(self, oxide: str, lease: str, parent: str, pool: str = "pool", extra: dict | None = None, skip: bool = False):
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
        env.pop("OXIDE_NO_SKIP", None)
        # The leased-path cases keep every `run` on Cargo; only the run-skip
        # proof exercises the fast path.
        if not skip:
            env["OXIDE_NO_SKIP"] = "1"
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


# The named cached inputs the Cargo-home case copies; nothing is downloaded.
CFG_IF_INPUTS = (
    "registry/index/index.crates.io-1949cf8c6b5b557f/config.json",
    "registry/index/index.crates.io-1949cf8c6b5b557f/.cache/cf/g-/cfg-if",
    "registry/cache/index.crates.io-1949cf8c6b5b557f/cfg-if-1.0.0.crate",
)
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
    CARGO_HOME. Setup copies one named cached archive (cfg-if 1.0.0) offline;
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
    refused(custom, f"Cargo config file {custom}/.cargo/config.toml exists", "custom source")

    # Setup, offline: copy exactly the named cached cfg-if 1.0.0 archive and
    # index inputs into the private home, then let Cargo register them
    # offline. Nothing is downloaded; itoa stays absent for the denial case.
    write_fixture(good, {"Cargo.lock": "version = 4\n\n" + cfg_if + lock_package("cache-good", "0.1.0", deps=["cfg-if"])})
    host_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    for relative in CFG_IF_INPUTS:
        source = os.path.join(host_home, relative)
        if not os.path.isfile(source):
            fail(f"named cache input missing, no download: {relative}")
        target = os.path.join(home, relative)
        os.makedirs(os.path.dirname(target), mode=0o700, exist_ok=True)
        shutil.copyfile(source, target)
    fetched = subprocess.run(
        ["cargo", "fetch", "--locked", "--offline"], cwd=good, env=env, capture_output=True, text=True, timeout=min(90, remaining())
    )
    expect(fetched.returncode, 0, "offline setup fetch (cfg-if 1.0.0)", fetched.stderr)
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
    print(f"oxide cargo-cache setup: 1 archive, cfg-if 1.0.0 copied from the host cache offline, {os.path.getsize(archives[0])} bytes, sha256 {digest}")

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


# `oxide run` no-op fast path (oxide#089): `--run-skip` mode.

SKIP_FIXTURE = {
    "repo/Cargo.toml": """[package]
name = "skipapp"
version = "0.1.0"
edition = "2021"

[dependencies]
helper = { path = "helper" }
outside = { path = "../outside" }

[workspace]
members = ["tool"]
""",
    "repo/rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "repo/.gitignore": "/target\n/helper/Cargo.toml\n",
    "repo/build.rs": 'fn main() {\n    println!("cargo:rerun-if-changed=build.rs");\n}\n',
    "repo/src/msg.rs": 'pub const VERSION: &str = "v1";\n',
    "repo/src/main.rs": """mod msg;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let note = option_env!("SKIP_NOTE").unwrap_or("-");
    println!("skip {} {} {} {note} {}", msg::VERSION, helper::flavor(), outside::value(), args.join(" "));
}
""",
    # An in-checkout dependency whose `[lib] path` leaves its directory and
    # whose manifest Git ignores: neither source ancestry nor Git covers it.
    "repo/helper/Cargo.toml": """[package]
name = "helper"
version = "0.1.0"
edition = "2021"

[features]
default = ["old"]
old = []
new = []

[lib]
path = "../shared/lib.rs"
""",
    "repo/shared/lib.rs": """pub fn flavor() -> &'static str {
    if cfg!(feature = "new") {
        "new"
    } else {
        "old"
    }
}
""",
    "repo/tool/Cargo.toml": '[package]\nname = "tool"\nversion = "0.1.0"\nedition = "2021"\n',
    "repo/tool/src/main.rs": 'fn main() {\n    println!("tool {}", std::env::args().skip(1).collect::<Vec<_>>().join(" "));\n}\n',
    # A path dependency outside the checkout: stat-recorded, not refused.
    "outside/Cargo.toml": '[package]\nname = "outside"\nversion = "0.1.0"\nedition = "2021"\n',
    "outside/src/lib.rs": "pub fn value() -> u32 {\n    5\n}\n",
}
SKIP_REGISTRY = {
    "Cargo.toml": '[package]\nname = "regapp"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ncfg-if = "=1.0.0"\n\n[workspace]\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "src/main.rs": """cfg_if::cfg_if! {
    if #[cfg(unix)] {
        const OS: &str = "unix";
    } else {
        const OS: &str = "other";
    }
}

fn main() {
    println!("registry {OS} {}", std::env::args().nth(1).unwrap_or_default());
}
""",
}
SKIP_SCRIPTED = {
    "Cargo.toml": '[package]\nname = "scriptedapp"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nlibc = "=0.2.190"\n\n[workspace]\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "src/main.rs": 'fn main() {\n    println!("scripted {}", unsafe { libc::getpid() } > 0);\n}\n',
}
# Automatic discovery: no build script and one binary at first.
SKIP_DISCOVERY = {
    "Cargo.toml": '[package]\nname = "discoverapp"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "src/main.rs": 'fn main() {\n    println!("discover {}", option_env!("DISCOVERY_MARKER").unwrap_or("old"));\n}\n',
}
SKIP_DISCOVERY_SCRIPT = 'fn main() {\n    println!("cargo:rerun-if-changed=build.rs");\n    println!("cargo:rustc-env=DISCOVERY_MARKER=new");\n}\n'
SKIP_INDEX = "index.crates.io-1949cf8c6b5b557f"
# Exactly these existing cached inputs seed the private CARGO_HOME.
SKIP_REGISTRY_INPUTS = (
    f"registry/index/{SKIP_INDEX}/config.json",
    f"registry/index/{SKIP_INDEX}/.cache/cf/g-/cfg-if",
    f"registry/cache/{SKIP_INDEX}/cfg-if-1.0.0.crate",
    f"registry/index/{SKIP_INDEX}/.cache/li/bc/libc",
    f"registry/cache/{SKIP_INDEX}/libc-0.2.190.crate",
)
SKIP_LINE = "unchanged — skipped Cargo"
# Oxide's quick profile, so the bare Cargo baseline builds what `oxide run` builds.
QUICK_PROFILE_ENVIRONMENT = {
    "CARGO_PROFILE_OXIDE_QUICK_INHERITS": "dev",
    "CARGO_PROFILE_OXIDE_QUICK_DEBUG": "line-tables-only",
}


def no_children() -> None:
    """preexec: no process this user owns may fork. A skip that succeeds
    under it started zero children; exec is unaffected."""
    import resource

    resource.setrlimit(resource.RLIMIT_NPROC, (0, 0))


def same_spawn() -> None:
    """preexec with the same spawn path as `no_children`, limiting nothing."""
    import resource

    resource.getrlimit(resource.RLIMIT_NPROC)


def timed(cmd: list, cwd: str, env: dict, preexec) -> tuple:
    started = time.perf_counter_ns()
    result = subprocess.run(
        cmd, cwd=cwd, env=env, capture_output=True, text=True,
        timeout=min(120, remaining()), preexec_fn=preexec,
    )
    return time.perf_counter_ns() - started, result.returncode, result.stdout, result.stderr


class SkipCase:
    def __init__(self, runner: Runner, repo: str):
        self.runner = runner
        self.repo = repo

    def run(self, label: str, args: list, cwd: str | None, extra: dict | None, preexec):
        ns, code, stdout, stderr = timed(
            [self.runner.oxide, "run", *args], cwd or self.repo, self.runner.environment(extra), preexec
        )
        return ns, code, stdout.strip(), stderr

    def skips(self, label: str, args: list, wanted: str, cwd: str | None = None, extra: dict | None = None) -> int:
        """Zero children: the run happens under RLIMIT_NPROC 0."""
        ns, code, stdout, stderr = self.run(label, args, cwd, extra, no_children)
        expect(code, 0, f"{label} (zero-child skip)", stderr)
        if SKIP_LINE not in stderr or stdout != wanted or " slot " in stderr:
            fail(f"{label}: expected a skip printing {wanted!r}, got {stdout!r} {stderr[-800:]!r}")
        return ns

    def builds(self, label: str, args: list, wanted, cwd: str | None = None, extra: dict | None = None) -> str:
        """Falls through: the leased summary names a slot. `wanted` is the
        output, or a tuple of acceptable outputs."""
        _, code, stdout, stderr = self.run(label, args, cwd, extra, same_spawn)
        expect(code, 0, label, stderr)
        if SKIP_LINE in stderr or stdout not in (wanted if isinstance(wanted, tuple) else (wanted,)):
            fail(f"{label}: expected Cargo printing {wanted!r}, got {stdout!r} {stderr[-800:]!r}")
        summary(stderr, label)
        return stderr


def skip_records(repo: str) -> list:
    directory = os.path.join(repo, "target", "oxide-run-skip")
    if not os.path.isdir(directory):
        return []
    return sorted(name for name in os.listdir(directory) if name.endswith(".json"))


def skip_record(cwd: str, args: list) -> str:
    digest = hashlib.sha256(os.path.realpath(cwd).encode())
    for argument in args:
        digest.update(b"\0" + argument.encode())
    return digest.hexdigest() + ".json"


def write_in_place(path: str, text: str) -> None:
    with open(path, "r+", encoding="utf-8") as handle:
        handle.seek(0)
        handle.write(text)
        handle.truncate()


def seed_registry(runner: Runner) -> None:
    host_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    for relative in SKIP_REGISTRY_INPUTS:
        source = os.path.join(host_home, relative)
        if not os.path.isfile(source):
            fail(f"named cache input missing, no substitute: {relative}")
        target = os.path.join(runner.cargo_home, relative)
        os.makedirs(os.path.dirname(target), mode=0o700, exist_ok=True)
        shutil.copyfile(source, target)


def prove_skip_cases(runner: Runner) -> dict:
    nest = os.path.join(runner.parent, "checkouts", "skip")
    repo = os.path.join(nest, "repo")
    write_fixture(nest, SKIP_FIXTURE)
    commit_lock(runner, repo)
    git(repo, "init", "--quiet")
    git(repo, "add", "-A")
    git(repo, "commit", "--quiet", "-m", "skip fixture")
    case = SkipCase(runner, repo)
    msg = os.path.join(repo, "src", "msg.rs")
    evidence = {}

    case.builds("first run", ["--", "a"], "skip v1 old 5 - a")
    if skip_records(repo) != [skip_record(repo, [])]:
        fail(f"a successful run published {skip_records(repo)}")
    with open(os.path.join(repo, "target", "oxide-run-skip", skip_record(repo, [])), encoding="utf-8") as handle:
        record = json.load(handle)
    paths = {os.path.realpath(entry["path"]) for entry in record["inputs"]}
    wanted_paths = {
        os.path.realpath(os.path.join(nest, "outside", "src", "lib.rs")),
        os.path.realpath(os.path.join(nest, "outside", "Cargo.toml")),
        os.path.realpath(os.path.join(repo, "helper", "Cargo.toml")),
        os.path.realpath(os.path.join(repo, "shared", "lib.rs")),
        os.path.realpath(os.path.join(repo, "Cargo.lock")),
        os.path.realpath(os.path.join(nest, ".cargo", "config.toml")),
        os.path.realpath(os.path.join(repo, "rust-toolchain.toml")),
    }
    missing = sorted(path for path in wanted_paths if path not in paths)
    if missing:
        fail(f"skip record lacks inputs {missing}")
    absent = [os.path.realpath(entry["path"]) for entry in record["inputs"] if entry["stat"] is None]
    if os.path.realpath(os.path.join(nest, ".cargo", "config.toml")) not in absent:
        fail("the absent ancestor config was not recorded as absent")
    evidence["record_inputs"] = len(record["inputs"])
    evidence["record_absent"] = len(absent)

    case.skips("repeat skip", ["--", "b"], "skip v1 old 5 - b")
    case.skips("repeat skip, program args after --", ["--", "--rebuild", "-p", "x"], "skip v1 old 5 - --rebuild -p x")
    # Control: the same limit refuses the leased path's Cargo spawn.
    _, code, _, stderr = case.run("control", [], None, {"OXIDE_NO_SKIP": "1"}, no_children)
    if code == 0:
        fail(f"the zero-child limit did not refuse a leased build: {stderr[-400:]!r}")
    case.builds("--rebuild", ["--rebuild"], "skip v1 old 5 -")
    case.skips("skip after --rebuild", [], "skip v1 old 5 -")
    # OXIDE_NO_SKIP is part of the full environment it records under.
    case.builds("OXIDE_NO_SKIP", [], "skip v1 old 5 -", extra={"OXIDE_NO_SKIP": "1"})
    case.builds("environment without OXIDE_NO_SKIP", [], "skip v1 old 5 -")
    case.skips("skip after forced builds", [], "skip v1 old 5 -")

    # Source edit.
    write_in_place(msg, 'pub const VERSION: &str = "v2";\n')
    case.builds("source edit", [], "skip v2 old 5 -")
    case.skips("source edit repeat", [], "skip v2 old 5 -")
    # Same size, same inode, mtime restored: only ctime moves.
    before = os.stat(msg)
    time.sleep(0.05)
    write_in_place(msg, 'pub const VERSION: &str = "v3";\n')
    os.utime(msg, ns=(before.st_atime_ns, before.st_mtime_ns))
    after = os.stat(msg)
    if (after.st_mtime_ns, after.st_size, after.st_ino) != (before.st_mtime_ns, before.st_size, before.st_ino):
        fail("the restored-mtime edit changed more than ctime")
    # The skip falls through; Cargo's own mtime comparison may still call
    # the unit fresh, so either output is Cargo's answer.
    case.builds("restored-old-mtime edit (ctime)", [], ("skip v2 old 5 -", "skip v3 old 5 -"))
    os.utime(msg)
    case.builds("source touched", [], "skip v3 old 5 -")
    case.skips("ctime repeat", [], "skip v3 old 5 -")

    # Manifests: the custom-lib-path, Git-ignored helper manifest.
    helper = os.path.join(repo, "helper", "Cargo.toml")
    write_in_place(helper, SKIP_FIXTURE["repo/helper/Cargo.toml"].replace('default = ["old"]', 'default = ["new"]'))
    case.builds("ignored custom-lib manifest", [], "skip v3 new 5 -")
    case.skips("manifest repeat", [], "skip v3 new 5 -")
    with open(os.path.join(repo, "Cargo.toml"), "a", encoding="utf-8") as handle:
        handle.write("\n[profile.dev]\n")
    case.builds("root manifest", [], "skip v3 new 5 -")
    case.skips("root manifest repeat", [], "skip v3 new 5 -")

    # Lock file.
    lock = os.path.join(repo, "Cargo.lock")
    with open(lock, encoding="utf-8") as handle:
        lock_text = handle.read()
    with open(lock, "a", encoding="utf-8") as handle:
        handle.write("\n")
    case.builds("lock change", [], "skip v3 new 5 -")
    with open(lock, "w", encoding="utf-8") as handle:
        handle.write(lock_text)
    case.builds("lock restored", [], "skip v3 new 5 -")
    case.skips("lock repeat", [], "skip v3 new 5 -")

    # Config added above the checkout, where Cargo reads it.
    os.makedirs(os.path.join(nest, ".cargo"))
    with open(os.path.join(nest, ".cargo", "config.toml"), "w", encoding="utf-8") as handle:
        handle.write('[env]\nSKIP_NOTE = "cfg"\n')
    case.builds("config added", [], "skip v3 new 5 cfg")
    case.skips("config repeat", [], "skip v3 new 5 cfg")

    # Toolchain file: same installed toolchain, new file identity.
    with open(os.path.join(repo, "rust-toolchain.toml"), "a", encoding="utf-8") as handle:
        handle.write("# touched\n")
    case.builds("toolchain file", [], "skip v3 new 5 cfg")
    case.skips("toolchain repeat", [], "skip v3 new 5 cfg")

    # Environment: the value is hashed, never stored.
    secret = {"SKIP_FIXTURE_VAR": "secret-skip-value"}
    case.builds("environment variable", [], "skip v3 new 5 cfg", extra=secret)
    case.skips("environment repeat", [], "skip v3 new 5 cfg", extra=secret)
    for name in skip_records(repo):
        if "secret-skip-value" in read_text(os.path.join(repo, "target", "oxide-run-skip", name)):
            fail("a raw environment value reached a skip record")
    case.builds("environment variable removed", [], "skip v3 new 5 cfg")

    # Deleted binary.
    os.remove(os.path.join(repo, "target", "oxide-quick", "skipapp"))
    case.builds("deleted binary", [], "skip v3 new 5 cfg")
    case.skips("deleted binary repeat", [], "skip v3 new 5 cfg")

    # A path dependency outside the checkout is recorded and caught.
    write_in_place(os.path.join(nest, "outside", "src", "lib.rs"), "pub fn value() -> u32 {\n    6\n}\n")
    case.builds("outside path dependency", [], "skip v3 new 6 cfg")
    case.skips("outside repeat", [], "skip v3 new 6 cfg")

    # A member working directory never runs another member's binary.
    case.builds("member", [], "tool", cwd=os.path.join(repo, "tool"))
    case.skips("member repeat", ["--", "x"], "tool x", cwd=os.path.join(repo, "tool"))
    case.skips("root after member", [], "skip v3 new 6 cfg")

    # A failed build leaves no record.
    write_in_place(msg, "pub const VERSION: &str = ;\n")
    _, code, _, stderr = case.run("failed build", [], None, None, same_spawn)
    expect(code, 101, "failed build", stderr)
    if skip_record(repo, []) in skip_records(repo):
        fail("a failed build left a skip record")
    write_in_place(msg, 'pub const VERSION: &str = "v4";\n')
    case.builds("after failure", [], "skip v4 new 6 cfg")
    case.skips("after failure repeat", [], "skip v4 new 6 cfg")

    # Cargo's automatic discovery: an added build.rs, then an added binary,
    # change no existing file but must still fall through.
    discovery = os.path.join(runner.parent, "checkouts", "discovery")
    write_fixture(discovery, SKIP_DISCOVERY)
    commit_lock(runner, discovery)
    case.builds("discovery first", [], "discover old", cwd=discovery)
    case.skips("discovery repeat", [], "discover old", cwd=discovery)
    with open(os.path.join(discovery, "build.rs"), "w", encoding="utf-8") as handle:
        handle.write(SKIP_DISCOVERY_SCRIPT)
    case.builds("discovered build script", [], "discover new", cwd=discovery)
    case.skips("discovered build script repeat", [], "discover new", cwd=discovery)
    os.makedirs(os.path.join(discovery, "src", "bin"))
    with open(os.path.join(discovery, "src", "bin", "other.rs"), "w", encoding="utf-8") as handle:
        handle.write("fn main() {}\n")
    _, code, _, stderr = case.run("discovered binary", [], discovery, None, same_spawn)
    if code != 101 or SKIP_LINE in stderr or "could not determine which binary to run" not in stderr:
        fail(f"an added binary did not reach Cargo's ambiguity refusal: exit {code} {stderr[-600:]!r}")

    # Registry packages: eligible without a build script, refused with one.
    seed_registry(runner)
    registry = os.path.join(runner.parent, "checkouts", "registry")
    write_fixture(registry, SKIP_REGISTRY)
    commit_lock(runner, registry)
    case.builds("registry dependency", ["--", "x"], "registry unix x", cwd=registry)
    if not skip_records(registry):
        fail("a registry dependency without a build script published no record")
    case.skips("registry repeat", ["--", "y"], "registry unix y", cwd=registry)
    scripted = os.path.join(runner.parent, "checkouts", "scripted")
    write_fixture(scripted, SKIP_SCRIPTED)
    commit_lock(runner, scripted)
    stderr = case.builds("registry build script", [], "scripted true", cwd=scripted)
    if skip_records(scripted) or "oxide: run skip not recorded" not in stderr or "build script" not in stderr:
        fail(f"a registry build-script dependency was not refused explicitly: {stderr[-600:]!r}")
    case.builds("registry build script repeat", [], "scripted true", cwd=scripted)

    if any(slot["state"] != "idle" for slot in runner.lease_status()["slots"]):
        fail("the skip cases left a slot held")
    return evidence


def timing_verdict(skip: list, leased: list, plain: list) -> dict:
    """#079's fixed two-part gate. Skip and leased samples are `oxide run`
    wall times that both execute the program; plain samples are warmed bare
    `cargo build` no-ops (build only). Part 1: the slowest skip is at most
    half the fastest leased run. Part 2: the fastest skip is at most the
    fastest plain no-op plus the larger observed spread (max - min) of the
    skip and plain samples. Fewer than three samples is insufficient."""
    if min(len(skip), len(leased), len(plain)) < 3:
        return {"failures": ["insufficient timing samples"], "part1": False, "part2": False}
    noise = max(max(skip) - min(skip), max(plain) - min(plain))
    part1 = max(skip) <= min(leased) / 2
    part2 = min(skip) <= min(plain) + noise
    failures = []
    if not part1:
        failures.append(f"part 1: slowest skip {max(skip)} ns > half of fastest leased {min(leased)} ns")
    if not part2:
        failures.append(f"part 2: fastest skip {min(skip)} ns > fastest plain {min(plain)} ns + noise {noise} ns")
    return {"failures": failures, "part1": part1, "part2": part2, "noise_ns": noise}


def prove_timing_verdict() -> None:
    """Pure checks of the gate on synthetic samples, before any native work."""
    ms = lambda *values: [value * 10**6 for value in values]
    if timing_verdict(ms(16, 17, 18), ms(430, 440, 450), ms(16, 16.5, 17))["failures"]:
        fail("timing gate refused a tie within noise")
    if len(timing_verdict(ms(16, 17, 18), ms(30, 31, 32), ms(16, 16.5, 17))["failures"]) != 1:
        fail("timing gate accepted a skip not materially faster than leased")
    if len(timing_verdict(ms(30, 30.5, 31), ms(430, 440, 450), ms(16, 16.5, 17))["failures"]) != 1:
        fail("timing gate accepted a skip slower than plain beyond noise")
    if timing_verdict(ms(16, 17), ms(430, 440, 450), ms(16, 16.5, 17))["failures"] != ["insufficient timing samples"]:
        fail("timing gate accepted too few samples")


def measure_skip_timing(runner: Runner, bounded) -> dict:
    """Three skips (`oxide run -- b`, zero children), three leased no-op runs
    (`oxide run --rebuild -- b`) and, after one warm-up, three bare
    `cargo build --profile oxide-quick` no-ops on the same fixture, profile,
    compiler and config, in a private target directory. Every sample uses the
    same preexec spawn path; wall times are `perf_counter_ns` around the
    request. Raw samples and exits are kept."""
    nest = os.path.join(runner.parent, "checkouts", "timing")
    repo = os.path.join(nest, "repo")
    write_fixture(nest, SKIP_FIXTURE)
    commit_lock(runner, repo)
    case = SkipCase(runner, repo)
    samples = []
    case.builds("timing setup", ["--", "a"], "skip v1 old 5 - a")
    skip = []
    for _ in range(3):
        ns = case.skips("timing skip", ["--", "b"], "skip v1 old 5 - b")
        skip.append(ns)
        samples.append({"kind": "skip", "ns": ns, "exit": 0, "children": 0})
    leased = []
    for _ in range(3):
        ns, code, stdout, stderr = case.run("timing leased", ["--rebuild", "--", "b"], None, None, same_spawn)
        expect(code, 0, "timing leased", stderr)
        if SKIP_LINE in stderr or stdout != "skip v1 old 5 - b":
            fail(f"timing leased sample did not build and run: {stdout!r} {stderr[-400:]!r}")
        leased.append(ns)
        samples.append({"kind": "leased", "ns": ns, "exit": code})
    plain_target = os.path.join(runner.parent, "timing-plain")
    plain_env = runner.environment({**QUICK_PROFILE_ENVIRONMENT, "CARGO_TARGET_DIR": plain_target})
    plain = []
    for n in range(4):
        ns, code, _, stderr = timed(
            ["cargo", "build", "--offline", "-q", "--profile", "oxide-quick"], repo, plain_env, same_spawn
        )
        expect(code, 0, f"bare cargo build {n}", stderr)
        if n:
            plain.append(ns)
            samples.append({"kind": "plain", "ns": ns, "exit": code})
    bounded("timing samples")
    shutil.rmtree(plain_target)
    return {"skip_ns": skip, "leased_ns": leased, "plain_ns": plain, "samples": samples}


def run_skip_main() -> int:
    """The focused `oxide run` fast-path proof: one fresh 128 MiB APFS pool,
    300 s from admission including product staging. Staging (offline, its
    target and temporary directories), fixtures, copies, the pool backing and
    every fixture temporary directory live under one owned parent, whose
    allocation is sampled after every phase against 512 MiB; the host
    reserve is sampled with it."""
    global STARTED
    STARTED = time.monotonic()
    prove_timing_verdict()
    if sys.platform != "darwin":
        fail("the bounded APFS run-skip proof requires macOS")
    root = scratch_root()
    free = host_free_bytes(root)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve")
    parent = os.path.join(root, "oxide-run-skip")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    peak = {"bytes": 0, "phase": ""}

    def bounded(phase: str) -> None:
        owned = tree_bytes(parent, True)
        if owned > peak["bytes"]:
            peak.update({"bytes": owned, "phase": phase})
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB after {phase}")
        free = host_free_bytes(parent)
        if free < HOST_RESERVE_BYTES:
            fail(f"host free {free} fell below the 20 GiB reserve after {phase}")

    executables = build_binaries(os.path.join(parent, "staging"))
    print(f"oxide product staging: {time.monotonic() - STARTED:.1f} s (offline, host Cargo home, owned target)")
    bounded("staging")
    runner = Runner(executables["oxide"], executables["oxide-storage-lease"], parent, skip=True)
    code, _, stderr = runner.run(
        runner.parent,
        ["--pool", runner.pool, "init", "--cap-bytes", str(IMAGE_BYTES), "--image-bytes", str(IMAGE_BYTES)],
        program=runner.lease,
    )
    expect(code, 0, "pool init", stderr)
    bounded("pool init")
    try:
        measured = measure_skip_timing(runner, bounded)
        verdict = timing_verdict(measured["skip_ns"], measured["leased_ns"], measured["plain_ns"])
        print("oxide-run-skip-timing-json: " + json.dumps({"verdict": verdict, **measured}, sort_keys=True))
        if verdict["failures"]:
            fail(f"no-go: fixed timing gate failed: {'; '.join(verdict['failures'])}; scratch retained at {parent}")
        evidence = prove_skip_cases(runner)
        bounded("correctness cases")
        inputs = sum(len(text.encode()) for fixture in (SKIP_FIXTURE, SKIP_FIXTURE, SKIP_REGISTRY, SKIP_SCRIPTED, SKIP_DISCOVERY) for text in fixture.values())
        inputs += len(SKIP_DISCOVERY_SCRIPT.encode())
        inputs += sum(os.path.getsize(os.path.join(runner.cargo_home, relative)) for relative in SKIP_REGISTRY_INPUTS)
        if inputs > SOURCE_LIMIT_BYTES:
            fail(f"fixture inputs {inputs} exceed 8 MiB")
    except Exception:
        print(f"oxide run-skip proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    payload = os.path.join(runner.pool, "payload")
    detach = subprocess.run(["/usr/bin/hdiutil", "detach", payload], capture_output=True, text=True, timeout=60)
    if detach.returncode:
        fail(f"detach failed after positive completion: {detach.stderr}")
    bounded("detach")
    elapsed = time.monotonic() - STARTED
    shutil.rmtree(parent)
    free = host_free_bytes(root)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} fell below the 20 GiB reserve after cleanup")
    ms = lambda values: ", ".join(f"{value / 1e6:.3f}" for value in values)
    print(
        f"oxide run-skip proof held: {elapsed:.0f} s, peak owned scratch {peak['bytes'] // (1024 * 1024)} MiB "
        f"(after {peak['phase']}; staging, temp, copies and pool backing included), fixture inputs {inputs} bytes; "
        f"record {evidence['record_inputs']} inputs ({evidence['record_absent']} absent); "
        f"skip [{ms(measured['skip_ns'])}] ms, leased [{ms(measured['leased_ns'])}] ms, "
        f"bare cargo build [{ms(measured['plain_ns'])}] ms, noise {verdict['noise_ns'] / 1e6:.3f} ms: "
        "part 1 pass, part 2 pass (fixture only, no real-world claim)"
    )
    return 0


def main() -> int:
    global STARTED
    # The clock starts at admission, before any provisioning or staging; the
    # host reserve is observed before effects.
    STARTED = time.monotonic()
    root = scratch_root()
    if sys.platform != "darwin":
        fail("the bounded APFS oxide proof requires macOS")
    free = host_free_bytes(root)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve")
    parent = os.path.join(root, "oxide-cli")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    peak = {"bytes": 0, "phase": ""}

    def bounded(phase: str) -> None:
        owned = tree_bytes(parent, True)
        if owned > peak["bytes"]:
            peak.update({"bytes": owned, "phase": phase})
        if owned > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {owned} exceeds 512 MiB after {phase}")
        free = host_free_bytes(parent)
        if free < HOST_RESERVE_BYTES:
            fail(f"host free {free} fell below the 20 GiB reserve after {phase}")

    prove_wait_lines()
    # Offline product staging under the owned, accounted parent.
    executables = build_binaries(os.path.join(parent, "staging"))
    print(f"oxide product staging: {time.monotonic() - STARTED:.0f} s (offline, host Cargo home, owned target)")
    bounded("staging")
    runner = Runner(executables["oxide"], executables["oxide-storage-lease"], parent)
    app = os.path.join(parent, "checkouts", "app")
    write_fixture(app)
    if tree_bytes(app, False) > SOURCE_LIMIT_BYTES:
        fail("fixture source exceeds 8 MiB")
    prove_usage(runner, app)
    prove_adoption(runner, os.path.join(parent, "adoption"))
    bounded("adoption")
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
    bounded("native cases")
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
    bounded("budget, pool-cap and Cargo-home cases")
    elapsed = time.monotonic() - STARTED
    shutil.rmtree(parent)
    free = host_free_bytes(root)
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} fell below the 20 GiB reserve after cleanup")
    print(
        f"oxide CLI proof held: {elapsed:.0f} s from admission, peak owned scratch {peak['bytes'] // (1024 * 1024)} MiB "
        f"after {peak['phase']} (staging, temp, copies and backing included), mounted payload {mounted // (1024 * 1024)} MiB"
    )
    return 0


if __name__ == "__main__":
    try:
        if sys.argv[1:] == ["--run-skip"]:
            sys.exit(run_skip_main())
        if sys.argv[1:]:
            fail(f"unknown arguments {sys.argv[1:]}")
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
