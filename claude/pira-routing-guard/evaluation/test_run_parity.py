from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace


HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "run_parity.py"
SPEC = importlib.util.spec_from_file_location("pira_parity_test_target", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
parity = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = parity
SPEC.loader.exec_module(parity)


class ParityRunnerTests(unittest.TestCase):
    def test_materialized_case_uses_synthetic_policy_and_profile(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            scenario = {"id": "example", "prompt": "test", "files": {"sample.txt": "x"}}
            project, agent, state = parity.materialize_case(root, scenario)

            self.assertTrue((project / "CLAUDE.md").is_file())
            self.assertTrue((project / "AGENTS.override.md").is_file())
            self.assertTrue((project / "sample.txt").is_file())
            self.assertEqual(agent.parent, project)
            self.assertIn(str(agent.resolve()).replace("\\", "/"), (agent / "AGENTS.md").read_text(encoding="utf-8"))
            self.assertIn("PIRA_EVAL_MODULE::user_profile", (agent / "USER.md").read_text(encoding="utf-8"))
            self.assertFalse(state.exists())

    def test_parse_codex_requires_module_completion_before_work(self) -> None:
        events = [
            {"type": "item.completed", "item": {"type": "command_execution", "aggregated_output": "PIRA_EVAL_MODULE::research", "status": "completed"}},
            {"type": "item.completed", "item": {"type": "command_execution", "aggregated_output": "project data", "status": "completed"}},
            {"type": "item.completed", "item": {"type": "agent_message", "text": "answer"}},
            {"type": "turn.completed", "usage": {"input_tokens": 10}},
        ]
        parsed = parity.parse_codex("\n".join(json.dumps(event) for event in events))
        self.assertEqual(parsed["loaded_modules"], ["research"])
        self.assertTrue(parsed["route_complete_before_work"])
        self.assertEqual(parsed["final_text"], "answer")

        reversed_events = [events[1], events[0], *events[2:]]
        parsed = parity.parse_codex("\n".join(json.dumps(event) for event in reversed_events))
        self.assertFalse(parsed["route_complete_before_work"])

        batched = [
            {
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "command": "pira_nav show modules/RESEARCH_POLICY.md sample.py",
                    "aggregated_output": "PIRA_EVAL_MODULE::research",
                    "status": "completed",
                },
            },
            events[2],
        ]
        parsed = parity.parse_codex(
            "\n".join(json.dumps(event) for event in batched), ("sample.py",)
        )
        self.assertTrue(parsed["route_complete_before_work"])

        batched[0]["item"]["command"] = "pira_nav show sample.py modules/RESEARCH_POLICY.md"
        parsed = parity.parse_codex(
            "\n".join(json.dumps(event) for event in batched), ("sample.py",)
        )
        self.assertFalse(parsed["route_complete_before_work"])

    def test_parse_codex_ignores_progress_and_failed_probe_without_task_data(self) -> None:
        events = [
            {"type": "item.completed", "item": {"type": "agent_message", "text": "I will inspect it."}},
            {
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "command": "pira_nav show sample.py",
                    "aggregated_output": "file not found",
                    "status": "failed",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "command": "pira_nav show modules/RESEARCH_POLICY.md modules/CODING_STYLE.md sample.py",
                    "aggregated_output": "PIRA_EVAL_MODULE::research\nPIRA_EVAL_MODULE::coding\nsample",
                    "status": "completed",
                },
            },
            {"type": "item.completed", "item": {"type": "agent_message", "text": "finding"}},
        ]
        parsed = parity.parse_codex(
            "\n".join(json.dumps(event) for event in events), ("sample.py",)
        )
        self.assertTrue(parsed["route_complete_before_work"])
        self.assertEqual(parsed["final_text"], "finding")

    def test_parse_codex_none_route_allows_direct_answer(self) -> None:
        event = {"type": "item.completed", "item": {"type": "agent_message", "text": "PONG"}}
        parsed = parity.parse_codex(json.dumps(event))
        self.assertEqual(parsed["loaded_modules"], [])
        self.assertTrue(parsed["route_complete_before_work"])

    def test_evaluate_applies_same_loaded_module_oracle(self) -> None:
        scenario = {
            "expected_route": ["coding"],
            "expected_loaded": ["research", "coding"],
            "result_regex": "finding",
        }
        parsed = {
            "parse_errors": [],
            "route_calls": [],
            "loaded_modules": ["research"],
            "route_complete_before_work": True,
            "hook_errors": [],
            "permission_denials": [],
            "turn_failed": False,
            "final_text": "finding",
        }
        failures = parity.evaluate("codex", scenario, parsed, 0)
        self.assertTrue(any("loaded" in failure for failure in failures))

    def test_commands_isolate_codex_and_load_claude_plugin(self) -> None:
        args = SimpleNamespace(
            prompt="test",
            claude_tools="Skill,Bash,Read",
            claude_model="sonnet",
            claude_effort="low",
            claude_max_budget=0.25,
            codex_model="gpt-5.6-sol",
            codex_effort="low",
        )
        project = Path("project")
        claude = parity.command_for("claude", "claude", project, args)
        codex = parity.command_for("codex", "codex", project, args)
        self.assertIn("--plugin-dir", claude)
        self.assertIn("--setting-sources", claude)
        self.assertIn("--ignore-user-config", codex)
        self.assertIn("--ignore-rules", codex)
        self.assertNotIn("--sandbox", codex)
        self.assertTrue(any("pira_eval_read" in argument for argument in codex))
        self.assertTrue(any(":workspace_roots" in argument and '"read"' in argument for argument in codex))


if __name__ == "__main__":
    unittest.main()
