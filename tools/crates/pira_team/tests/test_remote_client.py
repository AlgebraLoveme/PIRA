"""Offline client contracts with synthetic session/key fixtures and fake SSH.

Run outside a real Team worker ancestry, with a freshly built PIRA_TEAM_BIN.
This tests launcher behavior, not authenticated root identity or a real SSH host.
"""
from __future__ import annotations

import json
import os
import shlex
import unittest

from test_remote_fixture import RemoteFixture


@unittest.skipUnless(os.name == "posix", "POSIX fake SSH executable")
class RemoteClientTests(unittest.TestCase):
    def setUp(self):
        self.f = RemoteFixture(self)
        result = self.f.keygen()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.f.completed_reply()

    def run_remote(self, task="fixture task", request_id="request-1", **options):
        cwd = options.pop("remote-cwd", self.f.cwd)
        return self.f.invoke("run", *self.f.client_options(request_id, **options),
                             "--remote-cwd", cwd, "--detach", "--model", "test-model",
                             "--effort", "high", "--completion-gate", "Report fixture result", "--task", task)

    def require_success(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def state_snapshot(self):
        return {str(p.relative_to(self.f.state)):(p.read_bytes(), p.stat().st_mtime_ns)
                for p in self.f.state.rglob("*") if p.is_file()}

    def test_shell_metacharacters_remain_literal_structured_data(self):
        task = "--host=not-a-host; $(touch NEVER) 'quoted'\nλ"
        executable = "/opt/pira candidate 'quoted';$(NEVER)/pira_team"
        store = "/srv/store 'quoted';$(NEVER)/store"
        backend = "/opt/backend 'quoted';$(NEVER)/codex"
        cwd = "/srv/workspace 'quoted';$(NEVER)"
        self.require_success(self.run_remote(task, **{"remote-exe":executable, "remote-store":store,
                                                     "remote-backend":backend, "remote-cwd":cwd}))
        submitted = self.f.grants("submit")
        self.assertEqual(len(submitted), 1)
        self.assertIn(task, submitted[0]["args"])
        self.assertEqual(submitted[0]["endpoint"]["exe"], executable)
        self.assertEqual(submitted[0]["endpoint"]["store"], store)
        self.assertEqual(submitted[0]["endpoint"]["backend"], backend)
        self.assertEqual(submitted[0]["cwd"], cwd)
        for exchange in self.f.exchanges():
            argv = exchange["argv"]
            self.assertIn("-T", argv)
            for setting in ("BatchMode=yes", "StrictHostKeyChecking=yes", "ForwardAgent=no"):
                self.assertIn(setting, argv)
            self.assertEqual(shlex.split(argv[-1]), ["exec", executable, "__remote-serve"])
            self.assertNotIn(task, " ".join(argv))
            self.assertNotIn("PRIVATE KEY", exchange["wire"])

    def test_local_task_file_and_double_dash_are_task_not_remote_options(self):
        task = "--host literal\nλ $HOME; never execute"
        path = self.f.root / "local task.txt"
        path.write_text(task, encoding="utf-8")
        result = self.f.invoke("run", *self.f.client_options(), "--remote-cwd", self.f.cwd,
                               "--detach", "--completion-gate", "Deliver fixture", "--task-file", path)
        self.require_success(result)
        self.assertIn(task, self.f.grants("submit")[-1]["args"])
        result = self.f.invoke("run", *self.f.client_options("dash-task"), "--remote-cwd", self.f.cwd,
                               "--detach", "--completion-gate", "Deliver fixture", "--", task)
        self.require_success(result)
        self.assertIn(task, self.f.grants("submit")[-1]["args"])

    def test_service_tier_is_signed_for_run_resume_and_bound_to_replay(self):
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "retained-topology"
        options = self.f.client_options("tier-run")
        extra = ["--remote-cwd", str(self.f.cwd), "--detach", "--completion-gate", "Deliver fixture",
                 "--service-tier", "fast", "--task", "tier fixture"]
        self.require_success(self.f.invoke("run", *options, *extra))
        submitted = self.f.grants("submit")[-1]
        index = submitted["args"].index("--service-tier")
        self.assertEqual(submitted["args"][index + 1], "fast")
        before = len(self.f.exchanges())
        changed = extra.copy()
        changed[changed.index("fast")] = "standard"
        refused = self.f.invoke("run", *options, *changed)
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(len(self.f.exchanges()), before, "tier changes must not reuse a request ID")
        for request, tier in [("tier-resume-fast", "fast"), ("tier-resume-standard", "standard")]:
            with self.subTest(tier=tier):
                self.require_success(self.f.invoke("resume", *self.f.client_options(request),
                    "--run-request", "tier-run", "--detach", "--service-tier", tier))
                submitted = self.f.grants("submit")[-1]["args"]
                self.assertEqual(submitted[submitted.index("--service-tier") + 1], tier)

    def test_remote_live_tier_is_rejected_without_transport_or_state_changes(self):
        before = self.state_snapshot()
        refused = self.f.invoke("tier", *self.f.client_options(), "fast")
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("unsupported remote command", refused.stderr)
        self.assertEqual(self.f.exchanges(), [])
        self.assertEqual(self.state_snapshot(), before)

    def test_invalid_host_is_rejected_before_transport(self):
        for host in ("-oProxyCommand=touch", "host;touch", "host\ncommand", "$(command)"):
            with self.subTest(host=host):
                result = self.run_remote(**{"host":host})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("host", result.stderr.lower())
                self.assertEqual(self.f.exchanges(), [])

    def test_ambiguous_remote_paths_and_local_authority_options_reject(self):
        for option, value in (("remote-exe", "relative"), ("remote-store", "/tmp/../store"),
                              ("remote-backend", "/tmp/./codex"), ("remote-state", "relative")):
            with self.subTest(option=option):
                self.assertNotEqual(self.run_remote(**{option:value}).returncode, 0)
                self.assertEqual(self.f.exchanges(), [])
        for option in ("--cwd", "--store", "--schema", "--label"):
            with self.subTest(option=option):
                result = self.f.invoke("run", *self.f.client_options(), "--remote-cwd", self.f.cwd,
                                       option, "/tmp/not-authority", "--task", "unused", "--completion-gate", "unused")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(option, result.stderr)
                self.assertEqual(self.f.exchanges(), [])

    def test_protocol_mismatch_never_submits_work(self):
        self.f.respond({}, version=1)
        result = self.run_remote()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("protocol", result.stderr.lower())
        self.assertEqual(self.f.grants("submit"), [])

    def test_missing_capabilities_never_submits_work(self):
        self.f.respond({"protocol":2, "scope":"root-main-host-pinned", "capabilities":[]})
        result = self.run_remote()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("capabilit", result.stderr.lower())
        self.assertEqual(self.f.grants("submit"), [])

    def test_resume_requires_hostpin_capability_after_prior_promotion(self):
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "retained-topology"
        initial = self.require_success(self.run_remote())
        self.assertIs(initial["manifest"]["allow_subteam"], False)
        promoted = self.require_success(self.f.invoke(
            "resume", *self.f.client_options("resume-a"), "--run-request", "request-1",
            "--allow-subteam", "--detach"))
        self.assertEqual(promoted["run_id"], initial["run_id"])
        self.assertIs(promoted["manifest"]["allow_subteam"], True)
        self.assertEqual(promoted["manifest"]["revision"], 2)
        reply = json.loads(self.f.reply.read_text())
        reply["result"]["capabilities"].remove("host-pinned-subteam")
        self.f.reply.write_text(json.dumps(reply), encoding="utf-8")
        count = len(self.f.exchanges())
        refused = self.f.invoke("resume", *self.f.client_options("resume-b"),
                                "--run-request", "request-1", "--detach")
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("host-pinned-subteam", refused.stderr)
        attempted = self.f.grants()[count:]
        self.assertTrue(any(grant["op"] == "read" and grant["args"] == ["read"]
                            and grant["request_id"] == "request-1" and grant["context"] is None
                            for grant in attempted), "resume must query authenticated retained server topology")
        submitted = self.f.grants("submit")
        self.assertEqual([grant["request_id"] for grant in submitted], ["request-1", "resume-a"])
        self.assertNotIn("--allow-subteam", submitted[0]["args"])
        retained = self.require_success(self.f.invoke("read", *self.f.client_options()))
        self.assertIs(retained["manifest"]["allow_subteam"], True)
        self.assertEqual(retained["manifest"]["revision"], 2)

    def test_lost_submission_ack_reconciles_same_payload_and_attach_only_reads(self):
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "drop-once"
        first = self.run_remote()
        self.assertNotEqual(first.returncode, 0)
        self.assertIn("unknown", first.stderr.lower())
        before = self.state_snapshot()
        self.require_success(self.run_remote())
        submissions = self.f.grants("submit")
        self.assertEqual(len(submissions), 2)
        for field in ("request_id", "issuer", "endpoint", "args", "context", "cwd"):
            self.assertEqual(submissions[0][field], submissions[1][field], field)
        self.assertEqual(self.state_snapshot(), before)
        count = len(self.f.exchanges())
        self.require_success(self.f.invoke("attach", *self.f.client_options()))
        self.assertTrue(all(g["op"] in ("probe", "read") for g in self.f.grants()[count:]))
        self.assertEqual(len(self.f.grants("submit")), 2)

    def test_request_payload_or_endpoint_change_rejects_before_transport(self):
        self.require_success(self.run_remote())
        count = len(self.f.exchanges())
        self.assertNotEqual(self.run_remote("different payload").returncode, 0)
        self.assertEqual(len(self.f.exchanges()), count)
        for field, value in (("host", "another-alias"), ("remote-user", "another-user"),
                             ("remote-exe", "/different/pira_team"), ("remote-store", "/different/store")):
            with self.subTest(field=field):
                result = self.f.invoke("read", *self.f.client_options(**{field:value}))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(self.f.exchanges()), count)

    def test_tightened_policy_preserves_read_attach_interrupt_without_state_writes(self):
        self.require_success(self.run_remote())
        before = self.state_snapshot()
        self.f.local.parent([{"sandbox_policy":{"type":"read-only"}}])
        for operation in ("read", "attach", "interrupt"):
            with self.subTest(operation=operation):
                extra = ("--operation-id", "tightened-stop") if operation == "interrupt" else ()
                self.require_success(self.f.invoke(operation, *self.f.client_options(), *extra))
                self.assertEqual(self.state_snapshot(), before)
        count = len(self.f.exchanges())
        rejected = self.f.invoke("steer", *self.f.client_options(), "--operation-id", "tightened-steer",
                                 "--task", "unauthorized change", "--completion-gate", "unused")
        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(len(self.f.exchanges()), count)
        self.assertEqual(self.state_snapshot(), before)

    def test_changed_caller_cannot_reuse_authority_key(self):
        self.require_success(self.run_remote())
        count = len(self.f.exchanges())
        directory = self.f.root / "original-home/sessions/2026/10/01"
        context = {**self.f.local.permission_context(), "sandbox_policy":{"type":"danger-full-access"}}
        log = directory / "rollout-another-owner.jsonl"
        log.write_text(json.dumps({"type":"session_meta", "payload":{"id":"another-owner"}})+"\n"+
                       json.dumps({"type":"turn_context", "payload":context})+"\n", encoding="utf-8")
        self.f.env["CODEX_THREAD_ID"] = "another-owner"
        result = self.f.invoke("read", *self.f.client_options())
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.f.exchanges()), count)


if __name__ == "__main__":
    unittest.main()
