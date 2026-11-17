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
import shlex
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

IS_WINDOWS = os.name == "nt"
REPO_ROOT = Path(__file__).resolve().parents[2]
SELECTOR_PATH = REPO_ROOT / "tools" / "select_tool_for_platform.py"
RETIRED_TOOLS = {"pira_codenav"}
TEAM_CODEX_MIN_VERSION = (0, 166, 0)
TEAM_CLAUDE_MIN_VERSION = (2, 1, 311)
CLAUDE_INSTALL_GUIDE = "https://code.claude.com/docs/en/setup"
CODEX_RELEASE_API = "https://api.github.com/repos/openai/codex/releases/latest"
MAX_CODEX_BYTES = 512 * 1024 * 1024
CODEX_PACKAGE_DIR = ".pira-codex"
MANAGED_SELECTION = "managed-v1\n"
CODEX_TARGETS = {
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
    "linux-x64": "x86_64-unknown-linux-musl",
    "linux-arm64": "aarch64-unknown-linux-musl",
    "windows-x64": "x86_64-pc-windows-msvc",
    "windows-arm64": "aarch64-pc-windows-msvc",
}
CODEX_INSTALL_GUIDE = "https://learn.chatgpt.com/docs/cli"
WINDOWS_CODEX_GUIDANCE = (
    "Windows Team requires a native codex.exe, not codex.cmd/codex.ps1 shims. "
    "Add the directory containing a supported stable codex.exe to PATH for both setup and Team, "
    f"restart the session, then rerun setup: {CODEX_INSTALL_GUIDE}. "
    "Setup does not parse shims or request elevation."
)
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
        "write": "pira_write_check",
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
    suffix = ".exe" if IS_WINDOWS else ""
    return directory / f"{tool_name}{suffix}"


def windows_codex_binary() -> str | None:
    """Use Team's native SearchPathW lookup, ignoring PATHEXT command shims."""
    import ctypes
    from ctypes import wintypes

    search = ctypes.WinDLL("kernel32", use_last_error=True).SearchPathW
    search.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPCWSTR,
                       wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.LPWSTR)]
    search.restype = wintypes.DWORD
    buffer = ctypes.create_unicode_buffer(32768)
    count = search(None, "codex.exe", None, len(buffer), buffer, None)
    if count == 0:
        error = ctypes.get_last_error()
        if error in (2, 3):  # File/path not found; other native failures must stay visible.
            return None
        raise ctypes.WinError(error)
    if count >= len(buffer):
        raise RuntimeError("Windows codex.exe search result exceeds 32768 characters")
    return buffer.value


def codex_on_path(install_dir: Path | None = None) -> str | None:
    """Resolve Team's native Windows image, never an npm command shim."""
    if not IS_WINDOWS:
        return shutil.which("codex")
    # SearchPathW starts with the application's directory. Team's directory is
    # the tool install directory, not the Python interpreter's directory.
    team_image = team_install_dir(install_dir or default_install_dir()) / "codex.exe"
    if team_image.is_file():
        return str(team_image.resolve())
    selected = windows_codex_binary()
    if selected and Path(selected).resolve().parent == Path(sys.executable).resolve().parent:
        raise RuntimeError("Ambiguous codex.exe in the Python application directory; "
                           "setup cannot certify that Team will launch this image. " + WINDOWS_CODEX_GUIDANCE)
    return selected


def update_managed_block(path: Path, body: str, dry_run: bool) -> bool:
    old = stores.read_profile(path)
    new = stores.managed_block_text(path, old, body)
    if new == old:
        return False
    stores.write_profile(path, new, dry_run)
    return True


def windows_user_path(directory: Path, dry_run: bool, *, append: bool = False,
                      prefix: list[Path] | None = None) -> bool:
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
    if prefix is not None:
        selected = {os.path.normcase(os.path.realpath(entry)) for entry in prefix}
        updated = ";".join([*(str(entry) for entry in prefix), *(part for part in parts
            if os.path.normcase(os.path.realpath(os.path.expandvars(part))) not in selected)])
        if updated == current:
            return False
    elif append:
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


def managed_codex_selected(install_dir: Path) -> bool:
    package = install_dir / CODEX_PACKAGE_DIR
    marker = package / "selected"
    if package.is_symlink() or (package.exists() and not package.is_dir()):
        raise RuntimeError(f"Refusing ambiguous managed Codex package: {package}")
    if marker.is_symlink() or (marker.exists() and
            (not marker.is_file() or marker.read_bytes() != MANAGED_SELECTION.encode("utf-8"))):
        raise RuntimeError(f"Refusing ambiguous managed Codex selection: {marker}")
    return marker.is_file()


def team_install_dir(install_dir: Path) -> Path:
    """Colocate managed Windows Team/Codex so application-directory lookup wins."""
    if IS_WINDOWS and managed_codex_selected(install_dir):
        # PATH alone cannot beat application/current/system directories. Keep the
        # complete package together: copying codex.exe alone loses its resources.
        return install_dir / CODEX_PACKAGE_DIR / "bin"
    return install_dir


def claude_managed_bin(install_dir: Path) -> Path | None:
    package = install_dir / ".pira-claude"
    marker = package / "selected"
    if package.is_symlink() or marker.is_symlink():
        raise RuntimeError(f"Refusing ambiguous managed Claude selection: {marker}")
    if marker.exists():
        if not marker.is_file() or marker.read_bytes() != MANAGED_SELECTION.encode("utf-8"):
            raise RuntimeError(f"Refusing ambiguous managed Claude selection: {marker}")
        return package / "bin"
    return None


def activate_directory(directory: Path) -> None:
    """Process-local activation also works with --no-path; never persist here."""
    parts = [part for part in os.environ.get("PATH", "").split(os.pathsep) if part]
    key = os.path.normcase(os.path.realpath(directory))
    os.environ["PATH"] = os.pathsep.join([str(directory), *(part for part in parts
        if os.path.normcase(os.path.realpath(part)) != key)])


def activation_path_line(directory: Path | str) -> str:
    # Unlike fallback PATH entries, an explicitly selected backend must win even
    # when its directory was already present later in the caller's PATH.
    value = shlex.quote(str(directory))
    return f'case "$PATH" in {value}|{value}:*) ;; *) export PATH={value}:"$PATH" ;; esac'


def ensure_path(directory: Path, dry_run: bool, *, include_codex: bool = False) -> bool:
    codex_bin = directory / CODEX_PACKAGE_DIR / "bin"
    directories = [directory]
    if include_codex or executable_path(codex_bin, "codex").is_file():
        directories.append(codex_bin)
    claude_bin = claude_managed_bin(directory)
    if claude_bin is not None:
        directories.append(claude_bin)
    selected = selected_codex_binary(directory)
    external = selected is not None and os.path.normcase(os.path.realpath(selected)) != os.path.normcase(
        os.path.realpath(executable_path(codex_bin, "codex")))
    changed = False
    if os.name == "nt":
        if managed_codex_selected(directory) or claude_bin is not None:
            prefix = list(reversed([entry for entry in directories if entry != codex_bin or not external]))
            changed = windows_user_path(prefix[0], dry_run, prefix=prefix)
            if codex_bin in directories and external:
                changed = windows_user_path(codex_bin, dry_run, append=True) or changed
        else:
            for entry in directories:
                changed = windows_user_path(entry, dry_run, append=entry == codex_bin and external) or changed
    else:
        retained = [entry for entry in directories if entry != codex_bin or not external]
        lines = [stores.shell_path_line(entry) for entry in retained]
        if managed_codex_selected(directory) or claude_bin is not None:
            # Activate the whole prefix once: separate move-first lines would
            # alternate order and grow PATH every time the profile is sourced.
            lines.append(activation_path_line(os.pathsep.join(str(entry) for entry in reversed(retained))))
        body = "\n".join(lines)
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
        and any(line in path.read_text(encoding="utf-8") for line in
                (stores.shell_path_line(directory), activation_path_line(directory)))
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


class _IncludeHarness(argparse.Action):
    def __call__(self, parser: argparse.ArgumentParser, namespace: argparse.Namespace,
                 values: list[str], option_string: str | None = None) -> None:
        if len(set(values)) == 2:
            harness = "both"
        else:
            harness = "codex" if "Codex" in values else "claude"
        setattr(namespace, self.dest, harness)


def add_harness_arguments(parser: argparse.ArgumentParser) -> None:
    """Share setup selection while retaining the internal legacy harness names."""
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--include", nargs="+", choices=["Codex", "ClaudeCode"],
                           dest="harness", action=_IncludeHarness, default="codex",
                           help="CLIs to prepare automatically (default: Codex); compatible native installs and existing authentication are preserved.")
    selection.add_argument("--harness", choices=["codex", "claude", "both"], default=argparse.SUPPRESS,
                           help="Compatibility selection; cannot be combined with --include.")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Install or refresh cloud-built PIRA tools for this user."
    )
    stores.add_migration_arguments(parser)
    parser.add_argument("--install-dir", type=Path, default=None, help="Per-user PATH directory.")
    add_harness_arguments(parser)
    parser.add_argument("--dry-run", action="store_true", help="Describe changes without writing.")
    parser.add_argument("--auth-only", action="store_true",
                        help="Prepare/verify Codex file auth for Team only; no tool install, release lookup, PATH change or store migration.")
    parser.add_argument("--reauth", action="store_true",
                        help="Explicitly renew native file login during setup, even if offline status accepts the cache; not with verify or login skip.")
    parser.add_argument(
        "--codex-login", choices=["auto", "browser", "device", "skip"], default="auto",
        help="Missing Team file login: auto offers browser login only in a terminal; "
             "browser/device explicitly allow setup login, skip/verify/dry-run never sign in.",
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
            "Pin one tool as ctx=VERSION, dec=VERSION, nav=VERSION, svg=VERSION, team=VERSION, or write=VERSION; "
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


def check_team_initialize(command: list[str], env: dict[str, str], *, timeout: float = 10,
                          project: str | None = None) -> None:
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
                expected_id = 1
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
                        if not isinstance(reply, dict) or type(reply.get("id")) is not int or reply["id"] != expected_id:
                            continue
                        result = reply.get("result")
                        if "error" in reply or not isinstance(result, dict):
                            raise ValueError("invalid matching startup response")
                        if expected_id == 1:
                            if not isinstance(result.get("userAgent"), str):
                                raise ValueError("invalid matching initialize response")
                            if project is None:
                                matched = True
                                return
                            for request in ({"method": "initialized"}, {
                                    "id": 2, "method": "config/read",
                                    "params": {"includeLayers": True, "cwd": project}}):
                                child.stdin.write((json.dumps(request) + "\n").encode("utf-8"))
                            child.stdin.flush()
                            expected_id = 2
                            continue
                        if result.get("config", {}).get("projects") != {project: {"trust_level": "untrusted"}}:
                            raise ValueError("project trust override did not preserve the exact untrusted cwd key")
                        project_folder = str(Path(project) / ".codex")
                        def disabled_project_layer(layer: dict[str, object]) -> bool:
                            folder = layer.get("name", {}).get("dotCodexFolder")
                            if not isinstance(folder, str) or not layer.get("disabledReason"):
                                return False
                            try:
                                # Physical identity handles Windows verbatim paths and aliases;
                                # the config.projects trust key above must remain byte-exact.
                                return Path(folder).samefile(project_folder)
                            except (OSError, ValueError):
                                return False
                        if not any(disabled_project_layer(layer) for layer in result.get("layers", [])):
                            raise ValueError("untrusted project configuration layer was not disabled")
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
    # Match runtime: live tier updates are optional; steering/interrupt remain core.
    del contract["schemas"]["v2/TurnSettingsUpdateResponse.json"]
    del contract["schemas"]["ClientRequest.json"]["methods"]["turn/settings/update"]
    with tempfile.TemporaryDirectory(prefix="pira-team-backend-") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir()
        # No caller credentials or backend selectors enter this no-model probe.
        env = {key: value for key, value in os.environ.items()
               if key.upper() in {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"}}
        env.update(HOME=str(home), USERPROFILE=str(home), CODEX_HOME=str(home),
                   TMPDIR=temporary, TMP=temporary, TEMP=temporary)
        project = (root / "project.path with spaces").resolve()
        (project / ".git").mkdir(parents=True)
        (project / ".codex").mkdir()
        (project / ".codex/config.toml").write_text('model="project-layer-must-not-load"\n')
        # JSON strings are TOML basic strings except for literal DEL.
        project_key = json.dumps(str(project), ensure_ascii=False).replace("\x7f", "\\u007f")
        (home / "config.toml").write_text(f'[projects.{project_key}]\ntrust_level="trusted"\n', encoding="utf-8")
        command = [executable, "app-server", "--strict-config", "--stdio",
                   "-c", f'projects={{{project_key}={{trust_level="untrusted"}}}}']
        for setting in contract["config"]:
            command.extend(["-c", setting])
        schema_dir = root / "schema"
        result = subprocess.run(
            [executable, "app-server", "generate-json-schema", "--experimental", "--out", str(schema_dir)],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10,
            check=False, env=env,
        )
        if result.returncode:
            raise ValueError(f"native schema generation failed (exit {result.returncode}): "
                             f"{result.stdout.strip()}\n{result.stderr.strip()}")
        for name, expected in contract["schemas"].items():
            path = schema_dir / name
            if path.is_symlink() or not path.is_file() or path.stat().st_size > 16 * 1024 * 1024:
                raise ValueError(f"missing/oversized native schema {name}")
            schema = json.loads(path.read_text(encoding="utf-8"))
            try:
                check_team_schema(schema, expected)
            except (KeyError, TypeError, AttributeError, ValueError) as error:
                raise ValueError(f"{name}: {error}") from error
        check_team_initialize(command, env, project=str(project))


def check_team_runtime(tools: list[str], executable: str | None = None) -> str | None:
    """Check the backend only when Team is selected; never log in or install Codex."""
    if "pira_team" not in tools:
        return None
    executable = executable or codex_on_path()
    guidance = (
        "Install/update the Codex CLI on the agent's execution host and make codex available "
        f"in its PATH, then rerun setup: {CODEX_INSTALL_GUIDE}. "
        "You can keep using the app or IDE extension; the terminal UI need not be running."
    )
    if IS_WINDOWS:
        guidance += " " + WINDOWS_CODEX_GUIDANCE
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
            outputs = (getattr(error, "stdout", None), getattr(error, "stderr", None))
            detail = "\n".join(part.decode(errors="replace") if isinstance(part, bytes) else part
                               for part in outputs if part)
            raise RuntimeError(f"Cannot check the Codex backend for pira_team: {error}. "
                               f"{detail}\n{guidance}") from error
        if result.returncode != 0:
            raise RuntimeError(f"Codex {' '.join(arguments)} failed (exit {result.returncode}).\n"
                               f"{result.stdout}\n{result.stderr}\n{guidance}")
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
    print(f"Preparing official Codex: {tag} ({platform_key}) -> {destination}")
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
    """Resolve the persistent managed selection, otherwise external-first; no writes."""
    managed = executable_path(install_dir / CODEX_PACKAGE_DIR / "bin", "codex")
    if managed_codex_selected(install_dir):
        if managed.exists() and not managed.is_file():
            raise RuntimeError(f"Selected managed Codex is not a regular file: {managed}")
        selected = str(managed)
    else:
        selected = codex_on_path(install_dir) or (str(managed) if managed.is_file() else None)
    if selected:
        resolved = Path(selected).resolve()
        if IS_WINDOWS and resolved.suffix.lower() != ".exe":
            raise RuntimeError(f"Windows codex.exe resolves to a non-native launcher: {resolved}. "
                               + WINDOWS_CODEX_GUIDANCE)
        if Path(selected).parent == managed.parent and (managed.parent.is_symlink() or managed.is_symlink()):
            raise RuntimeError(f"Refusing symlink in managed Codex binary: {managed}")
        return str(resolved)
    return None


def prepare_claude_runtime(*, verify: bool, dry_run: bool, config_dir: Path | None = None,
                           install_dir: Path | None = None) -> str | None:
    """Compatibility boundary shared with full setup and its mocked backend."""
    import setup_pira_claude_runtime
    return setup_pira_claude_runtime.prepare_runtime(
        sys.modules[__name__], install_dir if install_dir is not None else default_install_dir(),
        verify=verify, dry_run=dry_run, config_dir=config_dir,
    )


def preserved_package_path(package: Path, label: str) -> Path:
    for number in range(1, 10001):
        candidate = package.with_name(f"{package.name}.{label}-{number}")
        if not candidate.exists() and not candidate.is_symlink():
            return candidate
    raise RuntimeError(f"Too many preserved Codex packages beside {package}")


def select_managed_codex(install_dir: Path) -> None:
    package = install_dir / CODEX_PACKAGE_DIR
    managed_codex_selected(install_dir)  # Reject malformed markers rather than overwriting them.
    marker = package / "selected"
    if not marker.exists():
        with marker.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(MANAGED_SELECTION)


def replace_managed_codex(install_dir: Path, platform_key: str) -> str:
    """Serialize publication; a stale/foreign lock requires explicit operator recovery."""
    install_dir.mkdir(parents=True, exist_ok=True)
    lock = install_dir / ".pira-codex-prepare.lock"
    try:
        lock.mkdir()
    except FileExistsError as error:
        raise RuntimeError(f"Managed Codex preparation is locked: {lock}. "
                           "Wait for the other setup; if interrupted, inspect before removing the stale lock.") from error
    try:
        return publish_managed_codex(install_dir, platform_key)
    finally:
        lock.rmdir()


def publish_managed_codex(install_dir: Path, platform_key: str) -> str:
    """Validate before switching, retain old packages, and restore on activation failure."""
    package = install_dir / CODEX_PACKAGE_DIR
    managed_codex_selected(install_dir)
    old = package.exists()
    if old:
        manifest = package / "codex-package.json"
        if not manifest.is_file() or any(path.is_symlink() for path in package.rglob("*")):
            raise RuntimeError(f"Refusing unrecognized or symlinked managed Codex package: {package}")
        # Stage beside the destination, on the same filesystem as the atomic switch.
        with tempfile.TemporaryDirectory(prefix=".pira-codex-update-", dir=install_dir) as temporary:
            stage = Path(temporary)
            install_codex(stage, platform_key)
            replacement = stage / CODEX_PACKAGE_DIR
            # Team can be activated beside Codex on Windows. Preserve that executable
            # across a partial-tools Codex update, without changing its released bytes.
            team = executable_path(package / "bin", "pira_team")
            if team.exists():
                if not team.is_file():
                    raise RuntimeError(f"Refusing non-regular managed Team image: {team}")
                shutil.copy2(team, replacement / "bin" / team.name)
            backup = preserved_package_path(package, "backup")
            package.rename(backup)
            published = False
            try:
                if package.exists() or package.is_symlink():
                    raise RuntimeError(f"Codex destination appeared during activation: {package}")
                replacement.rename(package)
                published = True
                version = check_team_runtime(["pira_team"], str(executable_path(package / "bin", "codex")))
                select_managed_codex(install_dir)
            except BaseException:
                if published:
                    package.rename(preserved_package_path(package, "failed"))
                elif package.exists() or package.is_symlink():
                    raise RuntimeError(f"Codex activation was interrupted by another writer; "
                                       f"previous package retained at {backup}; inspect before recovery")
                backup.rename(package)
                raise
            print(f"Preserved previous managed Codex package: {backup}")
    else:
        installed = install_codex(install_dir, platform_key)
        try:
            version = check_team_runtime(["pira_team"], str(installed))
            select_managed_codex(install_dir)
        except BaseException:
            if package.exists():
                package.rename(preserved_package_path(package, "failed"))
            raise
    return version


def prepare_codex_runtime(install_dir: Path, platform_key: str, *, verify: bool,
                          dry_run: bool) -> str | None:
    """Prepare the selected Codex even when released Team is filtered out."""
    return prepare_team_runtime(["pira_team"], install_dir, platform_key,
                                verify=verify, dry_run=dry_run)


def prepare_team_runtime(tools: list[str], install_dir: Path, platform_key: str,
                         *, verify: bool, dry_run: bool) -> str | None:
    if "pira_team" not in tools:
        return None
    executable = selected_codex_binary(install_dir)
    if executable:
        try:
            version = check_team_runtime(["pira_team"], executable)
        except RuntimeError:
            if verify:
                raise
        else:
            managed = executable_path(install_dir / CODEX_PACKAGE_DIR / "bin", "codex")
            if not verify and not dry_run and Path(executable) == managed.resolve():
                select_managed_codex(install_dir)
                activate_directory(managed.parent)
            return version
    elif verify:
        guidance = "Selected Codex is missing; rerun normal setup to install a supported native CLI."
        raise RuntimeError(guidance + (" " + WINDOWS_CODEX_GUIDANCE if IS_WINDOWS else ""))
    if dry_run:
        print("DRY-RUN: would prepare a supported official managed Codex package; "
              "external installs and authentication remain unchanged")
        return None
    managed = executable_path(install_dir / CODEX_PACKAGE_DIR / "bin", "codex")
    # A previously retained compatible package can replace an old external CLI
    # without another download. Only successful native checks authorize selection.
    if managed.is_file() and (executable is None or Path(executable) != managed.resolve()):
        try:
            version = check_team_runtime(["pira_team"], str(managed.resolve()))
        except RuntimeError:
            pass
        else:
            select_managed_codex(install_dir)
            activate_directory(managed.parent)
            return version
    version = replace_managed_codex(install_dir, platform_key)
    activate_directory(managed.parent)
    print("OK: managed Codex selected; external installs, account and credential storage unchanged")
    return version


def team_auth_selection(home: Path) -> Path | None:
    """Read only Team's non-secret selection, never credential contents."""
    path = home / "pira-team-auth.json"
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return None
    if (not path.is_file() or path.is_symlink() or metadata.st_size > 65536
            or (os.name != "nt" and (metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077))):
        raise RuntimeError("Invalid/private Team auth selection; preserved without login")
    try:
        selection = json.loads(path.read_text(encoding="utf-8"))
        if (not isinstance(selection, dict) or set(selection) != {"version", "home"}
                or type(selection["version"]) is not int or selection["version"] != 1
                or not isinstance(selection["home"], str) or "\0" in selection["home"]
                or not Path(selection["home"]).is_absolute()):
            raise ValueError()
        return Path(selection["home"])
    except (OSError, ValueError, TypeError) as error:
        raise RuntimeError("Invalid Team auth selection; preserved without login (values withheld)") from error


def team_auth_configuration(home: Path) -> tuple[str, bool, dict]:
    """Select the same supported persistent worker config as Team, not CLI overrides."""
    worker = home / "pira-team-worker.toml"
    source = worker if worker.exists() or worker.is_symlink() else home / "config.toml"
    try:
        if source.exists() or source.is_symlink():
            if not source.is_file() or source.stat().st_size > 1024 * 1024:
                raise ValueError()
            with source.open("rb") as stream:
                text = stream.read(1024 * 1024 + 1)
            if len(text) > 1024 * 1024:
                raise ValueError()
            data = stores.configuration_toml().loads(text.decode("utf-8"))
        else:
            data = {}
        if "profile" in data:
            profile = data["profiles"][data["profile"]]
            if not isinstance(profile, dict): raise ValueError()
        else:
            profile = {}
        provider = profile.get("model_provider", data.get("model_provider", "openai"))
        definition = data.get("model_providers", {}).get(provider, {})
        if ("model_catalog_json" in profile or "model_catalog_json" in data
                or "model_catalog_url" in definition
                or any(key in definition for key in ("auth", "aws", "credential_file", "gateway_oauth"))):
            raise RuntimeError("Selected Team catalog/native-helper mode is unsupported by the isolated app-server auth adapter; select an explicit supported worker route. No login or model request was started (values withheld)")
        mode = data.get("cli_auth_credentials_store", "file")
        if mode not in {"file", "keyring", "auto", "ephemeral"}:
            raise ValueError()
        constraints = {key: data[key] for key in ("forced_login_method", "forced_chatgpt_workspace_id") if key in data}
        if "forced_login_method" in constraints and constraints["forced_login_method"] not in ("api", "chatgpt"):
            raise ValueError()
        if any(not isinstance(value, str) and not (isinstance(value, list) and all(isinstance(v, str) for v in value))
               for value in constraints.values()):
            raise ValueError()
        return mode, provider == "openai" or definition.get("requires_openai_auth", False) is True, constraints
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise RuntimeError("Cannot select persistent Team provider/auth config; inspect pira-team-worker.toml/config.toml privately (values withheld)") from None


def ensure_team_auth(tools: list[str], install_dir: Path, *, verify: bool,
                     dry_run: bool, login: str, store_root: str | None = None,
                     reauth: bool = False) -> None:
    """Verify the actual isolated file-auth mode; user login is setup-only."""
    if "pira_team" not in tools:
        return
    if reauth and (verify or login == "skip"):
        raise RuntimeError("--reauth needs normal setup login, not --verify or --codex-login skip")
    if dry_run and not verify:
        print("DRY-RUN: would verify isolated Team file-auth access and prepare setup-only login if needed")
        return
    if os.environ.get("CODEX_API_KEY"):
        if reauth:
            raise RuntimeError("Explicit CODEX_API_KEY overrides native file login; repair the selected launcher API-key configuration privately. No login was started.")
        print("OK: CODEX_API_KEY is configured for Team (model access not remotely validated)")
        return
    home = Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex").resolve()
    mode, needs_login, constraints = team_auth_configuration(home)
    if not needs_login:
        if reauth:
            raise RuntimeError("Selected worker provider does not use native OpenAI login; repair its selected provider authentication privately. No native login was started.")
        print("OK: persistent worker provider does not use native OpenAI login; Team validates selected connection/auth settings at launch")
        return
    selected = team_auth_selection(home)
    executable = selected_codex_binary(install_dir)
    if executable is None:
        raise RuntimeError("Cannot verify Team authentication without selected Codex")
    guidance = "Rerun PIRA setup/install auth repair with --codex-login browser or device; Team launch never starts login."
    environment = dict(os.environ)
    for key in ("CODEX_API_KEY", "OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"):
        environment.pop(key, None)
    auto_file = False
    if selected is None and mode == "auto" and (home / "auth.json").is_file():
        # Native auto prefers keyring. Reuse the known file only when native
        # keyring status explicitly reports no login; uncertainty is not absence.
        try:
            result = subprocess.run([executable, "-c", 'cli_auth_credentials_store="keyring"', "login", "status"],
                                    stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                    timeout=15, check=False, env=dict(environment, CODEX_HOME=str(home)))
            auto_file = result.returncode == 1 and "Not logged in" in (result.stdout + result.stderr).splitlines()
        except (OSError, subprocess.TimeoutExpired):
            pass  # Explicit separate Team file fallback, never an arbitrary stale file.
    # Auto/keyring may prefer a different account than a leftover auth.json. Never
    # treat that unselected file as Team's login. No credential search/export.
    source_home = selected or (home if mode == "file" or auto_file else home / "pira-team-auth")
    source = source_home / "auth.json"
    if (source.exists() or source.is_symlink()) and not source.is_file():
        raise RuntimeError("Selected credential path is not a regular file; preserved without login. " + guidance)
    if store_root:
        root = Path(store_root)
        if not root.exists() and verify:
            raise RuntimeError("Team store is not prepared; cannot verify credential alias mode. " + guidance)
        if not verify:
            root.mkdir(parents=True, exist_ok=True, mode=0o700)
    else:
        root = None
    def status(probe: Path) -> bool:
        env = dict(environment, CODEX_HOME=str(probe))
        try:
            result = subprocess.run([executable, "-c", 'cli_auth_credentials_store="file"', "login", "status"],
                                    stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                    timeout=15, check=False, env=env)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise RuntimeError("Could not verify isolated Team file login; no automatic re-login attempted. " + guidance) from error
        if result.returncode == 0:
            return True
        if result.returncode == 1 and "Not logged in" in (result.stdout + result.stderr).splitlines():
            return False
        raise RuntimeError("Native file login status/configuration failed; values withheld and credentials preserved. " + guidance)

    def writable_alias(probe: Path) -> None:
        # Resolve only the selected location. Native refresh replaces the target
        # atomically beside itself; copying or hard-linking is not refresh-safe.
        target = source.resolve()
        try:
            (probe / "auth.json").symlink_to(target)
            with tempfile.TemporaryDirectory(prefix=".pira-auth-write-check-", dir=target.parent) as temporary:
                a, b = Path(temporary) / "a", Path(temporary) / "b"
                a.write_bytes(b"synthetic-write-check")
                b.write_bytes(b"synthetic-replacement")
                os.replace(a, b)
            if source.exists() and not os.access(target, os.R_OK | os.W_OK):
                raise OSError("credential target is not readable/writable")
            if IS_WINDOWS and source.exists() and target.stat().st_nlink > 1:
                raise OSError("native Windows replacement rejects multi-link files")
        except OSError as error:
            raise RuntimeError("Team requires a refresh-safe credential symlink and writable native target; check filesystem support, Windows Developer Mode/symlink privilege and permissions. Another login alone cannot fix this access mode; no copy/hard-link fallback was attempted.") from error

    if not source_home.exists():
        if verify or login == "skip" or (login == "auto" and not sys.stdin.isatty()):
            raise RuntimeError("Team-compatible persistent file login is missing; authentication incomplete. " + guidance)
        source_home.mkdir(parents=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix=".pira-team-auth-check-", dir=root) as temporary:
        probe = Path(temporary)
        (probe / "config.toml").write_text('cli_auth_credentials_store="file"\n' + "".join(
            f"{key} = {json.dumps(value)}\n" for key, value in constraints.items()), encoding="utf-8")
        writable_alias(probe)  # Check platform access before asking the user to login.
        # Offline status cannot detect provider-side revocation. Only an explicit
        # setup renewal request may replace an otherwise accepted cached login.
        ready = source.exists() and not reauth and status(probe)
        if not ready:
            if verify or login == "skip" or (login == "auto" and not sys.stdin.isatty()):
                raise RuntimeError("Team-compatible persistent file login is missing; authentication incomplete. " + guidance)
            command = [executable, "-c", 'cli_auth_credentials_store="file"', "login"]
            if login == "device":
                command.append("--device-auth")
            print("Team's isolated app-server needs a native file login. The official login flow saves it in the selected Team auth home; main config and keyring are unchanged. Protect the file like a password.", flush=True)
            print("Complete Codex's browser/link-code flow; waiting up to five minutes. Ctrl-C cancels setup.", flush=True)
            try:
                result = subprocess.run(command, stdin=subprocess.DEVNULL,
                                        env=dict(environment, CODEX_HOME=str(probe)), timeout=300, check=False)
            except subprocess.TimeoutExpired as error:
                raise RuntimeError("Codex login timed out; authentication incomplete. " + guidance) from error
            except KeyboardInterrupt as error:
                raise RuntimeError("Codex login cancelled; authentication incomplete.") from error
            except OSError as error:
                raise RuntimeError("Could not launch native Codex login; authentication incomplete. " + guidance) from error
            if result.returncode != 0:
                raise RuntimeError("Codex login did not complete; authentication incomplete. " + guidance)
            if not source.is_file() or not status(probe):
                raise RuntimeError("Login returned without a Team-compatible native file cache. " + guidance)
        # File auth is explicitly selected only after the isolated access check.
        if selected is None:
            if (verify or dry_run) and (source_home != home or mode != "file"):
                raise RuntimeError("Team file-auth selection is not recorded; authentication incomplete. " + guidance)
        if selected is None and not verify and not dry_run:
            marker = home / "pira-team-auth.json"
            descriptor, name = tempfile.mkstemp(dir=home, prefix=".pira-team-auth-")
            temporary = Path(name)
            try:
                with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                    json.dump({"version": 1, "home": str(source_home.resolve())}, stream)
                    stream.write("\n")
                if marker.exists() or marker.is_symlink():
                    raise RuntimeError("Team auth selection appeared concurrently; preserved. Rerun setup.")
                os.replace(temporary, marker)
            finally:
                temporary.unlink(missing_ok=True)
        print("OK: native file login accepted; credential alias and target-directory replacement checks passed (live refresh/model access not tested)")


def check_team_remote_signing(tools: list[str]) -> None:
    """Check the existing OpenSSH client; never install it or read a real key."""
    if "pira_team" not in tools:
        return
    candidates = [shutil.which("ssh-keygen")]
    if IS_WINDOWS:
        root = os.environ.get("SystemRoot") or os.environ.get("WINDIR")
        if root:
            candidates.append(str(Path(root) / "System32/OpenSSH/ssh-keygen.exe"))
    else:
        candidates.extend(["/usr/bin/ssh-keygen", "/bin/ssh-keygen"])
    executable = next((p for p in candidates if p and Path(p).is_file()), None)
    guidance = ("Remote Team requires OpenSSH ssh-keygen with -Y sign/verify. "
                "Install/update the OS OpenSSH client package and rerun PIRA setup on both hosts; "
                "ordinary launch never installs signing dependencies.")
    if executable is None:
        raise RuntimeError(guidance)
    try:
        result = subprocess.run([executable, "-?"], stdin=subprocess.DEVNULL,
                                capture_output=True, text=True, encoding="utf-8", errors="replace",
                                timeout=10, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(guidance) from error
    usage = result.stdout + result.stderr
    if "-Y sign" not in usage or "-Y verify" not in usage:
        raise RuntimeError(guidance)
    print("OK: OpenSSH ssh-keygen signing/verification available for Team remote control")


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.reauth and (args.verify or args.codex_login == "skip"):
        raise RuntimeError("--reauth needs normal setup login, not --verify or --codex-login skip")
    use_codex = args.harness != "claude"
    initial_path = os.environ.get("PATH", "")
    install_dir = (args.install_dir or default_install_dir()).expanduser().resolve(strict=False)
    if args.auth_only:
        if not use_codex or args.tools not in (None, ["pira_team"]):
            raise RuntimeError("--auth-only prepares Codex login for pira_team; use --harness codex and --tool pira_team (or omit --tool)")
        if not args.dry_run or args.verify:
            executable = selected_codex_binary(install_dir)
            if executable is None:
                raise RuntimeError("Codex is missing; run normal PIRA setup/install first")
            check_team_runtime(["pira_team"], executable)
        ensure_team_auth(["pira_team"], install_dir, verify=args.verify, dry_run=args.dry_run,
                         login=args.codex_login,
                         store_root=stores.selected_store_paths(["pira_team"])["PIRA_TEAM_DIR"],
                         reauth=args.reauth)
        print("Auth preparation only: no PIRA tool installation, PATH update or store migration was performed")
        return 0
    if args.harness in ("claude", "both"):
        runtime = prepare_claude_runtime(verify=args.verify, dry_run=args.dry_run, install_dir=install_dir)
        if runtime:
            print(f"OK: Claude Code runtime {runtime}")
        claude_bin = claude_managed_bin(install_dir)
        if claude_bin is not None and not args.verify and not args.dry_run:
            activate_directory(claude_bin)
    if not args.no_path:
        stores.configuration_toml()
    selector = load_selector()
    index = release_index()
    platform_key = selector.current_platform()
    tools = selected_tools(index, args.tools)
    check_team_remote_signing(tools)
    requested_versions = parse_versions(args.version)
    unselected_versions = sorted(set(requested_versions) - set(tools))
    if unselected_versions:
        raise RuntimeError(
            "version specified for tool excluded by --tool: "
            + ", ".join(unselected_versions)
        )
    if use_codex:
        runtime = prepare_codex_runtime(install_dir, platform_key,
                                        verify=args.verify, dry_run=args.dry_run)
        if runtime:
            print(f"OK: selected Codex {runtime}; native API inventory and isolated strict-config stdio initialize verified (not shared daemon/model access)")
    # Preparation can install a managed backend outside PATH. Pass that exact
    # selection to retained-history preflight before publishing any store paths.
    codex_binary = selected_codex_binary(install_dir) if use_codex else None
    store_plan = (stores.plan_store_environment(tools, codex_binary=codex_binary,
                  completed_ctx_only=args.completed_ctx_only, fresh_team=args.fresh_team,
                  exclude_ctx_records=args.exclude_ctx_record)
                  if not args.no_path else stores.StorePlan())
    if "pira_team" in tools and use_codex:
        ensure_team_auth(tools, install_dir, verify=args.verify, dry_run=args.dry_run,
                         login=args.codex_login, store_root=store_plan.stores.get("PIRA_TEAM_DIR"),
                         reauth=args.reauth)
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
        tool_selection(indexes[tool_name], tool_name, platform_key,
                       team_install_dir(install_dir) if tool_name == "pira_team" else install_dir)
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
        if (use_codex and managed_codex_selected(install_dir) and not args.no_path
                and not path_is_configured(codex_bin)):
            failures.append("managed Codex directory is not configured in the user PATH")
        claude_bin = claude_managed_bin(install_dir)
        if claude_bin is not None and not args.no_path and not path_is_configured(claude_bin):
            failures.append("managed Claude directory is not configured in the user PATH")
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
                    selection.destination.parent,
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

    path_changed = False
    if not args.no_path:
        stores.apply_store_environment(store_plan, dry_run=args.dry_run)
        path_changed = ensure_path(install_dir, args.dry_run,
                    include_codex=use_codex and (managed_codex_selected(install_dir) or not codex_on_path(install_dir)))

    if not args.dry_run:
        if any(selection.name == "pira_team" for selection in selections) and team_install_dir(install_dir) != install_dir:
            activate_directory(team_install_dir(install_dir))
        restart_needed = path_changed or os.environ.get("PATH", "") != initial_path
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
