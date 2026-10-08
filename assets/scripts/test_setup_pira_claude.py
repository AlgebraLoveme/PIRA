from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("setup_pira_claude.py")
SPEC = importlib.util.spec_from_file_location("pira_claude_setup_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)


class ClaudeSetupTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        self.agent = root / "agent"
        self.claude = root / ".claude"
        (self.agent / "modules").mkdir(parents=True)
        (self.agent / "AGENTS.md").write_text(setup.VERIFY_TOKEN + "\n", encoding="utf-8")
        (self.agent / "modules" / "CODING_STYLE.md").write_text("coding\n", encoding="utf-8")
        self.claude.mkdir()
        self.entry = self.claude / "rules" / setup.RULE_FILE
        self.arguments = [
            "--agent-dir", str(self.agent), "--claude-dir", str(self.claude), "--skip-tools"
        ]
        version = patch.object(setup, "claude_version", return_value=(2, 1, 283))
        version.start()
        self.addCleanup(version.stop)

    def run_setup(self, *extra: str) -> int:
        return setup.main([*self.arguments, *extra])

    def settings(self) -> dict[str, object]:
        return json.loads((self.claude / "settings.json").read_text(encoding="utf-8"))

    def test_fresh_install_verify_rerun_and_uninstall(self) -> None:
        codex_config = self.claude.parent / ".codex" / "config.toml"
        codex_config.parent.mkdir()
        codex_config.write_bytes(b"model = 'existing-codex-setting'\n")
        policy_before = (self.agent / "AGENTS.md").read_bytes()
        self.assertEqual(self.run_setup("--dry-run"), 0)
        self.assertFalse(self.entry.exists())
        self.assertEqual(self.run_setup(), 0)
        self.assertEqual(self.run_setup("--verify"), 0)
        self.assertEqual(self.run_setup(), 0)
        if os.name == "nt":
            self.assertTrue(self.entry.is_file())
            self.assertFalse(self.entry.is_symlink())
        else:
            self.assertTrue(self.entry.is_symlink())
        self.assertEqual(
            self.settings()["pluginConfigs"][setup.PLUGIN]["options"]["instructionFiles"],
            setup.MODE,
        )
        self.assertEqual(
            self.settings()["permissions"]["allow"],
            [setup.module_read_rule(self.agent), setup.profile_read_rule(self.claude),
             setup.shared_profile_read_rule(self.agent)],
        )
        self.assertTrue((self.claude / "pira" / "USER.md").is_file())
        self.assertEqual(self.run_setup("--uninstall"), 0)
        self.assertFalse(self.entry.exists())
        self.assertFalse((self.claude / "settings.json").exists())
        self.assertTrue((self.claude / "pira" / "USER.md").exists())
        self.assertEqual(codex_config.read_bytes(), b"model = 'existing-codex-setting'\n")
        self.assertEqual((self.agent / "AGENTS.md").read_bytes(), policy_before)

    def test_preserves_unrelated_settings_and_legacy_instructions(self) -> None:
        settings = {
            "permissions": {"allow": ["Read(~/notes.md)"]},
            "pluginConfigs": {setup.PLUGIN: {"options": {"instructionFiles": "claude-md"}}},
        }
        (self.claude / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        legacy = (
            "# Local rules\n\nKeep this.\n"
            f"{setup.LEGACY_START}\n@~/.claude/pira/AGENTS.md\n{setup.LEGACY_END}\n"
            "After the block.\n"
        )
        (self.claude / "CLAUDE.md").write_text(legacy, encoding="utf-8")
        self.assertEqual(self.run_setup("--user-mode", "keep"), 0)
        self.assertEqual(
            self.settings()["permissions"]["allow"],
            ["Read(~/notes.md)", setup.module_read_rule(self.agent),
             setup.profile_read_rule(self.claude), setup.shared_profile_read_rule(self.agent)],
        )
        self.assertEqual(
            (self.claude / "CLAUDE.md").read_text(encoding="utf-8"),
            "# Local rules\n\nKeep this.\n\nAfter the block.\n",
        )
        self.assertFalse((self.claude / "pira" / "USER.md").exists())
        self.assertEqual(self.run_setup("--uninstall"), 0)
        self.assertEqual(self.settings()["permissions"], settings["permissions"])
        self.assertEqual(
            self.settings()["pluginConfigs"][setup.PLUGIN]["options"]["instructionFiles"],
            "claude-md",
        )

    def test_legacy_removal_preserves_original_bytes(self) -> None:
        for original in (b"Local rules", b"Local rules\r\n", b"Local rules\n"):
            with self.subTest(original=original):
                legacy = self.claude / "CLAUDE.md"
                legacy.write_bytes(
                    original + setup.LEGACY_START.encode() +
                    b"\n@~/.claude/pira/AGENTS.md\n" + setup.LEGACY_END.encode() + b"\n"
                )
                self.assertEqual(self.run_setup(), 0)
                self.assertEqual(legacy.read_bytes(), original)

    def test_unmanaged_entry_and_modified_legacy_block_are_refused(self) -> None:
        entry = self.entry
        entry.parent.mkdir()
        entry.write_text("My own Claude instructions\n", encoding="utf-8")
        self.assertEqual(self.run_setup(), 1)
        self.assertEqual(entry.read_text(encoding="utf-8"), "My own Claude instructions\n")
        entry.unlink()
        legacy = self.claude / "CLAUDE.md"
        legacy.write_text(
            f"{setup.LEGACY_START}\nuser changes\n{setup.LEGACY_END}\n",
            encoding="utf-8",
        )
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse(entry.exists())

    def test_preexisting_identical_rule_requires_install_manifest(self) -> None:
        self.entry.parent.mkdir()
        if os.name != "nt":
            self.entry.symlink_to(self.agent / "AGENTS.md")
            self.assertEqual(self.run_setup(), 1)
            self.assertTrue(self.entry.is_symlink())
            self.entry.unlink()
        self.entry.write_bytes((self.agent / "AGENTS.md").read_bytes())
        with self.assertRaisesRegex(RuntimeError, "no install manifest"):
            setup.install_entry(
                self.entry, self.agent / "AGENTS.md", None, dry_run=True, copy_policy=True
            )

    def test_malformed_or_symlinked_settings_are_refused_before_link(self) -> None:
        settings = self.claude / "settings.json"
        settings.write_text("{broken", encoding="utf-8")
        self.assertEqual(self.run_setup(), 1)
        settings.unlink()
        if os.name == "nt":
            self.assertFalse(self.entry.exists())
            return
        settings.symlink_to(self.agent / "AGENTS.md")
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse(self.entry.exists())
        settings.unlink()
        settings.symlink_to(self.agent / "missing")
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse(self.entry.exists())

    def test_windows_copy_refresh_and_uninstall_after_source_update(self) -> None:
        entry = self.entry
        entry.parent.mkdir()
        manifest = {"policy_sha256": setup.sha256((self.agent / "AGENTS.md").read_bytes())}
        setup.install_entry(entry, self.agent / "AGENTS.md", manifest, dry_run=False, copy_policy=True)
        (self.agent / "AGENTS.md").write_text(setup.VERIFY_TOKEN + "\nupdated\n", encoding="utf-8")
        self.assertTrue(setup.verify_owned_entry(entry, self.agent / "AGENTS.md", manifest, copy_policy=True))
        self.assertFalse(setup.verify_entry(entry, self.agent / "AGENTS.md", copy_policy=True))
        setup.install_entry(entry, self.agent / "AGENTS.md", manifest, dry_run=False, copy_policy=True)
        self.assertTrue(setup.verify_entry(entry, self.agent / "AGENTS.md", copy_policy=True))
        self.assertTrue(setup.verify_owned_entry(
            entry, self.agent / "AGENTS.md",
            {"policy_sha256": setup.sha256(entry.read_bytes())}, copy_policy=True,
        ))
        entry.write_text("unmanaged\n", encoding="utf-8")
        self.assertFalse(setup.verify_owned_entry(
            entry, self.agent / "AGENTS.md", manifest, copy_policy=True,
        ))
        with self.assertRaisesRegex(RuntimeError, "not managed"):
            setup.install_entry(entry, self.agent / "AGENTS.md", manifest, dry_run=False, copy_policy=True)

    @unittest.skipIf(os.name == "nt", "creating symlinks may require Windows privileges")
    def test_broken_manifest_and_profile_links_fail_before_install(self) -> None:
        manifest = self.claude / "pira" / setup.MANIFEST
        manifest.parent.mkdir()
        manifest.symlink_to(self.agent / "missing")
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse(self.entry.exists())
        manifest.unlink()
        (self.claude / "pira" / "USER.md").symlink_to(self.agent / "missing")
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse(self.entry.exists())

    def test_legacy_snapshot_and_unrelated_global_agents_are_untouched(self) -> None:
        old = self.claude / "pira" / "AGENTS.md"
        old.parent.mkdir()
        old.write_text("old snapshot\n", encoding="utf-8")
        unrelated = self.claude / "AGENTS.md"
        unrelated.write_text("unrelated\n", encoding="utf-8")
        self.assertEqual(self.run_setup(), 0)
        self.assertEqual(old.read_text(encoding="utf-8"), "old snapshot\n")
        self.assertEqual(unrelated.read_text(encoding="utf-8"), "unrelated\n")

    @unittest.skipIf(os.name == "nt", "creating symlinks may require Windows privileges")
    def test_symlinked_rules_directory_is_refused(self) -> None:
        (self.claude / "rules").symlink_to(self.agent)
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse((self.claude / "settings.json").exists())

    def test_preexisting_module_rule_survives_uninstall(self) -> None:
        rule = setup.module_read_rule(self.agent)
        profile_rule = setup.profile_read_rule(self.claude)
        shared_rule = setup.shared_profile_read_rule(self.agent)
        settings = {"permissions": {"allow": [rule, profile_rule, shared_rule]}}
        (self.claude / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        self.assertEqual(self.run_setup(), 0)
        self.assertEqual(self.run_setup("--uninstall"), 0)
        self.assertEqual(self.settings()["permissions"], settings["permissions"])

    def test_verify_detects_removed_module_read_rule(self) -> None:
        self.assertEqual(self.run_setup(), 0)
        settings = self.settings()
        settings["permissions"]["allow"].clear()
        (self.claude / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        self.assertEqual(self.run_setup("--verify"), 1)

    def test_verify_detects_removed_private_profile_read_rule(self) -> None:
        self.assertEqual(self.run_setup(), 0)
        settings = self.settings()
        settings["permissions"]["allow"].remove(setup.profile_read_rule(self.claude))
        (self.claude / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        self.assertEqual(self.run_setup("--verify"), 1)

    def test_verify_detects_removed_shared_profile_read_rule(self) -> None:
        self.assertEqual(self.run_setup(), 0)
        settings = self.settings()
        settings["permissions"]["allow"].remove(setup.shared_profile_read_rule(self.agent))
        (self.claude / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        self.assertEqual(self.run_setup("--verify"), 1)

    def test_rerun_upgrades_previous_manifest_without_losing_user_rules(self) -> None:
        self.assertEqual(self.run_setup(), 0)
        manifest_path = self.claude / "pira" / setup.MANIFEST
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest.pop("profile_read_rule")
        manifest.pop("profile_read_rule_added")
        manifest.pop("shared_profile_read_rule")
        manifest.pop("shared_profile_read_rule_added")
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        settings = self.settings()
        settings["permissions"]["allow"].remove(setup.profile_read_rule(self.claude))
        settings["permissions"]["allow"].remove(setup.shared_profile_read_rule(self.agent))
        settings["permissions"]["allow"].append("Read(~/notes.md)")
        (self.claude / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        self.assertEqual(self.run_setup(), 0)
        self.assertEqual(self.run_setup("--verify"), 0)
        self.assertEqual(self.run_setup("--uninstall"), 0)
        self.assertEqual(self.settings()["permissions"]["allow"], ["Read(~/notes.md)"])

    def test_partial_private_profile_manifest_is_refused(self) -> None:
        self.assertEqual(self.run_setup(), 0)
        manifest_path = self.claude / "pira" / setup.MANIFEST
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest.pop("profile_read_rule_added")
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        settings_before = (self.claude / "settings.json").read_bytes()
        self.assertEqual(self.run_setup(), 1)
        self.assertEqual((self.claude / "settings.json").read_bytes(), settings_before)

    def test_malformed_permission_list_is_refused_before_install(self) -> None:
        (self.claude / "settings.json").write_text(
            json.dumps({"permissions": {"allow": "Read"}}), encoding="utf-8"
        )
        self.assertEqual(self.run_setup(), 1)
        self.assertFalse(self.entry.exists())

    def test_default_module_read_rule_is_home_scoped(self) -> None:
        self.assertEqual(
            setup.module_read_rule(Path.home() / "agent"),
            "Read(~/agent/modules/*.md)",
        )
        self.assertEqual(
            setup.profile_read_rule(Path.home() / ".claude"),
            "Read(~/.claude/pira/USER.md)",
        )
        self.assertEqual(
            setup.shared_profile_read_rule(Path.home() / "agent"),
            "Read(~/agent/USER.md)",
        )

    def test_uninstall_preserves_empty_permissions_shape(self) -> None:
        for settings in ({"permissions": {}}, {"permissions": {"allow": []}}):
            with self.subTest(settings=settings):
                path = self.claude / "settings.json"
                path.write_text(json.dumps(settings), encoding="utf-8")
                self.assertEqual(self.run_setup(), 0)
                self.assertEqual(self.run_setup("--uninstall"), 0)
                self.assertEqual(self.settings(), settings)

    def test_uninstall_succeeds_after_policy_source_is_removed(self) -> None:
        self.assertEqual(self.run_setup(), 0)
        (self.agent / "AGENTS.md").unlink()
        self.assertEqual(self.run_setup("--uninstall"), 0)
        self.assertFalse(self.entry.is_symlink())
        self.assertFalse((self.claude / "settings.json").exists())


if __name__ == "__main__":
    unittest.main()
