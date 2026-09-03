"""Unit tests for the adaptive-mode additions to run_parity.py and for run_multiturn.py."""

from __future__ import annotations

import importlib.util
import json
import sys
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


def hook_response(context: str) -> dict:
    output = json.dumps({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": context}})
    return {"type": "system", "subtype": "hook_response", "hook_name": "UserPromptSubmit", "output": output}


ADAPTIVE_CONTEXT = (
    "PIRA adaptive routing selected: research, coding. Apply the module context below to this turn.\n\n"
    "### Loaded PIRA module: research\n\nx\n\n### Loaded PIRA module: coding\n\nx\n\n"
    "PIRA routing is complete for this turn."
)


def adaptive_stream(extra_events: list[dict] | None = None, context: str = ADAPTIVE_CONTEXT) -> str:
    events = [
        hook_response(context),
        {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "a.py"}}]}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "answer"}]}},
        {"type": "result", "subtype": "success", "result": "answer", "num_turns": 2,
         "usage": {"input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 20}},
    ]
    return "\n".join(json.dumps(event) for event in (extra_events or []) + events)


class AdaptiveParityTests(unittest.TestCase):
    def test_hook_output_modules_count_as_loaded_before_work(self) -> None:
        parsed = parity.parse_claude(adaptive_stream())
        self.assertTrue(parsed["adaptive_selected"])
        self.assertEqual(parsed["loaded_modules"], ["research", "coding"])
        self.assertTrue(parsed["route_complete_before_work"])
        self.assertEqual(parsed["route_calls"], [])
        self.assertEqual(parsed["num_turns"], 2)

    def test_adaptive_evaluation_accepts_supersets_but_not_misses(self) -> None:
        parsed = parity.parse_claude(adaptive_stream())
        scenario = {"id": "x", "expected_loaded": ["coding", "research"]}
        self.assertEqual(parity.evaluate("claude-adaptive", scenario, parsed, 0), [])
        superset = {"id": "x", "expected_loaded": ["research"]}
        self.assertEqual(parity.evaluate("claude-adaptive", superset, parsed, 0), [])
        miss = {"id": "x", "expected_loaded": ["research", "coding", "explain"]}
        failures = parity.evaluate("claude-adaptive", miss, parsed, 0)
        self.assertTrue(any("missed required modules ['explain']" in failure for failure in failures))
        must_abstain = {"id": "x", "expected_loaded": ["research", "coding"], "expect_adaptive": "abstain"}
        self.assertTrue(any("must fall back" in failure for failure in parity.evaluate("claude-adaptive", must_abstain, parsed, 0)))

    def test_strict_client_still_requires_exact_route(self) -> None:
        parsed = parity.parse_claude(adaptive_stream())
        scenario = {"id": "x", "expected_loaded": ["research", "coding"]}
        failures = parity.evaluate("claude", scenario, parsed, 0)
        self.assertTrue(any("expected one route call" in failure for failure in failures))

    def test_adaptive_client_is_guarded_with_mode_env_and_metrics_are_paired(self) -> None:
        args = SimpleNamespace(prompt="p", claude_tools="Skill,Bash,Read", claude_model="sonnet", claude_effort="low", claude_max_budget=0.1)
        command = parity.command_for("claude-adaptive", "claude", Path("."), args)
        self.assertIn("--plugin-dir", command)
        self.assertNotIn("--plugin-dir", parity.command_for("claude-policy-only", "claude", Path("."), args))
        results = [
            {"client": "claude-policy-only", "id": "a", "repetition": 1, "passed": True, "num_turns": 2, "duration_seconds": 4.0,
             "usage": {"input_tokens": 4, "cache_creation_input_tokens": 15000, "cache_read_input_tokens": 32000, "output_tokens": 100}},
            {"client": "claude-adaptive", "id": "a", "repetition": 1, "passed": True, "num_turns": 2, "duration_seconds": 5.0,
             "adaptive_selected": True, "extra_modules": ["explain"], "route_calls": [],
             "usage": {"input_tokens": 4, "cache_creation_input_tokens": 16000, "cache_read_input_tokens": 32500, "output_tokens": 120}},
        ]
        metrics = parity.mode_metrics(results)
        adaptive = metrics["claude-adaptive"]
        self.assertEqual(adaptive["adaptive_selected_cases"], 1)
        self.assertEqual(adaptive["cases_with_extra_modules"], 1)
        self.assertEqual(adaptive["paired_delta_vs_policy_only_median"]["model_turns"], 0)
        self.assertEqual(adaptive["paired_delta_vs_policy_only_median"]["context_tokens"], 1500)


class MultiturnRunnerTests(unittest.TestCase):
    def test_commands_select_mode_specific_tools_and_resume(self) -> None:
        options = {"model": "sonnet", "effort": "low", "tools": "Skill,Bash,Read", "max_budget": 1.0}
        strict = multiturn.build_command("strict", "claude", "sid", resume=False, **options)
        self.assertIn("--session-id", strict)
        self.assertIn("--plugin-dir", strict)
        self.assertIn("stream-json", strict)
        policy = multiturn.build_command("policy-only", "claude", "sid", resume=True, **options)
        self.assertIn("--resume", policy)
        self.assertNotIn("--plugin-dir", policy)

    def test_turn_evaluation_uses_cumulative_loaded_modules_and_expectations(self) -> None:
        parsed = parity.parse_claude(adaptive_stream())
        turn = {"expected_loaded": ["research", "coding"], "expect_adaptive": "select"}
        self.assertEqual(multiturn.evaluate_turn("adaptive", turn, parsed, {"research", "coding"}, True), [])
        stale = {"expected_loaded": ["research", "writing"], "expect_adaptive": "abstain"}
        failures = multiturn.evaluate_turn("adaptive", stale, parsed, {"research", "coding"}, True)
        self.assertTrue(any("must be strict" in failure for failure in failures))
        self.assertTrue(any("never loaded" in failure for failure in failures))
        # A continuation turn that reused an earlier route loads nothing new but still passes.
        reuse_context = (
            "PIRA adaptive routing selected: research, coding. All selected modules are already loaded and "
            "unchanged in this session. PIRA routing is complete for this turn."
        )
        reuse = parity.parse_claude(adaptive_stream(context=reuse_context))
        self.assertTrue(reuse["adaptive_selected"])
        self.assertEqual(reuse["loaded_modules"], [])
        self.assertEqual(multiturn.evaluate_turn("adaptive", turn, reuse, {"research", "coding"}, True), [])

    def test_scenario_file_is_well_formed(self) -> None:
        document = json.loads((HERE / "multiturn.json").read_text(encoding="utf-8"))
        for scenario in document["scenarios"]:
            for turn in scenario["turns"]:
                self.assertIn("prompt", turn)
                if not turn.get("compact"):
                    self.assertIn("expected_loaded", turn)
                    self.assertLessEqual(set(turn["expected_loaded"]), set(parity.MODULE_FILES))


if __name__ == "__main__":
    unittest.main()
