"""Effect-free checks for retained constructor-bypassing monitor fixtures."""
import ast
import importlib.util
import io
import hashlib
import json
import os
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

    def test_expired_outer_window_never_forks(self):
        b = self.bootstrap
        with mock.patch.object(b, "now_ns", return_value=100), \
             mock.patch.object(b.os, "fork") as fork:
            self.assertEqual(b.bounded(lambda: 0, 100, 102), 124)
            self.assertEqual(b.bounded(lambda: 0, 101, 101), 124)
        fork.assert_not_called()
        with mock.patch.object(b, "now_ns", return_value=100), \
             mock.patch.object(b.os, "waitpid") as waitpid, \
             mock.patch.object(b.os, "kill") as kill:
            self.assertIsNone(b.hold(42, 100, [9]))
        waitpid.assert_not_called()
        kill.assert_not_called()

    def test_raw_sink_detects_split_markers_and_rejects_partial_writes(self):
        b = self.bootstrap
        stream = mock.Mock()
        stream.write.side_effect = lambda data: len(data)
        stream.fileno.return_value = 9
        sink = b.CaptureSink(stream)
        sink.write(b"output_trunc")
        sink.write(b"ated\neffigy: output complete (")
        with mock.patch.object(b.os, "fsync") as sync:
            record = sink.seal()
        self.assertEqual((record["truncation_markers"], record["footer_count"]), (1, 1))
        sync.assert_called_once_with(9)
        with self.assertRaises(ValueError):
            sink.write(b"late")
        stream = mock.Mock()
        stream.write.return_value = 1
        sink = b.CaptureSink(stream)
        with self.assertRaises(OSError):
            sink.write(b"partial")
        with self.assertRaises(ValueError):
            sink.seal()

    def test_raw_sink_extracts_one_complete_qualification_document(self):
        b = self.bootstrap
        stream = mock.Mock()
        stream.write.side_effect = lambda data: len(data)
        stream.fileno.return_value = 9
        sink = b.CaptureSink(stream)
        for piece in (b"qualifi", b"cation receipt: {\"mode\":", b"\"diagnostic\"}\n",
                      b"\neffigy: output complete (payload 3 bytes; cap 524288 bytes)\n"):
            sink.write(piece)
        with mock.patch.object(b.os, "fsync"):
            record = sink.seal()
        self.assertEqual(record["qualification_markers"], 1)
        self.assertEqual(record["qualification"], {"mode": "diagnostic"})
        self.assertEqual(b.capture_footer(record)["cap_bytes"], 524288)

    def test_capture_drains_to_eof_and_refuses_lost_eof(self):
        b = self.bootstrap
        sink = mock.Mock()
        sink.seal.return_value = {"sealed": True}
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b.select, "select", return_value=([7], [], [])), \
             mock.patch.object(b.os, "read", side_effect=[b"abc", b""]):
            self.assertEqual(b.drain_capture(7, sink, 20), {"sealed": True})
        sink.write.assert_called_once_with(b"abc")
        sink.seal.assert_called_once()
        sink.reset_mock()
        with mock.patch.object(b, "now_ns", return_value=20), \
             mock.patch.object(b.select, "select") as select_call:
            with self.assertRaises(TimeoutError):
                b.drain_capture(7, sink, 20)
        select_call.assert_not_called()
        sink.seal.assert_not_called()
        with mock.patch.object(b, "now_ns", side_effect=[10, 10, 10, 20]), \
             mock.patch.object(b.select, "select", return_value=([7], [], [])), \
             mock.patch.object(b.os, "read", return_value=b""):
            with self.assertRaises(TimeoutError):
                b.drain_capture(7, sink, 20)

    def test_bounded_command_drains_both_streams_and_preserves_unknown(self):
        b = self.bootstrap
        proc = mock.Mock(pid=42)
        proc.stdout.fileno.return_value = 7
        proc.stderr.fileno.return_value = 8
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b, "source_supervision_readiness", return_value={"ready": True}), \
             mock.patch.object(b.subprocess, "Popen", return_value=proc), \
             mock.patch.object(b.select, "select", return_value=([7, 8], [], [])), \
             mock.patch.object(b.os, "read", side_effect=[b"out", b"err", b"", b""]), \
             mock.patch.object(b, "hold", return_value=0):
            self.assertEqual(b.bounded_capture_command(["git", "status"], ".", 20, 10),
                             (0, b"out", b"err"))
        self.assertNotIn(proc, b._capture_unreaped)
        with mock.patch.object(b, "now_ns", side_effect=[10, 20]), \
             mock.patch.object(b, "source_supervision_readiness", return_value={"ready": True}), \
             mock.patch.object(b.subprocess, "Popen", return_value=proc):
            with self.assertRaises(TimeoutError):
                b.bounded_capture_command(["git", "status"], ".", 20, 10)
        self.assertIn(proc, b._capture_unreaped)
        b._capture_unreaped.remove(proc)

    def test_source_identity_requires_the_task_branch_and_clean_status(self):
        b = self.bootstrap
        rows = [(0, b"ns-55b433e9-d921-4cfb-a31a-98e2d58157a5\n", b""),
                (0, b"a" * 40 + b"\n", b""), (0, b"b" * 40 + b"\n", b""), (0, b"", b"")]
        with mock.patch.object(b, "bounded_capture_command", side_effect=rows):
            self.assertEqual(b.snapshot_git_identity(20)["status"], b"")
        rows[-1] = (0, b" M changed\0", b"")
        with mock.patch.object(b, "bounded_capture_command", side_effect=rows):
            with self.assertRaises(ValueError):
                b.snapshot_git_identity(20)

    def test_source_snapshot_refuses_unowned_child_custody_before_effects(self):
        b = self.bootstrap
        with mock.patch.object(b.os, "mkdir") as mkdir, \
             mock.patch.object(b, "bounded_capture_command") as command:
            with self.assertRaises(ValueError):
                b.snapshot_source_inputs("/unused", 20)
        mkdir.assert_not_called()
        command.assert_not_called()
        self.assertFalse(b.source_supervision_readiness()["qualified"])
        with mock.patch.object(b.subprocess, "Popen") as spawn:
            with self.assertRaises(ValueError):
                b.bounded_capture_command(["effigy", "docs:check"], ".", 20, 1024)
        spawn.assert_not_called()

    def test_real_capture_footer_parser_rejects_incomplete_output(self):
        b = self.bootstrap
        good = b"\neffigy: output complete (payload 1 bytes; cap 524288 bytes)\n"
        record = {"tail": good, "truncation_markers": 0, "footer_count": 1}
        self.assertEqual(b.capture_footer(record), {"payload_bytes": 1, "cap_bytes": 524288})
        for changes in ({"tail": good[:-1]}, {"tail": good + b"late"},
                        {"tail": good.replace(b"524288", b"524289")},
                        {"tail": good.replace(b"payload 1", b"payload 524288")},
                        {"footer_count": 0}, {"footer_count": 2},
                        {"truncation_markers": 1}):
            with self.assertRaises(ValueError):
                b.capture_footer(dict(record, **changes))

    def test_foreground_capture_uses_fixed_route_and_keeps_unknown_child(self):
        b = self.bootstrap
        proc = mock.Mock(pid=42)
        proc.stdout.fileno.return_value = 7
        raw = {"tail": b"\neffigy: output complete (payload 1 bytes; cap 524288 bytes)\n",
               "truncation_markers": 0, "footer_count": 1}
        provenance = {"environment": {"EFFIGY_STRICT_PERSISTENCE": "1",
                                       "EFFIGY_ADMISSION_PROTOCOL": None}}
        with mock.patch.object(b, "now_ns", side_effect=[10, 20]), \
             mock.patch.object(b, "source_supervision_readiness", return_value={"ready": True}), \
             mock.patch.object(b, "observe_installed_route", return_value=provenance), \
             mock.patch.object(b.subprocess, "Popen") as spawn:
            with self.assertRaises(TimeoutError):
                b.capture_foreground(mock.Mock(), 20)
        spawn.assert_not_called()
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b, "source_supervision_readiness", return_value={"ready": True}), \
             mock.patch.object(b, "observe_installed_route", return_value=provenance), \
             mock.patch.object(b.os, "pipe", return_value=(7, 8)), \
             mock.patch.object(b.os, "fpathconf", return_value=4096), \
             mock.patch.object(b.os, "close"), \
             mock.patch("builtins.open", mock.mock_open(read_data=b"source")), \
             mock.patch.object(b, "derived_route_extra", return_value={"bounded": True}), \
             mock.patch.object(b.subprocess, "Popen", return_value=proc) as spawn, \
             mock.patch.object(b, "drain_capture", return_value=raw), \
             mock.patch.object(b, "hold", return_value=0):
            result = b.capture_foreground(mock.Mock(), 20)
        self.assertEqual(result["effigy_exit"], 0)
        self.assertEqual(spawn.call_args.args[0], b.NATIVE_CAPTURE_ARGV)
        self.assertEqual(spawn.call_args.kwargs["env"]["EFFIGY_OUTPUT_CAP_BYTES"], "524288")
        self.assertNotIn(proc, b._capture_unreaped)
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b, "source_supervision_readiness", return_value={"ready": True}), \
             mock.patch.object(b, "observe_installed_route", return_value=provenance), \
             mock.patch.object(b.os, "pipe", return_value=(7, 8)), \
             mock.patch.object(b.os, "fpathconf", return_value=4096), \
             mock.patch.object(b.os, "close"), \
             mock.patch("builtins.open", mock.mock_open(read_data=b"source")), \
             mock.patch.object(b, "derived_route_extra", return_value={"bounded": True}), \
             mock.patch.object(b.subprocess, "Popen", return_value=proc), \
             mock.patch.object(b, "drain_capture", side_effect=TimeoutError):
            with self.assertRaises(TimeoutError):
                b.capture_foreground(mock.Mock(), 20)
        self.assertIn(proc, b._capture_unreaped)
        b._capture_unreaped.remove(proc)
        self.assertIn(7, b._capture_open_pipes)
        b._capture_open_pipes.remove(7)

    def test_single_log_creation_refuses_existing_or_expired(self):
        b = self.bootstrap
        stream = mock.MagicMock()
        stream.__enter__.return_value = stream
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b.os, "open", return_value=8) as open_call, \
             mock.patch.object(b.os, "fdopen", return_value=stream), \
             mock.patch.object(b, "capture_foreground", return_value={"bytes": 1}) as run:
            self.assertEqual(b.capture_raw_log("owned.raw", 20), {"bytes": 1})
        self.assertTrue(open_call.call_args.args[1] & b.os.O_EXCL)
        run.assert_called_once_with(stream, 20)
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b.os, "open", side_effect=FileExistsError), \
             mock.patch.object(b, "capture_foreground") as run:
            with self.assertRaises(FileExistsError):
                b.capture_raw_log("owned.raw", 20)
        run.assert_not_called()
        with mock.patch.object(b, "now_ns", return_value=20), \
             mock.patch.object(b.os, "open") as open_call:
            with self.assertRaises(TimeoutError):
                b.capture_raw_log("owned.raw", 20)
        open_call.assert_not_called()

    def test_joined_group_evidence_requires_both_certain_censuses(self):
        b = self.bootstrap
        empty = {"certain": True, "members": 0, "uncertain": 0}
        receipt = {"leader": [42, 99], "outcome": {"closure": "closed"},
                   "census_join": {"started": True, "joined": True,
                                   "deciding": empty, "final": empty},
                   "deciding_census": empty, "deciding_census_seq": 3,
                   "custodian_census": dict(empty, seq=3)}
        self.assertEqual(b.joined_group_evidence(receipt, [42, 99])["deciding_sequence"], 3)
        for key, replacement in (("leader", [42, 100]),
                                 ("deciding_census_seq", None),
                                 ("custodian_census", dict(empty, seq=4)),
                                 ("outcome", {"closure": "UNCLOSED"}),
                                 ("census_join", {"started": True, "joined": False,
                                                  "deciding": empty, "final": empty}),
                                 ("census_join", {"started": True, "joined": True,
                                                  "deciding": empty, "final": dict(empty, uncertain=1)})):
            with self.assertRaises(ValueError):
                b.joined_group_evidence(dict(receipt, **{key: replacement}), [42, 99])

    def test_declared_backing_evidence_requires_positive_observation(self):
        b = self.bootstrap
        guard = mock.Mock()
        guard.read_custody.return_value = (["/owned/image"], None)
        guard.custody_problems.return_value = []
        with mock.patch.object(b, "candidate", return_value=guard), \
             mock.patch.object(b, "now_ns", return_value=10):
            self.assertEqual(b.declared_backing_evidence({}, mock.Mock(), 20)["state"],
                             "whole-backing-absent")
            guard.custody_problems.return_value = ["unreadable"]
            with self.assertRaises(ValueError):
                b.declared_backing_evidence({}, mock.Mock(), 20)
            guard.read_custody.return_value = (None, None)
            with self.assertRaises(ValueError):
                b.declared_backing_evidence({}, mock.Mock(), 20)

    def test_fixed_suite_rechecks_every_image_case(self):
        b = self.bootstrap
        names = ("test_success_detaches_exactly_the_requested_volume",
                 "test_an_unbound_identity_is_refused_before_any_detach",
                 "test_an_unrequested_attachment_keeps_the_owned_root",
                 "test_a_stuck_detach_child_is_closed_by_the_custodian",
                 "test_an_unreadable_identity_closes_the_child_in_the_monitor",
                 "test_an_unreadable_identity_can_be_handed_off_by_parentage",
                 "test_a_custodian_side_unreadable_lookup_refuses_without_signalling",
                 "test_a_failed_attempt_keeps_the_mount_and_is_unqualified")
        rows = [{"case": name, "dir": f"/owned/evidence/{i}"} for i, name in enumerate(names)]
        results = {"cases": rows, "outcomes": [[f"case-{i}", "pass"] for i in range(445)]}
        probe = mock.Mock()
        probe.realpath.side_effect = lambda path: path
        def document(path, _deadline):
            if path.endswith("receipt.json"):
                return {"root": "/owned", "evidence": path.rsplit("/", 1)[0]}
            return {"image": path.rsplit("/", 1)[0] + "/image"}
        def custody(ctx, _probe, _deadline):
            return {"backings": [ctx["evidence"].rsplit("/", 1)[0] + "/image"],
                    "state": "whole-backing-absent"}
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b, "read_capture_document", side_effect=document), \
             mock.patch.object(b, "declared_backing_evidence", side_effect=custody) as check:
            self.assertEqual(len(b.suite_backing_evidence(results, "/owned/evidence", probe, 20)), 8)
            self.assertEqual(check.call_count, 8)
            with self.assertRaises(ValueError):
                b.suite_backing_evidence(dict(results, cases=rows[:-1]), "/owned/evidence", probe, 20)

    def test_capture_receipt_publication_refuses_short_or_failed_writes(self):
        b = self.bootstrap
        stream = mock.MagicMock()
        stream.__enter__.return_value = stream
        stream.fileno.return_value = 8
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b.os, "open", return_value=8), \
             mock.patch.object(b.os, "fdopen", return_value=stream), \
             mock.patch.object(b.os, "fsync") as sync, \
             mock.patch.object(b.os, "link") as link:
            stream.write.side_effect = lambda data: len(data)
            ack = b.publish_capture_receipt("capture.json", {"complete": True, "qualified": True}, 20)
            provisional = json.loads(stream.write.call_args.args[0])
            self.assertFalse(provisional["complete"])
            self.assertFalse(provisional["qualified"])
            self.assertEqual(ack["published_ns"], 10)
            sync.assert_called_once_with(8)
            link.assert_called_once_with("capture.json.partial", "capture.json")
            link.reset_mock()
            stream.write.side_effect = None
            stream.write.return_value = 1
            with self.assertRaises(OSError):
                b.publish_capture_receipt("capture.json", {"complete": True}, 20)
            link.assert_not_called()
            stream.write.side_effect = lambda data: len(data)
            sync.side_effect = OSError("failed flush")
            with self.assertRaises(OSError):
                b.publish_capture_receipt("capture.json", {"complete": True}, 20)
            link.assert_not_called()
        stream = mock.MagicMock()
        stream.__enter__.return_value = stream
        stream.write.side_effect = lambda data: len(data)
        with mock.patch.object(b, "now_ns", side_effect=[10, 10, 20]), \
             mock.patch.object(b.os, "open", return_value=8), \
             mock.patch.object(b.os, "fdopen", return_value=stream), \
             mock.patch.object(b.os, "fsync"), mock.patch.object(b.os, "link"):
            with self.assertRaises(TimeoutError):
                b.publish_capture_receipt("capture.json", {"complete": True}, 20)
        self.assertFalse(json.loads(stream.write.call_args.args[0])["complete"])

    def test_actor_accepts_only_settled_in_window_publication(self):
        b = self.bootstrap
        acknowledgment = {"sha256": "a" * 64, "bytes": 100, "published_ns": 12}
        raw = json.dumps(acknowledgment).encode() + b"\n"
        with mock.patch.object(b, "START", 0), mock.patch.object(b, "now_ns", side_effect=[10, 14, 15]), \
             mock.patch.object(b.os, "fork", return_value=42), \
             mock.patch.object(b.os, "close"), mock.patch.object(b.os, "set_blocking"), \
             mock.patch.object(b.os, "read", side_effect=[raw, b""]), \
             mock.patch.object(b.signal, "signal"), mock.patch.object(b, "hold", return_value=0):
            result = b.bounded(lambda: 0, 20, 22, ack_pipe=(7, 8))
        self.assertTrue(result["complete"])
        self.assertFalse(result["qualified"])
        with mock.patch.object(b, "START", 0):
            for value, status, observed in ((raw, 1, 15), (raw, 0, 20),
                                             (b"", 0, 15),
                                             (raw + b"late", 0, 15),
                                             (b'{"sha256":"a","sha256":"b"}', 0, 15)):
                with self.assertRaises((ValueError, json.JSONDecodeError)):
                    b.settled_publication(value, status, observed, 20)

    def test_composed_synthetic_capture_is_complete_but_never_qualified(self):
        b = self.bootstrap
        empty = {"certain": True, "members": 0, "uncertain": 0}
        outcome = {"closure": "closed", "status": 0, "fault": None, "deadline": False}
        def census(leader, result):
            return {"leader": leader, "outcome": result,
                    "census_join": {"started": True, "joined": True, "deciding": empty, "final": empty},
                    "deciding_census": empty, "deciding_census_seq": 3,
                    "custodian_census": dict(empty, seq=3)}
        nested = dict(census([42, 99], outcome), evidence="/owned/evidence")
        pins = {os.path.basename(path): "a" * 64 for path in b.SOURCES.values()}
        hashes = {name: pins[os.path.basename(path)] for name, path in b.SOURCES.items()}
        qualification = {"mode": "diagnostic", "qualified": False,
                         "selector": "proof:supervisor:ceiling:qualify:diagnostic",
                         "head": "b" * 40, "sha256": hashes, "candidate_outcome": outcome,
                         "default_scratch": {"topology": {"evidence": "/owned/evidence"}},
                         "cases": [], "outcomes": []}
        stream = mock.Mock()
        stream.write.side_effect = lambda data: len(data)
        stream.fileno.return_value = 9
        sink = b.CaptureSink(stream)
        sink.write(b"qualification receipt: " + json.dumps(qualification).encode() + b"\n")
        sink.write(b"\neffigy: output complete (payload 3 bytes; cap 524288 bytes)\n")
        with mock.patch.object(b.os, "fsync"):
            record = sink.seal()
        record.update(effigy_exit=1, effigy_footer=b.capture_footer(record),
                      installed_provenance={"launcher_sha256": b.INSTALLED_LAUNCHER_SHA256,
                                            "binary_sha256": b.INSTALLED_BINARY_SHA256,
                                            "receipt_sha256": b.INSTALLED_RECEIPT_SHA256,
                                            "source_head": b.INSTALLED_SOURCE_HEAD,
                                            "source_tree": b.INSTALLED_SOURCE_TREE,
                                            "source_hashes": b.INSTALLED_SOURCE_INPUTS,
                                            "environment": {"EFFIGY_LAUNCH": None,
                                                            "EFFIGY_STRICT_PERSISTENCE": "1",
                                                            "EFFIGY_ADMISSION_PROTOCOL": None}},
                      route_extra={"bounded": True, "effigy_cap_including_marker_footer": 524288,
                                   "route_extra_bytes_max": 859136})
        probe = mock.Mock()
        probe.realpath.side_effect = lambda path: path
        def document(path, _deadline):
            return pins if path.endswith("ceiling-qualification-pins.json") else nested
        with mock.patch.object(b, "verify_source_pins"), \
             mock.patch.object(b, "read_capture_document", side_effect=document), \
             mock.patch.object(b, "installed_file_digest", return_value=record["sha256"]), \
             mock.patch.object(b, "suite_backing_evidence", return_value=[{"state": "whole-backing-absent"}]):
            candidate_evidence = b.inspect_candidate_capture(record, "/owned/capture.raw", {"head": "b" * 40}, probe, 20)
        command = ["python", "worker"]
        group = dict(census([43, 100], {"closure": "closed", "status": 1, "fault": None, "deadline": False}),
                     command=command, evidence="/owned/outer", phase="monitor_complete",
                     output={name: {"bytes": 0, "persisted": 0, "forwarded": 0, "discarded": 0}
                             for name in ("stdout", "stderr")})
        self.assertNotEqual(b.inspect_outer_group(group, command, "/owned/outer", [42, 99])["leader"],
                            candidate_evidence["candidate_group"]["leader"])
        for bad in (dict(group, command=["other"]), dict(group, phase="started"),
                    dict(group, leader=[42, 99]), dict(group, output={}),
                    dict(group, outcome={"closure": "closed", "status": 1, "fault": None, "deadline": True})):
            with self.assertRaises(ValueError):
                b.inspect_outer_group(bad, command, "/owned/outer", [42, 99])
        output = mock.MagicMock()
        output.__enter__.return_value = output
        output.write.side_effect = lambda data: len(data)
        with mock.patch.object(b, "now_ns", return_value=10), \
             mock.patch.object(b.os, "open", return_value=8), mock.patch.object(b.os, "fdopen", return_value=output), \
             mock.patch.object(b.os, "fsync"), mock.patch.object(b.os, "link"):
            publication = b.publish_capture_receipt("/owned/final.json", {"candidate": candidate_evidence}, 20)
        with mock.patch.object(b, "START", 0):
            result = b.settled_publication(json.dumps(publication).encode(), 0, 15, 20)
        self.assertTrue(result["complete"])
        self.assertFalse(result["qualified"])

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
        installed = {name: 1 for name in b.INSTALLED_SOURCE_INPUTS}
        result = b.capture_fit(sizes, 200_001, 100_001, b.MIB, 4096,
                               ring_physical=20480, gates_physical=8192, prepare_physical=4096,
                               installed_source_sizes=installed)
        self.assertTrue(result["fits"])
        ceil = lambda n: ((n + 4095) // 4096) * 4096
        self.assertEqual(result["parts"]["raw_log_once"], 67 * b.MIB)
        self.assertEqual(result["parts"]["source_copies_before_after"],
                         2 * sum(ceil(n) for n in sizes.values()))
        self.assertEqual(result["parts"]["installed_source_copies_before_after"],
                         2 * sum(ceil(n) for n in installed.values()))
        self.assertEqual(result["parts"]["index_and_entries_before_after"],
                         2 * (ceil(200_001) + ceil(100_001)))
        self.assertEqual(result["parts"]["bootstrap_base"], 8 * b.MIB)
        self.assertEqual(result["parts"]["outer_group_root"], 8 * b.MIB)
        self.assertEqual(result["parts"]["outer_group_outputs"], 3 * b.MIB)
        self.assertEqual(result["parts"]["producer_manifest_receipt_copy"], 4 * b.MIB)
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
        physical = dict(ring_physical=20480, gates_physical=8192, prepare_physical=4096,
                        installed_source_sizes={name: 1 for name in b.INSTALLED_SOURCE_INPUTS})
        self.assertFalse(b.capture_fit(sizes, 1, 1, None, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, b.MIB + 1, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096, raw_copies=2, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096, raw_delivery_copies=1, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 30 * b.MIB, 1, 0, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit({}, 1, 1, 0, 4096, **physical)["fits"])
        self.assertFalse(b.capture_fit(sizes, 1, 1, 0, 4096,
                                       ring_physical=0, gates_physical=8192, prepare_physical=0)["fits"])

    def test_source_bound_guard_output_and_effigy_cap(self):
        b = self.bootstrap
        source = (scripts / "bounded-test-ceiling.py").read_bytes()
        tree = ast.parse(source)
        monitor = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Monitor")
        functions = {n.name: n for n in monitor.body if isinstance(n, ast.FunctionDef)}
        calls = lambda node, name: [x for x in ast.walk(node) if isinstance(x, ast.Call)
                                    and isinstance(x.func, ast.Attribute)
                                    and isinstance(x.func.value, ast.Name)
                                    and x.func.value.id == "self" and x.func.attr == name]
        self.assertEqual(len(calls(functions["finish_monitor"], "summary")), 2)
        self.assertEqual(len(calls(functions["run"], "summary")), 3)
        reports = [x for x in ast.walk(tree) if isinstance(x, ast.Call)
                   and isinstance(x.func, ast.Name) and x.func.id == "report"]
        summary_reports = [x for x in ast.walk(functions["summary"]) if isinstance(x, ast.Call)
                           and isinstance(x.func, ast.Name) and x.func.id == "report"]
        self.assertEqual((len(reports), len(summary_reports)), (14, 8))
        self.assertEqual(guard.FORWARD * 2, 64 * b.MIB)
        self.assertEqual(b.BOOTSTRAP_OUTPUT_LIMIT, 2 * b.MIB)
        self.assertEqual(b.EFFIGY_TEXT_CAP_BYTES, 524288)
        bound = b.derived_route_extra(source, 4096, b.EFFIGY_TEXT_CAP_BYTES)
        self.assertTrue(bound["bounded"])
        self.assertEqual(bound["guard_report_calls_max"], 81)
        self.assertEqual(bound["guard_actor_direct_bytes_max"], 334848)
        self.assertEqual(bound["route_extra_bytes_max"], 859136)
        self.assertLessEqual(bound["route_extra_bytes_max"], b.MIB)
        self.assertFalse(b.derived_route_extra(source + b"# drift", 512, b.EFFIGY_TEXT_CAP_BYTES)["bounded"])
        self.assertFalse(b.derived_route_extra(source, 4097, b.EFFIGY_TEXT_CAP_BYTES)["bounded"])
        self.assertFalse(b.derived_route_extra(source, 512, b.MIB)["bounded"])

    def test_installed_identity_and_launcher_drift_refuse(self):
        b = self.bootstrap
        with mock.patch.object(b.shutil, "which", return_value="/unexpected/effigy"), \
             mock.patch.object(b, "installed_file_digest") as digest:
            with self.assertRaises(ValueError):
                b.observe_installed_route(100)
        digest.assert_not_called()
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
        self.assertEqual(plan["prepare_store_wait_seconds"], 30)
        self.assertIsNone(plan["prepare_total_time_bound"])
        self.assertIsNone(plan["prepare_physical_bound"])
        self.assertEqual(plan["effective_protocol"], 1)
        self.assertEqual(b.strict_route_plan("1", "2", "ready", 20480)["effective_protocol"], 2)
        invalid = b.strict_route_plan("1", "3", "ready", 20480)
        self.assertIsNone(invalid["effective_protocol"])
        self.assertTrue(any("protocol" in reason for reason in invalid["missing"]))
        self.assertFalse(b.strict_route_plan("1", "2", "ready", 20480)["fits"])
        for state in ("missing", "full", "corrupt", "wrong-size"):
            refused = b.strict_route_plan("1", "2", state, None)
            self.assertFalse(refused["fits"])
            self.assertTrue(any("ring" in reason for reason in refused["missing"]))

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
