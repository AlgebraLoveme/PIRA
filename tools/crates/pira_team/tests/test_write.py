"""Offline --inject-write regressions; set PIRA_TEAM_BIN to a fresh binary."""
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import unittest

import test_team


class WriteTests(unittest.TestCase):
    def setUp(self) -> None:
        # Composition reuses the fake backend without discovering TeamTests again.
        self.fixture = test_team.TeamTests()
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.setUp()
        self.root = self.fixture.root
        self.env = self.fixture.env
        self.env["TEAM_ASSERT_PREFIX"] = "1"
        repo = Path(__file__).resolve().parents[4]
        self.writing = (repo / "modules/SCIENTIFIC_WRITING.md").read_text().rstrip()
        self.figure = (repo / "modules/PUBLIC_FIGURE_STYLE.md").read_text().rstrip()

    def receipt(self, result: subprocess.CompletedProcess[str]) -> dict:
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], "completed")
        return receipt

    def launch(self, *flags: str) -> dict:
        return self.receipt(self.fixture.launch("Assess assigned artifact", *flags))

    def resume(self, receipt: dict, *flags: str) -> dict:
        return self.receipt(self.fixture.access("resume", receipt["run_id"], *flags))

    def state(self, receipt: dict) -> dict:
        return json.loads((Path(receipt["run_root"]) / "codex-home/fixture-state.json").read_text())

    def phase(self, receipt: dict) -> str:
        return (Path(receipt["handoff_path"]).parent.parent / "phase.md").read_text()

    def assert_writing(self, text: str, count: int) -> None:
        self.assertEqual(text.count(self.writing), count)
        self.assertEqual(text.count("# SCIENTIFIC_WRITING\n"), count)

    def test_fresh_write_only_and_composed_guidance(self) -> None:
        for other in ((), ("--inject-review",), ("--inject-implement",),
                      ("--inject-fig",), ("--inject-implement", "--inject-fig")):
            with self.subTest(other=other):
                receipt = self.launch("--inject-write", *other)
                manifest = self.fixture.metadata(receipt)
                self.assertTrue(manifest["inject_write"])
                self.assertIsNone(manifest.get("stage"))
                turn, = self.state(receipt)["turn_details"]
                self.assert_writing(turn["policy"] + turn["phase"], 1)
                self.assert_writing(turn["policy"], 1)
                self.assertEqual("# Review\n" in turn["policy"], "--inject-review" in other)
                self.assertEqual("# Implementation\n" in turn["policy"], "--inject-implement" in other)
                self.assertEqual(turn["policy"].count(self.figure), int("--inject-fig" in other))

    def test_default_and_legacy_aliases_never_select_writing(self) -> None:
        for flags in ((), ("--code-review",), ("--allow-fix",), ("--inject-review",),
                      ("--inject-implement",), ("--inject-fig",),
                      ("--inject-review", "--inject-implement")):
            with self.subTest(flags=flags):
                receipt = self.launch(*flags)
                self.assertFalse(self.fixture.metadata(receipt)["inject_write"])
                for turn in self.state(receipt)["turn_details"]:
                    self.assert_writing(turn["policy"] + turn["phase"], 0)

    def test_resume_adds_writing_once_and_preserves_cached_prefix(self) -> None:
        first = self.launch("--inject-fig")
        run = Path(first["run_root"])
        prefix = (run / "policy.md").read_bytes()
        handoff = Path(first["handoff_path"]).read_bytes()
        first_thread = self.fixture.metadata(first)["thread_id"]
        added = self.resume(first, "--inject-write", "--inject-review", "--inject-implement")
        phase = self.phase(added)
        self.assert_writing(phase, 1)
        self.assertIn("# Review\n", phase)
        self.assertIn("# Implementation\n", phase)
        self.assertNotIn(self.figure, phase)
        for flags in ((), ("--inject-write",)):
            with self.subTest(flags=flags):
                again = self.resume(first, *flags)
                self.assert_writing(self.phase(again), 0)
                latest = Path(again["handoff_path"]).parent.parent
                self.assertEqual((latest / "policy.md").read_bytes(), prefix)
                manifest = self.fixture.metadata(again)
                self.assertTrue(manifest["inject_write"])
                self.assertEqual(manifest["thread_id"], first_thread)
                self.assertIsNone(manifest.get("stage"))
        self.assertEqual((run / "policy.md").read_bytes(), prefix)
        self.assertEqual(Path(first["handoff_path"]).read_bytes(), handoff)
        self.assert_writing("\n".join(self.state(first)["phases"]), 1)

    def test_fresh_writing_is_retained_without_resume_reinjection(self) -> None:
        first = self.launch("--inject-write")
        prefix = self.state(first)["base_policy"]
        for flags in ((), ("--inject-write",)):
            with self.subTest(flags=flags):
                resumed = self.resume(first, *flags)
                self.assertTrue(self.fixture.metadata(resumed)["inject_write"])
                turn = self.state(resumed)["turn_details"][-1]
                self.assertEqual(turn["policy"], prefix)
                self.assert_writing(turn["policy"] + turn["phase"], 1)
                self.assert_writing(turn["phase"], 0)

    def test_old_manifest_missing_writing_selection_migrates_without_implying_it(self) -> None:
        for version in (5, 20):
            with self.subTest(version=version):
                first = self.launch()
                run = Path(first["run_root"])
                path = run / "manifest.json"
                manifest = json.loads(path.read_text())
                manifest.pop("inject_write")
                manifest["worker_policy_version"] = version
                path.write_text(json.dumps(manifest))
                prefix = (run / "policy.md").read_bytes()
                for flags, selected in (((), False), (("--inject-write",), True), ((), True)):
                    resumed = self.resume(first, *flags)
                    self.assertEqual(self.fixture.metadata(resumed)["inject_write"], selected)
                    self.assert_writing(self.phase(resumed), int(bool(flags)))
                    latest = Path(resumed["handoff_path"]).parent.parent
                    self.assertEqual((latest / "policy.md").read_bytes(), prefix)
                self.assert_writing("\n".join(self.state(first)["phases"]), 1)

    def test_retained_coordinator_refresh_is_additive_and_injected_once(self) -> None:
        repo = Path(__file__).resolve().parents[4]
        coordinator = (repo / "tools/src/pira_team/coordinator.md").read_text().rstrip()
        for flags in ((), ("--inject-write",)):
            with self.subTest(flags=flags):
                first = self.launch("--allow-subteam", "--inject-review")
                run = Path(first["run_root"])
                path = run / "manifest.json"
                initial = self.fixture.metadata(first)
                current = initial["coordinator_policy_version"]
                self.assertGreater(current, 21)
                prefix = (run / "policy.md").read_bytes()
                retained = dict(initial, coordinator_policy_version=current - 1)
                path.write_text(json.dumps(retained))
                for refreshed in (True, False):
                    resumed = self.resume(first, *(flags if refreshed else ()))
                    manifest = self.fixture.metadata(resumed)
                    self.assertEqual(manifest["coordinator_policy_version"], current)
                    self.assertEqual(manifest["worker_policy_version"], initial["worker_policy_version"])
                    self.assertTrue(manifest["allow_subteam"])
                    self.assertTrue(manifest["inject_review"])
                    self.assertEqual(manifest["inject_write"], bool(flags))
                    phase = self.phase(resumed)
                    self.assertEqual(phase.count(coordinator), int(refreshed))
                    self.assertEqual(phase.count("# Coordinator Team usage"), int(refreshed))
                    self.assert_writing(phase, int(refreshed and bool(flags)))
                    for unchanged in ("# Technical artifact worker", "# Review\n", "# Implementation\n"):
                        self.assertNotIn(unchanged, phase)
                    latest = Path(resumed["handoff_path"]).parent.parent
                    self.assertEqual((latest / "policy.md").read_bytes(), prefix)
                    self.assertEqual((run / "policy.md").read_bytes(), prefix)

    def test_staged_review_implementation_keeps_writing_once_in_each_turn(self) -> None:
        first = self.launch("--inject-review", "--inject-implement", "--inject-fig", "--inject-write")
        manifest = self.fixture.metadata(first)
        self.assertTrue(manifest["inject_write"])
        self.assertEqual([attempt["stage"] for attempt in manifest["attempts"]], ["review", "implementation"])
        self.assertEqual(manifest["stage"], "implementation")
        state = self.state(first)
        review, implementation = state["turn_details"]
        self.assertEqual(review["policy"], implementation["policy"])
        for turn in (review, implementation):
            self.assert_writing(turn["policy"] + turn["phase"], 1)
            self.assertEqual((turn["policy"] + turn["phase"]).count(self.figure), 1)
        self.assertIn("Do not edit project source, tests, configuration", review["phase"])
        self.assertNotIn("# Implementation\n", review["policy"] + review["phase"])
        self.assertIn("# Implementation\n", implementation["phase"])
        checkpoint = Path(manifest["review_checkpoint"]).read_bytes()
        resumed = self.resume(first, "--inject-write")
        self.assertEqual(self.fixture.metadata(resumed)["stage"], "implementation")
        self.assertEqual(self.state(resumed)["turns"], 3)
        self.assert_writing(self.phase(resumed), 0)
        self.assertEqual(Path(manifest["review_checkpoint"]).read_bytes(), checkpoint)

    def test_unacknowledged_writing_is_not_marked_received(self) -> None:
        first = self.launch()
        self.env["TEAM_BAD_PERMISSION"] = "1"
        failed = self.fixture.access("resume", first["run_id"], "--inject-write")
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(failed.stdout, "")
        self.assertIn("Codex did not confirm", failed.stderr)
        self.assertFalse(self.fixture.metadata(first)["inject_write"])
        self.env.pop("TEAM_BAD_PERMISSION")
        resumed = self.resume(first)
        self.assert_writing(self.phase(resumed), 0)
        self.assertFalse(self.fixture.metadata(resumed)["inject_write"])
        retried = self.resume(first, "--inject-write")
        self.assert_writing(self.phase(retried), 1)
        self.assertTrue(self.fixture.metadata(retried)["inject_write"])

    def test_acknowledged_writing_survives_failed_turn_without_duplicate(self) -> None:
        first = self.launch()
        self.env["TEAM_PARTIAL_FAILURE"] = "before-rpc-error"
        failed = self.fixture.access("resume", first["run_id"], "--inject-write")
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(failed.stdout, "")
        self.assertIn("fixture turn/start error", failed.stderr)
        manifest = self.fixture.metadata(first)
        self.assertTrue(manifest["inject_write"])
        self.assert_writing(manifest["phase_instructions"], 1)
        self.env.pop("TEAM_PARTIAL_FAILURE")
        resumed = self.resume(first)
        self.assert_writing(self.phase(resumed), 0)
        self.assertTrue(self.fixture.metadata(resumed)["inject_write"])
        self.assert_writing("\n".join(self.state(resumed)["phases"]), 1)

    def test_format_repair_retains_added_writing_and_cached_prefix(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.fixture.candidate("{}")
        first = self.launch("--format", "json")
        prefix = (Path(first["run_root"]) / "policy.md").read_bytes()
        self.env["TEAM_CANDIDATE"] = self.fixture.candidate('{"ok":')
        self.env["TEAM_REPAIRED"] = self.fixture.candidate('{"ok":true}')
        repaired = self.resume(first, "--inject-write")
        manifest = self.fixture.metadata(repaired)
        self.assertTrue(manifest["inject_write"])
        self.assertEqual(manifest["repairs"], 1)
        self.assertEqual([attempt["status"] for attempt in manifest["attempts"]], ["invalid", "validated"])
        self.assertEqual(json.loads(Path(repaired["handoff_path"]).read_text()), {"ok": True})
        original, invalid, repair = self.state(repaired)["turn_details"]
        self.assert_writing(original["policy"] + original["phase"], 0)
        self.assert_writing(invalid["phase"], 1)
        self.assert_writing(repair["phase"], 0)
        self.assertIn("Format repair only", repair["phase"])
        repair_logs = Path(manifest["attempts"][-1]["logs"])
        self.assertEqual((repair_logs / "policy.md").read_bytes(), prefix)
        self.assert_writing("\n".join(self.state(repaired)["phases"]), 1)
        self.env["TEAM_CANDIDATE"] = self.fixture.candidate('{"ok":true}')
        self.env.pop("TEAM_REPAIRED")
        resumed = self.resume(first)
        self.assertTrue(self.fixture.metadata(resumed)["inject_write"])
        self.assert_writing(self.phase(resumed), 0)


if __name__ == "__main__":
    unittest.main()
