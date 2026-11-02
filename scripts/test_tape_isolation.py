#!/usr/bin/env python3
"""Source-level regression cases for the task-only private launcher.

These mock the live inventory and supervised child; no live account tree,
fixture, compiler, or subprocess is touched by this test module.
"""
import importlib.util
import hashlib
import json
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
    def test_missing_block_binding_refuses_before_child(self):
        with mock.patch.object(route, 'private_file', side_effect=lambda path, root: path), \
             mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(route.Refusal):
                route.block_inputs(Path('/owned'), {'commit': 'c', 'tree': 't'})

    def test_block_binds_source_history_ledger_and_unique_id(self):
        root = Path('/owned')
        binding = {'commit': 'c', 'tree': 't', 'files': {'launcher': 'digest'}}
        binding_sha = hashlib.sha256(json.dumps(binding, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        ledger = {'retained_roots': ['/owned']}
        history = {'task': route.TASK_ID, 'complete': True, 'retained_roots': ['/owned']}
        block = {'task': route.TASK_ID, 'root': '/owned', 'planner_reference': route.PLANNER_AUTHORITY,
                 'original_admission_charge_seconds': 1200, 'prospective_max_seconds': 1200,
                 'source_commit': 'c', 'source_tree': 't', 'binding_sha256': binding_sha,
                 'ledger_sha256': 'ledger', 'history_sha256': 'history',
                 'one_use_id': 'a' * 64, 'retained_roots_quiescent': True}
        content = {'budget.json': ledger, 'history-discovery.json': history,
                   'block-authorization.json': block}
        def read(path, *args, **kwargs):
            return json.dumps(content[path.name])
        def digest(path):
            return {'budget.json': 'ledger', 'history-discovery.json': 'history',
                    'block-authorization.json': 'b' * 64}[path.name]
        with mock.patch.object(route, 'private_file', side_effect=lambda path, root: path), \
             mock.patch.object(Path, 'read_text', read), mock.patch.object(route, 'digest', side_effect=digest), \
             mock.patch.dict(os.environ, {'TAPE_BLOCK_APPROVAL_SHA256': 'b' * 64}, clear=True):
            self.assertEqual(route.block_inputs(root, binding)[0]['one_use_id'], 'a' * 64)
            history['complete'] = False
            with self.assertRaises(route.Refusal):
                route.block_inputs(root, binding)
            history['complete'] = True
            block['source_tree'] = 'different'
            with self.assertRaises(route.Refusal):
                route.block_inputs(root, binding)

    def invoke(self, inventories, outcome=None):
        saved = []
        inventories = [[{'path': '.'}, {'path': 'state/pools.json', **row}]
                       for [row] in inventories]
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
        inventory_rows = [{'path': '.'}, {'path': 'state/pools.json', 'ino': 1}]
        with mock.patch.object(route, 'budget', return_value=(512, 'ledger-digest')), \
             mock.patch.object(route, 'inventory_account', side_effect=[inventory_rows, inventory_rows]) as inventory, \
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

    def test_empty_inventory_never_runs_child(self):
        with mock.patch.object(route, 'budget', return_value=(512, 'ledger-digest')), \
             mock.patch.object(route, 'inventory_account', return_value=[]), \
             mock.patch.object(route, 'run_bounded') as child:
            with self.assertRaises(route.Refusal):
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
