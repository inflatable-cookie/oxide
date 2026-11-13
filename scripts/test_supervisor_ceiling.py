"""Effect-free arithmetic cases for the unqualified ceiling candidate.

Full parse, admission, custody, and closure composition coverage is still
required before this candidate can run or receive qualification.
"""
import importlib.util
import pathlib
import unittest


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


if __name__ == "__main__":
    unittest.main()
