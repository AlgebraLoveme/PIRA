from __future__ import annotations

import os
import runpy
import shlex
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from tools.build import build_pira_ctx_platform_bins as builder


class RustToolsTests(unittest.TestCase):
    def test_team_entrypoint_enables_c_compiler_for_all_targets(self) -> None:
        self.addCleanup(builder.configure_tool, "pira_ctx")
        script = Path(__file__).with_name("build_pira_team_platform_bins.py")
        with patch.dict(sys.modules, {"build_pira_ctx_platform_bins": builder}), \
                patch.object(builder, "main", return_value=0) as main:
            with self.assertRaises(SystemExit) as result:
                runpy.run_path(str(script), run_name="__main__")
        self.assertEqual(result.exception.code, 0)
        main.assert_called_once_with()
        self.assertEqual(builder.TOOL_NAME, "pira_team")
        self.assertTrue(builder.USES_C_COMPILER)
        for name, target in builder.TARGETS.items():
            with self.subTest(platform=name):
                self.assertEqual(target.exe_name, "pira_team.exe" if name.startswith("windows-") else "pira_team")
                self.assertEqual(builder.uses_zig_for_target(target), name.startswith("linux-"))

    def test_team_linux_requires_or_bootstraps_pinned_zig(self) -> None:
        self.addCleanup(builder.configure_tool, "pira_ctx")
        builder.configure_tool("pira_team", uses_c_compiler=True)
        args = SimpleNamespace(zig=None, bootstrap_zig=False, zig_root=Path("/unused/zig"))
        for name in ("linux-arm64", "linux-x64"):
            with self.subTest(platform=name):
                with self.assertRaisesRegex(builder.BuildError, "Linux C cross-compilation requires Zig"):
                    builder.zig_tools(args, [name])
        args.bootstrap_zig = True
        with patch.object(builder, "bootstrap_zig", return_value=Path("/pinned/zig")) as bootstrap:
            self.assertEqual(builder.zig_tools(args, list(builder.TARGETS)), Path("/pinned/zig"))
        bootstrap.assert_called_once_with(args.zig_root.resolve())
        for name in ("darwin-arm64", "darwin-x64", "windows-x64"):
            with self.subTest(platform=name):
                self.assertIsNone(builder.zig_tools(args, [name]))

    def test_team_c_environment_preserves_each_target_toolchain(self) -> None:
        self.addCleanup(builder.configure_tool, "pira_ctx")
        builder.configure_tool("pira_team", uses_c_compiler=True)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            args = SimpleNamespace(build_root=root / "build", rustup_root=root / "rustup", toolchain="1.96.1")
            for name, target in builder.TARGETS.items():
                with self.subTest(platform=name):
                    run_root = root / name
                    run_root.mkdir()
                    with patch.object(builder, "output", return_value=str(root / "sysroot")), \
                            patch.object(builder.shutil, "which", return_value=str(root / "mingw-gcc")):
                        env = builder.deterministic_env(
                            {"PATH": "unused"}, target, run_root, args, root / "rustup-bin", root / "zig")
                    key = target.rust_target.replace("-", "_")
                    self.assertEqual(shlex.split(env[f"CFLAGS_{key}"]),
                                     builder.c_remap_flags([builder.REPO_ROOT, run_root, args.build_root,
                                                           args.rustup_root, root / "sysroot"]))
                    self.assertEqual(env["SOURCE_DATE_EPOCH"], builder.DEFAULT_SOURCE_DATE_EPOCH)
                    if name.startswith("linux-"):
                        self.assertEqual(env[f"CC_{key}"], str(run_root / "zig-cc"))
                        self.assertEqual(env[f"AR_{key}"], str(run_root / "zig-ar"))
                        self.assertIn(repr(target.zig_target), (run_root / "zig-cc").read_text())
                        self.assertIn("linker=rust-lld", env["RUSTFLAGS"])
                    else:
                        self.assertNotIn(f"CC_{key}", env)
                        self.assertNotIn(f"AR_{key}", env)
                    if name.startswith("darwin-"):
                        self.assertEqual(env["MACOSX_DEPLOYMENT_TARGET"], target.deployment_target)
                    if name == "windows-x64":
                        self.assertEqual(env[target.linker_env], str(root / "mingw-gcc"))

    def test_release_workflow_bootstraps_zig_for_nav_and_team_only(self) -> None:
        workflow = builder.REPO_ROOT / ".github/workflows/build-pira-tool-bundle.yml"
        text = workflow.read_text()
        setting = next(line.strip() for line in text.splitlines() if line.strip().startswith("NEEDS_ZIG:"))
        self.assertEqual(setting, "NEEDS_ZIG: ${{ inputs.tool == 'pira_nav' || inputs.tool == 'pira_team' }}")
        self.assertIn('if [[ "${NEEDS_ZIG}" == "true" ]]; then\n            args+=(--bootstrap-zig)\n          fi', text)
        self.assertIn('python3 "${BUILD_SCRIPT}" "${args[@]}"', text)

    def test_configures_pira_svg_check_workspace_tool(self) -> None:
        builder.configure_tool("pira_svg_check")
        self.assertEqual(builder.TOOL_NAME, "pira_svg_check")
        self.assertEqual(builder.TARGETS["windows-x64"].exe_name, "pira_svg_check.exe")

    def test_configures_write_checker_and_embedded_inventory(self) -> None:
        builder.configure_tool("pira_write_check")
        self.assertEqual(builder.TOOL_NAME, "pira_write_check")
        self.assertEqual(builder.TARGETS["windows-x64"].exe_name, "pira_write_check.exe")
        self.assertFalse(builder.USES_C_COMPILER)
        builder.require_release_inputs()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "crates/pira_write_check/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text('[workspace.metadata]\nversion = "9.9.9"\n'
                                '[package]\nname = "pira_write_check"\nversion = "1.2.3"\n',
                                encoding="utf-8")
            with patch.object(builder, "TOOLS_DIR", root):
                self.assertEqual(builder.cargo_package_version(), "1.2.3")
        self.assertTrue((builder.TOOLS_DIR / "src/pira_write_check/lexicon.json").is_file())

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
