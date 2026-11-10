#!/usr/bin/env python3
"""Integrated Oxide/Barge fixture proof (304). Native modes need planner activation.

Composition of the qualified shared guard, existing private source/seed builder,
image custody and direct-child helpers. No new supervisor, reserve sampler,
cleanup policy, alternate eligibility evaluator or Barge journal parser.
--self-check and --plan have no native/image/staging effects.
"""
from __future__ import annotations
import fcntl
import hashlib
import importlib.util
import json
import os
import subprocess
import plistlib
import re
from pathlib import Path
import secrets
import sys
import time

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
REPO = HERE.parent
SUPERVISOR = 'fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b'
BARGE = '63163686d212e6e5a900d6cc2ddfcce65b475890'
BARGE_REVIEWED = '547fbe99cdc9676b80b58730cf09715c64c5a9cd'
BARGE_TREE = '8a3b368e7883be862fe8319bb34e9f987e38731b'
STASH = '8A660440-C919-45C6-A3F1-3365B9BD2006'
SECONDS, MIB, BUDGET = 300, 1024 * 1024, 512
FEATURES = 'barge/proof-synthetic-account'
# Support is established from the published source export, lock and actual
# cached bytes below. No task-completion notice or boolean forces readiness.
SOURCE_BINDING = HERE / 'outputs-delegation-source.json'
# Every name has an executable body below. No native credit from self-checks.
CASES = (
    'positive-group', 'generic-release', 'selected', 'marker-nonce',
    'marker-digest', 'marker-owner', 'grant-withdrawn', 'grant-generation',
    'volume-kept', 'volume-unregistered', 'unavailable', 'grant-invalid', 'marker-volume',
    'lost-reply', 'malformed-reply', 'fresh-locked', 'resumed-locked',
    'intent-replay-oxide', 'intent-replay-barge', 'rename-replay-oxide',
    'partial-replay-barge', 'terminal-before-receipt', 'receipt-before-settled',
    'root-replaced', 'checkout-ancestor-replaced', 'missing-barge-after-rename', 'resolve-pending',
    'resolve-quarantine-original-absent', 'metadata-missing', 'metadata-corrupt',
    'metadata-swapped', 'adoption-race', 'internal-held-writer',
    'removable-held-writer', 'internal-unknown-custody', 'removable-unknown-custody',
    'sidecar-before-intent', 'oxide-intent-before-call', 'upgrade-internal-fence', 'upgrade-paired-fence',
    'upgrade-registry', 'upgrade-internal-ready', 'upgrade-paired-ready',
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
    prior.atomic_write(str(path), json.dumps(value, sort_keys=True).encode())


def last_json(text):
    for line in reversed(text.splitlines()):
        if line.startswith('{'):
            return json.loads(line)
    raise OracleFailure('child returned no JSON object')


def pending(doc):
    return [i for i in doc['oxide']['intents'] if not i.get('receipt_settled', False)]


def native_phase(doc, phase):
    """Require an actual typed Barge record, not a matching Oxide final row."""
    records = doc['barge']
    require(len(records) == 1, f'expected one actual Barge intent: {records}')
    record = records[0]
    require(record['phase'] == phase, f'actual Barge phase is {record["phase"]}, expected {phase}')
    require(len(record['intent_id']) == 32 and all(c in '0123456789abcdef' for c in record['intent_id']), 'actual Barge identity missing')
    return record


def fixture_env(proof, root, tools, stage_env):
    # Preserve the genuine inherited Nucleus hook; scrub product/tool overrides.
    env = proof.env()
    env = {k:v for k,v in env.items() if not k.startswith(('BARGE_', 'CARGO_', 'RUSTUP_', 'GIT_'))}
    home, cargo, state, scratch = [root / name for name in ('home','cargo-home','barge-state','scratch-pools')]
    for path in (home, cargo, state, scratch):
        path.mkdir(mode=0o700)
    env.update(HOME=str(home), BARGE_ACCOUNT_HOME=str(home), BARGE_STATE_DIR=str(state),
               OXIDE_TEST_ACCOUNT_HOME=str(home), OXIDE_TEST_HOME_FROM_ENV='1',
               OXIDE_POOL=str(scratch/'pool'), OXIDE_SCRATCH_ROOT=str(scratch),
               OXIDE_POOL_CAP='64MiB', OXIDE_POOL_CREATE='1', CARGO_HOME=str(cargo),
               CARGO_NET_OFFLINE='true', CARGO_BUILD_JOBS='2', RUSTUP_AUTO_INSTALL='0',
               RUSTUP_HOME=stage_env['RUSTUP_HOME'], RUSTUP_TOOLCHAIN=wrapper.BUILDER,
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
    require(set(doc)=={'version','published','reviewed','tree','files'},'source-binding fields differ')
    require(doc['version']=='oxide.outputs.delegation.source/1' and doc['published']==BARGE and doc['reviewed']==BARGE_REVIEWED and doc['tree']==BARGE_TREE,'unreviewed adapter source binding')
    return doc


def verify_source_files(files,reader):
    required={'Cargo.toml','crates/barge/Cargo.toml',*('crates/barge/src/'+name for name in ('oxide_outputs.rs','delete.rs','evidence.rs','reference.rs','state.rs'))}
    require(isinstance(files,dict) and required<=set(files) and len(files)<=128,'source binding omits deciding/replay source')
    for name,digest in sorted(files.items()):
        path=Path(name)
        require(not path.is_absolute() and all(p not in ('','.','..') for p in path.parts) and (name in ('Cargo.toml','crates/barge/Cargo.toml') or name.startswith('crates/barge/src/')),'unsafe bound source path')
        require(isinstance(digest,str) and re.fullmatch('[0-9a-f]{64}',digest),'invalid bound source digest')
        require(reader(path.parts)==digest,'cached adapter source differs: '+name)
    return {'files':len(files),'manifest_sha256':hashlib.sha256(json.dumps(files,sort_keys=True,separators=(',',':')).encode()).hexdigest()}


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
                verified=verify_source_files(doc['files'],digest)
                matches.append(dict(verified,path=str(cache.joinpath(*prefix)),bytes=used))
    require(len(matches)==1,'expected one exact reviewed adapter source export')
    return dict(matches[0],published=BARGE,reviewed=BARGE_REVIEWED,tree=BARGE_TREE)


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
    probe=subprocess.run(['/usr/sbin/diskutil','info','-plist',str(scratch)],capture_output=True,timeout=10,check=True)
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
    seed_provenance={}
    try:
        seed = wrapper.offline_seed.seed_cargo_home(host,cargo,lock,limits=wrapper.offline_seed.Limits(aggregate_bytes=128*MIB),deadline_ns=proof.deadline_ns,provenance=seed_provenance)
    finally:
        write_json(proof.evidence/'delegation-seed.json',seed_provenance)
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
    argv=[str(tools['cargo']),'build','--locked','--offline','--bin','oxide','--features',FEATURES]
    write_json(proof.evidence/'delegation-stage-inputs.json',{'head':head,'tree':tree,'source':binding,'lock_sha256':hashlib.sha256(lock).hexdigest(),'argv':argv,'features':FEATURES,'versions':versions,'flags':'','tools':{k:str(v) for k,v in tools.items()}})
    code,_,err = proof.run(argv,cwd=root,env=env,label='delegation-stage',limit=170)
    require(code == 0,err[-2000:])
    for item in seed['git_sources']:
        wrapper.offline_seed.verify_private_checkout(cargo,item['rev'],[c['database'] for c in item['candidates']],deadline_ns=proof.deadline_ns)
    require(wrapper.owned_bytes(root)+wrapper.owned_bytes(proof.root/'target')<=160*MIB,'staged source/target exceed prospective 160MiB allocation; no image allocated')
    binary = proof.root/'target/debug/oxide'
    provenance={'head':head,'tree':tree,'source':binding,'lock_sha256':hashlib.sha256(lock).hexdigest(),
                'barge':BARGE,'binary_sha256':wrapper.sha256_file(binary),
                'tools':{k:{'path':str(v),'sha256':wrapper.sha256_file(v)} for k,v in tools.items()},
                'versions':versions,'features':FEATURES,'argv':argv,'flags':'','seed':seed,
                'supervisor':SUPERVISOR,'proof_sha256':wrapper.sha256_file(Path(__file__)),
                'manifest_sha256':wrapper.sha256_file(REPO/'effigy.toml'),'adapter_source':adapter_binding(cargo)}
    write_json(proof.evidence/'delegation-stage.json',provenance)
    return binary, tools, env


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
        self.root=Path(self.data['root']); self.successor=Path(self.data['successor'])
        self.control=Path(self.data['control']); self.peer=Path(self.data['peer_control']); self.checkout=Path(self.data['checkout'])
        self.env['OXIDE_TEST_BARGE_RECORD_REPLY']=str(self.root)
        self.reply_records=set()
        if not name.startswith('upgrade-'):
            self.invoke(['outputs','upgrade-recovery'],expect=0,label='upgrade')
        self.plan_path=self.base/'presented.json'
        self.plan=None

    def invoke(self,args,*,expect=None,label='command',extra=None):
        env=dict(self.env,**(extra or {}))
        result=self.proof.run([str(self.binary),*map(str,args)],cwd=self.base,env=env,label=self.name+'-'+label,limit=15)
        code,out,err=result
        permanent.require_confirmed_closure(label,err)
        permanent.require_normal_exit(label,code)
        self.check_replies()
        if expect is not None and code!=expect:raise RuntimeError(f'{label}: unexpected exit {code}: {err[-1200:]}')
        return result

    def check_replies(self):
        if not hasattr(self,'reply_records'):return
        for path in self.proof.evidence.glob('barge-actual-reply-*.json'):
            if path in self.reply_records:continue
            value=json.loads(path.read_text())
            if value.get('root')!=str(self.root):continue
            self.reply_records.add(path)
            kind=value.get('kind');reason=str(value.get('reason') or '').lower()
            if any(word in reason for word in ('writer','descriptor','custody','process observation')) and kind not in ('Deleted','KnownNoEffectLocked'):
                raise RuntimeError('actual writer/custody uncertainty; retain and abort: '+str(value)[:2400])
            if kind in ('Unknown','Pending') and self.name!='root-replaced':
                raise RuntimeError('unexpected actual adapter uncertainty; retain and abort: '+str(value)[:2400])

    def helper(self,*args):
        _,out,_=self.invoke(['outputs','__delegation-fixture',*args],expect=0,label='fixture-'+str(args[0]))
        return last_json(out) if args[0] in ('setup','inspect','replay','generic-release') else None

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

    def kept(self):
        require(self.root.is_dir() and self.successor.is_dir(),'a kept root or successor changed')
        require(not list(self.root.parent.glob('.prune-*')),'local quarantine used for Barge route')

    def settled(self):
        doc=self.inspect()
        require(not self.root.exists() and self.successor.is_dir(),'root/successor deletion oracle failed')
        require(not pending(doc),'Oxide intent not settled')
        rows=[r for r in doc['oxide']['rows'] if r['root']==str(self.root)]
        require(len(rows)==2 and all(r.get('pruned_ms') is not None and not r.get('pruning') for r in rows),'whole group not settled')
        require(not list(self.root.parent.glob('.prune-*')),'local quarantine used')
        self.absence(native_phase(doc,'deleted'))
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
        child=permanent.start_child(self.proof,[str(self.binary),*map(str,args)],str(self.base),dict(self.env,**(extra or {})))
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
        self.check_replies()
        return code,out,err

    def wait(self,pattern,child):
        end=time.monotonic()+min(4,self.proof.remaining())
        while time.monotonic()<end:
            matches=[p for p in self.proof.evidence.glob(pattern) if p not in child.delegation_old_markers]
            if len(matches)==1:return matches[0]
            if child.poll() is not None: raise RuntimeError('child ended before observed durable point')
            time.sleep(.005)
        raise RuntimeError('bounded durable-point observation expired')

    def interrupt(self,phase,side='barge'):
        if self.plan is None:self.preview()
        extra=({'OXIDE_TEST_BARGE_DURABLE_PAUSE':str(self.root),'OXIDE_TEST_BARGE_PAUSE_PHASE':phase} if side=='barge' else {'OXIDE_TEST_BARGE_OXIDE_PAUSE':str(self.root),'OXIDE_TEST_BARGE_OXIDE_PHASE':phase})
        child=self.start(self.argv(),extra)
        try:
            self.wait(f'{side}-{phase}-*',child)
            doc=self.inspect()
            require(len(pending(doc))==1,'durable Oxide intent not observed')
            record=native_phase(doc,'intent' if phase in ('intent','renamed') else ('deleted' if side=='oxide' else 'renamed'))
            self.absence(record)
            original=self.root.exists()
            quarantine=self.root.parent/('.barge-prune-'+record['intent_id'])
            if phase=='partial':
                require(quarantine.is_dir(),'partial deletion quarantine absent')
                leaves=[p.name for p in quarantine.iterdir()]
                require(len([n for n in leaves if n.startswith('payload-')])<2 and bool(leaves),'no discriminating partial deletion observed')
            if phase=='intent': require(original,'intent-before-rename already renamed')
            else:require(not original,'post-rename original still exists')
            write_json(self.proof.evidence/(self.name+'-interruption.json'),{'pid':child.pid,'point':phase,'side':side,'observed':doc,'original_exists':original,'record':record})
            child.kill()
            self.finish(child,'interrupted',interrupted=True)
            return doc
        except BaseException:
            if child.poll() is None:
                child.kill()
                self.finish(child,'failed-interruption',interrupted=True)
            raise


def run_case(world, name):
    w=world
    if name.startswith('upgrade-'):
        upgrade_case(w,name.removeprefix('upgrade-'));return
    if name=='positive-group':w.preview();w.execute(expect=0);w.settled();return
    if name=='generic-release':
        report=w.helper('generic-release',w.data['uuid']); w.kept()
        require(not report['run'].get('rows') or all(r['outcome']!='released' for r in report['run']['rows']),report)
        require(not w.inspect()['barge'],'generic release wrote deletion intent');return
    if name=='selected':
        doc=w.preview();require(not any(c['root']==str(w.successor) for c in doc['candidates']),'selected root offered');w.execute(expect=0);w.settled();return
    if name in ('volume-kept','volume-unregistered','unavailable','grant-invalid'):
        if name=='volume-kept':w.helper('volume',w.data['uuid'],w.data['mount'],'kept')
        elif name=='volume-unregistered':
            # Private fixture registry loss, retained unchanged. Never journal parsing.
            path=Path(w.env['BARGE_STATE_DIR'])/'registry.json'
            path.rename(path.with_suffix('.retained'))
        elif name=='grant-invalid':w.helper('grant','invalid')
        else:w.helper('grant','withdraw')
        doc=w.preview(); require(any(c.get('barge_unavailable') for c in doc['candidates']),doc)
        w.execute(expect=3);w.kept();require(len(pending(w.inspect()))==1,'unavailable did not keep pending');return
    if name in ('lost-reply','malformed-reply'):
        w.preview();variable='OXIDE_TEST_BARGE_'+('LOST_REPLY' if name=='lost-reply' else 'MALFORMED_REPLY')
        w.execute(expect=3,extra={variable:str(w.root)})
        require(any(json.loads(p.read_text()).get('kind')=='Deleted' for p in w.reply_records),'lost/malformed case lacks actual Deleted reply')
        doc=w.inspect();native_phase(doc,'deleted');require(len(pending(doc))==1,'lost reply cleared exclusion')
        w.execute(expect=0,label='resume');w.settled();return
    if name in ('marker-nonce','marker-digest','marker-owner','marker-volume','grant-withdrawn','grant-generation','adoption-race'):
        if name=='grant-generation':w.helper('grant','present')
        w.preview();child=w.start(w.argv(),{'OXIDE_TEST_BARGE_BEFORE_CALL':str(w.root)})
        try:
            marker=w.wait('barge-before-call-*',child);doc=w.inspect();intent=pending(doc)[0]
            if name.startswith('marker-'):
                path=w.root/'.oxide-output.json';value=json.loads(path.read_text())
                key={'marker-nonce':'nonce','marker-digest':'label','marker-owner':'checkout_id','marker-volume':'volume_uuid'}[name]
                value[key]=('d'*32 if key=='nonce' else ('e'*64 if key=='checkout_id' else ('11111111-2222-3333-4444-555555555555' if key=='volume_uuid' else 'changed bytes')))
                write_json(path,value)
            elif name=='grant-withdrawn':w.helper('grant','withdraw')
            elif name=='grant-generation':w.helper('grant','older')
            else:
                # Actual competing adopt must encounter durable exclusion while
                # the caller retains the checkout barrier; do not mutate rows.
                w.checkout.mkdir()
                (w.checkout/'Cargo.toml').write_text('[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n')
                code,_,_=w.proof.run([str(w.binary),'outputs','adopt'],cwd=w.checkout,env=w.env,label=w.name+'-adopt-race',limit=4)
                require(code!=0,'adoption crossed a pending prune exclusion')
            (w.proof.evidence/('barge-call-release-'+intent['id'])).write_text('release')
            code,_,err=w.finish(child,'refusal')
            require(code in (0,2,3),err);w.kept();require(not w.inspect()['barge'],'refusal created a Barge effect')
        finally:
            if child.poll() is None:
                child.kill();w.finish(child,'refusal-stop',interrupted=True)
        return
    if name=='fresh-locked':
        w.preview();token=secrets.token_hex(16);child=w.start(['outputs','__delegation-fixture','lock',w.data['uuid'],token])
        try:
            w.wait('lock-'+token,child);w.execute(expect=0);w.kept()
            require(not pending(w.inspect()),'fresh known-no-effect Locked did not roll back')
        finally:
            (w.proof.evidence/('release-'+token)).write_text('release');w.finish(child,'lock')
        return
    if name in ('sidecar-before-intent','oxide-intent-before-call'):
        w.preview()
        sidecar=name=='sidecar-before-intent'
        extra=({'OXIDE_TEST_BARGE_OXIDE_PAUSE':str(w.root),'OXIDE_TEST_BARGE_OXIDE_PHASE':'request'} if sidecar else {'OXIDE_TEST_BARGE_BEFORE_CALL':str(w.root)})
        child=w.start(w.argv(),extra)
        try:
            w.wait('oxide-request-*' if sidecar else 'barge-before-call-*',child)
            doc=w.inspect()
            require(not doc['barge'] and len(pending(doc))==(0 if sidecar else 1),'pre-Barge publication boundary wrong')
            require(list((w.control/'outputs/prune-delegations').iterdir()),'synced original metadata missing')
            write_json(w.proof.evidence/(name+'-observed.json'),{'pid':child.pid,'observed':doc})
            child.kill();w.finish(child,'pre-Barge-interrupted',interrupted=True);w.kept()
            w.execute(expect=0,label='resume');w.settled()
        finally:
            if child.poll() is None:child.kill();w.finish(child,'pre-Barge-stop',interrupted=True)
        return
    if name in ('internal-held-writer','removable-held-writer'):
        held_writer(w,name.startswith('removable'));return
    if name in ('internal-unknown-custody','removable-unknown-custody'):
        control=w.peer if name.startswith('removable') else w.control
        # A real held flock means the actual writer witness is unavailable.
        # This is uncertainty in custody admission, not a fabricated process row.
        with (control/'admission.lock').open('rb') as handle:
            fcntl.flock(handle,fcntl.LOCK_EX|fcntl.LOCK_NB)
            before=fresh.tree_digest(str(w.control));doc=w.preview(candidate=False)
            require(not doc['coverage']['complete'] and not doc['candidates'],'unknown custody admitted a root')
            require('admission lock is busy' in json.dumps(doc),'unknown custody reason absent')
            require(fresh.tree_digest(str(w.control))==before,'unknown custody mutated journals')
        w.kept();return
    require(name in {'intent-replay-oxide','intent-replay-barge','rename-replay-oxide','partial-replay-barge','terminal-before-receipt','receipt-before-settled','root-replaced','checkout-ancestor-replaced','missing-barge-after-rename','resolve-pending','resolve-quarantine-original-absent','metadata-missing','metadata-corrupt','metadata-swapped','resumed-locked'}, 'unimplemented native case '+name)
    phase={'intent-replay-oxide':'intent','intent-replay-barge':'intent','rename-replay-oxide':'renamed','partial-replay-barge':'partial','terminal-before-receipt':'terminal','receipt-before-settled':'receipt'}.get(name,'renamed')
    if name in ('metadata-missing','metadata-corrupt','metadata-swapped','resumed-locked','resolve-pending'):
        phase='intent'
    doc=w.interrupt(phase,'oxide' if name in ('terminal-before-receipt','receipt-before-settled') else 'barge')
    intent=pending(doc)[0];actual=doc['barge'][0]['intent_id']
    if name=='checkout-ancestor-replaced':
        anchor=w.checkout.parent
        anchor.rename(anchor.with_name('retained-anchor'))
        anchor.mkdir(mode=0o700)
        require(anchor.stat().st_ino!=w.data['checkout_ancestor_inode'],'ancestor replacement did not change identity')
        # Actual replay must reprove the recorded ancestor. Its expected
        # conservative refusal is inspected directly, never retried destructively.
        report=w.helper('replay')
        require(bool(report['pending']),'changed ancestor was completed by Barge replay')
        require(len(pending(w.inspect()))==1,'changed ancestor cleared Oxide exclusion')
        return
    if name=='root-replaced':
        w.root.mkdir();(w.root/'replacement').write_text('different object')
        w.execute(expect=3);require((w.root/'replacement').exists(),'replacement deleted');return
    if name=='missing-barge-after-rename':
        state=Path(w.env['BARGE_STATE_DIR']);journal=state/'journal'
        journal.rename(state/'retained-journal');journal.mkdir(mode=0o700)
        w.execute(expect=3);require(len(pending(w.inspect()))==1,'missing Barge state was completion');return
    if name.startswith('metadata-'):
        files=list((w.control/'outputs/prune-delegations').glob('*.json'))
        require(len(files)==1,'expected exact original metadata')
        path=files[0]
        if name=='metadata-missing':path.rename(path.with_suffix('.retained'))
        elif name=='metadata-corrupt':path.write_bytes(b'{corrupt')
        else:
            value=json.loads(path.read_text());value['caller_intent_id']='f'*32;write_json(path,value)
        before=fresh.tree_digest(str(w.control));w.execute(expect=3);require(fresh.tree_digest(str(w.control))==before,'bad metadata changed exclusion');return
    if name in ('resolve-pending','resolve-quarantine-original-absent'):
        before=fresh.tree_digest(str(w.control));w.invoke(['outputs','resolve-intent',intent['id']],expect=2,label='resolve')
        require(fresh.tree_digest(str(w.control))==before,'pending Barge resolve changed journal')
        if name.endswith('absent'):require(not w.root.exists(),'original should be absent after real rename')
        return
    if name=='resumed-locked':
        child=w.start(w.argv(),{'OXIDE_TEST_BARGE_BEFORE_CALL':str(w.root)})
        lock=None;token=secrets.token_hex(16)
        try:
            w.wait('barge-before-call-'+intent['id'],child)
            lock=w.start(['outputs','__delegation-fixture','lock',w.data['uuid'],token]);w.wait('lock-'+token,lock)
            (w.proof.evidence/('barge-call-release-'+intent['id'])).write_text('release')
            code,_,_=w.finish(child,'resumed-locked');require(code==3,'resumed Locked rolled back')
            require(len(pending(w.inspect()))==1,'resumed Locked cleared pending');w.kept()
        finally:
            (w.proof.evidence/('release-'+token)).write_text('release')
            if lock is not None:w.finish(lock,'lock')
            if child.poll() is None:child.kill();w.finish(child,'resumed-stop',interrupted=True)
        return
    if name.endswith('barge'):
        w.helper('replay')
        # A pre-rename Barge replay can prove rollback; Oxide then reuses the
        # original request under its own intent, never another deleter.
    w.execute(expect=0,label='resume');w.settled()


def upgrade_case(w,phase):
    child=w.start(['outputs','upgrade-recovery'],{'OXIDE_TEST_RECOVERY_UPGRADE_PAUSE':str(w.control),'OXIDE_TEST_RECOVERY_UPGRADE_PHASE':phase})
    try:
        w.wait('upgrade-'+phase+'-*',child)
        states=[json.loads((p/'state.json').read_text()) for p in (w.control,w.peer)]
        registry=json.loads((w.control/'outputs/registry.json').read_text())
        expected={
            'internal-fence':(['/4','/3'],['initializing','ready'],2),
            'paired-fence':(['/4','/4'],['initializing','initializing'],2),
            'registry':(['/4','/4'],['initializing','initializing'],3),
            'internal-ready':(['/4','/4'],['ready','initializing'],3),
            'paired-ready':(['/4','/4'],['ready','ready'],3),
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
    child=permanent.start_child(w.proof,[str(w.binary),'exec','--','/usr/bin/python3',str(script)],str(w.checkout),dict(w.env,**env))
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
        doc=w.preview(candidate=False)
        require(not any(c['root']==str(w.root) for c in doc['candidates']),'actual held writer root offered')
        require('active writer' in json.dumps(doc),'held writer refusal not attributable')
        w.kept()
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
            'fit':{'measured':False,'prospective_seconds':{'seed_and_source':45,'build':170,'cases':55,'closure_margin':30},
                   'prospective_mib':{'seed':128,'source_and_target':160,'image_backing':128,'fixtures_and_evidence':32,'margin':64}},
            'premises':[
                'Installed toolchain resolved before private HOME; absolute Cargo/rustc; AUTO_INSTALL=0.',
                'Oxide synthetic account and Barge proof-account feature positively agree before fixture writes; production account lookup unproved.',
                'Scrub inherited product/Cargo/flags settings; retain genuine Nucleus hook and admission.',
                'Fixture roots are explicit synthetic history; no real checkout or auto-adoption. Actual competing adopt is a refusal case.',
                'Stage Cargo argv fixed locked/offline/bin oxide/features; exec writer uses explicit dependency-free command.',
                'Existing ImageCustody attaches APFS owners-on; fixture helper rebinds kernel UUID.',
                'Stage and fixture RUSTFLAGS empty, no encoded overrides; stage feature/binary/tool/lock hashes retained.',
                'All scratch/TMP values are guard-provided, private worlds under scratch; image backing under guard evidence.',
                'Every world uses /usr/bin/mktemp -d with explicit owned template ending XXXXXXXX.'],
            'cases':list(CASES),'limits':['no source test is native credit','no evaluator equivalence or production account qualification','unknown child/writer/image custody aborts and retains; no force or retry']}


def self_check():
    # Counterfactuals exercise orchestration oracles, not product semantics.
    require(len(set(CASES))==len(CASES),'duplicate cases')
    good={'barge':[{'intent_id':'a'*32,'phase':'intent'}]}
    native_phase(good,'intent')
    for bad in ({'barge':[]},{'barge':[{'intent_id':'a'*32,'phase':'deleted'}]}):
        try:native_phase(bad,'intent')
        except OracleFailure:pass
        else:raise AssertionError('phase oracle accepted missing/wrong actual record')
    require(pending({'oxide':{'intents':[{'receipt_settled':False},{'receipt_settled':True}]}})==[{'receipt_settled':False}],'pending settlement oracle')
    require(plan_document()['native_activated'] is False,'plan cannot activate native')
    fake={name:b'bound source' for name in ('Cargo.toml','crates/barge/Cargo.toml','crates/barge/src/oxide_outputs.rs','crates/barge/src/delete.rs','crates/barge/src/evidence.rs','crates/barge/src/reference.rs','crates/barge/src/state.rs')}
    digests={name:hashlib.sha256(data).hexdigest() for name,data in fake.items()}
    verify_source_files(digests,lambda parts:hashlib.sha256(fake['/'.join(parts)]).hexdigest())
    for altered in ('missing','changed'):
        bad=dict(digests)
        if altered=='missing':bad.pop('crates/barge/src/delete.rs')
        else:bad['crates/barge/src/delete.rs']='0'*64
        try:verify_source_files(bad,lambda parts:hashlib.sha256(fake['/'.join(parts)]).hexdigest())
        except OracleFailure:pass
        else:raise AssertionError('unbound adapter source accepted')
    binding_document()
    require(plan_document()['fit']['measured'] is False,'unmeasured plan claimed measurement')
    require(sum(plan_document()['fit']['prospective_mib'].values())==BUDGET,'envelope mismatch')
    require(sum(plan_document()['fit']['prospective_seconds'].values())==SECONDS,'time envelope mismatch')
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
    try:native_phase({'barge':[{'intent_id':'x'*32,'phase':'intent'}]},'intent')
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
    diagnostic=bool(argv);proof=wrapper.Proof();proof.unclosed=set();proof.mount=None
    rows=[];error=None;image=prior.ImageCustody('Ox304'+secrets.token_hex(4),custody_protocol=True)
    disposition='retained'
    try:
        source_readiness()
        # Selected scratch is verified by the real kernel before any backing
        # allocation. Neither quiet host nor task completion grants activation.
        code,out,err=proof.run(['/usr/sbin/diskutil','info','-plist',str(proof.root)],label='stash-identity',limit=10)
        import plistlib
        require(code==0 and plistlib.loads(out.encode()).get('VolumeUUID','').upper()==STASH,'scratch is not the approved Stash volume')
        binary,tools,env=stage(proof)
        image.allocate(str(proof.evidence),str(proof.root))
        fresh.image_document(proof.evidence,'image-custody.json',{'version':1,'backings':[os.path.realpath(image.backing)]})
        image.attach();proof.mount=image.mount
        fresh.image_document(proof.evidence,'delegation-image.json',{'mount':image.mount,'volume_uuid':image.volume_uuid,'backing':image.backing})
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
    except BaseException as exc:
        error=f'{type(exc).__name__}: {str(exc)[:1600]}'
    finally:
        if error is None and any(r['state']!='completed' for r in rows):error='classified oracle failures; failed worlds retained'
        disposition,detach_error=fresh.request_guard_detach(proof,image,error)
        error=error or (str(detach_error) if detach_error else None)
        observed={r['case'] for r in rows}
        rows.extend({'case':n,'state':'unexecuted'} for n in CASES if n not in observed)
        report={'version':'oxide.outputs.delegation.proof/1','diagnostic':diagnostic,'qualified':False,
                'cases':rows,'error':error,'image':image.state,'detach':disposition,
                'elapsed_s':proof.elapsed(),'limits':plan_document()['limits']}
        write_json(proof.evidence/'delegation-report.json',report)
        print(json.dumps(report,indent=2),flush=True)
    if error or disposition!='requested' or any(r['state']!='completed' for r in rows):return 1
    # Diagnostic is always uncounted/nonzero. Guard closure remains independent.
    return 1 if diagnostic else 0


if __name__=='__main__':
    raise SystemExit(main(sys.argv[1:]))
