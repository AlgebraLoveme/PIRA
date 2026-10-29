"""Guarded ownerless settlement against isolated fake-protocol Team attempts."""
import concurrent.futures
import json
import os
import socket
import subprocess
import threading
import unittest

import test_subteams
from native_fixture import Boundary, process_exit_waiter


class OrphanRecoveryTests(unittest.TestCase):
    # Reuse the service fixture without inheriting its unrelated test cases.
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
    child = test_subteams.SubteamTests.child
    manifest = test_subteams.SubteamTests.manifest
    finish = test_subteams.SubteamTests.finish
    wait = test_subteams.SubteamTests.wait

    def run_path(self, run):
        return self.root / "logs" / run

    def abandon(self, process, run):
        runtime = json.loads((self.run_path(run) / "runtime.json").read_text())
        exited = process_exit_waiter(runtime["backend_pid"])
        process.kill()  # Fixture-owned launcher handle, never a production PID.
        self.wait(process)
        exited()
        self.assertEqual(self.manifest(run)["status"], "running")
        return runtime

    def test_target_only_abandoned_owner_retains_artifacts_and_explicit_resume(self):
        process, run = self.start(allow=False)
        before = self.manifest(run)
        diagnostic = self.run_path(run) / "stderr.log"
        evidence = diagnostic.read_bytes()
        self.abandon(process, run)
        abandoned = (self.run_path(run) / "manifest.json").read_bytes()
        refused = self.root_command("resume", run)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("refusing automatic replay", refused.stderr)
        self.assertEqual((self.run_path(run) / "manifest.json").read_bytes(), abandoned)
        result = self.root_command("interrupt", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["stopped"])
        recovered = self.manifest(run)
        self.assertEqual(recovered["status"], "interrupted")
        self.assertFalse(recovered["usage_complete"])
        self.assertIsNone(recovered["active_turn"])
        for key in ("usage", "revision_usage", "thread_id", "creator_main_session_id", "parent_run_id"):
            self.assertEqual(recovered[key], before[key])
        self.assertTrue(recovered["revisions"][-1]["recovered"])
        snapshot = self.run_path(run) / "revisions/000001/manifest.json"
        retained = snapshot.read_bytes()
        self.assertEqual(json.loads(retained), recovered)
        self.assertTrue(diagnostic.read_bytes().startswith(evidence))
        self.assertEqual(self.root_command("read", run, "manifest.json").returncode, 0)
        self.assertEqual(self.root_command("path", run, ".").returncode, 0)
        process2, _ = self.start(resume=run, allow=False)
        self.assertEqual(self.manifest(run)["revision"], 2)
        self.assertEqual(self.manifest(run)["thread_id"], before["thread_id"])
        self.finish(run)
        self.assertEqual(self.wait(process2)[0], 0)
        self.assertEqual(snapshot.read_bytes(), retained)
        self.assertFalse(self.manifest(run)["usage_complete"])
        self.assertEqual([r["revision"] for r in self.manifest(run)["revisions"]], [1, 2])

    def test_missing_control_does_not_authorize_live_owner_settlement(self):
        process, run = self.start(allow=False)
        endpoint = self.run_path(run) / "control.json"
        saved = endpoint.read_bytes()
        endpoint.unlink()
        before = (self.run_path(run) / "manifest.json").read_bytes()
        result = self.root_command("interrupt", run)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.run_path(run) / "manifest.json").read_bytes(), before)
        self.assertIsNone(process.poll())
        endpoint.write_bytes(saved)
        self.finish(run)
        self.assertEqual(self.wait(process)[0], 0)

    @unittest.skipUnless(os.name == "posix", "requires replaceable Unix lock inode")
    def test_live_launcher_refused_even_if_owner_lock_is_unexpectedly_free(self):
        process, run = self.start(allow=False)
        path = self.run_path(run)
        endpoint = path / "control.json"
        saved = endpoint.read_bytes()
        endpoint.unlink()
        # Model an unexpectedly free lock without releasing the launcher's handle.
        original = path / "retained-run.lock"
        (path / "run.lock").rename(original)
        (path / "run.lock").touch(mode=0o600)
        before = (path / "manifest.json").read_bytes()
        try:
            result = self.root_command("interrupt", run)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("launcher", result.stderr)
            self.assertEqual((path / "manifest.json").read_bytes(), before)
            self.assertIsNone(process.poll())
        finally:
            (path / "run.lock").unlink()
            original.rename(path / "run.lock")
            endpoint.write_bytes(saved)
        self.finish(run)
        self.assertEqual(self.wait(process)[0], 0)

    def test_missing_or_mismatched_identity_evidence_fails_closed(self):
        process, run = self.start(allow=False)
        self.abandon(process, run)
        path = self.run_path(run)
        runtime_path = path / "runtime.json"
        tree_path = path / "tree.json"
        runtime = json.loads(runtime_path.read_text())
        tree = json.loads(tree_path.read_text())
        original = (path / "manifest.json").read_bytes()
        cases = [
            ("runtime", "backend_pid", None, "backend PID"),
            ("runtime", "backend_birth", None, "backend birth"),
            ("runtime", "launcher_pid", None, "launcher PID"),
            ("runtime", "launcher_birth", None, "launcher birth"),
            ("runtime", "revision", 2, "runtime revision"),
            ("member", "revision", 2, "subtree revision"),
            ("member", "launcher_birth", runtime["launcher_birth"] + 1, "subtree launcher identity"),
        ]
        if os.name == "nt":
            cases.append(("runtime", "backend_job_session", None, "backend job OS session"))
        for artifact, field, value, message in cases:
            with self.subTest(artifact=artifact, field=field):
                changed_runtime = dict(runtime)
                changed_tree = json.loads(json.dumps(tree))
                target = changed_runtime if artifact == "runtime" else changed_tree["members"][run]
                if value is None:
                    target.pop(field)
                else:
                    target[field] = value
                runtime_path.write_text(json.dumps(changed_runtime))
                tree_path.write_text(json.dumps(changed_tree))
                before_tree = tree_path.read_bytes()
                result = self.root_command("interrupt", run)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertEqual((path / "manifest.json").read_bytes(), original)
                self.assertEqual(tree_path.read_bytes(), before_tree)
                self.assertFalse((path / "revisions/000001/manifest.json").exists())
        tree_path.write_text(json.dumps(tree))
        runtime_path.unlink()
        result = self.root_command("interrupt", run)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((path / "manifest.json").read_bytes(), original)
        runtime_path.write_text(json.dumps(runtime))
        self.assertEqual(self.root_command("interrupt", run).returncode, 0)

    def test_live_backend_and_reused_scope_refused_without_signaling(self):
        process, run = self.start(allow=False)
        self.abandon(process, run)
        other_process, other = self.start(allow=False)
        live = json.loads((self.run_path(other) / "runtime.json").read_text())
        path = self.run_path(run)
        runtime_path = path / "runtime.json"
        runtime = json.loads(runtime_path.read_text())
        runtime.update(backend_pid=live["backend_pid"], backend_birth=live["backend_birth"],
                       backend_job=live["backend_job"], backend_job_session=live["backend_job_session"])
        runtime_path.write_text(json.dumps(runtime))
        original = (path / "manifest.json").read_bytes()
        result = self.root_command("interrupt", run)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("backend stop unconfirmed", result.stderr)
        self.assertEqual((path / "manifest.json").read_bytes(), original)
        # Deterministic PID-reuse evidence: the live PID belongs to a different birth.
        runtime.update(backend_birth=live["backend_birth"] + 1,
                       launcher_pid=live["launcher_pid"], launcher_birth=live["launcher_birth"] + 1)
        runtime_path.write_text(json.dumps(runtime))
        tree_path = path / "tree.json"
        tree = json.loads(tree_path.read_text())
        tree["members"][run].update(launcher_pid=runtime["launcher_pid"], launcher_birth=runtime["launcher_birth"])
        tree_path.write_text(json.dumps(tree))
        result = self.root_command("interrupt", run)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("scope", result.stderr)
        self.assertEqual((path / "manifest.json").read_bytes(), original)
        self.assertIsNone(other_process.poll())
        self.finish(other)
        self.assertEqual(self.wait(other_process)[0], 0)

    def test_existing_snapshot_republished_without_overwrite_or_duplicate_history(self):
        process, run = self.start(allow=False)
        self.abandon(process, run)
        path = self.run_path(run)
        running = (path / "manifest.json").read_bytes()
        self.assertEqual(self.root_command("interrupt", run).returncode, 0)
        snapshot = path / "revisions/000001/manifest.json"
        retained = snapshot.read_bytes()
        # Crash window: snapshot saved, latest manifest publication did not finish.
        (path / "manifest.json").write_bytes(running)
        result = self.root_command("interrupt", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(snapshot.read_bytes(), retained)
        self.assertEqual(json.loads(retained), self.manifest(run))
        self.assertEqual(len(self.manifest(run)["revisions"]), 1)
        # A present but wrongly bound snapshot must never be replaced or published.
        (path / "manifest.json").write_bytes(running)
        invalid = json.loads(retained)
        invalid["parent_run_id"] = "another-parent"
        snapshot.write_text(json.dumps(invalid))
        before = snapshot.read_bytes()
        result = self.root_command("interrupt", run)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid crashed revision snapshot", result.stderr)
        self.assertEqual(snapshot.read_bytes(), before)
        self.assertEqual((path / "manifest.json").read_bytes(), running)
        snapshot.write_bytes(retained)
        self.assertEqual(self.root_command("interrupt", run).returncode, 0)

    def test_stale_control_failure_cannot_settle_a_resumed_revision(self):
        process, run = self.start(allow=False)
        self.abandon(process, run)
        endpoint_path = self.run_path(run) / "control.json"
        original = endpoint_path.read_bytes()
        endpoint = json.loads(original)
        with socket.socket() as server, concurrent.futures.ThreadPoolExecutor(1) as pool:
            server.bind(("127.0.0.1", 0))
            server.listen()
            server.settimeout(8)
            host, port = server.getsockname()
            endpoint["address"] = f"{host}:{port}"
            endpoint_path.write_text(json.dumps(endpoint))
            pending = pool.submit(self.root_command, "interrupt", run)
            connection, _ = server.accept()
            with connection, connection.makefile("rwb") as stream:
                connection.settimeout(8)
                self.assertEqual(json.loads(stream.readline())["operation"], "interrupt")
                endpoint_path.write_bytes(original)
                self.assertEqual(self.root_command("interrupt", run).returncode, 0)
                resumed = self.root_command("resume", run, "--task", "ordinary", "--completion-gate", "Finish")
                self.assertEqual(resumed.returncode, 0, resumed.stderr)
                current = (self.run_path(run) / "manifest.json").read_bytes()
                self.assertEqual(json.loads(current)["revision"], 2)
                stream.write(b'{"status":"rejected","error":"fixture delayed control failure"}\n')
                stream.flush()
            refused = pending.result(timeout=8)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("attempt changed", refused.stderr)
        self.assertEqual((self.run_path(run) / "manifest.json").read_bytes(), current)
        self.assertEqual([r["revision"] for r in self.manifest(run)["revisions"]], [1, 2])

    def test_concurrent_recovery_and_resume_preserve_single_attempt_admission(self):
        process, run = self.start(allow=False)
        self.abandon(process, run)
        barrier = threading.Barrier(3)
        def invoke(*args):
            barrier.wait(timeout=8)
            return self.root_command(*args)
        with concurrent.futures.ThreadPoolExecutor(3) as pool:
            futures = [pool.submit(invoke, "interrupt", run) for _ in range(2)]
            futures.append(pool.submit(invoke, "resume", run, "--task", "ordinary", "--completion-gate", "Finish"))
            results = [future.result(timeout=20) for future in futures]
        self.assertTrue(any(result.returncode == 0 for result in results[:2]))
        if results[2].returncode:
            resumed = self.root_command("resume", run, "--task", "ordinary", "--completion-gate", "Finish")
            self.assertEqual(resumed.returncode, 0, resumed.stderr)
        self.assertEqual(self.manifest(run)["revision"], 2)
        self.assertEqual([r["revision"] for r in self.manifest(run)["revisions"]], [1, 2])
        self.assertFalse(self.manifest(run)["usage_complete"])
        # Hold the winner inside backend preflight until the losing launcher exits.
        boundary = Boundary()
        self.addCleanup(boundary.close)
        self.env.update(TEAM_PAUSE_AT="schema", **boundary.environment)
        barrier = threading.Barrier(2)
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            futures = [pool.submit(invoke, "resume", run, "--task", "ordinary", "--completion-gate", "Finish") for _ in range(2)]
            try:
                boundary.reached()
                done, pending = concurrent.futures.wait(futures, timeout=8, return_when=concurrent.futures.FIRST_COMPLETED)
                self.assertEqual(len(done), 1)
                loser = next(iter(done)).result()
                self.assertNotEqual(loser.returncode, 0)
                self.assertIn("active owner", loser.stderr)
                self.assertEqual(len(pending), 1)
            finally:
                boundary.release()
            winner = next(iter(pending)).result(timeout=15)
        self.assertEqual(winner.returncode, 0, winner.stderr)
        self.assertEqual(self.manifest(run)["revision"], 3)
        self.assertEqual([r["revision"] for r in self.manifest(run)["revisions"]], [1, 2, 3])

    @unittest.skipUnless(os.name == "posix", "requires POSIX backend process group")
    def test_surviving_owned_group_refuses_settlement_and_keeps_recursive_gate(self):
        boundary = Boundary()
        self.addCleanup(boundary.close)
        helper = (
            "import json,os,socket; "
            "connection=socket.create_connection(tuple(json.loads(os.environ['TEAM_BOUNDARY_ENDPOINT']))); "
            "connection.sendall(b'1'+str(os.getpid()).encode()+b'\\n'); "
            "assert connection.recv(1)==b'1'"
        )
        fake = self.root / "codex"
        source = fake.read_text()
        hook = ("args = sys.argv[1:]\n"
                "if os.environ.get('TEAM_ORPHAN_SCOPE_HELPER') and args[:3] == ['app-server','--strict-config','--stdio']:\n"
                "    import subprocess\n"
                f"    subprocess.Popen([sys.executable, '-c', {helper!r}], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)\n")
        self.assertIn("args = sys.argv[1:]\n", source)
        fake.write_text(source.replace("args = sys.argv[1:]\n", hook, 1))
        self.env.update(TEAM_ORPHAN_SCOPE_HELPER="1", **boundary.environment)
        process, run = self.start(allow=False)
        boundary.reached()
        with boundary.connection.makefile("rb") as stream:
            helper_pid = int(stream.readline())
        exited = process_exit_waiter(helper_pid)
        try:
            runtime = self.abandon(process, run)
            self.assertEqual(os.getpgid(helper_pid), runtime["backend_pid"])
            path = self.run_path(run)
            original = (path / "manifest.json").read_bytes()
            refused = self.root_command("interrupt", run)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("scope is not empty", refused.stderr)
            recursive = self.root_command("interrupt", "-r", run)
            self.assertNotEqual(recursive.returncode, 0)
            receipt = json.loads(recursive.stderr.removeprefix("pira_team: ").strip())
            self.assertEqual(receipt["status"], "incomplete")
            self.assertFalse(receipt["results"][0]["stopped"])
            self.assertEqual((path / "manifest.json").read_bytes(), original)
            self.assertFalse((path / "revisions/000001/manifest.json").exists())
            tree = json.loads((path / "tree.json").read_text())
            self.assertTrue(tree["blocked"])
            self.assertTrue(tree["members"][run]["active"])
            self.assertNotEqual(self.root_command("resume", run).returncode, 0)
        finally:
            boundary.release()
            exited()

    def test_successful_preflight_stops_schema_helper_before_backend_operation(self):
        schema = Boundary()
        helper = Boundary()
        self.addCleanup(schema.close)
        self.addCleanup(helper.close)
        helper_code = (
            "import json,os,socket; "
            "connection=socket.create_connection(tuple(json.loads(os.environ['TEAM_SCHEMA_HELPER_ENDPOINT']))); "
            "connection.sendall(b'1'+str(os.getpid()).encode()+b'\\n'); "
            "assert connection.recv(1)==b'1'"
        )
        fake = self.root / "codex"
        source = fake.read_text()
        hook = ("args = sys.argv[1:]\n"
                "if args[:2] == ['app-server','generate-json-schema']:\n"
                "    import subprocess\n"
                f"    subprocess.Popen([sys.executable, '-c', {helper_code!r}], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)\n")
        self.assertIn("args = sys.argv[1:]\n", source)
        fake.write_text(source.replace("args = sys.argv[1:]\n", hook, 1))
        self.env.update(TEAM_PAUSE_AT="schema", **schema.environment,
                        TEAM_SCHEMA_HELPER_ENDPOINT=json.dumps(helper.listener.getsockname()))
        with concurrent.futures.ThreadPoolExecutor(1) as pool:
            started = pool.submit(self.start, allow=False)
            schema.reached()
            helper.reached()
            with helper.connection.makefile("rb") as stream:
                helper_pid = int(stream.readline())
            # Register before schema exit, so cleanup cannot race waiter setup.
            exited = process_exit_waiter(helper_pid)
            if os.name == "posix":
                schema_group = os.getpgid(helper_pid)
            try:
                schema.release()
                exited()
            finally:
                # Old implementations leave the helper alive; release it on failure.
                try:
                    helper.release()
                except OSError:
                    pass  # Successful scope cleanup has closed this fixture socket.
            process, run = started.result(timeout=8)
        runtime = json.loads((self.run_path(run) / "runtime.json").read_text())
        if os.name == "posix":
            self.assertNotEqual(schema_group, runtime["backend_pid"])
        self.abandon(process, run)
        recovered = self.root_command("interrupt", run)
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assertEqual(self.manifest(run)["status"], "interrupted")
        self.assertFalse(self.manifest(run)["usage_complete"])

    def report_boundary_pid(self):
        fake = self.root / "codex"
        source = fake.read_text()
        self.assertIn('connection.sendall(b"1")', source)
        fake.write_text(source.replace('connection.sendall(b"1")',
                        'connection.sendall(b"1" + str(os.getpid()).encode() + b"\\n")', 1))

    def test_resume_preflight_missing_or_stale_revision_runtime_refused(self):
        first = self.launch("ordinary")
        self.assertEqual(first.returncode, 0, first.stderr)
        run = json.loads(first.stdout)["run_id"]
        path = self.run_path(run)
        previous = (path / "runtime.json").read_bytes()
        boundary = Boundary()
        self.addCleanup(boundary.close)
        self.report_boundary_pid()
        self.env.update(TEAM_PAUSE_AT="schema", **boundary.environment)
        process = subprocess.Popen([str(self.bin), "resume", run, "--task", "ordinary",
                                    "--completion-gate", "Finish", "--store", str(self.root / "logs")],
                                   env=self.env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.processes.append((process, run))
        self.wait_stderr_notice(process, "pira_team run_id:")
        boundary.reached()
        with boundary.connection.makefile("rb") as stream:
            schema_pid = int(stream.readline())
        exited = process_exit_waiter(schema_pid)
        try:
            self.assertFalse((path / "runtime.json").exists())
            self.assertEqual(self.manifest(run)["revision"], 2)
            process.kill()
            self.wait(process)
            original = (path / "manifest.json").read_bytes()
            result = self.root_command("interrupt", run)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("runtime.json", result.stderr)
            (path / "runtime.json").write_bytes(previous)
            result = self.root_command("interrupt", run)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("runtime revision", result.stderr)
            self.assertEqual((path / "manifest.json").read_bytes(), original)
            self.assertTrue(json.loads((path / "tree.json").read_text())["members"][run]["active"])
        finally:
            try:
                boundary.release()
            except OSError:
                if os.name != "nt":
                    raise
            exited()

    def test_same_revision_repair_preflight_rejects_previous_and_legacy_bindings(self):
        boundary = Boundary()
        self.addCleanup(boundary.close)
        self.report_boundary_pid()
        self.env.update(TEAM_PAUSE_AT="schema", TEAM_PAUSE_DIRECTORY="repair", TEAM_REPAIR_WAIT="1",
                        TEAM_CANDIDATE=json.dumps({"format":"json", "content":"{broken"}), **boundary.environment)
        process, run = self.start(allow=False, extra=("--format", "json"))
        path = self.run_path(run)
        previous = json.loads((path / "runtime.json").read_text())
        self.finish(run)
        boundary.reached()
        with boundary.connection.makefile("rb") as stream:
            schema_pid = int(stream.readline())
        exited = process_exit_waiter(schema_pid)
        try:
            self.assertFalse((path / "runtime.json").exists())
            self.assertEqual(len(self.manifest(run)["attempts"]), 2)
            process.kill()
            self.wait(process)
            original = (path / "manifest.json").read_bytes()
            missing = self.root_command("interrupt", run)
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn("runtime.json", missing.stderr)
            for legacy in (False, True):
                with self.subTest(legacy=legacy):
                    if legacy:
                        previous.pop("attempt_logs")
                    (path / "runtime.json").write_text(json.dumps(previous))
                    refused = self.root_command("interrupt", run)
                    self.assertNotEqual(refused.returncode, 0)
                    self.assertIn("current attempt", refused.stderr)
                    self.assertEqual((path / "manifest.json").read_bytes(), original)
                    self.assertTrue(json.loads((path / "tree.json").read_text())["members"][run]["active"])
        finally:
            try:
                boundary.release()
            except OSError:
                if os.name != "nt":
                    raise
            exited()

    def test_legacy_single_attempt_binding_remains_supported(self):
        process, run = self.start(allow=False)
        runtime = self.abandon(process, run)
        runtime.pop("attempt_logs")
        (self.run_path(run) / "runtime.json").write_text(json.dumps(runtime))
        result = self.root_command("interrupt", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.manifest(run)["status"], "interrupted")
        self.assertFalse(self.manifest(run)["usage_complete"])

    def test_abandoned_resume_preserves_prior_history_and_cumulative_usage(self):
        first = self.launch("ordinary")
        self.assertEqual(first.returncode, 0, first.stderr)
        run = json.loads(first.stdout)["run_id"]
        prior = self.manifest(run)
        snapshot = self.run_path(run) / "revisions/000001/manifest.json"
        retained = snapshot.read_bytes()
        process, _ = self.start(resume=run, allow=False)
        self.abandon(process, run)
        result = self.root_command("interrupt", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        current = self.manifest(run)
        self.assertEqual(current["usage"], prior["usage"])
        self.assertEqual(current["revisions"][0], prior["revisions"][0])
        self.assertEqual([record["revision"] for record in current["revisions"]], [1, 2])
        self.assertTrue(current["revisions"][-1]["recovered"])
        self.assertFalse(current["usage_complete"])
        self.assertEqual(snapshot.read_bytes(), retained)
        self.assertEqual(self.root_command("read", run, "manifest.json").returncode, 0)

    def test_target_only_orphan_leaves_children_until_recursive_gate_and_resume(self):
        process, run = self.start()
        child = self.child(run)["run_id"]
        self.abandon(process, run)
        result = self.root_command("interrupt", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["active_children_left_running"], [child])
        current = self.root_command("read", run, "manifest.json")
        self.assertEqual(current.returncode, 0, current.stderr)
        descendant = json.loads(current.stdout)["descendants"]
        self.assertTrue(descendant["runs"][0]["active"])
        self.assertFalse(descendant["usage_complete"])
        self.assertEqual(self.manifest(child)["status"], "running")
        stopped = self.root_command("interrupt", "-r", run)
        self.assertEqual(stopped.returncode, 0, stopped.stderr)
        tree = json.loads((self.run_path(run) / "tree.json").read_text())
        self.assertTrue(tree["blocked"])
        self.assertTrue(all(not member["active"] for member in tree["members"].values()))
        self.assertEqual(self.manifest(child)["status"], "interrupted")
        process2, _ = self.start(resume=run, allow=False)
        resumed = self.command(run, "resume", child, "--task", "ordinary", "--completion-gate", "Finish")
        self.assertEqual(resumed["returncode"], 0, resumed)
        self.finish(run)
        self.assertEqual(self.wait(process2)[0], 0)
        current = json.loads(self.root_command("read", run, "manifest.json").stdout)
        self.assertEqual(len(current["descendants"]["runs"]), 1)
        self.assertEqual(current["descendants"]["usage"], self.manifest(child)["usage"])
        self.assertFalse(current["descendants"]["usage_complete"])
        self.assertFalse(current["usage_complete"])


if __name__ == "__main__":
    unittest.main()
