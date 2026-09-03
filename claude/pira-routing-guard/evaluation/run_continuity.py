#!/usr/bin/env python3
"""Run a synthetic persisted-session resume probe for the PIRA Claude pilot."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import tempfile
import uuid
from pathlib import Path
from typing import Any

import run_matrix as matrix


def build_command(
    claude: str,
    plugin_root: Path,
    prompt: str,
    session_id: str,
    *,
    resume: bool,
    model: str,
    effort: str,
    tools: str,
    max_budget: float,
) -> list[str]:
    session_option = ["--resume", session_id] if resume else ["--session-id", session_id]
    return [
        claude,
        "-p",
        prompt,
        *session_option,
        "--plugin-dir",
        str(plugin_root),
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--tools",
        tools,
        "--allowedTools",
        f"Skill({matrix.ROUTE_SKILL} *),Bash(*pira-routing-guard/*/run-routing-guard.sh *)",
        "--model",
        model,
        "--effort",
        effort,
        "--output-format",
        "stream-json",
        "--include-hook-events",
        "--verbose",
        "--max-budget-usd",
        str(max_budget),
    ]


def run_turn(command: list[str], project: Path, environment: dict[str, str], timeout: int) -> dict[str, Any]:
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
    parsed = matrix.parse_stream(completed.stdout.splitlines())
    return {
        "exit_code": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
        "parsed": parsed,
    }


def turn_summary(
    turn: dict[str, Any], expected_route: list[str], expected_loaded: list[str]
) -> dict[str, Any]:
    passed, failures = matrix.evaluate(
        {"expected_route": expected_route, "expected_loaded": expected_loaded},
        turn["parsed"],
        turn["exit_code"],
    )
    result = turn["parsed"].get("result") or {}
    return {
        "passed": passed,
        "failures": failures,
        "route_calls": turn["parsed"]["route_calls"],
        "loaded_modules": turn["parsed"]["loaded_modules"],
        "task_tools": turn["parsed"]["task_tools"],
        "api_equivalent_estimate_usd": result.get("total_cost_usd"),
    }


def compaction_summary(turn: dict[str, Any]) -> dict[str, Any]:
    events: list[dict[str, Any]] = []
    parse_errors: list[str] = []
    for number, line in enumerate(turn["stdout"].splitlines(), start=1):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as exc:
            parse_errors.append(f"line {number}: {exc.msg}")
            continue
        if isinstance(event, dict):
            events.append(event)

    compact_success = any(
        event.get("type") == "system"
        and event.get("subtype") == "status"
        and event.get("compact_result") == "success"
        for event in events
    )
    pending_after_compact = any(
        event.get("type") == "system"
        and event.get("subtype") == "hook_response"
        and event.get("hook_name") == "SessionStart:compact"
        and "PIRA routing is pending" in str(event.get("output", ""))
        for event in events
    )
    post_compact_observed = any(
        event.get("hook_event") == "PostCompact"
        or "PostCompact" in json.dumps(event, ensure_ascii=False)
        for event in events
    )
    failures = list(parse_errors)
    if turn["exit_code"] != 0:
        failures.append(f"Claude exit code {turn['exit_code']}")
    if not compact_success:
        failures.append("manual compaction did not report success")
    if not post_compact_observed:
        failures.append("PostCompact hook execution was not observed")
    if not pending_after_compact:
        failures.append("SessionStart:compact did not restore pending routing context")
    return {
        "passed": not failures,
        "failures": failures,
        "compact_success": compact_success,
        "post_compact_observed": post_compact_observed,
        "pending_after_compact": pending_after_compact,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="sonnet")
    parser.add_argument("--effort", default="low")
    parser.add_argument("--tools", default="Skill,Bash,Read")
    parser.add_argument("--max-budget", type=float, default=0.16)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--artifact-root", type=Path)
    parser.add_argument("--resume-session", help="Resume this existing synthetic session instead of creating one")
    parser.add_argument("--resume-prompt", help="Prompt for --resume-session mode")
    parser.add_argument("--expected-route", help="Comma-separated expected route for resume mode")
    parser.add_argument("--expected-loaded", help="Comma-separated expected loaded modules for resume mode")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    claude = shutil.which("claude")
    if not claude:
        raise RuntimeError("Claude Code executable was not found")
    plugin_root = Path(__file__).resolve().parents[1]
    artifact_root = args.artifact_root or Path(tempfile.mkdtemp(prefix="pira-routing-continuity-"))
    project = artifact_root / "project"
    agent = artifact_root / "agent"
    state = artifact_root / "state"
    if args.resume_session:
        required = {
            "--artifact-root": args.artifact_root,
            "--resume-prompt": args.resume_prompt,
            "--expected-route": args.expected_route,
            "--expected-loaded": args.expected_loaded,
        }
        missing = [name for name, value in required.items() if value is None]
        if missing:
            raise ValueError("resume mode requires " + ", ".join(missing))
        if not project.is_dir() or not agent.is_dir():
            raise ValueError("resume artifact root must contain project and agent directories")
        environment = os.environ.copy()
        environment.update(
            {
                "PIRA_POLICY_DIR": str(agent),
                "PIRA_ROUTING_STATE_DIR": str(state),
                "PYTHONUTF8": "1",
            }
        )
        turn = run_turn(
            build_command(
                claude,
                plugin_root,
                args.resume_prompt,
                args.resume_session,
                resume=True,
                model=args.model,
                effort=args.effort,
                tools=args.tools,
                max_budget=args.max_budget,
            ),
            project,
            environment,
            args.timeout,
        )
        expected_route = [value for value in args.expected_route.split(",") if value]
        expected_loaded = [value for value in args.expected_loaded.split(",") if value]
        summary = turn_summary(turn, expected_route, expected_loaded)
        suffix = uuid.uuid4().hex[:8]
        (artifact_root / f"resume-{suffix}.jsonl").write_text(turn["stdout"], encoding="utf-8")
        (artifact_root / f"resume-{suffix}-stderr.txt").write_text(turn["stderr"], encoding="utf-8")
        print(
            f"RESUME {'PASS' if summary['passed'] else 'FAIL'} route={summary['route_calls']} "
            f"loaded={summary['loaded_modules']} estimate_usd={summary['api_equivalent_estimate_usd']}"
        )
        for failure in summary["failures"]:
            print(f"  - {failure}")
        return 0 if summary["passed"] else 1

    project.mkdir(parents=True, exist_ok=True)
    matrix.write_synthetic_agent(agent)
    (project / "sample.py").write_text("def first(items):\n    return items[1]\n", encoding="utf-8")
    (project / "draft.md").write_text("The result that was measured increased by five percent.\n", encoding="utf-8")

    environment = os.environ.copy()
    environment.update(
        {
            "PIRA_POLICY_DIR": str(agent),
            "PIRA_ROUTING_STATE_DIR": str(state),
            "PYTHONUTF8": "1",
        }
    )
    session_id = str(uuid.uuid4())
    first = run_turn(
        build_command(
            claude,
            plugin_root,
            "Review ./sample.py for correctness in one sentence. Do not modify files.",
            session_id,
            resume=False,
            model=args.model,
            effort=args.effort,
            tools=args.tools,
            max_budget=args.max_budget,
        ),
        project,
        environment,
        args.timeout,
    )
    second = run_turn(
        build_command(
            claude,
            plugin_root,
            "Now polish ./draft.md into one concise sentence without adding claims. Do not modify files.",
            session_id,
            resume=True,
            model=args.model,
            effort=args.effort,
            tools=args.tools,
            max_budget=args.max_budget,
        ),
        project,
        environment,
        args.timeout,
    )
    compact = run_turn(
        build_command(
            claude,
            plugin_root,
            "/compact",
            session_id,
            resume=True,
            model=args.model,
            effort=args.effort,
            tools=args.tools,
            max_budget=args.max_budget,
        ),
        project,
        environment,
        args.timeout,
    )
    third = run_turn(
        build_command(
            claude,
            plugin_root,
            "Explain in one sentence why zero is an even number.",
            session_id,
            resume=True,
            model=args.model,
            effort=args.effort,
            tools=args.tools,
            max_budget=args.max_budget,
        ),
        project,
        environment,
        args.timeout,
    )
    (artifact_root / "turn-1.jsonl").write_text(first["stdout"], encoding="utf-8")
    (artifact_root / "turn-2.jsonl").write_text(second["stdout"], encoding="utf-8")
    (artifact_root / "compact.jsonl").write_text(compact["stdout"], encoding="utf-8")
    (artifact_root / "turn-3.jsonl").write_text(third["stdout"], encoding="utf-8")
    (artifact_root / "stderr.txt").write_text(
        first["stderr"] + second["stderr"] + compact["stderr"] + third["stderr"],
        encoding="utf-8",
    )

    summaries = [
        turn_summary(first, ["coding"], ["research", "coding"]),
        turn_summary(second, ["writing"], ["research", "writing"]),
        turn_summary(third, ["explain"], ["explain"]),
    ]
    compact_result = compaction_summary(compact)
    output = {
        "schema_version": 1,
        "session_id": session_id,
        "artifact_root": str(artifact_root),
        "persisted_for_resume": True,
        "compaction": compact_result,
        "turns": summaries,
        "passed": compact_result["passed"] and all(turn["passed"] for turn in summaries),
    }
    (artifact_root / "summary.json").write_text(
        json.dumps(output, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    for index, turn in enumerate(summaries, start=1):
        status = "PASS" if turn["passed"] else "FAIL"
        print(
            f"TURN {index} {status} route={turn['route_calls']} loaded={turn['loaded_modules']} "
            f"estimate_usd={turn['api_equivalent_estimate_usd']}"
        )
        for failure in turn["failures"]:
            print(f"  - {failure}")
    print(
        f"COMPACT {'PASS' if compact_result['passed'] else 'FAIL'} "
        f"success={compact_result['compact_success']} "
        f"post_hook={compact_result['post_compact_observed']} "
        f"pending={compact_result['pending_after_compact']}"
    )
    for failure in compact_result["failures"]:
        print(f"  - {failure}")
    print(f"SESSION_ID {session_id}")
    print(f"SUMMARY {artifact_root / 'summary.json'}")
    return 0 if output["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
