"""Finite deterministic LSP peer: supplied positions/results, no source resolution heuristics."""
import json
from pathlib import Path
import signal
import sys
from urllib.parse import urlparse
from urllib.request import url2pathname

if hasattr(signal, "alarm"):
    signal.alarm(15)
root = Path.cwd()
config = json.loads(sys.argv[1]) if len(sys.argv) > 1 else json.loads((root / ".nav-lsp.json").read_text())
if config.get("startup_log"):
    Path(config["startup_log"]).write_text("started")


def read():
    size = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            size = int(line.split(b":", 1)[1])
    return json.loads(sys.stdin.buffer.read(size))


def location(target):
    if isinstance(target, dict):
        return target
    uri = target if "://" in target else (root / target).as_uri()
    span = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}
    return {"targetUri": uri, "targetRange": span, "targetSelectionRange": span}


while True:
    message = read()
    method = message.get("method")
    if method == "exit":
        sys.exit(0)
    if method == "textDocument/didOpen" and config.get("open_log"):
        with Path(config["open_log"]).open("a") as log:
            log.write(message["params"]["textDocument"]["uri"] + "\n")
    if "id" not in message:
        continue
    result = None
    if method == "initialize":
        result = {"capabilities": config.get("capabilities", {
            "definitionProvider": True, "documentSymbolProvider": True, "hoverProvider": True})}
    elif method == "textDocument/documentSymbol":
        uri = message["params"]["textDocument"]["uri"]
        if config.get("request_edit"):
            request = {"jsonrpc": "2.0", "id": "edit-probe", "method": "workspace/applyEdit",
                       "params": {"edit": {"changes": {uri: [{"range": {
                           "start": {"line": 0, "character": 0},
                           "end": {"line": 0, "character": 0}}, "newText": "MUST_NOT_APPEAR"}]}}}}
            payload = json.dumps(request).encode()
            sys.stdout.buffer.write(f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload)
            sys.stdout.buffer.flush()
            response = read()
            assert response["id"] == "edit-probe" and response["result"]["applied"] is False
        result = config.get("inventories", {}).get(uri, config.get("symbols", [{"name": "server_only", "kind": 12,
                   "range": {"start": {"line": 0, "character": 0},
                             "end": {"line": 0, "character": 18}},
                   "selectionRange": {"start": {"line": 0, "character": 4},
                                      "end": {"line": 0, "character": 10}}}]))
    elif method == "textDocument/prepareCallHierarchy":
        result = config.get("prepared_calls", [])
    elif method in ("callHierarchy/outgoingCalls", "callHierarchy/incomingCalls"):
        direction = "outgoing_calls" if method.endswith("outgoingCalls") else "incoming_calls"
        result = config.get(direction, {}).get(message["params"]["item"]["uri"], [])
    elif method in ("textDocument/definition", "textDocument/hover"):
        params = message["params"]
        path = Path(url2pathname(urlparse(params["textDocument"]["uri"]).path))
        position = params["position"]
        key = f"{path.relative_to(root).as_posix()}:{position['line']}:{position['character']}"
        if config.get("log_requests", True):
            with (root / ".requests").open("a") as log:
                log.write(key + "\n")
        targets = config.get("positions", {}).get(key, config.get("default", []))
        result = [location(target) for target in targets] if targets is not None else None
        if method == "textDocument/hover":
            result = {"contents": config.get("hovers", {}).get(key, "Fake semantic information.")}
    response = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(response)}\r\n\r\n".encode() + response)
    sys.stdout.buffer.flush()
