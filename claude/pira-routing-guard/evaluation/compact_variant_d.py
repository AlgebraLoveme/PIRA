#!/usr/bin/env python3
"""Compact, redacted evidence for the lite-guard variant D (per-turn reminder + PreToolUse deny, no Skill).

Inputs are run_parity summaries (policy-only client with the temporary plugin attached) and
run_multiturn summaries produced by the variant runners; the plugin's hook script is embedded so the
exact reminder and deny logic are on record without committing the plugin itself.
"""

from __future__ import annotations

import argparse
import collections
import json
import re
from pathlib import Path
from typing import Any

TOOL_USE_ID = re.compile(r"toolu_[A-Za-z0-9]+")
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
PATH_LIKE = re.compile(r"(?:[A-Za-z]:(?:\\\\|\\|/)|/tmp/|/Users/|/home/)[^'\"\s,}\]]*")


def redact(text: str) -> str:
    return PATH_LIKE.sub("<path>", UUID.sub("<id>", TOOL_USE_ID.sub("toolu_<redacted>", text)))


def category(row: dict[str, Any]) -> str:
    if row["routing_passed"]:
        return "exact and ordered"
    late = any("before task work" in f for f in row["routing_failures"])
    if not row["loaded_modules"]:
        return "nothing loaded"
    if row.get("extra_modules"):
        return "extra module" + (", late" if late else "")
    if row.get("missing_modules"):
        return "incomplete set" + (", late" if late else "")
    return "right set, loaded after task work" if late else "other"


def denials_in(events_path: Path) -> int:
    for line in events_path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("type") == "result":
            return len(event.get("permission_denials") or [])
    return 0


def compact_single(label: str, summary_path: Path) -> dict[str, Any]:
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    rows = [r for r in summary["results"] if r["client"] == "claude-policy-only"]
    cases = {}
    total_denials = 0
    for r in rows:
        usage = r.get("usage") or {}
        denials = denials_in(summary_path.parent / r["artifact_dir"] / "events.jsonl")
        total_denials += denials
        cases[f"{r['id']}#{r['repetition']}"] = {
            "routing_passed": r["routing_passed"], "task_passed": r["task_passed"], "category": category(r),
            "loaded_modules": r["loaded_modules"], "missing_modules": r.get("missing_modules"), "extra_modules": r.get("extra_modules"),
            "routing_failures": [redact(str(f)) for f in r["routing_failures"]], "task_failures": [redact(str(f)) for f in r["task_failures"]],
            "hook_denials": denials, "model_turns": r.get("num_turns"),
            "context_tokens": sum(int(usage.get(k) or 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")),
            "duration_seconds": r.get("duration_seconds"), "artifact_hashes": r.get("artifact_hashes"),
        }
    first_case = next((summary_path.parent / "claude-policy-only" / "repeat-1").iterdir())
    served = re.search(r'"model":"(claude-[a-z0-9.-]+)"', (first_case / "events.jsonl").read_text(encoding="utf-8", errors="replace"))
    med = summary["metrics"]["overall"]["claude-policy-only"]["median"]
    return {
        "label": label, "model_alias": summary["models"]["claude"]["model"], "served_model_id": served.group(1) if served else None,
        "effort": summary["models"]["claude"]["effort"], "client_version": summary["client_versions"].get("claude-policy-only"),
        "policy_commit": summary["policy_commit"], "worktree_dirty": summary["worktree_dirty"], "scenarios_sha256": summary["source_hashes"]["matrix_sha256"],
        "repetitions": summary["repetitions"], "cases": len(rows),
        "routing_contract_passed": sum(r["routing_passed"] for r in rows), "task_passed": sum(r["task_passed"] for r in rows),
        "hook_denials_total": total_denials,
        "failure_categories": dict(collections.Counter(c["category"] for c in cases.values())),
        "median_model_turns": med["model_turns"], "median_context_tokens": med["context_tokens"], "median_duration_seconds": med["duration_seconds"],
        "per_case": cases,
    }


def compact_multiturn(label: str, summary_path: Path) -> dict[str, Any]:
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    sessions = []
    for r in summary["results"]:
        sessions.append({
            "id": r["id"], "repetition": r.get("repetition"), "passed": r["passed"], "routing_passed": r.get("routing_passed"), "task_passed": r.get("task_passed"),
            "totals": r["totals"], "artifact_hashes": r.get("artifact_hashes"),
            "turns": [
                {k: ([redact(str(v)) for v in val] if k.endswith("failures") else val) for k, val in t.items() if k not in {"task_tools"}}
                for t in r["turns"]
            ],
        })
    return {"label": label, "model": summary["model"], "effort": summary["effort"], "policy_commit": summary["policy_commit"], "worktree_dirty": summary["worktree_dirty"],
            "scenarios_sha256": summary.get("scenarios_sha256"), "passed": summary["passed"], "total": summary["total"],
            "routing_contract_passed": summary["routing_contract_passed"], "task_passed": summary["task_passed"], "sessions": sessions}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--single", action="append", default=[], metavar="LABEL=SUMMARY", help="run_parity summary with a label; repeatable")
    parser.add_argument("--multiturn", action="append", default=[], metavar="LABEL=SUMMARY", help="run_multiturn summary with a label; repeatable")
    parser.add_argument("--hook-script", type=Path, required=True, help="the temporary plugin's hook script, embedded verbatim")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report: dict[str, Any] = {
        "schema_version": 1,
        "kind": "pira_lite_guard_variant_d",
        "note": (
            "Variant D: a temporary plugin outside the repository injects a routing reminder at SessionStart and every UserPromptSubmit and "
            "denies any tool other than a Read of a PIRA module file until one module has been read in the session; no route Skill. "
            "Scored with the exact routing contract (canonical module set loaded before the first task tool or answer). Denials issued by the "
            "plugin are recorded per case and are not counted as task failures."
        ),
        "hook_script": args.hook_script.read_text(encoding="utf-8"),
        "single_turn": [compact_single(*item.split("=", 1)) if False else compact_single(item.split("=", 1)[0], Path(item.split("=", 1)[1])) for item in args.single],
        "multi_turn": [compact_multiturn(item.split("=", 1)[0], Path(item.split("=", 1)[1])) for item in args.multiturn],
    }
    text = json.dumps(report, ensure_ascii=False, indent=1) + "\n"
    # The embedded hook script legitimately contains "Path:" annotations; check it only for drive letters.
    without_script = json.dumps({key: value for key, value in report.items() if key != "hook_script"}, ensure_ascii=False)
    if (
        PATH_LIKE.search(without_script.replace("<path>", ""))
        or UUID.search(text)
        or re.search(r"[A-Za-z]:[\\/][A-Za-z]", report["hook_script"])
    ):
        raise SystemExit("refusing to write a report that appears to contain an absolute path or identifier")
    args.output.write_text(text, encoding="utf-8")
    for block in report["single_turn"]:
        print(f"{block['label']}: {block['routing_contract_passed']}/{block['cases']} denials={block['hook_denials_total']} {block['failure_categories']}")
    for block in report["multi_turn"]:
        print(f"{block['label']}: {block['passed']}/{block['total']} sessions")
    print(f"REPORT {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
