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
import shlex
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
    required_tool_calls: tuple[tuple[str, str], ...] = ()
    require_policy_import: bool = False


PROBES = (
    Probe(
        name="policy_import",
        prompt=(
            "I am checking that my own PIRA instructions loaded. My user-level CLAUDE.md imports PIRA's "
            "AGENTS.md, which has a `## Verification Token` heading. Quote the number under that heading."
        ),
        # No file or shell tools; Skill stays available so user-installed hooks that
        # require a skill call before answering cannot block a tool-less answer.
        allowed_tools="Skill",
        required=(VERIFY_TOKEN,),
        require_policy_import=True,
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
        required_tool_calls=(("Read", "CODING_STYLE.md"), ("Read", "RESEARCH_POLICY.md")),
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
        required_tool_calls=(("Bash", "git --version"),),
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
    tool_calls: list[str]
    policy_import_loaded: bool | None
    error: str | None = None


def tool_call_text(name: str, tool_input: object) -> str:
    return f"{name} {json.dumps(tool_input, sort_keys=True, ensure_ascii=False)}"


def evaluate(probe: Probe, result_text: str, tool_calls: list[str]) -> tuple[list[str], list[str]]:
    """Return missing requirements and forbidden phrases found in output or tool calls."""
    missing = [phrase for phrase in probe.required if phrase not in result_text]
    for tool_name, phrase in probe.required_tool_calls:
        if not any(call.startswith(f"{tool_name} ") and phrase in call for call in tool_calls):
            missing.append(f"{tool_name} call containing {phrase!r}")
    observed = "\n".join((result_text, *tool_calls))
    unexpected = [phrase for phrase in probe.forbidden if phrase in observed]
    return missing, unexpected


def parse_claude_stream(stdout: str) -> tuple[str, list[str]]:
    """Extract the final result and actual tool calls from verbose stream JSON."""
    result_text = ""
    tool_calls: list[str] = []
    for line in stdout.splitlines():
        if not line.strip():
            continue
        payload = json.loads(line)
        if payload.get("type") == "result":
            result = payload.get("result", "")
            result_text = result if isinstance(result, str) else json.dumps(result)
        message = payload.get("message")
        if not isinstance(message, dict):
            continue
        for block in message.get("content", []):
            if isinstance(block, dict) and block.get("type") == "tool_use":
                tool_calls.append(tool_call_text(str(block.get("name", "")), block.get("input", {})))
    return result_text, tool_calls


def write_instruction_hook(workdir: Path, log_path: Path) -> Path:
    """Create a session-only hook that records instruction-load events in the smoke directory."""
    helper = workdir / "record_instruction.py"
    helper.write_text(
        "import json, pathlib, sys\n"
        "event = json.load(sys.stdin)\n"
        "with pathlib.Path(sys.argv[1]).open('a', encoding='utf-8') as stream:\n"
        "    stream.write(json.dumps(event) + '\\n')\n",
        encoding="utf-8",
    )
    command = " ".join(
        shlex.quote(Path(value).as_posix()) for value in (sys.executable, helper, log_path)
    )
    settings = workdir / "smoke-settings.json"
    settings.write_text(
        json.dumps(
            {
                "hooks": {
                    "InstructionsLoaded": [
                        {"matcher": "", "hooks": [{"type": "command", "command": command}]}
                    ]
                }
            }
        ),
        encoding="utf-8",
    )
    return settings


def read_instruction_events(log_path: Path, wait_seconds: float = 2.0) -> list[dict[str, object]]:
    """Read hook events, briefly allowing for the asynchronous InstructionsLoaded hook."""
    deadline = time.monotonic() + wait_seconds
    while not log_path.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    if not log_path.exists():
        return []
    events: list[dict[str, object]] = []
    for line in log_path.read_text(encoding="utf-8").splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict):
            events.append(payload)
    return events


def instruction_was_loaded(events: list[dict[str, object]], expected_path: Path) -> bool:
    expected = os.path.normcase(str(expected_path.resolve(strict=False)))
    return any(
        event.get("hook_event_name") == "InstructionsLoaded"
        and event.get("load_reason") == "include"
        and os.path.normcase(str(Path(str(event.get("file_path", ""))).resolve(strict=False))) == expected
        for event in events
    )


def command_output(command: list[str], cwd: Path | None = None) -> str:
    completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace", cwd=cwd)
    return completed.stdout.strip()


def run_probe(
    claude: str,
    probe: Probe,
    model: str,
    timeout: int,
    workdir: Path,
    settings_path: Path,
    instruction_log: Path,
    policy_path: Path,
) -> ProbeResult:
    command = [
        claude,
        "-p",
        probe.prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--model",
        model,
        "--max-turns",
        "8",
        "--no-session-persistence",
        "--settings",
        str(settings_path),
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
        return ProbeResult(probe.name, False, time.monotonic() - started, "", list(probe.required), [], [], None, "timeout")
    seconds = time.monotonic() - started
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()[-400:]
        return ProbeResult(probe.name, False, seconds, detail, list(probe.required), [], [], None, f"claude exited {completed.returncode}")
    try:
        result_text, tool_calls = parse_claude_stream(completed.stdout)
    except (json.JSONDecodeError, AttributeError) as exc:
        return ProbeResult(probe.name, False, seconds, completed.stdout[-400:], list(probe.required), [], [], None, f"unparseable output: {exc}")
    missing, unexpected = evaluate(probe, result_text, tool_calls)
    policy_loaded = instruction_was_loaded(read_instruction_events(instruction_log), policy_path)
    if probe.require_policy_import and not policy_loaded:
        missing.append(f"InstructionsLoaded include event for {policy_path}")
    return ProbeResult(
        probe.name,
        not missing and not unexpected,
        seconds,
        result_text[:400],
        missing,
        unexpected,
        tool_calls,
        policy_loaded if probe.require_policy_import else None,
    )


def policy_manifest(policy_dir: Path) -> dict[str, object] | None:
    """Read the installed bundle manifest without trusting it as executable input."""
    manifest = policy_dir / "install.json"
    if not manifest.is_file():
        return None
    try:
        payload = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    return payload if isinstance(payload, dict) else None


def policy_commit(policy_dir: Path) -> str | None:
    payload = policy_manifest(policy_dir)
    commit = payload.get("source_commit") if payload is not None else None
    return str(commit)[:7] if isinstance(commit, str) and commit else None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Smoke-test the installed PIRA Claude Code bridge with real claude -p sessions.")
    parser.add_argument("--policy-dir", default="~/.claude/pira", help="Installed PIRA policy directory (default: ~/.claude/pira).")
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
    policy_dir = Path(os.path.expanduser(args.policy_dir))
    selected = [probe for probe in PROBES if not args.only or probe.name in args.only]

    installed_manifest = policy_manifest(policy_dir)
    report: dict[str, object] = {
        "timestamp": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "platform": platform.platform(),
        "claude_version": command_output([claude, "--version"]),
        "model": args.model,
        "policy_dir": str(policy_dir),
        "policy_commit": policy_commit(policy_dir),
        "policy_source_dirty": installed_manifest.get("source_dirty") if installed_manifest else None,
        "probes": [],
    }
    dirty_label = " dirty-source" if report["policy_source_dirty"] else ""
    print(f"claude: {report['claude_version']}  model: {args.model}  policy: {report['policy_commit'] or 'unknown'}{dirty_label}")

    results: list[ProbeResult] = []
    with tempfile.TemporaryDirectory(prefix="pira_smoke_") as temporary:
        workdir = Path(temporary)
        instruction_log = workdir / "instructions.jsonl"
        settings_path = write_instruction_hook(workdir, instruction_log)
        for probe in selected:
            result = run_probe(
                claude,
                probe,
                args.model,
                args.timeout,
                workdir,
                settings_path,
                instruction_log,
                policy_dir / "AGENTS.md",
            )
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
