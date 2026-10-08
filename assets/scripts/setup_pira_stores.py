"""Shared, setup-only physical store selection and data-preserving configuration migration."""
from __future__ import annotations

import copy
import ctypes
import json
import os
import re
import shlex
import shutil
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType

import migrate_pira_stores as migration

BLOCK_START = "# >>> PIRA tools PATH >>>"
BLOCK_END = "# <<< PIRA tools PATH <<<"
STORE_BLOCK_START = "# >>> PIRA store paths >>>"
STORE_BLOCK_END = "# <<< PIRA store paths <<<"
STORE_ENV_KEYS = {"pira_ctx": "PIRA_CTX_STORE_DIR", "pira_dec": "PIRA_DEC_STORE_DIR",
                  "pira_team": "PIRA_TEAM_DIR"}


@dataclass
class StorePlan:
    """Validated store choices, read-only relocation plans and pending configuration updates."""
    stores: dict[str, str] = field(default_factory=dict)
    profiles: dict[Path, tuple[str, str]] = field(default_factory=dict)
    registry: dict[str, str] = field(default_factory=dict)
    notices: list[str] = field(default_factory=list)
    migrations: list[migration.Migration] = field(default_factory=list)
    team_migrations: list[migration.TeamMigration] = field(default_factory=list)


def physical_store_path(value: str | Path) -> Path:
    """Resolve directory aliases without creating stores or touching their records."""
    if not str(value):
        raise RuntimeError("Store path is empty; choose an explicit directory")
    # Do not expand '~' or environment variables: runtime overrides are literal paths.
    path = Path(value)
    if not path.is_absolute():
        path = Path.cwd() / path
    missing = False
    try:
        resolved = Path(path.anchor).resolve(strict=True)
        for part in path.parts[1:]:
            if part == "..":
                if missing:
                    raise RuntimeError("cannot resolve '..' after a nonexistent directory")
                resolved = resolved.parent
                continue
            candidate = resolved / part
            try:
                candidate.lstat()
            except FileNotFoundError:
                resolved = candidate
                missing = True
                continue
            # strict=True catches dangling links and loops rather than treating them
            # as a not-yet-created suffix. Resolve BEFORE processing the next '..'.
            resolved = candidate.resolve(strict=True)
            if not resolved.is_dir():
                raise RuntimeError(f"store ancestor is not a directory: {candidate}")
        return resolved
    except (OSError, RuntimeError, ValueError) as error:
        raise RuntimeError(f"Cannot resolve store path {value!s}: {error}") from error


def selected_store_paths(tools: list[str], overrides: dict[str, str] | None = None, *, use_environment: bool = True) -> dict[str, str]:
    """Select physical overrides or the approved persistent platform data layout."""
    stores: dict[str, str] = {}
    for tool, leaf in (("pira_ctx", "ctx"), ("pira_dec", "decision"), ("pira_team", "team")):
        key = STORE_ENV_KEYS[tool]
        if tool not in tools:
            continue
        if overrides is not None and key in overrides:
            value = overrides[key]
        elif use_environment and key in os.environ:
            value = os.environ[key]
        elif sys.platform == "win32" and os.environ.get("LOCALAPPDATA"):
            value = Path(os.environ["LOCALAPPDATA"]) / "PIRA" / leaf
        elif sys.platform == "darwin" and os.environ.get("HOME"):
            value = Path(os.environ["HOME"]) / "Library" / "Application Support" / "PIRA" / leaf
        elif sys.platform != "win32" and sys.platform != "darwin" and Path(os.environ.get("XDG_DATA_HOME", "")).is_absolute():
            value = Path(os.environ["XDG_DATA_HOME"]) / "pira" / leaf
        elif sys.platform != "win32" and sys.platform != "darwin" and os.environ.get("HOME"):
            value = Path(os.environ["HOME"]) / ".local" / "share" / "pira" / leaf
        else:
            raise RuntimeError(f"Cannot determine {tool} store; set {key}")
        stores[key] = str(physical_store_path(value))
    return stores


def historical_store_paths(tool: str) -> list[Path]:
    """Recognized shipped default roots only; never infer arbitrary custom roots."""
    paths: list[Path] = []
    if tool == "pira_team":
        paths.append(Path(tempfile.gettempdir()) / (f"pira-team-{os.geteuid()}" if hasattr(os, "geteuid") else "pira-team"))
    if tool == "pira_ctx" and sys.platform == "darwin" and os.environ.get("HOME"):
        paths.append(Path(os.environ["HOME"]) / "Library" / "Caches" / "PIRA" / "ctx")
    elif tool == "pira_ctx" and sys.platform not in ("darwin", "win32"):
        # Historical Ctx accepted relative/empty XDG_CACHE_HOME literally.
        if "XDG_CACHE_HOME" in os.environ:
            paths.append(Path(os.environ["XDG_CACHE_HOME"]) / "pira" / "ctx")
        if os.environ.get("HOME"):
            paths.append(Path(os.environ["HOME"]) / ".cache" / "pira" / "ctx")
    if tool == "pira_dec" and sys.platform not in ("darwin", "win32"):
        # Earlier Dec used even relative XDG_DATA_HOME; current defaults do not.
        if "XDG_DATA_HOME" in os.environ and not Path(os.environ["XDG_DATA_HOME"]).is_absolute():
            paths.append(Path(os.environ["XDG_DATA_HOME"]) / "pira" / "decision")
    return sorted({physical_store_path(path) for path in paths})


def automatic_store_choice(tool: str, value: str) -> str:
    """Map recognized historical default selections, not genuinely custom roots."""
    selected = physical_store_path(value)
    if selected in historical_store_paths(tool):
        return selected_store_paths([tool], use_environment=False)[STORE_ENV_KEYS[tool]]
    return str(selected)


def configuration_toml() -> ModuleType:
    """Load the standard-library parser before planning any configuration writes."""
    if sys.version_info < (3, 11):
        raise RuntimeError("PIRA configuration requires Python 3.11+; rerun setup with python3.11 or newer")
    import tomllib
    return tomllib


def parse_configuration(text: str) -> dict:
    parser = configuration_toml()
    try:
        return parser.loads(text)
    except parser.TOMLDecodeError as error:
        raise RuntimeError(f"Cannot safely edit Codex TOML: {error}") from error


def set_toml_string(text: str, keys: tuple[str, ...], value: str) -> str:
    """Accept a narrow text edit only if parsing proves exactly the intended change."""
    original = parse_configuration(text)
    expected = copy.deepcopy(original)
    table = expected
    for key in keys[:-1]:
        table = table.setdefault(key, {})
        if not isinstance(table, dict):
            raise RuntimeError(f"Codex {'.'.join(keys)} requires a table")
    old = table.get(keys[-1])
    if old == value:
        return text
    if old is not None and not isinstance(old, str):
        raise RuntimeError(f"Codex {'.'.join(keys)} must be a string")
    table[keys[-1]] = value
    encoded = json.dumps(value, ensure_ascii=False)
    parser = configuration_toml()

    def candidates():
        if old is not None:
            # PIRA: includes quoted/dotted keys and inline tables. Unusual/multiline
            # layouts may fail closed; no handwritten parser decides semantics.
            for match in re.finditer(r'''"(?:\\.|[^"\\\r\n])*"|'[^'\r\n]*' '''.strip(), text):
                try:
                    if parser.loads("v = " + match.group())["v"] == old:
                        yield text[:match.start()] + encoded + text[match.end():]
                except parser.TOMLDecodeError:
                    continue
        else:
            names = [".".join(json.dumps(k) for k in keys[i:]) for i in range(len(keys))]
            yield f"{names[0]} = {encoded}\n" + text
            for match in re.finditer(r"(?m)^\s*\[[^\r\n]+\][^\r\n]*(?:\n|$)", text):
                prefix = text[:match.end()]
                for name in names:
                    yield prefix + ("" if prefix.endswith("\n") else "\n") + f"{name} = {encoded}\n" + text[match.end():]
            for match in re.finditer(r"\{", text):
                for name in names:
                    for separator in (", ", ""):
                        yield text[:match.end()] + f"{name} = {encoded}{separator}" + text[match.end():]

    for candidate in candidates():
        try:
            if parser.loads(candidate) == expected:
                return candidate
        except parser.TOMLDecodeError:
            continue
    raise RuntimeError(f"Cannot safely edit Codex {'.'.join(keys)}; use ordinary single-line strings/table syntax and rerun setup")


def codex_store_configuration(text: str, tools: list[str], defaults: dict[str, str] | None = None) -> str:
    """Preserve Codex's own overrides, including profile-specific store identities."""
    parsed = parse_configuration(text)
    names = STORE_ENV_KEYS
    keys = [names[tool] for tool in tools if tool in names]
    if not keys:
        return text
    scopes = [((), parsed)]
    profiles = parsed.get("profiles", {})
    if not isinstance(profiles, dict):
        raise RuntimeError("Codex profiles must be a table")
    scopes.extend((("profiles", name), profile) for name, profile in profiles.items())
    for scope, settings in scopes:
        if not isinstance(settings, dict):
            raise RuntimeError("Codex profile must be a table")
        policy = settings.get("shell_environment_policy", {})
        if not isinstance(policy, dict) or not isinstance(policy.get("set", {}), dict):
            raise RuntimeError("Codex shell_environment_policy.set must be a table")
        configured = policy.get("set", {})
        overrides = dict(defaults or {})
        for key in keys:
            if key in configured:
                if not isinstance(configured[key], str):
                    raise RuntimeError(f"Codex {key} must be a string")
                overrides[key] = automatic_store_choice(next(tool for tool in tools if names.get(tool) == key), configured[key])
        scope_tools = tools if not scope else [tool for tool in tools if names.get(tool) in configured]
        selected = selected_store_paths(scope_tools, overrides)
        for key in keys:
            if not scope or key in configured:
                text = set_toml_string(text, (*scope, "shell_environment_policy", "set", key), selected[key])
    return text


def shell_store_value(raw: str, source: Path, assigned: set[str] | None = None) -> str:
    """Read static shell assignments without evaluating shell code."""
    if raw == "":
        return ""
    if raw.startswith("="):
        raise RuntimeError(f"Unsupported shell word in {source}; quote leading = to avoid zsh command-path expansion")
    if raw[:1].isspace():
        raise RuntimeError(f"Unsupported shell word in {source}; an assignment value must follow = without whitespace")
    # Read one shell word, then allow a comment only after word-separating
    # whitespace. shlex.split(comments=True) would corrupt paths containing '#'.
    lexer = shlex.shlex(raw, posix=True)
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        value = lexer.get_token()
        position = lexer.instream.tell()
        remainder = lexer.instream.read().lstrip()
    except ValueError as error:
        raise RuntimeError(f"Cannot parse store assignment in {source}: {error}") from error
    if value is None or (remainder and not remainder.startswith("#")):
        raise RuntimeError(f"Use a single quoted store path in {source}")
    raw = raw[:position].rstrip()
    literal = (raw.strip() == shlex.quote(value)
               or re.fullmatch(r"\s*'[^']*'\s*(?:#.*)?", raw) is not None)
    if not literal:
        variable = r"\$(?:\{([A-Za-z_][A-Za-z_0-9]*)\}|([A-Za-z_][A-Za-z_0-9]*))"
        masked = re.sub(variable, "ENV", raw)
        # A hash within the assignment word is literal, not a shell comment.
        plain = masked.replace("#", "HASH")
        double_quoted = re.fullmatch(r'"(?:\\.|[^"\\])*"', raw) is not None
        home_word = (masked == "~" or
                     (masked.startswith("~/") and shlex.quote(masked[2:]) == masked[2:]))
        # Do not confuse shlex word decoding with shell expansion: unquoted
        # braces, secondary tildes and concatenated quote forms can change values.
        if ("`" in raw or ("$" in raw and "\\" in raw)
                or not (double_quoted or home_word or shlex.quote(plain) == plain)):
            raise RuntimeError(f"Unsupported shell word in {source}; use a literal quoted path or a simple environment reference")
        if "$" in re.sub(variable, "", value):
            raise RuntimeError(f"Unsupported store expansion in {source}; use a literal path")
        def expand(match):
            name = match.group(1) or match.group(2)
            if assigned and name in assigned:
                raise RuntimeError(f"Store expansion in {source} references profile-assigned {name}; use a literal store path")
            if name not in os.environ:
                raise RuntimeError(f"Store assignment in {source} references unset {name}; use a literal path")
            return os.environ[name]
        value = re.sub(variable, expand, value)
        if raw.startswith("~"):
            if assigned and "HOME" in assigned:
                raise RuntimeError(f"Store expansion in {source} references profile-assigned HOME; use a literal store path")
            expanded = os.path.expanduser(value)
            if expanded == value:
                raise RuntimeError(f"Cannot expand store home in {source}; use a literal path")
            value = expanded
    return value


def split_store_block(text: str, source: Path) -> tuple[str, dict[str, str]]:
    """Remove only our validated block, preserving all opaque surrounding bytes."""
    if STORE_BLOCK_START not in text and STORE_BLOCK_END not in text:
        return text, {}
    pattern = re.compile(r"(?m)^" + re.escape(STORE_BLOCK_START) + r"\r?\n(.*?)^" + re.escape(STORE_BLOCK_END) + r"(?:\r?\n|$)", re.DOTALL)
    match = pattern.search(text)
    if (match is None or text.count(STORE_BLOCK_START) != 1 or text.count(STORE_BLOCK_END) != 1):
        raise RuntimeError(f"Invalid PIRA store block in {source}; repair its markers before setup")
    stores: dict[str, str] = {}
    for line in match.group(1).splitlines():
        assignment = re.fullmatch(r"export (PIRA_CTX_STORE_DIR|PIRA_DEC_STORE_DIR|PIRA_TEAM_DIR)=(.*)", line)
        if assignment is None or assignment[1] in stores:
            raise RuntimeError(f"Invalid PIRA store block in {source}; only unique store exports are managed")
        try:
            words = shlex.split(assignment[2])
        except ValueError as error:
            raise RuntimeError(f"Invalid PIRA store literal in {source}") from error
        if len(words) != 1 or shlex.quote(words[0]) != assignment[2]:
            raise RuntimeError(f"Nonliteral PIRA store block in {source}; choose literal paths before setup")
        stores[assignment[1]] = words[0]
    return text[:match.start()] + text[match.end():], stores


def profile_store_context(text: str, source: Path) -> tuple[dict[str, list[str]], set[str], bool]:
    """Recognize simple assignments for inference, not to edit or interpret profiles."""
    assignments: dict[str, list[str]] = {}
    opaque = False
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        generated = re.fullmatch(r'.* export PATH=(.+):"\$PATH" ;; esac', line)
        if generated:
            try:
                words = shlex.split(generated[1])
            except ValueError:
                words = []
            if len(words) == 1 and shell_path_line(Path(words[0])) == line:
                assignments.setdefault("PATH", []).append(generated[1])
                continue
        match = re.fullmatch(r"(?:export[ \t]+)?([A-Za-z_][A-Za-z_0-9]*)=(.*)", line)
        if match is None:
            opaque = True
            # PIRA: detect possible default mutations, not shell semantics. An
            # explicit store choice is required for these opaque statements.
            for name in ("HOME", "LOCALAPPDATA", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
                if (re.search(r"(?:^|[\s;])" + name + r"=", line)
                        or (re.search(r"\bunset\b", line)
                            and re.search(r"\b" + name + r"\b", line))):
                    assignments.setdefault(name, [])
            continue
        name, raw = match.groups()
        assignments.setdefault(name, []).append(raw)
        try:
            shell_store_value(raw, source)
        except RuntimeError:
            opaque = True
    return assignments, set(assignments), opaque


def plan_store_environment(
    tools: list[str], *, profile_paths: list[Path] | None = None,
    codex_text: str | None = None, codex_binary: str | Path | None = None,
) -> StorePlan:
    """Preflight selected stores; keep unrelated shell content opaque and unchanged."""
    configuration_toml()
    keys = [STORE_ENV_KEYS[tool] for tool in tools if tool in STORE_ENV_KEYS]
    if not keys:
        return StorePlan()
    notices: list[str] = []
    persisted: dict[str, list[str]] = {key: [] for key in keys}
    ambiguous: set[str] = set()
    profiles: dict[Path, tuple[str, str, dict[str, str]]] = {}
    registry: dict[str, str] = {}
    if sys.platform == "win32":
        import winreg
        import ntpath
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as handle:
                for key in keys:
                    try:
                        value, kind = winreg.QueryValueEx(handle, key)
                    except FileNotFoundError:
                        continue
                    if not isinstance(value, str) or kind not in (winreg.REG_SZ, winreg.REG_EXPAND_SZ):
                        raise RuntimeError(f"Windows user environment {key} must be a string")
                    registry[key] = value
                    persisted[key].append(ntpath.expandvars(value) if kind == winreg.REG_EXPAND_SZ else value)
        except FileNotFoundError:
            pass
    else:
        contexts = {}
        assigned: set[str] = set()
        opaque = False
        for path in (shell_profiles() if profile_paths is None else profile_paths):
            old = read_profile(path)
            managed_block_text(path, old, "")  # Validate existing PATH markers before writes.
            outside, managed = split_store_block(old, path)
            profiles[path] = (old, outside, managed)
            assignments, names, unknown = profile_store_context(outside, path)
            contexts[path] = assignments
            assigned.update(names)
            opaque = opaque or unknown
        for path, (_, outside, managed) in profiles.items():
            for key in keys:
                if key in managed:
                    persisted[key].append(managed[key])
                    continue  # This profile already has an explicit migrated choice.
                visible = any(re.search(r"\b" + key + r"\b", line)
                              for line in outside.splitlines() if not line.lstrip().startswith("#"))
                if not visible:
                    defaults = {"HOME", "LOCALAPPDATA", "XDG_DATA_HOME"}
                    if assigned & defaults:
                        ambiguous.add(key)
                    continue
                values = contexts[path].get(key, [])
                if opaque or not values:
                    ambiguous.add(key)
                    continue
                if not re.search(r"(?m)^export[ \t]+" + key + "=", outside) and key not in os.environ:
                    ambiguous.add(key)
                    continue
                try:
                    persisted[key].extend(shell_store_value(raw, path, assigned) for raw in values)
                except RuntimeError:
                    ambiguous.add(key)
    overrides: dict[str, str] = {}
    for key in keys:
        if key in os.environ:
            overrides[key] = automatic_store_choice(next(tool for tool in tools if STORE_ENV_KEYS.get(tool) == key), os.environ[key])
            if key in ambiguous or any(value != overrides[key] for value in persisted[key]):
                notices.append(f"MIGRATION: explicit {key} selects {overrides[key]}; the managed export selects this path if its block is reached. Records and prior shell code are unchanged; arbitrary shell behavior is not evaluated.")
        else:
            if key in ambiguous:
                raise RuntimeError(f"Ambiguous profile setting for {key}; set {key} explicitly to the intended physical store path and rerun setup (no records will be moved)")
            physical = {automatic_store_choice(next(tool for tool in tools if STORE_ENV_KEYS.get(tool) == key), value) for value in persisted[key]}
            if len(physical) > 1:
                raise RuntimeError(f"Conflicting {key} settings; set {key} explicitly to the intended physical path before setup")
            if physical:
                overrides[key] = physical.pop()
    stores = selected_store_paths(tools, overrides)
    relocations = []
    team_relocations = []
    codex_legacy: set[str] = set()
    if codex_text is not None:
        parsed = parse_configuration(codex_text)
        # Validate all selected scope shapes and choices before any migration.
        codex_store_configuration(codex_text, tools, stores)
        scopes = [parsed, *parsed.get("profiles", {}).values()]
        for settings in scopes:
            configured = settings.get("shell_environment_policy", {}).get("set", {})
            for tool in tools:
                key = STORE_ENV_KEYS.get(tool)
                if key in configured and physical_store_path(configured[key]) in historical_store_paths(tool):
                    codex_legacy.add(tool)
    defaults = {}
    for tool in tools:
        if tool in STORE_ENV_KEYS:
            try:
                defaults.update(selected_store_paths([tool], use_environment=False))
            except RuntimeError:
                # Explicit custom stores need not have a discoverable default.
                pass
    for tool in tools:
        key = STORE_ENV_KEYS.get(tool)
        if key is None:
            continue
        destination = Path(stores[key])
        if tool == "pira_team":
            if stores[key] == defaults.get(key) or tool in codex_legacy:
                destination = Path(defaults[key])
                sources = [path for path in historical_store_paths(tool) if path != destination and path.exists()]
                if sources:
                    team_relocations.extend(migration.preflight_team_relocation(sources, destination, codex_binary=codex_binary))
        elif stores[key] == defaults.get(key) or tool in codex_legacy:
            destination = Path(defaults[key])
            sources = [path for path in historical_store_paths(tool) if path != destination and path.exists()]
            if sources:
                relocations.append(migration.plan_migration(tool, sources, destination))
    if sys.platform != "win32" and any("\n" in value or "\r" in value for value in stores.values()):
        raise RuntimeError("Shell store paths must be single-line; choose a path without newline characters")
    updates: dict[Path, tuple[str, str]] = {}
    for path, (old, outside, managed) in profiles.items():
        managed.update(stores)
        block = STORE_BLOCK_START + "\n" + "".join(
            f"export {key}={shlex.quote(value)}\n" for key, value in sorted(managed.items())
        ) + STORE_BLOCK_END + "\n"
        new = outside + ("\n" if outside and not outside.endswith("\n") else "") + block
        if new != old:
            updates[path] = (old, new)
    registry_updates = {key: value for key, value in stores.items() if registry.get(key) != value} if sys.platform == "win32" else {}
    return StorePlan(stores, updates, registry_updates, notices, relocations, team_relocations)


def apply_store_migrations(plan: StorePlan, *, dry_run: bool, verify: bool = False) -> None:
    """Verified-copy barrier: call BEFORE any Codex/profile/registry switch.

    Unified setup must retain this exact plan through its configuration writes.
    Standalone tools setup gets the barrier via apply_store_environment below.
    """
    migration.apply_migrations(plan.migrations, dry_run=dry_run, verify=verify)
    migration.apply_team_migrations(plan.team_migrations, dry_run=dry_run, verify=verify)


def apply_store_environment(
    plan: StorePlan,
    *, dry_run: bool, verify: bool = False,
) -> None:
    stores, files, registry = plan.stores, plan.profiles, plan.registry
    if verify:
        apply_store_migrations(plan, dry_run=True, verify=True)
        if files or registry:
            raise RuntimeError("Physical store environment is missing or stale; rerun setup without --verify")
        return
    for notice in plan.notices:
        print(notice)
    for path, (old, new) in files.items():
        # Reject concurrent edits instead of overwriting unrelated changes.
        current = read_profile(path)
        if current != old:
            raise RuntimeError(f"Configuration changed during setup: {path}; rerun")
    apply_store_migrations(plan, dry_run=dry_run)
    for path, (old, _) in files.items():
        if read_profile(path) != old:
            raise RuntimeError(f"Configuration changed during migration: {path}; rerun")
    for path, (_, new) in files.items():
        write_profile(path, new, dry_run)
    if registry:
        if dry_run:
            print(r"DRY-RUN: would configure physical store paths in Windows HKCU\Environment")
        else:
            import winreg
            with winreg.CreateKey(winreg.HKEY_CURRENT_USER, "Environment") as handle:
                for key, value in registry.items():
                    winreg.SetValueEx(handle, key, 0, winreg.REG_SZ, value)
            notify_windows_environment()
    if stores:
        print("NOTE: restart shells/agents to activate physical store paths; records are unchanged")


def shell_profiles() -> list[Path]:
    shell = Path(os.environ.get("SHELL", "")).name
    if shell == "zsh" or sys.platform == "darwin":
        return [Path.home() / ".zprofile", Path.home() / ".zshrc"]
    if shell == "bash" and (Path.home() / ".bash_profile").exists():
        return [Path.home() / ".bash_profile", Path.home() / ".bashrc"]
    if shell == "bash":
        return [Path.home() / ".profile", Path.home() / ".bashrc"]
    return [Path.home() / ".profile"]


def shell_path_line(directory: Path) -> str:
    value = shlex.quote(str(directory))
    return f'case ":$PATH:" in *:{value}:*) ;; *) export PATH={value}:"$PATH" ;; esac'


def managed_block_text(path: Path, old: str, body: str) -> str:
    block = f"{BLOCK_START}\n{body}\n{BLOCK_END}"
    if BLOCK_START in old:
        start = old.index(BLOCK_START)
        end_marker = old.find(BLOCK_END, start)
        if end_marker < 0:
            raise RuntimeError(f"incomplete PIRA PATH block in {path}")
        return old[:start] + block + old[end_marker + len(BLOCK_END):]
    # Store exports stay last. Do not trim or reformat unrelated shell content.
    position = old.find(STORE_BLOCK_START)
    if position < 0:
        position = len(old)
    prefix, suffix = old[:position], old[position:]
    separator = "\n" if prefix and not prefix.endswith("\n") else ""
    return prefix + separator + block + "\n" + suffix


def read_profile(path: Path) -> str:
    if path.is_symlink():
        try:
            path = path.resolve(strict=True)
        except (OSError, RuntimeError) as error:
            raise RuntimeError(f"Cannot resolve shell profile {path}: {error}") from error
    if not path.exists():
        return ""
    with path.open("r", encoding="utf-8", newline="") as stream:
        return stream.read()


def write_profile(path: Path, new: str, dry_run: bool) -> None:
    if dry_run:
        print(f"DRY-RUN: would update shell configuration in {path}")
        return
    # Resolve existing profile links rather than replacing the link itself.
    try:
        path = path.resolve(strict=path.is_symlink())
    except (OSError, RuntimeError) as error:
        raise RuntimeError(f"Cannot resolve shell profile {path}: {error}") from error
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.pira-tmp-", dir=path.parent)
    os.close(descriptor)
    temporary = Path(name)
    try:
        if path.exists():
            descriptor, backup = tempfile.mkstemp(prefix=f"{path.name}.bak.", dir=path.parent)
            os.close(descriptor)
            shutil.copy2(path, backup)
        # Stage privately even for read-only originals, then restore supported
        # metadata/mode before replacement. New profiles stay private (0600).
        with temporary.open("w", encoding="utf-8", newline="") as stream:
            stream.write(new)
        if path.exists():
            shutil.copystat(path, temporary)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"Updated shell configuration in {path}")


def notify_windows_environment() -> None:
    try:
        ctypes.windll.user32.SendMessageTimeoutW(
            0xFFFF, 0x001A, 0, "Environment", 0x0002, 5000, None
        )
    except Exception:
        pass
