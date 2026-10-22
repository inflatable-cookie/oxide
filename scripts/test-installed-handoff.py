#!/usr/bin/env python3
"""Focused installed-binary hand-off proof for oxide#122.

One fixed script and whole command under the qualified shared supervisor
(`bounded-test.py`): the brief's 120 s wall limit and 64 MiB monitored scratch
budget including cleanup. It stages this clean HEAD's debug `oxide` itself with
plain Cargo and the pinned 1.97.1, offline, into its own scratch target, copies
the binary and deletes the target before the fixtures. No Oxide pool, no
installed binary, no host state and no network are involved. The stage is kept
inside the 64 MiB gate by single-threaded, single-codegen-unit, debug-free,
symbol-stripped, size-optimised codegen: the staged tree peaks near 58 MiB,
the fixtures near 2 MiB, and staging takes about 37 s.

Every fixture has a private HOME with receipts and a candidate stub, a private
`OXIDE_POOL` directory pool with planted control files, a private CARGO_HOME,
and a PATH whose only `oxide` is the fixture candidate. Cases cover: newer
state and registration versions; malformed, older and non-numeric documents
that must not trigger; a valid candidate that is executed exactly once with
byte-exact argv and the caller's environment plus the internal marker; the
marker refusing and being inert without newer state; every candidate,
ownership, path and replacement refusal; the running binary itself; a
replacement between hashing and the last check; skipped commands; and a
command that reaches final release after effects with newer state, which must
never execute anything.

Not a benchmark, not full QA and not real-drive or host-install qualification.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import json
import os
import shlex
import shutil
import stat
import subprocess
import tempfile
import time

MIB = 1024 * 1024
NOTICE = "oxide: handing off to the installed binary (state is newer than this one).\n"
REFUSAL = "oxide: oxide was upgraded during this command; re-run.\n"
STATE_VERSION = 2
REGISTRATION_VERSION = 2
CAPABILITIES_PROBE = "__oxide-build-capabilities"
SHUTDOWN_MARGIN_S = 20
CASES = []
# The platform toolchain stays reachable for `rustc -vV` and `cargo -V`
# probes; the fixture's own `cargo` and `oxide` stay first on PATH.
TOOLCHAIN_BIN = os.path.dirname(shutil.which("rustc") or "")


def fail(message: str) -> None:
    print(f"installed-handoff proof failed: {message}", file=sys.stderr)
    sys.exit(1)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def case(name: str) -> None:
    CASES.append(name)
    print(f"case {name}")


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 3:
        fail("work deadline reached before the supervisor's shutdown margin")
    return left


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def write(path: str, contents: str, mode: int) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(contents)
    os.chmod(path, mode)


def current_state(version: int = STATE_VERSION) -> str:
    return json.dumps(
        {
            "version": version,
            "next_id": 1,
            "next_use": 1,
            "tickets": [],
            "cleans": [],
            "records": [],
        }
    )


STUB_SOURCE = r"""
use std::io::Write;

fn main() {
    let record = std::env::var_os("OXIDE_HANDOFF_RECORD").expect("record path");
    let stem = record.to_string_lossy();
    let counts = format!("{stem}.count");
    let count = std::fs::read_to_string(&counts)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
        + 1;
    std::fs::write(&counts, count.to_string()).unwrap();
    let mut argv = std::fs::File::create(format!("{stem}.argv")).unwrap();
    for argument in std::env::args_os() {
        argv.write_all(argument.as_encoded_bytes()).unwrap();
        argv.write_all(b"\0").unwrap();
    }
    let mut env = std::fs::File::create(format!("{stem}.env")).unwrap();
    for (name, value) in std::env::vars_os() {
        env.write_all(name.as_encoded_bytes()).unwrap();
        env.write_all(b"=").unwrap();
        env.write_all(value.as_encoded_bytes()).unwrap();
        env.write_all(b"\0").unwrap();
    }
    let code = std::env::var("OXIDE_HANDOFF_EXIT")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(42);
    std::process::exit(code);
}
"""

STUB = ""


def compile_stub(root: str) -> str:
    """One tiny std-only recording binary, shared by hard link across cases.

    It records its own argv (including argv[0]) and environment, so the proof
    can show that a hand-off preserves both, and exits with
    OXIDE_HANDOFF_EXIT.
    """
    global STUB
    source = os.path.join(root, "handoff-stub.rs")
    write(source, STUB_SOURCE, 0o644)
    binary = os.path.join(root, "handoff-stub")
    result = subprocess.run(
        ["rustc", "--edition", "2021", "-O", "-o", binary, source],
        capture_output=True,
        text=True,
        timeout=min(60, work_left()),
    )
    expect(result.returncode == 0, f"stub compile failed: {result.stderr[-2000:]}")
    os.unlink(source)
    STUB = binary
    return binary


def install_stub(destination: str) -> None:
    try:
        os.link(STUB, destination)
    except OSError:
        shutil.copy2(STUB, destination)
    os.chmod(destination, 0o755)


REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"


def stage_subject(root: str) -> str:
    """Stage this HEAD's `oxide` with plain Cargo, offline, in owned scratch.

    No Oxide pool is created or touched, and the staged target is deleted once
    the binary is copied out, so the proof's own writes stay tiny.
    """
    stage = os.path.join(root, "stage")
    log = os.path.join(root, "stage.log")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", "1", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(root, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage)
    os.unlink(log)
    # The installer's hidden probe must report this build's own versions; a
    # future bump then fails this proof loudly instead of mis-testing it.
    probe = subprocess.run(
        [subject, CAPABILITIES_PROBE],
        capture_output=True,
        text=True,
        timeout=min(30, work_left()),
    )
    expect(probe.returncode == 0, f"capability probe exit {probe.returncode}: {probe.stderr!r}")
    expect(
        json.loads(probe.stdout) == {"control_state_version": STATE_VERSION, "registration_version": REGISTRATION_VERSION},
        f"capability probe {probe.stdout!r} disagrees with this proof's versions",
    )
    return subject


class Fixture:
    """One private HOME, install receipts and directory pool."""

    def __init__(self, root: str):
        self.root = root
        self.home = os.path.join(root, "home")
        self.pool = os.path.join(root, "pool")
        self.control = os.path.join(self.pool, "oxide-managed")
        self.bin = os.path.join(self.home, ".local/bin")
        self.receipts = os.path.join(self.home, ".local/state/oxide/install")
        self.cargo_home = os.path.join(root, "cargo-home")
        self.tools = os.path.join(root, "tools")
        for directory, mode in (
            (self.home, 0o700),
            (os.path.join(self.home, ".local"), 0o700),
            (self.bin, 0o700),
            (self.home + "/.local/state", 0o700),
            (os.path.join(self.home, ".local/state/oxide"), 0o700),
            (self.receipts, 0o700),
            (self.pool, 0o700),
            (self.control, 0o700),
            (self.cargo_home, 0o700),
            (self.tools, 0o700),
        ):
            os.makedirs(directory, exist_ok=True)
            os.chmod(directory, mode)
        write(
            os.path.join(self.pool, "config.json"),
            json.dumps({"backend": "directory-budget-v1"}),
            0o644,
        )
        self.record = os.path.join(root, "record")
        # A recording Cargo that answers version probes and fails workspace
        # discovery: no fixture ever starts the real Cargo, so no toolchain
        # download or real build can leak into the proof.
        write(
            os.path.join(self.tools, "cargo"),
            "#!/bin/sh\n"
            + 'printf "%s\\n" "$@" >> ' + shlex.quote(f"{self.record}.cargo") + "\n"
            + 'if [ "$1" = "-V" ] || [ "$1" = "--version" ]; then\n'
            + "  printf 'cargo 1.97.1 (fixture)\\n'\n"
            + "  exit 0\n"
            + "fi\n"
            + "exit 1\n",
            0o755,
        )

    @property
    def destination(self) -> str:
        return os.path.join(self.bin, "oxide")

    def plant_state(self, version: int) -> None:
        write(os.path.join(self.control, "state.json"), current_state(version), 0o600)

    def plant_registration(self, version: int) -> None:
        write(
            os.path.join(self.control, "removable.json"),
            json.dumps({"version": version}),
            0o600,
        )

    def stub(self) -> None:
        """Install the compiled recording candidate as this fixture's oxide."""
        install_stub(self.destination)

    def receipt(
        self,
        name: str = "20260101T000000Z-aaaaaaaaaaaa.json",
        *,
        path: str | None = None,
        digest: str | None = None,
        size: int | None = None,
        state: int | None = STATE_VERSION + 1,
        registration: int | None = REGISTRATION_VERSION,
    ) -> None:
        installed = {
            "path": path if path is not None else self.destination,
            "sha256": digest if digest is not None else sha256(self.destination),
            "size": size if size is not None else os.path.getsize(self.destination),
        }
        document = {"installed": installed}
        if state is not None:
            document["control_state_version"] = state
        if registration is not None:
            document["registration_version"] = registration
        write(os.path.join(self.receipts, name), json.dumps(document), 0o644)

    def env(self, binary: str) -> dict:
        return {
            "HOME": self.home,
            "OXIDE_POOL": self.pool,
            "CARGO_HOME": self.cargo_home,
            "PATH": ":".join(
                [
                    self.bin,
                    self.tools,
                    "/usr/bin",
                    "/bin",
                    "/usr/sbin",
                    "/sbin",
                    TOOLCHAIN_BIN,
                ]
            ),
            "OXIDE_TASK_SCRATCH_ROOT": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "TMPDIR": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "TMP": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "TEMP": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "OXIDE_HANDOFF_TOKEN": f"token-{os.path.basename(self.root)}",
            "OXIDE_HANDOFF_RECORD": self.record,
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", os.path.join(os.path.expanduser("~"), ".rustup")),
        }

    def control_listing(self) -> list:
        return sorted(os.listdir(self.control))

    def cargo_calls(self) -> list:
        path = f"{self.record}.cargo"
        if not os.path.exists(path):
            return []
        with open(path, encoding="utf-8") as handle:
            return handle.read().splitlines()

    def stub_runs(self) -> int:
        path = f"{self.record}.count"
        if not os.path.exists(path):
            return 0
        with open(path, encoding="utf-8") as handle:
            return int(handle.read() or "0")

    def stub_argv(self) -> list:
        with open(f"{self.record}.argv", "rb") as handle:
            entries = [entry for entry in handle.read().split(b"\0") if entry]
        return [entry.decode() for entry in entries]

    def stub_env(self) -> dict:
        with open(f"{self.record}.env", "rb") as handle:
            entries = [entry for entry in handle.read().split(b"\0") if entry]
        captured = {}
        for entry in entries:
            name, _, value = entry.partition(b"=")
            captured[name.decode()] = value.decode()
        return captured



def run(fixture: Fixture, binary: str, args: list, env_extra: dict | None = None, timeout: float = 30):
    env = fixture.env(binary)
    if env_extra:
        env.update(env_extra)
    limit = min(timeout, work_left())
    return subprocess.run(
        [binary, *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=limit,
    )


def expect_handoff_refusal(result, fixture: Fixture, name: str, before: list | None = None) -> None:
    expect(result.returncode == 75, f"{name}: exit {result.returncode}, stderr {result.stderr!r}")
    expect(result.stderr == REFUSAL, f"{name}: stderr {result.stderr!r} is not the pinned refusal")
    expect(result.stdout == "", f"{name}: stdout {result.stdout!r}")
    expect(fixture.stub_runs() == 0, f"{name}: the candidate ran")
    expect(fixture.cargo_calls() == [], f"{name}: Cargo ran")
    if before is not None:
        expect(fixture.control_listing() == before, f"{name}: control changed to {fixture.control_listing()}")


def newer_fixture(root: str, name: str) -> Fixture:
    fixture = Fixture(os.path.join(root, name))
    fixture.plant_state(STATE_VERSION + 1)
    return fixture


def check_handoff(root: str, binary: str) -> None:
    case("newer_state_hands_off_once")
    fixture = newer_fixture(root, "handoff")
    fixture.stub()
    fixture.receipt()
    argv = ["check", "--profile", "debug", "odd arg with spaces", "ünicode"]
    extra = {"OXIDE_HANDOFF_EXIT": "42"}
    before = fixture.control_listing()
    result = run(fixture, binary, argv, extra)
    expect(result.returncode == 42, f"hand-off exit {result.returncode}, stderr {result.stderr!r}")
    expect(result.stderr == NOTICE, f"hand-off stderr {result.stderr!r} is not one notice")
    expect(fixture.stub_runs() == 1, f"candidate ran {fixture.stub_runs()} times")
    expect(fixture.stub_argv() == [binary, *argv], f"argv {fixture.stub_argv()!r}")
    captured = fixture.stub_env()
    expected_env = dict(fixture.env(binary), **extra)
    expected_env["OXIDE_INTERNAL_HANDED_OFF"] = "1"
    expect(captured == expected_env, f"environment differs: {captured!r} vs {expected_env!r}")
    expect(fixture.control_listing() == before, f"control changed to {fixture.control_listing()}")
    expect(fixture.cargo_calls() == [], "Cargo ran during a hand-off")

    case("marker_present_refuses_even_with_newer_state")
    fixture = newer_fixture(root, "marker-refuses")
    fixture.stub()
    fixture.receipt()
    for value in ("1", "0", ""):
        result = run(fixture, binary, ["status"], {"OXIDE_INTERNAL_HANDED_OFF": value})
        expect_handoff_refusal(result, fixture, f"marker {value!r}")

    case("marker_is_inert_without_newer_state")
    fixture = Fixture(os.path.join(root, "marker-inert"))
    fixture.plant_state(STATE_VERSION)
    fixture.stub()
    fixture.receipt()
    result = run(fixture, binary, ["status"], {"OXIDE_INTERNAL_HANDED_OFF": "1"})
    expect(result.returncode == 0, f"marker inert exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr, "marker inert printed the hand-off notice")
    expect(REFUSAL not in result.stderr, "marker inert refused")
    expect(fixture.stub_runs() == 0, "marker inert ran the candidate")


def check_no_trigger(root: str, binary: str) -> None:
    case("non_numeric_older_and_malformed_documents_do_not_trigger")
    documents = {
        "malformed": "{",
        "array": "[]",
        "older": current_state(STATE_VERSION - 1),
        "string_version": json.dumps({"version": str(STATE_VERSION + 1)}),
        "negative": json.dumps({"version": -1}),
        "above_u32": json.dumps({"version": 2**32}),
        "float": json.dumps({"version": 3.5}),
        "missing_version": "{}",
    }
    for name, contents in documents.items():
        fixture = Fixture(os.path.join(root, f"document-{name}"))
        fixture.plant_state(STATE_VERSION)
        write(os.path.join(fixture.control, "state.json"), contents, 0o600)
        fixture.stub()
        fixture.receipt()
        result = run(fixture, binary, ["status"])
        expect(result.returncode == 0, f"{name}: exit {result.returncode}, stderr {result.stderr!r}")
        expect(NOTICE not in result.stderr, f"{name}: hand-off notice printed")
        expect(REFUSAL not in result.stderr, f"{name}: refused as newer")
        expect(fixture.stub_runs() == 0, f"{name}: candidate ran")

    case("malformed_and_oversized_documents_keep_the_existing_refusal")
    fixture = Fixture(os.path.join(root, "oversized"))
    write(
        os.path.join(fixture.control, "state.json"),
        json.dumps({"version": STATE_VERSION, "padding": "x" * (17 * MIB)}),
        0o600,
    )
    result = run(fixture, binary, ["status"])
    expect(result.returncode == 0, f"oversized exit {result.returncode}")
    expect(REFUSAL not in result.stderr and NOTICE not in result.stderr, "oversized document handed off")
    shutil.rmtree(fixture.root, ignore_errors=True)

    case("no_pool_means_no_trigger")
    fixture = Fixture(os.path.join(root, "no-pool"))
    shutil.rmtree(fixture.pool)
    fixture.stub()
    fixture.receipt()
    result = run(fixture, binary, ["status"])
    expect(result.returncode == 0, f"no pool exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, "no pool handed off")
    expect(not os.path.exists(fixture.pool), "status created the pool")
    expect(fixture.stub_runs() == 0, "no pool ran the candidate")


def check_candidate_refusals(root: str, binary: str) -> None:
    case("newer_registration_without_a_candidate_refuses")
    fixture = Fixture(os.path.join(root, "registration"))
    fixture.plant_state(STATE_VERSION)
    fixture.plant_registration(REGISTRATION_VERSION + 1)
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "registration", before)

    case("newer_state_without_receipts_or_candidate_refuses")
    fixture = newer_fixture(root, "no-receipt")
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "no receipt", before)
    fixture.stub()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "no receipt with candidate", before)

    def refused(name, mutate, *, args=None, env_for=None):
        fixture = newer_fixture(root, f"refuse-{name}")
        fixture.stub()
        fixture.receipt()
        mutate(fixture)
        before = fixture.control_listing()
        extra = env_for(fixture) if env_for else None
        result = run(fixture, binary, args or ["status"], extra)
        expect_handoff_refusal(result, fixture, name, before)

    refused(
        "hash",
        lambda fixture: fixture.receipt(digest=hashlib.sha256(b"other").hexdigest()),
    )
    refused("size", lambda fixture: fixture.receipt(size=os.path.getsize(fixture.destination) + 1))
    refused("missing_capabilities", lambda fixture: fixture.receipt(state=None, registration=None))
    refused("low_state_capability", lambda fixture: fixture.receipt(state=STATE_VERSION))
    refused(
        "receipt_path",
        lambda fixture: fixture.receipt(path=os.path.join(fixture.root, "elsewhere/oxide")),
    )
    refused("not_executable", lambda fixture: os.chmod(fixture.destination, 0o644))
    refused(
        "symlinked_candidate",
        lambda fixture: (
            os.rename(fixture.destination, os.path.join(fixture.root, "real-oxide")),
            os.symlink(os.path.join(fixture.root, "real-oxide"), fixture.destination),
        ),
    )
    refused(
        "symlinked_bin_directory",
        lambda fixture: (
            os.rename(fixture.bin, os.path.join(fixture.root, "moved-bin")),
            os.symlink(os.path.join(fixture.root, "moved-bin"), fixture.bin),
        ),
    )
    refused("no_path_entry", lambda fixture: None, env_for=lambda fixture: {"PATH": "/usr/bin:/bin"})
    refused(
        "another_oxide_on_path",
        lambda fixture: write(os.path.join(fixture.root, "other/oxide"), "#!/bin/sh\nexit 0\n", 0o755),
        env_for=lambda fixture: {
            "PATH": ":".join(
                [
                    os.path.dirname(os.path.join(fixture.root, "other/oxide")),
                    fixture.bin,
                    fixture.tools,
                    "/usr/bin",
                    "/bin",
                    "/usr/sbin",
                    "/sbin",
                ]
            )
        },
    )

    case("a_receipt_that_does_not_cover_a_newer_registration_refuses")
    fixture = Fixture(os.path.join(root, "low-registration"))
    fixture.plant_state(STATE_VERSION)
    fixture.plant_registration(REGISTRATION_VERSION + 1)
    fixture.stub()
    fixture.receipt(state=STATE_VERSION, registration=REGISTRATION_VERSION)
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "low registration", before)

    case("the_newest_receipt_is_the_only_candidate")
    fixture = newer_fixture(root, "newest-receipt")
    fixture.stub()
    fixture.receipt("20260101T000000Z-aaaaaaaaaaaa.json")
    write(
        os.path.join(fixture.receipts, "20260201T000000Z-bbbbbbbbbbbb.json"),
        json.dumps({"installed": {"path": "/elsewhere/oxide", "sha256": "0" * 64, "size": 1}}),
        0o644,
    )
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "newest receipt", before)

    case("a_replacement_before_the_last_check_refuses")
    fixture = newer_fixture(root, "replacement")
    fixture.stub()
    fixture.receipt()
    replacement = os.path.join(fixture.root, "replacement")
    with open(STUB, "rb") as handle:
        replaced = handle.read() + b"\0"
    with open(replacement, "wb") as handle:
        handle.write(replaced)
    os.chmod(replacement, 0o755)
    before = fixture.control_listing()
    result = run(
        fixture,
        binary,
        ["status"],
        {"OXIDE_TEST_HANDOFF_REPLACE": replacement},
    )
    expect_handoff_refusal(result, fixture, "replacement", before)
    with open(fixture.destination, "rb") as handle:
        expect(handle.read() == replaced, "the replacement hook did not replace the candidate")


def check_skipped_commands(root: str, binary: str) -> None:
    case("plain_passthrough_skips_the_hand_off_check")
    fixture = newer_fixture(root, "skip-plain")
    fixture.stub()
    fixture.receipt()
    write(
        os.path.join(fixture.tools, "cargo"),
        "#!/bin/sh\n"
        + 'printf "%s\\n" "$@" > ' + shlex.quote(f"{fixture.record}.cargo") + "\n"
        + 'printf "cargo ran\\n"\nexit 0\n',
        0o755,
    )
    result = run(fixture, binary, ["fmt", "--all", "--", "--check"])
    expect(result.returncode == 0, f"plain exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, "plain passthrough handed off")
    expect(fixture.stub_runs() == 0, "plain passthrough ran the candidate")
    expect(fixture.cargo_calls() == ["fmt", "--all", "--", "--check"], f"cargo calls {fixture.cargo_calls()}")

    case("usage_only_exec_skips_the_hand_off_check")
    fixture = newer_fixture(root, "skip-exec")
    for args, code in ((["exec", "--help"], 0), (["exec"], 1)):
        result = run(fixture, binary, args)
        expect(result.returncode == code, f"exec usage {args} exit {result.returncode}: {result.stderr!r}")
        expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, f"exec usage {args} handed off")

    case("launch_check_probe_skips_the_hand_off_check")
    fixture = newer_fixture(root, "skip-launch")
    result = run(
        fixture,
        binary,
        ["status", "--launch-check"],
        {"OXIDE_TEST_LAUNCH_LIMIT_MS": "20000"},
    )
    expect(result.returncode == 0, f"launch check exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, "launch check handed off")


def check_same_binary(root: str, binary: str) -> None:
    case("the_running_binary_refuses_to_hand_off_to_itself")
    fixture = newer_fixture(root, "same-inode")
    subject = os.path.join(fixture.root, "subject-oxide")
    try:
        os.link(binary, subject)
    except OSError:
        shutil.copy2(binary, subject)
    if os.path.lexists(fixture.destination):
        os.unlink(fixture.destination)
    try:
        os.link(subject, fixture.destination)
    except OSError:
        shutil.copy2(subject, fixture.destination)
        os.chmod(fixture.destination, 0o755)
    fixture.receipt()
    before = fixture.control_listing()
    result = run(fixture, subject, ["status"])
    expect_handoff_refusal(result, fixture, "same inode", before)
    shutil.rmtree(fixture.root, ignore_errors=True)


def check_final_release(root: str, binary: str) -> None:
    case("a_final_release_error_never_replays_the_command")
    fixture = Fixture(os.path.join(root, "final-release"))
    # Let the first command create the pool properly (config, ledger and
    # control), so the real run starts from an existing current-version pool.
    shutil.rmtree(fixture.pool)
    workspace = os.path.join(fixture.root, "workspace")
    os.makedirs(workspace)
    write(
        os.path.join(workspace, "Cargo.toml"),
        '[package]\nname = "handoff-fixture"\nversion = "0.1.0"\nedition = "2021"\n',
        0o644,
    )
    write(os.path.join(workspace, "src/lib.rs"), "pub fn value() -> u32 { 1 }\n", 0o644)
    fixture.stub()
    fixture.receipt()
    state = os.path.join(fixture.control, "state.json")
    header = (
        "#!/bin/sh\n"
        + 'printf "%s\\n" "$@" >> ' + shlex.quote(f"{fixture.record}.cargo") + "\n"
        + 'if [ "$1" = "-V" ] || [ "$1" = "--version" ]; then\n'
        + "  printf 'cargo 1.97.1 (fixture)\\n'\n"
        + "  exit 0\n"
        + "fi\n"
        + 'if [ "$1" = "locate-project" ]; then\n'
        + "  printf '%s\\n' " + shlex.quote(os.path.join(workspace, "Cargo.toml")) + "\n"
        + "  exit 0\n"
        + "fi\n"
    )
    # A first command creates the pool and its version-2 state; Cargo fails,
    # so nothing else happens.
    write(os.path.join(fixture.tools, "cargo"), header + "exit 1\n", 0o755)
    scaffold = subprocess.run(
        [binary, "check"],
        env=fixture.env(binary),
        cwd=workspace,
        capture_output=True,
        text=True,
        timeout=min(60, work_left()),
    )
    expect(scaffold.returncode == 1, f"scaffold exit {scaffold.returncode}: {scaffold.stderr!r}")
    with open(state, encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == STATE_VERSION, "the pool did not start at the current state version")
    expect(NOTICE not in scaffold.stderr, "the scaffold handed off")
    # The real command: a newer writer fences the state while Cargo runs.
    # The newer writer waits until the command has recorded its running
    # holder, so the fence lands after the command's last own save and the
    # final release is guaranteed to read it (no write-order race).
    poll = (
        "tries=0\n"
        "while [ $tries -lt 200 ]; do\n"
        "  if grep -q '\"phase\":\"running\"' " + shlex.quote(state) + " 2>/dev/null; then break; fi\n"
        "  sleep 0.05\n"
        "  tries=$((tries + 1))\n"
        "done\n"
    )
    write(
        os.path.join(fixture.tools, "cargo"),
        header + poll + "printf '%s' " + shlex.quote(current_state(STATE_VERSION + 1)) + " > " + shlex.quote(state) + "\nexit 0\n",
        0o755,
    )
    result = subprocess.run(
        [binary, "check"],
        env=fixture.env(binary),
        cwd=workspace,
        capture_output=True,
        text=True,
        timeout=min(60, work_left()),
    )
    expect(result.returncode == 1, f"final release exit {result.returncode}: {result.stderr!r}")
    expect(
        "unsupported version" in result.stderr,
        f"final release stderr {result.stderr!r} is not the existing refusal",
    )
    expect(NOTICE not in result.stderr, "final release printed the hand-off notice")
    expect(REFUSAL not in result.stderr, "final release used the hand-off refusal")
    expect(fixture.stub_runs() == 0, "final release executed the candidate")
    expect(fixture.cargo_calls() != [], "the command never reached Cargo, so it had no effects to replay")
    with open(state, encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == STATE_VERSION + 1, "the newer state was not written")
    expect(
        os.path.exists(os.path.join(fixture.control, "admission.lock")),
        "the pool shows no command effects",
    )

def check_cargo_exit_75(root: str, binary: str) -> None:
    case("cargo_exit_75_is_propagated_and_is_not_a_trigger")
    fixture = Fixture(os.path.join(root, "cargo-75"))
    shutil.rmtree(fixture.pool)
    workspace = os.path.join(fixture.root, "workspace")
    os.makedirs(workspace)
    write(
        os.path.join(workspace, "Cargo.toml"),
        '[package]\nname = "handoff-fixture"\nversion = "0.1.0"\nedition = "2021"\n',
        0o644,
    )
    write(os.path.join(workspace, "src/lib.rs"), "pub fn value() -> u32 { 1 }\n", 0o644)
    fixture.stub()
    fixture.receipt()
    write(
        os.path.join(fixture.tools, "cargo"),
        "#!/bin/sh\n"
        + 'if [ "$1" = "-V" ] || [ "$1" = "--version" ]; then\n'
        + "  printf 'cargo 1.97.1 (fixture)\\n'\n"
        + "  exit 0\n"
        + "fi\n"
        + 'if [ "$1" = "locate-project" ]; then\n'
        + "  printf '%s\\n' " + shlex.quote(os.path.join(workspace, "Cargo.toml")) + "\n"
        + "  exit 0\n"
        + "fi\n"
        + "exit 75\n",
        0o755,
    )
    # The first command creates the pool; the second starts from an existing
    # current-version pool. Both must return Cargo's 75 untouched.
    for label in ("creating", "existing"):
        result = subprocess.run(
            [binary, "check"],
            env=fixture.env(binary),
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=min(60, work_left()),
        )
        expect(result.returncode == 75, f"cargo 75 {label}: exit {result.returncode}: {result.stderr!r}")
        expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, f"cargo 75 {label} handed off")
        expect(fixture.stub_runs() == 0, f"cargo 75 {label} ran the candidate")
    state = os.path.join(fixture.control, "state.json")
    with open(state, encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == STATE_VERSION, "the pool state is not current")


def main() -> int:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    expect(bool(evidence) and os.path.isdir(evidence), "not running under the shared supervisor")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    root = tempfile.mkdtemp(prefix="installed-handoff.", dir=scratch)
    try:
        binary = stage_subject(root)
        digest = sha256(binary)
        print(f"subject {binary} sha256 {digest}")
        compile_stub(root)
        check_handoff(root, binary)
        check_no_trigger(root, binary)
        check_candidate_refusals(root, binary)
        check_skipped_commands(root, binary)
        check_same_binary(root, binary)
        check_cargo_exit_75(root, binary)
        check_final_release(root, binary)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    with open(os.path.join(evidence, "installed-handoff.txt"), "w", encoding="utf-8") as handle:
        handle.write(f"subject {binary}\nsubject sha256 {digest}\n")
        handle.write(f"cases {len(CASES)}\n")
        for name in CASES:
            handle.write(f"ok {name}\n")
    print(f"installed-handoff: {len(CASES)} cases passed")
    return 0

if __name__ == "__main__":
    sys.exit(main())
