#!/usr/bin/env python3
"""Task #240 private debug launcher; execution still needs the Queue gates.

Context is prepared under a fresh, owner-only Builds directory.  The `source`
child is an unmodified git-archive extraction of HEAD.  The caller supplies a
pre-populated offline cargo home and the existing 1.97.1 toolchain; this script
never installs either.  No inherited Oxide or XDG setting is passed on.

This source is not evidence of isolation: before any use the route needs its
source review, an authorized fresh live before/after inventory and the existing
aggregate wall/disk admission.  It intentionally does not manufacture those
receipts, claim that a subprocess stayed inside its environment, or clean up.
"""
import hashlib
import io
import json
import os
import pwd
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import time

BASE = Path('/Volumes/Builds/scratch/oxide')
BRANCH = 'ns-d41de092-b0b0-495d-b159-8c4d1730d6b4'
SUPERVISOR_SHA256 = 'fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b'
MAX_ARCHIVE = 64 * 1024 * 1024
MAX_LOG_LINE = 8 * 1024 * 1024
PLANNER_AUTHORITY = 'brief-v11:d133e22524df289aba98b74d0c87995d1902a594b5d574edb8035e0a1d12d0d4'
TASK_ID = 'd41de092-b0b0-495d-b159-8c4d1730d6b4'
BLOCK_DEADLINE_NS = None
COVER = {'cover-compile': ['rust:compile'], 'cover-release': ['rust:compile-release'],
         'cover-format': ['rust:format'], 'cover-test': ['test', 'rust'],
         'cover-docs': ['docs:check']}


class Refusal(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Refusal(message)


def digest(path):
    h = hashlib.sha256()
    with open(path, 'rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def private_dir(path, root=None):
    path = Path(path)
    require(path.is_absolute() and os.path.realpath(path) == str(path),
            f'noncanonical directory: {path}')
    if root is not None:
        require(path.is_relative_to(root), f'outside owned root: {path}')
        current = root
        parts = path.relative_to(root).parts
    else:
        current = path
        parts = ()
    for part in ('', *parts):
        if part:
            current = current / part
        st = os.lstat(current)
        require(stat.S_ISDIR(st.st_mode) and st.st_uid == os.getuid()
                and st.st_mode & 0o077 == 0, f'unsafe directory: {current}')
    return path


def private_components(root, path):
    """Validate every existing component, rejecting aliases and foreign owners."""
    require(path.is_relative_to(root), f'path outside context: {path}')
    current = root
    missing = False
    for part in path.relative_to(root).parts:
        current /= part
        if missing:
            continue
        try:
            st = os.lstat(current)
        except FileNotFoundError:
            missing = True
            continue
        require(not stat.S_ISLNK(st.st_mode) and st.st_uid == os.getuid()
                and st.st_mode & 0o077 == 0, f'unsafe destination: {current}')


def private_file(path, root):
    path = Path(path)
    private_dir(path.parent, root)
    st = os.lstat(path)
    require(stat.S_ISREG(st.st_mode) and st.st_uid == os.getuid()
            and st.st_mode & 0o077 == 0 and st.st_nlink == 1,
            f'unsafe file: {path}')
    return path


def git(repo, *args):
    result = subprocess.run(['/usr/bin/git', '-c', 'core.fsmonitor=false',
                             '-c', 'core.untrackedCache=false', '-C', str(repo), *args], check=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            env={'PATH': '/usr/bin:/bin', 'HOME': '/dev/null',
                                 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null'})
    require(len(result.stdout) <= MAX_ARCHIVE, 'git output exceeds bound')
    return result.stdout


def refuse_cargo_configs(source, cargo_home):
    # Cargo searches every cwd ancestor, not just CARGO_HOME. Refuse even a
    # private config until it has a separately bound, reviewed meaning.
    for parent in (source, *source.parents):
        directory = parent / '.cargo'
        try:
            st = os.lstat(directory)
        except FileNotFoundError:
            continue
        require(stat.S_ISDIR(st.st_mode), f'Cargo config ancestor is an alias: {directory}')
        for name in ('config', 'config.toml'):
            try:
                os.lstat(directory / name)
            except FileNotFoundError:
                continue
            raise Refusal(f'ambient Cargo config is not allowed: {directory / name}')
    for name in ('config', 'config.toml'):
        try:
            os.lstat(cargo_home / name)
        except FileNotFoundError:
            continue
        raise Refusal(f'Cargo home config is not allowed: {cargo_home / name}')


def private_cache_tree(path):
    for directory, dirs, files in os.walk(path, followlinks=False,
            onerror=lambda error: (_ for _ in ()).throw(error)):
        for name in dirs + files:
            entry = Path(directory) / name
            st = os.lstat(entry)
            require(st.st_uid == os.getuid() and st.st_mode & 0o077 == 0,
                    f'cargo cache is not private: {entry}')
            require(stat.S_ISDIR(st.st_mode) or stat.S_ISREG(st.st_mode),
                    f'cargo cache has an unbound entry: {entry}')


def source_binding(repo, source):
    require(git(repo, 'branch', '--show-current').strip().decode() == BRANCH,
            'wrong task branch')
    require(not git(repo, 'status', '--porcelain', '--untracked-files=all'),
            'task source is not clean')
    commit = git(repo, 'rev-parse', 'HEAD').strip().decode()
    tree = git(repo, 'rev-parse', 'HEAD^{tree}').strip().decode()
    archive = git(repo, 'archive', '--format=tar', 'HEAD')
    expected = {}
    modes = {}
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:') as tar:
        for member in tar:
            name = member.name.rstrip('/')
            require(name and not name.startswith('/') and '..' not in Path(name).parts,
                    'unsafe archive entry')
            if member.isdir():
                continue
            require(member.isfile(), f'unsupported archive entry: {name}')
            item = tar.extractfile(member)
            require(item is not None, f'unreadable archive entry: {name}')
            expected[name] = hashlib.sha256(item.read()).hexdigest()
            modes[name] = member.mode & 0o111
    actual = {}
    for directory, dirs, files in os.walk(source, followlinks=False):
        rel_dir = Path(directory).relative_to(source)
        if rel_dir == Path('.') and 'target' in dirs:
            dirs.remove('target')  # Oxide owns this checkout's target link.
        for name in dirs:
            require(not (Path(directory) / name).is_symlink(), 'source directory symlink')
        for name in files:
            path = Path(directory) / name
            require(not path.is_symlink() and path.is_file(), 'non-regular staged source')
            relative = str(path.relative_to(source))
            require(path.stat().st_mode & 0o111 == modes.get(relative), 'staged executable mode differs')
            actual[relative] = digest(path)
    require(actual == expected, 'staged source differs from git archive')
    return {'commit': commit, 'tree': tree, 'files': expected,
            'lock_sha256': expected['Cargo.lock'],
            'toolchain_file_sha256': expected['rust-toolchain.toml']}


def skill_binding(source, staged, root):
    """Bind the staged Northstar skill used by docs:check, without links."""
    require(source.is_absolute() and source.resolve(strict=True) == source and source.is_dir(),
            'TAPE_NORTHSTAR_DIR must be canonical')
    private_dir(staged, root)
    def collect(base, private):
        files = {}
        size = 0
        for directory, dirs, names in os.walk(base, followlinks=False,
                onerror=lambda error: (_ for _ in ()).throw(error)):
            for name in dirs + names:
                path = Path(directory) / name
                st = os.lstat(path)
                require(not stat.S_ISLNK(st.st_mode), 'skill symlink is unbound')
                if private:
                    require(st.st_uid == os.getuid() and st.st_mode & 0o077 == 0,
                            'staged skill is not private')
                if stat.S_ISREG(st.st_mode):
                    size += st.st_size
                    require(size <= 64 * 1024 * 1024 and len(files) < 10000, 'skill bound exceeded')
                    files[str(path.relative_to(base))] = digest(path)
                else:
                    require(stat.S_ISDIR(st.st_mode), 'unknown skill entry')
        return files
    expected = collect(source, False)
    require(expected and collect(staged, True) == expected, 'staged Northstar skill differs')
    return expected


def context():
    root = Path(os.environ.get('TAPE_RUN_ROOT', ''))
    require(root.is_absolute() and root != BASE and root.is_relative_to(BASE),
            'TAPE_RUN_ROOT must be a fresh child of Builds oxide scratch')
    private_dir(BASE)
    private_dir(root, BASE)
    paths = {'work': private_dir(root / 'work', root),
             'bootstrap-target': private_dir(root / 'bootstrap-target', root)}
    paths.update({name: private_dir(paths['work'] / name, root) for name in
                  ('source', 'home', 'cargo-home', 'scratch', 'bin')})
    paths['tmp'] = private_dir(paths['scratch'] / 'tmp', root)
    target = paths['source'] / 'target'
    if target.exists() or target.is_symlink():
        resolved = Path(os.path.realpath(target))
        require(resolved.is_relative_to(paths['scratch']), 'checkout target escapes private scratch')
        private_dir(resolved, root)
    config = private_file(paths['work'] / 'config.toml', root)
    for path in (paths['home'] / 'Library/Application Support/oxide/state/pools.json',
                 paths['home'] / 'Library/Application Support/oxide/state/scratch-pools',
                 paths['home'] / 'Library/Caches/oxide',
                 paths['home'] / '.config/oxide/config.toml',
                 paths['scratch'] / 'pool/oxide-managed/outputs/registry.json'):
        private_components(root, path)
    require(config.stat().st_size == 0, 'private config must be empty')
    repo = Path(os.environ.get('TAPE_REPOSITORY', ''))
    require(repo.is_absolute() and os.path.realpath(repo) == str(repo), 'repository must be canonical')
    rustup = Path(os.environ.get('RUSTUP_HOME', ''))
    toolchain = Path(os.environ.get('TAPE_TOOLCHAIN_BIN', ''))
    require(rustup.is_absolute() and os.path.realpath(rustup) == str(rustup), 'RUSTUP_HOME invalid')
    require(toolchain.is_absolute() and os.path.realpath(toolchain) == str(toolchain)
            and toolchain.parent.parent == rustup / 'toolchains'
            and toolchain.parent.name.startswith('1.97.1-') and toolchain.name == 'bin',
            'expected the existing 1.97.1 toolchain bin')
    for name in ('cargo', 'rustc', 'rustfmt', 'rustdoc'):
        program = toolchain / name
        require(program.is_file() and program.resolve(strict=True).parent == toolchain
                and os.access(program, os.X_OK), f'unbound toolchain executable: {name}')
    refuse_cargo_configs(paths['source'], paths['cargo-home'])
    private_cache_tree(paths['cargo-home'])
    binding = source_binding(repo, paths['source'])
    require(binding['files'].get('scripts/bounded-test.py') == SUPERVISOR_SHA256,
            'shared supervisor bytes are not the qualified version')
    skill = Path(os.environ.get('TAPE_NORTHSTAR_DIR', ''))
    binding['northstar_files'] = skill_binding(skill, paths['home'] / '.agents/skills/northstar', root)
    own_script = Path(__file__).resolve(strict=True)
    require(own_script in ((repo / 'scripts/tape-isolation-check.py').resolve(),
                           (paths['source'] / 'scripts/tape-isolation-check.py').resolve())
            and digest(own_script) == binding['files']['scripts/tape-isolation-check.py'],
            'launcher does not match staged source')
    python = Path(sys.executable).resolve(strict=True)
    binding['python_executable'] = str(python)
    binding['python_sha256'] = digest(python)
    binding['rustc_sha256'] = digest(toolchain / 'rustc')
    binding['cargo_sha256'] = digest(toolchain / 'cargo')
    binding['rustfmt_sha256'] = digest(toolchain / 'rustfmt')
    binding['rustdoc_sha256'] = digest(toolchain / 'rustdoc')
    binding['rustup_home'] = str(rustup)
    system_tools = ('/usr/bin/env', '/bin/sh', '/usr/bin/git', '/usr/bin/python3',
                    '/usr/sbin/diskutil', '/usr/bin/plutil')
    binding['system_tools'] = {}
    for name in system_tools:
        path = Path(name).resolve(strict=True)
        require(path.is_file() and os.access(path, os.X_OK), f'system executable unavailable: {name}')
        binding['system_tools'][name] = {'resolved': str(path), 'sha256': digest(path)}
    effigy = Path(os.environ.get('TAPE_EFFIGY', ''))
    require(effigy.is_absolute() and effigy.resolve(strict=True) == effigy
            and effigy.is_file() and os.access(effigy, os.X_OK), 'TAPE_EFFIGY must be a canonical executable')
    binding['effigy_executable'] = str(effigy)
    binding['effigy_sha256'] = digest(effigy)
    links = {entry.name: entry for entry in paths['bin'].iterdir()}
    expected_links = {'oxide', 'effigy'} if (root / 'bootstrap-receipt.json').exists() else set()
    require(set(links) == expected_links, 'private executable bindings are missing or foreign')
    for name, target_path in (('oxide', paths['bootstrap-target'] / 'debug/oxide'), ('effigy', effigy)):
        if name in links:
            require(links[name].is_symlink() and Path(os.readlink(links[name])) == target_path,
                    f'private {name} binding differs')
    env = {'PATH': f"{paths['bin']}:{toolchain}:/usr/bin:/bin:/usr/sbin:/sbin",

           'HOME': str(paths['home']), 'CARGO_HOME': str(paths['cargo-home']),
           'RUSTUP_HOME': str(rustup), 'RUSTUP_TOOLCHAIN': '1.97.1',
           'RUSTUP_AUTO_INSTALL': '0', 'RUSTUP_NO_UPDATE_CHECK': '1',
           'RUSTC': str(toolchain / 'rustc'), 'RUSTDOC': str(toolchain / 'rustdoc'),
           'OXIDE_TEST_HOME_FROM_ENV': '1', 'OXIDE_CONFIG': str(config),
           'OXIDE_POOL': str(paths['scratch'] / 'pool'), 'OXIDE_POOL_CAP': '1073741824',
           'OXIDE_SCRATCH_ROOT': str(paths['scratch']),
           'OXIDE_TASK_SCRATCH_ROOT': str(paths['work']),
           'TMPDIR': str(paths['tmp']), 'TMP': str(paths['tmp']), 'TEMP': str(paths['tmp']),
           'CARGO_NET_OFFLINE': 'true', 'PYTHONDONTWRITEBYTECODE': '1',
           'CARGO_TARGET_DIR': str(paths['bootstrap-target']),
           'TAPE_TOOLCHAIN_BIN': str(toolchain)}
    return root, paths, toolchain, binding, env


def receipt(path, value):
    # Exclusive creation preserves earlier packets, even on failure.
    data = json.dumps(value, sort_keys=True, indent=2).encode() + b'\n'
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as out:
        out.write(data)
        out.flush()
        os.fsync(out.fileno())


def inventory_account():
    """Bounded no-follow inventory. Any unknown is a refusal, not an empty set."""
    account = Path(pwd.getpwuid(os.getuid()).pw_dir)
    live = account / 'Library/Application Support/oxide'
    rows = []
    total = 0
    flags = os.O_RDONLY | os.O_NOFOLLOW

    def walk(fd, prefix):
        nonlocal total
        for name in sorted(os.listdir(fd)):
            require(name not in ('.', '..') and '/' not in name, 'invalid inventory name')
            st = os.stat(name, dir_fd=fd, follow_symlinks=False)
            rel = f'{prefix}/{name}' if prefix else name
            row = {'path': rel, 'dev': st.st_dev, 'ino': st.st_ino, 'mode': st.st_mode,
                   'uid': st.st_uid, 'gid': st.st_gid, 'size': st.st_size,
                   'mtime_ns': st.st_mtime_ns}
            rows.append(row)
            require(len(rows) <= 100000, 'live inventory row cap')
            if stat.S_ISDIR(st.st_mode):
                child = os.open(name, flags | os.O_DIRECTORY, dir_fd=fd)
                try:
                    opened = os.fstat(child)
                    require((opened.st_dev, opened.st_ino) == (st.st_dev, st.st_ino), 'inventory directory replaced')
                    walk(child, rel)
                    after = os.fstat(child)
                    require((opened.st_ino, opened.st_mtime_ns) == (after.st_ino, after.st_mtime_ns), 'inventory directory changed')
                finally:
                    os.close(child)
            elif stat.S_ISREG(st.st_mode):
                total += st.st_size
                require(total <= 128 * 1024 * 1024, 'live inventory byte cap')
                child = os.open(name, flags, dir_fd=fd)
                try:
                    opened = os.fstat(child)
                    require((opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns) ==
                            (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns), 'inventory file replaced')
                    h = hashlib.sha256()
                    while True:
                        block = os.read(child, 1024 * 1024)
                        if not block:
                            break
                        h.update(block)
                    after = os.fstat(child)
                    require((opened.st_ino, opened.st_size, opened.st_mtime_ns) ==
                            (after.st_ino, after.st_size, after.st_mtime_ns), 'inventory file changed')
                    row['sha256'] = h.hexdigest()
                finally:
                    os.close(child)
            elif stat.S_ISLNK(st.st_mode):
                row['link'] = os.readlink(name, dir_fd=fd)
            else:
                raise Refusal(f'unsupported live inventory entry: {rel}')
            end = os.stat(name, dir_fd=fd, follow_symlinks=False)
            require((end.st_dev, end.st_ino, end.st_mode, end.st_size, end.st_mtime_ns) ==
                    (st.st_dev, st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns), 'inventory entry changed')

    # Hold and recheck every parent so an alias or replacement cannot turn
    # a partial walk into a clean inventory.
    parents = []
    fd = os.open('/', flags | os.O_DIRECTORY)
    try:
        for part in live.parts[1:]:
            entry = os.stat(part, dir_fd=fd, follow_symlinks=False)
            require(stat.S_ISDIR(entry.st_mode), 'inventory parent is not a real directory')
            child = os.open(part, flags | os.O_DIRECTORY, dir_fd=fd)
            opened = os.fstat(child)
            require((entry.st_dev, entry.st_ino) == (opened.st_dev, opened.st_ino), 'inventory parent replaced')
            parents.append((fd, part, entry.st_dev, entry.st_ino))
            fd = child
        st = os.fstat(fd)
        rows.append({'path': '.', 'dev': st.st_dev, 'ino': st.st_ino, 'mode': st.st_mode,
                     'uid': st.st_uid, 'gid': st.st_gid, 'size': st.st_size, 'mtime_ns': st.st_mtime_ns})
        walk(fd, '')
        after = os.fstat(fd)
        require((st.st_dev, st.st_ino, st.st_mtime_ns) ==
                (after.st_dev, after.st_ino, after.st_mtime_ns), 'inventory root changed')
        for parent, name, dev, ino in parents:
            entry = os.stat(name, dir_fd=parent, follow_symlinks=False)
            require((entry.st_dev, entry.st_ino) == (dev, ino), 'inventory parent changed')
    finally:
        os.close(fd)
        for parent, _, _, _ in reversed(parents):
            os.close(parent)
    return rows


def require_original_charge(charged):
    require(isinstance(charged, (int, float)) and not isinstance(charged, bool)
            and charged == 1200, 'original allowance must be charged as 1200, not measured')


def require_remaining(deadline_ns, now_ns):
    require(deadline_ns is not None and now_ns + 300_000_000_000 <= deadline_ns,
            'single prospective block has insufficient remaining wall')


def require_child_marker(block, approval_sha, used):
    require(used.get('one_use_id') == block['one_use_id']
            and used.get('approval_sha256') == approval_sha, 'block marker mismatch')


def budget(root, paths, action):
    """Count retained bytes; the one prospective wall block is separate."""
    ledger_path = private_file(root / 'budget.json', root)
    ledger = json.loads(ledger_path.read_text())
    require(ledger.get('task') == TASK_ID
            and ledger.get('history_reference'), 'missing task budget history')
    charged = ledger.get('covering_seconds_charged')
    require_original_charge(charged)
    roots = [Path(value) for value in ledger.get('retained_roots', [])]
    require(root in roots and len(roots) >= 5 and len(roots) == len(set(roots)), 'incomplete retained root list')
    require(all(path.is_absolute() and path != BASE and path.is_relative_to(BASE) for path in roots),
            'retained root outside Builds scratch')
    require(not any(a != b and a.is_relative_to(b) for a in roots for b in roots), 'overlapping retained roots')
    def size_of(path):
        count = 0
        for directory, dirs, files in os.walk(path, followlinks=False, onerror=lambda error: (_ for _ in ()).throw(error)):
            for name in dirs + files:
                st = os.lstat(Path(directory) / name)
                require(st.st_uid == os.getuid(), 'foreign retained scratch entry')
                require(stat.S_ISDIR(st.st_mode) or stat.S_ISREG(st.st_mode) or stat.S_ISLNK(st.st_mode),
                        'unknown retained scratch entry')
                count += max(st.st_size, st.st_blocks * 512)
        return count
    sizes = {}
    for path in roots:
        private_dir(path, BASE)
        sizes[path] = size_of(path)
    require(sum(size for path, size in sizes.items() if path != root) >= 3468 * 1024 * 1024,
            'retained roots do not cover the historical baseline')
    require_remaining(BLOCK_DEADLINE_NS, time.clock_gettime_ns(time.CLOCK_UPTIME_RAW))
    monitored = sizes[root] if action == 'bootstrap' else size_of(paths['work'])
    outside = sum(sizes.values()) - monitored
    remaining_mib = (6 * 1024**3 - outside) // (1024 * 1024)
    per_run = min(1024 if action == 'bootstrap' else 512, remaining_mib)
    require(per_run > 0 and monitored <= per_run * 1024 * 1024, 'scratch budget unavailable')
    return int(per_run), digest(ledger_path)


def revalidate_sources(binding, paths, env):
    # Re-resolve the whole private context: source, toolchain, launcher,
    # Northstar copy, executable links and every existing destination.
    _, after_paths, _, after_binding, _ = context()
    require(after_paths == paths and after_binding == binding,
            'source, executable or private destination changed during command')


def retained_inventory(root):
    """Bounded no-follow point-in-time metadata/size inventory of other roots.

    Equal snapshots do not exclude transient growth or prove a continuous peak.
    """
    ledger = json.loads(private_file(root / 'budget.json', root).read_text())
    roots = [Path(value) for value in ledger['retained_roots'] if Path(value) != root]
    snapshots = []
    count = 0
    for other in roots:
        private_dir(other, BASE)
        rows = []
        size = 0
        for directory, dirs, files in os.walk(other, followlinks=False,
                onerror=lambda error: (_ for _ in ()).throw(error)):
            for name in ['.', *sorted(dirs), *sorted(files)]:
                path = Path(directory) if name == '.' else Path(directory) / name
                st = os.lstat(path)
                require(st.st_uid == os.getuid() and (stat.S_ISDIR(st.st_mode)
                        or stat.S_ISREG(st.st_mode) or stat.S_ISLNK(st.st_mode)),
                        'unknown or foreign retained entry')
                count += 1
                require(count <= 200000, 'retained inventory row cap')
                if name != '.':
                    size += max(st.st_size, st.st_blocks * 512)
                row = [str(path.relative_to(other)), st.st_dev, st.st_ino, st.st_mode,
                       st.st_uid, st.st_gid, st.st_size, st.st_blocks, st.st_mtime_ns,
                       st.st_ctime_ns]
                if stat.S_ISLNK(st.st_mode):
                    row.append(os.readlink(path))
                rows.append(row)
        rows.sort(key=lambda row: row[0])
        encoded = json.dumps(rows, separators=(',', ':'), ensure_ascii=True).encode()
        snapshots.append({'root': str(other), 'allocated_or_logical_bytes': size,
                          'rows': len(rows), 'inventory_sha256': hashlib.sha256(encoded).hexdigest()})
    require(snapshots, 'no retained-root inventory')
    return snapshots


def guarded_run(root, paths, env, action, argv, binding):
    scratch_mib, ledger_sha256 = budget(root, paths, action)
    env = dict(env, OXIDE_TASK_SCRATCH_ROOT=str(root if action == 'bootstrap' else paths['work']))
    retained_before = retained_inventory(root)
    require(sum(item['allocated_or_logical_bytes'] for item in retained_before)
            + scratch_mib * 1024 * 1024 <= 6 * 1024**3,
            'retained bytes and monitored bound exceed aggregate scratch ceiling')
    receipt(root / (action + '-retained-before.json'), retained_before)
    before = inventory_account()
    require(len(before) > 1 and any(row.get('path') == 'state/pools.json' for row in before),
            'live inventory is empty or lacks the expected state registry')
    receipt(root / (action + '-inventory-before.json'), before)
    error = None
    command = None
    try:
        command = run_bounded(paths, env, argv, paths['tmp'] / (action + '.log'),
                              scratch_mib)
    except (Refusal, OSError, subprocess.SubprocessError) as failure:
        error = str(failure)
    receipt(root / (action + '-result.json'), {'command': command, 'error': error,
            'environment': env, 'budget_ledger_sha256': ledger_sha256})
    after = inventory_account()
    receipt(root / (action + '-inventory-after.json'), after)
    retained_after = retained_inventory(root)
    receipt(root / (action + '-retained-after.json'), retained_after)
    require(retained_before == retained_after,
            f'retained roots changed during {action}; aggregate peak is unproven')
    require(before == after, f'live inventory changed during {action}; inspect retained before/after packets')
    require(error is None and command is not None, f'{action} failed: {error}')
    require(command['exit'] == 0, f'{action} failed; inspect retained result')
    revalidate_sources(binding, paths, env)
    receipt(root / (action + '-source-revalidated.json'), {'commit': binding['commit'], 'tree': binding['tree']})
    command['budget_ledger_sha256'] = ledger_sha256
    return command


def run_bounded(paths, env, argv, log, scratch_mib):
    require(not log.exists() and not log.is_symlink(), 'command log already exists')
    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    command = [str(Path(sys.executable).resolve(strict=True)), str(paths['source'] / 'scripts/bounded-test.py'),
               '--limit', '300', '--scratch-budget-mib', str(scratch_mib), '--', *map(str, argv)]
    started = time.monotonic()
    with os.fdopen(fd, 'wb') as output:
        result = subprocess.run(command, cwd=paths['source'], env=env, stdout=output,
                                stderr=subprocess.STDOUT, check=False)
    elapsed = time.monotonic() - started
    return {'argv': command, 'elapsed_seconds': elapsed, 'exit': result.returncode,
            'log_sha256': digest(log) if result.returncode == 0 else None}


def log_lines(path):
    """Stream the retained log; reject an unbounded individual artifact row."""
    with open(path, 'rb') as stream:
        while True:
            line = stream.readline(MAX_LOG_LINE + 1)
            if not line:
                return
            require(len(line) <= MAX_LOG_LINE, 'artifact log row exceeds parser bound')
            yield line


def run_action(action):
    os.umask(0o077)
    root, paths, toolchain, binding, env = context()
    # A failed, interrupted or changed inventory can never be erased by
    # starting another action in this context.
    success = {'bootstrap': 'bootstrap-receipt.json',
               'preflight-build': 'harness-receipt.json',
               'preflight': 'preflight-gate.json',
               'test': 'test-success.json', 'self-check': 'self-check-success.json',
               **{phase: phase + '-success.json' for phase in COVER}}
    for phase, completed in success.items():
        before = root / (phase + '-inventory-before.json')
        after = root / (phase + '-inventory-after.json')
        if before.exists() or after.exists():
            private_file(before, root)
            private_file(after, root)
            require(json.loads(before.read_text()) == json.loads(after.read_text())
                    and (root / completed).exists(), f'prior {phase} did not close cleanly')
    require(not (root / (action + '-inventory-before.json')).exists(), 'action already attempted')
    binary = paths['bootstrap-target'] / 'debug' / 'oxide'
    bootstrap_receipt = root / 'bootstrap-receipt.json'
    if action == 'bootstrap':
        require(not any(paths['bootstrap-target'].iterdir())
                and not (paths['source'] / 'target').exists()
                and not (paths['scratch'] / 'pool').exists(), 'bootstrap context is not fresh')
        command = guarded_run(root, paths, env, 'bootstrap',
                              [Path(sys.executable).resolve(strict=True), paths['source'] / 'scripts/tape-bootstrap.py'], binding)
        private_file(binary, root)
        require(os.access(binary, os.X_OK), 'debug oxide not executable')
        version_line = [line for line in log_lines(paths['tmp'] / 'bootstrap.log')
                        if line.startswith(b'rustc 1.97.1 ')]
        require(len(version_line) == 1, 'toolchain receipt missing or ambiguous')
        binding.update({'command': command, 'elapsed_seconds': command['elapsed_seconds'],
                        'rustc_version': version_line[0].decode('utf-8', errors='strict').rstrip(),
                        'bootstrap_log_sha256': digest(paths['tmp'] / 'bootstrap.log'),
                        'oxide_sha256': digest(binary)})
        os.symlink(binary, paths['bin'] / 'oxide')
        os.symlink(binding['effigy_executable'], paths['bin'] / 'effigy')
        receipt(bootstrap_receipt, binding)
        return
    private_file(bootstrap_receipt, root)
    recorded = json.loads(bootstrap_receipt.read_text())
    require(all(recorded.get(key) == value for key, value in binding.items()), 'bootstrap source binding changed')
    private_file(binary, root)
    require(os.access(binary, os.X_OK) and digest(binary) == recorded.get('oxide_sha256'),
            'debug oxide digest changed')
    # Cargo and every ordinary subprocess receive this same scrubbed context.
    # A separate source audit and inventory gate must still establish what
    # test/build subprocesses do with the environment they receive.
    if action == 'preflight-build':
        command = guarded_run(root, paths, env, 'preflight-build',
                              [binary, 'test', '--test', 'tape_prune_eval', '--no-run',
                               '--locked', '--message-format=json'], binding)
        log = paths['tmp'] / 'preflight-build.log'
        artifacts = []
        for line in log_lines(log):
            try:
                item = json.loads(line)
            except (ValueError, UnicodeDecodeError):
                continue
            if item.get('reason') == 'compiler-artifact' and item.get('target', {}).get('name') == 'tape_prune_eval' and item.get('executable'):
                artifacts.append(Path(item['executable']))
        require(len(artifacts) == 1, 'no unique harness artifact in Cargo output')
        harness = artifacts[0].resolve(strict=True)
        require(harness.is_relative_to(paths['work']), 'harness escaped owned work')
        private_file(harness, root)
        require(os.access(harness, os.X_OK), 'harness is not executable')
        receipt(root / 'harness-receipt.json', {'source': binding, 'oxide_sha256': digest(binary),
                'command': command, 'elapsed_seconds': command['elapsed_seconds'], 'harness': str(harness), 'harness_sha256': digest(harness)})
        return
    if action == 'preflight':
        private_file(root / 'harness-receipt.json', root)
        captured = json.loads((root / 'harness-receipt.json').read_text())
        require(captured.get('source') == binding and captured.get('oxide_sha256') == digest(binary), 'harness source binding changed')
        harness = Path(captured['harness'])
        private_file(harness, root)
        require(harness.resolve(strict=True) == harness and harness.is_relative_to(paths['work'])
                and digest(harness) == captured['harness_sha256'], 'harness digest changed')
        fixture = paths['work']
        fixture_env = dict(env, HOME=str(fixture / 'home'), OXIDE_CONFIG=str(fixture / 'config.toml'),
                           OXIDE_POOL=str(fixture / 'scratch/pool'), OXIDE_SCRATCH_ROOT=str(fixture / 'scratch'))
        private_dir(fixture / 'home', root)
        private_dir(fixture / 'scratch', root)
        private_file(fixture / 'config.toml', root)
        allowed = ('HOME', 'OXIDE_TEST_HOME_FROM_ENV', 'OXIDE_CONFIG', 'OXIDE_POOL',
                   'OXIDE_POOL_CAP', 'OXIDE_SCRATCH_ROOT')
        child = ['/usr/bin/env', '-i', 'PATH=/usr/bin:/bin:/usr/sbin:/sbin',
                 *[f'{key}={fixture_env[key]}' for key in allowed],
                 harness, '--path-preflight', fixture]
        command = guarded_run(root, paths, fixture_env, 'preflight', child, binding)
        receipt(root / 'preflight-gate.json', {'source': binding, 'oxide_sha256': digest(binary),
                'elapsed_seconds': command['elapsed_seconds'], 'harness_sha256': digest(harness)})
        return
    private_file(root / 'preflight-gate.json', root)
    private_file(root / 'harness-receipt.json', root)
    gate = json.loads((root / 'preflight-gate.json').read_text())
    captured = json.loads((root / 'harness-receipt.json').read_text())
    harness = Path(captured['harness'])
    private_file(harness, root)
    require(gate.get('source') == binding and gate.get('oxide_sha256') == digest(binary)
            and captured.get('source') == binding
            and harness.resolve(strict=True) == harness and harness.is_relative_to(paths['work'])
            and digest(harness) == gate.get('harness_sha256') == captured.get('harness_sha256'),
            'preflight or exported harness binding is stale')
    if action in COVER:
        args = [binding['effigy_executable'], *COVER[action]]
        if action == 'cover-test':
            # Only the existing Rust suite's approved boot-Data fixture root.
            # The outer guard still monitors Builds; its child uses the nested
            # suite guard on this separately owned, internal temporary root.
            internal = Path(os.environ.get('TAPE_TEST_INTERNAL_ROOT', ''))
            internal_base = Path('/private/tmp')
            require(internal.is_absolute() and internal != internal_base
                    and internal.is_relative_to(internal_base), 'missing suite internal root')
            private_dir(internal)
            require(not any(internal.iterdir()), 'suite internal root is not fresh')
            args = ['/usr/bin/env', f'OXIDE_TASK_SCRATCH_ROOT={internal}',
                    f'OXIDE_TEST_INTERNAL_ROOT={internal}', *args]
    else:
        args = [harness]
    if action == 'self-check':
        args.append('--self-check')
    command = guarded_run(root, paths, env, action, args, binding)
    receipt(root / (action + '-success.json'), {'source': binding,
            'harness_sha256': digest(harness), 'elapsed_seconds': command['elapsed_seconds'], 'command': command})


def block_inputs(root, binding):
    """Check a separately planner-bound authorization; it is not self-issued."""
    approval = private_file(root / 'block-authorization.json', root)
    expected = os.environ.get('TAPE_BLOCK_APPROVAL_SHA256', '')
    require(len(expected) == 64 and all(c in '0123456789abcdef' for c in expected)
            and digest(approval) == expected, 'missing or mismatched planner block binding')
    block = json.loads(approval.read_text())
    require(isinstance(block, dict) and set(block) == {
        'task', 'root', 'planner_reference', 'original_admission_charge_seconds',
        'prospective_max_seconds', 'source_commit', 'source_tree', 'binding_sha256',
        'ledger_sha256', 'history_sha256', 'one_use_id', 'retained_roots_quiescent',
    }, 'invalid block authorization fields')
    ledger = private_file(root / 'budget.json', root)
    history = private_file(root / 'history-discovery.json', root)
    ledger_doc = json.loads(ledger.read_text())
    history_doc = json.loads(history.read_text())
    binding_bytes = json.dumps(binding, sort_keys=True, separators=(',', ':')).encode()
    require(block.get('task') == TASK_ID and block.get('root') == str(root)
            and block.get('planner_reference') == PLANNER_AUTHORITY
            and block.get('original_admission_charge_seconds') == 1200
            and block.get('prospective_max_seconds') == 1200
            and block.get('source_commit') == binding['commit']
            and block.get('source_tree') == binding['tree']
            and block.get('binding_sha256') == hashlib.sha256(binding_bytes).hexdigest()
            and block.get('ledger_sha256') == digest(ledger)
            and block.get('history_sha256') == digest(history),
            'planner block does not bind this context, history and source')
    require(history_doc.get('complete') is True
            and history_doc.get('task') == TASK_ID
            and history_doc.get('retained_roots') == ledger_doc.get('retained_roots')
            and block.get('retained_roots_quiescent') is True,
            'incomplete root discovery or no quiescence admission')
    block_id = block.get('one_use_id')
    require(isinstance(block_id, str) and len(block_id) == 64
            and all(c in '0123456789abcdef' for c in block_id), 'invalid one-use block identity')
    return block, expected


def main():
    """A single supervisor owns the entire prospective block, including setup."""
    global BLOCK_DEADLINE_NS
    os.umask(0o077)
    require(len(sys.argv) == 2 and sys.argv[1] == 'sequence',
            'individual actions are disabled; use one planner-bound sequence')
    started_ns = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    BLOCK_DEADLINE_NS = started_ns + 1_200_000_000_000
    root = Path(os.environ.get('TAPE_RUN_ROOT', ''))
    require(root.is_absolute() and root != BASE and root.is_relative_to(BASE), 'invalid block root')
    private_dir(BASE)
    private_dir(root, BASE)
    approval = private_file(root / 'block-authorization.json', root)
    approval_sha = os.environ.get('TAPE_BLOCK_APPROVAL_SHA256', '')
    require(len(approval_sha) == 64 and all(c in '0123456789abcdef' for c in approval_sha)
            and digest(approval) == approval_sha, 'missing or mismatched planner block binding')
    block = json.loads(approval.read_text())
    require(isinstance(block, dict) and block.get('task') == TASK_ID
            and block.get('root') == str(root)
            and block.get('planner_reference') == PLANNER_AUTHORITY
            and isinstance(block.get('one_use_id'), str)
            and len(block['one_use_id']) == 64
            and all(c in '0123456789abcdef' for c in block['one_use_id']),
            'invalid initial block authorization')
    # Exclusive marker is durable for this root/id even after an interruption.
    # The planner binds the unique id to this root; cross-root uniqueness and
    # receipt authenticity are planner admission, not a claim from this file.
    receipt(root / 'block-used.json', {'one_use_id': block['one_use_id'],
            'approval_sha256': approval_sha, 'started_uptime_raw_ns': started_ns})
    root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(root_fd)
    finally:
        os.close(root_fd)
    root2, paths, _, binding, env = context()
    require(root2 == root, 'block context root changed')
    block_inputs(root, binding)
    scratch_mib, _ = budget(root, paths, 'bootstrap')
    remaining = (BLOCK_DEADLINE_NS - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) // 1_000_000_000
    require(remaining >= 300, 'setup exhausted prospective block')
    # The outer existing supervisor owns the wall and process custody. Inner
    # actions retain their own 300 s / scratch bounds and all their inventory
    # work falls inside this one outer deadline. There is no restart path.
    child_env = dict(env, TAPE_RUN_ROOT=str(root),
                     TAPE_REPOSITORY=os.environ['TAPE_REPOSITORY'],
                     TAPE_NORTHSTAR_DIR=os.environ['TAPE_NORTHSTAR_DIR'],
                     TAPE_EFFIGY=os.environ['TAPE_EFFIGY'],
                     TAPE_BLOCK_APPROVAL_SHA256=approval_sha,
                     OXIDE_TASK_SCRATCH_ROOT=str(root))
    if 'TAPE_TEST_INTERNAL_ROOT' in os.environ:
        child_env['TAPE_TEST_INTERNAL_ROOT'] = os.environ['TAPE_TEST_INTERNAL_ROOT']
    command = [str(Path(sys.executable).resolve(strict=True)), str(paths['source'] / 'scripts/bounded-test.py'),
               '--limit', str(remaining), '--scratch-budget-mib', str(scratch_mib), '--',
               str(Path(sys.executable).resolve(strict=True)),
               str(paths['source'] / 'scripts/tape-isolation-check.py'), '--sequence-child']
    result = subprocess.run(command, cwd=paths['source'], env=child_env, check=False)
    elapsed_ns = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - started_ns
    receipt(root / 'block-result.json', {'one_use_id': block['one_use_id'],
            'clock': 'CLOCK_UPTIME_RAW', 'elapsed_ns': elapsed_ns, 'exit': result.returncode})
    require(result.returncode == 0, f'prospective block failed: {result.returncode}')


def sequence_child():
    global BLOCK_DEADLINE_NS
    os.umask(0o077)
    root, _, _, binding, _ = context()
    evidence = Path(os.environ.get('OXIDE_PROOF_EVIDENCE', ''))
    run_id = os.environ.get('OXIDE_PROOF_RUN_ID', '')
    deadline_ns = os.environ.get('OXIDE_PROOF_DEADLINE_NS', '')
    require(evidence.is_absolute() and evidence.is_relative_to(root)
            and len(run_id) == 32 and all(c in '0123456789abcdef' for c in run_id)
            and deadline_ns.isdigit(), 'missing outer supervisor context')
    private_dir(evidence, root)
    block, approval_sha = block_inputs(root, binding)
    marker = private_file(root / 'block-used.json', root)
    used = json.loads(marker.read_text())
    require_child_marker(block, approval_sha, used)
    BLOCK_DEADLINE_NS = int(deadline_ns)
    require(BLOCK_DEADLINE_NS > time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
            'supervisor deadline is already spent')
    receipt(root / 'block-child-started.json', {'one_use_id': block['one_use_id']})
    for action in ('bootstrap', 'preflight-build', 'preflight', 'test',
                   'self-check', 'cover-compile', 'cover-release', 'cover-format',
                   'cover-test', 'cover-docs'):
        run_action(action)
    receipt(root / 'block-complete.json', {'one_use_id': block['one_use_id']})


if __name__ == '__main__':
    try:
        if sys.argv[1:] == ['--sequence-child']:
            sequence_child()
        else:
            main()
    except (Refusal, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        sys.exit(f'oxide#240 private route refused: {error}')
