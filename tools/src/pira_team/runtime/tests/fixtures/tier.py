#!/usr/bin/env python3
"""Model-free peer for the owner-loop tier tests; never accesses auth."""
import json
import os
from pathlib import Path
import sys
import threading

if "generate-json-schema" in sys.argv:
    output = Path(sys.argv[sys.argv.index("--out") + 1])
    # tier_fixture prepends the shared deterministic schema exporter.
    write_schemas(output, json.loads(Path(__file__).with_name("fixture-contract.json").read_text()))
    raise SystemExit(0)

# A failed assertion in the Rust driver must not leave an indefinite peer.
watchdog = threading.Timer(20, lambda: os._exit(10))
watchdog.daemon = True
watchdog.start()
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
