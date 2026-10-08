#!/usr/bin/env python3
"""Turn verified per-tool build artifacts into GitHub Release assets."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import tarfile
from pathlib import Path, PurePosixPath
from typing import BinaryIO

TOOLS = ("pira_ctx", "pira_dec", "pira_nav", "pira_svg_check")
PLATFORMS = (
    "darwin-arm64",
    "darwin-x64",
    "linux-arm64",
    "linux-x64",
    "windows-x64",
)
MAX_BINARY_BYTES = 128 * 1024 * 1024
GZIP_LEVEL = 9


class PackageError(RuntimeError):
    """Raised when a build artifact is incomplete or unsafe."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verified_archive(root: Path, tool: str) -> Path:
    archives = sorted(root.rglob(f"{tool}-*-bundle.tar.gz"))
    if len(archives) != 1:
        raise PackageError(f"expected one {tool} archive, found {len(archives)}")
    archive = archives[0]
    checksum_path = archive.with_name(f"{archive.name}.sha256")
    try:
        fields = checksum_path.read_text(encoding="utf-8").split()
    except OSError as error:
        raise PackageError(f"cannot read archive checksum {checksum_path}: {error}") from error
    if len(fields) < 2 or fields[1].lstrip("*") != archive.name:
        raise PackageError(f"invalid archive checksum file: {checksum_path}")
    expected = fields[0].lower()
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise PackageError(f"invalid archive checksum in {checksum_path}")
    actual = sha256_file(archive)
    if actual != expected:
        raise PackageError(f"archive checksum mismatch for {archive.name}")
    return archive


def read_member_bytes(archive: tarfile.TarFile, name: str, *, limit: int) -> bytes:
    try:
        member = archive.getmember(name)
    except KeyError as error:
        raise PackageError(f"archive member is missing: {name}") from error
    if not member.isfile() or member.name != name or member.size > limit:
        raise PackageError(f"unsafe archive member: {name}")
    handle: BinaryIO | None = archive.extractfile(member)
    if handle is None:
        raise PackageError(f"cannot read archive member: {name}")
    data = handle.read(limit + 1)
    if len(data) != member.size or len(data) > limit:
        raise PackageError(f"invalid archive member size: {name}")
    return data


def validate_record(tool: str, platform_key: str, record: object) -> tuple[str, str]:
    if not isinstance(record, dict):
        raise PackageError(f"invalid {tool} record for {platform_key}")
    path = record.get("path")
    checksum = record.get("sha256")
    if not isinstance(path, str) or not isinstance(checksum, str):
        raise PackageError(f"incomplete {tool} record for {platform_key}")
    relative = PurePosixPath(path)
    expected_name = f"{tool}.exe" if platform_key.startswith("windows-") else tool
    if relative.is_absolute() or ".." in relative.parts or relative.name != expected_name:
        raise PackageError(f"unsafe {tool} binary path for {platform_key}: {path}")
    if not re.fullmatch(r"[0-9a-fA-F]{64}", checksum):
        raise PackageError(f"invalid {tool} checksum for {platform_key}")
    return path, checksum.lower()


def package_tool(archive_path: Path, output_dir: Path, tool: str) -> dict[str, object]:
    with tarfile.open(archive_path, mode="r:gz") as archive:
        manifest_bytes = read_member_bytes(
            archive, f"{tool}/bundle.json", limit=1024 * 1024
        )
        try:
            manifest = json.loads(manifest_bytes)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise PackageError(f"invalid {tool} bundle manifest: {error}") from error
        if (
            not isinstance(manifest, dict)
            or manifest.get("schema_version") != 1
            or manifest.get("tool_name") != tool
            or not isinstance(manifest.get("tool_version"), str)
            or not isinstance(manifest.get("binaries"), dict)
        ):
            raise PackageError(f"unsupported {tool} bundle manifest")
        if not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]*", manifest["tool_version"]):
            raise PackageError(f"invalid {tool} version in bundle manifest")
        binaries = manifest["binaries"]
        if set(binaries) != set(PLATFORMS):
            raise PackageError(f"{tool} bundle does not contain every supported platform")

        release_records: dict[str, object] = {}
        for platform_key in PLATFORMS:
            path, expected = validate_record(tool, platform_key, binaries[platform_key])
            data = read_member_bytes(
                archive, f"{tool}/{path}", limit=MAX_BINARY_BYTES
            )
            actual = hashlib.sha256(data).hexdigest()
            if actual != expected:
                raise PackageError(f"{tool} checksum mismatch for {platform_key}")
            suffix = ".exe" if platform_key.startswith("windows-") else ""
            binary_name = f"{tool}-{manifest['tool_version']}-{platform_key}{suffix}"
            asset_name = f"{binary_name}.gz"
            destination = output_dir / asset_name
            destination.write_bytes(gzip.compress(data, compresslevel=GZIP_LEVEL, mtime=0))
            release_records[platform_key] = {
                "asset": asset_name,
                "compression": "gzip",
                "asset_sha256": sha256_file(destination),
                "asset_size": destination.stat().st_size,
                "sha256": actual,
                "size": len(data),
            }
    return {
        "version": manifest["tool_version"],
        "binaries": release_records,
    }


def package_release(
    artifacts_dir: Path,
    output_dir: Path,
    *,
    repository: str,
    tag: str,
    source_sha: str,
) -> Path:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise PackageError(f"invalid repository: {repository}")
    if not re.fullmatch(r"pira-tools-[A-Za-z0-9_.-]+", tag):
        raise PackageError(f"invalid release tag: {tag}")
    if not re.fullmatch(r"[0-9a-fA-F]{40,64}", source_sha):
        raise PackageError("invalid source commit SHA")
    if output_dir.exists() and any(output_dir.iterdir()):
        raise PackageError(f"release output directory is not empty: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)

    tools: dict[str, object] = {}
    for tool in TOOLS:
        tools[tool] = package_tool(
            verified_archive(artifacts_dir, tool), output_dir, tool
        )
    index = {
        "schema_version": 2,
        "repository": repository,
        "tag": tag,
        "source_sha": source_sha.lower(),
        "tools": tools,
    }
    path = output_dir / "pira-tools-release.json"
    path.write_text(json.dumps(index, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--source-sha", required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    index = package_release(
        args.artifacts_dir,
        args.output_dir,
        repository=args.repository,
        tag=args.tag,
        source_sha=args.source_sha,
    )
    print(index)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (PackageError, OSError, tarfile.TarError) as error:
        print(f"package_github_release.py: {error}", file=__import__("sys").stderr)
        raise SystemExit(1)
