from __future__ import annotations

import hashlib
import json
import os
import runpy
import shlex
import sys
import tempfile
import tomllib
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


class BundlePublicationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="pira-builder-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        previous = builder.TOOL_NAME, builder.USES_C_COMPILER
        self.addCleanup(builder.configure_tool, previous[0], uses_c_compiler=previous[1])
        builder.configure_tool("pira_ctx")
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.version = tomllib.loads((builder.TOOLS_DIR / "crates/pira_ctx/Cargo.toml").read_text())["package"]["version"]
        self.manifest = {"schema_version": 1, "tool_name": "pira_ctx", "tool_version": self.version,
                         "binaries": {"linux-x64": {"path": "linux-x64/pira_ctx",
                             "target": "x86_64-unknown-linux-musl", "linkage": "static-pie",
                             "sha256": hashlib.sha256(b"retained executable").hexdigest()}}}
        retained = self.bundle / "linux-x64/pira_ctx"
        retained.parent.mkdir()
        retained.write_bytes(b"retained executable")
        (self.bundle / "bundle.json").write_text(json.dumps(self.manifest))
        self.arguments = ["--bundle-dir", str(self.bundle), "--build-root", str(self.root / "build"),
                          "--rustup-root", str(self.root / "rustup"), "--zig-root", str(self.root / "zig"),
                          "--platform", "darwin-arm64", "--platform", "windows-x64"]
        self.rust = self.enterContext(patch.object(builder, "rust_tools", return_value=(self.root / "rustup", {})))
        self.enterContext(patch.object(builder, "zig_tools", return_value=None))
        self.enterContext(patch.object(builder, "prepare_toolchain"))
        self.roots = {}
        self.payloads = {}
        self.compile = self.enterContext(patch.object(builder, "build_target", side_effect=self.build))
        self.enterContext(patch("builtins.print"))

    def build(self, target, args, rustup, environment, run_root, zig):
        roots = self.roots.setdefault(target.platform_dir, [])
        roots.append(run_root)
        payload = self.payloads.get(target.platform_dir, [target.platform_dir.encode()] * 2)[len(roots) - 1]
        artifact = run_root / target.exe_name
        artifact.write_bytes(payload)
        return artifact

    def snapshot(self):
        return {path.relative_to(self.bundle): path.read_bytes() for path in self.bundle.rglob("*") if path.is_file()}

    def test_verified_partial_build_preserves_unselected_platform_and_publishes_known_bytes(self):
        self.assertEqual(builder.main(self.arguments), 0)
        manifest = json.loads((self.bundle / "bundle.json").read_text())
        self.assertEqual(manifest["binaries"]["linux-x64"], self.manifest["binaries"]["linux-x64"])
        self.assertEqual((self.bundle / "linux-x64/pira_ctx").read_bytes(), b"retained executable")
        self.assertEqual(set(manifest["binaries"]), {"darwin-arm64", "windows-x64", "linux-x64"})
        for platform, filename in (("darwin-arm64", "pira_ctx"), ("windows-x64", "pira_ctx.exe")):
            with self.subTest(platform=platform):
                self.assertEqual(len(self.roots[platform]), 2)
                self.assertEqual(len(set(self.roots[platform])), 2)
                record = manifest["binaries"][platform]
                self.assertEqual(record["path"], f"{platform}/{filename}")
                self.assertEqual(record["sha256"], hashlib.sha256(platform.encode()).hexdigest())
                self.assertEqual((self.bundle / record["path"]).read_bytes(), platform.encode())
        self.assertEqual(list((self.root / "build").iterdir()), [])

    def test_later_verification_failure_never_publishes_earlier_verified_target(self):
        before = self.snapshot()
        build_path = str(self.root / "build")
        cases = (("different-builds", [b"first", b"second"], "non-reproducible"),
                 ("utf8-path", [build_path.encode()] * 2, "embeds host path"),
                 ("utf16-path", [build_path.encode("utf-16-le")] * 2, "embeds host path"))
        for case, payloads, diagnostic in cases:
            with self.subTest(case=case):
                self.roots.clear()
                self.payloads["windows-x64"] = payloads
                with self.assertRaisesRegex(builder.BuildError, diagnostic):
                    builder.main(self.arguments)
                self.assertEqual(self.snapshot(), before)
                self.assertEqual(list((self.root / "build").iterdir()), [])

    def test_version_change_rejects_partial_build_before_toolchain_or_output_writes(self):
        self.manifest["tool_version"] = "0.0.0"
        (self.bundle / "bundle.json").write_text(json.dumps(self.manifest))
        before = self.snapshot()
        with self.assertRaisesRegex(builder.BuildError, "select every platform"):
            builder.main(self.arguments)
        self.rust.assert_not_called()
        self.compile.assert_not_called()
        self.assertEqual(self.snapshot(), before)
        self.assertFalse((self.root / "build").exists())


if __name__ == "__main__":
    unittest.main()
