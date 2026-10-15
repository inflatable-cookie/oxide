#!/usr/bin/env python3
"""Synthetic fixtures for the private build-context comparison launcher.

Never enters the live image/Cargo path. Fixture roots stay inside one fresh
mktemp directory. Aggregate budget is 60 seconds.
"""
from __future__ import annotations

import io
import json
import os
import py_compile
import signal
import sys
import tempfile
import time
import unittest
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
        rustc_running_prefix: str = "/usr/bin/rustc",
        negative_stderr: str | None = None,
    ) -> None:
        self.clock = clock
        self.rustc_ok = rustc_ok
        self.cargo_ok = cargo_ok
        self.advance_ns = advance_ns
        self.rustc_running_prefix = rustc_running_prefix
        self.negative_stderr = negative_stderr if negative_stderr is not None else DEFAULT_NEGATIVE_STDERR
        self.calls: list[tuple[str, ...]] = []
        self.envs: list[dict[str, str]] = []
        self.unknown_closure = False

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
        if argv[:2] == ["cargo", "-vV"]:
            stdout = f"cargo {bcc.RUSTC_RELEASE} (aaaaaaaaaaa 2026-01-01)\n" if self.cargo_ok else "cargo 1.96.1\n"
        elif argv[:2] == ["rustc", "-vV"]:
            if self.rustc_ok:
                stdout = f"release: {bcc.RUSTC_RELEASE}\ncommit-hash: {bcc.RUSTC_COMMIT_PREFIX}deadbeef\n"
            else:
                stdout = "release: 1.96.1\ncommit-hash: 000000000\n"
        elif argv and argv[0] == "effigy":
            target = Path(env["CARGO_TARGET_DIR"]) / "debug" / "deps"
            target.mkdir(parents=True, exist_ok=True)
            exe = target / "packages-deadbeef"
            if not exe.exists():
                exe.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
                os.chmod(exe, 0o755)
            obj = target / "packages-deadbeef.o"
            if not obj.exists():
                obj.write_bytes(b"obj")
            rel = "target/debug/deps/packages-deadbeef"
            stderr = (
                "   Compiling nucleus-core v0.0.0\n"
                "    Fresh syn v2.0.0\n"
                f"     Running `{self.rustc_running_prefix} --crate-name packages --edition 2021`\n"
                f"     Running tests/packages.rs ({rel})\n"
            )
            stdout = (
                f"test {bcc.TEST_NAME} ... ok\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
            )
        elif argv and "packages-deadbeef" in argv[0]:
            stderr = self.negative_stderr
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
            exit_code=1 if argv and "packages-deadbeef" in argv[0] else 0,
            pgid=1000 + len(self.calls),
            writer_closed=True,
            stdout=stdout,
            stderr=stderr,
            duration_ns=self.advance_ns,
        )


def _git_ok(argv, cwd=None) -> str:
    if argv[:2] == ["rev-parse", "HEAD"]:
        return bcc.NUCLEUS_COMMIT + "\n"
    if argv and argv[0] == "rev-parse" and "tree" in argv[1]:
        return bcc.NUCLEUS_TREE + "\n"
    if argv == ["ls-files"]:
        return "Cargo.lock\nCargo.toml\n"
    if argv[:2] == ["status", "--porcelain"] or argv[:1] == ["status"]:
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


def _plan(scratch: Path, launch_id: str = "fixture") -> bcc.ResourcePlan:
    parent = scratch / f"cmp-{launch_id}"
    src = scratch / "registry-src"
    src.mkdir(parents=True, exist_ok=True)
    crate = src / "foo.crate"
    crate.write_bytes(b"crate")
    return bcc.ResourcePlan(
        launch_id=launch_id,
        parent=parent,
        mount=parent / "m",
        backing=parent / "cmp.sparseimage",
        nucleus_source=scratch / "nucleus",
        registry_inputs=(bcc.RegistryInput(src=crate, rel="registry/cache/foo.crate"),),
    )


def _launcher(scratch: Path, **overrides):
    clock = overrides.pop("clock", FakeClock())
    image = overrides.pop("image", FakeImage(clock))
    spawner = overrides.pop("spawner", FakeSpawner(clock))
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
    return bcc.ComparisonLauncher(
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
        **overrides,
    ), clock, image, spawner


class PlanTests(unittest.TestCase):
    def test_plan_does_not_execute_effects(self) -> None:
        launcher, _, image, spawner = _launcher(SCRATCH)
        document = launcher.run(live=False, resource_plan=None)
        self.assertFalse(document["effects_executed"])
        self.assertEqual(document["outcome"], "plan")
        self.assertEqual(len(document["count"]["planned"]), 10)
        self.assertFalse(document["cargo_offline_readonly"])
        self.assertEqual(image.creates, [])
        self.assertEqual(spawner.calls, [])
        self.assertIn("time.monotonic_ns", document["clocks"]["deadline_clock"])

    def test_live_disabled_without_resource_plan(self) -> None:
        launcher, _, _, _ = _launcher(SCRATCH)
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

    def test_second_operation_after_expiry_refused(self) -> None:
        clock = FakeClock()
        launcher, _, _, spawner = _launcher(SCRATCH, clock=clock, budget_ns=10, reserve_ns=0)
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
        clock = FakeClock()
        spawner = FakeSpawner(clock)
        spawner.unknown_closure = True
        image = FakeImage(clock)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner, image=image)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "unknown"))
        self.assertEqual(document["outcome"], "unknown")
        self.assertTrue(document["resources_retained"])
        self.assertFalse(image.detached)
        self.assertTrue(document["preparation_after_clock"])
        self.assertIsNotNone(image.create_at)
        self.assertGreaterEqual(image.create_at, document["session"]["start_monotonic_ns"])


class ContainmentTests(unittest.TestCase):
    def test_backing_under_mount_refuses(self) -> None:
        parent = Path("/private/var/tmp/oxide-build-context-cmp-x")
        plan = bcc.ResourcePlan(
            launch_id="x",
            parent=parent,
            mount=parent / "m",
            backing=parent / "m" / "cmp.sparseimage",
            nucleus_source=Path("/Users/tom/Dev/projects/nucleus"),
            registry_inputs=(bcc.RegistryInput(src=SCRATCH / "c", rel="a"),),
        )
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_resource_plan(plan)
        self.assertIn(raised.exception.seam, {"backing_under_mount", "containment_names"})

    def test_existing_parent_refuses(self) -> None:
        plan = _plan(SCRATCH / "exists")
        plan.parent.mkdir(parents=True)
        launcher, _, _, _ = _launcher(SCRATCH)
        document = launcher.plan(plan)
        self.assertFalse(document["live_ready"])
        self.assertTrue(any("path_exists" in note for note in document["notes"]))

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
        clock = FakeClock()
        image = FakeImage(clock, IMAGEINFO_OVER)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, image=image)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "over"))
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "footprint")

    def test_host_space_unknown_stops(self) -> None:
        launcher, _, _, _ = _launcher(SCRATCH, space_probe=lambda: None)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "space"))
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "host_reserve")

    def test_source_cache_unbound_refuses(self) -> None:
        parent = Path("/private/var/tmp/oxide-build-context-cmp-unbound")
        plan = bcc.ResourcePlan(
            launch_id="unbound",
            parent=parent,
            mount=parent / "m",
            backing=parent / "cmp.sparseimage",
            nucleus_source=Path("/Users/tom/Dev/projects/nucleus"),
            registry_inputs=(),
        )
        with self.assertRaises(bcc.ComparisonError) as raised:
            bcc.validate_resource_plan(plan)
        self.assertEqual(raised.exception.seam, "source_cache_unbound")

    def test_source_cache_copy_stays_inside_mount(self) -> None:
        clock = FakeClock()
        image = FakeImage(clock)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, image=image)
        plan = _plan(SCRATCH / "cache")
        document = launcher.run(live=True, resource_plan=plan)
        copied = plan.mount / "cargo-home" / "registry" / "cache" / "foo.crate"
        self.assertTrue(copied.is_file())
        self.assertEqual(copied.read_bytes(), b"crate")
        self.assertTrue(document["preparation_after_clock"])


class IdentityTests(unittest.TestCase):
    def test_tool_mismatch_refuses_before_tests(self) -> None:
        clock = FakeClock()
        spawner = FakeSpawner(clock, rustc_ok=False)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "tool"))
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "tool_identity")
        self.assertFalse(any(call and call[0] == "effigy" for call in spawner.calls))

    def test_mixed_cargo_rustc_refuses(self) -> None:
        clock = FakeClock()
        spawner = FakeSpawner(clock, cargo_ok=False)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "mixed"))
        self.assertEqual(document["seam"], "tool_identity")
        self.assertEqual(spawner.calls, [("cargo", "-vV")])

    def test_source_pin_mismatch_refuses(self) -> None:
        launcher, _, _, spawner = _launcher(SCRATCH, git=_git_bad_tree)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "src"))
        self.assertEqual(document["seam"], "source_identity")
        self.assertFalse(any(call and call[0] == "effigy" for call in spawner.calls))

    def test_forbidden_config_refuses(self) -> None:
        plan = _plan(SCRATCH / "cfg")
        config_dir = SCRATCH / "cfg-config"
        config_dir.mkdir()
        config = config_dir / "config.toml"
        config.write_text("[build]\ntarget-dir = '/tmp/nope'\n", encoding="utf-8")

        def paths(*args):
            return [config]

        launcher, _, _, _ = _launcher(SCRATCH, config_paths=paths)
        document = launcher.run(live=True, resource_plan=plan)
        self.assertEqual(document["seam"], "config_identity")


class AdmissionTests(unittest.TestCase):
    def test_admission_fields_propagated_and_token_redacted(self) -> None:
        clock = FakeClock()
        spawner = FakeSpawner(clock)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "adm"))
        self.assertGreaterEqual(len(spawner.envs), 1)
        child = spawner.envs[0]
        self.assertEqual(child["HOST_RUN_TOKEN"], TOKEN)
        self.assertEqual(child["HOST_RUN_ID"], "run_0001")
        self.assertEqual(child["EFFIGY_HOST_SCHEDULER"], "1")
        self.assertIn("EFFIGY_HOST_RUN_ROOT", child)
        self.assertNotIn(TOKEN, document["log"])
        self.assertIn("<redacted>", document["log"])
        self.assertEqual(child["CARGO_INCREMENTAL"], "0")
        self.assertEqual(child["CARGO_PROFILE_DEV_SPLIT_DEBUGINFO"], "unpacked")

    def test_admission_names_match_contract(self) -> None:
        self.assertEqual(
            bcc.ADMISSION_PRESERVE[:2],
            ("HOST_RUN_TOKEN", "HOST_RUN_ID"),
        )
        self.assertIn("EFFIGY_HOST_RUN_ROOT", bcc.ADMISSION_PRESERVE)
        self.assertIn("EFFIGY_HOST_SCHEDULER", bcc.ADMISSION_PRESERVE)
        self.assertIn("EFFIGY_SCHEDULER_OVERRIDE", bcc.ADMISSION_PRESERVE)


class EvidenceTests(unittest.TestCase):
    def test_output_overflow_keeps_critical_identity(self) -> None:
        log = bcc.EvidenceLog(limit=2048, critical_reserve=256)
        log.write("x" * 4000)
        self.assertTrue(log.overflow)
        log.write(f"selected_test={bcc.TEST_NAME} failed", critical=True)
        text = log.text()
        self.assertIn("partial=true", text)
        self.assertIn(bcc.TEST_NAME, text)

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

    def test_restore_after_negative_check(self) -> None:
        clock = FakeClock()
        git_calls: list[list[str]] = []

        def git(argv, cwd=None):
            git_calls.append(list(argv))
            return _git_ok(argv, cwd)

        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, git=git)
        plan = _plan(SCRATCH / "neg")
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
        clock = FakeClock()
        spawner = FakeSpawner(clock, negative_stderr=HEADER_ONLY_NEGATIVE_STDERR)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "hdr"))
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "backtrace_frame")

    def test_restore_dirty_porcelain_is_partial(self) -> None:
        def git(argv, cwd=None):
            if argv[:2] == ["status", "--porcelain"]:
                return " M crates/nucleus-core/src/lib.rs\n"
            return _git_ok(argv, cwd)

        launcher, _, _, _ = _launcher(SCRATCH, git=git)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "dirty"))
        self.assertEqual(document["outcome"], "partial")
        self.assertEqual(document["seam"], "restore")

    def test_union_and_invocation_measurements_wired(self) -> None:
        clock = FakeClock()
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "meas"))
        self.assertEqual(document["outcome"], "success")
        self.assertEqual(len(document["measurements"]), 8)
        rec = document["measurements"][0]
        self.assertEqual(rec["mode"], "off")
        self.assertEqual(rec["step"], "C0")
        self.assertEqual(rec["exit_code"], 0)
        self.assertGreater(rec["duration_ns"], 0)
        self.assertGreaterEqual(rec["compiling"], 1)
        self.assertGreaterEqual(rec["fresh"], 1)
        self.assertIn("test result:", rec["test_result"] or "")
        self.assertGreaterEqual(rec["deps_files"], 1)
        self.assertGreaterEqual(rec["union_file_count"], 1)
        self.assertIn("cannot confound", rec["apfs_clone_sharing"])
        self.assertIn("measure off C0", document["log"])

    def test_rustc_wrapper_prefix_refuses(self) -> None:
        clock = FakeClock()
        spawner = FakeSpawner(clock, rustc_running_prefix="/usr/bin/sccache /usr/bin/rustc")
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "wrap"))
        self.assertEqual(document["outcome"], "refusal")
        self.assertEqual(document["seam"], "rustc_wrapper")


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


class SamplingTests(unittest.TestCase):
    def test_cadence_sample_during_helper(self) -> None:
        clock = FakeClock()
        spawner = FakeSpawner(clock, advance_ns=bcc.SAMPLE_CADENCE_NS)
        image = FakeImage(clock)
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner, image=image)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "cadence"))
        self.assertEqual(document["outcome"], "success")
        self.assertGreaterEqual(document["sampling"]["sample_count"], 2)
        self.assertIn("reason=cadence", document["log"])
        self.assertGreaterEqual(document["sampling"]["peak_image_used"], 4096)
        self.assertIsNotNone(document["sampling"]["min_host_free"])
        self.assertEqual(document["sampling"]["cadence_ns"], bcc.SAMPLE_CADENCE_NS)

    def test_cadence_image_threshold_stops(self) -> None:
        clock = FakeClock()
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
        launcher, _, _, _ = _launcher(SCRATCH, clock=clock, spawner=spawner, image=image)
        document = launcher.run(live=True, resource_plan=_plan(SCRATCH / "cad-th"))
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
        self.assertEqual(argv[1], "check:core:test")
        self.assertEqual(argv[2], "--")
        self.assertIn("--exact", argv)
        self.assertIn(bcc.TEST_NAME, argv)


def main() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        signal.alarm(0)
