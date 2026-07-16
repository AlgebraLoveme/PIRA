#!/usr/bin/env python3
"""Distribution, selection, and multi-tool setup regression tests."""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise AssertionError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


SELECTOR = load_module(
    "pira_distribution_selector", REPO_ROOT / "tools" / "select_tool_for_platform.py"
)
SETUP = load_module(
    "pira_distribution_setup", REPO_ROOT / "assets" / "scripts" / "setup_pira_tools.py"
)
BUILDER = load_module(
    "pira_distribution_builder",
    REPO_ROOT / "tools" / "build" / "build_pira_ctx_platform_bins.py",
)


class PiraToolDistributionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="pira-tool-dist-")
        self.root = Path(self.temporary.name)
        self.bundle_root = self.root / "dist"
        self.original_bundle_root = SELECTOR.BUNDLE_ROOT
        SELECTOR.BUNDLE_ROOT = self.bundle_root

    def tearDown(self) -> None:
        SELECTOR.BUNDLE_ROOT = self.original_bundle_root
        self.temporary.cleanup()

    def add_bundle(
        self, tool_name: str, version: str, *, executable_version: str | None = None
    ) -> None:
        platform_key = SELECTOR.current_platform()
        executable_name = f"{tool_name}.exe" if os.name == "nt" else tool_name
        binary = self.bundle_root / tool_name / platform_key / executable_name
        binary.parent.mkdir(parents=True)
        reported = executable_version or version
        binary.write_text(f"#!/bin/sh\necho '{tool_name} {reported}'\n", encoding="utf-8")
        binary.chmod(0o755)
        digest = hashlib.sha256(binary.read_bytes()).hexdigest()
        manifest = {
            "schema_version": 1,
            "tool_name": tool_name,
            "tool_version": version,
            "rust_toolchain": "test",
            "binaries": {
                platform_key: {
                    "path": f"{platform_key}/{executable_name}",
                    "target": "test-target",
                    "sha256": digest,
                }
            },
        }
        (binary.parent.parent / "bundle.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )

    def test_selector_discovers_selects_and_installs_distinct_tools(self) -> None:
        self.add_bundle("pira_ctx", "0.9.0")
        self.add_bundle("pira_codenav", "0.1.0")
        self.assertEqual(["pira_codenav", "pira_ctx"], SELECTOR.discover_tools())

        install_dir = self.root / "bin"
        for tool_name in SELECTOR.discover_tools():
            manifest = SELECTOR.load_manifest(tool_name=tool_name)
            binary, record = SELECTOR.select_binary(
                tool_name=tool_name, manifest=manifest
            )
            installed = SELECTOR.install_binary(
                binary, record, install_dir, tool_name=tool_name
            )
            self.assertEqual(tool_name, installed.stem)
            self.assertEqual(record["sha256"], hashlib.sha256(installed.read_bytes()).hexdigest())

    @unittest.skipIf(os.name == "nt", "fixture executables are POSIX shell scripts")
    def test_setup_installs_refreshes_and_verifies_all_bundled_tools(self) -> None:
        self.add_bundle("pira_ctx", "0.9.0")
        self.add_bundle("pira_codenav", "0.1.0")
        install_dir = self.root / "bin"
        original_loader = SETUP.load_selector
        SETUP.load_selector = lambda: SELECTOR
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(
                    0,
                    SETUP.main(["--install-dir", str(install_dir), "--no-path"]),
                )
                self.assertEqual(
                    0,
                    SETUP.main(
                        ["--install-dir", str(install_dir), "--no-path", "--verify"]
                    ),
                )
            ctx = install_dir / "pira_ctx"
            codenav = install_dir / "pira_codenav"
            self.assertTrue(ctx.is_file() and codenav.is_file())
            codenav_before = codenav.read_bytes()
            ctx.write_text("stale", encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(
                    0,
                    SETUP.main(
                        [
                            "--install-dir",
                            str(install_dir),
                            "--no-path",
                            "--tool",
                            "pira_ctx",
                        ]
                    ),
                )
            self.assertEqual(codenav_before, codenav.read_bytes())
            self.assertIn(b"pira_ctx 0.9.0", ctx.read_bytes())
        finally:
            SETUP.load_selector = original_loader

    @unittest.skipIf(os.name == "nt", "fixture executables are POSIX shell scripts")
    def test_setup_validates_every_bundle_before_writing(self) -> None:
        self.add_bundle("pira_ctx", "0.9.0")
        self.add_bundle("pira_codenav", "0.1.0", executable_version="9.9.9")
        install_dir = self.root / "bin"
        original_loader = SETUP.load_selector
        SETUP.load_selector = lambda: SELECTOR
        try:
            with self.assertRaisesRegex(RuntimeError, "unexpected bundled version"):
                SETUP.main(["--install-dir", str(install_dir), "--no-path"])
            self.assertFalse(install_dir.exists())
        finally:
            SETUP.load_selector = original_loader

    def test_builder_creates_first_release_manifest(self) -> None:
        BUILDER.configure_tool("pira_codenav")
        bundle = self.root / "bundle"
        target = BUILDER.TARGETS["darwin-arm64"]
        artifact = bundle / target.platform_dir / target.exe_name
        artifact.parent.mkdir(parents=True)
        artifact.write_bytes(b"first-release-artifact")

        BUILDER.update_bundle_manifest(bundle, [artifact], "1.96.1")
        manifest = json.loads((bundle / "bundle.json").read_text(encoding="utf-8"))
        self.assertEqual("pira_codenav", manifest["tool_name"])
        self.assertEqual(BUILDER.cargo_package_version(), manifest["tool_version"])
        self.assertEqual("aarch64-apple-darwin", manifest["binaries"]["darwin-arm64"]["target"])
        self.assertEqual("11.0", manifest["binaries"]["darwin-arm64"]["min_os"])
        self.assertEqual(
            hashlib.sha256(artifact.read_bytes()).hexdigest(),
            manifest["binaries"]["darwin-arm64"]["sha256"],
        )
        manifest["tool_version"] = "0.0.9"
        (bundle / "bundle.json").write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(BUILDER.BuildError, "select every platform"):
            BUILDER.validate_bundle_plan(bundle, ["darwin-arm64"])

    @unittest.skipIf(os.name == "nt", "fixture executable is a POSIX shell script")
    def test_zig_cc_shim_replaces_only_rust_target_argument(self) -> None:
        fake_zig = self.root / "zig"
        fake_zig.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n', encoding="utf-8")
        fake_zig.chmod(0o755)
        wrapper = self.root / "zig-cc"
        wrapper.write_text(
            BUILDER.zig_cc_wrapper_source(fake_zig, "aarch64-linux-musl"),
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
        result = subprocess.run(
            [
                str(wrapper),
                "--target=aarch64-unknown-linux-musl",
                "-O3",
                "--target",
                "ignored-second-form",
                "-c",
                "parser.c",
            ],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        self.assertEqual(
            ["cc", "-target", "aarch64-linux-musl", "-O3", "-c", "parser.c"],
            result.stdout.splitlines(),
        )

    def test_zig_is_required_only_for_tools_with_c_compilation(self) -> None:
        BUILDER.configure_tool("pira_ctx")
        self.assertFalse(BUILDER.uses_zig_for_target(BUILDER.TARGETS["linux-arm64"]))
        BUILDER.configure_tool("pira_codenav", uses_c_compiler=True)
        self.assertTrue(BUILDER.uses_zig_for_target(BUILDER.TARGETS["linux-arm64"]))

    def test_c_remap_flags_are_stable_and_shell_safe(self) -> None:
        source = self.root / "source tree"
        flags = BUILDER.c_remap_flags([source, source, self.root / "cargo"])
        self.assertEqual(2, len(flags))
        self.assertEqual(
            f"-ffile-prefix-map={source.resolve()}=/pira-build/c-path-0",
            flags[0],
        )
        self.assertEqual(flags, shlex.split(shlex.join(flags)))
        self.assertNotIn(
            str(self.root), " ".join(value.rsplit("=", 1)[1] for value in flags)
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
