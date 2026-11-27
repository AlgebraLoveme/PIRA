"""No inference: exercise Team through its CLI and synthetic app-server peer."""
import json
import subprocess
import time
import unittest

from team_fixture import TeamFixture


class GoalLifetimeTests(unittest.TestCase):
    def setUp(self):
        self.f = TeamFixture(self.addCleanup)

    def run_path(self):
        paths = list((self.f.root / "logs").glob("*/manifest.json"))
        self.assertEqual(len(paths), 1)
        return paths[0].parent

    def manifest(self, run):
        return json.loads((run / "manifest.json").read_text())

    def peer(self, run):
        return json.loads((run / "codex-home/fixture-state.json").read_text())

    def requests(self, run):
        return [json.loads(line) for path in run.rglob("requests.jsonl")
                for line in path.read_text().splitlines()]

    def starts(self, run):
        return [r for r in self.requests(run) if r.get("method") == "turn/start"]

    def test_native_successor_keeps_one_revision_and_final_handoff_wins(self):
        self.f.env["TEAM_GOAL_MODE"] = "continue"
        result = self.f.launch("GRH-like proof")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        manifest = self.manifest(run)
        self.assertEqual(manifest["status"], "completed")
        self.assertEqual(manifest["revision"], 1)
        self.assertEqual(len(manifest["attempts"]), 1)
        self.assertEqual(manifest["native_goal"]["status"], "complete")
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER goal final")
        peer = self.peer(run)
        self.assertEqual(peer["turns"], 2)
        self.assertEqual(len(peer["native_turns"]), 1)
        self.assertEqual(len(self.starts(run)), 1)
        self.assertEqual(manifest["thread_id"], peer["thread"])
        self.assertEqual(manifest["usage"]["input_tokens"], 34)
        self.assertEqual(manifest["usage"]["output_tokens"], 10)
        self.assertFalse((run / "repair").exists())

    def test_no_goal_is_one_turn_including_older_backend(self):
        self.f.env["TEAM_BAD_BACKEND"] = "no-goals"
        result = self.f.launch("ordinary")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        self.assertEqual(self.peer(run)["turns"], 1)
        self.assertEqual(len(self.starts(run)), 1)
        self.assertFalse(any(r["method"] == "thread/read" for r in self.requests(run)))
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER ordinary")
        resumed = self.f.access("resume", run.name)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(self.peer(run)["turns"], 2)

    def test_idle_grace_reads_status_preserves_goal_and_taskless_resume_has_no_kickoff(self):
        self.f.env["TEAM_GOAL_MODE"] = "gap"
        started = time.monotonic()
        result = self.f.launch("GRH-like proof")
        elapsed = time.monotonic() - started
        self.assertNotEqual(result.returncode, 0)
        self.assertGreaterEqual(elapsed, 5)
        run = self.run_path()
        manifest = self.manifest(run)
        self.assertEqual(manifest["status"], "incomplete")
        self.assertIn("readiness threshold", manifest["error"])
        self.assertIn("backend status read is idle", manifest["error"])
        goal = self.peer(run)["goal"]
        self.assertEqual(goal["status"], "active")
        self.assertEqual(sum(r["method"] == "thread/read" for r in self.requests(run)), 1)
        self.f.env["TEAM_GOAL_MODE"] = "resume"
        resumed = self.f.access("resume", run.name)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        state = self.manifest(run)
        self.assertEqual(state["status"], "completed")
        self.assertEqual(state["thread_id"], manifest["thread_id"])
        self.assertEqual(state["revision"], 2)
        self.assertEqual(len(self.starts(run)), 1)
        requests = self.requests(run)
        resumption, = [r for r in requests if r["method"] == "thread/resume"]
        self.assertTrue(resumption["params"]["baseInstructions"].startswith(
            (run / "revisions/000002/policy.md").read_text() + "\n\n"))
        self.assertTrue(resumption["params"]["baseInstructions"].endswith(
            (run / "revisions/000002/phase.md").read_text()))
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER goal final")
        # Replacing the task while an active goal persists must fail BEFORE resume.
        self.assertEqual(goal["objective"], self.peer(run)["goal"]["objective"])

    def test_active_goal_replacement_rejected_before_execution_capable_resume(self):
        self.f.env["TEAM_GOAL_MODE"] = "exit"
        self.assertNotEqual(self.f.launch("initial").returncode, 0)
        run = self.run_path()
        before = self.peer(run)
        result = self.f.access("resume", run.name, "--task", "replacement", "--model", "replacement-model")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no model work started", result.stderr)
        self.assertEqual(before, self.peer(run))
        requests = self.requests(run)
        self.assertFalse(any(r["method"] == "thread/resume" for r in requests))
        self.assertEqual(len(self.starts(run)), 1)
        self.assertEqual(self.manifest(run)["model"], "test-model")
        self.f.env["TEAM_GOAL_MODE"] = "resume"
        resumed = self.f.access("resume", run.name)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(self.manifest(run)["status"], "completed")
        self.assertEqual(len(self.starts(run)), 1)

    def test_idle_status_reply_does_not_hide_queued_native_admission(self):
        self.f.env["TEAM_GOAL_MODE"] = "status-race"
        result = self.f.launch("proof")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        self.assertEqual(self.manifest(run)["status"], "completed")
        self.assertEqual(self.peer(run)["turns"], 2)
        self.assertEqual(len(self.starts(run)), 1)

    def test_terminal_goal_snapshot_cannot_end_an_unannounced_running_turn(self):
        self.f.env["TEAM_GOAL_MODE"] = "terminal-race"
        result = self.f.launch("proof")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        self.assertEqual(self.manifest(run)["status"], "completed")
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER goal final")
        self.assertEqual(self.peer(run)["turns"], 2)
        self.assertEqual(len(self.starts(run)), 1)
        self.assertTrue(any(r["method"] == "thread/read" for r in self.requests(run)))

    def test_known_active_goal_requires_capability_before_resume(self):
        self.f.env["TEAM_GOAL_MODE"] = "exit"
        self.assertNotEqual(self.f.launch("proof").returncode, 0)
        run = self.run_path()
        before = self.peer(run)
        self.f.env["TEAM_BAD_BACKEND"] = "no-goals"
        result = self.f.access("resume", run.name, "--model", "unsupported-replacement")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("thread/goal/get unavailable", result.stderr)
        self.assertEqual(before, self.peer(run))
        self.assertFalse(any(r["method"] == "thread/resume" for r in self.requests(run)))

        self.assertEqual(self.manifest(run)["model"], "test-model")
        self.f.env.pop("TEAM_BAD_BACKEND")
        self.f.env["TEAM_GOAL_MODE"] = "resume"
        resumed = self.f.access("resume", run.name)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(len(self.starts(run)), 1)

    def test_new_goal_events_invalidate_stale_reads_and_tentative_settlement(self):
        for mode in ("stale-snapshot", "stale-settlement"):
            with self.subTest(mode=mode):
                f = TeamFixture(self.addCleanup)
                f.env.update(TEAM_GOAL_MODE=mode, TEAM_CANDIDATE=json.dumps({
                    "status": "completed", "format": "markdown", "content": "UNFINISHED CHECKPOINT"}))
                started = time.monotonic()
                result = f.launch("proof still active")
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertGreaterEqual(time.monotonic() - started, 5)
                path, = (f.root / "logs").glob("*/manifest.json")
                state = json.loads(path.read_text())
                self.assertEqual(state["status"], "incomplete")
                self.assertEqual(state["native_goal"]["status"], "active")
                self.assertIn("readiness threshold", state["error"])
                self.assertFalse(state.get("result"))
                peer = json.loads((path.parent / "codex-home/fixture-state.json").read_text())
                self.assertEqual(peer["goal"]["status"], "active")

    def test_old_record_missing_goal_field_or_raced_read_cannot_admit_resume(self):
        for fault in ("missing", "changed"):
            with self.subTest(fault=fault):
                f = TeamFixture(self.addCleanup)
                f.env["TEAM_GOAL_MODE"] = "exit"
                self.assertNotEqual(f.launch("old Team proof").returncode, 0)
                path, = (f.root / "logs").glob("*/manifest.json")
                state = json.loads(path.read_text())
                state.pop("native_goal", None)  # Old Team 0.9.0 record shape.
                path.write_text(json.dumps(state))
                peer_path = path.parent / "codex-home/fixture-state.json"
                before = peer_path.read_bytes()
                f.env["TEAM_GOAL_MODE"] = "resume"
                if fault == "missing":
                    f.env["TEAM_GOAL_REPLY"] = "{}"
                    expected = "missing required native Goal snapshot.goal field"
                else:
                    f.env["TEAM_GOAL_READ_RACE"] = "1"
                    expected = "Goal changed during pre-resume state read"
                result = f.access("resume", path.parent.name, "--model", "unadmitted-model")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stderr)
                self.assertEqual(peer_path.read_bytes(), before)
                self.assertEqual(json.loads(path.read_text())["model"], "test-model")
                requests = self.requests(path.parent)
                self.assertFalse(any(r["method"] == "thread/resume" for r in requests))
                self.assertEqual(sum(r["method"] == "turn/start" for r in requests), 1)

    def test_status_active_at_threshold_protects_running_turn_without_text(self):
        self.f.env["TEAM_GOAL_MODE"] = "status-active"
        # First admission waits for the authoritative read; successor is silent
        # for >5 seconds. Only the BETWEEN-turn interval has a deadline.
        result = subprocess.run([str(self.f.bin), "run", "--task", "proof",
            "--completion-gate", "Finish proof", "--model", "test-model", "--effort", "high",
            "--store", str(self.f.root / "logs"), "--cwd", str(self.f.root)],
            env=self.f.env, text=True, capture_output=True, timeout=16)
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        self.assertEqual(self.manifest(run)["status"], "completed")
        self.assertEqual(self.peer(run)["turns"], 2)
        self.assertEqual(sum(r["method"] == "thread/read" for r in self.requests(run)), 1)
        self.assertFalse(any(r["method"] == "turn/interrupt" for r in self.requests(run)))

    def test_backend_exit_with_active_goal_is_visible_failure(self):
        self.f.env["TEAM_GOAL_MODE"] = "exit"
        result = self.f.launch("proof")
        self.assertNotEqual(result.returncode, 0)
        run = self.run_path()
        self.assertEqual(self.manifest(run)["status"], "failed")
        self.assertEqual(self.peer(run)["goal"]["status"], "active")
        self.assertIn("app-server", self.manifest(run)["error"])

    def test_stopped_goal_states_wait_for_final_turn_but_never_claim_success(self):
        for status in ("paused", "blocked", "usageLimited", "budgetLimited", "cleared"):
            with self.subTest(status=status):
                f = TeamFixture(self.addCleanup)
                f.env["TEAM_GOAL_MODE"] = status
                result = f.launch("proof")
                self.assertNotEqual(result.returncode, 0)
                run, = (f.root / "logs").glob("*/manifest.json")
                manifest = json.loads(run.read_text())
                self.assertEqual(manifest["status"], "incomplete")
                self.assertIn(status, manifest["error"])
                # Stopped update precedes final text in the fixture.
                self.assertIn('"status": "completed"', (run.parent / "candidate.txt").read_text())
                self.assertFalse((run.parent / "repair").exists())

    def test_stopped_goal_preserves_validated_blocker_handoff(self):
        self.f.env.update(TEAM_GOAL_MODE="blocked", TEAM_CANDIDATE=json.dumps({
            "status": "needs_decision", "format": "markdown", "content": "Need a proof assumption"}))
        result = self.f.launch("proof")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        self.assertEqual(self.manifest(run)["status"], "needs_decision")
        self.assertEqual(self.f.access("read", run.name).stdout, "Need a proof assumption")
        self.assertEqual(self.manifest(run)["native_goal"]["status"], "blocked")

    def test_native_turn_controls_retarget_and_keep_same_owner(self):
        self.f.env["TEAM_GOAL_MODE"] = "control"
        owner, run, _ = self.f.start_control_fixture()
        self.f.wait_stderr_notice(owner, "pira_team native continuation:")
        endpoint = json.loads((run / "control.json").read_text())
        self.assertEqual(endpoint["turn_id"], "2")
        identity = json.loads((run / "runtime.json").read_text())
        steered = self.f.access("steer", run.name, "--task", "steered proof")
        self.assertEqual(steered.returncode, 0, steered.stderr)
        _, stderr = owner.communicate(timeout=8)
        self.assertEqual(owner.returncode, 0, stderr)
        self.assertEqual(self.f.access("read", run.name).stdout, "ANSWER steered proof")
        self.assertEqual(json.loads((run / "runtime.json").read_text()), identity)
        self.assertEqual(len(self.starts(run)), 1)
        steer, = [r for r in self.requests(run) if r["method"] == "turn/steer"]
        self.assertEqual(steer["params"]["expectedTurnId"], "2")

    def test_interrupt_and_tier_target_the_native_successor(self):
        self.f.env["TEAM_GOAL_MODE"] = "control"
        owner, run, _ = self.f.start_control_fixture()
        self.f.wait_stderr_notice(owner, "pira_team native continuation:")
        tier = self.f.access("tier", run.name, "fast")
        self.assertEqual(tier.returncode, 0, tier.stderr)
        self.assertEqual(json.loads(tier.stdout)["turn_id"], "2")
        interrupted = self.f.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        owner.communicate(timeout=8)
        self.assertEqual(self.manifest(run)["status"], "interrupted")
        self.assertEqual(self.peer(run)["goal"]["status"], "active")
        interrupt, = [r for r in self.requests(run) if r["method"] == "turn/interrupt"]
        self.assertEqual(interrupt["params"]["turnId"], "2")
        self.assertEqual(len(self.starts(run)), 1)

    def test_interrupt_between_turns_leaves_goal_resumable_without_new_turn(self):
        self.f.env["TEAM_GOAL_MODE"] = "gap"
        owner, run, _ = self.f.start_control_fixture()
        self.f.wait_stderr_notice(owner, "pira_team awaiting native continuation")
        self.assertIsNone(json.loads((run / "control.json").read_text())["turn_id"])
        rejected = self.f.access("steer", run.name, "--task", "gap replacement")
        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(self.peer(run)["turns"], 1)
        result = self.f.access("interrupt", run.name)
        self.assertEqual(result.returncode, 0, result.stderr)
        owner.communicate(timeout=8)
        self.assertEqual(self.manifest(run)["status"], "interrupted")
        self.assertEqual(self.peer(run)["goal"]["status"], "active")
        self.assertEqual(self.peer(run)["turns"], 1)
        self.assertFalse(any(r["method"] == "turn/interrupt" for r in self.requests(run)))

    def test_review_does_not_advance_on_checkpoint_while_goal_active(self):
        self.f.env["TEAM_GOAL_MODE"] = "stage"
        result = self.f.launch("review then implement", "--inject-review", "--inject-implement")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        state = self.peer(run)
        self.assertEqual(state["turns"], 3)
        self.assertEqual(len(state["native_turns"]), 1)
        self.assertEqual(len(self.starts(run)), 2)
        self.assertIn('"stage":"review"', state["native_turns"][0]["phase"])
        self.assertEqual(len(self.manifest(run)["attempts"]), 2)
        checkpoint = run / "artifacts/review-checkpoint.md"
        self.assertEqual(checkpoint.read_text(), "ANSWER goal final")
        # Implementation's inherited complete goal is not a new active goal.
        self.assertEqual(self.manifest(run)["status"], "completed")

    def test_repair_runs_only_after_goal_settles_and_does_not_resume_old_goal(self):
        self.f.env.update(TEAM_GOAL_MODE="continue", TEAM_CANDIDATE=self.f.candidate("{"),
                          TEAM_REPAIRED=self.f.candidate('{"ok":true}'))
        result = self.f.launch("proof JSON", "--format", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.run_path()
        self.assertEqual(self.peer(run)["turns"], 3)
        self.assertEqual(len(self.starts(run)), 2)
        self.assertEqual(len(self.manifest(run)["attempts"]), 2)
        self.assertEqual(self.f.access("read", run.name).stdout, '{"ok":true}')


if __name__ == "__main__":
    unittest.main()
