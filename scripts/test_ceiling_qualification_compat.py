"""Effect-free checks for retained constructor-bypassing monitor fixtures."""
import ast
import importlib.util
import io
import hashlib
import json
import os
import sys
from pathlib import Path
import threading
import unittest
from unittest import mock


scripts = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("ceiling_compat", scripts / "bounded-test-ceiling.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class StubCompatibility(unittest.TestCase):
    def test_each_retained_constructor_bypass_sets_context(self):
        tree = ast.parse((scripts / "test_supervisor_ceiling_qualification.py").read_text())
        count = 0
        for function in (node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef)):
            # Exclude nested functions from this function's own statements.
            statements = [node for statement in function.body for node in ast.walk(statement)
                          if not isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))]
            constructors = [node for node in statements if isinstance(node, ast.Assign)
                            and isinstance(node.value, ast.Call)
                            and ast.unparse(node.value.func) == "bounded.Monitor.__new__"]
            for constructor in constructors:
                count += 1
                self.assertEqual(ast.unparse(constructor.targets[0]), "monitor")
                contexts = [node for node in statements if isinstance(node, ast.Assign)
                            and any(ast.unparse(target) == "monitor.ctx" for target in node.targets)
                            and node.lineno > constructor.lineno]
                self.assertTrue(contexts, f"{function.name}: monitor.ctx is missing")
                self.assertEqual(ast.literal_eval(min(contexts, key=lambda node: node.lineno).value), {})
        self.assertEqual(count, 4)

    def monitor(self):
        monitor = guard.Monitor.__new__(guard.Monitor)
        monitor.ctx = {}
        monitor.cmd_r = 7
        monitor.lock = threading.Lock()
        monitor.receipt = {"custodian": []}
        monitor.send = mock.Mock()
        return monitor

    def test_drain_and_await_keep_records_with_absent_option(self):
        monitor = self.monitor()
        record = json.dumps({"type": "record", "record": {"event": "pinned"}}).encode() + b"\n"
        with mock.patch.object(guard.select, "select", side_effect=[([7], [], []), ([], [], [])]), \
             mock.patch.object(guard.os, "read", return_value=record):
            monitor.drain_records(0.3)
        self.assertEqual(monitor.receipt["custodian"], [{"event": "pinned"}])
        answer = record + json.dumps({"type": "detach_child_ack", "closed": True}).encode() + b"\n"
        with mock.patch.object(guard.select, "select", return_value=([7], [], [])), \
             mock.patch.object(guard.os, "read", return_value=answer):
            self.assertTrue(monitor.await_command("detach_child_ack", 0.3)["closed"])
        self.assertEqual(len(monitor.receipt["custodian"]), 2)

    def test_detach_authorization_uses_absent_option_context(self):
        monitor = self.monitor()
        answer = json.dumps({"type": "detach_go", "allowed": True, "budget_ns": guard.NS}).encode() + b"\n"
        with mock.patch.object(guard.select, "select", return_value=([7], [], [])), \
             mock.patch.object(guard.os, "read", return_value=answer):
            self.assertEqual(monitor.authorize_detach(2 * guard.NS), (True, None, guard.NS))



path = Path(__file__).with_name("qualify-supervisor-ceiling.py")
spec = importlib.util.spec_from_file_location("ceiling_entry", path)
entry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(entry)


class Entrypoint(unittest.TestCase):
    def test_command_keeps_candidate_limit_budget_and_suite(self):
        command = entry.command(Path("/owned"), "digest", 123456)
        self.assertIn("--root-parent", command)
        self.assertEqual(command[command.index("--limit") + 1], "560")
        self.assertEqual(command[command.index("--scratch-budget-mib") + 1], "384")
        self.assertEqual(command[command.index("--ceiling-uptime-ns") + 1], "123456")
        self.assertEqual(command[-2:], ["--pins-sha256", "digest"])
        self.assertIn(str(entry.SCRIPTS / "test_supervisor_ceiling_qualification.py"), command)

    def test_missing_pin_refuses_before_scratch(self):
        with mock.patch.object(entry.tempfile, "mkdtemp") as create:
            with self.assertRaises(SystemExit):
                entry.main([])
        create.assert_not_called()

    def test_changed_manifest_refuses_before_scratch(self):
        with mock.patch.object(entry.tempfile, "mkdtemp") as create:
            with self.assertRaises(SystemExit):
                entry.main(["--pins-sha256", "wrong"])
        create.assert_not_called()


class FailureReporting(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        path = scripts / "test_supervisor_ceiling_qualification.py"
        digest = hashlib.sha256((scripts / "ceiling-qualification-pins.json").read_bytes()).hexdigest()
        spec = importlib.util.spec_from_file_location("ceiling_suite", path)
        cls.suite = importlib.util.module_from_spec(spec)
        with mock.patch.object(sys, "argv", [str(path), "--pins-sha256", digest]):
            spec.loader.exec_module(cls.suite)

    def test_failure_is_flushed_with_case_and_fixture_before_summary(self):
        output = io.StringIO()
        stream = unittest.runner._WritelnDecorator(output)
        stream.flush = mock.Mock(wraps=stream.flush)
        result = self.suite.Results(stream, True, 2)
        test = unittest.FunctionTestCase(lambda: None)
        test.dir = "/owned/case"
        self.suite.CASES.append({"case": test.id().rsplit(".", 1)[-1], "exit": 0})
        try:
            result.startTest(test)
            try:
                raise AssertionError("specific failure")
            except AssertionError:
                result.addFailure(test, sys.exc_info())
            self.assertIn("active case:", output.getvalue())
            self.assertIn("specific failure", output.getvalue())
            self.assertIn("/owned/case", output.getvalue())
            self.assertGreaterEqual(stream.flush.call_count, 2)
            self.assertEqual(self.suite.Results.outcomes[-1][1], "fail")
        finally:
            self.suite.CASES.pop()
            self.suite.Results.outcomes.clear()

    def test_error_traceback_is_bounded_and_retains_error_status(self):
        output = io.StringIO()
        result = self.suite.Results(unittest.runner._WritelnDecorator(output), True, 2)
        test = unittest.FunctionTestCase(lambda: None)
        try:
            try:
                raise RuntimeError("x" * 12000)
            except RuntimeError:
                result.addError(test, sys.exc_info())
            self.assertIn("[traceback truncated]", output.getvalue())
            self.assertLess(len(output.getvalue()), 10000)
            self.assertEqual(self.suite.Results.outcomes[-1][1], "error")
        finally:
            self.suite.Results.outcomes.clear()


class CensusStageControls(unittest.TestCase):
    def test_trace_does_not_change_census_result(self):
        row = {"pid": 42, "start": 7, "zombie": False}
        def census():
            return guard.Census(1, snapshot=lambda: [row], getsid=lambda pid: 9,
                                reread=lambda pid: row)
        baseline = census().round()
        instrumented = census()
        instrumented.operation_trace = {"entered": None, "returned": None}
        self.assertEqual(instrumented.round(), baseline)
        self.assertEqual(instrumented.operation_trace["entered"]["operation"], "identity_reread")
        self.assertEqual(instrumented.operation_trace["returned"]["outcome"], "returned")

    def test_trace_preserves_last_entered_when_read_is_interrupted(self):
        class Interrupted(BaseException):
            pass
        row = {"pid": 42, "start": 7, "zombie": False}
        census = guard.Census(1, snapshot=lambda: [row], getsid=mock.Mock(side_effect=Interrupted()))
        census.operation_trace = {"entered": None, "returned": None}
        with self.assertRaises(Interrupted):
            census.round()
        self.assertEqual(census.operation_trace["entered"],
                         {"site": "round", "operation": "getsid", "pid": 42, "start": 7})
        self.assertEqual(census.operation_trace["returned"]["outcome"], "raised")

    def test_interruption_before_round_return_can_occur_in_distinct_stages(self):
        class Interrupted(BaseException):
            pass

        read = mock.Mock(side_effect=Interrupted("row"))
        snapshot = mock.Mock(side_effect=Interrupted("snapshot"))
        with self.assertRaisesRegex(Interrupted, "snapshot"):
            guard.Census(1, snapshot=snapshot, getsid=read).round()
        read.assert_not_called()

        row = {"pid": 42, "start": 7, "zombie": False}
        snapshot = mock.Mock(return_value=[row])
        with self.assertRaisesRegex(Interrupted, "row"):
            guard.Census(1, snapshot=snapshot, getsid=read).round()
        snapshot.assert_called_once()
        read.assert_called_once_with(42)


if __name__ == "__main__":
    unittest.main()
