#!/usr/bin/env python3
"""Mock-only controls for the canonical capture candidate consumer."""
import copy
import importlib.util
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.dont_write_bytecode = True
import private_build_candidate as consumer

spec = importlib.util.spec_from_file_location('tape_handoff', Path(__file__).with_name('tape-provider-handoff.py'))
handoff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(handoff)
provider = consumer.provider


def sample():
    digests = {name: (str(index) * 64) for index, name in enumerate(
        ('gate', 'candidate', 'qualification', 'issuance', 'guard', 'case_guard', 'live_before', 'live_after'), 1)}
    digests['live_after'] = digests['live_before']
    digests['case_log'] = '9' * 64
    artifact = 'a' * 64
    origin = {'root': '/old', 'artifact': {'sha256': artifact, 'bytes': 10},
              'lock_sha256': 'lock', 'toolchain_sha256': 'toolchain',
              'guard': {'closure': 'closed', 'status': 0, 'census_certain': True,
                        'census_members': 0, 'census_uncertain': 0,
                        'receipt_sha256': 'origin-guard', 'log_sha256': 'origin-log',
                        'receipt_path': '/origin-receipt', 'log_path': '/origin-log'}}
    candidate = {'schema': provider.SCHEMA_CANDIDATE_RECEIPT, 'mode': 'candidate',
                 'qualification': 'unqualified', 'ruling': provider.CANDIDATE_RULING,
                 'status_phase': provider.STATUS_SELECTOR, 'build': {'state': 'not-built'},
                 'origin': origin, 'copy': {'path': '/old/candidate', 'sha256': artifact},
                 'binding': {'commit': provider.OXIDE_PIN_COMMIT, 'lock_sha256': 'lock',
                             'toolchain_sha256': 'toolchain', 'oxide_sha256': artifact,
                             'rustc_sha256': 'rustc', 'cargo_sha256': 'cargo'},
                 'live_inventory': {'before_sha256': digests['live_before'],
                                    'after_sha256': digests['live_after']}}
    guard_command = ['python', 'test-private-build-validation.py', '--candidate']
    case_command = ['effigy', provider.STATUS_SELECTOR]
    allowance = {'remaining_seconds': 1000}
    gate = {'schema': provider.SCHEMA_GATE_RECEIPT, 'qualification': {'state': 'unqualified'},
            'failures': [], 'blocks': {'oxide_block_sha256': 'prior-block'},
            'bindings': {'effigy': 'effigy', 'git': 'git'}, 'allowance': allowance,
            'commands': [{'phase': provider.STATUS_SELECTOR, 'exit': 0,
                          'argv': ['python', 'bounded-test.py', '--', *case_command]}],
            'exports': {'candidate_receipt_sha256': digests['candidate'],
                        'candidate_copy_path': '/old/candidate', 'candidate_copy_sha256': artifact,
                        'candidate_origin_root': '/old', 'origin_guard_receipt_sha256': 'origin-guard',
                        'origin_build_log_sha256': 'origin-log',
                        'live_before_sha256': digests['live_before'],
                        'live_after_sha256': digests['live_after'],
                        'remaining_allowance': allowance}}
    def guard(command):
        return {'command': command, 'scratch': '/old',
                'outcome': {'closure': 'closed', 'status': 0, 'fault': None,
                            'deadline': False, 'launched': True},
                'census': {'certain': True, 'members': 0, 'uncertain': 0}}
    qualification = {'schema': consumer.QUALIFICATION_SCHEMA, 'state': 'qualified',
        'review_reference': 'planner-review', 'gate_sha256': digests['gate'],
        'candidate_sha256': digests['candidate'], 'guard_sha256': digests['guard'],
        'case_guard_sha256': digests['case_guard'], 'guard_command': guard_command,
        'case_log_sha256': digests['case_log'], 'status_probe_id': 'fixture',
        'guard_scratch': '/old', 'case_command': case_command, 'case_scratch': '/old',
        'product_head': '1' * 40, 'product_tree': '2' * 40,
        'producer_oxide_block_sha256': 'prior-block',
        'retained_roots': ['/old'],
        'offline_cache_source': '/old/cache',
        'admission_env': {'EFFIGY_ADMISSION_HOOK': '/hook', 'QUEUE_ADMISSION_SOCKET': '/socket'},
        'admission_audit': {'example': 'audit'},
        'test_internal_root': '/private/tmp/fixture',
        'test_validation_tmp': '/Volumes/Builds/scratch/oxide-validation/fixture',
        'live_before_sha256': digests['live_before'], 'live_after_sha256': digests['live_after']}
    tools = {name: {'path': '/tools/' + name, 'sha256': name} for name in
             ('effigy', 'rustc', 'cargo', 'supervisor', 'python', 'git')}
    tools['supervisor']['sha256'] = provider.SUPERVISOR_SHA256
    tools['python']['path'] = sys.executable
    tools['git']['path'] = '/usr/bin/git'
    tools['rustc']['path'] = '/rustup/toolchains/1.97.1-test/bin/rustc'
    tools['cargo']['path'] = '/rustup/toolchains/1.97.1-test/bin/cargo'
    issuance = {'schema': consumer.ISSUANCE_SCHEMA, 'qualification_sha256': digests['qualification'],
        'product_head': '1' * 40, 'product_tree': '2' * 40, 'candidate_sha256': artifact,
        'selectors': list(consumer.SELECTORS), 'max_seconds': 600, 'planner_reference': 'issued',
        'one_use_id': 'b' * 64, 'consumer_sha256': 'consumer', 'provider_sha256': 'provider',
        'tools': tools, 'rustup_home': '/rustup', 'scratch_mib': 512, 'pool_cap_bytes': 1,
        'expires_ns': 900, 'retained_roots': ['/old'], 'offline_cache_source': '/old/cache',
        'admission_env': qualification['admission_env'],
        'admission_audit': qualification['admission_audit'],
        'test_internal_root': qualification['test_internal_root'],
        'test_validation_tmp': qualification['test_validation_tmp']}
    docs = {'gate': gate, 'candidate': candidate, 'qualification': qualification, 'issuance': issuance,
            'guard': guard(guard_command), 'case_guard': guard(case_command),
            'live_before': [['state/pools.json']], 'live_after': [['state/pools.json']]}
    return docs, digests


class CandidateTest(unittest.TestCase):
    def test_missing_and_unguarded_entry_refuse(self):
        with mock.patch.object(handoff.consumer.provider, 'supervisor_context', side_effect=provider.GateRefusal('unguarded')):
            with self.assertRaisesRegex(SystemExit, 'missing'):
                handoff.handoff([])
            with self.assertRaises(provider.GateRefusal):
                handoff.handoff(['--input', '/input', '--sha256', 'a' * 64])

    def test_valid_bound_documents_and_refusals(self):
        docs, digests = sample()
        def check(d):
            return consumer.verify_documents(d['gate'], d['candidate'], d['qualification'],
                                             d['issuance'], digests, '1' * 40, '2' * 40)
        self.assertEqual(check(docs), 1000)
        for name, section, key, value in (
            ('unqualified', 'qualification', 'state', 'unqualified'),
            ('bootstrap confusion', 'candidate', 'mode', 'bootstrap'),
            ('wrong head', 'issuance', 'product_head', '3' * 40),
            ('depleted', 'issuance', 'max_seconds', 0),
            ('changed receipt', 'qualification', 'candidate_sha256', 'different'),
            ('wrong producer ruling', 'candidate', 'ruling', 'other'),
            ('missing qualification', 'qualification', 'review_reference', ''),
        ):
            changed = copy.deepcopy(docs)
            changed[section][key] = value
            with self.subTest(name=name), self.assertRaises(provider.GateRefusal):
                check(changed)

    def test_actual_consumer_reaches_only_named_mock_child(self):
        docs, digests = sample()
        options = {'paths': {**{key: key for key in docs}, 'case_log': 'case_log'}, 'digests': digests,
                   'product_repo': '/repo', 'product_head': '1' * 40}
        tool_hashes = {value['path']: value['sha256'] for value in docs['issuance']['tools'].values()}
        def sha(path, **_):
            if path == Path(consumer.__file__): return 'consumer'
            if path == consumer._provider_path: return 'provider'
            if str(path) == '/old/candidate': return 'a' * 64
            return tool_hashes[str(path)]
        child = mock.Mock(return_value='mock-only')
        with mock.patch.object(consumer, '_document', side_effect=lambda path, _: docs[path]), \
             mock.patch.object(consumer, '_text', return_value='mock-status'), \
             mock.patch.object(provider, 'verify_probe') as verify_probe, \
             mock.patch.object(provider, 'supervisor_context', return_value=(Path('/evidence'), 'run', 1000, Path('/scratch'))), \
             mock.patch.object(provider, 'repo_guard', return_value=Path('/repo')), \
             mock.patch.object(provider, 'extract_commit_identity', return_value=('1' * 40, '2' * 40)), \
             mock.patch.object(provider, 'canonical_file', side_effect=lambda p: p), \
             mock.patch.object(provider, 'direct_effigy', side_effect=lambda p: p), \
             mock.patch.object(provider, 'audited_admission', return_value=docs['issuance']['admission_env']), \
             mock.patch.object(provider, 'sha256_file', side_effect=sha), \
             mock.patch.object(provider, 'validate_candidate_origin', return_value=docs['candidate']['origin']), \
             mock.patch.object(provider, 'claim_issued_token') as claim, \
             mock.patch.object(provider, 'now_ns', return_value=1), \
             mock.patch.object(Path, 'resolve', lambda self, **_: self), \
             mock.patch.object(consumer.os, 'access', return_value=True):
            self.assertEqual(consumer.consume(options, execute=child), 'mock-only')
            child.assert_called_once_with(consumer.SELECTORS)
            verify_probe.assert_called_once()
            claim.assert_called_once()
            child.reset_mock()
            claim.side_effect = provider.GateRefusal('replayed')
            with self.assertRaises(provider.GateRefusal):
                consumer.consume(options, execute=child)
            child.assert_not_called()
            claim.side_effect = None
            docs['issuance']['tools']['rustc']['sha256'] = 'wrong'
            with self.assertRaises(provider.GateRefusal):
                consumer.consume(options, execute=child)
            child.assert_not_called()
            docs['issuance']['tools']['rustc']['sha256'] = 'rustc'
            docs['candidate']['origin']['root'] = '/different'
            with self.assertRaises(provider.GateRefusal):
                consumer.consume(options, execute=child)
            child.assert_not_called()
            docs['candidate']['origin']['root'] = '/old'
            docs['issuance']['expires_ns'] = 1
            with self.assertRaises(provider.GateRefusal):
                consumer.consume(options, execute=child)
            child.assert_not_called()


if __name__ == '__main__':
    unittest.main()
