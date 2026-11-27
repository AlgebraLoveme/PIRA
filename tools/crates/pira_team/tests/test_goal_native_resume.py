"""Opt-in real Codex cold resume against a scripted loopback provider, no inference."""
import gzip
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import unittest

from team_fixture import TeamFixture


class GoalResponses:
    def __enter__(self):
        self.captured = queue.Queue()
        self.releases = {}
        self.errors = []
        self.count = 0
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    assert self.path == "/v1/responses"
                    assert not self.headers.get("Authorization")
                    size = int(self.headers.get("Content-Length", "0"))
                    assert 0 < size <= 4 * 1024 * 1024
                    data = self.rfile.read(size)
                    encoding = self.headers.get("Content-Encoding", "identity")
                    if encoding == "gzip":
                        data = gzip.decompress(data)
                    else:
                        assert encoding == "identity"
                    body = json.loads(data)
                    assert body["model"] == "test-model" and body["stream"] is True
                    owner.count += 1
                    number = owner.count
                    if number >= 3:
                        release = threading.Event()
                        owner.releases[number] = release
                        owner.captured.put((number, body))
                        assert release.wait(30), "native test did not release held request"
                        # The test interrupts BEFORE release; no extra response or
                        # inference may complete the goal or mutate its objective.
                        self.send_error(400, "offline probe already interrupted")
                        return
                    if number == 1:
                        assert any(t.get("name") == "create_goal" for t in body["tools"])
                        item = {"id": "fc_goal", "type": "function_call", "call_id": "call_goal",
                                "name": "create_goal", "arguments": json.dumps({"objective": "Synthetic GRH proof"})}
                        events = [("response.output_item.added", {"output_index": 0, "item": dict(item, arguments="")}),
                                  ("response.function_call_arguments.delta", {"item_id": item["id"], "output_index": 0, "delta": item["arguments"]}),
                                  ("response.function_call_arguments.done", {"item_id": item["id"], "output_index": 0, "arguments": item["arguments"]}),
                                  ("response.output_item.done", {"output_index": 0, "item": item})]
                    else:
                        text = json.dumps({"status": "incomplete", "format": "markdown"})
                        part = {"type": "output_text", "text": text, "annotations": []}
                        item = {"id": "msg_checkpoint", "type": "message", "role": "assistant",
                                "status": "completed", "phase": "final_answer", "content": [part]}
                        events = [("response.output_item.added", {"output_index": 0, "item": dict(item, content=[])}),
                                  ("response.output_item.done", {"output_index": 0, "item": item})]
                    response = {"id": "resp_goal_" + str(number), "object": "response", "created_at": 1,
                                "status": "completed", "model": "test-model", "output": [item],
                                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
                    events = [("response.created", {"response": dict(response, status="in_progress", output=[])})] + events
                    events.append(("response.completed", {"response": response}))
                    payload = "".join("event: " + name + "\ndata: " + json.dumps(
                        dict(event, type=name, sequence_number=i)) + "\n\n"
                        for i, (name, event) in enumerate(events)).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected after the held native turn is interrupted.
                except Exception as error:
                    owner.errors.append(repr(error))
                    self.send_error(400)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = "http://127.0.0.1:" + str(self.server.server_address[1]) + "/v1"
        return self

    def __exit__(self, *_):
        for release in list(self.releases.values()):
            release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for part in value.values():
            yield from strings(part)
    elif isinstance(value, list):
        for part in value:
            yield from strings(part)


class NativeGoalResumeTests(unittest.TestCase):
    def test_old_team_cold_resume_delivers_current_contract_before_native_admission(self):
        f = TeamFixture(self.addCleanup)
        user = f.root / "user"
        user.mkdir()
        f.env.update(HOME=str(user), USERPROFILE=str(user), ZDOTDIR=str(user),
                     XDG_CONFIG_HOME=str(user), XDG_DATA_HOME=str(user), XDG_CACHE_HOME=str(user),
                     TEAM_REAL_COORDINATOR_CODEX=str(Path(os.environ["PIRA_TEAM_REAL_CODEX"]).resolve()))
        for key in list(f.env):
            if key.startswith(("OPENAI_", "ANTHROPIC_")):
                f.env.pop(key)
        f.parent([{"sandbox_policy": {"type": "danger-full-access"}}])
        with GoalResponses() as provider:
            f.env["TEAM_LOCAL_RESPONSES_URL"] = provider.url
            owner, run, _ = f.start_control_fixture()
            f.wait_stderr_notice(owner, "pira_team native continuation:", timeout=15)
            number, _ = provider.captured.get(timeout=10)
            self.assertEqual(number, 3)
            self.assertEqual(f.access("interrupt", run.name).returncode, 0)
            owner.communicate(timeout=10)
            provider.releases[3].set()
            state = json.loads((run / "manifest.json").read_text())
            self.assertEqual(state["status"], "interrupted")
            original_goal = state["native_goal"]
            self.assertEqual(original_goal["status"], "active")
            old_phase = (run / "phase.md").read_text()
            # Model an old Team 0.9.0 manifest without rewriting native history.
            for key in ("native_goal", "goal_resume_safe", "goal_resume_context"):
                state.pop(key, None)
            (run / "manifest.json").write_text(json.dumps(state))
            resumed = subprocess.Popen([str(f.bin), "resume", run.name, "--store", str(f.root / "logs")],
                                       env=f.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            def cleanup():
                if resumed.poll() is None:
                    resumed.terminate()
                try:
                    resumed.communicate(timeout=8)
                except subprocess.TimeoutExpired:
                    resumed.kill()
                    resumed.communicate(timeout=3)
            self.addCleanup(cleanup)
            f.wait_stderr_notice(resumed, "pira_team native continuation:", timeout=15)
            number, body = provider.captured.get(timeout=10)
            self.assertEqual(number, 4)
            new_phase = (run / "revisions/000002/phase.md").read_text()
            policy = (run / "revisions/000002/policy.md").read_text()
            self.assertEqual(f.access("interrupt", run.name).returncode, 0)
            resumed.communicate(timeout=10)
            provider.releases[4].set()
            # This is the ACTUAL model-facing native request, not RPC arguments.
            prefix = body["input"][0]
            self.assertEqual(prefix["role"], "developer")
            text, = [part for part in strings(prefix) if part.startswith(policy + "\n\n")]
            self.assertTrue(text.endswith(new_phase))
            self.assertIn("supersedes retained phase/output contracts", text)
            self.assertIn("regardless of their position in serialized history", text)
            self.assertTrue(any(old_phase in part for part in strings(body.get("input"))))
            final = json.loads((run / "manifest.json").read_text())
            self.assertEqual(final["thread_id"], original_goal["threadId"])
            for field in ("threadId", "objective", "createdAt", "tokenBudget"):
                self.assertEqual(final["native_goal"].get(field), original_goal.get(field), field)
            self.assertEqual(final["native_goal"]["status"], "active")
            requests = [json.loads(line) for path in run.rglob("requests.jsonl") for line in path.read_text().splitlines()]
            self.assertEqual(sum(r.get("method") == "turn/start" for r in requests), 1)
            self.assertEqual(provider.count, 4)
            self.assertEqual(provider.errors, [])
            print("NATIVE_COLD_RESUME " + json.dumps({"new_contract_in_base_developer_message": True,
                "old_context_preserved": True, "same_thread": True, "goal_preserved": True,
                "client_kickoffs": 1, "scripted_requests": provider.count}), flush=True)


def load_tests(loader, tests, pattern):
    if os.environ.get("PIRA_TEAM_REAL_CODEX"):
        return tests
    return unittest.TestSuite()


if __name__ == "__main__":
    unittest.main()
