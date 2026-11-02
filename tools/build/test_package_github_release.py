from __future__ import annotations

import hashlib
import gzip
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from collections.abc import Callable

from tools.build import package_github_release as release


class PackageGitHubReleaseTests(unittest.TestCase):
    def make_bundle(self, root: Path, tool: str, version: str) -> None:
        artifact_dir = root / f"{tool}-{version}-bundle"
        artifact_dir.mkdir()
        archive_path = artifact_dir / f"{tool}-{version}-bundle.tar.gz"
        binaries: dict[str, object] = {}
        members: dict[str, bytes] = {}
        for platform_key in release.PLATFORMS:
            filename = f"{tool}.exe" if platform_key.startswith("windows-") else tool
            relative = f"{platform_key}/{filename}"
            data = f"{tool}:{version}:{platform_key}".encode()
            members[f"{tool}/{relative}"] = data
            binaries[platform_key] = {
                "path": relative,
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        manifest = {
            "schema_version": 1,
            "tool_name": tool,
            "tool_version": version,
            "binaries": binaries,
        }
        members[f"{tool}/bundle.json"] = json.dumps(manifest).encode()
        with tarfile.open(archive_path, "w:gz") as archive:
            for name, data in members.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        digest = release.sha256_file(archive_path)
        archive_path.with_name(f"{archive_path.name}.sha256").write_text(
            f"{digest}  {archive_path.name}\n", encoding="utf-8"
        )

    def rewrite_bundle(self, root: Path, tool: str, mutate: Callable) -> None:
        """Rehash the archive so tests reach the inner manifest/binary gates."""
        archive_path = next(root.rglob(f"{tool}-*-bundle.tar.gz"))
        with tarfile.open(archive_path, "r:gz") as archive:
            members = {member.name: archive.extractfile(member).read() for member in archive}
        manifest_name = f"{tool}/bundle.json"
        manifest = json.loads(members[manifest_name])
        mutate(manifest, members)
        members[manifest_name] = json.dumps(manifest).encode()
        with tarfile.open(archive_path, "w:gz") as archive:
            for name, data in members.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        archive_path.with_name(f"{archive_path.name}.sha256").write_text(
            f"{release.sha256_file(archive_path)}  {archive_path.name}\n", encoding="utf-8"
        )

    def package(self, root: Path, output_name: str = "release") -> Path:
        return release.package_release(
            root, root / output_name, repository="AlgebraLoveme/PIRA",
            tag="pira-tools-test", source_sha="b" * 40,
        )

    def test_missing_last_tool_never_creates_publishable_index(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for tool in release.TOOLS[:-1]:
                self.make_bundle(root, tool, "1.0.0")
            with self.assertRaisesRegex(release.PackageError, "expected one"):
                self.package(root)
            self.assertFalse((root / "release/pira-tools-release.json").exists())

    def test_rejects_incomplete_mismatched_or_corrupt_inner_bundle(self) -> None:
        tool = release.TOOLS[-1]  # Earlier tools may already have compressed assets.
        platform = release.PLATFORMS[-1]

        def corrupt_binary(manifest, members):
            members[f"{tool}/{manifest['binaries'][platform]['path']}"] = b"corrupt"

        def unsafe_path(manifest, members):
            manifest["binaries"][platform]["path"] = f"../{tool}.exe"

        cases = (
            (lambda manifest, members: manifest["binaries"].pop(platform), "every supported platform"),
            (lambda manifest, members: manifest.update(tool_name="other"), "unsupported"),
            (corrupt_binary, "checksum mismatch"),
            (unsafe_path, "unsafe"),
        )
        for mutate, diagnostic in cases:
            with self.subTest(diagnostic=diagnostic), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                for name in release.TOOLS:
                    self.make_bundle(root, name, "1.0.0")
                self.rewrite_bundle(root, tool, mutate)
                with self.assertRaisesRegex(release.PackageError, diagnostic):
                    self.package(root)
                self.assertFalse((root / "release/pira-tools-release.json").exists())

    def test_duplicate_bundle_is_not_selected_as_reuse(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for tool in release.TOOLS:
                self.make_bundle(root, tool, "1.0.0")
            self.make_bundle(root, release.TOOLS[0], "0.9.0")
            with self.assertRaisesRegex(release.PackageError, "found 2"):
                self.package(root)
            self.assertFalse((root / "release/pira-tools-release.json").exists())

    def test_same_run_phase_artifacts_assemble_once_with_repeatable_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for phase, tools in (("changed", release.TOOLS[:2]), ("remaining", release.TOOLS[2:])):
                phase_root = root / phase
                phase_root.mkdir()
                for tool in tools:
                    self.make_bundle(phase_root, tool, "1.0.0")
            self.package(root, "release-one")
            self.package(root, "release-two")
            first = {p.name: p.read_bytes() for p in (root / "release-one").iterdir()}
            second = {p.name: p.read_bytes() for p in (root / "release-two").iterdir()}
            self.assertEqual(len(first), 31)
            self.assertEqual(first, second)

    def test_packages_compressed_assets_and_release_index(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            versions = {
                "pira_ctx": "1.6.0",
                "pira_dec": "0.5.1",
                "pira_nav": "0.11.0",
                "pira_svg_check": "0.1.0",
                "pira_team": "0.2.0",
                "pira_write_check": "0.1.0",
            }
            for tool, version in versions.items():
                self.make_bundle(root, tool, version)
            output = root / "release"
            index_path = release.package_release(
                root,
                output,
                repository="AlgebraLoveme/PIRA",
                tag="pira-tools-20260808-42",
                source_sha="a" * 40,
            )
            index = json.loads(index_path.read_text(encoding="utf-8"))
            self.assertEqual(index["schema_version"], 2)
            self.assertEqual(index["repository"], "AlgebraLoveme/PIRA")
            self.assertEqual(index["source_sha"], "a" * 40)
            self.assertEqual(set(index["tools"]), set(release.TOOLS))
            self.assertEqual(
                len(list(output.iterdir())),
                1 + len(release.TOOLS) * len(release.PLATFORMS),
            )
            for tool, version in versions.items():
                self.assertEqual(index["tools"][tool]["version"], version)
                for platform_key, record in index["tools"][tool]["binaries"].items():
                    asset = output / record["asset"]
                    self.assertTrue(asset.is_file())
                    self.assertTrue(asset.name.endswith(".gz"))
                    self.assertEqual(record["compression"], "gzip")
                    self.assertEqual(release.sha256_file(asset), record["asset_sha256"])
                    self.assertEqual(asset.stat().st_size, record["asset_size"])
                    data = gzip.decompress(asset.read_bytes())
                    self.assertEqual(hashlib.sha256(data).hexdigest(), record["sha256"])
                    self.assertEqual(len(data), record["size"])

    def test_rejects_changed_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for tool in release.TOOLS:
                self.make_bundle(root, tool, "1.0.0")
            archive = next(root.rglob("pira_ctx-*-bundle.tar.gz"))
            archive.write_bytes(archive.read_bytes() + b"changed")
            with self.assertRaisesRegex(release.PackageError, "checksum mismatch"):
                release.package_release(
                    root,
                    root / "release",
                    repository="AlgebraLoveme/PIRA",
                    tag="pira-tools-test",
                    source_sha="b" * 40,
                )


if __name__ == "__main__":
    unittest.main()
