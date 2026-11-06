"""Synthetic stores only. Native subprocess lifecycle, no inference or real cleanup."""
import concurrent.futures
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import time
import unittest

from native_fixture import process_exit_waiter
import test_subteams


class RetentionTests(unittest.TestCase):
    setUp = test_subteams.SubteamTests.setUp
    parent = test_subteams.SubteamTests.parent
    permission_context = test_subteams.SubteamTests.permission_context
    fail_startup = test_subteams.SubteamTests.fail_startup
    wait_stderr_notice = test_subteams.SubteamTests.wait_stderr_notice
    launch = test_subteams.SubteamTests.launch
    stop_processes = test_subteams.SubteamTests.stop_processes
    root_command = test_subteams.SubteamTests.root_command
    start = test_subteams.SubteamTests.start
    request = test_subteams.SubteamTests.request
    command = test_subteams.SubteamTests.command
    manifest = test_subteams.SubteamTests.manifest
    finish = test_subteams.SubteamTests.finish
    wait = test_subteams.SubteamTests.wait

    def preview(self):
        result = self.root_command("cleanup", "--dry-run", "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def expired(self):
        store = self.root / "logs"
        store.mkdir(exist_ok=True)
        old = store / "expired"
        old.mkdir(mode=0o700)
        (old / "run.lock").touch(mode=0o600)
        (old / "manifest.json").write_text(json.dumps({
            "run_id": "expired", "revision": 1, "status": "failed", "parent_run_id": None,
            "last_execution_activity_ms": time.time_ns() // 1_000_000 - 864_000_001}))
        return old

    def test_resume_refreshes_execution_reads_and_previews_do_not(self):
        first = self.launch("initial")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        run = Path(receipt["run_root"])
        initial = self.manifest(run.name)["last_execution_activity_ms"]
        before = (run / "manifest.json").read_bytes()
        self.assertEqual(self.root_command("read", run.name, "manifest.json").returncode, 0)
        self.preview()
        self.assertEqual((run / "manifest.json").read_bytes(), before)
        result = self.root_command("resume", run.name, "--task", "recall", "--completion-gate", "retain session")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertGreater(self.manifest(run.name)["last_execution_activity_ms"], initial)
        self.assertEqual(self.manifest(run.name)["revision"], 2)

    def test_live_coordinator_and_child_stay_one_protected_unit(self):
        process, parent = self.start()
        child = self.command(parent, "run", "--task", "subteam-service", "--completion-gate", "finish", operation="start")
        self.assertIn("run_id", child)
        preview = self.preview()
        unit, = [u for u in preview["units"] if u["root_id"] == parent]
        self.assertEqual(unit["member_count"], 2)
        self.assertIsNotNone(unit["skip"])
        self.assertEqual(unit["reasons"], [])
        self.finish(child["run_id"])
        self.assertEqual(self.command(parent, operation="wait", handle=child["handle"])["returncode"], 0)
        self.finish(parent)
        self.assertEqual(self.wait(process)[0], 0)
        unit, = [u for u in self.preview()["units"] if u["root_id"] == parent]
        self.assertEqual(unit["member_count"], 2)
        self.assertIsNone(unit["skip"], unit)

    def test_pending_partial_deletion_dry_run_is_same_plan_and_immutable(self):
        old = self.expired()
        manifest = json.loads((old / "manifest.json").read_text())
        record = {"version": 2, "root_id": "expired", "members": ["expired"],
                  "last_activity_ms": manifest["last_execution_activity_ms"], "reasons": ["age"],
                  "proofs": {"expired": {"revision": 1, "status": "failed", "runtime": None}}}
        (old.parent / ".retiring-expired.json").write_text(json.dumps(record))
        (old / "manifest.json").unlink()
        (old / "remaining-payload").write_text("synthetic")
        def snapshot():
            return {str(p.relative_to(old.parent)): (p.stat().st_mtime_ns, p.read_bytes() if p.is_file() else None)
                    for p in [old.parent, *old.parent.rglob("*")]}
        before = snapshot()
        preview = self.preview()
        self.assertEqual(snapshot(), before)
        unit, = preview["units"]
        self.assertEqual(unit["root_id"], "expired")
        self.assertTrue(unit["pending_recovery"])
        self.assertEqual(unit["reasons"], ["age"])
        actual = self.root_command("cleanup", "--json")
        self.assertEqual(actual.returncode, 0, actual.stderr)
        actual = json.loads(actual.stdout)
        self.assertEqual(actual["units"], preview["units"])
        self.assertTrue(actual["outcomes"][0]["deleted"], actual)
        self.assertFalse(old.exists())

    @unittest.skipUnless(os.name == "posix", "host suspension timing check uses SIGSTOP and native flock")
    def test_stalled_automatic_cleaner_never_blocks_creation_or_return_and_outlives_origin(self):
        import fcntl
        old = self.expired()
        store = old.parent
        bookmark_lock = (store / "bookmarks.lock").open("w+")
        fcntl.flock(bookmark_lock, fcntl.LOCK_EX)
        self.addCleanup(bookmark_lock.close)
        origin, run = self.start(allow=False)
        marker = store / ".retiring-expired.json"
        deadline = time.monotonic() + 4
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.002)
        self.assertTrue(marker.exists(), "automatic cleaner did not reach controlled bookmark boundary")
        children = subprocess.run(["pgrep", "-P", str(origin.pid)], capture_output=True, text=True, check=True).stdout.split()
        cleaners = [int(pid) for pid in children if Path(subprocess.check_output(
            ["ps", "-o", "comm=", "-p", pid], text=True).strip()).name == self.bin.name]
        self.assertEqual(len(cleaners), 1, cleaners)
        cleaner = cleaners[0]
        exited = process_exit_waiter(cleaner)
        os.kill(cleaner, signal.SIGSTOP)
        fcntl.flock(bookmark_lock, fcntl.LOCK_UN)
        def reap():
            try:
                os.kill(cleaner, signal.SIGCONT)
            except ProcessLookupError:
                pass
        self.addCleanup(reap)
        self.finish(run)
        returned = time.monotonic()
        self.assertEqual(self.wait(origin)[0], 0)
        origin_return = time.monotonic() - returned
        os.kill(cleaner, 0)  # Still suspended, after its originating launcher exited.
        self.assertTrue(old.exists())
        self.assertFalse((store / ".cleanup-report.json").exists())
        # No work or report can happen in competing triggers while this owner is stopped.
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            replies = list(pool.map(lambda _: self.root_command("cleanup", "--json"), range(4)))
        for reply in replies:
            self.assertEqual(reply.returncode, 0, reply.stderr)
            self.assertEqual(json.loads(reply.stdout), {"skipped": "cleaner_already_active"})
        candidate = self.bin
        baseline = Path(os.environ.get("PIRA_TEAM_BASELINE_BIN", str(candidate)))
        times = {"baseline": [], "candidate_stalled": []}
        try:
            for _ in range(3):
                for label, binary in [("baseline", baseline), ("candidate_stalled", candidate)]:
                    self.bin = binary
                    start = time.monotonic()
                    process, new_run = self.start(allow=False)
                    created = time.monotonic()
                    self.finish(new_run)
                    self.assertEqual(self.wait(process)[0], 0)
                    finished = time.monotonic()
                    times[label].append({"creation_seconds": created-start, "return_after_finish_seconds": finished-created,
                                         "total_seconds": finished-start})
                    os.kill(cleaner, 0)
            # Wide timing tolerance guards scheduling noise. The decisive property
            # is that every foreground process exited while the cleaner stayed stopped.
            for metric in ("creation_seconds", "return_after_finish_seconds"):
                base = statistics.median(t[metric] for t in times["baseline"])
                enabled = statistics.median(t[metric] for t in times["candidate_stalled"])
                self.assertLess(enabled, base + 2.0, times)
            print("RETENTION_LATENCY " + json.dumps({"baseline_binary": str(baseline), "origin_return_seconds": origin_return,
                  "samples": times, "cleaner_survived_origin": True, "concurrent_triggers_skipped": 4}), flush=True)
        finally:
            self.bin = candidate
            reap()
            exited()
        self.assertFalse(old.exists(), "released cleaner did not finish its synthetic deletion")
        self.assertTrue((store / ".cleanup-report.json").exists())

    @unittest.skipUnless(os.name == "posix", "controlled abandoned cleaner uses a native bookmark lock")
    def test_abandoned_automatic_cleaner_releases_store_lock_and_next_trigger_recovers(self):
        import fcntl
        old = self.expired()
        with (old.parent / "bookmarks.lock").open("w+") as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            origin, run = self.start(allow=False)
            deadline = time.monotonic() + 4
            while not (old.parent / ".retiring-expired.json").exists() and time.monotonic() < deadline:
                time.sleep(0.002)
            self.assertTrue((old.parent / ".retiring-expired.json").exists())
            children = subprocess.check_output(["pgrep", "-P", str(origin.pid)], text=True).split()
            cleaners = [int(pid) for pid in children if Path(subprocess.check_output(
                ["ps", "-o", "comm=", "-p", pid], text=True).strip()).name == self.bin.name]
            self.assertEqual(len(cleaners), 1)
            exited = process_exit_waiter(cleaners[0])
            os.kill(cleaners[0], signal.SIGKILL)
            exited()
        result = self.root_command("cleanup", "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["outcomes"][0]["deleted"], result.stdout)
        self.assertFalse(old.exists())
        self.finish(run)
        self.assertEqual(self.wait(origin)[0], 0)
