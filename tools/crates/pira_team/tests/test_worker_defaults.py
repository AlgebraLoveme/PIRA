"""Isolated worker-defaults CLI checks with fake Codex; no paid model calls.

Run with PIRA_TEAM_BIN pointing at a freshly built binary.
"""
from __future__ import annotations

import json
import subprocess
import unittest

import test_team


class WorkerDefaultsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.env = self.fixture.env
        self.store = self.root / "logs"
        self.env["PIRA_TEAM_DIR"] = str(self.store)
        self.fixture.parent([{"model": "gpt-6-astra", "effort": "ultra"}])

    def cli(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.fixture.bin), *args], cwd=self.root, env=self.env,
                              capture_output=True, text=True, timeout=10)

    def config(self, *args: str) -> dict:
        result = self.cli("config", *args)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def set_profile(self, main: str, model: str, effort: str) -> dict:
        return self.config("set", "--main", main, "--model", model, "--effort", effort)

    def launch(self, *extra: str) -> dict:
        result = self.cli("run", "--store", str(self.store), "--cwd", str(self.root),
                          "--task", "defaults-check", "--completion-gate", "Fixture complete", *extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def profile(self, receipt: dict) -> tuple[str, str, dict]:
        manifest = self.fixture.metadata(receipt)
        return manifest["model"], manifest["effort"], manifest["profile_sources"]

    def test_show_without_session_is_read_only_and_store_selection_is_isolated(self) -> None:
        self.env.pop("CODEX_THREAD_ID")
        report = self.config("show")
        self.assertEqual(report["path"], str(self.store / "worker_defaults.json"))
        self.assertEqual(report["overrides"], {})
        self.assertEqual(report["bundled"]["mappings"]["gpt-6-astra"],
                         {"model": "gpt-6.1-sol", "effort": "high"})
        self.assertFalse(self.store.exists())
        self.set_profile("main", "worker", "low")
        other = self.root / "other-store"
        report = self.config("show", "--store", str(other))
        self.assertEqual(report["overrides"], {})
        self.assertFalse(other.exists())
        report = self.config("set", "--main", "main", "--model", "other-worker",
                             "--effort", "medium", "--store", "other-store")
        self.assertEqual(report["path"], str(other / "worker_defaults.json"))
        self.assertEqual(self.config("show")["overrides"]["main"]["model"], "worker")
        self.assertEqual(self.config("show", "--store", str(other))["overrides"]["main"]["model"], "other-worker")
        self.assertEqual(self.config("reset", "--main", "main", "--store", str(other))["overrides"], {})

    def test_set_reset_preserves_other_exact_main_identifiers(self) -> None:
        self.set_profile("gpt-6-astra", "first-worker", "low")
        self.set_profile("GPT-6-ASTRA", "case-worker", "medium")
        self.set_profile("gpt-6-astra", "replacement-worker", "high")
        report = self.config("reset", "--main", "gpt-6-astra")
        self.assertEqual(report["overrides"], {"GPT-6-ASTRA": {"model": "case-worker", "effort": "medium"}})
        self.assertEqual(self.config("reset", "--main", "not-present")["overrides"], report["overrides"])
        self.config("reset", "--main", "GPT-6-ASTRA")
        self.assertEqual(self.profile(self.launch()),
                         ("gpt-6.1-sol", "high", {"model": "mapping", "effort": "mapping"}))

    def test_launch_config_and_per_field_explicit_precedence(self) -> None:
        self.set_profile("gpt-6-astra", "configured-worker", "low")
        cases = [
            ([], "configured-worker", "low", {"model": "config", "effort": "config"}),
            (["--model", "explicit-worker"], "explicit-worker", "low", {"model": "explicit", "effort": "config"}),
            (["--effort", "medium"], "configured-worker", "medium", {"model": "config", "effort": "explicit"}),
            (["--model", "explicit-worker", "--effort", "medium"], "explicit-worker", "medium", {"model": "explicit", "effort": "explicit"}),
        ]
        for args, model, effort, sources in cases:
            with self.subTest(args=args):
                self.assertEqual(self.profile(self.launch(*args)), (model, effort, sources))

    def test_resume_ignores_changed_and_malformed_config_and_retains_other_field(self) -> None:
        self.set_profile("gpt-6-astra", "original-worker", "low")
        receipt = self.launch()
        self.set_profile("gpt-6-astra", "new-worker", "high")
        self.fixture.parent([{"model": "gpt-6-sol", "effort": "ultra"}])
        result = self.fixture.access("resume", receipt["run_id"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.profile(receipt), ("original-worker", "low", {"model": "run", "effort": "run"}))
        (self.store / "worker_defaults.json").write_text("malformed")
        result = self.fixture.access("resume", receipt["run_id"], "--model", "resume-worker")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.profile(receipt), ("resume-worker", "low", {"model": "explicit", "effort": "run"}))

    def test_invalid_cli_input_does_not_change_prior_file(self) -> None:
        self.set_profile("main", "worker", "high")
        path = self.store / "worker_defaults.json"
        before = path.read_bytes()
        for args in [
            ["set", "--main", "main", "--model", "worker"],
            ["set", "--main", "main", "--model", "bad model", "--effort", "high"],
            ["set", "--main", "main", "--model", "worker", "--effort", "invalid"],
            ["set", "--main", "first", "--main", "second", "--model", "worker", "--effort", "high"],
            ["reset", "--main", "main", "--effort", "high"],
            ["show", "--main", "main"], ["show", "--store", ""], ["reset"],
        ]:
            with self.subTest(args=args):
                result = self.cli("config", *args)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(result.stderr)
                self.assertEqual(path.read_bytes(), before)

    def test_malformed_file_blocks_show_set_reset_and_new_launch_visibly(self) -> None:
        self.set_profile("main", "worker", "high")
        path = self.store / "worker_defaults.json"
        path.write_text('{"main":{"model":"worker"}}')
        before = path.read_bytes()
        for args in [
            ["config", "show"],
            ["config", "set", "--main", "other", "--model", "worker", "--effort", "low"],
            ["config", "reset", "--main", "main"],
            ["run", "--store", str(self.store), "--cwd", str(self.root), "--task", "defaults-check",
             "--completion-gate", "Fixture complete", "--model", "explicit", "--effort", "high"],
        ]:
            with self.subTest(args=args):
                result = self.cli(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("invalid worker defaults", result.stderr)
                self.assertIn(str(path), result.stderr)
                self.assertEqual(path.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
