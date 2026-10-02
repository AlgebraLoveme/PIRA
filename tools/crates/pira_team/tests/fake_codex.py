#!/usr/bin/env python3
"""Deterministic protocol peer, not a model; all writes simulate runtime metadata."""
import json
import os
from pathlib import Path
import select
import sys
import time

args = sys.argv[1:]
assert args[:3] == ["app-server", "--strict-config", "--stdio"]
sandbox = json.loads(next(a.split("=", 1)[1] for a in args if a.startswith("sandbox_mode=")))
assert sandbox in ("read-only", "workspace-write")
sandbox_type = "readOnly" if sandbox == "read-only" else "workspaceWrite"
assert "project_doc_max_bytes=0" in args
assert "CODEX_THREAD_ID" not in os.environ
assert 'approval_policy="never"' in args
home = Path(os.environ["CODEX_HOME"])
assert not (home / "AGENTS.md").exists()
if source := os.environ.get("TEAM_TEST_AUTH_SOURCE"):
    assert (home / "auth.json").samefile(source)
if os.environ.get("CODEX_API_KEY"):
    assert not (home / "auth.json").exists()
policy = Path(json.loads(next(a.split("=", 1)[1] for a in args if a.startswith("model_instructions_file=")))).read_text()
assert "Never read or expose secrets files" in policy and "pira_nav" in policy
assert Path.cwd() != home
state_path = home / "fixture-state.json"
state = json.loads(state_path.read_text()) if state_path.exists() else {"thread": "fixture-thread", "turns": 0, "history": [], "usage": {}}
active = None
due = None

def send(value):
    print(json.dumps(value), flush=True)

def notify(method, params):
    send({"method": method, "params": {"threadId": state["thread"], "turnId": active, **params}})

def finish(status="completed"):
    global active, due
    if status == "completed":
        for key, amount in {"inputTokens":17, "cachedInputTokens":3, "outputTokens":5, "reasoningOutputTokens":2, "cacheWriteInputTokens":1}.items():
            state["usage"][key] = state["usage"].get(key, 0) + amount
        notify("thread/tokenUsage/updated", {"tokenUsage": {"total": state["usage"]}})
        notify("item/completed", {"item": {"type": "agentMessage", "phase": "commentary", "text": "not final"}})
        if task != "missing":
            candidate = {"filename":"review.md", "format":"markdown", "content":"ANSWER " + task}
            if task == "recall":
                candidate["content"] = json.dumps(state["history"])
            if os.environ.get("TEAM_CANDIDATE"):
                candidate = json.loads(os.environ["TEAM_CANDIDATE"])
            if repair and os.environ.get("TEAM_REPAIRED"):
                candidate = json.loads(os.environ["TEAM_REPAIRED"])
            final = candidate if isinstance(candidate, str) else json.dumps(candidate)
            notify("item/completed", {"item":{"type":"agentMessage", "phase":"final_answer", "text":final}})
    state_path.write_text(json.dumps(state))
    notify("turn/completed", {"turn":{"id":active, "status":status}})
    active, due = None, None

buffer = b""
while True:
    wait = max(0, due - time.monotonic()) if due else None
    if b"\n" not in buffer:
        if not select.select([sys.stdin], [], [], wait)[0]:
            finish("failed" if task == "failed" else "completed")
            continue
        chunk = os.read(sys.stdin.fileno(), 65536)
        if not chunk: break
        buffer += chunk
        if b"\n" not in buffer: continue
    line, buffer = buffer.split(b"\n", 1)
    request = json.loads(line)
    method, params = request["method"], request.get("params", {})
    if method == "initialized":
        continue
    result = {}
    if method in ("thread/start", "thread/resume"):
        assert params["approvalPolicy"] == "never" and params["sandbox"] == sandbox
        assert params["baseInstructions"] == policy and params["developerInstructions"] == ""
        if method == "thread/resume":
            assert params["threadId"] == state["thread"] and state_path.exists()
        if os.environ.get("TEAM_EXPECT_MODEL"):
            assert params["model"] == os.environ["TEAM_EXPECT_MODEL"]
        result = {"thread":{"id":state["thread"]}, "sandbox":{"type":sandbox_type}, "approvalPolicy":"never"}
        if os.environ.get("TEAM_BAD_PERMISSION"):
            result["sandbox"]["type"] = "dangerFullAccess"
        state_path.write_text(json.dumps(state))
    elif method == "turn/start":
        assert params["approvalPolicy"] == "never" and params["sandboxPolicy"]["type"] == sandbox_type
        if os.environ.get("TEAM_EXPECT_EFFORT"):
            assert params["effort"] == os.environ["TEAM_EXPECT_EFFORT"]
        if sandbox == "workspace-write":
            assert params["sandboxPolicy"]["writableRoots"] == [params["cwd"]]
            assert params["sandboxPolicy"]["networkAccess"] is False
            assert params["sandboxPolicy"]["excludeTmpdirEnvVar"] is True
            assert params["sandboxPolicy"]["excludeSlashTmp"] is True
        task = params["input"][0]["text"]
        repair = task.startswith("Repair only the output format")
        if repair and os.environ.get("TEAM_REPAIR_CRASH"): sys.exit(8)
        if task == "crash": sys.exit(7)
        if task == "malformed":
            print("not-json", flush=True)
            sys.exit(0)
        state["turns"] += 1
        active = str(state["turns"])
        state["history"].append(task)
        state_path.write_text(json.dumps(state))
        due = time.monotonic() + float(os.environ.get("TEAM_DELAY", "0.001"))
        if task in ("interrupt", "timeout", "steerable") or (repair and os.environ.get("TEAM_REPAIR_WAIT")): due = None
        result = {"turn":{"id":active}}
    elif method == "turn/steer":
        assert params["expectedTurnId"] == active
        if sandbox == "workspace-write":
            assert params["sandboxPolicy"]["writableRoots"] == [params["cwd"]]
            assert params["sandboxPolicy"]["networkAccess"] is False
            assert params["sandboxPolicy"]["excludeTmpdirEnvVar"] is True
            assert params["sandboxPolicy"]["excludeSlashTmp"] is True
        task = params["input"][0]["text"]
        state["history"].append(task)
        result = {"turnId":active}
        due = time.monotonic() + .02
    elif method == "turn/interrupt":
        assert params["turnId"] == active
        if os.environ.get("TEAM_IGNORE_INTERRUPT"): continue
        send({"id":request["id"], "result":{}})
        finish("interrupted")
        continue
    elif method != "initialize":
        raise AssertionError(method)
    send({"id":request["id"], "result":result})
    if method == "thread/start" and os.environ.get("TEAM_BLOCK_INPUT"):
        os.write(int(os.environ["TEAM_READY_FD"]), b"1")
        time.sleep(30)
    if method == "turn/start" and os.environ.get("TEAM_READY_FD"):
        os.write(int(os.environ["TEAM_READY_FD"]), b"1")
