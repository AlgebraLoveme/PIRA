"""Deterministic tests; --native runs required disposable native coverage (no skip)."""
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import team_store_relocation as helper

THREAD='00000000-0000-4000-8000-000000000001'

class FakeNative:
    created=0
    fail_phase=None
    corrupt=None
    def __init__(self,plan,runtime,on_pid):
        self.plan=plan; self.closed=True; self.phase=type(self).created
        type(self).created+=1
    def __enter__(self): self.closed=False; return self
    def __exit__(self,*_): self.closed=True
    def request(self,method,params):
        if self.phase==self.fail_phase: raise RuntimeError('injected native failure')
        if method in ('thread/read','thread/resume'):
            t={'id':THREAD,'historyMode':'paginated','status':{'type':'idle'},
               'path':str(Path(self.plan['destination'])/self.plan['rollout'])}
            if self.corrupt=='provider' and method=='thread/resume': t['modelProvider']='changed'
            if self.corrupt=='identity' and method=='thread/resume': t['id']='wrong'
            if self.corrupt=='path' and method=='thread/resume': t['path']=str(Path(self.plan['source'])/self.plan['rollout'])
            return {'thread':t}
        if method in ('thread/items/list','thread/turns/list'):
            value=2 if params.get('cursor') else 1
            if self.corrupt=='history' and self.phase>0: value+=100
            return {'data':[{'id':str(value),'text':'original'}], 'nextCursor':None if params.get('cursor') else 'next'}
        raise AssertionError(method)

class RelocationTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='team-relocation-',dir=os.environ.get('PIRA_RELOCATION_TEST_SCRATCH'))
        self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name).resolve(); self.source=self.root/'source'; self.dest=self.root/'final'
        self.source.mkdir(); self.journal=self.root/'repair.json'
        self.rollout='sessions/2026/10/08/rollout-test.jsonl'
        path=self.source/self.rollout; path.parent.mkdir(parents=True)
        path.write_text(json.dumps({'type':'session_meta','payload':{'id':THREAD,'cli_version':'0.161.0','history_mode':'paginated'}})+'\n'+json.dumps({'type':'response_item','payload':{'type':'message','content':[{'type':'input_text','text':'keep'}]}})+'\n')
        (self.source/'state_5.sqlite').write_bytes(b'opaque native fixture; not opened as SQL')
        shutil.copytree(self.source,self.dest)
        self.original=helper._inventory(self.source)[0]
        self.addCleanup(patch.stopall)
        patch('team_store_relocation.shutil.which',return_value=sys.executable).start()
        patch('team_store_relocation.subprocess.run',return_value=SimpleNamespace(stdout=helper.NATIVE_VERSION+'\n')).start()
        FakeNative.created=0; FakeNative.fail_phase=None; FakeNative.corrupt=None
    def plan(self):
        return helper._preflight_home(self.source,self.dest,THREAD,codex_binary='codex',journal_path=self.journal)
    def assert_restored(self):
        self.assertFalse(self.source.is_symlink())
        self.assertEqual(helper._inventory(self.source)[0],self.original)
    @patch.object(helper, '_VALIDATED_PLATFORMS', frozenset())
    def test_preflight_readonly_and_admission_disabled(self):
        before=sorted(str(p) for p in self.root.rglob('*'))
        plan=self.plan(); self.assertFalse(plan['admitted'])
        self.assertEqual(before,sorted(str(p) for p in self.root.rglob('*')))
        self.assertFalse(self.journal.exists()); self.assert_restored()
    def test_success_restores_original_and_never_renames_destination(self):
        identity=helper._identity(self.dest)
        evidence=helper._repair(self.plan(),FakeNative)
        self.assertEqual(evidence['thread_id'],THREAD); self.assertEqual(evidence['alias_absent_restarts'],2)
        self.assertEqual(helper._identity(self.dest),identity); self.assert_restored()
        self.assertTrue(helper.recover(self.journal)['source_restored'])
    def test_failure_at_each_native_phase_restores_source(self):
        for phase in range(4):
            with self.subTest(phase=phase):
                self.journal=self.root/('failure-'+str(phase)+'.json')
                FakeNative.created=0; FakeNative.fail_phase=phase
                with self.assertRaises(helper.RelocationError) as error: helper._repair(self.plan(),FakeNative)
                self.assertEqual(error.exception.code,'repair_failed'); self.assert_restored()
    def test_identity_path_and_history_fail_closed(self):
        for corrupt in ('identity','path','history','provider'):
            with self.subTest(corrupt=corrupt):
                self.journal=self.root/(corrupt+'.json'); FakeNative.created=0; FakeNative.corrupt=corrupt
                with self.assertRaises(helper.RelocationError): helper._repair(self.plan(),FakeNative)
                self.assert_restored()
    def test_changed_payload_since_preflight_refused(self):
        plan=self.plan(); (self.dest/'state_5.sqlite').write_bytes(b'new destination history')
        with self.assertRaises(helper.RelocationError): helper._repair(plan,FakeNative)
        self.assertEqual((self.dest/'state_5.sqlite').read_bytes(),b'new destination history'); self.assert_restored()
    def test_preflight_divergence_and_extra_sessions_refused(self):
        (self.dest/'state_5.sqlite').write_bytes(b'diverged')
        with self.assertRaises(helper.RelocationError): self.plan()
        shutil.copy2(self.source/'state_5.sqlite',self.dest/'state_5.sqlite')
        (self.source/'sessions'/'extra.jsonl').write_text('{}\n')
        shutil.copy2(self.source/'sessions'/'extra.jsonl',self.dest/'sessions'/'extra.jsonl')
        with self.assertRaises(helper.RelocationError): self.plan()
    def test_unknown_archived_and_credential_contents_refused_without_reading(self):
        for name in ('archived_sessions','unknown-assets'):
            path=self.source/name; path.mkdir()
            with self.assertRaises(helper.RelocationError): self.plan()
            path.rmdir()
        (self.source/'auth.json').write_text('synthetic sentinel')
        with patch.object(Path,'open',side_effect=AssertionError('must not read credentials')):
            # Direct inventory encounters root credential before traversing sessions.
            with self.assertRaises(helper.RelocationError): helper._inventory(self.source)
    def test_fork_and_media_rejected(self):
        p=self.source/self.rollout; original=p.read_text()
        for extra in [{'forked_from_id':THREAD},None]:
            rows=[json.loads(x) for x in original.splitlines()]
            if extra: rows[0]['payload'].update(extra)
            else: rows.append({'type':'response_item','payload':{'type':'input_image','image_url':'synthetic'}})
            p.write_text(''.join(json.dumps(x)+'\n' for x in rows)); shutil.copy2(p,self.dest/self.rollout)
            with self.assertRaises(helper.RelocationError): self.plan()
    def test_hardlinks_and_symlink_files_rejected(self):
        p=self.dest/'state_5.sqlite'; p.unlink(); os.link(self.source/'state_5.sqlite',p)
        with self.assertRaises(helper.RelocationError): self.plan()
        p.unlink(); p.symlink_to(self.source/'state_5.sqlite')
        with self.assertRaises(helper.RelocationError): self.plan()
    def journal_state(self):
        plan=self.plan(); token='1'*32
        state=dict(plan,schema=1,transaction=token,phase='aliased',native_uncertain=False,backend_pid=None,
                   backup=str(self.source.with_name('.source.pira-relocate-'+token)))
        helper._save(self.journal,state,first=True)
        backup=Path(state['backup']); self.source.rename(backup)
        return state,backup
    def test_crash_recovery_and_idempotence(self):
        state,backup=self.journal_state(); self.source.symlink_to(self.dest,target_is_directory=True)
        self.assertTrue(helper.recover(self.journal)['source_restored']); self.assert_restored()
        self.assertTrue(helper.recover(self.journal)['source_restored']); self.assertFalse(backup.exists())
    def test_replaced_alias_and_occupied_source_not_overwritten(self):
        state,backup=self.journal_state(); other=self.root/'other'; other.mkdir()
        self.source.symlink_to(other,target_is_directory=True)
        original_target=os.readlink(self.source)
        with self.assertRaises(helper.RelocationError): helper.recover(self.journal)
        self.assertTrue(backup.exists()); self.assertEqual(os.readlink(self.source),original_target)
        self.source.unlink(); self.source.mkdir()
        with self.assertRaises(helper.RelocationError): helper.recover(self.journal)
        self.assertTrue(backup.exists())
    def test_uncertain_native_child_requires_attested_exit(self):
        state,backup=self.journal_state(); state['native_uncertain']=True; helper._save(self.journal,state)
        with self.assertRaises(helper.RelocationError): helper.recover(self.journal)
        self.assertTrue(helper.recover(self.journal,native_stopped=True)['source_restored'])
    def test_recorded_live_pid_never_killed_or_ignored(self):
        state,backup=self.journal_state(); state['backend_pid']=os.getpid(); helper._save(self.journal,state)
        with self.assertRaises(helper.RelocationError): helper.recover(self.journal,native_stopped=True)
        self.assertTrue(backup.exists())
    def test_existing_journal_refused(self):
        self.journal.write_text('unrelated')
        with self.assertRaises(helper.RelocationError): self.plan()
        self.assertEqual(self.journal.read_text(),'unrelated')
    def managed_runs(self):
        old=self.root/'old'/'run-1'; new=self.root/'new'/'run-1'
        old.mkdir(parents=True); new.mkdir(parents=True)
        self.source.rename(old/'codex-home'); self.dest.rename(new/'codex-home')
        self.source=old/'codex-home'; self.dest=new/'codex-home'
        for home in (self.source,self.dest): (home/'thread_history_1.sqlite').write_bytes(b'opaque')
        manifest={'schema_version':4,'transport':'app-server','run_id':'run-1','thread_id':THREAD,'status':'completed'}
        for run in (old,new): (run/'manifest.json').write_text(json.dumps(manifest))
        return old,new
    @patch.object(helper, '_VALIDATED_PLATFORMS', frozenset())
    def test_source_only_planning_requires_no_destination(self):
        old,new=self.managed_runs(); shutil.rmtree(new)
        before=helper._inventory(self.source)[0]
        plan=helper.inspect_source(old,'run-1',THREAD,codex_binary='codex')
        self.assertFalse(plan['admitted']); self.assertEqual(plan['thread_id'],THREAD)
        self.assertEqual(helper._inventory(self.source)[0],before)
        self.assertFalse(new.exists()); self.assertFalse(self.journal.exists())
    @patch.object(helper, '_VALIDATED_PLATFORMS', frozenset())
    def test_public_manifest_preflight_and_gate(self):
        old,new=self.managed_runs()
        plan=helper.preflight(old,new,'run-1',THREAD,codex_binary='codex',journal_path=self.journal)
        self.assertEqual(plan['run_id'],'run-1')
        with self.assertRaises(helper.RelocationError) as error:
            helper.repair(old,new,'run-1',THREAD,codex_binary='codex',journal_path=self.journal)
        self.assertEqual(error.exception.code,'capability_pending'); self.assertFalse(self.journal.exists())
        manifest=json.loads((old/'manifest.json').read_text()); manifest['status']='running'
        (old/'manifest.json').write_text(json.dumps(manifest))
        with self.assertRaises(helper.RelocationError):
            helper.preflight(old,new,'run-1',THREAD,codex_binary='codex',journal_path=self.journal)
    def test_readonly_lineage_accepts_continuation_not_replacement(self):
        old,new=self.managed_runs()
        with patch.object(helper,'_NativeSession',side_effect=AssertionError('no backend allowed')):
            first=helper.validate_identity(old,new,'run-1',THREAD)
            self.assertFalse(first['native_verified'])
            p=self.dest/self.rollout
            with p.open('a') as out: out.write(json.dumps({'type':'response_item','payload':{'type':'message','text':'later'}})+'\n')
            (self.dest/'state_5.sqlite').write_bytes(b'legitimately evolved index')
            self.assertEqual(helper.validate_identity(old,new,'run-1',THREAD)['thread_id'],THREAD)
            rows=p.read_text().splitlines(); rows[1]=json.dumps({'type':'response_item','payload':{'type':'message','text':'replacement'}})
            p.write_text('\n'.join(rows)+'\n')
            with self.assertRaises(helper.RelocationError): helper.validate_identity(old,new,'run-1',THREAD)
    def test_missing_native_index_and_manifest_identity_refused(self):
        old,new=self.managed_runs(); (self.dest/'thread_history_1.sqlite').unlink()
        with self.assertRaises(helper.RelocationError): helper.validate_identity(old,new,'run-1',THREAD)
        with self.assertRaises(helper.RelocationError): helper.validate_identity(old,new,'wrong-run',THREAD)
    def test_journal_cannot_modify_immutable_source_run(self):
        old,new=self.managed_runs()
        with self.assertRaises(helper.RelocationError):
            helper.preflight(old,new,'run-1',THREAD,codex_binary='codex',journal_path=old/'journal.json')
        self.assertFalse((old/'journal.json').exists())
    def test_malformed_journal_returns_recovery_error_without_mutation(self):
        self.journal.write_text('malformed')
        with self.assertRaises(helper.RelocationError) as error: helper.recover(self.journal)
        self.assertEqual(error.exception.code,'recovery_required'); self.assert_restored()
    def test_windows_native_link_namespace_is_not_a_foreign_target(self):
        self.assertTrue(helper._link_target_equal('\\\\?\\C:\\data\\home','C:\\data\\home',True))
        self.assertTrue(helper._link_target_equal('\\\\?\\UNC\\host\\share','\\\\host\\share',True))
        self.assertFalse(helper._link_target_equal('C:\\data\\other','C:\\data\\home',True))
        self.assertFalse(helper._link_target_equal('C:\\data\\x\\..\\home','C:\\data\\home',True))
    def test_pid_probe_does_not_kill_or_ignore_live_owner(self):
        self.assertTrue(helper._pid_running(os.getpid()))
    def test_path_failure_reports_native_filesystem_identity_without_admitting_it(self):
        target=self.dest/self.rollout; wrong=self.source/self.rollout
        reply={'thread':{'id':THREAD,'historyMode':'paginated','status':{'type':'idle'},'path':str(wrong)}}
        with self.assertRaises(helper.RelocationError) as error: helper._metadata(reply,THREAD,target)
        detail=json.loads(str(error.exception).split(': ',1)[1])
        self.assertEqual(detail['native_path'],str(wrong))
        self.assertEqual(detail['expected_resolved'],str(target.resolve()))
        self.assertFalse(detail['samefile'])
        wrong.unlink(); os.link(target,wrong)
        with self.assertRaises(helper.RelocationError) as error: helper._metadata(reply,THREAD,target)
        self.assertTrue(json.loads(str(error.exception).split(': ',1)[1])['samefile'])
    def test_fixture_candidate_admission_is_reversible(self):
        import team_relocation_fixture as fixture
        for admitted in (False, True):
            platforms=frozenset({helper.platform.system()}) if admitted else frozenset()
            with self.subTest(admitted=admitted), patch.object(helper, '_VALIDATED_PLATFORMS', platforms):
                with self.assertRaisesRegex(RuntimeError,'fixture failure'):
                    with fixture.candidate_admission() as candidate_only:
                        self.assertEqual(candidate_only, not admitted)
                        self.assertTrue(helper.capabilities()['admitted'])
                        raise RuntimeError('fixture failure')
                self.assertIs(helper._VALIDATED_PLATFORMS,platforms)
    def test_native_path_accepts_only_equivalent_windows_namespace(self):
        same=helper._native_path_equal
        self.assertTrue(same(r'\\?\D:\final\rollout.jsonl',r'D:\final\rollout.jsonl',True))
        self.assertTrue(same(r'\\?\UNC\server\share\rollout',r'\\server\share\rollout',True))
        for wrong in (r'\\?\D:\old\rollout.jsonl',r'\\?\D:\final\..\final\rollout.jsonl',
                      r'\\.\D:\final\rollout.jsonl',r'D:\FINAL\rollout.jsonl'):
            self.assertFalse(same(wrong,r'D:\final\rollout.jsonl',True))
        self.assertFalse(same(r'\\?\D:\final\rollout.jsonl',r'D:\final\rollout.jsonl',False))
    def test_fixture_independence_survives_native_cleanup_but_rejects_renamed_hardlink(self):
        import team_relocation_fixture as fixture
        stale=self.source/'state_5.sqlite-wal'; stale.write_bytes(b'synthetic checkpoint input')
        # Reproduce the previous same-name assertion failure on a removed WAL.
        with self.assertRaises(FileNotFoundError): os.path.samefile(stale,self.dest/stale.name)
        fixture.assert_independent_files(self.source,self.dest)
        os.link(stale,self.dest/'renamed-native-bytes')
        with self.assertRaisesRegex(RuntimeError,'hard-linked'):
            fixture.assert_independent_files(self.source,self.dest)
    def test_junction_buffer_is_exact_mount_point_layout(self):
        import struct
        target=r'D:\native\final'
        data=helper._junction_buffer(target)
        tag,size,reserved,so,sl,po,pl=struct.unpack('<IHHHHHH',data[:16])
        self.assertEqual((tag,size,reserved),(0xA0000003,len(data)-8,0))
        names=data[16:]
        self.assertEqual(names[so:so+sl].decode('utf-16-le'),'\\??\\'+target)
        self.assertEqual(names[po:po+pl].decode('utf-16-le'),target)
        self.assertEqual(names[sl:sl+2],b'\0\0'); self.assertEqual(names[po+pl:],b'\0\0')
        for wrong in (r'\\server\share',r'D:\native\..\final','relative'):
            with self.assertRaises(helper.RelocationError): helper._junction_buffer(wrong)
    def test_junction_set_failure_restores_source_from_journaled_empty_directory(self):
        state,backup=self.journal_state(); state['alias_kind']='junction'
        with patch.object(helper,'_junction_buffer',return_value=b'probe'), patch.object(helper,'_set_junction',side_effect=OSError('set failed')):
            with self.assertRaisesRegex(OSError,'set failed'):
                helper._create_alias(self.source,self.dest,state,self.journal)
        self.assertEqual(state['phase'],'alias_prepared')
        self.assertEqual(helper._identity(self.source),state['alias_identity'])
        self.assertTrue(helper.recover(self.journal)['source_restored'])
        self.assert_restored()
    def test_junction_staged_directory_with_foreign_contents_is_not_removed(self):
        state,backup=self.journal_state(); state['alias_kind']='junction'
        with patch.object(helper,'_junction_buffer',return_value=b'probe'), patch.object(helper,'_set_junction',side_effect=OSError('set failed')):
            with self.assertRaises(OSError): helper._create_alias(self.source,self.dest,state,self.journal)
        (self.source/'foreign').write_bytes(b'preserve')
        with self.assertRaises(helper.RelocationError): helper.recover(self.journal)
        self.assertEqual((self.source/'foreign').read_bytes(),b'preserve'); self.assertTrue(backup.exists())
    def test_new_journal_rejects_same_target_replacement_alias(self):
        state,backup=self.journal_state(); state['alias_kind']='symlink'
        helper._create_alias(self.source,self.dest,state,self.journal)
        held=self.root/'held'; self.source.rename(held)
        self.source.symlink_to(self.dest,target_is_directory=True)
        with self.assertRaises(helper.RelocationError): helper.recover(self.journal)
        self.assertTrue(backup.exists()); self.assertTrue(self.source.is_symlink())
    def test_alias_type_mismatch_never_removes_link(self):
        alias=self.root/'alias'; alias.symlink_to(self.dest,target_is_directory=True)
        with self.assertRaises(helper.RelocationError):
            helper._remove_alias(alias,self.dest,'junction',helper._identity(alias))
        self.assertTrue(alias.is_symlink()); self.assertTrue(self.dest.exists())
    def test_no_inference_rpc_is_available(self):
        self.assertNotIn('turn/start',helper._METHODS); self.assertNotIn('thread/inject_items',helper._METHODS)
    def test_credential_environment_not_inherited(self):
        with patch.dict(os.environ,{'OPENAI_API_KEY':'synthetic','CODEX_HOME':'untrusted'}):
            env=helper._environment(self.root,self.dest)
        self.assertNotIn('OPENAI_API_KEY',env); self.assertEqual(env['CODEX_HOME'],str(self.dest))


class AdmissionTests(unittest.TestCase):
    def test_admission_is_limited_to_validated_platforms(self):
        for system, admitted in [('Darwin', True), ('Linux', True), ('Windows', True), ('Unknown', False)]:
            with self.subTest(system=system), patch.object(helper.platform, 'system', return_value=system):
                self.assertEqual(helper.capabilities()['admitted'], admitted)

if __name__=='__main__':
    if '--native' in sys.argv:
        import team_relocation_fixture
        raise SystemExit(team_relocation_fixture.main([x for x in sys.argv[1:] if x!='--native']))
    unittest.main()
