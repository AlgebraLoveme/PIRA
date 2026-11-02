"""Finite private fixtures for review 1793477007354699000-42412-0."""
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

BIN, CASE = sys.argv[1:]
SIGNALS = (signal.SIGINT, signal.SIGTERM)


def ready(predicate):
    deadline = time.monotonic() + 3
    while not predicate():
        assert time.monotonic() < deadline, 'fixture not ready'
        time.sleep(.01)


def call(root, env, *args):
    return subprocess.run([BIN, *map(str, args)], cwd=root, env=env,
                          capture_output=True, timeout=6)


def setup(base, name):
    root = base / name
    root.mkdir()
    env = os.environ | {'PIRA_CTX_STORE_DIR': str(root / 'store'), 'TMPDIR': str(root),
                        'PIRA_CTX_LIVE_CHECKPOINT_MS': '30000'}
    return root, env


def stopped(pid):
    result = subprocess.run(['ps', '-o', 'stat=', '-p', str(pid)], capture_output=True)
    assert not result.stdout.strip() or result.stdout.strip().startswith(b'Z'), result.stdout


def interrupt(child, sig):
    start = time.monotonic()
    child.send_signal(sig)  # only the fresh, unreaped Popen wrapper
    child.wait(timeout=2)   # deliberately do not release stdin or drain output
    assert time.monotonic() - start < 2


def close(child):
    # Timeout cleanup releases only this harness's pipes; fixtures expire in 2s.
    if child.stdin:
        child.stdin.close()
        child.stdin = None
    child.communicate(timeout=6)


def seed(root, env):
    result = call(root, env, 'capture', '--intent', 'Private lifecycle seed', '--', sys.executable, '-c', "print('seed')")
    assert result.returncode == 0, result
    record, = (root / 'store').glob('*.piractx')
    return record.stem


def watch(base, kind):
    for sig in SIGNALS:
        root, env = setup(base, str(sig))
        marker = root / 'ready'
        descendant = root / 'descendant'
        code = f"""
import os, pathlib, signal, time
signal.signal(signal.SIGINT, signal.SIG_IGN)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pid = os.fork()
if pid == 0:
    pathlib.Path({str(descendant)!r}).write_text(str(os.getpid()))
else:
    print('partial evidence', flush=True)
    pathlib.Path({str(marker)!r}).write_text(str(os.getpid()))
time.sleep(2)
"""
        args = ['watch', '--deadline', '5s', '--attempt-timeout', '4s']
        if kind == 'analyzer':
            args += ['--analyzer-code', code]
            code = 'raise SystemExit(75)'
        if kind == 'idle':
            code = 'raise SystemExit(75)'
        child = subprocess.Popen([BIN, *args, '--', sys.executable, '-c', code],
                                 cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        try:
            def state_ready():
                files = list((root / 'store/watch/state').glob('*.json'))
                if not files:
                    return False
                obj = json.loads(files[0].read_text())
                if kind == 'idle':
                    return obj['attempts'] == 1 and obj['attempt'] == 'idle'
                return marker.exists() and marker.read_text() and descendant.exists() and descendant.read_text()
            ready(state_ready)
            interrupt(child, sig)
            assert child.returncode == 23, child.returncode
            path, = (root / 'store/watch/state').glob('*.json')
            state = json.loads(path.read_text())
            assert state['monitor'] == 'stopped' and state['attempt'] == 'idle', state
            latest = call(root, env, 'watch', state['id'], '--latest')
            assert b'Monitor: Stopped' in latest.stdout and b'Attempt: Idle' in latest.stdout, latest
            if kind != 'idle':
                stopped(int(marker.read_text()))
                stopped(int(descendant.read_text()))
        finally:
            close(child)


def stdin_exec(base):
    for sig in SIGNALS:
        root, env = setup(base, str(sig))
        rid = seed(root, env)
        marker = root / 'analysis-launched'
        child = subprocess.Popen([BIN, 'exec', rid, '--file', '-'], cwd=root, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        try:
            ready(lambda: list(root.glob('.pira_ctx-exec-*/captures.json')))
            # Supply partial code but no EOF: no analysis may run after interruption.
            child.stdin.write(f"from pathlib import Path; Path({str(marker)!r}).touch()\n".encode())
            child.stdin.flush()
            interrupt(child, sig)
            assert child.returncode == 128 + sig, child.returncode
            assert not marker.exists()
            assert not list(root.glob('.pira_ctx-exec-*'))
            assert len(list((root / 'store').glob('*.piractx'))) == 1
            assert not list((root / 'store/live').glob('*.live.json'))
        finally:
            close(child)


def preflight(base):
    for sig in SIGNALS:
        root, env = setup(base, str(sig))
        rid = seed(root, env)
        marker, descendant, launched = root / 'ready', root / 'descendant', root / 'launched'
        shim = root / 'python-shim'
        shim.write_text(f'''#!{sys.executable}
import os, pathlib, sys, time
if sys.argv[2].startswith('import sys; raise SystemExit'):
    pid = os.fork()
    if pid == 0:
        pathlib.Path({str(descendant)!r}).write_text(str(os.getpid()))
    else:
        pathlib.Path({str(marker)!r}).write_text(str(os.getpid()))
    time.sleep(2)
    raise SystemExit(0)
pathlib.Path({str(launched)!r}).touch()
os.execv(sys.executable, [sys.executable, *sys.argv[1:]])
''')
        shim.chmod(0o700)
        child = subprocess.Popen([BIN, 'exec', rid, '--code', "print('analysis')", '--python', str(shim)],
                                 cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        try:
            ready(lambda: marker.exists() and marker.read_text() and descendant.exists() and descendant.read_text())
            interrupt(child, sig)
            assert child.returncode == 128 + sig, child.returncode
            stopped(int(marker.read_text()))
            stopped(int(descendant.read_text()))
            assert not launched.exists()
            assert not list((root / 'store/live').glob('*.live.json'))
        finally:
            close(child)


def replay(base, short=False):
    cases = [('auto', 'stdout'), ('exec', 'stdout')] if short else [('exact', s) for s in ('stdout', 'stderr')]
    for mode, stream in cases:
        for sig in SIGNALS:
            root, env = setup(base, f'{mode}-{stream}-{sig}')
            count = 100 if short else 500000
            # Fill our own output pipe so even short automatic replay must wait.
            read_fd, write_fd = os.pipe()
            flags = fcntl.fcntl(write_fd, fcntl.F_GETFL)
            fcntl.fcntl(write_fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)
            try:
                while True:
                    os.write(write_fd, b'p' * 4096)
            except BlockingIOError:
                pass
            fcntl.fcntl(write_fd, fcntl.F_SETFL, flags)
            if mode == 'exec':
                rid = seed(root, env)
                args = ['exec', rid, '--code', f"import sys; sys.stdout.write('x'*{count})"]
            else:
                args = [mode, '--intent', 'Blocked replay fixture', '--', sys.executable, '-c', f"import sys; sys.{stream}.write('x'*{count})"]
            outputs = {stream: write_fd, 'stderr' if stream == 'stdout' else 'stdout': subprocess.PIPE}
            child = subprocess.Popen([BIN, *args], cwd=root, env=env, start_new_session=True, **outputs)
            os.close(write_fd)
            try:
                def captured():
                    paths = list((root / 'store/live').glob('*.live.json'))
                    if not paths:
                        return False
                    obj = json.loads(paths[0].read_text())
                    return obj['metadata'][stream + '_bytes'] == count
                ready(captured)
                path, = (root / 'store/live').glob('*.live.json')
                rid = json.loads(path.read_text())['metadata']['result_id']
                interrupt(child, sig)
                assert child.returncode == 128 + sig, child.returncode
                stats = call(root, env, 'stats', '--brief', rid)
                assert b'state=interrupted | exit=unknown' in stats.stdout, stats
                recovered = call(root, env, 'raw', rid, '--' + stream)
                assert recovered.returncode == 0 and recovered.stdout == b'x' * count, recovered
                assert not list(root.glob('.pira_ctx-exec-*'))
            finally:
                os.close(read_fd)
                close(child)


def normal(base):
    for mode in ('exact', 'auto', 'check', 'capture'):
        root, env = setup(base, mode)
        result = call(root, env, mode, '--intent', 'Normal completion diagnostics', '--', sys.executable,
                      '-c', "import sys; print('ordinary stdout'); print('ordinary stderr',file=sys.stderr)")
        assert result.returncode == 0, result
        if mode in ('exact', 'auto'):
            assert result.stdout == b'ordinary stdout\n' and result.stderr == b'ordinary stderr\n', result
        else:
            assert all(line.startswith(b'LIVE | result=') for line in result.stderr.splitlines()), result
            record, = (root / 'store').glob('*.piractx')
            assert call(root, env, 'raw', record.stem, '--stderr').stdout == b'ordinary stderr\n'
    for case, status in (('missing', 127), ('denied', 126)):
        root, env = setup(base, case)
        executable = root / 'program'
        if case == 'denied':
            executable.write_text('not executable')
        result = call(root, env, 'capture', '--intent', 'Preserve spawn failure', '--', executable)
        assert result.returncode == status, result
        assert not list((root / 'store/live').glob('*.live.json'))
        assert not list((root / 'store/live/spools').glob('.pira_ctx-spool-*'))


with tempfile.TemporaryDirectory(prefix='ctx-lifecycle-interrupt-') as tmp:
    base = Path(tmp)
    if CASE in ('probe', 'analyzer', 'idle'):
        watch(base, CASE)
    elif CASE == 'stdin':
        stdin_exec(base)
    elif CASE == 'preflight':
        preflight(base)
    elif CASE == 'short_replay':
        replay(base, short=True)
    elif CASE == 'replay':
        replay(base)
    else:
        normal(base)
