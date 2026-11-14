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

    def test_capture_composition_accounts_for_full_simultaneous_inventory(self):
        b = self.bootstrap
        sizes = {name: (scripts.parent / name).stat().st_size for name in b.CAPTURE_SOURCE_FILES}
        result = b.capture_fit(sizes, 200_001, 100_001, b.MIB, 4096,
                               ring_physical=20480, gates_physical=8192, prepare_physical=4096)
        self.assertTrue(result["fits"])
        ceil = lambda n: ((n + 4095) // 4096) * 4096
        self.assertEqual(result["parts"]["raw_log_once"], 67 * b.MIB)
        self.assertEqual(result["parts"]["source_copies_before_after"],
                         2 * sum(ceil(n) for n in sizes.values()))
        self.assertEqual(result["parts"]["index_and_entries_before_after"],
                         2 * (ceil(200_001) + ceil(100_001)))
        self.assertEqual(result["parts"]["bootstrap_base"], 8 * b.MIB)
        self.assertEqual(result["parts"]["producer_manifest_receipt_copy"], 3 * b.MIB)
        self.assertEqual(result["parts"]["manifest_and_delivery_copy"], 2 * b.MIB)
        self.assertEqual(result["parts"]["strict_ring_physical"], 20480)
        self.assertEqual(result["parts"]["gates_physical"], 8192)
        self.assertEqual(result["parts"]["separate_preparation_physical"], 4096)
        self.assertEqual(result["total"], sum(result["parts"].values()))
        self.assertEqual(result["remaining"], 128 * b.MIB - result["total"])
        self.assertEqual(b.NATIVE_CAPTURE_ARGV, ("effigy", "proof:supervisor:ceiling:qualify:diagnostic"))

    def test_capture_composition_refuses_unknown_or_extra_representations(self):
        b = self.bootstrap
        sizes = {name: 1 for name in b.CAPTURE_SOURCE_FILES}
        physical = dict(ring_physical=20480, gates_physical=8192, prepare_physical=4096)
        self.assertFalse(b.capture_fit(sizes, 1, 1, None, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, b.MIB + 1, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096, raw_copies=2, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096, raw_delivery_copies=1, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 30 * b.MIB, 1, 0, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit({}, 1, 1, 0, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096,
                                       ring_physical=0, gates_physical=8192, prepare_physical=0)["fits"])

    def test_installed_identity_and_launcher_drift_refuse(self):
        b = self.bootstrap
        args = [b.INSTALLED_LAUNCHER_SHA256, b.INSTALLED_BINARY_SHA256,
                b.INSTALLED_RECEIPT_SHA256, b.INSTALLED_SOURCE_HEAD, b.INSTALLED_SOURCE_TREE,
                None, "1", "2"]
        self.assertFalse(b.installed_route_control(*args)["fits"])
        for i in range(5):
            changed = list(args)
            changed[i] = "wrong"
            self.assertEqual(b.installed_route_control(*changed)["reason"], "installed route drift")
        changed = list(args)
        changed[5] = "stable"
        self.assertEqual(b.installed_route_control(*changed)["reason"], "installed route drift")
        self.assertFalse(b.installed_route_control(*args[:6], None, None)["fits"])

    def test_strict_preparation_is_distinct_and_never_grants_authority(self):
        b = self.bootstrap
        plan = b.strict_route_plan(None, None, None, None)
        self.assertFalse(plan["fits"])
        self.assertFalse(plan["native_authorized"])
        self.assertEqual(plan["ring_logical_bytes"], 16448)
        self.assertEqual(plan["terminal_writes"], [1023, 1])
        self.assertEqual(plan["prepare_argv"], ["effigy", "run", "prepare"])
        self.assertTrue(any("protocol" in reason for reason in plan["missing"]))
        self.assertFalse(b.strict_route_plan("1", "2", "ready", 20480)["fits"])

    def test_finalization_refuses_partial_marker_and_missing_evidence(self):
        b = self.bootstrap
        good = b"effigy: output complete (payload 1 bytes; cap 524288 bytes)\n"
        evidence = dict(source_attribution=True, capture_eof=True, joined_census=True,
                        terminal_settled=True, final_receipt=True)
        for raw in (b"", good[:-1], good + good, good + b"late",
                    b"output_truncated\n" + good,
                    b"effigy: output complete (payload 1 bytes; cap 524289 bytes)\n",
                    b"effigy: output complete (payload " + b"9" * 5000 + b" bytes; cap 524288 bytes)\n"):
            self.assertFalse(b.capture_finalization_control(raw, 524288, **evidence)["complete"])
        for key in evidence:
            missing = dict(evidence, **{key: False})
            self.assertFalse(b.capture_finalization_control(good, 524288, **missing)["complete"])
        result = b.capture_finalization_control(good, 524288, **evidence)
        self.assertFalse(result["qualified"])
        self.assertFalse(result["complete"])

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
