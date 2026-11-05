"""Pure injected preparation checks. No tracing/admission/guard/probe/native."""
import importlib.util
import json
from pathlib import Path
from unittest.mock import patch

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
    gate = dict(**ready, ready_sha256='digest', issued_mono_ns=100,
                challenge=P + '/challenge-' + 'a'*32)
    check('fresh gate', kit.gate_valid(gate, ready, 'digest', 101))
    for field, value in [('nonce', 'b'*32), ('session', P+'-other'), ('run','old-run'),
                         ('probe', {'pid':1,'start':3}), ('ready_sha256','old'),
                         ('challenge', '/outside'), ('issued_mono_ns',1010)]:
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
with patch.object(kit, 'private_file', side_effect=lambda p,*args: json.dumps(
        {'phase':'unclosed','outcome':{'closure':'UNCLOSED','status':1}} if p.name=='receipt.json' else ready).encode()), \
     patch.object(kit, 'validate_ready'), patch.object(Path,'exists',return_value=True), \
     patch.object(kit,'write_once',side_effect=lambda p,v: records.append((p.name,v))), \
     patch.object(kit.subprocess,'Popen') as route:
    route.return_value.wait.return_value = 125
    check('probe route failure preserved', kit.operator_start(Path(P),{},'a'*32,'digest') == 125)
    result = [v for n,v in records if n=='probe-route-exit.json'][0]
    check('guard UNCLOSED kept separately', result['guard_receipt']['outcome']['closure']=='UNCLOSED')
    check('tracer not inferred from probe', result['tracer_closure'].startswith('independent'))

print(json.dumps(dict(total_passed=len(checks), execution='PURE MOCKS ONLY; no native/trace/probe credit')))

check('inherited stream past margin stays UNKNOWN', kit.trace_window(36, 0, True) == 'handoff_unknown')
check('unknown wait past margin stays UNKNOWN', kit.trace_window(36, None) == 'handoff_unknown')
check('normal wait within window separate outcome', kit.trace_window(20, 0) == 'observe')
print(json.dumps(dict(final_total_passed=len(checks), execution='PURE MOCKS ONLY')))
