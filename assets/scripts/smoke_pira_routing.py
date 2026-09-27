#!/usr/bin/env python3
"""Compare natural PIRA module routing on the same synthetic review task.

This is an optional authenticated smoke test, not a CI test. It exercises the
shared policy as a project AGENTS.md; it does not verify Claude's global rule.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALLER = REPO_ROOT / "assets" / "scripts" / "setup_pira_claude.py"
MODULES = ("CODING_STYLE.md", "RESEARCH_POLICY.md")
SOURCE = "def average(values):\n    return sum(values) // len(values)\n"
PROMPT = "Review calc.py for correctness and maintainability. Give concise findings, without editing files."


def fixture(root: Path, include_claude: bool) -> tuple[Path, Path, Path | None]:
    agent = root / "agent"
    modules = agent / "modules"
    modules.mkdir(parents=True)
    policy = (REPO_ROOT / "AGENTS.md").read_text(encoding="utf-8")
    policy = policy.replace("~/agent/modules/", f"{modules}/")
    (agent / "AGENTS.md").write_text(policy, encoding="utf-8")
    for name in MODULES:
        shutil.copyfile(REPO_ROOT / "modules" / name, modules / name)

    project = root / "project"
    project.mkdir()
    (project / "AGENTS.md").write_text(policy, encoding="utf-8")
    (project / "calc.py").write_text(SOURCE, encoding="utf-8")

    if not include_claude:
        return project, modules, None
    config = root / "claude-settings"
    setup = subprocess.run(
        [sys.executable, str(INSTALLER), "--agent-dir", str(agent),
         "--claude-dir", str(config), "--skip-tools"],
        capture_output=True, text=True, timeout=30,
    )
    if setup.returncode:
        raise RuntimeError(f"temporary Claude setup failed (exit {setup.returncode})")
    return project, modules, config / "settings.json"


def events_from_jsonl(output: str) -> list[dict]:
    events = []
    for line in output.splitlines():
        if line.strip():
            event = json.loads(line)
            if isinstance(event, dict):
                events.append(event)
    return events


def claude_evidence(events: list[dict], project: Path, modules: Path) -> dict:
    results = {
        block.get("tool_use_id"): block
        for event in events if event.get("type") == "user"
        for block in event.get("message", {}).get("content", [])
        if block.get("type") == "tool_result" and block.get("tool_use_id")
    }
    reads = set()
    shell_outputs = []
    for event in events:
        if event.get("type") != "assistant":
            continue
        for block in event.get("message", {}).get("content", []):
            if block.get("type") != "tool_use":
                continue
            result = results.get(block.get("id"))
            if not result or result.get("is_error"):
                continue
            inputs = block.get("input", {})
            if block.get("name") == "Read":
                reads.add(Path(inputs.get("file_path", "")).resolve())
            elif block.get("name") == "Bash":
                output = result.get("content", "")
                if isinstance(output, str):
                    shell_outputs.append((inputs.get("command", ""), output))
    code_lines = [line.strip() for line in SOURCE.splitlines()]
    shell_code_read = any(
        "calc.py" in command and all(line in output for line in code_lines)
        for command, output in shell_outputs
    )
    report = next((event for event in reversed(events) if event.get("type") == "result"), {})
    return {
        "code_read": (project / "calc.py").resolve() in reads or shell_code_read,
        "modules_read": {
            name: (modules / name).resolve() in reads or any(
                name in command and f"# {name.removesuffix('.md')}" in output
                for command, output in shell_outputs
            )
            for name in MODULES
        },
        "denials": len(report.get("permission_denials") or []),
        "completed": report.get("subtype") == "success" and not report.get("is_error"),
    }


def codex_evidence(events: list[dict]) -> dict:
    commands = [
        event.get("item", {})
        for event in events
        if event.get("type") == "item.completed"
        and event.get("item", {}).get("type") == "command_execution"
        and event.get("item", {}).get("exit_code") == 0
    ]
    outputs = [item.get("aggregated_output", "") for item in commands]
    combined = "\n".join(value for value in outputs if isinstance(value, str))
    code_lines = [line.strip() for line in SOURCE.splitlines()]
    return {
        "code_read": any(
            "calc.py" in item.get("command", "")
            and all(line in item.get("aggregated_output", "") for line in code_lines)
            for item in commands
        ),
        "code_command": any("calc.py" in item.get("command", "") for item in commands),
        "modules_read": {name: f"# {name.removesuffix('.md')}" in combined for name in MODULES},
        "denials": None,
        "completed": any(event.get("type") == "turn.completed" for event in events)
        and not any(event.get("type") in ("turn.failed", "error") for event in events),
        "command_outputs": len(outputs),
    }


def run_claude(project: Path, modules: Path, settings: Path, model: str) -> dict:
    command = [
        "claude", "-p", "--model", model, "--permission-mode", "auto",
        "--max-turns", "8", "--no-session-persistence", "--verbose",
        "--output-format", "stream-json", "--settings", str(settings), PROMPT,
    ]
    result = subprocess.run(command, cwd=project, capture_output=True, text=True, timeout=240)
    evidence = claude_evidence(events_from_jsonl(result.stdout), project, modules)
    return {"client": "claude", "exit": result.returncode, **evidence}


def run_codex(project: Path, model: str | None) -> dict:
    command = [
        "codex", "exec", "--json", "--ephemeral", "--sandbox", "read-only",
        "--skip-git-repo-check", "--ignore-user-config",
    ]
    if model:
        command.extend(("--model", model))
    command.append(PROMPT)
    result = subprocess.run(command, cwd=project, capture_output=True, text=True, timeout=240)
    evidence = codex_evidence(events_from_jsonl(result.stdout))
    return {"client": "codex", "exit": result.returncode, **evidence}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client", choices=("both", "claude", "codex"), default="both")
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--claude-model", default="sonnet")
    parser.add_argument("--codex-model", help="Use Codex's CLI default when omitted.")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")

    with tempfile.TemporaryDirectory(prefix="pira-routing-smoke-") as directory:
        project, modules, settings = fixture(Path(directory), args.client != "codex")
        results = []
        for iteration in range(1, args.repeats + 1):
            if args.client in ("both", "claude"):
                if settings is None:
                    raise RuntimeError("Claude settings were not generated")
                results.append({"run": iteration, **run_claude(project, modules, settings, args.claude_model)})
            if args.client in ("both", "codex"):
                results.append({"run": iteration, **run_codex(project, args.codex_model)})
        if (project / "calc.py").read_text(encoding="utf-8") != SOURCE:
            raise RuntimeError("review changed synthetic source")
    for result in results:
        result["passed"] = (
            result["exit"] == 0 and result["completed"] and result["code_read"]
            and all(result["modules_read"].values()) and result["denials"] in (None, 0)
        )
        print(json.dumps(result, sort_keys=True))
    return 0 if all(result["passed"] for result in results) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1) from error
