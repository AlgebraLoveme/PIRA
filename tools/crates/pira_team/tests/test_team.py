"""CLI contract checks using a fake Codex process; no model calls or credentials."""
from __future__ import annotations

import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class TeamTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="team-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bin = Path(os.environ["PIRA_TEAM_BIN"]).resolve()
        home = self.root / "original-home"
        home.mkdir()
        (home / "AGENTS.md").write_text("global instructions must not be inherited")
        self.env = dict(os.environ, PATH=f"{self.root}:{os.environ['PATH']}", CODEX_HOME=str(home))
        self.env.pop("PIRA_TEAM_CHILD", None)
        self.env.pop("CODEX_THREAD_ID", None)
        self.env.pop("CODEX_API_KEY", None)
        self.env.pop("PIRA_TEAM_DIR", None)
        fake = self.root / "codex"
        fake.write_text(Path(__file__).with_name("fake_codex.py").read_text())
        fake.chmod(0o700)

    def launch(self, task: str, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.bin), "run", "--model", "test-model", "--effort", "high",
             "--store", str(self.root / "logs"), "--cwd", str(self.root), *extra, "--task", task],
            env=self.env, capture_output=True, text=True, timeout=10,
        )

    def test_explicit_task_literals_equals_and_resume_defaults(self) -> None:
        task = "--model is literal; $value 'quoted'\nλ and spaces"
        first = self.launch(task)
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        self.assertEqual(self.access("read", receipt["run_id"]).stdout, "ANSWER " + task)
        resumed = self.access("resume", receipt["run_id"], "--task=recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(json.loads(self.access("read", receipt["run_id"]).stdout), [task, "recall"])
        continued = self.access("resume", receipt["run_id"])
        self.assertEqual(continued.returncode, 0, continued.stderr)
        self.assertEqual(self.access("read", receipt["run_id"]).stdout, "ANSWER Continue the existing assignment.")
        file = self.root / "followup task.txt"; file.write_text("recall")
        self.assertEqual(self.access("resume", receipt["run_id"], "--task-file", str(file)).returncode, 0)
        equal = subprocess.run([str(self.bin), "run", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--task=--literal=value"], env=self.env,
            capture_output=True, text=True, timeout=5)
        self.assertEqual(equal.returncode, 0, equal.stderr)
        self.assertEqual(Path(json.loads(equal.stdout)["result"]).read_text(), "ANSWER --literal=value")

    def test_conflicting_task_sources_fail_without_mutating_runs(self) -> None:
        receipt = json.loads(self.launch("initial").stdout)
        manifest = Path(receipt["logs"]) / "manifest.json"
        original = manifest.read_bytes()
        bad = [
            ["--task", "one", "--task", "two"],
            ["--task=one", "--task=two"],
            ["--task", "one", "--task-file", "not-read"],
            ["--task-file", "not-read", "--task-file", "also-not-read"],
            ["--task", "one", "positional"],
            ["--task-file", "not-read", "positional"],
            ["--task", ""], ["--task="], ["--task"],
        ]
        for operation in ("run", "resume", "steer"):
            base = ["--model", "test-model", "--effort", "high"] if operation == "run" else [receipt["run_id"]]
            for flags in bad:
                with self.subTest(operation=operation, flags=flags):
                    result = subprocess.run([str(self.bin), operation, *base, "--store", str(self.root / "logs"), *flags],
                        env=self.env, capture_output=True, text=True, timeout=5)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, "")
                    self.assertEqual(manifest.read_bytes(), original)
                    self.assertEqual(len(list((self.root / "logs").iterdir())), 1)
        self.assertNotEqual(self.access("interrupt", receipt["run_id"], "--task", "invalid").returncode, 0)

    def parent(self, contexts: list[dict], *, identity: str = "parent-session") -> Path:
        self.env["CODEX_THREAD_ID"] = "parent-session"
        directory = self.root / "original-home/sessions/2026/10/01"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "rollout-2026-10-01-parent-session.jsonl"
        events = [{"type":"session_meta", "payload":{"id":identity}},
                  {"type":"response_item", "payload":{"text":"PARENT_TRANSCRIPT_MUST_NOT_BE_FORWARDED"}}]
        events += [{"type":"turn_context", "payload":c} for c in contexts]
        path.write_text("".join(json.dumps(e)+"\n" for e in events))
        # A sibling session must not be read; its content is deliberately invalid JSON.
        (directory / "rollout-other-session.jsonl").write_text("unrelated private conversation")
        return path

    def launch_inherited(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.bin), "run", "--store", str(self.root / "logs"),
            "--cwd", str(self.root), *extra, "review"], env=self.env, capture_output=True, text=True, timeout=10)

    def test_inherits_latest_parent_settings_and_not_transcript(self) -> None:
        self.parent([{"model":"previous-model", "effort":"low"}, {"model":"active-model", "effort":"high"}])
        self.env.update(TEAM_EXPECT_MODEL="active-model", TEAM_EXPECT_EFFORT="high")
        result = self.launch_inherited()
        self.assertEqual(result.returncode, 0, result.stderr)
        run = Path(json.loads(result.stdout)["logs"])
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual((manifest["model"],manifest["effort"]), ("active-model","high"))
        self.assertEqual(manifest["profile_sources"], {"model":"parent","effort":"parent"})
        for name in ["policy.md", "task.txt", "manifest.json", "events.jsonl"]:
            self.assertNotIn("PARENT_TRANSCRIPT_MUST_NOT_BE_FORWARDED", (run / name).read_text())

    def test_each_override_is_independent(self) -> None:
        self.parent([{"model":"parent-model", "effort":"medium"}])
        for flags,model,effort,sources in [
            (["--model","other-model"],"other-model","medium",{"model":"explicit","effort":"parent"}),
            (["--effort","low"],"parent-model","low",{"model":"parent","effort":"explicit"}),
            (["--model","other-model","--effort","high"],"other-model","high",{"model":"explicit","effort":"explicit"})]:
            self.env.update(TEAM_EXPECT_MODEL=model, TEAM_EXPECT_EFFORT=effort)
            result = self.launch_inherited(*flags)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads((Path(json.loads(result.stdout)["logs"]) / "manifest.json").read_text())
            self.assertEqual(manifest["profile_sources"],sources)

    def test_no_parent_or_unknown_latest_field_never_uses_defaults(self) -> None:
        result = self.launch_inherited()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--model", result.stderr)
        self.assertFalse((self.root / "logs").exists())
        self.parent([{"model":"old", "effort":"high"}, {"model":"current", "effort":None}])
        result = self.launch_inherited()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "logs").exists())
        result = self.launch_inherited("--effort","low")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_parent_identity_partial_record_and_ambiguous_files_fail_closed(self) -> None:
        path = self.parent([{"model":"parent", "effort":"high"}], identity="different-session")
        self.assertNotEqual(self.launch_inherited().returncode, 0)
        path = self.parent([{"model":"parent", "effort":"high"}])
        with path.open("a") as stream: stream.write('{"type":"turn_context"')
        self.assertNotEqual(self.launch_inherited().returncode, 0)
        self.assertFalse((self.root / "logs").exists())
        path = self.parent([{"model":"parent", "effort":"high"}])
        other = path.with_name("rollout-duplicate-parent-session.jsonl")
        other.write_text(path.read_text())
        self.assertNotEqual(self.launch_inherited().returncode, 0)
        # Explicit settings require no parent-session access even when discovery is broken.
        self.assertEqual(self.launch_inherited("--model","explicit","--effort","low").returncode, 0)

    @unittest.skipUnless(os.name == "posix", "symlink support")
    def test_parent_lookup_does_not_follow_symlinks(self) -> None:
        path = self.parent([{"model":"parent", "effort":"high"}])
        target = self.root / "outside-log.jsonl"
        path.rename(target)
        path.symlink_to(target)
        self.assertNotEqual(self.launch_inherited().returncode, 0)
        self.assertFalse((self.root / "logs").exists())

    def access(self, command: str, run: str, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.bin), command, run, *extra, "--store", str(self.root / "logs")],
            env=self.env, capture_output=True, text=True, timeout=5)

    def test_run_id_reads_exact_artifact_and_resolves_paths_without_parent(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate('line\r\nλ\n', "notes.txt", "text")
        launched = self.launch("review")
        self.assertEqual(launched.returncode, 0, launched.stderr)
        receipt = json.loads(launched.stdout)
        run = receipt["run_id"]
        self.assertEqual(Path(receipt["logs"]).name, run)
        self.assertEqual(receipt["artifact"], "artifacts/notes.txt")
        read = subprocess.run([str(self.bin), "read", run, "--store", str(self.root / "logs")],
            env=self.env, capture_output=True, timeout=5)
        self.assertEqual(read.returncode, 0, read.stderr)
        self.assertEqual(read.stdout, Path(receipt["result"]).read_bytes())
        self.assertEqual(self.access("path", run).stdout.strip(), receipt["result"])
        self.assertEqual(self.access("path", run, ".").stdout.strip(), receipt["logs"])
        self.assertEqual(json.loads(self.access("read", run, "manifest.json").stdout)["run_id"], run)
        self.assertIn("turn/completed", self.access("read", run, "events.jsonl").stdout)
        # Older completed manifests remain readable without new artifact/run_id fields.
        path = Path(receipt["logs"]) / "manifest.json"
        manifest = json.loads(path.read_text()); manifest.pop("artifact"); manifest.pop("run_id")
        path.write_text(json.dumps(manifest))
        self.assertEqual(self.access("path", run).stdout.strip(), receipt["result"])
        # New relative artifact metadata survives moving a complete run to another root.
        manifest["artifact"] = receipt["artifact"]
        path.write_text(json.dumps(manifest))
        moved = self.root / "moved store"; moved.mkdir()
        Path(receipt["logs"]).rename(moved / run)
        result = subprocess.run([str(self.bin), "path", run, "--store", str(moved)], env=self.env,
            capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(moved / run / receipt["artifact"]))

    def test_failed_run_diagnostics_and_repair_candidates_are_accessible(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate("[")
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        run = next((self.root / "logs").iterdir()).name
        self.assertIn(run, result.stderr)
        default = self.access("read", run)
        self.assertNotEqual(default.returncode, 0)
        self.assertEqual(default.stdout, "")
        self.assertEqual(json.loads(self.access("read", run, "manifest.json").stdout)["status"], "failed")
        self.assertIn("JSON", json.loads(self.access("read", run, "repair/validation.json").stdout)["error"])
        self.assertEqual(json.loads(self.access("read", run, "repair/candidate.txt").stdout)["content"], "[")

    def test_environment_store_and_explicit_store_precedence(self) -> None:
        store = self.root / "environment store"
        self.env["PIRA_TEAM_DIR"] = str(store)
        result = subprocess.run([str(self.bin), "run", "--model", "test-model", "--effort", "high", "review"],
            env=self.env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(Path(receipt["logs"]).parent, store)
        read = subprocess.run([str(self.bin), "read", receipt["run_id"]], env=self.env,
            capture_output=True, text=True, timeout=5)
        self.assertEqual(read.stdout, "ANSWER review")
        explicit = json.loads(self.launch("explicit").stdout)
        self.assertEqual(Path(explicit["logs"]).parent, self.root / "logs")
        self.assertEqual(self.access("read", explicit["run_id"]).stdout, "ANSWER explicit")
        missing = self.root / "not-created"
        self.env["PIRA_TEAM_DIR"] = str(missing)
        result = subprocess.run([str(self.bin), "read", "unknown"], env=self.env, capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(missing.exists())

    def test_lookup_rejects_escape_credentials_and_tampered_manifest(self) -> None:
        receipt = json.loads(self.launch("review").stdout)
        run = receipt["run_id"]
        outside = self.root / "outside.txt"; outside.write_text("must-not-be-read")
        for command in ["read", "path"]:
            for relative in ["../outside.txt", str(outside), "codex-home/auth.json", "repair/codex-home/auth.json"]:
                result = self.access(command, run, relative)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
            self.assertNotEqual(self.access(command, "../outside").returncode, 0)
        manifest_path = Path(receipt["logs"]) / "manifest.json"
        manifest = json.loads(manifest_path.read_text()); manifest["artifact"] = "artifacts/../../outside.txt"
        manifest_path.write_text(json.dumps(manifest))
        self.assertNotEqual(self.access("read", run).returncode, 0)

    @unittest.skipUnless(os.name == "posix", "symlink support")
    def test_lookup_does_not_follow_symlinks(self) -> None:
        receipt = json.loads(self.launch("review").stdout)
        target = self.root / "outside.txt"; target.write_text("must-not-be-read")
        link = Path(receipt["logs"]) / "artifacts/linked.txt"; link.symlink_to(target)
        self.assertNotEqual(self.access("read", receipt["run_id"], "artifacts/linked.txt").returncode, 0)
        run_link = self.root / "logs/linked-run"; run_link.symlink_to(Path(receipt["logs"]), target_is_directory=True)
        self.assertNotEqual(self.access("read", run_link.name).returncode, 0)

    def test_final_answer_and_full_logs(self) -> None:
        result = self.launch("review", "--output", "answer")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "ANSWER review")
        run = next((self.root / "logs").iterdir())
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual(manifest["status"], "completed")
        self.assertEqual(manifest["usage"]["input_tokens"], 17)
        self.assertIn("not final", (run / "events.jsonl").read_text())
        self.assertEqual((run / "artifacts/review.md").read_text(), "ANSWER review")
        self.assertIn(str(run), result.stderr)
        self.assertTrue((run / "codex-home").is_dir())
        self.assertFalse((run / "codex-home/auth.json").exists())
        if os.name == "posix": self.assertEqual(run.stat().st_mode & 0o777, 0o700)

    def test_artifact_receipt_hides_answer_and_preserves_report(self) -> None:
        result = self.launch("review '$value'", "--output", "artifact")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], "completed")
        self.assertNotIn("ANSWER", result.stdout)
        self.assertEqual(Path(receipt["result"]).read_text(), "ANSWER review '$value'")
        self.assertEqual(json.loads((Path(receipt["logs"])/"manifest.json").read_text())["output"], "artifact")
        failure = self.launch("crash", "--output", "artifact")
        self.assertNotEqual(failure.returncode, 0)
        self.assertEqual(failure.stdout, "")

    def test_default_navigation_embeds_full_nav_policy_without_other_global_rules(self) -> None:
        result = self.launch("review")
        self.assertEqual(result.returncode, 0, result.stderr)
        policy = (Path(json.loads(result.stdout)["logs"]) / "policy.md").read_text()
        source = Path(__file__).resolve().parents[3] / "src/pira_team"
        self.assertIn((source / "nav_policy.md").read_text().rstrip(), policy)
        self.assertIn("use LSP semantic commands when identity matters", policy)
        self.assertIn((source / "policy.md").read_text().rstrip(), policy)
        self.assertNotIn("do not launch language servers", policy)
        self.assertNotIn("## Memory System", policy)

    def test_fix_requires_completed_review_and_explicit_permission(self) -> None:
        invalid = self.launch("review", "--allow-fix")
        self.assertNotEqual(invalid.returncode, 0)
        self.assertFalse((self.root / "logs").exists())
        ordinary = json.loads(self.launch("review").stdout)
        manifest = Path(ordinary["logs"]) / "manifest.json"
        before = manifest.read_bytes()
        self.assertNotEqual(self.access("resume", ordinary["run_id"], "--allow-fix").returncode, 0)
        self.assertEqual(before, manifest.read_bytes())
        failed = self.launch("crash", "--code-review", "--allow-fix")
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(failed.stdout, "")
        runs = [p for p in (self.root / "logs").iterdir() if p.name != ordinary["run_id"]]
        state = json.loads((runs[0] / "manifest.json").read_text())
        self.assertEqual(state["revision"], 1)
        self.assertEqual(state["sandbox"], "read-only")
        self.assertNotEqual(self.access("resume", runs[0].name, "--allow-fix").returncode, 0)

    def test_fix_phases_inject_policy_and_preserve_review(self) -> None:
        source = Path(__file__).resolve().parents[3] / "src/pira_team"
        for automatic in (False, True):
            with self.subTest(automatic=automatic):
                result = self.launch("inspect assigned module", "--code-review", *(["--allow-fix"] if automatic else []))
                self.assertEqual(result.returncode, 0, result.stderr)
                receipt = json.loads(result.stdout)
                run = Path(self.access("path", receipt["run_id"], ".").stdout.strip())
                review = (run / "artifacts/review.md").read_bytes()
                if not automatic:
                    result = self.access("resume", receipt["run_id"], "--allow-fix")
                    self.assertEqual(result.returncode, 0, result.stderr)
                    receipt = json.loads(result.stdout)
                self.assertEqual(receipt["revision"], 2)
                self.assertEqual((run / "artifacts/review.md").read_bytes(), review)
                state = json.loads((run / "manifest.json").read_text())
                self.assertEqual(state["sandbox"], "workspace-write")
                self.assertEqual(len(state["revisions"]), 2)
                self.assertEqual(state["usage"]["input_tokens"], 34)
                fix = Path(receipt["logs"])
                for directory, expected in ((run, "code_review.md"), (fix, "code_fix.md")):
                    policy = (directory / "policy.md").read_text()
                    self.assertIn((source / expected).read_text().rstrip(), policy)
                    requests = [json.loads(line) for line in (directory / "requests.jsonl").read_text().splitlines()]
                    thread = next(r for r in requests if r.get("method") in ("thread/start", "thread/resume"))
                    self.assertEqual(thread["params"]["baseInstructions"], policy)
                self.assertNotIn("You are a read-only worker", (fix / "policy.md").read_text())
                resumed = self.access("resume", receipt["run_id"])
                self.assertEqual(resumed.returncode, 0, resumed.stderr)
                self.assertEqual(json.loads((run / "manifest.json").read_text())["sandbox"], "workspace-write")

    def test_fix_output_repair_is_read_only(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate("{}")
        first = json.loads(self.launch("review", "--code-review", "--format", "json").stdout)
        self.env.update(TEAM_CANDIDATE=self.candidate("["), TEAM_REPAIRED=self.candidate("{}"))
        fixed = self.access("resume", first["run_id"], "--allow-fix")
        self.assertEqual(fixed.returncode, 0, fixed.stderr)
        receipt = json.loads(fixed.stdout)
        requests = [json.loads(line) for line in (Path(receipt["logs"]) / "repair/requests.jsonl").read_text().splitlines()]
        thread = next(r for r in requests if r.get("method") == "thread/resume")
        self.assertEqual(thread["params"]["sandbox"], "read-only")
        self.assertIn("You are a read-only worker", thread["params"]["baseInstructions"])

    def test_code_review_is_opt_in_and_retained_on_resume_and_repair(self) -> None:
        source = Path(__file__).resolve().parents[3] / "src/pira_team/code_review.md"
        guidance = source.read_text().rstrip()
        ordinary = json.loads(self.launch("review").stdout)
        self.assertNotIn(guidance, (Path(ordinary["logs"]) / "policy.md").read_text())
        self.assertFalse(json.loads(self.access("read", ordinary["run_id"], "manifest.json").stdout)["code_review"])
        self.env.update(TEAM_CANDIDATE=self.candidate("["), TEAM_REPAIRED=self.candidate("{}"))
        result = self.launch("inspect complexity", "--code-review", "--navigation", "shell", "--format", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        first = json.loads(result.stdout)
        for name in ("policy.md", "repair/policy.md"):
            self.assertIn(guidance, (Path(first["logs"]) / name).read_text())
        self.env.pop("TEAM_CANDIDATE"); self.env.pop("TEAM_REPAIRED")
        self.env["TEAM_CANDIDATE"] = self.candidate("{}")
        resumed = self.access("resume", first["run_id"], "--task", "check the callers too")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        latest = json.loads(self.access("read", first["run_id"], "manifest.json").stdout)
        self.assertTrue(latest["code_review"])
        self.assertIn(guidance, (Path(json.loads(resumed.stdout)["logs"]) / "policy.md").read_text())
        self.assertNotEqual(self.launch("review", "--code-review", "--code-review").returncode, 0)

    def test_legacy_review_mode_defaults_false_and_malformed_mode_fails_closed(self) -> None:
        first = json.loads(self.launch("extract").stdout)
        path = Path(first["logs"]) / "manifest.json"
        manifest = json.loads(path.read_text()); manifest.pop("code_review")
        path.write_text(json.dumps(manifest))
        self.assertEqual(self.access("resume", first["run_id"]).returncode, 0)
        manifest = json.loads(path.read_text()); manifest["code_review"] = "yes"
        path.write_text(json.dumps(manifest)); before = path.read_bytes()
        result = self.access("resume", first["run_id"])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid persisted code_review", result.stderr)
        self.assertEqual(path.read_bytes(), before)

    def test_navigation_modes_and_invalid_options(self) -> None:
        result = self.launch("review", "--navigation", "shell")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = next((self.root / "logs").iterdir())
        policy = (run / "policy.md").read_text()
        self.assertIn("ordinary read-only shell inspection", policy)
        self.assertNotIn("### `pira_nav`:", policy)
        self.assertNotIn("or language servers", policy)
        self.assertEqual(json.loads((run / "manifest.json").read_text())["navigation"], "shell")
        for flag in ["--output", "--navigation"]:
            self.assertNotEqual(self.launch("review", flag, "invalid").returncode, 0)

    def test_failures_never_masquerade_as_answers(self) -> None:
        for task in ["crash", "missing", "malformed", "failed", "timeout"]:
            with self.subTest(task=task):
                result = self.launch(task, "--timeout", "1")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertIn("logs:", result.stderr)
        for path in (self.root / "logs").glob("*/manifest.json"):
            self.assertIn(json.loads(path.read_text())["status"], ["failed", "timed_out"])

    def test_parallel_runs_have_independent_logs(self) -> None:
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            results = list(pool.map(self.launch, ["one", "two"]))
        self.assertEqual([Path(json.loads(r.stdout)["result"]).read_text() for r in results], ["ANSWER one", "ANSWER two"])
        self.assertEqual(len(list((self.root / "logs").iterdir())), 2)
        for result, expected in zip(results, ["ANSWER one", "ANSWER two"]):
            self.assertEqual(self.access("read", json.loads(result.stdout)["run_id"]).stdout, expected)

    def candidate(self, content: str, filename: str = "findings.json", format: str = "json") -> str:
        return json.dumps({"filename": filename, "format": format, "content": content})

    def test_json_repair_retains_candidates_and_totals_usage(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate('{"findings": [1')
        self.env["TEAM_REPAIRED"] = self.candidate('{"findings": [1]}\n')
        result = self.launch("review", "--format", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["repairs"], 1)
        self.assertEqual(receipt["usage"], {"input_tokens":34,"cached_input_tokens":6,"output_tokens":10,"reasoning_output_tokens":4,"cache_write_input_tokens":2})
        self.assertTrue(receipt["usage_complete"])
        self.assertEqual(Path(receipt["result"]).read_text(), '{"findings": [1]}\n')
        run = Path(receipt["logs"])
        self.assertTrue((run / "validation.json").is_file())
        self.assertEqual(json.loads((run / "candidate.txt").read_text())["content"], '{"findings": [1')
        self.assertTrue((run / "repair/candidate.txt").is_file())
        self.assertFalse((run / "repair/codex-home").exists())
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual([a["status"] for a in manifest["attempts"]], ["invalid", "validated"])

    def test_unrepairable_envelope_and_unsafe_name_fail_closed(self) -> None:
        for candidate in [json.dumps("not JSON {"), self.candidate("{}", "../escape.json"),
                          self.candidate("{}", "CON.json"), self.candidate("{}").replace('"content"', '"extra"')]:
            self.env["TEAM_CANDIDATE"] = candidate
            result = self.launch("review")
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "")
        self.assertFalse((self.root / "escape.json").exists())
        for run in (self.root / "logs").iterdir():
            manifest = json.loads((run / "manifest.json").read_text())
            self.assertEqual(len(manifest["attempts"]), 2)
            self.assertEqual(manifest["usage"]["input_tokens"], 34)
            self.assertFalse((run / "artifacts").exists())

    def test_schema_and_csv_constraints(self) -> None:
        schema = self.root / "schema.json"
        schema.write_text(json.dumps({"type":"object", "required":["n"], "$defs":{"number":{"type":"integer"}}, "properties":{"n":{"$ref":"#/$defs/number"}}}))
        self.env["TEAM_CANDIDATE"] = self.candidate('{"n":"1"}')
        self.env["TEAM_REPAIRED"] = self.candidate('{"n":1}')
        result = self.launch("extract", "--format", "json", "--schema", str(schema))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["repairs"], 1)
        self.env["TEAM_CANDIDATE"] = self.candidate('a,b\n1\n', "table.csv", "csv")
        self.env["TEAM_REPAIRED"] = self.candidate('a,b\n1,2\n', "table.csv", "csv")
        result = self.launch("extract", "--format", "csv", "--columns", '["a","b"]')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(json.loads(result.stdout)["result"]).read_text(), 'a,b\n1,2\n')

    def test_invalid_contract_rejected_before_model(self) -> None:
        external = self.root / "external-schema.json"
        external.write_text('{"type":"object"}')
        for schema in [{"type":"nonsense"}, {"$ref":external.as_uri()},
                       {"$ref":"https://example.invalid/schema"}]:
            path = self.root / "schema.json"
            path.write_text(json.dumps(schema))
            result = self.launch("extract", "--format", "json", "--schema", str(path))
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((self.root / "logs").exists())
        for flags in [("--format","yaml"), ("--columns", '["a"]'),
                      ("--format","csv","--columns","[]")]:
            self.assertNotEqual(self.launch("extract", *flags).returncode, 0)

    def test_repair_crash_preserves_known_usage_without_claiming_total(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate("[")
        self.env["TEAM_REPAIR_CRASH"] = "1"
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        run = next((self.root / "logs").iterdir())
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual(manifest["usage"]["input_tokens"], 17)
        self.assertFalse(manifest["usage_complete"])
        self.assertEqual(manifest["attempts"][1]["status"], "failed")

    def test_timeout_budget_is_shared_with_repair(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate("[")
        self.env["TEAM_DELAY"] = "2.5"
        result = self.launch("review", "--timeout", "4")
        self.assertNotEqual(result.returncode, 0)
        run = next((self.root / "logs").iterdir())
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual(len(manifest["attempts"]), 2)
        self.assertIn("timed_out", manifest["error"])
        self.assertLess(manifest["elapsed_seconds"], 5.5)

    def test_nested_workers_are_rejected(self) -> None:
        self.env["PIRA_TEAM_CHILD"] = "1"
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "logs").exists())

    def test_task_file_and_space_paths(self) -> None:
        task_file = self.root / "review task.txt"
        task_file.write_text("review multiple\nlines")
        cwd = self.root / "source with spaces"
        cwd.mkdir()
        result = subprocess.run(
            [str(self.bin), "run", "--model", "test-model", "--effort", "high",
             "--store", str(self.root / "logs with spaces"), "--cwd", str(cwd),
             "--task-file", str(task_file)],
            env=self.env, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(json.loads(result.stdout)["result"]).read_text(), "ANSWER review multiple\nlines")

    def test_file_auth_reused_and_detached_on_run_and_resume(self) -> None:
        source = self.root / "original-home/auth.json"
        original = b'{"synthetic": "not-a-real-credential"}'
        source.write_bytes(original)
        self.env["TEAM_TEST_AUTH_SOURCE"] = str(source)
        first = self.launch("review")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        link = Path(receipt["logs"]) / "codex-home/auth.json"
        self.assertFalse(link.exists())
        self.assertFalse(link.is_symlink())
        resumed = self.access("resume", receipt["run_id"], "--task", "followup")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertFalse(link.exists())
        self.assertFalse(link.is_symlink())
        self.assertEqual(source.read_bytes(), original)

    def test_explicit_api_auth_does_not_link_file_auth(self) -> None:
        (self.root / "original-home/auth.json").write_text("synthetic unused credential")
        self.env["CODEX_API_KEY"] = "synthetic-test-value"
        result = self.launch("review")
        self.assertEqual(result.returncode, 0, result.stderr)

    @unittest.skipUnless(os.name == "posix", "POSIX ownership and permission checks")
    def test_rejects_symlink_and_shared_writable_stores(self) -> None:
        target = self.root / "target"
        target.mkdir()
        store = self.root / "logs"
        store.symlink_to(target, target_is_directory=True)
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(target.iterdir()), [])
        store.unlink()
        store.mkdir(mode=0o700)
        store.chmod(0o777)
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(store.iterdir()), [])
        self.assertEqual(store.stat().st_mode & 0o777, 0o777)
        store.chmod(0o700)
        self.root.chmod(0o777)
        try:
            result = self.launch("review")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsafe store ancestor", result.stderr)
            self.assertEqual(list(store.iterdir()), [])
        finally:
            self.root.chmod(0o700)

    @unittest.skipUnless(os.name == "posix", "POSIX signal lifecycle")
    def test_interrupt_reaps_worker_and_cleans_home(self) -> None:
        read_fd, write_fd = os.pipe()
        self.env["TEAM_READY_FD"] = str(write_fd)
        proc = subprocess.Popen([str(self.bin), "run", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--timeout", "20", "interrupt"],
            env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        os.close(write_fd)
        try:
            import select
            self.assertTrue(select.select([read_fd], [], [], 5)[0], "worker readiness timed out")
            self.assertEqual(os.read(read_fd, 1), b"1")
            proc.terminate()
            stdout, _ = proc.communicate(timeout=5)
            self.assertNotEqual(proc.returncode, 0)
            self.assertEqual(stdout, "")
            run = next((self.root / "logs").iterdir())
            self.assertTrue((run / "codex-home").is_dir())
            self.assertFalse((run / "codex-home/auth.json").exists())
            self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
        finally:
            os.close(read_fd)
            if proc.poll() is None: proc.terminate(); proc.communicate(timeout=5)

    @unittest.skipUnless(os.name == "posix", "POSIX terminal integration")
    def test_terminal_wrapper_exposes_live_control_id(self) -> None:
        import pty
        import re
        import select
        import shutil
        import time
        ctx = shutil.which("pira_ctx")
        if not ctx: self.skipTest("optional PIRA wrapper integration requires pira_ctx")
        master, slave = pty.openpty()
        command = [ctx, "exact", "--store-dir", str(self.root / "ctx-store"),
            "--intent", "Exercise live Team control in a terminal", "--", str(self.bin),
            "run", "--model", "test-model", "--effort", "high", "--store", str(self.root / "logs"),
            "--cwd", str(self.root), "--timeout", "15", "steerable"]
        proc = subprocess.Popen(command, env=self.env, cwd=self.root, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        try:
            output = b""
            deadline = time.monotonic() + 5
            while b"pira_team active:" not in output and time.monotonic() < deadline:
                if select.select([master], [], [], .1)[0]:
                    output += os.read(master, 65536)
            self.assertIn(b"pira_team active:", output, output.decode(errors="replace"))
            run_id = re.search(rb"pira_team run_id: ([a-zA-Z0-9_-]+)", output).group(1).decode()
            response = self.access("steer", run_id, "terminal-controlled")
            self.assertEqual(response.returncode, 0, response.stderr)
            self.assertEqual(proc.wait(timeout=5), 0)
            self.assertEqual(self.access("read", run_id).stdout, "ANSWER terminal-controlled")
        finally:
            os.close(master)
            if proc.poll() is None: proc.terminate(); proc.wait(timeout=8)

    @unittest.skipUnless(os.name == "posix", "POSIX signal lifecycle")
    def test_signal_cancels_blocked_request_write(self) -> None:
        import select
        read_fd, write_fd = os.pipe()
        self.env.update(TEAM_READY_FD=str(write_fd), TEAM_BLOCK_INPUT="1")
        task = self.root / "large-task.txt"
        task.write_text("read-only inspection " * 50000)
        proc = subprocess.Popen([str(self.bin), "run", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--timeout", "25", "--task-file", str(task)],
            env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        os.close(write_fd)
        try:
            self.assertTrue(select.select([read_fd], [], [], 5)[0])
            self.assertEqual(os.read(read_fd, 1), b"1")
            import time
            run = next((self.root / "logs").iterdir())
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                path = run / "manifest.json"
                if path.exists() and json.loads(path.read_text()).get("thread_id"): break
                time.sleep(.005)
            self.assertTrue(json.loads((run / "manifest.json").read_text()).get("thread_id"))
            # turn/start writes before checking cancellation; the peer no longer drains stdin.
            proc.terminate()
            out, err = proc.communicate(timeout=8)
            self.assertNotEqual(proc.returncode, 0)
            self.assertFalse(out)
            run = next((self.root / "logs").iterdir())
            self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
        finally:
            os.close(read_fd)
            if proc.poll() is None: proc.kill(); proc.communicate(timeout=5)

    def test_repair_rejects_steer_but_accepts_interrupt(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate("[")
        self.env["TEAM_REPAIR_WAIT"] = "1"
        proc = subprocess.Popen([str(self.bin), "run", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--timeout", "15", "review"],
            env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            run_id = proc.stderr.readline().strip().split(": ", 1)[1]
            while True:
                line = proc.stderr.readline()
                self.assertTrue(line, "worker exited before repair became active")
                if "repair=true" in line: break
            self.assertIn("steering unavailable", self.access("steer", run_id, "change evidence").stderr)
            self.assertEqual(self.access("interrupt", run_id).returncode, 0)
            out, err = proc.communicate(timeout=5)
            self.assertNotEqual(proc.returncode, 0)
            self.assertFalse(out)
            manifest = json.loads(self.access("read", run_id, "manifest.json").stdout)
            self.assertEqual(manifest["status"], "interrupted")
            self.assertEqual(manifest["usage"]["input_tokens"], 17)
            self.assertEqual(manifest["repairs"], 1)
        finally:
            if proc.poll() is None: proc.terminate(); proc.communicate(timeout=8)

    def test_stalled_cancellation_is_bounded_and_resumable(self) -> None:
        self.env["TEAM_IGNORE_INTERRUPT"] = "1"
        result = self.launch("timeout", "--timeout", "1")
        self.assertNotEqual(result.returncode, 0)
        run = next((self.root / "logs").iterdir())
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual(manifest["status"], "timed_out")
        self.assertLess(manifest["elapsed_seconds"], 8)
        self.assertFalse((run / "control.json").exists())
        self.env.pop("TEAM_IGNORE_INTERRUPT")
        self.assertEqual(self.access("resume", run.name, "recall", "--timeout", "10").returncode, 0)

    def test_resume_retains_context_revisions_and_profile(self) -> None:
        first = json.loads(self.launch("remember-blue").stdout)
        run = first["run_id"]
        resumed = self.access("resume", run, "recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        second = json.loads(resumed.stdout)
        self.assertEqual(second["revision"], 2)
        self.assertEqual(second["usage"]["input_tokens"], 34)
        self.assertEqual(second["revision_usage"]["input_tokens"], 17)
        self.assertTrue(second["usage_complete"])
        self.assertEqual(json.loads(self.access("read", run).stdout), ["remember-blue", "recall"])
        self.assertEqual(self.access("read", run, first["artifact"]).stdout, "ANSWER remember-blue")
        old = json.loads(self.access("read", run, "revisions/000001/manifest.json").stdout)
        new = json.loads(self.access("read", run, "manifest.json").stdout)
        self.assertEqual(old["revision"], 1)
        self.assertEqual(new["thread_id"], old["thread_id"])
        self.assertEqual(new["profile_sources"], {"model":"run", "effort":"run"})
        self.assertNotEqual(self.access("steer", run, "late").returncode, 0)
        self.assertNotEqual(self.access("interrupt", run).returncode, 0)
        self.assertNotEqual(self.access("resume", run, "--cwd", str(self.root), "bad").returncode, 0)

    def test_resume_keeps_embedded_contract_and_allows_explicit_profile(self) -> None:
        schema = self.root / "schema.json"
        schema.write_text('{"type":"integer"}')
        self.env["TEAM_CANDIDATE"] = self.candidate("1")
        first = json.loads(self.launch("extract", "--format", "json", "--schema", str(schema)).stdout)
        schema.unlink()
        self.env.update(TEAM_EXPECT_MODEL="other", TEAM_EXPECT_EFFORT="low")
        resumed = self.access("resume", first["run_id"], "continue", "--model", "other", "--effort", "low")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.env["TEAM_CANDIDATE"] = self.candidate('"not integer"')
        invalid = self.access("resume", first["run_id"], "continue")
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn("validation failed", invalid.stderr)
        self.assertEqual(self.access("read", first["run_id"], first["artifact"]).stdout, "1")

    def test_legacy_or_orphaned_runs_do_not_resume(self) -> None:
        receipt = json.loads(self.launch("review").stdout)
        manifest_path = Path(receipt["logs"]) / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["status"] = "running"
        manifest_path.write_text(json.dumps(manifest))
        self.assertIn("ambiguous recovery", self.access("resume", receipt["run_id"]).stderr)
        manifest["status"] = "completed"
        del manifest["transport"]
        manifest_path.write_text(json.dumps(manifest))
        self.assertIn("legacy ephemeral", self.access("resume", receipt["run_id"]).stderr)
        self.assertEqual(self.access("read", receipt["run_id"]).stdout, "ANSWER review")

    def test_runtime_must_confirm_read_only(self) -> None:
        self.env["TEAM_BAD_PERMISSION"] = "1"
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not confirm read-only", result.stderr)
        run = next((self.root / "logs").iterdir())
        self.assertNotIn('"method":"turn/start"', (run / "requests.jsonl").read_text())

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_active_steer_interrupt_and_exclusive_resume(self) -> None:
        import socket
        import select
        for operation in ("steer", "interrupt"):
            read_fd, write_fd = os.pipe()
            self.env["TEAM_READY_FD"] = str(write_fd)
            proc = subprocess.Popen([str(self.bin), "run", "--model", "test-model", "--effort", "high",
                "--store", str(self.root / "logs"), "--timeout", "20", "steerable"],
                env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            os.close(write_fd)
            try:
                self.assertTrue(select.select([read_fd], [], [], 5)[0])
                self.assertEqual(os.read(read_fd, 1), b"1")
                # Read the launcher's advertised ID, then synchronize with endpoint creation.
                run_id = proc.stderr.readline().strip().split(": ", 1)[1]
                run = self.root / "logs" / run_id
                # A native response precedes endpoint creation; bounded retry is test synchronization only.
                import time
                deadline = time.monotonic() + 3
                while not (run / "control.json").exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                endpoint = json.loads((run / "control.json").read_text())
                self.assertIn("active owner", self.access("resume", run_id, "collision").stderr)
                self.assertNotEqual(self.access("read", run_id, "control.json").returncode, 0)
                for token, turn in [("bad", endpoint["turn_id"]), (endpoint["token"], "stale")]:
                    host, port = endpoint["address"].split(":")
                    with socket.create_connection((host, int(port)), 2) as client:
                        client.sendall((json.dumps({"token":token, "turn_id":turn, "operation":"interrupt", "task":""})+"\n").encode())
                        reply = json.loads(client.makefile().readline())
                    self.assertEqual(reply["status"], "rejected")
                extra = ["--task", "changed task"] if operation == "steer" else []
                controlled = self.access(operation, run_id, *extra)
                self.assertEqual(controlled.returncode, 0, controlled.stderr)
                self.assertEqual(json.loads(controlled.stdout)["status"], "accepted")
                stdout, stderr = proc.communicate(timeout=5)
                if operation == "steer":
                    self.assertEqual(proc.returncode, 0, stderr)
                    self.assertEqual(Path(json.loads(stdout)["result"]).read_text(), "ANSWER changed task")
                else:
                    self.assertNotEqual(proc.returncode, 0)
                    self.assertEqual(stdout, "")
                    self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
                self.assertFalse((run / "control.json").exists())
                del self.env["TEAM_READY_FD"]
                resumed = self.access("resume", run_id, "recall")
                self.assertEqual(resumed.returncode, 0, resumed.stderr)
                self.assertIn("steerable", json.loads(self.access("read", run_id).stdout))
            finally:
                os.close(read_fd)
                if proc.poll() is None: proc.terminate(); proc.communicate(timeout=8)


if __name__ == "__main__":
    unittest.main()
