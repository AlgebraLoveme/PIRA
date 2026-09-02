from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("run_matrix.py")
SPEC = importlib.util.spec_from_file_location("pira_routing_matrix_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class MatrixRunnerTests(unittest.TestCase):
    def test_expanded_route_accepts_explicit_dependencies_but_not_unrelated_modules(self) -> None:
        self.assertEqual(runner.expanded_route(["paper_reading"]), ["paper_reading", "research"])
        self.assertEqual(
            runner.expanded_route(["paper_reading", "research"]), ["paper_reading", "research"]
        )
        self.assertIn("guidance", runner.expanded_route(["paper_reading", "guidance"]))

    def test_route_listing_leads_with_public_figure_code_combination(self) -> None:
        skill = SCRIPT.parents[1] / "skills" / "route" / "SKILL.md"
        content = skill.read_text(encoding="utf-8")
        description = content.split("when_to_use:", 1)[0]
        normalized = " ".join(line.strip() for line in description.splitlines())
        self.assertIn("two separate arguments `coding public_figure`", normalized)

    def test_route_listing_preserves_precision_rules_within_claude_cap(self) -> None:
        skill = SCRIPT.parents[1] / "skills" / "route" / "SKILL.md"
        content = skill.read_text(encoding="utf-8")
        frontmatter = content.split("---", 2)[1]
        description, remainder = frontmatter.split("when_to_use: >-", 1)
        when_to_use = remainder.split("user-invocable:", 1)[0]
        listing = " ".join(
            line.strip()
            for line in (description.split("description: >-", 1)[1] + when_to_use).splitlines()
        )
        self.assertLessEqual(len(listing), 1536)
        self.assertIn("not merely because an arbitrary file is inspected", listing)
        self.assertIn("not for ordinary analysis or an evidence conclusion", listing)
        self.assertIn("Reviewing an existing SVG or image", listing)
        self.assertIn("Add user_profile", listing)
        self.assertIn("explicitly evidence-based analysis/reporting", listing)
        self.assertIn("use paper_reading writing, never writing alone", listing)

    def test_parse_and_evaluate_successful_coding_route(self) -> None:
        lines = [
            '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"pira-routing-guard:route","args":"coding"}}]}}',
            '{"type":"system","subtype":"hook_response","output":"PIRA routing is complete for this turn.","stderr":""}',
            '{"type":"user","message":{"content":[{"type":"text","text":"### Loaded PIRA module: research\\r\\n### Loaded PIRA module: coding\\r\\n"}]}}',
            '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{}}]}}',
            '{"type":"result","subtype":"success","is_error":false,"result":"off-by-one","permission_denials":[],"total_cost_usd":0.01}',
        ]
        parsed = runner.parse_stream(lines)
        scenario = {
            "expected_route": ["coding"],
            "expected_loaded": ["research", "coding"],
            "result_regex": "off.by.one",
        }
        passed, failures = runner.evaluate(scenario, parsed, 0)
        self.assertTrue(passed, failures)
        self.assertEqual(parsed["task_tools"], ["Read"])

    def test_evaluate_detects_tool_before_route_completion(self) -> None:
        lines = [
            '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"pira-routing-guard:route","args":"none"}}]}}',
            '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{}}]}}',
            '{"type":"system","subtype":"hook_response","output":"PIRA routing is complete for this turn.","stderr":""}',
            '{"type":"result","subtype":"success","is_error":false,"result":"ok","permission_denials":[]}',
        ]
        parsed = runner.parse_stream(lines)
        scenario = {"expected_route": ["none"], "expected_loaded": []}
        passed, failures = runner.evaluate(scenario, parsed, 0)
        self.assertFalse(passed)
        self.assertIn("a task tool ran before routing completed", failures)

    def test_validate_matrix_rejects_unknown_modules(self) -> None:
        with self.assertRaisesRegex(ValueError, "unknown modules"):
            runner.validate_matrix(
                {
                    "schema_version": 1,
                    "scenarios": [
                        {
                            "id": "bad",
                            "prompt": "test",
                            "expected_route": ["invented"],
                            "expected_loaded": [],
                        }
                    ],
                }
            )


if __name__ == "__main__":
    unittest.main()
