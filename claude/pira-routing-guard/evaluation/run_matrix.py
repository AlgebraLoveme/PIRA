#!/usr/bin/env python3
"""Run synthetic Claude Code routing evaluations for the PIRA pilot."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROUTE_SKILL = "pira-routing-guard:route"
MODULE_FILES = {
    "user_profile": "USER.md",
    "research": "modules/RESEARCH_POLICY.md",
    "paper_reading": "modules/PAPER_READING.md",
    "coding": "modules/CODING_STYLE.md",
    "writing": "modules/SCIENTIFIC_WRITING.md",
    "public_figure": "modules/PUBLIC_FIGURE_STYLE.md",
    "explain": "modules/EXPLAIN_STYLE.md",
    "guidance": "modules/GUIDANCE.md",
    "maintenance": "modules/MAINTENANCE.md",
}
LOADED_PATTERN = re.compile(r"^### Loaded PIRA module: ([a-z_]+)\r?$", re.MULTILINE)


def route_tokens(arguments: str) -> list[str]:
    return [token.replace("-", "_") for token in re.split(r"[\s,]+", arguments.strip().lower()) if token]


def parse_stream(lines: list[str]) -> dict[str, Any]:
    events: list[dict[str, Any]] = []
    parse_errors: list[str] = []
    for number, line in enumerate(lines, start=1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
            if isinstance(value, dict):
                events.append(value)
            else:
                parse_errors.append(f"line {number}: event is not an object")
        except json.JSONDecodeError as exc:
            parse_errors.append(f"line {number}: {exc.msg}")

    route_calls: list[list[str]] = []
    loaded: list[str] = []
    task_tools: list[tuple[int, str]] = []
    route_complete_at: int | None = None
    hook_errors: list[str] = []
    result_event: dict[str, Any] | None = None

    for index, event in enumerate(events):
        if event.get("type") == "assistant":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            content = message.get("content") if isinstance(message.get("content"), list) else []
            for block in content:
                if not isinstance(block, dict) or block.get("type") != "tool_use":
                    continue
                name = str(block.get("name", ""))
                tool_input = block.get("input") if isinstance(block.get("input"), dict) else {}
                if name == "Skill" and tool_input.get("skill") == ROUTE_SKILL:
                    route_calls.append(route_tokens(str(tool_input.get("args", ""))))
                elif name != "Skill":
                    task_tools.append((index, name))
        if event.get("type") == "user":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            content = message.get("content")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        loaded.extend(LOADED_PATTERN.findall(str(block.get("text", ""))))
        if event.get("type") == "system" and event.get("subtype") == "hook_response":
            stderr = str(event.get("stderr", "")).strip()
            if stderr:
                hook_errors.append(stderr)
            if "PIRA routing is complete for this turn." in str(event.get("output", "")):
                route_complete_at = index
        if event.get("type") == "result":
            result_event = event

    first_task_at = task_tools[0][0] if task_tools else None
    return {
        "parse_errors": parse_errors,
        "route_calls": route_calls,
        "loaded_modules": list(dict.fromkeys(loaded)),
        "task_tools": [name for _, name in task_tools],
        "route_complete_before_task_tool": (
            first_task_at is None or (route_complete_at is not None and route_complete_at < first_task_at)
        ),
        "hook_errors": hook_errors,
        "result": result_event,
    }


def evaluate(scenario: dict[str, Any], parsed: dict[str, Any], exit_code: int) -> tuple[bool, list[str]]:
    failures: list[str] = []
    expected_route = sorted(scenario["expected_route"])
    expected_loaded = sorted(scenario["expected_loaded"])
    if exit_code != 0:
        failures.append(f"claude exit code {exit_code}")
    if parsed["parse_errors"]:
        failures.append("stream parse errors: " + "; ".join(parsed["parse_errors"]))
    if len(parsed["route_calls"]) != 1:
        failures.append(f"expected one route call, got {parsed['route_calls']}")
    elif sorted(parsed["route_calls"][0]) != expected_route:
        failures.append(f"route {parsed['route_calls'][0]} != {scenario['expected_route']}")
    if sorted(parsed["loaded_modules"]) != expected_loaded:
        failures.append(f"loaded {parsed['loaded_modules']} != {scenario['expected_loaded']}")
    if not parsed["route_complete_before_task_tool"]:
        failures.append("a task tool ran before routing completed")
    if parsed["hook_errors"]:
        failures.append("hook errors: " + "; ".join(parsed["hook_errors"]))
    result = parsed["result"]
    if not result or result.get("subtype") != "success" or result.get("is_error"):
        failures.append("Claude result was not successful")
    else:
        denials = result.get("permission_denials")
        if denials:
            failures.append(f"permission denials: {denials}")
        pattern = scenario.get("result_regex")
        if pattern and not re.search(pattern, str(result.get("result", ""))):
            failures.append(f"result did not match {pattern!r}")
    return not failures, failures


def write_synthetic_agent(root: Path) -> None:
    for module, relative in MODULE_FILES.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            f"# Synthetic {module} module\n\nEvaluation marker for `{module}`. "
            "Apply the user's request without inventing facts.\n",
            encoding="utf-8",
        )


def validate_matrix(document: dict[str, Any]) -> list[dict[str, Any]]:
    if document.get("schema_version") != 1 or not isinstance(document.get("scenarios"), list):
        raise ValueError("matrix must use schema_version 1 and contain a scenarios list")
    seen: set[str] = set()
    scenarios: list[dict[str, Any]] = []
    for item in document["scenarios"]:
        if not isinstance(item, dict):
            raise ValueError("each scenario must be an object")
        identifier = item.get("id")
        if not isinstance(identifier, str) or not identifier or identifier in seen:
            raise ValueError(f"scenario id is missing or duplicated: {identifier!r}")
        seen.add(identifier)
        for field in ("prompt", "expected_route", "expected_loaded"):
            if field not in item:
                raise ValueError(f"scenario {identifier!r} is missing {field}")
        unknown = (set(item["expected_route"]) - {"none"} - set(MODULE_FILES)) | (
            set(item["expected_loaded"]) - set(MODULE_FILES)
        )
        if unknown:
            raise ValueError(f"scenario {identifier!r} has unknown modules: {sorted(unknown)}")
        scenarios.append(item)
    return scenarios


def run_case(
    scenario: dict[str, Any],
    *,
    plugin_root: Path,
    claude: str,
    model: str,
    effort: str,
    tools: str,
    max_budget: float,
    timeout: int,
    artifact_root: Path,
) -> dict[str, Any]:
    case_root = artifact_root / scenario["id"]
    project = case_root / "project"
    agent = case_root / "agent"
    state = case_root / "state"
    project.mkdir(parents=True, exist_ok=True)
    write_synthetic_agent(agent)
    for relative, content in scenario.get("files", {}).items():
        path = project / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(str(content), encoding="utf-8")

    command = [
        claude,
        "-p",
        scenario["prompt"],
        "--plugin-dir",
        str(plugin_root),
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--tools",
        tools,
        "--allowedTools",
        f"Skill({ROUTE_SKILL} *),Bash(*pira-routing-guard/*/run-routing-guard.sh *)",
        "--model",
        model,
        "--effort",
        effort,
        "--output-format",
        "stream-json",
        "--include-hook-events",
        "--verbose",
        "--no-session-persistence",
        "--max-budget-usd",
        str(max_budget),
    ]
    environment = os.environ.copy()
    environment.update(
        {
            "PIRA_AGENT_DIR": str(agent),
            "PIRA_ROUTING_STATE_DIR": str(state),
            "PYTHONUTF8": "1",
        }
    )
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            cwd=project,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
        exit_code = completed.returncode
        stdout = completed.stdout
        stderr = completed.stderr
    except subprocess.TimeoutExpired as exc:
        exit_code = 124
        stdout = exc.stdout.decode("utf-8", "replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")
        stderr = exc.stderr.decode("utf-8", "replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
        stderr += f"\nTimed out after {timeout} seconds."
    elapsed = time.monotonic() - started
    raw_path = case_root / "events.jsonl"
    raw_path.write_text(stdout, encoding="utf-8")
    (case_root / "stderr.txt").write_text(stderr, encoding="utf-8")
    parsed = parse_stream(stdout.splitlines())
    passed, failures = evaluate(scenario, parsed, exit_code)
    result_event = parsed.get("result") or {}
    return {
        "id": scenario["id"],
        "category": scenario.get("category", "uncategorized"),
        "passed": passed,
        "failures": failures,
        "route_calls": parsed["route_calls"],
        "loaded_modules": parsed["loaded_modules"],
        "task_tools": parsed["task_tools"],
        "permission_denials": result_event.get("permission_denials", []),
        "cost_usd": result_event.get("total_cost_usd"),
        "duration_seconds": round(elapsed, 3),
        "artifact_dir": str(case_root),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    here = Path(__file__).resolve().parent
    parser.add_argument("--matrix", type=Path, default=here / "matrix.json")
    parser.add_argument("--scenario", action="append", help="Run only this scenario ID; repeatable")
    parser.add_argument("--max-cases", type=int, help="Run at most this many selected scenarios")
    parser.add_argument("--model", default="sonnet")
    parser.add_argument("--effort", default="low")
    parser.add_argument(
        "--tools",
        default="Skill,Bash,Read",
        help="Built-in Claude tools exposed during routing evaluation; use 'default' for all",
    )
    parser.add_argument("--max-budget", type=float, default=0.16, help="Per-case USD ceiling")
    parser.add_argument("--timeout", type=int, default=120, help="Per-case timeout in seconds")
    parser.add_argument("--artifact-root", type=Path, help="Directory for synthetic projects and raw events")
    parser.add_argument("--summary", type=Path, help="Optional summary JSON destination")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    document = json.loads(args.matrix.read_text(encoding="utf-8"))
    scenarios = validate_matrix(document)
    if args.scenario:
        requested = set(args.scenario)
        scenarios = [scenario for scenario in scenarios if scenario["id"] in requested]
        missing = requested - {scenario["id"] for scenario in scenarios}
        if missing:
            raise ValueError(f"unknown scenario IDs: {sorted(missing)}")
    if args.max_cases is not None:
        if args.max_cases < 1:
            raise ValueError("--max-cases must be positive")
        scenarios = scenarios[: args.max_cases]
    claude = shutil.which("claude")
    if not claude:
        raise RuntimeError("Claude Code executable was not found")
    plugin_root = Path(__file__).resolve().parents[1]
    artifact_root = args.artifact_root or Path(tempfile.mkdtemp(prefix="pira-routing-eval-"))
    artifact_root.mkdir(parents=True, exist_ok=True)

    results: list[dict[str, Any]] = []
    for index, scenario in enumerate(scenarios, start=1):
        result = run_case(
            scenario,
            plugin_root=plugin_root,
            claude=claude,
            model=args.model,
            effort=args.effort,
            tools=args.tools,
            max_budget=args.max_budget,
            timeout=args.timeout,
            artifact_root=artifact_root,
        )
        results.append(result)
        status = "PASS" if result["passed"] else "FAIL"
        print(
            f"[{index}/{len(scenarios)}] {status} {result['id']} "
            f"route={result['route_calls']} loaded={result['loaded_modules']} "
            f"cost={result['cost_usd']}",
            flush=True,
        )
        for failure in result["failures"]:
            print(f"  - {failure}", flush=True)

    total_cost = sum(float(result["cost_usd"] or 0) for result in results)
    summary = {
        "schema_version": 1,
        "model": args.model,
        "effort": args.effort,
        "tools": args.tools,
        "max_budget_usd_per_case": args.max_budget,
        "artifact_root": str(artifact_root),
        "total": len(results),
        "passed": sum(bool(result["passed"]) for result in results),
        "total_cost_usd": round(total_cost, 6),
        "results": results,
    }
    summary_path = args.summary or artifact_root / "summary.json"
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path.write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"SUMMARY {summary_path}")
    print(f"PASS {summary['passed']}/{summary['total']} COST_USD {summary['total_cost_usd']:.6f}")
    return 0 if summary["passed"] == summary["total"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
