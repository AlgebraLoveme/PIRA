"""Adversarial Team-interface isolation with verified fake main sessions, never OS security."""
from __future__ import annotations

import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import unittest

import test_team


class SessionIsolationTests(unittest.TestCase):
    def setUp(self):
        self.fixture = test_team.TeamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.store = self.root / "logs"
        self.a = dict(self.fixture.env, PIRA_TEAM_DIR=str(self.store))
        self.b = self.session("second-main")
        self.index = self.store / "bookmarks.json"

    def session(self, identity):
        home = Path(self.fixture.env["CODEX_HOME"])
        path = home / "sessions" / ("rollout-test-" + identity + ".jsonl")
        records = [{"type": "session_meta", "payload": {"id": identity}},
                   {"type": "turn_context", "payload": self.fixture.permission_context()}]
        path.write_text("".join(json.dumps(r) + "\n" for r in records))
        return dict(self.fixture.env, CODEX_THREAD_ID=identity, PIRA_TEAM_DIR=str(self.store))

    def cli(self, env, *args, explicit=True, cwd=None):
        return subprocess.run([str(self.fixture.bin), *args,
            *(["--store", str(self.store)] if explicit else [])],
            env=env, cwd=cwd or self.root, capture_output=True, text=True, timeout=8)

    def success(self, env, *args, **kwargs):
        result = self.cli(env, *args, **kwargs)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def launch(self, env, label):
        return self.success(env, "run", "--cwd", str(self.root), "--model", "test-model",
                            "--effort", "high", "--completion-gate", "Report fixture", "--task", "review",
                            "--label", label)

    def snapshot(self):
        return {str(p.relative_to(self.store)): (p.stat().st_mode, p.read_bytes() if p.is_file() else None)
                for p in self.store.rglob("*")}

    def denied(self, env, *args, **kwargs):
        before = self.snapshot()
        result = self.cli(env, *args, **kwargs)
        self.assertNotEqual(result.returncode, 0, (args, result.stdout, result.stderr))
        self.assertEqual(result.stdout, "")
        self.assertEqual(self.snapshot(), before, args)
        return result

    def foreign_attempts(self, env, reference):
        for args in [("read", reference), ("read", reference, "manifest.json"),
                     ("read", reference, "stderr.log"), ("path", reference),
                     ("path", reference, "."), ("path", reference, "manifest.json"),
                     ("resume", reference), ("resume", reference, "--task", "take over", "--completion-gate", "take over"),
                     ("steer", reference, "--task", "take over", "--completion-gate", "take over"),
                     ("interrupt", reference), ("bookmark", reference, "--label", "stolen")]:
            for explicit in (True, False):
                with self.subTest(args=args, explicit=explicit):
                    result = self.denied(env, *args, explicit=explicit)
                    self.assertNotIn(str(self.store), result.stderr)
                    self.assertNotIn("fixture-thread", result.stderr)
                    if not reference.startswith("@"):
                        self.assertNotIn(reference, result.stderr)

    def test_two_sessions_all_paths_same_labels_and_cross_workspace(self):
        runs_a = [self.launch(self.a, label) for label in ("shared", "a-only")]
        runs_b = [self.launch(self.b, label) for label in ("shared", "b-only")]
        for own, other, own_runs, other_runs, foreign_label in [
            (self.a, self.b, runs_a, runs_b, "b-only"),
            (self.b, self.a, runs_b, runs_a, "a-only")]:
            listed = self.success(own, "bookmarks")
            self.assertEqual({m["run_id"] for m in listed["matches"]}, {r["run_id"] for r in own_runs})
            self.assertFalse(listed["has_more"])
            self.assertFalse(self.success(own, "bookmarks", "--limit", "2")["has_more"])
            self.assertTrue(self.success(own, "bookmarks", "--limit", "1")["has_more"])
            self.assertEqual(len(self.success(own, "search", "shared", "--limit", "1")["matches"]), 1)
            self.assertFalse(self.success(own, "search", "shared", "--limit", "1")["has_more"])
            self.assertEqual(self.success(own, "search", foreign_label)["matches"], [])
            self.assertEqual(self.cli(own, "path", "@shared", ".").stdout.strip(), own_runs[0]["run_root"])
            before = self.snapshot()
            self.assertFalse(self.success(own, "unbookmark", foreign_label)["changed"])
            self.assertEqual(self.snapshot(), before)
            for foreign in other_runs:
                self.foreign_attempts(own, foreign["run_id"])
                missing = self.denied(own, "path", "nonexistent-id", ".")
                actual = self.denied(own, "path", foreign["run_id"], ".")
                self.assertEqual(actual.stderr, missing.stderr)
            self.foreign_attempts(own, "@" + foreign_label)
            copied = self.success(own, "bookmark", "@shared", "--label", "copy")
            self.assertEqual(copied["current"]["run_id"], own_runs[0]["run_id"])
        other_workspace = self.root / "other-workspace"
        other_workspace.mkdir()
        original = self.success(self.a, "bookmarks")
        # Change the verified caller's recorded workspace too, not only shell cwd.
        context = self.fixture.permission_context()
        context["cwd"] = str(other_workspace)
        context["sandbox_policy"]["writable_roots"] = [str(self.root)]
        parent_log = Path(self.a["CODEX_HOME"]) / "sessions/2026/10/01/rollout-2026-10-01-parent-session.jsonl"
        with parent_log.open("a") as stream:
            stream.write(json.dumps({"type":"turn_context", "payload":context}) + "\n")
        self.assertEqual(self.success(self.a, "bookmarks", cwd=other_workspace), original)
        self.assertEqual(self.cli(self.a, "read", runs_a[0]["run_id"], cwd=other_workspace).stdout, "ANSWER review")
        for ref in (runs_a[0]["run_id"], "@shared"):
            self.success(self.a, "resume", ref)
        run = Path(runs_a[0]["run_root"])
        for path in [run / "manifest.json", *run.glob("revisions/*/manifest.json")]:
            manifest = json.loads(path.read_text())
            self.assertEqual(manifest["creator_main_session_id"], "parent-session")
            self.assertEqual(manifest["execution_permissions"]["caller_thread_id"], "parent-session")
            self.assertNotEqual(manifest["thread_id"], manifest["creator_main_session_id"])
        self.foreign_attempts(self.b, runs_a[0]["run_id"])
        native = self.session("fixture-thread")
        self.denied(native, "read", runs_a[0]["run_id"], "manifest.json")
        self.denied(native, "resume", runs_a[0]["run_id"])

    def test_missing_invalid_and_unverified_identity_fail_closed_without_writes(self):
        run = self.launch(self.a, "shared")
        invalid = [dict(self.a, CODEX_THREAD_ID=""), dict(self.a, CODEX_THREAD_ID="bad/id"),
                   dict(self.a, CODEX_THREAD_ID="unrecorded")]
        missing = dict(self.a)
        missing.pop("CODEX_THREAD_ID")
        invalid.append(missing)
        mismatch = self.session("mismatch")
        log = Path(mismatch["CODEX_HOME"]) / "sessions/rollout-test-mismatch.jsonl"
        log.write_text(json.dumps({"type":"session_meta", "payload":{"id":"another"}}) + "\n")
        invalid.append(mismatch)
        no_turn = self.session("no-turn")
        (Path(no_turn["CODEX_HOME"]) / "sessions/rollout-test-no-turn.jsonl").write_text(
            json.dumps({"type":"session_meta", "payload":{"id":"no-turn"}}) + "\n")
        invalid.append(no_turn)
        for env in invalid:
            self.foreign_attempts(env, run["run_id"])
            self.foreign_attempts(env, "@shared")
            for args in [("bookmarks",), ("search", "shared"), ("unbookmark", "shared"),
                         ("run", "--task", "review", "--completion-gate", "report", "--model", "test-model", "--effort", "high")]:
                self.denied(env, *args)
            absent = self.root / "absent-store"
            self.denied(env, "bookmarks", "--store", str(absent), explicit=False)
            self.denied(env, "unbookmark", "missing", "--store", str(absent), explicit=False)
            self.assertFalse(absent.exists())

    def test_legacy_original_evidence_not_latest_execution_and_ambiguous_preservation(self):
        receipt = self.launch(self.a, "shared")
        self.success(self.a, "resume", receipt["run_id"])
        run = Path(receipt["run_root"])
        for path in [run / "manifest.json", *run.glob("revisions/*/manifest.json")]:
            manifest = json.loads(path.read_text())
            manifest.pop("creator_main_session_id")
            if manifest["revision"] != 1:
                manifest["execution_permissions"]["caller_thread_id"] = "second-main"
            path.write_text(json.dumps(manifest))
        self.foreign_attempts(self.b, run.name)
        self.assertEqual(self.cli(self.a, "read", run.name).returncode, 0)
        self.success(self.a, "resume", run.name)
        self.assertEqual(json.loads((run / "manifest.json").read_text())["creator_main_session_id"], "parent-session")
        # Current execution evidence without an original snapshot is never sufficient.
        ambiguous = self.store / "ambiguous"
        ambiguous.mkdir()
        (ambiguous / "manifest.json").write_text(json.dumps({"run_id": "ambiguous", "revision": 1,
            "status": "completed", "thread_id": "parent-session", "revisions": [],
            "execution_permissions": {"source":"verified-caller-turn-context", "caller_thread_id":"parent-session"}}))
        for env in (self.a, self.b):
            self.foreign_attempts(env, "ambiguous")
        manifest = json.loads((ambiguous / "manifest.json").read_text())
        manifest["status"] = "running"
        (ambiguous / "manifest.json").write_text(json.dumps(manifest))
        (ambiguous / "run.lock").write_text("")
        self.foreign_attempts(self.a, "ambiguous")  # Unlocked orphan, not active initial invocation.
        # Corrupt/missing original identity and invalid immutable identity fail closed.
        legacy = run / "revisions/000001/manifest.json"
        state = json.loads((run / "manifest.json").read_text())
        state.pop("creator_main_session_id")
        (run / "manifest.json").write_text(json.dumps(state))
        for data in ["{invalid", json.dumps({"revision": 1, "run_id": run.name}),
                     json.dumps({"revision": 2, "run_id": run.name,
                         "creator_main_session_id": "parent-session"})]:
            legacy.write_text(data)
            self.foreign_attempts(self.a, run.name)
        state["creator_main_session_id"] = "bad/id"
        (run / "manifest.json").write_text(json.dumps(state))
        self.foreign_attempts(self.a, run.name)

    def test_legacy_tags_preserved_and_inaccessible_pointer_receipts_redacted(self):
        a = self.launch(self.a, "a-only")
        b = self.launch(self.b, "b-only")
        legacy = [["global-only", a["run_id"]], ["shared", b["run_id"]]]
        self.index.write_text(json.dumps([1, legacy]))
        before = self.snapshot()
        for env in (self.a, self.b):
            self.assertEqual(self.success(env, "bookmarks")["matches"], [])
            self.assertEqual(self.success(env, "search", "global")["matches"], [])
            self.assertFalse(self.success(env, "unbookmark", "global-only")["changed"])
            self.foreign_attempts(env, "@global-only")
        self.assertEqual(self.snapshot(), before)
        self.success(self.a, "bookmark", a["run_id"], "--label", "shared")
        self.success(self.b, "bookmark", b["run_id"], "--label", "shared")
        self.assertEqual(json.loads(self.index.read_text())[2], legacy)
        # An inconsistent scoped pointer cannot leak foreign details through any consumer.
        document = json.loads(self.index.read_text())
        document[1].append(["parent-session", "poisoned", b["run_id"]])
        self.index.write_text(json.dumps(document))
        self.assertNotIn("poisoned", [m["label"] for m in self.success(self.a, "bookmarks")["matches"]])
        self.foreign_attempts(self.a, "@poisoned")
        removed = self.success(self.a, "unbookmark", "poisoned")
        self.assertIsNone(removed["previous"])
        document[1].append(["parent-session", "poisoned-again", b["run_id"]])
        self.index.write_text(json.dumps(document))
        assigned = self.success(self.a, "bookmark", a["run_id"], "--label", "poisoned-again")
        self.assertIsNone(assigned["previous"])
        self.assertNotIn(b["run_id"], json.dumps(assigned))
        self.assertEqual(json.loads(self.index.read_text())[2], legacy)

    def test_v2_corruption_across_foreign_namespace_never_drops_or_exposes_entries(self):
        a = self.launch(self.a, "shared")
        invalid = [
            [2, [["parent-session", "shared", a["run_id"]], ["parent-session", "shared", a["run_id"]]], []],
            [2, [["second-main", "bad", "../run"]], []],
            [2, [["bad/id", "shared", a["run_id"]]], []],
            [2, [[None, "shared", a["run_id"]]], []],
            [2, [["second-main", "bad/label", a["run_id"]]], []],
            [2, [], [["global", a["run_id"]], ["global", a["run_id"]]]],
            [2, {}, []],
        ]
        for value in invalid:
            self.index.write_text(json.dumps(value))
            for env in (self.a, self.b):
                for args in [("bookmarks", "--limit", "1"), ("search", "shared"),
                             ("unbookmark", "shared"), ("bookmark", a["run_id"], "--label", "shared"),
                             ("path", "@shared", ".")]:
                    self.denied(env, *args)
            self.assertEqual(self.cli(self.a, "read", a["run_id"]).returncode, 0)

    def test_two_session_concurrent_remaps_preserve_all_namespaces(self):
        aa = [self.launch(self.a, label) for label in ("shared", "a-extra")]
        bb = [self.launch(self.b, label) for label in ("shared", "b-extra")]
        def edit(env, runs, n):
            receipt = self.success(env, "bookmark", runs[n % 2]["run_id"], "--label", "shared")
            own_ids = {r["run_id"] for r in runs}
            self.assertIn(receipt["current"]["run_id"], own_ids)
            if receipt["previous"] is not None:
                self.assertIn(receipt["previous"]["run_id"], own_ids)
            return receipt
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            futures = [pool.submit(edit, env, runs, n) for n in range(8)
                       for env, runs in ((self.a, aa), (self.b, bb))]
            for future in futures:
                future.result(timeout=20)
        for env, runs in ((self.a, aa), (self.b, bb)):
            matches = self.success(env, "bookmarks")["matches"]
            self.assertEqual(len(matches), 2)
            self.assertTrue(all(m["run_id"] in {r["run_id"] for r in runs} for m in matches))
        self.assertEqual(len(json.loads(self.index.read_text())[1]), 4)
        self.assertEqual(list(self.store.glob(".bookmarks-*.tmp")), [])

    @unittest.skipUnless(os.name == "posix", "POSIX active worker fixture")
    def test_active_legacy_initial_owner_controls_and_denials(self):
        proc, run, endpoint = self.fixture.start_control_fixture(extra=("--label", "active",))
        # Legacy initial manifests have no immutable owner yet, but their lock is live.
        path = run / "manifest.json"
        state = json.loads(path.read_text())
        state.pop("creator_main_session_id")
        path.write_text(json.dumps(state))
        self.foreign_attempts(self.b, run.name)
        self.foreign_attempts(self.b, "@active")
        self.assertIsNone(proc.poll())
        self.assertEqual(json.loads((run / "control.json").read_text()), endpoint)
        self.assertEqual(self.cli(self.a, "read", run.name, "manifest.json").returncode, 0)
        self.assertEqual(self.cli(self.a, "path", "@active", ".").stdout.strip(), str(run))
        self.success(self.a, "bookmark", run.name, "--label", "active-copy")
        self.success(self.a, "interrupt", "@active-copy")
        stdout, stderr = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")
        self.foreign_attempts(self.b, run.name)
        self.success(self.a, "resume", run.name)
        self.assertEqual(json.loads(path.read_text())["creator_main_session_id"], "parent-session")


if __name__ == "__main__":
    unittest.main()
