from __future__ import annotations

import gzip
import hashlib
import importlib.util
import io
import json
import os
import shlex
import runpy
import sys
import subprocess
import tempfile
import tarfile
import time
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

SCRIPT = Path(__file__).with_name("setup_pira_tools.py")
sys.path.insert(0, str(SCRIPT.parent.resolve()))
SPEC = importlib.util.spec_from_file_location("pira_tools_setup_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)


class WindowsCodexSearchTests(unittest.TestCase):
    def test_searchpath_uses_exact_native_image_name_and_preserves_errors(self):
        import ctypes
        native = str(Path(tempfile.gettempdir()) / "native" / "codex.exe")
        search = MagicMock()
        def found(path, name, extension, capacity, buffer, file_part):
            self.assertEqual((path, name, extension, capacity, file_part),
                             (None, "codex.exe", None, 32768, None))
            buffer.value = native
            return len(native)
        search.side_effect = found
        library = MagicMock(SearchPathW=search)
        with patch.object(ctypes, "WinDLL", return_value=library, create=True) as loader:
            self.assertEqual(setup.windows_codex_binary(), native)
            loader.assert_called_once_with("kernel32", use_last_error=True)
            search.side_effect = None
            search.return_value = 0
            for code in (2, 3):
                with self.subTest(error=code), patch.object(ctypes, "get_last_error", return_value=code, create=True):
                    self.assertIsNone(setup.windows_codex_binary())
            with patch.object(ctypes, "get_last_error", return_value=5, create=True), \
                 patch.object(ctypes, "WinError", return_value=OSError(5, "native access denied"), create=True), \
                 self.assertRaisesRegex(OSError, "native access denied"):
                setup.windows_codex_binary()


class TeamInitializeProcessTests(unittest.TestCase):
    def setUp(self):
        search = patch.object(setup, "windows_codex_binary", side_effect=lambda: setup.shutil.which("codex.exe"))
        search.start()
        self.addCleanup(search.stop)

    PEER = r"""
import json, sys, threading, time
request = json.loads(sys.stdin.readline())
assert request["method"] == "initialize"
eof = threading.Event()
def read_eof():
    assert sys.stdin.read() == ""
    eof.set()
threading.Thread(target=read_eof, daemon=True).start()
mode = sys.argv[1]
if mode in ("reply", "stubborn", "bad_exit", "final_eof"):
    # An EOF-sensitive server drops pending initialization, like native Codex.
    if eof.wait(0.1):
        sys.exit(0)
    print("", flush=True)
    print(json.dumps({"id": 2, "result": None}), flush=True)
    print(json.dumps({"method": "notice"}), flush=True)
    print(json.dumps({"id": request["id"], "result": {"userAgent": "fixture"}}),
          end="" if mode == "final_eof" else "\n", flush=True)
    if mode == "final_eof":
        sys.exit(0)
    if mode == "stubborn":
        time.sleep(30)
    if not eof.wait(5):
        sys.exit(9)
    if mode == "bad_exit":
        sys.exit(3)
elif mode == "eof":
    sys.exit(0)
else:
    if mode == "error":
        print(json.dumps({"id": 1, "error": {"code": -1}}), flush=True)
    if mode == "oversize":
        print("x" * 65537, flush=True)
    if mode == "malformed":
        print("not json", flush=True)
    if mode == "partial":
        print('{"id":1', end="", flush=True)
    time.sleep(30)
"""

    def probe(self, mode: str, *, timeout: float = 1) -> None:
        real_popen = subprocess.Popen
        children = []
        streams = []
        def launch(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            streams.extend(stream for stream in (kwargs["stdout"], kwargs["stderr"])
                           if hasattr(stream, "closed"))
            return child
        started = time.monotonic()
        try:
            with patch.object(setup.subprocess, "Popen", side_effect=launch):
                setup.check_team_initialize([sys.executable, "-u", "-c", self.PEER, mode],
                                            dict(os.environ), timeout=timeout)
        finally:
            self.assertEqual(len(children), 1)
            self.assertIsNotNone(children[0].returncode, "probe must reap its child")
            self.assertTrue(children[0].stdin.closed)
            self.assertTrue(all(stream.closed for stream in streams))
            self.assertTrue(all(not Path(stream.name).exists() for stream in streams))
            self.assertLess(time.monotonic() - started, timeout + 3)

    def test_newly_prepared_backend_is_selected_before_retained_store_planning(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = setup.executable_path(root / setup.CODEX_PACKAGE_DIR / "bin", "codex")
            events = []
            def install(*args):
                events.append("installed")
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"fake package, never executed")
                return binary
            def checked(tools, executable):
                events.append("checked")
                self.assertEqual(Path(executable).resolve(), binary.resolve())
                return "codex-cli 0.166.0"
            def planned(tools, **kwargs):
                events.append("planned")
                self.assertEqual(kwargs["codex_binary"], str(binary.resolve()))
                raise RuntimeError("fixture preflight")
            with patch.object(setup, "release_index", return_value={"tools": {"pira_team": {}}}), \
                 patch.object(setup.shutil, "which", return_value=None), \
                 patch.object(setup, "install_codex", side_effect=install), \
                 patch.object(setup, "check_team_runtime", side_effect=checked), \
                 patch.object(setup.stores, "plan_store_environment", side_effect=planned), \
                 patch.object(setup, "ensure_team_auth") as auth:
                with self.assertRaisesRegex(RuntimeError, "fixture preflight"):
                    setup.main(["--install-dir", str(root)])
                self.assertEqual(events, ["installed", "checked", "planned"])
                auth.assert_not_called()

    def test_prepared_managed_backend_outside_path_is_used_for_store_preflight(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = setup.executable_path(root / setup.CODEX_PACKAGE_DIR / "bin", "codex")
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"fixture, never executed")
            for flags in ([], ["--dry-run"], ["--verify"]):
                events = []
                def checked(tools, executable):
                    events.append("checked")
                    self.assertEqual(executable, str(binary.resolve()))
                    return "codex-cli 0.166.0"
                def planned(tools, **kwargs):
                    events.append("planned")
                    self.assertEqual(kwargs["codex_binary"], str(binary.resolve()))
                    raise RuntimeError("fixture preflight")
                with self.subTest(flags=flags), \
                     patch.object(setup, "release_index", return_value={"tools": {"pira_team": {}}}), \
                     patch.object(setup.shutil, "which", return_value=None), \
                     patch.object(setup, "check_team_runtime", side_effect=checked), \
                     patch.object(setup, "install_codex") as install, \
                     patch.object(setup, "ensure_team_auth") as auth, \
                     patch.object(setup.stores, "plan_store_environment", side_effect=planned), \
                     patch.object(setup.stores, "apply_store_environment") as publish:
                    with self.assertRaisesRegex(RuntimeError, "fixture preflight"):
                        setup.main(["--install-dir", str(root), *flags])
                    self.assertEqual(events, ["checked", "planned"])
                    install.assert_not_called()
                    auth.assert_not_called()
                    publish.assert_not_called()

    def test_initialize_keeps_stdin_open_until_matching_response(self):
        for mode in ("reply", "stubborn", "final_eof"):
            with self.subTest(mode=mode):
                self.probe(mode)

    def test_initialize_error_and_early_eof_reap_the_child(self):
        for mode in ("error", "malformed", "eof", "bad_exit", "oversize"):
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                self.probe(mode)

    PROJECT_PEER = r'''
import json, sys
request = json.loads(sys.stdin.readline())
print(json.dumps({"id": request["id"], "result": {"userAgent": "fixture"}}), flush=True)
assert json.loads(sys.stdin.readline())["method"] == "initialized"
request = json.loads(sys.stdin.readline())
assert request["method"] == "config/read"
project = request["params"]["cwd"]
key, folder, reason = json.loads(sys.argv[1])
result = {"config": {"projects": {key: {"trust_level": "untrusted"}}},
          "layers": [{"name": {"type": "project", "dotCodexFolder": folder},
                      "disabledReason": reason}]}
print(json.dumps({"id": request["id"], "result": result}), flush=True)
sys.stdin.read()
'''

    def project_probe(self, project: str, key: str, folder: str, reason: str | None) -> None:
        setup.check_team_initialize(
            [sys.executable, "-u", "-c", self.PROJECT_PEER, json.dumps([key, folder, reason])],
            dict(os.environ), project=project,
        )

    def test_project_readback_requires_exact_key_and_disabled_layer(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = str(Path(temporary).resolve())
            folder = Path(project) / ".codex"
            folder.mkdir()
            self.project_probe(project, project, str(folder), "untrusted")
            with self.assertRaisesRegex(ValueError, "exact untrusted cwd key"):
                self.project_probe(project, '"' + project + '"', str(folder), "untrusted")
            with self.assertRaisesRegex(ValueError, "layer was not disabled"):
                self.project_probe(project, project, str(folder), None)

    def test_project_readback_accepts_physical_alias_but_not_alias_trust_key(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            (project / ".codex").mkdir(parents=True)
            # A dot component works on every host without symlink privileges.
            alias = str(project) + os.sep + "."
            self.assertNotEqual(alias, str(project))
            self.assertTrue(Path(alias).samefile(project))
            self.project_probe(str(project), str(project), alias + os.sep + ".codex", "untrusted")
            with self.assertRaisesRegex(ValueError, "exact untrusted cwd key"):
                self.project_probe(str(project), alias, str(project / ".codex"), "untrusted")

    @unittest.skipUnless(os.name == "nt", "Windows verbatim drive/UNC paths")
    def test_project_readback_accepts_windows_verbatim_path_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            normal = str(Path(temporary).resolve())
            if normal.startswith("\\\\?\\"):
                normal = normal[4:]
                if normal.startswith("UNC\\"):
                    normal = "\\\\" + normal[4:]
            folder = Path(normal) / ".codex"
            folder.mkdir()
            verbatim = "\\\\?\\UNC\\" + normal[2:] if normal.startswith("\\\\") else "\\\\?\\" + normal
            for project, returned in [(verbatim, str(folder)), (normal, verbatim + "\\.codex")]:
                with self.subTest(project=project):
                    self.project_probe(project, project, returned, "untrusted")
                    with self.assertRaisesRegex(ValueError, "layer was not disabled"):
                        self.project_probe(project, project, returned, None)

    def test_project_readback_rejects_unrelated_missing_and_invalid_layer_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / ".codex").mkdir()
            other = root / "other" / ".codex"
            other.mkdir(parents=True)
            for folder in (str(other), str(root / "missing"), "bad\0path"):
                with self.subTest(folder=folder), self.assertRaisesRegex(ValueError, "layer was not disabled"):
                    self.project_probe(str(root), str(root), folder, "untrusted")

    def test_initialize_deadline_covers_silent_and_partial_output(self):
        for mode in ("silent", "partial"):
            with self.subTest(mode=mode), self.assertRaises(subprocess.TimeoutExpired):
                self.probe(mode, timeout=0.25)


class TeamAuthTests(unittest.TestCase):
    def setUp(self) -> None:
        search = patch.object(setup, "windows_codex_binary", side_effect=lambda: setup.shutil.which("codex.exe"))
        search.start()
        self.addCleanup(search.stop)
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
        self.executable = str(self.root / ("codex.exe" if setup.IS_WINDOWS else "codex"))
        selection = patch.object(setup, "selected_codex_binary", return_value=self.executable)
        selection.start(); self.addCleanup(selection.stop)

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
        with patch.object(setup.subprocess, "run", return_value=self.result(1, "Not logged in")) as run:
            for options in (dict(verify=True), dict(login="skip"), dict(verify=True, dry_run=True)):
                with self.assertRaisesRegex(RuntimeError, "login is missing"):
                    self.ensure(**options)
            self.assertEqual(run.call_count, 3)
            self.assertTrue(all(call.args[0][-2:] == ["login", "status"] for call in run.call_args_list))
        self.assertFalse(self.auth.exists())

    def test_existing_file_cache_reused_without_leaking_status_output(self) -> None:
        self.auth.write_text("synthetic-cache")
        output = io.StringIO()
        with patch.object(setup.subprocess, "run", return_value=self.result(stderr="private-status")) as run, \
             patch.object(setup.sys, "stdout", output):
            self.ensure()
            run.assert_called_once()
            call = run.call_args
            self.assertEqual(call.args[0], [self.executable, "login", "status"])
            self.assertEqual(call.kwargs["env"]["OPENAI_API_KEY"], "synthetic-other-key")
            self.assertEqual(call.kwargs["env"]["CODEX_ACCESS_TOKEN"], "synthetic-other-token")
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
                    return self.result() if self.auth.exists() else self.result(1, "Not logged in")
                managed = str(setup.executable_path(self.root / setup.CODEX_PACKAGE_DIR / "bin", "codex"))
                with patch.object(setup, "selected_codex_binary", return_value=managed), \
                     patch.object(setup.subprocess, "run", side_effect=run) as calls, \
                     patch.object(setup.sys.stdin, "isatty", return_value=tty), \
                     patch.object(setup.shutil, "which", return_value=None):
                    self.ensure(login=login)
                    self.assertEqual(calls.call_count, 3)
                    self.assertEqual(calls.call_args_list[0].args[0][0],
                                     str(setup.executable_path(self.root / setup.CODEX_PACKAGE_DIR / "bin", "codex")))

    def test_login_failures_cancellation_and_missing_cache_are_not_success(self) -> None:
        for response, message in [(self.result(1), "did not complete"),
                                  (subprocess.TimeoutExpired("codex", 300), "timed out"),
                                  (KeyboardInterrupt(), "cancelled"),
                                  (OSError("unavailable"), "Could not launch"),
                                  (self.result(), "without a Team-compatible")]:
            with self.subTest(message=message), patch.object(setup.subprocess, "run") as run:
                run.side_effect = [self.result(1, "Not logged in"), response]
                with self.assertRaisesRegex(RuntimeError, message):
                    self.ensure(login="device")
                self.assertEqual(run.call_count, 2)

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

    def test_existing_signed_out_cache_is_never_overwritten(self) -> None:
        self.auth.write_text("synthetic-expired-cache")
        with patch.object(setup.subprocess, "run", return_value=self.result(1, "Not logged in")) as run:
            with self.assertRaisesRegex(RuntimeError, "preserved without overwrite"):
                self.ensure(login="browser")
            run.assert_called_once()
        self.assertEqual(self.auth.read_text(), "synthetic-expired-cache")

    def test_native_keyring_or_environment_auth_is_preserved_including_skip(self) -> None:
        for options in ({}, {"login": "skip"}, {"verify": True}, {"verify": True, "dry_run": True}):
            output = io.StringIO()
            with self.subTest(options=options), \
                 patch.object(setup.subprocess, "run", return_value=self.result(stderr="private-account")) as run, \
                 patch.object(setup.sys, "stdout", output):
                self.ensure(**options)
                run.assert_called_once()
                self.assertEqual(run.call_args.args[0], [self.executable, "login", "status"])
                self.assertIn("LIMITATION", output.getvalue())
                self.assertIn("not keyring-only", output.getvalue())
                self.assertNotIn("private-account", output.getvalue())
                self.assertFalse(self.auth.exists())

    def test_authentication_cache_is_not_opened_by_setup(self) -> None:
        self.auth.write_text("synthetic-do-not-read")
        native_open = Path.open
        def reject_credentials(path, *args, **kwargs):
            if path == self.auth:
                raise AssertionError("setup must not read credential contents")
            return native_open(path, *args, **kwargs)
        with patch.object(Path, "open", reject_credentials), \
             patch.object(setup.subprocess, "run", return_value=self.result()):
            self.ensure()

    @unittest.skipUnless(os.name == "posix", "credential-link fixture without Windows privileges")
    def test_existing_credential_link_is_reused_without_open_or_relogin(self):
        target = self.home / "private-cache"; target.write_bytes(b"synthetic-cache")
        self.auth.symlink_to(target)
        with patch.object(setup.subprocess, "run", return_value=self.result()) as run:
            self.ensure(login="skip")
            run.assert_called_once()
        self.assertTrue(self.auth.is_symlink())
        self.assertEqual(target.read_bytes(), b"synthetic-cache")

    def test_fresh_signed_out_auth_logs_in_once_then_checks_file_source(self) -> None:
        def native(command, **kwargs):
            if command[-1] == "status":
                return self.result() if self.auth.exists() else self.result(1, "Not logged in")
            self.auth.write_text("synthetic-fresh-cache")
            self.assertNotIn("OPENAI_API_KEY", kwargs["env"])
            self.assertNotIn("CODEX_ACCESS_TOKEN", kwargs["env"])
            return self.result()
        with patch.object(setup.subprocess, "run", side_effect=native) as run:
            self.ensure(login="browser")
            self.assertEqual(run.call_count, 3)
            self.assertEqual(run.call_args_list[0].args[0], [self.executable, "login", "status"])
            self.assertEqual(run.call_args_list[-1].args[0],
                [self.executable, "-c", 'cli_auth_credentials_store="file"', "login", "status"])


class SetupPiraToolsTests(unittest.TestCase):
    def setUp(self) -> None:
        search = patch.object(setup, "windows_codex_binary", side_effect=lambda: setup.shutil.which("codex.exe"))
        search.start()
        self.addCleanup(search.stop)
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        environment = patch.dict(setup.os.environ, {"HOME": home.name, "LOCALAPPDATA": home.name, "USERPROFILE": home.name, "SHELL": "/bin/sh"}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        # Native file operations stay real; registry persistence uses a private test double.
        if setup.os.name == "nt":
            from unittest.mock import MagicMock
            values = {}
            registry = MagicMock()
            registry.REG_SZ, registry.REG_EXPAND_SZ = 1, 2
            def query(handle, key):
                if key not in values:
                    raise FileNotFoundError(key)
                return values[key], registry.REG_SZ
            registry.QueryValueEx.side_effect = query
            registry.SetValueEx.side_effect = lambda handle, key, reserved, kind, value: values.__setitem__(key, value)
            mocked = patch.dict(sys.modules, {"winreg": registry})
            mocked.start()
            self.addCleanup(mocked.stop)
            notification = patch.object(setup.stores, "notify_windows_environment")
            notification.start()
            self.addCleanup(notification.stop)
        # Each network test must supply its own response; fixture mistakes must
        # fail locally rather than downloading from a real release endpoint.
        network = patch.object(setup, "urlopen", side_effect=AssertionError("unmocked setup network request"))
        network.start()
        self.addCleanup(network.stop)

    def test_include_selection_controls_existing_codex_preflight(self) -> None:
        for names, use_codex in [([], True), (["Codex"], True),
                                 (["ClaudeCode"], False), (["Codex", "ClaudeCode"], True)]:
            with self.subTest(names=names), \
                 patch.object(setup, "release_index", return_value={"tools": {"pira_team": {}}}), \
                 patch.object(setup, "prepare_claude_runtime") as claude, \
                 patch.object(setup, "prepare_team_runtime", return_value=None) as prepare, \
                 patch.object(setup, "selected_codex_binary", return_value="fixture-codex") as backend, \
                 patch.object(setup, "ensure_team_auth") as auth, \
                 patch.object(setup.stores, "plan_store_environment", return_value=setup.stores.StorePlan()) as plan, \
                 patch.object(setup, "tool_selection", side_effect=RuntimeError("fixture barrier")):
                selection = ["--include", *names] if names else []
                with self.assertRaisesRegex(RuntimeError, "fixture barrier"):
                    setup.main([*selection, "--dry-run"])
                self.assertEqual(prepare.call_count, int(use_codex))
                self.assertEqual(backend.call_count, int(use_codex))
                self.assertEqual(auth.call_count, int(use_codex))
                self.assertEqual(claude.call_count, int("ClaudeCode" in names))
                self.assertEqual(plan.call_args.kwargs["codex_binary"], "fixture-codex" if use_codex else None)

    @unittest.skipIf(os.name == "nt", "POSIX execute permission")
    @patch.object(setup, "prepare_codex_runtime", return_value="codex-cli 0.166.0")
    def test_nonexecutable_matching_binary_refresh_and_readonly_modes(self, prepared):
        root = Path(setup.os.environ["HOME"])
        binary = setup.executable_path(root / "bin", "pira_ctx")
        binary.parent.mkdir()
        binary.write_bytes(b"binary")
        binary.chmod(0o644)
        platform = setup.load_selector().current_platform()
        args = ["--tool", "pira_ctx", "--no-path", "--install-dir", str(binary.parent)]
        def download(tag, selection, directory):
            source = directory / "pira_ctx"
            source.write_bytes(b"binary")
            return source
        with patch.object(setup, "release_index", return_value=self.index(platform)), \
             patch.object(setup, "download_binary", side_effect=download) as fetch, \
             patch.object(setup, "direct_version", return_value="pira_ctx 1.6.0"):
            self.assertEqual(setup.main([*args, "--dry-run"]), 0)
            self.assertEqual(setup.main([*args, "--verify"]), 1)
            fetch.assert_not_called()
            self.assertEqual(binary.stat().st_mode & 0o777, 0o644)
            self.assertEqual(setup.main(args), 0)
            fetch.assert_called_once()
            self.assertTrue(os.access(binary, os.X_OK))
            inode = binary.stat().st_ino
            self.assertEqual(setup.main(args), 0)
            fetch.assert_called_once()
            self.assertEqual(binary.stat().st_ino, inode)

    @patch.object(setup, "prepare_codex_runtime", return_value="codex-cli 0.166.0")
    def test_no_path_skips_all_environment_persistence_and_verification(self, prepared) -> None:
        root = Path(setup.os.environ["HOME"])
        binary = setup.executable_path(root / "bin", "pira_ctx")
        binary.parent.mkdir()
        binary.write_bytes(b"binary")
        binary.chmod(0o755)
        selector = setup.load_selector()
        platform_key = selector.current_platform()
        for mode in ([], ["--verify"], ["--dry-run"]):
            with self.subTest(mode=mode), patch.object(setup, "release_index", return_value=self.index(platform_key)), \
                 patch.object(setup, "load_selector", return_value=selector), \
                 patch.object(selector, "current_platform", return_value=platform_key), \
                 patch.object(setup, "download_binary") as fetch, \
                 patch.object(setup, "direct_version", return_value="pira_ctx 1.6.0"), \
                 patch.object(setup.stores, "plan_store_environment") as stores, patch.object(setup, "ensure_path") as path:
                self.assertEqual(setup.main(["--tool", "pira_ctx", "--no-path", "--install-dir", str(binary.parent), *mode]), 0)
                fetch.assert_not_called()
                stores.assert_not_called()
                path.assert_not_called()
        self.assertEqual(list(root.iterdir()), [binary.parent])

    @unittest.skipIf(sys.platform == "win32", "Windows Ctx default did not relocate")
    @patch.object(setup, "prepare_codex_runtime", return_value="codex-cli 0.166.0")
    def test_standalone_migration_barrier_precedes_profile_and_path_switch(self, prepared) -> None:
        root = Path(setup.os.environ["HOME"])
        source = setup.stores.historical_store_paths("pira_ctx")[0]
        destination = Path(setup.stores.selected_store_paths(["pira_ctx"])["PIRA_CTX_STORE_DIR"])
        (source / "records").mkdir(parents=True)
        record = source / "records/synthetic.json"
        record.write_bytes(b'{"id":"synthetic"}\n')
        binary = setup.executable_path(root / "bin", "pira_ctx")
        binary.parent.mkdir()
        binary.write_bytes(b"binary")
        binary.chmod(0o755)
        platform = setup.load_selector().current_platform()
        args = ["--install-dir", str(binary.parent), "--tool", "pira_ctx"]
        profiles = setup.stores.shell_profiles()
        with patch.object(setup, "release_index", return_value=self.index(platform)), \
             patch.object(setup, "direct_version", return_value="pira_ctx 1.6.0"), \
             patch.object(setup, "ensure_path") as path:
            self.assertEqual(setup.main([*args, "--no-path"]), 0)
            self.assertFalse(destination.exists())
            path.assert_not_called()
            self.assertEqual(setup.main([*args, "--dry-run"]), 0)
            self.assertFalse(destination.exists())
            self.assertTrue(all(not profile.exists() for profile in profiles))
            path.reset_mock()
            with patch.object(setup.stores.migration, "verify_destination", side_effect=RuntimeError("verification failed")):
                with self.assertRaisesRegex(RuntimeError, "verification failed"):
                    setup.main(args)
            path.assert_not_called()
            self.assertTrue(all(not profile.exists() for profile in profiles))
            self.assertEqual(record.read_bytes(), b'{"id":"synthetic"}\n')
            self.assertEqual(setup.main(args), 0)
            self.assertEqual((destination / "records/synthetic.json").read_bytes(), record.read_bytes())
            self.assertTrue(all(str(destination) in profile.read_text() for profile in profiles))
            before = [profile.read_bytes() for profile in profiles]
            self.assertEqual(setup.main(args), 0)
            self.assertEqual([profile.read_bytes() for profile in profiles], before)

    def test_normal_selection_requires_team_but_partial_maintenance_is_allowed(self) -> None:
        index = self.index()
        with self.assertRaisesRegex(RuntimeError, "requires a release containing pira_team"):
            setup.selected_tools(index, None)
        self.assertEqual(setup.selected_tools(index, ["pira_ctx"]), ["pira_ctx"])
        index["tools"]["pira_team"] = {"version": "0.2.0", "binaries": {}}
        self.assertIn("pira_team", setup.selected_tools(index, None))
        with self.assertRaises(SystemExit):
            setup.build_parser().parse_args(["--no-team"])
        with patch.object(setup, "release_index", return_value=index), patch.object(setup, "prepare_team_runtime") as runtime:
            with self.assertRaisesRegex(RuntimeError, "excluded"):
                setup.main(["--tool", "pira_ctx", "--version", "team=0.2.0"])
            runtime.assert_not_called()

    def test_partial_install_verify_and_dry_run_prepare_codex_and_preserve_team(self) -> None:
        selector = setup.load_selector()
        platform_key = selector.current_platform()
        index = self.index(platform_key)
        index["tools"]["pira_team"] = {"version": "0.2.0", "binaries": {}}
        for mode in ([], ["--verify"], ["--dry-run"]):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                root = Path(temp).resolve()
                team = setup.executable_path(root, "pira_team")
                team.write_bytes(b"preexisting team")
                ctx = setup.executable_path(root, "pira_ctx")
                if mode:
                    ctx.write_bytes(b"binary")
                    ctx.chmod(0o755)
                def download(tag, selection, directory):
                    self.assertEqual(selection.name, "pira_ctx")
                    path = directory / selection.asset.removesuffix(".gz")
                    path.write_bytes(b"binary")
                    return path
                with patch.object(setup, "release_index", return_value=index), \
                     patch.object(setup, "load_selector", return_value=selector), \
                     patch.object(selector, "current_platform", return_value=platform_key), \
                     patch.object(setup, "prepare_codex_runtime", return_value="valid") as runtime, \
                     patch.object(setup, "ensure_team_auth") as auth, \
                     patch.object(setup, "download_binary", side_effect=download) as fetch, \
                     patch.object(setup, "direct_version", return_value="pira_ctx 1.6.0"), \
                     patch.object(setup, "ensure_path") as path, \
                     patch.object(setup, "path_is_configured", return_value=True):
                    if "--verify" in mode:
                        setup.stores.apply_store_environment(setup.stores.plan_store_environment(["pira_ctx"]), dry_run=False)
                    self.assertEqual(setup.main(["--tool", "pira_ctx", "--install-dir", str(root), *mode]), 0)
                    runtime.assert_called_once_with(root, platform_key, verify="--verify" in mode, dry_run="--dry-run" in mode)
                    auth.assert_not_called()
                    if mode:
                        fetch.assert_not_called()
                    else:
                        fetch.assert_called_once()
                    if "--verify" in mode:
                        path.assert_not_called()
                    else:
                        self.assertTrue(path.call_args.kwargs["include_codex"])
                self.assertEqual(team.read_bytes(), b"preexisting team")
                self.assertEqual(ctx.read_bytes(), b"binary")

    def test_team_preflight_skips_other_tools_and_rejects_missing_backend(self) -> None:
        with patch.object(setup.shutil, "which", return_value=None) as which:
            self.assertIsNone(setup.check_team_runtime(["pira_ctx", "pira_nav"]))
            which.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, "requires the Codex executable"):
                setup.check_team_runtime(["pira_team"])

    def test_windows_checks_native_image_instead_of_earlier_npm_shim(self) -> None:
        root = Path(setup.os.environ["HOME"])
        (root / "external").mkdir()
        native, shim = root / "external" / "codex.exe", root / "codex.cmd"
        native.write_bytes(b"native fixture, never executed")
        shim.write_text("arbitrary shim must not be parsed or executed")
        with patch.object(setup, "IS_WINDOWS", True, create=True), \
             patch.object(setup.shutil, "which", side_effect=lambda name: str(native if name == "codex.exe" else shim)) as which, \
             patch.object(setup.subprocess, "run", side_effect=[
                 subprocess.CompletedProcess([], 0, "codex-cli 0.166.0", ""),
                 subprocess.CompletedProcess([], 0, "--stdio --strict-config", "")]) as run, \
             patch.object(setup, "check_team_protocol") as protocol:
            self.assertEqual(setup.selected_codex_binary(root), str(native.resolve()))
            self.assertEqual(setup.prepare_team_runtime(["pira_team"], root, "windows-x64",
                                                        verify=True, dry_run=False), "codex-cli 0.166.0")
            self.assertEqual([call.args[0][0] for call in run.call_args_list], [str(native.resolve())] * 2)
            protocol.assert_called_once_with(str(native.resolve()))
            self.assertTrue(all(call.args == ("codex.exe",) for call in which.call_args_list))

    def test_windows_shim_only_layout_plans_or_installs_native_without_executing_shim(self) -> None:
        root = Path(setup.os.environ["HOME"])
        with patch.object(setup, "IS_WINDOWS", True), \
             patch.object(setup.shutil, "which", side_effect=lambda name: None if name == "codex.exe" else str(root / "codex.cmd")), \
             patch.object(setup, "check_team_runtime") as check, \
             patch.object(setup, "replace_managed_codex", return_value="valid") as install:
            with self.assertRaisesRegex(RuntimeError, "Selected Codex is missing"):
                setup.prepare_codex_runtime(root, "windows-x64", verify=True, dry_run=False)
            self.assertIsNone(setup.prepare_codex_runtime(root, "windows-x64", verify=False, dry_run=True))
            install.assert_not_called()
            self.assertEqual(setup.prepare_codex_runtime(root, "windows-x64", verify=False, dry_run=False), "valid")
            install.assert_called_once_with(root, "windows-x64")
            check.assert_not_called()

    def test_windows_managed_native_is_used_when_only_npm_shim_is_on_path(self) -> None:
        root = Path(setup.os.environ["HOME"])
        native = root / setup.CODEX_PACKAGE_DIR / "bin" / "codex.exe"
        native.parent.mkdir(parents=True)
        native.write_bytes(b"native fixture, never executed")
        with patch.object(setup, "IS_WINDOWS", True, create=True), \
             patch.object(setup.shutil, "which", side_effect=lambda name: None if name == "codex.exe" else str(root / "codex.cmd")), \
             patch.object(setup, "check_team_runtime", return_value="valid") as check, \
             patch.object(setup, "install_codex") as install:
            self.assertEqual(setup.prepare_team_runtime(["pira_team"], root, "windows-x64",
                                                       verify=True, dry_run=False), "valid")
            check.assert_called_once_with(["pira_team"], str(native.resolve()))
            install.assert_not_called()

    def test_windows_team_application_directory_takes_native_search_priority(self) -> None:
        root = Path(setup.os.environ["HOME"])
        native = root / "codex.exe"
        native.write_bytes(b"native fixture, never executed")
        with patch.object(setup, "IS_WINDOWS", True), \
             patch.object(setup, "windows_codex_binary", return_value=str(root / "other" / "codex.exe")) as search:
            self.assertEqual(setup.selected_codex_binary(root), str(native.resolve()))
            search.assert_not_called()

    def test_windows_managed_application_directory_wins_other_native_search_locations(self):
        root = Path(setup.os.environ["HOME"])
        package = root / setup.CODEX_PACKAGE_DIR
        native = package / "bin" / "codex.exe"
        native.parent.mkdir(parents=True); native.write_bytes(b"fixture")
        (package / "selected").write_bytes(b"managed-v1\n")
        with patch.object(setup, "IS_WINDOWS", True), \
             patch.object(setup, "windows_codex_binary", return_value=str(root / "shadow" / "codex.exe")) as search:
            self.assertEqual(setup.team_install_dir(root), native.parent)
            self.assertEqual(setup.codex_on_path(root), str(native.resolve()))
            self.assertEqual(setup.selected_codex_binary(root), str(native.resolve()))
            search.assert_not_called()

    def test_windows_python_application_directory_shadow_is_rejected(self) -> None:
        root = Path(setup.os.environ["HOME"])
        shadow = str(Path(sys.executable).resolve().with_name("codex.exe"))
        with patch.object(setup, "IS_WINDOWS", True), \
             patch.object(setup, "windows_codex_binary", return_value=shadow), \
             self.assertRaisesRegex(RuntimeError, "Ambiguous codex.exe.*cannot certify"):
            setup.selected_codex_binary(root)

    @unittest.skipUnless(os.name == "posix", "POSIX alias fixture without Windows symlink privileges")
    def test_windows_native_name_cannot_disguise_a_command_shim(self) -> None:
        root = Path(setup.os.environ["HOME"])
        shim = root / "codex.cmd"
        shim.write_text("arbitrary shim must not be read or executed")
        alias = root / "codex.exe"
        alias.symlink_to(shim)
        with patch.object(setup, "IS_WINDOWS", True), \
             patch.object(setup, "check_team_runtime") as check, \
             self.assertRaisesRegex(RuntimeError, "resolves to a non-native launcher"):
            setup.prepare_team_runtime(["pira_team"], root, "windows-x64", verify=True, dry_run=False)
        check.assert_not_called()

    def test_codex_version_and_help_failures_preserve_native_diagnostics(self) -> None:
        for first, replies in [
            ("--version", [subprocess.CompletedProcess([], 7, "native output", "native error")]),
            ("app-server --help", [subprocess.CompletedProcess([], 0, "codex-cli 0.166.0", ""),
                                   subprocess.CompletedProcess([], 7, "native output", "native error")]),
        ]:
            with self.subTest(command=first), patch.object(setup.subprocess, "run", side_effect=replies), \
                 self.assertRaises(RuntimeError) as raised:
                setup.check_team_runtime(["pira_team"], "/fixture/codex.exe")
            for text in (first, "exit 7", "native output", "native error"):
                self.assertIn(text, str(raised.exception))

    def test_team_preflight_checks_version_and_required_options_without_login(self) -> None:
        def result(text: str, code: int = 0):
            return subprocess.CompletedProcess([], code, text, "")
        with patch.object(setup.shutil, "which", return_value="/test/codex"):
            for version in ("codex-cli 0.166.0", "codex-cli 0.166.1", "codex-cli 0.166.0+build.1"):
                with self.subTest(version=version), patch.object(setup.subprocess, "run", side_effect=[
                    result(version), result("--stdio\n--strict-config")]) as run, \
                     patch.object(setup, "check_team_protocol") as protocol:
                    self.assertEqual(setup.check_team_runtime(["pira_team"]), version)
                    protocol.assert_called_once_with("/test/codex")
                    self.assertEqual([c.args[0][1:] for c in run.call_args_list],
                                     [["--version"], ["app-server", "--help"]])
                    self.assertTrue(all(c.kwargs["timeout"] == 10 for c in run.call_args_list))
            for version in ("codex-cli 0.166.0-beta.1", "unknown"):
                with self.subTest(version=version), patch.object(setup.subprocess, "run", return_value=result(version)) as run:
                    with self.assertRaisesRegex(RuntimeError, "Cannot verify a stable Codex version"):
                        setup.check_team_runtime(["pira_team"])
                    self.assertEqual(run.call_count, 1)
            with patch.object(setup.subprocess, "run", side_effect=[
                result("codex-cli 0.166.0"), result("--stdio-only --strict-config")]):
                with self.assertRaisesRegex(RuntimeError, "lacks required options"):
                    setup.check_team_runtime(["pira_team"])
            for failure in (subprocess.TimeoutExpired("codex", 10), OSError("unavailable")):
                with patch.object(setup.subprocess, "run", side_effect=failure):
                    with self.assertRaisesRegex(RuntimeError, "Cannot check"):
                        setup.check_team_runtime(["pira_team"])
            with patch.object(setup.subprocess, "run", return_value=result("", 1)):
                with self.assertRaisesRegex(RuntimeError, "failed"):
                    setup.check_team_runtime(["pira_team"])

    def test_team_preflight_rejects_older_versions_before_protocol(self):
        for version in ("codex-cli 0.159.0", "codex-cli 0.165.9"):
            with self.subTest(version=version), \
                 patch.object(setup.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, version, "")) as run, \
                 patch.object(setup, "check_team_protocol") as protocol:
                with self.assertRaisesRegex(RuntimeError, r"requires Codex >= 0\.166\.0"):
                    setup.check_team_runtime(["pira_team"], "/test/codex")
                self.assertEqual(run.call_count, 1)
                protocol.assert_not_called()

    def test_team_protocol_checks_native_inventory_before_direct_initialize(self):
        contract = json.loads((setup.REPO_ROOT / "tools/src/pira_team/backend_contract.json").read_text())
        fixture = runpy.run_path(str(setup.REPO_ROOT / "tools/crates/pira_team/tests/backend_fixture.py"))
        for fault in ("", "method", "field", "live_method", "live_response"):
            def run(command, **kwargs):
                self.assertEqual(kwargs["timeout"], 10)
                self.assertNotIn("--strict-config", command)
                self.assertEqual(command[:4], ["/fixture/codex", "app-server", "generate-json-schema", "--experimental"])
                directory = Path(command[command.index("--out") + 1])
                fixture["write_schemas"](directory, contract, fault)
                if fault == "live_method":
                    path = directory / "ClientRequest.json"
                    schema = json.loads(path.read_text())
                    schema["oneOf"] = [item for item in schema["oneOf"]
                                       if item["properties"]["method"]["enum"] != ["turn/settings/update"]]
                    path.write_text(json.dumps(schema))
                elif fault == "live_response":
                    (directory / "v2/TurnSettingsUpdateResponse.json").unlink()
                return subprocess.CompletedProcess(command, 0, "", "")
            def initialize(command, env, *, project):
                self.assertIn(".path with spaces", project)
                self.assertIn(f'projects={{{json.dumps(project, ensure_ascii=False)}={{trust_level="untrusted"}}}}', command)
                self.assertTrue((Path(project) / ".codex/config.toml").is_file())
                self.assertIn('trust_level="trusted"', (Path(env["CODEX_HOME"]) / "config.toml").read_text())
                self.assertNotIn("daemon", command)
                self.assertIn("--strict-config", command)
                self.assertIn("--stdio", command)
                self.assertTrue(Path(env["CODEX_HOME"]).is_dir())
                self.assertNotIn("CODEX_API_KEY", env)
            with self.subTest(fault=fault), patch.object(setup.subprocess, "run", side_effect=run), \
                 patch.object(setup, "check_team_initialize", side_effect=initialize) as probe:
                if fault:
                    with self.assertRaises(ValueError): setup.check_team_protocol("/fixture/codex")
                    probe.assert_not_called()
                else:
                    setup.check_team_protocol("/fixture/codex")
                    probe.assert_called_once()

    def test_team_protocol_failures_are_actionable(self):
        results = [subprocess.CompletedProcess([], 0, "codex-cli 0.166.0", ""),
                   subprocess.CompletedProcess([], 0, "--stdio --strict-config", "")]
        for failure in (ValueError("missing method"), subprocess.TimeoutExpired("codex", 10)):
            with patch.object(setup.subprocess, "run", side_effect=results), \
                 patch.object(setup, "check_team_protocol", side_effect=failure):
                with self.assertRaisesRegex(RuntimeError, "Unsupported native Codex backend"):
                    setup.check_team_runtime(["pira_team"], "/fixture/codex")

    def test_team_preflight_blocks_install_verify_and_dry_run_before_writes(self) -> None:
        index = self.index()
        index["tools"]["pira_team"] = {"version":"0.2.0", "binaries":{}}
        for mode in ([], ["--verify"], ["--dry-run"]):
            with patch.dict(setup.os.environ, {"PIRA_TEAM_DIR": str(Path(setup.os.environ["HOME"]) / "custom-team")}), \
                 patch.object(setup, "release_index", return_value=index), \
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
        with patch.dict(setup.os.environ, {"PIRA_TEAM_DIR": str(Path(setup.os.environ["HOME"]) / "custom-team")}), \
             patch.object(setup, "release_index", return_value=index), \
             patch.object(setup, "prepare_team_runtime", return_value="codex-cli 0.166.0"), \
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
            platform = setup.load_selector().current_platform()
            existing = setup.executable_path(root / "existing", "codex")
            with patch.object(setup.shutil, "which", return_value=None), \
                 patch.object(setup, "replace_managed_codex", return_value="valid") as install, \
                 patch.object(setup, "check_team_runtime") as check:
                self.assertIsNone(setup.prepare_team_runtime(["pira_nav"], root, platform, verify=False, dry_run=False))
                self.assertIsNone(setup.prepare_codex_runtime(root, platform, verify=False, dry_run=True))
                with self.assertRaisesRegex(RuntimeError, "Selected Codex is missing"):
                    setup.prepare_codex_runtime(root, platform, verify=True, dry_run=False)
                install.assert_not_called()
                check.assert_not_called()
                self.assertEqual(setup.prepare_codex_runtime(root, platform, verify=False, dry_run=False), "valid")
                install.assert_called_once_with(root, platform)
            with patch.object(setup.shutil, "which", return_value=str(existing)), \
                 patch.object(setup, "replace_managed_codex", return_value="new") as install, \
                 patch.object(setup, "check_team_runtime", side_effect=RuntimeError("old runtime")) as check:
                self.assertEqual(setup.prepare_codex_runtime(root, platform, verify=False, dry_run=False), "new")
                check.assert_called_once_with(["pira_team"], str(existing.resolve()))
                install.assert_called_once_with(root, platform)

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
        metadata = {"tag_name": "rust-v0.166.0", "draft": False, "prerelease": False,
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
                     patch.object(setup, "check_team_runtime", return_value="codex-cli 0.166.0"):
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
                        check.assert_called_once_with(["pira_team"], str(installed.resolve()))

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
        for changed in ({"prerelease": True}, {"draft": True}, {"tag_name": "rust-v0.166.0-beta"},
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
            with patch.object(setup.stores, "shell_profiles", return_value=[profile]):
                setup.ensure_path(root / "bin", True)
                self.assertFalse(profile.exists())
                setup.ensure_path(root / "bin", False)
                self.assertIn(setup.stores.shell_path_line(binary.parent), profile.read_text())
                self.assertTrue(setup.path_is_configured(binary.parent))
                self.assertFalse(setup.ensure_path(root / "bin", False))

    @unittest.skipIf(setup.os.name == "nt", "POSIX executable selection")
    def test_codex_profile_keeps_selected_backend_and_managed_fallback(self) -> None:
        root = Path(setup.os.environ["HOME"])
        install = root / "bin"
        managed = install / setup.CODEX_PACKAGE_DIR / "bin"
        external = root / "external"
        profile = root / "profile"
        original = "# unrelated user configuration\n"
        for directory, marker in ((managed, "managed"), (external, "external")):
            directory.mkdir(parents=True)
            binary = directory / "codex"
            binary.write_text("#!/bin/sh\nprintf '%s\\n' " + marker + "\n")
            binary.chmod(0o755)
        package_bytes = (managed / "codex").read_bytes()
        with patch.object(setup.stores, "shell_profiles", return_value=[profile]):
            for include in (True, False):  # Team install and subsequent non-Team setup.
                for path, expected in ((str(external), "external"), ("/usr/bin:/bin", "managed"),
                                       (str(managed) + ":" + str(external), "managed")):
                    with self.subTest(include_codex=include, selected=expected, path=path), \
                         patch.dict(setup.os.environ, {"PATH": path}):
                        # Migrate the old generated block, not just a fresh profile.
                        profile.write_text(original + setup.stores.BLOCK_START + "\n"
                                           + setup.stores.shell_path_line(managed) + "\n"
                                           + setup.stores.BLOCK_END + "\n")
                        before = profile.read_bytes()
                        setup.ensure_path(install, True, include_codex=include)
                        self.assertEqual(profile.read_bytes(), before)
                        setup.ensure_path(install, False, include_codex=include)
                        self.assertTrue(profile.read_text().startswith(original))
                        if expected == "managed":
                            self.assertTrue(setup.path_is_configured(managed))
                        self.assertFalse(setup.ensure_path(install, False, include_codex=include))
                        result = subprocess.run(
                            ["/bin/sh", "-c", '. "$1"; . "$1"; codex', "sh", str(profile)],
                            env={"PATH": path}, check=True, capture_output=True, text=True,
                        )
                        self.assertEqual(result.stdout.strip(), expected)
                        # New PATH syntax must remain compatible with store inference.
                        with profile.open("a") as stream:
                            stream.write("export PIRA_CTX_STORE_DIR=" + shlex.quote(str(root / "ctx")) + "\n")
                        self.assertEqual(setup.stores.plan_store_environment(["pira_ctx"]).stores,
                                         {"PIRA_CTX_STORE_DIR": str((root / "ctx").resolve())})
        self.assertEqual((managed / "codex").read_bytes(), package_bytes)

    def test_windows_codex_path_preserves_external_backend_priority(self) -> None:
        from unittest.mock import MagicMock
        root = Path(setup.os.environ["HOME"])
        managed = root / "bin" / setup.CODEX_PACKAGE_DIR / "bin"
        external = str(root / "external")
        registry = MagicMock()
        registry.REG_EXPAND_SZ = 2
        # A prior setup may already have placed the retained package first.
        for current in (external, str(managed) + ";" + external):
            with self.subTest(current=current):
                registry.QueryValueEx.return_value = (current, 2)
                registry.SetValueEx.reset_mock()
                with patch.dict(sys.modules, {"winreg": registry}), \
                     patch.object(setup.stores, "notify_windows_environment"):
                    self.assertTrue(setup.windows_user_path(managed, True, append=True))
                    registry.SetValueEx.assert_not_called()
                    self.assertTrue(setup.windows_user_path(managed, False, append=True))
                    updated = registry.SetValueEx.call_args.args[-1]
                    self.assertEqual(updated, external + ";" + str(managed))
                    registry.QueryValueEx.return_value = (updated, 2)
                    self.assertFalse(setup.windows_user_path(managed, False, append=True))

    def test_windows_path_identity_resolves_both_registry_and_requested_aliases(self) -> None:
        from unittest.mock import MagicMock
        root = Path(setup.os.environ["HOME"])
        real = root / "long directory" / "bin"
        real.mkdir(parents=True)
        alias = root / "alias"
        alias.symlink_to(real.parent, target_is_directory=True)
        aliased = alias / "bin"
        external = str(root / "external")
        registry = MagicMock()
        registry.REG_SZ, registry.REG_EXPAND_SZ = 1, 2
        expanded = "%PIRA_TEST_BIN%" if setup.os.name == "nt" else "$PIRA_TEST_BIN"
        for current_entry, requested in ((str(aliased), real), (str(real), aliased), (expanded, real)):
            with self.subTest(current=current_entry, requested=requested), \
                 patch.dict(setup.os.environ, {"PIRA_TEST_BIN": str(aliased)}), \
                 patch.dict(sys.modules, {"winreg": registry}), \
                 patch.object(setup.stores, "notify_windows_environment"):
                registry.QueryValueEx.return_value = (current_entry + ";" + external, 2)
                registry.SetValueEx.reset_mock()
                self.assertFalse(setup.windows_user_path(requested, False))
                registry.SetValueEx.assert_not_called()
                self.assertTrue(setup.windows_user_path(requested, True, append=True))
                registry.SetValueEx.assert_not_called()
                self.assertTrue(setup.windows_user_path(requested, False, append=True))
                updated = registry.SetValueEx.call_args.args[-1]
                self.assertEqual(updated, external + ";" + str(requested))
                registry.QueryValueEx.return_value = (updated, 2)
                registry.SetValueEx.reset_mock()
                self.assertFalse(setup.windows_user_path(requested, False, append=True))
                registry.SetValueEx.assert_not_called()
        for current, requested in ((real, aliased), (aliased, real)):
            with patch.dict(setup.os.environ, {"PATH": str(current)}):
                self.assertTrue(setup.path_is_configured(requested))
            registry.QueryValueEx.return_value = (str(current), 2)
            with patch.dict(setup.os.environ, {"PATH": ""}), \
                 patch.dict(sys.modules, {"winreg": registry}), \
                 patch.object(setup, "Path", type(real)), patch.object(setup.os, "name", "nt"):
                self.assertTrue(setup.path_is_configured(requested))
        if setup.os.name != "nt":
            profile = root / "profile"
            profile.write_text(setup.stores.BLOCK_START + "\n" + setup.stores.shell_path_line(aliased) + "\n")
            with patch.dict(setup.os.environ, {"PATH": ""}), \
                 patch.object(setup.stores, "shell_profiles", return_value=[profile]):
                self.assertTrue(setup.path_is_configured(aliased))

    def test_windows_path_uses_fallback_only_for_external_codex(self) -> None:
        root = Path(setup.os.environ["HOME"])
        managed = root / setup.CODEX_PACKAGE_DIR / "bin"
        for selected, append in ((str(root / "external" / "codex.exe"), True),
                                  (str(managed / "codex.exe"), False), (None, False)):
            with self.subTest(selected=selected), patch.object(setup.os, "name", "nt"), \
                 patch.object(setup, "IS_WINDOWS", True), \
                 patch.object(setup, "Path", type(root)), \
                 patch.object(setup, "codex_on_path", return_value=selected), \
                 patch.object(setup, "windows_user_path", return_value=False) as persist:
                setup.ensure_path(root, False, include_codex=True)
                self.assertEqual(persist.call_args.args, (managed, False))
                self.assertEqual(persist.call_args.kwargs, {"append": append})

    @unittest.skipIf(setup.os.name == "nt", "POSIX shell quoting")
    def test_shell_path_line_preserves_literal_paths_and_is_idempotent(self) -> None:
        for directory in (Path("/tmp/simple/bin"), Path("/tmp/space quote' $dollar/bin")):
            script = setup.stores.shell_path_line(directory)
            result = subprocess.run(["/bin/sh", "-c", script + "; " + script + '; printf %s "$PATH"'],
                                    env={"PATH": "/usr/bin:/bin"}, check=True, text=True, capture_output=True)
            self.assertEqual(result.stdout, str(directory) + ":/usr/bin:/bin")

    def index(self, platform_key: str = "linux-x64") -> dict[str, object]:
        data = b"binary"
        compressed = gzip.compress(data, mtime=0)
        suffix = ".exe" if platform_key.startswith("windows-") else ""
        record = {
            "asset": f"pira_ctx-1.6.0-{platform_key}{suffix}.gz",
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
                    "binaries": {platform_key: record},
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

    def test_version_help_lists_write_alias(self) -> None:
        self.assertIn("write=VERSION", setup.build_parser().format_help())

    def test_parses_per_tool_versions(self) -> None:
        self.assertEqual(
            setup.parse_versions(
                ["ctx=1.6.0", "pira_nav=0.11.0", "svg=0.1.0", "team=0.2.0", "write=0.1.0"]
            ),
            {
                "pira_ctx": "1.6.0",
                "pira_nav": "0.11.0",
                "pira_svg_check": "0.1.0",
                "pira_team": "0.2.0",
                "pira_write_check": "0.1.0",
            },
        )

    def test_selector_accepts_checkers_and_rejects_unsafe_names(self) -> None:
        for tool in ("pira_svg_check", "pira_write_check"):
            selector = setup.load_selector()
            self.assertEqual(selector.validate_tool_name(tool), tool)
            with self.assertRaises(selector.SelectionError):
                selector.validate_tool_name("../" + tool)

    def test_selector_installs_checkers_atomically(self) -> None:
        for tool in ("pira_svg_check", "pira_write_check"):
            selector = setup.load_selector()
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = root / ("downloaded-" + tool)
                source.write_bytes((tool + "-binary").encode())
                record = {"sha256": hashlib.sha256(source.read_bytes()).hexdigest()}
                source.chmod(0o755)
                for platform_key in ("darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "windows-x64"):
                    selected, selected_record = selector.select_binary(
                        platform_key, tool_name=tool, bundle_dir=root,
                        manifest={"schema_version": 1, "tool_name": tool,
                                  "binaries": {platform_key: {**record, "path": source.name}}},
                    )
                    self.assertEqual(selected, source.resolve())
                    installed = selector.install_binary(
                        selected, selected_record, root / "bin", tool_name=tool,
                    )
                    self.assertEqual(installed.name, tool)
                    self.assertEqual(installed.read_bytes(), source.read_bytes())

    def test_selection_includes_available_checkers(self) -> None:
        for tool in ("pira_svg_check", "pira_write_check"):
            index = self.index()
            index["tools"][tool] = {
                "version": "0.1.0",
                "binaries": {},
            }
            self.assertEqual(
                setup.selected_tools(index, ["pira_ctx", tool]),
                ["pira_ctx", tool],
            )

    def test_default_selection_includes_checker_only_when_released(self) -> None:
        index = self.index()
        index["tools"]["pira_team"] = {"version": "0.2.0", "binaries": {}}
        self.assertEqual(setup.selected_tools(index, None), ["pira_ctx", "pira_team"])
        with self.assertRaisesRegex(RuntimeError, "requested tool is not released"):
            setup.selected_tools(index, ["pira_write_check"])
        index["tools"]["pira_write_check"] = {"version": "0.1.0", "binaries": {}}
        self.assertEqual(
            setup.selected_tools(index, None),
            ["pira_ctx", "pira_team", "pira_write_check"],
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


class CodexPreparationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.install = self.root / "tools"
        self.install.mkdir()
        self.platform = "windows-x64" if setup.IS_WINDOWS else "linux-x64"
        self.package = self.install / setup.CODEX_PACKAGE_DIR
        self.external = setup.executable_path(self.root / "external", "codex")
        environment = patch.dict(os.environ, {"HOME": str(self.root), "USERPROFILE": str(self.root),
                                               "PATH": "", "LOCALAPPDATA": str(self.root)}, clear=True)
        environment.start(); self.addCleanup(environment.stop)
        for mocked in (
            patch.object(setup, "urlopen", side_effect=AssertionError("unexpected network")),
            patch.object(setup, "codex_on_path", side_effect=lambda directory=None:
                         str(self.external) if self.external.exists() else None),
            patch.object(setup, "check_team_runtime", side_effect=self.check),
        ):
            mocked.start(); self.addCleanup(mocked.stop)

    def package_at(self, install, content=b"compatible", platform=None):
        package = install / setup.CODEX_PACKAGE_DIR
        (package / "bin").mkdir(parents=True)
        (package / "codex-package.json").write_text('{"fixture":true}')
        suffix = ".exe" if (platform or self.platform).startswith("windows-") else ""
        binary = package / "bin" / ("codex" + suffix)
        binary.write_bytes(content); binary.chmod(0o755)
        (package / "bin" / ("codex-code-mode-host" + suffix)).write_bytes(b"resource")
        return binary

    def check(self, tools, executable):
        self.assertEqual(tools, ["pira_team"])
        if not Path(executable).is_file() or Path(executable).read_bytes() != b"compatible":
            raise RuntimeError("incompatible native version/protocol")
        return "codex-cli 0.166.0"

    def install_package(self, install, platform):
        return self.package_at(install, platform=platform)

    def prepare(self, **options):
        return setup.prepare_codex_runtime(self.install, self.platform,
            verify=options.get("verify", False), dry_run=options.get("dry_run", False))

    def test_compatible_external_is_unchanged_in_all_modes(self):
        self.external.parent.mkdir(); self.external.write_bytes(b"compatible")
        with patch.object(setup, "install_codex") as install:
            for options in ({}, {"verify": True}, {"dry_run": True}, {"verify": True, "dry_run": True}):
                self.assertEqual(self.prepare(**options), "codex-cli 0.166.0")
                self.assertEqual(setup.selected_codex_binary(self.install), str(self.external.resolve()))
                self.assertFalse(self.package.exists())
                self.assertEqual(os.environ["PATH"], "")
            install.assert_not_called()
        self.assertEqual(self.external.read_bytes(), b"compatible")

    def test_missing_readonly_then_install_and_rerun_are_deterministic(self):
        with patch.object(setup, "install_codex", side_effect=self.install_package) as install:
            with self.assertRaisesRegex(RuntimeError, "Selected Codex is missing"):
                self.prepare(verify=True)
            self.assertIsNone(self.prepare(dry_run=True))
            install.assert_not_called(); self.assertEqual(list(self.install.iterdir()), [])
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            selected = setup.selected_codex_binary(self.install)
            self.assertEqual(Path(selected).parent, self.package / "bin")
            self.assertEqual((self.package / "selected").read_bytes(), b"managed-v1\n")
            self.assertEqual(os.environ["PATH"].split(os.pathsep)[0], str(self.package / "bin"))
            before = {p.relative_to(self.package): p.read_bytes() for p in self.package.rglob("*") if p.is_file()}
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            self.assertEqual(self.prepare(verify=True), "codex-cli 0.166.0")
            install.assert_called_once_with(self.install, self.platform)
            self.assertEqual(before, {p.relative_to(self.package): p.read_bytes() for p in self.package.rglob("*") if p.is_file()})

    def test_incompatible_external_is_preserved_with_native_home_and_account(self):
        self.external.parent.mkdir(); self.external.write_bytes(b"old")
        home = self.root / "native-home"; home.mkdir()
        for name in ("config.toml", "auth.json"):
            (home / name).write_bytes(b"synthetic-preserve")
        with patch.dict(os.environ, {"CODEX_HOME": str(home)}), \
             patch.object(setup, "install_codex", side_effect=self.install_package) as install:
            with self.assertRaisesRegex(RuntimeError, "incompatible native"):
                self.prepare(verify=True)
            self.assertIsNone(self.prepare(dry_run=True)); install.assert_not_called()
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            self.assertEqual(setup.selected_codex_binary(self.install),
                             str(setup.executable_path(self.package / "bin", "codex").resolve()))
            self.assertEqual(os.environ["CODEX_HOME"], str(home))
        self.assertEqual(self.external.read_bytes(), b"old")
        self.assertEqual([p.read_bytes() for p in sorted(home.iterdir())], [b"synthetic-preserve"] * 2)

    def test_retained_compatible_package_is_selected_without_download(self):
        self.external.parent.mkdir(); self.external.write_bytes(b"old")
        binary = self.package_at(self.install)
        with patch.object(setup, "install_codex") as install:
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            install.assert_not_called()
        self.assertEqual(setup.selected_codex_binary(self.install), str(binary.resolve()))

    def test_managed_update_retains_all_prior_data_and_team_and_is_idempotent(self):
        binary = self.package_at(self.install, b"old")
        setup.select_managed_codex(self.install)
        team = setup.executable_path(binary.parent, "pira_team"); team.write_bytes(b"released-team")
        (self.package / "custom-data").write_bytes(b"never-delete")
        previous = self.package.with_name(self.package.name + ".backup-1")
        previous.mkdir(); (previous / "sentinel").write_bytes(b"earlier-backup")
        with patch.object(setup, "install_codex", side_effect=self.install_package) as install:
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            install.assert_called_once()
        backup = self.package.with_name(self.package.name + ".backup-2")
        self.assertEqual((backup / "bin" / binary.name).read_bytes(), b"old")
        self.assertEqual((backup / "custom-data").read_bytes(), b"never-delete")
        self.assertEqual((previous / "sentinel").read_bytes(), b"earlier-backup")
        self.assertEqual(team.read_bytes(), b"released-team")
        self.assertFalse(list(self.install.glob(".pira-codex-update-*")))

    def test_selected_missing_payload_is_repaired_without_falling_back_to_external(self):
        binary = self.package_at(self.install, b"old")
        setup.select_managed_codex(self.install); binary.unlink()
        self.external.parent.mkdir(); self.external.write_bytes(b"compatible")
        with patch.object(setup, "install_codex", side_effect=self.install_package) as install:
            with self.assertRaisesRegex(RuntimeError, "incompatible native"):
                self.prepare(verify=True)
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
            install.assert_called_once()
        self.assertEqual(setup.selected_codex_binary(self.install), str(binary.resolve()))

    def test_foreign_or_stale_preparation_lock_is_not_removed(self):
        lock = self.install / ".pira-codex-prepare.lock"; lock.mkdir()
        with patch.object(setup, "install_codex") as install:
            with self.assertRaisesRegex(RuntimeError, "preparation is locked"):
                self.prepare()
            install.assert_not_called()
        self.assertTrue(lock.is_dir()); self.assertFalse(self.package.exists())

    def test_failed_final_native_check_rolls_back_package_and_selection(self):
        binary = self.package_at(self.install, b"old")
        setup.select_managed_codex(self.install)
        original = {p.relative_to(self.package): p.read_bytes() for p in self.package.rglob("*") if p.is_file()}
        with patch.object(setup, "install_codex", side_effect=self.install_package), \
             patch.object(setup, "check_team_runtime", side_effect=RuntimeError("final native failure")):
            with self.assertRaisesRegex(RuntimeError, "final native failure"):
                self.prepare()
        self.assertEqual(original, {p.relative_to(self.package): p.read_bytes() for p in self.package.rglob("*") if p.is_file()})
        failed = self.package.with_name(self.package.name + ".failed-1")
        self.assertEqual((failed / "bin" / binary.name).read_bytes(), b"compatible")
        self.assertEqual(os.environ["PATH"], "")
        with patch.object(setup, "install_codex", side_effect=self.install_package):
            self.assertEqual(self.prepare(), "codex-cli 0.166.0")
        self.assertTrue(failed.is_dir())

    def test_failed_download_or_publication_restores_old_package(self):
        self.package_at(self.install, b"old")
        native_rename = Path.rename
        def reject_publication(path, target):
            if path.name == setup.CODEX_PACKAGE_DIR and path.parent.name.startswith(".pira-codex-update-"):
                raise OSError("publication denied")
            return native_rename(path, target)
        for fault in ("download", "publication"):
            with self.subTest(fault=fault), \
                 patch.object(setup, "install_codex", side_effect=RuntimeError("download failed") if fault == "download" else self.install_package), \
                 patch.object(Path, "rename", reject_publication):
                with self.assertRaisesRegex((RuntimeError, OSError), "download failed|publication denied"):
                    self.prepare()
                self.assertEqual(setup.executable_path(self.package / "bin", "codex").read_bytes(), b"old")
                self.assertFalse(list(self.install.glob(".pira-codex-update-*")))

    @unittest.skipUnless(os.name == "posix", "symlink fixture without Windows privileges")
    def test_symlink_or_ambiguous_managed_state_fails_without_install(self):
        foreign = self.root / "foreign"; foreign.mkdir()
        with patch.object(setup, "install_codex") as install:
            self.package.symlink_to(foreign, target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, "ambiguous managed"):
                self.prepare()
            self.package.unlink()
            self.package_at(self.install, b"old")
            marker = self.package / "selected"; marker.write_text("unexpected")
            with self.assertRaisesRegex(RuntimeError, "ambiguous managed"):
                self.prepare()
            marker.unlink(); (self.package / "linked").symlink_to(foreign, target_is_directory=True)
            with self.assertRaisesRegex(RuntimeError, "symlinked managed"):
                self.prepare()
            install.assert_not_called()
        self.assertEqual(list(foreign.iterdir()), [])

    def test_windows_adjacent_collision_uses_managed_team_destination_and_reruns(self):
        self.platform = "windows-x64"
        adjacent = self.install / "codex.exe"; adjacent.write_bytes(b"old-external")
        original_team = self.install / "pira_team.exe"; original_team.write_bytes(b"old-team")
        data = b"released-team"
        record = {"asset": "pira_team-0.2.0-windows-x64.exe", "size": len(data),
                  "sha256": hashlib.sha256(data).hexdigest()}
        index = {"tag": "pira-tools-fixture", "tools": {"pira_team": {
            "version": "0.2.0", "binaries": {"windows-x64": record}}}}
        selector = setup.load_selector()
        def download(tag, selection, directory):
            binary = directory / record["asset"]; binary.write_bytes(data); return binary
        with patch.object(setup, "IS_WINDOWS", True), \
             patch.object(setup, "codex_on_path", return_value=str(adjacent)), \
             patch.object(setup, "install_codex", side_effect=self.install_package) as install, \
             patch.object(setup, "release_index", return_value=index), \
             patch.object(setup, "load_selector", return_value=selector), \
             patch.object(selector, "current_platform", return_value=self.platform), \
             patch.object(setup, "ensure_team_auth") as auth, \
             patch.object(setup, "download_binary", side_effect=download) as fetch, \
             patch.object(setup, "direct_version", return_value="pira_team 0.2.0"), \
             patch.object(setup, "ensure_path") as persist:
            args = ["--install-dir", str(self.install), "--tool", "pira_team", "--no-path"]
            self.assertEqual(setup.main(args), 0)
            target = self.package / "bin" / "pira_team.exe"
            self.assertEqual(setup.team_install_dir(self.install), target.parent)
            self.assertEqual(target.read_bytes(), data)
            self.assertEqual(os.environ["PATH"].split(os.pathsep)[0], str(target.parent))
            self.assertEqual(setup.main(args), 0)
            self.assertEqual(setup.main([*args, "--verify"]), 0)
            install.assert_called_once(); fetch.assert_called_once(); persist.assert_not_called()
            self.assertEqual(auth.call_count, 3)
        self.assertEqual(adjacent.read_bytes(), b"old-external")
        self.assertEqual(original_team.read_bytes(), b"old-team")

    def test_parent_claude_boundary_and_marker_activation(self):
        import types
        parent = types.SimpleNamespace(prepare_runtime=MagicMock(return_value="2.1.311"))
        config = self.root / "claude-work"
        with patch.dict(sys.modules, {"setup_pira_claude_runtime": parent}):
            self.assertEqual(setup.prepare_claude_runtime(verify=True, dry_run=False,
                config_dir=config, install_dir=self.install), "2.1.311")
        parent.prepare_runtime.assert_called_once_with(setup, self.install,
            verify=True, dry_run=False, config_dir=config)
        package = self.install / ".pira-claude"; (package / "bin").mkdir(parents=True)
        (package / "selected").write_bytes(b"managed-v1\n")
        self.assertEqual(setup.claude_managed_bin(self.install), package / "bin")
        if os.name != "nt":
            profile = self.root / "profile"
            with patch.object(setup.stores, "shell_profiles", return_value=[profile]):
                setup.ensure_path(self.install, True)
                self.assertFalse(profile.exists())
                setup.ensure_path(self.install, False)
                prefix = os.pathsep.join(map(str, [package / "bin", self.install]))
                self.assertIn(setup.activation_path_line(prefix), profile.read_text())
                self.assertFalse(setup.ensure_path(self.install, False))

    @unittest.skipUnless(os.name == "posix", "POSIX combined profile activation")
    def test_combined_managed_profile_sourcing_is_stable_with_existing_late_paths(self):
        self.package_at(self.install); setup.select_managed_codex(self.install)
        claude = self.install / ".pira-claude"; (claude / "bin").mkdir(parents=True)
        (claude / "selected").write_bytes(b"managed-v1\n")
        profile = self.root / "profile"
        with patch.object(setup.stores, "shell_profiles", return_value=[profile]):
            self.assertTrue(setup.ensure_path(self.install, False))
            self.assertFalse(setup.ensure_path(self.install, False))
        original = "/external/bin:" + str(self.package / "bin") + ":" + str(claude / "bin")
        script = '. "$1"; printf "%s\\n" "$PATH"; . "$1"; printf "%s\\n" "$PATH"'
        result = subprocess.run(["/bin/sh", "-c", script, "sh", str(profile)],
            env={"PATH": original}, check=True, text=True, capture_output=True)
        once, twice = result.stdout.splitlines()
        self.assertEqual(once, twice)
        self.assertTrue(once.startswith(":".join(map(str, [claude / "bin", self.package / "bin", self.install])) + ":"))
        self.assertIn("/external/bin", once)

    def test_windows_combined_managed_path_order_is_durable_and_rerun_does_not_write(self):
        self.platform = "windows-x64"
        self.package_at(self.install); setup.select_managed_codex(self.install)
        claude = self.install / ".pira-claude"; (claude / "bin").mkdir(parents=True)
        (claude / "selected").write_bytes(b"managed-v1\n")
        external = str(self.root / "external")
        state = {"path": external + ";" + str(self.package / "bin")}
        registry = MagicMock(); registry.REG_EXPAND_SZ = 2
        registry.QueryValueEx.side_effect = lambda *args: (state["path"], 2)
        registry.SetValueEx.side_effect = lambda *args: state.__setitem__("path", args[-1])
        with patch.object(setup, "IS_WINDOWS", True), patch.object(setup.os, "name", "nt"), \
             patch.object(setup, "Path", type(self.root)), \
             patch.dict(sys.modules, {"winreg": registry}), \
             patch.object(setup.stores, "notify_windows_environment") as notify:
            self.assertTrue(setup.ensure_path(self.install, True))
            registry.SetValueEx.assert_not_called(); notify.assert_not_called()
            self.assertTrue(setup.ensure_path(self.install, False))
            self.assertEqual(state["path"], ";".join(map(str, [claude / "bin", self.package / "bin", self.install, external])))
            self.assertFalse(setup.ensure_path(self.install, False))
            registry.SetValueEx.assert_called_once(); notify.assert_called_once()

    def test_tools_main_activates_managed_claude_locally_only_in_normal_mode(self):
        claude = self.install / ".pira-claude"; (claude / "bin").mkdir(parents=True)
        (claude / "selected").write_bytes(b"managed-v1\n")
        for mode in (["--verify"], ["--dry-run"], []):
            with self.subTest(mode=mode), patch.dict(os.environ, {"PATH": "original"}), \
                 patch.object(setup, "prepare_claude_runtime", return_value="2.1.311") as prepare, \
                 patch.object(setup, "release_index", side_effect=RuntimeError("fixture release boundary")), \
                 patch.object(setup, "ensure_path") as persist:
                with self.assertRaisesRegex(RuntimeError, "fixture release boundary"):
                    setup.main(["--include", "ClaudeCode", "--install-dir", str(self.install), "--no-path", *mode])
                prepare.assert_called_once_with(verify="--verify" in mode, dry_run="--dry-run" in mode, install_dir=self.install)
                expected = "original" if mode else str(claude / "bin") + os.pathsep + "original"
                self.assertEqual(os.environ["PATH"], expected)
                persist.assert_not_called()

    @unittest.skipUnless(os.name == "posix", "POSIX activation script")
    def test_selected_path_activation_wins_existing_late_entry_and_is_idempotent(self):
        selected = self.root / "space quote' $literal" / "bin"
        script = setup.activation_path_line(selected)
        original = "/external/bin:" + str(selected) + ":/usr/bin"
        result = subprocess.run(["/bin/sh", "-c", script + "; " + script + '; printf %s "$PATH"'],
                                env={"PATH": original}, check=True, text=True, capture_output=True)
        self.assertEqual(result.stdout, str(selected) + ":" + original)


if __name__ == "__main__":
    unittest.main()
