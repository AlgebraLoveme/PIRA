#!/usr/bin/env python3
"""Create ~/pira only when absent; normal PIRA must already be installed.

Python 3.11+, standard library only. Never edits global settings or existing
workspace files. Run with the workspace idle: preflight is not a filesystem lock.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import stat
import sys


TEMPLATE_DIR = Path(__file__).resolve().parents[1]
TEMPLATES = {
    "AGENTS.md": "COMMUNICATION_AGENTS_TEMPLATE.md",
    "AGENT_WORKBOOK.md": "COMMUNICATION_WORKBOOK_TEMPLATE.md",
}


def path_kind(path: Path) -> str:
    """Inspect without following symlinks, including Windows reparse points."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        return "missing"
    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
        return "symlink/reparse point"
    if stat.S_ISDIR(info.st_mode):
        return "directory"
    if stat.S_ISREG(info.st_mode):
        return "file"
    return "special file"


def inspect_workspace(workspace: Path) -> tuple[dict[str, str], list[str]]:
    """Preflight all managed paths and the instruction override before writes."""
    for directory in [*reversed(workspace.parents), workspace]:
        kind = path_kind(directory)
        if kind == "missing":
            return {name: "missing" for name in TEMPLATES}, []
        if kind != "directory":
            return {}, [f"Conflicting {kind}: {directory}; choose a real directory path (no symlink ancestors)."]

    errors = []
    override = workspace / "AGENTS.override.md"
    if path_kind(override) != "missing":
        errors.append(f"Instruction override exists: {override}; resolve it manually before setup.")
    kinds = {name: path_kind(workspace / name) for name in TEMPLATES}
    for name, kind in kinds.items():
        if kind not in {"missing", "file"}:
            errors.append(f"Conflicting {kind}: {workspace / name}; preserve and resolve it manually.")
    return kinds, errors


def load_templates() -> tuple[dict[str, bytes], list[str]]:
    """Read both UTF-8 templates before any workspace writes."""
    contents = {}
    errors = []
    for name, template in TEMPLATES.items():
        path = TEMPLATE_DIR / template
        try:
            if path_kind(path) != "file":
                raise ValueError("expected a regular template file, not a missing path or alias")
            data = path.read_bytes()
            data.decode("utf-8")
            contents[name] = data
        except (OSError, ValueError) as exc:
            errors.append(f"Cannot read template {path}: {exc}")
    return contents, errors


def create_missing(path: Path, content: bytes) -> str:
    """Exclusively create a file, preserving a regular file that wins a race."""
    try:
        with path.open("xb") as target:
            target.write(content)
    except FileExistsError:
        if path_kind(path) != "file":
            raise RuntimeError(f"Conflicting path appeared: {path}; resolve it manually.")
        return "PRESERVED"
    return "CREATED"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Create the fixed ~/pira communicator home only when absent; skip existing directories unchanged.",
        epilog="Requires normal PIRA already installed. Fresh creation uses both shipped communication templates. No global changes.",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--dry-run", action="store_true", help="Preflight and report planned creates without writing.")
    mode.add_argument("--verify", action="store_true", help="Check required regular files and conflicts without writing; content may be customized.")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    created = preserved = errors = 0
    try:
        workspace = Path(os.path.abspath(Path.home() / "pira"))
        print(f"Workspace: {workspace}")
        kinds, conflicts = inspect_workspace(workspace)
        if conflicts:
            for problem in conflicts:
                print(f"ERROR: {problem}", file=sys.stderr)
            errors = len(conflicts)
        elif args.verify:
            for name, kind in kinds.items():
                path = workspace / name
                if kind == "file":
                    print(f"PRESERVED: {path}")
                    preserved += 1
                else:
                    print(f"ERROR: Missing required file: {path}", file=sys.stderr)
                    errors += 1
            if not errors:
                print("Verification passed (file presence and type; customized content preserved).")
        elif path_kind(workspace) == "directory":
            print(f"SKIPPED: existing directory preserved unchanged: {workspace}")
            preserved = sum(kind == "file" for kind in kinds.values())
        else:
            templates, template_errors = load_templates()
            if template_errors:
                for problem in template_errors:
                    print(f"ERROR: {problem}", file=sys.stderr)
                errors = len(template_errors)
            elif args.dry_run:
                print(f"DRY-RUN: would create directory {workspace}")
                for name in templates:
                    print(f"DRY-RUN: would create {workspace / name}")
            else:
                workspace.parent.mkdir(parents=True, exist_ok=True)
                # Recheck newly created home components before creating the workspace.
                _, conflicts = inspect_workspace(workspace)
                if conflicts:
                    raise RuntimeError("; ".join(conflicts))
                fresh = False
                try:
                    workspace.mkdir()
                    fresh = True
                except FileExistsError:
                    pass  # A concurrent creator owns the directory; never fill it in.
                kinds, conflicts = inspect_workspace(workspace)
                if conflicts:
                    raise RuntimeError("; ".join(conflicts))
                if path_kind(workspace) != "directory":
                    raise RuntimeError(f"Workspace disappeared during setup: {workspace}; retry when idle.")
                if not fresh:
                    print(f"SKIPPED: existing directory preserved unchanged: {workspace}")
                    preserved = sum(kind == "file" for kind in kinds.values())
                else:
                    for name, content in templates.items():
                        _, conflicts = inspect_workspace(workspace)
                        if conflicts:
                            raise RuntimeError("; ".join(conflicts))
                        result = create_missing(workspace / name, content)
                        print(f"{result}: {workspace / name}")
                        created += result == "CREATED"
                        preserved += result == "PRESERVED"
                    kinds, conflicts = inspect_workspace(workspace)
                    if conflicts:
                        raise RuntimeError("; ".join(conflicts))
                    if any(kind != "file" for kind in kinds.values()):
                        raise RuntimeError(f"Workspace changed during creation: {workspace}; inspect it manually.")
                    print(f"Open a NEW Codex or supported agent session in {workspace}.")
                    print("Keep that ONE conversation across all projects for shared context and coordination.")
                    print("Provide your current projects in that conversation.")
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        errors += 1
    print(f"Summary: created={created} preserved={preserved} errors={errors}")
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
