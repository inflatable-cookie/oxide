#!/usr/bin/env python3
"""S3 external scratch-pool proof, supervised by bounded-test.py.

Premise checklist (workflow.md):
1. toolchain provenance is resolved before private HOME; pinned offline staging;
   fixture lock is written before --locked calls (setup).
2. OXIDE_TEST_HOME_FROM_ENV=1 on every product child (setup).
3. fixture environment is built from a small allowlist; OXIDE/XDG/GIT/CARGO
   inheritance is not copied (setup).
4. automatic adoption is not an oracle; fixture outputs remain in private root.
5. no Cargo external subcommand is invoked.
6. both owned images attach with -owners on; custody and detach are in Images.
7. staging and fixture children use the same declared RUSTFLAGS; encoded flags
   are removed (setup).
8. all temporary files are in the qualified supervisor's scratch; images are
   in its evidence; the selector does not override default routing.
9. tempfile.mkdtemp uses an explicit dir under the published scratch.

Cases are ordered and share state. Diagnostic records each outcome and skips
all dependent cases after a fault; unknown image custody always aborts. The
supervisor owns deadline, process census, reserve monitoring and final detach.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parent.parent


def plan():
    print("S3 native: 01-10 except 04-internal and 06-inert, plus 12-15; "
          "two sequential 128 MiB owned APFS images; at most one attached; "
          "300 s; 512 MiB; private fixture account; case 11 is unit-proved")


def self_check():
    custody = custody_document('/owned/A', '/owned/B')
    assert custody == {'version': 1, 'backings': ['/owned/A', '/owned/B']}
    assert may_detach(True, False) and not may_detach(False, False)
    assert not may_detach(True, True)
    assert detached(True, []) and not detached(False, []) and not detached(True, ['/dev/disk1'])
    assert may_attach(False) and not may_attach(True)
    import inspect
    create_source = inspect.getsource(Images.create)
    attach_source = inspect.getsource(Images.attach)
    detach_source = inspect.getsource(Images.detach)
    assert create_source.index('image-custody.json') < create_source.index('hdiutil')
    assert 'may_attach(self.attached is not None)' in attach_source
    assert detach_source.index('self.request.unlink()') < detach_source.index('may_detach(') < detach_source.index("'detach'")
    assert 'detached(not os.path.ismount' in detach_source
    assert 'attachment entities unreadable' in detach_source
    print('scratch-pool proof self-check: custody, child closure, request withdrawal, post-state and exclusive attachment OK')
    return 0


def custody_document(a, b):
    return {'version': 1, 'backings': [a, b]}


def may_detach(children_reaped, request_present):
    return children_reaped and not request_present


def detached(mount_gone, entities):
    return mount_gone and not entities


def may_attach(attached):
    return not attached


def atomic(path, value):
    tmp = path.with_name(path.name + '.tmp')
    with tmp.open('x') as f:
        json.dump(value, f, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def remaining():
    left = (int(os.environ['OXIDE_PROOF_DEADLINE_NS']) - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - 20
    require(left > 1, 'work window exhausted')
    return left


def run(argv, **kwargs):
    return subprocess.run([str(x) for x in argv], stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=remaining(), **kwargs)


def tree(path):
    if not path.exists():
        return None
    result = hashlib.sha256()
    for root, dirs, files in os.walk(path, followlinks=False):
        dirs.sort(); files.sort()
        for name in dirs + files:
            p = Path(root) / name
            result.update(str(p.relative_to(path)).encode())
            if p.is_symlink(): result.update(os.readlink(p).encode())
            elif p.is_file(): result.update(p.read_bytes())
    return result.hexdigest()


class Images:
    def __init__(self, evidence):
        self.evidence = evidence
        self.a = evidence / 'scratch-A.sparseimage'
        self.b = evidence / 'scratch-B.sparseimage'
        self.attached = None
        self.request = evidence / 'detach-request.json'
        self.name = 'OxS3' + str(os.getpid())
        self.mount = Path('/Volumes') / self.name
        self.children_reaped = True
        self.observations = []

    def create(self):
        atomic(self.evidence / 'image-custody.json', custody_document(str(self.a), str(self.b)))
        for image in (self.a, self.b):
            p = run(['/usr/bin/hdiutil', 'create', '-size', '128m', '-fs', 'APFS',
                     '-volname', self.name, '-type', 'SPARSE', '-quiet', image])
            require(p.returncode == 0, 'image creation failed')

    def attach(self, image):
        require(may_attach(self.attached is not None), 'another image is attached')
        p = run(['/usr/bin/hdiutil', 'attach', '-owners', 'on', '-nobrowse', '-plist', image])
        require(p.returncode == 0, 'attach failed; custody retained')
        # If parsing fails the custody marker still forces the guard to keep both.
        self.attached = {'backing': str(image)}
        entities = plistlib.loads(p.stdout)['system-entities']
        mounted = [e for e in entities if e.get('mount-point')]
        require(len(mounted) == 1 and mounted[0]['mount-point'] == str(self.mount), 'unexpected mount')
        device = mounted[0]['dev-entry']
        info = run(['/usr/sbin/diskutil', 'info', '-plist', self.mount])
        require(info.returncode == 0, 'volume identity unreadable')
        uuid = plistlib.loads(info.stdout)['VolumeUUID']
        self.attached = {'version': 1, 'backing': str(image), 'mount': str(self.mount),
                         'device': device, 'volume_uuid': uuid}
        atomic(self.request, self.attached)
        self.observations.append(dict(self.attached))
        print('mounted', self.mount, device, uuid, flush=True)
        return uuid

    def detach(self):
        require(self.attached is not None, 'no attachment')
        observation = run(['/usr/sbin/diskutil', 'info', '-plist', self.attached['mount']])
        require(observation.returncode == 0, 'pre-detach identity unreadable')
        facts = plistlib.loads(observation.stdout)
        require(facts.get('VolumeUUID') == self.attached['volume_uuid'] and
                facts.get('DeviceNode') == self.attached['device'], 'pre-detach identity changed')
        self.request.unlink()
        atomic(self.evidence / f"detach-attempt-{len(self.observations)}.json", self.attached)
        require(may_detach(self.children_reaped, self.request.exists()), 'detach precondition')
        p = subprocess.run(['/usr/bin/hdiutil', 'detach', self.attached['device']],
                           stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           timeout=min(30, remaining()))
        require(p.returncode == 0, 'detach failed; both backings retained')
        info = run(['/usr/bin/hdiutil', 'info', '-plist'])
        require(info.returncode == 0, 'attachment post-state unreadable')
        images = plistlib.loads(info.stdout)['images']
        require(isinstance(images, list), 'attachment list malformed')
        entities = []
        for image in images:
            require(isinstance(image, dict) and isinstance(image.get('image-path'), str), 'attachment entry malformed')
            if image['image-path'] == self.attached['backing']:
                rows = image.get('system-entities')
                require(isinstance(rows, list) and rows and all(isinstance(e, dict) and isinstance(e.get('dev-entry'), str) for e in rows), 'attachment entities unreadable')
                entities.extend(rows)
        require(detached(not os.path.ismount(self.mount), entities), 'detach post-state unknown')
        self.attached = None

NATIVE_CASES = ('01', '02', '03', '04', '05', '06', '07', '08', '09',
                '10', '12', '13', '14', '15')


def fixture():
    spec = importlib.util.spec_from_file_location('pool_temp_proof', REPO / 'scripts/test-pool-temp-scratch.py')
    helper = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = helper
    spec.loader.exec_module(helper)
    toolchain = helper.resolve_toolchain_home(os.environ)
    scratch = Path(os.environ['OXIDE_TASK_SCRATCH_ROOT']).resolve(strict=True)
    evidence = Path(os.environ['OXIDE_PROOF_EVIDENCE']).resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix='s3.', dir=scratch))
    staging_env = {k: v for k, v in os.environ.items()
                   if not k.startswith(('OXIDE_', 'XDG_', 'GIT_', 'CARGO_', 'SCCACHE_'))
                   and k not in ('RUSTFLAGS', 'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER')}
    staging_env.update(RUSTUP_HOME=toolchain, RUSTUP_TOOLCHAIN='1.97.1', RUSTUP_AUTO_INSTALL='0',
                       CARGO_NET_OFFLINE='true', CARGO_HOME=os.environ.get('CARGO_HOME', str(Path(os.environ['HOME']) / '.cargo')),
                       CARGO_TARGET_DIR=str(root / 'target'), CARGO_INCREMENTAL='0',
                       CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_DEV_OPT_LEVEL='s',
                       CARGO_PROFILE_DEV_CODEGEN_UNITS='1', CARGO_PROFILE_DEV_PANIC='abort',
                       RUSTFLAGS='-C strip=symbols')
    built = run(['cargo', 'build', '-j', '1', '--locked', '--offline', '--bin', 'oxide'],
                cwd=REPO, env=staging_env)
    require(built.returncode == 0, 'staging failed: ' + built.stderr.decode(errors='replace')[-2000:])
    subject = root / 'target/debug/oxide'
    require(subject.is_file(), 'staged binary missing')
    home, cargo, checkout, tmp = (root / n for n in ('home', 'cargo', 'checkout', 'tmp'))
    for path in (home, cargo, checkout, tmp): path.mkdir(mode=0o700)
    (checkout / 'src').mkdir()
    (checkout / 'Cargo.toml').write_text('[package]\nname="scratch-proof"\nversion="0.1.0"\nedition="2021"\n')
    (checkout / 'Cargo.lock').write_text('# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = "scratch-proof"\nversion = "0.1.0"\n')
    (checkout / 'src/lib.rs').write_text('pub fn proof() -> u8 { 1 }\n')
    env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': str(home),
           'CARGO_HOME': str(cargo), 'RUSTUP_HOME': toolchain, 'RUSTUP_TOOLCHAIN': '1.97.1',
           'RUSTUP_AUTO_INSTALL': '0', 'CARGO_NET_OFFLINE': 'true',
           'RUSTFLAGS': '-C strip=symbols', 'OXIDE_TEST_HOME_FROM_ENV': '1',
           'OXIDE_POOL_CREATE': '1', 'OXIDE_POOL_CAP': '64MiB',
           'TMPDIR': str(tmp), 'TMP': str(tmp), 'TEMP': str(tmp),
           'OXIDE_CONFIG': str(root / 'config.toml')}
    return root, evidence, subject, checkout, env


def product(subject, checkout, env, *args, ok=None, contains=None):
    p = run([subject, *args], cwd=checkout, env=env)
    text = (p.stdout + p.stderr).decode(errors='replace')
    if ok is not None: require((p.returncode == 0) == ok, f'{args}: exit {p.returncode}: {text[-2000:]}')
    if contains is not None: require(contains in text, f'{args}: missing {contains!r}: {text[-2000:]}')
    return text


def host_record(home, pool):
    # macOS account-state root, same hash as the product's normalized pool path.
    digest = hashlib.sha256(os.fsencode(pool)).hexdigest()
    return home / 'Library/Application Support/oxide/state/scratch-pools' / (digest + '.json')


def proof(diagnostic):
    outcomes, first = [], None
    world = {}
    try:
        root, evidence, subject, checkout, env = fixture()
        images = Images(evidence)
        images.create()
        uuid_a = images.attach(images.a)
        mount = images.mount
        scratch = mount / 'scratch-root'
        scratch.mkdir(mode=0o700)
        pool = scratch / 'pool'
        env.update(OXIDE_SCRATCH_ROOT=str(scratch), OXIDE_POOL=str(pool))
        record = host_record(Path(env['HOME']), pool)
        binding = pool / 'oxide-managed/scratch.json'
        check = lambda e=env: product(subject, checkout, e, 'check', '--offline', '--locked', ok=True)
        def refuse(e, cls): product(subject, checkout, e, 'check', '--offline', '--locked', ok=False, contains=cls)
        def changed(**kw):
            result = dict(env)
            for k, v in kw.items():
                if v is None: result.pop(k, None)
                else: result[k] = str(v)
            return result
        def case01():
            require(not pool.exists(), 'pool already exists')
            check(); require(binding.is_file() and record.is_file() and (pool / 'config.json').is_file(), 'creation incomplete')
            require(json.loads(binding.read_text())['volume_uuid'] == uuid_a, 'binding UUID')
        def case02(): check()
        def case03():
            before = (tree(pool), record.read_bytes())
            refuse(changed(OXIDE_SCRATCH_ROOT=None), 'register-removable')
            require(before == (tree(pool), record.read_bytes()), 'ordinary refusal changed pool')
        def case04():
            for path, expected in ((scratch, 'scratch-path-outside-root'),
                                   (mount / 'scratch-root-other/pool', 'scratch-path-outside-root'),
                                   (scratch / 'missing/pool', 'scratch-parent-missing')):
                refuse(changed(OXIDE_POOL=path), expected)
                if path != scratch: require(not path.exists(), 'refused path was created')
            git = scratch / 'git'; git.mkdir(); (git / '.git').mkdir()
            refuse(changed(OXIDE_POOL=git / 'pool'), 'Git work tree')
        def case05(): refuse(changed(OXIDE_POOL=scratch / 'no-budget', OXIDE_POOL_CAP=None), 'scratch-budget-required')
        def case06():
            link = mount / 'root-link'; link.symlink_to(scratch)
            refuse(changed(OXIDE_SCRATCH_ROOT=link, OXIDE_POOL=link / 'pool2'), 'scratch-root-unsafe')
            scratch.chmod(0o770)
            try: refuse(changed(OXIDE_POOL=scratch / 'writable'), 'scratch-root-unsafe')
            finally: scratch.chmod(0o700)
            refuse(changed(OXIDE_SCRATCH_ROOT=mount, OXIDE_POOL=mount / 'direct'), 'scratch-root-unsafe')
        def case07():
            before = record.read_bytes(); images.detach()
            require(not mount.exists(), 'mount point remained after detach')
            walk = root / 'status-walk.log'
            text = product(subject, checkout, changed(OXIDE_TEST_WALK_LOG=walk), 'status', ok=True)
            require(not walk.exists() or not walk.read_bytes(), 'status walked the absent pool')
            require('unavailable' in text and uuid_a in text, 'missing absent status')
            refuse(env, uuid_a)
            require(not mount.exists() and record.read_bytes() == before, 'absent path or record changed')
        def case08():
            require(images.attach(images.a) == uuid_a, 'A UUID changed')
            require(images.mount == mount, 'mount changed'); check()
        def case09():
            images.detach(); uuid_b = images.attach(images.b)
            require(uuid_b != uuid_a and images.mount == mount, 'B identity or mount invalid')
            (mount / 'scratch-root').mkdir(mode=0o700)
            refuse(env, 'scratch-record-mismatch')
            images.detach(); require(images.attach(images.a) == uuid_a, 'A identity changed')
            world['uuid_b'] = uuid_b
        def case10():
            original = binding.read_bytes(); value = json.loads(original); value['volume_uuid'] = world['uuid_b']
            binding.write_text(json.dumps(value)); before = tree(pool)
            try:
                refuse(env, 'scratch-binding-mismatch')
                require(tree(pool) == before, 'mismatch changed pool')
            finally: binding.write_bytes(original)
        def case12():
            record.unlink(); before = tree(pool)
            walk = root / 'status-repair-walk.log'
            text = product(subject, checkout, changed(OXIDE_TEST_WALK_LOG=walk), 'status', ok=True)
            require(not walk.exists() or not walk.read_bytes(), 'status walked pool')
            require('scratch pool, volume' not in text and not record.exists() and tree(pool) == before, 'status repaired record')
            check(); require(record.is_file(), 'leased open failed to repair')
            require(uuid_a in product(subject, checkout, env, 'status', ok=True), 'missing scratch label')
        def case13():
            original = record.read_bytes(); value = json.loads(original); value['volume_uuid'] = world['uuid_b']
            record.write_text(json.dumps(value)); before = (tree(pool), record.read_bytes())
            try:
                refuse(env, 'scratch-record-mismatch'); require(before == (tree(pool), record.read_bytes()), 'mismatch changed bytes')
            finally: record.write_bytes(original)
        def case14():
            original = record.read_bytes()
            try:
                record.chmod(0)
                refuse(env, 'scratch-record-invalid')
                record.chmod(0o600)
                for content in (b'{', b' ' * 4097):
                    record.write_bytes(content); refuse(env, 'scratch-record-invalid')
                record.unlink(); record.symlink_to(root / 'missing')
                refuse(env, 'scratch-record-invalid')
            finally:
                record.unlink(missing_ok=True); record.write_bytes(original); record.chmod(0o600)
        def case15():
            record.unlink(); images.detach()
            refuse(env, 'no host record'); require(not mount.exists() and not record.exists(), 'absent pool created')
        cases = (case01, case02, case03, case04, case05, case06, case07,
                 case08, case09, case10, case12, case13, case14, case15)
        dependencies = {'02': ('01',), '03': ('02',), '07': ('03',),
                        '08': ('07',), '09': ('08',), '10': ('09',),
                        '12': ('10',), '13': ('12',), '14': ('13',), '15': ('14',)}
        completed = set()
        for name, case in zip(NATIVE_CASES, cases):
            missing = [n for n in dependencies.get(name, ()) if n not in completed]
            if missing or (first and not diagnostic):
                outcomes.append({'case': name, 'outcome': 'skipped', 'reason': 'prerequisite not completed: ' + ','.join(missing)})
                continue
            try:
                print('case', name, flush=True); case()
                completed.add(name)
                outcomes.append({'case': name, 'outcome': 'completed'})
            except Exception as exc:
                fault = f'{type(exc).__name__}: {exc}'[:1024]
                first = first or fault
                outcomes.append({'case': name, 'outcome': 'failed', 'reason': fault})
        receipt = {'qualified': False if diagnostic else first is None, 'cases': outcomes,
                   'first_fault': first, 'volume_a': uuid_a, 'volume_b': world.get('uuid_b'),
                   'mount': str(mount), 'attachments': images.observations}
        atomic(evidence / ('scratch-diagnostic.json' if diagnostic else 'scratch-result.json'), receipt)
    except Exception as exc:
        first = first or f'{type(exc).__name__}: {exc}'[:1024]
        print('scratch proof aborted:', first, file=sys.stderr)
        if diagnostic and os.environ.get('OXIDE_PROOF_EVIDENCE'):
            remaining_cases = [n for n in NATIVE_CASES if n not in {o['case'] for o in outcomes}]
            outcomes.extend({'case': n, 'outcome': 'skipped', 'reason': 'unknown setup or custody'} for n in remaining_cases)
            atomic(Path(os.environ['OXIDE_PROOF_EVIDENCE']) / 'scratch-diagnostic.json',
                   {'qualified': False, 'cases': outcomes, 'first_fault': first})
        return 1
    if diagnostic: return 1
    return int(first is not None)


def main(argv):
    if argv == ['--self-check']:
        assert NATIVE_CASES == ('01','02','03','04','05','06','07','08','09','10','12','13','14','15')
        return self_check()
    if argv == ['--plan']: plan(); return 0
    if argv not in ([], ['--diagnostic']): raise SystemExit('unknown arguments')
    return proof(bool(argv))


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
