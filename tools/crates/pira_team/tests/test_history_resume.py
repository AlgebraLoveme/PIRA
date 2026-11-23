"""Metadata-only resume through the real launcher and a non-generating peer."""
import json
from pathlib import Path
import unittest

from team_fixture import TeamFixture


class HistoryResumeTests(unittest.TestCase):
    def setUp(self):
        self.fixture = TeamFixture(self.addCleanup)

    def start(self):
        first = self.fixture.launch("retained task")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        run = Path(receipt["run_root"])
        return receipt, run, json.loads((run / "manifest.json").read_text())

    def check_resume(self, *, legacy=False):
        receipt, run, before = self.start()
        prefix = (run / "policy.md").read_bytes()
        resumed = self.fixture.access("resume", receipt["run_id"], "--task", "recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(json.loads(resumed.stdout)["run_id"], receipt["run_id"])
        after = json.loads((run / "manifest.json").read_text())
        self.assertEqual(after["thread_id"], before["thread_id"])
        self.assertEqual((run / "policy.md").read_bytes(), prefix)
        self.assertEqual(json.loads(self.fixture.access("read", receipt["run_id"]).stdout),
                         ["retained task", "recall"])
        self.assertIsNone(after["active_turn"])
        self.assertEqual(after["usage"]["input_tokens"], 34)
        self.assertEqual(after["revision_usage"]["input_tokens"], 17)
        self.assertTrue(after["usage_complete"])
        requests = [json.loads(line) for line in
                    (run / "revisions/000002/requests.jsonl").read_text().splitlines()]
        params = next(r["params"] for r in requests if r["method"] == "thread/resume")
        self.assertEqual(params["threadId"], before["thread_id"])
        if legacy:
            self.assertNotIn("excludeTurns", params)
            self.assertIn("bounded legacy resume", resumed.stderr)
        else:
            self.assertIs(params["excludeTurns"], True)
        initial = [json.loads(line) for line in (run / "requests.jsonl").read_text().splitlines()]
        self.assertNotIn("excludeTurns", next(r["params"] for r in initial if r["method"] == "thread/start"))
        # No history pagination/read is needed by this metadata-only consumer.
        self.assertFalse(any(r["method"] in ("thread/read", "thread/turns/list", "thread/items/list") for r in requests))
        events = run / "revisions/000002/events.jsonl"
        self.assertLess(events.stat().st_size, 64 * 1024)
        return run

    def test_ordinary_start_and_resume_preserve_session_and_usage(self):
        self.check_resume()

    def test_long_history_is_not_hydrated(self):
        self.fixture.env["TEAM_RESUME_HISTORY_BYTES"] = str(17 * 1024 * 1024)
        self.check_resume()

    def test_older_schema_keeps_legacy_resume(self):
        self.fixture.env["TEAM_BAD_BACKEND"] = "legacy-resume"
        self.check_resume(legacy=True)

    def test_legacy_long_history_still_enforces_frame_limit(self):
        self.fixture.env.update(TEAM_BAD_BACKEND="legacy-resume",
                                TEAM_RESUME_HISTORY_BYTES=str(17 * 1024 * 1024))
        receipt, run, before = self.start()
        resumed = self.fixture.access("resume", receipt["run_id"])
        self.assertNotEqual(resumed.returncode, 0)
        after = json.loads((run / "manifest.json").read_text())
        self.assertEqual(after["status"], "failed")
        self.assertIn("app-server event exceeds 16 MiB", after["error"])
        self.assertEqual(after["thread_id"], before["thread_id"])
        self.assertIsNone(after["active_turn"])
        self.assertEqual(after["usage"], before["usage"])
        self.assertFalse(after["usage_complete"])

    def test_missing_usage_on_metadata_only_resume_preserves_known_totals(self):
        receipt, run, before = self.start()
        self.fixture.env.update(TEAM_NO_USAGE="1", TEAM_RESUME_HISTORY_BYTES=str(17 * 1024 * 1024))
        resumed = self.fixture.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        after = json.loads((run / "manifest.json").read_text())
        self.assertEqual(after["usage"], before["usage"])
        self.assertEqual(after["revision_usage"], {})
        self.assertFalse(after["usage_complete"])
        self.assertIn("usage accounting incomplete", resumed.stderr)


if __name__ == "__main__":
    unittest.main()
