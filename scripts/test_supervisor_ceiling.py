"""Effect-free arithmetic cases for the unqualified ceiling candidate.

Full parse, admission, custody, and closure composition coverage is still
required before this candidate can run or receive qualification.
"""
import importlib.util
import json
import pathlib
import sys
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location(
    "bounded_ceiling", pathlib.Path(__file__).with_name("bounded-test-ceiling.py")
)
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)
baseline_spec = importlib.util.spec_from_file_location(
    "bounded_qualified", pathlib.Path(__file__).with_name("bounded-test.py")
)
baseline = importlib.util.module_from_spec(baseline_spec)
baseline_spec.loader.exec_module(baseline)


class InitialWindow(unittest.TestCase):
    def test_earlier_equal_and_later_ceilings(self):
        ns = candidate.NS
        start = 10 * ns
        for ceiling, absolute in ((90, 90), (110, 110), (120, 110)):
            with self.subTest(ceiling=ceiling):
                self.assertEqual(candidate.ceiling_deadlines(start, ceiling * ns, 100),
                                 (absolute * ns, (absolute - 20) * ns, (absolute - 2) * ns))

    def test_expired_and_insufficient_allowance_refused(self):
        ns = candidate.NS
        for ceiling in (9, 10, 40):
            with self.subTest(ceiling=ceiling), self.assertRaises(ValueError):
                candidate.ceiling_deadlines(10 * ns, ceiling * ns, 100)

    def test_malformed_and_overflow_refused(self):
        for ceiling in (None, True, -1, 0, 1.5, candidate.U64MAX + 1):
            with self.subTest(ceiling=ceiling), self.assertRaises(ValueError):
                candidate.ceiling_deadlines(0, ceiling, 100)
        for limit in (float("nan"), float("inf"), 30, 1e100):
            with self.subTest(limit=limit), self.assertRaises(ValueError):
                candidate.ceiling_deadlines(0, candidate.U64MAX, limit)

    def test_parse_explicit_ceiling_and_default_shape(self):
        command, default = candidate.parse(["--limit", "100", "--", "proof"])
        self.assertEqual(command, ["proof"])
        self.assertNotIn("ceiling_ns", default)
        command, opted = candidate.parse(["--limit", "100", "--ceiling-uptime-ns",
                                          str(200 * candidate.NS), "--", "proof"])
        self.assertEqual(command, ["proof"])
        self.assertEqual(opted["ceiling_ns"], 200 * candidate.NS)

    def test_default_parse_matches_qualified_baseline(self):
        for args in (["proof"], ["--limit", "95.5", "--", "proof"],
                     ["--scratch-budget-mib", "128", "--", "proof"]):
            with self.subTest(args=args):
                self.assertEqual(candidate.parse(list(args)), baseline.parse(list(args)))

    def test_parse_rejects_invalid_ceiling_or_relative_limit(self):
        for value in ("-1", "+123", "1.0", " ", "9" * 21, str(candidate.U64MAX + 1)):
            with self.subTest(value=value), self.assertRaises(SystemExit):
                candidate.parse(["--ceiling-uptime-ns", value, "--", "proof"])
        for value in ("nan", "inf", "1e100"):
            with self.subTest(limit=value), self.assertRaises(SystemExit):
                candidate.parse(["--limit", value, "--ceiling-uptime-ns",
                                 str(candidate.U64MAX), "--", "proof"])
        with self.assertRaises(SystemExit):
            candidate.parse(["--ceiling-uptime-ns", str(200 * candidate.NS),
                             "--ceiling-uptime-ns", str(200 * candidate.NS), "proof"])


class CustodianWindow(unittest.TestCase):
    def make_custodian(self, start, ceiling, limit=100):
        custodian = candidate.Custodian(["proof"], limit_s=limit, ceiling_ns=ceiling)
        with mock.patch.object(candidate, "capability", return_value=[]), \
             mock.patch.object(candidate.signal, "signal"), \
             mock.patch.object(candidate, "now_ns", return_value=start), \
             mock.patch.object(candidate.secrets, "token_hex", return_value="synthetic"):
            custodian.setup()
        custodian.record = mock.Mock()
        return custodian

    def test_delayed_start_refuses_insufficient_window(self):
        ns = candidate.NS
        with self.assertRaises(SystemExit):
            self.make_custodian(80 * ns, 110 * ns)
        custodian = self.make_custodian(20 * ns, 110 * ns)
        self.assertEqual((custodian.start, custodian.absolute, custodian.stop_at),
                         (20 * ns, 110 * ns, 90 * ns))

    def test_grants_cap_and_at_stop_is_late(self):
        ns = candidate.NS
        custodian = self.make_custodian(0, 110 * ns)
        custodian.admit({"command": "a", "waited_ns": 20 * ns}, 30 * ns)
        self.assertEqual(custodian.absolute, 110 * ns)
        self.assertEqual(custodian.admission_granted, 10 * ns)
        self.assertEqual(custodian.admission_ceiling_capped_ns, 10 * ns)
        custodian.admit({"command": "a", "waited_ns": 20 * ns}, 40 * ns)
        self.assertEqual(custodian.admission_duplicates, 1)
        custodian.admit({"command": "b", "waited_ns": ns}, 90 * ns)
        self.assertEqual(custodian.admission_late_reason, "stopped")
        self.assertEqual(custodian.absolute, 110 * ns)

    def test_monitor_window_never_exceeds_ceiling(self):
        ns = candidate.NS
        ctx = {"start_ns": 10 * ns, "limit_s": 100, "ceiling_ns": 90 * ns}
        with mock.patch.object(candidate, "now_ns", return_value=89 * ns):
            self.assertEqual(candidate.monitor_absolute(ctx), 90 * ns)
            self.assertEqual(candidate.monitor_remaining(ctx, 2 * ns), 0)
        ctx["ceiling_ns"] = 120 * ns
        with mock.patch.object(candidate, "now_ns", return_value=110 * ns):
            self.assertEqual(candidate.monitor_absolute(ctx), 110 * ns)
            self.assertEqual(candidate.monitor_remaining(ctx), 10)

    def test_monitor_binds_only_valid_effective_deadline(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "initial_absolute_ns": 100 * ns,
               "ceiling_ns": 120 * ns}
        for value in (None, True, 99 * ns, 121 * ns):
            with self.subTest(value=value):
                self.assertFalse(candidate.bind_monitor_deadline(ctx, {"effective_deadline_ns": value}))
                self.assertNotIn("effective_deadline_ns", ctx)
        self.assertTrue(candidate.bind_monitor_deadline(ctx, {"effective_deadline_ns": 110 * ns}))
        with mock.patch.object(candidate, "now_ns", return_value=109 * ns):
            self.assertEqual(candidate.monitor_remaining(ctx), 1.0)

    def test_overflow_and_duplicate_reports_do_not_extend(self):
        ns = candidate.NS
        custodian = self.make_custodian(0, 200 * ns)
        for number in range(candidate.ADMISSION_COMMANDS):
            custodian.admission_pair(str(number))
        custodian.admit({"command": "overflow", "waited_ns": 30 * ns}, 30 * ns)
        self.assertEqual(custodian.admission_unclassified, 1)
        self.assertEqual(custodian.absolute, 100 * ns)

    def test_deadline_and_uncertain_census_never_close(self):
        ns = candidate.NS
        custodian = self.make_custodian(0, 150 * ns)
        custodian.census_ok = True
        custodian.eof = {"stdout", "stderr"}
        with mock.patch.object(candidate, "now_ns", return_value=50 * ns):
            self.assertTrue(custodian.closure(True))
            custodian.census_uncertain = True
            self.assertFalse(custodian.closure(True))
            custodian.census_uncertain = False
            custodian.deadline_hit = True
            self.assertFalse(custodian.closure(True))
        custodian.deadline_hit = False
        with mock.patch.object(candidate, "now_ns", return_value=custodian.stop_at):
            self.assertFalse(custodian.closure(True))

    def test_expired_setup_never_forks(self):
        ns = candidate.NS
        custodian = self.make_custodian(0, 100 * ns)
        with mock.patch.object(candidate, "now_ns", return_value=custodian.setup_at), \
             mock.patch.object(candidate.os, "fork") as fork, \
             mock.patch.object(candidate.os, "pipe") as pipe:
            self.assertFalse(custodian.launch_monitor())
            fork.assert_not_called()
            pipe.assert_not_called()

    def test_expired_image_child_never_spawns(self):
        spawn = mock.Mock()
        with mock.patch.object(candidate, "now_ns", return_value=200 * candidate.NS):
            result = candidate.bounded_run(["synthetic"], 10, spawn=spawn,
                                           deadline_ns=200 * candidate.NS)
        spawn.assert_not_called()
        self.assertFalse(result["reaped"])
        self.assertIsNone(result["handle"])

    def test_slow_spawn_does_not_refund_time_or_lose_child(self):
        ns = candidate.NS
        child = mock.Mock()
        child.stdout.detach.side_effect = OSError("synthetic descriptor failure")
        child.wait.side_effect = candidate.subprocess.TimeoutExpired("synthetic", 0)
        with mock.patch.object(candidate, "now_ns", side_effect=[0, 0, 20 * ns, 20 * ns]):
            result = candidate.bounded_run(["synthetic"], 10, spawn=mock.Mock(return_value=child),
                                           deadline_ns=10 * ns)
        child.wait.assert_called_once_with(timeout=0.0)
        self.assertIs(result["handle"], child)
        self.assertFalse(result["reaped"])

    def test_ceiling_receipt_binds_actual_decision(self):
        ns = candidate.NS
        custodian = self.make_custodian(5 * ns, 115 * ns)
        custodian.cmd_w = 99
        custodian.admit({"command": "a", "waited_ns": 20 * ns}, 35 * ns)
        with mock.patch.object(candidate.os, "write") as write, \
             mock.patch.object(candidate, "now_ns", return_value=40 * ns):
            custodian.observe_admission(True)
        record = json.loads(write.call_args.args[1])["record"]
        self.assertEqual(record["requested_ceiling_ns"], 115 * ns)
        self.assertEqual(record["start_ns"], 5 * ns)
        self.assertEqual(record["absolute_ns"], 115 * ns)
        self.assertEqual(record["granted_ns"], 10 * ns)
        self.assertEqual(record["ceiling_capped_ns"], 10 * ns)

    def test_default_admission_retains_relative_behavior(self):
        ns = candidate.NS
        custodian = self.make_custodian(0, None)
        custodian.admit({"command": "a", "waited_ns": 10 * ns}, 20 * ns)
        self.assertEqual(custodian.absolute, 110 * ns)
        self.assertEqual(custodian.admission_granted, 10 * ns)

    def test_entrypoint_passes_ceiling_without_spawning(self):
        ns = candidate.NS
        with mock.patch.object(candidate, "Custodian") as constructor:
            constructor.return_value.run.return_value = 7
            self.assertEqual(candidate.main(["--limit", "100", "--ceiling-uptime-ns",
                                             str(200 * ns), "--", "proof"]), 7)
        self.assertEqual(constructor.call_args.args[-1], 200 * ns)

    def test_late_grants_do_not_move_deadline(self):
        ns = candidate.NS
        for offset in (0, 1, 20):
            custodian = self.make_custodian(0, 150 * ns)
            initial = custodian.absolute
            custodian.admit({"command": "late", "waited_ns": 10 * ns},
                            custodian.stop_at + offset * ns)
            self.assertEqual(custodian.absolute, initial)
            self.assertEqual(custodian.admission_late_reason, "stopped")

    def test_stale_and_malformed_admission_markers_remain_rejected(self):
        marker = {"schema": 2, "source": "fixture", "run": "old", "command": "a", "waited_ns": 1}
        self.assertEqual(candidate.admission_malformed(marker, "current"), "stale run")
        marker["run"] = "current"
        marker["waited_ns"] = candidate.U64MAX + 1
        self.assertEqual(candidate.admission_malformed(marker, "current"), "ill-typed waited_ns")
        marker["waited_ns"] = True
        self.assertEqual(candidate.admission_malformed(marker, "current"), "ill-typed waited_ns")

    def test_low_reserve_latches_fault_and_refuses_closure(self):
        ns = candidate.NS
        custodian = self.make_custodian(0, 150 * ns)
        custodian.census_ok = True
        custodian.eof = {"stdout", "stderr"}
        custodian.guard.reading({"kind": "reserve", "value": 0, "at": ns,
                                 "completed": ns}, now=ns)
        with mock.patch.object(candidate, "now_ns", return_value=2 * ns):
            self.assertFalse(custodian.closure(True))
        self.assertIsNotNone(custodian.guard.fault)

    def test_unreadable_image_and_spent_window_keep(self):
        ns = candidate.NS
        probe = mock.Mock()
        facts = mock.Mock(return_value=None)
        with mock.patch.object(candidate, "now_ns", return_value=90 * ns):
            problems = candidate.custody_problems(["synthetic"], probe, facts,
                                                   deadline_ns=100 * ns)
        self.assertTrue(problems)
        with mock.patch.object(candidate, "now_ns", return_value=100 * ns):
            facts.reset_mock()
            problems = candidate.custody_problems(["synthetic"], probe, facts,
                                                   deadline_ns=100 * ns)
        self.assertTrue(problems)
        facts.assert_not_called()

    def test_incomplete_join_is_not_closure(self):
        final = {"members": 0, "uncertain": 0, "certain": True}
        joined = {"joined": False}
        result, _reason = candidate.census_settled(joined, final, final)
        self.assertFalse(result)

    def test_join_is_capped_to_remaining_exit_window(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "ceiling_ns": 90 * ns, "inject": []}
        monitor = candidate.Monitor(ctx, 0, 0, 0, 0)
        monitor.receipt = {"survivors": [], "deciding_census": None}
        monitor.census_thread = mock.Mock()
        monitor.census_thread.is_alive.return_value = True
        with mock.patch.object(candidate, "now_ns", return_value=87 * ns):
            result = monitor.join_census(10)
        monitor.census_thread.join.assert_called_once_with(1.0)
        self.assertFalse(result["joined"])

    def test_expired_drain_does_not_wait(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "ceiling_ns": 90 * ns, "inject": []}
        monitor = candidate.Monitor(ctx, 0, 0, 0, 0)
        with mock.patch.object(candidate, "now_ns", return_value=90 * ns), \
             mock.patch.object(candidate.select, "select") as select:
            monitor.drain_records(10)
        select.assert_not_called()

    def test_unreadable_image_keeps_root(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "ceiling_ns": 90 * ns}
        with mock.patch.object(candidate, "read_custody", return_value=(["synthetic"], None)), \
             mock.patch.object(candidate, "now_ns", return_value=50 * ns):
            result = candidate.cleanup_safety(ctx, probe=mock.Mock(), facts=mock.Mock(return_value=None))
        self.assertEqual(result["state"], "kept")

    def test_delayed_authorization_never_waits_past_ceiling(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "ceiling_ns": 90 * ns, "inject": []}
        monitor = candidate.Monitor(ctx, 0, 0, 0, 0)
        monitor.send = mock.Mock()
        with mock.patch.object(candidate, "now_ns", side_effect=[87 * ns, 87 * ns, 89 * ns, 89 * ns]), \
             mock.patch.object(candidate.select, "select") as select:
            allowed, _reason, grant = monitor.authorize_detach(10 * ns)
        self.assertFalse(allowed)
        self.assertEqual(grant, 0)
        select.assert_not_called()

    def test_expired_publication_and_setup_refuse_effects(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "ceiling_ns": 90 * ns,
               "setup_at_ns": 10 * ns, "inject": []}
        monitor = candidate.Monitor(ctx, 0, 0, 0, 0)
        monitor.receipt = {}
        with mock.patch.object(candidate, "now_ns", return_value=90 * ns), \
             mock.patch("builtins.open") as opened:
            with self.assertRaises(RuntimeError):
                monitor.publish("complete")
            with self.assertRaises(candidate.SetupRefused):
                monitor.check_setup_clock()
        opened.assert_not_called()

    def test_image_cleanup_budget_has_no_floor(self):
        ns = candidate.NS
        ctx = {"start_ns": 0, "limit_s": 100, "ceiling_ns": 90 * ns}
        with mock.patch.object(candidate, "now_ns", return_value=90 * ns):
            self.assertEqual(candidate.detach_budget(ctx), 0)


if __name__ == "__main__":
    if sys.argv[1:] == ["--plan"]:
        suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
        for group in suite:
            for case in group:
                print(case.id())
        print("Synthetic, effect-free cases only; no qualification credit.")
    else:
        unittest.main()
