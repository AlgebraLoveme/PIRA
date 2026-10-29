"""Transparent remote CLI contracts, using only generated sessions/keys and peers.

Set PIRA_TEAM_BIN to a candidate build and run outside native worker ancestry.
Linux uses the real signed endpoint through a local subprocess proxy; macOS uses
a deterministic wire peer because the remote supervisor requires Linux. Neither
mode establishes genuine MAIN identity, real SSH connectivity, or model behavior.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import select
import socket
import subprocess
import sys
import time
import unittest
from urllib.parse import urlsplit

from test_remote_fixture import RemoteFixture, SignedEndpointFixture, wait_for_file


# PIRA: the macOS peer models protocol retention, not backend execution. Run these
# same cases on Linux for real signed endpoint and synthetic backend coverage.
ROUTING_SSH = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys

wire = sys.stdin.read()
envelope = json.loads(wire)
grant = envelope["grant"]
trace = Path(os.environ["TEAM_REMOTE_TRACE"])
with trace.open("a", encoding="utf-8") as out:
    out.write(json.dumps({"argv":sys.argv[1:], "wire":wire}) + "\n")
op = grant["op"]
drop = os.environ.get("TEAM_ROUTING_DROP", "")
marker = trace.with_name("drop-" + drop)
if op == "submit" and os.environ.get("TEAM_ROUTING_BARRIER"):
    with socket.create_connection(tuple(json.loads(os.environ["TEAM_ROUTING_BARRIER"])), timeout=15) as peer:
        peer.sendall(b"1")
        assert peer.recv(1) == b"1"
if drop == op + "-before" and not marker.exists():
    marker.touch()
    sys.exit(255)
if os.environ["TEAM_ROUTING_MODE"] == "proxy":
    command = shlex.split(sys.argv[-1])
    assert command.pop(0) == "exec", command
    result = subprocess.run(command, input=wire, capture_output=True, text=True,
                            encoding="utf-8", timeout=30)
    stdout, stderr, code = result.stdout, result.stderr, result.returncode
else:
    state_path = trace.with_suffix(".server.json")
    state = json.loads(state_path.read_text()) if state_path.exists() else {"requests":{}, "controls":{}, "runs":{}}
    requests, controls, runs = state["requests"], state["controls"], state["runs"]
    request = grant["request_id"]
    def view(run_id):
        run = runs[run_id]
        return {"request_id":request, "run_id":run_id, "accepted":True,
                "supervisor":"unknown", "operation_result":{"ok":True},
                "manifest":run["manifest"], "control_target":{
                    "revision":run["manifest"]["revision"], "subtree_epoch":0,
                    "control_epoch":"fixture-epoch", "turn_id":"fixture-turn"},
                "accounting_may_be_incomplete":False}
    def task(args):
        return args[args.index("--task") + 1] if "--task" in args else "fixture continuation"
    value, ok = None, True
    if op == "read" and grant["args"] == ["capabilities"]:
        value = {"protocol":2, "scope":"root-main-host-pinned", "capabilities":[
            "signed-delegation", "host-pinned-subteam", "durable-submit", "read", "resume", "steer", "interrupt"]}
    elif op == "submit":
        if request in requests:
            retained = requests[request]["envelope"]["grant"]
            payload = lambda g: {k:v for k,v in g.items() if k not in ("issued_at", "expires_at")}
            assert payload(retained) == payload(grant), "retry changed retained operation"
            value = view(requests[request]["run_id"])
        else:
            if grant["args"][0] == "run":
                run_id = "remote-" + grant["nonce"]
                manifest = {"run_id":run_id, "creator_main_session_id":grant["issuer"],
                            "parent_run_id":None, "allow_subteam":"--allow-subteam" in grant["args"], "revision":1,
                            "status":"completed", "task":task(grant["args"])}
                runs[run_id] = {"manifest":manifest, "handoff":"ANSWER " + manifest["task"], "files":{}}
            else:
                assert grant["args"][0] == "resume"
                run_id = requests[grant["run_request"]]["run_id"]
                runs[run_id]["manifest"]["revision"] += 1
                runs[run_id]["manifest"]["task"] = task(grant["args"])
                runs[run_id]["handoff"] = "ANSWER " + task(grant["args"])
            requests[request] = {"envelope":envelope, "run_id":run_id}
            value = view(run_id)
    elif op == "read" and grant["operation_id"] is not None:
        operation = grant["operation_id"]
        value = controls.get(operation, {"operation_id":operation, "status":"not_found"})
    elif op == "read":
        if request not in requests:
            value = {"request_id":request, "accepted":False, "status":"not_found"}
        else:
            run_id = requests[request]["run_id"]
            value = view(run_id)
            if grant["artifact"]:
                value["content"] = runs[run_id]["handoff"]
            elif grant["file"] == "manifest.json":
                value["content"] = json.dumps(runs[run_id]["manifest"], ensure_ascii=False)
            elif grant["file"] is not None:
                value["content"] = runs[run_id]["files"][grant["file"]]
    elif op in ("steer", "interrupt"):
        operation = grant["operation_id"]
        intent = {k:v for k,v in grant.items() if k not in ("issued_at", "expires_at", "nonce", "target")}
        value = {"operation_id":operation, "target":grant["target"], "intent":intent,
                 "result":{"status":"acknowledged", "completion_confirmed":False,
                           "receipt":{"status":"accepted"}}}
        if operation in controls:
            assert controls[operation] == value, "control changed payload or target"
        else:
            controls[operation] = value
            run_id = requests[request]["run_id"]
            if op == "steer":
                runs[run_id]["manifest"]["task"] = task(grant["args"])
                runs[run_id]["handoff"] = "ANSWER " + task(grant["args"])
            else:
                runs[run_id]["manifest"]["status"] = "interrupted"
    else:
        raise AssertionError("unexpected fixture operation: " + op)
    state_path.write_text(json.dumps(state), encoding="utf-8")
    stdout = json.dumps({"version":2, "ok":ok, "result" if ok else "error":value})
    stderr, code = "", 0
accepted = code == 0 and json.loads(stdout).get("ok") is True
if accepted and drop == op + "-after" and not marker.exists():
    marker.touch()
    sys.exit(255)
sys.stdout.write(stdout)
sys.stderr.write(stderr)
sys.exit(code)
'''


@unittest.skipUnless(os.name == "posix", "POSIX fake SSH executable")
class RemoteRoutingTests(unittest.TestCase):
    def setUp(self):
        self.proxy = sys.platform == "linux"
        self.f = (SignedEndpointFixture(self, issuer="parent-session") if self.proxy
                  else RemoteFixture(self))
        self.local_store = self.f.root / "local-team-store"
        self.f.env["PIRA_TEAM_DIR"] = str(self.local_store)
        keygen = self.f.keygen()
        self.assertEqual(keygen.returncode, 0, keygen.stderr)
        if self.proxy:
            self.f.enroll(self.f.issuer, self.f.root / "authority")
        transport = self.f.root / "ssh"
        transport.write_text(ROUTING_SSH.replace("#!/usr/bin/env python3", f"#!{sys.executable}"),
                             encoding="utf-8")
        self.f.env["TEAM_ROUTING_MODE"] = "proxy" if self.proxy else "fake"
        self.addCleanup(self.settle_owned)

    def options(self, request_id=None):
        overrides = {}
        if self.proxy:
            overrides = {"remote-user":self.f.endpoint_binding["user"],
                         "remote-exe":self.f.server,
                         "remote-backend":self.f.endpoint_binding["backend"]}
        options = self.f.client_options(request_id or "unused", **overrides)
        if request_id is None:
            index = options.index("--request-id")
            del options[index:index + 2]
        return options

    def launch(self, task="routing fixture λ\nsecond line", *extra):
        return self.f.invoke("run", *self.options(), "--store", self.local_store,
                             "--remote-cwd", self.f.cwd, "--detach", "--model", "test-model",
                             "--effort", "high", "--task", task,
                             "--completion-gate", "Deliver synthetic handoff", *extra)

    def ordinary(self, operation, run, *extra, store=None):
        return self.f.invoke(operation, run, *extra, "--store", store or self.local_store)

    def legacy_launch(self, request_id, task="legacy remote fixture"):
        return self.f.invoke("run", *self.options(request_id),
                             "--remote-cwd", self.f.cwd, "--detach", "--model", "test-model",
                             "--effort", "high", "--task", task,
                             "--completion-gate", "Deliver synthetic handoff")

    def success(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def receipt(self, result):
        value = self.success(result)
        self.assertEqual(set(value), {"run_id", "status", "run_root", "handoff_path"})
        self.assertRegex(value["run_id"], r"^remote-[0-9a-f]{64}$")
        self.assertIsInstance(value["status"], str)
        for key in ("run_root", "handoff_path"):
            path = value[key]
            if path is not None:
                self.assertIsInstance(path, str)
                self.assertTrue(urlsplit(path).scheme and urlsplit(path).netloc, path)
                self.assertFalse(Path(path).is_absolute(), path)
        return value

    def route_path(self, run):
        return self.local_store / ".remote-routes" / run / "route.json"

    def submissions(self):
        return self.f.grants("submit")

    def assert_same_operation(self, first, second):
        # Equivalent current authority can refresh expiry/signature. Allocation
        # nonce, request/control IDs, target epoch and execution payload stay fixed.
        payload = lambda grant: {k:v for k,v in grant.items() if k not in ("issued_at", "expires_at")}
        self.assertEqual(payload(first), payload(second))

    def settled(self, request):
        if self.proxy:
            directory = self.f.store / ".remote-v2" / request
            wait_for_file(directory / "result.json")
            from native_fixture import wait_owner
            wait_owner(directory / "supervisor.lock")

    def settle_owned(self):
        """Release only these generated runs; never use persisted PIDs or real SSH."""
        self.f.env.pop("TEAM_ROUTING_DROP", None)
        self.f.env.pop("TEAM_ROUTING_BARRIER", None)
        if not self.proxy:
            return
        seen = set()
        for request in self.submissions():
            if request["request_id"] in seen:
                continue
            seen.add(request["request_id"])
            directory = self.f.store / ".remote-v2" / request["request_id"]
            if not (directory / "request.json").exists():
                continue
            record = json.loads((directory / "request.json").read_text())
            run = Path(record["run"])
            if not (directory / "result.json").exists():
                if self.route_path(run.name).exists():
                    result = self.ordinary("interrupt", run.name)
                else:
                    result = self.f.invoke("interrupt", *self.options(request["request_id"]),
                                           "--operation-id", "cleanup-" + request["request_id"])
                self.assertEqual(result.returncode, 0, result.stderr)
            self.settled(request["request_id"])
            self.assertTrue((run / "manifest.json").exists())

    def started(self, task="routing fixture", *extra):
        receipt = self.receipt(self.launch(task, *extra))
        request = self.submissions()[-1]
        if task == "steerable" and self.proxy:
            directory = self.f.store / ".remote-v2" / request["request_id"]
            wait_for_file(directory / "stderr.log", lambda p: p.exists()
                          and "pira_team active:" in p.read_text())
        else:
            self.settled(request["request_id"])
        return receipt["run_id"]

    def test_launch_receipt_routes_default_and_manifest_reads_without_remote_options(self):
        task = "literal remote handoff λ\nno extra newline"
        run = self.started(task)
        grant = self.submissions()[0]
        self.assertEqual(run, "remote-" + grant["nonce"])
        self.assertTrue(grant["request_id"])
        self.assertTrue(self.route_path(run).is_file())
        self.assertFalse((self.local_store / run / "manifest.json").exists())
        before = len(self.submissions())
        artifact = self.ordinary("read", run)
        self.assertEqual(artifact.returncode, 0, artifact.stderr)
        self.assertEqual(artifact.stdout.encode("utf-8"), ("ANSWER " + task).encode("utf-8"))
        manifest = self.success(self.ordinary("read", run, "manifest.json"))
        self.assertEqual(manifest["run_id"], run)
        self.assertEqual(manifest["creator_main_session_id"], "parent-session")
        self.assertEqual(manifest["status"], "completed")
        self.assertEqual(len(self.submissions()), before, "read must not start a model turn")

    def test_explicit_request_id_retains_lower_level_structured_transport(self):
        result = self.legacy_launch("legacy-request")
        value = self.success(result)
        self.assertEqual(value["request_id"], "legacy-request")
        self.assertIs(value["accepted"], True)
        run = "remote-" + self.submissions()[0]["nonce"]
        self.assertEqual(value["run_id"], run)
        self.assertTrue(self.route_path(run).is_file())
        self.assertTrue((self.f.state / "legacy-request.json").is_file())
        self.assertFalse((self.f.state / ".remote-routes").exists())
        self.settled("legacy-request")
        read = self.success(self.f.invoke("read", *self.options("legacy-request"), "--artifact"))
        self.assertEqual(read["content"], "ANSWER legacy remote fixture")
        raw = self.ordinary("read", run)
        self.assertEqual(raw.returncode, 0, raw.stderr)
        self.assertEqual(raw.stdout, "ANSWER legacy remote fixture")
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["run_id"], run)
        before = len(self.f.exchanges())
        resolved = self.receipt(self.f.invoke("resolve", "legacy-request"))
        self.assertEqual(resolved["run_id"], run)
        self.assertEqual(len(self.f.exchanges()), before, "resolve is a local retained-handle lookup")

    def test_public_request_resolution_recovers_legacy_launch_with_no_run_receipt(self):
        for drop in ("submit-before", "submit-after"):
            with self.subTest(drop=drop):
                request = "legacy-lost-" + drop
                self.f.env["TEAM_ROUTING_DROP"] = drop
                first = self.legacy_launch(request, "recover " + request)
                self.assertNotEqual(first.returncode, 0)
                self.assertEqual(first.stdout, "", "structured run receipt was not delivered")
                # Recover using only the caller's known request ID, even if all
                # launch output (including stderr's allocated run ID) was lost.
                before = len(self.f.exchanges())
                resolved = self.receipt(self.f.invoke("resolve", request))
                self.assertEqual(len(self.f.exchanges()), before)
                run = resolved["run_id"]
                initial = next(g for g in self.submissions() if g["request_id"] == request)
                self.assertEqual(run, "remote-" + initial["nonce"])
                self.assertTrue(self.route_path(run).is_file())
                self.ordinary("read", run, "manifest.json")
                self.settled(request)
                raw = self.ordinary("read", run)
                self.assertEqual(raw.returncode, 0, raw.stderr)
                self.assertEqual(raw.stdout, "ANSWER recover " + request)
                attempts = [g for g in self.submissions() if g["request_id"] == request]
                self.assertEqual(len(attempts), 2 if drop == "submit-before" else 1)
                for grant in attempts:
                    self.assert_same_operation(grant, initial)
                self.assertTrue(all(g["args"][0] == "run" for g in self.submissions()))
                self.f.env.pop("TEAM_ROUTING_DROP", None)

    def test_relative_artifact_read_returns_raw_content_not_protocol_json(self):
        run = self.started()
        content = "# Fixture report λ\nraw $HOME; not JSON\nlast line"
        relative = "artifacts/report.md"
        if self.proxy:
            path = self.f.store / run / relative
            path.parent.mkdir(exist_ok=True, mode=0o700)
            path.write_text(content, encoding="utf-8")
            path.chmod(0o600)
        else:
            path = self.f.trace.with_suffix(".server.json")
            state = json.loads(path.read_text())
            state["runs"][run]["files"][relative] = content
            path.write_text(json.dumps(state), encoding="utf-8")
        before = len(self.submissions())
        result = self.ordinary("read", run, relative)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.encode("utf-8"), content.encode("utf-8"))
        request = self.f.grants("read")[-1]
        self.assertEqual(request["file"], relative)
        self.assertIs(request["artifact"], False)
        self.assertEqual(len(self.submissions()), before)

    def test_manifest_preserves_descendant_lifecycle_and_usage_summary(self):
        run = self.started("summary fixture", "--allow-subteam")
        child = "retained-fixture-child"
        usage = {"input_tokens":7, "output_tokens":3}
        expected = {"runs":[{"run_id":child, "status":"completed", "active":False,
                             "usage":usage, "usage_complete":True}],
                    "usage":usage, "usage_complete":True, "interruption_gate":False, "epoch":4}
        # Seed retained topology only; this is summary transport coverage, not a
        # child lifecycle test or a claim of actual delegated worker execution.
        if self.proxy:
            directory = self.f.store / child
            directory.mkdir(mode=0o700)
            manifest = directory / "manifest.json"
            manifest.write_text(json.dumps({"run_id":child, "status":"completed", "usage":usage,
                                            "usage_complete":True, "task":"not descendant content"}),
                                encoding="utf-8")
            manifest.chmod(0o600)
            path = self.f.store / run / "tree.json"
            tree = json.loads(path.read_text())
            tree.update(blocked=False, epoch=4)
            tree["members"][child] = {"active":False, "revision":1, "blocked":False}
            path.write_text(json.dumps(tree), encoding="utf-8")
        else:
            path = self.f.trace.with_suffix(".server.json")
            state = json.loads(path.read_text())
            state["runs"][run]["manifest"]["descendants"] = expected
            path.write_text(json.dumps(state), encoding="utf-8")
        before = len(self.submissions())
        manifest = self.success(self.ordinary("read", run, "manifest.json"))
        self.assertEqual(manifest["run_id"], run)
        self.assertEqual(manifest["creator_main_session_id"], "parent-session")
        self.assertIsNone(manifest["parent_run_id"])
        self.assertIs(manifest["allow_subteam"], True)
        self.assertEqual(manifest["descendants"], expected)
        self.assertEqual(len(self.submissions()), before)

    def test_each_explicit_resume_allocates_next_request_but_reads_do_not(self):
        run = self.started()
        initial = self.submissions()[0]
        for turn in (2, 3):
            result = self.ordinary("resume", run, "--detach", "--task", f"turn {turn}",
                                   "--completion-gate", "Deliver resumed synthetic handoff")
            self.assertEqual(self.receipt(result)["run_id"], run)
            request = self.submissions()[-1]
            self.assertEqual(request["args"][0], "resume")
            self.assertEqual(request["run_request"], initial["request_id"])
            self.settled(request["request_id"])
            manifest = self.success(self.ordinary("read", run, "manifest.json"))
            self.assertEqual(manifest["revision"], turn)
            self.assertEqual(self.ordinary("read", run).stdout, f"ANSWER turn {turn}")
            self.assertEqual(len(self.submissions()), turn)
        self.assertEqual(len({g["request_id"] for g in self.submissions()}), 3)

    def test_remote_path_fails_without_fabricating_local_path_or_starting_turn(self):
        run = self.started()
        before = len(self.submissions())
        result = self.ordinary("path", run, ".")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertRegex(result.stderr.lower(), r"remote.*(path|local)|(path|local).*remote")
        self.assertEqual(len(self.submissions()), before)

    def test_wait_by_run_id_and_bookmark_observes_without_new_turn(self):
        run = self.started("wait fixture", "--label", "wait-remote")
        before = len(self.submissions())
        for reference in (run, "@wait-remote"):
            with self.subTest(reference=reference):
                receipt = self.receipt(self.ordinary("wait", reference))
                self.assertEqual(receipt["run_id"], run)
                self.assertEqual(receipt["status"], "completed")
        self.assertEqual(len(self.submissions()), before)
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["revision"], 1)

    def test_local_bookmark_routes_read_resume_steer_and_interrupt(self):
        run = self.started("steerable", "--label", "remote λ")
        self.assertEqual(self.success(self.ordinary("read", "@remote λ", "manifest.json"))["run_id"], run)
        self.success(self.ordinary("steer", "@remote λ", "--task", "redirected literal $HOME λ",
                                   "--completion-gate", "Deliver redirected synthetic handoff"))
        self.settled(self.submissions()[-1]["request_id"])
        self.assertEqual(self.ordinary("read", "@remote λ").stdout, "ANSWER redirected literal $HOME λ")
        receipt = self.receipt(self.ordinary("resume", "@remote λ", "--detach", "--task", "steerable",
                                             "--completion-gate", "Deliver resumed synthetic handoff"))
        self.assertEqual(receipt["run_id"], run)
        if self.proxy:
            directory = self.f.store / ".remote-v2" / self.submissions()[-1]["request_id"]
            wait_for_file(directory / "stderr.log", lambda p: p.exists() and "pira_team active:" in p.read_text())
        self.success(self.ordinary("interrupt", "@remote λ"))
        self.settled(self.submissions()[-1]["request_id"])
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["status"], "interrupted")
        self.assertEqual([g["args"][0] for g in self.submissions()], ["run", "resume"])

    def test_lost_launch_ack_read_observes_one_initial_turn_without_resume(self):
        self.f.env["TEAM_ROUTING_DROP"] = "submit-after"
        first = self.launch("accepted despite dropped ack")
        self.assertNotEqual(first.returncode, 0)
        self.assertIn("unknown", first.stderr.lower())
        grant = self.submissions()[0]
        run = "remote-" + grant["nonce"]
        self.assertIn(run, first.stderr)
        self.assertTrue(self.route_path(run).is_file())
        self.settled(grant["request_id"])
        before = len(self.submissions())
        recovered = self.ordinary("read", run)
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assertEqual(recovered.stdout, "ANSWER accepted despite dropped ack")
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["revision"], 1)
        self.assertEqual(len(self.submissions()), before)

    def test_unaccepted_launch_read_retries_exact_retained_initial_operation(self):
        self.f.env["TEAM_ROUTING_DROP"] = "submit-before"
        first = self.launch("not accepted yet")
        self.assertNotEqual(first.returncode, 0)
        first_wire = [e["wire"] for e in self.f.exchanges()
                      if json.loads(e["wire"])["grant"]["op"] == "submit"][0]
        grant = json.loads(first_wire)["grant"]
        run = "remote-" + grant["nonce"]
        self.assertIn(run, first.stderr)
        # Recovery can precede creation of the remote manifest. Validate the
        # retained submission now, then the read result after endpoint settlement.
        self.ordinary("read", run, "manifest.json")
        wires = [e["wire"] for e in self.f.exchanges()
                 if json.loads(e["wire"])["grant"]["op"] == "submit"]
        self.assertEqual(len(wires), 2)
        self.assert_same_operation(json.loads(wires[0])["grant"], json.loads(wires[1])["grant"])
        self.settled(grant["request_id"])
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["revision"], 1)
        recovered = self.ordinary("read", run)
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assertEqual(recovered.stdout, "ANSWER not accepted yet")
        self.assertTrue(all(g["args"][0] == "run" for g in self.submissions()))

    def test_run_id_is_on_stderr_while_uncertain_submit_is_still_blocked(self):
        self.f.env["TEAM_ROUTING_DROP"] = "submit-before"
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            listener.settimeout(15)
            self.f.env["TEAM_ROUTING_BARRIER"] = json.dumps(listener.getsockname())
            args = [str(self.f.bin), "run", *self.options(), "--store", str(self.local_store),
                    "--remote-cwd", str(self.f.cwd), "--detach", "--model", "test-model",
                    "--effort", "high", "--task", "barrier fixture", "--completion-gate", "Deliver fixture"]
            process = subprocess.Popen(args, env=self.f.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            prefix = b""
            try:
                with listener.accept()[0] as peer:
                    peer.settimeout(15)
                    self.assertEqual(peer.recv(1), b"1")
                    try:
                        deadline = time.monotonic() + 10
                        while not re.search(rb"remote-[0-9a-f]{64}", prefix):
                            remaining = deadline - time.monotonic()
                            self.assertGreater(remaining, 0, prefix)
                            self.assertTrue(select.select([process.stderr], [], [], remaining)[0], prefix)
                            chunk = os.read(process.stderr.fileno(), 65536)
                            self.assertTrue(chunk, prefix)
                            prefix += chunk
                    finally:
                        peer.sendall(b"1")
                stdout, stderr = process.communicate(timeout=15)
                self.assertNotEqual(process.returncode, 0, stderr)
                self.assertEqual(stdout, b"")
                run = re.search(rb"remote-[0-9a-f]{64}", prefix).group().decode()
                self.assertEqual(run, "remote-" + self.submissions()[0]["nonce"])
                self.assertTrue(self.route_path(run).is_file())
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.communicate(timeout=15)
                self.f.env.pop("TEAM_ROUTING_BARRIER", None)

    def test_uncertain_resume_reuses_request_and_rejects_changed_pending_payload(self):
        run = self.started()
        self.f.env["TEAM_ROUTING_DROP"] = "submit-before"
        flags = ("--detach", "--task", "same explicit next turn", "--completion-gate", "Deliver next fixture")
        first = self.ordinary("resume", run, *flags)
        self.assertNotEqual(first.returncode, 0)
        pending = self.submissions()[-1]
        before = len(self.submissions())
        changed = self.ordinary("resume", run, "--detach", "--task", "different next turn",
                                "--completion-gate", "Deliver next fixture")
        self.assertNotEqual(changed.returncode, 0)
        self.assertRegex(changed.stderr.lower(), r"pending|payload|intent|different")
        self.assertEqual(len(self.submissions()), before)
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["revision"], 1)
        self.assertEqual(len(self.submissions()), before, "read must not submit a pending resume")
        raw = self.ordinary("read", run)
        self.assertEqual(raw.returncode, 0, raw.stderr)
        self.assertEqual(raw.stdout, "ANSWER routing fixture")
        self.assertEqual(len(self.submissions()), before, "default read must not submit a pending resume")
        self.assertEqual(self.receipt(self.ordinary("wait", run))["status"], "completed")
        self.assertEqual(len(self.submissions()), before, "wait must not submit a pending resume")
        self.assertEqual(self.receipt(self.ordinary("resume", run, *flags))["run_id"], run)
        self.settled(pending["request_id"])
        attempts = self.submissions()[1:]
        self.assertEqual(len(attempts), 2)
        self.assert_same_operation(attempts[0], attempts[1])
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["revision"], 2)

    def test_lost_resume_ack_reconciles_without_creating_another_turn(self):
        run = self.started()
        self.f.env["TEAM_ROUTING_DROP"] = "submit-after"
        flags = ("--detach", "--task", "accepted next turn", "--completion-gate", "Deliver next fixture")
        first = self.ordinary("resume", run, *flags)
        self.assertNotEqual(first.returncode, 0)
        pending = self.submissions()[-1]
        self.settled(pending["request_id"])
        self.assertEqual(self.receipt(self.ordinary("resume", run, *flags))["run_id"], run)
        resumes = self.submissions()[1:]
        self.assertTrue(resumes)
        for grant in resumes:
            self.assert_same_operation(grant, pending)
        count = len(self.submissions())
        self.assertEqual(self.success(self.ordinary("read", run, "manifest.json"))["revision"], 2)
        self.assertEqual(self.ordinary("read", run).stdout, "ANSWER accepted next turn")
        self.assertEqual(len(self.submissions()), count)

    def test_uncertain_controls_keep_id_epoch_payload_and_do_not_create_turn(self):
        for operation in ("steer", "interrupt"):
            with self.subTest(operation=operation):
                run = self.started("steerable")
                self.f.env["TEAM_ROUTING_DROP"] = operation + "-before"
                flags = (("--task", "literal changed λ $HOME", "--completion-gate", "Deliver controlled fixture")
                         if operation == "steer" else ())
                first = self.ordinary(operation, run, *flags)
                self.assertNotEqual(first.returncode, 0)
                attempted = self.f.grants(operation)[-1]
                self.assertTrue(attempted["operation_id"])
                self.assertEqual(set(attempted["target"]),
                                 {"revision", "subtree_epoch", "control_epoch", "turn_id"})
                if operation == "steer":
                    before = len(self.f.grants(operation))
                    rejected = self.ordinary(operation, run, "--task", "different pending steer",
                                             "--completion-gate", "Deliver controlled fixture")
                    self.assertNotEqual(rejected.returncode, 0)
                    self.assertEqual(len(self.f.grants(operation)), before)
                count = len(self.submissions())
                self.success(self.ordinary(operation, run, *flags))
                attempts = [g for g in self.f.grants(operation) if g["operation_id"] == attempted["operation_id"]]
                self.assertEqual(len(attempts), 2)
                self.assert_same_operation(attempts[0], attempts[1])
                self.settled(self.submissions()[-1]["request_id"])
                self.assertEqual(len(self.submissions()), count)
                self.f.env.pop("TEAM_ROUTING_DROP", None)

    def test_lost_control_ack_is_observed_without_second_control_effect(self):
        run = self.started("steerable")
        self.f.env["TEAM_ROUTING_DROP"] = "interrupt-after"
        first = self.ordinary("interrupt", run)
        self.assertNotEqual(first.returncode, 0)
        attempted = self.f.grants("interrupt")[-1]
        self.settled(self.submissions()[-1]["request_id"])
        self.success(self.ordinary("interrupt", run))
        self.assertEqual(len(self.f.grants("interrupt")), 1)
        observations = [g for g in self.f.grants("read") if g["operation_id"] == attempted["operation_id"]]
        self.assertTrue(observations)
        self.assertEqual(len(self.submissions()), 1)

    def test_caller_mismatch_cannot_route_by_id_or_bookmark(self):
        run = self.started("caller fixture", "--label", "owned-remote")
        route = self.route_path(run).read_bytes()
        directory = self.f.root / "original-home/sessions/2026/10/01"
        context = {**self.f.local.permission_context(), "sandbox_policy":{"type":"danger-full-access"}}
        (directory / "rollout-another-owner.jsonl").write_text(
            json.dumps({"type":"session_meta", "payload":{"id":"another-owner"}}) + "\n" +
            json.dumps({"type":"turn_context", "payload":context}) + "\n", encoding="utf-8")
        self.f.env["CODEX_THREAD_ID"] = "another-owner"
        before = len(self.f.exchanges())
        for reference in (run, "@owned-remote"):
            with self.subTest(reference=reference):
                result = self.ordinary("read", reference)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
        self.assertEqual(self.route_path(run).read_bytes(), route)
        self.assertEqual(len(self.f.exchanges()), before)

    def test_unknown_or_malformed_routes_fail_closed_without_transport(self):
        unknown = "remote-" + "e" * 64
        before = len(self.f.exchanges())
        rejected = self.ordinary("read", unknown)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(rejected.stdout, "")
        self.assertEqual(len(self.f.exchanges()), before)
        run = self.started()
        path = self.route_path(run)
        original = path.read_bytes()
        before = len(self.f.exchanges())
        for invalid in (b'{"version":', b"null", b"{}"):
            with self.subTest(invalid=invalid):
                path.write_bytes(invalid)
                try:
                    refused = self.ordinary("read", run)
                    self.assertNotEqual(refused.returncode, 0)
                    self.assertEqual(refused.stdout, "")
                    self.assertEqual(path.read_bytes(), invalid)
                    self.assertEqual(len(self.f.exchanges()), before)
                finally:
                    path.write_bytes(original)

    def test_symlinked_route_file_or_directory_cannot_authorize_transport(self):
        run = self.started()
        path = self.route_path(run)
        original = path.read_bytes()
        target = self.f.root / "outside-route.json"
        target.write_bytes(original)
        before = len(self.f.exchanges())
        path.unlink()
        path.symlink_to(target)
        try:
            rejected = self.ordinary("read", run)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual(rejected.stdout, "")
            self.assertEqual(len(self.f.exchanges()), before)
            self.assertEqual(target.read_bytes(), original)
        finally:
            path.unlink()
            path.write_bytes(original)
            path.chmod(0o600)
        retained = path.parent.with_name(run + "-retained")
        path.parent.rename(retained)
        path.parent.symlink_to(retained, target_is_directory=True)
        try:
            rejected = self.ordinary("read", run)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual(rejected.stdout, "")
            self.assertEqual(len(self.f.exchanges()), before)
        finally:
            path.parent.unlink()
            retained.rename(path.parent)

    def test_local_runs_and_other_stores_do_not_inherit_remote_route(self):
        remote = self.started()
        before = len(self.f.exchanges())
        other = self.f.root / "other-local-store"
        rejected = self.ordinary("read", remote, store=other)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(rejected.stdout, "")
        self.assertEqual(len(self.f.exchanges()), before)
        result = self.f.local.launch("ordinary local fixture")
        local = self.success(result)
        self.assertEqual(set(local), {"run_id", "status", "run_root", "handoff_path"})
        self.assertTrue(Path(local["run_root"]).is_dir())
        self.assertEqual(Path(local["handoff_path"]).read_text(), "ANSWER ordinary local fixture")
        read = self.f.local.access("read", local["run_id"])
        self.assertEqual(read.returncode, 0, read.stderr)
        self.assertEqual(read.stdout, "ANSWER ordinary local fixture")
        manifest = self.success(self.f.local.access("read", local["run_id"], "manifest.json"))
        self.assertEqual(manifest["run_id"], local["run_id"])
        waited = self.success(self.f.local.access("wait", local["run_id"]))
        self.assertEqual(waited["run_id"], local["run_id"])
        self.assertEqual(waited["status"], "completed")
        self.assertEqual(waited["run_root"], local["run_root"])
        self.assertEqual(len(self.f.exchanges()), before)
        self.assertFalse((self.f.root / "logs/.remote-routes").exists())


if __name__ == "__main__":
    unittest.main()
