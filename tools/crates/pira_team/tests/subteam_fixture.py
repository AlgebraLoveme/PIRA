"""Out-of-band deterministic fixture. Commands run beneath the actual fake backend."""
import fnmatch
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading


def complete_after_ack(connection, stream, operation, complete):
    stream.write(b'{"ok": true}\n')
    stream.flush()
    # Completion can kill this backend: require proof the client read our reply.
    connection.settimeout(5)
    if stream.readline(5) != b"ack\n":
        raise ValueError("missing or invalid completion acknowledgement")
    if operation == "finish":
        complete()
    else:
        complete("capacity")


def start(args, state, home, complete):
    prefix = "shell_environment_policy.exclude="
    excluded = json.loads(next(a[len(prefix):] for a in args if a.startswith(prefix)))
    shell = {k: v for k, v in os.environ.items()
             if not any(fnmatch.fnmatchcase(k.upper(), pat.upper()) for pat in excluded)}
    prefix = "shell_environment_policy.set."
    for arg in args:
        if arg.startswith(prefix):
            key, value = arg[len(prefix):].split("=", 1)
            shell[key] = json.loads(value)
    shell["CODEX_THREAD_ID"] = state["thread"]
    address = str(Path(os.environ["TEAM_SOCKET_DIR"]) / (home.parent.name + ".sock"))
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    # Test transport only: readiness is announced after this file is published.
    Path(address).write_text(json.dumps(listener.getsockname()), encoding="utf-8")
    processes = {}
    next_handle = 0
    mutex = threading.Lock()

    def handle(connection):
        nonlocal next_handle
        with connection, connection.makefile("rwb") as stream:
            try:
                request = json.loads(stream.readline())
                launcher_result = {}
                operation = request.get("operation", "command")
                if operation == "auth_probe":
                    import struct
                    control = json.loads((home.parent / "control.json").read_text(encoding="utf-8"))
                    endpoint = control["authentication"]["endpoint"]
                    from native_fixture import raw_authentication
                    with raw_authentication(endpoint) as client:
                        payload = json.dumps(request["claim"]).encode()
                        client.write(struct.pack("!I", len(payload)) + payload)
                        result = {"accepted":client.read(1) == b"\1"}
                elif operation in ("finish", "capacity_failure"):
                    complete_after_ack(connection, stream, operation, complete)
                    return
                elif operation == "identity":
                    result = {"pid": os.getpid(), "ppid": os.getppid()}
                elif operation == "close_output":
                    process = processes[request["handle"]]
                    for name in request["streams"]:
                        assert name in ("stdout", "stderr")
                        getattr(process, name).close()
                        setattr(process, name, None)
                    result = {"returncode": process.poll()}
                elif operation == "wait":
                    process = processes[request["handle"]]
                    stdout, stderr = process.communicate(timeout=20)
                    result = {"returncode":process.returncode, "stdout":stdout, "stderr":stderr}
                else:
                    env = dict(shell)
                    for key, value in request.get("env", {}).items():
                        if value is None:
                            env.pop(key, None)
                        else:
                            env[key] = value
                    command = [request.get("launcher", os.environ["TEAM_SUBTEAM_BIN"]), *request["args"]]
                    if "launcher" in request:
                        # Report only the selected executable, never the shell environment.
                        launcher_result["launcher"] = str(Path(shutil.which(command[0], path=env.get("PATH"))).resolve())
                    if operation == "broken_output":
                        process = subprocess.Popen(command, cwd=state["cwd"], env=env, text=True, encoding="utf-8",
                                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                        process.stderr.close()
                        result = {"returncode":process.wait(timeout=20)}
                    elif operation == "start":
                        process = subprocess.Popen(command, cwd=state["cwd"], env=env, text=True, encoding="utf-8",
                                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                                   pass_fds=tuple(json.loads(os.environ.get("TEAM_FIXTURE_PASS_FDS", "[]"))))
                        # Read the launcher's active notice, not file/socket polling.
                        notices = []
                        while line := process.stderr.readline():
                            notices.append(line)
                            if "pira_team active:" in line or (request.get("until") == "run_id" and "pira_team run_id:" in line):
                                run_id = next(line.split(": ", 1)[1].strip() for line in notices if "pira_team run_id:" in line)
                                break
                        else:
                            stdout, stderr = process.communicate(timeout=5)
                            result = {"returncode":process.returncode, "stdout":stdout, "stderr":''.join(notices) + stderr}
                            result.update(launcher_result)
                            stream.write(json.dumps(result).encode() + b"\n")
                            stream.flush()
                            return
                        with mutex:
                            next_handle += 1
                            processes[next_handle] = process
                            result = {"handle":next_handle, "run_id":run_id, "pid":process.pid}
                    else:
                        process = subprocess.run(command, cwd=state["cwd"], env=env, text=True, encoding="utf-8",
                                                 capture_output=True, timeout=20)
                        result = {"returncode":process.returncode, "stdout":process.stdout, "stderr":process.stderr}
                result.update(launcher_result)
                stream.write(json.dumps(result).encode() + b"\n")
                stream.flush()
            except Exception as error:
                stream.write(json.dumps({"fixture_error":str(error)}).encode() + b"\n")
                stream.flush()

    def serve():
        while True:
            connection, _ = listener.accept()
            threading.Thread(target=handle, args=(connection,), daemon=True).start()

    threading.Thread(target=serve, daemon=True).start()
    return listener
