"""Launch/cancellation/evidence regressions with synthetic peers and credentials only."""
from __future__ import annotations

import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import time
import unittest

import test_team


@unittest.skipUnless(os.name == "posix", "POSIX fake-peer lifecycle fixtures")
class LaunchBoundaryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.env = self.fixture.env
        source = self.root / "original-home/auth.json"
        source.write_bytes(b'{"synthetic":"original"}')
        self.env["TEAM_TEST_AUTH_SOURCE"] = str(source)

    def start(self, *, resume: str | None = None, task: str = "steerable", extra: tuple[str, ...] = (), pass_fds: tuple[int, ...] = ()):
        args = (["resume", resume] if resume else
                ["run", "--cwd", str(self.root), "--model", "test-model", "--effort", "high"])
        proc = subprocess.Popen([str(self.fixture.bin), *args, "--store", str(self.root / "logs"),
                                 "--completion-gate", "Fixture complete", *extra, "--task", task],
                                cwd=self.root, env=self.env, pass_fds=pass_fds,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.communicate(timeout=3)
        self.addCleanup(cleanup)
        return proc

    def gate(self, stage: str) -> tuple[int, int, tuple[int, ...]]:
        ready_read, ready_write = os.pipe()
        release_read, release_write = os.pipe()
        for fd in (ready_read, ready_write, release_read, release_write):
            self.addCleanup(os.close, fd)
        self.env.update(TEAM_PAUSE_AT=stage, TEAM_BOUNDARY_READY_FD=str(ready_write),
                        TEAM_BOUNDARY_RELEASE_FD=str(release_read))
        return ready_read, release_write, (ready_write, release_read)

    def ready(self, proc, fd: int) -> Path:
        self.fixture.assert_worker_ready(proc, fd)
        notice = self.fixture.wait_stderr_notice(proc, "pira_team run_id:")
        return self.root / "logs" / notice.strip().split(": ", 1)[1]

    def control(self, run: Path, operation: str):
        extra = ["--task", "replacement", "--completion-gate", "Replacement complete"] if operation == "steer" else []
        return subprocess.run([str(self.fixture.bin), operation, run.name,
                               "--store", str(self.root / "logs"), *extra],
                              env=self.env, capture_output=True, text=True, timeout=8)

    def frame(self, endpoint: dict, **overrides) -> dict:
        address, port = endpoint["address"].rsplit(":", 1)
        request = {"token": endpoint["token"], "turn_id": endpoint["turn_id"],
                   "operation": "interrupt", "task": "", **overrides}
        with socket.create_connection((address, int(port)), timeout=3) as peer:
            peer.sendall((json.dumps(request) + "\n").encode())
            with peer.makefile() as stream:
                return json.loads(stream.readline())

    def interrupted(self, proc, run: Path) -> dict:
        out, err = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0, err)
        self.assertEqual(out, "")
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        self.assertEqual(state["attempts"][-1]["status"], "interrupted")
        self.assertIsNone(state["active_turn"])
        self.assertFalse((run / "control.json").exists())
        self.assertFalse((run / "codex-home/auth.json").is_symlink())
        self.assertFalse(state["usage_complete"])
        self.assertNotIn("unsupported Codex backend", err)
        self.assertNotIn("update Codex/Team", err)
        return state

    def test_run_id_interrupt_covers_schema_and_each_startup_rpc(self):
        for stage in ("schema", "initialize", "thread/start", "thread/inject_items", "turn/start"):
            with self.subTest(stage=stage):
                rd, _, fds = self.gate(stage)
                proc = self.start(pass_fds=fds)
                run = self.ready(proc, rd)
                endpoint = json.loads((run / "control.json").read_text())
                self.assertIsNone(endpoint["turn_id"])
                self.assertEqual(self.frame(endpoint, token="bad")["status"], "rejected")
                self.assertEqual(self.frame(endpoint, turn_id="stale")["status"], "rejected")
                rejected = self.control(run, "steer")
                self.assertNotEqual(rejected.returncode, 0)
                self.assertIn("no active turn", rejected.stderr)
                accepted = self.control(run, "interrupt")
                self.assertEqual(accepted.returncode, 0, accepted.stderr)
                self.assertIsNone(json.loads(accepted.stdout)["turn_id"])
                state = self.interrupted(proc, run)
                self.assertEqual(state["task"], "steerable")
                log = (run / "controls.jsonl").read_text()
                self.assertIn('"status":"accepted"', log)

    def test_active_notice_wait_is_bounded_with_a_paused_peer(self):
        rd, _, fds = self.gate("initialize")
        proc = self.start(pass_fds=fds)
        run = self.ready(proc, rd)
        with self.assertRaisesRegex(AssertionError, "launcher notice timed out"):
            self.fixture.wait_stderr_notice(proc, "pira_team active:", timeout=0.2)
        self.assertIsNotNone(proc.poll())
        self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")

    def test_startup_signal_is_interruption_not_backend_incompatibility(self):
        for stage in ("schema", "initialize"):
            with self.subTest(stage=stage):
                rd, _, fds = self.gate(stage)
                proc = self.start(pass_fds=fds)
                run = self.ready(proc, rd)
                proc.terminate()
                self.interrupted(proc, run)

    def test_resume_startup_interrupt_preserves_prior_revision(self):
        first = self.fixture.launch("review")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        run = Path(receipt["run_root"])
        old_report = Path(receipt["handoff_path"]).read_bytes()
        old_manifest = (run / "revisions/000001/manifest.json").read_bytes()
        rd, _, fds = self.gate("thread/resume")
        proc = self.start(resume=run.name, pass_fds=fds)
        self.assertEqual(self.ready(proc, rd), run)
        self.assertEqual(self.control(run, "interrupt").returncode, 0)
        state = self.interrupted(proc, run)
        self.assertEqual(state["revision"], 2)
        self.assertEqual(state["usage"]["input_tokens"], 17)
        self.assertEqual(Path(receipt["handoff_path"]).read_bytes(), old_report)
        self.assertEqual((run / "revisions/000001/manifest.json").read_bytes(), old_manifest)
        self.env.pop("TEAM_PAUSE_AT")
        resumed = self.fixture.access("resume", run.name, "recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(self.fixture.metadata(json.loads(resumed.stdout))["revision"], 3)

    def test_startup_interrupt_during_repair_and_implementation_retains_totals(self):
        for directory in ("repair", "implementation"):
            with self.subTest(directory=directory):
                rd, _, fds = self.gate("initialize")
                self.env["TEAM_PAUSE_DIRECTORY"] = directory
                if directory == "repair":
                    self.env["TEAM_CANDIDATE"] = self.fixture.candidate("[")
                    extra = ()
                else:
                    self.env.pop("TEAM_CANDIDATE", None)
                    extra = ("--inject-review", "--inject-implement")
                proc = self.start(task="review", extra=extra, pass_fds=fds)
                # The first attempt already completed; readiness is for the selected next attempt.
                run = self.ready(proc, rd)
                self.assertEqual(self.control(run, "interrupt").returncode, 0)
                state = self.interrupted(proc, run)
                self.assertEqual(state["usage"]["input_tokens"], 17)
                self.assertEqual(len(state["attempts"]), 2)
                if directory == "implementation":
                    self.assertEqual(state["stage"], "implementation")
                    self.assertTrue(Path(state["review_checkpoint"]).exists())

    def test_startup_capability_is_stale_after_active_turn_publication(self):
        rd, release, fds = self.gate("schema")
        proc = self.start(pass_fds=fds)
        run = self.ready(proc, rd)
        startup = json.loads((run / "control.json").read_text())
        os.write(release, b"1")
        self.fixture.wait_stderr_notice(proc, "pira_team active:")
        active = json.loads((run / "control.json").read_text())
        self.assertEqual(startup["token"], active["token"])
        self.assertIsNotNone(active["turn_id"])
        self.assertEqual(self.frame(startup)["status"], "rejected")
        result = self.control(run, "interrupt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["turn_id"], active["turn_id"])
        self.interrupted(proc, run)

    def test_schema_process_failure_does_not_claim_incompatible_inventory(self):
        self.env["TEAM_SCHEMA_FAIL"] = "1"
        proc = self.start(task="review")
        out, err = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(out, "")
        self.assertIn("native schema generation failed", err)
        self.assertNotIn("unsupported Codex backend", err)
        run = next((self.root / "logs").iterdir())
        self.assertFalse((run / "requests.jsonl").exists())
        self.assertFalse((run / "control.json").exists())
        self.assertFalse((run / "codex-home/auth.json").is_symlink())

    def test_partial_evidence_survives_refusal_and_startup_crash_or_rpc_error(self):
        for failure in ("after-refusal", "before-refusal", "before-crash", "before-rpc-error"):
            with self.subTest(failure=failure):
                self.env["TEAM_PARTIAL_FAILURE"] = failure
                proc = self.start(task="review")
                notice = self.fixture.wait_stderr_notice(proc, "pira_team run_id:")
                run = self.root / "logs" / notice.strip().split(": ", 1)[1]
                out, err = proc.communicate(timeout=8)
                self.assertNotEqual(proc.returncode, 0, err)
                self.assertEqual(out, "")
                state = json.loads((run / "manifest.json").read_text())
                self.assertEqual(state["status"], "failed")
                self.assertEqual(state["usage"], {"input_tokens": 11, "cached_input_tokens": 3, "output_tokens": 2})
                self.assertEqual(state["attempts"][0]["usage"], state["usage"])
                self.assertFalse(state["usage_complete"])
                self.assertEqual((run / "candidate.txt").read_text(), "partial diagnostic")
                self.assertIn("usage accounting incomplete", err)
                if "refusal" in failure:
                    self.assertIn("unexpected", err)
                    self.assertIn("refused", err)
                    requests = (run / "requests.jsonl").read_text()
                    self.assertIn('"code":-32601', requests)

    def test_partial_evidence_survives_interrupt_before_start_ack(self):
        self.env["TEAM_PARTIAL_FAILURE"] = "before-wait"
        rd, _, fds = self.gate("turn/start")
        proc = self.start(pass_fds=fds)
        run = self.ready(proc, rd)
        self.assertEqual(self.control(run, "interrupt").returncode, 0)
        state = self.interrupted(proc, run)
        self.assertEqual(state["usage"]["input_tokens"], 11)
        self.assertEqual((run / "candidate.txt").read_text(), "partial diagnostic")

    def test_detached_inherited_pipe_does_not_block_finalization_or_get_killed(self):
        rd, _, fds = self.gate("unused")
        self.env["TEAM_DETACHED_PIPE"] = "1"
        proc = self.start(pass_fds=fds)
        run = self.ready(proc, rd)
        pid = int((run / "codex-home/detached-fixture.pid").read_text())
        try:
            self.fixture.wait_stderr_notice(proc, "pira_team active:")
            self.assertEqual(self.control(run, "interrupt").returncode, 0)
            self.interrupted(proc, run)
            os.kill(pid, 0)  # Excluded descendant survives; only owned transport was stopped.
        finally:
            os.kill(pid, signal.SIGKILL)  # Only this fixture's captured synthetic PID.

    @unittest.skipUnless(os.environ.get("TEAM_TEST_SLOW_PREFLIGHT") == "1", "opt-in healthy >10s boundary")
    def test_healthy_preflight_can_finish_after_ten_seconds(self):
        rd, release, fds = self.gate("schema")
        proc = self.start(task="review", pass_fds=fds)
        run = self.ready(proc, rd)
        start = time.monotonic()
        select.select([], [], [], 10.2)  # Deliberate cutoff boundary, after confirmed peer readiness.
        self.assertIsNone(proc.poll(), "healthy preflight was killed at the former cutoff")
        os.write(release, b"1")
        out, err = proc.communicate(timeout=8)
        self.assertEqual(proc.returncode, 0, err)
        self.assertGreater(time.monotonic() - start, 10)
        self.assertEqual(json.loads(out)["status"], "completed")
        self.assertFalse((run / "control.json").exists())

    def test_auth_home_ignores_empty_candidates_and_preserves_precedence(self):
        user, home = self.root / "synthetic-user", self.root / "synthetic-home"
        for directory in (user, home):
            shutil.copytree(self.root / "original-home/sessions", directory / ".codex/sessions")
            (directory / ".codex/auth.json").write_text(json.dumps({"synthetic": directory.name}))
        original = self.root / "original-home"
        cases = [(codex, home_value, user / ".codex/auth.json")
                 for codex in (None, "") for home_value in (None, "")]
        cases += [(None, str(home), home / ".codex/auth.json"),
                  (str(original), str(home), original / "auth.json")]
        for codex, home_value, source in cases:
            with self.subTest(codex=codex, home=home_value):
                for key, value in (("CODEX_HOME", codex), ("HOME", home_value), ("USERPROFILE", str(user))):
                    if value is None:
                        self.env.pop(key, None)
                    else:
                        self.env[key] = value
                self.env["TEAM_TEST_AUTH_SOURCE"] = str(source)
                before = source.read_bytes()
                proc = self.start(task="review")
                out, err = proc.communicate(timeout=8)
                self.assertEqual(proc.returncode, 0, err)
                run = Path(json.loads(out)["run_root"])
                self.assertFalse((run / "codex-home/auth.json").is_symlink())
                self.assertFalse((run / "codex-home/auth.json").exists())
                self.assertEqual(source.read_bytes(), before)
        # The same empty-HOME fallback must detach after permission-confirmation failure.
        self.env.update(CODEX_HOME="", HOME="", USERPROFILE=str(user),
                        TEAM_TEST_AUTH_SOURCE=str(user / ".codex/auth.json"), TEAM_BAD_PERMISSION="1")
        proc = self.start(task="review")
        notice = self.fixture.wait_stderr_notice(proc, "pira_team run_id:")
        run = self.root / "logs" / notice.strip().split(": ", 1)[1]
        out, err = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("did not confirm workspace-write", err)
        self.assertEqual((run / "stderr.log").read_text(), "")
        self.assertFalse((run / "codex-home/auth.json").is_symlink())


if __name__ == "__main__":
    unittest.main()
