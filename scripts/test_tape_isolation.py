#!/usr/bin/env python3
"""Source-level regression cases for the task-only private launcher.

These mock the live inventory and supervised child; no live account tree,
fixture, compiler, or subprocess is touched by this test module.
"""
import importlib.util
import os
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('tape_isolation', Path(__file__).with_name('tape-isolation-check.py'))
route = importlib.util.module_from_spec(spec)
spec.loader.exec_module(route)


class InventoryGateTests(unittest.TestCase):
    def invoke(self, inventories, outcome=None):
        saved = []
        def record(path, value):
            saved.append((path.name, value))
        with mock.patch.object(route, 'budget', return_value=(512, 'ledger-digest')), \
             mock.patch.object(route, 'inventory_account', side_effect=inventories), \
             mock.patch.object(route, 'receipt', side_effect=record), \
             mock.patch.object(route, 'revalidate_sources'), \
             mock.patch.object(route, 'run_bounded', side_effect=outcome or (lambda *args: {'argv': ['actual-command'], 'exit': 0})):
            result = route.guarded_run(Path('/owned'), {'tmp': Path('/owned/tmp'), 'work': Path('/owned/work')}, {}, 'probe', ['child'], {'commit': 'c', 'tree': 't'})
        return result, saved

    def test_unchanged_inventory_retains_both_receipts(self):
        result, saved = self.invoke([[{'ino': 1}], [{'ino': 1}]])
        self.assertEqual(result['argv'], ['actual-command'])
        self.assertEqual(result['budget_ledger_sha256'], 'ledger-digest')
        self.assertEqual([name for name, _ in saved], ['probe-inventory-before.json', 'probe-result.json', 'probe-inventory-after.json', 'probe-source-revalidated.json'])

    def test_same_bytes_replaced_inode_refuses(self):
        with self.assertRaises(route.Refusal):
            self.invoke([[{'ino': 1, 'sha256': 'same'}], [{'ino': 2, 'sha256': 'same'}]])

    def test_child_failure_still_takes_after_inventory(self):
        with mock.patch.object(route, 'budget', return_value=(512, 'ledger-digest')), \
             mock.patch.object(route, 'inventory_account', side_effect=[[{'ino': 1}], [{'ino': 1}]]) as inventory, \
             mock.patch.object(route, 'receipt'), \
             mock.patch.object(route, 'revalidate_sources'), \
             mock.patch.object(route, 'run_bounded', side_effect=route.Refusal('failed')):
            with self.assertRaises(route.Refusal):
                route.guarded_run(Path('/owned'), {'tmp': Path('/owned/tmp'), 'work': Path('/owned/work')}, {}, 'probe', ['child'], {'commit': 'c', 'tree': 't'})
        self.assertEqual(inventory.call_count, 2)

    def test_unreadable_initial_inventory_never_runs_child(self):
        with mock.patch.object(route, 'budget', return_value=(512, 'ledger-digest')), \
             mock.patch.object(route, 'inventory_account', side_effect=OSError('unreadable')), \
             mock.patch.object(route, 'run_bounded') as child:
            with self.assertRaises(OSError):
                route.guarded_run(Path('/owned'), {'tmp': Path('/owned/tmp'), 'work': Path('/owned/work')}, {}, 'probe', ['child'], {'commit': 'c', 'tree': 't'})
        child.assert_not_called()

    def test_non_builds_context_refuses_before_source_or_child(self):
        with mock.patch.dict(os.environ, {'TAPE_RUN_ROOT': '/Users/tom'}, clear=True), \
             mock.patch.object(route, 'source_binding') as source, \
             mock.patch.object(route, 'run_bounded') as child:
            with self.assertRaises(route.Refusal):
                route.context()
        source.assert_not_called()
        child.assert_not_called()

    def test_wrong_branch_refuses_before_archive(self):
        with mock.patch.object(route, 'git', return_value=b'wrong-branch\n') as git:
            with self.assertRaises(route.Refusal):
                route.source_binding(Path('/repo'), Path('/source'))
        self.assertEqual(git.call_count, 1)

    def test_cache_symlink_refuses(self):
        metadata = mock.Mock(st_uid=os.getuid(), st_mode=0o120700)
        with mock.patch.object(route.os, 'walk', return_value=[('/owned/cache', [], ['link'])]), \
             mock.patch.object(route.os, 'lstat', return_value=metadata):
            with self.assertRaises(route.Refusal):
                route.private_cache_tree(Path('/owned/cache'))

    def test_escape_refused_before_lookup(self):
        with self.assertRaises(route.Refusal):
            route.private_components(Path('/owned'), Path('/outside'))


if __name__ == '__main__':
    unittest.main()
