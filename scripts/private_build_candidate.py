"""Capture consumer for the shared private-build candidate provider.

The planner-bound qualification and issuance are separate from the producer's
deliberately unqualified candidate receipt. This module is a source component,
not a provider qualification, and can be staged and hashed by another consumer.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import sys

_provider_path = Path(__file__).with_name('test-private-build-validation.py')
_spec = importlib.util.spec_from_file_location('private_build_provider', _provider_path)
provider = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(provider)

SELECTORS = ('rust:compile', 'rust:compile-release', 'rust:format', 'test rust')
QUALIFICATION_SCHEMA = 'oxide.private-build-validation.qualification/1'
ISSUANCE_SCHEMA = 'oxide.private-build-validation.capture-issuance/1'


def require(condition, reason):
    if not condition:
        raise provider.GateRefusal(reason)


def verify_documents(gate, candidate, qualification, issuance, digests, product_head, product_tree):
    """Validate actual candidate exports plus an independent positive judgment.

    `digests` are computed from the retained bytes, not supplied values. The
    positive judgment and fresh issuance must independently bind those bytes.
    """
    require(gate.get('schema') == provider.SCHEMA_GATE_RECEIPT
            and gate.get('qualification', {}).get('state') == 'unqualified'
            and gate.get('failures') == [], 'invalid producer gate receipt')
    require(candidate.get('schema') == provider.SCHEMA_CANDIDATE_RECEIPT
            and candidate.get('mode') == 'candidate'
            and candidate.get('qualification') == 'unqualified'
            and candidate.get('build', {}).get('state') == 'not-built',
            'candidate is absent, qualified in-band, or confused with bootstrap')
    exports = gate.get('exports', {})
    binding = candidate.get('binding', {})
    copy = candidate.get('copy', {})
    origin = candidate.get('origin', {})
    require(exports.get('candidate_receipt_sha256') == digests['candidate']
            and candidate.get('ruling') == provider.CANDIDATE_RULING
            and candidate.get('status_phase') == provider.STATUS_SELECTOR
            and exports.get('candidate_copy_sha256') == copy.get('sha256')
            and exports.get('candidate_copy_path') == copy.get('path')
            and binding.get('oxide_sha256') == copy.get('sha256')
            and origin.get('artifact', {}).get('sha256') == copy.get('sha256')
            and exports.get('candidate_origin_root') == origin.get('root')
            and exports.get('origin_guard_receipt_sha256') == origin.get('guard', {}).get('receipt_sha256')
            and exports.get('origin_build_log_sha256') == origin.get('guard', {}).get('log_sha256'),
            'candidate copy or retained origin does not bind to producer exports')
    require(binding.get('commit') == provider.OXIDE_PIN_COMMIT
            and binding.get('lock_sha256') == origin.get('lock_sha256')
            and binding.get('toolchain_sha256') == origin.get('toolchain_sha256')
            and origin.get('guard', {}).get('closure') == 'closed'
            and origin.get('guard', {}).get('status') == 0
            and origin.get('guard', {}).get('census_certain') is True
            and origin.get('guard', {}).get('census_members') == 0
            and origin.get('guard', {}).get('census_uncertain') == 0,
            'candidate source, lock or toolchain origin differs')
    live = candidate.get('live_inventory', {})
    require(live.get('before_sha256') == digests['live_before']
            and live.get('after_sha256') == digests['live_after']
            and digests['live_before'] == digests['live_after']
            and exports.get('live_before_sha256') == digests['live_before']
            and exports.get('live_after_sha256') == digests['live_after'],
            'producer live inventory binding differs')
    case_command = qualification.get('case_command')
    status_rows = [row for row in gate.get('commands', [])
                   if row.get('phase') == provider.STATUS_SELECTOR and row.get('exit') == 0]
    require(len(status_rows) == 1 and isinstance(case_command, list)
            and isinstance(status_rows[0].get('argv'), list)
            and '--' in status_rows[0]['argv']
            and status_rows[0]['argv'][status_rows[0]['argv'].index('--') + 1:] == case_command,
            'producer status case did not bind to the qualified guard')
    remaining = exports.get('remaining_allowance', {}).get('remaining_seconds')
    require(type(remaining) is int and 0 < remaining <= 1200
            and gate.get('allowance') == exports.get('remaining_allowance'),
            'producer allowance unavailable or inconsistent')
    require(qualification.get('schema') == QUALIFICATION_SCHEMA
            and qualification.get('state') == 'qualified'
            and isinstance(qualification.get('review_reference'), str)
            and bool(qualification['review_reference'].strip())
            and qualification.get('gate_sha256') == digests['gate']
            and qualification.get('candidate_sha256') == digests['candidate']
            and qualification.get('guard_sha256') == digests['guard']
            and qualification.get('case_guard_sha256') == digests['case_guard']
            and qualification.get('case_log_sha256') == digests['case_log']
            and qualification.get('product_head') == product_head
            and qualification.get('product_tree') == product_tree
            and qualification.get('producer_oxide_block_sha256') == gate.get('blocks', {}).get('oxide_block_sha256')
            and qualification.get('retained_roots') == issuance.get('retained_roots')
            and qualification.get('offline_cache_source') == issuance.get('offline_cache_source')
            and qualification.get('admission_env') == issuance.get('admission_env')
            and qualification.get('admission_audit') == issuance.get('admission_audit')
            and qualification.get('test_internal_root') == issuance.get('test_internal_root')
            and qualification.get('test_validation_tmp') == issuance.get('test_validation_tmp')
            and qualification.get('live_before_sha256') == digests['live_before']
            and qualification.get('live_after_sha256') == digests['live_after'],
            'independent positive qualification is missing or mismatched')
    require(issuance.get('schema') == ISSUANCE_SCHEMA
            and issuance.get('qualification_sha256') == digests['qualification']
            and issuance.get('product_head') == product_head
            and issuance.get('product_tree') == product_tree
            and issuance.get('candidate_sha256') == copy.get('sha256')
            and issuance.get('selectors') == list(SELECTORS)
            and type(issuance.get('max_seconds')) is int
            and 0 < issuance['max_seconds'] <= remaining
            and isinstance(issuance.get('planner_reference'), str)
            and bool(issuance['planner_reference'].strip())
            and isinstance(issuance.get('one_use_id'), str)
            and provider.HEX64.fullmatch(issuance['one_use_id']) is not None,
            'fresh capture issuance does not bind this product or allowance')
    return remaining


def _document(path, expected):
    path = provider.canonical_file(Path(path))
    require(provider.sha256_file(path) == expected, 'bound document changed')
    data = path.read_bytes()
    require(len(data) <= provider.MAX_DOC_BYTES
            and hashlib.sha256(data).hexdigest() == expected, 'bound document bytes changed')
    value = json.loads(data)
    require(provider.sha256_file(path) == expected, 'bound document changed during read')
    return value


def _text(path, expected):
    path = provider.canonical_file(Path(path))
    require(provider.sha256_file(path, cap=32 * 1024 * 1024) == expected,
            'bound case output changed')
    data = path.read_bytes()
    require(len(data) <= 32 * 1024 * 1024 and hashlib.sha256(data).hexdigest() == expected,
            'bound case output changed during read')
    return data.decode('utf-8', errors='strict')


def consume(options, *, execute=None):
    """Guard, verify and execute the four fixed selectors under one claim.

    `execute` is solely a test seam; production callers use the real shared
    supervisor. No expectation or fixture data enters the engine.
    """
    started_ns = provider.now_ns()
    evidence, run_id, deadline_ns, scratch = provider.supervisor_context()
    paths = options['paths']
    expected = options['digests']
    docs = {name: _document(paths[name], expected[name]) for name in
            ('gate', 'candidate', 'qualification', 'issuance', 'guard', 'case_guard', 'live_before', 'live_after')}
    require(isinstance(docs['live_before'], list) and docs['live_before']
            and docs['live_before'] == docs['live_after'], 'empty or changed live inventory')
    guard = docs['guard']
    require(isinstance(docs['qualification'], dict), 'qualification is not an object')
    require('--candidate' in docs['qualification']['guard_command']
            and any(Path(part).name == 'test-private-build-validation.py'
                    for part in docs['qualification']['guard_command']),
            'qualified guard did not run the candidate provider')
    provider.validate_supervisor_receipt(guard, docs['qualification']['guard_command'],
                                         Path(docs['qualification']['guard_scratch']))
    provider.validate_supervisor_receipt(docs['case_guard'], docs['qualification']['case_command'],
                                         Path(docs['qualification']['case_scratch']))
    require(docs['qualification']['case_command'][-1] == provider.STATUS_SELECTOR,
            'qualified case is not the provider status selector')
    producer_root = Path(docs['candidate']['copy']['path']).parent.parent
    probe_id = docs['qualification']['status_probe_id']
    require(isinstance(probe_id, str) and provider.BLOCK_ID.fullmatch(probe_id),
            'status sentinel identity is invalid')
    provider.verify_probe(_text(paths['case_log'], expected['case_log']),
        home=producer_root / 'home', config=producer_root / 'config.toml',
        pool=producer_root / 'scratch/pool', registry_probe=producer_root / f'registry-probe-{probe_id}',
        cache_probe=producer_root / 'home/Library/Caches/oxide/build-pool')
    repo = provider.repo_guard(options['product_repo'])
    head, tree = provider.extract_commit_identity(repo, options['product_head'])
    verify_documents(docs['gate'], docs['candidate'], docs['qualification'], docs['issuance'],
                     expected, head, tree)
    issuance = docs['issuance']
    require(provider.sha256_file(Path(__file__), cap=provider.MAX_DOC_BYTES) == issuance.get('consumer_sha256')
            and provider.sha256_file(_provider_path, cap=provider.MAX_DOC_BYTES) == issuance.get('provider_sha256'),
            'consumer source changed')
    for tool in ('effigy', 'rustc', 'cargo', 'supervisor', 'python', 'git'):
        path = provider.canonical_file(Path(issuance['tools'][tool]['path']))
        require(provider.sha256_file(path, cap=256 * 1024 * 1024) == issuance['tools'][tool]['sha256'],
                f'{tool} executable changed')
    require(issuance['tools']['supervisor']['sha256'] == provider.SUPERVISOR_SHA256,
            'supervisor bytes differ')
    require(provider.direct_effigy(Path(issuance['tools']['effigy']['path']))
            == Path(issuance['tools']['effigy']['path']), 'Effigy is not the direct audited executable')
    require(provider.audited_admission(dict(os.environ), issuance['admission_audit'])
            == issuance['admission_env'], 'host admission differs from the issued audit')
    require(issuance['tools']['effigy']['sha256'] == docs['gate']['bindings']['effigy']
            and issuance['tools']['git']['sha256'] == docs['gate']['bindings']['git']
            and issuance['tools']['rustc']['sha256'] == docs['candidate']['binding']['rustc_sha256']
            and issuance['tools']['cargo']['sha256'] == docs['candidate']['binding']['cargo_sha256'],
            'tool digests differ from qualified producer')
    require(Path(issuance['tools']['python']['path']) == Path(sys.executable).resolve(strict=True)
            and Path(issuance['tools']['git']['path']) == Path('/usr/bin/git').resolve(strict=True),
            'interpreter or Git executable differs')
    rustup = Path(issuance['rustup_home'])
    toolchain = Path(issuance['tools']['rustc']['path']).parent
    require(rustup.is_absolute() and rustup.resolve(strict=True) == rustup
            and toolchain.parent.parent == rustup / 'toolchains'
            and toolchain.parent.name.startswith('1.97.1-')
            and Path(issuance['tools']['cargo']['path']).parent == toolchain,
            'toolchain does not bind to the declared existing 1.97.1 installation')
    candidate_path = provider.canonical_file(Path(docs['candidate']['copy']['path']))
    require(provider.sha256_file(candidate_path, cap=256 * 1024 * 1024)
            == issuance['candidate_sha256'] and os.access(candidate_path, os.X_OK), 'candidate binary changed')
    require(type(issuance.get('scratch_mib')) is int and 1 <= issuance['scratch_mib'] <= 512
            and type(issuance.get('pool_cap_bytes')) is int
            and 0 < issuance['pool_cap_bytes'] <= provider.OXIDE_CAP_BYTES
            and type(issuance.get('expires_ns')) is int, 'issuance bounds invalid')
    require(provider.now_ns() < deadline_ns and issuance['expires_ns'] <= deadline_ns
            and provider.now_ns() < issuance['expires_ns'], 'issuance expired or exceeds guard')
    retained = [Path(value) for value in issuance['retained_roots']]
    require(retained and len(retained) == len(set(retained))
            and all(path.is_absolute() and path != scratch and not scratch.is_relative_to(path)
                             and not path.is_relative_to(scratch) for path in retained),
            'retained roots overlap the active scratch root')
    origin = docs['candidate']['origin']
    origin_options = {
        'candidate_lock_sha256': origin['lock_sha256'],
        'candidate_toolchain_sha256': origin['toolchain_sha256'],
        'candidate_receipt': origin['guard']['receipt_path'],
        'expect_candidate_receipt': origin['guard']['receipt_sha256'],
        'candidate_log': origin['guard']['log_path'],
        'expect_candidate_log': origin['guard']['log_sha256'],
        'expect_candidate': issuance['candidate_sha256'],
        'candidate_bytes': origin['artifact']['bytes'],
    }
    checked_origin = provider.validate_candidate_origin(origin_root=Path(origin['root']),
        options=origin_options, ledger_roots=retained,
        pinned_lock_sha256=docs['candidate']['binding']['lock_sha256'],
        pinned_toolchain_sha256=docs['candidate']['binding']['toolchain_sha256'])
    require(checked_origin == origin, 'retained origin facts changed')
    require(any(candidate_path == path or candidate_path.is_relative_to(path) for path in retained),
            'candidate copy is not in the retained-root ledger')
    # The provider's durable exclusive companion claim is the one-use control.
    provider.claim_issued_token(Path(paths['issuance']), expected['issuance'], run_id)
    if execute is not None:
        return execute(SELECTORS)
    allowance = provider.Allowance(issuance['max_seconds'], min(deadline_ns, issuance['expires_ns']),
                                   None, started_ns=started_ns)
    allowance.require(300, 'capture preparation')
    active_receipt = provider.canonical_file(evidence / 'receipt.json')
    active_digest = provider.sha256_file(active_receipt)
    guard_live = _document(active_receipt, active_digest)
    guard_budget = guard_live.get('owned_bytes', {}).get('budget')
    require(type(guard_budget) is int and 0 < guard_budget <= issuance['scratch_mib'] * 1024 * 1024,
            'active outer guard scratch bound differs')
    outside = provider.allocated_bytes(retained, provider.OXIDE_CAP_BYTES, deadline_ns)
    require(outside + guard_budget <= provider.OXIDE_CAP_BYTES,
            'retained bytes and outer guard exceed aggregate cap')
    before = provider.live_inventory(provider.LIVE_ACCOUNT_ROOT, deadline_ns)
    retained_before = provider.retained_snapshot(retained, provider.SCRATCH_ROOT, deadline_ns)
    try:
        return _execute_guarded(issuance, candidate_path, repo, head, tree, scratch, evidence,
                                min(deadline_ns, issuance['expires_ns']), allowance)
    finally:
        after = provider.live_inventory(provider.LIVE_ACCOUNT_ROOT, deadline_ns)
        provider.verify_inventory(before, after, 'capture-covering', evidence)
        require(retained_before == provider.retained_snapshot(retained, provider.SCRATCH_ROOT, deadline_ns),
                'retained roots changed during covering')


def _execute_guarded(issuance, candidate, repo, head, tree, scratch, evidence, deadline_ns, allowance):
    """Stage the exact source and seed its private Cargo home, then run once."""
    root = provider.private_dir(scratch, Path('capture-covering'))
    source = provider.private_dir(root, Path('source'))
    files = provider.stage_git_tree(repo, source, head, tree)
    require(files.get('Cargo.lock') == issuance['product_lock_sha256']
            and files.get('rust-toolchain.toml') == issuance['product_toolchain_sha256'],
            'staged lock or toolchain differs')
    cache = provider.private_dir(root, Path('cache'))
    for name in ('home', 'tmp', 'scratch', 'bin'):
        provider.private_dir(root, Path(name))
    provider.private_write(root, Path('config.toml'), b'')
    sys.path.insert(0, str(source / 'scripts'))
    try:
        from offline_seed import seed_cargo_home
        seed_cargo_home(Path(issuance['offline_cache_source']), cache,
                        (source / 'Cargo.lock').read_bytes(), deadline_ns=deadline_ns)
    finally:
        sys.path.pop(0)
    tools = issuance['tools']
    os.symlink(candidate, root / 'bin/oxide')
    inherited = {'RUSTUP_HOME': issuance['rustup_home'], 'RUSTUP_TOOLCHAIN': '1.97.1',
                 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin'}
    env = provider.child_environment(inherited, private=root, oxide=candidate,
        rustc=Path(tools['rustc']['path']), wrapper_hash=issuance['consumer_sha256'],
        pool_cap=issuance['pool_cap_bytes'], admission_env=issuance['admission_env'])
    env['PATH'] = str(root / 'bin') + os.pathsep + str(Path(tools['rustc']['path']).parent) + ':/usr/bin:/bin:/usr/sbin:/sbin'
    retained = [Path(value) for value in issuance['retained_roots']]
    require(retained and all(path != scratch and not scratch.is_relative_to(path)
                             and not path.is_relative_to(scratch) for path in retained),
            'retained roots overlap the active scratch root')
    internal = Path(issuance['test_internal_root'])
    validation_tmp = Path(issuance['test_validation_tmp'])
    require(internal.is_absolute() and internal.is_relative_to(Path('/private/tmp'))
            and internal != Path('/private/tmp') and internal.resolve(strict=True) == internal
            and validation_tmp.is_absolute()
            and validation_tmp.is_relative_to(Path('/Volumes/Builds/scratch/oxide-validation'))
            and validation_tmp != Path('/Volumes/Builds/scratch/oxide-validation')
            and validation_tmp.resolve(strict=True) == validation_tmp,
            'covering suite roots are outside their approved locations')
    for directory in (internal, validation_tmp):
        st = os.lstat(directory)
        require(stat.S_ISDIR(st.st_mode) and st.st_uid == os.geteuid()
                and not st.st_mode & 0o077, 'suite root is not owner-only')
    for index, selector in enumerate(SELECTORS):
        allowance.require(300, selector)
        before = provider.live_inventory(provider.LIVE_ACCOUNT_ROOT, deadline_ns)
        retained_before = provider.retained_snapshot(retained, provider.SCRATCH_ROOT, deadline_ns)
        command = [tools['effigy']['path'], *selector.split()]
        child_evidence = provider.private_dir(evidence, Path(f'capture-{index}'))
        child_env = dict(env)
        child_scratch = root
        if selector == 'test rust':
            child_env.update(OXIDE_TASK_SCRATCH_ROOT=str(internal), OXIDE_TEST_INTERNAL_ROOT=str(internal),
                             TMPDIR=str(validation_tmp), TMP=str(validation_tmp), TEMP=str(validation_tmp))
            child_scratch = internal
        try:
            receipt, _, _ = provider.run_supervised(supervisor=Path(tools['supervisor']['path']),
                python=Path(sys.executable), command=command, cwd=source, env=child_env,
                limit_seconds=300, scratch_mib=issuance['scratch_mib'], evidence=child_evidence,
                allowance=allowance, phase=f'capture-{index}')
            provider.validate_supervisor_receipt(receipt, command, child_scratch)
        finally:
            after = provider.live_inventory(provider.LIVE_ACCOUNT_ROOT, deadline_ns)
            provider.verify_inventory(before, after, selector, evidence)
            require(retained_before == provider.retained_snapshot(retained, provider.SCRATCH_ROOT, deadline_ns),
                    'retained roots changed during covering selector')
    return 0
