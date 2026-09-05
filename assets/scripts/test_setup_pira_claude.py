from __future__ import annotations

import importlib.util
import io
import json
import shutil
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("setup_pira.py")
REPO_ROOT = SCRIPT.parents[2]
SPEC = importlib.util.spec_from_file_location("pira_setup_claude_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)

AGENTS_MD = (REPO_ROOT / "AGENTS.md").read_text(encoding="utf-8")
README_MD = (REPO_ROOT / "README.md").read_text(encoding="utf-8")


class SetupPiraClaudeTests(unittest.TestCase):
    def state(self, policy_dir: Path, *, dry_run: bool = False) -> Any:
        return setup.SetupState(
            repo_root=REPO_ROOT,
            policy_dir=policy_dir,
            dry_run=dry_run,
            yes=True,
            source_commit="a" * 40,
            source_branch="claude",
            source_dirty=False,
        )

    def install_bundle(self, state: Any) -> None:
        setup.install_policy_bundle(state)

    def test_managed_block_is_only_the_runtime_import(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            state = self.state(root / "pira")

            setup.update_claude_md(state, claude_md)
            installed = claude_md.read_text(encoding="utf-8")

            self.assertEqual(installed, setup.claude_managed_block(state.policy_dir))
            self.assertEqual(installed.splitlines()[1], "@" + setup.claude_import_path(state.policy_dir / "AGENTS.md"))
            self.assertEqual(len(installed.splitlines()), 3)

    def test_canonical_policy_is_claude_native_and_runtime_local(self) -> None:
        self.assertIn("Run every other shell command with native Bash", AGENTS_MD)
        self.assertIn("Load PIRA modules with Read, never with a shell command", AGENTS_MD)
        self.assertIn("~/.claude/pira/modules/CODING_STYLE.md", AGENTS_MD)
        self.assertNotIn("~/agent", AGENTS_MD)
        self.assertNotIn("Every shell/exec invocation", AGENTS_MD)
        self.assertNotIn("AGENTS.override.md", AGENTS_MD)

    def test_readme_defines_agent_managed_installation_and_all_layouts(self) -> None:
        self.assertIn("agent-managed operation", README_MD)
        self.assertIn("Install PIRA for Claude", README_MD)
        self.assertIn("PIRA + Codex only", README_MD)
        self.assertIn("PIRA + Claude only", README_MD)
        self.assertIn("Both clients", README_MD)
        self.assertIn("Separate `USER.md` files by default", README_MD)
        self.assertIn("one-time copy from Codex", README_MD)
        self.assertIn("compares the two SHA-256 hashes", README_MD)
        self.assertIn("must stop and ask rather than overwrite it", README_MD)

    def test_install_uninstall_preserves_user_bytes_exactly(self) -> None:
        contents = ["# Mine", "# Mine\n", "# Mine\n\n\n", "before\nafter\n"]
        for original in contents:
            with self.subTest(original=repr(original)), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                claude_md = root / "CLAUDE.md"
                claude_md.write_text(original, encoding="utf-8")
                state = self.state(root / "pira")

                setup.update_claude_md(state, claude_md)
                setup.remove_claude_md_block(state, claude_md)

                self.assertEqual(claude_md.read_text(encoding="utf-8"), original)

    def test_uninstall_absorbs_only_the_old_managed_final_newline(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            state = self.state(root / "pira")
            original = "# Mine\n\n\n"
            claude_md.write_text(original + setup.claude_managed_block(state.policy_dir) + "\n", encoding="utf-8")

            setup.remove_claude_md_block(state, claude_md)

            self.assertEqual(claude_md.read_text(encoding="utf-8"), original)

    def test_install_and_bundle_are_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            claude_md.write_text("# My instructions\n", encoding="utf-8")
            state = self.state(root / "pira")

            self.install_bundle(state)
            setup.update_claude_md(state, claude_md)
            first_claude = claude_md.read_bytes()
            first_manifest = (state.policy_dir / setup.MANIFEST_NAME).read_bytes()
            state.changed.clear()
            self.install_bundle(state)
            setup.update_claude_md(state, claude_md)

            self.assertEqual(claude_md.read_bytes(), first_claude)
            self.assertEqual((state.policy_dir / setup.MANIFEST_NAME).read_bytes(), first_manifest)
            self.assertEqual(state.changed, [])

    def test_replaces_stale_managed_block_in_place(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            stale = setup.CLAUDE_BLOCK_START + "\n@/old/AGENTS.md\nold override\n" + setup.CLAUDE_BLOCK_END
            claude_md.write_text("before\n" + stale + "\nafter\n", encoding="utf-8")
            state = self.state(root / "pira")

            setup.update_claude_md(state, claude_md)

            self.assertEqual(
                claude_md.read_text(encoding="utf-8"),
                "before\n" + setup.claude_managed_block(state.policy_dir) + "\nafter\n",
            )

    def test_rejects_bad_markers_and_non_utf8_without_changes(self) -> None:
        cases = [
            setup.CLAUDE_BLOCK_START,
            setup.CLAUDE_BLOCK_START + setup.CLAUDE_BLOCK_END + setup.CLAUDE_BLOCK_START,
            setup.CLAUDE_BLOCK_END + setup.CLAUDE_BLOCK_START,
        ]
        for content in cases:
            with self.subTest(content=content), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "CLAUDE.md"
                path.write_text(content, encoding="utf-8")
                state = self.state(Path(temporary) / "pira")
                with self.assertRaisesRegex(RuntimeError, "PIRA markers"):
                    setup.planned_claude_md(path, state.policy_dir)
                self.assertEqual(path.read_text(encoding="utf-8"), content)

        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "CLAUDE.md"
            path.write_bytes(b"\xff\xfe")
            with self.assertRaisesRegex(RuntimeError, "not valid UTF-8"):
                setup.planned_claude_md(path, Path(temporary) / "pira")
            self.assertEqual(path.read_bytes(), b"\xff\xfe")

    def test_bundle_contains_manifested_snapshot_and_renders_custom_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            policy_dir = Path(temporary) / "runtime"
            state = self.state(policy_dir)
            self.install_bundle(state)

            manifest = json.loads((policy_dir / setup.MANIFEST_NAME).read_text(encoding="utf-8"))
            paths = {entry["path"] for entry in manifest["files"]}
            installed_agents = (policy_dir / "AGENTS.md").read_text(encoding="utf-8")
            installed_maintenance = (policy_dir / "modules" / "MAINTENANCE.md").read_text(encoding="utf-8")

            self.assertEqual(manifest["source_commit"], "a" * 40)
            self.assertFalse(manifest["source_dirty"])
            self.assertIn("AGENTS.md", paths)
            self.assertIn("modules/CODING_STYLE.md", paths)
            self.assertIn(setup.claude_import_path(policy_dir), installed_agents)
            self.assertNotIn(setup.DEFAULT_POLICY_DIR, installed_agents)
            self.assertNotIn(setup.DEFAULT_POLICY_DIR, installed_maintenance)
            self.assertFalse((policy_dir / ".git").exists())

    def test_refuses_unowned_or_modified_policy_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            policy_dir = Path(temporary) / "pira"
            policy_dir.mkdir()
            (policy_dir / "AGENTS.md").write_text("mine", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "not owned"):
                setup.prepare_policy_bundle(self.state(policy_dir))

        with tempfile.TemporaryDirectory() as temporary:
            policy_dir = Path(temporary) / "pira"
            state = self.state(policy_dir)
            self.install_bundle(state)
            (policy_dir / "AGENTS.md").write_text("changed", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "modified managed"):
                setup.prepare_policy_bundle(state)

    def test_manifest_rejects_path_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            policy_dir = Path(temporary)
            payload = {
                "schema_version": setup.MANIFEST_SCHEMA,
                "target": "claude-code",
                "source_commit": "a" * 40,
                "source_branch": "claude",
                "source_dirty": False,
                "files": [{"path": "../outside", "sha256": "0" * 64}],
            }
            (policy_dir / setup.MANIFEST_NAME).write_text(json.dumps(payload), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "unsafe managed path"):
                setup.read_manifest(policy_dir)

    def test_uninstall_removes_only_owned_bundle_and_preserves_user_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            policy_dir = root / "pira"
            state = self.state(policy_dir)
            self.install_bundle(state)
            (policy_dir / "USER.md").write_text("private\n", encoding="utf-8")
            (policy_dir / "notes.txt").write_text("mine\n", encoding="utf-8")
            (policy_dir / "modules" / "PRIVATE.md").write_text("mine too\n", encoding="utf-8")
            manifest = setup.prepare_bundle_uninstall(state)

            setup.uninstall_policy_bundle(state, manifest)

            self.assertEqual((policy_dir / "USER.md").read_text(encoding="utf-8"), "private\n")
            self.assertEqual((policy_dir / "notes.txt").read_text(encoding="utf-8"), "mine\n")
            self.assertEqual((policy_dir / "modules" / "PRIVATE.md").read_text(encoding="utf-8"), "mine too\n")
            self.assertFalse((policy_dir / "AGENTS.md").exists())
            self.assertFalse((policy_dir / setup.MANIFEST_NAME).exists())

    def test_created_claude_file_is_deleted_on_uninstall(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            state = self.state(root / "pira")
            setup.update_claude_md(state, claude_md)
            setup.remove_claude_md_block(state, claude_md)
            self.assertFalse(claude_md.exists())
            self.assertEqual(len(list(root.glob("CLAUDE.md.bak.*"))), 1)

    def test_cli_installs_verifies_and_uninstalls_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / ".claude" / "CLAUDE.md"
            policy_dir = root / ".claude" / "pira"
            settings = root / ".claude" / "settings.json"
            common = ["--policy-dir", str(policy_dir), "--claude-md", str(claude_md), "--claude-settings", str(settings), "--skip-tools"]
            with redirect_stdout(io.StringIO()):
                installed = setup.main([*common, "--yes", "--user-mode", "placeholder"])
                hooks_after_install = json.loads(settings.read_text(encoding="utf-8"))["hooks"]
                verified = setup.main([*common, "--verify"])
                removed = setup.main([*common, "--uninstall"])

            self.assertEqual((installed, verified, removed), (0, 0, 0))
            self.assertEqual(set(hooks_after_install), set(setup.ROUTING_HOOK_EVENTS))
            self.assertFalse(settings.exists())
            self.assertFalse(claude_md.exists())
            self.assertTrue((policy_dir / "USER.md").exists())
            self.assertFalse((policy_dir / setup.MANIFEST_NAME).exists())

    def test_wrong_branch_and_whitespace_fail_before_first_write(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            policy_dir = root / "policy dir"
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                wrong_branch = setup.main(
                    ["--policy-dir", str(root / "pira"), "--claude-md", str(claude_md), "--skip-tools", "--expected-source-branch", "not-this-branch"]
                )
                whitespace = setup.main(
                    ["--policy-dir", str(policy_dir), "--claude-md", str(claude_md), "--skip-tools", "--yes"]
                )
            self.assertEqual((wrong_branch, whitespace), (1, 1))
            self.assertFalse(claude_md.exists())
            self.assertFalse((root / "pira").exists())
            self.assertFalse(policy_dir.exists())

    def test_audited_branch_install_rejects_dirty_source(self) -> None:
        with patch.object(
            setup,
            "command_output",
            side_effect=["a" * 40, "claude", " M AGENTS.md"],
        ):
            with self.assertRaisesRegex(RuntimeError, "uncommitted"):
                setup.source_metadata(REPO_ROOT, "claude")

    def test_codex_checkout_is_untouched_by_claude_install(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            codex = root / "agent"
            codex.mkdir()
            codex_policy = codex / "AGENTS.md"
            codex_policy.write_text("codex master\n", encoding="utf-8")
            state = self.state(root / ".claude" / "pira")

            self.install_bundle(state)

            self.assertEqual(codex_policy.read_text(encoding="utf-8"), "codex master\n")
            self.assertNotEqual((state.policy_dir / "AGENTS.md").read_text(encoding="utf-8"), "codex master\n")

    def test_installed_snapshot_survives_source_checkout_removal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            policy_dir = root / "runtime"
            (source / "modules").mkdir(parents=True)
            (source / "AGENTS.md").write_text(
                f"token {setup.VERIFY_TOKEN}\nread {setup.DEFAULT_POLICY_DIR}/modules/ONE.md\n",
                encoding="utf-8",
            )
            (source / "modules" / "ONE.md").write_text("module\n", encoding="utf-8")
            state = setup.SetupState(source, policy_dir, False, True, "b" * 40, "claude", False)

            with patch.object(setup, "tracked_policy_paths", return_value=["AGENTS.md", "modules/ONE.md"]):
                self.install_bundle(state)
            installed = (policy_dir / "AGENTS.md").read_bytes()
            shutil.rmtree(source)

            self.assertEqual((policy_dir / "AGENTS.md").read_bytes(), installed)
            self.assertEqual((policy_dir / "modules" / "ONE.md").read_text(encoding="utf-8"), "module\n")

    def test_snapshot_excludes_untracked_module_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            policy_dir = root / "runtime"
            (source / "modules").mkdir(parents=True)
            (source / "AGENTS.md").write_text(
                f"token {setup.VERIFY_TOKEN}\nread {setup.DEFAULT_POLICY_DIR}/modules/TRACKED.md\n",
                encoding="utf-8",
            )
            (source / "modules" / "TRACKED.md").write_text("tracked\n", encoding="utf-8")
            (source / "modules" / "LOCAL.md").write_text("private local file\n", encoding="utf-8")
            state = setup.SetupState(source, policy_dir, False, True, "b" * 40, "claude", False)

            with patch.object(setup, "tracked_policy_paths", return_value=["AGENTS.md", "modules/TRACKED.md"]):
                self.install_bundle(state)

            self.assertTrue((policy_dir / "modules" / "TRACKED.md").is_file())
            self.assertFalse((policy_dir / "modules" / "LOCAL.md").exists())

    def test_failed_replacement_restores_previous_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            policy_dir = Path(temporary) / "pira"
            old_state = self.state(policy_dir)
            self.install_bundle(old_state)
            old_agents = (policy_dir / "AGENTS.md").read_bytes()
            old_manifest = (policy_dir / setup.MANIFEST_NAME).read_bytes()
            new_state = setup.SetupState(REPO_ROOT, policy_dir, False, True, "c" * 40, "claude", False)
            real_replace = setup.os.replace
            calls = 0

            def fail_during_replace(source: object, target: object) -> None:
                nonlocal calls
                calls += 1
                if calls == 2:
                    raise OSError("injected replacement failure")
                real_replace(source, target)

            with patch.object(setup.os, "replace", side_effect=fail_during_replace):
                with self.assertRaisesRegex(RuntimeError, "previous files were restored"):
                    setup.install_policy_bundle(new_state)

            self.assertEqual((policy_dir / "AGENTS.md").read_bytes(), old_agents)
            self.assertEqual((policy_dir / setup.MANIFEST_NAME).read_bytes(), old_manifest)

    def test_prompt_uses_default_when_stdin_closes(self) -> None:
        with patch("builtins.input", side_effect=EOFError), redirect_stdout(io.StringIO()):
            self.assertTrue(setup.prompt_yes_no("Continue?", default=True))
            self.assertFalse(setup.prompt_yes_no("Continue?", default=False))

    def test_cli_rejects_removed_or_codex_only_options(self) -> None:
        options = [
            ["--agent-dir", "x"],
            ["--force-agent-link"],
            ["--legacy", "keep"],
            ["--execution-mode", "safe"],
            ["--audio", "yes"],
            ["--codex-config", "x"],
            ["--claude-code"],
        ]
        for extra_args in options:
            with self.subTest(extra_args=extra_args), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as raised:
                    setup.main(["--skip-tools", *extra_args])
                self.assertEqual(raised.exception.code, 2)


    def test_routing_hooks_are_added_once_and_preserve_other_settings(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            settings = Path(temporary) / "settings.json"
            user_hook = {"matcher": "Bash", "hooks": [{"type": "command", "command": "echo mine"}]}
            settings.write_text(json.dumps({"model": "opus", "hooks": {"PreToolUse": [user_hook], "UserPromptSubmit": [user_hook]}}), encoding="utf-8")
            state = self.state(Path(temporary) / "pira")

            setup.update_claude_settings(state, settings)
            setup.update_claude_settings(state, settings)
            data = json.loads(settings.read_text(encoding="utf-8"))

            self.assertEqual(data["model"], "opus")
            self.assertEqual(data["hooks"]["PreToolUse"], [user_hook])
            self.assertEqual(data["hooks"]["UserPromptSubmit"][0], user_hook)
            for event in setup.ROUTING_HOOK_EVENTS:
                groups = data["hooks"][event]
                self.assertEqual(sum(setup.is_pira_hook_group(group) for group in groups), 1)
            command = data["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            self.assertTrue(command.startswith("echo PIRA routing:"))
            self.assertIn("Read the exact PIRA module files", command)
            self.assertNotIn('"', command)

    def test_routing_hooks_uninstall_restores_other_settings_or_deletes_created_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = self.state(root / "pira")
            shared = root / "shared.json"
            original = {"permissions": {"allow": ["Read"]}, "hooks": {"Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "echo done"}]}]}}
            shared.write_text(json.dumps(original), encoding="utf-8")
            setup.update_claude_settings(state, shared)
            setup.remove_claude_settings_hooks(state, shared, setup.planned_settings_removal(shared))
            self.assertEqual(json.loads(shared.read_text(encoding="utf-8")), original)
            self.assertIsNone(setup.planned_settings_removal(shared))

            created = root / "created.json"
            setup.update_claude_settings(state, created)
            self.assertTrue(created.exists())
            setup.remove_claude_settings_hooks(state, created, setup.planned_settings_removal(created))
            self.assertFalse(created.exists())

    def test_routing_hooks_refuse_invalid_settings_without_writing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            settings = Path(temporary) / "settings.json"
            for content in ("{not json", "[]", json.dumps({"hooks": "text"})):
                settings.write_text(content, encoding="utf-8")
                with self.assertRaises(RuntimeError):
                    setup.planned_settings_install(settings)
                self.assertEqual(settings.read_text(encoding="utf-8"), content)

    def test_routing_hooks_dry_run_and_skip_flag_write_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            settings = root / "settings.json"
            state = self.state(root / "pira", dry_run=True)
            setup.update_claude_settings(state, settings)
            self.assertFalse(settings.exists())
            common = ["--policy-dir", str(root / "pira"), "--claude-md", str(root / "CLAUDE.md"), "--claude-settings", str(settings), "--skip-tools", "--skip-routing-hooks"]
            with redirect_stdout(io.StringIO()):
                self.assertEqual(setup.main([*common, "--yes", "--user-mode", "placeholder"]), 0)
            self.assertFalse(settings.exists())


if __name__ == "__main__":
    unittest.main()
