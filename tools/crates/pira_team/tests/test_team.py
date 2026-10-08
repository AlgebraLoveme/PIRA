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
        self.env["PIRA_CTX_STORE_DIR"] = str(self.root / "ctx-store")
        self.env["PIRA_DEC_STORE_DIR"] = str(self.root / "dec-store")
        self.env["TEAM_TEST_BACKEND_CONTRACT"] = str(Path(__file__).resolve().parents[3] / "src/pira_team/backend_contract.json")
        self.env["TEAM_TEST_BACKEND_FIXTURE"] = str(Path(__file__).with_name("backend_fixture.py").resolve())
        self.env.pop("PIRA_TEAM_BUILD_ROOTS", None)
        self.env.pop("PIRA_TEAM_CHILD", None)
        self.env.pop("CODEX_THREAD_ID", None)
        self.env.pop("CODEX_API_KEY", None)
        self.env.pop("PIRA_TEAM_DIR", None)
        fake = self.root / "codex"
        fake.write_text(Path(__file__).with_name("fake_codex.py").read_text())
        fake.chmod(0o700)
        self.parent([{}])

    def launch(self, task: str, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
             "--store", str(self.root / "logs"), "--cwd", str(self.root), *extra, "--task", task],
            env=self.env, capture_output=True, text=True, timeout=10,
        )

    def fail_startup(self, proc: subprocess.Popen[str], reason: str, consumed_stderr: str = "") -> None:
        if proc.poll() is None:
            proc.terminate()
        try:
            stdout, stderr = proc.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
            stdout, stderr = proc.communicate(timeout=5)
        self.fail(f"{reason}; launcher exit={proc.returncode}\nstdout:\n{stdout}\nstderr:\n{consumed_stderr}{stderr}")

    def assert_worker_ready(self, proc: subprocess.Popen[str], read_fd: int) -> None:
        import select
        if not select.select([read_fd], [], [], 5)[0]:
            self.fail_startup(proc, "worker readiness timed out")
        ready = os.read(read_fd, 1)
        if ready != b"1":
            self.fail_startup(proc, f"worker readiness returned {ready!r}, expected b'1'")

    def wait_stderr_notice(self, proc: subprocess.Popen[str], notice: str) -> str:
        consumed = []
        while True:
            line = proc.stderr.readline()
            if not line:
                self.fail_startup(proc, f"launcher exited before {notice}", "".join(consumed))
            consumed.append(line)
            if notice in line:
                return line

    def metadata(self, receipt: dict) -> dict:
        self.assertEqual(set(receipt), {"run_id", "status", "run_root", "handoff_path"})
        result = self.access("read", receipt["run_id"], "manifest.json")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

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
        equal = subprocess.run([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--cwd", str(self.root), "--task=--literal=value"], env=self.env,
            capture_output=True, text=True, timeout=5)
        self.assertEqual(equal.returncode, 0, equal.stderr)
        self.assertEqual(Path(json.loads(equal.stdout)["handoff_path"]).read_text(), "ANSWER --literal=value")

    def test_conflicting_task_sources_fail_without_mutating_runs(self) -> None:
        receipt = json.loads(self.launch("initial").stdout)
        manifest = Path(receipt["run_root"]) / "manifest.json"
        original = manifest.read_bytes()
        bad = [
            (["--task", "one", "--task", "two"], "only once"),
            (["--task=one", "--task=two"], "only once"),
            (["--task", "one", "--task-file", "not-read"], "mutually exclusive"),
            (["--task-file", "not-read", "--task-file", "also-not-read"], "only once"),
            (["--task", "one", "positional"], "only once"),
            (["--task-file", "not-read", "positional"], "mutually exclusive"),
            (["--task", ""], "task must be nonempty"),
            (["--task="], "task must be nonempty"),
            (["--task"], "missing value for --task"),
        ]
        for operation in ("run", "resume", "steer"):
            base = ["--model", "test-model", "--effort", "high"] if operation == "run" else [receipt["run_id"]]
            for flags, error in bad:
                with self.subTest(operation=operation, flags=flags):
                    # Other required inputs stay valid so missing gates cannot mask task errors.
                    result = subprocess.run([str(self.bin), operation, *base,
                        "--completion-gate", "Report actual checks", "--store", str(self.root / "logs"), *flags],
                        env=self.env, capture_output=True, text=True, timeout=5)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(error, result.stderr)
                    self.assertEqual(result.stdout, "")
                    self.assertEqual(manifest.read_bytes(), original)
                    self.assertEqual(len(list((self.root / "logs").iterdir())), 1)
        self.assertNotEqual(self.access("interrupt", receipt["run_id"], "--task", "invalid").returncode, 0)

    def permission_context(self) -> dict:
        return {"cwd":str(self.root), "approval_policy":"never", "sandbox_policy":{
            "type":"workspace-write", "writable_roots":[], "network_access":False,
            "exclude_tmpdir_env_var":True, "exclude_slash_tmp":True}}

    def parent(self, contexts: list[dict], *, identity: str = "parent-session") -> Path:
        self.env["CODEX_THREAD_ID"] = "parent-session"
        directory = self.root / "original-home/sessions/2026/10/01"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "rollout-2026-10-01-parent-session.jsonl"
        events = [{"type":"session_meta", "payload":{"id":identity}},
                  {"type":"response_item", "payload":{"text":"PARENT_TRANSCRIPT_MUST_NOT_BE_FORWARDED"}}]
        events += [{"type":"turn_context", "payload":{**self.permission_context(), **c}} for c in contexts]
        path.write_text("".join(json.dumps(e)+"\n" for e in events))
        # A sibling session must not be read; its content is deliberately invalid JSON.
        (directory / "rollout-other-session.jsonl").write_text("unrelated private conversation")
        return path

    @unittest.skipUnless(os.name == "posix", "POSIX fixture readiness")
    def test_startup_diagnostics_expose_scope_rejection(self) -> None:
        outside = self.root.parent
        for synchronization in ("ready", "notice"):
            with self.subTest(synchronization=synchronization):
                read_fd, write_fd = os.pipe()
                proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Fixture diagnostics",
                    "--model", "test-model", "--effort", "high", "--cwd", str(outside),
                    "--store", str(self.root / "logs"), "--task", "must not launch"],
                    env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                os.close(write_fd)
                try:
                    with self.assertRaisesRegex(AssertionError, "caller permissions do not allow required Team write path"):
                        if synchronization == "ready":
                            self.assert_worker_ready(proc, read_fd)
                        else:
                            self.wait_stderr_notice(proc, "pira_team run_id:")
                    self.assertEqual(proc.returncode, 1)
                    self.assertFalse((self.root / "logs").exists())
                finally:
                    os.close(read_fd)
                    if proc.poll() is None:
                        proc.terminate(); proc.communicate(timeout=8)

    def test_full_access_network_and_request_configuration(self) -> None:
        self.parent([{"sandbox_policy":{"type":"danger-full-access"},
                      "permission_profile":{"type":"disabled"}}])
        result = self.launch("full access")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        run = Path(receipt["run_root"])
        manifest = self.metadata(receipt)
        self.assertEqual(manifest["execution_permissions"]["sandbox_policy"], {"type":"dangerFullAccess"})
        self.assertEqual(manifest["execution_permissions"]["source"], "verified-caller-turn-context")
        requests = [json.loads(line) for line in (run / "requests.jsonl").read_text().splitlines()]
        thread = next(r["params"] for r in requests if r["method"] == "thread/start")
        turn = next(r["params"] for r in requests if r["method"] == "turn/start")
        self.assertEqual((thread["sandbox"], thread["config"], turn["sandboxPolicy"]),
                         ("danger-full-access", {}, {"type":"dangerFullAccess"}))
        self.assertIn('Safety:', (run / "policy.md").read_text().split('## Safety')[1].split('## Handoff')[0])
        self.assertNotIn('Safety:', (run / "phase.md").read_text().split('Latest assignment contract')[0])

    def test_resume_replaces_full_access_with_current_restricted_policy(self) -> None:
        self.parent([{"sandbox_policy":{"type":"danger-full-access"}}])
        first = self.launch("first")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        self.parent([{}])
        resumed = self.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        manifest = self.metadata(json.loads(resumed.stdout))
        self.assertEqual(manifest["sandbox"], "workspace-write")
        latest = Path(receipt["run_root"]) / "revisions/000002"
        requests = [json.loads(line) for line in (latest / "requests.jsonl").read_text().splitlines()]
        thread = next(r["params"] for r in requests if r["method"] == "thread/resume")
        turn = next(r["params"] for r in requests if r["method"] == "turn/start")
        expected = {"type":"workspaceWrite", "writableRoots":[str(self.root)],
                    "networkAccess":False, "excludeTmpdirEnvVar":True, "excludeSlashTmp":True}
        self.assertEqual(thread["sandbox"], "workspace-write")
        self.assertEqual(thread["config"]["sandbox_workspace_write"]["network_access"], False)
        self.assertEqual(turn["sandboxPolicy"], expected)
        self.assertEqual(manifest["execution_permissions"]["sandbox_policy"], expected)
        previous = json.loads((Path(receipt["run_root"]) / "revisions/000001/manifest.json").read_text())
        self.assertEqual(previous["sandbox"], "danger-full-access")

    def test_workspace_network_flags_and_roots_are_inherited_exactly(self) -> None:
        extra = self.root / "extra"; extra.mkdir()
        context = self.permission_context()
        context["sandbox_policy"].update(writable_roots=[str(extra)], network_access=True,
                                         exclude_tmpdir_env_var=False, exclude_slash_tmp=False)
        self.parent([context])
        result = self.launch("network enabled")
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = self.metadata(json.loads(result.stdout))
        expected = {"type":"workspaceWrite", "writableRoots":[str(self.root),str(extra)],
                    "networkAccess":True, "excludeTmpdirEnvVar":False, "excludeSlashTmp":False}
        self.assertEqual(manifest["execution_permissions"]["sandbox_policy"], expected)

    def test_missing_unknown_unrepresentable_and_approval_context_fail_closed(self) -> None:
        cases = [
            ({"sandbox_policy":None}, "sandbox_policy"),
            ({"sandbox_policy":{"type":"future"}}, "unsupported"),
            ({"sandbox_policy":{"type":"external-sandbox", "network_access":"enabled"}}, "unsupported"),
            ({"sandbox_policy":{"type":"read-only"}}, "read-only"),
            ({"approval_policy":None}, "approval"),
            ({"approval_policy":"on-request"}, "interactive"),
            ({"approval_policy":{"granular":{}}}, "interactive"),
            ({"permission_profile":{"type":"managed", "file_system":{"type":"restricted", "entries":[]}, "network":"restricted"}}, "permission_profile"),
            ({"file_system_sandbox_policy":{"type":"restricted"}}, "file_system_sandbox_policy"),
        ]
        malformed = self.permission_context()["sandbox_policy"]
        for key, value in [("network_access", "false"), ("exclude_slash_tmp", None), ("writable_roots", "all")]:
            cases.append(({"sandbox_policy":{**malformed,key:value}}, "malformed"))
        for context, error in cases:
            with self.subTest(context=context):
                self.parent([context])
                result = self.launch("must not start")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(error, result.stderr)
                self.assertFalse((self.root / "logs").exists())
        self.env.pop("CODEX_THREAD_ID")
        self.assertNotEqual(self.launch("explicit settings still require caller").returncode, 0)
        self.assertFalse((self.root / "logs").exists())

    def test_missing_latest_permissions_do_not_backfill_or_mutate_resume(self) -> None:
        receipt = json.loads(self.launch("first").stdout)
        manifest = Path(receipt["run_root"]) / "manifest.json"
        original = manifest.read_bytes()
        self.parent([{}, {"sandbox_policy":None}])
        result = self.access("resume", receipt["run_id"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(manifest.read_bytes(), original)
        self.assertFalse((Path(receipt["run_root"]) / "revisions/000002").exists())

    def test_managed_and_build_write_roots_do_not_expand_caller_scope(self) -> None:
        permitted = self.root / "permitted"; permitted.mkdir()
        self.parent([{"cwd":str(permitted)}])
        result = self.launch("out of caller scope")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no extra write root", result.stderr)
        self.assertFalse((self.root / "logs").exists())
        self.parent([{}])
        outside = tempfile.TemporaryDirectory(prefix="team-outside-")
        self.addCleanup(outside.cleanup)
        self.env["PIRA_TEAM_BUILD_ROOTS"] = json.dumps([str(Path(outside.name).resolve())])
        result = self.launch("outside build root")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no extra write root", result.stderr)
        self.assertFalse(list((self.root / "logs").glob("*/requests.jsonl")))

    def test_version_twelve_safety_migration_is_injected_once(self) -> None:
        receipt = json.loads(self.launch("first").stdout)
        run = Path(receipt["run_root"])
        manifest = run / "manifest.json"
        state = json.loads(manifest.read_text()); state["worker_policy_version"] = 12
        manifest.write_text(json.dumps(state))
        prefix = (run / "policy.md").read_bytes()
        for revision in (2,3):
            result = self.access("resume", receipt["run_id"])
            self.assertEqual(result.returncode, 0, result.stderr)
            phase = (run / f"revisions/{revision:06}/phase.md").read_text()
            self.assertEqual('exact prefix `Safety:`' in phase, revision == 2)
            self.assertEqual((run / "policy.md").read_bytes(), prefix)

    def launch_inherited(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--store", str(self.root / "logs"),
            "--cwd", str(self.root), *extra, "review"], env=self.env, capture_output=True, text=True, timeout=10)

    def test_inherits_latest_parent_settings_and_not_transcript(self) -> None:
        self.parent([{"model":"previous-model", "effort":"low"}, {"model":"active-model", "effort":"high"}])
        self.env.update(TEAM_EXPECT_MODEL="active-model", TEAM_EXPECT_EFFORT="high")
        result = self.launch_inherited()
        self.assertEqual(result.returncode, 0, result.stderr)
        run = Path(json.loads(result.stdout)["handoff_path"]).parent.parent
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
            manifest = json.loads((Path(json.loads(result.stdout)["handoff_path"]).parent.parent / "manifest.json").read_text())
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
        # Explicit model/effort never bypass execution permission verification.
        self.assertNotEqual(self.launch_inherited("--model","explicit","--effort","low").returncode, 0)

    @unittest.skipUnless(os.name == "posix", "symlink support")
    def test_parent_lookup_does_not_follow_symlinks(self) -> None:
        path = self.parent([{"model":"parent", "effort":"high"}])
        target = self.root / "outside-log.jsonl"
        path.rename(target)
        path.symlink_to(target)
        self.assertNotEqual(self.launch_inherited().returncode, 0)
        self.assertFalse((self.root / "logs").exists())

    def access(self, command: str, run: str, *extra: str) -> subprocess.CompletedProcess[str]:
        if command in ("resume", "steer") and extra and not any(x.startswith("--completion-gate") for x in extra):
            extra = (*extra, "--completion-gate", "Report findings and actual checks")
        return subprocess.run([str(self.bin), command, run, *extra, "--store", str(self.root / "logs")],
            env=self.env, capture_output=True, text=True, timeout=5)

    def test_run_id_reads_exact_artifact_and_resolves_paths_without_parent(self) -> None:
        self.env["TEAM_CANDIDATE"] = self.candidate('line\r\nλ\n', "notes.txt", "text")
        launched = self.launch("review")
        self.assertEqual(launched.returncode, 0, launched.stderr)
        receipt = json.loads(launched.stdout)
        run = receipt["run_id"]
        self.assertEqual(Path(receipt["run_root"]).name, run)
        self.assertEqual(str(Path(receipt["handoff_path"]).relative_to(receipt["run_root"])), "artifacts/handoff")
        read = subprocess.run([str(self.bin), "read", run, "--store", str(self.root / "logs")],
            env=self.env, capture_output=True, timeout=5)
        self.assertEqual(read.returncode, 0, read.stderr)
        self.assertEqual(read.stdout, Path(receipt["handoff_path"]).read_bytes())
        self.assertEqual(self.access("path", run).stdout.strip(), receipt["handoff_path"])
        self.assertEqual(self.access("path", run, ".").stdout.strip(), str(Path(receipt["run_root"])))
        self.assertEqual(json.loads(self.access("read", run, "manifest.json").stdout)["run_id"], run)
        self.assertIn("turn/completed", self.access("read", run, "events.jsonl").stdout)
        # Older completed manifests remain readable without new artifact/run_id fields.
        path = Path(receipt["run_root"]) / "manifest.json"
        manifest = json.loads(path.read_text()); manifest.pop("artifact"); manifest.pop("run_id")
        path.write_text(json.dumps(manifest))
        self.assertEqual(self.access("path", run).stdout.strip(), receipt["handoff_path"])
        # New relative artifact metadata survives moving a complete run to another root.
        manifest["artifact"] = str(Path(receipt["handoff_path"]).relative_to(receipt["run_root"]))
        path.write_text(json.dumps(manifest))
        moved = self.root / "moved store"; moved.mkdir()
        Path(receipt["run_root"]).rename(moved / run)
        result = subprocess.run([str(self.bin), "path", run, "--store", str(moved)], env=self.env,
            capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(moved / run / str(Path(receipt["handoff_path"]).relative_to(receipt["run_root"]))))

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
        self.assertEqual(self.access("read", run, "repair/rejected-handoff.txt").stdout, "[")

    def test_environment_store_and_explicit_store_precedence(self) -> None:
        store = self.root / "environment store"
        self.env["PIRA_TEAM_DIR"] = str(store)
        result = subprocess.run([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high", "--cwd", str(self.root), "review"],
            env=self.env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(Path(receipt["run_root"]).parent, store)
        read = subprocess.run([str(self.bin), "read", receipt["run_id"]], env=self.env,
            capture_output=True, text=True, timeout=5)
        self.assertEqual(read.stdout, "ANSWER review")
        explicit = json.loads(self.launch("explicit").stdout)
        self.assertEqual(Path(explicit["run_root"]).parent, self.root / "logs")
        self.assertEqual(self.access("read", explicit["run_id"]).stdout, "ANSWER explicit")
        missing = self.root / "not-created"
        self.env["PIRA_TEAM_DIR"] = str(missing)
        result = subprocess.run([str(self.bin), "read", "unknown"], env=self.env, capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(missing.exists())

    def test_persistent_default_lookup_and_explicit_store_precedence_without_backend(self):
        import sys
        home = self.root / "layout-home"
        local = self.root / "local-app-data"
        data = self.root / "xdg-data"
        self.env.update(HOME=str(home), LOCALAPPDATA=str(local), XDG_DATA_HOME=str(data))
        if sys.platform == "darwin":
            parent = home / "Library" / "Application Support" / "PIRA"
        elif os.name == "nt":
            parent = local / "PIRA"
        else:
            parent = data / "pira"
        default = parent / "team"
        explicit = self.root / "explicit-store"
        override = self.root / "environment-store"
        for root in (default, explicit, override):
            (root / "fixture-run").mkdir(parents=True)
        def lookup(*extra):
            return subprocess.run([str(self.bin), "path", "fixture-run", ".", *extra],
                                  env=self.env, capture_output=True, text=True, timeout=5)
        result = lookup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(result.stdout.strip()), (default / "fixture-run").resolve())
        self.env["PIRA_TEAM_DIR"] = str(override)
        result = lookup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(result.stdout.strip()), (override / "fixture-run").resolve())
        # CLI override works even when no platform-default parent can be resolved.
        for key in ("HOME", "LOCALAPPDATA", "XDG_DATA_HOME"):
            self.env.pop(key, None)
        result = lookup("--store", str(explicit))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(result.stdout.strip()), (explicit / "fixture-run").resolve())
        self.env.pop("PIRA_TEAM_DIR")
        self.assertEqual(lookup("--store", str(explicit)).returncode, 0)
        failed = lookup()
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("cannot resolve persistent PIRA store", failed.stderr)

    def test_missing_run_explains_old_store_access_without_migration(self):
        store = self.root / "empty-persistent-store"
        self.env["PIRA_TEAM_DIR"] = str(store)
        for create in (False, True):
            if create:
                store.mkdir()
            result = subprocess.run([str(self.bin), "path", "old-run"], env=self.env,
                                    capture_output=True, text=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("--store OLD_STORE", result.stderr)
            self.assertIn("absolute paths", result.stderr)
            self.assertEqual(store.exists(), create)
            if create:
                self.assertEqual(list(store.iterdir()), [])

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
        manifest_path = Path(receipt["run_root"]) / "manifest.json"
        manifest = json.loads(manifest_path.read_text()); manifest["artifact"] = "artifacts/../../outside.txt"
        manifest_path.write_text(json.dumps(manifest))
        self.assertNotEqual(self.access("read", run).returncode, 0)

    @unittest.skipUnless(os.name == "posix", "symlink support")
    def test_lookup_does_not_follow_symlinks(self) -> None:
        receipt = json.loads(self.launch("review").stdout)
        target = self.root / "outside.txt"; target.write_text("must-not-be-read")
        link = Path(receipt["run_root"]) / "artifacts/linked.txt"; link.symlink_to(target)
        self.assertNotEqual(self.access("read", receipt["run_id"], "artifacts/linked.txt").returncode, 0)
        run_link = self.root / "logs/linked-run"; run_link.symlink_to(Path(receipt["run_root"]), target_is_directory=True)
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
        self.assertEqual((run / "artifacts/handoff").read_text(), "ANSWER review")
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
        self.assertEqual(Path(receipt["handoff_path"]).read_text(), "ANSWER review '$value'")
        manifest = self.metadata(receipt)
        self.assertEqual(manifest["output"], "artifact")
        path = self.access("path", receipt["run_id"], "manifest.json")
        self.assertEqual(path.returncode, 0, path.stderr)
        self.assertEqual(json.loads(Path(path.stdout.strip()).read_text()), manifest)
        self.assertEqual(manifest["logs"], receipt["run_root"])
        self.assertEqual(manifest["format"], "markdown")
        self.assertIn("not factual accuracy", manifest["validation"])
        self.assertNotIn("warning:", result.stderr)
        self.assertIn("pira_team run_id: " + receipt["run_id"], result.stderr)

    def test_answer_bytes_and_missing_usage_warning(self) -> None:
        content = "λ\r\nexact bytes, no final newline"
        self.env["TEAM_CANDIDATE"] = self.candidate(content, format="text")
        self.env["TEAM_NO_USAGE"] = "1"
        result = subprocess.run(
            [str(self.bin), "run", "--task", "review", "--completion-gate", "Report checks",
             "--model", "test-model", "--effort", "high", "--cwd", str(self.root),
             "--store", str(self.root / "logs"), "--output", "answer"],
            env=self.env, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, content.encode())
        self.assertIn(b"usage accounting incomplete", result.stderr)
        run = next((self.root / "logs").iterdir())
        manifest = json.loads(self.access("read", run.name, "manifest.json").stdout)
        self.assertFalse(manifest["usage_complete"])

    def test_nav_rule_mirrors_main_policy(self):
        root = Path(__file__).resolve().parents[4]
        prefix = "- Let structural backends auto-select."
        def rule(path):
            with path.open() as source:
                matches = [line.rstrip("\n") for line in source if line.startswith(prefix)]
            self.assertEqual(len(matches), 1)
            return matches[0]
        self.assertEqual(rule(root / "AGENTS.md"), rule(root / "tools/src/pira_team/main.md"))

    def test_injects_exact_technical_and_tool_guidance(self):
        result = self.launch("review", "--inject-review", "--inject-implement")
        self.assertEqual(result.returncode, 0, result.stderr)
        policy = (Path(json.loads(result.stdout)["handoff_path"]).parent.parent / "policy.md").read_text()
        root = Path(__file__).resolve().parents[4]
        global_policy = (root / "AGENTS.md").read_text().split("## PIRA Internal Tools", 1)[1]
        for name in ("pira_ctx", "pira_dec", "pira_nav"):
            section = global_policy.split(f"### `{name}`:", 1)[1].split("\n### ", 1)[0]
            self.assertIn(section.rstrip(), policy)
        source = root / "tools/src/pira_team"
        for name in ("main.md", "review.md"):
            self.assertIn((source / name).read_text().rstrip(), policy)
        self.assertNotIn("# Implementation\n", policy)
        phase = (Path(json.loads(result.stdout)["handoff_path"]).parent.parent / "phase.md").read_text()
        self.assertIn((source / "implementation.md").read_text().rstrip(), phase)
        policy += "\n" + phase
        coding = (root / "modules/CODING_STYLE.md").read_text()
        for heading in ("Change Discipline", "Names, Types, and Dependencies", "Contracts, Failures, and Security", "Operability", "Verification"):
            section = coding.split("## " + heading + "\n", 1)[1].split("\n## ", 1)[0]
            section = "\n".join(line for line in section.splitlines() if not line.startswith("- Once relevant checks pass"))
            self.assertIn(section.rstrip(), policy)
        self.assertNotIn("- Once relevant checks pass", policy)
        self.assertNotIn("### `pira_team`:", policy)
        self.assertNotIn("No recursive delegation", policy)
        self.assertIn("A significant decision", policy)
        self.assertIn("Finish independent authorized work", policy)
        self.assertIn("must not be labeled completed as limitations", policy)
        self.assertIn("no mandatory duplicate main review", policy)
        self.assertIn("supporting files beside the handoff", policy)
        self.assertIn("do not use arbitrary hard truncation", policy)
        self.assertTrue(policy.startswith("# Technical artifact worker"))
        self.assertIn("does not grant project-edit authority", policy)
        self.assertIn("formalizations", policy)

    def test_staged_artifact_routes_without_worker(self):
        run = self.root / "logs" / "staged-fixture"
        run.mkdir(parents=True)
        for prefix in ("implementation", "revisions/000002/implementation"):
            directory = run / prefix
            (directory / "artifacts").mkdir(parents=True)
            (directory / "repair").mkdir()
            handoff = directory / "artifacts/handoff.md"
            handoff.write_text("Final public handoff")
            (directory / "phase.md").write_text("stage instructions")
            (directory / "repair/validation.json").write_text('{"error":"fixture"}')
            (run / "manifest.json").write_text(json.dumps({"status": "completed", "artifact": str(handoff.relative_to(run))}))
            self.assertEqual(self.access("read", run.name).stdout, "Final public handoff")
            self.assertEqual(self.access("path", run.name).stdout.strip(), str(handoff))
            self.assertEqual(self.access("read", run.name, prefix + "/phase.md").stdout, "stage instructions")
            self.assertEqual(self.access("read", run.name, prefix + "/repair/validation.json").returncode, 0)
            for path in (prefix + "/codex-home/auth.json", prefix + "/../manifest.json", prefix + "/implementation/phase.md"):
                self.assertNotEqual(self.access("read", run.name, path).returncode, 0)

    def test_combined_stages_preserve_thread_prefix_and_usage(self):
        self.env["TEAM_ASSERT_PREFIX"] = "1"
        result = self.launch("Review broadly and fix authorized findings", "--inject-review", "--inject-implement")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)  # exactly one public receipt
        run = Path(receipt["run_root"])
        manifest = self.metadata(receipt)
        state = json.loads((run / "codex-home/fixture-state.json").read_text())
        self.assertEqual(state["turns"], 2)
        self.assertEqual(manifest["thread_id"], state["thread"])
        self.assertEqual(manifest["stage"], "implementation")
        self.assertEqual([a["stage"] for a in manifest["attempts"]], ["review", "implementation"])
        self.assertEqual(manifest["attempts"][0]["status"], "checkpoint_validated")
        self.assertEqual(len(manifest["revisions"]), 1)
        self.assertEqual(manifest["usage"]["input_tokens"], 34)
        self.assertEqual(manifest["usage"], manifest["revision_usage"])
        self.assertTrue(manifest["usage_complete"])
        self.assertNotEqual(manifest["review_checkpoint"], receipt["handoff_path"])
        self.assertTrue(Path(manifest["review_checkpoint"]).is_file())
        first, second = state["turn_details"]
        self.assertEqual(first["policy"], second["policy"])
        self.assertIn("# Review\n", first["policy"])
        self.assertNotIn("# Implementation\n", first["policy"] + first["phase"])
        self.assertIn("Do not edit project source, tests, configuration", first["phase"])
        self.assertIn("NOT that the final completion gate is met", first["phase"])
        self.assertIn("Defer reading implementation/coding guidance, including CODING_STYLE.md", first["phase"])
        self.assertIn("even if the broader combined assignment asks to load it upfront", first["phase"])
        self.assertIn("# Implementation\n", second["phase"])
        self.assertIn("final diff and impact", state["history"][1])
        checkpoint = Path(manifest["review_checkpoint"]).read_bytes()
        original = Path(receipt["handoff_path"]).read_bytes()
        resumed = self.access("resume", receipt["run_id"], "--inject-review", "--inject-implement")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        latest = self.metadata(json.loads(resumed.stdout))
        self.assertEqual(latest["usage"]["input_tokens"], 51)
        self.assertEqual(latest["revision_usage"]["input_tokens"], 17)
        self.assertEqual(len(latest["attempts"]), 1)
        phase = (Path(json.loads(resumed.stdout)["handoff_path"]).parent.parent / "phase.md").read_text()
        self.assertNotIn("# Implementation\n", phase)
        self.assertNotIn("REVIEW STAGE ONLY", phase)
        self.assertEqual(Path(manifest["review_checkpoint"]).read_bytes(), checkpoint)
        self.assertEqual(Path(receipt["handoff_path"]).read_bytes(), original)

    def test_combined_review_blockers_resume_review_without_implementation(self):
        for status in ("needs_decision", "incomplete"):
            with self.subTest(status=status):
                self.env["TEAM_REVIEW_CANDIDATE"] = json.dumps({"status": status, "format": "markdown", "content": "Blocked: decision or missing prerequisite"})
                first = self.launch("Review and fix", "--inject-review", "--inject-implement")
                self.assertEqual(first.returncode, 0, first.stderr)
                receipt = json.loads(first.stdout)
                manifest = self.metadata(receipt)
                self.assertEqual(receipt["status"], status)
                self.assertEqual(manifest["stage"], "review")
                self.assertFalse(manifest["inject_implement"])
                self.assertEqual(len(manifest["attempts"]), 1)
                self.assertNotIn("checkpoint", receipt["handoff_path"])
                self.env.pop("TEAM_REVIEW_CANDIDATE")
                resumed = self.access("resume", receipt["run_id"], "--task", "Decision resolved; continue authorized work", "--inject-implement")
                self.assertEqual(resumed.returncode, 0, resumed.stderr)
                latest = self.metadata(json.loads(resumed.stdout))
                self.assertEqual([a["stage"] for a in latest["attempts"]], ["review", "implementation"])
                self.assertEqual(latest["usage"]["input_tokens"], 51)
                self.assertEqual(latest["revision_usage"]["input_tokens"], 34)
                review_phase = (Path(latest["logs"]) / "phase.md").read_text()
                self.assertNotIn("# Implementation\n", review_phase)

    def test_combined_review_failure_and_invalid_checkpoint_fail_closed(self):
        for task in ("failed", "crash", "missing"):
            with self.subTest(task=task):
                result = self.launch(task, "--inject-review", "--inject-implement")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                run = max((self.root / "logs").iterdir(), key=lambda p: p.name)
                state = json.loads((run / "manifest.json").read_text())
                self.assertEqual(state["status"], "failed")
                self.assertEqual(state["stage"], "review")
                self.assertFalse(state["inject_implement"])
                self.assertEqual(len(state["attempts"]), 1)
                self.assertEqual(state["repairs"], 0)
                self.assertFalse((run / "implementation").exists())
                self.assertNotIn("Operation not permitted", result.stderr)
                if task == "failed":
                    self.assertIn("worker failed", result.stderr)
                if task == "missing":
                    self.assertIn("control JSON", result.stderr)

    def test_combined_checkpoint_ignores_final_schema_and_final_gets_one_repair(self):
        schema = self.root / "schema.json"
        schema.write_text(json.dumps({"type": "object", "required": ["ok"], "properties": {"ok": {"const": True}}}))
        self.env["TEAM_REVIEW_CANDIDATE"] = json.dumps({"format": "markdown", "content": "Compact findings, no final JSON schema"})
        self.env["TEAM_CANDIDATE"] = json.dumps({"format": "json", "content": "{}"})
        self.env["TEAM_REPAIRED"] = json.dumps({"format": "json", "content": '{"ok":true}'})
        result = self.launch("Review and fix", "--inject-review", "--inject-implement", "--format", "json", "--schema", str(schema))
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        manifest = self.metadata(receipt)
        self.assertEqual(Path(receipt["handoff_path"]).read_text(), '{"ok":true}')
        self.assertEqual(manifest["repairs"], 1)
        self.assertEqual(manifest["usage"]["input_tokens"], 51)
        self.assertEqual([a["status"] for a in manifest["attempts"]], ["checkpoint_validated", "invalid", "validated"])
        repair = Path(manifest["attempts"][-1]["logs"])
        self.assertNotIn("# Implementation\n", (repair / "phase.md").read_text())
        self.assertIn("Format repair only", (repair / "phase.md").read_text())
        self.assertEqual(Path(manifest["review_checkpoint"]).read_text(), "Compact findings, no final JSON schema")

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_combined_review_control_preserves_stage_and_steered_gate(self):
        proc, run, _ = self.start_control_fixture(extra=("--inject-review", "--inject-implement"))
        active = json.loads((run / "manifest.json").read_text())
        self.assertEqual(active["stage"], "review")
        self.assertEqual(active["status"], "running")
        self.assertNotIn("result", active)
        steered = self.access("steer", run.name, "--task", "Fix revised authorized scope", "--completion-gate", "Revised final gate")
        self.assertEqual(steered.returncode, 0, steered.stderr)
        stdout, stderr = proc.communicate(timeout=5)
        self.assertEqual(proc.returncode, 0, stderr)
        receipt = json.loads(stdout)
        final = self.metadata(receipt)
        self.assertEqual(final["completion_gate"], "Revised final gate")
        self.assertEqual(final["task"], "Fix revised authorized scope")
        implementation = Path(receipt["handoff_path"]).parent.parent
        self.assertIn("Fix revised authorized scope", (implementation / "task.txt").read_text())
        self.assertIn("Revised final gate", (implementation / "phase.md").read_text())
        self.assertEqual(final["usage"]["input_tokens"], 34)
        self.assertEqual(self.access("read", run.name).stdout, Path(receipt["handoff_path"]).read_text())
        relative = str(implementation.relative_to(run) / "phase.md")
        self.assertEqual(self.access("read", run.name, relative).returncode, 0)
        for forbidden in ("implementation/codex-home/fixture-state.json", "implementation/../manifest.json"):
            self.assertNotEqual(self.access("read", run.name, forbidden).returncode, 0)

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_combined_review_interrupt_resumes_review_without_duplicate_guidance(self):
        proc, run, _ = self.start_control_fixture(extra=("--inject-review", "--inject-implement"))
        interrupted = self.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        stdout, stderr = proc.communicate(timeout=5)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        self.assertEqual(state["stage"], "review")
        self.assertFalse(state["inject_implement"])
        self.assertFalse((run / "implementation").exists())
        resumed = self.access("resume", run.name, "--task", "Continue after interruption", "--inject-review", "--inject-implement")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        latest = self.metadata(json.loads(resumed.stdout))
        self.assertEqual([a["stage"] for a in latest["attempts"]], ["review", "implementation"])
        self.assertEqual(latest["usage"]["input_tokens"], 34)
        self.assertFalse(latest["usage_complete"])  # existing latch survives an interrupted turn
        phases = json.loads((run / "codex-home/fixture-state.json").read_text())["phases"]
        self.assertEqual(sum("# Implementation\n" in p for p in phases), 1)
        self.assertNotIn("# Review\n", phases[1])

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_combined_implementation_interrupt_resumes_without_repeat_review(self):
        import time
        self.env["TEAM_IMPLEMENT_WAIT"] = "1"
        proc, run, _ = self.start_control_fixture(extra=("--inject-review", "--inject-implement"))
        steered = self.access("steer", run.name, "--task", "Proceed within authorization", "--completion-gate", "Final verified changes")
        self.assertEqual(steered.returncode, 0, steered.stderr)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            path = run / "control.json"
            if path.exists():
                endpoint = json.loads(path.read_text())
                if endpoint["turn_id"] == "2":
                    break
            if proc.poll() is not None:
                self.fail(proc.communicate(timeout=1)[1])
            time.sleep(.01)
        else:
            self.fail("implementation control endpoint not created")
        active = json.loads((run / "manifest.json").read_text())
        self.assertEqual(active["status"], "running")
        self.assertEqual(active["stage"], "implementation")
        self.assertNotIn("result", active)
        self.assertTrue(Path(active["review_checkpoint"]).exists())
        stopped = self.access("interrupt", run.name)
        self.assertEqual(stopped.returncode, 0, stopped.stderr)
        stdout, stderr = proc.communicate(timeout=5)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        self.assertEqual(state["stage"], "implementation")
        self.assertTrue(state["inject_implement"])
        self.env.pop("TEAM_IMPLEMENT_WAIT")
        resumed = self.access("resume", run.name)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        latest = self.metadata(json.loads(resumed.stdout))
        self.assertEqual([a["stage"] for a in latest["attempts"]], ["implementation"])
        self.assertEqual(latest["review_checkpoint"], active["review_checkpoint"])
        self.assertEqual(latest["usage"]["input_tokens"], 34)
        self.assertEqual(latest["revision_usage"]["input_tokens"], 17)
        phases = json.loads((run / "codex-home/fixture-state.json").read_text())["phases"]
        self.assertEqual(sum("# Implementation\n" in p for p in phases), 1)
        self.assertEqual(sum("REVIEW STAGE ONLY" in p for p in phases), 1)

    def test_combined_missing_usage_does_not_invent_stage_totals(self):
        self.env["TEAM_NO_USAGE"] = "1"
        result = self.launch("Review and fix", "--inject-review", "--inject-implement", "--output", "answer")
        self.assertEqual(result.returncode, 0, result.stderr)
        run, = (self.root / "logs").iterdir()
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(result.stdout, Path(state["result"]).read_text())
        self.assertNotEqual(result.stdout, Path(state["review_checkpoint"]).read_text())
        self.assertEqual(state["usage"], {})
        self.assertFalse(state["usage_complete"])
        self.assertIn("usage accounting incomplete", result.stderr)

    def test_combined_blocker_format_repair_never_starts_implementation(self):
        self.env["TEAM_REVIEW_CANDIDATE"] = json.dumps({"status": "needs_decision", "format": "markdown", "content": "Choose the contract"})
        self.env["TEAM_REPAIRED"] = json.dumps({"status": "needs_decision", "format": "json", "content": '{"question":"Choose the contract"}'})
        first = self.launch("Review and fix", "--inject-review", "--inject-implement", "--format", "json")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        state = self.metadata(receipt)
        self.assertEqual(receipt["status"], "needs_decision")
        self.assertEqual(state["stage"], "review")
        self.assertFalse(state["inject_implement"])
        self.assertEqual(state["repairs"], 1)
        self.assertEqual(state["usage"]["input_tokens"], 34)
        self.assertEqual([a["stage"] for a in state["attempts"]], ["review", "review"])
        self.assertEqual(json.loads(self.access("read", receipt["run_id"]).stdout), {"question": "Choose the contract"})
        self.env["TEAM_REPAIRED"] = json.dumps({"format": "json", "content": '{"ok":true}'})
        bad = self.launch("Review and fix", "--inject-review", "--inject-implement", "--format", "json")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("must preserve the non-completed outcome", bad.stderr)

    def test_malformed_review_blockers_get_format_repair_without_advancement(self):
        for status, format, invalid, repaired in (
            ("needs_decision", "json", "{broken", '{"question":"Choose"}'),
            ("incomplete", "csv", "a,b\n1\n", "reason\nBlocked\n"),
        ):
            with self.subTest(status=status, format=format):
                self.env["TEAM_REVIEW_CANDIDATE"] = json.dumps({"status": status, "format": format, "content": invalid})
                self.env["TEAM_REPAIRED"] = json.dumps({"status": status, "format": format, "content": repaired})
                result = self.launch("Review and fix", "--inject-review", "--inject-implement", "--format", format)
                self.assertEqual(result.returncode, 0, result.stderr)
                receipt = json.loads(result.stdout)
                state = self.metadata(receipt)
                self.assertEqual(receipt["status"], status)
                self.assertEqual(state["stage"], "review")
                self.assertEqual(state["repairs"], 1)
                self.assertFalse(state["inject_implement"])
                self.assertEqual([a["status"] for a in state["attempts"]], ["invalid", "validated"])
                self.assertEqual(Path(receipt["handoff_path"]).read_text(), repaired)
                self.assertEqual((Path(receipt["run_root"]) / "rejected-handoff.txt").read_text(), invalid)

    def test_successful_review_requires_markdown_checkpoint(self):
        for format, content in (("json", "{}"), ("text", "Findings"), ("csv", "finding\nnone\n")):
            with self.subTest(format=format):
                self.env["TEAM_REVIEW_CANDIDATE"] = json.dumps({"format": format, "content": content})
                result = self.launch("Review and fix", "--inject-review", "--inject-implement")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("expected format markdown", result.stderr)
                run = max((self.root / "logs").iterdir(), key=lambda p: p.name)
                state = json.loads((run / "manifest.json").read_text())
                self.assertEqual(state["status"], "failed")
                self.assertEqual(state["stage"], "review")
                self.assertEqual(state["repairs"], 0)
                self.assertFalse((run / "implementation").exists())

    def test_guidance_selection_and_additive_resume(self):
        for flags in ((), ("--inject-review",), ("--inject-implement",), ("--inject-review", "--inject-implement")):
            with self.subTest(flags=flags):
                result = self.launch("Assess assigned artifact", *flags)
                self.assertEqual(result.returncode, 0, result.stderr)
                receipt = json.loads(result.stdout)
                run = Path(receipt["run_root"])
                prefix = (run / "policy.md").read_text()
                self.assertEqual("# Review\n" in prefix, "--inject-review" in flags)
                self.assertEqual("# Implementation\n" in prefix, flags == ("--inject-implement",))
                resumed = self.access("resume", receipt["run_id"], "--inject-review", "--inject-implement")
                self.assertEqual(resumed.returncode, 0, resumed.stderr)
                latest = Path(json.loads(resumed.stdout)["handoff_path"]).parent.parent
                self.assertEqual((latest / "policy.md").read_text(), prefix)
                phase = (latest / "phase.md").read_text()
                self.assertEqual("# Review\n" in phase, "--inject-review" not in flags)
                self.assertEqual("# Implementation\n" in phase, "--inject-implement" not in flags)
                again = self.access("resume", receipt["run_id"])
                self.assertEqual(again.returncode, 0, again.stderr)
                phase = (Path(json.loads(again.stdout)["handoff_path"]).parent.parent / "phase.md").read_text()
                self.assertNotIn("# Review\n", phase)
                self.assertNotIn("# Implementation\n", phase)
        for flag in ("--inject-review", "--inject-implement"):
            self.assertNotEqual(self.launch("review", flag, flag).returncode, 0)

    def test_fix_needs_no_prior_review_marker(self):
        first = self.launch("Review assigned module only")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        resumed = self.access("resume", receipt["run_id"], "--task", "Implement the verified narrow fixes")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        state = json.loads(self.access("read", receipt["run_id"], "manifest.json").stdout)
        self.assertEqual(state["sandbox"], "workspace-write")
        self.assertEqual(state["thread_id"], "fixture-thread")
        deprecated = self.launch("Implement narrow fixes", "--allow-fix")
        self.assertEqual(deprecated.returncode, 0, deprecated.stderr)
        self.assertIn("deprecated", deprecated.stderr)

    def test_resume_preserves_prefix_and_prior_handoff(self):
        first = json.loads(self.launch("Review assigned module").stdout)
        original = Path(first["handoff_path"]).read_bytes()
        prefix = (Path(first["run_root"]) / "policy.md").read_bytes()
        second = self.access("resume", first["run_id"], "--task", "Implement fixes")
        self.assertEqual(second.returncode, 0, second.stderr)
        receipt = json.loads(second.stdout)
        self.assertEqual(Path(first["handoff_path"]).read_bytes(), original)
        self.assertEqual((Path(receipt["handoff_path"]).parent.parent / "policy.md").read_bytes(), prefix)
        self.assertNotEqual(first["handoff_path"], receipt["handoff_path"])
        phase = (Path(receipt["handoff_path"]).parent.parent / "phase.md").read_text()
        self.assertIn(receipt["handoff_path"], phase)
        self.assertIn(self.metadata(receipt)["completion_gate"], phase)
        self.assertNotIn("# Coding technical rules", phase)

    def test_missing_gate_rejected_before_launch_or_revision_mutation(self):
        result = subprocess.run([str(self.bin), "run", "--task", "Review code"], env=self.env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("completion-gate", result.stderr)
        self.assertFalse((self.root / "logs").exists())
        first = json.loads(self.launch("review").stdout)
        path = Path(first["run_root"]) / "manifest.json"
        before = path.read_bytes()
        result = subprocess.run([str(self.bin), "resume", first["run_id"], "--store", str(self.root / "logs"), "--task", "Implement fixes"], env=self.env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(path.read_bytes(), before)

    def test_incomplete_is_readable_resumable_but_not_completed(self):
        self.env["TEAM_CANDIDATE"] = json.dumps({"status":"incomplete", "format":"text", "content":"Compiler unavailable; no edits."})
        result = self.launch("review")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], "incomplete")
        self.assertEqual(self.metadata(receipt)["validation"], "format only; completion not claimed")
        self.assertIn("Compiler unavailable", self.access("read", receipt["run_id"]).stdout)
        self.env.pop("TEAM_CANDIDATE")
        self.assertEqual(self.access("resume", receipt["run_id"]).returncode, 0)

    def test_launcher_trusts_completion_without_semantic_gate_validation(self):
        result = self.launch("review", "--completion-gate", "Never duplicate a supplied gate")
        self.assertNotEqual(result.returncode, 0)  # duplicate gate fails parsing
        self.env["TEAM_CANDIDATE"] = self.candidate("Worker reports checks passed", "x.txt", "text")
        result = self.launch("Review and report")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], "completed")
        self.assertEqual(receipt["handoff_path"], self.metadata(receipt)["result"])
        self.assertEqual(receipt["run_root"], self.metadata(receipt)["logs"])

    def test_resume_preserves_existing_base_even_after_policy_upgrade(self) -> None:
        first = json.loads(self.launch("review", "--code-review").stdout)
        run = Path(first["run_root"])
        original = (run / "policy.md").read_text() + "\nLegacy instruction prefix.\n"
        (run / "policy.md").write_text(original)
        manifest = json.loads((run / "manifest.json").read_text())
        manifest.pop("phase_instructions", None)
        (run / "manifest.json").write_text(json.dumps(manifest))
        result = self.access("resume", first["run_id"], "--allow-fix")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((Path(json.loads(result.stdout)["handoff_path"]).parent.parent / "policy.md").read_text(), original)

    def test_legacy_retained_run_requires_gate_and_injects_new_contract(self):
        first = json.loads(self.launch("review").stdout)
        path = Path(first["run_root"]) / "manifest.json"
        state = json.loads(path.read_text())
        state.pop("completion_gate"); state.pop("worker_policy_version")
        state["schema_version"] = 3; state["sandbox"] = "read-only"
        path.write_text(json.dumps(state))
        prefix = (Path(first["run_root"]) / "policy.md").read_bytes()
        before = path.read_bytes()
        self.assertNotEqual(self.access("resume", first["run_id"]).returncode, 0)
        self.assertEqual(path.read_bytes(), before)
        resumed = self.access("resume", first["run_id"], "--completion-gate", "Report actual checks")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        latest = Path(json.loads(resumed.stdout)["handoff_path"]).parent.parent
        self.assertEqual((latest / "policy.md").read_bytes(), prefix)
        self.assertIn("# Implementation", (latest / "phase.md").read_text())

    def test_previous_policy_migrates_once_without_replacing_prefix(self):
        first = json.loads(self.launch("review").stdout)
        run = Path(first["run_root"])
        manifest = run / "manifest.json"
        state = json.loads(manifest.read_text())
        state["worker_policy_version"] = 4
        manifest.write_text(json.dumps(state))
        prefix = (run / "policy.md").read_bytes()
        for migrated in (True, False):
            result = self.access("resume", first["run_id"])
            self.assertEqual(result.returncode, 0, result.stderr)
            latest = Path(json.loads(result.stdout)["handoff_path"]).parent.parent
            self.assertEqual((latest / "policy.md").read_bytes(), prefix)
            self.assertEqual("# Technical artifact worker" in (latest / "phase.md").read_text(), migrated)
            self.assertEqual(json.loads(manifest.read_text())["worker_policy_version"], 13)

    def test_version_six_policy_gets_new_rules_once_without_adding_phases(self):
        first = json.loads(self.launch("review").stdout)
        run = Path(first["run_root"])
        manifest = run / "manifest.json"
        state = json.loads(manifest.read_text())
        state["worker_policy_version"] = 6
        manifest.write_text(json.dumps(state))
        prefix = (run / "policy.md").read_bytes()
        for migrated in (True, False):
            result = self.access("resume", first["run_id"])
            self.assertEqual(result.returncode, 0, result.stderr)
            phase = (Path(json.loads(result.stdout)["handoff_path"]).parent.parent / "phase.md").read_text()
            self.assertEqual("Finish independent authorized work" in phase, migrated)
            self.assertNotIn("# Review", phase)
            self.assertNotIn("# Implementation", phase)
            self.assertEqual((run / "policy.md").read_bytes(), prefix)

    def test_task_guidance_updates_migrate_once_without_replacing_prefix(self):
        guides = (
            ("--inject-implement", "Before fixing, trace the affected behavior", "# Review"),
            ("--inject-review", "Review the full assigned scope across consequential contracts", "# Implementation"),
        )
        for version in (7, 8, 9, 10, 11):
            for flag, rule, absent in guides:
                with self.subTest(version=version, flag=flag):
                    first = json.loads(self.launch("assigned task", flag).stdout)
                    run = Path(first["run_root"])
                    manifest = run / "manifest.json"
                    state = json.loads(manifest.read_text())
                    state["worker_policy_version"] = version
                    manifest.write_text(json.dumps(state))
                    prefix = (run / "policy.md").read_bytes()
                    original_handoff = Path(first["handoff_path"]).read_bytes()
                    for migrated in (True, False):
                        result = self.access("resume", first["run_id"])
                        self.assertEqual(result.returncode, 0, result.stderr)
                        latest = Path(json.loads(result.stdout)["handoff_path"]).parent.parent
                        phase = (latest / "phase.md").read_text()
                        self.assertEqual(rule in phase, migrated)
                        self.assertEqual("Configured build/cache roots granted by Team" in phase, migrated)
                        self.assertEqual("explicit `--lsp` selects the authoritative server inventory" in phase, migrated)
                        if flag == "--inject-implement":
                            self.assertEqual("Preserve compatibility when it is straightforward and low-cost" in phase, migrated)
                        self.assertNotIn(absent, phase)
                        self.assertEqual((latest / "policy.md").read_bytes(), prefix)
                        self.assertEqual((run / "policy.md").read_bytes(), prefix)
                        self.assertEqual(Path(first["handoff_path"]).read_bytes(), original_handoff)
                        self.assertEqual(json.loads(manifest.read_text())["worker_policy_version"], 13)

    def test_deprecated_navigation_does_not_remove_tools(self):
        result = self.launch("review", "--navigation", "shell")
        self.assertEqual(result.returncode, 0, result.stderr)
        policy = (Path(json.loads(result.stdout)["handoff_path"]).parent.parent / "policy.md").read_text()
        self.assertIn("### `pira_nav`:", policy)
        for flag in ["--output", "--navigation"]:
            self.assertNotEqual(self.launch("review", flag, "invalid").returncode, 0)

    def test_failures_never_masquerade_as_answers(self) -> None:
        for task in ["crash", "malformed", "failed"]:
            with self.subTest(task=task):
                result = self.launch(task)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertIn("logs:", result.stderr)
        for path in (self.root / "logs").glob("*/manifest.json"):
            self.assertIn(json.loads(path.read_text())["status"], ["failed"])

    def test_native_completed_error_never_publishes_success(self):
        combined = ("--inject-review", "--inject-implement")
        for stage, flags, attempts in (
            ("all", (), 1), ("all", combined, 1),
            ("implementation", combined, 2), ("repair", ("--format", "json"), 2),
        ):
            with self.subTest(stage=stage, flags=flags):
                self.env["TEAM_COMPLETED_ERROR"] = stage
                if stage == "repair":
                    self.env["TEAM_CANDIDATE"] = self.candidate("{broken")
                    self.env["TEAM_REPAIRED"] = self.candidate("{}")
                result = self.launch("Review scoped artifact", *flags)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertIn("fixture native completion error", result.stderr)
                run = max((self.root / "logs").iterdir(), key=lambda p: p.name)
                state = json.loads((run / "manifest.json").read_text())
                self.assertEqual(state["status"], "failed")
                self.assertNotIn("result", state)
                self.assertEqual(len(state["attempts"]), attempts)
                self.assertEqual(state["attempts"][-1]["status"], "failed")
                self.assertIn("fixture native completion error", state["attempts"][-1]["error"])
                self.assertEqual(state["usage"]["input_tokens"], attempts * 17)

    def test_parallel_runs_have_independent_logs(self) -> None:
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            results = list(pool.map(self.launch, ["one", "two"]))
        self.assertEqual([Path(json.loads(r.stdout)["handoff_path"]).read_text() for r in results], ["ANSWER one", "ANSWER two"])
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
        self.assertEqual(self.metadata(receipt)["repairs"], 1)
        self.assertEqual(self.metadata(receipt)["usage"], {"input_tokens":34,"cached_input_tokens":6,"output_tokens":10,"reasoning_output_tokens":4,"cache_write_input_tokens":2})
        self.assertTrue(self.metadata(receipt)["usage_complete"])
        self.assertIn("format repair attempted", result.stderr)
        self.assertNotIn("usage accounting incomplete", result.stderr)
        self.assertEqual(Path(receipt["handoff_path"]).read_text(), '{"findings": [1]}\n')
        run = Path(receipt["run_root"])
        self.assertTrue((run / "validation.json").is_file())
        self.assertEqual((run / "rejected-handoff.txt").read_text(), '{"findings": [1')
        self.assertTrue((run / "repair/candidate.txt").is_file())
        self.assertFalse((run / "repair/codex-home").exists())
        manifest = json.loads((run / "manifest.json").read_text())
        self.assertEqual([a["status"] for a in manifest["attempts"]], ["invalid", "validated"])
        repair = run / "repair"
        requests = [json.loads(line) for line in (repair / "requests.jsonl").read_text().splitlines()]
        thread = next(r for r in requests if r.get("method") == "thread/resume")
        self.assertEqual(thread["params"]["sandbox"], "workspace-write")
        self.assertIn("Format repair only: edit the handoff, not project files", (repair / "phase.md").read_text())
        self.assertEqual((run / "policy.md").read_bytes(), (repair / "policy.md").read_bytes())

    def test_invalid_control_missing_handoff_and_links_fail_closed(self):
        for candidate in [json.dumps("not JSON {"), json.dumps({"status":"completed"}), json.dumps({"status":"unknown","format":"json","content":"{}"})]:
            self.env["TEAM_CANDIDATE"] = candidate
            result = self.launch("review")
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "")
        self.env["TEAM_CANDIDATE"] = self.candidate("{}")
        target = self.root / "outside-handoff.json"; target.write_text("{}")
        for key, value in [("TEAM_SKIP_HANDOFF", "1"), ("TEAM_HANDOFF_SYMLINK", str(target)), ("TEAM_HANDOFF_HARDLINK", str(target))]:
            self.env[key] = value
            result = self.launch("review")
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertEqual(target.read_text(), "{}")
            del self.env[key]

    def test_schema_and_csv_constraints(self) -> None:
        schema = self.root / "schema.json"
        schema.write_text(json.dumps({"type":"object", "required":["n"], "$defs":{"number":{"type":"integer"}}, "properties":{"n":{"$ref":"#/$defs/number"}}}))
        self.env["TEAM_CANDIDATE"] = self.candidate('{"n":"1"}')
        self.env["TEAM_REPAIRED"] = self.candidate('{"n":1}')
        result = self.launch("extract", "--format", "json", "--schema", str(schema))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.metadata(json.loads(result.stdout))["repairs"], 1)
        self.env["TEAM_CANDIDATE"] = self.candidate('a,b\n1\n', "table.csv", "csv")
        self.env["TEAM_REPAIRED"] = self.candidate('a,b\n1,2\n', "table.csv", "csv")
        result = self.launch("extract", "--format", "csv", "--columns", '["a","b"]')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(json.loads(result.stdout)["handoff_path"]).read_text(), 'a,b\n1,2\n')

    def test_csv_preserves_quoted_multiline_and_varying_size_rows(self) -> None:
        content = 'a,b\n"long, quoted field","line one\nline two"\nx,λ\n"","escaped ""quote"""\n'
        self.env["TEAM_CANDIDATE"] = self.candidate(content, "table.csv", "csv")
        result = self.launch("extract", "--format", "csv", "--columns", '["a","b"]')
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(self.metadata(receipt)["repairs"], 0)
        self.assertEqual(Path(receipt["handoff_path"]).read_bytes(), content.encode())

    def test_csv_rejects_late_row_width_mismatch(self) -> None:
        for row in ["3", "3,4,5"]:
            with self.subTest(row=row):
                content = f"a,b\n1,2\n{row}\n"
                self.env["TEAM_CANDIDATE"] = self.candidate(content, "table.csv", "csv")
                self.env["TEAM_REPAIRED"] = self.env["TEAM_CANDIDATE"]
                result = self.launch("extract", "--format", "csv", "--columns", '["a","b"]')
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                run_id = next(line.split(": ", 1)[1] for line in result.stderr.splitlines()
                              if line.startswith("pira_team run_id:"))
                diagnostics = self.access("read", run_id, "repair/validation.json")
                self.assertEqual(diagnostics.returncode, 0, diagnostics.stderr)
                self.assertIn("CSV row:", json.loads(diagnostics.stdout)["error"])

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
        self.assertIn("usage accounting incomplete", result.stderr)
        self.assertIn("format repair attempted", result.stderr)
        self.assertEqual(manifest["attempts"][1]["status"], "failed")

    def test_no_execution_deadline_and_legacy_resume_limit_ignored(self) -> None:
        result = self.launch("review")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        path = Path(receipt["run_root"]) / "manifest.json"
        state = json.loads(path.read_text())
        self.assertIsNone(state["timeout_seconds"])
        state["timeout_seconds"] = 1
        path.write_text(json.dumps(state))
        self.env["TEAM_DELAY"] = "1.2"
        resumed = self.access("resume", receipt["run_id"], "--task", "recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertIsNone(json.loads(path.read_text())["timeout_seconds"])
        self.assertNotEqual(self.launch("review", "--timeout", "1").returncode, 0)

    def test_combined_decision_pauses_and_resume_retains_write_permission(self) -> None:
        decision = {"status": "needs_decision", "question": "Which behavior is intended?",
                    "options": ["Preserve behavior: compatibility", "Change behavior: simpler contract"],
                    "partial_work": "No edits; implementation awaits this choice."}
        self.env["TEAM_CANDIDATE"] = json.dumps({"status":"needs_decision","format":"json","content":json.dumps(decision)})
        result = self.launch("review", "--code-review", "--allow-fix", "--format", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], "needs_decision")
        self.assertEqual(self.metadata(receipt)["revision"], 1)
        self.assertEqual(json.loads(self.access("read", receipt["run_id"]).stdout), decision)
        self.env["TEAM_CANDIDATE"] = self.candidate("{}")
        resumed = self.access("resume", receipt["run_id"], "--task", "Preserve behavior; complete fixes.")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(json.loads(resumed.stdout)["status"], "completed")
        self.assertEqual(self.metadata(json.loads(resumed.stdout))["revision"], 2)
        state = json.loads(self.access("read", receipt["run_id"], "manifest.json").stdout)
        self.assertEqual(state["sandbox"], "workspace-write")

    def test_crash_retains_partial_answer_and_observed_usage(self) -> None:
        result = self.launch("partial-crash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        run = next((self.root / "logs").iterdir())
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "failed")
        self.assertEqual(state["usage"]["input_tokens"], 11)
        self.assertFalse(state["usage_complete"])
        self.assertEqual((run / "candidate.txt").read_text(), "partial diagnostic")
        self.assertEqual(self.access("resume", run.name, "--task", "recall").returncode, 0)

    def test_fix_decision_preserves_permission_and_prior_artifact(self) -> None:
        reviewed = self.launch("review", "--code-review")
        receipt = json.loads(reviewed.stdout)
        original = Path(receipt["handoff_path"]).read_bytes()
        decision = {"status": "needs_decision", "question": "Change the public API?",
                    "options": ["Keep it compatible"], "partial_work": "No edits."}
        self.env["TEAM_CANDIDATE"] = json.dumps({"status":"needs_decision","format":"json","content":json.dumps(decision)})
        fix = self.access("resume", receipt["run_id"], "--allow-fix")
        self.assertEqual(fix.returncode, 0, fix.stderr)
        self.assertEqual(json.loads(fix.stdout)["status"], "needs_decision")
        self.assertEqual(Path(receipt["handoff_path"]).read_bytes(), original)
        self.env.pop("TEAM_CANDIDATE")
        resumed = self.access("resume", receipt["run_id"], "--task", "Keep compatibility.")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        state = json.loads(self.access("read", receipt["run_id"], "manifest.json").stdout)
        self.assertEqual(state["sandbox"], "workspace-write")
        self.assertEqual(state["status"], "completed")

    def test_blocker_bypasses_schema_but_not_format(self):
        schema = self.root / "schema.json"; schema.write_text('{"type":"integer"}')
        self.env["TEAM_CANDIDATE"] = json.dumps({"status":"needs_decision", "format":"json", "content":"{"})
        self.env["TEAM_REPAIRED"] = json.dumps({"status":"needs_decision", "format":"json", "content":'{"question":"Change contract?","options":["Keep compatibility"],"partial_work":"No edits"}'})
        result = self.launch("review", "--format", "json", "--schema", str(schema))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "needs_decision")
        self.assertEqual(self.metadata(json.loads(result.stdout))["repairs"], 1)

    def test_temp_defaults_use_scratch_without_ambient_root_grants(self) -> None:
        forbidden = []
        for key in ("TMPDIR", "TMP", "TEMP"):
            path = self.root / ("ambient-" + key)
            path.mkdir()
            self.env[key] = str(path)
            forbidden.append(str(path))
        self.env["TEAM_ASSERT_TEMP"] = "1"
        self.env["TEAM_FORBIDDEN_TEMP_ROOTS"] = json.dumps(forbidden)
        result = self.launch("review")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        # Same scratch contract applies to resumed turns and output-format repairs.
        self.env["TEAM_CANDIDATE"] = self.candidate("[")
        self.env["TEAM_REPAIRED"] = self.candidate("{}")
        resumed = self.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(self.metadata(json.loads(resumed.stdout))["repairs"], 1)
        for path in forbidden:
            self.assertEqual(list(Path(path).iterdir()), [])

    def test_control_errors_report_persisted_state_without_launching(self) -> None:
        # Control needs only persisted state; no backend or listening owner is required.
        run = self.root / "logs" / "stopped-run"
        run.mkdir(parents=True, mode=0o700)
        manifest = run / "manifest.json"
        state = {}
        for status, turn, expected in (
            ("completed", None, "read the handoff; use explicit resume"),
            ("running", None, "starting or between turns"),
            ("running", "old-turn", "owner may be stopping or unavailable"),
            ("interrupted", None, "run stopped"),
        ):
            with self.subTest(status=status, turn=turn):
                state.update(status=status, active_turn=turn, revision=2)
                manifest.write_text(json.dumps(state))
                before = {str(p): p.read_bytes() for p in run.rglob("*") if p.is_file()}
                for op in ("steer", "interrupt"):
                    flags = ("--task", "more work") if op == "steer" else ()
                    result = self.access(op, run.name, *flags)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(f"persisted status={status}, revision=2", result.stderr)
                    self.assertIn(expected, result.stderr)
                    self.assertEqual(result.stdout, "")
                self.assertEqual({str(p): p.read_bytes() for p in run.rglob("*") if p.is_file()}, before)

        manifest.write_text("invalid JSON")
        result = self.access("interrupt", run.name)
        self.assertIn("persisted lifecycle/revision unavailable", result.stderr)

    def test_environment_inheritance_exclusions_and_resume(self) -> None:
        for key in ("CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR", "TMPDIR"):
            self.env[key] = str(self.root / key.lower())
        self.env["TEAM_ASSERT_BUILD_ENV"] = "1"
        self.env.update({
            "PRIVATE_ACCESS_TOKEN": "synthetic-not-a-secret",
            "CODEX_API_KEY": "synthetic-backend-auth",
            "AWS_SECRET_ACCESS_KEY": "synthetic-aws-key",
            "GH_TOKEN": "synthetic-github-token",
            "SSH_AUTH_SOCK": "/synthetic/agent.sock",
            "CODEX_SESSION_ID": "parent-session",
            "PIRA_CTX_THREAD_ID": "parent-context",
            "PIRA_TEAM_HANDOFF": "stale-parent-handoff",
            "FONTCONFIG_FILE": str(self.root / "fonts config.conf"),
            "CARGO_BUILD_JOBS": "2",
            "UNRECOGNIZED_TASK_OPTION": "spaces 'quotes'\nλ",
            "TASK_KEYWORDS": "not-a-credential",
        })
        result = self.launch("review")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.env["UNRECOGNIZED_TASK_OPTION"] = "changed on resume"
        resumed = self.access("resume", json.loads(result.stdout)["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)

    def test_backend_capability_rejection_precedes_a_model_turn(self):
        for fault in ("method", "field", "required"):
            with self.subTest(fault=fault):
                self.env["TEAM_BAD_BACKEND"] = fault
                result = self.launch("review")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unsupported Codex backend", result.stderr)
                self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    def test_configured_build_grants_survive_resume_and_format_repair(self):
        cache = tempfile.TemporaryDirectory(prefix="team-cache-")
        self.addCleanup(cache.cleanup)
        cache_root = Path(cache.name).resolve()
        context = self.permission_context()
        context["sandbox_policy"]["writable_roots"] = [str(cache_root)]
        self.parent([context])
        roots = []
        for key in ("CARGO_HOME", "CARGO_TARGET_DIR", "UV_CACHE_DIR", "npm_config_cache", "GOCACHE", "GRADLE_USER_HOME"):
            directory = cache_root / key
            directory.mkdir()
            self.env[key] = str(directory)
            roots.append(str(directory))
        extra = cache_root / "explicit build output"
        extra.mkdir()
        roots.append(str(extra))
        self.env["PIRA_TEAM_BUILD_ROOTS"] = json.dumps([str(extra), roots[0]])
        unknown = cache_root / "unknown"
        unknown.mkdir()
        self.env["UNRECOGNIZED_OUTPUT_DIR"] = str(unknown)
        self.env["TEAM_EXPECT_BUILD_ROOTS"] = json.dumps(roots)
        self.env["TEAM_FORBIDDEN_BUILD_ROOTS"] = json.dumps([str(unknown)])
        self.env["TEAM_CANDIDATE"] = json.dumps({"format":"json", "content":"invalid json"})
        self.env["TEAM_REPAIRED"] = json.dumps({"format":"json", "content":"{}"})
        first = self.launch("review", "--format", "json")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        self.assertEqual(self.metadata(receipt)["repairs"], 1)
        self.assertTrue(set(roots) <= set(self.metadata(receipt)["build_roots"]))
        changed = cache_root / "changed cache"
        changed.mkdir()
        self.env["UV_CACHE_DIR"] = str(changed)
        roots.remove(str(cache_root / "UV_CACHE_DIR"))
        roots.append(str(changed))
        self.env["TEAM_EXPECT_BUILD_ROOTS"] = json.dumps(roots)
        resumed = self.access("resume", receipt["run_id"])
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertTrue(set(roots) <= set(self.metadata(receipt)["build_roots"]))

    def test_nested_workers_are_rejected(self) -> None:
        self.env["PIRA_TEAM_CHILD"] = "1"
        for op in ("run", "resume", "steer", "interrupt", "read", "path"):
            with self.subTest(op=op):
                result = subprocess.run([str(self.bin), op], env=self.env, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("nested delegation is forbidden", result.stderr)
                self.assertFalse((self.root / "logs").exists())

    def test_task_file_and_space_paths(self) -> None:
        task_file = self.root / "review task.txt"
        task_file.write_text("review multiple\nlines")
        cwd = self.root / "source with spaces"
        cwd.mkdir()
        result = subprocess.run(
            [str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
             "--store", str(self.root / "logs with spaces"), "--cwd", str(cwd),
             "--task-file", str(task_file)],
            env=self.env, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(Path(json.loads(result.stdout)["handoff_path"]).read_text(), "ANSWER review multiple\nlines")

    def test_file_auth_reused_and_detached_on_run_and_resume(self) -> None:
        source = self.root / "original-home/auth.json"
        original = b'{"synthetic": "not-a-real-credential"}'
        source.write_bytes(original)
        self.env["TEAM_TEST_AUTH_SOURCE"] = str(source)
        first = self.launch("review")
        self.assertEqual(first.returncode, 0, first.stderr)
        receipt = json.loads(first.stdout)
        link = Path(receipt["run_root"]) / "codex-home/auth.json"
        self.assertFalse(link.exists())
        self.assertFalse(link.is_symlink())
        resumed = self.access("resume", receipt["run_id"], "--task", "followup")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertFalse(link.exists())
        self.assertFalse(link.is_symlink())
        self.assertEqual(source.read_bytes(), original)

    def test_empty_codex_home_uses_home_auth_and_detaches_on_startup_failure(self) -> None:
        home = self.root / "synthetic-user-home"
        source = home / ".codex/auth.json"
        source.parent.mkdir(parents=True)
        original = b'{"synthetic": "not-a-real-credential"}'
        source.write_bytes(original)
        import shutil
        shutil.copytree(self.root / "original-home/sessions", home / ".codex/sessions")
        self.env.update(CODEX_HOME="", HOME=str(home), USERPROFILE=str(home),
                        TEAM_TEST_AUTH_SOURCE=str(source), TEAM_BAD_PERMISSION="1")
        # Permission rejection is intentional: validate startup/auth without a paid turn
        # or the live control endpoint required by turn execution.
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not confirm workspace-write", result.stderr)
        run = next((self.root / "logs").iterdir())
        self.assertEqual((run / "stderr.log").read_text(), "")
        requests = [json.loads(line) for line in (run / "requests.jsonl").read_text().splitlines()]
        self.assertNotIn("turn/start", [r["method"] for r in requests])
        self.assertFalse((run / "codex-home/auth.json").exists())
        self.assertFalse((run / "codex-home/auth.json").is_symlink())
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
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--cwd", str(self.root), "interrupt"],
            env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        os.close(write_fd)
        try:
            self.assert_worker_ready(proc, read_fd)
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
            "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high", "--store", str(self.root / "logs"),
            "--cwd", str(self.root), "steerable"]
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
            # Drain the terminal while waiting: receipts must not block on PTY backpressure.
            deadline = time.monotonic() + 5
            while proc.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], .1)[0]:
                    try: output += os.read(master, 65536)
                    except OSError: break
            self.assertEqual(proc.wait(timeout=1), 0, output.decode(errors="replace"))
            self.assertEqual(self.access("read", run_id).stdout, "ANSWER terminal-controlled")
        finally:
            os.close(master)
            if proc.poll() is None: proc.terminate(); proc.wait(timeout=8)

    @unittest.skipUnless(os.name == "posix", "POSIX signal lifecycle")
    def test_signal_cancels_blocked_request_write(self) -> None:
        read_fd, write_fd = os.pipe()
        self.env.update(TEAM_READY_FD=str(write_fd), TEAM_BLOCK_INPUT="1")
        task = self.root / "large-task.txt"
        task.write_text("read-only inspection " * 50000)
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--cwd", str(self.root), "--task-file", str(task)],
            env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        os.close(write_fd)
        try:
            self.assert_worker_ready(proc, read_fd)
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
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--cwd", str(self.root), "review"],
            env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            run_id = self.wait_stderr_notice(proc, "pira_team run_id:").strip().split(": ", 1)[1]
            self.wait_stderr_notice(proc, "repair=true")
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

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_stalled_cancellation_is_bounded_and_resumable(self) -> None:
        read_fd, write_fd = os.pipe()
        self.env["TEAM_READY_FD"] = str(write_fd)
        self.env["TEAM_IGNORE_INTERRUPT"] = "1"
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
            "--store", str(self.root / "logs"), "--cwd", str(self.root), "--task", "interrupt"],
            env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        os.close(write_fd)
        try:
            self.assert_worker_ready(proc, read_fd)
            self.wait_stderr_notice(proc, "pira_team active:")
            run = next((self.root / "logs").iterdir())
            control = subprocess.run([str(self.bin), "interrupt", run.name,
                "--store", str(self.root / "logs")], env=self.env,
                capture_output=True, text=True, timeout=10)
            self.assertNotEqual(control.returncode, 0)  # no acknowledgement was received
            out, err = proc.communicate(timeout=9)
            self.assertNotEqual(proc.returncode, 0, err)
            self.assertFalse(out)
            run = next((self.root / "logs").iterdir())
            self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
            self.assertFalse((run / "control.json").exists())
            self.env.pop("TEAM_IGNORE_INTERRUPT")
            self.env.pop("TEAM_READY_FD")
            self.assertEqual(self.access("resume", run.name, "recall").returncode, 0)
        finally:
            os.close(read_fd)
            if proc.poll() is None:
                proc.kill()
                proc.communicate(timeout=5)

    def test_resume_retains_context_revisions_and_profile(self) -> None:
        first = json.loads(self.launch("remember-blue").stdout)
        run = first["run_id"]
        resumed = self.access("resume", run, "recall")
        self.assertEqual(resumed.returncode, 0, resumed.stderr)
        second = json.loads(resumed.stdout)
        self.assertEqual(self.metadata(second)["revision"], 2)
        self.assertEqual(self.metadata(second)["usage"]["input_tokens"], 34)
        self.assertEqual(self.metadata(second)["revision_usage"]["input_tokens"], 17)
        self.assertTrue(self.metadata(second)["usage_complete"])
        self.assertEqual(json.loads(self.access("read", run).stdout), ["remember-blue", "recall"])
        self.assertEqual(self.access("read", run, str(Path(first["handoff_path"]).relative_to(first["run_root"]))).stdout, "ANSWER remember-blue")
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
        self.assertEqual(self.access("read", first["run_id"], str(Path(first["handoff_path"]).relative_to(first["run_root"]))).stdout, "1")

    def test_legacy_or_orphaned_runs_do_not_resume(self) -> None:
        receipt = json.loads(self.launch("review").stdout)
        manifest_path = Path(receipt["run_root"]) / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["status"] = "running"
        manifest_path.write_text(json.dumps(manifest))
        self.assertIn("ambiguous recovery", self.access("resume", receipt["run_id"]).stderr)
        manifest["status"] = "completed"
        del manifest["transport"]
        manifest_path.write_text(json.dumps(manifest))
        self.assertIn("legacy ephemeral", self.access("resume", receipt["run_id"]).stderr)
        self.assertEqual(self.access("read", receipt["run_id"]).stdout, "ANSWER review")

    def test_runtime_must_confirm_exact_caller_permissions(self) -> None:
        self.env["TEAM_BAD_PERMISSION"] = "1"
        result = self.launch("review")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not confirm workspace-write", result.stderr)
        run = next((self.root / "logs").iterdir())
        self.assertNotIn('"method":"turn/start"', (run / "requests.jsonl").read_text())

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_interrupt_is_not_blocked_by_pending_steer_acknowledgement(self) -> None:
        import select
        import time
        read_fd, write_fd = os.pipe()
        self.env.update(TEAM_STALL_STEER="1", TEAM_STEER_READY_FD=str(write_fd))
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report actual checks",
            "--model", "test-model", "--effort", "high", "--cwd", str(self.root),
            "--store", str(self.root / "logs"), "--task", "steerable"],
            env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        os.close(write_fd)
        steer = None
        try:
            deadline = time.monotonic() + 5
            endpoints = []
            while time.monotonic() < deadline:
                endpoints = list((self.root / "logs").glob("*/control.json"))
                if endpoints or proc.poll() is not None:
                    break
                time.sleep(.01)
            self.assertEqual(len(endpoints), 1)
            run = endpoints[0].parent
            steer = subprocess.Popen([str(self.bin), "steer", run.name, "--task", "replacement",
                "--completion-gate", "Replacement reported", "--store", str(self.root / "logs")],
                env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            # Backend receipt, not a timing guess: the steer is in flight without an ACK.
            self.assertTrue(select.select([read_fd], [], [], 5)[0])
            self.assertEqual(os.read(read_fd, 1), b"1")
            result = self.access("interrupt", run.name)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["status"], "accepted")
            stdout, stderr = proc.communicate(timeout=8)
            self.assertNotEqual(proc.returncode, 0, stderr)
            self.assertEqual(stdout, "")
            state = json.loads((run / "manifest.json").read_text())
            self.assertEqual(state["status"], "interrupted")
            self.assertEqual(state["task"], "steerable")
            self.assertFalse((run / "control.json").exists())
            stdout, stderr = steer.communicate(timeout=5)
            self.assertNotEqual(steer.returncode, 0)
            self.assertEqual(stdout, "")
            self.assertIn("delivery outcome unknown", stderr)
        finally:
            os.close(read_fd)
            for child in (steer, proc):
                if child is not None:
                    if child.poll() is None:
                        child.terminate()
                    child.communicate(timeout=8)

    @unittest.skipUnless(os.name == "posix", "POSIX stalled-pipe fixture")
    def test_remote_interrupt_stops_backend_that_no_longer_reads_stdin(self):
        import socket
        self.env["TEAM_BLOCK_ACTIVE_INPUT"] = "1"
        proc, run, endpoint = self.start_control_fixture()
        streams = []
        try:
            # Exceed ordinary pipe capacity without exceeding control admission limits.
            # The peer has stopped reading immediately after acknowledging turn/start.
            address, port = endpoint["address"].rsplit(":", 1)
            for _ in range(24):
                stream = socket.create_connection((address, int(port)), timeout=3)
                streams.append(stream)
                frame = {"token": endpoint["token"], "turn_id": endpoint["turn_id"],
                         "operation": "steer", "task": "x" * 60000, "completion_gate": "Report"}
                stream.sendall((json.dumps(frame) + "\n").encode())
            control = subprocess.run([str(self.bin), "interrupt", run.name,
                "--store", str(self.root / "logs")], env=self.env,
                capture_output=True, text=True, timeout=12)
            self.assertNotEqual(control.returncode, 0)  # peer never acknowledged
            self.assertIn("delivery outcome unknown", control.stderr)
            out, err = proc.communicate(timeout=8)
            self.assertNotEqual(proc.returncode, 0, err)
            self.assertEqual(out, "")
            state = json.loads((run / "manifest.json").read_text())
            self.assertEqual(state["status"], "interrupted")
            self.assertEqual(state["task"], "steerable")
            self.assertFalse((run / "control.json").exists())
            self.env.pop("TEAM_BLOCK_ACTIVE_INPUT")
            self.assertEqual(self.access("resume", run.name, "recall").returncode, 0)
        finally:
            for stream in streams:
                stream.close()

    def start_control_fixture(self, *, pass_fds: tuple[int, ...] = (), extra: tuple[str, ...] = ()):
        import time
        proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report actual checks",
            "--model", "test-model", "--effort", "high", "--cwd", str(self.root),
            "--store", str(self.root / "logs"), *extra, "--task", "steerable"],
            env=self.env, pass_fds=pass_fds, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.communicate(timeout=3)
        self.addCleanup(cleanup)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            endpoints = list((self.root / "logs").glob("*/control.json"))
            if endpoints:
                endpoint, = endpoints
                return proc, endpoint.parent, json.loads(endpoint.read_text())
            if proc.poll() is not None:
                self.fail(proc.communicate(timeout=1)[1])
            time.sleep(.01)
        self.fail("control endpoint not created")

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_interrupt_retains_admission_when_steer_replies_are_saturated(self) -> None:
        import select
        import socket
        read_fd, write_fd = os.pipe()
        self.addCleanup(os.close, read_fd)
        self.env.update(TEAM_STALL_STEER="1", TEAM_STEER_READY_FD=str(write_fd))
        try:
            proc, run, endpoint = self.start_control_fixture(pass_fds=(write_fd,))
        finally:
            os.close(write_fd)
        host, port = endpoint["address"].split(":")
        def request(operation="steer", token=None):
            client = socket.create_connection((host, int(port)), timeout=5)
            self.addCleanup(client.close)
            client.sendall((json.dumps({"token":endpoint["token"] if token is None else token,
                "turn_id":endpoint["turn_id"], "operation":operation,
                "task":"replacement", "completion_gate":"Replacement reported"}) + "\n").encode())
            return client
        clients = []
        for _ in range(64):
            clients.append(request())
            # Ensure every slot represents a delivered, unacknowledged native steer,
            # not requests still queued for processing by the owner.
            self.assertTrue(select.select([read_fd], [], [], 5)[0])
            self.assertEqual(os.read(read_fd, 1), b"1")
        for client, error in [(request(), "too many pending"),
                              (request("interrupt", "bad"), "invalid control capability")]:
            with client.makefile("rb") as stream:
                reply = json.loads(stream.readline())
            self.assertEqual(reply["status"], "rejected")
            self.assertIn(error, reply["error"])
        result = self.access("interrupt", run.name)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "accepted")
        stdout, stderr = proc.communicate(timeout=8)
        self.assertNotEqual(proc.returncode, 0, stderr)
        self.assertEqual(stdout, "")
        state = json.loads((run / "manifest.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        self.assertEqual(state["task"], "steerable")
        self.assertFalse((run / "control.json").exists())
        for client in clients:
            with client.makefile("rb") as stream:
                reply = json.loads(stream.readline())
            self.assertEqual(reply["status"], "rejected")
            self.assertIn("delivery outcome unknown", reply["error"])

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_slow_control_frame_has_total_budget_and_does_not_hold_shutdown(self) -> None:
        import socket
        import threading
        proc, run, endpoint = self.start_control_fixture()
        host, port = endpoint["address"].split(":")
        stopped = threading.Event()
        clients = []
        threads = []
        def trickle(client):
            while not stopped.is_set():
                try:
                    client.sendall(b" ")
                except OSError:
                    return
                stopped.wait(.05)
        try:
            client = socket.create_connection((host, int(port)), timeout=5)
            clients.append(client)
            thread = threading.Thread(target=trickle, args=(client,))
            threads.append(thread)
            thread.start()
            # Bytes arrive much faster than the old per-read three-second timeout;
            # only a total assembly budget can reject this unfinished frame.
            with client.makefile("rb") as stream:
                reply = json.loads(stream.readline())
            self.assertEqual(reply["status"], "rejected")
            self.assertIn("control frame assembly timed out", reply["error"])
            # A second unfinished connection must not trap Endpoint::drop while
            # the native turn is ending. Signal cancellation retains normal semantics.
            client = socket.create_connection((host, int(port)), timeout=5)
            clients.append(client)
            client.sendall(b" ")
            thread = threading.Thread(target=trickle, args=(client,))
            threads.append(thread)
            thread.start()
            proc.terminate()
            stdout, stderr = proc.communicate(timeout=2)
            self.assertNotEqual(proc.returncode, 0, stderr)
            self.assertEqual(stdout, "")
            self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
            self.assertFalse((run / "control.json").exists())
        finally:
            stopped.set()
            for client in clients:
                client.close()
            for thread in threads:
                thread.join(timeout=2)
                self.assertFalse(thread.is_alive())

    @unittest.skipUnless(os.name == "posix", "POSIX lifecycle fixture")
    def test_active_steer_interrupt_and_exclusive_resume(self) -> None:
        import socket
        for operation in ("steer", "interrupt"):
            read_fd, write_fd = os.pipe()
            self.env["TEAM_READY_FD"] = str(write_fd)
            proc = subprocess.Popen([str(self.bin), "run", "--completion-gate", "Report findings and actual checks", "--model", "test-model", "--effort", "high",
                "--store", str(self.root / "logs"), "--cwd", str(self.root), "steerable"],
                env=self.env, pass_fds=(write_fd,), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            os.close(write_fd)
            try:
                self.assert_worker_ready(proc, read_fd)
                # Read the launcher's advertised ID, then synchronize with endpoint creation.
                run_id = self.wait_stderr_notice(proc, "pira_team run_id:").strip().split(": ", 1)[1]
                run = self.root / "logs" / run_id
                # A native response precedes endpoint creation; keep the wait bounded.
                import time
                deadline = time.monotonic() + 3
                while not (run / "control.json").exists() and time.monotonic() < deadline:
                    if proc.poll() is not None:
                        self.wait_stderr_notice(proc, "pira_team active:")
                    time.sleep(.01)
                if not (run / "control.json").exists():
                    self.fail_startup(proc, "control endpoint readiness timed out")
                endpoint = json.loads((run / "control.json").read_text())
                self.assertIn("active owner", self.access("resume", run_id, "collision").stderr)
                self.assertNotEqual(self.access("read", run_id, "control.json").returncode, 0)
                for token, turn in [("bad", endpoint["turn_id"]), (endpoint["token"], "stale")]:
                    host, port = endpoint["address"].split(":")
                    with socket.create_connection((host, int(port)), 2) as client:
                        client.sendall((json.dumps({"token":token, "turn_id":turn, "operation":"interrupt", "task":""})+"\n").encode())
                        reply = json.loads(client.makefile().readline())
                    self.assertEqual(reply["status"], "rejected")
                extra = ["--task", "changed task", "--completion-gate", "Redirected task reported"] if operation == "steer" else []
                controlled = self.access(operation, run_id, *extra)
                self.assertEqual(controlled.returncode, 0, controlled.stderr)
                self.assertEqual(json.loads(controlled.stdout)["status"], "accepted")
                stdout, stderr = proc.communicate(timeout=5)
                if operation == "steer":
                    self.assertEqual(proc.returncode, 0, stderr)
                    self.assertEqual(Path(json.loads(stdout)["handoff_path"]).read_text(), "ANSWER changed task")
                    state = json.loads((run / "manifest.json").read_text())
                    self.assertEqual(state["completion_gate"], "Redirected task reported")
                    self.assertEqual(state["task"], "changed task")
                    self.assertEqual(self.metadata(json.loads(stdout))["completion_gate"], state["completion_gate"])
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
