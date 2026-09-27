"""Focused trace-parsing tests for the optional live routing smoke."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from smoke_pira_routing import SOURCE, claude_evidence, codex_evidence


class RoutingEvidenceTests(unittest.TestCase):
    def test_claude_counts_actual_read_calls_not_answer_claims(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            modules = root / "modules"
            project = root / "project"
            reads = [
                project / "calc.py",
                modules / "CODING_STYLE.md",
                modules / "RESEARCH_POLICY.md",
            ]
            uses = [
                {"type": "tool_use", "id": str(index), "name": "Read",
                 "input": {"file_path": str(path)}}
                for index, path in enumerate(reads)
            ]
            results = [
                {"type": "tool_result", "tool_use_id": str(index), "content": "file text"}
                for index in range(len(reads))
            ]
            events = [
                {"type": "assistant", "message": {"content": uses}},
                {"type": "user", "message": {"content": results}},
                {"type": "result", "subtype": "success", "is_error": False,
                 "permission_denials": []},
            ]
            evidence = claude_evidence(events, project, modules)
            self.assertTrue(evidence["code_read"])
            self.assertTrue(all(evidence["modules_read"].values()))
            self.assertTrue(evidence["completed"])

            removed = events[0]["message"]["content"].pop()
            self.assertFalse(claude_evidence(events, project, modules)["modules_read"]["RESEARCH_POLICY.md"])

            events[0]["message"]["content"].append(removed)
            results[-1]["is_error"] = True
            self.assertFalse(claude_evidence(events, project, modules)["modules_read"]["RESEARCH_POLICY.md"])

    def test_claude_counts_successful_bash_read(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            events = [
                {"type": "assistant", "message": {"content": [
                    {"type": "tool_use", "id": "shell", "name": "Bash",
                     "input": {"command": "cat calc.py"}},
                ]}},
                {"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "shell",
                     "content": "1 def average(values):\n2     return sum(values) // len(values)"},
                ]}},
                {"type": "result", "subtype": "success"},
            ]
            self.assertTrue(claude_evidence(events, root, root)["code_read"])

    def test_codex_requires_successful_tool_output(self) -> None:
        successful = {
            "type": "item.completed",
            "item": {"type": "command_execution", "exit_code": 0, "command": "cat calc.py",
                     "aggregated_output": "# CODING_STYLE\n# RESEARCH_POLICY\n"
                     "1 def average(values):\n"
                     "2     return sum(values) // len(values)\n"},
        }
        events = [successful, {"type": "turn.completed"}]
        evidence = codex_evidence(events)
        self.assertTrue(evidence["code_read"])
        self.assertTrue(all(evidence["modules_read"].values()))
        self.assertTrue(evidence["completed"])

        successful["item"]["exit_code"] = 1
        evidence = codex_evidence(events)
        self.assertFalse(evidence["code_read"])
        self.assertFalse(any(evidence["modules_read"].values()))

        successful["item"]["exit_code"] = 0
        successful["item"]["command"] = "cat AGENTS.md"
        self.assertFalse(codex_evidence(events)["code_read"])


if __name__ == "__main__":
    unittest.main()
