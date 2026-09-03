#!/usr/bin/env python3
"""Compact, privacy-preserving report over run_parity (claude-modes) and run_multiturn summaries.

Writes one JSON file without absolute paths or session identifiers and prints a Markdown digest.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import statistics
from pathlib import Path
from typing import Any


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


TOOL_USE_ID = re.compile(r"toolu_[A-Za-z0-9]+")


def redact(text: str) -> str:
    return TOOL_USE_ID.sub("toolu_<redacted>", text)


def compact_parity(summary: dict[str, Any]) -> dict[str, Any]:
    cases: dict[str, dict[str, Any]] = {}
    for result in summary["results"]:
        usage = result.get("usage") or {}
        cases.setdefault(result["client"], {})[f"{result['id']}#{result['repetition']}"] = {
            "passed": result["passed"],
            "failures": [redact(str(failure)) for failure in result["failures"]],
            "route_calls": result.get("route_calls"),
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
        "metrics": summary.get("metrics"),
        "cases": cases,
    }


def adaptive_subset_deltas(summary: dict[str, Any]) -> dict[str, Any] | None:
    """Paired deltas restricted to cases where adaptive actually selected (its target population)."""
    by_key: dict[tuple[str, int], dict[str, dict[str, Any]]] = {}
    for result in summary["results"]:
        by_key.setdefault((result["id"], result["repetition"]), {})[result["client"]] = result

    def context(result: dict[str, Any]) -> int:
        usage = result.get("usage") or {}
        return sum(int(usage.get(key) or 0) for key in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))

    rows = []
    for key, clients in by_key.items():
        adaptive = clients.get("claude-adaptive")
        strict = clients.get("claude")
        baseline = clients.get("claude-policy-only")
        if not adaptive or not adaptive.get("adaptive_selected") or not strict or not baseline:
            continue
        rows.append(
            {
                "strict_minus_baseline_context": context(strict) - context(baseline),
                "adaptive_minus_baseline_context": context(adaptive) - context(baseline),
                "strict_minus_baseline_turns": (strict.get("num_turns") or 0) - (baseline.get("num_turns") or 0),
                "adaptive_minus_baseline_turns": (adaptive.get("num_turns") or 0) - (baseline.get("num_turns") or 0),
                "adaptive_minus_strict_seconds": (adaptive.get("duration_seconds") or 0) - (strict.get("duration_seconds") or 0),
                "strict_turns": strict.get("num_turns"),
                "adaptive_turns": adaptive.get("num_turns"),
            }
        )
    if not rows:
        return None
    medians = {key: statistics.median(row[key] for row in rows) for key in rows[0]}
    strict_delta = medians["strict_minus_baseline_context"]
    adaptive_delta = medians["adaptive_minus_baseline_context"]
    reduction = None if not strict_delta else round(1 - adaptive_delta / strict_delta, 3)
    return {"paired_cases": len(rows), "median": medians, "context_overhead_reduction_vs_strict": reduction}


def compact_multiturn(summary: dict[str, Any]) -> dict[str, Any]:
    sessions = []
    for result in summary["results"]:
        sessions.append(
            {
                "mode": result["mode"],
                "id": result["id"],
                "passed": result["passed"],
                "totals": result["totals"],
                "turns": [
                    {key: value for key, value in turn.items() if key not in {"task_tools"}}
                    for turn in result["turns"]
                ],
            }
        )
    return {"policy_commit": summary.get("policy_commit"), "model": summary.get("model"), "effort": summary.get("effort"), "passed": summary.get("passed"), "total": summary.get("total"), "sessions": sessions}


def markdown(report: dict[str, Any]) -> str:
    lines = []
    for name in ("matrix", "heldout"):
        block = report.get(name)
        if not block:
            continue
        lines.append(f"### {name}: pass counts")
        lines.append("| Mode | Passed | Adaptive selected | Route Skill calls | Cases with extra modules | Cases with missing modules |")
        lines.append("|---|---:|---:|---:|---:|---:|")
        for client, metrics in (block.get("metrics") or {}).items():
            lines.append(
                f"| {client} | {metrics['passed']}/{metrics['cases']} | {metrics['adaptive_selected_cases']} | "
                f"{metrics['route_skill_calls']} | {metrics['cases_with_extra_modules']} | {metrics['cases_with_missing_modules']} |"
            )
        lines.append("")
        lines.append(f"### {name}: per-case medians")
        lines.append("| Mode | Model turns | Context tokens | Cache creation | Cache read | Output | Seconds |")
        lines.append("|---|---:|---:|---:|---:|---:|---:|")
        for client, metrics in (block.get("metrics") or {}).items():
            median = metrics.get("median") or {}
            lines.append(
                f"| {client} | {median.get('model_turns')} | {median.get('context_tokens')} | {median.get('cache_creation_tokens')} | "
                f"{median.get('cache_read_tokens')} | {median.get('output_tokens')} | {median.get('duration_seconds')} |"
            )
        subset = block.get("adaptive_selected_subset")
        if subset:
            lines.append("")
            lines.append(
                f"Adaptive-selected subset ({subset['paired_cases']} paired cases): strict adds a median of "
                f"{subset['median']['strict_minus_baseline_context']:.0f} context tokens and "
                f"{subset['median']['strict_minus_baseline_turns']:.0f} model turns over policy-only; adaptive adds "
                f"{subset['median']['adaptive_minus_baseline_context']:.0f} tokens and "
                f"{subset['median']['adaptive_minus_baseline_turns']:.0f} turns "
                f"(context-overhead reduction {subset['context_overhead_reduction_vs_strict']})."
            )
        lines.append("")
    multiturn = report.get("multiturn")
    if multiturn:
        lines.append("### multi-turn sessions")
        lines.append("| Mode | Session | Passed | Model turns | Context tokens | Route Skill calls | Adaptive turns | Seconds |")
        lines.append("|---|---|---|---:|---:|---:|---:|---:|")
        for session in multiturn["sessions"]:
            totals = session["totals"]
            lines.append(
                f"| {session['mode']} | {session['id']} | {session['passed']} | {totals['model_turns']} | {totals['context_tokens']} | "
                f"{totals['route_skill_calls']} | {totals['adaptive_selected_turns']} | {totals['duration_seconds']} |"
            )
        lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix-summary", type=Path)
    parser.add_argument("--heldout-summary", type=Path)
    parser.add_argument("--multiturn-summary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report: dict[str, Any] = {"schema_version": 1, "kind": "pira_compact_claude_modes_evidence", "source_summary_sha256": {}}
    for name, path in (("matrix", args.matrix_summary), ("heldout", args.heldout_summary)):
        if path:
            summary = json.loads(path.read_text(encoding="utf-8"))
            block = compact_parity(summary)
            block["adaptive_selected_subset"] = adaptive_subset_deltas(summary)
            report[name] = block
            report["source_summary_sha256"][name] = sha256_file(path)
    if args.multiturn_summary:
        summary = json.loads(args.multiturn_summary.read_text(encoding="utf-8"))
        report["multiturn"] = compact_multiturn(summary)
        report["source_summary_sha256"]["multiturn"] = sha256_file(args.multiturn_summary)
    text = json.dumps(report, ensure_ascii=False, indent=1) + "\n"
    if ":\\\\" in text or ":/" in text.replace("https://", ""):
        raise SystemExit("refusing to write a report that appears to contain an absolute path")
    args.output.write_text(text, encoding="utf-8")
    print(markdown(report))
    print(f"REPORT {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
