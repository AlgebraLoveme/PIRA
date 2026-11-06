#!/usr/bin/env python3
"""Deterministic Claude stream-json peer; never calls a model."""
import json, sys, os, threading
from pathlib import Path
args=sys.argv[1:]
def option(name): return args[args.index(name)+1]
output_lock = threading.Lock()
def send(value):
    with output_lock:
        print(json.dumps(value), flush=True)
settings=json.loads(option('--settings'))
attempt=Path(settings['env']['PIRA_TEAM_ATTEMPT'])
assert Path(option('--system-prompt-file')).read_bytes() == (attempt/'policy.md').read_bytes()+b'\n\n'+(attempt/'phase.md').read_bytes()
assert '--system-prompt' not in args
session=option('--resume') if '--resume' in args else option('--session-id')
state_path=Path.cwd()/('claude-fixture-'+session+'.json')
history=json.loads(state_path.read_text()) if '--resume' in args else []
for line in sys.stdin:
    message=json.loads(line)
    if message['type']=='control_request':
        send({'type':'control_response','response':{'subtype':'success','request_id':message['request_id'],'response':{}}})
        if message['request']['subtype']=='interrupt':
            send({'type':'result','session_id':session,'subtype':'error_during_execution','is_error':True})
    elif message['type']=='user':
        task=message['message']['content']
        if task=='stderr-failure':
            print('original Claude CLI diagnostic: fixture refusal',file=sys.stderr,flush=True)
            raise SystemExit(9)
        history.append(task); state_path.write_text(json.dumps(history))
        send({'type':'system','subtype':'init','session_id':session,'model':option('--model'),'permissionMode':'bypassPermissions','tools':['Bash','Read','Write','Edit','Glob','Grep']})
        send(message)
        send({"type":"system","subtype":"session_state_changed","state":"running","session_id":session})
        if task == 'mixed-service':
            sys.path.insert(0, str(Path(os.environ['TEAM_SUBTEAM_FIXTURE']).parent))
            import subteam_fixture
            def complete_service(status='completed'):
                handoff = json.loads(os.environ['TEAM_CANDIDATE'])['content']
                Path(settings['env']['PIRA_TEAM_HANDOFF']).write_text(handoff)
                send({'type':'result','session_id':session,'subtype':'success','is_error':False,
                      'result':json.dumps({'status':'completed','format':'text'}),
                      'usage':{'input_tokens':2,'cache_read_input_tokens':3,'cache_creation_input_tokens':5,'output_tokens':7}})
                send({'type':'system','subtype':'session_state_changed','state':'idle','session_id':session})
            shell = dict(os.environ)
            shell.update(settings['env'])
            shell['CLAUDE_CODE_SESSION_ID'] = session
            shell.pop('CODEX_THREAD_ID', None)
            service = subteam_fixture.start_with_shell({'thread':session,'cwd':str(Path.cwd())},
                Path(settings['env']['PIRA_TEAM_CONTEXT'])/'unused-native-home', complete_service, shell)
            continue
        if task=='hold': continue
        if task == 'delegate-probe':
            import os, subprocess
            env = dict(os.environ, CLAUDE_CODE_SESSION_ID=session)
            child = subprocess.run([env['TEAM_CLAUDE_BIN'], 'run', '--model', 'test-model',
                '--effort', 'low', '--task', 'child', '--completion-gate', 'fixture child'],
                env=env, text=True, capture_output=True, timeout=15)
            report = {'returncode': child.returncode, 'stderr': child.stderr}
            if child.returncode == 0:
                receipt = json.loads(child.stdout)
                report['manifest'] = json.loads((Path(receipt['run_root']) / 'manifest.json').read_text())
            handoff = json.dumps(report)
        elif task == 'context-probe':
            import os, subprocess
            checks = []
            for case in ['normal', 'missing', 'replaced']:
                env = dict(os.environ, CLAUDE_CODE_SESSION_ID=session)
                if case != 'normal':
                    env['PATH'] = env['TEAM_CLAUDE_CODEX_ONLY'] + os.pathsep + os.defpath
                if case == 'replaced':
                    env['PATH'] = env['TEAM_CLAUDE_REPLACEMENT'] + os.pathsep + env['PATH']
                args = [env['TEAM_CLAUDE_BIN'], 'config', 'set', '--store', env['TEAM_CLAUDE_STORE'],
                        '--main', 'claude-fixture', '--model', 'test-model', '--effort', 'low']
                result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=5)
                checks.append({'case': case, 'operation': 'config', 'returncode': result.returncode,
                               'stderr': result.stderr})
                # An accidentally selected session/harness must disagree with the supplied run.
                env.pop('CLAUDE_CODE_SESSION_ID', None)
                env.update(CODEX_THREAD_ID='parent-session', CODEX_HOME=env['TEAM_CLAUDE_CODEX_HOME'])
                args = [env['TEAM_CLAUDE_BIN'], 'run', '--store', env['TEAM_CLAUDE_STORE'],
                        '--cwd', str(Path.cwd()), '--model', 'test-model', '--effort', 'low',
                        '--task', 'fake-child', '--completion-gate', 'fixture output', '--allow-subteam']
                result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=7)
                checks.append({'case': case, 'operation': 'run', 'returncode': result.returncode,
                               'stderr': result.stderr})
            handoff = json.dumps(checks)
        else:
            handoff = json.dumps(history) if task == 'recall' else 'ANSWER ' + task
        Path(settings['env']['PIRA_TEAM_HANDOFF']).write_text(handoff)
        send({'type':'result','session_id':('wrong' if task=='wrong-session' else session),'subtype':'success','is_error':task=='error-result','result':json.dumps({'status':'completed','format':'text'}),'usage':{'input_tokens':2,'cache_read_input_tokens':3,'cache_creation_input_tokens':5,'output_tokens':7}})
        send({"type":"system","subtype":"session_state_changed","state":"idle","session_id":session})
