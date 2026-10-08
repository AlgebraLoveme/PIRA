import hashlib
import json
import os
import pathlib
import struct
import subprocess
import sys
import tempfile
import time

binary, mode = sys.argv[1:]

with tempfile.TemporaryDirectory(prefix="ctx-native-cwd-") as temporary:
    root = pathlib.Path(temporary).resolve()
    store = root / "store"
    ordinary = root / "ordinary"
    sibling = root / "workspace-\ufffd"
    ordinary.mkdir()
    sibling.mkdir()

    def run(*args, cwd=ordinary):
        return subprocess.run(
            [binary, args[0], "--store-dir", str(store), *args[1:]],
            cwd=cwd, capture_output=True, timeout=15,
        )

    def command(path, cwd=ordinary):
        result = run("command", str(path), cwd=cwd)
        assert result.returncode == 0, result
        return json.loads(result.stdout)

    def legacy_copy(source, cwd, name, native=None, compatibility=4):
        data = source.read_bytes()
        assert data[:8] == b"PIRACTX4"
        size = struct.unpack_from("<Q", data, 16)[0]
        metadata = json.loads(data[192:192+size])
        metadata.pop("cwd_native", None)
        if native is not None:
            metadata["cwd_native"] = native
        metadata["cwd"] = str(cwd)
        metadata["compat_version"] = compatibility
        encoded = json.dumps(metadata, separators=(",", ":")).encode()
        header = bytearray(data[:192])
        struct.pack_into("<Q", header, 16, len(encoded))
        header[64:96] = hashlib.sha256(encoded).digest()
        path = root / name
        path.write_bytes(header + encoded + data[192+size:])
        return path

    if mode == "legacy":
        result = run("capture", "--intent", "Legacy cwd fixture", "--", sys.executable, "-c", "print('ok')")
        assert result.returncode == 0, result
        capture = next(store.glob("*.piractx"))
        record = command(capture)
        assert record["exact"] and record["cwd_native"]["encoding"] == "utf8", record
        assert os.path.samefile(record["cwd_native"]["value"], ordinary), record
        assert record["cwd"] == record["cwd_native"]["value"], record
        legacy = legacy_copy(capture, ordinary.resolve(), "legacy.piractx")
        assert command(legacy)["exact"]
        ambiguous = legacy_copy(capture, sibling.resolve(), "ambiguous.piractx")
        record = command(ambiguous)
        assert not record["exact"] and record["cwd_native"] is None, record
        assert run("verify", str(ambiguous)).returncode == 0
        # Exercise native-only format reading on every host without inventing filesystem bytes.
        encoded_native = {"encoding": "unix_bytes", "value": list(b"/raw-\xff")}
        native_record = legacy_copy(capture, "/raw-\ufffd", "native-only.piractx", encoded_native, 7)
        record = command(native_record)
        assert not record["exact"] and record["cwd_native"] == encoded_native, record
        assert run("verify", str(native_record)).returncode == 0
        missing_native = legacy_copy(capture, "/raw-\ufffd", "missing-native.piractx", compatibility=7)
        assert run("command", str(missing_native)).returncode == 125


        probe = "import os,sys; print(os.getcwdb().hex(),flush=True); sys.exit(75)"
        result = run("watch", "--deadline", "30s", "--sample-every", "1s", "--review-after", "1s", "--", sys.executable, "-c", probe)
        assert result.returncode == 10, result
        state_path = next((store / "watch/state").glob("*.json"))
        state = json.loads(state_path.read_text())
        state.pop("source_cwd_native")
        state_path.write_text(json.dumps(state))
        result = run("watch", state["id"], "--review-after", "1s")
        assert result.returncode == 10 and os.fsencode(ordinary.resolve()).hex().encode() in result.stdout.replace(b"\n", b""), result
        state = json.loads(state_path.read_text())
        state["source_cwd"] = str(sibling.resolve())
        marker = root / "wrong-directory-executed"
        state["source"] = [sys.executable, "-c", "import pathlib; pathlib.Path(" + repr(str(marker)) + ").write_text('wrong')"]
        original = json.dumps(state).encode()
        state_path.write_bytes(original)
        result = run("watch", state["id"], "--review-after", "1s")
        assert result.returncode == 125 and b"ambiguous legacy cwd" in result.stderr, result
        assert not marker.exists() and state_path.read_bytes() == original
        assert run("watch", state["id"], "--latest").returncode == 0
        # New explicit Unicode paths containing U+FFFD are not ambiguous.
        result = run("watch", "--deadline", "3s", "--", sys.executable, "-c", "import os; print(os.getcwdb().hex())", cwd=sibling)
        assert result.returncode == 0 and os.fsencode(sibling.resolve()).hex().encode() in result.stdout.replace(b"\n", b""), result
    else:
        native = os.fsencode(root) + (b"/workspace-\xff" if mode == "native" else b"/live-workspace")
        os.mkdir(native)
        expected = native.hex().encode()
        env = dict(os.environ, PIRA_CTX_LIVE_CHECKPOINT_MS="100")
        producer = "import os,sys; print(os.getcwdb().hex(),flush=True); sys.stdin.buffer.read(1)"
        process = subprocess.Popen(
            [binary, "capture", "--store-dir", str(store), "--intent", "Native cwd fixture", "--", sys.executable, "-c", producer],
            cwd=native, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            deadline = time.monotonic()+10
            while True:
                files = list((store / "live").glob("*.json"))
                if files:
                    manifest = json.loads(files[0].read_text())
                    if manifest["metadata"]["stdout_bytes"]: break
                assert process.poll() is None and time.monotonic()<deadline
                time.sleep(.02)
            result_id = manifest["metadata"]["result_id"]
            # Live ID lookup is workspace-scoped, unlike completed full IDs.
            # An explicit manifest path also permits reconstruction from another cwd.
            record = command(files[0])
            assert command(result_id, cwd=native) == record
            assert process.poll() is None
            expected_cwd = {"encoding": "unix_bytes", "value": list(native)} if mode == "native" else {"encoding": "utf8", "value": os.fsdecode(native)}
            assert record["exact"] == (mode != "native") and record["cwd_native"] == expected_cwd, record
            assert manifest["schema"] == (3 if mode == "native" else 1)
            assert manifest["metadata"]["compat_version"] == (7 if mode == "native" else 4)
            process.communicate(b"x", timeout=10)
            assert process.returncode == 0
        finally:
            if process.poll() is None: process.kill(); process.communicate(timeout=10)
        assert command(result_id) == record
        raw = run("raw", result_id, "--stdout")
        assert raw.returncode == 0 and raw.stdout.strip() == expected, raw
        assert run("verify", result_id).returncode == 0
        if mode == "live":
            sys.exit(0)
        # Probe and analyzer must both use the raw-byte cwd, including after resume.
        analyzer = "import os,json; print(json.dumps({'progress':os.getcwdb().hex()}))"
        result = run("watch", "--deadline", "30s", "--sample-every", "1s", "--review-after", "1s", "--analyzer-code", analyzer, "--", sys.executable, "-c", "import os,sys; print(os.getcwdb().hex(),flush=True); sys.exit(75)", cwd=native)
        assert result.returncode == 10 and expected in result.stdout.replace(b"\n", b""), result
        state_path = next((store / "watch/state").glob("*.json"))
        state = json.loads(state_path.read_text())
        assert state["schema"] == 3 and state["progress"].encode() == expected, state
        assert expected.decode() in state["visible_stdout"].replace("\n", ""), state
        assert state["source_cwd_native"]["value"] == list(native), state
        result = run("watch", state["id"], "--review-after", "1s")
        assert result.returncode == 10 and expected in result.stdout.replace(b"\n", b""), result
        assert json.loads(state_path.read_text())["progress"].encode() == expected
