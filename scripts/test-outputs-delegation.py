#!/usr/bin/env python3
"""Integrated Oxide/Barge fixture proof (304). Native modes need planner activation.

Composition of the qualified shared guard, existing private source/seed builder,
image custody and direct-child helpers. No new supervisor, reserve sampler,
cleanup policy, alternate eligibility evaluator or Barge journal parser.
--self-check uses bounded private local files; neither it nor --plan has
child/engine/seed/image/staging effects.
"""
from __future__ import annotations
import fcntl
import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import plistlib
import re
from pathlib import Path
import secrets
import shutil
import sys
import time

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
REPO = HERE.parent
SUPERVISOR = 'fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b'
BARGE = '8efd3452fcced9b5b322eb3f7ba0ac794244fc82'
BARGE_REVIEWED = 'b09ce942cb9bfc4936034d91190a16be71f17e1a'
BARGE_TREE = '07147ae958bae5744a0fabbdbcd9db8bc2599152'
STASH = '8A660440-C919-45C6-A3F1-3365B9BD2006'
SECONDS, MIB, BUDGET = 600, 1024 * 1024, 512
PREPARE_MIB, RESIDENT_MIB, GROWTH_MIB = 448, 288, 32
FEATURES = 'outputs-operation-proof,barge/proof-synthetic-account'
# Support is established from the published source export, lock and actual
# cached bytes below. No task-completion notice or boolean forces readiness.
SOURCE_BINDING = HERE / 'outputs-delegation-source.json'
# Every name has an executable body below. No native credit from self-checks.
CASES = (
    'positive-group', 'generic-release', 'selected',
    'marker-nonce', 'marker-digest', 'marker-owner', 'marker-volume',
    'grant-withdrawn', 'grant-generation', 'volume-kept', 'volume-unregistered',
    'unavailable', 'grant-invalid', 'adoption-race', 'fresh-busy',
    'internal-held-writer', 'removable-held-writer',
    'internal-unknown-custody', 'removable-unknown-custody',
    'intent-replay', 'rename-replay', 'removed-replay', 'capture-gap', 'unlink-gap',
    'deleted-before-hook', 'mark-before-removal', 'settled-before-ack', 'lost-ack',
    'root-replaced', 'checkout-ancestor-replaced', 'journal-missing', 'journal-gap',
    'partial-group', 'extra-group', 'absent-without-settlement', 'receipt-changed',
    'upgrade-internal-fence', 'upgrade-paired-fence', 'upgrade-registry',
    'upgrade-internal-ready', 'upgrade-paired-ready',
)


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


wrapper = load('delegation_wrapper', 'test-oxide-wrapper-bypass.py')
prior = load('delegation_image', 'test-outputs-prune.py')
fresh = load('delegation_custody', 'test-prune-fresh-custody.py')
permanent = load('delegation_children', 'test-permanent-outputs.py')
source = load('delegation_source', 'test-private-build-validation.py')


class OracleFailure(AssertionError):
    """Classified, recoverable assertion after every case child is closed."""


def require(value, message):
    if not value:
        raise OracleFailure(str(message)[:1200])


def write_json(path, value):
    fresh.atomic_write(str(path), json.dumps(value, sort_keys=True).encode(), mode=0o600)


def last_json(text):
    for line in reversed(text.splitlines()):
        if line.startswith('{'):
            return json.loads(line)
    raise OracleFailure('child returned no JSON object')


def pending(doc):
    return [record for record in doc['barge'] if record.get('phase') not in ('complete','cancelled')]


def fixture_groups(doc, root, successor):
    """Capture both actual member groups before any prune effect."""
    groups={}
    for key,path,selected in (('old',root,False),('successor',successor,True)):
        rows=sorted((r for r in doc['oxide']['rows'] if r['root']==str(path)),key=lambda r:r['member'])
        require(len(rows)==2 and {r['member'] for r in rows}=={'debug','release'},'fixture lacks a complete two-member '+key+' group')
        require(all(r.get('selected') is selected and not r.get('pending') and all(r.get(field) is None for field in ('pruning','pruned_ms','reservation','cancelling')) for r in rows),'fixture '+key+' group is not settled')
        groups[key]=json.loads(json.dumps(rows))
    return groups


def settled_group(doc, root, successor, before):
    fixture_groups({'oxide':{'rows':before['old']+before['successor']}},root,successor)
    require(not [r for r in doc['oxide']['rows'] if r['root']==str(root)],'completed old members remain')
    rows=sorted((r for r in doc['oxide']['rows'] if r['root']==str(successor)),key=lambda r:r['member'])
    require(rows==before['successor'],'selected successor group changed')
    require(not doc['oxide'].get('intents',[]) and 'deletion_routes' not in doc['oxide'],'covered path used a second journal/route')
    record=native_phase(doc,'complete')
    require(record.get('finalize_status')=='acknowledged','domain settlement was not acknowledged')
    payload=json.loads(record['finalize']['payload'])
    entries=[entry for entry in doc['oxide'].get('settled',[]) if entry['caller']=='oxide.outputs' and entry['key']==record['key']]
    require(len(entries)==1,'exact atomic settlement fact missing or repeated')
    entry=entries[0]
    require(entry['root']==str(root) and entry['nonce']==before['old'][0]['nonce']
            and entry['checkout_id']==before['old'][0]['checkout_id']
            and entry['request_sha256']==record['request_digest']
            and entry['terminal_ms']==payload['terminal_ms'] and entry['outcome']=='deleted','settlement fact not original-bound')
    require(hashlib.sha256(record['finalize']['payload'].encode()).hexdigest()==record['request_digest'],'actual payload digest differs')


def native_phase(doc, phase):
    records=doc['barge']
    require(len(records)==1,'expected exactly one checked operation record')
    record=records[0]
    require(record.get('schema')=='barge.operation/1' and isinstance(record.get('sequence'),int) and record['sequence']>0,'actual operation identity missing')
    require(record.get('phase')==phase,f'actual operation phase differs: {record.get("phase")} vs {phase}')
    require(record.get('caller')=='oxide.outputs' and re.fullmatch('[0-9a-f]{32}',record.get('key','')),'actual operation caller/key differs')
    return record


def fixture_env(proof, root, tools, stage_env):
    # Preserve the genuine inherited Nucleus hook; scrub product/tool overrides.
    device,_=owned_directory(proof.root)
    require(root.parent==proof.root,'fixture home escapes owned scratch')
    owned_directory(root,device)
    require(all(key in tools and Path(tools[key]).is_absolute() for key in ('cargo','rustc')),
            'fixture installed tools missing')
    rustup=Path(stage_env['RUSTUP_HOME'])
    owned_directory(rustup)
    env = proof.env()
    env = {k:v for k,v in env.items() if not k.startswith(('BARGE_', 'CARGO_', 'RUSTUP_', 'GIT_', 'XDG_'))}
    home, cargo, state, scratch = [root / name for name in ('home','cargo-home','barge-state','scratch-pools')]
    for path in (home, cargo, state, scratch):
        path.mkdir(mode=0o700)
    env.update(HOME=str(home), BARGE_ACCOUNT_HOME=str(home), BARGE_STATE_DIR=str(state),
               OXIDE_TEST_HOME_FROM_ENV='1',
               OXIDE_POOL=str(scratch/'pool'), OXIDE_SCRATCH_ROOT=str(scratch),
               OXIDE_POOL_CAP='64MiB', OXIDE_POOL_CREATE='1', CARGO_HOME=str(cargo),
               CARGO_NET_OFFLINE='true', CARGO_BUILD_JOBS='2', RUSTUP_AUTO_INSTALL='0',
               RUSTUP_HOME=str(rustup), RUSTUP_TOOLCHAIN=wrapper.BUILDER,
               RUSTC=str(tools['rustc']), CARGO=str(tools['cargo']),
               RUSTFLAGS='', RUSTC_WRAPPER='', CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0')
    return env


def host_cache():
    # Same source normalization as the existing fresh-custody preparation.
    # No-follow anchoring begins at the positively resolved common root;
    # links INSIDE that namespace remain refused by the seeder.
    given=Path(os.environ.get('CARGO_HOME',Path.home()/'.cargo'))
    registry=(given/'registry').resolve(strict=True)
    git=(given/'git').resolve(strict=True)
    if registry.name!='registry' or git.name!='git' or registry.parent!=git.parent:
        raise RuntimeError('registry and Git caches do not share one canonical source')
    return registry.parent


def binding_document():
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result:raise OracleFailure('duplicate source-binding key')
            result[key]=value
        return result
    data=SOURCE_BINDING.read_bytes()
    require(len(data)<=32*1024,'source-binding document exceeds bound')
    doc=json.loads(data,object_pairs_hook=unique)
    require(set(doc)=={'version','published','reviewed','tree','files','links'},'source-binding fields differ')
    require(doc['version']=='oxide.outputs.delegation.source/1' and doc['published']==BARGE and doc['reviewed']==BARGE_REVIEWED and doc['tree']==BARGE_TREE,'unreviewed adapter source binding')
    return doc


def verify_source_files(files,reader):
    required={'Cargo.toml','crates/barge/Cargo.toml',*('crates/barge/src/'+name for name in ('operation.rs','oxide_outputs.rs','delete.rs','evidence.rs','reference.rs','state.rs'))}
    require(isinstance(files,dict) and required<=set(files) and len(files)<=128,'source binding omits deciding/replay source')
    for name,digest in sorted(files.items()):
        path=Path(name)
        require(not path.is_absolute() and all(p not in ('','.','..') for p in path.parts) and (name in ('Cargo.toml','crates/barge/Cargo.toml') or name.startswith('crates/barge/src/')),'unsafe bound source path')
        require(isinstance(digest,str) and re.fullmatch('[0-9a-f]{64}',digest),'invalid bound source digest')
        require(reader(path.parts)==digest,'cached adapter source differs: '+name)
    return {'files':len(files),'manifest_sha256':hashlib.sha256(json.dumps(files,sort_keys=True,separators=(',',':')).encode()).hexdigest()}


def verify_source_links(files,links,reader):
    require(isinstance(links,dict) and len(links)<=8,'invalid source link table')
    for name,target in links.items():
        path=Path(name);relative=Path(target)
        require(name.startswith('crates/barge/src/') and not path.is_absolute() and all(p not in ('.','..') for p in path.parts),'unsafe source link path')
        require(not relative.is_absolute() and bool(relative.parts) and all(p not in ('.','..') for p in relative.parts),'source link escapes bound files')
        require(str(path.parent/relative) in files,'source link target is not independently bound')
        require(reader(path.parts)==target,'published source link changed')
    return len(links)


def adapter_binding(cache_root=None):
    doc=binding_document()
    seed=wrapper.offline_seed
    budget=seed.Budget(seed.Limits(work_seconds=10))
    _,packages=seed.parse_lock_packages((REPO/'Cargo.lock').read_text(),budget)
    barge=[p for p in packages if p['name']=='barge']
    require(len(barge)==1 and barge[0]['rev']==BARGE,'Cargo.lock does not bind the reviewed adapter')
    require(re.search(r'^barge = .*rev = "'+BARGE+r'"',(REPO/'Cargo.toml').read_text(),re.M),'manifest pin differs')
    cache=cache_root or host_cache()
    matches=[]
    with seed.Anchor(cache,'adapter source export') as anchor:
        for namespace in seed._list_names(anchor,('git','checkouts'),budget,'adapter checkout namespaces'):
            if not re.fullmatch(r'barge-[0-9a-f]{16}',namespace):continue
            base=('git','checkouts',namespace)
            for revision in seed._list_names(anchor,base,budget,'adapter revisions'):
                if not re.fullmatch(re.escape(BARGE[:7])+r'(?:-[0-9]+)?',revision):continue
                prefix=base+(revision,)
                used=0
                def digest(parts):
                    nonlocal used
                    fd,info=anchor.open_file(prefix+parts)
                    try:
                        require(info.st_size<=4*MIB and used+info.st_size<=16*MIB,'adapter export exceeds read bound')
                        used+=info.st_size;result=hashlib.sha256();remaining=info.st_size
                        while remaining:
                            budget.check('adapter source digest')
                            data=os.read(fd,min(65536,remaining));require(bool(data),'adapter source shortened')
                            result.update(data);remaining-=len(data)
                        require(not os.read(fd,1),'adapter source grew')
                        after=os.fstat(fd)
                        require((info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns),'adapter source changed during read')
                        return result.hexdigest()
                    finally:os.close(fd)
                def link_target(parts):
                    dfd=anchor.open_dir(prefix+parts[:-1])
                    try:
                        before=os.stat(parts[-1],dir_fd=dfd,follow_symlinks=False)
                        require(stat.S_ISLNK(before.st_mode) and before.st_size<=1024,'published source link has wrong type/bound')
                        target=os.readlink(parts[-1],dir_fd=dfd)
                        after=os.stat(parts[-1],dir_fd=dfd,follow_symlinks=False)
                        require((before.st_dev,before.st_ino,before.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_mtime_ns),'source link changed during observation')
                        return target
                    finally:os.close(dfd)
                verified=verify_source_files(doc['files'],digest)
                verified['links']=verify_source_links(doc['files'],doc['links'],link_target)
                matches.append(dict(verified,path=str(cache.joinpath(*prefix)),bytes=used))
    require(len(matches)==1,'expected one exact reviewed adapter source export')
    return dict(matches[0],published=BARGE,reviewed=BARGE_REVIEWED,tree=BARGE_TREE)


def stash_mount(scratch):
    mount=Path('/Volumes/Stash').resolve(strict=True)
    owned=mount/'scratch/oxide'
    require(scratch.is_relative_to(owned) and scratch!=owned and scratch.stat().st_dev==mount.stat().st_dev,'scratch is not on the owned Stash subtree')
    return mount


def source_readiness():
    result={'adapter':adapter_binding(),'supervisor_sha256':wrapper.sha256_file(HERE/'bounded-test.py')}
    require(result['supervisor_sha256']==SUPERVISOR,'ordinary supervisor differs')
    result['head']=source.git_output(REPO,'rev-parse','HEAD').decode().strip()
    result['tree']=source.git_output(REPO,'rev-parse','HEAD^{tree}').decode().strip()
    result['clean']=not source.git_output(REPO,'status','--porcelain').strip()
    require(result['clean'],'source plan requires a clean published tree')
    require(sys.platform=='darwin','native apparatus requires macOS')
    scratch=Path(os.environ['TMPDIR']).resolve(strict=True)
    info=scratch.stat()
    require(str(scratch)==os.environ['TMPDIR'] and scratch.is_dir() and info.st_uid==os.getuid(),'selected scratch not canonical/owned')
    probe=subprocess.run(['/usr/sbin/diskutil','info','-plist',str(stash_mount(scratch))],capture_output=True,timeout=10)
    require(probe.returncode==0,'Stash diskutil info refused: '+probe.stderr.decode(errors='replace')[-600:])
    volume=plistlib.loads(probe.stdout)
    require(volume.get('VolumeUUID','').upper()==STASH,'selected scratch is not the approved Stash UUID')
    result['scratch']={'path':str(scratch),'volume_uuid':STASH,'point_in_time_only':True}
    env=dict(os.environ,RUSTUP_AUTO_INSTALL='0')
    result['toolchain']={}
    for tool in ('cargo','rustc'):
        probe=subprocess.run(['rustup','which','--toolchain',wrapper.BUILDER,tool],env=env,capture_output=True,timeout=10,check=True)
        path=Path(probe.stdout.decode().strip()).resolve(strict=True)
        require(path.is_file(),'required installed tool absent')
        result['toolchain'][tool]=str(path)
    result['limits']='source and installed-input observations only; no seed, staging, image or native outcome; no activation or reservation'
    return result


def secondary_error(proof, phase, exc):
    message=f'{phase}: {type(exc).__name__}: {str(exc)[:600]}'
    if len(proof.secondary_errors)<8:proof.secondary_errors.append(message)
    try:print(message,file=sys.stderr,flush=True)
    except BaseException as error:
        if len(proof.secondary_errors)<8:proof.secondary_errors.append(f'secondary stderr publication: {type(error).__name__}: {str(error)[:600]}')


def seed_inputs(proof, host, cargo, lock):
    # Preserve the seed exception if its provenance publication also fails.
    provenance={}
    try:
        seed=wrapper.offline_seed.seed_cargo_home(host,cargo,lock,limits=wrapper.offline_seed.Limits(aggregate_bytes=128*MIB),deadline_ns=proof.deadline_ns,provenance=provenance)
    except BaseException:
        try:write_json(proof.evidence/'delegation-seed.json',provenance)
        except BaseException as exc:secondary_error(proof,'seed provenance publication',exc)
        raise
    write_json(proof.evidence/'delegation-seed.json',provenance)
    return seed


def owned_directory(path, device=None):
    facts=path.lstat()
    require(path.resolve(strict=True)==path and stat.S_ISDIR(facts.st_mode),'staging directory is linked/noncanonical')
    require(facts.st_uid==os.getuid() and not facts.st_mode&0o022,'staging directory ownership is unsafe')
    require(device is None or facts.st_dev==device,'staging directory crossed devices')
    return (facts.st_dev,facts.st_ino)


def phase_snapshot(proof,phase,limit):
    proof.remaining()
    root=Path(os.path.commonpath((proof.root,proof.evidence)))
    require(root in (proof.root,proof.root.parent),'evidence/scratch do not share this owned run root')
    device=owned_directory(root)[0]
    owned_directory(proof.root,device);owned_directory(proof.evidence,device)
    components={}
    for name,path in [('source',proof.root/'source'),('target',proof.root/'target'),
        ('expanded_cargo_including_seed',proof.root/'stage-cargo'),('stage_home',proof.root/'stage-home'),
        ('evidence_including_sealed_artifact',proof.evidence)]:
        if path.exists():owned_directory(path,device)
        components[name]=wrapper.owned_bytes(path) if path.exists() else 0
    total=wrapper.owned_bytes(root)
    components['other_owned_bytes']=total-sum(components.values())
    require(components['other_owned_bytes']>=0,'phase components overlap')
    record={'phase':phase,'total_allocated_bytes':total,'components':components,'limit_bytes':limit,
        'scope':'all current run-owned data; sampled phase observation, not a continuous guarantee'}
    if not hasattr(proof,'stage_phases'):proof.stage_phases=[]
    proof.stage_phases.append(record)
    write_json(proof.evidence/'delegation-stage-phases.json',proof.stage_phases)
    # Include this publication too; no optimistic subtraction for expanded
    # cache, logs, copied artifacts, or arbitrary other run-owned files.
    admitted=wrapper.owned_bytes(root)
    record['total_after_publication_bytes']=admitted
    require(admitted<=limit,f'{phase}: all-owned phase allocation {admitted} exceeds {limit} bytes')
    proof.remaining()
    return record


def producer_closed(proof,code,stderr):
    permanent.require_normal_exit('stage producer',code)
    permanent.require_confirmed_closure('stage producer',stderr)
    require(code==0 and proof.closure_known and not proof.unclosed,'stage producer custody is not positively closed/reaped')
    return {'exit':code,'reaped':True,'direct_child_reaped_by':'existing Proof.run wait',
        'closure_known':proof.closure_known,'unclosed_handles':len(proof.unclosed),
        'limit':'cooperative producer custody; final qualified guard group census remains separate'}


def dispose_fresh_target(proof,target,identity):
    # Target-specific authorization around Python's established fd-safe
    # rmtree. Never a general cache/root deletion interface.
    require(target==proof.root/'target','only this fresh compilation target may be disposed')
    device=owned_directory(proof.root)[0]
    require(owned_directory(target,device)==identity,'fresh target identity changed')
    require(shutil.rmtree.avoids_symlink_attacks,'fd-safe target cleanup is unavailable')
    proof.remaining()
    for _,dirs,files,fd in os.fwalk(target,follow_symlinks=False):
        proof.remaining()
        for name in dirs+files:
            info=os.stat(name,dir_fd=fd,follow_symlinks=False)
            require(info.st_uid==os.getuid() and info.st_dev==device and
                (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)),
                'fresh target contains unknown ownership/device/type or a link')
    parent=os.open(proof.root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try:
        info=os.stat('target',dir_fd=parent,follow_symlinks=False)
        require((info.st_dev,info.st_ino)==identity,'fresh target replaced before cleanup')
        proof.remaining()
        fd=os.open('target',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=parent)
        try:
            require((os.fstat(fd).st_dev,os.fstat(fd).st_ino)==identity,'target changed while opened for cleanup')
            def fail_cleanup(function,path,exc_info):raise exc_info[1]
            # The fd-safe engine already used by this Python's rmtree; its
            # public wrapper lacks dir_fd here. Reuse it, never pathname-follow.
            shutil._rmtree_safe_fd(fd,str(target),fail_cleanup)
        finally:os.close(fd)
        info=os.stat('target',dir_fd=parent,follow_symlinks=False)
        require((info.st_dev,info.st_ino)==identity,'target changed before final rmdir')
        proof.remaining()
        os.rmdir('target',dir_fd=parent)
        try:os.stat('target',dir_fd=parent,follow_symlinks=False)
        except FileNotFoundError:pass
        else:raise RuntimeError('fresh target cleanup did not establish absence')
    finally:os.close(parent)
    proof.remaining()


def seal_stage(proof,binary,target_identity,provenance):
    require(proof.closure_known and not proof.unclosed,'unknown stage custody refuses sealing/cleanup')
    require(provenance.get('producer',{}).get('reaped') is True and provenance['producer']['exit']==0,'stage producer reap is not recorded')
    target=proof.root/'target'
    require(owned_directory(target,owned_directory(proof.root)[0])==target_identity,'target identity changed before sealing')
    require(binary==target/'debug/oxide','unexpected stage artifact path')
    source.canonical_file(binary)
    info=binary.lstat()
    require(info.st_uid==os.getuid() and info.st_dev==target_identity[0] and info.st_mode&0o100,'stage artifact ownership/device/mode is unsafe')
    digest=source.sha256_file(binary,cap=source.MAX_ADMISSION_ARTIFACT_BYTES,deadline_ns=proof.deadline_ns)
    artifact=source.copy_candidate_artifact(binary,proof.evidence,digest,info.st_size,deadline_ns=proof.deadline_ns)
    sealed=Path(artifact['path'])
    sealed_identity=sealed.lstat()
    artifact['identity']=[sealed_identity.st_dev,sealed_identity.st_ino]
    provenance.update(binary_sha256=digest,binary_bytes=info.st_size,sealed_artifact=artifact,
        target={'path':str(target),'identity':list(target_identity),'cleanup':'not_started'})
    write_json(proof.evidence/'delegation-stage.json',provenance)
    phase_snapshot(proof,'sealed-before-target-cleanup',PREPARE_MIB*MIB)
    # Verify the retained copy and durable provenance before authorizing any
    # deletion. A copy/hash/publication error leaves the target untouched.
    require(source.sha256_file(sealed,cap=source.MAX_ADMISSION_ARTIFACT_BYTES,deadline_ns=proof.deadline_ns)==digest,'sealed artifact hash changed')
    provenance['target']['cleanup']='started'
    write_json(proof.evidence/'delegation-stage.json',provenance)
    dispose_fresh_target(proof,target,target_identity)
    after=sealed.lstat()
    require(sealed.resolve(strict=True)==sealed and (after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns)==(sealed_identity.st_dev,sealed_identity.st_ino,sealed_identity.st_size,sealed_identity.st_mtime_ns),'sealed artifact identity changed during cleanup')
    require(source.sha256_file(sealed,cap=source.MAX_ADMISSION_ARTIFACT_BYTES,deadline_ns=proof.deadline_ns)==digest,'sealed artifact changed after cleanup')
    provenance['target']['cleanup']='absent_verified'
    write_json(proof.evidence/'delegation-stage.json',provenance)
    phase_snapshot(proof,'resident-after-target-cleanup',RESIDENT_MIB*MIB)
    return sealed


def run_phase_snapshot(proof,image,phase):
    record=phase_snapshot(proof,phase,(RESIDENT_MIB+128+GROWTH_MIB)*MIB)
    backing=Path(image.backing)
    info=backing.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid==os.getuid(),'image backing accounting is unknown')
    image_bytes=info.st_blocks*512
    require(image_bytes<=128*MIB,'image backing exceeds its128MiB allocation')
    require(record['total_after_publication_bytes']-image_bytes-proof.resident_baseline<=GROWTH_MIB*MIB,
        'all fixture/evidence growth exceeds32MiB')


def stage(proof):
    require(wrapper.sha256_file(HERE/'bounded-test.py') == SUPERVISOR, 'ordinary supervisor bytes changed')
    head = wrapper.git(proof, 'rev-parse', 'HEAD')
    require(wrapper.git(proof, 'status', '--porcelain') == '', 'source must be clean')
    tree = wrapper.git(proof, 'rev-parse', 'HEAD^{tree}')
    root = proof.root/'source'
    root.mkdir(mode=0o700)
    binding = source.stage_git_tree(REPO, root, head, tree, deadline_ns=proof.deadline_ns)
    lock = (root/'Cargo.lock').read_bytes()
    require(BARGE.encode() in lock, 'reviewed Barge pin missing')
    tools = {}
    for tool in ('cargo','rustc'):
        code,out,err = proof.run(['rustup','which','--toolchain',wrapper.BUILDER,tool], label='resolve-'+tool,limit=10)
        require(code == 0, err)
        tools[tool] = Path(out.strip()).resolve(strict=True)
    rustup_home = Path(os.environ.get('RUSTUP_HOME',Path.home()/'.rustup')).resolve(strict=True)
    home, cargo = proof.root/'stage-home', proof.root/'stage-cargo'
    home.mkdir(mode=0o700); cargo.mkdir(mode=0o700)
    host = host_cache()
    # This bounded seeder copies only locked inputs; never changes host caches.
    seed = seed_inputs(proof,host,cargo,lock)
    env = proof.env({'HOME':str(home),'CARGO_HOME':str(cargo),'RUSTUP_HOME':str(rustup_home),
                     'RUSTUP_TOOLCHAIN':wrapper.BUILDER,'RUSTC':str(tools['rustc']),
                     'CARGO_NET_OFFLINE':'true','CARGO_BUILD_JOBS':'6','CARGO_INCREMENTAL':'0',
                     'CARGO_PROFILE_DEV_DEBUG':'0','RUSTFLAGS':'','RUSTC_WRAPPER':'',
                     'CARGO_TARGET_DIR':str(proof.root/'target')})
    env = {k:v for k,v in env.items() if not k.startswith(('BARGE_','GIT_'))}
    versions=[]
    for tool in ('cargo','rustc'):
        code,out,err = proof.run([str(tools[tool]),'--version'],env=env,label='version-'+tool,limit=10)
        require(code == 0,err); versions.append(out.strip())
    require(tuple(versions) == wrapper.BUILDER_VERSIONS, f'toolchain mismatch: {versions}')
    target=proof.root/'target';target.mkdir(mode=0o700)
    target_identity=owned_directory(target,owned_directory(proof.root)[0])
    phase_snapshot(proof,'prepared-before-build',PREPARE_MIB*MIB)
    argv=[str(tools['cargo']),'build','--locked','--offline','--bin','oxide','--features',FEATURES]
    write_json(proof.evidence/'delegation-stage-inputs.json',{'head':head,'tree':tree,'source':binding,'lock_sha256':hashlib.sha256(lock).hexdigest(),'argv':argv,'features':FEATURES,'versions':versions,'flags':'','tools':{k:str(v) for k,v in tools.items()}})
    code,_,err = proof.run(argv,cwd=root,env=env,label='delegation-stage',limit=170)
    producer=producer_closed(proof,code,err)
    phase_snapshot(proof,'build-finished-before-sealing',PREPARE_MIB*MIB)
    for item in seed['git_sources']:
        wrapper.offline_seed.verify_private_checkout(cargo,item['rev'],[c['database'] for c in item['candidates']],deadline_ns=proof.deadline_ns)
    binary = proof.root/'target/debug/oxide'
    provenance={'head':head,'tree':tree,'source':binding,'lock_sha256':hashlib.sha256(lock).hexdigest(),
                'barge':BARGE,'producer':producer,
                'tools':{k:{'path':str(v),'sha256':wrapper.sha256_file(v)} for k,v in tools.items()},
                'versions':versions,'features':FEATURES,'argv':argv,'flags':'','seed':seed,
                'supervisor':SUPERVISOR,'proof_sha256':wrapper.sha256_file(Path(__file__)),
                'manifest_sha256':wrapper.sha256_file(REPO/'effigy.toml'),'adapter_source':adapter_binding(cargo)}
    sealed=seal_stage(proof,binary,target_identity,provenance)
    return sealed, tools, env


def require_custody_observation(code,err):
    # Pinned Barge059 check_captured_writers: process_start -> bsd_info (or
    # missing self); scan_open_descriptors -> process_table (including its
    # parser/own-row check) -> scan_processes. effective_uid cannot fail on
    # macOS; later scan faults are Bounded/Unreadable, not propagated Err.
    # Match whole producer families, including dynamic sizes, offsets and PIDs,
    # rather than one spelling of a bounded scan. No kernel query occurs here.
    if code in (0,-9):return
    text=err.lower()
    if 'a live writer holds the captured unit' in text:
        raise RuntimeError('unexpected live writer; abort and retain: '+err[-1600:])
    scan=any(word in text for word in ('custody is not proven','descriptor scan','process observation',
                                      'captured-unit writer custody is unknown','the process table'))
    identity=('the calling process identity is unavailable' in text
              or 'pid 0 has no process start identity' in text
              or re.search(r'\bthe facts of pid [0-9]+ are unreadable:',text) is not None)
    if scan or identity:raise RuntimeError('actual custody uncertainty; abort and retain: '+err[-1600:])


class World:
    def __init__(self, proof, binary, tools, stage_env, image, name):
        self.proof, self.binary, self.name = proof, binary, name
        # macOS mktemp requires an explicit template ending in Xs.
        code,out,err = proof.run(['/usr/bin/mktemp','-d',str(proof.root/('delegation-'+name+'.XXXXXXXX'))],label=name+'-mktemp',limit=5)
        require(code == 0,err)
        self.base=Path(out.strip()).resolve(strict=True)
        require(self.base.parent == proof.root,'mktemp escaped owned scratch')
        self.env=fixture_env(proof,self.base,tools,stage_env)
        self.data=self.helper('setup',hashlib.sha256(name.encode()).hexdigest()[:12],image.mount,image.volume_uuid)
        self.check_setup(image)
        self.root=Path(self.data['root']); self.successor=Path(self.data['successor'])
        self.control=Path(self.data['control']); self.peer=Path(self.data['peer_control']); self.checkout=Path(self.data['checkout'])
        if not name.startswith('upgrade-'):
            self.invoke(['outputs','upgrade-recovery'],expect=0,label='upgrade')
        self.groups_before=fixture_groups(self.inspect(),self.root,self.successor)
        self.receipt_sha=hashlib.sha256(Path(self.data['receipt']).read_bytes()).hexdigest()
        write_json(self.proof.evidence/(self.name+'-groups-before.json'),self.groups_before)
        self.plan_path=self.base/'presented.json'
        self.plan=None

    def check_setup(self,image):
        require(all(isinstance(self.data.get(key),str) and self.data[key] for key in
                    ('root','successor','control','peer_control','checkout','uuid','mount','checkout_volume_uuid')),
                'fixture setup paths/identity missing')
        require(self.data['mount']==image.mount and self.data['uuid']==image.volume_uuid,
                'fixture setup image binding differs')
        for key in ('root','successor'):
            path=Path(self.data[key])
            require(path.parent==Path(image.mount)/'oxide-outputs' and path.resolve(strict=True)==path,
                    'fixture setup root escaped image')
        for key in ('control','peer_control','checkout'):
            path=Path(self.data[key])
            require(path.is_relative_to(self.base) and path!=self.base and path.resolve()==path,
                    'fixture setup private path escaped')

    def command_env(self,extra=None):
        env=dict(self.env,**(extra or {}))
        require(env.get('OXIDE_TEST_HOME_FROM_ENV')=='1' and 'OXIDE_TEST_ACCOUNT_HOME' not in env,
                'fixture account mode differs or conflicts')
        for key,relative in (('HOME','home'),('BARGE_ACCOUNT_HOME','home'),('BARGE_STATE_DIR','barge-state'),
                             ('CARGO_HOME','cargo-home'),('OXIDE_SCRATCH_ROOT','scratch-pools')):
            require(env.get(key)==str(self.base/relative),'fixture private environment escaped: '+key)
        require(env.get('OXIDE_POOL')==str(self.base/'scratch-pools/pool'),'fixture pool escaped')
        require(all(env.get(key)==str(self.proof.root) for key in ('OXIDE_TASK_SCRATCH_ROOT','TMPDIR','TMP','TEMP')),
                'fixture temporary root differs')
        require(env.get('OXIDE_PROOF_EVIDENCE')==str(self.proof.evidence)
                and env.get('OXIDE_PROOF_DEADLINE_NS')==str(self.proof.deadline_ns+wrapper.SHUTDOWN_MARGIN_NS),
                'fixture guard context differs')
        return env

    def invoke(self,args,*,expect=None,label='command',extra=None):
        env=self.command_env(extra)
        result=self.proof.run([str(self.binary),*map(str,args)],cwd=self.base,env=env,label=self.name+'-'+label,limit=15)
        code,out,err=result
        permanent.require_confirmed_closure(label,err)
        permanent.require_normal_exit(label,code)
        require_custody_observation(code,err)
        if expect is not None and code!=expect:raise RuntimeError(f'{label}: unexpected exit {code}: {err[-1200:]}')
        return result

    def helper(self,*args):
        _,out,_=self.invoke(['outputs','__delegation-fixture',*args],expect=0,label='fixture-'+str(args[0]))
        if args[0] not in ('setup','inspect','replay','generic-release'):return None
        value=last_json(out)
        require(isinstance(value,dict),'fixture helper response is not an object')
        if args[0]=='inspect':
            require(isinstance(value.get('oxide'),dict) and isinstance(value.get('barge'),list)
                    and isinstance(value['oxide'].get('rows'),list),'fixture inspect shape differs')
        return value

    def preview(self, candidate=True):
        before=fresh.tree_digest(str(self.control))
        _,out,err=self.invoke(['outputs','prune','--dry-run'],expect=0,label='preview')
        require(fresh.tree_digest(str(self.control)) == before,'dry-run mutated control')
        doc=json.loads(out)
        if candidate:
            offered=[c for c in doc['candidates'] if c['root']==str(self.root)]
            require(len(offered)==1 and offered[0].get('deleter')=='barge',f'covered candidate absent: {doc}')
        self.plan_path.write_bytes(out.encode())
        self.plan=hashlib.sha256(out.encode()).hexdigest()
        require(self.plan in err,'preview digest does not bind stdout bytes')
        return doc

    def argv(self):
        require(self.plan is not None,'preview missing')
        return ['outputs','prune','--plan',str(self.plan_path),'--plan-sha256',self.plan]

    def execute(self,expect=None,extra=None,label='prune'):
        return self.invoke(self.argv(),expect=expect,extra=extra,label=label)

    def inspect(self): return self.helper('inspect')

    def domain_digest(self):
        return {str(path):hashlib.sha256(path.read_bytes()).hexdigest() for path in
                (self.control/'outputs/registry.json',Path(self.data['receipt']))}

    def kept(self):
        require(self.root.is_dir() and self.successor.is_dir(),'a kept root or successor changed')
        require(not list(self.root.parent.glob('.prune-*')),'local quarantine used for Barge route')

    def settled(self):
        doc=self.inspect()
        require(not self.root.exists() and self.successor.is_dir(),'root/successor deletion oracle failed')
        settled_group(doc,self.root,self.successor,self.groups_before)
        require(not list(self.root.parent.glob('.prune-*')),'local quarantine used')
        self.absence(native_phase(doc,'complete'))
        require(hashlib.sha256(Path(self.data['receipt']).read_bytes()).hexdigest()==self.receipt_sha,'released-root receipt was rewritten')
        require(not (self.control/'outputs/prune-delegations').exists(),'old covered request sidecars were created')
        return doc

    def absence(self,record):
        value=record.get('absence')
        require(isinstance(value,dict) and value.get('verdict')=='absent','actual Barge absence evidence missing')
        ancestor=value.get('ancestor',{});volume=ancestor.get('volume',{})
        require(value.get('path')==str(self.checkout) and value.get('anchor',{}).get('kind')=='registered','absence path/route not the real Stash checkout')
        require(volume.get('kind')=='registered' and str(volume.get('volume_uuid','')).upper()==self.data['checkout_volume_uuid'].upper(),'absence checkout volume differs')
        require(ancestor.get('path')==str(self.checkout.parent) and ancestor.get('inode')==self.data['checkout_ancestor_inode'],'absence ancestor differs')

    def start(self,args,extra=None):
        old_markers=set(self.proof.evidence.iterdir())
        child=permanent.start_child(self.proof,[str(self.binary),*map(str,args)],str(self.base),self.command_env(extra))
        child.delegation_old_markers=old_markers
        return child

    def finish(self,child,label,interrupted=False):
        code,out,err=permanent.finish_child(self.proof,child,self.name+'-'+label)
        (self.proof.evidence/(self.name+'-'+label+'.child.json')).write_text(json.dumps({'pid':child.pid,'exit':code,'interrupted':interrupted,'stdout':out[-8192:],'stderr':err[-8192:]}))
        if interrupted:
            # This direct child executes no Cargo/build/writer descendants; its
            # only work is the real synchronous adapter. Reap is mandatory.
            require(code == -9,'durable-point child did not terminate by our owned-child KILL')
        else:
            permanent.require_normal_exit(label,code)
            permanent.require_confirmed_closure(label,err)
        require_custody_observation(code,err)
        return code,out,err

    def wait(self,pattern,child):
        end=time.monotonic()+min(4,self.proof.remaining())
        while time.monotonic()<end:
            matches=[p for p in self.proof.evidence.glob(pattern) if p not in child.delegation_old_markers]
            if len(matches)==1:return matches[0]
            if child.poll() is not None: raise RuntimeError('child ended before observed durable point')
            time.sleep(.005)
        raise RuntimeError('bounded durable-point observation expired')

    def interrupt(self,phase):
        if self.plan is None:self.preview()
        child=self.start(self.argv(),{'OXIDE_TEST_OPERATION_ROOT':str(self.root),'OXIDE_TEST_OPERATION_PAUSE':phase})
        try:
            marker=self.wait('operation-'+phase+'-*',child)
            doc=self.inspect()
            record=native_phase(doc,'intent' if phase=='intent' else 'deleted')
            self.absence(record)
            require(not doc['oxide'].get('intents',[]),'second Oxide progress journal observed')
            write_json(self.proof.evidence/(self.name+'-interruption.json'),{'pid':child.pid,'point':phase,'observed':doc,'marker':str(marker)})
            child.kill();self.finish(child,'interrupted',interrupted=True)
            return doc
        except BaseException:
            if child.poll() is None:child.kill();self.finish(child,'failed-interruption',interrupted=True)
            raise


def replacement_refusal(w,record):
    # Replace the operation's ACTUAL captured root, not its former visible name.
    # Pending("the captured unit identity changed") is the pinned checked-replay
    # refusal before its writer scan. A different refusal proves nothing here.
    name=record['unit']['capture_name']
    require(isinstance(name,str) and bool(name) and name not in ('.','..') and Path(name).name==name,
            'captured root name is not one component')
    require(record['unit']['name']==w.root.name,'operation does not bind this root name')
    capture=w.root.parent/name
    require(capture.resolve(strict=True)==capture,'captured root is linked')
    facts=capture.lstat()
    require(stat.S_ISDIR(facts.st_mode) and facts.st_uid==os.getuid()
            and facts.st_ino==record['unit']['inode'],'capture does not bind the original root')
    retained=capture.with_name(capture.name+'.fixture-original')
    require(not retained.exists() and not retained.is_symlink(),'fresh retained fixture path is taken')
    capture.rename(retained)
    capture.mkdir(mode=0o700)
    with os.fdopen(os.open(capture/'replacement',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600),'w') as file:
        file.write('different object')
    require(capture.stat().st_ino!=facts.st_ino,'replacement did not change root identity')
    before=w.domain_digest()
    trees={path:fresh.tree_digest(str(path)) for path in (capture,retained,w.successor)}
    _,_,err=w.execute(expect=3,label='replacement-refusal')
    expected=f'operation {record["key"]} for {w.root} remains pending: Pending("the captured unit identity changed")'
    require(expected in err,'replacement did not produce its exact correlated identity refusal: '+err[-1200:])
    after=w.inspect()
    require(native_phase(after,'renamed')==record,'replacement refusal changed the original operation')
    require(not after['oxide']['settled'] and w.domain_digest()==before,
            'replacement refusal changed or settled the domain')
    require(not w.root.exists(),'replacement case recreated the old visible name')
    require(all(fresh.tree_digest(str(path))==tree for path,tree in trees.items()),
            'replacement refusal changed the replacement, retained original or successor')


def held_writer_refusal(w):
    # These intentional writer cases prove Oxide POOL custody in an effect-free
    # preview, not a captured-unit replay refusal. No broad writer exemption.
    doc=w.preview(candidate=False)
    require(not any(c['root']==str(w.root) for c in doc['candidates']),'actual held writer root offered')
    require('active writer' in json.dumps(doc),'held writer refusal not attributable')
    w.kept()


def run_case(w, name):
    if name.startswith('upgrade-'):upgrade_case(w,name.removeprefix('upgrade-'));return
    if name in ('positive-group','selected'):
        doc=w.preview()
        require(not any(c['root']==str(w.successor) for c in doc['candidates']),'selected successor offered')
        w.execute(expect=0);w.settled();return
    if name=='generic-release':
        report=w.helper('generic-release',w.data['uuid']);w.kept()
        require(not report['run'].get('rows') or all(row['outcome']!='released' for row in report['run']['rows']),report)
        require(not w.inspect()['barge'],'generic release created a delegated operation');return
    if name in ('volume-kept','volume-unregistered','unavailable','grant-invalid'):
        if name=='volume-kept':w.helper('volume',w.data['uuid'],w.data['mount'],'kept')
        elif name=='volume-unregistered':
            path=Path(w.env['BARGE_STATE_DIR'])/'registry.json';path.rename(path.with_suffix('.retained'))
        else:w.helper('grant','invalid' if name=='grant-invalid' else 'withdraw')
        doc=w.preview();require(any(c.get('barge_unavailable') for c in doc['candidates']),'unavailable reason absent')
        w.execute(expect=3);w.kept();require(not w.inspect()['barge'],'refused begin allocated an operation');return
    if name.startswith('marker-') or name in ('grant-withdrawn','grant-generation','adoption-race'):
        if name=='grant-generation':w.helper('grant','present')
        w.preview();child=w.start(w.argv(),{'OXIDE_TEST_OPERATION_ROOT':str(w.root),'OXIDE_TEST_OPERATION_PAUSE':'verdict'})
        try:
            marker=w.wait('operation-verdict-*',child);key=marker.name.removeprefix('operation-verdict-')
            require(not w.inspect()['barge'],'begin preceded the guarded verdict observation')
            if name.startswith('marker-'):
                path=w.root/'.oxide-output.json';value=json.loads(path.read_text())
                field={'marker-nonce':'nonce','marker-digest':'label','marker-owner':'checkout_id','marker-volume':'volume_uuid'}[name]
                value[field]={'nonce':'d'*32,'label':'changed bytes','checkout_id':'e'*64,'volume_uuid':'11111111-2222-3333-4444-555555555555'}[field]
                write_json(path,value)
            elif name.startswith('grant-'):w.helper('grant','older' if name=='grant-generation' else 'withdraw')
            else:
                w.checkout.mkdir();(w.checkout/'Cargo.toml').write_text('[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n')
                code,_,_=w.proof.run([str(w.binary),'outputs','adopt'],cwd=w.checkout,env=w.command_env(),label='competing-adopt',limit=4)
                require(code!=0,'competing adoption crossed the real barrier')
            (w.proof.evidence/('operation-release-'+key)).write_text('release')
            code,_,err=w.finish(child,'changed-authority');require(code in (2,3),err);w.kept()
            require(all(record['phase']=='cancelled' for record in w.inspect()['barge']),'changed authority caused an effect')
        finally:
            if child.poll() is None:child.kill();w.finish(child,'race-stop',interrupted=True)
        return
    if name=='fresh-busy':
        w.preview();token=secrets.token_hex(16);child=w.start(['outputs','__delegation-fixture','lock',w.data['uuid'],token])
        try:
            w.wait('lock-'+token,child);w.execute(expect=3);w.kept()
            require(not w.inspect()['barge'],'busy begin created a record')
        finally:
            (w.proof.evidence/('release-'+token)).write_text('release');code,_,err=w.finish(child,'busy-lock');require(code==0,err)
        return
    if name in ('internal-held-writer','removable-held-writer'):held_writer(w,name.startswith('removable'));return
    if name in ('internal-unknown-custody','removable-unknown-custody'):
        control=w.peer if name.startswith('removable') else w.control
        with (control/'admission.lock').open('rb') as handle:
            fcntl.flock(handle,fcntl.LOCK_EX|fcntl.LOCK_NB)
            before=fresh.tree_digest(str(w.control));doc=w.preview(candidate=False)
            require(not doc['coverage']['complete'] and not doc['candidates'],'unknown custody admitted root')
            require('admission lock is busy' in json.dumps(doc),'actual unknown observation absent')
            require(fresh.tree_digest(str(w.control))==before,'unknown observation mutated state')
        w.kept();return
    if name=='lost-ack':
        w.preview();w.execute(expect=3,extra={'OXIDE_TEST_OPERATION_ROOT':str(w.root),'OXIDE_TEST_OPERATION_FAULT':'lost-ack'})
        doc=w.inspect();native_phase(doc,'deleted')
        require(not [row for row in doc['oxide']['rows'] if row['root']==str(w.root)] and len(doc['oxide']['settled'])==1,'lost-ack lacked atomic removal fact')
        w.execute(expect=0,label='retry-ack');w.settled();return
    if name in ('intent-replay','deleted-before-hook','mark-before-removal','settled-before-ack','partial-group','extra-group','absent-without-settlement','receipt-changed','journal-missing','journal-gap'):
        phase={'intent-replay':'intent','deleted-before-hook':'deleted','mark-before-removal':'marked','settled-before-ack':'settled','journal-missing':'intent','journal-gap':'intent'}.get(name,'marked')
        w.interrupt(phase)
        if name.startswith('journal-'):
            w.helper('journal-fault',name.removeprefix('journal-'));w.execute(expect=3,label='unknown-journal');return
        if name in ('partial-group','extra-group','absent-without-settlement','receipt-changed'):
            kind={'partial-group':'partial','extra-group':'extra','absent-without-settlement':'absent','receipt-changed':'receipt'}[name]
            w.helper('domain-fault',w.root,kind)
            before=w.domain_digest();record=w.inspect()['barge'][0]
            w.execute(expect=3,label='changed-domain')
            require(w.domain_digest()==before,'changed domain was partially settled')
            require(w.inspect()['barge'][0]==record,'refused finalization rewrote the operation')
            native_phase(w.inspect(),'deleted');return
        if name=='deleted-before-hook':
            doc=w.helper('replay');require(str(w.root) in doc['completed_roots'],'explicit caller drain did not settle')
            w.settled();return
        w.execute(expect=0,label='resume');w.settled();return
    require(name in ('rename-replay','removed-replay','capture-gap','unlink-gap','root-replaced','checkout-ancestor-replaced'),'unimplemented fixed case '+name)
    w.preview();fault={'rename-replay':'renamed','removed-replay':'removed','root-replaced':'renamed','checkout-ancestor-replaced':'renamed','capture-gap':'capture-gap','unlink-gap':'unlink-gap'}[name]
    w.execute(expect=3,extra={'OXIDE_TEST_OPERATION_ROOT':str(w.root),'OXIDE_TEST_OPERATION_FAULT':fault})
    doc=w.inspect();record=native_phase(doc,{'capture-gap':'intent','unlink-gap':'marker_last','removed-replay':'removed'}.get(name,'renamed'))
    w.absence(record)
    if name=='root-replaced':replacement_refusal(w,record);return
    if name=='checkout-ancestor-replaced':
        anchor=w.checkout.parent;anchor.rename(anchor.with_name('retained-anchor'));anchor.mkdir(mode=0o700)
        require(anchor.stat().st_ino!=w.data['checkout_ancestor_inode'],'ancestor replacement was not discriminating')
    if name in ('capture-gap','unlink-gap','checkout-ancestor-replaced'):
        w.execute(expect=3,label='conservative-replay');require(len(pending(w.inspect()))==1,'unknown replay was acknowledged')
        require(not w.inspect()['oxide']['settled'],'unknown removal produced a settlement fact');return
    w.execute(expect=0,label='checked-replay');w.settled()


def upgrade_case(w,phase):
    child=w.start(['outputs','upgrade-recovery'],{'OXIDE_TEST_RECOVERY_UPGRADE_PAUSE':str(w.control),'OXIDE_TEST_RECOVERY_UPGRADE_PHASE':phase})
    try:
        w.wait('upgrade-'+phase+'-*',child)
        states=[json.loads((p/'state.json').read_text()) for p in (w.control,w.peer)]
        registry=json.loads((w.control/'outputs/registry.json').read_text())
        expected={
            'internal-fence':(['/5','/3'],['initializing','ready'],2),
            'paired-fence':(['/5','/5'],['initializing','initializing'],2),
            'registry':(['/5','/5'],['initializing','initializing'],4),
            'internal-ready':(['/5','/5'],['ready','initializing'],4),
            'paired-ready':(['/5','/5'],['ready','ready'],4),
        }[phase]
        require([s['owned_outputs'][-2:] for s in states]==expected[0],'upgrade fence ordering')
        require([next(iter(s['outputs_init'])) for s in states]==expected[1],'upgrade ready ordering')
        require(registry['schema']==expected[2],'upgrade registry publication ordering')
        write_json(w.proof.evidence/(w.name+'-observed.json'),{'states':states,'registry':registry,'pid':child.pid})
        child.kill();w.finish(child,'upgrade-interrupted',interrupted=True)
        if phase!='paired-ready':
            before=fresh.tree_digest(str(w.control));w.invoke(['outputs','prune','--dry-run'],expect=2,label='partial-refusal')
            require(fresh.tree_digest(str(w.control))==before,'ordinary mutation crossed partial upgrade')
        w.invoke(['outputs','upgrade-recovery'],expect=0,label='upgrade-resume')
        w.preview();w.execute(expect=0);w.settled()
    finally:
        if child.poll() is None:child.kill();w.finish(child,'upgrade-stop',interrupted=True)


def held_writer(w, removable):
    w.checkout.mkdir();(w.checkout/'src').mkdir()
    (w.checkout/'Cargo.toml').write_text('[package]\nname="f"\nversion="0.1.0"\nedition="2021"\n')
    (w.checkout/'src/main.rs').write_text('fn main() {}\n')
    gate=w.base/'writer-gate';gate.mkdir()
    # Cargo exec is genuine, tiny and dependency-free. Its child remains in
    # Oxide's ordinary lease/custody path, with real pool state and locks.
    script=w.base/'hold.py'
    script.write_text('import os,time,pathlib\np=pathlib.Path(os.environ["WRITER_GATE"])\n(p/"started").write_text(str(os.getpid()))\nwhile not (p/"release").exists(): time.sleep(.01)\n')
    env={'WRITER_GATE':str(gate)}
    registration=w.control/'removable.json'
    retained=registration.with_suffix('.writer-retained')
    if not removable:
        # Fixture-only absence during selection, restored before the custody
        # observation. The real command positively records its chosen pool.
        registration.rename(retained)
    child=permanent.start_child(w.proof,[str(w.binary),'exec','--','/usr/bin/python3',str(script)],str(w.checkout),w.command_env(env))
    away=w.checkout.with_name('retained-running-checkout')
    try:
        end=time.monotonic()+min(10,w.proof.remaining())
        while not (gate/'started').exists():
            if child.poll() is not None or time.monotonic()>end:raise RuntimeError('real writer never entered its gated child')
            time.sleep(.01)
        if not removable: retained.rename(registration)
        # Assert which pool the REAL command actually entered. An ignored
        # selection variable cannot earn evidence for the other pool.
        controls=[w.control,w.peer]
        states=[json.loads((c/'state.json').read_text()) for c in controls]
        active=[n for n,state in enumerate(states) if any(r.get('checkout')==str(w.checkout) and r.get('phase') in ('running','spawning') for r in state.get('records',[]))]
        require(active==[1 if removable else 0],f'writer entered wrong pool: {active}')
        w.checkout.rename(away)
        held_writer_refusal(w)
    finally:
        if retained.exists():retained.rename(registration)
        if away.exists():away.rename(w.checkout)
        (gate/'release').write_text('release')
        code,_,err=w.finish(child,'held-writer');require(code==0,err)


def input_fit():
    """Read-only bounded metadata using the existing seeder's anchored walker.
    This proves neither cached commit validity nor build/native runtime fit.
    """
    seed=wrapper.offline_seed
    budget=seed.Budget(seed.Limits(aggregate_bytes=128*MIB,work_seconds=10))
    registry,git=seed.parse_lock_packages((REPO/'Cargo.lock').read_text(),budget)
    host=host_cache()
    total=0;enumerated=0;databases=[];missing=[]
    with seed.Anchor(host,'host readiness only') as anchor:
        ns=seed.CRATES_NAMESPACE
        parts=[('registry','index',ns,'config.json')]
        for package in registry:
            parts.append(('registry','index',ns,'.cache',*seed.sparse_index_path(package['name'])))
            parts.append(('registry','cache',ns,package['name']+'-'+package['version']+'.crate'))
        for path in parts:
            fd,info=anchor.open_file(path)
            os.close(fd);total+=info.st_size
        names=seed._list_names(anchor,('git','db'),budget,'readiness candidate list')
        for repository in dict.fromkeys(p['repository'] for p in git):
            matches=[n for n in names if re.fullmatch(re.escape(repository)+r'-[0-9a-f]{16}',n)]
            if not 1<=len(matches)<=budget.limits.git_candidates:raise RuntimeError('locked Git candidates absent or exceed bound: '+repository)
            admissible=0
            for name in matches:
                size=seed._measure_git(anchor,('git','db',name),budget)
                enumerated+=size
                fits=total+size+len(seed.GIT_CONFIG)<128*MIB
                if fits: total+=size+len(seed.GIT_CONFIG);admissible+=1
                databases.append({'repository':repository,'candidate':name,'bytes':size,'fits_remaining_seed_cap':fits,'disposition':'metadata only; may be copied' if fits else 'oversized; seeder skips without copying'})
            if not admissible:missing.append(repository)
    return {'locked_registry_packages':len(registry),'git_candidates':databases,
            'admissible_candidate_and_registry_bytes':total,'enumerated_git_bytes':enumerated,'seed_cap_bytes':128*MIB,
            'seed_metadata_fits':total<128*MIB and not missing,'missing_admissible_repositories':missing,'entries':budget.entries_used,
            'limitation':'metadata only; rejects missing/unsafe inputs; no copy, checkout, build, commit verification or runtime measurement'}


def plan_document():
    return {'version':'oxide.outputs.delegation.proof/1','native_activated':False,'execution_ready':False,'readiness_meaning':'Source/installed-input plan only; a ready plan never activates execution.',
            'selectors':['proof:outputs-delegation:test','proof:outputs-delegation:plan','rust:outputs-delegation:diagnostic','rust:outputs-delegation'],
            'supervisor_sha256':SUPERVISOR,'barge':BARGE,'seconds':SECONDS,'scratch_mib':BUDGET,
            'work_stop_margin_s':30,'image_mib':128,'staging':'clean exact tree, Rust1.97.1 locked/offline, private Cargo seed, '+FEATURES,
            'fit':{'measured':False,'prospective_seconds':{'seed_and_source':45,'build_seal_cleanup':170,'cases':355,'closure_margin':30},
                   'estimate_basis':'170s existing stage ceiling + 45s bounded seed/source + 355s fixed case allowance + original30s margin. New coordinator prerequisite fixtures observed about20-28s for14 cases; this integration includes more command startup and pool/receipt work, so no linear equivalence is claimed.',
                   'unmeasured':'full positive/recovery and repeated command costs; new-protocol600s sufficiency is prospective, not established. Earlier300s/600s failed diagnostics retain their exact limits/outcomes/charges and open custody.',
                   'prospective_phase_mib':{'preparation':{'all_owned':448,'margin':64},'run':{'retained':288,'image':128,'fixture_and_evidence_growth':32,'margin':64}},
                   'seed_copy_mib':128,'artifact_lifecycle':'closed producer -> exclusive verified executable in evidence -> bound provenance -> dispose only fresh target -> verify retained copy -> resident gate',
                   'accounting':'all run-owned source, target, expanded Cargo cache including seed, home, tools copied if any, evidence, sealed artifact and other files; phase observations plus unchanged qualified guard'},
            'premises':[
                'Installed toolchain resolved before private HOME; absolute Cargo/rustc; AUTO_INSTALL=0.',
                'One Oxide private HOME route (TEST_HOME_FROM_ENV=1, no TEST_ACCOUNT_HOME) and Barge proof-account feature agree on that same home before fixture writes; production account lookup unproved.',
                'Scrub inherited product/Cargo/flags settings; retain genuine Nucleus hook and admission.',
                'Fixture roots are explicit synthetic history; no real checkout or auto-adoption. Actual competing adopt is a refusal case.',
                'Stage Cargo argv fixed locked/offline/bin oxide/features; exec writer uses explicit dependency-free command.',
                'Native entry binds its actual ImageCustody module to the permanent runtime; shared guard CLOCK_UPTIME_RAW deadline/30s margin checked before allocation. Default effects attach APFS owners-on; fixture helper rebinds kernel UUID.',
                'Stage and fixture RUSTFLAGS empty, no encoded overrides; stage feature/binary/tool/lock hashes retained.',
                'All scratch/TMP values are guard-provided, private worlds under scratch; image backing under guard evidence. Fixture outputs container uses actual volume.dir_at owner/type/device gate; fresh 0700 roots and exclusive no-follow 0600 marker/payload/receipt writes; product controls and negative cases unchanged.',
                'Every world uses /usr/bin/mktemp -d with explicit owned template ending XXXXXXXX.'],
            'cases':list(CASES),'limits':['no source test is native credit','no evaluator equivalence or production account qualification','unknown child/writer/image custody aborts and retains; no force or retry']}


def settlement_oracle_check():
    # Synthetic receipt/record images exercise this apparatus's evidence
    # requirements only; they are never presented as product outcomes.
    old=[dict(root='old',member=member,selected=False,pending=False,nonce='b'*32,checkout_id='a'*64) for member in ('debug','release')]
    successor=[dict(root='successor',member=member,selected=True,pending=False) for member in ('debug','release')]
    before=fixture_groups({'oxide':{'rows':old+successor}},'old','successor')
    payload=json.dumps({'terminal_ms':123})
    digest=hashlib.sha256(payload.encode()).hexdigest()
    record={'schema':'barge.operation/1','sequence':1,'phase':'complete','caller':'oxide.outputs','key':'d'*32,
            'request_digest':digest,'finalize_status':'acknowledged','finalize':{'payload':payload}}
    entry={'caller':'oxide.outputs','key':'d'*32,'root':'old','nonce':'b'*32,'checkout_id':'a'*64,
           'request_sha256':digest,'terminal_ms':123,'outcome':'deleted'}
    good={'oxide':{'rows':successor,'settled':[entry]},'barge':[record]}
    settled_group(good,'old','successor',before)
    bads=[]
    for mutate in (
        lambda doc:doc['oxide'].pop('settled'),
        lambda doc:doc['oxide']['settled'].append(dict(entry)),
        lambda doc:doc['oxide']['settled'][0].update(key='e'*32),
        lambda doc:doc['oxide']['settled'][0].update(request_sha256='e'*64),
        lambda doc:doc['oxide']['settled'][0].update(terminal_ms=124),
        lambda doc:doc['oxide']['rows'].append(old[0]),
        lambda doc:doc['oxide']['rows'].pop(),
        lambda doc:doc['oxide'].update(intents=[{'phase':'deleted'}]),
        lambda doc:doc['barge'][0].update(phase='deleted'),
        lambda doc:doc['barge'][0].update(finalize_status='owed'),
    ):
        doc=json.loads(json.dumps(good));mutate(doc);bads.append(doc)
    for bad in bads:
        try:settled_group(bad,'old','successor',before)
        except OracleFailure:pass
        else:raise AssertionError('missing/wrong atomic fact, group or actual acknowledgement accepted')


def busy_oracle_check():
    class Evidence:
        def __init__(self,events):self.events=events
        def __truediv__(self,key):return self
        def write_text(self,text):self.events.append('release')
    class Busy:
        data={'uuid':'synthetic'}
        def __init__(self,code=3,records=None):
            self.code,self.records,self.events=code,records or [],[]
            self.proof=type('Proof',(),{'evidence':Evidence(self.events)})()
        def preview(self):self.events.append('preview')
        def start(self,args):self.events.append('start');return object()
        def wait(self,*args):self.events.append('wait')
        def execute(self,expect):self.events.append('execute');require(expect==self.code,'busy begin exit differs')
        def kept(self):self.events.append('kept')
        def inspect(self):return {'barge':self.records}
        def finish(self,*args):self.events.append('finish');return 0,'',''
    world=Busy();run_case(world,'fresh-busy')
    require(world.events==['preview','start','wait','execute','kept','release','finish'],'busy case skipped a real command/closure obligation')
    for world in (Busy(code=0),Busy(records=[{'phase':'intent'}])):
        try:run_case(world,'fresh-busy')
        except OracleFailure:pass
        else:raise AssertionError('busy begin accepted an effect/record or wrong exit')
        require(world.events[-2:]==['release','finish'],'busy oracle failure abandoned its child')


def self_check():
    # Counterfactuals exercise orchestration oracles, not product semantics.
    load('delegation_controls','test-outputs-delegation-controls.py').check(sys.modules[__name__])
    settlement_oracle_check()
    busy_oracle_check()
    require(len(set(CASES))==len(CASES),'duplicate cases')
    good={'barge':[{'schema':'barge.operation/1','sequence':1,'key':'a'*32,'caller':'oxide.outputs','phase':'intent'}]}
    native_phase(good,'intent')
    for bad in ({'barge':[]},{'barge':[dict(good['barge'][0],phase='deleted')]}):
        try:native_phase(bad,'intent')
        except OracleFailure:pass
        else:raise AssertionError('missing/wrong actual operation accepted')
    require(len(pending({'barge':[{'phase':'intent'},{'phase':'complete'},{'phase':'cancelled'}]}))==1,'pending operation oracle')
    require(plan_document()['native_activated'] is False,'plan cannot activate native')
    fake={name:b'bound source' for name in ('Cargo.toml','crates/barge/Cargo.toml','crates/barge/src/operation.rs','crates/barge/src/oxide_outputs.rs','crates/barge/src/delete.rs','crates/barge/src/evidence.rs','crates/barge/src/reference.rs','crates/barge/src/state.rs')}
    digests={name:hashlib.sha256(data).hexdigest() for name,data in fake.items()}
    verify_source_files(digests,lambda parts:hashlib.sha256(fake['/'.join(parts)]).hexdigest())
    for altered in ('missing','changed'):
        bad=dict(digests)
        if altered=='missing':bad.pop('crates/barge/src/delete.rs')
        else:bad['crates/barge/src/delete.rs']='0'*64
        try:verify_source_files(bad,lambda parts:hashlib.sha256(fake['/'.join(parts)]).hexdigest())
        except OracleFailure:pass
        else:raise AssertionError('unbound coordinator source accepted')
    links={'crates/barge/src/alias.rs':'delete.rs'}
    verify_source_links(digests,links,lambda parts:'delete.rs')
    for bad in ('../delete.rs','missing.rs'):
        try:verify_source_links(digests,{'crates/barge/src/alias.rs':bad},lambda parts:bad)
        except OracleFailure:pass
        else:raise AssertionError('escaping/unbound source link accepted')
    try:verify_source_links(digests,links,lambda parts:'other.rs')
    except OracleFailure:pass
    else:raise AssertionError('changed source link accepted')
    binding_document()
    require(plan_document()['fit']['measured'] is False,'unmeasured plan claimed measurement')
    require(all(sum(phase.values())==BUDGET for phase in plan_document()['fit']['prospective_phase_mib'].values()),'phase envelope mismatch')
    require(sum(plan_document()['fit']['prospective_seconds'].values())==SECONDS,'time envelope mismatch')
    manifest=(HERE.parent/'effigy.toml').read_text()
    for selector,suffix in (('rust:outputs-delegation',''),('rust:outputs-delegation:diagnostic',' --diagnostic')):
        section=manifest.split('[tasks."'+selector+'"]',1)[1].split('[tasks.',1)[0]
        command=f'python3 scripts/bounded-test.py --limit {SECONDS} --scratch-budget-mib {BUDGET} -- python3 scripts/test-outputs-delegation.py'+suffix
        require('run = "'+command+'"' in section,'integrated selector/time envelope mismatch: '+selector)
    require(SECONDS==600 and plan_document()['work_stop_margin_s']==30,'prospective clock amendment mismatch')
    class StopProbe(Exception):pass
    class Probe:
        data={'uuid':'fixture','mount':'fixture'}
        name='probe'
        root=Path('/not-used')
        def __getattr__(self,key):
            def stop(*args,**kwargs):raise StopProbe(key)
            return stop
    # Every declared case must reach its named executable body. First operation
    # is intercepted, so no fixture, native child or host read can happen.
    original_writer,original_upgrade=globals()['held_writer'],globals()['upgrade_case']
    globals()['held_writer']=globals()['upgrade_case']=lambda *args: (_ for _ in ()).throw(StopProbe())
    for name in CASES:
        if name in ('volume-unregistered','internal-unknown-custody','removable-unknown-custody'):continue
        try:run_case(Probe(),name)
        except StopProbe:pass
        except (AttributeError,TypeError) as exc:
            raise AssertionError('case dispatch escaped its first action: '+name) from exc
    globals()['held_writer'],globals()['upgrade_case']=original_writer,original_upgrade
    try:native_phase({'barge':[dict(good['barge'][0],sequence=0)]},'intent')
    except OracleFailure:pass
    else:raise AssertionError('malformed actual Barge id accepted')
    print('outputs delegation apparatus self-check: OK (effect-free; no native credit)')
    return 0


def main(argv):
    if argv==['--self-check']:return self_check()
    if argv==['--plan']:
        doc=plan_document()
        try:
            doc['source_readiness']=source_readiness();doc['input_readiness']=input_fit()
            doc['execution_ready']=doc['input_readiness']['seed_metadata_fits']
        except Exception as exc:doc['impediment']=type(exc).__name__+': '+str(exc)[:1200]
        print(json.dumps(doc,indent=2));return 0 if doc['execution_ready'] else 2
    if argv not in ([],['--diagnostic']):raise SystemExit('use --self-check, --plan, --diagnostic or no args')
    proof=wrapper.Proof();proof.unclosed=set();proof.mount=None
    # These imports are distinct module instances. Wiring fresh.prior does
    # not initialize the ImageCustody/default effects used by this entry.
    prior.permanent=permanent
    prior.expect=permanent.expect
    prior.fail=permanent.fail
    image=prior.ImageCustody('Ox304'+secrets.token_hex(4),custody_protocol=True)
    return run_proof(bool(argv),proof,image)


def attach_image(proof,image):
    require(prior.permanent is permanent and prior.expect is permanent.expect
            and prior.fail is permanent.fail,'image helper runtime binding differs')
    require(proof.deadline_ns==int(os.environ['OXIDE_PROOF_DEADLINE_NS'])-wrapper.SHUTDOWN_MARGIN_NS
            and permanent.SHUTDOWN_MARGIN_S*wrapper.NS==wrapper.SHUTDOWN_MARGIN_NS,
            'image helper does not share the guard work deadline')
    # Use the original exported CLOCK_UPTIME_RAW deadline and 30s margin.
    # Missing/expired input refuses before allocation or an external effect.
    permanent.work_left()
    image.allocate(str(proof.evidence),str(proof.root))
    fresh.image_document(proof.evidence,'image-custody.json',{'version':1,'backings':[os.path.realpath(image.backing)]})
    image.attach();proof.mount=image.mount
    fresh.image_document(proof.evidence,'delegation-image.json',{'mount':image.mount,'volume_uuid':image.volume_uuid,'backing':image.backing})


def run_proof(diagnostic,proof,image):
    # Shared by native entry and local failure controls; executors are replaced
    # only by the self-check. Publication and error composition stay real.
    proof.secondary_errors=[]
    rows=[];error=None
    disposition='retained'
    try:
        source_readiness()
        # Selected scratch is verified by the real kernel before any backing
        # allocation. Neither quiet host nor task completion grants activation.
        code,out,err=proof.run(['/usr/sbin/diskutil','info','-plist',str(stash_mount(proof.root))],label='stash-identity',limit=10)
        import plistlib
        require(code==0 and plistlib.loads(out.encode()).get('VolumeUUID','').upper()==STASH,'scratch is not the approved Stash volume')
        binary,tools,env=stage(proof)
        resident=phase_snapshot(proof,'resident-before-image',RESIDENT_MIB*MIB)
        proof.resident_baseline=resident['total_after_publication_bytes']
        attach_image(proof,image)
        run_phase_snapshot(proof,image,'image-attached')
        for name in CASES:
            proof.remaining()
            try:
                world=World(proof,binary,tools,env,image,name)
                run_case(world,name)
                require(not proof.unclosed and proof.closure_known,'child custody not closed')
                rows.append({'case':name,'state':'completed'})
            except OracleFailure as exc:
                rows.append({'case':name,'state':'failed','reason':str(exc)[:1200]})
                if not diagnostic or proof.unclosed or not proof.closure_known:raise
            # Every partial case result is retained even if a later stage fails.
            write_json(proof.evidence/'delegation-partial.json',{'cases':rows,'qualified':False})
            run_phase_snapshot(proof,image,'case-'+name)
    except BaseException as exc:
        error=f'{type(exc).__name__}: {str(exc)[:1600]}'
    finally:
        if error is None and any(r['state']!='completed' for r in rows):error='classified oracle failures; failed worlds retained'
        try:
            disposition,detach_error=fresh.request_guard_detach(proof,image,error)
            error=error or (str(detach_error) if detach_error else None)
        except BaseException as exc:
            secondary_error(proof,'detach disposition',exc)
            error=error or f'{type(exc).__name__}: {str(exc)[:1600]}'
        observed={r['case'] for r in rows}
        rows.extend({'case':n,'state':'unexecuted'} for n in CASES if n not in observed)
        report={'version':'oxide.outputs.delegation.proof/1','diagnostic':diagnostic,'qualified':False,
                'cases':rows,'error':error,'secondary_errors':proof.secondary_errors,
                'image':image.state,'detach':disposition,'report_published':True,
                'elapsed_s':proof.elapsed(),'phase_accounting':getattr(proof,'stage_phases',[]),'limits':plan_document()['limits']}
        try:write_json(proof.evidence/'delegation-report.json',report)
        except BaseException as exc:
            secondary_error(proof,'final report publication',exc)
            error=error or f'{type(exc).__name__}: {str(exc)[:1600]}'
            report.update(error=error,report_published=False)
        # A failed final write still leaves bounded primary/secondary facts in
        # the guard's captured output. Never retry publication or hide failure.
        print(json.dumps(report,indent=2),flush=True)
    if error or proof.secondary_errors or disposition!='requested' or any(r['state']!='completed' for r in rows):return 1
    # Diagnostic is always uncounted/nonzero. Guard closure remains independent.
    return 1 if diagnostic else 0


if __name__=='__main__':
    raise SystemExit(main(sys.argv[1:]))
