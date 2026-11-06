"""Bounded contention, output retention and index recovery; disposable stores only."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

BINARY = sys.argv[1]
ENV = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}


def run(store, mode, *args):
    return subprocess.run([BINARY, mode, "--store-dir", str(store), *map(str, args)],
                          env=ENV, capture_output=True, timeout=15)


def ok(result):
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    return result.stdout.decode()


def wait_for(predicate):
    deadline = time.monotonic() + 5
    while not predicate():
        assert time.monotonic() < deadline, "publishers did not reach index lock"
        time.sleep(0.01)


def verify_output(store, path, stdout, stderr, truncated=False):
    result = run(store, "raw", path, "--stdout")
    assert result.returncode == 0 and result.stdout == stdout, result.stderr
    result = run(store, "raw", path, "--stderr")
    assert result.returncode == 0 and result.stdout == stderr, result.stderr
    text = ok(run(store, "search", path, "TOKEN"))
    assert "1 hits" in text and "TOKEN" in text and "complete=0" not in text, text
    assert f"truncated={str(truncated).lower()}" in ok(run(store, "stats", path))
    if truncated:
        text = ok(run(store, "search", path, "TAIL_OUTSIDE_INDEX"))
        assert "0 hits" in text and "search covered only the indexed retained prefix" in text, text


def exercise(root, mode, count):
    store = root / mode
    ok(run(store, "capture", "--intent", "index seed", "--", sys.executable,
           "-c", "print('seed')"))
    indexes = store / "indexes"
    lock = (indexes / ".index.owner-lock").open("r+")
    processes = []
    try:
        fcntl.flock(lock, fcntl.LOCK_EX)
        # Force the exact observed timeout once. The child still exits successfully
        # and leaves its durable capture discoverable via authoritative scan fallback.
        result = run(store, "capture", "--intent", "bounded held-owner reproduction", "--",
                     sys.executable, "-c", "import sys;print('TOKEN orphan');print('err',file=sys.stderr)")
        assert result.returncode == 0, result.stderr
        assert b"stored result but could not update index: timed out" in result.stderr, result.stderr
        orphan = next(p for p in store.glob("*.piractx") if (indexes / (".dirty-" + p.stem)).exists())
        verify_output(store, orphan, b"TOKEN orphan\n", b"err\n")
        assert orphan.stem in ok(run(store, "list"))
        fcntl.flock(lock, fcntl.LOCK_UN)

        # Rebuild once after explicit invalidation, before the overlap check. The
        # original orphan marker must also be retired, rather than poison every update.
        ok(run(store, "capture", "--intent", "repair invalidated index", "--",
               sys.executable, "-c", "print('repair')"))
        assert not list(indexes.glob(".dirty-*")), "published orphan marker survived repair"
        unrelated = indexes / "0000000000000000.jsonl"
        unrelated.write_bytes(b"")
        fcntl.flock(lock, fcntl.LOCK_EX)
        for i in range(count):
            code = (f"import sys;print('TOKEN {mode}-{i}');"
                    f"print('diagnostic {i}',file=sys.stderr)")
            env = ENV
            if mode == "check":
                code += f";print('detail\\n'*1100,end='');print('TAIL_OUTSIDE_INDEX');sys.exit({7 * (i % 2)})"
                env = {**ENV, "PIRA_CTX_MAX_INDEXED_LINES": "1000"}
            processes.append(subprocess.Popen(
                [BINARY, mode, "--store-dir", str(store), "--intent", f"concurrent {mode} {i}",
                 "--", sys.executable, "-c", code], env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE))
        wait_for(lambda: len(list(indexes.glob(".dirty-*"))) == count)
        fcntl.flock(lock, fcntl.LOCK_UN)
        for i, process in enumerate(processes):
            stdout, stderr = process.communicate(timeout=15)
            expected_exit = 7 * (i % 2) if mode == "check" else 0
            assert process.returncode == expected_exit and b"warning:" not in stderr, (stdout, stderr)
            if mode == "check":
                assert b"index_truncated=1" in stdout
        assert unrelated.exists(), "overlapping publishers rebuilt unrelated indexes"
        assert not list(indexes.glob(".dirty-*")), "completed marker not reconciled"
        captures = {p.stem: p for p in store.glob("*.piractx")}
        rows = [json.loads(line) for p in indexes.glob("*.jsonl") for line in p.read_text().splitlines()]
        assert {r["id"] for r in rows} == set(captures), "index inventory incomplete"
        assert len(captures) == count + 3
        for i in range(count):
            token = f"TOKEN {mode}-{i}\n".encode()
            expected = token + (b"detail\n" * 1100 + b"TAIL_OUTSIDE_INDEX\n" if mode == "check" else b"")
            matches = [p for p in captures.values() if run(store, "raw", p, "--stdout").stdout == expected]
            assert len(matches) == 1
            verify_output(store, matches[0], expected, f"diagnostic {i}\n".encode(), mode == "check")
        print(f"{mode}: held-owner timeout retained raw/search/list; {count} overlapping publishers recovered complete indexes")
    finally:
        fcntl.flock(lock, fcntl.LOCK_UN)
        lock.close()
        for process in processes:
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)


with tempfile.TemporaryDirectory(prefix="pira-ctx-index-") as tmp:
    root = Path(tmp)
    exercise(root, "capture", 6)
    exercise(root, "check", 4)
