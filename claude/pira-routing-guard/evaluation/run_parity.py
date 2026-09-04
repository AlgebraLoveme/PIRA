#!/usr/bin/env python3
"""Run paired, synthetic PIRA policy-conformance evaluations for Claude and Codex."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
PLUGIN_ROOT = HERE.parent
REPO_ROOT = PLUGIN_ROOT.parents[1]
SETUP_SCRIPT = REPO_ROOT / "assets" / "scripts" / "setup_pira.py"
ROUTE_SKILL = "pira-routing-guard:route"
CLAUDE_CLIENTS = {"claude", "claude-policy-only", "claude-adaptive"}
GUARDED_CLIENTS = {"claude", "claude-adaptive"}
ADAPTIVE_MARKER = "PIRA adaptive route confirmed:"
ADAPTIVE_ROUTE = re.compile(re.escape(ADAPTIVE_MARKER) + r"\s*([a-z_, ]+?)\s*(?:\(|\.)")
ROUTE_COMPLETE_MARKER = "PIRA routing is complete for this turn."
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
MODULE_MARKER = re.compile(r"PIRA_EVAL_MODULE::([a-z_]+)")
ROUTE_HEADER = re.compile(r"^### Loaded PIRA module: ([a-z_]+)\r?$", re.MULTILINE)
PERMISSION_FAILURE = re.compile(
    r"(?i)(permission denied|access denied|zugriff verweigert|operation not permitted|sandbox.*denied)"
)
SKILL_ACCESS = re.compile(r"(?i)(?:^|[/\\])SKILL\.md(?:$|[\s'\"])")


def load_module(path: Path, name: str) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


matrix_runner = load_module(HERE / "run_matrix.py", "pira_parity_matrix_runner")
setup_pira = load_module(SETUP_SCRIPT, "pira_parity_setup")


def route_tokens(arguments: str) -> list[str]:
    return [token.replace("-", "_") for token in re.split(r"[\s,]+", arguments.strip().lower()) if token]


def unique(values: list[str]) -> list[str]:
    return list(dict.fromkeys(values))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def discover_skill_files(project: Path) -> list[Path]:
    """Find every locally discoverable Codex skill that could contaminate the control."""
    home = Path.home()
    codex_home = Path(os.environ.get("CODEX_HOME", home / ".codex"))
    roots = [
        codex_home / "skills",
        codex_home / "plugins",
        home / ".agents" / "skills",
    ]
    if os.name != "nt":
        roots.append(Path("/etc/codex/skills"))

    current = project.resolve()
    while True:
        roots.append(current / ".agents" / "skills")
        if current.parent == current:
            break
        current = current.parent

    found: dict[str, Path] = {}
    for root in roots:
        if not root.is_dir():
            continue
        for path in root.rglob("SKILL.md"):
            resolved = path.resolve()
            found[os.path.normcase(str(resolved))] = resolved
    return [found[key] for key in sorted(found)]


def disabled_skills_config(skill_files: list[Path]) -> str:
    entries = [
        "{path=" + json.dumps(str(path.resolve()), ensure_ascii=False) + ",enabled=false}"
        for path in skill_files
    ]
    # JSON string syntax is valid TOML basic-string syntax. Passing an array of TOML
    # inline tables at CLI precedence avoids modifying user configuration.
    return "skills.config=[" + ",".join(entries) + "]"


def skill_manifest_hash(skill_files: list[Path]) -> str:
    digest = hashlib.sha256()
    for path in skill_files:
        digest.update(os.path.normcase(str(path.resolve())).encode("utf-8"))
        digest.update(b"\0")
    return digest.hexdigest()


def synthetic_policy(canonical: str, agent_dir: Path) -> str:
    root = agent_dir.resolve().as_posix()
    return canonical.replace(setup_pira.DEFAULT_POLICY_DIR, root)


def materialize_case(case_root: Path, scenario: dict[str, Any]) -> tuple[Path, Path, Path]:
    project = case_root / "project"
    agent = project / ".pira-eval-agent"
    state = case_root / "state"
    project.mkdir(parents=True, exist_ok=True)
    agent.mkdir(parents=True, exist_ok=True)

    for module, relative in MODULE_FILES.items():
        path = agent / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            f"# Synthetic {module} module\n\nPIRA_EVAL_MODULE::{module}\n"
            "Apply the request without inventing facts.\n",
            encoding="utf-8",
        )

    canonical = (REPO_ROOT / "AGENTS.md").read_text(encoding="utf-8")
    policy = synthetic_policy(canonical, agent)
    (agent / "AGENTS.md").write_text(policy, encoding="utf-8")
    (project / "AGENTS.override.md").write_text(
        "# Synthetic PIRA parity policy\n\n"
        "For this evaluation, the policy below and its synthetic absolute paths replace any "
        "other PIRA bootstrap or module paths. Do not read a real user profile or real PIRA "
        "module tree.\n\n"
        + policy,
        encoding="utf-8",
    )
    (project / "CLAUDE.md").write_text(
        setup_pira.claude_managed_block(agent) + "\n",
        encoding="utf-8",
    )
    for relative, content in scenario.get("files", {}).items():
        path = project / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(str(content), encoding="utf-8")
    return project, agent, state


def parse_jsonl(text: str) -> tuple[list[dict[str, Any]], list[str]]:
    events: list[dict[str, Any]] = []
    errors: list[str] = []
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            errors.append(f"line {number}: {exc.msg}")
            continue
        if isinstance(value, dict):
            events.append(value)
        else:
            errors.append(f"line {number}: event is not an object")
    return events, errors


def hook_additional_context(output: str) -> str:
    """Return the additionalContext carried by a hook_response output, or an empty string."""
    try:
        decoded = json.loads(output)
    except (json.JSONDecodeError, TypeError):
        return ""
    if not isinstance(decoded, dict):
        return ""
    specific = decoded.get("hookSpecificOutput")
    context = specific.get("additionalContext") if isinstance(specific, dict) else None
    return context if isinstance(context, str) else ""


def parse_claude(text: str) -> dict[str, Any]:
    events, parse_errors = parse_jsonl(text)
    route_calls: list[list[str]] = []
    loaded: list[str] = []
    task_tools: list[str] = []
    route_complete_at: int | None = None
    first_task_at: int | None = None
    first_answer_at: int | None = None
    hook_errors: list[str] = []
    result_event: dict[str, Any] | None = None
    adaptive_selected = False
    active_route: list[str] | None = None

    for index, event in enumerate(events):
        if event.get("type") == "assistant":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            content = message.get("content") if isinstance(message.get("content"), list) else []
            for block in content:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    name = str(block.get("name", ""))
                    tool_input = block.get("input") if isinstance(block.get("input"), dict) else {}
                    if name == "Skill" and tool_input.get("skill") == ROUTE_SKILL:
                        route_calls.append(route_tokens(str(tool_input.get("args", ""))))
                        active_route = matrix_runner.expanded_route(route_calls[-1])
                    elif name != "Skill":
                        task_tools.append(name)
                        if first_task_at is None:
                            first_task_at = index
                elif block.get("type") == "text" and str(block.get("text", "")).strip():
                    first_answer_at = index if first_answer_at is None else first_answer_at
        if event.get("type") == "user":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            content = message.get("content")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        loaded.extend(ROUTE_HEADER.findall(str(block.get("text", ""))))
        if event.get("type") == "system" and event.get("subtype") == "hook_response":
            stderr = str(event.get("stderr", "")).strip()
            if stderr:
                hook_errors.append(stderr)
            output = str(event.get("output", ""))
            context = hook_additional_context(output)
            loaded.extend(ROUTE_HEADER.findall(context))
            match = ADAPTIVE_ROUTE.search(context)
            if match:
                adaptive_selected = True
                active_route = sorted(token.strip() for token in match.group(1).split(",") if token.strip())
            if ROUTE_COMPLETE_MARKER in output:
                route_complete_at = index
        if event.get("type") == "result":
            result_event = event

    first_work_at = min(
        value for value in (first_task_at, first_answer_at) if value is not None
    ) if first_task_at is not None or first_answer_at is not None else None
    return {
        "parse_errors": parse_errors,
        "route_calls": route_calls,
        "loaded_modules": unique(loaded),
        "task_tools": task_tools,
        "route_complete_before_work": (
            first_work_at is None or (route_complete_at is not None and route_complete_at < first_work_at)
        ),
        "hook_errors": hook_errors,
        "result_event": result_event,
        "final_text": str((result_event or {}).get("result", "")),
        "usage": (result_event or {}).get("usage", {}),
        "num_turns": (result_event or {}).get("num_turns"),
        "adaptive_selected": adaptive_selected,
        "active_route": active_route,
    }


def resolved_tool_path(value: object, project: Path) -> str:
    if not isinstance(value, str) or not value:
        return ""
    path = Path(os.path.expanduser(value))
    if not path.is_absolute():
        path = project / path
    return os.path.normcase(str(path.resolve(strict=False)))


def parse_claude_policy_only(
    text: str,
    project: Path,
    policy_dir: Path,
) -> dict[str, Any]:
    """Observe canonical Read-based routing without the routing-guard plugin."""
    events, parse_errors = parse_jsonl(text)
    module_by_path = {
        os.path.normcase(str((policy_dir / relative).resolve(strict=False))): module
        for module, relative in MODULE_FILES.items()
    }
    pending_modules: dict[str, str] = {}
    loaded: list[str] = []
    loaded_at: list[int] = []
    task_tools: list[str] = []
    first_work_at: int | None = None
    result_event: dict[str, Any] | None = None

    for index, event in enumerate(events):
        if event.get("type") == "assistant":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            content = message.get("content") if isinstance(message.get("content"), list) else []
            has_tool = any(isinstance(block, dict) and block.get("type") == "tool_use" for block in content)
            for block in content:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    name = str(block.get("name", ""))
                    tool_input = block.get("input") if isinstance(block.get("input"), dict) else {}
                    module = (
                        module_by_path.get(resolved_tool_path(tool_input.get("file_path"), project))
                        if name == "Read"
                        else None
                    )
                    if module:
                        pending_modules[str(block.get("id", ""))] = module
                    else:
                        task_tools.append(name)
                        first_work_at = index if first_work_at is None else first_work_at
                elif block.get("type") == "text" and str(block.get("text", "")).strip() and not has_tool:
                    first_work_at = index if first_work_at is None else first_work_at
        if event.get("type") == "user":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            content = message.get("content") if isinstance(message.get("content"), list) else []
            for block in content:
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    continue
                module = pending_modules.pop(str(block.get("tool_use_id", "")), None)
                if module and not block.get("is_error"):
                    marker = f"PIRA_EVAL_MODULE::{module}"
                    if marker in json.dumps(block.get("content", ""), ensure_ascii=False):
                        loaded.append(module)
                        loaded_at.append(index)
        if event.get("type") == "result":
            result_event = event

    route_complete_at = max(loaded_at, default=-1)
    return {
        "parse_errors": parse_errors,
        "route_calls": [],
        "loaded_modules": unique(loaded),
        "task_tools": task_tools,
        "route_complete_before_work": first_work_at is None or route_complete_at < first_work_at,
        "hook_errors": [],
        "result_event": result_event,
        "final_text": str((result_event or {}).get("result", "")),
        "usage": (result_event or {}).get("usage", {}),
        "num_turns": (result_event or {}).get("num_turns"),
        "adaptive_selected": False,
        "active_route": sorted(unique(loaded)) if loaded else None,
    }


def parse_codex(text: str, task_paths: tuple[str, ...] = ()) -> dict[str, Any]:
    events, parse_errors = parse_jsonl(text)
    loaded: list[str] = []
    task_tools: list[str] = []
    route_complete_at: tuple[int, int] | None = None
    first_work_at: tuple[int, int] | None = None
    final_text = ""
    final_message_at: int | None = None
    permission_denials: list[str] = []
    turn_failed = False
    usage: dict[str, Any] = {}
    unexpected_skill_access_count = 0

    for index, event in enumerate(events):
        event_type = str(event.get("type", ""))
        item = event.get("item") if isinstance(event.get("item"), dict) else {}
        item_type = str(item.get("type", ""))
        if event_type == "item.completed" and item_type == "command_execution":
            output = str(item.get("aggregated_output", ""))
            command = str(item.get("command", "")).replace("\\", "/")
            if SKILL_ACCESS.search(command):
                unexpected_skill_access_count += 1
            task_positions = [
                command.find(path.replace("\\", "/"))
                for path in task_paths
                if path.replace("\\", "/") in command
            ]
            touches_task = bool(task_positions)
            markers = MODULE_MARKER.findall(output)
            if markers:
                loaded.extend(markers)
                module_positions = []
                for marker in markers:
                    module_path = MODULE_FILES.get(marker, "").replace("\\", "/")
                    position = command.find(module_path)
                    if position < 0 and module_path:
                        position = command.find(Path(module_path).name)
                    module_positions.append(position)
                route_position = max(module_positions) if module_positions and min(module_positions) >= 0 else 0
                route_complete_at = (index, route_position)
                if touches_task:
                    task_tools.append("command_execution")
                    work_at = (index, min(task_positions))
                    first_work_at = work_at if first_work_at is None else min(first_work_at, work_at)
            elif item.get("status") == "completed":
                task_tools.append("command_execution")
                work_at = (index, min(task_positions) if task_positions else 0)
                first_work_at = work_at if first_work_at is None else min(first_work_at, work_at)
            if item.get("status") == "failed" and PERMISSION_FAILURE.search(output):
                permission_denials.append(output.strip()[:500])
        elif event_type == "item.completed" and item_type == "agent_message":
            final_text = str(item.get("text", ""))
            final_message_at = index
        elif event_type == "turn.failed":
            turn_failed = True
        elif event_type == "turn.completed" and isinstance(event.get("usage"), dict):
            usage = event["usage"]

    return {
        "parse_errors": parse_errors,
        "route_calls": [],
        "loaded_modules": unique(loaded),
        "task_tools": task_tools,
        "route_complete_before_work": (
            not loaded
            or (first_work_at is None and final_message_at is None)
            or (
                first_work_at is not None
                and route_complete_at is not None
                and route_complete_at < first_work_at
            )
            or (
                first_work_at is None
                and route_complete_at is not None
                and final_message_at is not None
                and route_complete_at[0] < final_message_at
            )
        ),
        "hook_errors": [],
        "result_event": None,
        "final_text": final_text,
        "usage": usage,
        "permission_denials": permission_denials,
        "turn_failed": turn_failed,
        "unexpected_skill_access_count": unexpected_skill_access_count,
    }


def evaluate(client: str, scenario: dict[str, Any], parsed: dict[str, Any], exit_code: int) -> tuple[list[str], list[str]]:
    """Return (routing_contract_failures, task_failures); a case passes only when both are empty."""
    routing: list[str] = []
    task: list[str] = []
    expected = sorted(scenario["expected_loaded"])
    any_route = bool(scenario.get("accept_any_route"))
    adaptive = client == "claude-adaptive" and bool(parsed.get("adaptive_selected"))
    if exit_code != 0:
        task.append(f"{client} exit code {exit_code}")
    if parsed["parse_errors"]:
        routing.append("stream parse errors: " + "; ".join(parsed["parse_errors"]))
    if parsed["hook_errors"]:
        routing.append("hook errors: " + "; ".join(parsed["hook_errors"]))

    if client in GUARDED_CLIENTS:
        if adaptive and len(parsed["route_calls"]) > 1:
            routing.append(f"more than one route call after adaptive selection: {parsed['route_calls']}")
        if not adaptive and not any_route and len(parsed["route_calls"]) != 1:
            routing.append(f"expected one route call, got {parsed['route_calls']}")
        if client == "claude-adaptive":
            expectation = scenario.get("expect_adaptive")
            if expectation == "select" and not adaptive:
                routing.append("adaptive routing did not select on a confident prompt")
            if expectation == "abstain" and adaptive:
                routing.append("adaptive routing selected on a prompt that must fall back to strict")
    active = parsed.get("active_route")
    loaded = sorted(parsed["loaded_modules"])
    if not any_route:
        if client in CLAUDE_CLIENTS or client == "codex":
            if (active or []) != expected:
                routing.append(f"active route {active} != expected {expected}")
        if loaded != expected:
            routing.append(f"loaded {loaded} != expected {expected}")
    elif active is not None and loaded and sorted(set(loaded)) != active:
        routing.append(f"loaded {loaded} != active route {active}")
    if not parsed["route_complete_before_work"]:
        routing.append("routing did not complete before task work or final answer")
    if parsed.get("unexpected_skill_access_count"):
        routing.append(f"unexpected external skill access count: {parsed['unexpected_skill_access_count']}")

    if client in CLAUDE_CLIENTS:
        result = parsed["result_event"]
        if not result or result.get("subtype") != "success" or result.get("is_error"):
            task.append("Claude result was not successful")
        elif result.get("permission_denials"):
            task.append(f"permission denials: {result['permission_denials']}")
    elif parsed.get("turn_failed"):
        task.append("Codex turn failed")
    if parsed.get("permission_denials"):
        task.append(f"permission denials: {parsed['permission_denials']}")
    pattern = scenario.get("result_regex")
    if pattern and not re.search(pattern, parsed["final_text"]):
        task.append(f"result did not match {pattern!r}")
    if not parsed["final_text"].strip():
        task.append("final answer is empty")
    return routing, task


def run_process(command: list[str], cwd: Path, env: dict[str, str], timeout: int) -> tuple[int, str, str, float]:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
        return completed.returncode, completed.stdout, completed.stderr, time.monotonic() - started
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout.decode("utf-8", "replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")
        stderr = exc.stderr.decode("utf-8", "replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
        return 124, stdout, stderr + f"\nTimed out after {timeout} seconds.", time.monotonic() - started


def command_for(
    client: str,
    executable: str,
    project: Path,
    args: argparse.Namespace,
    codex_skill_files: list[Path] | None = None,
) -> list[str]:
    if client in CLAUDE_CLIENTS:
        command = [
            executable,
            "-p",
            args.prompt,
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--tools",
            args.claude_tools if client in GUARDED_CLIENTS else "Read,Bash",
            "--model",
            args.claude_model,
            "--effort",
            args.claude_effort,
            "--output-format",
            "stream-json",
            "--include-hook-events",
            "--verbose",
            "--no-session-persistence",
            "--max-budget-usd",
            str(args.claude_max_budget),
        ]
        if client in GUARDED_CLIENTS:
            command[3:3] = ["--plugin-dir", str(PLUGIN_ROOT)]
            command.extend(
                ["--allowedTools", f"Skill({ROUTE_SKILL} *),Bash(*pira-routing-guard/*/run-routing-guard.sh *)"]
            )
        else:
            command.extend(["--allowedTools", "Read,Bash"])
        return command
    command = [
        executable,
        "exec",
        "--json",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--skip-git-repo-check",
        "-C",
        str(project),
        "--model",
        args.codex_model,
        "--config",
        f'model_reasoning_effort="{args.codex_effort}"',
        "--config",
        disabled_skills_config(codex_skill_files or []),
    ]
    if sys.platform == "win32":
        command.extend(["--config", 'windows.sandbox="elevated"'])
    command.extend(
        [
            "--config",
            'default_permissions="pira_eval_read"',
            "--config",
            'permissions.pira_eval_read.filesystem={":minimal"="read",":workspace_roots"={"."="read"}}',
            args.prompt,
        ]
    )
    return command


def run_case(
    client: str,
    executable: str,
    scenario: dict[str, Any],
    repetition: int,
    artifact_root: Path,
    args: argparse.Namespace,
) -> dict[str, Any]:
    case_root = artifact_root / client / f"repeat-{repetition}" / scenario["id"]
    project, agent, state = materialize_case(case_root, scenario)
    args.prompt = scenario["prompt"]
    codex_skill_files = discover_skill_files(project) if client == "codex" else []
    command = command_for(client, executable, project, args, codex_skill_files)
    env = os.environ.copy()
    env.update(
        {
            "PIRA_POLICY_DIR": str(agent),
            "PIRA_ROUTING_STATE_DIR": str(state),
            "PYTHONUTF8": "1",
            "PIRA_ROUTING_GUARD_MODE": "adaptive" if client == "claude-adaptive" else "strict",
        }
    )
    exit_code, stdout, stderr, elapsed = run_process(command, project, env, args.timeout)
    events_path = case_root / "events.jsonl"
    stderr_path = case_root / "stderr.txt"
    events_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")
    if client in GUARDED_CLIENTS:
        parsed = parse_claude(stdout)
    elif client == "claude-policy-only":
        parsed = parse_claude_policy_only(stdout, project, agent)
    else:
        parsed = parse_codex(stdout, tuple(str(path) for path in scenario.get("files", {})))
    routing_failures, task_failures = evaluate(client, scenario, parsed, exit_code)
    expected = set(scenario["expected_loaded"])
    active = set(parsed.get("active_route") or [])
    return {
        "client": client,
        "repetition": repetition,
        "id": scenario["id"],
        "category": scenario.get("category", "uncategorized"),
        "passed": not routing_failures and not task_failures,
        "routing_passed": not routing_failures,
        "task_passed": not task_failures,
        "failures": routing_failures + task_failures,
        "routing_failures": routing_failures,
        "task_failures": task_failures,
        "active_route": parsed.get("active_route"),
        "module_requiring": bool(expected) and scenario.get("expect_adaptive") != "abstain",
        "route_calls": parsed["route_calls"],
        "loaded_modules": parsed["loaded_modules"],
        "task_tools": parsed["task_tools"],
        "permission_denials": parsed.get("permission_denials", []),
        "unexpected_skill_access_count": parsed.get("unexpected_skill_access_count", 0),
        "duration_seconds": round(elapsed, 3),
        "usage": parsed["usage"],
        "num_turns": parsed.get("num_turns"),
        "adaptive_selected": bool(parsed.get("adaptive_selected")),
        "extra_modules": sorted(active - expected) if parsed.get("active_route") is not None else sorted(set(parsed["loaded_modules"]) - expected),
        "missing_modules": sorted(expected - active) if parsed.get("active_route") is not None else sorted(expected - set(parsed["loaded_modules"])),
        "artifact_dir": case_root.relative_to(artifact_root).as_posix(),
        "artifact_hashes": {
            "events_jsonl_sha256": sha256_file(events_path),
            "stderr_txt_sha256": sha256_file(stderr_path),
        },
    }


def context_tokens(usage: dict[str, Any]) -> int:
    return sum(int(usage.get(key) or 0) for key in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))


def median_or_none(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def measure(result: dict[str, Any]) -> dict[str, float]:
    usage = result.get("usage") or {}
    return {
        "model_turns": float(result.get("num_turns") or 0),
        "context_tokens": float(context_tokens(usage)),
        "cache_creation_tokens": float(usage.get("cache_creation_input_tokens") or 0),
        "cache_read_tokens": float(usage.get("cache_read_input_tokens") or 0),
        "output_tokens": float(usage.get("output_tokens") or 0),
        "duration_seconds": float(result.get("duration_seconds") or 0),
    }


def summarize_rows(rows: list[dict[str, Any]], baseline: dict[tuple[str, int], dict[str, float]]) -> dict[str, Any]:
    measured = [measure(row) for row in rows]
    summary: dict[str, Any] = {
        "cases": len(rows),
        "passed": sum(bool(row["passed"]) for row in rows),
        "routing_contract_passed": sum(bool(row.get("routing_passed")) for row in rows),
        "task_passed": sum(bool(row.get("task_passed")) for row in rows),
        "route_skill_calls": sum(len(row.get("route_calls") or []) for row in rows),
        "adaptive_selected_cases": sum(bool(row.get("adaptive_selected")) for row in rows),
        "cases_with_extra_modules": sum(bool(row.get("extra_modules")) for row in rows),
        "cases_with_missing_modules": sum(bool(row.get("missing_modules")) for row in rows),
        "median": {key: median_or_none([m[key] for m in measured]) for key in measured[0]} if measured else {},
    }
    deltas = [
        {key: measure(row)[key] - baseline[(row["id"], row["repetition"])][key] for key in measure(row)}
        for row in rows
        if (row["id"], row["repetition"]) in baseline
    ]
    if deltas:
        summary["paired_cases"] = len(deltas)
        summary["paired_delta_vs_policy_only_median"] = {key: median_or_none([d[key] for d in deltas]) for key in deltas[0]}
    return summary


def mode_metrics(results: list[dict[str, Any]]) -> dict[str, Any]:
    """Per-client summaries over all cases, plus the adaptive-selected subset compared with strict on the same cases."""
    by_client: dict[str, list[dict[str, Any]]] = {}
    for result in results:
        by_client.setdefault(result["client"], []).append(result)
    baseline = {(r["id"], r["repetition"]): measure(r) for r in by_client.get("claude-policy-only", [])}
    metrics: dict[str, Any] = {"overall": {client: summarize_rows(rows, baseline) for client, rows in by_client.items()}}

    adaptive_rows = by_client.get("claude-adaptive", [])
    selected = [row for row in adaptive_rows if row.get("adaptive_selected")]
    module_requiring = [row for row in adaptive_rows if row.get("module_requiring")]
    if adaptive_rows:
        keys = {(row["id"], row["repetition"]) for row in selected}
        strict_same = [row for row in by_client.get("claude", []) if (row["id"], row["repetition"]) in keys]
        subset: dict[str, Any] = {
            "selected_cases": len(selected),
            "module_requiring_cases": len(module_requiring),
            "coverage": round(sum(bool(row.get("adaptive_selected")) for row in module_requiring) / len(module_requiring), 3) if module_requiring else None,
            "adaptive": summarize_rows(selected, baseline) if selected else {},
            "strict_on_same_cases": summarize_rows(strict_same, baseline) if strict_same else {},
        }
        adaptive_delta = (subset["adaptive"].get("paired_delta_vs_policy_only_median") or {})
        strict_delta = (subset["strict_on_same_cases"].get("paired_delta_vs_policy_only_median") or {})
        if adaptive_delta and strict_delta and strict_delta.get("context_tokens"):
            subset["context_overhead_reduction_vs_strict"] = round(1 - adaptive_delta["context_tokens"] / strict_delta["context_tokens"], 3)
            subset["model_turn_reduction_vs_strict_median"] = (subset["strict_on_same_cases"]["median"]["model_turns"] - subset["adaptive"]["median"]["model_turns"])
        metrics["adaptive_selected_subset"] = subset
    return metrics


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix", type=Path, default=HERE / "matrix.json")
    parser.add_argument(
        "--client",
        choices=["claude", "claude-policy-only", "claude-adaptive", "claude-ab", "claude-modes", "codex", "both"],
        default="both",
    )
    parser.add_argument("--repetitions", type=int, default=2)
    parser.add_argument("--scenario", action="append")
    parser.add_argument("--max-cases", type=int)
    parser.add_argument("--claude-model", default="sonnet")
    parser.add_argument("--claude-effort", default="low")
    parser.add_argument("--claude-tools", default="Skill,Bash,Read")
    parser.add_argument("--claude-max-budget", type=float, default=0.25)
    parser.add_argument("--codex-model", default="gpt-5.6-sol")
    parser.add_argument("--codex-effort", default="low")
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--artifact-root", type=Path)
    parser.add_argument("--summary", type=Path)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.repetitions < 1:
        raise ValueError("--repetitions must be positive")
    document = json.loads(args.matrix.read_text(encoding="utf-8"))
    scenarios = matrix_runner.validate_matrix(document)
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

    if args.client == "both":
        clients = ["claude", "codex"]
    elif args.client == "claude-ab":
        clients = ["claude-policy-only", "claude"]
    elif args.client == "claude-modes":
        clients = ["claude-policy-only", "claude", "claude-adaptive"]
    else:
        clients = [args.client]
    executables = {
        client: shutil.which("claude" if client.startswith("claude") else client)
        for client in clients
    }
    missing_clients = [client for client, executable in executables.items() if not executable]
    if missing_clients:
        raise RuntimeError("missing client executable(s): " + ", ".join(missing_clients))
    artifact_root = args.artifact_root or Path(tempfile.mkdtemp(prefix="pira-parity-eval-"))
    artifact_root.mkdir(parents=True, exist_ok=True)
    codex_skill_files = discover_skill_files(artifact_root) if "codex" in clients else []

    results: list[dict[str, Any]] = []
    total = len(clients) * args.repetitions * len(scenarios)
    counter = 0
    for client in clients:
        for repetition in range(1, args.repetitions + 1):
            for scenario in scenarios:
                counter += 1
                result = run_case(client, str(executables[client]), scenario, repetition, artifact_root, args)
                results.append(result)
                print(
                    f"[{counter}/{total}] {'PASS' if result['passed'] else 'FAIL'} "
                    f"(routing={'ok' if result['routing_passed'] else 'FAIL'} task={'ok' if result['task_passed'] else 'FAIL'}) "
                    f"{client} repeat={repetition} {scenario['id']} route={result['active_route']}",
                    flush=True,
                )
                for failure in result["failures"]:
                    print(f"  - {failure}", flush=True)

    versions: dict[str, str] = {}
    for client, executable in executables.items():
        completed = subprocess.run(
            [str(executable), "--version"], capture_output=True, text=True, encoding="utf-8", errors="replace"
        )
        versions[client] = (completed.stdout or completed.stderr).strip()
    git_status = subprocess.run(
        ["git", "status", "--porcelain"], cwd=REPO_ROOT, capture_output=True, text=True, check=True
    ).stdout
    summary = {
        "schema_version": 2,
        "policy_commit": subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=REPO_ROOT, capture_output=True, text=True, check=True
        ).stdout.strip(),
        "plugin_version": json.loads(
            (PLUGIN_ROOT / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8")
        )["version"],
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "client_versions": versions,
        "models": {
            "claude": {"model": args.claude_model, "effort": args.claude_effort},
            "codex": {"model": args.codex_model, "effort": args.codex_effort},
        },
        "repetitions": args.repetitions,
        "client_config": {
            "claude": {
                "tools": args.claude_tools,
                "setting_sources": "project",
                "strict_mcp_config": True,
                "session_persistence": False,
            },
            "claude-policy-only": {
                "tools": "Read,Bash",
                "setting_sources": "project",
                "strict_mcp_config": True,
                "session_persistence": False,
                "routing_guard": False,
            },
            "claude-adaptive": {
                "tools": args.claude_tools,
                "setting_sources": "project",
                "strict_mcp_config": True,
                "session_persistence": False,
                "routing_guard_mode": "adaptive",
            },
            "codex": {
                "ignore_user_config": True,
                "ignore_rules": True,
                "permission_profile": "pira_eval_read",
                "filesystem": {":minimal": "read", ":workspace_roots": {".": "read"}},
                "windows_sandbox": "elevated" if sys.platform == "win32" else None,
                "external_skills_disabled": True,
                "disabled_skill_count": len(codex_skill_files),
                "disabled_skill_manifest_sha256": skill_manifest_hash(codex_skill_files),
            },
        },
        "source_hashes": {
            "matrix_sha256": sha256_file(args.matrix),
            "runner_sha256": sha256_file(Path(__file__)),
            "protocol_sha256": sha256_file(HERE / "PARITY_PROTOCOL.md"),
        },
        "worktree_dirty": bool(git_status.strip()),
        "artifact_layout": "Result artifact_dir values are relative to the runtime artifact root printed by the runner.",
        "total": len(results),
        "passed": sum(bool(result["passed"]) for result in results),
        "by_client": {
            client: {
                "total": sum(result["client"] == client for result in results),
                "passed": sum(result["client"] == client and bool(result["passed"]) for result in results),
            }
            for client in clients
        },
        "metrics": mode_metrics(results),
        "routing_contract_passed": sum(bool(result.get("routing_passed")) for result in results),
        "task_passed": sum(bool(result.get("task_passed")) for result in results),
        "results": results,
    }
    summary_path = args.summary or artifact_root / "summary.json"
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"SUMMARY {summary_path}")
    print(f"PASS {summary['passed']}/{summary['total']}")
    return 0 if summary["passed"] == summary["total"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
