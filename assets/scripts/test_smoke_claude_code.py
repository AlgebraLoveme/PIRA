from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("smoke_claude_code.py")
SPEC = importlib.util.spec_from_file_location("pira_smoke_claude_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
smoke = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = smoke
SPEC.loader.exec_module(smoke)


class SmokeClaudeCodeTests(unittest.TestCase):
    def probe(self, name: str) -> object:
        return next(probe for probe in smoke.PROBES if probe.name == name)

    def test_policy_probe_requires_the_verification_token(self) -> None:
        probe = self.probe("policy_import")
        self.assertEqual(smoke.evaluate(probe, f"The token is {smoke.VERIFY_TOKEN}."), ([], []))
        self.assertEqual(smoke.evaluate(probe, "I do not know."), ([smoke.VERIFY_TOKEN], []))

    def test_shell_probe_rejects_pira_ctx_wrapping(self) -> None:
        probe = self.probe("shell_routing")
        self.assertEqual(smoke.evaluate(probe, "git --version"), ([], []))
        self.assertEqual(smoke.evaluate(probe, "pira_ctx --intent x -- git --version"), ([], ["pira_ctx"]))

    def test_module_probe_requires_both_files(self) -> None:
        probe = self.probe("module_routing")
        self.assertEqual(smoke.evaluate(probe, "LOADED ~/agent/modules/CODING_STYLE.md"), (["RESEARCH_POLICY.md"], []))

    def test_parse_claude_json_handles_object_and_array_outputs(self) -> None:
        self.assertEqual(smoke.parse_claude_json(json.dumps({"result": "hello"})), "hello")
        stream = json.dumps([{"type": "system"}, {"type": "result", "result": "final"}])
        self.assertEqual(smoke.parse_claude_json(stream), "final")


if __name__ == "__main__":
    unittest.main()
