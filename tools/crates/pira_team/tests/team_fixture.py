"""Shared fake-backend environment and lifecycle helpers; never a test suite."""
from __future__ import annotations

from collections.abc import Callable
import json
import os
from pathlib import Path
import subprocess
import tempfile

from native_fixture import cleanup_temporary, windows_verbatim_path


class TeamFixture:
    """Own an isolated session; register teardown before any fallible setup."""
    def __init__(self, add_cleanup: Callable[..., None]) -> None:
        self.add_cleanup = add_cleanup
        self.temp = tempfile.TemporaryDirectory(prefix="team-test-")
        self.add_cleanup(cleanup_temporary, self.temp)
        self.root = Path(self.temp.name).resolve()
        if os.name == "nt":
            self.root = Path(windows_verbatim_path(str(self.root)))
        self.bin = Path(os.environ["PIRA_TEAM_BIN"]).resolve()
        home = self.root / "original-home"
        home.mkdir()
        (home / "auth.json").write_text('{"OPENAI_API_KEY":"synthetic-fixture-key"}')
        (home / "AGENTS.md").write_text("global instructions must not be inherited")
        self.env = dict(os.environ, PATH=f"{self.root}{os.pathsep}{os.environ['PATH']}", CODEX_HOME=str(home))
        platform_temp = self.root / "platform-temp"
        platform_temp.mkdir()
        self.env.update({key: str(platform_temp) for key in ("TMPDIR", "TMP", "TEMP")})
        self.env["PIRA_CTX_STORE_DIR"] = str(self.root / "ctx-store")
        self.env["PIRA_DEC_STORE_DIR"] = str(self.root / "dec-store")
        self.env["TEAM_TEST_BACKEND_CONTRACT"] = str(Path(__file__).resolve().parents[3] / "src/pira_team/backend_contract.json")
        self.env["TEAM_TEST_BACKEND_FIXTURE"] = str(Path(__file__).with_name("backend_fixture.py").resolve())
        self.env.pop("PIRA_TEAM_BUILD_ROOTS", None)
        self.env.pop("PIRA_TEAM_CHILD", None)
        self.env.pop("PIRA_TEAM_ATTEMPT", None)
        self.env.pop("PIRA_TEAM_CONTEXT", None)
        self.env.pop("PIRA_TEAM_HANDOFF", None)
        self.env.pop("CLAUDE_CODE_SESSION_ID", None)
        self.env.pop("CODEX_THREAD_ID", None)
        self.env.pop("CODEX_API_KEY", None)
        self.env.pop("PIRA_TEAM_DIR", None)
        fake = self.root / "codex"
        fake.write_text(Path(__file__).with_name("fake_codex.py").read_text())
        fake.chmod(0o700)
        if os.name == "nt":
            import shutil
            import sys
            shutil.copyfile(os.environ["PIRA_TEAM_TEST_CODEX"], self.root / "codex.exe")
            self.env.update(TEAM_FIXTURE_PYTHON=sys.executable, TEAM_FIXTURE_CODEX=str(fake))
        self.parent([{}])

    def launch(self, task: str, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
             "--store", str(self.root / "logs"), "--cwd", str(self.root), *extra, "--task", task],
            env=self.env, capture_output=True, text=True, timeout=10,
        )

    def fail_startup(self, proc: subprocess.Popen[str], reason: str, consumed_stderr: str = "") -> None:
        if proc.poll() is None:
            proc.terminate()
        try:
            stdout, stderr = proc.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
            stdout, stderr = proc.communicate(timeout=5)
        raise AssertionError(f"{reason}; launcher exit={proc.returncode}\nstdout:\n{stdout}\nstderr:\n{consumed_stderr}{stderr}")

    def assert_worker_ready(self, proc: subprocess.Popen[str], read_fd: int) -> None:
        import select
        if not select.select([read_fd], [], [], 5)[0]:
            self.fail_startup(proc, "worker readiness timed out")
        ready = os.read(read_fd, 1)
        if ready != b"1":
            self.fail_startup(proc, f"worker readiness returned {ready!r}, expected b'1'")

    def wait_stderr_notice(self, proc: subprocess.Popen[str], notice: str, timeout: float = 5) -> str:
        import queue
        import threading
        consumed = []
        result = queue.Queue(maxsize=1)
        def read_notice():
            while True:
                line = proc.stderr.readline()
                consumed.append(line)
                if not line or notice in line:
                    result.put(line)
                    return
        reader = threading.Thread(target=read_notice, daemon=True)
        reader.start()
        try:
            try:
                line = result.get(timeout=timeout)
            except queue.Empty:
                if proc.poll() is None:
                    proc.terminate()
                reader.join(timeout=8)
                self.fail_startup(proc, f"launcher notice timed out: {notice}", "".join(consumed))
            if not line:
                self.fail_startup(proc, f"launcher exited before {notice}", "".join(consumed))
            return line
        finally:
            reader.join(timeout=1)

    def permission_context(self) -> dict:
        return {"cwd":str(self.root), "approval_policy":"never", "sandbox_policy":{
            "type":"workspace-write", "writable_roots":[], "network_access":False,
            "exclude_tmpdir_env_var":True, "exclude_slash_tmp":True}}

    def parent(self, contexts: list[dict], *, identity: str = "parent-session") -> Path:
        self.env["CODEX_THREAD_ID"] = "parent-session"
        directory = self.root / "original-home/sessions/2026/10/01"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "rollout-2026-10-01-parent-session.jsonl"
        events = [{"type":"session_meta", "payload":{"id":identity}},
                  {"type":"response_item", "payload":{"text":"PARENT_TRANSCRIPT_MUST_NOT_BE_FORWARDED"}}]
        events += [{"type":"turn_context", "payload":{**self.permission_context(), **c}} for c in contexts]
        path.write_text("".join(json.dumps(e)+"\n" for e in events))
        # A sibling session must not be read; its content is deliberately invalid JSON.
        (directory / "rollout-other-session.jsonl").write_text("unrelated private conversation")
        return path

    def access(self, command: str, run: str, *extra: str) -> subprocess.CompletedProcess[str]:
        if command in ("resume", "steer") and extra and not any(x.startswith("--completion-gate") for x in extra):
            extra = (*extra, "--completion-gate", "Report findings and actual checks")
        return subprocess.run([str(self.bin), command, run, *extra, "--store", str(self.root / "logs")],
            env=self.env, capture_output=True, text=True, timeout=5)

    def candidate(self, content: str, filename: str = "findings.json", format: str = "json") -> str:
        return json.dumps({"filename": filename, "format": format, "content": content})

    def start_control_fixture(self, *, pass_fds: tuple[int, ...] = (), extra: tuple[str, ...] = ()):
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report actual checks",
            "--model", "test-model", "--effort", "high", "--cwd", str(self.root),
            "--store", str(self.root / "logs"), *extra, "--task", "steerable"],
            env=self.env, pass_fds=pass_fds, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.communicate(timeout=3)
        self.add_cleanup(cleanup)
        self.wait_stderr_notice(proc, "pira_team active:")
        endpoint, = (self.root / "logs").glob("*/control.json")
        return proc, endpoint.parent, json.loads(endpoint.read_text())
