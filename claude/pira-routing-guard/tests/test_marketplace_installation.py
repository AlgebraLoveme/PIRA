from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


PLUGIN_ID = "pira-routing-guard@pira"
PLUGIN_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = PLUGIN_ROOT.parents[1]


@unittest.skipUnless(shutil.which("claude"), "Claude Code CLI is not installed")
class MarketplaceInstallationTests(unittest.TestCase):
    def run_claude(self, *arguments: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["claude", *arguments],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
            timeout=120,
        )

    def test_user_install_is_idempotent_and_reversible(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            config_dir = Path(temporary) / "claude-config"
            config_dir.mkdir()
            settings_path = config_dir / "settings.json"
            sentinel = {"env": {"PIRA_INSTALL_TEST_SENTINEL": "keep"}}
            settings_path.write_text(json.dumps(sentinel), encoding="utf-8")

            env = os.environ.copy()
            env["CLAUDE_CONFIG_DIR"] = str(config_dir)
            env["CLAUDE_CODE_PLUGIN_CACHE_DIR"] = str(config_dir / "plugins")

            self.run_claude(
                "plugin", "marketplace", "add", str(REPO_ROOT), "--scope", "user", env=env
            )
            self.run_claude("plugin", "install", PLUGIN_ID, "--scope", "user", env=env)
            self.run_claude("plugin", "install", PLUGIN_ID, "--scope", "user", env=env)

            installed = json.loads(
                self.run_claude("plugin", "list", "--json", env=env).stdout
            )
            self.assertIn(PLUGIN_ID, json.dumps(installed))
            self.assertEqual(
                json.loads(settings_path.read_text(encoding="utf-8"))["env"],
                sentinel["env"],
            )

            self.run_claude("plugin", "uninstall", PLUGIN_ID, "--scope", "user", env=env)
            self.run_claude("plugin", "marketplace", "remove", "pira", env=env)

            after = self.run_claude("plugin", "list", "--json", env=env).stdout
            self.assertNotIn(PLUGIN_ID, after)
            self.assertEqual(
                json.loads(settings_path.read_text(encoding="utf-8"))["env"],
                sentinel["env"],
            )


if __name__ == "__main__":
    unittest.main()
