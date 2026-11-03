#!/usr/bin/env python3
"""Deterministic Claude stream-json peer; never calls a model."""
import json, sys
from pathlib import Path
args=sys.argv[1:]
def option(name): return args[args.index(name)+1]
def send(value): print(json.dumps(value),flush=True)
settings=json.loads(option('--settings'))
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
        if task=='hold': continue
        if task == 'ancestry-probe':
            import os, subprocess
            checks = []
            for case in ['normal', 'missing', 'replaced']:
                env = dict(os.environ, CLAUDE_CODE_SESSION_ID=session)
                for key in ['PIRA_TEAM_CHILD', 'PIRA_TEAM_CONTEXT']:
                    env.pop(key, None)
                if case != 'normal':
                    env['PATH'] = env['TEAM_CLAUDE_CODEX_ONLY'] + os.pathsep + os.defpath
                if case == 'replaced':
                    env['PATH'] = env['TEAM_CLAUDE_REPLACEMENT'] + os.pathsep + env['PATH']
                args = [env['TEAM_CLAUDE_BIN'], 'config', 'set', '--store', env['TEAM_CLAUDE_STORE'],
                        '--main', 'claude-fixture', '--model', 'test-model', '--effort', 'low']
                result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=5)
                checks.append({'case': case, 'operation': 'config', 'returncode': result.returncode,
                               'stderr': result.stderr})
                # Even another matching caller transcript must not hide actual worker ancestry.
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
