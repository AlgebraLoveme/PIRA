"""Bounded transport must preserve required notifications under healthy bursts."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import unittest

import test_team


class NotificationBurstTests(unittest.TestCase):
    def setUp(self):
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def test_required_burst_and_paced_notifications_before_and_after_start_ack(self):
        for stage, delay in (("before-ack", "0"), ("after-ack", "0"), ("after-ack", ".001")):
            with self.subTest(stage=stage, delay=delay):
                self.fixture.env.update(TEAM_BURST_AT=stage, TEAM_BURST_COUNT="128", TEAM_BURST_DELAY=delay)
                result = self.fixture.launch("burst")
                self.assertEqual(result.returncode, 0, result.stderr)
                receipt = json.loads(result.stdout)
                self.assertEqual(Path(receipt["handoff_path"]).read_text(), "ANSWER burst")
                manifest = self.fixture.metadata(receipt)
                self.assertEqual(manifest["status"], "completed")
                self.assertEqual(manifest["usage"]["input_tokens"], 128 + 17)
                self.assertEqual(manifest["usage"]["output_tokens"], 128 + 5)
                self.assertTrue(manifest["usage_complete"])
                events = [json.loads(line) for line in (Path(receipt["run_root"]) / "events.jsonl").read_text().splitlines()]
                texts = [event.get("params", {}).get("item", {}).get("text", "") for event in events]
                self.assertEqual([text for text in texts if text.startswith("burst-")],
                                 [f"burst-{index}" for index in range(128)])
                self.assertEqual(texts.count("progress"), 128)  # Filtered from delivery, never from logs.

    def test_malformed_event_after_burst_retains_latest_evidence_before_and_after_ack(self):
        for stage in ("before-ack", "after-ack"):
            with self.subTest(stage=stage):
                self.fixture.env.update(TEAM_BURST_AT=stage, TEAM_BURST_COUNT="128", TEAM_BURST_MALFORMED="1")
                result = self.fixture.launch("burst")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertIn("invalid app-server event", result.stderr)
                runs = sorted((self.fixture.root / "logs").glob("*/manifest.json"))
                run = runs[-1].parent
                manifest = json.loads((run / "manifest.json").read_text())
                self.assertEqual(manifest["status"], "failed")
                self.assertEqual(manifest["usage"]["input_tokens"], 128)
                self.assertFalse(manifest["usage_complete"])
                self.assertEqual((run / "candidate.txt").read_text(), "burst-127")
                self.assertFalse((run / "control.json").exists())

    @unittest.skipUnless(os.name == "posix", "POSIX fake-peer active notice")
    def test_interrupt_remains_available_during_required_notification_burst(self):
        self.fixture.env.update(TEAM_BURST_AT="after-ack", TEAM_BURST_COUNT="4096")
        proc = subprocess.Popen([str(self.fixture.bin), "run", "--cwd", str(self.fixture.root),
                                 "--store", str(self.fixture.root / "logs"), "--model", "test-model",
                                 "--effort", "high", "--completion-gate", "Stopped", "--task", "steerable"],
                                env=self.fixture.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            notice = self.fixture.wait_stderr_notice(proc, "pira_team active:")
            run_id = notice.split("pira_team active: ", 1)[1].split()[0]
            result = self.fixture.access("interrupt", run_id)
            self.assertEqual(result.returncode, 0, result.stderr)
            stdout, stderr = proc.communicate(timeout=8)
            self.assertNotEqual(proc.returncode, 0, stderr)
            self.assertEqual(stdout, "")
            run = self.fixture.root / "logs" / run_id
            manifest = json.loads((run / "manifest.json").read_text())
            self.assertEqual(manifest["status"], "interrupted")
            self.assertNotIn("queue overflow", manifest.get("error", ""))
            self.assertFalse((run / "control.json").exists())
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.communicate(timeout=8)
