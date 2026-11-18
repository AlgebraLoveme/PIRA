"""Retained outcomes remain truthful and recoverable after rejected revisions."""
import json
import subprocess
import unittest

from team_fixture import TeamFixture


class RetainedOutcomeTests(unittest.TestCase):
    def setUp(self):
        self.f = TeamFixture(self.addCleanup)

    def test_unverified_settlement_never_retains_completed(self):
        # Only the temporary peer is changed. The watcher still reads a healthy
        # tree; the final settlement cannot open its lock after validation.
        peer = self.f.root / "codex"
        original = peer.read_text()
        needle = '    notify("turn/completed", {"turn": turn})'
        self.assertEqual(original.count(needle), 1)
        peer.write_text(original.replace(needle, '''    if status == "completed" and task == "settlement-failure":
        lock = home.parent / "tree.lock"
        lock.unlink()
        lock.mkdir()
''' + needle))
        process, active_run, _ = self.f.start_control_fixture(extra=("--allow-subteam",))
        steered = self.f.access("steer", active_run.name,
                                "--task", "settlement-failure")
        self.assertEqual(steered.returncode, 0, steered.stderr)
        stdout, stderr = process.communicate(timeout=10)
        self.assertNotEqual(process.returncode, 0, stdout)
        self.assertIn("open subtree lock", stderr)
        run, = (self.f.root / "logs").glob("*/manifest.json")
        current = json.loads(run.read_text())
        self.assertEqual(current["attempts"][-1]["status"], "validated")
        self.assertEqual(current["status"], "failed")
        self.assertIn("open subtree lock", current["error"])
        snapshot = run.parent / "revisions/000001/manifest.json"
        self.assertEqual(json.loads(snapshot.read_text()), current)
        self.assertEqual(current["revisions"][-1]["status"], "failed")
        waited = self.f.access("wait", current["run_id"])
        self.assertEqual(waited.returncode, 0, waited.stderr)
        self.assertEqual(json.loads(waited.stdout)["status"], "failed")

    def test_oversized_encoded_resume_preserves_prior_revision(self):
        first = self.f.launch("initial")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        run = self.f.root / "logs" / receipt["run_id"]
        before = {name: (run / name).read_bytes() for name in
                  ("manifest.json", "tree.json", "revisions/000001/manifest.json")}
        task = self.f.root / "replacement.txt"
        # Raw UTF-8 fits under 16 MiB; JSON escaping alone crosses the limit.
        task.write_text("\t" * (8 * 1024 * 1024))
        # Whitespace-only tasks are invalid independently of their encoded size.
        with task.open("a") as stream:
            stream.write("x")
        rejected = subprocess.run(
            [str(self.f.bin), "resume", receipt["run_id"], "--task-file", str(task),
             "--completion-gate", "Report actual checks", "--store", str(self.f.root / "logs")],
            env=self.f.env, capture_output=True, text=True, timeout=45)
        self.assertNotEqual(rejected.returncode, 0, rejected.stdout)
        self.assertIn("run metadata exceeds 16 MiB", rejected.stderr)
        for name, content in before.items():
            self.assertEqual((run / name).read_bytes(), content, name)
        self.assertFalse((run / "revisions/000002").exists())
        self.assertFalse(list(run.glob("manifest.*.tmp")))
        artifact = self.f.access("read", receipt["run_id"])
        self.assertEqual(artifact.returncode, 0, artifact.stderr)
        self.assertEqual(artifact.stdout, "ANSWER initial")
        resumed = self.f.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(json.loads((run / "manifest.json").read_text())["revision"], 2)


if __name__ == "__main__":
    unittest.main()
