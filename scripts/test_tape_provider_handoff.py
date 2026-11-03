#!/usr/bin/env python3
"""Pure control: local tape handoff refuses without executing anything."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('tape_handoff', Path(__file__).with_name('tape-provider-handoff.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class HandoffTest(unittest.TestCase):
    def test_unqualified_local_invocation_refuses_without_child(self):
        with mock.patch.object(subprocess, 'run') as child:
            with self.assertRaisesRegex(SystemExit, 'qualified shared'):
                module.handoff()
        child.assert_not_called()


if __name__ == '__main__':
    unittest.main()
