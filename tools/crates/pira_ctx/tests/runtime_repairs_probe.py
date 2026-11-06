"""Bounded synthetic process/store fixtures; no installed binaries or live stores."""
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys
import tempfile
import time

BIN, CASE = sys.argv[1:]


def call(env, *args, **kwargs):
    return subprocess.run([BIN, *map(str, args)], env=env, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, timeout=6, start_new_session=True, **kwargs)


def ready(path):
    deadline = time.monotonic() + 3
    while not path.exists():
        assert time.monotonic() < deadline, f"fixture not ready: {path}"
        time.sleep(.01)


def finish(child):
    out, err = child.communicate(timeout=6)
    return child.returncode, out.decode(), err.decode()


def spawn(env, code, *args, mode="capture"):
    return subprocess.Popen([BIN, mode, "--intent", "Synthetic runtime repair regression",
                             "--", sys.executable, "-c", code, *map(str, args)],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)


def manifest(store):
    files = list((store / "live").glob("*.live.json"))
    assert len(files) == 1, files
    return files[0], json.loads(files[0].read_text())


def fixture(root, name):
    root = root / name
    root.mkdir()
    tmp = root / "tmp"
    tmp.mkdir()
    store = root / "store"
    env = os.environ | {"PIRA_CTX_STORE_DIR": str(store), "TMPDIR": str(tmp),
                        "PIRA_CTX_LIVE_CHECKPOINT_MS": "100"}
    return root, store, env


def failed_capture(root, name):
    root, store, env = fixture(root, name)
    marker = root / "ready"
    child = spawn(env, "import pathlib,sys,time; print('valuable output',flush=True); "
                  "pathlib.Path(sys.argv[1]).touch(); time.sleep(.4)", marker)
    ready(marker)
    path, obj = manifest(store)
    (store / "indexes").write_text("injected persistence conflict")
    result = finish(child)
    assert result[0] == 125, result
    path, obj = manifest(store)
    return root, store, env, path, obj, result


def diagnostics(root):
    for mode in ("capture", "watch"):
        _, store, env = fixture(root, mode)
        read_fd, write_fd = os.pipe()
        os.close(read_fd)  # closed before spawn: deterministic diagnostic EPIPE
        args = ["capture", "--intent", "Closed diagnostic regression"] if mode == "capture" else ["watch", "--deadline", "2s"]
        try:
            result = subprocess.run([BIN, *args, "--", sys.executable, "-c",
                                     "import time; time.sleep(.4); print('retained')"],
                                    env=env, stdout=subprocess.PIPE, stderr=write_fd, timeout=5)
        finally:
            os.close(write_fd)
        assert result.returncode == 0, (mode, result)
        if mode == "capture":
            record, = store.glob("*.piractx")
            assert call(env, "raw", record.stem, "--stdout").stdout == b"retained\n"


def escape(root):
    root, store, env = fixture(root, "escape")
    marker, survivor = root / "ready", root / "survivor"
    child = spawn(env, "import os,pathlib,sys,time; os.setpgid(0,os.getpgid(os.getppid())); "
                  "pathlib.Path(sys.argv[1]).touch(); print('ready',flush=True); time.sleep(2); "
                  "pathlib.Path(sys.argv[2]).touch()", marker, survivor)
    ready(marker)
    _, obj = manifest(store)
    start = time.monotonic()
    assert call(env, "cancel", obj["metadata"]["result_id"]).returncode == 0
    result = finish(child)
    assert time.monotonic() - start < 1.5, result
    assert not survivor.exists(), result
    assert result[0] != 0, result
    print("owned escaped child reaped; caller/new group survived")


def batch(root):
    for fast_first in (True, False):
        root2, store, env = fixture(root, str(fast_first))
        store.mkdir()
        (store / "indexes").write_text("injected persistence conflict")
        started, survived = root2 / "started", root2 / "survived"
        slow = [sys.executable, "-c", "import pathlib,sys,time; pathlib.Path(sys.argv[1]).touch(); "
                "time.sleep(2); pathlib.Path(sys.argv[2]).touch()", str(started), str(survived)]
        fast = [sys.executable, "-c", "import time; time.sleep(.3)"]
        commands = [fast, slow] if fast_first else [slow, fast]
        spec = root2 / "batch.json"
        spec.write_text(json.dumps({"concurrency": 2, "commands": [
            {"intent": "Bounded batch regression", "argv": argv} for argv in commands]}))
        start = time.monotonic()
        result = call(env, "batch", spec)
        assert result.returncode == 125, result
        assert started.exists() and not survived.exists(), result
        assert time.monotonic() - start < 1.5, result
        assert len(list((store / "live").glob("*.live.json"))) == 2


def recovery(root):
    _, store, env, _, obj, result = failed_capture(root, "recovery")
    rid = obj["metadata"]["result_id"]
    assert "retained private snapshot" in result[2], result
    assert not list(store.glob("*.piractx"))
    for key in ("stdout_path", "stderr_path"):
        path = Path(obj[key])
        assert path.is_file() and path.stat().st_mode & 0o777 == 0o600
    stats = call(env, "stats", "--brief", rid)
    assert stats.returncode == 0 and b"state=interrupted | exit=unknown" in stats.stdout, stats
    listed = call(env, "list")
    assert b" | capture | interrupted |" in listed.stdout and b" | - |" in listed.stdout, listed
    assert call(env, "raw", rid, "--stdout").stdout == b"valuable output\n"
    state = call(env, "exec", rid, "--code", "print(MSG_STATE); print(MSG_EXIT)")
    assert state.returncode == 0 and b"interrupted" in state.stdout and b"None" in state.stdout, state
    assert call(env, "cancel", rid).returncode == 125
    watched = call(env, "watch", "--capture", rid, "--deadline", "1s")
    assert watched.returncode == 22 and b"interrupted checkpoint" in watched.stdout, watched
    verified = call(env, "verify", rid)
    assert verified.returncode == 125, verified


def reader(root):
    root, store, env = fixture(root, "reader")
    marker, survived = root / "ready", root / "survived"
    def file_limit():
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
        resource.setrlimit(resource.RLIMIT_FSIZE, (8192, 8192))
    # Finite, small output. Limit is inherited only by this disposable wrapper.
    code = ("import os,pathlib,sys,time; print('prefix',flush=True); "
            "pathlib.Path(sys.argv[1]).touch(); time.sleep(.3)\n"
            "try: os.write(1,b'x'*16384)\nexcept BrokenPipeError: pass\n"
            "time.sleep(2); pathlib.Path(sys.argv[2]).touch()")
    start = time.monotonic()
    result = call(env, "capture", "--intent", "Injected spool write failure", "--", sys.executable,
                  "-c", code, marker, survived, preexec_fn=file_limit)
    assert result.returncode == 125 and b"reader failed" in result.stderr, result
    assert time.monotonic() - start < 1.5 and not survived.exists(), result
    _, obj = manifest(store)
    assert call(env, "raw", obj["metadata"]["result_id"], "--stdout").stdout == b"prefix\n"
    assert Path(obj["stdout_path"]).stat().st_size == 8192


def tmpdir(root):
    root, store, env = fixture(root, "tmpdir")
    marker = root / "ready"
    child = spawn(env, "import pathlib,sys,time; print('retained',flush=True); "
                  "pathlib.Path(sys.argv[1]).touch(); time.sleep(2)", marker)
    ready(marker)
    _, obj = manifest(store)
    rid = obj["metadata"]["result_id"]
    other = root / "other"
    other.mkdir()
    env = env | {"TMPDIR": str(other)}
    assert call(env, "stats", rid).returncode == 0
    assert b"running" in call(env, "list").stdout
    assert call(env, "cancel", rid).returncode == 0
    assert finish(child)[0] != 0


def legacy(root):
    root, _, env, path, obj, _ = failed_capture(root, "legacy")
    legacy_dir = root / "legacy"
    legacy_dir.mkdir(mode=0o700)
    for key in ("stdout_path", "stderr_path"):
        target = legacy_dir / (".pira_ctx-spool-legacy-" + key)
        Path(obj[key]).rename(target)
        obj[key] = str(target)
        obj.pop(key + "_native", None)
    path.write_text(json.dumps(obj))
    rid = obj["metadata"]["result_id"]
    assert call(env, "raw", rid, "--stdout").stdout == b"valuable output\n"
    stdout = Path(obj["stdout_path"])
    stdout.chmod(0o644)
    denied = call(env, "stats", rid)
    assert denied.returncode == 125 and b"privately owned" in denied.stderr, denied
    stdout.chmod(0o600)
    alias = stdout.with_name(".pira_ctx-spool-alias")
    alias.symlink_to(stdout)
    obj["stdout_path"] = str(alias)
    path.write_text(json.dumps(obj))
    denied = call(env, "stats", rid)
    assert denied.returncode == 125 and b"symlink" in denied.stderr, denied
    obj["stdout_path"] = str(stdout)
    path.write_text(json.dumps(obj))
    alias.unlink()
    os.link(stdout, alias)
    denied = call(env, "stats", rid)
    assert denied.returncode == 125 and b"hard-linked" in denied.stderr, denied
    alias.unlink()
    directory_alias = root / "directory-alias"
    directory_alias.symlink_to(legacy_dir, target_is_directory=True)
    obj["stdout_path"] = str(directory_alias / stdout.name)
    path.write_text(json.dumps(obj))
    denied = call(env, "stats", rid)
    assert denied.returncode == 125 and b"symlink" in denied.stderr, denied


def watch_stop(root):
    root, store, env = fixture(root, "watch_stop")
    marker = root / "analyzer_ready"
    analyzer = "import os,pathlib,time; os.close(0); pathlib.Path(" + repr(str(marker)) + ").touch(); time.sleep(2)"
    code = "import os; os.write(1,bytes(16000)); os.write(2,bytes(16000)); raise SystemExit(75)"
    child = subprocess.Popen([BIN, "watch", "--deadline", "5s", "--attempt-timeout", "3s",
                              "--analyzer-code", analyzer, "--", sys.executable, "-c", code],
                             env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    ready(marker)
    state_path, = (store / "watch/state").glob("*.json")
    wid = json.loads(state_path.read_text())["id"]
    result = call(env, "watch", wid, "--stop")
    owner = finish(child)
    assert result.returncode == 0 and b"stopped" in result.stdout, result
    assert owner[0] == 23, owner
    state = json.loads(state_path.read_text())
    assert state["monitor"] == "stopped", state


def replay(root):
    for mode in ("auto", "exact"):
        root2, store, env = fixture(root, mode)
        # This case targets replay before checkpoint retention; call() already bounds execution to 6s.
        env = env | {"PIRA_CTX_STORE_DIR": "store", "PIRA_CTX_LIVE_CHECKPOINT_MS": "60000"}
        result = call(env, mode, "--intent", "Preserve short replay", "--", sys.executable,
                      "-c", "print('small exact output')", cwd=root2)
        assert result.returncode == 0 and result.stdout == b"small exact output\n", result
        assert not list(store.glob("*.piractx"))
        assert not list((store / "live").glob("*.live.json"))
        assert not list((store / "live/spools").glob("*"))


def watch_escape(root):
    for kind in ("probe", "analyzer"):
        root2, _, env = fixture(root, kind)
        survived = root2 / "survived"
        code = ("import os,pathlib,time; os.setpgid(0,os.getpgid(os.getppid())); "
                "time.sleep(3); pathlib.Path(" + repr(str(survived)) + ").touch()")
        args = ["watch", "--deadline", "3s", "--attempt-timeout", "1s"]
        if kind == "analyzer":
            args += ["--analyzer-code", code]
            probe = "raise SystemExit(75)"
        else:
            probe = code
        start = time.monotonic()
        result = call(env, *args, "--", sys.executable, "-c", probe)
        assert result.returncode == 22 and b"attempt timed out" in result.stdout, result
        assert time.monotonic() - start < 2.5 and not survived.exists(), result


def stop_race(root):
    import fcntl
    _, store, env = fixture(root, "stop-race")
    paused = call(env, "watch", "--deadline", "2s", "--review-after", "1s",
                  "--", sys.executable, "-c", "raise SystemExit(75)")
    assert paused.returncode == 10, paused
    path, = (store / "watch/state").glob("*.json")
    obj = json.loads(path.read_text())
    wid = obj["id"]
    with (store / "watch/owners" / (wid + ".lock")).open("a+b") as lease:
        fcntl.flock(lease, fcntl.LOCK_EX)
        obj["monitor"] = "active"
        path.write_text(json.dumps(obj))
        child = subprocess.Popen([BIN, "watch", wid, "--stop"], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        control = store / "watch/control" / (wid + ".json")
        deadline = time.monotonic() + 3
        while not json.loads(control.read_text())["stop_requested"]:
            assert time.monotonic() < deadline, "stop was not queued"
            time.sleep(.005)
        # Simulate the owner persisting a terminal error immediately before releasing its lease.
        obj["monitor"] = "failed"
        temporary = path.with_suffix(".pending")
        temporary.write_text(json.dumps(obj))
        temporary.replace(path)
    result = finish(child)
    assert result[0] == 125 and "stopped;" not in result[1], result
    assert "Failed" in result[2] and json.loads(path.read_text())["monitor"] == "failed", result


def batch_success(root):
    root, store, env = fixture(root, "batch-success")
    spec = root / "batch.json"
    spec.write_text(json.dumps({"concurrency": 2, "commands": [
        {"intent": "First successful child", "argv": [sys.executable, "-c", "import time; time.sleep(.2); print('first')"]},
        {"intent": "Second nonzero child", "argv": [sys.executable, "-c", "print('second'); raise SystemExit(7)"]}]}))
    result = call(env, "batch", spec)
    assert result.returncode == 7, result
    lines = result.stdout.decode().splitlines()
    assert lines[0] == "index | exit | duration_ms | result | intent", result
    assert lines[1].startswith("1 | 0 |") and lines[2].startswith("2 | 7 |"), result
    records = list(store.glob("*.piractx"))
    assert len(records) == 2 and not list((store / "live").glob("*.live.json")), result
    assert {call(env, "raw", record.stem, "--stdout").stdout for record in records} == {b"first\n", b"second\n"}


with tempfile.TemporaryDirectory(prefix="ctx-runtime-repair-") as directory:
    globals()[CASE](Path(directory))
print(CASE + ": passed")
