"""Managed diagnostic and caller-home regressions; fake Codex only."""
from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import unittest

import test_team


class StorageProfileTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root

    def launch(self, task: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.fixture.bin), "run", "--completion-gate", "Fixture complete",
             "--model", "test-model", "--effort", "high", "--store", str(self.root / "logs"),
             "--cwd", str(self.root), "--task", task],
            cwd=self.root, env=self.fixture.env, capture_output=True, text=True, timeout=10,
        )

    def lookup(self, command: str, run: str, relative: str) -> subprocess.CompletedProcess[bytes]:
        return subprocess.run(
            [str(self.fixture.bin), command, run, relative, "--store", str(self.root / "logs")],
            env=self.fixture.env, capture_output=True, timeout=5,
        )

    def test_backend_failure_log_is_accessible_at_all_managed_stages(self) -> None:
        (self.root / "codex").write_text("#!/bin/sh\necho fixture-schema-failure >&2\nexit 7\n")
        result = self.launch("backend-check")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("see backend-check.log", result.stderr)
        run = next((self.root / "logs").iterdir())
        content = b"fixture-schema-failure\n"
        self.assertEqual((run / "backend-check.log").read_bytes(), content)
        for prefix in ("", "repair", "implementation", "implementation/repair",
                       "revisions/000002", "revisions/000002/repair",
                       "revisions/000002/implementation", "revisions/000002/implementation/repair"):
            with self.subTest(prefix=prefix):
                directory = run / prefix
                directory.mkdir(parents=True, exist_ok=True)
                if prefix:
                    (directory / "backend-check.log").write_bytes(content)
                (directory / "stderr.log").write_bytes(b"neighbor\n")
                for name, expected in (("backend-check.log", content), ("stderr.log", b"neighbor\n")):
                    path = directory / name
                    relative = str(path.relative_to(run))
                    read = self.lookup("read", run.name, relative)
                    self.assertEqual(read.returncode, 0, read.stderr)
                    self.assertEqual(read.stdout, expected)
                    resolved = self.lookup("path", run.name, relative)
                    self.assertEqual(resolved.returncode, 0, resolved.stderr)
                    self.assertEqual(Path(os.fsdecode(resolved.stdout).strip()), path)

    def test_log_lookup_does_not_expand_private_or_traversal_routes(self) -> None:
        run = self.root / "logs" / "fixture-run"
        run.mkdir(parents=True)
        for relative in ("codex-home/backend-check.log", "codex-home/auth.json",
                         "backend-schema/backend-check.log", "run.lock",
                         "repair/auth.json", "implementation/codex-home/backend-check.log",
                         "revisions/000002/implementation/repair/auth.json",
                         "revisions/2/backend-check.log", "implementation/implementation/backend-check.log",
                         "repair/../backend-check.log", "../backend-check.log", str(run / "backend-check.log")):
            for command in ("read", "path"):
                with self.subTest(relative=relative, command=command):
                    result = self.lookup(command, run.name, relative)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, b"")
                    self.assertRegex(os.fsdecode(result.stderr),
                                     "not a managed artifact or diagnostic path|"
                                     "managed file must be a relative path without traversal")

    @unittest.skipUnless(os.name == "posix", "symlink and FIFO fixtures")
    def test_log_lookup_rejects_symlinks_and_special_files(self) -> None:
        run = self.root / "logs" / "fixture-run"
        run.mkdir(parents=True)
        target = self.root / "outside.log"
        target.write_bytes(b"must-not-read\n")
        (run / "backend-check.log").symlink_to(target)
        (run / "implementation").symlink_to(self.root, target_is_directory=True)
        repair = run / "repair"
        repair.mkdir()
        os.mkfifo(repair / "backend-check.log")
        for relative in ("backend-check.log", "implementation/backend-check.log", "repair/backend-check.log"):
            for command in ("read", "path"):
                with self.subTest(relative=relative, command=command):
                    result = self.lookup(command, run.name, relative)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, b"")
                    self.assertIn(b"symlinks or special files", result.stderr)
        self.assertEqual(target.read_bytes(), b"must-not-read\n")

    def test_caller_home_skips_empty_values_without_changing_precedence(self) -> None:
        user = self.root / "user-profile"
        home = self.root / "user-home"
        for directory in (user, home):
            shutil.copytree(self.root / "original-home/sessions", directory / ".codex/sessions")
        cases = [
            (codex, home_value, str(user))
            for codex in (None, "") for home_value in (None, "")
        ] + [
            (None, str(home), str(self.root / "missing-user-profile")),
            (str(self.root / "original-home"), str(self.root / "missing-home"), str(user)),
        ]
        for codex, home_value, user_value in cases:
            with self.subTest(codex=codex, home=home_value, user=user_value):
                for key, value in (("CODEX_HOME", codex), ("HOME", home_value), ("USERPROFILE", user_value)):
                    if value is None:
                        self.fixture.env.pop(key, None)
                    else:
                        self.fixture.env[key] = value
                result = self.launch("home-discovery")
                self.assertEqual(result.returncode, 0, result.stderr)
        for key in ("CODEX_HOME", "HOME", "USERPROFILE"):
            self.fixture.env[key] = ""
        result = self.launch("missing-home")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot locate Codex home", result.stderr)


if __name__ == "__main__":
    unittest.main()
