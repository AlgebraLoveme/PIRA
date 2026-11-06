"""Bounded Unix watch status regressions with task-owned detached pipe holders."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

BINARY, CASE = sys.argv[1:]


def wait_for(predicate):
    deadline = time.monotonic() + 3
    while not predicate():
        assert time.monotonic() < deadline, "fixture did not become ready"
        time.sleep(0.01)


def exercise(root, code, held="", stop=False):
    store = root / f"store-{code}-{held}-{stop}"
    pid_file = root / f"holder-{code}-{held}-{stop}"
    script = "import os,sys,time;from pathlib import Path;print('probe-stdout',flush=True);print('probe-stderr',file=sys.stderr,flush=True)\n"
    if held:
        script += f"""
r,w=os.pipe()
pid=os.fork()
if pid==0:
    os.close(r)
    os.setsid()
    if {held!r}=='stdout': os.dup2(os.open(os.devnull,os.O_WRONLY),2)
    if {held!r}=='stderr': os.dup2(os.open(os.devnull,os.O_WRONLY),1)
    Path({str(pid_file)!r}).write_text(str(os.getpid()))
    os.write(w,b'1');os.close(w)
    time.sleep(30)
    os._exit(0)
os.close(w);os.read(r,1);os.close(r)
"""
    script += f"os._exit({code})\n"
    command = [BINARY, "watch", "--store-dir", str(store), "--deadline", "6s",
               "--sample-every", "1s", "--review-after", "2s", "--", sys.executable, "-c", script]
    started = time.monotonic()
    owner = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        if stop:
            wait_for(pid_file.exists)
            state_file = next((store / "watch/state").glob("*.json"))
            stopped = subprocess.run([BINARY, "watch", "--store-dir", str(store), state_file.stem,
                                      "--stop"], capture_output=True, timeout=4)
            assert stopped.returncode == 0, stopped.stderr
        stdout, stderr = owner.communicate(timeout=8)
        assert time.monotonic() - started < 5, "bounded drain regressed"
        state_file = next((store / "watch/state").glob("*.json"))
        state = json.loads(state_file.read_text())
        if stop:
            assert owner.returncode == 23 and state["monitor"] == "stopped", (stdout, stderr, state)
            return
        if held and code == 75:
            assert owner.returncode == 22 and state["job"] == "unknown", (stdout, stderr, state)
            assert state["monitor"] == "failed" and not state["rendered_reliable"], state
            assert "did not close after process-tree cleanup" in state["detail"], state
            return
        expected_job = {0: "succeeded", 2: "failed", 75: "pending"}[code]
        expected_exit = {0: 0, 2: 20, 75: 10}[code]
        assert owner.returncode == expected_exit, (owner.returncode, stdout, stderr, state)
        assert state["job"] == expected_job and state["attempt"] == "idle", state
        assert state["monitor"] == ("paused" if code == 75 else "complete"), state
        assert state["rendered_reliable"] == (not held), state
        if held:
            assert "output incomplete" in state["detail"], state
            assert "did not close after process-tree cleanup" in state["detail"], state
            for stream in ["stdout", "stderr"] if held == "both" else [held]:
                assert f"{stream} output incomplete" in state["detail"], state
            assert b"render reliable: false" in stdout, stdout
        else:
            assert state["raw_stdout"] == list(b"probe-stdout\n"), state
            assert state["raw_stderr"] == list(b"probe-stderr\n"), state
            assert "output incomplete" not in state["detail"], state
        if held == "stdout":
            assert state["raw_stderr"] == list(b"probe-stderr\n"), state
        if held == "stderr":
            assert state["raw_stdout"] == list(b"probe-stdout\n"), state
    finally:
        if owner.poll() is None:
            owner.kill()
        owner.communicate(timeout=3)
        if pid_file.exists():
            # The task-owned child sleeps 30s, beyond the sum of bounded waits.
            try:
                os.kill(int(pid_file.read_text()), signal.SIGTERM)
            except ProcessLookupError:
                pass


with tempfile.TemporaryDirectory(prefix="pira-watch-status-") as tmp:
    root = Path(tmp)
    if CASE == "ordinary":
        for code in [0, 2, 75]:
            exercise(root, code)
    else:
        code, held = {"success": (0, "stdout"), "failure": (2, "stderr"), "both": (0, "both"),
                      "pending": (75, "both"), "stop": (0, "both")}[CASE]
        exercise(root, code, held, CASE == "stop")
