"""Persistent auth selection/refresh tests; native peers and credentials are synthetic."""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
import unittest

import test_provider_config
from team_fixture import TeamFixture


class NativeAuthTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = TeamFixture(self.addCleanup)
        self.home = Path(self.fixture.env["CODEX_HOME"])
        for key in ("PIRA_DEC_WORKSPACE_DIR", "CODEX_SQLITE_HOME", "TEAM_REAL_COORDINATOR_CODEX"):
            self.fixture.env.pop(key, None)

    def marker(self, selected: Path) -> Path:
        marker = self.home / "pira-team-auth.json"
        marker.write_text(json.dumps({"version": 1, "home": str(selected)}))
        marker.chmod(0o600)
        return marker

    def launch(self) -> dict:
        result = self.fixture.launch("inert persistent auth fixture")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_selected_file_native_atomic_refresh_target_is_retained_through_resume(self):
        selected = self.fixture.root / "team-native-login"
        selected.mkdir()
        source = selected / "auth.json"
        source.write_text("synthetic-selected-file")
        stale = self.home / "auth.json"
        stale.write_text("synthetic-unselected-main-file")
        (self.home / "config.toml").write_text('cli_auth_credentials_store="auto"\n')
        marker = self.marker(selected)
        marker_before = marker.read_bytes()
        peer = self.fixture.root / "codex"
        script = peer.read_text()
        probe = '\n'.join([
            'import tomllib',
            'if "--stdio" in sys.argv:',
            '    _home = Path(os.environ["CODEX_HOME"])',
            '    assert tomllib.loads((_home / "config.toml").read_text())["cli_auth_credentials_store"] == "file"',
            '    _link = _home / "auth.json"',
            '    assert _link.is_symlink()',
            '    _target = _link.resolve()',
            '    assert str(_target) == ' + repr(str(source)),
            '    _replacement = _target.with_name("synthetic-native-replacement")',
            '    _replacement.write_text("synthetic-native-refreshed")',
            '    os.replace(_replacement, _target)',
        ])
        peer.write_text(script.replace('args = sys.argv[1:]', probe + '\nargs = sys.argv[1:]', 1))
        receipt = self.launch()
        run = Path(receipt["run_root"])
        self.assertEqual(source.read_text(), "synthetic-native-refreshed")
        self.assertTrue((run / "codex-route.sha256").is_file())
        self.assertNotIn("codex_provider_route", json.loads((run / "manifest.json").read_text()))
        self.assertNotEqual(self.fixture.access("read", run.name, "codex-route.sha256").returncode, 0)
        self.assertEqual(stale.read_text(), "synthetic-unselected-main-file")
        self.assertEqual(marker.read_bytes(), marker_before)
        self.assertFalse((run / "codex-home/auth.json").is_symlink())
        resumed = self.fixture.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(source.read_text(), "synthetic-native-refreshed")
        for path in run.rglob("requests.jsonl"):
            requests = [json.loads(line) for line in path.read_text().splitlines()]
            self.assertFalse(any(r.get("method", "").startswith("account/login") for r in requests))
            self.assertNotIn("synthetic-selected-file", path.read_text())
        self.assertNotEqual(self.fixture.access("read", run.name, "codex-home/auth.json").returncode, 0)

    def test_runtime_never_uses_a_stale_file_for_unselected_native_stores(self):
        for mode in ("auto", "keyring", "ephemeral"):
            with self.subTest(mode=mode):
                (self.home / "config.toml").write_text(f'cli_auth_credentials_store="{mode}"\n')
                result = self.fixture.launch("must repair in setup, not launch login")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("rerun PIRA setup/install", result.stderr)
                self.assertIn("never starts login", result.stderr)
                self.assertFalse(list((self.fixture.root / "logs").rglob("requests.jsonl")))

    def test_missing_file_fails_before_backend_with_setup_only_repair_guidance(self):
        (self.home / "auth.json").unlink()
        result = self.fixture.launch("no interactive runtime authentication")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rerun PIRA setup/install", result.stderr)
        self.assertFalse(list((self.fixture.root / "logs").rglob("requests.jsonl")))

    def test_failed_native_turn_preserves_error_and_points_to_setup_renewal(self):
        result = self.fixture.launch("failed")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("setup_pira_tools.py --auth-only", result.stderr)
        self.assertIn("--reauth", result.stderr)

    def test_claude_coordinator_does_not_require_unused_codex_file_login(self):
        import test_claude
        self.fixture.parent([{"sandbox_policy": {"type": "danger-full-access"}}])
        (self.home / "auth.json").unlink()
        test_claude.install_claude(self.fixture.root, self.fixture.env)
        self.fixture.env["TEAM_CLAUDE_BIN"] = str(self.fixture.bin)
        for mode in ("file", "keyring", "auto", "ephemeral"):
            with self.subTest(mode=mode):
                (self.home / "config.toml").write_text(f'cli_auth_credentials_store="{mode}"\n')
                result = subprocess.run([str(self.fixture.bin), "run", "--cwd", str(self.fixture.root),
                    "--store", str(self.fixture.root / "logs"), "--model", "claude-future-family-100",
                    "--effort", "low", "--task", "inert coordinator", "--completion-gate", "fixture output",
                    "--allow-subteam"], env=self.fixture.env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                run = Path(json.loads(result.stdout)["run_root"])
                self.assertFalse((run / "codex-home/auth.json").exists())
    def test_worker_persistent_provider_config_is_explicit_and_does_not_merge_main(self):
        test_provider_config.install_probe(self.fixture)
        (self.home / "pira-team-worker.toml").write_text(test_provider_config.CONFIG)
        (self.home / "config.toml").write_text('malformed main configuration deliberately not inherited')
        self.launch()

    @unittest.skipUnless(os.name == "posix", "POSIX broken config alias")
    def test_broken_selected_worker_config_does_not_fall_back_to_default_provider(self):
        (self.home / "pira-team-worker.toml").symlink_to(self.fixture.root / "missing-config")
        result = self.fixture.launch("must not silently select OpenAI")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(list((self.fixture.root / "logs").rglob("requests.jsonl")))

    def test_bad_selection_is_withheld_but_unused_selection_is_not_required(self):
        marker = self.home / "pira-team-auth.json"
        marker.write_text('{"version":1,"home":"PRIVATE_SENTINEL","extra":true}')
        marker.chmod(0o600)
        result = self.fixture.launch("invalid private selection")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("PRIVATE_SENTINEL", result.stderr)
        self.assertIn("PIRA setup/install", result.stderr)
        test_provider_config.install_probe(self.fixture)
        self.launch()

    @unittest.skipUnless(os.name == "posix", "POSIX private marker boundary")
    def test_shared_or_symlinked_selection_is_rejected(self):
        selected = self.fixture.root / "native-selected"; selected.mkdir()
        (selected / "auth.json").write_text("synthetic-file")
        marker = self.marker(selected)
        marker.chmod(0o644)
        self.assertNotEqual(self.fixture.launch("shared selection").returncode, 0)
        marker.unlink()
        target = self.fixture.root / "private-marker"
        target.write_text(json.dumps({"version":1, "home":str(selected)})); target.chmod(0o600)
        marker.symlink_to(target)
        self.assertNotEqual(self.fixture.launch("linked selection").returncode, 0)


if __name__ == "__main__":
    unittest.main()
