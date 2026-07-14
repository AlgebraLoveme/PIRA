#!/usr/bin/env python3
"""Select, verify, and optionally install a bundled PIRA tool.

Selection is local and deterministic. The script does not invoke an agent or a
shell, install dependencies, or build code. PIRA setup should call ``--install``
once; runtime callers should then execute the canonical installed path directly.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

TOOLS_DIR = Path(__file__).resolve().parent
BUNDLE_ROOT = TOOLS_DIR / "dist"
DEFAULT_TOOL = "pira_ctx"
DEFAULT_RUNTIME_DIR = TOOLS_DIR / "bin" / DEFAULT_TOOL

OS_ALIASES = {
    "darwin": "darwin",
    "linux": "linux",
    "win32": "windows",
    "cygwin": "windows",
    "msys": "windows",
}

ARCH_ALIASES = {
    "x86_64": "x64",
    "amd64": "x64",
    "aarch64": "arm64",
    "arm64": "arm64",
}


class SelectionError(RuntimeError):
    """Raised when no safe bundled executable can be selected."""


def validate_tool_name(tool_name: str) -> str:
    """Return a safe PIRA executable name."""
    if (
        not tool_name.isascii()
        or not tool_name.startswith("pira_")
        or not tool_name.replace("_", "").isalnum()
    ):
        raise SelectionError(f"invalid PIRA tool name: {tool_name}")
    return tool_name


def bundle_directory(tool_name: str, bundle_root: Path | None = None) -> Path:
    root = BUNDLE_ROOT if bundle_root is None else bundle_root
    return root / validate_tool_name(tool_name)


def discover_tools(bundle_root: Path | None = None) -> list[str]:
    """Return bundled tools that have a manifest, in deterministic order."""
    root = BUNDLE_ROOT if bundle_root is None else bundle_root
    if not root.is_dir():
        return []
    return sorted(
        path.name
        for path in root.iterdir()
        if path.is_dir()
        and not path.is_symlink()
        and (path / "bundle.json").is_file()
        and path.name.isascii()
        and path.name.startswith("pira_")
        and path.name.replace("_", "").isalnum()
    )


def normalize_platform(sys_platform: str, machine: str) -> str:
    """Return the canonical ``os-arch`` key for explicit platform values."""
    system = sys_platform.lower()
    os_name = next(
        (
            normalized
            for prefix, normalized in OS_ALIASES.items()
            if system == prefix or system.startswith(prefix)
        ),
        system,
    )
    architecture = ARCH_ALIASES.get(machine.lower(), machine.lower())
    return f"{os_name}-{architecture}"


def current_platform() -> str:
    """Return the canonical key for the current Python process."""
    return normalize_platform(sys.platform, platform.machine())


def load_manifest(
    path: Path | None = None,
    *,
    tool_name: str = DEFAULT_TOOL,
    bundle_root: Path | None = None,
) -> dict[str, Any]:
    tool_name = validate_tool_name(tool_name)
    path = bundle_directory(tool_name, bundle_root) / "bundle.json" if path is None else path
    if path.parent.is_symlink() or path.is_symlink():
        raise SelectionError(f"refusing symlinked {tool_name} bundle manifest: {path}")
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise SelectionError(f"{tool_name} bundle manifest is missing: {path}") from error
    except (OSError, json.JSONDecodeError) as error:
        raise SelectionError(f"cannot read {tool_name} bundle manifest {path}: {error}") from error
    if manifest.get("schema_version") != 1 or not isinstance(manifest.get("binaries"), dict):
        raise SelectionError(f"unsupported {tool_name} bundle manifest: {path}")
    recorded_name = manifest.get("tool_name")
    if recorded_name is not None and recorded_name != tool_name:
        raise SelectionError(
            f"bundle manifest tool mismatch: expected {tool_name}, found {recorded_name}"
        )
    return manifest


def select_binary(
    platform_key: str | None = None,
    *,
    tool_name: str = DEFAULT_TOOL,
    bundle_dir: Path | None = None,
    manifest: dict[str, Any] | None = None,
    verify: bool = True,
) -> tuple[Path, dict[str, Any]]:
    """Select a bundled binary and optionally verify its recorded SHA-256."""
    tool_name = validate_tool_name(tool_name)
    bundle_dir = bundle_directory(tool_name) if bundle_dir is None else bundle_dir
    if bundle_dir.is_symlink():
        raise SelectionError(f"refusing symlinked {tool_name} bundle directory: {bundle_dir}")
    manifest = (
        load_manifest(bundle_dir / "bundle.json", tool_name=tool_name)
        if manifest is None
        else manifest
    )
    if manifest.get("schema_version") != 1 or not isinstance(manifest.get("binaries"), dict):
        raise SelectionError(f"unsupported {tool_name} bundle manifest")
    recorded_name = manifest.get("tool_name")
    if recorded_name is not None and recorded_name != tool_name:
        raise SelectionError(
            f"bundle manifest tool mismatch: expected {tool_name}, found {recorded_name}"
        )
    platform_key = current_platform() if platform_key is None else platform_key
    binaries = manifest["binaries"]
    record = binaries.get(platform_key)
    if not isinstance(record, dict):
        supported = ", ".join(sorted(binaries))
        raise SelectionError(f"unsupported platform {platform_key}; supported: {supported}")

    relative = Path(str(record.get("path", "")))
    if relative.is_absolute() or ".." in relative.parts:
        raise SelectionError(f"unsafe binary path in bundle manifest for {platform_key}")
    binary = (bundle_dir / relative).resolve()
    try:
        binary.relative_to(bundle_dir.resolve())
    except ValueError as error:
        raise SelectionError(f"binary path escapes bundle directory for {platform_key}") from error
    if not binary.is_file():
        raise SelectionError(
            f"{tool_name} binary for {platform_key} is not bundled at {binary}; "
            "install a PIRA release that includes this platform binary"
        )
    if os.name != "nt" and not os.access(binary, os.X_OK):
        raise SelectionError(f"{tool_name} binary is not executable: {binary}")
    if verify:
        expected = record.get("sha256")
        if not isinstance(expected, str) or len(expected) != 64:
            raise SelectionError(f"missing SHA-256 for {platform_key} in bundle manifest")
        actual = sha256_file(binary)
        if actual != expected.lower():
            raise SelectionError(
                f"{tool_name} checksum mismatch for {platform_key}: "
                f"expected {expected}, got {actual}"
            )
    return binary, record


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def install_binary(
    binary: Path,
    record: dict[str, Any],
    install_dir: Path = DEFAULT_RUNTIME_DIR,
    *,
    tool_name: str = DEFAULT_TOOL,
) -> Path:
    """Atomically copy a verified binary to the canonical runtime directory."""
    tool_name = validate_tool_name(tool_name)
    if install_dir.is_symlink():
        raise SelectionError(f"refusing symlinked PIRA runtime directory: {install_dir}")
    try:
        install_dir.mkdir(parents=True, exist_ok=True)
    except OSError as error:
        raise SelectionError(
            f"cannot create PIRA runtime directory {install_dir}: {error}"
        ) from error
    if not install_dir.is_dir():
        raise SelectionError(f"PIRA runtime path is not a directory: {install_dir}")

    executable_name = f"{tool_name}.exe" if binary.suffix.lower() == ".exe" else tool_name
    destination = install_dir / executable_name
    temporary: Path | None = None
    try:
        with binary.open("rb") as source, tempfile.NamedTemporaryFile(
            mode="wb",
            prefix=f".{executable_name}.tmp-",
            dir=install_dir,
            delete=False,
        ) as output:
            temporary = Path(output.name)
            shutil.copyfileobj(source, output, length=1024 * 1024)
            output.flush()
            os.fsync(output.fileno())
        if os.name != "nt":
            temporary.chmod(0o755)
        expected = str(record.get("sha256", "")).lower()
        actual = sha256_file(temporary)
        if actual != expected:
            raise SelectionError(
                f"installed {tool_name} checksum mismatch: expected {expected}, got {actual}"
            )
        os.replace(temporary, destination)
        temporary = None
    except OSError as error:
        raise SelectionError(f"cannot install {tool_name} at {destination}: {error}") from error
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)

    alternate_name = tool_name if executable_name.endswith(".exe") else f"{tool_name}.exe"
    alternate = install_dir / alternate_name
    if alternate.is_symlink() or alternate.is_file():
        alternate.unlink()
    elif alternate.exists():
        raise SelectionError(f"unexpected directory at alternate runtime path: {alternate}")
    return destination.resolve()


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Select or install one bundled PIRA tool for this platform."
    )
    parser.add_argument(
        "--tool",
        default=DEFAULT_TOOL,
        help=f"bundled tool name (default: {DEFAULT_TOOL})",
    )
    output = parser.add_mutually_exclusive_group()
    output.add_argument(
        "--platform",
        "--print-platform",
        action="store_true",
        help="print only the normalized platform key",
    )
    output.add_argument("--json", action="store_true", help="print selection details as JSON")
    parser.add_argument(
        "--install",
        action="store_true",
        help="copy the selected binary to the canonical runtime directory",
    )
    parser.add_argument(
        "--install-dir",
        type=Path,
        default=None,
        help="runtime directory used with --install",
    )
    parser.add_argument("--no-verify", action="store_true", help="skip SHA-256 verification")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    tool_name = validate_tool_name(args.tool)
    key = current_platform()
    binary, record = select_binary(key, tool_name=tool_name, verify=not args.no_verify)
    source_binary = binary
    if args.install:
        install_dir = args.install_dir or (TOOLS_DIR / "bin" / tool_name)
        binary = install_binary(binary, record, install_dir, tool_name=tool_name)
    if args.platform:
        print(key)
    elif args.json:
        print(
            json.dumps(
                {
                    **record,
                    "tool": tool_name,
                    "installed": args.install,
                    "platform": key,
                    "source_path": str(source_binary),
                    "path": str(binary),
                },
                sort_keys=True,
            )
        )
    else:
        print(binary)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SelectionError as error:
        print(f"select_tool_for_platform.py: {error}", file=sys.stderr)
        raise SystemExit(1)
