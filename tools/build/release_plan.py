#!/usr/bin/env python3
"""Order fresh release builds; this plan never authorizes cross-release reuse."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import tomllib
from pathlib import Path

from tools.build.package_github_release import TOOLS


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


def make_plan(changed: set[str], reason: str) -> dict[str, object]:
    if not changed <= set(TOOLS):
        raise ValueError("unknown release tool")
    first = [tool for tool in TOOLS if tool in changed]
    remaining = [tool for tool in TOOLS if tool not in changed]

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
        "reuse": "same-workflow artifacts only; all six tools freshly built",
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--base", default="", help="Full local ancestor commit SHA; absent/uncertain means all tools")
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    plan = plan_repository(args.root, args.base)
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            for key in ("has_changed", "has_remaining", "changed_matrix", "remaining_matrix"):
                output.write(f"{key}={json.dumps(plan[key], separators=(',', ':'))}\n")
    print(json.dumps(plan, indent=2))


if __name__ == "__main__":
    main()
