"""Caller-visible observation and tier updates against synthetic native workers."""
import json
import os
from pathlib import Path
import signal
import subprocess
import unittest

from team_fixture import TeamFixture


class ActiveControlTests(unittest.TestCase):
    def setUp(self):
        self.f = TeamFixture(self.addCleanup)

    def read_manifest(self, run):
        result = self.f.access("read", run.name, "manifest.json")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def requests(self, run):
        return [json.loads(line) for line in (run / "requests.jsonl").read_text().splitlines()]

    def start_wait(self, run, reference=None):
        process = subprocess.Popen([str(self.f.bin), "wait", reference or run.name, "--store", str(self.f.root / "logs")],
                                   env=self.f.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            if process.poll() is None:
                process.terminate()
            try:
                process.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate(timeout=3)
        self.addCleanup(cleanup)
        # No receipt is permitted while this owner remains running. This also
        # gives the tiny CLI time to install its signal handler before cancellation.
        with self.assertRaises(subprocess.TimeoutExpired):
            process.communicate(timeout=1)
        return process

    def test_wait_keeps_resolved_label_until_active_owner_settles(self):
        owner, run, _ = self.f.start_control_fixture(extra=("--label", "waiting"))
        waiter = self.start_wait(run, "@waiting")
        self.assertEqual(self.read_manifest(run)["status"], "running")
        other = self.f.launch("different completed worker")
        self.assertEqual(other.returncode, 0, other.stderr)
        moved = self.f.access("bookmark", json.loads(other.stdout)["run_id"], "--label", "waiting")
        self.assertEqual(moved.returncode, 0, moved.stderr)
        with self.assertRaises(subprocess.TimeoutExpired):
            waiter.communicate(timeout=1)
        steered = self.f.access("steer", run.name, "--task", "waited result")
        self.assertEqual(steered.returncode, 0, steered.stderr)
        owner_stdout, owner_stderr = owner.communicate(timeout=8)
        self.assertEqual(owner.returncode, 0, owner_stderr)
        stdout, stderr = waiter.communicate(timeout=8)
        self.assertEqual(waiter.returncode, 0, stderr)
        self.assertEqual(json.loads(stdout), json.loads(owner_stdout))
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER waited result")
        self.assertEqual(sum(r.get("method") == "turn/start" for r in self.requests(run)), 1)

    def test_stopping_wait_client_does_not_interrupt_active_owner(self):
        owner, run, endpoint = self.f.start_control_fixture()
        before = self.read_manifest(run)
        waiter = self.start_wait(run)
        if os.name == "posix":
            waiter.send_signal(signal.SIGINT)
        else:
            waiter.terminate()
        stdout, stderr = waiter.communicate(timeout=8)
        self.assertNotEqual(waiter.returncode, 0)
        self.assertEqual(stdout, "")
        if os.name == "posix":
            self.assertIn("waiting client stopped; worker was not interrupted", stderr)
        self.assertIsNone(owner.poll())
        self.assertEqual(self.read_manifest(run), before)
        self.assertEqual(json.loads((run / "control.json").read_text()), endpoint)
        result = self.f.access("steer", run.name, "--task", "survived waiter")
        self.assertEqual(result.returncode, 0, result.stderr)
        _, stderr = owner.communicate(timeout=8)
        self.assertEqual(owner.returncode, 0, stderr)
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER survived waiter")
        self.assertFalse(any(r.get("method") == "turn/interrupt" for r in self.requests(run)))

    def test_live_tier_applied_and_unknown_keep_work_and_resume_intent(self):
        self.f.env["TEAM_TIER_REPLIES"] = json.dumps([
            {"result": {"status": "applied"}},
            {"result": {"status": "unrecognized"}},
        ])
        owner, run, endpoint = self.f.start_control_fixture()
        initial = self.read_manifest(run)
        runtime = json.loads((run / "runtime.json").read_text())
        for tier, code, publication in [("fast", 0, "applied"), ("standard", 1, "unknown")]:
            with self.subTest(publication=publication):
                result = self.f.access("tier", run.name, tier)
                self.assertEqual(result.returncode, code, result.stderr)
                receipt = json.loads(result.stdout)
                self.assertEqual(receipt["publication"], publication)
                self.assertIsNone(receipt["actual_service_tier"])
                state = self.read_manifest(run)
                self.assertEqual(state["service_tier"], "fast")
                self.assertEqual(state["status"], "running")
                self.assertEqual(state["thread_id"], initial["thread_id"])
                self.assertEqual(json.loads((run / "control.json").read_text()), endpoint)
                current_runtime = json.loads((run / "runtime.json").read_text())
                for field in ("backend_pid", "backend_birth"):
                    self.assertEqual(current_runtime[field], runtime[field])
                self.assertIsNone(owner.poll())
        requests = self.requests(run)
        self.assertEqual(sum(r.get("method") == "turn/start" for r in requests), 1)
        self.assertFalse(any(r.get("method") in ("turn/interrupt", "turn/steer") for r in requests))
        finished = self.f.access("steer", run.name, "--task", "tier work completed")
        self.assertEqual(finished.returncode, 0, finished.stderr)
        _, stderr = owner.communicate(timeout=8)
        self.assertEqual(owner.returncode, 0, stderr)
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER tier work completed")
        resumed = self.f.access("resume", run.name)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        state = self.read_manifest(run)
        self.assertEqual(state["service_tier"], "fast")
        requests = [json.loads(line) for line in (Path(state["logs"]) / "requests.jsonl").read_text().splitlines()]
        starts = [request for request in requests if request.get("method") == "turn/start"]
        self.assertEqual(starts[0]["params"]["serviceTierForTurn"], "fast")

    def test_live_tier_rejects_other_session_and_changed_policy_without_publication(self):
        owner, run, _ = self.f.start_control_fixture()
        identity = "foreign-main"
        log = Path(self.f.env["CODEX_HOME"]) / "sessions" / ("rollout-" + identity + ".jsonl")
        records = [{"type": "session_meta", "payload": {"id": identity}},
                   {"type": "turn_context", "payload": self.f.permission_context()}]
        log.write_text("".join(json.dumps(record) + "\n" for record in records))
        files = [run / name for name in ("manifest.json", "control.json", "requests.jsonl", "controls.jsonl")]
        before = [path.read_bytes() for path in files]
        foreign = subprocess.run([str(self.f.bin), "tier", run.name, "fast", "--store", str(self.f.root / "logs")],
                                 env=dict(self.f.env, CODEX_THREAD_ID=identity), capture_output=True, text=True, timeout=8)
        self.assertNotEqual(foreign.returncode, 0)
        self.assertIn("run unavailable to this main session", foreign.stderr)
        self.assertEqual(foreign.stdout, "")
        self.assertEqual([path.read_bytes() for path in files], before)
        self.f.parent([{"sandbox_policy": {"type": "danger-full-access"}}])
        changed = self.f.access("tier", run.name, "fast")
        self.assertNotEqual(changed.returncode, 0)
        self.assertIn("current caller permissions differ", changed.stderr)
        self.assertEqual(changed.stdout, "")
        self.assertEqual([path.read_bytes() for path in files], before)
        self.assertIsNone(owner.poll())
        # The same active endpoint accepts its original valid caller afterward.
        self.f.parent([{}])
        accepted = self.f.access("tier", run.name, "fast")
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        self.assertEqual(self.read_manifest(run)["service_tier"], "fast")
        self.assertEqual(self.f.access("interrupt", run.name).returncode, 0)
        owner.communicate(timeout=8)

    def test_live_tier_is_available_during_format_repair(self):
        self.f.env.update(TEAM_CANDIDATE=self.f.candidate("{"), TEAM_REPAIR_WAIT="1")
        owner = subprocess.Popen([str(self.f.bin), "run", "--cwd", str(self.f.root),
            "--store", str(self.f.root / "logs"), "--model", "test-model", "--effort", "high",
            "--task", "repair tier", "--completion-gate", "Deliver JSON", "--format", "json"],
            env=self.f.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            if owner.poll() is None:
                owner.terminate()
            try:
                owner.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                owner.kill()
                owner.communicate(timeout=3)
        self.addCleanup(cleanup)
        self.f.wait_stderr_notice(owner, "repair=true")
        run, = (self.f.root / "logs").glob("*/control.json")
        run = run.parent
        result = self.f.access("tier", run.name, "fast")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["publication"], "applied")
        self.assertEqual(self.read_manifest(run)["service_tier"], "fast")
        self.assertIsNone(owner.poll())
        requests = [json.loads(line) for line in (run / "repair/requests.jsonl").read_text().splitlines()]
        self.assertEqual(sum(r.get("method") == "turn/start" for r in requests), 1)
        self.assertFalse(any(r.get("method") in ("turn/interrupt", "turn/steer") for r in requests))
        self.assertEqual(self.f.access("interrupt", run.name).returncode, 0)
        owner.communicate(timeout=8)
        self.assertEqual(self.read_manifest(run)["status"], "interrupted")


if __name__ == "__main__":
    unittest.main()
