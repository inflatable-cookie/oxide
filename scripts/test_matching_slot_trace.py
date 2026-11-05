"""Pure injected preparation checks. No tracing/admission/guard/probe/native."""
import importlib.util
import json
from pathlib import Path
from unittest.mock import patch
from contextlib import ExitStack

spec = importlib.util.spec_from_file_location('trace280_tested', Path(__file__).with_name('matching-slot-trace.py'))
kit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kit)
P = '/Volumes/Stash/scratch/oxide/trace280-abcdef-new'
checks = []


def check(name, value):
    if not value:
        raise AssertionError(name)
    checks.append(name)


def line(body, op='rename', actor='Finder.987'):
    return ('19:30:01.123456 ' + op + ' ' + body + ' 0.000012 W ' + actor + '\n').encode()


# Every code path exercised below has process launches explicitly disabled.
with patch.object(kit.subprocess, 'Popen', side_effect=AssertionError('process launch forbidden')), \
     patch.object(kit.subprocess, 'run', side_effect=AssertionError('process run forbidden')):
    event, kind = kit.parse_line(line(P + '/runs/oxide-proof.a/evidence/f -> /Users/secret/private'), P)
    check('owned rename retained', kind == 'matching')
    rendered = json.dumps(event)
    check('foreign same-line path redacted', 'secret' not in rendered and event['unrelated_paths_redacted'] == 1)
    check('thread suffix never inferred PID', event['thread_reported'] == '987' and event['pid'] is None and event['identity'] == 'unverified')
    for path in [P + '-other/f', P + '/../escape', P + '//double', '/Users/secret', P + '/f->/outside']:
        event, kind = kit.parse_line(line(path), P)
        check('escape or foreign refused: ' + path.split('/')[-1], event is None)
    event, kind = kit.parse_line(line(P + '/f [ 2]', 'lstat64'), P)
    check('errno retained without body', event['errno_reported'] == 2 and 'body' not in event)
    for raw in [b'garbage /Users/secret\n', b'fs_usage: warning dropped events /Users/secret\n',
                line(P + '/f', actor='unsafe/path.123'), b'\xff\n']:
        event, kind = kit.parse_line(raw, P)
        check('malformed/drop/privacy refusal' + str(len(checks)), event is None)
    sink = []
    f = kit.Filter(P, sink.append)
    for part in [line(P + '/f', 'stat64')[:7], line(P + '/f', 'stat64')[7:]]:
        f.feed(part)
    check('incremental split line', f.counts['matching'] == 1)
    f.feed(b'x' * (kit.LINE + 400) + b' /Users/secret\n')
    check('overlong counted/discarded', f.counts['overlong'] == 1 and len(f.buffer) == 0)
    f.feed(b'incomplete')
    f.finish()
    check('partial final line inconclusive', f.counts['unparsed'] == 1)
    f = kit.Filter(P, sink.append)
    for _ in range(3000):
        f.feed(line(P + '/runs/oxide-proof.a/evidence/f', 'stat64'))
    check('retained trace ceiling', f.kept <= kit.CAP and f.counts['matching_truncated'] > 0)
    check('no unrelated trace stored', b'secret' not in b''.join(sink))

    ready = dict(session=P, nonce='a'*32, run='run-a', probe={'pid': 1, 'start': 2})
    gate = dict(**ready, ready_sha256='digest', clock=kit.GATE_CLOCK, issued_uptime_ns=100,
                challenge=P + '/challenge-' + 'a'*32)
    check('fresh gate', kit.gate_valid(gate, ready, 'digest', 101))
    for field, value in [('nonce', 'b'*32), ('session', P+'-other'), ('run','old-run'),
                         ('probe', {'pid':1,'start':3}), ('ready_sha256','old'),
                         ('challenge', '/outside'), ('issued_uptime_ns',1010)]:
        wrong = dict(gate, **{field:value})
        check('wrong/stale gate refused: ' + field, not kit.gate_valid(wrong, ready, 'digest', 101))
    check('expired gate refused', not kit.gate_valid(gate, ready, 'digest', 3_000_000_101))
    # Structural path rejection happens before any metadata read/old-root access.
    escaped = dict(ready, manifest='digest', root='/Users/secret', evidence='/Users/secret/evidence')
    try:
        kit.validate_ready(Path(P), ready['nonce'], 'digest', escaped)
    except ValueError:
        check('ready escape refused before metadata', True)
    else:
        raise AssertionError('ready escape accepted')
    bound = dict(ready, manifest='digest', root=P+'/runs/oxide-proof.a',
                 evidence=P+'/runs/oxide-proof.a/evidence', root_identity={}, evidence_identity={})
    with patch.object(kit, 'safe_path', side_effect=lambda p:p), patch.object(kit, 'fact', return_value={'changed':True}):
        try:
            kit.validate_ready(Path(P), ready['nonce'], 'digest', bound)
        except ValueError:
            check('replaced ready identity refused', True)
        else:
            raise AssertionError('replacement accepted')
    check('default mode effect-free', kit.main([]) == 0)

# Source-level fixed boundaries are also explicit in reviewed operator code.
check('fixed privileged argv', kit.TRACE == ['/usr/bin/sudo','-n','/usr/bin/fs_usage','-w','-f','filesys','-t','30'])
source = Path(kit.__file__).read_text()
check('no kit kill or detach', all(x not in source for x in ['.kill(', 'killpg(', 'start_new_session=', 'os.setsid(', 'os.system(']))
check('independent outcomes not conflated', 'privileged_closure' in source and 'guard_receipt' in source and 'not independently proven' in source)
check('unknown timeout and handoff retained', 'trace-window.json' in source and "elapsed > 35" in source and "while child.poll() is None" in source)
check('probe20s and no reset', '20_000_000_000' in source and '120_000_000_000' in source and "deadline - 30_000_000_000" in source)
print(json.dumps(dict(passed=len(checks), checks=checks, execution='PURE MOCKS ONLY; no native/trace/probe credit'), indent=2))

# Actual collector timeout/unknown-wait branch with mocked processes, clocks,
# descriptors, selectors and private metadata. No OS actor/file is created.
class Clock:
    def __init__(self): self.n = 0
    def tick(self):
        self.n += 1
        return self.n

class Pipe:
    def __init__(self, number): self.number, self.closed = number, False
    def fileno(self): return self.number
    def close(self): self.closed = True

class Selection:
    def register(self, *args): pass
    def get_map(self): return {}
    def select(self, timeout): return []
    def close(self): pass

for exit_status in (0, None):
    clock = Clock()
    child = type('Child', (), {})()
    child.pid, child.stdout, child.stderr = 999, Pipe(11), Pipe(12)
    child.poll = lambda: 0 if clock.n > 37 else None
    child.wait = lambda: exit_status
    observed = {'state':'observed', 'row':{'pid':999,'start':123}}
    ready = dict(session=P,nonce='a'*32,manifest='digest',run='run',probe=observed,
                 deadline_ns=999999999999)
    records = []
    with patch.object(kit, 'private_file', return_value=json.dumps(ready).encode()), \
         patch.object(kit, 'validate_ready'), patch.object(kit, 'reader', return_value=object()), \
         patch.object(kit, 'probe_live', return_value=True), \
         patch.object(kit, 'identity', return_value=observed), \
         patch.object(kit, 'write_once', side_effect=lambda p,v: records.append((p.name,v))), \
         patch.object(Path, 'exists', return_value=False), patch.object(Path,'lstat'), \
         patch.object(kit.os, 'open', return_value=10), patch.object(kit.os,'close'), \
         patch.object(kit.os, 'fdopen') as stream, patch.object(kit.os,'set_blocking'), \
         patch.object(kit.subprocess,'Popen',return_value=child) as launch, \
         patch.object(kit.selectors,'DefaultSelector',return_value=Selection()), \
         patch.object(kit.time,'monotonic',side_effect=clock.tick):
        result = kit.collect(Path(P), {}, 'a'*32, 'digest')
        check('timeout/unknown job not PASS ' + str(exit_status), result == 1)
        check('only exact privileged command ' + str(exit_status), launch.call_args.args[0] == kit.TRACE)
        end = [v for n,v in records if n == 'trace-end.json'][0]
        check('actual nullable wait preserved ' + str(exit_status), end['sudo_wait_status'] == exit_status)
        check('timeout uncertainty preserved ' + str(exit_status), any(n=='trace-window.json' for n,v in records))
        check('root closure not invented ' + str(exit_status), end['privileged_closure']=='not independently proven')

# Probe/guard result is independent from a successful tracer launcher status.
records = []
ready = dict(root=P+'/runs/oxide-proof.a', evidence=P+'/runs/oxide-proof.a/evidence')
with patch.object(kit, 'launch_context', return_value=Path('/reviewed/checkout')), \
     patch.dict(kit.os.environ, {'NUCLEUS_RUN_ID':'test-run'}), \
     patch.object(kit, 'private_file', side_effect=lambda p,*args: json.dumps(
        {'phase':'unclosed','outcome':{'closure':'UNCLOSED','status':1}} if p.name=='receipt.json' else ready).encode()), \
     patch.object(kit, 'validate_ready'), patch.object(Path,'exists',return_value=True), \
     patch.object(kit,'write_once',side_effect=lambda p,v: records.append((p.name,v))), \
     patch.object(kit.subprocess,'Popen') as route:
    route.return_value.wait.return_value = 125
    check('probe route failure preserved', kit.operator_start(Path(P),{'effigy':'/reviewed/effigy'},'a'*32,'digest') == 125)
    check('Effigy launch uses bound executable/cwd', route.call_args.args[0] == ['/reviewed/effigy',kit.TASK] and route.call_args.kwargs['cwd']==Path('/reviewed/checkout'))
    result = [v for n,v in records if n=='probe-route-exit.json'][0]
    check('guard UNCLOSED kept separately', result['guard_receipt']['outcome']['closure']=='UNCLOSED')
    check('tracer not inferred from probe', result['tracer_closure'].startswith('independent'))

print(json.dumps(dict(total_passed=len(checks), execution='PURE MOCKS ONLY; no native/trace/probe credit')))

check('inherited stream past margin stays UNKNOWN', kit.trace_window(36, 0, True) == 'handoff_unknown')
check('unknown wait past margin stays UNKNOWN', kit.trace_window(36, None) == 'handoff_unknown')
check('normal wait within window separate outcome', kit.trace_window(20, 0) == 'observe')
print(json.dumps(dict(final_total_passed=len(checks), execution='PURE MOCKS ONLY')))

check('private ordinary guard receipt umask', 'os.umask(0o077)' in Path(kit.__file__).read_text())
print(json.dumps(dict(final_total_passed=len(checks), execution='PURE MOCKS ONLY')))

from types import SimpleNamespace
fake = SimpleNamespace(Kinfo=SimpleNamespace(one=lambda pid: (_ for _ in ()).throw(OSError('injected'))))
check('observable PID kept with unreadable start identity', kit.identity(fake,321) == dict(pid=321,state='unreadable',row=None))
print(json.dumps(dict(final_total_passed=len(checks), execution='PURE MOCKS ONLY')))


# Retained-worker admission validation: no credential export, no actual hook.
from unittest.mock import mock_open
import stat
worker = dict(NUCLEUS_TASK_ID=kit.TASK_ID, NUCLEUS_RUN_ID='mock-run',
              NUCLEUS_RUN_TOKEN='mock-token', QUEUE_ADMISSION_SOCKET='/mock/socket',
              EFFIGY_ADMISSION_HOOK='ELECTRON_RUN_AS_NODE=1 /mock/node /mock/queue-effigy-admission-hook.mjs')
with patch.dict(kit.os.environ, worker, clear=True), \
     patch.object(kit.os, 'access', return_value=True), \
     patch.object(Path, 'lstat', return_value=SimpleNamespace(st_mode=stat.S_IFSOCK|0o600, st_uid=kit.os.getuid())), \
     patch('builtins.open', mock_open(read_data=b'mock supported hook')):
    context = kit.worker_context()
    check('context records hashes not credentials/settings', 'mock-token' not in json.dumps(context) and '/mock/' not in json.dumps(context))
    for key in ('EFFIGY_ADMISSION_HOOK', 'NUCLEUS_RUN_TOKEN', 'QUEUE_ADMISSION_SOCKET', 'NUCLEUS_TASK_ID'):
        changed = dict(worker); del changed[key]
        with patch.dict(kit.os.environ, changed, clear=True):
            try: kit.worker_context()
            except ValueError: check('missing worker context refuses '+key, True)
            else: raise AssertionError('missing context accepted')

c = dict(admission_context={'trusted':True}, checkout='/reviewed/checkout', checkout_identity={},
         hashes={'matching-slot-trace.py':kit.sha(b'kit')}, catalog_sha256=kit.sha(b'catalog'),
         effigy='/reviewed/effigy', effigy_identity={})
with patch.object(kit,'worker_context',return_value={'trusted':True}), \
     patch.object(kit,'safe_path',side_effect=lambda p:p), patch.object(kit,'fact',return_value={}), \
     patch.object(Path,'cwd',return_value=Path(c['checkout'])), \
     patch.object(Path,'read_bytes',side_effect=lambda:b'catalog') as read, \
     patch.object(kit.shutil,'which',return_value=c['effigy']), \
     patch.dict(kit.os.environ,{},clear=True):
    read.side_effect = [b'catalog', b'kit']
    check('bound checkout and catalog accepted', kit.launch_context(c)==Path(c['checkout']))
    for marker in ('', 'old-run'):
        with patch.dict(kit.os.environ,{'EFFIGY_ADMITTED':marker}):
            try: kit.launch_context(c)
            except ValueError: check('stale admission refuses before read '+repr(marker), True)
            else: raise AssertionError('stale admitted accepted')
    with patch.object(Path,'cwd',return_value=Path('/wrong/catalog')):
        try: kit.launch_context(c)
        except ValueError: check('wrong cwd refuses before launch',True)
        else: raise AssertionError('wrong cwd accepted')
    read.side_effect = [b'changed catalog']
    try: kit.launch_context(c)
    except ValueError: check('changed catalog refused',True)
    else: raise AssertionError('wrong catalog accepted')
    with patch.object(kit,'worker_context',return_value={'trusted':False}):
        try: kit.launch_context(c)
        except ValueError: check('changed inherited hook context refused',True)
        else: raise AssertionError('changed hook accepted')
    try: kit.launch_context(c,admitted=True)
    except ValueError: check('direct guarded mode lacks Effigy marker refuses',True)
    else: raise AssertionError('direct guard accepted')

# Exercise the actual tiny probe through injected interfaces only: first
# mismatch/missing path must unwind before another create/read. No file effects.
m = SimpleNamespace(st_dev=1,st_ino=2,st_mode=stat.S_IFREG|0o600,st_size=40,
                    st_blocks=8,st_mtime_ns=10,st_ctime_ns=10)
changed = SimpleNamespace(**dict(vars(m),st_ino=3))
for label, observations, expected_creates, expected_reads, fails in (
        ('post-close replacement',[changed],1,1,True),
        ('post-close disappearance',[FileNotFoundError()],1,1,True),
        ('subsequent change',[m,m,m,changed],3,4,True),
        ('subsequent disappearance',[m,m,m,FileNotFoundError()],3,4,True),
        ('late change',[m]*42+[changed],3,43,True),
        ('unchanged',[m]*300,3,None,False),
        ('expired readiness',[],0,0,False),
        ('wrong clock readiness',[],0,0,False)):
    records=[]; clock=Clock()
    g=SimpleNamespace(now_ns=lambda:clock.tick()*100_000_000)
    env=dict(OXIDE_TASK_SCRATCH_ROOT=P+'/runs/oxide-proof.a/scratch',
             OXIDE_PROOF_EVIDENCE=P+'/runs/oxide-proof.a/evidence',
             OXIDE_PROOF_DEADLINE_NS='300000000000',OXIDE_PROOF_RUN_ID='mock-run')
    def exists(path): return path.name=='gate.json'
    with ExitStack() as stack:
        for mock in [patch.dict(kit.os.environ,env,clear=True),
                     patch.object(kit,'reader',return_value=g),
                     patch.object(kit,'identity',return_value={'state':'observed'}),
                     patch.object(kit,'safe_path',side_effect=lambda p:p),
                     patch.object(kit,'fact',return_value={}),
                     patch.object(kit,'write_once',side_effect=lambda p,v:records.append((p.name,v))),
                     patch.object(kit,'private_file',return_value=b'{}'),
                     patch.object(kit,'gate_decision',return_value={'accepted':expected_creates>0,
                                  'reason':'gate_expired' if label=='expired readiness' else
                                  'gate_clock_mismatch' if label=='wrong clock readiness' else 'gate_accepted',
                                  'clock':kit.GATE_CLOCK}),
                     patch.object(Path,'exists',exists), patch.object(Path,'mkdir'),
                     patch.object(kit.os,'fdopen'), patch.object(kit.os,'fstat',return_value=m),
                     patch.object(kit.time,'sleep'), patch.object(kit.signal,'signal'),
                     patch.object(kit.signal,'setitimer'),
                     patch.object(kit.subprocess,'Popen',side_effect=AssertionError('launch forbidden'))]:
            stack.enter_context(mock)
        reads=stack.enter_context(patch.object(Path,'lstat',side_effect=observations))
        creates=stack.enter_context(patch.object(kit.os,'open',return_value=99))
        refused=False
        try: result=kit.probe(Path(P),{},'a'*32,'digest')
        except (ValueError,FileNotFoundError): refused=True
        check(label+' refusal matches',refused==fails)
        check(label+' no additional creation',creates.call_count==expected_creates)
        check(label+' no additional read',expected_reads is None or reads.call_count==expected_reads)
        receipt=records[-1][1]
        check(label+' descriptor facts match effects',any(r.get('phase')=='creation-descriptor' for r in receipt['rows'])==(expected_creates>0))
        if fails:
            check(label+' bounded refusal retained',records[-1][0]=='trace-probe-refusal.json' and any(r.get('changed') or r.get('error') for r in receipt['rows']))
        elif expected_creates:
            check('nonmutation finishes normally',result==0 and records[-1][0]=='trace-probe-receipt.json')
        else:
            check(label+' reason persisted without effects',result==1 and receipt['readiness']['reason'] in ('gate_expired','gate_clock_mismatch') and receipt['rows']==[])
print(json.dumps(dict(final_total_passed=len(checks), execution='PURE MOCKS ONLY; no admission/guard/probe/trace')))


# Cross-process freshness uses only injected qualified shared uptime, even
# when Python monotonic clocks have unrelated origins. No real clock read.
ready=dict(session=P,nonce='a'*32,run='mock-run',probe={'pid':1,'start':2})
challenge=Path(P)/('challenge-'+ready['nonce'])
for collector_origin,probe_origin in [(0,90_000_000_000),(90_000_000_000,0)]:
    shared=SimpleNamespace(now_ns=lambda:100_000_000_000)
    with patch.object(kit.time,'monotonic_ns',return_value=collector_origin) as local:
        gate=kit.gate_record(ready,'digest',challenge,shared)
        check('collector does not read process-origin clock '+str(collector_origin),local.call_count==0)
    with patch.object(kit.time,'monotonic_ns',return_value=probe_origin) as local:
        d=kit.gate_decision(gate,ready,'digest',101_000_000_000)
        check('differing clock origins still fresh '+str(probe_origin),d['accepted'] and d['age_ns']==1_000_000_000 and local.call_count==0)
    check('shared3s exact boundary accepted '+str(collector_origin),kit.gate_valid(gate,ready,'digest',103_000_000_000))
    d=kit.gate_decision(gate,ready,'digest',103_000_000_001)
    check('shared3s unchanged rejection '+str(collector_origin),d['reason']=='gate_expired' and not d['accepted'] and d['age_ns']==3_000_000_001)
    d=kit.gate_decision(gate,ready,'digest',99_999_999_999)
    check('future clock explicitly refused '+str(collector_origin),d['reason']=='gate_timestamp_future' and not d['accepted'])
for field,value,reason in [('clock','process-local','gate_clock_mismatch'),
                           ('issued_uptime_ns',True,'gate_clock_value_invalid'),
                           ('nonce','wrong','gate_nonce_mismatch'),
                           ('ready_sha256','wrong','gate_ready_sha256_mismatch')]:
    d=kit.gate_decision(dict(gate,**{field:value}),ready,'digest',101_000_000_000)
    check('explicit rejection reason '+field,not d['accepted'] and d['reason']==reason)
old=dict(gate);del old['clock'];del old['issued_uptime_ns'];old['issued_mono_ns']=3681133083
check('old timestamps neither adopted nor reinterpreted',kit.gate_decision(old,ready,'digest',101_000_000_000)['reason']=='gate_clock_mismatch' and old['issued_mono_ns']==3681133083)
check('malformed gate rejection recorded',kit.gate_decision(None,ready,'digest',101_000_000_000)['reason']=='gate_not_object')
source=Path(kit.__file__).read_text()
check('actual probe compares qualified clock and records reason', 'gate_decision(gate, ready, ready_digest, g.now_ns())' in source and 'readiness=readiness' in source and 'time.monotonic_ns()' not in source)
print(json.dumps(dict(final_total_passed=len(checks), execution='PURE MOCKS ONLY; no restaging/admission/guard/probe/trace')))
