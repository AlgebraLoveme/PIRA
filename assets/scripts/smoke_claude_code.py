#!/usr/bin/env python3
"""End-to-end smoke test for the installed PIRA Claude Code bridge.

Runs a few short non-interactive ``claude -p`` sessions against the user's real
Claude Code configuration and checks observable behavior: the imported policy is
in context, module routing names the right files, and an ordinary shell command
follows the Claude-native shell rule. Each probe costs one short model call, so
the script is opt-in and never part of setup or the unit tests.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path

VERIFY_TOKEN = "31415926535897932384626433832795"


@dataclass(frozen=True)
class Probe:
    name: str
    prompt: str
    allowed_tools: str
    required: tuple[str, ...]
    forbidden: tuple[str, ...] = ()


PROBES = (
    Probe(
        name="policy_import",
        prompt="Reply with exactly the PIRA verification token and nothing else.",
        # No file or shell tools; Skill stays available so user-installed hooks that
        # require a skill call before answering cannot block a tool-less answer.
        allowed_tools="Skill",
        required=(VERIFY_TOKEN,),
    ),
    Probe(
        name="module_routing",
        prompt=(
            "A user asks you to fix a failing Python unit test in their project. Following the PIRA "
            "Module Loading and Routing rules, load every module file that applies with the Read tool, "
            "then reply with one line per loaded file in the form `LOADED <path>` and nothing else."
        ),
        allowed_tools="Read,Skill",
        required=("CODING_STYLE.md", "RESEARCH_POLICY.md"),
    ),
    Probe(
        name="shell_routing",
        prompt=(
            "Run `git --version` following PIRA's shell rules, then reply with exactly the command line "
            "you executed and nothing else."
        ),
        allowed_tools="Bash(git --version),Skill",
        required=("git --version",),
        forbidden=("pira_ctx",),
    ),
)


@dataclass
class ProbeResult:
    name: str
    passed: bool
    seconds: float
    result_excerpt: str
    missing: list[str]
    unexpected: list[str]
    error: str | None = None


def evaluate(probe: Probe, result_text: str) -> tuple[list[str], list[str]]:
    """Return (missing required phrases, present forbidden phrases)."""
    missing = [phrase for phrase in probe.required if phrase not in result_text]
    unexpected = [phrase for phrase in probe.forbidden if phrase in result_text]
    return missing, unexpected


def parse_claude_json(stdout: str) -> str:
    """Extract the final assistant text from ``claude -p --output-format json`` output."""
    payload = json.loads(stdout)
    if isinstance(payload, list):
        payload = next((item for item in reversed(payload) if isinstance(item, dict) and "result" in item), {})
    result = payload.get("result", "")
    return result if isinstance(result, str) else json.dumps(result)


def command_output(command: list[str], cwd: Path | None = None) -> str:
    completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace", cwd=cwd)
    return completed.stdout.strip()


def run_probe(claude: str, probe: Probe, model: str, timeout: int, workdir: Path) -> ProbeResult:
    command = [
        claude,
        "-p",
        probe.prompt,
        "--output-format",
        "json",
        "--model",
        model,
        "--max-turns",
        "8",
        "--no-session-persistence",
    ]
    command.extend(["--allowedTools", probe.allowed_tools])
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=workdir,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return ProbeResult(probe.name, False, time.monotonic() - started, "", list(probe.required), [], "timeout")
    seconds = time.monotonic() - started
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()[-400:]
        return ProbeResult(probe.name, False, seconds, detail, list(probe.required), [], f"claude exited {completed.returncode}")
    try:
        result_text = parse_claude_json(completed.stdout)
    except (json.JSONDecodeError, AttributeError) as exc:
        return ProbeResult(probe.name, False, seconds, completed.stdout[-400:], list(probe.required), [], f"unparseable output: {exc}")
    missing, unexpected = evaluate(probe, result_text)
    return ProbeResult(probe.name, not missing and not unexpected, seconds, result_text[:400], missing, unexpected)


def policy_commit(agent_dir: Path) -> str | None:
    if not shutil.which("git") or not (agent_dir / ".git").exists():
        return None
    commit = command_output(["git", "-C", str(agent_dir), "rev-parse", "--short", "HEAD"])
    return commit or None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Smoke-test the installed PIRA Claude Code bridge with real claude -p sessions.")
    parser.add_argument("--agent-dir", default="~/agent", help="PIRA agent directory whose policy should be loaded (default: ~/agent).")
    parser.add_argument("--model", default="sonnet", help="Claude model alias for the probes (default: sonnet).")
    parser.add_argument("--timeout", type=int, default=300, help="Seconds allowed per probe (default: 300).")
    parser.add_argument("--only", action="append", choices=[probe.name for probe in PROBES], help="Run only the named probe; repeatable.")
    parser.add_argument("--report", default=None, help="Write the JSON report to this path in addition to stdout.")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    claude = shutil.which("claude")
    if claude is None:
        print("ERROR: claude CLI not found on PATH", file=sys.stderr)
        return 2
    agent_dir = Path(os.path.expanduser(args.agent_dir))
    selected = [probe for probe in PROBES if not args.only or probe.name in args.only]

    report: dict[str, object] = {
        "timestamp": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "platform": platform.platform(),
        "claude_version": command_output([claude, "--version"]),
        "model": args.model,
        "agent_dir": str(agent_dir),
        "policy_commit": policy_commit(agent_dir),
        "probes": [],
    }
    print(f"claude: {report['claude_version']}  model: {args.model}  policy: {report['policy_commit'] or 'unknown'}")

    results: list[ProbeResult] = []
    with tempfile.TemporaryDirectory(prefix="pira_smoke_") as temporary:
        workdir = Path(temporary)
        for probe in selected:
            result = run_probe(claude, probe, args.model, args.timeout, workdir)
            results.append(result)
            status = "PASS" if result.passed else "FAIL"
            detail = result.error or (f"missing={result.missing} unexpected={result.unexpected}" if not result.passed else "")
            print(f"{status}: {probe.name} ({result.seconds:.0f}s) {detail}".rstrip())
            if not result.passed:
                print(f"  result: {result.result_excerpt!r}")
    report["probes"] = [asdict(result) for result in results]
    report["passed"] = all(result.passed for result in results)

    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"report: {args.report}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
