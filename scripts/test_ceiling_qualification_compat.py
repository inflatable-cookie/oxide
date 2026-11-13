"""Effect-free checks for retained constructor-bypassing monitor fixtures."""
import ast
import importlib.util
import io
import hashlib
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


class BootstrapBounds(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("ceiling_bootstrap", scripts / "qualify-supervisor-ceiling.py")
        cls.bootstrap = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.bootstrap)

    def test_partition_and_delivery_reserves(self):
        b = self.bootstrap
        self.assertEqual((b.GUARD_BUDGET_MIB, b.OUTSIDE_BUDGET_MIB), (384, 128))
        self.assertEqual(b.GUARD_BUDGET_MIB + b.OUTSIDE_BUDGET_MIB, 512)
        self.assertEqual(b.OUTSIDE_PARTS_MIB["forwarded_streams"], 64)
        self.assertGreaterEqual(b.OUTSIDE_PARTS_MIB["bootstrap_stream_and_actor"] * b.MIB, b.BOOTSTRAP_OUTPUT_LIMIT + 4096)
        self.assertLessEqual(sum(b.OUTSIDE_PARTS_MIB.values()), 128)
        self.assertEqual(b.EXPECTED_CASES, 445)
        self.assertEqual(guard.FORWARD, 32 * b.MIB)

    def test_pins_and_default_isolation(self):
        b = self.bootstrap
        raw = (scripts / "ceiling-qualification-pins.json").read_bytes()
        b.verify_source_pins(hashlib.sha256(raw).hexdigest())
        self.assertEqual(hashlib.sha256((scripts / "bounded-test.py").read_bytes()).hexdigest(),
                         "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b")
        self.assertEqual(hashlib.sha256((scripts / "bounded-test-candidate.py").read_bytes()).hexdigest(),
                         "c578243126a2470cde0dfd19530254b3ec61b4ea5bb1fa43c6bbd2d41853c2b7")
        self.assertNotIn('"--root-parent"', (scripts / "qualify-supervisor-ceiling.py").read_text())

    def test_input_rejects_oversize_and_malformed_without_truncation(self):
        b = self.bootstrap
        for content in (b'{}' + b' ' * b.MAX_DOCUMENT, b'{'):
            with mock.patch("builtins.open", return_value=io.BytesIO(content)):
                with self.assertRaises(ValueError):
                    b.read_document("irrelevant")
        with mock.patch("builtins.open", side_effect=FileNotFoundError):
            with self.assertRaises(FileNotFoundError):
                b.read_document("missing")
        with mock.patch("builtins.open", return_value=io.BytesIO(b'{"case":1}')):
            self.assertEqual(b.read_document("irrelevant"), {"case": 1})

    def test_output_rejects_entire_oversize_record(self):
        b = self.bootstrap
        with self.assertRaises(b.OutputLimit):
            b.qualification_bytes({"data": "x" * b.QUALIFICATION_DOCUMENT_LIMIT})
        sink = io.StringIO()
        with mock.patch.object(b, "_output_bytes", b.BOOTSTRAP_OUTPUT_LIMIT - 1):
            with self.assertRaises(b.OutputLimit):
                b.bounded_print("not a partial record", file=sink)
        self.assertEqual(sink.getvalue(), "")
        binary = io.BytesIO()
        alternate_encoding = mock.Mock(buffer=binary)
        with mock.patch.object(b, "_output_bytes", 0):
            b.bounded_print("é", file=alternate_encoding)
        self.assertEqual(binary.getvalue(), b"\xc3\xa9\n")
        alternate_encoding.write.assert_not_called()
        self.assertEqual(json.loads(b.qualification_bytes({"qualified": False})), {"qualified": False})


if __name__ == "__main__":
    unittest.main()
