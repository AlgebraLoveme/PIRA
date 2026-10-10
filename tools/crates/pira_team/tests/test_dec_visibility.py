"""Real Dec CLI calls from main and fake Team worker shells; no model calls.

Run with PIRA_TEAM_BIN and PIRA_DEC_BIN pointing at freshly built binaries.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import unittest

import test_team


class DecVisibilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.env = self.fixture.env
        self.env.pop("PIRA_DEC_WORKSPACE_DIR", None)
        self.dec = str(Path(os.environ["PIRA_DEC_BIN"]).resolve())
        self.env["TEAM_DEC_BIN"] = self.dec
        self.repo = self.root / "repo"
        self.child = self.repo / "src" / "deep"
        self.child.mkdir(parents=True)
        (self.repo / ".git").mkdir()

    @staticmethod
    def add_args(context: str, maker: str = "agent") -> list[str]:
        return ["add", "--context", context, "--choice", "share workspace decisions",
                "--choice", "partition by thread", "--decision", "1", "--maker", maker]

    def main(self, args: list[str], cwd: Path | None = None) -> str:
        result = subprocess.run([self.dec, *args], cwd=cwd or self.repo, env=self.env,
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def search(self, cwd: Path | None = None, *extra: str) -> list[dict]:
        return json.loads(self.main(["search", "visibility", "--json", *extra], cwd))["matches"]

    def worker(self, commands: list[list[str] | dict], cwd: Path | None = None,
               run: str | None = None, caller: Path | None = None) -> tuple[dict, list[str]]:
        self.env["TEAM_DEC_COMMANDS"] = json.dumps(commands)
        args = [str(self.fixture.bin)]
        if run:
            args += ["resume", run]
        else:
            args += ["run", "--cwd", str(cwd or self.child),
                     "--model", "test-model", "--effort", "high"]
        args += ["--store", str(self.root / "logs"), "--completion-gate", "Probe Dec visibility",
                 "--task", "decision-probe"]
        result = subprocess.run(args, cwd=caller or self.repo, env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        outputs = json.loads(Path(receipt["handoff_path"]).read_text())
        self.assertEqual(len(outputs), len(commands))
        for request, output in zip(commands, outputs):
            command = request["args"] if isinstance(request, dict) else request
            if command[0] == "search" and output["returncode"] == 1:
                self.assertEqual(json.loads(output["stdout"])["matches"], [])
                self.assertEqual(output["stderr"], "")
            else:
                self.assertEqual(output["returncode"], 0, output["stderr"])
        return receipt, [output["stdout"] for output in outputs]

    def assert_ranked(self, rows: list[dict]) -> None:
        keys = [(r["timestamp_ms"], r["id"]) for r in rows]
        self.assertEqual(keys, sorted(keys, reverse=True))
        self.assertEqual(len(keys), len(set(keys)))

    def bidirectional(self) -> None:
        main_id = self.main(self.add_args("visibility main first")).split(" | ")[0]
        query = ["search", "visibility", "--json"]
        receipt, outputs = self.worker([query, self.add_args("visibility worker", "human"), query])
        self.assertEqual([r["id"] for r in json.loads(outputs[0])["matches"]], [main_id])
        worker_id = outputs[1].split(" | ")[0]
        rows = self.search()
        self.assertEqual(rows, json.loads(outputs[2])["matches"])
        self.assertEqual({r["id"] for r in rows}, {main_id, worker_id})
        self.assert_ranked(rows)
        self.assertEqual(json.loads(self.main(["show", worker_id, "--json"]))["maker"], "human")
        self.assertEqual(self.search(None, "--limit", "1"), rows[:1])

        # Resume retains original absolute store and worker cwd despite a new caller/store.
        main_later = self.main(self.add_args("visibility main later", "human")).split(" | ")[0]
        expected = self.search()
        original_store = self.env["PIRA_DEC_STORE_DIR"]
        self.env["PIRA_DEC_STORE_DIR"] = str(self.root / "other-store")
        self.env["PIRA_DEC_WORKSPACE_DIR"] = str(self.root)
        _, resumed = self.worker([query, self.add_args("visibility resumed worker"), query],
                                 run=receipt["run_id"], caller=self.root)
        self.assertEqual(json.loads(resumed[0])["matches"], expected)
        self.env["PIRA_DEC_STORE_DIR"] = original_store
        self.env.pop("PIRA_DEC_WORKSPACE_DIR")
        self.env["CODEX_THREAD_ID"] = "another-main-thread"
        rows = self.search()
        self.assertEqual(rows, json.loads(resumed[2])["matches"])
        self.assertEqual(len(rows), 4)
        self.assertIn(main_later, [r["id"] for r in rows])
        self.assert_ranked(rows)
        self.assertEqual(self.search(None, "--limit", "1"), rows[:1])
        records = list((self.repo / original_store if not Path(original_store).is_absolute()
                        else Path(original_store)).glob("*/records/*.piradec"))
        self.assertEqual(len(records), 4)
        self.assertFalse((self.root / "other-store").exists())

    def test_absolute_store_git_subdirectory_and_retained_resume(self) -> None:
        self.bidirectional()

    def test_relative_store_git_subdirectory_and_retained_resume(self) -> None:
        self.env["PIRA_DEC_STORE_DIR"] = "relative-dec"
        self.bidirectional()
        self.assertFalse((self.child / "relative-dec").exists())

    def test_non_git_descendants_and_retained_resume(self) -> None:
        (self.repo / ".git").rmdir()
        self.bidirectional()

    def test_non_git_relative_store_and_retained_resume(self) -> None:
        (self.repo / ".git").rmdir()
        self.env["PIRA_DEC_STORE_DIR"] = "relative-dec"
        self.bidirectional()
        self.assertFalse((self.child / "relative-dec").exists())

    def test_worker_directory_changes_obey_physical_and_git_boundaries(self) -> None:
        (self.repo / ".git").rmdir()
        main_id = self.main(self.add_args("visibility main")).split(" | ")[0]
        deeper = self.child / "deeper"
        nested = self.repo / "nested"
        outside = self.root / "outside"
        prefix = self.root / "repo-extra"
        for path in (deeper, nested, outside, prefix):
            path.mkdir()
        (nested / ".git").write_text("gitdir: external-worktree")
        boundaries = [nested, outside, prefix]
        if os.name == "posix":
            escape = self.repo / "escape"
            escape.symlink_to(outside, target_is_directory=True)
            boundaries.append(escape)
        query = ["search", "visibility", "--json"]
        commands = [{"cwd": str(deeper), "args": query},
                    {"cwd": str(deeper), "args": self.add_args("visibility descendant", "human")}]
        for path in boundaries:
            commands.append({"cwd": str(path), "args": query})
        _, outputs = self.worker(commands)
        self.assertEqual([r["id"] for r in json.loads(outputs[0])["matches"]], [main_id])
        for output in outputs[2:]:
            self.assertEqual(json.loads(output)["matches"], [])
        self.assertEqual(len(self.search()), 2)

    @unittest.skipUnless(os.name == "posix", "physical cwd alias fixture")
    def test_caller_symlink_is_persisted_as_physical_anchor(self) -> None:
        (self.repo / ".git").rmdir()
        main_id = self.main(self.add_args("visibility main")).split(" | ")[0]
        alias = self.root / "caller-alias"
        alias.symlink_to(self.repo, target_is_directory=True)
        receipt, outputs = self.worker([["search", "visibility", "--json"]], caller=alias)
        self.assertEqual(json.loads(outputs[0])["matches"][0]["id"], main_id)
        manifest = json.loads((Path(receipt["run_root"]) / "manifest.json").read_text())
        self.assertEqual(manifest["dec_workspace"], str(self.repo.resolve()))

    def test_legacy_resume_does_not_infer_or_inherit_new_anchor(self) -> None:
        (self.repo / ".git").rmdir()
        self.main(self.add_args("visibility main"))
        legacy_id = self.main(self.add_args("visibility legacy child"), self.child).split(" | ")[0]
        query = ["search", "visibility", "--json"]
        receipt, _ = self.worker([query])
        path = Path(receipt["run_root"]) / "manifest.json"
        manifest = json.loads(path.read_text())
        manifest.pop("dec_workspace")
        path.write_text(json.dumps(manifest))
        self.env["PIRA_DEC_WORKSPACE_DIR"] = str(self.repo)
        _, outputs = self.worker([query], run=receipt["run_id"], caller=self.root)
        self.assertEqual([r["id"] for r in json.loads(outputs[0])["matches"]], [legacy_id])
        self.assertIsNone(json.loads(path.read_text())["dec_workspace"])

    def test_nested_and_unrelated_workspaces_remain_isolated(self) -> None:
        self.main(self.add_args("visibility main"))
        sibling = self.root / "other-repo"
        sibling.mkdir()
        (sibling / ".git").mkdir()
        nested = self.repo / "nested"
        nested.mkdir()
        (nested / ".git").write_text("gitdir: external-worktree-metadata")
        plain = self.root / "non-git"
        plain.mkdir()
        other_plain = self.root / "another-non-git"
        other_plain.mkdir()
        for cwd in (sibling, nested, plain, other_plain):
            with self.subTest(cwd=cwd.name):
                _, outputs = self.worker([["search", "visibility", "--json"],
                                          self.add_args("visibility " + cwd.name)], cwd=cwd)
                self.assertEqual(json.loads(outputs[0])["matches"], [])
                self.assertEqual(len(self.search()), 1)
                self.assertEqual(len(self.search(cwd)), 1)

    def test_explicit_cli_store_precedence_and_store_isolation(self) -> None:
        self.main(self.add_args("visibility default"))
        alternate = str(self.root / "alternate-dec")
        _, outputs = self.worker([
            ["search", "visibility", "--json", "--store-dir", alternate],
            [*self.add_args("visibility explicit"), "--store-dir", alternate],
            ["search", "visibility", "--json", "--store-dir", alternate]])
        self.assertEqual(json.loads(outputs[0])["matches"], [])
        self.assertEqual(len(self.search()), 1)
        self.assertEqual(self.search(None, "--store-dir", alternate), json.loads(outputs[2])["matches"])


if __name__ == "__main__":
    unittest.main()
