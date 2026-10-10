"""Small isolated regressions for independent pira_ctx repairs (no user stores)."""
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import time

BINARY, CASE = sys.argv[1:]


def run(store, operation, *args, env=None):
    result = subprocess.run(
        [BINARY, operation, "--store-dir", str(store), *map(str, args)],
        env=env or ENV, capture_output=True, timeout=12,
    )
    return result


def ok(result):
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    return result.stdout.decode()


def capture(store, text="hello"):
    ok(run(store, "capture", "--intent", "small repair fixture", "--", sys.executable,
           "-c", "print(" + repr(text) + ")"))
    return max(store.glob("*.piractx"), key=lambda p: metadata(p)["start_unix_ms"])


def metadata(path):
    data = path.read_bytes()
    length = struct.unpack_from("<Q", data, 16)[0]
    return json.loads(data[192:192 + length])


def rewrite_metadata(path, **changes):
    data = path.read_bytes()
    length = struct.unpack_from("<Q", data, 16)[0]
    record = json.loads(data[192:192 + length])
    record.update(changes)
    encoded = json.dumps(record, separators=(",", ":")).encode()
    header = bytearray(data[:192])
    struct.pack_into("<Q", header, 16, len(encoded))
    header[64:96] = hashlib.sha256(encoded).digest()
    path.write_bytes(header + encoded + data[192 + length:])


def cache_rows(store):
    path = next((store / "indexes").glob("*.jsonl"))
    return path, [json.loads(row) for row in path.read_text().splitlines()]


def search():
    store = ROOT / "search"
    producer = "for i in range(100): print(('A_UNIQUE B_MANY' if i==50 else 'B_MANY')+' '+str(i)+' '+'x'*1500)"
    ok(run(store, "capture", "--intent", "unequal query queues", "--", sys.executable, "-c", producer))
    path = next(store.glob("*.piractx"))
    for context in ["0", "20"]:
        result = run(store, "search", path, "-e", "A_UNIQUE", "-e", "B_MANY", "--limit", "100", "--context", context)
        text = ok(result)
        assert sum(row.startswith("q2 L") for row in text.splitlines()) == 100, text[:500]
        assert "omitted=61" not in text and len(result.stdout) <= 65536
    text = ok(run(store, "search", path, "B_MANY", "--limit", "2", "--context", "1"))
    assert "shown=2 omitted=98" in text and "L" in text


def check():
    for lines, status in [(1000, 0), (1001, 0), (1001, 7)]:
        store = ROOT / f"check-{lines}-{status}"
        env = {**ENV, "PIRA_CTX_MAX_INDEXED_LINES": "1000"}
        result = run(store, "check", "--intent", "index cap boundary", "--", sys.executable, "-c",
                     f"import sys;print('line\\n'*{lines},end='');sys.exit({status})", env=env)
        assert result.returncode == status, result.stderr
        assert (b"index_truncated=1" in result.stdout) == (lines > 1000)
        path = next(store.glob("*.piractx"))
        raw = run(store, "raw", path, "--stdout")
        assert raw.returncode == 0 and len(raw.stdout.splitlines()) == lines
        assert ("truncated=true" in ok(run(store, "stats", path))) == (lines > 1000)
        if status == 0:
            assert result.stdout.count(b"\n") == 1 and result.stdout.startswith(b"PASS")


def history():
    store = ROOT / "history"
    ok(run(store, "exact", "--intent", "intact history", "--", sys.executable, "-c", "print('ok')"))
    assert "history_hits=1" in ok(run(store, "history"))
    records = next((store / ".events").glob("*/*/records"))
    held = records.with_name("held")
    index = records.parent.parent / ".retention.piraidx"
    records.rename(held)
    records.symlink_to(held, target_is_directory=True)
    index.unlink()
    broken = run(store, "history")
    assert broken.returncode == 125 and b"symlinked" in broken.stderr
    assert not index.exists()
    records.unlink()
    held.rename(records)
    assert "history_hits=1" in ok(run(store, "history"))
    # The record/retention writer must not hide another rejected scope during recovery.
    foreign = records.parent.parent / ("a" * 64)
    foreign.mkdir()
    (foreign / "records").symlink_to(records, target_is_directory=True)
    index.unlink()
    result = run(store, "exact", "--intent", "failed recovery writer", "--", sys.executable, "-c", "print('ok')")
    assert result.returncode == 0 and b"symlinked" in result.stderr
    assert not index.exists()
    (foreign / "records").unlink()
    foreign.rmdir()
    assert "history_hits=1" in ok(run(store, "history"))


def index():
    store = ROOT / "index"
    path = capture(store)
    sentinel = ROOT / "sentinel.jsonl"
    sentinel.write_text("SENTINEL\n")
    rewrite_metadata(path, workspace_hash="../../sentinel")
    (store / "indexes" / ".complete-v2").unlink()
    result = run(store, "capture", "--intent", "rebuild crafted index", "--", sys.executable, "-c", "print('normal')")
    assert result.returncode == 0 and b"invalid workspace identity" in result.stderr
    assert sentinel.read_text() == "SENTINEL\n"
    assert not (store / "indexes" / ".complete-v2").exists()
    # The source remains readable; confinement need not change container compatibility.
    ok(run(store, "stats", path))


def prune():
    store = ROOT / "prune"
    older = capture(store, "older")
    newer = capture(store, "newer")
    assert metadata(older)["start_unix_ms"] < metadata(newer)["start_unix_ms"]
    cache, rows = cache_rows(store)
    for row in rows:
        row["start_ms"] = 0
    cache.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
    assert "files=0 " in ok(run(store, "prune", "--max-age-days", "1"))
    assert older.exists() and newer.exists()
    cache, rows = cache_rows(store)
    for row in rows:
        row["start_ms"] = 0 if row["filename"] == newer.name else 2**80
    cache.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
    budget = newer.stat().st_size
    assert "files=1 " in ok(run(store, "prune", "--max-store-bytes", budget))
    assert not older.exists() and newer.exists()
    rewrite_metadata(newer, start_unix_ms=0)
    assert "files=1 " in ok(run(store, "prune", "--max-age-days", "1"))
    assert not newer.exists()


def exec_case():
    store = ROOT / "exec"
    path = capture(store, "exact α bytes")
    native = ROOT / "temp-�-é"
    native.mkdir()
    environments = [{**ENV, "TMPDIR": str(native), "TMP": str(native), "TEMP": str(native)}]
    if os.name == "posix" and sys.platform != "darwin":
        bad = os.fsencode(ROOT) + b"/native-\xff"
        os.mkdir(bad)
        environments.append({**ENV, "TMPDIR": os.fsdecode(bad), "TMP": os.fsdecode(bad), "TEMP": os.fsdecode(bad)})
    code = ("import os,sys;assert isinstance(MSG_PATH,str);"
            "assert os.path.samefile(os.path.dirname(os.path.dirname(MSG_PATH)),os.environ['TMPDIR']);"
            "assert open(MSG_PATH,'rb').read()==MSG_BYTES;"
            "assert open(MSG_STDOUT_PATH,'rb').read()==MSG_BYTES;"
            "assert open(MSG_STDERR_PATH,'rb').read()==b'';"
            "sys.stdout.buffer.write(MSG.strip().encode('utf-8')+b'\\n')")
    for env in environments:
        result = run(store, "exec", path, "--code", code, env=env)
        assert ok(result) == "exact α bytes\n", (result.returncode, result.stdout, result.stderr)
        assert not list(Path(env["TMPDIR"]).glob(".pira_ctx-exec-*"))
    # Multiple labels keep the same public eager snapshot/path API.
    code = ("import sys;assert CAPTURE_NAMES==['first','second'];"
            "assert CAPTURES['first']['bytes']==CAPTURES['second']['bytes'];"
            "sys.stdout.buffer.write(b'snapshots\\n')")
    result = run(store, "exec", "--input", "first=" + str(path), "--input", "second=" + str(path), "--code", code)
    assert ok(result) == "snapshots\n", (result.returncode, result.stdout, result.stderr)


def attention():
    code = "import sys;sys.stdout.buffer.write(b'\\x1b[8;20;80t');sys.stdout.flush();sys.exit(STATUS)"
    for status, expected in [(75, 10), (0, 0), (2, 20)]:
        result = run(ROOT / f"attention-{status}", "watch", "--deadline", "2s", "--", sys.executable, "-c", code.replace("STATUS", str(status)))
        assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
        assert b"render reliable: false" in result.stdout
        assert (b"Attention:" in result.stdout) == (status == 75)
    store = ROOT / "attention-cache"
    result = run(store, "watch", "--deadline", "2s", "--sample-every", "1s", "--attention", "cache", "--", sys.executable, "-c", code.replace("STATUS", "75"))
    assert result.returncode == 21 and b"Attention:" in result.stdout
    state = json.loads(next((store / "watch/state").glob("*.json")).read_text())
    assert state["attention_sequence"] == 1 and state["attempts"] >= 2


def controls():
    import fcntl
    store = ROOT / "controls"
    path = capture(store)
    result = run(store, "watch", "--capture", path, "--deadline", "60s")
    assert result.returncode == 0
    state_path = next((store / "watch/state").glob("*.json"))
    state = json.loads(state_path.read_text())
    # A completed capture-only watch is enough to queue controls;
    # no process launch or forged config is needed for the concurrent update trigger.
    wid = state["id"]
    control_path = store / "watch/control" / (wid + ".json")
    updates = []
    with open(store / "watch/control" / (wid + ".control-lock"), "a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            for args in [["--sample-every", "100ms"], ["--set-analyzer-code", "print('{}')"]]:
                updates.append(subprocess.Popen([BINARY, "watch", "--store-dir", str(store), wid, *args], env=ENV, stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            time.sleep(.3)
            assert all(p.poll() is None for p in updates)
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)
    statuses = []
    for process in updates:
        out, err = process.communicate(timeout=5)
        statuses.append(process.returncode)
        if process.returncode == 125:
            assert b"at least 1s" in err
    assert sorted(statuses) == [0, 125], statuses
    control = json.loads(control_path.read_text())
    if control["analyzer"] is not None:
        assert control.get("configuration") is None or control["configuration"]["sample_every_ms"] >= 1000
    else:
        assert control["configuration"]["sample_every_ms"] == 100
    # Older invalid merged controls must also be refused on resume before launch.
    marker = ROOT / "must-not-launch"
    state.update(monitor="paused", source_kind="probe", job="unknown", sample_every_ms=100,
                 source=[sys.executable, "-c", f"from pathlib import Path;Path({str(marker)!r}).write_text('launched')"])
    state_path.write_text(json.dumps(state))
    control_path.write_text(json.dumps({"stop_requested": False, "analyzer_revision": 0,
                                      "analyzer": None, "clear_analyzer": False}))
    result = run(store, "watch", wid)
    assert result.returncode == 125 and b"at least 1s" in result.stderr
    assert not marker.exists()



def checkpoints():
    # The ordinary Unicode replacement-character path must not need a new format.
    ordinary = ROOT / "spools-�-é"
    ordinary.mkdir()
    directories = [(ordinary, 1)]
    native = os.fsencode(ROOT) + b"/spools-\xff"
    try:
        os.mkdir(native)
    except OSError as error:
        assert sys.platform == "darwin" and error.errno == 92, error
        print("macOS native byte-0xff directory fixture unavailable: errno=92 Illegal byte sequence")
    else:
        directories.append((Path(os.fsdecode(native)), 4))
    for number, (spool, schema) in enumerate(directories):
        env = {**ENV, "TMPDIR": str(spool), "PIRA_CTX_LIVE_CHECKPOINT_MS": "100"}
        store = ROOT / f"checkpoint-{number}"
        watch_store = ROOT / f"separate-watch-{number}"
        marker = ROOT / f"child-launched-{number}"
        code = f"import time;from pathlib import Path;Path({str(marker)!r}).write_text('yes');print('live-marker',flush=True);time.sleep(3)"
        owner = subprocess.Popen([BINARY, "check", "--store-dir", str(store), "--intent", "finite native checkpoint", "--", sys.executable, "-c", code],
                                 env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert owner.stderr.readline().startswith(b"LIVE")
            path = next((store / "live").glob("*.live.json"))
            checkpoint = json.loads(path.read_bytes())
            assert checkpoint["schema"] == schema, checkpoint
            assert ("stdout_path_native" in checkpoint) == (schema == 4)
            assert ("stderr_path_native" in checkpoint) == (schema == 4)
            ok(run(store, "stats", path, env=env))
            # Live watches must stay in the capture store; completed external reads are valid.
            outside = run(watch_store, "watch", "--capture", path, "--deadline", "1s", env=env)
            assert outside.returncode == 125 and b"outside its store" in outside.stderr
            pending = run(store, "watch", "--capture", path, "--deadline", "1s", "--sample-every", "100ms", env=env)
            assert pending.returncode == 21 and b"Job: Pending" in pending.stdout, (pending.returncode, pending.stdout, pending.stderr)
            assert b"render reliable: true" in pending.stdout
            assert "files=0 " in ok(run(store, "prune", "--max-store-bytes", "0", env=env))
            assert path.exists()
            out, err = owner.communicate(timeout=5)
            assert owner.returncode == 0 and marker.exists(), (owner.returncode, out, err)
            final = next(store.glob("*.piractx"))
            assert metadata(final)["compat_version"] == 4
            assert run(store, "raw", final, "--stdout", env=env).stdout == b"live-marker\n"
            completed = run(watch_store, "watch", "--capture", final, "--deadline", "1s", env=env)
            assert completed.returncode == 0 and b"Job: Succeeded" in completed.stdout, (completed.returncode, completed.stdout, completed.stderr)
            assert not list(spool.glob(".pira_ctx-spool-*"))
        finally:
            if owner.poll() is None:
                owner.kill()
                owner.communicate(timeout=5)


def interrupted():
    store = ROOT / "interrupted"
    pipe_read, pipe_write = os.pipe()
    code = f"import os,time;print('initial',flush=True);time.sleep(1);os.write({pipe_write},b'done')"
    owner = subprocess.Popen([BINARY, "check", "--store-dir", str(store), "--intent", "finite owner-death", "--", sys.executable, "-c", code],
                             env={**ENV, "PIRA_CTX_LIVE_CHECKPOINT_MS": "100"}, stdout=subprocess.PIPE, stderr=subprocess.PIPE, pass_fds=(pipe_write,))
    os.close(pipe_write)
    try:
        assert owner.stderr.readline().startswith(b"LIVE")
        path = next((store / "live").glob("*.live.json"))
        owner.kill()
        owner.communicate(timeout=5)
        # EOF on the inherited completion pipe awaits the finite child's actual exit.
        import select
        assert select.select([pipe_read], [], [], 5)[0]
        with os.fdopen(pipe_read, "rb") as completed:
            assert completed.read() == b"done"
        assert b"interrupted" in run(store, "list").stdout
        result = run(store, "watch", "--capture", path, "--deadline", "2s", "--sample-every", "100ms")
        assert result.returncode == 22, (result.returncode, result.stdout, result.stderr)
        assert b"owner lost" in result.stdout and b"Job: Unknown" in result.stdout
        assert b"render reliable: false" in result.stdout
    finally:
        if owner.poll() is None:
            owner.kill()
            owner.communicate(timeout=5)


with tempfile.TemporaryDirectory(prefix="pira-ctx-repair-") as temporary:
    ROOT = Path(temporary)
    spool = ROOT / "spools"
    spool.mkdir()
    ENV = {**os.environ, "TMPDIR": str(spool), "PIRA_CTX_STORE_DIR": str(ROOT / "unused"), "PIRA_CTX_THREAD_ID": "repair-regression"}
    # Both capture and exec children must emit UTF-8, including on CP1252 hosts.
    ENV.update(PYTHONIOENCODING="utf-8", TMP=str(spool), TEMP=str(spool))
    {"search": search, "check": check, "history": history, "index": index,
     "prune": prune, "exec": exec_case, "attention": attention,
     "controls": controls, "checkpoints": checkpoints, "interrupted": interrupted}[CASE]()
    print(CASE + ": passed")
