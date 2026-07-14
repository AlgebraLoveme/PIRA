#!/usr/bin/env python3
"""Build reproducible binaries for one bundled PIRA tool.

The release inputs are the tracked Cargo manifest, lockfile, and Rust source.
The builder pins Rust by default, uses locked dependencies, disables incremental
compilation, normalizes locale/time/build paths, and builds every selected
target twice in independent directories. Artifacts are published only when the
two builds are byte-identical and contain none of the known host paths.

End users install prebuilt artifacts and do not need this script or Rust. A
release maintainer can run, from the repository root:

    python3 tools/build/build_pira_ctx_platform_bins.py --bootstrap-rustup

Requirements beyond rustup vary by target. Windows x64 needs
``x86_64-w64-mingw32-gcc`` (provided by Homebrew ``mingw-w64`` on macOS).
The Linux targets use Rust's bundled LLD and musl targets.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path
from urllib.request import urlopen

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_DIR = REPO_ROOT / "tools"
DEFAULT_BUNDLE_DIR = TOOLS_DIR / "dist" / "pira_ctx"
DEFAULT_BUILD_ROOT = Path(tempfile.gettempdir()) / "pira_ctx-release-build"
DEFAULT_RUSTUP_ROOT = Path(tempfile.gettempdir()) / "pira_ctx-release-rustup"
DEFAULT_TOOLCHAIN = "1.96.1"
DEFAULT_ZIG_VERSION = "0.16.0"
DEFAULT_ZIG_ROOT = Path(tempfile.gettempdir()) / f"pira-release-zig-{DEFAULT_ZIG_VERSION}"
TOOL_NAME = "pira_ctx"
USES_C_COMPILER = False

ZIG_RELEASES = {
    ("darwin", "arm64"): (
        "https://ziglang.org/download/0.16.0/zig-aarch64-macos-0.16.0.tar.xz",
        "b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489",
    ),
    ("darwin", "x86_64"): (
        "https://ziglang.org/download/0.16.0/zig-x86_64-macos-0.16.0.tar.xz",
        "0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7",
    ),
    ("linux", "arm64"): (
        "https://ziglang.org/download/0.16.0/zig-aarch64-linux-0.16.0.tar.xz",
        "ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17",
    ),
    ("linux", "x86_64"): (
        "https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz",
        "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00",
    ),
}


@dataclass(frozen=True)
class BuildTarget:
    rust_target: str
    platform_dir: str
    exe_name: str
    linker_env: str | None = None
    linker: str | None = None
    rustflags: tuple[str, ...] = ()
    deployment_target: str | None = None
    min_os: str | None = None
    linkage: str | None = None
    zig_target: str | None = None


TARGETS: dict[str, BuildTarget] = {
    "darwin-arm64": BuildTarget(
        "aarch64-apple-darwin",
        "darwin-arm64",
        "pira_ctx",
        deployment_target="11.0",
        min_os="11.0",
    ),
    "darwin-x64": BuildTarget(
        "x86_64-apple-darwin",
        "darwin-x64",
        "pira_ctx",
        deployment_target="10.12",
        min_os="10.12",
    ),
    "linux-arm64": BuildTarget(
        "aarch64-unknown-linux-musl",
        "linux-arm64",
        "pira_ctx",
        rustflags=("-C", "linker=rust-lld"),
        linkage="static",
        zig_target="aarch64-linux-musl",
    ),
    "linux-x64": BuildTarget(
        "x86_64-unknown-linux-musl",
        "linux-x64",
        "pira_ctx",
        rustflags=("-C", "linker=rust-lld"),
        linkage="static-pie",
        zig_target="x86_64-linux-musl",
    ),
    "windows-x64": BuildTarget(
        "x86_64-pc-windows-gnu",
        "windows-x64",
        "pira_ctx.exe",
        linker_env="CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER",
        linker="x86_64-w64-mingw32-gcc",
        min_os="10",
    ),
}


class BuildError(RuntimeError):
    pass


def configure_tool(name: str, *, uses_c_compiler: bool = False) -> None:
    """Select one workspace package without changing any other tool artifact."""
    global TOOL_NAME, USES_C_COMPILER, DEFAULT_BUNDLE_DIR, DEFAULT_BUILD_ROOT
    global DEFAULT_RUSTUP_ROOT, TARGETS
    if (
        not name.isascii()
        or not name.startswith("pira_")
        or not name.replace("_", "").isalnum()
    ):
        raise BuildError(f"invalid PIRA tool package name: {name}")
    TOOL_NAME = name
    USES_C_COMPILER = uses_c_compiler
    DEFAULT_BUNDLE_DIR = TOOLS_DIR / "dist" / name
    DEFAULT_BUILD_ROOT = Path(tempfile.gettempdir()) / f"{name}-release-build"
    DEFAULT_RUSTUP_ROOT = Path(tempfile.gettempdir()) / f"{name}-release-rustup"
    TARGETS = {
        key: replace(target, exe_name=f"{name}.exe" if key.startswith("windows-") else name)
        for key, target in TARGETS.items()
    }


def sh_quote(value: str) -> str:
    if all(c.isalnum() or c in "_./:=+-" for c in value):
        return value
    return "'" + value.replace("'", "'\\''") + "'"


def run(cmd: list[str], *, env: dict[str, str], cwd: Path = REPO_ROOT) -> None:
    print("+ " + " ".join(sh_quote(part) for part in cmd))
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def output(cmd: list[str], *, env: dict[str, str], cwd: Path = REPO_ROOT) -> str:
    return subprocess.check_output(cmd, cwd=cwd, env=env, text=True).strip()


def rustup_init_url() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()
    arch = {
        "arm64": "aarch64",
        "aarch64": "aarch64",
        "x86_64": "x86_64",
        "amd64": "x86_64",
    }.get(machine)
    if arch is None:
        raise BuildError(f"unsupported bootstrap host architecture: {machine}")
    if system == "darwin":
        host = f"{arch}-apple-darwin"
    elif system == "linux":
        host = f"{arch}-unknown-linux-gnu"
    elif system == "windows":
        host = f"{arch}-pc-windows-msvc"
    else:
        raise BuildError(f"unsupported bootstrap host OS: {system}")
    suffix = ".exe" if system == "windows" else ""
    return f"https://static.rust-lang.org/rustup/dist/{host}/rustup-init{suffix}"


def bootstrap_rustup(rustup_root: Path, toolchain: str) -> tuple[Path, dict[str, str]]:
    cargo_home = rustup_root / "cargo"
    rustup_home = rustup_root / "rustup"
    cargo_bin = cargo_home / "bin"
    exe = ".exe" if os.name == "nt" else ""
    rustup = cargo_bin / f"rustup{exe}"
    env = os.environ.copy()
    env["CARGO_HOME"] = str(cargo_home)
    env["RUSTUP_HOME"] = str(rustup_home)
    env["PATH"] = str(cargo_bin) + os.pathsep + env.get("PATH", "")
    if rustup.exists():
        return rustup, env

    rustup_root.mkdir(parents=True, exist_ok=True)
    init_path = rustup_root / f"rustup-init{exe}"
    print(f"Downloading official isolated rustup: {rustup_init_url()}")
    temporary = init_path.with_suffix(init_path.suffix + ".tmp")
    with urlopen(rustup_init_url(), timeout=60) as response, temporary.open("wb") as output_file:
        shutil.copyfileobj(response, output_file)
    os.replace(temporary, init_path)
    init_path.chmod(0o755)
    run(
        [
            str(init_path),
            "-y",
            "--no-modify-path",
            "--profile",
            "minimal",
            "--default-toolchain",
            toolchain,
        ],
        env=env,
    )
    return rustup, env


def host_zig_release() -> tuple[str, str]:
    system = platform.system().lower()
    machine = platform.machine().lower()
    machine = {"aarch64": "arm64", "amd64": "x86_64"}.get(machine, machine)
    release = ZIG_RELEASES.get((system, machine))
    if release is None:
        raise BuildError(f"no pinned Zig {DEFAULT_ZIG_VERSION} archive for {system}-{machine}")
    return release


def check_zig_version(zig: Path) -> None:
    try:
        version = subprocess.check_output([str(zig), "version"], text=True).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise BuildError(f"cannot run Zig compiler {zig}: {error}") from error
    if version != DEFAULT_ZIG_VERSION:
        raise BuildError(
            f"Zig {DEFAULT_ZIG_VERSION} is required for reproducible C cross-builds; found {version}"
        )


def bootstrap_zig(zig_root: Path) -> Path:
    installed = zig_root / "zig"
    if installed.is_file():
        check_zig_version(installed)
        return installed

    url, expected = host_zig_release()
    zig_root.parent.mkdir(parents=True, exist_ok=True)
    archive = zig_root.with_suffix(".tar.xz")
    temporary = archive.with_suffix(".tar.xz.tmp")
    print(f"Downloading official Zig {DEFAULT_ZIG_VERSION}: {url}")
    digest = hashlib.sha256()
    with urlopen(url, timeout=120) as response, temporary.open("wb") as output_file:
        for block in iter(lambda: response.read(1024 * 1024), b""):
            digest.update(block)
            output_file.write(block)
    actual = digest.hexdigest()
    if actual != expected:
        temporary.unlink(missing_ok=True)
        raise BuildError(f"Zig archive checksum mismatch: expected {expected}, got {actual}")
    os.replace(temporary, archive)

    extraction = Path(tempfile.mkdtemp(prefix="pira-zig-extract-", dir=zig_root.parent))
    try:
        with tarfile.open(archive, mode="r:xz") as source:
            source.extractall(extraction, filter="data")
        roots = [path for path in extraction.iterdir() if path.is_dir()]
        if len(roots) != 1 or not (roots[0] / "zig").is_file():
            raise BuildError("unexpected Zig archive layout")
        os.replace(roots[0], zig_root)
    finally:
        shutil.rmtree(extraction, ignore_errors=True)
    check_zig_version(installed)
    return installed


def zig_tools(args: argparse.Namespace, selected: list[str]) -> Path | None:
    needs_zig = any(uses_zig_for_target(TARGETS[name]) for name in selected)
    if not needs_zig:
        return None
    if args.zig:
        zig = args.zig.expanduser().resolve()
        check_zig_version(zig)
        return zig
    if not args.bootstrap_zig:
        raise BuildError(
            "Linux C cross-compilation requires Zig; pass --zig PATH or --bootstrap-zig"
        )
    return bootstrap_zig(args.zig_root.expanduser().resolve())


def uses_zig_for_target(target: BuildTarget) -> bool:
    return USES_C_COMPILER and target.zig_target is not None


def rust_tools(args: argparse.Namespace) -> tuple[Path, dict[str, str]]:
    env = os.environ.copy()
    if args.rustup_home:
        env["RUSTUP_HOME"] = str(args.rustup_home.resolve())
    if args.cargo_home:
        env["CARGO_HOME"] = str(args.cargo_home.resolve())
        env["PATH"] = str(args.cargo_home.resolve() / "bin") + os.pathsep + env.get("PATH", "")

    rustup_path = shutil.which("rustup", path=env.get("PATH"))
    if rustup_path is None:
        if not args.bootstrap_rustup:
            raise BuildError("rustup not found; install rustup or rerun with --bootstrap-rustup")
        return bootstrap_rustup(args.rustup_root.resolve(), args.toolchain)
    return Path(rustup_path).resolve(), env


def require_release_inputs() -> None:
    required = [
        TOOLS_DIR / "Cargo.toml",
        TOOLS_DIR / "Cargo.lock",
        TOOLS_DIR / "crates" / TOOL_NAME / "Cargo.toml",
        TOOLS_DIR / "src" / TOOL_NAME / "lib.rs",
        TOOLS_DIR / "src" / TOOL_NAME / "main.rs",
    ]
    missing = [path for path in required if not path.is_file()]
    if missing:
        raise BuildError(f"missing {TOOL_NAME} release input: " + ", ".join(map(str, missing)))


def source_date_epoch(env: dict[str, str]) -> str:
    configured = env.get("SOURCE_DATE_EPOCH")
    if configured:
        if not configured.isdigit():
            raise BuildError("SOURCE_DATE_EPOCH must be an integer Unix timestamp")
        return configured
    try:
        return output(["git", "log", "-1", "--format=%ct"], env=env)
    except (FileNotFoundError, subprocess.CalledProcessError):
        return "0"


def prepare_toolchain(
    selected: list[str], args: argparse.Namespace, rustup: Path, env: dict[str, str]
) -> None:
    run(
        [str(rustup), "toolchain", "install", args.toolchain, "--profile", "minimal"],
        env=env,
    )
    for rust_target in sorted({TARGETS[name].rust_target for name in selected}):
        run(
            [
                str(rustup),
                "target",
                "add",
                rust_target,
                "--toolchain",
                args.toolchain,
            ],
            env=env,
        )


def remap_flags(paths: list[Path]) -> list[str]:
    flags: list[str] = []
    seen: set[str] = set()
    for index, path in enumerate(paths):
        value = str(path.resolve())
        if value in seen:
            continue
        seen.add(value)
        flags.append(f"--remap-path-prefix={value}=/pira-build/path-{index}")
    return flags


def c_remap_flags(paths: list[Path]) -> list[str]:
    """Keep C compiler diagnostics and __FILE__ independent of host paths."""
    flags: list[str] = []
    seen: set[str] = set()
    for index, path in enumerate(paths):
        value = str(path.resolve())
        if value in seen:
            continue
        seen.add(value)
        flags.append(f"-ffile-prefix-map={value}=/pira-build/c-path-{index}")
    return flags


def zig_cc_wrapper_source(zig: Path, zig_target: str) -> str:
    """Return a shim that replaces cc-rs's Rust triple with Zig's target syntax."""
    return (
        "#!/usr/bin/env python3\n"
        "import os\n"
        "import sys\n"
        f"zig = {str(zig)!r}\n"
        "args = []\n"
        "skip = False\n"
        "for arg in sys.argv[1:]:\n"
        "    if skip:\n"
        "        skip = False\n"
        "    elif arg == '--target':\n"
        "        skip = True\n"
        "    elif not arg.startswith('--target='):\n"
        "        args.append(arg)\n"
        f"os.execv(zig, [zig, 'cc', '-target', {zig_target!r}, *args])\n"
    )


def deterministic_env(
    base_env: dict[str, str],
    target: BuildTarget,
    run_root: Path,
    args: argparse.Namespace,
    rustup: Path,
    zig: Path | None,
) -> dict[str, str]:
    env = base_env.copy()
    env.update(
        {
            "CARGO_INCREMENTAL": "0",
            "LC_ALL": "C",
            "LANG": "C",
            "TZ": "UTC",
            "SOURCE_DATE_EPOCH": source_date_epoch(base_env),
        }
    )
    if target.linker_env and target.linker:
        linker = shutil.which(target.linker, path=env.get("PATH"))
        if linker is None:
            raise BuildError(f"missing linker for {target.platform_dir}: {target.linker}")
        env[target.linker_env] = str(Path(linker).resolve())
    if target.deployment_target:
        env["MACOSX_DEPLOYMENT_TARGET"] = target.deployment_target
    if uses_zig_for_target(target):
        if zig is None:
            raise BuildError(f"Zig is required for target {target.rust_target}")
        zig_target = target.zig_target
        if zig_target is None:
            raise BuildError(f"missing Zig target for {target.rust_target}")
        cc_wrapper = run_root / "zig-cc"
        ar_wrapper = run_root / "zig-ar"
        cc_wrapper.write_text(
            zig_cc_wrapper_source(zig, zig_target),
            encoding="utf-8",
        )
        ar_wrapper.write_text(
            f"#!/bin/sh\nexec {sh_quote(str(zig))} ar \"$@\"\n", encoding="utf-8"
        )
        cc_wrapper.chmod(0o755)
        ar_wrapper.chmod(0o755)
        target_key = target.rust_target.replace("-", "_")
        env[f"CC_{target_key}"] = str(cc_wrapper)
        env[f"AR_{target_key}"] = str(ar_wrapper)
        env["ZIG_GLOBAL_CACHE_DIR"] = str(run_root / "zig-global-cache")
        env["ZIG_LOCAL_CACHE_DIR"] = str(run_root / "zig-local-cache")

    sysroot = Path(
        output(
            [str(rustup), "run", args.toolchain, "rustc", "--print", "sysroot"],
            env=base_env,
        )
    )
    path_inputs = [REPO_ROOT, run_root, args.build_root, args.rustup_root, sysroot]
    for name in ("CARGO_HOME", "RUSTUP_HOME"):
        if env.get(name):
            path_inputs.append(Path(env[name]))
    if env.get("HOME"):
        home = Path(env["HOME"])
        path_inputs.extend((home / ".cargo", home / ".rustup"))
    flags = [*target.rustflags, *remap_flags(path_inputs)]
    env["RUSTFLAGS"] = " ".join(flags)
    if USES_C_COMPILER:
        target_key = target.rust_target.replace("-", "_")
        env[f"CFLAGS_{target_key}"] = shlex.join(c_remap_flags(path_inputs))
    return env


def build_target(
    target: BuildTarget,
    args: argparse.Namespace,
    rustup: Path,
    base_env: dict[str, str],
    run_root: Path,
    zig: Path | None,
) -> Path:
    env = deterministic_env(base_env, target, run_root, args, rustup, zig)
    run(
        [
            str(rustup),
            "run",
            args.toolchain,
            "cargo",
            "build",
            "--manifest-path",
            str(TOOLS_DIR / "Cargo.toml"),
            "--package",
            TOOL_NAME,
            "--release",
            "--locked",
            "--target",
            target.rust_target,
            "--target-dir",
            str(run_root),
        ],
        env=env,
    )
    artifact = run_root / target.rust_target / "release" / target.exe_name
    if not artifact.is_file():
        raise BuildError(f"expected build output missing: {artifact}")
    return artifact


def forbidden_host_paths(
    args: argparse.Namespace, base_env: dict[str, str], run_roots: list[Path]
) -> set[str]:
    paths = {
        str(REPO_ROOT.resolve()),
        str(args.build_root.resolve()),
        str(args.rustup_root.resolve()),
    }
    paths.update(str(path.resolve()) for path in run_roots)
    for name in ("HOME", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR"):
        if base_env.get(name):
            paths.add(str(Path(base_env[name]).resolve()))
    return {path for path in paths if len(path) > 1}


def assert_no_host_paths(artifact: Path, forbidden: set[str]) -> None:
    data = artifact.read_bytes()
    leaks = [
        path
        for path in sorted(forbidden)
        if path.encode() in data or path.encode("utf-16-le") in data
    ]
    if leaks:
        raise BuildError(f"{artifact} embeds host path(s): {', '.join(leaks)}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def publish_artifact(source: Path, target: BuildTarget, bundle_dir: Path) -> Path:
    destination_dir = bundle_dir / target.platform_dir
    destination_dir.mkdir(parents=True, exist_ok=True)
    destination = destination_dir / target.exe_name
    temporary = destination.with_name(destination.name + ".tmp")
    shutil.copyfile(source, temporary)
    if os.name != "nt":
        temporary.chmod(0o755)
    os.replace(temporary, destination)
    return destination


def manifest_record(target: BuildTarget, digest: str) -> dict[str, str]:
    record = {
        "path": f"{target.platform_dir}/{target.exe_name}",
        "target": target.rust_target,
        "sha256": digest,
    }
    if target.min_os:
        record["min_os"] = target.min_os
    if target.linkage:
        record["linkage"] = target.linkage
    return record


def update_bundle_manifest(bundle_dir: Path, built: list[Path], toolchain: str) -> None:
    manifest_path = bundle_dir / "bundle.json"
    if manifest_path.exists():
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BuildError(f"cannot read bundle manifest {manifest_path}: {error}") from error
        if manifest.get("schema_version") != 1 or not isinstance(
            manifest.get("binaries"), dict
        ):
            raise BuildError(f"unsupported bundle manifest: {manifest_path}")
        recorded_name = manifest.get("tool_name")
        if recorded_name is not None and recorded_name != TOOL_NAME:
            raise BuildError(
                f"bundle manifest tool mismatch: expected {TOOL_NAME}, found {recorded_name}"
            )
    else:
        manifest = {"schema_version": 1, "binaries": {}}
    cargo_manifest = tomllib.loads(
        (TOOLS_DIR / "crates" / TOOL_NAME / "Cargo.toml").read_text(encoding="utf-8")
    )
    new_version = cargo_manifest["package"]["version"]
    built_platforms = {path.parent.name for path in built}
    if (
        manifest.get("tool_version") not in (None, new_version)
        and built_platforms != set(TARGETS)
    ):
        raise BuildError(
            f"{TOOL_NAME} version changed from {manifest.get('tool_version')} to {new_version}; "
            "rebuild every platform so one manifest never describes mixed versions"
        )
    manifest["tool_name"] = TOOL_NAME
    manifest["tool_version"] = new_version
    manifest["rust_toolchain"] = toolchain
    for path in built:
        platform_name = path.parent.name
        target = TARGETS.get(platform_name)
        if target is None:
            raise BuildError(f"unexpected artifact platform directory: {platform_name}")
        manifest["binaries"][platform_name] = manifest_record(target, sha256(path))
    bundle_dir.mkdir(parents=True, exist_ok=True)
    temporary = manifest_path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, manifest_path)


def validate_bundle_plan(bundle_dir: Path, selected: list[str]) -> None:
    """Reject a partial rebuild that would mix tool versions in one bundle."""
    manifest_path = bundle_dir / "bundle.json"
    if not manifest_path.exists():
        return
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        cargo_manifest = tomllib.loads(
            (TOOLS_DIR / "crates" / TOOL_NAME / "Cargo.toml").read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        raise BuildError(f"cannot validate release metadata: {error}") from error
    if manifest.get("schema_version") != 1 or not isinstance(manifest.get("binaries"), dict):
        raise BuildError(f"unsupported bundle manifest: {manifest_path}")
    recorded_name = manifest.get("tool_name")
    if recorded_name is not None and recorded_name != TOOL_NAME:
        raise BuildError(
            f"bundle manifest tool mismatch: expected {TOOL_NAME}, found {recorded_name}"
        )
    new_version = cargo_manifest["package"]["version"]
    if manifest.get("tool_version") != new_version and set(selected) != set(TARGETS):
        raise BuildError(
            f"{TOOL_NAME} version changed from {manifest.get('tool_version')} to {new_version}; "
            "select every platform for this release"
        )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=f"Reproducibly build bundled {TOOL_NAME} binaries for supported platforms."
    )
    parser.add_argument(
        "--toolchain",
        default=DEFAULT_TOOLCHAIN,
        help=f"exact rustup toolchain/version (default: {DEFAULT_TOOLCHAIN})",
    )
    parser.add_argument(
        "--platform",
        action="append",
        choices=sorted(TARGETS),
        help="platform to build; repeatable; default: all",
    )
    parser.add_argument(
        "--bundle-dir", type=Path, default=DEFAULT_BUNDLE_DIR, help="output bundle directory"
    )
    parser.add_argument(
        "--build-root", type=Path, default=DEFAULT_BUILD_ROOT, help="temporary build parent"
    )
    parser.add_argument(
        "--rustup-root",
        type=Path,
        default=DEFAULT_RUSTUP_ROOT,
        help="isolated rustup root used when bootstrapping",
    )
    parser.add_argument("--rustup-home", type=Path, help="existing RUSTUP_HOME")
    parser.add_argument("--cargo-home", type=Path, help="existing CARGO_HOME")
    parser.add_argument(
        "--zig",
        type=Path,
        help=f"existing Zig {DEFAULT_ZIG_VERSION} executable for Linux C cross-builds",
    )
    parser.add_argument(
        "--zig-root",
        type=Path,
        default=DEFAULT_ZIG_ROOT,
        help="isolated directory used by --bootstrap-zig",
    )
    parser.add_argument(
        "--bootstrap-zig",
        action="store_true",
        help=f"download pinned official Zig {DEFAULT_ZIG_VERSION} when required",
    )
    parser.add_argument(
        "--bootstrap-rustup",
        action="store_true",
        help="install official isolated rustup when rustup is absent",
    )
    parser.add_argument(
        "--skip-reproducibility-check",
        action="store_true",
        help="build once instead of requiring two byte-identical builds (not for releases)",
    )
    parser.add_argument(
        "--keep-build-roots", action="store_true", help="retain temporary Cargo target directories"
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    args.build_root = args.build_root.resolve()
    args.rustup_root = args.rustup_root.resolve()
    args.bundle_dir = args.bundle_dir.resolve()
    require_release_inputs()
    selected = args.platform or sorted(TARGETS)
    validate_bundle_plan(args.bundle_dir, selected)
    rustup, base_env = rust_tools(args)
    zig = zig_tools(args, selected)
    prepare_toolchain(selected, args, rustup, base_env)
    args.build_root.mkdir(parents=True, exist_ok=True)

    run_roots: list[Path] = []
    staging_root = Path(tempfile.mkdtemp(prefix="verified-", dir=args.build_root))
    run_roots.append(staging_root)
    verified: list[tuple[Path, BuildTarget]] = []
    try:
        for name in selected:
            target = TARGETS[name]
            print(f"\n=== {name} ({target.rust_target}) ===")
            first_root = Path(tempfile.mkdtemp(prefix=f"{name}-a-", dir=args.build_root))
            run_roots.append(first_root)
            first = build_target(target, args, rustup, base_env, first_root, zig)
            platform_roots = [first_root]

            if not args.skip_reproducibility_check:
                second_root = Path(tempfile.mkdtemp(prefix=f"{name}-b-", dir=args.build_root))
                run_roots.append(second_root)
                platform_roots.append(second_root)
                second = build_target(target, args, rustup, base_env, second_root, zig)
                first_hash, second_hash = sha256(first), sha256(second)
                if first_hash != second_hash:
                    raise BuildError(
                        f"non-reproducible {name}: first={first_hash}, second={second_hash}"
                    )
                print(f"reproducible: {first_hash}")

            forbidden = forbidden_host_paths(args, base_env, run_roots)
            assert_no_host_paths(first, forbidden)
            staged = publish_artifact(first, target, staging_root)
            verified.append((staged, target))
            if not args.keep_build_roots:
                for path in platform_roots:
                    shutil.rmtree(path, ignore_errors=True)

        published = [
            publish_artifact(source, target, args.bundle_dir) for source, target in verified
        ]
        update_bundle_manifest(args.bundle_dir, published, args.toolchain)
        print("\nPublished binaries:")
        for path in published:
            try:
                displayed = path.relative_to(REPO_ROOT)
            except ValueError:
                displayed = path
            print(f"{sha256(path)}  {displayed}")
        return 0
    finally:
        if not args.keep_build_roots:
            for path in run_roots:
                shutil.rmtree(path, ignore_errors=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (BuildError, subprocess.CalledProcessError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(1)
