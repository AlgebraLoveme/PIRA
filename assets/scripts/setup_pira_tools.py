#!/usr/bin/env python3
"""Install or refresh released PIRA tools in a per-user PATH directory."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

import setup_pira_stores as stores

REPO_ROOT = Path(__file__).resolve().parents[2]
SELECTOR_PATH = REPO_ROOT / "tools" / "select_tool_for_platform.py"
RETIRED_TOOLS = {"pira_codenav"}
TEAM_CODEX_MIN_VERSION = (0, 159, 0)
CODEX_RELEASE_API = "https://api.github.com/repos/openai/codex/releases/latest"
MAX_CODEX_BYTES = 512 * 1024 * 1024
CODEX_PACKAGE_DIR = ".pira-codex"
CODEX_TARGETS = {
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
    "linux-x64": "x86_64-unknown-linux-musl",
    "linux-arm64": "aarch64-unknown-linux-musl",
    "windows-x64": "x86_64-pc-windows-msvc",
    "windows-arm64": "aarch64-pc-windows-msvc",
}
CODEX_INSTALL_GUIDE = "https://learn.chatgpt.com/docs/cli"
RELEASE_REPOSITORY = "AlgebraLoveme/PIRA"
RELEASE_INDEX_NAME = "pira-tools-release.json"
LATEST_RELEASE_BASE = (
    f"https://github.com/{RELEASE_REPOSITORY}/releases/latest/download"
)
RELEASES_API = f"https://api.github.com/repos/{RELEASE_REPOSITORY}/releases"
MAX_INDEX_BYTES = 1024 * 1024
MAX_BINARY_BYTES = 128 * 1024 * 1024
DOWNLOAD_CHUNK_BYTES = 64 * 1024
PROGRESS_INTERVAL_SECONDS = 5.0
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
    asset_hash: str
    asset_size: int
    compression: str | None
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


def request_to_path(
    url: str,
    destination: Path,
    *,
    limit: int,
    expected_size: int,
    expected_hash: str,
) -> None:
    if expected_size <= 0 or expected_size > limit:
        raise RuntimeError(f"invalid expected release asset size: {expected_size}")
    request = Request(
        url,
        headers={
            "Accept": "application/octet-stream",
            "User-Agent": "PIRA-setup",
        },
    )
    digest = hashlib.sha256()
    received = 0
    last_progress = time.monotonic()
    created = False
    try:
        with urlopen(request, timeout=30) as response:
            length = response.headers.get("Content-Length")
            if length is not None:
                try:
                    recorded_length = int(length)
                except ValueError as error:
                    raise RuntimeError(
                        "release asset has an invalid Content-Length"
                    ) from error
                if recorded_length > limit:
                    raise RuntimeError(
                        f"release asset exceeds the {limit}-byte safety limit"
                    )
                if recorded_length != expected_size:
                    raise RuntimeError(
                        "release asset Content-Length mismatch: "
                        f"expected {expected_size}, got {recorded_length}"
                    )
            with destination.open("xb") as output:
                created = True
                while chunk := response.read(DOWNLOAD_CHUNK_BYTES):
                    received += len(chunk)
                    if received > limit:
                        raise RuntimeError(
                            f"release asset exceeds the {limit}-byte safety limit"
                        )
                    output.write(chunk)
                    digest.update(chunk)
                    now = time.monotonic()
                    if now - last_progress >= PROGRESS_INTERVAL_SECONDS:
                        percent = 100.0 * received / expected_size
                        print(
                            f"Downloading {destination.name}: "
                            f"{received / (1024 * 1024):.1f}/"
                            f"{expected_size / (1024 * 1024):.1f} MiB "
                            f"({min(percent, 100.0):.0f}%)",
                            file=sys.stderr,
                            flush=True,
                        )
                        last_progress = now
    except (HTTPError, URLError, TimeoutError) as error:
        if created:
            destination.unlink(missing_ok=True)
        raise RuntimeError(f"cannot download {url}: {error}") from error
    except Exception:
        if created:
            destination.unlink(missing_ok=True)
        raise
    if received != expected_size:
        destination.unlink(missing_ok=True)
        raise RuntimeError(
            f"release asset size mismatch: expected {expected_size}, got {received}"
        )
    actual = digest.hexdigest()
    if actual != expected_hash:
        destination.unlink(missing_ok=True)
        raise RuntimeError(
            f"release asset checksum mismatch: expected {expected_hash}, got {actual}"
        )


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
        or index.get("schema_version") not in {1, 2}
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
    aliases = {
        "ctx": "pira_ctx",
        "dec": "pira_dec",
        "nav": "pira_nav",
        "svg": "pira_svg_check",
        "team": "pira_team",
    }
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
        tool_name: {
            f"{tool_name}-{version}-{platform_key}{suffix}",
            f"{tool_name}-{version}-{platform_key}{suffix}.gz",
        }
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
            for tool_name, candidates in expected_assets.items():
                if tool_name not in found and candidates.intersection(names):
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
    asset_path = directory / selection.asset
    request_to_path(
        release_asset_url(tag, selection.asset),
        asset_path,
        limit=MAX_BINARY_BYTES,
        expected_size=selection.asset_size,
        expected_hash=selection.asset_hash,
    )
    path = asset_path
    if selection.compression == "gzip":
        path = directory / selection.asset.removesuffix(".gz")
        digest = hashlib.sha256()
        size = 0
        created = False
        try:
            with gzip.open(asset_path, "rb") as source, path.open("xb") as output:
                created = True
                while chunk := source.read(DOWNLOAD_CHUNK_BYTES):
                    size += len(chunk)
                    if size > selection.record["size"]:
                        raise RuntimeError(
                            f"expanded {selection.name} exceeds its declared size"
                        )
                    output.write(chunk)
                    digest.update(chunk)
        except (OSError, EOFError) as error:
            if created:
                path.unlink(missing_ok=True)
            raise RuntimeError(
                f"cannot decompress release asset {selection.asset}: {error}"
            ) from error
        except Exception:
            if created:
                path.unlink(missing_ok=True)
            raise
        if size != selection.record["size"]:
            path.unlink(missing_ok=True)
            raise RuntimeError(
                f"expanded {selection.name} size mismatch: "
                f"expected {selection.record['size']}, got {size}"
            )
        actual = digest.hexdigest()
        if actual != selection.expected_hash:
            path.unlink(missing_ok=True)
            raise RuntimeError(
                f"expanded {selection.name} checksum mismatch: "
                f"expected {selection.expected_hash}, got {actual}"
            )
    if os.name != "nt":
        path.chmod(0o755)
    return path


def executable_path(directory: Path, tool_name: str) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return directory / f"{tool_name}{suffix}"


def update_managed_block(path: Path, body: str, dry_run: bool) -> bool:
    old = stores.read_profile(path)
    new = stores.managed_block_text(path, old, body)
    if new == old:
        return False
    stores.write_profile(path, new, dry_run)
    return True


def windows_user_path(directory: Path, dry_run: bool, *, append: bool = False) -> bool:
    import winreg

    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
            current, kind = winreg.QueryValueEx(key, "Path")
    except FileNotFoundError:
        current, kind = "", winreg.REG_EXPAND_SZ
    parts = [part for part in current.split(";") if part]
    normalized = os.path.normcase(os.path.realpath(directory))
    # Resolve both sides: Windows short names and directory aliases identify the same entry.
    present = [part for part in parts
               if os.path.normcase(os.path.realpath(os.path.expandvars(part))) == normalized]
    if append:
        # Move only this managed entry behind existing user choices, including on reruns.
        updated = ";".join([*(part for part in parts if part not in present), str(directory)])
        if updated == current:
            return False
    else:
        if present:
            return False
        updated = ";".join([str(directory), *parts])
    if dry_run:
        print(f"DRY-RUN: would {'append' if append else 'prepend'} {directory} to the user PATH")
        return True
    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
        winreg.SetValueEx(key, "Path", 0, kind, updated)
    stores.notify_windows_environment()
    print(f"Updated Windows user PATH with {directory}")
    return True


def ensure_path(directory: Path, dry_run: bool, *, include_codex: bool = False) -> bool:
    codex_bin = directory / CODEX_PACKAGE_DIR / "bin"
    directories = [directory]
    if include_codex or executable_path(codex_bin, "codex").is_file():
        directories.append(codex_bin)
    selected = shutil.which("codex")
    external = selected is not None and os.path.normcase(os.path.realpath(selected)) != os.path.normcase(
        os.path.realpath(executable_path(codex_bin, "codex")))
    changed = False
    if os.name == "nt":
        for entry in directories:
            changed = windows_user_path(entry, dry_run, append=entry == codex_bin and external) or changed
    else:
        # Do not activate a retained package over the external backend we checked.
        # The package stays available to prepare_team_runtime if that backend disappears.
        body = "\n".join(stores.shell_path_line(entry) for entry in directories
                         if entry != codex_bin or not external)
        for path in stores.shell_profiles():
            changed = update_managed_block(path, body, dry_run) or changed
    return changed


def path_is_configured(directory: Path) -> bool:
    normalized = directory.resolve(strict=False)
    active = {
        Path(value).expanduser().resolve(strict=False)
        for value in os.environ.get("PATH", "").split(os.pathsep)
        if value
    }
    if normalized in active:
        return True
    if os.name == "nt":
        import winreg
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Environment") as key:
                current, _ = winreg.QueryValueEx(key, "Path")
        except (FileNotFoundError, OSError):
            return False
        return any(
            Path(os.path.expandvars(value)).resolve(strict=False) == normalized
            for value in current.split(";") if value
        )
    profiles = stores.shell_profiles()
    return all(
        path.exists()
        and stores.BLOCK_START in path.read_text(encoding="utf-8")
        and stores.shell_path_line(directory) in path.read_text(encoding="utf-8")
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
    stores.add_migration_arguments(parser)
    parser.add_argument("--install-dir", type=Path, default=None, help="Per-user PATH directory.")
    parser.add_argument("--dry-run", action="store_true", help="Describe changes without writing.")
    parser.add_argument(
        "--codex-login", choices=["auto", "browser", "device", "skip"], default="auto",
        help="Missing Team login: auto uses browser in a terminal, otherwise device link/code; "
             "skip fails without signing in. Verify/dry-run never start login.",
    )
    parser.add_argument(
        "--verify", action="store_true", help="Verify installed tools without changing them."
    )
    parser.add_argument(
        "--no-path", action="store_true", help="Do not change or verify user PATH/store environment persistence."
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
            "Pin one tool as ctx=VERSION, dec=VERSION, nav=VERSION, svg=VERSION, or team=VERSION; "
            "repeatable. "
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
        if "pira_team" not in released:
            raise RuntimeError("ordinary PIRA tools setup requires a release containing pira_team; use --tool only for partial maintenance")
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
    binary_asset = f"{tool_name}-{version}-{platform_key}{suffix}"
    compression = record.get("compression")
    if compression is None:
        expected_asset = binary_asset
    elif compression == "gzip":
        expected_asset = f"{binary_asset}.gz"
    else:
        raise RuntimeError(f"unsupported release compression for {tool_name}")
    if asset != expected_asset:
        raise RuntimeError(f"invalid release asset name for {tool_name}: {asset}")
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", expected):
        raise RuntimeError(f"invalid release checksum for {tool_name}")
    if not isinstance(size, int) or size <= 0 or size > MAX_BINARY_BYTES:
        raise RuntimeError(f"invalid release size for {tool_name}")
    if compression == "gzip":
        asset_hash = record.get("asset_sha256")
        asset_size = record.get("asset_size")
        if not isinstance(asset_hash, str) or not re.fullmatch(
            r"[0-9a-fA-F]{64}", asset_hash
        ):
            raise RuntimeError(f"invalid compressed asset checksum for {tool_name}")
        if (
            not isinstance(asset_size, int)
            or asset_size <= 0
            or asset_size > MAX_BINARY_BYTES
        ):
            raise RuntimeError(f"invalid compressed asset size for {tool_name}")
    else:
        asset_hash = expected
        asset_size = size
    destination = executable_path(install_dir, tool_name)
    existing_hash = (
        sha256(destination)
        if destination.is_file() and not destination.is_symlink()
        else None
    )
    action = (
        "unchanged"
        if existing_hash == expected.lower() and (os.name == "nt" or os.access(destination, os.X_OK))
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
        asset_hash=asset_hash.lower(),
        asset_size=asset_size,
        compression=compression,
        existing_hash=existing_hash,
        action=action,
    )


def check_team_schema(schema: dict, expected: dict) -> None:
    """Check the published native API inventory; runtime also validates request values."""
    def fields(node: dict, names: list[str]) -> None:
        for name in names:
            if name not in node.get("properties", {}):
                raise ValueError(f"missing published field {name}")

    def variant(node: dict, tag: str, value: str) -> dict:
        for item in node.get("oneOf", []):
            if value in item.get("properties", {}).get(tag, {}).get("enum", []):
                return item
        raise ValueError(f"missing published {tag} {value} (unsupported schema layout)")

    fields(schema, expected.get("fields", []))
    for signal in expected.get("signals", []):
        variant(schema, "method", signal)
    for method, names in expected.get("methods", {}).items():
        item = variant(schema, "method", method)
        reference = item["properties"]["params"]["$ref"]
        if not reference.startswith("#/definitions/"):
            raise ValueError(f"unsupported parameter reference for {method}")
        fields(schema["definitions"][reference.removeprefix("#/definitions/")], names)
    for name, names in expected.get("definitions", {}).items():
        fields(schema["definitions"][name], names)
    for name, variants in expected.get("variants", {}).items():
        for tag, names in variants.items():
            fields(variant(schema["definitions"][name], "type", tag), names)


def check_team_initialize(command: list[str], env: dict[str, str], *, timeout: float = 10) -> None:
    """Wait with live stdin: 10s by default, then 1s EOF grace and 2s kill/reap limit."""
    initialize = {"id": 1, "method": "initialize", "params": {
        "clientInfo": {"name": "pira_team_setup", "version": "1"},
        "capabilities": {"experimentalApi": False}}}
    # Regular files avoid blocking pipe readers (including inherited descendant pipes),
    # without platform-specific pipe polling. Read/write opens need independent offsets.
    with tempfile.TemporaryDirectory(prefix="pira-team-initialize-") as temporary:
        root = Path(temporary)
        with (root / "stdout").open("wb") as out, (root / "stderr").open("wb") as err, \
             (root / "stdout").open("rb") as reader:
            child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=out, stderr=err,
                                     env=env, bufsize=0)
            matched = False
            try:
                deadline = time.monotonic() + timeout
                # This one fixed, small initialize frame fits an empty stdin pipe.
                child.stdin.write((json.dumps(initialize) + "\n").encode("utf-8"))
                child.stdin.flush()
                pending = b""
                received = 0
                exited = False
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(command, timeout)
                    data = reader.readline(65537 - received)
                    if data:
                        received += len(data)
                        if received > 65536:
                            raise ValueError("initialize output exceeds 64 KiB")
                        pending += data
                    if pending and (pending.endswith(b"\n") or (exited and not data)):
                        frame, pending = pending, b""
                        if not frame.strip():
                            continue
                        reply = json.loads(frame)
                        if not isinstance(reply, dict) or type(reply.get("id")) is not int or reply["id"] != 1:
                            continue
                        if ("error" in reply or not isinstance(reply.get("result"), dict)
                                or not isinstance(reply["result"].get("userAgent"), str)):
                            raise ValueError("invalid matching initialize response")
                        matched = True
                        return
                    if data:
                        continue
                    # A process may write its final reply between our read and exit.
                    # Drain once more after observing exit before diagnosing missing output.
                    if exited:
                        raise ValueError("backend exited without a complete matching initialize response")
                    try:
                        child.wait(timeout=min(0.02, remaining))
                        exited = True
                    except subprocess.TimeoutExpired:
                        pass
            except ValueError as error:
                with (root / "stderr").open("rb") as diagnostics_file:
                    diagnostics = diagnostics_file.read(65536).decode("utf-8", errors="replace")
                raise ValueError(f"native strict-config stdio initialize failed: {error}; "
                                 f"stderr (up to 64 KiB): {diagnostics.strip()}; "
                                 "interactive/shared-daemon compatibility is a separate check") from error
            finally:
                try:
                    child.stdin.close()
                except OSError:
                    pass  # An already-exited peer may have closed its input.
                forced = False
                try:
                    child.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    forced = True
                    child.kill()
                    child.wait(timeout=2)
                if matched and not forced and child.returncode:
                    raise ValueError(f"native stdio initialize backend exited with status {child.returncode}")


def check_team_protocol(executable: str) -> None:
    """Probe direct stdio, not the interactive shared daemon; no auth or model turn."""
    contract_path = REPO_ROOT / "tools/src/pira_team/backend_contract.json"
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory(prefix="pira-team-backend-") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir()
        env = dict(os.environ, CODEX_HOME=str(home), TMPDIR=temporary, TMP=temporary, TEMP=temporary)
        for key in ("CODEX_API_KEY", "OPENAI_API_KEY", "CODEX_THREAD_ID", "CODEX_SESSION_ID"):
            env.pop(key, None)
        command = [executable, "app-server", "--strict-config", "--stdio"]
        for setting in contract["config"]:
            command.extend(["-c", setting])
        schema_dir = root / "schema"
        result = subprocess.run(
            [executable, "app-server", "generate-json-schema", "--out", str(schema_dir)],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10,
            check=False, env=env,
        )
        if result.returncode:
            raise ValueError(f"native schema generation failed (exit {result.returncode}): {result.stderr.strip()}")
        for name, expected in contract["schemas"].items():
            path = schema_dir / name
            if path.is_symlink() or not path.is_file() or path.stat().st_size > 16 * 1024 * 1024:
                raise ValueError(f"missing/oversized native schema {name}")
            schema = json.loads(path.read_text(encoding="utf-8"))
            try:
                check_team_schema(schema, expected)
            except (KeyError, TypeError, AttributeError, ValueError) as error:
                raise ValueError(f"{name}: {error}") from error
        check_team_initialize(command, env)


def check_team_runtime(tools: list[str], executable: str | None = None) -> str | None:
    """Check the backend only when Team is selected; never log in or install Codex."""
    if "pira_team" not in tools:
        return None
    executable = executable or shutil.which("codex")
    guidance = (
        "Install/update the Codex CLI on the agent's execution host and make codex available "
        f"in its PATH, then rerun setup: {CODEX_INSTALL_GUIDE}. "
        "You can keep using the app or IDE extension; the terminal UI need not be running."
    )
    if executable is None:
        raise RuntimeError(f"pira_team requires the Codex executable. {guidance}")
    def probe(arguments: list[str]) -> str:
        try:
            with tempfile.TemporaryDirectory(prefix="pira-team-version-") as home:
                result = subprocess.run(
                    [executable, *arguments], stdin=subprocess.DEVNULL,
                    capture_output=True, text=True, timeout=10, check=False,
                    env=dict(os.environ, CODEX_HOME=home),
                )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise RuntimeError(f"Cannot check the Codex backend for pira_team: {error}. {guidance}") from error
        if result.returncode != 0:
            raise RuntimeError(f"Codex {' '.join(arguments)} failed (exit {result.returncode}). {guidance}")
        return result.stdout.strip()
    version_text = probe(["--version"])
    version = re.fullmatch(r"codex-cli (\d+)\.(\d+)\.(\d+)(?:\+[A-Za-z0-9.-]+)?", version_text)
    if version is None:
        raise RuntimeError(f"Cannot verify a stable Codex version for pira_team. {guidance}")
    if tuple(map(int, version.groups())) < TEAM_CODEX_MIN_VERSION:
        minimum = ".".join(map(str, TEAM_CODEX_MIN_VERSION))
        raise RuntimeError(f"pira_team requires Codex >= {minimum}; found {version_text}. {guidance}")
    help_text = probe(["app-server", "--help"])
    missing = [flag for flag in ("--stdio", "--strict-config")
               if re.search(r"(?<![\w-])" + re.escape(flag) + r"(?![\w-])", help_text) is None]
    if missing:
        raise RuntimeError(
            "Codex app-server lacks required options: " + ", ".join(missing)
            + ". Use a Codex build with the required native app-server interface; " + guidance
        )
    try:
        check_team_protocol(executable)
    except (OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError) as error:
        raise RuntimeError(f"Unsupported native Codex backend for pira_team: {error}. {guidance}") from error
    return version_text


def install_codex(install_dir: Path, platform_key: str) -> Path:
    """Install the latest stable official package without login or remote script execution."""
    target = CODEX_TARGETS.get(platform_key)
    if target is None:
        raise RuntimeError(f"Automatic Codex installation does not support {platform_key}")
    destination = install_dir / CODEX_PACKAGE_DIR
    if destination.exists() or destination.is_symlink():
        raise RuntimeError(f"Refusing to overwrite existing Codex package: {destination}")
    try:
        release = json.loads(request_bytes(CODEX_RELEASE_API, limit=MAX_INDEX_BYTES,
                                           accept="application/vnd.github+json"))
    except (ValueError, UnicodeError) as error:
        raise RuntimeError("Invalid official Codex release metadata") from error
    tag = release.get("tag_name", "") if isinstance(release, dict) else ""
    if (not isinstance(tag, str) or not re.fullmatch(r"rust-v[0-9]+\.[0-9]+\.[0-9]+", tag)
            or release.get("draft") is not False or release.get("prerelease") is not False):
        raise RuntimeError("Cannot resolve a stable official Codex release")
    name = f"codex-package-{target}.tar.gz"
    assets = release.get("assets", [])
    if not isinstance(assets, list):
        raise RuntimeError("Invalid Codex release assets")
    matches = [a for a in assets if isinstance(a, dict) and a.get("name") == name]
    if len(matches) != 1:
        raise RuntimeError(f"Official Codex release lacks a unique package for {platform_key}")
    asset = matches[0]
    digest = asset.get("digest", "")
    size = asset.get("size")
    if (not isinstance(digest, str) or not re.fullmatch(r"sha256:[0-9a-fA-F]{64}", digest)
            or type(size) is not int or not 0 < size <= MAX_CODEX_BYTES):
        raise RuntimeError("Invalid Codex package checksum or size")
    # Construct the official URL; do not execute URLs/scripts supplied by metadata.
    url = f"https://github.com/openai/codex/releases/download/{tag}/{name}"
    print(f"Installing missing Codex: {tag} ({platform_key}) -> {destination}")
    with tempfile.TemporaryDirectory(prefix="pira-codex-download-") as temporary:
        archive = Path(temporary) / name
        request_to_path(url, archive, limit=MAX_CODEX_BYTES, expected_size=size,
                        expected_hash=digest[7:].lower())
        install_dir.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".pira-codex-stage-", dir=install_dir) as stage:
            package = Path(stage) / "package"
            package.mkdir()
            # Never extract links, devices or archive-provided permissions/ownership.
            total = 0
            seen: set[str] = set()
            try:
                with tarfile.open(archive, "r:gz") as tar:
                    for member in tar:
                        parts = member.name.split("/")
                        parts = [part for part in parts if part != "."]
                        if (not parts or any(not part or part == ".." or
                                re.search(r'[\\:\x00-\x1f]', part) for part in parts)
                                or member.name.startswith("/") or len(seen) >= 1024):
                            raise RuntimeError("Unsafe Codex archive path")
                        relative = "/".join(parts)
                        if relative in seen:
                            raise RuntimeError("Duplicate Codex archive path")
                        seen.add(relative)
                        out = package.joinpath(*parts)
                        if member.isdir():
                            out.mkdir(parents=True, exist_ok=True)
                        elif member.isfile():
                            total += member.size
                            if member.size < 0 or total > 2 * MAX_CODEX_BYTES:
                                raise RuntimeError("Expanded Codex package exceeds safety limit")
                            out.parent.mkdir(parents=True, exist_ok=True)
                            source = tar.extractfile(member)
                            if source is None:
                                raise RuntimeError("Missing Codex archive member")
                            with source, out.open("xb") as sink:
                                shutil.copyfileobj(source, sink)
                            out.chmod(0o755 if member.mode & 0o111 else 0o644)
                        else:
                            raise RuntimeError("Codex archive contains a link or special file")
            except tarfile.TarError as error:
                raise RuntimeError(f"Invalid Codex package archive: {error}") from error
            suffix = ".exe" if platform_key.startswith("windows-") else ""
            required = ["codex-package.json", f"bin/codex{suffix}",
                        f"bin/codex-code-mode-host{suffix}", f"codex-path/rg{suffix}"]
            if platform_key.startswith("linux-"):
                required.append("codex-resources/bwrap")
            if any(not (package / entry).is_file() for entry in required):
                raise RuntimeError("Incomplete Codex package")
            binary = package / "bin" / f"codex{suffix}"
            version = check_team_runtime(["pira_team"], str(binary))
            if version != f"codex-cli {tag.removeprefix('rust-v')}":
                raise RuntimeError("Downloaded Codex version does not match release metadata")
            if destination.exists() or destination.is_symlink():
                raise RuntimeError(f"Codex destination appeared during installation: {destination}")
            package.rename(destination)
    return destination / "bin" / f"codex{suffix}"


def selected_codex_binary(install_dir: Path) -> str | None:
    """Resolve the same external-first backend for setup and migration; no writes."""
    existing = shutil.which("codex")
    if existing:
        return str(Path(existing).resolve())
    managed = executable_path(install_dir / CODEX_PACKAGE_DIR / "bin", "codex")
    return str(managed.resolve()) if managed.is_file() else None


def prepare_team_runtime(tools: list[str], install_dir: Path, platform_key: str,
                         *, verify: bool, dry_run: bool) -> str | None:
    if "pira_team" not in tools:
        return None
    executable = selected_codex_binary(install_dir)
    if executable:
        return check_team_runtime(tools, executable)
    managed = executable_path(install_dir / CODEX_PACKAGE_DIR / "bin", "codex")
    if verify:
        return check_team_runtime(tools)  # Fail without downloading or writing.
    if dry_run:
        print(f"DRY-RUN: would download latest stable Codex into {managed.parent.parent}")
        return None
    return check_team_runtime(tools, str(install_codex(install_dir, platform_key)))


def ensure_team_auth(tools: list[str], install_dir: Path, *, verify: bool,
                     dry_run: bool, login: str) -> None:
    """Check Team's auth source, initiating official login only during normal setup."""
    if "pira_team" not in tools:
        return
    if dry_run and not verify:
        print("DRY-RUN: would check Team-compatible Codex authentication and offer login if missing")
        return
    if os.environ.get("CODEX_API_KEY"):
        print("OK: CODEX_API_KEY is configured for Team (not remotely validated)")
        return
    executable = shutil.which("codex") or str(
        executable_path(install_dir / CODEX_PACKAGE_DIR / "bin", "codex")
    )
    home = Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")
    auth_file = home / "auth.json"
    # Team links file auth into a private home; a keyring-only status is insufficient.
    command = [executable, "-c", 'cli_auth_credentials_store="file"', "login"]
    env = dict(os.environ)
    for key in ("OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"):
        env.pop(key, None)

    def cached_login_ready() -> bool:
        if not auth_file.is_file():
            return False
        try:
            result = subprocess.run(
                [*command, "status"], stdin=subprocess.DEVNULL, capture_output=True,
                text=True, timeout=15, check=False, env=env,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise RuntimeError("Could not check Codex login status; fix the runtime/configuration "
                               "and rerun setup. No login was started.") from error
        if result.returncode == 0:
            return True
        if result.returncode == 1 and "Not logged in" in (result.stdout + result.stderr).splitlines():
            return False
        # Status may include an account identifier or partial key; never echo its output.
        raise RuntimeError("Codex login status failed unexpectedly; inspect Codex configuration "
                           "or run codex login status privately. No automatic re-login was attempted.")

    if cached_login_ready():
        print("OK: Codex recognizes Team's file-backed login (model access not remotely validated)")
        return
    guidance = ("Rerun setup with --codex-login browser for local browser login, or "
                "--codex-login device for a link/code. Device login must be enabled in "
                "ChatGPT security/workspace settings. Admin-enforced storage rules are not bypassed.")
    if verify or login == "skip":
        raise RuntimeError("Team-compatible Codex login is missing. " + guidance)
    mode = login
    if mode == "auto":
        mode = "browser" if sys.stdin.isatty() else "device"
    print("Team needs a file-backed Codex login. The official flow will save credentials in "
          "CODEX_HOME/auth.json (default ~/.codex/auth.json); protect this file like a password. "
          "Global credential-storage configuration is unchanged.", flush=True)
    if mode == "device":
        command.append("--device-auth")
        print("Open Codex's displayed link and enter its code. Device login must be enabled "
              "in ChatGPT security/workspace settings.", flush=True)
    else:
        print("Complete login in the browser Codex opens, or use the URL it displays.", flush=True)
    print("Waiting up to 5 minutes; Ctrl-C cancels. Run setup directly in a terminal "
          "if your agent hides subprocess output.", flush=True)
    try:
        # Inherit output so the official URL/code reaches the user immediately.
        result = subprocess.run(command, stdin=subprocess.DEVNULL, env=env,
                                timeout=300, check=False)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("Codex login timed out. " + guidance) from error
    except KeyboardInterrupt as error:
        raise RuntimeError("Codex login cancelled; setup is incomplete.") from error
    except OSError as error:
        raise RuntimeError("Could not launch Codex login. " + guidance) from error
    if result.returncode != 0:
        raise RuntimeError("Codex login did not complete. " + guidance)
    if not cached_login_ready():
        raise RuntimeError("Login returned without a Team-compatible file cache. "
                           "Check enforced credential-storage requirements. " + guidance)
    print("OK: Team-compatible Codex login is ready (model access not remotely validated)")

def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if not args.no_path:
        stores.configuration_toml()
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
    if "pira_team" in tools:
        runtime = prepare_team_runtime(tools, install_dir, platform_key,
                                       verify=args.verify, dry_run=args.dry_run)
        if runtime:
            print(f"OK: Team backend {runtime}; native API inventory and isolated strict-config stdio initialize verified (not shared daemon/model access)")
    # Preparation can install a managed backend outside PATH. Pass that exact
    # selection to retained-history preflight before publishing any store paths.
    codex_binary = selected_codex_binary(install_dir) if "pira_team" in tools else None
    store_plan = (stores.plan_store_environment(tools, codex_binary=codex_binary,
                  completed_ctx_only=args.completed_ctx_only, fresh_team=args.fresh_team,
                  exclude_ctx_records=args.exclude_ctx_record)
                  if not args.no_path else stores.StorePlan())
    if "pira_team" in tools:
        ensure_team_auth(tools, install_dir, verify=args.verify, dry_run=args.dry_run,
                         login=args.codex_login)
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
        stores.apply_store_environment(store_plan, dry_run=True, verify=True)
        failures: list[str] = []
        for selection in selections:
            if selection.action != "unchanged":
                failures.append(f"{selection.name}: installed binary is missing or stale")
                continue
            version = direct_version(selection.destination)
            if not version_matches(selection.name, selection.version, version):
                failures.append(f"{selection.name}: unexpected version: {version}")
        codex_bin = install_dir / CODEX_PACKAGE_DIR / "bin"
        if ("pira_team" in tools and not shutil.which("codex") and not args.no_path
                and not path_is_configured(codex_bin)):
            failures.append("managed Codex directory is not configured in the user PATH")
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
        stores.apply_store_environment(store_plan, dry_run=args.dry_run)
        ensure_path(install_dir, args.dry_run,
                    include_codex="pira_team" in tools and not shutil.which("codex"))

    if not args.dry_run:
        restart_needed = "pira_team" in tools and not shutil.which("codex")
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
