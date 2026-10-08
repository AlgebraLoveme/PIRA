"""Finite, local protocol peer for pira_nav's process/pipe regression tests."""
import json
import os
import signal
import sys
import time
from pathlib import Path
from urllib.parse import unquote, urlparse

signal.alarm(8)
mode = sys.argv[1]


def send(value):
    payload = json.dumps(value).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload)
    sys.stdout.buffer.flush()


def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\n", b"\r\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1])
    return json.loads(sys.stdin.buffer.read(length))


while True:
    message = read()
    method = message.get("method")
    if method == "initialize":
        if mode == "header":
            sys.stdout.buffer.write(b"X" * 8193)
            sys.stdout.buffer.flush()
            time.sleep(4)
            sys.exit(0)
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {"definitionProvider": True, "documentSymbolProvider": True}}})
    elif method == "initialized" and mode in ("duplex", "overflow"):
        # Do not read didOpen until the burst is completely written. Its 1 MiB
        # source also exceeds a pipe buffer: blocking reader backpressure deadlocks.
        for _ in range(128 if mode == "duplex" else 257):
            send({"jsonrpc": "2.0", "method": "test/notification", "params": "x" * 8192})
    elif method == "textDocument/documentSymbol":
        uri = message["params"]["textDocument"]["uri"]
        path = Path(unquote(urlparse(uri).path))
        if mode == "snapshot":
            path.write_text("def other(): pass\n# outside\n")
        elif mode == "uri":
            # Opening the URI's actual identity must find the backslash-bearing file.
            assert path.read_text().startswith("def alpha")
            Path("uri.txt").write_text(uri)
        send({"jsonrpc": "2.0", "id": message["id"], "result": [{
            "name": "alpha", "kind": 12,
            "range": {"start": {"line": 0, "character": 0},
                      "end": {"line": 1, "character": 0}},
        }]})
    elif method == "textDocument/definition":
        position = {"line": 2147483648 if mode == "integer" else 1, "character": 0}
        if mode == "descendant":
            ready_read, ready_write = os.pipe()
            if os.fork() == 0:
                os.close(ready_read)
                os.write(ready_write, b"ready")
                os.close(ready_write)
                # Retain stdin/stdout/stderr but do no I/O. Finite even on regressions.
                time.sleep(4)
                os._exit(0)
            os.close(ready_write)
            assert os.read(ready_read, 5) == b"ready"
            os.close(ready_read)
        send({"jsonrpc": "2.0", "id": message["id"], "result": [{
            "uri": message["params"]["textDocument"]["uri"],
            "range": {"start": position, "end": position},
        }]})
        if mode == "descendant":
            os._exit(0)
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
    elif method == "exit":
        sys.exit(0)
