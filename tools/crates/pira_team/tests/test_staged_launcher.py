"""Staged local coordinators select their own launcher without weakening peer identity."""
import json
import os
from pathlib import Path
import shutil
import unittest

import test_subteams


@unittest.skipUnless(os.name == "posix", "POSIX worker PATH subprocess fixture")
class StagedLauncherTests(unittest.TestCase):
    def setUp(self):
        # Compose helpers: inheriting SubteamTests would rediscover its entire suite.
        self.fixture = test_subteams.SubteamTests(methodName="runTest")
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.setUp()
        fixture = self.fixture
        candidate = fixture.bin
        name = "pira_team.exe" if os.name == "nt" else "pira_team"
        self.staged = fixture.root / "staged parent" / name
        self.ambient = fixture.root / "ambient launcher" / name
        for target in (self.staged, self.ambient):
            target.parent.mkdir()
            shutil.copy2(candidate, target)
        self.assertNotEqual(self.staged.resolve(), self.ambient.resolve())
        fixture.bin = self.staged
        fixture.env["TEAM_SUBTEAM_BIN"] = str(self.staged)
        fixture.env["PATH"] = str(self.ambient.parent) + os.pathsep + fixture.env["PATH"]
        self.assertEqual(Path(shutil.which("pira_team", path=fixture.env["PATH"])).resolve(), self.ambient)
        # Synthetic input only; never inspect authentication payloads or tokens.
        fixture.env["CODEX_API_KEY"] = "synthetic-staged-launcher-key"

    def run_ids(self):
        return {path.parent.name for path in (self.fixture.root / "logs").glob("*/manifest.json")}

    def finish_parent(self, process, coordinator):
        self.fixture.finish(coordinator)
        result = self.fixture.wait(process)
        self.assertEqual(result[0], 0, result)
        self.assertEqual(self.fixture.manifest(coordinator)["status"], "completed")

    def test_bare_child_selects_staged_parent_and_completes_once(self):
        fixture = self.fixture
        process, coordinator = fixture.start()
        self.assertEqual(self.run_ids(), {coordinator})
        child = fixture.child(coordinator, launcher="pira_team")
        self.assertIn("run_id", child, child)
        self.assertEqual(child["launcher"], str(self.staged.resolve()))
        child_id = child["run_id"]
        parent = fixture.manifest(coordinator)
        manifest = fixture.manifest(child_id)
        self.assertEqual(manifest["parent_run_id"], coordinator)
        self.assertEqual(manifest["creator_main_session_id"], parent["thread_id"])
        self.assertFalse(manifest["allow_subteam"])
        self.assertEqual(manifest["revision"], 1)
        self.assertEqual(self.run_ids(), {coordinator, child_id})
        fixture.finish(child_id)
        result = fixture.command(coordinator, operation="wait", handle=child["handle"])
        self.assertEqual(result["returncode"], 0, result)
        self.assertEqual(json.loads(result["stdout"])["run_id"], child_id)
        self.assertEqual(fixture.manifest(child_id)["status"], "completed")
        requests = [json.loads(line) for line in
                    (fixture.root / "logs" / child_id / "requests.jsonl").read_text(encoding="utf-8").splitlines()]
        self.assertEqual(sum(request["method"] == "thread/start" for request in requests), 1)
        self.assertEqual(sum(request["method"] == "turn/start" for request in requests), 1)
        self.finish_parent(process, coordinator)
        self.assertEqual(self.run_ids(), {coordinator, child_id})

    def test_explicit_different_launcher_still_fails_equality_guard(self):
        fixture = self.fixture
        process, coordinator = fixture.start()
        rejected = fixture.child(coordinator, launcher=str(self.ambient))
        self.assertEqual(rejected["launcher"], str(self.ambient.resolve()))
        self.assertNotEqual(rejected["returncode"], 0, rejected)
        self.assertIn("launcher authentication request rejected", rejected["stderr"])
        self.assertNotIn("handle", rejected)
        children = self.run_ids() - {coordinator}
        self.assertEqual(len(children), 1, "one rejected admission, no duplicate launches")
        child_id = children.pop()
        self.assertEqual(fixture.manifest(child_id)["parent_run_id"], coordinator)
        self.assertEqual(fixture.manifest(child_id)["status"], "failed")
        self.assertFalse((fixture.sockets / (child_id + ".sock")).exists())
        self.finish_parent(process, coordinator)
        self.assertEqual(len(self.run_ids()), 2)

    def test_worker_path_does_not_redirect_selected_backend(self):
        fixture = self.fixture
        fixture.env.update(OPENAI_BASE_URL="https://selected.invalid/fake",
                           TEAM_EXPECT_BACKEND_URL="https://selected.invalid/fake")
        # If backend resolution incorrectly uses worker PATH, this fails even
        # before the fixture service starts. Backend PATH must stay inherited.
        decoy = self.staged.parent / "codex"
        decoy.write_text("#!/bin/sh\nexit 77\n", encoding="utf-8")
        decoy.chmod(0o700)
        process, coordinator = fixture.start()
        result = fixture.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x",
                                 launcher="pira_team", env={"OPENAI_BASE_URL": "https://different.invalid/not-selected"})
        self.assertEqual(result["returncode"], 0, result)
        self.assertEqual(result["launcher"], str(self.staged.resolve()))
        child_id = json.loads(result["stdout"])["run_id"]
        self.assertEqual(fixture.manifest(child_id)["status"], "completed")
        self.assertEqual(fixture.manifest(child_id)["parent_run_id"], coordinator)
        self.finish_parent(process, coordinator)
        self.assertEqual(self.run_ids(), {coordinator, child_id})


if __name__ == "__main__":
    unittest.main()
