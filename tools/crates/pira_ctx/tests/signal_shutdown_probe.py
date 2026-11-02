"""Only finite synthetic children in a private temporary workspace/store."""
import json
import os
from pathlib import Path
import pty
import fcntl
import select
import signal
import subprocess
import sys
import tempfile
import termios
import time

BIN, CASE = sys.argv[1:]
# Both generations ignore terminal signals: only wrapper containment can stop them.
# The finite lifetime also bounds cleanup if a pre-fix wrapper dies immediately.
CODE = """
import os, pathlib, signal, sys, time
signal.signal(signal.SIGINT, signal.SIG_IGN)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pid = os.fork()
if pid == 0:
    print('descendant output', flush=True)
    pathlib.Path(sys.argv[1] + '.descendant').write_text(str(os.getpid()))
else:
    print('partial stdout', flush=True)
    print('partial stderr', file=sys.stderr, flush=True)
    pathlib.Path(sys.argv[1]).write_text(str(os.getpid()))
time.sleep(3)
"""


def run(root, env, *args):
    return subprocess.run([BIN, *map(str, args)], cwd=root, env=env,
                          capture_output=True, timeout=6)


def wait_ready(path):
    deadline = time.monotonic() + 3
    while not path.exists() or not path.read_text().strip():
        assert time.monotonic() < deadline, path
        time.sleep(.01)


def check_stopped(pid):
    # Read-only observation, never signal a PID recovered from a file.
    status = subprocess.run(['ps', '-o', 'stat=', '-p', str(pid)], capture_output=True)
    assert not status.stdout.strip() or status.stdout.strip().startswith(b'Z'), status.stdout


def attempt(root, mode='capture', action='term', fail=False):
    root.mkdir()
    store = root / 'store'
    env = os.environ | {'PIRA_CTX_STORE_DIR': str(store), 'PIRA_CTX_LIVE_CHECKPOINT_MS': '30000'}
    marker = root / 'ready'
    argv = [sys.executable, '-c', CODE, str(marker)]
    if mode == 'batch':
        commands = [{'intent': 'Synthetic signal fixture', 'argv': argv},
                    {'intent': 'Synthetic signal peer', 'argv': [*argv[:-1], str(root / 'peer')]},
                    {'intent': 'Must not start after signal', 'argv': [sys.executable, '-c',
                     "import pathlib,sys; pathlib.Path(sys.argv[1]).touch()", str(root / 'pending')]}]
        spec = root / 'batch.json'
        spec.write_text(json.dumps({'concurrency': 2, 'commands': commands}))
        args = ['batch', str(spec)]
    else:
        args = [mode, '--intent', 'Synthetic signal fixture', '--', *argv]
    master = slave = None
    options = {'start_new_session': True}
    if action == 'tty':
        master, slave = pty.openpty()
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        options = {'stdin': slave, 'preexec_fn': controlling_terminal}
    child = subprocess.Popen([BIN, *args], cwd=root, env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, **options)
    try:
        markers = [marker, Path(str(marker) + '.descendant')]
        if mode == 'batch':
            markers += [root / 'peer', root / 'peer.descendant']
        for path in markers:
            wait_ready(path)
        if fail:
            (store / 'indexes').write_text('injected publication conflict')
        start = time.monotonic()
        if action == 'cancel':
            manifest, = (store / 'live').glob('*.live.json')
            rid = json.loads(manifest.read_text())['metadata']['result_id']
            result = run(root, env, 'cancel', rid)
            assert result.returncode == 0, result
        elif action == 'group':
            os.killpg(child.pid, signal.SIGINT)  # own fresh session; wrapper has not been reaped
        elif action == 'tty':
            os.write(master, b'\x03')
            # Darwin session teardown waits for queued terminal output. Consume
            # the echoed control character instead of deadlocking the test's PTY.
            readable, _, _ = select.select([master], [], [], 1)
            assert readable and b'^C' in os.read(master, 128)
        else:
            child.send_signal(signal.SIGINT if action == 'int' else signal.SIGTERM)
        try:
            out, err = child.communicate(timeout=6)
        except subprocess.TimeoutExpired:
            state = subprocess.run(['ps', '-o', 'pid,ppid,pgid,tpgid,stat,command', '-p', str(child.pid)], capture_output=True)
            print(state.stdout.decode(), file=sys.stderr, flush=True)
            raise
        assert child.returncode == (125 if fail else 137), (child.returncode, out, err)
        assert time.monotonic() - start < 2, (out, err)
        for path in markers:
            check_stopped(int(path.read_text()))
        if fail:
            manifest, = (store / 'live').glob('*.live.json')
            rid = json.loads(manifest.read_text())['metadata']['result_id']
            stats = run(root, env, 'stats', '--brief', rid)
            assert b'state=interrupted | exit=unknown' in stats.stdout, stats
        else:
            records = list(store.glob('*.piractx'))
            assert len(records) == (2 if mode == 'batch' else 1), (out, err, records)
            assert not list((store / 'live').glob('*.live.json'))
            for record in records:
                rid = record.stem
                stats = run(root, env, 'stats', '--brief', rid)
                assert b'state=cancelled | exit=137' in stats.stdout, stats
                assert run(root, env, 'verify', rid).returncode == 0
                assert run(root, env, 'cancel', rid).returncode == 125
            rid = records[0].stem
        assert b'partial stdout\n' in run(root, env, 'raw', rid, '--stdout').stdout
        assert run(root, env, 'raw', rid, '--stderr').stdout == b'partial stderr\n'
        assert not (root / 'pending').exists()
    finally:
        for fd in (master, slave):
            if fd is not None:
                os.close(fd)
        # Direct wrapper handle only. Finite fixture descendants expire naturally
        # on baseline failure; never recover numeric PIDs for cleanup signals.
        if child.poll() is None:
            child.terminate()
        child.communicate(timeout=6)


def completion(root):
    env = os.environ | {'PIRA_CTX_STORE_DIR': str(root / 'store')}
    marker = root / 'descendant'
    code = """
import os, pathlib, sys, time
pid = os.fork()
if pid == 0:
    pathlib.Path(sys.argv[1]).write_text(str(os.getpid()))
    time.sleep(3)
else:
    marker = pathlib.Path(sys.argv[1])
    deadline = time.monotonic() + 1
    while not marker.exists() or not marker.read_text().strip():
        assert time.monotonic() < deadline, 'descendant not ready'
        time.sleep(.01)
    print('completed output', flush=True)
    sys.exit(23)
"""
    result = run(root, env, 'capture', '--intent', 'Preserve direct child status', '--',
                 sys.executable, '-c', code, marker)
    assert result.returncode == 23, result
    check_stopped(int(marker.read_text()))
    record, = (root / 'store').glob('*.piractx')
    stats = run(root, env, 'stats', '--brief', record.stem)
    assert b'state=complete | exit=23' in stats.stdout, stats
    assert run(root, env, 'raw', record.stem, '--stdout').stdout == b'completed output\n'


def missing_handle(root):
    env = os.environ | {'PIRA_CTX_STORE_DIR': str(root / 'store'), 'PIRA_CTX_THREAD_ID': 'signal-test'}
    result = run(root, env, 'capture', '--intent', 'Handle fixture', '--', sys.executable, '-c', "print('ok')")
    assert result.returncode == 0, result
    # Ensure the reservation directories exist, so lookup fails at the binding,
    # not the already-actionable missing-directory diagnostic.
    assert list((root / 'store' / 'short-ids').rglob('*'))
    result = run(root, env, 'cancel', '@missing123456')
    assert result.returncode == 125, result
    assert b'not found in this workspace/session' in result.stderr, result
    assert b'original workspace/session' in result.stderr, result
    bindings = [p for p in (root / 'store' / 'short-ids').rglob('*') if p.is_file()]
    assert len(bindings) == 1, bindings
    handle = '@' + bindings[0].name
    other = root / 'other-workspace'
    other.mkdir()
    result = run(other, env, 'capture', '--intent', 'Other workspace fixture', '--', sys.executable, '-c', "print('other')")
    assert result.returncode == 0, result
    result = run(other, env, 'stats', handle)
    assert result.returncode == 125 and b'not found in this workspace/session' in result.stderr, result
    assert run(root, env, 'stats', handle).returncode == 0


with tempfile.TemporaryDirectory(prefix='pira-ctx-signals-') as tmp:
    root = Path(tmp)
    if CASE == 'modes':
        for mode in ('auto', 'exact', 'check'):
            attempt(root / mode, mode=mode)
    elif CASE == 'handle':
        missing_handle(root)
    elif CASE == 'completion':
        completion(root)
    else:
        attempt(root / CASE, mode='batch' if CASE == 'batch' else 'capture',
                action=CASE, fail=CASE == 'failure')
