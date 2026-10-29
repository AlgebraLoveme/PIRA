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
        if endpoint := os.environ.get("TEAM_BOUNDARY_ENDPOINT"):
            import socket
            with socket.create_connection(tuple(json.loads(endpoint)), timeout=20) as connection:
                connection.sendall(b"1")
                assert connection.recv(1) == b"1"
        else:
            os.write(int(os.environ["TEAM_BOUNDARY_READY_FD"]), b"1")
            assert os.read(int(os.environ["TEAM_BOUNDARY_RELEASE_FD"]), 1) == b"1"


args = sys.argv[1:]
if args and args[0] == "native-fixture-app-server":
    args[0] = "app-server"
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
if real_codex := os.environ.get("TEAM_REAL_COORDINATOR_CODEX"):
    home = Path(os.environ["CODEX_HOME"])
    manifest = json.loads((home.parent / "manifest.json").read_text())
    if manifest["parent_run_id"] is None:
        from urllib.parse import urlparse
        url = os.environ["TEAM_LOCAL_RESPONSES_URL"]
        endpoint = urlparse(url)
        assert endpoint.scheme == "http" and endpoint.hostname == "127.0.0.1" and endpoint.port
        assert endpoint.username is None and endpoint.password is None
        config = [
            'model_provider="relocation_probe"',
            'model_providers.relocation_probe.name="Offline abandonment fixture"',
            "model_providers.relocation_probe.base_url=" + json.dumps(url),
            'model_providers.relocation_probe.wire_api="responses"',
            'model_providers.relocation_probe.requires_openai_auth=false',
            'model_providers.relocation_probe.request_max_retries=0',
            'model_providers.relocation_probe.stream_max_retries=0',
            'features.shell_snapshot=false',
        ]
        command = [real_codex, *args, *[arg for value in config for arg in ("-c", value)]]
        if os.environ.get("TEAM_PAUSE_AT") == "capacity-terminal":
            import subprocess
            # The Python proxy is Team's registered backend identity, not its
            # Codex subprocess. Keep the same shell settings without advertising
            # a second ancestry marker on the subprocess's command line.
            forwarded, settings = [], {}
            index = 0
            prefix = "shell_environment_policy.set."
            while index < len(command):
                if command[index] == "-c" and command[index + 1].startswith(prefix):
                    key, value = command[index + 1][len(prefix):].split("=", 1)
                    settings[key] = json.loads(value)
                    index += 2
                else:
                    forwarded.append(command[index])
                    index += 1
            forwarded += ["-c", "shell_environment_policy.set={" + ",".join(
                key + "=" + json.dumps(value) for key, value in settings.items()) + "}"]
            # Hold the terminal before Team sees it: scope cleanup cannot yet
            # have started. Keep stdin open so the real backend remains active.
            peer = subprocess.Popen(forwarded, stdin=sys.stdin, stdout=subprocess.PIPE, text=True)
            for line in peer.stdout:
                event = json.loads(line)
                if event.get("method") == "turn/completed" and event["params"]["turn"]["status"] == "failed":
                    pause_at("capacity-terminal")
                sys.stdout.write(line)
                sys.stdout.flush()
            code = peer.wait(timeout=10)
            Path(os.environ["TEAM_REAL_COORDINATOR_EXIT"]).write_text(json.dumps({"returncode": code}))
            sys.exit(code)
        os.execv(real_codex, command)
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
if os.environ.get("TEAM_FORBID_API_KEY"):
    assert not os.environ.get("CODEX_API_KEY"), "unexpected API-key authentication selection"
if expected_url := os.environ.get("TEAM_EXPECT_BACKEND_URL"):
    assert os.environ.get("OPENAI_BASE_URL") == expected_url, "selected backend endpoint changed"
if expected := os.environ.get("TEAM_EXPECT_AUTH_SHA256"):
    import fnmatch
    import hashlib
    selected_key = os.environ.get("CODEX_API_KEY", "")
    assert hashlib.sha256(selected_key.encode()).hexdigest() == expected, "backend authentication selection mismatch"
    excluded = json.loads(next(a.split("=",1)[1] for a in args if a.startswith("shell_environment_policy.exclude=")))
    shell = {key:value for key,value in os.environ.items()
             if not any(fnmatch.fnmatchcase(key.upper(), pattern.upper()) for pattern in excluded)}
    prefix = "shell_environment_policy.set."
    for arg in args:
        if arg.startswith(prefix):
            key,value = arg[len(prefix):].split("=",1)
            shell[key] = json.loads(value)
    assert all(selected_key not in str(value) for value in shell.values()), "credential exposed to worker environment"
    assert all(selected_key not in arg for arg in args), "credential exposed in backend arguments"
    del selected_key
policy = Path(json.loads(next(a.split("=", 1)[1] for a in args if a.startswith("model_instructions_file=")))).read_text(encoding="utf-8")
assert "Never read or expose secrets files" in policy and "pira_nav" in policy
assert Path.cwd() != home
state_path = home / "fixture-state.json"
state = json.loads(state_path.read_text()) if state_path.exists() else {"thread": "fixture-thread", "turns": 0, "history": [], "usage": {}}
if os.environ.get("TEAM_SUBTEAM_FIXTURE") and not state_path.exists():
    state["thread"] = "worker-" + home.parent.name
active = None
due = None
subteam_service = None
service_read, service_write = os.pipe()
handoff = Path(json.loads(next(a.split("=", 1)[1] for a in args if a.startswith("shell_environment_policy.set.PIRA_TEAM_HANDOFF="))))

def send(value):
    print(json.dumps(value), flush=True)

def notify(method, params):
    send({"method": method, "params": {"threadId": state["thread"], "turnId": active, **params}})

def notification_burst():
    for index in range(int(os.environ.get("TEAM_BURST_COUNT", "0"))):
        state["usage"] = {"inputTokens": index + 1, "cachedInputTokens": 0, "outputTokens": index + 1}
        notify("thread/tokenUsage/updated", {"tokenUsage": {"total": state["usage"]}})
        notify("item/completed", {"item": {"type": "agentMessage", "phase": "final_answer",
                                           "text": f"burst-{index}"}})
        notify("item/completed", {"item": {"type": "agentMessage", "phase": "commentary", "text": "progress"}})
        if delay := float(os.environ.get("TEAM_BURST_DELAY", "0")):
            time.sleep(delay)
    if os.environ.get("TEAM_BURST_MALFORMED"):
        print("not-json", flush=True)
        sys.exit(9)

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
    turn = {"id": active, "status": "failed" if status == "capacity" else status}
    if status == "capacity":
        turn["error"] = {"message": "Selected model is at capacity. Please try a different model.",
                         "codexErrorInfo": "serverOverloaded"}
    error_stage = os.environ.get("TEAM_COMPLETED_ERROR")
    if status == "completed" and (error_stage == "all"
            or (error_stage == "implementation" and "IMPLEMENTATION STAGE." in state.get("phase", ""))
            or (error_stage == "repair" and repair)):
        turn["error"] = {"message": "fixture native completion error"}
    notify("turn/completed", {"turn": turn})
    active, due = None, None

if os.name == "nt":
    import queue
    import threading
    native_input = queue.Queue()
    def read_input():
        # Raw reads cannot hold Python's buffered-stdin lock during failure exit.
        pending = b""
        while chunk := os.read(sys.stdin.fileno(), 65536):
            pending += chunk
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                native_input.put(line)
        native_input.put(b"")
    threading.Thread(target=read_input, daemon=True).start()

buffer = b""
while True:
    wait = max(0, due - time.monotonic()) if due else None
    if os.name == "nt":
        try:
            line = native_input.get(timeout=wait)
        except queue.Empty:
            finish("failed" if task == "failed" else "completed")
            continue
        if line is None:
            finish()
            continue
        if line == "capacity":
            finish("capacity")
            continue
        if not line:
            break
    elif b"\n" not in buffer:
        ready = select.select([sys.stdin, service_read], [], [], wait)[0]
        if service_read in ready:
            finish("capacity" if os.read(service_read, 1) == b"F" else "completed")
            continue
        if not ready:
            finish("failed" if task == "failed" else "completed")
            continue
        chunk = os.read(sys.stdin.fileno(), 65536)
        if not chunk: break
        buffer += chunk
        if b"\n" not in buffer: continue
    if os.name != "nt":
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
        if os.environ.get("TEAM_SUBTEAM_FIXTURE"):
            sessions = home / "sessions"
            sessions.mkdir(exist_ok=True)
            caller_policy = ({"type":"danger-full-access"} if sandbox == "danger-full-access" else {
                "type":"workspace-write", "writable_roots":expected_sandbox["writableRoots"],
                "network_access":expected_sandbox["networkAccess"],
                "exclude_tmpdir_env_var":expected_sandbox["excludeTmpdirEnvVar"],
                "exclude_slash_tmp":expected_sandbox["excludeSlashTmp"]})
            (sessions / f"rollout-worker-{state['thread']}.jsonl").write_text(
                json.dumps({"type":"session_meta", "payload":{"id":state["thread"]}}) + "\n" +
                json.dumps({"type":"turn_context", "payload":{"model":params["model"], "effort":"high", "cwd":state["cwd"],
                    "approval_policy":"never", "sandbox_policy":caller_policy}}) + "\n")
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
            def physical_path(path):
                value = str(Path(path).resolve())
                # ntpath.realpath deliberately preserves a supplied verbatim
                # prefix, but removes it for ordinary paths to the same file.
                if value.startswith("\\\\?\\UNC\\"):
                    value = "\\\\" + value[8:]
                elif value.startswith("\\\\?\\"):
                    value = value[4:]
                return Path(value)
            physical_roots = [physical_path(root) for root in roots]
            def within_roots(path):
                path = physical_path(path)
                return any(path.is_relative_to(root) for root in physical_roots)
            assert within_roots(params["cwd"])
            assert within_roots(handoff.parent)
            assert str(home) not in roots
            assert within_roots(scratch)
            for expected in json.loads(os.environ.get("TEAM_EXPECT_BUILD_ROOTS", "[]")):
                assert within_roots(expected), (expected, roots)
            for forbidden in json.loads(os.environ.get("TEAM_FORBIDDEN_BUILD_ROOTS", "[]")):
                assert forbidden not in roots
            for forbidden in json.loads(os.environ.get("TEAM_FORBIDDEN_TEMP_ROOTS", "[]")):
                assert forbidden not in roots
            for key in ("PIRA_CTX_STORE_DIR", "PIRA_DEC_STORE_DIR"):
                prefix = f"shell_environment_policy.set.{key}="
                target = json.loads(next(a[len(prefix):] for a in args if a.startswith(prefix)))
                assert within_roots(target), (key, target, roots)

        task = params["input"][0]["text"]
        if task == "subteam-service" or (task == "Continue the existing assignment." and os.environ.get("TEAM_SUBTEAM_FIXTURE")):
            import runpy
            sys.path.insert(0, str(Path(os.environ["TEAM_SUBTEAM_FIXTURE"]).parent))
            fixture = runpy.run_path(os.environ["TEAM_SUBTEAM_FIXTURE"])
            def complete_subteam(status="completed"):
                if os.name == "nt":
                    native_input.put("capacity" if status == "capacity" else None)
                else:
                    os.write(service_write, b"F" if status == "capacity" else b"1")
            subteam_service = fixture["start"](args, state, home, complete_subteam)
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
        if task in ("interrupt", "timeout", "steerable", "subteam-service") or subteam_service is not None or (repair and os.environ.get("TEAM_REPAIR_WAIT")): due = None
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
    if method == "turn/start" and os.environ.get("TEAM_BURST_AT") == "before-ack":
        notification_burst()
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
    if method == "turn/start" and os.environ.get("TEAM_BURST_AT") == "after-ack":
        notification_burst()
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
