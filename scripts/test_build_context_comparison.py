#!/usr/bin/env python3
"""Synthetic fixtures for the private build-context comparison launcher.

Never enters the live image/Cargo path. Fixture roots stay inside one fresh
mktemp directory. Aggregate budget is 60 seconds.
"""
from __future__ import annotations

import hashlib
import io
import json
import os
import py_compile
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from contextlib import redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_context_comparison as bcc

py_compile.compile(str(Path(__file__).resolve().with_name("build_context_comparison.py")), doraise=True)
py_compile.compile(str(Path(__file__).resolve()), doraise=True)

signal.alarm(60)

_SCRATCH_DIR = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
SCRATCH = Path(tempfile.mkdtemp(prefix="oxide-bcc-", dir=_SCRATCH_DIR or None)).resolve()
IMAGEINFO_OK = (
    "Size Information:\n"
    "\tTotal Bytes: 16106127360\n"
    "\tTotal Non-Empty Bytes: 4096\n"
    "\tTotal Empty Bytes: 16106123264\n"
)
IMAGEINFO_OVER = (
    "Size Information:\n"
    "\tTotal Bytes: 19327352832\n"
    "\tTotal Non-Empty Bytes: 17179869185\n"
    "\tTotal Empty Bytes: 0\n"
)
IMAGEINFO_AT_THRESHOLD = (
    "Size Information:\n"
    "\tTotal Bytes: 16106127360\n"
    "\tTotal Non-Empty Bytes: 15032385536\n"
    "\tTotal Empty Bytes: 1073741824\n"
)
TOKEN = "eyJydW5JZCI6InJ1bl8wMDAxIiwiZXBvY2giOjN9.synthetic-fixture-token"
REQUEST_ID = "8f2c0c4e-0f4a-4a4a-9a4a-2b2b2b2b2b2b"
RUSTC_PATH = "/usr/bin/rustc"
CARGO_PATH = "/usr/bin/cargo"
DEFAULT_TEST_LINE = (
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; "
    "finished in 0.01s"
)
DEFAULT_NEGATIVE_STDERR = (
    "thread 'committed_workflow_packages_parse' panicked at tests/packages.rs:11:5:\n"
    "packages directory\n"
    "stack backtrace:\n"
    "   0: rust_begin_unwind\n"
    "             at /rustc/8bab26f4f/library/std/src/panicking.rs:698:5\n"
    "   1: committed_workflow_packages_parse\n"
    "             at crates/nucleus-core/tests/packages.rs:11:5\n"
)
HEADER_ONLY_NEGATIVE_STDERR = (
    "thread 'committed_workflow_packages_parse' panicked at tests/packages.rs:11:5:\n"
    "packages directory\n"
)


class FakeClock:
    def __init__(self, start: int = 1_000_000_000) -> None:
        self.mono = start
        self.utc = "2026-10-15T00:00:00Z"

    def monotonic_ns(self) -> int:
        return self.mono

    def clock_basis(self) -> str:
        return "fake:monotonic"

    def utc_iso(self) -> str:
        return self.utc

    def advance(self, ns: int) -> None:
        self.mono += ns


class FakeImage:
    def __init__(self, clock: FakeClock, info: str = IMAGEINFO_OK) -> None:
        self.clock = clock
        self.info = info
        self.creates: list[tuple[Path, str]] = []
        self.attaches: list[Path] = []
        self.create_at: int | None = None
        self.detached = False

    def create(self, backing: Path, size: str, remaining_ns: int) -> str:
        if remaining_ns <= 0:
            raise bcc.ComparisonError("deadline", "deadline", "create after deadline")
        self.create_at = self.clock.monotonic_ns()
        self.creates.append((backing, size))
        backing.write_bytes(b"img")
        return "created"

    def attach(self, backing: Path, mount: Path, remaining_ns: int) -> str:
        if remaining_ns <= 0:
            raise bcc.ComparisonError("deadline", "deadline", "attach after deadline")
        self.attaches.append(mount)
        mount.mkdir(parents=True, exist_ok=True)
        return "attached"

    def imageinfo(self, backing: Path, remaining_ns: int) -> str:
        if remaining_ns <= 0:
            raise bcc.ComparisonError("deadline", "deadline", "imageinfo after deadline")
        if self.info is None:
            raise bcc.ComparisonError("unknown", "imageinfo", "missing sample")
        return self.info

    def detach(self, mount: Path) -> None:
        self.detached = True


class FakeSpawner:
    def __init__(
        self,
        clock: FakeClock,
        *,
        rustc_ok: bool = True,
        cargo_ok: bool = True,
        advance_ns: int = 1_000_000,
        rustc_path: str = RUSTC_PATH,
        emit_rustc: bool = True,
        emit_debuginfo: bool = True,
        force_incremental: str | None = None,
        test_line: str | None = DEFAULT_TEST_LINE,
        test_ran: bool = True,
        exit_code: int = 0,
        negative_stderr: str | None = None,
    ) -> None:
        self.clock = clock
        self.rustc_ok = rustc_ok
        self.cargo_ok = cargo_ok
        self.advance_ns = advance_ns
        self.rustc_path = rustc_path
        self.emit_rustc = emit_rustc
        self.emit_debuginfo = emit_debuginfo
        self.force_incremental = force_incremental
        self.test_line = test_line
        self.test_ran = test_ran
        self.exit_code = exit_code
        self.negative_stderr = negative_stderr if negative_stderr is not None else DEFAULT_NEGATIVE_STDERR
        self.calls: list[tuple[str, ...]] = []
        self.envs: list[dict[str, str]] = []
        self.unknown_closure = False
        self.effigy_calls = 0

    def _rustc_flags(self, mode: str, crate: str) -> str:
        pieces = [
            f"{self.rustc_path} --crate-name {crate} --edition 2021",
        ]
        if self.emit_debuginfo:
            pieces.append("-C debuginfo=line-tables-only")
        pieces.append("-C split-debuginfo=unpacked")
        if crate == "nucleus-core":
            flag = self.force_incremental
            if flag is None:
                flag = "on" if mode == "on" else "off"
            if flag == "on":
                pieces.append("-C incremental=/scratch/incremental")
        return " ".join(pieces)

    def run(self, argv, *, env, cwd, remaining_ns, log, clock, session, poll=None) -> bcc.RunResult:
        if remaining_ns <= 0 or session.expired_hard(clock.monotonic_ns()):
            return bcc.RunResult(outcome="deadline", spawned=False, argv=tuple(argv), seam="deadline_before_spawn")
        self.calls.append(tuple(argv))
        self.envs.append(dict(env))
        self.clock.advance(self.advance_ns)
        if poll is not None:
            poll()
        stdout = ""
        stderr = ""
        code = 0
        if argv[:2] == ["cargo", "-vV"]:
            stdout = f"cargo {bcc.RUSTC_RELEASE} (aaaaaaaaaaa 2026-01-01)\n" if self.cargo_ok else "cargo 1.96.1\n"
        elif argv[:2] == ["rustc", "-vV"]:
            if self.rustc_ok:
                stdout = f"release: {bcc.RUSTC_RELEASE}\ncommit-hash: {bcc.RUSTC_COMMIT_PREFIX}deadbeef\n"
            else:
                stdout = "release: 1.96.1\ncommit-hash: 000000000\n"
        elif argv and argv[0] == "effigy":
            self.effigy_calls += 1
            index = self.effigy_calls
            step = ("C0", "E1", "E2", "U")[(index - 1) % 4]
            mode = "off" if index <= 4 else "on"
            target = Path(env["CARGO_TARGET_DIR"]) / "debug" / "deps"
            incremental = Path(env["CARGO_TARGET_DIR"]) / "debug" / "incremental"
            target.mkdir(parents=True, exist_ok=True)
            incremental.mkdir(parents=True, exist_ok=True)
            session_dir = incremental / "sess-deadbeef"
            session_dir.mkdir(exist_ok=True)
            (session_dir / "work.o").write_bytes(b"incremental")
            exe = target / "packages-deadbeef"
            if not exe.exists():
                exe.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
                os.chmod(exe, 0o755)
            obj = target / "packages-deadbeef.o"
            if not obj.exists():
                obj.write_bytes(b"obj")
            lines = []
            if step == "U":
                lines.append("       Fresh nucleus-core v0.0.0")
            else:
                lines.append("   Compiling nucleus-core v0.0.0")
                if self.emit_rustc:
                    lines.append(f"     Running `{self._rustc_flags(mode, 'nucleus-core')}`")
            rel = "target/debug/deps/packages-deadbeef"
            lines.append(f"     Running tests/packages.rs ({rel})")
            stderr = "\n".join(lines) + "\n"
            body = ""
            if self.test_ran:
                body += f"test {bcc.TEST_NAME} ... ok\n"
            if self.test_line is not None:
                body += self.test_line + "\n"
            stdout = body
            code = self.exit_code
        elif argv and "packages-deadbeef" in argv[0]:
            stderr = self.negative_stderr
            code = 1
        log.write(stdout + stderr)
        if self.unknown_closure:
            return bcc.RunResult(
                outcome="ok",
                spawned=True,
                argv=tuple(argv),
                exit_code=None,
                pgid=4242,
                writer_closed=False,
                stdout=stdout,
                stderr=stderr,
                duration_ns=self.advance_ns,
            )
        return bcc.RunResult(
            outcome="ok",
            spawned=True,
            argv=tuple(argv),
            exit_code=code,
            pgid=1000 + len(self.calls),
            writer_closed=True,
            stdout=stdout,
            stderr=stderr,
            duration_ns=self.advance_ns,
        )


class RecordingSubmitter:
    def __init__(self, states: list[str]) -> None:
        self.states = list(states)
        self.calls: list[dict[str, object]] = []

    def submit(self, *, argv, cwd, request_id, caller, env, timeout_ns=None):
        binding = Path(cwd) / bcc.STAGED_BINDING_NAME
        self.calls.append(
            {
                "argv": list(argv),
                "cwd": cwd,
                "request_id": request_id,
                "caller": caller,
                "binding_present": binding.is_file(),
            }
        )
        state = self.states.pop(0) if self.states else "held"
        return bcc.HeavySubmitOutcome(state=state, run_id="run_0001" if state == "launched" else None)


class RecoveringSubmitter:
    """Held submit reply, then a found request that is followed to settlement."""

    def __init__(self, follow_state: str = "launched") -> None:
        self.follow_state = follow_state
        self.submits = 0
        self.statuses = 0
        self.follows = 0

    def submit(self, *, argv, cwd, request_id, caller, env, timeout_ns=None):
        self.submits += 1
        return bcc.HeavySubmitOutcome(state="held")

    def request_status(self, *, caller, request_id, env, timeout_ns=None):
        self.statuses += 1
        return bcc.HeavySubmitOutcome(state="found", run_id="run_0001")

    def request_follow(self, *, caller, request_id, env, timeout_ns=None):
        self.follows += 1
        return bcc.HeavySubmitOutcome(state=self.follow_state)


class LostThenAbsentSubmitter:
    """Lost submit reply, status shows authenticated absence, replay succeeds."""

    def __init__(self) -> None:
        self.submits: list[list[str]] = []
        self.statuses = 0

    def submit(self, *, argv, cwd, request_id, caller, env, timeout_ns=None):
        self.submits.append(list(argv))
        if len(self.submits) == 1:
            return bcc.HeavySubmitOutcome(state="held")
        return bcc.HeavySubmitOutcome(state="launched")

    def request_status(self, *, caller, request_id, env, timeout_ns=None):
        self.statuses += 1
        return bcc.HeavySubmitOutcome(state="authenticated_absence")


def _git_ok(argv, cwd=None) -> str:
    if argv[:2] == ["rev-parse", "HEAD"]:
        return bcc.NUCLEUS_COMMIT + "\n"
    if argv and argv[0] == "rev-parse" and "tree" in argv[1]:
        return bcc.NUCLEUS_TREE + "\n"
    if argv == ["ls-files"]:
        return "Cargo.lock\nCargo.toml\n"
    if argv[:1] == ["status"]:
        return ""
    return ""


def _git_bad_tree(argv, cwd=None) -> str:
    if argv[:2] == ["rev-parse", "HEAD"]:
        return bcc.NUCLEUS_COMMIT + "\n"
    if argv and argv[0] == "rev-parse" and "tree" in argv[1]:
        return "0" * 40 + "\n"
    return _git_ok(argv, cwd)


def _clone(plan, mode, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "packages").mkdir(exist_ok=True)
    (dest / "Cargo.lock").write_text("lock\n", encoding="utf-8")
    for rel in bcc.EDIT_FILES:
        path = dest / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fn item() {}\n", encoding="utf-8")


def _copy(src: Path, dest: Path) -> int:
    data = b"crate"
    dest.write_bytes(data)
    os.chmod(dest, 0o600)
    return len(data)


def _plan(scratch: Path, launch_id: str = "fixture") -> bcc.ResourcePlan:
    parent = scratch / f"cmp-{launch_id}"
    parent.parent.mkdir(parents=True, exist_ok=True)
    return bcc.ResourcePlan(
        launch_id=launch_id,
        parent=parent,
        mount=parent / "m",
        backing=parent / "cmp.sparseimage",
        nucleus_source=scratch / "nucleus",
        registry_inputs=(
            bcc.RegistryInput(
                src=bcc.CARGO_CACHE_ROOT / "foo-0.1.0.crate",
                rel="registry/cache/index.crates.io-1949cf8c6b5b557f/foo-0.1.0.crate",
            ),
            bcc.RegistryInput(
                src=bcc.CARGO_INDEX_ROOT / ".cache" / "3" / "foo",
                rel="registry/index/index.crates.io-1949cf8c6b5b557f/.cache/3/foo",
            ),
        ),
    )


def _stage(
    plan: bcc.ResourcePlan,
    registry: bcc.RegistryInputPlan,
    clock: FakeClock,
    *,
    smoke: bool = False,
) -> bcc.StagedSession:
    launcher_path = Path(bcc.__file__).resolve()
    binding = bcc.LaunchBinding(
        caller="northstar-worker",
        request_id=REQUEST_ID,
        request_body_digest="0" * 64,
        parent=plan.parent,
        launcher_sha256=hashlib.sha256(launcher_path.read_bytes()).hexdigest(),
        nucleus_commit=bcc.NUCLEUS_COMMIT,
        nucleus_tree=bcc.NUCLEUS_TREE,
        cargo_lock_sha256=bcc.CARGO_LOCK_SHA256,
        registry_plan_digest=registry.digest(),
        audit_start_utc=clock.utc_iso(),
        deadline_monotonic_ns=clock.monotonic_ns() + bcc.BUDGET_NS,
        declared_budget_ns=bcc.BUDGET_NS,
        mode="smoke" if smoke else "live",
        clock_basis=clock.clock_basis(),
    )
    return bcc.stage_session(
        plan,
        launcher_path=launcher_path,
        binding=binding.to_document(),
        registry_plan=registry,
        smoke=smoke,
    )


def _launcher(scratch: Path, *, plan=None, stage=False, **overrides):
    clock = overrides.pop("clock", FakeClock())
    image = overrides.pop("image", FakeImage(clock))
    spawner = overrides.pop("spawner", FakeSpawner(clock))
    registry = overrides.pop("registry_plan", bcc.derive_registry_input_plan_from_evidence())
    parent_env = overrides.pop(
        "parent_env",
        {
            "PATH": "/usr/bin",
            "HOME": str(scratch / "home"),
            "HOST_RUN_TOKEN": TOKEN,
            "HOST_RUN_ID": "run_0001",
            "EFFIGY_HOST_RUN_ROOT": str(scratch / "host-run"),
            "EFFIGY_HOST_SCHEDULER": "1",
        },
    )
    admitted_root = overrides.pop("admitted_root", None)
    if stage:
        assert plan is not None
        _stage(plan, registry, clock)
        admitted_root = plan.parent
    launcher = bcc.ComparisonLauncher(
        clock=clock,
        spawner=spawner,
        image=image,
        parent_env=parent_env,
        space_probe=overrides.pop("space_probe", lambda: 50 * 1024 ** 3),
        git=overrides.pop("git", _git_ok),
        clone_source=overrides.pop("clone_source", _clone),
        lock_sha256=overrides.pop("lock_sha256", lambda path: bcc.CARGO_LOCK_SHA256),
        config_paths=overrides.pop("config_paths", lambda *args: []),
        strict_names=overrides.pop("strict_names", False),
        budget_ns=overrides.pop("budget_ns", bcc.BUDGET_NS),
        reserve_ns=overrides.pop("reserve_ns", bcc.RESERVE_NS),
        registry_plan=registry,
        admitted_root=admitted_root,
        tool_paths={"rustc": RUSTC_PATH, "cargo": CARGO_PATH},
        caller="northstar-worker",
        request_id=REQUEST_ID,
        copy=overrides.pop("copy", _copy),
        **overrides,
    )
    return launcher, plan, clock, image, spawner


def _admitted_launcher(scratch: Path, name: str, **overrides):
    plan = _plan(scratch / name)
    clock = overrides.pop("clock", FakeClock())
    _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
    launcher, _, _, _, spawner = _launcher(
        scratch, plan=plan, clock=clock, admitted_root=plan.parent, **overrides
    )
    return launcher, plan, spawner


class PlanTests(unittest.TestCase):
    def test_plan_does_not_execute_effects(self) -> None:
        launcher, _, _, image, spawner = _launcher(SCRATCH)
        document = launcher.run(live=False, resource_plan=None)
        self.assertFalse(document["effects_executed"])
        self.assertEqual(document["outcome"], "plan")
        self.assertEqual(len(document["count"]["planned"]), 10)
        self.assertFalse(document["cargo_offline_readonly"])
        self.assertEqual(image.creates, [])
        self.assertEqual(spawner.calls, [])
        self.assertIn(bcc.CLOCK_BASIS, document["clocks"]["deadline_clock"])

    def test_plan_reports_heavy_entry_and_registry_plan(self) -> None:
        launcher, _, _, _, _ = _launcher(SCRATCH)
        plan = _plan(SCRATCH / "packet")
        document = launcher.plan(plan)
        packet = document["launch_packet"]
        self.assertEqual(packet["heavy_selector"], bcc.HEAVY_SELECTOR)
        self.assertEqual(packet["admission"], "heavy")
        self.assertEqual(packet["token_root"], str(plan.parent))
        self.assertIn('admission = "heavy"', packet["manifest_template"])
        self.assertIn("--host-run-request-id", packet["submit_argv"])
        self.assertGreater(packet["registry_plan"]["archive_count"], 0)
        self.assertGreater(packet["registry_plan"]["index_count"], 0)
        self.assertFalse(document["effects_executed"])

    def test_live_disabled_without_resource_plan(self) -> None:
        launcher, _, _, _, _ = _launcher(SCRATCH)
        with self.assertRaises(bcc.ComparisonError) as raised:
            launcher.run(live=True, resource_plan=None)
        self.assertEqual(raised.exception.seam, "live_opt_in_incomplete")
        self.assertFalse(launcher.effects_executed)

    def test_cli_default_is_plan(self) -> None:
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = bcc.main([])
        self.assertEqual(code, 0)
        document = json.loads(buf.getvalue())
        self.assertEqual(document["outcome"], "plan")
        self.assertFalse(document["effects_executed"])
        self.assertFalse(document["live"])

    def test_cli_live_without_plan_refuses(self) -> None:
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = bcc.main(["--live"])
        self.assertEqual(code, 2)
        document = json.loads(buf.getvalue())
        self.assertEqual(document["seam"], "live_opt_in_incomplete")
        self.assertFalse(document["effects_executed"])

    def test_count_plan_is_ten(self) -> None:
        counter = bcc.CommandCounter()
        self.assertEqual(len(counter.plan), 10)
        for mode, name in list(bcc.COUNTED_STEPS):
            counter.before_delivery(mode, name)
        with self.assertRaises(bcc.ComparisonError) as raised:
            counter.before_delivery("on", "extra")
        self.assertEqual(raised.exception.seam, "count")
        self.assertIn("11th", raised.exception.message)

    def test_cli_live_wires_submitter_and_mints_uuid(self) -> None:
        options = bcc.parse_cli(["--live", "--resource-plan", "/x/plan.json"])
        launcher = bcc.build_launcher(options)
        self.assertIsInstance(launcher.submitter, bcc.SubprocessSubmitter)
        self.assertEqual(launcher.caller, bcc.DEFAULT_CALLER)
        parsed = uuid.UUID(launcher.request_id)
        self.assertEqual(str(parsed), launcher.request_id)
        self.assertNotEqual(
            launcher.request_id, "00000000-0000-4000-8000-000000000000"
        )

    def test_cli_admitted_and_plan_roles_have_no_submitter(self) -> None:
        admitted = bcc.build_launcher(
            bcc.parse_cli(["--live", "--admitted-root", "/p", "--resource-plan", "/x"])
        )
        self.assertIsNone(admitted.submitter)
        planned = bcc.build_launcher(bcc.parse_cli(["--resource-plan", "/x"]))
        self.assertIsNone(planned.submitter)

    def test_cli_request_id_can_be_replayed(self) -> None:
        launcher = bcc.build_launcher(
            bcc.parse_cli(["--live", "--request-id", REQUEST_ID, "--resource-plan", "/x"])
        )
        self.assertEqual(launcher.request_id, REQUEST_ID)


class RegistryPlanTests(unittest.TestCase):
    def test_lock_parser_and_shards(self) -> None:
        text = (
            '[[package]]\nname = "cc"\nversion = "1.4.7"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "abc"\n\n'
            '[[package]]\nname = "serde"\nversion = "1.0.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "def"\n'
        )
        packages = bcc.parse_lock_packages(text)
        self.assertEqual([p.name for p in packages], ["cc", "serde"])
        self.assertEqual(bcc.sparse_index_shard("cc"), "2")
        self.assertEqual(bcc.sparse_index_shard("a"), "1")
        self.assertEqual(bcc.sparse_index_shard("abc"), "3/a")
        self.assertEqual(bcc.sparse_index_shard("serde"), "se/rd")
        plan = bcc.derive_registry_input_plan(packages, lock_sha256=bcc.CARGO_LOCK_SHA256)
        rels = [item.rel for item in plan.inputs()]
        self.assertIn("registry/cache/index.crates.io-1949cf8c6b5b557f/cc-1.4.7.crate", rels)
        self.assertIn(
            "registry/index/index.crates.io-1949cf8c6b5b557f/.cache/2/cc", rels
        )

    def test_evidence_derivation_is_canonical_and_deterministic(self) -> None:
        first = bcc.derive_registry_input_plan_from_evidence()
        second = bcc.derive_registry_input_plan_from_evidence()
        self.assertEqual(first.digest(), second.digest())
        self.assertEqual(first.lock_sha256, bcc.CARGO_LOCK_SHA256)
        for item in first.inputs():
            self.assertTrue(
                bcc._is_relative_to(item.src, bcc.CARGO_CACHE_ROOT)
                or bcc._is_relative_to(item.src, bcc.CARGO_INDEX_ROOT)
            )

    def test_non_crates_io_source_refuses(self) -> None:
        package = bcc.LockedPackage(name="x", version="1", source="git+https://example", checksum="a")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.derive_registry_input_plan([package], lock_sha256=bcc.CARGO_LOCK_SHA256)
        self.assertEqual(raised.exception.seam, "registry_plan")

    def test_registry_input_outside_roots_refuses(self) -> None:
        plan = _plan(SCRATCH / "outside")
        plan = bcc.ResourcePlan(
            launch_id=plan.launch_id,
            parent=plan.parent,
            mount=plan.mount,
            backing=plan.backing,
            nucleus_source=plan.nucleus_source,
            registry_inputs=(
                bcc.RegistryInput(src=Path("/tmp/elsewhere/foo.crate"), rel="registry/cache/x"),
            ),
        )
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_resource_plan(plan, strict_names=False)
        self.assertEqual(raised.exception.seam, "source_cache_unbound")


class StagingTests(unittest.TestCase):
    def test_stage_and_validate_exact_bootstrap(self) -> None:
        plan = _plan(SCRATCH / "stage")
        registry = bcc.derive_registry_input_plan_from_evidence()
        clock = FakeClock()
        staged = _stage(plan, registry, clock)
        self.assertEqual(sorted(p.name for p in plan.parent.iterdir()), sorted(bcc.STAGED_ENTRIES))
        validated = bcc.validate_staged_session(
            plan,
            expected=None,
            launcher_path=Path(bcc.__file__).resolve(),
            registry_plan=registry,
        )
        self.assertEqual(validated.manifest_digest, staged.manifest_digest)
        self.assertEqual(validated.plan_digest, staged.plan_digest)

    def test_preexisting_root_is_refused(self) -> None:
        plan = _plan(SCRATCH / "preexisting")
        plan.parent.mkdir(parents=True)
        launcher, _, _, _, _ = _launcher(SCRATCH)
        document = launcher.plan(plan)
        self.assertFalse(document["live_ready"])
        self.assertTrue(any("path_exists" in note for note in document["notes"]))

    def test_unexpected_entry_refuses(self) -> None:
        plan = _plan(SCRATCH / "extra")
        registry = bcc.derive_registry_input_plan_from_evidence()
        _stage(plan, registry, FakeClock())
        (plan.parent / "unexpected.txt").write_text("nope", encoding="utf-8")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_staged_session(
                plan,
                expected=None,
                launcher_path=Path(bcc.__file__).resolve(),
                registry_plan=registry,
            )
        self.assertEqual(raised.exception.seam, "staged_session")

    def test_tampered_manifest_refuses(self) -> None:
        plan = _plan(SCRATCH / "tamper")
        registry = bcc.derive_registry_input_plan_from_evidence()
        _stage(plan, registry, FakeClock())
        (plan.parent / bcc.STAGED_MANIFEST_NAME).write_text("garbage", encoding="utf-8")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_staged_session(
                plan,
                expected=None,
                launcher_path=Path(bcc.__file__).resolve(),
                registry_plan=registry,
            )
        self.assertEqual(raised.exception.seam, "staged_session")

    def test_staged_plan_is_loadable_resource_plan(self) -> None:
        plan = _plan(SCRATCH / "loadable")
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), FakeClock())
        loaded = bcc.load_resource_plan(plan.parent / bcc.STAGED_PLAN_NAME)
        self.assertEqual(loaded.launch_id, plan.launch_id)
        self.assertEqual(loaded.parent, plan.parent)
        self.assertEqual(loaded.mount, plan.mount)
        self.assertEqual(loaded.backing, plan.backing)
        self.assertEqual(loaded.nucleus_source, plan.nucleus_source)
        self.assertTrue(loaded.registry_inputs)
        self.assertTrue(
            all(item.src.is_absolute() for item in loaded.registry_inputs)
        )

    def test_path_escape_rel_refuses(self) -> None:
        plan = _plan(SCRATCH / "escape")
        plan = bcc.ResourcePlan(
            launch_id=plan.launch_id,
            parent=plan.parent,
            mount=plan.mount,
            backing=plan.backing,
            nucleus_source=plan.nucleus_source,
            registry_inputs=(
                bcc.RegistryInput(
                    src=bcc.CARGO_CACHE_ROOT / "foo.crate",
                    rel="../../escape/foo.crate",
                ),
            ),
        )
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_resource_plan(plan, strict_names=False)
        self.assertEqual(raised.exception.seam, "source_cache_unbound")


class AdmissionTests(unittest.TestCase):
    def test_admitted_run_requires_supplied_root(self) -> None:
        plan = _plan(SCRATCH / "noadmit")
        registry = bcc.derive_registry_input_plan_from_evidence()
        _stage(plan, registry, FakeClock())
        launcher, _, _, _, _ = _launcher(SCRATCH, plan=plan, admitted_root=plan.parent / "other")
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["seam"], "admission_unverified")
        self.assertFalse(document["effects_executed"])

    def test_admitted_root_mismatch_refuses(self) -> None:
        plan = _plan(SCRATCH / "wrongroot")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.verify_admission(
                plan,
                admitted_root=plan.parent.parent / "other",
                parent_env={"HOST_RUN_ID": "r", "HOST_RUN_TOKEN": "t"},
            )
        self.assertEqual(raised.exception.seam, "admission_unverified")

    def test_absent_token_refuses(self) -> None:
        plan = _plan(SCRATCH / "notoken")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.verify_admission(plan, admitted_root=plan.parent, parent_env={})
        self.assertEqual(raised.exception.seam, "admission_unverified")

    def test_admitted_run_under_staged_root_succeeds(self) -> None:
        plan = _plan(SCRATCH / "admitted")
        registry = bcc.derive_registry_input_plan_from_evidence()
        _stage(plan, registry, FakeClock())
        launcher, _, _, _, spawner = _launcher(SCRATCH, plan=plan, admitted_root=plan.parent)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "success", document.get("seam"))
        self.assertEqual(document["count_delivered"], 10)
        self.assertEqual(document["launch_packet"]["admission"]["root"], str(plan.parent))
        self.assertTrue(document["log"].find("<redacted>") >= 0)
        self.assertNotIn(TOKEN, document["log"])
        for call in spawner.calls:
            if call and call[0] == "effigy":
                self.assertEqual(call[1], bcc.SELECTOR)
        source = plan.mount / "src-on"
        self.assertTrue(str(source).startswith(str(plan.parent)))


class SubmissionTests(unittest.TestCase):
    def test_binding_persisted_before_delivery(self) -> None:
        plan = _plan(SCRATCH / "bind")
        submitter = RecordingSubmitter(["launched"])
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "launched")
        self.assertTrue(submitter.calls[0]["binding_present"])
        self.assertEqual(submitter.calls[0]["request_id"], REQUEST_ID)
        self.assertIn("--host-run-request-id", submitter.calls[0]["argv"])
        self.assertEqual(submitter.calls[0]["cwd"], str(plan.parent))
        self.assertEqual(document["launch_packet"]["binding"]["request_id"], REQUEST_ID)
        self.assertFalse(document["launch_packet"]["never_launched"])

    def test_authenticated_absence_allows_same_request_replay(self) -> None:
        plan = _plan(SCRATCH / "replay")
        submitter = RecordingSubmitter(["authenticated_absence", "launched"])
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "launched")
        self.assertEqual(len(submitter.calls), 2)
        self.assertEqual(submitter.calls[0]["request_id"], submitter.calls[1]["request_id"])
        self.assertIn("authenticated_absence=unknown_run", document["log"])

    def test_capacity_timeout_is_never_launched_partial(self) -> None:
        plan = _plan(SCRATCH / "capacity")
        submitter = RecordingSubmitter(["capacity_timeout"])
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "never_launched_capacity_timeout")
        self.assertTrue(document["launch_packet"]["never_launched"])
        self.assertTrue(document["resources_retained"])

    def test_held_reply_is_unknown_not_success(self) -> None:
        plan = _plan(SCRATCH / "held")
        submitter = RecordingSubmitter(["held"])
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "unknown")
        self.assertEqual(document["seam"], "submit_ambiguous")
        self.assertTrue(document["resources_retained"])

    def test_held_then_authenticated_absence_replays_original_request(self) -> None:
        plan = _plan(SCRATCH / "lostreply")
        submitter = LostThenAbsentSubmitter()
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "launched")
        self.assertEqual(len(submitter.submits), 2)
        self.assertEqual(submitter.statuses, 1)
        self.assertEqual(submitter.submits[0], submitter.submits[1])
        self.assertIn("--host-run-request-id", submitter.submits[1])
        self.assertIn(submitter.submits[1][2], submitter.submits[0])
        self.assertIn("replaying the same request", document["log"])

    def test_submitter_timeout_is_held_not_raised(self) -> None:
        submitter = bcc.SubprocessSubmitter()
        outcome = submitter.submit(
            argv=[sys.executable, "-c", "import time; time.sleep(5)"],
            cwd=str(SCRATCH),
            request_id=REQUEST_ID,
            caller="northstar-worker",
            env={"PATH": os.environ.get("PATH", "/usr/bin")},
            timeout_ns=50_000_000,
        )
        self.assertEqual(outcome.state, "held")
        self.assertIn("timeout", outcome.detail or "")

    def test_held_reply_follows_found_request(self) -> None:
        plan = _plan(SCRATCH / "follow")
        submitter = RecoveringSubmitter(follow_state="launched")
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "launched")
        self.assertEqual(submitter.statuses, 1)
        self.assertEqual(submitter.follows, 1)
        self.assertIn("request found; following the settled host run", document["log"])

    def test_held_reply_follow_failure_stays_unknown(self) -> None:
        plan = _plan(SCRATCH / "followfail")
        submitter = RecoveringSubmitter(follow_state="held")
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "unknown")
        self.assertEqual(document["seam"], "submit_ambiguous")
        self.assertTrue(document["resources_retained"])


class DeadlineTests(unittest.TestCase):
    def test_utc_jump_does_not_extend_deadline(self) -> None:
        clock = FakeClock()
        session = bcc.SessionClock(
            start_monotonic_ns=clock.monotonic_ns(),
            start_utc=clock.utc_iso(),
            deadline_monotonic_ns=clock.monotonic_ns() + 1_000_000_000,
            reserve_ns=0,
        )
        clock.utc = "2026-10-15T02:00:00Z"
        clock.advance(100)
        self.assertFalse(session.expired_hard(clock.monotonic_ns()))
        clock.advance(1_000_000_000)
        self.assertTrue(session.expired_hard(clock.monotonic_ns()))
        self.assertEqual(session.deadline_monotonic_ns, 2_000_000_000)

    def test_binding_deadline_is_adopted_not_reset(self) -> None:
        plan = _plan(SCRATCH / "adopt")
        registry = bcc.derive_registry_input_plan_from_evidence()
        clock = FakeClock()
        _stage(plan, registry, clock)
        clock.advance(300_000_000_000)  # 5 minutes of capacity wait
        launcher, _, _, _, _ = _launcher(SCRATCH, plan=plan, clock=clock, admitted_root=plan.parent)
        document = launcher.run(live=True, resource_plan=plan)
        expected_deadline = 1_000_000_000 + bcc.BUDGET_NS
        self.assertEqual(document["session"]["deadline_monotonic_ns"], expected_deadline)
        self.assertIn("session_adopted", document["log"])

    def test_second_operation_after_expiry_refused(self) -> None:
        clock = FakeClock()
        launcher, _, _, _, spawner = _launcher(SCRATCH, clock=clock, budget_ns=10, reserve_ns=0)
        launcher._open_session()
        clock.advance(50)
        with self.assertRaises(bcc.ComparisonError) as raised:
            launcher._deliver("probe", "cargo-vV", ["cargo", "-vV"], {}, str(SCRATCH))
        self.assertEqual(raised.exception.outcome, "deadline")
        self.assertEqual(spawner.calls, [])

    def test_slow_child_terminated_at_original_deadline(self) -> None:
        launcher = bcc.ComparisonLauncher(
            clock=bcc.RealClock(),
            spawner=bcc.RealSpawner(),
            budget_ns=400_000_000,
            reserve_ns=50_000_000,
            parent_env={"PATH": os.environ.get("PATH", "/usr/bin")},
        )
        started = time.monotonic_ns()
        result = launcher.run_timed_helper(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            env={"PATH": os.environ.get("PATH", "/usr/bin")},
            cwd=str(SCRATCH),
        )
        elapsed = time.monotonic_ns() - started
        self.assertEqual(result.outcome, "deadline")
        self.assertTrue(result.spawned)
        self.assertTrue(result.child_terminated)
        self.assertTrue(result.writer_closed)
        self.assertLess(elapsed, 2_000_000_000)
        if result.pgid is not None:
            try:
                os.kill(result.pgid, 0)
                alive = True
            except ProcessLookupError:
                alive = False
            self.assertFalse(alive)

    def test_unknown_closure_retains_resources(self) -> None:
        plan = _plan(SCRATCH / "unknown")
        clock = FakeClock()
        spawner = FakeSpawner(clock)
        spawner.unknown_closure = True
        image = FakeImage(clock)
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, spawner=spawner, image=image, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "unknown")
        self.assertTrue(document["resources_retained"])
        self.assertFalse(image.detached)
        self.assertTrue(document["preparation_after_clock"])


class BoundedIOTests(unittest.TestCase):
    def _real_launcher(self, budget_ns: int = 5_000_000_000):
        return bcc.ComparisonLauncher(
            clock=bcc.RealClock(),
            spawner=bcc.RealSpawner(),
            budget_ns=budget_ns,
            reserve_ns=100_000_000,
            parent_env={"PATH": os.environ.get("PATH", "/usr/bin")},
        )

    def test_descendant_holding_pipe_is_not_infinite(self) -> None:
        launcher = self._real_launcher()
        started = time.monotonic_ns()
        result = launcher.run_timed_helper(
            [
                "/bin/sh",
                "-c",
                "sleep 20 & echo emitted; exit 0",
            ],
            env={"PATH": os.environ.get("PATH", "/usr/bin")},
            cwd=str(SCRATCH),
        )
        elapsed = time.monotonic_ns() - started
        try:
            self.assertEqual(result.outcome, "ok")
            self.assertIn("emitted", result.stdout)
            self.assertFalse(result.writer_closed)
            self.assertFalse(result.child_terminated)
            self.assertLess(elapsed, 3_000_000_000)
            self.assertGreaterEqual(elapsed, bcc.PIPE_DRAIN_GRACE_NS - 200_000_000)
        finally:
            if result.pgid is not None:
                try:
                    os.killpg(result.pgid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_large_eof_output_is_bounded(self) -> None:
        launcher = self._real_launcher()
        size = bcc.PIPE_CAPTURE_CAP + 1024 * 1024
        result = launcher.run_timed_helper(
            [sys.executable, "-c", f"import sys; sys.stdout.write('x' * {size})"],
            env={"PATH": os.environ.get("PATH", "/usr/bin")},
            cwd=str(SCRATCH),
        )
        self.assertEqual(result.outcome, "ok")
        self.assertTrue(result.writer_closed)
        self.assertLessEqual(len(result.stdout.encode("utf-8")), bcc.PIPE_CAPTURE_CAP)


class ContainmentTests(unittest.TestCase):
    def test_backing_under_mount_refuses(self) -> None:
        parent = Path("/private/var/tmp/oxide-build-context-cmp-x")
        plan = bcc.ResourcePlan(
            launch_id="x",
            parent=parent,
            mount=parent / "m",
            backing=parent / "m" / "cmp.sparseimage",
            nucleus_source=Path("/Users/tom/Dev/projects/nucleus"),
            registry_inputs=(bcc.RegistryInput(src=bcc.CARGO_CACHE_ROOT / "c", rel="a"),),
        )
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_resource_plan(plan)
        self.assertIn(raised.exception.seam, {"backing_under_mount", "containment_names"})

    def test_symlink_path_refuses(self) -> None:
        root = SCRATCH / "sym"
        root.mkdir()
        real = root / "real"
        real.mkdir()
        link = root / "link"
        os.symlink(real, link)
        nested = link / "cmp-parent"
        self.assertTrue(bcc.ancestor_symlink(nested))

    def test_imageinfo_overhead_sum_included(self) -> None:
        plan = _plan(SCRATCH / "over")
        clock = FakeClock()
        image = FakeImage(clock, IMAGEINFO_OVER)
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, image=image, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "footprint")

    def test_host_space_unknown_stops(self) -> None:
        plan = _plan(SCRATCH / "space")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, space_probe=lambda: None, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "host_reserve")

    def test_missing_named_input_refuses(self) -> None:
        plan = _plan(SCRATCH / "missing")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)

        def failing_copy(src, dest):
            raise FileNotFoundError(str(src))

        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, copy=failing_copy, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "source_cache_unbound")

    def test_source_cache_copy_stays_inside_mount(self) -> None:
        plan = _plan(SCRATCH / "cache")
        clock = FakeClock()
        image = FakeImage(clock)
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, image=image, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        copied = plan.mount / "cargo-home" / "registry" / "cache" / "index.crates.io-1949cf8c6b5b557f" / "foo-0.1.0.crate"
        self.assertTrue(copied.is_file())
        self.assertEqual(copied.read_bytes(), b"crate")
        self.assertTrue(document["preparation_after_clock"])


class IdentityTests(unittest.TestCase):
    def _admitted(self, name: str, **overrides):
        plan = _plan(SCRATCH / name)
        clock = overrides.pop("clock", FakeClock())
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        launcher, _, _, _, spawner = _launcher(
            SCRATCH, plan=plan, clock=clock, admitted_root=plan.parent, **overrides
        )
        return launcher, plan, spawner

    def test_tool_mismatch_refuses_before_tests(self) -> None:
        clock = FakeClock()
        launcher, plan, spawner = self._admitted("tool", clock=clock, spawner=FakeSpawner(clock, rustc_ok=False))
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "tool_identity")
        self.assertFalse(any(call and call[0] == "effigy" for call in spawner.calls))

    def test_mixed_cargo_rustc_refuses(self) -> None:
        clock = FakeClock()
        launcher, plan, spawner = self._admitted("mixed", clock=clock, spawner=FakeSpawner(clock, cargo_ok=False))
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["seam"], "tool_identity")
        self.assertEqual(spawner.calls, [("cargo", "-vV")])

    def test_source_pin_mismatch_refuses(self) -> None:
        launcher, plan, spawner = self._admitted("src", git=_git_bad_tree)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["seam"], "source_identity")
        self.assertFalse(any(call and call[0] == "effigy" for call in spawner.calls))

    def test_forbidden_config_refuses(self) -> None:
        plan = _plan(SCRATCH / "cfg")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        config_dir = SCRATCH / "cfg-config"
        config_dir.mkdir()
        config = config_dir / "config.toml"
        config.write_text("[build]\ntarget-dir = '/tmp/nope'\n", encoding="utf-8")

        def paths(*args):
            return [config]

        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, config_paths=paths, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["seam"], "config_identity")


class MeasurementTests(unittest.TestCase):
    def _admitted(self, name: str, **overrides):
        return _admitted_launcher(SCRATCH, name, **overrides)

    def test_measurements_use_debug_dirs_and_union(self) -> None:
        launcher, plan, _ = self._admitted("meas")
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "success", document.get("seam"))
        self.assertEqual(len(document["measurements"]), 8)
        rec = document["measurements"][0]
        self.assertEqual((rec["mode"], rec["step"]), ("off", "C0"))
        self.assertEqual(rec["problems"], [])
        self.assertIn(
            str(plan.mount / "target-off" / "debug" / "deps"), rec["per_dir"]
        )
        self.assertIn(
            str(plan.mount / "target-off" / "debug" / "incremental"), rec["per_dir"]
        )
        self.assertGreater(rec["union_file_count"], 0)
        self.assertIn("cannot confound", rec["apfs_clone_sharing"])

    def test_zero_test_result_is_partial(self) -> None:
        launcher, plan, _ = self._admitted(
            "zero",
            spawner=FakeSpawner(
                FakeClock(), test_line="test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out", test_ran=False
            ),
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "measurement")

    def test_wrong_incremental_flag_is_partial(self) -> None:
        launcher, plan, _ = self._admitted(
            "wronginc", spawner=FakeSpawner(FakeClock(), force_incremental="off")
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "measurement")

    def test_missing_debuginfo_is_partial(self) -> None:
        launcher, plan, _ = self._admitted(
            "nodebug", spawner=FakeSpawner(FakeClock(), emit_debuginfo=False)
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "measurement")

    def test_missing_rustc_invocations_is_partial(self) -> None:
        launcher, plan, _ = self._admitted(
            "norustc", spawner=FakeSpawner(FakeClock(), emit_rustc=False)
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "measurement")

    def test_rustc_wrapper_prefix_refuses(self) -> None:
        launcher, plan, _ = self._admitted(
            "wrap", spawner=FakeSpawner(FakeClock(), rustc_path="/usr/bin/sccache")
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "rustc_wrapper")

    def test_hardlink_union_accounting(self) -> None:
        root = SCRATCH / "acct"
        deps = root / "deps"
        incremental = root / "incremental"
        deps.mkdir(parents=True)
        incremental.mkdir()
        one = deps / "a.o"
        one.write_bytes(b"hello-world-bytes")
        two = incremental / "a.o"
        os.link(one, two)
        acc = bcc.union_account(
            [deps, incremental],
            owned_roots=[root],
            clock=FakeClock(),
            remaining_ns=1_000_000_000,
        )
        self.assertEqual(acc.file_count, 2)
        self.assertEqual(acc.hardlink_dedup_count, 1)
        self.assertEqual(acc.logical_bytes, 2 * len(b"hello-world-bytes"))
        self.assertEqual(acc.allocated_bytes, bcc.allocated_bytes(os.lstat(one)))
        self.assertIn("cannot confound", acc.apfs_clone_sharing)
        self.assertEqual(acc.per_dir[str(deps)].file_count, 1)

    def test_host_cargo_home_is_not_scanned(self) -> None:
        outside = SCRATCH / "fake-cargo-home"
        outside.mkdir()
        (outside / "x").write_text("n", encoding="utf-8")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.union_account(
                [outside],
                owned_roots=[SCRATCH / "owned"],
                clock=FakeClock(),
                remaining_ns=1_000_000_000,
            )
        self.assertEqual(raised.exception.seam, "accounting_root_outside_owned")


class EvidenceTests(unittest.TestCase):
    def test_output_overflow_keeps_critical_identity(self) -> None:
        log = bcc.EvidenceLog(limit=2048, critical_reserve=256)
        log.write("x" * 4000)
        self.assertTrue(log.overflow)
        log.write(f"selected_test={bcc.TEST_NAME} failed", critical=True)
        text = log.text()
        self.assertIn("partial=true", text)
        self.assertIn(bcc.TEST_NAME, text)

    def test_admission_fields_propagated_and_token_redacted(self) -> None:
        launcher, plan, _ = _admitted_launcher(SCRATCH, "adm")
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "success", document.get("seam"))
        self.assertNotIn(TOKEN, document["log"])
        self.assertIn("<redacted>", document["log"])

    def test_admission_names_match_contract(self) -> None:
        self.assertEqual(
            bcc.ADMISSION_PRESERVE[:2],
            ("HOST_RUN_TOKEN", "HOST_RUN_ID"),
        )
        self.assertIn("EFFIGY_HOST_RUN_ROOT", bcc.ADMISSION_PRESERVE)
        self.assertIn("EFFIGY_HOST_SCHEDULER", bcc.ADMISSION_PRESERVE)
        self.assertIn("EFFIGY_SCHEDULER_OVERRIDE", bcc.ADMISSION_PRESERVE)

    def test_restore_after_negative_check(self) -> None:
        clock = FakeClock()
        git_calls: list[list[str]] = []

        def git(argv, cwd=None):
            git_calls.append(list(argv))
            return _git_ok(argv, cwd)

        plan = _plan(SCRATCH / "neg")
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, git=git, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        source = plan.mount / "src-on"
        self.assertTrue((source / "packages").is_dir())
        self.assertFalse((source / "packages.oxide-moved").exists())
        self.assertTrue(any(call[:1] == ["checkout"] for call in git_calls))
        self.assertTrue(any(call[:2] == ["status", "--porcelain"] for call in git_calls))
        self.assertIn("restore packages=True", document["log"])
        self.assertEqual(document["count_delivered"], 10)
        self.assertEqual(document["outcome"], "success")

    def test_panic_header_without_frame_is_partial(self) -> None:
        plan = _plan(SCRATCH / "hdr")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        spawner = FakeSpawner(clock, negative_stderr=HEADER_ONLY_NEGATIVE_STDERR)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, spawner=spawner, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "backtrace_frame")

    def test_restore_dirty_porcelain_is_partial(self) -> None:
        plan = _plan(SCRATCH / "dirty")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)

        def git(argv, cwd=None):
            if argv[:2] == ["status", "--porcelain"]:
                return " M crates/nucleus-core/src/lib.rs\n"
            return _git_ok(argv, cwd)

        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, git=git, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "restore")


class LocatorTests(unittest.TestCase):
    def test_paren_form_on_stderr_resolves_via_target_dir(self) -> None:
        target = SCRATCH / "tgt-paren"
        deps = target / "debug" / "deps"
        deps.mkdir(parents=True)
        exe = deps / "packages-abc123"
        exe.write_text("x", encoding="utf-8")
        cwd = SCRATCH / "src-paren"
        cwd.mkdir(exist_ok=True)
        text = (
            "     Running `/usr/bin/rustc --crate-name packages`\n"
            "     Running tests/packages.rs (target/debug/deps/packages-abc123)\n"
        )
        reported = bcc.parse_reported_executable(text)
        self.assertEqual(reported, "target/debug/deps/packages-abc123")
        located = bcc.locate_reported_executable(reported, target, cwd=cwd)
        self.assertEqual(located.resolve(), exe.resolve())

    def test_backtick_form_absolute(self) -> None:
        target = SCRATCH / "tgt-tick"
        deps = target / "debug" / "deps"
        deps.mkdir(parents=True)
        exe = deps / "packages-bead"
        exe.write_text("x", encoding="utf-8")
        text = f"     Running `{exe} --exact {bcc.TEST_NAME}`\n"
        reported = bcc.parse_reported_executable(text)
        self.assertEqual(reported, str(exe))
        located = bcc.locate_reported_executable(reported, target)
        self.assertEqual(located, exe)

    def test_skips_rustc_running_line(self) -> None:
        text = "     Running `/usr/bin/rustc --crate-name packages --edition 2021`\n"
        self.assertIsNone(bcc.parse_reported_executable(text))

    def test_panic_header_is_not_a_frame(self) -> None:
        self.assertFalse(bcc.backtrace_has_packages_frame(HEADER_ONLY_NEGATIVE_STDERR))
        self.assertTrue(bcc.backtrace_has_packages_frame(DEFAULT_NEGATIVE_STDERR))

    def test_stale_executable_ambiguity(self) -> None:
        target = SCRATCH / "tgt"
        deps = target / "debug" / "deps"
        deps.mkdir(parents=True)
        first = deps / "packages-old"
        second = deps / "packages-new"
        first.write_text("old", encoding="utf-8")
        second.write_text("new", encoding="utf-8")
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.locate_reported_executable(None, target)
        self.assertEqual(raised.exception.seam, "stale_executable_ambiguity")
        located = bcc.locate_reported_executable(str(second), target)
        self.assertEqual(located, second)

    def test_parses_rustc_flags(self) -> None:
        text = (
            "     Running `/usr/bin/rustc --crate-name nucleus-core --edition 2021 "
            "-C debuginfo=line-tables-only -C split-debuginfo=unpacked "
            "-C incremental=/tmp/inc`\n"
        )
        invocations = bcc.parse_rustc_invocations(text)
        self.assertEqual(len(invocations), 1)
        self.assertEqual(invocations[0].crate_name, "nucleus-core")
        self.assertEqual(invocations[0].incremental, "/tmp/inc")
        self.assertEqual(invocations[0].debuginfo, "line-tables-only")
        self.assertEqual(invocations[0].split_debuginfo, "unpacked")


class SamplingTests(unittest.TestCase):
    def test_cadence_sample_during_helper(self) -> None:
        plan = _plan(SCRATCH / "cadence")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        spawner = FakeSpawner(clock, advance_ns=bcc.SAMPLE_CADENCE_NS)
        image = FakeImage(clock)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, spawner=spawner, image=image, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "success", document.get("seam"))
        self.assertGreaterEqual(document["sampling"]["sample_count"], 2)
        self.assertIn("reason=cadence", document["log"])
        self.assertGreaterEqual(document["sampling"]["peak_image_used"], 4096)
        self.assertIsNotNone(document["sampling"]["min_host_free"])
        self.assertEqual(document["sampling"]["cadence_ns"], bcc.SAMPLE_CADENCE_NS)

    def test_cadence_image_threshold_stops(self) -> None:
        plan = _plan(SCRATCH / "cad-th")
        clock = FakeClock()
        _stage(plan, bcc.derive_registry_input_plan_from_evidence(), clock)
        image = FakeImage(clock)
        n = {"c": 0}
        original = image.imageinfo

        def wrapped(backing, remaining_ns):
            n["c"] += 1
            if n["c"] >= 3:
                image.info = IMAGEINFO_AT_THRESHOLD
            return original(backing, remaining_ns)

        image.imageinfo = wrapped  # type: ignore[method-assign]
        spawner = FakeSpawner(clock, advance_ns=bcc.SAMPLE_CADENCE_NS)
        launcher, _, _, _, _ = _launcher(
            SCRATCH, plan=plan, clock=clock, spawner=spawner, image=image, admitted_root=plan.parent
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "image_threshold")


class ParseTests(unittest.TestCase):
    def test_fake_imageinfo_missing_fields_unknown(self) -> None:
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.parse_imageinfo("not an imageinfo document\n")
        self.assertEqual(raised.exception.outcome, "unknown")

    def test_counted_effigy_argv_has_separators(self) -> None:
        argv = bcc.EFFIGY_TEST_ARGV
        self.assertEqual(argv[0], "effigy")
        self.assertEqual(argv[1], bcc.SELECTOR)
        self.assertEqual(argv[2], "--")
        self.assertIn("--exact", argv)
        self.assertIn(bcc.TEST_NAME, argv)


class ManifestCommandTests(unittest.TestCase):
    def test_staged_manifest_is_one_command_not_an_argv_sequence(self) -> None:
        plan = _plan(SCRATCH / "manifest-live")
        launcher_path = Path(bcc.__file__).resolve()
        plan_path = plan.parent / bcc.STAGED_PLAN_NAME
        text = bcc.staged_manifest_text(launcher_path, plan_path, plan.parent)
        # A TOML array is a sequence of separate shell commands in Effigy, not
        # one argv vector. The staged entry must be a single command string.
        self.assertNotIn("run = [", text)
        run_lines = [line for line in text.splitlines() if line.startswith("run = ")]
        self.assertEqual(len(run_lines), 1)
        self.assertTrue(run_lines[0].startswith('run = "'))
        command = json.loads(run_lines[0][len("run = "):])
        parts = shlex.split(command)
        self.assertEqual(
            parts,
            [
                sys.executable,
                launcher_path.as_posix(),
                "--live",
                "--admitted-root",
                plan.parent.as_posix(),
                "--resource-plan",
                plan_path.as_posix(),
            ],
        )

    def test_smoke_manifest_adds_only_the_smoke_binding_flag(self) -> None:
        plan = _plan(SCRATCH / "manifest-smoke")
        launcher_path = Path(bcc.__file__).resolve()
        plan_path = plan.parent / bcc.STAGED_PLAN_NAME
        text = bcc.staged_manifest_text(launcher_path, plan_path, plan.parent, smoke=True)
        run_lines = [line for line in text.splitlines() if line.startswith("run = ")]
        parts = shlex.split(json.loads(run_lines[0][len("run = "):]))
        self.assertEqual(parts[-1], "--smoke")
        self.assertEqual(parts.count("--smoke"), 1)

    def test_shell_execution_preserves_spaces_and_metacharacters(self) -> None:
        root = SCRATCH / "shell-exec"
        root.mkdir(parents=True, exist_ok=True)
        output = root / "argv'out.json"
        stub = root / "stub script.py"
        stub.write_text(
            "import json,sys\nopen(sys.argv[1],'w').write(json.dumps(sys.argv[2:]))\n",
            encoding="utf-8",
        )
        argv = [
            sys.executable,
            str(stub),
            str(output),
            "a b",
            "c'd",
            "e;f",
            "$HOME",
            "*",
            "sp ace",
            "line\nbreak",
        ]
        command = bcc.shell_command_for_argv(argv)
        completed = subprocess.run(
            ["/bin/sh", "-c", command], capture_output=True, text=True, cwd=str(root)
        )
        self.assertEqual(completed.returncode, 0, completed.stderr[-400:])
        recorded = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(recorded, argv[3:])


class StagedRuntimeStateTests(unittest.TestCase):
    def _staged(self, name: str) -> tuple[bcc.ResourcePlan, bcc.RegistryInputPlan, bcc.StagedSession]:
        plan = _plan(SCRATCH / name)
        registry = bcc.derive_registry_input_plan_from_evidence()
        staged = _stage(plan, registry, FakeClock())
        return plan, registry, staged

    def _validate(self, plan: bcc.ResourcePlan, registry: bcc.RegistryInputPlan) -> bcc.StagedSession:
        return bcc.validate_staged_session(
            plan,
            expected=None,
            launcher_path=Path(bcc.__file__).resolve(),
            registry_plan=registry,
        )

    def test_installed_effigy_runtime_state_is_accepted_narrowly(self) -> None:
        plan, registry, staged = self._staged("runtime-ok")
        runtime = plan.parent / bcc.STAGED_RUNTIME_DIR
        (runtime / "locks").mkdir(parents=True)
        (runtime / "runtime" / "tasks" / "active").mkdir(parents=True)
        (runtime / "reports" / "tasks").mkdir(parents=True)
        validated = self._validate(plan, registry)
        self.assertEqual(validated.manifest_digest, staged.manifest_digest)
        self.assertEqual(validated.binding_digest, staged.binding_digest)

    def test_runtime_state_unknown_entry_refuses(self) -> None:
        plan, registry, _ = self._staged("runtime-evil")
        runtime = plan.parent / bcc.STAGED_RUNTIME_DIR
        runtime.mkdir()
        (runtime / "gateway").mkdir()
        with self.assertRaises(bcc.ComparisonError) as raised:
            self._validate(plan, registry)
        self.assertEqual(raised.exception.seam, "staged_session")

    def test_symlinked_runtime_state_refuses(self) -> None:
        plan, registry, _ = self._staged("runtime-link")
        target = SCRATCH / "runtime-link-target"
        target.mkdir()
        os.symlink(target, plan.parent / bcc.STAGED_RUNTIME_DIR)
        with self.assertRaises(bcc.ComparisonError) as raised:
            self._validate(plan, registry)
        self.assertEqual(raised.exception.seam, "staged_session")

    def test_runtime_state_does_not_weaken_bootstrap_digests(self) -> None:
        plan, registry, _ = self._staged("runtime-tamper")
        (plan.parent / bcc.STAGED_RUNTIME_DIR).mkdir()
        (plan.parent / bcc.STAGED_MANIFEST_NAME).write_text("garbage", encoding="utf-8")
        with self.assertRaises(bcc.ComparisonError) as raised:
            self._validate(plan, registry)
        self.assertEqual(raised.exception.seam, "staged_session")

    def test_extra_root_file_still_refuses(self) -> None:
        plan, registry, _ = self._staged("runtime-extra")
        (plan.parent / "unexpected.txt").write_text("nope", encoding="utf-8")
        with self.assertRaises(bcc.ComparisonError) as raised:
            self._validate(plan, registry)
        self.assertEqual(raised.exception.seam, "staged_session")


_CLOCK_CHILD_CODE = (
    "import importlib.util, sys\n"
    "spec = importlib.util.spec_from_file_location('bcc', sys.argv[1])\n"
    "m = importlib.util.module_from_spec(spec)\n"
    "sys.modules['bcc'] = m\n"
    "spec.loader.exec_module(m)\n"
    "print(m.RealClock().monotonic_ns())\n"
)

_NATIVE_CHILD_CODE = (
    "import importlib.util, json, sys\n"
    "spec = importlib.util.spec_from_file_location('bcc', sys.argv[1])\n"
    "m = importlib.util.module_from_spec(spec)\n"
    "sys.modules['bcc'] = m\n"
    "spec.loader.exec_module(m)\n"
    "plan = m.load_resource_plan(m.Path(sys.argv[2]))\n"
    "launcher = m.ComparisonLauncher(\n"
    "    admitted_root=plan.parent,\n"
    "    smoke=('--smoke' in sys.argv),\n"
    "    strict_names=False,\n"
    "    registry_plan=m.derive_registry_input_plan_from_evidence(),\n"
    ")\n"
    "document = launcher.run(live=True, resource_plan=plan)\n"
    "print(json.dumps(document))\n"
    "sys.exit(0 if document.get('outcome') in {'success', 'smoke_reached'} else 2)\n"
)


def _stage_real(
    plan: bcc.ResourcePlan,
    registry: bcc.RegistryInputPlan,
    clock: bcc.RealClock,
    *,
    deadline: int,
    budget: int,
    smoke: bool,
) -> bcc.StagedSession:
    launcher_path = Path(bcc.__file__).resolve()
    binding = bcc.LaunchBinding(
        caller="northstar-worker",
        request_id=REQUEST_ID,
        request_body_digest="0" * 64,
        parent=plan.parent,
        launcher_sha256=hashlib.sha256(launcher_path.read_bytes()).hexdigest(),
        nucleus_commit=bcc.NUCLEUS_COMMIT,
        nucleus_tree=bcc.NUCLEUS_TREE,
        cargo_lock_sha256=bcc.CARGO_LOCK_SHA256,
        registry_plan_digest=registry.digest(),
        audit_start_utc=clock.utc_iso(),
        deadline_monotonic_ns=deadline,
        declared_budget_ns=budget,
        mode="smoke" if smoke else "live",
        clock_basis=clock.clock_basis(),
    )
    return bcc.stage_session(
        plan,
        launcher_path=launcher_path,
        binding=binding.to_document(),
        registry_plan=registry,
        smoke=smoke,
    )


def _run_native_admitted_child(
    plan: bcc.ResourcePlan, *, smoke: bool, timeout: float = 30.0
) -> tuple[subprocess.CompletedProcess, dict]:
    argv = [
        sys.executable,
        "-c",
        _NATIVE_CHILD_CODE,
        str(Path(bcc.__file__).resolve()),
        str(plan.parent / bcc.STAGED_PLAN_NAME),
    ]
    if smoke:
        argv.append("--smoke")
    env = dict(os.environ)
    env.update({"HOST_RUN_ID": "native_test_run", "HOST_RUN_TOKEN": "native-test-token"})
    completed = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=timeout)
    document: dict = {}
    for line in completed.stdout.splitlines():
        stripped = line.strip()
        if stripped.startswith("{"):
            document = json.loads(stripped)
    return completed, document


class NativeClockTests(unittest.TestCase):
    def test_real_clock_basis_is_shared_by_a_spawned_child(self) -> None:
        clock = bcc.RealClock()
        before = clock.monotonic_ns()
        completed = subprocess.run(
            [sys.executable, "-c", _CLOCK_CHILD_CODE, str(Path(bcc.__file__).resolve())],
            capture_output=True,
            text=True,
            check=True,
        )
        after = clock.monotonic_ns()
        child = int(completed.stdout.strip())
        # A process-local clock (this host's `time.monotonic_ns()`) would be a
        # few milliseconds here and could not fall inside the parent interval.
        self.assertGreaterEqual(child, before)
        self.assertLessEqual(child, after)

    def test_delayed_admitted_child_adopts_parent_deadline_without_reset(self) -> None:
        plan = _plan(SCRATCH / "native-delay")
        registry = bcc.derive_registry_input_plan_from_evidence()
        clock = bcc.RealClock()
        budget = 600_000_000_000
        start = clock.monotonic_ns()
        deadline = start + budget
        _stage_real(plan, registry, clock, deadline=deadline, budget=budget, smoke=True)
        time.sleep(0.25)
        parent_remaining = deadline - clock.monotonic_ns()
        completed, document = _run_native_admitted_child(plan, smoke=True)
        self.assertEqual(document.get("outcome"), "smoke_reached", completed.stderr[-500:])
        self.assertEqual(document["session"]["deadline_monotonic_ns"], deadline)
        self.assertEqual(document["session"]["start_monotonic_ns"], start)
        child_remaining = document["session"]["remaining_hard_ns"]
        self.assertGreater(child_remaining, 0)
        self.assertLessEqual(child_remaining, parent_remaining)

    def test_expired_admitted_child_refuses_before_effects(self) -> None:
        plan = _plan(SCRATCH / "native-expired")
        registry = bcc.derive_registry_input_plan_from_evidence()
        clock = bcc.RealClock()
        deadline = clock.monotonic_ns() - 1
        _stage_real(plan, registry, clock, deadline=deadline, budget=bcc.BUDGET_NS, smoke=True)
        completed, document = _run_native_admitted_child(plan, smoke=True)
        self.assertEqual(document.get("outcome"), "deadline", completed.stderr[-500:])
        self.assertEqual(document.get("seam"), "deadline")
        self.assertEqual(completed.returncode, 2)
        self.assertFalse(plan.mount.exists())
        self.assertFalse(plan.backing.exists())

    def test_binding_with_unsupported_clock_basis_refuses(self) -> None:
        base = {
            "schema": bcc.BINDING_SCHEMA,
            "caller": "northstar-worker",
            "request_id": REQUEST_ID,
            "request_body_digest": "0" * 64,
            "parent": "/x",
            "launcher_sha256": "0" * 64,
            "nucleus_commit": bcc.NUCLEUS_COMMIT,
            "nucleus_tree": bcc.NUCLEUS_TREE,
            "cargo_lock_sha256": bcc.CARGO_LOCK_SHA256,
            "registry_plan_digest": "0" * 64,
            "audit_start_utc": "2026-10-15T00:00:00Z",
            "deadline_monotonic_ns": 1,
            "declared_budget_ns": 1,
            "mode": "live",
        }
        with self.assertRaises(bcc.ComparisonError) as missing:
            bcc.LaunchBinding.from_document(base)
        self.assertEqual(missing.exception.seam, "launch_binding")
        # A recorded but different basis is refused when the binding is adopted
        # by a clock that does not match it (no stale compatibility claim).
        stale = bcc.LaunchBinding.from_document(dict(base, clock_basis="time.monotonic_ns"))
        launcher = bcc.ComparisonLauncher(clock=bcc.RealClock())
        with self.assertRaises(bcc.ComparisonError) as adopted:
            launcher._open_session_from_binding(stale)
        self.assertEqual(adopted.exception.seam, "clock_basis")


class SmokingSubmitter:
    def __init__(self, output: str, state: str = "launched") -> None:
        self.output = output
        self.state = state
        self.submits = 0
        self.follows = 0

    def submit(self, *, argv, cwd, request_id, caller, env, timeout_ns=None):
        self.submits += 1
        return bcc.HeavySubmitOutcome(state=self.state, output=self.output)

    def request_follow(self, *, caller, request_id, env, timeout_ns=None):
        self.follows += 1
        return bcc.HeavySubmitOutcome(state="launched", output=self.output)


class SmokeModeTests(unittest.TestCase):
    def test_smoke_outer_confirms_sentinel_in_settlement(self) -> None:
        plan = _plan(SCRATCH / "smoke-ok")
        submitter = SmokingSubmitter(f"relay prefix {bcc.SMOKE_SENTINEL} relay suffix")
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter, smoke=True)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "smoke_reached")
        self.assertEqual(document["smoke_sentinel"], bcc.SMOKE_SENTINEL)
        self.assertEqual(document["count_delivered"], 0)
        self.assertEqual(submitter.submits, 1)

    def test_smoke_outer_without_sentinel_is_partial(self) -> None:
        plan = _plan(SCRATCH / "smoke-nosentinel")
        submitter = SmokingSubmitter("the run produced no sentinel")
        launcher, _, _, _, _ = _launcher(SCRATCH, submitter=submitter, smoke=True)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "smoke_evidence")

    def test_smoke_staging_binds_mode_and_manifest_flag(self) -> None:
        plan = _plan(SCRATCH / "smoke-bind")
        registry = bcc.derive_registry_input_plan_from_evidence()
        _stage(plan, registry, FakeClock(), smoke=True)
        binding = bcc.binding_from_staged_root(plan)
        self.assertEqual(binding.mode, "smoke")
        manifest = (plan.parent / bcc.STAGED_MANIFEST_NAME).read_text(encoding="utf-8")
        self.assertIn("--smoke", manifest)
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_staged_session(
                plan,
                expected=None,
                launcher_path=Path(bcc.__file__).resolve(),
                registry_plan=registry,
                smoke=False,
            )
        self.assertEqual(raised.exception.seam, "staged_session")

    def test_live_inner_against_smoke_binding_refuses_before_effects(self) -> None:
        plan = _plan(SCRATCH / "smoke-live-mismatch")
        clock = FakeClock()
        registry = bcc.derive_registry_input_plan_from_evidence()
        _stage(plan, registry, clock, smoke=True)
        launcher, _, _, image, spawner = _launcher(
            SCRATCH, plan=plan, clock=clock, admitted_root=plan.parent, smoke=False
        )
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "staged_session")
        self.assertEqual(spawner.calls, [])
        self.assertEqual(image.creates, [])


def main() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        signal.alarm(0)
