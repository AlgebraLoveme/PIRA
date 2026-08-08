from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("setup_pira_tools.py")
SPEC = importlib.util.spec_from_file_location("pira_tools_setup_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)


class SetupPiraToolsTests(unittest.TestCase):
    def index(self) -> dict[str, object]:
        data = b"binary"
        record = {
            "asset": "pira_ctx-1.6.0-linux-x64",
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        }
        return {
            "schema_version": 1,
            "repository": setup.RELEASE_REPOSITORY,
            "tag": "pira-tools-20260808-42",
            "source_sha": "a" * 40,
            "tools": {
                "pira_ctx": {
                    "version": "1.6.0",
                    "binaries": {"linux-x64": record},
                }
            },
        }

    def test_release_index_accepts_expected_repository(self) -> None:
        encoded = json.dumps(self.index()).encode()
        with patch.object(setup, "request_bytes", return_value=encoded):
            self.assertEqual(setup.release_index()["tag"], "pira-tools-20260808-42")

    def test_exact_release_tag_uses_immutable_release_url(self) -> None:
        encoded = json.dumps(self.index()).encode()
        with patch.object(setup, "request_bytes", return_value=encoded) as request:
            setup.release_index("pira-tools-20260808-42")
        self.assertIn(
            "/releases/download/pira-tools-20260808-42/",
            request.call_args.args[0],
        )

    def test_parses_per_tool_versions(self) -> None:
        self.assertEqual(
            setup.parse_versions(["ctx=1.6.0", "pira_nav=0.11.0"]),
            {"pira_ctx": "1.6.0", "pira_nav": "0.11.0"},
        )

    def test_finds_release_containing_exact_version_asset(self) -> None:
        releases = [
            {
                "tag_name": "pira-tools-20260808-42",
                "draft": False,
                "assets": [{"name": "pira_ctx-1.6.0-linux-x64"}],
            }
        ]
        with patch.object(
            setup, "request_bytes", return_value=json.dumps(releases).encode()
        ):
            tags = setup.release_tags_for_versions(
                {"pira_ctx": "1.6.0"}, "linux-x64"
            )
        self.assertEqual(tags, {"pira_ctx": "pira-tools-20260808-42"})

    def test_release_index_rejects_repository_substitution(self) -> None:
        index = self.index()
        index["repository"] = "attacker/PIRA"
        with patch.object(setup, "request_bytes", return_value=json.dumps(index).encode()):
            with self.assertRaisesRegex(RuntimeError, "unsupported"):
                setup.release_index()

    def test_download_uses_tag_specific_url_and_verifies_bytes(self) -> None:
        index = self.index()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            selection = setup.tool_selection(
                index, "pira_ctx", "linux-x64", directory / "install"
            )
            with patch.object(setup, "request_bytes", return_value=b"binary") as request:
                path = setup.download_binary(str(index["tag"]), selection, directory)
            self.assertEqual(path.read_bytes(), b"binary")
            self.assertIn("/pira-tools-20260808-42/", request.call_args.args[0])

    def test_selection_rejects_unexpected_asset_name(self) -> None:
        index = self.index()
        index["tools"]["pira_ctx"]["binaries"]["linux-x64"]["asset"] = "other"
        with self.assertRaisesRegex(RuntimeError, "asset name"):
            setup.tool_selection(index, "pira_ctx", "linux-x64", Path("install"))


if __name__ == "__main__":
    unittest.main()
