#!/usr/bin/env python3
"""Compact, privacy-preserving report over run_parity (claude-modes) and run_multiturn summaries.

Writes one JSON file without absolute paths, session identifiers or tool-use identifiers and
prints a Markdown digest. Overall metrics and the adaptive-selected subset are both reported.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import Any

TOOL_USE_ID = re.compile(r"toolu_[A-Za-z0-9]+")
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
ABSOLUTE_PATH = re.compile(r"[A-Za-z]:\\\\|[A-Za-z]:/|/Users/|/home/|\\\\Users\\\\")


PATH_LIKE = re.compile(r"(?:[A-Za-z]:(?:\\\\|\\|/)|/tmp/|/Users/|/home/)[^'\"\\s,}\\]]*")


def redact(text: str) -> str:
    return PATH_LIKE.sub("<path>", UUID.sub("<id>", TOOL_USE_ID.sub("toolu_<redacted>", text)))


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compact_parity(summary: dict[str, Any]) -> dict[str, Any]:
    cases: dict[str, dict[str, Any]] = {}
    for result in summary["results"]:
        usage = result.get("usage") or {}
        cases.setdefault(result["client"], {})[f"{result['id']}#{result['repetition']}"] = {
            "passed": result["passed"],
            "routing_passed": result.get("routing_passed"),
            "task_passed": result.get("task_passed"),
            "routing_failures": [redact(str(item)) for item in result.get("routing_failures", [])],
            "task_failures": [redact(str(item)) for item in result.get("task_failures", [])],
            "route_calls": result.get("route_calls"),
            "active_route": result.get("active_route"),
            "adaptive_selected": result.get("adaptive_selected", False),
            "loaded_modules": result.get("loaded_modules"),
            "extra_modules": result.get("extra_modules", []),
            "missing_modules": result.get("missing_modules", []),
            "model_turns": result.get("num_turns"),
            "context_tokens": sum(int(usage.get(key) or 0) for key in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")),
            "cache_creation_tokens": usage.get("cache_creation_input_tokens"),
            "cache_read_tokens": usage.get("cache_read_input_tokens"),
            "output_tokens": usage.get("output_tokens"),
            "duration_seconds": result.get("duration_seconds"),
            "artifact_hashes": result.get("artifact_hashes"),
        }
    return {
        "policy_commit": summary.get("policy_commit"),
        "plugin_version": summary.get("plugin_version"),
        "client_versions": summary.get("client_versions"),
        "models": summary.get("models"),
        "repetitions": summary.get("repetitions"),
        "source_hashes": summary.get("source_hashes"),
        "worktree_dirty": summary.get("worktree_dirty"),
        "by_client": summary.get("by_client"),
        "routing_contract_passed": summary.get("routing_contract_passed"),
        "task_passed": summary.get("task_passed"),
        "metrics": summary.get("metrics"),
        "cases": cases,
    }


def compact_multiturn(summary: dict[str, Any]) -> dict[str, Any]:
    sessions = []
    for result in summary["results"]:
        sessions.append(
            {
                "mode": result["mode"],
                "repetition": result.get("repetition"),
                "id": result["id"],
                "passed": result["passed"],
                "routing_passed": result.get("routing_passed"),
                "task_passed": result.get("task_passed"),
                "totals": result["totals"],
                "artifact_hashes": result.get("artifact_hashes"),
                "turns": [
                    {
                        key: ([redact(str(item)) for item in value] if key.endswith("failures") else value)
                        for key, value in turn.items()
                        if key not in {"task_tools"}
                    }
                    for turn in result["turns"]
                ],
            }
        )
    return {
        "policy_commit": summary.get("policy_commit"),
        "plugin_version": summary.get("plugin_version"),
        "worktree_dirty": summary.get("worktree_dirty"),
        "scenarios_sha256": summary.get("scenarios_sha256"),
        "model": summary.get("model"),
        "effort": summary.get("effort"),
        "repetitions": summary.get("repetitions"),
        "passed": summary.get("passed"),
        "routing_contract_passed": summary.get("routing_contract_passed"),
        "task_passed": summary.get("task_passed"),
        "total": summary.get("total"),
        "sessions": sessions,
    }


def fmt(value: Any) -> str:
    if isinstance(value, float):
        return f"{value:.1f}" if abs(value) < 1000 else f"{value / 1000:.1f}k"
    return str(value)


def markdown(report: dict[str, Any]) -> str:
    lines: list[str] = []
    for name in ("matrix", "development", "prospective"):
        block = report.get(name)
        if not block:
            continue
        metrics = block.get("metrics") or {}
        overall = metrics.get("overall") or {}
        lines.append(f"### {name} (commit {str(block.get('policy_commit'))[:7]}, dirty={block.get('worktree_dirty')})")
        lines.append("| Mode | Passed | Routing contract | Task | Adaptive selected | Route Skill calls | Extra | Missing | Turns (med) | Context (med) | Seconds (med) |")
        lines.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
        for client, row in overall.items():
            median = row.get("median") or {}
            lines.append(
                f"| {client} | {row['passed']}/{row['cases']} | {row['routing_contract_passed']}/{row['cases']} | {row['task_passed']}/{row['cases']} | "
                f"{row['adaptive_selected_cases']} | {row['route_skill_calls']} | {row['cases_with_extra_modules']} | {row['cases_with_missing_modules']} | "
                f"{fmt(median.get('model_turns'))} | {fmt(median.get('context_tokens'))} | {fmt(median.get('duration_seconds'))} |"
            )
        subset = metrics.get("adaptive_selected_subset")
        if subset:
            adaptive = subset.get("adaptive") or {}
            strict = subset.get("strict_on_same_cases") or {}
            lines.append("")
            lines.append(
                f"Adaptive-selected subset: {subset['selected_cases']} cases, coverage {subset.get('coverage')} of {subset['module_requiring_cases']} module-requiring cases; "
                f"extra-module cases {adaptive.get('cases_with_extra_modules')}, missing-module cases {adaptive.get('cases_with_missing_modules')}; "
                f"median turns adaptive {fmt((adaptive.get('median') or {}).get('model_turns'))} vs strict {fmt((strict.get('median') or {}).get('model_turns'))}; "
                f"paired context overhead vs policy-only adaptive {fmt((adaptive.get('paired_delta_vs_policy_only_median') or {}).get('context_tokens'))} vs strict "
                f"{fmt((strict.get('paired_delta_vs_policy_only_median') or {}).get('context_tokens'))} "
                f"(reduction {subset.get('context_overhead_reduction_vs_strict')})."
            )
        lines.append("")
    for name in ("multiturn", "prospective_multiturn"):
        block = report.get(name)
        if not block:
            continue
        lines.append(f"### {name} (commit {str(block.get('policy_commit'))[:7]}, dirty={block.get('worktree_dirty')}): {block['passed']}/{block['total']} sessions, routing {block['routing_contract_passed']}/{block['total']}, task {block['task_passed']}/{block['total']}")
        lines.append("| Mode | Rep | Session | Passed | Routing | Task | Turns | Context | Route Skill calls | Adaptive turns | Seconds |")
        lines.append("|---|---:|---|---|---|---|---:|---:|---:|---:|---:|")
        for session in block["sessions"]:
            totals = session["totals"]
            lines.append(
                f"| {session['mode']} | {session.get('repetition')} | {session['id']} | {session['passed']} | {session.get('routing_passed')} | {session.get('task_passed')} | "
                f"{totals['model_turns']} | {totals['context_tokens']} | {totals['route_skill_calls']} | {totals['adaptive_selected_turns']} | {totals['duration_seconds']} |"
            )
        lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix-summary", type=Path)
    parser.add_argument("--development-summary", type=Path)
    parser.add_argument("--prospective-summary", type=Path)
    parser.add_argument("--multiturn-summary", type=Path)
    parser.add_argument("--prospective-multiturn-summary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report: dict[str, Any] = {"schema_version": 2, "kind": "pira_compact_claude_modes_evidence", "source_summary_sha256": {}}
    for name, path in (
        ("matrix", args.matrix_summary),
        ("development", args.development_summary),
        ("prospective", args.prospective_summary),
    ):
        if path:
            report[name] = compact_parity(json.loads(path.read_text(encoding="utf-8")))
            report["source_summary_sha256"][name] = sha256_file(path)
    for name, path in (("multiturn", args.multiturn_summary), ("prospective_multiturn", args.prospective_multiturn_summary)):
        if path:
            report[name] = compact_multiturn(json.loads(path.read_text(encoding="utf-8")))
            report["source_summary_sha256"][name] = sha256_file(path)
    commits = {str(block.get("policy_commit")) for key, block in report.items() if isinstance(block, dict) and block.get("policy_commit")}
    report["runtime_commits"] = sorted(commits)
    text = json.dumps(report, ensure_ascii=False, indent=1) + "\n"
    if ABSOLUTE_PATH.search(text) or UUID.search(text):
        raise SystemExit("refusing to write a report that appears to contain an absolute path or identifier")
    args.output.write_text(text, encoding="utf-8")
    print(markdown(report))
    print(f"REPORT {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
