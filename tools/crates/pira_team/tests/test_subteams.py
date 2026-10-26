"""Real isolated subprocesses; fake protocol peer, no model calls or live stores."""
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest
import test_team
from native_fixture import Boundary, process_exit_waiter, wait_owner


class SubteamTests(unittest.TestCase):
    parent = test_team.TeamTests.parent
    permission_context = test_team.TeamTests.permission_context
    fail_startup = test_team.TeamTests.fail_startup
    wait_stderr_notice = test_team.TeamTests.wait_stderr_notice
    launch = test_team.TeamTests.launch

    def setUp(self):
        test_team.TeamTests.setUp(self)
        for name in list(self.env):
            backend_setting = name.upper().startswith(("OPENAI_", "AZURE_OPENAI_", "CODEX_")) or name.upper() in {
                "HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","NO_PROXY","SSL_CERT_FILE","SSL_CERT_DIR","CURL_CA_BUNDLE",
                "LD_PRELOAD","LD_LIBRARY_PATH","LD_AUDIT","DYLD_INSERT_LIBRARIES","DYLD_LIBRARY_PATH","DYLD_FRAMEWORK_PATH",
                "DYLD_FORCE_FLAT_NAMESPACE","PYTHONHOME","PYTHONPATH","PYTHONSTARTUP"}
            if backend_setting and name not in ("CODEX_HOME", "CODEX_THREAD_ID"):
                self.env.pop(name)
        sockets = tempfile.TemporaryDirectory(prefix="team-sock-", dir="/tmp" if os.name == "posix" else None)
        self.addCleanup(sockets.cleanup)
        self.sockets = Path(sockets.name)
        self.env.update(TEAM_SOCKET_DIR=str(self.sockets), TEAM_SUBTEAM_BIN=str(self.bin),
                        TEAM_SUBTEAM_FIXTURE=str(Path(__file__).with_name("subteam_fixture.py").resolve()))
        self.processes = []
        self.addCleanup(self.stop_processes)

    def stop_processes(self):
        for process, run in reversed(self.processes):
            self.root_command("interrupt", "-r", run)
            try:
                process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.communicate(timeout=10)

    def root_command(self, *args):
        return subprocess.run([str(self.bin), *map(str, args), "--store", str(self.root / "logs")],
                              env=self.env, capture_output=True, text=True, timeout=30)

    def start(self, *, allow=True, resume=None, extra=(), pass_fds=()):
        args = (["resume", resume] if resume else ["run", "--model", "test-model", "--effort", "high", "--cwd", str(self.root)])
        args += ["--task", "subteam-service", "--completion-gate", "Integrate required child work", *extra]
        if allow:
            args += ["--allow-subteam"]
        process = subprocess.Popen([str(self.bin), *args, "--store", str(self.root / "logs")], env=self.env,
                                   pass_fds=pass_fds, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            self.wait_stderr_notice(process, "pira_team active:")
        except AssertionError as error:
            logs = "\n".join(p.read_text() for p in (self.root / "logs").glob("*/stderr.log"))
            raise AssertionError(f"{error}\nbackend diagnostics:\n{logs}") from error
        runs = [p.parent for p in (self.root / "logs").glob("*/control.json")]
        run = next(p for p in runs if json.loads((p / "manifest.json").read_text())["parent_run_id"] is None
                   and p.name not in [r for _, r in self.processes]) if not resume else self.root / "logs" / resume
        self.processes.append((process, run.name))
        return process, run.name

    def request(self, run, **request):
        with socket.socket() as client:
            client.settimeout(30)
            client.connect(tuple(json.loads((self.sockets / (run + ".sock")).read_text())))
            with client.makefile("rwb") as stream:
                stream.write(json.dumps(request).encode() + b"\n")
                stream.flush()
                response = stream.readline()
                if not response:
                    raise ConnectionError("fixture backend exited before replying")
                result = json.loads(response)
        self.assertNotIn("fixture_error", result, result)
        return result

    def command(self, run, *args, **extra):
        return self.request(run, args=list(args), **extra)

    def child(self, parent, task="subteam-service", **extra):
        return self.command(parent, "run", "--task", task, "--completion-gate", "Finish assigned child work", operation="start", **extra)

    def manifest(self, run):
        return json.loads((self.root / "logs" / run / "manifest.json").read_text())

    def finish(self, run):
        self.request(run, operation="finish")

    def wait(self, process):
        stdout, stderr = process.communicate(timeout=15)
        return process.returncode, stdout, stderr

    def test_writing_guidance_child_preserves_store_boundary(self):
        process, coordinator = self.start(extra=("--inject-write",))
        child = self.command(coordinator, "run", "--task", "subteam-service",
                             "--completion-gate", "Finish assigned child work", "--inject-write", operation="start")
        child_id = child["run_id"]
        writing = (Path(__file__).resolve().parents[4] / "modules/SCIENTIFIC_WRITING.md").read_text()
        for run in (coordinator, child_id):
            self.assertTrue(self.manifest(run)["inject_write"])
            self.assertEqual((self.root / "logs" / run / "policy.md").read_text().count(writing), 1)
        escape = self.root / "escape"
        escape.mkdir()
        escaped = self.command(coordinator, "run", "--inject-write", "--store", str(escape),
                               "--task", "unused", "--completion-gate", "unused")
        self.assertNotEqual(escaped["returncode"], 0)
        self.assertIn("retained Team store", escaped["stderr"])
        self.assertFalse(list(escape.iterdir()))
        self.finish(child_id)
        self.assertEqual(self.command(coordinator, operation="wait", handle=child["handle"])["returncode"], 0)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)

    def test_allowed_children_noninheritance_and_scoped_ownership(self):
        process, coordinator = self.start()
        other_process, sibling = self.start()
        child = self.child(coordinator)
        child_id = child["run_id"]
        parent = self.manifest(coordinator)
        native = self.manifest(child_id)
        self.assertTrue(parent["allow_subteam"])
        self.assertFalse(native["allow_subteam"])
        self.assertEqual(native["parent_run_id"], coordinator)
        self.assertEqual(native["execution_permissions"]["sandbox_policy"], parent["execution_permissions"]["sandbox_policy"])
        self.assertEqual(native["creator_main_session_id"], parent["thread_id"])
        self.assertNotIn("# Coordinator Team usage", (self.root / "logs" / child_id / "policy.md").read_text())
        self.assertIn("# Coordinator Team usage", (self.root / "logs" / coordinator / "policy.md").read_text())
        for run in (coordinator, sibling):
            for op in ("read", "path", "interrupt", "bookmark"):
                args = ("--label", "forbidden") if op == "bookmark" else ()
                result = self.command(coordinator, op, run, *args)
                self.assertNotEqual(result["returncode"], 0, result)
        for op in ("read", "path", "interrupt", "bookmark"):
            extra = ("--label", "forbidden") if op == "bookmark" else ()
            self.assertNotEqual(self.root_command(op, child_id, *extra).returncode, 0)
        self.assertEqual(self.command(coordinator, "bookmark", child_id, "--label", "child")["returncode"], 0)
        self.assertEqual(json.loads(self.command(coordinator, "bookmarks")["stdout"])["matches"][0]["label"], "child")
        self.assertEqual(json.loads(self.command(sibling, "bookmarks")["stdout"])["matches"], [])
        self.assertEqual(json.loads(self.root_command("bookmarks").stdout)["matches"], [])
        for args in (("run", "--allow-subteam", "--task", "x"), ("resume", child_id, "--allow-subteam"),
                     ("config", "show")):
            self.assertNotEqual(self.command(coordinator, *args)["returncode"], 0)
        self.assertNotEqual(self.command(child_id, "run", "--task", "x", "--completion-gate", "x")["returncode"], 0)
        self.finish(child_id)
        result = self.request(coordinator, operation="wait", handle=child["handle"])
        self.assertEqual(result["returncode"], 0, result)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)

    def test_forged_environment_cross_store_and_caller_identity(self):
        process, coordinator = self.start()
        outside = self.root / "outside"
        outside.mkdir()
        for args, overrides in (
            (("run", "--store", str(outside), "--task", "x", "--completion-gate", "x"), {}),
            (("read", coordinator, "manifest.json"), {"CODEX_THREAD_ID":"parent-session", "CODEX_HOME":self.env["CODEX_HOME"]}),
            (("run", "--task", "x", "--completion-gate", "x", "--allow-subteam"), {"PIRA_TEAM_CHILD":None, "PIRA_TEAM_CONTEXT":None}),
        ):
            result = self.command(coordinator, *args, env=overrides)
            self.assertNotEqual(result["returncode"], 0, result)
        # Removing environment discovery hints cannot escape actual backend ancestry.
        result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x", env={
            "PIRA_TEAM_CHILD":None, "PIRA_TEAM_CONTEXT":None, "PIRA_TEAM_DIR":str(outside), "CODEX_HOME":self.env["CODEX_HOME"]})
        self.assertEqual(result["returncode"], 0, result)
        self.assertEqual(len(list(outside.iterdir())), 0)
        self.assertEqual(len(json.loads(self.root_command("read", coordinator, "manifest.json").stdout)["descendants"]["runs"]), 1)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)
        fake_env = dict(self.env, PIRA_TEAM_CHILD="1", PIRA_TEAM_CONTEXT=str(self.root / "logs" / coordinator))
        result = subprocess.run([str(self.bin), "read", coordinator, "manifest.json", "--store", str(self.root / "logs")],
                                env=fake_env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ancestry", result.stderr)

    def test_completion_rejected_with_active_child_and_accounting_resume(self):
        process, coordinator = self.start()
        child = self.child(coordinator)
        self.finish(coordinator)
        code, _, stderr = self.wait(process)
        self.assertNotEqual(code, 0)
        self.assertIn("cannot complete while children are active", stderr)
        self.assertEqual(self.manifest(coordinator)["status"], "incomplete")
        active = json.loads(self.root_command("read", coordinator, "manifest.json").stdout)
        self.assertTrue(active["descendants"]["runs"][0]["active"])
        self.assertFalse(active["descendants"]["usage_complete"])
        self.finish(child["run_id"])
        process2, _ = self.start(resume=coordinator, allow=False)
        self.assertEqual(self.manifest(coordinator)["coordinator_policy_version"], 22)
        self.assertNotIn("# Coordinator Team usage", self.manifest(coordinator)["phase_instructions"])
        # Permission is retained; child resume stays owned by coordinator and nondelegating.
        result = self.command(coordinator, "resume", child["run_id"], "--task", "ordinary", "--completion-gate", "x")
        self.assertEqual(result["returncode"], 0, result)
        self.assertFalse(self.manifest(child["run_id"])["allow_subteam"])
        self.finish(coordinator)
        self.assertEqual(self.wait(process2)[0], 0)
        current = json.loads(self.root_command("read", coordinator, "manifest.json").stdout)
        self.assertEqual(current["usage"]["input_tokens"], 34)
        self.assertEqual(current["descendants"]["usage"]["input_tokens"], 34)
        self.assertTrue(current["descendants"]["usage_complete"])
        self.assertEqual(len(current["descendants"]["runs"]), 1)

    def test_target_only_then_recursive_stopped_parent_and_recovery(self):
        process, coordinator = self.start()
        child = self.child(coordinator)
        result = self.root_command("interrupt", coordinator)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["active_children_left_running"], [child["run_id"]])
        self.wait(process)
        self.assertEqual(self.manifest(child["run_id"])["status"], "running")
        result = self.root_command("interrupt", "--recursive", coordinator)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(all(r["stopped"] for r in json.loads(result.stdout)["results"]))
        self.assertEqual(self.manifest(child["run_id"])["status"], "interrupted")
        process2, _ = self.start(resume=coordinator, allow=False)
        # Deliberate root resume opens a new epoch and retains permission.
        result = self.command(coordinator, "resume", child["run_id"], "--task", "ordinary", "--completion-gate", "x")
        self.assertEqual(result["returncode"], 0, result)
        self.finish(coordinator)
        self.assertEqual(self.wait(process2)[0], 0)

    def test_recursive_active_subtree_and_stale_endpoint(self):
        process, coordinator = self.start()
        child = self.child(coordinator)
        (self.root / "logs" / child["run_id"] / "control.json").write_text('{"address":"127.0.0.1:1","token":"stale"}')
        result = self.root_command("interrupt", "-r", coordinator)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "stopped")
        self.assertNotEqual(self.wait(process)[0], 0)
        self.assertEqual(self.manifest(child["run_id"])["status"], "interrupted")
        self.assertTrue(json.loads((self.root / "logs" / coordinator / "tree.json").read_text())["blocked"])

    def test_legacy_default_and_explicit_root_resume_grant(self):
        first = self.launch("ordinary")
        self.assertEqual(first.returncode, 0, first.stderr)
        coordinator = json.loads(first.stdout)["run_id"]
        path = self.root / "logs" / coordinator / "manifest.json"
        manifest = json.loads(path.read_text())
        manifest.pop("allow_subteam")
        manifest["worker_policy_version"] = 17
        path.write_text(json.dumps(manifest))
        original = (path.parent / "policy.md").read_bytes()
        process, _ = self.start(allow=False, resume=coordinator)
        self.assertFalse(self.manifest(coordinator)["allow_subteam"])
        self.assertNotEqual(self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")["returncode"], 0)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)
        process2, _ = self.start(allow=True, resume=coordinator)
        result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")
        self.assertEqual(result["returncode"], 0, result)
        self.assertEqual((path.parent / "policy.md").read_bytes(), original)
        self.assertIn("# Coordinator Team usage", self.manifest(coordinator)["phase_instructions"])
        self.finish(coordinator)
        self.assertEqual(self.wait(process2)[0], 0)
        process3, _ = self.start(allow=False, resume=coordinator)
        self.assertNotIn("# Coordinator Team usage", self.manifest(coordinator)["phase_instructions"])
        self.assertTrue(self.manifest(coordinator)["allow_subteam"])
        self.finish(coordinator)
        self.assertEqual(self.wait(process3)[0], 0)

    def test_recursive_child_startup_and_resume_gate(self):
        boundary = Boundary()
        self.addCleanup(boundary.close)
        try:
            process, coordinator = self.start()
            child = self.child(coordinator, until="run_id", env={
                "TEAM_PAUSE_AT":"schema", **boundary.environment})
            boundary.reached()
            result = self.root_command("interrupt", "-r", coordinator)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.manifest(child["run_id"])["status"], "interrupted")
            self.wait(process)
            tree = json.loads((self.root / "logs" / coordinator / "tree.json").read_text())
            self.assertTrue(tree["blocked"])
            self.assertTrue(all(not member["active"] for member in tree["members"].values()))
        finally:
            boundary.close()

    def test_recursive_crashed_parent_reconciles_and_remains_resumable(self):
        import select
        process, coordinator = self.start()
        child = self.child(coordinator)
        runtime = json.loads((self.root / "logs" / coordinator / "runtime.json").read_text())
        pid = runtime["backend_pid"]
        wait_exit = process_exit_waiter(pid)
        process.kill()  # This fixture owns the live Popen handle, not a persisted PID.
        self.wait(process)
        wait_exit()
        result = self.root_command("interrupt", "-r", coordinator)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.manifest(child["run_id"])["status"], "interrupted")
        self.assertEqual(self.manifest(coordinator)["status"], "interrupted")
        self.assertFalse(self.manifest(coordinator)["usage_complete"])
        recovered = self.manifest(coordinator)
        self.assertTrue(recovered["revisions"][0]["recovered"])
        self.assertEqual(json.loads((self.root / "logs" / coordinator / "revisions/000001/manifest.json").read_text())["status"], "interrupted")
        process2, _ = self.start(resume=coordinator, allow=False)
        self.finish(coordinator)
        self.assertEqual(self.wait(process2)[0], 0)

    def test_completion_serializes_with_racing_child_start(self):
        import threading
        process, coordinator = self.start()
        barrier = threading.Barrier(2)
        def launch_child():
            barrier.wait(timeout=5)
            try:
                return self.child(coordinator)
            except (ConnectionError, FileNotFoundError):
                return None  # Parent's backend exited before command admission.
        def finish_parent():
            barrier.wait(timeout=5)
            self.finish(coordinator)
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            child_future = pool.submit(launch_child)
            finished = pool.submit(finish_parent)
            child = child_future.result(timeout=20)
            finished.result(timeout=20)
        self.wait(process)
        summary = json.loads(self.root_command("read", coordinator, "manifest.json").stdout)
        if child and "run_id" in child:
            self.assertEqual(summary["status"], "incomplete")
            self.assertTrue(summary["descendants"]["runs"][0]["active"])
        if summary["status"] == "completed":
            self.assertFalse(any(r["active"] for r in summary["descendants"]["runs"]))
        else:
            self.assertEqual(summary["status"], "incomplete")
        stopped = self.root_command("interrupt", "-r", coordinator)
        self.assertEqual(stopped.returncode, 0, stopped.stderr)

    def test_recursive_racing_resume_and_child_target_gate(self):
        process, coordinator = self.start()
        first = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")
        self.assertEqual(first["returncode"], 0, first)
        child = json.loads(first["stdout"])["run_id"]
        active = self.command(coordinator, "resume", child, "--task", "subteam-service", "--completion-gate", "x", operation="start")
        self.assertIn("handle", active)
        result = self.command(coordinator, "interrupt", "-r", child)
        self.assertEqual(result["returncode"], 0, result)
        self.assertEqual(self.request(coordinator, operation="wait", handle=active["handle"])["returncode"], 1)
        # A coordinator cannot undo its child's interruption gate by a racing resume.
        result = self.command(coordinator, "resume", child, "--task", "ordinary", "--completion-gate", "x")
        self.assertNotEqual(result["returncode"], 0, result)
        self.assertIn("deliberate root resume", result["stderr"])
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)
        process2, _ = self.start(resume=coordinator, allow=False)
        self.assertEqual(self.command(coordinator, "resume", child, "--task", "ordinary", "--completion-gate", "x")["returncode"], 0)
        self.finish(coordinator)
        self.assertEqual(self.wait(process2)[0], 0)

    def test_recursive_interrupt_during_coordinator_format_repair(self):
        self.env.update(TEAM_REPAIR_WAIT="1", TEAM_CANDIDATE=json.dumps({"format":"json", "content":"{broken"}))
        process, coordinator = self.start(extra=("--format", "json"))
        child = self.child(coordinator, env={"TEAM_CANDIDATE":None, "TEAM_REPAIR_WAIT":None})
        self.finish(coordinator)
        self.wait_stderr_notice(process, "repair=true")
        self.assertEqual(self.manifest(coordinator)["repairs"], 1)
        result = self.root_command("interrupt", "-r", coordinator)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.wait(process)[0], 1)
        self.assertEqual(self.manifest(coordinator)["status"], "interrupted")
        self.assertEqual(self.manifest(child["run_id"])["status"], "interrupted")

    def test_recursive_interrupt_blocks_inflight_child_resume(self):
        boundary = Boundary()
        self.addCleanup(boundary.close)
        try:
            process, coordinator = self.start()
            first = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")
            self.assertEqual(first["returncode"], 0, first)
            child = json.loads(first["stdout"])["run_id"]
            result = self.command(coordinator, "resume", child, "--task", "subteam-service", "--completion-gate", "x",
                operation="start", until="run_id", env={"TEAM_PAUSE_AT":"schema", **boundary.environment})
            self.assertIn("handle", result)
            boundary.reached()
            result = self.root_command("interrupt", "-r", coordinator)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.wait(process)
            self.assertEqual(self.manifest(child)["revision"], 2)
            self.assertEqual(self.manifest(child)["status"], "interrupted")
            self.assertEqual(self.manifest(child)["usage"]["input_tokens"], 17)
            self.assertFalse(self.manifest(child)["usage_complete"])
        finally:
            boundary.close()

    def test_closed_parent_diagnostic_pipe_does_not_orphan_child(self):
        process, coordinator = self.start()
        result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x", operation="broken_output")
        self.assertEqual(result["returncode"], 0, result)
        state = json.loads(self.root_command("read", coordinator, "manifest.json").stdout)
        self.assertEqual(len(state["descendants"]["runs"]), 1)
        self.assertEqual(state["descendants"]["runs"][0]["status"], "completed")
        self.assertFalse(state["descendants"]["runs"][0]["active"])
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)

    def test_nested_file_auth_uses_verified_parent_home(self):
        source = self.root / "original-home/auth.json"
        source.write_text("fixture authentication, not real credentials")
        self.env["TEAM_TEST_AUTH_SOURCE"] = str(source)
        self.env["TEAM_FORBID_API_KEY"] = "1"
        process, coordinator = self.start()
        result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x", env={"CODEX_API_KEY":"synthetic-injected-key"})
        self.assertEqual(result["returncode"], 0, result)
        child = json.loads(result["stdout"])["run_id"]
        self.assertFalse((self.root / "logs" / child / "codex-home/auth.json").exists())
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)

    def test_binding_write_failure_finalizes_owned_backend(self):
        boundary = Boundary()
        self.addCleanup(boundary.close)
        env = dict(self.env, TEAM_PAUSE_AT="schema", **boundary.environment)
        try:
            process = subprocess.Popen([str(self.bin), "run", "--task", "ordinary", "--completion-gate", "x",
                "--model", "test-model", "--effort", "high", "--cwd", str(self.root),
                "--store", str(self.root / "logs")], env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            self.wait_stderr_notice(process, "pira_team run_id:")
            run, = (self.root / "logs").iterdir()
            self.processes.append((process, run.name))
            boundary.reached()
            collision = run / f"runtime.{process.pid}.tmp"
            collision.write_text("preexisting fixture, not owned by binding save")
            boundary.release()
            code, _, stderr = self.wait(process)
            self.assertNotEqual(code, 0)
            self.assertIn(collision.name, stderr)
            self.assertEqual(collision.read_text(), "preexisting fixture, not owned by binding save")
            self.assertEqual(self.manifest(run.name)["status"], "failed")
            self.assertFalse(json.loads((run / "tree.json").read_text())["members"][run.name]["active"])
            self.assertFalse((run / "control.json").exists())
        finally:
            boundary.close()

    def test_root_cannot_impersonate_coordinator_by_selecting_worker_home(self):
        process, coordinator = self.start()
        child = self.child(coordinator)
        forged = dict(self.env, CODEX_THREAD_ID=self.manifest(coordinator)["thread_id"],
                      CODEX_HOME=str(self.root / "logs" / coordinator / "codex-home"))
        for args in (("read", child["run_id"], "manifest.json"), ("bookmark", child["run_id"], "--label", "forged"),
                     ("interrupt", child["run_id"]), ("resume", child["run_id"]),
                     ("run", "--allow-subteam", "--task", "x", "--completion-gate", "x")):
            with self.subTest(args=args):
                result = subprocess.run([str(self.bin), *args, "--store", str(self.root / "logs")],
                                        env=forged, text=True, capture_output=True, timeout=5)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("identity", result.stderr)

    def api_key(self):
        import hashlib
        import secrets
        key = "synthetic-" + secrets.token_hex(24)
        self.env.update(CODEX_API_KEY=key, TEAM_EXPECT_AUTH_SHA256=hashlib.sha256(key.encode()).hexdigest())
        return key

    def no_credential_artifacts(self, *keys):
        for path in (self.root / "logs").rglob("*"):
            if path.is_file() and not path.is_symlink():
                content = path.read_bytes()
                self.assertFalse(any(key.encode() in content for key in keys), f"credential in retained file: {path.name}")

    def test_api_key_handoff_is_launcher_only_and_resumes_use_current_selection(self):
        first_key = self.api_key()
        process, coordinator = self.start()
        child = self.child(coordinator)
        self.assertTrue("run_id" in child, child.get("stderr", "child startup failed").replace(first_key, "<redacted>"))
        child_id = child["run_id"]
        # A worker knowing every ordinary management field still has the wrong
        # kernel peer PID/executable and cannot redeem the child's credential.
        runtime = json.loads((self.root / "logs" / coordinator / "runtime.json").read_text())
        claim = {"child":child_id, "revision":1, "epoch":self.manifest(child_id)["subtree_epoch"], "runtime":runtime}
        probe = self.request(coordinator, operation="auth_probe", claim=claim)
        self.assertFalse(probe["accepted"])
        self.finish(child_id)
        self.assertEqual(self.request(coordinator, operation="wait", handle=child["handle"])["returncode"], 0)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)
        second_key = self.api_key()
        process2, _ = self.start(resume=coordinator, allow=False)
        result = self.command(coordinator, "resume", child_id, "--task", "ordinary", "--completion-gate", "x")
        self.assertEqual(result["returncode"], 0, result["stderr"])
        self.finish(coordinator)
        self.assertEqual(self.wait(process2)[0], 0)
        self.no_credential_artifacts(first_key, second_key)

    def test_selected_auth_cached_for_child_repair_after_target_only_parent_stop(self):
        for mode in ("api-key", "file"):
            with self.subTest(mode=mode):
                key = None
                if mode == "api-key":
                    key = self.api_key()
                else:
                    self.env.pop("CODEX_API_KEY", None)
                    self.env.pop("TEAM_EXPECT_AUTH_SHA256", None)
                    source = self.root / "original-home/auth.json"
                    source.write_text("synthetic file authentication")
                    self.env.update(TEAM_TEST_AUTH_SOURCE=str(source), TEAM_FORBID_API_KEY="1")
                process, coordinator = self.start()
                candidate = json.dumps({"format":"json", "content":"{broken"})
                repaired = json.dumps({"format":"json", "content":"{}"})
                child = self.command(coordinator, "run", "--task", "subteam-service", "--completion-gate", "x", "--format", "json",
                    operation="start", env={"TEAM_CANDIDATE":candidate, "TEAM_REPAIRED":repaired})
                self.assertTrue("run_id" in child, child.get("stderr", "child startup failed").replace(key or "no-api-key", "<redacted>"))
                child_id = child["run_id"]
                self.assertEqual(self.root_command("interrupt", coordinator).returncode, 0)
                self.wait(process)
                self.finish(child_id)
                # Wait on the real owner lock, not an acknowledgement or polling loop.

                with concurrent.futures.ThreadPoolExecutor(1) as pool:
                    stopped = pool.submit(wait_owner, self.root / "logs" / child_id / "run.lock")
                    try:
                        stopped.result(timeout=12)
                    except concurrent.futures.TimeoutError:
                        self.root_command("interrupt", "-r", coordinator)
                        stopped.result(timeout=5)
                        self.fail("child repair did not settle after parent stop")
                state = self.manifest(child_id)
                self.assertEqual(state["status"], "completed", state.get("error"))
                self.assertEqual(state["repairs"], 1)
                process2, _ = self.start(resume=coordinator, allow=False)
                self.finish(coordinator)
                self.assertEqual(self.wait(process2)[0], 0)
                if key:
                    self.no_credential_artifacts(key)

    def test_delegated_auth_cannot_be_redirected_by_worker_backend_environment(self):
        key = self.api_key()
        self.env.update(OPENAI_BASE_URL="https://selected.invalid/fake", TEAM_EXPECT_BACKEND_URL="https://selected.invalid/fake")
        process, coordinator = self.start()
        hijack = self.root / "other-backend"
        hijack.mkdir()
        program = hijack / "codex"
        program.write_text("#!/bin/sh\nexit 77\n")
        program.chmod(0o700)
        result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x", env={
            "PATH":str(hijack) + os.pathsep + self.env["PATH"], "OPENAI_BASE_URL":"https://different.invalid/not-selected"})
        self.assertEqual(result["returncode"],0,result["stderr"].replace(key,"<redacted>"))
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0],0)
        self.no_credential_artifacts(key)

    def test_authentication_endpoint_errors_fail_closed_before_backend_start(self):
        key = self.api_key()
        process, coordinator = self.start()
        path = self.root / "logs" / coordinator / "control.json"
        original = path.read_text()
        for fault in ("closed", "stale"):
            with self.subTest(fault=fault):
                control = json.loads(original)
                if fault == "stale":
                    control["authentication"]["revision"] += 1
                else:
                    missing = self.root / "missing-endpoint"
                    missing.mkdir(exist_ok=True)
                    control["authentication"]["endpoint"] = {"kind":"relative", "root":str(missing), "name":"credential.sock"}
                path.write_text(json.dumps(control))
                result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")
                self.assertNotEqual(result["returncode"], 0)
                self.assertIn("authentication", result["stderr"])
                state = json.loads(self.root_command("read", coordinator, "manifest.json").stdout)
                self.assertFalse(any(run["active"] for run in state["descendants"]["runs"]))
        path.write_text(original)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)
        self.no_credential_artifacts(key)

    def test_authentication_server_peer_cannot_be_substituted(self):
        key = self.api_key()
        process, coordinator = self.start()
        path = self.root / "logs" / coordinator / "control.json"
        original = path.read_text()
        if os.name == "nt":
            from multiprocessing.connection import Listener
            address = r"\\.\pipe\team-foreign-auth-" + coordinator
            with Listener(address, family="AF_PIPE") as listener:
                control = json.loads(original)
                control["authentication"]["endpoint"] = {"kind":"pipe", "name":address}
                path.write_text(json.dumps(control))
                def accept_foreign():
                    with listener.accept() as connection:
                        try:
                            connection.recv_bytes()
                        except EOFError:
                            return
                        self.fail("authentication metadata leaked to substituted server")
                with concurrent.futures.ThreadPoolExecutor(1) as pool:
                    accepted = pool.submit(accept_foreign)
                    result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")
                    accepted.result(timeout=8)
                self.assertNotEqual(result["returncode"], 0)
                self.assertIn("retained coordinator launcher", result["stderr"])
        else:
            fake = self.sockets / "credential.sock"
            with socket.socket(socket.AF_UNIX) as listener:
                listener.settimeout(5)
                listener.bind(str(fake))
                listener.listen()
                control = json.loads(original)
                endpoint = {"kind":"relative", "root":str(self.sockets), "name":"credential.sock"}
                if __import__("sys").platform == "linux":
                    # Linux production transport uses abstract sockets.
                    listener.close()
                    listener = socket.socket(socket.AF_UNIX)
                    listener.settimeout(5)
                    address = "team-foreign-auth-" + coordinator
                    listener.bind("\0" + address)
                    listener.listen()
                    endpoint = {"kind":"abstract", "name":address}
                control["authentication"]["endpoint"] = endpoint
                path.write_text(json.dumps(control))
                try:
                    result = self.command(coordinator, "run", "--task", "ordinary", "--completion-gate", "x")
                    self.assertNotEqual(result["returncode"], 0)
                    self.assertIn("retained coordinator launcher", result["stderr"])
                    connection, _ = listener.accept()
                    with connection:
                        self.assertEqual(connection.recv(1), b"")
                finally:
                    listener.close()
        path.write_text(original)
        self.finish(coordinator)
        self.assertEqual(self.wait(process)[0], 0)
        self.no_credential_artifacts(key)


if __name__ == "__main__":
    unittest.main()
