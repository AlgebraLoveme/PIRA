from __future__ import annotations

import importlib.util
import sys
import unittest
from unittest.mock import patch
from pathlib import Path


SCRIPT = Path(__file__).with_name("setup_pira.py")
SPEC = importlib.util.spec_from_file_location("pira_setup_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
setup = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = setup
SPEC.loader.exec_module(setup)


class AutoRecapTests(unittest.TestCase):
    def test_codex_login_mode_forwarded_to_tool_setup(self) -> None:
        for mode in ("auto", "browser", "device", "skip"):
            args = setup.build_parser().parse_args(["--codex-login", mode])
            state = setup.SetupState(repo_root=SCRIPT.parents[2], agent_dir=Path("/unused"),
                                     dry_run=False, yes=False)
            with patch.object(setup.subprocess, "run") as run:
                setup.configure_tools(state, None, None, verify_only=True, codex_login=args.codex_login)
                command = run.call_args.args[0]
                self.assertEqual(command[command.index("--codex-login") + 1], mode)
                self.assertIn("--verify", command)

    def test_default_preserves_settings_and_is_idempotent(self) -> None:
        import tomllib

        cases = [
            "",
            'model = "example"\n[features]\nexample = true\n',
            '[tui] # display\nnotifications = false\nauto_recap = true\n[features]\nexample = true\n',
            '[tui]\nnotifications = false',
            'tui.auto_recap = true\ntui.notifications = false\n',
        ]
        for original in cases:
            with self.subTest(original=original):
                expected = tomllib.loads(original)
                expected.setdefault("tui", {})["auto_recap"] = False
                result = setup.disable_auto_recap(original)
                self.assertEqual(tomllib.loads(result), expected)
                self.assertEqual(setup.disable_auto_recap(result), result)


if __name__ == "__main__":
    unittest.main()
