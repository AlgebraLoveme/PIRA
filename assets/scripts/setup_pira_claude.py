"""Claude setup planning; native CLI installation and authentication are caller-owned."""
from __future__ import annotations

import json
import os
from pathlib import Path

IS_WINDOWS = os.name == "nt"
AGENTS_PLUGIN = "cc-plugin-agents-md@builtin"
START = "<!-- PIRA managed instructions -->"
END = "<!-- End PIRA managed instructions -->"


def read_file(path: Path) -> str:
    """Read optional Claude configuration, rejecting non-regular files."""
    if path.is_symlink() or (path.exists() and not path.is_file()):
        raise RuntimeError(f"Refusing non-regular Claude configuration: {path}")
    if not path.exists():
        return ""
    with path.open(encoding="utf-8", newline="") as source:
        return source.read()


def import_line(target: Path) -> str:
    """Reference the canonical instructions without duplicating policy text."""
    path = target.as_posix()
    if any(c in path for c in "\r\n\t`"):
        raise RuntimeError("PIRA instruction path contains unsupported import characters")
    return "@" + path.replace(" ", "\\ ") + "\n"


def matches(path: Path, content: str | Path) -> bool:
    """Check a planned file or canonical link, including Windows import fallback."""
    if isinstance(content, Path):
        if path.is_symlink():
            return path.is_file() and path.resolve() == content.resolve()
        return IS_WINDOWS and path.is_file() and read_file(path) == import_line(content)
    return not path.is_symlink() and path.is_file() and read_file(path) == content


def instruction_plan(instructions: Path, target: Path) -> str | Path:
    """Link canonical AGENTS, preserving custom global instructions with an import."""
    line = import_line(target)
    if instructions.is_symlink():
        if instructions.resolve() != target.resolve():
            raise RuntimeError(f"Conflicting Claude instruction symlink: {instructions}")
        return target
    original = read_file(instructions)
    if original.count(START) != original.count(END) or original.count(START) > 1:
        raise RuntimeError(f"Malformed PIRA instruction block: {instructions}")
    if START in original:
        start, end = original.index(START), original.index(END)
        if end < start:
            raise RuntimeError(f"Reversed PIRA instruction markers: {instructions}")
        body = original[start + len(START):end].strip("\r\n").splitlines()
        # Recognize only the previously generated import and optional legacy hint.
        if (not body or not body[0].startswith("@") or not body[0].endswith("/AGENTS.md")
                or len(body) > 2 or (len(body) == 2 and body[1] !=
                    "If the working directory has AGENTS.md and it is not already in context, read it before work.")):
            raise RuntimeError(f"Customized PIRA instruction block; preserve it manually: {instructions}")
        before, after = original[:start], original[end + len(END):]
        return before + line.rstrip("\n") + after if (before + after).strip() else target
    if not original.strip() or original.strip() == line.strip():
        return target
    if line.rstrip("\n") in original.splitlines():
        return original
    separator = "\n" if not original.endswith("\n") else ""
    return original + separator + line


def plan(
    directory: Path,
    agent_dir: Path,
    store_paths: dict[str, str],
    execution_mode: str,
) -> dict[Path, str | Path]:
    """Plan settings text and instruction text/link targets without writing files."""
    if directory.is_symlink() or (directory.exists() and not directory.is_dir()):
        raise RuntimeError(f"Refusing non-directory Claude configuration: {directory}")
    if execution_mode == "safe":
        raise RuntimeError(
            "Claude Team supports full-access only; use soft-safe or keep. "
            "Codex workspace-write has no Claude equivalent."
        )
    instructions = directory / "CLAUDE.md"
    instruction_content = instruction_plan(instructions, agent_dir / "AGENTS.md")

    settings = directory / "settings.json"
    raw = read_file(settings)
    value = json.loads(raw) if raw.strip() else {}
    if not isinstance(value, dict):
        raise RuntimeError("Claude settings must be a JSON object")
    before = json.dumps(value, sort_keys=True)
    env = value.setdefault("env", {})
    if not isinstance(env, dict):
        raise RuntimeError("Claude env settings must be a JSON object")
    for key, selected in store_paths.items():
        existing = env.get(key)
        if existing is not None and (
            not isinstance(existing, str)
            or Path(existing).resolve() != Path(selected).resolve()
        ):
            raise RuntimeError(
                f"Conflicting Claude {key}; explicitly select the intended store in the "
                "environment before rerunning setup. Existing records and settings are unchanged."
            )
        env[key] = selected
    enabled = value.setdefault("enabledPlugins", {})
    if not isinstance(enabled, dict):
        raise RuntimeError("Claude enabledPlugins settings must be a JSON object")
    enabled[AGENTS_PLUGIN] = True
    options = value
    for key in ("pluginConfigs", AGENTS_PLUGIN, "options"):
        options = options.setdefault(key, {})
        if not isinstance(options, dict):
            raise RuntimeError(f"Claude {key} settings must be a JSON object")
    options["instructionFiles"] = "claude-md-and-agents-md"
    if execution_mode == "soft-safe":
        for key in ("permissions", "sandbox"):
            if not isinstance(value.setdefault(key, {}), dict):
                raise RuntimeError(f"Claude {key} settings must be a JSON object")
        value["permissions"]["defaultMode"] = "bypassPermissions"
        value["sandbox"]["enabled"] = False
    if raw and before == json.dumps(value, sort_keys=True):
        rendered = raw
    else:
        rendered = json.dumps(value, indent=2, ensure_ascii=False) + "\n"
    return {instructions: instruction_content, settings: rendered}
