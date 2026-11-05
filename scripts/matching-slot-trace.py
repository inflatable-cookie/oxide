#!/usr/bin/env python3
"""Oxide#280 fixed trace kit. PREPARATION ONLY until separately activated.

No/default/self-check/prepare mode starts sudo, fs_usage, admission, guard or
probe. Only explicit later operator modes can do so. No other process is
signalled by this kit; the probe may arm its own20s alarm. The privileged foreground sudo/fs_usage job is operator-owned;
its wait status is NOT the ordinary guard's probe-closure verdict.

Nine premises: (1) no Cargo/toolchain child or replacement HOME; (2) no account
API, private-account override n/a; (3) probe environment strips OXIDE_*/XDG_*
except guard protocol and this session binding; (4) no adoption; (5) no Cargo
external argv; (6) no images; (7) no compiler flags/build; (8) only the verified
Stash session/runs explicit-root exception, qualified guard samples whole root
including evidence, probe data ONLY in actual evidence; (9) mkdtemp always has
explicit parent. Collector bookkeeping is separately bounded private control
plane, not charged as probe scratch. No payload reads, cleanup or attribution.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import secrets
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
GUARD = 'fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b'
UUID = '8A660440-C919-45C6-A3F1-3365B9BD2006'
PARENT = Path('/Volumes/Stash/scratch/oxide')
TRACE = ['/usr/bin/sudo', '-n', '/usr/bin/fs_usage', '-w', '-f', 'filesys', '-t', '30']
CAP = 512 * 1024
LINE = 4096
TASK = 'proof:matching-slot-trace:operator'
META = {'lstat', 'lstat64', 'stat', 'stat64', 'getattrlist', 'getattrlistbulk', 'access'}
OPS = META | {'open', 'openat', 'open_nocancel', 'close', 'fstat', 'fstat64',
              'rename', 'renameat', 'unlink', 'unlinkat', 'mkdir', 'rmdir',
              'read', 'write', 'fsync', 'fcntl', 'readlink', 'getdirentries64'}
ROW = re.compile(r'^(\d{2}:\d{2}:\d{2}\.\d{6})\s+([A-Za-z_][A-Za-z0-9_]*)\s+(.*?)\s+(\d+\.\d+)(?:\s+W)?\s+([A-Za-z0-9_-]{1,64})\.(\d{1,20})\s*$')
PATH = re.compile(r'^/[A-Za-z0-9_./-]+$')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def fact(path):
    m = path.lstat()
    return dict(dev=m.st_dev, ino=m.st_ino, uid=m.st_uid, mode=m.st_mode)


def safe_path(path):
    raw = str(path)
    if not raw.startswith('/') or any(p in ('.', '..', '') for p in raw.split('/')[1:]):
        raise ValueError('unsafe path components')
    current = Path('/')
    for part in path.parts[1:]:
        current /= part
        if stat.S_ISLNK(current.lstat().st_mode):
            raise ValueError('symlink component')
    return path


def private_file(path, limit=16384):
    safe_path(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        m = os.fstat(fd)
        if not stat.S_ISREG(m.st_mode) or m.st_uid != os.getuid() or m.st_mode & 0o077:
            raise ValueError('unsafe private file')
        data = os.read(fd, limit + 1)
        if len(data) > limit:
            raise ValueError('document bound')
        return data
    finally:
        os.close(fd)


def write_once(path, value):
    data = (json.dumps(value, sort_keys=True) + '\n').encode()
    if len(data) > 16384:
        raise ValueError('receipt bound')
    temporary = path.with_name(path.name + '.' + secrets.token_hex(8) + '.pending')
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as out:
        out.write(data)
        out.flush()
    os.link(temporary, path, follow_symlinks=False)  # atomic publish, refuses existing name
    # Keep our tiny pending hardlink; no cleanup of any session resources.


def volume():
    # Effect-free single-volume metadata read, never a pool/tree scan.
    r = subprocess.run(['/usr/sbin/diskutil', 'info', '-plist', '/Volumes/Stash'],
                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=True)
    if len(r.stdout) > 65536:
        raise ValueError('volume document bound')
    d = plistlib.loads(r.stdout)
    if d.get('VolumeUUID') != UUID or d.get('MountPoint') != '/Volumes/Stash':
        raise ValueError('wrong Stash UUID/mount')
    return UUID


def bind(session, nonce, digest):
    if any(part in ('', '.', '..') for part in session.split('/')[1:]):
        raise ValueError('unsafe raw session components')
    session = safe_path(Path(session))
    raw = private_file(session / 'session.json')
    if sha(raw) != digest:
        raise ValueError('manifest hash')
    c = json.loads(raw)
    if c['session'] != str(session) or c['nonce'] != nonce or fact(session) != c['identity']:
        raise ValueError('session identity')
    if c['identity']['uid'] != os.getuid() or stat.S_IMODE(c['identity']['mode']) != 0o700:
        raise ValueError('session ownership/mode')
    volume()
    if c['identity']['dev'] != Path('/Volumes/Stash').stat().st_dev:
        raise ValueError('session not on actual Stash device')
    if str(Path(sys.executable).resolve()) != c['interpreter']:
        raise ValueError('interpreter changed')
    if sha(Path(__file__).read_bytes()) != c['hashes']['matching-slot-trace.py']:
        raise ValueError('executing kit differs from staged source')
    for name in ('kit', 'runs'):
        safe_path(session / name)
        if fact(session / name) != c['directories'][name]:
            raise ValueError('directory replaced')
    for name, digest in c['hashes'].items():
        if sha(private_file(session / 'kit' / name, 512 * 1024)) != digest:
            raise ValueError('staged source changed')
    if c['hashes']['bounded-test.py'] != GUARD:
        raise ValueError('guard pin')
    return session, c


def reader(session):
    spec = importlib.util.spec_from_file_location('trace280_guard_reader', session / 'kit/bounded-test.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.Kinfo.check()
    return module


def identity(module, pid):
    try:
        row = module.Kinfo.one(pid)
        return dict(state='observed' if row else 'absent', row=row)
    except (OSError, ValueError):
        return dict(state='unreadable', row=None)


def owned_token(token, prefix):
    return (bool(PATH.fullmatch(token)) and token.startswith(prefix + '/') and
            all(p not in ('', '.', '..') for p in token.split('/')[1:]))


def parse_line(raw, prefix):
    """Strict supported subset. Never return raw body or unrelated paths."""
    if len(raw) > LINE:
        return None, 'overlong'
    try:
        text = raw.decode('ascii').strip()
    except UnicodeDecodeError:
        return None, 'unparsed'
    if re.search(r'(?i)drop|lost.event|warning|trace.*(fail|error)|permission.denied', text):
        return None, 'utility_warning_or_drop'
    match = ROW.fullmatch(text)
    if not match or match[2] not in OPS:
        return None, 'unparsed'
    tokens = re.findall(r'\[\s*\d+\s*\]|\S+', match[3])
    if len(tokens) > 32:
        return None, 'unparsed'
    paths, foreign, errno = [], 0, None
    for token in tokens:
        if token.startswith('/'):
            if owned_token(token, prefix):
                paths.append(token)
            else:
                foreign += 1
        elif re.fullmatch(r'\[\s*\d+\s*\]', token):
            errno = int(token.strip('[] '))
        elif not re.fullmatch(r'(?:[FBAODSC]=[0-9A-Fa-fx]+|->)', token):
            return None, 'unparsed'
    if not paths:
        return None, 'discarded'
    return dict(time_of_day=match[1], operation=match[2], owned_paths=paths,
                errno_reported=errno, elapsed_reported=match[4], process_reported=match[5],
                thread_reported=match[6], pid=None, identity='unverified',
                unrelated_paths_redacted=foreign), 'matching'


class Filter:
    def __init__(self, prefix, sink):
        self.prefix, self.sink = prefix, sink
        self.buffer, self.overlong = bytearray(), False
        self.counts = dict(discarded=0, unparsed=0, overlong=0,
                           matching=0, matching_truncated=0, utility_warning_or_drop=0)
        self.kept = 0

    def feed(self, chunk):
        events = []
        for byte in chunk:
            if byte == 10:
                if self.overlong:
                    self.counts['overlong'] += 1
                else:
                    event, kind = parse_line(bytes(self.buffer), self.prefix)
                    self.counts[kind] += 1
                    if event:
                        encoded = (json.dumps(event, sort_keys=True) + '\n').encode()
                        if self.kept + len(encoded) <= CAP:
                            self.sink(encoded)
                            self.kept += len(encoded)
                            events.append(event)
                        else:
                            self.counts['matching_truncated'] += 1
                self.buffer.clear()
                self.overlong = False
            elif not self.overlong:
                if len(self.buffer) >= LINE:
                    self.buffer.clear()
                    self.overlong = True
                else:
                    self.buffer.append(byte)
        return events

    def finish(self):
        if self.buffer or self.overlong:
            self.counts['overlong' if self.overlong else 'unparsed'] += 1
        self.buffer.clear()


def gate_valid(gate, ready, digest, now):
    return (gate.get('nonce') == ready['nonce'] and gate.get('session') == ready['session'] and
            gate.get('ready_sha256') == digest and gate.get('run') == ready['run'] and
            gate.get('probe') == ready['probe'] and
            type(gate.get('issued_mono_ns')) is int and
            0 <= now - gate['issued_mono_ns'] <= 3_000_000_000 and
            gate.get('challenge') == str(Path(ready['session']) / ('challenge-' + ready['nonce'])))


def prepare():
    volume()
    safe_path(PARENT)
    if PARENT.stat().st_dev != Path('/Volumes/Stash').stat().st_dev:
        raise ValueError('control parent not on Stash device')
    repo = Path(__file__).resolve().parent.parent
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo).decode().strip()
    branch = 'ns-b2c68e24-53b8-4f24-b4ee-ec18cb74a999'
    remote = subprocess.check_output(['git', 'ls-remote', 'origin', 'refs/heads/' + branch], cwd=repo).decode().split()[0]
    if head != remote or subprocess.check_output(['git', 'status', '--porcelain'], cwd=repo):
        raise ValueError('prepare requires clean exact pushed source')
    sources = {name: (repo / 'scripts' / name).read_bytes()
               for name in ('matching-slot-trace.py', 'bounded-test.py', 'test_matching_slot_trace.py')}
    if sha(sources['bounded-test.py']) != GUARD:
        raise ValueError('guard source mismatch')
    for name, data in sources.items():
        if subprocess.check_output(['git', 'show', head + ':scripts/' + name], cwd=repo) != data:
            raise ValueError('source not at pushed head')
    nonce = secrets.token_hex(16)
    session = Path(tempfile.mkdtemp(prefix='trace280-' + nonce + '-', dir=PARENT))
    for name in ('kit', 'runs'):
        (session / name).mkdir(mode=0o700)
    for name, data in sources.items():
        fd = os.open(session / 'kit' / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700)
        with os.fdopen(fd, 'wb') as out:
            out.write(data)
    c = dict(session=str(session), nonce=nonce, identity=fact(session), volume_uuid=UUID,
             source_head=head, interpreter=str(Path(sys.executable).resolve()), directories={n: fact(session / n) for n in ('kit', 'runs')},
             hashes={n: sha(data) for n, data in sources.items()}, phase='PREPARED; EXECUTION HELD')
    write_once(session / 'session.json', c)
    digest = sha((session / 'session.json').read_bytes())
    args = f"'{session}' '{nonce}' '{digest}'"
    kit = session / 'kit/matching-slot-trace.py'
    python = c['interpreter']
    instructions = f'''# Oxide280: HELD pending exact-head review AND separate planner activation

Session: {session}
Nonce: {nonce}
Session manifest SHA256: {digest}
Source head: {head}
Kit SHA256: {c['hashes']['matching-slot-trace.py']}
Guard SHA256: {GUARD}
Stash UUID: {UUID}
Root-parent (later guard allocates one fresh child): {session / 'runs'}

Do not run these activation commands during preparation or review.
After reviewer acceptance, the planner must separately activate this ONE occurrence.
All code below runs as your ordinary user. Never run Python, shell, or kit with sudo.

Terminal A, after activation, starts normal real heavy admission:
'{python}' '{kit}' --operator-start {args}
Admission may wait before any trace clock starts. Leave it foreground; do not retry.
Wait for `GUARD READY` naming actual root/evidence and the120s ready window.
No probe files exist yet. Terminal A must remain open.

Terminal B, only after that ready message, authenticate interactively:
/usr/bin/sudo -v
Then immediately (within the ready window) start the unprivileged collector:
'{python}' '{kit}' --operator-collect {args}
The collector launches ONLY this privileged command, once:
/usr/bin/sudo -n /usr/bin/fs_usage -w -f filesys -t 30
No raw trace reaches your terminal or any file. Do not pipe fs_usage yourself.
Collector requires a fresh session challenge event within5s of launch before
opening the probe gate. No readiness => no .DS_Store probe effects.
Probe observes metadata for <=20s after gate, within its cumulative300s guard
allowance (120s readiness maximum plus30s shutdown margin; no clock reset).
Probe also arms its own20s SIGALRM (no other process signalled); blocked kernel
operations may delay delivery, and that uncertainty is not hidden. The unchanged
300s guard remains the independent outer bound. Trace has30s timeout plus5s observation margin. No second trace or probe attempt.

Abort: press Ctrl-C in Terminal B (your terminal/sudo owns privileged cancellation).
Kit never signals the tracer or another process. The probe alarms only itself.
It writes the session abort indication and stops probe
work cooperatively. If sudo/job has not exited by35s, collector retains UNKNOWN,
prints its exact observable PID/start and stays foreground awaiting you. Press
Ctrl-C in B to request normal sudo interruption and wait for its exit. If it
still does not exit, leave both terminals open and return the UNKNOWN receipt,
PID/start to the planner; do not use a pattern kill, detach, disown or another
root command. Root fs_usage descendants may not be independently observable;
a sudo wait status is recorded at that level, never invented root closure.
Terminal A separately waits for real guard completion; tracer exit is NOT probe
closure and probe exit is NOT tracer closure. No cleanup of retained resources.

View ONLY bounded receipts (never a raw trace):
'{python}' '{kit}' --receipts {args}

Privacy/limits: only structurally parsed ASCII metadata events naming this exact
fresh session prefix are retained (512KiB cap,4096B line bound). Other paths on
matching lines are omitted. Discard/unparsed/overlong/truncated/warning/drop
counters remain explicit. Unknown formats/truncation/missing events/identity
ambiguity are INCONCLUSIVE, not proof no actor exists. fs_usage TOD microseconds,
collector monotonic arrival and kernel PID/start identity tokens have different
bases; never order them as one causal clock. Process suffix is a reported THREAD
ID per local fs_usage manpage, not a verified PID. No historical holder claim.
Execution is observational only; mutation is recorded without recreation or
adoption. No cost fixture, installation, live readiness, QA or merge follows.
'''
    fd = os.open(session / 'OPERATOR.md', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as out:
        out.write(instructions)
    write_once(session / 'stage-receipt.json', dict(session_manifest_sha256=digest,
               instructions_sha256=sha(instructions.encode()), source_head=head, hashes=c['hashes']))
    print(json.dumps(dict(session=str(session), nonce=nonce, manifest_sha256=digest,
                         instructions_sha256=sha(instructions.encode()), identity=c['identity'],
                         source_head=head, hashes=c['hashes']), indent=2))


def validate_ready(session, nonce, digest, ready):
    root, evidence = Path(ready['root']), Path(ready['evidence'])
    if (ready['session'] != str(session) or ready['nonce'] != nonce or ready['manifest'] != digest or
            root.parent != session / 'runs' or not root.name.startswith('oxide-proof.') or
            evidence != root / 'evidence'):
        raise ValueError('wrong ready binding/escape')
    if (fact(safe_path(root)) != ready['root_identity'] or
            fact(safe_path(evidence)) != ready['evidence_identity']):
        raise ValueError('ready directory identity changed')


def operator_start(session, c, nonce, digest):
    # Explicit operator mode only. No override of admission or guard settings.
    write_once(session / 'launch.json', dict(nonce=nonce, manifest=digest))
    env = dict(os.environ, OXIDE_TRACE280_SESSION=str(session), OXIDE_TRACE280_NONCE=nonce,
               OXIDE_TRACE280_MANIFEST=digest)
    route = subprocess.Popen(['effigy', TASK], env=env)
    while True:
        try:
            code = route.wait()
            break
        except KeyboardInterrupt:
            print('Awaiting ordinary guard completion; leave Terminal A foreground.', flush=True)
    ready = json.loads(private_file(session / 'ready.json')) if (session / 'ready.json').exists() else None
    guard = None
    if ready:
        try:
            validate_ready(session, nonce, digest, ready)
            raw = private_file(Path(ready['evidence']) / 'receipt.json', 512 * 1024)
            document = json.loads(raw)
            guard = dict(path=str(Path(ready['evidence']) / 'receipt.json'), sha256=sha(raw),
                         phase=document.get('phase'), outcome=document.get('outcome'))
        except Exception as error:
            guard = dict(state='UNKNOWN', error=type(error).__name__)
    write_once(session / 'probe-route-exit.json', dict(effigy_exit=code, guard_receipt=guard,
               tracer_closure='independent; see trace receipts'))
    return code


def guarded_operator(session, c, nonce, digest):
    if json.loads(private_file(session / 'launch.json')) != dict(nonce=nonce, manifest=digest):
        raise ValueError('wrong launch binding')
    write_once(session / 'guard-entered.json', dict(nonce=nonce, manifest=digest))
    kit = session / 'kit'
    env = {k: v for k, v in os.environ.items() if not k.startswith(('OXIDE_', 'XDG_'))}
    argv = [sys.executable, str(kit / 'bounded-test.py'), '--limit', '300',
            '--scratch-budget-mib', '512', '--root-parent', str(session / 'runs'), '--',
            sys.executable, str(kit / 'matching-slot-trace.py'), '--probe', str(session), nonce, digest]
    os.umask(0o077)  # private future guard receipts; process-local, no host setting
    os.execve(sys.executable, argv, env)


def probe(session, c, nonce, digest):
    g = reader(session)
    scratch = safe_path(Path(os.environ['OXIDE_TASK_SCRATCH_ROOT']))
    evidence = safe_path(Path(os.environ['OXIDE_PROOF_EVIDENCE']))
    root = scratch.parent
    if root.parent != session / 'runs' or not root.name.startswith('oxide-proof.') or evidence != root / 'evidence' or scratch != root / 'scratch':
        raise ValueError('actual guard layout outside session')
    alarm_installed = False
    prior_alarm = None
    try:
        deadline = int(os.environ['OXIDE_PROOF_DEADLINE_NS'])
        me = identity(g, os.getpid())
        if me['state'] != 'observed':
            raise ValueError('probe identity unreadable')
        ready = dict(session=str(session), nonce=nonce, run=os.environ['OXIDE_PROOF_RUN_ID'],
                     probe=me, root=str(root), evidence=str(evidence), root_identity=fact(root),
                     evidence_identity=fact(evidence), deadline_ns=deadline, manifest=digest)
        write_once(session / 'ready.json', ready)
        ready_digest = sha(private_file(session / 'ready.json'))
        print('GUARD READY ' + json.dumps(ready) + '; gate wait <=120s', flush=True)
        until = min(g.now_ns() + 120_000_000_000, deadline - 30_000_000_000)
        accepted = False
        while g.now_ns() < until:
            if (session / 'abort.json').exists():
                break
            if (session / 'gate.json').exists():
                gate = json.loads(private_file(session / 'gate.json'))
                accepted = gate_valid(gate, ready, ready_digest, time.monotonic_ns())
                break
            time.sleep(.1)
        rows = []
        outcome = 'readiness_refused_no_probe_files'
        if accepted and deadline - g.now_ns() >= 50_000_000_000:
            stop = min(g.now_ns() + 20_000_000_000, deadline - 30_000_000_000)
            def observation_alarm(_number, _frame):
                raise TimeoutError('probe20s observation window ended')
            prior_alarm = signal.signal(signal.SIGALRM, observation_alarm)
            alarm_installed = True
            signal.setitimer(signal.ITIMER_REAL, 20)
            tree = evidence / ('trace-probe-' + nonce)
            expected = {}
            tree.mkdir(mode=0o700)
            (tree / 'debug').mkdir(mode=0o700)
            parents = {tree: fact(tree), tree / 'debug': fact(tree / 'debug')}
            try:
                for name in ('.DS_Store', 'debug/.DS_Store', 'ordinary-control'):
                    if g.now_ns() >= stop:
                        break
                    path = tree / name
                    if fact(safe_path(path.parent)) != parents[path.parent]:
                        raise ValueError('probe parent identity changed')
                    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                    with os.fdopen(fd, 'wb') as out:
                        out.write(b'O' * 40)
                        out.flush()
                        m = os.fstat(out.fileno())
                        expected[name] = [m.st_dev, m.st_ino, m.st_mode, m.st_size, m.st_blocks,
                                          m.st_mtime_ns, m.st_ctime_ns]
                        rows.append(dict(name=name, phase='creation-descriptor', expected=expected[name]))
                    # immediate metadata observation, no payload read/adoption
                    m = path.lstat()
                    rows.append(dict(name=name, phase='post-close', expected=expected[name],
                                     observed=[m.st_dev,m.st_ino,m.st_mode,m.st_size,m.st_blocks,m.st_mtime_ns,m.st_ctime_ns]))
            except (OSError, ValueError) as error:
                rows.append(dict(phase='creation_refusal', error=type(error).__name__))
            outcome = 'metadata_observations_only; no attribution or qualification'
            while g.now_ns() < stop and len(rows) < 256:
                if (session / 'abort.json').exists():
                    outcome = 'operator_abort; observations_kept'
                    break
                for name, original in expected.items():
                    if g.now_ns() >= stop:
                        break
                    try:
                        m = (tree / name).lstat()
                        observed = [m.st_dev,m.st_ino,m.st_mode,m.st_size,m.st_blocks,m.st_mtime_ns,m.st_ctime_ns]
                        rows.append(dict(name=name, expected=original, observed=observed, changed=observed != original))
                    except OSError as error:
                        rows.append(dict(name=name, expected=original, error=type(error).__name__))
                time.sleep(.25)
        if alarm_installed:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, prior_alarm)
            alarm_installed = False
        # bounded metadata only; reduce rows to fit a single small receipt
        write_once(evidence / 'trace-probe-receipt.json', dict(outcome=outcome, ready=ready,
                   rows=rows[:40], observed_rows=len(rows), omitted_rows=max(0,len(rows)-40),
                   closure='not claimed here; qualified guard owns closure'))
        return 0 if accepted else 1
    except Exception as error:
        if alarm_installed:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, prior_alarm)
        write_once(evidence / 'trace-probe-refusal.json', dict(error=type(error).__name__,
                   effects='unknown if already activated; retained, never retried',
                   closure='guard verdict only'))
        raise


def probe_live(module, ready):
    expected = ready['probe']
    return (expected.get('state') == 'observed' and
            identity(module, expected['row']['pid']) == expected and
            ready['deadline_ns'] - module.now_ns() >= 50_000_000_000)


def trace_window(elapsed, wait_status, pipes_open=False):
    return 'handoff_unknown' if elapsed > 35 and (wait_status is None or pipes_open) else 'observe'


def collect(session, c, nonce, digest):
    ready_raw = private_file(session / 'ready.json')
    ready = json.loads(ready_raw)
    validate_ready(session, nonce, digest, ready)
    for name in ('gate.json', 'trace-window.json', 'trace-end.json'):
        if (session / name).exists():
            raise ValueError('one-shot or stale collector state')
    g = reader(session)
    if not probe_live(g, ready):
        raise ValueError('probe gone/replaced/expired before tracer launch')
    write_once(session / 'collector.json', dict(identity=identity(g, os.getpid()), nonce=nonce))
    output_fd = os.open(session / 'owned-trace.jsonl', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    stream = os.fdopen(output_fd, 'wb', buffering=0)
    filtered = Filter(str(session), stream.write)
    # Fixed privileged argv, foreground inherited session; NO shell or signals.
    child = subprocess.Popen(TRACE, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             env=dict(os.environ, LC_ALL='C'))
    job_identity = identity(g, child.pid)
    selector = selectors.DefaultSelector()
    errors = Filter('__stderr_has_no_retained_paths__', lambda data: None)
    start = time.monotonic()
    challenge = session / ('challenge-' + nonce)
    gate = False
    handed_back = False
    failure = None
    aborted = False
    def abort(reason):
        if not (session / 'abort.json').exists():
            write_once(session / 'abort.json', dict(nonce=nonce, reason=reason))
    try:
        write_once(session / 'tracer-start.json', dict(command=TRACE, launcher_identity=job_identity,
                   root_fs_usage_identity='not independently observed; operator owned', nonce=nonce))
        for pipe in (child.stdout, child.stderr):
            os.set_blocking(pipe.fileno(), False)
            selector.register(pipe, selectors.EVENT_READ)
        fd = os.open(challenge, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        os.close(fd)
        while selector.get_map() or child.poll() is None:
            try:
                elapsed = time.monotonic() - start
                if not gate and not aborted and elapsed <= 5:
                    challenge.lstat()
                elif not gate:
                    abort('tracer readiness not observed within5s')
                if trace_window(elapsed, child.poll(), bool(selector.get_map())) == 'handoff_unknown' and not handed_back:
                    abort('tracer did not end within30s plus5s margin')
                    write_once(session / 'trace-window.json', dict(state='UNKNOWN', launcher_identity=job_identity,
                               sudo_exit=child.poll(), counts=filtered.counts, nonce=nonce))
                    print('OPERATOR NEEDED: tracer job not closed. PID/identity=' + json.dumps(job_identity) +
                          '. Press Ctrl-C in THIS terminal to request normal sudo interruption; wait. '
                          'Kit sends no signal. Leave foreground if it does not exit.', flush=True)
                    handed_back = True
                for key, _ in selector.select(.1):
                    chunk = os.read(key.fileobj.fileno(), LINE)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                        continue
                    # Never retain anything beyond the observation margin.
                    if elapsed > 35:
                        continue
                    parser = errors if key.fileobj is child.stderr else filtered
                    for event in parser.feed(chunk):
                        if (not gate and not aborted and elapsed <= 5 and event['operation'] in META and
                                str(challenge) in event['owned_paths'] and probe_live(g, ready)):
                            write_once(session / 'gate.json', dict(session=str(session), nonce=nonce,
                                run=ready['run'], probe=ready['probe'], ready_sha256=sha(ready_raw),
                                issued_mono_ns=time.monotonic_ns(), challenge=str(challenge)))
                            gate = True
            except KeyboardInterrupt:
                # Terminal/sudo, not this kit, owns any privileged cancellation.
                aborted = True
                abort('terminal interrupt; no kit signal')
    except BaseException as error:
        failure = type(error).__name__
        try:
            abort('collector fault; stop probe work')
            write_once(session / 'collector-fault.json', dict(error=failure,
                       launcher_identity=job_identity, tracer_closure='UNKNOWN'))
        except Exception:
            pass
        print('OPERATOR NEEDED: collector fault, tracer closure UNKNOWN. Interrupt Terminal A to stop '
              'the probe if control receipts could not be written. PID/identity=' + json.dumps(job_identity) +
              '. Press Ctrl-C here to request normal sudo interruption, then wait; no kit signal.', flush=True)
    finally:
        # No exception drops the live privileged job handle. It stays foreground
        # for human disposition, discarding bounded chunks; never kill/disown.
        drainable = []
        for pipe in (child.stdout, child.stderr):
            if not pipe.closed:
                try:
                    os.set_blocking(pipe.fileno(), False)
                    drainable.append(pipe)
                except OSError:
                    failure = failure or 'pipe state unreadable; operator disposition'
        while child.poll() is None:
            try:
                elapsed = time.monotonic() - start
                if trace_window(elapsed, child.poll()) == 'handoff_unknown' and not handed_back:
                    try:
                        abort('tracer closure unknown at margin')
                    except Exception:
                        pass
                    print('OPERATOR NEEDED: still awaiting tracer job ' + json.dumps(job_identity) +
                          '; Ctrl-C requests normal terminal/sudo interruption; leave foreground and wait.', flush=True)
                    handed_back = True
                for pipe in drainable:
                    if not pipe.closed:
                        try:
                            os.read(pipe.fileno(), LINE)  # discard, never store global output
                        except OSError:
                            pass
                time.sleep(.1)
            except KeyboardInterrupt:
                handed_back = True
                continue
        for pipe in (child.stdout, child.stderr):
            if not pipe.closed:
                pipe.close()
        selector.close()
        filtered.finish()
        errors.finish()
        stream.close()
    status = child.wait()
    write_once(session / 'trace-end.json', dict(sudo_wait_status=status, launcher_identity=job_identity,
               fs_usage_exit='not separately observed', privileged_closure='not independently proven',
               gate_opened=gate, operator_handoff=handed_back, collector_fault=failure,
               stdout_counts=filtered.counts, stderr_counts=errors.counts,
               retained_bytes=filtered.kept, complete_event_coverage=False,
               verdict='INCONCLUSIVE observation; no attribution/qualification', nonce=nonce))
    return 0 if status == 0 and gate and not handed_back and failure is None else 1


def receipts(session, nonce, digest):
    for name in ('stage-receipt.json', 'ready.json', 'tracer-start.json', 'trace-window.json',
                 'trace-end.json', 'probe-route-exit.json'):
        if (session / name).exists():
            print(name + ': ' + private_file(session / name).decode().strip())
    if (session / 'ready.json').exists():
        ready = json.loads(private_file(session / 'ready.json'))
        validate_ready(session, nonce, digest, ready)
        for name in ('trace-probe-receipt.json', 'trace-probe-refusal.json'):
            path = Path(ready['evidence']) / name
            if path.exists():
                print(name + ': ' + private_file(path).decode().strip())


def main(argv):
    if os.geteuid() == 0:
        raise ValueError('kit must never run as root')
    if not argv:
        print('PREPARATION ONLY; execution held. Use --self-check or explicitly authorized --prepare.')
        return 0
    if argv == ['--self-check']:
        import runpy
        runpy.run_path(str(Path(__file__).with_name('test_matching_slot_trace.py')), run_name='__main__')
        return 0
    if argv == ['--prepare']:
        prepare()
        return 0
    if argv == ['--guarded-operator']:
        argv += [os.environ['OXIDE_TRACE280_SESSION'], os.environ['OXIDE_TRACE280_NONCE'],
                 os.environ['OXIDE_TRACE280_MANIFEST']]
    if len(argv) != 4 or argv[0] not in ('--operator-start', '--operator-collect', '--guarded-operator', '--probe', '--receipts'):
        raise ValueError('unknown mode; no execution')
    mode, path, nonce, digest = argv
    session, c = bind(path, nonce, digest)
    if mode == '--operator-start': return operator_start(session, c, nonce, digest)
    if mode == '--operator-collect': return collect(session, c, nonce, digest)
    if mode == '--guarded-operator': return guarded_operator(session, c, nonce, digest)
    if mode == '--probe': return probe(session, c, nonce, digest)
    receipts(session, nonce, digest)
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main(sys.argv[1:]))
    except Exception as error:
        # No raw tracer bytes or arbitrary exception strings escape to terminal.
        print('trace280 refused: ' + type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
