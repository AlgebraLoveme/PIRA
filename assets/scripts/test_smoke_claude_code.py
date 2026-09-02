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
        self.assertEqual(smoke.evaluate(probe, f"The token is {smoke.VERIFY_TOKEN}.", []), ([], []))
        self.assertEqual(smoke.evaluate(probe, "I do not know.", []), ([smoke.VERIFY_TOKEN], []))

    def test_shell_probe_rejects_pira_ctx_wrapping(self) -> None:
        probe = self.probe("shell_routing")
        native = [smoke.tool_call_text("Bash", {"command": "git --version"})]
        wrapped = [smoke.tool_call_text("Bash", {"command": "pira_ctx --intent x -- git --version"})]
        self.assertEqual(smoke.evaluate(probe, "git --version", native), ([], []))
        missing, _ = smoke.evaluate(probe, "git --version", [])
        self.assertIn("Bash call", missing[0])
        self.assertEqual(smoke.evaluate(probe, "git --version", wrapped)[1], ["pira_ctx"])

    def test_module_probe_requires_both_files(self) -> None:
        probe = self.probe("module_routing")
        calls = [smoke.tool_call_text("Read", {"file_path": "~/agent/modules/CODING_STYLE.md"})]
        missing, unexpected = smoke.evaluate(probe, "LOADED CODING_STYLE.md\nLOADED RESEARCH_POLICY.md", calls)
        self.assertEqual(unexpected, [])
        self.assertEqual(missing, ["Read call containing 'RESEARCH_POLICY.md'"])

    def test_parse_claude_stream_extracts_result_and_tool_calls(self) -> None:
        stream = "\n".join(
            (
                json.dumps({"type": "system", "subtype": "init"}),
                json.dumps(
                    {
                        "type": "assistant",
                        "message": {
                            "content": [
                                {"type": "tool_use", "name": "Read", "input": {"file_path": "/tmp/a.py"}}
                            ]
                        },
                    }
                ),
                json.dumps({"type": "result", "result": "final"}),
            )
        )
        result, calls = smoke.parse_claude_stream(stream)
        self.assertEqual(result, "final")
        self.assertEqual(calls, ['Read {"file_path": "/tmp/a.py"}'])

    def test_instruction_loaded_requires_expected_include_event(self) -> None:
        expected = Path("/tmp/agent/AGENTS.md")
        events = [
            {
                "hook_event_name": "InstructionsLoaded",
                "file_path": str(expected),
                "load_reason": "include",
            }
        ]
        self.assertTrue(smoke.instruction_was_loaded(events, expected))
        events[0]["load_reason"] = "session_start"
        self.assertFalse(smoke.instruction_was_loaded(events, expected))


if __name__ == "__main__":
    unittest.main()
