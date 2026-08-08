from __future__ import annotations

import hashlib
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

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

    def test_packages_direct_assets_and_release_index(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            versions = {"pira_ctx": "1.6.0", "pira_dec": "0.5.1", "pira_nav": "0.11.0"}
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
            self.assertEqual(index["repository"], "AlgebraLoveme/PIRA")
            self.assertEqual(index["source_sha"], "a" * 40)
            self.assertEqual(set(index["tools"]), set(release.TOOLS))
            self.assertEqual(len(list(output.iterdir())), 16)
            for tool, version in versions.items():
                self.assertEqual(index["tools"][tool]["version"], version)
                for platform_key, record in index["tools"][tool]["binaries"].items():
                    asset = output / record["asset"]
                    self.assertTrue(asset.is_file())
                    self.assertEqual(release.sha256_file(asset), record["sha256"])

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
