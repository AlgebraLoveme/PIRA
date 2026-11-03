"""Native Claude wire and provider-family tests, without inference or credentials."""
import json, os, shutil, subprocess, sys, unittest
from pathlib import Path
import test_team

def install_claude(root, environment):
    peer = Path(__file__).with_name('fake_claude.py')
    if os.name == 'nt':
        shutil.copyfile(os.environ['PIRA_TEAM_TEST_CLAUDE'], root / 'claude.exe')
        environment.update(TEAM_CLAUDE_PYTHON=sys.executable, TEAM_CLAUDE_PEER=str(peer))
    else:
        executable = root / 'claude'
        executable.write_text(peer.read_text()); executable.chmod(0o700)

class ClaudeTests(unittest.TestCase):
    def setUp(self):
        self.f=test_team.TeamTests(); self.f.setUp(); self.addCleanup(self.f.doCleanups)
        self.f.parent([{'sandbox_policy':{'type':'danger-full-access'}}])
        self.f.env.pop('CLAUDE_CODE_SESSION_ID',None)
        self.f.env['PYTHONPYCACHEPREFIX']=str(self.f.root/'pycache')
        install_claude(self.f.root, self.f.env)
        self.f.env['TEAM_CLAUDE_BIN'] = str(self.f.bin)
    def launch(self,task,*extra):
        return subprocess.run([str(self.f.bin),'run','--cwd',str(self.f.root),'--store',str(self.f.root/'logs'),
            '--model','claude-future-family-100','--effort','low','--task',task,'--completion-gate','fixture output',*extra],
            env=self.f.env,text=True,capture_output=True,timeout=10)
    @unittest.skipUnless(os.name == 'nt', 'Windows external-main caller regression')
    def test_windows_claude_main_uses_native_session_without_backend_ancestor(self):
        session = 'main-native-session'
        home = self.f.root / 'claude-main-home'
        project = home / 'projects' / 'fixture'
        project.mkdir(parents=True)
        transcript = project / (session + '.jsonl')
        def write(mode):
            transcript.write_text(json.dumps({'type':'user', 'sessionId':session,
                'cwd':str(self.f.root), 'permissionMode':mode,
                'message':{'content':'fixture task'}}) + '\n')
        self.f.env.update(CLAUDE_CONFIG_DIR=str(home), CLAUDE_CODE_SESSION_ID=session)
        write('bypassPermissions')
        result = self.launch('initial')
        self.assertEqual(result.returncode, 0, result.stderr)
        run = Path(json.loads(result.stdout)['run_root'])
        self.assertEqual(json.loads((run/'manifest.json').read_text())['creator_main_session_id'],
                         'claude-' + session)
        write('plan')
        rejected = self.launch('restricted')
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn('bypassPermissions', rejected.stderr)
        self.assertEqual(len(list((self.f.root/'logs').glob('*/manifest.json'))), 1)
    def test_launch_resume_and_cumulative_usage(self):
        first=self.launch('initial'); self.assertEqual(first.returncode,0,first.stderr)
        receipt=json.loads(first.stdout); run=Path(receipt['run_root'])
        manifest=json.loads((run/'manifest.json').read_text())
        self.assertEqual(manifest['transport'],'claude-stream-json')
        self.assertFalse((run/'codex-home').exists())
        self.assertEqual(manifest['usage']['input_tokens'],10)
        result=self.f.access('resume',receipt['run_id'],'--task','recall')
        self.assertEqual(result.returncode,0,result.stderr)
        new=json.loads((run/'manifest.json').read_text())
        self.assertEqual(new['thread_id'],manifest['thread_id'])
        self.assertEqual(new['usage']['input_tokens'],20)
        self.assertEqual(json.loads(Path(json.loads(result.stdout)['handoff_path']).read_text()),['initial','recall'])
    def test_parallel_workers_can_use_both_families(self):
        import concurrent.futures
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            a=pool.submit(self.launch,'claude-work')
            b=pool.submit(self.f.launch,'codex-work')
            results=[a.result(),b.result()]
        for result in results: self.assertEqual(result.returncode,0,result.stderr)
        manifests=[json.loads((Path(json.loads(result.stdout)['run_root'])/'manifest.json').read_text()) for result in results]
        self.assertEqual({m['transport'] for m in manifests},{'app-server','claude-stream-json'})
        self.assertEqual(len({m['creator_main_session_id'] for m in manifests}),1)

    @unittest.skipUnless(os.name == "posix", "POSIX PATH symlink fixture")
    def test_worker_context_preserves_role_when_path_changes(self):
        import sys
        tests = Path(__file__).parent
        native = self.f.root / 'versioned-native-image'
        subprocess.run(['rustc', '--edition=2024', str(tests / 'native_fake_claude.rs'),
                        '-o', str(native)], check=True, capture_output=True)
        (self.f.root / 'claude').unlink()
        (self.f.root / 'claude').symlink_to(native)
        codex_only = self.f.root / 'codex-only'; codex_only.mkdir()
        (codex_only / 'codex').symlink_to(self.f.root / 'codex')
        (codex_only / 'python3').symlink_to(sys.executable)
        replacement = self.f.root / 'replacement'; replacement.mkdir()
        # Ambient CLI resolution does not change the supplied run/session context.
        (replacement / 'claude').symlink_to(self.f.root / 'codex')
        self.f.env.update(TEAM_CLAUDE_PYTHON=sys.executable,
            TEAM_CLAUDE_PEER=str(tests / 'fake_claude.py'), TEAM_CLAUDE_BIN=str(self.f.bin),
            TEAM_CLAUDE_STORE=str(self.f.root / 'logs'), TEAM_CLAUDE_CODEX_ONLY=str(codex_only),
            TEAM_CLAUDE_CODEX_HOME=str(self.f.root / 'original-home'),
            TEAM_CLAUDE_REPLACEMENT=str(replacement))
        result = self.launch('context-probe')
        self.assertEqual(result.returncode, 0, result.stderr)
        run = Path(json.loads(result.stdout)['run_root'])
        checks = json.loads(Path(json.loads(result.stdout)['handoff_path']).read_text())
        self.assertEqual(len(checks), 6)
        for check in checks:
            with self.subTest(case=check['case'], operation=check['operation']):
                self.assertNotEqual(check['returncode'], 0, check)
        self.assertFalse((self.f.root / 'logs/worker_defaults.json').exists())
        self.assertEqual(len(list((self.f.root / 'logs').glob('*/manifest.json'))), 1)
        self.assertFalse(json.loads((run / 'manifest.json').read_text())['allow_subteam'])
    def test_original_cli_error_is_returned(self):
        result=self.launch('stderr-failure')
        self.assertNotEqual(result.returncode,0)
        self.assertIn('original Claude CLI diagnostic: fixture refusal',result.stderr)

    def test_added_guidance_survives_later_resume(self):
        receipt=json.loads(self.launch('initial').stdout)
        run=Path(receipt['run_root'])
        added=self.f.access('resume',receipt['run_id'],'--inject-write','--task','with writing')
        self.assertEqual(added.returncode,0,added.stderr)
        again=self.f.access('resume',receipt['run_id'],'--task','still writing')
        self.assertEqual(again.returncode,0,again.stderr)
        writing=Path(__file__).resolve().parents[4]/'modules/SCIENTIFIC_WRITING.md'
        self.assertIn(writing.read_bytes(),(run/'revisions/000003/policy.md').read_bytes())
        self.assertEqual((run/'revisions/000003/phase.md').read_bytes().count(writing.read_bytes()),0)

    def test_codex_coordinator_launches_claude_child(self):
        import test_subteams
        f=test_subteams.SubteamTests();f.setUp();self.addCleanup(f.doCleanups)
        f.parent([{'sandbox_policy':{'type':'danger-full-access'}}])
        f.env['PYTHONPYCACHEPREFIX']=str(f.root/'pycache')
        install_claude(f.root, f.env)
        process,coordinator=f.start()
        child=f.command(coordinator,'run','--model','claude-future-family-100','--effort','low','--task','child-work','--completion-gate','fixture child completion')
        self.assertEqual(child['returncode'],0,child)
        receipt=json.loads(child['stdout'])
        manifest=json.loads((Path(receipt['run_root'])/'manifest.json').read_text())
        self.assertEqual(manifest['parent_run_id'],coordinator)
        self.assertEqual(manifest['transport'],'claude-stream-json')
        f.request(coordinator,operation='finish')
        process.communicate(timeout=10)
        self.assertEqual(process.returncode,0)

    def test_claude_coordinator_launches_codex_child(self):
        result = self.launch('delegate-probe', '--allow-subteam')
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        parent = json.loads((Path(receipt['run_root']) / 'manifest.json').read_text())
        report = json.loads(Path(receipt['handoff_path']).read_text())
        self.assertEqual(report['returncode'], 0, report)
        child = report['manifest']
        self.assertEqual(child['parent_run_id'], receipt['run_id'])
        self.assertEqual(child['transport'], 'app-server')
        self.assertEqual(child['creator_main_session_id'], 'claude-' + parent['thread_id'])
        self.assertFalse(child['allow_subteam'])

    def test_refuses_backend_switch_on_resume(self):
        receipt=json.loads(self.launch('initial').stdout)
        path=Path(receipt['run_root'])/'manifest.json'; before=path.read_bytes()
        result=self.f.access('resume',receipt['run_id'],'--model','gpt-test-model')
        self.assertNotEqual(result.returncode,0); self.assertIn('different backend family',result.stderr)
        self.assertEqual(path.read_bytes(),before)
    def test_rejects_wrong_session_and_error_results(self):
        for task in ['wrong-session','error-result']:
            with self.subTest(task=task):
                result=self.launch(task); self.assertNotEqual(result.returncode,0,result.stdout)
    def test_rejects_restricted_caller_and_unrepresentable_effort(self):
        self.f.parent([{}])
        result=self.launch('initial'); self.assertNotEqual(result.returncode,0)
        self.assertIn('full-access',result.stderr)
        self.f.parent([{'sandbox_policy':{'type':'danger-full-access'}}])
        result=subprocess.run([str(self.f.bin),'run','--model','claude-future','--effort','ultra','--task','x','--completion-gate','x','--cwd',str(self.f.root),'--store',str(self.f.root/'logs')],env=self.f.env,text=True,capture_output=True)
        self.assertNotEqual(result.returncode,0);self.assertIn('does not support reasoning effort',result.stderr)

if __name__=='__main__': unittest.main()
