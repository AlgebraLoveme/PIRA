from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from tools.build import build_pira_ctx_platform_bins as builder


class RustToolsTests(unittest.TestCase):
    def test_preserves_rustup_multicall_symlink_name(self) -> None:
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
