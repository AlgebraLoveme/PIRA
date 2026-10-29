"""Linux signed endpoint + native server-local child fixtures; no real SSH/model.

Only this fixture's bounded workers/stores are controlled. This does not establish
genuine root caller identity, real host authentication, or disconnect durability.
"""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

import test_subteams
from test_remote_fixture import SignedEndpointFixture, wait_for_file


@unittest.skipUnless(sys.platform == "linux", "Linux signed supervisor integration")
class RemoteSubteamTests(unittest.TestCase):
    def setUp(self):
        self.f = SignedEndpointFixture(self, issuer="parent-session")
        self.f.store = self.f.root / "logs"
        self.f.endpoint_binding["store"] = str(self.f.store)
        keygen = self.f.keygen()
        self.assertEqual(keygen.returncode, 0, keygen.stderr)
        # Public fixture enrollment is deliberately separate from submission.
        self.f.enroll(self.f.issuer, self.f.root / "authority")
        temporary = tempfile.TemporaryDirectory(prefix="remote-tree-sock-", dir="/tmp")
        self.addCleanup(temporary.cleanup)
        self.sockets = Path(temporary.name)
        self.f.env.update(TEAM_REMOTE_TRANSPORT="proxy", TEAM_SOCKET_DIR=str(self.sockets),
                          TEAM_SUBTEAM_BIN=str(self.f.server),
                          TEAM_SUBTEAM_FIXTURE=str(Path(__file__).with_name("subteam_fixture.py")))
        self.tree = test_subteams.SubteamTests()
        self.tree.root, self.tree.bin, self.tree.env = self.f.root, self.f.server, self.f.env
        self.tree.sockets = self.sockets
        self.submissions = []
        self.addCleanup(self.stop_owned)

    def options(self, request="request-1"):
        return self.f.client_options(request, **{"remote-user":self.f.endpoint_binding["user"],
            "remote-exe":self.f.server, "remote-backend":self.f.endpoint_binding["backend"]})

    def invoke(self, operation, request="request-1", *extra):
        return self.f.invoke(operation, *self.options(request), *extra)

    def success(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def active(self, request):
        directory = self.f.store / ".remote-v2" / request
        wait_for_file(directory / "stderr.log", lambda p: p.exists() and "pira_team active:" in p.read_text())

    def settled(self, request):
        directory = self.f.store / ".remote-v2" / request
        wait_for_file(directory / "result.json")
        # Native blocking ownership waiter, after the supervisor's final record.
        from native_fixture import wait_owner
        wait_owner(directory / "supervisor.lock")

    def stop_owned(self):
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "proxy"
        for request in reversed(self.submissions):
            view = self.success(self.invoke("read", request))
            manifest = view["manifest"] or {}
            active_children = any(child["active"] for child in manifest.get("descendants", {}).get("runs", []))
            if manifest.get("status") in ("completed", "needs_decision", "incomplete", "interrupted", "failed") and not active_children:
                self.settled(request)
                continue
            result = self.invoke("interrupt", request, "--recursive", "--operation-id", "cleanup-"+request)
            if result.returncode:
                # Do not terminate by persisted PID or broaden cleanup authority.
                self.fail(f"owned remote fixture interruption failed: {result.stderr}")
            self.settled(request)

    def start(self, request="request-1", *, resume=None, allow=True):
        operation = "resume" if resume else "run"
        extra = ["--detach", "--task", "subteam-service", "--completion-gate", "Integrate fixture children"]
        if resume:
            extra += ["--run-request", resume]
        else:
            extra += ["--remote-cwd", str(self.f.cwd), "--model", "test-model", "--effort", "high"]
        if allow:
            extra += ["--allow-subteam"]
        receipt = self.success(self.invoke(operation, request, *extra))
        self.submissions.append(request)
        self.active(request)
        run = receipt["run_id"]
        wait_for_file(self.sockets / (run+".sock"))
        return run

    def manifest(self, request="request-1"):
        return self.success(self.invoke("read", request))["manifest"]

    def test_lost_submission_ack_preserves_one_execution_artifact_and_read_only_attach(self):
        request = "artifact-worker"
        task = "remote artifact λ"
        extra = ("--remote-cwd", str(self.f.cwd), "--detach", "--model", "test-model", "--effort", "high",
                 "--task", task, "--completion-gate", "Deliver fixture artifact")
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "proxy-drop-submit-once"
        lost = self.invoke("run", request, *extra)
        self.assertNotEqual(lost.returncode, 0)
        self.assertIn("unknown", lost.stderr.lower())
        self.submissions.append(request)
        accepted = json.loads((self.f.store / ".remote-v2" / request / "request.json").read_text())
        receipt = self.success(self.invoke("run", request, *extra))
        self.assertEqual(receipt["run_id"], Path(accepted["run"]).name)
        self.settled(request)
        run = self.f.store / receipt["run_id"]
        backend_state = run / "codex-home/fixture-state.json"
        before = backend_state.read_bytes()
        self.assertEqual(json.loads(before)["turns"], 1)
        self.assertEqual(json.loads(before)["history"], [task])
        view = self.success(self.invoke("read", request, "--artifact"))
        self.assertEqual(view["content"], "ANSWER " + task)
        self.assertEqual(view["manifest"]["status"], "completed")
        self.assertIs(view["manifest"]["usage_complete"], True)
        self.success(self.invoke("attach", request))
        self.assertEqual(backend_state.read_bytes(), before)
        self.assertEqual(len(list(self.f.store.glob("*/run.lock"))), 1)

    def test_host_pinned_children_ownership_root_read_usage_and_recursive_control(self):
        coordinator = self.start()
        busy = self.success(self.invoke("resume", "busy-resume", "--run-request", "request-1", "--detach"))
        self.submissions.append("busy-resume")
        self.assertEqual(busy["run_id"], coordinator)
        self.settled("busy-resume")
        result = self.success(self.invoke("read", "busy-resume"))["operation_result"]
        self.assertIs(result["ok"], False)
        self.assertIn("active owner", result["error"])
        self.assertEqual(self.tree.manifest(coordinator)["revision"], 1)
        self.assertEqual(self.tree.manifest(coordinator)["status"], "running")
        completed = self.tree.command(coordinator, "run", "--task", "ordinary",
                                      "--completion-gate", "Deliver fixture handoff")
        self.assertEqual(completed["returncode"], 0, completed)
        child_id = json.loads(completed["stdout"])["run_id"]
        child = self.tree.manifest(child_id)
        self.assertEqual(child["parent_run_id"], coordinator)
        self.assertFalse(child["allow_subteam"])
        self.assertEqual(child["creator_main_session_id"], self.tree.manifest(coordinator)["thread_id"])
        root_view = self.manifest()
        self.assertEqual(len(root_view["descendants"]["runs"]), 1)
        self.assertGreater(root_view["descendants"]["usage"]["input_tokens"], 0)
        self.assertEqual(root_view["descendants"]["usage"], child["usage"])
        denied = self.invoke("read", child_id)
        self.assertNotEqual(denied.returncode, 0)
        denied = self.invoke("read", "request-1", "--file", "../"+child_id+"/manifest.json")
        self.assertNotEqual(denied.returncode, 0)
        active_child = self.tree.child(coordinator)
        for caller in (coordinator, active_child["run_id"]):
            rejected = self.tree.command(caller, "run", "--host", "another-host",
                                         "--task", "forbidden hop", "--completion-gate", "unused")
            self.assertNotEqual(rejected["returncode"], 0, rejected)
        self.success(self.invoke("interrupt", "request-1", "--recursive", "--operation-id", "stop-tree"))
        self.settled("request-1")
        self.assertEqual(self.tree.manifest(coordinator)["status"], "interrupted")
        self.assertEqual(self.tree.manifest(active_child["run_id"])["status"], "interrupted")

    def test_lost_interrupt_ack_and_stale_control_cannot_stop_resumed_turn(self):
        coordinator = self.start(allow=False)
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "proxy-drop-control-once"
        result = self.invoke("interrupt", "request-1", "--operation-id", "original-stop")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown", result.stderr.lower())
        controls = [json.loads(item["wire"]) for item in self.f.exchanges()
                    if json.loads(item["wire"])["grant"]["op"] == "interrupt"]
        self.assertEqual(len(controls), 1, "unknown delivery must not automatically retry effect")
        original_control = controls[0]
        stale_unsent = json.loads(json.dumps(original_control["grant"]))
        stale_unsent["operation_id"] = "never-delivered-old-stop"
        self.settled("request-1")
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "proxy"
        retained = self.success(self.invoke("read", "request-1", "--operation-id", "original-stop"))
        self.assertEqual(retained["operation_id"], "original-stop")
        self.assertEqual(retained["target"], original_control["grant"]["target"])
        self.assertEqual(retained["result"]["status"], "acknowledged")
        # Reconcile exactly the original operation before explicit resume.
        self.success(self.invoke("interrupt", "request-1", "--operation-id", "original-stop"))
        resumed = self.start("resume-1", resume="request-1", allow=False)
        self.assertEqual(resumed, coordinator)
        before = self.tree.manifest(coordinator)
        self.assertEqual(before["status"], "running")
        self.assertEqual(before["revision"], 2)
        # Replaying an accepted signed control may return its retained receipt or
        # reject staleness, but must never signal the new revision/turn.
        replay = self.f.endpoint(original_control)
        self.assertEqual(replay.returncode, 0, replay.stderr)
        replayed = json.loads(replay.stdout)
        self.assertIs(replayed["ok"], True, replayed)
        self.assertEqual(replayed["result"]["target"], original_control["grant"]["target"])
        self.assertEqual(self.tree.manifest(coordinator)["status"], "running")
        self.assertEqual(self.tree.manifest(coordinator)["revision"], before["revision"])
        stale = self.f.endpoint(self.f.envelope(stale_unsent, self.f.root / "authority"))
        self.assertEqual(stale.returncode, 0, stale.stderr)
        stale_result = json.loads(stale.stdout)
        self.assertIs(stale_result["ok"], True, stale_result)
        self.assertEqual(stale_result["result"]["result"]["status"], "failed_or_unknown")
        self.assertEqual(self.tree.manifest(coordinator)["status"], "running")
        again = self.success(self.invoke("interrupt", "request-1", "--operation-id", "original-stop"))
        self.assertEqual(again["target"], original_control["grant"]["target"])
        self.assertEqual(again["result"]["status"], "acknowledged")
        self.assertEqual(self.tree.manifest(coordinator)["status"], "running")
        self.success(self.invoke("interrupt", "resume-1", "--operation-id", "fresh-stop"))
        self.settled("resume-1")
        self.assertEqual(self.tree.manifest(coordinator)["status"], "interrupted")


if __name__ == "__main__":
    unittest.main()
