from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
SCRIPT = HERE / "run_continuity.py"
SPEC = importlib.util.spec_from_file_location("pira_routing_continuity_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
continuity = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(continuity)


class ContinuityRunnerTests(unittest.TestCase):
    def test_commands_persist_then_resume_without_disabling_history(self) -> None:
        common = {
            "model": "sonnet",
            "effort": "low",
            "tools": "Skill,Bash,Read",
            "max_budget": 0.16,
        }
        first = continuity.build_command(
            "claude", Path("plugin"), "first", "session-id", resume=False, **common
        )
        second = continuity.build_command(
            "claude", Path("plugin"), "second", "session-id", resume=True, **common
        )
        self.assertIn("--session-id", first)
        self.assertNotIn("--resume", first)
        self.assertIn("--resume", second)
        self.assertNotIn("--session-id", second)
        self.assertNotIn("--no-session-persistence", first + second)
        self.assertIn("--strict-mcp-config", first + second)

    def test_resume_mode_requires_explicit_expectations(self) -> None:
        parser = continuity.build_parser()
        args = parser.parse_args(["--resume-session", "session-id"])
        self.assertIsNone(args.expected_route)
        self.assertIsNone(args.expected_loaded)

    def test_compaction_summary_requires_success_hook_and_pending_context(self) -> None:
        events = [
            {"type": "system", "subtype": "status", "compact_result": "success"},
            {
                "type": "system",
                "subtype": "hook_response",
                "hook_name": "SessionStart:compact",
                "output": "PIRA routing is pending",
            },
            {"type": "user", "message": {"content": "PostCompact completed successfully"}},
        ]
        turn = {
            "exit_code": 0,
            "stdout": "\n".join(json.dumps(event) for event in events),
            "stderr": "",
        }
        summary = continuity.compaction_summary(turn)
        self.assertTrue(summary["passed"])

        turn["stdout"] = json.dumps(events[0])
        summary = continuity.compaction_summary(turn)
        self.assertFalse(summary["passed"])


if __name__ == "__main__":
    unittest.main()
