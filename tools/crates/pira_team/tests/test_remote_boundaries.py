"""Host selection stays root-only; existing backend ancestry fixtures are synthetic.

No real SSH or model calls. These cases remain useful independently of the signed
remote wire format and exercise actual fixture parent/child subprocess ancestry.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import unittest

import test_subteams
import test_team
from test_remote_fixture import isolate_environment


class LocalRemoteLiteralTests(unittest.TestCase):
    def setUp(self):
        self.local = test_team.TeamTests()
        self.addCleanup(self.local.doCleanups)
        self.local.setUp()
        isolate_environment(self.local.env, self.local.root)

    def test_tier_default_and_resume_retention_with_remote_dispatch_enabled(self):
        self.local.parent([{"service_tier":"fast"}])
        first = self.local.launch("tier lifecycle fixture")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)

        def assert_tier(tier, backend):
            manifest = self.local.metadata(receipt)
            self.assertEqual(manifest["service_tier"], tier)
            requests = [json.loads(line) for line in
                        (Path(manifest["logs"]) / "requests.jsonl").read_text().splitlines()]
            starts = [request for request in requests if request["method"] == "turn/start"]
            self.assertEqual(len(starts), 1)
            self.assertEqual(starts[0]["params"]["serviceTierForTurn"], backend)
            return manifest

        initial = assert_tier("standard", "default")
        for extra, tier, backend in [(("--service-tier", "fast"), "fast", "fast"),
                                     ((), "fast", "fast"),
                                     (("--service-tier", "standard"), "standard", "default")]:
            with self.subTest(extra=extra):
                resumed = self.local.access("resume", receipt["run_id"], *extra)
                self.assertEqual(resumed.returncode, 0, resumed.stderr)
                manifest = assert_tier(tier, backend)
                self.assertEqual(manifest["thread_id"], initial["thread_id"])

    def test_host_option_inside_task_remains_local_task_data(self):
        task = "--host=untrusted.example; $(must-not-execute)"
        result = self.local.launch(task)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(Path(receipt["handoff_path"]).read_text(encoding="utf-8"), "ANSWER " + task)
        self.assertEqual(self.local.metadata(receipt)["task"], task)


@unittest.skipUnless(os.name == "posix", "POSIX subprocess ancestry and fake SSH fixture")
class RemoteHostSelectionTests(unittest.TestCase):
    def setUp(self):
        self.tree = test_subteams.SubteamTests()
        self.addCleanup(self.tree.doCleanups)
        self.tree.setUp()
        isolate_environment(self.tree.env, self.tree.root)
        self.transport_marker = self.tree.root / "unexpected-ssh"
        ssh = self.tree.root / "ssh"
        ssh.write_text("#!/bin/sh\nprintf unexpected > '" + str(self.transport_marker) + "'\nexit 97\n", encoding="utf-8")
        ssh.chmod(0o700)

    def test_coordinator_and_child_cannot_select_or_hop_to_host(self):
        process, coordinator = self.tree.start()
        child = self.tree.child(coordinator)
        child_id = child["run_id"]
        for caller in (coordinator, child_id):
            for operation in ("run", "read"):
                with self.subTest(caller=caller, operation=operation):
                    args = [operation]
                    args += ["--host", "fixture.invalid", "--remote-user", "fixture-user",
                             "--remote-endpoint-id", "b"*64, "--remote-exe", "/opt/pira/pira_team",
                             "--remote-backend", "/opt/codex/codex", "--remote-store", "/tmp/fixture-store",
                             "--remote-state", str(self.tree.root / "controller"),
                             "--authority-key", str(self.tree.root / "authority"), "--request-id", "request-1"]
                    if operation == "run":
                        args += ["--remote-cwd", "/tmp", "--task", "must not start",
                                 "--completion-gate", "must not start"]
                    result = self.tree.command(caller, *args)
                    self.assertNotEqual(result["returncode"], 0, result)
                    self.assertRegex(result["stderr"], "root main|nested delegation")
                    self.assertFalse(self.transport_marker.exists(), result)
        summary = self.tree.root_command("read", coordinator, "manifest.json")
        self.assertEqual(summary.returncode, 0, summary.stderr)
        self.assertEqual(len(json.loads(summary.stdout)["descendants"]["runs"]), 1)
        self.tree.finish(child_id)
        result = self.tree.request(coordinator, operation="wait", handle=child["handle"])
        self.assertEqual(result["returncode"], 0, result)
        self.tree.finish(coordinator)
        self.assertEqual(self.tree.wait(process)[0], 0)


if __name__ == "__main__":
    unittest.main()
