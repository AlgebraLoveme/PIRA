#!/usr/bin/env python3
"""Run synthetic multi-turn Claude Code sessions to compare routing modes turn by turn.

Each session runs in one `claude -p --input-format stream-json` process, so follow-up turns
trigger UserPromptSubmit without a SessionStart, as in interactive use. A turn marked
`compact` sends `/compact`; a turn marked `resume` closes the process and resumes the
persisted session in a new one. Every prompt turn records two verdicts: the routing contract
(the active route at the end of the turn equals the turn's exact expected set, its modules were
loaded in the session, and routing completed before work) and the task outcome. Per-turn model
turns, usage, duration, route Skill calls and adaptive selections are recorded; raw streams stay
under the artifact root.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path
from typing import Any

import run_parity as parity

MODES = ("policy-only", "strict", "adaptive")


def client_for(mode: str) -> str:
    return {"policy-only": "claude-policy-only", "strict": "claude", "adaptive": "claude-adaptive"}[mode]


def build_command(
    mode: str,
    executable: str,
    session_id: str,
    *,
    resume: bool,
    model: str,
    effort: str,
    tools: str,
    max_budget: float,
) -> list[str]:
    command = [
        executable,
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        *(["--resume", session_id] if resume else ["--session-id", session_id]),
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--model",
        model,
        "--effort",
        effort,
        "--max-budget-usd",
        str(max_budget),
    ]
    if mode == "policy-only":
        command.extend(["--tools", "Read,Bash", "--allowedTools", "Read,Bash"])
    else:
        command.extend(
            [
                "--plugin-dir",
                str(parity.PLUGIN_ROOT),
                "--tools",
                tools,
                "--allowedTools",
                f"Skill({parity.ROUTE_SKILL} *),Bash(*pira-routing-guard/*/run-routing-guard.sh *)",
            ]
        )
    return command


class ClaudeProcess:
    """One stream-json Claude process with a background stdout reader."""

    def __init__(self, command: list[str], cwd: Path, env: dict[str, str]) -> None:
        self.process = subprocess.Popen(
            command,
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        self.lines: queue.Queue[str | None] = queue.Queue()
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def send(self, text: str) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps({"type": "user", "message": {"role": "user", "content": text}}) + "\n")
        self.process.stdin.flush()

    def read_turn(self, timeout: float, *, until_compact: bool = False) -> tuple[list[str], bool]:
        """Collect lines until this turn's result event (or a compaction status) or timeout."""
        collected: list[str] = []
        deadline = time.monotonic() + timeout
        settled_at: float | None = None
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return collected, False
            if settled_at is not None and time.monotonic() >= settled_at:
                return collected, True
            try:
                line = self.lines.get(timeout=min(remaining, 0.5))
            except queue.Empty:
                continue
            if line is None:
                return collected, False
            collected.append(line)
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict):
                continue
            if event.get("type") == "result":
                return collected, True
            if until_compact and event.get("type") == "system" and event.get("compact_result") == "success":
                settled_at = time.monotonic() + 2.0

    def close(self, timeout: float) -> tuple[int, str]:
        assert self.process.stdin is not None and self.process.stderr is not None
        try:
            self.process.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=10)
        return self.process.returncode, self.process.stderr.read()


def compaction_observed(lines: list[str]) -> bool:
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("type") == "system" and event.get("compact_result") == "success":
            return True
    return False


def evaluate_turn(
    mode: str,
    turn: dict[str, Any],
    parsed: dict[str, Any],
    cumulative_loaded: set[str],
    completed: bool,
) -> tuple[list[str], list[str]]:
    """Return (routing_contract_failures, task_failures) for one prompt turn."""
    routing: list[str] = []
    task: list[str] = []
    expected = sorted(turn.get("expected_loaded", []))
    any_route = bool(turn.get("accept_any_route"))
    adaptive = mode == "adaptive" and bool(parsed.get("adaptive_selected"))
    active = parsed.get("active_route")
    if not completed:
        task.append("turn did not produce a result event before the timeout")
    if parsed["parse_errors"]:
        routing.append("stream parse errors: " + "; ".join(parsed["parse_errors"]))
    if parsed["hook_errors"]:
        routing.append("hook errors: " + "; ".join(parsed["hook_errors"]))
    if mode == "adaptive":
        expectation = turn.get("expect_adaptive")
        if expectation == "select" and not adaptive:
            routing.append("adaptive routing did not select on a confident turn")
        if expectation == "abstain" and adaptive:
            routing.append("adaptive routing selected on a turn that must be strict")
    if mode != "policy-only":
        if adaptive and len(parsed["route_calls"]) > 1:
            routing.append(f"more than one route call after adaptive selection: {parsed['route_calls']}")
        if not adaptive and not any_route and len(parsed["route_calls"]) != 1:
            routing.append(f"expected one route call, got {parsed['route_calls']}")
        if not parsed["route_complete_before_work"]:
            routing.append("routing did not complete before task work or final answer")
    if not any_route:
        if mode == "policy-only":
            if sorted(cumulative_loaded & set(expected)) != expected:
                routing.append(f"required modules never loaded in this session: {sorted(set(expected) - cumulative_loaded)}")
        elif (active or []) != expected:
            routing.append(f"active route {active} != expected {expected}")
    if active and not set(active) <= cumulative_loaded:
        routing.append(f"active route modules never loaded in this session: {sorted(set(active) - cumulative_loaded)}")
    result = parsed["result_event"]
    if not result or result.get("subtype") != "success" or result.get("is_error"):
        task.append("Claude result was not successful")
    elif result.get("permission_denials"):
        task.append(f"permission denials: {result['permission_denials']}")
    pattern = turn.get("result_regex")
    if pattern and not re.search(pattern, parsed["final_text"]):
        task.append(f"result did not match {pattern!r}")
    return routing, task


def turn_metrics(parsed: dict[str, Any], elapsed: float) -> dict[str, Any]:
    usage = parsed.get("usage") or {}
    return {
        "model_turns": parsed.get("num_turns"),
        "context_tokens": parity.context_tokens(usage),
        "cache_creation_tokens": usage.get("cache_creation_input_tokens"),
        "cache_read_tokens": usage.get("cache_read_input_tokens"),
        "output_tokens": usage.get("output_tokens"),
        "duration_seconds": round(elapsed, 3),
        "api_equivalent_estimate_usd": (parsed.get("result_event") or {}).get("total_cost_usd"),
    }


def run_session(mode: str, scenario: dict[str, Any], repetition: int, artifact_root: Path, args: argparse.Namespace) -> dict[str, Any]:
    executable = shutil.which("claude")
    if not executable:
        raise RuntimeError("Claude Code executable was not found")
    case_root = artifact_root / mode / f"repeat-{repetition}" / scenario["id"]
    project, agent, state = parity.materialize_case(case_root, scenario)
    env = os.environ.copy()
    env.update(
        {
            "PIRA_POLICY_DIR": str(agent),
            "PIRA_ROUTING_STATE_DIR": str(state),
            "PYTHONUTF8": "1",
            "PIRA_ROUTING_GUARD_MODE": "adaptive" if mode == "adaptive" else "strict",
        }
    )
    session_id = str(uuid.uuid4())
    options = {"model": args.model, "effort": args.effort, "tools": args.tools, "max_budget": args.max_budget}
    process = ClaudeProcess(build_command(mode, executable, session_id, resume=False, **options), project, env)
    cumulative_loaded: set[str] = set()
    turns: list[dict[str, Any]] = []
    stderr_chunks: list[str] = []
    hashes: dict[str, str] = {}
    for index, turn in enumerate(scenario["turns"], start=1):
        if turn.get("resume"):
            _, stderr = process.close(args.timeout)
            stderr_chunks.append(stderr)
            process = ClaudeProcess(build_command(mode, executable, session_id, resume=True, **options), project, env)
        started = time.monotonic()
        process.send(turn["prompt"])
        lines, completed = process.read_turn(args.timeout, until_compact=bool(turn.get("compact")))
        elapsed = time.monotonic() - started
        text = "".join(lines)
        turn_path = case_root / f"turn-{index}.jsonl"
        turn_path.write_text(text, encoding="utf-8")
        hashes[turn_path.name] = parity.sha256_file(turn_path)
        if turn.get("compact"):
            observed = compaction_observed(lines)
            turns.append(
                {
                    "index": index,
                    "kind": "compact",
                    "passed": observed,
                    "routing_passed": observed,
                    "task_passed": True,
                    "failures": [] if observed else ["compaction was not observed"],
                }
            )
            continue
        parsed = parity.parse_claude(text) if mode != "policy-only" else parity.parse_claude_policy_only(text, project, agent)
        cumulative_loaded.update(parsed["loaded_modules"])
        routing_failures, task_failures = evaluate_turn(mode, turn, parsed, cumulative_loaded, completed)
        expected = set(turn.get("expected_loaded", []))
        active = set(parsed.get("active_route") or [])
        turns.append(
            {
                "index": index,
                "kind": "resume" if turn.get("resume") else turn.get("kind", "prompt"),
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
                "module_requiring": bool(expected) and turn.get("expect_adaptive") != "abstain",
                "task_tools": parsed["task_tools"],
                "metrics": turn_metrics(parsed, elapsed),
            }
        )
    _, stderr = process.close(args.timeout)
    stderr_chunks.append(stderr)
    (case_root / "stderr.txt").write_text("".join(stderr_chunks), encoding="utf-8")
    prompt_turns = [turn for turn in turns if turn.get("metrics")]

    def total(key: str) -> float:
        return sum(float(turn["metrics"][key] or 0) for turn in prompt_turns)

    return {
        "mode": mode,
        "client": client_for(mode),
        "repetition": repetition,
        "id": scenario["id"],
        "passed": all(turn["passed"] for turn in turns),
        "routing_passed": all(turn["routing_passed"] for turn in turns),
        "task_passed": all(turn["task_passed"] for turn in turns),
        "turns": turns,
        "totals": {
            "model_turns": int(total("model_turns")),
            "context_tokens": int(total("context_tokens")),
            "cache_creation_tokens": int(total("cache_creation_tokens")),
            "cache_read_tokens": int(total("cache_read_tokens")),
            "output_tokens": int(total("output_tokens")),
            "duration_seconds": round(total("duration_seconds"), 3),
            "route_skill_calls": sum(len(turn.get("route_calls") or []) for turn in prompt_turns),
            "adaptive_selected_turns": sum(bool(turn.get("adaptive_selected")) for turn in prompt_turns),
        },
        "artifact_hashes": hashes,
        "artifact_dir": case_root.relative_to(artifact_root).as_posix(),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenarios", type=Path, default=parity.HERE / "multiturn.json")
    parser.add_argument("--mode", action="append", choices=MODES, help="Routing mode to run; repeatable (default: all)")
    parser.add_argument("--scenario", action="append", help="Run only this session ID; repeatable")
    parser.add_argument("--repetitions", type=int, default=1)
    parser.add_argument("--model", default="sonnet")
    parser.add_argument("--effort", default="low")
    parser.add_argument("--tools", default="Skill,Bash,Read")
    parser.add_argument("--max-budget", type=float, default=1.5, help="Per-session USD ceiling")
    parser.add_argument("--timeout", type=int, default=180, help="Per-turn timeout in seconds")
    parser.add_argument("--artifact-root", type=Path)
    parser.add_argument("--summary", type=Path)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.repetitions < 1:
        raise ValueError("--repetitions must be positive")
    document = json.loads(args.scenarios.read_text(encoding="utf-8"))
    scenarios = document["scenarios"]
    if args.scenario:
        requested = set(args.scenario)
        scenarios = [scenario for scenario in scenarios if scenario["id"] in requested]
    modes = args.mode or list(MODES)
    artifact_root = args.artifact_root or Path(tempfile.mkdtemp(prefix="pira-multiturn-eval-"))
    artifact_root.mkdir(parents=True, exist_ok=True)
    results: list[dict[str, Any]] = []
    for mode in modes:
        for repetition in range(1, args.repetitions + 1):
            for scenario in scenarios:
                result = run_session(mode, scenario, repetition, artifact_root, args)
                results.append(result)
                print(
                    f"{'PASS' if result['passed'] else 'FAIL'} (routing={'ok' if result['routing_passed'] else 'FAIL'} "
                    f"task={'ok' if result['task_passed'] else 'FAIL'}) {mode} repeat={repetition} {scenario['id']} "
                    f"totals={result['totals']}",
                    flush=True,
                )
                for turn in result["turns"]:
                    if turn.get("failures"):
                        print(f"  turn {turn['index']}: " + "; ".join(turn["failures"]), flush=True)
    git_status = subprocess.run(["git", "status", "--porcelain"], cwd=parity.REPO_ROOT, capture_output=True, text=True, check=True).stdout
    summary = {
        "schema_version": 2,
        "policy_commit": subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=parity.REPO_ROOT, capture_output=True, text=True, check=True
        ).stdout.strip(),
        "plugin_version": json.loads((parity.PLUGIN_ROOT / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))["version"],
        "worktree_dirty": bool(git_status.strip()),
        "scenarios_sha256": parity.sha256_file(args.scenarios),
        "model": args.model,
        "effort": args.effort,
        "modes": modes,
        "repetitions": args.repetitions,
        "passed": sum(bool(result["passed"]) for result in results),
        "routing_contract_passed": sum(bool(result["routing_passed"]) for result in results),
        "task_passed": sum(bool(result["task_passed"]) for result in results),
        "total": len(results),
        "results": results,
    }
    summary_path = args.summary or artifact_root / "summary.json"
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"SUMMARY {summary_path}")
    print(f"PASS {summary['passed']}/{summary['total']}")
    return 0 if summary["passed"] == summary["total"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
