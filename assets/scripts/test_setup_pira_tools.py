from __future__ import annotations

import gzip
import hashlib
import importlib.util
import io
import json
import sys
import subprocess
import tempfile
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("setup_pira_tools.py")
SPEC = importlib.util.spec_from_file_location("pira_tools_setup_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)


class TeamAuthTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "home"; self.home.mkdir()
        self.auth = self.home / "auth.json"
        self.env = patch.dict(setup.os.environ, {"CODEX_HOME": str(self.home), "CODEX_API_KEY": "",
                                                "OPENAI_API_KEY": "synthetic-other-key",
                                                "CODEX_ACCESS_TOKEN": "synthetic-other-token"})
        self.env.start(); self.addCleanup(self.env.stop)
        self.which = patch.object(setup.shutil, "which", return_value="/test/codex")
        self.which.start(); self.addCleanup(self.which.stop)

    def ensure(self, **kwargs) -> None:
        options = dict(verify=False, dry_run=False, login="auto")
        options.update(kwargs)
        setup.ensure_team_auth(["pira_team"], self.root, **options)

    def result(self, code=0, stderr=""):
        return subprocess.CompletedProcess([], code, "", stderr)

    def test_no_auth_actions_for_other_tools_dry_run_or_explicit_api_key(self) -> None:
        with patch.object(setup.subprocess, "run") as run:
            setup.ensure_team_auth(["pira_nav"], self.root, verify=False, dry_run=False, login="auto")
            self.ensure(dry_run=True)
            with patch.dict(setup.os.environ, {"CODEX_API_KEY": "synthetic-configured"}):
                self.ensure(verify=True)
            run.assert_not_called()
        self.assertEqual(list(self.home.iterdir()), [])

    def test_verify_and_skip_fail_missing_login_without_starting_flow(self) -> None:
        with patch.object(setup.subprocess, "run") as run:
            for options in (dict(verify=True), dict(login="skip"), dict(verify=True, dry_run=True)):
                with self.assertRaisesRegex(RuntimeError, "login is missing"):
                    self.ensure(**options)
            run.assert_not_called()
        self.assertFalse(self.auth.exists())

    def test_existing_file_cache_reused_without_leaking_status_output(self) -> None:
        self.auth.write_text("synthetic-cache")
        output = io.StringIO()
        with patch.object(setup.subprocess, "run", return_value=self.result(stderr="private-status")) as run, \
             patch.object(setup.sys, "stdout", output):
            self.ensure()
            run.assert_called_once()
            call = run.call_args
            self.assertEqual(call.args[0], ["/test/codex", "-c", 'cli_auth_credentials_store="file"', "login", "status"])
            self.assertNotIn("OPENAI_API_KEY", call.kwargs["env"])
            self.assertNotIn("CODEX_ACCESS_TOKEN", call.kwargs["env"])
            self.assertNotIn("private-status", output.getvalue())
        self.assertEqual(self.auth.read_text(), "synthetic-cache")

    def test_browser_device_auto_and_managed_binary_recheck_login(self) -> None:
        for login, tty, device in [("auto", True, False), ("auto", False, True),
                                   ("browser", False, False), ("device", True, True)]:
            with self.subTest(login=login, tty=tty):
                self.auth.unlink(missing_ok=True)
                def run(command, **kwargs):
                    if command[-1] != "status":
                        self.assertNotIn("capture_output", kwargs)
                        self.assertEqual(kwargs["timeout"], 300)
                        self.assertEqual("--device-auth" in command, device)
                        self.auth.write_text("synthetic-cache")
                    return self.result()
                with patch.object(setup.subprocess, "run", side_effect=run) as calls, \
                     patch.object(setup.sys.stdin, "isatty", return_value=tty), \
                     patch.object(setup.shutil, "which", return_value=None):
                    self.ensure(login=login)
                    self.assertEqual(calls.call_count, 2)
                    self.assertEqual(calls.call_args_list[0].args[0][0],
                                     str(setup.executable_path(self.root / setup.CODEX_PACKAGE_DIR / "bin", "codex")))

    def test_login_failures_cancellation_and_missing_cache_are_not_success(self) -> None:
        for response, message in [(self.result(1), "did not complete"),
                                  (subprocess.TimeoutExpired("codex", 300), "timed out"),
                                  (KeyboardInterrupt(), "cancelled"),
                                  (OSError("unavailable"), "Could not launch"),
                                  (self.result(), "without a Team-compatible")]:
            with self.subTest(message=message), patch.object(setup.subprocess, "run") as run:
                if isinstance(response, BaseException): run.side_effect = response
                else: run.return_value = response
                with self.assertRaisesRegex(RuntimeError, message):
                    self.ensure(login="device")
                self.assertEqual(run.call_count, 1)

    def test_status_configuration_errors_never_trigger_login_or_echo_details(self) -> None:
        self.auth.write_text("synthetic-cache")
        for response in (self.result(1, "private-error"), subprocess.TimeoutExpired("status", 15)):
            with patch.object(setup.subprocess, "run") as run:
                if isinstance(response, BaseException): run.side_effect = response
                else: run.return_value = response
                with self.assertRaises(RuntimeError) as error:
                    self.ensure()
                self.assertNotIn("private-error", str(error.exception))
                self.assertEqual(run.call_count, 1)

    def test_recognized_signed_out_status_starts_login_then_rechecks(self) -> None:
        self.auth.write_text("synthetic-cache")
        with patch.object(setup.subprocess, "run",
                          side_effect=[self.result(1, "WARNING: helper aliases unavailable\nNot logged in\n"), self.result(), self.result()]) as run:
            self.ensure(login="browser")
            self.assertEqual(run.call_count, 3)
            self.assertEqual(run.call_args_list[0].args[0][-1], "status")
            self.assertEqual(run.call_args_list[1].args[0][-1], "login")
            self.assertEqual(run.call_args_list[2].args[0][-1], "status")


class SetupPiraToolsTests(unittest.TestCase):
    def test_team_preflight_skips_other_tools_and_rejects_missing_backend(self) -> None:
        with patch.object(setup.shutil, "which", return_value=None) as which:
            self.assertIsNone(setup.check_team_runtime(["pira_ctx", "pira_nav"]))
            which.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, "requires the Codex executable"):
                setup.check_team_runtime(["pira_team"])

    def test_team_preflight_checks_version_and_required_options_without_login(self) -> None:
        def result(text: str, code: int = 0):
            return subprocess.CompletedProcess([], code, text, "")
        with patch.object(setup.shutil, "which", return_value="/test/codex"):
            for version in ("codex-cli 0.159.0", "codex-cli 0.160.1"):
                with patch.object(setup.subprocess, "run", side_effect=[
                    result(version), result("--stdio\n--strict-config")]) as run:
                    self.assertEqual(setup.check_team_runtime(["pira_team"]), version)
                    self.assertEqual([c.args[0][1:] for c in run.call_args_list],
                                     [["--version"], ["app-server", "--help"]])
                    self.assertTrue(all(c.kwargs["timeout"] == 10 for c in run.call_args_list))
            for version in ("codex-cli 0.158.9", "codex-cli 0.159.0-beta.1", "unknown"):
                with patch.object(setup.subprocess, "run", return_value=result(version)) as run:
                    with self.assertRaises(RuntimeError):
                        setup.check_team_runtime(["pira_team"])
                    self.assertEqual(run.call_count, 1)
            with patch.object(setup.subprocess, "run", side_effect=[
                result("codex-cli 0.160.0"), result("--stdio-only --strict-config")]):
                with self.assertRaisesRegex(RuntimeError, "lacks required options"):
                    setup.check_team_runtime(["pira_team"])
            for failure in (subprocess.TimeoutExpired("codex", 10), OSError("unavailable")):
                with patch.object(setup.subprocess, "run", side_effect=failure):
                    with self.assertRaisesRegex(RuntimeError, "Cannot check"):
                        setup.check_team_runtime(["pira_team"])
            with patch.object(setup.subprocess, "run", return_value=result("", 1)):
                with self.assertRaisesRegex(RuntimeError, "failed"):
                    setup.check_team_runtime(["pira_team"])

    def test_team_preflight_blocks_install_verify_and_dry_run_before_writes(self) -> None:
        index = self.index()
        index["tools"]["pira_team"] = {"version":"0.2.0", "binaries":{}}
        for mode in ([], ["--verify"], ["--dry-run"]):
            with patch.object(setup, "release_index", return_value=index), \
                 patch.object(setup, "prepare_team_runtime", side_effect=RuntimeError("backend unavailable")) as check, \
                 patch.object(setup, "download_binary") as download, \
                 patch.object(setup, "ensure_path") as path:
                with self.assertRaisesRegex(RuntimeError, "backend unavailable"):
                    setup.main(["--tool", "pira_team", *mode])
                self.assertEqual(check.call_args.args[0], ["pira_team"])
                download.assert_not_called()
                path.assert_not_called()

    def test_setup_checks_auth_before_installing_team(self) -> None:
        index = self.index()
        index["tools"]["pira_team"] = {"version": "0.2.0", "binaries": {}}
        with patch.object(setup, "release_index", return_value=index), \
             patch.object(setup, "prepare_team_runtime", return_value="codex-cli 0.159.3"), \
             patch.object(setup, "ensure_team_auth", side_effect=RuntimeError("login missing")) as auth, \
             patch.object(setup, "download_binary") as download:
            with self.assertRaisesRegex(RuntimeError, "login missing"):
                setup.main(["--tool", "pira_team", "--codex-login", "device"])
            self.assertEqual(auth.call_args.args[0], ["pira_team"])
            self.assertEqual(auth.call_args.kwargs, dict(verify=False, dry_run=False, login="device"))
            download.assert_not_called()

    def test_missing_codex_install_modes_and_existing_runtime_preservation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with patch.object(setup.shutil, "which", return_value=None), \
                 patch.object(setup, "install_codex", return_value=root / "codex") as install, \
                 patch.object(setup, "check_team_runtime", return_value="codex-cli 0.159.3") as check:
                self.assertIsNone(setup.prepare_team_runtime(["pira_nav"], root, "linux-x64", verify=False, dry_run=False))
                self.assertIsNone(setup.prepare_team_runtime(["pira_team"], root, "linux-x64", verify=False, dry_run=True))
                install.assert_not_called()
                check.assert_not_called()
                setup.prepare_team_runtime(["pira_team"], root, "linux-x64", verify=True, dry_run=False)
                install.assert_not_called()
                setup.prepare_team_runtime(["pira_team"], root, "linux-x64", verify=False, dry_run=False)
                install.assert_called_once_with(root, "linux-x64")
            with patch.object(setup.shutil, "which", return_value="/existing/codex"), \
                 patch.object(setup, "install_codex") as install, \
                 patch.object(setup, "check_team_runtime", side_effect=RuntimeError("old runtime")):
                with self.assertRaisesRegex(RuntimeError, "old runtime"):
                    setup.prepare_team_runtime(["pira_team"], root, "linux-x64", verify=False, dry_run=False)
                install.assert_not_called()

    def codex_package(self, platform: str, extra: tarfile.TarInfo | None = None) -> tuple[bytes, bytes]:
        suffix = ".exe" if platform.startswith("windows-") else ""
        files = ["codex-package.json", f"bin/codex{suffix}", f"bin/codex-code-mode-host{suffix}",
                 f"codex-path/rg{suffix}"]
        if platform.startswith("linux-"): files.append("codex-resources/bwrap")
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w:gz") as tar:
            for name in files:
                entry = tarfile.TarInfo(name); entry.size = 7; entry.mode = 0o755
                tar.addfile(entry, io.BytesIO(b"fixture"))
            if extra is not None:
                tar.addfile(extra, io.BytesIO(b"x" * extra.size))
        data = archive.getvalue()
        metadata = {"tag_name": "rust-v0.159.3", "draft": False, "prerelease": False,
                    "assets": [{"name": f"codex-package-{setup.CODEX_TARGETS[platform]}.tar.gz",
                                "size": len(data), "digest": "sha256:" + hashlib.sha256(data).hexdigest()}]}
        return data, json.dumps(metadata).encode()

    def test_codex_package_install_all_platform_layouts_and_reuse(self) -> None:
        for platform in setup.CODEX_TARGETS:
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                data, metadata = self.codex_package(platform)
                response = io.BytesIO(data); response.headers = {"Content-Length": str(len(data))}
                root = Path(tmp) / "bin"
                with patch.object(setup, "request_bytes", return_value=metadata), \
                     patch.object(setup, "urlopen", return_value=response), \
                     patch.object(setup, "check_team_runtime", return_value="codex-cli 0.159.3"):
                    installed = setup.install_codex(root, platform)
                    self.assertEqual(installed.read_bytes(), b"fixture")
                    self.assertTrue((root / setup.CODEX_PACKAGE_DIR / "codex-package.json").exists())
                    self.assertEqual(list(root.glob(".pira-codex-stage-*")), [])
                    with self.assertRaisesRegex(RuntimeError, "Refusing to overwrite"):
                        setup.install_codex(root, platform)
                    self.assertEqual(installed.read_bytes(), b"fixture")
                if platform == ("windows-x64" if setup.os.name == "nt" else "linux-x64"):
                    with patch.object(setup.shutil, "which", return_value=None), \
                         patch.object(setup, "install_codex") as install, \
                         patch.object(setup, "check_team_runtime", return_value="valid") as check:
                        setup.prepare_team_runtime(["pira_team"], root, platform, verify=False, dry_run=False)
                        install.assert_not_called()
                        check.assert_called_once_with(["pira_team"], str(installed))

    def test_codex_package_rejects_unsafe_archives_and_corrupt_downloads(self) -> None:
        entries = []
        for name in ("../escaped", "/absolute", "bin/../../escaped", "C:\\escaped", "bin/codex"):
            entries.append(tarfile.TarInfo(name))
        link = tarfile.TarInfo("linked"); link.type = tarfile.SYMTYPE; link.linkname = "../escaped"
        entries.append(link)
        for entry in [*entries, None]:
            with self.subTest(entry=entry), tempfile.TemporaryDirectory() as tmp:
                data, metadata = self.codex_package("linux-x64", entry)
                if entry is None: data = data[:-1] + bytes([data[-1] ^ 1])
                response = io.BytesIO(data); response.headers = {}
                root = Path(tmp) / "bin"
                with patch.object(setup, "request_bytes", return_value=metadata), \
                     patch.object(setup, "urlopen", return_value=response), \
                     patch.object(setup, "check_team_runtime") as check:
                    with self.assertRaises(RuntimeError): setup.install_codex(root, "linux-x64")
                    check.assert_not_called()
                    self.assertFalse((root / setup.CODEX_PACKAGE_DIR).exists())
                    self.assertFalse((root / "escaped").exists())
                    self.assertEqual(list(root.glob(".pira-codex-stage-*")), [])

    def test_codex_release_validation_and_failed_probe_do_not_publish(self) -> None:
        data, encoded = self.codex_package("linux-x64")
        original = json.loads(encoded)
        for changed in ({"prerelease": True}, {"draft": True}, {"tag_name": "rust-v0.159.3-beta"},
                        {"assets": []}, {"assets": [{**original["assets"][0], "digest": "missing"}]}):
            with tempfile.TemporaryDirectory() as tmp, \
                 patch.object(setup, "request_bytes", return_value=json.dumps({**original, **changed}).encode()), \
                 patch.object(setup, "request_to_path") as download:
                with self.assertRaises(RuntimeError): setup.install_codex(Path(tmp), "linux-x64")
                download.assert_not_called()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            response = io.BytesIO(data); response.headers = {}
            with patch.object(setup, "request_bytes", return_value=encoded), \
                 patch.object(setup, "urlopen", return_value=response), \
                 patch.object(setup, "check_team_runtime", return_value="codex-cli 0.158.0"):
                with self.assertRaisesRegex(RuntimeError, "does not match"):
                    setup.install_codex(root, "linux-x64")
            self.assertEqual(list(root.iterdir()), [])

    @unittest.skipIf(setup.os.name == "nt", "Unix PATH block")
    def test_managed_codex_path_survives_non_team_setup_and_dry_run(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            profile = root / "profile"
            binary = root / "bin" / setup.CODEX_PACKAGE_DIR / "bin" / "codex"
            binary.parent.mkdir(parents=True)
            binary.write_text("fixture")
            with patch.object(setup, "shell_profiles", return_value=[profile]):
                setup.ensure_path(root / "bin", True)
                self.assertFalse(profile.exists())
                setup.ensure_path(root / "bin", False)
                self.assertIn(setup.shell_path_line(binary.parent), profile.read_text())
                self.assertTrue(setup.path_is_configured(binary.parent))
                self.assertFalse(setup.ensure_path(root / "bin", False))

    @unittest.skipIf(setup.os.name == "nt", "POSIX shell quoting")
    def test_shell_path_line_preserves_literal_paths_and_is_idempotent(self) -> None:
        for directory in (Path("/tmp/simple/bin"), Path("/tmp/space quote' $dollar/bin")):
            script = setup.shell_path_line(directory)
            result = subprocess.run(["/bin/sh", "-c", script + "; " + script + '; printf %s "$PATH"'],
                                    env={"PATH": "/usr/bin:/bin"}, check=True, text=True, capture_output=True)
            self.assertEqual(result.stdout, str(directory) + ":/usr/bin:/bin")

    def index(self) -> dict[str, object]:
        data = b"binary"
        compressed = gzip.compress(data, mtime=0)
        record = {
            "asset": "pira_ctx-1.6.0-linux-x64.gz",
            "compression": "gzip",
            "asset_sha256": hashlib.sha256(compressed).hexdigest(),
            "asset_size": len(compressed),
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        }
        return {
            "schema_version": 2,
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
            setup.parse_versions(
                ["ctx=1.6.0", "pira_nav=0.11.0", "svg=0.1.0", "team=0.2.0"]
            ),
            {
                "pira_ctx": "1.6.0",
                "pira_nav": "0.11.0",
                "pira_svg_check": "0.1.0",
                "pira_team": "0.2.0",
            },
        )

    def test_selector_accepts_pira_svg_check_and_rejects_unsafe_names(self) -> None:
        selector = setup.load_selector()
        self.assertEqual(selector.validate_tool_name("pira_svg_check"), "pira_svg_check")
        with self.assertRaises(selector.SelectionError):
            selector.validate_tool_name("../pira_svg_check")

    def test_selector_installs_pira_svg_check_atomically(self) -> None:
        selector = setup.load_selector()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "downloaded-pira-svg-check"
            source.write_bytes(b"pira-svg-check-binary")
            record = {"sha256": hashlib.sha256(source.read_bytes()).hexdigest()}
            installed = selector.install_binary(
                source,
                record,
                root / "bin",
                tool_name="pira_svg_check",
            )
            self.assertEqual(installed.name, "pira_svg_check")
            self.assertEqual(installed.read_bytes(), source.read_bytes())

    def test_default_selection_includes_released_pira_svg_check(self) -> None:
        index = self.index()
        index["tools"]["pira_svg_check"] = {
            "version": "0.1.0",
            "binaries": {},
        }
        self.assertEqual(
            setup.selected_tools(index, None),
            ["pira_ctx", "pira_svg_check"],
        )

    def test_finds_release_containing_exact_version_asset(self) -> None:
        releases = [
            {
                "tag_name": "pira-tools-20260808-42",
                "draft": False,
                "assets": [{"name": "pira_ctx-1.6.0-linux-x64.gz"}],
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
            compressed = gzip.compress(b"binary", mtime=0)

            def download(_url: str, destination: Path, **_kwargs: object) -> None:
                destination.write_bytes(compressed)

            with patch.object(setup, "request_to_path", side_effect=download) as request:
                path = setup.download_binary(str(index["tag"]), selection, directory)
            self.assertEqual(path.read_bytes(), b"binary")
            self.assertIn("/pira-tools-20260808-42/", request.call_args.args[0])

    def test_streamed_download_verifies_asset_and_preserves_existing_file(self) -> None:
        data = b"compressed asset"

        def response() -> io.BytesIO:
            value = io.BytesIO(data)
            value.headers = {"Content-Length": str(len(data))}  # type: ignore[attr-defined]
            return value

        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "asset.gz"
            with patch.object(setup, "urlopen", return_value=response()):
                setup.request_to_path(
                    "https://example.invalid/asset.gz",
                    destination,
                    limit=1024,
                    expected_size=len(data),
                    expected_hash=hashlib.sha256(data).hexdigest(),
                )
            self.assertEqual(destination.read_bytes(), data)

            destination.write_bytes(b"keep")
            with patch.object(setup, "urlopen", return_value=response()):
                with self.assertRaises(FileExistsError):
                    setup.request_to_path(
                        "https://example.invalid/asset.gz",
                        destination,
                        limit=1024,
                        expected_size=len(data),
                        expected_hash=hashlib.sha256(data).hexdigest(),
                    )
            self.assertEqual(destination.read_bytes(), b"keep")

    def test_decompression_rejects_content_larger_than_declared(self) -> None:
        index = self.index()
        record = index["tools"]["pira_ctx"]["binaries"]["linux-x64"]
        record["size"] = len(b"binary") - 1
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            selection = setup.tool_selection(
                index, "pira_ctx", "linux-x64", directory / "install"
            )

            def download(_url: str, destination: Path, **_kwargs: object) -> None:
                destination.write_bytes(gzip.compress(b"binary", mtime=0))

            with patch.object(setup, "request_to_path", side_effect=download):
                with self.assertRaisesRegex(RuntimeError, "declared size"):
                    setup.download_binary(str(index["tag"]), selection, directory)
            self.assertFalse((directory / "pira_ctx-1.6.0-linux-x64").exists())

    def test_legacy_uncompressed_release_record_remains_supported(self) -> None:
        index = self.index()
        index["schema_version"] = 1
        record = index["tools"]["pira_ctx"]["binaries"]["linux-x64"]
        record.clear()
        record.update(
            {
                "asset": "pira_ctx-1.6.0-linux-x64",
                "sha256": hashlib.sha256(b"binary").hexdigest(),
                "size": len(b"binary"),
            }
        )
        selection = setup.tool_selection(
            index, "pira_ctx", "linux-x64", Path("install")
        )
        self.assertIsNone(selection.compression)
        self.assertEqual(selection.asset_hash, selection.expected_hash)

    def test_selection_rejects_unexpected_asset_name(self) -> None:
        index = self.index()
        index["tools"]["pira_ctx"]["binaries"]["linux-x64"]["asset"] = "other"
        with self.assertRaisesRegex(RuntimeError, "asset name"):
            setup.tool_selection(index, "pira_ctx", "linux-x64", Path("install"))


if __name__ == "__main__":
    unittest.main()
