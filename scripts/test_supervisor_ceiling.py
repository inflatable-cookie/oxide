"""Effect-free arithmetic cases for the unqualified ceiling candidate.

Full parse, admission, custody, and closure composition coverage is still
required before this candidate can run or receive qualification.
"""
import importlib.util
import pathlib
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location(
    "bounded_ceiling", pathlib.Path(__file__).with_name("bounded-test-ceiling.py")
)
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)


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


if __name__ == "__main__":
    unittest.main()
