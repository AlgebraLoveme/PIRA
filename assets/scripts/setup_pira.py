#!/usr/bin/env python3
"""Deterministic setup helper for PIRA.

The script intentionally uses only the Python standard library. It configures the
current machine for the existing global PIRA layout centered on ``~/agent`` and
keeps all writes explicit, backed up, and verifiable.
"""

from __future__ import annotations

import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Iterable, Literal

VERIFY_TOKEN = "31415926535897932384626433832795"
DEFAULT_PROJECT_DOC_MAX_BYTES = "65536"
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
PROJECT_AGENTS_GUARD = """# PIRA Repository Guard

PIRA's global policy is already loaded from `AGENTS.md` through Codex `model_instructions_file`; do not load it again. If that policy is absent from the current context, read `AGENTS.md` before proceeding.
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


def config_path_string(path: Path) -> str:
    """Return a stable config path string without resolving symlinks."""
    expanded = path.expanduser()
    if not expanded.is_absolute():
        expanded = Path.cwd() / expanded
    home = Path.home()
    try:
        return "~/" + str(expanded.relative_to(home))
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
    answer = input(f"{question} {suffix} ").strip().lower()
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


def write_text(state: SetupState, path: Path, content: str, description: str, *, backup: bool = True) -> None:
    old = path.read_text(encoding="utf-8") if path.exists() else None
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


def split_toml_preamble(text: str) -> tuple[list[str], list[str]]:
    lines = text.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if re.match(r"\s*\[", line):
            return lines[:index], lines[index:]
    return lines, []


def top_level_keys(text: str) -> dict[str, str]:
    preamble, _ = split_toml_preamble(text)
    result: dict[str, str] = {}
    for line in preamble:
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        result[key.strip()] = value.strip()
    return result


def toml_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def upsert_top_level(text: str, updates: dict[str, str], remove_keys: Iterable[str] = ()) -> str:
    remove_set = set(remove_keys)
    preamble, rest = split_toml_preamble(text)
    seen: set[str] = set()
    new_preamble: list[str] = []
    key_pattern = re.compile(r"^(\s*)([A-Za-z0-9_.-]+)(\s*=)(.*)$")
    for line in preamble:
        match = key_pattern.match(line)
        if not match:
            new_preamble.append(line)
            continue
        key = match.group(2)
        if key in remove_set:
            continue
        if key in updates:
            new_preamble.append(f"{key} = {updates[key]}\n")
            seen.add(key)
        else:
            new_preamble.append(line)
    additions = [f"{key} = {value}\n" for key, value in updates.items() if key not in seen]
    if additions:
        if new_preamble and new_preamble[-1].strip() != "":
            new_preamble.append("\n")
        new_preamble.extend(additions)
    if rest and new_preamble and new_preamble[-1].strip() != "":
        new_preamble.append("\n")
    return "".join(new_preamble + rest)


def configure_codex(
    state: SetupState,
    config_path: Path,
    execution_mode: Literal["ask", "safe", "soft-safe", "keep"],
    replace_permissions: bool,
) -> None:
    if execution_mode == "ask":
        if state.yes or not sys.stdin.isatty():
            execution_mode = "keep"
            state.warn("Execution mode left unchanged; pass --execution-mode safe or soft-safe to set it non-interactively")
        else:
            print("Execution modes:")
            print("  1. safe: approval_policy=on-request, sandbox_mode=workspace-write")
            print("  2. soft-safe: approval_policy=never, sandbox_mode=danger-full-access")
            print("  3. keep: do not change approval/sandbox settings")
            choice = input("Choose execution mode [1/2/3, default 1]: ").strip()
            execution_mode = {"": "safe", "1": "safe", "2": "soft-safe", "3": "keep"}.get(choice, "safe")  # type: ignore[assignment]

    existing = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
    keys = top_level_keys(existing)
    instructions_path = state.agent_dir / "AGENTS.md"
    instructions_ref = config_path_string(instructions_path)
    updates = {
        "model_instructions_file": toml_string(instructions_ref),
        "project_doc_max_bytes": DEFAULT_PROJECT_DOC_MAX_BYTES,
    }
    remove_keys: list[str] = []
    if execution_mode == "safe":
        updates.update({"approval_policy": toml_string("on-request"), "sandbox_mode": toml_string("workspace-write")})
    elif execution_mode == "soft-safe":
        updates.update({"approval_policy": toml_string("never"), "sandbox_mode": toml_string("danger-full-access")})

    if execution_mode in {"safe", "soft-safe"} and "default_permissions" in keys:
        if not replace_permissions:
            state.warn(
                "Codex config has top-level default_permissions; not setting sandbox_mode because Codex docs warn not to combine them. "
                "Rerun with --replace-permissions to remove default_permissions and set the selected mode."
            )
            updates.pop("sandbox_mode", None)
        else:
            remove_keys.append("default_permissions")

    new_text = upsert_top_level(existing, updates, remove_keys=remove_keys)
    write_text(state, config_path, new_text, "Codex config.toml")
    ensure_project_agents_guard(state)
    remove_duplicate_global_agents(state, config_path.parent / "AGENTS.md", instructions_path)


def ensure_project_agents_guard(state: SetupState) -> None:
    """Prevent the configured global policy from being rediscovered in the PIRA repo."""
    write_text(
        state,
        state.agent_dir / "AGENTS.override.md",
        PROJECT_AGENTS_GUARD,
        "local PIRA repository AGENTS guard",
        backup=False,
    )


def remove_duplicate_global_agents(state: SetupState, global_path: Path, pira_path: Path) -> None:
    """Remove only the old PIRA symlink; preserve unrelated global instructions."""
    global_label = config_path_string(global_path)
    if not global_path.is_symlink() or not same_location(global_path, pira_path):
        if global_path.exists() or global_path.is_symlink():
            state.warn(f"Preserved separate global instructions at {global_label}")
        return
    if state.dry_run:
        print(f"DRY-RUN: would remove duplicate PIRA symlink {global_label}")
        state.note_change(f"would remove duplicate {global_label} symlink")
        return
    global_path.unlink()
    state.note_change(f"removed duplicate {global_label} symlink")


def configure_audio(
    state: SetupState,
    audio: Literal["ask", "yes", "no"],
    config_path: Path,
    audio_dir: Path | None,
    force_audio: bool,
) -> None:
    system = platform.system().lower()
    supported = system in {"darwin", "windows"}
    if audio == "ask":
        if not supported:
            print("OK: audio notifications are supported only on macOS and Windows; skipping")
            return
        if state.yes or not sys.stdin.isatty():
            print("OK: audio notifications not enabled by default")
            return
        audio = "yes" if prompt_yes_no("Enable optional Codex audio notifications?", default=False) else "no"
    if audio == "no":
        print("OK: audio notifications skipped")
        return
    if not supported:
        raise RuntimeError("Audio setup is supported only on macOS and Windows")

    if audio_dir is None:
        audio_dir = state.agent_dir / "PIRA_Voice" / "Samantha"
    for name in ["complete_msg.m4a", "waiting_msg.m4a"]:
        candidate = audio_dir / name
        if not candidate.exists():
            raise RuntimeError(f"Audio file missing: {candidate}")

    script = state.agent_dir / "assets" / "scripts" / "setup_codex_audio_mode.py"
    platform_name = "macos" if system == "darwin" else "windows"
    cmd = [sys.executable, str(script), "--platform", platform_name, "--config", str(config_path), "--audio-dir", str(audio_dir)]
    if force_audio:
        cmd.append("--force")

    if state.dry_run:
        print("DRY-RUN: would run audio setup command:")
        print("  " + " ".join(sh_quote(part) for part in cmd))
        state.note_change("would configure Codex audio notifications")
        return
    subprocess.run(cmd, check=True)
    state.note_change("configured Codex audio notifications")


def sh_quote(value: str) -> str:
    if re.fullmatch(r"[A-Za-z0-9_./:=+-]+", value):
        return value
    return "'" + value.replace("'", "'\\''") + "'"


def verify(state: SetupState, config_path: Path, skip_codex: bool) -> None:
    def add(name: str, passed: bool, detail: str) -> None:
        state.verification.append((name, passed, detail))
        label = "PASS" if passed else "FAIL"
        print(f"{label}: {name} — {detail}")

    agents = state.agent_dir / "AGENTS.md"
    add("AGENTS.md exists", agents.exists(), display_path(agents))
    user = state.agent_dir / "USER.md"
    add("USER.md exists", user.exists(), display_path(user))
    token_ok = agents.exists() and VERIFY_TOKEN in agents.read_text(encoding="utf-8")
    add("verification token", token_ok, VERIFY_TOKEN)
    legacy_existing = [path for path in parse_legacy_paths(pira_source_root(state), state.agent_dir) if path.exists() or path.is_symlink()]
    add("legacy files absent", not legacy_existing, ", ".join(display_path(p) for p in legacy_existing) or "none")

    if not skip_codex:
        if not config_path.exists():
            add("Codex config exists", False, display_path(config_path))
        else:
            text = config_path.read_text(encoding="utf-8")
            keys = top_level_keys(text)
            expected_instructions = toml_string(config_path_string(state.agent_dir / "AGENTS.md"))
            add("Codex config points to PIRA", keys.get("model_instructions_file") == expected_instructions, f"{display_path(config_path)} -> {expected_instructions}")
            add("Codex project_doc_max_bytes", keys.get("project_doc_max_bytes") == DEFAULT_PROJECT_DOC_MAX_BYTES, keys.get("project_doc_max_bytes", "missing"))
            guard = state.agent_dir / "AGENTS.override.md"
            add("PIRA repository duplicate guard", guard.exists() and guard.read_text(encoding="utf-8") == PROJECT_AGENTS_GUARD, display_path(guard))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Set up PIRA for the current machine.")
    parser.add_argument("--agent-dir", default="~/agent", help="Global PIRA path to configure (default: ~/agent).")
    parser.add_argument("--codex-config", default="~/.codex/config.toml", help="Codex config.toml path.")
    parser.add_argument("--skip-codex", action="store_true", help="Do not edit Codex configuration.")
    parser.add_argument("--skip-tools", action="store_true", help="Do not install or refresh bundled PIRA tools.")
    parser.add_argument("--tools-install-dir", default=None, help="Override the per-user PIRA tools PATH directory.")
    parser.add_argument(
        "--tools-version",
        action="append",
        default=None,
        help=(
            "Pin a native tool as ctx=VERSION, dec=VERSION, nav=VERSION, or "
            "svg=VERSION; repeatable."
        ),
    )
    parser.add_argument("--execution-mode", choices=["ask", "safe", "soft-safe", "keep"], default="ask")
    parser.add_argument("--replace-permissions", action="store_true", help="Remove top-level default_permissions when setting sandbox_mode.")
    parser.add_argument("--user-mode", choices=["interactive", "placeholder", "keep"], default="interactive")
    parser.add_argument("--legacy", choices=["ask", "remove", "keep"], default="ask", help="How to handle paths listed in assets/LEGACY_LIST.md.")
    parser.add_argument("--force-agent-link", action="store_true", help="Move a conflicting --agent-dir aside and symlink this repo there.")
    parser.add_argument("--audio", choices=["ask", "yes", "no"], default="ask", help="Whether to install optional Codex audio notifications.")
    parser.add_argument("--audio-dir", default=None, help="Audio set directory for optional Codex audio notifications.")
    parser.add_argument("--force-audio", action="store_true", help="Allow the audio helper to replace an existing notify entry.")
    parser.add_argument("--verify", action="store_true", help="Only verify the current setup; do not write.")
    parser.add_argument("--dry-run", action="store_true", help="Print planned changes without writing.")
    parser.add_argument("--yes", action="store_true", help="Assume yes for setup confirmations; does not enable audio unless --audio yes is set.")
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
    config_path = expand_path(args.codex_config)
    audio_dir = expand_path(args.audio_dir) if args.audio_dir else None

    print("PIRA setup")
    print(f"Repository: {display_path(repo_root)}")
    print(f"Agent dir:  {display_path(state.agent_dir)}")
    print(f"Dry run:    {state.dry_run}")

    try:
        if not args.verify:
            ensure_agent_dir(state, force_agent_link=args.force_agent_link)
            ensure_user_md(state, args.user_mode)
            remove_legacy_files(state, args.legacy)
            if not args.skip_codex:
                configure_codex(state, config_path, args.execution_mode, args.replace_permissions)
            configure_audio(state, args.audio, config_path, audio_dir, args.force_audio)
            if not args.skip_tools:
                configure_tools(
                    state,
                    args.tools_install_dir,
                    args.tools_version,
                    verify_only=False,
                )
        if args.dry_run and not args.verify:
            print("DRY-RUN: verification skipped because planned changes were not applied")
        else:
            verify(state, config_path, skip_codex=args.skip_codex)
            if args.verify and not args.skip_tools:
                configure_tools(
                    state,
                    args.tools_install_dir,
                    args.tools_version,
                    verify_only=True,
                )
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
