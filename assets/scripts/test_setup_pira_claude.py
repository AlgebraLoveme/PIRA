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
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.install = self.root / "tools"
        self.config = self.root / "profile"
        self.config.mkdir()
        self.native = self.root / ("claude.exe" if tools.IS_WINDOWS else "claude")
        self.native.write_bytes(b"\x7fELFsynthetic native fixture; never executed")
        for mocker in (
            patch.dict(os.environ, {"HOME": str(self.root), "USERPROFILE": str(self.root),
                                    "CLAUDE_CONFIG_DIR": str(self.config)}, clear=True),
            patch.object(tools.shutil, "which", return_value=str(self.native)),
            patch.object(tools, "ensure_path"),
        ):
            mocker.start()
            self.addCleanup(mocker.stop)
        execution = patch.object(tools.subprocess, "run", return_value=self.result("2.1.311 (Claude Code)"))
        self.run_cli = execution.start()
        self.addCleanup(execution.stop)
        network = patch.object(tools, "request_bytes", side_effect=AssertionError("unmocked installer request"))
        self.requests = network.start()
        self.addCleanup(network.stop)
        download = patch.object(tools, "request_to_path", side_effect=AssertionError("unmocked binary download"))
        self.download = download.start()
        self.addCleanup(download.stop)

    def result(self, stdout: str, *, stderr: str = "", code: int = 0):
        return subprocess.CompletedProcess([], code, stdout, stderr)

    def prepare(self, **kwargs):
        from setup_pira_claude_runtime import prepare_runtime
        return prepare_runtime(tools, self.install, config_dir=self.config,
                               verify=kwargs.get("verify", False), dry_run=kwargs.get("dry_run", False))

    def native_downloads(self, version="2.1.311"):
        import hashlib
        data = b"synthetic verified native distribution"
        platform = tools.load_selector().current_platform().replace("windows-", "win32-")
        name = "claude.exe" if tools.IS_WINDOWS else "claude"
        manifest = {"version": version, "platforms": {platform: {
            "binary": name, "checksum": hashlib.sha256(data).hexdigest(), "size": len(data)}}}
        self.requests.side_effect = [version.encode(), json.dumps(manifest).encode()]
        def download(url, path, **kwargs):
            self.assertEqual(kwargs["expected_hash"], hashlib.sha256(data).hexdigest())
            self.assertEqual(kwargs["expected_size"], len(data))
            path.write_bytes(data)
        self.download.side_effect = download
        return data

    def test_compatible_cli_is_unchanged_in_all_modes(self):
        for version in ("2.1.311 (Claude Code)", "2.2.0 (Claude Code)", "3.0.0"):
            for flags in ({}, {"verify": True}, {"dry_run": True}):
                with self.subTest(version=version, flags=flags):
                    self.run_cli.reset_mock()
                    self.run_cli.return_value = self.result(version)
                    self.assertEqual(self.prepare(**flags), version)
                    self.assertEqual([call.args[0] for call in self.run_cli.call_args_list],
                                     [[str(self.native), "--version"]])
                    self.assertEqual(self.run_cli.call_args.kwargs["env"]["CLAUDE_CONFIG_DIR"], str(self.config))
                    self.assertEqual(self.run_cli.call_args.kwargs["env"]["DISABLE_AUTOUPDATER"], "1")
        self.requests.assert_not_called()
        self.assertFalse(self.install.exists())

    def test_missing_cli_installs_checked_native_and_rerun_is_unchanged(self):
        data = self.native_downloads()
        with patch.object(tools.shutil, "which", return_value=None):
            self.assertEqual(self.prepare(), "2.1.311 (Claude Code)")
        binary = tools.executable_path(self.install / ".pira-claude" / "bin", "claude")
        self.assertEqual(binary.read_bytes(), data)
        requests = self.requests.call_count
        self.assertEqual(self.prepare(), "2.1.311 (Claude Code)")
        self.assertEqual(self.requests.call_count, requests)
        self.assertEqual(self.run_cli.call_args.args[0][0], str(binary))
        self.assertEqual(list(self.install.glob(".pira-claude.backup-*")), [])

    def test_old_native_updates_once_without_changing_channel_or_auth(self):
        settings = self.config / "settings.json"
        settings.write_text('{"autoUpdatesChannel":"stable","custom":"keep"}')
        auth = self.config / "synthetic-auth-fixture"
        auth.write_bytes(b"preserve account fixture")
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    self.result("native update done\n", stderr="native notice\n"),
                                    self.result("2.1.311 (Claude Code)")]
        output, error = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            self.assertEqual(self.prepare(), "2.1.311 (Claude Code)")
        self.assertIn("native update done", output.getvalue())
        self.assertIn("native notice", error.getvalue())
        self.assertEqual(settings.read_text(), '{"autoUpdatesChannel":"stable","custom":"keep"}')
        self.assertEqual(auth.read_bytes(), b"preserve account fixture")
        self.assertEqual([c.args[0][1] for c in self.run_cli.call_args_list], ["--version", "update", "--version"])
        self.requests.assert_not_called()

    def test_failed_or_noop_native_update_falls_back_to_managed(self):
        for failure in (self.result("native output", stderr="native denied", code=7),
                        self.result("already current\n")):
            with self.subTest(failure=failure.returncode), tempfile.TemporaryDirectory() as temporary:
                self.install = Path(temporary) / "tools"
                self.native_downloads()
                replies = [self.result("2.1.310 (Claude Code)"), failure]
                if not failure.returncode:
                    replies.append(self.result("2.1.310 (Claude Code)"))
                replies.extend([self.result("2.1.311 (Claude Code)")] * 2)
                self.run_cli.side_effect = replies
                self.assertEqual(self.prepare(), "2.1.311 (Claude Code)")
                self.assertEqual(self.native.read_bytes(), b"\x7fELFsynthetic native fixture; never executed")

    def test_npm_launcher_gets_managed_native_without_package_manager_update(self):
        npm = self.root / "node_modules" / "package" / "claude"
        npm.parent.mkdir(parents=True)
        npm.write_bytes(b"\x7fELFnpm managed native fixture")
        self.native_downloads()
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    self.result("2.1.311 (Claude Code)"), self.result("2.1.311 (Claude Code)")]
        with patch.object(tools.shutil, "which", return_value=str(npm)):
            self.assertEqual(self.prepare(), "2.1.311 (Claude Code)")
        self.assertNotIn("update", [c.args[0][1] for c in self.run_cli.call_args_list])
        self.assertEqual(npm.read_bytes(), b"\x7fELFnpm managed native fixture")

    def test_readonly_missing_old_and_malformed_never_mutate(self):
        for value in (None, "2.1.310 (Claude Code)", "bad version"):
            for verify in (False, True):
                with self.subTest(version=value, verify=verify):
                    self.run_cli.reset_mock()
                    self.run_cli.return_value = self.result(value or "")
                    with patch.object(tools.shutil, "which", return_value=str(self.native) if value else None):
                        if verify:
                            with self.assertRaises(RuntimeError):
                                self.prepare(verify=True, dry_run=True)
                        else:
                            self.assertIsNone(self.prepare(dry_run=True))
                    self.assertNotIn("update", [c.args[0][1] for c in self.run_cli.call_args_list])
                    self.assertFalse(self.install.exists())
        self.requests.assert_not_called()

    def test_bad_manifests_fail_before_binary_download(self):
        for record in ({"version":"wrong"}, {"version":"2.1.311", "platforms":[]},
                       {"version":"2.1.311", "platforms":{}}):
            with self.subTest(manifest=record), patch.object(tools.shutil, "which", return_value=None):
                self.requests.side_effect = [b"2.1.311", json.dumps(record).encode()]
                with self.assertRaisesRegex(RuntimeError, "Managed Claude preparation failed"):
                    self.prepare()
        self.download.assert_not_called()
        self.assertFalse(self.install.exists())

    def test_stable_channel_preserved_and_managed_update_keeps_old_package(self):
        settings = self.config / "settings.json"
        original = '{"autoUpdatesChannel":"stable","custom":"keep"}'
        settings.write_text(original)
        self.native_downloads()
        with patch.object(tools.shutil, "which", return_value=None):
            self.prepare()
        old = tools.executable_path(self.install / ".pira-claude" / "bin", "claude").read_bytes()
        self.native_downloads("2.1.312")
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"),
                                    self.result("2.1.312 (Claude Code)"), self.result("2.1.312 (Claude Code)")]
        self.assertEqual(self.prepare(), "2.1.312 (Claude Code)")
        backups = list(self.install.glob(".pira-claude.backup-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(tools.executable_path(backups[0] / "bin", "claude").read_bytes(), old)
        self.assertEqual(settings.read_text(), original)
        self.assertTrue(self.requests.call_args_list[0].args[0].endswith("/stable"))

    def test_downloaded_version_mismatch_preserves_previous_package(self):
        self.native_downloads()
        with patch.object(tools.shutil, "which", return_value=None):
            self.prepare()
        binary = tools.executable_path(self.install / ".pira-claude" / "bin", "claude")
        before = binary.read_bytes()
        self.native_downloads("2.1.312")
        self.run_cli.side_effect = [self.result("2.1.310 (Claude Code)"), self.result("2.1.311 (Claude Code)")]
        with self.assertRaisesRegex(RuntimeError, "does not match native manifest"):
            self.prepare()
        self.assertEqual(binary.read_bytes(), before)
        self.assertEqual(list(self.install.glob(".pira-claude.backup-*")), [])

    def test_disable_updates_and_unknown_selection_fail_without_changes(self):
        self.run_cli.return_value = self.result("2.1.310 (Claude Code)")
        with patch.dict(os.environ, {"DISABLE_UPDATES":"1"}), self.assertRaisesRegex(RuntimeError, "DISABLE_UPDATES"):
            self.prepare()
        self.requests.assert_not_called()
        marker = self.install / ".pira-claude" / "selected"
        marker.parent.mkdir(parents=True)
        marker.write_text("unknown")
        with self.assertRaisesRegex(RuntimeError, "Unrecognized managed Claude selection"):
            self.prepare()

    def test_failure_diagnostics_are_preserved_if_managed_download_also_fails(self):
        failures = [
            (self.result("native output", stderr="native error", code=3),
             ("exit 3", "native output", "native error")),
            (OSError("native loader failure"), ("native loader failure",)),
            (subprocess.TimeoutExpired("claude", 10, output=b"partial stdout", stderr=b"partial stderr"),
             ("partial stdout", "partial stderr")),
            (self.result("malformed version", stderr="version diagnostic"),
             ("Cannot parse", "version diagnostic")),
        ]
        for response, diagnostics in failures:
            with self.subTest(diagnostics=diagnostics):
                self.run_cli.reset_mock()
                self.run_cli.side_effect = response if isinstance(response, BaseException) else None
                self.run_cli.return_value = response
                self.requests.side_effect = RuntimeError("distribution unavailable")
                with self.assertRaises(RuntimeError) as raised:
                    self.prepare()
                for text in (*diagnostics, "distribution unavailable"):
                    self.assertIn(text, str(raised.exception))
                self.assertEqual(self.run_cli.call_count, 1)

    def test_full_setup_checks_only_selected_claude_before_writes_even_without_tools(self):
        selections = [[], ["--include", "Codex"], ["--include", "ClaudeCode"],
                      ["--include", "Codex", "ClaudeCode"], ["--harness", "claude"], ["--harness", "both"]]
        for selection in selections:
            selected = "ClaudeCode" in selection or "--harness" in selection
            for flags in ([], ["--verify"], ["--dry-run"]):
                with self.subTest(selection=selection, flags=flags), \
                     patch.object(tools, "prepare_codex_runtime", return_value=None), \
                     patch.object(tools, "prepare_claude_runtime", side_effect=RuntimeError("runtime boundary")) as gate, \
                     patch.object(setup, "ensure_agent_dir", side_effect=RuntimeError("write boundary")) as writes, \
                     patch.object(setup, "verify", side_effect=RuntimeError("verify boundary")), \
                     contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(setup.main([*selection, *flags, "--skip-codex", "--skip-tools",
                                                "--claude-dir", str(self.config),
                                                "--tools-install-dir", str(self.install)]), 1)
                    self.assertEqual(gate.call_count, int(selected))
                    if selected:
                        gate.assert_called_once_with(verify="--verify" in flags, dry_run="--dry-run" in flags,
                                                     config_dir=self.config, install_dir=self.install)
                        writes.assert_not_called()
        self.run_cli.assert_not_called()

    def test_tools_only_gate_selection_and_modes_before_release_access(self):
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
                        tools.main([*selection, *flags, "--no-path", "--install-dir", str(self.install)])
                    self.assertEqual(gate.call_count, int(selected))
                    if selected:
                        gate.assert_called_once_with(verify="--verify" in flags, dry_run="--dry-run" in flags,
                                                    install_dir=self.install)
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
        instructions.write_text("My existing instruction.\n", newline="")
        stores = {"PIRA_CTX_STORE_DIR": str(self.root / "ctx")}
        plan = claude.plan(self.directory, self.agent, stores, "keep")
        self.assertTrue(plan[instructions].startswith("My existing instruction.\n"))
        self.assertIn("agent\\ with\\ space/AGENTS.md", plan[instructions])
        config = json.loads(plan[path])
        self.assertEqual(config["model"], "custom-model")
        self.assertEqual(config["env"]["CUSTOM"], "keep")
        self.assertNotIn("permissions", config)
        for target, text in plan.items():
            target.write_text(text, newline="")
        self.assertEqual(claude.plan(self.directory, self.agent, stores, "keep"), plan)

    def test_migrates_only_generated_block_preserving_surrounding_custom_text(self) -> None:
        instructions = self.directory / "CLAUDE.md"
        instructions.write_text("Before\n" + self.legacy_block() + "After\n", newline="")
        plan = claude.plan(self.directory, self.agent, {}, "keep")
        expected = "Before\n" + claude.import_line(self.agent / "AGENTS.md") + "After\n"
        self.assertEqual(plan[instructions], expected)
        instructions.write_text(expected, newline="")
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
                runtime.assert_called_once_with(verify="--verify" in flags, dry_run="--dry-run" in flags,
                                                install_dir=self.root.resolve())
                self.assertIsNone(stores.call_args.kwargs["codex_binary"])


if __name__ == "__main__":
    unittest.main()
