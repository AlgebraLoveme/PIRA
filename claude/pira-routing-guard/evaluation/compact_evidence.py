#!/usr/bin/env python3
"""Compact, redacted evidence from run_parity and run_multiturn summaries.

One row per case (or per turn) as a fixed-order array, no failure strings, no paths or
identifiers, plus per-run provenance and aggregate metrics. Raw streams stay in the platform
temporary directory; the artifact hashes here let a raw run be matched to this file.

Usage:
  compact_evidence.py --kind KIND --output FILE [--note TEXT] [--attach NAME=FILE]...
      --single LABEL=summary.json ...  --multiturn LABEL=summary.json ...
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any

CASE_COLUMNS = [
    "client", "repetition", "id", "active_route", "routing_ok", "task_ok", "outcome", "adaptive_selected",
    "route_skill_calls", "hook_denials", "model_turns", "context_tokens", "cache_creation_tokens",
    "cache_read_tokens", "output_tokens", "duration_seconds", "events_sha256",
]
TURN_COLUMNS = [
    "mode", "repetition", "session", "turn", "kind", "active_route", "loaded_this_turn", "routing_ok", "task_ok",
    "outcome", "adaptive_selected", "route_skill_calls", "model_turns", "context_tokens", "cache_creation_tokens",
    "cache_read_tokens", "output_tokens", "duration_seconds", "events_sha256",
]
IDENTIFIER = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|toolu_[A-Za-z0-9]{6,}")
DRIVE_PATH = re.compile(r"[A-Za-z]:[\\/](?:Users|Temp|home)|/Users/|/home/|/tmp/")


def outcome(routing_ok: bool, task_ok: bool, routing_failures: list[str], task_failures: list[str], loaded: list[str], extra: list[str], missing: list[str]) -> str:
    """A short category token instead of the failure strings."""
    if routing_ok and task_ok:
        return "ok"
    tokens: list[str] = []
    if not routing_ok:
        late = any("before task work" in f for f in routing_failures)
        if any("must fall back" in f for f in routing_failures):
            tokens.append("selected-on-abstain-case")
        elif any("did not select" in f for f in routing_failures):
            tokens.append("did-not-select")
        elif not loaded:
            tokens.append("nothing-loaded")
        elif extra and missing:
            tokens.append("wrong-set")
        elif extra:
            tokens.append("extra-module")
        elif missing:
            tokens.append("missing-module")
        elif late:
            tokens.append("loaded-late")
        elif any("route call" in f for f in routing_failures):
            tokens.append("route-call-count")
        elif any("hook errors" in f for f in routing_failures):
            tokens.append("hook-error")
        else:
            tokens.append("routing-other")
        if late and tokens[-1] not in ("loaded-late", "nothing-loaded"):
            tokens[-1] += "+late"
    if not task_ok:
        if any("permission denials" in f for f in task_failures):
            tokens.append("task:denied-tool")
        elif any("did not match" in f for f in task_failures):
            tokens.append("task:answer-mismatch")
        elif any("exit code" in f or "not successful" in f or "timeout" in f for f in task_failures):
            tokens.append("task:client-failure")
        else:
            tokens.append("task:other")
    return ",".join(tokens)


def hook_denials(events_path: Path) -> int | None:
    if not events_path.exists():
        return None
    for line in events_path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("type") == "result":
            return len(event.get("permission_denials") or [])
    return None


def compact_single(label: str, path: Path) -> dict[str, Any]:
    summary = json.loads(path.read_text(encoding="utf-8"))
    rows = []
    for r in summary["results"]:
        usage = r.get("usage") or {}
        rows.append([
            r["client"], r["repetition"], r["id"], r.get("active_route"), bool(r.get("routing_passed")), bool(r.get("task_passed")),
            outcome(bool(r.get("routing_passed")), bool(r.get("task_passed")), r.get("routing_failures", []), r.get("task_failures", []),
                    r.get("loaded_modules", []), r.get("extra_modules", []), r.get("missing_modules", [])),
            bool(r.get("adaptive_selected")), len(r.get("route_calls") or []), hook_denials(path.parent / r["artifact_dir"] / "events.jsonl"),
            r.get("num_turns"), sum(int(usage.get(k) or 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")),
            usage.get("cache_creation_input_tokens"), usage.get("cache_read_input_tokens"), usage.get("output_tokens"), r.get("duration_seconds"),
            (r.get("artifact_hashes") or {}).get("events_jsonl_sha256"),
        ])
    served = None
    first = path.parent / summary["results"][0]["artifact_dir"] / "events.jsonl"
    if first.exists():
        match = re.search(r'"model":"(claude-[a-z0-9.-]+)"', first.read_text(encoding="utf-8", errors="replace"))
        served = match.group(1) if match else None
    return {
        "label": label,
        "policy_commit": summary.get("policy_commit"), "worktree_dirty": summary.get("worktree_dirty"),
        "plugin_version": summary.get("plugin_version"), "client_versions": summary.get("client_versions"),
        "model": summary.get("models", {}).get("claude"), "served_model_id": served,
        "corpus_sha256": (summary.get("source_hashes") or {}).get("matrix_sha256"), "repetitions": summary.get("repetitions"),
        "rescored": summary.get("rescored"),
        "metrics": summary.get("metrics"),
        "case_columns": CASE_COLUMNS, "cases": rows,
    }


def compact_multiturn(label: str, path: Path) -> dict[str, Any]:
    summary = json.loads(path.read_text(encoding="utf-8"))
    rows = []
    for r in summary["results"]:
        for t in r["turns"]:
            metrics = t.get("metrics") or {}
            rows.append([
                r["mode"], r.get("repetition"), r["id"], t["index"], t.get("kind"), t.get("active_route"), t.get("loaded_modules_this_turn"),
                bool(t.get("routing_passed")), bool(t.get("task_passed")),
                outcome(bool(t.get("routing_passed")), bool(t.get("task_passed")), t.get("routing_failures", []), t.get("task_failures", []),
                        t.get("loaded_modules_this_turn") or [], t.get("extra_modules", []), t.get("missing_modules", [])) if t.get("kind") != "compact" else ("ok" if t.get("passed") else "compact-not-observed"),
                bool(t.get("adaptive_selected")), len(t.get("route_calls") or []), metrics.get("model_turns"), metrics.get("context_tokens"),
                metrics.get("cache_creation_tokens"), metrics.get("cache_read_tokens"), metrics.get("output_tokens"), metrics.get("duration_seconds"),
                (r.get("artifact_hashes") or {}).get(f"turn-{t['index']}.jsonl"),
            ])
    return {
        "label": label, "policy_commit": summary.get("policy_commit"), "worktree_dirty": summary.get("worktree_dirty"),
        "plugin_version": summary.get("plugin_version"), "model": summary.get("model"), "effort": summary.get("effort"),
        "scenarios_sha256": summary.get("scenarios_sha256"), "repetitions": summary.get("repetitions"), "rescored": summary.get("rescored"),
        "sessions_passed": summary.get("passed"), "sessions_total": summary.get("total"),
        "sessions_routing_passed": summary.get("routing_contract_passed"), "sessions_task_passed": summary.get("task_passed"),
        "turn_columns": TURN_COLUMNS, "turns": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", required=True)
    parser.add_argument("--note", default="")
    parser.add_argument("--single", action="append", default=[], metavar="LABEL=SUMMARY")
    parser.add_argument("--multiturn", action="append", default=[], metavar="LABEL=SUMMARY")
    parser.add_argument("--attach", action="append", default=[], metavar="NAME=FILE", help="embed a small text file verbatim (e.g. a probe hook script)")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report: dict[str, Any] = {"schema_version": 3, "kind": args.kind, "note": args.note, "runs": [], "attachments": {}}
    for item in args.single:
        label, path = item.split("=", 1)
        report["runs"].append(compact_single(label, Path(path)))
    for item in args.multiturn:
        label, path = item.split("=", 1)
        report["runs"].append(compact_multiturn(label, Path(path)))
    for item in args.attach:
        name, path = item.split("=", 1)
        report["attachments"][name] = Path(path).read_text(encoding="utf-8")
    text = json.dumps(report, ensure_ascii=False, separators=(",", ":")) + "\n"
    if IDENTIFIER.search(text) or DRIVE_PATH.search(text):
        raise SystemExit("refusing to write a report that appears to contain a path or identifier")
    args.output.write_text(text, encoding="utf-8")
    for run in report["runs"]:
        if "cases" in run:
            by_client: dict[str, list[int]] = {}
            for row in run["cases"]:
                by_client.setdefault(row[0], [0, 0])
                by_client[row[0]][0] += row[4]
                by_client[row[0]][1] += 1
            print(f"{run['label']}: " + ", ".join(f"{c} routing {v[0]}/{v[1]}" for c, v in by_client.items()))
        else:
            print(f"{run['label']}: sessions {run['sessions_passed']}/{run['sessions_total']}")
    print(f"REPORT {args.output} ({len(text)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
