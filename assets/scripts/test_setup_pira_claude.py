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


class SetupPiraClaudeTests(unittest.TestCase):
    def state(self, root: Path) -> Any:
        return setup.SetupState(
            repo_root=root,
            agent_dir=root / "agent",
            dry_run=False,
            yes=True,
        )

    def test_adds_one_agents_import_and_thin_bridge(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / ".claude" / "CLAUDE.md"
            state = self.state(root)

            setup.update_claude_md(state, claude_md)
            installed = claude_md.read_text(encoding="utf-8")

            self.assertEqual(
                installed,
                setup.claude_managed_block(state.agent_dir) + "\n",
            )
            self.assertIn(
                "@" + setup.claude_import_path(state.agent_dir / "AGENTS.md"),
                installed,
            )
            self.assertEqual(installed.count("@"), 1)
            self.assertIn("load all required modules via `pira_ctx exact`", installed)
            self.assertIn("Never invoke Bash directly", installed)

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
            self.assertIn("# My instructions\n\nKeep this.\n", first)
            self.assertEqual(first.count(setup.CLAUDE_BLOCK_START), 1)
            self.assertEqual(len(list(root.glob("CLAUDE.md.bak.*"))), 1)

    def test_replaces_only_existing_managed_block(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            claude_md.write_text(
                "before\n"
                + setup.CLAUDE_BLOCK_START
                + "\n@/old/AGENTS.md\n"
                + setup.CLAUDE_BLOCK_END
                + "\nafter\n",
                encoding="utf-8",
            )
            state = self.state(root)

            setup.update_claude_md(state, claude_md)
            updated = claude_md.read_text(encoding="utf-8")

            self.assertTrue(updated.startswith("before\n"))
            self.assertTrue(updated.endswith("\nafter\n"))
            self.assertNotIn("@/old/AGENTS.md", updated)

    def test_rejects_unbalanced_or_duplicate_markers(self) -> None:
        cases = [
            setup.CLAUDE_BLOCK_START + "\n",
            setup.CLAUDE_BLOCK_START
            + "\n"
            + setup.CLAUDE_BLOCK_END
            + "\n"
            + setup.CLAUDE_BLOCK_START,
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
                self.assertEqual(claude_md.read_text(encoding="utf-8"), content)

    def test_rejects_non_utf8_content_without_modifying_it(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            claude_md = root / "CLAUDE.md"
            original = b"\xff\xfe\x00"
            claude_md.write_bytes(original)
            state = self.state(root)

            with self.assertRaisesRegex(RuntimeError, "not valid UTF-8"):
                setup.update_claude_md(state, claude_md)
            with self.assertRaisesRegex(RuntimeError, "not valid UTF-8"):
                setup.verify_claude(state, claude_md)

            self.assertEqual(claude_md.read_bytes(), original)
            self.assertEqual(list(root.glob("CLAUDE.md.bak.*")), [])

    def test_cli_installs_only_the_managed_bridge(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            claude_md = Path(temporary) / ".claude" / "CLAUDE.md"
            agent_dir = Path(temporary) / "agent"
            with (
                patch.object(setup, "ensure_agent_dir"),
                patch.object(setup, "ensure_user_md"),
                patch.object(setup, "remove_legacy_files"),
                patch.object(setup, "verify"),
                redirect_stdout(io.StringIO()),
            ):
                result = setup.main(
                    [
                        "--claude-code",
                        "--agent-dir",
                        str(agent_dir),
                        "--claude-md",
                        str(claude_md),
                        "--yes",
                        "--skip-tools",
                        "--user-mode",
                        "keep",
                        "--legacy",
                        "keep",
                    ]
                )

            self.assertEqual(result, 0)
            self.assertEqual(
                claude_md.read_text(encoding="utf-8"),
                setup.claude_managed_block(agent_dir) + "\n",
            )

    def test_cli_rejects_codex_only_modes_before_writing(self) -> None:
        cases = [["--execution-mode", "safe"], ["--audio", "yes"]]
        for extra_args in cases:
            with self.subTest(extra_args=extra_args), tempfile.TemporaryDirectory() as temporary:
                claude_md = Path(temporary) / "CLAUDE.md"
                with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    result = setup.main(
                        [
                            "--claude-code",
                            "--claude-md",
                            str(claude_md),
                            "--skip-tools",
                            *extra_args,
                        ]
                    )

                self.assertEqual(result, 1)
                self.assertFalse(claude_md.exists())


if __name__ == "__main__":
    unittest.main()
