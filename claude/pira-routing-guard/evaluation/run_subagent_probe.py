#!/usr/bin/env python3
"""Probe the parent/subagent routing boundary with one real Claude Code subagent.

The parent is asked to delegate a code review to a `general-purpose` subagent. In adaptive mode
the parent turn may be routed by the hook, but the subagent never receives UserPromptSubmit, so
it must route through the strict Skill path with its own isolated state. The probe records hook
names, the parent's adaptive selection, the subagent's route Skill call, the number of isolated
state directories, and the result, and writes a summary without identifiers.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

import run_parity as parity

PROMPT = (
    "Use the Task tool to launch one general-purpose subagent that reviews ./sample.py for correctness "
    "in one sentence without modifying files, then repeat its one-sentence answer verbatim."
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("strict", "adaptive"), default="adaptive")
    parser.add_argument("--model", default="sonnet")
    parser.add_argument("--effort", default="low")
    parser.add_argument("--max-budget", type=float, default=1.0)
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--artifact-root", type=Path)
    args = parser.parse_args()
    executable = shutil.which("claude")
    if not executable:
        raise RuntimeError("Claude Code executable was not found")
    root = args.artifact_root or Path(tempfile.mkdtemp(prefix="pira-subagent-probe-"))
    root.mkdir(parents=True, exist_ok=True)
    scenario = {"id": "subagent_probe", "prompt": PROMPT, "files": {"sample.py": "def first(items):\n    return items[1]\n"}}
    project, agent, state = parity.materialize_case(root, scenario)
    env = os.environ.copy()
    env.update(
        {
            "PIRA_POLICY_DIR": str(agent),
            "PIRA_ROUTING_STATE_DIR": str(state),
            "PYTHONUTF8": "1",
            "PIRA_ROUTING_GUARD_MODE": args.mode,
        }
    )
    command = [
        executable, "-p", PROMPT, "--plugin-dir", str(parity.PLUGIN_ROOT), "--setting-sources", "project",
        "--strict-mcp-config", "--tools", "Task,Skill,Bash,Read",
        "--allowedTools", f"Skill({parity.ROUTE_SKILL} *),Bash(*pira-routing-guard/*/run-routing-guard.sh *),Task",
        "--model", args.model, "--effort", args.effort, "--output-format", "stream-json", "--include-hook-events",
        "--verbose", "--no-session-persistence", "--max-budget-usd", str(args.max_budget),
    ]
    started = time.monotonic()
    exit_code, stdout, stderr, _ = parity.run_process(command, project, env, args.timeout)
    elapsed = time.monotonic() - started
    (root / "events.jsonl").write_text(stdout, encoding="utf-8")
    (root / "stderr.txt").write_text(stderr, encoding="utf-8")
    events, parse_errors = parity.parse_jsonl(stdout)

    hook_names: list[str] = []
    parent_adaptive = False
    subagent_route_calls: list[list[str]] = []
    parent_route_calls: list[list[str]] = []
    subagent_loaded: list[str] = []
    subagent_denials: list[str] = []
    result_event: dict[str, Any] | None = None
    for event in events:
        if event.get("type") == "system" and event.get("subtype") == "hook_started":
            hook_names.append(str(event.get("hook_name")))
        if event.get("type") == "system" and event.get("subtype") == "hook_response":
            context = parity.hook_additional_context(str(event.get("output", "")))
            if parity.ADAPTIVE_MARKER in context and not event.get("parent_tool_use_id"):
                parent_adaptive = True
            if "permissionDecision\\\":\\\"deny" in json.dumps(event.get("output", "")) or '"permissionDecision": "deny"' in str(event.get("output", "")):
                subagent_denials.append(str(event.get("hook_name")))
        if event.get("type") == "assistant":
            in_subagent = bool(event.get("parent_tool_use_id"))
            for block in event.get("message", {}).get("content", []):
                if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == "Skill":
                    tokens = parity.route_tokens(str((block.get("input") or {}).get("args", "")))
                    (subagent_route_calls if in_subagent else parent_route_calls).append(tokens)
        if event.get("type") == "user" and event.get("parent_tool_use_id"):
            for block in event.get("message", {}).get("content", []) if isinstance(event.get("message", {}).get("content"), list) else []:
                if isinstance(block, dict) and block.get("type") == "text":
                    subagent_loaded.extend(parity.ROUTE_HEADER.findall(str(block.get("text", ""))))
        if event.get("type") == "result":
            result_event = event
    state_dirs = sorted(path.name[:8] + "…" for path in state.iterdir() if path.is_dir()) if state.is_dir() else []
    summary = {
        "schema_version": 1,
        "mode": args.mode,
        "policy_commit": subprocess.run(["git", "rev-parse", "HEAD"], cwd=parity.REPO_ROOT, capture_output=True, text=True, check=True).stdout.strip(),
        "worktree_dirty": bool(subprocess.run(["git", "status", "--porcelain"], cwd=parity.REPO_ROOT, capture_output=True, text=True, check=True).stdout.strip()),
        "exit_code": exit_code,
        "parse_errors": parse_errors,
        "hook_names": hook_names,
        "parent_adaptive_selected": parent_adaptive,
        "parent_route_calls": parent_route_calls,
        "subagent_route_calls": subagent_route_calls,
        "subagent_loaded_modules": list(dict.fromkeys(subagent_loaded)),
        "isolated_state_directories": len(state_dirs),
        "result_success": bool(result_event and result_event.get("subtype") == "success" and not result_event.get("is_error")),
        "permission_denials": (result_event or {}).get("permission_denials", []),
        "num_turns": (result_event or {}).get("num_turns"),
        "usage": (result_event or {}).get("usage", {}),
        "duration_seconds": round(elapsed, 3),
        "events_jsonl_sha256": parity.sha256_file(root / "events.jsonl"),
    }
    checks = {
        "subagent_start_hook_fired": any(name.startswith("SubagentStart") for name in hook_names),
        "subagent_routed_with_skill": bool(subagent_route_calls),
        "subagent_loaded_before_completion": bool(subagent_loaded),
        "two_isolated_state_directories": len(state_dirs) >= 2,
        "subagent_stop_hook_fired": any(name.startswith("SubagentStop") for name in hook_names),
        "result_success": summary["result_success"],
        "no_permission_denials": not summary["permission_denials"],
    }
    summary["checks"] = checks
    summary["passed"] = all(checks.values())
    (root / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in summary.items() if key not in {"usage", "hook_names"}}, ensure_ascii=False))
    print(f"SUMMARY {root / 'summary.json'}")
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
