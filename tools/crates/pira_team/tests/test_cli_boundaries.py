"""Shared argv boundaries: synthetic local turns and offline signed remote requests."""
import json
import os
import unittest

from team_fixture import TeamFixture
from test_remote_fixture import RemoteFixture


class CliBoundaryTests(unittest.TestCase):
    def test_local_run_and_resume_keep_flag_looking_task_values(self):
        fixture = TeamFixture(self.addCleanup)
        flags = ("--inject-write", "--inject-fig")
        for task in ("--allow-subteam", "--host=literal-task"):
            with self.subTest(task=task):
                result = fixture.launch(task, *flags)
                self.assertEqual(result.returncode, 0, result.stderr)
                run = json.loads(result.stdout)["run_id"]
                self.assertEqual(fixture.access("read", run).stdout, "ANSWER " + task)
                result = fixture.access("resume", run, *flags, "--task=" + task)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(fixture.access("read", run).stdout, "ANSWER " + task)

    @unittest.skipUnless(os.name == "posix", "POSIX offline SSH fixture")
    def test_remote_flags_leave_task_data_out_of_option_scanning(self):
        fixture = RemoteFixture(self)
        result = fixture.keygen()
        self.assertEqual(result.returncode, 0, result.stderr)
        fixture.completed_reply()
        for index, task in enumerate(("--allow-subteam", "--host=literal-task")):
            with self.subTest(task=task):
                result = fixture.invoke("run", *fixture.client_options("boundary-" + str(index)),
                    "--remote-cwd", fixture.cwd, "--detach", "--inject-write", "--inject-fig",
                    "--completion-gate", "Report fixture", "--task", task)
                self.assertEqual(result.returncode, 0, result.stderr)
                args = fixture.grants("submit")[-1]["args"]
                self.assertEqual(args[args.index("--task") + 1], task)
                self.assertIn("--inject-write", args)
                self.assertIn("--inject-fig", args)


if __name__ == "__main__":
    unittest.main()
