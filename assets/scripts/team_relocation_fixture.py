#!/usr/bin/env python3
"""Pinned native regression support, NEVER a selected-store migration command.

Python 3.9+, installed Codex 0.161.0, writable disposable --scratch, Windows local
drive junctions (POSIX directory symlinks) and stdio subprocesses required.
--completed-turns also needs loopback
bind; scripted local Responses only, no credentials or external inference.
Exit 2 (not skip) if required capability/check unavailable. No downloads.
"""
import argparse
import contextlib
import gzip
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import traceback
from unittest.mock import patch
import team_store_relocation as helper

class LocalResponses:
    def __enter__(self):
        owner = self
        self.requests = 0
        self.errors = []
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass
            def do_GET(self):
                self.send_error(404)
            def do_POST(self):
                try:
                    if self.path != '/v1/responses' or self.headers.get('Authorization'):
                        raise ValueError('unexpected provider route or auth')
                    length = int(self.headers.get('Content-Length','0'))
                    if not 0 < length <= 4*1024*1024:
                        raise ValueError('unexpected request size/encoding')
                    data = self.rfile.read(length)
                    encoding = self.headers.get('Content-Encoding', 'identity')
                    if encoding == 'gzip': data = gzip.decompress(data)
                    elif encoding != 'identity': raise ValueError('unsupported encoding: '+encoding)
                    body = json.loads(data)
                    if body.get('model') != 'relocation-probe' or body.get('stream') is not True:
                        raise ValueError('unexpected model or streaming mode')
                    owner.requests += 1
                    number = owner.requests
                    text = 'PIRA_LOCAL_RESPONSE_'+str(number)
                    rid, mid = 'resp_fixture_'+str(number), 'msg_fixture_'+str(number)
                    part = {'type':'output_text','text':text,'annotations':[]}
                    item = {'id':mid,'type':'message','role':'assistant','status':'completed',
                            'content':[part]}
                    response = {'id':rid,'object':'response','created_at':1,'status':'completed',
                                'model':'relocation-probe','output':[item],
                                'usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2}}
                    events = [
                        ('response.created',{'response':dict(response,status='in_progress',output=[])}),
                        ('response.output_item.added',{'output_index':0,'item':dict(item,status='in_progress',content=[])}),
                        ('response.content_part.added',{'item_id':mid,'output_index':0,'content_index':0,
                                                       'part':dict(part,text='')}),
                        ('response.output_text.delta',{'item_id':mid,'output_index':0,'content_index':0,'delta':text}),
                        ('response.output_text.done',{'item_id':mid,'output_index':0,'content_index':0,'text':text}),
                        ('response.content_part.done',{'item_id':mid,'output_index':0,'content_index':0,'part':part}),
                        ('response.output_item.done',{'output_index':0,'item':item}),
                        ('response.completed',{'response':response})]
                    payload = ''.join('event: '+name+'\ndata: '+json.dumps(dict(event,type=name,sequence_number=i))+'\n\n'
                                      for i,(name,event) in enumerate(events)).encode()
                    self.send_response(200)
                    self.send_header('Content-Type','text/event-stream')
                    self.send_header('Content-Length',str(len(payload)))
                    self.end_headers(); self.wfile.write(payload)
                except Exception as error:
                    owner.errors.append(repr(error)); self.send_error(400)
        # Never fall back to a remote provider or widen the bind address.
        self.server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.thread = threading.Thread(target=self.server.serve_forever,daemon=True)
        self.thread.start()
        self.url = 'http://127.0.0.1:'+str(self.server.server_address[1])+'/v1'
        return self
    def __exit__(self,*_):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=5)



class _RecordingQueue(queue.Queue):
    def __init__(self,completed):
        super().__init__(); self.completed=completed
    def put(self,value,*args,**kwargs):
        if isinstance(value,dict) and value.get('method')=='turn/completed':
            self.completed.put(value['params'])
        return super().put(value,*args,**kwargs)


class FixtureSession(helper._NativeSession):
    methods=helper._METHODS|{'thread/start','thread/inject_items','turn/start'}
    def __init__(self,*args):
        super().__init__(*args)
        self.completed=queue.Queue(); self.messages=_RecordingQueue(self.completed)


def inject(peer,thread,marker):
    peer.request('thread/inject_items',{'threadId':thread,'items':[{'type':'message','role':'user',
                 'content':[{'type':'input_text','text':marker}]}]})


@contextlib.contextmanager
def candidate_admission():
    """Use production admission when available; isolate candidate-only probes."""
    if helper.capabilities()['admitted']:
        yield False
        return
    previous=helper._VALIDATED_PLATFORMS
    try:
        helper._VALIDATED_PLATFORMS=frozenset({helper.platform.system()})
        yield True
    finally:
        helper._VALIDATED_PLATFORMS=previous


def file_snapshot(root):
    snapshot={}
    for path in root.rglob('*'):
        if path.is_symlink(): raise RuntimeError('unexpected alias left in idle fixture: '+str(path))
        if path.is_file(): snapshot[str(path.relative_to(root))]=helper._hash(path)
    return snapshot


def assert_independent_files(source, destination):
    # Native checkpoint/cleanup may remove copied WAL/SHM/lock files. The
    # invariant after native use is independent current bytes, not identical
    # directory membership. Source hashes and native full history are checked
    # separately. Check all destination names, including renamed hardlinks.
    source_ids=set()
    for path in source.rglob('*'):
        if path.is_file():
            stat=path.stat(); source_ids.add((stat.st_dev,stat.st_ino))
    for path in destination.rglob('*'):
        if path.is_file():
            stat=path.stat()
            if (stat.st_dev,stat.st_ino) in source_ids:
                raise RuntimeError('public caller hard-linked source/native payload: '+str(path))


def native_junction_checks(root):
    """Required Windows OS semantics, including same-target replacement refusal."""
    root.mkdir(); target=root/'target'; target.mkdir(); (target/'retained').write_bytes(b'keep')
    other=root/'other'; other.mkdir(); alias=root/'alias'
    def create(path,dest,name):
        state={'alias_kind':'junction'}
        _journal=root/(name+'.json')
        helper._create_alias(path,dest,state,_journal)
        if helper._alias_kind(path)!='junction': raise RuntimeError('native fixture did not create a mount-point junction')
        return state['alias_identity']
    identity=create(alias,target,'first')
    if (alias/'retained').read_bytes()!=b'keep': raise RuntimeError('junction did not resolve to intended bytes')
    held=root/'held'; alias.rename(held)
    replacement=create(alias,target,'replacement')
    try: helper._remove_alias(alias,target,'junction',identity)
    except helper.RelocationError: pass
    else: raise RuntimeError('same-target replacement junction was removed')
    helper._remove_alias(alias,target,'junction',replacement)
    retargeted=create(alias,other,'retargeted')
    try: helper._remove_alias(alias,target,'junction',retargeted)
    except helper.RelocationError: pass
    else: raise RuntimeError('retargeted junction was removed')
    try: helper._remove_alias(alias,other,'symlink',retargeted)
    except helper.RelocationError: pass
    else: raise RuntimeError('junction accepted as a symlink')
    helper._remove_alias(alias,other,'junction',retargeted)
    helper._remove_alias(held,target,'junction',identity)
    if (target/'retained').read_bytes()!=b'keep' or not other.is_dir():
        raise RuntimeError('junction removal damaged destination')
    return {'kind':'junction','target_preserved':True,'replacement_retarget_type_refused':True}


def public_caller_case(root,source_run,binary,thread_id,completed,before_history):
    """Actual caller APIs, not a native/helper double or OS-home discovery.

    Constructing the public StorePlan with explicit fixture profile paths avoids
    reading/writing real profiles or Windows registry while exercising exactly
    its migration-before-configuration publication path on every native runner.
    """
    import migrate_pira_stores as migration
    import setup_pira_stores as setup
    root.mkdir()
    target_store=root/'nonempty-team-store'; target_store.mkdir()
    unrelated=target_store/'unrelated-existing-record'; unrelated.mkdir()
    (unrelated/'retained.txt').write_bytes(b'unrelated existing destination data\n')
    unrelated_before=file_snapshot(unrelated)
    profile=root/'fixture.profile'
    old='fixture configuration before migration\n'
    new='fixture PIRA_TEAM_DIR='+str(target_store)+'\n'
    profile.write_bytes(old.encode('utf-8'))
    expected_id=source_run.name; target=target_store/expected_id
    source_fingerprint=migration.team_source_fingerprint(source_run)
    source_before=file_snapshot(source_run)
    with candidate_admission() as candidate_only:
        plans=migration.preflight_team_relocation([source_run.parent],target_store,codex_binary=binary)
        if len(plans)!=1: raise RuntimeError('public fixture expected one authoritative managed run')
        plan=plans[0]
        store_plan=setup.StorePlan(stores={'PIRA_TEAM_DIR':str(target_store)},
            profiles={profile:(old,new)},team_migrations=plans)
        # Both public planning modes must be nonmutating, including no native
        # app-server/cache creation. Verify is expected to reject unapplied data.
        before=file_snapshot(root)
        setup.apply_store_environment(store_plan,dry_run=True)
        try: setup.apply_store_environment(store_plan,dry_run=True,verify=True)
        except RuntimeError as error:
            if 'migration incomplete' not in str(error): raise
        else: raise RuntimeError('verify accepted an unpublished native migration')
        if file_snapshot(root)!=before or file_snapshot(source_run)!=source_before:
            raise RuntimeError('public dry-run/verify changed fixture data')
        # Real source conflict after planning: no mocked copy, helper or writer.
        # Restore only our synthetic manifest; failure must not publish config.
        manifest_path=source_run/'manifest.json'; original_manifest=manifest_path.read_bytes()
        active=json.loads(original_manifest); active['status']='running'
        try:
            manifest_path.write_text(json.dumps(active),encoding='utf-8')
            try: setup.apply_store_environment(store_plan,dry_run=False)
            except RuntimeError as error:
                if 'Active/ambiguous Team run' not in str(error): raise
            else: raise RuntimeError('source conflict passed the setup configuration barrier')
            if profile.read_text()!=old or target.exists() or (plan.ledger/'state.json').exists():
                raise RuntimeError('configuration or completed target published on failed migration')
        finally: manifest_path.write_bytes(original_manifest)
        # First public apply does all final-path copying, helper repair, durable
        # receipt publication and profile update. No native repair is doubled.
        setup.apply_store_environment(store_plan,dry_run=False)
        state_path=plan.ledger/'state.json'; state=json.loads(state_path.read_text())
        if state['phase']!='complete' or profile.read_text()!=new:
            raise RuntimeError('public migration/receipt/configuration did not complete')
        if state['fingerprint']!=source_fingerprint or state['receipt']['source_fingerprint']!=source_fingerprint:
            raise RuntimeError('public receipt did not retain immutable source provenance')
        if migration.team_source_fingerprint(source_run)!=source_fingerprint or file_snapshot(source_run)!=source_before:
            raise RuntimeError('public migration changed retained original source')
        if file_snapshot(unrelated)!=unrelated_before: raise RuntimeError('nonempty target records changed')
        assert_independent_files(source_run,target)
        evidence=state['repair_evidence']; thread=evidence['thread_id']
        if thread!=thread_id or evidence['run_id']!=expected_id:
            raise RuntimeError('public receipt retained wrong native identity')
        native_plan={'binary':binary,'destination':str(target/'codex-home')}
        with FixtureSession(native_plan,root/'real-continuation-runtime',lambda pid:None) as peer:
            resumed=peer.request('thread/resume',{'threadId':thread})['thread']
            if not helper._native_path_equal(resumed['path'],evidence['rollout']): raise RuntimeError('public target resolved stale path')
            if helper._history(peer,thread)!=before_history: raise RuntimeError('public copy lost full original native history')
            if completed:
                turn=peer.request('turn/start',{'threadId':thread,'input':[{'type':'text','text':'PIRA_PUBLIC_CONTINUATION'}]})['turn']
                event=peer.completed.get(timeout=30)
                if event['threadId']!=thread or event['turn']['id']!=turn['id'] or event['turn']['status']!='completed' or event['turn'].get('error') is not None:
                    raise RuntimeError('public migrated conversation could not complete another native turn')
            else: inject(peer,thread,'PIRA_PUBLIC_CONTINUATION')
            after_history=helper._history(peer,thread)
            if completed and len(after_history['thread/turns/list'])!=3:
                raise RuntimeError('public native continuation did not persist the third turn')
        if 'PIRA_PUBLIC_CONTINUATION' not in Path(evidence['rollout']).read_text():
            raise RuntimeError('public continuation missing from actual destination rollout')
        receipt_before=state_path.read_bytes(); journal=Path(evidence['journal']); journal_before=journal.read_bytes()
        # A fresh public plan must recognize the evolving target by receipt,
        # not first-copy equality. Read-only verify cannot spawn native services.
        rerun=migration.preflight_team_relocation([source_run.parent],target_store,codex_binary=binary)
        rerun_plan=setup.StorePlan(stores={'PIRA_TEAM_DIR':str(target_store)},team_migrations=rerun)
        before=file_snapshot(root)
        setup.apply_store_environment(rerun_plan,dry_run=True,verify=True)
        if file_snapshot(root)!=before: raise RuntimeError('receipt verify mutated the evolved destination')
        setup.apply_store_environment(rerun_plan,dry_run=False)
        if state_path.read_bytes()!=receipt_before or journal.read_bytes()!=journal_before:
            raise RuntimeError('rerun replaced receipt or repeated first native repair')
        with FixtureSession(native_plan,root/'post-rerun-runtime',lambda pid:None) as peer:
            resumed=peer.request('thread/resume',{'threadId':thread})['thread']
            if not helper._native_path_equal(resumed['path'],evidence['rollout']) or helper._history(peer,thread)!=after_history:
                raise RuntimeError('public rerun replaced or lost continued native history')
        if file_snapshot(source_run)!=source_before or migration.team_source_fingerprint(source_run)!=source_fingerprint:
            raise RuntimeError('public rerun modified immutable original source')
        if file_snapshot(unrelated)!=unrelated_before or profile.read_text()!=new:
            raise RuntimeError('public rerun changed unrelated data or published configuration')
        return {'public_setup_api':True,'candidate_admission_only':candidate_only,'nonempty_target_preserved':True,
                'source_preserved':True,'independent_files':True,'dry_run_verify_readonly':True,
                'failure_config_barrier':True,'receipt_unchanged_after_use':True,'rerun_without_repair':True,
                'native_continuation_turns':1 if completed else 0,'ledger':str(plan.ledger),
                'destination':str(target),'thread_id':thread}


def case(root,binary,completed,index):
    root.mkdir(); workspace=root/'workspace'; workspace.mkdir()
    source_run=root/'old'/'managed-run'; source_run.mkdir(parents=True)
    dest_run=root/'final'/'managed-run'; dest_run.mkdir(parents=True)
    source=source_run/'codex-home'; source.mkdir(); dest=dest_run/'codex-home'
    plan={'binary':binary,'destination':str(source)}
    with FixtureSession(plan,root/'create-runtime',lambda pid:None) as peer:
        thread=peer.request('thread/start',{'ephemeral':False,'model':'relocation-probe',
            'cwd':str(workspace),'approvalPolicy':'never','sandbox':'workspace-write',
            'baseInstructions':'Synthetic local fixture. No tools.', 'developerInstructions':''})['thread']
        thread_id=thread['id']
        for number in range(2):
            marker=f'PIRA_LOCAL_USER_{index}_{number}'
            if completed:
                turn=peer.request('turn/start',{'threadId':thread_id,'input':[{'type':'text','text':marker}]})['turn']
                event=peer.completed.get(timeout=30)
                if event['threadId']!=thread_id or event['turn']['id']!=turn['id'] or event['turn']['status']!='completed' or event['turn'].get('error') is not None:
                    raise RuntimeError('genuine native fixture turn failed: '+json.dumps(event))
            else: inject(peer,thread_id,marker)
        before_history=helper._history(peer,thread_id)
        if completed and (len(before_history['thread/turns/list'])!=2 or len(before_history['thread/items/list'])<4):
            raise RuntimeError('completed fixture lacks two full native turns')
    original=helper._inventory(source)[0]
    manifest={'schema_version':4,'transport':'app-server','run_id':'managed-run','thread_id':thread_id,'status':'completed'}
    for run in (source_run,dest_run):
        (run/'manifest.json').write_text(json.dumps(manifest))
        (run/'run.lock').write_bytes(b'0')
    shutil.copytree(source,dest)  # Independent files; no hardlink staging.
    journal=root/'repair.json'
    plan=helper.preflight(source_run,dest_run,'managed-run',thread_id,codex_binary=binary,journal_path=journal)
    # Exercise precisely the candidate transaction without enabling production.
    evidence=dict(helper._repair(plan),run_id='managed-run',source_run=str(source_run),destination_run=str(dest_run))
    if evidence['alias_kind']!=('junction' if os.name=='nt' else 'symlink'):
        raise RuntimeError('native fixture exercised the wrong alias route')
    if not evidence['source_restored'] or helper._inventory(source)[0]!=original:
        raise RuntimeError('original source not restored byte-identically')
    if helper._digest(before_history)!=evidence['history_fingerprint']:
        raise RuntimeError('source vs final full native history differs')
    # Source exists again: ensure lookup still chooses the destination, then
    # exercise a newly persisted no-inference write at that final location.
    with FixtureSession(plan,root/'after-restoration-runtime',lambda pid:None) as peer:
        result=peer.request('thread/resume',{'threadId':thread_id})
        target=dest/plan['rollout']
        if not helper._native_path_equal(result['thread']['path'],target): raise RuntimeError('destination resumed old source')
        if helper._history(peer,thread_id)!=before_history: raise RuntimeError('history mismatch on final restart')
        inject(peer,thread_id,'PIRA_DESTINATION_ONLY_'+thread_id)
    if 'PIRA_DESTINATION_ONLY_'+thread_id not in target.read_text():
        raise RuntimeError('new destination marker not physically persisted')
    if helper._inventory(source)[0]!=original: raise RuntimeError('destination write modified original source')
    readonly=helper.validate_identity(source_run,dest_run,'managed-run',thread_id)
    if readonly['native_verified']: raise RuntimeError('read-only check falsely claims native verification')
    continuation=helper._verify_native_identity(source_run,dest_run,'managed-run',thread_id,
        evidence=evidence,codex_binary=binary,runtime_dir=root/'continuation-runtime')
    if not continuation['native_verified']: raise RuntimeError('native continuation check failed')
    # Caller receipt semantics are deliberately NOT raw destination equality.
    # Do not repair the evolved destination again; make a fresh disposable copy
    # of the still-immutable source for the injected-failure regression.
    failed_run=root/'failure'/'managed-run'; failed_run.mkdir(parents=True)
    (failed_run/'manifest.json').write_text(json.dumps(manifest))
    failed=failed_run/'codex-home'; shutil.copytree(source,failed)
    fail_plan=helper.preflight(source_run,failed_run,'managed-run',thread_id,codex_binary=binary,journal_path=root/'failure.json')
    class FailingSession(helper._NativeSession):
        def request(self,method,params):
            if method=='thread/read': raise RuntimeError('fixture failure after alias and native startup')
            return super().request(method,params)
    try: helper._repair(fail_plan,FailingSession)
    except helper.RelocationError as error:
        if error.code!='repair_failed': raise
    else: raise RuntimeError('injected failure unexpectedly succeeded')
    if helper._inventory(source)[0]!=original or source.is_symlink(): raise RuntimeError('failure rollback lost source')
    if json.loads(Path(fail_plan['journal']).read_text())['phase']!='source_restored': raise RuntimeError('failure journal not recoverable')
    integration=public_caller_case(root/'public-integration',source_run,binary,thread_id,completed,before_history) if index==0 else None
    return dict(evidence, completed_turns=2 if completed else 0, destination_marker_persisted=True,
                rollback_verified=True, public_integration=integration, production_admitted=helper.capabilities()['admitted'])


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scratch',required=True,type=Path)
    parser.add_argument('--binary',default='codex')
    parser.add_argument('--completed-turns',action='store_true')
    args=parser.parse_args(argv)
    if not args.scratch.is_dir(): parser.error('--scratch must already exist and be disposable')
    binary=shutil.which(args.binary)
    if not binary: parser.error('installed pinned Codex required; no auto-install')
    root=Path(tempfile.mkdtemp(prefix='team-native-helper-',dir=args.scratch)).resolve()
    report={'root':str(root),'platform':helper.platform.system(),'completed_turns':args.completed_turns,'cases':[]}
    old_config=helper._CONFIG[:]; old_page=helper._PAGE_SIZE
    try:
        version=subprocess.run([binary,'--version'],env=helper._environment(root,root/'version-home'),
                               cwd=root,capture_output=True,text=True,check=True,timeout=15).stdout.strip()
        if version!=helper.NATIVE_VERSION: raise RuntimeError('requires '+helper.NATIVE_VERSION)
        report['version']=version; helper._PAGE_SIZE=1  # Force actual multi-page APIs.
        with (LocalResponses() if args.completed_turns else contextlib.nullcontext()) as provider:
            if provider:
                helper._CONFIG[:]=[('model_providers.relocation_probe.base_url='+json.dumps(provider.url))
                    if value.startswith('model_providers.relocation_probe.base_url=') else value for value in helper._CONFIG]
            # A privileged runner must not accidentally cover the old symlink
            # route instead. Windows required mode rejects any symlink attempt.
            with (patch.object(Path,'symlink_to',side_effect=AssertionError('Windows fixture forbids symlink creation'))
                  if os.name=='nt' else contextlib.nullcontext()):
                if os.name=='nt': report['junction_checks']=native_junction_checks(root/'junction-checks')
                for index in range(3):
                    report['cases'].append(case(root/('run-'+str(index)),str(Path(binary).resolve()),args.completed_turns,index))
            if provider and (provider.requests!=7 or provider.errors):
                raise RuntimeError('unexpected local response count/errors: '+repr(provider.errors))
            report['scripted_responses']=provider.requests if provider else 0
        report['status']='verified_fixture_only'
    except Exception as error:
        report.update(status='blocked',error=str(error),error_type=type(error).__name__,
                      traceback=traceback.format_exc(),filename=getattr(error,'filename',None),
                      filename2=getattr(error,'filename2',None))
    finally:
        helper._CONFIG[:]=old_config; helper._PAGE_SIZE=old_page
        (root/'result.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
        print(json.dumps(report,indent=2))
    return 0 if report['status']=='verified_fixture_only' else 2

if __name__=='__main__': raise SystemExit(main())
