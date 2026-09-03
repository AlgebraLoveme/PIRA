#!/usr/bin/env python3
"""Install a stable PIRA policy bundle for Claude Code.

The source checkout may live anywhere. Claude Code imports a validated snapshot
from ``~/.claude/pira`` so changing a Git branch cannot silently change the
installed policy. Native PIRA tools remain shared through the user's PATH.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Literal

VERIFY_TOKEN = "31415926535897932384626433832795"
CLAUDE_BLOCK_START = "<!-- PIRA:BEGIN (managed by setup_pira.py; do not edit inside) -->"
CLAUDE_BLOCK_END = "<!-- PIRA:END -->"
DEFAULT_POLICY_DIR = "~/.claude/pira"
MANIFEST_NAME = "install.json"
MANIFEST_SCHEMA = 1
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
    policy_dir: Path
    dry_run: bool
    yes: bool
    source_commit: str
    source_branch: str | None
    source_dirty: bool
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
    return path if path.is_absolute() else Path.cwd() / path


def display_path(path: Path) -> str:
    expanded = path.expanduser()
    if not expanded.is_absolute():
        expanded = Path.cwd() / expanded
    home = Path.home()
    try:
        return "~/" + expanded.relative_to(home).as_posix()
    except ValueError:
        pass
    try:
        return "~/" + expanded.resolve(strict=False).relative_to(home.resolve()).as_posix()
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
    except EOFError:
        print()
        return default
    if not answer:
        return default
    return answer in {"y", "yes"}


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
        backup_file = backup_path(path)
        shutil.copy2(path, backup_file)
        print(f"Backup: {display_path(path)} -> {display_path(backup_file)}")
    path.write_text(content, encoding="utf-8")
    state.note_change(f"updated {display_path(path)}")


def path_under(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
        return True
    except ValueError:
        return False


def command_output(command: list[str], description: str) -> str:
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"Could not {description}: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise RuntimeError(f"Could not {description}: {detail or f'exit {completed.returncode}'}")
    return completed.stdout.strip()


def source_metadata(repo_root: Path, expected_branch: str | None) -> tuple[str, str | None, bool]:
    commit = command_output(["git", "-C", str(repo_root), "rev-parse", "HEAD"], "read the source Git commit")
    branch = command_output(
        ["git", "-C", str(repo_root), "branch", "--show-current"],
        "read the source Git branch",
    ) or None
    dirty = bool(command_output(["git", "-C", str(repo_root), "status", "--porcelain"], "inspect the source worktree"))
    if expected_branch and branch != expected_branch:
        raise RuntimeError(
            f"Expected source branch {expected_branch!r}, but {display_path(repo_root)} is on {branch or 'detached HEAD'!r}. "
            "Use a clean checkout of the Claude branch; setup will not switch branches for you."
        )
    if expected_branch and dirty:
        raise RuntimeError(
            f"Source checkout {display_path(repo_root)} has uncommitted files. "
            "Commit or remove them before an audited branch installation."
        )
    return commit, branch, dirty


def claude_import_path(path: Path) -> str:
    """Return an unquoted Claude import path, rejecting unsupported whitespace."""
    expanded = path.expanduser().absolute()
    home = Path.home().absolute()
    try:
        import_path = "~/" + expanded.relative_to(home).as_posix()
    except ValueError:
        import_path = expanded.as_posix()
    if any(character.isspace() for character in import_path):
        raise RuntimeError(
            f"CLAUDE.md imports cannot contain whitespace, but the policy directory resolves to {import_path!r}. "
            "Use the default ~/.claude/pira location or another whitespace-free path."
        )
    return import_path


def claude_managed_block(policy_dir: Path) -> str:
    return f"{CLAUDE_BLOCK_START}\n@{claude_import_path(policy_dir / 'AGENTS.md')}\n{CLAUDE_BLOCK_END}"


def locate_managed_block(text: str, claude_md_path: Path) -> tuple[int, int] | None:
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
    end = end_start + len(CLAUDE_BLOCK_END)
    # The previous bridge format owned one final newline after the end marker.
    # Absorb it only when it is the complete suffix, never when user text follows.
    if text[end:] in {"\n", "\r\n"}:
        end = len(text)
    return start, end


def planned_claude_md(claude_md_path: Path, policy_dir: Path) -> str:
    """Build the next CLAUDE.md without claiming any surrounding user bytes."""
    block = claude_managed_block(policy_dir)
    existing = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md") if claude_md_path.exists() else ""
    span = locate_managed_block(existing, claude_md_path)
    if span is not None:
        return existing[: span[0]] + block + existing[span[1] :]
    # The managed marker itself is the separator. Adding no inferred whitespace
    # makes install followed by uninstall byte-for-byte reversible.
    return existing + block


def update_claude_md(state: SetupState, claude_md_path: Path, planned: str | None = None) -> None:
    updated = planned if planned is not None else planned_claude_md(claude_md_path, state.policy_dir)
    write_text(state, claude_md_path, updated, "Claude Code CLAUDE.md")


def planned_claude_removal(claude_md_path: Path) -> str | None:
    if not claude_md_path.exists():
        return None
    existing = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    span = locate_managed_block(existing, claude_md_path)
    if span is None:
        return existing
    return existing[: span[0]] + existing[span[1] :]


def remove_claude_md_block(state: SetupState, claude_md_path: Path, planned: str | None = None) -> None:
    if not claude_md_path.exists():
        print(f"OK: {display_path(claude_md_path)} does not exist; nothing to remove")
        return
    existing = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    remaining = planned if planned is not None else planned_claude_removal(claude_md_path)
    if remaining == existing:
        print(f"OK: no PIRA block in {display_path(claude_md_path)}")
        return
    assert remaining is not None
    if remaining:
        write_text(state, claude_md_path, remaining, "Claude Code CLAUDE.md")
        return
    if state.dry_run:
        print(f"DRY-RUN: would back up and delete {display_path(claude_md_path)} because only the PIRA block remains")
        state.note_change(f"would delete {display_path(claude_md_path)}")
        return
    backup_file = backup_path(claude_md_path)
    shutil.copy2(claude_md_path, backup_file)
    print(f"Backup: {display_path(claude_md_path)} -> {display_path(backup_file)}")
    claude_md_path.unlink()
    state.note_change(f"deleted {display_path(claude_md_path)} (only the PIRA block remained)")


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def safe_manifest_path(value: object) -> str:
    if not isinstance(value, str) or "\\" in value:
        raise RuntimeError("PIRA install manifest contains an invalid managed path")
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        raise RuntimeError(f"PIRA install manifest contains an unsafe managed path: {value!r}")
    if value != "AGENTS.md" and path.parts[0] != "modules":
        raise RuntimeError(f"PIRA install manifest claims an unexpected file: {value!r}")
    return value


def read_manifest(policy_dir: Path) -> dict[str, object] | None:
    path = policy_dir / MANIFEST_NAME
    if not path.exists() and not path.is_symlink():
        return None
    if path.is_symlink() or not path.is_file():
        raise RuntimeError(f"PIRA install manifest is not a regular file: {display_path(path)}")
    try:
        payload = json.loads(read_utf8_text(path, "PIRA install manifest"))
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"PIRA install manifest is invalid JSON: {display_path(path)}") from exc
    if not isinstance(payload, dict) or payload.get("schema_version") != MANIFEST_SCHEMA or payload.get("target") != "claude-code":
        raise RuntimeError(f"PIRA install manifest has an unsupported schema or target: {display_path(path)}")
    source_commit = payload.get("source_commit")
    source_branch = payload.get("source_branch")
    if (
        not isinstance(source_commit, str)
        or len(source_commit) not in {40, 64}
        or any(character not in "0123456789abcdef" for character in source_commit)
        or (source_branch is not None and not isinstance(source_branch, str))
        or not isinstance(payload.get("source_dirty"), bool)
    ):
        raise RuntimeError(f"PIRA install manifest has invalid source metadata: {display_path(path)}")
    files = payload.get("files")
    if not isinstance(files, list):
        raise RuntimeError("PIRA install manifest has no managed file list")
    seen: set[str] = set()
    for entry in files:
        if not isinstance(entry, dict):
            raise RuntimeError("PIRA install manifest contains an invalid file entry")
        relative = safe_manifest_path(entry.get("path"))
        digest = entry.get("sha256")
        if (
            relative in seen
            or not isinstance(digest, str)
            or len(digest) != 64
            or any(character not in "0123456789abcdef" for character in digest)
        ):
            raise RuntimeError(f"PIRA install manifest contains an invalid entry for {relative!r}")
        seen.add(relative)
    return payload


def manifest_files(manifest: dict[str, object] | None) -> dict[str, str]:
    if manifest is None:
        return {}
    return {str(entry["path"]): str(entry["sha256"]) for entry in manifest["files"] if isinstance(entry, dict)}


def tracked_policy_paths(repo_root: Path) -> list[str]:
    output = command_output(
        ["git", "-C", str(repo_root), "ls-files", "--", "AGENTS.md", "modules"],
        "list tracked Claude policy files",
    )
    paths = [line.strip().replace("\\", "/") for line in output.splitlines() if line.strip()]
    if "AGENTS.md" not in paths:
        raise RuntimeError("Claude PIRA source AGENTS.md is not tracked by Git")
    for relative in paths:
        if relative != "AGENTS.md":
            safe_manifest_path(relative)
    return sorted(paths)


def source_policy_files(state: SetupState) -> dict[str, bytes]:
    agents_path = state.repo_root / "AGENTS.md"
    modules_dir = state.repo_root / "modules"
    if not agents_path.is_file() or agents_path.is_symlink():
        raise RuntimeError(f"Claude PIRA source AGENTS.md is missing or unsafe: {display_path(agents_path)}")
    if not modules_dir.is_dir() or modules_dir.is_symlink():
        raise RuntimeError(f"Claude PIRA source modules directory is missing or unsafe: {display_path(modules_dir)}")
    source_agents = read_utf8_text(agents_path, "Claude PIRA source AGENTS.md")
    installed_root = claude_import_path(state.policy_dir)
    if DEFAULT_POLICY_DIR not in source_agents:
        raise RuntimeError(f"Claude PIRA source AGENTS.md does not reference {DEFAULT_POLICY_DIR}")
    rendered_agents = source_agents.replace(DEFAULT_POLICY_DIR, installed_root)
    if VERIFY_TOKEN not in rendered_agents:
        raise RuntimeError("Claude PIRA source AGENTS.md is missing the verification token")
    files = {"AGENTS.md": rendered_agents.encode("utf-8")}
    for relative in tracked_policy_paths(state.repo_root):
        if relative == "AGENTS.md":
            continue
        source = state.repo_root.joinpath(*PurePosixPath(relative).parts)
        if source.is_symlink():
            raise RuntimeError(f"Refusing to copy symlink from PIRA modules: {display_path(source)}")
        if not source.is_file():
            raise RuntimeError(f"Tracked Claude PIRA module is missing or not a file: {relative}")
        module_text = read_utf8_text(source, f"Claude PIRA source module {relative}")
        files[relative] = module_text.replace(DEFAULT_POLICY_DIR, installed_root).encode("utf-8")
    if len(files) == 1:
        raise RuntimeError("Claude PIRA source contains no module files")
    return files


def expected_manifest(state: SetupState, files: dict[str, bytes]) -> dict[str, object]:
    return {
        "schema_version": MANIFEST_SCHEMA,
        "target": "claude-code",
        "source_commit": state.source_commit,
        "source_branch": state.source_branch,
        "source_dirty": state.source_dirty,
        "files": [
            {"path": relative, "sha256": sha256_bytes(content)}
            for relative, content in sorted(files.items())
        ],
    }


def manifest_text(manifest: dict[str, object]) -> str:
    return json.dumps(manifest, indent=2, sort_keys=True) + "\n"


def managed_target(policy_dir: Path, relative: str) -> Path:
    target = policy_dir.joinpath(*PurePosixPath(safe_manifest_path(relative)).parts)
    if not path_under(target, policy_dir):
        raise RuntimeError(f"Managed path escapes the Claude PIRA directory: {relative!r}")
    return target


def validate_owned_files(policy_dir: Path, old_manifest: dict[str, object] | None, new_files: dict[str, bytes]) -> None:
    if policy_dir.is_symlink():
        raise RuntimeError(f"Claude PIRA policy directory must not be a symlink: {display_path(policy_dir)}")
    old_files = manifest_files(old_manifest)
    for relative in sorted(set(old_files) | set(new_files)):
        target = managed_target(policy_dir, relative)
        if not target.exists() and not target.is_symlink():
            continue
        if target.is_symlink() or not target.is_file():
            raise RuntimeError(f"Refusing to replace non-regular policy path: {display_path(target)}")
        if relative not in old_files:
            raise RuntimeError(f"Refusing to overwrite file not owned by the PIRA manifest: {display_path(target)}")
        current = hashlib.sha256(target.read_bytes()).hexdigest()
        if current != old_files[relative]:
            raise RuntimeError(f"Refusing to overwrite modified managed policy file: {display_path(target)}")


def prepare_policy_bundle(state: SetupState) -> tuple[dict[str, bytes], dict[str, object], dict[str, object] | None]:
    files = source_policy_files(state)
    manifest = expected_manifest(state, files)
    old_manifest = read_manifest(state.policy_dir)
    validate_owned_files(state.policy_dir, old_manifest, files)
    return files, manifest, old_manifest


def bundle_is_current(policy_dir: Path, files: dict[str, bytes], manifest: dict[str, object]) -> bool:
    manifest_path = policy_dir / MANIFEST_NAME
    if not manifest_path.is_file() or read_utf8_text(manifest_path, "PIRA install manifest") != manifest_text(manifest):
        return False
    for relative, content in files.items():
        target = managed_target(policy_dir, relative)
        if not target.is_file() or target.is_symlink() or target.read_bytes() != content:
            return False
    return True


def install_policy_bundle(
    state: SetupState,
    prepared: tuple[dict[str, bytes], dict[str, object], dict[str, object] | None] | None = None,
) -> None:
    files, manifest, old_manifest = prepared or prepare_policy_bundle(state)
    if bundle_is_current(state.policy_dir, files, manifest):
        print(f"OK: Claude PIRA policy bundle already up to date ({display_path(state.policy_dir)})")
        return
    if state.dry_run:
        print(f"DRY-RUN: would install validated Claude PIRA policy bundle at {display_path(state.policy_dir)}")
        state.note_change(f"would update {display_path(state.policy_dir)}")
        return

    state.policy_dir.parent.mkdir(parents=True, exist_ok=True)
    state.policy_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pira-stage-", dir=state.policy_dir.parent) as temporary:
        stage = Path(temporary)
        for relative, content in files.items():
            target = stage.joinpath(*PurePosixPath(relative).parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        (stage / MANIFEST_NAME).write_text(manifest_text(manifest), encoding="utf-8")
        for relative, content in files.items():
            staged = stage.joinpath(*PurePosixPath(relative).parts)
            if staged.read_bytes() != content:
                raise RuntimeError(f"Staged policy validation failed for {relative}")
        if read_manifest(stage) != manifest:
            raise RuntimeError("Staged policy manifest validation failed")

        old_files = manifest_files(old_manifest)
        affected = sorted(set(old_files) | set(files) | {MANIFEST_NAME})
        rollback = stage / ".rollback"
        existed: set[str] = set()
        for relative in affected:
            target = state.policy_dir / relative
            if target.is_file() and not target.is_symlink():
                saved = rollback / relative
                saved.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(target, saved)
                existed.add(relative)
        try:
            for relative in sorted(set(old_files) - set(files)):
                managed_target(state.policy_dir, relative).unlink(missing_ok=True)
            for relative in sorted(files):
                source = stage.joinpath(*PurePosixPath(relative).parts)
                target = managed_target(state.policy_dir, relative)
                target.parent.mkdir(parents=True, exist_ok=True)
                os.replace(source, target)
            os.replace(stage / MANIFEST_NAME, state.policy_dir / MANIFEST_NAME)
        except OSError as exc:
            for relative in reversed(affected):
                target = state.policy_dir / relative
                if target.is_file() or target.is_symlink():
                    target.unlink()
                if relative in existed:
                    saved = rollback / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    os.replace(saved, target)
            raise RuntimeError(f"Could not replace the Claude PIRA policy bundle; previous files were restored: {exc}") from exc
    state.note_change(f"installed Claude PIRA policy bundle at {display_path(state.policy_dir)}")


def prepare_user_md(state: SetupState, user_mode: Literal["keep", "placeholder", "interactive"]) -> str | None:
    user_path = state.policy_dir / "USER.md"
    if user_path.exists():
        if user_path.is_symlink() or not user_path.is_file():
            raise RuntimeError(f"Claude PIRA USER.md must be a regular file: {display_path(user_path)}")
        read_utf8_text(user_path, "Claude PIRA USER.md")
        return None
    if user_mode == "keep":
        return None
    if user_mode == "interactive" and not state.yes and sys.stdin.isatty():
        print("Claude PIRA USER.md is missing. A private placeholder is safe and can be edited later.")
        if not prompt_yes_no("Create a private placeholder USER.md now?", default=True):
            return None
    return USER_PLACEHOLDER_TEXT


def ensure_user_md(state: SetupState, prepared_content: str | None) -> None:
    user_path = state.policy_dir / "USER.md"
    if user_path.exists():
        print(f"OK: preserving Claude PIRA USER.md ({display_path(user_path)})")
        return
    if prepared_content is None:
        state.warn("Claude PIRA USER.md is missing; leaving it absent because --user-mode keep was selected")
        return
    write_text(state, user_path, prepared_content, "private Claude PIRA USER.md placeholder", backup=False)


def prepare_bundle_uninstall(state: SetupState) -> dict[str, object] | None:
    manifest = read_manifest(state.policy_dir)
    if manifest is not None:
        validate_owned_files(state.policy_dir, manifest, {})
    return manifest


def uninstall_policy_bundle(state: SetupState, manifest: dict[str, object] | None) -> None:
    if manifest is None:
        print(f"OK: no managed Claude PIRA policy bundle at {display_path(state.policy_dir)}")
        return
    owned = manifest_files(manifest)
    if state.dry_run:
        print(f"DRY-RUN: would remove manifest-owned policy files from {display_path(state.policy_dir)} and preserve USER.md")
        state.note_change(f"would uninstall {display_path(state.policy_dir)}")
        return
    for relative in sorted(owned, reverse=True):
        managed_target(state.policy_dir, relative).unlink(missing_ok=True)
    (state.policy_dir / MANIFEST_NAME).unlink(missing_ok=True)
    modules_dir = state.policy_dir / "modules"
    directories = sorted((path for path in modules_dir.rglob("*") if path.is_dir()), reverse=True) if modules_dir.exists() else []
    for directory in directories:
        try:
            directory.rmdir()
        except OSError:
            pass
    if modules_dir.exists():
        try:
            modules_dir.rmdir()
        except OSError:
            pass
    try:
        state.policy_dir.rmdir()
    except OSError:
        pass
    state.note_change(f"removed manifest-owned Claude PIRA policy files from {display_path(state.policy_dir)}")


def verify_claude(state: SetupState, claude_md_path: Path) -> None:
    if not claude_md_path.exists():
        state.check("Claude Code CLAUDE.md exists", False, display_path(claude_md_path))
        return
    text = read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    expected = claude_managed_block(state.policy_dir)
    state.check(
        "Claude Code PIRA import",
        text.count(CLAUDE_BLOCK_START) == 1 and text.count(CLAUDE_BLOCK_END) == 1 and expected in text,
        f"{display_path(claude_md_path)} -> {claude_import_path(state.policy_dir / 'AGENTS.md')}",
    )


def verify_claude_removed(state: SetupState, claude_md_path: Path) -> None:
    present = claude_md_path.exists() and CLAUDE_BLOCK_START in read_utf8_text(claude_md_path, "Claude Code CLAUDE.md")
    state.check("Claude Code PIRA import removed", not present, display_path(claude_md_path))


def verify_policy_bundle(state: SetupState) -> None:
    try:
        files = source_policy_files(state)
        expected = expected_manifest(state, files)
        installed = read_manifest(state.policy_dir)
    except RuntimeError as exc:
        state.check("Claude PIRA policy bundle", False, str(exc))
        return
    state.check("Claude PIRA install manifest", installed == expected, display_path(state.policy_dir / MANIFEST_NAME))
    installed_files = manifest_files(installed)
    files_ok = installed is not None and set(installed_files) == set(files)
    for relative, content in files.items():
        target = managed_target(state.policy_dir, relative)
        files_ok = files_ok and target.is_file() and not target.is_symlink() and target.read_bytes() == content
    state.check("Claude PIRA managed policy files", files_ok, f"{len(files)} files at {display_path(state.policy_dir)}")
    agents = state.policy_dir / "AGENTS.md"
    token_ok = agents.is_file() and VERIFY_TOKEN in read_utf8_text(agents, "installed Claude PIRA AGENTS.md")
    state.check("verification token", token_ok, VERIFY_TOKEN)
    user = state.policy_dir / "USER.md"
    state.check("Claude PIRA USER.md", True, display_path(user) if user.exists() else "optional and absent")


def verify_removed(state: SetupState) -> None:
    state.check("Claude PIRA install manifest removed", not (state.policy_dir / MANIFEST_NAME).exists(), display_path(state.policy_dir))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Install a stable PIRA policy snapshot for Claude Code.")
    parser.add_argument("--policy-dir", default=DEFAULT_POLICY_DIR, help="Installed Claude policy directory (default: ~/.claude/pira).")
    parser.add_argument("--claude-md", default="~/.claude/CLAUDE.md", help="Claude Code user instruction file.")
    parser.add_argument("--expected-source-branch", default=None, help="Fail unless the source checkout is on this branch; setup never switches branches.")
    parser.add_argument("--uninstall", action="store_true", help="Remove the managed CLAUDE.md block and manifest-owned policy files; preserve USER.md and tools.")
    parser.add_argument("--skip-tools", action="store_true", help="Do not install or refresh bundled PIRA tools.")
    parser.add_argument("--tools-install-dir", default=None, help="Override the per-user PIRA tools PATH directory.")
    parser.add_argument(
        "--tools-version",
        action="append",
        default=None,
        help="Pin a native tool as ctx=VERSION, dec=VERSION, nav=VERSION, or svg=VERSION; repeatable.",
    )
    parser.add_argument("--user-mode", choices=["interactive", "placeholder", "keep"], default="interactive")
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
    try:
        source_commit, source_branch, source_dirty = source_metadata(repo_root, args.expected_source_branch)
        state = SetupState(
            repo_root=repo_root,
            policy_dir=expand_path(args.policy_dir),
            dry_run=args.dry_run or args.verify,
            yes=args.yes,
            source_commit=source_commit,
            source_branch=source_branch,
            source_dirty=source_dirty,
        )
        claude_md_path = expand_path(args.claude_md)

        print("PIRA setup (Claude Code)")
        dirty_suffix = ", dirty" if source_dirty else ""
        print(f"Source:      {display_path(repo_root)} @ {source_commit[:7]} ({source_branch or 'detached HEAD'}{dirty_suffix})")
        print(f"Policy dir:  {display_path(state.policy_dir)}")
        print(f"CLAUDE.md:   {display_path(claude_md_path)}")
        print(f"Dry run:     {state.dry_run}")

        if args.uninstall:
            if args.verify:
                raise RuntimeError("--uninstall and --verify cannot be combined")
            planned_removal = planned_claude_removal(claude_md_path)
            old_manifest = prepare_bundle_uninstall(state)
            remove_claude_md_block(state, claude_md_path, planned_removal)
            uninstall_policy_bundle(state, old_manifest)
            if not args.dry_run:
                verify_claude_removed(state, claude_md_path)
                verify_removed(state)
        elif args.verify:
            verify_policy_bundle(state)
            verify_claude(state, claude_md_path)
            if not args.skip_tools:
                configure_tools(state, args.tools_install_dir, args.tools_version, verify_only=True)
        else:
            # Validate every outcome-changing path and source before the first write.
            planned_claude = planned_claude_md(claude_md_path, state.policy_dir)
            prepared_bundle = prepare_policy_bundle(state)
            prepared_user = prepare_user_md(state, args.user_mode)
            install_policy_bundle(state, prepared_bundle)
            ensure_user_md(state, prepared_user)
            update_claude_md(state, claude_md_path, planned_claude)
            if not args.skip_tools:
                configure_tools(state, args.tools_install_dir, args.tools_version, verify_only=False)
            if args.dry_run:
                print("DRY-RUN: verification skipped because planned changes were not applied")
            else:
                verify_policy_bundle(state)
                verify_claude(state, claude_md_path)
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
