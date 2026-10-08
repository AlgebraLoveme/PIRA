#!/usr/bin/env python3
"""Smoke-test Claude's installed read rules with synthetic profiles and project files.

Uses the current Claude login plus an installer-generated --settings file. This
does not test loading the global user rule from an isolated CLAUDE_CONFIG_DIR.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALLER = REPO_ROOT / "assets" / "scripts" / "setup_pira_claude.py"


def install_config(agent: Path, config: Path, mode: str) -> None:
    command = [
        sys.executable, str(INSTALLER), "--agent-dir", str(agent),
        "--claude-dir", str(config), "--skip-tools",
    ]
    if mode == "fallback":
        command.extend(("--user-mode", "keep"))
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f"installer failed for {mode}: {result.stderr.strip()}")


def run_case(root: Path, mode: str, iteration: int, model: str) -> bool:
    agent = root / "agent"
    config = root / f"claude-{mode}"
    project = root / "project"
    install_config(agent, config, mode)
    profile = config / "pira" / "USER.md" if mode == "private" else agent / "USER.md"
    if mode == "fallback" and (config / "pira" / "USER.md").exists():
        raise RuntimeError("fallback test unexpectedly created a private profile")

    markers = {name: uuid.uuid4().hex for name in ("agents", "claude", "profile")}
    (project / "AGENTS.md").write_text(
        f"Synthetic project AGENTS marker: {markers['agents']}\n", encoding="utf-8"
    )
    (project / "CLAUDE.md").write_text(
        f"Synthetic project CLAUDE marker: {markers['claude']}\n", encoding="utf-8"
    )
    profile.write_text(f"Synthetic profile marker: {markers['profile']}\n", encoding="utf-8")
    prompt = (
        f"I own the synthetic test profile at {profile} and authorize access to it. "
        "First use the Read tool on that exact file. Then report its marker and "
        "both project instruction markers. Do not guess or omit any marker."
    )
    result = subprocess.run(
        ["claude", "-p", "--model", model, "--max-turns", "3", "--verbose",
         "--output-format", "stream-json", "--settings", str(config / "settings.json"), prompt],
        cwd=project, capture_output=True, text=True, timeout=180,
    )
    try:
        events = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        report = next(event for event in reversed(events) if event.get("type") == "result")
    except json.JSONDecodeError as error:
        raise RuntimeError(f"Claude did not return JSON lines for {mode} run {iteration}") from error
    except StopIteration as error:
        raise RuntimeError(f"Claude returned no result for {mode} run {iteration}") from error
    tool_uses = [
        block
        for event in events if event.get("type") == "assistant"
        for block in event.get("message", {}).get("content", [])
        if block.get("type") == "tool_use"
    ]
    profile_read = any(
        block.get("name") == "Read"
        and Path(block.get("input", {}).get("file_path", "")).resolve() == profile.resolve()
        for block in tool_uses
    )
    answer = report.get("result") or ""
    missing = [name for name, marker in markers.items() if marker not in answer]
    denials = report.get("permission_denials", [])
    passed = (result.returncode == 0 and not report.get("is_error")
              and profile_read and not missing and not denials)
    print(
        f"{'PASS' if passed else 'FAIL'} {mode} run {iteration}: "
        f"exit={result.returncode} read_profile={profile_read} "
        f"missing={missing} denials={len(denials)} "
        f"tools={[block.get('name') for block in tool_uses]} "
        f"terminal={report.get('terminal_reason')}"
    )
    if not passed:
        denied_tools = [item.get("tool_name") for item in denials]
        print(f"  is_error={report.get('is_error')} denied_tools={denied_tools}")
    return passed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="sonnet", help="Claude model alias for the live smoke.")
    parser.add_argument("--repeats", type=int, default=1, help="Fresh Claude sessions per profile mode.")
    parser.add_argument("--profile-mode", choices=("private", "fallback", "both"), default="both")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")

    with tempfile.TemporaryDirectory(prefix="pira-claude-smoke-") as directory:
        root = Path(directory)
        agent = root / "agent"
        (agent / "modules").mkdir(parents=True)
        shutil.copyfile(REPO_ROOT / "AGENTS.md", agent / "AGENTS.md")
        shutil.copyfile(
            REPO_ROOT / "modules" / "CODING_STYLE.md",
            agent / "modules" / "CODING_STYLE.md",
        )
        (root / "project").mkdir()
        modes = ("private", "fallback") if args.profile_mode == "both" else (args.profile_mode,)
        outcomes = [
            run_case(root, mode, iteration, args.model)
            for mode in modes for iteration in range(1, args.repeats + 1)
        ]
    print(f"Synthetic Claude smoke: {sum(outcomes)}/{len(outcomes)} passed")
    return 0 if all(outcomes) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1) from error
