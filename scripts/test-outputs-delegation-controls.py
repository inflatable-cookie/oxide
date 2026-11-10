#!/usr/bin/env python3
"""Local apparatus controls only: real JSON writer, fake execution boundaries."""
import ast
import inspect
import io
import json
import os
from pathlib import Path
import stat
import tempfile
from contextlib import ExitStack, redirect_stdout, redirect_stderr
from unittest.mock import patch
from types import SimpleNamespace


def imported_contracts(m):
    roots={'wrapper':m.wrapper,'prior':m.prior,'fresh':m.fresh,
           'permanent':m.permanent,'source':m.source,'seed':m.wrapper.offline_seed}
    calls=set()
    for node in ast.walk(ast.parse(Path(m.__file__).read_text())):
        if not isinstance(node,ast.Call):continue
        parts=[];target=node.func
        while isinstance(target,ast.Attribute):parts.insert(0,target.attr);target=target.value
        if not isinstance(target,ast.Name) or target.id not in roots:continue
        function=roots[target.id]
        for part in parts:function=getattr(function,part)  # old missing writer fails here
        assert callable(function),parts
        assert not any(isinstance(arg,ast.Starred) for arg in node.args)
        inspect.signature(function).bind(*([None]*len(node.args)),**{k.arg:None for k in node.keywords})
        calls.add('.'.join([target.id,*parts]))
    # Runtime receiver methods do not appear as module-qualified AST calls.
    for cls,name,args,keywords in (
        (m.wrapper.Proof,'run',[None,[]],{'label':'control','cwd':None,'env':{},'limit':1}),
        (m.wrapper.Proof,'env',[None,{}],{}),
        (m.wrapper.Proof,'remaining',[None],{}),
        (m.wrapper.Proof,'elapsed',[None],{}),
        (m.prior.ImageCustody,'allocate',[None,'evidence','scratch'],{}),
        (m.prior.ImageCustody,'attach',[None],{}),
        (m.wrapper.offline_seed.Anchor,'open_file',[None,('file',)],{}),
        (m.wrapper.offline_seed.Anchor,'open_dir',[None,('dir',)],{})):
        inspect.signature(getattr(cls,name)).bind(*args,**keywords)
    assert m.shutil.rmtree.avoids_symlink_attacks
    inspect.signature(m.shutil._rmtree_safe_fd).bind(0,'target',lambda *args:None)
    print('imported helper call shapes checked:',len(calls))


def publication(m,root):
    path=root/'partial.json'
    m.write_json(path,{'cases':[]})
    assert json.loads(path.read_text())=={'cases':[]}
    first=path.stat().st_ino
    m.write_json(path,{'cases':[{'state':'completed'}]})
    assert json.loads(path.read_text())=={'cases':[{'state':'completed'}]}
    assert path.stat().st_ino!=first and stat.S_IMODE(path.stat().st_mode)==0o600
    # Real imported writer's mode and replace failures must propagate.
    with patch.object(m.fresh.os,'chmod',side_effect=PermissionError('MODE_FAILURE')):
        try:m.write_json(root/'mode.json',{'bounded':True})
        except PermissionError as exc:assert str(exc)=='MODE_FAILURE'
        else:raise AssertionError('mode failure hidden')
    blocked=root/'directory.json';blocked.mkdir()
    try:m.write_json(blocked,{'bounded':True})
    except IsADirectoryError:pass
    else:raise AssertionError('publication failure hidden')
    m.fresh.image_document(root,'once.json',{'once':1})
    try:m.fresh.image_document(root,'once.json',{'once':2})
    except RuntimeError as exc:assert 'already exists' in str(exc)
    else:raise AssertionError('create-only custody indication replaced')
    assert json.loads((root/'once.json').read_text())=={'once':1}


def setup_failures(m,root):
    class Proof:
        def __init__(self,base):
            self.root=base;self.evidence=base/'evidence';self.evidence.mkdir()
            self.deadline_ns=1;self.unclosed=set();self.closure_known=True;self.mount=None
            self.commands=[]
        def elapsed(self):return 0.0
        def run(self,argv,*,label,limit):
            self.commands.append(label)
            if label=='stash-identity':return 0,m.plistlib.dumps({'VolumeUUID':m.STASH}).decode(),''
            assert label in ('resolve-cargo','resolve-rustc'),'real build/child boundary reached'
            return 0,str(root/label)+'\n',''
    class Image:
        state='unallocated'
        def allocate(self,*args):raise AssertionError('image allocation reached')
        def attach(self):raise AssertionError('image attach reached')
    for tool in ('resolve-cargo','resolve-rustc'):(root/tool).write_text('local control, never executed')
    rustup=root/'rustup';rustup.mkdir()
    for kind in ('setup','seed','seed-and-provenance','provenance','report','unwritable'):
        base=root/kind;base.mkdir();proof=Proof(base);events=[]
        if kind=='seed-and-provenance':(proof.evidence/'delegation-seed.json').mkdir()
        if kind=='provenance':(proof.evidence/'delegation-seed.json').mkdir()
        if kind=='report':(proof.evidence/'delegation-report.json').mkdir()
        if kind=='unwritable':proof.evidence.chmod(0o500)
        def readiness():
            events.append('readiness')
            if kind in ('setup','report'):raise RuntimeError('SETUP_PRIMARY')
        def archive(repo,destination,head,tree,*,deadline_ns):
            events.append('archive-fake')
            (destination/'Cargo.lock').write_text(m.BARGE)
            return {'local_control_only':True}
        def seed(host,cargo,lock,*,limits,deadline_ns,provenance):
            events.append('seed-fake');provenance['local_control_only']=True
            if kind!='provenance':raise RuntimeError('SEED_PRIMARY')
            return {'git_sources':[]}
        out,err=io.StringIO(),io.StringIO()
        try:
            with ExitStack() as stack:
                stack.enter_context(patch.object(m,'source_readiness',readiness))
                stack.enter_context(patch.object(m.prior,'ImageCustody',side_effect=AssertionError('orchestration replaced its supplied image executor')))
                stack.enter_context(patch.object(m,'stash_mount',lambda path:base))
                stack.enter_context(patch.object(m,'host_cache',lambda:base/'never-read-host'))
                stack.enter_context(patch.object(m.wrapper,'git',lambda proof,*args:'b'*40 if args[-1]=='HEAD^{tree}' else ('a'*40 if args[-1]=='HEAD' else '')))
                stack.enter_context(patch.object(m.source,'stage_git_tree',archive))
                stack.enter_context(patch.object(m.wrapper.offline_seed,'seed_cargo_home',seed))
                stack.enter_context(patch.dict(os.environ,{'RUSTUP_HOME':str(rustup)}))
                stack.enter_context(redirect_stdout(out));stack.enter_context(redirect_stderr(err))
                code=m.run_proof(True,proof,Image())  # real orchestration + real publication
        finally:proof.evidence.chmod(0o700)
        report=json.loads(out.getvalue())
        assert code==1 and report['qualified'] is False and report['detach']=='retained'
        assert len(out.getvalue())<64*1024 and len(report['cases'])==43
        assert all(row['state']=='unexecuted' for row in report['cases'])
        assert [row['case'] for row in report['cases']]==list(m.CASES)
        primary='SETUP_PRIMARY' if kind in ('setup','report') else ('IsADirectoryError' if kind=='provenance' else 'SEED_PRIMARY')
        assert primary in report['error'],(kind,report)
        assert not proof.unclosed and proof.closure_known
        assert not (proof.evidence/'image-custody.json').exists()
        assert not (proof.evidence/'delegation-stage-inputs.json').exists()
        if kind in ('report','unwritable'):
            assert report['report_published'] is False
            assert any('final report publication' in error for error in report['secondary_errors'])
            assert 'final report publication' in err.getvalue()
            assert not (proof.evidence/'delegation-report.json').is_file()
        else:
            assert report['report_published'] is True
            assert json.loads((proof.evidence/'delegation-report.json').read_text())==report
        if kind in ('seed-and-provenance','unwritable'):
            assert any('seed provenance publication' in error for error in report['secondary_errors'])
            assert 'seed provenance publication' in err.getvalue()
        if kind in ('setup','report'):assert events==['readiness'] and not proof.commands
        else:assert events==['readiness','archive-fake','seed-fake']
        if kind=='seed':
            assert json.loads((proof.evidence/'delegation-seed.json').read_text())=={'local_control_only':True}
            assert not report['secondary_errors']


def stage_contract(m,root,case="ok"):
    # Traverse the real stage function with synthetic, never-executed tools,
    # archive, seed and child executors; real publication/hashes stay active.
    base=root/('stage-'+case);base.mkdir();events=[]
    for name in ('cargo','rustc'):(base/name).write_text('local tool bytes, never executed')
    rustup=base/'rustup';rustup.mkdir()
    class Proof:
        root=base;deadline_ns=None;secondary_errors=[];tool_dirs=str(base)
        closure_known=True;unclosed=set()
        def remaining(self):
            if case=='expired' and (self.evidence/'candidate/oxide').exists():raise RuntimeError('EXPIRED_WINDOW')
            return 1
        def elapsed(self):return 0.0
        evidence=base/'evidence'
        def env(self,extra=None):return m.wrapper.Proof.env(self,extra)
        def run(self,argv,*,label,limit,cwd=None,env=None):
            events.append(label)
            if label=='stash-identity':return 0,m.plistlib.dumps({'VolumeUUID':m.STASH}).decode(),''
            if label.startswith('resolve-'):return 0,str(base/label.removeprefix('resolve-'))+'\n',''
            assert env['RUSTFLAGS']=='' and env['RUSTC_WRAPPER']==''
            assert 'CARGO_ENCODED_RUSTFLAGS' not in env
            assert env['RUSTUP_AUTO_INSTALL']=='0' and env['CARGO_NET_OFFLINE']=='true'
            assert env['HOME']==str(base/'stage-home') and env['CARGO_HOME']==str(base/'stage-cargo')
            if label.startswith('version-'):
                index=0 if label=='version-cargo' else 1
                return 0,m.wrapper.BUILDER_VERSIONS[index],' '
            assert label=='delegation-stage'
            assert argv==[str(base/'cargo'),'build','--locked','--offline','--bin','oxide','--features',m.FEATURES]
            binary=base/'target/debug/oxide'
            if case=='outside':
                outside=base/'outside';outside.mkdir();(base/'target/debug').symlink_to(outside,target_is_directory=True)
            else:binary.parent.mkdir(parents=True)
            if case!='missing':
                if case=='linked':binary.symlink_to(base/'cargo')
                else:binary.write_bytes(b'not an executable');binary.chmod(0o600 if case=='mode' else 0o700)
            if case=='unconfirmed':self.closure_known=False
            return 0,'',''
    proof=Proof();proof.evidence.mkdir()
    def archive(repo,destination,head,tree,*,deadline_ns):
        (destination/'Cargo.lock').write_text(m.BARGE)
        return {'local_control_only':True}
    def seed(host,cargo,lock,*,limits,deadline_ns,provenance):
        assert limits.aggregate_bytes==128*m.MIB
        provenance['local_control_only']=True
        (cargo/'expanded-cache.bin').write_bytes(b'expanded private cache')
        return {'git_sources':[{'rev':m.BARGE,'candidates':[{'database':'local-only'}]}]}
    def checkout(cargo,rev,idents,*,deadline_ns):
        assert rev==m.BARGE and idents==['local-only'];events.append('checkout-verified-fake');return 'local-only'
    original_copy=m.source.copy_candidate_artifact
    def copy(*args,**kwargs):
        if case=='copy':raise OSError('COPY_FAILURE')
        result=original_copy(*args,**kwargs)
        if case=='hash':Path(result['path']).write_bytes(b'wrong copied bytes')
        if case=='target-swap':
            (base/'target').rename(base/'retained-target');(base/'target').mkdir(mode=0o700)
        return result
    original_write=m.write_json
    def publish(path,value):
        if case=='publication' and path.name=='delegation-stage.json':raise OSError('PUBLICATION_FAILURE')
        return original_write(path,value)
    class Image:
        state='unallocated'
        def allocate(self,*args):events.append('IMAGE_ALLOCATION_REACHED');raise AssertionError('IMAGE_ALLOCATION_REACHED')
        def attach(self):raise AssertionError('IMAGE_ATTACH_REACHED')
    out=io.StringIO();err=io.StringIO()
    with ExitStack() as stack:
        for obj,name,value in ((m.wrapper,'git',lambda proof,*args:'b'*40 if args[-1]=='HEAD^{tree}' else ('a'*40 if args[-1]=='HEAD' else '')),
            (m.source,'stage_git_tree',archive),(m,'host_cache',lambda:base/'not-read-host'),
            (m.wrapper.offline_seed,'seed_cargo_home',seed),(m.wrapper.offline_seed,'verify_private_checkout',checkout),
            (m,'adapter_binding',lambda cache:{'local_control_only':True})):
            stack.enter_context(patch.object(obj,name,value))
        stack.enter_context(patch.dict(os.environ,{'RUSTUP_HOME':str(rustup),'CARGO_ENCODED_RUSTFLAGS':'must-be-scrubbed'}))
        stack.enter_context(patch.object(m.source,'copy_candidate_artifact',copy))
        stack.enter_context(patch.object(m,'write_json',publish))
        stack.enter_context(patch.object(m,'source_readiness',lambda:None))
        stack.enter_context(patch.object(m,'stash_mount',lambda path:base))
        if case=='cleanup':stack.enter_context(patch.object(m.shutil,'_rmtree_safe_fd',side_effect=OSError('CLEANUP_FAILURE')))
        if case=='over-prep':stack.enter_context(patch.object(m,'PREPARE_MIB',0))
        if case=='over-resident':stack.enter_context(patch.object(m,'RESIDENT_MIB',0))
        if case=='ok':binary,tools,env=m.stage(proof)
        else:
            stack.enter_context(redirect_stdout(out));stack.enter_context(redirect_stderr(err))
            code=m.run_proof(True,proof,Image())
    if case!='ok':
        report=json.loads(out.getvalue());assert code==1 and report['error']
        expected={'copy':'COPY_FAILURE','hash':'hash changed','publication':'PUBLICATION_FAILURE',
            'cleanup':'CLEANUP_FAILURE','unconfirmed':'custody','over-prep':'prepared-before-build',
            'over-resident':'resident-after-target-cleanup','expired':'EXPIRED_WINDOW',
            'missing':'FileNotFoundError','linked':'canonical regular','outside':'canonical regular',
            'mode':'mode is unsafe','target-swap':'identity changed'}[case]
        assert expected in report['error'],(case,report['error'])
        assert all(row['state']=='unexecuted' for row in report['cases']) and 'IMAGE_ALLOCATION_REACHED' not in events
        assert (base/'target').exists() or case=='over-resident'
        if case in ('hash','publication','cleanup','target-swap','expired','over-resident'):
            assert (proof.evidence/'candidate/oxide').is_file()
        return

    assert events==['resolve-cargo','resolve-rustc','version-cargo','version-rustc','delegation-stage','checkout-verified-fake']
    assert binary.read_bytes()==b'not an executable' and tools['cargo']==base/'cargo'
    assert binary==proof.evidence/'candidate/oxide' and stat.S_IMODE(binary.stat().st_mode)==0o700
    assert not (base/'target').exists() and (base/'source/Cargo.lock').is_file()
    inputs=json.loads((proof.evidence/'delegation-stage-inputs.json').read_text())
    final=json.loads((proof.evidence/'delegation-stage.json').read_text())
    assert inputs['features']==m.FEATURES and inputs['flags']==final['flags']==''
    assert final['seed']['git_sources'][0]['rev']==m.BARGE and final['adapter_source']=={'local_control_only':True}
    assert final['source']=={'local_control_only':True} and len(final['binary_sha256'])==64
    assert final['target']['cleanup']=='absent_verified' and final['producer']['reaped'] is True
    assert any(r['components']['expanded_cargo_including_seed']>0 and r['components']['evidence_including_sealed_artifact']>0 and r['components']['other_owned_bytes']>0 for r in proof.stage_phases)
    assert proof.stage_phases[-1]['components']['target']==0
    fixture=base/'fixture';fixture.mkdir()
    fixture_env=m.fixture_env(proof,fixture,tools,env)
    assert fixture_env['HOME']==fixture_env['BARGE_ACCOUNT_HOME']
    assert fixture_env['OXIDE_TEST_HOME_FROM_ENV']=='1' and 'OXIDE_TEST_ACCOUNT_HOME' not in fixture_env
    assert fixture_env['RUSTFLAGS']=='' and 'CARGO_ENCODED_RUSTFLAGS' not in fixture_env
    assert fixture_env['RUSTUP_AUTO_INSTALL']=='0'
    assert fixture_env.get('CARGO_TARGET_DIR')!=str(base/'target')


def run_accounting(m,root):
    base=root/'run-accounting';base.mkdir();evidence=base/'evidence';evidence.mkdir()
    class Proof:
        root=base
        def remaining(self):return 1
    proof=Proof();proof.evidence=evidence
    baseline=m.wrapper.owned_bytes(base)
    backing=evidence/'synthetic-accounting-image';backing.write_bytes(b'not an image')
    proof.resident_baseline=baseline
    image=type('Image',(),{'backing':str(backing)})()
    m.run_phase_snapshot(proof,image,'local-run-accounting')
    (base/'expanded-later-cache').write_bytes(b'counts as growth')
    with patch.object(m,'GROWTH_MIB',0):
        try:m.run_phase_snapshot(proof,image,'local-growth-refusal')
        except m.OracleFailure as error:assert 'growth exceeds' in str(error)
        else:raise AssertionError('fixture/evidence/cache growth was not charged')


def closure_request(m,root):
    base=root/'closure';base.mkdir()
    proof=type('Proof',(),{'evidence':base,'closure_known':True,'unclosed':set()})()
    image=type('Image',(),{'state':'attached','volume_uuid':'FAKE-CONTROL',
        'device':'/dev/fake-control','mount':'/not-mounted-control','backing':str(base/'never-created')})()
    for error,known,unclosed in (('PRIMARY',True,set()),(None,False,set()),(None,True,{123})):
        proof.closure_known=known;proof.unclosed=unclosed
        disposition,_=m.fresh.request_guard_detach(proof,image,error)
        assert disposition=='retained' and not (base/'detach-request.json').exists()
    proof.closure_known=True;proof.unclosed=set()
    assert m.fresh.request_guard_detach(proof,image,None)==('requested',None)
    request=json.loads((base/'detach-request.json').read_text())
    assert request['device']=='/dev/fake-control' and request['backing']==image.backing
    assert image.state=='attached'  # a request never establishes detach/absence
    assert not Path(image.backing).exists()


def child_contract(m,root):
    events=[]
    class Child:
        pid=123;returncode=0
        def communicate(self,timeout):events.append('communicate');return b'out',b'err'
    proof=type('Proof',(),{'unclosed':set(),'mount':None})()
    scratch=root/'scratch';scratch.mkdir()
    env={'OXIDE_POOL':str(scratch/'pool'),'OXIDE_SCRATCH_ROOT':str(scratch),'TMPDIR':str(scratch)}
    out=io.StringIO()
    with patch.object(m.permanent.subprocess,'Popen',return_value=Child()) as spawn,patch.object(m.permanent,'work_left',return_value=1),redirect_stdout(out):
        child=m.permanent.start_child(proof,['never-executed'],str(root),env)
        assert child.pid in proof.unclosed and spawn.call_count==1
        assert m.permanent.finish_child(proof,child,'local-control')==(0,'out','err')
    assert not proof.unclosed and events==['communicate']
    for code in (None,-9):
        try:m.permanent.require_normal_exit('control',code)
        except m.permanent.ProofAbort:pass
        else:raise AssertionError('unknown child exit accepted')
    try:m.permanent.require_confirmed_closure('control','could not confirm Cargo group')
    except m.permanent.ProofAbort:pass
    else:raise AssertionError('unknown writer closure accepted')


def native_image_composition(m,root):
    # Main, Proof construction, ImageCustody and DEFAULT CustodyEffects remain
    # real. Only process/kernel lookups are deterministic fakes. The successful
    # consumer stops at the real guard request: it asserts no detach/closure.
    original_run=m.run_proof
    for case in ('ok','missing-binding','wrong-binding','missing-deadline','expired',
                 'malformed-attach','unknown-volume','unknown-child'):
        base=root/('image-'+case);base.mkdir(mode=0o700)
        scratch=base/'scratch';scratch.mkdir(mode=0o700)
        evidence=base/'evidence';evidence.mkdir(mode=0o700)
        calls=[];now=[m.wrapper.NS];deadline=51*m.wrapper.NS
        def external(argv,*,capture_output,timeout):
            assert capture_output is True and timeout==20.0,(argv,timeout)
            calls.append(argv)
            if argv[:3]==['/usr/bin/hdiutil','create','-size']:
                assert argv[3:6]==['128m','-fs','APFS'] and argv[-3:-1]==['-type','SPARSE']
                backing=Path(argv[-1]);assert backing.parent==evidence
                indication=json.loads((evidence/'image-custody.json').read_text())
                assert indication['backings']==[str(backing)]
                backing.write_bytes(b'synthetic local bytes; never an image')
                return SimpleNamespace(returncode=0,stdout=b'',stderr=b'')
            if argv[:2]==['/usr/bin/hdiutil','attach']:
                assert argv[2:6]==['-nobrowse','-owners','on','-plist']
                name=Path(argv[-1]).name.removeprefix('152-image-').removesuffix('.sparseimage')
                data=m.plistlib.dumps({'system-entities':[
                    {'dev-entry':'/dev/control-container'},
                    {'dev-entry':'/dev/control-volume','mount-point':'/Volumes/'+name}]})
                return SimpleNamespace(returncode=0,stdout=b'bad plist' if case=='malformed-attach' else data,stderr=b'')
            assert argv==['/usr/sbin/diskutil','info','-plist','/dev/control-volume'],argv
            data={'VolumeUUID':'' if case=='unknown-volume' else 'CONTROL-UUID','ParentWholeDisk':'control-whole'}
            return SimpleNamespace(returncode=0,stdout=m.plistlib.dumps(data),stderr=b'')
        def consumer(diagnostic,proof,image):
            assert diagnostic and type(proof) is m.wrapper.Proof
            assert type(image) is m.prior.ImageCustody and image.effects is m.prior.REAL_CUSTODY_EFFECTS
            assert type(image.effects) is m.prior.CustodyEffects
            assert m.prior.permanent is m.permanent and m.prior.expect is m.permanent.expect and m.prior.fail is m.permanent.fail
            assert m.fresh.prior is not m.prior and m.fresh.prior.permanent is m.fresh.permanent
            # Audit the globals of every reached collaborator, including both
            # module instances and the real publication/detach request path.
            functions=[m.prior.ImageCustody.allocate,m.prior.ImageCustody.attach,
                m.prior.ImageCustody._write,m.prior.ImageCustody.verify_backing,
                m.prior.verify_evidence_root,m.prior.parse_attach_entities,
                m.permanent.work_left,m.fresh.image_document,m.fresh.atomic_write,
                m.fresh.request_guard_detach]
            functions += [getattr(m.prior.CustodyEffects,n) for n in
                ('stat','realpath','euid','exists','write_record','create_image','attach_raw','retain_attach_document','volume_facts')]
            for function in functions:
                assert all(value is not None for value in inspect.getclosurevars(function).globals.values()),function.__qualname__
            assert m.prior.CustodyEffects.create_image.__globals__ is vars(m.prior)
            assert proof.deadline_ns==deadline-30*m.wrapper.NS and m.permanent.work_left()==20.0
            if case=='missing-binding':m.prior.permanent=None
            if case=='wrong-binding':m.prior.permanent=m.fresh.permanent
            if case=='missing-deadline':os.environ.pop('OXIDE_PROOF_DEADLINE_NS')
            if case=='expired':now[0]+=16*m.wrapper.NS
            if case in ('ok','unknown-volume','unknown-child'):
                m.attach_image(proof,image)
                assert image.state=='attached' and image.mount==image.expected_mount
                assert image.device=='/dev/control-volume' and image.whole_disk=='/dev/control-whole'
                assert image.backing_identity['ino']==Path(image.backing).stat().st_ino
                assert Path(image.attach_document).read_bytes()
                if case=='unknown-child':proof.closure_known=False
                disposition,error=m.fresh.request_guard_detach(proof,image,None)
                if case=='ok':
                    assert (disposition,error)==('requested',None)
                    request=json.loads((evidence/'detach-request.json').read_text())
                    assert request==dict(version=1,backing=image.backing,mount=image.expected_mount,
                        device='/dev/control-volume',volume_uuid='control-uuid',note='fresh custody proof')
                else:assert disposition=='retained' and not (evidence/'detach-request.json').exists()
                assert image.state=='attached' and Path(image.backing).exists()  # never closed by a request
            else:
                # The real final-report/error path, with only unrelated stage
                # and kernel/child executors faked; no case or engine runs.
                out=io.StringIO()
                with patch.object(m,'source_readiness',lambda:None),patch.object(m,'stash_mount',lambda root:base), \
                     patch.object(m,'stage',lambda proof:(None,None,None)), \
                     patch.object(proof,'run',return_value=(0,m.plistlib.dumps({'VolumeUUID':m.STASH}).decode(),'')),redirect_stdout(out):
                    assert original_run(True,proof,image)==1
                report=json.loads((evidence/'delegation-report.json').read_text())
                assert report==json.loads(out.getvalue()) and report['report_published'] and not report['secondary_errors']
                assert all(r['state']=='unexecuted' for r in report['cases']) and report['detach']=='retained'
                expected={'missing-binding':'binding differs','wrong-binding':'binding differs',
                    'missing-deadline':'KeyError','expired':'work deadline','malformed-attach':'malformed'}[case]
                assert expected in report['error'],report
                if case=='malformed-attach':
                    assert image.state=='unknown' and len(calls)==2 and Path(image.backing).exists()
                    assert json.loads(Path(image.record).read_text())['state']=='unknown'
                else:assert not calls and image.state=='unallocated' and not (evidence/'image-custody.json').exists()
            assert not (evidence/'detach-request.json').exists() or case=='ok'
            return 73
        with ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ,{**{k:str(scratch) for k in m.wrapper.TEMP_VARS},
                'OXIDE_PROOF_EVIDENCE':str(evidence),'OXIDE_PROOF_DEADLINE_NS':str(deadline)}))
            stack.enter_context(patch.object(m.permanent.time,'clock_gettime_ns',side_effect=lambda clock:now[0]))
            stack.enter_context(patch.object(m.wrapper.shutil,'which',side_effect=lambda tool:str(base/tool)))
            stack.enter_context(patch.object(m.prior.subprocess,'run',external))
            for name in ('permanent','expect','fail'):stack.enter_context(patch.object(m.prior,name,None))
            stack.enter_context(patch.object(m,'run_proof',consumer))
            assert m.main(['--diagnostic'])==73
        assert all('detach' not in argv for argv in calls)
    print('native image composition: real entry/default effects, fake external executors; OK')


def fixture_composition(m,root):
    # Exercise real fixture_env, World, invoke/helper and child helpers. Fake
    # ONLY external executors; response data below is labelled synthetic and
    # makes no claim about any product command or eligibility outcome.
    base=root/'fixture-composition';base.mkdir(mode=0o700)
    scratch=base/'scratch';scratch.mkdir(mode=0o700)
    evidence=base/'evidence';evidence.mkdir(mode=0o700)
    rustup=base/'rustup';rustup.mkdir()
    mount=base/'synthetic-mount';mount.mkdir()
    image=SimpleNamespace(mount=str(mount),volume_uuid='synthetic-volume')
    tools={name:base/name for name in ('cargo','rustc')};binary=base/'never-executed'
    context={k:v for k,v in os.environ.items() if k.startswith('NUCLEUS_') or k=='EFFIGY_ADMISSION_HOOK'}
    dirty={k:'inherited-contamination' for k in ('OXIDE_TEST_ACCOUNT_HOME','OXIDE_TEST_HOME_FROM_ENV',
        'OXIDE_POOL','OXIDE_BARGE_UNAVAILABLE','BARGE_ACCOUNT_HOME','BARGE_STATE_DIR','XDG_CONFIG_HOME',
        'XDG_CACHE_HOME','CARGO_HOME','CARGO_TARGET_DIR','CARGO_ENCODED_RUSTFLAGS','GIT_CONFIG_GLOBAL',
        'RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','SCCACHE_DIR','RUSTUP_TOOLCHAIN','RUSTUP_HOME')}
    calls=[];data={};mode=['ok']
    def spawn(argv,*,env=None,cwd=None,stdout=None,stderr=None):
        calls.append((argv,cwd,env))
        if argv[0]=='/usr/bin/mktemp':
            assert argv[1:3]==['-d',str(scratch/('delegation-'+current[0]+'.XXXXXXXX'))]
            child_base=scratch/current[0];child_base.mkdir(mode=0o700)
            text=str(child_base)+'\n';err='';code=0
        else:
            assert argv[0]==str(binary) and cwd is not None
            home=str(Path(cwd)/'home')
            assert env['HOME']==env['BARGE_ACCOUNT_HOME']==home
            assert env['OXIDE_TEST_HOME_FROM_ENV']=='1' and 'OXIDE_TEST_ACCOUNT_HOME' not in env
            assert env['BARGE_STATE_DIR']==str(Path(cwd)/'barge-state') and env['CARGO_HOME']==str(Path(cwd)/'cargo-home')
            assert env['RUSTUP_HOME']==str(rustup) and env['RUSTUP_TOOLCHAIN']==m.wrapper.BUILDER
            assert env['CARGO']==str(tools['cargo']) and env['RUSTC']==str(tools['rustc'])
            assert env['RUSTUP_AUTO_INSTALL']=='0' and env['CARGO_NET_OFFLINE']=='true'
            assert env['CARGO_PROFILE_DEV_DEBUG']=='0' and env['CARGO_INCREMENTAL']=='0'
            assert env['RUSTFLAGS']==env['RUSTC_WRAPPER']=='' and env['CARGO_BUILD_JOBS']=='2'
            assert not any(k.startswith(('XDG_','GIT_','SCCACHE_')) for k in env)
            assert 'CARGO_ENCODED_RUSTFLAGS' not in env and 'CARGO_TARGET_DIR' not in env and 'RUSTC_WORKSPACE_WRAPPER' not in env
            assert 'OXIDE_BARGE_UNAVAILABLE' not in env
            assert all(env.get(k)==v for k,v in context.items())
            assert env['OXIDE_PROOF_DEADLINE_NS']==str(51*m.wrapper.NS) and env['OXIDE_PROOF_EVIDENCE']==str(evidence)
            assert all(env[k]==str(scratch) for k in m.wrapper.TEMP_VARS)
            text='';err='';code=0
            if argv[1:4]==['outputs','__delegation-fixture','setup']:
                assert argv[4:]==[m.hashlib.sha256(current[0].encode()).hexdigest()[:12],image.mount,image.volume_uuid]
                control=Path(cwd)/'scratch-pools/pool/oxide-managed'
                peer=control/'removable/oxide-managed'
                old=mount/('oxide-outputs/'+current[0]+'-old');successor=old.with_name(current[0]+'-successor')
                for path in (control,peer,old,successor):path.mkdir(parents=True,exist_ok=True)
                data.update(root=str(old),successor=str(successor),control=str(control),peer_control=str(peer),
                    checkout=home+'/checkouts/anchor/c',uuid=image.volume_uuid,mount=image.mount,
                    checkout_volume_uuid='synthetic-checkout-volume',checkout_ancestor_inode=1)
                text=json.dumps(data)
                if mode[0]=='setup-malformed':text='[]'
            elif argv[1:4]==['outputs','__delegation-fixture','inspect']:
                rows=[dict(root=data[key],member=member,selected=selected,pending=False)
                    for key,selected in (('root',False),('successor',True)) for member in ('debug','release')]
                text=json.dumps({'oxide':{'rows':rows,'intents':[]},'barge':[]})
                if mode[0]=='inspect-malformed':text='{"oxide":[],"barge":{}}'
            elif argv[1:4]==['outputs','__delegation-fixture','replay']:text='{"synthetic_control_only":true}'
            else:assert argv[1:]==['outputs','upgrade-recovery'] or argv[1:3]==['exec','--'],argv
            if mode[0]=='exit':code=2;err='SYNTHETIC_SETUP_REFUSAL'
            if mode[0]=='no-json':text='synthetic command has no response'
        class Child:
            pid=100+len(calls);returncode=None
            def wait(self,timeout):
                assert 0<timeout<=20
                stdout.write(text.encode());stderr.write(err.encode());self.returncode=code;return code
            def communicate(self,timeout):self.returncode=code;return text.encode(),err.encode()
        return Child()
    with ExitStack() as stack:
        stack.enter_context(patch.dict(os.environ,{**dirty,**{k:str(scratch) for k in m.wrapper.TEMP_VARS},
            'OXIDE_PROOF_EVIDENCE':str(evidence),'OXIDE_PROOF_DEADLINE_NS':str(51*m.wrapper.NS)}))
        stack.enter_context(patch.object(m.permanent.time,'clock_gettime_ns',return_value=m.wrapper.NS))
        stack.enter_context(patch.object(m.wrapper.shutil,'which',side_effect=lambda tool:str(base/tool)))
        stack.enter_context(patch.object(m.wrapper.subprocess,'Popen',spawn))
        proof=m.wrapper.Proof();proof.unclosed=set();proof.mount=None
        stage_env={'RUSTUP_HOME':str(rustup)}
        for current_name in ('positive-group','upgrade-internal-fence'):
            current=[current_name];start=len(calls)
            world=m.World(proof,binary,tools,stage_env,image,current_name)
            commands=[argv[1:] for argv,_,_ in calls[start:] if argv[0]==str(binary)]
            assert commands[0][:3]==['outputs','__delegation-fixture','setup']
            assert commands[-1]==['outputs','__delegation-fixture','inspect']
            assert (['outputs','upgrade-recovery'] in commands)==(current_name=='positive-group')
            if current_name.startswith('upgrade-'):
                world.invoke(['outputs','upgrade-recovery'],expect=0,label='local-upgrade-resume')
        assert world.helper('replay')=={'synthetic_control_only':True}
        child=world.start(['outputs','upgrade-recovery'],{'OXIDE_TEST_RECOVERY_UPGRADE_PHASE':'registry'})
        with redirect_stdout(io.StringIO()):assert world.finish(child,'local-replay')[0]==0
        assert not proof.unclosed
        # Held-writer's actual child helper consumes the same deciding env.
        with redirect_stdout(io.StringIO()):
            child=m.permanent.start_child(proof,[str(binary),'exec','--','/usr/bin/python3','never-executed.py'],str(world.base),world.command_env({'WRITER_GATE':str(world.base)}))
            assert m.permanent.finish_child(proof,child,'local-writer')[0]==0
        for extra in ({'OXIDE_TEST_ACCOUNT_HOME':str(world.base/'home')},{'OXIDE_TEST_HOME_FROM_ENV':''},
                      {'HOME':str(base)},{'BARGE_ACCOUNT_HOME':str(base)}, {'OXIDE_PROOF_DEADLINE_NS':'999999999999999'}):
            count=len(calls)
            try:world.invoke(['outputs','upgrade-recovery'],extra=extra)
            except m.OracleFailure:pass
            else:raise AssertionError('conflicting/missing/escaped private account reached command')
            assert len(calls)==count
        original_data=world.data
        for changes in ({'checkout':None},{'control':str(base)},{'root':str(scratch)}, {'uuid':'wrong-image'}):
            world.data=dict(original_data,**changes);count=len(calls)
            try:world.check_setup(image)
            except m.OracleFailure:pass
            else:raise AssertionError('missing/escaped/swapped setup binding accepted')
            assert len(calls)==count
        world.data=original_data
        for bad in ('setup-malformed','inspect-malformed','no-json','exit'):
            mode[0]=bad
            try:world.helper('setup',m.hashlib.sha256(current[0].encode()).hexdigest()[:12],image.mount,image.volume_uuid) if bad in ('setup-malformed','exit') else world.inspect()
            except (m.OracleFailure,RuntimeError):pass
            else:raise AssertionError('malformed/refused helper accepted: '+bad)
        mode[0]='ok'
        for invalid_root,invalid_tools,invalid_stage in ((base,tools,stage_env),(scratch/'missing',tools,stage_env),
                (world.base,{},stage_env),(world.base,tools,{})):
            count=len(calls)
            try:m.fixture_env(proof,invalid_root,invalid_tools,invalid_stage)
            except (m.OracleFailure,FileNotFoundError,KeyError):pass
            else:raise AssertionError('missing/escaped fixture input accepted')
            assert len(calls)==count
    print('fixture composition: actual environment/World/invoke/helpers, fake external commands; OK')


def check(m):
    imported_contracts(m)
    # Standard secure mkdtemp: no child is launched. Leave the owned root and
    # small failed-publication files as local-control evidence, never old roots.
    parent=Path(os.environ['TMPDIR']).resolve(strict=True)
    root=Path(tempfile.mkdtemp(prefix='delegation-controls.',dir=parent))
    assert root.parent==parent and stat.S_IMODE(root.stat().st_mode)==0o700
    print('local control root:',root)
    publication(m,root)
    setup_failures(m,root)
    child_contract(m,root)
    native_image_composition(m,root)
    fixture_composition(m,root)
    closure_request(m,root)
    run_accounting(m,root)
    for case in ('ok','missing','linked','outside','mode','copy','hash','publication','cleanup','unconfirmed','target-swap','expired','over-prep','over-resident'):
        stage_contract(m,root,case)
    allocated=m.wrapper.owned_bytes(root);assert allocated<=1024*1024,'local controls exceed1MiB allocation'
    print('local publication/setup/child controls: OK; allocated bytes',allocated,'; small owned files retained at',root)
