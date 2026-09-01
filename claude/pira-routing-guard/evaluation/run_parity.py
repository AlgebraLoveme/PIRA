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


def synthetic_policy(canonical: str, agent_dir: Path) -> str:
    root = agent_dir.resolve().as_posix()
    return canonical.replace("~/agent", root)


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
            if "PIRA routing is complete for this turn." in str(event.get("output", "")):
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

    for index, event in enumerate(events):
        event_type = str(event.get("type", ""))
        item = event.get("item") if isinstance(event.get("item"), dict) else {}
        item_type = str(item.get("type", ""))
        if event_type == "item.completed" and item_type == "command_execution":
            output = str(item.get("aggregated_output", ""))
            command = str(item.get("command", "")).replace("\\", "/")
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
            or (route_complete_at is not None and route_complete_at < first_work_at)
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
    }


def evaluate(client: str, scenario: dict[str, Any], parsed: dict[str, Any], exit_code: int) -> list[str]:
    failures: list[str] = []
    expected_route = sorted(scenario["expected_route"])
    expected_loaded = sorted(scenario["expected_loaded"])
    if exit_code != 0:
        failures.append(f"{client} exit code {exit_code}")
    if parsed["parse_errors"]:
        failures.append("stream parse errors: " + "; ".join(parsed["parse_errors"]))
    if client == "claude":
        if len(parsed["route_calls"]) != 1:
            failures.append(f"expected one route call, got {parsed['route_calls']}")
        elif sorted(parsed["route_calls"][0]) != expected_route:
            failures.append(f"route {parsed['route_calls'][0]} != {scenario['expected_route']}")
        result = parsed["result_event"]
        if not result or result.get("subtype") != "success" or result.get("is_error"):
            failures.append("Claude result was not successful")
        elif result.get("permission_denials"):
            failures.append(f"permission denials: {result['permission_denials']}")
    elif parsed.get("turn_failed"):
        failures.append("Codex turn failed")
    if sorted(parsed["loaded_modules"]) != expected_loaded:
        failures.append(f"loaded {parsed['loaded_modules']} != {scenario['expected_loaded']}")
    if not parsed["route_complete_before_work"]:
        failures.append("routing did not complete before task work or final answer")
    if parsed["hook_errors"]:
        failures.append("hook errors: " + "; ".join(parsed["hook_errors"]))
    if parsed.get("permission_denials"):
        failures.append(f"permission denials: {parsed['permission_denials']}")
    pattern = scenario.get("result_regex")
    if pattern and not re.search(pattern, parsed["final_text"]):
        failures.append(f"result did not match {pattern!r}")
    if not parsed["final_text"].strip():
        failures.append("final answer is empty")
    return failures


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


def command_for(client: str, executable: str, project: Path, args: argparse.Namespace) -> list[str]:
    if client == "claude":
        return [
            executable,
            "-p",
            args.prompt,
            "--plugin-dir",
            str(PLUGIN_ROOT),
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--tools",
            args.claude_tools,
            "--allowedTools",
            f"Skill({ROUTE_SKILL} *),Bash(*pira-routing-guard/*/run-routing-guard.sh *)",
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
    command = command_for(client, executable, project, args)
    env = os.environ.copy()
    env.update({"PIRA_AGENT_DIR": str(agent), "PIRA_ROUTING_STATE_DIR": str(state), "PYTHONUTF8": "1"})
    exit_code, stdout, stderr, elapsed = run_process(command, project, env, args.timeout)
    events_path = case_root / "events.jsonl"
    stderr_path = case_root / "stderr.txt"
    events_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")
    parsed = (
        parse_claude(stdout)
        if client == "claude"
        else parse_codex(stdout, tuple(str(path) for path in scenario.get("files", {})))
    )
    failures = evaluate(client, scenario, parsed, exit_code)
    return {
        "client": client,
        "repetition": repetition,
        "id": scenario["id"],
        "category": scenario.get("category", "uncategorized"),
        "passed": not failures,
        "failures": failures,
        "route_calls": parsed["route_calls"],
        "loaded_modules": parsed["loaded_modules"],
        "task_tools": parsed["task_tools"],
        "permission_denials": parsed.get("permission_denials", []),
        "duration_seconds": round(elapsed, 3),
        "usage": parsed["usage"],
        "artifact_dir": case_root.relative_to(artifact_root).as_posix(),
        "artifact_hashes": {
            "events_jsonl_sha256": sha256_file(events_path),
            "stderr_txt_sha256": sha256_file(stderr_path),
        },
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix", type=Path, default=HERE / "matrix.json")
    parser.add_argument("--client", choices=["claude", "codex", "both"], default="both")
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

    clients = ["claude", "codex"] if args.client == "both" else [args.client]
    executables = {client: shutil.which(client) for client in clients}
    missing_clients = [client for client, executable in executables.items() if not executable]
    if missing_clients:
        raise RuntimeError("missing client executable(s): " + ", ".join(missing_clients))
    artifact_root = args.artifact_root or Path(tempfile.mkdtemp(prefix="pira-parity-eval-"))
    artifact_root.mkdir(parents=True, exist_ok=True)

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
                    f"{client} repeat={repetition} {scenario['id']} loaded={result['loaded_modules']}",
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
        "schema_version": 1,
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
            "codex": {
                "ignore_user_config": True,
                "ignore_rules": True,
                "permission_profile": "pira_eval_read",
                "filesystem": {":minimal": "read", ":workspace_roots": {".": "read"}},
                "windows_sandbox": "elevated" if sys.platform == "win32" else None,
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
