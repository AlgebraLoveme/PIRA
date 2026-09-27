#!/usr/bin/env python3
"""Connect shared PIRA policy through a Claude user rule, without a CLAUDE.md shim."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime
from pathlib import Path

VERIFY_TOKEN = "31415926535897932384626433832795"
MODE = "claude-md-and-agents-md"
PLUGIN = "agents-md@builtin"
MANIFEST = "rule_install.json"
RULE_FILE = "pira.md"
LEGACY_START = "<!-- PIRA:BEGIN (managed by setup_pira.py; do not edit inside) -->"
LEGACY_END = "<!-- PIRA:END -->"
USER_PLACEHOLDER = """# USER

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


def expand(value: str) -> Path:
    return Path(os.path.expanduser(os.path.expandvars(value))).absolute()


def backup(path: Path) -> Path:
    stamp = datetime.now().strftime("%Y%m%d%H%M%S%f")
    target = path.with_name(f"{path.name}.bak.{stamp}")
    shutil.copy2(path, target, follow_symlinks=False)
    print(f"Backup: {path} -> {target}")
    return target


def write_bytes(path: Path, content: bytes, *, dry_run: bool) -> None:
    if path.is_symlink():
        raise RuntimeError(f"refusing to replace symlinked file: {path}")
    if path.is_file() and path.read_bytes() == content:
        return
    if dry_run:
        print(f"DRY-RUN: would write {path}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        backup(path)
    handle, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(handle, "wb") as stream:
            stream.write(content)
        os.chmod(name, 0o600)
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)
    print(f"Updated: {path}")


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def source_policy(agent_dir: Path) -> Path:
    policy = agent_dir / "AGENTS.md"
    if not policy.is_file():
        raise RuntimeError(f"expected a readable PIRA policy at {policy}")
    try:
        content = policy.read_text(encoding="utf-8")
    except UnicodeError as error:
        raise RuntimeError(f"PIRA policy is not UTF-8: {policy}") from error
    if VERIFY_TOKEN not in content:
        raise RuntimeError(f"PIRA verification token missing from {policy}")
    if not (agent_dir / "modules" / "CODING_STYLE.md").is_file():
        raise RuntimeError(f"PIRA modules missing from {agent_dir}")
    return policy


def claude_version(command: str) -> tuple[int, int, int]:
    result = subprocess.run([command, "--version"], capture_output=True, text=True, check=True)
    match = re.match(r"(\d+)\.(\d+)\.(\d+) \(Claude Code\)", result.stdout.strip())
    if not match:
        raise RuntimeError(f"cannot identify Claude Code version: {result.stdout.strip()}")
    version = tuple(map(int, match.groups()))
    if version < (2, 1, 281):
        raise RuntimeError("native AGENTS.md setup requires Claude Code v2.1.281 or later")
    return version


def read_object(path: Path, description: str) -> dict[str, object]:
    if path.is_symlink():
        raise RuntimeError(f"refusing unsafe {description}: {path}")
    if not path.exists():
        return {}
    if not path.is_file():
        raise RuntimeError(f"refusing unsafe {description}: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"invalid {description} at {path}: {error}") from error
    if not isinstance(data, dict):
        raise RuntimeError(f"{description} must be a JSON object: {path}")
    return data


def settings_mode(data: dict[str, object]) -> tuple[bool, str | None]:
    plugins = data.get("pluginConfigs", {})
    if not isinstance(plugins, dict):
        raise RuntimeError("Claude settings pluginConfigs must be an object")
    entry = plugins.get(PLUGIN, {})
    if not isinstance(entry, dict):
        raise RuntimeError(f"Claude settings {PLUGIN} must be an object")
    options = entry.get("options", {})
    if not isinstance(options, dict):
        raise RuntimeError(f"Claude settings {PLUGIN}.options must be an object")
    value = options.get("instructionFiles")
    if "instructionFiles" in options and not isinstance(value, str):
        raise RuntimeError("Claude instructionFiles setting must be a string")
    return "instructionFiles" in options, value


def permission_anchor(directory: Path) -> str:
    if any(any(char in part for char in "*?[]\n\r") for part in directory.parts):
        raise RuntimeError(f"cannot safely express Claude read path: {directory}")
    try:
        relative = directory.relative_to(Path.home())
    except ValueError:
        absolute = directory.as_posix()
        if os.name == "nt":
            if ":" not in absolute:
                raise RuntimeError(
                    f"network paths are not supported for Claude permissions: {directory}"
                )
            drive, rest = absolute.split(":", 1)
            absolute = f"/{drive.lower()}{rest}"
        return "//" + absolute.lstrip("/")
    return "~/" + relative.as_posix()


def module_read_rule(agent_dir: Path) -> str:
    return f"Read({permission_anchor(agent_dir)}/modules/*.md)"


def profile_read_rule(claude_dir: Path) -> str:
    return f"Read({permission_anchor(claude_dir)}/pira/USER.md)"


def shared_profile_read_rule(agent_dir: Path) -> str:
    return f"Read({permission_anchor(agent_dir)}/USER.md)"


def settings_allow(data: dict[str, object]) -> list[str]:
    permissions = data.get("permissions", {})
    if not isinstance(permissions, dict):
        raise RuntimeError("Claude settings permissions must be an object")
    allowed = permissions.get("allow", [])
    if not isinstance(allowed, list) or any(not isinstance(rule, str) for rule in allowed):
        raise RuntimeError("Claude settings permissions.allow must be a list of strings")
    return allowed


def set_module_rule(data: dict[str, object], rule: str, *, remove: bool = False) -> dict[str, object]:
    data = json.loads(json.dumps(data))
    allowed = settings_allow(data)
    if remove:
        if rule not in allowed:
            return data
        allowed = [item for item in allowed if item != rule]
        if allowed:
            data["permissions"]["allow"] = allowed
        else:
            data["permissions"].pop("allow", None)
            if not data["permissions"]:
                data.pop("permissions", None)
    elif rule not in allowed:
        data.setdefault("permissions", {})["allow"] = [*allowed, rule]
    return data


def set_mode(data: dict[str, object], value: str | None, *, remove: bool = False) -> dict[str, object]:
    data = json.loads(json.dumps(data))
    plugins = data.setdefault("pluginConfigs", {})
    entry = plugins.setdefault(PLUGIN, {})
    options = entry.setdefault("options", {})
    if remove:
        options.pop("instructionFiles", None)
        if not options:
            entry.pop("options", None)
        if not entry:
            plugins.pop(PLUGIN, None)
        if not plugins:
            data.pop("pluginConfigs", None)
    else:
        options["instructionFiles"] = value
    return data


def json_bytes(data: dict[str, object]) -> bytes:
    return (json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def read_manifest(path: Path) -> dict[str, object] | None:
    data = read_object(path, "PIRA Claude install manifest") if path.exists() or path.is_symlink() else None
    if data is not None:
        if data.get("schema") != 1 or data.get("target") != "claude-rule":
            raise RuntimeError(f"unsupported PIRA Claude install manifest: {path}")
        if (not isinstance(data.get("previous_mode_present"), bool)
                or not isinstance(data.get("settings_existed"), bool)
                or not isinstance(data.get("previous_mode"), (str, type(None)))
                or not isinstance(data.get("policy_path"), str)
                or not isinstance(data.get("module_read_rule"), str)
                or not isinstance(data.get("module_read_rule_added"), bool)
                or not isinstance(data.get("permissions_present"), bool)
                or not isinstance(data.get("allow_present"), bool)):
            raise RuntimeError(f"invalid PIRA Claude install manifest: {path}")
        for key in ("profile_read_rule", "shared_profile_read_rule"):
            present = key in data
            added_key = f"{key}_added"
            if (present != (added_key in data)
                    or (present and (
                        not isinstance(data[key], str)
                        or not isinstance(data[added_key], bool)
                    ))):
                raise RuntimeError(f"invalid PIRA Claude install manifest: {path}")
        if (not isinstance(data.get("policy_sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", data["policy_sha256"])):
            raise RuntimeError(f"invalid PIRA Claude install manifest: {path}")
    return data


def legacy_without_pira(path: Path) -> bytes | None:
    if path.is_symlink():
        raise RuntimeError(f"refusing unsafe Claude instructions: {path}")
    if not path.exists():
        return None
    if not path.is_file():
        raise RuntimeError(f"refusing unsafe Claude instructions: {path}")
    try:
        text = path.read_bytes().decode("utf-8")
    except UnicodeError as error:
        raise RuntimeError(f"Claude instructions are not UTF-8: {path}") from error
    if LEGACY_START not in text and LEGACY_END not in text:
        return None
    if text.count(LEGACY_START) != 1 or text.count(LEGACY_END) != 1:
        raise RuntimeError(f"ambiguous PIRA-managed CLAUDE.md block: {path}")
    start = text.index(LEGACY_START)
    end = text.index(LEGACY_END, start) + len(LEGACY_END)
    if text[end:] in {"\n", "\r\n"}:
        end = len(text)
    body = text[start + len(LEGACY_START):text.index(LEGACY_END, start)].strip()
    if not re.fullmatch(r"@(?:~|[^ \t\r\n]+)/\.claude/pira/AGENTS\.md", body):
        raise RuntimeError(f"PIRA-managed CLAUDE.md block was edited: {path}")
    remaining = text[:start] + text[end:]
    return remaining.encode("utf-8") if remaining.strip() else b""


def install_entry(
    entry: Path, policy: Path, owned: dict[str, object] | None,
    *, dry_run: bool, copy_policy: bool
) -> None:
    if not copy_policy:
        if (entry.exists() or entry.is_symlink()) and owned is None:
            raise RuntimeError(f"existing Claude PIRA rule has no install manifest: {entry}")
        if entry.is_symlink() and entry.resolve() == policy.resolve():
            return
        if entry.exists() or entry.is_symlink():
            raise RuntimeError(f"existing Claude PIRA rule is not the managed link: {entry}")
        if dry_run:
            print(f"DRY-RUN: would link {entry} -> {policy}")
            return
        entry.parent.mkdir(parents=True, exist_ok=True)
        entry.symlink_to(policy)
        print(f"Linked: {entry} -> {policy}")
    else:
        expected = policy.read_bytes()
        if entry.exists() or entry.is_symlink():
            if owned is None:
                raise RuntimeError(f"existing Claude PIRA rule has no install manifest: {entry}")
            if entry.is_symlink() or not entry.is_file():
                raise RuntimeError(f"unsafe Claude PIRA rule: {entry}")
            current_hash = sha256(entry.read_bytes())
            owned_hash = owned.get("policy_sha256") if owned else None
            if current_hash != sha256(expected) and current_hash != owned_hash:
                raise RuntimeError(f"existing Claude PIRA rule is not managed by PIRA: {entry}")
        write_bytes(entry, expected, dry_run=dry_run)


def verify_entry(entry: Path, policy: Path, *, copy_policy: bool) -> bool:
    if not copy_policy:
        return entry.is_symlink() and entry.resolve() == policy.resolve()
    return entry.is_file() and not entry.is_symlink() and entry.read_bytes() == policy.read_bytes()


def verify_owned_entry(entry: Path, policy: Path, manifest: dict[str, object], *, copy_policy: bool) -> bool:
    if not copy_policy:
        return verify_entry(entry, policy, copy_policy=False)
    return (entry.is_file() and not entry.is_symlink()
            and sha256(entry.read_bytes()) == manifest["policy_sha256"])


def remove_legacy(path: Path, remaining: bytes | None, *, dry_run: bool) -> None:
    if remaining is None:
        return
    if dry_run:
        print(f"DRY-RUN: would remove the managed PIRA block from {path}")
        return
    if remaining:
        write_bytes(path, remaining, dry_run=False)
    else:
        backup(path)
        path.unlink()
        print(f"Removed empty managed Claude instructions: {path}")


def tools_setup(repo_root: Path, *, verify: bool, dry_run: bool) -> None:
    command = [sys.executable, str(repo_root / "assets/scripts/setup_pira_tools.py")]
    if verify:
        command.append("--verify")
    elif dry_run:
        command.append("--dry-run")
    subprocess.run(command, check=True)


def install(args: argparse.Namespace, repo_root: Path) -> None:
    agent_dir = expand(args.agent_dir)
    if agent_dir != expand("~/agent"):
        print(
            "WARNING: PIRA policy module paths assume ~/agent; "
            "a custom --agent-dir is for controlled testing."
        )
    claude_dir = expand(args.claude_dir)
    policy = source_policy(agent_dir)
    entry = claude_dir / "rules" / RULE_FILE
    settings_path = claude_dir / "settings.json"
    legacy_path = claude_dir / "CLAUDE.md"
    profile_path = claude_dir / "pira" / "USER.md"
    manifest_path = claude_dir / "pira" / MANIFEST
    for directory in (claude_dir / "rules", claude_dir / "pira"):
        if directory.is_symlink() or (directory.exists() and not directory.is_dir()):
            raise RuntimeError(f"unsafe Claude configuration directory: {directory}")
    copy_policy = os.name == "nt"
    version = claude_version(args.claude_bin)
    print(f"Claude Code: {'.'.join(map(str, version))}; PIRA policy: {policy}")
    manifest = read_manifest(manifest_path)
    settings = read_object(settings_path, "Claude settings")
    prior_present, prior_mode = settings_mode(settings)
    read_rule = module_read_rule(agent_dir)
    user_rule = profile_read_rule(claude_dir)
    shared_user_rule = shared_profile_read_rule(agent_dir)
    allowed = settings_allow(settings)
    print(f"PIRA module read allowance: {read_rule}")
    print(f"Claude private profile read allowance: {user_rule}")
    print(f"Shared fallback profile read allowance: {shared_user_rule}")
    legacy = legacy_without_pira(legacy_path)
    if entry.exists() or entry.is_symlink():
        install_entry(entry, policy, manifest, dry_run=True, copy_policy=copy_policy)
    if profile_path.is_symlink() or (profile_path.exists() and not profile_path.is_file()):
        raise RuntimeError(f"unsafe Claude PIRA user profile: {profile_path}")
    if manifest is None:
        manifest = {
            "schema": 1,
            "target": "claude-rule",
            "settings_existed": settings_path.exists(),
            "previous_mode_present": prior_present,
            "previous_mode": prior_mode,
            "policy_path": str(policy),
            "module_read_rule": read_rule,
            "module_read_rule_added": read_rule not in allowed,
            "profile_read_rule": user_rule,
            "profile_read_rule_added": user_rule not in allowed,
            "shared_profile_read_rule": shared_user_rule,
            "shared_profile_read_rule_added": shared_user_rule not in allowed,
            "permissions_present": "permissions" in settings,
            "allow_present": "permissions" in settings and "allow" in settings["permissions"],
            "policy_sha256": sha256(policy.read_bytes()),
        }
    else:
        if manifest["policy_path"] != str(policy):
            raise RuntimeError("PIRA policy path changed since Claude installation")
        if manifest["module_read_rule"] != read_rule:
            raise RuntimeError("PIRA source directory changed since Claude installation")
        for key, rule in (("profile_read_rule", user_rule),
                          ("shared_profile_read_rule", shared_user_rule)):
            if key in manifest and manifest[key] != rule:
                raise RuntimeError(f"Claude read permission path changed since installation: {key}")
            if key not in manifest:
                manifest[key] = rule
                manifest[f"{key}_added"] = rule not in allowed
        if copy_policy:
            manifest["policy_sha256"] = sha256(policy.read_bytes())
    if args.verify:
        checks = {
            "Claude PIRA user rule": verify_entry(entry, policy, copy_policy=copy_policy),
            "Claude both-files mode": prior_mode == MODE,
            "PIRA module read permission": read_rule in allowed,
            "Claude private profile read permission": user_rule in allowed,
            "Shared fallback profile read permission": shared_user_rule in allowed,
            "legacy PIRA CLAUDE.md bridge absent": legacy is None,
            "install manifest": manifest_path.is_file(),
        }
        if not args.skip_tools:
            tools_setup(repo_root, verify=True, dry_run=False)
        for name, okay in checks.items():
            print(f"{'PASS' if okay else 'FAIL'}: {name}")
        if not all(checks.values()):
            raise RuntimeError("Claude PIRA verification failed")
        return
    if not args.skip_tools:
        tools_setup(repo_root, verify=False, dry_run=args.dry_run)
    install_entry(entry, policy, read_manifest(manifest_path), dry_run=args.dry_run, copy_policy=copy_policy)
    configured = set_mode(settings, MODE)
    for rule in (read_rule, user_rule, shared_user_rule):
        configured = set_module_rule(configured, rule)
    if configured != settings:
        write_bytes(settings_path, json_bytes(configured), dry_run=args.dry_run)
    if not profile_path.exists() and args.user_mode == "placeholder":
        write_bytes(profile_path, USER_PLACEHOLDER.encode("utf-8"), dry_run=args.dry_run)
    remove_legacy(legacy_path, legacy, dry_run=args.dry_run)
    write_bytes(manifest_path, json_bytes(manifest), dry_run=args.dry_run)
    if not args.dry_run:
        print("PASS: Claude PIRA rule setup complete")


def uninstall(args: argparse.Namespace) -> None:
    claude_dir = expand(args.claude_dir)
    entry = claude_dir / "rules" / RULE_FILE
    manifest_path = claude_dir / "pira" / MANIFEST
    for directory in (claude_dir / "rules", claude_dir / "pira"):
        if directory.is_symlink() or (directory.exists() and not directory.is_dir()):
            raise RuntimeError(f"unsafe Claude configuration directory: {directory}")
    manifest = read_manifest(manifest_path)
    if manifest is None:
        raise RuntimeError("no PIRA rule install manifest; refusing to remove unmanaged files")
    policy = Path(manifest["policy_path"])
    if (not policy.is_absolute() or policy.name != "AGENTS.md"
            or manifest["module_read_rule"] != module_read_rule(policy.parent)):
        raise RuntimeError("invalid PIRA policy path in Claude install manifest")
    if not verify_owned_entry(entry, policy, manifest, copy_policy=os.name == "nt"):
        raise RuntimeError(f"Claude PIRA rule changed since installation: {entry}")
    settings_path = claude_dir / "settings.json"
    settings = read_object(settings_path, "Claude settings")
    _, current_mode = settings_mode(settings)
    settings_allow(settings)
    if current_mode != MODE:
        raise RuntimeError("Claude instructionFiles setting changed since PIRA installation")
    prior_present = manifest["previous_mode_present"]
    restored = set_mode(settings, manifest["previous_mode"], remove=not prior_present)
    removed_rule = False
    for key in ("module_read_rule", "profile_read_rule", "shared_profile_read_rule"):
        if manifest.get(f"{key}_added", False):
            restored = set_module_rule(restored, manifest[key], remove=True)
            removed_rule = True
    if removed_rule:
        if manifest["allow_present"] and not settings_allow(restored):
            restored.setdefault("permissions", {})["allow"] = []
        elif manifest["permissions_present"] and "permissions" not in restored:
            restored["permissions"] = {}
    if args.dry_run:
        print(f"DRY-RUN: would remove {entry} and {manifest_path} and restore {settings_path}")
        return
    if not manifest["settings_existed"] and not restored:
        backup(settings_path)
        settings_path.unlink()
    else:
        write_bytes(settings_path, json_bytes(restored), dry_run=False)
    entry.unlink()
    manifest_path.unlink()
    print(f"Removed PIRA user rule; preserved {claude_dir / 'pira' / 'USER.md'}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent-dir", default="~/agent", help="Canonical PIRA checkout, shared with Codex.")
    parser.add_argument("--claude-dir", default="~/.claude", help="Claude Code user configuration directory.")
    parser.add_argument("--claude-bin", default="claude", help="Claude Code executable for version checking.")
    parser.add_argument("--user-mode", choices=("placeholder", "keep"), default="placeholder")
    parser.add_argument(
        "--skip-tools", action="store_true", help="Skip shared native PIRA tool installation."
    )
    parser.add_argument("--dry-run", action="store_true", help="Preview without writing.")
    parser.add_argument("--verify", action="store_true", help="Check installed setup without writing.")
    parser.add_argument(
        "--uninstall", action="store_true", help="Remove only the managed user rule and setting."
    )
    args = parser.parse_args(argv)
    if args.uninstall and args.verify:
        parser.error("--uninstall and --verify are incompatible")
    try:
        if args.uninstall:
            uninstall(args)
        else:
            install(args, Path(__file__).resolve().parents[2])
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
