"""Distinct offline policy-addition routes; use a freshly built PIRA_TEAM_BIN.

Fresh selector combinations and format repair are covered by test_team/test_write.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import unittest

import test_team


class PolicyRouteTests(unittest.TestCase):
    def setUp(self) -> None:
        # Composition keeps unittest from rediscovering the entire TeamTests suite.
        self.fixture = test_team.TeamTests()
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.setUp()
        self.env = self.fixture.env
        self.env["TEAM_ASSERT_PREFIX"] = "1"
        repo = Path(__file__).resolve().parents[4]
        self.figure = (repo / "modules/PUBLIC_FIGURE_STYLE.md").read_text().rstrip()
        self.writing = (repo / "modules/SCIENTIFIC_WRITING.md").read_text().rstrip()

    def receipt(self, result: subprocess.CompletedProcess[str]) -> dict:
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], "completed")
        return receipt

    def state(self, run: Path) -> dict:
        return json.loads((run / "codex-home/fixture-state.json").read_text())

    def assert_additions(self, text: str, count: int) -> None:
        self.assertEqual(text.count(self.figure), count)
        self.assertEqual(text.count(self.writing), count)

    def guidance_snapshot(self, run: Path) -> dict[str, bytes]:
        return {name: (run / name).read_bytes() for name in (
            "manifest.json", "policy.md", "phase.md", "requests.jsonl", "controls.jsonl",
            "codex-home/fixture-state.json",
        )}

    def test_optional_guidance_rule_is_consistent_across_usage_sources(self) -> None:
        repo = Path(__file__).resolve().parents[4]
        rule = ("Select only optional guidance directly required by the assigned task; "
                "never blanket-inject all policies. Keep applicable quality requirements intact.")
        for relative in ("AGENTS.md", "tools/src/pira_team/coordinator.md", "tools/src/pira_team/help.md"):
            with self.subTest(source=relative):
                self.assertIn(rule, (repo / relative).read_text())

    def test_resume_adds_figure_and_writing_together_once(self) -> None:
        first = self.receipt(self.fixture.launch("Assess assigned artifact"))
        run = Path(first["run_root"])
        prefix = (run / "policy.md").read_bytes()
        handoff = Path(first["handoff_path"]).read_bytes()
        thread = self.fixture.metadata(first)["thread_id"]
        self.assert_additions(self.state(run)["base_policy"], 0)
        for flags, count in (
            (("--inject-fig", "--inject-write"), 1),
            ((), 0),
            (("--inject-fig", "--inject-write"), 0),
        ):
            with self.subTest(flags=flags, expected_additions=count):
                resumed = self.receipt(self.fixture.access("resume", run.name, *flags))
                latest = Path(resumed["handoff_path"]).parent.parent
                self.assert_additions((latest / "phase.md").read_text(), count)
                self.assertEqual((latest / "policy.md").read_bytes(), prefix)
                manifest = self.fixture.metadata(resumed)
                self.assertTrue(manifest["inject_fig"])
                self.assertTrue(manifest["inject_write"])
                self.assertFalse(manifest["inject_review"])
                self.assertFalse(manifest["inject_implement"])
                self.assertEqual(manifest["thread_id"], thread)
        self.assert_additions("\n".join(self.state(run)["phases"]), 1)
        self.assertEqual((run / "policy.md").read_bytes(), prefix)
        self.assertEqual(Path(first["handoff_path"]).read_bytes(), handoff)

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_active_steer_rejects_policy_flags_before_control_or_guidance_changes(self) -> None:
        proc, run, _ = self.fixture.start_control_fixture()
        before = self.guidance_snapshot(run)
        for flag in ("--inject-review", "--inject-implement", "--inject-fig", "--inject-write"):
            with self.subTest(flag=flag):
                rejected = self.fixture.access("steer", run.name, flag, "--task", "Revised scope",
                                               "--completion-gate", "Revised checks")
                self.assertNotEqual(rejected.returncode, 0)
                self.assertIn(f"unsupported steer option {flag}", rejected.stderr)
                self.assertEqual(rejected.stdout, "")
                self.assertEqual(self.guidance_snapshot(run), before)
                self.assertIsNone(proc.poll())
        interrupted = self.fixture.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        stdout, stderr = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_ordinary_steer_changes_task_and_gate_not_selected_policies(self) -> None:
        for flags in ((), ("--inject-fig", "--inject-write")):
            with self.subTest(flags=flags):
                fixture = self.fixture
                proc, run, _ = fixture.start_control_fixture(extra=flags)
                before = json.loads((run / "manifest.json").read_text())
                prefix = (run / "policy.md").read_bytes()
                phase = (run / "phase.md").read_bytes()
                task = "--inject-review --inject-implement --inject-fig --inject-write are task literals"
                gate = "Report only the redirected scope and checks"
                steered = fixture.access("steer", run.name, "--task=" + task, "--completion-gate", gate)
                self.assertEqual(steered.returncode, 0, steered.stderr)
                stdout, stderr = proc.communicate(timeout=8)
                self.assertEqual(proc.returncode, 0, stderr)
                receipt = json.loads(stdout)
                after = fixture.metadata(receipt)
                self.assertEqual(after["task"], task)
                self.assertEqual(after["completion_gate"], gate)
                for key in ("inject_review", "inject_implement", "inject_fig", "inject_write"):
                    self.assertEqual(after[key], before[key], key)
                self.assertEqual((run / "policy.md").read_bytes(), prefix)
                self.assertEqual((run / "phase.md").read_bytes(), phase)
                state = self.state(run)
                self.assertEqual(state["history"], ["steerable", task])
                self.assertEqual(state["turns"], 1)
                self.assertEqual(len(state["phases"]), 1)
                self.assert_additions(state["base_policy"], int(bool(flags)))
                self.assertNotIn("# Review\n", state["base_policy"])
                self.assertNotIn("# Implementation\n", state["base_policy"])
                self.assertEqual(Path(receipt["handoff_path"]).read_text(), "ANSWER " + task)

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_active_resume_requires_owner_exit_before_adding_policies(self) -> None:
        proc, run, _ = self.fixture.start_control_fixture()
        before = self.guidance_snapshot(run)
        entries = sorted(path.name for path in run.iterdir())
        rejected = self.fixture.access("resume", run.name, "--inject-fig", "--inject-write")
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("run already has an active owner", rejected.stderr)
        self.assertEqual(rejected.stdout, "")
        self.assertEqual(self.guidance_snapshot(run), before)
        self.assertEqual(sorted(path.name for path in run.iterdir()), entries)
        self.assertIsNone(proc.poll())

        interrupted = self.fixture.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        # Acknowledging interrupt is not owner exit: await the original launcher before resuming.
        stdout, stderr = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")
        self.assertFalse((run / "control.json").exists())
        self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
        prefix = before["policy.md"]
        for flags, count in ((("--inject-fig", "--inject-write"), 1), ((), 0)):
            with self.subTest(flags=flags):
                resumed = self.receipt(self.fixture.access("resume", run.name, *flags))
                latest = Path(resumed["handoff_path"]).parent.parent
                self.assertEqual((latest / "policy.md").read_bytes(), prefix)
                self.assert_additions((latest / "phase.md").read_text(), count)
                manifest = self.fixture.metadata(resumed)
                self.assertTrue(manifest["inject_fig"])
                self.assertTrue(manifest["inject_write"])
        self.assert_additions("\n".join(self.state(run)["phases"]), 1)
        self.assertEqual((run / "policy.md").read_bytes(), prefix)


if __name__ == "__main__":
    unittest.main()
