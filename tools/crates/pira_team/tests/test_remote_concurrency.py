"""Offline client persistence concurrency; fake SSH is not server dedup evidence.

Run CLI cases from legitimate MAIN ancestry with a fresh combined PIRA_TEAM_BIN.
Only generated synthetic grants/public keys are inspected, never private keys.
"""
from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import time
import unittest

from test_remote_fixture import RemoteFixture


SIGNER = r'''#!PYTHON
import json
import os
import socket
import subprocess
import sys

real = REAL_KEYGEN
args = sys.argv[1:]
if args[:2] != ["-Y", "sign"]:
    os.execv(real, [real, *args])
data = sys.stdin.buffer.read(1048577)
assert len(data) <= 1048576, "synthetic grant exceeds fixture limit"
grant = json.loads(data)
if grant["op"] == "submit":
    with socket.create_connection(tuple(json.loads(os.environ["TEAM_SIGN_GATE"])), timeout=15) as client:
        client.settimeout(15)
        with client.makefile("rwb") as stream:
            stream.write(json.dumps({"op":grant["op"], "request_id":grant["request_id"]}).encode() + b"\n")
            stream.flush()
            assert stream.readline(4) == b"go\n", "signing gate did not release"
# Preserve exact grant bytes, real OpenSSH crypto, arguments and output streams.
result = subprocess.run([real, *args], input=data, timeout=10)
sys.exit(result.returncode)
'''


class SigningGate:
    def __init__(self, case, fixture):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(8)
        self.held = []
        case.addCleanup(self.release)
        real = shutil.which("ssh-keygen", path=fixture.env["PATH"])
        if real is None:
            raise AssertionError("OpenSSH ssh-keygen required for synthetic signing fixture")
        self.real = real
        directory = fixture.root / "signer-wrapper"
        directory.mkdir()
        self.program = directory / "ssh-keygen"
        self.program.write_text(SIGNER.replace("#!PYTHON", "#!" + sys.executable)
                                .replace("REAL_KEYGEN", repr(real)), encoding="utf-8")
        self.program.chmod(0o700)
        fixture.env.update(PATH=str(directory) + os.pathsep + fixture.env["PATH"],
                           TEAM_SIGN_GATE=json.dumps(self.listener.getsockname()))

    def hold(self, count):
        """Return only after every signer is held, before releasing any signer."""
        deadline = time.monotonic() + 10
        arrived = []
        for _ in range(count):
            self.listener.settimeout(max(0.001, deadline - time.monotonic()))
            try:
                connection, _ = self.listener.accept()
            except socket.timeout as error:
                raise AssertionError(f"signing gate arrivals {arrived}; expected {count}") from error
            self.held.append(connection)
            connection.settimeout(max(0.001, deadline - time.monotonic()))
            with connection.makefile("rb") as stream:
                notice = json.loads(stream.readline(4096))
            if notice["op"] != "submit":
                raise AssertionError(f"unexpected signing notice: {notice}")
            arrived.append(notice["request_id"])
        return arrived

    def release(self):
        for connection in self.held:
            try:
                connection.sendall(b"go\n")
            except OSError:
                pass  # A failed fixture client may already have exited.
            finally:
                connection.close()
        self.held.clear()
        self.listener.close()


def verify_signature(fixture, real, envelope):
    """Verify synthetic envelope against generated public data using real crypto."""
    grant = envelope["grant"]
    anchor = fixture.root / "verify-allowed-signers"
    public = (fixture.root / "authority.pub").read_text(encoding="utf-8").strip()
    anchor.write_text(f'{grant["issuer"]} namespaces="pira-team-remote-v2" {public}\n', encoding="utf-8")
    signature = fixture.root / "verify-signature"
    signature.write_text(envelope["signature"], encoding="utf-8")
    data = json.dumps(grant, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    result = subprocess.run([real, "-Y", "verify", "-f", str(anchor), "-I", grant["issuer"],
                             "-n", "pira-team-remote-v2", "-s", str(signature)],
                            input=data, env=fixture.env, capture_output=True, text=True, timeout=10)
    return result


@unittest.skipUnless(os.name == "posix", "POSIX fake SSH and signing wrapper")
class RemoteConcurrencyTests(unittest.TestCase):
    def setUp(self):
        self.f = RemoteFixture(self)
        self.processes = []
        self.addCleanup(self.stop_processes)
        result = self.f.keygen()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.f.completed_reply()
        self.gate = SigningGate(self, self.f)

    def stop_processes(self):
        for process in self.processes:
            if process.poll() is None:
                process.terminate()
            try:
                process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate(timeout=5)

    def start(self, request_id, task):
        options = self.f.client_options(request_id, **{"remote-state": self.f.state})
        process = subprocess.Popen([str(self.f.bin), "run", *options, "--remote-cwd", str(self.f.cwd),
                                    "--detach", "--model", "test-model", "--effort", "high",
                                    "--completion-gate", "Report fixture result", "--task", task],
                                   env=self.f.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, encoding="utf-8")
        self.processes.append(process)
        return process

    def result(self, process):
        stdout, stderr = process.communicate(timeout=20)
        return process.returncode, stdout, stderr

    def submissions(self):
        return [exchange for exchange in self.f.exchanges()
                if json.loads(exchange["wire"])["grant"]["op"] == "submit"]

    def test_distinct_ids_share_controller_and_hold_four_signing_sections(self):
        ids = [f"concurrent-{index}" for index in range(4)]
        processes = [self.start(request_id, "task " + request_id) for request_id in ids]
        try:
            arrived = self.gate.hold(4)
        except AssertionError as error:
            self.gate.release()
            self.fail(f"{error}; CLI outcomes: {[self.result(process) for process in processes]}")
        self.assertCountEqual(arrived, ids)
        self.assertEqual(len(set(arrived)), 4)
        self.assertEqual(self.submissions(), [], "all signers must still be inside persistence sections")
        self.gate.release()
        for process in processes:
            result = self.result(process)
            self.assertEqual(result[0], 0, result)
        submissions = self.submissions()
        self.assertEqual(len(submissions), 4)
        envelopes = [json.loads(exchange["wire"]) for exchange in submissions]
        self.assertCountEqual([envelope["grant"]["request_id"] for envelope in envelopes], ids)
        self.assertEqual(len({envelope["grant"]["nonce"] for envelope in envelopes}), 4)
        for envelope in envelopes:
            grant = envelope["grant"]
            self.assertIn("task " + grant["request_id"], grant["args"])
            retained = json.loads((self.f.state / (grant["request_id"] + ".json")).read_text(encoding="utf-8"))
            self.assertEqual(retained, envelope)
            verified = verify_signature(self.f, self.gate.real, envelope)
            self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_same_id_conflict_rejected_then_identical_retry_reuses_envelope(self):
        request_id = "same-request"
        self.f.env["TEAM_REMOTE_TRANSPORT"] = "drop-once"
        first = self.start(request_id, "original task")
        self.assertEqual(self.gate.hold(1), [request_id])
        before = self.f.exchanges()
        conflicting = self.start(request_id, "changed task")
        result = self.result(conflicting)
        self.assertNotEqual(result[0], 0, result)
        self.assertIn("live owner", result[2])
        self.assertEqual(self.f.exchanges(), before, "same-ID contender must fail before transport")
        self.gate.release()
        result = self.result(first)
        self.assertNotEqual(result[0], 0, result)
        self.assertIn("unknown", result[2].lower())
        original = self.submissions()
        self.assertEqual(len(original), 1)
        path = self.f.state / (request_id + ".json")
        snapshot = path.read_bytes(), path.stat().st_mtime_ns
        before = self.f.exchanges()
        result = self.result(self.start(request_id, "changed task"))
        self.assertNotEqual(result[0], 0, result)
        self.assertIn("already binds different", result[2])
        self.assertEqual(self.f.exchanges(), before, "persisted conflict must fail before transport")
        result = self.result(self.start(request_id, "original task"))
        self.assertEqual(result[0], 0, result)
        submissions = self.submissions()
        self.assertEqual(len(submissions), 2)
        self.assertEqual(submissions[0]["wire"], submissions[1]["wire"],
                         "retry must retain request, nonce and signature, not re-sign")
        envelope = json.loads(submissions[0]["wire"])
        self.assertEqual(json.loads(snapshot[0]), envelope)
        self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns), snapshot)
        verified = verify_signature(self.f, self.gate.real, envelope)
        self.assertEqual(verified.returncode, 0, verified.stderr)


@unittest.skipUnless(os.name == "posix", "POSIX fake SSH and signing wrapper")
class SigningHarnessTests(unittest.TestCase):
    def setUp(self):
        self.f = RemoteFixture(self)
        self.f.generate_key("authority")  # Direct real keygen, no root CLI needed.
        self.f.completed_reply()
        self.gate = SigningGate(self, self.f)

    def test_gate_preserves_real_signature_and_fake_transport_bytes(self):
        grant = {"op":"submit", "request_id":"wrapper-check", "issuer":"synthetic-owner", "args":["λ task"]}
        data = json.dumps(grant, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
        process = subprocess.Popen([str(self.gate.program), "-Y", "sign", "-f", str(self.f.root / "authority"),
                                    "-n", "pira-team-remote-v2"], env=self.f.env,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        def cleanup():
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)
        self.addCleanup(cleanup)
        process.stdin.write(data.encode())
        process.stdin.close()
        process.stdin = None
        self.assertEqual(self.gate.hold(1), ["wrapper-check"])
        self.assertIsNone(process.poll(), "real signing cannot finish before release")
        self.gate.release()
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(process.returncode, 0, stderr)
        envelope = {"version":2, "grant":grant, "signature":stdout.decode()}
        verified = verify_signature(self.f, self.gate.real, envelope)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        wire = json.dumps(envelope)
        delivered = subprocess.run([str(self.f.root / "ssh"), "fixture-host", "exec fixture __remote-serve"],
                                   input=wire, env=self.f.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(delivered.returncode, 0, delivered.stderr)
        self.assertTrue(json.loads(delivered.stdout)["ok"])
        self.assertEqual(self.f.exchanges()[0]["wire"], wire)
        envelope["grant"] = {**grant, "args":["changed task"]}
        self.assertNotEqual(verify_signature(self.f, self.gate.real, envelope).returncode, 0)

    def test_non_submit_signing_passes_without_gate(self):
        grant = {"op":"read", "request_id":"read-check", "issuer":"synthetic-owner"}
        signature = self.f.sign(grant, self.f.root / "authority", "pira-team-remote-v2")
        verified = verify_signature(self.f, self.gate.real, {"grant":grant, "signature":signature})
        self.assertEqual(verified.returncode, 0, verified.stderr)


if __name__ == "__main__":
    unittest.main()
