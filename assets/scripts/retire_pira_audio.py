#!/usr/bin/env python3
"""Retire historical PIRA audio only; never install tools or migrate stores.

Managed TOML blocks are parsed and checked against the independently derived
result before publication. Shared feature flags and all media are left alone:
there is no provenance for the old value of features.hooks. Modified/foreign
helper scripts remain in place, inactive, rather than being deleted by name.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import os
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path

START = "# BEGIN PIRA Codex speech notifications"
END = "# END PIRA Codex speech notifications"
STARTUP_MARKERS = (
    ("# BEGIN PIRA Codex startup audio", "# END PIRA Codex startup audio"),
    ("# BEGIN PIRA Codex startup speech wrapper", "# END PIRA Codex startup speech wrapper"),
)
# Shipped helper bodies, with only installer-substituted literal paths normalized.
HELPERS = {
    "speak_notify.sh": "9f8980712052b2bb6adddece5072b3f535c2a96960fc53cb99db60f21e3bc37b",
    "speak_waiting.sh": "30bb078c323992b8580f85721d7fd7bb1ccf0e5d55c7aa00e97c86c23a9a9c42",
    "speak_notify.ps1": "e6c81bd917af364d87e92d297f80ad3ee08461f536e855f101104e38f43a6dbd",
    "speak_waiting.ps1": "cef6ea434848e4e95bfbbedc5505a0834c0063408b7929227b9079cc0366fc19",
    "pira_play_audio.ps1": "95e820755b8a8b73dc13d313169faec59ca3d4c32276aa621c9cbd04d5278d22",
}


def managed_spans(text: str, start: str, end: str) -> list[tuple[int, int]]:
    """Require exact line markers, paired without nesting or stray ends."""
    result = []
    opened = None
    offset = 0
    for line in text.splitlines(keepends=True):
        token = line.rstrip("\r\n")
        if start in token or end in token:
            if token == start and opened is None:
                opened = offset
            elif token == end and opened is not None:
                result.append((opened, offset + len(line)))
                opened = None
            else:
                raise RuntimeError("Malformed/ambiguous PIRA audio markers")
        offset += len(line)
    if opened is not None:
        raise RuntimeError("Unterminated PIRA audio block")
    return result


def command_matches(value: object, hooks: Path, waiting: bool = False) -> bool:
    name = "speak_waiting" if waiting else "speak_notify"
    if waiting:
        script = str(hooks / (name + ".sh"))
        literal = "'" + script.replace("'", "'\"'\"'") + "'"
        if value in ("/bin/bash " + shlex.quote(script), "/bin/bash " + literal):
            return True
        if not isinstance(value, str):
            return False
        match = re.fullmatch(r'((?:.*[\\/])?(?:powershell|pwsh)(?:\.exe)?) -NoProfile -ExecutionPolicy Bypass -File "([^"\n]+)"', value, re.I)
        return bool(match and same_script(match[2], hooks / (name + ".ps1")))
    if not isinstance(value, list):
        return False
    if len(value) == 2 and value[0] == "/bin/bash":
        return same_script(value[1], hooks / (name + ".sh"))
    return (len(value) == 6 and isinstance(value[0], str)
            and re.fullmatch(r"(?:.*[\\/])?(?:powershell|pwsh)(?:\.exe)?", value[0], re.I) is not None
            and value[1:5] == ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
            and same_script(value[5], hooks / (name + ".ps1")))


def same_script(value: object, expected: Path) -> bool:
    if not isinstance(value, str):
        return False
    # Windows historical installers emit backslashes; no path is resolved or followed.
    left, right = value.replace("\\", "/"), str(expected).replace("\\", "/")
    return left.casefold() == right.casefold() if expected.suffix == ".ps1" else left == right


def toml_statements(text: str):
    """Locate complete source statements using tomllib, including multiline values.

    The complete document is validated first. Parsing individual statements
    supplies boundaries/keys, not replacement text or document-level semantics.
    """
    begin = None
    end = 0
    for line in text.splitlines(keepends=True):
        offset = end
        end += len(line)
        if begin is None:
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            begin = offset
        statement = text[begin:end]
        try:
            value = tomllib.loads(statement)
        except tomllib.TOMLDecodeError:
            continue
        yield begin, end, value, statement.lstrip().startswith("[")
        begin = None
    if begin is not None:
        raise RuntimeError("Cannot safely locate PIRA audio TOML statements")


def table_path(value: dict) -> tuple[str, ...]:
    """Decode a standalone header's quoted/dotted path without a key lexer."""
    path = []
    while isinstance(value, dict) and len(value) == 1:
        key, value = next(iter(value.items()))
        path.append(key)
        if isinstance(value, list) and len(value) == 1:
            value = value[0]
    if value != {}:
        raise RuntimeError("Ambiguous PIRA audio TOML header")
    return tuple(path)


def verified_waiting_event(event: dict, hooks: Path) -> bool:
    commands = event.get("hooks")
    if not isinstance(commands, list) or len(commands) != 1 or not isinstance(commands[0], dict):
        return False
    command = commands[0]
    return (set(event) == {"matcher", "hooks"} and event["matcher"] == "*"
            and set(command) == {"type", "command", "timeout", "statusMessage"}
            and command["type"] == "command" and type(command["timeout"]) is int
            and command["timeout"] == 1 and command["statusMessage"] == "Checking waiting status audio"
            and command_matches(command["command"], hooks, waiting=True))


def clean_config(text: str, hooks: Path) -> str:
    original = tomllib.loads(text)
    expected = copy.deepcopy(original)
    spans = managed_spans(text, START, END)
    cuts = []
    contents = []
    for begin, end in spans:
        first_end = text.index("\n", begin, end) + 1
        last_begin = text.rfind("\n", begin, end - 1) + 1
        contents.append((first_end, last_begin))
        cuts.extend(((begin, first_end), (last_begin, end)))

    def enclosing(begin: int, end: int) -> int | None:
        return next((i for i, (left, right) in enumerate(contents) if left <= begin and end <= right), None)

    seen = set()
    covered = set()

    def remove_owned(kind: str, parts: list[tuple[int, int]]) -> None:
        owner = enclosing(*parts[0])
        if owner is None or any(enclosing(*part) != owner for part in parts):
            raise RuntimeError("PIRA audio entry crosses marker boundaries; inspect before retirement")
        if kind in seen:
            raise RuntimeError("Duplicate PIRA audio blocks")
        seen.add(kind)
        covered.add(owner)
        cuts.extend(parts)

    scope = ()
    event_parts: list[list[tuple[int, int]]] = []
    explicit_hooks = False
    for begin, end, value, header in toml_statements(text) if spans else ():
        if header:
            scope = table_path(value)
            explicit_hooks |= scope == ("hooks",)
            if scope == ("hooks", "PermissionRequest") and text[begin:end].lstrip().startswith("[["):
                event_parts.append([])
        elif not scope and set(value) == {"notify"} and enclosing(begin, end) is not None:
            if not command_matches(value["notify"], hooks):
                raise RuntimeError("Foreign/custom PIRA notify; inspect before retirement")
            remove_owned("notify", [(begin, end)])
            expected.pop("notify")
        if scope[:2] == ("hooks", "PermissionRequest") and event_parts:
            event_parts[-1].append((begin, end))

    hooks_table = original.get("hooks", {})
    events = hooks_table.get("PermissionRequest", []) if isinstance(hooks_table, dict) else []
    removed_events = set()
    for index, parts in enumerate(event_parts):
        if enclosing(*parts[0]) is None:
            continue
        if not isinstance(events, list) or len(events) != len(event_parts):
            raise RuntimeError("Ambiguous PIRA waiting event layout")
        event = events[index]
        commands = event.get("hooks", []) if isinstance(event, dict) else []
        if isinstance(commands, dict):
            commands = [commands]  # Identify a customized object, never accept it as the old array.
        # Preserve independent events. Known signatures or indistinguishable
        # legacy-shaped events must validate exactly, not silently lose markers.
        owned = isinstance(commands, list) and any(
            isinstance(command, dict) and (command_matches(command.get("command"), hooks, waiting=True)
                                          or command.get("statusMessage") == "Checking waiting status audio"
                                          or (event.get("matcher") == "*" and set(command) ==
                                              {"type", "command", "timeout", "statusMessage"}))
            for command in commands)
        if owned:
            if not verified_waiting_event(event, hooks):
                raise RuntimeError("Foreign/custom content in PIRA waiting block; inspect before retirement")
            remove_owned("waiting", parts)
            removed_events.add(index)
    if removed_events:
        remaining = [event for i, event in enumerate(expected["hooks"]["PermissionRequest"]) if i not in removed_events]
        if remaining:
            expected["hooks"]["PermissionRequest"] = remaining
        else:
            del expected["hooks"]["PermissionRequest"]
            if not expected["hooks"] and not explicit_hooks:
                del expected["hooks"]
    if len(covered) != len(spans):
        raise RuntimeError("Foreign/custom content in PIRA audio block; inspect before retirement")
    cleaned = text
    for begin, end in sorted(cuts, reverse=True):
        cleaned = cleaned[:begin] + cleaned[end:]
    if tomllib.loads(cleaned) != expected:
        raise RuntimeError("PIRA audio entries share TOML scope with unrelated settings; inspect before retirement")
    # Never remove an unmarked notify/hook by filename, nor retire its helper.
    if command_matches(expected.get("notify"), hooks):
        raise RuntimeError("Unmarked PIRA audio notify; inspect before retirement")
    hooks_table = expected.get("hooks", {})
    events = hooks_table.get("PermissionRequest", []) if isinstance(hooks_table, dict) else []
    if not isinstance(events, list):
        events = []
    for event in events:
        commands = event.get("hooks", []) if isinstance(event, dict) else []
        if not isinstance(commands, list):
            continue
        for hook in commands:
            if not isinstance(hook, dict):
                continue
            if command_matches(hook.get("command"), hooks, waiting=True):
                raise RuntimeError("Unmarked PIRA waiting hook; inspect before retirement")
    return cleaned


def helper_fingerprint(text: str) -> str:
    text = text.replace("\r\n", "\n")
    def bash(match: re.Match) -> str:
        value = match[2]
        try:
            words = shlex.split(value)
        except ValueError:
            return match[0]
        if len(words) != 1 or shlex.quote(words[0]) != value:
            return match[0]  # custom shell expressions are not installer substitutions
        return match[1] + "=<PIRA>"
    text = re.sub(r"(?m)^(PLAYER_CMD|FINISHED_AUDIO|WAITING_AUDIO)=(.*)$", bash, text)
    text = re.sub(r"(?m)^(\$(?:FinishedAudio|WaitingAudio)) = '(?:[^']|'')*'$", r"\1 = <PIRA>", text)
    text = re.sub(r"Start-Process -FilePath '(?:[^']|'')*' -ArgumentList", "Start-Process -FilePath <PIRA> -ArgumentList", text)
    return hashlib.sha256(text.rstrip("\n").encode()).hexdigest()


@dataclass
class Edit:
    path: Path
    before: bytes
    after: bytes | None  # None retires a known shipped helper after backup
    identity: tuple[int, int, int, int]
    origin: Path  # Bind any logical alias to its planned physical target.


def read_safe(path: Path) -> tuple[bytes, tuple[int, int, int, int]] | None:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or getattr(info, "st_file_attributes", 0) & 0x400:
        raise RuntimeError(f"Unsafe audio retirement file (alias/special): {path}")
    if info.st_size > 2 * 1024 * 1024:
        raise RuntimeError(f"Oversized audio retirement file: {path}")
    return path.read_bytes(), (info.st_dev, info.st_ino, info.st_mode, info.st_nlink)


def profile_encoding(data: bytes) -> tuple[str, bytes, str]:
    for prefix, codec in ((b"\xff\xfe", "utf-16-le"), (b"\xfe\xff", "utf-16-be"), (b"\xef\xbb\xbf", "utf-8")):
        if data.startswith(prefix):
            return data[len(prefix):].decode(codec), prefix, codec
    return data.decode("utf-8"), b"", "utf-8"


def plan_retirement(config: Path, profiles: list[Path]) -> list[Edit]:
    edits = []
    hooks = config.parent / "hooks"
    source = read_safe(config.resolve())
    parsed = {}
    remaining_profiles = []
    if source:
        data, identity = source
        cleaned = clean_config(data.decode("utf-8"), hooks).encode("utf-8")
        parsed = tomllib.loads(cleaned.decode("utf-8"))
        if cleaned != data:
            edits.append(Edit(config.resolve(), data, cleaned, identity, config))
    for profile in dict.fromkeys(profiles):
        source = read_safe(profile.resolve())
        if source:
            data, identity = source
            text, prefix, codec = profile_encoding(data)
            for start, end in STARTUP_MARKERS:
                spans = managed_spans(text, start, end)
                if len(spans) > 1:
                    raise RuntimeError(f"Duplicate PIRA startup blocks: {profile}")
                for begin, finish in reversed(spans):
                    text = text[:begin] + text[finish:]
            remaining_profiles.append(text)
            after = prefix + text.encode(codec)
            if after != data:
                edits.append(Edit(profile.resolve(), data, after, identity, profile))
    # A user-owned hook can reference a shared PIRA helper: do not break that hook.
    references = repr(parsed) + "\n" + "\n".join(remaining_profiles)
    for name, fingerprint in HELPERS.items():
        path = hooks / name
        source = read_safe(path.resolve())
        if source:
            data, identity = source
            if name in references:
                print(f"PRESERVE: helper referenced by unrelated configuration: {path}")
            elif helper_fingerprint(data.decode("utf-8", errors="replace")) == fingerprint:
                edits.append(Edit(path.resolve(), data, None, identity, path))
            else:
                print(f"PRESERVE: custom/unrecognized helper: {path}")
    return edits


def apply_retirement(edits: list[Edit], *, dry_run: bool = False, verify: bool = False,
                     backup_dir: Path | None = None) -> None:
    if verify and edits:
        raise RuntimeError("PIRA audio retirement pending: " + ", ".join(str(e.path) for e in edits))
    # All conflicts checked before any publication. Check again per edit as well.
    for edit in edits:
        if edit.origin.resolve() != edit.path or read_safe(edit.path) != (edit.before, edit.identity):
            raise RuntimeError(f"Audio retirement input changed: {edit.path}")
    if edits and backup_dir is not None and not dry_run:
        backup_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    for edit in edits:
        if dry_run:
            print(f"DRY-RUN: retire PIRA audio from {edit.path}")
            continue
        if edit.origin.resolve() != edit.path or read_safe(edit.path) != (edit.before, edit.identity):
            raise RuntimeError(f"Audio retirement input changed: {edit.path}")
        # Unique, exclusive backup; no overwrite or hardlink.
        fd, name = tempfile.mkstemp(prefix=edit.path.name + ".pira-audio.bak.",
                                    dir=backup_dir if backup_dir is not None else edit.path.parent)
        backup = Path(name)
        with os.fdopen(fd, "wb") as handle:
            handle.write(edit.before)
            handle.flush()
            os.fsync(handle.fileno())
        shutil.copystat(edit.path, backup)
        if edit.after is None:
            edit.path.unlink()
        else:
            fd, name = tempfile.mkstemp(prefix=".pira-audio-", dir=edit.path.parent)
            temporary = Path(name)
            try:
                with os.fdopen(fd, "wb") as handle:
                    handle.write(edit.after)
                    handle.flush()
                    os.fsync(handle.fileno())
                temporary.chmod(stat.S_IMODE(edit.identity[2]))
                os.replace(temporary, edit.path)
            finally:
                temporary.unlink(missing_ok=True)
        print(f"RETIRED: {edit.path}; backup: {backup}")


def default_profiles() -> list[Path]:
    if os.name != "nt":
        return [Path(os.environ.get("ZDOTDIR", str(Path.home()))) / ".zshrc"]
    # Match historical installers, including redirected Windows Documents.
    completed = subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                                "[Environment]::GetFolderPath('MyDocuments')"],
                               check=True, capture_output=True, text=True, timeout=10)
    value = completed.stdout.strip()
    if not value:
        raise RuntimeError("Cannot locate Windows Documents; pass explicit --profile paths")
    docs = Path(value)
    return [docs / shell / "Microsoft.PowerShell_profile.ps1" for shell in ("PowerShell", "WindowsPowerShell")]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default=str(Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "config.toml"))
    parser.add_argument("--profile", action="append", help="Exact shell profile; repeatable, replaces platform defaults.")
    parser.add_argument("--backup-dir", help="Rollback directory; defaults to beside each changed file.")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--dry-run", action="store_true")
    mode.add_argument("--verify", action="store_true")
    args = parser.parse_args(argv)
    try:
        profiles = [Path(p).expanduser().absolute() for p in args.profile] if args.profile else default_profiles()
        edits = plan_retirement(Path(args.config).expanduser().absolute(), profiles)
        backup_dir = Path(args.backup_dir).expanduser().absolute() if args.backup_dir else None
        apply_retirement(edits, dry_run=args.dry_run, verify=args.verify, backup_dir=backup_dir)
        print("OK: audio retirement " + ("plan checked" if args.dry_run else "verified" if args.verify else "complete"))
        return 0
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
