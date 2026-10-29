"""Linux endpoint fault contracts with generated keys and retained-state fixtures.

No real caller identity, SSH, credentials, model, or server processes are involved.
Run outside Team ancestry. Retained fixtures intentionally have no supervisor;
they test verification/reconciliation, not model launch or Linux detachment.
"""
from __future__ import annotations

import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest

from test_remote_fixture import SignedEndpointFixture


@unittest.skipUnless(sys.platform == "linux", "Linux remote endpoint only")
class RemoteEndpointTests(unittest.TestCase):
    def setUp(self):
        self.f = SignedEndpointFixture(self)

    def reply(self, envelope):
        result = self.f.endpoint(envelope)
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value["version"], 2)
        return value

    def rejected(self, envelope):
        value = self.reply(envelope)
        self.assertIs(value["ok"], False, value)
        self.assertTrue(value.get("error"), value)
        return value

    def snapshot(self):
        return {str(p.relative_to(self.f.store)):p.read_bytes()
                for p in self.f.store.rglob("*") if p.is_file()}

    def test_valid_enrolled_root_signed_probe(self):
        value = self.reply(self.f.envelope(self.f.grant("probe")))
        self.assertIs(value["ok"], True, value)
        self.assertEqual(value["result"]["protocol"], 2)
        self.assertIn("durable-submit", value["result"]["capabilities"])
        self.assertIs(value["result"]["backend_available"], True)
        self.assertFalse(self.f.store.exists())

    def test_unsigned_and_self_signed_context_cannot_enroll_authority(self):
        grant = self.f.grant("submit")
        self.rejected({"version":2, "grant":grant})
        rogue_key = self.f.generate_key("rogue-authority")
        envelope = self.f.envelope(grant, rogue_key)
        rejected = self.rejected(envelope)
        self.assertIn("signature", rejected["error"].lower())
        # A caller-supplied public key/anchor must not replace the fixed anchor.
        envelope["public_key"] = Path(str(rogue_key)+".pub").read_text()
        envelope["allowed_signers"] = str(self.f.anchor)
        self.rejected(envelope)
        self.assertFalse(self.f.store.exists())

    def test_mutated_signed_context_rejected_before_store_creation(self):
        envelope = self.f.envelope(self.f.grant("submit"))
        envelope["grant"]["context"]["caller_thread_id"] = "forged-owner"
        self.rejected(envelope)
        self.assertFalse(self.f.store.exists())

    def test_valid_signature_does_not_allow_issuer_context_disagreement(self):
        grant = self.f.grant("submit")
        grant["context"]["caller_thread_id"] = "different-owner"
        self.rejected(self.f.envelope(grant))
        self.assertFalse(self.f.store.exists())

    def test_wrong_namespace_or_protocol_or_scope_rejected(self):
        grant = self.f.grant("probe")
        envelope = self.f.envelope(grant)
        envelope["signature"] = self.f.sign(grant, self.f.key, "not-the-team-namespace")
        self.rejected(envelope)
        for field, value in (("protocol", 1), ("scope", "ordinary-worker")):
            with self.subTest(field=field):
                invalid = {**grant, field:value}
                self.rejected(self.f.envelope(invalid))
        envelope = self.f.envelope(grant)
        envelope["version"] = 1
        self.rejected(envelope)

    def test_missing_fixed_anchor_is_not_replaced_from_request(self):
        self.f.anchor.unlink()
        envelope = self.f.envelope(self.f.grant("probe"))
        rejected = self.rejected(envelope)
        self.assertIn("anchor", rejected["error"].lower())
        envelope["public_key"] = Path(str(self.f.key)+".pub").read_text()
        self.rejected(envelope)
        self.assertFalse(self.f.store.exists())

    def test_public_endpoint_nonce_prevents_cross_enrollment_replay(self):
        envelope = self.f.envelope(self.f.grant("probe"))
        descriptor = json.loads(self.f.descriptor.read_text())
        descriptor["endpoint_id"] = "c"*64
        self.f.descriptor.write_text(json.dumps(descriptor), encoding="utf-8")
        self.rejected(envelope)
        self.assertFalse(self.f.store.exists())

    def test_unsafe_store_ancestor_is_not_remote_authority(self):
        unsafe = self.f.root / "untrusted-parent"
        unsafe.mkdir()
        unsafe.chmod(0o777)
        self.addCleanup(unsafe.chmod, 0o700)
        grant = self.f.grant("submit")
        grant["endpoint"]["store"] = str(unsafe / "store")
        self.rejected(self.f.envelope(grant))
        self.assertFalse((unsafe / "store").exists())

    def test_wrong_execution_user_binary_or_backend_rejected(self):
        for field, value in (("user", "not-this-os-user"), ("exe", "/not/this/pira_team"),
                             ("backend", "relative-codex")):
            with self.subTest(field=field):
                grant = self.f.grant("probe")
                grant["endpoint"][field] = value
                self.rejected(self.f.envelope(grant))
                self.assertFalse(self.f.store.exists())

    def test_expired_future_and_overlong_new_grants_fail_closed(self):
        for start, end in ((-400, -100), (120, 300), (0, 301)):
            with self.subTest(start=start, end=end):
                grant = self.f.grant("submit")
                now = grant["issued_at"]
                grant.update(issued_at=now+start, expires_at=now+end)
                rejected = self.rejected(self.f.envelope(grant))
                self.assertRegex(rejected["error"], "expired|not yet valid|validity interval")
                self.assertFalse(self.f.store.exists())

    def test_restricted_execute_policy_never_becomes_remote_full_access(self):
        grant = self.f.grant("submit")
        grant["context"].update(cwd=str(self.f.cwd), sandbox_policy={
            "type":"workspace-write", "writable_roots":[], "network_access":False,
            "exclude_tmpdir_env_var":True, "exclude_slash_tmp":True})
        rejected = self.rejected(self.f.envelope(grant))
        self.assertIn("requires fresh full-access/approval-never", rejected["error"])
        self.assertFalse(self.f.store.exists())

    def test_signed_argv_cannot_override_endpoint_cwd_or_store(self):
        for option in ("--cwd", "--store"):
            with self.subTest(option=option):
                grant = self.f.grant("submit")
                grant["args"] += [option, str(self.f.root / "outside-authorized-endpoint")]
                rejected = self.rejected(self.f.envelope(grant))
                self.assertIn(option, rejected["error"])
                self.assertFalse(self.f.store.exists())

    def test_owner_key_endpoint_and_request_binding_protect_retained_read(self):
        _, request, run = self.f.retained()
        self.assertIs(self.reply(self.f.envelope(self.f.grant()))["ok"], True)
        before = self.snapshot()
        other_key = self.f.generate_key("other-root-key")
        cases = []
        other_owner = self.f.grant(issuer="other-root")
        cases.append((other_owner, other_key, "exact root principal"))
        # Keep each candidate enrolled to ONE root. Make the alternate store's
        # record readable so endpoint mismatch, not a missing file, denies it.
        other_store = self.f.root / "another-store"
        other_request = other_store / ".remote-v2" / "request-1"
        other_request.mkdir(parents=True, mode=0o700)
        other_store.chmod(0o700)
        other_request.parent.chmod(0o700)
        (other_request / "request.json").write_bytes((request / "request.json").read_bytes())
        (other_request / "request.json").chmod(0o600)
        for field, value in (("host", "another-alias"), ("store", str(self.f.root / "another-store"))):
            grant = self.f.grant()
            grant["endpoint"][field] = value
            cases.append((grant, self.f.key, "unavailable to this verified root/endpoint"))
        for grant, key, error in cases:
            with self.subTest(issuer=grant["issuer"], endpoint=grant["endpoint"], request=grant["request_id"]):
                rejected = self.rejected(self.f.envelope(grant, key))
                self.assertIn(error, rejected["error"])
                self.assertEqual(self.snapshot(), before)

        def store_state():
            return {str(path.relative_to(self.f.store)):
                    (path.stat().st_mode, path.stat().st_mtime_ns,
                     None if path.is_dir() else path.read_bytes())
                    for path in (self.f.store, *self.f.store.rglob("*"))}

        absent_grant = self.f.grant(request_id="another-request")
        before_absence = store_state()
        wrong_owner = self.f.grant(request_id="another-request", issuer="other-root")
        wrong_endpoint = copy.deepcopy(absent_grant)
        wrong_endpoint["endpoint"]["endpoint_id"] = "c" * 64
        for grant, key, error in (
                (wrong_owner, self.f.key, "exact root principal"),
                (absent_grant, other_key, "signature"),
                (wrong_endpoint, self.f.key, "endpoint identity")):
            with self.subTest(absent_request=True, issuer=grant["issuer"], error=error):
                rejected = self.rejected(self.f.envelope(grant, key))
                self.assertIn(error, rejected["error"])
                self.assertNotIn("result", rejected)
                self.assertEqual(store_state(), before_absence)

        absent = self.reply(self.f.envelope(absent_grant))
        self.assertIs(absent["ok"], True)
        self.assertIs(absent["result"]["accepted"], False)
        # Exact shape excludes retained manifest/content and any run allocation.
        self.assertEqual(absent, {"version":2, "ok":True, "result":{
            "request_id":"another-request", "accepted":False, "status":"not_found"}})
        self.assertEqual(store_state(), before_absence)
        # The unsigned ledger pointer cannot redirect a valid signed submission
        # to a different run, even inside the selected store.
        alternate_run = run.parent / "another-run"
        alternate_run.mkdir(mode=0o700)
        record_path = request / "request.json"
        record = json.loads(record_path.read_text())
        record["run"] = str(alternate_run)
        record_path.write_text(json.dumps(record), encoding="utf-8")
        rejected = self.rejected(self.f.envelope(self.f.grant()))
        self.assertIn("retained run escapes signed endpoint store", rejected["error"])

    def test_replay_of_accepted_claim_never_restarts_absent_supervisor(self):
        envelope, request, run = self.f.retained()
        before = self.snapshot()
        value = self.reply(envelope)
        self.assertIs(value["ok"], True, value)
        self.assertEqual(value["result"]["run_id"], run.name)
        self.assertEqual(value["result"]["supervisor"], "unknown")
        self.assertIsNone(value["result"]["operation_result"])
        self.assertIs(value["result"]["accounting_may_be_incomplete"], True)
        self.assertFalse((request / "executing.json").exists())
        self.assertEqual(self.snapshot(), before)

    def test_crashed_execution_reconciles_unknown_without_automatic_replay(self):
        envelope, _, _ = self.f.retained(executing=True)
        before = self.snapshot()
        value = self.reply(envelope)
        self.assertIs(value["ok"], True, value)
        self.assertEqual(value["result"]["supervisor"], "unknown")
        self.assertIsNone(value["result"]["operation_result"])
        self.assertIs(value["result"]["accounting_may_be_incomplete"], True)
        self.assertEqual(self.snapshot(), before)

    def test_same_request_with_changed_payload_rejected_without_mutation(self):
        envelope, _, _ = self.f.retained()
        before = self.snapshot()
        grant = copy.deepcopy(envelope["grant"])
        grant["args"] += ["--inject-write"]
        rejected = self.rejected(self.f.envelope(grant))
        self.assertIn("different signed payload", rejected["error"])
        self.assertEqual(self.snapshot(), before)

    def test_truncated_request_record_is_not_replaced_or_replayed(self):
        envelope, request, _ = self.f.retained()
        (request / "request.json").write_text('{"envelope":', encoding="utf-8")
        before = self.snapshot()
        self.rejected(envelope)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse((request / "executing.json").exists())

    def test_partial_claim_without_request_is_not_filled_in_on_retry(self):
        envelope, request, _ = self.f.retained()
        (request / "request.json").unlink()
        before = self.snapshot()
        self.rejected(envelope)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse((request / "executing.json").exists())

    def test_claimed_control_without_result_reconciles_unknown_without_effect(self):
        _, request, _ = self.f.retained()
        grant = self.f.grant("interrupt", operation_id="stop-1",
                             target={"revision":1, "subtree_epoch":0,
                                     "control_epoch":"old-epoch", "turn_id":"old-turn"})
        envelope = self.f.envelope(grant)
        operation = request / "controls" / "stop-1"
        operation.mkdir(parents=True, mode=0o700)
        operation.parent.chmod(0o700)
        record = operation / "request.json"
        record.write_text(json.dumps(envelope), encoding="utf-8")
        record.chmod(0o600)
        before = self.snapshot()
        value = self.reply(envelope)
        self.assertIs(value["ok"], True, value)
        self.assertEqual(value["result"]["result"]["status"], "unknown")
        self.assertIs(value["result"]["result"]["effect_reissued"], False)
        self.assertEqual(self.snapshot(), before)

    def test_partial_control_claim_cannot_authorize_a_second_effect(self):
        _, request, _ = self.f.retained()
        operation = request / "controls" / "stop-1"
        operation.mkdir(parents=True, mode=0o700)
        operation.parent.chmod(0o700)
        grant = self.f.grant("interrupt", operation_id="stop-1",
                             target={"revision":1, "subtree_epoch":0,
                                     "control_epoch":"old-epoch", "turn_id":"old-turn"})
        before = self.snapshot()
        self.rejected(self.f.envelope(grant))
        self.assertEqual(self.snapshot(), before)
        self.assertFalse((operation / "executing.json").exists())

    def test_supervisor_independently_rejects_forged_persisted_context(self):
        _, request, _ = self.f.retained()
        path = request / "request.json"
        record = json.loads(path.read_text())
        record["envelope"]["grant"]["context"]["caller_thread_id"] = "forged-root"
        path.write_text(json.dumps(record), encoding="utf-8")
        result = subprocess.run([str(self.f.server), "__remote-supervise", str(request)],
                                env=self.f.env, capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signature", result.stderr)
        self.assertFalse((request / "executing.json").exists())

    def test_live_supervisor_lock_is_observed_without_restarting_it(self):
        import fcntl
        envelope, request, _ = self.f.retained(executing=True)
        path = request / "supervisor.lock"
        with path.open("w") as owner:
            path.chmod(0o600)
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
            value = self.reply(envelope)
            self.assertIs(value["ok"], True, value)
            self.assertEqual(value["result"]["supervisor"], "live")
            self.assertIsNone(value["result"]["operation_result"])

    def test_expired_original_acceptance_may_reconcile_but_not_create_new_run(self):
        grant = self.f.grant("submit")
        grant.update(issued_at=grant["issued_at"]-600, expires_at=grant["expires_at"]-600)
        envelope, _, run = self.f.retained(grant)
        before = self.snapshot()
        value = self.reply(envelope)
        self.assertIs(value["ok"], True, value)
        self.assertEqual(value["result"]["run_id"], run.name)
        self.assertEqual(self.snapshot(), before)


if __name__ == "__main__":
    unittest.main()
