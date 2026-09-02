#!/usr/bin/env python3
"""Deterministic setup helper for PIRA on Claude Code.

The script intentionally uses only the Python standard library. It configures the
current machine for the existing global PIRA layout centered on ``~/agent`` and
keeps all writes explicit, backed up, and verifiable. Claude Code reads
``CLAUDE.md``, so setup manages exactly one marked block in the user-level
``~/.claude/CLAUDE.md`` that imports the canonical ``~/agent/AGENTS.md``.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Literal

VERIFY_TOKEN = "31415926535897932384626433832795"
CLAUDE_BLOCK_START = "<!-- PIRA:BEGIN (managed by setup_pira.py; do not edit inside) -->"
CLAUDE_BLOCK_END = "<!-- PIRA:END -->"
USER_PLACEHOLDER_TEXT = """# USER

## Knowledge Domains
- fill manually

## Technical Ability
- fill manually

## Strengths
- fill manually

## Learning Targets
- fill manually

## Working Preferences
- fill manually
"""


@dataclass
class SetupState:
    repo_root: Path
    agent_dir: Path
    dry_run: bool
    yes: bool
    changed: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    verification: list[tuple[str, bool, str]] = field(default_factory=list)

    def note_change(self, message: str) -> None:
        self.changed.append(message)
        print(f"CHANGE: {message}")

    def warn(self, message: str) -> None:
        self.warnings.append(message)
        print(f"WARNING: {message}")

    def check(self, name: str, passed: bool, detail: str) -> None:
        self.verification.append((name, passed, detail))
        print(f"{'PASS' if passed else 'FAIL'}: {name} — {detail}")


def expand_path(value: str) -> Path:
    path = Path(os.path.expandvars(os.path.expanduser(value)))
    if path.is_absolute():
        return path
    return Path.cwd() / path


def display_path(path: Path) -> str:
    expanded = path.expanduser()
    if not expanded.is_absolute():
        expanded = Path.cwd() / expanded
    home = Path.home()
    try:
        return "~/" + str(expanded.relative_to(home))
    except ValueError:
        pass
    try:
        return "~/" + str(expanded.resolve(strict=False).relative_to(home.resolve()))
    except ValueError:
        return str(expanded)


def backup_path(path: Path) -> Path:
    stamp = datetime.now().strftime("%Y%m%d%H%M%S%f")
    candidate = path.with_name(f"{path.name}.bak.{stamp}")
    suffix = 1
    while candidate.exists() or candidate.is_symlink():
        candidate = path.with_name(f"{path.name}.bak.{stamp}.{suffix}")
        suffix += 1
    return candidate


def prompt_yes_no(question: str, default: bool = False) -> bool:
    suffix = "[Y/n]" if default else "[y/N]"
    try:
        answer = input(f"{question} {suffix} ").strip().lower()
    except EOFError:  # stdin looked interactive but closed, e.g. some wrapper shells
        print()
        return default
    if not answer:
        return default
    return answer in {"y", "yes"}


def confirm_or_skip(state: SetupState, question: str, default: bool = False) -> bool:
    if state.yes:
        return True
    if not sys.stdin.isatty():
        state.warn(f"Skipped because confirmation is required in non-interactive mode: {question}")
        return False
    return prompt_yes_no(question, default=default)


def read_utf8_text(path: Path, description: str) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        raise RuntimeError(f"{description} is not valid UTF-8: {display_path(path)}") from exc


def write_text(state: SetupState, path: Path, content: str, description: str, *, backup: bool = True) -> None:
    old = read_utf8_text(path, description) if path.exists() else None
    if old == content:
        print(f"OK: {description} already up to date ({display_path(path)})")
        return
    if state.dry_run:
        print(f"DRY-RUN: would write {description}: {display_path(path)}")
        state.note_change(f"would update {display_path(path)}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    if backup and path.exists():
        backup = backup_path(path)
        shutil.copy2(path, backup)
        print(f"Backup: {display_path(path)} -> {display_path(backup)}")
    path.write_text(content, encoding="utf-8")
    state.note_change(f"updated {display_path(path)}")


def path_under(path: Path, root: Path) -> bool:
    try:
        path.absolute().relative_to(root.absolute())
        return True
    except ValueError:
        return False


def backup_legacy_target(state: SetupState, path: Path) -> Path:
    stamp = datetime.now().strftime("%Y%m%d%H%M%S%f")
    try:
        relative = path.relative_to(state.agent_dir)
    except ValueError:
        relative = Path(path.name)
    candidate = state.agent_dir / ".backup" / "setup_pira_legacy" / relative
    candidate = candidate.with_name(f"{candidate.name}.bak.{stamp}")
    suffix = 1
    while candidate.exists() or candidate.is_symlink():
        candidate = candidate.with_name(f"{candidate.name}.{suffix}")
        suffix += 1
    return candidate


def remove_path(state: SetupState, path: Path) -> None:
    target = backup_legacy_target(state, path)
    if state.dry_run:
        print(f"DRY-RUN: would move legacy path {display_path(path)} to backup {display_path(target)}")
        state.note_change(f"would move legacy {display_path(path)} to backup")
        return
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(path), str(target))
    state.note_change(f"moved legacy {display_path(path)} to backup {display_path(target)}")


def same_location(a: Path, b: Path) -> bool:
    try:
        return a.resolve() == b.resolve()
    except FileNotFoundError:
        return False


def pira_source_root(state: SetupState) -> Path:
    """Return where PIRA source files can be read during the current run."""
    if (state.agent_dir / "AGENTS.md").exists():
        return state.agent_dir
    return state.repo_root


def ensure_agent_dir(state: SetupState, force_agent_link: bool) -> None:
    agent_dir = state.agent_dir
    repo_root = state.repo_root
    if same_location(agent_dir, repo_root):
        print(f"OK: repository is available at {display_path(agent_dir)}")
        return
    if not agent_dir.exists() and not agent_dir.is_symlink():
        if state.dry_run:
            print(f"DRY-RUN: would create symlink {display_path(agent_dir)} -> {display_path(repo_root)}")
            state.note_change(f"would create {display_path(agent_dir)} symlink")
            return
        agent_dir.parent.mkdir(parents=True, exist_ok=True)
        try:
            agent_dir.symlink_to(repo_root, target_is_directory=True)
        except OSError as exc:
            raise RuntimeError(
                f"Could not create symlink {agent_dir} -> {repo_root}: {exc}. "
                "Move the repository to ~/agent or rerun with --agent-dir PATH."
            ) from exc
        state.note_change(f"created symlink {display_path(agent_dir)} -> {display_path(repo_root)}")
        return

    if not force_agent_link:
        raise RuntimeError(
            f"{display_path(agent_dir)} already exists and does not point to this repository. "
            "Move it manually, choose --agent-dir PATH, or rerun with --force-agent-link."
        )

    target = backup_path(agent_dir)
    if state.dry_run:
        print(f"DRY-RUN: would move existing {display_path(agent_dir)} to {display_path(target)}")
        print(f"DRY-RUN: would create symlink {display_path(agent_dir)} -> {display_path(repo_root)}")
        state.note_change(f"would replace conflicting {display_path(agent_dir)}")
        return
    agent_dir.rename(target)
    agent_dir.symlink_to(repo_root, target_is_directory=True)
    state.note_change(f"moved existing {display_path(agent_dir)} to {display_path(target)} and linked PIRA")


def ensure_user_md(state: SetupState, user_mode: Literal["keep", "placeholder", "interactive"]) -> None:
    user_path = state.agent_dir / "USER.md"
    source_user_path = pira_source_root(state) / "USER.md"
    if user_path.exists() or source_user_path.exists():
        print(f"OK: USER.md exists ({display_path(source_user_path if source_user_path.exists() else user_path)})")
        return
    if user_mode == "keep":
        state.warn("USER.md is missing; leaving it absent because --user-mode keep was selected")
        return
    if user_mode == "interactive" and not state.yes and sys.stdin.isatty():
        print("USER.md is missing. PIRA works best with stable user preferences, but a placeholder is safe.")
        if not prompt_yes_no("Create a private placeholder USER.md now?", default=True):
            state.warn("USER.md placeholder was not created")
            return
    write_text(state, user_path, USER_PLACEHOLDER_TEXT, "private USER.md placeholder", backup=False)


def parse_legacy_paths(source_root: Path, agent_dir: Path) -> list[Path]:
    legacy_file = source_root / "assets" / "LEGACY_LIST.md"
    if not legacy_file.exists():
        return []
    paths: list[Path] = []
    for line in legacy_file.read_text(encoding="utf-8").splitlines():
        match = re.match(r"\s*-\s*`([^`]+)`", line)
        if not match:
            continue
        raw = match.group(1).replace("~/agent", str(agent_dir))
        paths.append(expand_path(raw))
    return paths


def remove_legacy_files(state: SetupState, legacy_mode: Literal["ask", "remove", "keep"]) -> None:
    existing = [path for path in parse_legacy_paths(pira_source_root(state), state.agent_dir) if path.exists() or path.is_symlink()]
    if not existing:
        print("OK: no legacy files found")
        return
    for path in existing:
        print(f"Legacy path found: {display_path(path)}")
    if legacy_mode == "keep":
        state.warn("Legacy files remain because --legacy keep was selected")
        return
    if legacy_mode == "ask" and not confirm_or_skip(state, "Remove the legacy files listed above?", default=True):
        state.warn("Legacy files remain")
        return
    for path in existing:
        if not path_under(path, state.agent_dir):
            state.warn(f"Skipped legacy path outside agent directory: {display_path(path)}")
            continue
        remove_path(state, path)


def claude_import_path(path: Path) -> str:
    """Return a stable ``@`` import path for CLAUDE.md.

    Claude Code resolves ``~/`` imports and does not document any quoting for
    whitespace; upstream issue anthropics/claude-code#56927 shows such imports
    failing silently, so whitespace is rejected instead of written.
    """
    expanded = path.expanduser().absolute()
    home = Path.home().absolute()
    try:
        import_path = "~/" + expanded.relative_to(home).as_posix()
    except ValueError:
        import_path = expanded.as_posix()
    if any(character.isspace() for character in import_path):
        raise RuntimeError(
            f"CLAUDE.md imports cannot contain whitespace, but the agent directory resolves to {import_path!r}. "
            "Use --agent-dir with a whitespace-free path or a symlink to one."
        )
    return import_path


def claude_managed_block(agent_dir: Path) -> str:
    """The whole managed block: one import of the canonical policy, nothing else."""
    return f"{CLAUDE_BLOCK_START}\n@{claude_import_path(agent_dir / 'AGENTS.md')}\n{CLAUDE_BLOCK_END}"


def locate_managed_block(text: str, claude_md_path: Path) -> tuple[int, int] | None:
    """Return the [start, end) span of the single managed block, or None when absent."""
    start_count = text.count(CLAUDE_BLOCK_START)
    end_count = text.count(CLAUDE_BLOCK_END)
    if start_count != end_count or start_count > 1:
        raise RuntimeError(
            f"Refusing to edit malformed PIRA markers in {display_path(claude_md_path)} "
            f"(start={start_count}, end={end_count})"
        )
    if start_count == 0:
        return None
    start = text.index(CLAUDE_BLOCK_START)
    end_start = text.index(CLAUDE_BLOCK_END)
    if end_start < start:
        raise RuntimeError(f"Refusing to edit reversed PIRA markers in {display_path(claude_md_path)}")
    return start, end_start + len(CLAUDE_BLOCK_END)


def update_claude_md(state: SetupState, claude_md_path: Path) -> None:
    """Install or refresh the managed import while preserving all user-owned content."""
    block = claude_managed_block(state.agent_dir)
    existing = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md") if claude_md_path.exists() else ""
    span = locate_managed_block(existing, claude_md_path)
    if span is not None:
        updated = existing[: span[0]] + block + existing[span[1] :]
    else:
        separator = "" if not existing else ("\n" if existing.endswith("\n") else "\n\n")
        updated = existing + separator + block + "\n"
    write_text(state, claude_md_path, updated, "Claude Code CLAUDE.md")


def remove_claude_md_block(state: SetupState, claude_md_path: Path) -> None:
    """Remove only the managed block; user-owned content and PIRA tools stay untouched."""
    if not claude_md_path.exists():
        print(f"OK: {display_path(claude_md_path)} does not exist; nothing to remove")
        return
    existing = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    span = locate_managed_block(existing, claude_md_path)
    if span is None:
        print(f"OK: no PIRA block in {display_path(claude_md_path)}")
        return
    end = span[1] + 1 if existing[span[1] : span[1] + 1] == "\n" else span[1]
    before = existing[: span[0]]
    if before.strip():
        before = before.rstrip("\n") + "\n"  # drop the separator added when the block was appended
    remaining = before + existing[end:]
    if remaining.strip():
        write_text(state, claude_md_path, remaining, "Claude Code CLAUDE.md")
        return
    if state.dry_run:
        print(f"DRY-RUN: would back up and delete {display_path(claude_md_path)} because only the PIRA block remains")
        state.note_change(f"would delete {display_path(claude_md_path)}")
        return
    backup = backup_path(claude_md_path)
    shutil.copy2(claude_md_path, backup)
    print(f"Backup: {display_path(claude_md_path)} -> {display_path(backup)}")
    claude_md_path.unlink()
    state.note_change(f"deleted {display_path(claude_md_path)} (only the PIRA block remained)")


def verify_claude(state: SetupState, claude_md_path: Path) -> None:
    if not claude_md_path.exists():
        state.check("Claude Code CLAUDE.md exists", False, display_path(claude_md_path))
        return
    text = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    try:
        expected = claude_managed_block(state.agent_dir)
    except RuntimeError as exc:
        state.check("Claude Code PIRA import", False, str(exc))
        return
    state.check(
        "Claude Code PIRA import",
        text.count(CLAUDE_BLOCK_START) == 1 and text.count(CLAUDE_BLOCK_END) == 1 and expected in text,
        f"{display_path(claude_md_path)} -> {claude_import_path(state.agent_dir / 'AGENTS.md')}",
    )


def verify_claude_removed(state: SetupState, claude_md_path: Path) -> None:
    present = claude_md_path.exists() and CLAUDE_BLOCK_START in read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    state.check("Claude Code PIRA import removed", not present, display_path(claude_md_path))


def verify(state: SetupState) -> None:
    agents = state.agent_dir / "AGENTS.md"
    state.check("AGENTS.md exists", agents.exists(), display_path(agents))
    user = state.agent_dir / "USER.md"
    state.check("USER.md exists", user.exists(), display_path(user))
    token_ok = agents.exists() and VERIFY_TOKEN in agents.read_text(encoding="utf-8")
    state.check("verification token", token_ok, VERIFY_TOKEN)
    legacy_existing = [path for path in parse_legacy_paths(pira_source_root(state), state.agent_dir) if path.exists() or path.is_symlink()]
    state.check("legacy files absent", not legacy_existing, ", ".join(display_path(p) for p in legacy_existing) or "none")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Set up PIRA for Claude Code on the current machine.")
    parser.add_argument("--agent-dir", default="~/agent", help="Global PIRA path to configure (default: ~/agent).")
    parser.add_argument("--claude-md", default="~/.claude/CLAUDE.md", help="Claude Code user instruction file.")
    parser.add_argument("--uninstall", action="store_true", help="Remove the PIRA block from CLAUDE.md; leaves tools, ~/agent, and other content untouched.")
    parser.add_argument("--skip-tools", action="store_true", help="Do not install or refresh bundled PIRA tools.")
    parser.add_argument("--tools-install-dir", default=None, help="Override the per-user PIRA tools PATH directory.")
    parser.add_argument(
        "--tools-version",
        action="append",
        default=None,
        help="Pin a native tool as ctx=VERSION, dec=VERSION, nav=VERSION, or svg=VERSION; repeatable.",
    )
    parser.add_argument("--user-mode", choices=["interactive", "placeholder", "keep"], default="interactive")
    parser.add_argument("--legacy", choices=["ask", "remove", "keep"], default="ask", help="How to handle paths listed in assets/LEGACY_LIST.md.")
    parser.add_argument("--force-agent-link", action="store_true", help="Move a conflicting --agent-dir aside and symlink this repo there.")
    parser.add_argument("--verify", action="store_true", help="Only verify the current setup; do not write.")
    parser.add_argument("--dry-run", action="store_true", help="Print planned changes without writing.")
    parser.add_argument("--yes", action="store_true", help="Assume yes for setup confirmations.")
    return parser


def configure_tools(
    state: SetupState,
    install_dir: str | None,
    versions: list[str] | None,
    *,
    verify_only: bool,
) -> None:
    script = state.repo_root / "assets" / "scripts" / "setup_pira_tools.py"
    if not script.is_file():
        raise RuntimeError(f"PIRA tools setup script is missing: {script}")
    command = [sys.executable, str(script)]
    if install_dir:
        command.extend(["--install-dir", str(expand_path(install_dir))])
    for version in versions or []:
        command.extend(["--version", version])
    if verify_only:
        command.append("--verify")
    elif state.dry_run:
        command.append("--dry-run")
    subprocess.run(command, check=True)


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repo_root = Path(__file__).resolve().parents[2]
    state = SetupState(repo_root=repo_root, agent_dir=expand_path(args.agent_dir), dry_run=args.dry_run or args.verify, yes=args.yes)
    claude_md_path = expand_path(args.claude_md)

    print("PIRA setup (Claude Code)")
    print(f"Repository: {display_path(repo_root)}")
    print(f"Agent dir:  {display_path(state.agent_dir)}")
    print(f"CLAUDE.md:  {display_path(claude_md_path)}")
    print(f"Dry run:    {state.dry_run}")

    try:
        if args.uninstall:
            if args.verify:
                raise RuntimeError("--uninstall and --verify cannot be combined")
            remove_claude_md_block(state, claude_md_path)
            if not args.dry_run:
                verify_claude_removed(state, claude_md_path)
        else:
            if not args.verify:
                ensure_agent_dir(state, force_agent_link=args.force_agent_link)
                ensure_user_md(state, args.user_mode)
                remove_legacy_files(state, args.legacy)
                update_claude_md(state, claude_md_path)
                if not args.skip_tools:
                    configure_tools(state, args.tools_install_dir, args.tools_version, verify_only=False)
            if args.dry_run and not args.verify:
                print("DRY-RUN: verification skipped because planned changes were not applied")
            else:
                verify(state)
                verify_claude(state, claude_md_path)
                if args.verify and not args.skip_tools:
                    configure_tools(state, args.tools_install_dir, args.tools_version, verify_only=True)
    except (RuntimeError, subprocess.CalledProcessError, OSError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1

    print("\nSummary")
    if state.changed:
        for item in state.changed:
            print(f"- {item}")
    else:
        print("- No changes")
    if state.warnings:
        print("Warnings:")
        for item in state.warnings:
            print(f"- {item}")
    failed = [name for name, passed, _ in state.verification if not passed]
    if failed:
        print("Verification failed:")
        for item in failed:
            print(f"- {item}")
        return 1
    if state.verification:
        print("Verification passed.")
    else:
        print("Verification skipped.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
