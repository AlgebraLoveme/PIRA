"""Unit tests for the adaptive-mode additions to run_parity.py, run_multiturn.py and report_modes.py."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent


def load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


parity = load("run_parity", "run_parity.py")
multiturn = load("run_multiturn", "run_multiturn.py")
report = load("report_modes", "report_modes.py")


def hook_response(context: str, hook_name: str = "UserPromptSubmit") -> dict:
    output = json.dumps({"hookSpecificOutput": {"hookEventName": hook_name, "additionalContext": context}})
    return {"type": "system", "subtype": "hook_response", "hook_name": hook_name, "output": output}


ADAPTIVE_CONTEXT = (
    "PIRA adaptive route confirmed: research, coding.\n\n"
    "### Loaded PIRA module: research\n\nx\n\n### Loaded PIRA module: coding\n\nx\n\n"
    "Invoke `pira-routing-guard:route` only if a required PIRA module is missing from this route. "
    "PIRA routing is complete for this turn."
)
REUSE_CONTEXT = (
    "PIRA adaptive route confirmed: research, coding (already loaded and unchanged). "
    "Invoke `pira-routing-guard:route` only if a required PIRA module is missing from this route. "
    "PIRA routing is complete for this turn."
)
RESULT = {
    "type": "result", "subtype": "success", "result": "answer", "num_turns": 2,
    "usage": {"input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 20},
}


def stream(events: list[dict]) -> str:
    return "\n".join(json.dumps(event) for event in events)


def skill_call(args: str, tool_use_id: str = "r1") -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tool_use_id, "name": "Skill", "input": {"skill": parity.ROUTE_SKILL, "args": args}}]}}


READ = {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a.py"}}]}}
TEXT = {"type": "assistant", "message": {"content": [{"type": "text", "text": "answer"}]}}


class ParityV2Tests(unittest.TestCase):
    def test_adaptive_selection_sets_active_route_and_loaded_modules(self) -> None:
        parsed = parity.parse_claude(stream([hook_response(ADAPTIVE_CONTEXT), READ, TEXT, RESULT]))
        self.assertTrue(parsed["adaptive_selected"])
        self.assertEqual(parsed["active_route"], ["coding", "research"])
        self.assertEqual(parsed["loaded_modules"], ["research", "coding"])
        self.assertTrue(parsed["route_complete_before_work"])
        routing, task = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["coding", "research"]}, parsed, 0)
        self.assertEqual((routing, task), ([], []))

    def test_extra_and_missing_modules_both_fail_the_routing_contract(self) -> None:
        parsed = parity.parse_claude(stream([hook_response(ADAPTIVE_CONTEXT), READ, TEXT, RESULT]))
        routing, _ = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["research"]}, parsed, 0)
        self.assertTrue(any("active route" in item for item in routing))
        routing, _ = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["research", "coding", "explain"]}, parsed, 0)
        self.assertTrue(any("active route" in item for item in routing))
        routing, _ = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["research", "coding"], "expect_adaptive": "abstain"}, parsed, 0)
        self.assertTrue(any("must fall back" in item for item in routing))

    def test_skill_reroute_after_adaptive_selection_is_checked_on_the_final_route(self) -> None:
        skill_stream = stream([
            hook_response(ADAPTIVE_CONTEXT),
            skill_call("coding explain"),
            hook_response("PIRA routing is complete for this turn.", "PostToolUse"),
            {"type": "user", "message": {"content": [{"type": "text", "text": "### Loaded PIRA module: explain\n\nx\n"}]}},
            READ, TEXT, RESULT,
        ])
        parsed = parity.parse_claude(skill_stream)
        self.assertTrue(parsed["adaptive_selected"])
        self.assertEqual(parsed["active_route"], ["coding", "explain", "research"])
        routing, _ = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["research", "coding"]}, parsed, 0)
        self.assertTrue(any("active route" in item for item in routing))
        routing, _ = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["research", "coding", "explain"]}, parsed, 0)
        self.assertEqual(routing, [])

    def test_task_failures_are_reported_separately_from_routing(self) -> None:
        denied = dict(RESULT, permission_denials=[{"tool_name": "Bash"}])
        parsed = parity.parse_claude(stream([hook_response(ADAPTIVE_CONTEXT), READ, TEXT, denied]))
        routing, task = parity.evaluate("claude-adaptive", {"id": "x", "expected_loaded": ["research", "coding"]}, parsed, 0)
        self.assertEqual(routing, [])
        self.assertTrue(any("permission denials" in item for item in task))

    def test_strict_client_requires_exactly_one_matching_route_call(self) -> None:
        parsed = parity.parse_claude(stream([hook_response(ADAPTIVE_CONTEXT), READ, TEXT, RESULT]))
        routing, _ = parity.evaluate("claude", {"id": "x", "expected_loaded": ["research", "coding"]}, parsed, 0)
        self.assertTrue(any("expected one route call" in item for item in routing))
        strict = stream([
            skill_call("coding"),
            hook_response("PIRA routing is complete for this turn.", "PostToolUse"),
            {"type": "user", "message": {"content": [{"type": "text", "text": "### Loaded PIRA module: research\n\nx\n\n### Loaded PIRA module: coding\n\nx\n"}]}},
            READ, TEXT, RESULT,
        ])
        parsed = parity.parse_claude(strict)
        self.assertEqual(parsed["active_route"], ["coding", "research"])
        self.assertEqual(parity.evaluate("claude", {"id": "x", "expected_loaded": ["research", "coding"]}, parsed, 0), ([], []))

    def test_metrics_report_overall_and_adaptive_subset_with_coverage(self) -> None:
        def row(client: str, case: str, **extra):
            base = {"client": client, "id": case, "repetition": 1, "passed": True, "routing_passed": True, "task_passed": True,
                    "num_turns": 4 if client == "claude" else 2, "duration_seconds": 5.0, "route_calls": [["coding"]] if client == "claude" else [],
                    "module_requiring": True, "adaptive_selected": False, "extra_modules": [], "missing_modules": [],
                    "usage": {"input_tokens": 4, "cache_creation_input_tokens": 15000, "cache_read_input_tokens": 60000 if client == "claude" else 32000, "output_tokens": 100}}
            base.update(extra)
            return base
        results = [
            row("claude-policy-only", "a"), row("claude-policy-only", "b"),
            row("claude", "a"), row("claude", "b"),
            row("claude-adaptive", "a", adaptive_selected=True),
            row("claude-adaptive", "b", num_turns=4, route_calls=[["coding"]], usage={"input_tokens": 4, "cache_creation_input_tokens": 15000, "cache_read_input_tokens": 60000, "output_tokens": 100}),
        ]
        metrics = parity.mode_metrics(results)
        self.assertEqual(set(metrics["overall"]), {"claude-policy-only", "claude", "claude-adaptive"})
        subset = metrics["adaptive_selected_subset"]
        self.assertEqual(subset["selected_cases"], 1)
        self.assertEqual(subset["coverage"], 0.5)
        self.assertEqual(subset["adaptive"]["paired_delta_vs_policy_only_median"]["context_tokens"], 0)
        self.assertEqual(subset["strict_on_same_cases"]["paired_delta_vs_policy_only_median"]["context_tokens"], 28000)
        self.assertEqual(subset["context_overhead_reduction_vs_strict"], 1.0)
        self.assertEqual(subset["model_turn_reduction_vs_strict_median"], 2)

    def test_adaptive_client_command_is_guarded(self) -> None:
        args = SimpleNamespace(prompt="p", claude_tools="Skill,Bash,Read", claude_model="sonnet", claude_effort="low", claude_max_budget=0.1)
        self.assertIn("--plugin-dir", parity.command_for("claude-adaptive", "claude", Path("."), args))
        self.assertNotIn("--plugin-dir", parity.command_for("claude-policy-only", "claude", Path("."), args))


class MultiturnV2Tests(unittest.TestCase):
    def test_commands_select_mode_specific_tools_and_resume(self) -> None:
        options = {"model": "sonnet", "effort": "low", "tools": "Skill,Bash,Read", "max_budget": 1.0}
        strict = multiturn.build_command("strict", "claude", "sid", resume=False, **options)
        self.assertIn("--session-id", strict)
        self.assertIn("--plugin-dir", strict)
        policy = multiturn.build_command("policy-only", "claude", "sid", resume=True, **options)
        self.assertIn("--resume", policy)
        self.assertNotIn("--plugin-dir", policy)

    def test_turn_verdicts_use_the_active_route_not_cumulative_loading(self) -> None:
        parsed = parity.parse_claude(stream([hook_response(REUSE_CONTEXT), READ, TEXT, RESULT]))
        self.assertEqual(parsed["active_route"], ["coding", "research"])
        turn = {"expected_loaded": ["research", "coding"]}
        self.assertEqual(multiturn.evaluate_turn("adaptive", turn, parsed, {"research", "coding"}, True), ([], []))
        # Cumulative loading of writing earlier does not make a coding-route turn pass a writing expectation.
        routing, _ = multiturn.evaluate_turn("adaptive", {"expected_loaded": ["research", "writing"]}, parsed, {"research", "coding", "writing"}, True)
        self.assertTrue(any("active route" in item for item in routing))
        # A reused route whose modules were never loaded in this session fails.
        routing, _ = multiturn.evaluate_turn("adaptive", turn, parsed, set(), True)
        self.assertTrue(any("never loaded" in item for item in routing))
        routing, _ = multiturn.evaluate_turn("adaptive", dict(turn, expect_adaptive="abstain"), parsed, {"research", "coding"}, True)
        self.assertTrue(any("must be strict" in item for item in routing))

    def test_scenario_files_are_well_formed(self) -> None:
        for name in ("multiturn.json", "prospective-multiturn.json"):
            document = json.loads((HERE / name).read_text(encoding="utf-8"))
            for scenario in document["scenarios"]:
                for turn in scenario["turns"]:
                    self.assertIn("prompt", turn)
                    if not turn.get("compact"):
                        self.assertLessEqual(set(turn["expected_loaded"]), set(parity.MODULE_FILES))


class ReportTests(unittest.TestCase):
    def test_report_redacts_identifiers_and_keeps_case_detail(self) -> None:
        summary = {
            "policy_commit": "abc", "plugin_version": "0.5.0", "worktree_dirty": False, "repetitions": 1,
            "by_client": {"claude-adaptive": {"total": 1, "passed": 0}}, "routing_contract_passed": 1, "task_passed": 0,
            "metrics": {"overall": {"claude-adaptive": {"cases": 1, "passed": 0, "routing_contract_passed": 1, "task_passed": 0, "adaptive_selected_cases": 1,
                                                          "route_skill_calls": 0, "cases_with_extra_modules": 0, "cases_with_missing_modules": 0, "median": {}}}},
            "results": [{"client": "claude-adaptive", "id": "x", "repetition": 1, "passed": False, "routing_passed": True, "task_passed": False,
                         "routing_failures": [], "task_failures": ["permission denials: toolu_01ABC in 12345678-1234-1234-1234-123456789abc"],
                         "route_calls": [], "active_route": ["coding", "research"], "adaptive_selected": True, "loaded_modules": ["research", "coding"],
                         "extra_modules": [], "missing_modules": [], "num_turns": 2, "usage": {}, "duration_seconds": 1.0,
                         "artifact_hashes": {"events_jsonl_sha256": "00"}}],
        }
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "summary.json"
            path.write_text(json.dumps(summary), encoding="utf-8")
            block = report.compact_parity(json.loads(path.read_text(encoding="utf-8")))
        case = block["cases"]["claude-adaptive"]["x#1"]
        self.assertIn("toolu_<redacted>", case["task_failures"][0])
        self.assertIn("<id>", case["task_failures"][0])
        self.assertEqual(case["active_route"], ["coding", "research"])
        self.assertFalse(case["passed"])
        self.assertTrue(case["routing_passed"])
        text = report.markdown({"matrix": block})
        self.assertIn("Routing contract", text)


if __name__ == "__main__":
    unittest.main()


class PreambleTests(unittest.TestCase):
    def test_preamble_text_before_tool_calls_is_not_the_final_answer(self) -> None:
        preamble = {"type": "assistant", "message": {"id": "m1", "content": [{"type": "text", "text": "Let me read the policy first."}]}}
        skill = {"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "r1", "name": "Skill", "input": {"skill": parity.ROUTE_SKILL, "args": "coding"}}]}}
        loaded = {"type": "user", "message": {"content": [{"type": "text", "text": "### Loaded PIRA module: research\n\nx\n\n### Loaded PIRA module: coding\n\nx\n"}]}}
        answer = {"type": "assistant", "message": {"id": "m3", "content": [{"type": "text", "text": "answer"}]}}
        parsed = parity.parse_claude(stream([preamble, skill, hook_response("PIRA routing is complete for this turn.", "PostToolUse"), loaded, dict(READ, message={"id": "m2", **READ["message"]}), answer, RESULT]))
        self.assertTrue(parsed["route_complete_before_work"])
        # A text-only message that is not followed by tool use in the same message is the answer.
        parsed = parity.parse_claude(stream([answer, skill, hook_response("PIRA routing is complete for this turn.", "PostToolUse"), loaded, RESULT]))
        self.assertFalse(parsed["route_complete_before_work"])

    def test_policy_only_parser_ignores_preamble_too(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            module = root / "policy" / "modules" / "CODING_STYLE.md"
            module.parent.mkdir(parents=True)
            module.write_text("PIRA_EVAL_MODULE::coding\n", encoding="utf-8")
            events = [
                {"type": "assistant", "message": {"id": "m1", "content": [{"type": "text", "text": "Reading the coding module first."}]}},
                {"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": str(module)}}]}},
                {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "PIRA_EVAL_MODULE::coding"}]}},
                {"type": "assistant", "message": {"id": "m2", "content": [{"type": "text", "text": "answer"}]}},
                {"type": "result", "subtype": "success", "result": "answer", "usage": {}},
            ]
            parsed = parity.parse_claude_policy_only(stream(events), root / "project", root / "policy")
        self.assertEqual(parsed["loaded_modules"], ["coding"])
        self.assertTrue(parsed["route_complete_before_work"])
