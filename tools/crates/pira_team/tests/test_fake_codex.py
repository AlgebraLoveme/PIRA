"""Exercise the protocol peer directly, without Team binaries or model calls."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class FakeCodexPolicyTests(unittest.TestCase):
    def test_thread_start_preserves_exact_lf_and_crlf_policy(self):
        for ending in (b"\n", b"\r\n"):
            with self.subTest(ending=ending), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                home, cwd, scratch = root / "home", root / "work", root / "scratch"
                for directory in (home, cwd, scratch):
                    directory.mkdir()
                policy_bytes = ending.join([b"Never read or expose secrets files", b"pira_nav", b""])
                policy = root / "policy.md"
                policy.write_bytes(policy_bytes)
                settings = {
                    "sandbox_mode": "danger-full-access", "approval_policy": "never",
                    "model_instructions_file": str(policy),
                    "shell_environment_policy.set.PIRA_TEAM_HANDOFF": str(root / "handoff"),
                }
                env = {key: os.environ[key] for key in ("SystemRoot", "WINDIR", "COMSPEC", "PATH")
                       if key in os.environ}
                env.update(CODEX_HOME=str(home), PYTHONDONTWRITEBYTECODE="1")
                for key in ("TMPDIR", "TMP", "TEMP", "TMPPREFIX"):
                    value = str(scratch / "zsh" if key == "TMPPREFIX" else scratch)
                    env[key] = value
                    settings["shell_environment_policy.set." + key] = value
                command = [sys.executable, str(Path(__file__).with_name("fake_codex.py")),
                           "app-server", "--strict-config", "--stdio", "agents.enabled=false",
                           "features.multi_agent=false", "project_doc_max_bytes=0",
                           *[key + "=" + json.dumps(value) for key, value in settings.items()]]
                request = {"id": 1, "method": "thread/start", "params": {
                    "cwd": str(cwd), "approvalPolicy": "never", "sandbox": "danger-full-access",
                    "baseInstructions": policy_bytes.decode("utf-8"), "developerInstructions": "", "config": {},
                }}
                result = subprocess.run(command, input=json.dumps(request) + "\n", cwd=cwd,
                                        env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["result"]["thread"]["id"], "fixture-thread")
                state = json.loads((home / "fixture-state.json").read_text())
                self.assertEqual(state["base_policy"].encode("utf-8"), policy_bytes)
                self.assertEqual(policy.read_bytes(), policy_bytes)


if __name__ == "__main__":
    unittest.main()
