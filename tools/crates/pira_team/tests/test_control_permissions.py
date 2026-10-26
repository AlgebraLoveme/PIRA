"""Current caller policy must authorize new steering, but never prevent interruption."""
from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import unittest

import test_team


@unittest.skipUnless(os.name == "posix", "POSIX fake-peer control fixtures")
class ControlPermissionTests(unittest.TestCase):
    def setUp(self):
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def assert_steer_denied(self, run: Path, error: str):
        files = [run / name for name in ("manifest.json", "requests.jsonl", "controls.jsonl")]
        before = [path.read_bytes() for path in files]
        result = self.fixture.access("steer", run.name, "--task", "unauthorized replacement",
                                     "--completion-gate", "Replacement complete")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(error, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertEqual([path.read_bytes() for path in files], before)

    def test_current_policy_is_verified_without_blocking_interrupt(self):
        full = {"sandbox_policy": {"type": "danger-full-access"}}
        self.fixture.parent([full])
        proc, run, _ = self.fixture.start_control_fixture()
        contexts = [
            (self.fixture.permission_context(), "active worker execution policy"),
            ({"sandbox_policy": {"type": "read-only"}}, "read-only caller"),
            ({**full, "approval_policy": "on-request"}, "approval policy"),
            ({"sandbox_policy": None}, "malformed caller sandbox_policy"),
            ({**full, "permission_profile": {"type": "restricted"}}, "permission_profile"),
            ({**full, "file_system_sandbox_policy": {"type": "restricted"}}, "file_system_sandbox_policy"),
        ]
        for context, error in contexts:
            with self.subTest(context=context):
                self.fixture.parent([context])
                self.assert_steer_denied(run, error)
                self.assertIsNone(proc.poll())
        # Execution validation remains deliberately absent for interruption.
        interrupted = self.fixture.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        stdout, stderr = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        self.assertEqual(state["creator_main_session_id"], "parent-session")

    def test_changed_roots_network_and_temp_policy_rejected_unchanged_policy_accepted(self):
        child = self.fixture.root / "caller"
        child.mkdir()
        original = self.fixture.permission_context()
        original["cwd"] = str(child)
        original["sandbox_policy"].update(writable_roots=[str(self.fixture.root)],
                                           network_access=True, exclude_tmpdir_env_var=False,
                                           exclude_slash_tmp=False)
        self.fixture.parent([original])
        proc, run, endpoint = self.fixture.start_control_fixture()
        for field, value in (("writable_roots", []), ("network_access", False),
                             ("exclude_tmpdir_env_var", True), ("exclude_slash_tmp", True)):
            with self.subTest(field=field):
                current = copy.deepcopy(original)
                current["sandbox_policy"][field] = value
                self.fixture.parent([current])
                self.assert_steer_denied(run, "active worker execution policy")
                self.assertEqual(json.loads((run / "control.json").read_text()), endpoint)
        # Model/effort changes are not execution-policy changes.
        self.fixture.parent([{**original, "model": "another-model", "effort": "low"}])
        accepted = self.fixture.access("steer", run.name, "--task", "authorized replacement",
                                       "--completion-gate", "Replacement complete")
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        stdout, stderr = proc.communicate(timeout=8)
        self.assertEqual(proc.returncode, 0, stderr)
        self.assertEqual(Path(json.loads(stdout)["handoff_path"]).read_text(), "ANSWER authorized replacement")

    def test_policy_check_is_bound_to_endpoint_not_mutable_manifest(self):
        proc, run, endpoint = self.fixture.start_control_fixture()
        path = run / "manifest.json"
        manifest = json.loads(path.read_text())
        manifest["execution_permissions"]["sandbox_policy"] = {"type": "dangerFullAccess"}
        path.write_text(json.dumps(manifest))
        self.fixture.parent([{"sandbox_policy": {"type": "danger-full-access"}}])
        self.assert_steer_denied(run, "active worker execution policy")
        # Old/unverifiable endpoints fail closed for steer, not for interrupt.
        endpoint.pop("sandbox_policy")
        (run / "control.json").write_text(json.dumps(endpoint))
        self.assert_steer_denied(run, "active worker execution policy")
        interrupted = self.fixture.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0)
