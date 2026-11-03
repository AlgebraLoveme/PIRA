from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import setup_pira_claude as claude
import setup_pira_tools as tools
import setup_pira as setup


class ClaudeRuntimeTests(unittest.TestCase):
    def setUp(self) -> None:
        discovery = patch.object(tools.shutil, "which", return_value="/fixture/claude")
        discovery.start()
        self.addCleanup(discovery.stop)
        execution = patch.object(tools.subprocess, "run")
        self.run_cli = execution.start()
        self.addCleanup(execution.stop)

    def result(self, stdout: str, *, stderr: str = "", code: int = 0) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(["/fixture/claude"], code, stdout, stderr)

    def test_floor_and_newer_are_accepted_without_update_in_all_modes(self) -> None:
        for version in ("2.1.311 (Claude Code)", "2.1.312 (Claude Code)", "2.2.0 (Claude Code)", "3.0.0"):
            for verify, dry_run in ((False, False), (True, False), (False, True)):
                with self.subTest(version=version, verify=verify, dry_run=dry_run):
                    self.run_cli.reset_mock()
                    self.run_cli.return_value = self.result(version + "\n")
                    self.assertEqual(tools.prepare_claude_runtime(verify=verify, dry_run=dry_run), version)
                    self.assertEqual([call.args[0] for call in self.run_cli.call_args_list],
                                     [["/fixture/claude", "--version"]])
                    self.assertEqual(self.run_cli.call_args.kwargs["env"]["DISABLE_AUTOUPDATER"], "1")

    def test_old_native_cli_updates_once_and_rechecks_without_switching_channel(self) -> None:
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    self.result("native update done\n", stderr="native notice\n"),
                                    self.result("2.1.311 (Claude Code)")]
        with tempfile.TemporaryDirectory() as temporary:
            config_dir = Path(temporary)
            settings = config_dir / "settings.json"
            original = '{"autoUpdatesChannel":"stable","custom":"preserve"}'
            settings.write_text(original)
            output, error = io.StringIO(), io.StringIO()
            with patch.dict(os.environ, {"CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE": "1"}), \
                 contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
                self.assertEqual(tools.prepare_claude_runtime(verify=False, dry_run=False, config_dir=config_dir),
                                 "2.1.311 (Claude Code)")
                self.assertEqual(os.environ["CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE"], "1")
            self.assertEqual(settings.read_text(), original)
            self.assertIn("native update done", output.getvalue())
            self.assertIn("native notice", error.getvalue())
            self.assertEqual([call.args[0] for call in self.run_cli.call_args_list],
                             [["/fixture/claude", "--version"], ["/fixture/claude", "update"],
                              ["/fixture/claude", "--version"]])
            for call in self.run_cli.call_args_list:
                self.assertEqual(call.kwargs["env"]["CLAUDE_CONFIG_DIR"], str(config_dir))
                self.assertEqual(call.kwargs["env"]["CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE"], "0")
                self.assertEqual(call.kwargs["stdin"], subprocess.DEVNULL)

    def test_old_cli_readonly_modes_fail_without_update(self) -> None:
        self.run_cli.return_value = self.result("2.1.310 (Claude Code)")
        for verify, dry_run in ((True, False), (False, True), (True, True)):
            with self.subTest(verify=verify, dry_run=dry_run):
                self.run_cli.reset_mock()
                with self.assertRaisesRegex(RuntimeError, r"2\.1\.311.*found 2\.1\.310.*will not update"):
                    tools.prepare_claude_runtime(verify=verify, dry_run=dry_run)
                self.assertEqual([call.args[0] for call in self.run_cli.call_args_list],
                                 [["/fixture/claude", "--version"]])

    def test_update_success_that_stays_old_fails_with_manual_upgrade_guidance(self) -> None:
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    self.result("Claude is up to date!\n"),
                                    self.result("2.1.310 (Claude Code)")]
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(RuntimeError) as raised:
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        message = str(raised.exception)
        self.assertIn("still below required >= 2.1.311 after claude update", message)
        self.assertIn("Package-manager/custom-launcher", message)
        self.assertIn("channel", message)
        self.assertIn(tools.CLAUDE_INSTALL_GUIDE, message)
        self.assertEqual(self.run_cli.call_count, 3)

    def test_update_failure_preserves_native_stdout_and_stderr(self) -> None:
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    self.result("native updater output", stderr="native updater denied", code=7)]
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(RuntimeError) as raised:
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        for detail in ("update failed (exit 7)", "native updater output", "native updater denied"):
            self.assertIn(detail, str(raised.exception))
        self.assertEqual(self.run_cli.call_count, 2)

    def test_old_npm_and_script_launchers_require_manual_upgrade_without_update(self) -> None:
        launchers = ["/fixture/node_modules/@anthropic-ai/claude-code/claude",
                     "/fixture/claude.cmd", "/fixture/claude.ps1", "/fixture/cli.js"]
        for executable in launchers:
            with self.subTest(executable=executable), \
                 patch.object(tools.shutil, "which", return_value=executable):
                self.run_cli.reset_mock()
                self.run_cli.return_value = self.result("2.1.310 (Claude Code)")
                with self.assertRaisesRegex(RuntimeError, "Refusing automatic update.*npm/script launcher"):
                    tools.prepare_claude_runtime(verify=False, dry_run=False)
                self.assertEqual([call.args[0] for call in self.run_cli.call_args_list],
                                 [[executable, "--version"]])

    @unittest.skipUnless(os.name == "posix", "POSIX npm launcher symlink")
    def test_npm_launcher_symlink_is_resolved_before_update(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "node_modules" / "@anthropic-ai" / "claude-code" / "claude"
            binary.parent.mkdir(parents=True)
            binary.touch()
            launcher = root / "claude"
            launcher.symlink_to(binary)
            self.run_cli.return_value = self.result("2.1.310 (Claude Code)")
            with patch.object(tools.shutil, "which", return_value=str(launcher)), \
                 self.assertRaisesRegex(RuntimeError, "Refusing automatic update"):
                tools.prepare_claude_runtime(verify=False, dry_run=False)
            self.assertEqual(self.run_cli.call_count, 1)

    def test_missing_cli_gives_supported_install_guidance_without_execution(self) -> None:
        with patch.object(tools.shutil, "which", return_value=None), self.assertRaises(RuntimeError) as raised:
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        self.assertIn("Claude Code >= 2.1.311", str(raised.exception))
        self.assertIn(tools.CLAUDE_INSTALL_GUIDE, str(raised.exception))
        self.assertIn("native", str(raised.exception))
        self.run_cli.assert_not_called()

    def test_malformed_version_fails_without_attempting_update(self) -> None:
        for version in ("", "not a version", "2.1", "2.1.311 (Claude Code)\nextra output"):
            with self.subTest(version=version):
                self.run_cli.reset_mock()
                self.run_cli.return_value = self.result(version, stderr="version diagnostic")
                with self.assertRaisesRegex(RuntimeError, "Cannot parse Claude Code version") as raised:
                    tools.prepare_claude_runtime(verify=False, dry_run=False)
                self.assertIn("version diagnostic", str(raised.exception))
                self.assertEqual(self.run_cli.call_count, 1)

    def test_malformed_post_update_version_is_not_accepted(self) -> None:
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"), self.result("updated\n"),
                                    self.result("unrecognizable")]
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError, "Cannot parse"):
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        self.assertEqual(self.run_cli.call_count, 3)

    def test_version_command_failure_preserves_native_diagnostics(self) -> None:
        self.run_cli.return_value = self.result("native output", stderr="native error", code=3)
        with self.assertRaises(RuntimeError) as raised:
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        for detail in ("--version failed (exit 3)", "native output", "native error"):
            self.assertIn(detail, str(raised.exception))
        self.assertEqual(self.run_cli.call_count, 1)

    def test_unlaunchable_cli_fails_without_update(self) -> None:
        self.run_cli.side_effect = OSError("native loader failure")
        with self.assertRaisesRegex(RuntimeError, "native loader failure"):
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        self.assertEqual(self.run_cli.call_count, 1)

    def test_update_timeout_preserves_partial_native_diagnostics(self) -> None:
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    subprocess.TimeoutExpired(["claude", "update"], 300,
                                                              output=b"partial stdout", stderr=b"partial stderr")]
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(RuntimeError) as raised:
            tools.prepare_claude_runtime(verify=False, dry_run=False)
        self.assertIn("partial stdout", str(raised.exception))
        self.assertIn("partial stderr", str(raised.exception))
        self.assertEqual(self.run_cli.call_count, 2)

    def test_full_setup_checks_only_selected_claude_before_writes_even_without_tools(self) -> None:
        config_dir = Path(tempfile.gettempdir()).resolve() / "claude-gate-config"
        selections = [[], ["--include", "Codex"], ["--include", "ClaudeCode"],
                      ["--include", "Codex", "ClaudeCode"], ["--harness", "claude"], ["--harness", "both"]]
        for selection in selections:
            selected = "ClaudeCode" in selection or "--harness" in selection
            for flags in ([], ["--verify"], ["--dry-run"]):
                with self.subTest(selection=selection, flags=flags), \
                     patch.object(tools, "prepare_claude_runtime", side_effect=RuntimeError("runtime boundary")) as gate, \
                     patch.object(setup, "ensure_agent_dir", side_effect=RuntimeError("write boundary")) as writes, \
                     patch.object(setup, "verify", side_effect=RuntimeError("verify boundary")), \
                     contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(setup.main([*selection, *flags, "--skip-codex", "--skip-tools",
                                                 "--claude-dir", str(config_dir)]), 1)
                    self.assertEqual(gate.call_count, int(selected))
                    if selected:
                        gate.assert_called_once_with(verify="--verify" in flags, dry_run="--dry-run" in flags,
                                                     config_dir=config_dir)
                        writes.assert_not_called()
        self.run_cli.assert_not_called()

    def test_tools_only_gate_selection_and_modes_before_release_access(self) -> None:
        selections = [[], ["--include", "Codex"], ["--include", "ClaudeCode"],
                      ["--include", "Codex", "ClaudeCode"], ["--harness", "claude"], ["--harness", "both"]]
        for selection in selections:
            selected = "ClaudeCode" in selection or "--harness" in selection
            for flags in ([], ["--verify"], ["--dry-run"]):
                with self.subTest(selection=selection, flags=flags), \
                     patch.object(tools, "prepare_claude_runtime", return_value="2.1.311 (Claude Code)") as gate, \
                     patch.object(tools, "release_index", side_effect=RuntimeError("release boundary")), \
                     contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(RuntimeError, "release boundary"):
                        tools.main([*selection, *flags, "--no-path"])
                    self.assertEqual(gate.call_count, int(selected))
                    if selected:
                        gate.assert_called_once_with(verify="--verify" in flags, dry_run="--dry-run" in flags)
        self.run_cli.assert_not_called()


class ClaudeSetupTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.directory = self.root / "claude"
        self.directory.mkdir()
        self.agent = self.root / "agent with space"

    def test_fresh_plan_links_canonical_agents(self) -> None:
        plan = claude.plan(self.directory, self.agent, {}, "keep")
        self.assertEqual(plan[self.directory / "CLAUDE.md"], self.agent / "AGENTS.md")
        settings = json.loads(plan[self.directory / "settings.json"])
        self.assertTrue(settings["enabledPlugins"]["cc-plugin-agents-md@builtin"])
        self.assertEqual(settings["pluginConfigs"]["cc-plugin-agents-md@builtin"]["options"],
                         {"instructionFiles": "claude-md-and-agents-md"})
        self.assertEqual(list(self.directory.iterdir()), [])

    def legacy_block(self) -> str:
        return (f"{claude.START}\n@/previous/agent/AGENTS.md\n"
                "If the working directory has AGENTS.md and it is not already in context, read it before work.\n"
                f"{claude.END}\n")

    def test_preserves_custom_settings_and_reuses_one_policy_source(self) -> None:
        path = self.directory / "settings.json"
        path.write_text('{"model":"custom-model","env":{"CUSTOM":"keep"}}')
        instructions = self.directory / "CLAUDE.md"
        instructions.write_text("My existing instruction.\n")
        stores = {"PIRA_CTX_STORE_DIR": str(self.root / "ctx")}
        plan = claude.plan(self.directory, self.agent, stores, "keep")
        self.assertTrue(plan[instructions].startswith("My existing instruction.\n"))
        self.assertIn("agent\\ with\\ space/AGENTS.md", plan[instructions])
        config = json.loads(plan[path])
        self.assertEqual(config["model"], "custom-model")
        self.assertEqual(config["env"]["CUSTOM"], "keep")
        self.assertNotIn("permissions", config)
        for target, text in plan.items():
            target.write_text(text)
        self.assertEqual(claude.plan(self.directory, self.agent, stores, "keep"), plan)

    def test_migrates_only_generated_block_preserving_surrounding_custom_text(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        instructions.write_text("Before\n" + self.legacy_block() + "After\n")
        plan = claude.plan(self.directory, self.agent, {}, "keep")
        expected = "Before\n" + claude.import_line(self.agent / "AGENTS.md") + "After\n"
        self.assertEqual(plan[instructions], expected)
        instructions.write_text(expected)
        self.assertEqual(claude.plan(self.directory, self.agent, {}, "keep")[instructions], expected)

    def test_generated_only_block_becomes_canonical_link(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        instructions.write_text(self.legacy_block())
        self.assertEqual(claude.plan(self.directory, self.agent, {}, "keep")[instructions],
                         self.agent / "AGENTS.md")

    def test_customized_managed_block_is_not_discarded(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        cases = [self.legacy_block().replace(claude.END, "User-added rule\n" + claude.END),
                 self.legacy_block().replace("@/previous/agent/AGENTS.md", "@/previous/agent/AGENTS.md # User note")]
        for original in cases:
            with self.subTest(content=original):
                instructions.write_text(original)
                with self.assertRaisesRegex(RuntimeError, "Customized"):
                    claude.plan(self.directory, self.agent, {}, "keep")
                self.assertEqual(instructions.read_text(), original)

    @unittest.skipUnless(os.name == "posix", "POSIX link publication")
    def test_migration_publishes_link_with_one_backup_and_rerun_is_unchanged(self) -> None:
        self.agent.mkdir()
        target = self.agent / "AGENTS.md"
        target.write_text("canonical fixture")
        instructions = self.directory / "CLAUDE.md"
        original = self.legacy_block().encode()
        instructions.write_bytes(original)
        state = setup.SetupState(self.root, self.agent, False, True)
        setup.write_claude_link(state, instructions, target)
        self.assertTrue(instructions.is_symlink())
        self.assertEqual(instructions.resolve(), target.resolve())
        backups = list(self.directory.glob("*.bak.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), original)
        before = instructions.lstat().st_mtime_ns
        setup.write_claude_link(state, instructions, target)
        self.assertEqual(instructions.lstat().st_mtime_ns, before)
        self.assertEqual(list(self.directory.glob("*.bak.*")), backups)

    def test_symlink_failure_preserves_original_and_windows_uses_import(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        original = self.legacy_block().encode()
        instructions.write_bytes(original)
        target = self.agent / "AGENTS.md"
        state = setup.SetupState(self.root, self.agent, False, True)
        with patch.object(Path, "symlink_to", side_effect=OSError("fixture: links unavailable")):
            with patch.object(claude, "IS_WINDOWS", False):
                with self.assertRaisesRegex(OSError, "links unavailable"):
                    setup.write_claude_link(state, instructions, target)
                self.assertEqual(instructions.read_bytes(), original)
                self.assertFalse(list(self.directory.glob("*.bak.*")))
            with patch.object(claude, "IS_WINDOWS", True):
                setup.write_claude_link(state, instructions, target)
                self.assertEqual(instructions.read_text(), claude.import_line(target))
                self.assertTrue(claude.matches(instructions, target))
                before = instructions.stat().st_mtime_ns
                setup.write_claude_link(state, instructions, target)
                self.assertEqual(instructions.stat().st_mtime_ns, before)
        self.assertEqual(next(self.directory.glob("*.bak.*")).read_bytes(), original)

    def test_dry_run_leaves_legacy_file_and_creates_no_link_or_backup(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        original = self.legacy_block().encode()
        instructions.write_bytes(original)
        state = setup.SetupState(self.root, self.agent, True, True)
        setup.write_claude_link(state, instructions, self.agent / "AGENTS.md")
        self.assertFalse(instructions.is_symlink())
        self.assertEqual(instructions.read_bytes(), original)
        self.assertEqual(list(self.directory.iterdir()), [instructions])

    def test_custom_global_crlf_content_preserved_and_rerun_keeps_bytes(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        original = b"Custom global rule.\r\nDo not replace this rule.\r\n"
        instructions.write_bytes(original)
        planned = claude.plan(self.directory, self.agent, {}, "keep")[instructions]
        state = setup.SetupState(self.root, self.agent, False, True)
        setup.write_claude_text(state, instructions, planned)
        expected = original + claude.import_line(self.agent / "AGENTS.md").encode()
        self.assertEqual(instructions.read_bytes(), expected)
        backups = list(self.directory.glob("*.bak.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), original)
        repeated = claude.plan(self.directory, self.agent, {}, "keep")[instructions]
        setup.write_claude_text(state, instructions, repeated)
        self.assertEqual(instructions.read_bytes(), expected)
        self.assertEqual(list(self.directory.glob("*.bak.*")), backups)

    @unittest.skipUnless(os.name == "posix", "POSIX symlink fixture")
    def test_accepts_canonical_link_and_rejects_conflicting_or_dangling_links(self) -> None:
        self.agent.mkdir()
        target = self.agent / "AGENTS.md"
        target.write_text("canonical policy")
        instructions = self.directory / "CLAUDE.md"
        instructions.symlink_to(target)
        plan = claude.plan(self.directory, self.agent, {}, "keep")
        self.assertTrue(claude.matches(instructions, plan[instructions]))
        for other in (self.root / "unrelated.md", self.root / "missing.md"):
            with self.subTest(target=other.name):
                instructions.unlink()
                if other.name == "unrelated.md":
                    other.write_text("unrelated custom policy")
                instructions.symlink_to(other)
                with self.assertRaisesRegex(RuntimeError, "Conflicting.*symlink"):
                    claude.plan(self.directory, self.agent, {}, "keep")
                self.assertEqual(instructions.readlink(), other)

    def test_keep_preserves_permission_settings_and_json_formatting(self) -> None:
        path = self.directory / "settings.json"
        raw = ('{"env": {}, "permissions": {"defaultMode": "plan"}, "sandbox": {"enabled": true}, '
               '"enabledPlugins": {"cc-plugin-agents-md@builtin": true}, '
               '"pluginConfigs": {"cc-plugin-agents-md@builtin": '
               '{"options": {"instructionFiles": "claude-md-and-agents-md"}}}}')
        path.write_text(raw)
        plan = claude.plan(self.directory, self.agent, {}, "keep")
        self.assertEqual(plan[path], raw)

    def test_configures_agents_coexistence_without_changing_other_plugin_settings(self) -> None:
        path = self.directory / "settings.json"
        original = {
            "enabledPlugins": {"other@market": False, "cc-plugin-agents-md@builtin": False},
            "pluginConfigs": {
                "other@market": {"options": {"custom": "keep"}},
                "cc-plugin-agents-md@builtin": {"options": {"instructionFiles": "claude-md"}},
            },
        }
        path.write_text(json.dumps(original))
        planned = json.loads(claude.plan(self.directory, self.agent, {}, "keep")[path])
        self.assertFalse(planned["enabledPlugins"]["other@market"])
        self.assertEqual(planned["pluginConfigs"]["other@market"], original["pluginConfigs"]["other@market"])
        self.assertTrue(planned["enabledPlugins"]["cc-plugin-agents-md@builtin"])
        self.assertEqual(planned["pluginConfigs"]["cc-plugin-agents-md@builtin"]["options"]["instructionFiles"],
                         "claude-md-and-agents-md")

    def test_rejects_invalid_plugin_setting_shapes_without_writing(self) -> None:
        path = self.directory / "settings.json"
        cases = [
            {"enabledPlugins": []},
            {"pluginConfigs": []},
            {"pluginConfigs": {"cc-plugin-agents-md@builtin": []}},
            {"pluginConfigs": {"cc-plugin-agents-md@builtin": {"options": []}}},
        ]
        for value in cases:
            with self.subTest(value=value):
                original = json.dumps(value)
                path.write_text(original)
                with self.assertRaisesRegex(RuntimeError, "must be a JSON object"):
                    claude.plan(self.directory, self.agent, {}, "keep")
                self.assertEqual(path.read_text(), original)
                self.assertFalse((self.directory / "CLAUDE.md").exists())

    def test_explicit_full_access_and_unsupported_safe_mode(self) -> None:
        plan = claude.plan(self.directory, self.agent, {}, "soft-safe")
        config = json.loads(plan[self.directory / "settings.json"])
        self.assertEqual(config["permissions"]["defaultMode"], "bypassPermissions")
        self.assertFalse(config["sandbox"]["enabled"])
        with self.assertRaisesRegex(RuntimeError, "full-access"):
            claude.plan(self.directory, self.agent, {}, "safe")
        self.assertEqual(list(self.directory.iterdir()), [])

    def test_rejects_malformed_and_conflicting_state_without_writing(self) -> None:
        settings = self.directory / "settings.json"
        for raw in ["[]", '{"env":[]}', '{"env":{"PIRA_CTX_STORE_DIR":"/other"}}']:
            with self.subTest(raw=raw):
                settings.write_text(raw)
                with self.assertRaises(RuntimeError):
                    claude.plan(self.directory, self.agent, {"PIRA_CTX_STORE_DIR": str(self.root)}, "keep")
                self.assertEqual(settings.read_text(), raw)
        settings.write_text("{}")
        instructions = self.directory / "CLAUDE.md"
        for text, error in [
            (claude.START, "Malformed"),
            (claude.END + claude.START, "Reversed"),
            ((claude.START + claude.END) * 2, "Malformed"),
        ]:
            with self.subTest(text=text):
                instructions.write_text(text)
                with self.assertRaisesRegex(RuntimeError, error):
                    claude.plan(self.directory, self.agent, {}, "keep")
                self.assertEqual(instructions.read_text(), text)

    def test_claude_tools_setup_never_prepares_codex_or_logs_in(self) -> None:
        for flags in ([], ["--dry-run"], ["--verify"]):
            with self.subTest(flags=flags), \
                 patch.object(tools, "release_index", return_value={"tools": {"pira_team": {}}}), \
                 patch.object(tools, "prepare_claude_runtime") as runtime, \
                 patch.object(tools, "prepare_team_runtime") as prepare, \
                 patch.object(tools, "selected_codex_binary") as backend, \
                 patch.object(tools, "ensure_team_auth") as auth, \
                 patch.object(tools.stores, "plan_store_environment", return_value=tools.stores.StorePlan()) as stores, \
                 patch.object(tools, "tool_selection", side_effect=RuntimeError("fixture barrier")):
                with self.assertRaisesRegex(RuntimeError, "fixture barrier"):
                    tools.main(["--harness", "claude", "--install-dir", str(self.root), *flags])
                prepare.assert_not_called()
                backend.assert_not_called()
                auth.assert_not_called()
                runtime.assert_called_once_with(verify="--verify" in flags, dry_run="--dry-run" in flags)
                self.assertIsNone(stores.call_args.kwargs["codex_binary"])


if __name__ == "__main__":
    unittest.main()
