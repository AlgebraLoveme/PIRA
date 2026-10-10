#!/usr/bin/env python3
"""Deterministic protocol peer, not a model; all writes simulate runtime metadata."""
import json
import os
from pathlib import Path
import select
import sys
import time

def pause_at(stage):
    if os.environ.get("TEAM_PAUSE_AT") == stage and os.environ.get("TEAM_PAUSE_DIRECTORY", Path.cwd().name) == Path.cwd().name:
        os.write(int(os.environ["TEAM_BOUNDARY_READY_FD"]), b"1")
        assert os.read(int(os.environ["TEAM_BOUNDARY_RELEASE_FD"]), 1) == b"1"


args = sys.argv[1:]
if args[:2] == ["app-server", "generate-json-schema"]:
    import runpy
    pause_at("schema")
    if os.environ.get("TEAM_SCHEMA_FAIL"):
        print("fixture schema process failure", file=sys.stderr)
        sys.exit(7)
    contract = json.loads(Path(os.environ["TEAM_TEST_BACKEND_CONTRACT"]).read_text())
    fixture = runpy.run_path(os.environ["TEAM_TEST_BACKEND_FIXTURE"])
    fixture["write_schemas"](args[args.index("--out") + 1], contract, os.environ.get("TEAM_BAD_BACKEND", ""))
    sys.exit(0)
assert args[:3] == ["app-server", "--strict-config", "--stdio"]
sandbox = json.loads(next(a.split("=", 1)[1] for a in args if a.startswith("sandbox_mode=")))
if os.environ.get("TEAM_ASSERT_BUILD_ENV"):
    import fnmatch
    assert 'shell_environment_policy.inherit="all"' in args
    assert 'shell_environment_policy.ignore_default_excludes=true' in args
    prefix = "shell_environment_policy.exclude="
    excluded = json.loads(next(a[len(prefix):] for a in args if a.startswith(prefix)))
    shell = {k: v for k, v in os.environ.items()
             if not any(fnmatch.fnmatchcase(k.upper(), pat.upper()) for pat in excluded)}
    prefix = "shell_environment_policy.set."
    for arg in args:
        if arg.startswith(prefix):
            key, value = arg[len(prefix):].split("=", 1)
            shell[key] = json.loads(value)
    for key in ("CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR",
                "FONTCONFIG_FILE", "CARGO_BUILD_JOBS", "UNRECOGNIZED_TASK_OPTION", "TASK_KEYWORDS"):
        assert shell[key] == os.environ[key]
        assert not any(a.startswith(f"shell_environment_policy.set.{key}=") for a in args)
    for key in ("PRIVATE_ACCESS_TOKEN", "CODEX_API_KEY", "AWS_SECRET_ACCESS_KEY", "GH_TOKEN",
                "SSH_AUTH_SOCK", "CODEX_HOME",
                "CODEX_SESSION_ID", "PIRA_CTX_THREAD_ID", "PIRA_TEAM_DIR"):
        assert key not in shell
    assert shell["PIRA_TEAM_CHILD"] == "1"
    assert shell["PIRA_TEAM_HANDOFF"] != "stale-parent-handoff"
assert "agents.enabled=false" in args
assert "features.multi_agent=false" in args
scratch = Path(os.environ["CODEX_HOME"]).parent / "scratch"
for key in ("TMPDIR", "TMP", "TEMP", "TMPPREFIX"):
    prefix = f"shell_environment_policy.set.{key}="
    expected = str(scratch / "zsh" if key == "TMPPREFIX" else scratch)
    assert json.loads(next(a[len(prefix):] for a in args if a.startswith(prefix))) == expected
    assert os.environ[key] == expected
if os.environ.get("TEAM_ASSERT_TEMP"):
    import tempfile
    import subprocess
    assert Path(tempfile.gettempdir()) == scratch
    with tempfile.NamedTemporaryFile() as file:
        assert Path(file.name).parent == scratch
        file.write(b"scratch write")
        file.flush()
    # Large heredoc exercises shell temporary storage on shells that spill to disk.
    for shell in ("/bin/sh", "/bin/zsh"):
        if Path(shell).is_file():
            subprocess.run([shell, "-c", "cat >/dev/null <<'END'\n" + "x" * 32768 + "\nEND\n"],
                           check=True, timeout=5, capture_output=True)
assert sandbox in ("workspace-write", "danger-full-access")
sandbox_type = "dangerFullAccess" if sandbox == "danger-full-access" else "workspaceWrite"
config = {a.split("=",1)[0]:json.loads(a.split("=",1)[1]) for a in args if a.startswith("sandbox_workspace_write.")}
expected_sandbox = ({"type":"dangerFullAccess"} if sandbox == "danger-full-access" else {
    "type":"workspaceWrite", "writableRoots":config["sandbox_workspace_write.writable_roots"],
    "networkAccess":config["sandbox_workspace_write.network_access"],
    "excludeTmpdirEnvVar":config["sandbox_workspace_write.exclude_tmpdir_env_var"],
    "excludeSlashTmp":config["sandbox_workspace_write.exclude_slash_tmp"]})
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
handoff = Path(json.loads(next(a.split("=", 1)[1] for a in args if a.startswith("shell_environment_policy.set.PIRA_TEAM_HANDOFF="))))

def send(value):
    print(json.dumps(value), flush=True)

def notify(method, params):
    send({"method": method, "params": {"threadId": state["thread"], "turnId": active, **params}})

def partial_evidence(*, acknowledged):
    notify("thread/tokenUsage/updated", {"tokenUsage": {"total": {"inputTokens": 11, "cachedInputTokens": 3, "outputTokens": 2}}})
    notify("item/completed", {"item": {"type": "agentMessage", "phase": "final_answer", "text": "partial diagnostic"}})
    # Foreign observations must not contaminate accounting or the retained candidate.
    for ids in [{"threadId": "foreign-thread"}] + ([{"turnId": "foreign-turn"}] if acknowledged else []):
        notify("thread/tokenUsage/updated", {**ids, "tokenUsage": {"total": {"inputTokens": 999, "cachedInputTokens": 9, "outputTokens": 99}}})
        notify("item/completed", {**ids, "item": {"type": "agentMessage", "phase": "final_answer", "text": "foreign diagnostic"}})
    notify("item/completed", {"item": {"type": "agentMessage", "phase": "commentary", "text": "not final"}})


def refuse_request():
    send({"id": "backend-1", "method": "item/tool/request", "params": {}})
    reply = json.loads(sys.stdin.readline())
    assert reply["id"] == "backend-1" and reply["error"]["code"] == -32601
    sys.exit(0)


def finish(status="completed"):
    global active, due
    if status == "completed":
        for key, amount in {"inputTokens":17, "cachedInputTokens":3, "outputTokens":5, "reasoningOutputTokens":2, "cacheWriteInputTokens":1}.items():
            state["usage"][key] = state["usage"].get(key, 0) + amount
        if not os.environ.get("TEAM_NO_USAGE"):
            notify("thread/tokenUsage/updated", {"tokenUsage": {"total": state["usage"]}})
        notify("item/completed", {"item": {"type": "agentMessage", "phase": "commentary", "text": "not final"}})
        if task != "missing":
            candidate = {"filename":"review.md", "format":"markdown", "content":"ANSWER " + task}
            if task == "recall":
                candidate["content"] = json.dumps(state["history"])
            if task == "decision-probe":
                import fnmatch
                import subprocess
                excluded = json.loads(next(a.split("=", 1)[1] for a in args
                                           if a.startswith("shell_environment_policy.exclude=")))
                shell = {k: v for k, v in os.environ.items()
                         if not any(fnmatch.fnmatchcase(k.upper(), pat.upper()) for pat in excluded)}
                prefix = "shell_environment_policy.set."
                for arg in args:
                    if arg.startswith(prefix):
                        key, value = arg[len(prefix):].split("=", 1)
                        shell[key] = json.loads(value)
                shell["CODEX_THREAD_ID"] = state["thread"]
                results = []
                for request in json.loads(os.environ["TEAM_DEC_COMMANDS"]):
                    command = request["args"] if isinstance(request, dict) else request
                    cwd = request.get("cwd", state["cwd"]) if isinstance(request, dict) else state["cwd"]
                    proc = subprocess.run([os.environ["TEAM_DEC_BIN"], *command],
                                          cwd=cwd, env=shell, capture_output=True,
                                          text=True, timeout=5)
                    results.append({"returncode": proc.returncode, "stdout": proc.stdout,
                                    "stderr": proc.stderr})
                candidate["content"] = json.dumps(results)
            if os.environ.get("TEAM_CANDIDATE"):
                candidate = json.loads(os.environ["TEAM_CANDIDATE"])
            if handoff.name == "review-checkpoint.md" and os.environ.get("TEAM_REVIEW_CANDIDATE"):
                candidate = json.loads(os.environ["TEAM_REVIEW_CANDIDATE"])
            if repair and os.environ.get("TEAM_REPAIRED"):
                candidate = json.loads(os.environ["TEAM_REPAIRED"])
            if isinstance(candidate, dict) and "content" in candidate:
                if os.environ.get("TEAM_HANDOFF_SYMLINK"):
                    if not handoff.is_symlink(): handoff.symlink_to(os.environ["TEAM_HANDOFF_SYMLINK"])
                elif os.environ.get("TEAM_HANDOFF_HARDLINK"):
                    if not handoff.exists(): os.link(os.environ["TEAM_HANDOFF_HARDLINK"], handoff)
                elif not os.environ.get("TEAM_SKIP_HANDOFF"):
                    handoff.write_text(candidate["content"])
                final = json.dumps({"status":candidate.get("status", "completed"), "format":candidate["format"]})
            else:
                final = candidate if isinstance(candidate, str) else json.dumps(candidate)
            notify("item/completed", {"item":{"type":"agentMessage", "phase":"final_answer", "text":final}})
    state_path.write_text(json.dumps(state))
    turn = {"id": active, "status": status}
    error_stage = os.environ.get("TEAM_COMPLETED_ERROR")
    if status == "completed" and (error_stage == "all"
            or (error_stage == "implementation" and "IMPLEMENTATION STAGE." in state.get("phase", ""))
            or (error_stage == "repair" and repair)):
        turn["error"] = {"message": "fixture native completion error"}
    notify("turn/completed", {"turn": turn})
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
        state["cwd"] = params["cwd"]
        assert params["approvalPolicy"] == "never" and params["sandbox"] == sandbox
        assert params["baseInstructions"] == policy and params["developerInstructions"] == ""
        if "base_policy" in state and os.environ.get("TEAM_ASSERT_PREFIX"):
            assert state["base_policy"] == policy
        else:
            state["base_policy"] = policy
        if method == "thread/resume":
            assert params["threadId"] == state["thread"] and state_path.exists()
        if os.environ.get("TEAM_EXPECT_MODEL"):
            assert params["model"] == os.environ["TEAM_EXPECT_MODEL"]
        assert params["config"] == ({"sandbox_workspace_write":{k.split(".")[1]:v for k,v in config.items()}} if config else {})
        result = {"thread":{"id":state["thread"]}, "sandbox":dict(expected_sandbox), "approvalPolicy":"never"}
        if os.environ.get("TEAM_BAD_PERMISSION"):
            result["sandbox"]["type"] = "dangerFullAccess"
        state_path.write_text(json.dumps(state))
    elif method == "thread/inject_items":
        assert params["threadId"] == state["thread"]
        item, = params["items"]
        assert item["role"] == "developer" and item["type"] == "message"
        state["phase"] = item["content"][0]["text"]
        state.setdefault("phases", []).append(state["phase"])
        state_path.write_text(json.dumps(state))
    elif method == "turn/start":
        assert params["approvalPolicy"] == "never" and params["sandboxPolicy"]["type"] == sandbox_type
        if os.environ.get("TEAM_EXPECT_EFFORT"):
            assert params["effort"] == os.environ["TEAM_EXPECT_EFFORT"]
        assert params["sandboxPolicy"] == expected_sandbox
        if sandbox == "workspace-write":
            roots = params["sandboxPolicy"]["writableRoots"]
            assert any(Path(params["cwd"]).is_relative_to(Path(r)) for r in roots)
            assert any(handoff.parent.is_relative_to(Path(r)) for r in roots)
            assert str(home) not in roots
            assert any(scratch.is_relative_to(Path(r)) for r in roots)
            for expected in json.loads(os.environ.get("TEAM_EXPECT_BUILD_ROOTS", "[]")):
                assert any(Path(expected).is_relative_to(Path(r)) for r in roots), (expected, roots)
            for forbidden in json.loads(os.environ.get("TEAM_FORBIDDEN_BUILD_ROOTS", "[]")):
                assert forbidden not in roots
            for forbidden in json.loads(os.environ.get("TEAM_FORBIDDEN_TEMP_ROOTS", "[]")):
                assert forbidden not in roots
            for key in ("PIRA_CTX_STORE_DIR", "PIRA_DEC_STORE_DIR"):
                prefix = f"shell_environment_policy.set.{key}="
                assert any(Path(json.loads(next(a[len(prefix):] for a in args if a.startswith(prefix)))).is_relative_to(Path(r)) for r in roots)

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
        state.setdefault("turn_details", []).append({"phase": state.get("phase"), "handoff": str(handoff), "policy": policy})
        state_path.write_text(json.dumps(state))
        due = time.monotonic() + float(os.environ.get("TEAM_DELAY", "0.001"))
        if task in ("interrupt", "timeout", "steerable") or (repair and os.environ.get("TEAM_REPAIR_WAIT")): due = None
        if os.environ.get("TEAM_IMPLEMENT_WAIT") and "IMPLEMENTATION STAGE." in state.get("phase", ""):
            due = None
        result = {"turn":{"id":active}}
    elif method == "turn/steer":
        assert params["expectedTurnId"] == active
        if os.environ.get("TEAM_STALL_STEER"):
            os.write(int(os.environ["TEAM_STEER_READY_FD"]), b"1")
            continue
        task, gate = params["input"][0]["text"].rsplit("\n\nReplacement completion gate: ", 1)
        assert gate.strip()
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
    failure = os.environ.get("TEAM_PARTIAL_FAILURE", "")
    if method == "turn/start" and failure.startswith("before-"):
        partial_evidence(acknowledged=False)
        if failure == "before-refusal":
            refuse_request()
        if failure == "before-crash":
            sys.exit(9)
        if failure == "before-rpc-error":
            due = None  # A rejected startup cannot later complete successfully.
            send({"id": request["id"], "error": {"message": "fixture turn/start error"}})
            continue
    pause_at(method)
    send({"id":request["id"], "result":result})
    if method == "turn/start" and failure == "after-refusal":
        partial_evidence(acknowledged=True)
        refuse_request()
    if method == "turn/start" and os.environ.get("TEAM_DETACHED_PIPE"):
        import subprocess
        child = subprocess.Popen([sys.executable, "-c", "import signal; signal.pause()"], start_new_session=True)
        (home / "detached-fixture.pid").write_text(str(child.pid))
        os.write(int(os.environ["TEAM_BOUNDARY_READY_FD"]), b"1")
    if method == "turn/start" and task == "partial-crash":
        notify("thread/tokenUsage/updated", {"tokenUsage": {"total": {"inputTokens": 11, "cachedInputTokens": 3, "outputTokens": 2}}})
        notify("item/completed", {"item": {"type": "agentMessage", "phase": "final_answer", "text": "partial diagnostic"}})
        sys.exit(9)
    if method == "thread/start" and os.environ.get("TEAM_BLOCK_INPUT"):
        os.write(int(os.environ["TEAM_READY_FD"]), b"1")
        time.sleep(30)
    if method == "turn/start" and os.environ.get("TEAM_READY_FD"):
        os.write(int(os.environ["TEAM_READY_FD"]), b"1")
    if method == "turn/start" and os.environ.get("TEAM_BLOCK_ACTIVE_INPUT"):
        # The launcher can now expose controls, but this peer will never drain them.
        time.sleep(30)
