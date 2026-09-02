from __future__ import annotations

import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("setup_pira.py")
SPEC = importlib.util.spec_from_file_location("pira_setup_claude_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)

AGENTS_MD = (SCRIPT.parents[2] / "AGENTS.md").read_text(encoding="utf-8")


class SetupPiraClaudeTests(unittest.TestCase):
    def state(self, root: Path, agent_dir: Path | None = None) -> Any:
        return setup.SetupState(
            repo_root=root,
            agent_dir=agent_dir or root / "agent",
            dry_run=False,
            yes=True,
        )

    def test_managed_block_is_only_the_import(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / ".claude" / "CLAUDE.md"
            state = self.state(root)

            setup.update_claude_md(state, claude_md)
            installed = claude_md.read_text(encoding="utf-8")

            self.assertEqual(installed, setup.claude_managed_block(state.agent_dir) + "\n")
            lines = installed.splitlines()
            self.assertEqual(lines[0], setup.CLAUDE_BLOCK_START)
            self.assertEqual(lines[1], "@" + setup.claude_import_path(state.agent_dir / "AGENTS.md"))
            self.assertEqual(lines[2], setup.CLAUDE_BLOCK_END)
            self.assertEqual(len(lines), 3)

    def test_canonical_policy_states_one_shell_rule(self) -> None:
        self.assertIn("Run every other shell command with native Bash", AGENTS_MD)
        self.assertIn("Load PIRA modules with Read, never with a shell command", AGENTS_MD)
        self.assertNotIn("Every shell/exec invocation", AGENTS_MD)
        self.assertNotIn("AGENTS.override.md", AGENTS_MD)

    def test_preserves_user_content_and_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            claude_md.write_text("# My instructions\n\nKeep this.\n", encoding="utf-8")
            state = self.state(root)

            setup.update_claude_md(state, claude_md)
            first = claude_md.read_text(encoding="utf-8")
            setup.update_claude_md(state, claude_md)

            self.assertEqual(claude_md.read_text(encoding="utf-8"), first)
            self.assertTrue(first.startswith("# My instructions\n\nKeep this.\n"))
            self.assertEqual(first.count(setup.CLAUDE_BLOCK_START), 1)
            self.assertEqual(len(list(root.glob("CLAUDE.md.bak.*"))), 1)

    def test_replaces_stale_managed_block_in_place(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            stale = (
                setup.CLAUDE_BLOCK_START
                + "\n@/old/AGENTS.md\n\n## Claude Code bridge\n- old override\n"
                + setup.CLAUDE_BLOCK_END
            )
            claude_md.write_text("before\n" + stale + "\nafter\n", encoding="utf-8")
            state = self.state(root)

            setup.update_claude_md(state, claude_md)
            updated = claude_md.read_text(encoding="utf-8")

            self.assertEqual(updated, "before\n" + setup.claude_managed_block(state.agent_dir) + "\nafter\n")

    def test_rejects_unbalanced_duplicate_or_reversed_markers(self) -> None:
        cases = [
            setup.CLAUDE_BLOCK_START + "\n",
            setup.CLAUDE_BLOCK_START + "\n" + setup.CLAUDE_BLOCK_END + "\n" + setup.CLAUDE_BLOCK_START,
            setup.CLAUDE_BLOCK_END + "\n" + setup.CLAUDE_BLOCK_START + "\n" + setup.CLAUDE_BLOCK_END,
            setup.CLAUDE_BLOCK_END + "\n" + setup.CLAUDE_BLOCK_START,
        ]
        for content in cases:
            with self.subTest(content=content), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                claude_md = root / "CLAUDE.md"
                claude_md.write_text(content, encoding="utf-8")
                state = self.state(root)

                with self.assertRaisesRegex(RuntimeError, "PIRA markers"):
                    setup.update_claude_md(state, claude_md)
                with self.assertRaisesRegex(RuntimeError, "PIRA markers"):
                    setup.remove_claude_md_block(state, claude_md)
                self.assertEqual(claude_md.read_text(encoding="utf-8"), content)

    def test_rejects_non_utf8_claude_md(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            claude_md.write_bytes(b"\xff\xfe# not utf-8\n")
            state = self.state(root)

            with self.assertRaisesRegex(RuntimeError, "not valid UTF-8"):
                setup.update_claude_md(state, claude_md)
            self.assertEqual(claude_md.read_bytes(), b"\xff\xfe# not utf-8\n")

    def test_rejects_whitespace_in_agent_dir_before_writing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            state = self.state(root, agent_dir=root / "my agent")

            with self.assertRaisesRegex(RuntimeError, "whitespace"):
                setup.update_claude_md(state, claude_md)
            self.assertFalse(claude_md.exists())

    def test_verify_fails_for_whitespace_agent_dir_and_wrong_import(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            good = self.state(root)
            setup.update_claude_md(good, claude_md)

            spaced = self.state(root, agent_dir=root / "my agent")
            with redirect_stdout(io.StringIO()):
                setup.verify_claude(spaced, claude_md)
            self.assertEqual([passed for _, passed, _ in spaced.verification], [False])

            other = self.state(root, agent_dir=root / "elsewhere")
            with redirect_stdout(io.StringIO()):
                setup.verify_claude(other, claude_md)
            self.assertEqual([passed for _, passed, _ in other.verification], [False])

    def test_uninstall_removes_only_the_managed_block(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            claude_md.write_text("# Mine\n", encoding="utf-8")
            state = self.state(root)
            setup.update_claude_md(state, claude_md)

            setup.remove_claude_md_block(state, claude_md)

            self.assertEqual(claude_md.read_text(encoding="utf-8"), "# Mine\n")
            self.assertEqual(len(list(root.glob("CLAUDE.md.bak.*"))), 2)
            with redirect_stdout(io.StringIO()):
                setup.remove_claude_md_block(state, claude_md)
            self.assertEqual(claude_md.read_text(encoding="utf-8"), "# Mine\n")

    def test_uninstall_deletes_file_that_held_only_the_block(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            state = self.state(root)
            setup.update_claude_md(state, claude_md)

            setup.remove_claude_md_block(state, claude_md)

            self.assertFalse(claude_md.exists())
            self.assertEqual(len(list(root.glob("CLAUDE.md.bak.*"))), 1)

    def test_cli_installs_the_import_and_verifies(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            claude_md = Path(temporary) / ".claude" / "CLAUDE.md"
            agent_dir = Path(temporary) / "agent"
            common = ["--agent-dir", str(agent_dir), "--claude-md", str(claude_md), "--skip-tools"]
            with (
                patch.object(setup, "ensure_agent_dir"),
                patch.object(setup, "ensure_user_md"),
                patch.object(setup, "remove_legacy_files"),
                patch.object(setup, "verify"),
                redirect_stdout(io.StringIO()),
            ):
                installed = setup.main([*common, "--yes", "--user-mode", "keep", "--legacy", "keep"])
                verified = setup.main([*common, "--verify"])
                removed = setup.main([*common, "--uninstall"])

            self.assertEqual((installed, verified, removed), (0, 0, 0))
            self.assertFalse(claude_md.exists())

    def test_prompt_uses_default_when_stdin_closes(self) -> None:
        with patch("builtins.input", side_effect=EOFError), redirect_stdout(io.StringIO()):
            self.assertTrue(setup.prompt_yes_no("Continue?", default=True))
            self.assertFalse(setup.prompt_yes_no("Continue?", default=False))

    def test_cli_rejects_codex_only_options(self) -> None:
        for extra_args in (["--execution-mode", "safe"], ["--audio", "yes"], ["--codex-config", "x"], ["--claude-code"]):
            with self.subTest(extra_args=extra_args), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as raised:
                    setup.main(["--skip-tools", *extra_args])
                self.assertEqual(raised.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
