from __future__ import annotations

import contextlib
import importlib.util
import io
import os
from pathlib import Path
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("setup_pira_communication.py").resolve()
SPEC = importlib.util.spec_from_file_location("communication_setup", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)


class CommunicationSetupTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.home = self.root / "home"
        self.workspace = self.home / "pira"

    def run_setup(self, *flags: str, home: Path | None = None) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with patch.object(setup.Path, "home", return_value=home or self.home), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = setup.main(list(flags))
        return code, out.getvalue(), err.getvalue()

    def fixture_templates(self) -> Path:
        directory = self.root / "templates"
        directory.mkdir()
        for template in setup.TEMPLATES.values():
            (directory / template).write_bytes((setup.TEMPLATE_DIR / template).read_bytes())
        return directory

    def symlink(self, link: Path, target: Path, *, directory: bool = False) -> None:
        try:
            link.symlink_to(target, target_is_directory=directory)
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"Symlink creation unavailable: {exc}")

    def snapshot(self, directory: Path) -> dict:
        return {str(p.relative_to(directory)): (p.lstat().st_mode, p.lstat().st_mtime_ns,
                p.read_bytes() if p.is_file() else None)
                for p in [directory, *sorted(directory.rglob("*"))]}

    def assert_no_onboarding(self, output: str) -> None:
        for text in ("Open a NEW", "ONE conversation", "Provide your current projects"):
            self.assertNotIn(text, output)

    def test_fresh_default_copies_shipped_templates_only(self) -> None:
        code, out, err = self.run_setup()
        self.assertEqual((code, err), (0, ""))
        self.assertEqual({p.name for p in self.workspace.iterdir()}, set(setup.TEMPLATES))
        self.assertEqual({p.name for p in self.home.iterdir()}, {"pira"})
        for name, template in setup.TEMPLATES.items():
            self.assertEqual((self.workspace / name).read_bytes(),
                             (setup.TEMPLATE_DIR / template).read_bytes())
        self.assertIn("Summary: created=2 preserved=0 errors=0", out)
        self.assertIn(f"Open a NEW Codex or supported agent session in {self.workspace}.", out)
        self.assertIn("ONE conversation across all projects", out)
        self.assertIn("shared context and coordination", out)
        self.assertIn("Provide your current projects", out)
        self.assertTrue(self.workspace.is_absolute())

    def test_fixed_home_uses_path_home_for_all_platform_layouts(self) -> None:
        for layout in ("C/Users/Synthetic User", "Users/synthetic", "home/synthetic"):
            with self.subTest(layout=layout):
                home = self.root / layout
                out, err = io.StringIO(), io.StringIO()
                with patch.dict(os.environ, {"HOME": str(self.root / "decoy"),
                                             "USERPROFILE": str(self.root / "other"),
                                             "LOCALAPPDATA": str(self.root / "appdata")}), \
                     patch.object(setup.Path, "home", return_value=home) as lookup, \
                     contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    self.assertEqual(setup.main([]), 0)
                lookup.assert_called_once_with()
                self.assertTrue((home / "pira/AGENTS.md").is_file())
                self.assertIn(f"session in {home / 'pira'}.", out.getvalue())
                self.assertFalse((self.root / "decoy").exists())
                self.assertFalse((self.root / "appdata").exists())
                self.assertEqual(err.getvalue(), "")

    def test_custom_workspace_option_is_rejected(self) -> None:
        for flags in (["--workspace", str(self.root / "custom")], ["--workspace=custom"]):
            with self.subTest(flags=flags), contextlib.redirect_stderr(io.StringIO()), \
                 self.assertRaises(SystemExit) as raised:
                setup.main(flags)
            self.assertEqual(raised.exception.code, 2)
        self.assertFalse(self.home.exists())
        self.assertFalse((self.root / "custom").exists())
        help_text = setup.build_parser().format_help()
        self.assertNotIn("--workspace", help_text)
        self.assertIn("Requires normal PIRA already installed", help_text)

    def test_rerun_preserves_edited_files_and_unrelated_content_byte_for_byte(self) -> None:
        self.assertEqual(self.run_setup()[0], 0)
        for name, data in {"AGENTS.md": b"synthetic instructions\r\n",
                           "AGENT_WORKBOOK.md": b"synthetic notes\x00\xff"}.items():
            (self.workspace / name).write_bytes(data)
        (self.workspace / "nested").mkdir()
        (self.workspace / "nested/other.txt").write_bytes(b"unrelated")
        before = self.snapshot(self.workspace)
        for flags in ((), ("--dry-run",), ()):
            code, out, err = self.run_setup(*flags)
            self.assertEqual((code, err), (0, ""))
            self.assertIn("SKIPPED: existing directory preserved unchanged", out)
            self.assertNotIn("would create", out)
            self.assert_no_onboarding(out)
            self.assertEqual(self.snapshot(self.workspace), before)

    def test_empty_and_incomplete_directories_are_skipped_without_filling(self) -> None:
        for existing in (None, *setup.TEMPLATES):
            with self.subTest(existing=existing):
                home = self.root / str(existing)
                workspace = home / "pira"
                workspace.mkdir(parents=True)
                if existing:
                    (workspace / existing).write_bytes(b"custom synthetic content")
                before = self.snapshot(workspace)
                with patch.object(setup, "load_templates", side_effect=AssertionError("skip needs no templates")):
                    for flags in ((), ("--dry-run",), ()):
                        code, out, err = self.run_setup(*flags, home=home)
                        self.assertEqual((code, err), (0, ""))
                        self.assertIn(f"SKIPPED: existing directory preserved unchanged: {workspace}", out)
                        self.assertNotIn("would create", out)
                        self.assert_no_onboarding(out)
                        self.assertEqual(self.snapshot(workspace), before)

    def test_dry_run_does_not_create_missing_parents(self) -> None:
        home = self.root / "missing parent/nested home"
        before = list(self.root.iterdir())
        code, out, err = self.run_setup("--dry-run", home=home)
        self.assertEqual((code, err), (0, ""))
        self.assertIn(f"DRY-RUN: would create directory {home / 'pira'}", out)
        for name in setup.TEMPLATES:
            self.assertIn(f"DRY-RUN: would create {home / 'pira' / name}", out)
        self.assertEqual(list(self.root.iterdir()), before)
        self.assert_no_onboarding(out)

    def test_verify_missing_files_is_read_only(self) -> None:
        code, out, err = self.run_setup("--verify")
        self.assertEqual(code, 1)
        self.assertIn("Summary: created=0 preserved=0 errors=2", out)
        for name in setup.TEMPLATES:
            self.assertIn(f"Missing required file: {self.workspace / name}", err)
        self.assertFalse(self.home.exists())
        self.assert_no_onboarding(out)

    def test_verify_incomplete_directory_never_fills_missing_files(self) -> None:
        self.workspace.mkdir(parents=True)
        (self.workspace / "AGENTS.md").write_bytes(b"custom")
        before = self.snapshot(self.workspace)
        code, out, err = self.run_setup("--verify")
        self.assertEqual(code, 1)
        self.assertIn("Missing required file", err)
        self.assertEqual(self.snapshot(self.workspace), before)
        self.assert_no_onboarding(out)

    def test_verify_accepts_customized_regular_files_without_writing(self) -> None:
        self.workspace.mkdir(parents=True)
        for name in setup.TEMPLATES:
            (self.workspace / name).write_bytes(b"synthetic custom\x00\xff")
        before = self.snapshot(self.workspace)
        with patch.object(setup, "load_templates", side_effect=AssertionError("verify needs no templates")):
            code, out, err = self.run_setup("--verify")
        self.assertEqual((code, err), (0, ""))
        self.assertIn("Verification passed", out)
        self.assertEqual(self.snapshot(self.workspace), before)
        self.assert_no_onboarding(out)

    def test_conflicting_workspace_or_home_file_is_not_modified(self) -> None:
        for component in ("home", "workspace"):
            for flags in ((), ("--dry-run",), ("--verify",)):
                with self.subTest(component=component, flags=flags):
                    home = self.root / f"{component}-{flags}"
                    conflict = home if component == "home" else home / "pira"
                    conflict.parent.mkdir(parents=True, exist_ok=True)
                    conflict.write_bytes(b"synthetic conflicting file")
                    before = conflict.read_bytes(), conflict.stat().st_mtime_ns
                    code, out, err = self.run_setup(*flags, home=home)
                    self.assertEqual(code, 1)
                    self.assertIn("Conflicting file", err)
                    self.assertEqual((conflict.read_bytes(), conflict.stat().st_mtime_ns), before)
                    self.assert_no_onboarding(out)

    def test_directory_at_each_managed_file_blocks_all_creates_in_all_modes(self) -> None:
        for name in setup.TEMPLATES:
            for flags in ((), ("--dry-run",), ("--verify",)):
                with self.subTest(name=name, flags=flags):
                    home = self.root / f"{name}-{flags}"
                    workspace = home / "pira"
                    (workspace / name).mkdir(parents=True)
                    before = self.snapshot(workspace)
                    code, _, err = self.run_setup(*flags, home=home)
                    self.assertEqual(code, 1)
                    self.assertIn("Conflicting directory", err)
                    self.assertEqual(self.snapshot(workspace), before)

    def test_override_any_type_blocks_all_creates(self) -> None:
        for kind in ("file", "directory", "dangling symlink"):
            for flags in ((), ("--dry-run",), ("--verify",)):
                with self.subTest(kind=kind, flags=flags):
                    home = self.root / f"{kind}-{flags}"
                    workspace = home / "pira"
                    workspace.mkdir(parents=True)
                    override = workspace / "AGENTS.override.md"
                    if kind == "file":
                        override.write_bytes(b"synthetic override")
                    elif kind == "directory":
                        override.mkdir()
                    else:
                        self.symlink(override, self.root / "absent")
                    code, _, err = self.run_setup(*flags, home=home)
                    self.assertEqual(code, 1)
                    self.assertIn("Instruction override exists", err)
                    self.assertEqual(list(workspace.iterdir()), [override])

    def test_workspace_home_and_ancestor_symlinks_are_not_followed(self) -> None:
        for component in ("workspace", "home", "ancestor"):
            with self.subTest(component=component):
                actual = self.root / f"actual-{component}"
                actual.mkdir()
                home = self.root / component
                if component == "workspace":
                    home.mkdir()
                    link = home / "pira"
                else:
                    link = home
                self.symlink(link, actual, directory=True)
                if component == "ancestor":
                    home = link / "nested"
                for flags in ((), ("--dry-run",), ("--verify",)):
                    code, _, err = self.run_setup(*flags, home=home)
                    self.assertEqual(code, 1)
                    self.assertIn("symlink/reparse point", err)
                    self.assertEqual(list(actual.iterdir()), [])
                self.assertTrue(link.is_symlink())

    def test_dangling_home_and_workspace_symlinks_are_not_followed(self) -> None:
        for component in ("home", "workspace"):
            with self.subTest(component=component):
                home = self.root / component
                actual = self.root / f"absent-{component}"
                if component == "workspace":
                    home.mkdir()
                link = home if component == "home" else home / "pira"
                self.symlink(link, actual, directory=True)
                code, _, err = self.run_setup(home=home)
                self.assertEqual(code, 1)
                self.assertIn("symlink/reparse point", err)
                self.assertFalse(actual.exists())
                self.assertTrue(link.is_symlink())

    def test_managed_symlinks_to_files_directories_or_missing_paths_are_preserved(self) -> None:
        for name in setup.TEMPLATES:
            for kind in ("file", "directory", "missing"):
                with self.subTest(name=name, kind=kind):
                    home = self.root / f"{name}-{kind}"
                    workspace = home / "pira"
                    workspace.mkdir(parents=True)
                    actual = self.root / f"actual-{name}-{kind}"
                    if kind == "file":
                        actual.write_bytes(b"outside synthetic content")
                    elif kind == "directory":
                        actual.mkdir()
                    link = workspace / name
                    self.symlink(link, actual, directory=kind == "directory")
                    for flags in ((), ("--dry-run",), ("--verify",)):
                        code, _, err = self.run_setup(*flags, home=home)
                        self.assertEqual(code, 1)
                        self.assertIn("symlink/reparse point", err)
                        self.assertTrue(link.is_symlink())
                        self.assertEqual(list(workspace.iterdir()), [link])
                        if kind == "file":
                            self.assertEqual(actual.read_bytes(), b"outside synthetic content")
                        elif kind == "directory":
                            self.assertEqual(list(actual.iterdir()), [])
                        else:
                            self.assertFalse(actual.exists())

    @unittest.skipUnless(hasattr(os, "mkfifo"), "FIFO needs POSIX")
    def test_special_file_conflict_does_not_block_or_write(self) -> None:
        self.workspace.mkdir(parents=True)
        fifo = self.workspace / "AGENT_WORKBOOK.md"
        os.mkfifo(fifo)
        code, _, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("Conflicting special file", err)
        self.assertEqual(list(self.workspace.iterdir()), [fifo])
        self.assertTrue(stat.S_ISFIFO(fifo.lstat().st_mode))

    def test_windows_reparse_point_is_not_treated_as_regular_directory(self) -> None:
        info = SimpleNamespace(st_mode=stat.S_IFDIR, st_file_attributes=0x400)
        with patch.object(setup.Path, "lstat", return_value=info):
            self.assertEqual(setup.path_kind(self.workspace), "symlink/reparse point")

    def test_missing_either_template_blocks_fresh_writes(self) -> None:
        directory = self.fixture_templates()
        for template in setup.TEMPLATES.values():
            path = directory / template
            content = path.read_bytes()
            path.unlink()
            for flags in ((), ("--dry-run",)):
                with self.subTest(template=template, flags=flags), patch.object(setup, "TEMPLATE_DIR", directory):
                    code, out, err = self.run_setup(*flags)
                    self.assertEqual(code, 1)
                    self.assertIn(f"Cannot read template {path}", err)
                    self.assertFalse(self.home.exists())
                    self.assert_no_onboarding(out)
            path.write_bytes(content)

    def test_invalid_utf8_template_blocks_all_fresh_writes(self) -> None:
        directory = self.fixture_templates()
        (directory / "COMMUNICATION_WORKBOOK_TEMPLATE.md").write_bytes(b"\xff")
        with patch.object(setup, "TEMPLATE_DIR", directory):
            code, _, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("Cannot read template", err)
        self.assertFalse(self.home.exists())

    def test_templates_are_read_at_runtime_and_copied_without_newline_conversion(self) -> None:
        directory = self.fixture_templates()
        for template in setup.TEMPLATES.values():
            (directory / template).write_bytes("synthetic template — runtime\r\n".encode("utf-8"))
        with patch.object(setup, "TEMPLATE_DIR", directory):
            self.assertEqual(self.run_setup()[0], 0)
        for name in setup.TEMPLATES:
            self.assertEqual((self.workspace / name).read_bytes(), "synthetic template — runtime\r\n".encode("utf-8"))

    def test_concurrent_workspace_creator_wins_without_any_filling(self) -> None:
        original_mkdir = Path.mkdir
        winner = self.workspace / "AGENTS.md"
        before = {}
        def competing_mkdir(path, *args, **kwargs):
            if path == self.workspace:
                original_mkdir(path)
                winner.write_bytes(b"concurrent synthetic instructions")
                before.update(self.snapshot(path))
                raise FileExistsError(str(path))
            return original_mkdir(path, *args, **kwargs)
        with patch.object(setup.Path, "mkdir", new=competing_mkdir):
            code, out, err = self.run_setup()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("SKIPPED: existing directory preserved unchanged", out)
        self.assertEqual(self.snapshot(self.workspace), before)
        self.assertFalse((self.workspace / "AGENT_WORKBOOK.md").exists())
        self.assert_no_onboarding(out)

    def test_concurrent_conflicting_workspace_is_not_followed_or_overwritten(self) -> None:
        original_mkdir = Path.mkdir
        actual = self.root / "outside"
        actual.mkdir()
        probe = self.root / "probe"
        self.symlink(probe, actual, directory=True)
        probe.unlink()
        for kind in ("file", "symlink"):
            with self.subTest(kind=kind):
                home = self.root / kind
                workspace = home / "pira"
                def competing_mkdir(path, *args, **kwargs):
                    if path == workspace:
                        if kind == "file":
                            path.write_bytes(b"concurrent file")
                        else:
                            path.symlink_to(actual, target_is_directory=True)
                        raise FileExistsError(str(path))
                    return original_mkdir(path, *args, **kwargs)
                with patch.object(setup.Path, "mkdir", new=competing_mkdir):
                    code, out, err = self.run_setup(home=home)
                self.assertEqual(code, 1)
                self.assertIn("Conflicting", err)
                self.assertEqual(list(actual.iterdir()), [])
                if kind == "file":
                    self.assertEqual(workspace.read_bytes(), b"concurrent file")
                else:
                    self.assertTrue(workspace.is_symlink())
                self.assert_no_onboarding(out)

    def test_concurrent_home_alias_is_rejected_before_workspace_creation(self) -> None:
        actual = self.root / "outside-home"
        actual.mkdir()
        original_mkdir = Path.mkdir
        def competing_mkdir(path, *args, **kwargs):
            if path == self.home:
                self.symlink(path, actual, directory=True)
                return None
            return original_mkdir(path, *args, **kwargs)
        with patch.object(setup.Path, "mkdir", new=competing_mkdir):
            code, out, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("symlink/reparse point", err)
        self.assertEqual(list(actual.iterdir()), [])
        self.assertTrue(self.home.is_symlink())
        self.assert_no_onboarding(out)

    def test_disappearing_concurrent_workspace_fails_closed(self) -> None:
        original_mkdir = Path.mkdir
        def disappearing_mkdir(path, *args, **kwargs):
            if path == self.workspace:
                raise FileExistsError(str(path))
            return original_mkdir(path, *args, **kwargs)
        with patch.object(setup.Path, "mkdir", new=disappearing_mkdir):
            code, out, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("Workspace disappeared", err)
        self.assertFalse(self.workspace.exists())
        self.assert_no_onboarding(out)

    def test_concurrent_override_blocks_all_file_creates(self) -> None:
        original_mkdir = Path.mkdir
        override = self.workspace / "AGENTS.override.md"
        def competing_mkdir(path, *args, **kwargs):
            result = original_mkdir(path, *args, **kwargs)
            if path == self.workspace:
                override.write_bytes(b"concurrent synthetic override")
            return result
        with patch.object(setup.Path, "mkdir", new=competing_mkdir):
            code, out, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("Instruction override exists", err)
        self.assertEqual(list(self.workspace.iterdir()), [override])
        self.assert_no_onboarding(out)

    def test_exclusive_create_preserves_concurrent_regular_file(self) -> None:
        winner = self.workspace / "AGENTS.md"
        original_open = Path.open
        def competing_open(path, mode="r", *args, **kwargs):
            if path == winner and mode == "xb" and not path.exists():
                path.write_bytes(b"concurrent synthetic content")
            return original_open(path, mode, *args, **kwargs)
        with patch.object(setup.Path, "open", new=competing_open):
            code, out, err = self.run_setup()
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(winner.read_bytes(), b"concurrent synthetic content")
        self.assertIn("Summary: created=1 preserved=1 errors=0", out)

    def test_exclusive_create_does_not_follow_concurrent_symlink(self) -> None:
        actual = self.root / "outside"
        actual.write_bytes(b"outside synthetic content")
        probe = self.root / "probe"
        self.symlink(probe, actual)
        probe.unlink()
        winner = self.workspace / "AGENTS.md"
        original_open = Path.open
        def competing_open(path, mode="r", *args, **kwargs):
            if path == winner and mode == "xb" and not path.is_symlink():
                path.symlink_to(actual)
            return original_open(path, mode, *args, **kwargs)
        with patch.object(setup.Path, "open", new=competing_open):
            code, out, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("Conflicting path appeared", err)
        self.assertEqual(actual.read_bytes(), b"outside synthetic content")
        self.assertTrue(winner.is_symlink())
        self.assertFalse((self.workspace / "AGENT_WORKBOOK.md").exists())
        self.assert_no_onboarding(out)

    def test_override_appearing_during_final_write_prevents_onboarding(self) -> None:
        override = self.workspace / "AGENTS.override.md"
        original_open = Path.open
        def competing_open(path, mode="r", *args, **kwargs):
            if path == self.workspace / "AGENT_WORKBOOK.md" and mode == "xb":
                override.write_bytes(b"concurrent synthetic override")
            return original_open(path, mode, *args, **kwargs)
        with patch.object(setup.Path, "open", new=competing_open):
            code, out, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("Instruction override exists", err)
        self.assertEqual(override.read_bytes(), b"concurrent synthetic override")
        self.assertEqual({p.name for p in self.workspace.iterdir()},
                         {*setup.TEMPLATES, "AGENTS.override.md"})
        self.assert_no_onboarding(out)

    def test_write_failure_is_visible_and_partial_directory_is_not_filled_on_rerun(self) -> None:
        original_open = Path.open
        def failing_open(path, mode="r", *args, **kwargs):
            if path.name == "AGENT_WORKBOOK.md" and mode == "xb":
                raise PermissionError("fixture write denied")
            return original_open(path, mode, *args, **kwargs)
        with patch.object(setup.Path, "open", new=failing_open):
            code, out, err = self.run_setup()
        self.assertEqual(code, 1)
        self.assertIn("fixture write denied", err)
        self.assertIn("Summary: created=1 preserved=0 errors=1", out)
        self.assertEqual((self.workspace / "AGENTS.md").read_bytes(),
                         (setup.TEMPLATE_DIR / setup.TEMPLATES["AGENTS.md"]).read_bytes())
        self.assertFalse((self.workspace / "AGENT_WORKBOOK.md").exists())
        self.assert_no_onboarding(out)
        before = self.snapshot(self.workspace)
        code, out, err = self.run_setup()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("SKIPPED", out)
        self.assertEqual(self.snapshot(self.workspace), before)
        self.assert_no_onboarding(out)

    def test_read_only_modes_are_mutually_exclusive(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as raised:
            setup.main(["--dry-run", "--verify"])
        self.assertEqual(raised.exception.code, 2)
        self.assertFalse(self.home.exists())


if __name__ == "__main__":
    unittest.main()
