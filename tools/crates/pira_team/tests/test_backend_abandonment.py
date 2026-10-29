"""Capacity is a trigger; separately exercise output loss and real exec teardown.

PIRA_TEAM_BIN must name a workspace build. PIRA_TEAM_REAL_CODEX optionally enables
the installed app-server check, with scripted loopback Responses, never inference.
"""
import gzip
import json
import os
from pathlib import Path
import shlex
import sys
import threading
import unittest

import test_subteams
from native_fixture import Boundary, process_exit_waiter, wait_owner


@unittest.skipUnless(os.name == "posix", "POSIX process-group and broken-pipe checks")
class BackendAbandonmentTests(unittest.TestCase):
    def setUp(self):
        self.fixture = test_subteams.SubteamTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def assert_active_child(self, coordinator, child):
        f = self.fixture
        identity = f.request(child["run_id"], operation="identity")
        self.assertEqual(identity["ppid"], child["pid"], identity)
        self.assertEqual(os.getpgid(child["pid"]), child["pid"])
        self.assertEqual(f.manifest(child["run_id"])["status"], "running")
        tree = json.loads((f.root / "logs" / coordinator / "tree.json").read_text())
        self.assertTrue(tree["members"][child["run_id"]]["active"])

    def assert_completed_child(self, coordinator, child):
        f = self.fixture
        run = f.root / "logs" / child["run_id"]
        state = f.manifest(child["run_id"])
        self.assertEqual(state["status"], "completed")
        self.assertEqual(Path(state["result"]).read_text(), "ANSWER subteam-service")
        snapshot = json.loads((run / "revisions/000001/manifest.json").read_text())
        self.assertEqual(snapshot["status"], "completed")
        tree = json.loads((f.root / "logs" / coordinator / "tree.json").read_text())
        self.assertFalse(tree["members"][child["run_id"]]["active"])

    def test_capacity_terminal_alone_leaves_isolated_child_running(self):
        f = self.fixture
        process, coordinator = f.start()
        child = f.child(coordinator)
        self.assert_active_child(coordinator, child)
        exited = process_exit_waiter(child["pid"])
        f.request(coordinator, operation="capacity_failure")
        code, _, _ = f.wait(process)
        self.assertNotEqual(code, 0)
        self.assertEqual(f.manifest(coordinator)["status"], "failed")
        self.assertIn("serverOverloaded", f.manifest(coordinator)["error"])
        self.assert_active_child(coordinator, child)
        # The dead fake coordinator also dropped BOTH pipe readers. A real
        # active child still answers and can publish its final state afterward.
        f.finish(child["run_id"])
        exited()
        wait_owner(f.root / "logs" / child["run_id"] / "run.lock")
        self.assert_completed_child(coordinator, child)

    def test_active_child_survives_abrupt_loss_of_both_output_readers(self):
        f = self.fixture
        process, coordinator = f.start()
        child = f.child(coordinator)
        result = f.command(coordinator, operation="close_output", handle=child["handle"],
                           streams=["stdout", "stderr"])
        self.assertIsNone(result["returncode"])
        self.assert_active_child(coordinator, child)
        f.finish(child["run_id"])
        f.command(coordinator, operation="wait", handle=child["handle"])
        self.assert_completed_child(coordinator, child)
        f.finish(coordinator)
        self.assertEqual(f.wait(process)[0], 0)

    def test_completed_work_remains_durable_after_broken_stdout_receipt(self):
        f = self.fixture
        process, coordinator = f.start()
        child = f.child(coordinator)
        f.command(coordinator, operation="close_output", handle=child["handle"], streams=["stdout"])
        f.finish(child["run_id"])
        result = f.command(coordinator, operation="wait", handle=child["handle"])
        self.assert_completed_child(coordinator, child)
        f.finish(coordinator)
        self.assertEqual(f.wait(process)[0], 0)
        # Receipt delivery can fail AFTER work and revision settlement succeed.
        print("broken-stdout-receipt " + json.dumps(result), flush=True)

    def test_completed_work_remains_durable_after_broken_stderr_warning(self):
        f = self.fixture
        process, coordinator = f.start()
        child = f.child(coordinator, env={"TEAM_NO_USAGE": "1"})
        f.command(coordinator, operation="close_output", handle=child["handle"], streams=["stderr"])
        f.finish(child["run_id"])
        result = f.command(coordinator, operation="wait", handle=child["handle"])
        state = f.manifest(child["run_id"])
        self.assert_completed_child(coordinator, child)
        self.assertFalse(state["usage_complete"])
        f.finish(coordinator)
        self.assertEqual(f.wait(process)[0], 0)
        print("broken-stderr-warning " + json.dumps(result), flush=True)

    @unittest.skipUnless(os.environ.get("PIRA_TEAM_REAL_CODEX"), "set PIRA_TEAM_REAL_CODEX for installed offline app-server")
    def test_real_exec_shutdown_after_capacity_terminal(self):
        f = self.fixture
        boundary = Boundary()
        self.addCleanup(boundary.close)
        real_exit = f.root / "real-backend-exit.json"
        # Native homes and shell startup files must never use the real user home.
        user = f.root / "user"
        user.mkdir()
        f.env.update(HOME=str(user), USERPROFILE=str(user), ZDOTDIR=str(user),
                     XDG_CONFIG_HOME=str(user), XDG_DATA_HOME=str(user), XDG_CACHE_HOME=str(user),
                     TEAM_REAL_COORDINATOR_CODEX=str(Path(os.environ["PIRA_TEAM_REAL_CODEX"]).resolve()),
                     TEAM_PAUSE_AT="capacity-terminal", TEAM_REAL_COORDINATOR_EXIT=str(real_exit),
                     **boundary.environment)
        f.parent([{"sandbox_policy": {"type": "danger-full-access"}}])
        with CapacityResponses(str(f.bin)) as provider:
            f.env["TEAM_LOCAL_RESPONSES_URL"] = provider.url
            process, coordinator = f.start()
            self.assertTrue(provider.exec_returned.wait(15), provider.errors)
            children = [p.parent for p in (f.root / "logs").glob("*/manifest.json")
                        if json.loads(p.read_text()).get("parent_run_id") == coordinator]
            self.assertEqual(len(children), 1, {"errors": provider.errors, "tool_outputs": provider.tool_outputs})
            child_id = children[0].name
            tree = json.loads((f.root / "logs" / coordinator / "tree.json").read_text())
            child = {"run_id": child_id, "pid": tree["members"][child_id]["launcher_pid"]}
            self.assert_active_child(coordinator, child)
            provider.release_capacity.set()
            boundary.reached()
            # The real terminal exists, but Team has NOT received it. Thus
            # Team's Server::shutdown / Scope::terminate cannot explain a death here.
            self.assert_active_child(coordinator, child)
            runtime = json.loads((f.root / "logs" / coordinator / "runtime.json").read_text())
            child_runtime = json.loads((children[0] / "runtime.json").read_text())
            scope_group = os.getpgid(runtime["backend_pid"])
            self.assertEqual(scope_group, runtime["backend_pid"])
            self.assertNotEqual(os.getpgid(child["pid"]), scope_group)
            exited = process_exit_waiter(child["pid"])
            backend_exited = process_exit_waiter(child_runtime["backend_pid"])
            boundary.release()
            code, _, stderr = f.wait(process)
            self.assertNotEqual(code, 0, stderr)
            self.assertEqual(f.manifest(coordinator)["status"], "failed")
            self.assertTrue(real_exit.exists(), "real backend did not exit before Team scope watchdog")
            self.assertEqual(json.loads(real_exit.read_text())["returncode"], 0)
            exited()
            backend_exited()
            wait_owner(children[0] / "run.lock")
            state = f.manifest(child_id)
            retained = json.loads((f.root / "logs" / coordinator / "tree.json").read_text())
            print("real-backend-abandonment " + json.dumps({
                "coordinator": f.manifest(coordinator)["status"], "child": state["status"],
                "child_active": retained["members"][child_id]["active"],
                "real_backend_exit": json.loads(real_exit.read_text())["returncode"],
                "provider_requests": provider.requests,
                "terminal_error": f.manifest(coordinator)["error"],
                "child_group": child["pid"], "team_scope_group": scope_group,
            }), flush=True)
            self.assertEqual(provider.errors, [])
            # Exact hosted serverOverloaded is covered by the fake terminal
            # above. Native HTTP 503 has a different subtype, but follows the
            # same failed-terminal -> backend-shutdown path in Team.
            self.assertEqual(state["status"], "running")
            self.assertTrue(retained["members"][child_id]["active"])
            error = json.loads(f.manifest(coordinator)["error"])
            self.assertEqual(error["codexErrorInfo"], {"httpConnectionFailed": {"httpStatusCode": 503}})
            recovery = f.root_command("interrupt", "-r", coordinator)
            self.assertEqual(recovery.returncode, 0, recovery.stderr)
            self.assertEqual(f.manifest(child_id)["status"], "interrupted")
            settled = json.loads((f.root / "logs" / coordinator / "tree.json").read_text())
            self.assertFalse(settled["members"][child_id]["active"])
            self.assertEqual(json.loads((children[0] / "revisions/000001/manifest.json").read_text())["status"], "interrupted")


class CapacityResponses:
    """Reuse the existing credential-free loopback server with scripted tool SSE."""
    def __init__(self, binary):
        self.binary = binary
        self.exec_returned = threading.Event()
        self.release_capacity = threading.Event()
        self.tool_outputs = []

    def __enter__(self):
        scripts = str(Path(__file__).resolve().parents[4] / "assets/scripts")
        sys.path.insert(0, scripts)
        try:
            from team_relocation_fixture import LocalResponses
        finally:
            sys.path.remove(scripts)
        self.provider = LocalResponses().__enter__()
        self.url = self.provider.url
        self.errors = self.provider.errors
        owner = self

        class Handler(self.provider.server.RequestHandlerClass):
            def do_POST(self):
                try:
                    assert self.path == "/v1/responses" and not self.headers.get("Authorization")
                    size = int(self.headers.get("Content-Length", "0"))
                    assert 0 < size <= 4 * 1024 * 1024
                    data = self.rfile.read(size)
                    if self.headers.get("Content-Encoding") == "gzip":
                        data = gzip.decompress(data)
                    else:
                        assert self.headers.get("Content-Encoding", "identity") == "identity"
                    body = json.loads(data)
                    assert body["model"] == "test-model" and body["stream"] is True
                    owner.provider.requests += 1
                    if owner.provider.requests == 1:
                        names = [tool.get("name") for tool in body["tools"]]
                        assert "exec_command" in names, names
                        command = shlex.join([owner.binary, "run", "--task", "subteam-service",
                                              "--completion-gate", "Finish child work"])
                        item = {"id": "fc_fixture", "type": "function_call", "call_id": "call_fixture",
                                "name": "exec_command", "arguments": json.dumps({
                                    "cmd": command, "yield_time_ms": 1000,
                                    "max_output_tokens": 256, "login": False, "tty": False})}
                        response = {"id": "resp_fixture", "object": "response", "created_at": 1,
                                    "status": "completed", "model": "test-model", "output": [item],
                                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
                        events = [("response.created", {"response": dict(response, status="in_progress", output=[])}),
                                  ("response.output_item.added", {"output_index": 0, "item": dict(item, arguments="")}),
                                  ("response.function_call_arguments.delta", {"item_id": item["id"], "output_index": 0, "delta": item["arguments"]}),
                                  ("response.function_call_arguments.done", {"item_id": item["id"], "output_index": 0, "arguments": item["arguments"]}),
                                  ("response.output_item.done", {"output_index": 0, "item": item}),
                                  ("response.completed", {"response": response})]
                    else:
                        owner.tool_outputs = [item for item in body.get("input", [])
                                              if item.get("type") == "function_call_output"]
                        owner.exec_returned.set()
                        assert owner.release_capacity.wait(20), "test did not release capacity error"
                        payload = json.dumps({"error": {
                            "code": "server_overloaded", "type": "server_error",
                            "message": "Selected model is at capacity. Please try a different model."}}).encode()
                        self.send_response(503)
                        self.send_header("Content-Type", "application/json")
                        self.send_header("Content-Length", str(len(payload)))
                        self.end_headers()
                        self.wfile.write(payload)
                        return
                    payload = "".join("event: " + name + "\ndata: " + json.dumps(
                        dict(event, type=name, sequence_number=i)) + "\n\n"
                        for i, (name, event) in enumerate(events)).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except Exception as error:
                    owner.errors.append(repr(error))
                    self.send_error(400)

        self.provider.server.RequestHandlerClass = Handler
        return self

    @property
    def requests(self):
        return self.provider.requests

    def __exit__(self, *args):
        self.release_capacity.set()
        self.provider.__exit__(*args)


def load_tests(loader, tests, pattern):
    # Native CI requires zero skips. Keep all fake checks in default discovery;
    # opt in to the installed binary rather than discover an optional skip.
    if os.environ.get("PIRA_TEAM_REAL_CODEX"):
        return tests
    return unittest.TestSuite(BackendAbandonmentTests(name)
                              for name in loader.getTestCaseNames(BackendAbandonmentTests)
                              if name != "test_real_exec_shutdown_after_capacity_terminal")


if __name__ == "__main__":
    unittest.main()
