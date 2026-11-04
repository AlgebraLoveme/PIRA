#!/usr/bin/env python3
"""Order fresh builds or explicitly pin authenticated retained tool outputs."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import tomllib
from pathlib import Path

from tools.build.package_github_release import TOOLS, PackageError, resolve_retained, retained_requests, strict_json


def version_only_lock_changes(before: str, after: str) -> set[str] | None:
    """Recognize only workspace-tool version bumps, not dependency changes."""
    try:
        documents = [tomllib.loads(text) for text in (before, after)]
        versions = []
        for document in documents:
            own = {}
            for package in document["package"]:
                if package["name"] in TOOLS and "source" not in package:
                    name = package["name"]
                    if name in own:
                        return None
                    own[name] = package.pop("version")
            if set(own) != set(TOOLS):
                return None
            versions.append(own)
        if documents[0] != documents[1]:
            return None
        return {tool for tool in TOOLS if versions[0][tool] != versions[1][tool]}
    except (KeyError, TypeError, ValueError):
        return None


def changed_tools(paths: list[str], lock_versions: set[str] | None = None) -> set[str]:
    changed: set[str] = set()
    for path in paths:
        if path == "tools/Cargo.lock" and lock_versions is not None:
            changed.update(lock_versions)
            continue
        owner = next((tool for tool in TOOLS if path.startswith(
            (f"tools/src/{tool}/", f"tools/crates/{tool}/")
        )), None)
        if owner is None:
            # Workspace, dependency, builder, SDK/settings, setup, policy and
            # unknown paths are shared/uncertain: validate everything first.
            return set(TOOLS)
        changed.add(owner)
    return changed


def make_plan(changed: set[str], reason: str, retained: dict | None = None) -> dict[str, object]:
    retained = retained or {}
    if not changed <= set(TOOLS) or not set(retained) < set(TOOLS) or changed & set(retained):
        raise ValueError("unknown release tool")
    first = [tool for tool in TOOLS if tool in changed]
    remaining = [tool for tool in TOOLS if tool not in changed and tool not in retained]

    def matrix(tools: list[str]) -> dict[str, object]:
        # GitHub rejects empty matrices. A disabled phase gets one inert row;
        # its job-level condition must remain false (tested with the workflow).
        return {"include": [{"tool": tool} for tool in (tools or TOOLS[:1])]}

    return {
        "changed": first,
        "remaining": remaining,
        "has_changed": bool(first),
        "has_remaining": bool(remaining),
        "changed_matrix": matrix(first),
        "remaining_matrix": matrix(remaining),
        "reason": reason,
        "retained_tools": retained,
        "build_tools": [tool for tool in TOOLS if tool not in retained],
        "reuse": "explicit authenticated output retention; no current-input equivalence claimed" if retained else "same-workflow artifacts only; all six tools freshly built",
    }


def plan_repository(root: Path, base: str, head: str = "HEAD") -> dict[str, object]:
    def git(*args: str) -> str:
        return subprocess.check_output(
            ["git", "-C", str(root), *args], stderr=subprocess.PIPE,
            timeout=30,
        ).decode("utf-8")

    if not re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", base):
        return make_plan(set(TOOLS), "missing/invalid baseline; validate all tools first")
    try:
        # No fetch or network. A missing/nonancestor baseline is not trusted.
        git("merge-base", "--is-ancestor", base, head)
        paths = git("diff", "--name-only", "--no-renames", "-z", base, head, "--").split("\0")
        paths = [path for path in paths if path]
        lock_versions = None
        if "tools/Cargo.lock" in paths:
            lock_versions = version_only_lock_changes(
                git("show", f"{base}:tools/Cargo.lock"),
                git("show", f"{head}:tools/Cargo.lock"),
            )
        return make_plan(changed_tools(paths, lock_versions), "local ancestor diff; uncertain inputs select all tools")
    except (subprocess.SubprocessError, OSError, UnicodeError):
        return make_plan(set(TOOLS), "baseline/diff unavailable; validate all tools first")


def plan_retained(root: Path, repository: str, requests: dict) -> dict[str, object]:
    requests = retained_requests(json.dumps(requests))
    for tool, pin in requests.items():
        manifest = tomllib.loads((root / f"tools/crates/{tool}/Cargo.toml").read_text())
        if manifest["package"]["version"] != pin["version"]:
            raise PackageError(f"retained pin disagrees with assembly manifest version: {tool}")
    receipts = resolve_retained(repository, requests)
    return make_plan(set(TOOLS) - set(receipts), "explicit pinned output selection, not inferred unchanged inputs", receipts)


def first_tools(text: str) -> list[str]:
    """Validate an explicit first phase; every remaining fresh tool still builds."""
    tools = strict_json(text)
    if (not isinstance(tools, list) or any(not isinstance(tool, str) or tool not in TOOLS for tool in tools)
            or len(tools) != len(set(tools))):
        raise PackageError("first tools must be a JSON list of distinct known tool names")
    return tools


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--base", default="", help="Full local ancestor commit SHA; absent/uncertain means all tools")
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--retained-tools", default="{}", help="JSON map of explicit tool tag/version/source_sha pins")
    parser.add_argument("--first-tools", default="[]", help="JSON list of fresh tools to finish before other fresh builds; empty preserves automatic ordering")
    parser.add_argument("--repository", default="", help="GitHub owner/repo for authenticated retained origins")
    args = parser.parse_args()
    try:
        first = first_tools(args.first_tools)
        requests = retained_requests(args.retained_tools)
        plan = plan_retained(args.root, args.repository, requests) if requests else plan_repository(args.root, args.base)
        if first:
            if set(first) & set(plan["retained_tools"]):
                raise PackageError("first tools cannot include retained tools")
            plan = make_plan(set(first), "explicit fresh first phase; all other fresh tools follow", plan["retained_tools"])
    except (PackageError, OSError, ValueError) as error:
        parser.error(str(error))
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            for key in ("has_changed", "has_remaining", "changed_matrix", "remaining_matrix", "retained_tools", "build_tools"):
                output.write(f"{key}={json.dumps(plan[key], separators=(',', ':'))}\n")
    print(json.dumps(plan, indent=2))


if __name__ == "__main__":
    main()
