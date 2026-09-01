from __future__ import annotations

import json
import os
import shlex
import shutil
import subprocess
import unittest
import uuid
from pathlib import Path

PLUGIN_ROOT = Path(__file__).resolve().parents[1]
LAUNCHER = Path("claude/pira-routing-guard/scripts/run-routing-guard.sh")
REPOSITORY = PLUGIN_ROOT.parents[1]


class RoutingLauncherTests(unittest.TestCase):
    def test_posix_launcher_accepts_hook_json_on_stdin(self) -> None:
        bash = shutil.which("bash")
        if not bash:
            self.skipTest("bash is unavailable")
        state = f"/tmp/pira-routing-launcher-{uuid.uuid4().hex}"
        try:
            environment = os.environ.copy()
            environment["PIRA_ROUTING_STATE_DIR"] = state
            completed = subprocess.run(
                [bash, LAUNCHER.as_posix()],
                cwd=REPOSITORY,
                env=environment,
                input=json.dumps(
                    {"session_id": "posix-launcher-smoke", "hook_event_name": "SessionStart"}
                ),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=20,
                check=False,
            )
        finally:
            subprocess.run(
                [bash, "-lc", f"rm -rf -- {shlex.quote(state)}"],
                capture_output=True,
                timeout=20,
                check=False,
            )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        output = json.loads(completed.stdout)
        context = output["hookSpecificOutput"]
        self.assertEqual(context["hookEventName"], "SessionStart")
        self.assertIn("pira-routing-guard:route", context["additionalContext"])


if __name__ == "__main__":
    unittest.main()
