"""Structural provider inheritance using synthetic homes and independent TOML reads."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import unittest

from team_fixture import TeamFixture


CONFIG = r'''model_provider = "unused"
profile = "chosen"
model = "do-not-inherit-model"
model_instructions_file = "/never-load"
approval_policy = "on-request"
cli_auth_credentials_store = "file"
forced_login_method = "api"
forced_chatgpt_workspace_id = ["synthetic-workspace"]
[profiles.chosen]
model_provider = "vendor.with.dots"
sandbox_mode = "danger-full-access"
developer_instructions = "never inherit"
[model_providers."vendor.with.dots"]
name = "Synthetic Vendor"
base_url = "https://example.invalid/v1"
wire_api = "responses"
env_key = "SERVICE_CREDENTIAL"
experimental_bearer_token = "synthetic-inline-bearer"
http_headers = { "X-Static" = "synthetic-inline-header" }
env_http_headers = { "X-Env" = "HEADER_CREDENTIAL", "X-Unset" = "UNSET_CREDENTIAL" }
query_params = { "api-version" = "2026-01" }
requires_openai_auth = false
[model_providers.unused.auth]
command = "/never-execute"
[hooks]
command = "/never-execute"
[mcp_servers.unsafe]
command = "/never-execute"
[features]
hooks = true
[permissions]
network = "enabled"
'''
EXPECTED = {
    "model_provider": "vendor.with.dots", "cli_auth_credentials_store": "file",
    "forced_login_method": "api", "forced_chatgpt_workspace_id": ["synthetic-workspace"],
    "model_providers": {"vendor.with.dots": {
        "name": "Synthetic Vendor", "base_url": "https://example.invalid/v1", "wire_api": "responses",
        "env_key": "SERVICE_CREDENTIAL", "experimental_bearer_token": "synthetic-inline-bearer",
        "http_headers": {"X-Static": "synthetic-inline-header"},
        "env_http_headers": {"X-Env": "HEADER_CREDENTIAL", "X-Unset": "UNSET_CREDENTIAL"},
        "query_params": {"api-version": "2026-01"}, "requires_openai_auth": False,
    }},
}
PROBE = r'''
# Test-only independent oracle: inspect the actual private home, not RPC values.
import tomllib
_provider_expected = json.loads(os.environ["TEAM_PROVIDER_EXPECT"])
_provider_actual = tomllib.loads((Path(os.environ["CODEX_HOME"]) / "config.toml").read_text())
if _provider_actual != _provider_expected:
    raise RuntimeError("selected provider projection mismatch (values withheld)")
if os.environ.get("SERVICE_CREDENTIAL") != "synthetic-service-credential" or os.environ.get("HEADER_CREDENTIAL") != "synthetic-header-credential":
    raise RuntimeError("selected provider environment was not pinned")
if "UNSET_CREDENTIAL" in os.environ:
    raise RuntimeError("worker supplied a credential absent from the coordinator")
for _synthetic_value in ["synthetic-inline-bearer", "synthetic-inline-header", "synthetic-service-credential", "synthetic-header-credential"]:
    if any(_synthetic_value in arg for arg in sys.argv):
        raise RuntimeError("provider value leaked into process arguments")
if "generate-json-schema" not in sys.argv:
    _prefix = "shell_environment_policy.exclude="
    _excluded = json.loads(next(arg[len(_prefix):] for arg in sys.argv if arg.startswith(_prefix)))
    if not {"SERVICE_CREDENTIAL", "HEADER_CREDENTIAL", "UNSET_CREDENTIAL"}.issubset(_excluded):
        raise RuntimeError("provider environment is available to worker shells")
'''


def install_probe(fixture: TeamFixture) -> None:
    peer = fixture.root / "codex"
    source = peer.read_text()
    assert "args = sys.argv[1:]" in source
    peer.write_text(source.replace("args = sys.argv[1:]", PROBE + "\nargs = sys.argv[1:]", 1))
    fixture.env.pop("TEAM_REAL_COORDINATOR_CODEX", None)
    fixture.env.update(TEAM_PROVIDER_EXPECT=json.dumps(EXPECTED),
                       SERVICE_CREDENTIAL="synthetic-service-credential",
                       HEADER_CREDENTIAL="synthetic-header-credential")
    fixture.env.pop("UNSET_CREDENTIAL", None)
    (Path(fixture.env["CODEX_HOME"]) / "config.toml").write_text(CONFIG)


class ProviderConfigTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = TeamFixture(self.addCleanup)
        self.fixture.env.pop("PIRA_DEC_WORKSPACE_DIR", None)
        self.source = Path(self.fixture.env["CODEX_HOME"]) / "config.toml"

    def assert_no_provider_values_in_outputs(self, run: Path) -> None:
        for name in ("manifest.json", "requests.jsonl", "events.jsonl", "stderr.log", "backend-check.log", "phase.md", "policy.md"):
            for path in run.rglob(name):
                data = path.read_text()
                for value in ("synthetic-inline-bearer", "synthetic-inline-header", "synthetic-service-credential", "synthetic-header-credential"):
                    self.assertNotIn(value, data)

    def test_selected_profile_auth_projection_run_resume_and_detach(self) -> None:
        import tomllib
        install_probe(self.fixture)
        # Verify the source syntax independently, not via a second use of Rust selection.
        self.assertEqual(tomllib.loads(CONFIG)["profiles"]["chosen"]["model_provider"], "vendor.with.dots")
        first = self.fixture.launch("provider fixture")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        run = Path(receipt["run_root"])
        self.assertFalse((run / "codex-home/config.toml").exists())
        self.assertEqual(self.source.read_text(), CONFIG)
        self.assert_no_provider_values_in_outputs(run)
        resumed = self.fixture.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        changed = CONFIG.replace("example.invalid/v1", "changed.invalid/v2")
        self.source.write_text(changed)
        refused = self.fixture.access("resume", receipt["run_id"])
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("worker route changed", refused.stderr)
        self.assertFalse((run / "codex-home/config.toml").exists())
        self.assert_no_provider_values_in_outputs(run)

    def test_unused_native_credential_store_does_not_block_environment_provider(self) -> None:
        install_probe(self.fixture)
        self.source.write_text(CONFIG.replace('cli_auth_credentials_store = "file"', 'cli_auth_credentials_store = "auto"'))
        expected = json.loads(json.dumps(EXPECTED))
        expected["cli_auth_credentials_store"] = "auto"
        self.fixture.env["TEAM_PROVIDER_EXPECT"] = json.dumps(expected)
        result = self.fixture.launch("environment auth ignores native credential store")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_explicit_api_key_does_not_require_an_unused_native_credential_store(self) -> None:
        install_probe(self.fixture)
        self.source.write_text(CONFIG.replace('cli_auth_credentials_store = "file"', 'cli_auth_credentials_store = "auto"').replace('requires_openai_auth = false', 'requires_openai_auth = true'))
        expected = json.loads(json.dumps(EXPECTED))
        expected["cli_auth_credentials_store"] = "auto"
        expected["model_providers"]["vendor.with.dots"]["requires_openai_auth"] = True
        self.fixture.env.update(TEAM_PROVIDER_EXPECT=json.dumps(expected), CODEX_API_KEY="synthetic-explicit-key")
        result = self.fixture.launch("explicit API authentication needs no persisted login")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_preflight_failure_detaches_config_and_values_stay_private(self) -> None:
        install_probe(self.fixture)
        self.fixture.env["TEAM_SCHEMA_FAIL"] = "1"
        failed = self.fixture.launch("preflight failure")
        self.assertNotEqual(failed.returncode, 0)
        run, = (self.fixture.root / "logs").glob("*/manifest.json")
        self.assertFalse((run.parent / "codex-home/config.toml").exists())
        self.assert_no_provider_values_in_outputs(run.parent)
        self.assertEqual(self.source.read_text(), CONFIG)

    def test_bad_toml_or_selected_executable_auth_fails_before_backend_without_values(self) -> None:
        self.fixture.env.pop("TEAM_REAL_COORDINATOR_CODEX", None)
        for source in (
            'model_provider = "synthetic-private"\nmodel_provider = "duplicate"',
            'model_provider = "synthetic-private"\n[model_providers.synthetic-private.auth]\ncommand="/never-execute"',
            '[model_providers.openai]\nenv_key="PIRA_TEAM_CONTEXT"',
            'cli_auth_credentials_store="keyring"',
        ):
            with self.subTest(case=source.splitlines()[0].split("=")[0]):
                self.source.write_text(source)
                failed = self.fixture.launch("must not start")
                self.assertNotEqual(failed.returncode, 0)
                self.assertNotIn("synthetic-private", failed.stderr)
                self.assertFalse(list((self.fixture.root / "logs").rglob("requests.jsonl")))
                self.assertEqual(self.source.read_text(), source)

    @unittest.skipUnless(os.name == "posix", "POSIX inert coordinator and shell fixture")
    def test_coordinator_pins_config_and_arbitrary_auth_environment_against_child_overrides(self) -> None:
        import test_subteams
        fixture = test_subteams.SubteamTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        install_probe(fixture)
        process, coordinator = fixture.start()
        (Path(fixture.env["CODEX_HOME"]) / "config.toml").write_text('model_provider = "worker-changed"')
        alternative = fixture.root / "worker-home"
        alternative.mkdir()
        (alternative / "config.toml").write_text('model_provider = "worker-changed"')
        child = fixture.child(coordinator, env={"CODEX_HOME": str(alternative),
            "SERVICE_CREDENTIAL": "worker-forged", "HEADER_CREDENTIAL": "worker-forged",
            "UNSET_CREDENTIAL": "worker-forged"})
        self.assertIn("run_id", child, child)
        run = fixture.root / "logs" / child["run_id"]
        inaccessible = fixture.command(coordinator, "read", child["run_id"], "codex-home/config.toml")
        self.assertNotEqual(inaccessible["returncode"], 0)
        self.assertTrue((run / "codex-home/config.toml").is_file())
        self.assertEqual((run / "codex-home/config.toml").stat().st_mode & 0o777, 0o600)
        fixture.finish(child["run_id"])
        result = fixture.command(coordinator, operation="wait", handle=child["handle"])
        self.assertEqual(result["returncode"], 0, result)
        self.assertFalse((run / "codex-home/config.toml").exists())
        self.assert_no_provider_values_in_outputs(run)
        fixture.finish(coordinator)
        self.assertEqual(fixture.wait(process)[0], 0)
        root = fixture.root / "logs" / coordinator
        self.assertFalse((root / "codex-home/config.toml").exists())
        self.assert_no_provider_values_in_outputs(root)

    @unittest.skipUnless(os.name == "posix", "POSIX shell filter")
    def test_claude_shell_filter_removes_selected_custom_provider_credentials(self) -> None:
        prefix = self.fixture.root / "pira-team-claude-shell"
        prefix.symlink_to(self.fixture.bin)
        env = dict(self.fixture.env, SERVICE_CREDENTIAL="synthetic", HEADER_CREDENTIAL="synthetic",
                   PIRA_TEAM_PROVIDER_ENV=json.dumps(["SERVICE_CREDENTIAL", "HEADER_CREDENTIAL"]), ORDINARY_SETTING="kept")
        command = 'test -z "${SERVICE_CREDENTIAL+x}" && test -z "${HEADER_CREDENTIAL+x}" && test -z "${PIRA_TEAM_PROVIDER_ENV+x}" && test "$ORDINARY_SETTING" = kept'
        checked = subprocess.run([str(prefix), command], env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(checked.returncode, 0, checked.stderr)


if __name__ == "__main__":
    unittest.main()
