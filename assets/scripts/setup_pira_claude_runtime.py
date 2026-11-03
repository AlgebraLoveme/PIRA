"""Prepare a supported native Claude CLI without changing auth or release channels."""
from __future__ import annotations

import json
import os
from pathlib import Path
import platform
import re
import sys
import tempfile
import time
from types import ModuleType

BASE_URL = "https://downloads.claude.ai/claude-code-releases"
PACKAGE = ".pira-claude"
SELECTION = b"managed-v1\n"
MAX_BINARY_BYTES = 512 * 1024 * 1024


def _version(text: str) -> tuple[int, ...]:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?: \(Claude Code\))?", text)
    if match is None:
        raise RuntimeError(f"Cannot parse Claude Code version: {text!r}")
    return tuple(map(int, match.groups()))


def _invoke(backend: ModuleType, executable: Path | str, argument: str,
            environment: dict[str, str], timeout: int = 10) -> str:
    try:
        result = backend.subprocess.run([str(executable), argument], stdin=backend.subprocess.DEVNULL,
                                        capture_output=True, text=True, timeout=timeout,
                                        check=False, env=environment)
    except (OSError, backend.subprocess.TimeoutExpired) as error:
        outputs = (getattr(error, "stdout", None), getattr(error, "stderr", None))
        detail = "\n".join(part.decode(errors="replace") if isinstance(part, bytes) else part
                           for part in outputs if part)
        raise RuntimeError(f"Cannot run Claude Code {argument}: {error}\n{detail}") from error
    if result.returncode:
        raise RuntimeError(f"Claude Code {argument} failed (exit {result.returncode}).\n"
                           f"{result.stdout}\n{result.stderr}")
    if argument == "update":
        if result.stdout:
            print(result.stdout, end="")
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr)
    text = result.stdout.strip()
    if argument == "--version":
        try:
            _version(text)
        except RuntimeError as error:
            raise RuntimeError(f"{error}\n{result.stderr}") from error
    return text


def _channel(config_dir: Path) -> str:
    settings = config_dir / "settings.json"
    if settings.is_symlink():
        raise RuntimeError(f"Refusing symlinked Claude settings: {settings}")
    config = json.loads(settings.read_text(encoding="utf-8")) if settings.exists() else {}
    if not isinstance(config, dict):
        raise RuntimeError("Claude settings must be a JSON object")
    channel = config.get("autoUpdatesChannel", "latest")
    if channel not in ("stable", "latest"):
        raise RuntimeError(f"Unsupported Claude autoUpdatesChannel: {channel!r}; channel was not changed")
    return channel


def _install(backend: ModuleType, install_dir: Path, config_dir: Path,
             environment: dict[str, str]) -> Path:
    """Use the same native manifest/checksum distribution as the official bootstrap."""
    channel = _channel(config_dir)
    release = backend.request_bytes(f"{BASE_URL}/{channel}", limit=128).decode("ascii").strip()
    if not re.fullmatch(r"\d+\.\d+\.\d+", release) or _version(release) < backend.TEAM_CLAUDE_MIN_VERSION:
        raise RuntimeError(f"Claude channel {channel} does not provide the required supported release: {release!r}")
    target = backend.load_selector().current_platform().replace("windows-", "win32-")
    if target.startswith("linux-") and (platform.libc_ver()[0] == "musl"
                                       or any(Path("/lib").glob("libc.musl-*.so.1"))):
        target += "-musl"
    manifest = json.loads(backend.request_bytes(f"{BASE_URL}/{release}/manifest.json", limit=2 * 1024 * 1024))
    if not isinstance(manifest, dict) or manifest.get("version") != release:
        raise RuntimeError("Claude native manifest version mismatch")
    platforms = manifest.get("platforms")
    record = platforms.get(target) if isinstance(platforms, dict) else None
    name = "claude.exe" if target.startswith("win32-") else "claude"
    if not isinstance(record, dict) or record.get("binary") != name:
        raise RuntimeError(f"Claude native manifest lacks supported platform {target}")
    checksum, size = record.get("checksum"), record.get("size")
    if (not isinstance(checksum, str) or not re.fullmatch(r"[0-9a-f]{64}", checksum)
            or type(size) is not int or not 0 < size <= MAX_BINARY_BYTES):
        raise RuntimeError("Invalid Claude native checksum or size")
    destination = install_dir / PACKAGE
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise RuntimeError(f"Refusing unsafe managed Claude directory: {destination}")
    install_dir.mkdir(parents=True, exist_ok=True)
    print(f"Preparing managed Claude Code {release} ({channel}, {target}); external installation and channel settings unchanged")
    with tempfile.TemporaryDirectory(prefix=".pira-claude-stage-", dir=install_dir) as temporary:
        package = Path(temporary) / "package"
        binary = package / "bin" / name
        binary.parent.mkdir(parents=True)
        backend.request_to_path(f"{BASE_URL}/{release}/{target}/{name}", binary,
                                limit=MAX_BINARY_BYTES, expected_size=size, expected_hash=checksum)
        binary.chmod(0o755)
        if _version(_invoke(backend, binary, "--version", environment)) != _version(release):
            raise RuntimeError("Downloaded Claude version does not match native manifest")
        (package / "selected").write_bytes(SELECTION)
        backup = destination.with_name(f"{PACKAGE}.backup-{time.time_ns()}")
        previous = destination.exists()
        if previous:
            destination.rename(backup)
        try:
            package.rename(destination)
        except OSError:
            if previous:
                backup.rename(destination)
            raise
        if previous:
            print(f"Preserved previous managed Claude installation: {backup}")
    return destination / "bin" / name


def prepare_runtime(backend: ModuleType, install_dir: Path, *, verify: bool, dry_run: bool,
                    config_dir: Path | None = None) -> str | None:
    """Keep compatible CLI/auth intact; otherwise use native update or managed fallback."""
    config_dir = config_dir or Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude").expanduser()
    environment = dict(os.environ, DISABLE_AUTOUPDATER="1", CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE="0",
                       CLAUDE_CONFIG_DIR=str(config_dir))
    package = install_dir / PACKAGE
    marker = package / "selected"
    if (package.is_symlink() or marker.is_symlink() or (package / "bin").is_symlink()
            or backend.executable_path(package / "bin", "claude").is_symlink()):
        raise RuntimeError(f"Unsafe managed Claude selection: {package}")
    managed = marker.exists()
    if managed and (marker.stat().st_size != len(SELECTION) or marker.read_bytes() != SELECTION):
        raise RuntimeError(f"Unrecognized managed Claude selection: {marker}")
    executable = (backend.executable_path(package / "bin", "claude") if managed else
                  backend.shutil.which("claude.exe" if backend.IS_WINDOWS else "claude"))
    reason = "Selected Claude CLI is missing"
    updatable = False
    if executable:
        try:
            text = _invoke(backend, executable, "--version", environment)
            if _version(text) >= backend.TEAM_CLAUDE_MIN_VERSION:
                if managed and not (verify or dry_run):
                    _activate(package)
                return text
            minimum = ".".join(map(str, backend.TEAM_CLAUDE_MIN_VERSION))
            reason = f"Claude Code >= {minimum} required; found {text}"
            updatable = True
        except RuntimeError as error:
            reason = str(error)
    if verify:
        raise RuntimeError(f"{reason}. Verify will not install or update. {backend.CLAUDE_INSTALL_GUIDE}")
    if dry_run:
        print(f"DRY-RUN: {reason}; would prepare a supported managed native Claude CLI in {package}")
        return None
    if os.environ.get("DISABLE_UPDATES", "").lower() in ("1", "true", "yes"):
        raise RuntimeError(f"{reason}. DISABLE_UPDATES prohibits automatic Claude installation/update")
    if executable and updatable and not managed:
        resolved = Path(executable).resolve()
        scripted = ("node_modules" in (part.casefold() for part in resolved.parts)
                    or resolved.suffix.lower() in (".js", ".cjs", ".mjs", ".cmd", ".bat", ".ps1"))
        try:
            with resolved.open("rb") as source:
                header = source.read(4)
            native = (header.startswith(b"MZ") or header == b"\x7fELF"
                      or header in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf",
                                    b"\xfe\xed\xfa\xce", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"))
        except OSError:
            native = False
        if native and not scripted:
            try:
                print(f"{reason}; trying native claude update without changing release channel")
                _invoke(backend, executable, "update", environment, 300)
                text = _invoke(backend, executable, "--version", environment)
                if _version(text) >= backend.TEAM_CLAUDE_MIN_VERSION:
                    return text
                reason = f"Claude Code still unsupported after claude update: {text}"
            except RuntimeError as error:
                reason = str(error)
    print(f"{reason}; using a managed native installation without replacing the external CLI")
    try:
        executable = _install(backend, install_dir, config_dir, environment)
    except (RuntimeError, OSError, ValueError) as error:
        raise RuntimeError(f"{reason}. Managed Claude preparation failed: {error}. {backend.CLAUDE_INSTALL_GUIDE}") from error
    _activate(package)
    return _invoke(backend, executable, "--version", environment)


def _activate(package: Path) -> None:
    directory = str(package / "bin")
    parts = [part for part in os.environ.get("PATH", "").split(os.pathsep) if part != directory]
    os.environ["PATH"] = os.pathsep.join([directory, *parts])
