#!/usr/bin/env python3
"""Install or refresh released PIRA tools in a per-user PATH directory."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from types import ModuleType
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

REPO_ROOT = Path(__file__).resolve().parents[2]
SELECTOR_PATH = REPO_ROOT / "tools" / "select_tool_for_platform.py"
BLOCK_START = "# >>> PIRA tools PATH >>>"
BLOCK_END = "# <<< PIRA tools PATH <<<"
RETIRED_TOOLS = {"pira_codenav"}
RELEASE_REPOSITORY = "AlgebraLoveme/PIRA"
RELEASE_INDEX_NAME = "pira-tools-release.json"
LATEST_RELEASE_BASE = (
    f"https://github.com/{RELEASE_REPOSITORY}/releases/latest/download"
)
RELEASES_API = f"https://api.github.com/repos/{RELEASE_REPOSITORY}/releases"
MAX_INDEX_BYTES = 1024 * 1024
MAX_BINARY_BYTES = 128 * 1024 * 1024
LEGACY_MANAGED_HASHES = {
    "pira_codenav": {
        "c2cba4a149da97ef68a233b7ddb37b70e13efb097f14d1045b48320645cb52dc",
        "c97cf837ba97ccc71f5fd64ed3415227473847aef8957ddeca0663848837558a",
        "cf11c7f6f9c0d8213d3391da875866ea6a493812c20a145f749191ed68da4eca",
        "d5846ff161b53072fc7303b5a024e4d3a72a732e5635db24adfb0528fded6516",
        "5b839a231b41e87b9186c834316ad26804ad16ab3141f54b284edeb7644f6b7d",
    }
}


@dataclass(frozen=True)
class ToolSelection:
    name: str
    version: str
    release_tag: str
    asset: str
    record: dict[str, object]
    destination: Path
    expected_hash: str
    existing_hash: str | None
    action: str


def default_install_dir() -> Path:
    if os.name == "nt":
        root = os.environ.get("LOCALAPPDATA")
        if not root:
            raise RuntimeError("LOCALAPPDATA is unset; pass --install-dir")
        return Path(root) / "PIRA" / "bin"
    return Path.home() / ".local" / "bin"


def load_selector() -> ModuleType:
    spec = importlib.util.spec_from_file_location("pira_tool_selector", SELECTOR_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load selector: {SELECTOR_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def request_bytes(
    url: str, *, limit: int, accept: str = "application/octet-stream"
) -> bytes:
    request = Request(
        url,
        headers={
            "Accept": accept,
            "User-Agent": "PIRA-setup",
        },
    )
    try:
        with urlopen(request, timeout=30) as response:
            length = response.headers.get("Content-Length")
            if length is not None:
                try:
                    recorded_length = int(length)
                except ValueError as error:
                    raise RuntimeError("release asset has an invalid Content-Length") from error
                if recorded_length > limit:
                    raise RuntimeError(f"release asset exceeds the {limit}-byte safety limit")
            data = response.read(limit + 1)
    except (HTTPError, URLError, TimeoutError) as error:
        raise RuntimeError(f"cannot download {url}: {error}") from error
    if len(data) > limit:
        raise RuntimeError(f"release asset exceeds the {limit}-byte safety limit")
    return data


def release_index(tag: str | None = None) -> dict[str, object]:
    requested_tag = tag
    if requested_tag is not None and not re.fullmatch(
        r"pira-tools-[A-Za-z0-9_.-]+", requested_tag
    ):
        raise RuntimeError(f"invalid PIRA tools release tag: {requested_tag}")
    base = (
        f"https://github.com/{RELEASE_REPOSITORY}/releases/download/"
        f"{quote(requested_tag, safe='')}"
        if requested_tag
        else LATEST_RELEASE_BASE
    )
    url = f"{base}/{RELEASE_INDEX_NAME}"
    try:
        index = json.loads(request_bytes(url, limit=MAX_INDEX_BYTES))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"invalid PIRA release index: {error}") from error
    if (
        not isinstance(index, dict)
        or index.get("schema_version") != 1
        or index.get("repository") != RELEASE_REPOSITORY
        or not isinstance(index.get("tools"), dict)
    ):
        raise RuntimeError("unsupported PIRA release index")
    index_tag = index.get("tag")
    source_sha = index.get("source_sha")
    if not isinstance(index_tag, str) or not re.fullmatch(
        r"pira-tools-[A-Za-z0-9_.-]+", index_tag
    ):
        raise RuntimeError("invalid tag in PIRA release index")
    if requested_tag is not None and index_tag != requested_tag:
        raise RuntimeError(
            f"release index tag mismatch: expected {requested_tag}, found {index_tag}"
        )
    if not isinstance(source_sha, str) or not re.fullmatch(
        r"[0-9a-fA-F]{40,64}", source_sha
    ):
        raise RuntimeError("invalid source commit in PIRA release index")
    return index


def parse_versions(values: list[str] | None) -> dict[str, str]:
    aliases = {"ctx": "pira_ctx", "dec": "pira_dec", "nav": "pira_nav"}
    versions: dict[str, str] = {}
    for value in values or []:
        if "=" not in value:
            raise RuntimeError(f"invalid tool version {value!r}; expected TOOL=VERSION")
        name, version = value.split("=", 1)
        name = aliases.get(name, name)
        if name not in aliases.values():
            raise RuntimeError(f"unknown versioned tool: {name}")
        if not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]*", version):
            raise RuntimeError(f"invalid version for {name}: {version}")
        if name in versions and versions[name] != version:
            raise RuntimeError(f"conflicting versions requested for {name}")
        versions[name] = version
    return versions


def release_tags_for_versions(
    versions: dict[str, str], platform_key: str
) -> dict[str, str]:
    if not versions:
        return {}
    suffix = ".exe" if platform_key.startswith("windows-") else ""
    expected_assets = {
        tool_name: f"{tool_name}-{version}-{platform_key}{suffix}"
        for tool_name, version in versions.items()
    }
    found: dict[str, str] = {}
    for page in range(1, 11):
        url = f"{RELEASES_API}?per_page=100&page={page}"
        try:
            releases = json.loads(
                request_bytes(
                    url,
                    limit=4 * 1024 * 1024,
                    accept="application/vnd.github+json",
                )
            )
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise RuntimeError(f"invalid GitHub releases response: {error}") from error
        if not isinstance(releases, list):
            raise RuntimeError("invalid GitHub releases response")
        for release in releases:
            if not isinstance(release, dict) or release.get("draft") is True:
                continue
            tag = release.get("tag_name")
            assets = release.get("assets")
            if not (
                isinstance(tag, str)
                and re.fullmatch(r"pira-tools-[A-Za-z0-9_.-]+", tag)
                and isinstance(assets, list)
            ):
                continue
            names = {
                asset.get("name")
                for asset in assets
                if isinstance(asset, dict) and isinstance(asset.get("name"), str)
            }
            for tool_name, expected_asset in expected_assets.items():
                if tool_name not in found and expected_asset in names:
                    found[tool_name] = tag
            if len(found) == len(expected_assets):
                return found
        if len(releases) < 100:
            break
    missing = [
        f"{tool_name}={versions[tool_name]}"
        for tool_name in versions
        if tool_name not in found
    ]
    raise RuntimeError(
        f"no cloud build is available for {', '.join(missing)} on {platform_key}; "
        "exact-version history starts with the GitHub Release build system"
    )


def release_asset_url(tag: str, asset: str) -> str:
    return (
        f"https://github.com/{RELEASE_REPOSITORY}/releases/download/"
        f"{quote(tag, safe='')}/{quote(asset, safe='')}"
    )


def download_binary(tag: str, selection: ToolSelection, directory: Path) -> Path:
    data = request_bytes(
        release_asset_url(tag, selection.asset), limit=MAX_BINARY_BYTES
    )
    if len(data) != selection.record["size"]:
        raise RuntimeError(
            f"downloaded {selection.name} size mismatch: "
            f"expected {selection.record['size']}, got {len(data)}"
        )
    actual = hashlib.sha256(data).hexdigest()
    if actual != selection.expected_hash:
        raise RuntimeError(
            f"downloaded {selection.name} checksum mismatch: "
            f"expected {selection.expected_hash}, got {actual}"
        )
    path = directory / selection.asset
    path.write_bytes(data)
    if os.name != "nt":
        path.chmod(0o755)
    return path


def executable_path(directory: Path, tool_name: str) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return directory / f"{tool_name}{suffix}"


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
    home = Path.home()
    try:
        relative = directory.relative_to(home)
        value = f'$HOME/{relative.as_posix()}'
    except ValueError:
        value = "'" + str(directory).replace("'", "'\\''") + "'"
    return f'case ":$PATH:" in *":{value}:"*) ;; *) export PATH="{value}:$PATH" ;; esac'


def update_managed_block(path: Path, body: str, dry_run: bool) -> bool:
    old = path.read_text(encoding="utf-8") if path.exists() else ""
    block = f"{BLOCK_START}\n{body}\n{BLOCK_END}"
    if BLOCK_START in old:
        start = old.index(BLOCK_START)
        end_marker = old.find(BLOCK_END, start)
        if end_marker < 0:
            raise RuntimeError(f"incomplete PIRA PATH block in {path}")
        end = end_marker + len(BLOCK_END)
        prefix = old[:start].rstrip()
        suffix = old[end:].strip("\n")
        new = (prefix + "\n\n" if prefix else "") + block
        new += "\n\n" + suffix + "\n" if suffix else "\n"
    else:
        new = old.rstrip() + ("\n\n" if old.strip() else "") + block + "\n"
    if new == old:
        return False
    if dry_run:
        print(f"DRY-RUN: would update PATH block in {path}")
        return True
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        stamp = datetime.now().strftime("%Y%m%d%H%M%S%f")
        shutil.copy2(path, path.with_name(f"{path.name}.bak.{stamp}"))
    temporary = path.with_name(f".{path.name}.pira-tmp-{os.getpid()}")
    temporary.write_text(new, encoding="utf-8")
    os.replace(temporary, path)
    print(f"Updated PATH in {path}")
    return True


def windows_user_path(directory: Path, dry_run: bool) -> bool:
    import winreg

    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, r"Environment") as key:
        try:
            current, kind = winreg.QueryValueEx(key, "Path")
        except FileNotFoundError:
            current, kind = "", winreg.REG_EXPAND_SZ
        parts = [part for part in current.split(";") if part]
        normalized = os.path.normcase(str(directory.resolve()))
        if any(os.path.normcase(os.path.expandvars(part)) == normalized for part in parts):
            return False
        updated = ";".join([str(directory), *parts])
        if dry_run:
            print(f"DRY-RUN: would prepend {directory} to the user PATH")
            return True
        winreg.SetValueEx(key, "Path", 0, kind, updated)
    try:
        ctypes.windll.user32.SendMessageTimeoutW(
            0xFFFF, 0x001A, 0, "Environment", 0x0002, 5000, None
        )
    except Exception:
        pass
    print(f"Updated Windows user PATH with {directory}")
    return True


def ensure_path(directory: Path, dry_run: bool) -> bool:
    if os.name == "nt":
        return windows_user_path(directory, dry_run)
    changed = False
    for path in shell_profiles():
        changed = update_managed_block(path, shell_path_line(directory), dry_run) or changed
    return changed


def path_is_configured(directory: Path) -> bool:
    active = {
        Path(value).expanduser().resolve(strict=False)
        for value in os.environ.get("PATH", "").split(os.pathsep)
        if value
    }
    if directory in active:
        return True
    if os.name == "nt":
        import winreg
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Environment") as key:
                current, _ = winreg.QueryValueEx(key, "Path")
        except (FileNotFoundError, OSError):
            return False
        return any(
            Path(os.path.expandvars(value)).resolve(strict=False) == directory
            for value in current.split(";") if value
        )
    profiles = shell_profiles()
    return all(
        path.exists()
        and BLOCK_START in path.read_text(encoding="utf-8")
        and shell_path_line(directory) in path.read_text(encoding="utf-8")
        for path in profiles
    )


def direct_version(binary: Path) -> str:
    result = subprocess.run(
        [str(binary), "--version"],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return result.stdout.strip()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Install or refresh cloud-built PIRA tools for this user."
    )
    parser.add_argument("--install-dir", type=Path, default=None, help="Per-user PATH directory.")
    parser.add_argument("--dry-run", action="store_true", help="Describe changes without writing.")
    parser.add_argument(
        "--verify", action="store_true", help="Verify installed tools without changing them."
    )
    parser.add_argument(
        "--no-path", action="store_true", help="Do not persist the install directory in PATH."
    )
    parser.add_argument(
        "--force", action="store_true", help="Refresh even when installed hashes already match."
    )
    parser.add_argument(
        "--tool",
        action="append",
        dest="tools",
        help="Install or verify only this released tool; repeatable. Default: all tools.",
    )
    parser.add_argument(
        "--version",
        action="append",
        help=(
            "Pin one tool as ctx=VERSION, dec=VERSION, or nav=VERSION; repeatable. "
            "Unspecified tools use latest. Exact history begins with cloud releases."
        ),
    )
    return parser


def selected_tools(index: dict[str, object], requested: list[str] | None) -> list[str]:
    released = sorted(
        tool
        for tool in index["tools"]
        if isinstance(tool, str) and tool not in RETIRED_TOOLS
    )
    if not released:
        raise RuntimeError("no PIRA tools were found in the latest release")
    if requested is None:
        return released
    tools = sorted(set(requested))
    missing = [name for name in tools if name not in released]
    if missing:
        raise RuntimeError(
            f"requested tool is not released: {', '.join(missing)}; "
            f"available: {', '.join(released)}"
        )
    return tools


def remove_managed_legacy_tools(install_dir: Path, dry_run: bool) -> None:
    for tool_name, known_hashes in LEGACY_MANAGED_HASHES.items():
        path = executable_path(install_dir, tool_name)
        if not path.is_file() or path.is_symlink() or sha256(path) not in known_hashes:
            continue
        if dry_run:
            print(f"DRY-RUN: would remove retired managed tool {path}")
        else:
            path.unlink()
            print(f"Removed retired managed tool: {path}")


def version_matches(tool_name: str, expected: str, version: str) -> bool:
    return version == f"{tool_name} {expected}"


def tool_selection(
    index: dict[str, object],
    tool_name: str,
    platform_key: str,
    install_dir: Path,
) -> ToolSelection:
    tool = index["tools"].get(tool_name)
    if not isinstance(tool, dict):
        raise RuntimeError(f"invalid release record for {tool_name}")
    version = tool.get("version")
    binaries = tool.get("binaries")
    if not isinstance(version, str) or not isinstance(binaries, dict):
        raise RuntimeError(f"incomplete release record for {tool_name}")
    record = binaries.get(platform_key)
    if not isinstance(record, dict):
        supported = ", ".join(sorted(str(key) for key in binaries))
        raise RuntimeError(
            f"unsupported platform {platform_key}; supported: {supported}"
        )
    asset = record.get("asset")
    expected = record.get("sha256")
    size = record.get("size")
    suffix = ".exe" if platform_key.startswith("windows-") else ""
    expected_asset = f"{tool_name}-{version}-{platform_key}{suffix}"
    if asset != expected_asset:
        raise RuntimeError(f"invalid release asset name for {tool_name}: {asset}")
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", expected):
        raise RuntimeError(f"invalid release checksum for {tool_name}")
    if not isinstance(size, int) or size <= 0 or size > MAX_BINARY_BYTES:
        raise RuntimeError(f"invalid release size for {tool_name}")
    destination = executable_path(install_dir, tool_name)
    existing_hash = (
        sha256(destination)
        if destination.is_file() and not destination.is_symlink()
        else None
    )
    action = (
        "unchanged"
        if existing_hash == expected.lower()
        else ("refresh" if destination.exists() or destination.is_symlink() else "install")
    )
    return ToolSelection(
        name=tool_name,
        version=version,
        release_tag=str(index["tag"]),
        asset=asset,
        record=record,
        destination=destination,
        expected_hash=expected.lower(),
        existing_hash=existing_hash,
        action=action,
    )


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    selector = load_selector()
    install_dir = (args.install_dir or default_install_dir()).expanduser().resolve(strict=False)
    index = release_index()
    platform_key = selector.current_platform()
    tools = selected_tools(index, args.tools)
    requested_versions = parse_versions(args.version)
    unselected_versions = sorted(set(requested_versions) - set(tools))
    if unselected_versions:
        raise RuntimeError(
            "version specified for tool excluded by --tool: "
            + ", ".join(unselected_versions)
        )
    indexes = {tool_name: index for tool_name in index["tools"]}
    historical_versions: dict[str, str] = {}
    for tool_name, version in requested_versions.items():
        latest_tool = index["tools"].get(tool_name)
        if isinstance(latest_tool, dict) and latest_tool.get("version") == version:
            continue
        historical_versions[tool_name] = version
    historical_tags = release_tags_for_versions(historical_versions, platform_key)
    tagged_indexes: dict[str, dict[str, object]] = {}
    for tool_name, version in historical_versions.items():
        tag = historical_tags[tool_name]
        if tag not in tagged_indexes:
            tagged_indexes[tag] = release_index(tag)
        exact_index = tagged_indexes[tag]
        exact_tool = exact_index["tools"].get(tool_name)
        if not isinstance(exact_tool, dict) or exact_tool.get("version") != version:
            raise RuntimeError(
                f"release {tag} does not contain requested {tool_name}={version}"
            )
        indexes[tool_name] = exact_index
    selections = [
        tool_selection(indexes[tool_name], tool_name, platform_key, install_dir)
        for tool_name in tools
    ]

    print(f"Latest release: {index['tag']}")
    print(f"Platform: {platform_key}")

    if args.verify:
        failures: list[str] = []
        for selection in selections:
            if selection.existing_hash != selection.expected_hash:
                failures.append(f"{selection.name}: installed binary is missing or stale")
                continue
            version = direct_version(selection.destination)
            if not version_matches(selection.name, selection.version, version):
                failures.append(f"{selection.name}: unexpected version: {version}")
        if not args.no_path and not path_is_configured(install_dir):
            failures.append("install directory is not configured in the user PATH")
        if failures:
            for failure in failures:
                print(f"FAIL: {failure}", file=sys.stderr)
            return 1
        for selection in selections:
            print(f"OK: {direct_version(selection.destination)}; SHA-256 verified")
        return 0

    with tempfile.TemporaryDirectory(prefix="pira-tools-download-") as temporary:
        download_dir = Path(temporary)
        for selection in selections:
            print(f"\nTool:     {selection.name} {selection.version}")
            print(f"Release:  {selection.release_tag}")
            print(f"Asset:    {selection.asset}")
            print(f"Target:   {selection.destination}")
            if selection.action == "unchanged" and not args.force:
                print("OK: installed tool already matches the selected release")
            elif args.dry_run:
                print(f"DRY-RUN: would download and {selection.action} {selection.destination}")
            else:
                source = download_binary(selection.release_tag, selection, download_dir)
                source_version = direct_version(source)
                if not version_matches(selection.name, selection.version, source_version):
                    raise RuntimeError(
                        f"unexpected downloaded version for {selection.name}: {source_version}"
                    )
                installed = selector.install_binary(
                    source,
                    selection.record,
                    install_dir,
                    tool_name=selection.name,
                )
                actual = sha256(installed)
                if actual != selection.expected_hash:
                    raise RuntimeError(
                        f"installed {selection.name} hash does not match release index"
                    )
                completed_action = {
                    "install": "Installed",
                    "refresh": "Refreshed",
                    "unchanged": "Refreshed",
                }[selection.action]
                print(f"{completed_action}: {installed}")

    if any(selection.name == "pira_nav" for selection in selections):
        remove_managed_legacy_tools(install_dir, args.dry_run)

    if not args.no_path:
        ensure_path(install_dir, args.dry_run)

    if not args.dry_run:
        restart_needed = False
        for selection in selections:
            version = direct_version(selection.destination)
            if not version_matches(selection.name, selection.version, version):
                raise RuntimeError(
                    f"unexpected installed version for {selection.name}: {version}"
                )
            print(f"Verified: {version}; SHA-256 {selection.expected_hash}")
            resolved = shutil.which(selection.name)
            if not resolved or Path(resolved).resolve() != selection.destination.resolve():
                restart_needed = True
        if restart_needed:
            print("NOTE: restart the shell or agent process to activate the updated tools in PATH")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"setup_pira_tools.py: {error}", file=sys.stderr)
        raise SystemExit(1)
