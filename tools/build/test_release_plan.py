from __future__ import annotations

import itertools
import json
import re
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.build import release_plan as plan


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/build-pira-tool-bundles.yml"
BUNDLE_WORKFLOW = WORKFLOW.with_name("build-pira-tool-bundle.yml")


def lockfile(version: str = "1.0.0", dependency: str = "2.0.0") -> str:
    return 'version = 4\n' + ''.join(
        f'[[package]]\nname = "{tool}"\nversion = "{version if tool == "pira_team" else "1.0.0"}"\n'
        for tool in plan.TOOLS
    ) + f'[[package]]\nname = "third-party"\nversion = "{dependency}"\nsource = "registry+fixture"\n'


class ReleasePlanTests(unittest.TestCase):
    def test_tool_local_source_tests_manifest_and_help(self) -> None:
        for tool in plan.TOOLS:
            for path in (f"tools/src/{tool}/lib.rs", f"tools/src/{tool}/help.md",
                         f"tools/src/{tool}/docs/help.md",
                         f"tools/crates/{tool}/Cargo.toml", f"tools/crates/{tool}/tests/a.rs"):
                with self.subTest(path=path):
                    self.assertEqual(plan.changed_tools([path]), {tool})

    def test_shared_and_uncertain_inputs_select_every_tool(self) -> None:
        for path in ("tools/Cargo.lock", "tools/Cargo.toml", "rust-toolchain.toml",
                     "tools/build/build_pira_nav_platform_bins.py", ".cargo/config.toml",
                     "assets/scripts/setup_pira.py", "README.md", "unknown/file",
                     "tools/src/shared.rs", "tools/src/pira_team-other/a.rs"):
            with self.subTest(path=path):
                self.assertEqual(plan.changed_tools(["tools/src/pira_team/lib.rs", path]), set(plan.TOOLS))

    def test_version_only_lock_bump_keeps_tool_first(self) -> None:
        versions = plan.version_only_lock_changes(lockfile(), lockfile("1.0.1"))
        self.assertEqual(versions, {"pira_team"})
        self.assertEqual(plan.changed_tools(["tools/Cargo.lock"], versions), {"pira_team"})

    def test_dependency_or_unrecognized_lock_change_selects_all(self) -> None:
        for after in (lockfile(dependency="2.0.1"), "not toml", 'package = []',
                      lockfile() + 'checksum = "changed"\n',
                      lockfile().replace('name = "pira_team"', 'name = "other"'),
                      lockfile() + '[[package]]\nname = "pira_team"\nversion = "1.1.0"\n'):
            with self.subTest(after=after):
                self.assertIsNone(plan.version_only_lock_changes(lockfile(), after))

    def test_all_partitions_cover_six_without_duplicate_active_builds(self) -> None:
        for flags in itertools.product((False, True), repeat=len(plan.TOOLS)):
            changed = {tool for tool, flag in zip(plan.TOOLS, flags) if flag}
            result = plan.make_plan(changed, "fixture")
            self.assertEqual(set(result["changed"]), changed)
            self.assertEqual(set(result["changed"]) | set(result["remaining"]), set(plan.TOOLS))
            self.assertFalse(set(result["changed"]) & set(result["remaining"]))
            for phase in ("changed", "remaining"):
                self.assertEqual(result[f"has_{phase}"], bool(result[phase]))
                rows = result[f"{phase}_matrix"]["include"]
                self.assertTrue(rows)  # Inert row for disabled phase, never invalid empty matrix.
                if result[f"has_{phase}"]:
                    self.assertEqual([row["tool"] for row in rows], result[phase])
        with self.assertRaises(ValueError):
            plan.make_plan({"not-a-tool"}, "fixture")

    def test_missing_or_invalid_baseline_does_not_run_git(self) -> None:
        with patch.object(plan.subprocess, "check_output") as git:
            for base in ("", "master", "-danger", "a" * 39, "a" * 41):
                self.assertEqual(plan.plan_repository(Path("."), base)["changed"], list(plan.TOOLS))
            git.assert_not_called()

    def test_local_diff_uses_no_renames_and_nul_delimiters(self) -> None:
        with patch.object(plan.subprocess, "check_output", side_effect=[
            b"", b"tools/src/pira_team/removed.rs\0tools/src/pira_nav/new.rs\0",
        ]) as git:
            result = plan.plan_repository(Path("fixture"), "a" * 40)
        self.assertEqual(set(result["changed"]), {"pira_team", "pira_nav"})
        self.assertEqual(git.call_args_list[1].args[0][-5:], ["--no-renames", "-z", "a" * 40, "HEAD", "--"])

    def test_empty_diff_and_lock_only_diff(self) -> None:
        for responses, expected in (([b"", b""], []),
                                    ([b"", b"tools/Cargo.lock\0", lockfile().encode(), lockfile("1.0.1").encode()], ["pira_team"])):
            with patch.object(plan.subprocess, "check_output", side_effect=responses):
                self.assertEqual(plan.plan_repository(Path("."), "b" * 40)["changed"], expected)

    def test_unavailable_nonancestor_or_undecodable_diff_rebuilds_all(self) -> None:
        for error in (subprocess.CalledProcessError(1, "git"), subprocess.TimeoutExpired("git", 30), OSError("fixture")):
            with patch.object(plan.subprocess, "check_output", side_effect=error):
                self.assertEqual(plan.plan_repository(Path("."), "a" * 40)["changed"], list(plan.TOOLS))
        with patch.object(plan.subprocess, "check_output", side_effect=[b"", b"\xff"]):
            self.assertEqual(plan.plan_repository(Path("."), "a" * 40)["changed"], list(plan.TOOLS))

    def test_cli_writes_single_line_outputs_without_commands(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "github-output"
            with patch("sys.argv", ["release_plan", "--github-output", str(output)]), patch("builtins.print"):
                plan.main()
            records = dict(line.split("=", 1) for line in output.read_text().splitlines())
            self.assertEqual(json.loads(records["has_changed"]), True)
            self.assertEqual(json.loads(records["has_remaining"]), False)
            self.assertEqual(len(json.loads(records["changed_matrix"])["include"]), 6)


class ReleaseMetadataTests(unittest.TestCase):
    def test_all_six_workspace_versions_match_lockfile(self) -> None:
        workspace = tomllib.loads((ROOT / "tools/Cargo.toml").read_text())
        members = workspace["workspace"]["members"]
        manifests = [tomllib.loads((ROOT / "tools" / member / "Cargo.toml").read_text())["package"]
                     for member in members]
        self.assertEqual({package["name"] for package in manifests}, set(plan.TOOLS))
        self.assertEqual(len(manifests), len(plan.TOOLS))
        locked = tomllib.loads((ROOT / "tools/Cargo.lock").read_text())["package"]
        for package in manifests:
            with self.subTest(tool=package["name"]):
                entries = [entry for entry in locked
                           if entry["name"] == package["name"] and "source" not in entry]
                self.assertEqual(len(entries), 1)
                self.assertEqual(entries[0]["version"], package["version"])


class WorkflowGateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = WORKFLOW.read_text()
        cls.bundle = BUNDLE_WORKFLOW.read_text()
        cls.jobs = dict(re.findall(r"^  ([a-z_-]+):\n(.*?)(?=^  [a-z_-]+:\n|\Z)", cls.source.split("jobs:\n", 1)[1], re.M | re.S))

    def allowed(self, job: str, changed: bool, remaining: bool, **results: str) -> bool:
        expression = re.search(r"    if: (.*?)(?=\n    \w)", self.jobs[job], re.S).group(1)
        expression = expression.removeprefix(">-\n").strip().removeprefix("${{").removesuffix("}}").strip()
        expression = expression.replace("always()", "True").replace("!cancelled()", str(results.pop("cancelled", False) is False))
        for phase, present in (("changed", changed), ("remaining", remaining)):
            expression = expression.replace(f"needs.authorize.outputs.has_{phase}", repr(str(present).lower()))
        defaults = {"authorize": "success", "build_changed": "success" if changed else "skipped",
                    "build_remaining": "success" if remaining else "skipped", "test-windows": "success"}
        defaults.update(results)
        for name, value in defaults.items():
            expression = expression.replace(f"needs.{name}.result", repr(value))
        expression = expression.replace("&&", " and ").replace("||", " or ")
        expression = " ".join(expression.split())  # YAML's folded scalar.
        self.assertNotIn("needs.", expression)
        return bool(eval(expression, {"__builtins__": {}}, {}))

    def test_actual_workflow_gates_all_partitions_failures_and_cancellation(self) -> None:
        for changed, remaining in itertools.product((False, True), repeat=2):
            if not (changed or remaining):
                continue
            self.assertEqual(self.allowed("build_changed", changed, remaining), changed)
            self.assertEqual(self.allowed("build_remaining", changed, remaining), remaining)
            self.assertTrue(self.allowed("test-windows", changed, remaining))
            self.assertTrue(self.allowed("publish", changed, remaining))
            for result in ("failure", "cancelled"):
                for job in ("build_remaining", "test-windows", "publish"):
                    self.assertFalse(self.allowed(job, changed, remaining, build_changed=result))
                self.assertFalse(self.allowed("publish", changed, remaining, build_remaining=result))
                self.assertFalse(self.allowed("publish", changed, remaining, **{"test-windows": result}))
            for job in ("build_remaining", "test-windows", "publish"):
                self.assertFalse(self.allowed(job, changed, remaining, authorize="failure"))
                self.assertFalse(self.allowed(job, changed, remaining, cancelled=True))
            if changed:
                self.assertFalse(self.allowed("publish", changed, remaining, build_changed="skipped"))
            if remaining:
                self.assertFalse(self.allowed("publish", changed, remaining, build_remaining="skipped"))

    def test_shared_builder_gates_and_single_full_windows_gate_are_retained(self) -> None:
        for job in ("build_changed", "build_remaining"):
            self.assertIn("uses: ./.github/workflows/build-pira-tool-bundle.yml", self.jobs[job])
            self.assertIn(f"fromJSON(needs.authorize.outputs.{job.removeprefix('build_')}_matrix)", self.jobs[job])
        self.assertIn("needs: [authorize, build_changed]", self.jobs["build_remaining"])
        self.assertIn("needs: [authorize, build_changed]", self.jobs["test-windows"])
        self.assertIn("needs: [authorize, build_changed, build_remaining, test-windows]", self.jobs["publish"])
        self.assertEqual(self.source.count("--workspace --locked -- --test-threads=1"), 1)
        for token in ("--bootstrap-rustup", "--bootstrap-zig", "Build every platform twice",
                      "cargo fmt", "cargo test", "TEAM_TEST_SLOW_PREFLIGHT=1",
                      "--skip unlinked_live_executable_never_becomes_missing_worker_or_exited_backend",
                      "shasum -a 256", "if-no-files-found: error"):
            self.assertIn(token, self.bundle)
        for workflow in (self.jobs["test-windows"], self.bundle):
            self.assertIn("tools.build.test_release_plan", workflow)
            self.assertIn("assets.scripts.test_setup_pira_communication", workflow)
            self.assertIn("assets.scripts.test_setup_pira_claude", workflow)
            self.assertIn("assets.scripts.test_python_bootstrap", workflow)
        self.assertIn("runs-on: ubuntu-latest", self.jobs["authorize"])
        self.assertIn("assets.scripts.test_setup_pira_claude", self.jobs["authorize"])
        self.assertIn("assets.scripts.test_python_bootstrap", self.jobs["authorize"])
        self.assertIn('pattern: "*-bundle"', self.jobs["publish"])
        self.assertIn("package_github_release.py", self.jobs["publish"])
        self.assertIn("Refusing to replace existing published release", self.jobs["publish"])
        self.assertIn('[[ "${PUBLISH_RELEASE}" == "true" ]]', self.jobs["publish"])


if __name__ == "__main__":
    unittest.main()
