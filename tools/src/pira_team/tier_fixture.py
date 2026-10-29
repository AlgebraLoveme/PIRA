#!/usr/bin/env python3
"""Model-free peer for the ignored owner-loop tier test; never accesses auth."""
import json
import os
from pathlib import Path
import shutil
import sys
import threading

if "generate-json-schema" in sys.argv:
    output = Path(sys.argv[sys.argv.index("--out") + 1])
    shutil.copytree(os.environ["PIRA_TIER_SCHEMA"], output, dirs_exist_ok=True)
    raise SystemExit(0)

root = Path.cwd()
os.mkfifo(root / "finish.fifo")
lock = threading.Lock()
updates = 0

def send(message):
    with lock:
        print(json.dumps(message), flush=True)

def finish_job():
    with (root / "finish.fifo").open() as stream:
        command = stream.read()
    if command == "exit":
        os._exit(9)
    (root / "job-finished").write_text("completed normally")
    send({"method": "turn/completed", "params": {"threadId": "thread", "turn": {"id": "turn", "status": "completed", "error": None}}})

threading.Thread(target=finish_job, daemon=True).start()
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if "id" not in request:
        continue
    response = {"id": request["id"]}
    if method == "initialize" or method == "thread/inject_items":
        response["result"] = {}
    elif method in ["thread/start", "thread/resume"]:
        response["result"] = {"thread": {"id": "thread"}, "sandbox": {"type": "dangerFullAccess"}, "approvalPolicy": "never"}
    elif method == "turn/start":
        (root / "job-running").write_text(str(os.getpid()))
        response["result"] = {"turn": {"id": "turn", "status": "inProgress"}}
    elif method == "turn/settings/update":
        assert (root / "job-running").exists() and not (root / "job-finished").exists()
        updates += 1
        if updates == 6:
            (root / "pending-tier").write_text("awaiting natural turn completion without ack")
            continue
        if updates == 2:
            response["error"] = {"code": -32602, "message": "fixture tier rejection"}
        else:
            response["result"] = {"status": {3: "targetUnavailable", 4: "unrecognized"}.get(updates, "applied")}
    else:
        raise AssertionError("unexpected owner RPC: " + str(method))
    send(response)

    if method == "turn/start" and (root / "close-stdin").exists():
        os.close(0)
        (root / "input-closed").write_text("stdin closed but workload remains active")
        threading.Event().wait()
