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

    def test_parse_policy_only_requires_successful_module_reads_before_work(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            policy = root / "policy"
            project.mkdir()
            module = policy / "modules" / "RESEARCH_POLICY.md"
            module.parent.mkdir(parents=True)
            module.write_text("PIRA_EVAL_MODULE::research\n", encoding="utf-8")
            module_id = "module-read"
            events = [
                {
                    "type": "assistant",
                    "message": {
                        "content": [
                            {
                                "type": "tool_use",
                                "id": module_id,
                                "name": "Read",
                                "input": {"file_path": str(module)},
                            }
                        ]
                    },
                },
                {
                    "type": "user",
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": module_id,
                                "content": "PIRA_EVAL_MODULE::research",
                            }
                        ]
                    },
                },
                {
                    "type": "assistant",
                    "message": {
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "task-read",
                                "name": "Read",
                                "input": {"file_path": str(project / "evidence.txt")},
                            }
                        ]
                    },
                },
                {"type": "result", "subtype": "success", "result": "answer"},
            ]
            parsed = parity.parse_claude_policy_only(
                "\n".join(json.dumps(event) for event in events), project, policy
            )
            self.assertEqual(parsed["loaded_modules"], ["research"])
            self.assertEqual(parsed["task_tools"], ["Read"])
            self.assertTrue(parsed["route_complete_before_work"])

            reversed_events = [events[2], events[0], events[1], events[3]]
            parsed = parity.parse_claude_policy_only(
                "\n".join(json.dumps(event) for event in reversed_events), project, policy
            )
            self.assertFalse(parsed["route_complete_before_work"])

            events[1]["message"]["content"][0]["is_error"] = True
            parsed = parity.parse_claude_policy_only(
                "\n".join(json.dumps(event) for event in events), project, policy
            )
            self.assertEqual(parsed["loaded_modules"], [])

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

    def test_parse_codex_allows_routed_direct_answer_without_task_tool(self) -> None:
        events = [
            {
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "command": "pira_nav show modules/EXPLAIN_STYLE.md",
                    "aggregated_output": "PIRA_EVAL_MODULE::explain",
                    "status": "completed",
                },
            },
            {"type": "item.completed", "item": {"type": "agent_message", "text": "answer"}},
        ]
        parsed = parity.parse_codex("\n".join(json.dumps(event) for event in events))
        self.assertTrue(parsed["route_complete_before_work"])

    def test_parse_codex_rejects_external_skill_access(self) -> None:
        event = {
            "type": "item.completed",
            "item": {
                "type": "command_execution",
                "command": "pira_nav show C:/home/.codex/skills/example/SKILL.md",
                "aggregated_output": "skill data",
                "status": "completed",
            },
        }
        parsed = parity.parse_codex(json.dumps(event))
        self.assertEqual(parsed["unexpected_skill_access_count"], 1)
        scenario = {"expected_loaded": [], "prompt": "test"}
        failures = sum(parity.evaluate("codex", scenario, parsed | {"final_text": "answer"}, 0), [])
        self.assertTrue(any("external skill" in failure for failure in failures))

    def test_skill_disable_config_covers_discovered_skill_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            skill = Path(temporary) / "example" / "SKILL.md"
            skill.parent.mkdir(parents=True)
            skill.write_text("example", encoding="utf-8")
            config = parity.disabled_skills_config([skill])
            self.assertTrue(config.startswith("skills.config="))
            self.assertIn("enabled=false", config)
            self.assertIn(json.dumps(str(skill.resolve())), config)

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
        failures = sum(parity.evaluate("codex", scenario, parsed, 0), [])
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
        policy_only = parity.command_for("claude-policy-only", "claude", project, args)
        skill = Path("skills") / "example" / "SKILL.md"
        codex = parity.command_for("codex", "codex", project, args, [skill])
        self.assertIn("--plugin-dir", claude)
        self.assertNotIn("--plugin-dir", policy_only)
        self.assertIn("Read,Bash", policy_only)
        self.assertIn("--setting-sources", claude)
        self.assertIn("--ignore-user-config", codex)
        self.assertIn("--ignore-rules", codex)
        self.assertNotIn("--sandbox", codex)
        self.assertTrue(any("pira_eval_read" in argument for argument in codex))
        self.assertTrue(any(":workspace_roots" in argument and '"read"' in argument for argument in codex))
        self.assertTrue(any(argument.startswith("skills.config=") and "SKILL.md" in argument for argument in codex))


if __name__ == "__main__":
    unittest.main()
