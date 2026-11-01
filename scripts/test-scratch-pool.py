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
    print("S3: cases 01-15; two sequential 128 MiB owned APFS images; "
          "at most one attached; 300 s; 512 MiB; private fixture account")


def self_check():
    custody = custody_document('/owned/A', '/owned/B')
    assert custody == {'version': 1, 'backings': ['/owned/A', '/owned/B']}
    assert may_detach(True, False) and not may_detach(False, False)
    assert not may_detach(True, True)
    assert detached(True, []) and not detached(False, []) and not detached(True, ['/dev/disk1'])
    assert may_attach(False) and not may_attach(True)
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
        print('mounted', self.mount, device, uuid, flush=True)
        return uuid

    def detach(self):
        require(self.attached is not None, 'no attachment')
        self.request.unlink()
        require(may_detach(self.children_reaped, self.request.exists()), 'detach precondition')
        p = subprocess.run(['/usr/bin/hdiutil', 'detach', self.attached['device']],
                           stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           timeout=min(30, remaining()))
        require(p.returncode == 0, 'detach failed; both backings retained')
        info = run(['/usr/bin/hdiutil', 'info', '-plist'])
        require(info.returncode == 0, 'attachment post-state unreadable')
        images = plistlib.loads(info.stdout)['images']
        entities = [e for image in images if image.get('image-path') == self.attached['backing']
                    for e in image.get('system-entities', [])]
        require(detached(not os.path.ismount(self.mount), entities), 'detach post-state unknown')
        self.attached = None
