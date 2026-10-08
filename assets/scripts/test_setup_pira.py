from __future__ import annotations

import importlib.util
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch
from pathlib import Path


SCRIPT = Path(__file__).with_name("setup_pira.py")
SPEC = importlib.util.spec_from_file_location("pira_setup_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)


class AutoRecapTests(unittest.TestCase):
    def setUp(self) -> None:
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        physical_home = str(Path(home.name).resolve())
        environment = patch.dict(setup.os.environ, {"HOME": physical_home, "LOCALAPPDATA": physical_home, "USERPROFILE": physical_home, "SHELL": "/bin/sh"}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        legacy_temp = patch.object(setup.stores.tempfile, "gettempdir", return_value=physical_home)
        legacy_temp.start()
        self.addCleanup(legacy_temp.stop)
        # Native file operations stay real; registry persistence uses a private test double.
        if setup.os.name == "nt":
            from unittest.mock import MagicMock
            values = {}
            registry = MagicMock()
            registry.REG_SZ, registry.REG_EXPAND_SZ = 1, 2
            def query(handle, key):
                if key not in values:
                    raise FileNotFoundError(key)
                return values[key], registry.REG_SZ
            registry.QueryValueEx.side_effect = query
            registry.SetValueEx.side_effect = lambda handle, key, reserved, kind, value: values.__setitem__(key, value)
            mocked = patch.dict(sys.modules, {"winreg": registry})
            mocked.start()
            self.addCleanup(mocked.stop)
            notification = patch.object(setup.stores, "notify_windows_environment")
            notification.start()
            self.addCleanup(notification.stop)

    def test_full_setup_prepares_missing_backend_before_retained_migration(self):
        import test_migrate_pira_stores as fixtures
        fixture = fixtures.MigrationTests()
        root = Path(setup.os.environ["HOME"])
        for failure in (None, "prepare", "migration"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(dir=root) as temporary:
                home = Path(temporary)
                source = home / "historical/run-id"
                fixture.put_team_run(source)
                original = setup.stores.migration.inventory(source)
                config = home / "config.toml"
                config.write_text('model = "preserved"\n', encoding="utf-8")
                install_dir = home / "tools"
                binary = setup.tool_setup.executable_path(install_dir / setup.tool_setup.CODEX_PACKAGE_DIR / "bin", "codex")
                helper = fixture.fake_team_backend()
                if failure == "migration":
                    helper.repair.side_effect = RuntimeError("injected migration failure")
                def install(*args):
                    if failure == "prepare": raise RuntimeError("injected preparation failure")
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b"inert managed fixture")
                    return binary
                with patch.dict(setup.os.environ, {"HOME": str(home), "USERPROFILE": str(home), "LOCALAPPDATA": str(home)}), \
                     patch.object(setup.tool_setup.shutil, "which", return_value=None), \
                     patch.object(setup.tool_setup, "install_codex", side_effect=install) as installer, \
                     patch.object(setup.tool_setup, "check_team_runtime", return_value="codex-cli 0.161.0") as check, \
                     patch.object(setup.stores.migration, "team_backend", return_value=helper), \
                     patch.object(setup.stores, "historical_store_paths", side_effect=lambda tool: [source.parent] if tool == "pira_team" else []), \
                     patch.object(setup, "ensure_agent_dir"), patch.object(setup, "ensure_user_md"), \
                     patch.object(setup, "remove_legacy_files"), patch.object(setup, "configure_audio"), \
                     patch.object(setup, "verify"), \
                     patch.object(setup, "configure_codex", wraps=setup.configure_codex) as publish, \
                     patch.object(setup, "configure_tools") as tools:
                    code = setup.main(["--agent-dir", str(home / "agent"), "--codex-config", str(config),
                                       "--tools-install-dir", str(install_dir), "--execution-mode", "keep"])
                    self.assertEqual(code, 1 if failure else 0)
                    installer.assert_called_once()
                    if failure:
                        if failure == "migration": helper.repair.assert_called_once()
                        publish.assert_not_called()
                        tools.assert_not_called()
                        self.assertEqual(config.read_text(), 'model = "preserved"\n')
                    else:
                        check.assert_called_once()
                        self.assertEqual(helper.inspect_source.call_args.kwargs['codex_binary'], str(binary.resolve()))
                        helper.repair.assert_called_once()
                        publish.assert_called_once()
                        tools.assert_called_once()
                        self.assertIn('PIRA_TEAM_DIR', config.read_text())
                    self.assertEqual(setup.stores.migration.inventory(source), original)

    def test_missing_backend_noninstalling_modes_never_prepare(self):
        root = Path(setup.os.environ["HOME"])
        for mode in (["--skip-tools"], ["--dry-run"], ["--verify"], ["--no-team"]):
            with self.subTest(mode=mode), \
                 patch.object(setup.tool_setup, "selected_codex_binary", return_value=None), \
                 patch.object(setup.tool_setup, "prepare_team_runtime") as prepare, \
                 patch.object(setup.stores, "plan_store_environment", side_effect=RuntimeError("fixture planning stop")), \
                 patch.object(setup, "configure_codex") as publish:
                self.assertEqual(setup.main(["--agent-dir", str(root / "agent"), "--codex-config", str(root / "config.toml"),
                                             "--execution-mode", "keep", *mode]), 1)
                prepare.assert_not_called()
                publish.assert_not_called()

    def test_managed_backend_outside_path_reaches_unified_migration_before_publication(self):
        root = Path(setup.os.environ["HOME"])
        install_dir = root / "custom-tools"
        binary = setup.tool_setup.executable_path(install_dir / setup.tool_setup.CODEX_PACKAGE_DIR / "bin", "codex")
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"fixture, never executed")
        config = root / "config.toml"
        config.write_text('model = "preserved"\n', encoding="utf-8")
        original = config.read_bytes()
        for flags in ([], ["--skip-tools"], ["--dry-run"], ["--verify"], ["--no-team"]):
            events = []
            def checked(*args):
                events.append("checked")
                self.assertEqual(args[1], str(binary.resolve()))
            def planned(tools, **kwargs):
                events.append("planned")
                self.assertEqual(kwargs["codex_text"], config.read_text(encoding="utf-8"))
                self.assertEqual(kwargs["codex_binary"], None if "--no-team" in flags else str(binary.resolve()))
                return setup.stores.StorePlan()
            with self.subTest(flags=flags), \
                 patch.object(setup.tool_setup.shutil, "which", return_value=None), \
                 patch.object(setup.tool_setup, "check_team_runtime", side_effect=checked), \
                 patch.object(setup.tool_setup, "install_codex") as install, \
                 patch.object(setup.stores, "plan_store_environment", side_effect=planned), \
                 patch.object(setup, "plan_codex_configuration", return_value=None), \
                 patch.object(setup.stores, "apply_store_migrations", side_effect=RuntimeError("fixture barrier")), \
                 patch.object(setup, "configure_codex") as publish, \
                 patch.object(setup, "configure_tools") as tools:
                self.assertEqual(setup.main(["--tools-install-dir", str(install_dir), "--codex-config", str(config), *flags]), 1)
                self.assertEqual(events, ["planned"] if "--no-team" in flags else ["checked", "planned"])
                install.assert_not_called()
                publish.assert_not_called()
                tools.assert_not_called()
                self.assertEqual(config.read_bytes(), original)

    def test_selected_backend_keyword_reaches_retained_team_preflight(self):
        root = Path(setup.os.environ["HOME"])
        source = root / "historical-team"
        source.mkdir()
        binary = root / "managed-codex"
        binary.write_bytes(b"not executed")
        with patch.object(setup.stores, "historical_store_paths", return_value=[source]), \
             patch.object(setup.stores.migration, "preflight_team_relocation", return_value=[]) as preflight:
            setup.stores.plan_store_environment(["pira_team"], profile_paths=[], codex_binary=str(binary))
        self.assertEqual(preflight.call_args.kwargs["codex_binary"], str(binary))
        self.assertEqual(preflight.call_args.args[0], [source])

    @unittest.skipIf(sys.platform == "win32", "Windows Ctx default did not relocate")
    def test_unified_migration_copies_before_config_switch_and_is_repeatable(self) -> None:
        root = Path(setup.os.environ["HOME"])
        (root / "AGENTS.md").write_text("# Fixture\n" + setup.VERIFY_TOKEN)
        (root / "USER.md").write_text("fixture")
        source = setup.stores.historical_store_paths("pira_ctx")[0]
        destination = Path(setup.stores.selected_store_paths(["pira_ctx"])["PIRA_CTX_STORE_DIR"])
        (source / "records").mkdir(parents=True)
        (source / "records/old.json").write_bytes(b'{"id":"old"}\n')
        (destination / "records").mkdir(parents=True)
        (destination / "records/new.json").write_bytes(b'{"id":"new"}\n')
        config = root / "config.toml"
        original = 'model = "custom"\n[shell_environment_policy.set]\nPIRA_CTX_STORE_DIR = ' + setup.toml_string(str(source)) + '\n'
        config.write_text(original)
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--no-team", "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--audio", "no", "--legacy", "keep"]
        with patch.object(setup, "ensure_agent_dir"):
            self.assertEqual(setup.main([*args, "--dry-run"]), 0)
            self.assertEqual(setup.main([*args, "--verify"]), 1)  # Missing migrated data is not success.
        self.assertEqual(config.read_text(), original)
        self.assertFalse((destination / "records/old.json").exists())
        configure = setup.configure_codex
        def publish(*positional, **keywords):
            self.assertEqual((destination / "records/old.json").read_bytes(), (source / "records/old.json").read_bytes())
            self.assertEqual((destination / "records/new.json").read_bytes(), b'{"id":"new"}\n')
            configure(*positional, **keywords)
        with patch.object(setup, "ensure_agent_dir"), patch.object(setup, "configure_codex", side_effect=publish):
            self.assertEqual(setup.main(args), 0)
            first = config.read_bytes()
            self.assertEqual(setup.main(args), 0)
            self.assertEqual(config.read_bytes(), first)
            self.assertEqual(setup.main([*args, "--verify"]), 0)
        self.assertEqual(tomllib.loads(first.decode())["shell_environment_policy"]["set"]["PIRA_CTX_STORE_DIR"], str(destination))
        self.assertEqual((source / "records/old.json").read_bytes(), b'{"id":"old"}\n')
        self.assertFalse((root / ".profile").exists())

    def test_migration_barrier_modes_selection_and_failure_precede_all_publication(self) -> None:
        root = Path(setup.os.environ["HOME"])
        config = root / "config.toml"
        original = 'model = "custom"\n'
        config.write_text(original)
        for enabled in (False, True):
            for mode in ([], ["--dry-run"], ["--verify"]):
                with self.subTest(enabled=enabled, mode=mode), \
                     patch.dict(setup.os.environ, {"PIRA_TEAM_DIR": str(root / "custom-team")}), \
                     patch.object(setup.stores, "plan_store_environment", wraps=setup.stores.plan_store_environment) as plan, \
                     patch.object(setup.stores, "apply_store_migrations", side_effect=RuntimeError("injected migration failure")) as barrier, \
                     patch.object(setup, "ensure_agent_dir") as agent, \
                     patch.object(setup, "configure_codex") as publish, \
                     patch.object(setup, "configure_tools") as tools:
                    args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                            "--execution-mode", "keep", "--audio", "no", *mode]
                    if not enabled:
                        args.append("--no-team")
                    self.assertEqual(setup.main(args), 1)
                    self.assertEqual(plan.call_args.args[0], ["pira_ctx", "pira_dec"] + (["pira_team"] if enabled else []))
                    self.assertEqual(plan.call_args.kwargs["codex_text"], original)
                    self.assertEqual(barrier.call_args.kwargs, {"dry_run": bool(mode), "verify": mode == ["--verify"]})
                    agent.assert_not_called()
                    publish.assert_not_called()
                    tools.assert_not_called()
                    self.assertEqual(config.read_text(), original)

    def test_skip_codex_does_not_read_its_config_before_tools_migration(self) -> None:
        root = Path(setup.os.environ["HOME"])
        config = root / "config.toml"
        config.write_bytes(b"\xff not UTF-8")
        with patch.object(setup.stores, "plan_store_environment", wraps=setup.stores.plan_store_environment) as plan, \
             patch.object(setup.stores, "apply_store_migrations", side_effect=RuntimeError("stop before install")), \
             patch.object(setup, "configure_codex") as publish, \
             patch.object(setup, "configure_tools") as tools:
            self.assertEqual(setup.main(["--agent-dir", str(root), "--codex-config", str(config),
                                         "--skip-codex", "--no-team", "--audio", "no"]), 1)
            self.assertIsNone(plan.call_args.kwargs["codex_text"])
            publish.assert_not_called()
            tools.assert_not_called()
        self.assertEqual(config.read_bytes(), b"\xff not UTF-8")

    def test_empty_team_setup_publishes_all_selected_stores(self) -> None:
        root = Path(setup.os.environ["HOME"])
        (root / "AGENTS.md").write_text("# Fixture\n" + setup.VERIFY_TOKEN)
        (root / "USER.md").write_text("fixture")
        config = root / "config.toml"
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--audio", "no", "--legacy", "keep"]
        # Never inspect the machine's old native Team runtime directory.
        with patch.object(setup, "ensure_agent_dir"), patch.object(setup.stores.tempfile, "gettempdir", return_value=str(root)):
            self.assertEqual(setup.main(args), 0)
        configured = tomllib.loads(config.read_text())["shell_environment_policy"]["set"]
        self.assertEqual(configured, setup.stores.selected_store_paths(["pira_ctx", "pira_dec", "pira_team"]))

    def test_store_setup_default_and_custom_codex_flow(self) -> None:
        import os
        root = Path(os.environ["HOME"])
        (root / "AGENTS.md").write_text("# Fixture\n" + setup.VERIFY_TOKEN)
        (root / "USER.md").write_text("fixture")
        physical = root / "stores"
        physical.mkdir()
        alias = root / "alias"
        alias.symlink_to(physical, target_is_directory=True)
        config = root / "codex" / "config.toml"
        config.parent.mkdir()
        config.write_text('model = "custom"\nshell_environment_policy = { inherit = "none", set = { PIRA_CTX_STORE_DIR = ' + setup.toml_string(str(alias / "ctx")) + ' } }\n')
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--no-team", "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--audio", "no", "--legacy", "keep"]
        with patch.object(setup, "ensure_agent_dir"):
            self.assertEqual(setup.main(args), 0)
            first = config.read_bytes()
            parsed = tomllib.loads(first.decode())
            self.assertEqual(parsed["model"], "custom")
            policy = parsed["shell_environment_policy"]
            self.assertEqual(policy["inherit"], "none")
            self.assertEqual(policy["set"]["PIRA_CTX_STORE_DIR"], str(physical.resolve() / "ctx"))
            self.assertIn("PIRA_DEC_STORE_DIR", policy["set"])
            self.assertEqual(setup.main(args), 0)
            self.assertEqual(config.read_bytes(), first)
            self.assertEqual(setup.main([*args, "--verify"]), 0)
            self.assertEqual(config.read_bytes(), first)
        self.assertFalse((physical / "ctx").exists())
        self.assertFalse((root / ".profile").exists())  # --skip-tools leaves shell persistence alone.

    def test_configuration_failure_precedes_every_destination_write(self) -> None:
        import os
        root = Path(os.environ["HOME"])
        config = root / "config.toml"
        for original in ('invalid = [\n',
                         'shell_environment_policy.set.PIRA_CTX_STORE_DIR = ""\n',
                         'features = { other = true }\n'):
            with self.subTest(original=original):
                config.write_text(original)
                with patch.object(setup, "ensure_agent_dir") as agent, \
                     patch.object(setup, "ensure_user_md") as user, \
                     patch.object(setup, "configure_tools") as tools:
                    self.assertEqual(setup.main(["--agent-dir", str(root / "agent"), "--codex-config", str(config),
                                                "--execution-mode", "keep", "--audio", "no"]), 1)
                    agent.assert_not_called()
                    user.assert_not_called()
                    tools.assert_not_called()
                self.assertEqual(config.read_text(), original)
                self.assertEqual(list(root.iterdir()), [config])

    def test_quoted_existing_permissions_are_not_combined_with_sandbox_mode(self) -> None:
        root = Path(setup.os.environ["HOME"])
        config = root / "config.toml"
        config.write_text('"default_permissions" = "custom"\n')
        state = setup.SetupState(root, root, False, True)
        setup.configure_codex(state, config, "safe", False)
        parsed = tomllib.loads(config.read_text())
        self.assertEqual(parsed["default_permissions"], "custom")
        self.assertNotIn("sandbox_mode", parsed)

    def test_python_requirement_fails_before_configuration_writes(self) -> None:
        import os
        root = Path(os.environ["HOME"])
        with patch.object(setup.sys, "version_info", (3, 10)), patch.object(setup, "ensure_agent_dir") as agent:
            self.assertEqual(setup.main(["--agent-dir", str(root / "agent"), "--codex-config", str(root / "config.toml")]), 1)
            agent.assert_not_called()
        self.assertEqual(list(root.iterdir()), [])

    def test_team_hint_preserves_settings_and_is_idempotent(self) -> None:
        cases = [
            '',
            'model = "example"\n[features]\nexample = true\n',
            '[features.multi_agent_v2] # custom\nother = true',
            '[features.multi_agent_v2]\nother = true\n[features]\nexample = true\n',
            'features.multi_agent_v2.other = true\n',
            '[features]\nmulti_agent_v2.multi_agent_mode_hint_text = "custom"\n',
            '["features"."multi_agent_v2"]\n"multi_agent_mode_hint_text" = "custom"\n',
            '"features.multi_agent_v2.multi_agent_mode_hint_text" = "unrelated"\n',
        ]
        for original in cases:
            with self.subTest(original=original):
                expected = tomllib.loads(original)
                expected.setdefault("features", {}).setdefault("multi_agent_v2", {})["multi_agent_mode_hint_text"] = ""
                result = setup.disable_multi_agent_hint(original)
                self.assertEqual(tomllib.loads(result), expected)
                self.assertEqual(setup.disable_multi_agent_hint(result), result)

    def test_unsupported_team_toml_fails_before_config_write(self) -> None:
        for original in ('features = { other = true }\n', 'prompt = """multiline\ntext"""\n'):
            with self.subTest(original=original), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                config = root / "config.toml"
                config.write_text(original)
                state = setup.SetupState(root, root, False, True)
                with self.assertRaises(RuntimeError):
                    setup.configure_codex(state, config, "keep", False)
                self.assertEqual(config.read_text(), original)
                self.assertFalse((root / "AGENTS.override.md").exists())

    def test_disabled_instructions_and_rerun_preserve_source_and_settings(self) -> None:
        # Reproduce a legacy Windows text locale even on UTF-8 development hosts.
        original_open = Path.open
        def locale_open(path, mode="r", buffering=-1, encoding=None, errors=None, newline=None):
            if "b" not in mode and encoding in (None, "locale"):
                encoding = "cp1252"
            return original_open(path, mode, buffering, encoding, errors, newline)
        locale = patch.object(Path, "open", new=locale_open)
        locale.start()
        self.addCleanup(locale.stop)
        source = (SCRIPT.parents[2] / "AGENTS.md").read_text(encoding="utf-8")
        self.assertIn("### `pira_team`:", source)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "AGENTS.md").write_text(source, encoding="utf-8")
            (root / "USER.md").write_text("fixture", encoding="utf-8")
            config = root / "codex" / "config.toml"
            config.parent.mkdir()
            config.write_text('model = "custom"\n[features]\nother = true\n', encoding="utf-8")
            global_agents = config.parent / "AGENTS.md"
            global_agents.symlink_to(root / "AGENTS.md")
            state = setup.SetupState(root, root, False, True, team_enabled=False)
            for enabled in (False, True, False, False, True):
                with self.subTest(enabled=enabled):
                    state.team_enabled = enabled
                    before = tomllib.loads(config.read_text(encoding="utf-8"))
                    setup.configure_codex(state, config, "keep", False)
                    parsed = tomllib.loads(config.read_text(encoding="utf-8"))
                    self.assertEqual(parsed["model"], "custom")
                    self.assertTrue(parsed["features"]["other"])
                    if enabled:
                        self.assertEqual(parsed["features"]["multi_agent_v2"]["multi_agent_mode_hint_text"], "")
                    else:
                        self.assertEqual(parsed["features"], before["features"])
                        policy = setup.instructions_path(state, config).read_text(encoding="utf-8")
                        self.assertNotIn("pira_team", policy)
                        for line in source.splitlines():
                            if "~/agent/modules/" in line or "~/agent/USER.md" in line:
                                self.assertIn(line, policy)
                    self.assertEqual((root / "AGENTS.md").read_text(encoding="utf-8"), source)
                    self.assertFalse(global_agents.is_symlink())
                    state.verification.clear()
                    setup.verify(state, config, False)
                    self.assertTrue(all(passed for _, passed, _ in state.verification))

    def test_disabled_preserves_custom_hint_and_unrelated_global_agents(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "AGENTS.md").write_text("# Fixture\n")
            config = root / "codex" / "config.toml"
            config.parent.mkdir()
            original = '[features.multi_agent_v2]\nmulti_agent_mode_hint_text = "user-owned"\n'
            config.write_text(original)
            global_agents = config.parent / "AGENTS.md"
            global_agents.write_text("User instructions")
            state = setup.SetupState(root, root, False, True, team_enabled=False)
            setup.configure_codex(state, config, "keep", False)
            self.assertEqual(tomllib.loads(config.read_text())["features"], tomllib.loads(original)["features"])
            self.assertEqual(global_agents.read_text(), "User instructions")

    def test_no_team_main_forwarding_dry_run_and_verify(self) -> None:
        for mode in ([], ["--dry-run"], ["--verify"]):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                with patch.object(setup, "ensure_agent_dir"), patch.object(setup, "ensure_user_md"), \
                     patch.object(setup, "remove_legacy_files"), patch.object(setup, "configure_audio"), \
                     patch.object(setup, "configure_codex") as configure, patch.object(setup, "verify"), \
                     patch.object(setup.subprocess, "run") as run:
                    self.assertEqual(setup.main(["--no-team", "--agent-dir", str(root), *mode]), 0)
                    command = run.call_args.args[0]
                    self.assertIn("--no-team", command)
                    for flag in mode:
                        self.assertIn(flag, command)
                    if "--verify" not in mode:
                        self.assertFalse(configure.call_args.args[0].team_enabled)

    def test_no_team_dry_run_writes_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "AGENTS.md").write_text("# Fixture\n")
            state = setup.SetupState(root, root, True, True, team_enabled=False)
            setup.configure_codex(state, root / "codex" / "config.toml", "keep", False)
            self.assertEqual(list(root.iterdir()), [root / "AGENTS.md"])

    def test_codex_login_mode_forwarded_to_tool_setup(self) -> None:
        for mode in ("auto", "browser", "device", "skip"):
            args = setup.build_parser().parse_args(["--codex-login", mode])
            state = setup.SetupState(repo_root=SCRIPT.parents[2], agent_dir=Path("/unused"),
                                     dry_run=False, yes=False)
            with patch.object(setup.subprocess, "run") as run:
                setup.configure_tools(state, None, None, verify_only=True, codex_login=args.codex_login)
                command = run.call_args.args[0]
                self.assertEqual(command[command.index("--codex-login") + 1], mode)
                self.assertIn("--verify", command)

    def test_default_preserves_settings_and_is_idempotent(self) -> None:
        import tomllib

        cases = [
            "",
            'model = "example"\n[features]\nexample = true\n',
            '[tui] # display\nnotifications = false\nauto_recap = true\n[features]\nexample = true\n',
            '[tui]\nnotifications = false',
            'tui.auto_recap = true\ntui.notifications = false\n',
        ]
        for original in cases:
            with self.subTest(original=original):
                expected = tomllib.loads(original)
                expected.setdefault("tui", {})["auto_recap"] = False
                result = setup.disable_auto_recap(original)
                self.assertEqual(tomllib.loads(result), expected)
                self.assertEqual(setup.disable_auto_recap(result), result)


if __name__ == "__main__":
    unittest.main()
