#!/usr/bin/env python3
"""Turn verified per-tool build artifacts into GitHub Release assets."""

from __future__ import annotations

import argparse
import copy
from functools import cache
import io
import gzip
import hashlib
import json
import re
import subprocess
import tarfile
import tomllib
from pathlib import Path, PurePosixPath
from typing import BinaryIO

TOOLS = ("pira_ctx", "pira_dec", "pira_nav", "pira_svg_check", "pira_team", "pira_write_check")
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


def valid_sha(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value) is not None


def positive_integer(value: object) -> bool:
    return type(value) is int and value > 0


def strict_json(data: str | bytes) -> object:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise PackageError(f"duplicate JSON key: {key}")
            result[key] = value
        return result
    def invalid_constant(value):
        raise PackageError(f"nonstandard JSON constant: {value}")

    try:
        return json.loads(data, object_pairs_hook=unique, parse_constant=invalid_constant)
    except (ValueError, UnicodeError) as error:
        raise PackageError(f"invalid release JSON: {error}") from error


def retained_requests(text: str) -> dict[str, dict[str, str]]:
    """Parse explicit tool version/tag/build-source pins, never infer cache eligibility."""
    requests = strict_json(text)
    if not isinstance(requests, dict) or not set(requests) <= set(TOOLS):
        raise PackageError("retained tools must be a JSON object of known tool names")
    if len(requests) == len(TOOLS):
        raise PackageError("a release must freshly build at least one tool")
    for tool, pin in requests.items():
        if (not isinstance(pin, dict) or set(pin) != {"tag", "version", "source_sha"}
                or not isinstance(pin["tag"], str)
                or not re.fullmatch(r"pira-tools-[A-Za-z0-9_.-]+", pin["tag"])
                or not isinstance(pin["version"], str)
                or not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]*", pin["version"])
                or not valid_sha(pin["source_sha"])):
            raise PackageError(f"invalid retained tag/version/source pin for {tool}")
    return requests


def github_json(repository: str, path: str) -> object:
    try:
        return strict_json(subprocess.check_output(["gh", "api", f"repos/{repository}/{path}"]))
    except subprocess.CalledProcessError as error:
        raise PackageError(f"authenticated GitHub metadata failed: {path}") from error


@cache
def github_raw_flags() -> list[str]:
    # New gh versions guard terminal escapes; older versions already return raw bytes.
    help_text = subprocess.check_output(["gh", "api", "--help"], text=True)
    return ["--allow-escape-sequences"] if "--allow-escape-sequences" in help_text else []


def github_asset(repository: str, asset_id: int, limit: int) -> bytes:
    command = ["gh", "api", *github_raw_flags(),
               f"repos/{repository}/releases/assets/{asset_id}", "-H", "Accept: application/octet-stream"]
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
        data = process.stdout.read(limit + 1)
        if len(data) > limit:
            process.kill()
            process.communicate()
            raise PackageError(f"retained asset {asset_id} exceeds size limit")
        try:
            _, diagnostic = process.communicate(timeout=180)
        except subprocess.TimeoutExpired as error:
            process.kill()
            process.communicate()
            raise PackageError(f"retained asset {asset_id} download timed out") from error
        if process.returncode:
            raise PackageError(f"retained asset {asset_id} download failed: {diagnostic.decode(errors='replace')}")
    return data


def resolve_retained(repository: str, requests: dict) -> dict:
    """Authenticate published origins and pin index/asset identities before building."""
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise PackageError("invalid retained repository")
    requests = retained_requests(json.dumps(requests))
    cache = {}

    def load(tag):
        if tag in cache:
            return cache[tag]
        release = github_json(repository, f"releases/tags/{tag}")
        if (not isinstance(release, dict) or release.get("tag_name") != tag
                or release.get("draft") is not False or release.get("prerelease") is not False
                or not positive_integer(release.get("id")) or not isinstance(release.get("assets"), list)):
            raise PackageError("retained origin must be a published stable release")
        assets = {}
        ids = set()
        for asset in release["assets"]:
            if (not isinstance(asset, dict) or not isinstance(asset.get("name"), str)
                    or asset["name"] in assets or not positive_integer(asset.get("id"))
                    or asset["id"] in ids or asset.get("state") != "uploaded"
                    or not positive_integer(asset.get("size"))
                    or asset["size"] > MAX_BINARY_BYTES
                    or not isinstance(asset.get("digest"), str)
                    or not re.fullmatch(r"sha256:[0-9a-f]{64}", asset["digest"])):
                raise PackageError("invalid/duplicate retained asset metadata")
            assets[asset["name"]] = {k: asset[k] for k in ("id", "name", "size", "digest")}
            ids.add(asset["id"])
        index_asset = assets.get("pira-tools-release.json")
        if index_asset is None or index_asset["size"] > 1024 * 1024:
            raise PackageError("missing/oversized retained release index")
        data = github_asset(repository, index_asset["id"], 1024 * 1024)
        if len(data) != index_asset["size"] or "sha256:" + hashlib.sha256(data).hexdigest() != index_asset["digest"]:
            raise PackageError("retained index digest/size disagreement")
        index = strict_json(data)
        # PIRA: retention supports schema2 gzip indexes; rebuild older raw formats.
        if (not isinstance(index, dict) or type(index.get("schema_version")) is not int or index.get("schema_version") != 2
                or index.get("repository") != repository or index.get("tag") != tag
                or not valid_sha(index.get("source_sha"))
                or release.get("target_commitish") != index["source_sha"]
                or index.get("source_sha_scope") not in (None, "assembly")
                or not isinstance(index.get("tools"), dict) or set(index["tools"]) != set(TOOLS)):
            raise PackageError("retained index repository/tag/source/schema disagreement")
        ref = github_json(repository, f"git/ref/tags/{tag}")
        obj = ref.get("object", {}) if isinstance(ref, dict) else {}
        for _ in range(8):
            if not isinstance(obj, dict) or not valid_sha(obj.get("sha")):
                raise PackageError("retained Git tag/source disagreement")
            if obj.get("type") != "tag":
                break
            tagged = github_json(repository, f"git/tags/{obj['sha']}")
            obj = tagged.get("object", {}) if isinstance(tagged, dict) else {}
        if not isinstance(obj, dict) or obj.get("type") != "commit" or obj.get("sha") != index["source_sha"]:
            raise PackageError("retained Git tag/source disagreement")
        expected_names = {"pira-tools-release.json"}
        for name, entry in index["tools"].items():
            if (not isinstance(entry, dict) or not isinstance(entry.get("version"), str)
                    or not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]*", entry["version"])
                    or not isinstance(entry.get("binaries"), dict) or set(entry["binaries"]) != set(PLATFORMS)):
                raise PackageError(f"invalid retained version/platforms for {name}")
            source = entry.get("build_source_sha", index["source_sha"] if index.get("source_sha_scope") is None else None)
            if not valid_sha(source):
                raise PackageError(f"missing/invalid per-tool build source for {name}")
            if "retained_from" not in entry and source != index["source_sha"]:
                raise PackageError(f"fresh tool source disagrees with assembly: {name}")
            if index.get("source_sha_scope") is None and (source != index["source_sha"] or "retained_from" in entry):
                raise PackageError("legacy index cannot claim mixed build origins")
            if "build_run_id" in entry and not positive_integer(entry["build_run_id"]):
                raise PackageError("invalid retained build run ID")
            for platform, record in entry["binaries"].items():
                suffix = ".exe" if platform.startswith("windows-") else ""
                filename = f"{name}-{entry['version']}-{platform}{suffix}.gz"
                if (not isinstance(record, dict) or record.get("asset") != filename
                        or record.get("compression") != "gzip"
                        or not all(isinstance(record.get(k), str) and re.fullmatch(r"[0-9a-f]{64}", record[k]) for k in ("sha256", "asset_sha256"))
                        or not all(positive_integer(record.get(k)) and record[k] <= MAX_BINARY_BYTES for k in ("size", "asset_size"))):
                    raise PackageError(f"invalid retained asset/version/platform record: {name}/{platform}")
                api_asset = assets.get(filename)
                if api_asset is None or api_asset["size"] != record["asset_size"] or api_asset["digest"] != "sha256:" + record["asset_sha256"]:
                    raise PackageError("retained asset digest/size/version disagreement")
                expected_names.add(filename)
        if set(assets) != expected_names:
            raise PackageError("retained release asset inventory disagreement")
        result = {"release_id": release["id"], "index_asset_id": index_asset["id"],
                  "index_sha256": index_asset["digest"].removeprefix("sha256:"),
                  "index": index, "assets": assets}
        cache[tag] = result
        return result

    def entry(tool, tag, seen=()):
        # PIRA: bound lineage depth; select the original build release to flatten it.
        if tag in seen or len(seen) >= 16:
            raise PackageError("cyclic/overlong retained provenance; pin original build release")
        origin = load(tag)
        record = origin["index"]["tools"][tool]
        source = record.get("build_source_sha", origin["index"]["source_sha"])
        previous = record.get("retained_from")
        if previous is not None:
            fields = {"release_id", "tag", "assembly_source_sha", "index_asset_id", "index_sha256"}
            if (not isinstance(previous, dict) or set(previous) != fields
                    or not all(positive_integer(previous[k]) for k in ("release_id", "index_asset_id"))
                    or not valid_sha(previous["assembly_source_sha"])
                    or not isinstance(previous["index_sha256"], str)
                    or not re.fullmatch(r"[0-9a-f]{64}", previous["index_sha256"])
                    or not isinstance(previous["tag"], str)
                    or not re.fullmatch(r"pira-tools-[A-Za-z0-9_.-]+", previous["tag"])):
                raise PackageError("malformed retained provenance")
            prior, prior_record, prior_source = entry(tool, previous["tag"], (*seen, tag))
            expected = {"release_id": prior["release_id"], "tag": previous["tag"],
                        "assembly_source_sha": prior["index"]["source_sha"],
                        "index_asset_id": prior["index_asset_id"], "index_sha256": prior["index_sha256"]}
            if previous != expected or source != prior_source or record["version"] != prior_record["version"]:
                raise PackageError("retained provenance source/version/identity disagreement")
            for platform in PLATFORMS:
                old = prior_record["binaries"][platform]
                current = record["binaries"][platform]
                keys = ("asset", "compression", "sha256", "size", "asset_sha256", "asset_size")
                if (any(current.get(k) != old.get(k) for k in keys)
                        or not positive_integer(current.get("origin_asset_id"))
                        or current.get("origin_asset_id") != prior["assets"][old["asset"]]["id"]):
                    raise PackageError("retained provenance asset disagreement")
        elif any("origin_asset_id" in r for r in record["binaries"].values()):
            raise PackageError("origin asset identity lacks retained provenance")
        if "build_run_id" in record:
            run = github_json(repository, f"actions/runs/{record['build_run_id']}")
            if not isinstance(run, dict) or run.get("head_sha") != source or run.get("status") != "completed" or run.get("conclusion") != "success":
                raise PackageError("retained build run/source disagreement")
        return origin, record, source

    receipts = {}
    for tool, pin in requests.items():
        origin, record, source = entry(tool, pin["tag"])
        if record["version"] != pin["version"] or source != pin["source_sha"]:
            raise PackageError(f"retained version/build-source pin disagreement: {tool}")
        receipts[tool] = {"request": pin, "release_id": origin["release_id"],
                          "assembly_source_sha": origin["index"]["source_sha"],
                          "index_asset_id": origin["index_asset_id"], "index_sha256": origin["index_sha256"],
                          "record": record, "assets": {platform: origin["assets"][record["binaries"][platform]["asset"]] for platform in PLATFORMS}}
    return receipts


def package_retained(repository: str, tool: str, receipt: dict, output: Path) -> dict:
    record = copy.deepcopy(receipt["record"])
    source = receipt["request"]["source_sha"]
    record["build_source_sha"] = source
    record["retained_from"] = {"release_id": receipt["release_id"], "tag": receipt["request"]["tag"],
                               "assembly_source_sha": receipt["assembly_source_sha"],
                               "index_asset_id": receipt["index_asset_id"], "index_sha256": receipt["index_sha256"]}
    for platform, api_asset in receipt["assets"].items():
        data = github_asset(repository, api_asset["id"], MAX_BINARY_BYTES)
        binary = record["binaries"][platform]
        if len(data) != binary["asset_size"] or hashlib.sha256(data).hexdigest() != binary["asset_sha256"]:
            raise PackageError("retained compressed asset digest/size disagreement")
        try:
            with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
                raw = compressed.read(binary["size"] + 1)
        except (OSError, EOFError) as error:
            raise PackageError("invalid retained gzip asset") from error
        if len(raw) != binary["size"] or hashlib.sha256(raw).hexdigest() != binary["sha256"]:
            raise PackageError("retained raw binary digest/size disagreement")
        binary["origin_asset_id"] = api_asset["id"]
        (output / binary["asset"]).write_bytes(data)
    return record


def package_release(
    artifacts_dir: Path,
    output_dir: Path,
    *,
    repository: str,
    tag: str,
    source_sha: str,
    retained: dict | None = None,
    build_run_id: int | None = None,
    expected_versions: dict[str, str] | None = None,
) -> Path:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise PackageError(f"invalid repository: {repository}")
    if not re.fullmatch(r"pira-tools-[A-Za-z0-9_.-]+", tag):
        raise PackageError(f"invalid release tag: {tag}")
    if not isinstance(source_sha, str) or not valid_sha(source_sha.lower()):
        raise PackageError("invalid source commit SHA")
    if build_run_id is not None and not positive_integer(build_run_id):
        raise PackageError("invalid build run ID")
    retained = {} if retained is None else retained
    if not isinstance(retained, dict) or not set(retained) < set(TOOLS):
        raise PackageError("invalid retained receipt inventory")
    try:
        requests = {tool: receipt["request"] for tool, receipt in retained.items()}
    except (KeyError, TypeError) as error:
        raise PackageError("malformed retained receipt") from error
    if retained and resolve_retained(repository, requests) != retained:
        raise PackageError("retained release changed since planning")
    for tool in retained:
        if list(artifacts_dir.rglob(f"{tool}-*-bundle.tar.gz")):
            raise PackageError(f"retained tool also has a fresh bundle: {tool}")
    if output_dir.exists() and any(output_dir.iterdir()):
        raise PackageError(f"release output directory is not empty: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)

    tools: dict[str, object] = {}
    for tool in TOOLS:
        if tool in retained:
            tools[tool] = package_retained(repository, tool, retained[tool], output_dir)
        else:
            tools[tool] = package_tool(verified_archive(artifacts_dir, tool), output_dir, tool)
            tools[tool]["build_source_sha"] = source_sha.lower()
            if build_run_id is not None:
                tools[tool]["build_run_id"] = build_run_id
    if expected_versions is not None and (
            set(expected_versions) != set(TOOLS)
            or any(tools[tool]["version"] != expected_versions[tool] for tool in TOOLS)):
        raise PackageError("packaged versions disagree with assembly source manifests")
    index = {
        "schema_version": 2,
        "repository": repository,
        "tag": tag,
        "source_sha": source_sha.lower(),
        "source_sha_scope": "assembly",
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
    parser.add_argument("--retained-manifest", type=Path)
    parser.add_argument("--build-run-id", type=int)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    retained = strict_json(args.retained_manifest.read_bytes()) if args.retained_manifest else {}
    if not isinstance(retained, dict):
        raise PackageError("retained manifest must be a JSON receipt object")
    root = Path(__file__).resolve().parents[2]
    versions = {tool: tomllib.loads((root / f"tools/crates/{tool}/Cargo.toml").read_text())["package"]["version"] for tool in TOOLS}
    index = package_release(
        args.artifacts_dir,
        args.output_dir,
        repository=args.repository,
        tag=args.tag,
        source_sha=args.source_sha,
        retained=retained,
        build_run_id=args.build_run_id,
        expected_versions=versions,
    )
    print(index)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (PackageError, OSError, tarfile.TarError) as error:
        print(f"package_github_release.py: {error}", file=__import__("sys").stderr)
        raise SystemExit(1)
