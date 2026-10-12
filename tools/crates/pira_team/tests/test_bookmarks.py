"""Management-only movable bookmarks with synthetic stores and fake native Codex."""
from __future__ import annotations

import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import unittest

import test_team


class BookmarkTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.store = self.root / "logs"
        self.index = self.store / "bookmarks.json"
        self.env = self.fixture.env
        self.env["PIRA_TEAM_DIR"] = str(self.store)
        self.management_env = dict(self.env)
        for key in ("CODEX_THREAD_ID", "CODEX_SESSION_ID", "CODEX_API_KEY", "OPENAI_API_KEY"):
            self.management_env.pop(key, None)
        self.management_env.update(CODEX_HOME=str(self.root / "no-session-or-auth"),
                                   HOME=str(self.root / "no-personal-home"))

    def cli(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.fixture.bin), *args], cwd=self.root,
                              env=self.management_env, capture_output=True, text=True, timeout=8)

    def success(self, *args: str) -> dict:
        result = self.cli(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def failure(self, *args: str) -> subprocess.CompletedProcess[str]:
        result = self.cli(*args)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        return result

    def legacy(self, run_id: str, status: str = "completed") -> Path:
        run = self.store / run_id
        (run / "artifacts").mkdir(parents=True, mode=0o700)
        (run / "artifacts/handoff").write_text("legacy handoff " + run_id)
        (run / "manifest.json").write_text(json.dumps({"run_id": run_id, "status": status,
            "artifact": "artifacts/handoff", "task": "not searchable task keywords"}))
        return run

    def reference(self, run: Path) -> dict:
        return {"run_id": run.name, "run_root": str(run)}

    def assign(self, run: str, label: str) -> dict:
        return self.success("bookmark", run, "--label", label)

    def test_no_caller_management_legacy_move_exact_equality_and_removal(self) -> None:
        empty = {"matches": [], "has_more": False}
        self.assertEqual(self.success("bookmarks"), empty)
        self.assertEqual(self.success("search", "missing"), empty)
        self.assertFalse(self.success("unbookmark", "absent")["changed"])
        self.assertFalse(self.store.exists())
        first, second = self.legacy("first"), self.legacy("second", "running")
        originals = {p: p.read_bytes() for run in (first, second) for p in run.rglob("*") if p.is_file()}
        self.assertEqual(self.success("bookmarks"), empty)
        receipt = self.assign(first.name, "Reusable")
        self.assertEqual(receipt, {"label": "Reusable", "previous": None,
                                  "current": self.reference(first), "changed": True})
        before = (self.index.read_bytes(), self.index.stat().st_mtime_ns)
        receipt = self.assign(first.name, "Reusable")
        self.assertEqual(receipt["previous"], receipt["current"])
        self.assertFalse(receipt["changed"])
        self.assertEqual((self.index.read_bytes(), self.index.stat().st_mtime_ns), before)
        self.assign(first.name, "reusable")  # Exact case-sensitive identity.
        receipt = self.assign(second.name, "Reusable")
        self.assertEqual(receipt["previous"], self.reference(first))
        self.assertEqual(receipt["current"], self.reference(second))
        self.assertEqual(self.cli("path", "@Reusable", ".").stdout.strip(), str(second))
        self.assertEqual(self.cli("read", "@reusable").stdout, "legacy handoff first")
        self.assign("@Reusable", "copy")
        self.assertEqual(self.cli("path", "@copy", ".").stdout.strip(), str(second))
        self.failure("path", "Reusable", ".")  # Plain labels never override IDs.
        removed = self.success("unbookmark", "Reusable")
        self.assertEqual(removed, {"label": "Reusable", "previous": self.reference(second),
                                   "current": None, "changed": True})
        self.assertFalse(self.success("unbookmark", "Reusable")["changed"])
        self.failure("path", "@Reusable", ".")
        self.assertEqual(self.cli("read", first.name).stdout, "legacy handoff first")
        self.assertEqual({p: p.read_bytes() for p in originals}, originals)
        other = self.root / "other-store"
        self.assertEqual(self.success("bookmarks", "--store", str(other)), empty)
        self.assertFalse(other.exists())
        self.assertEqual(len(self.success("bookmarks", "--store", "logs")["matches"]), 2)
        self.assertFalse((self.root / "no-session-or-auth").exists())
        self.assertFalse(any((run / "codex-home").exists() for run in (first, second)))

    def test_literal_case_search_order_limits_and_metadata_only_fields(self) -> None:
        run = self.legacy("run")
        for label in ("alpha ordinary", "ALPHA [X].* Λ", "Alpha [x].* λ"):
            self.assign(run.name, label)
        result = self.success("search", "[x].* λ", "--limit", "1")
        self.assertEqual(result, {"matches": [{"label": "ALPHA [X].* Λ", **self.reference(run)}],
                                  "has_more": True})
        self.assertFalse(self.success("search", "[x].* λ", "--limit", "2")["has_more"])
        self.assertEqual([r["label"] for r in self.success("search", "alpha")["matches"]],
                         ["ALPHA [X].* Λ", "Alpha [x].* λ", "alpha ordinary"])
        self.assertEqual(self.success("search", "alpha.*")["matches"], [])
        self.assertEqual(self.success("search", "task keywords")["matches"], [])
        self.assertEqual(self.success("search", "--", "--literal")["matches"], [])
        for n in range(21):
            self.assign(run.name, f"default-{n:02}")
        result = self.success("search", "default-")
        self.assertEqual(len(result["matches"]), 20)
        self.assertTrue(result["has_more"])
        self.assertEqual(len(self.success("search", "default-", "--limit", "1000")["matches"]), 21)
        self.assertTrue(self.success("bookmarks", "--limit", "1")["has_more"])
        # Listing/search are pointers, not stale classification or task/log scans.
        (run / "manifest.json").write_text("invalid manifest")
        self.assertEqual(len(self.success("search", "default-", "--limit", "1000")["matches"]), 21)

    def test_invalid_cli_and_label_boundaries_leave_index_unchanged(self) -> None:
        run = self.legacy("valid")
        self.assign(run.name, "λ" * 64)
        before = self.index.read_bytes()
        for label in ("", " ", " edge", "edge ", ".", "..", "../escape", "a/b", "a\\b", "@alias", "\x01", "λ" * 65):
            with self.subTest(label=label):
                self.failure("bookmark", run.name, "--label", label)
                self.assertEqual(self.index.read_bytes(), before)
        for args in [("bookmark", run.name), ("bookmark", "../valid", "--label", "x"),
                     ("bookmark", "@missing", "--label", "x"),
                     ("bookmark", "missing", "--label", "x"),
                     ("bookmark", run.name, "--label", "a", "--label", "b"),
                     ("bookmarks", "unexpected"), ("unbookmark",), ("search", ""),
                     ("search", "x", "--limit", "0"), ("search", "x", "--limit", "1001"),
                     ("search", "x", "--limit", "bad"), ("search", "x", "--limit", "1", "--limit", "2"),
                     ("bookmarks", "--store", ""), ("path", "@../escape", "."),
                     ("path", "@@alias", ".")]:
            with self.subTest(args=args):
                self.failure(*args)
                self.assertEqual(self.index.read_bytes(), before)

    def test_corruption_fails_all_metadata_reads_edits_without_reset_or_partial_results(self) -> None:
        run = self.legacy("valid")
        self.assign(run.name, "needle")
        invalid = [b"not JSON", b'[2,[]]', b'[1,{"needle":"valid"}]',
                   b'[1,[["needle","valid"],["needle","other"]]]',
                   b'[1,[["needle","valid"],["bad","../outside"]]]',
                   b'[1,[["needle","valid"],["bad","@needle"]]]',
                   b'[1,[["needle","valid"],["bad",7]]]', b"x" * (1024 * 1024 + 1)]
        for data in invalid:
            with self.subTest(size=len(data)):
                self.index.write_bytes(data)
                for args in [("bookmarks", "--limit", "1"), ("search", "needle", "--limit", "1"),
                             ("path", "@needle", "."), ("bookmark", run.name, "--label", "new"),
                             ("unbookmark", "needle")]:
                    result = self.failure(*args)
                    self.assertLess(len(result.stderr), 600)
                    self.assertEqual(self.index.read_bytes(), data)
                self.assertEqual(self.cli("read", run.name).stdout, "legacy handoff valid")

    def test_index_capacity_rejects_growth_without_replacement(self) -> None:
        run = self.legacy("valid")
        self.assign(run.name, "initial")
        # Near-ceiling valid input, independently serialized without implementation helpers.
        pair_bytes = len(json.dumps(["x" * 128, run.name], separators=(",", ":"))) + 1
        count = (1024 * 1024 - 8) // pair_bytes
        entries = [[f"{n:08}" + "x" * 120, run.name] for n in range(count)]
        before = json.dumps([1, entries], separators=(",", ":")).encode()
        self.assertLessEqual(len(before), 1024 * 1024)
        self.index.write_bytes(before)
        result = self.failure("bookmark", run.name, "--label", "n" * 128)
        self.assertIn("would exceed 1 MiB", result.stderr)
        self.assertEqual(self.index.read_bytes(), before)
        self.assertEqual(list(self.store.glob(".bookmarks-*.tmp")), [])
        self.assertTrue(self.success("unbookmark", entries[0][0])["changed"])
        self.assign(run.name, "n" * 128)

    @unittest.skipUnless(os.name == "posix", "POSIX no-follow/privacy/lock guards")
    def test_path_privacy_and_dangling_pointer_boundaries(self) -> None:
        run = self.legacy("valid")
        self.assign(run.name, "prior")
        original = self.index.read_bytes()
        outside = self.root / "outside.json"
        outside.write_bytes(original)
        self.index.unlink()
        self.index.symlink_to(outside)
        self.failure("bookmarks")
        self.failure("bookmark", run.name, "--label", "new")
        self.failure("unbookmark", "prior")
        self.assertEqual(outside.read_bytes(), original)
        self.index.unlink()
        os.mkfifo(self.index)
        self.failure("bookmarks")
        self.index.unlink()
        self.index.write_bytes(original)
        self.index.chmod(0o666)
        self.failure("bookmarks")
        self.failure("unbookmark", "prior")
        self.index.chmod(0o600)
        if os.geteuid() != 0:
            self.index.chmod(0o000)
            self.failure("bookmarks")
            self.failure("bookmark", run.name, "--label", "new")
            self.index.chmod(0o600)
            self.assertEqual(self.index.read_bytes(), original)
        lock_path = self.store / "bookmarks.lock"
        self.assign(run.name, "after-lock")
        lock_path.unlink()
        lock_path.symlink_to(outside)
        start = time.monotonic()
        self.failure("bookmark", run.name, "--label", "new")
        self.assertLess(time.monotonic() - start, 2)  # Unsafe lock is not contention.
        lock_path.unlink()
        alias = self.root / "store-alias"
        alias.symlink_to(self.store, target_is_directory=True)
        self.failure("bookmarks", "--store", str(alias))
        self.failure("path", "@prior", ".", "--store", str(alias))
        self.store.chmod(0o777)
        self.failure("bookmarks")
        self.store.chmod(0o700)
        (self.store / "linked-run").symlink_to(run, target_is_directory=True)
        self.failure("bookmark", "linked-run", "--label", "new")
        # A synthetically dangling mapping remains listed and may be explicitly removed.
        self.index.write_text('[1,[["dangling","missing"],["linked","linked-run"]]]')
        self.assertEqual(len(self.success("bookmarks")["matches"]), 2)
        self.failure("path", "@dangling", ".")
        self.failure("path", "@linked", ".")
        self.assertEqual(self.success("unbookmark", "dangling")["previous"]["run_id"], "missing")
        (run / "manifest.json").unlink()
        (run / "manifest.json").symlink_to(outside)
        self.failure("bookmark", run.name, "--label", "new")

    def test_concurrent_edits_and_readers_observe_atomic_snapshots(self) -> None:
        first, second, third = (self.legacy(name) for name in ("first", "second", "third"))
        self.assign(first.name, "movable")
        barrier = threading.Barrier(5)
        def writer(n: int):
            barrier.wait(timeout=5)
            return self.cli("bookmark", (first if n % 2 else second).name, "--label", f"parallel-{n}")
        def reader():
            barrier.wait(timeout=5)
            for _ in range(8):
                result = self.success("bookmarks")
                for match in result["matches"]:
                    self.assertIn(match["run_id"], (first.name, second.name, third.name))
                    self.assertEqual(match["run_root"], str(self.store / match["run_id"]))
        with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
            writers = [pool.submit(writer, n) for n in range(4)]
            read = pool.submit(reader)
            results = [future.result(timeout=10) for future in writers]
            read.result(timeout=10)
        for result in results:
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.success("bookmarks")["matches"]), 5)
        barrier = threading.Barrier(3)
        def move(run: Path):
            barrier.wait(timeout=5)
            return self.cli("bookmark", run.name, "--label", "movable")
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            futures = [pool.submit(move, run) for run in (second, third)]
            reads = pool.submit(reader)
            results = [f.result(timeout=10) for f in futures]
            reads.result(timeout=10)
        for result in results:
            self.assertEqual(result.returncode, 0, result.stderr)
        receipts = {json.loads(r.stdout)["current"]["run_id"]: json.loads(r.stdout) for r in results}
        final = Path(self.cli("path", "@movable", ".").stdout.strip())
        self.assertIn(final, (second, third))
        earlier = third if final == second else second
        self.assertEqual(receipts[earlier.name]["previous"], self.reference(first))
        self.assertEqual(receipts[final.name]["previous"], self.reference(earlier))
        self.assertTrue(all(r["changed"] for r in receipts.values()))
        self.assertEqual(len(self.success("bookmarks")["matches"]), 5)
        for run in (first, second, third):
            self.assertTrue((run / "artifacts/handoff").exists())
        self.assertEqual(list(self.store.glob(".bookmarks-*.tmp")), [])

    def start_labeled(self, label: str) -> tuple[subprocess.Popen[str], Path]:
        from test_launch_boundaries import LaunchBoundaryTests
        boundary = LaunchBoundaryTests()
        boundary.fixture, boundary.root, boundary.env = self.fixture, self.root, self.env
        self.addCleanup(boundary.doCleanups)
        proc = boundary.start(task="review", extra=("--label", label))
        notice = self.fixture.wait_stderr_notice(proc, "pira_team run_id:")
        return proc, self.store / notice.strip().split(": ", 1)[1]

    def assert_waiting_without_worker(self, proc: subprocess.Popen[str], run: Path) -> None:
        # Native bounded wait, not a product hook: a held metadata lock must keep
        # this invocation pending. An immediate try_lock failure completes here.
        try:
            out, err = proc.communicate(timeout=0.05)
        except subprocess.TimeoutExpired:
            pass
        else:
            self.fail(f"launcher ended while its bookmark lock was held: {proc.returncode}; {out}; {err}")
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "running")
        self.assertEqual(state["attempts"], [])
        self.assertFalse((run / "codex-home").exists())

    @unittest.skipUnless(os.name == "posix", "POSIX stable-lock fixture")
    def test_parallel_labeled_launches_wait_for_brief_lock_without_new_runs_or_attempts(self) -> None:
        import fcntl
        old = [self.legacy(name) for name in ("old-a", "old-b")]
        labels = ("launch-a", "launch-b")
        for run, label in zip(old, labels):
            self.assign(run.name, label)
        original = self.index.read_bytes()
        with (self.store / "bookmarks.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            active = [self.start_labeled(label) for label in labels]
            for proc, run in active:
                self.assert_waiting_without_worker(proc, run)
            self.assertEqual(self.index.read_bytes(), original)
            self.assertEqual(self.cli("path", "@launch-a", ".").stdout.strip(), str(old[0]))
            fcntl.flock(lock, fcntl.LOCK_UN)
        for label, (proc, run) in zip(labels, active):
            out, err = proc.communicate(timeout=8)
            self.assertEqual(proc.returncode, 0, err)
            self.assertEqual(json.loads(out)["run_id"], run.name)
            state = json.loads((run / "manifest.json").read_text())
            self.assertEqual(state["status"], "completed")
            self.assertEqual(state["revision"], 1)
            self.assertEqual(len(state["attempts"]), 1)
            self.assertEqual(self.cli("path", "@" + label, ".").stdout.strip(), str(run))
        self.assertEqual({p for p in self.store.iterdir() if p.is_dir()}, set(old) | {r for _, r in active})
        for run in old:
            self.assertTrue((run / "artifacts/handoff").exists())

    @unittest.skipUnless(os.name == "posix", "POSIX stable-lock fixture")
    def test_lock_budget_exhaustion_preserves_mapping_and_failed_run_without_inference(self) -> None:
        import fcntl
        old = self.legacy("old")
        self.assign(old.name, "reusable")
        original, old_manifest = self.index.read_bytes(), (old / "manifest.json").read_bytes()
        with (self.store / "bookmarks.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            start = time.monotonic()
            proc, run = self.start_labeled("reusable")
            out, err = proc.communicate(timeout=8)
            self.assertNotEqual(proc.returncode, 0)
            self.assertEqual(out, "")
            self.assertIn("busy after waiting 5 seconds", err)
            self.assertGreaterEqual(time.monotonic() - start, 5)
            self.assertEqual(self.index.read_bytes(), original)
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "failed")
        self.assertEqual(state["attempts"], [])
        self.assertFalse((run / "codex-home").exists())
        self.assertFalse((run / "requests.jsonl").exists())
        self.assertEqual((old / "manifest.json").read_bytes(), old_manifest)
        self.assertTrue((old / "artifacts/handoff").exists())
        self.assertEqual({p for p in self.store.iterdir() if p.is_dir()}, {old, run})

    @unittest.skipUnless(os.name == "posix", "POSIX installed cancellation handler")
    def test_signal_cancels_bookmark_lock_wait_before_any_worker_or_mapping_update(self) -> None:
        import fcntl
        old = self.legacy("old")
        self.assign(old.name, "reusable")
        original = self.index.read_bytes()
        with (self.store / "bookmarks.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            proc, run = self.start_labeled("reusable")
            self.assert_waiting_without_worker(proc, run)
            # Existing native controls are not installed until after attachment.
            self.assertIn("no readable control endpoint", self.failure("interrupt", run.name).stderr)
            start = time.monotonic()
            proc.terminate()
            out, err = proc.communicate(timeout=2)
            self.assertNotEqual(proc.returncode, 0)
            self.assertEqual(out, "")
            self.assertIn("interrupted while waiting for bookmark lock", err)
            self.assertLess(time.monotonic() - start, 2)
            self.assertEqual(self.index.read_bytes(), original)
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        self.assertEqual(state["attempts"], [])
        self.assertFalse((run / "codex-home").exists())

    def assert_not_worker_input(self, run: Path, markers: tuple[str, ...]) -> None:
        files = [run / name for name in ("manifest.json", "policy.md", "phase.md", "task.txt", "requests.jsonl")]
        files.extend(run.glob("revisions/*/manifest.json"))
        files.extend(run.glob("revisions/*/phase.md"))
        files.extend(run.glob("revisions/*/requests.jsonl"))
        for path in files:
            self.assertTrue(path.is_file(), str(path))
            for marker in markers:
                self.assertNotIn(marker, path.read_text(), str(path))

    def test_optional_launch_label_alias_resume_and_no_default_or_prompt_change(self) -> None:
        label = "MANAGEMENT_ONLY_67eac"
        first = self.fixture.launch("review", "--label=" + label)
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        run = Path(receipt["run_root"])
        self.assertEqual(self.cli("read", "@" + label).stdout, "ANSWER review")
        self.assertEqual(self.cli("path", "@" + label).stdout.strip(), receipt["handoff_path"])
        base = (run / "policy.md").read_bytes()
        self.assert_not_worker_input(run, (label,))
        index_before = self.index.read_bytes()
        resumed = self.fixture.access("resume", "@" + label, "recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(json.loads(resumed.stdout)["run_id"], run.name)
        self.assertEqual(json.loads(self.cli("read", "@" + label).stdout), ["review", "recall"])
        self.assertEqual(self.index.read_bytes(), index_before)
        self.assertEqual((run / "policy.md").read_bytes(), base)
        self.assert_not_worker_input(run, (label,))
        self.index.write_text("corrupt")
        plain = self.fixture.launch("review")
        self.assertEqual(plain.returncode, 0, plain.stderr)
        plain_run = Path(json.loads(plain.stdout)["run_root"])
        self.assertEqual((plain_run / "policy.md").read_bytes(), base)
        self.assertEqual(self.index.read_text(), "corrupt")
        self.assertFalse((plain_run / "comment.json").exists())
        self.assertNotEqual(self.fixture.launch("review", "--comment", "superseded").returncode, 0)
        self.failure("comment", run.name, "--text", "superseded")

    def test_startup_label_failure_contract_preserves_or_moves_at_defined_boundary(self) -> None:
        old = self.legacy("old")
        self.assign(old.name, "reusable")
        before = self.index.read_bytes()
        runs = {p for p in self.store.iterdir() if p.is_dir()}
        for flags in [("--label", "invalid/label"), ("--label", "x", "--label", "y")]:
            self.assertNotEqual(self.fixture.launch("review", *flags).returncode, 0)
            self.assertEqual(self.index.read_bytes(), before)
            self.assertEqual({p for p in self.store.iterdir() if p.is_dir()}, runs)
        self.env["TEAM_SCHEMA_FAIL"] = "1"
        result = self.fixture.launch("review", "--label", "reusable")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("native schema generation failed", result.stderr)
        new = Path(self.success("search", "reusable")["matches"][0]["run_root"])
        self.assertNotEqual(new, old)
        self.assertEqual(json.loads((new / "manifest.json").read_text())["status"], "failed")
        self.assertEqual((old / "artifacts/handoff").read_text(), "legacy handoff old")
        self.env.pop("TEAM_SCHEMA_FAIL")
        before = self.index.read_bytes()
        self.index.write_text("corrupt")
        result = self.fixture.launch("review", "--label", "reusable")
        self.assertNotEqual(result.returncode, 0)
        failed = ({p for p in self.store.iterdir() if p.is_dir()} - runs - {new}).pop()
        self.assertEqual(json.loads((failed / "manifest.json").read_text())["status"], "failed")
        self.assertFalse((failed / "codex-home").exists())
        self.assertEqual(self.index.read_text(), "corrupt")
        self.index.write_bytes(before)

    @unittest.skipUnless(os.name == "posix", "POSIX fake native controls")
    def test_running_bookmark_reassignment_and_alias_native_controls(self) -> None:
        from test_launch_boundaries import LaunchBoundaryTests
        boundary = LaunchBoundaryTests()
        boundary.fixture, boundary.root, boundary.env = self.fixture, self.root, self.env
        self.addCleanup(boundary.doCleanups)
        for operation in ("steer", "interrupt"):
            with self.subTest(operation=operation):
                rd, release, fds = boundary.gate("turn/start")
                proc = boundary.start(task="steerable", extra=("--label", "live"), pass_fds=fds)
                run = boundary.ready(proc, rd)
                before = (run / "manifest.json").read_bytes()
                self.assign(run.name, "control-alias")
                self.assertEqual((run / "manifest.json").read_bytes(), before)
                os.write(release, b"1")
                self.fixture.wait_stderr_notice(proc, "pira_team active:")
                extra = ("--task", "replacement", "--completion-gate", "Replacement complete") if operation == "steer" else ()
                result = self.success(operation, "@control-alias", *extra)
                self.assertEqual(result["run_id"], run.name)
                out, err = proc.communicate(timeout=8)
                state = json.loads((run / "manifest.json").read_text())
                if operation == "steer":
                    self.assertEqual(proc.returncode, 0, err)
                    self.assertEqual(json.loads(out)["status"], "completed")
                    self.assertEqual(self.cli("read", "@live").stdout, "ANSWER replacement")
                else:
                    self.assertNotEqual(proc.returncode, 0)
                    self.assertEqual(state["status"], "interrupted")
                before = (run / "manifest.json").read_bytes()
                self.assign(run.name, "terminal")
                self.assertEqual((run / "manifest.json").read_bytes(), before)
                self.assert_not_worker_input(run, ("control-alias",))


if __name__ == "__main__":
    unittest.main()
