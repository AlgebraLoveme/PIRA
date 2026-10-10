from __future__ import annotations

import importlib.util
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch
from pathlib import Path
from test_migrate_pira_stores import native_ctx_environment


import os
NATIVE_CTX_BINARY = os.environ.get("PIRA_TEST_CTX_BINARY")

SCRIPT = Path(__file__).with_name("setup_pira.py")
sys.path.insert(0, str(SCRIPT.parent.resolve()))
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
        environment = patch.dict(setup.os.environ, {**native_ctx_environment(Path(physical_home)), "SHELL": "/bin/sh"}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        profiles = patch.object(setup.audio_retirement, "default_profiles", return_value=[Path(physical_home) / ".zshrc"])
        profiles.start()
        self.addCleanup(profiles.stop)
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

    def test_setup_environment_preserves_only_execution_inputs(self):
        execution = {key: "fixture-" + key for key in
                     ("PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "SystemDrive")}
        with patch.dict(os.environ, {**execution, "HOME": "do-not-inherit",
                                    "OPENAI_API_KEY": "synthetic-do-not-inherit",
                                    "CODEX_HOME": "do-not-inherit"}, clear=True):
            fixture = AutoRecapTests()
            try:
                fixture.setUp()
                for key, value in execution.items():
                    self.assertEqual(os.environ[key], value)
                home = os.environ["HOME"]
                self.assertNotEqual(home, "do-not-inherit")
                for key in ("USERPROFILE", "LOCALAPPDATA", "APPDATA", "TMPDIR", "TMP", "TEMP"):
                    self.assertEqual(os.environ[key], home)
                for key in ("OPENAI_API_KEY", "CODEX_HOME"):
                    self.assertNotIn(key, os.environ)
            finally:
                fixture.doCleanups()

    def test_keep_optional_files_setup_and_verify(self):
        root = Path(setup.os.environ["HOME"])
        (root / "AGENTS.md").write_text(setup.VERIFY_TOKEN)
        legacy = root / "legacy"
        legacy.write_text("preserved")
        args = ["--agent-dir", str(root), "--skip-codex", "--skip-tools",
                "--user-mode", "keep", "--legacy", "keep"]
        with patch.object(setup, "ensure_agent_dir"), patch.object(setup, "parse_legacy_paths", return_value=[legacy]):
            for mode in ([], ["--verify"]):
                self.assertEqual(setup.main([*args, *mode]), 0)
                self.assertFalse((root / "USER.md").exists())
                self.assertEqual(legacy.read_text(), "preserved")
            state = setup.SetupState(root, root, True, True)
            setup.verify(state, root / "config.toml", True)
            self.assertEqual([name for name, passed, _ in state.verification if not passed],
                             ["USER.md exists", "legacy files absent"])

    def test_guard_conflicts_precede_configuration_and_migration(self):
        root = Path(setup.os.environ["HOME"])
        config = root / "config.toml"
        config.write_text('model = "custom"\n')
        policy = root / "AGENTS.md"
        policy.write_text("canonical fixture")
        guard = root / "AGENTS.override.md"
        for case in ("custom", "modified", "symlink", "hardlink", "directory"):
            with self.subTest(case=case):
                if guard.is_dir() and not guard.is_symlink(): guard.rmdir()
                elif guard.exists() or guard.is_symlink(): guard.unlink()
                if case == "symlink": guard.symlink_to(policy)
                elif case == "hardlink": setup.os.link(policy, guard)
                elif case == "directory": guard.mkdir()
                else: guard.write_text("custom policy" if case == "custom" else setup.PROJECT_AGENTS_GUARD + "custom policy\n")
                with patch.object(setup.stores, "apply_store_migrations") as migrate:
                    state = setup.SetupState(root, root, False, True)
                    with self.assertRaisesRegex(RuntimeError, "guard"):
                        setup.configure_codex(state, config, "keep", False)
                    migrate.assert_not_called()
                self.assertEqual(config.read_text(), 'model = "custom"\n')
                self.assertEqual(policy.read_text(), "canonical fixture")
                if guard.is_dir() and not guard.is_symlink(): guard.rmdir()
                else: guard.unlink()

    def test_explicit_force_link_preserves_old_custom_guard(self):
        root = Path(setup.os.environ["HOME"])
        repo = root / "repo"
        repo.mkdir()
        (repo / "AGENTS.md").write_text(setup.VERIFY_TOKEN)
        old = root / "agent"
        old.mkdir()
        (old / "AGENTS.override.md").write_text("old custom guard")
        with patch.object(setup, "__file__", str(repo / "assets/scripts/setup_pira.py")), \
             patch.object(setup, "migration_codex_binary", return_value=None):
            self.assertEqual(setup.main(["--agent-dir", str(old), "--force-agent-link", "--skip-tools",
                                         "--codex-config", str(root / "config.toml"), "--execution-mode", "keep",
                                         "--user-mode", "keep", "--legacy", "keep"]), 0)
        backup = next(root.glob("agent.bak.*"))
        self.assertEqual((backup / "AGENTS.override.md").read_text(), "old custom guard")
        self.assertEqual(old.resolve(), repo)
        self.assertEqual((repo / "AGENTS.override.md").read_text(), setup.PROJECT_AGENTS_GUARD)

    def test_known_guard_upgrade_backup_and_rerun(self):
        root = Path(setup.os.environ["HOME"])
        guard = root / "AGENTS.override.md"
        # Exact short template shipped before the sandbox paragraph.
        old = setup.PROJECT_AGENTS_GUARD.split("\nCreating a sandbox", 1)[0]
        guard.write_text(old)
        state = setup.SetupState(root, root, False, True)
        setup.ensure_project_agents_guard(state, root / "config.toml")
        self.assertEqual(guard.read_text(), setup.PROJECT_AGENTS_GUARD)
        backups = list(root.glob("AGENTS.override.md.bak.*"))
        self.assertEqual([p.read_text() for p in backups], [old])
        setup.ensure_project_agents_guard(state, root / "config.toml")
        self.assertEqual(list(root.glob("AGENTS.override.md.bak.*")), backups)

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
                     patch.object(setup, "remove_legacy_files"), \
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
        for mode in (["--skip-tools"], ["--dry-run"], ["--verify"]):
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
        for flags in ([], ["--skip-tools"], ["--dry-run"], ["--verify"]):
            events = []
            def checked(*args):
                events.append("checked")
                self.assertEqual(args[1], str(binary.resolve()))
            def planned(tools, **kwargs):
                events.append("planned")
                self.assertEqual(kwargs["codex_text"], config.read_text(encoding="utf-8"))
                self.assertEqual(kwargs["codex_binary"], str(binary.resolve()))
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
                self.assertEqual(events, ["checked", "planned"])
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
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--legacy", "keep"]
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
        for enabled in (True,):
            for mode in ([], ["--dry-run"], ["--verify"]):
                with self.subTest(enabled=enabled, mode=mode), \
                     patch.dict(setup.os.environ, {"PIRA_TEAM_DIR": str(root / "custom-team")}), \
                     patch.object(setup.stores, "plan_store_environment", wraps=setup.stores.plan_store_environment) as plan, \
                     patch.object(setup.stores, "apply_store_migrations", side_effect=RuntimeError("injected migration failure")) as barrier, \
                     patch.object(setup, "ensure_agent_dir") as agent, \
                     patch.object(setup, "configure_codex") as publish, \
                     patch.object(setup, "configure_tools") as tools:
                    args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                            "--execution-mode", "keep", *mode]
                    self.assertEqual(setup.main(args), 1)
                    self.assertEqual(plan.call_args.args[0], ["pira_ctx", "pira_dec", "pira_team"])
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
        with patch.object(setup, "migration_codex_binary", return_value=None) as backend, \
             patch.object(setup.stores, "plan_store_environment", wraps=setup.stores.plan_store_environment) as plan, \
             patch.object(setup.stores, "apply_store_migrations", side_effect=RuntimeError("stop before install")), \
             patch.object(setup, "configure_codex") as publish, \
             patch.object(setup, "configure_tools") as tools:
            self.assertEqual(setup.main(["--agent-dir", str(root), "--codex-config", str(config),
                                         "--skip-codex"]), 1)
            self.assertIsNone(plan.call_args.kwargs["codex_text"])
            backend.assert_called_once()
            publish.assert_not_called()
            tools.assert_not_called()
        self.assertEqual(config.read_bytes(), b"\xff not UTF-8")

    def test_empty_team_setup_publishes_all_selected_stores(self) -> None:
        root = Path(setup.os.environ["HOME"])
        (root / "AGENTS.md").write_text("# Fixture\n" + setup.VERIFY_TOKEN)
        (root / "USER.md").write_text("fixture")
        config = root / "config.toml"
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--legacy", "keep"]
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
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--legacy", "keep"]
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
                                                "--execution-mode", "keep"]), 1)
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

    def test_setup_retires_audio_and_does_not_republish_stale_plan(self):
        from test_retire_pira_audio import historical_config, PLAY
        root = Path(setup.os.environ["HOME"])
        (root / "AGENTS.md").write_text(setup.VERIFY_TOKEN)
        config = root / "config.toml"
        hooks = root / "hooks"
        hooks.mkdir()
        helper = hooks / "pira_play_audio.ps1"
        helper.write_text(PLAY)
        original = historical_config(hooks)
        config.write_text(original)
        args = ["--agent-dir", str(root), "--codex-config", str(config), "--skip-tools",
                "--execution-mode", "keep", "--user-mode", "keep", "--legacy", "keep"]
        with patch.object(setup, "ensure_agent_dir"):
            self.assertEqual(setup.main([*args, "--dry-run"]), 0)
            self.assertEqual(config.read_text(), original)
            self.assertTrue(helper.exists())
            self.assertEqual(setup.main([*args, "--verify"]), 1)
            self.assertEqual(config.read_text(), original)
            self.assertEqual(setup.main(args), 0)
            parsed = tomllib.loads(config.read_text())
            self.assertNotIn("notify", parsed)
            self.assertTrue(parsed["features"]["hooks"])
            self.assertEqual(parsed["hooks"]["PermissionRequest"][0]["matcher"], "user")
            self.assertEqual(parsed["features"]["multi_agent_v2"]["multi_agent_mode_hint_text"], "")
            self.assertFalse(helper.exists())
            first = config.read_bytes()
            self.assertEqual(setup.main(args), 0)
            self.assertEqual(setup.main([*args, "--verify"]), 0)
            self.assertEqual(config.read_bytes(), first)

    def test_audio_conflict_precedes_migration_and_backend_preparation(self):
        root = Path(setup.os.environ["HOME"])
        config = root / "config.toml"
        config.write_text(setup.audio_retirement.START + "\nnotify = [\"custom\"]\n" + setup.audio_retirement.END)
        with patch.object(setup.stores, "apply_store_migrations") as migrate, \
             patch.object(setup.tool_setup, "prepare_team_runtime") as prepare:
            self.assertEqual(setup.main(["--agent-dir", str(root), "--codex-config", str(config)]), 1)
            migrate.assert_not_called()
            prepare.assert_not_called()

    @unittest.skipUnless(NATIVE_CTX_BINARY, "isolated native Ctx fixture requires explicit binary")
    def test_fresh_ctx_capture_blocks_full_setup_retained_migration(self):
        import subprocess
        root = Path(setup.os.environ["HOME"])
        binary = NATIVE_CTX_BINARY
        env = native_ctx_environment(root)
        old, destination = root / "old", root / "new"
        env["PIRA_CTX_STORE_DIR"] = str(old)
        created = subprocess.run([binary, "capture", "--intent", "create isolated old record", "--", sys.executable, "-c", "print('fixture')"], env=env, capture_output=True, text=True)
        self.assertEqual(created.returncode, 0, created.stderr)
        self.assertTrue(list(old.rglob("*.piractx")), created.stdout + created.stderr)
        agent = root / "agent"
        agent.mkdir()
        (agent / "AGENTS.md").write_text(setup.VERIFY_TOKEN)
        config = root / "config.toml"
        config.write_text('model = "custom"\n')
        code = ("import sys; from pathlib import Path; from unittest.mock import patch, MagicMock; "
                + "sys.path.insert(0," + repr(str(SCRIPT.parent.resolve())) + "); import setup_pira as s; "
                + "registry=MagicMock(); registry.OpenKey.side_effect=FileNotFoundError; "
                + "winreg=patch.dict(sys.modules,{'winreg':registry}); winreg.start(); "
                + "notify=patch.object(s.stores,'notify_windows_environment'); notify.start(); "
                + "historical=lambda tool: [Path(" + repr(str(old)) + ")] if tool=='pira_ctx' else []; "
                + "ctx=patch.object(s.stores,'historical_store_paths',side_effect=historical); ctx.start(); "
                + "backend=patch.object(s,'migration_codex_binary',return_value=None); backend.start(); "
                + "link=patch.object(s,'ensure_agent_dir'); link.start(); "
                + "paths=" + repr({"PIRA_CTX_STORE_DIR": str(destination), "PIRA_DEC_STORE_DIR": str(root / "dec"), "PIRA_TEAM_DIR": str(root / "team")}) + "; "
                + "choose=lambda tools,*a,**kw: {s.stores.STORE_ENV_KEYS[t]:paths[s.stores.STORE_ENV_KEYS[t]] for t in tools if t in s.stores.STORE_ENV_KEYS}; "
                + "selection=patch.object(s.stores,'selected_store_paths',side_effect=choose); selection.start(); "
                + "result=s.main(" + repr(["--agent-dir", str(root / "agent"), "--codex-config", str(config), "--skip-tools", "--execution-mode", "keep", "--user-mode", "keep", "--legacy", "keep"]) + "); "
                + "registry.SetValueEx.assert_not_called(); s.stores.notify_windows_environment.assert_not_called(); raise SystemExit(result)")
        env.update(PIRA_CTX_STORE_DIR=str(destination), PIRA_DEC_STORE_DIR=str(root / "dec"), PIRA_TEAM_DIR=str(root / "team"))
        captured = subprocess.run([binary, "capture", "--interest", "(?i)active|error", "--intent", "probe fresh captured setup", "--", sys.executable, "-c", code], env=env, capture_output=True, text=True)
        self.assertIn("Unfinished capture requires recovery", captured.stdout + captured.stderr, f"wrapper exit={captured.returncode}: {captured.stdout} {captured.stderr}")
        self.assertEqual(config.read_text(), 'model = "custom"\n')
        self.assertFalse((agent / "AGENTS.override.md").exists())
        exact = subprocess.run([binary, "exact", "--intent", "inspect complete setup output", "--", sys.executable, "-c", code], env=env, capture_output=True, text=True)
        self.assertEqual(exact.returncode, 0, exact.stdout + exact.stderr)
        self.assertIn("Verification passed", exact.stdout)
        self.assertIn("PIRA_TEAM_DIR", config.read_text())
        self.assertFalse(agent.is_symlink())

    def test_retired_options_are_rejected(self) -> None:
        for flag in ("--no-team", "--audio", "--audio-dir", "--force-audio"):
            with self.subTest(flag=flag), self.assertRaises(SystemExit):
                setup.build_parser().parse_args([flag])

    def test_old_team_free_guard_upgrades_without_changing_old_policy(self) -> None:
        root = Path(setup.os.environ["HOME"])
        config = root / "codex/config.toml"
        config.parent.mkdir()
        policy = config.parent / "pira/AGENTS.md"
        policy.parent.mkdir()
        policy.write_text("old policy retained")
        (root / "AGENTS.md").write_text("canonical fixture")
        (root / "AGENTS.override.md").write_text(setup.PROJECT_AGENTS_GUARD.replace("`AGENTS.md`", f"`{setup.config_path_string(policy)}`"))
        config.write_text('model_instructions_file = ' + setup.toml_string(str(policy)) + '\n')
        state = setup.SetupState(root, root, False, True)
        setup.configure_codex(state, config, "keep", False)
        self.assertEqual(policy.read_text(), "old policy retained")
        self.assertEqual(tomllib.loads(config.read_text())["model_instructions_file"], setup.config_path_string(root / "AGENTS.md"))
        self.assertEqual((root / "AGENTS.override.md").read_text(), setup.PROJECT_AGENTS_GUARD)
        first = config.read_bytes()
        setup.configure_codex(state, config, "keep", False)
        self.assertEqual(config.read_bytes(), first)

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
