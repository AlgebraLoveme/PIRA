#!/usr/bin/env python3
"""Re-score a run_parity or run_multiturn summary from its saved event streams.

Used when the evaluator's parsing changes after a run: the model outputs are untouched, only the
verdicts and derived metrics are recomputed. The rewritten summary records the rescoring.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import run_multiturn as multiturn
import run_parity as parity


def rescore_parity(summary: dict[str, Any], artifact_root: Path, scenarios: dict[str, dict[str, Any]]) -> dict[str, Any]:
    for result in summary["results"]:
        case_root = artifact_root / result["artifact_dir"]
        text = (case_root / "events.jsonl").read_text(encoding="utf-8", errors="replace")
        scenario = scenarios[result["id"]]
        client = result["client"]
        if client in parity.GUARDED_CLIENTS:
            parsed = parity.parse_claude(text)
        elif client == "claude-policy-only":
            project = case_root / "project"
            parsed = parity.parse_claude_policy_only(text, project, project / ".pira-eval-agent")
        else:
            continue
        exit_code = 0 if not any("exit code" in f for f in result.get("task_failures", []) + result.get("failures", [])) else 1
        routing_failures, task_failures = parity.evaluate(client, scenario, parsed, exit_code)
        expected = set(scenario["expected_loaded"])
        active = set(parsed.get("active_route") or [])
        result.update(
            {
                "passed": not routing_failures and not task_failures,
                "routing_passed": not routing_failures,
                "task_passed": not task_failures,
                "failures": routing_failures + task_failures,
                "routing_failures": routing_failures,
                "task_failures": task_failures,
                "active_route": parsed.get("active_route"),
                "route_calls": parsed["route_calls"],
                "loaded_modules": parsed["loaded_modules"],
                "task_tools": parsed["task_tools"],
                "adaptive_selected": bool(parsed.get("adaptive_selected")),
                "extra_modules": sorted(active - expected) if parsed.get("active_route") is not None else sorted(set(parsed["loaded_modules"]) - expected),
                "missing_modules": sorted(expected - active) if parsed.get("active_route") is not None else sorted(expected - set(parsed["loaded_modules"])),
            }
        )
    summary["passed"] = sum(bool(r["passed"]) for r in summary["results"])
    summary["routing_contract_passed"] = sum(bool(r.get("routing_passed")) for r in summary["results"])
    summary["task_passed"] = sum(bool(r.get("task_passed")) for r in summary["results"])
    summary["by_client"] = {
        client: {"total": sum(r["client"] == client for r in summary["results"]), "passed": sum(r["client"] == client and bool(r["passed"]) for r in summary["results"])}
        for client in summary["by_client"]
    }
    summary["metrics"] = parity.mode_metrics(summary["results"])
    summary["rescored"] = {"runner_sha256": parity.sha256_file(Path(parity.__file__)), "reason": "preamble text blocks no longer count as the final answer"}
    return summary


def rescore_multiturn(summary: dict[str, Any], artifact_root: Path, sessions: dict[str, dict[str, Any]]) -> dict[str, Any]:
    for result in summary["results"]:
        case_root = artifact_root / result["artifact_dir"]
        session = sessions[result["id"]]
        mode = result["mode"]
        cumulative: set[str] = set()
        for turn_result, turn in zip(result["turns"], session["turns"]):
            if turn.get("compact"):
                continue
            text = (case_root / f"turn-{turn_result['index']}.jsonl").read_text(encoding="utf-8", errors="replace")
            project = case_root / "project"
            parsed = parity.parse_claude(text) if mode != "policy-only" else parity.parse_claude_policy_only(text, project, project / ".pira-eval-agent")
            cumulative.update(parsed["loaded_modules"])
            completed = not any("timeout" in f for f in turn_result.get("task_failures", []))
            routing_failures, task_failures = multiturn.evaluate_turn(mode, turn, parsed, cumulative, completed)
            expected = set(turn.get("expected_loaded", []))
            active = set(parsed.get("active_route") or [])
            turn_result.update(
                {
                    "passed": not routing_failures and not task_failures,
                    "routing_passed": not routing_failures,
                    "task_passed": not task_failures,
                    "failures": routing_failures + task_failures,
                    "routing_failures": routing_failures,
                    "task_failures": task_failures,
                    "route_calls": parsed["route_calls"],
                    "active_route": parsed.get("active_route"),
                    "adaptive_selected": bool(parsed.get("adaptive_selected")),
                    "loaded_modules_this_turn": parsed["loaded_modules"],
                    "extra_modules": sorted(active - expected),
                    "missing_modules": sorted(expected - active) if not turn.get("accept_any_route") else [],
                }
            )
        result["passed"] = all(t["passed"] for t in result["turns"])
        result["routing_passed"] = all(t["routing_passed"] for t in result["turns"])
        result["task_passed"] = all(t["task_passed"] for t in result["turns"])
    summary["passed"] = sum(bool(r["passed"]) for r in summary["results"])
    summary["routing_contract_passed"] = sum(bool(r["routing_passed"]) for r in summary["results"])
    summary["task_passed"] = sum(bool(r["task_passed"]) for r in summary["results"])
    summary["rescored"] = {"runner_sha256": parity.sha256_file(Path(parity.__file__)), "reason": "preamble text blocks no longer count as the final answer"}
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", type=Path, required=True, help="summary.json produced by run_parity or run_multiturn")
    parser.add_argument("--scenarios", type=Path, required=True, help="the matrix or sessions file used for the run")
    parser.add_argument("--output", type=Path, help="destination (default: overwrite the summary)")
    args = parser.parse_args()
    summary = json.loads(args.summary.read_text(encoding="utf-8"))
    document = json.loads(args.scenarios.read_text(encoding="utf-8"))
    by_id = {item["id"]: item for item in document["scenarios"]}
    artifact_root = args.summary.parent
    if "modes" in summary:
        summary = rescore_multiturn(summary, artifact_root, by_id)
    else:
        summary = rescore_parity(summary, artifact_root, by_id)
    (args.output or args.summary).write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"RESCORED passed {summary['passed']}/{summary['total']} routing {summary['routing_contract_passed']} task {summary['task_passed']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
