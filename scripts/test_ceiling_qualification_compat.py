"""Effect-free checks for retained constructor-bypassing monitor fixtures."""
import ast
import importlib.util
import json
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


if __name__ == "__main__":
    unittest.main()
