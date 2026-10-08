from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from tools.build import build_pira_ctx_platform_bins as builder


class RustToolsTests(unittest.TestCase):
    def test_configures_pira_svg_check_workspace_tool(self) -> None:
        builder.configure_tool("pira_svg_check")
        self.assertEqual(builder.TOOL_NAME, "pira_svg_check")
        self.assertEqual(builder.TARGETS["windows-x64"].exe_name, "pira_svg_check.exe")

    def test_rejects_unsafe_workspace_tool_name(self) -> None:
        with self.assertRaises(builder.BuildError):
            builder.configure_tool("../pira_svg_check")

    def test_preserves_rustup_multicall_symlink_name(self) -> None:
        if os.name == "nt":
            self.skipTest("Windows does not provide POSIX symlink executable lookup semantics")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "rustup-init"
            target.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            target.chmod(0o755)
            rustup = root / "rustup"
            rustup.symlink_to(target.name)
            args = SimpleNamespace(
                rustup_home=None,
                cargo_home=None,
                bootstrap_rustup=False,
            )

            with patch.dict(os.environ, {"PATH": str(root)}):
                selected, _ = builder.rust_tools(args)

            self.assertEqual(selected, rustup)
            self.assertTrue(selected.is_symlink())


if __name__ == "__main__":
    unittest.main()
