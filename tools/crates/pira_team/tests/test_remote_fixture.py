"""Isolated transport helpers, not proof of real SSH or caller authentication.

All envelopes/keys recorded here are generated synthetic test data. Never point
these helpers at a live controller state directory, credential home or SSH host.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

import test_team


def isolate_environment(environment, root):
    """Retain fixture settings, not caller credentials, profiles or proxy settings."""
    names = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "CODEX_HOME", "CODEX_THREAD_ID",
             "PIRA_CTX_STORE_DIR", "PIRA_DEC_STORE_DIR", "PIRA_TEAM_BIN", "PIRA_TEAM_TEST_CODEX",
             "TEAM_TEST_BACKEND_CONTRACT", "TEAM_TEST_BACKEND_FIXTURE", "TEAM_SOCKET_DIR",
             "TEAM_SUBTEAM_BIN", "TEAM_SUBTEAM_FIXTURE", "TEAM_FIXTURE_PYTHON", "TEAM_FIXTURE_CODEX"}
    clean = {key:value for key, value in environment.items() if key.upper() in names}
    home = root / "isolated-user-home"
    home.mkdir(exist_ok=True)
    clean.update(HOME=str(home), USERPROFILE=str(home), LOCALAPPDATA=str(home),
                 XDG_DATA_HOME=str(home), XDG_CONFIG_HOME=str(home), PYTHONDONTWRITEBYTECODE="1")
    environment.clear()
    environment.update(clean)


FAKE_SSH = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

wire = sys.stdin.read()
trace = Path(os.environ["TEAM_REMOTE_TRACE"])
with trace.open("a", encoding="utf-8") as out:
    out.write(json.dumps({"argv":sys.argv[1:], "wire":wire}) + "\n")
mode = os.environ.get("TEAM_REMOTE_TRANSPORT", "reply")
if mode.startswith("proxy"):
    # Parse the command exactly as a POSIX shell would tokenize it, but never
    # evaluate substitutions or shell operators in this offline test fixture.
    command = shlex.split(sys.argv[-1])
    assert command.pop(0) == "exec", command
    result = subprocess.run(command, input=wire, capture_output=True, text=True,
                            encoding="utf-8", timeout=30)
    accepted = False
    if result.returncode == 0:
        try:
            accepted = json.loads(result.stdout).get("ok") is True
        except json.JSONDecodeError:
            pass
    # Lose acknowledgements only after endpoint acceptance. Rejections remain
    # visible; a broken fixture must not masquerade as an uncertain submission.
    if accepted and mode == "proxy-drop-control-once" and json.loads(wire).get("grant", {}).get("op") in ("steer", "interrupt"):
        marker = trace.with_suffix(".control-dropped")
        if not marker.exists():
            marker.touch()
            sys.exit(255)
    if accepted and mode == "proxy-drop-submit-once" and json.loads(wire).get("grant", {}).get("op") == "submit":
        marker = trace.with_suffix(".submit-dropped")
        if not marker.exists():
            marker.touch()
            sys.exit(255)
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    sys.exit(result.returncode)
if mode == "drop-once" and json.loads(wire).get("grant", {}).get("op") == "submit":
    marker = trace.with_suffix(".dropped")
    if not marker.exists():
        marker.touch()
        sys.exit(255)
if mode == "invalid-json":
    print("not a protocol response")
    sys.exit(0)
reply = json.loads(Path(os.environ["TEAM_REMOTE_REPLY"]).read_text(encoding="utf-8"))
grant = json.loads(wire)["grant"]
if reply.get("version") == 2 and reply.get("ok") is True:
    if grant["op"] == "read" and grant["args"] == ["capabilities"]:
        reply["result"] = {name:reply["result"].get(name) for name in ("protocol", "scope", "capabilities")}
    elif grant["op"] == "read" and grant["operation_id"] is not None:
        controls = trace.with_suffix(".controls.json")
        retained = json.loads(controls.read_text()) if controls.exists() else {}
        reply["result"] = retained.get(grant["operation_id"],
                                       {"operation_id":grant["operation_id"], "status":"not_found"})
    elif grant["op"] in ("interrupt", "steer"):
        controls = trace.with_suffix(".controls.json")
        retained = json.loads(controls.read_text()) if controls.exists() else {}
        intent = {name:value for name, value in grant.items()
                  if name not in ("issued_at", "expires_at", "nonce", "target")}
        result = {"operation_id":grant["operation_id"], "target":grant["target"], "intent":intent,
                  "result":{"status":"acknowledged", "completion_confirmed":False}}
        if grant["operation_id"] in retained:
            result = retained[grant["operation_id"]]
            if result["intent"] != intent or result["target"] != grant["target"]:
                reply = {"version":2, "ok":False, "error":"fixture control intent/epoch mismatch"}
        else:
            retained[grant["operation_id"]] = result
            controls.write_text(json.dumps(retained), encoding="utf-8")
        if reply["ok"] is True:
            reply["result"] = result
    elif mode == "retained-topology" and grant["op"] in ("submit", "read"):
        # PIRA: this mode models one ordinary run and explicit resume promotion,
        # not backend execution. Server state is separate from controller intent.
        topology_path = trace.with_suffix(".topology.json")
        topology = json.loads(topology_path.read_text()) if topology_path.exists() else None
        if grant["op"] == "submit":
            if grant["args"][0] == "run":
                assert "--allow-subteam" not in grant["args"]
                run_id = "remote-"+grant["nonce"]
                topology = {"request_id":grant["request_id"], "run_id":run_id, "accepted":True,
                            "supervisor":"unknown", "operation_result":{"ok":True},
                            "manifest":{"run_id":run_id, "creator_main_session_id":grant["issuer"],
                                        "parent_run_id":None, "allow_subteam":False,
                                        "revision":1, "status":"completed"}}
            else:
                assert topology is not None and grant["args"][0] == "resume"
                assert grant["run_request"] == topology["request_id"]
                assert grant["issuer"] == topology["manifest"]["creator_main_session_id"]
                if grant["args"][1:] == ["--allow-subteam"]:
                    topology["manifest"]["allow_subteam"] = True
                topology["manifest"]["revision"] += 1
            topology_path.write_text(json.dumps(topology), encoding="utf-8")
        else:
            assert topology is not None and grant["request_id"] == topology["request_id"]
        reply["result"] = {**topology, "request_id":grant["request_id"]}
print(json.dumps(reply))
'''


class RemoteFixture:
    """Composition avoids rediscovering the existing TeamTests suite."""

    def __init__(self, case):
        self.local = test_team.TeamTests()
        case.addCleanup(self.local.doCleanups)
        self.local.setUp()
        self.root = self.local.root
        self.bin = self.local.bin
        self.env = self.local.env
        isolate_environment(self.env, self.root)
        self.local.parent([{"sandbox_policy":{"type":"danger-full-access"}}])
        backend = self.root / "codex"
        source = backend.read_text(encoding="utf-8")
        # Existing fake_codex implements app-server, not the availability probe.
        # This is explicitly a synthetic backend version, never real CLI evidence.
        prelude = ('import sys\nif sys.argv[1:] == ["--version"]:\n'
                   '    print("codex-cli synthetic-remote-fixture")\n    raise SystemExit(0)\n')
        first, rest = source.split("\n", 1)
        backend.write_text(first+"\n"+prelude+rest, encoding="utf-8")
        self.trace = self.root / "ssh-trace.jsonl"
        self.reply = self.root / "ssh-reply.json"
        self.state = self.root / "controller"
        self.store = self.root / "remote-store"
        self.cwd = self.root / "remote-workspace"
        self.cwd.mkdir()
        self.env.update(TEAM_REMOTE_TRACE=str(self.trace), TEAM_REMOTE_REPLY=str(self.reply))
        fake = self.root / "ssh"
        fake.write_text(FAKE_SSH.replace("#!/usr/bin/env python3", f"#!{sys.executable}"), encoding="utf-8")
        fake.chmod(0o700)

    def respond(self, result, *, version=2, ok=True, error="fixture error"):
        value = {"version":version, "ok":ok}
        value["result" if ok else "error"] = result if ok else error
        self.reply.write_text(json.dumps(value) + "\n", encoding="utf-8")

    def invoke(self, *args, timeout=30):
        return subprocess.run([str(self.bin), *map(str,args)], env=self.env,
                              capture_output=True, text=True, encoding="utf-8", timeout=timeout)

    def exchanges(self):
        return [json.loads(line) for line in self.trace.read_text(encoding="utf-8").splitlines()] if self.trace.exists() else []

    def endpoint(self, request):
        return subprocess.run([str(self.bin), "__remote-serve"], input=json.dumps(request),
                              env=self.env, capture_output=True, text=True, encoding="utf-8", timeout=30)

    def generate_key(self, name):
        """Only ssh-keygen accesses generated private bytes; public output is fixture data."""
        key = self.root / name
        result = subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
                                env=self.env, capture_output=True, text=True, encoding="utf-8", timeout=10)
        if result.returncode:
            raise AssertionError(f"synthetic key generation failed: {result.stderr}")
        return key

    def sign(self, payload, key, namespace):
        canonical = json.dumps(payload, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
        result = subprocess.run(["ssh-keygen", "-Y", "sign", "-n", namespace, "-f", str(key)],
                                env=self.env, input=canonical, capture_output=True, text=True, encoding="utf-8", timeout=10)
        if result.returncode:
            raise AssertionError(f"synthetic signing failed: {result.stderr}")
        return result.stdout

    def client_options(self, request_id="request-1", **overrides):
        values = {"host":"fixture-alias", "remote-user":"fixture-user",
                  "remote-endpoint-id":"b"*64,
                  "remote-exe":"/opt/pira/pira_team", "remote-backend":"/opt/codex/codex",
                  "remote-store":str(self.store), "remote-state":str(self.state),
                  "authority-key":str(self.root / "authority"), "request-id":request_id}
        values.update(overrides)
        return [part for name, value in values.items() for part in ("--"+name, str(value))]

    def keygen(self):
        return self.invoke("remote-keygen", "--authority-key", self.root / "authority")

    def completed_reply(self):
        self.respond({"protocol":2, "scope":"root-main-host-pinned",
                     "capabilities":["durable-submit", "read", "resume", "steer", "interrupt",
                                     "signed-delegation", "host-pinned-subteam"],
                     "request_id":"request-1", "run_id":"fixture-run", "accepted":True,
                     "supervisor":"unknown", "operation_result":{"ok":True},
                     "control_target":{"revision":1, "subtree_epoch":0,
                                       "control_epoch":"fixture-epoch", "turn_id":"fixture-turn"}})

    def grants(self, op=None):
        grants = [json.loads(item["wire"])["grant"] for item in self.exchanges()]
        return [grant for grant in grants if op is None or grant["op"] == op]


class SignedEndpointFixture(RemoteFixture):
    """Generated authority + copied endpoint; no real root identity is asserted."""

    issuer = "fixture-root"
    namespace = "pira-team-remote-v2"

    def __init__(self, case, *, issuer="fixture-root"):
        super().__init__(case)
        self.issuer = issuer
        import pwd
        server_dir = self.root / "server"
        server_dir.mkdir(mode=0o700)
        self.server = server_dir / "pira_team"
        shutil.copy2(self.bin, self.server)
        self.key = self.generate_key("synthetic-authority")
        self.anchor = Path(str(self.server)+".allowed-signers")
        self.enroll(self.issuer, self.key)
        self.endpoint_binding = {"host":"fixture-alias", "user":pwd.getpwuid(os.geteuid()).pw_name,
                                 "endpoint_id":"b"*64,
                                 "exe":str(self.server), "backend":str(self.root / "codex"),
                                 "store":str(self.store)}
        self.descriptor = Path(str(self.server)+".remote-endpoint.json")
        self.descriptor.write_text(json.dumps({"version":2, "endpoint_id":"b"*64,
                                   "user":self.endpoint_binding["user"], "exe":str(self.server)}), encoding="utf-8")
        self.descriptor.chmod(0o600)
        self.context = {"caller_thread_id":self.issuer, "approval_policy":"never",
                        "sandbox_policy":{"type":"danger-full-access"}, "permission_profile":None,
                        "file_system_sandbox_policy":None, "model":"test-model", "effort":"high"}

    def enroll(self, issuer, key):
        public = Path(str(key)+".pub").read_text(encoding="utf-8").strip()
        line = f'{issuer} namespaces="{self.namespace}" {public}\n'
        with self.anchor.open("w", encoding="utf-8") as out:
            out.write(line)
        self.anchor.chmod(0o600)

    def grant(self, op="read", request_id="request-1", **changes):
        now = int(time.time())
        value = {"protocol":2, "scope":"root-main-host-pinned", "issuer":self.issuer,
                 "endpoint":self.endpoint_binding.copy(), "op":op, "request_id":request_id,
                 "run_request":None, "args":[op], "context":None, "cwd":None,
                 "issued_at":now, "expires_at":now+300, "nonce":"a"*64,
                 "file":None, "artifact":False, "operation_id":None, "target":None}
        if op in ("submit", "steer", "probe"):
            value["context"] = self.context.copy()
        if op == "submit":
            value.update(args=["run", "--model", "test-model", "--effort", "high",
                               "--task", "harmless fixture", "--completion-gate", "Deliver fixture handoff"],
                         cwd=str(self.cwd))
        value.update(changes)
        return value

    def envelope(self, grant, key=None):
        return {"version":2, "grant":grant,
                "signature":self.sign(grant, key or self.key, self.namespace)}

    def endpoint(self, envelope):
        return subprocess.run([str(self.server), "__remote-serve"], input=json.dumps(envelope),
                              env=self.env, capture_output=True, text=True, encoding="utf-8", timeout=30)

    def retained(self, grant=None, *, executing=False):
        """Construct a durable accepted/no-owner fixture, never launch a supervisor."""
        grant = grant or self.grant("submit")
        run = self.store / ("remote-"+grant["nonce"])
        run.mkdir(mode=0o700, parents=True)
        self.store.chmod(0o700)
        ledger = self.store / ".remote-v2"
        ledger.mkdir(mode=0o700)
        request = ledger / grant["request_id"]
        request.mkdir(mode=0o700)
        envelope = self.envelope(grant)
        records = {"request.json":{"envelope":envelope, "run":str(run),
                                    "accepted_at":grant["issued_at"]}, "claimed.json":{"claimed":True}}
        if executing:
            records["executing.json"] = {"started":True}
        for name, value in records.items():
            path = request / name
            path.write_text(json.dumps(value), encoding="utf-8")
            path.chmod(0o600)
        manifest = run / "manifest.json"
        manifest.write_text(json.dumps({"status":"running", "usage_complete":False,
                                        "run_id":run.name, "revision":1, "subtree_epoch":0,
                                        "allow_subteam":False,
                                        "creator_main_session_id":grant["issuer"],
                                        "parent_run_id":None}), encoding="utf-8")
        manifest.chmod(0o600)
        return envelope, request, run


def wait_for_file(path, predicate=lambda path: path.exists(), timeout=15):
    """Linux filesystem events, not sleeps or status polling; parent must exist."""
    import ctypes
    import select
    libc = ctypes.CDLL(None, use_errno=True)
    libc.inotify_init1.argtypes = [ctypes.c_int]
    libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    fd = libc.inotify_init1(os.O_CLOEXEC | os.O_NONBLOCK)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1 failed")
    try:
        # MODIFY/CLOSE_WRITE/CREATE/MOVED_TO cover append logs and atomic records.
        if libc.inotify_add_watch(fd, os.fsencode(path.parent), 0x2 | 0x8 | 0x100 | 0x80) < 0:
            raise OSError(ctypes.get_errno(), "inotify_add_watch failed")
        deadline = time.monotonic()+timeout
        while not predicate(path):
            remaining = deadline-time.monotonic()
            if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
                raise AssertionError(f"fixture file event timeout: {path}")
            os.read(fd, 65536)
    finally:
        os.close(fd)
